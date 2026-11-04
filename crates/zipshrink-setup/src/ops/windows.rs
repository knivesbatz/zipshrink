//! The real installer actions (Windows only). See mod.rs for the contract.

use super::{Backend, Existing, Plan, Progress};
use std::path::{Path, PathBuf};

pub struct Windows;

impl Backend for Windows {
    fn check_system(&self) -> Result<(), String> {
        todo!("check Windows 10+ 64-bit")
    }
    fn detect(&self) -> Option<Existing> {
        todo!()
    }
    fn default_dir(&self) -> PathBuf {
        todo!()
    }
    fn free_space(&self, _dir: &Path) -> Option<u64> {
        todo!()
    }
    fn app_running(&self, _dir: &Path) -> bool {
        todo!()
    }
    fn wait_closed(&self, _dir: &Path, _secs: u32) -> bool {
        todo!()
    }
    fn install(&self, _plan: &Plan, _progress: Progress) -> Result<(), String> {
        todo!()
    }
    fn uninstall(&self, _dir: &Path, _progress: Progress) -> Result<(), String> {
        todo!()
    }
    fn launch(&self, _dir: &Path) {
        todo!()
    }
    fn is_demo(&self) -> bool {
        false
    }
}
