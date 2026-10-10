//! Mouse and keyboard on Linux (X11), through the XTEST extension.
//! The other side sends Windows virtual-key codes; they're turned into X keys here.

use super::capture::Monitor;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Keycode, Keysym, Window, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use std::time::{Duration, Instant};

const MODIFIERS: [u16; 8] = [0x10, 0x11, 0x12, 0x5B, 0xA0, 0xA2, 0xA4, 0x5C]; // shift ctrl alt win, left variants
const SHIFT_L: Keysym = 0xffe1;

struct X {
    conn: RustConnection,
    root: Window,
    min_code: Keycode,
    per_code: usize,
    /// Keysyms for each keycode, from `min_code` up.
    map: Vec<Keysym>,
    /// Keycodes with nothing on them, borrowed to type characters the keyboard doesn't have.
    spares: Vec<Keycode>,
    /// Which character each borrowed keycode types right now, and when it was last used.
    borrowed: Vec<(Keycode, Keysym, Instant)>,
}

impl X {
    fn open() -> Option<Self> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let setup = conn.setup();
        let root = setup.roots[screen].root;
        let (min_code, max_code) = (setup.min_keycode, setup.max_keycode);
        let reply = conn.get_keyboard_mapping(min_code, max_code - min_code + 1).ok()?.reply().ok()?;
        let per_code = reply.keysyms_per_keycode.max(1) as usize;
        let map = reply.keysyms;
        let spares = (0..map.len() / per_code)
            .rev()
            .filter(|&i| map[i * per_code..(i + 1) * per_code].iter().all(|&k| k == 0))
            .map(|i| min_code + i as u8)
            .collect();
        Some(Self { conn, root, min_code, per_code, map, spares, borrowed: Vec::new() })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        let _ = self.conn.xtest_fake_input(kind, detail, 0, self.root, x, y, 0);
    }

    /// Waits until the X server has handled everything sent so far.
    fn sync(&self) {
        let _ = self.conn.get_input_focus().map(|c| c.reply());
    }

    fn flush(&self) {
        let _ = self.conn.flush();
    }

    /// Keycode for a keysym, and whether Shift is needed to get it.
    fn find(&self, sym: Keysym) -> Option<(Keycode, bool)> {
        for (i, codes) in self.map.chunks(self.per_code).enumerate() {
            if let Some(level) = codes.iter().take(2).position(|&k| k == sym) {
                return Some((self.min_code + i as u8, level == 1));
            }
        }
        None
    }

    fn tap_keysym(&mut self, sym: Keysym) {
        if let Some((code, shift)) = self.find(sym) {
            let shift_code = if shift { self.find(SHIFT_L).map(|(c, _)| c) } else { None };
            if let Some(s) = shift_code {
                self.fake(KEY_PRESS_EVENT, s, 0, 0);
            }
            self.fake(KEY_PRESS_EVENT, code, 0, 0);
            self.fake(KEY_RELEASE_EVENT, code, 0, 0);
            if let Some(s) = shift_code {
                self.fake(KEY_RELEASE_EVENT, s, 0, 0);
            }
            return;
        }
        // Not on the keyboard (accents, symbols, other alphabets): put it on a spare key.
        // It stays there for a while: apps look the key up when they get the press, so
        // changing it straight back would make them see the wrong character.
        if let Some(code) = self.borrow_key(sym) {
            self.fake(KEY_PRESS_EVENT, code, 0, 0);
            self.fake(KEY_RELEASE_EVENT, code, 0, 0);
        }
    }

    fn borrow_key(&mut self, sym: Keysym) -> Option<Keycode> {
        let now = Instant::now();
        if let Some(b) = self.borrowed.iter_mut().find(|b| b.1 == sym) {
            b.2 = now;
            return Some(b.0);
        }
        let free = self.spares.iter().copied().find(|c| !self.borrowed.iter().any(|b| b.0 == *c));
        let code = match free {
            Some(c) => {
                self.borrowed.push((c, sym, now));
                c
            }
            None => {
                // Reuse the one used longest ago, giving apps a moment to finish with it.
                let b = self.borrowed.iter_mut().min_by_key(|b| b.2)?;
                let wait = Duration::from_millis(60).saturating_sub(b.2.elapsed());
                *b = (b.0, sym, now);
                let code = b.0;
                std::thread::sleep(wait);
                code
            }
        };
        let syms = vec![sym; self.per_code];
        self.conn.change_keyboard_mapping(1, code, self.per_code as u8, &syms).ok()?;
        self.sync();
        Some(code)
    }

    /// Gives the borrowed keys back (blank again).
    fn return_keys(&mut self) {
        let blank = vec![0; self.per_code];
        for (code, _, _) in std::mem::take(&mut self.borrowed) {
            let _ = self.conn.change_keyboard_mapping(1, code, self.per_code as u8, &blank);
        }
        self.flush();
    }

    fn pointer(&self) -> Option<(i32, i32)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x as i32, r.root_y as i32))
    }
}

/// Windows virtual-key code → X keysym.
fn keysym(vk: u16) -> Option<Keysym> {
    Some(match vk {
        0x08 => 0xff08, // BackSpace
        0x09 => 0xff09, // Tab
        0x0D => 0xff0d, // Return
        0x10 | 0xA0 => 0xffe1, // Shift_L
        0xA1 => 0xffe2,
        0x11 | 0xA2 => 0xffe3, // Control_L
        0xA3 => 0xffe4,
        0x12 | 0xA4 => 0xffe9, // Alt_L
        0xA5 => 0xffea,
        0x13 => 0xff13, // Pause
        0x14 => 0xffe5, // Caps_Lock
        0x1B => 0xff1b, // Escape
        0x20 => 0x20,
        0x21 => 0xff55, // Prior
        0x22 => 0xff56, // Next
        0x23 => 0xff57, // End
        0x24 => 0xff50, // Home
        0x25 => 0xff51, // Left
        0x26 => 0xff52, // Up
        0x27 => 0xff53, // Right
        0x28 => 0xff54, // Down
        0x2C => 0xff61, // Print
        0x2D => 0xff63, // Insert
        0x2E => 0xffff, // Delete
        0x30..=0x39 => vk as Keysym,
        0x41..=0x5A => vk as Keysym + 0x20, // lower-case letters
        0x5B => 0xffeb, // Super_L
        0x5C => 0xffec,
        0x5D => 0xff67, // Menu
        0x60..=0x69 => 0xffb0 + (vk - 0x60) as Keysym, // KP_0..9
        0x6A => 0xffaa,
        0x6B => 0xffab,
        0x6D => 0xffad,
        0x6E => 0xffae,
        0x6F => 0xffaf,
        0x70..=0x87 => 0xffbe + (vk - 0x70) as Keysym, // F1..F24
        0x90 => 0xff7f, // Num_Lock
        0x91 => 0xff14, // Scroll_Lock
        0xAD => 0x1008ff12, // mute
        0xAE => 0x1008ff11, // volume down
        0xAF => 0x1008ff13, // volume up
        0xB0 => 0x1008ff17, // next track
        0xB1 => 0x1008ff16, // previous track
        0xB3 => 0x1008ff14, // play/pause
        0xBA => 0x3b, // ;
        0xBB => 0x3d, // =
        0xBC => 0x2c, // ,
        0xBD => 0x2d, // -
        0xBE => 0x2e, // .
        0xBF => 0x2f, // /
        0xC0 => 0x60, // `
        0xDB => 0x5b, // [
        0xDC => 0x5c, // \
        0xDD => 0x5d, // ]
        0xDE => 0x27, // '
        _ => return None,
    })
}

/// Keysym for a typed character.
fn char_keysym(c: char) -> Keysym {
    match c {
        '\n' | '\r' => 0xff0d,
        '\t' => 0xff09,
        '\u{8}' => 0xff08,
        ' '..='~' | '\u{a0}'..='\u{ff}' => c as Keysym,
        _ => 0x0100_0000 + c as Keysym,
    }
}

pub struct Input {
    x: Option<X>,
    /// On Wayland, mouse and keys go through the desktop's remote desktop portal instead.
    wl: Option<std::sync::Arc<super::wayland::Portal>>,
    /// Leftover sub-pixel movement from relative moves, and part-notches of scrolling.
    frac: (f64, f64),
    scroll_left: (i32, i32),
    held_keys: Vec<u16>,
    held_buttons: [bool; 3],
}

impl Input {
    pub fn new() -> Self {
        if super::wayland::active() {
            let wl = super::wayland::portal().map_err(|e| crate::log!("Can't control the mouse and keyboard: {e}")).ok();
            return Self { x: None, wl, frac: (0.0, 0.0), scroll_left: (0, 0), held_keys: Vec::new(), held_buttons: [false; 3] };
        }
        let x = X::open();
        if x.is_none() {
            crate::log!("Can't control the mouse and keyboard: no X display");
        }
        Self { x, wl: None, frac: (0.0, 0.0), scroll_left: (0, 0), held_keys: Vec::new(), held_buttons: [false; 3] }
    }

    fn move_to(&self, x: i32, y: i32) {
        if let Some(c) = &self.x {
            c.fake(MOTION_NOTIFY_EVENT, 0, x as i16, y as i16);
            c.flush();
        }
    }

    /// Moves to a point given as 0..1 fractions of the monitor.
    pub fn move_abs(&mut self, m: &Monitor, fx: f32, fy: f32) {
        if let Some(w) = &self.wl {
            return w.move_abs(m, fx, fy);
        }
        let x = m.x + (fx.clamp(0.0, 1.0) * (m.w - 1) as f32).round() as i32;
        let y = m.y + (fy.clamp(0.0, 1.0) * (m.h - 1) as f32).round() as i32;
        self.frac = (0.0, 0.0);
        self.move_to(x, y);
    }

    /// Moves relative to the current position; deltas are fractions of the monitor size.
    pub fn move_rel(&mut self, m: &Monitor, fx: f32, fy: f32) {
        if let Some(w) = &self.wl {
            return w.move_rel(m, fx, fy);
        }
        let Some((px, py)) = self.x.as_ref().and_then(X::pointer) else { return };
        let dx = fx as f64 * m.w as f64 + self.frac.0;
        let dy = fy as f64 * m.h as f64 + self.frac.1;
        let (ix, iy) = (dx.trunc(), dy.trunc());
        self.frac = (dx - ix, dy - iy);
        if ix == 0.0 && iy == 0.0 {
            return;
        }
        let x = (px + ix as i32).clamp(m.x, m.x + m.w - 1);
        let y = (py + iy as i32).clamp(m.y, m.y + m.h - 1);
        self.move_to(x, y);
    }

    /// If the mouse is on a different screen than the one being viewed, brings it to the middle
    /// of the viewed one, so clicks and typing go where the other side is looking.
    pub fn ensure_on(&mut self, m: &Monitor) {
        // On Wayland every move names its screen, so there's nothing to do.
        if let Some((x, y)) = self.x.as_ref().and_then(X::pointer)
            && (x < m.x || x >= m.x + m.w || y < m.y || y >= m.y + m.h)
        {
            self.move_to(m.x + m.w / 2, m.y + m.h / 2);
        }
    }

    pub fn button(&mut self, b: u8, down: bool) {
        let detail = match b {
            0 => 1,
            1 => 3,
            2 => 2,
            _ => return,
        };
        self.held_buttons[b as usize] = down;
        if let Some(w) = &self.wl {
            return w.button(b, down);
        }
        if let Some(c) = &self.x {
            c.fake(if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, detail, 0, 0);
            c.flush();
        }
    }

    /// 120 = one wheel notch (as on Windows). X scrolls with buttons 4-7, one click per notch.
    pub fn scroll(&mut self, dy: i16, dx: i16) {
        self.scroll_left.0 += dy as i32;
        self.scroll_left.1 += dx as i32;
        if let Some(w) = &self.wl {
            let (ny, nx) = (self.scroll_left.0 / 120, self.scroll_left.1 / 120);
            self.scroll_left.0 -= ny * 120;
            self.scroll_left.1 -= nx * 120;
            // Wayland counts down/right as positive; a Windows wheel notch up is +120.
            w.scroll(true, -ny);
            w.scroll(false, nx);
            return;
        }
        let Some(c) = &self.x else { return };
        let clicks = |left: &mut i32, pos: u8, neg: u8| {
            let n = *left / 120;
            *left -= n * 120;
            let button = if n > 0 { pos } else { neg };
            for _ in 0..n.unsigned_abs().min(50) {
                c.fake(BUTTON_PRESS_EVENT, button, 0, 0);
                c.fake(BUTTON_RELEASE_EVENT, button, 0, 0);
            }
        };
        clicks(&mut self.scroll_left.0, 4, 5);
        clicks(&mut self.scroll_left.1, 7, 6);
        c.flush();
    }

    pub fn key(&mut self, vk: u16, down: bool) {
        if down {
            if !self.held_keys.contains(&vk) {
                self.held_keys.push(vk);
            }
        } else {
            self.held_keys.retain(|&k| k != vk);
        }
        if let Some(w) = &self.wl {
            if let Some(sym) = keysym(vk) {
                w.key(sym, down);
            }
            return;
        }
        let Some(c) = &self.x else { return };
        let Some((code, _)) = keysym(vk).and_then(|s| c.find(s)) else { return };
        c.fake(if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, code, 0, 0);
        c.flush();
    }

    pub fn text(&mut self, s: &str) {
        // With Ctrl/Alt/Super held, press the plain key so shortcuts like Ctrl+C work.
        let shortcut = self.held_keys.iter().any(|k| MODIFIERS.contains(k) && !matches!(k, 0x10 | 0xA0));
        if let Some(w) = &self.wl {
            for ch in s.chars() {
                let sym = if shortcut && ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() as Keysym } else { char_keysym(ch) };
                w.key(sym, true);
                w.key(sym, false);
            }
            return;
        }
        let Some(c) = &mut self.x else { return };
        for ch in s.chars() {
            if shortcut && ch.is_ascii_alphanumeric() {
                if let Some((code, _)) = c.find(ch.to_ascii_lowercase() as Keysym) {
                    c.fake(KEY_PRESS_EVENT, code, 0, 0);
                    c.fake(KEY_RELEASE_EVENT, code, 0, 0);
                }
                continue;
            }
            c.tap_keysym(char_keysym(ch));
        }
        c.flush();
    }

    /// Lets go of everything still held (called when the other side disconnects).
    pub fn release_all(&mut self) {
        for vk in std::mem::take(&mut self.held_keys) {
            self.key(vk, false);
        }
        for b in 0..3 {
            if self.held_buttons[b] {
                self.button(b as u8, false);
            }
        }
        if let Some(x) = &mut self.x {
            x.return_keys();
        }
    }
}

/// Moves the mouse to the middle of `m` if it's on another monitor.
pub fn bring_onto(m: &Monitor) {
    if super::wayland::active() {
        return;
    }
    Input::new().ensure_on(m);
}

/// Cursor position in screen pixels.
pub fn cursor() -> Option<(i32, i32)> {
    // On Wayland the pointer is drawn into the picture and its position isn't known.
    if super::wayland::active() {
        return None;
    }
    thread_local! {
        static CONN: Option<X> = X::open();
    }
    CONN.with(|c| c.as_ref().and_then(X::pointer))
}

/// Not needed on Linux: reading the screen doesn't depend on the monitors being awake.
pub fn wake_displays() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(keysym(0x41), Some(0x61));
        assert_eq!(keysym(0x0D), Some(0xff0d));
        assert_eq!(keysym(0x71), Some(0xffbf)); // F2
        assert_eq!(char_keysym('é'), 0xe9);
        assert_eq!(char_keysym('€'), 0x0100_20ac);
    }
}
