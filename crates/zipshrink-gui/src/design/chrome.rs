//! Window chrome in the Big Sur style. On macOS the system draws the real
//! traffic lights over the full-size content view; elsewhere the window has
//! no system frame and this module draws them, makes the unified title bar
//! draggable (double-click zooms) and adds resize edges. On Windows 11 the
//! window also gets DWM's rounded corners and shadow.

use super::theme::pal;
use super::widgets::is_dark;
use eframe::egui::{
    self, pos2, vec2, Color32, Context, CursorIcon, Id, Rect, ResizeDirection, Sense, Stroke, TextureHandle, Ui, ViewportCommand,
};

/// Whether the operating system draws the window controls itself.
pub const NATIVE_CONTROLS: bool = cfg!(target_os = "macos");

/// The three window controls at `origin` (centre of the first light).
pub fn traffic_lights(ui: &mut Ui, origin: egui::Pos2) {
    if NATIVE_CONTROLS {
        return;
    }
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    let group = Rect::from_min_max(origin - vec2(8.0, 8.0), origin + vec2(48.0, 8.0));
    let hover_group = ui.rect_contains_pointer(group);
    let lights: [(Color32, Color32, &str, u8); 3] = [
        (Color32::from_rgb(0xFF, 0x5F, 0x57), Color32::from_rgb(0xE2, 0x46, 0x3F), "Close", 0),
        (Color32::from_rgb(0xFE, 0xBC, 0x2E), Color32::from_rgb(0xE1, 0xA1, 0x16), "Minimize", 1),
        (Color32::from_rgb(0x28, 0xC8, 0x40), Color32::from_rgb(0x1A, 0xAB, 0x29), "Zoom", 2),
    ];
    for (i, (fill, edge, name, kind)) in lights.into_iter().enumerate() {
        let c = origin + vec2(20.0 * i as f32, 0.0);
        let r = Rect::from_center_size(c, vec2(14.0, 14.0));
        let resp = ui.interact(r, Id::unique(("zs.light", i)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
        let (fill, edge) = if focused || hover_group {
            (fill, edge)
        } else if is_dark(&p) {
            (Color32::from_gray(0x55), Color32::from_gray(0x4A))
        } else {
            (Color32::from_gray(0xDA), Color32::from_gray(0xC8))
        };
        let fill = if resp.is_pointer_button_down_on() { fill.lerp_to_gamma(Color32::BLACK, 0.18) } else { fill };
        ui.painter().circle_filled(c, 6.0, fill);
        ui.painter().circle_stroke(c, 5.75, Stroke::new(0.5, edge));
        if hover_group {
            let g = Color32::from_black_alpha(150);
            let s = Stroke::new(1.1, g);
            match kind {
                0 => {
                    ui.painter().line_segment([c + vec2(-2.4, -2.4), c + vec2(2.4, 2.4)], s);
                    ui.painter().line_segment([c + vec2(2.4, -2.4), c + vec2(-2.4, 2.4)], s);
                }
                1 => {
                    ui.painter().line_segment([c + vec2(-3.0, 0.0), c + vec2(3.0, 0.0)], s);
                }
                _ => {
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![c + vec2(-2.8, -2.8), c + vec2(1.4, -2.8), c + vec2(-2.8, 1.4)],
                        g,
                        Stroke::NONE,
                    ));
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![c + vec2(2.8, 2.8), c + vec2(-1.4, 2.8), c + vec2(2.8, -1.4)],
                        g,
                        Stroke::NONE,
                    ));
                }
            }
        }
        if resp.has_focus() {
            super::widgets::focus_ring(ui, r.shrink(1.0), 6.0);
        }
        if resp.clicked() {
            match kind {
                0 => ctx.send_viewport_cmd(ViewportCommand::Close),
                1 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                _ => zoom(&ctx),
            }
        }
    }
}

fn zoom(ctx: &Context) {
    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
}

/// Makes `rect` (empty title-bar space) move the window; double-click zooms.
/// Call before drawing controls into that space so they take precedence.
pub fn drag_region(ui: &mut Ui, rect: Rect, salt: &str) {
    let resp = ui.interact(rect, Id::unique(("zs.drag", salt)), Sense::click_and_drag());
    if resp.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        title_double_click(ui.ctx());
    }
}

/// Double-click on the title bar: zoom, or on macOS whatever Desktop & Dock
/// settings ask for.
fn title_double_click(ctx: &Context) {
    #[cfg(target_os = "macos")]
    match crate::macos::title_double_click() {
        crate::macos::TitleDoubleClick::Zoom => zoom(ctx),
        crate::macos::TitleDoubleClick::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
        crate::macos::TitleDoubleClick::Nothing => {}
    }
    #[cfg(not(target_os = "macos"))]
    zoom(ctx);
}

/// Invisible edges and corners that resize a frameless window.
pub fn resize_edges(ctx: &Context) {
    if NATIVE_CONTROLS || ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let r = ctx.content_rect();
    let e = 5.0;
    let c = 12.0;
    let zones = [
        (Rect::from_min_max(r.min, pos2(r.left() + c, r.top() + c)), ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (
            Rect::from_min_max(pos2(r.right() - c, r.top()), pos2(r.right(), r.top() + c)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_max(pos2(r.left(), r.bottom() - c), pos2(r.left() + c, r.bottom())),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (Rect::from_min_max(r.max - vec2(c, c), r.max), ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
        (
            Rect::from_min_max(pos2(r.left() + c, r.top()), pos2(r.right() - c, r.top() + e)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(pos2(r.left() + c, r.bottom() - e), pos2(r.right() - c, r.bottom())),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(pos2(r.left(), r.top() + c), pos2(r.left() + e, r.bottom() - c)),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(pos2(r.right() - e, r.top() + c), pos2(r.right(), r.bottom() - c)),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
    ];
    egui::Area::new(Id::unique("zs.resize")).order(egui::Order::Foreground).fixed_pos(r.min).show(ctx, |ui| {
        for (i, (zone, dir, cursor)) in zones.into_iter().enumerate() {
            let resp = ui.interact(zone, Id::unique(("zs.resize", i)), Sense::drag());
            if resp.hovered() {
                ctx.set_cursor_icon(cursor);
            }
            if resp.drag_started() {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        }
    });
}

/// The app icon as a texture (for alerts, notifications and About).
pub fn app_icon(ctx: &Context) -> Option<TextureHandle> {
    let id = Id::unique("zs.app-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(t);
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../../assets/icon-256.png")).ok()?;
    let img = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    let tex = ctx.load_texture("app-icon", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

pub fn paint_app_icon(ui: &Ui, rect: Rect) {
    if let Some(t) = app_icon(ui.ctx()) {
        ui.painter().image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// Windows 11: DWM's rounded corners and shadow for the frameless window.
#[cfg(windows)]
pub fn round_corners(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    let Ok(handle) = cc.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let pref: i32 = DWMWCP_ROUND;
    let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
    // SAFETY: hwnd is this process's live top-level window; both calls only
    // read the values passed by pointer. Failure (Windows 10) is harmless.
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&pref as *const i32).cast(), 4);
        DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}
