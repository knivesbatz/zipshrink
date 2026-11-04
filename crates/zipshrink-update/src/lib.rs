//! Checks GitHub Releases for a newer ZipShrink and installs it.
//!
//! A check only reads the public release list (automatically at most once a
//! week, see [`CHECK_INTERVAL`]). A release that has no package for this
//! system ([`platform_asset`]) counts as no update. When the user agrees to
//! an update, the package is downloaded to the temp folder and its SHA-256
//! compared with the digest GitHub publishes for the file (or, for releases
//! without one, with the release's `SHA256SUMS`); a file that does not match
//! is deleted.
//!
//! - Windows: the verified installer runs silently (`/S`): it waits for
//!   ZipShrink to close, upgrades the installation in place keeping the
//!   user's choices, and with `/RESTART` starts ZipShrink again.
//! - macOS: `ZipShrink-mac.zip` is unpacked and checked next to the running
//!   `ZipShrink.app`, the two bundles are swapped atomically, and a detached
//!   shell removes the old copy once ZipShrink has quit (see `mac.rs`).
//!
//! The app installs right away (Restart) or when it closes. Portable copies,
//! copies that cannot replace themselves ([`Method::Blocked`]) and other
//! systems are pointed to the release page instead.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
mod mac;

/// `owner/repo` the releases come from, fixed at build time by the release
/// workflow (`ZIPSHRINK_UPDATE_REPO=${{ github.repository }}`).
pub const REPO: &str = match option_env!("ZIPSHRINK_UPDATE_REPO") {
    Some(r) => r,
    None => "knivesbatz/zipshrink",
};
/// The version updates are compared with. CI builds a copy with
/// `ZIPSHRINK_UPDATE_FROM=0.0.1` to exercise a real download and install.
pub const CURRENT: &str = match option_env!("ZIPSHRINK_UPDATE_FROM") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// The Windows installer asset of every release (also what Windows users download).
pub const WINDOWS_INSTALLER: &str = "ZipShrink-setup.exe";
/// The zipped `ZipShrink.app` the macOS updater installs. Never rename it:
/// every installed Mac copy looks for this name.
pub const MAC_APP_ZIP: &str = "ZipShrink-mac.zip";
/// The bundle identifier of `ZipShrink.app`; an update must have the same.
pub const MAC_BUNDLE_ID: &str = "io.github.knivesbatz.zipshrink";
/// Only for releases without GitHub's per-asset SHA-256 digest; ZipShrink
/// 0.2 and 0.3 need it in every release.
pub const CHECKSUMS: &str = "SHA256SUMS";
/// Automatic checks: at most once a week (seconds).
pub const CHECK_INTERVAL: u64 = 7 * 86400;
/// Shipped inside the portable zip; its presence means "do not run setup".
pub const PORTABLE_MARKER: &str = "zipshrink-portable.txt";

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub page: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    pub size: u64,
    /// `sha256:<hex>`, recorded by GitHub for each uploaded file.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug)]
pub enum UpdateError {
    Network(String),
    /// GitHub refused the request because of its rate limit.
    RateLimited,
    Invalid(String),
    Checksum(String),
    /// The downloaded update failed a check before it was installed.
    Install(String),
    Io(std::io::Error),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "could not reach the update server: {e}"),
            UpdateError::RateLimited => write!(f, "GitHub is limiting requests from this network right now; try again in an hour"),
            UpdateError::Invalid(e) => write!(f, "unexpected update information: {e}"),
            UpdateError::Checksum(e) => write!(f, "the download failed its integrity check and was deleted: {e}"),
            UpdateError::Install(e) => write!(f, "the update was not installed: {e}"),
            UpdateError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        UpdateError::Io(e)
    }
}

/// Parses `v1.2.3` / `1.2.3-beta` into a comparable triple.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let v = (it.next()?.ok()?, it.next().unwrap_or(Ok(0)).ok()?, it.next().unwrap_or(Ok(0)).ok()?);
    Some(v)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .https_only(true)
        // The operating system's certificate store, so proxies and
        // corporate CAs that the system trusts work too.
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .user_agent(format!("ZipShrink/{CURRENT} (update check)"))
        .build()
        .into()
}

fn net_error(e: ureq::Error) -> UpdateError {
    match e {
        ureq::Error::StatusCode(403 | 429) => UpdateError::RateLimited,
        e => UpdateError::Network(e.to_string()),
    }
}

/// The release asset this system updates from: the installer on Windows,
/// the zipped app on macOS, none elsewhere.
pub fn platform_asset() -> Option<&'static str> {
    if cfg!(windows) {
        Some(WINDOWS_INSTALLER)
    } else if cfg!(target_os = "macos") {
        Some(MAC_APP_ZIP)
    } else {
        None
    }
}

/// Whether a release with these assets is an update for a system that
/// installs from `wanted` (see [`platform_asset`]). Systems without a
/// package of their own are pointed to the release page, so any release is.
fn offers(assets: &[Asset], wanted: Option<&str>) -> bool {
    wanted.is_none_or(|w| assets.iter().any(|a| a.name == w))
}

/// Returns the latest release if it is newer than this build and has a
/// package for this system (a release without one counts as up to date).
pub fn check() -> Result<Option<Release>, UpdateError> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent().get(&url).header("Accept", "application/vnd.github+json").call();
    let mut resp = match resp {
        Ok(r) => r,
        // No release has been published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(net_error(e)),
    };
    let r: ApiRelease = resp.body_mut().read_json().map_err(|e| UpdateError::Invalid(e.to_string()))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let version = r.tag_name.trim_start_matches(['v', 'V']).to_string();
    if parse_version(&version).is_none() {
        return Err(UpdateError::Invalid(format!("release tag {:?} is not a version", r.tag_name)));
    }
    if !is_newer(&version, CURRENT) || !offers(&r.assets, platform_asset()) {
        return Ok(None);
    }
    Ok(Some(Release { version, tag: r.tag_name, page: r.html_url, notes: r.body.unwrap_or_default(), assets: r.assets }))
}

/// How this copy can be updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Download and run the Windows installer.
    Installer,
    /// Download the zipped app and put it in place of this `ZipShrink.app` (macOS).
    ReplaceApp,
    /// Open the release page (portable copies, builds outside an app bundle,
    /// other systems).
    ReleasePage,
    /// This copy could update itself, but not where it is now.
    Blocked(Blocker),
}

/// Why a macOS copy cannot replace itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    /// macOS runs it from a temporary, read-only copy (App Translocation),
    /// because it was opened where it was downloaded or from the disk image.
    Translocated,
    /// It is on a read-only volume, such as the mounted disk image.
    ReadOnlyVolume,
    /// This account cannot change the app or the folder it is in.
    NotWritable,
}

impl Blocker {
    /// What to do about it, in one short sentence (for the update banner).
    pub fn advice(&self) -> &'static str {
        match self {
            Blocker::Translocated | Blocker::ReadOnlyVolume => "Move ZipShrink to Applications to update it.",
            Blocker::NotWritable => "Download it from the release page.",
        }
    }
}

impl Method {
    /// Whether the app downloads and installs updates by itself.
    pub fn installs_itself(&self) -> bool {
        matches!(self, Method::Installer | Method::ReplaceApp)
    }

    /// One or two sentences on how this copy gets updated.
    pub fn explain(&self) -> String {
        match self {
            Method::Installer => "The new version's setup program installs it in place; your settings are kept.",
            Method::ReplaceApp => "The new version replaces this copy of ZipShrink; your settings are kept.",
            Method::ReleasePage if cfg!(windows) => "This copy is portable: download the new version from the release page.",
            Method::ReleasePage => "Download the new version from the release page.",
            Method::Blocked(Blocker::Translocated) => {
                "macOS runs ZipShrink from a temporary, read-only copy because it has not been moved since it was \
                 downloaded. Move ZipShrink to the Applications folder and open it from there to update it."
            }
            Method::Blocked(Blocker::ReadOnlyVolume) => {
                "ZipShrink is running from a read-only disk, such as its disk image. Move ZipShrink to the \
                 Applications folder and open it from there to update it."
            }
            Method::Blocked(Blocker::NotWritable) => {
                "This account cannot change the folder ZipShrink is in, so it cannot update itself. Download the \
                 new version from the release page, or update it from an administrator account."
            }
        }
        .to_string()
    }
}

/// How this copy can be updated.
#[cfg(not(target_os = "macos"))]
pub fn method() -> Method {
    if cfg!(windows) && !is_portable() {
        Method::Installer
    } else {
        Method::ReleasePage
    }
}

/// How this copy can be updated: by replacing its app bundle, unless it is
/// not in one or cannot change it where it is.
#[cfg(target_os = "macos")]
pub fn method() -> Method {
    mac::method()
}

pub fn is_portable() -> bool {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(PORTABLE_MARKER).exists())).unwrap_or(false)
}

fn download_to(url: &str, dest: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<[u8; 32], UpdateError> {
    let mut resp = agent().get(url).call().map_err(net_error)?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(1 << 30).reader();
    let mut f = std::fs::File::create(dest)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    f.sync_all()?;
    Ok(h.finalize().into())
}

/// Finds `name`'s hash in a `sha256sum`-style listing.
pub fn expected_hash(sums: &str, name: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        (file == name).then(|| hex::decode(hash).ok()?.try_into().ok()).flatten()
    })
}

/// Reads GitHub's asset digest, `sha256:<64 hex digits>`.
pub fn parse_digest(digest: &str) -> Option<[u8; 32]> {
    let hex = digest.trim().strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    hex::decode(hex).ok()?.try_into().ok()
}

fn fetch_text(url: &str, limit: u64) -> Result<String, UpdateError> {
    let mut text = String::new();
    agent()
        .get(url)
        .call()
        .map_err(net_error)?
        .body_mut()
        .with_config()
        .limit(limit)
        .reader()
        .read_to_string(&mut text)
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    Ok(text)
}

/// The SHA-256 `asset` of `release` must have: GitHub's digest of the
/// asset, or for releases without one, its line in `SHA256SUMS`.
fn asset_hash(release: &Release, asset: &Asset) -> Result<[u8; 32], UpdateError> {
    if let Some(h) = asset.digest.as_deref().and_then(parse_digest) {
        return Ok(h);
    }
    let name = &asset.name;
    let sums_asset = release
        .assets
        .iter()
        .find(|a| a.name == CHECKSUMS)
        .ok_or_else(|| UpdateError::Invalid(format!("release {} publishes no SHA-256 for {name}", release.tag)))?;
    let sums = fetch_text(&sums_asset.url, 1 << 20)?;
    expected_hash(&sums, name).ok_or_else(|| UpdateError::Invalid(format!("{CHECKSUMS} does not list {name}")))
}

/// Where a download of `asset` for `version` goes in the temp folder:
/// `ZipShrink-setup.exe` becomes `ZipShrink-1.2.0-setup.exe`.
fn download_name(asset: &str, version: &str) -> String {
    match asset.strip_prefix("ZipShrink-") {
        Some(rest) => format!("ZipShrink-{version}-{rest}"),
        None => format!("ZipShrink-{version}-{asset}"),
    }
}

/// Downloads this system's package of `release` ([`platform_asset`]: the
/// installer on Windows, the zipped app on macOS) into the temp folder and
/// checks its SHA-256. Returns its path; a download that does not match is
/// deleted.
pub fn download_installer(release: &Release, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, UpdateError> {
    let name = platform_asset().ok_or_else(|| UpdateError::Invalid("there is no update package for this system".into()))?;
    download_verified(release, name, progress)
}

/// Downloads asset `name` of `release` into the temp folder and checks its
/// SHA-256 (see [`asset_hash`]); a download that does not match is deleted.
pub fn download_verified(release: &Release, name: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, UpdateError> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| UpdateError::Invalid(format!("release {} has no {name}", release.tag)))?;
    let want = asset_hash(release, asset)?;
    let dest = std::env::temp_dir().join(download_name(name, &release.version));
    let _ = std::fs::remove_file(&dest);
    let got = match download_to(&asset.url, &dest, progress) {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(&dest);
            return Err(e);
        }
    };
    if got != want {
        let _ = std::fs::remove_file(&dest);
        return Err(UpdateError::Checksum(format!("expected {}, got {}", hex::encode(want), hex::encode(got))));
    }
    Ok(dest)
}

/// Gets a downloaded update ready to install without further waiting: on
/// macOS unpacks the zipped app next to this one and checks it (bundle id,
/// newer version, minimum macOS, code signature, that its command-line tool
/// runs), deletes the zip and returns the checked `.app`; elsewhere returns
/// `path` as it is. [`launch_installer`] takes either.
#[cfg(target_os = "macos")]
pub fn prepare_update(path: &Path) -> Result<PathBuf, UpdateError> {
    mac::prepare(path)
}

/// Gets a downloaded update ready to install: nothing to do on this system.
#[cfg(not(target_os = "macos"))]
pub fn prepare_update(path: &Path) -> Result<PathBuf, UpdateError> {
    Ok(path.to_path_buf())
}

/// Starts the downloaded installer silently (`"path" /S`, plus `/RESTART`
/// to start ZipShrink again when it is done). It waits for ZipShrink to
/// close, so the caller should exit soon after.
#[cfg(not(target_os = "macos"))]
pub fn launch_installer(path: &Path, restart: bool) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(path);
    cmd.arg("/S");
    if restart {
        cmd.arg("/RESTART");
    }
    cmd.spawn().map(|_| ())
}

/// Installs a downloaded update (the zip from [`download_installer`], or the
/// `.app` from [`prepare_update`]): puts the new `ZipShrink.app` in place of
/// this one with an atomic swap, then starts a detached clean-up that waits
/// for this process to exit, removes the old copy and, with `restart`, opens
/// the new one. The caller should exit soon after.
#[cfg(target_os = "macos")]
pub fn launch_installer(path: &Path, restart: bool) -> std::io::Result<()> {
    mac::install(path, restart)
}

/// Removes what an earlier update left behind (staging folders of processes
/// that are gone) and the quarantine flag of this app bundle, in the
/// background. Call it once, from the main app instance, at start-up.
#[cfg(target_os = "macos")]
pub fn mac_cleanup_leftovers() {
    std::thread::spawn(mac::cleanup_leftovers);
}

/// Opens a URL in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Settings shared by the app: stored as JSON in the user's config folder.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "yes")]
    pub check_for_updates: bool,
    #[serde(default)]
    pub last_check: u64,
    #[serde(default)]
    pub skipped_version: Option<String>,
    /// 0 follows the system, 1 is light, 2 is dark.
    #[serde(default)]
    pub appearance: u8,
    #[serde(default)]
    pub reduce_motion: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { check_for_updates: true, last_check: 0, skipped_version: None, appearance: 0, reduce_motion: false }
    }
}

pub fn settings_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("ZipShrink").join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        settings_path().and_then(|p| std::fs::read(p).ok()).and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = settings_path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    /// Whether an automatic check is due (at most once a week).
    pub fn check_due(&self) -> bool {
        self.check_for_updates && now().saturating_sub(self.last_check) >= CHECK_INTERVAL
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert_eq!(parse_version("v2.3"), Some((2, 3, 0)));
    }

    #[test]
    fn checksum_listing() {
        let h = "ab".repeat(32);
        let sums = format!("{h}  ZipShrink-setup.exe\n{}  other.zip\n", "cd".repeat(32));
        assert_eq!(expected_hash(&sums, WINDOWS_INSTALLER), Some([0xab; 32]));
        assert_eq!(expected_hash(&sums, "missing"), None);
        let star = format!("{h} *ZipShrink-setup.exe\r\n");
        assert_eq!(expected_hash(&star, WINDOWS_INSTALLER), Some([0xab; 32]));
    }

    #[test]
    fn digests() {
        let h = "0123456789abcdef".repeat(4);
        let want: [u8; 32] = hex::decode(&h).unwrap().try_into().unwrap();
        assert_eq!(parse_digest(&format!("sha256:{h}")), Some(want));
        assert_eq!(parse_digest(&format!("sha256:{}", h.to_uppercase())), Some(want));
        assert_eq!(parse_digest(&h), None);
        assert_eq!(parse_digest(&format!("sha512:{h}")), None);
        assert_eq!(parse_digest(&format!("sha256:{}", &h[..62])), None);
        assert_eq!(parse_digest(&format!("sha256:{h}00")), None);
        assert_eq!(parse_digest(&format!("sha256:{}zz", &h[..62])), None);
    }

    #[test]
    fn release_assets_from_github_json() {
        let h = "ab".repeat(32);
        let json = format!(
            r#"{{"tag_name":"v1.0.0","html_url":"https://github.com/o/r/releases/tag/v1.0.0","assets":[
              {{"name":"SHA256SUMS","browser_download_url":"https://x/SHA256SUMS","size":100,"digest":null}},
              {{"name":"ZipShrink-setup.exe","browser_download_url":"https://x/ZipShrink-setup.exe","size":5,"digest":"sha256:{h}"}},
              {{"name":"old.zip","browser_download_url":"https://x/old.zip","size":1}}]}}"#
        );
        let r: ApiRelease = serde_json::from_str(&json).unwrap();
        assert_eq!(r.assets.len(), 3);
        assert_eq!(r.assets[0].digest, None);
        assert_eq!(r.assets[2].digest, None);
        let release = Release { version: "1.0.0".into(), tag: r.tag_name, page: r.html_url, notes: String::new(), assets: r.assets };
        let setup = release.assets.iter().find(|a| a.name == WINDOWS_INSTALLER).unwrap();
        assert_eq!(setup.digest.as_deref().and_then(parse_digest), Some([0xab; 32]));
        // With a digest, no SHA256SUMS download is needed.
        assert_eq!(asset_hash(&release, setup).unwrap(), [0xab; 32]);
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), url: format!("https://x/{name}"), size: 1, digest: None }
    }

    #[test]
    fn releases_without_this_systems_package_are_not_updates() {
        let windows_only = [asset(CHECKSUMS), asset(WINDOWS_INSTALLER)];
        let both = [asset(WINDOWS_INSTALLER), asset("ZipShrink.dmg"), asset(MAC_APP_ZIP), asset(CHECKSUMS)];
        assert!(offers(&windows_only, Some(WINDOWS_INSTALLER)));
        assert!(!offers(&windows_only, Some(MAC_APP_ZIP)));
        assert!(offers(&both, Some(MAC_APP_ZIP)));
        assert!(offers(&both, None));
        assert!(offers(&[], None));
        assert_eq!(platform_asset().is_some(), cfg!(any(windows, target_os = "macos")));
    }

    #[test]
    fn download_names() {
        assert_eq!(download_name(WINDOWS_INSTALLER, "1.2.0"), "ZipShrink-1.2.0-setup.exe");
        assert_eq!(download_name(MAC_APP_ZIP, "1.2.0"), "ZipShrink-1.2.0-mac.zip");
        assert_eq!(download_name("other.zip", "1.2.0"), "ZipShrink-1.2.0-other.zip");
    }

    #[test]
    fn methods_explain_themselves() {
        assert!(Method::Installer.installs_itself() && Method::ReplaceApp.installs_itself());
        assert!(!Method::ReleasePage.installs_itself());
        for b in [Blocker::Translocated, Blocker::ReadOnlyVolume, Blocker::NotWritable] {
            assert!(!Method::Blocked(b).installs_itself());
            assert!(!b.advice().is_empty());
        }
        assert!(Method::Blocked(Blocker::Translocated).explain().contains("Applications"));
        assert!(Method::Blocked(Blocker::ReadOnlyVolume).explain().contains("Applications"));
        assert!(Method::Blocked(Blocker::NotWritable).explain().contains("release page"));
        assert!(Blocker::Translocated.advice().contains("Move ZipShrink to Applications"));
        if !cfg!(target_os = "macos") {
            assert_ne!(method(), Method::ReplaceApp);
        }
    }

    #[test]
    fn settings_defaults_and_old_files() {
        let s: Settings = serde_json::from_str(r#"{"check_for_updates":false,"last_check":5}"#).unwrap();
        assert!(!s.check_for_updates);
        assert_eq!((s.appearance, s.reduce_motion), (0, false));
        let d = Settings::default();
        assert!(d.check_for_updates && d.check_due());
        let recent = Settings { last_check: now() - CHECK_INTERVAL + 60, ..Settings::default() };
        assert!(!recent.check_due());
        let old = Settings { last_check: now() - CHECK_INTERVAL, ..Settings::default() };
        assert!(old.check_due());
    }
}
