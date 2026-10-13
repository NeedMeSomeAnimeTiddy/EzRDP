//! The other direction: a phone shares its screen and this PC controls it.
//! Opens a window showing the phone. Click = tap, drag = swipe, right-click = Back,
//! middle-click = Home, mouse wheel = scroll, typing goes into the phone's text box.

use crate::login::Login;
use crate::protocol::*;
use crate::state::App;
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

const WM_NEW_FRAME: u32 = WM_APP + 10;
const WM_END: u32 = WM_APP + 11;

const ID_BACK: usize = 1;
const ID_HOME: usize = 2;
const ID_RECENTS: usize = 3;
const ID_NOTIFICATIONS: usize = 4;

/// The latest decoded phone picture, ready to draw (BGRA, top-down).
#[derive(Default)]
struct Picture {
    w: usize,
    h: usize,
    bgra: Vec<u8>,
}

struct View {
    tx: Arc<Sender>,
    picture: Arc<Mutex<Picture>>,
    stream: TcpStream,
    pressed: bool,
    sized: bool,
    /// The window showing this phone, so a closing old window can't touch a newer phone.
    hwnd: isize,
}

/// Only one phone can be shown at a time; a new one replaces the old.
static VIEW: Mutex<Option<View>> = Mutex::new(None);

/// Runs `f` on the phone shown in this window, if it is still the current one.
fn with_view<T>(hwnd: HWND, f: impl FnOnce(&mut View) -> T) -> Option<T> {
    VIEW.lock().unwrap().as_mut().filter(|v| v.hwnd == hwnd.0 as isize).map(f)
}

pub fn run(stream: TcpStream, login: Login, _app: Arc<App>) {
    let mut decoder = match openh264::decoder::Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            crate::log!("phone view: decoder failed: {e}");
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };
    let tx = Arc::new(Sender::new(stream.try_clone().unwrap(), login.tx));
    let picture = Arc::new(Mutex::new(Picture::default()));

    // Hang up any phone already being shown.
    if let Some(old) = VIEW.lock().unwrap().take() {
        let _ = old.stream.shutdown(Shutdown::Both);
    }
    *VIEW.lock().unwrap() = Some(View {
        tx: tx.clone(),
        picture: picture.clone(),
        stream: stream.try_clone().unwrap(),
        pressed: false,
        sized: false,
        hwnd: 0,
    });

    let title = format!("{} — EzRDP", login.device_name);
    let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel();
    let window = std::thread::spawn(move || window_thread(&title, hwnd_tx));
    let Ok(hwnd) = hwnd_rx.recv() else {
        let _ = stream.shutdown(Shutdown::Both);
        let mut slot = VIEW.lock().unwrap();
        if slot.as_ref().is_some_and(|v| Arc::ptr_eq(&v.tx, &tx)) {
            *slot = None;
        }
        return;
    };
    if let Some(v) = VIEW.lock().unwrap().as_mut().filter(|v| Arc::ptr_eq(&v.tx, &tx)) {
        v.hwnd = hwnd;
    }
    // Draw "Waiting for the phone's screen…" now that the window knows its phone.
    unsafe {
        let _ = InvalidateRect(Some(HWND(hwnd as *mut _)), None, false);
    }
    let post = |msg| unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    };

    let mut reader = Receiver::new_video(std::io::BufReader::new(stream.try_clone().unwrap()), login.rx);
    let mut rgba = Vec::new();
    let mut waiting_for_key = false;
    while let Ok((kind, body)) = reader.read() {
        match kind {
            P_SCREEN_CONFIG => {
                // New size (phone rotated): start fresh.
                if let Ok(d) = openh264::decoder::Decoder::new() {
                    decoder = d;
                }
                if let Some(v) = VIEW.lock().unwrap().as_mut().filter(|v| Arc::ptr_eq(&v.tx, &tx)) {
                    v.sized = false;
                }
            }
            P_SCREEN_FRAME if body.len() > 5 => {
                let id = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                let key = body[4] != 0;
                let _ = tx.send(S_FRAME_ACK, &id.to_be_bytes());
                if waiting_for_key && !key {
                    continue;
                }
                waiting_for_key = false;
                match decoder.decode(&body[5..]) {
                    Ok(Some(yuv)) => {
                        use openh264::formats::YUVSource;
                        let (w, h) = yuv.dimensions();
                        rgba.resize(w * h * 4, 0);
                        yuv.write_rgba8(&mut rgba);
                        let mut p = picture.lock().unwrap();
                        p.w = w;
                        p.h = h;
                        p.bgra.resize(w * h * 4, 0);
                        for (d, s) in p.bgra.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
                            d[0] = s[2];
                            d[1] = s[1];
                            d[2] = s[0];
                            d[3] = 255;
                        }
                        drop(p);
                        post(WM_NEW_FRAME);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // Broken picture: skip until the next full one.
                        waiting_for_key = true;
                        let _ = tx.send(S_KEYFRAME, &[]);
                    }
                }
            }
            _ => {}
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
    post(WM_END);
    let _ = window.join();
    let mut slot = VIEW.lock().unwrap();
    if slot.as_ref().is_some_and(|v| Arc::ptr_eq(&v.tx, &tx)) {
        *slot = None;
    }
}

fn send(hwnd: HWND, kind: u8, payload: &[u8]) {
    let tx = with_view(hwnd, |v| v.tx.clone());
    if let Some(tx) = tx {
        let _ = tx.send(kind, payload);
    }
}

fn nav(hwnd: HWND, n: u8) {
    send(hwnd, S_NAV, &[n]);
}

/// Where the phone picture is drawn inside the window (keeps its shape, centred).
fn picture_rect(client: RECT, pw: usize, ph: usize) -> RECT {
    let (cw, ch) = ((client.right - client.left) as f32, (client.bottom - client.top) as f32);
    if pw == 0 || ph == 0 || cw <= 0.0 || ch <= 0.0 {
        return client;
    }
    let s = (cw / pw as f32).min(ch / ph as f32);
    let (w, h) = (pw as f32 * s, ph as f32 * s);
    let left = ((cw - w) / 2.0) as i32;
    let top = ((ch - h) / 2.0) as i32;
    RECT { left, top, right: left + w as i32, bottom: top + h as i32 }
}

/// Mouse position as 0..1 fractions of the phone screen.
fn to_phone(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let (pw, ph) = with_view(hwnd, |v| {
        let p = v.picture.lock().unwrap();
        (p.w, p.h)
    })
    .unwrap_or((0, 0));
    let r = picture_rect(client, pw, ph);
    let fx = (x - r.left as f32) / (r.right - r.left).max(1) as f32;
    let fy = (y - r.top as f32) / (r.bottom - r.top).max(1) as f32;
    (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0))
}

fn touch(hwnd: HWND, action: u8, (x, y): (f32, f32)) {
    let mut p = vec![action];
    p.extend_from_slice(&x.to_be_bytes());
    p.extend_from_slice(&y.to_be_bytes());
    send(hwnd, S_TOUCH, &p);
}

/// Makes the window the phone's shape the first time a picture arrives (or after rotating).
fn fit_window(hwnd: HWND) {
    let size = with_view(hwnd, |v| {
        let p = v.picture.lock().unwrap();
        if v.sized || p.w == 0 {
            return None;
        }
        let size = (p.w, p.h);
        drop(p);
        v.sized = true;
        Some(size)
    });
    let Some(Some(size)) = size else { return };
    unsafe {
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let (ww, wh) = ((work.right - work.left) as f32 * 0.9, (work.bottom - work.top) as f32 * 0.85);
        let s = (ww / size.0 as f32).min(wh / size.1 as f32);
        let mut r = RECT { left: 0, top: 0, right: (size.0 as f32 * s) as i32, bottom: (size.1 as f32 * s) as i32 };
        let _ = AdjustWindowRectEx(&mut r, WS_OVERLAPPEDWINDOW, true, WINDOW_EX_STYLE(0));
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = work.top + ((work.bottom - work.top) - h) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let picture = with_view(hwnd, |v| v.picture.clone());
        let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        match picture {
            Some(pic) => {
                let p = pic.lock().unwrap();
                if p.w == 0 {
                    FillRect(dc, &client, black);
                    let mut r = client;
                    SetTextColor(dc, COLORREF(0xFFFFFF));
                    SetBkMode(dc, TRANSPARENT);
                    let mut text: Vec<u16> = "Waiting for the phone's screen…".encode_utf16().collect();
                    DrawTextW(dc, &mut text, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                } else {
                    let r = picture_rect(client, p.w, p.h);
                    // Black bars around the picture.
                    for bar in [
                        RECT { right: r.left, ..client },
                        RECT { left: r.right, ..client },
                        RECT { left: r.left, right: r.right, bottom: r.top, ..client },
                        RECT { left: r.left, right: r.right, top: r.bottom, ..client },
                    ] {
                        FillRect(dc, &bar, black);
                    }
                    let info = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: p.w as i32,
                            biHeight: -(p.h as i32),
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    SetStretchBltMode(dc, HALFTONE);
                    StretchDIBits(
                        dc,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        0,
                        0,
                        p.w as i32,
                        p.h as i32,
                        Some(p.bgra.as_ptr() as *const _),
                        &info,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                }
            }
            None => {
                FillRect(dc, &client, black);
            }
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NEW_FRAME => {
            fit_window(hwnd);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            unsafe {
                SetCapture(hwnd);
            }
            with_view(hwnd, |v| v.pressed = true);
            touch(hwnd, 0, to_phone(hwnd, lparam));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if with_view(hwnd, |v| v.pressed) == Some(true) {
                touch(hwnd, 1, to_phone(hwnd, lparam));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let was = with_view(hwnd, |v| std::mem::replace(&mut v.pressed, false));
            if was == Some(true) {
                touch(hwnd, 2, to_phone(hwnd, lparam));
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            nav(hwnd, NAV_BACK);
            LRESULT(0)
        }
        WM_MBUTTONUP => {
            nav(hwnd, NAV_HOME);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Wheel position comes in screen coordinates.
            let mut pt = POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let (x, y) = to_phone(hwnd, LPARAM(((pt.y as u16 as isize) << 16) | pt.x as u16 as isize));
            let notches = ((wparam.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
            // One notch moves about a sixth of the screen; wheel up = see what's above.
            let mut p = Vec::new();
            for v in [x, y, 0.0, notches * 0.16] {
                p.extend_from_slice(&f32::to_be_bytes(v));
            }
            send(hwnd, S_SWIPE, &p);
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control()) {
                send(hwnd, S_TYPE, c.to_string().as_bytes());
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match VIRTUAL_KEY(wparam.0 as u16) {
                VK_BACK => nav(hwnd, NAV_BACKSPACE),
                VK_RETURN => nav(hwnd, NAV_ENTER),
                VK_ESCAPE => nav(hwnd, NAV_BACK),
                VK_HOME => nav(hwnd, NAV_HOME),
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match wparam.0 & 0xffff {
                ID_BACK => nav(hwnd, NAV_BACK),
                ID_HOME => nav(hwnd, NAV_HOME),
                ID_RECENTS => nav(hwnd, NAV_RECENTS),
                ID_NOTIFICATIONS => nav(hwnd, NAV_NOTIFICATIONS),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // Closing the window ends the phone's sharing.
            with_view(hwnd, |v| {
                let _ = v.stream.shutdown(Shutdown::Both);
            });
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_END => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn window_thread(title: &str, hwnd_tx: std::sync::mpsc::Sender<isize>) {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("EzRDPPhone"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: crate::tray::app_icon(),
            ..Default::default()
        };
        RegisterClassW(&class);

        let menu = CreateMenu().ok();
        if let Some(m) = menu {
            let _ = AppendMenuW(m, MF_STRING, ID_BACK, w!("Back"));
            let _ = AppendMenuW(m, MF_STRING, ID_HOME, w!("Home"));
            let _ = AppendMenuW(m, MF_STRING, ID_RECENTS, w!("Recent apps"));
            let _ = AppendMenuW(m, MF_STRING, ID_NOTIFICATIONS, w!("Notifications"));
        }
        let Ok(window) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EzRDPPhone"),
            &HSTRING::from(title),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            420,
            860,
            None,
            menu,
            Some(instance),
            None,
        ) else {
            crate::log!("phone view: couldn't open window");
            return;
        };
        let _ = hwnd_tx.send(window.0 as isize);
        let _ = SetForegroundWindow(window);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_keeps_shape() {
        let client = RECT { left: 0, top: 0, right: 400, bottom: 400 };
        let r = picture_rect(client, 100, 200);
        assert_eq!((r.left, r.top, r.right, r.bottom), (100, 0, 300, 400));
    }
}
