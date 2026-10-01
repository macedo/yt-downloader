//! Log file in `%APPDATA%\yt-downloader\logs`, including panics, so problems
//! can be diagnosed even though release builds have no console.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The log is rotated to `yt-downloader.old.log` once it grows past this.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

static LOG: Mutex<Option<File>> = Mutex::new(None);

pub fn log_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("yt-downloader").join("logs"))
}

/// Opens the log file and installs a panic hook that records panics in it.
/// Logging is best effort: if the file can't be opened, the app runs anyway.
pub fn init() {
    let Some(dir) = log_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("yt-downloader.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("yt-downloader.old.log"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The message includes the file and line; release builds are stripped,
        // so a backtrace would have no symbols.
        write(&format!("PANIC: {info}"));
        default_hook(info);
    }));
}

/// Appends a timestamped line to the log file.
pub fn write(message: &str) {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{} {message}", timestamp(SystemTime::now()));
    }
}

/// `2026-10-01 14:05:09Z` (UTC).
fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to a (year, month, day) date, using Howard
/// Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::timestamp;

    #[test]
    fn formats_utc_timestamps() {
        let at = |secs| timestamp(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01 00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(at(1_000_000_000), "2001-09-09 01:46:40Z");
        assert_eq!(at(1_790_863_509), "2026-10-01 14:05:09Z");
        assert_eq!(at(4_102_444_799), "2099-12-31 23:59:59Z");
    }
}
