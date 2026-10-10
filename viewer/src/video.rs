//! Decodes the PC's H.264 video on its own thread and hands the newest picture to the UI.

use eframe::egui::{Color32, ColorImage, Context};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Video {
    input: mpsc::Sender<Option<Vec<u8>>>,
    latest: Mutex<Option<ColorImage>>,
}

impl Video {
    /// `keyframe` asks the PC for a full picture (after a decode error).
    pub fn start(ctx: Context, keyframe: impl Fn() + Send + 'static) -> Arc<Self> {
        let (input, frames) = mpsc::channel::<Option<Vec<u8>>>();
        let video = Arc::new(Video { input, latest: Mutex::new(None) });
        let weak = Arc::downgrade(&video);
        std::thread::spawn(move || {
            let mut decoder = match Decoder::new() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("video decoder unavailable: {e}");
                    return;
                }
            };
            let mut asked = Instant::now() - Duration::from_secs(5);
            while let Ok(Some(data)) = frames.recv() {
                let picture = match decoder.decode(&data) {
                    Ok(Some(yuv)) => {
                        let (w, h) = yuv.dimensions();
                        let mut pixels = vec![Color32::BLACK; w * h];
                        // Color32 is four bytes (r, g, b, a); the decoder writes exactly that.
                        let bytes = unsafe { std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<u8>(), w * h * 4) };
                        yuv.write_rgba8(bytes);
                        Some(ColorImage::new([w, h], pixels))
                    }
                    Ok(None) => None,
                    Err(_) => {
                        if asked.elapsed() > Duration::from_secs(1) {
                            asked = Instant::now();
                            keyframe();
                        }
                        None
                    }
                };
                let Some(picture) = picture else { continue };
                let Some(video) = weak.upgrade() else { break };
                *video.latest.lock().unwrap() = Some(picture);
                ctx.request_repaint();
            }
        });
        video
    }

    pub fn push(&self, data: Vec<u8>) {
        let _ = self.input.send(Some(data));
    }

    pub fn stop(&self) {
        let _ = self.input.send(None);
    }

    /// The newest decoded picture, if there's one the UI hasn't shown yet.
    pub fn take(&self) -> Option<ColorImage> {
        self.latest.lock().unwrap().take()
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.stop();
    }
}
