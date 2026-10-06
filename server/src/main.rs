#![cfg_attr(not(windows), allow(dead_code))]
//! EzRDP server: run this on the PC, connect with the EzRDP phone app.

mod convert;
mod crypto;
mod encoder;
mod login;
mod protocol;
mod upnp;

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod session;

use std::path::PathBuf;

struct Config {
    port: u16,
    password: String,
    /// Open the port on the router automatically so the phone can connect from anywhere.
    internet: bool,
    salt: [u8; 16],
}

fn config_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name("ezrdp.conf")
}

fn random_password() -> String {
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("no random source");
    bytes.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn parse_salt(s: &str) -> Option<[u8; 16]> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn load_config() -> Config {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut cfg = Config { port: protocol::DEFAULT_PORT, password: String::new(), internet: true, salt: [0; 16] };
    let mut salt = None;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "port" => cfg.port = v.trim().parse().unwrap_or(cfg.port),
                "password" => cfg.password = v.trim().to_string(),
                "internet" => cfg.internet = !matches!(v.trim(), "off" | "no" | "false" | "0"),
                "salt" => salt = parse_salt(v),
                _ => {}
            }
        }
    }
    let incomplete = cfg.password.is_empty() || salt.is_none();
    if cfg.password.is_empty() {
        cfg.password = random_password();
    }
    cfg.salt = salt.unwrap_or_else(|| {
        let mut s = [0u8; 16];
        getrandom::fill(&mut s).expect("no random source");
        s
    });
    if incomplete {
        let body = format!(
            "# EzRDP settings. Restart EzRDP after editing.\n\
             port={}\n\
             password={}\n\
             # internet=on lets your phone connect from anywhere (opens the port on your router automatically).\n\
             internet={}\n\
             # Don't change this one.\n\
             salt={}\n",
            cfg.port,
            cfg.password,
            if cfg.internet { "on" } else { "off" },
            hex(&cfg.salt)
        );
        if let Err(e) = std::fs::write(&path, body) {
            eprintln!("Could not save {}: {e}", path.display());
        }
    }
    cfg
}

fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// Off Windows there's no screen to share; `ezrdp-server test-server <password>` runs a
/// protocol test server used to check the phone app's login and encryption.
#[cfg(not(windows))]
fn main() {
    let _ = load_config;
    let _ = upnp::open;
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("test-server") {
        eprintln!("The EzRDP server only runs on Windows.");
        return;
    }
    let password = args.get(2).cloned().unwrap_or_default();
    let salt = [9u8; 16];
    let master = crypto::master_key(&password, &salt);
    let listener = std::net::TcpListener::bind("127.0.0.1:7270").unwrap();
    println!("listening");
    let (mut stream, _) = listener.accept().unwrap();
    let Some(l) = login::authenticate(&mut stream, &master, &salt).unwrap() else {
        println!("auth failed");
        return;
    };
    println!("auth ok quality={}", l.quality);
    let tx = protocol::Sender::new(stream.try_clone().unwrap(), l.tx);
    tx.send(protocol::S_INFO, lan_ip().map(|i| i.to_string()).unwrap_or_default().as_bytes()).unwrap();
    tx.send_parts(protocol::S_VIDEO_FRAME, &[0, 0, 0, 1, 1], &[0, 0, 0, 1, 0x67, 0xAA]).unwrap();
    let mut rx = protocol::Receiver::new(stream, l.rx);
    while let Ok((kind, body)) = rx.read() {
        println!("got {kind:#04x} {body:?}");
    }
}

#[cfg(windows)]
fn main() {
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    unsafe {
        use windows::Win32::UI::HiDpi::*;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let cfg = load_config();
    let listener = match TcpListener::bind(("0.0.0.0", cfg.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Can't listen on port {}: {e}", cfg.port);
            std::thread::sleep(Duration::from_secs(10));
            return;
        }
    };

    println!("EzRDP is starting...");
    let master = crypto::master_key(&cfg.password, &cfg.salt);
    let internet = cfg.internet.then(|| upnp::open(cfg.port, lan_ip));

    println!();
    println!("EzRDP is running.");
    println!();
    match &internet {
        Some(upnp::Status::Open(ip)) => {
            println!("  Address:   {ip}   (works from anywhere)");
        }
        Some(upnp::Status::SharedAddress(ip)) => {
            println!("  Couldn't open EzRDP to the internet: your internet provider shares one");
            println!("  address between homes ({ip}), so outside connections can't get in.");
            println!("  It will still work on your home Wi-Fi.");
        }
        Some(upnp::Status::Failed(why)) => {
            println!("  Couldn't open EzRDP to the internet automatically: {why}.");
            println!("  Fix: turn on UPnP in your router, or forward TCP port {} to this PC.", cfg.port);
        }
        None => {}
    }
    if let Some(ip) = lan_ip() {
        println!("  Home IP:   {ip}");
    }
    println!("  Port:      {}", cfg.port);
    println!("  Password:  {}", cfg.password);
    println!();
    println!("Settings file: {}", config_path().display());
    println!();

    let salt = cfg.salt;
    type Active = Option<(Arc<session::Shared>, TcpStream)>;
    let active: Arc<Mutex<Active>> = Arc::new(Mutex::new(None));

    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let active = active.clone();
        std::thread::spawn(move || {
            let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let login = match login::authenticate(&mut stream, &master, &salt) {
                Ok(Some(l)) => l,
                _ => {
                    println!("Rejected connection from {peer} (wrong password?)");
                    return;
                }
            };
            let shared = session::new_shared();
            {
                let mut slot = active.lock().unwrap();
                if let Some((old, old_stream)) = slot.take() {
                    session::kick(&old, &old_stream);
                }
                *slot = Some((shared.clone(), stream.try_clone().unwrap()));
            }
            println!("Phone connected: {peer}");
            let home = lan_ip().map(|ip| ip.to_string());
            session::run(stream, login, home, shared.clone());
            println!("Phone disconnected: {peer}");
            let mut slot = active.lock().unwrap();
            if slot.as_ref().is_some_and(|(s, _)| Arc::ptr_eq(s, &shared)) {
                *slot = None;
            }
        });
    }
}
