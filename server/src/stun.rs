//! Finds this PC's internet address with a STUN request (the same trick video-call apps use).

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

const SERVERS: [&str; 3] = ["stun.l.google.com:19302", "stun.cloudflare.com:3478", "stun1.l.google.com:19302"];
const MAGIC: u32 = 0x2112_A442;

pub fn public_ip() -> Option<IpAddr> {
    SERVERS.iter().find_map(|s| ask(s))
}

fn ask(server: &str) -> Option<IpAddr> {
    let addr = server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&1u16.to_be_bytes()); // binding request
    req[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    getrandom::fill(&mut req[8..20]).ok()?;
    for _ in 0..2 {
        sock.send_to(&req, addr).ok()?;
        let mut buf = [0u8; 512];
        if let Ok((n, _)) = sock.recv_from(&mut buf)
            && let Some(ip) = parse(&buf[..n], &req[8..20])
        {
            return Some(ip);
        }
    }
    None
}

/// Reads the (XOR-)MAPPED-ADDRESS from a STUN binding response.
fn parse(resp: &[u8], txid: &[u8]) -> Option<IpAddr> {
    if resp.len() < 20 || resp[0..2] != [0x01, 0x01] || &resp[8..20] != txid {
        return None;
    }
    let mut i = 20;
    while i + 4 <= resp.len() {
        let kind = u16::from_be_bytes([resp[i], resp[i + 1]]);
        let len = u16::from_be_bytes([resp[i + 2], resp[i + 3]]) as usize;
        let v = resp.get(i + 4..i + 4 + len)?;
        if (kind == 0x0020 || kind == 0x0001) && len >= 8 && v[1] == 0x01 {
            let mut ip = [v[4], v[5], v[6], v[7]];
            if kind == 0x0020 {
                for (b, m) in ip.iter_mut().zip(MAGIC.to_be_bytes()) {
                    *b ^= m;
                }
            }
            return Some(IpAddr::V4(Ipv4Addr::from(ip)));
        }
        i += 4 + len.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xor_mapped_address() {
        let tx = [7u8; 12];
        let mut r = vec![0x01, 0x01, 0, 12];
        r.extend_from_slice(&MAGIC.to_be_bytes());
        r.extend_from_slice(&tx);
        // XOR-MAPPED-ADDRESS for 81.2.69.160:7270
        let ip = [81u8, 2, 69, 160];
        let m = MAGIC.to_be_bytes();
        r.extend_from_slice(&[0x00, 0x20, 0, 8, 0, 1, 0, 0]);
        r.extend(ip.iter().zip(m).map(|(a, b)| a ^ b));
        assert_eq!(parse(&r, &tx), Some("81.2.69.160".parse().unwrap()));
    }
}
