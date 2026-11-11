//! Tiny append-only diagnostic log.
//!
//! The log exists to explain hangs: every directory load, image decode and job
//! is recorded with a timestamp and duration, and slow loads are marked `WARN`.
//! It is intentionally dependency-free, never panics, and can be disabled with
//! `YAZI2D_LOG=off`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, SyncSender},
    OnceLock,
};

use chrono::Local;

/// Rotate the log once it grows past this size.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

struct Sink {
    sender: SyncSender<String>,
    path: PathBuf,
}

static SINK: OnceLock<Option<Sink>> = OnceLock::new();

/// Open the log sink once. Safe to call more than once; later calls are no-ops.
pub fn init() {
    let _ = SINK.set(open_sink());
    info(format!(
        "Yazi2D {} started (pid {}, cwd {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "?".to_string())
    ));
}

/// Whether a log file is active.
pub fn enabled() -> bool {
    matches!(SINK.get(), Some(Some(_)))
}

/// Path of the active log file, if any.
pub fn path() -> Option<&'static Path> {
    match SINK.get() {
        Some(Some(sink)) => Some(sink.path.as_path()),
        _ => None,
    }
}

pub fn info(message: impl AsRef<str>) {
    write("INFO", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write("WARN", message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    write("ERROR", message.as_ref());
}

fn write(level: &str, message: &str) {
    let Some(Some(sink)) = SINK.get() else {
        return;
    };
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let _ = sink
        .sender
        .try_send(format!("{timestamp} {level:<5} {message}"));
}

fn open_sink() -> Option<Sink> {
    let requested = std::env::var("YAZI2D_LOG").ok();
    let path = match requested.as_deref() {
        Some(value) if is_off(value) => return None,
        Some(value) if !value.is_empty() && !is_on(value) => PathBuf::from(value),
        _ => default_path()?,
    };

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    rotate(&path);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let (sender, receiver) = mpsc::sync_channel::<String>(1024);
    std::thread::spawn(move || {
        let mut file = file;
        while let Ok(line) = receiver.recv() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    });
    Some(Sink { sender, path })
}

fn is_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "none"
    )
}

fn is_on(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

fn default_path() -> Option<PathBuf> {
    Some(crate::favorites::config_dir().join("yazi2d.log"))
}

fn rotate(path: &Path) {
    let too_big = fs::metadata(path)
        .map(|meta| meta.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    if too_big {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_switches_are_recognized() {
        assert!(is_off("off"));
        assert!(is_off(" OFF "));
        assert!(is_off("0"));
        assert!(!is_off("1"));
        assert!(is_on("on"));
        assert!(is_on("TRUE"));
        assert!(!is_on("off"));
    }

    #[test]
    fn logging_without_init_is_a_no_op() {
        // Tests never call `init`, so this must not create a file or panic.
        info("this line goes nowhere");
        warn("this one too");
        error("and this one");
        assert!(!enabled());
        assert!(path().is_none());
    }
}
