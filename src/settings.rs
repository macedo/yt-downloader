//! User settings, persisted as JSON in %APPDATA%.

use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};

pub const VIDEO_QUALITIES: &[(&str, Option<u32>)] = &[
    ("Best available", None),
    ("2160p (4K)", Some(2160)),
    ("1440p", Some(1440)),
    ("1080p", Some(1080)),
    ("720p", Some(720)),
    ("480p", Some(480)),
    ("360p", Some(360)),
];

pub const AUDIO_FORMATS: &[(&str, &str)] = &[
    ("MP3", "mp3"),
    ("M4A (AAC)", "m4a"),
    ("Opus", "opus"),
    ("FLAC (lossless)", "flac"),
    ("WAV", "wav"),
    ("Original (no conversion)", "best"),
];

pub const AUDIO_QUALITIES: &[(&str, &str)] = &[
    ("Highest (VBR 0)", "0"),
    ("320 kbps", "320K"),
    ("256 kbps", "256K"),
    ("192 kbps", "192K"),
    ("128 kbps", "128K"),
];

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Video,
    Audio,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        });
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub out_dir: PathBuf,
    pub mode: Mode,
    pub video_quality: usize,
    pub audio_format: usize,
    pub audio_quality: usize,
    pub playlist: bool,
    pub embed_thumbnail: bool,
    pub split_chapters: bool,
    pub theme: ThemeChoice,
}

impl Default for Settings {
    fn default() -> Self {
        let out_dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            out_dir,
            mode: Mode::Video,
            video_quality: 0,
            audio_format: 0,
            audio_quality: 0,
            playlist: false,
            embed_thumbnail: true,
            split_chapters: false,
            theme: ThemeChoice::System,
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("yt-downloader").join("config.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .map(|mut s| {
                s.video_quality = s.video_quality.min(VIDEO_QUALITIES.len() - 1);
                s.audio_format = s.audio_format.min(AUDIO_FORMATS.len() - 1);
                s.audio_quality = s.audio_quality.min(AUDIO_QUALITIES.len() - 1);
                s
            })
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, json);
            }
        }
    }
}
