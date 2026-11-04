//! What the installer does to the system. The interface (this file) is
//! shared by the window (`ui`) and the silent mode (`main.rs`); `windows.rs`
//! does the real work and `demo.rs` pretends to, for development builds and
//! non-Windows hosts.
//!
//! Compatibility with the NSIS installer of ZipShrink 0.2-1.0 is part of the
//! contract: same install folder, the same registry values, the same
//! `Uninstall.exe` name, and the same command line (`/S`, `/RESTART`).

use std::path::PathBuf;

#[cfg(windows)]
mod windows;
mod demo;

/// The optional parts. All on by default; an upgrade keeps what the
/// existing installation had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Components {
    pub start_menu: bool,
    pub desktop: bool,
    /// "Compress with ZipShrink…" in File Explorer's right-click menu.
    pub context_menu: bool,
    /// The `zipshrink` command on the user's PATH.
    pub path: bool,
}

impl Default for Components {
    fn default() -> Self {
        Components { start_menu: true, desktop: true, context_menu: true, path: true }
    }
}

/// An installation that is already there.
#[derive(Clone, Debug)]
pub struct Existing {
    /// DisplayVersion from Add/Remove Programs, if known.
    pub version: Option<String>,
    pub dir: PathBuf,
    pub components: Components,
    /// A copy installed from the 0.3 MSI is present and will be removed.
    pub msi: bool,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub dir: PathBuf,
    pub components: Components,
}

/// Progress callback: fraction 0..=1 and a short status ("Copying files…").
pub type Progress<'a> = &'a (dyn Fn(f32, &str) + Sync);

/// The system calls, real or pretend.
pub trait Backend: Send + Sync {
    /// `Err` with a user-facing reason when this Windows cannot run ZipShrink.
    fn check_system(&self) -> Result<(), String>;
    fn detect(&self) -> Option<Existing>;
    fn default_dir(&self) -> PathBuf;
    /// Free bytes on the volume holding `dir` (or its nearest existing parent).
    fn free_space(&self, dir: &std::path::Path) -> Option<u64>;
    /// Whether ZipShrink (gui or cli) is running from `dir`.
    fn app_running(&self, dir: &std::path::Path) -> bool;
    /// Waits up to `secs` for ZipShrink in `dir` to close; true once it has.
    fn wait_closed(&self, dir: &std::path::Path, secs: u32) -> bool;
    fn install(&self, plan: &Plan, progress: Progress) -> Result<(), String>;
    /// Removes the installation in `dir` (files, shortcuts, registry, PATH).
    fn uninstall(&self, dir: &std::path::Path, progress: Progress) -> Result<(), String>;
    /// Starts the installed app (not elevated, detached).
    fn launch(&self, dir: &std::path::Path);
    /// Whether this is the pretend backend.
    fn is_demo(&self) -> bool;
}

/// The backend for this build: real on Windows with a payload, otherwise demo.
pub fn backend() -> &'static dyn Backend {
    #[cfg(windows)]
    if !crate::payload::is_demo() && std::env::var_os("ZIPSHRINK_SETUP_DEMO").is_none() {
        return &windows::Windows;
    }
    &demo::Demo
}
