//! The installer: Introduction, License, Destination Select, Installation
//! Type, Installation and Summary, as in macOS's Installer.app.

use super::sheet::{self, Spec, Style};
use super::task::{self, Job, Phase, Task};
use super::{content_box, drive, size, window_chrome, EXIT_CANCELLED, EXIT_DONE, EXIT_FAILED, EXIT_UNSUPPORTED, TITLE_H};
use crate::design::{self, chrome, icons, motion, pal, space, typ, Button, Icon, Kind};
use crate::ops::{Backend, Components, Existing, Plan};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, Id, Key, Layout, Rect, Sense, Stroke, Ui, UiBuilder, ViewportCommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

pub const SIZE: [f32; 2] = [680.0, 480.0];

const SIDE_W: f32 = 196.0;
const MARGIN: f32 = 20.0;
/// Room for the heading above the box.
const HEADING_H: f32 = 42.0;
const BUTTON_W: f32 = 100.0;

static LICENSE: &str = include_str!("../../../../LICENSE");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Introduction,
    License,
    Destination,
    InstallType,
    Installation,
    Summary,
}

const STEPS: [(Step, &str); 6] = [
    (Step::Introduction, "Introduction"),
    (Step::License, "License"),
    (Step::Destination, "Destination Select"),
    (Step::InstallType, "Installation Type"),
    (Step::Installation, "Installation"),
    (Step::Summary, "Summary"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sheet {
    Agree,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Next,
    Back,
    Customize,
    Standard,
    SaveLicense,
    Install,
    Open,
    Close,
}

/// The buttons under the box: an optional one on the left (Customize,
/// Save…), the secondary one and the default one on the right.
struct Nav {
    left: Option<(&'static str, Act)>,
    back: Option<(&'static str, bool, Act)>,
    next: (&'static str, bool, Act),
}

/// The optional parts as rows of the Customize table.
struct Part {
    name: &'static str,
    about: &'static str,
    size: u64,
    get: fn(&Components) -> bool,
    set: fn(&mut Components, bool),
}

const PARTS: [Part; 4] = [
    Part {
        name: "Start menu shortcut",
        about: "Puts ZipShrink in the Start menu.",
        size: 2048,
        get: |c| c.start_menu,
        set: |c, v| c.start_menu = v,
    },
    Part {
        name: "Desktop shortcut",
        about: "Puts a ZipShrink icon on the desktop.",
        size: 2048,
        get: |c| c.desktop,
        set: |c, v| c.desktop = v,
    },
    Part {
        name: "Right-click “Compress with ZipShrink”",
        about: "Adds “Compress with ZipShrink…” to File Explorer’s menu when you right-click a file or a folder.",
        size: 0,
        get: |c| c.context_menu,
        set: |c, v| c.context_menu = v,
    },
    Part {
        name: "Command-line tool on PATH",
        about: "Lets you run the zipshrink command from any Command Prompt or PowerShell window.",
        size: 0,
        get: |c| c.path,
        set: |c, v| c.path = v,
    },
];

pub struct Installer {
    backend: &'static dyn Backend,
    code: Arc<AtomicI32>,
    /// Why this Windows cannot run ZipShrink, if it cannot.
    unsupported: Option<String>,
    existing: Option<Existing>,
    step: Step,
    agreed: bool,
    dir: PathBuf,
    free: Option<u64>,
    components: Components,
    customize: bool,
    /// Selected row of the Customize table (0 = ZipShrink itself).
    row: usize,
    task: Option<Arc<Task>>,
    sheet: Option<Sheet>,
    sheet_serial: u64,
    quitting: bool,
    license: Vec<String>,
}

impl Installer {
    pub fn new(backend: &'static dyn Backend, code: Arc<AtomicI32>) -> Self {
        let unsupported = backend.check_system().err().or_else(|| {
            super::demo_switch(backend, "ZIPSHRINK_SETUP_DEMO_UNSUPPORTED")
                .map(|_| format!("ZipShrink {} needs 64-bit Windows 10 or Windows 11.", crate::VERSION))
        });
        let existing = if unsupported.is_none() { backend.detect() } else { None };
        if unsupported.is_some() {
            code.store(EXIT_UNSUPPORTED, Ordering::SeqCst);
        }
        let dir = existing.as_ref().map(|e| e.dir.clone()).unwrap_or_else(|| backend.default_dir());
        let free = if unsupported.is_none() { backend.free_space(&dir) } else { None };
        Installer {
            backend,
            code,
            unsupported,
            components: existing.as_ref().map(|e| e.components).unwrap_or_default(),
            existing,
            step: Step::Introduction,
            agreed: false,
            dir,
            free,
            customize: false,
            row: 0,
            task: None,
            sheet: None,
            sheet_serial: 0,
            quitting: false,
            license: super::reflow(LICENSE),
        }
    }

    // -------------------------------------------------------- facts

    /// What ZipShrink itself takes: the files in this installer and the
    /// uninstaller (a copy of this program).
    fn base_size(&self) -> u64 {
        if crate::payload::is_demo() {
            // A demonstration build carries no files: show a typical size.
            return 17_600 * 1024;
        }
        let me = std::env::current_exe().and_then(std::fs::metadata).map(|m| m.len()).unwrap_or(0);
        crate::payload::installed_size() + me
    }

    fn required(&self) -> u64 {
        self.base_size() + PARTS.iter().filter(|p| (p.get)(&self.components)).map(|p| p.size).sum::<u64>()
    }

    fn enough_space(&self) -> bool {
        self.free.is_none_or(|f| f >= self.required())
    }

    fn disk(&self) -> String {
        drive(&self.dir)
    }

    /// "ZipShrink 1.0.0" for the installed copy, or "ZipShrink".
    fn installed_name(&self) -> String {
        match self.existing.as_ref().and_then(|e| e.version.as_deref()) {
            Some(v) => format!("ZipShrink {v}"),
            None => "ZipShrink".into(),
        }
    }

    fn installing(&self) -> Option<Phase> {
        self.task.as_ref().map(|t| t.snapshot().phase)
    }

    fn heading(&self) -> String {
        if self.unsupported.is_some() {
            return "ZipShrink can’t be installed on this computer".into();
        }
        match self.step {
            Step::Introduction => "Welcome to the ZipShrink Installer".into(),
            Step::License => "Software License Agreement".into(),
            Step::Destination => "Select a Destination".into(),
            Step::InstallType if self.customize => format!("Custom Install on “{}”", self.disk()),
            Step::InstallType => format!("Standard Install on “{}”", self.disk()),
            Step::Installation => "Installing ZipShrink".into(),
            Step::Summary => match self.installing() {
                Some(Phase::Done(Err(_))) => "The installation failed.".into(),
                _ => "The installation was completed successfully.".into(),
            },
        }
    }

    fn nav(&self) -> Nav {
        if self.unsupported.is_some() {
            return Nav { left: None, back: None, next: ("Quit", true, Act::Close) };
        }
        let back = |on| Some(("Go Back", on, Act::Back));
        match self.step {
            Step::Introduction => Nav { left: None, back: back(false), next: ("Continue", true, Act::Next) },
            Step::License => Nav { left: Some(("Save…", Act::SaveLicense)), back: back(true), next: ("Continue", true, Act::Next) },
            Step::Destination => Nav { left: None, back: back(true), next: ("Continue", self.enough_space(), Act::Next) },
            Step::InstallType => Nav {
                left: Some(if self.customize { ("Standard Install", Act::Standard) } else { ("Customize", Act::Customize) }),
                back: back(true),
                next: ("Install", self.enough_space(), Act::Install),
            },
            Step::Installation => Nav { left: None, back: back(false), next: ("Continue", false, Act::Next) },
            Step::Summary => match self.installing() {
                Some(Phase::Done(Ok(()))) => {
                    Nav { left: None, back: Some(("Open ZipShrink", true, Act::Open)), next: ("Close", true, Act::Close) }
                }
                _ => Nav { left: None, back: None, next: ("Close", true, Act::Close) },
            },
        }
    }

    // -------------------------------------------------------- actions

    fn act(&mut self, ctx: &egui::Context, act: Act) {
        match act {
            Act::Next => match self.step {
                Step::Introduction => self.go(Step::License),
                Step::License if self.agreed => self.go(Step::Destination),
                Step::License => self.open_sheet(Sheet::Agree),
                Step::Destination => self.go(Step::InstallType),
                _ => {}
            },
            Act::Back => match self.step {
                Step::License => self.go(Step::Introduction),
                Step::Destination => self.go(Step::License),
                Step::InstallType => self.go(Step::Destination),
                _ => {}
            },
            Act::Customize => self.customize = true,
            Act::Standard => self.customize = false,
            Act::SaveLicense => {
                if let Some(path) = rfd::FileDialog::new().set_title("Save License").set_file_name("ZipShrink License.txt").save_file() {
                    let _ = std::fs::write(path, LICENSE.replace("\r\n", "\n").replace('\n', "\r\n"));
                }
            }
            Act::Install => {
                let plan = Plan { dir: self.dir.clone(), components: self.components };
                self.task = Some(Task::start(ctx, self.backend, Job::Install(plan)));
                self.go(Step::Installation);
            }
            Act::Open => {
                self.backend.launch(&self.dir);
                self.quit(ctx);
            }
            Act::Close => self.quit(ctx),
        }
    }

    fn go(&mut self, step: Step) {
        self.step = step;
    }

    fn open_sheet(&mut self, s: Sheet) {
        self.sheet = Some(s);
        self.sheet_serial += 1;
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        if let Some(t) = &self.task {
            t.cancel();
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Watches the background install; moves to Summary when it ends.
    fn poll(&mut self) {
        if self.step != Step::Installation {
            return;
        }
        if let Some(Phase::Done(r)) = self.installing() {
            self.code.store(if r.is_ok() { EXIT_DONE } else { EXIT_FAILED }, Ordering::SeqCst);
            self.go(Step::Summary);
        }
    }

    /// The window's close button (or Alt+F4): asks first while there is
    /// something to lose, refuses while files are being changed.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.quitting {
            return;
        }
        let ask = self.unsupported.is_none()
            && match self.step {
                Step::Introduction | Step::Summary => false,
                Step::Installation => self.installing() == Some(Phase::Waiting),
                _ => true,
            };
        let busy = self.step == Step::Installation && self.installing() == Some(Phase::Working);
        if ask || busy {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if ask && self.sheet != Some(Sheet::Quit) {
                self.open_sheet(Sheet::Quit);
            }
        } else if let Some(t) = &self.task {
            t.cancel();
        }
    }

    // -------------------------------------------------------- drawing

    fn sidebar(&self, ui: &mut Ui, side: Rect, first_y: f32) {
        let p = pal(ui.ctx());
        let ctx = ui.ctx().clone();
        // The large faint app icon at the foot of the step list.
        let icon = Rect::from_min_size(pos2(side.left() + 22.0, side.bottom() - 22.0 - 152.0), vec2(152.0, 152.0));
        let mut faint = ui.new_child(UiBuilder::new().max_rect(side));
        faint.multiply_opacity(if design::is_dark(&p) { 0.09 } else { 0.12 });
        chrome::paint_app_icon(&faint, icon);

        let current = if self.unsupported.is_some() { Step::Introduction } else { self.step };
        let mut y = first_y;
        for (step, name) in STEPS {
            let on = motion::fade(&ctx, Id::unique(("zs.setup.step", step as u8)), step == current, design::tokens::motion::BASE);
            let done = step < current;
            let gray = if design::is_dark(&p) { p.text_tertiary } else { Color32::from_gray(0xA4) };
            let c = pos2(side.left() + 30.0, y);
            let fill = design::lerp_color(if done { gray } else { gray.gamma_multiply(0.0) }, p.accent, on);
            ui.painter().circle_filled(c, 4.5, fill);
            if !done {
                ui.painter().circle_stroke(c, 4.0, Stroke::new(1.0, design::lerp_color(gray, p.accent, on)));
            }
            let (font, color) = if step == current {
                (typ::headline(), p.text)
            } else if done {
                (typ::body(), p.text)
            } else {
                (typ::body(), p.text_secondary)
            };
            ui.painter().text(pos2(side.left() + 44.0, y), Align2::LEFT_CENTER, name, font, color);
            y += 27.0;
        }
    }

    fn buttons(&self, ui: &mut Ui, row: Rect) -> Option<Act> {
        let nav = self.nav();
        let mut act = None;
        let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 12.0;
        let (label, on, a) = nav.next;
        if right.add_enabled_ui(on, |ui| Button::primary(label).min_width(BUTTON_W).show(ui)).inner.clicked() {
            act = Some(a);
        }
        if let Some((label, on, a)) = nav.back {
            if right.add_enabled_ui(on, |ui| Button::secondary(label).min_width(BUTTON_W).show(ui)).inner.clicked() {
                act = Some(a);
            }
        }
        if let Some((label, a)) = nav.left {
            let mut left = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)));
            if Button::secondary(label).min_width(BUTTON_W).show(&mut left).clicked() {
                act = Some(a);
            }
        }
        // Return is the default button, Escape goes back.
        if self.sheet.is_none() && ui.ctx().memory(|m| m.focused().is_none()) {
            let ctx = ui.ctx();
            if on && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                act = act.or(Some(a));
            }
            if let Some((_, true, Act::Back)) = nav.back {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                    act = act.or(Some(Act::Back));
                }
            }
        }
        act
    }

    /// The step's content inside the box. `inner` is the box less its
    /// padding; some steps (the table) use the whole box.
    fn page(&mut self, ui: &mut Ui, boxr: Rect) {
        if let Some(reason) = self.unsupported.clone() {
            return unsupported(ui, &reason);
        }
        match self.step {
            Step::Introduction => self.introduction(ui),
            Step::License => self.license(ui),
            Step::Destination => self.destination(ui),
            Step::InstallType if self.customize => self.custom(ui, boxr),
            Step::InstallType => self.standard(ui),
            Step::Installation => self.progress(ui),
            Step::Summary => self.summary(ui),
        }
    }

    fn introduction(&self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        ui.spacing_mut().item_spacing.y = 12.0;
        design::para(
            ui,
            "ZipShrink compresses a folder into a standard ZIP archive without needing room for both. If you ask it to, it deletes each \
             original as soon as its compressed copy has been written, read back and proven identical, so the space comes back while \
             the archive grows.",
            typ::body(),
            p.text,
        );
        design::para(
            ui,
            "It also checks archives, splits them into parts of any size, and picks up an interrupted job where it stopped. The \
             zipshrink command-line tool comes with it, and ZipShrink keeps itself up to date.",
            typ::body(),
            p.text,
        );
        design::para(ui, "You will be guided through the steps necessary to install this software.", typ::body(), p.text);
        if let Some(e) = &self.existing {
            ui.add_space(space::XS);
            let title = match e.version.as_deref() {
                Some(v) if v == crate::VERSION => format!("ZipShrink {v} is already installed. Continuing installs it again."),
                Some(v) => format!("This will update ZipShrink {v} to {}.", crate::VERSION),
                None => format!("This will update ZipShrink to {}.", crate::VERSION),
            };
            let body = e.msi.then_some(
                "The copy installed from a .msi package by an earlier version will be removed; Windows may ask for your permission.",
            );
            design::notice(ui, design::Tone::Info, &title, body);
        }
        if self.backend.is_demo() {
            let bottom = ui.max_rect().bottom();
            let r = Rect::from_min_max(pos2(ui.max_rect().left(), bottom - 16.0), pos2(ui.max_rect().right(), bottom));
            ui.painter().text(
                r.left_center(),
                Align2::LEFT_CENTER,
                "Demonstration build: nothing will be changed.",
                typ::caption(),
                p.text_tertiary,
            );
        }
    }

    fn license(&self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            for (i, para) in self.license.iter().enumerate() {
                let (font, color) = if i == 0 { (typ::headline(), p.text) } else { (typ::body(), p.text) };
                design::para(ui, para, font, color);
            }
        });
    }

    fn destination(&mut self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        ui.spacing_mut().item_spacing.y = 6.0;
        design::text(ui, "How do you want to install this software?", typ::body(), p.text);
        ui.add_space(space::S);
        // The one choice, drawn like Installer's selected destination.
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, 112.0), Sense::hover());
        let tile = Rect::from_center_size(row.center(), vec2(168.0, 112.0));
        let sel = if design::is_dark(&p) { Color32::from_white_alpha(14) } else { Color32::from_black_alpha(10) };
        ui.painter().rect_filled(tile, 9.0, sel);
        let icon = Rect::from_center_size(pos2(tile.center().x, tile.top() + 38.0), vec2(56.0, 56.0));
        chrome::paint_app_icon(ui, icon);
        icons::badge(ui.painter(), icon.right_bottom() - vec2(6.0, 6.0), 9.0, p.green, Icon::Check, Color32::WHITE);
        ui.painter().text(
            pos2(tile.center().x, tile.top() + 80.0),
            Align2::CENTER_CENTER,
            "Install for me only",
            typ::body_medium(),
            p.text,
        );
        ui.painter().text(
            pos2(tile.center().x, tile.top() + 97.0),
            Align2::CENTER_CENTER,
            "No administrator needed",
            typ::caption(),
            p.text_secondary,
        );
        ui.add_space(space::M);
        design::para(ui, format!("Installing this software requires {} of space.", size(self.required())), typ::body(), p.text);
        let upgrade = self.existing.is_some();
        let where_ = if upgrade {
            format!("{} is installed in this folder and will be updated there:", self.installed_name())
        } else {
            "You have chosen to install this software in the folder:".into()
        };
        design::para(ui, where_, typ::body(), p.text);
        let path = self.dir.to_string_lossy().into_owned();
        let shown = design::elide_start(ui, &path, typ::body_medium(), ui.available_width());
        design::text(ui, shown, typ::body_medium(), p.text).on_hover_text(path);
        ui.add_space(space::XS);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::M;
            let change = ui.add_enabled_ui(!upgrade, |ui| Button::secondary("Change Install Location…").show(ui)).inner;
            if change.clicked() {
                self.pick_folder();
            }
            let disk = self.disk();
            match self.free {
                Some(f) if !self.enough_space() => {
                    design::text(ui, format!("Not enough space on “{disk}”: {} available.", size(f)), typ::callout(), p.red);
                }
                Some(f) => {
                    design::text(ui, format!("{} available on “{disk}”", size(f)), typ::callout(), p.text_secondary);
                }
                None => {}
            }
        });
    }

    /// Change Install Location…: like the old installer, a folder that is
    /// not already called ZipShrink gets a ZipShrink folder inside it.
    fn pick_folder(&mut self) {
        let start = self.dir.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| self.dir.clone());
        let Some(picked) = rfd::FileDialog::new().set_title("Choose Install Location").set_directory(start).pick_folder() else { return };
        self.dir = install_dir(picked);
        self.free = self.backend.free_space(&self.dir);
    }

    fn standard(&self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        ui.spacing_mut().item_spacing.y = 12.0;
        design::para(ui, format!("This will take {} of space on your computer.", size(self.required())), typ::body(), p.text);
        let disk = self.disk();
        let line = match &self.existing {
            Some(_) => format!(
                "Click Install to update {} to {} on the disk “{disk}”. The parts you chose before are kept.",
                self.installed_name(),
                crate::VERSION
            ),
            None => format!("Click Install to perform a standard installation of this software for you on the disk “{disk}”."),
        };
        design::para(ui, line, typ::body(), p.text);
        ui.add_space(space::XS);
        ui.spacing_mut().item_spacing.y = 7.0;
        let mut items = vec!["The ZipShrink app and the zipshrink command-line tool"];
        items.extend(PARTS.iter().filter(|part| (part.get)(&self.components)).map(|part| part.name));
        for item in items {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Check, p.green);
                design::text(ui, item, typ::body(), p.text_secondary);
            });
        }
    }

    /// The Customize table: every part with a checkbox, what it will do and
    /// its size; the selected row's description; the space it all needs.
    fn custom(&mut self, ui: &mut Ui, boxr: Rect) {
        let p = pal(ui.ctx());
        let inner = boxr.shrink(1.0);
        let mut t = ui.new_child(UiBuilder::new().max_rect(inner));
        t.spacing_mut().item_spacing.y = 0.0;
        let cols = [
            design::Col { title: "Package Name", width: 0.0, right: false },
            design::Col { title: "Action", width: 86.0, right: false },
            design::Col { title: "Size", width: 84.0, right: true },
        ];
        let ranges = design::table_header(&mut t, &cols);
        let upgrade = self.existing.is_some();
        let had = self.existing.as_ref().map(|e| e.components);
        for i in 0..=PARTS.len() {
            let (rect, resp, fg, fg2) = design::table_row(&mut t, i, self.row == i, false);
            if resp.clicked() {
                self.row = i;
            }
            let (name, mut on, bytes, was) = if i == 0 {
                ("ZipShrink", true, self.base_size(), upgrade)
            } else {
                let part = &PARTS[i - 1];
                (part.name, (part.get)(&self.components), part.size, had.is_some_and(|h| (part.get)(&h)))
            };
            let cell = Rect::from_min_max(pos2(ranges[0].0, rect.top()), pos2(ranges[0].1, rect.bottom()));
            let mut c = t.new_child(UiBuilder::new().max_rect(cell).layout(Layout::left_to_right(Align::Center)));
            if i == 0 {
                // Always installed: a dimmed checkbox, but a readable name.
                c.spacing_mut().item_spacing.x = 0.0;
                c.add_enabled_ui(false, |ui| design::checkbox(ui, &mut on, ""));
                design::text(&mut c, name, typ::body(), fg);
            } else if design::checkbox(&mut c, &mut on, name).changed() {
                (PARTS[i - 1].set)(&mut self.components, on);
                self.row = i;
            }
            let action = match (on, was) {
                (false, _) => "Skip",
                (true, true) => "Upgrade",
                (true, false) => "Install",
            };
            let cy = rect.center().y;
            t.painter().text(pos2(ranges[1].0, cy), Align2::LEFT_CENTER, action, typ::body(), if on { fg } else { fg2 });
            let sz = if on { size(bytes) } else { "—".into() };
            t.painter().text(pos2(ranges[2].1, cy), Align2::RIGHT_CENTER, sz, typ::body(), fg2);
        }
        // The description and the totals sit in the box's lower part.
        let sep_y = inner.bottom() - 84.0;
        ui.painter().line_segment([pos2(inner.left(), sep_y), pos2(inner.right(), sep_y)], Stroke::new(1.0, p.separator));
        let about = if self.row == 0 {
            "The ZipShrink app, the zipshrink command-line tool and the uninstaller. Always installed."
        } else {
            PARTS[self.row - 1].about
        };
        let desc = Rect::from_min_max(pos2(inner.left() + 16.0, sep_y + 10.0), pos2(inner.right() - 16.0, inner.bottom() - 30.0));
        let mut d = ui.new_child(UiBuilder::new().max_rect(desc));
        design::para(&mut d, about, typ::callout(), p.text_secondary);
        let y = inner.bottom() - 16.0;
        let need = format!("Space Required:  {}", size(self.required()));
        ui.painter().text(pos2(inner.left() + 16.0, y), Align2::LEFT_CENTER, need, typ::callout(), p.text_secondary);
        if let Some(f) = self.free {
            let left = format!("Remaining:  {}", size(f.saturating_sub(self.required())));
            let color = if self.enough_space() { p.text_secondary } else { p.red };
            ui.painter().text(pos2(inner.right() - 16.0, y), Align2::RIGHT_CENTER, left, typ::callout(), color);
        }
    }

    fn progress(&self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        let Some(task) = &self.task else { return };
        let s = task.snapshot();
        let area = ui.max_rect();
        let bar_w = (area.width() - 40.0).min(360.0);
        let col = Rect::from_center_size(area.center() - vec2(0.0, 14.0), vec2(bar_w, 96.0));
        let mut c = ui.new_child(UiBuilder::new().max_rect(col));
        c.spacing_mut().item_spacing.y = 10.0;
        let (status, frac, under) = match s.phase {
            Phase::Waiting => (
                "ZipShrink is open. It will be updated when you quit it.".to_string(),
                None,
                "Installation continues by itself once ZipShrink has closed.".to_string(),
            ),
            _ => (s.status.clone(), Some(s.frac), format!("Install time remaining: {}", task::remaining(&s))),
        };
        design::text(&mut c, status, typ::body(), p.text);
        design::progress_bar(&mut c, Id::unique("zs.setup.progress"), frac, 6.0);
        design::text(&mut c, under, typ::callout(), p.text_secondary);
    }

    fn summary(&self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        let area = ui.max_rect();
        let ok = !matches!(self.installing(), Some(Phase::Done(Err(_))));
        let t = motion::appear(ui.ctx(), Id::unique("zs.setup.done"), 1, 1.0);
        let c = pos2(area.center().x, area.top() + area.height() * 0.36);
        let r = 34.0;
        let grow = (t / 0.45).min(1.0);
        let painter = ui.painter();
        if ok {
            painter.circle_filled(c, r * (0.6 + 0.4 * grow), p.green.gamma_multiply(grow));
            let draw = ((t - 0.35) / 0.65).clamp(0.0, 1.0);
            let glyph = Rect::from_center_size(c, vec2(r * 1.5, r * 1.5));
            icons::check_stroke(painter, glyph, Color32::WHITE, 5.0, draw);
        } else {
            icons::badge(painter, c, r * (0.6 + 0.4 * grow), p.red.gamma_multiply(grow), Icon::Close, Color32::WHITE.gamma_multiply(grow));
        }
        let text = Rect::from_min_max(pos2(area.left() + 12.0, c.y + r + 18.0), pos2(area.right() - 12.0, area.bottom()));
        let mut col = ui.new_child(UiBuilder::new().max_rect(text).layout(Layout::top_down(Align::Center)));
        col.spacing_mut().item_spacing.y = 6.0;
        let center = |ui: &mut Ui, s: String, font, color| {
            ui.add(egui::Label::new(egui::RichText::new(s).font(font).color(color)).wrap().halign(Align::Center));
        };
        match self.installing() {
            Some(Phase::Done(Err(reason))) => {
                center(&mut col, "The installation failed.".into(), typ::title2(), p.text);
                center(
                    &mut col,
                    "The installer encountered an error that caused the installation to fail.".into(),
                    typ::body(),
                    p.text_secondary,
                );
                center(&mut col, reason, typ::callout(), p.text_secondary);
            }
            _ => {
                center(&mut col, "The installation was successful.".into(), typ::title2(), p.text);
                let what = match &self.existing {
                    Some(_) => format!("ZipShrink was updated to {}.", crate::VERSION),
                    None => "ZipShrink was installed.".into(),
                };
                center(&mut col, what, typ::body(), p.text_secondary);
            }
        }
    }
}

/// Windows too old (or not 64-bit): what is wrong, and that nothing changed.
fn unsupported(ui: &mut Ui, reason: &str) {
    let p = pal(ui.ctx());
    let area = ui.max_rect();
    let icon = Rect::from_center_size(pos2(area.center().x, area.top() + area.height() * 0.30), vec2(72.0, 72.0));
    chrome::paint_app_icon(ui, icon);
    icons::badge(ui.painter(), icon.right_bottom() - vec2(10.0, 10.0), 13.0, p.orange, Icon::Warning, Color32::WHITE);
    let text = Rect::from_min_max(pos2(area.left() + 16.0, icon.bottom() + 20.0), pos2(area.right() - 16.0, area.bottom()));
    let mut col = ui.new_child(UiBuilder::new().max_rect(text).layout(Layout::top_down(Align::Center)));
    col.spacing_mut().item_spacing.y = 8.0;
    col.add(egui::Label::new(egui::RichText::new(reason).font(typ::body()).color(p.text)).wrap().halign(Align::Center));
    col.add(
        egui::Label::new(egui::RichText::new("Nothing was changed on this computer.").font(typ::callout()).color(p.text_secondary))
            .wrap()
            .halign(Align::Center),
    );
}

/// The folder ZipShrink goes in when the user picks `picked`.
fn install_dir(picked: PathBuf) -> PathBuf {
    let named = picked.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("ZipShrink"));
    if named {
        picked
    } else {
        picked.join("ZipShrink")
    }
}

impl eframe::App for Installer {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = super::begin_frame(&ctx);
        self.poll();
        self.handle_close(&ctx);
        window_chrome(ui, "Install ZipShrink");

        let full = ui.max_rect();
        let side = Rect::from_min_max(pos2(full.left(), full.top() + TITLE_H), pos2(full.left() + SIDE_W, full.bottom()));
        let pane_l = side.right();
        let pane_r = full.right() - MARGIN;
        let buttons = Rect::from_min_max(pos2(pane_l, full.bottom() - MARGIN - space::CONTROL), pos2(pane_r, full.bottom() - MARGIN));
        let boxr = Rect::from_min_max(pos2(pane_l, full.top() + TITLE_H + HEADING_H), pos2(pane_r, buttons.top() - space::L));
        self.sidebar(ui, side, boxr.top() + 16.0);

        // The step changes with a short cross-fade and slide.
        let key = (self.step as u64) << 1 | self.customize as u64 | (self.unsupported.is_some() as u64) << 8;
        let t = motion::appear(&ctx, Id::unique("zs.setup.page"), key, motion::PAGE);
        let dx = motion::travel(&ctx, t, 10.0);
        let heading = self.heading();
        let heading_font = typ::toolbar();
        let shown = design::elide_end(ui, &heading, heading_font.clone(), boxr.width());
        let hy = boxr.top() - HEADING_H / 2.0 + 1.0;
        ui.painter().text(pos2(pane_l + 2.0 + dx, hy), Align2::LEFT_CENTER, shown, heading_font, p.text.gamma_multiply(t));

        content_box(ui, boxr);
        let mut content = ui.new_child(UiBuilder::new().max_rect(boxr.shrink2(vec2(22.0, 18.0)).translate(vec2(dx, 0.0))));
        content.set_clip_rect(boxr.shrink(1.0));
        content.multiply_opacity(t);
        if self.sheet.is_some() {
            content.disable();
        }
        self.page(&mut content, boxr.translate(vec2(dx, 0.0)));

        let act = {
            let mut row = ui.new_child(UiBuilder::new().max_rect(buttons));
            if self.sheet.is_some() {
                row.disable();
            }
            self.buttons(&mut row, buttons)
        };
        if let Some(a) = act {
            self.act(&ctx, a);
        }
        self.sheets(&ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(design::tokens::LIGHT.sidebar).to_array()
    }
}

impl Installer {
    fn sheets(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else { return };
        match sheet {
            Sheet::Agree => {
                let spec = Spec {
                    style: Style::Wide,
                    title: "To continue installing the software you must agree to the terms of the software license agreement.",
                    body: &["Click Agree to continue or click Disagree to cancel the installation and quit the Installer."],
                    buttons: &[
                        ("Read License", Kind::Secondary, true),
                        ("Disagree", Kind::Secondary, false),
                        ("Agree", Kind::Primary, false),
                    ],
                };
                match sheet::show(ctx, &spec, self.sheet_serial) {
                    Some(0) => self.sheet = None,
                    Some(1) => {
                        self.sheet = None;
                        self.code.store(EXIT_CANCELLED, Ordering::SeqCst);
                        self.quit(ctx);
                    }
                    Some(_) => {
                        self.sheet = None;
                        self.agreed = true;
                        self.go(Step::Destination);
                    }
                    None => {}
                }
            }
            Sheet::Quit => {
                let body: &[&str] = if self.step == Step::Installation {
                    &["ZipShrink has not been updated yet. Nothing has been changed on this computer."]
                } else {
                    &["Nothing has been installed yet."]
                };
                let spec = Spec {
                    style: Style::Alert,
                    title: "Are you sure you want to quit the installer?",
                    body,
                    buttons: &[("Cancel", Kind::Secondary, true), ("Quit", Kind::Primary, false)],
                };
                match sheet::show(ctx, &spec, self.sheet_serial) {
                    Some(0) => self.sheet = None,
                    Some(_) => {
                        self.sheet = None;
                        self.code.store(EXIT_CANCELLED, Ordering::SeqCst);
                        self.quit(ctx);
                    }
                    None => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picked_folders_get_a_zipshrink_folder() {
        assert_eq!(install_dir(PathBuf::from("/apps")), PathBuf::from("/apps/ZipShrink"));
        assert_eq!(install_dir(PathBuf::from("/apps/zipshrink")), PathBuf::from("/apps/zipshrink"));
    }

    #[test]
    fn steps_are_in_order() {
        assert!(STEPS.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
