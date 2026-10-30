//! Progress reporting. Every value corresponds to a real operation.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Scanning,
    Recovering,
    Compressing,
    Finalizing,
    VerifyingArchive,
    Cleanup,
    Done,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Scanning => "Scanning",
            Phase::Recovering => "Checking the interrupted job",
            Phase::Compressing => "Compressing",
            Phase::Finalizing => "Writing the central directory",
            Phase::VerifyingArchive => "Verifying the finished archive",
            Phase::Cleanup => "Removing emptied folders",
            Phase::Done => "Done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryStatus {
    Waiting,
    Compressing,
    Verifying,
    /// Committed to the journal; the original (if any) is untouched.
    Archived,
    /// Archived and the original was deleted.
    Deleted,
    /// Archived; the original was deliberately kept, with the reason.
    Kept(String),
    /// Not archived (link, special file, unreadable name...).
    Skipped(String),
    /// Could not be archived in this run; the original is untouched.
    Failed(String),
    /// Committed in an interrupted job: recoverable with `resume`.
    Recoverable,
}

impl EntryStatus {
    pub fn label(&self) -> &'static str {
        match self {
            EntryStatus::Waiting => "Waiting",
            EntryStatus::Compressing => "Compressing",
            EntryStatus::Verifying => "Verifying",
            EntryStatus::Archived => "Safely archived",
            EntryStatus::Deleted => "Original deleted",
            EntryStatus::Kept(_) => "Safely archived, original kept",
            EntryStatus::Skipped(_) => "Skipped",
            EntryStatus::Failed(_) => "Failed",
            EntryStatus::Recoverable => "Recoverable",
        }
    }
    pub fn detail(&self) -> Option<&str> {
        match self {
            EntryStatus::Kept(s) | EntryStatus::Skipped(s) | EntryStatus::Failed(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Current {
    pub idx: u64,
    pub name: String,
    pub size: u64,
    pub done: u64,
    pub status: Option<EntryStatus>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub phase: Phase,
    pub total_files: u64,
    pub total_dirs: u64,
    /// Bytes of all planned files.
    pub total_bytes: u64,
    pub files_archived: u64,
    pub dirs_archived: u64,
    pub originals_deleted: u64,
    pub originals_kept: u64,
    pub files_failed: u64,
    pub files_skipped: u64,
    /// Uncompressed bytes read from sources so far (including the file in progress).
    pub bytes_processed: u64,
    /// Uncompressed bytes of committed entries.
    pub bytes_committed: u64,
    /// Compressed bytes of committed entries.
    pub compressed_committed: u64,
    /// Current size of the partial archive on disk.
    pub archive_size: u64,
    /// Bytes of originals deleted.
    pub reclaimed: u64,
    pub free_space: Option<u64>,
    /// Bytes per second over the recent past.
    pub speed: f64,
    pub elapsed: Duration,
    pub current: Option<Current>,
    pub delete_originals: bool,
    /// Bytes verified in the final archive check.
    pub verify_done: u64,
    pub verify_total: u64,
}

impl Stats {
    pub fn percent(&self) -> f64 {
        match self.phase {
            Phase::Done => 100.0,
            Phase::VerifyingArchive if self.verify_total > 0 => 100.0 * self.verify_done as f64 / self.verify_total as f64,
            _ if self.total_bytes == 0 => {
                let t = self.total_files + self.total_dirs;
                if t == 0 {
                    0.0
                } else {
                    100.0 * (self.files_archived + self.dirs_archived) as f64 / t as f64
                }
            }
            _ => (100.0 * self.bytes_processed as f64 / self.total_bytes as f64).min(100.0),
        }
    }
    /// Compressed / uncompressed for committed data.
    pub fn ratio(&self) -> Option<f64> {
        (self.bytes_committed > 0).then(|| self.compressed_committed as f64 / self.bytes_committed as f64)
    }
    pub fn remaining_files(&self) -> u64 {
        self.total_files.saturating_sub(self.files_archived + self.files_failed)
    }
    /// Estimated growth of the archive from the remaining data.
    pub fn estimated_remaining_archive(&self) -> u64 {
        let rest = self.total_bytes.saturating_sub(self.bytes_processed);
        (rest as f64 * self.ratio().unwrap_or(1.0)) as u64
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Phase(Phase),
    /// An entry's status changed.
    Entry {
        idx: u64,
        name: String,
        size: u64,
        status: EntryStatus,
    },
    /// Periodic snapshot (about ten per second at most).
    Progress(Stats),
    Message(String),
}

pub trait Observer: Send {
    fn on_event(&mut self, ev: Event);
}

impl<F: FnMut(Event) + Send> Observer for F {
    fn on_event(&mut self, ev: Event) {
        self(ev)
    }
}

/// An observer that ignores everything.
pub struct Silent;
impl Observer for Silent {
    fn on_event(&mut self, _ev: Event) {}
}
