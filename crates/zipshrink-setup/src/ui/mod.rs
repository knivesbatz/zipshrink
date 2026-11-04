//! The installer window, after the macOS Installer: a step list on the
//! left, the current step in a content pane, Go Back / Continue below.

use crate::ops::Backend;

/// What the window was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Install,
    Uninstall,
}

/// Opens the installer window and returns the process exit code.
pub fn run(_mode: Mode, _backend: &'static dyn Backend) -> i32 {
    todo!("installer window")
}
