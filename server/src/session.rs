//! One connected phone: a video thread, an audio thread and an input loop.

use crate::capture::{CapError, Monitor, Source, monitors};
use crate::convert::{Yuv, out_size};
use crate::encoder::{H264, Quality, quality, scale_for};
use crate::hwenc::HwEncoder;
use crate::input::{Input, cursor, wake_displays};
use crate::protocol::*;
use crate::login::Login;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::System::Power::*;

/// Frames sent but not yet confirmed by the phone. Keeping this small stops lag from piling up.
const MAX_IN_FLIGHT: u32 = 3;

pub struct Shared {
    stop: AtomicBool,
    pub audio_on: AtomicBool,
    monitor: AtomicU8,
    quality: AtomicU8,
    /// Bumped when monitor or quality changes so the video loop restarts.
    generation: AtomicU32,
    last_ack: AtomicU32,
    keyframe: AtomicBool,
    current: Mutex<Option<Monitor>>,
    /// Biggest picture the phone can show, and whether the graphics-card encoder may be used.
    max_size: (usize, usize),
    hw_encoder: AtomicBool,
    phone_name: String,
}

impl Shared {
    pub fn stopped(&self) -> bool {
        self.stop.load(Relaxed)
    }
}

/// The connected phone's sender, so the PC side can say goodbye before hanging up.
static CURRENT: Mutex<Option<Arc<Sender>>> = Mutex::new(None);

/// Ends the session from the PC: tells the phone why (so it doesn't reconnect), then hangs up.
pub fn end(reason: u8) {
    let tx = CURRENT.lock().unwrap().take();
    // On its own thread: a slow network mustn't freeze the tray menu.
    std::thread::spawn(move || {
        if let Some(tx) = tx {
            let _ = tx.send(S_BYE, &[reason]);
        }
        crate::server::hang_up();
    });
}

/// A phone is controlling this PC right now.
pub fn active() -> bool {
    CURRENT.lock().unwrap().is_some()
}

fn end_from_banner() {
    end(BYE_ENDED);
}

/// Runs a session until the phone disconnects or the socket is shut down (new phone / Pause).
pub fn run(stream: TcpStream, login: Login, info: Vec<u8>, _app: Arc<crate::state::App>) {
    let shared = new_shared(&login);
    let tx = Arc::new(Sender::new(stream.try_clone().unwrap(), login.tx));
    shared.quality.store(login.quality, Relaxed);
    *CURRENT.lock().unwrap() = Some(tx.clone());
    let _ = tx.send(S_INFO, &info);

    // Start on the screen the mouse is on: that's where the person was working.
    let mons = monitors();
    let start = cursor()
        .and_then(|(x, y)| mons.iter().position(|m| x >= m.x && x < m.x + m.w && y >= m.y && y < m.y + m.h))
        .or_else(|| mons.iter().position(|m| m.primary))
        .unwrap_or(0);
    shared.monitor.store(start as u8, Relaxed);
    let _ = send_monitors(&tx, &mons);
    unsafe {
        // Finer sleep timing for smoother frame pacing.
        windows::Win32::Media::timeBeginPeriod(1);
    }

    let video = {
        let (tx, shared) = (tx.clone(), shared.clone());
        std::thread::spawn(move || video_loop(&tx, &shared))
    };
    let audio = {
        let (tx, shared) = (tx.clone(), shared.clone());
        std::thread::spawn(move || crate::audio::run(&shared, &tx))
    };

    let pinger = {
        let (tx, shared) = (tx.clone(), shared.clone());
        std::thread::spawn(move || {
            while !shared.stopped() && tx.send(S_PING, &[]).is_ok() {
                std::thread::sleep(Duration::from_secs(2));
            }
        })
    };

    let mut input = Input::new();
    let mut reader = Receiver::new(std::io::BufReader::new(stream.try_clone().unwrap()), login.rx);
    while !shared.stopped() {
        let Ok((kind, body)) = reader.read() else { break };
        handle(kind, &body, &shared, &mut input, &tx);
    }
    input.release_all();
    shared.stop.store(true, Relaxed);
    let _ = stream.shutdown(Shutdown::Both);
    let _ = video.join();
    let _ = audio.join();
    let _ = pinger.join();
    crate::overlay::hide();
    {
        let mut cur = CURRENT.lock().unwrap();
        if cur.as_ref().is_some_and(|t| Arc::ptr_eq(t, &tx)) {
            *cur = None;
        }
    }
    unsafe {
        windows::Win32::Media::timeEndPeriod(1);
    }
}

fn new_shared(login: &Login) -> Arc<Shared> {
    Arc::new(Shared {
        stop: AtomicBool::new(false),
        audio_on: AtomicBool::new(false),
        monitor: AtomicU8::new(0),
        quality: AtomicU8::new(1),
        generation: AtomicU32::new(0),
        last_ack: AtomicU32::new(0),
        keyframe: AtomicBool::new(false),
        current: Mutex::new(None),
        max_size: (login.max_w as usize, login.max_h as usize),
        hw_encoder: AtomicBool::new(login.hw_encoder),
        phone_name: login.device_name.clone(),
    })
}


fn send_monitors(tx: &Sender, mons: &[Monitor]) -> std::io::Result<()> {
    let mut p = vec![mons.len() as u8];
    for (i, m) in mons.iter().enumerate() {
        p.push(i as u8);
        p.push(m.primary as u8);
        p.extend_from_slice(&(m.w as u16).to_be_bytes());
        p.extend_from_slice(&(m.h as u16).to_be_bytes());
        p.extend_from_slice(&(m.name.len() as u16).to_be_bytes());
        p.extend_from_slice(m.name.as_bytes());
    }
    tx.send(S_MONITORS, &p)
}

fn handle(kind: u8, body: &[u8], shared: &Shared, input: &mut Input, tx: &Sender) {
    let mut p = Payload(body);
    let mon = || shared.current.lock().unwrap().clone();
    match kind {
        C_MOUSE_ABS => {
            if let (Some(x), Some(y), Some(m)) = (p.f32(), p.f32(), mon()) {
                input.move_abs(&m, x, y);
            }
        }
        C_MOUSE_REL => {
            if let (Some(x), Some(y), Some(m)) = (p.f32(), p.f32(), mon()) {
                input.move_rel(&m, x, y);
            }
        }
        C_MOUSE_BUTTON => {
            if let (Some(b), Some(d)) = (p.u8(), p.u8()) {
                // A click must land on the screen the phone is showing, not wherever the mouse was left.
                if d != 0 && let Some(m) = mon() {
                    input.ensure_on(&m);
                }
                input.button(b, d != 0);
            }
        }
        C_SCROLL => {
            if let (Some(dy), Some(dx)) = (p.i16(), p.i16()) {
                if let Some(m) = mon() {
                    input.ensure_on(&m);
                }
                input.scroll(dy, dx);
            }
        }
        C_TEXT => input.text(&String::from_utf8_lossy(body)),
        C_KEY => {
            if let (Some(vk), Some(d)) = (p.u16(), p.u8()) {
                input.key(vk, d != 0);
            }
        }
        C_SELECT_MONITOR => {
            if let Some(i) = p.u8() {
                shared.monitor.store(i, Relaxed);
                shared.generation.fetch_add(1, Relaxed);
                let _ = send_monitors(tx, &monitors());
            }
        }
        C_FRAME_ACK => {
            if let Some(id) = p.u32() {
                shared.last_ack.fetch_max(id, Relaxed);
            }
        }
        C_SET_AUDIO => {
            if let Some(on) = p.u8() {
                shared.audio_on.store(on != 0, Relaxed);
            }
        }
        C_KEYFRAME => shared.keyframe.store(true, Relaxed),
        C_SET_QUALITY => {
            if let Some(q) = p.u8() {
                shared.quality.store(q, Relaxed);
                shared.generation.fetch_add(1, Relaxed);
            }
        }
        C_PING => {}
        _ => {}
    }
}

/// Whichever encoder is in use: the graphics card's, or the software one.
enum Video {
    Hard(HwEncoder),
    Soft(H264),
}

impl Video {
    fn open(w: usize, h: usize, q: Quality, try_hw: bool) -> Result<Self, String> {
        if try_hw {
            match HwEncoder::new(w, h, q.fps, q.bitrate) {
                Ok(e) => {
                    crate::log!("Video: graphics card encoder ({}) at {w}x{h}", e.name);
                    return Ok(Video::Hard(e));
                }
                Err(e) => crate::log!("Video: graphics card encoder unavailable ({e}); using software"),
            }
        }
        crate::log!("Video: software encoder at {w}x{h}");
        H264::new(q).map(Video::Soft).map_err(|e| e.to_string())
    }

    fn encode(&mut self, yuv: &Yuv) -> Result<Option<(&[u8], bool)>, String> {
        match self {
            Video::Hard(e) => e.encode(yuv),
            Video::Soft(e) => e.encode(yuv).map_err(|e| e.to_string()),
        }
    }

    fn force_keyframe(&mut self) {
        match self {
            Video::Hard(e) => e.force_keyframe(),
            Video::Soft(e) => e.force_keyframe(),
        }
    }

    fn set_bitrate(&mut self, bps: u32) {
        match self {
            Video::Hard(e) => e.set_bitrate(bps),
            Video::Soft(e) => e.set_bitrate(bps),
        }
    }
}

/// Picks the shrink factor: the quality's width limit, and never bigger than the phone can decode.
fn pick_scale(m: &Monitor, q: Quality, max: (usize, usize)) -> usize {
    let (w, h) = (m.w as usize, m.h as usize);
    let mut scale = scale_for(w, q.max_width);
    // Compare long side with long side, so a portrait monitor isn't penalised.
    let (long, short) = (w.max(h), w.min(h));
    let (max_long, max_short) = (max.0.max(max.1), max.0.min(max.1));
    if max_long > 0 && max_short > 0 {
        while scale < 4 && (long / scale > max_long || short / scale > max_short) {
            scale += 1;
        }
    }
    scale
}

fn video_loop(tx: &Sender, shared: &Shared) {
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
    }
    wake_displays();
    let mut frame_id: u32 = 0;
    let mut last_error: Option<String> = None;
    let mut last_monitor = None;
    'outer: while !shared.stopped() {
        let generation = shared.generation.load(Relaxed);
        let q = quality(shared.quality.load(Relaxed));
        let mut cap = match Source::open(shared.monitor.load(Relaxed) as usize) {
            Ok(c) => c,
            Err(e) => {
                // Happens briefly on the lock screen, UAC prompts and display changes.
                if last_error.as_deref() != Some(e.as_str()) {
                    crate::log!("capture unavailable: {e}");
                    last_error = Some(e);
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        last_error = None;
        let m = cap.monitor().clone();
        *shared.current.lock().unwrap() = Some(m.clone());
        // Switched to another monitor: bring the mouse along, so it's where the phone is looking.
        let index = shared.monitor.load(Relaxed);
        if last_monitor.is_some_and(|i| i != index) {
            crate::input::bring_onto(&m);
        }
        last_monitor = Some(index);
        crate::overlay::show(&shared.phone_name, &m, end_from_banner);
        let scale = pick_scale(&m, q, shared.max_size);
        let (vw, vh) = out_size(m.w as usize, m.h as usize, scale);
        let mut enc = match Video::open(vw, vh, q, shared.hw_encoder.load(Relaxed)) {
            Ok(e) => e,
            Err(e) => {
                crate::log!("encoder error: {e}");
                break;
            }
        };
        let mut cfg = vec![index];
        cfg.extend_from_slice(&(vw as u16).to_be_bytes());
        cfg.extend_from_slice(&(vh as u16).to_be_bytes());
        if tx.send(S_VIDEO_CONFIG, &cfg).is_err() {
            break;
        }

        let min_gap = Duration::from_secs(1) / q.fps;
        let mut yuv = Yuv::default();
        let mut have_image = false;
        let mut last_sent = Instant::now() - min_gap;
        let mut last_cursor = None;
        let mut last_cursor_sent = Instant::now();
        let mut need_send = false;
        // After the screen stops changing, re-encode a few times so the picture sharpens up.
        let mut refine_left = 0;
        // Bitrate adapts to the network: back off when frames pile up, creep back up when clear.
        let mut bitrate = q.bitrate;
        let mut window_start = Instant::now();
        let mut blocked = 0u32;
        let mut sent = 0u32;
        // Graphics-card encoders that stop giving pictures get swapped for the software one.
        let mut empty_in_row = 0u32;

        while !shared.stopped() && shared.generation.load(Relaxed) == generation {
            match cap.frame(8, &mut yuv, scale) {
                Ok(true) => {
                    have_image = true;
                    need_send = true;
                    refine_left = 3;
                }
                Ok(false) => {
                    if refine_left > 0 && last_sent.elapsed() > Duration::from_millis(150) {
                        refine_left -= 1;
                        need_send = true;
                    }
                }
                Err(CapError::Lost) => continue 'outer,
                Err(CapError::Other(e)) => {
                    crate::log!("capture error: {e}");
                    continue 'outer;
                }
            }

            let cur = cursor().map(|(x, y)| ((x - m.x) as f32 / m.w as f32, (y - m.y) as f32 / m.h as f32));
            // Also resend now and then, so the phone's pointer can't stay out of step.
            if cur != last_cursor || last_cursor_sent.elapsed() > Duration::from_millis(500) {
                last_cursor = cur;
                last_cursor_sent = Instant::now();
                let (x, y) = cur.unwrap_or((0.0, 0.0));
                let mut p = x.to_be_bytes().to_vec();
                p.extend_from_slice(&y.to_be_bytes());
                p.push(cur.is_some() as u8);
                if tx.send(S_CURSOR, &p).is_err() {
                    break 'outer;
                }
            }

            if window_start.elapsed() >= Duration::from_secs(1) {
                let target = if blocked > 6 {
                    (bitrate as f32 * 0.7) as u32
                } else if blocked == 0 && sent > 0 {
                    (bitrate as f32 * 1.15) as u32
                } else {
                    bitrate
                }
                .clamp(q.bitrate / 6, q.bitrate);
                if target != bitrate {
                    bitrate = target;
                    enc.set_bitrate(bitrate);
                }
                window_start = Instant::now();
                blocked = 0;
                sent = 0;
            }

            if shared.keyframe.swap(false, Relaxed) {
                enc.force_keyframe();
                need_send = have_image;
            }
            if !need_send || !have_image || last_sent.elapsed() < min_gap {
                continue;
            }
            let in_flight = frame_id.wrapping_sub(shared.last_ack.load(Relaxed));
            if in_flight >= MAX_IN_FLIGHT {
                blocked += 1;
                continue;
            }
            need_send = false;
            last_sent = Instant::now();
            match enc.encode(&yuv) {
                Ok(Some((data, key))) => {
                    empty_in_row = 0;
                    sent += 1;
                    frame_id = frame_id.wrapping_add(1);
                    let mut head = frame_id.to_be_bytes().to_vec();
                    head.push(key as u8);
                    if tx.send_parts(S_VIDEO_FRAME, &head, data).is_err() {
                        break 'outer;
                    }
                }
                Ok(None) => {
                    need_send = true; // skipped to save bandwidth, or still busy; try again
                    empty_in_row += 1;
                    if empty_in_row > 60 && matches!(enc, Video::Hard(_)) {
                        crate::log!("Video: graphics card encoder stopped giving pictures; switching to software");
                        shared.hw_encoder.store(false, Relaxed);
                        continue 'outer;
                    }
                }
                Err(e) => {
                    crate::log!("encode error: {e}");
                    if matches!(enc, Video::Hard(_)) {
                        shared.hw_encoder.store(false, Relaxed);
                        continue 'outer;
                    }
                }
            }
        }
    }
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS);
    }
}
