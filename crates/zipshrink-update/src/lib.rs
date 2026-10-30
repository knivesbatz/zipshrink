//! Checks GitHub Releases for a newer ZipShrink and fetches its installer.
//!
//! Nothing is ever installed silently. A check only reads the public
//! release list; a download is started by the user, saved to the temp
//! folder, and its SHA-256 compared with the `SHA256SUMS` file published in
//! the same release before it is run. The installer then runs with its
//! normal interface. Portable copies and non-Windows systems are pointed to
//! the release page instead.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `owner/repo` the releases come from, fixed at build time by the release
/// workflow (`ZIPSHRINK_UPDATE_REPO=${{ github.repository }}`).
pub const REPO: &str = match option_env!("ZIPSHRINK_UPDATE_REPO") {
    Some(r) => r,
    None => "knivesbatz/zipshrink",
};
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const WINDOWS_INSTALLER: &str = "ZipShrink-x64-setup.exe";
pub const CHECKSUMS: &str = "SHA256SUMS";
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
    Invalid(String),
    Checksum(String),
    Io(std::io::Error),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "could not reach the update server: {e}"),
            UpdateError::Invalid(e) => write!(f, "unexpected update information: {e}"),
            UpdateError::Checksum(e) => write!(f, "the download failed its integrity check and was deleted: {e}"),
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

/// Returns the latest release if it is newer than this build.
pub fn check() -> Result<Option<Release>, UpdateError> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent().get(&url).header("Accept", "application/vnd.github+json").call();
    let mut resp = match resp {
        Ok(r) => r,
        // No release has been published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(UpdateError::Network(e.to_string())),
    };
    let r: ApiRelease = resp.body_mut().read_json().map_err(|e| UpdateError::Invalid(e.to_string()))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let version = r.tag_name.trim_start_matches(['v', 'V']).to_string();
    if parse_version(&version).is_none() {
        return Err(UpdateError::Invalid(format!("release tag {:?} is not a version", r.tag_name)));
    }
    if !is_newer(&version, CURRENT) {
        return Ok(None);
    }
    Ok(Some(Release { version, tag: r.tag_name, page: r.html_url, notes: r.body.unwrap_or_default(), assets: r.assets }))
}

/// How this copy can be updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Download and run the Windows installer.
    Installer,
    /// Open the release page (portable copies, macOS, Linux).
    ReleasePage,
}

pub fn method() -> Method {
    if cfg!(windows) && !is_portable() {
        Method::Installer
    } else {
        Method::ReleasePage
    }
}

pub fn is_portable() -> bool {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(PORTABLE_MARKER).exists())).unwrap_or(false)
}

fn download_to(url: &str, dest: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<[u8; 32], UpdateError> {
    let mut resp = agent().get(url).call().map_err(|e| UpdateError::Network(e.to_string()))?;
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

/// Downloads the Windows installer of `release` into the temp folder and
/// checks it against the release's SHA256SUMS. Returns its path.
pub fn download_installer(release: &Release, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, UpdateError> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == WINDOWS_INSTALLER)
        .ok_or_else(|| UpdateError::Invalid(format!("release {} has no {WINDOWS_INSTALLER}", release.tag)))?;
    let sums_asset = release
        .assets
        .iter()
        .find(|a| a.name == CHECKSUMS)
        .ok_or_else(|| UpdateError::Invalid(format!("release {} has no {CHECKSUMS}", release.tag)))?;
    let mut sums = String::new();
    agent()
        .get(&sums_asset.url)
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .reader()
        .read_to_string(&mut sums)
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    let want = expected_hash(&sums, WINDOWS_INSTALLER)
        .ok_or_else(|| UpdateError::Invalid(format!("{CHECKSUMS} does not list {WINDOWS_INSTALLER}")))?;
    let dest = std::env::temp_dir().join(format!("ZipShrink-{}-setup.exe", release.version));
    let got = download_to(&asset.url, &dest, progress)?;
    if got != want {
        let _ = std::fs::remove_file(&dest);
        return Err(UpdateError::Checksum(format!("expected {}, got {}", hex::encode(want), hex::encode(got))));
    }
    Ok(dest)
}

/// Starts the downloaded installer; the caller should exit right after.
pub fn launch_installer(path: &Path) -> std::io::Result<()> {
    std::process::Command::new(path).spawn().map(|_| ())
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
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { check_for_updates: true, last_check: 0, skipped_version: None }
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

    /// Whether an automatic check is due (at most once a day).
    pub fn check_due(&self) -> bool {
        self.check_for_updates && now().saturating_sub(self.last_check) >= 24 * 3600
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
        let sums = format!("{h}  ZipShrink-x64-setup.exe\n{}  other.zip\n", "cd".repeat(32));
        assert_eq!(expected_hash(&sums, WINDOWS_INSTALLER), Some([0xab; 32]));
        assert_eq!(expected_hash(&sums, "missing"), None);
        let star = format!("{h} *ZipShrink-x64-setup.exe\n");
        assert_eq!(expected_hash(&star, WINDOWS_INSTALLER), Some([0xab; 32]));
    }
}
