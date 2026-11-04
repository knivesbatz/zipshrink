//! Big Sur sheets: a raised panel that slides down from the title bar over
//! a dimmed window. Two styles, as Installer.app uses them: the alert
//! (app icon, bold title, small text, equal-width buttons) and the wide
//! license sheet (text on the left, a button at each end).

use crate::design::{self, chrome, icons, motion, pal, space, typ, Button, Icon, Kind};
use eframe::egui::{self, vec2, Color32, Context, Id, Key, RichText, Sense, Ui};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Alert,
    Wide,
}

pub struct Spec<'a> {
    pub style: Style,
    pub title: &'a str,
    pub body: &'a [&'a str],
    /// Label, look, and whether Escape picks it. The `Primary` one is what
    /// Return picks.
    pub buttons: &'a [(&'a str, Kind, bool)],
}

/// Draws the sheet; returns the index of the button chosen this frame.
/// `serial` changes each time a sheet opens, restarting its entrance.
pub fn show(ctx: &Context, spec: &Spec, serial: u64) -> Option<usize> {
    let p = pal(ctx);
    let t = motion::appear(ctx, Id::unique("zs.setup.sheet"), serial, 0.26);
    let width = match spec.style {
        Style::Alert => 280.0,
        Style::Wide => 440.0,
    };
    let dy = motion::travel(ctx, t, -16.0);
    let edge = if design::is_dark(&p) { Color32::from_white_alpha(26) } else { Color32::from_black_alpha(30) };
    let frame = egui::Frame::new()
        .fill(p.elevated)
        .stroke(egui::Stroke::new(1.0, edge))
        .corner_radius(design::tokens::corner(design::tokens::radius::SHEET))
        .shadow(design::tokens::shadow_sheet(&p))
        .inner_margin(egui::Margin::same(20));
    let area =
        egui::Modal::default_area(Id::unique("zs.setup.sheet-area")).anchor(egui::Align2::CENTER_TOP, vec2(0.0, super::TITLE_H + 2.0 + dy));
    let modal = egui::Modal::new(Id::unique("zs.setup.sheet-area")).area(area).backdrop_color(p.scrim.gamma_multiply(t)).frame(frame);
    let mut choice = None;
    modal.show(ctx, |ui: &mut Ui| {
        ui.multiply_opacity(t);
        ui.set_width(width - 40.0);
        match spec.style {
            Style::Alert => alert(ui, spec, &mut choice),
            Style::Wide => wide(ui, spec, &mut choice),
        }
    });
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
    if esc {
        choice = choice.or(spec.buttons.iter().position(|b| b.2));
    }
    if ctx.memory(|m| m.focused().is_none()) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
        choice = choice.or(spec.buttons.iter().position(|b| b.1 == Kind::Primary));
    }
    choice
}

fn alert(ui: &mut Ui, spec: &Spec, choice: &mut Option<usize>) {
    let p = pal(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
        chrome::paint_app_icon(ui, r);
        icons::badge(ui.painter(), r.right_bottom() - vec2(10.0, 10.0), 11.0, p.orange, Icon::Warning, Color32::WHITE);
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(spec.title).font(typ::headline()).color(p.text)).wrap().halign(egui::Align::Center));
        for b in spec.body {
            ui.add(egui::Label::new(RichText::new(*b).font(typ::callout()).color(p.text)).wrap().halign(egui::Align::Center));
        }
    });
    ui.add_space(space::L);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let n = spec.buttons.len() as f32;
        let bw = ((ui.available_width() - 10.0 * (n - 1.0)) / n).floor();
        for (i, (label, kind, _)) in spec.buttons.iter().enumerate() {
            if Button::new(label, *kind).min_width(bw).show(ui).clicked() {
                *choice = Some(i);
            }
        }
    });
}

fn wide(ui: &mut Ui, spec: &Spec, choice: &mut Option<usize>) {
    let p = pal(ui.ctx());
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.add(egui::Label::new(RichText::new(spec.title).font(typ::headline()).color(p.text)).wrap());
    for b in spec.body {
        design::para(ui, *b, typ::body(), p.text_secondary);
    }
    ui.add_space(space::M);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let Some(((first, kind, _), rest)) = spec.buttons.split_first() else { return };
        if Button::new(first, *kind).min_width(96.0).show(ui).clicked() {
            *choice = Some(0);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (i, (label, kind, _)) in rest.iter().enumerate().rev() {
                if Button::new(label, *kind).min_width(84.0).show(ui).clicked() {
                    *choice = Some(i + 1);
                }
            }
        });
    });
}
