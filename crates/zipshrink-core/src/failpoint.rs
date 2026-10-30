//! Fault injection for the crash and error tests.
//!
//! With the `failpoints` feature, named points in the engine can be armed
//! through the `ZIPSHRINK_FAILPOINT` environment variable, as
//! `name[:N[+]][=action]` (comma separated), where `N` is the hit that
//! triggers (default 1; with `+`, that hit and every later one) and `action` is `abort` (default; ends the process
//! immediately, like a crash or power cut as far as user space is
//! concerned), `enospc` or `eio` (the operation fails with that error).
//! Tests running in-process arm points per thread with [`arm`].
//!
//! Without the feature every function here compiles to nothing.

use std::fs::File;
use std::io;

#[cfg(feature = "failpoints")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::OnceLock;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Action {
        Abort,
        Enospc,
        Eio,
    }

    #[derive(Default)]
    struct Points {
        armed: HashMap<String, (u64, Action, bool)>,
        hits: HashMap<String, u64>,
    }

    thread_local! {
        static LOCAL: RefCell<Option<Points>> = const { RefCell::new(None) };
        static HOOKS: RefCell<HashMap<String, Box<dyn FnMut()>>> = RefCell::new(HashMap::new());
    }

    /// Runs `f` every time the named point is reached on this thread.
    pub fn set_hook(name: &str, f: impl FnMut() + 'static) {
        HOOKS.with(|h| h.borrow_mut().insert(name.to_string(), Box::new(f)));
    }

    pub fn run_hook(name: &str) {
        let hook = HOOKS.with(|h| h.borrow_mut().remove(name));
        if let Some(mut f) = hook {
            f();
            HOOKS.with(|h| {
                h.borrow_mut().entry(name.to_string()).or_insert(f);
            });
        }
    }

    fn from_env() -> Points {
        let mut p = Points::default();
        if let Ok(spec) = std::env::var("ZIPSHRINK_FAILPOINT") {
            for item in spec.split(',').filter(|s| !s.is_empty()) {
                let (head, action) = match item.split_once('=') {
                    Some((h, "enospc")) => (h, Action::Enospc),
                    Some((h, "eio")) => (h, Action::Eio),
                    Some((h, _)) => (h, Action::Abort),
                    None => (item, Action::Abort),
                };
                let (name, n, repeat) = match head.split_once(':') {
                    Some((n, c)) => match c.strip_suffix('+') {
                        Some(c) => (n, c.parse().unwrap_or(1), true),
                        None => (n, c.parse().unwrap_or(1), false),
                    },
                    None => (head, 1, false),
                };
                p.armed.insert(name.to_string(), (n, action, repeat));
            }
        }
        p
    }

    static ENV_SET: OnceLock<bool> = OnceLock::new();

    /// Triggers `action` on the `nth` hit of `name`.
    pub fn arm(name: &str, nth: u64, action: Action) {
        LOCAL.with(|l| {
            l.borrow_mut().get_or_insert_with(Points::default).armed.insert(name.to_string(), (nth, action, false));
        });
    }

    /// Triggers `action` on the `nth` hit of `name` and every hit after it.
    pub fn arm_from(name: &str, nth: u64, action: Action) {
        LOCAL.with(|l| {
            l.borrow_mut().get_or_insert_with(Points::default).armed.insert(name.to_string(), (nth, action, true));
        });
    }

    pub fn disarm_all() {
        LOCAL.with(|l| *l.borrow_mut() = None);
        HOOKS.with(|h| h.borrow_mut().clear());
    }

    pub fn check(name: &str) -> Option<Action> {
        LOCAL.with(|l| {
            let mut l = l.borrow_mut();
            if l.is_none() && *ENV_SET.get_or_init(|| std::env::var_os("ZIPSHRINK_FAILPOINT").is_some()) {
                *l = Some(from_env());
            }
            let p = l.as_mut()?;
            let (nth, action, repeat) = *p.armed.get(name)?;
            let h = p.hits.entry(name.to_string()).or_default();
            *h += 1;
            (*h == nth || (repeat && *h > nth)).then_some(action)
        })
    }
}

#[cfg(feature = "failpoints")]
pub use imp::{arm, arm_from, disarm_all, set_hook, Action};

/// Triggers the named point if armed: aborts the process, or returns the
/// configured error.
#[inline]
pub fn hit(name: &str) -> io::Result<()> {
    #[cfg(feature = "failpoints")]
    {
        imp::run_hook(name);
        match imp::check(name) {
            None => {}
            Some(Action::Abort) => {
                eprintln!("failpoint {name}: aborting");
                std::process::abort();
            }
            Some(Action::Enospc) => return Err(io::Error::from(io::ErrorKind::StorageFull)),
            Some(Action::Eio) => return Err(io::Error::other(format!("injected I/O error at {name}"))),
        }
    }
    let _ = name;
    Ok(())
}

/// The `journal_torn` point writes half of the pending journal records and
/// then aborts, leaving a torn final line as a power cut can.
#[inline]
pub fn hit_torn_journal(f: &File, at: u64, buf: &[u8]) -> io::Result<()> {
    #[cfg(feature = "failpoints")]
    {
        if imp::check("journal_torn") == Some(Action::Abort) {
            let half = &buf[..buf.len() / 2 + 1];
            crate::platform::pwrite_all(f, half, at)?;
            let _ = f.sync_all();
            eprintln!("failpoint journal_torn: aborting");
            std::process::abort();
        }
    }
    let _ = (f, at, buf);
    Ok(())
}
