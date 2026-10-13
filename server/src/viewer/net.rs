//! Talking to an EzRDP PC: finding it on the network, pairing, logging in and the live session.
//! This side plays the same part as the phone app (see android/.../Connection.kt).

use super::audio::Player;
use crate::crypto::{self, Seal};
use crate::pairing::{self, KeyPair};
use crate::protocol::*;
use super::store::SavedPc;
use crate::trust::hex;
use super::video::Video;
use std::io::{self, BufReader};
use std::net::{Shutdown, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum ConnError {
    Io(io::Error),
    /// The PC runs a different EzRDP version.
    Version(u8),
    Paused,
    /// The PC doesn't know this computer (it was removed there).
    NotTrusted,
    /// Something answered at that address, but it's a different PC.
    WrongPc,
    /// Pairing was refused (RESULT_* code).
    Refused(u8),
}

impl From<io::Error> for ConnError {
    fn from(e: io::Error) -> Self {
        ConnError::Io(e)
    }
}

impl ConnError {
    /// Worth trying the next address?
    fn try_next(&self) -> bool {
        matches!(self, ConnError::Io(_) | ConnError::WrongPc)
    }

    pub fn advice(&self, pc: &str) -> String {
        match self {
            ConnError::Io(e) if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock => format!(
                "{pc} didn't answer. Check EzRDP is running on it. Away from home, the PC's router must let port 7270 through."
            ),
            ConnError::Io(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                format!("{pc} is switched on but EzRDP isn't running on it.")
            }
            ConnError::Io(e) => format!("Couldn't reach {pc} ({e})."),
            ConnError::Version(v) if *v < VERSION => format!(
                "{pc} runs an older EzRDP (version {v}; this one is version {VERSION}). Update EzRDP on {pc}."
            ),
            ConnError::Version(v) => format!(
                "{pc} runs a newer EzRDP (version {v}; this one is version {VERSION}). Update the EzRDP viewer here."
            ),
            ConnError::Paused => format!("Connections are paused on {pc}. Resume them from its tray icon."),
            ConnError::NotTrusted => format!("{pc} no longer trusts this computer. Forget it here and pair again."),
            ConnError::WrongPc => format!("Another PC answered at {pc}'s address."),
            ConnError::Refused(RESULT_NOT_LOCAL) => {
                "Pairing only works when both computers are on the same home network.".into()
            }
            ConnError::Refused(RESULT_BUSY) => format!("{pc} is already pairing with something else. Try again in a moment."),
            ConnError::Refused(RESULT_PAUSED) => format!("Connections are paused on {pc}."),
            ConnError::Refused(_) => format!("{pc} said no."),
        }
    }
}

pub struct Hello {
    pub pc_id: [u8; 16],
    nonce: [u8; 16],
    pub paused: bool,
    pub name: String,
}

fn read_hello(s: &mut TcpStream) -> Result<Hello, ConnError> {
    let not_ezrdp = || ConnError::Io(io::Error::new(io::ErrorKind::InvalidData, "not an EzRDP PC"));
    let (kind, body) = read_msg(s).map_err(|_| not_ezrdp())?;
    if kind != S_HELLO || body.is_empty() {
        return Err(not_ezrdp());
    }
    if body[0] != VERSION {
        return Err(ConnError::Version(body[0]));
    }
    if body.len() < 34 {
        return Err(not_ezrdp());
    }
    Ok(Hello {
        pc_id: body[1..17].try_into().unwrap(),
        nonce: body[17..33].try_into().unwrap(),
        paused: body[33] != 0,
        name: String::from_utf8_lossy(&body[34..]).into_owned(),
    })
}

fn open(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "address not found");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => {
                s.set_nodelay(true)?;
                s.set_read_timeout(Some(Duration::from_secs(15)))?;
                s.set_write_timeout(Some(Duration::from_secs(10)))?;
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "PC".into())
}

// ---------- Finding PCs ----------

#[derive(Clone)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub paused: bool,
    pub version: u8,
    seen: Instant,
}

/// Asks "EZRDP?" on the local network every couple of seconds and keeps a list of who answers.
/// `own` is this PC's id: it answers too, but isn't one to control.
pub fn discover(found: Arc<Mutex<Vec<Found>>>, ctx: eframe::egui::Context, own: Option<String>) {
    std::thread::spawn(move || {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
        let _ = sock.set_broadcast(true);
        let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
        let mut last_ask = Instant::now() - Duration::from_secs(10);
        let mut buf = [0u8; 512];
        loop {
            if last_ask.elapsed() > Duration::from_secs(2) {
                last_ask = Instant::now();
                let _ = sock.send_to(b"EZRDP?", ("255.255.255.255", DEFAULT_PORT));
                let mut list = found.lock().unwrap();
                let before = list.len();
                list.retain(|f| f.seen.elapsed() < Duration::from_secs(7));
                if list.len() != before {
                    ctx.request_repaint();
                }
            }
            let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
            let d = &buf[..n];
            if n < 26 || &d[..6] != b"EZRDP!" {
                continue;
            }
            let f = Found {
                version: d[6],
                id: hex(&d[7..23]),
                port: u16::from_be_bytes([d[23], d[24]]),
                paused: d[25] != 0,
                name: String::from_utf8_lossy(&d[26..]).into_owned(),
                host: from.ip().to_string(),
                seen: Instant::now(),
            };
            if own.as_deref() == Some(f.id.as_str()) {
                continue;
            }
            let mut list = found.lock().unwrap();
            match list.iter_mut().find(|x| x.id == f.id) {
                Some(x) => *x = f,
                None => {
                    list.push(f);
                    ctx.request_repaint();
                }
            }
        }
    });
}

// ---------- Pairing ----------

/// Pairs with the PC at `host`. Blocking. `on_code` gets the 6-digit code to show while the
/// person clicks Yes on the PC. `cancel` receives the socket so the UI can hang up.
pub fn pair(
    host: &str,
    port: u16,
    cancel: &Mutex<Option<TcpStream>>,
    on_code: impl FnOnce(String, String),
) -> Result<SavedPc, ConnError> {
    let mut s = open(host, port, Duration::from_secs(4))?;
    *cancel.lock().unwrap() = s.try_clone().ok();
    let hello = read_hello(&mut s)?;
    if hello.paused {
        return Err(ConnError::Refused(RESULT_PAUSED));
    }
    let ours = KeyPair::new();
    let mut start = pairing::commit(&ours.public).to_vec();
    start.extend_from_slice(computer_name().as_bytes());
    write_plain(&mut s, C_PAIR_START, &start)?;
    let (kind, pc_pub) = read_msg(&mut s)?;
    if kind == S_PAIR_RESULT {
        return Err(ConnError::Refused(pc_pub.first().copied().unwrap_or(RESULT_DENIED)));
    }
    if kind != S_PAIR_KEY || pc_pub.len() != 65 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad pairing reply").into());
    }
    write_plain(&mut s, C_PAIR_KEY, &ours.public)?;
    on_code(pairing::format_code(pairing::code(&ours.public, &pc_pub)), hello.name.clone());
    // Time for the person to walk over to the PC and click Yes.
    s.set_read_timeout(Some(Duration::from_secs(130)))?;
    let (kind, result) = read_msg(&mut s)?;
    if kind != S_PAIR_RESULT || result.first() != Some(&RESULT_OK) || result.len() < 17 {
        return Err(ConnError::Refused(result.first().copied().unwrap_or(RESULT_DENIED)));
    }
    let key = ours
        .device_key(&pc_pub, &ours.public, &pc_pub)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad PC key"))?;
    Ok(SavedPc {
        id: hex(&hello.pc_id),
        name: hello.name,
        device_id: result[1..17].try_into().unwrap(),
        key,
        home: host.to_string(),
        internet: String::new(),
        manual: String::new(),
        port,
    })
}

// ---------- Session ----------

#[derive(Clone)]
pub struct MonitorInfo {
    pub index: u8,
    pub primary: bool,
    pub w: u16,
    pub h: u16,
    pub name: String,
}

/// What the PC has told us, read by the UI each frame.
#[derive(Default)]
pub struct State {
    pub monitors: Vec<MonitorInfo>,
    pub monitor: u8,
    /// Size of the video picture.
    pub video: (u16, u16),
    /// Set once the connection is gone: (was it ended on purpose by the PC (BYE_*), error).
    pub ended: Option<(Option<u8>, String)>,
    /// New addresses to remember: home, internet, port.
    pub addresses: Option<(String, String, u16)>,
    /// Something the PC wants the person to know, and when it arrived.
    pub notice: Option<(String, std::time::Instant)>,
}

pub struct Session {
    pub state: Arc<Mutex<State>>,
    pub video: Arc<Video>,
    out: mpsc::Sender<(u8, Vec<u8>)>,
    stream: TcpStream,
    closed: Arc<AtomicBool>,
}

impl Session {
    /// Tries the PC's addresses, best first, and logs in.
    pub fn connect(pc: &SavedPc, live: Option<&str>, quality: u8, ctx: eframe::egui::Context) -> Result<Self, ConnError> {
        let mut last = ConnError::Io(io::Error::new(io::ErrorKind::NotFound, "no address known for this PC"));
        for (host, timeout) in pc.addresses(live) {
            match login(&host, pc, quality, timeout) {
                Ok((stream, tx, rx)) => return Ok(Self::start(stream, tx, rx, ctx)),
                Err(e) if e.try_next() => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    fn start(stream: TcpStream, tx: Seal, rx: Seal, ctx: eframe::egui::Context) -> Self {
        let sender = Arc::new(Sender::new(stream.try_clone().unwrap(), tx));
        let state = Arc::new(Mutex::new(State::default()));
        let closed = Arc::new(AtomicBool::new(false));
        let video = Video::start(ctx.clone(), {
            let sender = sender.clone();
            move || {
                let _ = sender.send(C_KEYFRAME, &[]);
            }
        });
        let (out, queue) = mpsc::channel::<(u8, Vec<u8>)>();

        // Writer: the UI never waits on the network.
        {
            let (sender, stream) = (sender.clone(), stream.try_clone().unwrap());
            std::thread::spawn(move || {
                while let Ok((kind, body)) = queue.recv() {
                    if sender.send(kind, &body).is_err() {
                        let _ = stream.shutdown(Shutdown::Both);
                        break;
                    }
                }
            });
        }
        // Keep-alive, so the PC can tell a dead connection.
        {
            let (out, closed) = (out.clone(), closed.clone());
            std::thread::spawn(move || {
                while !closed.load(Relaxed) && out.send((C_PING, Vec::new())).is_ok() {
                    std::thread::sleep(Duration::from_secs(3));
                }
            });
        }
        // Reader.
        {
            let (state, video, closed) = (state.clone(), video.clone(), closed.clone());
            let reader = BufReader::with_capacity(1 << 16, stream.try_clone().unwrap());
            std::thread::spawn(move || {
                let mut rx = Receiver::new_video(reader, rx);
                let mut audio: Option<Player> = None;
                let mut bye = None;
                let error = loop {
                    let (kind, body) = match rx.read() {
                        Ok(m) => m,
                        Err(e) => break e.to_string(),
                    };
                    let mut p = Payload(&body);
                    match kind {
                        S_VIDEO_FRAME if body.len() > 5 => {
                            // Confirm at once so the PC can send the next picture.
                            let _ = sender.send(C_FRAME_ACK, &body[..4]);
                            video.push(body[5..].to_vec());
                        }
                        S_VIDEO_CONFIG => {
                            if let (Some(m), Some(w), Some(h)) = (p.u8(), p.u16(), p.u16()) {
                                let mut s = state.lock().unwrap();
                                s.monitor = m;
                                s.video = (w, h);
                            }
                        }
                        S_MONITORS => {
                            let list = parse_monitors(&body);
                            state.lock().unwrap().monitors = list;
                        }
                        S_AUDIO_CONFIG => {
                            if let (Some(rate), Some(ch)) = (p.u32(), p.u8()) {
                                let player = audio.get_or_insert_with(Player::start);
                                player.configure(rate, ch);
                            }
                        }
                        S_AUDIO => {
                            if let Some(a) = &audio {
                                a.push(&body);
                            }
                        }
                        S_INFO => {
                            if let Some(a) = parse_info(&body) {
                                state.lock().unwrap().addresses = Some(a);
                            }
                        }
                        S_BYE => bye = p.u8(),
                        S_NOTICE => {
                            let text = String::from_utf8_lossy(&body).into_owned();
                            state.lock().unwrap().notice = Some((text, std::time::Instant::now()));
                        }
                        _ => {} // S_PING, S_CURSOR (the pointer is drawn by this computer)
                    }
                    if !matches!(kind, S_PING | S_AUDIO | S_CURSOR | S_VIDEO_FRAME) {
                        ctx.request_repaint();
                    }
                };
                closed.store(true, Relaxed);
                video.stop();
                state.lock().unwrap().ended = Some((bye, error));
                ctx.request_repaint();
            });
        }
        Self { state, video, out, stream, closed }
    }

    pub fn send(&self, kind: u8, body: Vec<u8>) {
        let _ = self.out.send((kind, body));
    }

    pub fn mouse_abs(&self, x: f32, y: f32) {
        let mut b = x.to_be_bytes().to_vec();
        b.extend_from_slice(&y.to_be_bytes());
        self.send(C_MOUSE_ABS, b);
    }

    pub fn button(&self, button: u8, down: bool) {
        self.send(C_MOUSE_BUTTON, vec![button, down as u8]);
    }

    pub fn scroll(&self, dy: i32, dx: i32) {
        let mut b = (dy.clamp(-32000, 32000) as i16).to_be_bytes().to_vec();
        b.extend_from_slice(&(dx.clamp(-32000, 32000) as i16).to_be_bytes());
        self.send(C_SCROLL, b);
    }

    pub fn key(&self, vk: u16, down: bool) {
        let mut b = vk.to_be_bytes().to_vec();
        b.push(down as u8);
        self.send(C_KEY, b);
    }

    pub fn text(&self, s: &str) {
        self.send(C_TEXT, s.as_bytes().to_vec());
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Relaxed)
    }

    pub fn close(&self) {
        self.closed.store(true, Relaxed);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

fn login(host: &str, pc: &SavedPc, quality: u8, timeout: Duration) -> Result<(TcpStream, Seal, Seal), ConnError> {
    let mut s = open(host, pc.port, timeout)?;
    let hello = read_hello(&mut s)?;
    if hex(&hello.pc_id) != pc.id {
        return Err(ConnError::WrongPc);
    }
    if hello.paused {
        return Err(ConnError::Paused);
    }
    let mut nc = [0u8; 16];
    getrandom::fill(&mut nc).map_err(io::Error::other)?;
    let ns = hello.nonce;
    let mut auth = pc.device_id.to_vec();
    auth.extend_from_slice(&nc);
    auth.extend_from_slice(&crypto::derive(&pc.key, b"auth", &ns, &nc));
    // quality, role 0 (control the PC), no size limit, graphics-card encoder allowed.
    auth.extend_from_slice(&[quality, 0, 0, 0, 0, 0, 1]);
    write_plain(&mut s, C_AUTH, &auth)?;
    let (kind, body) = read_msg(&mut s)?;
    match (kind, body.first()) {
        (S_AUTH_RESULT, Some(&RESULT_OK)) => {}
        (S_AUTH_RESULT, Some(&RESULT_PAUSED)) => return Err(ConnError::Paused),
        _ => return Err(ConnError::NotTrusted),
    }
    // The PC pings every 2 seconds, so 10 seconds of silence means the link is dead.
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let tx = Seal::new(&crypto::derive(&pc.key, b"c2s", &ns, &nc));
    let rx = Seal::new(&crypto::derive(&pc.key, b"s2c", &ns, &nc));
    Ok((s, tx, rx))
}

fn parse_monitors(body: &[u8]) -> Vec<MonitorInfo> {
    let mut p = Payload(body);
    let mut out = Vec::new();
    let count = p.u8().unwrap_or(0);
    for _ in 0..count {
        let (Some(index), Some(primary), Some(w), Some(h), Some(len)) = (p.u8(), p.u8(), p.u16(), p.u16(), p.u16()) else {
            break;
        };
        let len = (len as usize).min(p.0.len());
        let name = String::from_utf8_lossy(&p.0[..len]).into_owned();
        p.0 = &p.0[len..];
        out.push(MonitorInfo { index, primary: primary != 0, w, h, name });
    }
    out
}

fn parse_info(body: &[u8]) -> Option<(String, String, u16)> {
    let mut p = Payload(body);
    let mut text = || {
        let n = (p.u8()? as usize).min(p.0.len());
        let s = String::from_utf8_lossy(&p.0[..n]).into_owned();
        p.0 = &p.0[n..];
        Some(s)
    };
    let home = text()?;
    let internet = text()?;
    Some((home, internet, p.u16()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitors_and_info() {
        let m = vec![2, 0, 1, 7, 128, 4, 56, 0, 1, b'A', 1, 0, 5, 0, 2, 208, 0, 0];
        let list = parse_monitors(&m);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].w, list[0].h, list[0].name.as_str(), list[0].primary), (1920, 1080, "A", true));
        assert_eq!((list[1].index, list[1].w, list[1].h), (1, 1280, 720));

        let info = [3, b'1', b'.', b'2', 0, 0x1c, 0x66];
        assert_eq!(parse_info(&info), Some(("1.2".into(), String::new(), 7270)));
    }
}
