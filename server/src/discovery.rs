//! Lets the phone find this PC on the same Wi-Fi without typing an address.
//! Phone broadcasts "EZRDP?" to UDP port 7270; the PC answers with its id and name.

use crate::state::App;
use std::net::UdpSocket;
use std::sync::Arc;

pub const ASK: &[u8] = b"EZRDP?";
pub const ANSWER: &[u8] = b"EZRDP!";

/// Answer: "EZRDP!" version u8, pc_id [16], tcp port u16, flags u8 (1 = paused), name utf8.
pub fn answer(app: &App) -> Vec<u8> {
    let mut out = ANSWER.to_vec();
    out.push(crate::protocol::VERSION);
    out.extend_from_slice(&app.pc_id);
    out.extend_from_slice(&app.port.to_be_bytes());
    out.push(app.paused() as u8);
    out.extend_from_slice(app.pc_name.as_bytes());
    out
}

pub fn run(app: Arc<App>) {
    let sock = match UdpSocket::bind(("0.0.0.0", app.port)) {
        Ok(s) => s,
        Err(e) => {
            crate::log!("discovery unavailable: {e}");
            return;
        }
    };
    let mut buf = [0u8; 64];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        if &buf[..n] == ASK {
            let _ = sock.send_to(&answer(&app), from);
        }
    }
}
