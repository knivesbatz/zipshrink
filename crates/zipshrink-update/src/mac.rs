//! Replacing the macOS app bundle with a newer one.
//!
//! The release's `ZipShrink-mac.zip` holds one ad-hoc signed `ZipShrink.app`.
//! [`stage`] unpacks it into a staging folder on the same volume as the
//! running bundle and checks it: bundle id, a newer version, the minimum
//! macOS, `codesign --verify --deep --strict`, and that its command-line
//! tool runs. [`swap`] exchanges the two bundles with one atomic
//! `renamex_np(RENAME_SWAP)`, so the bundle's path always holds a complete
//! app (two renames that are undone on failure where the volume cannot swap).
//! A detached shell ([`spawn_finisher`]) then waits for this process to
//! exit, unregisters and deletes the old copy, registers the new one with
//! LaunchServices and, for a restart, opens it.
//!
//! Compiled on every Unix so its pure parts are tested on Linux too; only the
//! macOS updater calls it.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::{is_newer, parse_version, Blocker, Method, UpdateError, CURRENT, MAC_BUNDLE_ID};
use std::ffi::{CString, OsStr};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Staging folders are named this plus the id of the process that made them,
/// so a later start can tell abandoned ones from one in use.
const STAGE_PREFIX: &str = ".ZipShrink-update-";

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// The `.app` folder an executable at `X.app/Contents/MacOS/exe` belongs to.
pub(crate) fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let inside = macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app";
    inside.then(|| app.to_path_buf())
}

/// The bundle this process runs from (symbolic links, such as a
/// `/usr/local/bin/zipshrink` link to the bundled tool, resolved).
pub(crate) fn bundle_root() -> Option<PathBuf> {
    bundle_of(&std::env::current_exe().ok()?.canonicalize().ok()?)
}

/// Whether macOS runs the app from an App Translocation mount (a random,
/// read-only location such as `/private/var/folders/…/AppTranslocation/<id>/d/ZipShrink.app`).
/// `path` must be canonical.
pub(crate) fn is_translocated(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "AppTranslocation")
}

fn cstr(path: &Path) -> io::Result<CString> {
    Ok(CString::new(path.as_os_str().as_bytes())?)
}

fn writable(path: &Path) -> io::Result<()> {
    let c = cstr(path)?;
    // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Why `bundle` cannot be replaced where it is, if it cannot. Moving the
/// bundle needs write access to its folder and (as its `..` changes) to the
/// bundle itself; deleting the old copy needs it inside too. `access(2)`
/// checks this without touching anything, so no privacy prompt appears.
pub(crate) fn blocker(bundle: &Path) -> Option<Blocker> {
    if is_translocated(bundle) {
        return Some(Blocker::Translocated);
    }
    let parent = bundle.parent().unwrap_or(Path::new("/"));
    for dir in [parent.to_path_buf(), bundle.to_path_buf(), bundle.join("Contents"), bundle.join("Contents/MacOS")] {
        if let Err(e) = writable(&dir) {
            return Some(match e.kind() {
                io::ErrorKind::ReadOnlyFilesystem => Blocker::ReadOnlyVolume,
                _ => Blocker::NotWritable,
            });
        }
    }
    None
}

pub(crate) fn method() -> Method {
    match bundle_root() {
        // A development build outside an app bundle.
        None => Method::ReleasePage,
        Some(bundle) => match blocker(&bundle) {
            Some(b) => Method::Blocked(b),
            None => Method::ReplaceApp,
        },
    }
}

/// The running bundle, if it can replace itself.
fn installable_bundle() -> Result<PathBuf, UpdateError> {
    match method() {
        Method::ReplaceApp => bundle_root().ok_or_else(|| UpdateError::Install("ZipShrink.app was not found".into())),
        m => Err(UpdateError::Install(m.explain())),
    }
}

/// `~/Library/Caches/ZipShrink/update`, where updates are staged when it is
/// on the bundle's volume: LaunchServices does not register apps there, so a
/// staged copy never receives Finder's files or Services.
fn caches_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/ZipShrink/update"))
}

/// The process id in a staging folder's name.
pub(crate) fn stage_pid(name: &str) -> Option<u32> {
    let pid: u32 = name.strip_prefix(STAGE_PREFIX)?.parse().ok()?;
    (pid > 0 && pid <= i32::MAX as u32).then_some(pid)
}

/// A new, empty staging folder on the same volume as `bundle` (a swap
/// cannot cross volumes): in `caches` when that is on it, otherwise hidden
/// beside the bundle.
fn staging_dir(bundle: &Path, caches: Option<&Path>) -> io::Result<PathBuf> {
    let parent = bundle.parent().ok_or_else(|| io::Error::other("the app has no parent folder"))?;
    let dev = fs::metadata(parent)?.dev();
    let name = format!("{STAGE_PREFIX}{}", std::process::id());
    let in_caches = caches.and_then(|c| {
        fs::create_dir_all(c).ok()?;
        (fs::metadata(c).ok()?.dev() == dev).then(|| c.join(&name))
    });
    let dir = in_caches.unwrap_or_else(|| parent.join(&name));
    if let Err(e) = fs::remove_dir_all(&dir) {
        if e.kind() != io::ErrorKind::NotFound {
            return Err(e);
        }
    }
    fs::create_dir(&dir)?;
    Ok(dir)
}

/// Runs a tool and turns a failure into an error that says what failed.
fn run(cmd: &mut Command, what: &str) -> Result<(), UpdateError> {
    let out = cmd.stdin(Stdio::null()).output()?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr);
    let msg = msg.trim();
    Err(UpdateError::Install(if msg.is_empty() { format!("{what} failed ({})", out.status) } else { format!("{what} failed: {msg}") }))
}

/// One key of a bundle's `Info.plist`. `PlistBuddy`, because
/// `plutil -extract … raw` needs macOS 12 and ZipShrink supports 11.
fn plist(app: &Path, key: &str) -> Option<String> {
    let out = Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg(format!("Print :{key}"))
        .arg(app.join("Contents/Info.plist"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// This Mac's macOS version, e.g. `14.5`.
fn os_version() -> Option<String> {
    let out = Command::new("/usr/sbin/sysctl").args(["-n", "kern.osproductversion"]).stderr(Stdio::null()).output().ok()?;
    let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// Whether macOS `os` (unknown: assume yes) meets an app's `LSMinimumSystemVersion`.
pub(crate) fn os_allows(min: &str, os: Option<&str>) -> bool {
    match (parse_version(min), os.map(parse_version)) {
        (None, _) => false,
        (Some(m), Some(Some(o))) => m <= o,
        (Some(_), _) => true,
    }
}

/// Unpacks `zip` into a new staging folder for replacing `bundle` and checks
/// the app inside; returns the staged `.app`. Nothing is left behind on failure.
pub(crate) fn stage(zip: &Path, bundle: &Path, caches: Option<&Path>) -> Result<PathBuf, UpdateError> {
    let dir = staging_dir(bundle, caches)?;
    let r = stage_into(zip, bundle, &dir);
    if r.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    r
}

fn stage_into(zip: &Path, bundle: &Path, dir: &Path) -> Result<PathBuf, UpdateError> {
    // --noqtn: nothing unpacked inherits a quarantine flag from the zip.
    run(Command::new("/usr/bin/ditto").args(["-x", "-k", "--noqtn"]).arg(zip).arg(dir), "Unpacking the update")?;
    let entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| !n.as_bytes().starts_with(b".") && n != "__MACOSX"))
        .collect();
    let app = match entries.as_slice() {
        [a] if a.extension() == Some(OsStr::new("app")) && a.is_dir() => a.clone(),
        _ => return Err(UpdateError::Install("the download does not hold exactly one app".into())),
    };
    // Belt and braces after --noqtn: a quarantined tool would be stopped by
    // Gatekeeper below and after the update.
    let _ = Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let id = plist(&app, "CFBundleIdentifier");
    if id.as_deref() != Some(MAC_BUNDLE_ID) {
        return Err(UpdateError::Install(format!("the download is not ZipShrink (bundle id {})", id.unwrap_or_default())));
    }
    let version = plist(&app, "CFBundleShortVersionString").ok_or_else(|| UpdateError::Install("the download has no version".into()))?;
    // The installed bundle's own version, not CURRENT: a test build that
    // only claims to be old must still not be "updated" to an older app.
    let installed = plist(bundle, "CFBundleShortVersionString").unwrap_or_else(|| CURRENT.to_string());
    if !is_newer(&version, &installed) {
        return Err(UpdateError::Install(format!("the download ({version}) is not newer than this copy ({installed})")));
    }
    if let Some(min) = plist(&app, "LSMinimumSystemVersion") {
        let os = os_version();
        if !os_allows(&min, os.as_deref()) {
            let os = os.unwrap_or_else(|| "an unknown version".into());
            return Err(UpdateError::Install(format!("ZipShrink {version} needs macOS {min} or later; this Mac has {os}")));
        }
    }
    let exe = plist(&app, "CFBundleExecutable").unwrap_or_default();
    if exe.is_empty() || !app.join("Contents/MacOS").join(&exe).is_file() {
        return Err(UpdateError::Install("the download's app has no program".into()));
    }
    run(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&app), "Checking the code signature")?;
    let out = Command::new(app.join("Contents/MacOS/zipshrink")).arg("--version").stdin(Stdio::null()).output()?;
    let said = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !said.contains(&version) {
        return Err(UpdateError::Install(format!("the new command-line tool did not run ({})", said.trim())));
    }
    Ok(app)
}

/// Exchanges the staged app with `bundle`. Returns where the old copy is now
/// (inside the staging folder).
pub(crate) fn swap(staged: &Path, bundle: &Path) -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let (a, b) = (cstr(staged)?, cstr(bundle)?);
        // SAFETY: both are valid NUL-terminated strings for the duration of the call.
        if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
            return Ok(staged.to_path_buf());
        }
        let e = io::Error::last_os_error();
        // Volumes that cannot swap (some network and FAT volumes) say ENOTSUP
        // or EINVAL; anything else would fail the renames too.
        if !matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) {
            return Err(swap_error(e, bundle));
        }
    }
    swap_by_renames(staged, bundle).map_err(|e| swap_error(e, bundle))
}

/// The fallback: the old bundle moves into the staging folder, then the new
/// one into its place; if that fails, the old one moves back.
pub(crate) fn swap_by_renames(staged: &Path, bundle: &Path) -> io::Result<PathBuf> {
    // Not ending in .app, so LaunchServices never takes it for an app.
    let aside = staged.with_extension("previous");
    fs::rename(bundle, &aside)?;
    if let Err(e) = fs::rename(staged, bundle) {
        if let Err(back) = fs::rename(&aside, bundle) {
            return Err(io::Error::new(
                e.kind(),
                format!("{e}; the previous copy could not be put back either ({back}) and is at {}", aside.display()),
            ));
        }
        return Err(e);
    }
    Ok(aside)
}

fn swap_error(e: io::Error, bundle: &Path) -> io::Error {
    let why = match e.raw_os_error() {
        // macOS 13+ App Management, or a locked bundle.
        Some(libc::EPERM) => "macOS did not allow it. Allow ZipShrink under System Settings > Privacy & Security > \
                              App Management, or download the new version from the release page"
            .to_string(),
        Some(libc::EACCES) => "this account cannot change that folder".to_string(),
        _ => e.to_string(),
    };
    io::Error::new(e.kind(), format!("could not replace {}: {why}", bundle.display()))
}

/// The detached clean-up after a swap: `$1` pid to wait for, `$2` the old
/// copy, `$3` the staging folder, `$4` the installed bundle, `$5` 1 to open
/// it again. A restart only follows a prompt exit (60 s), not a quit hours
/// after an install whose window was kept open.
const FINISHER: &str = r#"trap '' HUP INT
pid=$1 old=$2 stage=$3 app=$4 restart=$5 lsregister=$6
n=0
while kill -0 "$pid" 2>/dev/null; do
  n=$((n + 1))
  sleep 0.2
done
if [ -x "$lsregister" ]; then "$lsregister" -u "$old" >/dev/null 2>&1; fi
rm -rf "$stage"
if [ -x "$lsregister" ]; then "$lsregister" -f "$app" >/dev/null 2>&1; fi
if [ "$restart" = 1 ] && [ "$n" -lt 300 ]; then /usr/bin/open -n "$app"; fi
exit 0
"#;

/// Starts [`FINISHER`] in its own process group (a Ctrl+C in the terminal
/// that ran `zipshrink update` does not reach it) with no terminal.
pub(crate) fn spawn_finisher(pid: u32, old: &Path, stage: &Path, app: &Path, restart: bool) -> io::Result<()> {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(FINISHER)
        .arg("zipshrink-update")
        .arg(pid.to_string())
        .arg(old)
        .arg(stage)
        .arg(app)
        .arg(if restart { "1" } else { "0" })
        .arg(LSREGISTER)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

/// Stages the downloaded `zip` for replacing `bundle`, then deletes the zip
/// (also when staging fails: a download that failed a check is not kept).
fn stage_and_delete(zip: &Path, bundle: &Path, caches: Option<&Path>) -> Result<PathBuf, UpdateError> {
    let r = stage(zip, bundle, caches);
    let _ = fs::remove_file(zip);
    r
}

/// [`crate::prepare_update`]: stages the downloaded zip and deletes it.
pub(crate) fn prepare(zip: &Path) -> Result<PathBuf, UpdateError> {
    let bundle = installable_bundle()?;
    stage_and_delete(zip, &bundle, caches_dir().as_deref())
}

/// [`crate::launch_installer`]: stages `path` unless it is already a staged
/// app, swaps it in and starts the clean-up.
pub(crate) fn install(path: &Path, restart: bool) -> io::Result<()> {
    let bundle = installable_bundle().map_err(to_io)?;
    install_to(path, &bundle, caches_dir().as_deref(), restart)
}

fn to_io(e: UpdateError) -> io::Error {
    match e {
        UpdateError::Io(e) => e,
        e => io::Error::other(e.to_string()),
    }
}

/// Puts the update at `path` (a downloaded zip, or an app [`stage`]d
/// earlier) in place of `bundle`, then starts the clean-up that waits for
/// this process.
fn install_to(path: &Path, bundle: &Path, caches: Option<&Path>, restart: bool) -> io::Result<()> {
    let staged = if path.extension() == Some(OsStr::new("app")) {
        if !path.join("Contents/Info.plist").is_file() {
            return Err(io::Error::other(format!("the staged update at {} is gone", path.display())));
        }
        // Staged a while ago: `zipshrink update` in a terminal may have
        // installed this version (or a newer one) since.
        if let (Some(new), Some(cur)) = (plist(path, "CFBundleShortVersionString"), plist(bundle, "CFBundleShortVersionString")) {
            if !is_newer(&new, &cur) {
                return Err(io::Error::other(format!("ZipShrink {cur} is already installed; quit ZipShrink and open it again")));
            }
        }
        path.to_path_buf()
    } else {
        stage_and_delete(path, bundle, caches).map_err(to_io)?
    };
    // The clean-up deletes this folder, so it must be one of ours.
    let stage_dir = staged
        .parent()
        .filter(|d| d.file_name().and_then(OsStr::to_str).and_then(stage_pid).is_some())
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other(format!("{} is not a staged update", staged.display())))?;
    let old = swap(&staged, bundle)?;
    // The update is in place either way; without the clean-up only the old
    // copy stays in the staging folder until the next start.
    if let Err(e) = spawn_finisher(std::process::id(), &old, &stage_dir, bundle, restart) {
        eprintln!("ZipShrink: could not start the clean-up after the update: {e}");
    }
    Ok(())
}

/// Whether a process with this id exists.
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// [`crate::mac_cleanup_leftovers`].
pub(crate) fn cleanup_leftovers() {
    let me = std::process::id();
    let beside = bundle_root().and_then(|b| b.parent().map(Path::to_path_buf));
    let dirs = caches_dir().into_iter().chain(beside);
    for dir in dirs {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for e in entries.filter_map(Result::ok) {
            let Some(pid) = e.file_name().to_str().and_then(stage_pid) else { continue };
            // Another ZipShrink (say `zipshrink update` in a terminal) may be
            // using its folder right now; a folder with this process's id is
            // from a process that is gone.
            if pid != me && alive(pid) {
                continue;
            }
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn finds_the_bundle_of_an_executable() {
        let app = Path::new("/Applications/ZipShrink.app");
        assert_eq!(bundle_of(&app.join("Contents/MacOS/zipshrink-gui")).as_deref(), Some(app));
        assert_eq!(bundle_of(&app.join("Contents/MacOS/zipshrink")).as_deref(), Some(app));
        let renamed = Path::new("/Users/a/Apps/ZipShrink 2.app");
        assert_eq!(bundle_of(&renamed.join("Contents/MacOS/zipshrink-gui")).as_deref(), Some(renamed));
        assert_eq!(bundle_of(Path::new("/usr/local/bin/zipshrink")), None);
        assert_eq!(bundle_of(Path::new("/x/target/debug/zipshrink-gui")), None);
        assert_eq!(bundle_of(Path::new("/x/ZipShrink/Contents/MacOS/zipshrink-gui")), None);
        assert_eq!(bundle_of(Path::new("/x/ZipShrink.app/Contents/Resources/zipshrink")), None);
        assert_eq!(bundle_of(Path::new("zipshrink")), None);
    }

    #[test]
    fn translocation_is_detected_from_the_path() {
        let t = Path::new("/private/var/folders/x1/abc123/T/AppTranslocation/0E8A6E5A-1C9B-4E8A-9C61-5C3A1A7B2D10/d/ZipShrink.app");
        assert!(is_translocated(t));
        assert_eq!(blocker(t), Some(Blocker::Translocated));
        assert!(!is_translocated(Path::new("/Applications/ZipShrink.app")));
        assert!(!is_translocated(Path::new("/Users/a/Downloads/ZipShrink.app")));
        assert!(!is_translocated(Path::new("/Users/a/AppTranslocationNotes/ZipShrink.app")));
    }

    #[test]
    fn staging_folder_names() {
        assert_eq!(stage_pid(".ZipShrink-update-4242"), Some(4242));
        assert_eq!(stage_pid(".ZipShrink-update-0"), None);
        assert_eq!(stage_pid(".ZipShrink-update-99999999999"), None);
        assert_eq!(stage_pid(".ZipShrink-update-12x"), None);
        assert_eq!(stage_pid("ZipShrink.app"), None);
        assert_eq!(stage_pid(&format!("{STAGE_PREFIX}{}", std::process::id())), Some(std::process::id()));
        assert!(alive(std::process::id()));
        assert!(!alive(0));
    }

    #[test]
    fn minimum_system_versions() {
        assert!(os_allows("11.0", Some("11.0")));
        assert!(os_allows("11.0", Some("14.5")));
        assert!(os_allows("11.0", Some("11.7.10")));
        assert!(!os_allows("12.0", Some("11.7.10")));
        assert!(!os_allows("15.1", Some("15.0.1")));
        assert!(os_allows("11.0", None));
        assert!(!os_allows("eleven", Some("14.5")));
    }

    #[test]
    fn a_writable_bundle_is_not_blocked() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("ZipShrink.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        assert_eq!(blocker(&app), None);
        // A missing bundle cannot be replaced.
        assert_eq!(blocker(&t.path().join("Gone.app")), Some(Blocker::NotWritable));
    }

    /// A fake bundle that only needs a valid Info.plist to look installed.
    fn bundle_dir(dir: &Path, name: &str, marker: &str) -> PathBuf {
        let app = dir.join(name);
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/Info.plist"), marker).unwrap();
        app
    }

    #[test]
    fn swap_by_renames_keeps_a_complete_bundle_in_place() {
        let t = tempfile::tempdir().unwrap();
        let installed = bundle_dir(t.path(), "Applications/ZipShrink.app", "old");
        let stage = t.path().join(".ZipShrink-update-1");
        let staged = bundle_dir(&stage, "ZipShrink.app", "new");
        let old = swap_by_renames(&staged, &installed).unwrap();
        assert_eq!(fs::read_to_string(installed.join("Contents/Info.plist")).unwrap(), "new");
        assert_eq!(fs::read_to_string(old.join("Contents/Info.plist")).unwrap(), "old");
        assert!(old.starts_with(&stage) && !staged.exists());
        // A failed second rename puts the old copy back.
        let missing = stage.join("Missing.app");
        assert!(swap_by_renames(&missing, &installed).is_err());
        assert_eq!(fs::read_to_string(installed.join("Contents/Info.plist")).unwrap(), "new");
    }

    #[test]
    fn swap_exchanges_the_bundles() {
        let t = tempfile::tempdir().unwrap();
        let installed = bundle_dir(t.path(), "Applications/ZipShrink.app", "old");
        let staged = bundle_dir(&t.path().join(".ZipShrink-update-1"), "ZipShrink.app", "new");
        let old = swap(&staged, &installed).unwrap();
        assert_eq!(fs::read_to_string(installed.join("Contents/Info.plist")).unwrap(), "new");
        assert_eq!(fs::read_to_string(old.join("Contents/Info.plist")).unwrap(), "old");
        if cfg!(target_os = "macos") {
            // RENAME_SWAP: the old copy takes the staged one's place.
            assert_eq!(old, staged);
        }
    }

    #[test]
    fn only_staged_apps_are_installed() {
        let t = tempfile::tempdir().unwrap();
        let installed = bundle_dir(t.path(), "Applications/ZipShrink.app", "old");
        // The clean-up would delete the folder an app is in: never one of the user's.
        let other = bundle_dir(t.path(), "Downloads/ZipShrink.app", "new");
        let e = install_to(&other, &installed, None, false).unwrap_err().to_string();
        assert!(e.contains("not a staged update"), "{e}");
        let gone = t.path().join(".ZipShrink-update-1/ZipShrink.app");
        let e = install_to(&gone, &installed, None, false).unwrap_err().to_string();
        assert!(e.contains("is gone"), "{e}");
        assert_eq!(fs::read_to_string(installed.join("Contents/Info.plist")).unwrap(), "old");
        assert!(other.exists());
    }

    #[test]
    fn finisher_waits_for_the_process_then_cleans_up() {
        let t = tempfile::tempdir().unwrap();
        let app = bundle_dir(t.path(), "ZipShrink.app", "new");
        let stage = t.path().join(".ZipShrink-update-1");
        let old = bundle_dir(&stage, "ZipShrink.app", "old");
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        spawn_finisher(child.id(), &old, &stage, &app, false).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert!(old.exists(), "the old copy went while its process was still running");
        child.kill().unwrap();
        child.wait().unwrap();
        let start = Instant::now();
        while stage.exists() && start.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!stage.exists(), "the staging folder was not removed");
        assert_eq!(fs::read_to_string(app.join("Contents/Info.plist")).unwrap(), "new");
    }

    #[test]
    fn staging_goes_to_the_caches_on_the_same_volume() {
        let t = tempfile::tempdir().unwrap();
        let app = bundle_dir(t.path(), "Applications/ZipShrink.app", "x");
        let caches = t.path().join("Caches/ZipShrink/update");
        let dir = staging_dir(&app, Some(&caches)).unwrap();
        assert_eq!(dir, caches.join(format!("{STAGE_PREFIX}{}", std::process::id())));
        assert!(dir.is_dir() && fs::read_dir(&dir).unwrap().next().is_none());
        let beside = staging_dir(&app, None).unwrap();
        assert_eq!(beside.parent(), app.parent());
    }

    /// The real thing on macOS: an ad-hoc signed fake app zipped with
    /// `ditto` as the release workflow does, staged, then swapped in.
    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;

        fn quarantined(path: &Path) -> bool {
            let p = cstr(path).unwrap();
            // SAFETY: valid NUL-terminated strings; a null buffer of size 0 only asks for the size.
            unsafe { libc::getxattr(p.as_ptr(), c"com.apple.quarantine".as_ptr(), std::ptr::null_mut(), 0, 0, 0) >= 0 }
        }

        fn sh(cmd: &mut Command) {
            let out = cmd.output().unwrap();
            assert!(out.status.success(), "{cmd:?}: {}", String::from_utf8_lossy(&out.stderr));
        }

        /// `A.app` with version `v`: two tiny programs (`zipshrink --version`
        /// prints "zipshrink v"), signed inside out like the real bundle.
        fn fake_app(dir: &Path, v: &str, gui_body: &str) -> PathBuf {
            let app = dir.join("A.app");
            let bin = app.join("Contents/MacOS");
            fs::create_dir_all(&bin).unwrap();
            fs::write(
                app.join("Contents/Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{MAC_BUNDLE_ID}</string>
<key>CFBundleName</key><string>ZipShrink</string>
<key>CFBundleExecutable</key><string>zipshrink-gui</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{v}</string>
<key>CFBundleVersion</key><string>{v}</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#
                ),
            )
            .unwrap();
            let src = dir.join("src");
            fs::create_dir_all(&src).unwrap();
            fs::write(src.join("cli.c"), format!("#include <stdio.h>\nint main(void){{puts(\"zipshrink {v}\");return 0;}}\n")).unwrap();
            fs::write(src.join("gui.c"), format!("#include <stdio.h>\n#include <unistd.h>\nint main(void){{{gui_body}return 0;}}\n"))
                .unwrap();
            sh(Command::new("cc").arg("-o").arg(bin.join("zipshrink")).arg(src.join("cli.c")));
            sh(Command::new("cc").arg("-o").arg(bin.join("zipshrink-gui")).arg(src.join("gui.c")));
            fs::remove_dir_all(&src).unwrap();
            sh(Command::new("xattr").arg("-cr").arg(&app));
            sh(Command::new("codesign")
                .args(["--force", "-s", "-", "--identifier", "io.github.knivesbatz.zipshrink.cli"])
                .arg(bin.join("zipshrink")));
            sh(Command::new("codesign").args(["--force", "-s", "-"]).arg(&app));
            sh(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&app));
            app
        }

        fn zip(app: &Path, out: &Path) {
            sh(Command::new("ditto").args(["-c", "-k", "--norsrc", "--noextattr", "--noqtn", "--keepParent"]).arg(app).arg(out));
        }

        fn version(app: &Path) -> Option<String> {
            plist(app, "CFBundleShortVersionString")
        }

        /// Installed 1.0.0 in `Applications/`, the 9.9.9 update zipped, and a
        /// caches folder, all in one temporary folder (one volume).
        fn setup(t: &Path) -> (PathBuf, PathBuf, PathBuf) {
            let installed = fake_app(&t.join("Applications"), "1.0.0", "");
            let built = fake_app(&t.join("build"), "9.9.9", "");
            let z = t.join("ZipShrink-mac.zip");
            zip(&built, &z);
            // As a browser would mark it.
            sh(Command::new("xattr").args(["-w", "com.apple.quarantine", "0081;00000000;Safari;"]).arg(&z));
            (installed, z, t.join("Caches"))
        }

        #[test]
        fn stage_then_swap() {
            let t = tempfile::tempdir().unwrap();
            let t = t.path().canonicalize().unwrap();
            let (installed, z, caches) = setup(&t);
            let staged = stage(&z, &installed, Some(&caches)).unwrap();
            assert!(staged.starts_with(&caches));
            assert_eq!(version(&staged).as_deref(), Some("9.9.9"));
            assert!(!quarantined(&staged), "the staged app is quarantined");
            assert!(!quarantined(&staged.join("Contents/MacOS/zipshrink")), "the staged tool is quarantined");
            let old = swap(&staged, &installed).unwrap();
            assert_eq!(old, staged, "RENAME_SWAP was not used");
            assert_eq!(version(&installed).as_deref(), Some("9.9.9"));
            assert_eq!(version(&old).as_deref(), Some("1.0.0"));
            sh(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&installed));
        }

        #[test]
        fn stage_then_swap_by_renames() {
            let t = tempfile::tempdir().unwrap();
            let t = t.path().canonicalize().unwrap();
            let (installed, z, caches) = setup(&t);
            let staged = stage(&z, &installed, Some(&caches)).unwrap();
            let old = swap_by_renames(&staged, &installed).unwrap();
            assert_eq!(version(&installed).as_deref(), Some("9.9.9"));
            assert_eq!(version(&old).as_deref(), Some("1.0.0"));
            assert!(old.starts_with(staged.parent().unwrap()));
        }

        #[test]
        fn install_from_the_zip_then_refuse_a_stale_stage() {
            let t = tempfile::tempdir().unwrap();
            let t = t.path().canonicalize().unwrap();
            let (installed, z, caches) = setup(&t);
            // Staged by the app, say, before `zipshrink update` installed it.
            let early = stage(&z, &installed, Some(&t.join("Caches2"))).unwrap();
            install_to(&z, &installed, Some(&caches), false).unwrap();
            assert!(!z.exists(), "the zip was kept");
            assert_eq!(version(&installed).as_deref(), Some("9.9.9"));
            sh(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&installed));
            let e = install_to(&early, &installed, Some(&caches), false).unwrap_err().to_string();
            assert!(e.contains("already installed"), "{e}");
            assert_eq!(version(&early).as_deref(), Some("9.9.9"));
            // Only staged apps are taken: the clean-up deletes their folder.
            let elsewhere = fake_app(&t.join("elsewhere"), "9.9.10", "");
            let e = install_to(&elsewhere, &installed, Some(&caches), false).unwrap_err().to_string();
            assert!(e.contains("not a staged update"), "{e}");
            assert_eq!(version(&installed).as_deref(), Some("9.9.9"));
        }

        #[test]
        fn stage_refuses_bad_updates() {
            let t = tempfile::tempdir().unwrap();
            let t = t.path().canonicalize().unwrap();
            let (installed, _, caches) = setup(&t);
            // Not newer than the installed copy.
            let same = fake_app(&t.join("same"), "1.0.0", "");
            let z = t.join("same.zip");
            zip(&same, &z);
            let e = stage(&z, &installed, Some(&caches)).unwrap_err().to_string();
            assert!(e.contains("not newer"), "{e}");
            // A broken signature.
            let bad = fake_app(&t.join("bad"), "9.9.9", "");
            fs::write(bad.join("Contents/Info.plist.extra"), "x").unwrap();
            let z = t.join("bad.zip");
            zip(&bad, &z);
            let e = stage(&z, &installed, Some(&caches)).unwrap_err().to_string();
            assert!(e.contains("signature"), "{e}");
            // Nothing is left in the caches.
            assert_eq!(fs::read_dir(&caches).unwrap().count(), 0);
            assert_eq!(version(&installed).as_deref(), Some("1.0.0"));
        }

        #[test]
        fn a_read_only_volume_is_blocked() {
            let t = tempfile::tempdir().unwrap();
            let t = t.path().canonicalize().unwrap();
            fake_app(&t.join("src"), "1.0.0", "");
            let dmg = t.join("x.dmg");
            let made =
                Command::new("hdiutil").args(["create", "-quiet", "-format", "UDZO", "-srcfolder"]).arg(t.join("src")).arg(&dmg).status();
            if !made.is_ok_and(|s| s.success()) {
                eprintln!("hdiutil create failed; skipped");
                return;
            }
            let mnt = t.join("mnt");
            fs::create_dir(&mnt).unwrap();
            let attached =
                Command::new("hdiutil").args(["attach", "-quiet", "-readonly", "-nobrowse", "-mountpoint"]).arg(&mnt).arg(&dmg).status();
            if !attached.is_ok_and(|s| s.success()) {
                eprintln!("hdiutil attach failed; skipped");
                return;
            }
            let b = blocker(&mnt.join("A.app"));
            let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&mnt).status();
            assert_eq!(b, Some(Blocker::ReadOnlyVolume));
        }

        /// `cargo test -p zipshrink-update -- --ignored`: the finisher opens
        /// the new app once the old process has exited.
        #[test]
        #[ignore = "opens an app through LaunchServices"]
        fn finisher_relaunches_the_app() {
            let t = tempfile::tempdir().unwrap();
            let t = t.path().canonicalize().unwrap();
            let marker = t.join("launched");
            let body = format!("FILE *f = fopen(\"{}\", \"w\"); if (f) {{ fputs(\"yes\", f); fclose(f); }}", marker.display());
            let app = fake_app(&t.join("Applications"), "9.9.9", &body);
            let stage = t.join(".ZipShrink-update-1");
            let old = bundle_dir(&stage, "ZipShrink.app", "old");
            let mut child = Command::new("sleep").arg("30").spawn().unwrap();
            spawn_finisher(child.id(), &old, &stage, &app, true).unwrap();
            std::thread::sleep(Duration::from_secs(1));
            assert!(!marker.exists(), "opened before the old process exited");
            child.kill().unwrap();
            child.wait().unwrap();
            let start = Instant::now();
            while !marker.exists() && start.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(200));
            }
            assert!(marker.exists(), "the new app was not opened");
            assert!(!stage.exists());
        }
    }
}
