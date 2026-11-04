//! ZipShrink desktop app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod design;
mod instance;
mod worker;

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
    let primary = match instance::claim(&sources, resume.as_deref()) {
        instance::Instance::Forwarded => return,
        instance::Instance::Primary(p) => std::sync::Arc::new(std::sync::Mutex::new(p)),
    };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png")).ok();
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("ZipShrink")
        .with_app_id("zipshrink")
        .with_inner_size([980.0, 720.0])
        .with_min_inner_size([620.0, 520.0])
        .with_drag_and_drop(true);
    // Big Sur's unified title bar: on macOS the content runs under the real
    // traffic lights; elsewhere the window is frameless and draws its own.
    viewport = if cfg!(target_os = "macos") {
        viewport.with_fullsize_content_view(true).with_title_shown(false).with_titlebar_shown(false)
    } else {
        viewport.with_decorations(false).with_resizable(true)
    };
    if let Some(icon) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let make = |renderer| eframe::NativeOptions { viewport: viewport.clone(), renderer, ..Default::default() };
    let start = |opts: eframe::NativeOptions, sources: Vec<PathBuf>, resume: Option<PathBuf>| {
        let primary = primary.clone();
        eframe::run_native(
            "ZipShrink",
            opts,
            Box::new(move |cc| {
                #[cfg(windows)]
                design::chrome::round_corners(cc);
                let p = primary.lock().ok().and_then(|mut g| g.take());
                Ok(Box::new(app::App::new(&cc.egui_ctx, sources, resume, p)))
            }),
        )
    };
    let prefer_gl = std::env::var("ZIPSHRINK_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("gl"));
    let first = if prefer_gl { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    // Direct3D 12 / Vulkan through wgpu first; OpenGL where that fails.
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
