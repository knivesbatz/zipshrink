//! ZipShrink's installer and uninstaller for Windows.
//!
//! `ZipShrink-setup.exe` opens the installer window; with `/S` it installs
//! silently (what ZipShrink's updater runs) and `/RESTART` then starts
//! ZipShrink again. The same program, copied into the install folder as
//! `Uninstall.exe`, removes ZipShrink (`/S` for silently).
//! Exit codes: 0 done, 1 failed, 2 cancelled, 3 this Windows is not supported.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[path = "../../zipshrink-gui/src/design/mod.rs"]
#[allow(dead_code)]
mod design;
mod ops;
mod payload;
mod ui;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Args {
    silent: bool,
    restart: bool,
    uninstall: bool,
    /// NSIS-style `/D=C:\path` install folder (must be last).
    dir: Option<std::path::PathBuf>,
}

fn args() -> Args {
    let mut a = Args { silent: false, restart: false, uninstall: false, dir: None };
    let exe_is_uninstaller = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case("uninstall")))
        .unwrap_or(false);
    a.uninstall = exe_is_uninstaller;
    for arg in std::env::args().skip(1) {
        let up = arg.to_ascii_uppercase();
        match up.as_str() {
            "/S" | "--SILENT" => a.silent = true,
            "/RESTART" | "--RESTART" => a.restart = true,
            "/UNINSTALL" | "--UNINSTALL" => a.uninstall = true,
            _ if up.starts_with("/D=") => a.dir = Some(std::path::PathBuf::from(&arg[3..])),
            _ => {}
        }
    }
    a
}

fn main() {
    let a = args();
    let backend = ops::backend();
    let code = if a.silent { silent(&a, backend) } else { ui::run(if a.uninstall { ui::Mode::Uninstall } else { ui::Mode::Install }, backend) };
    std::process::exit(code);
}

/// The silent paths, for the updater and for administrators.
fn silent(a: &Args, backend: &'static dyn ops::Backend) -> i32 {
    if let Err(e) = backend.check_system() {
        eprintln!("{e}");
        return 3;
    }
    let existing = backend.detect();
    if a.uninstall {
        let Some(dir) = existing.map(|e| e.dir).or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))) else {
            return 1;
        };
        return match backend.uninstall(&dir, &|_, _| {}) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
    }
    let dir = a.dir.clone().or_else(|| existing.as_ref().map(|e| e.dir.clone())).unwrap_or_else(|| backend.default_dir());
    let components = existing.as_ref().map(|e| e.components).unwrap_or_default();
    // The updater starts setup and then closes ZipShrink: give it a minute.
    backend.wait_closed(&dir, 60);
    match backend.install(&ops::Plan { dir: dir.clone(), components }, &|_, _| {}) {
        Ok(()) => {
            if a.restart {
                backend.launch(&dir);
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
