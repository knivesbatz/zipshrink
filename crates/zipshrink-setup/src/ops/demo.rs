//! A backend that changes nothing, for development and screenshots.

use super::{Backend, Components, Existing, Plan, Progress};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Demo;

impl Backend for Demo {
    fn check_system(&self) -> Result<(), String> {
        Ok(())
    }
    fn detect(&self) -> Option<Existing> {
        std::env::var_os("ZIPSHRINK_SETUP_DEMO_UPGRADE").map(|_| Existing {
            version: Some("1.0.0".into()),
            dir: self.default_dir(),
            components: Components { desktop: false, ..Components::default() },
            msi: false,
        })
    }
    fn default_dir(&self) -> PathBuf {
        PathBuf::from(r"C:\Users\You\AppData\Local\Programs\ZipShrink")
    }
    fn free_space(&self, _dir: &Path) -> Option<u64> {
        Some(212 << 30)
    }
    fn app_running(&self, _dir: &Path) -> bool {
        false
    }
    fn wait_closed(&self, _dir: &Path, _secs: u32) -> bool {
        true
    }
    fn install(&self, _plan: &Plan, progress: Progress) -> Result<(), String> {
        let steps = ["Preparing…", "Copying ZipShrink…", "Copying the command-line tool…", "Adding shortcuts…", "Registering ZipShrink…"];
        for (i, s) in steps.iter().enumerate() {
            for k in 0..10 {
                progress((i as f32 + k as f32 / 10.0) / steps.len() as f32, s);
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        progress(1.0, "Done");
        Ok(())
    }
    fn uninstall(&self, _dir: &Path, progress: Progress) -> Result<(), String> {
        for k in 0..20 {
            progress(k as f32 / 20.0, "Removing ZipShrink…");
            std::thread::sleep(Duration::from_millis(40));
        }
        progress(1.0, "Done");
        Ok(())
    }
    fn launch(&self, _dir: &Path) {}
    fn is_demo(&self) -> bool {
        true
    }
}
