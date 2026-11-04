//! Operating-system specific file handling.
//!
//! Everything that touches durability, identity or deletion lives here so
//! the rules are in one place:
//!
//! * sources are opened without following symbolic links or reparse points;
//! * on Windows, sources are opened with `FILE_SHARE_READ` only, so no other
//!   program can write, rename or delete them while they are read;
//! * deletion is tied to the exact file object that was compressed (same
//!   volume, same file id, same size, same modification and change times,
//!   and by default the same SHA-256), and on Windows is performed through
//!   the very handle that was verified;
//! * verification reads bypass or evict the OS cache where possible, so
//!   data is checked as it is on the storage device.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

/// What a path is, without following links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    /// A symbolic link, junction, mount point or other reparse point.
    Link,
    /// Device, FIFO, socket, cloud placeholder or anything else we refuse.
    Other(&'static str),
}

/// Identity and metadata of a file at one point in time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileStat {
    pub size: u64,
    /// Volume or device id.
    pub dev: u64,
    /// Inode number or NTFS file index.
    pub ino: u64,
    /// Modification time (seconds, nanoseconds).
    pub mtime: Option<(i64, u32)>,
    pub atime: Option<(i64, u32)>,
    pub btime: Option<(i64, u32)>,
    /// Status-change time (Unix ctime, Windows ChangeTime) as an opaque
    /// tick count. Any write, rename, chmod or link change moves it.
    pub change: i64,
    pub readonly: bool,
    pub unix_mode: Option<u32>,
    pub dos_attrs: u8,
}

impl FileStat {
    /// True when nothing observable about the file's identity or content
    /// metadata differs.
    pub fn same_content_identity(&self, other: &FileStat) -> bool {
        self.size == other.size
            && self.dev == other.dev
            && self.ino == other.ino
            && self.mtime == other.mtime
            && self.change == other.change
    }

    /// Describes the first difference, for messages.
    pub fn difference(&self, now: &FileStat) -> &'static str {
        if self.dev != now.dev || self.ino != now.ino {
            "it was replaced by a different file"
        } else if self.size != now.size {
            "its size changed"
        } else if self.mtime != now.mtime {
            "its modification time changed"
        } else if self.change != now.change {
            "it was modified (its change time moved)"
        } else {
            "it changed"
        }
    }
}

/// Result of an attempt to delete an archived original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted,
    /// The file was left in place; the reason is shown to the user.
    Kept(String),
    /// The file no longer exists.
    Missing,
}

/// How the verification pass read the archive back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReadbackMode {
    /// Reads went to the device, bypassing the OS cache.
    Uncached,
    /// Cached pages were evicted first, so reads came from the device
    /// unless the platform ignored the request.
    CacheEvicted,
    /// Ordinary reads (cache could not be bypassed on this filesystem).
    Cached,
}

pub fn is_disk_full(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::StorageFull || e.kind() == io::ErrorKind::QuotaExceeded {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(c) => c == libc::ENOSPC || c == libc::EDQUOT,
        #[cfg(windows)]
        Some(c) => c == 112 || c == 39,
        _ => false,
    }
}

pub fn is_file_too_large(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::FileTooLarge {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(c) => c == libc::EFBIG,
        #[cfg(windows)]
        Some(c) => c == 223,
        _ => false,
    }
}

/// Free space available to this user on the volume holding `path`.
pub fn available_space(path: &Path) -> io::Result<u64> {
    fs4::available_space(path)
}

/// Writes all of `buf` at `offset`.
pub fn pwrite_all(f: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = pwrite(f, buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "failed to write whole buffer"));
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Reads exactly `buf.len()` bytes at `offset`.
pub fn pread_exact(f: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = pread(f, buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected end of archive"));
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Hashes everything readable from `f` with SHA-256, from its current position.
pub fn sha256_reader(mut f: impl io::Read) -> io::Result<([u8; 32], u64)> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((h.finalize().into(), n))
}

/// Checks the file behind `f` against what was archived, hashing it again
/// when `sha` is given. Shared by the platform deletion routines.
pub(crate) fn verify_open_source(f: &File, now: &FileStat, expected: &FileStat, sha: Option<&[u8; 32]>) -> io::Result<Option<String>> {
    if !expected.same_content_identity(now) {
        return Ok(Some(format!("kept because {} after it was archived", expected.difference(now))));
    }
    if let Some(want) = sha {
        let (got, n) = sha256_reader(f)?;
        if n != expected.size || &got != want {
            return Ok(Some("kept because its contents no longer match the archived copy".into()));
        }
    }
    Ok(None)
}
