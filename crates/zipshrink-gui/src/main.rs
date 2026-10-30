//! ZipShrink desktop app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![allow(clippy::large_enum_variant, clippy::type_complexity)]

mod app;
mod icons;
mod theme;
mod worker;
mod y2k;

use std::path::PathBuf;

fn main() {
    let mut sources = Vec::new();
    let mut resume = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--resume" {
            resume = args.next().map(PathBuf::from);
        } else {
            sources.push(PathBuf::from(a));
        }
    }
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png")).ok();
    // The window draws its own period title bar, so the system one is off.
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("ZipShrink")
        .with_inner_size([720.0, 720.0])
        .with_min_inner_size([640.0, 540.0])
        .with_decorations(false)
        .with_resizable(true)
        .with_drag_and_drop(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let make = |renderer| eframe::NativeOptions { viewport: viewport.clone(), renderer, ..Default::default() };
    let start = |opts: eframe::NativeOptions, sources: Vec<PathBuf>, resume: Option<PathBuf>| {
        eframe::run_native(
            "ZipShrink",
            opts,
            Box::new(move |cc| {
                theme::apply(&cc.egui_ctx);
                Ok(Box::new(app::App::new(&cc.egui_ctx, sources, resume)))
            }),
        )
    };
    let prefer_gl = std::env::var("ZIPSHRINK_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("gl"));
    let first = if prefer_gl { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    // wgpu (Direct3D 12 / Vulkan / Metal) first; OpenGL where it fails.
    if let Err(e) = start(make(first), sources.clone(), resume.clone()) {
        eprintln!("renderer unavailable ({e}); trying the other one");
        let second = if prefer_gl { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow };
        if let Err(e) = start(make(second), sources, resume) {
            fatal(&format!("ZipShrink could not open its window: {e}"));
        }
    }
}

fn fatal(msg: &str) {
    eprintln!("{msg}");
    let _ = rfd::MessageDialog::new().set_title("ZipShrink").set_description(msg).set_level(rfd::MessageLevel::Error).show();
}
