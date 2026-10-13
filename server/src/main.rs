//! EzRDP: lives in the tray on the PC; the EzRDP phone app (or the EzRDP viewer on another PC)
//! connects to it. On Linux it runs from a terminal or the desktop's autostart instead.
#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(windows, windows_subsystem = "windows")]

mod convert;
mod crypto;
mod discovery;
mod echo;
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

// Linux (X11): same session code, with Linux screen capture, input and sound.
#[cfg(target_os = "linux")]
#[path = "linux/audio.rs"]
mod audio;
#[cfg(target_os = "linux")]
#[path = "linux/capture.rs"]
mod capture;
#[cfg(target_os = "linux")]
#[path = "linux/hwenc.rs"]
mod hwenc;
#[cfg(target_os = "linux")]
#[path = "linux/input.rs"]
mod input;
#[cfg(target_os = "linux")]
#[path = "linux/overlay.rs"]
mod overlay;
#[cfg(target_os = "linux")]
#[path = "linux/wayland.rs"]
mod wayland;
#[cfg(target_os = "linux")]
#[path = "linux/tray.rs"]
mod tray;
#[cfg(target_os = "linux")]
mod session;

// The window for controlling other PCs (same program, so there's one app to install).
#[cfg(any(windows, target_os = "linux"))]
#[allow(dead_code)]
mod viewer;

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

#[cfg(not(target_os = "linux"))]
fn data_path(file: &str) -> PathBuf {
    std::env::current_exe().unwrap_or_default().with_file_name(file)
}

/// On Linux settings live in ~/.config/ezrdp (the program itself is often somewhere read-only).
#[cfg(target_os = "linux")]
fn data_path(file: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("ezrdp");
    let _ = std::fs::create_dir_all(&dir);
    // The trusted-device keys live here: keep other users on this PC out.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir.join(file)
}

/// This PC's own id (hex), so the window doesn't list this PC as one to control.
pub fn own_pc_id() -> Option<String> {
    let text = std::fs::read_to_string(data_path("ezrdp.conf")).ok()?;
    text.lines().find_map(|l| l.trim().strip_prefix("id=").map(|v| v.trim().to_string()))
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "My PC".into())
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
        cfg.name = computer_name();
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
        connected_id: Mutex::new(None),
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

/// `ezrdp-server test-server` runs only the network side (pairing auto-approves and prints
/// the code, nothing is captured) so the apps' protocol can be tested anywhere.
#[cfg(not(windows))]
fn test_server() {
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

#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("test-server") {
        test_server();
    } else {
        eprintln!("EzRDP can be controlled on Windows and Linux only.");
    }
}

/// Asks the person at this PC whether to trust a new phone or PC: a desktop dialog if
/// zenity or kdialog is installed, otherwise a question in the terminal.
#[cfg(target_os = "linux")]
fn approve_linux(name: &str, code: u32) -> bool {
    use std::io::IsTerminal;
    use std::process::{Command, Stdio};
    let code = pairing::format_code(code);
    // No markup in the name: zenity and kdialog would treat tags in it as formatting.
    let name = name.replace(['<', '>', '&'], "");
    let text = format!(
        "A device wants to control this PC. Check it shows this code:\n\n    {code}\n\n\
         It calls itself: {name}\n\nOnly click Yes if it's yours and the codes match."
    );
    let quiet = |c: &mut Command| c.stdout(Stdio::null()).stderr(Stdio::null()).status();
    let zenity = ["--question", "--title=EzRDP", "--width=360", "--default-cancel", "--timeout=120"];
    if let Ok(s) = quiet(Command::new("zenity").args(zenity).arg(format!("--text={text}"))) {
        return s.success();
    }
    if let Ok(s) = quiet(Command::new("kdialog").args(["--title", "EzRDP", "--yesno", &text])) {
        return s.success();
    }
    if std::io::stdin().is_terminal() {
        println!("\n{text}\nType y and press Enter to allow: ");
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        return answer.trim().eq_ignore_ascii_case("y") || answer.trim().eq_ignore_ascii_case("yes");
    }
    log!("Pairing request from {name} refused: install zenity, or run EzRDP in a terminal to answer it.");
    false
}

#[cfg(target_os = "linux")]
fn autostart_file() -> PathBuf {
    data_path("").parent().map(|p| p.join("autostart")).unwrap_or_default().join("ezrdp.desktop")
}

#[cfg(target_os = "linux")]
pub fn linux_autostart_enabled() -> bool {
    autostart_file().exists()
}

/// Starts EzRDP when you sign in to the desktop (or stops doing that).
#[cfg(target_os = "linux")]
pub fn linux_autostart(on: bool) {
    let file = autostart_file();
    let dir = file.parent().map(PathBuf::from).unwrap_or_default();
    if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        let _ = std::fs::create_dir_all(&dir);
        let body = format!("[Desktop Entry]\nType=Application\nName=EzRDP\nComment=Lets your phone or another PC control this PC\nExec=\"{exe}\" --background\nX-GNOME-Autostart-enabled=true\n");
        match std::fs::write(&file, body) {
            Ok(()) => println!("EzRDP will start when you sign in ({}).", file.display()),
            Err(e) => println!("Couldn't set that up: {e}"),
        }
    } else {
        let _ = std::fs::remove_file(&file);
        println!("EzRDP won't start by itself any more.");
    }
}

/// Lets phones and other PCs control this PC, in the background. Returns the thread that keeps
/// listening, or why this PC can't be controlled.
#[cfg(target_os = "linux")]
fn linux_host() -> Result<Arc<App>, String> {
    use std::time::Duration;
    // On Wayland the desktop asks permission when someone first connects; on X11 check now.
    if !wayland::active() && capture::monitors().is_empty() {
        return Err("Other devices can't control this PC: EzRDP can't see the screen. \
                    Start EzRDP from inside your desktop session (DISPLAY must be set)."
            .into());
    }

    let cfg = load_config();
    let app = new_app(&cfg);
    {
        let app = app.clone();
        let internet = cfg.internet;
        std::thread::spawn(move || {
            log!("{}", reach_internet(app.clone(), internet));
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

    fn on_session(stream: std::net::TcpStream, l: login::Login, info: Vec<u8>, app: Arc<App>) {
        if l.role == 1 {
            log!("{} wanted to share its screen; showing a phone's screen only works on Windows for now.", l.device_name);
            return;
        }
        // A desktop notification, like the Windows tray balloon (if notify-send is installed).
        let _ = std::process::Command::new("notify-send")
            .args(["EzRDP", &format!("{} is now controlling this PC.", l.device_name)])
            .stderr(std::process::Stdio::null())
            .spawn();
        session::run(stream, l, info, app);
    }

    let trusted = app.trusted.lock().unwrap().devices.len();
    log!(
        "This PC can be controlled: {} (home address {}, port {}, {trusted} trusted devices).\n\
         To add a phone or PC: on the same network, open EzRDP on it and pick this PC.\n\
         Settings and trusted devices are in {}.",
        app.pc_name,
        server::lan_ip().map(|i| i.to_string()).unwrap_or_else(|| "unknown".into()),
        app.port,
        data_path("").display()
    );
    let port = cfg.port;
    std::thread::spawn({
        let app = app.clone();
        move || {
            if let Err(e) = server::listen(app, approve_linux, on_session) {
                log!("Other devices can't control this PC: port {port} is in use ({e}).");
            }
        }
    });
    Ok(app)
}

/// Where a running EzRDP listens for "open your window" from a second start.
#[cfg(target_os = "linux")]
fn instance_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("ezrdp.sock")
}

/// Becomes the running EzRDP: later starts ask it to open the window instead of starting again.
#[cfg(target_os = "linux")]
fn claim_instance() {
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    let path = instance_socket();
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let mut b = [0u8; 1];
            if conn.read_exact(&mut b).is_ok() && b[0] == b'w' {
                open_window();
            }
        }
    });
}

/// Opens the window for controlling another PC, as its own process so the tray keeps going.
#[cfg(target_os = "linux")]
pub fn open_window() {
    if let Ok(exe) = std::env::current_exe()
        && let Ok(mut child) = std::process::Command::new(exe).arg("--window").spawn()
    {
        // Collect it when the window closes, so it doesn't linger as a dead process.
        std::thread::spawn(move || child.wait());
    }
}

#[cfg(target_os = "linux")]
fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        "test-server" => return test_server(),
        "--autostart" => return linux_autostart(std::env::args().nth(2).as_deref() != Some("off")),
        "--help" | "-h" => {
            println!(
                "EzRDP: control other PCs from this one, and let your phone or other PCs control this one.\n\n\
                 ezrdp                    tray icon + the EzRDP window; this PC can be controlled while EzRDP runs\n\
                 ezrdp --background       tray icon only (what starting at sign-in uses)\n\
                 ezrdp --window           only the window\n\
                 ezrdp --autostart        let others control this PC from when you sign in\n\
                 ezrdp --autostart off    undo that\n\n\
                 Settings and trusted devices: {}",
                data_path("").display()
            );
            return;
        }
        "--window" => {
            if let Err(e) = viewer::run() {
                eprintln!("Can't open the EzRDP window: {e}");
            }
            return;
        }
        _ => {}
    }
    // Already running? Ask it to open its window (not for --background) and stop here.
    if let Ok(mut conn) = std::os::unix::net::UnixStream::connect(instance_socket()) {
        if arg != "--background" {
            use std::io::Write;
            let _ = conn.write_all(b"w");
        }
        return;
    }
    log::init(data_path("ezrdp.log"));
    claim_instance();
    let (app, problem) = match linux_host() {
        Ok(app) => (Some(app), None),
        Err(e) => {
            log!("{e}");
            (None, Some(e))
        }
    };
    tray::run(app, problem);
    if arg != "--background" {
        open_window();
    }
    // The tray menu's Quit (or Ctrl+C) ends EzRDP.
    loop {
        std::thread::park();
    }
}

/// Opens the window for controlling another PC, as a separate process so the tray keeps going.
#[cfg(windows)]
pub fn open_window() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--window").spawn();
    }
}

/// Whether the copy of EzRDP in the tray is this very program (same file, not an older version).
#[cfg(windows)]
fn running_is_this_program() -> bool {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PWSTR, w};
    unsafe {
        let Ok(win) = FindWindowW(w!("EzRDPTray"), None) else { return false };
        let mut pid = 0u32;
        GetWindowThreadProcessId(win, Some(&mut pid));
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(p);
        if !ok {
            return false;
        }
        let theirs = std::fs::metadata(String::from_utf16_lossy(&buf[..len as usize]));
        let ours = std::env::current_exe().and_then(std::fs::metadata);
        match (theirs, ours) {
            (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
            _ => false,
        }
    }
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
    // The window for controlling another PC runs as its own process, next to the tray.
    if std::env::args().any(|a| a == "--window") {
        let _ = viewer::run();
        return;
    }
    // Started as administrator from the tray menu to turn "Control admin windows too" off.
    if std::env::args().any(|a| a == "--admin-off") {
        autostart::set_admin(false);
        return;
    }
    // Started as administrator from the tray menu to turn it on: this copy then takes over.
    let admin_on = std::env::args().any(|a| a == "--admin-on");
    // Started with Windows: just the tray, no window.
    let background = admin_on || std::env::args().any(|a| a == "--background");

    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\EzRDP"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // This same program is already in the tray: just open the window. Unless this copy was
            // started as administrator: then it replaces the normal one, so admin windows can be
            // controlled too.
            if running_is_this_program() && !input::elevated() {
                if !background {
                    open_window();
                }
                return;
            }
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
    log::init(data_path("ezrdp.log"));

    let cfg = load_config();
    if admin_on && input::elevated() {
        if autostart::set_admin(true) {
            log!("Control admin windows too: on");
        } else {
            log!("Control admin windows too: couldn't set up starting as administrator");
        }
    }
    if autostart::refresh_admin() {
        // Starts as administrator through the sign-in task: keep it pointing at this copy (in
        // case it was moved or replaced), and no Run entry, which would start a normal copy.
        if input::elevated() {
            autostart::set_admin(true);
        } else {
            autostart::set(false);
        }
    } else if !cfg.first_run_done || autostart::enabled() {
        // First run: start with Windows. Later runs: keep "start with Windows" pointing at this
        // copy, in case it was moved or replaced.
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
        tray::notify("Connected", &format!("{} is now controlling this PC.", l.device_name));
        session::run(stream, l, info, app);
    }

    log!("EzRDP started: {} ({} trusted phones)", app.pc_name, app.trusted.lock().unwrap().devices.len());
    if !background {
        open_window();
    }
    if app.trusted.lock().unwrap().devices.is_empty() {
        tray::notify(
            "EzRDP is running",
            "Open EzRDP on your phone (same Wi-Fi) and tap this PC to pair it.",
        );
    }
    tray::run(app);
}
