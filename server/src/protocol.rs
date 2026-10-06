//! Wire format. During login, messages are `[type: u8][len: u32][payload]`.
//! After login everything is encrypted: `[len: u32][AES-GCM of (type, payload) + tag]`.
//! All numbers are big-endian (matches Java's DataInputStream).

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;

use crate::crypto::{Seal, TAG};

pub const VERSION: u8 = 4;
pub const DEFAULT_PORT: u16 = 7270;

// Server -> client
pub const S_HELLO: u8 = 0x01; // version u8, pc_id [16], nonce [16], flags u8 (1 = paused), pc name utf8
pub const S_AUTH_RESULT: u8 = 0x02; // RESULT_* u8
pub const S_MONITORS: u8 = 0x03; // count u8, then per monitor: index u8, primary u8, w u16, h u16, name_len u16, name utf8
pub const S_VIDEO_CONFIG: u8 = 0x04; // monitor u8, video w u16, video h u16
pub const S_VIDEO_FRAME: u8 = 0x05; // frame id u32, keyframe u8, h264 annex-b data
pub const S_CURSOR: u8 = 0x06; // x f32, y f32 (0..1 of monitor), visible u8
pub const S_AUDIO_CONFIG: u8 = 0x07; // sample rate u32, channels u8
pub const S_AUDIO: u8 = 0x08; // s16le interleaved PCM
pub const S_PING: u8 = 0x09; // (empty) keep-alive so the phone can spot dead connections
pub const S_INFO: u8 = 0x0A; // home ip (u8 len + utf8), internet ip (u8 len + utf8), port u16
pub const S_PAIR_KEY: u8 = 0x0B; // PC public key [65]
pub const S_PAIR_RESULT: u8 = 0x0C; // RESULT_* u8, then device id [16] if ok
pub const S_BYE: u8 = 0x0D; // BYE_* u8: the PC ended the session on purpose (don't reconnect)

pub const BYE_ENDED: u8 = 0; // someone at the PC clicked disconnect
pub const BYE_PAUSED: u8 = 1; // connections were paused
pub const BYE_REMOVED: u8 = 2; // this phone is no longer trusted

// Client -> server
pub const C_AUTH: u8 = 0x81; // device id [16], nonce [16], proof [32], quality u8, role u8, max w u16, max h u16, flags u8 (1 = graphics-card encoder ok)
pub const C_MOUSE_ABS: u8 = 0x82; // x f32, y f32 (0..1 of monitor)
pub const C_MOUSE_REL: u8 = 0x83; // dx f32, dy f32 (fraction of monitor width/height)
pub const C_MOUSE_BUTTON: u8 = 0x84; // button u8 (0 left, 1 right, 2 middle), down u8
pub const C_SCROLL: u8 = 0x85; // dy i16, dx i16 (120 = one wheel notch)
pub const C_TEXT: u8 = 0x86; // utf8
pub const C_KEY: u8 = 0x87; // windows virtual key u16, down u8
pub const C_SELECT_MONITOR: u8 = 0x88; // index u8
pub const C_FRAME_ACK: u8 = 0x89; // frame id u32
pub const C_SET_AUDIO: u8 = 0x8A; // on u8
pub const C_KEYFRAME: u8 = 0x8B; // (empty)
pub const C_SET_QUALITY: u8 = 0x8C; // quality u8 (0 high, 1 balanced, 2 data saver)
pub const C_PING: u8 = 0x8D; // (empty)
pub const C_PAIR_START: u8 = 0x8E; // commitment [32], phone name utf8
pub const C_PAIR_KEY: u8 = 0x8F; // phone public key [65]

// Phone sharing its own screen (role 1). Phone -> PC:
pub const P_SCREEN_CONFIG: u8 = 0x90; // video w u16, video h u16
pub const P_SCREEN_FRAME: u8 = 0x91; // frame id u32, keyframe u8, h264 annex-b data
// PC -> phone:
pub const S_TOUCH: u8 = 0x20; // action u8 (0 down, 1 move, 2 up), x f32, y f32 (0..1 of the phone screen)
pub const S_NAV: u8 = 0x21; // NAV_* u8
pub const S_TYPE: u8 = 0x22; // utf8 text to type into the focused box
pub const S_SWIPE: u8 = 0x23; // x f32, y f32, dx f32, dy f32 (scroll-wheel swipes)
pub const S_FRAME_ACK: u8 = 0x24; // frame id u32
pub const S_KEYFRAME: u8 = 0x25; // (empty) please send a full picture

pub const NAV_BACK: u8 = 0;
pub const NAV_HOME: u8 = 1;
pub const NAV_RECENTS: u8 = 2;
pub const NAV_NOTIFICATIONS: u8 = 3;
pub const NAV_BACKSPACE: u8 = 4;
pub const NAV_ENTER: u8 = 5;

pub const RESULT_DENIED: u8 = 0; // wrong/untrusted phone, or user clicked Deny
pub const RESULT_OK: u8 = 1;
pub const RESULT_PAUSED: u8 = 2;
pub const RESULT_NOT_LOCAL: u8 = 3; // pairing only works on the home network
pub const RESULT_BUSY: u8 = 4; // another phone is pairing right now

const MAX_CLIENT_MSG: usize = 64 * 1024;
/// Video from a phone sharing its screen can be much bigger.
const MAX_VIDEO_MSG: usize = 8 * 1024 * 1024;

/// Reads a plain (login) message.
pub fn read_msg(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut hdr = [0u8; 5];
    r.read_exact(&mut hdr)?;
    let len = u32::from_be_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
    if len > MAX_CLIENT_MSG {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok((hdr[0], buf))
}

pub fn write_plain(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)
}

/// Reads and decrypts messages from the phone.
pub struct Receiver<R> {
    pub reader: R,
    seal: Seal,
    max: usize,
}

impl<R: Read> Receiver<R> {
    pub fn new(reader: R, seal: Seal) -> Self {
        Self { reader, seal, max: MAX_CLIENT_MSG }
    }

    /// For a phone sending video.
    pub fn new_video(reader: R, seal: Seal) -> Self {
        Self { reader, seal, max: MAX_VIDEO_MSG }
    }

    pub fn read(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut len = [0u8; 4];
        self.reader.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if !(TAG + 1..=self.max).contains(&len) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad message size"));
        }
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf)?;
        if !self.seal.open(&mut buf) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message failed to decrypt"));
        }
        let kind = buf.remove(0);
        Ok((kind, buf))
    }
}

/// Thread-safe encrypted sender. Each message is sealed and written under one lock,
/// so threads never interleave and the message counter always matches write order.
pub struct Sender {
    inner: Mutex<(TcpStream, Seal)>,
}

impl Sender {
    pub fn new(stream: TcpStream, seal: Seal) -> Self {
        Self { inner: Mutex::new((stream, seal)) }
    }

    pub fn send(&self, kind: u8, payload: &[u8]) -> io::Result<()> {
        self.send_parts(kind, payload, &[])
    }

    /// Sends one message made of a small header and a (possibly large) body.
    pub fn send_parts(&self, kind: u8, head: &[u8], body: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(4 + 1 + head.len() + body.len() + TAG);
        buf.extend_from_slice(&[0, 0, 0, 0, kind]);
        buf.extend_from_slice(head);
        buf.extend_from_slice(body);
        let len = (buf.len() - 4 + TAG) as u32;
        buf[..4].copy_from_slice(&len.to_be_bytes());
        let mut guard = self.inner.lock().unwrap();
        let (stream, seal) = &mut *guard;
        let tag = seal.seal_slice(&mut buf[4..]);
        buf.extend_from_slice(&tag);
        stream.write_all(&buf)
    }
}

/// Small big-endian reader over a payload.
pub struct Payload<'a>(pub &'a [u8]);

impl Payload<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        if self.0.len() < N {
            return None;
        }
        let (a, b) = self.0.split_at(N);
        self.0 = b;
        a.try_into().ok()
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_be_bytes)
    }
    pub fn i16(&mut self) -> Option<i16> {
        self.take().map(i16::from_be_bytes)
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_be_bytes)
    }
    pub fn f32(&mut self) -> Option<f32> {
        self.take().map(f32::from_be_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = vec![C_MOUSE_ABS, 0, 0, 0, 8];
        buf.extend_from_slice(&0.25f32.to_be_bytes());
        buf.extend_from_slice(&0.75f32.to_be_bytes());
        let (t, p) = read_msg(&mut buf.as_slice()).unwrap();
        assert_eq!(t, C_MOUSE_ABS);
        let mut p = Payload(&p);
        assert_eq!(p.f32(), Some(0.25));
        assert_eq!(p.f32(), Some(0.75));
        assert_eq!(p.u8(), None);
    }

    #[test]
    fn encrypted_roundtrip() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let a = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (b, _) = l.accept().unwrap();
        let key = [7u8; 32];
        let tx = Sender::new(a, Seal::new(&key));
        let mut rx = Receiver::new(b, Seal::new(&key));
        tx.send_parts(C_TEXT, b"hi ", b"there").unwrap();
        tx.send(C_PING, &[]).unwrap();
        assert_eq!(rx.read().unwrap(), (C_TEXT, b"hi there".to_vec()));
        assert_eq!(rx.read().unwrap(), (C_PING, vec![]));
    }
}
