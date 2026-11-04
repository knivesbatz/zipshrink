//! The component library, modelled on macOS Big Sur's AppKit controls:
//! push buttons, pop-up buttons, checkboxes, segmented controls, steppers,
//! text fields, forms with right-aligned labels, Finder-style tables,
//! group boxes and progress indicators. Screens are built only from these.

use super::icons::{self, Icon};
use super::motion;
use super::theme::pal;
use super::tokens::{corner, motion as mo, radius, space, typ, Palette};
use eframe::egui::{
    self, epaint::Mesh, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Id, Margin, Pos2, Rect, Response, RichText, Role, Sense,
    Shape, Stroke, StrokeKind, TextEdit, Ui, WidgetInfo,
};

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------- drawing

/// Points around a rounded rectangle (for gradients and custom fills).
fn rrect_points(r: Rect, rad: f32) -> Vec<Pos2> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0);
    let mut pts = Vec::with_capacity(40);
    let corners = [
        (pos2(r.right() - rad, r.top() + rad), -90.0f32),
        (pos2(r.right() - rad, r.bottom() - rad), 0.0),
        (pos2(r.left() + rad, r.bottom() - rad), 90.0),
        (pos2(r.left() + rad, r.top() + rad), 180.0),
    ];
    for (c, start) in corners {
        for k in 0..=8 {
            let a = (start + 90.0 * k as f32 / 8.0).to_radians();
            pts.push(c + vec2(a.cos(), a.sin()) * rad);
        }
    }
    pts
}

/// A rounded rectangle filled with a top-to-bottom gradient (AppKit's
/// default buttons and checkboxes have a slight one).
pub fn gradient_rrect(painter: &egui::Painter, r: Rect, rad: f32, top: Color32, bottom: Color32) {
    let pts = rrect_points(r, rad);
    let mut mesh = Mesh::default();
    let c = r.center();
    let color_at = |y: f32| lerp_color(top, bottom, (y - r.top()) / r.height().max(1.0));
    mesh.colored_vertex(c, color_at(c.y));
    for p in &pts {
        mesh.colored_vertex(*p, color_at(p.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(Shape::mesh(mesh));
}

/// The soft shadow under raised controls.
/// AppKit draws raised controls with a hairline edge all round and a
/// slightly stronger shadow along the bottom.
fn control_shadow(painter: &egui::Painter, r: Rect, rad: f32, dark: bool) {
    let (edge, under) = if dark {
        (Color32::from_black_alpha(90), Color32::from_black_alpha(60))
    } else {
        (Color32::from_black_alpha(36), Color32::from_black_alpha(22))
    };
    painter.rect_filled(r.translate(vec2(0.0, 0.8)), rad, under);
    painter.rect_stroke(r, rad, Stroke::new(1.0, edge), StrokeKind::Outside);
}

pub fn is_dark(p: &Palette) -> bool {
    p.bg.r() < 128
}

// ---------------------------------------------------------------- text

/// A label in `font` and `color`, with the type scale's tracking.
pub fn text(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    let track = typ::tracking(font.size);
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color).extra_letter_spacing(track)))
}

/// Wrapping paragraph text.
pub fn para(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color)).wrap())
}

/// Secondary, smaller explanatory text (AppKit's "small" help text).
pub fn secondary(ui: &mut Ui, s: impl Into<String>) -> Response {
    let c = pal(ui.ctx()).text_secondary;
    para(ui, s, typ::callout(), c)
}

/// Single line that ends in "…" when it does not fit.
pub fn truncated(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color)).truncate())
}

/// Lays out `s` on one line no wider than `max_w`, dropping characters from
/// the start (for paths, whose end matters most) and adding "…".
pub fn elide_start(ui: &Ui, s: &str, font: FontId, max_w: f32) -> String {
    let fits = |t: &str| ui.painter().layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE).size().x <= max_w;
    if fits(s) {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let t: String = std::iter::once('…').chain(chars[mid..].iter().copied()).collect();
        if fits(&t) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    std::iter::once('…').chain(chars[lo.min(chars.len())..].iter().copied()).collect()
}

/// One line elided at the end to `max_w`.
pub fn elide_end(ui: &Ui, s: &str, font: FontId, max_w: f32) -> String {
    let fits = |t: &str| ui.painter().layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE).size().x <= max_w;
    if fits(s) {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let t: String = chars[..mid].iter().copied().chain(std::iter::once('…')).collect();
        if fits(&t) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().copied().chain(std::iter::once('…')).collect()
}

/// A full-width hairline between groups.
pub fn separator(ui: &mut Ui) {
    let p = pal(ui.ctx());
    ui.add_space(space::M);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(1.0, p.separator));
    ui.add_space(space::M);
}

// ---------------------------------------------------------------- focus

/// The keyboard focus ring: AppKit's soft accent halo around the control.
pub fn focus_ring(ui: &Ui, rect: Rect, r: f32) {
    let p = pal(ui.ctx());
    ui.painter().rect_stroke(rect.expand(2.0), r + 2.0, Stroke::new(3.5, p.accent.gamma_multiply(0.45)), StrokeKind::Middle);
}

fn hover_press(ui: &Ui, resp: &Response) -> (f32, f32) {
    let ctx = ui.ctx();
    let h = motion::fade(ctx, resp.id.with("hover"), resp.hovered() && ui.is_enabled(), mo::FAST);
    let p = motion::fade(ctx, resp.id.with("press"), resp.is_pointer_button_down_on(), 0.05);
    (h, p)
}

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The default button (blue): the action Return triggers.
    Primary,
    /// A regular push button.
    Secondary,
    /// A push button whose action deletes or cannot be undone (red text).
    Destructive,
}

pub struct Button<'a> {
    label: &'a str,
    kind: Kind,
    min_width: f32,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, kind: Kind) -> Self {
        Button { label, kind, min_width: 0.0 }
    }
    pub fn primary(label: &'a str) -> Self {
        Self::new(label, Kind::Primary)
    }
    pub fn secondary(label: &'a str) -> Self {
        Self::new(label, Kind::Secondary)
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let p = pal(ui.ctx());
        let dark = is_dark(&p);
        let enabled = ui.is_enabled();
        let font = typ::body();
        let galley = ui.painter().layout_no_wrap(self.label.to_owned(), font, p.text);
        let h = space::CONTROL;
        let w = (galley.size().x + 2.0 * 12.0).max(self.min_width).max(if self.label.len() <= 3 { 64.0 } else { 0.0 });
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, self.label));
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let (_, pt) = hover_press(ui, &resp);
        let r = radius::CONTROL as f32;
        let painter = ui.painter();
        let fg = match self.kind {
            Kind::Primary if enabled => {
                control_shadow(painter, rect, r, dark);
                let top = lerp_color(lerp_color(p.accent, Color32::WHITE, 0.12), p.accent_pressed, pt);
                let bottom = lerp_color(p.accent, p.accent_pressed, pt);
                gradient_rrect(painter, rect, r, top, bottom);
                p.on_accent
            }
            _ => {
                control_shadow(painter, rect, r, dark);
                let base = lerp_color(p.control, p.control_pressed, pt);
                let top = if dark { lerp_color(base, Color32::WHITE, 0.06) } else { base };
                gradient_rrect(painter, rect, r, top, base);
                if dark {
                    painter.rect_stroke(rect, r, Stroke::new(1.0, Color32::from_white_alpha(14)), StrokeKind::Inside);
                }
                match (self.kind, enabled) {
                    (_, false) => p.text_tertiary,
                    (Kind::Destructive, true) => p.red,
                    _ => p.text,
                }
            }
        };
        painter.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, fg);
        if resp.has_focus() {
            focus_ring(ui, rect, r);
        }
        resp
    }
}

/// A borderless toolbar button: an SF-Symbol-style glyph that gains a
/// rounded background on hover, as in Big Sur's unified toolbars.
pub fn toolbar_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    let p = pal(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 28.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label));
    let (ht, pt) = hover_press(ui, &resp);
    let bg = lerp_color(Color32::TRANSPARENT, p.fill.gamma_multiply(0.85), ht);
    let bg = lerp_color(bg, p.fill_pressed, pt);
    ui.painter().rect_filled(rect, 6.0, bg);
    let fg = if ui.is_enabled() { p.text_secondary } else { p.text_tertiary };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(17.0, 17.0)), icon, fg);
    if resp.has_focus() {
        focus_ring(ui, rect, 6.0);
    }
    resp.on_hover_text(label)
}

/// Small round icon button (remove a row, dismiss).
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str, size: f32) -> Response {
    let p = pal(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label));
    let (ht, pt) = hover_press(ui, &resp);
    let fill = lerp_color(p.text_tertiary.gamma_multiply(0.55), p.text_secondary, ht.max(pt));
    ui.painter().circle_filled(rect.center(), size / 2.0, fill);
    icons::paint(ui.painter(), rect.shrink(size * 0.26), icon, Color32::WHITE);
    if resp.has_focus() {
        focus_ring(ui, rect, size / 2.0);
    }
    resp.on_hover_text(label)
}

/// Inline text link.
pub fn link(ui: &mut Ui, label: &str) -> Response {
    let p = pal(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), typ::callout(), p.accent);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Link, ui.is_enabled(), label));
    let ht = motion::fade(ui.ctx(), resp.id.with("hover"), resp.hovered(), mo::FAST);
    ui.painter().galley_with_override_text_color(rect.min, galley, p.accent);
    if ht > 0.0 {
        let y = rect.bottom() - 0.5;
        ui.painter().line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, p.accent.gamma_multiply(ht)));
    }
    if resp.has_focus() {
        focus_ring(ui, rect, 3.0);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------- checkbox

/// An AppKit checkbox with its label; clicking the label toggles it too.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let p = pal(ui.ctx());
    let dark = is_dark(&p);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), typ::body(), p.text);
    let size = vec2(14.0 + 6.0 + galley.size().x, galley.size().y.max(16.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, ui.is_enabled(), *on, label));
    let enabled = ui.is_enabled();
    let b = Rect::from_min_size(pos2(rect.left(), rect.center().y - 7.0), vec2(14.0, 14.0));
    let t = motion::fade(ui.ctx(), resp.id.with("on"), *on, mo::FAST);
    let (_, pt) = hover_press(ui, &resp);
    let painter = ui.painter();
    control_shadow(painter, b, 3.5, dark);
    let off = lerp_color(p.control, p.control_pressed, pt);
    painter.rect_filled(b, 3.5, off);
    if dark {
        painter.rect_stroke(b, 3.5, Stroke::new(1.0, Color32::from_white_alpha(18)), StrokeKind::Inside);
    }
    if t > 0.0 {
        let a = if enabled { 1.0 } else { 0.45 };
        let top = lerp_color(p.accent, Color32::WHITE, 0.12).gamma_multiply(t * a);
        gradient_rrect(painter, b, 3.5, top, p.accent.gamma_multiply(t * a));
        icons::check_stroke(painter, b.shrink(1.0), Color32::WHITE.gamma_multiply(t), 1.8, if motion::reduced(ui.ctx()) { 1.0 } else { t });
    }
    let fg = if enabled { p.text } else { p.text_tertiary };
    painter.galley_with_override_text_color(pos2(b.right() + 6.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    if resp.has_focus() {
        focus_ring(ui, b, 3.5);
    }
    resp
}

// ---------------------------------------------------------------- segmented

/// A segmented control (NSSegmentedControl, "select one").
pub fn segmented(ui: &mut Ui, id_salt: &str, selected: &mut usize, items: &[&str], seg_min: f32) -> Response {
    let p = pal(ui.ctx());
    let font = typ::body();
    let widths: Vec<f32> = items.iter().map(|s| ui.painter().layout_no_wrap((*s).to_owned(), font.clone(), p.text).size().x).collect();
    let seg_w = widths.iter().cloned().fold(seg_min, f32::max) + 22.0;
    let h = space::CONTROL;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(seg_w * items.len() as f32 + 4.0, h), Sense::hover());
    let id = ui.scope_id().with(id_salt);
    let painter = ui.painter().clone();
    let r = radius::CONTROL as f32 + 1.0;
    painter.rect_filled(rect, r, p.fill);
    let x = motion::follow(ui.ctx(), id.with("thumb"), *selected as f32, mo::BASE);
    let thumb = Rect::from_min_size(pos2(rect.left() + 2.0 + x * seg_w, rect.top() + 2.0), vec2(seg_w, h - 4.0));
    painter.rect_filled(thumb.translate(vec2(0.0, 0.5)), r - 1.5, Color32::from_black_alpha(30));
    painter.rect_filled(thumb, r - 1.5, p.thumb);
    for (i, label) in items.iter().enumerate() {
        let sr_rect = Rect::from_min_size(pos2(rect.left() + 2.0 + i as f32 * seg_w, rect.top() + 2.0), vec2(seg_w, h - 4.0));
        let sr = ui.interact(sr_rect, id.with(i), Sense::click());
        let is_sel = *selected == i;
        sr.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), is_sel, *label));
        if sr.clicked() && !is_sel {
            *selected = i;
            resp.mark_changed();
        }
        if i > 0 && (x - i as f32).abs() > 0.9 && (x - (i as f32 - 1.0)).abs() > 0.9 {
            let dx = sr_rect.left();
            painter.line_segment(
                [pos2(dx, sr_rect.top() + 4.0), pos2(dx, sr_rect.bottom() - 4.0)],
                Stroke::new(1.0, p.outline.gamma_multiply(0.6)),
            );
        }
        let color = if ui.is_enabled() { p.text } else { p.text_tertiary };
        painter.text(sr_rect.center(), Align2::CENTER_CENTER, *label, font.clone(), color);
        if sr.has_focus() {
            focus_ring(ui, sr_rect, r - 1.5);
        }
    }
    resp
}

// ---------------------------------------------------------------- pop-up button

/// An NSPopUpButton: a white button with the blue double-chevron cap; a click
/// opens a menu of the choices with a check mark on the current one.
pub fn popup_button(ui: &mut Ui, id_salt: &str, selected: &mut usize, items: &[&str], width: f32) -> Response {
    popup_button_ex(ui, id_salt, selected, items, width, None)
}

pub fn popup_button_ex(ui: &mut Ui, id_salt: &str, selected: &mut usize, items: &[&str], width: f32, icon: Option<Icon>) -> Response {
    let p = pal(ui.ctx());
    let dark = is_dark(&p);
    let label = items.get(*selected).copied().unwrap_or("");
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, space::CONTROL), Sense::click());
    resp.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::ComboBox, ui.is_enabled(), id_salt);
        info.current_text_value = Some(label.to_owned());
        info
    });
    let (_, pt) = hover_press(ui, &resp);
    let r = radius::CONTROL as f32;
    let painter = ui.painter();
    control_shadow(painter, rect, r, dark);
    let base = lerp_color(p.control, p.control_pressed, pt);
    gradient_rrect(painter, rect, r, if dark { lerp_color(base, Color32::WHITE, 0.06) } else { base }, base);
    // The blue cap with the up/down chevrons.
    let cap = Rect::from_min_max(pos2(rect.right() - 19.0, rect.top() + 3.0), pos2(rect.right() - 3.0, rect.bottom() - 3.0));
    let enabled = ui.is_enabled();
    let cap_color = if enabled { p.accent } else { p.text_tertiary };
    gradient_rrect(painter, cap, 3.5, lerp_color(cap_color, Color32::WHITE, 0.14), cap_color);
    let c = cap.center();
    let w = Stroke::new(1.4, Color32::WHITE);
    painter.add(Shape::line(vec![c + vec2(-2.8, -1.6), c + vec2(0.0, -4.2), c + vec2(2.8, -1.6)], w));
    painter.add(Shape::line(vec![c + vec2(-2.8, 1.6), c + vec2(0.0, 4.2), c + vec2(2.8, 1.6)], w));
    let mut tx = rect.left() + 9.0;
    if let Some(ic) = icon {
        icons::paint(painter, Rect::from_center_size(pos2(tx + 7.0, rect.center().y), vec2(15.0, 15.0)), ic, p.accent);
        tx += 21.0;
    }
    let shown = elide_end(ui, label, typ::body(), cap.left() - 6.0 - tx);
    ui.painter().text(pos2(tx, rect.center().y), Align2::LEFT_CENTER, shown, typ::body(), if enabled { p.text } else { p.text_tertiary });
    if resp.has_focus() {
        focus_ring(ui, rect, r);
    }
    let mut changed = false;
    let popup_id = egui::Popup::default_response_id(&resp);
    egui::Popup::menu(&resp).width(width.max(160.0)).frame(popover_frame(&p)).show(|ui| {
        for (i, item) in items.iter().enumerate() {
            if item.is_empty() {
                menu_separator(ui);
                continue;
            }
            if menu_item(ui, item, *selected == i).clicked() {
                *selected = i;
                changed = true;
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
        }
    });
    if changed {
        resp.mark_changed();
    }
    resp
}

/// Menus and popovers: a light material with a soft shadow.
pub fn popover_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.elevated.gamma_multiply(0.98))
        .stroke(Stroke::new(1.0, if is_dark(p) { Color32::from_white_alpha(22) } else { Color32::from_black_alpha(26) }))
        .corner_radius(corner(6))
        .inner_margin(Margin::same(5))
        .shadow(super::tokens::shadow_popover(p))
}

/// A row in a menu: accent highlight with white text on hover, check mark
/// column on the left (NSMenu).
pub fn menu_item(ui: &mut Ui, label: &str, checked: bool) -> Response {
    let p = pal(ui.ctx());
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::MenuItemRadio, true, checked, label));
    let hot = resp.hovered() || resp.has_focus();
    if hot {
        ui.painter().rect_filled(rect, 4.0, p.accent);
    }
    let fg = if hot { p.on_accent } else { p.text };
    if checked {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(11.0, 11.0)), Icon::Check, fg);
    }
    ui.painter().text(pos2(rect.left() + 22.0, rect.center().y), Align2::LEFT_CENTER, label, typ::body(), fg);
    resp
}

pub fn menu_separator(ui: &mut Ui) {
    let p = pal(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().line_segment([pos2(r.left() + 10.0, r.center().y), pos2(r.right() - 10.0, r.center().y)], Stroke::new(1.0, p.separator));
}

// ---------------------------------------------------------------- text field

/// A rounded text field with AppKit's focus ring.
pub fn text_field(ui: &mut Ui, value: &mut String, hint: &str, width: f32, label: &str) -> Response {
    let p = pal(ui.ctx());
    let invalid_id = ui.scope_id().with(label).with("invalid");
    let invalid: bool = ui.ctx().data(|d| d.get_temp(invalid_id)).unwrap_or(false);
    let (rect, _) = ui.allocate_exact_size(vec2(width, space::CONTROL - 2.0), Sense::hover());
    let r = 5.0;
    ui.painter().rect_filled(rect, r, p.field);
    // The editor lives in a child so the row's cursor and spacing are untouched.
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(7.0, 0.0))).layout(egui::Layout::left_to_right(Align::Center)));
    let te = child.add_sized(
        rect.shrink2(vec2(7.0, 0.0)).size(),
        TextEdit::singleline(value)
            .frame(egui::Frame::NONE)
            .align(Align2::LEFT_CENTER)
            .hint_text(RichText::new(hint).color(p.text_tertiary))
            .font(typ::body())
            .text_color(p.text)
            .desired_width(width - 14.0)
            .id_salt(label),
    );
    let edge = if invalid { p.red } else { p.outline };
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0, edge), StrokeKind::Inside);
    // AppKit fields are slightly darker along the top edge.
    ui.painter()
        .line_segment([rect.left_top() + vec2(r, 0.5), rect.right_top() + vec2(-r, 0.5)], Stroke::new(1.0, edge.gamma_multiply(0.5)));
    if te.has_focus() {
        focus_ring(ui, rect, r);
    }
    te
}

/// Marks the field `label` invalid (red outline) on the next frame.
pub fn set_invalid(ui: &Ui, label: &str, invalid: bool) {
    let id = ui.scope_id().with(label).with("invalid");
    ui.ctx().data_mut(|d| d.insert_temp(id, invalid));
}

/// A number field with an NSStepper beside it.
pub fn stepper(ui: &mut Ui, value: &mut usize, min: usize, max: usize, label: &str) -> Response {
    let p = pal(ui.ctx());
    let dark = is_dark(&p);
    let text_id = ui.scope_id().with(label).with("text");
    let mut s: String = ui.ctx().data(|d| d.get_temp(text_id)).unwrap_or_else(|| value.to_string());
    let mut resp = text_field(ui, &mut s, "", 56.0, label);
    if resp.changed() {
        if let Ok(n) = s.trim().parse::<usize>() {
            *value = n.clamp(min, max);
        }
    }
    if !resp.has_focus() {
        s = value.to_string();
    }
    ui.ctx().data_mut(|d| d.insert_temp(text_id, s));
    ui.add_space(-4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(15.0, space::CONTROL - 2.0), Sense::hover());
    control_shadow(ui.painter(), rect, 4.0, dark);
    ui.painter().rect_filled(rect, 4.0, p.control);
    let halves = [
        Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y)),
        Rect::from_min_max(pos2(rect.left(), rect.center().y), rect.max),
    ];
    for (k, half) in halves.iter().enumerate() {
        let up = k == 0;
        let allowed = if up { *value < max } else { *value > min };
        let id = ui.scope_id().with(label).with(k);
        let b = ui.interact(*half, id, Sense::click());
        b.widget_info(|| WidgetInfo::labeled(Role::Button, allowed, if up { format!("More {label}") } else { format!("Fewer {label}") }));
        if b.is_pointer_button_down_on() && allowed {
            ui.painter().rect_filled(*half, 4.0, p.control_pressed);
        }
        let c = half.center() + vec2(0.0, if up { 0.8 } else { -0.8 });
        let dy = if up { -1.0 } else { 1.0 };
        let col = if allowed { p.text } else { p.text_tertiary };
        ui.painter().add(Shape::convex_polygon(
            vec![c + vec2(-3.0, -dy * 1.6), c + vec2(3.0, -dy * 1.6), c + vec2(0.0, dy * 1.8)],
            col,
            Stroke::NONE,
        ));
        if b.clicked() && allowed {
            *value = if up { *value + 1 } else { *value - 1 };
            resp.mark_changed();
            ui.ctx().data_mut(|d| d.insert_temp(text_id, value.to_string()));
        }
    }
    resp
}

// ---------------------------------------------------------------- forms

/// A form row: a right-aligned "Label:" column and the control(s), as in
/// System Preferences. `add` lays out controls and any help text.
pub fn form_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui.ctx());
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let (r, _) = ui.allocate_exact_size(vec2(space::LABEL, space::CONTROL), Sense::hover());
        if !label.is_empty() {
            ui.painter().text(pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, label, typ::body(), p.text);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            add(ui)
        })
        .inner
    })
    .inner
}

/// Help text under a control in a form.
pub fn help(ui: &mut Ui, s: impl Into<String>) {
    let p = pal(ui.ctx());
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(420.0));
        para(ui, s, typ::callout(), p.text_secondary);
    });
}

// ---------------------------------------------------------------- tables

/// A table column: title, width (0 = take the remaining space), alignment.
pub struct Col {
    pub title: &'static str,
    pub width: f32,
    pub right: bool,
}

pub const ROW_H: f32 = 24.0;

/// Draws a Finder-style table header; returns each column's x range.
pub fn table_header(ui: &mut Ui, cols: &[Col]) -> Vec<(f32, f32)> {
    let p = pal(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    let fixed: f32 = cols.iter().map(|c| c.width).sum();
    let flex = (w - fixed).max(60.0);
    let mut x = rect.left();
    let mut ranges = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        let cw = if c.width == 0.0 { flex } else { c.width };
        let (l, r) = (x, x + cw);
        let pad = 10.0;
        let (pos, align) = if c.right {
            (pos2(r - pad, rect.center().y), Align2::RIGHT_CENTER)
        } else {
            (pos2(l + pad, rect.center().y), Align2::LEFT_CENTER)
        };
        ui.painter().text(pos, align, c.title, typ::caption_medium(), p.text_secondary);
        if i > 0 {
            ui.painter().line_segment([pos2(l, rect.top() + 5.0), pos2(l, rect.bottom() - 5.0)], Stroke::new(1.0, p.separator));
        }
        ranges.push((l + pad, r - pad));
        x = r;
    }
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, p.separator));
    ranges
}

/// One table row with alternating background and AppKit's blue selection.
/// Returns the row rect, its response and the primary and secondary text
/// colours to use on it.
pub fn table_row(ui: &mut Ui, index: usize, selected: bool, focused: bool) -> (Rect, Response, Color32, Color32) {
    let p = pal(ui.ctx());
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
    let bg = if selected {
        if focused {
            p.accent
        } else {
            p.selection
        }
    } else if index % 2 == 1 {
        p.stripe
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, if selected { 4.0 } else { 0.0 }, bg);
    let (fg, fg2) = if selected && focused { (p.on_accent, p.on_accent.gamma_multiply(0.85)) } else { (p.text, p.text_secondary) };
    (rect, resp, fg, fg2)
}

/// The bordered "controlBackground" area tables sit in.
pub fn table_frame<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui.ctx());
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0, if is_dark(&p) { Color32::from_white_alpha(20) } else { Color32::from_black_alpha(30) }))
        .corner_radius(corner(radius::CONTROL))
        .inner_margin(Margin::same(1))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

// ---------------------------------------------------------------- reveal

/// Shows `add` with an animated height and fade as `open` changes.
pub fn reveal<R>(ui: &mut Ui, id: Id, open: bool, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let t = motion::slide(ui.ctx(), id.with("reveal"), open, mo::BASE);
    let f = motion::fade(ui.ctx(), id.with("reveal-fade"), open, mo::BASE);
    if t <= 0.0 && f <= 0.0 {
        return None;
    }
    let full_id = id.with("reveal-h");
    let full: f32 = ui.ctx().data(|d| d.get_temp(full_id)).unwrap_or(0.0);
    let avail = ui.available_rect_before_wrap();
    let shown = if t >= 1.0 || full <= 0.0 { None } else { Some(full * t) };
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(avail.min, vec2(avail.width(), 10_000.0))).layout(*ui.layout()));
    if let Some(h) = shown {
        child.set_clip_rect(child.clip_rect().intersect(Rect::from_min_size(avail.min, vec2(avail.width(), h))));
    }
    child.multiply_opacity(f.max(0.0));
    let r = add(&mut child);
    let used = child.min_rect().height();
    ui.ctx().data_mut(|d| d.insert_temp(full_id, used));
    let h = shown.unwrap_or(used);
    ui.allocate_exact_size(vec2(avail.width(), h), Sense::hover());
    Some(r)
}

/// A disclosure button: a chevron that turns as it opens, and its label.
pub fn disclosure(ui: &mut Ui, label: &str, open: &mut bool) -> Response {
    let p = pal(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), typ::body(), p.text);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, space::CONTROL), Sense::click());
    resp.widget_info(|| {
        let mut i = WidgetInfo::labeled(Role::Button, true, label);
        i.selected = Some(*open);
        i
    });
    if resp.clicked() {
        *open = !*open;
        resp.mark_changed();
    }
    let t = motion::slide(ui.ctx(), resp.id.with("turn"), *open, mo::BASE);
    let c = p.text_secondary;
    let center = pos2(rect.left() + 6.0, rect.center().y);
    let a = t * std::f32::consts::FRAC_PI_2;
    let rot = |x: f32, y: f32| center + vec2(x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos());
    ui.painter().add(Shape::line(vec![rot(-1.8, -3.6), rot(2.0, 0.0), rot(-1.8, 3.6)], Stroke::new(1.6, c)));
    ui.painter().galley_with_override_text_color(pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0), galley, p.text);
    if resp.has_focus() {
        focus_ring(ui, rect, 4.0);
    }
    resp
}

// ---------------------------------------------------------------- progress

/// NSProgressIndicator (bar). `None` is indeterminate.
pub fn progress_bar(ui: &mut Ui, id: Id, frac: Option<f32>, height: f32) -> Response {
    let p = pal(ui.ctx());
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    resp.widget_info(|| {
        let mut i = WidgetInfo::new(Role::ProgressIndicator);
        i.value = frac.map(|f| (f * 100.0) as f64);
        i
    });
    let r = height / 2.0;
    let track = if is_dark(&p) { Color32::from_white_alpha(28) } else { Color32::from_black_alpha(22) };
    ui.painter().rect_filled(rect, r, track);
    match frac {
        Some(f) => {
            let shown = motion::follow(ui.ctx(), id.with("progress"), f.clamp(0.0, 1.0), 0.35);
            if shown > 0.0 {
                let bar = Rect::from_min_size(rect.min, vec2((w * shown).max(height), height));
                ui.painter().rect_filled(bar, r, p.accent);
            }
        }
        None => {
            let time = ui.input(|i| i.time) as f32;
            let seg = w * 0.28;
            let phase = if motion::reduced(ui.ctx()) { 0.36 } else { (time * 0.7).fract() };
            let x = rect.left() - seg + (w + seg) * mo::ease_in_out(phase);
            let bar = Rect::from_min_max(pos2(x.max(rect.left()), rect.top()), pos2((x + seg).min(rect.right()), rect.bottom()));
            if bar.width() > 0.0 {
                ui.painter().rect_filled(bar, r, p.accent);
            }
            if !motion::reduced(ui.ctx()) {
                ui.ctx().request_repaint();
            }
        }
    }
    resp
}

/// Asynchronous activity indicator (12 fading spokes).
pub fn spinner(ui: &mut Ui, size: f32) -> Response {
    let p = pal(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::ProgressIndicator, true, "Working"));
    let time = ui.input(|i| i.time) as f32;
    let n = 12;
    let head = if motion::reduced(ui.ctx()) { 0 } else { ((time * 12.0) as usize) % n };
    let c = rect.center();
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let dir = vec2(a.cos(), a.sin());
        let age = (head + n - k) % n;
        let alpha = 1.0 - age as f32 / n as f32 * 0.85;
        ui.painter().line_segment(
            [c + dir * size * 0.24, c + dir * size * 0.47],
            Stroke::new(size * 0.085, p.text_secondary.gamma_multiply(alpha)),
        );
    }
    if !motion::reduced(ui.ctx()) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
    }
    resp
}

/// A storage bar like About This Mac's: coloured segments over a track.
pub fn capacity_bar(ui: &mut Ui, id: Id, total: u64, segments: &[(Color32, u64)]) {
    let p = pal(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    let track = if is_dark(&p) { Color32::from_white_alpha(22) } else { Color32::from_black_alpha(16) };
    ui.painter().rect_filled(rect, 5.0, track);
    let clip = ui.painter().with_clip_rect(rect);
    let mut x = rect.left();
    for (i, (c, n)) in segments.iter().enumerate() {
        let frac = if total == 0 { 0.0 } else { (*n as f64 / total as f64).min(1.0) as f32 };
        let shown = motion::follow(ui.ctx(), id.with(i), frac, 0.4);
        let seg_w = (w * shown).min(rect.right() - x);
        if seg_w > 0.5 {
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(seg_w, rect.height()));
            clip.rect_filled(r, CornerRadius::ZERO, *c);
            clip.line_segment([pos2(r.right(), r.top()), pos2(r.right(), r.bottom())], Stroke::new(1.0, p.bg));
        }
        x += seg_w;
    }
    // Round the ends by painting the background over the corners.
    ui.painter().rect_stroke(rect.expand(0.5), 5.5, Stroke::new(1.5, p.bg), StrokeKind::Inside);
}

pub fn legend(ui: &mut Ui, color: Color32, label: &str) {
    let p = pal(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().rect_filled(r, 2.5, color);
    text(ui, label, typ::callout(), p.text_secondary);
    ui.add_space(space::S);
}

// ---------------------------------------------------------------- status

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

impl Tone {
    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            Tone::Info => p.accent,
            Tone::Success => p.green,
            Tone::Warning => p.orange,
            Tone::Error => p.red,
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            Tone::Info => Icon::Info,
            Tone::Success => Icon::Check,
            Tone::Warning => Icon::Warning,
            Tone::Error => Icon::Close,
        }
    }
}

/// A status line: a small coloured symbol, a title and optional detail —
/// how AppKit apps report state inline (no tinted boxes).
pub fn notice(ui: &mut Ui, tone: Tone, title: &str, body: Option<&str>) {
    let p = pal(ui.ctx());
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 17.0), Sense::hover());
        icons::badge(ui.painter(), pos2(r.center().x, r.top() + 8.5), 7.5, tone.color(&p), tone.icon(), Color32::WHITE);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            para(ui, title, typ::body(), p.text);
            if let Some(b) = body {
                para(ui, b, typ::callout(), p.text_secondary);
            }
        });
    });
}

/// A small status dot.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 3.5, color);
}

/// "Label:  value" lines of a summary, with the label column right-aligned.
pub fn key_value(ui: &mut Ui, key: &str, value: impl Into<String>, color: Option<Color32>) {
    let p = pal(ui.ctx());
    let value = value.into();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let (r, _) = ui.allocate_exact_size(vec2(space::LABEL, 17.0), Sense::hover());
        ui.painter().text(pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, key, typ::body(), p.text_secondary);
        para(ui, value, typ::body(), color.unwrap_or(p.text));
    });
}

/// An empty state: a large gray symbol, a title and a line of guidance.
pub fn empty_state(ui: &mut Ui, icon: Icon, title: &str, body: &str, actions: impl FnOnce(&mut Ui)) {
    let p = pal(ui.ctx());
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
        icons::paint(ui.painter(), r, icon, p.text_tertiary);
        ui.add_space(space::M);
        text(ui, title, typ::title2(), p.text_secondary);
        ui.add_space(space::XS);
        ui.scope(|ui| {
            ui.set_max_width(360.0);
            para(ui, body, typ::body(), p.text_secondary);
        });
        ui.add_space(space::L);
        actions(ui);
    });
}

/// Lays out `add` centred in a horizontal row of about `width`.
pub fn centered_row(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    let avail = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_space(((avail - width) / 2.0).max(0.0));
        add(ui);
    });
}
