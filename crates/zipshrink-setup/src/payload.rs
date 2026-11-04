//! The files embedded by build.rs, and a reader for them.
//!
//! Format (see build.rs): for each file, u16 name length (LE), name (UTF-8),
//! u64 original length (LE), u64 compressed length (LE), raw-deflate data.

use std::io::{Read, Write};

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));

pub struct Entry {
    pub name: &'static str,
    /// Size once unpacked.
    pub size: u64,
    packed: &'static [u8],
}

impl Entry {
    /// Unpacks the file into memory.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn bytes(&self) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.size as usize);
        self.unpack_to(&mut out)?;
        Ok(out)
    }

    /// Unpacks the file into `w`, checking that it comes out whole.
    pub fn unpack_to(&self, w: &mut dyn Write) -> std::io::Result<()> {
        // One byte more than expected is enough to notice a file too long.
        let mut r = flate2::read::DeflateDecoder::new(self.packed).take(self.size + 1);
        let n = std::io::copy(&mut r, w)?;
        if n != self.size {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} is damaged in the installer", self.name)));
        }
        Ok(())
    }
}

/// Reads a payload blob; `Err` when it is cut short or malformed.
fn parse(blob: &'static [u8]) -> Result<Vec<Entry>, String> {
    let damaged = || "the installer is damaged (its files are incomplete); download it again".to_owned();
    let mut out = Vec::new();
    let mut b = blob;
    let take = |b: &mut &'static [u8], n: usize| -> Option<&'static [u8]> {
        if b.len() < n {
            return None;
        }
        let (h, t) = b.split_at(n);
        *b = t;
        Some(h)
    };
    let u64_at = |s: &[u8]| u64::from_le_bytes(s.try_into().unwrap_or([0; 8]));
    while !b.is_empty() {
        let l = take(&mut b, 2).ok_or_else(damaged)?;
        let nl = u16::from_le_bytes([l[0], l[1]]) as usize;
        let name = take(&mut b, nl).ok_or_else(damaged)?;
        let size = u64_at(take(&mut b, 8).ok_or_else(damaged)?);
        let plen = usize::try_from(u64_at(take(&mut b, 8).ok_or_else(damaged)?)).map_err(|_| damaged())?;
        let packed = take(&mut b, plen).ok_or_else(damaged)?;
        let name = std::str::from_utf8(name).map_err(|_| damaged())?;
        out.push(Entry { name, size, packed });
    }
    Ok(out)
}

/// Every embedded file, in name order, or why they cannot be read.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn checked_entries() -> Result<Vec<Entry>, String> {
    parse(BLOB)
}

/// Every embedded file, in name order (none if the payload is damaged).
pub fn entries() -> Vec<Entry> {
    parse(BLOB).unwrap_or_default()
}

/// True when this build carries no files: a demonstration build.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_demo() -> bool {
    BLOB.is_empty()
}

/// Space the installed files take.
pub fn installed_size() -> u64 {
    entries().iter().map(|e| e.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer half, exactly as build.rs packs files.
    fn pack(files: &[(&str, &[u8])]) -> &'static [u8] {
        let mut blob = Vec::new();
        for (name, data) in files {
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(data).unwrap();
            let packed = enc.finish().unwrap();
            blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
            blob.extend_from_slice(name.as_bytes());
            blob.extend_from_slice(&(data.len() as u64).to_le_bytes());
            blob.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            blob.extend_from_slice(&packed);
        }
        Vec::leak(blob)
    }

    #[test]
    fn round_trip() {
        let big: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let files: [(&str, &[u8]); 4] =
            [("LICENSE.txt", b"MIT License\r\n"), ("empty.txt", b""), ("zipshrink-gui.exe", &big), ("\u{fc}nicode.txt", b"\xc3\xbc")];
        let entries = parse(pack(&files)).unwrap();
        assert_eq!(entries.len(), files.len());
        for (e, (name, data)) in entries.iter().zip(files) {
            assert_eq!(e.name, name);
            assert_eq!(e.size, data.len() as u64);
            assert_eq!(e.bytes().unwrap(), data);
            let mut streamed = Vec::new();
            e.unpack_to(&mut streamed).unwrap();
            assert_eq!(streamed, data);
        }
        assert!(parse(&[]).unwrap().is_empty());
    }

    #[test]
    fn cut_short_is_an_error() {
        let first = pack(&[("a.txt", b"hello hello hello")]);
        let blob = pack(&[("a.txt", b"hello hello hello"), ("b.txt", b"world")]);
        for n in 1..blob.len() {
            // Cut exactly after a whole entry, the rest just looks absent.
            if n != first.len() {
                assert!(parse(&blob[..n]).is_err(), "cut at {n}");
            }
        }
        assert_eq!(parse(&blob[..first.len()]).unwrap().len(), 1);
    }

    #[test]
    fn wrong_size_is_damage() {
        let blob = pack(&[("a.txt", b"hello hello hello")]).to_vec();
        for size in [3u64, 100] {
            let mut b = blob.clone();
            b[2 + 5..2 + 5 + 8].copy_from_slice(&size.to_le_bytes());
            let e = parse(Vec::leak(b)).unwrap().pop().unwrap();
            assert!(e.bytes().is_err(), "claimed {size}");
        }
    }
}
