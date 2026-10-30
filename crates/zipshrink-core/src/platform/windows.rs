use super::{verify_open_source, DeleteOutcome, FileStat, Kind, ReadbackMode};
use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FileBasicInfo, FileDispositionInfo, FileDispositionInfoEx, GetFileInformationByHandle, GetFileInformationByHandleEx,
    GetVolumeInformationByHandleW, MoveFileExW, SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_SEQUENTIAL_SCAN, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH,
};

const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;
const ERROR_ACCESS_DENIED: i32 = 5;

pub fn pwrite(f: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    f.seek_write(buf, offset)
}

pub fn pread(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    f.seek_read(buf, offset)
}

fn ft_to_unix(t: u64) -> Option<(i64, u32)> {
    if t == 0 {
        return None;
    }
    let secs = (t / 10_000_000) as i64 - 11_644_473_600;
    Some((secs, ((t % 10_000_000) * 100) as u32))
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn raw(f: &File) -> HANDLE {
    f.as_raw_handle() as HANDLE
}

fn handle_info(f: &File) -> io::Result<(BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO)> {
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(raw(f), &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut basic: FILE_BASIC_INFO = std::mem::zeroed();
        if GetFileInformationByHandleEx(
            raw(f),
            FileBasicInfo,
            &mut basic as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok((info, basic))
    }
}

pub fn stat_handle(f: &File) -> io::Result<FileStat> {
    let (info, basic) = handle_info(f)?;
    let attrs = info.dwFileAttributes;
    Ok(FileStat {
        size: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        dev: info.dwVolumeSerialNumber as u64,
        ino: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        mtime: ft_to_unix(ft(info.ftLastWriteTime)),
        atime: ft_to_unix(ft(info.ftLastAccessTime)),
        btime: ft_to_unix(ft(info.ftCreationTime)),
        change: basic.ChangeTime,
        readonly: attrs & FILE_ATTRIBUTE_READONLY != 0,
        unix_mode: None,
        dos_attrs: (attrs & 0x27) as u8,
    })
}

/// Metadata of a directory, as far as `std` reports it (no file id).
pub fn stat_from_metadata(m: &Metadata) -> FileStat {
    let attrs = m.file_attributes();
    FileStat {
        size: m.len(),
        dev: 0,
        ino: 0,
        mtime: ft_to_unix(m.last_write_time()),
        atime: ft_to_unix(m.last_access_time()),
        btime: ft_to_unix(m.creation_time()),
        change: 0,
        readonly: attrs & FILE_ATTRIBUTE_READONLY != 0,
        unix_mode: None,
        dos_attrs: (attrs & 0x37) as u8,
    }
}

/// Classifies `path` without following links or reparse points.
pub fn kind_of(path: &Path) -> io::Result<(Kind, Metadata)> {
    let m = fs::symlink_metadata(path)?;
    let a = m.file_attributes();
    let k = if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 || m.file_type().is_symlink() {
        // Symbolic links, junctions, mount points, OneDrive and other cloud
        // placeholders, deduplicated files: never followed or deleted.
        Kind::Link
    } else if a & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN) != 0 {
        Kind::Other("offline or cloud-only file")
    } else if a & FILE_ATTRIBUTE_DIRECTORY != 0 {
        Kind::Dir
    } else if m.is_file() {
        Kind::File
    } else {
        Kind::Other("special file")
    };
    Ok((k, m))
}

fn check_plain_file(f: &File) -> io::Result<()> {
    let (info, _) = handle_info(f)?;
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file (reparse point or directory)"));
    }
    Ok(())
}

/// Opens a source for compression. Other programs may keep reading it, but
/// cannot write, rename or delete it until we close it.
pub fn open_source(path: &Path) -> io::Result<(File, FileStat)> {
    let f = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)?;
    check_plain_file(&f)?;
    let st = stat_handle(&f)?;
    Ok((f, st))
}

pub fn sync_file(f: &File) -> io::Result<()> {
    f.sync_all() // FlushFileBuffers
}

pub fn sync_dir(_dir: &Path) -> io::Result<()> {
    // NTFS journals directory changes, and renames use MOVEFILE_WRITE_THROUGH.
    Ok(())
}

fn wide(p: &Path) -> io::Result<Vec<u16>> {
    let abs = std::path::absolute(p)?;
    let s = abs.as_os_str().to_string_lossy();
    let verbatim: PathBuf = if s.starts_with(r"\\?\") {
        abs.clone()
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{unc}"))
    } else {
        PathBuf::from(format!(r"\\?\{s}"))
    };
    Ok(OsStr::new(verbatim.as_os_str()).encode_wide().chain(Some(0)).collect())
}

fn move_file(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let a = wide(from)?;
    let b = wide(to)?;
    let flags = MOVEFILE_WRITE_THROUGH | if replace { MOVEFILE_REPLACE_EXISTING } else { 0 };
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    move_file(from, to, false)
}

pub fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    move_file(from, to, true)
}

fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_GENERIC_READ)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

pub fn volume_id(path: &Path) -> io::Result<u64> {
    let f = open_dir(path)?;
    let (info, _) = handle_info(&f)?;
    Ok(info.dwVolumeSerialNumber as u64)
}

pub fn fs_name(path: &Path) -> Option<String> {
    let f = open_dir(path).ok()?;
    let mut name = [0u16; 64];
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            raw(&f),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..n]))
}

const ALIGN: usize = 4096;

/// Reads `[offset, offset + len)` through a separate unbuffered handle, so
/// the data comes from the device and not from the file cache.
pub fn read_range_uncached(
    f: &File,
    path: &Path,
    offset: u64,
    len: u64,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<ReadbackMode> {
    let direct = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_NO_BUFFERING)
        .open(path);
    if let Ok(d) = direct {
        match read_aligned(&d, offset, len, sink) {
            Ok(()) => return Ok(ReadbackMode::Uncached),
            // ERROR_INVALID_PARAMETER: the volume needs a larger alignment.
            Err(e) if e.raw_os_error() == Some(87) => {}
            Err(e) => return Err(e),
        }
    }
    let mut buf = vec![0u8; 1 << 20];
    let mut pos = offset;
    let end = offset + len;
    while pos < end {
        let want = ((end - pos) as usize).min(buf.len());
        super::pread_exact(f, &mut buf[..want], pos)?;
        sink(&buf[..want])?;
        pos += want as u64;
    }
    Ok(ReadbackMode::Cached)
}

fn read_aligned(d: &File, offset: u64, len: u64, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    const CHUNK: usize = 1 << 20;
    let mut storage = vec![0u8; CHUNK + ALIGN];
    let shift = storage.as_ptr().align_offset(ALIGN);
    let buf = &mut storage[shift..shift + CHUNK];
    let end = offset + len;
    let mut pos = offset - offset % ALIGN as u64;
    while pos < end {
        let n = pread(d, buf, pos)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected end of archive"));
        }
        let chunk_end = pos + n as u64;
        let from = offset.max(pos);
        let to = end.min(chunk_end);
        if to > from {
            sink(&buf[(from - pos) as usize..(to - pos) as usize])?;
        }
        pos = chunk_end;
        if n % ALIGN != 0 && pos < end {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short unbuffered read"));
        }
    }
    Ok(())
}

pub fn evict_cache(_f: &File, _offset: u64, _len: u64) -> ReadbackMode {
    ReadbackMode::Uncached
}

fn mark_for_deletion(f: &File) -> io::Result<()> {
    unsafe {
        let ex = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS };
        if SetFileInformationByHandle(
            raw(f),
            FileDispositionInfoEx,
            &ex as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        ) != 0
        {
            return Ok(());
        }
        // Older Windows 10 builds and some filesystems (FAT, exFAT) lack
        // FileDispositionInfoEx; the classic form deletes on close.
        let classic = FILE_DISPOSITION_INFO { DeleteFile: true };
        if SetFileInformationByHandle(
            raw(f),
            FileDispositionInfo,
            &classic as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        ) != 0
        {
            return Ok(());
        }
        Err(io::Error::last_os_error())
    }
}

/// Deletes `path` only if it is still the exact file that was archived.
/// The file is verified and deleted through one handle that denies writes,
/// renames and deletes by anyone else, so nothing can swap it in between.
pub fn delete_verified(path: &Path, expected: &FileStat, sha: Option<&[u8; 32]>) -> io::Result<DeleteOutcome> {
    let f = match OpenOptions::new()
        .access_mode(FILE_GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DeleteOutcome::Missing),
        Err(e) if is_sharing_violation(&e) => return Ok(DeleteOutcome::Kept("kept because another program has it open".into())),
        Err(e) => return Err(e),
    };
    if check_plain_file(&f).is_err() {
        return Ok(DeleteOutcome::Kept("kept because it was replaced by a link or reparse point".into()));
    }
    let now = stat_handle(&f)?;
    if let Some(reason) = verify_open_source(&f, &now, expected, sha)? {
        return Ok(DeleteOutcome::Kept(reason));
    }
    match mark_for_deletion(&f) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) && now.readonly => {
            return Ok(DeleteOutcome::Kept("kept because it is read-only".into()))
        }
        Err(e) => return Err(e),
    }
    drop(f); // the file is removed when its last handle closes
    Ok(DeleteOutcome::Deleted)
}

/// Removes an empty directory if it is still a plain directory.
pub fn remove_dir_verified(path: &Path, _expected: &FileStat) -> io::Result<bool> {
    match kind_of(path) {
        Ok((Kind::Dir, _)) => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    match fs::remove_dir(path) {
        Ok(()) => Ok(true),
        // ERROR_DIR_NOT_EMPTY
        Err(e) if e.raw_os_error() == Some(145) || e.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn is_sharing_violation(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION))
}
