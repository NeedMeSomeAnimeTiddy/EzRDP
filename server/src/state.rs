//! State shared by the tray, the network listener and sessions.

use crate::trust::Trusted;
use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

pub struct App {
    pub pc_id: [u8; 16],
    pub pc_name: String,
    pub port: u16,
    pub paused: AtomicBool,
    pub trusted: Mutex<Trusted>,
    /// Name of the phone currently connected, if any.
    pub connected: Mutex<Option<String>>,
    /// This PC's internet address, when known.
    pub public_ip: Mutex<Option<IpAddr>>,
    /// Only one "Allow this phone?" prompt at a time.
    pub pairing: AtomicBool,
    /// Called when something the tray shows has changed.
    pub on_change: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl App {
    pub fn paused(&self) -> bool {
        self.paused.load(Relaxed)
    }

    pub fn changed(&self) {
        if let Some(f) = self.on_change.lock().unwrap().as_ref() {
            f();
        }
    }
}
