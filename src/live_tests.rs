//! Checks against the real yt-dlp, FFmpeg and YouTube.
//!
//! They need external tools, so they are `#[ignore]`d and run explicitly:
//!
//! - `tools_*`: yt-dlp and FFmpeg on a short video generated locally. No
//!   internet needed. CI runs them weekly with the latest yt-dlp
//!   (`.github/workflows/upstream.yml`):
//!
//!   ```text
//!   cargo test --release -- --ignored tools_
//!   ```
//!
//! - `live_*`: the real YouTube. YouTube asks datacenter IPs (such as CI
//!   runners) to prove they aren't bots, so these run weekly on the
//!   maintainer's PC instead (`tools/live-check.ps1`):
//!
//!   ```text
//!   cargo test --release -- --ignored live_
//!   ```
//!
//! Set `YTDLP` / `FFMPEG` to use specific executables; otherwise the ones on
//! the PATH are used.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::args::download_args;
use crate::media::parse_media_info;
use crate::settings::{AUDIO_FORMATS, Mode, Settings, VIDEO_QUALITIES};

/// Long-lived YouTube videos with different format sets: an old 240p video,
/// one with chapters and a "Premium" 1080p format, and a 4K music video.
const VIDEOS: &[&str] = &[
    "https://www.youtube.com/watch?v=jNQXAC9IVRw",
    "https://www.youtube.com/watch?v=b1Fo_M_tj6w",
    "https://www.youtube.com/watch?v=tpdzUk_8DJg",
];

/// Clip passed to `download_args`: start and optional end, in seconds.
type Clip = Option<(u64, Option<u64>)>;

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn tool(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_owned())
}

/// Runs yt-dlp and returns its stdout. `Ok(None)` means YouTube asked to
/// confirm we're not a bot, which says nothing about this app.
fn ytdlp(args: &[String]) -> Result<Option<String>, String> {
    let exe = tool("YTDLP", "yt-dlp");
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

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("yt-downloader-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// A 5-second video with an audio track, generated once with FFmpeg, as a
/// `file://` URL that yt-dlp can "download" with `--enable-file-urls`.
fn sample_video_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let path = temp_dir("sample").join("sample.mp4");
        let status = Command::new(tool("FFMPEG", "ffmpeg"))
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=duration=5:size=320x240:rate=25")
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=5"])
            .args(["-c:v", "libx264", "-c:a", "aac", "-shortest"])
            .arg(&path)
            .status()
            .expect("run ffmpeg to create the sample video");
        assert!(status.success(), "ffmpeg could not create the sample video");
        format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
    })
}

/// The app's arguments for the sample video, as a yt-dlp command line.
fn sample_args(settings: &Settings, cut: Clip) -> Vec<String> {
    let mut args = download_args(settings, cut, &[sample_video_url().to_owned()]);
    args.insert(0, "--enable-file-urls".to_owned());
    args
}

fn audio_format(ext: &str) -> usize {
    AUDIO_FORMATS.iter().position(|f| f.1 == ext).unwrap()
}

fn settings(mode: Mode, out_dir: &Path) -> Settings {
    Settings {
        mode,
        out_dir: out_dir.to_owned(),
        ..Settings::default()
    }
}

#[test]
#[ignore = "needs yt-dlp and FFmpeg"]
fn tools_ytdlp_accepts_download_args() {
    let out = temp_dir("accepts");
    let mut cases: Vec<(String, Settings, Clip)> = vec![
        (
            "video, best quality".into(),
            settings(Mode::Video, &out),
            None,
        ),
        (
            "video, 720p, whole playlist".into(),
            Settings {
                video_quality: 4,
                playlist: true,
                ..settings(Mode::Video, &out)
            },
            None,
        ),
        (
            "video, clip".into(),
            settings(Mode::Video, &out),
            Some((1, Some(3))),
        ),
        (
            "audio, clip to the end".into(),
            settings(Mode::Audio, &out),
            Some((2, None)),
        ),
        (
            "audio, split by chapters".into(),
            Settings {
                split_chapters: true,
                ..settings(Mode::Audio, &out)
            },
            None,
        ),
    ];
    for (i, (label, _)) in AUDIO_FORMATS.iter().enumerate() {
        let s = Settings {
            audio_format: i,
            ..settings(Mode::Audio, &out)
        };
        cases.push((format!("audio, {label}"), s, None));
    }

    let mut failures = Vec::new();
    for (label, settings, cut) in cases {
        // Validates every option without downloading anything.
        let mut args = sample_args(&settings, cut);
        args.insert(0, "--simulate".to_owned());
        if let Err(e) = ytdlp(&args) {
            failures.push(format!("[{label}] {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "yt-dlp rejected the app's download arguments:\n{}",
        failures.join("\n\n")
    );
}

#[test]
#[ignore = "needs yt-dlp and FFmpeg"]
fn tools_downloads_produce_the_expected_files() {
    // Clips can't be tested here: yt-dlp only cuts streams downloaded over
    // the network, not local files.
    let cases = [
        ("video", Mode::Video, "mp4", "sample.mp4"),
        ("audio, MP3", Mode::Audio, "mp3", "sample.mp3"),
        ("audio, M4A", Mode::Audio, "m4a", "sample.m4a"),
        ("audio, WAV", Mode::Audio, "wav", "sample.wav"),
    ];
    let mut failures = Vec::new();
    for (label, mode, ext, expected) in cases {
        // A folder per case: audio extraction deletes the intermediate .mp4.
        let out = temp_dir(&format!("download-{ext}"));
        let mut s = settings(mode, &out);
        if mode == Mode::Audio {
            s.audio_format = audio_format(ext);
        }
        if let Err(e) = ytdlp(&sample_args(&s, None)) {
            failures.push(format!("[{label}] {e}"));
            continue;
        }
        let mut files: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        if files != [expected] {
            failures.push(format!("[{label}] expected only {expected}, got {files:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "downloads didn't produce the expected files:\n{}",
        failures.join("\n")
    );
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

#[test]
#[ignore = "needs yt-dlp, Deno and access to YouTube"]
fn live_estimate_matches_ytdlp_format_choice() {
    let info_file = temp_dir("estimate").join("info.json");
    let mut failures = Vec::new();
    let mut checked = 0;

    for url in VIDEOS {
        let Some(json) = ytdlp(&strings(&["-J", "--no-warnings", "--no-playlist", url])).unwrap()
        else {
            println!("YouTube asked to confirm we're not a bot for {url}; skipped it");
            continue;
        };
        checked += 1;
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
        cases.push((
            "audio, original format".to_owned(),
            Settings {
                mode: Mode::Audio,
                audio_format: audio_format("best"),
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

    // A run where YouTube blocked every video tested nothing: fail instead of
    // passing silently.
    assert!(
        checked > 0,
        "YouTube asked to confirm we're not a bot for every video; nothing was checked"
    );
    assert!(
        failures.is_empty(),
        "the size estimate no longer matches yt-dlp's format choice:\n{}",
        failures.join("\n")
    );
}
