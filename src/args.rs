//! yt-dlp download arguments.

use crate::clip::format_time;
use crate::settings::{AUDIO_FORMATS, AUDIO_QUALITIES, Mode, Settings, VIDEO_QUALITIES};

/// Arguments for downloading `urls` with yt-dlp using the given settings and,
/// optionally, only the clip `cut` (start, optional end) in seconds.
pub fn download_args(
    s: &Settings,
    cut: Option<(u64, Option<u64>)>,
    urls: &[String],
) -> Vec<String> {
    let output = match cut {
        Some((start, end)) => format!(
            "%(title)s (clip {}-{}).%(ext)s",
            format_time(start),
            end.map_or("end".to_owned(), format_time)
        ),
        None => "%(title)s.%(ext)s".to_owned(),
    };
    let mut a: Vec<String> =
        vec![
        "--newline".into(),
        "--no-colors".into(),
        "--no-mtime".into(),
        "--progress-template".into(),
        "download:[PROG]%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s"
            .into(),
        "-P".into(),
        s.out_dir.to_string_lossy().into_owned(),
        "-o".into(),
        output,
        if s.playlist { "--yes-playlist" } else { "--no-playlist" }.into(),
        "--embed-metadata".into(),
    ];

    match s.mode {
        Mode::Video => {
            let sort = match VIDEO_QUALITIES[s.video_quality].1 {
                Some(h) => format!("res:{h},ext:mp4:m4a"),
                None => "res,ext:mp4:m4a".to_owned(),
            };
            a.extend([
                "-f".into(),
                "bv*+ba/b".into(),
                "-S".into(),
                sort,
                "--merge-output-format".into(),
                "mp4".into(),
            ]);
            if s.embed_thumbnail {
                a.push("--embed-thumbnail".into());
            }
        }
        Mode::Audio => {
            let fmt = AUDIO_FORMATS[s.audio_format].1;
            a.extend([
                "-f".into(),
                "ba/b".into(),
                "-x".into(),
                "--audio-format".into(),
                fmt.into(),
                "--audio-quality".into(),
                AUDIO_QUALITIES[s.audio_quality].1.into(),
            ]);
            // WAV doesn't support embedded cover art.
            if s.embed_thumbnail && fmt != "wav" {
                a.extend([
                    "--embed-thumbnail".into(),
                    "--convert-thumbnails".into(),
                    "jpg".into(),
                ]);
            }
        }
    }

    if let Some((start, end)) = cut {
        a.extend([
            "--download-sections".into(),
            format!(
                "*{start}-{}",
                end.map_or("inf".to_owned(), |e| e.to_string())
            ),
            // Without this the clip starts at the previous keyframe (audio came
            // out a few seconds too long).
            "--force-keyframes-at-cuts".into(),
        ]);
    } else if s.split_chapters {
        a.extend([
            "--split-chapters".into(),
            "-o".into(),
            "chapter:%(title)s/%(section_number)02d - %(section_title)s.%(ext)s".into(),
        ]);
    }

    a.push("--".into());
    a.extend(urls.iter().cloned());
    a
}
