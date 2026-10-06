//! Log to `ezrdp.log` next to the exe (the tray app has no console window).

use std::io::Write;
use std::sync::Mutex;

pub static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init() {
    let path = std::env::current_exe().unwrap_or_default().with_file_name("ezrdp.log");
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
