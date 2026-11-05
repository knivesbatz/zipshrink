//! The install or uninstall running on a background thread, so the window
//! keeps drawing. It first waits for a running ZipShrink to close.
//!
//! Quitting the installer and the work starting are decided under one lock:
//! either `cancel` comes first and nothing is changed, or the work has
//! started and `cancel` refuses (the window must then stay until it ends).

use crate::ops::{Backend, Plan};
use eframe::egui::Context;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub enum Job {
    Install(Plan),
    Uninstall(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// ZipShrink is open; the files cannot be replaced until it quits.
    Waiting,
    /// Files are being changed (or are about to be): the window cannot be
    /// closed now.
    Working,
    Done(Result<(), String>),
    /// Quit before anything was changed.
    Cancelled,
}

#[derive(Clone)]
pub struct Snapshot {
    pub phase: Phase,
    pub frac: f32,
    pub status: String,
    /// When the work (not the waiting) started.
    pub started: Option<Instant>,
}

struct State {
    snap: Snapshot,
    /// The backend has been (or is about to be) asked to change files.
    begun: bool,
    cancelled: bool,
}

pub struct Task {
    state: Mutex<State>,
}

impl Task {
    pub fn start(ctx: &Context, backend: &'static dyn Backend, job: Job) -> Arc<Task> {
        let ctx = ctx.clone();
        Self::spawn(backend, job, move || ctx.request_repaint())
    }

    /// `start` without a window: `repaint` is called after every change.
    fn spawn(backend: &'static dyn Backend, job: Job, repaint: impl Fn() + Send + Sync + 'static) -> Arc<Task> {
        let task = Arc::new(Task {
            state: Mutex::new(State {
                snap: Snapshot { phase: Phase::Working, frac: 0.0, status: "Preparing…".into(), started: None },
                begun: false,
                cancelled: false,
            }),
        });
        let t = task.clone();
        std::thread::spawn(move || t.run(backend, job, &repaint));
        task
    }

    fn run(&self, backend: &'static dyn Backend, job: Job, repaint: &(dyn Fn() + Sync)) {
        let dir = match &job {
            Job::Install(plan) => plan.dir.clone(),
            Job::Uninstall(dir) => dir.clone(),
        };
        let pretend_open = super::demo_switch(backend, "ZIPSHRINK_SETUP_DEMO_RUNNING").and_then(|s| s.parse::<u64>().ok());
        if let Some(secs) = pretend_open {
            self.set(repaint, |s| s.phase = Phase::Waiting);
            let until = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < until && !self.cancelled() {
                std::thread::sleep(Duration::from_millis(100));
            }
        } else if backend.app_running(&dir) {
            self.set(repaint, |s| s.phase = Phase::Waiting);
            // Short waits, so quitting the installer is noticed quickly.
            while !self.cancelled() && !backend.wait_closed(&dir, 1) {}
        }
        // The cancel may have come while ZipShrink was closing: check again,
        // under the same lock `cancel` takes, before changing anything.
        {
            let mut st = self.lock();
            if st.cancelled {
                st.snap.phase = Phase::Cancelled;
                drop(st);
                repaint();
                return;
            }
            st.begun = true;
            st.snap.phase = Phase::Working;
            st.snap.started = Some(Instant::now());
        }
        repaint();
        let progress = |f: f32, status: &str| {
            self.set(repaint, |s| {
                s.frac = f.clamp(0.0, 1.0);
                if !status.is_empty() {
                    status.clone_into(&mut s.status);
                }
            })
        };
        let work = std::panic::AssertUnwindSafe(|| match &job {
            Job::Install(plan) => backend.install(plan, &progress),
            Job::Uninstall(dir) => backend.uninstall(dir, &progress),
        });
        let mut result = std::panic::catch_unwind(work).unwrap_or_else(|_| Err("The installer stopped unexpectedly.".into()));
        if result.is_ok() && super::demo_switch(backend, "ZIPSHRINK_SETUP_DEMO_FAIL").is_some() {
            result = Err(format!("Could not replace {}: access is denied.", dir.join("zipshrink-gui.exe").display()));
        }
        self.set(repaint, |s| s.phase = Phase::Done(result));
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cancelled(&self) -> bool {
        self.lock().cancelled
    }

    fn set(&self, repaint: &(dyn Fn() + Sync), f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.lock().snap);
        repaint();
    }

    pub fn snapshot(&self) -> Snapshot {
        self.lock().snap.clone()
    }

    /// Asks the task to stop (the window wants to close). True when it was
    /// still waiting or had not started, so nothing will be changed; false
    /// once the files are being changed or the task has ended.
    pub fn cancel(&self) -> bool {
        let mut st = self.lock();
        if st.begun {
            return false;
        }
        st.cancelled = true;
        true
    }
}

/// "Less than a minute", "About 3 minutes": the time left at this pace.
pub fn remaining(s: &Snapshot) -> String {
    let Some(started) = s.started else { return "Estimating…".into() };
    let elapsed = started.elapsed();
    if s.frac < 0.04 || elapsed < Duration::from_millis(600) {
        return "Estimating…".into();
    }
    let left = elapsed.as_secs_f32() * (1.0 - s.frac) / s.frac;
    match left {
        l if l < 60.0 => "Less than a minute".into(),
        l if l < 100.0 => "About a minute".into(),
        l => format!("About {} minutes", (l / 60.0).round() as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Components, Existing, Progress};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A backend whose ZipShrink is "open" until `running` is cleared, and
    /// whose install blocks until `release` is set.
    #[derive(Default)]
    struct Probe {
        running: AtomicBool,
        release: AtomicBool,
        installs: AtomicUsize,
    }

    impl Backend for Probe {
        fn check_system(&self) -> Result<(), String> {
            Ok(())
        }
        fn detect(&self) -> Option<Existing> {
            None
        }
        fn default_dir(&self) -> PathBuf {
            PathBuf::from("/probe")
        }
        fn free_space(&self, _: &Path) -> Option<u64> {
            None
        }
        fn app_running(&self, _: &Path) -> bool {
            self.running.load(Ordering::SeqCst)
        }
        fn wait_closed(&self, _: &Path, _: u32) -> bool {
            std::thread::sleep(Duration::from_millis(5));
            !self.app_running(Path::new(""))
        }
        fn install(&self, _: &Plan, progress: Progress) -> Result<(), String> {
            self.installs.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            progress(1.0, "Done");
            Ok(())
        }
        fn uninstall(&self, _: &Path, _: Progress) -> Result<(), String> {
            Ok(())
        }
        fn launch(&self, _: &Path) {}
        fn is_demo(&self) -> bool {
            false
        }
    }

    fn probe(running: bool) -> &'static Probe {
        let p: &'static Probe = Box::leak(Box::default());
        p.running.store(running, Ordering::SeqCst);
        p
    }

    fn install(p: &'static Probe) -> Arc<Task> {
        let plan = Plan { dir: PathBuf::from("/probe"), components: Components::default() };
        Task::spawn(p, Job::Install(plan), || {})
    }

    fn wait_for(t: &Task, what: impl Fn(&Phase) -> bool) -> Phase {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let phase = t.snapshot().phase;
            if what(&phase) {
                return phase;
            }
            assert!(Instant::now() < until, "stuck in {phase:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn cancel_while_waiting_changes_nothing() {
        let p = probe(true);
        let t = install(p);
        wait_for(&t, |ph| *ph == Phase::Waiting);
        assert!(t.cancel());
        // ZipShrink closes right after: the install must still not start.
        p.running.store(false, Ordering::SeqCst);
        p.release.store(true, Ordering::SeqCst);
        wait_for(&t, |ph| *ph == Phase::Cancelled);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(p.installs.load(Ordering::SeqCst), 0);
        assert_eq!(t.snapshot().phase, Phase::Cancelled);
    }

    #[test]
    fn cancel_once_working_is_refused_and_the_install_completes() {
        let p = probe(true);
        let t = install(p);
        wait_for(&t, |ph| *ph == Phase::Waiting);
        p.running.store(false, Ordering::SeqCst);
        wait_for(&t, |_| p.installs.load(Ordering::SeqCst) == 1);
        assert_eq!(t.snapshot().phase, Phase::Working);
        assert!(!t.cancel());
        p.release.store(true, Ordering::SeqCst);
        assert_eq!(wait_for(&t, |ph| matches!(ph, Phase::Done(_))), Phase::Done(Ok(())));
        assert_eq!(p.installs.load(Ordering::SeqCst), 1);
        assert!(!t.cancel(), "a finished task cannot be cancelled");
    }

    #[test]
    fn cancel_on_a_finished_task_is_refused() {
        let p = probe(false);
        p.release.store(true, Ordering::SeqCst);
        let t = install(p);
        assert_eq!(wait_for(&t, |ph| matches!(ph, Phase::Done(_))), Phase::Done(Ok(())));
        assert!(!t.cancel());
    }
}
