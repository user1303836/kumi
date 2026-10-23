//! A blocking check that a bridge answers: connect, authenticate, ask its status, close. For doctor and setup, which
//! have no async runtime of their own.
use crate::auth::Secret;
use crate::frame::{Framer, MAX_FRAME_BYTES};
use crate::handshake::{ClientHandshake, ClientStep, HandshakeError};
use crate::loopback::peer_allowed;
use crate::wire::{Frame, Request, Welcome};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum ProbeError {
    NotLoopback,
    /// Nothing listens there (or it broke off).
    Io(std::io::Error),
    Timeout,
    Handshake(HandshakeError),
    Closed,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::NotLoopback => f.write_str("not a loopback address"),
            ProbeError::Io(error) => error.fmt(f),
            ProbeError::Timeout => f.write_str("no answer in time"),
            ProbeError::Handshake(error) => error.fmt(f),
            ProbeError::Closed => f.write_str("the connection closed"),
        }
    }
}

impl std::error::Error for ProbeError {}

#[derive(Debug, Clone)]
pub struct Probe {
    pub welcome: Welcome,
    /// `status`'s result, or None when it answered with an error (busy, say).
    pub status: Option<Value>,
}

/// Whether something accepts connections on the port (it may not be a bridge).
pub fn listening(port: u16, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout).is_ok()
}

pub fn probe(address: SocketAddr, secret: &Secret, client: &str, timeout: Duration) -> Result<Probe, ProbeError> {
    if !peer_allowed(&address) {
        return Err(ProbeError::NotLoopback);
    }
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(ProbeError::Io)?;
    stream.set_nodelay(true).map_err(ProbeError::Io)?;
    let (mut handshake, hello) = ClientHandshake::start(secret.clone(), client);
    stream.write_all(&hello).map_err(ProbeError::Io)?;
    let mut framer = Framer::new(64 * 1024);
    let mut lines = Lines { stream: &mut stream, framer: &mut framer, deadline };
    let welcome = loop {
        match handshake.on_line(&lines.next()?).map_err(ProbeError::Handshake)? {
            ClientStep::Send(bytes) => lines.stream.write_all(&bytes).map_err(ProbeError::Io)?,
            ClientStep::Welcome(welcome) => break *welcome,
        }
    };
    lines.framer.set_max_bytes(MAX_FRAME_BYTES);
    let request = Frame::Request(Request { id: 1, method: "status".into(), params: json!({}), deadline_ms: None }).encode();
    lines.stream.write_all(&request).map_err(ProbeError::Io)?;
    let status = loop {
        if let Ok(Frame::Response(response)) = Frame::parse(&lines.next()?) {
            if response.id == 1 {
                break if response.ok { response.result } else { None };
            }
        }
    };
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(Probe { welcome, status })
}

struct Lines<'a> {
    stream: &'a mut TcpStream,
    framer: &'a mut Framer,
    deadline: Instant,
}

impl Lines<'_> {
    fn next(&mut self) -> Result<String, ProbeError> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if let Some(line) = self.framer.next_line() {
                return line.map_err(|error| ProbeError::Handshake(HandshakeError::Unexpected(error.code().into())));
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ProbeError::Timeout);
            }
            self.stream.set_read_timeout(Some(left)).map_err(ProbeError::Io)?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ProbeError::Closed),
                Ok(count) => self.framer.feed(&chunk[..count]),
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Err(ProbeError::Timeout)
                }
                Err(error) => return Err(ProbeError::Io(error)),
            }
        }
    }
}

/// A bridge on a thread, for tests here and in crates without an async runtime: answers `ping` and `status` for
/// every connection until dropped.
#[doc(hidden)]
pub mod testing {
    use crate::auth::Secret;
    use crate::handshake::{Offer, ServerAction, ServerConnection};
    use crate::wire::{Frame, Identity};
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};

    pub struct ThreadBridge {
        pub address: SocketAddr,
    }

    pub fn serve(secret: Secret, identity: Identity) -> ThreadBridge {
        serve_with(secret, identity, serde_json::json!({}))
    }

    /// `serve`, its status carrying `more`'s fields too.
    pub fn serve_with(secret: Secret, identity: Identity, more: serde_json::Value) -> ThreadBridge {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (secret, identity, more) = (secret.clone(), identity.clone(), more.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let clock = std::time::Instant::now();
                    let mut connection = ServerConnection::new(secret, 0);
                    let mut chunk = [0u8; 4096];
                    while let Ok(count) = stream.read(&mut chunk) {
                        if count == 0 {
                            return;
                        }
                        let mut who = || (identity.clone(), Offer::default());
                        for action in connection.on_bytes(&chunk[..count], clock.elapsed().as_millis() as u64, &mut who) {
                            let bytes = match action {
                                ServerAction::Send(bytes) => bytes,
                                ServerAction::Request(request) => {
                                    let mut status = serde_json::to_value(&identity).unwrap();
                                    status["busy"] = false.into();
                                    for (key, value) in more.as_object().into_iter().flatten() {
                                        status[key] = value.clone();
                                    }
                                    Frame::ok(request.id, if request.method == "status" { status } else { serde_json::json!({}) }).encode()
                                }
                                ServerAction::Close(_) => return,
                                ServerAction::Authenticated => continue,
                            };
                            if stream.write_all(&bytes).is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        ThreadBridge { address }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Identity;

    #[test]
    fn a_probe_authenticates_and_reads_the_status() {
        let secret = Secret::new("p".repeat(40)).unwrap();
        let identity = Identity {
            daw: "renoise".into(),
            bridge: "0.1.0".into(),
            daw_version: "3.5.4".into(),
            pid: 9,
            resource: "/prefs/V3.5.4".into(),
            project: None,
        };
        let bridge = testing::serve(secret.clone(), identity.clone());
        let found = probe(bridge.address, &secret, "test", Duration::from_secs(5)).unwrap();
        assert_eq!(found.welcome.identity, identity);
        assert_eq!(found.status.unwrap()["busy"], false);
        assert!(listening(bridge.address.port(), Duration::from_secs(1)));
        let wrong = Secret::new("q".repeat(40)).unwrap();
        assert!(matches!(
            probe(bridge.address, &wrong, "test", Duration::from_secs(5)),
            Err(ProbeError::Handshake(HandshakeError::BadProof))
        ));
        assert!(matches!(probe("10.0.0.1:9780".parse().unwrap(), &secret, "test", Duration::from_secs(1)), Err(ProbeError::NotLoopback)));
    }
}
