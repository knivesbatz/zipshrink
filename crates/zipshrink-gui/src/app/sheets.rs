//! Sheets: modal confirmations that scale and fade in over a dimmed
//! window. Enter chooses the default button, Escape cancels.

use super::compress::Flow;
use super::fmt;
use super::{App, Page};
use crate::design::{self, icons, motion, pal, space, typ, Button, Icon, Kind, Tone};
use eframe::egui::{self, vec2, Color32, Context, Id, Key, Sense, Ui};
use std::path::PathBuf;

#[derive(Clone)]
pub enum Sheet {
    ConsentDelete { understood: bool },
    ConfirmOverwrite(Vec<PathBuf>),
    Resume { path: PathBuf, parts: usize, deleting: bool },
    ConfirmClose,
    Message { tone: Tone, title: String, text: String },
}

#[derive(Default)]
pub struct Sheets {
    current: Option<Sheet>,
    closing: Option<(Sheet, f64)>,
    serial: u64,
}

impl Sheets {
    pub fn open(&mut self, s: Sheet) {
        self.serial += 1;
        self.current = Some(s);
    }

    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    fn close(&mut self, now: f64) {
        if let Some(s) = self.current.take() {
            self.closing = Some((s, now));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Cancel,
    Confirm,
    Alt,
}

const OUT: f64 = 0.16;

pub fn ui(app: &mut App, ctx: &Context) {
    let now = ctx.input(|i| i.time);
    if let Some((s, at)) = app.sheets.closing.clone() {
        let t = 1.0 - ((now - at) / if motion::reduced(ctx) { 0.08 } else { OUT }).clamp(0.0, 1.0) as f32;
        if t <= 0.0 || app.sheets.current.is_some() {
            app.sheets.closing = None;
        } else {
            let mut copy = s;
            show(ctx, &mut copy, t, false, app.busy());
            ctx.request_repaint();
        }
    }
    let Some(mut sheet) = app.sheets.current.take() else { return };
    let t = motion::appear(ctx, Id::unique("zs.sheet"), app.sheets.serial, 0.24);
    let choice = show(ctx, &mut sheet, t, true, app.busy());
    app.sheets.current = Some(sheet.clone());
    let Some(choice) = choice else { return };
    app.sheets.close(now);
    if choice == Choice::Cancel {
        return;
    }
    match sheet {
        Sheet::ConsentDelete { .. } => app.compress.delete = true,
        Sheet::ConfirmOverwrite(_) => app.compress.create(ctx, true),
        Sheet::Resume { path, .. } => {
            if app.busy() {
                app.toasts.show(Tone::Warning, "A job is running", "Resume this one when it finishes.");
            } else {
                app.compress.resume(ctx, path, choice == Choice::Alt);
                app.navigate(Page::Compress);
            }
        }
        Sheet::ConfirmClose => {
            if let Flow::Run(rv) = &mut app.compress.flow {
                rv.stop(true);
            }
        }
        Sheet::Message { .. } => {}
    }
}

struct Spec<'a> {
    tone: Tone,
    title: String,
    body: Vec<String>,
    buttons: Vec<(&'a str, Kind, Choice, bool)>,
}

fn spec(sheet: &Sheet, busy: bool) -> Spec<'static> {
    match sheet {
        Sheet::ConsentDelete { understood } => Spec {
            tone: Tone::Warning,
            title: "Delete originals as they're archived?".into(),
            body: vec![
                "Each original is deleted permanently — not sent to the Recycle Bin — as soon as its compressed copy has been written, flushed to the disk, read back, decompressed, matched against the original's CRC-32 and SHA-256, and recorded in the recovery journal.".into(),
                "Files that change, can't be verified, or are read-only are kept. After an interruption the job can be resumed, but the archive will then hold the only copy of files already deleted. Keep a backup of anything irreplaceable.".into(),
            ],
            buttons: vec![("Cancel", Kind::Secondary, Choice::Cancel, true), ("Delete Originals", Kind::Destructive, Choice::Confirm, *understood)],
        },
        Sheet::ConfirmOverwrite(paths) => Spec {
            tone: Tone::Warning,
            title: if paths.len() == 1 { format!("Replace “{}”?", fmt::file_name(&paths[0])) } else { format!("Replace {} existing files?", paths.len()) },
            body: vec![
                if paths.len() == 1 {
                    "A file with that name already exists in this folder.".into()
                } else {
                    format!("These already exist: {}.", paths.iter().map(|p| fmt::file_name(p)).collect::<Vec<_>>().join(", "))
                },
                "The existing file stays until the new archive is complete and verified.".into(),
            ],
            buttons: vec![("Cancel", Kind::Secondary, Choice::Cancel, true), ("Replace", Kind::Destructive, Choice::Confirm, true)],
        },
        Sheet::Resume { path, parts, deleting } => {
            let name = fmt::file_name(&zipshrink_core::JobPaths::from_any(path).output);
            let mut body = vec!["ZipShrink checks the partial archive against its journal, discards any half-written entry, and continues where it stopped.".to_string()];
            if *deleting {
                body.push("This job deletes originals. Files already archived but not yet deleted are verified again before they're deleted. Choose Resume Without Deleting to keep them.".into());
            }
            if busy {
                body.push("A job is running now; resume this one when it finishes.".into());
            }
            let mut buttons = vec![("Cancel", Kind::Secondary, Choice::Cancel, true)];
            if *deleting {
                buttons.push(("Resume Without Deleting", Kind::Secondary, Choice::Alt, !busy));
            }
            buttons.push(("Resume", Kind::Primary, Choice::Confirm, !busy));
            Spec {
                tone: Tone::Info,
                title: if *parts > 1 { format!("Resume {parts} unfinished archives?") } else { format!("Resume “{name}”?") },
                body,
                buttons,
            }
        }
        Sheet::ConfirmClose => Spec {
            tone: Tone::Warning,
            title: "Pause the job and quit?".into(),
            body: vec!["The job stops at the next safe point, then ZipShrink quits. Everything archived so far is kept, and you can resume later.".into()],
            buttons: vec![("Cancel", Kind::Secondary, Choice::Cancel, true), ("Pause and Quit", Kind::Primary, Choice::Confirm, true)],
        },
        Sheet::Message { tone, title, text } => Spec {
            tone: *tone,
            title: title.clone(),
            body: text.split("\n\n").map(str::to_string).collect(),
            buttons: vec![("OK", Kind::Primary, Choice::Confirm, true)],
        },
    }
}

/// Draws the alert as a Big Sur sheet: it hangs from the toolbar of the
/// content area, with the app icon, a bold title, small informative text,
/// and equal-width buttons (stacked when there are three).
fn show(ctx: &Context, sheet: &mut Sheet, t: f32, interactive: bool, busy: bool) -> Option<Choice> {
    let p = pal(ctx);
    let sp = spec(sheet, busy);
    let main = super::main_rect(ctx);
    let width = 280.0;
    let mut choice = None;
    let dy = motion::travel(ctx, t, -18.0);
    let frame = egui::Frame::new()
        .fill(p.elevated)
        .stroke(egui::Stroke::new(1.0, if design::is_dark(&p) { Color32::from_white_alpha(26) } else { Color32::from_black_alpha(30) }))
        .corner_radius(design::tokens::corner(design::tokens::radius::SHEET))
        .shadow(design::tokens::shadow_sheet(&p))
        .inner_margin(egui::Margin::symmetric(20, 20));
    let pos = egui::pos2(main.center().x, main.top() + space::TOOLBAR + 8.0 + dy);
    let area = egui::Modal::default_area(Id::unique("zs.sheet-area"))
        .anchor(egui::Align2::CENTER_TOP, pos - egui::pos2(ctx.content_rect().center().x, 0.0));
    let modal = egui::Modal::new(Id::unique("zs.sheet-area")).area(area).backdrop_color(p.scrim.gamma_multiply(t)).frame(frame);
    let resp = modal.show(ctx, |ui: &mut Ui| {
        ui.multiply_opacity(t);
        if !interactive {
            ui.disable();
        }
        ui.set_width(width - 40.0);
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
            design::chrome::paint_app_icon(ui, r);
            if sp.tone == Tone::Warning || sp.tone == Tone::Error {
                // Caution badge on the icon, as on Big Sur alerts.
                icons::badge(ui.painter(), r.right_bottom() - vec2(10.0, 10.0), 11.0, sp.tone.color(&p), Icon::Warning, Color32::WHITE);
            }
            ui.add_space(4.0);
            ui.add(egui::Label::new(egui::RichText::new(&sp.title).font(typ::headline()).color(p.text)).wrap().halign(egui::Align::Center));
            for b in &sp.body {
                ui.add(egui::Label::new(egui::RichText::new(b).font(typ::callout()).color(p.text)).wrap().halign(egui::Align::Center));
            }
        });
        if let Sheet::ConsentDelete { understood } = sheet {
            ui.add_space(space::S);
            design::checkbox(ui, understood, "I understand originals will be deleted");
        }
        ui.add_space(space::L);
        let stacked = sp.buttons.len() > 2;
        if stacked {
            // Default first, Cancel last.
            for (label, kind, c, enabled) in sp.buttons.iter().rev() {
                let r = ui.add_enabled_ui(*enabled, |ui| Button::new(label, *kind).min_width(ui.available_width()).show(ui)).inner;
                if r.clicked() {
                    choice = Some(*c);
                }
                ui.add_space(2.0);
            }
        } else {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let n = sp.buttons.len() as f32;
                let bw = ((ui.available_width() - 10.0 * (n - 1.0)) / n - 1.0).floor();
                for (label, kind, c, enabled) in sp.buttons.iter() {
                    let r = ui.add_enabled_ui(*enabled, |ui| Button::new(label, *kind).min_width(bw).show(ui)).inner;
                    if r.clicked() {
                        choice = Some(*c);
                    }
                }
            });
        }
    });
    if !interactive {
        return None;
    }
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
    if resp.should_close() && esc || esc {
        if sp.buttons.len() == 1 {
            choice = Some(Choice::Confirm);
        } else if sp.buttons.iter().any(|b| b.2 == Choice::Cancel) {
            choice = Some(Choice::Cancel);
        }
    }
    if !ctx.egui_wants_keyboard_input()
        && ctx.memory(|m| m.focused().is_none())
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
    {
        if let Some((_, Kind::Primary, c, true)) = sp.buttons.iter().find(|b| b.1 == Kind::Primary) {
            choice = Some(*c);
        }
    }
    choice
}
