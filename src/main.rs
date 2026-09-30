// Esconde a janela de console no build de release.
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

/// Impede que processos filhos (yt-dlp, winget, ffmpeg) abram janelas de console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_LOG_LINES: usize = 3000;

const VIDEO_QUALITIES: &[(&str, Option<u32>)] = &[
    ("Melhor disponível", None),
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
    ("FLAC (sem perdas)", "flac"),
    ("WAV", "wav"),
    ("Original (sem conversão)", "best"),
];

const AUDIO_QUALITIES: &[(&str, &str)] = &[
    ("Máxima (VBR 0)", "0"),
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

/// Mensagens enviadas pelas threads do processo filho para a interface.
enum Msg {
    Line(String),
    Progress { pct: f32, speed: String, eta: String },
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
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_zoom_factor(1.1);
        let settings = Settings::load();
        settings.theme.apply(&cc.egui_ctx);
        let ytdlp = find_ytdlp();
        let status = match &ytdlp {
            Some(_) => "Pronto.".to_owned(),
            None => "yt-dlp não encontrado — clique em \"Instalar yt-dlp\".".to_owned(),
        };
        Self {
            saved_settings: settings.clone(),
            settings,
            urls: String::new(),
            ytdlp,
            job: None,
            log: Vec::new(),
            progress: 0.0,
            status,
            item_info: String::new(),
        }
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        let is_error = line.starts_with("ERROR") || line.contains("ERRO:");
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

    fn build_args(&self, urls: &[String]) -> Vec<String> {
        let s = &self.settings;
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
            "%(title)s.%(ext)s".into(),
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
                // WAV não suporta capa embutida.
                if s.embed_thumbnail && fmt != "wav" {
                    a.extend(["--embed-thumbnail".into(), "--convert-thumbnails".into(), "jpg".into()]);
                }
            }
        }

        a.push("--".into());
        a.extend(urls.iter().cloned());
        a
    }

    fn start_download(&mut self) {
        let urls = self.urls();
        if urls.is_empty() {
            self.status = "Cole pelo menos um link válido (http/https).".into();
            return;
        }
        let Some(exe) = self.ytdlp.clone() else {
            self.status = "yt-dlp não encontrado. Instale primeiro.".into();
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&self.settings.out_dir) {
            self.status = format!("Não foi possível criar a pasta de destino: {e}");
            return;
        }
        let args = self.build_args(&urls);
        self.log.clear();
        self.push_log(format!("> yt-dlp {}", args.join(" ")));
        self.progress = 0.0;
        self.item_info.clear();
        self.run(JobKind::Download, &exe, &args, "Iniciando…");
    }

    fn install_or_update_ytdlp(&mut self) {
        let is_winget = self.ytdlp.as_ref().is_none_or(|p| {
            p.to_string_lossy().to_lowercase().contains("winget")
        });
        let (exe, args, label): (PathBuf, Vec<String>, &str) = match (&self.ytdlp, is_winget) {
            (Some(p), false) => (p.clone(), vec!["-U".into()], "Atualizando yt-dlp…"),
            (existing, _) => (
                PathBuf::from("winget"),
                [
                    if existing.is_some() { "upgrade" } else { "install" },
                    "-e",
                    "--id",
                    "yt-dlp.yt-dlp",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ]
                .map(String::from)
                .to_vec(),
                if existing.is_some() { "Atualizando yt-dlp via winget…" } else { "Instalando yt-dlp via winget (pode demorar)…" },
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
                self.job = Some(Job { kind, pid, rx, cancelled: false });
            }
            Err(e) => {
                self.status = format!("Falha ao iniciar {}: {e}", exe.display());
                self.push_log(format!("ERRO: {e}"));
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(job) = &mut self.job {
            job.cancelled = true;
            // /T encerra também os filhos (ffmpeg).
            let mut cmd = Command::new("taskkill");
            cmd.args(["/T", "/F", "/PID", &job.pid.to_string()]);
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            let _ = cmd.output();
            self.status = "Cancelando…".into();
        }
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let msgs: Vec<Msg> = job.rx.try_iter().collect();
        for msg in msgs {
            match msg {
                Msg::Progress { pct, speed, eta } => {
                    self.progress = pct / 100.0;
                    self.status = format!("Baixando… {pct:.1}%   {speed}   restante {eta}");
                }
                Msg::Line(line) => {
                    if let Some(rest) = line.strip_prefix("[download] Downloading item ") {
                        self.item_info = format!("Item {rest}");
                    } else if line.starts_with("[ExtractAudio]") {
                        self.status = "Convertendo áudio…".into();
                    } else if line.starts_with("[Merger]") {
                        self.status = "Juntando vídeo e áudio…".into();
                    } else if line.starts_with("[EmbedThumbnail]") {
                        self.status = "Embutindo capa…".into();
                    }
                    self.push_log(line);
                }
                Msg::Done(code) => {
                    let job = self.job.take().expect("job ativo");
                    let errors = self.log.iter().filter(|(_, e)| *e).count();
                    self.status = if job.cancelled {
                        "Cancelado.".into()
                    } else if code == Some(0) {
                        self.progress = 1.0;
                        match job.kind {
                            JobKind::Download => "Concluído! ✔".into(),
                            JobKind::Tooling => "yt-dlp instalado/atualizado.".into(),
                        }
                    } else {
                        format!(
                            "Terminou com erro (código {}, {errors} erro(s) no log).",
                            code.map_or("?".to_owned(), |c| c.to_string())
                        )
                    };
                    if job.kind == JobKind::Tooling {
                        self.ytdlp = find_ytdlp();
                        if let Some(p) = &self.ytdlp {
                            self.push_log(format!("yt-dlp em: {}", p.display()));
                        }
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

    /// Atalhos de teclado. Roda antes dos widgets para que o campo de links
    /// não receba as mesmas teclas.
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
                let btn = egui::Button::new(egui::RichText::new("⏹  Cancelar").strong())
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
                if ui.add(btn).on_hover_text("Interromper (Esc)").clicked() {
                    self.cancel();
                }
            } else {
                let btn = egui::Button::new(egui::RichText::new("⬇  Baixar").strong())
                    .fill(ui.visuals().selection.bg_fill);
                let why_disabled = if self.ytdlp.is_none() {
                    "Instale o yt-dlp primeiro"
                } else {
                    "Cole um ou mais links"
                };
                if ui
                    .add_enabled(self.ytdlp.is_some() && has_urls, btn)
                    .on_hover_text("Baixar os links (Ctrl+Enter)")
                    .on_disabled_hover_text(why_disabled)
                    .clicked()
                {
                    self.start_download();
                }
            }

            ui.separator();

            ui.add_enabled_ui(!busy, |ui| {
                ui.selectable_value(&mut self.settings.mode, Mode::Video, "🎞  Vídeo")
                    .on_hover_text("Baixar vídeo (MP4)");
                ui.selectable_value(&mut self.settings.mode, Mode::Audio, "🎵  Áudio")
                    .on_hover_text("Extrair somente o áudio");
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("🌓", |ui| {
                    for (choice, label) in [
                        (ThemeChoice::System, "Seguir o sistema"),
                        (ThemeChoice::Light, "Claro"),
                        (ThemeChoice::Dark, "Escuro"),
                    ] {
                        if ui.radio_value(&mut self.settings.theme, choice, label).clicked() {
                            choice.apply(ui.ctx());
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Tema");

                let (label, tip) = if self.ytdlp.is_some() {
                    ("🔄  Atualizar yt-dlp", "Atualizar o yt-dlp para a versão mais recente")
                } else {
                    ("⬇  Instalar yt-dlp", "Instalar yt-dlp, FFmpeg e Deno via winget")
                };
                if ui.add_enabled(!busy, egui::Button::new(label)).on_hover_text(tip).clicked() {
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
                    section(ui, "🎞  Vídeo");
                    ui.label("Qualidade");
                    combo(ui, "vq", &mut self.settings.video_quality, VIDEO_QUALITIES.iter().map(|q| q.0));
                    ui.add_space(2.0);
                    ui.weak("Salvo em MP4 com o melhor áudio.");
                }
                Mode::Audio => {
                    section(ui, "🎵  Áudio");
                    ui.label("Formato");
                    combo(ui, "af", &mut self.settings.audio_format, AUDIO_FORMATS.iter().map(|f| f.0));
                    ui.add_space(6.0);
                    let lossy = !matches!(AUDIO_FORMATS[self.settings.audio_format].1, "flac" | "wav" | "best");
                    ui.add_enabled_ui(lossy, |ui| {
                        ui.label("Qualidade");
                        combo(ui, "aq", &mut self.settings.audio_quality, AUDIO_QUALITIES.iter().map(|q| q.0));
                    });
                }
            }

            ui.add_space(16.0);
            section(ui, "⚙  Opções");
            ui.checkbox(&mut self.settings.playlist, "Baixar playlist inteira")
                .on_hover_text("Se o link fizer parte de uma playlist, baixa todos os itens");
            ui.checkbox(&mut self.settings.embed_thumbnail, "Embutir capa")
                .on_hover_text("Grava a miniatura do vídeo como capa do arquivo");

            ui.add_space(16.0);
            section(ui, "🗀  Destino");
            ui.add(egui::Label::new(self.settings.out_dir.display().to_string()).wrap());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Alterar…").clicked() {
                    self.choose_folder();
                }
                if ui.button("Abrir").clicked() {
                    self.open_folder();
                }
            });
        });
    }

    fn main_area(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Links").strong());
            ui.weak("um por linha — vídeos ou playlists");
            let count = self.urls().len();
            if count > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(if count == 1 { "1 link".to_owned() } else { format!("{count} links") });
                });
            }
        });
        ui.add_enabled(
            !busy,
            egui::TextEdit::multiline(&mut self.urls)
                .id_salt("urls")
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .hint_text("Cole aqui: https://www.youtube.com/watch?v=…"),
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Log").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!self.log.is_empty(), egui::Button::new("Limpar log").small()).clicked() {
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
                            ui.weak("Nenhuma atividade ainda.");
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
        let running_download = self.job.as_ref().is_some_and(|j| j.kind == JobKind::Download);

        ui.add_space(6.0);
        ui.add(
            egui::ProgressBar::new(self.progress)
                .desired_height(8.0)
                .animate(busy && (self.progress == 0.0 || !running_download)),
        );
        ui.horizontal(|ui| {
            // Direita primeiro, para o status (à esquerda) truncar no espaço que sobra.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, color, tip) = match &self.ytdlp {
                    Some(p) => ("yt-dlp", egui::Color32::from_rgb(60, 170, 90), p.display().to_string()),
                    None => (
                        "yt-dlp ausente",
                        ui.visuals().error_fg_color,
                        "Use \"Instalar yt-dlp\" na barra de ferramentas".to_owned(),
                    ),
                };
                ui.label(egui::RichText::new(label).small()).on_hover_text(&tip);
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        self.handle_input(ui.ctx());
        if self.job.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.settings != self.saved_settings {
            self.settings.save();
            self.saved_settings = self.settings.clone();
        }

        egui::Panel::top("toolbar")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .show(ui, |ui| self.status_bar(ui));
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

fn combo<'a>(ui: &mut egui::Ui, id: &str, selected: &mut usize, items: impl Iterator<Item = &'a str> + Clone) {
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

/// PATH atualizado: o do processo + o do registro (usuário e máquina).
/// O winget adiciona yt-dlp, ffmpeg e deno ao PATH do registro, que este
/// processo só enxergaria depois de reiniciar o Explorer/sessão.
fn search_paths() -> Vec<PathBuf> {
    let mut dirs_out: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
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
            (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        ];
        for (root, key) in sources {
            if let Ok(value) = RegKey::predef(root)
                .open_subkey(key)
                .and_then(|k| k.get_value::<String, _>("Path"))
            {
                dirs_out.extend(value.split(';').filter(|s| !s.is_empty()).map(|s| PathBuf::from(expand_env(s))));
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

/// Expande variáveis no formato %VAR% (valores REG_EXPAND_SZ).
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

fn find_ytdlp() -> Option<PathBuf> {
    search_paths().into_iter().map(|d| d.join("yt-dlp.exe")).find(|p| p.is_file())
}

fn spawn_process(exe: &Path, args: &[String]) -> std::io::Result<(u32, Receiver<Msg>)> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    // Garante que o yt-dlp ache ffmpeg/deno instalados pelo winget.
    if let Ok(joined) = std::env::join_paths(search_paths()) {
        cmd.env("PATH", joined);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn()?;
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
        // winget usa \r para animar o progresso; fica só com o último trecho.
        let line = text.trim_end().rsplit('\r').next().unwrap_or("").trim_end().to_owned();
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
    eframe::run_native("YT Downloader", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
