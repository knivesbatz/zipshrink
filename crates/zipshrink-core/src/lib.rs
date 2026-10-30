//! ZipShrink core engine.
//!
//! Creates ZIP archives one verified entry at a time and, when the caller
//! explicitly asks for it, deletes each original only after its compressed
//! copy has been flushed to storage, read back from the device, decompressed
//! and matched against the CRC-32 and SHA-256 taken from the source, and
//! recorded in a durable journal. Any interruption leaves a partial archive
//! and journal from which [`Job::open`] resumes.
//!
//! See `docs/ARCHITECTURE.md` and `docs/DATA_SAFETY.md` in the repository.

pub mod engine;
pub mod error;
pub mod failpoint;
pub mod format;
pub mod journal;
pub mod platform;
pub mod progress;
pub mod salvage;
pub mod scan;
pub mod space;
pub mod split;
pub mod util;
pub mod verify;

pub use engine::{CreateRequest, Job, JobPaths, Outcome, Report};
pub use error::{Error, Result};
pub use journal::JobOptions;
pub use progress::{EntryStatus, Event, Observer, Phase, Stats};
pub use space::{Preflight, Verdict};

/// Default free space kept on the destination volume: 256 MiB.
pub const DEFAULT_MIN_FREE: u64 = 256 << 20;
/// Hard floor for the free-space reserve, whatever the user asks for.
pub const MIN_FREE_FLOOR: u64 = 16 << 20;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
