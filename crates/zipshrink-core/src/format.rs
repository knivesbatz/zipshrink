//! ZIP / ZIP64 on-disk structures, written and parsed by hand.
//!
//! We write our own records instead of using a general-purpose ZIP writer
//! because crash recovery depends on knowing, byte for byte, where every
//! entry starts and ends. Every entry is a local header followed by its data;
//! sizes and CRC-32 are patched into the local header after the data is
//! written (no data descriptors), so the partial archive can be read
//! sequentially and its central directory rebuilt from it at any time.
//!
//! References: PKWARE APPNOTE.TXT 6.3.10, sections 4.3 (structure), 4.4
//! (fields), 4.5 (extensible data fields), and Info-ZIP's extra field notes
//! for 0x5455 (extended timestamp).

use std::io::{self, Read};

pub const LOCAL_SIG: u32 = 0x0403_4b50;
pub const CENTRAL_SIG: u32 = 0x0201_4b50;
pub const EOCD_SIG: u32 = 0x0605_4b50;
pub const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;
pub const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;

pub const FLAG_UTF8: u16 = 1 << 11;
pub const METHOD_STORE: u16 = 0;
pub const METHOD_DEFLATE: u16 = 8;

pub const EXTRA_ZIP64: u16 = 0x0001;
pub const EXTRA_NTFS: u16 = 0x000a;
pub const EXTRA_UT: u16 = 0x5455;
/// ZipShrink content hash: "ZSH1" followed by the 32-byte SHA-256 of the
/// uncompressed data. Unknown extra fields are skipped by conforming readers.
pub const EXTRA_ZSHASH: u16 = 0x5a53;
pub const ZSHASH_MAGIC: &[u8; 4] = b"ZSH1";

pub const LOCAL_HEADER_FIXED: usize = 30;
pub const CENTRAL_HEADER_FIXED: usize = 46;

const U32_MAX: u64 = 0xffff_ffff;

/// Entries whose sizes could reach this are written with ZIP64 local
/// headers. Deflate expands incompressible input by at most 5 bytes per
/// 16 KiB stored block plus a few bytes, so the margin below is generous.
pub fn needs_zip64_sizes(uncompressed: u64) -> bool {
    let worst = worst_case_compressed(uncompressed);
    worst >= U32_MAX || uncompressed >= U32_MAX
}

/// Upper bound on the deflate output for `n` input bytes.
pub fn worst_case_compressed(n: u64) -> u64 {
    n + n / 1000 + 1024
}

/// Metadata that goes into an entry's headers.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntryMeta {
    /// Archive name, `/`-separated; directories end with `/`.
    pub name: String,
    pub is_dir: bool,
    /// Modification time, seconds since the Unix epoch, and nanoseconds.
    pub mtime: Option<(i64, u32)>,
    pub atime: Option<(i64, u32)>,
    pub btime: Option<(i64, u32)>,
    /// Unix permission bits including the file type, when known.
    pub unix_mode: Option<u32>,
    /// MS-DOS / Windows attribute bits (read-only, hidden, system, dir, archive).
    pub dos_attrs: u8,
}

/// Values only known once the data is written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EntrySizes {
    pub method: u16,
    pub crc32: u32,
    pub compressed: u64,
    pub uncompressed: u64,
    pub sha256: Option<[u8; 32]>,
    /// Whether the local header carries a ZIP64 extra field.
    pub zip64_local: bool,
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}

/// MS-DOS date and time in local time, clamped to the representable range.
pub fn dos_datetime(mtime: Option<(i64, u32)>) -> (u16, u16) {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let Some((secs, _)) = mtime else {
        return (0, (1 << 5) | 1); // 1980-01-01 00:00
    };
    let Some(dt) = Local.timestamp_opt(secs, 0).earliest() else {
        return (0, (1 << 5) | 1);
    };
    let year = dt.year();
    if year < 1980 {
        return (0, (1 << 5) | 1);
    }
    if year > 2107 {
        return ((23 << 11) | (59 << 5) | 29, (127 << 9) | (12 << 5) | 31);
    }
    let time = ((dt.hour() as u16) << 11) | ((dt.minute() as u16) << 5) | ((dt.second() as u16) / 2);
    let date = (((year - 1980) as u16) << 9) | ((dt.month() as u16) << 5) | dt.day() as u16;
    (time, date)
}

fn filetime(t: (i64, u32)) -> u64 {
    // 100 ns intervals since 1601-01-01.
    let secs = t.0.saturating_add(11_644_473_600);
    if secs < 0 {
        return 0;
    }
    (secs as u64).saturating_mul(10_000_000) + (t.1 / 100) as u64
}

pub fn version_made_by() -> u16 {
    let host: u16 = if cfg!(windows) { 0 } else { 3 };
    (host << 8) | 63
}

fn version_needed(meta: &EntryMeta, sizes: &EntrySizes, zip64: bool) -> u16 {
    if zip64 {
        45
    } else if meta.is_dir || sizes.method == METHOD_DEFLATE {
        20
    } else {
        10
    }
}

pub fn external_attrs(meta: &EntryMeta) -> u32 {
    let mut attrs = meta.dos_attrs as u32;
    if meta.is_dir {
        attrs |= 0x10;
    }
    if let Some(mode) = meta.unix_mode {
        attrs |= mode << 16;
    }
    attrs
}

fn time_extras(meta: &EntryMeta, out: &mut Vec<u8>, central: bool) {
    // Info-ZIP extended timestamp: 32-bit signed seconds, mtime only.
    if let Some((secs, _)) = meta.mtime {
        if (i32::MIN as i64..=i32::MAX as i64).contains(&secs) {
            put16(out, EXTRA_UT);
            put16(out, 5);
            out.push(1);
            put32(out, secs as i32 as u32);
        }
    }
    // NTFS timestamps, 100 ns precision; used by Windows tools and 7-Zip.
    if let Some(m) = meta.mtime {
        let a = meta.atime.unwrap_or(m);
        let c = meta.btime.unwrap_or(m);
        put16(out, EXTRA_NTFS);
        put16(out, 32);
        put32(out, 0);
        put16(out, 1);
        put16(out, 24);
        put64(out, filetime(m));
        put64(out, filetime(a));
        put64(out, filetime(c));
    }
    let _ = central;
}

fn hash_extra(sha: Option<[u8; 32]>, out: &mut Vec<u8>) {
    put16(out, EXTRA_ZSHASH);
    put16(out, 36);
    out.extend_from_slice(ZSHASH_MAGIC);
    out.extend_from_slice(&sha.unwrap_or([0; 32]));
}

/// Builds a local file header. Its length depends only on `meta` and
/// `sizes.zip64_local`, so a header written with placeholder sizes can be
/// overwritten in place once the real values are known.
pub fn local_header(meta: &EntryMeta, sizes: &EntrySizes) -> Vec<u8> {
    let zip64 = sizes.zip64_local;
    let mut extra = Vec::new();
    if zip64 {
        put16(&mut extra, EXTRA_ZIP64);
        put16(&mut extra, 16);
        put64(&mut extra, sizes.uncompressed);
        put64(&mut extra, sizes.compressed);
    }
    time_extras(meta, &mut extra, false);
    if !meta.is_dir {
        hash_extra(sizes.sha256, &mut extra);
    }
    let (time, date) = dos_datetime(meta.mtime);
    let name = meta.name.as_bytes();
    let mut b = Vec::with_capacity(LOCAL_HEADER_FIXED + name.len() + extra.len());
    put32(&mut b, LOCAL_SIG);
    put16(&mut b, version_needed(meta, sizes, zip64));
    put16(&mut b, FLAG_UTF8);
    put16(&mut b, sizes.method);
    put16(&mut b, time);
    put16(&mut b, date);
    put32(&mut b, sizes.crc32);
    if zip64 {
        put32(&mut b, U32_MAX as u32);
        put32(&mut b, U32_MAX as u32);
    } else {
        put32(&mut b, sizes.compressed as u32);
        put32(&mut b, sizes.uncompressed as u32);
    }
    put16(&mut b, name.len() as u16);
    put16(&mut b, extra.len() as u16);
    b.extend_from_slice(name);
    b.extend_from_slice(&extra);
    b
}

/// Builds a central directory file header.
pub fn central_header(meta: &EntryMeta, sizes: &EntrySizes, local_offset: u64) -> Vec<u8> {
    let big_sizes = sizes.compressed >= U32_MAX || sizes.uncompressed >= U32_MAX || sizes.zip64_local;
    let big_offset = local_offset >= U32_MAX;
    let mut extra = Vec::new();
    if big_sizes || big_offset {
        let mut z = Vec::new();
        if big_sizes {
            put64(&mut z, sizes.uncompressed);
            put64(&mut z, sizes.compressed);
        }
        if big_offset {
            put64(&mut z, local_offset);
        }
        put16(&mut extra, EXTRA_ZIP64);
        put16(&mut extra, z.len() as u16);
        extra.extend_from_slice(&z);
    }
    time_extras(meta, &mut extra, true);
    if !meta.is_dir {
        hash_extra(sizes.sha256, &mut extra);
    }
    let (time, date) = dos_datetime(meta.mtime);
    let name = meta.name.as_bytes();
    let mut b = Vec::with_capacity(CENTRAL_HEADER_FIXED + name.len() + extra.len());
    put32(&mut b, CENTRAL_SIG);
    put16(&mut b, version_made_by());
    put16(&mut b, version_needed(meta, sizes, big_sizes || big_offset));
    put16(&mut b, FLAG_UTF8);
    put16(&mut b, sizes.method);
    put16(&mut b, time);
    put16(&mut b, date);
    put32(&mut b, sizes.crc32);
    if big_sizes {
        put32(&mut b, U32_MAX as u32);
        put32(&mut b, U32_MAX as u32);
    } else {
        put32(&mut b, sizes.compressed as u32);
        put32(&mut b, sizes.uncompressed as u32);
    }
    put16(&mut b, name.len() as u16);
    put16(&mut b, extra.len() as u16);
    put16(&mut b, 0); // comment length
    put16(&mut b, 0); // disk number start
    put16(&mut b, 0); // internal attributes
    put32(&mut b, external_attrs(meta));
    put32(&mut b, if big_offset { U32_MAX as u32 } else { local_offset as u32 });
    b.extend_from_slice(name);
    b.extend_from_slice(&extra);
    b
}

/// Builds the end of central directory records (ZIP64 ones when needed).
pub fn end_records(entries: u64, cd_offset: u64, cd_size: u64) -> Vec<u8> {
    let zip64 = entries >= 0xffff || cd_offset >= U32_MAX || cd_size >= U32_MAX;
    let mut b = Vec::new();
    if zip64 {
        let z64_offset = cd_offset + cd_size;
        put32(&mut b, ZIP64_EOCD_SIG);
        put64(&mut b, 44);
        put16(&mut b, version_made_by());
        put16(&mut b, 45);
        put32(&mut b, 0);
        put32(&mut b, 0);
        put64(&mut b, entries);
        put64(&mut b, entries);
        put64(&mut b, cd_size);
        put64(&mut b, cd_offset);
        put32(&mut b, ZIP64_LOCATOR_SIG);
        put32(&mut b, 0);
        put64(&mut b, z64_offset);
        put32(&mut b, 1);
    }
    put32(&mut b, EOCD_SIG);
    put16(&mut b, 0);
    put16(&mut b, 0);
    let n16 = if zip64 { 0xffff } else { entries as u16 };
    put16(&mut b, n16);
    put16(&mut b, n16);
    put32(&mut b, if zip64 { U32_MAX as u32 } else { cd_size as u32 });
    put32(&mut b, if zip64 { U32_MAX as u32 } else { cd_offset as u32 });
    put16(&mut b, 0);
    b
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Iterates `(id, data)` over an extra field block, stopping at malformed data.
pub fn extra_fields(mut extra: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    std::iter::from_fn(move || {
        if extra.len() < 4 {
            return None;
        }
        let id = le16(extra, 0);
        let len = le16(extra, 2) as usize;
        if extra.len() < 4 + len {
            return None;
        }
        let data = &extra[4..4 + len];
        extra = &extra[4 + len..];
        Some((id, data))
    })
}

pub fn find_hash(extra: &[u8]) -> Option<[u8; 32]> {
    extra_fields(extra)
        .find_map(|(id, d)| (id == EXTRA_ZSHASH && d.len() == 36 && &d[..4] == ZSHASH_MAGIC).then(|| d[4..36].try_into().unwrap()))
}

/// A local header read back from an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLocal {
    pub name: Vec<u8>,
    pub flags: u16,
    pub method: u16,
    pub crc32: u32,
    pub compressed: u64,
    pub uncompressed: u64,
    pub zip64: bool,
    pub sha256: Option<[u8; 32]>,
    /// Header length, i.e. offset of the data relative to the header start.
    pub header_len: u64,
}

/// Parses a local header from a reader positioned at its signature.
pub fn read_local_header(r: &mut impl Read) -> io::Result<ParsedLocal> {
    let mut fixed = [0u8; LOCAL_HEADER_FIXED];
    r.read_exact(&mut fixed)?;
    if le32(&fixed, 0) != LOCAL_SIG {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a local file header"));
    }
    let flags = le16(&fixed, 6);
    let method = le16(&fixed, 8);
    let crc32 = le32(&fixed, 14);
    let mut compressed = le32(&fixed, 18) as u64;
    let mut uncompressed = le32(&fixed, 22) as u64;
    let name_len = le16(&fixed, 26) as usize;
    let extra_len = le16(&fixed, 28) as usize;
    let mut name = vec![0; name_len];
    r.read_exact(&mut name)?;
    let mut extra = vec![0; extra_len];
    r.read_exact(&mut extra)?;
    let mut zip64 = false;
    for (id, d) in extra_fields(&extra) {
        if id == EXTRA_ZIP64 && d.len() >= 16 {
            zip64 = true;
            if uncompressed == U32_MAX {
                uncompressed = le64(d, 0);
            }
            if compressed == U32_MAX {
                compressed = le64(d, 8);
            }
        }
    }
    Ok(ParsedLocal {
        name,
        flags,
        method,
        crc32,
        compressed,
        uncompressed,
        zip64,
        sha256: find_hash(&extra),
        header_len: (LOCAL_HEADER_FIXED + name_len + extra_len) as u64,
    })
}

/// A central directory entry read back from an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCentral {
    pub name: Vec<u8>,
    pub method: u16,
    pub crc32: u32,
    pub compressed: u64,
    pub uncompressed: u64,
    pub local_offset: u64,
    pub sha256: Option<[u8; 32]>,
}

/// Locates and parses the central directory of a complete archive.
pub fn read_central_directory(f: &std::fs::File) -> io::Result<Vec<ParsedCentral>> {
    use std::io::{Seek, SeekFrom};
    let invalid = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let mut f = f;
    let len = f.seek(SeekFrom::End(0))?;
    // The EOCD is at most 22 + 65535 bytes from the end.
    let tail_len = len.min(22 + 65_535);
    let mut tail = vec![0u8; tail_len as usize];
    f.seek(SeekFrom::Start(len - tail_len))?;
    f.read_exact(&mut tail)?;
    let pos = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| le32(&tail, i) == EOCD_SIG && i + 22 + le16(&tail, i + 20) as usize == tail.len())
        .ok_or_else(|| invalid("end of central directory not found"))?;
    let eocd_at = len - tail_len + pos as u64;
    let mut entries = le16(&tail, pos + 10) as u64;
    let mut cd_size = le32(&tail, pos + 12) as u64;
    let mut cd_offset = le32(&tail, pos + 16) as u64;
    if (entries == 0xffff || cd_size == U32_MAX || cd_offset == U32_MAX) && eocd_at >= 20 {
        let mut loc = [0u8; 20];
        f.seek(SeekFrom::Start(eocd_at - 20))?;
        f.read_exact(&mut loc)?;
        if le32(&loc, 0) == ZIP64_LOCATOR_SIG {
            let z = le64(&loc, 8);
            let mut rec = [0u8; 56];
            f.seek(SeekFrom::Start(z))?;
            f.read_exact(&mut rec)?;
            if le32(&rec, 0) != ZIP64_EOCD_SIG {
                return Err(invalid("bad ZIP64 end of central directory"));
            }
            entries = le64(&rec, 32);
            cd_size = le64(&rec, 40);
            cd_offset = le64(&rec, 48);
        }
    }
    if cd_offset.checked_add(cd_size).is_none_or(|e| e > len) {
        return Err(invalid("central directory lies outside the file"));
    }
    let mut cd = vec![0u8; cd_size as usize];
    f.seek(SeekFrom::Start(cd_offset))?;
    f.read_exact(&mut cd)?;
    let mut out = Vec::with_capacity(entries as usize);
    let mut p = 0usize;
    for _ in 0..entries {
        if p + CENTRAL_HEADER_FIXED > cd.len() || le32(&cd, p) != CENTRAL_SIG {
            return Err(invalid("corrupt central directory entry"));
        }
        let method = le16(&cd, p + 10);
        let crc32 = le32(&cd, p + 16);
        let mut compressed = le32(&cd, p + 20) as u64;
        let mut uncompressed = le32(&cd, p + 24) as u64;
        let nl = le16(&cd, p + 28) as usize;
        let el = le16(&cd, p + 30) as usize;
        let cl = le16(&cd, p + 32) as usize;
        let mut local_offset = le32(&cd, p + 42) as u64;
        let end = p + CENTRAL_HEADER_FIXED + nl + el + cl;
        if end > cd.len() {
            return Err(invalid("truncated central directory entry"));
        }
        let name = cd[p + 46..p + 46 + nl].to_vec();
        let extra = &cd[p + 46 + nl..p + 46 + nl + el];
        for (id, d) in extra_fields(extra) {
            if id == EXTRA_ZIP64 {
                let mut q = 0;
                let next = |q: &mut usize| -> Option<u64> {
                    let v = (d.len() >= *q + 8).then(|| le64(d, *q));
                    *q += 8;
                    v
                };
                if uncompressed == U32_MAX {
                    uncompressed = next(&mut q).ok_or_else(|| invalid("short ZIP64 field"))?;
                }
                if compressed == U32_MAX {
                    compressed = next(&mut q).ok_or_else(|| invalid("short ZIP64 field"))?;
                }
                if local_offset == U32_MAX {
                    local_offset = next(&mut q).ok_or_else(|| invalid("short ZIP64 field"))?;
                }
            }
        }
        out.push(ParsedCentral { name, method, crc32, compressed, uncompressed, local_offset, sha256: find_hash(extra) });
        p = end;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(name: &str) -> EntryMeta {
        EntryMeta { name: name.into(), mtime: Some((1_700_000_000, 5)), unix_mode: Some(0o100644), ..Default::default() }
    }

    #[test]
    fn local_header_length_is_stable() {
        let m = meta("dir/ファイル.txt");
        for zip64 in [false, true] {
            let a = local_header(&m, &EntrySizes { zip64_local: zip64, ..Default::default() });
            let b = local_header(
                &m,
                &EntrySizes {
                    method: 8,
                    crc32: 0xdead_beef,
                    compressed: 123,
                    uncompressed: 456,
                    sha256: Some([7; 32]),
                    zip64_local: zip64,
                },
            );
            assert_eq!(a.len(), b.len());
            let p = read_local_header(&mut &b[..]).unwrap();
            assert_eq!(p.name, m.name.as_bytes());
            assert_eq!((p.crc32, p.compressed, p.uncompressed), (0xdead_beef, 123, 456));
            assert_eq!(p.sha256, Some([7; 32]));
            assert_eq!(p.zip64, zip64);
            assert_eq!(p.header_len as usize, b.len());
        }
    }

    #[test]
    fn dos_time_clamps() {
        assert_eq!(dos_datetime(Some((0, 0))), (0, 33));
        assert_eq!(dos_datetime(None), (0, 33));
    }

    #[test]
    fn zip64_threshold() {
        assert!(!needs_zip64_sizes(1 << 20));
        assert!(needs_zip64_sizes(0xffff_0000));
        assert!(needs_zip64_sizes(5 << 30));
    }
}
