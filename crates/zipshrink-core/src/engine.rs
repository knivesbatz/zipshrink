//! The job: plan, compress, verify, commit, delete, finalize, resume.
//!
//! Transaction for every batch of entries (see docs/DATA_SAFETY.md):
//!
//! 1. each source is opened (no links followed; on Windows no writers
//!    allowed), read once while computing CRC-32 and SHA-256, compressed
//!    and appended to `<name>.zip.partial`; its local header is patched
//!    with the final sizes and checksum; the file's identity and
//!    timestamps are compared before and after reading;
//! 2. the partial archive is flushed to storage (`fsync` / `FlushFileBuffers`);
//! 3. every entry of the batch is read back from the device, its header
//!    parsed and its data decompressed and matched against the recorded
//!    CRC-32 and SHA-256;
//! 4. commit records go to the journal, which is flushed to storage;
//! 5. only then, if deletion was explicitly enabled, each original is
//!    re-checked (identity, size, times and, unless disabled, SHA-256) and
//!    deleted, and the deletion is recorded.
//!
//! Anything not committed is cut off the partial archive on resume. The
//! central directory is written only at the end, verified with an
//! independent reader, and the partial is renamed to the final name.

use crate::error::{Error, Result};
use crate::failpoint;
use crate::format::{self, EntryMeta, EntrySizes, METHOD_DEFLATE, METHOD_STORE};
use crate::journal::{self, Commit, Header, JobOptions, JournalWriter, PlanEntry, Record};
use crate::platform::{self, DeleteOutcome, FileStat, ReadbackMode};
use crate::progress::{Current, EntryStatus, Event, Observer, Phase, Stats};
use crate::scan::{self, ScanResult};
use crate::verify::{self, ArchiveCheck, Expected};
use crossbeam_channel::{Receiver, Sender};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Flush (sync, verify, commit, delete) after this much source data...
const BATCH_BYTES: u64 = 64 << 20;
/// ...or this many entries...
const BATCH_ENTRIES: usize = 1024;
/// ...or this much time.
const BATCH_TIME: Duration = Duration::from_secs(3);
/// Free space is re-checked after every this many bytes written.
const SPACE_CHECK_EVERY: u64 = 16 << 20;
/// Files up to this size may be compressed by worker threads in memory.
const SMALL_FILE: u64 = 2 << 20;
const IO_BUF: usize = 1 << 20;
/// Attempts to read a file that keeps changing.
const CHANGE_RETRIES: usize = 3;

pub struct CreateRequest {
    pub sources: Vec<PathBuf>,
    pub output: PathBuf,
    pub options: JobOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPaths {
    pub output: PathBuf,
    pub partial: PathBuf,
    pub journal: PathBuf,
}

impl JobPaths {
    pub fn for_output(output: &Path) -> Self {
        let mut p = output.as_os_str().to_owned();
        p.push(".partial");
        let partial = PathBuf::from(p);
        let journal = journal::journal_path_for(&partial);
        JobPaths { output: output.to_owned(), partial, journal }
    }

    /// Accepts the final archive name, the `.partial` file or the journal.
    pub fn from_any(p: &Path) -> Self {
        let s = p.as_os_str().to_string_lossy();
        if let Some(base) = s.strip_suffix(".partial.journal") {
            return Self::for_output(Path::new(base));
        }
        if let Some(base) = s.strip_suffix(".partial") {
            return Self::for_output(Path::new(base));
        }
        Self::for_output(p)
    }

    pub fn dest_dir(&self) -> PathBuf {
        self.output.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    }

    /// Whether an unfinished job exists for this output.
    pub fn has_unfinished_job(&self) -> bool {
        self.journal.exists() || self.partial.exists()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Finished; every planned entry archived and verified.
    Completed,
    /// Finished, but some files were skipped, failed or kept.
    CompletedWithIssues,
    /// Stopped before finishing. Committed data is safe in the partial archive.
    Stopped { error: String, resumable: bool, kind: StopKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopKind {
    Cancelled,
    LowSpace,
    DiskFull,
    Damaged,
    VerifyFailed,
    Other,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub outcome: Outcome,
    pub paths: JobPaths,
    pub archive_size: u64,
    /// Uncompressed bytes of archived files.
    pub original_bytes: u64,
    pub reclaimed: u64,
    pub files_archived: u64,
    pub dirs_archived: u64,
    pub originals_deleted: u64,
    pub delete_requested: bool,
    pub kept: Vec<(String, String)>,
    pub failed: Vec<(PathBuf, String)>,
    pub skipped: Vec<(PathBuf, String)>,
    pub dirs_removed: u64,
    pub verification: Option<ArchiveCheck>,
    pub readback: Option<ReadbackMode>,
    pub elapsed: Duration,
    pub warnings: Vec<String>,
    /// Entries safely held in the partial archive of a stopped job.
    pub recoverable_entries: u64,
}

impl Report {
    pub fn finished(&self) -> bool {
        matches!(self.outcome, Outcome::Completed | Outcome::CompletedWithIssues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Deletion {
    Deleted,
    Missing,
    Kept(String),
}

struct Archive {
    file: File,
    path: PathBuf,
    len: u64,
}

impl Archive {
    fn append(&mut self, buf: &[u8]) -> io::Result<()> {
        failpoint::hit("archive_write")?;
        platform::pwrite_all(&self.file, buf, self.len)?;
        self.len += buf.len() as u64;
        Ok(())
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)?;
        self.len = len;
        Ok(())
    }
}

/// Why one entry could not be written.
enum EntryErr {
    /// The source could not be read; the file is left alone.
    Source(String),
    /// The source changed while being read.
    Changed(String),
    /// Too big for in-memory compression (grew since planning).
    TooLarge,
    /// Stop the job.
    Stop(Error),
}

/// A small file compressed in memory by a worker thread.
struct Prepared {
    data: Vec<u8>,
    method: u16,
    crc32: u32,
    usize: u64,
    sha256: [u8; 32],
    stat: FileStat,
}

struct Shared<'a> {
    cancel: &'a AtomicBool,
    dest_dir: PathBuf,
    min_free: u64,
}

pub struct Job {
    pub paths: JobPaths,
    header: Header,
    plan: Vec<PlanEntry>,
    commits: Vec<Commit>,
    /// plan idx -> position in `commits`
    committed: HashMap<u64, usize>,
    failed: HashMap<u64, String>,
    deletion: HashMap<u64, Deletion>,
    scan_skips: Vec<(PathBuf, String)>,
    journal: Option<JournalWriter>,
    archive: Option<Archive>,
    finalized: bool,
    resumed: bool,
    stats: Stats,
    last_tick: Instant,
    speed_mark: (Instant, u64),
    started: Instant,
    readback: Option<ReadbackMode>,
    verification: Option<ArchiveCheck>,
    dirs_removed: u64,
    pub warnings: Vec<String>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn job_id() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("{:x}-{:x}", t.as_nanos(), std::process::id())
}

fn absolute_output(output: &Path) -> Result<PathBuf> {
    let name = output.file_name().ok_or_else(|| Error::Invalid(format!("{} is not a valid archive file name", output.display())))?;
    let parent = match output.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let parent = dunce::canonicalize(&parent).map_err(|e| Error::io(&parent, e))?;
    if !parent.is_dir() {
        return Err(Error::Invalid(format!("{} is not a folder", parent.display())));
    }
    Ok(parent.join(name))
}

/// Validates a request and returns the resolved output path and roots,
/// without touching the disk.
pub fn validate(req: &CreateRequest) -> Result<(JobPaths, Vec<PathBuf>)> {
    if req.options.level > 9 {
        return Err(Error::Invalid("compression level must be 0 to 9".into()));
    }
    let output = absolute_output(&req.output)?;
    let paths = JobPaths::for_output(&output);
    let roots = scan::canonical_roots(&req.sources)?;
    for r in &roots {
        if scan::is_within(&output, r) {
            return Err(Error::Invalid(format!(
                "the archive cannot be saved inside {}, which is being compressed; choose another destination",
                r.display()
            )));
        }
    }
    Ok((paths, roots))
}

impl Job {
    /// Plans a new job: scans the sources, writes the journal and creates
    /// the empty partial archive. Nothing is compressed or deleted yet.
    pub fn create(req: &CreateRequest, obs: &mut dyn Observer, cancel: &AtomicBool) -> Result<Job> {
        let (paths, roots) = validate(req)?;
        obs.on_event(Event::Phase(Phase::Scanning));
        let exclude = vec![paths.output.clone(), paths.partial.clone(), paths.journal.clone()];
        let scan = scan::scan(&roots, &exclude, cancel)?;
        Self::create_from_scan(req, scan)
    }

    /// Like [`Job::create`], reusing a scan made for the preflight check.
    pub fn create_from_scan(req: &CreateRequest, scan: ScanResult) -> Result<Job> {
        let (paths, roots) = validate(req)?;
        if roots != scan.roots {
            return Err(Error::Invalid("the selection changed after it was checked".into()));
        }
        if paths.journal.exists() || paths.partial.exists() {
            return Err(Error::JobExists { partial: paths.partial.clone() });
        }
        match std::fs::symlink_metadata(&paths.output) {
            Ok(m) if m.is_dir() => {
                return Err(Error::Invalid(format!("{} is a folder", paths.output.display())));
            }
            Ok(_) if !req.options.overwrite => return Err(Error::OutputExists(paths.output.clone())),
            _ => {}
        }
        if scan.entries.is_empty() {
            return Err(Error::Invalid("there is nothing to archive in the selection".into()));
        }
        let mut options = req.options.clone();
        options.min_free = options.min_free.max(crate::MIN_FREE_FLOOR);
        options.threads = options.threads.clamp(1, 64);
        let header = Header {
            format: journal::JOURNAL_FORMAT.into(),
            version: journal::JOURNAL_VERSION,
            job_id: job_id(),
            created: now_unix(),
            output: paths.output.clone(),
            sources: roots,
            options,
            app_version: crate::VERSION.into(),
        };
        // The header is written to a temporary file and renamed into place,
        // so a journal either does not exist or starts with a whole header.
        let mut tmp = paths.journal.as_os_str().to_owned();
        tmp.push(".new");
        let tmp = PathBuf::from(tmp);
        if tmp.exists() {
            // Left by a create that crashed before the rename: it holds at
            // most a header, never any record of archived data.
            match journal::load(&tmp) {
                Ok(l) if l.records.iter().all(|r| matches!(r, Record::Header(_))) => {
                    std::fs::remove_file(&tmp).map_err(|e| Error::io(&tmp, e))?
                }
                _ => return Err(Error::JobExists { partial: paths.partial.clone() }),
            }
        }
        let mut jw = JournalWriter::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        jw.append(&Record::Header(header.clone()));
        jw.sync().map_err(|e| Error::io(&tmp, e))?;
        let header_len = std::fs::metadata(&tmp).map_err(|e| Error::io(&tmp, e))?.len();
        jw.close();
        platform::rename_noreplace(&tmp, &paths.journal).map_err(|e| Error::io(&paths.journal, e))?;
        platform::sync_dir(&paths.dest_dir()).map_err(|e| Error::io(paths.dest_dir(), e))?;
        let mut jw = JournalWriter::reopen(&paths.journal, header_len).map_err(|e| Error::io(&paths.journal, e))?;
        for (path, reason) in &scan.skipped {
            jw.append(&Record::ScanSkip { path: path.clone(), reason: reason.clone() });
        }
        for e in &scan.entries {
            jw.append(&Record::Plan(e.clone()));
        }
        failpoint::hit("before_plan_done").map_err(|e| Error::io(&paths.journal, e))?;
        jw.append(&Record::PlanDone { entries: scan.entries.len() as u64, files: scan.files, dirs: scan.dirs, bytes: scan.bytes });
        jw.sync().map_err(|e| Error::io(&paths.journal, e))?;
        let file =
            OpenOptions::new().read(true).write(true).create_new(true).open(&paths.partial).map_err(|e| Error::io(&paths.partial, e))?;
        platform::sync_file(&file).map_err(|e| Error::io(&paths.partial, e))?;
        platform::sync_dir(&paths.dest_dir()).map_err(|e| Error::io(paths.dest_dir(), e))?;
        let mut job = Job::empty(paths, header);
        job.plan = scan.entries;
        job.scan_skips = scan.skipped;
        job.warnings = scan.warnings;
        job.journal = Some(jw);
        job.archive = Some(Archive { file, path: job.paths.partial.clone(), len: 0 });
        job.init_stats();
        Ok(job)
    }

    fn empty(paths: JobPaths, header: Header) -> Job {
        let now = Instant::now();
        Job {
            paths,
            header,
            plan: Vec::new(),
            commits: Vec::new(),
            committed: HashMap::new(),
            failed: HashMap::new(),
            deletion: HashMap::new(),
            scan_skips: Vec::new(),
            journal: None,
            archive: None,
            finalized: false,
            resumed: false,
            stats: Stats::default(),
            last_tick: now - Duration::from_secs(1),
            speed_mark: (now, 0),
            started: now,
            readback: None,
            verification: None,
            dirs_removed: 0,
            warnings: Vec::new(),
        }
    }

    pub fn options(&self) -> &JobOptions {
        &self.header.options
    }

    pub fn sources(&self) -> &[PathBuf] {
        &self.header.sources
    }

    /// Turns deletion off for the rest of this run.
    pub fn disable_deletion(&mut self) {
        self.header.options.delete_originals = false;
        self.stats.delete_originals = false;
    }

    /// Changes the free-space reserve for the rest of this run.
    pub fn set_min_free(&mut self, bytes: u64) {
        self.header.options.min_free = bytes.max(crate::MIN_FREE_FLOOR);
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Entries planned for this job, in archive order.
    pub fn plan(&self) -> &[PlanEntry] {
        &self.plan
    }

    /// Opens an interrupted job for resuming. `path` may be the archive
    /// name, the `.partial` file or its journal. The partial archive is
    /// checked against the journal and any uncommitted tail is cut off.
    pub fn open(path: &Path) -> Result<Job> {
        let paths = JobPaths::from_any(path);
        let paths = match absolute_output(&paths.output) {
            Ok(o) => JobPaths::for_output(&o),
            Err(_) => paths,
        };
        if !paths.journal.exists() {
            if paths.partial.exists() {
                return Err(Error::Damaged(format!(
                    "{} has no journal next to it. Nothing more will be deleted. Run `zipshrink salvage` on it to rebuild a readable archive from the entries it holds.",
                    paths.partial.display()
                )));
            }
            return Err(Error::Invalid(format!("no unfinished job found for {}", paths.output.display())));
        }
        let loaded = journal::load(&paths.journal)?;
        let Some(Record::Header(header)) = loaded.records.first().cloned() else {
            return Err(Error::Invalid(
                "the job was interrupted before it started; nothing was changed. Delete the .partial and .journal files and start again."
                    .into(),
            ));
        };
        let mut job = Job::empty(paths.clone(), header);
        job.resumed = true;
        let mut plan_done = false;
        let mut finalize_started = false;
        let mut header_len = 0u64;
        {
            // Length of the header line, to rewind to when re-planning.
            let first = journal::encode_line(&loaded.records[0]);
            header_len += first.len() as u64;
        }
        for r in loaded.records.into_iter().skip(1) {
            match r {
                Record::Header(_) => return Err(Error::Damaged("the journal has two headers".into())),
                Record::ScanSkip { path, reason } => job.scan_skips.push((path, reason)),
                Record::Plan(p) => {
                    if p.idx != job.plan.len() as u64 {
                        return Err(Error::Damaged("the journal's plan is out of order".into()));
                    }
                    job.plan.push(p)
                }
                Record::PlanDone { .. } => plan_done = true,
                Record::Commit(c) => {
                    let Some(pe) = job.plan.get(c.idx as usize) else {
                        return Err(Error::Damaged("the journal commits an entry it never planned".into()));
                    };
                    if c.meta.name != pe.meta.name || job.committed.contains_key(&c.idx) {
                        return Err(Error::Damaged("the journal's commits are inconsistent".into()));
                    }
                    if let Some(last) = job.commits.last() {
                        if c.offset != last.end {
                            return Err(Error::Damaged("the journal's commits are not contiguous".into()));
                        }
                    } else if c.offset != 0 {
                        return Err(Error::Damaged("the journal's first commit does not start the archive".into()));
                    }
                    job.failed.remove(&c.idx);
                    job.committed.insert(c.idx, job.commits.len());
                    job.commits.push(c);
                }
                Record::Deleted { idx, missing } => {
                    job.deletion.insert(idx, if missing { Deletion::Missing } else { Deletion::Deleted });
                }
                Record::Kept { idx, reason } => {
                    job.deletion.insert(idx, Deletion::Kept(reason));
                }
                Record::Failed { idx, reason } => {
                    job.failed.insert(idx, reason);
                }
                Record::FinalizeStart { .. } => finalize_started = true,
                Record::Finalized { .. } => job.finalized = true,
            }
        }
        // Failures are retried on resume; keep the reasons only for display.
        let mut jw = JournalWriter::reopen(&paths.journal, if plan_done { loaded.valid_len } else { header_len }).map_err(|e| {
            if e.kind() == io::ErrorKind::ResourceBusy {
                Error::Busy(e.to_string())
            } else {
                Error::io(&paths.journal, e)
            }
        })?;
        if !plan_done {
            // Interrupted while planning: nothing was written to the archive.
            job.plan.clear();
            job.scan_skips.clear();
            let roots = job.header.sources.clone();
            let exclude = vec![paths.output.clone(), paths.partial.clone(), paths.journal.clone()];
            let scan = scan::scan(&roots, &exclude, &AtomicBool::new(false))?;
            for (path, reason) in &scan.skipped {
                jw.append(&Record::ScanSkip { path: path.clone(), reason: reason.clone() });
            }
            for e in &scan.entries {
                jw.append(&Record::Plan(e.clone()));
            }
            jw.append(&Record::PlanDone { entries: scan.entries.len() as u64, files: scan.files, dirs: scan.dirs, bytes: scan.bytes });
            jw.sync().map_err(|e| Error::io(&paths.journal, e))?;
            job.plan = scan.entries;
            job.scan_skips = scan.skipped;
            job.warnings = scan.warnings;
        }
        job.journal = Some(jw);
        let committed_end = job.commits.last().map(|c| c.end).unwrap_or(0);
        if job.finalized {
            // Only cleanup remained.
        } else if paths.partial.exists() {
            let file = OpenOptions::new().read(true).write(true).open(&paths.partial).map_err(|e| Error::io(&paths.partial, e))?;
            let len = file.metadata().map_err(|e| Error::io(&paths.partial, e))?.len();
            if len < committed_end {
                return Err(Error::Damaged(format!(
                    "{} is shorter ({} bytes) than the {} bytes it held when last committed. Keep the file and run `zipshrink salvage` to recover the entries it still contains.",
                    paths.partial.display(),
                    len,
                    committed_end
                )));
            }
            // Cheap structural check of every committed header, byte for byte.
            for c in &job.commits {
                let hlen = (c.end - c.offset).saturating_sub(c.csize) as usize;
                let mut hdr = vec![0u8; hlen];
                platform::pread_exact(&file, &mut hdr, c.offset).map_err(|e| Error::io(&paths.partial, e))?;
                let ok = match c.header_crc {
                    Some(crc) => crc32fast::hash(&hdr) == crc,
                    None => format::read_local_header(&mut &hdr[..])
                        .is_ok_and(|p| p.name == c.meta.name.as_bytes() && p.crc32 == c.crc32 && p.header_len as usize == hlen),
                };
                if !ok {
                    return Err(Error::Damaged(format!(
                        "the entry for \"{}\" at offset {} no longer matches the journal. Keep both files and run `zipshrink salvage`.",
                        c.meta.name, c.offset
                    )));
                }
            }
            let mut archive = Archive { file, path: paths.partial.clone(), len };
            if len != committed_end {
                archive.truncate(committed_end).map_err(|e| Error::io(&paths.partial, e))?;
                platform::sync_file(&archive.file).map_err(|e| Error::io(&paths.partial, e))?;
            }
            job.archive = Some(archive);
        } else if finalize_started && paths.output.exists() {
            // Crashed right after the final rename: confirm and record it.
            let check = verify::check_archive(&paths.output, true, &AtomicBool::new(false), &mut |_| {})
                .map_err(|e| Error::io(&paths.output, e))?;
            job.cross_check(&paths.output, &check)?;
            job.verification = Some(check);
            let jw = job.journal.as_mut().unwrap();
            jw.append(&Record::Finalized { path: paths.output.clone() });
            jw.sync().map_err(|e| Error::io(&paths.journal, e))?;
            job.finalized = true;
        } else if job.commits.is_empty() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&paths.partial)
                .map_err(|e| Error::io(&paths.partial, e))?;
            job.archive = Some(Archive { file, path: paths.partial.clone(), len: 0 });
        } else {
            return Err(Error::Damaged(format!(
                "{} is missing although the journal records {} archived entries. If you moved it, put it back next to the journal and resume again.",
                paths.partial.display(),
                job.commits.len()
            )));
        }
        job.init_stats();
        Ok(job)
    }

    fn init_stats(&mut self) {
        let s = &mut self.stats;
        s.total_files = self.plan.iter().filter(|p| !p.meta.is_dir).count() as u64;
        s.total_dirs = self.plan.len() as u64 - s.total_files;
        s.total_bytes = self.plan.iter().map(|p| p.size).sum();
        s.files_skipped = self.scan_skips.len() as u64;
        s.delete_originals = self.header.options.delete_originals;
        for c in &self.commits {
            if c.meta.is_dir {
                s.dirs_archived += 1;
            } else {
                s.files_archived += 1;
                s.bytes_committed += c.usize;
                s.compressed_committed += c.csize;
                s.bytes_processed += self.plan[c.idx as usize].size;
            }
        }
        for (idx, d) in &self.deletion {
            match d {
                Deletion::Deleted => {
                    s.originals_deleted += 1;
                    if let Some(&i) = self.committed.get(idx) {
                        s.reclaimed += self.commits[i].usize;
                    }
                }
                Deletion::Kept(_) => s.originals_kept += 1,
                Deletion::Missing => {}
            }
        }
        s.archive_size = self.archive.as_ref().map(|a| a.len).unwrap_or(0);
    }

    fn tick(&mut self, obs: &mut dyn Observer, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_tick) < Duration::from_millis(100) {
            return;
        }
        self.last_tick = now;
        let (t0, b0) = self.speed_mark;
        let dt = now.duration_since(t0).as_secs_f64();
        if dt >= 0.5 {
            let inst = self.stats.bytes_processed.saturating_sub(b0) as f64 / dt;
            self.stats.speed = if self.stats.speed == 0.0 { inst } else { self.stats.speed * 0.6 + inst * 0.4 };
            self.speed_mark = (now, self.stats.bytes_processed);
        }
        self.stats.elapsed = now.duration_since(self.started);
        if let Some(a) = &self.archive {
            self.stats.archive_size = a.len;
        }
        self.stats.free_space = platform::available_space(&self.paths.dest_dir()).ok();
        obs.on_event(Event::Progress(self.stats.clone()));
    }

    fn phase(&mut self, obs: &mut dyn Observer, p: Phase) {
        self.stats.phase = p;
        obs.on_event(Event::Phase(p));
        self.tick(obs, true);
    }

    fn status(&self, obs: &mut dyn Observer, idx: u64, status: EntryStatus) {
        let p = &self.plan[idx as usize];
        obs.on_event(Event::Entry { idx, name: p.meta.name.clone(), size: p.size, status });
    }

    fn jw(&mut self) -> &mut JournalWriter {
        self.journal.as_mut().expect("journal is open while the job runs")
    }

    fn sync_journal(&mut self) -> Result<()> {
        let p = self.paths.journal.clone();
        self.jw().sync().map_err(|e| Error::io(p, e))
    }

    /// Runs the job to completion or until it must stop. Never panics on
    /// I/O problems; the report says what happened and what to do next.
    pub fn run(&mut self, obs: &mut dyn Observer, cancel: &AtomicBool) -> Report {
        self.started = Instant::now();
        let res = self.run_inner(obs, cancel);
        let outcome = match res {
            Ok(()) => {
                let issues = !self.failed.is_empty()
                    || !self.scan_skips.is_empty()
                    || self.deletion.values().any(|d| matches!(d, Deletion::Kept(_)))
                    || self.committed.len() < self.plan.len();
                if issues {
                    Outcome::CompletedWithIssues
                } else {
                    Outcome::Completed
                }
            }
            Err(e) => {
                let kind = match &e {
                    Error::Cancelled => StopKind::Cancelled,
                    Error::LowSpace(_) => StopKind::LowSpace,
                    Error::DiskFull(_) => StopKind::DiskFull,
                    Error::Damaged(_) | Error::Journal(_) => StopKind::Damaged,
                    Error::VerifyFailed(_) => StopKind::VerifyFailed,
                    _ => StopKind::Other,
                };
                Outcome::Stopped { error: e.to_string(), resumable: e.is_resumable(), kind }
            }
        };
        if !matches!(outcome, Outcome::Stopped { .. }) {
            self.stats.phase = Phase::Done;
        }
        self.tick(obs, true);
        self.report(outcome)
    }

    fn report(&self, outcome: Outcome) -> Report {
        let mut kept = Vec::new();
        for c in &self.commits {
            if let Some(Deletion::Kept(r)) = self.deletion.get(&c.idx) {
                kept.push((c.meta.name.clone(), r.clone()));
            }
        }
        let mut failed: Vec<(PathBuf, String)> = self.failed.iter().map(|(i, r)| (self.plan[*i as usize].src.clone(), r.clone())).collect();
        failed.sort();
        let archive_size = if self.finalized {
            std::fs::metadata(&self.paths.output).map(|m| m.len()).unwrap_or(0)
        } else {
            self.archive.as_ref().map(|a| a.len).unwrap_or(0)
        };
        let finished = !matches!(outcome, Outcome::Stopped { .. });
        Report {
            outcome,
            paths: self.paths.clone(),
            archive_size,
            original_bytes: self.stats.bytes_committed,
            reclaimed: self.stats.reclaimed,
            files_archived: self.stats.files_archived,
            dirs_archived: self.stats.dirs_archived,
            originals_deleted: self.stats.originals_deleted,
            delete_requested: self.header.options.delete_originals,
            kept,
            failed,
            skipped: self.scan_skips.clone(),
            dirs_removed: self.dirs_removed,
            verification: self.verification.clone(),
            readback: self.readback,
            elapsed: self.started.elapsed(),
            warnings: self.warnings.clone(),
            recoverable_entries: if finished { 0 } else { self.commits.len() as u64 },
        }
    }

    fn run_inner(&mut self, obs: &mut dyn Observer, cancel: &AtomicBool) -> Result<()> {
        if !self.finalized {
            if self.resumed {
                self.phase(obs, Phase::Recovering);
                self.resume_pending_deletions(obs, cancel)?;
            }
            self.phase(obs, Phase::Compressing);
            self.compress_all(obs, cancel)?;
            self.phase(obs, Phase::Finalizing);
            self.finalize(obs, cancel)?;
        }
        self.phase(obs, Phase::Cleanup);
        self.cleanup(obs)?;
        Ok(())
    }

    /// After a crash, entries may be committed but their originals not yet
    /// deleted. Re-verify each one's archived data from the device before
    /// deleting anything.
    fn resume_pending_deletions(&mut self, obs: &mut dyn Observer, cancel: &AtomicBool) -> Result<()> {
        if !self.header.options.delete_originals {
            return Ok(());
        }
        let pending: Vec<usize> = (0..self.commits.len())
            .filter(|&i| !self.commits[i].meta.is_dir && !self.deletion.contains_key(&self.commits[i].idx))
            .collect();
        for i in pending {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let c = self.commits[i].clone();
            self.status(obs, c.idx, EntryStatus::Verifying);
            let archive = self.archive.as_ref().expect("partial archive open");
            if let Err(e) = verify::check_entry(&archive.file, &archive.path, &expected(&c)) {
                return Err(Error::Damaged(format!(
                    "the archived copy of \"{}\" failed verification ({e}). Its original was kept. Keep the .partial and .journal files and run `zipshrink salvage`.",
                    c.meta.name
                )));
            }
            // After an interruption always compare full contents.
            self.delete_original(obs, &c, true)?;
            self.tick(obs, false);
        }
        self.sync_journal()
    }

    fn compress_all(&mut self, obs: &mut dyn Observer, cancel: &AtomicBool) -> Result<()> {
        let todo: Vec<u64> = self.plan.iter().map(|p| p.idx).filter(|i| !self.committed.contains_key(i)).collect();
        let threads = self.header.options.threads.max(1) as usize;
        let mut pre = Prefetcher::new(if threads > 1 { threads } else { 0 }, self.header.options.level, cancel);
        let mut pending: Vec<Commit> = Vec::new();
        let mut pending_bytes = 0u64;
        let mut batch_started = Instant::now();
        let shared = Shared { cancel, dest_dir: self.paths.dest_dir(), min_free: self.header.options.min_free };
        let batch_entries = match self.header.options.batch_entries {
            0 => BATCH_ENTRIES,
            n => n as usize,
        };
        let result = (|| -> Result<()> {
            for (pos, &idx) in todo.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                pre.fill(&self.plan, &todo, pos);
                let entry = self.plan[idx as usize].clone();
                self.stats.current =
                    Some(Current { idx, name: entry.meta.name.clone(), size: entry.size, done: 0, status: Some(EntryStatus::Compressing) });
                self.status(obs, idx, EntryStatus::Compressing);
                let mut retried_after_flush = false;
                let mut attempt = 0;
                let mut prepared = pre.take(idx);
                let result = loop {
                    let r = self.ensure_space(&shared, 64 << 10).map(|_| self.write_entry(obs, &entry, prepared.take(), &shared));
                    let r = match r {
                        Ok(r) => r,
                        Err(e) => Err(EntryErr::Stop(e)),
                    };
                    match r {
                        Err(EntryErr::Changed(_)) | Err(EntryErr::TooLarge) if attempt + 1 < CHANGE_RETRIES => {
                            attempt += 1;
                            continue;
                        }
                        Err(EntryErr::Stop(Error::LowSpace(_) | Error::DiskFull(_))) if !pending.is_empty() && !retried_after_flush => {
                            // Committing (and deleting) the batch may free space.
                            self.flush_batch(obs, &mut pending)?;
                            pending_bytes = 0;
                            batch_started = Instant::now();
                            retried_after_flush = true;
                            continue;
                        }
                        other => break other,
                    }
                };
                match result {
                    Ok(c) => {
                        pending_bytes += c.usize;
                        pending.push(c);
                    }
                    Err(EntryErr::Source(r)) | Err(EntryErr::Changed(r)) => self.mark_failed(obs, idx, r),
                    Err(EntryErr::TooLarge) => self.mark_failed(obs, idx, "it kept growing while being read".into()),
                    Err(EntryErr::Stop(e)) => return Err(e),
                }
                if pending_bytes >= BATCH_BYTES || pending.len() >= batch_entries || batch_started.elapsed() >= BATCH_TIME {
                    self.flush_batch(obs, &mut pending)?;
                    pending_bytes = 0;
                    batch_started = Instant::now();
                }
                self.tick(obs, false);
            }
            self.flush_batch(obs, &mut pending)
        })();
        pre.shutdown();
        self.stats.current = None;
        if result.is_err() {
            // Uncommitted data at the end of the partial archive is left for
            // `resume`, which cuts the file back to what the journal holds.
            let _ = self.journal.as_mut().map(|j| j.sync());
        }
        result
    }

    fn mark_failed(&mut self, obs: &mut dyn Observer, idx: u64, reason: String) {
        self.status(obs, idx, EntryStatus::Failed(reason.clone()));
        self.jw().append(&Record::Failed { idx, reason: reason.clone() });
        self.failed.insert(idx, reason);
        self.stats.files_failed += 1;
        self.stats.bytes_processed += self.plan[idx as usize].size;
    }

    fn ensure_space(&self, shared: &Shared<'_>, need: u64) -> Result<()> {
        let avail = platform::available_space(&shared.dest_dir).map_err(|e| Error::io(&shared.dest_dir, e))?;
        if avail < shared.min_free.saturating_add(need) {
            return Err(Error::LowSpace(format!(
                "{} free, {} reserved. Free up space, or lower the reserve, then resume.",
                crate::util::human_bytes(avail),
                crate::util::human_bytes(shared.min_free)
            )));
        }
        Ok(())
    }

    /// Writes one entry to the end of the partial archive. On error the
    /// archive is cut back to where the entry started.
    fn write_entry(
        &mut self,
        obs: &mut dyn Observer,
        entry: &PlanEntry,
        prepared: Option<std::result::Result<Prepared, EntryErr>>,
        shared: &Shared<'_>,
    ) -> std::result::Result<Commit, EntryErr> {
        let start = self.archive.as_ref().unwrap().len;
        let r = if entry.meta.is_dir {
            self.write_dir(entry)
        } else {
            match prepared {
                Some(Ok(p)) => self.write_prepared(entry, p),
                Some(Err(EntryErr::TooLarge)) | None => self.write_streamed(obs, entry, shared),
                Some(Err(e)) => Err(e),
            }
        };
        if r.is_err() {
            let a = self.archive.as_mut().unwrap();
            if let Err(e) = a.truncate(start) {
                return Err(EntryErr::Stop(Error::io(&a.path, e)));
            }
        }
        r
    }

    fn archive_err(&self, e: io::Error) -> EntryErr {
        let path = self.paths.partial.clone();
        if platform::is_disk_full(&e) {
            EntryErr::Stop(Error::DiskFull(format!("{}. Everything committed so far is safe. Free up space, then resume.", e)))
        } else if platform::is_file_too_large(&e) {
            EntryErr::Stop(Error::Invalid(format!(
                "the destination filesystem cannot hold a file this large (FAT32 is limited to 4 GiB): {e}"
            )))
        } else {
            EntryErr::Stop(Error::io(path, e))
        }
    }

    fn write_dir(&mut self, entry: &PlanEntry) -> std::result::Result<Commit, EntryErr> {
        let offset = self.archive.as_ref().unwrap().len;
        let zip64 = self.header.options.force_zip64;
        let sizes = EntrySizes { method: METHOD_STORE, zip64_local: zip64, ..Default::default() };
        let hdr = format::local_header(&entry.meta, &sizes);
        self.archive.as_mut().unwrap().append(&hdr).map_err(|e| self.archive_err(e))?;
        Ok(commit_from(entry, offset, offset + hdr.len() as u64, &sizes, entry.meta.clone(), None, &hdr))
    }

    fn write_prepared(&mut self, entry: &PlanEntry, p: Prepared) -> std::result::Result<Commit, EntryErr> {
        let offset = self.archive.as_ref().unwrap().len;
        let meta = meta_with_stat(&entry.meta, &p.stat);
        let sizes = EntrySizes {
            method: p.method,
            crc32: p.crc32,
            compressed: p.data.len() as u64,
            uncompressed: p.usize,
            sha256: Some(p.sha256),
            zip64_local: self.header.options.force_zip64 || format::needs_zip64_sizes(p.usize),
        };
        let hdr = format::local_header(&meta, &sizes);
        let a = self.archive.as_mut().unwrap();
        let r = a.append(&hdr).and_then(|_| {
            failpoint::hit("entry_header_written")?;
            a.append(&p.data)
        });
        r.map_err(|e| self.archive_err(e))?;
        let end = self.archive.as_ref().unwrap().len;
        self.stats.bytes_processed += p.usize;
        Ok(commit_from(entry, offset, end, &sizes, meta, Some(p.stat), &hdr))
    }

    fn write_streamed(&mut self, obs: &mut dyn Observer, entry: &PlanEntry, shared: &Shared<'_>) -> std::result::Result<Commit, EntryErr> {
        let (mut src, st0) = match platform::open_source(&entry.src).and_then(|x| failpoint::hit("open_source").map(|_| x)) {
            Ok(x) => x,
            Err(e) => return Err(EntryErr::Source(describe_source_error(&e))),
        };
        let level = self.header.options.level;
        let method = if level == 0 { METHOD_STORE } else { METHOD_DEFLATE };
        let zip64 = self.header.options.force_zip64 || format::needs_zip64_sizes(st0.size);
        let meta = meta_with_stat(&entry.meta, &st0);
        let offset = self.archive.as_ref().unwrap().len;
        let placeholder = EntrySizes { method, zip64_local: zip64, ..Default::default() };
        let hdr = format::local_header(&meta, &placeholder);
        self.archive.as_mut().unwrap().append(&hdr).map_err(|e| self.archive_err(e))?;
        failpoint::hit("entry_header_written").map_err(|e| self.archive_err(e))?;
        let data_start = self.archive.as_ref().unwrap().len;

        let processed_before = self.stats.bytes_processed;
        let mut crc = crc32fast::Hasher::new();
        let mut sha = Sha256::new();
        let mut read_total = 0u64;
        let mut buf = vec![0u8; IO_BUF];
        let mut stop: Option<EntryErr> = None;
        {
            let archive = self.archive.as_mut().unwrap();
            let z = (method == METHOD_DEFLATE).then(|| flate2::Compress::new(flate2::Compression::new(level), false));
            let mut sink = Sink { archive, buf: Vec::with_capacity(IO_BUF * 2), since_check: 0, shared, err: None, z };
            let mut last_report = Instant::now();
            loop {
                if shared.cancel.load(Ordering::Relaxed) {
                    stop = Some(EntryErr::Stop(Error::Cancelled));
                    break;
                }
                let n = match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        stop = Some(EntryErr::Source(format!("could not be read: {e}")));
                        break;
                    }
                };
                crc.update(&buf[..n]);
                sha.update(&buf[..n]);
                read_total += n as u64;
                if sink.push(&buf[..n]).is_err() {
                    break; // the sink recorded why
                }
                if read_total > st0.size {
                    stop = Some(EntryErr::Changed("it grew while being read".into()));
                    break;
                }
                if last_report.elapsed() >= Duration::from_millis(100) {
                    last_report = Instant::now();
                    if let Some(c) = self.stats.current.as_mut() {
                        c.done = read_total;
                    }
                    self.stats.bytes_processed = processed_before + read_total;
                    let mut s = self.stats.clone();
                    s.archive_size = sink.archive.len + sink.buf.len() as u64;
                    s.elapsed = self.started.elapsed();
                    obs.on_event(Event::Progress(s));
                }
            }
            if stop.is_none() && sink.err.is_none() {
                let _ = sink.finish();
            }
            if let Some(e) = sink.err.take() {
                stop = Some(e);
            }
        }
        self.stats.bytes_processed = processed_before;
        if let Some(e) = stop {
            return Err(e);
        }
        let st1 = platform::stat_handle(&src).map_err(|e| EntryErr::Source(format!("could not be examined: {e}")))?;
        if read_total != st0.size || !st0.same_content_identity(&st1) {
            return Err(EntryErr::Changed(format!("{} while being read", st0.difference(&st1))));
        }
        drop(src);
        let end = self.archive.as_ref().unwrap().len;
        let sizes = EntrySizes {
            method,
            crc32: crc.finalize(),
            compressed: end - data_start,
            uncompressed: read_total,
            sha256: Some(sha.finalize().into()),
            zip64_local: zip64,
        };
        if !zip64 && (sizes.compressed >= 0xffff_ffff || sizes.uncompressed >= 0xffff_ffff) {
            return Err(EntryErr::Changed("it grew past the size planned for its header".into()));
        }
        let hdr2 = format::local_header(&meta, &sizes);
        assert_eq!(hdr.len(), hdr2.len(), "local header length must not change");
        platform::pwrite_all(&self.archive.as_ref().unwrap().file, &hdr2, offset).map_err(|e| self.archive_err(e))?;
        failpoint::hit("entry_data_written").map_err(|e| self.archive_err(e))?;
        self.stats.bytes_processed = processed_before + read_total;
        Ok(commit_from(entry, offset, end, &sizes, meta, Some(st1), &hdr2))
    }

    /// Sync, verify from the device, commit to the journal, then delete.
    fn flush_batch(&mut self, obs: &mut dyn Observer, pending: &mut Vec<Commit>) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let partial = self.paths.partial.clone();
        let first_offset = pending[0].offset;
        let fail = |job: &mut Job, e: Error| -> Error {
            if let Some(a) = job.archive.as_mut() {
                let _ = a.truncate(first_offset);
                let _ = platform::sync_file(&a.file);
            }
            e
        };
        failpoint::hit("before_archive_sync").map_err(|e| Error::io(&partial, e))?;
        if let Err(e) = platform::sync_file(&self.archive.as_ref().unwrap().file) {
            return Err(fail(self, Error::io(&partial, e)));
        }
        failpoint::hit("batch_synced").map_err(|e| Error::io(&partial, e))?;
        for c in pending.iter() {
            self.status(obs, c.idx, EntryStatus::Verifying);
            if let Some(cur) = self.stats.current.as_mut() {
                if cur.idx == c.idx {
                    cur.status = Some(EntryStatus::Verifying);
                }
            }
            self.tick(obs, false);
            let a = self.archive.as_ref().unwrap();
            match verify::check_entry(&a.file, &a.path, &expected(c)) {
                Ok(mode) => {
                    self.readback = Some(match (self.readback, mode) {
                        (Some(ReadbackMode::Cached), _) | (_, ReadbackMode::Cached) => ReadbackMode::Cached,
                        (Some(ReadbackMode::CacheEvicted), _) | (_, ReadbackMode::CacheEvicted) => ReadbackMode::CacheEvicted,
                        _ => ReadbackMode::Uncached,
                    });
                }
                Err(verify::EntryCheckError::Io(e)) => return Err(fail(self, Error::io(&partial, e))),
                Err(verify::EntryCheckError::Mismatch(m)) => {
                    return Err(fail(
                        self,
                        Error::VerifyFailed(format!(
                            "\"{}\" read back differently from what was written ({m}). The storage may be failing; its original and every later file were kept.",
                            c.meta.name
                        )),
                    ))
                }
            }
        }
        for c in pending.iter() {
            self.jw().append(&Record::Commit(c.clone()));
        }
        failpoint::hit("before_journal_commit").map_err(|e| Error::io(&partial, e))?;
        self.sync_journal()?;
        // From here on these entries are committed: never cut them off.
        let first = self.commits.len();
        for c in pending.drain(..) {
            self.failed.remove(&c.idx);
            if c.meta.is_dir {
                self.stats.dirs_archived += 1;
            } else {
                self.stats.files_archived += 1;
                self.stats.bytes_committed += c.usize;
                self.stats.compressed_committed += c.csize;
            }
            self.committed.insert(c.idx, self.commits.len());
            self.status(obs, c.idx, EntryStatus::Archived);
            self.commits.push(c);
        }
        failpoint::hit("journal_committed").map_err(|e| Error::io(&partial, e))?;
        if self.header.options.delete_originals {
            let full = !self.header.options.quick_change_check;
            for i in first..self.commits.len() {
                if !self.commits[i].meta.is_dir {
                    let c = self.commits[i].clone();
                    self.delete_original(obs, &c, full)?;
                }
            }
        }
        self.sync_journal()?;
        self.tick(obs, true);
        Ok(())
    }

    fn delete_original(&mut self, obs: &mut dyn Observer, c: &Commit, full_hash: bool) -> Result<()> {
        let pe = self.plan[c.idx as usize].clone();
        let Some(stat) = c.src_stat else { return Ok(()) };
        let root = &self.header.sources[pe.root as usize];
        let outcome = if !scan::is_within(&pe.src, root) {
            DeleteOutcome::Kept("kept because it is outside the selected folders".into())
        } else if stat.readonly && !self.header.options.delete_readonly {
            DeleteOutcome::Kept("kept because it is read-only".into())
        } else {
            failpoint::hit("before_delete").map_err(|e| Error::io(&pe.src, e))?;
            let r = failpoint::hit("delete_source")
                .and_then(|_| platform::delete_verified(&pe.src, &stat, if full_hash { c.sha256.as_ref() } else { None }));
            match r {
                Ok(o) => o,
                Err(e) => DeleteOutcome::Kept(format!("kept because it could not be deleted: {e}")),
            }
        };
        failpoint::hit("after_delete").map_err(|e| Error::io(&pe.src, e))?;
        match outcome {
            DeleteOutcome::Deleted => {
                self.jw().append(&Record::Deleted { idx: c.idx, missing: false });
                self.deletion.insert(c.idx, Deletion::Deleted);
                self.stats.originals_deleted += 1;
                self.stats.reclaimed += c.usize;
                self.status(obs, c.idx, EntryStatus::Deleted);
            }
            DeleteOutcome::Missing => {
                self.jw().append(&Record::Deleted { idx: c.idx, missing: true });
                self.deletion.insert(c.idx, Deletion::Missing);
                self.status(obs, c.idx, EntryStatus::Deleted);
            }
            DeleteOutcome::Kept(reason) => {
                self.jw().append(&Record::Kept { idx: c.idx, reason: reason.clone() });
                self.deletion.insert(c.idx, Deletion::Kept(reason.clone()));
                self.stats.originals_kept += 1;
                self.status(obs, c.idx, EntryStatus::Kept(reason));
            }
        }
        Ok(())
    }

    /// Checks the central directory we wrote against the journal.
    fn cross_check(&self, path: &Path, check: &ArchiveCheck) -> Result<()> {
        if !check.ok() {
            return Err(Error::VerifyFailed(format!("{}: {}", path.display(), check.problems.join("; "))));
        }
        let f = File::open(path).map_err(|e| Error::io(path, e))?;
        let cd = format::read_central_directory(&f).map_err(|e| Error::io(path, e))?;
        if cd.len() != self.commits.len() {
            return Err(Error::VerifyFailed(format!("the archive lists {} entries but {} were committed", cd.len(), self.commits.len())));
        }
        for (e, c) in cd.iter().zip(&self.commits) {
            if e.name != c.meta.name.as_bytes()
                || e.crc32 != c.crc32
                || e.compressed != c.csize
                || e.uncompressed != c.usize
                || e.local_offset != c.offset
                || e.sha256 != c.sha256
            {
                return Err(Error::VerifyFailed(format!("the central directory entry for \"{}\" does not match the journal", c.meta.name)));
            }
        }
        Ok(())
    }

    fn finalize(&mut self, obs: &mut dyn Observer, cancel: &AtomicBool) -> Result<()> {
        let partial = self.paths.partial.clone();
        let cd_offset = self.commits.last().map(|c| c.end).unwrap_or(0);
        {
            let a = self.archive.as_mut().unwrap();
            if a.len != cd_offset {
                a.truncate(cd_offset).map_err(|e| Error::io(&partial, e))?;
            }
        }
        let n = self.commits.len() as u64;
        self.jw().append(&Record::FinalizeStart { cd_offset, entries: n });
        self.sync_journal()?;
        let mut buf = Vec::with_capacity(IO_BUF + 1024);
        for (i, c) in self.commits.iter().enumerate() {
            let sizes = EntrySizes {
                method: c.method,
                crc32: c.crc32,
                compressed: c.csize,
                uncompressed: c.usize,
                sha256: c.sha256,
                zip64_local: c.zip64_local,
            };
            buf.extend_from_slice(&format::central_header(&c.meta, &sizes, c.offset));
            if buf.len() >= IO_BUF || i + 1 == self.commits.len() {
                let a = self.archive.as_mut().unwrap();
                a.append(&buf).map_err(|e| Error::io(&partial, e))?;
                buf.clear();
                if i + 1 < self.commits.len() {
                    failpoint::hit("finalize_cd_partial").map_err(|e| Error::io(&partial, e))?;
                }
            }
        }
        let a = self.archive.as_mut().unwrap();
        let cd_size = a.len - cd_offset;
        failpoint::hit("finalize_cd_partial").map_err(|e| Error::io(&partial, e))?;
        a.append(&format::end_records(n, cd_offset, cd_size)).map_err(|e| Error::io(&partial, e))?;
        failpoint::hit("finalize_written").map_err(|e| Error::io(&partial, e))?;
        platform::sync_file(&a.file).map_err(|e| Error::io(&partial, e))?;
        self.stats.archive_size = a.len;

        self.phase(obs, Phase::VerifyingArchive);
        let data = self.header.options.final_data_verify;
        self.stats.verify_total = if data { self.stats.bytes_committed } else { 0 };
        self.stats.verify_done = 0;
        let mut done = 0u64;
        let mut last = Instant::now();
        let stats = self.stats.clone();
        let check = verify::check_archive(&partial, data, cancel, &mut |k| {
            done += k;
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                let mut s = stats.clone();
                s.verify_done = done;
                obs.on_event(Event::Progress(s));
            }
        });
        let check = match check {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(Error::Cancelled),
            Err(e) => return Err(Error::VerifyFailed(format!("the finished archive could not be read back: {e}"))),
        };
        self.stats.verify_done = done;
        self.cross_check(&partial, &check)?;
        self.verification = Some(check);

        failpoint::hit("before_rename").map_err(|e| Error::io(&partial, e))?;
        let archive = self.archive.take().unwrap();
        drop(archive.file);
        let out = self.paths.output.clone();
        let renamed = if out.exists() {
            if !self.header.options.overwrite {
                return Err(Error::OutputExists(out));
            }
            platform::rename_replace(&partial, &out)
        } else {
            platform::rename_noreplace(&partial, &out)
        };
        renamed.map_err(|e| Error::io(&out, e))?;
        platform::sync_dir(&self.paths.dest_dir()).map_err(|e| Error::io(self.paths.dest_dir(), e))?;
        failpoint::hit("after_rename").map_err(|e| Error::io(&out, e))?;
        self.jw().append(&Record::Finalized { path: out });
        self.sync_journal()?;
        self.finalized = true;
        Ok(())
    }

    fn cleanup(&mut self, obs: &mut dyn Observer) -> Result<()> {
        failpoint::hit("before_cleanup").map_err(|e| Error::io(&self.paths.journal, e))?;
        if self.header.options.delete_originals {
            // Deepest first; a folder that is not empty is simply kept.
            let dirs: Vec<PlanEntry> =
                self.plan.iter().rev().filter(|p| p.meta.is_dir && self.committed.contains_key(&p.idx)).cloned().collect();
            for d in dirs {
                let root = &self.header.sources[d.root as usize];
                if !scan::is_within(&d.src, root) {
                    continue;
                }
                match platform::remove_dir_verified(&d.src, &d.dir_stat.unwrap_or_default()) {
                    Ok(true) => self.dirs_removed += 1,
                    Ok(false) => {}
                    Err(e) => obs.on_event(Event::Message(format!("{} was left in place: {e}", d.src.display()))),
                }
            }
        }
        if let Some(j) = self.journal.take() {
            j.close();
        }
        std::fs::remove_file(&self.paths.journal).map_err(|e| Error::io(&self.paths.journal, e))?;
        let _ = platform::sync_dir(&self.paths.dest_dir());
        Ok(())
    }
}

fn expected(c: &Commit) -> Expected<'_> {
    Expected {
        name: &c.meta.name,
        offset: c.offset,
        end: c.end,
        method: c.method,
        crc32: c.crc32,
        csize: c.csize,
        usize: c.usize,
        sha256: c.sha256,
        zip64_local: c.zip64_local,
        header_crc: c.header_crc,
    }
}

fn commit_from(
    entry: &PlanEntry,
    offset: u64,
    end: u64,
    s: &EntrySizes,
    meta: EntryMeta,
    src_stat: Option<FileStat>,
    header: &[u8],
) -> Commit {
    Commit {
        header_crc: Some(crc32fast::hash(header)),
        idx: entry.idx,
        offset,
        end,
        method: s.method,
        crc32: s.crc32,
        csize: s.compressed,
        usize: s.uncompressed,
        sha256: s.sha256,
        zip64_local: s.zip64_local,
        src_stat,
        meta,
    }
}

fn meta_with_stat(m: &EntryMeta, st: &FileStat) -> EntryMeta {
    EntryMeta {
        name: m.name.clone(),
        is_dir: m.is_dir,
        mtime: st.mtime,
        atime: st.atime,
        btime: st.btime,
        unix_mode: st.unix_mode,
        dos_attrs: st.dos_attrs,
    }
}

fn describe_source_error(e: &io::Error) -> String {
    if platform::is_sharing_violation(e) {
        return "could not be opened because another program is using it".into();
    }
    match e.kind() {
        io::ErrorKind::PermissionDenied => format!("could not be opened (permission denied): {e}"),
        io::ErrorKind::NotFound => "no longer exists".into(),
        _ if e.raw_os_error() == Some(eloop()) => "was replaced by a symbolic link".into(),
        _ => format!("could not be opened: {e}"),
    }
}

fn eloop() -> i32 {
    #[cfg(unix)]
    {
        libc::ELOOP
    }
    #[cfg(not(unix))]
    {
        -1
    }
}

/// Compresses into a buffer, appends it to the archive, watches free space.
struct Sink<'a, 'b> {
    archive: &'a mut Archive,
    buf: Vec<u8>,
    since_check: u64,
    shared: &'a Shared<'b>,
    err: Option<EntryErr>,
    /// Raw deflate state; `None` stores the data as is.
    z: Option<flate2::Compress>,
}

impl Sink<'_, '_> {
    fn flush_buf(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        if self.since_check >= SPACE_CHECK_EVERY {
            self.since_check = 0;
            if let Ok(avail) = platform::available_space(&self.shared.dest_dir) {
                if avail < self.shared.min_free.saturating_add(SPACE_CHECK_EVERY) {
                    self.err = Some(EntryErr::Stop(Error::LowSpace(format!(
                        "{} free, {} reserved. Free up space, or lower the reserve, then resume.",
                        crate::util::human_bytes(avail),
                        crate::util::human_bytes(self.shared.min_free)
                    ))));
                    return Err(io::Error::other("low space"));
                }
            }
        }
        let n = self.buf.len() as u64;
        if let Err(e) = self.archive.append(&self.buf) {
            self.err = Some(if platform::is_disk_full(&e) {
                EntryErr::Stop(Error::DiskFull(format!("{e}. Everything committed so far is safe. Free up space, then resume.")))
            } else if platform::is_file_too_large(&e) {
                EntryErr::Stop(Error::Invalid(format!(
                    "the destination filesystem cannot hold a file this large (FAT32 is limited to 4 GiB): {e}"
                )))
            } else {
                EntryErr::Stop(Error::io(&self.archive.path, e))
            });
            return Err(io::Error::other("archive write failed"));
        }
        self.since_check += n;
        self.buf.clear();
        Ok(())
    }

    fn make_room(&mut self) -> io::Result<()> {
        if self.buf.len() >= IO_BUF {
            self.flush_buf()?;
        }
        if self.buf.capacity() - self.buf.len() < 64 << 10 {
            self.buf.reserve(IO_BUF);
        }
        Ok(())
    }

    fn push(&mut self, mut data: &[u8]) -> io::Result<()> {
        if self.err.is_some() {
            return Err(io::Error::other("stopped"));
        }
        if self.z.is_none() {
            self.buf.extend_from_slice(data);
            return self.make_room();
        }
        while !data.is_empty() {
            self.make_room()?;
            let z = self.z.as_mut().unwrap();
            let before = z.total_in();
            z.compress_vec(data, &mut self.buf, flate2::FlushCompress::None)
                .map_err(|e| io::Error::other(format!("compression failed: {e}")))?;
            let used = (z.total_in() - before) as usize;
            data = &data[used..];
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.z.is_some() {
            loop {
                self.make_room()?;
                let z = self.z.as_mut().unwrap();
                let st = z
                    .compress_vec(&[], &mut self.buf, flate2::FlushCompress::Finish)
                    .map_err(|e| io::Error::other(format!("compression failed: {e}")))?;
                if st == flate2::Status::StreamEnd {
                    break;
                }
            }
        }
        self.flush_buf()
    }
}

/// Pre-compresses small files on worker threads, in plan order windows.
struct Prefetcher {
    tx: Option<Sender<(u64, PathBuf)>>,
    rx: Option<Receiver<(u64, std::result::Result<Prepared, EntryErr>)>>,
    handles: Vec<std::thread::JoinHandle<()>>,
    ready: HashMap<u64, std::result::Result<Prepared, EntryErr>>,
    dispatched: HashSet<u64>,
    next: usize,
    inflight_bytes: u64,
    sizes: HashMap<u64, u64>,
    cancel: Arc<AtomicBool>,
    threads: usize,
}

impl Prefetcher {
    fn new(threads: usize, level: u32, outer_cancel: &AtomicBool) -> Self {
        let _ = outer_cancel;
        let cancel = Arc::new(AtomicBool::new(false));
        if threads == 0 {
            return Prefetcher {
                tx: None,
                rx: None,
                handles: vec![],
                ready: HashMap::new(),
                dispatched: HashSet::new(),
                next: 0,
                inflight_bytes: 0,
                sizes: HashMap::new(),
                cancel,
                threads,
            };
        }
        let (tx, jobs) = crossbeam_channel::unbounded::<(u64, PathBuf)>();
        let (done_tx, rx) = crossbeam_channel::unbounded();
        let mut handles = Vec::new();
        for _ in 0..threads {
            let jobs = jobs.clone();
            let done_tx = done_tx.clone();
            let cancel = cancel.clone();
            handles.push(std::thread::spawn(move || {
                for (idx, path) in jobs.iter() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let r = compress_small(&path, level);
                    if done_tx.send((idx, r)).is_err() {
                        break;
                    }
                }
            }));
        }
        Prefetcher {
            tx: Some(tx),
            rx: Some(rx),
            handles,
            ready: HashMap::new(),
            dispatched: HashSet::new(),
            next: 0,
            inflight_bytes: 0,
            sizes: HashMap::new(),
            cancel,
            threads,
        }
    }

    /// Dispatches small files ahead of `pos`, within a bounded window.
    fn fill(&mut self, plan: &[PlanEntry], todo: &[u64], pos: usize) {
        let Some(tx) = &self.tx else { return };
        self.next = self.next.max(pos);
        let max_ahead = self.threads * 16;
        while self.next < todo.len() && self.next < pos + max_ahead && self.inflight_bytes < 64 << 20 {
            let idx = todo[self.next];
            let e = &plan[idx as usize];
            if !e.meta.is_dir && e.size <= SMALL_FILE {
                self.dispatched.insert(idx);
                self.inflight_bytes += e.size;
                self.sizes.insert(idx, e.size);
                let _ = tx.send((idx, e.src.clone()));
            }
            self.next += 1;
        }
    }

    fn take(&mut self, idx: u64) -> Option<std::result::Result<Prepared, EntryErr>> {
        if !self.dispatched.remove(&idx) {
            return None;
        }
        self.inflight_bytes -= self.sizes.remove(&idx).unwrap_or(0);
        loop {
            if let Some(r) = self.ready.remove(&idx) {
                return Some(r);
            }
            match self.rx.as_ref()?.recv() {
                Ok((i, r)) => {
                    self.ready.insert(i, r);
                }
                Err(_) => return None,
            }
        }
    }

    fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx = None;
        self.rx = None;
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn compress_small(path: &Path, level: u32) -> std::result::Result<Prepared, EntryErr> {
    let (mut f, st0) = platform::open_source(path)
        .and_then(|x| failpoint::hit("open_source").map(|_| x))
        .map_err(|e| EntryErr::Source(describe_source_error(&e)))?;
    if st0.size > SMALL_FILE {
        return Err(EntryErr::TooLarge);
    }
    let mut raw = Vec::with_capacity(st0.size as usize);
    (&mut f).take(SMALL_FILE + 1).read_to_end(&mut raw).map_err(|e| EntryErr::Source(format!("could not be read: {e}")))?;
    if raw.len() as u64 > SMALL_FILE {
        return Err(EntryErr::TooLarge);
    }
    let st1 = platform::stat_handle(&f).map_err(|e| EntryErr::Source(format!("could not be examined: {e}")))?;
    if raw.len() as u64 != st0.size || !st0.same_content_identity(&st1) {
        return Err(EntryErr::Changed(format!("{} while being read", st0.difference(&st1))));
    }
    let crc32 = crc32fast::hash(&raw);
    let sha256: [u8; 32] = Sha256::digest(&raw).into();
    let (method, data) = if level == 0 {
        (METHOD_STORE, raw)
    } else {
        let mut z = flate2::write::DeflateEncoder::new(Vec::with_capacity(raw.len() / 2 + 64), flate2::Compression::new(level));
        z.write_all(&raw).map_err(|e| EntryErr::Source(e.to_string()))?;
        (METHOD_DEFLATE, z.finish().map_err(|e| EntryErr::Source(e.to_string()))?)
    };
    let usize = st0.size;
    Ok(Prepared { data, method, crc32, usize, sha256, stat: st1 })
}
