//! The recovery journal: an append-only, checksummed, fsynced log next to
//! the partial archive.
//!
//! Each line is `<crc32 of json, 8 hex digits> <json>\n`. A crash can leave
//! at most the final line torn; such a line is detected by its missing
//! newline or bad checksum and discarded. A bad line anywhere else means
//! the journal itself is damaged, and the job stops without deleting
//! anything further.

use crate::format::EntryMeta;
use crate::platform::{self, FileStat};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const JOURNAL_FORMAT: &str = "zipshrink-journal";
pub const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobOptions {
    /// 0 stores without compression; 1–9 are deflate levels.
    pub level: u32,
    pub delete_originals: bool,
    pub delete_readonly: bool,
    /// Bytes of free space that must remain on the destination volume.
    pub min_free: u64,
    /// Compare metadata only, instead of re-hashing, before deletion.
    pub quick_change_check: bool,
    pub overwrite: bool,
    /// Decompress every entry again after the central directory is written.
    pub final_data_verify: bool,
    /// Worker threads that pre-compress small files (1 = no parallelism).
    pub threads: u32,
    /// Testing aid: force ZIP64 headers for every entry.
    #[serde(default)]
    pub force_zip64: bool,
    /// Entries per commit batch; 0 uses the default (1024). Smaller batches
    /// sync more often and free space sooner.
    #[serde(default)]
    pub batch_entries: u32,
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            level: 6,
            delete_originals: false,
            delete_readonly: false,
            min_free: crate::DEFAULT_MIN_FREE,
            quick_change_check: false,
            overwrite: false,
            final_data_verify: true,
            threads: 1,
            force_zip64: false,
            batch_entries: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub format: String,
    pub version: u32,
    pub job_id: String,
    pub created: i64,
    pub output: PathBuf,
    pub sources: Vec<PathBuf>,
    pub options: JobOptions,
    pub app_version: String,
}

/// One planned archive entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanEntry {
    pub idx: u64,
    pub src: PathBuf,
    /// Index of the source root this entry came from.
    pub root: u32,
    pub meta: EntryMeta,
    /// Size at planning time (progress only; the real size is re-read).
    pub size: u64,
    /// Directory identity at planning time (directories only).
    #[serde(default)]
    pub dir_stat: Option<FileStat>,
}

/// An entry whose data is on disk, synced, and verified by reading it back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Commit {
    pub idx: u64,
    pub offset: u64,
    pub end: u64,
    pub method: u16,
    pub crc32: u32,
    pub csize: u64,
    pub usize: u64,
    #[serde(with = "hex_opt")]
    pub sha256: Option<[u8; 32]>,
    pub zip64_local: bool,
    /// The source as it was when compressed; deletion requires a match.
    pub src_stat: Option<FileStat>,
    /// Metadata as written into the headers.
    pub meta: EntryMeta,
    /// CRC-32 of the local header bytes exactly as written.
    #[serde(default)]
    pub header_crc: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Record {
    Header(Header),
    /// A path found while planning that will not be archived.
    ScanSkip {
        path: PathBuf,
        reason: String,
    },
    Plan(PlanEntry),
    PlanDone {
        entries: u64,
        files: u64,
        dirs: u64,
        bytes: u64,
    },
    Commit(Commit),
    /// The original was deleted (or found already gone).
    Deleted {
        idx: u64,
        missing: bool,
    },
    /// The original was deliberately kept.
    Kept {
        idx: u64,
        reason: String,
    },
    /// The file could not be archived in this run (it stays in place and
    /// is retried on resume).
    Failed {
        idx: u64,
        reason: String,
    },
    FinalizeStart {
        cd_offset: u64,
        entries: u64,
    },
    Finalized {
        path: PathBuf,
    },
}

mod hex_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_some(&hex::encode(b)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let o: Option<String> = Option::deserialize(d)?;
        match o {
            None => Ok(None),
            Some(s) => {
                let v = hex::decode(&s).map_err(serde::de::Error::custom)?;
                let a: [u8; 32] = v.try_into().map_err(|_| serde::de::Error::custom("bad hash length"))?;
                Ok(Some(a))
            }
        }
    }
}

pub fn journal_path_for(partial: &Path) -> PathBuf {
    let mut s = partial.as_os_str().to_owned();
    s.push(".journal");
    PathBuf::from(s)
}

pub fn encode_line(r: &Record) -> Vec<u8> {
    let json = serde_json::to_vec(r).expect("journal records always serialize");
    let crc = crc32fast::hash(&json);
    let mut line = format!("{crc:08x} ").into_bytes();
    line.extend_from_slice(&json);
    line.push(b'\n');
    line
}

/// Appends records; nothing is durable until [`JournalWriter::sync`].
pub struct JournalWriter {
    file: File,
    path: PathBuf,
    buf: Vec<u8>,
    len: u64,
}

impl JournalWriter {
    /// Creates a new journal; fails if one exists.
    pub fn create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        lock(&file)?;
        Ok(JournalWriter { file, path: path.to_owned(), buf: Vec::new(), len: 0 })
    }

    /// Opens an existing journal for appending after `valid_len` bytes,
    /// cutting off any torn tail.
    pub fn reopen(path: &Path, valid_len: u64) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        lock(&file)?;
        if file.metadata()?.len() != valid_len {
            file.set_len(valid_len)?;
            platform::sync_file(&file)?;
        }
        Ok(JournalWriter { file, path: path.to_owned(), buf: Vec::new(), len: valid_len })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, r: &Record) {
        self.buf.extend_from_slice(&encode_line(r));
    }

    /// Writes buffered records and flushes them to stable storage.
    pub fn sync(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            crate::failpoint::hit_torn_journal(&self.file, self.len, &self.buf)?;
            platform::pwrite_all(&self.file, &self.buf, self.len)?;
            self.len += self.buf.len() as u64;
            self.buf.clear();
        }
        platform::sync_file(&self.file)
    }

    /// Releases the lock and closes the file.
    pub fn close(self) {
        let _ = self.file.unlock();
    }
}

fn lock(f: &File) -> io::Result<()> {
    match f.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(io::Error::new(io::ErrorKind::ResourceBusy, "this job is being processed by another ZipShrink window or process"))
        }
        Err(std::fs::TryLockError::Error(e)) if e.kind() == io::ErrorKind::Unsupported => Ok(()),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The parsed contents of a journal.
#[derive(Debug)]
pub struct Loaded {
    pub records: Vec<Record>,
    /// Bytes of the journal made of complete, valid records.
    pub valid_len: u64,
    /// True if a torn final record was found and ignored.
    pub torn_tail: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot read the journal: {0}")]
    Io(#[from] io::Error),
    #[error("the journal is damaged at byte {offset}: {why}")]
    Damaged { offset: u64, why: String },
    #[error("this is not a ZipShrink journal, or it was written by a newer version")]
    NotJournal,
}

pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let mut data = Vec::new();
    File::open(path)?.read_to_end(&mut data)?;
    let mut records = Vec::new();
    let mut pos = 0usize;
    let mut torn_tail = false;
    while pos < data.len() {
        let Some(nl) = data[pos..].iter().position(|&b| b == b'\n') else {
            torn_tail = true; // unterminated final line
            break;
        };
        let line = &data[pos..pos + nl];
        let parsed = parse_line(line);
        let is_last = pos + nl + 1 >= data.len();
        match parsed {
            Ok(r) => records.push(r),
            Err(why) if is_last => {
                let _ = why;
                torn_tail = true;
                break;
            }
            Err(why) => return Err(LoadError::Damaged { offset: pos as u64, why }),
        }
        pos += nl + 1;
    }
    match records.first() {
        Some(Record::Header(h)) if h.format == JOURNAL_FORMAT && h.version <= JOURNAL_VERSION => {}
        None if torn_tail || data.is_empty() => {}
        _ => return Err(LoadError::NotJournal),
    }
    Ok(Loaded { records, valid_len: pos as u64, torn_tail })
}

fn parse_line(line: &[u8]) -> Result<Record, String> {
    if line.len() < 10 || line[8] != b' ' {
        return Err("malformed record".into());
    }
    let crc_text = std::str::from_utf8(&line[..8]).map_err(|_| "malformed checksum")?;
    let crc = u32::from_str_radix(crc_text, 16).map_err(|_| "malformed checksum")?;
    let json = &line[9..];
    if crc32fast::hash(json) != crc {
        return Err("checksum mismatch".into());
    }
    serde_json::from_slice(json).map_err(|e| format!("unreadable record: {e}"))
}

pub fn write_all_sync(f: &mut File, data: &[u8]) -> io::Result<()> {
    f.write_all(data)?;
    platform::sync_file(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Record {
        Record::Header(Header {
            format: JOURNAL_FORMAT.into(),
            version: 1,
            job_id: "x".into(),
            created: 0,
            output: "/o.zip".into(),
            sources: vec![],
            options: JobOptions::default(),
            app_version: "t".into(),
        })
    }

    #[test]
    fn torn_tail_is_ignored_and_middle_damage_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("j");
        let mut w = JournalWriter::create(&p).unwrap();
        w.append(&header());
        w.append(&Record::Kept { idx: 1, reason: "r".into() });
        w.sync().unwrap();
        w.close();
        let good = std::fs::read(&p).unwrap();

        let mut torn = good.clone();
        torn.extend_from_slice(b"0000abcd {\"t\":\"kep");
        std::fs::write(&p, &torn).unwrap();
        let l = load(&p).unwrap();
        assert!(l.torn_tail);
        assert_eq!(l.records.len(), 2);
        assert_eq!(l.valid_len, good.len() as u64);

        let mut bad = good.clone();
        let i = bad.iter().position(|&b| b == b'\n').unwrap() + 15;
        bad[i] ^= 1;
        bad.extend_from_slice(&encode_line(&Record::Kept { idx: 2, reason: "r".into() }));
        std::fs::write(&p, &bad).unwrap();
        assert!(matches!(load(&p), Err(LoadError::Damaged { .. })));
    }
}
