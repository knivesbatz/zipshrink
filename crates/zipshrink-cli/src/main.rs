//! `zipshrink`: create ZIP archives that reclaim disk space as they go.

use clap::{Args, Parser, Subcommand};
use console::style;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use zipshrink_core::engine::{Outcome, StopKind};
use zipshrink_core::journal::{self, Record};
use zipshrink_core::progress::{EntryStatus, Event, Phase, Stats};
use zipshrink_core::util::{human_bytes, parse_size};
use zipshrink_core::{salvage, scan, space, split, verify, CreateRequest, Error, Job, JobOptions, JobPaths, Report, Verdict};

mod exit {
    pub const OK: u8 = 0;
    pub const ERROR: u8 = 1;
    pub const ISSUES: u8 = 3;
    pub const NO_SPACE: u8 = 4;
    pub const INTERRUPTED: u8 = 5;
    pub const VERIFY_FAILED: u8 = 6;
    pub const DAMAGED: u8 = 7;
}

const AFTER_HELP: &str = "\
EXIT CODES:
  0  success
  1  error (invalid input, I/O error, job in use)
  2  invalid command line
  3  finished, but some files were skipped, failed or kept
  4  not enough disk space (nothing was changed, or the job paused safely)
  5  interrupted; run `zipshrink resume` to continue
  6  verification failed
  7  the partial archive or journal is damaged; see `zipshrink salvage`

Originals are never deleted unless --delete-originals is given.";

#[derive(Parser)]
#[command(
    name = "zipshrink",
    version,
    about = "Create ZIP archives that reclaim disk space as they go",
    long_about = "Creates standard ZIP archives (ZIP64 when needed). With --delete-originals, each original \
                  file is deleted only after its compressed copy has been flushed to disk, read back, \
                  decompressed and matched by CRC-32 and SHA-256, and recorded in a crash-safe journal. \
                  Interrupted jobs can be resumed.",
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create an archive from files and folders
    Create(CreateArgs),
    /// Check whether an archive would fit, without changing anything
    Check(CheckArgs),
    /// Resume an interrupted job
    Resume(ResumeArgs),
    /// Verify an archive: every entry decompressed and checked by CRC-32, and SHA-256 where stored
    Verify(VerifyArgs),
    /// Show the state of an unfinished job
    Status(StatusArgs),
    /// Rebuild a readable archive from a .partial file whose journal is lost or damaged
    Salvage(SalvageArgs),
    /// Check for a newer version, and on Windows download and start its installer
    Update(UpdateArgs),
}

#[derive(Args)]
struct UpdateArgs {
    /// Only report whether an update is available
    #[arg(long)]
    check: bool,
}

#[derive(Args, Clone)]
struct Common {
    /// Files and folders to archive
    #[arg(required = true, value_name = "SOURCE")]
    sources: Vec<PathBuf>,
    /// The ZIP file to create
    #[arg(short, long, value_name = "ZIP")]
    output: PathBuf,
    /// Compression level: 0 stores, 1 is fastest, 9 is smallest
    #[arg(short, long, default_value_t = 6, value_parser = clap::value_parser!(u32).range(0..=9))]
    level: u32,
    /// Free space to keep on the destination drive, e.g. 512M or 2G
    #[arg(long, value_name = "SIZE", default_value = "256M", value_parser = parse_size)]
    min_free: u64,
    /// Split into this many separate, independently readable ZIP files of about equal size
    /// (NAME.part1.zip, NAME.part2.zip, ...)
    #[arg(short = 'p', long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=999))]
    parts: u32,
}

#[derive(Args)]
struct CreateArgs {
    #[command(flatten)]
    common: Common,
    /// Permanently delete each original after its archived copy is verified on disk
    #[arg(long)]
    delete_originals: bool,
    /// Do not ask for confirmation before deleting originals
    #[arg(short, long)]
    yes: bool,
    /// Also delete read-only files (normally they are kept)
    #[arg(long, requires = "delete_originals")]
    delete_readonly: bool,
    /// Before deleting, compare size, times and file id instead of re-reading the whole file
    #[arg(long, requires = "delete_originals")]
    quick_change_check: bool,
    /// Replace the output archive if it exists (only once the new one is verified)
    #[arg(long)]
    overwrite: bool,
    /// Worker threads for compressing small files
    #[arg(short = 'j', long, default_value_t = default_threads())]
    threads: u32,
    /// Skip decompressing every entry again after finishing (each entry is still verified when written)
    #[arg(long)]
    no_final_verify: bool,
    /// Proceed even if the preflight check says the archive may not fit
    #[arg(long)]
    ignore_space_check: bool,
    /// Entries per commit batch (testing and tuning)
    #[arg(long, hide = true, default_value_t = 0)]
    batch_entries: u32,
    #[command(flatten)]
    out: OutputArgs,
}

#[derive(Args)]
struct CheckArgs {
    #[command(flatten)]
    common: Common,
    /// Assume originals will be deleted as they are archived
    #[arg(long)]
    delete_originals: bool,
}

#[derive(Args)]
struct ResumeArgs {
    /// The .partial file, its .journal, or the archive name
    path: PathBuf,
    /// Do not delete any more originals in this run
    #[arg(long)]
    no_delete: bool,
    /// Do not ask for confirmation before continuing to delete originals
    #[arg(short, long)]
    yes: bool,
    /// Change the free space to keep on the destination drive
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    min_free: Option<u64>,
    #[command(flatten)]
    out: OutputArgs,
}

#[derive(Args)]
struct VerifyArgs {
    archive: PathBuf,
    /// Only check the archive structure, without decompressing
    #[arg(long)]
    quick: bool,
}

#[derive(Args)]
struct StatusArgs {
    /// The .partial file, its .journal, or the archive name
    path: PathBuf,
}

#[derive(Args)]
struct SalvageArgs {
    /// The .partial archive
    partial: PathBuf,
    /// Name of the rebuilt archive (default: the name without .partial)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args, Clone, Copy)]
struct OutputArgs {
    /// Print only the summary and problems
    #[arg(short, long)]
    quiet: bool,
    /// List every file as it is archived and deleted
    #[arg(short, long)]
    verbose: bool,
}

fn default_threads() -> u32 {
    std::thread::available_parallelism().map(|n| n.get().min(4) as u32).unwrap_or(1)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cancel = Arc::new(AtomicBool::new(false));
    install_ctrlc(cancel.clone());
    let code = match cli.cmd {
        Cmd::Create(a) => cmd_create(a, &cancel),
        Cmd::Check(a) => cmd_check(a),
        Cmd::Resume(a) => cmd_resume(a, &cancel),
        Cmd::Verify(a) => cmd_verify(a, &cancel),
        Cmd::Status(a) => cmd_status(a),
        Cmd::Salvage(a) => cmd_salvage(a, &cancel),
        Cmd::Update(a) => cmd_update(a),
    };
    ExitCode::from(code)
}

fn install_ctrlc(cancel: Arc<AtomicBool>) {
    let presses = AtomicUsize::new(0);
    let _ = ctrlc::set_handler(move || {
        if presses.fetch_add(1, Ordering::SeqCst) == 0 {
            cancel.store(true, Ordering::SeqCst);
            eprintln!("\nStopping safely at the next safe point... (press Ctrl+C again to quit now; that is safe too)");
        } else {
            eprintln!("\nQuitting. Run `zipshrink resume` to continue.");
            std::process::exit(exit::INTERRUPTED as i32);
        }
    });
}

fn err(msg: impl std::fmt::Display) {
    eprintln!("{} {msg}", style("error:").red().bold());
}

fn warn(msg: impl std::fmt::Display) {
    eprintln!("{} {msg}", style("warning:").yellow().bold());
}

fn code_for(e: &Error) -> u8 {
    match e {
        Error::InsufficientSpace(_) | Error::LowSpace(_) | Error::DiskFull(_) => exit::NO_SPACE,
        Error::Cancelled => exit::INTERRUPTED,
        Error::VerifyFailed(_) => exit::VERIFY_FAILED,
        Error::Damaged(_) | Error::Journal(_) => exit::DAMAGED,
        _ => exit::ERROR,
    }
}

fn confirm_deletion(what: &str) -> bool {
    eprintln!();
    eprintln!("{}", style("  PERMANENT DELETION  ").white().on_red().bold());
    eprintln!("Each original file in {what} will be permanently deleted (not moved to the");
    eprintln!("Recycle Bin or Trash) right after its compressed copy is verified on disk.");
    eprintln!("Files that change, cannot be verified or are read-only are kept.");
    if !std::io::stdin().is_terminal() {
        warn("standard input is not a terminal; proceeding because --delete-originals was given");
        return true;
    }
    eprint!("Type {} to continue: ", style("delete").bold());
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    line.trim().eq_ignore_ascii_case("delete")
}

fn print_preflight(p: &space::Preflight) {
    println!("Selected:     {} files, {} folders, {}", p.files, p.dirs, human_bytes(p.bytes));
    println!(
        "Archive size: about {} ({}), at most {}",
        human_bytes(p.estimated_archive),
        p.sampled_ratio.map(|r| format!("{:.0}% of the original, sampled", r * 100.0)).unwrap_or_else(|| "no sample".into()),
        human_bytes(p.worst_case_archive)
    );
    println!(
        "Free space:   {} on {}{} (keeping {} free)",
        human_bytes(p.free_space),
        p.dest_dir.display(),
        p.fs_name.as_deref().map(|f| format!(" [{f}]")).unwrap_or_default(),
        human_bytes(p.min_free)
    );
    if p.delete_originals {
        println!(
            "Deletion:     originals deleted as they are archived; {} of them are on the destination drive",
            human_bytes(p.same_volume_bytes)
        );
    }
    let verdict = match p.verdict {
        Verdict::Fits => style(&p.explanation).green(),
        Verdict::ProbablyFits => style(&p.explanation).yellow(),
        Verdict::DoesNotFit => style(&p.explanation).red(),
    };
    println!("Result:       {verdict}");
    for w in &p.warnings {
        warn(w);
    }
    if !p.skipped.is_empty() {
        warn(format!("{} items will not be archived (and will not be deleted):", p.skipped.len()));
        for (path, why) in p.skipped.iter().take(20) {
            eprintln!("  {}: {why}", path.display());
        }
        if p.skipped.len() > 20 {
            eprintln!("  ... and {} more", p.skipped.len() - 20);
        }
    }
}

type Checked = (CreateRequest, scan::ScanResult, space::Preflight, Vec<split::PartPlan>);

fn scan_and_check(common: &Common, delete: bool) -> Result<Checked, Error> {
    let options = JobOptions { level: common.level, delete_originals: delete, min_free: common.min_free, ..Default::default() };
    let req = CreateRequest { sources: common.sources.clone(), output: common.output.clone(), options };
    let (paths, roots) = zipshrink_core::engine::validate(&req)?;
    let exclude = vec![paths.output.clone(), paths.partial.clone(), paths.journal.clone()];
    let s = scan::scan(&roots, &exclude, &AtomicBool::new(false))?;
    let p = space::preflight(&s, &paths, &req.options)?;
    let parts = if common.parts > 1 { split::split_scan(&s, &paths.output, common.parts as usize, p.sampled_ratio)? } else { Vec::new() };
    Ok((req, s, p, parts))
}

fn print_parts(parts: &[split::PartPlan]) {
    if parts.is_empty() {
        return;
    }
    println!("Split into {} separate ZIP files:", parts.len());
    for p in parts {
        println!(
            "  {:<40} {:>6} files  {:>10} of data  ->  about {:>10} (at most {})",
            p.output.file_name().unwrap_or_default().to_string_lossy(),
            p.files,
            human_bytes(p.bytes),
            human_bytes(p.estimated),
            human_bytes(p.worst_case)
        );
    }
}

fn cmd_check(a: CheckArgs) -> u8 {
    match scan_and_check(&a.common, a.delete_originals) {
        Ok((req, _, p, parts)) => {
            print_preflight(&p);
            print_parts(&parts);
            let paths = JobPaths::for_output(&zipshrink_core::engine::validate(&req).map(|x| x.0.output).unwrap_or(req.output));
            if paths.has_unfinished_job() {
                warn(format!("an unfinished job exists for this archive: run `zipshrink resume \"{}\"`", paths.partial.display()));
            } else if paths.output.exists() {
                warn(format!("{} already exists; creating it would need --overwrite", paths.output.display()));
            }
            if p.verdict == Verdict::DoesNotFit {
                exit::NO_SPACE
            } else {
                exit::OK
            }
        }
        Err(e) => {
            err(&e);
            code_for(&e)
        }
    }
}

fn cmd_create(a: CreateArgs, cancel: &AtomicBool) -> u8 {
    let (mut req, s, p, parts) = match scan_and_check(&a.common, a.delete_originals) {
        Ok(x) => x,
        Err(e) => {
            err(&e);
            return code_for(&e);
        }
    };
    if !a.out.quiet {
        print_preflight(&p);
        print_parts(&parts);
    }
    if p.verdict == Verdict::DoesNotFit && !a.ignore_space_check {
        err("not enough disk space; nothing was changed (use --ignore-space-check to try anyway; the job pauses safely if space runs out)");
        return exit::NO_SPACE;
    }
    if a.delete_originals && !a.yes {
        let what = if a.common.sources.len() == 1 {
            format!("\"{}\"", a.common.sources[0].display())
        } else {
            format!("the {} selected items", a.common.sources.len())
        };
        if !confirm_deletion(&what) {
            eprintln!("Cancelled; nothing was changed.");
            return exit::ERROR;
        }
    }
    req.options.delete_readonly = a.delete_readonly;
    req.options.quick_change_check = a.quick_change_check;
    req.options.overwrite = a.overwrite;
    req.options.threads = a.threads.max(1);
    req.options.final_data_verify = !a.no_final_verify;
    req.options.batch_entries = a.batch_entries;
    let jobs = if parts.is_empty() { Job::create_from_scan(&req, s).map(|j| vec![j]) } else { split::create_jobs(&req, &parts) };
    match jobs {
        Ok(jobs) => run_jobs(jobs, a.out, cancel),
        Err(e) => {
            err(&e);
            code_for(&e)
        }
    }
}

/// Runs jobs one after another, stopping at the first that does not finish.
fn run_jobs(jobs: Vec<Job>, out: OutputArgs, cancel: &AtomicBool) -> u8 {
    let n = jobs.len();
    let mut worst = exit::OK;
    for (i, mut job) in jobs.into_iter().enumerate() {
        if n > 1 && !out.quiet {
            println!("\n== Part {} of {n}: {}", i + 1, job.paths.output.display());
        }
        let code = run_job(&mut job, out, cancel);
        drop(job);
        if code != exit::OK && code != exit::ISSUES {
            if n > 1 && i + 1 < n {
                eprintln!("Parts {} to {n} were not started; `zipshrink resume` continues them too.", i + 2);
            }
            return code;
        }
        worst = worst.max(code);
    }
    worst
}

fn cmd_resume(a: ResumeArgs, cancel: &AtomicBool) -> u8 {
    let targets = split::resume_targets(&a.path);
    let mut jobs = Vec::new();
    for t in &targets {
        match Job::open(t) {
            Ok(mut job) => {
                if let Some(m) = a.min_free {
                    job.set_min_free(m);
                }
                if a.no_delete {
                    job.disable_deletion();
                }
                jobs.push(job);
            }
            Err(e) => {
                err(&e);
                return code_for(&e);
            }
        }
    }
    if !a.no_delete && !a.yes && jobs.iter().any(|j| j.options().delete_originals) {
        let job = &jobs[0];
        let what = if job.sources().len() == 1 {
            format!("\"{}\"", job.sources()[0].display())
        } else {
            format!("the {} selected items", job.sources().len())
        };
        eprintln!("This job was started with --delete-originals (use --no-delete to stop deleting).");
        if !confirm_deletion(&what) {
            eprintln!("Cancelled; nothing was changed.");
            return exit::ERROR;
        }
    }
    if !a.out.quiet {
        for j in &jobs {
            println!("Resuming {}", j.paths.partial.display());
        }
    }
    run_jobs(jobs, a.out, cancel)
}

fn cmd_update(a: UpdateArgs) -> u8 {
    use zipshrink_update as up;
    println!("ZipShrink {} — checking {} for updates…", up::CURRENT, up::REPO);
    let rel = match up::check() {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("You have the latest version.");
            return exit::OK;
        }
        Err(e) => {
            err(&e);
            return exit::ERROR;
        }
    };
    println!("{} ZipShrink {} is available: {}", style("Update:").green().bold(), rel.version, rel.page);
    if a.check {
        return exit::OK;
    }
    if up::method() == up::Method::ReleasePage {
        println!("Download it from the release page (this copy is portable or not on Windows).");
        up::open_url(&rel.page);
        return exit::OK;
    }
    let bar = ProgressBar::new(1);
    bar.set_style(ProgressStyle::with_template("Downloading [{bar:30.cyan/blue}] {bytes}/{total_bytes}").unwrap().progress_chars("━╸─"));
    let b2 = bar.clone();
    let res = up::download_installer(&rel, &mut |d, t| {
        b2.set_length(t.max(d));
        b2.set_position(d);
    });
    bar.finish_and_clear();
    match res {
        Ok(path) => {
            println!("Verified SHA-256 of {}; starting the installer.", path.display());
            match up::launch_installer(&path) {
                Ok(()) => exit::OK,
                Err(e) => {
                    err(e);
                    exit::ERROR
                }
            }
        }
        Err(e) => {
            err(&e);
            exit::ERROR
        }
    }
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan} {prefix:<11} [{bar:30.cyan/blue}] {percent:>3}%  {msg}").unwrap().progress_chars("━╸─")
}

fn stats_line(s: &Stats) -> String {
    let mut parts = vec![format!("{}/{} files", s.files_archived, s.total_files)];
    parts.push(format!("archive {}", human_bytes(s.archive_size)));
    if s.delete_originals {
        parts.push(format!("reclaimed {}", human_bytes(s.reclaimed)));
    }
    if let Some(f) = s.free_space {
        parts.push(format!("free {}", human_bytes(f)));
    }
    if let Some(r) = s.ratio() {
        parts.push(format!("ratio {:.0}%", r * 100.0));
    }
    if s.speed > 0.0 {
        parts.push(format!("{}/s", human_bytes(s.speed as u64)));
    }
    parts.join(" · ")
}

fn run_job(job: &mut Job, out: OutputArgs, cancel: &AtomicBool) -> u8 {
    let tty = std::io::stderr().is_terminal() && !out.quiet;
    let bar = ProgressBar::with_draw_target(Some(1000), if tty { ProgressDrawTarget::stderr() } else { ProgressDrawTarget::hidden() });
    bar.set_style(bar_style());
    bar.enable_steady_tick(Duration::from_millis(120));
    let verbose = out.verbose;
    let b2 = bar.clone();
    let mut obs = move |ev: Event| match ev {
        Event::Phase(p) => b2.set_prefix(match p {
            Phase::Scanning => "Scanning",
            Phase::Recovering => "Recovering",
            Phase::Compressing => "Compressing",
            Phase::Finalizing => "Finalizing",
            Phase::VerifyingArchive => "Verifying",
            Phase::Cleanup => "Cleaning up",
            Phase::Done => "Done",
        }),
        Event::Progress(s) => {
            b2.set_position((s.percent() * 10.0) as u64);
            let cur = s.current.as_ref().map(|c| {
                let name = shorten(&c.name, 40);
                if c.size > 0 && c.done > 0 {
                    format!("{name} {:.0}% · ", 100.0 * c.done as f64 / c.size as f64)
                } else {
                    format!("{name} · ")
                }
            });
            b2.set_message(format!("{}{}", cur.unwrap_or_default(), stats_line(&s)));
        }
        Event::Entry { name, status, .. } => {
            let line = match &status {
                EntryStatus::Failed(r) => Some(format!("{} {name}: {r}", style("failed").red())),
                EntryStatus::Kept(r) => Some(format!("{} {name}: {r}", style("kept").yellow())),
                EntryStatus::Deleted if verbose => Some(format!("{} {name}", style("deleted").green())),
                EntryStatus::Archived if verbose => Some(format!("{} {name}", style("archived").cyan())),
                _ => None,
            };
            if let Some(l) = line {
                b2.suspend(|| eprintln!("{l}"));
            }
        }
        Event::Message(m) => b2.suspend(|| eprintln!("{m}")),
    };
    let report = job.run(&mut obs, cancel);
    bar.finish_and_clear();
    print_report(&report, out.quiet)
}

fn shorten(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    let tail: String = s.chars().skip(count - (n - 1)).collect();
    format!("…{tail}")
}

fn print_report(r: &Report, quiet: bool) -> u8 {
    let _ = quiet;
    match &r.outcome {
        Outcome::Completed | Outcome::CompletedWithIssues => {
            println!();
            println!("{} {}", style("Archive created:").green().bold(), r.paths.output.display());
            println!("  Original size:   {}", human_bytes(r.original_bytes));
            println!(
                "  Archive size:    {}{}",
                human_bytes(r.archive_size),
                if r.original_bytes > 0 {
                    format!(" ({:.1}% of original)", 100.0 * r.archive_size as f64 / r.original_bytes as f64)
                } else {
                    String::new()
                }
            );
            println!("  Archived:        {} files, {} folders", r.files_archived, r.dirs_archived);
            if r.delete_requested {
                println!(
                    "  Originals:       {} deleted, {} kept; {} reclaimed; {} emptied folders removed",
                    r.originals_deleted,
                    r.kept.len(),
                    human_bytes(r.reclaimed),
                    r.dirs_removed
                );
            } else {
                println!("  Originals:       untouched");
            }
            if let Some(v) = &r.verification {
                println!(
                    "  Verification:    {} — {} entries; {}",
                    if v.ok() { style("passed").green().bold() } else { style("FAILED").red().bold() },
                    v.entries,
                    if v.data_checked {
                        format!("{} files decompressed and matched by SHA-256 and CRC-32", v.sha_checked + v.crc_only)
                    } else {
                        "structure checked (data was verified as each entry was written)".into()
                    }
                );
            }
            print_issues(r);
            if r.outcome == Outcome::Completed {
                exit::OK
            } else {
                exit::ISSUES
            }
        }
        Outcome::Stopped { error, resumable, kind } => {
            println!();
            err(error);
            print_issues(r);
            let p = &r.paths;
            if *resumable {
                eprintln!(
                    "\nThe job is paused. {} entries are safely stored in {}.\nTo continue: zipshrink resume \"{}\"",
                    r.recoverable_entries,
                    p.partial.display(),
                    p.partial.display()
                );
            } else if *kind == StopKind::Damaged || *kind == StopKind::VerifyFailed {
                eprintln!(
                    "\nNo further originals will be deleted. Do not delete {} or {}.\nTo inspect: zipshrink status \"{}\"\nTo rebuild a readable archive from what is intact: zipshrink salvage \"{}\"",
                    p.partial.display(),
                    p.journal.display(),
                    p.partial.display(),
                    p.partial.display()
                );
            }
            match kind {
                StopKind::Cancelled => exit::INTERRUPTED,
                StopKind::LowSpace | StopKind::DiskFull => exit::NO_SPACE,
                StopKind::Damaged => exit::DAMAGED,
                StopKind::VerifyFailed => exit::VERIFY_FAILED,
                StopKind::Other if *resumable => exit::INTERRUPTED,
                StopKind::Other => exit::ERROR,
            }
        }
    }
}

fn print_issues(r: &Report) {
    let list = |title: &str, items: Vec<String>| {
        if items.is_empty() {
            return;
        }
        warn(format!("{title} ({}):", items.len()));
        for i in items.iter().take(50) {
            eprintln!("  {i}");
        }
        if items.len() > 50 {
            eprintln!("  ... and {} more", items.len() - 50);
        }
    };
    list("not archived, left in place", r.failed.iter().map(|(p, w)| format!("{}: {w}", p.display())).collect());
    list("skipped", r.skipped.iter().map(|(p, w)| format!("{}: {w}", p.display())).collect());
    list("archived, original kept", r.kept.iter().map(|(n, w)| format!("{n}: {w}")).collect());
    for w in &r.warnings {
        warn(w);
    }
}

fn cmd_verify(a: VerifyArgs, cancel: &AtomicBool) -> u8 {
    let total = std::fs::metadata(&a.archive).map(|m| m.len()).unwrap_or(0);
    let bar = ProgressBar::with_draw_target(
        Some(total.max(1)),
        if std::io::stderr().is_terminal() { ProgressDrawTarget::stderr() } else { ProgressDrawTarget::hidden() },
    );
    bar.set_style(
        ProgressStyle::with_template("{spinner:.cyan} Verifying [{bar:30.cyan/blue}] {bytes}  {bytes_per_sec}")
            .unwrap()
            .progress_chars("━╸─"),
    );
    let b2 = bar.clone();
    let res = verify::check_archive(&a.archive, !a.quick, cancel, &mut |k| b2.inc(k));
    bar.finish_and_clear();
    match res {
        Ok(c) if c.ok() => {
            println!("{} {}", style("Verified:").green().bold(), a.archive.display());
            println!("  {} entries ({} files, {} folders), {}", c.entries, c.files, c.dirs, human_bytes(c.bytes));
            if c.data_checked {
                println!("  {} files matched their stored SHA-256 and CRC-32", c.sha_checked);
                if c.crc_only > 0 {
                    println!("  {} files have no stored SHA-256 (not made by ZipShrink) and matched their CRC-32", c.crc_only);
                }
            } else {
                println!("  structure only (--quick); data was not decompressed");
            }
            exit::OK
        }
        Ok(c) => {
            err(format!("{} failed verification:", a.archive.display()));
            for p in c.problems.iter().take(100) {
                eprintln!("  {p}");
            }
            exit::VERIFY_FAILED
        }
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => exit::INTERRUPTED,
        Err(e) => {
            err(format!("{}: {e}", a.archive.display()));
            exit::VERIFY_FAILED
        }
    }
}

fn cmd_status(a: StatusArgs) -> u8 {
    let paths = JobPaths::from_any(&a.path);
    let loaded = match journal::load(&paths.journal) {
        Ok(l) => l,
        Err(e) => {
            err(format!("{}: {e}", paths.journal.display()));
            return if paths.journal.exists() { exit::DAMAGED } else { exit::ERROR };
        }
    };
    let (mut planned, mut files, mut bytes, mut commits, mut committed_bytes, mut deleted, mut kept, mut failed) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut end = 0u64;
    let (mut finalizing, mut finalized, mut delete) = (false, false, false);
    for r in &loaded.records {
        match r {
            Record::Header(h) => {
                println!("Job {} created {} by ZipShrink {}", h.job_id, h.created, h.app_version);
                println!("Output:  {}", h.output.display());
                for s in &h.sources {
                    println!("Source:  {}", s.display());
                }
                delete = h.options.delete_originals;
            }
            Record::Plan(p) => {
                planned += 1;
                if !p.meta.is_dir {
                    files += 1;
                    bytes += p.size;
                }
            }
            Record::Commit(c) => {
                commits += 1;
                committed_bytes += c.usize;
                end = c.end;
            }
            Record::Deleted { .. } => deleted += 1,
            Record::Kept { .. } => kept += 1,
            Record::Failed { .. } => failed += 1,
            Record::FinalizeStart { .. } => finalizing = true,
            Record::Finalized { .. } => finalized = true,
            _ => {}
        }
    }
    println!("Planned: {planned} entries ({files} files, {})", human_bytes(bytes));
    println!("Safely archived: {commits} entries, {} of data, ending at byte {end}", human_bytes(committed_bytes));
    println!("Deletion of originals: {}", if delete { format!("on — {deleted} deleted, {kept} kept") } else { "off".into() });
    if failed > 0 {
        println!("Failures recorded: {failed} (retried on resume)");
    }
    let partial_len = std::fs::metadata(&paths.partial).map(|m| m.len()).ok();
    match partial_len {
        Some(l) if l >= end => {
            println!("Partial archive: {} ({} bytes; {} beyond the last commit will be discarded)", paths.partial.display(), l, l - end)
        }
        Some(l) => println!("Partial archive: {} is SHORTER than committed ({l} < {end}): damaged", paths.partial.display()),
        None => println!("Partial archive: missing"),
    }
    if loaded.torn_tail {
        println!("The last journal record was cut off by the interruption and will be ignored.");
    }
    println!(
        "State: {}",
        if finalized {
            "finished; only cleanup remains"
        } else if finalizing {
            "interrupted while finalizing"
        } else {
            "interrupted while compressing"
        }
    );
    println!("To continue: zipshrink resume \"{}\"", paths.partial.display());
    exit::OK
}

fn cmd_salvage(a: SalvageArgs, cancel: &AtomicBool) -> u8 {
    let paths = JobPaths::from_any(&a.partial);
    if paths.journal.exists() {
        warn(format!(
            "a journal exists for this job; `zipshrink resume \"{}\"` is the normal way to finish it. Salvage ignores the journal.",
            paths.partial.display()
        ));
    }
    let output = a.output.unwrap_or_else(|| salvage::default_output(&a.partial));
    let bar = ProgressBar::with_draw_target(
        Some(1000),
        if std::io::stderr().is_terminal() { ProgressDrawTarget::stderr() } else { ProgressDrawTarget::hidden() },
    );
    bar.set_style(ProgressStyle::with_template("{spinner:.cyan} Scanning [{bar:30.cyan/blue}] {percent}%").unwrap());
    let b2 = bar.clone();
    let res = salvage::salvage(&a.partial, &output, cancel, &mut |p, t| b2.set_position((p * 1000).checked_div(t).unwrap_or(0)));
    bar.finish_and_clear();
    match res {
        Ok(r) => {
            println!("{} {}", style("Recovered:").green().bold(), r.output.display());
            println!("  {} entries ({} files, {} folders), {}", r.entries, r.files, r.dirs, human_bytes(r.bytes));
            println!("  every recovered file matched its CRC-32 (and SHA-256 where stored)");
            for (from, to) in &r.damaged {
                warn(format!("bytes {from}..{to} held no complete entry (an interrupted write, or damage) and were skipped"));
            }
            if paths.journal.exists() {
                println!(
                    "The old journal {} is no longer needed by this archive; keep it until you have checked the result.",
                    paths.journal.display()
                );
            }
            exit::OK
        }
        Err(e) => {
            err(&e);
            code_for(&e)
        }
    }
}
