//! The install or uninstall running on a background thread, so the window
//! keeps drawing. It first waits for a running ZipShrink to close.

use crate::ops::{Backend, Plan};
use eframe::egui::Context;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub enum Job {
    Install(Plan),
    Uninstall(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// ZipShrink is open; the files cannot be replaced until it quits.
    Waiting,
    /// Files are being changed: the window cannot be closed now.
    Working,
    Done(Result<(), String>),
}

#[derive(Clone)]
pub struct Snapshot {
    pub phase: Phase,
    pub frac: f32,
    pub status: String,
    /// When the work (not the waiting) started.
    pub started: Option<Instant>,
}

pub struct Task {
    state: Mutex<Snapshot>,
    cancel: AtomicBool,
}

impl Task {
    pub fn start(ctx: &Context, backend: &'static dyn Backend, job: Job) -> Arc<Task> {
        let task = Arc::new(Task {
            state: Mutex::new(Snapshot { phase: Phase::Working, frac: 0.0, status: "Preparing…".into(), started: None }),
            cancel: AtomicBool::new(false),
        });
        let (t, ctx) = (task.clone(), ctx.clone());
        std::thread::spawn(move || {
            let dir = match &job {
                Job::Install(plan) => plan.dir.clone(),
                Job::Uninstall(dir) => dir.clone(),
            };
            let pretend_open = super::demo_switch(backend, "ZIPSHRINK_SETUP_DEMO_RUNNING").and_then(|s| s.parse::<u64>().ok());
            if let Some(secs) = pretend_open {
                t.set(&ctx, |s| s.phase = Phase::Waiting);
                let until = Instant::now() + Duration::from_secs(secs);
                while Instant::now() < until {
                    if t.cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            } else if backend.app_running(&dir) {
                t.set(&ctx, |s| s.phase = Phase::Waiting);
                // Short waits, so quitting the installer is noticed quickly.
                while !backend.wait_closed(&dir, 1) {
                    if t.cancel.load(Ordering::SeqCst) {
                        return;
                    }
                }
            }
            t.set(&ctx, |s| {
                s.phase = Phase::Working;
                s.started = Some(Instant::now());
            });
            let progress = |f: f32, status: &str| {
                t.set(&ctx, |s| {
                    s.frac = f.clamp(0.0, 1.0);
                    if !status.is_empty() {
                        status.clone_into(&mut s.status);
                    }
                })
            };
            let run = std::panic::AssertUnwindSafe(|| match &job {
                Job::Install(plan) => backend.install(plan, &progress),
                Job::Uninstall(dir) => backend.uninstall(dir, &progress),
            });
            let mut result = std::panic::catch_unwind(run).unwrap_or_else(|_| Err("The installer stopped unexpectedly.".into()));
            if result.is_ok() && super::demo_switch(backend, "ZIPSHRINK_SETUP_DEMO_FAIL").is_some() {
                result = Err(format!("Could not replace {}: access is denied.", dir.join("zipshrink-gui.exe").display()));
            }
            t.set(&ctx, |s| s.phase = Phase::Done(result));
        });
        task
    }

    fn set(&self, ctx: &Context, f: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut s) = self.state.lock() {
            f(&mut s);
        }
        ctx.request_repaint();
    }

    pub fn snapshot(&self) -> Snapshot {
        match self.state.lock() {
            Ok(s) => s.clone(),
            Err(e) => e.into_inner().clone(),
        }
    }

    /// Stops waiting for ZipShrink to close (the window is closing).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
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
