//! Captures whatever the PC is playing (WASAPI loopback) and streams it as 16-bit PCM.

use crate::protocol::{S_AUDIO, S_AUDIO_CONFIG, Sender};
use crate::session::Shared;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::*;

pub fn run(shared: &Shared, tx: &Sender) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    // Log each problem once, not every retry (e.g. no speakers plugged in).
    let mut last_error = String::new();
    while !shared.stopped() {
        if !shared.audio_on.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if let Err(e) = capture(shared, tx) {
            let e = e.to_string();
            if e != last_error {
                crate::log!("audio: {e}");
                last_error = e;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn capture(shared: &Shared, tx: &Sender) -> windows::core::Result<()> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let fmt_ptr = client.GetMixFormat()?;
        let fmt = std::ptr::read_unaligned(fmt_ptr);
        let tag = fmt.wFormatTag as u32;
        let float = tag == WAVE_FORMAT_IEEE_FLOAT
            || (tag == WAVE_FORMAT_EXTENSIBLE && {
                let ext = std::ptr::read_unaligned(fmt_ptr as *const WAVEFORMATEXTENSIBLE);
                let sub = ext.SubFormat;
                sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            });
        let channels = fmt.nChannels.max(1) as usize;
        let bits = fmt.wBitsPerSample;
        let rate = fmt.nSamplesPerSec;
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 200_000, 0, fmt_ptr, None);
        CoTaskMemFree(Some(fmt_ptr as *const _));
        init?;
        if !(float && bits == 32) && bits != 16 {
            crate::log!("audio: unsupported format ({bits}-bit), audio disabled");
            shared.audio_on.store(false, Relaxed);
            return Ok(());
        }

        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        let mut cfg = rate.to_be_bytes().to_vec();
        cfg.push(2);
        if tx.send(S_AUDIO_CONFIG, &cfg).is_err() {
            return Ok(());
        }

        let mut out: Vec<u8> = Vec::with_capacity(16 * 1024);
        while shared.audio_on.load(Relaxed) && !shared.stopped() {
            std::thread::sleep(Duration::from_millis(10));
            while cap.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    out.resize(out.len() + n * 4, 0);
                } else if float {
                    let s = std::slice::from_raw_parts(data as *const f32, n * channels);
                    for f in s.chunks_exact(channels) {
                        let l = f[0];
                        let r = if channels > 1 { f[1] } else { l };
                        out.extend_from_slice(&((l.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                        out.extend_from_slice(&((r.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                    }
                } else {
                    let s = std::slice::from_raw_parts(data as *const i16, n * channels);
                    for f in s.chunks_exact(channels) {
                        out.extend_from_slice(&f[0].to_le_bytes());
                        out.extend_from_slice(&f[if channels > 1 { 1 } else { 0 }].to_le_bytes());
                    }
                }
                cap.ReleaseBuffer(frames)?;
            }
            // Playing another PC's sound here? Don't send it back (it would echo forever).
            if !out.is_empty() && crate::echo::remote_sound_playing() {
                out.clear();
            }
            if !out.is_empty() {
                if tx.send(S_AUDIO, &out).is_err() {
                    break;
                }
                out.clear();
            }
        }
        let _ = client.Stop();
    }
    Ok(())
}
