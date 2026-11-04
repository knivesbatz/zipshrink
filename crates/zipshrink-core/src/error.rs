use std::path::PathBuf;

/// Errors that stop a job. Everything recoverable leaves the partial
/// archive and journal in place; the message says what to do next.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{} already exists. Choose another name, or allow overwriting it.", .0.display())]
    OutputExists(PathBuf),
    #[error("an unfinished job for this archive already exists ({}). Resume it, or move it away first.", partial.display())]
    JobExists { partial: PathBuf },
    #[error("not enough disk space: {0}")]
    InsufficientSpace(String),
    #[error("paused because free space on the destination fell to the reserve: {0}")]
    LowSpace(String),
    #[error("the disk is full: {0}")]
    DiskFull(String),
    #[error("{0}")]
    Journal(#[from] crate::journal::LoadError),
    #[error("the partial archive is damaged, so no further originals will be deleted: {0}")]
    Damaged(String),
    #[error("verification failed: {0}")]
    VerifyFailed(String),
    #[error("stopped at your request")]
    Cancelled,
    #[error("{0}")]
    Busy(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }

    /// Whether `resume` can pick the job up again.
    pub fn is_resumable(&self) -> bool {
        matches!(self, Error::LowSpace(_) | Error::DiskFull(_) | Error::Cancelled | Error::Io { .. } | Error::Busy(_))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
