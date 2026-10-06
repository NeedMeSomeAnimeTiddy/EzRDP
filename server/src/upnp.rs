//! Asks the home router to forward the EzRDP port (UPnP), so the phone can connect
//! over the internet with no extra apps or router settings.

use igd_next::{PortMappingProtocol, SearchOptions, search_gateway};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub enum Status {
    /// Reachable from the internet at this address.
    Open(IpAddr),
    /// The router's own internet address is a private/shared one: the internet provider
    /// puts several homes behind one address, so incoming connections can't reach this PC.
    SharedAddress(IpAddr),
    Failed(String),
}

fn is_shared(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private() || v4.is_loopback() || v4.is_unspecified() || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        IpAddr::V6(_) => false,
    }
}

fn map(port: u16, lan: IpAddr) -> Status {
    let mut opts = SearchOptions::default();
    opts.timeout = Some(Duration::from_secs(3));
    let gw = match search_gateway(opts) {
        Ok(g) => g,
        Err(e) => return Status::Failed(format!("no UPnP router found ({e})")),
    };
    let local = SocketAddr::new(lan, port);
    // Some routers only accept permanent mappings (lease 0).
    let added = gw
        .add_port(PortMappingProtocol::TCP, port, local, 3600, "EzRDP")
        .or_else(|_| gw.add_port(PortMappingProtocol::TCP, port, local, 0, "EzRDP"));
    if let Err(e) = added {
        return Status::Failed(format!("router refused to open the port ({e})"));
    }
    match gw.get_external_ip() {
        Ok(ip) if is_shared(ip) => Status::SharedAddress(ip),
        Ok(ip) => Status::Open(ip),
        Err(e) => Status::Failed(format!("router didn't report its internet address ({e})")),
    }
}

/// Opens the port now and keeps it open in the background. Returns the first result.
pub fn open(port: u16, lan_ip: fn() -> Option<IpAddr>) -> Status {
    let Some(lan) = lan_ip() else { return Status::Failed("no network connection".into()) };
    let first = map(port, lan);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(20 * 60));
            if let Some(lan) = lan_ip() {
                let _ = map(port, lan);
            }
        }
    });
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_ranges() {
        assert!(is_shared("192.168.1.1".parse().unwrap()));
        assert!(is_shared("100.72.3.4".parse().unwrap()));
        assert!(is_shared("10.0.0.1".parse().unwrap()));
        assert!(!is_shared("81.2.69.160".parse().unwrap()));
    }
}
