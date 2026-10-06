//! Login handshake (shared by Windows server and the test server).

use crate::crypto::{self, Seal};
use crate::protocol::*;
use std::net::TcpStream;
use std::time::Duration;

pub struct Login {
    pub quality: u8,
    pub tx: Seal,
    pub rx: Seal,
}

/// Login handshake. Returns the session keys if the phone knows the password.
pub fn authenticate(stream: &mut TcpStream, master: &[u8; 32], salt: &[u8]) -> std::io::Result<Option<Login>> {
    let mut ns = [0u8; 16];
    getrandom::fill(&mut ns).map_err(std::io::Error::other)?;
    let mut hello = vec![VERSION];
    hello.extend_from_slice(salt);
    hello.extend_from_slice(&crypto::ROUNDS.to_be_bytes());
    hello.extend_from_slice(&ns);
    write_plain(stream, S_HELLO, &hello)?;

    let (kind, body) = read_msg(stream)?;
    if kind != C_AUTH || body.len() < 49 {
        return Ok(None);
    }
    let (nc, proof, quality) = (&body[..16], &body[16..48], body[48]);
    let ok = crypto::verify(master, &ns, nc, proof);
    if !ok {
        std::thread::sleep(Duration::from_secs(1)); // slow down guessing
    }
    write_plain(stream, S_AUTH_RESULT, &[ok as u8])?;
    Ok(ok.then(|| Login {
        quality,
        tx: Seal::new(&crypto::derive(master, b"s2c", &ns, nc)),
        rx: Seal::new(&crypto::derive(master, b"c2s", &ns, nc)),
    }))
}

