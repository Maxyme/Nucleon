use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use log::{Level, Metadata, Record, SetLoggerError};

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

        let timestamp = current_timestamp();
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

fn current_timestamp() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 64];
        let len = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c"%Y-%m-%d %H:%M:%S".as_ptr(),
            &tm,
        );
        if len > 0 {
            String::from_utf8_lossy(&buf[..len as usize]).to_string()
        } else {
            String::new()
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
