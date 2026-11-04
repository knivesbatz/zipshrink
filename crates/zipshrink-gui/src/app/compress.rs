//! The Compress page: choose items and options, see the estimate, start.
//! While a job runs or after it ends, the page shows `run.rs` instead.

use super::fmt;
use super::run::{DoneView, RunView};
use super::sheets::{Sheet, Sheets};
use super::toast::Toasts;
use super::{App, Page};
use crate::design::{self, icons, pal, space, typ, Button, Col, Icon, Tone};
use crate::worker::{self, Checked, Split, Start};
use eframe::egui::{self, pos2, vec2, Align, Align2, Id, Layout, Rect, Sense, Stroke, Ui, UiBuilder};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use zipshrink_core::{JobOptions, Verdict};

pub const LEVELS: [(u32, &str); 5] = [(0, "None (Store Only)"), (1, "Fastest"), (3, "Fast"), (6, "Normal"), (9, "Maximum")];
const SPLITS: [&str; 3] = ["Don't Split", "Into a Number of Files", "Into Files of a Size"];
const SIZE_PRESETS: [(&str, &str); 5] =
    [("Email attachment", "25 MB"), ("CD", "650 MB"), ("FAT32 drive limit", "4000 MB"), ("DVD", "4.37 GB"), ("Blu-ray", "23 GB")];

pub struct Source {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

pub enum Flow {
    Setup,
    Run(Box<RunView>),
    Done(Box<DoneView>),
}

pub struct Compress {
    pub sources: Vec<Source>,
    selected: Option<usize>,
    dest_dir: Option<PathBuf>,
    file_name: String,
    name_touched: bool,
    level: usize,
    split_mode: usize,
    parts: usize,
    size_text: String,
    pub delete: bool,
    pub delete_readonly: bool,
    reserve_text: String,
    show_parts: bool,
    check_key: String,
    check_rx: Option<Receiver<Result<Checked, String>>>,
    checked: Option<Result<Checked, String>>,
    check_dirty_at: Option<Instant>,
    pub flow: Flow,
    finished: bool,
}

impl Default for Compress {
    fn default() -> Self {
        Compress {
            sources: Vec::new(),
            selected: None,
            dest_dir: None,
            file_name: String::new(),
            name_touched: false,
            level: 3,
            split_mode: 0,
            parts: 2,
            size_text: "650 MB".into(),
            delete: false,
            delete_readonly: false,
            reserve_text: "256 MB".into(),
            show_parts: false,
            check_key: String::new(),
            check_rx: None,
            checked: None,
            check_dirty_at: None,
            flow: Flow::Setup,
            finished: false,
        }
    }
}

fn default_threads() -> u32 {
    std::thread::available_parallelism().map(|n| n.get().min(4) as u32).unwrap_or(1)
}

enum Act {
    AddFiles,
    AddFolder,
    Remove(usize),
    Clear,
    BrowseDest,
    Start,
    AskDelete,
    ShowResume,
}

impl Compress {
    pub fn running(&self) -> bool {
        matches!(self.flow, Flow::Run(_))
    }

    pub fn stage(&self) -> u64 {
        match self.flow {
            Flow::Setup => 0,
            Flow::Run(_) => 1,
            Flow::Done(_) => 2,
        }
    }

    pub fn percent(&self) -> Option<f64> {
        match &self.flow {
            Flow::Run(rv) if rv.stats.total_bytes > 0 => Some(rv.stats.percent()),
            _ => None,
        }
    }

    pub fn closing_after_stop(&self) -> bool {
        matches!(&self.flow, Flow::Run(rv) if rv.close_after)
    }

    /// True once after a job ends (so the Resume list can refresh).
    pub fn take_finished_outputs(&mut self) -> Option<()> {
        std::mem::take(&mut self.finished).then_some(())
    }

    pub fn add_sources(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let p = std::path::absolute(&p).unwrap_or(p);
            if self.sources.iter().any(|s| s.path == p) {
                continue;
            }
            let meta = std::fs::metadata(&p).ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let size = meta.filter(|m| m.is_file()).map(|m| m.len());
            self.sources.push(Source { path: p, is_dir, size });
        }
        if self.dest_dir.is_none() {
            self.dest_dir = self.sources.first().and_then(|s| s.path.parent()).map(Path::to_path_buf);
        }
        if !self.name_touched {
            self.file_name = match self.sources.as_slice() {
                [] => String::new(),
                [one] => one
                    .path
                    .file_stem()
                    .or(one.path.file_name())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Archive".into()),
                _ => "Archive".into(),
            };
        }
    }

    pub fn pick_files(&mut self) {
        if let Some(files) = rfd::FileDialog::new().set_title("Add Files").pick_files() {
            self.add_sources(files);
        }
    }

    pub fn pick_folder(&mut self) {
        if let Some(dirs) = rfd::FileDialog::new().set_title("Add Folders").pick_folders() {
            self.add_sources(dirs);
        }
    }

    fn output(&self) -> Result<PathBuf, &'static str> {
        let name = self.file_name.trim();
        if name.is_empty() {
            return Err("Give the archive a name.");
        }
        if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            return Err("A name can't contain / \\ : * ? \" < > |");
        }
        let name = if name.to_ascii_lowercase().ends_with(".zip") { name.to_string() } else { format!("{name}.zip") };
        Ok(self.dest_dir.as_ref().ok_or("Choose where to save the archive.")?.join(name))
    }

    fn reserve(&self) -> Result<u64, String> {
        fmt::parse_size(&self.reserve_text)
    }

    fn split(&self) -> Result<Split, String> {
        match self.split_mode {
            1 => Ok(Split::Count(self.parts)),
            2 => {
                let s = fmt::parse_size(&self.size_text)?;
                if s < zipshrink_core::split::MIN_PART_SIZE {
                    return Err("Each file must be at least 64 KB.".into());
                }
                Ok(Split::Size(s))
            }
            _ => Ok(Split::None),
        }
    }

    pub fn options(&self) -> JobOptions {
        JobOptions {
            level: LEVELS[self.level].0,
            delete_originals: self.delete,
            delete_readonly: self.delete && self.delete_readonly,
            min_free: self.reserve().unwrap_or(zipshrink_core::DEFAULT_MIN_FREE),
            threads: default_threads(),
            ..Default::default()
        }
    }

    /// What blocks starting, if anything (shown in the footer).
    fn blocker(&self) -> Option<String> {
        if self.sources.is_empty() {
            return Some("Add files or folders to begin.".into());
        }
        if let Err(e) = self.output() {
            return Some(e.into());
        }
        if let Err(e) = self.reserve() {
            return Some(format!("Free space to keep: {e}"));
        }
        if let Err(e) = self.split() {
            return Some(e);
        }
        None
    }

    /// Re-runs the scan and estimate shortly after the inputs change.
    fn refresh_check(&mut self, ctx: &egui::Context) {
        if self.blocker().is_some() {
            self.checked = None;
            self.check_key.clear();
            self.check_rx = None;
            return;
        }
        let (Ok(out), Ok(split)) = (self.output(), self.split()) else { return };
        let paths: Vec<&PathBuf> = self.sources.iter().map(|s| &s.path).collect();
        let key = format!("{paths:?}|{}|{}|{}|{}|{split:?}", out.display(), self.level, self.delete, self.reserve_text);
        if key != self.check_key {
            self.check_key = key;
            self.check_dirty_at = Some(Instant::now());
        }
        if let Some(t) = self.check_dirty_at {
            if t.elapsed() >= Duration::from_millis(350) && self.check_rx.is_none() {
                self.check_dirty_at = None;
                let sources = self.sources.iter().map(|s| s.path.clone()).collect();
                self.check_rx = Some(worker::preflight(ctx.clone(), sources, out, self.options(), split));
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

    fn ready(&self) -> bool {
        self.blocker().is_none() && !self.checking() && matches!(&self.checked, Some(Ok(c)) if c.preflight.verdict != Verdict::DoesNotFit)
    }

    /// Starts the job (after any sheet confirmed it).
    pub fn create(&mut self, ctx: &egui::Context, overwrite: bool) {
        let Some(Ok(c)) = self.checked.take() else { return };
        let mut req = c.req;
        req.options = self.options();
        req.options.overwrite = overwrite;
        let delete = req.options.delete_originals;
        let outputs: Vec<PathBuf> =
            if c.parts.is_empty() { vec![req.output.clone()] } else { c.parts.iter().map(|p| p.output.clone()).collect() };
        super::jobs::remember(outputs);
        self.flow =
            Flow::Run(Box::new(RunView::new(worker::run(ctx.clone(), Start::Create { req, scan: c.scan, parts: c.parts }), delete)));
        self.check_key.clear();
    }

    pub fn resume(&mut self, ctx: &egui::Context, path: PathBuf, no_delete: bool) {
        let targets = zipshrink_core::split::resume_targets(&path);
        super::jobs::remember(targets);
        self.flow = Flow::Run(Box::new(RunView::new(worker::run(ctx.clone(), Start::Resume { path, no_delete }), !no_delete)));
    }

    pub fn new_archive(&mut self) {
        *self = Compress { level: self.level, dest_dir: None, ..Default::default() };
    }

    pub fn poll(&mut self, ctx: &egui::Context, sheets: &mut Sheets, toasts: &mut Toasts) {
        if let Flow::Run(rv) = &mut self.flow {
            match rv.poll(ctx) {
                super::run::Polled::Running => {}
                super::run::Polled::Done(done) => {
                    self.flow = Flow::Done(done);
                    self.finished = true;
                    if let Flow::Done(d) = &self.flow {
                        toasts.finished(d);
                    }
                }
                super::run::Polled::Failed(e) => {
                    self.flow = Flow::Setup;
                    self.finished = true;
                    sheets.open(Sheet::Message {
                        tone: Tone::Error,
                        title: "The job couldn't start".into(),
                        text: format!("{e}\n\nNothing was changed."),
                    });
                }
            }
        }
    }

    pub fn start(&mut self, ctx: &egui::Context, sheets: &mut Sheets) {
        let Some(Ok(c)) = &self.checked else { return };
        if let Some(u) = &c.unfinished {
            let targets = zipshrink_core::split::resume_targets(u);
            sheets.open(Sheet::Resume { path: u.clone(), parts: targets.len(), deleting: super::jobs::deletes_originals(u) });
        } else if !c.existing.is_empty() {
            sheets.open(Sheet::ConfirmOverwrite(c.existing.clone()));
        } else {
            self.create(ctx, false);
        }
    }
}

// ================================================================ page

pub fn page(app: &mut App, ui: &mut Ui) {
    match &app.compress.flow {
        Flow::Setup => setup(app, ui),
        Flow::Run(_) => super::run::running_page(app, ui),
        Flow::Done(_) => super::run::done_page(app, ui),
    }
}

/// Centres page content at a comfortable width, like a preferences pane.
pub fn column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_rect_before_wrap();
    let w = (avail.width() - 2.0 * space::XL).clamp(300.0, space::CONTENT_MAX);
    let x = avail.left() + ((avail.width() - w) / 2.0).max(space::L);
    let r = Rect::from_min_size(pos2(x, avail.top()), vec2(w, avail.height()));
    let mut child = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Min)));
    let out = add(&mut child);
    let used = child.min_rect();
    ui.allocate_rect(Rect::from_min_max(avail.min, pos2(avail.right(), used.bottom())), Sense::hover());
    out
}

pub fn shortcut(ui: &Ui, s: &str) -> String {
    if ui.ctx().os() == egui::os::OperatingSystem::Mac {
        s.replace("Ctrl+Shift+", "⇧⌘").replace("Ctrl+", "⌘")
    } else {
        s.to_string()
    }
}

fn setup(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    app.compress.refresh_check(&ctx);
    let mut acts: Vec<Act> = Vec::new();
    let full = ui.max_rect();
    let footer_h = if app.compress.sources.is_empty() { 0.0 } else { 56.0 };
    let body = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - footer_h));
    let foot = Rect::from_min_max(pos2(full.left(), body.bottom()), full.max);
    let unfinished = app.resume.count();

    let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("setup-body"));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut body_ui, |ui| {
        ui.add_space(space::XL);
        column(ui, |ui| {
            let c = &mut app.compress;
            if unfinished > 0 {
                ui.horizontal(|ui| {
                    let t = if unfinished == 1 {
                        "An interrupted job can be resumed.".to_string()
                    } else {
                        format!("{unfinished} interrupted jobs can be resumed.")
                    };
                    design::notice(ui, Tone::Info, &t, None);
                    if design::link(ui, "Show Interrupted Jobs").clicked() {
                        acts.push(Act::ShowResume);
                    }
                });
                ui.add_space(space::L);
            }
            if c.sources.is_empty() {
                drop_zone(ui, &mut acts);
            } else {
                items(ui, c, &mut acts);
                ui.add_space(space::XL);
                options(ui, c, &mut acts);
                design::separator(ui);
                estimate(ui, c);
            }
            ui.add_space(space::XL);
        });
    });
    if footer_h > 0.0 {
        footer(ui, foot, &mut app.compress, &mut acts);
    }

    for a in acts {
        let c = &mut app.compress;
        match a {
            Act::AddFiles => c.pick_files(),
            Act::AddFolder => c.pick_folder(),
            Act::Remove(i) => {
                if i < c.sources.len() {
                    c.sources.remove(i);
                    c.selected = None;
                }
            }
            Act::Clear => {
                c.sources.clear();
                c.selected = None;
                if !c.name_touched {
                    c.file_name.clear();
                }
            }
            Act::BrowseDest => {
                let mut dlg = rfd::FileDialog::new().set_title("Choose Where to Save the Archive");
                if let Some(d) = &c.dest_dir {
                    dlg = dlg.set_directory(d);
                }
                if let Some(d) = dlg.pick_folder() {
                    c.dest_dir = Some(d);
                }
            }
            Act::Start => c.start(&ctx, &mut app.sheets),
            Act::AskDelete => app.sheets.open(Sheet::ConsentDelete { understood: false }),
            Act::ShowResume => app.navigate(Page::Resume),
        }
    }
    // Delete removes the selected item (when no text field has focus).
    if !ctx.egui_wants_keyboard_input() && !app.sheets.is_open() {
        if let Some(i) = app.compress.selected {
            if ctx.input(|inp| inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace))
                && i < app.compress.sources.len()
            {
                app.compress.sources.remove(i);
                app.compress.selected = None;
            }
        }
    }
    let go = ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter)));
    if go && app.compress.ready() && !app.sheets.is_open() {
        app.compress.start(&ctx, &mut app.sheets);
    }
}

fn drop_zone(ui: &mut Ui, acts: &mut Vec<Act>) {
    let p = pal(ui.ctx());
    let h = (ui.available_height() - space::XL).clamp(280.0, 420.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    super::dashed_rect(ui.painter(), rect, 12.0, Stroke::new(1.5, p.outline));
    let mut inner = ui.new_child(UiBuilder::new().max_rect(rect.shrink(space::XL)).layout(Layout::top_down(Align::Center)));
    inner.add_space((h - 260.0).max(0.0) / 2.0 + 20.0);
    design::empty_state(
        &mut inner,
        Icon::Archive,
        "Drop Files and Folders Here",
        "They're compressed into one ZIP archive. Nothing changes on your disk until you click Compress.",
        |ui| {
            design::centered_row(ui, 236.0, |ui| {
                if Button::secondary("Add Files…").show(ui).on_hover_text(shortcut(ui, "Ctrl+O")).clicked() {
                    acts.push(Act::AddFiles);
                }
                if Button::secondary("Add Folder…").show(ui).on_hover_text(shortcut(ui, "Ctrl+Shift+O")).clicked() {
                    acts.push(Act::AddFolder);
                }
            });
        },
    );
}

fn items(ui: &mut Ui, c: &mut Compress, acts: &mut Vec<Act>) {
    let p = pal(ui.ctx());
    let n = c.sources.len();
    let cols = [
        Col { title: "Name", width: 0.0, right: false },
        Col { title: "Kind", width: 90.0, right: false },
        Col { title: "Size", width: 90.0, right: true },
    ];
    design::table_frame(ui, |ui| {
        let ranges = design::table_header(ui, &cols);
        let mut rows = |ui: &mut Ui| {
            for i in 0..n {
                let s = &c.sources[i];
                let selected = c.selected == Some(i);
                let (rect, resp, fg, fg2) = design::table_row(ui, i, selected, true);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Row, true, selected, fmt::file_name(&s.path)));
                if resp.clicked() {
                    c.selected = if selected { None } else { Some(i) };
                }
                let resp = resp.on_hover_text(s.path.display().to_string());
                resp.context_menu(|ui| {
                    if design::menu_item(ui, "Remove from List", false).clicked() {
                        acts.push(Act::Remove(i));
                        ui.close();
                    }
                    if design::menu_item(
                        ui,
                        if cfg!(windows) {
                            "Show in Explorer"
                        } else if cfg!(target_os = "macos") {
                            "Show in Finder"
                        } else {
                            "Show in Folder"
                        },
                        false,
                    )
                    .clicked()
                    {
                        crate::worker::reveal(&s.path);
                        ui.close();
                    }
                });
                let (l, r) = ranges[0];
                let ic = Rect::from_center_size(pos2(l + 8.0, rect.center().y), vec2(16.0, 16.0));
                let icon_color = if selected {
                    fg
                } else if s.is_dir {
                    p.accent
                } else {
                    p.text_secondary
                };
                icons::paint(ui.painter(), ic, if s.is_dir { Icon::Folder } else { Icon::Doc }, icon_color);
                let name = design::elide_end(ui, &fmt::file_name(&s.path), typ::body(), r - ic.right() - 8.0);
                ui.painter().text(pos2(ic.right() + 7.0, rect.center().y), Align2::LEFT_CENTER, name, typ::body(), fg);
                let kind = if s.is_dir { "Folder" } else { "Document" };
                ui.painter().text(pos2(ranges[1].0, rect.center().y), Align2::LEFT_CENTER, kind, typ::body(), fg2);
                let size = match (s.is_dir, s.size) {
                    (true, _) => "—".to_string(),
                    (false, Some(n)) => fmt::size(n),
                    (false, None) => "Missing".to_string(),
                };
                ui.painter().text(pos2(ranges[2].1, rect.center().y), Align2::RIGHT_CENTER, size, typ::body(), fg2);
            }
            // Keep a few empty striped rows so the table reads as a list.
            for i in n..4 {
                design::table_row(ui, i, false, false);
            }
        };
        if n > 7 {
            egui::ScrollArea::vertical().max_height(design::ROW_H * 7.0).id_salt("items").show(ui, rows);
        } else {
            rows(ui);
        }
    });
    // The +/- buttons under the table, and a count.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if square_button(ui, "+", "Add Files…").clicked() {
            acts.push(Act::AddFiles);
        }
        let can_remove = c.selected.is_some();
        let r = ui.add_enabled_ui(can_remove, |ui| square_button(ui, "−", "Remove the Selected Item")).inner;
        if r.clicked() {
            if let Some(i) = c.selected {
                acts.push(Act::Remove(i));
            }
        }
        ui.add_space(space::M);
        let files: u64 = c.sources.iter().filter_map(|s| s.size).sum();
        let dirs = c.sources.iter().filter(|s| s.is_dir).count() as u64;
        let mut t = fmt::count(n as u64, "item", "items");
        if dirs > 0 {
            t.push_str(&format!(" · {}", fmt::count(dirs, "folder", "folders")));
        }
        if files > 0 {
            t.push_str(&format!(" · {} in files", fmt::size(files)));
        }
        design::text(ui, t, typ::callout(), p.text_secondary);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if design::link(ui, "Remove All").clicked() {
                acts.push(Act::Clear);
            }
        });
    });
}

/// The small square add/remove buttons under an AppKit table.
fn square_button(ui: &mut Ui, glyph: &str, label: &str) -> egui::Response {
    let p = pal(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let fill = if resp.is_pointer_button_down_on() { p.control_pressed } else { p.control };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, p.outline.gamma_multiply(0.8)), egui::StrokeKind::Inside);
    let c = if ui.is_enabled() { p.text } else { p.text_tertiary };
    ui.painter().text(rect.center() - vec2(0.0, 1.0), Align2::CENTER_CENTER, glyph, typ::body_medium(), c);
    if resp.has_focus() {
        design::focus_ring(ui, rect, 2.0);
    }
    resp.on_hover_text(label)
}

fn options(ui: &mut Ui, c: &mut Compress, acts: &mut Vec<Act>) {
    let p = pal(ui.ctx());
    design::form_row(ui, "Save As:", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if design::text_field(ui, &mut c.file_name, "Archive", 240.0, "Archive name").changed() {
                c.name_touched = true;
            }
            design::set_invalid(ui, "Archive name", c.output().is_err() && c.dest_dir.is_some());
            design::text(ui, ".zip", typ::body(), p.text_secondary);
        });
    });
    ui.add_space(6.0);
    design::form_row(ui, "Where:", |ui| {
        let current = c.dest_dir.as_ref().map(|d| fmt::file_name(d)).unwrap_or_else(|| "Choose a Folder".into());
        let items = [current.as_str(), "", "Other…"];
        let mut sel = 0;
        let r = design::popup_button_ex(ui, "Where", &mut sel, &items, 240.0, Some(Icon::Folder));
        if r.changed() && sel == 2 {
            acts.push(Act::BrowseDest);
        }
        if let Some(d) = &c.dest_dir {
            let shown = design::elide_start(ui, &d.display().to_string(), typ::caption(), ui.available_width().min(420.0));
            design::text(ui, shown, typ::caption(), p.text_secondary);
        }
    });
    ui.add_space(space::L);
    let level_names: Vec<&str> = LEVELS.iter().map(|l| l.1).collect();
    design::form_row(ui, "Compression:", |ui| {
        design::popup_button(ui, "Compression", &mut c.level, &level_names, 190.0);
        design::help(ui, "Higher levels make smaller archives but take longer.");
    });
    ui.add_space(6.0);
    design::form_row(ui, "Split:", |ui| {
        design::popup_button(ui, "Split", &mut c.split_mode, &SPLITS, 190.0);
        let id = ui.scope_id().with("split-detail");
        let mode = c.split_mode;
        design::reveal(ui, id.with(1), mode == 1, |ui| {
            ui.horizontal(|ui| {
                design::stepper(ui, &mut c.parts, 2, zipshrink_core::split::MAX_PARTS, "files");
                design::text(ui, "ZIP files, balanced by size", typ::body(), p.text);
            });
        });
        design::reveal(ui, id.with(2), mode == 2, |ui| {
            ui.horizontal(|ui| {
                design::text_field(ui, &mut c.size_text, "650 MB", 96.0, "Part size");
                design::set_invalid(ui, "Part size", c.split().is_err());
                design::text(ui, "each", typ::body(), p.text);
                let mut names: Vec<String> = vec!["Common Sizes".into(), String::new()];
                names.extend(SIZE_PRESETS.iter().map(|(n, s)| format!("{n} — {s}")));
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let mut sel = 0;
                if design::popup_button(ui, "Common sizes", &mut sel, &refs, 140.0).changed() && sel >= 2 {
                    c.size_text = SIZE_PRESETS[sel - 2].1.into();
                }
            });
        });
        design::help(ui, "Each part is a complete ZIP file that opens on its own. A file is never cut in two.");
    });
    ui.add_space(space::L);
    design::form_row(ui, "Originals:", |ui| {
        let mut want = c.delete;
        if design::checkbox(ui, &mut want, "Delete originals after verifying them").changed() {
            if want {
                acts.push(Act::AskDelete);
            } else {
                c.delete = false;
                c.delete_readonly = false;
            }
        }
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                design::help(
                    ui,
                    if cfg!(target_os = "macos") {
                        "Frees space as the archive is built. Each file is deleted permanently, not moved to the Trash, once its compressed copy has been read back and checked."
                    } else {
                        "Frees space as the archive is built. Each file is deleted permanently, not moved to the Recycle Bin, once its compressed copy has been read back and checked."
                    },
                );
                ui.add_space(2.0);
                ui.add_enabled_ui(c.delete, |ui| {
                    design::checkbox(ui, &mut c.delete_readonly, "Include read-only files");
                });
            });
        });
    });
    ui.add_space(space::L);
    design::form_row(ui, "Keep Free:", |ui| {
        ui.horizontal(|ui| {
            design::text_field(ui, &mut c.reserve_text, "256 MB", 96.0, "Free space to keep");
            design::set_invalid(ui, "Free space to keep", c.reserve().is_err());
            design::text(ui, "on the destination drive", typ::body(), p.text);
        });
        design::help(ui, "ZipShrink pauses safely rather than fill the drive below this.");
    });
}

fn estimate(ui: &mut Ui, c: &mut Compress) {
    let p = pal(ui.ctx());
    let checking = c.checking();
    let split = c.split();
    let blocker = c.blocker();
    let mut show_parts = c.show_parts;
    ui.horizontal(|ui| {
        design::text(ui, "Estimate", typ::headline(), p.text);
        if checking || (c.checked.is_none() && blocker.is_none()) {
            design::spinner(ui, 14.0);
        }
    });
    ui.add_space(space::S);
    match &c.checked {
        _ if blocker.is_some() => {
            design::secondary(ui, blocker.clone().unwrap_or_default());
        }
        None => {
            design::secondary(ui, "Scanning the selection and sampling how well it compresses…");
        }
        Some(Err(e)) => design::notice(ui, Tone::Error, "The selection can't be compressed.", Some(e)),
        Some(Ok(ch)) => {
            let pf = &ch.preflight;
            ui.scope(|ui| {
                if checking {
                    ui.multiply_opacity(0.55);
                }
                let usable = pf.free_space.saturating_sub(pf.min_free);
                let need_color = match pf.verdict {
                    Verdict::Fits => p.accent,
                    Verdict::ProbablyFits => p.orange,
                    Verdict::DoesNotFit => p.red,
                };
                design::capacity_bar(
                    ui,
                    Id::unique("est-bar"),
                    pf.free_space.max(1),
                    &[(need_color, pf.needed_estimate.min(usable.max(1))), (p.text_tertiary.gamma_multiply(0.6), pf.min_free)],
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    design::legend(ui, need_color, &format!("This job needs ≈ {}", fmt::size(pf.needed_estimate)));
                    design::legend(ui, p.text_tertiary.gamma_multiply(0.6), &format!("Kept free {}", fmt::size(pf.min_free)));
                    design::text(ui, format!("{} free on the drive", fmt::size(pf.free_space)), typ::callout(), p.text_secondary);
                });
                ui.add_space(space::M);
                design::key_value(
                    ui,
                    "Selected:",
                    format!(
                        "{} in {} and {}",
                        fmt::size(pf.bytes),
                        fmt::count(pf.files, "file", "files"),
                        fmt::count(pf.dirs, "folder", "folders")
                    ),
                    None,
                );
                design::key_value(
                    ui,
                    "Archive:",
                    format!("about {} (at most {})", fmt::size(pf.estimated_archive), fmt::size(pf.worst_case_archive)),
                    None,
                );
                if pf.delete_originals {
                    design::key_value(
                        ui,
                        "Space needed:",
                        format!("about {} at most at any moment, as originals are deleted along the way", fmt::size(pf.needed_estimate)),
                        None,
                    );
                }
                if !ch.parts.is_empty() {
                    let oversized = match split {
                        Ok(Split::Size(s)) => ch.parts.iter().filter(|pt| pt.estimated > s && pt.files == 1).count(),
                        _ => 0,
                    };
                    design::form_row(ui, "Parts:", |ui| {
                        design::disclosure(ui, &format!("{} ZIP files", ch.parts.len()), &mut show_parts);
                        if oversized > 0 {
                            design::help(
                                ui,
                                format!(
                                    "{} hold a single file larger than the size you chose; a file is never cut in two.",
                                    fmt::count(oversized as u64, "part", "parts")
                                ),
                            );
                        }
                    });
                    design::reveal(ui, ui.scope_id().with("parts"), show_parts, |ui| {
                        ui.add_space(space::S);
                        let cols = [
                            Col { title: "Name", width: 0.0, right: false },
                            Col { title: "Contents", width: 170.0, right: false },
                            Col { title: "Estimated Size", width: 110.0, right: true },
                        ];
                        design::table_frame(ui, |ui| {
                            let ranges = design::table_header(ui, &cols);
                            egui::ScrollArea::vertical().max_height(design::ROW_H * 6.0).id_salt("parts-table").show(ui, |ui| {
                                for (i, pt) in ch.parts.iter().enumerate() {
                                    let (rect, _, fg, fg2) = design::table_row(ui, i, false, false);
                                    let y = rect.center().y;
                                    let name = design::elide_end(ui, &fmt::file_name(&pt.output), typ::body(), ranges[0].1 - ranges[0].0);
                                    ui.painter().text(pos2(ranges[0].0, y), Align2::LEFT_CENTER, name, typ::body(), fg);
                                    ui.painter().text(
                                        pos2(ranges[1].0, y),
                                        Align2::LEFT_CENTER,
                                        format!("{} · {}", fmt::count(pt.files, "file", "files"), fmt::size(pt.bytes)),
                                        typ::body(),
                                        fg2,
                                    );
                                    ui.painter().text(
                                        pos2(ranges[2].1, y),
                                        Align2::RIGHT_CENTER,
                                        format!("≈ {}", fmt::size(pt.estimated)),
                                        typ::body(),
                                        fg2,
                                    );
                                }
                            });
                        });
                    });
                } else if matches!(split, Ok(Split::Size(_))) {
                    design::key_value(ui, "Parts:", "Everything fits in one ZIP file of that size, so it won't be split.", None);
                }
                ui.add_space(space::M);
                let (tone, title) = match pf.verdict {
                    Verdict::Fits => (Tone::Success, "There's enough space."),
                    Verdict::ProbablyFits => (Tone::Warning, "There's probably enough space."),
                    Verdict::DoesNotFit => (Tone::Error, "There isn't enough space."),
                };
                design::notice(ui, tone, title, Some(&pf.explanation));
                for w in &pf.warnings {
                    ui.add_space(6.0);
                    design::notice(ui, Tone::Warning, w, None);
                }
                if !pf.skipped.is_empty() {
                    ui.add_space(6.0);
                    design::notice(
                        ui,
                        Tone::Info,
                        &format!("{} will be left in place.", fmt::count(pf.skipped.len() as u64, "item", "items")),
                        Some("Links and special files can't be archived safely, so they're skipped."),
                    );
                }
                if let Some(u) = &ch.unfinished {
                    ui.add_space(6.0);
                    design::notice(
                        ui,
                        Tone::Info,
                        &format!("“{}” was interrupted earlier.", fmt::file_name(u)),
                        Some("Compress offers to resume it."),
                    );
                } else if !ch.existing.is_empty() {
                    ui.add_space(6.0);
                    design::notice(
                        ui,
                        Tone::Warning,
                        &format!("“{}” already exists.", fmt::file_name(&ch.existing[0])),
                        Some("You'll be asked before it's replaced. The old file stays until the new one is complete and verified."),
                    );
                }
            });
        }
    }
    c.show_parts = show_parts;
}

fn footer(ui: &mut Ui, rect: Rect, c: &mut Compress, acts: &mut Vec<Act>) {
    let p = pal(ui.ctx());
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, p.separator));
    let inner = rect.shrink2(vec2(space::XL, 0.0));
    let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)).id_salt("footer"));
    let ready = c.ready();
    let label = if c.delete { "Compress and Delete Originals" } else { "Compress" };
    let resp = ui.add_enabled_ui(ready, |ui| Button::primary(label).min_width(96.0).show(ui)).inner;
    if resp.clicked() {
        acts.push(Act::Start);
    }
    resp.on_hover_text(shortcut(&ui, "Ctrl+Enter"));
    ui.add_space(space::M);
    let status_rect = Rect::from_min_max(inner.min, pos2(ui.cursor().right(), inner.bottom()));
    let mut status = ui.new_child(UiBuilder::new().max_rect(status_rect).layout(Layout::left_to_right(Align::Center)));
    let status = &mut status;
    if let Some(b) = c.blocker() {
        design::truncated(status, b, typ::callout(), p.text_secondary);
    } else if c.checking() || c.checked.is_none() {
        design::truncated(status, "Estimating…", typ::callout(), p.text_secondary);
    } else if let Some(Ok(ch)) = &c.checked {
        let pf = &ch.preflight;
        let (color, verdict) = match pf.verdict {
            Verdict::Fits => (p.green, "fits on the drive"),
            Verdict::ProbablyFits => (p.orange, "should fit on the drive"),
            Verdict::DoesNotFit => (p.red, "doesn't fit on the drive"),
        };
        design::dot(status, color);
        let parts = if ch.parts.is_empty() { String::new() } else { format!(" in {} files", ch.parts.len()) };
        design::truncated(status, format!("About {}{parts}, {verdict}", fmt::size(pf.estimated_archive)), typ::callout(), p.text_secondary);
    } else if let Some(Err(_)) = &c.checked {
        design::dot(status, p.red);
        design::truncated(status, "The selection can't be compressed.", typ::callout(), p.text_secondary);
    }
}
