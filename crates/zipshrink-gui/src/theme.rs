//! A late-90s desktop look, drawn by hand: silver 3D bevels, navy title
//! bars, square corners, no anti-aliasing, and a monochrome-rasterised
//! pixel font drawn at its design size so every glyph lands on the pixel
//! grid, the way text looked before font smoothing.

use eframe::egui::{
    self, pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke, TextStyle, Ui, Vec2, Visuals,
};

pub const FACE: Color32 = Color32::from_rgb(192, 192, 192);
pub const HI: Color32 = Color32::WHITE;
pub const LIGHT: Color32 = Color32::from_rgb(223, 223, 223);
pub const SHADOW: Color32 = Color32::from_rgb(128, 128, 128);
pub const DARK: Color32 = Color32::BLACK;
pub const WINDOW: Color32 = Color32::WHITE;
pub const TEXT: Color32 = Color32::BLACK;
pub const GRAY_TEXT: Color32 = Color32::from_rgb(128, 128, 128);
pub const NAVY: Color32 = Color32::from_rgb(0, 0, 128);
pub const TITLE_END: Color32 = Color32::from_rgb(16, 132, 208);
pub const INACTIVE: Color32 = Color32::from_rgb(128, 128, 128);
pub const INACTIVE_END: Color32 = Color32::from_rgb(181, 181, 181);
pub const TEAL: Color32 = Color32::from_rgb(0, 128, 128);
pub const RED: Color32 = Color32::from_rgb(170, 0, 0);
pub const GREEN: Color32 = Color32::from_rgb(0, 112, 0);
pub const OLIVE: Color32 = Color32::from_rgb(128, 96, 0);
pub const PURPLE: Color32 = Color32::from_rgb(112, 0, 128);
pub const BLUE: Color32 = Color32::from_rgb(0, 0, 192);
pub const TOOLTIP: Color32 = Color32::from_rgb(255, 255, 225);

/// Design size of the pixel font (its line height in pixels).
pub const PX: f32 = 13.0;

pub fn bold() -> FontId {
    FontId::new(PX, FontFamily::Name("bold".into()))
}
pub fn regular() -> FontId {
    FontId::new(PX, FontFamily::Proportional)
}

pub fn apply(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    ctx.add_font(FontInsert::new(
        "pixel",
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/ZipShrinkPixel-Regular.ttf")),
        vec![
            InsertFontFamily { family: FontFamily::Proportional, priority: FontPriority::Highest },
            InsertFontFamily { family: FontFamily::Monospace, priority: FontPriority::Highest },
            InsertFontFamily { family: FontFamily::Name("bold".into()), priority: FontPriority::Lowest },
        ],
    ));
    ctx.add_font(FontInsert::new(
        "pixel-bold",
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/ZipShrinkPixel-Bold.ttf")),
        vec![InsertFontFamily { family: FontFamily::Name("bold".into()), priority: FontPriority::Highest }],
    ));
    // Crisp, aliased edges everywhere.
    ctx.tessellation_options_mut(|t| t.feathering = false);

    let mut v = Visuals::light();
    v.panel_fill = FACE;
    v.window_fill = FACE;
    v.extreme_bg_color = WINDOW;
    v.faint_bg_color = FACE;
    v.code_bg_color = WINDOW;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = BLUE;
    v.selection.bg_fill = NAVY;
    v.selection.stroke = Stroke::new(1.0, HI);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, DARK);
    v.text_cursor.stroke = Stroke::new(1.0, DARK);
    v.text_cursor.blink = true;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::ZERO;
        w.bg_fill = FACE;
        w.weak_bg_fill = FACE;
        w.bg_stroke = Stroke::new(1.0, SHADOW);
        w.fg_stroke = Stroke::new(1.0, TEXT);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SHADOW);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, DARK);
    v.widgets.active.bg_fill = LIGHT;
    v.widgets.active.weak_bg_fill = LIGHT;
    v.widgets.open.bg_fill = NAVY;
    v.widgets.open.weak_bg_fill = NAVY;
    v.widgets.open.fg_stroke = Stroke::new(1.0, HI);
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        for (style, id) in [
            (TextStyle::Body, regular()),
            (TextStyle::Button, regular()),
            (TextStyle::Small, regular()),
            (TextStyle::Monospace, FontId::new(PX, FontFamily::Monospace)),
            (TextStyle::Heading, bold()),
        ] {
            s.text_styles.insert(style, id);
        }
        s.spacing.item_spacing = vec2(6.0, 5.0);
        s.spacing.button_padding = vec2(8.0, 4.0);
        s.spacing.interact_size = vec2(20.0, 21.0);
        s.spacing.menu_margin = egui::Margin::same(3);
        s.spacing.window_margin = egui::Margin::same(3);
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        s.spacing.scroll.bar_width = 16.0;
        s.spacing.scroll.handle_min_length = 16.0;
        s.spacing.scroll.bar_inner_margin = 0.0;
        s.spacing.scroll.bar_outer_margin = 0.0;
        s.spacing.scroll.foreground_color = false;
        s.animation_time = 0.0;
        s.interaction.tooltip_delay = 0.4;
    });
}

/// Keeps the scale an integer multiple of the pixel grid, so the bitmap
/// font and the bevels stay sharp (1x at 100-124 % display scaling, 2x
/// from 150 %, as programs of the era did).
pub fn snap_scale(ctx: &egui::Context) {
    let native = ctx.native_pixels_per_point().unwrap_or(1.0);
    let want = native.round().max(1.0);
    if (ctx.pixels_per_point() - want).abs() > 0.01 {
        ctx.set_pixels_per_point(want);
    }
}

fn hline(p: &egui::Painter, x0: f32, x1: f32, y: f32, c: Color32) {
    p.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1, y + 1.0)), 0.0, c);
}
fn vline(p: &egui::Painter, x: f32, y0: f32, y1: f32, c: Color32) {
    p.rect_filled(Rect::from_min_max(pos2(x, y0), pos2(x + 1.0, y1)), 0.0, c);
}

/// Draws a two-tone 1px frame: `tl` on the top and left, `br` on the bottom and right.
pub fn edge(p: &egui::Painter, r: Rect, tl: Color32, br: Color32) {
    let r = Rect::from_min_max(r.min.round(), r.max.round());
    hline(p, r.left(), r.right() - 1.0, r.top(), tl);
    vline(p, r.left(), r.top(), r.bottom() - 1.0, tl);
    hline(p, r.left(), r.right(), r.bottom() - 1.0, br);
    vline(p, r.right() - 1.0, r.top(), r.bottom(), br);
}

#[derive(Clone, Copy)]
pub enum Bevel {
    /// Buttons and window frames.
    Raised,
    /// Pressed buttons.
    Pressed,
    /// Text boxes, list boxes, progress bars.
    Field,
    /// Status bar panes.
    Thin,
    /// Group boxes and separators.
    Etched,
}

pub fn bevel(p: &egui::Painter, r: Rect, kind: Bevel) {
    let inner = r.shrink(1.0);
    match kind {
        Bevel::Raised => {
            edge(p, r, HI, DARK);
            edge(p, inner, LIGHT, SHADOW);
        }
        Bevel::Pressed => {
            edge(p, r, DARK, HI);
            edge(p, inner, SHADOW, LIGHT);
        }
        Bevel::Field => {
            edge(p, r, SHADOW, HI);
            edge(p, inner, DARK, LIGHT);
        }
        Bevel::Thin => edge(p, r, SHADOW, HI),
        Bevel::Etched => {
            edge(p, Rect::from_min_max(r.min, r.max - vec2(1.0, 1.0)), SHADOW, SHADOW);
            edge(p, Rect::from_min_max(r.min + vec2(1.0, 1.0), r.max), HI, HI);
        }
    }
}

fn text_size(ui: &Ui, text: &str, font: FontId) -> Vec2 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, TEXT).size())
}

/// Paints `text` like a 90s disabled label when `enabled` is false:
/// grey with a white emboss.
pub fn paint_text(p: &egui::Painter, at: Pos2, text: &str, font: FontId, color: Color32, enabled: bool) {
    let at = at.round();
    if enabled {
        p.text(at, egui::Align2::LEFT_TOP, text, font, color);
    } else {
        p.text(at + vec2(1.0, 1.0), egui::Align2::LEFT_TOP, text, font.clone(), HI);
        p.text(at, egui::Align2::LEFT_TOP, text, font, GRAY_TEXT);
    }
}

/// Draws a dotted focus rectangle.
pub fn focus_rect(p: &egui::Painter, r: Rect) {
    let r = Rect::from_min_max(r.min.round(), r.max.round());
    let mut x = r.left();
    while x < r.right() {
        p.rect_filled(Rect::from_min_size(pos2(x, r.top()), vec2(1.0, 1.0)), 0.0, DARK);
        p.rect_filled(Rect::from_min_size(pos2(x, r.bottom() - 1.0), vec2(1.0, 1.0)), 0.0, DARK);
        x += 2.0;
    }
    let mut y = r.top();
    while y < r.bottom() {
        p.rect_filled(Rect::from_min_size(pos2(r.left(), y), vec2(1.0, 1.0)), 0.0, DARK);
        p.rect_filled(Rect::from_min_size(pos2(r.right() - 1.0, y), vec2(1.0, 1.0)), 0.0, DARK);
        y += 2.0;
    }
}

/// A push button. `default` draws the heavy black border of the default button.
pub fn button_ex(ui: &mut Ui, text: &str, enabled: bool, default: bool, min_width: f32) -> Response {
    let ts = text_size(ui, text, regular());
    let size = vec2((ts.x + 24.0).max(min_width).round(), 23.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, FACE);
        let mut r = rect;
        if default {
            edge(p, r, DARK, DARK);
            r = r.shrink(1.0);
        }
        let pressed = enabled && resp.is_pointer_button_down_on();
        bevel(p, r, if pressed { Bevel::Pressed } else { Bevel::Raised });
        let off = if pressed { vec2(1.0, 1.0) } else { Vec2::ZERO };
        let at = rect.center() - ts / 2.0 + off;
        paint_text(p, at, text, regular(), TEXT, enabled);
        if enabled && (resp.has_focus() || (default && resp.hovered())) {
            focus_rect(p, r.shrink(3.0));
        }
    }
    resp
}

pub fn button(ui: &mut Ui, text: &str) -> Response {
    button_ex(ui, text, true, false, 75.0)
}

/// A classic check box: sunken white square with a black tick.
pub fn checkbox(ui: &mut Ui, value: &mut bool, text: &str, enabled: bool) -> Response {
    let ts = text_size(ui, text, regular());
    let size = vec2(13.0 + 6.0 + ts.x + 4.0, ts.y.max(13.0) + 2.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let b = Rect::from_min_size(pos2(rect.left(), (rect.center().y - 6.5).round()), vec2(13.0, 13.0));
        let pressed = resp.is_pointer_button_down_on();
        p.rect_filled(b, 0.0, if pressed || !enabled { FACE } else { WINDOW });
        bevel(p, b, Bevel::Field);
        if *value {
            // The 7x7 tick, drawn as 3px-tall pixel columns.
            let o = b.min + vec2(3.0, 3.0);
            for (i, y) in [2.0, 3.0, 4.0, 3.0, 2.0, 1.0, 0.0].iter().enumerate() {
                p.rect_filled(Rect::from_min_size(o + vec2(i as f32, *y), vec2(1.0, 3.0)), 0.0, if enabled { DARK } else { SHADOW });
            }
        }
        let at = pos2(b.right() + 6.0, rect.center().y - ts.y / 2.0);
        paint_text(p, at, text, regular(), TEXT, enabled);
        if resp.has_focus() {
            focus_rect(p, Rect::from_min_size(at - vec2(2.0, 1.0), ts + vec2(4.0, 2.0)));
        }
    }
    resp
}

/// A group box: etched frame with its caption sitting in the top edge.
pub fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let ir = egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 10, top: 18, bottom: 10 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let r = ir.response.rect;
    let p = ui.painter();
    let frame = Rect::from_min_max(r.min + vec2(0.0, 6.0), r.max);
    bevel(p, frame, Bevel::Etched);
    let ts = text_size(ui, title, regular());
    let cap = Rect::from_min_size(r.min + vec2(7.0, 0.0), vec2(ts.x + 4.0, ts.y));
    p.rect_filled(cap, 0.0, FACE);
    paint_text(p, cap.min + vec2(2.0, 0.0), title, regular(), TEXT, true);
    ir.inner
}

/// A sunken white box (list views, read-only fields).
pub fn field<R>(ui: &mut Ui, fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let ir = egui::Frame::new().fill(fill).inner_margin(egui::Margin::same(3)).show(ui, add);
    bevel(ui.painter(), ir.response.rect, Bevel::Field);
    ir.inner
}

/// A segmented progress bar, filled with navy blocks.
pub fn progress(ui: &mut Ui, frac: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width.round(), 18.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, FACE);
    bevel(p, rect, Bevel::Thin);
    let inner = rect.shrink(2.0);
    let fill_w = (inner.width() * frac.clamp(0.0, 1.0)).floor();
    let mut x = inner.left();
    while x + 1.0 < inner.left() + fill_w {
        let w = 8.0f32.min(inner.left() + fill_w - x);
        p.rect_filled(Rect::from_min_size(pos2(x, inner.top()), vec2(w, inner.height())), 0.0, NAVY);
        x += 10.0;
    }
}

/// A title bar gradient strip with bold white caption.
pub fn title_strip(ui: &mut Ui, rect: Rect, text: &str, active: bool) {
    let (a, b) = if active { (NAVY, TITLE_END) } else { (INACTIVE, INACTIVE_END) };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), a);
    mesh.colored_vertex(rect.right_top(), b);
    mesh.colored_vertex(rect.right_bottom(), b);
    mesh.colored_vertex(rect.left_bottom(), a);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
    let icon = Rect::from_min_size(pos2(rect.left() + 2.0, (rect.center().y - 8.0).round()), vec2(16.0, 16.0));
    crate::icons::paint(ui.painter(), icon.min, crate::icons::APP, 1.0);
    ui.painter().text(pos2(icon.right() + 3.0, (rect.center().y - PX / 2.0).round()), egui::Align2::LEFT_TOP, text, bold(), HI);
}

/// The small square buttons on a title bar (minimise, maximise, close).
pub fn caption_button(ui: &mut Ui, rect: Rect, glyph: u8) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with(("caption", glyph)), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, FACE);
    let pressed = resp.is_pointer_button_down_on();
    bevel(p, rect, if pressed { Bevel::Pressed } else { Bevel::Raised });
    let o = rect.min + vec2(4.0, 3.0) + if pressed { vec2(1.0, 1.0) } else { Vec2::ZERO };
    let px = |x: f32, y: f32, w: f32, h: f32| {
        p.rect_filled(Rect::from_min_size(o + vec2(x, y), vec2(w, h)), 0.0, DARK);
    };
    match glyph {
        b'_' => px(1.0, 6.0, 6.0, 2.0),
        b'o' => {
            px(0.0, 0.0, 9.0, 2.0);
            px(0.0, 0.0, 1.0, 7.0);
            px(8.0, 0.0, 1.0, 7.0);
            px(0.0, 6.0, 9.0, 1.0);
        }
        _ => {
            for i in 0..7 {
                let f = i as f32;
                px(f + 1.0, f, 2.0, 1.0);
                px(6.0 - f + 1.0, f, 2.0, 1.0);
            }
        }
    }
    resp
}

/// Colour for an entry status in a list view.
pub fn status_color(s: &zipshrink_core::EntryStatus) -> Color32 {
    use zipshrink_core::EntryStatus::*;
    match s {
        Waiting => GRAY_TEXT,
        Compressing => BLUE,
        Verifying => PURPLE,
        Archived => TEAL,
        Deleted => GREEN,
        Kept(_) | Skipped(_) => OLIVE,
        Failed(_) => RED,
        Recoverable => BLUE,
    }
}
