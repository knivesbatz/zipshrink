//! The files embedded by build.rs, and a reader for them.

use std::io::Read;

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));

pub struct Entry {
    pub name: &'static str,
    /// Size once unpacked.
    pub size: u64,
    packed: &'static [u8],
}

impl Entry {
    /// Unpacks the file into memory.
    pub fn bytes(&self) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.size as usize);
        flate2::read::DeflateDecoder::new(self.packed).read_to_end(&mut out)?;
        if out.len() as u64 != self.size {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} is damaged in the installer", self.name)));
        }
        Ok(out)
    }
}

/// Every embedded file, in name order.
pub fn entries() -> Vec<Entry> {
    let mut out = Vec::new();
    let mut b = BLOB;
    let take = |b: &mut &'static [u8], n: usize| -> Option<&'static [u8]> {
        if b.len() < n {
            return None;
        }
        let (h, t) = b.split_at(n);
        *b = t;
        Some(h)
    };
    while !b.is_empty() {
        let Some(l) = take(&mut b, 2) else { break };
        let nl = u16::from_le_bytes([l[0], l[1]]) as usize;
        let (Some(name), Some(sz), Some(pl)) = (take(&mut b, nl), take(&mut b, 8), take(&mut b, 8)) else { break };
        let size = u64::from_le_bytes(sz.try_into().unwrap_or([0; 8]));
        let plen = u64::from_le_bytes(pl.try_into().unwrap_or([0; 8])) as usize;
        let Some(packed) = take(&mut b, plen) else { break };
        let Ok(name) = std::str::from_utf8(name) else { break };
        out.push(Entry { name, size, packed });
    }
    out
}

/// True when this build carries no files: a demonstration build.
pub fn is_demo() -> bool {
    BLOB.is_empty()
}

/// Space the installed files take.
pub fn installed_size() -> u64 {
    entries().iter().map(|e| e.size).sum()
}
