//! The icon next to the clock, with a right-click menu (Pause/Resume, trusted phones, ...).

use crate::state::App;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
const WM_REFRESH: u32 = WM_APP + 2;
const WM_BALLOON: u32 = WM_APP + 3;

const ID_PAUSE: usize = 2;
const ID_AUTOSTART: usize = 3;
const ID_INFO: usize = 4;
const ID_QUIT: usize = 5;
const ID_ADD_PHONE: usize = 6;
const ID_FIREWALL: usize = 7;
const ID_REMOVE_PHONE: usize = 100;

static APP: OnceLock<Arc<App>> = OnceLock::new();
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static BALLOON: Mutex<Option<(String, String)>> = Mutex::new(None);
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
static ICONS: OnceLock<(isize, isize)> = OnceLock::new();

/// Extra info for the "Show info" box, filled in by main (firewall state etc.).
pub static EXTRA_INFO: Mutex<String> = Mutex::new(String::new());
/// Runs when the user asks to fix the firewall from the menu.
pub static FIX_FIREWALL: OnceLock<fn()> = OnceLock::new();
/// False while Windows Firewall isn't set up; the menu then offers to fix it.
pub static FIREWALL_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn hwnd() -> HWND {
    HWND(WINDOW.load(Relaxed) as *mut _)
}

/// Thread-safe: asks the tray to redraw its tooltip/icon.
pub fn refresh() {
    unsafe {
        let _ = PostMessageW(Some(hwnd()), WM_REFRESH, WPARAM(0), LPARAM(0));
    }
}

/// Thread-safe: shows a small notification from the tray icon.
pub fn notify(title: &str, text: &str) {
    *BALLOON.lock().unwrap() = Some((title.to_string(), text.to_string()));
    unsafe {
        let _ = PostMessageW(Some(hwnd()), WM_BALLOON, WPARAM(0), LPARAM(0));
    }
}

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, c) in out.iter_mut().zip(s.encode_utf16().take(N - 1)) {
        *o = c;
    }
    out
}

/// Draws the 32x32 icon: a rounded square with a little monitor in it.
fn make_icon(color: u32) -> HICON {
    const S: i32 = 32;
    let mut px = vec![0u32; (S * S) as usize];
    for y in 0..S {
        for x in 0..S {
            // Rounded square background.
            let (cx, cy) = ((x.min(S - 1 - x)) as f32, (y.min(S - 1 - y)) as f32);
            let r = 7.0f32;
            let inside = cx >= r || cy >= r || (r - cx).hypot(r - cy) <= r;
            if !inside {
                continue;
            }
            let mut c = color;
            // Monitor frame (x 6..26, y 7..21), stand and base.
            let frame = (6..26).contains(&x) && (7..21).contains(&y) && !((8..24).contains(&x) && (9..19).contains(&y));
            let stand = (14..18).contains(&x) && (21..24).contains(&y);
            let base = (10..22).contains(&x) && (24..26).contains(&y);
            if frame || stand || base {
                c = 0xFFFFFFFF;
            }
            px[(y * S + x) as usize] = c;
        }
    }
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color_bm = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        }
        let mask = vec![0u8; (S * S / 8) as usize];
        let mask_bm = CreateBitmap(S, S, 1, 1, Some(mask.as_ptr() as *const _));
        let ii = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask_bm, hbmColor: color_bm };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(color_bm.into());
        let _ = DeleteObject(mask_bm.into());
        icon
    }
}

fn status_text(app: &App) -> String {
    if app.paused() {
        "EzRDP: paused".into()
    } else if let Some(name) = app.connected.lock().unwrap().as_ref() {
        format!("EzRDP: {name} is connected")
    } else {
        "EzRDP: ready".into()
    }
}

fn icon_data(app: &App) -> NOTIFYICONDATAW {
    let (on, off) = *ICONS.get().unwrap();
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd(),
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: HICON((if app.paused() { off } else { on }) as *mut _),
        szTip: wide(&status_text(app)),
        ..Default::default()
    }
}

fn add_icon(app: &App) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &icon_data(app));
    }
}

fn update_icon(app: &App) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &icon_data(app));
    }
}

fn message(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("EzRDP"), flags | MB_SETFOREGROUND | MB_TOPMOST) }
}

/// Asks the person at the PC whether to trust a new phone. Runs on a network thread.
pub fn approve(name: &str, code: u32) -> bool {
    let text = format!(
        "A phone wants to control this PC:\n\n    {name}\n\nCheck the phone shows this code:\n\n    {}\n\n\
         Only click Yes if it's your phone and the codes match.",
        crate::pairing::format_code(code)
    );
    message(&text, MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) == IDYES
}

fn show_menu(app: &App) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let _ = AppendMenuW(menu, flags, id, &HSTRING::from(text));
        };
        add(MF_STRING | MF_GRAYED, 0, &status_text(app));
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, ID_PAUSE, if app.paused() { "Resume connections" } else { "Pause connections" });

        let phones = CreatePopupMenu().unwrap_or_default();
        let devices = app.trusted.lock().unwrap().devices.clone();
        if devices.is_empty() {
            let _ = AppendMenuW(phones, MF_STRING | MF_GRAYED, 0, w!("No phones yet"));
        }
        for (i, d) in devices.iter().enumerate() {
            let _ = AppendMenuW(phones, MF_STRING, ID_REMOVE_PHONE + i, &HSTRING::from(format!("Remove {}", d.name)));
        }
        let _ = AppendMenuW(phones, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(phones, MF_STRING, ID_ADD_PHONE, w!("How to add a phone"));
        let _ = AppendMenuW(menu, MF_POPUP, phones.0 as usize, w!("Trusted phones"));

        let auto = if crate::autostart::enabled() { MF_CHECKED } else { MF_UNCHECKED };
        add(MF_STRING | auto, ID_AUTOSTART, "Start with Windows");
        add(MF_STRING, ID_INFO, "Connection info");
        if !FIREWALL_OK.load(Relaxed) {
            add(MF_STRING, ID_FIREWALL, "Allow through Windows Firewall...");
        }
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, ID_QUIT, "Quit EzRDP");

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd());
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd(), None);
        let _ = PostMessageW(Some(hwnd()), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        handle(app, cmd.0 as usize, &devices);
    }
}

fn handle(app: &App, cmd: usize, devices: &[crate::trust::Device]) {
    match cmd {
        ID_PAUSE => {
            let now = !app.paused();
            app.paused.store(now, Relaxed);
            if now {
                crate::server::hang_up();
            }
            app.changed();
        }
        ID_AUTOSTART => crate::autostart::set(!crate::autostart::enabled()),
        ID_INFO => {
            let home = crate::server::lan_ip().map(|i| i.to_string()).unwrap_or("unknown".into());
            let net = app.public_ip.lock().unwrap().map(|i| i.to_string()).unwrap_or("unknown".into());
            let text = format!(
                "PC name: {}\nHome address: {home}\nInternet address: {net}\nPort: {}\n{}\nTrusted phones: {}\n\n\
                 Phones don't need any of this: they find the PC by themselves.",
                app.pc_name,
                app.port,
                EXTRA_INFO.lock().unwrap(),
                devices.len()
            );
            message(&text, MB_OK | MB_ICONINFORMATION);
        }
        ID_ADD_PHONE => {
            message(
                "To add a phone:\n\n1. Connect the phone to the same Wi-Fi as this PC.\n2. Open EzRDP on the phone.\n\
                 3. Tap this PC in the list.\n4. Click Yes here when the codes match.\n\n\
                 After that the phone can connect from anywhere.",
                MB_OK | MB_ICONINFORMATION,
            );
        }
        ID_FIREWALL => {
            if let Some(f) = FIX_FIREWALL.get() {
                f();
            }
        }
        ID_QUIT => unsafe {
            let data = icon_data(app);
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            PostQuitMessage(0);
        },
        c if c >= ID_REMOVE_PHONE => {
            if let Some(d) = devices.get(c - ID_REMOVE_PHONE) {
                let q = format!("Stop trusting \"{}\"? It will need to be paired again to connect.", d.name);
                if message(&q, MB_YESNO | MB_ICONWARNING) == IDYES {
                    app.trusted.lock().unwrap().remove(&d.id);
                    if app.connected.lock().unwrap().as_deref() == Some(d.name.as_str()) {
                        crate::server::hang_up();
                    }
                }
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(app) = APP.get() else { return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) } };
    match msg {
        WM_TRAY => {
            let event = (lparam.0 & 0xffff) as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_CONTEXTMENU {
                show_menu(app);
            }
            LRESULT(0)
        }
        WM_REFRESH => {
            update_icon(app);
            LRESULT(0)
        }
        WM_BALLOON => {
            if let Some((title, text)) = BALLOON.lock().unwrap().take() {
                let mut data = icon_data(app);
                data.uFlags = NIF_INFO;
                data.szInfoTitle = wide(&title);
                data.szInfo = wide(&text);
                data.dwInfoFlags = NIIF_INFO;
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
                }
            }
            LRESULT(0)
        }
        m if Some(&m) == TASKBAR_CREATED.get() => {
            add_icon(app); // Explorer restarted: put the icon back.
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Runs the tray until Quit. Must be called on the main thread.
pub fn run(app: Arc<App>) {
    let _ = APP.set(app.clone());
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("EzRDPTray"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let Ok(window) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EzRDPTray"),
            w!("EzRDP"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        ) else {
            return;
        };
        WINDOW.store(window.0 as isize, Relaxed);
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        let _ = ICONS.set((make_icon(0xFF4F8CFF).0 as isize, make_icon(0xFF8A8F98).0 as isize));
        add_icon(&app);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

