//! Jobs this app started, remembered so interrupted ones can be offered
//! for resuming on the Resume page (jobs started elsewhere can be opened
//! from their .partial or .journal file).
//!
//! A job is forgotten only once its folder can be read and holds neither
//! its .partial nor its journal. A folder that cannot be looked at now (a
//! drive that is not connected, or one macOS has not allowed ZipShrink to
//! open again since an update) says nothing about the job, and for one
//! that deletes originals the archive may hold the only copy of the files.

use std::io;
use std::path::{Path, PathBuf};
use zipshrink_core::journal::{self, Record};
use zipshrink_core::JobPaths;

#[derive(Clone, Debug)]
pub struct Unfinished {
    pub output: PathBuf,
    pub deletes_originals: bool,
    pub modified: Option<std::time::SystemTime>,
    /// False when its folder could not be looked at: the job may or may
    /// not be finished.
    pub reachable: bool,
}

fn store() -> Option<PathBuf> {
    zipshrink_update::settings_path().and_then(|p| p.parent().map(|d| d.join("jobs.json")))
}

fn load() -> Vec<PathBuf> {
    store().and_then(|p| std::fs::read(p).ok()).and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn save(list: &[PathBuf]) {
    if let Some(p) = store() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(list).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Records the outputs of a job that is about to start.
pub fn remember(outputs: impl IntoIterator<Item = PathBuf>) {
    let mut list = load();
    for o in outputs {
        if !list.contains(&o) {
            list.push(o);
        }
    }
    // Keep the list short; finished jobs are dropped by `unfinished` and
    // `prune`.
    let excess = list.len().saturating_sub(200);
    list.drain(..excess);
    save(&list);
}

/// Removes a job from the list (the user's choice; no files are touched).
pub fn forget(output: &Path) {
    let mut list = load();
    list.retain(|o| o != output);
    save(&list);
}

/// How many jobs are remembered, without looking at their folders.
pub fn remembered() -> usize {
    load().len()
}

pub fn deletes_originals(path: &Path) -> bool {
    journal::load(&JobPaths::from_any(path).journal)
        .ok()
        .and_then(|l| match l.records.first() {
            Some(Record::Header(h)) => Some(h.options.delete_originals),
            _ => None,
        })
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Unfinished,
    Finished,
    /// The folder cannot be looked at now.
    Unknown,
}

fn state_of(journal: io::Result<bool>, partial: io::Result<bool>, folder: impl FnOnce() -> io::Result<()>) -> State {
    match (journal, partial) {
        (Ok(true), _) | (_, Ok(true)) => State::Unfinished,
        // Both missing: that means finished only if the folder is there
        // and can be read (a missing drive also reports "not found").
        (Ok(false), Ok(false)) => match folder() {
            Ok(()) => State::Finished,
            Err(_) => State::Unknown,
        },
        _ => State::Unknown,
    }
}

fn state(paths: &JobPaths) -> State {
    state_of(paths.journal.try_exists(), paths.partial.try_exists(), || std::fs::read_dir(paths.dest_dir()).map(drop))
}

/// Forgets those of `outputs` (a job that just ended, in a folder the app
/// has just used) that are finished, without looking at other folders.
pub fn prune(outputs: &[PathBuf]) {
    let mut list = load();
    let before = list.len();
    list.retain(|o| !outputs.contains(o) || state(&JobPaths::for_output(o)) != State::Finished);
    if list.len() != before {
        save(&list);
    }
}

/// Remembered jobs that still have a .partial or journal, or whose folder
/// cannot be looked at now, newest first. Finished ones are forgotten.
pub fn unfinished() -> Vec<Unfinished> {
    let list = load();
    let mut keep = Vec::new();
    let mut out = Vec::new();
    for o in list {
        let paths = JobPaths::for_output(&o);
        match state(&paths) {
            State::Finished => continue,
            State::Unfinished => {
                let modified = std::fs::metadata(&paths.journal).and_then(|m| m.modified()).ok();
                out.push(Unfinished { deletes_originals: deletes_originals(&o), output: o.clone(), modified, reachable: true });
            }
            State::Unknown => out.push(Unfinished { deletes_originals: false, output: o.clone(), modified: None, reachable: false }),
        }
        keep.push(o);
    }
    save(&keep);
    // Unreachable ones (no date) last.
    out.sort_by_key(|u| std::cmp::Reverse(u.modified));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(kind: io::ErrorKind) -> io::Result<bool> {
        Err(io::Error::from(kind))
    }

    #[test]
    fn only_a_readable_folder_without_the_files_means_finished() {
        let ok = || Ok(());
        let gone = || Err(io::Error::from(io::ErrorKind::NotFound));
        let denied = || Err(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(state_of(Ok(true), Ok(false), ok), State::Unfinished);
        assert_eq!(state_of(Ok(false), Ok(true), gone), State::Unfinished);
        assert_eq!(state_of(err(io::ErrorKind::PermissionDenied), Ok(true), ok), State::Unfinished);
        assert_eq!(state_of(Ok(false), Ok(false), ok), State::Finished);
        // A drive that is not connected: its folder is not found either.
        assert_eq!(state_of(Ok(false), Ok(false), gone), State::Unknown);
        // macOS privacy protection said no, or any other error.
        assert_eq!(state_of(Ok(false), Ok(false), denied), State::Unknown);
        assert_eq!(state_of(err(io::ErrorKind::PermissionDenied), err(io::ErrorKind::PermissionDenied), ok), State::Unknown);
        assert_eq!(state_of(Ok(false), err(io::ErrorKind::Other), ok), State::Unknown);
    }

    #[test]
    fn states_of_real_folders() {
        let dir = std::env::temp_dir().join(format!("zipshrink-jobs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("a.zip");
        let paths = JobPaths::for_output(&out);
        assert_eq!(state(&paths), State::Finished);
        std::fs::write(&paths.partial, b"x").unwrap();
        assert_eq!(state(&paths), State::Unfinished);
        let elsewhere = JobPaths::for_output(&dir.join("not connected").join("a.zip"));
        assert_eq!(state(&elsewhere), State::Unknown);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
