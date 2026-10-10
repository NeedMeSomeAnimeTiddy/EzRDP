//! Log to `ezrdp.log` (the tray app has no console window).

use std::io::Write;
use std::sync::Mutex;

pub static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init(path: std::path::PathBuf) {
    *FILE.lock().unwrap() = std::fs::File::create(path).ok();
}

pub fn write(msg: &str) {
    eprintln!("{msg}");
    if let Some(f) = FILE.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{msg}");
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(&format!($($t)*)) };
}
