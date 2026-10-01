//! Running external programs (yt-dlp, winget) and finding yt-dlp.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Keeps child processes (yt-dlp, winget, ffmpeg) from opening console windows.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Messages sent from the child-process threads to the UI.
pub enum Msg {
    Line(String),
    Progress {
        pct: f32,
        speed: String,
        eta: String,
    },
    Done(Option<i32>),
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

/// Process with no console window, UTF-8 output and a PATH that includes what
/// winget installed (ffmpeg/deno), even if this process has a stale PATH.
pub fn background_command(exe: &Path) -> Command {
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

pub fn find_ytdlp() -> Option<PathBuf> {
    search_paths()
        .into_iter()
        .map(|d| d.join("yt-dlp.exe"))
        .find(|p| p.is_file())
}

pub fn spawn_process(exe: &Path, args: &[String]) -> std::io::Result<(u32, Receiver<Msg>)> {
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
