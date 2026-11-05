//! The application window, laid out like a Big Sur app: a full-height
//! sidebar under the traffic lights, a unified toolbar with the page title,
//! and the page content. Also navigation, shortcuts and worker glue.

mod compress;
mod fmt;
mod jobs;
mod prefs;
mod resume;
mod run;
mod sheets;
mod toast;
mod updates;
mod verify;

use crate::design::{self, chrome, icons, motion, pal, space, theme, typ, Icon};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, Id, Key, KeyboardShortcut, Layout, Modifiers, Rect, Sense, Stroke, Ui, UiBuilder,
};
use std::path::PathBuf;
use zipshrink_update::Settings;

pub use sheets::Sheet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Compress,
    Verify,
    Resume,
    Preferences,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Compress => "Compress",
            Page::Verify => "Verify",
            Page::Resume => "Resume",
            Page::Preferences => "Preferences",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Page::Compress => Icon::Archive,
            Page::Verify => Icon::Shield,
            Page::Resume => Icon::Resume,
            Page::Preferences => Icon::Gear,
        }
    }
}

/// Commands from the keyboard and the macOS menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Cmd {
    /// Add files, choose archives or open a job, depending on the page.
    Open,
    OpenFolder,
    Go(Page),
    /// Close the window (asks first while a job runs).
    Close,
    /// Quit: the same path as closing the window, even with a sheet open.
    Quit,
    InstallCli,
    Help,
}

/// Toolbar actions, handled after the frame is drawn.
enum Tool {
    AddFiles,
    AddFolder,
    ChooseArchives,
    OpenJob,
    Pause,
    NewArchive,
    Preferences,
}

pub struct App {
    page: Page,
    compress: compress::Compress,
    verify: verify::Verify,
    resume: resume::Resume,
    prefs: prefs::Prefs,
    sheets: sheets::Sheets,
    toasts: toast::Toasts,
    updates: updates::Updates,
    settings: Settings,
    appearance: theme::Appearance,
    /// Set while the window is closing because the user confirmed it.
    closing: bool,
    /// The single-instance lock and the inbox other launches write to.
    instance: Option<crate::instance::Primary>,
    /// Wall-clock time: `logic` also runs while the window is hidden, when
    /// egui's input time stands still.
    next_inbox: std::time::Instant,
    /// Smoke-test mode (`ZIPSHRINK_SMOKE`): first frame reported, close sent.
    smoke_ready: bool,
    smoke_closed: bool,
}

impl App {
    pub fn new(ctx: &egui::Context, sources: Vec<PathBuf>, resume: Option<PathBuf>, instance: Option<crate::instance::Primary>) -> Self {
        design::fonts::install(ctx);
        theme::install_style(ctx);
        let settings = Settings::load();
        let mut app = App {
            page: Page::Compress,
            compress: compress::Compress::default(),
            verify: verify::Verify::default(),
            resume: resume::Resume::default(),
            prefs: prefs::Prefs::default(),
            sheets: sheets::Sheets::default(),
            toasts: toast::Toasts::default(),
            updates: updates::Updates::new(ctx),
            appearance: theme::Appearance::from_setting(settings.appearance),
            settings,
            closing: false,
            instance,
            next_inbox: std::time::Instant::now(),
            smoke_ready: false,
            smoke_closed: false,
        };
        // On macOS remembered jobs may sit in protected folders (Desktop,
        // Documents) whose access an update resets; look at them only when
        // the Resume page opens, not at every start, and until then count
        // the remembered ones.
        if cfg!(target_os = "macos") {
            app.resume.recount();
        } else {
            app.resume.refresh();
        }
        app.open_paths(ctx, sources);
        if let Some(r) = resume {
            app.ask_resume(r);
        }
        app
    }

    fn busy(&self) -> bool {
        self.compress.running()
    }

    fn reduce_motion(&self) -> bool {
        self.settings.reduce_motion || motion::system_prefers_reduced()
    }

    /// Paths from the command line, the right-click menu, a drop, or a dialog.
    fn open_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        crate::smoke::opened(&paths);
        let (jobs, rest): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| {
            let s = p.to_string_lossy();
            s.ends_with(".partial") || s.ends_with(".partial.journal")
        });
        if let Some(j) = jobs.into_iter().next() {
            self.ask_resume(j);
        }
        if rest.is_empty() {
            return;
        }
        if self.page == Page::Verify && rest.iter().all(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))) {
            self.verify.start(ctx, rest);
            return;
        }
        if self.busy() {
            self.toasts.show(design::Tone::Warning, "A Job Is Running", "Add more items when it finishes or after you pause it.");
            return;
        }
        let n = rest.len();
        let had = self.compress.sources.len();
        self.compress.add_sources(rest);
        self.page = Page::Compress;
        if had > 0 {
            self.toasts.show(
                design::Tone::Success,
                &format!("Added {}", fmt::count(n as u64, "Item", "Items")),
                "They'll go into the same archive.",
            );
        }
    }

    fn ask_resume(&mut self, path: PathBuf) {
        let targets = zipshrink_core::split::resume_targets(&path);
        let deleting = jobs::deletes_originals(&targets[0]);
        self.sheets.open(Sheet::Resume { path, parts: targets.len(), deleting });
    }

    /// Paths handed over by later launches (Explorer's right-click menu
    /// starts one per selected item).
    fn poll_inbox(&mut self, ctx: &egui::Context) {
        const EVERY: std::time::Duration = std::time::Duration::from_millis(400);
        if self.instance.is_none() {
            return;
        }
        let now = std::time::Instant::now();
        if now < self.next_inbox {
            ctx.request_repaint_after(self.next_inbox - now);
            return;
        }
        self.next_inbox = now + EVERY;
        ctx.request_repaint_after(EVERY);
        let Some(msgs) = self.instance.as_ref().and_then(|i| i.take()) else { return };
        reveal(ctx);
        for m in msgs {
            match m {
                crate::instance::Message::Open(paths) => self.open_paths(ctx, paths),
                crate::instance::Message::Resume(p) => self.ask_resume(p),
            }
        }
    }

    /// Files from Finder, the Dock and the Services menu, menu commands and
    /// Quit, queued by the AppKit bridge.
    #[cfg(target_os = "macos")]
    fn macos_events(&mut self, ctx: &egui::Context) {
        for e in crate::macos::take() {
            match e {
                crate::macos::Event::Open(paths) => {
                    reveal(ctx);
                    self.open_paths(ctx, paths);
                }
                crate::macos::Event::Command(c) => {
                    if c == Cmd::Quit {
                        crate::smoke::log("quit");
                    }
                    self.command(ctx, c);
                }
                crate::macos::Event::CliInstalled(Ok(())) => {
                    self.toasts.show(design::Tone::Success, "Command Line Tool Installed", "Type zipshrink in Terminal to use it.")
                }
                crate::macos::Event::CliInstalled(Err(e)) => {
                    self.toasts.show(design::Tone::Error, "The Command Line Tool Couldn't Be Installed", &e)
                }
            }
        }
    }

    fn command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        if cmd == Cmd::Quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.sheets.is_open() {
            return;
        }
        let idle_compress = self.page == Page::Compress && !self.busy();
        match cmd {
            Cmd::Go(page) => self.navigate(page),
            Cmd::Open => match self.page {
                Page::Verify => self.verify.pick(ctx),
                Page::Resume => self.pick_job(),
                _ if idle_compress => self.compress.pick_files(),
                _ => {}
            },
            Cmd::OpenFolder => {
                if idle_compress {
                    self.compress.pick_folder();
                }
            }
            Cmd::Close | Cmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Cmd::InstallCli => {
                #[cfg(target_os = "macos")]
                crate::macos::install_cli();
            }
            Cmd::Help => zipshrink_update::open_url(&format!("https://github.com/{}#readme", zipshrink_update::REPO)),
        }
    }

    fn navigate(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            if page == Page::Resume {
                self.resume.refresh();
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.sheets.is_open() {
            return;
        }
        // On macOS the menu bar takes these keys first and sends the same
        // commands.
        let cmd = |k: Key| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let keys = [
            (cmd(Key::Num1), Cmd::Go(Page::Compress)),
            (cmd(Key::Num2), Cmd::Go(Page::Verify)),
            (cmd(Key::Num3), Cmd::Go(Page::Resume)),
            (cmd(Key::Comma), Cmd::Go(Page::Preferences)),
            (KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::O), Cmd::OpenFolder),
            (cmd(Key::O), Cmd::Open),
            (cmd(Key::W), Cmd::Close),
        ];
        for (k, c) in keys {
            if ctx.input_mut(|i| i.consume_shortcut(&k)) {
                self.command(ctx, c);
            }
        }
    }

    fn pick_job(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open an Interrupted Job")
            .add_filter("Interrupted ZipShrink job", &["partial", "journal"])
            .pick_file()
        {
            self.ask_resume(p);
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.busy() && !self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.compress.closing_after_stop() {
                crate::smoke::log("confirm-close");
                self.sheets.open(Sheet::ConfirmClose);
                // Quit from the Dock or at log out may come while the
                // window is minimized or hidden: show the question.
                reveal(ctx);
            }
            return;
        }
        // A downloaded update installs as the app closes.
        self.updates.install_on_close();
    }

    fn window_title(&self, ctx: &egui::Context) {
        let title = match self.compress.percent() {
            Some(p) => format!("ZipShrink — {p:.0}%"),
            None => "ZipShrink".into(),
        };
        let id = Id::unique("zs.title");
        let prev: Option<String> = ctx.data(|d| d.get_temp(id));
        if prev.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            ctx.data_mut(|d| d.insert_temp(id, title));
        }
    }

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut Ui, compact: f32) {
        let p = pal(ui.ctx());
        let rect = ui.max_rect();
        // A vibrancy-like material: a soft vertical tint and a hairline edge.
        let top = design::lerp_color(p.sidebar, Color32::WHITE, if design::is_dark(&p) { 0.02 } else { 0.25 });
        design::gradient_rrect(ui.painter(), rect, 0.0, top, p.sidebar);
        let edge = if design::is_dark(&p) { Color32::BLACK } else { p.separator };
        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, edge));
        chrome::drag_region(ui, Rect::from_min_size(rect.min, vec2(rect.width(), space::TOOLBAR)), "sidebar");
        chrome::traffic_lights(ui, rect.min + vec2(20.0, space::TOOLBAR / 2.0));

        let inner = Rect::from_min_max(rect.min + vec2(10.0, space::TOOLBAR + 4.0), rect.max - vec2(10.0, 10.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.y = 1.0;
        let (r, _) = ui.allocate_exact_size(vec2(inner.width(), 22.0), Sense::hover());
        if compact < 1.0 {
            ui.painter().text(
                pos2(r.left() + 8.0, r.center().y + 2.0),
                Align2::LEFT_CENTER,
                "Archive",
                typ::caption_medium(),
                p.text_secondary.gamma_multiply(0.85 * (1.0 - compact)),
            );
        }
        let mut target = None;
        let unfinished = self.resume.count();
        for page in [Page::Compress, Page::Verify, Page::Resume] {
            let badge = (page == Page::Resume && unfinished > 0).then_some(unfinished);
            let running = page == Page::Compress && self.busy();
            if self.nav_item(ui, page, compact, badge, running).clicked() {
                target = Some(page);
            }
        }
        if let Some((title, detail)) = self.updates.sidebar_note() {
            let free = ui.available_height() - 46.0;
            if free > 0.0 {
                ui.add_space(free);
            }
            let (r, resp) = ui.allocate_exact_size(vec2(inner.width(), 42.0), Sense::click());
            let ht = motion::fade(ui.ctx(), resp.id.with("h"), resp.hovered(), design::tokens::motion::FAST);
            ui.painter().rect_filled(r, 6.0, p.fill_hover.gamma_multiply(0.5 * ht));
            let ic = Rect::from_min_size(r.min + vec2(8.0, 12.0), vec2(18.0, 18.0));
            icons::paint(ui.painter(), ic, Icon::Download, p.accent);
            if compact < 1.0 {
                let a = 1.0 - compact;
                let c = ui.painter().with_clip_rect(r);
                c.text(pos2(ic.right() + 8.0, r.top() + 13.0), Align2::LEFT_CENTER, &title, typ::body(), p.text.gamma_multiply(a));
                c.text(
                    pos2(ic.right() + 8.0, r.top() + 29.0),
                    Align2::LEFT_CENTER,
                    &detail,
                    typ::caption(),
                    p.text_secondary.gamma_multiply(a),
                );
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &title));
            if resp.clicked() {
                self.updates.show_banner();
            }
        }
        if let Some(t) = target {
            self.navigate(t);
        }
    }

    fn nav_item(&self, ui: &mut Ui, page: Page, compact: f32, badge: Option<usize>, running: bool) -> egui::Response {
        let p = pal(ui.ctx());
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
        let selected = self.page == page;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, page.title()));
        let ctx = ui.ctx();
        let sel = motion::fade(ctx, resp.id.with("sel"), selected, design::tokens::motion::BASE);
        if sel > 0.0 {
            ui.painter().rect_filled(rect, 5.0, p.selection.gamma_multiply(sel));
        }
        let icon_rect = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 8.5), vec2(17.0, 17.0));
        icons::paint(ui.painter(), icon_rect, page.icon(), p.accent);
        if compact < 1.0 {
            let a = 1.0 - compact;
            ui.painter().with_clip_rect(rect).text(
                pos2(icon_rect.right() + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                page.title(),
                typ::body(),
                p.text.gamma_multiply(a),
            );
        }
        let right = pos2(rect.right() - 10.0, rect.center().y);
        if running {
            let t = ui.input(|i| i.time) as f32;
            let pulse = if motion::reduced(ctx) { 1.0 } else { 0.5 + 0.5 * (t * 3.0).sin().abs() };
            let at = if compact >= 1.0 { pos2(rect.right() - 7.0, rect.top() + 7.0) } else { right };
            ui.painter().circle_filled(at, 3.0, p.accent.gamma_multiply(pulse));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
        } else if let Some(n) = badge {
            let at = if compact >= 1.0 { pos2(rect.right() - 6.0, rect.top() + 7.0) } else { right };
            ui.painter().text(at, Align2::RIGHT_CENTER, n.to_string(), typ::body(), p.text_secondary);
        }
        if resp.has_focus() {
            design::focus_ring(ui, rect, 5.0);
        }
        if compact >= 1.0 {
            resp.on_hover_text(page.title())
        } else {
            resp
        }
    }

    // ------------------------------------------------------------ toolbar

    fn subtitle(&self) -> Option<String> {
        match self.page {
            Page::Compress => match &self.compress.flow {
                compress::Flow::Setup if !self.compress.sources.is_empty() => {
                    Some(fmt::count(self.compress.sources.len() as u64, "item", "items"))
                }
                compress::Flow::Run(_) => self.compress.percent().map(|p| format!("{p:.0}% done")),
                _ => None,
            },
            Page::Resume if self.resume.count() > 0 => Some(fmt::count(self.resume.count() as u64, "interrupted job", "interrupted jobs")),
            Page::Verify if self.verify.running() => Some("Checking…".into()),
            _ => None,
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, rect: Rect) -> Option<Tool> {
        let p = pal(ui.ctx());
        chrome::drag_region(ui, rect, "toolbar");
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, p.separator));
        let mut out = None;
        let mut child =
            ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(space::M, 0.0))).layout(Layout::right_to_left(Align::Center)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 2.0;
        let idle = !self.busy();
        if self.page != Page::Preferences
            && design::toolbar_button(ui, Icon::Gear, &format!("Preferences ({})", compress::shortcut(ui, "Ctrl+,"))).clicked()
        {
            out = Some(Tool::Preferences);
        }
        match (self.page, &self.compress.flow) {
            (Page::Compress, compress::Flow::Setup) => {
                if design::toolbar_button(ui, Icon::Folder, &format!("Add Folder ({})", compress::shortcut(ui, "Ctrl+Shift+O"))).clicked() {
                    out = Some(Tool::AddFolder);
                }
                if design::toolbar_button(ui, Icon::Plus, &format!("Add Files ({})", compress::shortcut(ui, "Ctrl+O"))).clicked() {
                    out = Some(Tool::AddFiles);
                }
            }
            (Page::Compress, compress::Flow::Run(_)) => {
                if design::toolbar_button(ui, Icon::Pause, &format!("Pause at the Next Safe Point ({})", compress::shortcut(ui, "Ctrl+.")))
                    .clicked()
                {
                    out = Some(Tool::Pause);
                }
            }
            (Page::Compress, compress::Flow::Done(_)) => {
                if design::toolbar_button(ui, Icon::Plus, &format!("New Archive ({})", compress::shortcut(ui, "Ctrl+N"))).clicked() {
                    out = Some(Tool::NewArchive);
                }
            }
            (Page::Verify, _) => {
                let label = format!("Choose Archives ({})", compress::shortcut(ui, "Ctrl+O"));
                let r = ui.add_enabled_ui(!self.verify.running(), |ui| design::toolbar_button(ui, Icon::Plus, &label)).inner;
                if r.clicked() {
                    out = Some(Tool::ChooseArchives);
                }
            }
            (Page::Resume, _) => {
                let label = format!("Open a Job File ({})", compress::shortcut(ui, "Ctrl+O"));
                let r = ui.add_enabled_ui(idle, |ui| design::toolbar_button(ui, Icon::Folder, &label)).inner;
                if r.clicked() {
                    out = Some(Tool::OpenJob);
                }
            }
            _ => {}
        }
        let left = rect.left() + space::L + 4.0;
        let painter = ui.painter();
        match self.subtitle() {
            Some(s) => {
                painter.text(pos2(left, rect.center().y - 7.5), Align2::LEFT_CENTER, self.page.title(), typ::toolbar(), p.text);
                painter.text(pos2(left, rect.center().y + 10.0), Align2::LEFT_CENTER, s, typ::caption(), p.text_secondary);
            }
            None => {
                painter.text(pos2(left, rect.center().y), Align2::LEFT_CENTER, self.page.title(), typ::toolbar(), p.text);
            }
        }
        out
    }

    fn content(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let key = self.page as u64 * 16 + self.compress.stage();
        let t = motion::appear(&ctx, Id::unique("zs.page"), key, motion::PAGE);
        let dy = motion::travel(&ctx, t, 6.0);
        let full = ui.max_rect();
        let mut child = ui.new_child(UiBuilder::new().max_rect(full.translate(vec2(0.0, dy))));
        child.set_clip_rect(full);
        child.multiply_opacity(t);
        let ui = &mut child;
        match self.page {
            Page::Compress => compress::page(self, ui),
            Page::Verify => verify::page(self, ui),
            Page::Resume => resume::page(self, ui),
            Page::Preferences => prefs::page(self, ui),
        }
    }

    /// The drop target shown while files are dragged over the window.
    fn drop_overlay(&mut self, ctx: &egui::Context, area: Rect) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let t = motion::fade(ctx, Id::unique("zs.drop"), hovering, design::tokens::motion::FAST);
        if t > 0.0 {
            let p = pal(ctx);
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("zs.drop.layer")));
            let r = area.shrink(10.0);
            painter.rect_filled(r, 10.0, p.accent.gamma_multiply(0.06 * t));
            painter.rect_stroke(r, 10.0, Stroke::new(3.0, p.accent.gamma_multiply(0.85 * t)), egui::StrokeKind::Inside);
            let (title, sub) = match self.page {
                Page::Verify => ("Drop to Verify", "ZIP archives are read back and checked entry by entry."),
                _ if self.busy() => ("A Job Is Running", "You can add items when it finishes."),
                _ => ("Drop to Add", "Files and folders are added to the new archive."),
            };
            let c = r.center();
            painter.text(c, Align2::CENTER_CENTER, title, typ::title2(), p.text.gamma_multiply(t));
            painter.text(c + vec2(0.0, 24.0), Align2::CENTER_CENTER, sub, typ::body(), p.text_secondary.gamma_multiply(t));
        }
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            self.open_paths(ctx, dropped);
        }
    }

    /// `ZIPSHRINK_SMOKE`: report the first frame and a few platform facts.
    fn smoke_ready(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if crate::smoke::remaining().is_none() {
            return;
        }
        if !self.smoke_ready {
            self.smoke_ready = true;
            crate::smoke::log("ready");
            crate::smoke::log(&format!("system_theme={:?} reduce_motion={}", ctx.system_theme(), motion::system_prefers_reduced()));
            #[cfg(target_os = "macos")]
            {
                use raw_window_handle::HasWindowHandle;
                if let Ok(h) = frame.window_handle() {
                    crate::smoke::log(&format!("chrome {:?}", eframe::WindowChromeMetrics::from_window_handle(&h.as_raw())));
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = frame;
    }

    /// `ZIPSHRINK_SMOKE`: close once the time is up (also while hidden).
    fn smoke_timer(&mut self, ctx: &egui::Context) {
        let Some(left) = crate::smoke::remaining() else { return };
        if !left.is_zero() {
            ctx.request_repaint_after(left);
        } else if !self.smoke_closed {
            self.smoke_closed = true;
            crate::smoke::log("close");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn apply_tool(&mut self, ctx: &egui::Context, tool: Tool) {
        match tool {
            Tool::AddFiles => self.compress.pick_files(),
            Tool::AddFolder => self.compress.pick_folder(),
            Tool::ChooseArchives => self.verify.pick(ctx),
            Tool::OpenJob => self.pick_job(),
            Tool::Pause => {
                if let compress::Flow::Run(rv) = &mut self.compress.flow {
                    rv.stop(false);
                }
            }
            Tool::NewArchive => self.compress.new_archive(),
            Tool::Preferences => self.navigate(Page::Preferences),
        }
    }
}

/// Rounded rectangle outline drawn with dashes.
pub fn dashed_rect(painter: &egui::Painter, r: Rect, radius: f32, stroke: Stroke) {
    let rr = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    let mut pts = Vec::new();
    let corners = [
        (r.right_top() + vec2(-rr, rr), -90.0f32),
        (r.right_bottom() + vec2(-rr, -rr), 0.0),
        (r.left_bottom() + vec2(rr, -rr), 90.0),
        (r.left_top() + vec2(rr, rr), 180.0),
    ];
    for (c, start) in corners {
        for k in 0..=8 {
            let a = (start + 90.0 * k as f32 / 8.0).to_radians();
            pts.push(c + vec2(a.cos(), a.sin()) * rr);
        }
    }
    pts.push(pts[0]);
    painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
}

/// The area right of the sidebar (sheets hang from its toolbar).
pub fn main_rect(ctx: &egui::Context) -> Rect {
    ctx.data(|d| d.get_temp(Id::unique("zs.main-rect"))).unwrap_or_else(|| ctx.content_rect())
}

/// Brings the window back (from the Dock, from Hide or from behind other
/// windows) for something that needs the user: files handed over, or a
/// question before quitting.
fn reveal(ctx: &egui::Context) {
    #[cfg(target_os = "macos")]
    crate::macos::reveal();
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

impl eframe::App for App {
    /// Runs before every frame, and also while the window is minimized,
    /// hidden or covered, when eframe skips `ui`: Quit (from the Dock, the
    /// menu or at log out), files from Finder, Services and other launches,
    /// and the end of a job must not wait for the window to be seen again.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        self.macos_events(ctx);
        self.handle_close(ctx);
        self.poll_inbox(ctx);
        self.compress.poll(ctx, &mut self.sheets, &mut self.toasts);
        if let Some(outputs) = self.compress.take_finished_outputs() {
            jobs::prune(&outputs);
            // Look in other folders only where that is no privacy question,
            // or after the Resume page did.
            if cfg!(target_os = "macos") && !self.resume.checked() {
                self.resume.recount();
            } else {
                self.resume.refresh();
            }
        }
        self.smoke_timer(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        motion::set_reduced(&ctx, self.reduce_motion());
        let p = theme::frame(&ctx, self.appearance);
        // macOS: the real traffic lights, centred in the toolbar strip like
        // Big Sur's unified title bar (AppKit moves them back on every
        // layout, so this runs every frame). Does nothing elsewhere.
        frame.set_traffic_lights_position(&ctx, egui::Rangef::new(0.0, space::TOOLBAR), 13.0);
        self.verify.poll();
        self.updates.poll(&ctx, self.busy(), &mut self.toasts, &mut self.settings);
        self.shortcuts(&ctx);
        self.window_title(&ctx);

        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, p.bg);
        let compact_target = full.width() < 760.0;
        let compact = motion::slide(&ctx, Id::unique("zs.sidebar"), compact_target, design::tokens::motion::SLOW);
        let side_w = egui::lerp(space::SIDEBAR..=space::SIDEBAR_COMPACT, compact);
        let side = Rect::from_min_size(full.min, vec2(side_w, full.height()));
        let main = Rect::from_min_max(pos2(side.right(), full.top()), full.max);
        let bar = Rect::from_min_size(main.min, vec2(main.width(), space::TOOLBAR));
        let body = Rect::from_min_max(pos2(main.left(), bar.bottom()), main.max);
        ctx.data_mut(|d| d.insert_temp(Id::unique("zs.main-rect"), main));
        {
            let mut s = ui.new_child(UiBuilder::new().max_rect(side).id_salt("sidebar"));
            self.sidebar(&mut s, compact);
        }
        let tool = {
            let mut t = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("toolbar"));
            self.toolbar(&mut t, bar)
        };
        {
            let mut m = ui.new_child(UiBuilder::new().max_rect(body).id_salt("main"));
            m.set_clip_rect(body);
            self.content(&mut m);
        }
        if let Some(t) = tool {
            self.apply_tool(&ctx, t);
        }
        self.updates.banner(&ctx, main, self.busy());
        self.toasts.ui(&ctx, main);
        self.drop_overlay(&ctx, main);
        sheets::ui(self, &ctx);
        chrome::resize_edges(&ctx);
        self.smoke_ready(&ctx, frame);
    }

    /// Last resort for quitting paths that never ask the window to close
    /// (`close_requested` is not set): a downloaded update still installs.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.updates.install_on_close();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(design::tokens::LIGHT.bg).to_array()
    }
}
