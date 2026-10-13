//! Sound on Linux: records what the PC is playing from the PulseAudio / PipeWire "monitor"
//! with `parec`, and streams it as 16-bit PCM.

use crate::protocol::{S_AUDIO, S_AUDIO_CONFIG, Sender};
use crate::session::Shared;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

const RATE: u32 = 48_000;

pub fn run(shared: &Shared, tx: &Sender) {
    while !shared.stopped() {
        if !shared.audio_on.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if let Err(e) = capture(shared, tx) {
            crate::log!("audio: {e} (sound needs `parec`, from the pulseaudio-utils package)");
            shared.audio_on.store(false, Relaxed);
        }
    }
}

fn capture(shared: &Shared, tx: &Sender) -> std::io::Result<()> {
    let mut child = Command::new("parec")
        .args(["--device=@DEFAULT_MONITOR@", "--format=s16le", "--channels=2", "--latency-msec=20"])
        .arg(format!("--rate={RATE}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = child.stdout.take().unwrap();
    let mut cfg = RATE.to_be_bytes().to_vec();
    cfg.push(2);
    let mut result = tx.send(S_AUDIO_CONFIG, &cfg);
    // 10 ms of sound per message.
    let mut buf = vec![0u8; RATE as usize / 100 * 4];
    while result.is_ok() && shared.audio_on.load(Relaxed) && !shared.stopped() {
        result = out.read_exact(&mut buf);
        // Playing another PC's sound here? Don't send it back (it would echo forever).
        if result.is_ok() && !crate::echo::remote_sound_playing() {
            result = tx.send(S_AUDIO, &buf);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    // A failed send means the session is ending; that's not a sound problem.
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(std::io::Error::other("parec stopped")),
        _ => Ok(()),
    }
}
