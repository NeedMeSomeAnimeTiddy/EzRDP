//! Keyboard keys → Windows virtual-key codes (what the PC side expects, on Windows and Linux).

use eframe::egui::Key;

pub const SHIFT: u16 = 0xA0;
pub const CTRL: u16 = 0xA2;
pub const ALT: u16 = 0xA4;
pub const WIN: u16 = 0x5B;

/// How a key is sent.
pub enum Kind {
    /// Shift, Ctrl, Alt, Windows/Super.
    Modifier,
    /// Arrows, F-keys, Enter, Esc...: always sent as a key press.
    Special,
    /// Letters, digits, punctuation: sent as typed text, or as a key press while Ctrl/Alt/Win is held.
    Printable,
}

pub fn vk(key: Key) -> Option<(u16, Kind)> {
    use Key::*;
    use Kind::*;
    let special = |v: u16| Some((v, Special));
    let printable = |v: u16| Some((v, Printable));
    match key {
        ArrowDown => special(0x28),
        ArrowLeft => special(0x25),
        ArrowRight => special(0x27),
        ArrowUp => special(0x26),
        Escape => special(0x1B),
        Tab => special(0x09),
        Backspace => special(0x08),
        Enter => special(0x0D),
        Insert => special(0x2D),
        Delete => special(0x2E),
        Home => special(0x24),
        End => special(0x23),
        PageUp => special(0x21),
        PageDown => special(0x22),
        BrowserBack => special(0xA6),
        ShiftLeft => Some((SHIFT, Modifier)),
        ShiftRight => Some((0xA1, Modifier)),
        ControlLeft => Some((CTRL, Modifier)),
        ControlRight => Some((0xA3, Modifier)),
        AltLeft => Some((ALT, Modifier)),
        AltRight => Some((0xA5, Modifier)),
        SuperLeft => Some((WIN, Modifier)),
        SuperRight => Some((0x5C, Modifier)),
        Space => printable(0x20),
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 => {
            let n = [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9].iter().position(|k| *k == key)?;
            printable(0x30 + n as u16)
        }
        Minus => printable(0xBD),
        Equals | Plus => printable(0xBB),
        OpenBracket | OpenCurlyBracket => printable(0xDB),
        CloseBracket | CloseCurlyBracket => printable(0xDD),
        Backslash | Pipe => printable(0xDC),
        Semicolon | Colon => printable(0xBA),
        Quote => printable(0xDE),
        Backtick => printable(0xC0),
        Comma => printable(0xBC),
        Period => printable(0xBE),
        Slash | Questionmark => printable(0xBF),
        Exclamationmark => printable(0x31),
        IntlBackslash => printable(0xE2),
        _ => {
            let name = key.name();
            let c = name.chars().next()?;
            if name.len() == 1 && c.is_ascii_uppercase() {
                return printable(c as u16);
            }
            // F1..F24
            let n: u16 = name.strip_prefix('F')?.parse().ok()?;
            (1..=24).contains(&n).then_some((0x70 + n - 1, Special))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        assert!(matches!(vk(Key::A), Some((0x41, Kind::Printable))));
        assert!(matches!(vk(Key::Z), Some((0x5A, Kind::Printable))));
        assert!(matches!(vk(Key::Num7), Some((0x37, Kind::Printable))));
        assert!(matches!(vk(Key::F1), Some((0x70, Kind::Special))));
        assert!(matches!(vk(Key::F12), Some((0x7B, Kind::Special))));
        assert!(matches!(vk(Key::Enter), Some((0x0D, Kind::Special))));
        assert!(matches!(vk(Key::ControlLeft), Some((CTRL, Kind::Modifier))));
        assert!(vk(Key::F35).is_none());
    }
}
