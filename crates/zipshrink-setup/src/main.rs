//! ZipShrink's installer and uninstaller for Windows.
//!
//! `ZipShrink-setup.exe` opens the installer window; with `/S` it installs
//! silently (what ZipShrink's updater runs) and `/RESTART` then starts
//! ZipShrink again. The same program, copied into the install folder as
//! `Uninstall.exe`, removes ZipShrink (`/S` for silently).
//! Exit codes: 0 done, 1 failed, 2 cancelled, 3 this Windows is not supported.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[path = "../../zipshrink-gui/src/design/mod.rs"]
#[allow(dead_code, unused_imports)]
mod design;
mod ops;
mod payload;
mod ui;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Args {
    silent: bool,
    restart: bool,
    uninstall: bool,
    /// This program is `Uninstall.exe` (in the folder it removes).
    is_uninstaller: bool,
    /// NSIS-style `/D=C:\path` install folder, or for the uninstaller
    /// `_?=C:\path`; either must come last and may contain spaces.
    dir: Option<std::path::PathBuf>,
}

fn args() -> Args {
    let mut a = Args { silent: false, restart: false, uninstall: false, is_uninstaller: false, dir: None };
    a.is_uninstaller = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case("uninstall")))
        .unwrap_or(false);
    a.uninstall = a.is_uninstaller;
    let all: Vec<String> = std::env::args().skip(1).collect();
    for (i, arg) in all.iter().enumerate() {
        let up = arg.to_ascii_uppercase();
        match up.as_str() {
            "/S" | "--SILENT" => a.silent = true,
            "/RESTART" | "--RESTART" => a.restart = true,
            "/UNINSTALL" | "--UNINSTALL" => a.uninstall = true,
            _ if up.starts_with("/D=") || up.starts_with("_?=") => {
                // An unquoted path with spaces arrives as several arguments.
                let rest = std::iter::once(&arg[3..]).chain(all[i + 1..].iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
                let rest = rest.trim().trim_matches('"');
                if !rest.is_empty() {
                    a.dir = Some(std::path::PathBuf::from(rest));
                }
                break;
            }
            _ => {}
        }
    }
    a
}

fn main() {
    let a = args();
    let backend = ops::backend();
    let code = if a.silent {
        // A panic must still end in a failure code the updater understands.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| silent(&a, backend))).unwrap_or_else(|_| {
            note("setup stopped unexpectedly");
            1
        })
    } else {
        ui::run(if a.uninstall { ui::Mode::Uninstall } else { ui::Mode::Install }, backend)
    };
    std::process::exit(code);
}

/// Reports a silent-mode problem: on stderr when there is a console, and in
/// %TEMP%\ZipShrink-setup.log, since the updater shows no window.
fn note(msg: &str) {
    use std::io::Write;
    eprintln!("{msg}");
    let log = std::env::temp_dir().join("ZipShrink-setup.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{when}] ZipShrink setup {VERSION}: {msg}");
    }
}

/// The silent paths, for the updater and for administrators.
/// Exit codes as at the top of this file; nothing is changed before a
/// refusal (3) or when ZipShrink stays open (1).
fn silent(a: &Args, backend: &'static dyn ops::Backend) -> i32 {
    if a.uninstall {
        let own_dir = || std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let dir = a.dir.clone().or_else(|| if a.is_uninstaller { own_dir() } else { None }).or_else(|| backend.detect().map(|e| e.dir));
        let Some(dir) = dir else {
            note("there is no ZipShrink installation to remove");
            return 1;
        };
        if !backend.wait_closed(&dir, 60) {
            note("ZipShrink is still open; nothing was removed");
            return 1;
        }
        return match backend.uninstall(&dir, &|_, _| {}) {
            Ok(()) => 0,
            Err(e) => {
                note(&e);
                1
            }
        };
    }
    if let Err(e) = backend.check_system() {
        note(&e);
        return 3;
    }
    let existing = backend.detect();
    let dir = a.dir.clone().or_else(|| existing.as_ref().map(|e| e.dir.clone())).unwrap_or_else(|| backend.default_dir());
    // An upgrade keeps exactly the optional parts the installation had.
    let components = existing.as_ref().map(|e| e.components).unwrap_or_default();
    note(&match &existing {
        _ if backend.is_demo() => "demonstration build: nothing is installed".to_owned(),
        Some(e) => format!(
            "upgrading ZipShrink {}{}",
            e.version.as_deref().unwrap_or("(unknown version)"),
            if e.msi { ", removing the copy installed from the .msi" } else { "" }
        ),
        None => "installing".to_owned(),
    });
    // New files are written beside the old ones before replacing them.
    let need = payload::installed_size() + (16 << 20);
    if backend.free_space(&dir).is_some_and(|free| free < need) {
        note(&format!("not enough free space in {} ({} MB needed); nothing was changed", dir.display(), need >> 20));
        return 1;
    }
    // The updater starts setup and then closes ZipShrink: give it a minute.
    if backend.app_running(&dir) && !backend.wait_closed(&dir, 60) {
        note("ZipShrink is still open after a minute; nothing was changed");
        return 1;
    }
    let plan = ops::Plan { dir: dir.clone(), components };
    let c = plan.components;
    note(&format!(
        "into {}: Start menu {}, desktop {}, right-click menu {}, PATH {}",
        plan.dir.display(),
        c.start_menu,
        c.desktop,
        c.context_menu,
        c.path
    ));
    match backend.install(&plan, &|_, _| {}) {
        Ok(()) => {
            if a.restart {
                backend.launch(&dir);
            }
            0
        }
        Err(e) => {
            note(&e);
            // The updater closed ZipShrink for this; give the user back the
            // version that is still there.
            if a.restart && existing.is_some() {
                backend.launch(&dir);
            }
            1
        }
    }
}
