//! Keeps EzRDP answering phones. If part of it gets stuck for good (a crashed thread, a session
//! that never finishes closing, or the port no longer answering), it writes down what happened
//! and starts a fresh copy: the same thing as quitting and starting EzRDP by hand.

use crate::protocol::S_HELLO;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

static STARTED: OnceLock<Instant> = OnceLock::new();
static RESTARTING: AtomicBool = AtomicBool::new(false);
/// Restarting straight after starting could go round in circles: only log then.
const SETTLE: Duration = Duration::from_secs(120);

/// Turns on the checks. Call once, after the log is set up.
pub fn start(port: u16) {
    STARTED.get_or_init(Instant::now);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let thread = std::thread::current();
        crate::log!("Crash in thread '{}': {info}", thread.name().unwrap_or("unnamed"));
        restart("part of EzRDP crashed");
    }));
    std::thread::spawn(move || {
        let mut failed = 0;
        loop {
            std::thread::sleep(Duration::from_secs(60));
            match answers(port) {
                Ok(()) => failed = 0,
                Err(e) => {
                    failed += 1;
                    crate::log!("Self-check: the port didn't answer ({e})");
                    if failed >= 2 {
                        restart("the port stopped answering");
                        failed = 0;
                    }
                }
            }
        }
    });
}

/// Connects to this PC's own port like a phone would and waits for the greeting.
fn answers(port: u16) -> std::io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut kind = [0u8; 1];
    s.read_exact(&mut kind)?;
    if kind[0] != S_HELLO {
        return Err(std::io::Error::other("unexpected reply"));
    }
    Ok(())
}

/// Starts a fresh copy of EzRDP, which takes over from this one.
pub fn restart(why: &str) {
    if STARTED.get().is_some_and(|t| t.elapsed() < SETTLE) {
        crate::log!("Not restarting ({why}): EzRDP only just started");
        return;
    }
    if RESTARTING.swap(true, Relaxed) {
        return;
    }
    crate::log!("Restarting EzRDP: {why}");
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe).args(["--restarted", "--background"]).spawn()
    });
    if let Err(e) = started {
        crate::log!("Couldn't restart EzRDP: {e}");
        RESTARTING.store(false, Relaxed);
    }
}

/// Restarts EzRDP if it isn't dropped within `limit`: for steps that must never hang.
pub struct Watch(Arc<AtomicBool>);

impl Watch {
    pub fn new(what: &'static str, limit: Duration) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            if !flag.load(Relaxed) {
                restart(&format!("{what} was still stuck after {} seconds", limit.as_secs()));
            }
        });
        Watch(done)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn self_check_needs_the_greeting() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut good, _) = l.accept().unwrap();
            good.write_all(&[S_HELLO, 0, 0, 0, 0]).unwrap();
            let (silent, _) = l.accept().unwrap();
            drop(silent);
        });
        assert!(answers(port).is_ok());
        assert!(answers(port).is_err());
        server.join().unwrap();
    }
}
