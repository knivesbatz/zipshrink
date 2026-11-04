//! ZipShrink desktop app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod design;
mod instance;
#[cfg(target_os = "macos")]
mod macos;
mod smoke;
mod worker;

use std::path::PathBuf;

fn main() {
    smoke::init();
    let mut sources = Vec::new();
    let mut resume = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--resume" {
            resume = args.next().map(PathBuf::from);
        } else if a.to_string_lossy().starts_with("-psn_") {
            // The process serial number macOS may pass to an app that
            // Finder starts; not a file.
        } else {
            sources.push(PathBuf::from(a));
        }
    }
    let primary = match instance::claim(&sources, resume.as_deref()) {
        instance::Instance::Forwarded => return,
        instance::Instance::Primary(p) => {
            #[cfg(target_os = "macos")]
            if p.is_some() {
                // Only the copy that holds the single-instance lock tidies
                // up after an update and touches its own bundle.
                zipshrink_update::mac_cleanup_leftovers();
                macos::clear_own_quarantine();
            }
            std::sync::Arc::new(std::sync::Mutex::new(p))
        }
    };
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
    // On macOS the Dock shows the bundle's own icon (AppIcon.icns).
    if !cfg!(target_os = "macos") {
        if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png")) {
            viewport = viewport.with_icon(std::sync::Arc::new(icon));
        }
    }
    #[cfg(target_os = "macos")]
    macos::install_early();
    let make = |renderer| eframe::NativeOptions { viewport: viewport.clone(), renderer, ..Default::default() };
    let start = |opts: eframe::NativeOptions, sources: Vec<PathBuf>, resume: Option<PathBuf>| {
        let primary = primary.clone();
        eframe::run_native(
            "ZipShrink",
            opts,
            Box::new(move |cc| {
                #[cfg(windows)]
                design::chrome::round_corners(cc);
                #[cfg(target_os = "macos")]
                {
                    macos::attach(&cc.egui_ctx);
                    macos::install_menu();
                }
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
