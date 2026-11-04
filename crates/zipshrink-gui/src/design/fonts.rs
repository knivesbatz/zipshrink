//! Typography, following Big Sur: SF Pro Text for the interface, SF Pro
//! Display for large headings and SF Mono for fixed-width text, when the
//! user has those fonts installed (Apple's licence does not allow shipping
//! them). Otherwise the bundled Inter and Inter Display, which are drawn in
//! the same spirit, and egui's monospace font. Characters neither has
//! (CJK, Arabic, ...) come from the system's own fonts through eframe's
//! system-font fallback.

use super::tokens::typ;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};
use std::path::PathBuf;
use std::sync::Arc;

static INTER_REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.otf");
static INTER_MEDIUM: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Medium.otf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../../assets/fonts/Inter-SemiBold.otf");
static INTER_DISPLAY: &[u8] = include_bytes!("../../../../assets/fonts/InterDisplay-SemiBold.otf");

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        if let Some(w) = env("WINDIR") {
            dirs.push(w.join("Fonts"));
        }
        if let Some(l) = env("LOCALAPPDATA") {
            dirs.push(l.join("Microsoft").join("Windows").join("Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push("/Library/Fonts".into());
        if let Some(h) = env("HOME") {
            dirs.push(h.join("Library/Fonts"));
        }
    } else {
        if let Some(h) = env("HOME") {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
        dirs.push("/usr/share/fonts/opentype/sf-pro".into());
        dirs.push("/usr/local/share/fonts".into());
    }
    dirs
}

fn find(names: &[&str]) -> Option<Vec<Vec<u8>>> {
    for dir in font_dirs() {
        let read: Vec<Option<Vec<u8>>> = names.iter().map(|n| std::fs::read(dir.join(n)).ok()).collect();
        if read.iter().all(Option::is_some) {
            return Some(read.into_iter().flatten().collect());
        }
    }
    None
}

/// Installs the fonts and the text styles egui's own widgets use.
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let text = find(&["SF-Pro-Text-Regular.otf", "SF-Pro-Text-Medium.otf", "SF-Pro-Text-Semibold.otf"]);
    let [regular, medium, semibold] = match text {
        Some(v) if v.len() == 3 => {
            let mut it = v.into_iter().map(FontData::from_owned);
            [
                it.next().unwrap_or(FontData::from_static(INTER_REGULAR)),
                it.next().unwrap_or(FontData::from_static(INTER_MEDIUM)),
                it.next().unwrap_or(FontData::from_static(INTER_SEMIBOLD)),
            ]
        }
        _ => [FontData::from_static(INTER_REGULAR), FontData::from_static(INTER_MEDIUM), FontData::from_static(INTER_SEMIBOLD)],
    };
    let display = find(&["SF-Pro-Display-Semibold.otf"])
        .and_then(|mut v| v.pop())
        .map(FontData::from_owned)
        .unwrap_or(FontData::from_static(INTER_DISPLAY));
    for (name, data) in [(typ::REGULAR, regular), (typ::MEDIUM, medium), (typ::SEMIBOLD, semibold), (typ::DISPLAY, display)] {
        defs.font_data.insert(name.to_owned(), Arc::new(data));
    }
    if let Some(mut mono) = find(&["SF-Mono-Regular.otf"]) {
        if let Some(m) = mono.pop() {
            defs.font_data.insert("sf-mono".into(), Arc::new(FontData::from_owned(m)));
            defs.families.entry(FontFamily::Monospace).or_default().insert(0, "sf-mono".into());
        }
    }
    // egui's bundled fonts stay as fallbacks for symbols.
    let fallbacks: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for weight in [typ::REGULAR, typ::MEDIUM, typ::SEMIBOLD, typ::DISPLAY] {
        let mut list = vec![weight.to_owned()];
        if weight != typ::REGULAR {
            list.push(typ::REGULAR.to_owned());
        }
        list.extend(fallbacks.iter().cloned());
        defs.families.insert(FontFamily::Name(weight.into()), list);
    }
    let mut prop = vec![typ::REGULAR.to_owned()];
    prop.extend(fallbacks);
    defs.families.insert(FontFamily::Proportional, prop);
    ctx.set_fonts(defs);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, typ::title()),
            (TextStyle::Body, typ::body()),
            (TextStyle::Button, typ::body_medium()),
            (TextStyle::Small, typ::caption()),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
    });
}
