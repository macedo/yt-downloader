// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use eframe::egui;
use serde::{Deserialize, Serialize};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Keeps child processes (yt-dlp, winget, ffmpeg) from opening console windows.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_LOG_LINES: usize = 3000;

const VIDEO_QUALITIES: &[(&str, Option<u32>)] = &[
    ("Best available", None),
    ("2160p (4K)", Some(2160)),
    ("1440p", Some(1440)),
    ("1080p", Some(1080)),
    ("720p", Some(720)),
    ("480p", Some(480)),
    ("360p", Some(360)),
];

const AUDIO_FORMATS: &[(&str, &str)] = &[
    ("MP3", "mp3"),
    ("M4A (AAC)", "m4a"),
    ("Opus", "opus"),
    ("FLAC (lossless)", "flac"),
    ("WAV", "wav"),
    ("Original (no conversion)", "best"),
];

const AUDIO_QUALITIES: &[(&str, &str)] = &[
    ("Highest (VBR 0)", "0"),
    ("320 kbps", "320K"),
    ("256 kbps", "256K"),
    ("192 kbps", "192K"),
    ("128 kbps", "128K"),
];

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Video,
    Audio,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        });
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
struct Settings {
    out_dir: PathBuf,
    mode: Mode,
    video_quality: usize,
    audio_format: usize,
    audio_quality: usize,
    playlist: bool,
    embed_thumbnail: bool,
    split_chapters: bool,
    theme: ThemeChoice,
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

    fn load() -> Self {
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

    fn save(&self) {
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

/// Messages sent from the child-process threads to the UI.
enum Msg {
    Line(String),
    Progress {
        pct: f32,
        speed: String,
        eta: String,
    },
    Done(Option<i32>),
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Download,
    Tooling,
}

struct Job {
    kind: JobKind,
    pid: u32,
    rx: Receiver<Msg>,
    cancelled: bool,
}

/// Link (and whether playlists are enabled) the preview was requested for.
type PreviewKey = (String, bool);

enum Preview {
    None,
    /// Waits for the user to stop typing before querying.
    Waiting {
        key: PreviewKey,
        since: f64,
    },
    Loading {
        key: PreviewKey,
        rx: Receiver<Result<MediaInfo, String>>,
    },
    Ready {
        key: PreviewKey,
        info: Box<MediaInfo>,
    },
    Failed {
        key: PreviewKey,
        error: String,
    },
}

impl Preview {
    fn key(&self) -> Option<&PreviewKey> {
        match self {
            Preview::None => None,
            Preview::Waiting { key, .. }
            | Preview::Loading { key, .. }
            | Preview::Ready { key, .. }
            | Preview::Failed { key, .. } => Some(key),
        }
    }
}

const PREVIEW_DELAY_SECS: f64 = 0.6;

struct App {
    settings: Settings,
    saved_settings: Settings,
    urls: String,
    ytdlp: Option<PathBuf>,
    job: Option<Job>,
    log: Vec<(String, bool)>,
    progress: f32,
    status: String,
    item_info: String,
    egui_ctx: egui::Context,
    version_rx: Option<Receiver<VersionInfo>>,
    ytdlp_version: Option<String>,
    /// Newest published yt-dlp version, when newer than the installed one.
    update_available: Option<String>,
    update_dismissed: bool,
    /// Clip range: applies only to the current download, so it is not saved.
    cut_enabled: bool,
    cut_start: String,
    cut_end: String,
    preview: Preview,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_zoom_factor(1.1);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings = Settings::load();
        settings.theme.apply(&cc.egui_ctx);
        let ytdlp = find_ytdlp();
        let status = match &ytdlp {
            Some(_) => "Ready.".to_owned(),
            None => "yt-dlp not found — click \"Install yt-dlp\".".to_owned(),
        };
        let mut app = Self {
            saved_settings: settings.clone(),
            settings,
            urls: String::new(),
            ytdlp,
            job: None,
            log: Vec::new(),
            progress: 0.0,
            status,
            item_info: String::new(),
            egui_ctx: cc.egui_ctx.clone(),
            version_rx: None,
            ytdlp_version: None,
            update_available: None,
            update_dismissed: false,
            cut_enabled: false,
            cut_start: String::new(),
            cut_end: String::new(),
            preview: Preview::None,
        };
        app.start_version_check();
        app
    }

    /// Looks up the installed and latest published versions in the background.
    fn start_version_check(&mut self) {
        if let Some(exe) = &self.ytdlp {
            self.version_rx = Some(check_ytdlp_version(exe.clone(), self.egui_ctx.clone()));
        }
    }

    /// Requests the preview when there is exactly one link and collects the result.
    fn update_preview(&mut self, now: f64) {
        let wanted = match self.urls().as_slice() {
            [url] if self.ytdlp.is_some() => Some((url.clone(), self.settings.playlist)),
            _ => None,
        };
        if wanted.as_ref() != self.preview.key() {
            // Link changed: drop the previous preview (an in-flight lookup is ignored).
            self.preview = match wanted {
                Some(key) => Preview::Waiting { key, since: now },
                None => Preview::None,
            };
        }

        self.preview = match std::mem::replace(&mut self.preview, Preview::None) {
            Preview::Waiting { key, since } if now - since >= PREVIEW_DELAY_SECS => {
                let exe = self.ytdlp.clone().expect("yt-dlp found");
                let rx = fetch_media_info(exe, key.0.clone(), key.1, self.egui_ctx.clone());
                Preview::Loading { key, rx }
            }
            Preview::Loading { key, rx } => match rx.try_recv() {
                Ok(Ok(info)) => Preview::Ready {
                    key,
                    info: Box::new(info),
                },
                Ok(Err(error)) => Preview::Failed { key, error },
                Err(mpsc::TryRecvError::Disconnected) => Preview::Failed {
                    key,
                    error: "the lookup was interrupted".to_owned(),
                },
                Err(mpsc::TryRecvError::Empty) => Preview::Loading { key, rx },
            },
            other => other,
        };
    }

    fn preview_card(&self, ui: &mut egui::Ui) {
        if matches!(self.preview, Preview::None) {
            return;
        }
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(6.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match &self.preview {
                    Preview::None => {}
                    Preview::Waiting { .. } | Preview::Loading { .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak("Fetching link info…");
                        });
                    }
                    Preview::Failed { error, .. } => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Couldn't fetch the link info.",
                        );
                        ui.add(egui::Label::new(egui::RichText::new(error).weak().small()).wrap());
                    }
                    Preview::Ready { info, .. } => self.media_info_view(ui, info),
                }
            });
    }

    fn media_info_view(&self, ui: &mut egui::Ui, info: &MediaInfo) {
        const THUMB: egui::Vec2 = egui::vec2(160.0, 90.0);
        ui.horizontal_top(|ui| {
            match &info.thumbnail {
                Some((uri, bytes)) => {
                    ui.add(
                        egui::Image::from_bytes(uri.clone(), bytes.clone())
                            .fit_to_exact_size(THUMB)
                            .corner_radius(4.0),
                    );
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(THUMB, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
                }
            }

            ui.vertical(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(&info.title).strong()).wrap());

                let mut meta: Vec<String> = Vec::new();
                if let Some(count) = info.playlist_count {
                    meta.push(if count == 1 {
                        "Playlist • 1 video".to_owned()
                    } else {
                        format!("Playlist • {count} videos")
                    });
                }
                meta.extend(info.channel.clone());
                if let Some(d) = info.duration {
                    meta.push(if info.playlist_count.is_some() {
                        format!("{} total", format_duration(d))
                    } else {
                        format_duration(d)
                    });
                }
                meta.extend(info.upload_date.clone());
                if let Some(v) = info.views {
                    meta.push(format!("{} views", format_count(v)));
                }
                ui.weak(meta.join("  •  "));

                let mut details: Vec<String> = Vec::new();
                if info.chapters > 0 {
                    let chapters = if info.chapters == 1 {
                        "1 chapter".to_owned()
                    } else {
                        format!("{} chapters", info.chapters)
                    };
                    details.push(if self.settings.split_chapters && !self.cut_enabled {
                        format!("{chapters} (will be split)")
                    } else {
                        chapters
                    });
                }
                if info.playlist_count.is_none() {
                    // When clipping, the size is proportional to the clip.
                    let fraction = match (self.cut_range(), info.duration) {
                        (Ok(Some((start, end))), Some(total)) if total > 0.0 => {
                            let end = end.map_or(total, |e| (e as f64).min(total));
                            ((end - start as f64) / total).clamp(0.0, 1.0)
                        }
                        _ => 1.0,
                    };
                    match info.estimated_size(&self.settings) {
                        Some(size) => {
                            let size = (size as f64 * fraction) as u64;
                            let suffix = if fraction < 1.0 { " (clip)" } else { "" };
                            details.push(format!("Approx. size: {}{suffix}", format_size(size)));
                        }
                        None if !info.formats.is_empty() => {
                            details.push("Size not reported for this quality".to_owned());
                        }
                        None => {}
                    }
                }
                if !details.is_empty() {
                    ui.label(details.join("  •  "));
                }
            });
        });
    }

    fn poll_version_check(&mut self) {
        let Some(rx) = &self.version_rx else { return };
        match rx.try_recv() {
            Ok(info) => {
                self.update_available = match (info.latest, &info.installed) {
                    (Some(latest), Some(installed)) if is_newer(&latest, installed) => Some(latest),
                    _ => None,
                };
                self.ytdlp_version = info.installed;
                self.version_rx = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.version_rx = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        let is_error = line.starts_with("ERROR");
        self.log.push((line, is_error));
        if self.log.len() > MAX_LOG_LINES {
            let excess = self.log.len() - MAX_LOG_LINES;
            self.log.drain(..excess);
        }
    }

    fn urls(&self) -> Vec<String> {
        self.urls
            .split_whitespace()
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            .map(str::to_owned)
            .collect()
    }

    /// Clip to download in seconds (start, optional end), if clipping is on.
    fn cut_range(&self) -> Result<Option<(u64, Option<u64>)>, &'static str> {
        if !self.cut_enabled {
            return Ok(None);
        }
        let start = parse_time(&self.cut_start).ok_or("Invalid start — use 1:30, 90 or 1:02:03")?;
        let end = parse_time(&self.cut_end).ok_or("Invalid end — use 1:30, 90 or 1:02:03")?;
        let start = start.unwrap_or(0);
        match end {
            Some(end) if end <= start => Err("The end must be after the start"),
            None if start == 0 => Err("Enter the clip start and/or end"),
            _ => Ok(Some((start, end))),
        }
    }

    fn build_args(&self, urls: &[String]) -> Vec<String> {
        let s = &self.settings;
        let cut = self.cut_range().ok().flatten();
        let output = match cut {
            Some((start, end)) => format!(
                "%(title)s (clip {}-{}).%(ext)s",
                format_time(start),
                end.map_or("end".to_owned(), format_time)
            ),
            None => "%(title)s.%(ext)s".to_owned(),
        };
        let mut a: Vec<String> = vec![
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

    fn start_download(&mut self) {
        let urls = self.urls();
        if urls.is_empty() {
            self.status = "Paste at least one valid link (http/https).".into();
            return;
        }
        let Some(exe) = self.ytdlp.clone() else {
            self.status = "yt-dlp not found. Install it first.".into();
            return;
        };
        if let Err(e) = self.cut_range() {
            self.status = e.into();
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.settings.out_dir) {
            self.status = format!("Couldn't create the destination folder: {e}");
            return;
        }
        let args = self.build_args(&urls);
        self.log.clear();
        self.push_log(format!("> yt-dlp {}", args.join(" ")));
        self.progress = 0.0;
        self.item_info.clear();
        self.run(JobKind::Download, &exe, &args, "Starting…");
    }

    fn install_or_update_ytdlp(&mut self) {
        // Update with yt-dlp itself (-U), which downloads from GitHub — the same
        // source as the version check. The winget catalog can lag behind new
        // releases. winget is only used for the first install, because it also
        // brings FFmpeg and Deno.
        let (exe, args, label): (PathBuf, Vec<String>, &str) = match &self.ytdlp {
            Some(p) => (p.clone(), vec!["-U".into()], "Updating yt-dlp…"),
            None => (
                PathBuf::from("winget"),
                [
                    "install",
                    "-e",
                    "--id",
                    "yt-dlp.yt-dlp",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ]
                .map(String::from)
                .to_vec(),
                "Installing yt-dlp via winget (this may take a while)…",
            ),
        };
        self.log.clear();
        self.push_log(format!("> {} {}", exe.display(), args.join(" ")));
        self.run(JobKind::Tooling, &exe, &args, label);
    }

    fn run(&mut self, kind: JobKind, exe: &Path, args: &[String], status: &str) {
        match spawn_process(exe, args) {
            Ok((pid, rx)) => {
                self.status = status.to_owned();
                self.job = Some(Job {
                    kind,
                    pid,
                    rx,
                    cancelled: false,
                });
            }
            Err(e) => {
                self.status = format!("Failed to start {}: {e}", exe.display());
                self.push_log(format!("ERROR: {e}"));
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(job) = &mut self.job {
            job.cancelled = true;
            // /T also kills child processes (ffmpeg).
            let mut cmd = Command::new("taskkill");
            cmd.args(["/T", "/F", "/PID", &job.pid.to_string()]);
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            let _ = cmd.output();
            self.status = "Cancelling…".into();
        }
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let msgs: Vec<Msg> = job.rx.try_iter().collect();
        for msg in msgs {
            match msg {
                Msg::Progress { pct, speed, eta } => {
                    self.progress = pct / 100.0;
                    self.status = format!("Downloading… {pct:.1}%   {speed}   ETA {eta}");
                }
                Msg::Line(line) => {
                    if let Some(rest) = line.strip_prefix("[download] Downloading item ") {
                        self.item_info = format!("Item {rest}");
                    } else if line.starts_with("[ExtractAudio]") {
                        self.status = "Converting audio…".into();
                    } else if line.starts_with("[Merger]") {
                        self.status = "Merging video and audio…".into();
                    } else if line.starts_with("[EmbedThumbnail]") {
                        self.status = "Embedding cover art…".into();
                    } else if line.starts_with("[SplitChapters]") {
                        self.status = "Splitting chapters…".into();
                    }
                    self.push_log(line);
                }
                Msg::Done(code) => {
                    let job = self.job.take().expect("active job");
                    let errors = self.log.iter().filter(|(_, e)| *e).count();
                    self.status = if job.cancelled {
                        "Cancelled.".into()
                    } else if code == Some(0) {
                        self.progress = 1.0;
                        match job.kind {
                            JobKind::Download => "Done! ✔".into(),
                            JobKind::Tooling => "yt-dlp installed/updated.".into(),
                        }
                    } else {
                        format!(
                            "Finished with an error (code {}, {errors} error(s) in the log).",
                            code.map_or("?".to_owned(), |c| c.to_string())
                        )
                    };
                    if job.kind == JobKind::Tooling {
                        self.ytdlp = find_ytdlp();
                        if let Some(p) = &self.ytdlp {
                            self.push_log(format!("yt-dlp at: {}", p.display()));
                        }
                        self.update_dismissed = false;
                        self.start_version_check();
                    }
                    return;
                }
            }
        }
    }

    fn open_folder(&self) {
        let _ = Command::new("explorer").arg(&self.settings.out_dir).spawn();
    }

    fn choose_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_directory(&self.settings.out_dir)
            .pick_folder()
        {
            self.settings.out_dir = dir;
        }
    }

    /// Keyboard shortcuts. Runs before the widgets so the links field
    /// doesn't receive the same keys.
    fn handle_input(&mut self, ctx: &egui::Context) {
        let (start, cancel) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });

        let busy = self.job.is_some();
        if start && !busy && self.ytdlp.is_some() {
            self.start_download();
        }
        if cancel && busy {
            self.cancel();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let has_urls = !self.urls().is_empty();

        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
            ui.spacing_mut().item_spacing.x = 6.0;

            if busy {
                let btn = egui::Button::new(egui::RichText::new("⏹  Cancel").strong())
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
                if ui.add(btn).on_hover_text("Stop (Esc)").clicked() {
                    self.cancel();
                }
            } else {
                let btn = egui::Button::new(egui::RichText::new("⬇  Download").strong())
                    .fill(ui.visuals().selection.bg_fill);
                let cut = self.cut_range();
                let why_disabled = if self.ytdlp.is_none() {
                    "Install yt-dlp first"
                } else if let Err(e) = cut {
                    e
                } else {
                    "Paste one or more links"
                };
                if ui
                    .add_enabled(self.ytdlp.is_some() && has_urls && cut.is_ok(), btn)
                    .on_hover_text("Download the links (Ctrl+Enter)")
                    .on_disabled_hover_text(why_disabled)
                    .clicked()
                {
                    self.start_download();
                }
            }

            ui.separator();

            ui.add_enabled_ui(!busy, |ui| {
                ui.selectable_value(&mut self.settings.mode, Mode::Video, "🎞  Video")
                    .on_hover_text("Download video (MP4)");
                ui.selectable_value(&mut self.settings.mode, Mode::Audio, "🎵  Audio")
                    .on_hover_text("Extract audio only");
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("🌓", |ui| {
                    for (choice, label) in [
                        (ThemeChoice::System, "Follow system"),
                        (ThemeChoice::Light, "Light"),
                        (ThemeChoice::Dark, "Dark"),
                    ] {
                        if ui
                            .radio_value(&mut self.settings.theme, choice, label)
                            .clicked()
                        {
                            choice.apply(ui.ctx());
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Theme");

                // Updates are offered by the notice banner; this only handles installing.
                if self.ytdlp.is_none()
                    && ui
                        .add_enabled(!busy, egui::Button::new("⬇  Install yt-dlp"))
                        .on_hover_text("Install yt-dlp, FFmpeg and Deno via winget")
                        .clicked()
                {
                    self.install_or_update_ytdlp();
                }
            });
        });
    }

    fn options_panel(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        ui.add_space(10.0);

        ui.add_enabled_ui(!busy, |ui| {
            match self.settings.mode {
                Mode::Video => {
                    section(ui, "🎞  Video");
                    ui.label("Quality");
                    combo(
                        ui,
                        "vq",
                        &mut self.settings.video_quality,
                        VIDEO_QUALITIES.iter().map(|q| q.0),
                    );
                    ui.add_space(2.0);
                    ui.weak("Saved as MP4 with the best audio.");
                }
                Mode::Audio => {
                    section(ui, "🎵  Audio");
                    ui.label("Format");
                    combo(
                        ui,
                        "af",
                        &mut self.settings.audio_format,
                        AUDIO_FORMATS.iter().map(|f| f.0),
                    );
                    ui.add_space(6.0);
                    let lossy = !matches!(
                        AUDIO_FORMATS[self.settings.audio_format].1,
                        "flac" | "wav" | "best"
                    );
                    ui.add_enabled_ui(lossy, |ui| {
                        ui.label("Quality");
                        combo(
                            ui,
                            "aq",
                            &mut self.settings.audio_quality,
                            AUDIO_QUALITIES.iter().map(|q| q.0),
                        );
                    });
                }
            }

            ui.add_space(16.0);
            section(ui, "⚙  Options");
            ui.checkbox(&mut self.settings.playlist, "Download whole playlist")
                .on_hover_text("If the link is part of a playlist, download every item");
            ui.checkbox(&mut self.settings.embed_thumbnail, "Embed cover art")
                .on_hover_text("Saves the video thumbnail as the file's cover art");
            ui.add_enabled(
                !self.cut_enabled,
                egui::Checkbox::new(&mut self.settings.split_chapters, "Split by chapters"),
            )
            .on_hover_text(
                "Besides the full file, creates one file per chapter (e.g. album tracks) \
                 in a folder named after the video. Videos without chapters are unaffected.",
            )
            .on_disabled_hover_text("Unavailable when downloading a clip");

            ui.add_space(16.0);
            section(ui, "✂  Clip");
            ui.checkbox(&mut self.cut_enabled, "Download only a clip")
                .on_hover_text("Applies to every link in the list");
            ui.add_enabled_ui(self.cut_enabled, |ui| {
                egui::Grid::new("cut")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Start");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.cut_start)
                                .desired_width(90.0)
                                .hint_text("0:00"),
                        );
                        ui.end_row();
                        ui.label("End");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.cut_end)
                                .desired_width(90.0)
                                .hint_text("to the end"),
                        );
                        ui.end_row();
                    });
                match self.cut_range() {
                    Err(e) if self.cut_enabled => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    _ => {
                        ui.weak("E.g. 1:30, 90 or 1:02:03");
                    }
                }
            });

            ui.add_space(16.0);
            section(ui, "🗀  Destination");
            ui.add(egui::Label::new(self.settings.out_dir.display().to_string()).wrap());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Change…").clicked() {
                    self.choose_folder();
                }
                if ui.button("Open").clicked() {
                    self.open_folder();
                }
            });
        });
    }

    fn main_area(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Links").strong());
            ui.weak("one per line — videos or playlists");
            let count = self.urls().len();
            if count > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(if count == 1 {
                        "1 link".to_owned()
                    } else {
                        format!("{count} links")
                    });
                });
            }
        });
        ui.add_enabled(
            !busy,
            egui::TextEdit::multiline(&mut self.urls)
                .id_salt("urls")
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .hint_text("Paste here: https://www.youtube.com/watch?v=…"),
        );
        self.preview_card(ui);

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Log").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.log.is_empty(), egui::Button::new("Clear log").small())
                    .clicked()
                {
                    self.log.clear();
                }
            });
        });

        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(6.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.log.is_empty() {
                            ui.weak("No activity yet.");
                        }
                        for (line, is_error) in &self.log {
                            let mut text = egui::RichText::new(line).monospace().size(11.5);
                            if *is_error {
                                text = text.color(ui.visuals().error_fg_color);
                            }
                            ui.add(egui::Label::new(text).extend());
                        }
                    });
            });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let running_download = self
            .job
            .as_ref()
            .is_some_and(|j| j.kind == JobKind::Download);

        ui.add_space(6.0);
        ui.add(
            egui::ProgressBar::new(self.progress)
                .desired_height(8.0)
                .animate(busy && (self.progress == 0.0 || !running_download)),
        );
        ui.horizontal(|ui| {
            // Right side first, so the status (on the left) truncates in the space left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let version = self.ytdlp_version.as_deref().unwrap_or("");
                let (label, color, tip) = match (&self.ytdlp, &self.update_available) {
                    (Some(p), Some(latest)) => (
                        format!("yt-dlp {version} — update available"),
                        ui.visuals().warn_fg_color,
                        format!("New version: {latest}\n{}", p.display()),
                    ),
                    (Some(p), None) => (
                        format!("yt-dlp {version}").trim_end().to_owned(),
                        egui::Color32::from_rgb(60, 170, 90),
                        p.display().to_string(),
                    ),
                    (None, _) => (
                        "yt-dlp missing".to_owned(),
                        ui.visuals().error_fg_color,
                        "Use \"Install yt-dlp\" in the toolbar".to_owned(),
                    ),
                };
                ui.label(egui::RichText::new(label).small())
                    .on_hover_text(&tip);
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, color);
                if !self.item_info.is_empty() {
                    ui.separator();
                    ui.weak(&self.item_info);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(&self.status).truncate());
                });
            });
        });
        ui.add_space(2.0);
    }

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let latest = self.update_available.clone().unwrap_or_default();
        let installed = self.ytdlp_version.clone().unwrap_or_default();
        let busy = self.job.is_some();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("New yt-dlp version available").strong());
            ui.label(format!("{latest}  (installed: {installed})"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✖").on_hover_text("Dismiss").clicked() {
                    self.update_dismissed = true;
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("🔄  Update now"))
                    .on_disabled_hover_text("Wait for the current download to finish")
                    .clicked()
                {
                    self.install_or_update_ytdlp();
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        self.poll_version_check();
        self.handle_input(ui.ctx());
        self.update_preview(ui.input(|i| i.time));
        if self.job.is_some()
            || matches!(
                self.preview,
                Preview::Waiting { .. } | Preview::Loading { .. }
            )
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.settings != self.saved_settings {
            self.settings.save();
            self.saved_settings = self.settings.clone();
        }

        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| self.toolbar(ui));
        let updating = self
            .job
            .as_ref()
            .is_some_and(|j| j.kind == JobKind::Tooling);
        if self.update_available.is_some() && !self.update_dismissed && !updating {
            egui::Panel::top("update_banner")
                .frame(
                    egui::Frame::new()
                        .fill(ui.visuals().warn_fg_color.gamma_multiply(0.18))
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ui, |ui| self.update_banner(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("options")
            .resizable(false)
            .exact_size(250.0)
            .show(ui, |ui| self.options_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.main_area(ui));
    }

    fn on_exit(&mut self) {
        self.settings.save();
        if self.job.is_some() {
            self.cancel();
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong().size(15.0));
    ui.add_space(4.0);
}

fn combo<'a>(
    ui: &mut egui::Ui,
    id: &str,
    selected: &mut usize,
    items: impl Iterator<Item = &'a str> + Clone,
) {
    let current = items.clone().nth(*selected).unwrap_or_default();
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (i, name) in items.enumerate() {
                ui.selectable_value(selected, i, name);
            }
        });
}

/// Up-to-date PATH: the process's own plus the registry's (user and machine).
/// winget adds yt-dlp, ffmpeg and deno to the registry PATH, which this
/// process would only see after restarting Explorer or the session.
fn search_paths() -> Vec<PathBuf> {
    let mut dirs_out: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs_out.push(dir);
    }
    if let Some(p) = std::env::var_os("PATH") {
        dirs_out.extend(std::env::split_paths(&p));
    }
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        let sources = [
            (HKEY_CURRENT_USER, "Environment"),
            (
                HKEY_LOCAL_MACHINE,
                r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            ),
        ];
        for (root, key) in sources {
            if let Ok(value) = RegKey::predef(root)
                .open_subkey(key)
                .and_then(|k| k.get_value::<String, _>("Path"))
            {
                dirs_out.extend(
                    value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(|s| PathBuf::from(expand_env(s))),
                );
            }
        }
    }
    if let Some(local) = dirs::data_local_dir() {
        dirs_out.push(local.join("Microsoft").join("WinGet").join("Links"));
    }
    let mut seen = std::collections::HashSet::new();
    dirs_out.retain(|d| seen.insert(d.to_string_lossy().to_lowercase()));
    dirs_out
}

/// Expands %VAR% variables (REG_EXPAND_SZ values).
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut parts = s.split('%');
    out.push_str(parts.next().unwrap_or(""));
    let mut in_var = true;
    for part in parts {
        if in_var {
            match std::env::var(part) {
                Ok(v) if !part.is_empty() => out.push_str(&v),
                _ => {
                    out.push('%');
                    out.push_str(part);
                    out.push('%');
                }
            }
        } else {
            out.push_str(part);
        }
        in_var = !in_var;
    }
    out
}

const YTDLP_LATEST_RELEASE_URL: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";

/// Result of the background yt-dlp version check.
struct VersionInfo {
    installed: Option<String>,
    latest: Option<String>,
}

fn check_ytdlp_version(exe: PathBuf, ctx: egui::Context) -> Receiver<VersionInfo> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let installed = installed_ytdlp_version(&exe);
        let latest = latest_ytdlp_version();
        let _ = tx.send(VersionInfo { installed, latest });
        ctx.request_repaint();
    });
    rx
}

/// Process with no console window, UTF-8 output and a PATH that includes what
/// winget installed (ffmpeg/deno), even if this process has a stale PATH.
fn background_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    if let Ok(joined) = std::env::join_paths(search_paths()) {
        cmd.env("PATH", joined);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent(concat!("yt-downloader/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
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
fn is_newer(latest: &str, installed: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(installed)
}

/// Parses "90", "1:30" or "1:02:03" as seconds. Empty = `Some(None)`; invalid = `None`.
fn parse_time(s: &str) -> Option<Option<u64>> {
    let s = s.trim();
    if s.is_empty() {
        return Some(None);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let nums: Vec<u64> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    // Minutes and seconds after the first field must be below 60.
    if nums.iter().skip(1).any(|&n| n >= 60) {
        return None;
    }
    Some(Some(nums.iter().fold(0, |acc, n| acc * 60 + n)))
}

/// Time for file names (no ":", which Windows rejects): 1m30s, 1h02m03s.
fn format_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m{s:02}s"),
        _ => format!("{h}h{m:02}m{s:02}s"),
    }
}

/// Data shown in a link's preview, before downloading.
struct MediaInfo {
    title: String,
    channel: Option<String>,
    /// In seconds. For a playlist, the sum of its videos.
    duration: Option<f64>,
    /// YYYY-MM-DD
    upload_date: Option<String>,
    views: Option<u64>,
    chapters: usize,
    /// `Some(n)` when the link is a playlist with n videos.
    playlist_count: Option<usize>,
    formats: Vec<FormatInfo>,
    thumbnail_url: Option<String>,
    /// (uri for egui, image bytes), downloaded along with the info.
    thumbnail: Option<(String, egui::load::Bytes)>,
}

struct FormatInfo {
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

fn parse_media_info(v: &serde_json::Value) -> MediaInfo {
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
    fn estimated_size(&self, s: &Settings) -> Option<u64> {
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
fn fetch_media_info(
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
fn format_duration(secs: f64) -> String {
    let secs = secs.round() as u64;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 2,814,943
fn format_count(n: u64) -> String {
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
fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

fn find_ytdlp() -> Option<PathBuf> {
    search_paths()
        .into_iter()
        .map(|d| d.join("yt-dlp.exe"))
        .find(|p| p.is_file())
}

fn spawn_process(exe: &Path, args: &[String]) -> std::io::Result<(u32, Receiver<Msg>)> {
    let mut child = background_command(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();

    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let tx_err = tx.clone();
    let err_reader = thread::spawn(move || read_lines(stderr, &tx_err));

    thread::spawn(move || {
        read_lines(stdout, &tx);
        let _ = err_reader.join();
        let code = child.wait().ok().and_then(|s| s.code());
        let _ = tx.send(Msg::Done(code));
    });

    Ok((pid, rx))
}

fn read_lines(stream: impl Read, tx: &Sender<Msg>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buf);
        // winget uses \r to animate progress; keep only the last segment.
        let line = text
            .trim_end()
            .rsplit('\r')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_owned();
        if line.trim().is_empty() {
            continue;
        }
        let msg = match line.strip_prefix("[PROG]") {
            Some(rest) => parse_progress(rest),
            None => Msg::Line(line),
        };
        if tx.send(msg).is_err() {
            break;
        }
    }
}

fn parse_progress(rest: &str) -> Msg {
    let mut parts = rest.split('|').map(str::trim);
    let pct = parts
        .next()
        .and_then(|p| p.trim_end_matches('%').trim().parse::<f32>().ok())
        .unwrap_or(0.0);
    let speed = parts.next().unwrap_or("").to_owned();
    let eta = parts.next().unwrap_or("").to_owned();
    Msg::Progress { pct, speed, eta }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("YT Downloader")
            .with_inner_size([940.0, 640.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "YT Downloader",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::{format_time, is_newer, parse_time};

    #[test]
    fn parses_clip_times() {
        assert_eq!(parse_time(""), Some(None));
        assert_eq!(parse_time("  "), Some(None));
        assert_eq!(parse_time("90"), Some(Some(90)));
        assert_eq!(parse_time("1:30"), Some(Some(90)));
        assert_eq!(parse_time("01:02:03"), Some(Some(3723)));
        assert_eq!(parse_time("75:00"), Some(Some(4500)));
        assert_eq!(parse_time("1:60"), None);
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time("1:"), None);
        assert_eq!(parse_time("-5"), None);
        assert_eq!(parse_time("1.5"), None);
        assert_eq!(parse_time("abc"), None);
    }

    #[test]
    fn formats_time_for_file_names() {
        assert_eq!(format_time(0), "0s");
        assert_eq!(format_time(45), "45s");
        assert_eq!(format_time(90), "1m30s");
        assert_eq!(format_time(3723), "1h02m03s");
    }

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

#[cfg(test)]
mod preview_tests {
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
