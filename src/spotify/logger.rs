use log::{Level, LevelFilter, Log, Metadata, Record};
use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static LOGGER: QuietSpotifyLogger = QuietSpotifyLogger;

struct QuietSpotifyLogger;

fn has_log_file() -> bool {
    LOG_FILE
        .get()
        .is_some_and(|m| m.lock().unwrap_or_else(|e| e.into_inner()).is_some())
}

fn with_log_file(f: impl FnOnce(&mut File)) {
    if let Some(file_mutex) = LOG_FILE.get() {
        let mut guard = file_mutex.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut file) = *guard {
            f(file);
        }
    }
}

impl Log for QuietSpotifyLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if has_log_file() {
            metadata.level() <= Level::Info
        } else {
            metadata.target().starts_with("librespot") && metadata.level() <= Level::Warn
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target();
        let level = record.level();
        let message = format!("{}", record.args());

        if target.starts_with("librespot") && matches!(level, Level::Error | Level::Warn) {
            let mut guard = LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(message.clone());
        }

        with_log_file(|file| {
            let _ = writeln!(file, "[{level}] {target}: {message}");
            let _ = file.flush();
        });
    }

    fn flush(&self) {
        with_log_file(|file| {
            let _ = file.flush();
        });
    }
}

pub fn init() {
    let file_opened = LOG_FILE
        .get_or_init(|| {
            let file = env::var("CRYSTALIFY_LOG")
                .ok()
                .filter(|p| !p.trim().is_empty())
                .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
            Mutex::new(file)
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();

    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(if file_opened {
            LevelFilter::Info
        } else {
            LevelFilter::Warn
        });
    }
}

pub fn clear_last_error() {
    let mut guard = LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

#[allow(dead_code)]
pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn take_last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quiet_logger_captures_librespot_errors_and_ignores_other_targets() {
        clear_last_error();
        assert_eq!(last_error(), None);

        let other_record = Record::builder()
            .level(Level::Error)
            .target("reqwest::connect")
            .args(format_args!("network glitch"))
            .build();
        LOGGER.log(&other_record);
        assert_eq!(last_error(), None);

        let info_record = Record::builder()
            .level(Level::Info)
            .target("librespot_playback::player")
            .args(format_args!("loading track"))
            .build();
        LOGGER.log(&info_record);
        assert_eq!(last_error(), None);

        let warn_record = Record::builder()
            .level(Level::Warn)
            .target("librespot_playback::player")
            .args(format_args!("audio key timeout"))
            .build();
        LOGGER.log(&warn_record);
        assert_eq!(last_error(), Some("audio key timeout".to_string()));

        let err_record = Record::builder()
            .level(Level::Error)
            .target("librespot_core::channel")
            .args(format_args!("channel closed"))
            .build();
        LOGGER.log(&err_record);
        assert_eq!(take_last_error(), Some("channel closed".to_string()));
        assert_eq!(last_error(), None);
    }
}
