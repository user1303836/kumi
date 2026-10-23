//! The host's connection to a bridge: connect on loopback, prove the secret both ways, then requests, events and
//! heartbeats.
use crate::flow::{window_from, Credit, RECV_BUFFER_BYTES};
use daw_bridge::auth::Secret;
use daw_bridge::frame::{Framer, MAX_FRAME_BYTES};
use daw_bridge::handshake::{ClientHandshake, ClientStep, HandshakeError};
use daw_bridge::loopback::peer_allowed;
use daw_bridge::wire::{
    code, nests_within, Event, Frame, Request, Welcome, WireError, CLIENT_HANDSHAKE_TIMEOUT_MS, HEARTBEATS_BEFORE_BUSY, MAX_PAYLOAD_DEPTH,
    MAX_SAFE_INTEGER,
};
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::{broadcast, oneshot};

/// Lines the bridge sends before its welcome: larger than the bridge's own 1024-byte limit, since a welcome carries
/// paths and a project name.
const HANDSHAKE_LINE_BYTES: usize = 64 * 1024;
const READ_CHUNK: usize = 64 * 1024;
/// A host that has only listened for this many bytes sends the bridge a heartbeat frame. A bridge cuts off a host that
/// has let 64 KiB of events and heartbeats pile up (Renoise's does: a send to a full socket may block its GUI), and
/// a quiet host that is reading would otherwise look like one that isn't.
const ACK_AFTER_BYTES: u64 = 16 * 1024;

/// Why a connection could not be made.
#[derive(Debug)]
pub enum ConnectError {
    NotLoopback(SocketAddr),
    /// Nothing listens there, or the connection broke.
    Io(std::io::Error),
    Timeout,
    Handshake(HandshakeError),
    /// The bridge closed the connection before its welcome.
    Closed,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::NotLoopback(address) => write!(f, "{address} is not a loopback address"),
            ConnectError::Io(error) => error.fmt(f),
            ConnectError::Timeout => f.write_str("the bridge didn't finish the handshake in time"),
            ConnectError::Handshake(error) => error.fmt(f),
            ConnectError::Closed => f.write_str("the bridge closed the connection during the handshake"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// Why a request has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The bridge answered with an error (`code` from [`daw_bridge::wire::code`]).
    Bridge { code: String, message: String },
    /// No answer in time: the request may or may not have run.
    Timeout,
    /// The connection is gone.
    Closed,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Bridge { code, message } => write!(f, "{message} ({code})"),
            RequestError::Timeout => f.write_str("the bridge didn't answer in time"),
            RequestError::Closed => f.write_str("the bridge closed the connection"),
        }
    }
}

impl std::error::Error for RequestError {}

type Pending = HashMap<u64, oneshot::Sender<Result<Value, RequestError>>>;

struct Inner {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    last_frame: Mutex<Instant>,
    /// Bytes read since this host last wrote anything.
    unacked: AtomicU64,
    /// What the host lets a bridge that sends within credit (its welcome said so) run ahead of its reading.
    credit_window: Option<u64>,
    /// The most bytes the bridge was ever entitled to send that the host had not read.
    max_unread: AtomicU64,
    /// A host that stops reading, for tests.
    paused: AtomicBool,
    /// Credit the host has counted as given back but not yet written: a grant that could not be written in time (the writer
    /// held, the socket full) is not lost, it is written when it can be, or the bridge would be short of it for good.
    grant_owed: AtomicU64,
    /// Whether a task is writing `grant_owed`.
    grant_writer: AtomicBool,
    closed: AtomicBool,
    events: broadcast::Sender<Event>,
    welcome: Welcome,
}

/// One authenticated connection. Cloning shares it; dropping the last clone doesn't close it (call [`close`]).
///
/// [`close`]: BridgeClient::close
#[derive(Clone)]
pub struct BridgeClient {
    inner: Arc<Inner>,
}

impl BridgeClient {
    /// Connects and authenticates; `client` names this host in the hello (e.g. `daw-mcp-server/0.1.0`).
    pub async fn connect(address: SocketAddr, secret: Secret, client: &str) -> Result<Self, ConnectError> {
        Self::connect_within(address, secret, client, Duration::from_millis(CLIENT_HANDSHAKE_TIMEOUT_MS)).await
    }

    /// `timeout` covers the connect and the handshake together. A connect nobody accepts in time is an `Io` error of kind
    /// `TimedOut` (Windows keeps retrying the SYN to a closed loopback port for about 2 s before it refuses), a handshake
    /// that doesn't finish is `Timeout`.
    pub async fn connect_within(address: SocketAddr, secret: Secret, client: &str, timeout: Duration) -> Result<Self, ConnectError> {
        Self::connect_with(address, secret, client, timeout, RECV_BUFFER_BYTES).await
    }

    /// As [`connect_within`], asking the system for a receive buffer of `recv_buffer` bytes (a test gives a small one).
    ///
    /// [`connect_within`]: BridgeClient::connect_within
    pub async fn connect_with(
        address: SocketAddr,
        secret: Secret,
        client: &str,
        timeout: Duration,
        recv_buffer: u32,
    ) -> Result<Self, ConnectError> {
        if !peer_allowed(&address) {
            return Err(ConnectError::NotLoopback(address));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let socket = if address.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }.map_err(ConnectError::Io)?;
        // Asked for before the connect, so the window the host advertises can grow to it; what the system gave is read back,
        // because that is how much a bridge can send ahead of the host's reading without filling the socket.
        let _ = socket.set_recv_buffer_size(recv_buffer);
        let held = socket.recv_buffer_size().unwrap_or(0) as usize;
        let stream = match tokio::time::timeout_at(deadline, socket.connect(address)).await {
            Ok(stream) => stream.map_err(ConnectError::Io)?,
            Err(_) => {
                return Err(ConnectError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "nothing accepted the connection in time")))
            }
        };
        match tokio::time::timeout_at(deadline, Self::handshake(stream, secret, client, held)).await {
            Ok(result) => result,
            Err(_) => Err(ConnectError::Timeout),
        }
    }

    async fn handshake(stream: TcpStream, secret: Secret, client: &str, held: usize) -> Result<Self, ConnectError> {
        stream.set_nodelay(true).map_err(ConnectError::Io)?;
        let (mut reader, mut writer) = stream.into_split();
        let (mut handshake, hello) = ClientHandshake::start(secret, client);
        writer.write_all(&hello).await.map_err(ConnectError::Io)?;
        let mut framer = Framer::new(HANDSHAKE_LINE_BYTES);
        let mut chunk = vec![0u8; READ_CHUNK];
        let welcome = 'handshake: loop {
            while let Some(line) = framer.next_line() {
                let line = line.map_err(|error| ConnectError::Handshake(HandshakeError::Unexpected(error.code().into())))?;
                match handshake.on_line(&line).map_err(ConnectError::Handshake)? {
                    ClientStep::Send(bytes) => writer.write_all(&bytes).await.map_err(ConnectError::Io)?,
                    ClientStep::Welcome(welcome) => break 'handshake welcome,
                }
            }
            let read = reader.read(&mut chunk).await.map_err(ConnectError::Io)?;
            if read == 0 {
                return Err(ConnectError::Closed);
            }
            framer.feed(&chunk[..read]);
        };
        framer.set_max_bytes(MAX_FRAME_BYTES);
        let (events, _) = broadcast::channel(1024);
        // A bridge that sends within credit starts with its floor: the first grant tops it up to what the receive buffer holds.
        let (credit, first_grant) = match welcome.credit_bytes {
            Some(floor) => {
                let (credit, first) = Credit::new(floor, window_from(held));
                (Some(credit), first)
            }
            None => (None, None),
        };
        let inner = Arc::new(Inner {
            writer: tokio::sync::Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            last_frame: Mutex::new(Instant::now()),
            unacked: AtomicU64::new(0),
            credit_window: credit.as_ref().map(Credit::window),
            max_unread: AtomicU64::new(credit.as_ref().map_or(0, Credit::unread)),
            paused: AtomicBool::new(false),
            grant_owed: AtomicU64::new(0),
            grant_writer: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            events,
            welcome: *welcome,
        });
        if let Some(grant) = first_grant {
            owe_grant(&inner, grant);
        }
        tokio::spawn(read_loop(Arc::downgrade(&inner), reader, framer, credit));
        Ok(Self { inner })
    }

    pub fn welcome(&self) -> &Welcome {
        &self.inner.welcome
    }

    /// The bytes a bridge that sends within credit may run ahead of this host's reading (None for one that sends freely).
    pub fn credit_window(&self) -> Option<u64> {
        self.inner.credit_window
    }

    /// The most bytes such a bridge was ever entitled to send that this host had not read: never over the window.
    pub fn max_unread_credit(&self) -> u64 {
        self.inner.max_unread.load(Ordering::SeqCst)
    }

    /// Stops this host reading the socket (or starts it again), as a host busy elsewhere would. For tests.
    pub fn pause_reading(&self, paused: bool) {
        self.inner.paused.store(paused, Ordering::SeqCst);
    }

    /// Holds the writer for `time`, as a write stuck on a socket without room does. For tests.
    pub async fn hold_writer(&self, time: Duration) {
        let writer = self.inner.writer.lock().await;
        tokio::time::sleep(time).await;
        drop(writer);
    }

    /// Credit counted as given back and not yet written. For tests.
    pub fn grant_owed(&self) -> u64 {
        self.inner.grant_owed.load(Ordering::SeqCst)
    }

    /// Whether both are the same connection (a clone of it).
    pub fn same_connection(&self, other: &BridgeClient) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Whether the bridge has been silent for three heartbeats: the DAW is in a dialog, rendering or stuck. A closed
    /// connection isn't busy, it's gone.
    pub fn busy(&self) -> bool {
        let silent = self.inner.last_frame.lock().unwrap().elapsed();
        !self.is_closed() && silent >= Duration::from_millis(self.inner.welcome.heartbeat_ms.max(1) * HEARTBEATS_BEFORE_BUSY)
    }

    /// The bridge's events from now on.
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RequestError> {
        if self.is_closed() {
            return Err(RequestError::Closed);
        }
        if !nests_within(&params, MAX_PAYLOAD_DEPTH) {
            let message = format!("the params nest deeper than {MAX_PAYLOAD_DEPTH} levels");
            return Err(RequestError::Bridge { code: code::BAD_PARAMS.into(), message });
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        if id > MAX_SAFE_INTEGER {
            return Err(RequestError::Closed);
        }
        let (answer, answered) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, answer);
        // The bridge doesn't start work this host has stopped waiting for: it gets the time the host will give up.
        let deadline_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|now| (now + timeout).as_millis().min(MAX_SAFE_INTEGER as u128) as u64);
        let frame = Frame::Request(Request { id, method: method.into(), params, deadline_ms }).encode();
        // The timeout covers the write too: a bridge that stopped reading fills the socket, and a write that waits for
        // room would hold the writer (and every request behind it) past their timeouts.
        let deadline = tokio::time::Instant::now() + timeout;
        match write_before(&self.inner, &frame, deadline).await {
            Ok(()) => {}
            Err(failure) => {
                self.inner.pending.lock().unwrap().remove(&id);
                return Err(match failure {
                    WriteFailure::Closed => RequestError::Closed,
                    WriteFailure::TimedOut => RequestError::Timeout,
                });
            }
        }
        match tokio::time::timeout_at(deadline, answered).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RequestError::Closed),
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err(RequestError::Timeout)
            }
        }
    }

    pub async fn close(&self) {
        let _ = self.inner.writer.lock().await.shutdown().await;
        fail_all(&self.inner);
    }
}

enum WriteFailure {
    Closed,
    TimedOut,
}

/// Writes one line, waiting for the writer and for room in the socket no longer than `deadline`. A write that timed
/// out midway may have put part of the line out, so the connection can't carry another one and is failed and shut down.
async fn write_before(inner: &Inner, line: &[u8], deadline: tokio::time::Instant) -> Result<(), WriteFailure> {
    let writing = AtomicBool::new(false);
    let written = tokio::time::timeout_at(deadline, async {
        let mut writer = inner.writer.lock().await;
        writing.store(true, Ordering::SeqCst);
        writer.write_all(line).await
    })
    .await;
    match written {
        Ok(Ok(())) => {
            inner.unacked.store(0, Ordering::SeqCst);
            Ok(())
        }
        Ok(Err(_)) => {
            fail_all(inner);
            Err(WriteFailure::Closed)
        }
        Err(_) => {
            if writing.load(Ordering::SeqCst) {
                fail_all(inner);
                let _ = tokio::time::timeout(Duration::from_millis(200), async { inner.writer.lock().await.shutdown().await }).await;
            }
            Err(WriteFailure::TimedOut)
        }
    }
}

/// The host has read `bytes` the bridge may now send again. Written by a task that keeps at it until it is written: a
/// write that waits for the writer past its time is tried again, and what is owed is never forgotten.
fn owe_grant(inner: &Arc<Inner>, bytes: u64) {
    inner.grant_owed.fetch_add(bytes, Ordering::SeqCst);
    start_grant_writer(inner);
}

fn start_grant_writer(inner: &Arc<Inner>) {
    if inner.grant_writer.swap(true, Ordering::SeqCst) {
        return;
    }
    let inner = inner.clone();
    tokio::spawn(async move {
        loop {
            if inner.closed.load(Ordering::SeqCst) {
                inner.grant_writer.store(false, Ordering::SeqCst);
                return;
            }
            let owed = inner.grant_owed.swap(0, Ordering::SeqCst);
            if owed == 0 {
                inner.grant_writer.store(false, Ordering::SeqCst);
                // (something may have been owed since the swap)
                if inner.grant_owed.load(Ordering::SeqCst) > 0 && !inner.grant_writer.swap(true, Ordering::SeqCst) {
                    continue;
                }
                return;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            match write_before(&inner, &Frame::Credit { bytes: owed }.encode(), deadline).await {
                Ok(()) => {}
                Err(WriteFailure::Closed) => {
                    inner.grant_writer.store(false, Ordering::SeqCst);
                    return;
                }
                Err(WriteFailure::TimedOut) => {
                    inner.grant_owed.fetch_add(owed, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    });
}

fn fail_all(inner: &Inner) {
    inner.closed.store(true, Ordering::SeqCst);
    for (_, answer) in inner.pending.lock().unwrap().drain() {
        let _ = answer.send(Err(RequestError::Closed));
    }
}

async fn read_loop(inner: std::sync::Weak<Inner>, mut reader: OwnedReadHalf, mut framer: Framer, mut credit: Option<Credit>) {
    let mut chunk = vec![0u8; READ_CHUNK];
    // What the welcome's read carried beyond the welcome counts as read.
    let mut taken = framer.pending_bytes() as u64;
    loop {
        let Some(live) = inner.upgrade() else { return };
        // Lines already fed (the welcome's read may have carried more).
        while let Some(line) = framer.next_line() {
            let Ok(line) = line else {
                fail_all(&live);
                return;
            };
            *live.last_frame.lock().unwrap() = Instant::now();
            match Frame::parse(&line) {
                Ok(Frame::Response(response)) => {
                    let answer = live.pending.lock().unwrap().remove(&response.id);
                    if let Some(answer) = answer {
                        let result = if response.ok {
                            Ok(response.result.unwrap_or(Value::Null))
                        } else {
                            let error = response.error.unwrap_or_else(|| unreachable!("checked when parsed"));
                            Err(RequestError::Bridge { code: error.code, message: error.message })
                        };
                        let _ = answer.send(result);
                    }
                }
                Ok(Frame::Event(event)) => {
                    let _ = live.events.send(event);
                }
                Ok(Frame::Bye { .. }) => {
                    fail_all(&live);
                    return;
                }
                Ok(_) => {}
                // No bridge sends a frame nested this deep (they refuse to encode one): the frame is dropped alone, and
                // the request it answered times out, instead of the whole connection going.
                Err(WireError::TooDeep) => {}
                Err(_) => {
                    fail_all(&live);
                    return;
                }
            }
        }
        // A bridge that sends within credit holds back once it has used what it was granted: it gets back what was read,
        // in steps of a quarter of the window, as the host takes the bytes in (a host that stops reading stops granting).
        // Bytes count when they come off the socket, whole lines or not: an answer longer than the window could not
        // be read to its end otherwise.
        if let Some(credit) = credit.as_mut() {
            if let Some(grant) = credit.read(std::mem::take(&mut taken)) {
                owe_grant(&live, grant);
            }
            live.max_unread.fetch_max(credit.unread(), Ordering::SeqCst);
        }
        let paused = live.paused.load(Ordering::SeqCst);
        drop(live);
        if paused {
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }
        let read = reader.read(&mut chunk).await;
        let Some(live) = inner.upgrade() else { return };
        match read {
            Ok(0) | Err(_) => {
                fail_all(&live);
                return;
            }
            Ok(count) => {
                taken = count as u64;
                framer.feed(&chunk[..count]);
                // Listening without saying anything: tell the bridge this host is still reading.
                if live.unacked.fetch_add(count as u64, Ordering::SeqCst) + count as u64 >= ACK_AFTER_BYTES {
                    live.unacked.store(0, Ordering::SeqCst);
                    let inner = live.clone();
                    tokio::spawn(async move {
                        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                        let _ = write_before(&inner, &Frame::Heartbeat { ms: 0 }.encode(), deadline).await;
                    });
                }
            }
        }
    }
}
