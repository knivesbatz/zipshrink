//! Background threads: the UI thread never touches the disk or network.

use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
use zipshrink_core::progress::Event;
use zipshrink_core::split::{self, PartPlan};
use zipshrink_core::verify::ArchiveCheck;
use zipshrink_core::{engine, scan, space, CreateRequest, Job, JobOptions, JobPaths, Report};
use zipshrink_update::Release;

pub struct Checked {
    pub req: CreateRequest,
    pub scan: scan::ScanResult,
    pub preflight: space::Preflight,
    pub parts: Vec<PartPlan>,
    /// Archives that already exist (the output, or any part).
    pub existing: Vec<PathBuf>,
    /// An unfinished job for the output or one of its parts.
    pub unfinished: Option<PathBuf>,
}

/// Scans the selection, estimates space and plans the split, off the UI thread.
pub fn preflight(
    ctx: egui::Context,
    sources: Vec<PathBuf>,
    output: PathBuf,
    options: JobOptions,
    parts: usize,
) -> Receiver<Result<Checked, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = (|| {
            let req = CreateRequest { sources, output, options };
            let (paths, roots) = engine::validate(&req).map_err(|e| e.to_string())?;
            let exclude = vec![paths.output.clone(), paths.partial.clone(), paths.journal.clone()];
            let scan = scan::scan(&roots, &exclude, &AtomicBool::new(false)).map_err(|e| e.to_string())?;
            let preflight = space::preflight(&scan, &paths, &req.options).map_err(|e| e.to_string())?;
            let plans = if parts > 1 {
                split::split_scan(&scan, &paths.output, parts, preflight.sampled_ratio).map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            let outputs: Vec<PathBuf> =
                if plans.is_empty() { vec![paths.output.clone()] } else { plans.iter().map(|p| p.output.clone()).collect() };
            let existing = outputs.iter().filter(|o| o.exists()).cloned().collect();
            let unfinished = outputs.iter().map(|o| JobPaths::for_output(o)).find(|p| p.has_unfinished_job()).map(|p| p.partial);
            Ok(Checked { req, scan, preflight, parts: plans, existing, unfinished })
        })();
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

pub enum Msg {
    /// Part `number` of `of` started (1 of 1 for a single archive).
    Part {
        number: usize,
        of: usize,
        output: PathBuf,
    },
    Event(Event),
    PartDone(Report),
    /// Every job ran (or the sequence stopped at a job that did not finish).
    AllDone,
    /// Nothing could start; nothing was changed.
    Failed(String),
}

pub enum Start {
    Create { req: CreateRequest, scan: scan::ScanResult, parts: Vec<PartPlan> },
    Resume { path: PathBuf, no_delete: bool },
}

pub struct Running {
    pub rx: Receiver<Msg>,
    pub cancel: Arc<AtomicBool>,
}

pub fn run(ctx: egui::Context, start: Start) -> Running {
    let (tx, rx) = channel::<Msg>();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        let jobs: Result<Vec<Job>, String> = match start {
            Start::Create { req, scan, parts } => {
                if parts.is_empty() {
                    Job::create_from_scan(&req, scan).map(|j| vec![j]).map_err(|e| e.to_string())
                } else {
                    split::create_jobs(&req, &parts).map_err(|e| e.to_string())
                }
            }
            Start::Resume { path, no_delete } => split::resume_targets(&path)
                .iter()
                .map(|t| {
                    Job::open(t).map(|mut j| {
                        if no_delete {
                            j.disable_deletion();
                        }
                        j
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string()),
        };
        let jobs = match jobs {
            Ok(j) => j,
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
                ctx.request_repaint();
                return;
            }
        };
        let of = jobs.len();
        for (i, mut job) in jobs.into_iter().enumerate() {
            let _ = tx.send(Msg::Part { number: i + 1, of, output: job.paths.output.clone() });
            let tx2: Sender<Msg> = tx.clone();
            let ctx2 = ctx.clone();
            let mut obs = move |ev: Event| {
                let _ = tx2.send(Msg::Event(ev));
                ctx2.request_repaint_after(Duration::from_millis(50));
            };
            let report = job.run(&mut obs, &c2);
            let finished = report.finished();
            let _ = tx.send(Msg::PartDone(report));
            if !finished {
                break;
            }
        }
        let _ = tx.send(Msg::AllDone);
        ctx.request_repaint();
    });
    Running { rx, cancel }
}

pub fn verify(ctx: egui::Context, path: PathBuf, progress: Arc<AtomicU64>) -> Receiver<Result<ArchiveCheck, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = zipshrink_core::verify::check_archive(&path, true, &AtomicBool::new(false), &mut |k| {
            progress.fetch_add(k, Ordering::Relaxed);
            ctx.request_repaint_after(Duration::from_millis(100));
        })
        .map_err(|e| e.to_string());
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

pub fn check_update(ctx: egui::Context) -> Receiver<Result<Option<Release>, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(zipshrink_update::check().map_err(|e| e.to_string()));
        ctx.request_repaint();
    });
    rx
}

pub fn download_update(
    ctx: egui::Context,
    release: Release,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
) -> Receiver<Result<PathBuf, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = zipshrink_update::download_installer(&release, &mut |d, t| {
            done.store(d, Ordering::Relaxed);
            total.store(t, Ordering::Relaxed);
            ctx.request_repaint_after(Duration::from_millis(100));
        })
        .map_err(|e| e.to_string());
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

/// Shows `path` in the system file manager.
pub fn reveal(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        let _ = std::process::Command::new("explorer.exe").arg(arg).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
