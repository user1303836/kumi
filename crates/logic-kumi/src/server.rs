//! The loopback server, on its own threads: the handshake and framing come from daw-bridge; `ping`, `status` and
//! `focus` are answered here from the last look at Logic ([`Model`] and the focus cache), so they answer while the
//! helper is busy or gone; `read`, `snapshot` and `focus_body` go to the run loop through the [`Queue`], since the
//! helper is the run loop's alone; events and heartbeats come from the run loop through the [`Hub`]. Copied from the
//! REAPER extension's server: what the two share could move into daw-bridge.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daw_bridge::auth::Secret;
use daw_bridge::files::{self, Discovery, Reference, DISCOVERY_SCHEMA};
use daw_bridge::handshake::{self, Offer, ServerAction, ServerConnection};
use daw_bridge::wire::{self, code, Event, Frame, Identity, Project, Request, MAX_AUTHENTICATED, MAX_UNAUTHENTICATED, PROTOCOL};
use daw_bridge::{loopback, Daw};
use serde_json::{json, Value};

use crate::focus::Focus;
use crate::log;
use crate::logic::Model;
use crate::queue::{Outcome, Queue, Waiter};
use crate::BRIDGE_VERSION;

/// Frames queued for one connection before it counts as stuck and is closed.
const OUTBOX_FRAMES: usize = 4096;
/// How long a socket read waits before its thread looks at the handshake's deadline and whether the server is
/// stopping.
const READ_POLL: Duration = Duration::from_millis(200);
/// A write that makes no progress for this long means the peer stopped reading: the connection is closed.
const STUCK_WRITE: Duration = Duration::from_secs(5);
/// How long stop() waits for each step (readers done, byes written) before it closes the sockets hard.
const STOP_GRACE: Duration = Duration::from_millis(500);
/// A failed attempt to write the discovery file is tried again after this, not on every tick.
const DISCOVERY_RETRY: Duration = Duration::from_secs(5);
/// How often the accept loop looks for a connection, and at whether the server is stopping.
const ACCEPT_POLL: Duration = Duration::from_millis(10);
/// The accept loop waits this long after a failed accept (out of file descriptors, say) rather than spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
/// How often a connection's writer looks whether the server has said everything it will: its outbox may be kept open by
/// a sender nobody dropped, and stopping must not wait for that.
const WRITER_POLL: Duration = Duration::from_millis(50);

/// The methods served: `ping`, `status` and `focus` answered from the last look and never waiting for Logic, the rest
/// read or written through the helper on the run loop; and the events sent.
pub const METHODS: [&str; 20] = [
    "ping",
    "status",
    "focus",
    "read",
    "snapshot",
    "focus_body",
    "change_preview",
    "change_apply",
    "change_restore",
    "transport_stop",
    "transport_play",
    "cursor_set",
    "undo_step_begin",
    "undo_step_end",
    "project_save",
    "bounce",
    "export_tracks",
    "hear",
    "audition",
    "render",
];
pub const EVENTS: [&str; 5] =
    [wire::event::PROJECT, wire::event::TRANSPORT, wire::event::TRACKS, wire::event::SELECTION, wire::event::UNDONE_IN_DAW];
/// The methods the run loop answers.
const QUEUED_METHODS: [&str; 17] = [
    "read",
    "snapshot",
    "focus_body",
    "change_preview",
    "change_apply",
    "change_restore",
    "transport_stop",
    "transport_play",
    "cursor_set",
    "undo_step_begin",
    "undo_step_end",
    "project_save",
    "bounce",
    "export_tracks",
    "hear",
    "audition",
    "render",
];
/// How long a queued request waits for the run loop without a deadline of its own, and the most it may ask for.
const DEFAULT_WAIT: Duration = Duration::from_secs(10);
const MAX_WAIT: Duration = Duration::from_secs(60);
/// The most a listening request (`hear`, `audition`, `render`) may be waited for, whatever its deadline: Logic plays
/// the part through, so its wait is the part's length and a margin (`listen::bound_seconds`) unless the host's call
/// says otherwise.
const MOST_LISTENING: Duration = Duration::from_secs(2 * 3600);
/// Queued requests one connection may have waiting for the run loop at once, each on a thread of its own; past that, a
/// request is waited for on the connection's reader, as every one was before.
const MAX_ANSWERING: usize = 8;
const CLOSING: &str = "the Logic Pro bridge is closing; nothing changed";
/// What a read answers while the helper lacks the Accessibility grant: `failed`, never `busy`, with the status's
/// `permission` naming what to grant.
pub const UNTRUSTED: &str = "Kumi's helper isn't allowed under Accessibility, so nothing of Logic Pro can be read";

/// The connections, shared by the socket threads and the run loop.
#[derive(Clone, Default)]
pub struct Hub {
    inner: Arc<HubInner>,
}

#[derive(Default)]
struct HubInner {
    conns: Mutex<Vec<Arc<Conn>>>,
    /// How long a test holds this server's readers between queuing a welcome and opening the connection to events.
    #[cfg(test)]
    welcome_pause_ms: AtomicU64,
}

/// What the run loop may touch of a connection. The handshake's state machine isn't here: the reader thread owns it,
/// so parsing a big frame never holds anything the run loop (events, heartbeats) waits for.
struct Conn {
    id: u64,
    out: SyncSender<Vec<u8>>,
    stream: TcpStream,
    /// Set once the welcome is queued: events and heartbeats go out only after it.
    authenticated: AtomicBool,
    /// Held while the reader queues the welcome and sets `authenticated`, and while the run loop sends an event or a
    /// heartbeat: nothing goes out before the welcome, and none is missed by a host that has read it.
    opening: Mutex<()>,
    /// The next event's number on this connection (only the run loop sends events).
    next_seq: AtomicU64,
    /// When the connection was accepted: a heartbeat's `ms` counts from here, as the Renoise tool's does.
    opened: Instant,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hub {
    fn conns(&self) -> Vec<Arc<Conn>> {
        lock(&self.inner.conns).clone()
    }

    fn add(&self, conn: Arc<Conn>) {
        lock(&self.inner.conns).push(conn);
    }

    fn remove(&self, id: u64) {
        lock(&self.inner.conns).retain(|c| c.id != id);
    }

    fn counts(&self) -> (usize, usize) {
        let conns = lock(&self.inner.conns);
        let authenticated = conns.iter().filter(|c| c.authenticated.load(Ordering::Acquire)).count();
        (conns.len() - authenticated, authenticated)
    }

    fn send(&self, conn: &Conn, bytes: Vec<u8>) {
        if conn.out.try_send(bytes).is_err() {
            log::line(&format!("connection {} isn't reading; closing it", conn.id));
            self.remove(conn.id);
            let _ = conn.stream.shutdown(Shutdown::Both);
        }
    }

    pub fn connections(&self) -> usize {
        lock(&self.inner.conns).len()
    }

    /// An event to every authenticated connection, numbered per connection.
    pub fn event(&self, name: &str, data: Value) {
        if !wire::nests_within(&data, wire::MAX_PAYLOAD_DEPTH) {
            log::line(&format!("event {name} isn't sent: its data nests deeper than {} levels", wire::MAX_PAYLOAD_DEPTH));
            return;
        }
        for conn in self.conns() {
            let _opening = lock(&conn.opening);
            if conn.authenticated.load(Ordering::Acquire) {
                let seq = conn.next_seq.fetch_add(1, Ordering::Relaxed);
                self.send(&conn, Frame::Event(Event { seq, name: name.into(), data: data.clone() }).encode());
            }
        }
    }

    pub fn heartbeat(&self) {
        for conn in self.conns() {
            let _opening = lock(&conn.opening);
            if conn.authenticated.load(Ordering::Acquire) {
                self.send(&conn, Frame::Heartbeat { ms: conn.opened.elapsed().as_millis() as u64 }.encode());
            }
        }
    }
}

/// What the server needs from the reference file.
#[derive(Debug, Clone)]
pub struct Config {
    pub secret_file: PathBuf,
    pub discovery_dir: PathBuf,
    pub port: Option<u16>,
    pub log_dir: Option<PathBuf>,
    /// Makes each accepted connection's handshake: random nonces, or the conformance vectors' fixed ones.
    pub connection: fn(Secret, u64) -> ServerConnection,
}

impl Config {
    pub fn from_reference(reference: &Reference) -> Config {
        Config {
            secret_file: reference.secret_file.clone().into(),
            discovery_dir: reference.discovery_dir.clone().into(),
            port: reference.port,
            log_dir: reference.log_dir.clone().map(PathBuf::from),
            connection: ServerConnection::new,
        }
    }

    /// Binds 127.0.0.1 on the first free port of Logic's range, from the reference's port.
    pub fn bind(&self) -> io::Result<TcpListener> {
        loopback::bind(Daw::Logic, self.port)
    }
}

pub struct Server {
    config: Config,
    port: u16,
    hub: Hub,
    stopping: Arc<AtomicBool>,
    /// Set by stop() once the byes are queued: a writer with nothing left to write may end.
    flushed: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    threads: Arc<Mutex<Vec<Spawned>>>,
    started_ms: u64,
    discovery: Option<(PathBuf, Option<Project>)>,
    /// When writing the discovery file last failed, and what it said (logged once).
    discovery_failure: Option<(Instant, String)>,
}

/// The connections a stopping server has to say goodbye to.
struct Closing {
    conns: Vec<Arc<Conn>>,
}

/// One connection's threads, and its socket for stop() to close.
struct Spawned {
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    stream: TcpStream,
}

impl Spawned {
    fn finished(&self) -> bool {
        self.reader.is_finished() && self.writer.is_finished()
    }
}

/// Waits until `done` holds or the grace runs out.
fn wait_until(done: impl Fn() -> bool) {
    let grace = Instant::now() + STOP_GRACE;
    while !done() && Instant::now() < grace {
        thread::sleep(Duration::from_millis(5));
    }
}

/// What the run loop shares with the server: the last look at Logic, the focus cache and the queue of work.
#[derive(Clone)]
pub struct Shared2 {
    pub model: Arc<Mutex<Model>>,
    pub focus: Arc<Mutex<Focus>>,
    pub queue: Queue,
}

impl Shared2 {
    pub fn new(model: Arc<Mutex<Model>>) -> Shared2 {
        Shared2 { model, focus: Arc::new(Mutex::new(Focus::default())), queue: Queue::default() }
    }
}

impl Server {
    pub fn start(listener: TcpListener, config: Config, hub: Hub, state: Shared2) -> io::Result<Server> {
        let port = listener.local_addr()?.port();
        let stopping = Arc::new(AtomicBool::new(false));
        let flushed = Arc::new(AtomicBool::new(false));
        let threads = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(Shared {
            config: config.clone(),
            hub: hub.clone(),
            model: state.model,
            focus: state.focus,
            queue: state.queue,
            stopping: stopping.clone(),
            flushed: flushed.clone(),
        });
        let accept = {
            let threads = threads.clone();
            thread::Builder::new()
                .name("kumi-accept".into())
                .spawn(move || crate::guard("accept", (), || accept_loop(listener, shared, threads)))?
        };
        Ok(Server {
            config,
            port,
            hub,
            stopping,
            flushed,
            accept: Some(accept),
            threads,
            started_ms: unix_ms(),
            discovery: None,
            discovery_failure: None,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Writes the discovery file when it's missing or the project changed. The run loop, each tick.
    pub fn publish(&mut self, model: &Model) {
        if self.discovery.as_ref().is_some_and(|(_, project)| *project == model.project) {
            return;
        }
        if self.discovery_failure.as_ref().is_some_and(|(at, _)| at.elapsed() < DISCOVERY_RETRY) {
            return;
        }
        let discovery = Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: Daw::Logic.id().into(),
            pid: model.pid,
            host: loopback::BIND_ADDRESS.to_string(),
            port: self.port,
            protocol: PROTOCOL.into(),
            version: BRIDGE_VERSION.into(),
            daw_version: model.daw_version.clone(),
            resource: model.resource.clone(),
            project: model.project.clone(),
            started_ms: self.started_ms,
        };
        match files::write_discovery(&self.config.discovery_dir, &discovery) {
            Ok(path) => {
                self.discovery = Some((path, model.project.clone()));
                self.discovery_failure = None;
            }
            Err(error) => {
                let said = error.to_string();
                if self.discovery_failure.as_ref().is_none_or(|(_, last)| *last != said) {
                    log::line(&format!("couldn't write the discovery file: {said}"));
                }
                self.discovery_failure = Some((Instant::now(), said));
            }
        }
    }

    /// Removes the discovery file, says bye (`quit` or `shutdown`), closes every connection and waits for the
    /// threads. Bounded: a peer that stopped reading is cut off after a grace.
    pub fn stop(mut self, reason: &str) {
        let closing = self.begin_stop();
        self.finish_stop(closing, reason);
    }

    /// The first step of stopping: the connections as they are now, taken before anything can wake a reader. A reader
    /// that sees `stopping` first leaves the hub and ends, and would otherwise be missed afterwards and never told
    /// goodbye.
    fn begin_stop(&mut self) -> Closing {
        let conns = self.hub.conns();
        self.stopping.store(true, Ordering::Release);
        Closing { conns }
    }

    fn finish_stop(mut self, closing: Closing, reason: &str) {
        let mut conns = closing.conns;
        if let Some((path, _)) = self.discovery.take() {
            let _ = std::fs::remove_file(path);
        }
        if let Some(accept) = self.accept.take() {
            // It looks at `stopping` every few milliseconds. A wait with an end, not a join without one.
            wait_until(|| accept.is_finished());
            if accept.is_finished() {
                let _ = accept.join();
            } else {
                log::line("the accept thread didn't stop; leaving it");
            }
        }
        let spawned = std::mem::take(&mut *lock(&self.threads));
        for conn in std::mem::take(&mut *lock(&self.hub.inner.conns)) {
            if !conns.iter().any(|known| known.id == conn.id) {
                conns.push(conn);
            }
        }
        for conn in &conns {
            let _ = conn.stream.shutdown(Shutdown::Read);
        }
        // Readers see `stopping` within a poll.
        wait_until(|| spawned.iter().all(|s| s.reader.is_finished()));
        for conn in conns.iter().filter(|c| c.authenticated.load(Ordering::Acquire)) {
            let _ = conn.out.try_send(Frame::Bye { reason: reason.into() }.encode());
        }
        // Each writer ends once its outbox (the bye last) is written.
        self.flushed.store(true, Ordering::Release);
        drop(conns);
        wait_until(|| spawned.iter().all(Spawned::finished));
        for s in &spawned {
            let _ = s.stream.shutdown(Shutdown::Both);
        }
        for s in spawned {
            let _ = s.reader.join();
            let _ = s.writer.join();
        }
    }
}

/// What a connection's reader makes of a request.
enum Received {
    /// Answered at once: ping, status and focus from what is known, and refusals.
    Answered(Frame),
    /// Queued for the run loop, its answer waited for by `Shared::wait`.
    Queued(Queued),
}

/// A request in the run loop's queue, and how long its answer is waited for.
struct Queued {
    id: u64,
    waiter: Waiter,
    wait: Duration,
    /// The host gave a deadline: a request never started by then is late, not busy.
    deadline: bool,
}

struct Shared {
    config: Config,
    hub: Hub,
    model: Arc<Mutex<Model>>,
    focus: Arc<Mutex<Focus>>,
    queue: Queue,
    stopping: Arc<AtomicBool>,
    flushed: Arc<AtomicBool>,
}

impl Shared {
    fn identity(&self) -> (Identity, Offer) {
        let identity = lock(&self.model).identity();
        let methods = METHODS.iter().map(|m| m.to_string()).collect();
        let events = EVENTS.iter().map(|e| e.to_string()).collect();
        (identity, Offer { methods, events, ..Offer::default() })
    }

    /// `focus`: the cache, with how old it is and why Logic is busy if it is. Answered here, so it never waits.
    fn focus(&self, id: u64) -> Frame {
        let (busy, permission) = {
            let model = lock(&self.model);
            (model.busy, model.permission)
        };
        if permission.is_some() {
            return Frame::error(id, code::FAILED, UNTRUSTED);
        }
        let focus = lock(&self.focus).clone();
        if focus.refreshed.is_none() {
            return match busy {
                Some(busy) => Frame::error(id, code::BUSY, format!("{}; nothing changed", busy.reason())),
                None => Frame::error(id, code::FAILED, "Logic Pro hasn't been read yet"),
            };
        }
        let mut answer = focus.to_json();
        if let Some(busy) = busy {
            answer["busy"] = busy.code().into();
        }
        Frame::ok(id, answer)
    }

    /// Answers a request at once (ping, status and focus from what is known, or a refusal), or queues it for the run
    /// loop in the order its connection sent it: `wait` then gets the run loop's answer.
    fn receive(&self, request: Request) -> Received {
        let id = request.id;
        match request.method.as_str() {
            "ping" => return Received::Answered(Frame::ok(id, json!({}))),
            "status" => return Received::Answered(Frame::ok(id, lock(&self.model).status())),
            "focus" => return Received::Answered(self.focus(id)),
            _ => {}
        }
        if !QUEUED_METHODS.contains(&request.method.as_str()) {
            let unknown = format!("the Logic Pro bridge has no method {:?}", request.method);
            return Received::Answered(Frame::error(id, code::UNKNOWN_METHOD, unknown));
        }
        if self.stopping.load(Ordering::Acquire) {
            return Received::Answered(Frame::error(id, code::FAILED, CLOSING));
        }
        if lock(&self.model).permission.is_some() {
            return Received::Answered(Frame::error(id, code::FAILED, UNTRUSTED));
        }
        let now = unix_ms();
        // A listening request plays its part through: it waits that long and a margin, the rest the usual.
        let (usual, most) = if crate::listen::METHODS.contains(&request.method.as_str()) {
            let tempo = lock(&self.focus).transport.tempo;
            let bound = Duration::from_secs_f64(crate::listen::bound_seconds(&request.method, &request.params, tempo));
            (bound, MOST_LISTENING)
        } else {
            (DEFAULT_WAIT, MAX_WAIT)
        };
        let wait = match request.deadline_ms {
            Some(deadline) if deadline <= now => {
                let late = "the deadline passed before the bridge got to it; nothing changed";
                return Received::Answered(Frame::error(id, code::DEADLINE, late));
            }
            Some(deadline) => Duration::from_millis(deadline - now),
            None => usual,
        };
        let deadline = request.deadline_ms.is_some();
        let waiter = self.queue.submit(&request.method, request.params);
        Received::Queued(Queued { id, waiter, wait: wait.min(most), deadline })
    }

    /// A queued request's answer: the run loop's, or why there is none (it never started, or didn't finish in time).
    fn wait(&self, queued: Queued) -> Frame {
        let Queued { id, waiter, wait, deadline } = queued;
        match waiter.wait(wait, &self.stopping) {
            Outcome::Answered(Ok(result)) => Frame::ok(id, result),
            Outcome::Answered(Err((code, message))) => Frame::error(id, &code, message),
            Outcome::Cancelled => match lock(&self.model).busy {
                Some(busy) => Frame::error(id, code::BUSY, format!("{}; nothing changed", busy.reason())),
                None if deadline => Frame::error(id, code::DEADLINE, "the deadline passed before the bridge got to it; nothing changed"),
                None => Frame::error(id, code::BUSY, "the bridge didn't get to it in time; nothing changed"),
            },
            Outcome::Uncertain => Frame::error(id, code::UNCERTAIN, "the bridge started this but didn't finish in time"),
            Outcome::Closed => Frame::error(id, code::FAILED, CLOSING),
        }
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, threads: Arc<Mutex<Vec<Spawned>>>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let started = Instant::now();
    // Polled, not blocked in accept: stopping looks at the flag, and doesn't depend on a connection arriving (or being
    // made, and getting through) to wake the thread.
    let _ = listener.set_nonblocking(true);
    loop {
        if shared.stopping.load(Ordering::Acquire) {
            break;
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
                continue;
            }
            Err(_) => {
                // Out of descriptors, say: retrying at once would spin a core.
                thread::sleep(ACCEPT_BACKOFF);
                continue;
            }
        };
        // (What a non-blocking listener accepts may be non-blocking itself.)
        let _ = stream.set_nonblocking(false);
        let Ok(peer) = stream.peer_addr() else { continue };
        if !loopback::peer_allowed(&peer) {
            continue;
        }
        let (waiting, authenticated) = shared.hub.counts();
        if waiting >= MAX_UNAUTHENTICATED || authenticated >= MAX_AUTHENTICATED {
            log::line("a connection was turned away: too many open");
            turn_away(&mut stream);
            continue;
        }
        let secret = match files::read_secret(&shared.config.secret_file) {
            Ok(secret) => secret,
            Err(error) => {
                log::line(&format!("a connection was closed: the secret can't be read ({error})"));
                continue;
            }
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let now_ms = started.elapsed().as_millis() as u64;
        match spawn_connection(id, stream, (shared.config.connection)(secret, now_ms), started, &shared) {
            Ok(spawned) => {
                let mut threads = lock(&threads);
                threads.retain(|s| !s.finished());
                threads.push(spawned);
            }
            Err(error) => log::line(&format!("couldn't start a connection: {error}")),
        }
    }
}

/// Tells a connection over the limits why, and closes it without a reset: what the host already sent is read first,
/// since closing a socket with unread input resets the connection, and the refusal may then be dropped still unread
/// on the other side.
fn turn_away(stream: &mut TcpStream) {
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    let _ = stream.write_all(&handshake::refuse_connection());
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut unread = [0u8; 4096];
    let until = Instant::now() + Duration::from_millis(100);
    while Instant::now() < until && matches!(stream.read(&mut unread), Ok(n) if n > 0) {}
}

fn spawn_connection(id: u64, stream: TcpStream, state: ServerConnection, started: Instant, shared: &Arc<Shared>) -> io::Result<Spawned> {
    stream.set_nodelay(true)?;
    // Writes wake as often as reads do: a blocked send isn't woken by shutting the socket on every OS.
    stream.set_write_timeout(Some(READ_POLL))?;
    let (out, rx) = mpsc::sync_channel::<Vec<u8>>(OUTBOX_FRAMES);
    let conn = Arc::new(Conn {
        id,
        out,
        stream: stream.try_clone()?,
        authenticated: AtomicBool::new(false),
        opening: Mutex::new(()),
        next_seq: AtomicU64::new(1),
        opened: Instant::now(),
    });
    let mut writer = stream.try_clone()?;
    let closer = stream.try_clone()?;
    let stopping = shared.stopping.clone();
    let flushed = shared.flushed.clone();
    let write = thread::Builder::new().name("kumi-write".into()).spawn(move || {
        loop {
            match rx.recv_timeout(WRITER_POLL) {
                Ok(bytes) => {
                    if !write_frame(&mut writer, &bytes, &stopping) {
                        break;
                    }
                }
                // Idle, and the server has said all it will: done, whoever still holds the outbox open.
                Err(RecvTimeoutError::Timeout) if flushed.load(Ordering::Acquire) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = writer.shutdown(Shutdown::Both);
    })?;
    shared.hub.add(conn.clone());
    let reader_shared = shared.clone();
    let read = thread::Builder::new().name("kumi-read".into()).spawn(move || {
        let shared = reader_shared;
        crate::guard("connection", (), || serve(&conn, state, stream, started, &shared));
        shared.hub.remove(conn.id);
    });
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            // Nothing reads this connection: it isn't one, and mustn't count against the limits.
            shared.hub.remove(id);
            return Err(error);
        }
    };
    Ok(Spawned { reader: read, writer: write, stream: closer })
}

/// Writes one frame: false when the socket failed, the peer stopped reading (no progress for [`STUCK_WRITE`]) or the
/// server is stopping and the peer isn't taking bytes.
fn write_frame(stream: &mut TcpStream, mut bytes: &[u8], stopping: &AtomicBool) -> bool {
    let mut stalled: Option<Instant> = None;
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => return false,
            Ok(n) => {
                bytes = &bytes[n..];
                stalled = None;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                if stopping.load(Ordering::Acquire) || stalled.get_or_insert_with(Instant::now).elapsed() >= STUCK_WRITE {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

fn serve(conn: &Arc<Conn>, state: ServerConnection, stream: TcpStream, started: Instant, shared: &Arc<Shared>) {
    let mut answering = Vec::new();
    read_requests(conn, state, stream, started, shared, &mut answering);
    // A stopping server's bye follows the answers still waited for, as it followed a reader's own answer before: each
    // ends within a poll of `stopping`. A connection that went away doesn't wait for them, and frees its place at once.
    if shared.stopping.load(Ordering::Acquire) {
        for answer in answering {
            let _ = answer.join();
        }
    }
}

fn read_requests(
    conn: &Arc<Conn>,
    mut state: ServerConnection,
    mut stream: TcpStream,
    started: Instant,
    shared: &Arc<Shared>,
    answering: &mut Vec<JoinHandle<()>>,
) {
    let mut buf = vec![0u8; 64 * 1024];
    let _ = stream.set_read_timeout(Some(READ_POLL));
    loop {
        if shared.stopping.load(Ordering::Acquire) {
            return;
        }
        let actions = match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => state.on_bytes(&buf[..n], started.elapsed().as_millis() as u64, &mut || shared.identity()),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                state.on_tick(started.elapsed().as_millis() as u64)
            }
            Err(_) => return,
        };
        let mut actions = actions.into_iter().peekable();
        while let Some(action) = actions.next() {
            match action {
                // The welcome, queued and the connection opened to events as one step (see `Conn::opening`). The
                // outbox is all but empty this early, so queuing it never waits.
                ServerAction::Send(bytes) if matches!(actions.peek(), Some(ServerAction::Authenticated)) => {
                    let _opening = lock(&conn.opening);
                    if conn.out.try_send(bytes).is_err() {
                        return;
                    }
                    #[cfg(test)]
                    thread::sleep(Duration::from_millis(shared.hub.inner.welcome_pause_ms.load(Ordering::Relaxed)));
                    conn.authenticated.store(true, Ordering::Release);
                    actions.next();
                }
                ServerAction::Send(bytes) => {
                    if conn.out.send(bytes).is_err() {
                        return;
                    }
                }
                ServerAction::Close(reason) => {
                    log::line(&format!("connection {} closed: {reason:?}", conn.id));
                    return;
                }
                ServerAction::Authenticated => conn.authenticated.store(true, Ordering::Release),
                ServerAction::Request(request) => {
                    let sent = match shared.receive(request) {
                        Received::Answered(frame) => conn.out.send(frame.encode()).is_ok(),
                        Received::Queued(queued) => answer_apart(conn, shared, queued, answering),
                    };
                    if !sent {
                        return;
                    }
                }
            }
        }
    }
}

/// Waits for a queued request's answer on a thread of its own. The run loop may take seconds over a read or a change
/// (an import, notes written row by row), and meanwhile the connection goes on answering ping, status and focus: a host
/// never takes a bridge at work for one gone silent, and Logic Pro for closed. The queue keeps the order requests came
/// in; their answers carry their ids, so they go out as they come. Past `MAX_ANSWERING`, or with no thread to be had,
/// the answer is waited for here. False when the connection is gone.
fn answer_apart(conn: &Arc<Conn>, shared: &Arc<Shared>, queued: Queued, answering: &mut Vec<JoinHandle<()>>) -> bool {
    answering.retain(|answer| !answer.is_finished());
    let queued = if answering.len() < MAX_ANSWERING {
        // Handed over once the thread is there: one that can't be started drops its end, and the request comes back.
        let (hand, handed) = mpsc::sync_channel::<Queued>(1);
        let (waiting, out) = (shared.clone(), conn.out.clone());
        let spawned = thread::Builder::new().name("kumi-answer".into()).spawn(move || {
            if let Ok(queued) = handed.recv() {
                let _ = out.send(waiting.wait(queued).encode());
            }
        });
        match hand.send(queued) {
            Ok(()) => {
                answering.extend(spawned.ok());
                return true;
            }
            Err(mpsc::SendError(queued)) => queued,
        }
    } else {
        queued
    };
    conn.out.send(shared.wait(queued).encode()).is_ok()
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::{Busy, Permission, MAX_NAME, MAX_PATH};
    use daw_bridge::auth::{proof, Role};
    use daw_bridge::handshake::{ClientHandshake, ClientStep, HandshakeError};
    use daw_bridge::owner;
    use std::io::{BufRead, BufReader};

    const VECTOR_SECRET: &str = "kumi-test-secret-0123456789abcdefghijklmnopqrstuvwxyz";

    fn vectors(name: &str) -> Value {
        let path = kumi_testkit::workspace_path("crates/daw-bridge/tests/vectors").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn vector_connection(secret: Secret, now_ms: u64) -> ServerConnection {
        let v = vectors("server.json");
        ServerConnection::with_nonces(secret, v["serverNonce"].as_str().unwrap().into(), v["session"].as_str().unwrap().into(), now_ms)
    }

    fn model() -> Model {
        let mut model = Model::new(4242, "com.apple.logic10", "12.3.1");
        model.project = Some(Project {
            id: Some("a".repeat(32)),
            name: Some("Song".into()),
            path: Some("/m/Song.logicx".into()),
            ..Project::default()
        });
        model
    }

    struct Lab {
        dir: tempfile::TempDir,
        server: Option<Server>,
        hub: Hub,
        model: Arc<Mutex<Model>>,
        focus: Arc<Mutex<Focus>>,
        queue: Queue,
    }

    impl Lab {
        fn start(secret: &str, connection: fn(Secret, u64) -> ServerConnection) -> Lab {
            let dir = tempfile::tempdir().unwrap();
            let bridge = dir.path().join("bridges").join("logic");
            std::fs::create_dir_all(&bridge).unwrap();
            owner::write_atomic(&bridge.join("bridge.secret"), format!("{secret}\n").as_bytes()).unwrap();
            let config = Config {
                secret_file: bridge.join("bridge.secret"),
                discovery_dir: bridge.join("discovery"),
                port: None,
                log_dir: None,
                connection,
            };
            let model = Arc::new(Mutex::new(model()));
            let hub = Hub::default();
            let listener = TcpListener::bind((loopback::BIND_ADDRESS, 0)).unwrap();
            let state = Shared2::new(model.clone());
            let focus = state.focus.clone();
            let queue = state.queue.clone();
            let server = Server::start(listener, config, hub.clone(), state).unwrap();
            Lab { dir, server: Some(server), hub, model, focus, queue }
        }

        fn server(&mut self) -> &mut Server {
            self.server.as_mut().unwrap()
        }

        fn secret_file(&self) -> PathBuf {
            self.dir.path().join("bridges/logic/bridge.secret")
        }

        fn discovery_dir(&self) -> PathBuf {
            self.dir.path().join("bridges/logic/discovery")
        }

        fn connect(&mut self) -> (TcpStream, BufReader<TcpStream>) {
            let stream = TcpStream::connect((loopback::BIND_ADDRESS, self.server().port())).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            (stream, reader)
        }

        fn authenticated(&mut self) -> (TcpStream, BufReader<TcpStream>, Value) {
            self.authenticated_with(VECTOR_SECRET)
        }

        fn authenticated_with(&mut self, secret: &str) -> (TcpStream, BufReader<TcpStream>, Value) {
            let (mut stream, mut reader) = self.connect();
            let (mut handshake, hello) = ClientHandshake::start(Secret::new(secret).unwrap(), "test");
            stream.write_all(&hello).unwrap();
            loop {
                match handshake.on_line(&read_line(&mut reader).unwrap()).unwrap() {
                    ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
                    ClientStep::Welcome(welcome) => return (stream, reader, serde_json::to_value(*welcome).unwrap()),
                }
            }
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            if let Some(server) = self.server.take() {
                server.stop("test over");
            }
        }
    }

    fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end().to_string()),
        }
    }

    fn read_frame_of_any_kind(reader: &mut BufReader<TcpStream>) -> Value {
        serde_json::from_str(&read_line(reader).expect("a frame")).unwrap()
    }

    fn read_frame(reader: &mut BufReader<TcpStream>) -> Value {
        loop {
            let frame: Value = serde_json::from_str(&read_line(reader).expect("a frame")).unwrap();
            if frame["type"] != "heartbeat" {
                return frame;
            }
        }
    }

    fn send(stream: &mut TcpStream, id: u64, method: &str) {
        stream
            .write_all(Frame::Request(Request { id, method: method.into(), params: json!({}), deadline_ms: None }).encode().as_slice())
            .unwrap();
    }

    fn request(stream: &mut TcpStream, reader: &mut BufReader<TcpStream>, id: u64, method: &str) -> Value {
        send(stream, id, method);
        let response = read_frame(reader);
        assert_eq!(response["id"], id);
        response
    }

    /// The next job for the run loop, as the run loop takes it.
    fn next_job(queue: &Queue) -> crate::queue::Job {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(job) = queue.take() {
                return job;
            }
            assert!(Instant::now() < until, "no job came");
            thread::sleep(Duration::from_millis(5));
        }
    }

    // Replays daw-bridge's server vectors against the real sockets: what a host sees from this bridge.
    #[test]
    fn the_wire_matches_daw_bridges_server_vectors() {
        let v = vectors("server.json");
        assert_eq!(v["secret"], VECTOR_SECRET);
        let mut lab = Lab::start(VECTOR_SECRET, vector_connection);
        for case in v["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let (mut stream, mut reader) = lab.connect();
            let accepted = Instant::now();
            let mut closed = false;
            for step in case["steps"].as_array().unwrap() {
                if let Some(hex) = step["inHex"].as_str() {
                    // Bytes meant for after the 3 s deadline: the server's own timer (a poll every 200 ms) may refuse and
                    // close before they would land, and writing to a closed socket resets the connection, which may drop
                    // the refusal still unread. Those bytes are not sent; the replay waits for the refusal instead. The
                    // byte that triggers it is covered by daw-bridge's own replay of these vectors.
                    let mut send = true;
                    if let Some(at) = step["atMs"].as_u64() {
                        let limit = wire::HANDSHAKE_TIMEOUT_MS;
                        send = at < limit;
                        // Real sockets can't land on a millisecond: bytes meant for just before the deadline go well
                        // before it.
                        let at = if at + 150 > limit { limit.saturating_sub(150).min(at) } else { at };
                        thread::sleep(Duration::from_millis(at).saturating_sub(accepted.elapsed()));
                    }
                    if send {
                        // A failed write is fine once the bridge closed; the expectations say what it sent before.
                        let _ = stream.write_all(&hex::decode(hex).unwrap());
                    }
                    thread::sleep(Duration::from_millis(20));
                } else {
                    let tick = Duration::from_millis(step["tickMs"].as_u64().unwrap());
                    // Waiting a minute isn't worth it: past the handshake's 3 s, being still open proves the point.
                    let wait = tick.min(Duration::from_millis(3600));
                    thread::sleep(wait.saturating_sub(accepted.elapsed()));
                }
                for expect in step["expect"].as_array().unwrap() {
                    if let Some(want) = expect.get("send") {
                        let got: Value =
                            serde_json::from_str(&read_line(&mut reader).unwrap_or_else(|| panic!("{name}: no frame"))).unwrap();
                        if want["type"] == "welcome" {
                            for key in ["type", "session", "maxFrameBytes", "heartbeatMs"] {
                                assert_eq!(got[key], want[key], "{name}: {key}");
                            }
                            assert_eq!(got["daw"], "logic");
                            assert!(got["methods"].as_array().unwrap().iter().any(|m| m == "status"), "{name}");
                        } else {
                            assert_eq!(&got, want, "{name}");
                        }
                    } else if let Some(want) = expect.get("request") {
                        // The bridge answers each request in order.
                        let got = read_frame(&mut reader);
                        assert_eq!((got["type"].as_str(), &got["id"], &got["ok"]), (Some("response"), &want["id"], &json!(true)), "{name}");
                    } else if expect.get("close").is_some() {
                        assert_eq!(read_line(&mut reader), None, "{name}: expected the bridge to close");
                        closed = true;
                    }
                }
            }
            if !closed {
                // Still open: a ping answers (after auth) or nothing more arrives (before it).
                if case["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["expect"].as_array().unwrap().iter().any(|e| e.get("authenticated").is_some()))
                {
                    assert_eq!(request(&mut stream, &mut reader, 99, "ping")["ok"], true, "{name}");
                }
            }
        }
    }

    // Replays the handshake vectors over the socket: the challenge's proof for each secret, and the secret files the
    // bridge serves with or closes on.
    #[test]
    fn the_handshake_vectors_hold_over_the_socket_and_a_bad_secret_file_gets_no_challenge() {
        let v = vectors("handshake.json");
        assert_eq!(v["protocol"], PROTOCOL);
        for case in v["cases"].as_array().unwrap() {
            let secret = case["secret"].as_str().unwrap();
            assert_eq!(case["serverNonce"], vectors("server.json")["serverNonce"], "the fixed server nonce is the vectors'");
            let mut lab = Lab::start(secret, vector_connection);
            let (mut stream, mut reader) = lab.connect();
            let hello = format!(
                "{{\"type\":\"hello\",\"protocol\":\"{PROTOCOL}\",\"nonce\":\"{}\",\"client\":\"test\"}}\n",
                case["clientNonce"].as_str().unwrap()
            );
            stream.write_all(hello.as_bytes()).unwrap();
            let challenge = read_frame(&mut reader);
            assert_eq!(challenge["type"], "challenge");
            assert_eq!(challenge["nonce"], case["serverNonce"]);
            assert_eq!(challenge["proof"], case["serverProof"], "the proof over {:?}", case["serverMessage"]);
            let auth = format!("{{\"type\":\"auth\",\"proof\":\"{}\"}}\n", case["clientProof"].as_str().unwrap());
            stream.write_all(auth.as_bytes()).unwrap();
            let welcome = read_frame(&mut reader);
            assert_eq!(welcome["type"], "welcome");
            assert_eq!(welcome["pid"], 4242);
            // The bridge's own proof, with the client's role, is refused: the roles are distinct.
            let (mut stream, mut reader) = lab.connect();
            stream.write_all(hello.as_bytes()).unwrap();
            let _ = read_frame(&mut reader);
            let own = proof(
                &Secret::new(secret).unwrap(),
                Role::Server,
                case["clientNonce"].as_str().unwrap(),
                case["serverNonce"].as_str().unwrap(),
            );
            stream.write_all(format!("{{\"type\":\"auth\",\"proof\":\"{own}\"}}\n").as_bytes()).unwrap();
            assert_eq!(read_frame(&mut reader), json!({"type": "refused", "reason": "auth"}));
        }
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        for case in v["secretFiles"].as_array().unwrap() {
            // The file as the vector spells it: the bridge reads it again for each connection.
            owner::write_atomic(&lab.secret_file(), case["text"].as_str().unwrap().as_bytes()).unwrap();
            let (mut stream, mut reader) = lab.connect();
            let (mut handshake, hello) = ClientHandshake::start(Secret::new(VECTOR_SECRET).unwrap(), "test");
            stream.write_all(&hello).unwrap();
            match case.get("secret") {
                Some(secret) => {
                    assert_eq!(secret, VECTOR_SECRET);
                    let challenge = read_line(&mut reader).unwrap_or_else(|| panic!("{:?}: no challenge", case["text"]));
                    assert!(matches!(handshake.on_line(&challenge), Ok(ClientStep::Send(_))), "{:?}: the proof holds", case["text"]);
                }
                None => {
                    assert_eq!(read_line(&mut reader), None, "{:?}: closed without a frame", case["text"]);
                    assert!(case["error"].is_string());
                }
            }
        }
        // A secret other than the file's can't get in, and the host says so from the bridge's own proof.
        owner::write_atomic(&lab.secret_file(), format!("{}\n", "o".repeat(40)).as_bytes()).unwrap();
        let (mut stream, mut reader) = lab.connect();
        let (mut handshake, hello) = ClientHandshake::start(Secret::new(VECTOR_SECRET).unwrap(), "test");
        stream.write_all(&hello).unwrap();
        assert!(matches!(handshake.on_line(&read_line(&mut reader).unwrap()), Err(HandshakeError::BadProof)));
    }

    #[test]
    fn ping_and_status_answer_from_the_last_look_and_events_follow_in_order() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, welcome) = lab.authenticated();
        assert_eq!(welcome["daw"], "logic");
        assert_eq!(welcome["pid"], 4242);
        assert_eq!(welcome["resource"], "com.apple.logic10");
        assert_eq!(welcome["dawVersion"], "12.3.1");
        assert_eq!(welcome["project"], json!({"id": "a".repeat(32), "name": "Song", "path": "/m/Song.logicx"}));
        assert_eq!(
            welcome["methods"],
            json!([
                "ping",
                "status",
                "focus",
                "read",
                "snapshot",
                "focus_body",
                "change_preview",
                "change_apply",
                "change_restore",
                "transport_stop",
                "transport_play",
                "cursor_set",
                "undo_step_begin",
                "undo_step_end",
                "project_save",
                "bounce",
                "export_tracks",
                "hear",
                "audition",
                "render"
            ])
        );
        assert_eq!(welcome["events"], json!(["project", "transport", "tracks", "selection", "undone_in_daw"]));
        assert_eq!(welcome.get("creditBytes"), None);

        assert_eq!(request(&mut stream, &mut reader, 1, "ping")["result"], json!({}));
        let status = request(&mut stream, &mut reader, 2, "status")["result"].clone();
        assert_eq!(status["project"], welcome["project"]);
        assert_eq!((&status["busy"], &status["permission"]), (&json!(false), &Value::Null));
        assert_eq!(status.as_object().unwrap().keys().collect::<Vec<_>>(), ["project", "busy", "permission"]);
        // The next look says Logic is behind a dialog and the helper lacks its permission: status says so at once.
        {
            let mut model = lock(&lab.model);
            model.busy = Some(Busy::Modal);
            model.permission = Some(Permission::Accessibility);
        }
        let status = request(&mut stream, &mut reader, 3, "status")["result"].clone();
        assert_eq!((&status["busy"], &status["permission"]), (&json!("modal"), &json!("accessibility")));
        // A method the bridge doesn't have is refused by name, not queued.
        let refused = request(&mut stream, &mut reader, 4, "bogus");
        assert_eq!(refused["error"]["code"], code::UNKNOWN_METHOD);
        // Without the Accessibility grant a read fails, in words, and is never busy; so does the focus.
        let refused = request(&mut stream, &mut reader, 5, "read");
        assert_eq!((refused["error"]["code"].as_str(), refused["error"]["message"].as_str()), (Some(code::FAILED), Some(UNTRUSTED)));
        assert_eq!(request(&mut stream, &mut reader, 6, "focus")["error"]["code"], code::FAILED);
        lock(&lab.model).permission = None;
        // Granted, but nothing read yet while Logic is busy: the focus says so; a read waits for the run loop, which
        // answers it through the queue.
        assert_eq!(request(&mut stream, &mut reader, 7, "focus")["error"]["code"], code::BUSY);
        lock(&lab.model).busy = None;
        assert_eq!(request(&mut stream, &mut reader, 8, "focus")["error"]["message"], "Logic Pro hasn't been read yet");
        let queue = lab.queue.clone();
        let answerer = thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(job) = queue.take() {
                    assert_eq!((job.method.as_str(), &job.params), ("read", &json!({})));
                    job.answer(Ok(json!({"items": [], "epoch": "1"})));
                    return;
                }
                assert!(Instant::now() < until, "no job came");
                thread::sleep(Duration::from_millis(5));
            }
        });
        assert_eq!(request(&mut stream, &mut reader, 9, "read")["result"]["epoch"], "1");
        answerer.join().unwrap();
        {
            let mut focus = lock(&lab.focus);
            focus.update(crate::focus::Look { epoch: 3, view: "tracks", ..Default::default() });
        }
        let focus = request(&mut stream, &mut reader, 10, "focus")["result"].clone();
        assert_eq!((focus["epoch"].as_str(), focus["view"].as_str()), (Some("3"), Some("tracks")));
        assert!(focus.get("busy").is_none());
        lock(&lab.model).busy = Some(Busy::Modal);
        assert_eq!(request(&mut stream, &mut reader, 11, "focus")["result"]["busy"], "modal");
        lock(&lab.model).busy = None;

        lab.hub.event("project", json!({"name": "Other"}));
        lab.hub.event("project", Value::Null);
        let first = read_frame(&mut reader);
        let second = read_frame(&mut reader);
        assert_eq!((first["type"].as_str(), first["seq"].as_u64(), first["name"].as_str()), (Some("event"), Some(1), Some("project")));
        assert_eq!(first["data"], json!({"name": "Other"}));
        assert_eq!((second["seq"].as_u64(), &second["data"]), (Some(2), &Value::Null));

        lab.hub.heartbeat();
        let beat: Value = serde_json::from_str(&read_line(&mut reader).unwrap()).unwrap();
        assert_eq!(beat["type"], "heartbeat");
    }

    // What the run loop takes seconds over (an import, notes written row by row) doesn't hold up its connection: ping,
    // status and focus are answered meanwhile. A bridge that went quiet mid-import was taken by its host for one gone,
    // and Kumi told the producer Logic Pro had closed. The run loop gets the requests in the order they were sent, and
    // their answers go out as they come, each with its id.
    #[test]
    fn a_request_waiting_for_the_run_loop_doesnt_hold_up_ping_status_or_focus() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        lock(&lab.focus).update(crate::focus::Look { epoch: 3, view: "tracks", ..Default::default() });
        send(&mut stream, 1, "change_apply");
        send(&mut stream, 2, "read");
        assert_eq!(request(&mut stream, &mut reader, 3, "ping")["result"], json!({}));
        assert!(request(&mut stream, &mut reader, 4, "status")["result"].is_object());
        assert_eq!(request(&mut stream, &mut reader, 5, "focus")["result"]["epoch"], "3");
        let (apply, read) = (next_job(&lab.queue), next_job(&lab.queue));
        assert_eq!((apply.method.as_str(), read.method.as_str()), ("change_apply", "read"));
        read.answer(Ok(json!({"items": [], "epoch": "3"})));
        assert_eq!(read_frame(&mut reader)["id"], 2);
        apply.answer(Ok(json!({"epoch": "4"})));
        let applied = read_frame(&mut reader);
        assert_eq!((applied["id"].as_u64(), &applied["result"]), (Some(1), &json!({"epoch": "4"})));
    }

    // A stopping server says goodbye last: what waits for the run loop is answered first (closing, nothing changed).
    #[test]
    fn a_stopping_server_answers_what_waits_for_the_run_loop_before_its_bye() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        send(&mut stream, 1, "read");
        assert_eq!(request(&mut stream, &mut reader, 2, "ping")["result"], json!({}));
        lab.server.take().unwrap().stop("quit");
        let closing = read_frame(&mut reader);
        assert_eq!((closing["id"].as_u64(), closing["error"]["message"].as_str()), (Some(1), Some(CLOSING)));
        assert_eq!(read_frame(&mut reader), json!({"type": "bye", "reason": "quit"}));
    }

    // A host that lets go of a connection while a request on it waits for the run loop frees its place at once, not when
    // the run loop gets to the request: a host replacing connections mid-import found the bridge full ("too many open").
    #[test]
    fn a_connection_let_go_of_while_a_request_waits_frees_its_place() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        send(&mut stream, 1, "read");
        assert_eq!(request(&mut stream, &mut reader, 2, "ping")["result"], json!({}));
        assert_eq!(lab.hub.counts(), (0, 1));
        drop((stream, reader));
        let until = Instant::now() + Duration::from_secs(2);
        while lab.hub.counts() != (0, 0) {
            assert!(Instant::now() < until, "the connection still holds its place");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_discovery_file_that_cant_be_written_is_tried_again_in_a_while_not_every_tick() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let model = lock(&lab.model).clone();
        let good = lab.discovery_dir();
        // A file where the folder should be.
        let blocked = lab.dir.path().join("blocked");
        std::fs::write(&blocked, "").unwrap();
        lab.server().config.discovery_dir = blocked.join("discovery");
        lab.server().publish(&model);
        assert!(lab.server().discovery.is_none() && lab.server().discovery_failure.is_some());
        // The problem goes away, but the next tick doesn't try yet.
        lab.server().config.discovery_dir = good.clone();
        lab.server().publish(&model);
        assert!(files::scan_discovery(&good, Daw::Logic).is_empty());
        lab.server().discovery_failure.as_mut().unwrap().0 -= DISCOVERY_RETRY;
        lab.server().publish(&model);
        assert_eq!(files::scan_discovery(&good, Daw::Logic).len(), 1);
        assert!(lab.server().discovery_failure.is_none());
    }

    #[test]
    fn the_discovery_file_follows_the_project_and_goes_with_the_server() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let model = lock(&lab.model).clone();
        lab.server().publish(&model);
        let dir = lab.discovery_dir();
        let found = files::scan_discovery(&dir, Daw::Logic);
        assert_eq!(found.len(), 1);
        let discovery = found[0].discovery.as_ref().unwrap();
        assert_eq!(discovery.port, lab.server().port());
        assert_eq!((discovery.daw.as_str(), discovery.pid, discovery.resource.as_str()), ("logic", 4242, "com.apple.logic10"));
        assert_eq!((discovery.version.as_str(), discovery.daw_version.as_str()), (BRIDGE_VERSION, "12.3.1"));
        assert_eq!(discovery.project.as_ref().unwrap().name.as_deref(), Some("Song"));
        assert_eq!(found[0].path.file_name().unwrap().to_string_lossy(), files::discovery_file_name("logic", 4242, "com.apple.logic10"));

        let mut moved = model.clone();
        moved.project = Some(Project::default());
        lab.server().publish(&moved);
        let found = files::scan_discovery(&dir, Daw::Logic);
        assert_eq!(found[0].discovery.as_ref().unwrap().project, Some(Project::default()));

        let (_stream, mut reader, _) = lab.authenticated();
        lab.server.take().unwrap().stop("quit");
        assert_eq!(read_frame(&mut reader), json!({"type": "bye", "reason": "quit"}));
        assert_eq!(read_line(&mut reader), None);
        assert!(files::scan_discovery(&dir, Daw::Logic).is_empty());
    }

    #[test]
    fn a_long_project_title_is_cut_in_the_discovery_file_and_the_welcome() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let window = crate::logic::Window {
            title: "é".repeat(100_000),
            role: "AXWindow".into(),
            main: true,
            document: Some(format!("/m/{}.logicx", "p".repeat(100_000))),
            ..Default::default()
        };
        let model = {
            let mut model = lock(&lab.model);
            model.look(&[window], &[]);
            model.clone()
        };
        lab.server().publish(&model);
        let dir = lab.discovery_dir();
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(files.len(), 1);
        // Far under the 64 KiB a host will read, and still found.
        assert!(files[0].metadata().unwrap().len() < 8 * 1024, "{} bytes", files[0].metadata().unwrap().len());
        let found = files::scan_discovery(&dir, Daw::Logic);
        let project = found[0].discovery.as_ref().unwrap().project.clone().unwrap();
        assert_eq!(project.path.unwrap().chars().count(), MAX_PATH);
        assert!(project.name.unwrap().chars().count() <= MAX_NAME);
        // The welcome carries the same, so a long title can't hold up the handshake either.
        let (_stream, _reader, welcome) = lab.authenticated();
        assert_eq!(welcome["project"]["path"].as_str().unwrap().chars().count(), MAX_PATH);
    }

    #[test]
    fn a_host_whose_reader_left_before_the_stop_took_its_list_is_still_told_goodbye() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (_stream, mut reader, _) = lab.authenticated();
        let mut server = lab.server.take().unwrap();
        let closing = server.begin_stop();
        // The race, made certain: the reader sees `stopping` and leaves the hub before the stop goes any further.
        let until = Instant::now() + Duration::from_secs(5);
        while server.hub.connections() > 0 && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.hub.connections(), 0, "the reader should have left");
        server.finish_stop(closing, "quit");
        assert_eq!(read_frame(&mut reader), json!({"type": "bye", "reason": "quit"}));
        assert_eq!(read_line(&mut reader), None);
    }

    #[test]
    fn an_outbox_somebody_still_holds_open_cant_hold_up_stop() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (_stream, mut reader, _) = lab.authenticated();
        // A sender nobody dropped: the writer's channel never closes by itself.
        let held = lab.hub.conns();
        assert_eq!(held.len(), 1);
        let started = Instant::now();
        lab.server.take().unwrap().stop("quit");
        assert!(started.elapsed() < Duration::from_secs(3), "stop took {:?}", started.elapsed());
        assert_eq!(read_frame(&mut reader), json!({"type": "bye", "reason": "quit"}));
        drop(held);
    }

    #[test]
    fn the_accept_thread_ends_by_itself_once_stopping_with_no_connection_to_wake_it() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let port = lab.server().port();
        let mut server = lab.server.take().unwrap();
        let closing = server.begin_stop();
        let until = Instant::now() + Duration::from_secs(2);
        while !server.accept.as_ref().unwrap().is_finished() && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(server.accept.as_ref().unwrap().is_finished(), "the accept thread is still waiting for a connection");
        server.finish_stop(closing, "quit");
        // And its port is free: nothing listens any more.
        assert!(TcpStream::connect((loopback::BIND_ADDRESS, port)).is_err());
    }

    #[test]
    fn a_peer_that_stopped_reading_doesnt_hold_up_stop() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (_stream, _reader, _) = lab.authenticated();
        // Enough events to fill the socket's buffers, so the writer blocks on a peer that never reads.
        let big = "x".repeat(256 * 1024);
        for _ in 0..64 {
            lab.hub.event("project", json!({"pad": big}));
        }
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        lab.server.take().unwrap().stop("quit");
        assert!(started.elapsed() < Duration::from_secs(3), "stop took {:?}", started.elapsed());
    }

    #[test]
    fn parsing_a_big_frame_never_holds_up_the_run_loops_events() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        // Notes by the hundred thousand: parsed into a Value, the slow shape (strings parse fast).
        let mut bytes = br#"{"type":"request","id":7,"method":"ping","params":{"notes":["#.to_vec();
        while bytes.len() < (8 << 20) {
            bytes.extend_from_slice(br#"{"pitch":60,"vel":100,"start":0.25,"len":0.5},"#);
        }
        bytes.extend_from_slice(b"{}]}}\n");
        let answered = Arc::new(AtomicBool::new(false));
        let done = answered.clone();
        let host = thread::spawn(move || {
            stream.write_all(&bytes).unwrap();
            while read_frame_of_any_kind(&mut reader)["id"] != 7 {}
            done.store(true, Ordering::Release);
            stream
        });
        let (mut worst, mut calls) = (Duration::ZERO, 0);
        while !answered.load(Ordering::Acquire) {
            let started = Instant::now();
            lab.hub.event("project", json!({"call": calls}));
            worst = worst.max(started.elapsed());
            calls += 1;
            thread::sleep(Duration::from_millis(1));
        }
        let _stream = host.join().unwrap();
        // The reader was parsing for most of that time; an event from the run loop must not have waited for it.
        assert!(worst < Duration::from_millis(300), "an event took {worst:?} (of {calls} calls)");
    }

    #[test]
    fn an_event_nested_past_the_limit_is_not_sent_and_takes_no_number() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (_stream, mut reader, _) = lab.authenticated();
        let deep = (0..wire::MAX_PAYLOAD_DEPTH + 1).fold(json!(1), |inner, _| json!([inner]));
        lab.hub.event("project", deep);
        lab.hub.event("project", json!({"count": 2}));
        let event = read_frame(&mut reader);
        assert_eq!((event["seq"].as_u64(), event["data"].clone()), (Some(1), json!({"count": 2})));
    }

    #[test]
    fn a_heartbeat_counts_from_the_connection_not_from_the_server() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        thread::sleep(Duration::from_millis(1200));
        let (_stream, mut reader, _) = lab.authenticated();
        lab.hub.heartbeat();
        let beat: Value = serde_json::from_str(&read_line(&mut reader).unwrap()).unwrap();
        assert_eq!(beat["type"], "heartbeat");
        assert!(beat["ms"].as_u64().unwrap() < 900, "{beat}");
    }

    /// The welcome and the connection's opening to events are one step: an event sent the moment the host has read
    /// its welcome always reaches it, and none goes out before the welcome.
    #[test]
    fn an_event_sent_as_soon_as_the_host_has_its_welcome_reaches_it() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        // The host reads its welcome while the reader is held before it opens the connection: the event still arrives.
        lab.hub.inner.welcome_pause_ms.store(200, Ordering::Relaxed);
        for round in 0..5 {
            let (stream, mut reader, _) = lab.authenticated();
            lab.hub.event("project", json!({"round": round}));
            let event: Value = serde_json::from_str(&read_line(&mut reader).unwrap_or_else(|| panic!("round {round}: no event"))).unwrap();
            assert_eq!((event["type"].as_str(), event["data"]["round"].as_u64()), (Some("event"), Some(round)), "{event}");
            drop((stream, reader));
            wait_until(|| lab.hub.connections() == 0);
        }
    }

    #[test]
    fn connections_over_the_limit_are_told_why() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let waiting: Vec<_> = (0..MAX_UNAUTHENTICATED).map(|_| lab.connect()).collect();
        thread::sleep(Duration::from_millis(100));
        let (_stream, mut reader) = lab.connect();
        assert_eq!(read_frame(&mut reader), json!({"type": "refused", "reason": "connections"}));
        assert_eq!(read_line(&mut reader), None);
        drop(waiting);
    }

    #[test]
    fn a_refused_host_that_already_sent_its_hello_still_reads_why() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let waiting: Vec<_> = (0..MAX_UNAUTHENTICATED).map(|_| lab.connect()).collect();
        thread::sleep(Duration::from_millis(100));
        let (mut stream, mut reader) = lab.connect();
        // The hello is on its way as the refusal is written: closing on it unread would reset the connection.
        let (_, hello) = ClientHandshake::start(Secret::new(VECTOR_SECRET).unwrap(), "test");
        stream.write_all(&hello).unwrap();
        assert_eq!(read_frame(&mut reader), json!({"type": "refused", "reason": "connections"}));
        assert_eq!(read_line(&mut reader), None);
        drop(waiting);
    }

    // Bytes that keep coming reset the read timeout, so only the clock handed to the handshake with each read can end it:
    // this fails if the bridge passes a stale one.
    #[test]
    fn a_peer_that_keeps_sending_is_refused_at_the_deadline_by_the_real_clock() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader) = lab.connect();
        let accepted = Instant::now();
        let refused_at = Arc::new(Mutex::new(None));
        let seen = refused_at.clone();
        // Read as it comes: a write after the bridge closed resets the connection, and a refusal not yet read may be lost.
        let watcher = thread::spawn(move || {
            while let Some(line) = read_line(&mut reader) {
                if line.contains("\"refused\"") {
                    *seen.lock().unwrap() = Some((accepted.elapsed(), line));
                    return;
                }
            }
        });
        let (_, hello) = ClientHandshake::start(Secret::new(VECTOR_SECRET).unwrap(), "test");
        stream.write_all(&hello).unwrap();
        while refused_at.lock().unwrap().is_none() && accepted.elapsed() < Duration::from_secs(6) {
            let _ = stream.write_all(b"\n");
            thread::sleep(Duration::from_millis(100));
        }
        watcher.join().unwrap();
        let (after, line) = refused_at.lock().unwrap().clone().expect("never refused");
        assert!(line.contains("\"timeout\""), "{line}");
        let limit = Duration::from_millis(wire::HANDSHAKE_TIMEOUT_MS);
        assert!(after >= limit - Duration::from_millis(100) && after < limit + Duration::from_millis(1500), "refused after {after:?}");
    }

    #[test]
    fn a_wrong_secret_never_gets_a_welcome() {
        let mut lab = Lab::start(&"o".repeat(40), ServerConnection::new);
        let (mut stream, mut reader) = lab.connect();
        let (mut handshake, hello) = ClientHandshake::start(Secret::new(VECTOR_SECRET).unwrap(), "test");
        stream.write_all(&hello).unwrap();
        let challenge = read_line(&mut reader).unwrap();
        assert!(handshake.on_line(&challenge).is_err(), "the host refuses a bridge that can't prove the secret");
    }
}
