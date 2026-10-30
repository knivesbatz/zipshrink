//! Rebuilds a readable archive from a partial archive without its journal.
//!
//! Entries are found by walking local headers from the start of the file.
//! Each one is accepted only if its data decompresses to exactly the size,
//! CRC-32 and (when present) SHA-256 its header records. Damaged regions
//! are skipped by searching for the next valid header; the bytes are left
//! in the file. A new central directory listing the valid entries is then
//! appended, the result verified, and the file renamed.

use crate::error::{Error, Result};
use crate::format::{self, EntryMeta, EntrySizes};
use crate::platform;
use crate::verify::{self, ArchiveCheck, Inflater};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct SalvageReport {
    pub output: PathBuf,
    pub entries: u64,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// Byte ranges that held no valid entry.
    pub damaged: Vec<(u64, u64)>,
    pub check: ArchiveCheck,
}

struct Found {
    offset: u64,
    meta: EntryMeta,
    sizes: EntrySizes,
}

fn read_at(f: &File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut b = vec![0u8; len];
    platform::pread_exact(f, &mut b, offset)?;
    Ok(b)
}

fn mtime_from_extra(extra: &[u8]) -> Option<(i64, u32)> {
    for (id, d) in format::extra_fields(extra) {
        if id == format::EXTRA_NTFS && d.len() >= 32 {
            let t = u64::from_le_bytes(d[8..16].try_into().unwrap());
            if t > 0 {
                return Some(((t / 10_000_000) as i64 - 11_644_473_600, ((t % 10_000_000) * 100) as u32));
            }
        }
        if id == format::EXTRA_UT && d.len() >= 5 && d[0] & 1 != 0 {
            return Some((i32::from_le_bytes(d[1..5].try_into().unwrap()) as i64, 0));
        }
    }
    None
}

/// Tries to read a valid entry at `offset`; returns it and its end.
fn entry_at(f: &File, offset: u64, len: u64) -> Option<(Found, u64)> {
    if offset + format::LOCAL_HEADER_FIXED as u64 > len {
        return None;
    }
    let fixed = read_at(f, offset, format::LOCAL_HEADER_FIXED).ok()?;
    let nl = u16::from_le_bytes([fixed[26], fixed[27]]) as u64;
    let el = u16::from_le_bytes([fixed[28], fixed[29]]) as u64;
    let hlen = format::LOCAL_HEADER_FIXED as u64 + nl + el;
    if offset + hlen > len {
        return None;
    }
    let raw = read_at(f, offset, hlen as usize).ok()?;
    let p = format::read_local_header(&mut &raw[..]).ok()?;
    if p.flags & 0x0008 != 0 || (p.method != format::METHOD_STORE && p.method != format::METHOD_DEFLATE) {
        return None;
    }
    let name = String::from_utf8(p.name.clone()).ok()?;
    if name.is_empty() {
        return None;
    }
    let end = offset.checked_add(hlen)?.checked_add(p.compressed)?;
    if end > len {
        return None;
    }
    let is_dir = name.ends_with('/');
    if is_dir && (p.compressed != 0 || p.uncompressed != 0) {
        return None;
    }
    let mut inf = Inflater::new(p.method);
    let mut buf = vec![0u8; 1 << 20];
    let mut pos = offset + hlen;
    while pos < end {
        let k = ((end - pos) as usize).min(buf.len());
        platform::pread_exact(f, &mut buf[..k], pos).ok()?;
        inf.feed(&buf[..k]).ok()?;
        pos += k as u64;
    }
    let (crc, sha, n, complete) = inf.finish().ok()?;
    if !complete || n != p.uncompressed || crc != p.crc32 || p.sha256.is_some_and(|s| s != sha) {
        return None;
    }
    let extra = &raw[format::LOCAL_HEADER_FIXED + nl as usize..];
    let meta = EntryMeta { name, is_dir, mtime: mtime_from_extra(extra), dos_attrs: if is_dir { 0x10 } else { 0 }, ..Default::default() };
    let sizes = EntrySizes {
        method: p.method,
        crc32: p.crc32,
        compressed: p.compressed,
        uncompressed: p.uncompressed,
        sha256: p.sha256,
        zip64_local: p.zip64,
    };
    Some((Found { offset, meta, sizes }, end))
}

/// Finds the next local header signature at or after `from`.
fn next_signature(f: &File, from: u64, len: u64) -> io::Result<Option<u64>> {
    let sig = format::LOCAL_SIG.to_le_bytes();
    let mut pos = from;
    let mut buf = vec![0u8; 1 << 20];
    while pos + 4 <= len {
        let k = ((len - pos) as usize).min(buf.len());
        platform::pread_exact(f, &mut buf[..k], pos)?;
        if let Some(i) = buf[..k].windows(4).position(|w| w == sig) {
            return Ok(Some(pos + i as u64));
        }
        if k < 4 {
            break;
        }
        pos += (k - 3) as u64;
    }
    Ok(None)
}

pub fn default_output(partial: &Path) -> PathBuf {
    let s = partial.as_os_str().to_string_lossy();
    let base = s.strip_suffix(".partial").map(str::to_owned).unwrap_or_else(|| format!("{s}.zip"));
    let p = PathBuf::from(&base);
    if !p.exists() {
        return p;
    }
    let stem = base.strip_suffix(".zip").unwrap_or(&base).to_owned();
    (1..).map(|n| PathBuf::from(format!("{stem} (recovered {n}).zip"))).find(|p| !p.exists()).unwrap()
}

/// Rebuilds `partial` in place and renames it to `output`.
pub fn salvage(partial: &Path, output: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<SalvageReport> {
    if output.exists() {
        return Err(Error::OutputExists(output.to_owned()));
    }
    let f = OpenOptions::new().read(true).write(true).open(partial).map_err(|e| Error::io(partial, e))?;
    let len = f.metadata().map_err(|e| Error::io(partial, e))?.len();
    let mut found: Vec<Found> = Vec::new();
    let mut damaged = Vec::new();
    let mut pos = 0u64;
    let mut last_end = 0u64;
    while pos < len {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        progress(pos, len);
        match entry_at(&f, pos, len) {
            Some((e, end)) => {
                found.push(e);
                pos = end;
                last_end = end;
            }
            None => {
                let mut probe = pos + 1;
                let mut resumed = None;
                while let Some(s) = next_signature(&f, probe, len).map_err(|e| Error::io(partial, e))? {
                    if entry_at(&f, s, len).is_some() {
                        resumed = Some(s);
                        break;
                    }
                    probe = s + 1;
                }
                match resumed {
                    Some(s) => {
                        damaged.push((pos, s));
                        pos = s;
                    }
                    None => {
                        damaged.push((pos, len));
                        break;
                    }
                }
            }
        }
    }
    if found.is_empty() {
        return Err(Error::Damaged(format!("{} holds no complete, valid entries", partial.display())));
    }
    // A damaged tail (an unfinished entry or an old central directory) is
    // replaced; damage between valid entries is left where it is.
    let cd_offset = last_end;
    f.set_len(cd_offset).map_err(|e| Error::io(partial, e))?;
    let mut cd = Vec::new();
    for e in &found {
        cd.extend_from_slice(&format::central_header(&e.meta, &e.sizes, e.offset));
    }
    cd.extend_from_slice(&format::end_records(found.len() as u64, cd_offset, cd.len() as u64));
    platform::pwrite_all(&f, &cd, cd_offset).map_err(|e| Error::io(partial, e))?;
    platform::sync_file(&f).map_err(|e| Error::io(partial, e))?;
    drop(f);
    let check = verify::check_archive(partial, true, cancel, &mut |_| {}).map_err(|e| Error::io(partial, e))?;
    if !check.ok() {
        return Err(Error::VerifyFailed(check.problems.join("; ")));
    }
    platform::rename_noreplace(partial, output).map_err(|e| Error::io(output, e))?;
    let _ = platform::sync_dir(output.parent().unwrap_or(Path::new(".")));
    let files = found.iter().filter(|e| !e.meta.is_dir).count() as u64;
    Ok(SalvageReport {
        output: output.to_owned(),
        entries: found.len() as u64,
        files,
        dirs: found.len() as u64 - files,
        bytes: found.iter().map(|e| e.sizes.uncompressed).sum(),
        damaged,
        check,
    })
}
