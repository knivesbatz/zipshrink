//! Read-back verification of single entries and of finished archives.

use crate::format::{self, ParsedLocal, METHOD_DEFLATE, METHOD_STORE};
use crate::platform::{self, ReadbackMode};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// What an entry is expected to contain.
#[derive(Debug, Clone)]
pub struct Expected<'a> {
    pub name: &'a str,
    pub offset: u64,
    pub end: u64,
    pub method: u16,
    pub crc32: u32,
    pub csize: u64,
    pub usize: u64,
    pub sha256: Option<[u8; 32]>,
    pub zip64_local: bool,
    /// CRC-32 of the local header bytes as written.
    pub header_crc: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum EntryCheckError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Mismatch(String),
}

/// Decompresses a stream fed in chunks and hashes the output.
pub struct Inflater {
    method: u16,
    z: flate2::Decompress,
    out: Vec<u8>,
    crc: crc32fast::Hasher,
    sha: Sha256,
    pub total_out: u64,
    pub total_in: u64,
    pub ended: bool,
    trailing: bool,
}

impl Inflater {
    pub fn new(method: u16) -> Self {
        Inflater {
            method,
            z: flate2::Decompress::new(false),
            out: vec![0; 256 << 10],
            crc: crc32fast::Hasher::new(),
            sha: Sha256::new(),
            total_out: 0,
            total_in: 0,
            ended: method == METHOD_STORE,
            trailing: false,
        }
    }

    pub fn feed(&mut self, mut input: &[u8]) -> io::Result<()> {
        self.total_in += input.len() as u64;
        if self.method == METHOD_STORE {
            self.crc.update(input);
            self.sha.update(input);
            self.total_out += input.len() as u64;
            return Ok(());
        }
        while !input.is_empty() {
            if self.ended {
                self.trailing = true;
                return Ok(());
            }
            let before_in = self.z.total_in();
            let before_out = self.z.total_out();
            let st = self
                .z
                .decompress(input, &mut self.out, flate2::FlushDecompress::None)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("deflate data is corrupt: {e}")))?;
            let used = (self.z.total_in() - before_in) as usize;
            let produced = (self.z.total_out() - before_out) as usize;
            self.crc.update(&self.out[..produced]);
            self.sha.update(&self.out[..produced]);
            self.total_out += produced as u64;
            input = &input[used..];
            if st == flate2::Status::StreamEnd {
                self.ended = true;
            } else if used == 0 && produced == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "deflate stream stalled"));
            }
        }
        Ok(())
    }

    /// Drains output still buffered inside the decompressor.
    fn drain(&mut self) -> io::Result<()> {
        while self.method == METHOD_DEFLATE && !self.ended {
            let before_out = self.z.total_out();
            let st = self
                .z
                .decompress(&[], &mut self.out, flate2::FlushDecompress::Finish)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("deflate data is corrupt: {e}")))?;
            let produced = (self.z.total_out() - before_out) as usize;
            self.crc.update(&self.out[..produced]);
            self.sha.update(&self.out[..produced]);
            self.total_out += produced as u64;
            if st == flate2::Status::StreamEnd {
                self.ended = true;
            } else if produced == 0 {
                break;
            }
        }
        Ok(())
    }

    /// Returns (crc32, sha256, uncompressed size, stream complete without trailing bytes).
    pub fn finish(mut self) -> io::Result<(u32, [u8; 32], u64, bool)> {
        self.drain()?;
        let ok = self.ended && !self.trailing;
        Ok((self.crc.finalize(), self.sha.finalize().into(), self.total_out, ok))
    }
}

fn mismatch(m: impl Into<String>) -> EntryCheckError {
    EntryCheckError::Mismatch(m.into())
}

/// Reads one entry back from the archive (bypassing the cache where
/// possible), parses its local header, decompresses its data and compares
/// everything with what was recorded while compressing.
pub fn check_entry(f: &File, path: &Path, exp: &Expected<'_>) -> Result<ReadbackMode, EntryCheckError> {
    let total = exp.end - exp.offset;
    let mut header_bytes: Vec<u8> = Vec::new();
    let mut parsed: Option<ParsedLocal> = None;
    let mut inflater = Inflater::new(exp.method);
    let mut first_err: Option<EntryCheckError> = None;
    let mode = platform::read_range_uncached(f, path, exp.offset, total, &mut |mut chunk| {
        if first_err.is_some() {
            return Ok(());
        }
        if parsed.is_none() {
            header_bytes.extend_from_slice(chunk);
            if header_bytes.len() < format::LOCAL_HEADER_FIXED {
                return Ok(());
            }
            let hlen = format::LOCAL_HEADER_FIXED
                + u16::from_le_bytes([header_bytes[26], header_bytes[27]]) as usize
                + u16::from_le_bytes([header_bytes[28], header_bytes[29]]) as usize;
            if header_bytes.len() < hlen {
                return Ok(());
            }
            if exp.header_crc.is_some_and(|c| c != crc32fast::hash(&header_bytes[..hlen])) {
                first_err = Some(mismatch("local header bytes differ from what was written"));
                return Ok(());
            }
            match format::read_local_header(&mut &header_bytes[..hlen]) {
                Ok(p) => parsed = Some(p),
                Err(e) => {
                    first_err = Some(mismatch(format!("local header is unreadable: {e}")));
                    return Ok(());
                }
            }
            let rest = header_bytes[hlen..].to_vec();
            header_bytes.clear();
            chunk = &[];
            if let Err(e) = inflater.feed(&rest) {
                first_err = Some(mismatch(e.to_string()));
            }
        }
        if let Err(e) = inflater.feed(chunk) {
            first_err = Some(mismatch(e.to_string()));
        }
        Ok(())
    })?;
    if let Some(e) = first_err {
        return Err(e);
    }
    let p = parsed.ok_or_else(|| mismatch("entry is shorter than its header"))?;
    if p.name != exp.name.as_bytes() {
        return Err(mismatch("local header names a different file"));
    }
    if p.method != exp.method || p.crc32 != exp.crc32 || p.compressed != exp.csize || p.uncompressed != exp.usize {
        return Err(mismatch("local header sizes or checksum differ from what was written"));
    }
    if p.zip64 != exp.zip64_local || p.sha256 != exp.sha256 {
        return Err(mismatch("local header extra fields differ from what was written"));
    }
    if p.header_len + exp.csize != total {
        return Err(mismatch("entry length does not match its header"));
    }
    let (crc, sha, n, complete) = inflater.finish()?;
    if !complete {
        return Err(mismatch("compressed data is incomplete or has trailing bytes"));
    }
    if n != exp.usize || crc != exp.crc32 {
        return Err(mismatch("decompressed data does not match the source (CRC-32)"));
    }
    if exp.sha256.is_some_and(|s| s != sha) {
        return Err(mismatch("decompressed data does not match the source (SHA-256)"));
    }
    Ok(mode)
}

#[derive(Debug, Clone, Default)]
pub struct ArchiveCheck {
    pub entries: u64,
    pub files: u64,
    pub dirs: u64,
    /// Files whose SHA-256 (stored by ZipShrink) was checked.
    pub sha_checked: u64,
    /// Files checked by CRC-32 only (no stored SHA-256).
    pub crc_only: u64,
    pub bytes: u64,
    pub data_checked: bool,
    pub problems: Vec<String>,
}

impl ArchiveCheck {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Checks a complete archive with an independent ZIP reader (the `zip`
/// crate) plus our own central directory parser for the stored SHA-256s.
/// With `data`, every entry is decompressed, its CRC-32 checked by the
/// reader and its SHA-256 compared. `on_bytes` reports progress.
pub fn check_archive(path: &Path, data: bool, cancel: &AtomicBool, on_bytes: &mut dyn FnMut(u64)) -> io::Result<ArchiveCheck> {
    let f = File::open(path)?;
    let _ = platform::evict_cache(&f, 0, f.metadata()?.len());
    let central = format::read_central_directory(&f)?;
    let mut zip = zip::ZipArchive::new(File::open(path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("the archive cannot be opened: {e}")))?;
    let mut out = ArchiveCheck { data_checked: data, ..Default::default() };
    if zip.len() != central.len() {
        out.problems.push(format!("entry count differs: {} vs {}", zip.len(), central.len()));
        return Ok(out);
    }
    let mut buf = vec![0u8; 1 << 20];
    for (i, c) in central.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        out.entries += 1;
        let name = String::from_utf8_lossy(&c.name).into_owned();
        let mut zf = match zip.by_index(i) {
            Ok(z) => z,
            Err(e) => {
                out.problems.push(format!("{name}: cannot be read: {e}"));
                continue;
            }
        };
        if zf.name_raw() != c.name.as_slice() || zf.size() != c.uncompressed || zf.crc32() != c.crc32 {
            out.problems.push(format!("{name}: central directory disagrees with the reader"));
            continue;
        }
        if zf.is_dir() {
            out.dirs += 1;
            continue;
        }
        out.files += 1;
        out.bytes += c.uncompressed;
        if !data {
            continue;
        }
        let mut sha = Sha256::new();
        let mut n = 0u64;
        let res: io::Result<()> = (|| loop {
            let k = zf.read(&mut buf)?;
            if k == 0 {
                return Ok(());
            }
            sha.update(&buf[..k]);
            n += k as u64;
            on_bytes(k as u64);
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
        })();
        match res {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
            // The reader verifies CRC-32 at the end of each entry.
            Err(e) => out.problems.push(format!("{name}: {e}")),
            Ok(()) if n != c.uncompressed => out.problems.push(format!("{name}: wrong length")),
            Ok(()) => match c.sha256 {
                Some(want) if want != <[u8; 32]>::from(sha.finalize()) => out.problems.push(format!("{name}: SHA-256 does not match")),
                Some(_) => out.sha_checked += 1,
                None => out.crc_only += 1,
            },
        }
    }
    Ok(out)
}
