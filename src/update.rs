//! Checking for new yt-dlp versions.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui;

use crate::net::http_agent;
use crate::process::background_command;

const YTDLP_LATEST_RELEASE_URL: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";

/// Result of the background yt-dlp version check.
pub struct VersionInfo {
    pub installed: Option<String>,
    pub latest: Option<String>,
}

pub fn check_ytdlp_version(exe: PathBuf, ctx: egui::Context) -> Receiver<VersionInfo> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let installed = installed_ytdlp_version(&exe);
        let latest = latest_ytdlp_version();
        let _ = tx.send(VersionInfo { installed, latest });
        ctx.request_repaint();
    });
    rx
}

fn installed_ytdlp_version(exe: &Path) -> Option<String> {
    let out = background_command(exe)
        .arg("--version")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !version.is_empty()).then_some(version)
}

/// Latest version published on GitHub. Without internet, there is simply no notice.
fn latest_ytdlp_version() -> Option<String> {
    let mut resp = http_agent()
        .get(YTDLP_LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?;
    let body = resp.body_mut().read_to_string().ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("tag_name")?.as_str().map(str::to_owned)
}

/// yt-dlp versions are dates (2026.08.19), sometimes with a suffix (.1).
pub fn is_newer(latest: &str, installed: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(installed)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_ytdlp_versions() {
        assert!(is_newer("2026.09.02", "2026.08.19"));
        assert!(is_newer("2026.08.19.1", "2026.08.19"));
        assert!(is_newer("2027.01.01", "2026.12.31"));
        assert!(!is_newer("2026.08.19", "2026.08.19"));
        assert!(!is_newer("2026.08.19", "2026.08.19.1"));
        assert!(!is_newer("2026.07.30", "2026.08.19"));
    }
}
