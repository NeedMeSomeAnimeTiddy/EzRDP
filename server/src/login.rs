//! What happens right after a phone connects: log in as a trusted phone, or pair a new one.

use crate::crypto::{self, Seal};
use crate::pairing::{self, KeyPair};
use crate::protocol::*;
use crate::state::App;
use std::io;
use std::net::{IpAddr, TcpStream};
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

pub struct Login {
    pub quality: u8,
    pub tx: Seal,
    pub rx: Seal,
    pub device_name: String,
}

pub enum Outcome {
    Session(Box<Login>),
    Paired(String),
    Rejected(&'static str),
}

/// Pairing is only allowed from the home network (or a private VPN), never from the internet.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_private() || v.is_loopback() || v.is_link_local() || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            let s = v.segments()[0];
            v.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

/// `approve(phone name, code)` asks the person at the PC whether to trust a new phone.
pub fn handshake(stream: &mut TcpStream, app: &App, approve: &dyn Fn(&str, u32) -> bool) -> io::Result<Outcome> {
    let mut ns = [0u8; 16];
    getrandom::fill(&mut ns).map_err(io::Error::other)?;
    let mut hello = vec![VERSION];
    hello.extend_from_slice(&app.pc_id);
    hello.extend_from_slice(&ns);
    hello.push(app.paused() as u8);
    hello.extend_from_slice(app.pc_name.as_bytes());
    write_plain(stream, S_HELLO, &hello)?;

    let (kind, body) = read_msg(stream)?;
    match kind {
        C_AUTH if body.len() >= 65 => {
            let (id, nc, proof, quality) = (&body[..16], &body[16..32], &body[32..64], body[64]);
            let device = app.trusted.lock().unwrap().find(id).cloned();
            let Some(device) = device.filter(|d| crypto::verify(&d.key, &ns, nc, proof)) else {
                std::thread::sleep(Duration::from_secs(1));
                write_plain(stream, S_AUTH_RESULT, &[RESULT_DENIED])?;
                return Ok(Outcome::Rejected("unknown phone"));
            };
            if app.paused() {
                write_plain(stream, S_AUTH_RESULT, &[RESULT_PAUSED])?;
                return Ok(Outcome::Rejected("paused"));
            }
            write_plain(stream, S_AUTH_RESULT, &[RESULT_OK])?;
            Ok(Outcome::Session(Box::new(Login {
                quality,
                tx: Seal::new(&crypto::derive(&device.key, b"s2c", &ns, nc)),
                rx: Seal::new(&crypto::derive(&device.key, b"c2s", &ns, nc)),
                device_name: device.name,
            })))
        }
        C_PAIR_START if body.len() >= 32 => {
            let commitment: [u8; 32] = body[..32].try_into().unwrap();
            let name = String::from_utf8_lossy(&body[32..]).to_string();
            let refuse = |stream: &mut TcpStream, code| -> io::Result<Outcome> {
                write_plain(stream, S_PAIR_RESULT, &[code])?;
                Ok(Outcome::Rejected("pairing refused"))
            };
            if app.paused() {
                return refuse(stream, RESULT_PAUSED);
            }
            if !stream.peer_addr().is_ok_and(|a| is_local(a.ip())) {
                return refuse(stream, RESULT_NOT_LOCAL);
            }
            if app.pairing.swap(true, Relaxed) {
                return refuse(stream, RESULT_BUSY);
            }
            let result = pair(stream, app, approve, commitment, &name);
            app.pairing.store(false, Relaxed);
            result
        }
        _ => Ok(Outcome::Rejected("bad hello")),
    }
}

fn pair(
    stream: &mut TcpStream,
    app: &App,
    approve: &dyn Fn(&str, u32) -> bool,
    commitment: [u8; 32],
    name: &str,
) -> io::Result<Outcome> {
    let ours = KeyPair::new();
    write_plain(stream, S_PAIR_KEY, &ours.public)?;
    let (kind, phone_pub) = read_msg(stream)?;
    let key = (kind == C_PAIR_KEY && pairing::commit(&phone_pub) == commitment)
        .then(|| ours.device_key(&phone_pub, &phone_pub, &ours.public))
        .flatten();
    let Some(key) = key else {
        write_plain(stream, S_PAIR_RESULT, &[RESULT_DENIED])?;
        return Ok(Outcome::Rejected("bad pairing key"));
    };
    // Give the person time to walk over and click.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    let code = pairing::code(&phone_pub, &ours.public);
    if !approve(name, code) {
        write_plain(stream, S_PAIR_RESULT, &[RESULT_DENIED])?;
        return Ok(Outcome::Rejected("denied by user"));
    }
    let id = app.trusted.lock().unwrap().add(name, key);
    let mut out = vec![RESULT_OK];
    out.extend_from_slice(&id);
    write_plain(stream, S_PAIR_RESULT, &out)?;
    Ok(Outcome::Paired(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses() {
        for ip in ["192.168.1.5", "10.0.0.2", "127.0.0.1", "100.100.1.1", "fe80::1", "::ffff:192.168.0.9"] {
            assert!(is_local(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["81.2.69.160", "8.8.8.8", "2a00:1450::1"] {
            assert!(!is_local(ip.parse().unwrap()), "{ip}");
        }
    }
}
