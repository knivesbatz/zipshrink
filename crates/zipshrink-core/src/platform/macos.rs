//! macOS specifics used by `unix.rs`: file flags, extended attributes,
//! cache eviction, flush fallbacks and filesystem names.
//!
//! The pure helpers at the top are also compiled (and unit tested) on other
//! Unix systems; everything that calls into the kernel is macOS only.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use super::Kind;

/// `st_flags` bit for a file whose data lives only in the cloud (iCloud
/// Drive "Optimize Mac Storage", File Provider). Reading it would start a
/// download, so it is never archived or deleted.
pub const SF_DATALESS: u32 = 0x4000_0000;
const UF_IMMUTABLE: u32 = 0x0000_0002;
const UF_APPEND: u32 = 0x0000_0004;
const SF_IMMUTABLE: u32 = 0x0002_0000;
const SF_APPEND: u32 = 0x0004_0000;
/// Locks the owner can clear (Finder's "Locked" is `uchg`).
pub const USER_LOCKS: u32 = UF_IMMUTABLE | UF_APPEND;
/// Locks only the superuser can clear (`schg`, `sappnd`).
pub const SYSTEM_LOCKS: u32 = SF_IMMUTABLE | SF_APPEND;

/// Whether the flags make the file impossible to modify or delete as is.
pub fn is_locked(flags: u32) -> bool {
    flags & (USER_LOCKS | SYSTEM_LOCKS) != 0
}

/// `Kind::Other` for cloud-only items, which are refused.
pub fn dataless_kind(flags: u32, is_dir: bool) -> Option<Kind> {
    if flags & SF_DATALESS == 0 {
        return None;
    }
    Some(Kind::Other(if is_dir { "cloud-only folder that is not downloaded" } else { "cloud-only file that is not downloaded" }))
}

/// Extended attributes that can be lost without losing anything the user
/// made: download provenance, privacy bookkeeping and caches.
const DISPOSABLE_XATTRS: &[&str] = &[
    "com.apple.quarantine",
    "com.apple.provenance",
    "com.apple.macl",
    "com.apple.lastuseddate#PS",
    "com.apple.metadata:kMDItemWhereFroms",
    "kMDItemWhereFroms",
    "com.apple.metadata:kMDItemDownloadedDate",
    "kMDItemDownloadedDate",
    "com.apple.TextEncoding",
];

const FINDER_INFO: &str = "com.apple.FinderInfo";

fn describe_xattr(name: &[u8]) -> &'static str {
    match name {
        b"com.apple.metadata:_kMDItemUserTags" => "Finder tags",
        b"com.apple.ResourceFork" => "a resource fork",
        b"com.apple.metadata:kMDItemFinderComment" => "a Finder comment",
        b"com.apple.FinderInfo" => "Finder information (such as a colour label or custom icon)",
        _ => "extended attributes",
    }
}

/// Given the NUL-separated list returned by `listxattr`, returns what the
/// first attribute a ZIP cannot hold is, if any. `get` reads an attribute's
/// value (only needed for `com.apple.FinderInfo`, which is harmless when it
/// is all zeros); `None` means it could not be read and counts as data.
pub fn blocking_xattr(list: &[u8], mut get: impl FnMut(&[u8]) -> Option<Vec<u8>>) -> Option<&'static str> {
    for name in list.split(|&b| b == 0).filter(|n| !n.is_empty()) {
        if DISPOSABLE_XATTRS.iter().any(|d| d.as_bytes() == name) {
            continue;
        }
        if name == FINDER_INFO.as_bytes() && get(name).is_some_and(|v| v.iter().all(|&b| b == 0)) {
            continue;
        }
        return Some(describe_xattr(name));
    }
    None
}

/// Reason given when a file or folder is kept for its metadata.
pub fn metadata_reason(what: &str) -> String {
    format!("it has {what}, macOS metadata that a ZIP cannot hold")
}

/// Display name for a `statfs` `f_fstypename`.
pub fn fs_display_name(fstypename: &str) -> String {
    match fstypename {
        "apfs" => "APFS".into(),
        "hfs" => "HFS+".into(),
        "msdos" => "FAT".into(),
        "exfat" => "exFAT".into(),
        "smbfs" => "SMB".into(),
        "nfs" => "NFS".into(),
        "afpfs" => "AFP".into(),
        "webdav" => "WebDAV".into(),
        n if n.to_ascii_lowercase().contains("ntfs") => "NTFS".into(),
        n => n.to_string(),
    }
}

/// Reason given when macOS answers EPERM ("Operation not permitted"):
/// usually privacy protection (Files and Folders, Full Disk Access) or a
/// locked parent folder.
pub const EPERM_HINT: &str = "macOS did not allow it (operation not permitted); check that ZipShrink is allowed in System Settings > Privacy & Security > Files and Folders (or Full Disk Access), and that the folder is not locked";

#[cfg(target_os = "macos")]
pub use sys::*;

#[cfg(target_os = "macos")]
mod sys {
    use super::{SYSTEM_LOCKS, USER_LOCKS};
    use crate::platform::DeleteOutcome;
    use std::ffi::{CStr, CString};
    use std::fs::{self, File, Metadata, OpenOptions};
    use std::io;
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::macos::fs::MetadataExt as _;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::sync::Mutex;

    pub fn flags(m: &Metadata) -> u32 {
        m.st_flags()
    }

    fn cpath(path: &Path) -> io::Result<CString> {
        Ok(CString::new(path.as_os_str().as_bytes())?)
    }

    /// How far a sync got.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SyncLevel {
        /// `F_FULLFSYNC`: data reached the medium, drive cache flushed.
        Full,
        /// `F_BARRIERFSYNC`: ordered against later writes, drive cache not flushed.
        Barrier,
        /// Plain `fsync`: handed to the drive, drive cache not flushed.
        Plain,
    }

    fn unsupported(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(c) if c == libc::EINVAL || c == libc::ENOTSUP || c == libc::EOPNOTSUPP || c == libc::ENOTTY)
    }

    /// `F_FULLFSYNC`, falling back to `F_BARRIERFSYNC` and then `fsync`
    /// on filesystems that do not support the stronger request (SMB, FUSE,
    /// some exFAT drivers, and APFS for directories).
    pub fn full_sync(fd: RawFd) -> io::Result<SyncLevel> {
        if unsafe { libc::fcntl(fd, libc::F_FULLFSYNC) } != -1 {
            return Ok(SyncLevel::Full);
        }
        let e = io::Error::last_os_error();
        if !unsupported(&e) {
            return Err(e);
        }
        if unsafe { libc::fcntl(fd, libc::F_BARRIERFSYNC) } != -1 {
            return Ok(SyncLevel::Barrier);
        }
        let e = io::Error::last_os_error();
        if !unsupported(&e) {
            return Err(e);
        }
        if unsafe { libc::fsync(fd) } == 0 {
            return Ok(SyncLevel::Plain);
        }
        Err(io::Error::last_os_error())
    }

    /// Volumes (st_dev) on which a file sync could not flush the drive cache.
    static WEAK_VOLUMES: Mutex<Vec<u64>> = Mutex::new(Vec::new());

    pub fn note_weak_sync(f: &File) {
        if let Ok(m) = f.metadata() {
            let mut v = WEAK_VOLUMES.lock().unwrap_or_else(|p| p.into_inner());
            if !v.contains(&m.dev()) {
                v.push(m.dev());
            }
        }
    }

    pub fn drive_cache_not_flushed(path: &Path) -> bool {
        match fs::metadata(path) {
            Ok(m) => WEAK_VOLUMES.lock().unwrap_or_else(|p| p.into_inner()).contains(&m.dev()),
            Err(_) => false,
        }
    }

    /// Drops the cached pages of `[offset, offset + len)` (already synced)
    /// from the unified buffer cache: maps the page-aligned range shared and
    /// read-only, then `msync(MS_SYNC | MS_INVALIDATE)`. The cache is per
    /// file, so this applies to every handle of the file. False when any
    /// step failed, so the caller can report the read-back as cached.
    pub fn invalidate(fd: RawFd, offset: u64, len: u64) -> bool {
        if len == 0 {
            return true;
        }
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0 {
            return false;
        }
        let page = page as u64;
        let Some(end) = offset.checked_add(len) else { return false };
        // A multiple of every page size (4 KiB and 16 KiB).
        const CHUNK: u64 = 256 << 20;
        let mut pos = offset / page * page;
        while pos < end {
            let n = (end - pos).min(CHUNK) as usize;
            let Ok(off) = libc::off_t::try_from(pos) else { return false };
            let p = unsafe { libc::mmap(std::ptr::null_mut(), n, libc::PROT_READ, libc::MAP_SHARED, fd, off) };
            if p == libc::MAP_FAILED {
                return false;
            }
            let r = unsafe { libc::msync(p, n, libc::MS_SYNC | libc::MS_INVALIDATE) };
            unsafe { libc::munmap(p, n) };
            if r != 0 {
                return false;
            }
            pos += n as u64;
        }
        true
    }

    /// A second read-only handle on the same file with `F_NOCACHE` set, so
    /// verification reads do not repopulate the cache. The flag is never
    /// set on the caller's handle (it would stay on the archive's write
    /// handle for the rest of the job). None when `path` no longer names
    /// the file behind `f` or the flag is refused.
    pub fn open_nocache(path: &Path, f: &File) -> Option<File> {
        let g = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path).ok()?;
        let (a, b) = (f.metadata().ok()?, g.metadata().ok()?);
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return None;
        }
        if unsafe { libc::fcntl(g.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
            return None;
        }
        Some(g)
    }

    fn list_xattrs(list: impl Fn(*mut libc::c_char, usize) -> libc::ssize_t) -> io::Result<Vec<u8>> {
        loop {
            let n = list(std::ptr::null_mut(), 0);
            if n < 0 {
                let e = io::Error::last_os_error();
                // No extended attribute support at all: nothing to lose.
                if matches!(e.raw_os_error(), Some(c) if c == libc::ENOTSUP || c == libc::EOPNOTSUPP) {
                    return Ok(Vec::new());
                }
                return Err(e);
            }
            if n == 0 {
                return Ok(Vec::new());
            }
            let mut buf = vec![0u8; n as usize];
            let m = list(buf.as_mut_ptr().cast(), buf.len());
            if m >= 0 {
                buf.truncate(m as usize);
                return Ok(buf);
            }
            let e = io::Error::last_os_error();
            // The list grew in between: ask for its size again.
            if e.raw_os_error() != Some(libc::ERANGE) {
                return Err(e);
            }
        }
    }

    fn blocking(
        list: impl Fn(*mut libc::c_char, usize) -> libc::ssize_t,
        get: impl Fn(*const libc::c_char, *mut libc::c_void, usize) -> libc::ssize_t,
    ) -> io::Result<Option<&'static str>> {
        let names = list_xattrs(list)?;
        Ok(super::blocking_xattr(&names, |name| {
            let name = CString::new(name).ok()?;
            let mut v = vec![0u8; 64];
            let n = get(name.as_ptr(), v.as_mut_ptr().cast(), v.len());
            if n < 0 {
                return None;
            }
            v.truncate(n as usize);
            Some(v)
        }))
    }

    /// What the file behind `fd` carries that a ZIP cannot hold, if
    /// anything (see [`super::blocking_xattr`]).
    pub fn fd_blocking_xattr(fd: RawFd) -> io::Result<Option<&'static str>> {
        blocking(
            |buf, size| unsafe { libc::flistxattr(fd, buf, size, 0) },
            |name, value, size| unsafe { libc::fgetxattr(fd, name, value, size, 0, 0) },
        )
    }

    fn path_blocking_xattr(path: &Path) -> io::Result<Option<&'static str>> {
        let c = cpath(path)?;
        blocking(
            |buf, size| unsafe { libc::listxattr(c.as_ptr(), buf, size, libc::XATTR_NOFOLLOW) },
            |name, value, size| unsafe { libc::getxattr(c.as_ptr(), name, value, size, 0, libc::XATTR_NOFOLLOW) },
        )
    }

    /// Unlinks a file that passed every check, clearing a Finder lock
    /// (`uchg`, `uappnd`) first when read-only files may be deleted and
    /// putting it back if the unlink fails. System locks keep the file.
    pub fn remove_file_unlocking(f: &File, path: &Path, allow_readonly: bool) -> io::Result<DeleteOutcome> {
        let flags = f.metadata()?.st_flags();
        if flags & SYSTEM_LOCKS != 0 {
            return Ok(DeleteOutcome::Kept(
                "kept because it is locked by the system (schg or sappnd flag), which only an administrator can clear".into(),
            ));
        }
        let user = flags & USER_LOCKS;
        if user != 0 {
            if !allow_readonly {
                return Ok(DeleteOutcome::Kept("kept because it is locked (read-only)".into()));
            }
            if unsafe { libc::fchflags(f.as_raw_fd(), flags & !user) } != 0 {
                let e = io::Error::last_os_error();
                return Ok(DeleteOutcome::Kept(format!("kept because it is locked and the lock could not be removed: {e}")));
            }
        }
        match fs::remove_file(path) {
            Ok(()) => Ok(DeleteOutcome::Deleted),
            Err(e) => {
                if user != 0 {
                    unsafe { libc::fchflags(f.as_raw_fd(), flags) };
                }
                if e.raw_os_error() == Some(libc::EPERM) {
                    return Ok(DeleteOutcome::Kept(format!("kept because {}", super::EPERM_HINT)));
                }
                Err(e)
            }
        }
    }

    /// Why the folder at `path` (already identified as the scanned one)
    /// must be kept for metadata a ZIP cannot hold. None when it has none,
    /// or when it is not empty and so is kept anyway.
    pub fn dir_kept_reason(path: &Path) -> io::Result<Option<String>> {
        let Some(what) = path_blocking_xattr(path)? else { return Ok(None) };
        if fs::read_dir(path).is_ok_and(|mut d| d.next().is_some()) {
            return Ok(None);
        }
        Ok(Some(super::metadata_reason(what)))
    }

    fn statfs(path: &Path) -> io::Result<libc::statfs> {
        let c = cpath(path)?;
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(s)
    }

    pub fn fs_name(path: &Path) -> Option<String> {
        let s = statfs(path).ok()?;
        let name = unsafe { CStr::from_ptr(s.f_fstypename.as_ptr()) };
        let name = name.to_str().ok()?;
        (!name.is_empty()).then(|| super::fs_display_name(name))
    }

    /// Whether `path` (canonical) is where a volume is mounted. APFS
    /// volume groups firmlink the Data volume into the System volume, so a
    /// device number comparison with the parent is not enough: `statfs`
    /// names the mount point of the volume holding the path.
    pub fn is_mount_point(path: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        if let Ok(s) = statfs(path) {
            let on = unsafe { CStr::from_ptr(s.f_mntonname.as_ptr()) };
            if on.to_bytes() == path.as_os_str().as_encoded_bytes() {
                return true;
            }
        }
        let Some(parent) = path.parent() else { return true };
        match (fs::symlink_metadata(path), fs::metadata(parent)) {
            (Ok(a), Ok(b)) => a.dev() != b.dev(),
            _ => false,
        }
    }

    /// `statfs` keeps 64-bit block counts (`statvfs` on macOS has 32-bit ones).
    pub fn available_space(path: &Path) -> io::Result<u64> {
        let s = statfs(path)?;
        Ok(s.f_bavail.saturating_mul(u64::from(s.f_bsize)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataless_items_are_refused() {
        assert_eq!(dataless_kind(0, false), None);
        assert_eq!(dataless_kind(USER_LOCKS, false), None);
        assert!(matches!(dataless_kind(SF_DATALESS, false), Some(Kind::Other(s)) if s.contains("cloud-only file")));
        assert!(matches!(dataless_kind(SF_DATALESS | UF_IMMUTABLE, true), Some(Kind::Other(s)) if s.contains("cloud-only folder")));
    }

    #[test]
    fn locks_count_as_read_only() {
        assert!(!is_locked(0));
        assert!(!is_locked(SF_DATALESS));
        for f in [UF_IMMUTABLE, UF_APPEND, SF_IMMUTABLE, SF_APPEND] {
            assert!(is_locked(f), "{f:#x}");
        }
    }

    #[test]
    fn only_disposable_xattrs_allow_deletion() {
        let none = |_: &[u8]| -> Option<Vec<u8>> { panic!("value not needed") };
        assert_eq!(blocking_xattr(b"", none), None);
        assert_eq!(blocking_xattr(b"com.apple.quarantine\0com.apple.provenance\0com.apple.macl\0", none), None);
        assert_eq!(blocking_xattr(b"com.apple.metadata:kMDItemWhereFroms\0com.apple.lastuseddate#PS\0", none), None);
        assert_eq!(blocking_xattr(b"com.apple.quarantine\0com.apple.metadata:_kMDItemUserTags\0", none), Some("Finder tags"));
        assert_eq!(blocking_xattr(b"com.apple.ResourceFork\0", none), Some("a resource fork"));
        assert_eq!(blocking_xattr(b"com.dropbox.attrs\0", none), Some("extended attributes"));
        // FinderInfo is harmless when all zero, and kept when unreadable.
        assert_eq!(blocking_xattr(b"com.apple.FinderInfo\0", |_| Some(vec![0; 32])), None);
        assert!(blocking_xattr(b"com.apple.FinderInfo\0", |_| Some({
            let mut v = vec![0; 32];
            v[9] = 0x0c;
            v
        }))
        .is_some());
        assert!(blocking_xattr(b"com.apple.FinderInfo\0", |_| None).is_some());
    }

    #[test]
    fn filesystem_names() {
        assert_eq!(fs_display_name("apfs"), "APFS");
        assert_eq!(fs_display_name("msdos"), "FAT");
        assert_eq!(fs_display_name("exfat"), "exFAT");
        assert_eq!(fs_display_name("ufsd_NTFS"), "NTFS");
        assert_eq!(fs_display_name("macfuse"), "macfuse");
    }
}
