//! One connected phone: auth, then a video thread, an audio thread and an input loop.

use crate::capture::{CapError, Monitor, Source, monitors};
use crate::convert::{Yuv, out_size};
use crate::encoder::{H264, quality, scale_for};
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
}

impl Shared {
    pub fn stopped(&self) -> bool {
        self.stop.load(Relaxed)
    }
}

/// Runs a session until the phone disconnects or `kick` is set by a newer connection.
pub fn run(stream: TcpStream, login: Login, lan_ip: Option<String>, shared: Arc<Shared>) {
    let tx = Arc::new(Sender::new(stream.try_clone().unwrap(), login.tx));
    shared.quality.store(login.quality, Relaxed);
    if let Some(ip) = lan_ip {
        let _ = tx.send(S_INFO, ip.as_bytes());
    }

    let mons = monitors();
    let primary = mons.iter().position(|m| m.primary).unwrap_or(0);
    shared.monitor.store(primary as u8, Relaxed);
    let _ = send_monitors(&tx, &mons);

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
}

pub fn new_shared() -> Arc<Shared> {
    Arc::new(Shared {
        stop: AtomicBool::new(false),
        audio_on: AtomicBool::new(false),
        monitor: AtomicU8::new(0),
        quality: AtomicU8::new(1),
        generation: AtomicU32::new(0),
        last_ack: AtomicU32::new(0),
        keyframe: AtomicBool::new(false),
        current: Mutex::new(None),
    })
}

pub fn kick(shared: &Shared, stream: &TcpStream) {
    shared.stop.store(true, Relaxed);
    let _ = stream.shutdown(Shutdown::Both);
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
                input.button(b, d != 0);
            }
        }
        C_SCROLL => {
            if let (Some(dy), Some(dx)) = (p.i16(), p.i16()) {
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

fn video_loop(tx: &Sender, shared: &Shared) {
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
    }
    wake_displays();
    let mut frame_id: u32 = 0;
    let mut last_error: Option<String> = None;
    'outer: while !shared.stopped() {
        let generation = shared.generation.load(Relaxed);
        let q = quality(shared.quality.load(Relaxed));
        let mut cap = match Source::open(shared.monitor.load(Relaxed) as usize) {
            Ok(c) => c,
            Err(e) => {
                // Happens briefly on the lock screen, UAC prompts and display changes.
                if last_error.as_deref() != Some(e.as_str()) {
                    eprintln!("capture unavailable: {e}");
                    last_error = Some(e);
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        last_error = None;
        let mut enc = match H264::new(q) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("encoder error: {e}");
                break;
            }
        };
        let m = cap.monitor().clone();
        *shared.current.lock().unwrap() = Some(m.clone());
        let scale = scale_for(m.w as usize, q.max_width);
        let (vw, vh) = out_size(m.w as usize, m.h as usize, scale);
        let index = shared.monitor.load(Relaxed);
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
        let mut need_send = false;
        // After the screen stops changing, re-encode a few times so the picture sharpens up.
        let mut refine_left = 0;

        while !shared.stopped() && shared.generation.load(Relaxed) == generation {
            match cap.frame(16, &mut yuv, scale) {
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
                    eprintln!("capture error: {e}");
                    continue 'outer;
                }
            }

            let cur = cursor().map(|(x, y)| ((x - m.x) as f32 / m.w as f32, (y - m.y) as f32 / m.h as f32));
            if cur != last_cursor {
                last_cursor = cur;
                let (x, y) = cur.unwrap_or((0.0, 0.0));
                let mut p = x.to_be_bytes().to_vec();
                p.extend_from_slice(&y.to_be_bytes());
                p.push(cur.is_some() as u8);
                if tx.send(S_CURSOR, &p).is_err() {
                    break 'outer;
                }
            }

            if shared.keyframe.swap(false, Relaxed) {
                enc.force_keyframe();
                need_send = have_image;
            }
            let in_flight = frame_id.wrapping_sub(shared.last_ack.load(Relaxed));
            if !need_send || !have_image || in_flight >= MAX_IN_FLIGHT || last_sent.elapsed() < min_gap {
                continue;
            }
            need_send = false;
            last_sent = Instant::now();
            match enc.encode(&yuv) {
                Ok(Some((data, key))) => {
                    frame_id = frame_id.wrapping_add(1);
                    let mut head = frame_id.to_be_bytes().to_vec();
                    head.push(key as u8);
                    if tx.send_parts(S_VIDEO_FRAME, &head, data).is_err() {
                        break 'outer;
                    }
                }
                Ok(None) => need_send = true, // encoder skipped it to save bandwidth; try again
                Err(e) => eprintln!("encode error: {e}"),
            }
        }
    }
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS);
    }
}
