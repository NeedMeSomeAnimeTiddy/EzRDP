//! Accepts phone connections and hands logged-in ones to a session.

use crate::login::{Login, Outcome, handshake};
use crate::log;
use crate::state::App;
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Approve = fn(&str, u32) -> bool;
pub type RunSession = fn(TcpStream, Login, Vec<u8>, Arc<App>);

/// The connected phone's socket, so a new connection or Pause can hang it up.
static ACTIVE: Mutex<Option<(u64, TcpStream)>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// Disconnects whoever is connected (used by Pause).
pub fn hang_up() {
    if let Some((_, s)) = ACTIVE.lock().unwrap().take() {
        let _ = s.shutdown(Shutdown::Both);
    }
}

/// Addresses the phone should remember: home ip, internet ip, port.
fn info(app: &App) -> Vec<u8> {
    let mut out = Vec::new();
    for ip in [lan_ip(), *app.public_ip.lock().unwrap()] {
        let s = ip.map(|i| i.to_string()).unwrap_or_default();
        out.push(s.len() as u8);
        out.extend_from_slice(s.as_bytes());
    }
    out.extend_from_slice(&app.port.to_be_bytes());
    out
}

pub fn listen(app: Arc<App>, approve: Approve, run: RunSession) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", app.port))?;
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let app = app.clone();
        std::thread::spawn(move || {
            let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            match handshake(&mut stream, &app, &approve) {
                Ok(Outcome::Session(login)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
                    let id = NEXT_ID.fetch_add(1, Relaxed);
                    {
                        let mut slot = ACTIVE.lock().unwrap();
                        if let Some((_, old)) = slot.take() {
                            let _ = old.shutdown(Shutdown::Both);
                        }
                        *slot = stream.try_clone().ok().map(|s| (id, s));
                    }
                    let name = login.device_name.clone();
                    log!("Phone connected: {name} ({peer})");
                    *app.connected.lock().unwrap() = Some(name.clone());
                    app.changed();
                    run(stream, *login, info(&app), app.clone());
                    log!("Phone disconnected: {name}");
                    let mut slot = ACTIVE.lock().unwrap();
                    if slot.as_ref().is_some_and(|(i, _)| *i == id) {
                        *slot = None;
                        *app.connected.lock().unwrap() = None;
                    }
                    drop(slot);
                    app.changed();
                }
                Ok(Outcome::Paired(name)) => {
                    log!("Now trusting phone: {name}");
                    app.changed();
                }
                Ok(Outcome::Rejected(why)) => log!("Refused {peer}: {why}"),
                Err(e) => log!("Connection from {peer} failed: {e}"),
            }
        });
    }
    Ok(())
}
