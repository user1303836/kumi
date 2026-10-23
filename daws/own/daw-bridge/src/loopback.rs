//! Loopback only: the address a bridge binds, the peers it accepts, the port it takes.
use crate::daw::Daw;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};

pub const BIND_ADDRESS: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Whether a peer may talk to a bridge: 127.0.0.1, ::1, or 127.0.0.1 mapped into IPv6. Everything else is closed
/// before a byte is read.
pub fn peer_allowed(peer: &SocketAddr) -> bool {
    match peer.ip() {
        IpAddr::V4(ip) => ip == Ipv4Addr::LOCALHOST,
        IpAddr::V6(ip) => ip.is_loopback() || ip.to_ipv4_mapped() == Some(Ipv4Addr::LOCALHOST),
    }
}

/// The ports a bridge tries, in order: from `start` (the reference file's port) to the end of its DAW's range, then
/// the range's start up to `start`. A `start` outside the range is ignored.
pub fn candidate_ports(daw: Daw, start: Option<u16>) -> Vec<u16> {
    let range: Vec<u16> = daw.ports().collect();
    let first = start.and_then(|port| range.iter().position(|candidate| *candidate == port)).unwrap_or(0);
    range[first..].iter().chain(&range[..first]).copied().collect()
}

/// Binds 127.0.0.1 on the first free candidate port.
pub fn bind(daw: Daw, start: Option<u16>) -> std::io::Result<TcpListener> {
    let mut last = None;
    for port in candidate_ports(daw, start) {
        match TcpListener::bind((BIND_ADDRESS, port)) {
            Ok(listener) => return Ok(listener),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrInUse, "no port to try")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_peers() {
        assert!(peer_allowed(&"127.0.0.1:5".parse().unwrap()));
        assert!(peer_allowed(&"[::1]:5".parse().unwrap()));
        assert!(peer_allowed(&"[::ffff:127.0.0.1]:5".parse().unwrap()));
        assert!(!peer_allowed(&"127.0.0.2:5".parse().unwrap()));
        assert!(!peer_allowed(&"192.168.1.4:5".parse().unwrap()));
        assert!(!peer_allowed(&"[::ffff:10.0.0.1]:5".parse().unwrap()));
        assert!(!peer_allowed(&"0.0.0.0:5".parse().unwrap()));
    }

    #[test]
    fn ports_wrap_within_the_range() {
        assert_eq!(candidate_ports(Daw::Reaper, None)[..2], [9770, 9771]);
        let from = candidate_ports(Daw::Renoise, Some(9788));
        assert_eq!(from, vec![9788, 9789, 9780, 9781, 9782, 9783, 9784, 9785, 9786, 9787]);
        assert_eq!(candidate_ports(Daw::Renoise, Some(9765))[0], 9780);
    }
}
