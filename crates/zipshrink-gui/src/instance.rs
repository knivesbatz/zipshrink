//! One window at a time. Explorer's "Compress with ZipShrink" starts one
//! process per selected item; the first one to start keeps a lock and its
//! window, and later ones hand their paths over through a small inbox
//! folder and exit, so a multi-item selection lands in a single archive.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const RESUME: &str = "resume\t";

pub struct Primary {
    _lock: File,
    inbox: PathBuf,
}

pub enum Instance {
    /// This process owns the window.
    Primary(Option<Primary>),
    /// Another window took the paths; exit.
    Forwarded,
}

/// A request from another launch.
pub enum Message {
    Open(Vec<PathBuf>),
    Resume(PathBuf),
}

fn dir() -> Option<PathBuf> {
    zipshrink_update::settings_path().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Takes the single-instance lock, or forwards `paths` to the window that
/// holds it.
pub fn claim(paths: &[PathBuf], resume: Option<&Path>) -> Instance {
    let Some(dir) = dir() else { return Instance::Primary(None) };
    let inbox = dir.join("inbox");
    if std::fs::create_dir_all(&inbox).is_err() {
        return Instance::Primary(None);
    }
    let Ok(lock) = File::options().create(true).truncate(false).write(true).open(dir.join("instance.lock")) else {
        return Instance::Primary(None);
    };
    if lock.try_lock().is_ok() {
        // Requests left by launches that raced a window that was closing.
        if let Ok(rd) = std::fs::read_dir(&inbox) {
            for e in rd.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
                if old.is_none_or(|age| age > Duration::from_secs(30)) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        return Instance::Primary(Some(Primary { _lock: lock, inbox }));
    }
    let mut text = String::new();
    for p in paths {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.clone());
        text.push_str(&abs.to_string_lossy());
        text.push('\n');
    }
    if let Some(r) = resume {
        let abs = std::path::absolute(r).unwrap_or_else(|_| r.to_path_buf());
        text.push_str(RESUME);
        text.push_str(&abs.to_string_lossy());
        text.push('\n');
    }
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let name = format!("{}-{nanos}", std::process::id());
    let tmp = inbox.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, inbox.join(format!("{name}.req"))).is_ok() {
        Instance::Forwarded
    } else {
        let _ = std::fs::remove_file(&tmp);
        // Could not hand over; open a window of our own instead.
        Instance::Primary(None)
    }
}

impl Primary {
    /// Requests waiting in the inbox (each also asks for the window to come
    /// to the front, even when it carries no paths).
    pub fn take(&self) -> Option<Vec<Message>> {
        let rd = std::fs::read_dir(&self.inbox).ok()?;
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "req")).collect();
        if files.is_empty() {
            return None;
        }
        files.sort();
        let mut out = Vec::new();
        let mut open = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            let _ = std::fs::remove_file(&f);
            for line in text.lines().filter(|l| !l.is_empty()) {
                match line.strip_prefix(RESUME) {
                    Some(r) => out.push(Message::Resume(PathBuf::from(r))),
                    None => open.push(PathBuf::from(line)),
                }
            }
        }
        if !open.is_empty() {
            out.insert(0, Message::Open(open));
        }
        Some(out)
    }
}
