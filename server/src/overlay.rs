//! While a phone controls this PC: a coloured frame around the screen being shown, and a
//! banner at the top saying who's in control (click it to disconnect). Both are hidden from
//! the screen capture where Windows allows it, so the phone doesn't see them.

use crate::capture::Monitor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

const WM_PLACE: u32 = WM_APP + 20;
const COLOR: u32 = 0x00FF8C4F; // EzRDP blue, as 0x00BBGGRR
const EDGE: i32 = 4;

/// Banner text and the screen to frame.
static STATE: Mutex<Option<(String, Monitor)>> = Mutex::new(None);
/// The overlay thread's banner window (0 = not running).
static BANNER: AtomicIsize = AtomicIsize::new(0);
/// Called when the banner is clicked.
static ON_CLICK: Mutex<Option<fn()>> = Mutex::new(None);

/// Shows the frame and banner (or moves them to another screen). Thread-safe.
pub fn show(phone: &str, m: &Monitor, on_click: fn()) {
    *STATE.lock().unwrap() = Some((phone.to_string(), m.clone()));
    *ON_CLICK.lock().unwrap() = Some(on_click);
    // -1 = the window is being made right now; it will read the new state when it's ready.
    match BANNER.compare_exchange(0, -1, Relaxed, Relaxed) {
        Ok(_) => {
            std::thread::spawn(run);
        }
        Err(-1) => {}
        Err(banner) => unsafe {
            let _ = PostMessageW(Some(HWND(banner as *mut _)), WM_PLACE, WPARAM(0), LPARAM(0));
        },
    }
}

/// Whether a screen point is on the banner (while it's showing).
pub fn on_banner(x: i32, y: i32) -> bool {
    let banner = BANNER.load(Relaxed);
    if banner <= 0 {
        return false;
    }
    let mut r = RECT::default();
    unsafe { GetWindowRect(HWND(banner as *mut _), &mut r) }.is_ok()
        && x >= r.left
        && x < r.right
        && y >= r.top
        && y < r.bottom
}

/// The banner was clicked with the PC's own mouse.
pub fn clicked() {
    let f = *ON_CLICK.lock().unwrap();
    if let Some(f) = f {
        f();
    }
}

/// Removes the frame and banner. Thread-safe.
pub fn hide() {
    *STATE.lock().unwrap() = None;
    let banner = BANNER.swap(0, Relaxed);
    if banner > 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(banner as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn text() -> String {
    STATE
        .lock()
        .unwrap()
        .as_ref()
        .map(|(n, _)| format!("{n} is controlling this PC  ·  click here or press Ctrl+Esc to disconnect"))
        .unwrap_or_default()
}

unsafe extern "system" fn banner_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                let bg = CreateSolidBrush(COLORREF(COLOR));
                FillRect(dc, &r, bg);
                let _ = DeleteObject(bg.into());
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                let font = CreateFontW(
                    -(15 * dpi / 96), 0, 0, 0, FW_SEMIBOLD.0 as i32, 0, 0, 0,
                    DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                    DEFAULT_PITCH.0 as u32, w!("Segoe UI"),
                );
                let old = SelectObject(dc, font.into());
                SetTextColor(dc, COLORREF(0x00FFFFFF));
                SetBkMode(dc, TRANSPARENT);
                let mut t: Vec<u16> = text().encode_utf16().collect();
                DrawTextW(dc, &mut t, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(dc, old);
                let _ = DeleteObject(font.into());
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_PLACE => {
            place(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn edge_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                let bg = CreateSolidBrush(COLORREF(COLOR));
                FillRect(dc, &r, bg);
                let _ = DeleteObject(bg.into());
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

static EDGES: Mutex<[isize; 4]> = Mutex::new([0; 4]);

/// Puts the banner at the top middle of the viewed screen and the frame around it.
fn place(banner: HWND) {
    let Some((_, m)) = STATE.lock().unwrap().clone() else { return };
    unsafe {
        let dpi = GetDpiForWindow(banner).max(96) as i32;
        let edge = EDGE * dpi / 96;
        // Size the banner to its text.
        let dc = GetDC(Some(banner));
        let font = CreateFontW(
            -(15 * dpi / 96), 0, 0, 0, FW_SEMIBOLD.0 as i32, 0, 0, 0,
            DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32, w!("Segoe UI"),
        );
        let old = SelectObject(dc, font.into());
        let mut t: Vec<u16> = text().encode_utf16().collect();
        let mut tr = RECT::default();
        DrawTextW(dc, &mut t, &mut tr, DT_CALCRECT | DT_SINGLELINE);
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        ReleaseDC(Some(banner), dc);
        let bw = (tr.right - tr.left + 36 * dpi / 96).min(m.w - 2 * edge);
        let bh = 30 * dpi / 96;
        let bx = m.x + (m.w - bw) / 2;
        let by = m.y + edge;
        let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
        let _ = SetWindowPos(banner, Some(HWND_TOPMOST), bx, by, bw, bh, flags);
        let rgn = CreateRoundRectRgn(0, 0, bw + 1, bh + 1, bh, bh);
        SetWindowRgn(banner, Some(rgn), true);
        let _ = InvalidateRect(Some(banner), None, true);

        let rects = [
            (m.x, m.y, m.w, edge),
            (m.x, m.y + m.h - edge, m.w, edge),
            (m.x, m.y, edge, m.h),
            (m.x + m.w - edge, m.y, edge, m.h),
        ];
        let edges = *EDGES.lock().unwrap();
        for (h, (x, y, w, hgt)) in edges.iter().zip(rects) {
            if *h != 0 {
                let _ = SetWindowPos(HWND(*h as *mut _), Some(HWND_TOPMOST), x, y, w, hgt, flags);
            }
        }
    }
}

fn run() {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        for (name, proc_) in [
            (w!("EzRDPBanner"), banner_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
            (w!("EzRDPEdge"), edge_proc),
        ] {
            let class = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: instance, lpszClassName: name, ..Default::default() };
            RegisterClassW(&class);
        }
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED;
        let make = |class, extra: WINDOW_EX_STYLE| {
            CreateWindowExW(ex | extra, class, &HSTRING::from("EzRDP"), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance), None).ok()
        };
        // Clicks go through the banner: the phone can't see it (it's hidden from capture), so its
        // taps there are meant for what's underneath. Clicks from the PC's own mouse are picked
        // out by the tray's mouse watcher instead (see `on_banner`).
        let Some(banner) = make(w!("EzRDPBanner"), WS_EX_TRANSPARENT) else { return };
        let _ = SetLayeredWindowAttributes(banner, COLORREF(0), 235, LWA_ALPHA);
        // Hide from screen capture (Windows 10 2004 and newer), so the phone sees the real screen.
        let _ = SetWindowDisplayAffinity(banner, WDA_EXCLUDEFROMCAPTURE);
        let mut edges = [0isize; 4];
        for e in edges.iter_mut() {
            // The frame lets clicks through to whatever is underneath.
            if let Some(h) = make(w!("EzRDPEdge"), WS_EX_TRANSPARENT) {
                let _ = SetLayeredWindowAttributes(h, COLORREF(0), 200, LWA_ALPHA);
                let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
                *e = h.0 as isize;
            }
        }
        *EDGES.lock().unwrap() = edges;
        let ready = BANNER.compare_exchange(-1, banner.0 as isize, Relaxed, Relaxed).is_ok();
        if ready && STATE.lock().unwrap().is_some() {
            place(banner);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        for h in edges {
            if h != 0 {
                let _ = DestroyWindow(HWND(h as *mut _));
            }
        }
        *EDGES.lock().unwrap() = [0; 4];
        let _ = DestroyWindow(banner);
    }
}
