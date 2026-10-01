//! Live checks against the real yt-dlp and YouTube.
//!
//! They need network access and yt-dlp (plus Deno, which YouTube requires,
//! and FFmpeg), so they are `#[ignore]`d. CI runs them weekly through
//! `.github/workflows/upstream.yml`. To run them locally:
//!
//! ```text
//! cargo test --release -- --ignored live_
//! ```
//!
//! Set `YTDLP` to the path of a specific yt-dlp executable; otherwise the one
//! on the PATH is used.

use std::process::Command;

use crate::args::download_args;
use crate::media::parse_media_info;
use crate::settings::{AUDIO_FORMATS, Mode, Settings, VIDEO_QUALITIES};

/// Long-lived videos with different format sets: an old 240p video, one with
/// chapters and a "Premium" 1080p format, and a 4K music video.
const VIDEOS: &[&str] = &[
    "https://www.youtube.com/watch?v=jNQXAC9IVRw",
    "https://www.youtube.com/watch?v=b1Fo_M_tj6w",
    "https://www.youtube.com/watch?v=tpdzUk_8DJg",
];

/// Clip passed to `download_args`: start and optional end, in seconds.
type Clip = Option<(u64, Option<u64>)>;

/// Video with chapters, used to check the download arguments.
const CHAPTERS_VIDEO: &str = "https://www.youtube.com/watch?v=b1Fo_M_tj6w";

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Runs yt-dlp and returns its stdout. `Ok(None)` means YouTube asked to
/// confirm we're not a bot (common on CI runners' IP addresses), which says
/// nothing about this app, so callers skip instead of failing.
fn ytdlp(args: &[String]) -> Result<Option<String>, String> {
    let exe = std::env::var("YTDLP").unwrap_or_else(|_| "yt-dlp".to_owned());
    let out = Command::new(&exe)
        .args(args)
        .output()
        .map_err(|e| format!("could not run {exe}: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
    } else if stderr.contains("confirm you") && stderr.contains("bot") {
        Ok(None)
    } else {
        Err(format!("yt-dlp {} failed:\n{stderr}", args.join(" ")))
    }
}

/// Shows up as a warning annotation in the GitHub Actions run.
fn warn_bot_check(url: &str) {
    println!("::warning::YouTube asked to confirm we're not a bot for {url}; skipped it");
}

/// Every value that follows `flag` in `args`.
fn values_after(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].clone())
        .collect()
}

/// Size of the formats yt-dlp picked, reported the way `estimated_size` does:
/// the sum of the picked formats, or `None` if any of them has no size.
fn picked_size(picked: &serde_json::Value) -> Option<u64> {
    let size = |f: &serde_json::Value| {
        f.get("filesize")
            .and_then(|x| x.as_u64())
            .or_else(|| f.get("filesize_approx").and_then(|x| x.as_u64()))
    };
    match picked.get("requested_formats").and_then(|r| r.as_array()) {
        Some(formats) => formats.iter().map(size).sum(),
        None => size(picked),
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("yt-downloader-live-tests");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
#[ignore = "needs network access and yt-dlp"]
fn live_estimate_matches_ytdlp_format_choice() {
    let info_file = temp_dir().join("info.json");
    let mut failures = Vec::new();

    for url in VIDEOS {
        let Some(json) = ytdlp(&strings(&["-J", "--no-warnings", "--no-playlist", url])).unwrap()
        else {
            warn_bot_check(url);
            continue;
        };
        let info = parse_media_info(&serde_json::from_str(&json).expect("yt-dlp -J output"));
        std::fs::write(&info_file, &json).expect("write info.json");

        let mut cases: Vec<(String, Settings)> = VIDEO_QUALITIES
            .iter()
            .enumerate()
            .map(|(i, (label, _))| {
                let s = Settings {
                    mode: Mode::Video,
                    video_quality: i,
                    ..Settings::default()
                };
                (format!("video, {label}"), s)
            })
            .collect();
        let original = AUDIO_FORMATS.iter().position(|f| f.1 == "best").unwrap();
        cases.push((
            "audio, original format".to_owned(),
            Settings {
                mode: Mode::Audio,
                audio_format: original,
                ..Settings::default()
            },
        ));

        for (label, settings) in cases {
            // Ask yt-dlp which formats it picks with the exact -f/-S options
            // the app passes when downloading, without downloading anything.
            let args = download_args(&settings, None, &[url.to_string()]);
            let mut query = strings(&["--no-warnings", "-j", "--load-info-json"]);
            query.push(info_file.to_string_lossy().into_owned());
            for flag in ["-f", "-S"] {
                for value in values_after(&args, flag) {
                    query.push(flag.to_owned());
                    query.push(value);
                }
            }
            let picked = ytdlp(&query)
                .unwrap()
                .expect("no bot check when reading a saved info.json");
            let picked: serde_json::Value =
                serde_json::from_str(&picked).expect("yt-dlp -j output");

            let expected = picked_size(&picked);
            let estimated = info.estimated_size(&settings);
            if estimated != expected {
                failures.push(format!(
                    "{url} [{label}]: the app estimates {estimated:?}, \
                     yt-dlp picks {} = {expected:?}",
                    picked["format_id"]
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "the size estimate no longer matches yt-dlp's format choice:\n{}",
        failures.join("\n")
    );
}

#[test]
#[ignore = "needs network access and yt-dlp"]
fn live_ytdlp_accepts_download_args() {
    let out_dir = temp_dir();
    let base = |mode| Settings {
        mode,
        out_dir: out_dir.clone(),
        ..Settings::default()
    };
    let audio_format = |ext: &str| AUDIO_FORMATS.iter().position(|f| f.1 == ext).unwrap();

    let cases: Vec<(&str, Settings, Clip)> = vec![
        ("video, best quality", base(Mode::Video), None),
        (
            "video, 720p, whole playlist",
            Settings {
                video_quality: 4,
                playlist: true,
                ..base(Mode::Video)
            },
            None,
        ),
        ("audio, MP3", base(Mode::Audio), None),
        (
            "audio, WAV",
            Settings {
                audio_format: audio_format("wav"),
                ..base(Mode::Audio)
            },
            None,
        ),
        ("video, clip", base(Mode::Video), Some((10, Some(20)))),
        (
            "audio, clip to the end",
            base(Mode::Audio),
            Some((60, None)),
        ),
        (
            "audio, split by chapters",
            Settings {
                split_chapters: true,
                ..base(Mode::Audio)
            },
            None,
        ),
    ];

    let mut failures = Vec::new();
    for (label, settings, cut) in cases {
        let mut args = download_args(&settings, cut, &[CHAPTERS_VIDEO.to_owned()]);
        // Run everything except the actual download.
        args.insert(0, "--simulate".to_owned());
        match ytdlp(&args) {
            Ok(Some(_)) => {}
            Ok(None) => {
                warn_bot_check(CHAPTERS_VIDEO);
                return;
            }
            Err(e) => failures.push(format!("[{label}] {e}")),
        }
    }

    assert!(
        failures.is_empty(),
        "yt-dlp rejected the app's download arguments:\n{}",
        failures.join("\n\n")
    );
}
