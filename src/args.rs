//! yt-dlp download arguments.

use crate::clip::format_time;
use crate::settings::{AUDIO_FORMATS, AUDIO_QUALITIES, Mode, Settings, VIDEO_QUALITIES};

/// Which part of each video to download.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Whole,
    /// Start and optional end, in seconds.
    Clip {
        start: u64,
        end: Option<u64>,
    },
    /// Chapters picked by title in the link preview; each becomes its own file.
    Chapters(Vec<String>),
}

/// Arguments for downloading `part` of each of `urls` with yt-dlp, using the
/// given settings.
pub fn download_args(s: &Settings, part: &Part, urls: &[String]) -> Vec<String> {
    let output = match part {
        Part::Whole => "%(title)s.%(ext)s".to_owned(),
        Part::Clip { start, end } => format!(
            "%(title)s (clip {}-{}).%(ext)s",
            format_time(*start),
            end.map_or("end".to_owned(), format_time)
        ),
        // section_number is 0-based: "Lesson - 02 - Solo.mp3".
        Part::Chapters(_) => {
            "%(title)s - %(section_number+1)02d - %(section_title)s.%(ext)s".to_owned()
        }
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

    match part {
        Part::Clip { start, end } => {
            a.extend([
                "--download-sections".into(),
                format!(
                    "*{start}-{}",
                    end.map_or("inf".to_owned(), |e| e.to_string())
                ),
                // Without this the clip starts at the previous keyframe (audio
                // came out a few seconds too long).
                "--force-keyframes-at-cuts".into(),
            ]);
        }
        Part::Chapters(titles) => {
            // By title rather than by time, so yt-dlp fills %(section_title)s
            // for the file name. The title is a regex for yt-dlp: anchored and
            // escaped, so it matches that chapter's whole name only.
            for title in titles {
                a.push("--download-sections".into());
                a.push(format!("^{}$", regex_escape(title)));
            }
            a.push("--force-keyframes-at-cuts".into());
        }
        Part::Whole if s.split_chapters => {
            a.extend([
                "--split-chapters".into(),
                "-o".into(),
                "chapter:%(title)s/%(section_number)02d - %(section_title)s.%(ext)s".into(),
            ]);
        }
        Part::Whole => {}
    }

    a.push("--".into());
    a.extend(urls.iter().cloned());
    a
}

/// Escapes the characters that are special in Python regular expressions
/// (yt-dlp matches chapter names with `re.search`).
fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if r"\.^$*+?()[]{}|#&~-".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Part, download_args};
    use crate::settings::{AUDIO_FORMATS, AUDIO_QUALITIES, Mode, Settings, VIDEO_QUALITIES};

    fn settings(mode: Mode) -> Settings {
        Settings {
            mode,
            out_dir: PathBuf::from(r"C:\out"),
            ..Settings::default()
        }
    }

    fn urls(list: &[&str]) -> Vec<String> {
        list.iter().map(|u| u.to_string()).collect()
    }

    /// Every value that follows `flag` in `args`.
    fn values_after<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].as_str())
            .collect()
    }

    fn has(args: &[String], flag: &str) -> bool {
        args.iter().any(|a| a == flag)
    }

    fn audio_format_index(ext: &str) -> usize {
        AUDIO_FORMATS.iter().position(|f| f.1 == ext).unwrap()
    }

    #[test]
    fn default_video_download_full_args() {
        let args = download_args(
            &settings(Mode::Video),
            &Part::Whole,
            &urls(&["https://a", "https://b"]),
        );
        assert_eq!(
            args,
            [
                "--newline",
                "--no-colors",
                "--no-mtime",
                "--progress-template",
                "download:[PROG]%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s",
                "-P",
                r"C:\out",
                "-o",
                "%(title)s.%(ext)s",
                "--no-playlist",
                "--embed-metadata",
                "-f",
                "bv*+ba/b",
                "-S",
                "res,ext:mp4:m4a",
                "--merge-output-format",
                "mp4",
                "--embed-thumbnail",
                "--",
                "https://a",
                "https://b",
            ]
        );
    }

    #[test]
    fn video_quality_limits_the_resolution() {
        for (i, (label, height)) in VIDEO_QUALITIES.iter().enumerate() {
            let mut s = settings(Mode::Video);
            s.video_quality = i;
            let expected = match height {
                Some(h) => format!("res:{h},ext:mp4:m4a"),
                None => "res,ext:mp4:m4a".to_owned(),
            };
            let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
            assert_eq!(values_after(&args, "-S"), [expected.as_str()], "{label}");
        }
    }

    #[test]
    fn audio_download_extracts_with_format_and_quality() {
        for (fi, (_, ext)) in AUDIO_FORMATS.iter().enumerate() {
            for (qi, (_, quality)) in AUDIO_QUALITIES.iter().enumerate() {
                let mut s = settings(Mode::Audio);
                s.audio_format = fi;
                s.audio_quality = qi;
                let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
                assert_eq!(values_after(&args, "-f"), ["ba/b"]);
                assert!(has(&args, "-x"));
                assert_eq!(values_after(&args, "--audio-format"), [*ext]);
                assert_eq!(values_after(&args, "--audio-quality"), [*quality]);
                // Video-only options must not leak into audio downloads.
                assert!(!has(&args, "-S"));
                assert!(!has(&args, "--merge-output-format"));
            }
        }
    }

    #[test]
    fn audio_cover_art_is_converted_to_jpg_except_for_wav() {
        let mut s = settings(Mode::Audio);
        s.audio_format = audio_format_index("mp3");
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(has(&args, "--embed-thumbnail"));
        assert_eq!(values_after(&args, "--convert-thumbnails"), ["jpg"]);

        s.audio_format = audio_format_index("wav");
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(!has(&args, "--embed-thumbnail"));
        assert!(!has(&args, "--convert-thumbnails"));
    }

    #[test]
    fn cover_art_can_be_turned_off() {
        for mode in [Mode::Video, Mode::Audio] {
            let mut s = settings(mode);
            s.embed_thumbnail = false;
            let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
            assert!(!has(&args, "--embed-thumbnail"));
            assert!(!has(&args, "--convert-thumbnails"));
        }
    }

    #[test]
    fn playlist_option() {
        let mut s = settings(Mode::Video);
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(has(&args, "--no-playlist") && !has(&args, "--yes-playlist"));

        s.playlist = true;
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(has(&args, "--yes-playlist") && !has(&args, "--no-playlist"));
    }

    #[test]
    fn clip_with_start_and_end() {
        let args = download_args(
            &settings(Mode::Audio),
            &Part::Clip {
                start: 90,
                end: Some(165),
            },
            &urls(&["https://a"]),
        );
        assert_eq!(
            values_after(&args, "-o"),
            ["%(title)s (clip 1m30s-2m45s).%(ext)s"]
        );
        assert_eq!(values_after(&args, "--download-sections"), ["*90-165"]);
        assert!(has(&args, "--force-keyframes-at-cuts"));
    }

    #[test]
    fn clip_without_end_runs_to_the_end() {
        let args = download_args(
            &settings(Mode::Video),
            &Part::Clip {
                start: 30,
                end: None,
            },
            &urls(&["https://a"]),
        );
        assert_eq!(
            values_after(&args, "-o"),
            ["%(title)s (clip 30s-end).%(ext)s"]
        );
        assert_eq!(values_after(&args, "--download-sections"), ["*30-inf"]);
    }

    #[test]
    fn split_chapters_adds_a_chapter_output_template() {
        let mut s = settings(Mode::Audio);
        s.split_chapters = true;
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(has(&args, "--split-chapters"));
        assert_eq!(
            values_after(&args, "-o"),
            [
                "%(title)s.%(ext)s",
                "chapter:%(title)s/%(section_number)02d - %(section_title)s.%(ext)s",
            ]
        );
    }

    #[test]
    fn clip_takes_precedence_over_split_chapters() {
        let mut s = settings(Mode::Video);
        s.split_chapters = true;
        let args = download_args(
            &s,
            &Part::Clip {
                start: 10,
                end: Some(20),
            },
            &urls(&["https://a"]),
        );
        assert!(has(&args, "--download-sections"));
        assert!(!has(&args, "--split-chapters"));
    }

    #[test]
    fn urls_always_come_after_the_option_separator() {
        // A "link" that looks like an option must never be read as one.
        let list = urls(&["https://a", "-x", "--exec=calc"]);
        for mode in [Mode::Video, Mode::Audio] {
            let args = download_args(
                &settings(mode),
                &Part::Clip {
                    start: 1,
                    end: None,
                },
                &list,
            );
            let sep = args.iter().position(|a| a == "--").expect("-- separator");
            assert_eq!(&args[sep + 1..], list.as_slice());
            assert_eq!(args.iter().filter(|a| *a == "--").count(), 1);
        }
    }

    #[test]
    fn progress_template_matches_what_the_app_parses() {
        // process.rs reads lines starting with "[PROG]" and splits on '|'
        // into percent, speed and ETA.
        let args = download_args(&settings(Mode::Video), &Part::Whole, &urls(&["https://a"]));
        let template = values_after(&args, "--progress-template")[0];
        let line = template
            .strip_prefix("download:")
            .expect("download: prefix");
        assert!(line.starts_with("[PROG]"));
        assert_eq!(line.matches('|').count(), 2);
    }

    #[test]
    fn chapters_are_downloaded_by_anchored_title() {
        let mut s = settings(Mode::Audio);
        s.split_chapters = true; // ignored when picking chapters
        let part = Part::Chapters(vec!["Intro".into(), "Solo (part 2) [A.B]*?".into()]);
        let args = download_args(&s, &part, &urls(&["https://a"]));
        assert_eq!(
            values_after(&args, "--download-sections"),
            ["^Intro$", r"^Solo \(part 2\) \[A\.B\]\*\?$"]
        );
        assert!(has(&args, "--force-keyframes-at-cuts"));
        assert!(!has(&args, "--split-chapters"));
        assert_eq!(
            values_after(&args, "-o"),
            ["%(title)s - %(section_number+1)02d - %(section_title)s.%(ext)s"]
        );
    }
}
