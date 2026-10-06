//! EzRDP: lives in the tray on the PC; the EzRDP phone app connects to it.
#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(windows, windows_subsystem = "windows")]

mod convert;
mod crypto;
mod discovery;
mod encoder;
mod log;
mod login;
mod pairing;
mod protocol;
mod server;
mod state;
mod stun;
mod trust;
mod upnp;

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod firewall;
#[cfg(windows)]
mod hwenc;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod phone_view;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod tray;

use state::App;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

struct Config {
    port: u16,
    /// Open the port on the router automatically so the phone can connect from anywhere.
    internet: bool,
    pc_id: [u8; 16],
    name: String,
    /// Set after the first run, so "Start with Windows" is only switched on once.
    first_run_done: bool,
}

fn data_path(file: &str) -> PathBuf {
    std::env::current_exe().unwrap_or_default().with_file_name(file)
}

fn load_config() -> Config {
    let path = data_path("ezrdp.conf");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut cfg = Config {
        port: protocol::DEFAULT_PORT,
        internet: true,
        pc_id: [0; 16],
        name: String::new(),
        first_run_done: false,
    };
    let mut id = None;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            match k.trim() {
                "port" => cfg.port = v.parse().unwrap_or(cfg.port),
                "internet" => cfg.internet = !matches!(v, "off" | "no" | "false" | "0"),
                "id" => id = trust::unhex::<16>(v),
                "name" => cfg.name = v.to_string(),
                "first_run_done" => cfg.first_run_done = v == "yes",
                _ => {}
            }
        }
    }
    let incomplete = id.is_none() || !cfg.first_run_done;
    cfg.pc_id = id.unwrap_or_else(|| {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("no random source");
        b
    });
    if incomplete {
        save_config(&cfg);
    }
    if cfg.name.is_empty() {
        cfg.name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "My PC".into());
    }
    cfg
}

fn save_config(cfg: &Config) {
    let body = format!(
        "# EzRDP settings. Restart EzRDP after editing.\n\
         port={}\n\
         # internet=on lets trusted phones connect from anywhere (opens the port on your router).\n\
         internet={}\n\
         # Name shown on the phone (blank = this PC's name).\n\
         name={}\n\
         # Don't change these.\n\
         id={}\n\
         first_run_done=yes\n",
        cfg.port,
        if cfg.internet { "on" } else { "off" },
        cfg.name,
        trust::hex(&cfg.pc_id)
    );
    let _ = std::fs::write(data_path("ezrdp.conf"), body);
}

fn new_app(cfg: &Config) -> Arc<App> {
    Arc::new(App {
        pc_id: cfg.pc_id,
        pc_name: cfg.name.clone(),
        port: cfg.port,
        paused: AtomicBool::new(false),
        trusted: Mutex::new(trust::Trusted::load(data_path("trusted.txt"))),
        connected: Mutex::new(None),
        public_ip: Mutex::new(None),
        pairing: AtomicBool::new(false),
        on_change: Mutex::new(None),
    })
}

/// Finds this PC's internet address and opens the router port, in the background.
fn reach_internet(app: Arc<App>, internet: bool) -> String {
    let upnp = internet.then(|| upnp::open(app.port, server::lan_ip));
    let stun = stun::public_ip();
    let upnp_ip = match &upnp {
        Some(upnp::Status::Open(ip)) => Some(*ip),
        _ => None,
    };
    *app.public_ip.lock().unwrap() = stun.or(upnp_ip);
    match upnp {
        None => "Internet access: off (internet=off in ezrdp.conf)".into(),
        Some(upnp::Status::Open(_)) => "Router: port opened automatically".into(),
        Some(upnp::Status::SharedAddress(ip)) => format!(
            "Router: your internet provider shares one address between homes (CGNAT, router sees {ip}), \
             so phones can only connect on home Wi-Fi"
        ),
        Some(upnp::Status::Failed(why)) => {
            format!("Router: couldn't open the port automatically ({why}). Fine if you forwarded port {} yourself.", app.port)
        }
    }
}

/// Off Windows there's no screen to share; `ezrdp-server test-server` runs the network side
/// (pairing auto-approves and prints the code) so the phone app's protocol can be tested.
#[cfg(not(windows))]
fn main() {
    if std::env::args().nth(1).as_deref() != Some("test-server") {
        eprintln!("EzRDP only runs on Windows.");
        return;
    }
    let cfg = load_config();
    let app = new_app(&cfg);
    std::thread::spawn({
        let app = app.clone();
        move || discovery::run(app)
    });
    fn approve(name: &str, code: u32) -> bool {
        println!("pair request from {name}, code {}", pairing::format_code(code));
        true
    }
    fn session(stream: std::net::TcpStream, l: login::Login, info: Vec<u8>, _app: Arc<App>) {
        println!("session for {} quality={}", l.device_name, l.quality);
        let tx = protocol::Sender::new(stream.try_clone().unwrap(), l.tx);
        tx.send(protocol::S_INFO, &info).unwrap();
        tx.send_parts(protocol::S_VIDEO_FRAME, &[0, 0, 0, 1, 1], &[0, 0, 0, 1, 0x67, 0xAA]).unwrap();
        let mut rx = protocol::Receiver::new(stream, l.rx);
        while let Ok((kind, body)) = rx.read() {
            println!("got {kind:#04x} {body:?}");
        }
    }
    println!("test server listening on {}", cfg.port);
    server::listen(app, approve, session).unwrap();
}

/// Closes the copy of EzRDP that's already running and waits until it's gone.
/// Asks nicely first; older versions don't know how to quit on request, so they're ended.
#[cfg(windows)]
fn take_over(mutex: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;
    unsafe {
        let got = |ms| {
            let r = WaitForSingleObject(mutex, ms);
            r == WAIT_OBJECT_0 || r == WAIT_ABANDONED
        };
        let Ok(old) = FindWindowW(w!("EzRDPTray"), None) else {
            return got(5000);
        };
        // Look this up first: older versions destroy the window when asked to close.
        let mut pid = 0u32;
        GetWindowThreadProcessId(old, Some(&mut pid));
        let _ = PostMessageW(Some(old), WM_CLOSE, WPARAM(0), LPARAM(0));
        if got(3000) {
            return true;
        }
        if let Ok(p) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(p, 0);
            let _ = CloseHandle(p);
        }
        // Remove the old copy's tray icon so it doesn't linger.
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: old,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        got(5000)
    }
}

#[cfg(windows)]
fn main() {
    use std::time::Duration;
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{HSTRING, w};

    // Started elevated just to set up the firewall.
    if std::env::args().any(|a| a == "--setup-firewall") {
        let _ = firewall::allow();
        return;
    }

    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\EzRDP"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Another copy is running (usually the old version after an update): replace it.
            let Ok(mutex) = mutex else { return };
            if !take_over(mutex) {
                MessageBoxW(
                    None,
                    w!("EzRDP is already running and couldn't be closed. Right-click its icon near the clock, choose Quit, then start EzRDP again."),
                    w!("EzRDP"),
                    MB_OK,
                );
                return;
            }
        }
        use windows::Win32::UI::HiDpi::*;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    log::init();

    let cfg = load_config();
    // First run: start with Windows. Later runs: keep "start with Windows" pointing at this
    // copy, in case it was moved or replaced.
    if !cfg.first_run_done || autostart::enabled() {
        autostart::set(true);
    }
    let app = new_app(&cfg);
    *app.on_change.lock().unwrap() = Some(Box::new(tray::refresh));

    fn fix_firewall() {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        unsafe {
            // Shows the Windows "allow changes?" prompt; the elevated copy adds the rule and exits.
            ShellExecuteW(None, w!("runas"), &HSTRING::from(exe), w!("--setup-firewall"), None, SW_HIDE);
        }
    }
    let _ = tray::FIX_FIREWALL.set(fix_firewall);

    // Firewall, router and internet address: in the background so the tray appears at once.
    {
        let app = app.clone();
        let internet = cfg.internet;
        std::thread::spawn(move || {
            let fw = match firewall::allow() {
                firewall::Status::Ok => true,
                firewall::Status::NeedsAdmin => {
                    fix_firewall();
                    std::thread::sleep(Duration::from_secs(10));
                    matches!(firewall::allow(), firewall::Status::Ok)
                }
                firewall::Status::Failed(why) => {
                    log!("firewall: {why}");
                    false
                }
            };
            tray::FIREWALL_OK.store(fw, std::sync::atomic::Ordering::Relaxed);
            let router = reach_internet(app.clone(), internet);
            let fw_text = if fw { "Firewall: allowed" } else { "Firewall: NOT set up (use the tray menu to fix)" };
            log!("{fw_text}\n{router}");
            *tray::EXTRA_INFO.lock().unwrap() = format!("{fw_text}\n{router}");
            // Keep the internet address fresh (it can change).
            loop {
                std::thread::sleep(Duration::from_secs(30 * 60));
                if let Some(ip) = stun::public_ip() {
                    *app.public_ip.lock().unwrap() = Some(ip);
                }
            }
        });
    }

    std::thread::spawn({
        let app = app.clone();
        move || discovery::run(app)
    });
    std::thread::spawn({
        let app = app.clone();
        move || {
            if let Err(e) = server::listen(app, tray::approve, on_session) {
                log!("Can't listen on the port: {e}");
                unsafe {
                    MessageBoxW(
                        None,
                        &HSTRING::from(format!("EzRDP can't start: port is in use ({e}).")),
                        w!("EzRDP"),
                        MB_OK | MB_ICONERROR,
                    );
                }
                std::process::exit(1);
            }
        }
    });

    fn on_session(stream: std::net::TcpStream, l: login::Login, info: Vec<u8>, app: Arc<App>) {
        if l.role == 1 {
            phone_view::run(stream, l, app);
            return;
        }
        tray::notify("Phone connected", &format!("{} is now controlling this PC.", l.device_name));
        session::run(stream, l, info, app);
    }

    log!("EzRDP started: {} ({} trusted phones)", app.pc_name, app.trusted.lock().unwrap().devices.len());
    if app.trusted.lock().unwrap().devices.is_empty() {
        tray::notify(
            "EzRDP is running",
            "Open EzRDP on your phone (same Wi-Fi) and tap this PC to pair it.",
        );
    }
    tray::run(app);
}
