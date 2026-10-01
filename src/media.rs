//! Link preview: media info from `yt-dlp -J` and file size estimation.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui;

use crate::net::http_agent;
use crate::process::background_command;
use crate::settings::{AUDIO_FORMATS, AUDIO_QUALITIES, Mode, Settings, VIDEO_QUALITIES};

/// Data shown in a link's preview, before downloading.
pub struct MediaInfo {
    pub title: String,
    pub channel: Option<String>,
    /// In seconds. For a playlist, the sum of its videos.
    pub duration: Option<f64>,
    /// YYYY-MM-DD
    pub upload_date: Option<String>,
    pub views: Option<u64>,
    pub chapters: usize,
    /// `Some(n)` when the link is a playlist with n videos.
    pub playlist_count: Option<usize>,
    pub formats: Vec<FormatInfo>,
    pub thumbnail_url: Option<String>,
    /// (uri for egui, image bytes), downloaded along with the info.
    pub thumbnail: Option<(String, egui::load::Bytes)>,
}

pub struct FormatInfo {
    ext: String,
    vcodec: String,
    acodec: String,
    height: Option<u32>,
    fps: f64,
    has_video: bool,
    has_audio: bool,
    /// `preference` field: yt-dlp ranks it ahead of every other criterion.
    /// On YouTube it is negative for dubbed audio tracks; missing = 0.
    preference: i64,
    /// `source_preference` field (99 on YouTube "Premium" formats). The
    /// YouTube extractor compares it before the codec.
    source_preference: i64,
    /// Higher for the video's original language.
    language_preference: i64,
    quality: f64,
    tbr: f64,
    abr: f64,
    /// m3u8 formats usually don't report their size.
    size: Option<u64>,
}

impl FormatInfo {
    /// yt-dlp's default video codec preference (AV1 > VP9 > H.264).
    fn vcodec_rank(&self) -> u8 {
        match self.vcodec.split('.').next().unwrap_or("") {
            "av01" => 3,
            "vp9" | "vp09" => 2,
            "avc1" | "h264" => 1,
            _ => 0,
        }
    }

    /// yt-dlp's default audio codec preference (Opus > AAC).
    fn acodec_rank(&self) -> u8 {
        match self.acodec.split('.').next().unwrap_or("") {
            "opus" => 2,
            "mp4a" | "aac" => 1,
            _ => 0,
        }
    }
}

pub fn parse_media_info(v: &serde_json::Value) -> MediaInfo {
    let str_field = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let entries = v.get("entries").and_then(|e| e.as_array());
    let is_playlist = v.get("_type").and_then(|t| t.as_str()) == Some("playlist");

    let duration = if is_playlist {
        let total: f64 = entries
            .iter()
            .flat_map(|e| e.iter())
            .filter_map(|e| e.get("duration")?.as_f64())
            .sum();
        (total > 0.0).then_some(total)
    } else {
        v.get("duration").and_then(|d| d.as_f64())
    };

    let upload_date = str_field("upload_date")
        .filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
        .map(|d| format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]));

    let mut formats: Vec<FormatInfo> = Vec::new();
    for f in v
        .get("formats")
        .and_then(|f| f.as_array())
        .into_iter()
        .flatten()
    {
        let get_str = |k: &str| f.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
        let get_f64 = |k: &str| f.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let vcodec = get_str("vcodec");
        let acodec = get_str("acodec");
        formats.push(FormatInfo {
            ext: get_str("ext"),
            height: f.get("height").and_then(|h| h.as_u64()).map(|h| h as u32),
            fps: get_f64("fps"),
            has_video: !vcodec.is_empty() && vcodec != "none",
            has_audio: !acodec.is_empty() && acodec != "none",
            vcodec,
            acodec,
            preference: f.get("preference").and_then(|x| x.as_i64()).unwrap_or(0),
            source_preference: f
                .get("source_preference")
                .and_then(|x| x.as_i64())
                .unwrap_or(0),
            language_preference: f
                .get("language_preference")
                .and_then(|x| x.as_i64())
                .unwrap_or(0),
            quality: get_f64("quality"),
            tbr: get_f64("tbr"),
            abr: get_f64("abr"),
            size: f
                .get("filesize")
                .and_then(|x| x.as_u64())
                .or_else(|| f.get("filesize_approx").and_then(|x| x.as_u64())),
        });
    }

    MediaInfo {
        title: str_field("title").unwrap_or_else(|| "(untitled)".to_owned()),
        channel: str_field("channel").or_else(|| str_field("uploader")),
        duration,
        upload_date,
        views: v.get("view_count").and_then(|x| x.as_u64()),
        chapters: v
            .get("chapters")
            .and_then(|c| c.as_array())
            .map_or(0, Vec::len),
        playlist_count: is_playlist.then(|| {
            v.get("playlist_count")
                .and_then(|c| c.as_u64())
                .map_or_else(|| entries.map_or(0, Vec::len), |c| c as usize)
        }),
        formats,
        thumbnail_url: pick_thumbnail(v),
        thumbnail: None,
    }
}

/// Small thumbnail (~320px) in a format we can display.
fn pick_thumbnail(v: &serde_json::Value) -> Option<String> {
    let supported = |url: &str| {
        let path = url.split('?').next().unwrap_or(url).to_lowercase();
        [".jpg", ".jpeg", ".png", ".webp"]
            .iter()
            .any(|e| path.ends_with(e))
    };
    let from_list = v
        .get("thumbnails")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let url = t.get("url")?.as_str()?;
            let width = t.get("width")?.as_u64()?;
            supported(url).then_some((url, width))
        })
        .min_by_key(|(_, w)| w.abs_diff(320))
        .map(|(url, _)| url.to_owned());
    from_list.or_else(|| {
        v.get("thumbnail")
            .and_then(|t| t.as_str())
            .filter(|u| supported(u))
            .map(str::to_owned)
    })
}

impl MediaInfo {
    /// Approximate size of the final file with the current options. Mimics the
    /// order in which yt-dlp picks YouTube formats with the app's arguments:
    /// `preference`, then the `-S` criteria (resolution, mp4/m4a), then the
    /// extractor's order (quality, fps, `source_preference`, codec…). For audio
    /// conversion it uses the target bitrate. `None` if the chosen format
    /// doesn't report its size (common for "Premium" formats served via m3u8).
    pub fn estimated_size(&self, s: &Settings) -> Option<u64> {
        use std::cmp::Ordering;
        let by = |a: f64, b: f64| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
        let best_audio = |prefer_m4a: bool| {
            self.formats
                .iter()
                .filter(|f| f.has_audio && !f.has_video)
                .max_by(|a, b| {
                    let m4a = |f: &FormatInfo| prefer_m4a && f.ext == "m4a";
                    a.preference
                        .cmp(&b.preference)
                        .then(m4a(a).cmp(&m4a(b)))
                        .then(by(a.quality, b.quality))
                        .then(a.source_preference.cmp(&b.source_preference))
                        .then(a.acodec_rank().cmp(&b.acodec_rank()))
                        .then(a.language_preference.cmp(&b.language_preference))
                        .then(by(a.abr, b.abr))
                })
        };

        match s.mode {
            Mode::Video => {
                let limit = VIDEO_QUALITIES[s.video_quality].1.unwrap_or(u32::MAX);
                let video = self
                    .formats
                    .iter()
                    .filter(|f| f.has_video && !f.has_audio)
                    .filter(|f| f.height.is_some_and(|h| h <= limit))
                    .max_by(|a, b| {
                        a.preference
                            .cmp(&b.preference)
                            .then(a.height.cmp(&b.height))
                            .then((a.ext == "mp4").cmp(&(b.ext == "mp4")))
                            .then(by(a.quality, b.quality))
                            .then(by(a.fps, b.fps))
                            .then(a.source_preference.cmp(&b.source_preference))
                            .then(a.vcodec_rank().cmp(&b.vcodec_rank()))
                            .then(by(a.tbr, b.tbr))
                    })?;
                Some(video.size? + best_audio(true)?.size?)
            }
            Mode::Audio => {
                let source = best_audio(false)?;
                let fmt = AUDIO_FORMATS[s.audio_format].1;
                let kbps = match fmt {
                    "best" => return source.size,
                    "wav" => 1411.0,
                    // FLAC varies a lot with the content; ~900 kbps is typical for music.
                    "flac" => 900.0,
                    _ => match AUDIO_QUALITIES[s.audio_quality].1.strip_suffix('K') {
                        Some(k) => k.parse().ok()?,
                        // VBR 0: MP3 lands near 245 kbps; Opus/AAC follow the source.
                        None if fmt == "mp3" => 245.0,
                        None => source.abr.max(96.0),
                    },
                };
                Some((kbps * 1000.0 / 8.0 * self.duration?) as u64)
            }
        }
    }
}

/// Fetches the link info with `yt-dlp -J` (and the thumbnail) in the background.
pub fn fetch_media_info(
    exe: PathBuf,
    url: String,
    playlist: bool,
    ctx: egui::Context,
) -> Receiver<Result<MediaInfo, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| {
            let out = background_command(&exe)
                .args([
                    "-J",
                    "--no-warnings",
                    "--no-colors",
                    "--flat-playlist",
                    if playlist {
                        "--yes-playlist"
                    } else {
                        "--no-playlist"
                    },
                    "--",
                    &url,
                ])
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let msg = stderr
                    .lines()
                    .find_map(|l| l.strip_prefix("ERROR: "))
                    .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
                    .unwrap_or("yt-dlp returned no information");
                return Err(msg.trim().to_owned());
            }
            let json: serde_json::Value = serde_json::from_slice(&out.stdout)
                .map_err(|e| format!("invalid response from yt-dlp: {e}"))?;
            let mut info = parse_media_info(&json);
            if let Some(thumb_url) = &info.thumbnail_url {
                info.thumbnail = download_thumbnail(thumb_url);
            }
            Ok(info)
        })();
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

fn download_thumbnail(url: &str) -> Option<(String, egui::load::Bytes)> {
    let mut resp = http_agent().get(url).call().ok()?;
    let bytes = resp.body_mut().read_to_vec().ok()?;
    // The extension in the uri helps the image loader detect the format.
    let ext = url.split('?').next()?.rsplit('.').next()?.to_lowercase();
    Some((
        format!("bytes://thumb/{:x}.{ext}", hash_str(url)),
        bytes.into(),
    ))
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// 3:25 or 1:02:03
pub fn format_duration(secs: f64) -> String {
    let secs = secs.round() as u64;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 2,814,943
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 2.8 MB / 1.2 GB
pub fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: Mode) -> Settings {
        Settings {
            mode,
            ..Settings::default()
        }
    }

    fn fmt(
        ext: &str,
        vcodec: &str,
        acodec: &str,
        height: Option<u64>,
        abr: f64,
        size: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "protocol": "https", "ext": ext, "vcodec": vcodec, "acodec": acodec,
            "height": height, "abr": abr, "tbr": abr, "filesize": size
        })
    }

    #[test]
    fn parses_video_and_estimates_size() {
        let json = serde_json::json!({
            "_type": "video", "title": "Test", "channel": "Channel", "duration": 100.0,
            "upload_date": "20200616", "view_count": 2814943,
            "chapters": [{"title": "a"}, {"title": "b"}],
            "thumbnails": [
                {"url": "https://x/big.jpg", "width": 1280},
                {"url": "https://x/mq.jpg", "width": 320},
                {"url": "https://x/small.jpg", "width": 120}
            ],
            "formats": [
                fmt("m4a", "none", "mp4a.40.2", None, 128.0, 1_000),
                fmt("webm", "none", "opus", None, 140.0, 1_200),
                fmt("mp4", "avc1.64", "none", Some(1080), 0.0, 20_000),
                fmt("mp4", "av01.0", "none", Some(1080), 0.0, 15_000),
                fmt("mp4", "av01.0", "none", Some(720), 0.0, 8_000),
                fmt("webm", "vp9", "none", Some(2160), 0.0, 90_000),
                {"protocol": "m3u8_native", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 1080, "tbr": 900.0}
            ]
        });
        let info = parse_media_info(&json);
        assert_eq!(info.title, "Test");
        assert_eq!(info.channel.as_deref(), Some("Channel"));
        assert_eq!(info.upload_date.as_deref(), Some("2020-06-16"));
        assert_eq!(info.chapters, 2);
        assert_eq!(info.playlist_count, None);
        assert_eq!(info.formats.len(), 7);
        assert_eq!(info.thumbnail_url.as_deref(), Some("https://x/mq.jpg"));

        // Best: 4K VP9 + m4a audio.
        assert_eq!(info.estimated_size(&settings(Mode::Video)), Some(91_000));
        // Up to 1080p: prefers mp4 and AV1 (15,000) + m4a.
        let mut s = settings(Mode::Video);
        s.video_quality = 3;
        assert_eq!(info.estimated_size(&s), Some(16_000));
        // Original audio: best audio (opus 140 kbps).
        let mut s = settings(Mode::Audio);
        s.audio_format = 5;
        assert_eq!(info.estimated_size(&s), Some(1_200));
        // MP3 320 kbps for 100 s = 4,000,000 bytes.
        s.audio_format = 0;
        s.audio_quality = 1;
        assert_eq!(info.estimated_size(&s), Some(4_000_000));
    }

    #[test]
    fn premium_format_without_size_is_not_estimated() {
        // As on YouTube: "Premium" (source_preference 99) beats AV1 at the same
        // height; with no reported size, don't make up a number.
        let json = serde_json::json!({
            "duration": 100.0,
            "formats": [
                fmt("m4a", "none", "mp4a.40.2", None, 128.0, 1_000),
                fmt("mp4", "av01.0", "none", Some(1080), 0.0, 15_000),
                fmt("mp4", "av01.0", "none", Some(720), 0.0, 8_000),
                {"protocol": "m3u8_native", "ext": "mp4", "vcodec": "vp09.00.40.08", "acodec": "none",
                 "height": 1080, "tbr": 2180.0, "source_preference": 99}
            ]
        });
        let info = parse_media_info(&json);
        assert_eq!(info.estimated_size(&settings(Mode::Video)), None);
        let mut s = settings(Mode::Video);
        s.video_quality = 4; // 720p: Premium is excluded
        assert_eq!(info.estimated_size(&s), Some(9_000));
    }

    #[test]
    fn prefers_original_audio_track() {
        // Dubbed tracks have a negative "preference"; the original has none.
        let mut dub = fmt("m4a", "none", "mp4a.40.2", None, 200.0, 5_000);
        dub["preference"] = serde_json::json!(-3);
        let json = serde_json::json!({
            "duration": 100.0,
            "formats": [dub, fmt("m4a", "none", "mp4a.40.2", None, 128.0, 1_000)]
        });
        let mut s = settings(Mode::Audio);
        s.audio_format = 5; // original
        assert_eq!(parse_media_info(&json).estimated_size(&s), Some(1_000));
    }

    #[test]
    fn parses_playlist() {
        let json = serde_json::json!({
            "_type": "playlist", "title": "List", "uploader": "Channel", "playlist_count": 3,
            "entries": [{"duration": 60.0}, {"duration": 90.0}, {"duration": null}],
            "thumbnail": "https://x/a.webp"
        });
        let info = parse_media_info(&json);
        assert_eq!(info.playlist_count, Some(3));
        assert_eq!(info.channel.as_deref(), Some("Channel"));
        assert_eq!(info.duration, Some(150.0));
        assert_eq!(info.thumbnail_url.as_deref(), Some("https://x/a.webp"));
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_count(2_814_943), "2,814,943");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_duration(79.4), "1:19");
        assert_eq!(format_duration(3723.0), "1:02:03");
        assert_eq!(format_size(2_936_013), "2.8 MB");
        assert_eq!(format_size(52_428_800), "50 MB");
        assert_eq!(format_size(1_288_490_189), "1.2 GB");
    }
}
