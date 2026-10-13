//! Stops sound going round in circles when two PCs control each other with sound on:
//! while this PC plays sound from another PC, it doesn't send its own sound out.
//! (The window can be a separate process, so the "playing" mark is a file both can see.)

use std::time::{Duration, SystemTime};

fn mark() -> std::path::PathBuf {
    std::env::temp_dir().join("ezrdp-playing-remote-sound")
}

/// Called by the window while it plays another PC's sound (at most about once a second).
pub fn playing_remote_sound() {
    let _ = std::fs::write(mark(), b"");
}

/// True if sound from another PC is playing here right now.
pub fn remote_sound_playing() -> bool {
    std::fs::metadata(mark())
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(2))
}
