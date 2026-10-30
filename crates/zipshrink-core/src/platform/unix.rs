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
        readonly: m.mode() & 0o200 == 0,
        unix_mode: Some(m.mode()),
        dos_attrs: if m.mode() & 0o200 == 0 { 0x01 } else { 0 },
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
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::fd::AsRawFd;
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
    Ok((f, stat_from_metadata(&m)))
}

pub fn sync_file(f: &File) -> io::Result<()> {
    // On macOS std uses F_FULLFSYNC, which flushes the drive cache as well.
    f.sync_all()
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
    let d = File::open(dir)?;
    match d.sync_all() {
        Ok(()) => Ok(()),
        // Some filesystems (and FUSE mounts) refuse fsync on directories.
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) || e.raw_os_error() == Some(libc::ENOTSUP) => Ok(()),
        Err(e) => Err(e),
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
        if e.raw_os_error() != Some(libc::ENOTSUP) {
            return Err(e);
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
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
}

/// Reads `[offset, offset + len)` of the archive for verification, after
/// evicting those pages from the cache so the data comes from the device.
pub fn read_range_uncached(
    f: &File,
    _path: &Path,
    offset: u64,
    len: u64,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<ReadbackMode> {
    let mode = evict_cache(f, offset, len);
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
        let _ = (offset, len);
        let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) };
        if r != -1 {
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
pub fn delete_verified(path: &Path, expected: &FileStat, sha: Option<&[u8; 32]>) -> io::Result<DeleteOutcome> {
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
    fs::remove_file(path)?;
    Ok(DeleteOutcome::Deleted)
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
