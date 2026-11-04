//! Toasts: brief, non-blocking confirmations at the bottom of the window.

use super::fmt;
use super::run::DoneView;
use crate::design::{self, icons, motion, pal, space, typ, Tone};
use eframe::egui::{self, pos2, vec2, Color32, Context, Id, Rect, Sense};

struct Toast {
    id: u64,
    tone: Tone,
    title: String,
    body: String,
    born: f64,
    ttl: f64,
}

#[derive(Default)]
pub struct Toasts {
    list: Vec<Toast>,
    next: u64,
}

impl Toasts {
    pub fn show(&mut self, tone: Tone, title: &str, body: &str) {
        self.next += 1;
        // Same message again just refreshes the existing one.
        self.list.retain(|t| t.title != title);
        self.list.push(Toast {
            id: self.next,
            tone,
            title: title.into(),
            body: body.into(),
            born: f64::NAN,
            ttl: if tone == Tone::Error { 8.0 } else { 4.5 },
        });
        if self.list.len() > 3 {
            self.list.remove(0);
        }
    }

    pub fn finished(&mut self, d: &DoneView) {
        let (tone, title) = d.headline();
        let (_, arch, freed) = d.saved();
        let body = if freed > 0 {
            format!("{} archive · {} freed", fmt::size(arch), fmt::size(freed))
        } else {
            format!("{} archive", fmt::size(arch))
        };
        self.show(tone, title, &body);
    }

    /// Notification Center-style banners at the top right: the app icon,
    /// a bold title, the message and "now"; they slide in from the edge.
    pub fn ui(&mut self, ctx: &Context, area: Rect) {
        let now = ctx.input(|i| i.time);
        let p = pal(ctx);
        let w = 344.0_f32.min(area.width() - 2.0 * space::M);
        let mut y = area.top() + space::TOOLBAR + space::S;
        let mut dismiss = None;
        for t in self.list.iter_mut() {
            if t.born.is_nan() {
                t.born = now;
            }
            let age = now - t.born;
            let id = Id::unique("zs.toast").with(t.id);
            let enter = motion::appear(ctx, id.with("in"), t.id, design::tokens::motion::SLOW);
            let leave = ((t.ttl - age) / 0.3).clamp(0.0, 1.0) as f32;
            let a = enter.min(leave);
            let dx = motion::travel(ctx, a, 40.0);
            let h = 64.0;
            let rect = Rect::from_min_size(pos2(area.right() - w - space::M + dx, y), vec2(w, h));
            let resp = egui::Area::new(id).order(egui::Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
                ui.multiply_opacity(a);
                let (r, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
                ui.painter().add(design::tokens::shadow_popover(&p).as_shape(r, 13.0));
                ui.painter().rect_filled(r, 13.0, p.elevated.gamma_multiply(0.97));
                let edge = if design::is_dark(&p) { Color32::from_white_alpha(24) } else { Color32::from_black_alpha(22) };
                ui.painter().rect_stroke(r, 13.0, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
                let icon = Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 18.0), vec2(36.0, 36.0));
                design::chrome::paint_app_icon(ui, icon);
                if t.tone != Tone::Success && t.tone != Tone::Info {
                    icons::badge(ui.painter(), icon.right_bottom() - vec2(3.0, 3.0), 7.0, t.tone.color(&p), t.tone.icon(), Color32::WHITE);
                }
                let x = icon.right() + 10.0;
                let clip = ui.painter().with_clip_rect(r.shrink(4.0));
                clip.text(pos2(r.right() - 12.0, r.top() + 18.0), egui::Align2::RIGHT_CENTER, "now", typ::caption(), p.text_secondary);
                let title = design::elide_end(ui, &t.title, typ::headline(), r.right() - 50.0 - x);
                clip.text(pos2(x, r.top() + 18.0), egui::Align2::LEFT_CENTER, title, typ::headline(), p.text);
                let body = design::elide_end(ui, &t.body, typ::body(), r.right() - 14.0 - x);
                clip.text(pos2(x, r.top() + 39.0), egui::Align2::LEFT_CENTER, body, typ::body(), p.text);
                if resp.hovered() {
                    // The close button appears at the top-left corner on hover.
                    let c = r.left_top() + vec2(4.0, 4.0);
                    ui.painter().circle_filled(c, 9.0, p.elevated);
                    ui.painter().circle_stroke(c, 9.0, egui::Stroke::new(1.0, edge));
                    icons::paint(ui.painter(), Rect::from_center_size(c, vec2(10.0, 10.0)), icons::Icon::Close, p.text_secondary);
                }
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Alert, true, format!("{}. {}", t.title, t.body)));
                resp
            });
            let resp = resp.inner;
            if resp.hovered() {
                t.born = now - age.min(t.ttl - 1.0);
            }
            if resp.clicked() {
                dismiss = Some(t.id);
            }
            y += h + space::S;
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(d) = dismiss {
            if let Some(t) = self.list.iter_mut().find(|t| t.id == d) {
                t.ttl = (now - t.born) + 0.3;
            }
        }
        self.list.retain(|t| t.born.is_nan() || now - t.born < t.ttl);
    }
}
