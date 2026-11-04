//! The Resume page: interrupted jobs, ready to continue.

use super::compress::{column, shortcut};
use super::fmt;
use super::jobs::{self, Unfinished};
use super::App;
use crate::design::{self, icons, pal, space, typ, Button, Col, Icon};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, Layout, Rect, Sense, Ui};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Resume {
    list: Vec<Unfinished>,
    checked_at: Option<Instant>,
}

impl Resume {
    pub fn refresh(&mut self) {
        self.list = jobs::unfinished();
        self.checked_at = Some(Instant::now());
    }

    pub fn count(&self) -> usize {
        self.list.len()
    }
}

fn ago(t: Option<std::time::SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..=89 => "Just now".into(),
        90..=3599 => format!("{} minutes ago", s / 60),
        3600..=7199 => "An hour ago".into(),
        7200..=86399 => format!("{} hours ago", s / 3600),
        86400..=172799 => "Yesterday".into(),
        _ => format!("{} days ago", s / 86400),
    }
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    if app.resume.checked_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) && !app.busy() {
        app.resume.refresh();
    }
    let busy = app.busy();
    let mut resume = None;
    let mut open = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(space::XL);
        column(ui, |ui| {
            if app.resume.list.is_empty() {
                ui.add_space(space::XXL * 2.0);
                design::empty_state(
                    ui,
                    Icon::Resume,
                    "No Interrupted Jobs",
                    "If a job is paused, or stops because of a crash or a full drive, it appears here. Everything archived before that is kept safe.",
                    |ui| {
                        design::centered_row(ui, 130.0, |ui| {
                            if Button::secondary("Open Job File…").show(ui).on_hover_text(format!("Open a .partial or .journal file ({})", shortcut(ui, "Ctrl+O"))).clicked() {
                                open = true;
                            }
                        });
                    },
                );
                return;
            }
            design::secondary(ui, "These jobs stopped before they finished. Resume continues where each one stopped; nothing archived so far is lost.");
            ui.add_space(space::M);
            let cols = [Col { title: "Archive", width: 0.0, right: false }, Col { title: "Interrupted", width: 120.0, right: false }, Col { title: "", width: 96.0, right: true }];
            let sel_id = ui.scope_id().with("sel");
            let mut selected: Option<usize> = ctx.data(|d| d.get_temp(sel_id)).flatten();
            design::table_frame(ui, |ui| {
                let ranges = design::table_header(ui, &cols);
                for (i, u) in app.resume.list.iter().enumerate() {
                    let is_sel = selected == Some(i);
                    let w = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
                    let bg = if is_sel { p.accent } else if i % 2 == 1 { p.stripe } else { Color32::TRANSPARENT };
                    ui.painter().rect_filled(rect, if is_sel { 4.0 } else { 0.0 }, bg);
                    let (fg, fg2) = if is_sel { (p.on_accent, p.on_accent.gamma_multiply(0.85)) } else { (p.text, p.text_secondary) };
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Row, true, is_sel, fmt::file_name(&u.output)));
                    if resp.clicked() {
                        selected = Some(i);
                    }
                    if resp.double_clicked() && !busy {
                        resume = Some(u.output.clone());
                    }
                    let ic = Rect::from_center_size(pos2(ranges[0].0 + 10.0, rect.center().y), vec2(20.0, 20.0));
                    icons::paint(ui.painter(), ic, Icon::Archive, if is_sel { fg } else { p.accent });
                    let tx = ic.right() + 8.0;
                    let maxw = ranges[0].1 - tx;
                    let mut name = fmt::file_name(&u.output);
                    if u.deletes_originals {
                        name.push_str("  · deletes originals");
                    }
                    let name = design::elide_end(ui, &name, typ::body(), maxw);
                    ui.painter().text(pos2(tx, rect.center().y - 7.0), Align2::LEFT_CENTER, name, typ::body(), fg);
                    let folder = design::elide_start(ui, &fmt::parent(&u.output), typ::caption(), maxw);
                    ui.painter().text(pos2(tx, rect.center().y + 8.0), Align2::LEFT_CENTER, folder, typ::caption(), fg2);
                    ui.painter().text(pos2(ranges[1].0, rect.center().y), Align2::LEFT_CENTER, ago(u.modified), typ::body(), fg2);
                    let br = Rect::from_min_max(pos2(ranges[2].0, rect.center().y - 12.0), pos2(ranges[2].1, rect.center().y + 12.0));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(Layout::right_to_left(Align::Center)));
                    let b = child.add_enabled_ui(!busy, |ui| Button::secondary("Resume…").show(ui)).inner;
                    if b.clicked() {
                        resume = Some(u.output.clone());
                    }
                }
            });
            ctx.data_mut(|d| d.insert_temp(sel_id, selected));
            if busy {
                ui.add_space(space::M);
                design::notice(ui, design::Tone::Info, "A job is running.", Some("You can resume another job when it finishes."));
            }
        });
    });
    if open {
        app.pick_job();
    }
    if let Some(r) = resume {
        app.ask_resume(r);
    }
}
