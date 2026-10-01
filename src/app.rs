//! App state and egui UI.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::clip::parse_time;
use crate::logging;
use crate::media::{MediaInfo, fetch_media_info, format_count, format_duration, format_size};
use crate::process::{CREATE_NO_WINDOW, Msg, find_ytdlp, spawn_process};
use crate::settings::{
    AUDIO_FORMATS, AUDIO_QUALITIES, Mode, Settings, ThemeChoice, VIDEO_QUALITIES,
};
use crate::update::{VersionInfo, check_ytdlp_version, is_newer};

const MAX_LOG_LINES: usize = 3000;

/// Opens GitHub's "new issue" page, which offers the bug report template.
const REPORT_ISSUE_URL: &str = "https://github.com/macedo/yt-downloader/issues/new/choose";

#[derive(Clone, Copy, Debug, PartialEq)]
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

pub struct App {
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
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_zoom_factor(1.1);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings = Settings::load();
        settings.theme.apply(&cc.egui_ctx);
        let ytdlp = find_ytdlp();
        logging::write(&match &ytdlp {
            Some(p) => format!("yt-dlp found at {}", p.display()),
            None => "yt-dlp not found".to_owned(),
        });
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
                Ok(Err(error)) => {
                    logging::write(&format!("Preview of {} failed: {error}", key.0));
                    Preview::Failed { key, error }
                }
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
                logging::write(&format!(
                    "yt-dlp version: installed {}, latest {}",
                    info.installed.as_deref().unwrap_or("unknown"),
                    info.latest.as_deref().unwrap_or("unknown (offline?)"),
                ));
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
        // Commands and errors also go to the log file; progress lines don't.
        if is_error || line.starts_with("> ") {
            logging::write(&line);
        }
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
        let args =
            crate::args::download_args(&self.settings, self.cut_range().ok().flatten(), &urls);
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
                    logging::write(&format!("{:?} finished: {}", job.kind, self.status));
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
                ui.menu_button("⚙", |ui| {
                    ui.label(egui::RichText::new("Theme").weak());
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
                    ui.separator();
                    if ui.button("Open log folder").clicked() {
                        if let Some(dir) = logging::log_dir() {
                            let _ = Command::new("explorer").arg(dir).spawn();
                        }
                        ui.close();
                    }
                    if ui.button("Report a problem…").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(REPORT_ISSUE_URL));
                        ui.close();
                    }
                    ui.separator();
                    ui.label(
                        egui::RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).weak(),
                    );
                })
                .response
                .on_hover_text("Settings and help");

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
