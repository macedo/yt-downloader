//! Updating the app itself from its GitHub Releases: check for a newer
//! version, download its installer, verify it against the release's
//! SHA256SUMS.txt and run it silently. The installer closes the app if it is
//! still open, updates it in place and starts it again (`/RESTARTAPP=1`).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui;
use sha2::{Digest, Sha256};

use crate::net::{download_agent, http_agent};
use crate::update::is_newer;

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/macedo/yt-downloader/releases/latest";
/// The installer and its checksum are only taken from this repository's releases.
const DOWNLOAD_PREFIX: &str = "https://github.com/macedo/yt-downloader/releases/download/";
/// Registry key Inno Setup writes for the app (`AppId` in installer/yt-downloader.iss).
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{3F85C58E-589A-4F2F-A7A1-98BF8E6C2EEF}_is1";

/// A published version of the app and where to get it.
#[derive(Clone, Debug, PartialEq)]
pub struct AppRelease {
    pub version: String,
    /// The release page, with its notes.
    pub notes_url: String,
    installer_name: String,
    installer_url: String,
    sums_url: String,
}

/// Reads GitHub's "latest release" JSON. `None` if the installer or its
/// checksum file is missing, or points outside this repository.
fn parse_release(v: &serde_json::Value) -> Option<AppRelease> {
    let version = v
        .get("tag_name")?
        .as_str()?
        .trim_start_matches('v')
        .to_owned();
    let notes_url = v.get("html_url")?.as_str()?.to_owned();
    let assets = v.get("assets")?.as_array()?;
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(name))
            .and_then(|a| a.get("browser_download_url")?.as_str())
            .filter(|url| url.starts_with(DOWNLOAD_PREFIX))
            .map(str::to_owned)
    };
    let installer_name = format!("YT-Downloader-Setup-{version}.exe");
    Some(AppRelease {
        installer_url: url_of(&installer_name)?,
        sums_url: url_of("SHA256SUMS.txt")?,
        installer_name,
        version,
        notes_url,
    })
}

/// Checks GitHub in the background. Sends `Some` only when the latest release
/// is newer than this build; offline or on any error, `None`.
pub fn check_for_update(ctx: egui::Context) -> Receiver<Option<AppRelease>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let latest = (|| {
            let mut resp = http_agent()
                .get(LATEST_RELEASE_URL)
                .header("Accept", "application/vnd.github+json")
                .call()
                .ok()?;
            let body = resp.body_mut().read_to_string().ok()?;
            parse_release(&serde_json::from_str(&body).ok()?)
        })();
        let _ = tx.send(latest.filter(|r| is_newer(&r.version, env!("CARGO_PKG_VERSION"))));
        ctx.request_repaint();
    });
    rx
}

/// Whether this running copy is the one the installer put in place, so the
/// installer can update it. A copy run from elsewhere (e.g. a build folder)
/// would be left as it is, so it gets a link to the release instead.
pub fn running_installed_copy() -> bool {
    let (Some(dir), Ok(exe)) = (installed_dir(), std::env::current_exe()) else {
        return false;
    };
    let canonical = |p: &Path| std::fs::canonicalize(p).ok();
    match (exe.parent().and_then(canonical), canonical(&dir)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn installed_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(UNINSTALL_KEY)
            .ok()?;
        let dir: String = key.get_value("InstallLocation").ok()?;
        Some(PathBuf::from(dir))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub enum UpdateMsg {
    /// 0.0 to 1.0, when the size is known.
    Progress(f32),
    /// The verified installer, ready to run.
    Ready(PathBuf),
    Failed(String),
}

/// Downloads the release's installer to the temp folder and checks its
/// SHA-256 against the release's SHA256SUMS.txt.
pub fn download_update(release: AppRelease, ctx: egui::Context) -> Receiver<UpdateMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let progress_tx = tx.clone();
        let progress_ctx = ctx.clone();
        let report = move |fraction: f32| {
            let _ = progress_tx.send(UpdateMsg::Progress(fraction));
            progress_ctx.request_repaint();
        };
        let msg = match download_and_verify(&release, report) {
            Ok(path) => UpdateMsg::Ready(path),
            Err(e) => UpdateMsg::Failed(e),
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
    rx
}

fn download_and_verify(release: &AppRelease, report: impl Fn(f32)) -> Result<PathBuf, String> {
    let sums = http_agent()
        .get(&release.sums_url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| format!("couldn't download SHA256SUMS.txt: {e}"))?;
    let expected = expected_sha256(&sums, &release.installer_name)
        .ok_or("the release has no checksum for its installer")?;

    let mut resp = download_agent()
        .get(&release.installer_url)
        .call()
        .map_err(|e| format!("couldn't download the installer: {e}"))?;
    let total = resp.body().content_length();
    let path = std::env::temp_dir().join(&release.installer_name);
    let mut file = File::create(&path).map_err(|e| format!("couldn't save the installer: {e}"))?;
    let mut reader = resp.body_mut().as_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut done, mut last_pct) = (0u64, 0u64);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("the download was interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("couldn't save the installer: {e}"))?;
        done += n as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            let pct = done * 100 / total;
            if pct > last_pct {
                last_pct = pct;
                report(done as f32 / total as f32);
            }
        }
    }
    drop(file);

    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        let _ = std::fs::remove_file(&path);
        return Err(
            "the installer's checksum doesn't match the release; nothing was installed".into(),
        );
    }
    Ok(path)
}

/// The SHA-256 listed for `file_name` in a SHA256SUMS.txt (`<hash>  <name>` per line).
fn expected_sha256(sums: &str, file_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        (name.trim() == file_name
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

/// Starts the installer silently, without its "install dependencies" task
/// (they are already installed). It closes the app if it is still running,
/// updates it, and starts it again. The caller should close the app right
/// after.
pub fn run_installer(path: &Path) -> std::io::Result<()> {
    Command::new(path)
        .args([
            "/SP-",
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
            "/MERGETASKS=!deps",
            "/RESTARTAPP=1",
        ])
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(installer_url: &str) -> serde_json::Value {
        serde_json::json!({
            "tag_name": "v1.3.0",
            "html_url": "https://github.com/macedo/yt-downloader/releases/tag/v1.3.0",
            "assets": [
                {"name": "SHA256SUMS.txt",
                 "browser_download_url": "https://github.com/macedo/yt-downloader/releases/download/v1.3.0/SHA256SUMS.txt"},
                {"name": "YT-Downloader-Setup-1.3.0.exe", "browser_download_url": installer_url}
            ]
        })
    }

    #[test]
    fn parses_the_latest_release() {
        let url = "https://github.com/macedo/yt-downloader/releases/download/v1.3.0/YT-Downloader-Setup-1.3.0.exe";
        let release = parse_release(&release_json(url)).unwrap();
        assert_eq!(release.version, "1.3.0");
        assert_eq!(release.installer_name, "YT-Downloader-Setup-1.3.0.exe");
        assert_eq!(release.installer_url, url);
        assert!(release.sums_url.ends_with("/v1.3.0/SHA256SUMS.txt"));
    }

    #[test]
    fn rejects_installers_from_elsewhere_or_missing() {
        let elsewhere = "https://example.com/YT-Downloader-Setup-1.3.0.exe";
        assert_eq!(parse_release(&release_json(elsewhere)), None);

        let mut no_installer =
            release_json("https://github.com/macedo/yt-downloader/releases/download/v1.3.0/x");
        no_installer["assets"].as_array_mut().unwrap().pop();
        assert_eq!(parse_release(&no_installer), None);
    }

    #[test]
    fn reads_the_installer_checksum() {
        let hash = "22eb60286e78e86f2a4d4a8e95ba8a7bea33a8bf95d6e27961019d655d7ea462";
        let sums = format!("{}  YT-Downloader-Setup-1.1.0.exe\n", hash.to_uppercase());
        assert_eq!(
            expected_sha256(&sums, "YT-Downloader-Setup-1.1.0.exe").as_deref(),
            Some(hash)
        );
        assert_eq!(
            expected_sha256(&sums, "YT-Downloader-Setup-9.9.9.exe"),
            None
        );
        assert_eq!(
            expected_sha256(
                "not-a-hash  YT-Downloader-Setup-1.1.0.exe",
                "YT-Downloader-Setup-1.1.0.exe"
            ),
            None
        );
    }

    #[test]
    fn semver_versions_compare_numerically() {
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.2.0", "1.2.0"));
    }

    #[test]
    #[ignore = "needs access to GitHub"]
    fn live_downloads_and_verifies_the_latest_installer() {
        let mut resp = http_agent()
            .get(LATEST_RELEASE_URL)
            .header("Accept", "application/vnd.github+json")
            .call()
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&resp.body_mut().read_to_string().unwrap()).unwrap();
        let release =
            parse_release(&json).expect("the latest release has an installer and SHA256SUMS.txt");
        let progress = std::cell::Cell::new(0.0f32);
        let path =
            download_and_verify(&release, |f| progress.set(f)).expect("download and checksum");
        assert!(path.is_file());
        assert!(progress.get() > 0.99, "progress reached {}", progress.get());
        std::fs::remove_file(path).unwrap();
    }
}
