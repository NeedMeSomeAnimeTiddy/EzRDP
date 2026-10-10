//! On Windows a banner shows who is in control. On Linux it's written to the log instead.

use super::capture::Monitor;
use std::sync::Mutex;

static SHOWN: Mutex<Option<String>> = Mutex::new(None);

pub fn show(name: &str, m: &Monitor, _on_click: fn()) {
    let mut shown = SHOWN.lock().unwrap();
    if shown.as_deref() != Some(name) {
        crate::log!("{name} is controlling this PC (viewing {}). Press Ctrl+C here to stop EzRDP.", m.name);
        *shown = Some(name.to_string());
    }
}

pub fn hide() {
    *SHOWN.lock().unwrap() = None;
}
