//! Jobs this app started, remembered so interrupted ones can be offered
//! for resuming on the Resume page (jobs started elsewhere can be opened
//! from their .partial or .journal file).

use std::path::{Path, PathBuf};
use zipshrink_core::journal::{self, Record};
use zipshrink_core::JobPaths;

#[derive(Clone, Debug)]
pub struct Unfinished {
    pub output: PathBuf,
    pub deletes_originals: bool,
    pub modified: Option<std::time::SystemTime>,
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
    // Keep the list short; finished jobs are dropped by `unfinished`.
    let excess = list.len().saturating_sub(200);
    list.drain(..excess);
    save(&list);
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

/// Remembered jobs that still have a .partial or journal, newest first.
/// Finished ones are forgotten.
pub fn unfinished() -> Vec<Unfinished> {
    let list = load();
    let mut keep = Vec::new();
    let mut out = Vec::new();
    for o in list {
        let paths = JobPaths::for_output(&o);
        if paths.has_unfinished_job() {
            let modified = std::fs::metadata(&paths.journal).and_then(|m| m.modified()).ok();
            out.push(Unfinished { deletes_originals: deletes_originals(&o), output: o.clone(), modified });
            keep.push(o);
        }
    }
    save(&keep);
    out.sort_by_key(|u| std::cmp::Reverse(u.modified));
    out
}
