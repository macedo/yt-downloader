//! yt-dlp download arguments.

use crate::clip::format_time;
use crate::settings::{AUDIO_FORMATS, AUDIO_QUALITIES, Mode, Settings, VIDEO_QUALITIES};

/// SponsorBlock categories removed by the "Skip sponsors" option: paid
/// sponsors, self-promotion, "like and subscribe" reminders, and non-music
/// parts of music videos (handy for backing tracks). Intros and outros are
/// kept: in a lesson they may matter.
pub const SPONSORBLOCK_CATEGORIES: &str = "sponsor,selfpromo,interaction,music_offtopic";

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
/// given settings, with files named after the video title.
#[cfg(test)]
pub fn download_args(s: &Settings, part: &Part, urls: &[String]) -> Vec<String> {
    download_args_named(s, part, None, urls)
}

/// Like [`download_args`], but files are named `name` instead of after the
/// video title when a name is given (see [`clean_file_name`]).
pub fn download_args_named(
    s: &Settings,
    part: &Part,
    name: Option<&str>,
    urls: &[String],
) -> Vec<String> {
    let custom = name.and_then(clean_file_name);
    let base = match &custom {
        // A literal name for every item of a playlist would make them all the
        // same file: number them ("Backing track - 2"). Empty for a lone video.
        Some(n) if s.playlist => format!("{}%(playlist_index& - {{}}|)s", template_literal(n)),
        Some(n) => template_literal(n),
        None => "%(title)s".to_owned(),
    };
    let output = match part {
        Part::Whole => format!("{base}.%(ext)s"),
        // A chosen name is used as is, without the clip range.
        Part::Clip { .. } if custom.is_some() => format!("{base}.%(ext)s"),
        Part::Clip { start, end } => format!(
            "{base} (clip {}-{}).%(ext)s",
            format_time(*start),
            end.map_or("end".to_owned(), format_time)
        ),
        // section_number is 0-based: "Lesson - 02 - Solo.mp3".
        Part::Chapters(_) => {
            format!("{base} - %(section_number+1)02d - %(section_title)s.%(ext)s")
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
                format!("chapter:{base}/%(section_number)02d - %(section_title)s.%(ext)s"),
            ]);
        }
        Part::Whole => {}
    }

    if s.write_description {
        // A .txt next to the download. (--write-description would save a
        // ".description" file, which Windows doesn't know how to open.)
        a.extend([
            "--print-to-file".into(),
            "%(description|)s".into(),
            format!("{base} - description.txt"),
        ]);
    }

    // SponsorBlock's segment times are for the whole video: applied to a clip
    // or a chapter they miss, or cut the wrong part. Whole downloads only.
    if s.sponsorblock && *part == Part::Whole {
        a.extend([
            "--sponsorblock-remove".into(),
            SPONSORBLOCK_CATEGORIES.into(),
        ]);
    }

    a.push("--".into());
    a.extend(urls.iter().cloned());
    a
}

/// Characters Windows doesn't allow in file names.
pub const INVALID_FILE_NAME_CHARS: &str = r#"\/:*?"<>|"#;

/// Extensions the app's downloads can end in; typing one in the name is
/// harmless, yt-dlp adds the real one.
const MEDIA_EXTENSIONS: &[&str] = &["mp4", "mp3", "m4a", "opus", "flac", "wav", "ogg", "webm"];

/// A name typed by the user, made safe as a Windows file name: invalid and
/// control characters removed, a typed media extension dropped, no leading or
/// trailing spaces nor trailing dots, and reserved device names (CON, NUL…)
/// avoided. None when nothing usable is left (the video title is used).
pub fn clean_file_name(name: &str) -> Option<String> {
    let mut n: String = name
        .chars()
        .filter(|c| !c.is_control() && !INVALID_FILE_NAME_CHARS.contains(*c))
        .collect();
    if let Some((stem, ext)) = n.rsplit_once('.')
        && MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    {
        n = stem.to_owned();
    }
    let n = n.trim().trim_end_matches(['.', ' ']).to_owned();
    if n.is_empty() {
        return None;
    }
    let upper = n.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit());
    Some(if reserved { format!("{n}_") } else { n })
}

/// `text` as literal text in a yt-dlp output template.
fn template_literal(text: &str) -> String {
    text.replace('%', "%%")
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

    use super::{Part, clean_file_name, download_args, download_args_named};
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
    #[test]
    fn description_is_saved_as_a_txt_file() {
        let mut s = settings(Mode::Audio);
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(!has(&args, "--print-to-file"));

        s.write_description = true;
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        let at = args.iter().position(|a| a == "--print-to-file").unwrap();
        assert_eq!(
            &args[at + 1..at + 3],
            ["%(description|)s", "%(title)s - description.txt"]
        );
    }

    #[test]
    fn sponsorblock_only_applies_to_whole_downloads() {
        let mut s = settings(Mode::Video);
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert!(!has(&args, "--sponsorblock-remove"));

        s.sponsorblock = true;
        let args = download_args(&s, &Part::Whole, &urls(&["https://a"]));
        assert_eq!(
            values_after(&args, "--sponsorblock-remove"),
            ["sponsor,selfpromo,interaction,music_offtopic"]
        );

        // Segment times are for the whole video, so they'd cut the wrong part
        // of a clip or chapter.
        for part in [
            Part::Clip {
                start: 10,
                end: Some(20),
            },
            Part::Chapters(vec!["Solo".into()]),
        ] {
            let args = download_args(&s, &part, &urls(&["https://a"]));
            assert!(!has(&args, "--sponsorblock-remove"), "{part:?}");
        }
    }

    #[test]
    fn file_name_is_cleaned_for_windows() {
        assert_eq!(clean_file_name("Blues in A"), Some("Blues in A".into()));
        assert_eq!(
            clean_file_name(r#"  AC/DC: Back in Black? <solo> "live" | 2  "#),
            Some("ACDC Back in Black solo live  2".into())
        );
        assert_eq!(
            clean_file_name("Backing track.MP3"),
            Some("Backing track".into())
        );
        assert_eq!(clean_file_name("Lesson 1.5"), Some("Lesson 1.5".into()));
        assert_eq!(clean_file_name("Take five..."), Some("Take five".into()));
        assert_eq!(clean_file_name("con"), Some("con_".into()));
        assert_eq!(clean_file_name("COM1"), Some("COM1_".into()));
        assert_eq!(clean_file_name("Combo"), Some("Combo".into()));
        for empty in ["", "   ", "???", ".mp4", "..."] {
            assert_eq!(clean_file_name(empty), None, "{empty:?}");
        }
    }

    #[test]
    fn chosen_name_replaces_the_title_everywhere() {
        let mut s = settings(Mode::Audio);
        s.split_chapters = true;
        s.write_description = true;
        let name = Some("Blues 100% shuffle");
        let args = download_args_named(&s, &Part::Whole, name, &urls(&["https://a"]));
        assert_eq!(
            values_after(&args, "-o"),
            [
                "Blues 100%% shuffle.%(ext)s",
                "chapter:Blues 100%% shuffle/%(section_number)02d - %(section_title)s.%(ext)s",
            ]
        );
        let at = args.iter().position(|a| a == "--print-to-file").unwrap();
        assert_eq!(args[at + 2], "Blues 100%% shuffle - description.txt");

        let clip = Part::Clip {
            start: 10,
            end: Some(20),
        };
        let args = download_args_named(&s, &clip, name, &urls(&["https://a"]));
        assert_eq!(values_after(&args, "-o"), ["Blues 100%% shuffle.%(ext)s"]);

        let chapters = Part::Chapters(vec!["Solo".into()]);
        let args = download_args_named(&s, &chapters, name, &urls(&["https://a"]));
        assert_eq!(
            values_after(&args, "-o"),
            ["Blues 100%% shuffle - %(section_number+1)02d - %(section_title)s.%(ext)s"]
        );
    }

    #[test]
    fn chosen_name_is_numbered_for_playlists() {
        let mut s = settings(Mode::Video);
        s.playlist = true;
        let args = download_args_named(&s, &Part::Whole, Some("Track"), &urls(&["https://a"]));
        assert_eq!(
            values_after(&args, "-o"),
            ["Track%(playlist_index& - {}|)s.%(ext)s"]
        );
    }

    #[test]
    fn empty_or_unusable_name_keeps_the_title() {
        let s = settings(Mode::Video);
        for name in [None, Some(""), Some("  "), Some("???")] {
            assert_eq!(
                download_args_named(&s, &Part::Whole, name, &urls(&["https://a"])),
                download_args(&s, &Part::Whole, &urls(&["https://a"])),
                "{name:?}"
            );
        }
    }
}
