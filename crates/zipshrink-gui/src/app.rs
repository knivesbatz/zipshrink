use crate::icons::{self, Art};
use crate::theme::{
    self, bevel, button, button_ex, checkbox, field, group, progress, Bevel, FACE, GRAY_TEXT, GREEN, NAVY, OLIVE, RED, TEXT, WINDOW,
};
use crate::worker::{self, Checked, Msg, Running, Start};
use eframe::egui::{self, pos2, vec2, Align, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zipshrink_core::engine::{Outcome, StopKind};
use zipshrink_core::progress::{Event, Phase, Stats};
use zipshrink_core::util::{human_bytes, parse_size};
use zipshrink_core::verify::ArchiveCheck;
use zipshrink_core::{EntryStatus, JobOptions, JobPaths, Report, Verdict};
use zipshrink_update::{Release, Settings};

const LEVELS: [(u32, &str); 5] = [(0, "Store (no compression)"), (1, "Fastest"), (3, "Fast"), (6, "Normal"), (9, "Maximum")];
const TITLE_H: f32 = 18.0;

#[derive(Clone)]
struct Row {
    idx: u64,
    name: String,
    size: u64,
    status: EntryStatus,
}

struct RunView {
    running: Running,
    stats: Stats,
    phase: Phase,
    part: (usize, usize, PathBuf),
    reports: Vec<Report>,
    recent: VecDeque<Row>,
    issues: Vec<Row>,
    stopping: bool,
    close_after: bool,
    show_issues: bool,
}

enum Screen {
    Setup,
    Run(Box<RunView>),
    Done { reports: Vec<Report>, issues: Vec<Row>, show_details: bool },
}

enum Modal {
    None,
    ConsentDelete { understood: bool },
    ConfirmOverwrite(Vec<PathBuf>),
    Resume(PathBuf),
    ConfirmClose,
    Message { title: String, icon: Art, text: String },
    About,
}

enum Update {
    Idle,
    Checking { rx: Receiver<Result<Option<Release>, String>>, manual: bool },
    Available(Release),
    Downloading { rx: Receiver<Result<PathBuf, String>>, done: Arc<AtomicU64>, total: Arc<AtomicU64>, version: String },
}

struct VerifyRun {
    rx: Option<Receiver<Vec<(PathBuf, Result<ArchiveCheck, String>)>>>,
    progress: Arc<AtomicU64>,
    total: u64,
    result: Option<Vec<(PathBuf, Result<ArchiveCheck, String>)>>,
}

enum Action {
    AddFiles,
    AddFolder,
    Remove,
    Clear,
    BrowseDest,
    Start,
    Create { overwrite: bool },
    Resume { path: PathBuf, no_delete: bool },
    AskResume,
    AskVerify,
    Verify(Vec<PathBuf>),
    Reveal(PathBuf),
    NewArchive,
    Pause,
    StopAndClose,
    Close,
    CheckUpdates,
    UpdateNow(Release),
    SkipVersion(String),
    OpenUrl(String),
    ToggleAutoUpdate,
}

pub struct App {
    screen: Screen,
    modal: Modal,
    sources: Vec<PathBuf>,
    selected: Option<usize>,
    dest_dir: Option<PathBuf>,
    file_name: String,
    name_touched: bool,
    level: usize,
    parts: usize,
    delete: bool,
    delete_readonly: bool,
    reserve_text: String,
    check_key: String,
    check_rx: Option<Receiver<Result<Checked, String>>>,
    checked: Option<Result<Checked, String>>,
    check_dirty_at: Option<Instant>,
    verify: Option<VerifyRun>,
    settings: Settings,
    update: Update,
    maximized: bool,
    acts: Vec<Action>,
    anim: Instant,
}

fn default_threads() -> u32 {
    std::thread::available_parallelism().map(|n| n.get().min(4) as u32).unwrap_or(1)
}

fn file_label(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

impl App {
    pub fn new(ctx: &egui::Context, sources: Vec<PathBuf>, resume: Option<PathBuf>) -> Self {
        let mut settings = Settings::load();
        let update = if settings.check_due() {
            settings.last_check = zipshrink_update::now();
            settings.save();
            Update::Checking { rx: worker::check_update(ctx.clone()), manual: false }
        } else {
            Update::Idle
        };
        let mut app = App {
            screen: Screen::Setup,
            modal: Modal::None,
            sources: Vec::new(),
            selected: None,
            dest_dir: None,
            file_name: String::new(),
            name_touched: false,
            level: 3,
            parts: 1,
            delete: false,
            delete_readonly: false,
            reserve_text: "256 MB".into(),
            check_key: String::new(),
            check_rx: None,
            checked: None,
            check_dirty_at: None,
            verify: None,
            settings,
            update,
            maximized: false,
            acts: Vec::new(),
            anim: Instant::now(),
        };
        app.add_sources(sources);
        if let Some(r) = resume {
            app.modal = Modal::Resume(r);
        }
        app
    }

    fn add_sources(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let p = std::path::absolute(&p).unwrap_or(p);
            let s = p.to_string_lossy();
            if s.ends_with(".partial") || s.ends_with(".partial.journal") {
                self.modal = Modal::Resume(p);
                continue;
            }
            if !self.sources.contains(&p) {
                self.sources.push(p);
            }
        }
        if self.dest_dir.is_none() {
            self.dest_dir = self.sources.first().and_then(|p| p.parent()).map(Path::to_path_buf);
        }
        if !self.name_touched {
            self.file_name = match self.sources.as_slice() {
                [] => String::new(),
                [one] => format!(
                    "{}.zip",
                    one.file_stem().or(one.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Archive".into())
                ),
                _ => "Archive.zip".into(),
            };
        }
    }

    fn output(&self) -> Option<PathBuf> {
        let name = self.file_name.trim();
        if name.is_empty() || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            return None;
        }
        let name = if name.to_ascii_lowercase().ends_with(".zip") { name.to_string() } else { format!("{name}.zip") };
        Some(self.dest_dir.as_ref()?.join(name))
    }

    fn reserve(&self) -> Result<u64, String> {
        parse_size(&self.reserve_text.to_ascii_uppercase().replace("MB", "M").replace("GB", "G").replace("KB", "K"))
    }

    fn options(&self) -> JobOptions {
        JobOptions {
            level: LEVELS[self.level].0,
            delete_originals: self.delete,
            delete_readonly: self.delete_readonly,
            min_free: self.reserve().unwrap_or(zipshrink_core::DEFAULT_MIN_FREE),
            threads: default_threads(),
            ..Default::default()
        }
    }

    fn busy(&self) -> bool {
        matches!(self.screen, Screen::Run(_))
    }

    /// Re-runs the preflight shortly after the inputs change.
    fn refresh_check(&mut self, ctx: &egui::Context) {
        let Some(out) = self.output() else {
            self.checked = None;
            return;
        };
        if self.sources.is_empty() {
            self.checked = None;
            self.check_key.clear();
            return;
        }
        let key = format!("{:?}|{}|{}|{}|{}|{}", self.sources, out.display(), self.level, self.delete, self.reserve_text, self.parts);
        if key != self.check_key {
            self.check_key = key;
            self.check_dirty_at = Some(Instant::now());
        }
        if let Some(t) = self.check_dirty_at {
            if t.elapsed() >= Duration::from_millis(300) && self.check_rx.is_none() {
                self.check_dirty_at = None;
                self.check_rx = Some(worker::preflight(ctx.clone(), self.sources.clone(), out, self.options(), self.parts));
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(rx) = &self.check_rx {
            if let Ok(r) = rx.try_recv() {
                self.check_rx = None;
                if self.check_dirty_at.is_none() {
                    self.checked = Some(r);
                }
            }
        }
    }

    fn checking(&self) -> bool {
        self.check_rx.is_some() || self.check_dirty_at.is_some()
    }
}

impl RunView {
    fn new(running: Running, delete: bool) -> Self {
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
            show_issues: false,
        }
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Phase(p) => self.phase = p,
            Event::Progress(s) => {
                self.phase = s.phase;
                self.stats = s;
            }
            Event::Message(m) => self.issues.push(Row { idx: u64::MAX, name: m, size: 0, status: EntryStatus::Waiting }),
            Event::Entry { idx, name, size, status } => {
                if matches!(status, EntryStatus::Failed(_) | EntryStatus::Kept(_) | EntryStatus::Skipped(_)) {
                    self.issues.push(Row { idx, name: name.clone(), size, status: status.clone() });
                }
                if let Some(r) = self.recent.iter_mut().rev().take(64).find(|r| r.idx == idx && r.name == name) {
                    r.status = status;
                } else {
                    self.recent.push_back(Row { idx, name, size, status });
                    while self.recent.len() > 500 {
                        self.recent.pop_front();
                    }
                }
            }
        }
    }
}

fn eta(s: &Stats) -> Option<Duration> {
    if s.speed <= 1.0 || s.total_bytes == 0 || s.phase != Phase::Compressing {
        return None;
    }
    Some(Duration::from_secs_f64(s.total_bytes.saturating_sub(s.bytes_processed) as f64 / s.speed))
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{} hours {} minutes", s / 3600, (s % 3600) / 60)
    } else if s >= 120 {
        format!("{} minutes", s / 60 + 1)
    } else if s >= 60 {
        "1 minute".into()
    } else {
        format!("{} seconds", s.max(1))
    }
}

/// Single-line text placed exactly on the pixel grid, so the bitmap font
/// is never resampled.
fn pixel_text(ui: &mut Ui, text: String, font: egui::FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let size = vec2(galley.size().x.ceil(), galley.size().y.ceil());
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().galley(rect.min.round(), galley, color);
}

fn label(ui: &mut Ui, text: impl Into<String>) {
    pixel_text(ui, text.into(), theme::regular(), TEXT);
}

fn colored(ui: &mut Ui, text: impl Into<String>, c: Color32) {
    pixel_text(ui, text.into(), theme::regular(), c);
}

fn bold(ui: &mut Ui, text: impl Into<String>) {
    pixel_text(ui, text.into(), theme::bold(), TEXT);
}

fn icon(ui: &mut Ui, art: Art, scale: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0 * scale), Sense::hover());
    icons::paint(ui.painter(), r.min, art, scale);
}

/// A sunken single-line text box.
fn text_box(ui: &mut Ui, text: &mut String, width: f32) -> egui::Response {
    field(ui, WINDOW, |ui| {
        ui.spacing_mut().interact_size.y = 15.0;
        ui.add(egui::TextEdit::singleline(text).frame(egui::Frame::NONE).desired_width(width).margin(vec2(1.0, 1.0)))
    })
}

/// A read-only sunken box showing `text`, cut at the left if too long.
fn read_box(ui: &mut Ui, text: &str, width: f32) {
    field(ui, WINDOW, |ui| {
        ui.set_width(width);
        ui.set_min_height(15.0);
        ui.add_space(1.0);
        ui.add(egui::Label::new(RichText::new(text).color(TEXT)).truncate());
    });
}

/// An up/down spin control.
fn spin(ui: &mut Ui, value: &mut usize, min: usize, max: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        field(ui, WINDOW, |ui| {
            ui.set_width(34.0);
            ui.set_min_height(15.0);
            ui.add_space(1.0);
            ui.label(RichText::new(value.to_string()).color(TEXT));
        });
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 21.0), Sense::hover());
        let up = Rect::from_min_size(r.min, vec2(16.0, 10.0));
        let down = Rect::from_min_size(r.min + vec2(0.0, 11.0), vec2(16.0, 10.0));
        for (i, (rect, delta)) in [(up, 1i64), (down, -1)].into_iter().enumerate() {
            let resp = ui.interact(rect, ui.scope_id().with(("spin", i)), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 0.0, FACE);
            bevel(p, rect, if resp.is_pointer_button_down_on() { Bevel::Pressed } else { Bevel::Raised });
            let c = rect.center();
            for k in 0..3 {
                let w = 1.0 + 2.0 * k as f32;
                let y = if delta > 0 { c.y - 2.0 + k as f32 } else { c.y + 1.0 - k as f32 };
                p.rect_filled(Rect::from_min_size(pos2((c.x - w / 2.0).round(), y.round()), vec2(w, 1.0)), 0.0, TEXT);
            }
            if resp.clicked() {
                let n = (*value as i64 + delta).clamp(min as i64, max as i64) as usize;
                if n != *value {
                    *value = n;
                    changed = true;
                }
            }
        }
    });
    changed
}

/// A list view: sunken white box with rows of (colour, columns).
fn list_view(
    ui: &mut Ui,
    id: &str,
    height: f32,
    widths: &[f32],
    rows: &[(Color32, Vec<String>)],
    selected: Option<usize>,
) -> Option<usize> {
    let mut clicked = None;
    field(ui, WINDOW, |ui| {
        ui.set_width(ui.available_width());
        ui.set_height(height);
        if rows.is_empty() {
            return;
        }
        let h = 16.0;
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show_rows(ui, h, rows.len(), |ui, range| {
            for i in range {
                let (color, cols) = &rows[i];
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                let sel = selected == Some(i);
                if sel {
                    ui.painter().rect_filled(rect, 0.0, NAVY);
                }
                let mut x = rect.left() + 2.0;
                for (c, text) in cols.iter().enumerate() {
                    let w = widths.get(c).copied().unwrap_or(rect.right() - x);
                    let clip = Rect::from_min_max(pos2(x, rect.top()), pos2((x + w - 4.0).min(rect.right()), rect.bottom()));
                    let col = if sel {
                        Color32::WHITE
                    } else if c == 0 {
                        *color
                    } else {
                        TEXT
                    };
                    ui.painter().with_clip_rect(clip).text(pos2(x, rect.top() + 1.0), egui::Align2::LEFT_TOP, text, theme::regular(), col);
                    x += w;
                }
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

fn status_rows(rows: &[Row]) -> Vec<(Color32, Vec<String>)> {
    rows.iter()
        .map(|r| {
            let mut name = r.name.clone();
            if let Some(d) = r.status.detail() {
                name.push_str(" - ");
                name.push_str(d);
            }
            let st = if r.idx == u64::MAX && r.status == EntryStatus::Waiting { "Note" } else { r.status.label() };
            (theme::status_color(&r.status), vec![st.to_string(), if r.size > 0 { human_bytes(r.size) } else { String::new() }, name])
        })
        .collect()
}

/// A Windows-style message box drawn as a modal window.
fn msgbox(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    icon_art: Option<Art>,
    width: f32,
    body: impl FnOnce(&mut Ui),
    buttons: &[&str],
) -> Option<usize> {
    let mut pressed = None;
    egui::Modal::new(egui::Id::unique(id))
        .backdrop_color(Color32::TRANSPARENT)
        .frame(egui::Frame::new().fill(FACE).inner_margin(egui::Margin::same(3)))
        .show(ctx, |ui| {
            ui.set_width(width);
            let (bar, _) = ui.allocate_exact_size(vec2(width, TITLE_H), Sense::hover());
            theme::title_strip(ui, bar, title, true);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if let Some(a) = icon_art {
                    icon(ui, a, 2.0);
                    ui.add_space(8.0);
                }
                ui.vertical(|ui| {
                    ui.set_width(width - if icon_art.is_some() { 64.0 } else { 24.0 });
                    body(ui);
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let total: f32 = buttons.len() as f32 * 81.0;
                ui.add_space(((width - total) / 2.0).max(0.0));
                for (i, b) in buttons.iter().enumerate() {
                    if button_ex(ui, b, true, i == 0, 75.0).clicked() {
                        pressed = Some(i);
                    }
                }
            });
            ui.add_space(4.0);
            let r = ui.min_rect().expand(3.0);
            bevel(&ui.painter().with_clip_rect(r), r, Bevel::Raised);
        });
    pressed
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::snap_scale(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !matches!(&self.screen, Screen::Run(rv) if rv.close_after) {
                self.modal = Modal::ConfirmClose;
            }
        }
        self.poll(&ctx);
        let full = ui.max_rect();
        egui::Panel::top("chrome")
            .frame(egui::Frame::new().fill(FACE).inner_margin(egui::Margin { left: 4, right: 4, top: 4, bottom: 0 }))
            .show(ui, |ui| {
                self.title_bar(ui, &ctx);
                self.menu_bar(ui);
            });
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(FACE).inner_margin(egui::Margin { left: 4, right: 4, top: 2, bottom: 4 }))
            .show(ui, |ui| self.status_bar(ui, &ctx));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(FACE).inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 4 }))
            .show(ui, |ui| match &self.screen {
                Screen::Setup => {
                    self.refresh_check(&ctx);
                    self.setup_screen(ui, &ctx);
                }
                Screen::Run(_) => self.run_screen(ui, &ctx),
                Screen::Done { .. } => self.done_screen(ui),
            });
        // The window's own 3D frame (the system title bar is replaced).
        bevel(&ui.painter().with_clip_rect(full), full, Bevel::Raised);
        self.modals(&ctx);
        self.verify_window(&ctx);
        self.update_window(&ctx);
        let acts = std::mem::take(&mut self.acts);
        for a in acts {
            self.act(a, &ctx);
        }
    }
}

impl App {
    fn poll(&mut self, ctx: &egui::Context) {
        let mut next = None;
        if let Screen::Run(rv) = &mut self.screen {
            while let Ok(m) = rv.running.rx.try_recv() {
                match m {
                    Msg::Part { number, of, output } => rv.part = (number, of, output),
                    Msg::Event(e) => rv.on_event(e),
                    Msg::PartDone(r) => rv.reports.push(r),
                    Msg::AllDone => {
                        if rv.close_after {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        next = Some(Screen::Done {
                            reports: std::mem::take(&mut rv.reports),
                            issues: std::mem::take(&mut rv.issues),
                            show_details: false,
                        });
                        break;
                    }
                    Msg::Failed(e) => {
                        self.modal =
                            Modal::Message { title: "ZipShrink".into(), icon: icons::ERROR, text: format!("{e}\n\nNothing was changed.") };
                        next = Some(Screen::Setup);
                        break;
                    }
                }
            }
        }
        if let Some(n) = next {
            self.screen = n;
        }
        if let Update::Checking { rx, manual } = &self.update {
            if let Ok(r) = rx.try_recv() {
                let manual = *manual;
                self.update = Update::Idle;
                match r {
                    Ok(Some(rel)) if manual || self.settings.skipped_version.as_deref() != Some(rel.version.as_str()) => {
                        self.update = Update::Available(rel)
                    }
                    Ok(_) if manual => {
                        self.modal = Modal::Message {
                            title: "Check for Updates".into(),
                            icon: icons::INFO,
                            text: format!("You have the latest version of ZipShrink ({}).", zipshrink_update::CURRENT),
                        }
                    }
                    Err(e) if manual => self.modal = Modal::Message { title: "Check for Updates".into(), icon: icons::WARNING, text: e },
                    _ => {}
                }
            }
        }
        if let Update::Downloading { rx, .. } = &self.update {
            if let Ok(r) = rx.try_recv() {
                self.update = Update::Idle;
                match r.and_then(|p| zipshrink_update::launch_installer(&p).map_err(|e| e.to_string())) {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => {
                        self.modal = Modal::Message {
                            title: "Update".into(),
                            icon: icons::ERROR,
                            text: format!("The update could not be installed: {e}"),
                        }
                    }
                }
            }
        }
    }

    fn title_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let (bar, resp) = ui.allocate_exact_size(vec2(ui.available_width(), TITLE_H), Sense::click_and_drag());
        let title = match &self.screen {
            Screen::Run(rv) if rv.stats.total_bytes > 0 => format!("{:.0}% - ZipShrink", rv.stats.percent()),
            _ => "ZipShrink".to_string(),
        };
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        theme::title_strip(ui, bar, &title, focused);
        if resp.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            self.maximized = !self.maximized;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.maximized));
        }
        let mut x = bar.right() - 2.0 - 16.0;
        let y = bar.top() + 2.0;
        if theme::caption_button(ui, Rect::from_min_size(pos2(x, y), vec2(16.0, 14.0)), b'x').clicked() {
            self.acts.push(Action::Close);
        }
        x -= 18.0;
        if theme::caption_button(ui, Rect::from_min_size(pos2(x, y), vec2(16.0, 14.0)), b'o').clicked() {
            self.maximized = !self.maximized;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.maximized));
        }
        x -= 16.0;
        if theme::caption_button(ui, Rect::from_min_size(pos2(x, y), vec2(16.0, 14.0)), b'_').clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        let idle = !self.busy();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.add_enabled(idle, egui::Button::new("Add Files...")).clicked() {
                    self.acts.push(Action::AddFiles);
                }
                if ui.add_enabled(idle, egui::Button::new("Add Folder...")).clicked() {
                    self.acts.push(Action::AddFolder);
                }
                ui.separator();
                if ui.add_enabled(idle, egui::Button::new("Resume Interrupted Job...")).clicked() {
                    self.acts.push(Action::AskResume);
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    self.acts.push(Action::Close);
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.add_enabled(idle, egui::Button::new("Verify Archive...")).clicked() {
                    self.acts.push(Action::AskVerify);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Check for Updates...").clicked() {
                    self.acts.push(Action::CheckUpdates);
                }
                let mut auto = self.settings.check_for_updates;
                if ui.checkbox(&mut auto, "Check for Updates Automatically").clicked() {
                    self.acts.push(Action::ToggleAutoUpdate);
                }
                ui.separator();
                if ui.button("About ZipShrink...").clicked() {
                    self.modal = Modal::About;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let text = match &self.screen {
            Screen::Setup if self.checking() => "Scanning the selection...".to_string(),
            Screen::Setup if self.sources.is_empty() => "Ready. Drag files and folders onto this window, or use Add Folder.".to_string(),
            Screen::Setup => "Ready".to_string(),
            Screen::Run(rv) => {
                if rv.stopping {
                    "Stopping at the next safe point...".into()
                } else {
                    rv.phase.label().to_string()
                }
            }
            Screen::Done { .. } => "Done".to_string(),
        };
        let free = self
            .dest_dir
            .as_ref()
            .and_then(|d| zipshrink_core::platform::available_space(d).ok())
            .map(|f| format!("{} free", human_bytes(f)))
            .unwrap_or_default();
        let ver = format!("Version {}", zipshrink_core::VERSION);
        let p = ui.painter();
        let w2 = 130.0;
        let w3 = 100.0;
        let r1 = Rect::from_min_max(bar.min, pos2(bar.right() - w2 - w3 - 22.0, bar.bottom()));
        let r2 = Rect::from_min_max(pos2(r1.right() + 2.0, bar.top()), pos2(r1.right() + 2.0 + w2, bar.bottom()));
        let r3 = Rect::from_min_max(pos2(r2.right() + 2.0, bar.top()), pos2(r2.right() + 2.0 + w3, bar.bottom()));
        for (r, t) in [(r1, text.as_str()), (r2, free.as_str()), (r3, ver.as_str())] {
            bevel(p, r, Bevel::Thin);
            p.with_clip_rect(r.shrink(2.0)).text(pos2(r.left() + 4.0, r.top() + 3.0), egui::Align2::LEFT_TOP, t, theme::regular(), TEXT);
        }
        // Size grip.
        let g = Rect::from_min_max(pos2(bar.right() - 16.0, bar.top() + 2.0), bar.max);
        for k in 0..3 {
            let o = 4.0 * k as f32;
            for d in 0..(10 - o as i32) {
                let d = d as f32;
                p.rect_filled(
                    Rect::from_min_size(pos2(g.right() - 3.0 - o - d, g.bottom() - 12.0 + o + d + o * 0.0), vec2(1.0, 1.0)),
                    0.0,
                    theme::SHADOW,
                );
                p.rect_filled(
                    Rect::from_min_size(pos2(g.right() - 2.0 - o - d, g.bottom() - 12.0 + o + d), vec2(1.0, 1.0)),
                    0.0,
                    theme::HI,
                );
            }
        }
        let resp = ui.interact(g, ui.scope_id().with("grip"), Sense::drag());
        if resp.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(egui::ResizeDirection::SouthEast));
        }
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
    }

    fn setup_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.add_sources(dropped);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            group(ui, "Files and folders to compress", |ui| {
                ui.horizontal(|ui| {
                    let list_w = ui.available_width() - 96.0;
                    ui.vertical(|ui| {
                        ui.set_width(list_w);
                        let rows: Vec<(Color32, Vec<String>)> = self
                            .sources
                            .iter()
                            .map(|p| (TEXT, vec![if p.is_dir() { "Folder".into() } else { "File".into() }, p.display().to_string()]))
                            .collect();
                        let top = ui.cursor().min;
                        if let Some(i) = list_view(ui, "sources", 96.0, &[52.0], &rows, self.selected) {
                            self.selected = Some(i);
                        }
                        if self.sources.is_empty() || hovering {
                            let r = Rect::from_min_size(top + vec2(3.0, 3.0), vec2(list_w - 6.0, 90.0));
                            let p = ui.painter();
                            if hovering {
                                theme::focus_rect(p, r.shrink(2.0));
                            }
                            icons::paint(p, pos2(r.center().x - 16.0, r.top() + 14.0), icons::FOLDER, 2.0);
                            p.text(pos2(r.center().x, r.top() + 54.0), egui::Align2::CENTER_TOP, "Drag files and folders here", theme::regular(), GRAY_TEXT);
                        }
                    });
                    ui.vertical(|ui| {
                        if button_ex(ui, "Add Files...", true, false, 86.0).clicked() {
                            self.acts.push(Action::AddFiles);
                        }
                        if button_ex(ui, "Add Folder...", true, false, 86.0).clicked() {
                            self.acts.push(Action::AddFolder);
                        }
                        if button_ex(ui, "Remove", self.selected.is_some(), false, 86.0).clicked() {
                            self.acts.push(Action::Remove);
                        }
                        if button_ex(ui, "Clear", !self.sources.is_empty(), false, 86.0).clicked() {
                            self.acts.push(Action::Clear);
                        }
                    });
                });
            });
            ui.add_space(4.0);
            group(ui, "Archive", |ui| {
                egui::Grid::new("dest").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    label(ui, "Save in:");
                    ui.horizontal(|ui| {
                        let d = self.dest_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(choose a folder)".into());
                        read_box(ui, &d, (ui.available_width() - 100.0).max(120.0));
                        if button(ui, "Browse...").clicked() {
                            self.acts.push(Action::BrowseDest);
                        }
                    });
                    ui.end_row();
                    label(ui, "File name:");
                    if text_box(ui, &mut self.file_name, (ui.available_width() - 100.0).max(120.0)).changed() {
                        self.name_touched = true;
                    }
                    ui.end_row();
                    label(ui, "Compression:");
                    egui::ComboBox::from_id_salt("level").selected_text(LEVELS[self.level].1).width(170.0).show_ui(ui, |ui| {
                        for (i, (_, name)) in LEVELS.iter().enumerate() {
                            ui.selectable_value(&mut self.level, i, *name);
                        }
                    });
                    ui.end_row();
                    label(ui, "Split into:");
                    ui.horizontal(|ui| {
                        spin(ui, &mut self.parts, 1, zipshrink_core::split::MAX_PARTS.min(99));
                        label(ui, if self.parts == 1 { "ZIP file" } else { "ZIP files, balanced by size (files are never cut in two)" });
                    });
                    ui.end_row();
                    label(ui, "Keep free:");
                    ui.horizontal(|ui| {
                        text_box(ui, &mut self.reserve_text, 70.0);
                        match self.reserve() {
                            Ok(_) => label(ui, "on the destination drive at all times"),
                            Err(e) => colored(ui, e, RED),
                        }
                    });
                    ui.end_row();
                });
            });
            ui.add_space(4.0);
            group(ui, "Original files", |ui| {
                let mut want = self.delete;
                if checkbox(ui, &mut want, "Delete original files after successful compression", true).changed() {
                    if want {
                        self.modal = Modal::ConsentDelete { understood: false };
                    } else {
                        self.delete = false;
                        self.delete_readonly = false;
                    }
                }
                if self.delete {
                    ui.horizontal(|ui| {
                        icon(ui, icons::WARNING, 1.0);
                        colored(ui, "Each original is PERMANENTLY deleted - not sent to the Recycle Bin - once its compressed copy is verified on disk.", RED);
                    });
                    checkbox(ui, &mut self.delete_readonly, "Also delete read-only files", true);
                } else {
                    colored(ui, "Your files are left untouched. Turn this on to free disk space while the archive is built.", GRAY_TEXT);
                }
            });
            ui.add_space(4.0);
            group(ui, "Disk space", |ui| self.space_panel(ui));
            ui.add_space(6.0);
            let ready = matches!(&self.checked, Some(Ok(c)) if c.preflight.verdict != Verdict::DoesNotFit) && !self.checking() && self.reserve().is_ok();
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if button_ex(ui, "Close", true, false, 75.0).clicked() {
                    self.acts.push(Action::Close);
                }
                let text = if self.delete { "Compress && Delete" } else { "Compress" };
                if button_ex(ui, &text.replace("&&", "&"), ready, true, 120.0).clicked() {
                    self.acts.push(Action::Start);
                }
            });
        });
    }

    fn space_panel(&mut self, ui: &mut Ui) {
        if self.sources.is_empty() {
            colored(ui, "Add files or folders to see how much space the archive needs.", GRAY_TEXT);
            return;
        }
        if self.output().is_none() {
            colored(ui, "Choose a folder and a valid file name.", OLIVE);
            return;
        }
        match &self.checked {
            None => label(ui, "Scanning the selection and sampling compression..."),
            Some(Err(e)) => {
                ui.horizontal(|ui| {
                    icon(ui, icons::ERROR, 1.0);
                    colored(ui, e, RED);
                });
            }
            Some(Ok(c)) => {
                let p = &c.preflight;
                egui::Grid::new("space").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                    label(ui, "Selected:");
                    label(ui, format!("{} in {} files and {} folders", human_bytes(p.bytes), p.files, p.dirs));
                    ui.end_row();
                    label(ui, "Estimated archive size:");
                    label(ui, format!("about {} (at most {})", human_bytes(p.estimated_archive), human_bytes(p.worst_case_archive)));
                    ui.end_row();
                    label(ui, "Space needed:");
                    label(
                        ui,
                        format!(
                            "about {}{}",
                            human_bytes(p.needed_estimate),
                            if p.delete_originals { ", as originals are deleted along the way" } else { "" }
                        ),
                    );
                    ui.end_row();
                    label(ui, "Free on destination:");
                    label(ui, format!("{} ({} kept free)", human_bytes(p.free_space), human_bytes(p.min_free)));
                    ui.end_row();
                });
                let usable = p.free_space.saturating_sub(p.min_free).max(1);
                progress(ui, (p.needed_estimate as f64 / usable as f64) as f32, ui.available_width());
                let (art, color) = match p.verdict {
                    Verdict::Fits => (icons::CHECK, GREEN),
                    Verdict::ProbablyFits => (icons::WARNING, OLIVE),
                    Verdict::DoesNotFit => (icons::ERROR, RED),
                };
                ui.horizontal(|ui| {
                    icon(ui, art, 1.0);
                    ui.add(egui::Label::new(RichText::new(&p.explanation).color(color)).wrap());
                });
                for w in &p.warnings {
                    ui.horizontal(|ui| {
                        icon(ui, icons::WARNING, 1.0);
                        ui.add(egui::Label::new(RichText::new(w).color(OLIVE)).wrap());
                    });
                }
                if !p.skipped.is_empty() {
                    ui.horizontal(|ui| {
                        icon(ui, icons::INFO, 1.0);
                        label(ui, format!("{} items will be skipped and left in place (links, special files).", p.skipped.len()));
                    });
                }
                if let Some(u) = &c.unfinished {
                    colored(ui, format!("An interrupted job exists for {}. Compress will offer to resume it.", file_label(u)), theme::BLUE);
                } else if !c.existing.is_empty() {
                    colored(ui, format!("{} already exists; you will be asked before it is replaced.", file_label(&c.existing[0])), OLIVE);
                }
                if !c.parts.is_empty() {
                    ui.add_space(4.0);
                    bold(ui, format!("The archive will be split into {} ZIP files:", c.parts.len()));
                    let rows: Vec<(Color32, Vec<String>)> = c
                        .parts
                        .iter()
                        .map(|pt| {
                            (
                                TEXT,
                                vec![
                                    file_label(&pt.output),
                                    pt.files.to_string(),
                                    human_bytes(pt.bytes),
                                    format!("about {}", human_bytes(pt.estimated)),
                                    human_bytes(pt.worst_case),
                                ],
                            )
                        })
                        .collect();
                    let mut all =
                        vec![(NAVY, vec!["Name".into(), "Files".into(), "Data".into(), "Estimated size".into(), "At most".into()])];
                    all.extend(rows);
                    let h = (all.len() as f32 * 16.0 + 8.0).min(140.0);
                    list_view(ui, "parts", h, &[190.0, 60.0, 90.0, 120.0], &all, None);
                }
            }
        }
    }

    fn run_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let anim_t = self.anim.elapsed().as_secs_f32();
        let Screen::Run(rv) = &mut self.screen else { return };
        let s = rv.stats.clone();
        if s.phase == Phase::Compressing && !rv.stopping {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        // The flying-page animation of the era's copy dialogs.
        let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        {
            let p = ui.painter();
            icons::paint(p, pos2(strip.left() + 20.0, strip.top() + 4.0), icons::FOLDER, 2.0);
            icons::paint(p, pos2(strip.left() + 260.0, strip.top() + 4.0), icons::APP, 2.0);
            if s.phase == Phase::Compressing && !rv.stopping {
                let f = (anim_t * 0.9).fract();
                let x = strip.left() + 52.0 + f * 196.0;
                let y = strip.top() + 14.0 - (f * std::f32::consts::PI).sin() * 12.0;
                icons::paint(p, pos2(x.round(), y.round()), icons::FILE, 1.0);
            }
        }
        let (n, of, out) = rv.part.clone();
        bold(ui, if rv.stopping { "Stopping safely...".to_string() } else { format!("{}...", rv.phase.label()) });
        if of > 1 {
            label(ui, format!("Archive {n} of {of}: {}", file_label(&out)));
        } else if !out.as_os_str().is_empty() {
            label(ui, format!("Archive: {}", out.display()));
        }
        ui.add_space(4.0);
        match &s.current {
            Some(c) => {
                ui.add(egui::Label::new(RichText::new(format!("File: {}", c.name)).color(TEXT)).truncate());
                let st = c.status.clone().unwrap_or(EntryStatus::Compressing);
                ui.horizontal(|ui| {
                    label(ui, "Status:");
                    colored(ui, st.label(), theme::status_color(&st));
                    label(ui, format!("  ({} of {})", human_bytes(c.done), human_bytes(c.size)));
                });
                progress(ui, if c.size > 0 { c.done as f32 / c.size as f32 } else { 1.0 }, ui.available_width());
            }
            None => {
                label(
                    ui,
                    match s.phase {
                        Phase::VerifyingArchive => {
                            format!("Reading back the finished archive: {} of {}", human_bytes(s.verify_done), human_bytes(s.verify_total))
                        }
                        Phase::Finalizing => "Writing the central directory...".into(),
                        Phase::Recovering => "Checking the interrupted job against its journal...".into(),
                        Phase::Cleanup => "Removing emptied folders...".into(),
                        _ => "Please wait...".into(),
                    },
                );
                label(ui, " ");
                progress(ui, (s.verify_done as f32 / s.verify_total.max(1) as f32).min(1.0), ui.available_width());
            }
        }
        ui.add_space(4.0);
        let mut overall = format!(
            "Overall: {:.0}%   ({} of {})",
            s.percent(),
            human_bytes(s.bytes_processed.min(s.total_bytes)),
            human_bytes(s.total_bytes)
        );
        if let Some(e) = eta(&s) {
            overall.push_str(&format!("   {} remaining", fmt_dur(e)));
        }
        label(ui, overall);
        progress(ui, (s.percent() / 100.0) as f32, ui.available_width());
        ui.add_space(6.0);
        group(ui, "Details", |ui| {
            egui::Grid::new("stats").num_columns(4).spacing([14.0, 2.0]).show(ui, |ui| {
                let row = |ui: &mut Ui, a: &str, b: String, c: &str, d: String| {
                    label(ui, a);
                    bold(ui, b);
                    label(ui, c);
                    bold(ui, d);
                    ui.end_row();
                };
                row(ui, "Original data:", human_bytes(s.total_bytes), "Archive size:", human_bytes(s.archive_size));
                row(
                    ui,
                    "Est. remaining:",
                    human_bytes(s.estimated_remaining_archive()),
                    "Free space:",
                    s.free_space.map(human_bytes).unwrap_or_else(|| "-".into()),
                );
                row(
                    ui,
                    "Space reclaimed:",
                    if s.delete_originals { human_bytes(s.reclaimed) } else { "(deletion off)".into() },
                    "Files done:",
                    format!("{} ({} remaining)", s.files_archived, s.remaining_files()),
                );
                row(
                    ui,
                    "Compression:",
                    s.ratio().map(|r| format!("{:.0}% of original", r * 100.0)).unwrap_or_else(|| "-".into()),
                    "Speed:",
                    if s.speed > 0.0 { format!("{}/s", human_bytes(s.speed as u64)) } else { "-".into() },
                );
                row(
                    ui,
                    "Originals deleted:",
                    if s.delete_originals { s.originals_deleted.to_string() } else { "-".into() },
                    "Kept / failed:",
                    format!("{} / {}", s.originals_kept, s.files_failed),
                );
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if button_ex(ui, "Activity", true, !rv.show_issues, 80.0).clicked() {
                rv.show_issues = false;
            }
            if button_ex(ui, &format!("Problems ({})", rv.issues.len()), true, rv.show_issues, 100.0).clicked() {
                rv.show_issues = true;
            }
        });
        let rows: Vec<Row> = if rv.show_issues { rv.issues.clone() } else { rv.recent.iter().rev().cloned().collect() };
        let h = (ui.available_height() - 34.0).max(60.0);
        list_view(ui, "activity", h, &[150.0, 80.0], &status_rows(&rows), None);
        ui.add_space(4.0);
        let stopping = rv.stopping;
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if button_ex(ui, "Pause", !stopping, false, 75.0)
                .on_hover_text("Stops at the next safe point. Everything archived so far is kept; resume later.")
                .clicked()
            {
                self.acts.push(Action::Pause);
            }
        });
    }

    fn done_screen(&mut self, ui: &mut Ui) {
        let Screen::Done { reports, issues, show_details } = &mut self.screen else { return };
        let last = reports.last().cloned();
        let Some(last) = last else { return };
        let finished_all = reports.iter().all(|r| r.finished());
        let issues_any = reports.iter().any(|r| r.outcome == Outcome::CompletedWithIssues);
        let (headline, art) = match &last.outcome {
            Outcome::Stopped { kind: StopKind::Cancelled, .. } => ("The job was paused.", icons::INFO),
            Outcome::Stopped { resumable: true, .. } => ("The job paused and needs your attention.", icons::WARNING),
            Outcome::Stopped { .. } => ("The job stopped.", icons::ERROR),
            _ if issues_any => ("Done. Some files were left out or kept.", icons::WARNING),
            _ if reports.len() > 1 => ("All archives were created successfully.", icons::INFO),
            _ => ("The archive was created successfully.", icons::INFO),
        };
        let tot = |f: fn(&Report) -> u64| reports.iter().map(f).sum::<u64>();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                icon(ui, art, 2.0);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    bold(ui, headline);
                });
            });
            if let Outcome::Stopped { error, resumable, .. } = &last.outcome {
                ui.add_space(4.0);
                field(ui, theme::TOOLTIP, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(error).color(TEXT)).wrap());
                    if *resumable {
                        ui.add(egui::Label::new(RichText::new(format!("{} entries are safely stored in {}. Nothing archived so far is lost. Click Resume to continue.", last.recoverable_entries, file_label(&last.paths.partial))).color(TEXT)).wrap());
                    } else {
                        ui.add(egui::Label::new(RichText::new("No further originals will be deleted. Keep the .partial and .journal files. From a command prompt, \"zipshrink status\" shows what they hold and \"zipshrink salvage\" rebuilds a readable archive from the intact entries.").color(TEXT)).wrap());
                    }
                });
            }
            ui.add_space(4.0);
            group(ui, "Summary", |ui| {
                egui::Grid::new("summary").num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
                    label(ui, "Archive location:");
                    label(ui, if reports.len() > 1 { format!("{} ({} files)", last.paths.output.parent().map(|p| p.display().to_string()).unwrap_or_default(), reports.len()) } else if last.finished() { last.paths.output.display().to_string() } else { last.paths.partial.display().to_string() });
                    ui.end_row();
                    label(ui, "Total original size:");
                    bold(ui, human_bytes(tot(|r| r.original_bytes)));
                    ui.end_row();
                    label(ui, if finished_all { "Final archive size:" } else { "Archive size so far:" });
                    let a = tot(|r| r.archive_size);
                    let o = tot(|r| r.original_bytes);
                    bold(ui, format!("{}{}", human_bytes(a), if o > 0 { format!("  ({:.1}% of original)", 100.0 * a as f64 / o as f64) } else { String::new() }));
                    ui.end_row();
                    label(ui, "Disk space reclaimed:");
                    bold(ui, if last.delete_requested { human_bytes(tot(|r| r.reclaimed)) } else { "none - originals kept".into() });
                    ui.end_row();
                    label(ui, "Files archived:");
                    label(ui, format!("{} files, {} folders", tot(|r| r.files_archived), tot(|r| r.dirs_archived)));
                    ui.end_row();
                    label(ui, "Originals deleted:");
                    label(ui, if last.delete_requested { format!("{} files, {} emptied folders removed", tot(|r| r.originals_deleted), tot(|r| r.dirs_removed)) } else { "0 (deletion was off)".into() });
                    ui.end_row();
                    label(ui, "Skipped or failed:");
                    let sk: usize = reports.iter().map(|r| r.skipped.len()).sum();
                    let fl: usize = reports.iter().map(|r| r.failed.len()).sum();
                    let kp: usize = reports.iter().map(|r| r.kept.len()).sum();
                    colored(ui, format!("{sk} skipped, {fl} failed, {kp} originals kept"), if sk + fl + kp > 0 { OLIVE } else { TEXT });
                    ui.end_row();
                    label(ui, "Verification:");
                    let checks: Vec<&ArchiveCheck> = reports.iter().filter_map(|r| r.verification.as_ref()).collect();
                    if checks.is_empty() {
                        colored(ui, "not run (the job did not finish)", GRAY_TEXT);
                    } else if checks.iter().all(|c| c.ok()) {
                        let n: u64 = checks.iter().map(|c| c.sha_checked + c.crc_only).sum();
                        colored(ui, format!("Passed - every entry read back; {n} files matched SHA-256 and CRC-32"), GREEN);
                    } else {
                        colored(ui, "FAILED - see details", RED);
                    }
                    ui.end_row();
                });
            });
            if reports.len() > 1 {
                ui.add_space(4.0);
                group(ui, "Archives", |ui| {
                    let rows: Vec<(Color32, Vec<String>)> = reports
                        .iter()
                        .map(|r| {
                            let (st, c) = match &r.outcome {
                                Outcome::Completed => ("Done", GREEN),
                                Outcome::CompletedWithIssues => ("Done, with notes", OLIVE),
                                Outcome::Stopped { .. } => ("Paused", RED),
                            };
                            (c, vec![st.into(), file_label(if r.finished() { &r.paths.output } else { &r.paths.partial }), human_bytes(r.archive_size), format!("{} files", r.files_archived)])
                        })
                        .collect();
                    list_view(ui, "parts_done", (rows.len() as f32 * 16.0 + 8.0).min(120.0), &[110.0, 220.0, 90.0], &rows, None);
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if finished_all {
                    if button_ex(ui, "Open Folder", true, true, 95.0).clicked() {
                        self.acts.push(Action::Reveal(last.paths.output.clone()));
                    }
                    if button_ex(ui, "Verify Again", true, false, 95.0).clicked() {
                        self.acts.push(Action::Verify(reports.iter().map(|r| r.paths.output.clone()).collect()));
                    }
                } else {
                    if matches!(last.outcome, Outcome::Stopped { resumable: true, .. }) && button_ex(ui, "Resume", true, true, 95.0).clicked() {
                        self.acts.push(Action::Resume { path: last.paths.partial.clone(), no_delete: !last.delete_requested });
                    }
                    if button_ex(ui, "Open Folder", true, false, 95.0).clicked() {
                        self.acts.push(Action::Reveal(last.paths.partial.clone()));
                    }
                }
                if button_ex(ui, "New Archive", true, false, 95.0).clicked() {
                    self.acts.push(Action::NewArchive);
                }
            });
            let mut all: Vec<Row> = issues.clone();
            for r in reports.iter() {
                all.extend(r.failed.iter().map(|(p, w)| Row { idx: u64::MAX - 1, name: p.display().to_string(), size: 0, status: EntryStatus::Failed(w.clone()) }));
                all.extend(r.skipped.iter().map(|(p, w)| Row { idx: u64::MAX - 1, name: p.display().to_string(), size: 0, status: EntryStatus::Skipped(w.clone()) }));
                all.extend(r.warnings.iter().map(|w| Row { idx: u64::MAX, name: w.clone(), size: 0, status: EntryStatus::Waiting }));
                if let Some(v) = &r.verification {
                    all.extend(v.problems.iter().map(|p| Row { idx: u64::MAX - 1, name: p.clone(), size: 0, status: EntryStatus::Failed("verification".into()) }));
                }
            }
            if !all.is_empty() {
                ui.add_space(6.0);
                checkbox(ui, show_details, &format!("Show details ({} notes)", all.len()), true);
                if *show_details {
                    list_view(ui, "details", 160.0, &[150.0, 80.0], &status_rows(&all), None);
                }
            }
        });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        let mut close = false;
        match &mut self.modal {
            Modal::None => {}
            Modal::ConsentDelete { understood } => {
                let mut u = *understood;
                let r = msgbox(
                    ctx,
                    "consent",
                    "Confirm File Delete",
                    Some(icons::WARNING),
                    470.0,
                    |ui| {
                        bold(ui, "Permanently delete the original files?");
                        ui.add(egui::Label::new(RichText::new("ZipShrink will delete each original permanently - it is NOT sent to the Recycle Bin - as soon as its compressed copy has been written, flushed to the disk, read back, decompressed, matched against the original's CRC-32 and SHA-256, and recorded in the recovery journal.").color(TEXT)).wrap());
                        ui.add(egui::Label::new(RichText::new("Files that change, cannot be verified or are read-only are kept. After a crash the job can be resumed, but the archive will then hold the only copy of the deleted files. Keep a backup of anything irreplaceable.").color(TEXT)).wrap());
                        ui.add_space(4.0);
                        checkbox(ui, &mut u, "I understand that my original files will be deleted", true);
                    },
                    &["Yes", "No"],
                );
                *understood = u;
                match r {
                    Some(0) if u => {
                        self.delete = true;
                        close = true;
                    }
                    Some(1) => close = true,
                    _ => {}
                }
            }
            Modal::ConfirmOverwrite(paths) => {
                let names: Vec<String> = paths.iter().map(|p| file_label(p)).collect();
                match msgbox(
                    ctx,
                    "overwrite",
                    "Confirm File Replace",
                    Some(icons::WARNING),
                    430.0,
                    |ui| {
                        label(ui, format!("This folder already contains {}.", names.join(", ")));
                        label(ui, "Would you like to replace the existing file?");
                        colored(ui, "The old file is kept until the new one is fully written and verified.", GRAY_TEXT);
                    },
                    &["Yes", "No"],
                ) {
                    Some(0) => {
                        self.acts.push(Action::Create { overwrite: true });
                        close = true;
                    }
                    Some(_) => close = true,
                    None => {}
                }
            }
            Modal::Resume(p) => {
                let paths = JobPaths::from_any(p);
                let targets = zipshrink_core::split::resume_targets(p);
                let deleting = zipshrink_core::journal::load(&JobPaths::for_output(&targets[0]).journal)
                    .ok()
                    .and_then(|l| match l.records.first() {
                        Some(zipshrink_core::journal::Record::Header(h)) => Some(h.options.delete_originals),
                        _ => None,
                    })
                    .unwrap_or(false);
                let buttons: &[&str] = if deleting { &["Resume", "No Delete", "Cancel"] } else { &["Resume", "Cancel"] };
                let r = msgbox(
                    ctx,
                    "resume",
                    "Resume Interrupted Job",
                    Some(icons::INFO),
                    450.0,
                    |ui| {
                        bold(
                            ui,
                            if targets.len() > 1 {
                                format!("Resume {} unfinished archives?", targets.len())
                            } else {
                                format!("Resume {}?", file_label(&paths.output))
                            },
                        );
                        ui.add(egui::Label::new(RichText::new("ZipShrink checks the partial archive against its journal, discards any half-written entry and continues where it stopped.").color(TEXT)).wrap());
                        if deleting {
                            ui.add(egui::Label::new(RichText::new("This job deletes originals. Files already archived but not yet deleted are verified again first. Choose No Delete to stop deleting.").color(RED)).wrap());
                        }
                    },
                    buttons,
                );
                match r {
                    Some(0) => {
                        self.acts.push(Action::Resume { path: p.clone(), no_delete: false });
                        close = true;
                    }
                    Some(1) if deleting => {
                        self.acts.push(Action::Resume { path: p.clone(), no_delete: true });
                        close = true;
                    }
                    Some(_) => close = true,
                    None => {}
                }
            }
            Modal::ConfirmClose => match msgbox(
                ctx,
                "close",
                "ZipShrink",
                Some(icons::WARNING),
                400.0,
                |ui| {
                    ui.add(egui::Label::new(RichText::new("A job is running. Stop it at the next safe point and close? Everything archived so far is kept, and you can resume later.").color(TEXT)).wrap());
                },
                &["Stop", "Cancel"],
            ) {
                Some(0) => {
                    self.acts.push(Action::StopAndClose);
                    close = true;
                }
                Some(_) => close = true,
                None => {}
            },
            Modal::Message { title, icon: art, text } => {
                let (t, a, x) = (title.clone(), *art, text.clone());
                if msgbox(
                    ctx,
                    "message",
                    &t,
                    Some(a),
                    420.0,
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(x).color(TEXT)).wrap());
                    },
                    &["OK"],
                )
                .is_some()
                {
                    close = true;
                }
            }
            Modal::About => {
                if msgbox(
                    ctx,
                    "about",
                    "About ZipShrink",
                    Some(icons::APP),
                    400.0,
                    |ui| {
                        bold(ui, "ZipShrink");
                        label(ui, format!("Version {}", zipshrink_core::VERSION));
                        label(ui, "Creates ZIP archives and reclaims disk space as it goes.");
                        label(ui, "Free software under the MIT License.");
                        colored(ui, "Pixel font derived from Liberation Sans (SIL OFL 1.1).", GRAY_TEXT);
                        colored(ui, format!("Updates from github.com/{}", zipshrink_update::REPO), GRAY_TEXT);
                    },
                    &["OK"],
                )
                .is_some()
                {
                    close = true;
                }
            }
        }
        if close {
            self.modal = Modal::None;
        }
    }

    fn verify_window(&mut self, ctx: &egui::Context) {
        let Some(v) = &mut self.verify else { return };
        if let Some(rx) = &v.rx {
            if let Ok(r) = rx.try_recv() {
                v.result = Some(r);
                v.rx = None;
            }
        }
        let done = v.progress.load(Ordering::Relaxed);
        let total = v.total;
        let result = v.result.clone();
        let r = msgbox(
            ctx,
            "verify",
            "Verify Archive",
            Some(match &result {
                None => icons::APP,
                Some(rs) if rs.iter().all(|(_, r)| matches!(r, Ok(c) if c.ok())) => icons::INFO,
                Some(_) => icons::ERROR,
            }),
            440.0,
            |ui| match &result {
                None => {
                    label(ui, format!("Decompressing every entry: {} of {}", human_bytes(done), human_bytes(total)));
                    progress(ui, if total > 0 { done as f32 / total as f32 } else { 0.0 }, ui.available_width());
                }
                Some(rs) => {
                    for (p, r) in rs {
                        match r {
                            Ok(c) if c.ok() => {
                                bold(ui, format!("{}: intact", file_label(p)));
                                label(
                                    ui,
                                    format!(
                                        "{} entries, {}; {} files matched SHA-256 and CRC-32{}",
                                        c.entries,
                                        human_bytes(c.bytes),
                                        c.sha_checked,
                                        if c.crc_only > 0 { format!(", {} CRC-32 only", c.crc_only) } else { String::new() }
                                    ),
                                );
                            }
                            Ok(c) => {
                                colored(ui, format!("{}: {} problems", file_label(p), c.problems.len()), RED);
                                for pr in c.problems.iter().take(8) {
                                    label(ui, pr);
                                }
                            }
                            Err(e) => colored(ui, format!("{}: {e}", file_label(p)), RED),
                        }
                    }
                }
            },
            &[if result.is_some() { "OK" } else { "Close" }],
        );
        if r.is_some() && result.is_some() {
            self.verify = None;
        }
    }

    fn update_window(&mut self, ctx: &egui::Context) {
        if self.busy() || !matches!(self.modal, Modal::None) {
            return; // never interrupt a job; ask again when idle
        }
        match &self.update {
            Update::Available(rel) => {
                let rel = rel.clone();
                let installer = zipshrink_update::method() == zipshrink_update::Method::Installer;
                let buttons: &[&str] = if installer { &["Update Now", "Later", "Skip"] } else { &["Download", "Later", "Skip"] };
                let r = msgbox(
                    ctx,
                    "update",
                    "Update Available",
                    Some(icons::INFO),
                    420.0,
                    |ui| {
                        bold(ui, format!("ZipShrink {} is available.", rel.version));
                        label(ui, format!("You have version {}.", zipshrink_update::CURRENT));
                        if installer {
                            ui.add(egui::Label::new(RichText::new("Update Now downloads the installer, checks its SHA-256 against the release, and starts it. ZipShrink closes so it can be replaced.").color(TEXT)).wrap());
                        } else {
                            label(ui, "Download opens the release page in your browser.");
                        }
                        if ui.link("What's new in this version").clicked() {
                            self.acts.push(Action::OpenUrl(rel.page.clone()));
                        }
                    },
                    buttons,
                );
                match r {
                    Some(0) if installer => self.acts.push(Action::UpdateNow(rel)),
                    Some(0) => {
                        self.acts.push(Action::OpenUrl(rel.page.clone()));
                        self.update = Update::Idle;
                    }
                    Some(1) => self.update = Update::Idle,
                    Some(_) => self.acts.push(Action::SkipVersion(rel.version.clone())),
                    None => {}
                }
            }
            Update::Downloading { done, total, version, .. } => {
                let (d, t, v) = (done.load(Ordering::Relaxed), total.load(Ordering::Relaxed), version.clone());
                msgbox(
                    ctx,
                    "download",
                    "Downloading Update",
                    Some(icons::APP),
                    400.0,
                    |ui| {
                        label(ui, format!("Downloading ZipShrink {v}..."));
                        progress(ui, if t > 0 { d as f32 / t as f32 } else { 0.0 }, ui.available_width());
                        label(ui, format!("{} of {}", human_bytes(d), if t > 0 { human_bytes(t) } else { "?".into() }));
                    },
                    &[],
                );
            }
            _ => {}
        }
    }

    fn act(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::AddFiles => {
                if let Some(files) = rfd::FileDialog::new().set_title("Add Files").pick_files() {
                    self.add_sources(files);
                }
            }
            Action::AddFolder => {
                if let Some(dirs) = rfd::FileDialog::new().set_title("Add Folder").pick_folders() {
                    self.add_sources(dirs);
                }
            }
            Action::Remove => {
                if let Some(i) = self.selected.take() {
                    if i < self.sources.len() {
                        self.sources.remove(i);
                    }
                }
            }
            Action::Clear => {
                self.sources.clear();
                self.selected = None;
            }
            Action::BrowseDest => {
                let mut dlg = rfd::FileDialog::new().set_title("Save the archive in");
                if let Some(d) = &self.dest_dir {
                    dlg = dlg.set_directory(d);
                }
                if let Some(d) = dlg.pick_folder() {
                    self.dest_dir = Some(d);
                }
            }
            Action::Start => {
                if let Some(Ok(c)) = &self.checked {
                    if let Some(u) = &c.unfinished {
                        self.modal = Modal::Resume(u.clone());
                    } else if !c.existing.is_empty() {
                        self.modal = Modal::ConfirmOverwrite(c.existing.clone());
                    } else {
                        self.act(Action::Create { overwrite: false }, ctx);
                    }
                }
            }
            Action::Create { overwrite } => {
                let Some(Ok(c)) = self.checked.take() else { return };
                let mut req = c.req;
                req.options = self.options();
                req.options.overwrite = overwrite;
                let delete = req.options.delete_originals;
                self.screen = Screen::Run(Box::new(RunView::new(
                    worker::run(ctx.clone(), Start::Create { req, scan: c.scan, parts: c.parts }),
                    delete,
                )));
                self.check_key.clear();
            }
            Action::Resume { path, no_delete } => {
                self.screen = Screen::Run(Box::new(RunView::new(worker::run(ctx.clone(), Start::Resume { path, no_delete }), !no_delete)));
            }
            Action::AskResume => {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Open an unfinished job")
                    .add_filter("Unfinished ZipShrink job", &["partial", "journal"])
                    .pick_file()
                {
                    self.modal = Modal::Resume(p);
                }
            }
            Action::AskVerify => {
                if let Some(ps) = rfd::FileDialog::new().set_title("Verify Archive").add_filter("ZIP archive", &["zip"]).pick_files() {
                    self.act(Action::Verify(ps), ctx);
                }
            }
            Action::Verify(paths) => {
                let progress = Arc::new(AtomicU64::new(0));
                let total = paths.iter().map(|p| zip_uncompressed_total(p)).sum();
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx2 = ctx.clone();
                let pr = progress.clone();
                std::thread::spawn(move || {
                    let mut out = Vec::new();
                    for p in paths {
                        let rx = worker::verify(ctx2.clone(), p.clone(), pr.clone());
                        out.push((p, rx.recv().unwrap_or_else(|_| Err("verification stopped".into()))));
                    }
                    let _ = tx.send(out);
                    ctx2.request_repaint();
                });
                self.verify = Some(VerifyRun { rx: Some(rx), progress, total, result: None });
            }
            Action::Reveal(p) => worker::reveal(&p),
            Action::NewArchive => {
                self.sources.clear();
                self.selected = None;
                self.name_touched = false;
                self.file_name.clear();
                self.delete = false;
                self.parts = 1;
                self.checked = None;
                self.check_key.clear();
                self.screen = Screen::Setup;
            }
            Action::Pause => {
                if let Screen::Run(rv) = &mut self.screen {
                    rv.stopping = true;
                    rv.running.cancel.store(true, Ordering::SeqCst);
                }
            }
            Action::StopAndClose => {
                if let Screen::Run(rv) = &mut self.screen {
                    rv.stopping = true;
                    rv.close_after = true;
                    rv.running.cancel.store(true, Ordering::SeqCst);
                }
            }
            Action::Close => {
                if self.busy() {
                    self.modal = Modal::ConfirmClose;
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::CheckUpdates => {
                self.settings.last_check = zipshrink_update::now();
                self.settings.save();
                self.update = Update::Checking { rx: worker::check_update(ctx.clone()), manual: true };
            }
            Action::UpdateNow(rel) => {
                let done = Arc::new(AtomicU64::new(0));
                let total = Arc::new(AtomicU64::new(0));
                let version = rel.version.clone();
                self.update = Update::Downloading {
                    rx: worker::download_update(ctx.clone(), rel, done.clone(), total.clone()),
                    done,
                    total,
                    version,
                };
            }
            Action::SkipVersion(v) => {
                self.settings.skipped_version = Some(v);
                self.settings.save();
                self.update = Update::Idle;
            }
            Action::OpenUrl(u) => zipshrink_update::open_url(&u),
            Action::ToggleAutoUpdate => {
                self.settings.check_for_updates = !self.settings.check_for_updates;
                self.settings.save();
            }
        }
    }
}

fn zip_uncompressed_total(path: &Path) -> u64 {
    std::fs::File::open(path)
        .ok()
        .and_then(|f| zipshrink_core::format::read_central_directory(&f).ok())
        .map(|cd| cd.iter().map(|e| e.uncompressed).sum())
        .unwrap_or(0)
}
