//! Mouse and keyboard injection with SendInput.

use crate::capture::Monitor;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const MODIFIERS: [u16; 8] = [0x10, 0x11, 0x12, 0x5B, 0xA0, 0xA2, 0xA4, 0x5C]; // shift ctrl alt win, left variants

pub struct Input {
    /// Leftover sub-pixel movement from relative moves.
    frac: (f64, f64),
    /// Where the last relative move sent the pointer, and where it was read before that move.
    /// Windows can still report the old spot for a moment after a move, which would lose movement.
    last_move: Option<(POINT, POINT)>,
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

/// Turns a pixel (0..size-1) into SendInput's 0..65535 scale, aimed at the middle of the pixel.
/// Aiming at the pixel's left/top edge with rounding down could land one pixel short, and since
/// relative moves start from where the pointer is, those lost pixels added up into drift.
fn to_normalized(px: i32, size: i32) -> i32 {
    ((((px as i64) * 2 + 1) * 65536 / (2 * size as i64)) as i32).clamp(0, 65535)
}

/// Moves the pointer to a screen pixel.
fn move_to(x: i32, y: i32) {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2);
        let nx = to_normalized(x - vx, vw);
        let ny = to_normalized(y - vy, vh);
        send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0)]);
    }
}

/// Where the last move should have put the pointer, and when it was sent.
static LANDING: Mutex<Option<(POINT, Instant)>> = Mutex::new(None);

/// Once moves stop, checks the pointer really ended up where it was sent, and puts it there if
/// it's a pixel or two off (Windows' own rounding). A bigger difference means someone moved the
/// real mouse or a program is holding it, so it's left alone.
fn check_landing(target: POINT) {
    static WATCHER: Once = Once::new();
    *LANDING.lock().unwrap() = Some((target, Instant::now()));
    WATCHER.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_millis(30));
                let mut landing = LANDING.lock().unwrap();
                let Some((target, at)) = *landing else { continue };
                if at.elapsed() < Duration::from_millis(60) {
                    continue;
                }
                *landing = None;
                let mut p = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut p);
                }
                let (dx, dy) = ((p.x - target.x).abs(), (p.y - target.y).abs());
                if (dx, dy) != (0, 0) && dx <= 2 && dy <= 2 {
                    unsafe {
                        let _ = SetCursorPos(target.x, target.y);
                    }
                }
            }
        });
    });
}

/// Turns a 0..1 fraction of `size` pixels into a pixel. The exact opposite of how the pointer's
/// position is reported back (pixel / size), so the other side draws it where it was sent.
fn to_pixel(f: f32, size: i32) -> i32 {
    ((f.clamp(0.0, 1.0) as f64 * size as f64 + 1e-3).floor() as i32).min(size - 1)
}

impl Input {
    pub fn new() -> Self {
        Self { frac: (0.0, 0.0), last_move: None, held_keys: Vec::new(), held_buttons: [false; 3] }
    }

    fn move_to(&self, x: i32, y: i32) {
        move_to(x, y);
        check_landing(POINT { x, y });
    }

    /// Moves to a point given as 0..1 fractions of the monitor.
    pub fn move_abs(&mut self, m: &Monitor, fx: f32, fy: f32) {
        let x = m.x + to_pixel(fx, m.w);
        let y = m.y + to_pixel(fy, m.h);
        self.frac = (0.0, 0.0);
        self.last_move = None;
        self.move_to(x, y);
    }

    /// Moves relative to the current position; deltas are fractions of the monitor size.
    /// The cursor is kept inside the monitor being viewed.
    pub fn move_rel(&mut self, m: &Monitor, fx: f32, fy: f32) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        if let Some((target, before)) = self.last_move
            && (p == before || p == target)
        {
            p = target; // the last move hasn't shown up yet, or landed exactly
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
        self.last_move = Some((POINT { x, y }, p));
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
        move_to(m.x + m.w / 2, m.y + m.h / 2);
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

/// Whether `inner` lies inside `outer`, give or take `slack` pixels (window borders and shadows).
fn rect_within(inner: &RECT, outer: &RECT, slack: i32) -> bool {
    inner.left >= outer.left - slack
        && inner.top >= outer.top - slack
        && inner.right <= outer.right + slack
        && inner.bottom <= outer.bottom + slack
}

/// A "keep the pointer inside this box" lock (`ClipCursor`) is in force, and nothing in front can
/// be the one holding it. A game grabbing the mouse is always the window in front, and its box is
/// inside that window. Windows does not lift the lock when the program that set it is killed, which
/// is how capture scripts leave the pointer stuck: the game is gone, and what is in front is the
/// desktop, the taskbar, or some other window the box doesn't fit in.
fn clip_is_orphaned() -> bool {
    unsafe {
        let mut clip = RECT::default();
        if GetClipCursor(&mut clip).is_err() {
            return false;
        }
        let virt = RECT {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            right: GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        };
        if rect_within(&virt, &clip, 0) {
            return false; // the box is the whole desktop: no lock
        }
        let front = GetForegroundWindow();
        if front.is_invalid() || front == GetShellWindow() || front == GetDesktopWindow() {
            return true;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(front, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return true;
        }
        let mut window = RECT::default();
        if GetWindowRect(front, &mut window).is_err() {
            return false;
        }
        !rect_within(&clip, &window, 32)
    }
}

/// Starts a background check that lifts a pointer lock whose program is gone, so a game that was
/// killed doesn't leave the pointer stuck and a remote viewer without a way to unstick it.
/// It has to see the lock orphaned twice in a row, so a game switching windows isn't disturbed.
pub fn watch_for_orphaned_clip() {
    std::thread::spawn(|| {
        let mut seen = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if clip_is_orphaned() {
                seen += 1;
                if seen >= 2 {
                    unsafe {
                        let _ = ClipCursor(None);
                    }
                    crate::log!("lifted a pointer lock left behind by a program that quit");
                    seen = 0;
                }
            } else {
                seen = 0;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn normalized_coordinates_map_back_to_the_same_pixel() {
        for size in [800, 1366, 1919, 1920, 2560, 3840, 5120, 7680] {
            for px in 0..size {
                let n = to_normalized(px, size) as i64;
                assert_eq!((n * size as i64 / 65536) as i32, px, "size {size} px {px}");
            }
        }
    }

    #[test]
    fn fractions_turn_into_the_pixel_they_were_reported_as() {
        for size in [1366, 1920, 2560] {
            for px in 0..size {
                assert_eq!(to_pixel(px as f32 / size as f32, size), px, "size {size} px {px}");
            }
            assert_eq!(to_pixel(1.0, size), size - 1);
            assert_eq!(to_pixel(0.0, size), 0);
        }
    }

    #[test]
    fn a_box_inside_the_window_in_front_is_not_orphaned() {
        let window = rect(100, 100, 1380, 820);
        assert!(rect_within(&rect(108, 131, 1372, 812), &window, 32));
    }

    #[test]
    fn a_box_the_window_doesnt_hold_is_orphaned() {
        let window = rect(0, 0, 800, 600);
        assert!(!rect_within(&rect(1000, 500, 2000, 1100), &window, 32));
    }

    #[test]
    fn the_whole_desktop_counts_as_no_lock() {
        let desktop = rect(0, 0, 4096, 1152);
        assert!(rect_within(&desktop, &rect(0, 0, 4096, 1152), 0));
        assert!(!rect_within(&desktop, &rect(0, 0, 4000, 1100), 0));
    }
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

/// Windows is showing one of its protected screens that no program may see or control: the
/// lock screen, an "allow changes?" (UAC) prompt, or a Windows Security / Windows Hello prompt.
pub fn security_prompt() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::StationsAndDesktops::*;
    use windows::Win32::System::Threading::*;
    unsafe {
        // The lock screen and UAC prompts switch to a separate desktop ("Winlogon"), which
        // normal programs can't even open.
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else {
            return true;
        };
        let mut name = [0u16; 64];
        let mut len = 0u32;
        let got = GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(desk.0),
            UOI_NAME,
            Some(name.as_mut_ptr() as *mut _),
            (name.len() * 2) as u32,
            Some(&mut len),
        )
        .is_ok();
        let _ = CloseDesktop(desk);
        if got && !String::from_utf16_lossy(&name).trim_end_matches('\0').eq_ignore_ascii_case("Default") {
            return true;
        }
        // Windows Security prompts (Windows Hello, passkeys, saved passwords) stay on the normal
        // desktop but are protected the same way.
        let win = GetForegroundWindow();
        if win.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(win, Some(&mut pid));
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 512];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(p);
        if !ok {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
        let exe = path.rsplit('\\').next().unwrap_or("");
        matches!(exe, "credentialuibroker.exe" | "consent.exe" | "logonui.exe")
    }
}

/// A tiny mouse nudge wakes monitors that went to sleep, so the phone isn't stuck on a black screen.
pub fn wake_displays() {
    send(&[mouse(MOUSEEVENTF_MOVE, 0, 0, 0)]);
}
