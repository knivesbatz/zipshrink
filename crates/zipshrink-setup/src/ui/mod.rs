//! The installer window, after the macOS Big Sur Installer: a step list on
//! the left, the current step in a white box on the right, Go Back /
//! Continue below. The uninstaller is a small window of its own. Both are
//! built only from the app's design system (`crate::design`).

mod install;
mod sheet;
mod task;
mod uninstall;

use crate::design::{self, chrome, motion, theme, tokens::Palette};
use crate::ops::Backend;
use eframe::egui::{self, pos2, vec2, Color32, Context, Rect, Ui, ViewportCommand};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// What the window was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Install,
    Uninstall,
}

/// Height of the title bar (traffic lights and the centred title).
const TITLE_H: f32 = 38.0;

/// Exit codes (see main.rs).
const EXIT_DONE: i32 = 0;
const EXIT_FAILED: i32 = 1;
const EXIT_CANCELLED: i32 = 2;
const EXIT_UNSUPPORTED: i32 = 3;

/// Opens the installer window and returns the process exit code.
pub fn run(mode: Mode, backend: &'static dyn Backend) -> i32 {
    let code = Arc::new(AtomicI32::new(EXIT_CANCELLED));
    let (title, size) = match mode {
        Mode::Install => ("Install ZipShrink", install::SIZE),
        Mode::Uninstall => ("Uninstall ZipShrink", uninstall::SIZE),
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_app_id("zipshrink-setup")
        .with_inner_size(size)
        .with_resizable(false)
        .with_maximize_button(false);
    // Like the app: on macOS the content runs under the real traffic
    // lights; elsewhere the window is frameless and draws its own.
    viewport = if cfg!(target_os = "macos") {
        viewport.with_fullsize_content_view(true).with_title_shown(false).with_titlebar_shown(false)
    } else {
        viewport.with_decorations(false)
    };
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../../../assets/icon-256.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let make = |renderer| eframe::NativeOptions { viewport: viewport.clone(), renderer, centered: true, ..Default::default() };
    let start = |opts: eframe::NativeOptions| {
        let code = code.clone();
        eframe::run_native(
            title,
            opts,
            Box::new(move |cc| {
                #[cfg(windows)]
                chrome::round_corners(cc);
                design::fonts::install(&cc.egui_ctx);
                theme::install_style(&cc.egui_ctx);
                let app: Box<dyn eframe::App> = match mode {
                    Mode::Install => Box::new(install::Installer::new(backend, code)),
                    Mode::Uninstall => Box::new(uninstall::Uninstaller::new(backend, code)),
                };
                Ok(app)
            }),
        )
    };
    // The same renderer choice as the app: Direct3D 12 / Vulkan through
    // wgpu first, OpenGL where that fails (ZIPSHRINK_RENDERER=gl swaps them).
    let prefer_gl = std::env::var("ZIPSHRINK_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("gl"));
    let first = if prefer_gl { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    if let Err(e) = start(make(first)) {
        eprintln!("renderer unavailable ({e}); trying the other one");
        let second = if prefer_gl { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow };
        if let Err(e) = start(make(second)) {
            let msg = format!("The ZipShrink installer could not open its window: {e}");
            eprintln!("{msg}");
            let _ = rfd::MessageDialog::new().set_title(title).set_description(msg).set_level(rfd::MessageLevel::Error).show();
            return EXIT_FAILED;
        }
    }
    code.load(Ordering::SeqCst)
}

/// Light, dark or (the default) the system's appearance. The hidden
/// `ZIPSHRINK_SETUP_APPEARANCE=light|dark` forces one, for screenshots.
fn appearance() -> theme::Appearance {
    match std::env::var("ZIPSHRINK_SETUP_APPEARANCE").map(|v| v.to_ascii_lowercase()).as_deref() {
        Ok("dark") => theme::Appearance::Dark,
        Ok("light") => theme::Appearance::Light,
        _ => theme::Appearance::System,
    }
}

/// Switches for the demonstration build, to show states the pretend
/// backend never reaches: `ZIPSHRINK_SETUP_DEMO_UNSUPPORTED`,
/// `ZIPSHRINK_SETUP_DEMO_FAIL` and `ZIPSHRINK_SETUP_DEMO_RUNNING=<seconds>`.
/// They do nothing in a real installer.
fn demo_switch(backend: &dyn Backend, name: &str) -> Option<String> {
    if backend.is_demo() {
        std::env::var(name).ok()
    } else {
        None
    }
}

/// Start of every frame: motion preference and the palette.
fn begin_frame(ctx: &Context) -> Palette {
    motion::set_reduced(ctx, motion::system_prefers_reduced());
    theme::frame(ctx, appearance())
}

/// The window background: one soft material for the title bar and the
/// whole window, as in Installer.app, with the traffic lights and the
/// centred title. The window has a fixed size, so zoom does nothing.
fn window_chrome(ui: &mut Ui, title: &str) {
    let ctx = ui.ctx().clone();
    let p = design::pal(&ctx);
    let full = ui.max_rect();
    let top = design::lerp_color(p.sidebar, Color32::WHITE, if design::is_dark(&p) { 0.03 } else { 0.38 });
    design::gradient_rrect(ui.painter(), full, 0.0, top, p.sidebar);
    let bar = Rect::from_min_size(full.min, vec2(full.width(), TITLE_H));
    chrome::drag_region(ui, bar, "setup-title");
    chrome::traffic_lights(ui, full.min + vec2(20.0, TITLE_H / 2.0));
    if !chrome::NATIVE_CONTROLS {
        // The title with its proxy icon, centred as one group.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let color = if focused { p.text } else { p.text_tertiary };
        let galley = ui.painter().layout_no_wrap(title.to_owned(), design::typ::headline(), color);
        let (icon, gap) = (16.0, 6.0);
        let left = bar.center().x - (icon + gap + galley.size().x) / 2.0;
        let mut faded = ui.new_child(egui::UiBuilder::new().max_rect(bar));
        faded.multiply_opacity(if focused { 1.0 } else { 0.5 });
        chrome::paint_app_icon(&faded, Rect::from_center_size(pos2(left + icon / 2.0, bar.center().y), vec2(icon, icon)));
        let pos = pos2(left + icon + gap, bar.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, color);
    }
    if ctx.input(|i| i.viewport().maximized == Some(true)) {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
    }
}

/// The white (dark: charcoal) box with a hairline edge that holds a step.
fn content_box(ui: &Ui, rect: Rect) {
    let p = design::pal(ui.ctx());
    let edge = if design::is_dark(&p) { Color32::from_white_alpha(22) } else { Color32::from_black_alpha(34) };
    let fill = if design::is_dark(&p) { p.bg } else { Color32::WHITE };
    ui.painter().rect(rect, design::tokens::radius::CARD, fill, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
}

// ------------------------------------------------------------ formatting

/// Sizes the way File Explorer shows them: 1024-based, "KB", "MB", "GB".
fn size(n: u64) -> String {
    if n == 0 {
        return "Zero KB".into();
    }
    if n < 1024 * 1024 {
        return format!("{} KB", n.div_ceil(1024));
    }
    const UNITS: [&str; 4] = ["MB", "GB", "TB", "PB"];
    let mut v = n as f64 / (1024.0 * 1024.0);
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// The disk a path is on, as Windows names it: "C:" or "\\server\share".
fn drive(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return s[..2].to_ascii_uppercase();
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        let mut parts = rest.split('\\').filter(|x| !x.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return format!(r"\\{server}\{share}");
        }
    }
    path.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_else(|| s.into_owned())
}

/// A text with hard line breaks (LICENSE) as flowing paragraphs.
fn reflow(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|para| para.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sizes_read_like_explorer() {
        assert_eq!(size(0), "Zero KB");
        assert_eq!(size(1), "1 KB");
        assert_eq!(size(1536), "2 KB");
        assert_eq!(size(17 * 1024 * 1024 + 400 * 1024), "17.4 MB");
        assert_eq!(size(212 << 30), "212 GB");
    }

    #[test]
    fn drives() {
        assert_eq!(drive(Path::new(r"c:\Users\You\AppData")), "C:");
        assert_eq!(drive(Path::new(r"\\nas\apps\ZipShrink")), r"\\nas\apps");
    }

    #[test]
    fn license_reflows_into_paragraphs() {
        let p = reflow("MIT License\n\nCopyright (c)\nsomeone\n\n\nPermission is\r\nhereby granted");
        assert_eq!(p, vec!["MIT License", "Copyright (c) someone", "Permission is hereby granted"]);
        assert!(reflow(include_str!("../../../../LICENSE")).len() >= 3);
    }
}
