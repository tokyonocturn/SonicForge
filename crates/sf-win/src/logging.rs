//! Minimal rolling file logger.
//!
//! The audio thread never logs. Everything here is called from normal threads
//! (startup, reconfiguration, user actions) and must therefore stay cheap and
//! must never panic: logging failure is never fatal.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum log size before rotation (bytes).
const MAX_SIZE: u64 = 2 * 1024 * 1024;

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

struct Logger {
    path: PathBuf,
    file: File,
    size: u64,
}

fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    // Convert to a readable UTC timestamp without pulling in a date crate.
    let days = secs / 86_400;
    let time = secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{:03}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60,
        now.subsec_millis()
    )
}

/// Howard Hinnant's days-to-civil conversion.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Initialises the logger, rotating an oversized file.
pub fn init() {
    let dir = crate::paths::log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("sonicforge.log");
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_SIZE {
        let _ = std::fs::rename(&path, dir.join("sonicforge.old.log"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        if let Ok(mut guard) = LOGGER.lock() {
            *guard = Some(Logger { path, file, size });
        }
    }
}

fn write(level: &str, message: &str) {
    let line = format!("{} [{level}] {message}\n", timestamp());
    if let Ok(mut guard) = LOGGER.lock() {
        if guard.is_none() {
            drop(guard);
            init();
            guard = match LOGGER.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
        }
        if let Some(logger) = guard.as_mut() {
            let _ = logger.file.write_all(line.as_bytes());
            let _ = logger.file.flush();
            logger.size += line.len() as u64;
            if logger.size > MAX_SIZE {
                let dir = logger.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let _ = std::fs::rename(&logger.path, dir.join("sonicforge.old.log"));
                if let Ok(file) = OpenOptions::new().create(true).append(true).open(&logger.path) {
                    logger.file = file;
                    logger.size = 0;
                }
            }
        }
    }
    // Mirror to stderr when a console is attached, which makes CLI debugging
    // pleasant without reading the log file.
    #[cfg(windows)]
    {
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            eprint!("{line}");
        }
    }
    #[cfg(not(windows))]
    eprint!("{line}");
}

pub fn info(message: &str) {
    write("info", message);
}

pub fn warn(message: &str) {
    write("warn", message);
}

pub fn error(message: &str) {
    write("error", message);
}

/// Reads the tail of the log file, newest last. Used by the diagnostics panel.
pub fn tail(max_lines: usize) -> Vec<String> {
    let path = crate::paths::log_dir().join("sonicforge.log");
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
            if lines.len() > max_lines {
                lines.drain(..lines.len() - max_lines);
            }
            lines
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_correct() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
    }

    #[test]
    fn timestamp_is_iso_like() {
        let ts = timestamp();
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.len(), 24, "{ts}");
    }
}
