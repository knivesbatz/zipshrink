//! Preferences, arranged like a Big Sur preferences window: icon tabs
//! across the top (General, Updates, About) and a centred form below.

use super::compress::column;
use super::App;
use crate::design::{self, chrome, icons, motion, pal, space, theme::Appearance, typ, Button, Icon};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Id, Rect, Sense, Stroke, Ui};

#[derive(Default)]
pub struct Prefs {
    tab: usize,
}

const TABS: [(&str, Icon); 3] = [("General", Icon::Gear), ("Updates", Icon::Download), ("About", Icon::Info)];

fn tab_bar(ui: &mut Ui, tab: &mut usize) {
    let p = pal(ui.ctx());
    let w = 76.0;
    let total = w * TABS.len() as f32;
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, (label, icon)) in TABS.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 50.0), Sense::click());
            let sel = *tab == i;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, sel, *label));
            let t = motion::fade(ui.ctx(), resp.id.with("sel"), sel, design::tokens::motion::BASE);
            if t > 0.0 {
                ui.painter().rect_filled(rect.shrink2(vec2(4.0, 0.0)), 6.0, p.fill.gamma_multiply(t));
            }
            let c = if sel { p.accent } else { p.text_secondary };
            icons::paint(ui.painter(), Rect::from_center_size(rect.center() - vec2(0.0, 8.0), vec2(22.0, 22.0)), *icon, c);
            ui.painter().text(
                rect.center() + vec2(0.0, 15.0),
                Align2::CENTER_CENTER,
                *label,
                typ::caption(),
                if sel { p.text } else { p.text_secondary },
            );
            if resp.has_focus() {
                design::focus_ring(ui, rect.shrink2(vec2(4.0, 0.0)), 6.0);
            }
            if resp.clicked() {
                *tab = i;
            }
        }
    });
}

/// A miniature window in the given appearance, as in Big Sur's General pane.
fn appearance_swatch(ui: &mut Ui, kind: Appearance, selected: bool) -> egui::Response {
    let p = pal(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(68.0, 64.0), Sense::click());
    let label = match kind {
        Appearance::Light => "Light",
        Appearance::Dark => "Dark",
        Appearance::System => "Auto",
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, label));
    let thumb = Rect::from_min_size(rect.min + vec2(0.0, 2.0), vec2(68.0, 44.0));
    let draw = |painter: &egui::Painter, r: Rect, light: bool| {
        let (desk, win, side, bar) = if light {
            (Color32::from_rgb(0x9C, 0xC4, 0xEE), Color32::WHITE, Color32::from_rgb(0xE6, 0xE6, 0xE8), Color32::from_rgb(0xD0, 0xD0, 0xD4))
        } else {
            (
                Color32::from_rgb(0x2C, 0x3E, 0x66),
                Color32::from_rgb(0x26, 0x26, 0x28),
                Color32::from_rgb(0x36, 0x36, 0x39),
                Color32::from_rgb(0x4A, 0x4A, 0x4E),
            )
        };
        painter.rect_filled(r, 5.0, desk);
        let w = Rect::from_min_max(r.min + vec2(9.0, 9.0), r.max - vec2(9.0, 7.0));
        painter.rect_filled(w, 3.0, win);
        painter.rect_filled(
            Rect::from_min_max(w.min, pos2(w.left() + 16.0, w.bottom())),
            egui::CornerRadius { nw: 3, sw: 3, ne: 0, se: 0 },
            side,
        );
        for k in 0..3 {
            painter.circle_filled(
                w.min + vec2(3.5 + 3.5 * k as f32, 3.5),
                1.1,
                [Color32::from_rgb(255, 95, 87), Color32::from_rgb(254, 188, 46), Color32::from_rgb(40, 200, 64)][k],
            );
        }
        painter.rect_filled(Rect::from_min_size(w.min + vec2(21.0, 5.0), vec2(20.0, 2.5)), 1.0, bar);
        painter.rect_filled(Rect::from_min_size(w.min + vec2(21.0, 11.0), vec2(14.0, 2.5)), 1.0, bar);
    };
    match kind {
        Appearance::Light => draw(ui.painter(), thumb, true),
        Appearance::Dark => draw(ui.painter(), thumb, false),
        Appearance::System => {
            let half = thumb.center().x;
            draw(&ui.painter().with_clip_rect(Rect::from_min_max(thumb.min, pos2(half, thumb.bottom()))), thumb, true);
            draw(&ui.painter().with_clip_rect(Rect::from_min_max(pos2(half, thumb.top()), thumb.max)), thumb, false);
        }
    }
    let t = motion::fade(ui.ctx(), resp.id.with("sel"), selected, design::tokens::motion::BASE);
    let edge = if design::is_dark(&p) { Color32::from_white_alpha(30) } else { Color32::from_black_alpha(30) };
    ui.painter().rect_stroke(thumb, 5.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    if t > 0.0 {
        ui.painter().rect_stroke(thumb.expand(2.5), 7.5, Stroke::new(3.0, p.accent.gamma_multiply(t)), egui::StrokeKind::Middle);
    }
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 6.0),
        Align2::CENTER_CENTER,
        label,
        typ::caption(),
        if selected { p.text } else { p.text_secondary },
    );
    if resp.has_focus() {
        design::focus_ring(ui, thumb, 5.0);
    }
    resp
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    let mut check = false;
    let mut save = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(space::M);
        column(ui, |ui| {
            tab_bar(ui, &mut app.prefs.tab);
            design::separator(ui);
            let t = motion::appear(&ctx, Id::unique("zs.prefs-tab"), app.prefs.tab as u64, design::tokens::motion::BASE);
            ui.multiply_opacity(t);
            match app.prefs.tab {
                0 => {
                    design::form_row(ui, "Appearance:", |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::M;
                            for kind in [Appearance::Light, Appearance::Dark, Appearance::System] {
                                if appearance_swatch(ui, kind, app.appearance == kind).clicked() && app.appearance != kind {
                                    app.appearance = kind;
                                    app.settings.appearance = kind.to_setting();
                                    save = true;
                                }
                            }
                        });
                        design::help(
                            ui,
                            if cfg!(target_os = "macos") {
                                "Auto follows the appearance set in System Settings."
                            } else {
                                "Auto follows the appearance set in Windows."
                            },
                        );
                    });
                    ui.add_space(space::L);
                    let system = motion::system_prefers_reduced();
                    design::form_row(ui, "Motion:", |ui| {
                        let mut on = app.settings.reduce_motion || system;
                        ui.add_enabled_ui(!system, |ui| {
                            if design::checkbox(ui, &mut on, "Reduce motion").changed() {
                                app.settings.reduce_motion = on;
                                save = true;
                            }
                        });
                        design::help(
                            ui,
                            if system && cfg!(target_os = "macos") {
                                "On because Reduce motion is turned on in System Settings > Accessibility > Display."
                            } else if system {
                                "On because animation effects are turned off in Windows Settings."
                            } else {
                                "Replaces sliding and scaling with simple fades."
                            },
                        );
                    });
                    design::separator(ui);
                    design::text(ui, "Keyboard Shortcuts", typ::headline(), p.text);
                    ui.add_space(space::S);
                    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
                    let k = |s: &str| if mac { s.replace("Ctrl+Shift+", "⇧⌘").replace("Ctrl+", "⌘") } else { s.to_string() };
                    for (keys, what) in [
                        ("Ctrl+O", "Add files, choose archives or open a job"),
                        ("Ctrl+Shift+O", "Add a folder"),
                        ("Ctrl+Enter", "Compress"),
                        ("Ctrl+.", "Pause the running job"),
                        ("Ctrl+N", "New archive, after a job"),
                        ("Ctrl+1, 2, 3", "Compress, Verify, Resume"),
                        ("Ctrl+,", "Preferences"),
                        ("Delete", "Remove the selected item"),
                        ("Tab", "Move between controls"),
                    ] {
                        design::key_value(ui, &k(keys), what, None);
                    }
                }
                1 => {
                    design::form_row(ui, "", |ui| {
                        if design::checkbox(ui, &mut app.settings.check_for_updates, "Automatically keep ZipShrink up to date").changed() {
                            save = true;
                        }
                        design::help(ui, "Checks GitHub once a week, downloads a new version in the background and checks its SHA-256, then asks before installing.");
                    });
                    ui.add_space(space::L);
                    design::form_row(ui, "Status:", |ui| {
                        ui.horizontal(|ui| {
                            if app.updates.busy() {
                                design::spinner(ui, 14.0);
                            }
                            design::para(ui, app.updates.status_line(), typ::body(), p.text);
                        });
                        ui.add_space(4.0);
                        let r = ui.add_enabled_ui(!app.updates.busy(), |ui| Button::secondary("Check Now").show(ui)).inner;
                        if r.clicked() {
                            check = true;
                        }
                    });
                    ui.add_space(space::L);
                    design::key_value(ui, "Installed version:", zipshrink_update::CURRENT, None);
                    design::key_value(ui, "Updates from:", format!("github.com/{}", zipshrink_update::REPO), None);
                }
                _ => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(space::M);
                        let (r, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
                        chrome::paint_app_icon(ui, r);
                        ui.add_space(space::S);
                        design::text(ui, "ZipShrink", typ::title(), p.text);
                        design::text(ui, format!("Version {}", zipshrink_update::CURRENT), typ::callout(), p.text_secondary);
                        ui.add_space(space::M);
                        ui.scope(|ui| {
                            ui.set_max_width(360.0);
                            design::para(ui, "Creates verified ZIP archives, and frees disk space as it goes by deleting each original only after its compressed copy is safely on disk.", typ::body(), p.text);
                        });
                        ui.add_space(space::M);
                        design::centered_row(ui, 280.0, |ui| {
                            if design::link(ui, "Website").clicked() {
                                zipshrink_update::open_url(&format!("https://github.com/{}", zipshrink_update::REPO));
                            }
                            design::text(ui, "·", typ::callout(), p.text_tertiary);
                            if design::link(ui, "Release Notes").clicked() {
                                zipshrink_update::open_url(&format!("https://github.com/{}/releases", zipshrink_update::REPO));
                            }
                            design::text(ui, "·", typ::callout(), p.text_tertiary);
                            if design::link(ui, "Report a Problem").clicked() {
                                zipshrink_update::open_url(&format!("https://github.com/{}/issues/new/choose", zipshrink_update::REPO));
                            }
                        });
                        ui.add_space(space::L);
                        design::para(ui, "Free software under the MIT License.\nTypeface: Inter, by Rasmus Andersson, under the SIL Open Font License 1.1.", typ::caption(), p.text_secondary);
                    });
                }
            }
            ui.add_space(space::XXL);
        });
    });
    if save {
        app.settings.save();
    }
    if check {
        app.updates.check(&ctx, true, &mut app.settings);
    }
}
