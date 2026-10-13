//! Accepts connections (phones, or the viewer on another PC) and hands logged-in ones to a session.

use crate::login::{Login, Outcome, handshake};
use crate::log;
use crate::state::App;
use std::collections::HashMap;
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Connections still logging in, by address, so a flood of them can't use up the PC.
static LOGGING_IN: Mutex<Option<HashMap<IpAddr, usize>>> = Mutex::new(None);
const MAX_LOGGING_IN: usize = 64;
const MAX_LOGGING_IN_PER_ADDRESS: usize = 8;

/// A place in `LOGGING_IN`, given back when dropped.
struct Slot(IpAddr);

impl Slot {
    fn take(ip: IpAddr) -> Option<Slot> {
        let mut guard = LOGGING_IN.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        let total: usize = map.values().sum();
        let mine = map.entry(ip).or_insert(0);
        if total >= MAX_LOGGING_IN || *mine >= MAX_LOGGING_IN_PER_ADDRESS {
            return None;
        }
        *mine += 1;
        Some(Slot(ip))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut guard = LOGGING_IN.lock().unwrap();
        if let Some(map) = guard.as_mut()
            && let Some(n) = map.get_mut(&self.0)
        {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.0);
            }
        }
    }
}

pub type Approve = fn(&str, u32) -> bool;
pub type RunSession = fn(TcpStream, Login, Vec<u8>, Arc<App>);

/// The connected phone's socket, so a new connection or Pause can hang it up.
static ACTIVE: Mutex<Option<(u64, TcpStream)>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Phones sharing their own screen with this PC: (connection, device id, socket).
static SHARING: Mutex<Vec<(u64, [u8; 16], TcpStream)>> = Mutex::new(Vec::new());

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

/// Disconnects phones sharing their screen with this PC: all of them, or one device's.
pub fn hang_up_sharing(device: Option<&[u8; 16]>) {
    for (_, id, s) in SHARING.lock().unwrap().iter() {
        if device.is_none_or(|d| d == id) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// Pause: drop everyone connected, whichever way round.
pub fn pause_all() {
    crate::session::end(crate::protocol::BYE_PAUSED);
    hang_up_sharing(None);
}

/// A device was un-trusted: drop it if it's connected.
pub fn drop_device(app: &App, id: &[u8; 16]) {
    if app.connected_id.lock().unwrap().as_ref() == Some(id) {
        crate::session::end(crate::protocol::BYE_REMOVED);
    }
    hang_up_sharing(Some(id));
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
    // Retry for a bit: right after an update the old copy may still be letting go of the port.
    let mut tries = 0;
    let listener = loop {
        match TcpListener::bind(("0.0.0.0", app.port)) {
            Ok(l) => break l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    };
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let Some(slot) = stream.peer_addr().ok().and_then(|a| Slot::take(a.ip())) else { continue };
        let app = app.clone();
        let spawned = std::thread::Builder::new().spawn(move || {
            let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let outcome = handshake(&mut stream, &app, approve);
            drop(slot);
            match outcome {
                Ok(Outcome::Session(login)) if login.role == 1 => {
                    // The phone shares its own screen; this doesn't affect a phone controlling the PC.
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
                    let name = login.device_name.clone();
                    let id = NEXT_ID.fetch_add(1, Relaxed);
                    if let Ok(s) = stream.try_clone() {
                        SHARING.lock().unwrap().push((id, login.device_id, s));
                    }
                    log!("Phone sharing its screen: {name} ({peer})");
                    run(stream, *login, Vec::new(), app.clone());
                    SHARING.lock().unwrap().retain(|(i, _, _)| *i != id);
                    log!("Phone stopped sharing: {name}");
                }
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
                    log!("Connected: {name} ({peer})");
                    *app.connected.lock().unwrap() = Some(name.clone());
                    *app.connected_id.lock().unwrap() = Some(login.device_id);
                    app.changed();
                    run(stream, *login, info(&app), app.clone());
                    log!("Disconnected: {name}");
                    let mut slot = ACTIVE.lock().unwrap();
                    if slot.as_ref().is_some_and(|(i, _)| *i == id) {
                        *slot = None;
                        *app.connected.lock().unwrap() = None;
                        *app.connected_id.lock().unwrap() = None;
                    }
                    drop(slot);
                    app.changed();
                }
                Ok(Outcome::Paired(name)) => {
                    log!("Now trusting: {name}");
                    app.changed();
                }
                Ok(Outcome::Rejected(why)) => log!("Refused {peer}: {why}"),
                Err(e) => log!("Connection from {peer} failed: {e}"),
            }
        });
        if let Err(e) = spawned {
            // Out of threads for now: drop this connection but keep listening.
            log!("Couldn't take a connection: {e}");
        }
    }
    Ok(())
}
