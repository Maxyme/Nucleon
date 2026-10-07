use log::{Level, Metadata, Record, SetLoggerError};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

struct HookFileLogger {
    file: Mutex<Option<File>>,
}

impl log::Log for HookFileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let log_line = format!(
            "{} [nucleon-hook] {:<5} [{}] {}\n",
            timestamp,
            record.level(),
            record.target(),
            record.args()
        );

        // Also write to stderr if stderr is valid
        eprint!("{log_line}");

        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.write_all(log_line.as_bytes());
                let _ = f.flush();
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.flush();
            }
        }
    }
}

pub fn init(log_path: PathBuf) -> Result<(), SetLoggerError> {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let logger = Box::new(HookFileLogger {
        file: Mutex::new(file),
    });

    log::set_boxed_logger(logger)?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}
