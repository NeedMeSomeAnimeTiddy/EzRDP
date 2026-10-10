//! Plays the PC's sound (16-bit PCM) on this computer's speakers.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Shared {
    /// Stereo samples (-1..1) at the PC's sample rate.
    queue: Mutex<VecDeque<[f32; 2]>>,
    rate: AtomicU32,
    channels: AtomicU32,
    stop: AtomicBool,
}

pub struct Player {
    shared: Arc<Shared>,
    /// When this PC last noted that it's playing another PC's sound (see crate::echo).
    marked: Mutex<std::time::Instant>,
}

/// Keep at most this much sound waiting, so it never lags behind the picture.
const MAX_QUEUE_SECS: f32 = 0.2;

impl Player {
    pub fn start() -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            rate: AtomicU32::new(48_000),
            channels: AtomicU32::new(2),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        // The audio stream lives on its own thread (it can't be moved between threads everywhere).
        std::thread::spawn(move || {
            let stream = match open(s.clone()) {
                Ok(st) => st,
                Err(e) => {
                    eprintln!("no sound: {e}");
                    return;
                }
            };
            let _ = stream.play();
            while !s.stop.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        Self { shared, marked: Mutex::new(std::time::Instant::now() - Duration::from_secs(10)) }
    }

    pub fn configure(&self, rate: u32, channels: u8) {
        self.shared.rate.store(rate.max(8000), Relaxed);
        self.shared.channels.store(channels.max(1) as u32, Relaxed);
        self.shared.queue.lock().unwrap().clear();
    }

    /// Adds s16le interleaved samples.
    pub fn push(&self, data: &[u8]) {
        let mut marked = self.marked.lock().unwrap();
        if marked.elapsed() > Duration::from_millis(500) {
            *marked = std::time::Instant::now();
            crate::echo::playing_remote_sound();
        }
        drop(marked);
        let ch = self.shared.channels.load(Relaxed) as usize;
        let rate = self.shared.rate.load(Relaxed) as f32;
        let mut q = self.shared.queue.lock().unwrap();
        for frame in data.chunks_exact(2 * ch) {
            let l = i16::from_le_bytes([frame[0], frame[1]]) as f32 / 32768.0;
            let r = if ch > 1 { i16::from_le_bytes([frame[2], frame[3]]) as f32 / 32768.0 } else { l };
            q.push_back([l, r]);
        }
        let max = (rate * MAX_QUEUE_SECS) as usize;
        if q.len() > max {
            let extra = q.len() - max / 2;
            q.drain(..extra);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
    }
}

fn open(s: Arc<Shared>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host().default_output_device().ok_or("no speakers found")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    let out_rate = config.sample_rate as f32;
    let out_ch = (config.channels as usize).max(1);
    // Position between source samples, for simple linear resampling.
    let mut pos = 0f32;
    let mut prev = [0f32; 2];
    let mut fill = move |frames: usize, out: &mut dyn FnMut(usize, f32)| {
        let step = s.rate.load(Relaxed) as f32 / out_rate;
        let mut q = s.queue.lock().unwrap();
        for i in 0..frames {
            let next = q.front().copied().unwrap_or(prev);
            let v = [prev[0] + (next[0] - prev[0]) * pos, prev[1] + (next[1] - prev[1]) * pos];
            for c in 0..out_ch {
                out(i * out_ch + c, v[c.min(1)]);
            }
            pos += step;
            while pos >= 1.0 {
                pos -= 1.0;
                prev = q.pop_front().unwrap_or(prev);
            }
        }
    };
    let err = |e| eprintln!("sound error: {e}");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::I16 => device.build_output_stream::<i16, _, _>(
            config,
            move |data, _| fill(data.len() / out_ch, &mut |i, v| data[i] = (v.clamp(-1.0, 1.0) * 32767.0) as i16),
            err,
            None,
        ),
        _ => device.build_output_stream::<f32, _, _>(
            config,
            move |data, _| fill(data.len() / out_ch, &mut |i, v| data[i] = v),
            err,
            None,
        ),
    };
    stream.map_err(|e| e.to_string())
}
