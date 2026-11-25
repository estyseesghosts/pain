//! Startup and error reporting.
//!
//! The Windows `shell` build is a GUI-subsystem binary, so `eprintln!` goes
//! nowhere. Diagnostics are appended to a small text file instead. Nothing in
//! this module reads configuration or creates UI; startup stays cheap.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// `PAIN_LOG` overrides the destination; otherwise use the platform's normal
/// per-user data directory.
#[must_use]
pub fn log_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PAIN_LOG") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share"))
                .unwrap_or_else(std::env::temp_dir)
        });
    base.join("pain").join("pain.log")
}

/// Append one line. Best effort: logging must never abort terminal startup.
pub fn log(level: &str, scope: &str, message: &str) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let line = format_log_line(millis, level, scope, message);
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}");
    }
    // Kept for headless builds and for a user who launched from a terminal.
    eprintln!("{line}");
}

pub fn error(scope: &str, message: &str) {
    log("ERROR", scope, message);
}

pub fn info(scope: &str, message: &str) {
    log("INFO", scope, message);
}

/// Pure timestamp conversion so the log format is testable without touching
/// the filesystem or the clock. Howard Hinnant's civil-from-days algorithm.
fn timestamp_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let within_day = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        within_day / 3600,
        (within_day % 3600) / 60,
        within_day % 60
    )
}

#[must_use]
pub fn format_log_line(millis: u128, level: &str, scope: &str, message: &str) -> String {
    format!(
        "{} [{level}] {scope}: {message}",
        timestamp_utc((millis / 1000) as u64)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamp_is_human_readable() {
        assert_eq!(timestamp_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp_utc(86_400), "1970-01-02T00:00:00Z");
        assert_eq!(timestamp_utc(3600 + 12), "1970-01-01T01:00:12Z");
    }

    #[test]
    fn log_line_carries_level_scope_and_message() {
        let line = format_log_line(0, "ERROR", "shell", "spawn failed");
        assert_eq!(line, "1970-01-01T00:00:00Z [ERROR] shell: spawn failed");
    }
}
