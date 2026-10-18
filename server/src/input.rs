//! Mouse and keyboard injection with SendInput.

use crate::capture::Monitor;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const MODIFIERS: [u16; 8] = [0x10, 0x11, 0x12, 0x5B, 0xA0, 0xA2, 0xA4, 0x5C]; // shift ctrl alt win, left variants

pub struct Input {
    /// Leftover sub-pixel movement from relative moves.
    frac: (f64, f64),
    held_keys: Vec<u16>,
    held_buttons: [bool; 3],
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 // page up/down, end, home, arrows
        | 0x2D | 0x2E // insert, delete
        | 0x5B | 0x5C | 0x5D // win keys, menu
        | 0xA3 | 0xA5 // right ctrl/alt
        | 0x6F // numpad divide
        | 0x90 // num lock
    )
}

fn key_event(vk: u16, down: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    key(vk, scan, flags)
}

impl Input {
    pub fn new() -> Self {
        Self { frac: (0.0, 0.0), held_keys: Vec::new(), held_buttons: [false; 3] }
    }

    fn move_to(&self, x: i32, y: i32) {
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2);
            let nx = ((x - vx) as i64 * 65535 / (vw - 1) as i64) as i32;
            let ny = ((y - vy) as i64 * 65535 / (vh - 1) as i64) as i32;
            send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0)]);
        }
    }

    /// Moves to a point given as 0..1 fractions of the monitor.
    pub fn move_abs(&mut self, m: &Monitor, fx: f32, fy: f32) {
        let x = m.x + (fx.clamp(0.0, 1.0) * (m.w - 1) as f32).round() as i32;
        let y = m.y + (fy.clamp(0.0, 1.0) * (m.h - 1) as f32).round() as i32;
        self.frac = (0.0, 0.0);
        self.move_to(x, y);
    }

    /// Moves relative to the current position; deltas are fractions of the monitor size.
    /// The cursor is kept inside the monitor being viewed.
    pub fn move_rel(&mut self, m: &Monitor, fx: f32, fy: f32) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        let dx = fx as f64 * m.w as f64 + self.frac.0;
        let dy = fy as f64 * m.h as f64 + self.frac.1;
        let (ix, iy) = (dx.trunc(), dy.trunc());
        self.frac = (dx - ix, dy - iy);
        if ix == 0.0 && iy == 0.0 {
            return;
        }
        let x = (p.x + ix as i32).clamp(m.x, m.x + m.w - 1);
        let y = (p.y + iy as i32).clamp(m.y, m.y + m.h - 1);
        self.move_to(x, y);
    }

    /// If the mouse is on a different screen than the one being viewed, brings it to the middle
    /// of the viewed one, so clicks and typing go where the phone is looking.
    pub fn ensure_on(&mut self, m: &Monitor) {
        bring_onto(m);
    }

    pub fn button(&mut self, b: u8, down: bool) {
        let flags = match (b, down) {
            (0, true) => MOUSEEVENTF_LEFTDOWN,
            (0, false) => MOUSEEVENTF_LEFTUP,
            (1, true) => MOUSEEVENTF_RIGHTDOWN,
            (1, false) => MOUSEEVENTF_RIGHTUP,
            (2, true) => MOUSEEVENTF_MIDDLEDOWN,
            (2, false) => MOUSEEVENTF_MIDDLEUP,
            _ => return,
        };
        self.held_buttons[b as usize] = down;
        send(&[mouse(flags, 0, 0, 0)]);
    }

    pub fn scroll(&mut self, dy: i16, dx: i16) {
        if dy != 0 {
            send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, dy as i32)]);
        }
        if dx != 0 {
            send(&[mouse(MOUSEEVENTF_HWHEEL, 0, 0, dx as i32)]);
        }
    }

    pub fn key(&mut self, vk: u16, down: bool) {
        if down {
            if !self.held_keys.contains(&vk) {
                self.held_keys.push(vk);
            }
        } else {
            self.held_keys.retain(|&k| k != vk);
        }
        send(&[key_event(vk, down)]);
    }

    pub fn text(&mut self, s: &str) {
        // With Ctrl/Alt/Win held, send real key presses so shortcuts like Ctrl+C work.
        let shortcut = self.held_keys.iter().any(|k| MODIFIERS.contains(k) && !matches!(k, 0x10 | 0xA0));
        let mut inputs = Vec::new();
        for c in s.chars() {
            let vk = match c {
                '\n' | '\r' => Some(0x0D),
                '\t' => Some(0x09),
                '\u{8}' => Some(0x08),
                'a'..='z' if shortcut => Some(c.to_ascii_uppercase() as u16),
                'A'..='Z' | '0'..='9' if shortcut => Some(c as u16),
                _ => None,
            };
            if let Some(vk) = vk {
                inputs.push(key_event(vk, true));
                inputs.push(key_event(vk, false));
                continue;
            }
            let mut buf = [0u16; 2];
            for &unit in c.encode_utf16(&mut buf).iter() {
                inputs.push(key(0, unit, KEYEVENTF_UNICODE));
                inputs.push(key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
        if !inputs.is_empty() {
            send(&inputs);
        }
    }

    /// Lets go of everything still held (called when the phone disconnects).
    pub fn release_all(&mut self) {
        for vk in std::mem::take(&mut self.held_keys) {
            send(&[key_event(vk, false)]);
        }
        for b in 0..3 {
            if self.held_buttons[b] {
                self.button(b as u8, false);
            }
        }
    }
}

/// Moves the mouse to the middle of `m` if it's on another monitor.
pub fn bring_onto(m: &Monitor) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    if p.x < m.x || p.x >= m.x + m.w || p.y < m.y || p.y >= m.y + m.h {
        Input::new().move_to(m.x + m.w / 2, m.y + m.h / 2);
    }
}

/// Gets the mouse back after a program (a game, a capture script) quit without letting go of it:
/// lifts any "keep the pointer inside this box" lock, unblocks input, lets go of stuck buttons
/// and brings back the normal pointer shapes.
pub fn free_mouse() {
    unsafe {
        let _ = ClipCursor(None);
        let _ = BlockInput(false);
        let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    send(&[
        mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0),
        mouse(MOUSEEVENTF_RIGHTUP, 0, 0, 0),
        mouse(MOUSEEVENTF_MIDDLEUP, 0, 0, 0),
    ]);
}

/// Cursor position in screen pixels, or `None` if it's hidden.
pub fn cursor() -> Option<(i32, i32)> {
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    unsafe {
        GetCursorInfo(&mut ci).ok()?;
    }
    if ci.flags.0 & CURSOR_SHOWING.0 == 0 {
        return None;
    }
    Some((ci.ptScreenPos.x, ci.ptScreenPos.y))
}

/// Whether this copy of EzRDP is running as administrator.
pub fn elevated() -> bool {
    use std::sync::OnceLock;
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe { process_elevated(windows::Win32::System::Threading::GetCurrentProcess()) == Some(true) })
}

/// `Some(true)` if the process runs as administrator, `None` if we aren't allowed to look.
unsafe fn process_elevated(process: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = Default::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok.then_some(e.TokenIsElevated != 0)
    }
}

/// The window in front belongs to a program running as administrator, and this copy isn't.
/// Windows then silently throws away every click and key EzRDP sends.
pub fn admin_window_in_front() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::*;
    if elevated() {
        return false;
    }
    unsafe {
        let win = GetForegroundWindow();
        if win.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(win, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return false;
        }
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        // Not allowed to look at its token: it runs with more rights than us.
        let admin = process_elevated(p).unwrap_or(true);
        let _ = CloseHandle(p);
        admin
    }
}

/// A tiny mouse nudge wakes monitors that went to sleep, so the phone isn't stuck on a black screen.
pub fn wake_displays() {
    send(&[mouse(MOUSEEVENTF_MOVE, 0, 0, 0)]);
}
