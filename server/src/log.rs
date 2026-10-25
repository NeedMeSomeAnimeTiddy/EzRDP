//! Log to `ezrdp.log` (the tray app has no console window). The previous run's log is kept as
//! `ezrdp.old.log`, so what happened before a restart isn't lost.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
static STARTED: OnceLock<Instant> = OnceLock::new();

pub fn init(path: std::path::PathBuf) {
    STARTED.get_or_init(Instant::now);
    let _ = std::fs::rename(&path, path.with_extension("old.log"));
    *FILE.lock().unwrap() = std::fs::File::create(path).ok();
}

pub fn write(msg: &str) {
    // Time since EzRDP started, so problems that build up over hours can be spotted.
    let secs = STARTED.get_or_init(Instant::now).elapsed().as_secs();
    let line = format!("[{}:{:02}:{:02}] {msg}", secs / 3600, secs / 60 % 60, secs % 60);
    eprintln!("{line}");
    // A crash while logging mustn't make every later log line crash too.
    let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = file.as_mut() {
        let _ = writeln!(f, "{line}");
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(&format!($($t)*)) };
}
