//! The uninstaller (this program run as Uninstall.exe): a small window in
//! the style of a Big Sur alert, then progress, then the outcome.

use super::task::{Job, Phase, Task};
use super::{window_chrome, EXIT_CANCELLED, EXIT_DONE, EXIT_FAILED, TITLE_H};
use crate::design::{self, chrome, icons, pal, space, typ, Button, Icon, Kind};
use crate::ops::Backend;
use eframe::egui::{self, pos2, vec2, Align, Color32, Id, Key, Layout, Rect, Ui, UiBuilder, ViewportCommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

pub const SIZE: [f32; 2] = [500.0, 214.0];

const MARGIN: f32 = 22.0;
const ICON: f32 = 64.0;

pub struct Uninstaller {
    backend: &'static dyn Backend,
    code: Arc<AtomicI32>,
    dir: PathBuf,
    version: Option<String>,
    task: Option<Arc<Task>>,
    quitting: bool,
}

impl Uninstaller {
    pub fn new(backend: &'static dyn Backend, code: Arc<AtomicI32>) -> Self {
        let existing = backend.detect();
        let dir = existing
            .as_ref()
            .map(|e| e.dir.clone())
            .or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())))
            .unwrap_or_else(|| backend.default_dir());
        Uninstaller { backend, code, dir, version: existing.and_then(|e| e.version), task: None, quitting: false }
    }

    fn phase(&self) -> Option<Phase> {
        self.task.as_ref().map(|t| t.snapshot().phase)
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        if let Some(t) = &self.task {
            t.cancel();
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn body(&self, ui: &mut Ui) {
        let p = pal(ui.ctx());
        ui.spacing_mut().item_spacing.y = 6.0;
        let name = match &self.version {
            Some(v) => format!("ZipShrink {v}"),
            None => "ZipShrink".into(),
        };
        match self.phase() {
            None => {
                design::para(ui, format!("Are you sure you want to uninstall {name}?"), typ::headline(), p.text);
                design::para(
                    ui,
                    "ZipShrink will be removed from this computer, with its shortcuts, its “Compress with ZipShrink…” menu entry and the \
                     zipshrink command. Your settings and your archives are kept.",
                    typ::callout(),
                    p.text_secondary,
                );
                let path = self.dir.to_string_lossy().into_owned();
                let shown = design::elide_start(ui, &path, typ::callout(), ui.available_width());
                design::text(ui, shown, typ::callout(), p.text_tertiary).on_hover_text(path);
            }
            Some(Phase::Waiting) => {
                design::para(ui, "ZipShrink is open.", typ::headline(), p.text);
                design::para(ui, "It will be removed when you quit it.", typ::callout(), p.text_secondary);
                ui.add_space(space::XS);
                design::progress_bar(ui, Id::unique("zs.setup.un-progress"), None, 6.0);
            }
            Some(Phase::Working) => {
                let s = self.task.as_ref().map(|t| t.snapshot());
                design::para(ui, format!("Uninstalling {name}…"), typ::headline(), p.text);
                design::para(ui, s.as_ref().map(|s| s.status.clone()).unwrap_or_default(), typ::callout(), p.text_secondary);
                ui.add_space(space::XS);
                design::progress_bar(ui, Id::unique("zs.setup.un-progress"), s.map(|s| s.frac), 6.0);
            }
            Some(Phase::Done(Ok(()))) => {
                design::para(ui, "ZipShrink was uninstalled.", typ::headline(), p.text);
                design::para(ui, "Your settings and your archives were kept.", typ::callout(), p.text_secondary);
            }
            Some(Phase::Done(Err(reason))) => {
                design::para(ui, "ZipShrink could not be uninstalled.", typ::headline(), p.text);
                design::para(ui, reason, typ::callout(), p.text_secondary);
            }
        }
    }
}

impl eframe::App for Uninstaller {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = super::begin_frame(&ctx);
        let phase = self.phase();
        if let Some(Phase::Done(r)) = &phase {
            self.code.store(if r.is_ok() { EXIT_DONE } else { EXIT_FAILED }, Ordering::SeqCst);
        }
        // Files are being removed: the window stays until that is done.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if phase == Some(Phase::Working) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            } else if let Some(t) = &self.task {
                t.cancel();
            }
        }
        window_chrome(ui, "Uninstall ZipShrink");

        let full = ui.max_rect();
        let icon = Rect::from_min_size(pos2(full.left() + MARGIN + 2.0, full.top() + TITLE_H + 10.0), vec2(ICON, ICON));
        chrome::paint_app_icon(ui, icon);
        let badge = match &phase {
            Some(Phase::Done(Ok(()))) => Some((p.green, Icon::Check)),
            Some(Phase::Done(Err(_))) => Some((p.red, Icon::Close)),
            _ => None,
        };
        if let Some((color, glyph)) = badge {
            icons::badge(ui.painter(), icon.right_bottom() - vec2(9.0, 9.0), 11.0, color, glyph, Color32::WHITE);
        }
        let text = Rect::from_min_max(
            pos2(icon.right() + 18.0, icon.top() + 4.0),
            pos2(full.right() - MARGIN, full.bottom() - MARGIN - space::CONTROL - space::M),
        );
        let mut col = ui.new_child(UiBuilder::new().max_rect(text));
        self.body(&mut col);

        let row = Rect::from_min_max(
            pos2(text.left(), full.bottom() - MARGIN - space::CONTROL),
            pos2(full.right() - MARGIN, full.bottom() - MARGIN),
        );
        let mut b = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)));
        b.spacing_mut().item_spacing.x = 12.0;
        let keys_free = ctx.memory(|m| m.focused().is_none());
        match phase {
            None => {
                let uninstall = Button::new("Uninstall", Kind::Destructive).min_width(96.0).show(&mut b);
                let cancel = Button::secondary("Cancel").min_width(96.0).show(&mut b);
                let esc = keys_free && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
                if cancel.clicked() || esc {
                    self.code.store(EXIT_CANCELLED, Ordering::SeqCst);
                    self.quit(&ctx);
                } else if uninstall.clicked() {
                    self.task = Some(Task::start(&ctx, self.backend, Job::Uninstall(self.dir.clone())));
                }
            }
            Some(Phase::Waiting) => {
                b.add_enabled_ui(false, |ui| Button::new("Uninstall", Kind::Destructive).min_width(96.0).show(ui));
                if Button::secondary("Cancel").min_width(96.0).show(&mut b).clicked() {
                    self.code.store(EXIT_CANCELLED, Ordering::SeqCst);
                    self.quit(&ctx);
                }
            }
            Some(Phase::Working) => {
                b.add_enabled_ui(false, |ui| Button::new("Uninstall", Kind::Destructive).min_width(96.0).show(ui));
                b.add_enabled_ui(false, |ui| Button::secondary("Cancel").min_width(96.0).show(ui));
            }
            Some(Phase::Done(_)) => {
                let close = Button::primary("Close").min_width(96.0).show(&mut b);
                let enter = keys_free && ctx.input_mut(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape));
                if close.clicked() || enter {
                    self.quit(&ctx);
                }
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(design::tokens::LIGHT.sidebar).to_array()
    }
}
