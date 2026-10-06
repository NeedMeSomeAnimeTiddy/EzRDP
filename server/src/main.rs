//! EzRDP server: run this on the PC, connect with the EzRDP phone app.

mod convert;
mod encoder;
mod protocol;

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

fn load_config() -> Config {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut cfg = Config { port: protocol::DEFAULT_PORT, password: String::new() };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "port" => cfg.port = v.trim().parse().unwrap_or(cfg.port),
                "password" => cfg.password = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if cfg.password.is_empty() {
        cfg.password = random_password();
        let body = format!("# EzRDP settings. Restart EzRDP after editing.\nport={}\npassword={}\n", cfg.port, cfg.password);
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

#[cfg(not(windows))]
fn main() {
    let _ = load_config;
    let _ = lan_ip;
    eprintln!("The EzRDP server only runs on Windows.");
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

    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    println!("EzRDP is running.");
    println!();
    println!("  PC name:   {name}");
    if let Some(ip) = lan_ip() {
        println!("  Home IP:   {ip}");
    }
    println!("  Port:      {}", cfg.port);
    println!("  Password:  {}", cfg.password);
    println!();
    println!("Away from home: use this PC's Tailscale IP (100.x.x.x) in the app.");
    println!("Settings file: {}", config_path().display());
    println!();

    let password = Arc::new(cfg.password);
    type Active = Option<(Arc<session::Shared>, TcpStream)>;
    let active: Arc<Mutex<Active>> = Arc::new(Mutex::new(None));

    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let (password, active) = (password.clone(), active.clone());
        std::thread::spawn(move || {
            let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let quality = match session::authenticate(&mut stream, &password) {
                Ok(Some(q)) => q,
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
            session::run(stream, quality, shared.clone());
            println!("Phone disconnected: {peer}");
            let mut slot = active.lock().unwrap();
            if slot.as_ref().is_some_and(|(s, _)| Arc::ptr_eq(s, &shared)) {
                *slot = None;
            }
        });
    }
}
