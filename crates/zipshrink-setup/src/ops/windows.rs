//! The real installer actions (Windows only). See mod.rs for the contract
//! and the NSIS script this replaces (packaging/windows/zipshrink.nsi in
//! git history up to 1.0):
//! the folder, registry values, shortcut names and command line match it,
//! so an upgrade over an NSIS-installed ZipShrink (0.2 to 1.0) is seamless
//! and Add/Remove Programs keeps working.
//!
//! Everything is per user (HKCU, %LOCALAPPDATA%), so no administrator
//! rights are needed. Uninstalling removes only what an installer of
//! ZipShrink put there: known file names, shortcuts that start our
//! program, menu entries that run it, and our own PATH entry.

use super::winpath;
use super::{Backend, Components, Existing, Plan, Progress};
use crate::payload;
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, MoveFileExW, FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Registry::REG_EXPAND_SZ;
use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS};
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Programs};

mod reg;
mod shell;

const APP_KEY: &str = r"Software\ZipShrink";
const UNINST_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ZipShrink";
const MENU_KEYS: [&str; 2] = [r"Software\Classes\*\shell\ZipShrink", r"Software\Classes\Directory\shell\ZipShrink"];
const ENV_KEY: &str = "Environment";
/// The Windows Installer packages of versions 0.1 to 0.3 (installed for all users).
const MSI_UPGRADE_CODE: &str = "{6B0E2F9A-3C41-4E7B-9F0D-2A5C8E1B7D34}";
const GUI: &str = "zipshrink-gui.exe";
const CLI: &str = "zipshrink.exe";
const UNINSTALLER: &str = "Uninstall.exe";
const LINK: &str = "ZipShrink.lnk";
/// Files earlier installers put in the folder: 0.2-1.0 (NSIS) and 0.1.
const KNOWN_FILES: [&str; 7] = [GUI, CLI, "LICENSE.txt", "THIRD_PARTY_LICENSES.txt", "README.txt", "LICENSE", "THIRD_PARTY_LICENSES.md"];
const LEGACY_FILES: [&str; 2] = ["LICENSE", "THIRD_PARTY_LICENSES.md"];
/// A new file is written as `<name>` + this, then moved over `<name>`.
const NEW_SUFFIX: &str = ".zipshrink-new";
const STILL_OPEN: &str = "ZipShrink is open. Quit ZipShrink, then try again.";

pub struct Windows;

/// A NUL-terminated UTF-16 copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// SAFETY: `p` must point to a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: *const u16) -> String {
    let mut n = 0;
    // SAFETY: the caller promises a terminator.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: n characters were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

/// `%VARS%` expanded the way Windows expands them in PATH.
fn expand(s: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    let src = wide(s);
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: buf holds buf.len() characters.
        let n = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 {
            return s.to_owned();
        }
        if n > buf.len() {
            buf.resize(n, 0);
            continue;
        }
        return String::from_utf16_lossy(&buf[..n - 1]);
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn same_path(a: &Path, b: &Path) -> bool {
    winpath::same(&s(a), &s(b), &expand)
}

fn start_menu_link() -> Option<PathBuf> {
    shell::known_folder(&FOLDERID_Programs).map(|d| d.join(LINK))
}

fn desktop_link() -> Option<PathBuf> {
    shell::known_folder(&FOLDERID_Desktop).map(|d| d.join(LINK))
}

fn system_dir() -> PathBuf {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260];
    // SAFETY: buf holds 260 characters.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return PathBuf::from(r"C:\Windows\System32");
    }
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

/// The real Windows version (RtlGetVersion is not subject to the
/// compatibility shims that make GetVersionEx lie).
fn windows_version() -> (u32, u32, u32) {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    // SAFETY: OSVERSIONINFOW is plain data; all zeroes is valid.
    let mut v: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    // SAFETY: v is a valid, sized OSVERSIONINFOW.
    let st = unsafe { RtlGetVersion(&mut v) };
    if st < 0 {
        return (0, 0, 0);
    }
    (v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
}

/// Product codes of ZipShrink copies installed from the 0.1-0.3 MSI.
fn msi_products() -> Vec<String> {
    use windows_sys::Win32::System::ApplicationInstallationAndServicing::MsiEnumRelatedProductsW;
    let code = wide(MSI_UPGRADE_CODE);
    let mut out = Vec::new();
    for i in 0..16 {
        let mut buf = [0u16; 39];
        // SAFETY: buf holds the 39 characters a product code needs.
        if unsafe { MsiEnumRelatedProductsW(code.as_ptr(), 0, i, buf.as_mut_ptr()) } != 0 {
            break;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        out.push(String::from_utf16_lossy(&buf[..n]));
    }
    out
}

/// Tells Explorer and other programs that the user's environment changed.
fn broadcast_environment() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
    let env = wide("Environment");
    let mut result = 0usize;
    // SAFETY: the string outlives the (synchronous, time-limited) call.
    unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, env.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
}

/// Whether a program file is in use: it exists and cannot be opened for
/// writing (NSIS's WaitUnlocked test). A running .exe is always locked.
fn locked(p: &Path) -> bool {
    if !p.exists() {
        return false;
    }
    match std::fs::OpenOptions::new().write(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(p) {
        Ok(_) => false,
        // A read-only file refuses writers too, without anyone using it.
        Err(_) => !std::fs::metadata(p).map(|m| m.permissions().readonly()).unwrap_or(false),
    }
}

/// Moves `from` over `to` in one step (the old file stays whole until the
/// new one is complete), retrying briefly: virus scanners and Explorer's
/// thumbnailer hold new files open for a moment.
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let (f, t) = (wide_path(from), wide_path(to));
    let mut tries = 0;
    loop {
        // SAFETY: two NUL-terminated paths.
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } != 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        tries += 1;
        if tries >= 15 {
            return Err(e);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn remove_file(p: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Bytes the files in `dir` (and below) take.
fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Deletes `file` (and then `dir`, if empty) once this process has exited:
/// a running program cannot delete its own .exe. A hidden `cmd` retries
/// every second for ten minutes, so it also works while the uninstaller's
/// window stays open.
fn delete_after_exit(file: &Path, dir: Option<&Path>) {
    let f = s(file);
    let rmdir = dir.map(|d| format!("rmdir \"{}\" >nul 2>&1 & ", s(d))).unwrap_or_default();
    let script = format!(
        "/d /s /c \"for /l %i in (1,1,600) do @(ping -n 2 127.0.0.1 >nul & del /f /q \"{f}\" >nul 2>&1 & if not exist \"{f}\" ({rmdir}exit))\""
    );
    let start = |flags: u32| {
        Command::new(system_dir().join("cmd.exe"))
            .raw_arg(&script)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | flags)
            .spawn()
    };
    // Out of setup's job, if any, so it is not ended with setup (as in
    // `launch`); not every job allows that.
    if start(CREATE_BREAKAWAY_FROM_JOB).is_err() {
        let _ = start(0);
    }
}

/// Removes `<dir>\Uninstall.exe`. When that is this very program, it is
/// first moved out of the folder into %TEMP% (Windows lets a running .exe
/// be renamed, not deleted) so the folder can go now, and the moved copy
/// is deleted after exit.
fn remove_uninstaller(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(UNINSTALLER);
    let me = std::env::current_exe().ok();
    if !me.as_deref().is_some_and(|m| same_path(m, &path)) {
        return remove_file(&path);
    }
    let parked = std::env::temp_dir().join(format!("ZipShrink-uninstall-{}.exe", std::process::id()));
    let (f, t) = (wide_path(&path), wide_path(&parked));
    // SAFETY: two NUL-terminated paths. No MOVEFILE_COPY_ALLOWED: a copy would leave the original.
    if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), MOVEFILE_REPLACE_EXISTING) } != 0 {
        delete_after_exit(&parked, None);
    } else {
        // %TEMP% is on another drive: delete it, and the folder, after exit.
        delete_after_exit(&path, Some(dir));
    }
    Ok(())
}

/// The user's PATH as stored (unexpanded) and its registry type; empty
/// when there is none. `Err` when a Path value is there but is not text
/// or cannot be read: it must then be left exactly as it is.
fn user_path() -> Result<(String, u32), String> {
    let Some(key) = reg::open(ENV_KEY, false) else { return Ok((String::new(), REG_EXPAND_SZ)) };
    Ok(key.read_str("Path")?.unwrap_or((String::new(), REG_EXPAND_SZ)))
}

/// Adds (`add`) or removes `dir` from the user's PATH, keeping the value's
/// type (normally REG_EXPAND_SZ, so `%USERPROFILE%\...` entries keep
/// working) and every other entry as written. True when it changed.
/// A PATH that cannot be read as text is not touched (and that is logged):
/// rewriting it would lose the user's entries.
fn edit_path(dir: &Path, add: bool) -> Result<bool, String> {
    let (cur, ty) = match user_path() {
        Ok(p) => p,
        Err(e) => {
            crate::note(&format!("the user PATH was left unchanged: {e}"));
            return Ok(false);
        }
    };
    let new = if add { winpath::list_add(&cur, &s(dir), &expand) } else { winpath::list_remove(&cur, &s(dir), &expand) };
    let Some(new) = new else { return Ok(false) };
    let key = reg::create(ENV_KEY)?;
    if new.is_empty() {
        key.delete_value("Path");
    } else {
        key.set_str_typed("Path", &new, ty)?;
    }
    Ok(true)
}

/// Removes the shortcut `lnk` if it starts `<dir>\zipshrink-gui.exe`.
fn remove_link(lnk: Option<PathBuf>, dir: &Path) {
    let Some(lnk) = lnk.filter(|l| l.exists()) else { return };
    if shell::link_target(&lnk).is_some_and(|t| same_path(Path::new(&t), &dir.join(GUI))) {
        let _ = std::fs::remove_file(&lnk);
    }
}

/// Removes the right-click entries that run `<dir>\zipshrink-gui.exe`.
fn remove_menu(dir: &Path) -> Result<(), String> {
    let gui = s(&dir.join(GUI));
    for key in MENU_KEYS {
        if reg::open(key, false).is_none() {
            continue;
        }
        let ours = match reg::get_str(&format!(r"{key}\command"), "") {
            Some(cmd) => winpath::command_runs(&cmd, &gui, &expand),
            // An entry without a command does nothing; it is a broken one of ours.
            None => true,
        };
        if ours {
            reg::delete_tree(key)?;
        }
    }
    Ok(())
}

fn add_menu(dir: &Path) -> Result<(), String> {
    let gui = s(&dir.join(GUI));
    for key in MENU_KEYS {
        let k = reg::create(key)?;
        k.set_str("", "Compress with ZipShrink...")?;
        k.set_str("Icon", &gui)?;
        reg::create(&format!(r"{key}\command"))?.set_str("", &format!("\"{gui}\" \"%1\""))?;
    }
    Ok(())
}

/// Writes the Add/Remove Programs entry and the folder record.
fn register(dir: &Path) -> Result<(), String> {
    let d = s(dir);
    reg::create(APP_KEY)?.set_str("InstallDir", &d)?;
    let k = reg::create(UNINST_KEY)?;
    let uninst = s(&dir.join(UNINSTALLER));
    k.set_str("DisplayName", "ZipShrink")?;
    k.set_str("DisplayVersion", crate::VERSION)?;
    k.set_str("Publisher", "ZipShrink contributors")?;
    k.set_str("DisplayIcon", &s(&dir.join(GUI)))?;
    k.set_str("InstallLocation", &d)?;
    k.set_str("UninstallString", &format!("\"{uninst}\""))?;
    k.set_str("QuietUninstallString", &format!("\"{uninst}\" /S"))?;
    k.set_str("URLInfoAbout", "https://github.com/knivesbatz/zipshrink")?;
    k.set_dword("NoModify", 1)?;
    k.set_dword("NoRepair", 1)?;
    k.set_dword("EstimatedSize", u32::try_from(dir_size(dir) / 1024).unwrap_or(u32::MAX))?;
    Ok(())
}

/// Removes the Add/Remove Programs entry and folder record if they are
/// about `dir` (another copy may have been installed elsewhere since).
fn unregister(dir: &Path) -> Result<(), String> {
    let about_dir = |v: Option<String>| v.is_none_or(|v| v.is_empty() || same_path(Path::new(&v), dir));
    if reg::open(UNINST_KEY, false).is_some() && about_dir(reg::get_str(UNINST_KEY, "InstallLocation")) {
        reg::delete_tree(UNINST_KEY)?;
    }
    if let Some(k) = reg::open(APP_KEY, true) {
        if about_dir(k.get_str("InstallDir").map(|(v, _)| v)) {
            k.delete_value("InstallDir");
        }
        drop(k);
        reg::delete_if_empty(APP_KEY);
    }
    Ok(())
}

/// Removes ZipShrink copies installed from the 0.1-0.3 MSI (for all users,
/// so Windows asks for permission), as the NSIS installer did.
fn remove_msi_copies(progress: Progress, at: f32) {
    let products = msi_products();
    if products.is_empty() {
        return;
    }
    progress(at, "Removing the older ZipShrink installed from an .msi package…");
    for p in products {
        let _ = Command::new(system_dir().join("msiexec.exe")).args(["/x", &p, "/qb-"]).status();
    }
}

/// Writes `w` through, reporting progress every few hundred kilobytes.
struct Counting<'a, W: Write> {
    inner: W,
    done: u64,
    report: &'a dyn Fn(u64),
}

impl<W: Write> Write for Counting<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        let before = self.done >> 18;
        self.done += n as u64;
        if self.done >> 18 != before {
            (self.report)(self.done);
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn status_for(name: &str) -> &'static str {
    match name {
        GUI => "Copying ZipShrink…",
        CLI => "Copying the command-line tool…",
        _ => "Copying documents…",
    }
}

fn friendly_io(what: &str, e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(112) | Some(39) => format!("There is not enough free space to {what}."),
        Some(5) | Some(32) | Some(33) => format!("Could not {what}: a file is in use or protected. {STILL_OPEN}"),
        _ => format!("Could not {what}: {e}"),
    }
}

impl Windows {
    /// Unpacks every file next to its destination as `<name>.zipshrink-new`.
    /// Returns (temporary, destination) pairs; on error removes what it wrote.
    fn stage(&self, dir: &Path, entries: &[payload::Entry], progress: Progress) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        let total: u64 = entries.iter().map(|e| e.size).sum::<u64>().max(1);
        let mut base = 0u64;
        let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
        let cleanup = |staged: &[(PathBuf, PathBuf)]| staged.iter().for_each(|(t, _)| drop(std::fs::remove_file(t)));
        for e in entries {
            let target = dir.join(e.name);
            let tmp = dir.join(format!("{}{NEW_SUFFIX}", e.name));
            let status = status_for(e.name);
            let at = |done: u64| 0.03 + 0.75 * ((base + done.min(e.size)) as f32 / total as f32);
            progress(at(0), status);
            let report = |done: u64| progress(at(done), status);
            let written = (|| -> std::io::Result<()> {
                let f = std::fs::File::create(&tmp)?;
                let mut w = Counting { inner: std::io::BufWriter::with_capacity(1 << 20, f), done: 0, report: &report };
                e.unpack_to(&mut w)?;
                let f = w.inner.into_inner().map_err(|e| e.into_error())?;
                f.sync_all()
            })();
            staged.push((tmp, target));
            if let Err(err) = written {
                cleanup(&staged);
                return Err(friendly_io(&format!("copy {}", e.name), &err));
            }
            base += e.size;
        }
        // The uninstaller is this program (it carries the same files).
        progress(0.8, "Adding the uninstaller…");
        let target = dir.join(UNINSTALLER);
        let me = std::env::current_exe().map_err(|e| format!("Could not find the installer program: {e}"))?;
        if !same_path(&me, &target) {
            let tmp = dir.join(format!("{UNINSTALLER}{NEW_SUFFIX}"));
            let copied = std::fs::copy(&me, &tmp);
            staged.push((tmp, target));
            if let Err(err) = copied {
                cleanup(&staged);
                return Err(friendly_io("add the uninstaller", &err));
            }
        }
        Ok(staged)
    }

    fn components_in(&self, dir: &Path) -> Components {
        let has_link = |l: Option<PathBuf>| l.is_some_and(|l| l.exists());
        Components {
            start_menu: has_link(start_menu_link()),
            desktop: has_link(desktop_link()),
            context_menu: reg::get_str(&format!(r"{}\command", MENU_KEYS[1]), "").is_some_and(|c| !c.is_empty()),
            path: user_path().is_ok_and(|(p, _)| winpath::list_contains(&p, &s(dir), &expand)),
        }
    }
}

impl Backend for Windows {
    fn check_system(&self) -> Result<(), String> {
        let (major, _, _) = windows_version();
        // An x86-64 program only runs on 64-bit Windows, so the version is
        // all that is left to check.
        if major < 10 {
            return Err(format!(
                "ZipShrink {} needs 64-bit Windows 10 or Windows 11. Nothing was changed: the ZipShrink you have keeps working.",
                crate::VERSION
            ));
        }
        Ok(())
    }

    /// Like the NSIS .onInit: the recorded folder decides where an upgrade
    /// goes, and an optional part counts as chosen when it is present.
    fn detect(&self) -> Option<Existing> {
        let msi = !msi_products().is_empty();
        let recorded = reg::get_str(APP_KEY, "InstallDir").filter(|d| !d.trim().is_empty());
        let Some(dir) = recorded.map(PathBuf::from) else {
            return msi.then(|| Existing { version: None, dir: self.default_dir(), components: Components::default(), msi });
        };
        Some(Existing {
            version: reg::get_str(UNINST_KEY, "DisplayVersion").filter(|v| !v.is_empty()),
            components: self.components_in(&dir),
            dir,
            msi,
        })
    }

    fn default_dir(&self) -> PathBuf {
        let local = shell::known_folder(&FOLDERID_LocalAppData)
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
            .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local")))
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        local.join("Programs").join("ZipShrink")
    }

    fn free_space(&self, dir: &Path) -> Option<u64> {
        let existing = dir.ancestors().find(|a| a.is_dir())?;
        let p = wide_path(existing);
        let mut free = 0u64;
        // SAFETY: NUL-terminated path; only the caller's free bytes are asked for.
        let ok = unsafe { GetDiskFreeSpaceExW(p.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }

    fn app_running(&self, dir: &Path) -> bool {
        [GUI, CLI].iter().any(|n| locked(&dir.join(n)))
    }

    fn wait_closed(&self, dir: &Path, secs: u32) -> bool {
        for _ in 0..secs * 2 {
            if !self.app_running(dir) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        !self.app_running(dir)
    }

    /// Installs or upgrades. The plan's components describe the end state:
    /// a part switched off is removed if this installation had it.
    fn install(&self, plan: &Plan, progress: Progress) -> Result<(), String> {
        let dir = plan.dir.as_path();
        let c = plan.components;
        progress(0.0, "Preparing…");
        if !dir.is_absolute() || dir.parent().is_none() {
            return Err(format!("\"{}\" is not a folder ZipShrink can be installed in.", dir.display()));
        }
        let entries = payload::checked_entries()?;
        if let Some(bad) = entries.iter().find(|e| e.name.contains(['\\', '/', ':']) || e.name.starts_with('.') || e.name == UNINSTALLER) {
            return Err(format!("The installer is damaged (it holds a file named \"{}\").", bad.name));
        }
        std::fs::create_dir_all(dir).map_err(|e| friendly_io(&format!("create the folder {}", dir.display()), &e))?;
        if self.app_running(dir) {
            return Err(STILL_OPEN.into());
        }

        // 1. Every file is written in full before any is replaced, so a full
        //    disk or a damaged installer leaves the old version working.
        let staged = self.stage(dir, &entries, progress)?;
        progress(0.84, "Putting the new files in place…");
        for (i, (tmp, target)) in staged.iter().enumerate() {
            if let Err(e) = replace(tmp, target) {
                staged[i..].iter().for_each(|(t, _)| drop(std::fs::remove_file(t)));
                let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                return Err(friendly_io(&format!("replace {name}"), &e));
            }
        }
        for name in LEGACY_FILES {
            let _ = remove_file(&dir.join(name));
        }

        // 2. Only one ZipShrink: remove a 0.1-0.3 MSI copy before registering,
        //    so its removal cannot take our entries with it.
        remove_msi_copies(progress, 0.86);

        progress(0.9, "Registering ZipShrink…");
        register(dir)?;

        // 3. The optional parts.
        progress(0.92, if c.start_menu || c.desktop { "Adding shortcuts…" } else { "Updating shortcuts…" });
        let gui = dir.join(GUI);
        for (want, lnk) in [(c.start_menu, start_menu_link()), (c.desktop, desktop_link())] {
            match lnk {
                Some(l) if want => shell::create_link(&l, &gui, "Compress files and folders into small ZIP archives")?,
                l => remove_link(l, dir),
            }
        }
        progress(0.95, if c.context_menu { "Adding the right-click menu…" } else { "Updating the right-click menu…" });
        if c.context_menu {
            add_menu(dir)?;
        } else {
            remove_menu(dir)?;
        }
        progress(0.97, if c.path { "Adding the zipshrink command…" } else { "Updating the zipshrink command…" });
        if edit_path(dir, c.path)? {
            broadcast_environment();
        }
        progress(1.0, "Done");
        Ok(())
    }

    fn uninstall(&self, dir: &Path, progress: Progress) -> Result<(), String> {
        progress(0.0, "Preparing…");
        if !dir.is_absolute() || dir.parent().is_none() {
            return Err(format!("\"{}\" is not a folder ZipShrink can be removed from.", dir.display()));
        }
        let recorded = reg::get_str(APP_KEY, "InstallDir").is_some_and(|d| same_path(Path::new(&d), dir));
        if !recorded && !dir.join(GUI).exists() && !dir.join(UNINSTALLER).exists() {
            return Err(format!("There is no ZipShrink installation in {}.", dir.display()));
        }
        if self.app_running(dir) {
            return Err(STILL_OPEN.into());
        }
        // Not inside the folder being removed (it could not be deleted).
        let _ = std::env::set_current_dir(std::env::temp_dir());

        progress(0.1, "Removing shortcuts…");
        remove_link(start_menu_link(), dir);
        remove_link(desktop_link(), dir);
        progress(0.25, "Removing the right-click menu…");
        remove_menu(dir)?;
        progress(0.4, "Removing the zipshrink command…");
        if edit_path(dir, false)? {
            broadcast_environment();
        }

        progress(0.55, "Removing files…");
        let payload_names = payload::entries().into_iter().map(|e| e.name);
        let mut names: Vec<&str> = KNOWN_FILES.into_iter().chain(payload_names).collect();
        names.sort_unstable();
        names.dedup();
        let mut failed = Vec::new();
        for name in names {
            let _ = remove_file(&dir.join(format!("{name}{NEW_SUFFIX}")));
            if let Err(e) = remove_file(&dir.join(name)) {
                failed.push(format!("{name} ({e})"));
            }
        }
        let _ = remove_file(&dir.join(format!("{UNINSTALLER}{NEW_SUFFIX}")));
        if !failed.is_empty() {
            // Keep the Add/Remove Programs entry so it can be tried again.
            return Err(format!("Some files could not be removed: {}. {STILL_OPEN}", failed.join(", ")));
        }

        progress(0.8, "Unregistering ZipShrink…");
        unregister(dir)?;
        remove_uninstaller(dir).map_err(|e| format!("Could not remove {UNINSTALLER}: {e}"))?;
        // Only if empty: anything the user put there stays.
        let _ = std::fs::remove_dir(dir);
        progress(1.0, "Done");
        Ok(())
    }

    fn launch(&self, dir: &Path) {
        let start = |flags: u32| {
            Command::new(dir.join(GUI))
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | flags)
                .spawn()
        };
        // Out of setup's job, if any, so closing setup's parent keeps ZipShrink;
        // not every job allows that.
        if start(CREATE_BREAKAWAY_FROM_JOB).is_err() {
            let _ = start(0);
        }
    }

    fn is_demo(&self) -> bool {
        false
    }
}
