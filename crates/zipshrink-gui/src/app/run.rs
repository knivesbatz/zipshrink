//! The running job and its result.

use super::compress::{column, shortcut, Flow};
use super::fmt;
use super::{App, Page};
use crate::design::{self, icons, motion, pal, space, typ, Button, Col, Icon, Tone};
use crate::worker::{Msg, Running};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, Id, Layout, Rect, Sense, Ui};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use zipshrink_core::engine::{Outcome, StopKind};
use zipshrink_core::progress::{Event, Phase, Stats};
use zipshrink_core::{EntryStatus, Report};

#[derive(Clone)]
pub struct Row {
    pub idx: u64,
    pub name: String,
    pub status: EntryStatus,
}

pub struct RunView {
    running: Running,
    pub stats: Stats,
    phase: Phase,
    part: (usize, usize, PathBuf),
    reports: Vec<Report>,
    recent: VecDeque<Row>,
    issues: Vec<Row>,
    stopping: bool,
    pub close_after: bool,
    tab: usize,
}

pub enum Polled {
    Running,
    Done(Box<DoneView>),
    Failed(String),
}

impl RunView {
    pub fn new(running: Running, delete: bool) -> Self {
        RunView {
            running,
            stats: Stats { delete_originals: delete, ..Default::default() },
            phase: Phase::Scanning,
            part: (1, 1, PathBuf::new()),
            reports: Vec::new(),
            recent: VecDeque::new(),
            issues: Vec::new(),
            stopping: false,
            close_after: false,
            tab: 0,
        }
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Phase(p) => self.phase = p,
            Event::Progress(s) => {
                self.phase = s.phase;
                self.stats = s;
            }
            Event::Message(m) => self.issues.push(Row { idx: u64::MAX, name: m, status: EntryStatus::Waiting }),
            Event::Entry { idx, name, status, .. } => {
                if matches!(status, EntryStatus::Failed(_) | EntryStatus::Kept(_) | EntryStatus::Skipped(_)) {
                    self.issues.push(Row { idx, name: name.clone(), status: status.clone() });
                }
                if let Some(r) = self.recent.iter_mut().rev().take(64).find(|r| r.idx == idx && r.name == name) {
                    r.status = status;
                } else {
                    self.recent.push_back(Row { idx, name, status });
                    while self.recent.len() > 300 {
                        self.recent.pop_front();
                    }
                }
            }
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Polled {
        while let Ok(m) = self.running.rx.try_recv() {
            match m {
                Msg::Part { number, of, output } => self.part = (number, of, output),
                Msg::Event(e) => self.on_event(e),
                Msg::PartDone(r) => self.reports.push(r),
                Msg::AllDone => {
                    if self.close_after {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    return Polled::Done(Box::new(DoneView {
                        reports: std::mem::take(&mut self.reports),
                        issues: std::mem::take(&mut self.issues),
                        show_details: false,
                    }));
                }
                Msg::Failed(e) => return Polled::Failed(e),
            }
        }
        Polled::Running
    }

    pub fn stop(&mut self, close_after: bool) {
        self.stopping = true;
        self.close_after |= close_after;
        self.running.cancel.store(true, Ordering::SeqCst);
    }
}

pub struct DoneView {
    pub reports: Vec<Report>,
    issues: Vec<Row>,
    show_details: bool,
}

impl DoneView {
    pub fn headline(&self) -> (Tone, &'static str) {
        let last = self.reports.last();
        let issues = self.reports.iter().any(|r| r.outcome == Outcome::CompletedWithIssues);
        match last.map(|r| &r.outcome) {
            Some(Outcome::Stopped { kind: StopKind::Cancelled, .. }) => (Tone::Info, "Paused"),
            Some(Outcome::Stopped { resumable: true, .. }) => (Tone::Warning, "Paused — needs your attention"),
            Some(Outcome::Stopped { .. }) | None => (Tone::Error, "The job stopped"),
            _ if issues => (Tone::Warning, "Done, with notes"),
            _ if self.reports.len() > 1 => (Tone::Success, "Archives created"),
            _ => (Tone::Success, "Archive created"),
        }
    }

    pub fn saved(&self) -> (u64, u64, u64) {
        let o = self.reports.iter().map(|r| r.original_bytes).sum();
        let a = self.reports.iter().map(|r| r.archive_size).sum();
        let f = self.reports.iter().map(|r| r.reclaimed).sum();
        (o, a, f)
    }
}

fn eta(s: &Stats) -> Option<Duration> {
    if s.speed <= 1.0 || s.total_bytes == 0 || s.phase != Phase::Compressing {
        return None;
    }
    Some(Duration::from_secs_f64(s.total_bytes.saturating_sub(s.bytes_processed) as f64 / s.speed))
}

fn status_style(st: &EntryStatus, p: &design::tokens::Palette) -> (Color32, &'static str) {
    match st {
        EntryStatus::Deleted | EntryStatus::Archived => (p.green, st.label()),
        EntryStatus::Kept(_) | EntryStatus::Skipped(_) => (p.orange, st.label()),
        EntryStatus::Failed(_) => (p.red, st.label()),
        EntryStatus::Waiting | EntryStatus::Recoverable => (p.text_tertiary, st.label()),
        EntryStatus::Compressing | EntryStatus::Verifying => (p.accent, st.label()),
    }
}

/// A Finder-style table of file activity.
fn activity_list(ui: &mut Ui, rows: &[Row], height: f32, empty: &str) {
    let p = pal(ui.ctx());
    let cols = [Col { title: "Name", width: 0.0, right: false }, Col { title: "Status", width: 230.0, right: false }];
    design::table_frame(ui, |ui| {
        let ranges = design::table_header(ui, &cols);
        egui::ScrollArea::vertical()
            .max_height(height)
            .min_scrolled_height(height.min(design::ROW_H * 4.0))
            .auto_shrink([false, true])
            .show_rows(ui, design::ROW_H, rows.len().max(4), |ui, range| {
                for i in range {
                    let (rect, _, fg, fg2) = design::table_row(ui, i, false, false);
                    let Some(r) = rows.get(i) else {
                        if i == 0 && !empty.is_empty() {
                            ui.painter().text(
                                pos2(ranges[0].0, rect.center().y),
                                Align2::LEFT_CENTER,
                                empty,
                                typ::body(),
                                p.text_secondary,
                            );
                        }
                        continue;
                    };
                    let (c, label) = status_style(&r.status, &p);
                    let y = rect.center().y;
                    let name = design::elide_start(ui, &r.name, typ::body(), ranges[0].1 - ranges[0].0);
                    ui.painter().text(pos2(ranges[0].0, y), Align2::LEFT_CENTER, name, typ::body(), fg);
                    ui.painter().circle_filled(pos2(ranges[1].0 + 4.0, y), 3.5, c);
                    let detail = r.status.detail().map(|d| format!("{label}: {d}")).unwrap_or_else(|| label.to_string());
                    let detail = design::elide_end(ui, &detail, typ::body(), ranges[1].1 - ranges[1].0 - 14.0);
                    ui.painter().text(pos2(ranges[1].0 + 14.0, y), Align2::LEFT_CENTER, detail, typ::body(), fg2);
                }
            });
    });
}

// ================================================================ running

pub fn running_page(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    let Flow::Run(rv) = &mut app.compress.flow else { return };
    if !rv.stopping {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    let s = rv.stats.clone();
    let mut pause = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(space::XL);
        column(ui, |ui| {
            let (n, of, out) = rv.part.clone();
            let verb = if rv.stopping {
                "Pausing safely…".to_string()
            } else {
                match rv.phase {
                    Phase::Scanning => "Preparing…".into(),
                    Phase::Recovering => "Checking the interrupted job…".into(),
                    Phase::Compressing => format!("Compressing to “{}”", fmt::file_name(&out)),
                    Phase::Finalizing => format!("Finishing “{}”…", fmt::file_name(&out)),
                    Phase::VerifyingArchive => format!("Verifying “{}”…", fmt::file_name(&out)),
                    Phase::Cleanup => "Tidying up…".into(),
                    Phase::Done => "Done".into(),
                }
            };
            // Finder's copy window: icon, line, bar, detail line, stop button.
            ui.horizontal_top(|ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                design::chrome::paint_app_icon(ui, ir);
                ui.add_space(space::S);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 36.0);
                    ui.add_space(2.0);
                    design::truncated(ui, &verb, typ::body_medium(), p.text);
                    ui.add_space(6.0);
                    let frac = match rv.phase {
                        Phase::VerifyingArchive if s.verify_total == 0 => None,
                        Phase::VerifyingArchive => Some((s.verify_done as f32 / s.verify_total as f32).min(1.0)),
                        Phase::Scanning | Phase::Recovering => None,
                        _ => Some((s.percent() / 100.0) as f32),
                    };
                    design::progress_bar(ui, Id::unique("run-bar"), frac, 6.0);
                    ui.add_space(6.0);
                    let mut line = if rv.phase == Phase::VerifyingArchive && s.verify_total == 0 {
                        "Reading back every entry…".to_string()
                    } else if rv.phase == Phase::VerifyingArchive {
                        format!("{} of {} read back", fmt::size(s.verify_done), fmt::size(s.verify_total))
                    } else {
                        format!("{} of {}", fmt::size(s.bytes_processed.min(s.total_bytes)), fmt::size(s.total_bytes))
                    };
                    if let Some(e) = eta(&s) {
                        line.push_str(&format!(" — About {} remaining", fmt::duration(e).to_lowercase()));
                    }
                    if of > 1 {
                        line.push_str(&format!(" · Archive {n} of {of}"));
                    }
                    design::truncated(ui, line, typ::callout(), p.text_secondary);
                });
                let r = ui.add_enabled_ui(!rv.stopping, |ui| {
                    ui.add_space(14.0);
                    design::icon_button(ui, Icon::Close, &format!("Pause at the next safe point ({})", shortcut(ui, "Ctrl+.")), 18.0)
                });
                if r.inner.clicked() {
                    pause = true;
                }
            });
            ui.add_space(space::L);
            if let Some(c) = &s.current {
                let st = c.status.clone().unwrap_or(EntryStatus::Compressing);
                design::key_value(ui, "Now:", format!("{} — {}", st.label(), c.name), None);
            }
            design::key_value(
                ui,
                "Files:",
                format!("{} archived, {} to go", fmt::group(s.files_archived), fmt::group(s.remaining_files())),
                None,
            );
            design::key_value(
                ui,
                "Archive size:",
                format!(
                    "{}{}",
                    fmt::size(s.archive_size),
                    s.ratio().map(|r| format!(" ({:.0}% of the original)", r * 100.0)).unwrap_or_default()
                ),
                None,
            );
            if s.delete_originals {
                design::key_value(
                    ui,
                    "Space freed:",
                    format!("{} — {} originals deleted", fmt::size(s.reclaimed), fmt::group(s.originals_deleted)),
                    None,
                );
            } else {
                design::key_value(ui, "Originals:", "Kept (deletion is off)", None);
            }
            if s.speed > 0.0 {
                design::key_value(ui, "Speed:", format!("{}/s", fmt::size(s.speed as u64)), None);
            }
            if let Some(f) = s.free_space {
                design::key_value(ui, "Free on drive:", fmt::size(f), None);
            }
            if s.originals_kept + s.files_failed > 0 {
                design::key_value(ui, "Kept / failed:", format!("{} / {}", s.originals_kept, s.files_failed), Some(p.orange));
            }
            ui.add_space(space::L);
            ui.horizontal(|ui| {
                let issues = rv.issues.len();
                let labels = ["Activity".to_string(), if issues > 0 { format!("Notes ({issues})") } else { "Notes".into() }];
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                design::segmented(ui, "run-tabs", &mut rv.tab, &refs, 64.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let r = ui.add_enabled_ui(!rv.stopping, |ui| Button::secondary("Pause").show(ui)).inner;
                    if r.on_hover_text("Stops at the next safe point. Everything archived so far is kept, and you can resume later.")
                        .clicked()
                    {
                        pause = true;
                    }
                });
            });
            ui.add_space(space::S);
            let h = (ui.available_height() - space::L).max(design::ROW_H * 6.0);
            if rv.tab == 0 {
                let rows: Vec<Row> = rv.recent.iter().rev().take(300).cloned().collect();
                activity_list(ui, &rows, h, "Files appear here as they're archived.");
            } else {
                activity_list(ui, &rv.issues, h, "No notes so far.");
            }
        });
    });
    if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Period))) {
        pause = true;
    }
    if pause {
        rv.stop(false);
    }
}

// ================================================================ done

enum DoneAct {
    Reveal(PathBuf),
    Verify(Vec<PathBuf>),
    Resume(PathBuf, bool),
    New,
}

pub fn done_page(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    let Flow::Done(dv) = &mut app.compress.flow else { return };
    let mut act = None;
    let Some(last) = dv.reports.last().cloned() else { return };
    let finished_all = dv.reports.iter().all(|r| r.finished());
    let (tone, headline) = dv.headline();
    let (orig, arch, freed) = dv.saved();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(space::XXL);
        column(ui, |ui| {
            ui.vertical_centered(|ui| {
                // The app icon with a status badge that draws itself in.
                let t = motion::appear(&ctx, Id::unique("done-mark"), 1, 0.7);
                let (rect, _) = ui.allocate_exact_size(vec2(80.0, 80.0), Sense::hover());
                design::chrome::paint_app_icon(ui, rect);
                let c = tone.color(&p);
                let bc = rect.right_bottom() - vec2(12.0, 12.0);
                let scale = if motion::reduced(&ctx) { 1.0 } else { design::tokens::motion::ease_out((t * 1.8).min(1.0)) };
                if scale > 0.0 {
                    ui.painter().circle_filled(bc, 15.0 * scale, p.bg);
                    ui.painter().circle_filled(bc, 13.0 * scale, c);
                    if tone == Tone::Success {
                        let draw_t = if motion::reduced(&ctx) { 1.0 } else { ((t - 0.35) / 0.65).clamp(0.0, 1.0) };
                        icons::check_stroke(ui.painter(), Rect::from_center_size(bc, vec2(18.0, 18.0)), Color32::WHITE, 2.4, draw_t);
                    } else {
                        icons::badge(ui.painter(), bc, 13.0 * scale, c, tone.icon(), Color32::WHITE);
                    }
                }
                ui.add_space(space::M);
                design::text(ui, headline, typ::title2(), p.text);
                ui.add_space(2.0);
                let where_ = if dv.reports.len() > 1 {
                    format!("{} ZIP files in {}", dv.reports.len(), fmt::parent(&last.paths.output))
                } else if last.finished() {
                    last.paths.output.display().to_string()
                } else {
                    last.paths.partial.display().to_string()
                };
                design::secondary(ui, where_);
            });
            ui.add_space(space::XL);
            if let Outcome::Stopped { error, resumable, .. } = &last.outcome {
                let body = if *resumable {
                    format!("{} entries are safely stored in {}. Nothing archived so far is lost.", fmt::group(last.recoverable_entries), fmt::file_name(&last.paths.partial))
                } else {
                    "No further originals will be deleted. Keep the .partial and .journal files: in a command prompt, \"zipshrink status\" shows what they hold and \"zipshrink salvage\" rebuilds a readable archive from the intact entries.".to_string()
                };
                design::notice(ui, if *resumable { Tone::Info } else { Tone::Error }, error, Some(&body));
                ui.add_space(space::L);
            }
            design::key_value(ui, "Original size:", format!("{} in {}", fmt::size(orig), fmt::count(dv.reports.iter().map(|r| r.files_archived).sum(), "file", "files")), None);
            design::key_value(ui, if finished_all { "Archive size:" } else { "Archived so far:" }, format!("{} ({} of the original)", fmt::size(arch), fmt::percent(arch, orig)), None);
            if last.delete_requested {
                let deleted: u64 = dv.reports.iter().map(|r| r.originals_deleted).sum();
                design::key_value(ui, "Space freed:", format!("{} — {} originals deleted", fmt::size(freed), fmt::group(deleted)), Some(p.green));
            } else {
                design::key_value(ui, "Originals:", "Kept (deletion was off)", None);
            }
            let sk: usize = dv.reports.iter().map(|r| r.skipped.len() + r.failed.len() + r.kept.len()).sum();
            if sk > 0 {
                design::key_value(ui, "Left out or kept:", fmt::count(sk as u64, "item", "items"), Some(p.orange));
            }
            let checks: Vec<_> = dv.reports.iter().filter_map(|r| r.verification.as_ref()).collect();
            let verified = if checks.is_empty() {
                ("Not yet — the job didn't finish.".to_string(), None)
            } else if checks.iter().all(|c| c.ok()) {
                let n: u64 = checks.iter().map(|c| c.sha_checked + c.crc_only).sum();
                (format!("Every entry read back; {} matched SHA-256 and CRC-32.", fmt::count(n, "file", "files")), Some(p.green))
            } else {
                ("Problems found — see Notes below.".to_string(), Some(p.red))
            };
            design::key_value(ui, "Verified:", verified.0, verified.1);
            ui.add_space(space::XL);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if finished_all {
                    if Button::primary(if cfg!(windows) { "Show in Explorer" } else { "Show in Folder" }).show(ui).clicked() {
                        act = Some(DoneAct::Reveal(last.paths.output.clone()));
                    }
                    if Button::secondary("Verify Again").show(ui).clicked() {
                        act = Some(DoneAct::Verify(dv.reports.iter().map(|r| r.paths.output.clone()).collect()));
                    }
                } else {
                    if matches!(last.outcome, Outcome::Stopped { resumable: true, .. }) && Button::primary("Resume").show(ui).clicked() {
                        act = Some(DoneAct::Resume(last.paths.partial.clone(), !last.delete_requested));
                    }
                    if Button::secondary("Show in Folder").show(ui).clicked() {
                        act = Some(DoneAct::Reveal(last.paths.partial.clone()));
                    }
                }
                if Button::secondary("New Archive").show(ui).on_hover_text(shortcut(ui, "Ctrl+N")).clicked() {
                    act = Some(DoneAct::New);
                }
            });
            let mut all: Vec<Row> = dv.issues.clone();
            for r in &dv.reports {
                all.extend(r.failed.iter().map(|(p, w)| Row { idx: 0, name: p.display().to_string(), status: EntryStatus::Failed(w.clone()) }));
                all.extend(r.skipped.iter().map(|(p, w)| Row { idx: 0, name: p.display().to_string(), status: EntryStatus::Skipped(w.clone()) }));
                all.extend(r.kept.iter().map(|(n, w)| Row { idx: 0, name: n.clone(), status: EntryStatus::Kept(w.clone()) }));
                all.extend(r.warnings.iter().map(|w| Row { idx: 0, name: w.clone(), status: EntryStatus::Waiting }));
                if let Some(v) = &r.verification {
                    all.extend(v.problems.iter().map(|pr| Row { idx: 0, name: pr.clone(), status: EntryStatus::Failed("verification".into()) }));
                }
            }
            if dv.reports.len() > 1 {
                design::separator(ui);
                design::text(ui, "Archives", typ::headline(), p.text);
                ui.add_space(space::S);
                let cols = [Col { title: "Name", width: 0.0, right: false }, Col { title: "Status", width: 150.0, right: false }, Col { title: "Size", width: 90.0, right: true }];
                design::table_frame(ui, |ui| {
                    let ranges = design::table_header(ui, &cols);
                    for (i, r) in dv.reports.iter().enumerate() {
                        let (c, st) = match &r.outcome {
                            Outcome::Completed => (p.green, "Done"),
                            Outcome::CompletedWithIssues => (p.orange, "Done, with notes"),
                            Outcome::Stopped { .. } => (p.red, "Paused"),
                        };
                        let path = if r.finished() { &r.paths.output } else { &r.paths.partial };
                        let (rect, _, fg, fg2) = design::table_row(ui, i, false, false);
                        let y = rect.center().y;
                        ui.painter().text(pos2(ranges[0].0, y), Align2::LEFT_CENTER, fmt::file_name(path), typ::body(), fg);
                        ui.painter().circle_filled(pos2(ranges[1].0 + 4.0, y), 3.5, c);
                        ui.painter().text(pos2(ranges[1].0 + 14.0, y), Align2::LEFT_CENTER, st, typ::body(), fg2);
                        ui.painter().text(pos2(ranges[2].1, y), Align2::RIGHT_CENTER, fmt::size(r.archive_size), typ::body(), fg2);
                    }
                });
            }
            if !all.is_empty() {
                ui.add_space(space::L);
                design::disclosure(ui, &format!("Notes ({})", all.len()), &mut dv.show_details);
                design::reveal(ui, Id::unique("done-notes"), dv.show_details, |ui| {
                    ui.add_space(space::S);
                    activity_list(ui, &all, 220.0, "");
                });
            }
            ui.add_space(space::XXL);
        });
    });
    if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N))) && !app.sheets.is_open()
    {
        act = Some(DoneAct::New);
    }
    match act {
        Some(DoneAct::Reveal(p)) => crate::worker::reveal(&p),
        Some(DoneAct::Verify(ps)) => {
            app.verify.start(&ctx, ps);
            app.navigate(Page::Verify);
        }
        Some(DoneAct::Resume(path, no_delete)) => app.compress.resume(&ctx, path, no_delete),
        Some(DoneAct::New) => app.compress.new_archive(),
        None => {}
    }
}
