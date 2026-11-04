use super::{verify_open_source, DeleteOutcome, FileStat, Kind, ReadbackMode};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

pub fn pwrite(f: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    f.write_at(buf, offset)
}

pub fn pread(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    f.read_at(buf, offset)
}

fn ts(secs: i64, nsec: i64) -> Option<(i64, u32)> {
    Some((secs, nsec.clamp(0, 999_999_999) as u32))
}

pub fn stat_from_metadata(m: &Metadata) -> FileStat {
    #[cfg(target_os = "macos")]
    let locked = super::macos::is_locked(super::macos::flags(m));
    #[cfg(not(target_os = "macos"))]
    let locked = false;
    // Finder's "Locked" (uchg) and the append-only flags count as read-only.
    let readonly = m.mode() & 0o200 == 0 || locked;
    let btime = m.created().ok().and_then(|t| match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => Some((d.as_secs() as i64, d.subsec_nanos())),
        Err(_) => None,
    });
    FileStat {
        size: m.len(),
        dev: m.dev(),
        ino: m.ino(),
        mtime: ts(m.mtime(), m.mtime_nsec()),
        atime: ts(m.atime(), m.atime_nsec()),
        btime,
        change: m.ctime().saturating_mul(1_000_000_000).saturating_add(m.ctime_nsec()),
        readonly,
        unix_mode: Some(m.mode()),
        dos_attrs: if readonly { 0x01 } else { 0 },
    }
}

pub fn stat_handle(f: &File) -> io::Result<FileStat> {
    Ok(stat_from_metadata(&f.metadata()?))
}

/// Classifies `path` without following links.
pub fn kind_of(path: &Path) -> io::Result<(Kind, Metadata)> {
    let m = fs::symlink_metadata(path)?;
    let t = m.file_type();
    let k = if t.is_symlink() {
        Kind::Link
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_file() {
        Kind::File
    } else {
        use std::os::unix::fs::FileTypeExt;
        Kind::Other(if t.is_fifo() {
            "named pipe"
        } else if t.is_socket() {
            "socket"
        } else if t.is_block_device() || t.is_char_device() {
            "device file"
        } else {
            "special file"
        })
    };
    #[cfg(target_os = "macos")]
    if matches!(k, Kind::File | Kind::Dir) {
        if let Some(cloud) = super::macos::dataless_kind(super::macos::flags(&m), k == Kind::Dir) {
            return Ok((cloud, m));
        }
    }
    Ok((k, m))
}

/// Opens a source file for compression without following symbolic links.
pub fn open_source(path: &Path) -> io::Result<(File, FileStat)> {
    let f =
        OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_NONBLOCK).open(path)?;
    let m = f.metadata()?;
    if !m.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    // Reading a cloud-only file would download it first.
    #[cfg(target_os = "macos")]
    if super::macos::flags(&m) & super::macos::SF_DATALESS != 0 {
        return Err(io::Error::other("it is a cloud-only file that is not downloaded"));
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::fd::AsRawFd;
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
    Ok((f, stat_from_metadata(&m)))
}

// Durability on macOS. Plain `fsync` hands data to the drive but does not
// flush the drive's own write cache; `F_FULLFSYNC` does. Directory syncs
// may end up as plain `fsync` (APFS rejects `F_FULLFSYNC` on directories),
// which is safe only because of this invariant: every directory change the
// engine relies on (creating the partial archive, the final rename) is on
// the volume of the archive and journal, and is followed by a file
// `sync_file` there (the next batch's archive sync, or the journal sync
// after the rename) before anything is deleted or the journal removed. A
// full flush of that later file flushes the whole drive cache, including
// the earlier directory changes. When `sync_file` itself has to fall back,
// that is recorded and the job's report says the drive cache was not
// flushed (`drive_cache_not_flushed`).
pub fn sync_file(f: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if super::macos::full_sync(f.as_raw_fd())? != super::macos::SyncLevel::Full {
            super::macos::note_weak_sync(f);
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    f.sync_all()
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let d = File::open(dir)?;
    // macOS: F_FULLFSYNC, then F_BARRIERFSYNC, then fsync (see above).
    #[cfg(target_os = "macos")]
    let r = super::macos::full_sync(d.as_raw_fd()).map(|_| ());
    #[cfg(not(target_os = "macos"))]
    let r = if unsafe { libc::fsync(d.as_raw_fd()) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) };
    let Err(e) = r else { return Ok(()) };
    match e.raw_os_error() {
        // Some filesystems (and FUSE mounts) do not support syncing directories.
        Some(c) if c == libc::EINVAL || c == libc::ENOTSUP || c == libc::ENOTTY || c == libc::EBADF || c == libc::EROFS => Ok(()),
        _ => Err(e),
    }
}

/// Renames `from` to `to`, failing if `to` exists.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(from.as_os_str().as_bytes())?;
    let b = CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    {
        let r = unsafe { libc::renameat2(libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), libc::RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(c) if c == libc::EINVAL || c == libc::ENOSYS || c == libc::ENOTSUP => {}
            _ => return Err(e),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let r = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_EXCL) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // Volumes without RENAME_EXCL answer ENOTSUP (or EINVAL on some
        // network and FUSE filesystems).
        match e.raw_os_error() {
            Some(c) if c == libc::ENOTSUP || c == libc::EOPNOTSUPP || c == libc::EINVAL => {}
            _ => return Err(e),
        }
    }
    // Filesystems without an atomic no-replace rename (FAT, some network
    // mounts): a hard link is atomic and fails if the target exists, and is
    // the fallback where links are supported; otherwise check then rename.
    let _ = (&a, &b);
    match fs::hard_link(from, to) {
        Ok(()) => fs::remove_file(from),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if fs::symlink_metadata(to).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"));
            }
            fs::rename(from, to)
        }
    }
}

pub fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

pub fn volume_id(path: &Path) -> io::Result<u64> {
    Ok(fs::metadata(path)?.dev())
}

/// Name of the filesystem holding `path`, when cheaply known.
pub fn fs_name(path: &Path) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let name = match s.f_type as i64 {
            0xEF53 => "ext4",
            0x9123683E => "btrfs",
            0x58465342 => "xfs",
            0x01021994 => "tmpfs",
            0x4d44 => "FAT",
            0x2011BAB0 => "exFAT",
            0x5346544e | 0x65735546 => "NTFS",
            0x6969 => "NFS",
            0xFF534D42 | 0xFE534D42 => "SMB",
            0x794c7630 => "overlayfs",
            0x2FC12FC1 => "zfs",
            0xF2F52010 => "f2fs",
            _ => return None,
        };
        Some(name.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::fs_name(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = path;
        None
    }
}

/// Reads `[offset, offset + len)` of the archive for verification, after
/// evicting those pages from the cache so the data comes from the device.
pub fn read_range_uncached(
    f: &File,
    path: &Path,
    offset: u64,
    len: u64,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<ReadbackMode> {
    let mode = evict_cache(f, offset, len);
    // macOS: read through a separate F_NOCACHE handle so the reads do not
    // refill the cache; the caller's handle is left as it was.
    #[cfg(target_os = "macos")]
    let nocache = super::macos::open_nocache(path, f);
    #[cfg(target_os = "macos")]
    let f = nocache.as_ref().unwrap_or(f);
    #[cfg(not(target_os = "macos"))]
    let _ = path;
    let mut buf = vec![0u8; 1 << 20];
    let mut pos = offset;
    let end = offset + len;
    while pos < end {
        let want = ((end - pos) as usize).min(buf.len());
        super::pread_exact(f, &mut buf[..want], pos)?;
        sink(&buf[..want])?;
        pos += want as u64;
    }
    Ok(mode)
}

/// Asks the kernel to drop cached pages of a range that is already synced.
/// The cache is per file, so this works through any handle of the file.
pub fn evict_cache(f: &File, offset: u64, len: u64) -> ReadbackMode {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let r = unsafe { libc::posix_fadvise(f.as_raw_fd(), offset as i64, len as i64, libc::POSIX_FADV_DONTNEED) };
        if r == 0 {
            ReadbackMode::CacheEvicted
        } else {
            ReadbackMode::Cached
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if super::macos::invalidate(f.as_raw_fd(), offset, len) {
            ReadbackMode::CacheEvicted
        } else {
            ReadbackMode::Cached
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (f, offset, len);
        ReadbackMode::Cached
    }
}

/// Deletes `path` only if it is still the exact file that was archived.
/// Read-only permissions do not stop unlinking on Unix, so `allow_readonly`
/// is only checked by the caller, except for macOS file locks (`uchg`),
/// which must be cleared first.
pub fn delete_verified(path: &Path, expected: &FileStat, sha: Option<&[u8; 32]>, allow_readonly: bool) -> io::Result<DeleteOutcome> {
    let f =
        match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_NONBLOCK).open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DeleteOutcome::Missing),
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Ok(DeleteOutcome::Kept("kept because it was replaced by a symbolic link".into()))
            }
            Err(e) => return Err(e),
        };
    let now = stat_handle(&f)?;
    if let Some(reason) = verify_open_source(&f, &now, expected, sha)? {
        return Ok(DeleteOutcome::Kept(reason));
    }
    // The hash pass must not have raced with a writer.
    let after = stat_handle(&f)?;
    if !after.same_content_identity(expected) {
        return Ok(DeleteOutcome::Kept(format!("kept because {} while it was being checked", expected.difference(&after))));
    }
    // The name must still point at the file we verified.
    let at_path = stat_from_metadata(&fs::symlink_metadata(path)?);
    if at_path.dev != after.dev || at_path.ino != after.ino {
        return Ok(DeleteOutcome::Kept("kept because the path now refers to a different file".into()));
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // Finder tags, comments, resource forks... are not in the archive.
        if let Some(what) = super::macos::fd_blocking_xattr(f.as_raw_fd())? {
            return Ok(DeleteOutcome::Kept(format!("kept because {}", super::macos::metadata_reason(what))));
        }
        super::macos::remove_file_unlocking(&f, path, allow_readonly)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = allow_readonly;
        fs::remove_file(path)?;
        Ok(DeleteOutcome::Deleted)
    }
}

/// Removes an empty directory if it is still the one that was scanned.
pub fn remove_dir_verified(path: &Path, expected: &FileStat) -> io::Result<bool> {
    let (k, m) = match kind_of(path) {
        Ok(x) => x,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let s = stat_from_metadata(&m);
    if k != Kind::Dir || s.dev != expected.dev || s.ino != expected.ino {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    if let Some(reason) = super::macos::dir_kept_reason(path)? {
        return Err(io::Error::other(reason));
    }
    match fs::remove_dir(path) {
        Ok(()) => Ok(true),
        Err(e)
            if e.kind() == io::ErrorKind::DirectoryNotEmpty
                || e.raw_os_error() == Some(libc::ENOTEMPTY)
                || e.raw_os_error() == Some(libc::EEXIST) =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

pub fn is_sharing_violation(_e: &io::Error) -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn page() -> usize {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    /// Pages of the first `len` bytes of `f` that are in memory, seen
    /// through a fresh mapping.
    fn resident_pages(f: &File, len: usize) -> usize {
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, f.as_raw_fd(), 0) };
        assert_ne!(p, libc::MAP_FAILED, "{}", io::Error::last_os_error());
        let mut v = vec![0 as libc::c_char; len.div_ceil(page())];
        let r = unsafe { libc::mincore(p.cast_const(), len, v.as_mut_ptr()) };
        let e = io::Error::last_os_error();
        unsafe { libc::munmap(p, len) };
        assert_eq!(r, 0, "mincore: {e}");
        v.iter().filter(|&&b| i32::from(b) & libc::MINCORE_INCORE != 0).count()
    }

    fn written_file(dir: &Path, len: usize) -> (File, std::path::PathBuf, Vec<u8>) {
        let path = dir.join("readback.bin");
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let f = OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        f.write_all_at(&data, 0).unwrap();
        sync_file(&f).unwrap();
        (f, path, data)
    }

    #[test]
    fn evict_cache_drops_resident_pages() {
        let t = tempfile::tempdir().unwrap();
        let len = 8 << 20;
        let (f, path, data) = written_file(t.path(), len);
        let mut buf = vec![0u8; len];
        f.read_exact_at(&mut buf, 0).unwrap();
        let before = resident_pages(&f, len);
        assert!(before > 0, "the pages just read should be cached");
        assert_eq!(evict_cache(&f, 0, len as u64), ReadbackMode::CacheEvicted);
        let after = resident_pages(&f, len);
        eprintln!("ZIPSHRINK_EVICT resident pages: {before} before eviction, {after} after ({} total)", len / page());
        assert_eq!(after, 0, "msync(MS_INVALIDATE) left pages in the cache; evict_cache must report Cached on this system");
        let mut got = Vec::with_capacity(len);
        let mode = read_range_uncached(&f, &path, 0, len as u64, &mut |c| {
            got.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
        assert_eq!(mode, ReadbackMode::CacheEvicted);
        assert!(got == data, "read-back differs");
        eprintln!("ZIPSHRINK_EVICT resident pages after the F_NOCACHE read-back: {}", resident_pages(&f, len));
    }

    #[test]
    fn readback_leaves_the_callers_handle_cacheable() {
        // FNOCACHE as kept in the open file's flags, which F_GETFL returns.
        const FNOCACHE: i32 = 0x40000;
        let t = tempfile::tempdir().unwrap();
        let (f, path, _) = written_file(t.path(), 1 << 20);
        read_range_uncached(&f, &path, 4096, 100_000, &mut |_| Ok(())).unwrap();
        let probe = File::open(&path).unwrap();
        assert_ne!(unsafe { libc::fcntl(probe.as_raw_fd(), libc::F_NOCACHE, 1) }, -1);
        if unsafe { libc::fcntl(probe.as_raw_fd(), libc::F_GETFL) } & FNOCACHE == 0 {
            eprintln!("F_GETFL does not show F_NOCACHE on this system; probe skipped");
            return;
        }
        assert_eq!(unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFL) } & FNOCACHE, 0, "F_NOCACHE leaked onto the caller's handle");
    }

    /// Logs which flush each volume accepts. Extra folders (for example on
    /// FAT or exFAT disk images) can be listed in ZIPSHRINK_SYNC_PROBE.
    #[test]
    fn log_sync_levels() {
        let t = tempfile::tempdir().unwrap();
        let mut dirs = vec![t.path().to_path_buf()];
        if let Some(extra) = std::env::var_os("ZIPSHRINK_SYNC_PROBE") {
            dirs.extend(std::env::split_paths(&extra));
        }
        for d in dirs {
            let p = d.join(".zipshrink-sync-probe");
            let f = File::create(&p).unwrap();
            f.write_all_at(b"probe", 0).unwrap();
            let file = super::super::macos::full_sync(f.as_raw_fd());
            let dir = super::super::macos::full_sync(File::open(&d).unwrap().as_raw_fd());
            eprintln!("ZIPSHRINK_SYNC {} [{:?}]: file {file:?}, folder {dir:?}", d.display(), fs_name(&d));
            drop(f);
            fs::remove_file(&p).unwrap();
            file.unwrap();
            sync_dir(&d).unwrap();
        }
    }

    #[test]
    fn fs_name_of_the_temp_folder_is_apfs() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(fs_name(t.path()).as_deref(), Some("APFS"));
        assert!(super::super::available_space(t.path()).unwrap() > 0);
    }
}
