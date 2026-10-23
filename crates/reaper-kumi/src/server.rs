//! The loopback server, on its own threads: the handshake and framing come from daw-bridge; requests other than
//! `ping` and `status` go to the main thread's queue; events and heartbeats come back from it through the [`Hub`].

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daw_bridge::auth::Secret;
use daw_bridge::files::{self, Discovery, Reference, DISCOVERY_SCHEMA};
use daw_bridge::handshake::{self, Offer, ServerAction, ServerConnection};
use daw_bridge::wire::{self, code, Event, Frame, Identity, Project, Request, MAX_AUTHENTICATED, MAX_UNAUTHENTICATED, PROTOCOL};
use daw_bridge::{loopback, paths, Daw};
use serde_json::{json, Value};

use crate::extension::{Outbox, ProjectSummary, Snapshot, BRIDGE_VERSION};
use crate::log;
use crate::queue::{self, Job, Outcome};
use crate::reaper::Busy;

/// How long a request may wait for the main thread when it names no deadline: no longer than the host's own timeouts
/// (it names a deadline, but another host may not), so work isn't run for a caller that has stopped waiting.
const DEFAULT_WAIT: Duration = Duration::from_secs(2);
/// The longest a request waits for the main thread, whatever deadline it names.
const MAX_WAIT: Duration = Duration::from_secs(60);
/// No tick for this long: the main thread is held (a modal dialog, a long load or render).
const STALL: Duration = Duration::from_millis(1500);
/// Frames queued for one connection before it counts as stuck and is closed.
const OUTBOX_FRAMES: usize = 4096;
/// How long a socket read waits before its thread looks at the handshake's deadline and whether the server is
/// stopping: on Windows a blocking read isn't sure to wake when stop() shuts the socket.
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

const CLOSING: &str = "REAPER is closing; nothing changed";

/// Methods served off the main thread: they answer even while REAPER shows a dialog.
const SOCKET_METHODS: [&str; 3] = ["ping", "status", "focus"];
const MAIN_METHODS: [&str; 19] = [
    "action",
    "quit",
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
    // m4-r-fxenv
    "fx_chain_save",
    // (r-tracks)
    "project_save",
    "track_template_save",
    // M6: catch-up and watch_me
    "digest",
    // (#401) scripts kept as actions
    "script_install",
    "script_remove",
];
const EVENTS: [&str; 5] =
    [wire::event::PROJECT, wire::event::TRANSPORT, wire::event::TRACKS, wire::event::SELECTION, wire::event::UNDONE_IN_DAW];
// M5 listening (m5-reaper): the job's status, its cancel and a preview's stop answer off the main thread, even while it is
// inside REAPER's render for Kumi; the rest run on it. A sweep of scratch copies an earlier session left is told.
const LISTENING_SOCKET_METHODS: [&str; 3] = ["listen_status", "listen_cancel", "audition_stop"];
const LISTENING_MAIN_METHODS: [&str; 7] =
    ["render_start", "capture_start", "audition", "audio_file", "scratch", "deliver_targets", "deliver_start"];
const LISTENING_EVENTS: [&str; 1] = [crate::listening::SWEPT];

/// Where the reference file lives for a resource folder.
pub fn reference_path(resource: &Path) -> PathBuf {
    resource.join("UserPlugins").join(paths::REAPER_REFERENCE)
}

/// The connections, shared by the socket threads and the main thread.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<HubInner>,
}

struct HubInner {
    conns: Mutex<Vec<Arc<Conn>>>,
    /// How long a test holds this server's readers between queuing a welcome and opening the connection to events.
    #[cfg(test)]
    welcome_pause_ms: AtomicU64,
}

/// What the main thread may touch of a connection. The handshake's state machine isn't here: the reader thread owns
/// it, so parsing a big frame never holds anything the main thread (events, heartbeats) waits for.
struct Conn {
    id: u64,
    out: SyncSender<Vec<u8>>,
    stream: TcpStream,
    /// Set once the welcome is queued: events and heartbeats go out only after it.
    authenticated: AtomicBool,
    /// Held while the reader queues the welcome and sets `authenticated`, and while the main thread sends an event or
    /// a heartbeat: nothing goes out before the welcome, and none is missed by a host that has read it.
    opening: Mutex<()>,
    /// The next event's number on this connection (only the main thread sends events).
    next_seq: AtomicU64,
    /// When the connection was accepted: a heartbeat's `ms` counts from here, as the Renoise tool's does.
    opened: Instant,
}

impl Default for Hub {
    fn default() -> Self {
        Hub {
            inner: Arc::new(HubInner {
                conns: Mutex::new(Vec::new()),
                #[cfg(test)]
                welcome_pause_ms: AtomicU64::new(0),
            }),
        }
    }
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
}

impl Outbox for Hub {
    fn event(&self, name: &str, data: Value) {
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

    fn heartbeat(&self) {
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

    /// Binds 127.0.0.1 on the first free port of REAPER's range, from the reference's port.
    pub fn bind(&self) -> io::Result<TcpListener> {
        loopback::bind(Daw::Reaper, self.port)
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
    discovery: Option<(PathBuf, Option<ProjectSummary>)>,
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

impl Server {
    pub fn start(listener: TcpListener, config: Config, hub: Hub, jobs: Sender<Job>, snapshot: Arc<Mutex<Snapshot>>) -> io::Result<Server> {
        let port = listener.local_addr()?.port();
        let stopping = Arc::new(AtomicBool::new(false));
        let flushed = Arc::new(AtomicBool::new(false));
        let threads = Arc::new(Mutex::new(Vec::new()));
        let shared = Shared {
            config: config.clone(),
            hub: hub.clone(),
            jobs: Mutex::new(jobs),
            step: Mutex::new(None),
            snapshot,
            stopping: stopping.clone(),
            flushed: flushed.clone(),
        };
        let shared = Arc::new(shared);
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

    /// Writes the discovery file when it's missing or the project changed. Main thread, each tick.
    pub fn publish(&mut self, snapshot: &Snapshot) {
        if snapshot.daw_version.is_empty() {
            return;
        }
        if self.discovery.as_ref().is_some_and(|(_, project)| *project == snapshot.project) {
            return;
        }
        if self.discovery_failure.as_ref().is_some_and(|(at, _)| at.elapsed() < DISCOVERY_RETRY) {
            return;
        }
        let discovery = Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: Daw::Reaper.id().into(),
            pid: std::process::id(),
            host: loopback::BIND_ADDRESS.to_string(),
            port: self.port,
            protocol: PROTOCOL.into(),
            version: BRIDGE_VERSION.into(),
            daw_version: snapshot.daw_version.clone(),
            resource: snapshot.resource.clone(),
            project: snapshot.project.as_ref().map(project),
            started_ms: self.started_ms,
        };
        match files::write_discovery(&self.config.discovery_dir, &discovery) {
            Ok(path) => {
                self.discovery = Some((path, snapshot.project.clone()));
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

    /// Removes the discovery file, answers waiting requests "closing", says bye (`quit`, `reload` or `shutdown`),
    /// closes every connection and waits for the threads. Bounded: nothing it waits for waits on the main thread, and
    /// a peer that stopped reading is cut off after a grace.
    pub fn stop(mut self, reason: &str) {
        let closing = self.begin_stop();
        self.finish_stop(closing, reason);
    }

    /// The first step of stopping: the connections as they are now, taken before anything can wake a reader. A reader
    /// that sees `stopping` first answers, leaves the hub and ends, and would otherwise be missed afterwards and never
    /// told goodbye. Requests waiting for the main thread are answered "closing" from here on.
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
        // Readers see `stopping` within a poll; a request waiting for the main thread is answered "closing" first.
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

/// The longest project name and path the discovery file and the welcome carry (in characters): hosts skip a discovery
/// file over 64 KiB, and a project title can be as long as its author likes.
pub const MAX_NAME: usize = 256;
pub const MAX_PATH: usize = 1024;

/// Text cut to `max` characters, the last of them an ellipsis when something was cut.
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

fn project(summary: &ProjectSummary) -> Project {
    Project {
        name: (!summary.name.is_empty()).then(|| cut(&summary.name, MAX_NAME)),
        path: (!summary.path.is_empty()).then(|| cut(&summary.path, MAX_PATH)),
        ..Project::default()
    }
}

struct Shared {
    config: Config,
    hub: Hub,
    jobs: Mutex<Sender<Job>>,
    /// The plan's undo step as it was begun: its id, the label it was begun with and the connection that began it, so that
    /// a close asked while REAPER is busy can be accepted for the next free tick and say what it will be named, and a step
    /// whose connection went is closed.
    step: Mutex<Option<OpenStep>>,
    snapshot: Arc<Mutex<Snapshot>>,
    stopping: Arc<AtomicBool>,
    flushed: Arc<AtomicBool>,
}

/// A plan's undo step the server saw begin.
#[derive(Clone)]
struct OpenStep {
    id: String,
    label: String,
    conn: u64,
}

impl Shared {
    fn identity(&self) -> (Identity, Offer) {
        let snapshot = lock(&self.snapshot).clone();
        let identity = Identity {
            daw: Daw::Reaper.id().into(),
            bridge: BRIDGE_VERSION.into(),
            daw_version: snapshot.daw_version,
            pid: std::process::id(),
            resource: snapshot.resource,
            project: snapshot.project.as_ref().map(project),
        };
        let methods = SOCKET_METHODS
            .iter()
            .chain(MAIN_METHODS.iter())
            .chain(LISTENING_SOCKET_METHODS.iter())
            .chain(LISTENING_MAIN_METHODS.iter())
            .map(|m| m.to_string())
            .collect();
        let events = EVENTS.iter().chain(LISTENING_EVENTS.iter()).map(|e| e.to_string()).collect();
        (identity, Offer { methods, events, ..Offer::default() })
    }

    /// Why the main thread isn't ticking, if it isn't: nothing queued would run soon.
    fn stalled(&self) -> Option<Busy> {
        let snapshot = lock(&self.snapshot);
        match snapshot.last_tick {
            Some(tick) if tick.elapsed() > STALL => match snapshot.working {
                // Running a request it claimed, the main thread doesn't tick: that isn't a stall while the request is young.
                // One that runs longer may be inside a dialog it opened: what comes meanwhile is answered busy.
                Some(since) if since.elapsed() <= STALL => None,
                Some(_) => Some(Busy::Working),
                None => Some(snapshot.busy.unwrap_or(Busy::Modal)),
            },
            None => Some(Busy::Starting),
            _ => None,
        }
    }

    /// Why the main thread can't take work now, if it can't.
    fn busy(&self) -> Option<Busy> {
        // M5 listening (m5-reaper): a render Kumi asked for holds every other request from when it is queued.
        self.stalled().or_else(|| lock(&self.snapshot).busy).or_else(|| self.listening().holding().then_some(Busy::KumiRender))
    }

    /// M5 listening (m5-reaper): the jobs and the preview.
    fn listening(&self) -> Arc<crate::listening::Board> {
        lock(&self.snapshot).listening.clone()
    }

    fn status(&self) -> Value {
        let (identity, _) = self.identity();
        let snapshot = lock(&self.snapshot).clone();
        let mut status = serde_json::to_value(identity).unwrap_or_else(|_| json!({}));
        let busy = self.busy();
        status["busy"] = busy.map_or(Value::Bool(false), |b| b.code().into());
        if let Some(busy) = busy {
            status["busyReason"] = busy.reason().into();
        }
        status["lastTickMs"] = snapshot.last_tick.map(|t| t.elapsed().as_millis() as u64).into();
        status["working"] = snapshot.working.is_some().into();
        if let Some(since) = snapshot.working {
            status["workingMs"] = (since.elapsed().as_millis() as u64).into();
        }
        status["connections"] = self.hub.connections().into();
        if let Value::Object(extra) = snapshot.status {
            for (key, value) in extra {
                status[key] = value;
            }
        }
        status
    }

    /// `focus`: what the main thread last polled, with how old that is. Answered here, so it never waits for REAPER.
    fn focus(&self, id: u64) -> Frame {
        let (focus, polled) = {
            let snapshot = lock(&self.snapshot);
            (snapshot.focus.clone(), snapshot.focus_at)
        };
        let busy = self.busy();
        if focus.is_null() {
            return match busy {
                Some(busy) => Frame::error(id, code::BUSY, format!("{}; nothing changed", busy.reason())),
                None => Frame::error(id, code::FAILED, "REAPER has no project open"),
            };
        }
        let mut focus = focus;
        focus["ageMs"] = polled.map(|at| at.elapsed().as_millis() as u64).into();
        if let Some(busy) = busy {
            focus["busy"] = busy.code().into();
        }
        Frame::ok(id, focus)
    }

    /// `undo_step_end` while REAPER is busy: queued for the first tick it can run, answered `{deferred, desc}` with the name
    /// the block will have.
    fn defer_step_end(&self, id: u64, params: Value) -> Frame {
        let step = params["stepId"].as_str().unwrap_or_default().to_string();
        let begun = match lock(&self.step).clone() {
            Some(open) if open.id == step => open.label,
            _ => return Frame::error(id, code::FAILED, "that step isn't open (it ended already, or the project changed under it)"),
        };
        let desc = params["label"].as_str().filter(|label| !label.is_empty()).map_or(begun, str::to_string);
        // Nobody waits for the job's answer: it runs when the main thread is free, and the host has the name already.
        let (job, _waiter) = queue::job("undo_step_end", params);
        if lock(&self.jobs).send(job).is_err() {
            return Frame::error(id, code::FAILED, CLOSING);
        }
        *lock(&self.step) = None;
        Frame::ok(id, json!({"deferred": true, "desc": desc}))
    }

    /// A connection ended. A plan's step it began and never ended is closed at the first free tick: nothing else would end
    /// it (the host died, was killed or closed its connection mid-plan), and a block left open folds every edit the
    /// producer makes into Kumi's point, which an undo of it then takes too.
    fn gone(&self, conn: u64) {
        let step = {
            let mut open = lock(&self.step);
            match open.as_ref() {
                Some(step) if step.conn == conn => open.take(),
                _ => None,
            }
        };
        let Some(step) = step else { return };
        log::line(&format!("connection {conn} ended with a plan's step open: it is closed"));
        // (nobody waits for the answer, as for a close asked while REAPER is busy)
        let (job, _waiter) = queue::job("undo_step_end", json!({"stepId": step.id}));
        let _ = lock(&self.jobs).send(job);
    }

    fn answer(&self, conn: u64, request: Request) -> Frame {
        let id = request.id;
        match request.method.as_str() {
            "ping" => return Frame::ok(id, json!({})),
            "status" => return Frame::ok(id, self.status()),
            "focus" => return self.focus(id),
            // M5 listening (m5-reaper).
            "listen_status" | "listen_cancel" | "audition_stop" => {
                let board = self.listening();
                let answer = match request.method.as_str() {
                    "listen_status" => board.status(&request.params),
                    "listen_cancel" => board.cancel(&request.params, crate::listening::board::CANCEL_WAIT),
                    _ => board.stop_preview(&request.params),
                };
                return match answer {
                    Ok(result) => Frame::ok(id, result),
                    Err((code, message)) => Frame::error(id, &code, message),
                };
            }
            _ => {}
        }
        if self.stopping.load(Ordering::Acquire) {
            return Frame::error(id, code::FAILED, CLOSING);
        }
        let now = unix_ms();
        let wait = match request.deadline_ms {
            Some(deadline) if deadline <= now => {
                return Frame::error(id, code::DEADLINE, "the deadline passed before REAPER got to it; nothing changed")
            }
            Some(deadline) => Duration::from_millis(deadline - now),
            None => DEFAULT_WAIT,
        };
        if !MAIN_METHODS.contains(&request.method.as_str()) && !LISTENING_MAIN_METHODS.contains(&request.method.as_str()) {
            return Frame::error(id, code::UNKNOWN_METHOD, format!("the REAPER bridge has no method {:?}", request.method));
        }
        // REAPER can't take work (a dialog has the main window, it loads, saves or renders, or the main thread isn't
        // ticking): the work would sit until the wait ran out, and then be cancelled. Say so now, and why.
        if let Some(busy) = self.busy() {
            // A plan's step that Kumi stopped because REAPER went busy must still be closed: it is taken for the first free
            // tick, and answered as such, so that the block isn't left open over the producer's own edits.
            if request.method == "undo_step_end" {
                return self.defer_step_end(id, request.params);
            }
            return Frame::error(id, code::BUSY, format!("{}; nothing changed", busy.reason()));
        }
        let begun = (request.method == "undo_step_begin").then(|| request.params["label"].as_str().unwrap_or_default().to_string());
        let ending = (request.method == "undo_step_end").then(|| request.params["stepId"].as_str().unwrap_or_default().to_string());
        let (job, waiter) = queue::job(&request.method, request.params);
        if lock(&self.jobs).send(job).is_err() {
            return Frame::error(id, code::FAILED, CLOSING);
        }
        // Whatever stops REAPER taking work while the request waits ends the wait: nothing would run it until that is over.
        // Once the main thread runs it, the request waits for its own answer.
        let claimed = waiter.claimed();
        let cannot_run = || !claimed() && self.busy().is_some();
        match waiter.wait_unless(wait.min(MAX_WAIT), &self.stopping, &cannot_run) {
            Outcome::Answered(Ok(result)) => {
                if let (Some(label), Some(step)) = (begun, result["stepId"].as_str()) {
                    *lock(&self.step) = Some(OpenStep { id: step.to_string(), label, conn });
                }
                if ending.is_some() {
                    *lock(&self.step) = None;
                }
                Frame::ok(id, result)
            }
            Outcome::Answered(Err((code, message))) => Frame::error(id, &code, message),
            Outcome::Cancelled => match self.busy() {
                Some(busy) => Frame::error(id, code::BUSY, format!("{}; nothing changed", busy.reason())),
                None if request.deadline_ms.is_some() => {
                    Frame::error(id, code::DEADLINE, "the deadline passed before REAPER got to it; nothing changed")
                }
                None => Frame::error(id, code::BUSY, "REAPER didn't get to it in time; nothing changed"),
            },
            Outcome::Uncertain => {
                Frame::error(id, code::UNCERTAIN, "REAPER started this but didn't finish in time; it may or may not have happened")
            }
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
/// since closing a socket with unread input resets the connection, and Windows then drops the refusal still unread
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
        shared.gone(conn.id);
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

fn serve(conn: &Arc<Conn>, mut state: ServerConnection, mut stream: TcpStream, started: Instant, shared: &Shared) {
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
                    let response = shared.answer(conn.id, request);
                    if conn.out.send(response.encode()).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_bridge::handshake::{ClientHandshake, ClientStep};
    use daw_bridge::owner;
    use std::io::{BufRead, BufReader};

    const VECTOR_SECRET: &str = "kumi-test-secret-0123456789abcdefghijklmnopqrstuvwxyz";

    fn vectors() -> Value {
        let path = kumi_testkit::workspace_path("daws/own/daw-bridge/tests/vectors/server.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn vector_connection(secret: Secret, now_ms: u64) -> ServerConnection {
        let v = vectors();
        ServerConnection::with_nonces(secret, v["serverNonce"].as_str().unwrap().into(), v["session"].as_str().unwrap().into(), now_ms)
    }

    struct Lab {
        dir: tempfile::TempDir,
        server: Option<Server>,
        hub: Hub,
        snapshot: Arc<Mutex<Snapshot>>,
        jobs: mpsc::Receiver<Job>,
        tx: mpsc::Sender<Job>,
    }

    impl Lab {
        fn start(secret: &str, connection: fn(Secret, u64) -> ServerConnection) -> Lab {
            let dir = tempfile::tempdir().unwrap();
            let bridge = dir.path().join("bridges").join("reaper");
            std::fs::create_dir_all(&bridge).unwrap();
            owner::write_atomic(&bridge.join("bridge.secret"), format!("{secret}\n").as_bytes()).unwrap();
            let config = Config {
                secret_file: bridge.join("bridge.secret"),
                discovery_dir: bridge.join("discovery"),
                port: None,
                log_dir: None,
                connection,
            };
            let snapshot = Arc::new(Mutex::new(Snapshot {
                daw_version: "7.82/macOS-arm64".into(),
                resource: dir.path().join("REAPER").to_string_lossy().into_owned(),
                project: Some(ProjectSummary { name: "Song.rpp".into(), path: "/m/Song.rpp".into() }),
                last_tick: Some(Instant::now()),
                busy: None,
                status: json!({"tracks": 3}),
                focus: Value::Null,
                focus_at: None,
                working: None,
                listening: Default::default(),
            }));
            let hub = Hub::default();
            let (tx, jobs) = queue::channel();
            let listener = TcpListener::bind((loopback::BIND_ADDRESS, 0)).unwrap();
            let server = Server::start(listener, config, hub.clone(), tx.clone(), snapshot.clone()).unwrap();
            Lab { dir, server: Some(server), hub, snapshot, jobs, tx }
        }

        fn server(&mut self) -> &mut Server {
            self.server.as_mut().unwrap()
        }

        fn connect(&mut self) -> (TcpStream, BufReader<TcpStream>) {
            let stream = TcpStream::connect((loopback::BIND_ADDRESS, self.server().port())).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            (stream, reader)
        }

        fn authenticated(&mut self) -> (TcpStream, BufReader<TcpStream>, Value) {
            let (mut stream, mut reader) = self.connect();
            let (mut handshake, hello) = ClientHandshake::start(Secret::new(VECTOR_SECRET).unwrap(), "test");
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

    fn request(stream: &mut TcpStream, reader: &mut BufReader<TcpStream>, id: u64, method: &str) -> Value {
        stream
            .write_all(Frame::Request(Request { id, method: method.into(), params: json!({}), deadline_ms: None }).encode().as_slice())
            .unwrap();
        let response = read_frame(reader);
        assert_eq!(response["id"], id);
        response
    }

    // Replays daw-bridge's server vectors against the real sockets: what a host sees from this extension.
    #[test]
    fn the_wire_matches_daw_bridges_server_vectors() {
        let v = vectors();
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
                    // close before they would land, and writing to a closed socket resets the connection, which Windows
                    // answers by dropping the refusal still unread. Those bytes are not sent; the replay waits for the
                    // refusal instead. The byte that triggers it is covered by daw-bridge's own replay of these vectors.
                    let mut send = true;
                    if let Some(at) = step["atMs"].as_u64() {
                        let limit = wire::HANDSHAKE_TIMEOUT_MS;
                        send = at < limit;
                        // Real sockets can't land on a millisecond (Windows' timers are 15 ms apart): bytes meant for
                        // just before the deadline go well before it.
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
                            assert_eq!(got["daw"], "reaper");
                            assert!(got["methods"].as_array().unwrap().iter().any(|m| m == "quit"), "{name}");
                        } else {
                            assert_eq!(&got, want, "{name}");
                        }
                    } else if let Some(want) = expect.get("request") {
                        // The extension answers each request in order.
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

    #[test]
    fn requests_reach_the_main_thread_and_events_follow_in_order() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, welcome) = lab.authenticated();
        assert_eq!(welcome["daw"], "reaper");
        assert_eq!(welcome["project"], json!({"name": "Song.rpp", "path": "/m/Song.rpp"}));
        assert_eq!(welcome["events"], json!(["project", "transport", "tracks", "selection", "undone_in_daw", "scratch_swept"]));

        let jobs = std::mem::replace(&mut lab.jobs, mpsc::channel().1);
        let main = thread::spawn(move || {
            let job = jobs.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(job.claim());
            assert_eq!(job.method, "quit");
            job.answer(Ok(json!({"quitting": true})));
        });
        assert_eq!(request(&mut stream, &mut reader, 1, "quit")["result"], json!({"quitting": true}));
        main.join().unwrap();

        lab.hub.event("tracks", json!({"count": 4}));
        lab.hub.event("selection", json!({"tracks": []}));
        let first = read_frame(&mut reader);
        let second = read_frame(&mut reader);
        assert_eq!((first["type"].as_str(), first["seq"].as_u64(), first["name"].as_str()), (Some("event"), Some(1), Some("tracks")));
        assert_eq!(first["data"], json!({"count": 4}));
        assert_eq!((second["seq"].as_u64(), second["name"].as_str()), (Some(2), Some("selection")));

        lab.hub.heartbeat();
        let beat: Value = serde_json::from_str(&read_line(&mut reader).unwrap()).unwrap();
        assert_eq!(beat["type"], "heartbeat");
    }

    #[test]
    fn reads_are_listed_in_the_welcome_and_reach_the_main_thread() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, welcome) = lab.authenticated();
        let methods: Vec<&str> = welcome["methods"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(["read", "snapshot", "focus_body"].iter().all(|m| methods.contains(m)), "{methods:?}");
        let jobs = std::mem::replace(&mut lab.jobs, mpsc::channel().1);
        let main = thread::spawn(move || {
            for _ in 0..2 {
                let job = jobs.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(job.claim());
                let method = job.method.clone();
                job.answer(Ok(json!({"method": method})));
            }
        });
        assert_eq!(request(&mut stream, &mut reader, 1, "read")["result"], json!({"method": "read"}));
        assert_eq!(request(&mut stream, &mut reader, 2, "snapshot")["result"], json!({"method": "snapshot"}));
        main.join().unwrap();
    }

    #[test]
    fn focus_is_answered_off_the_main_thread_with_its_age_and_why_it_may_be_stale() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, welcome) = lab.authenticated();
        assert!(welcome["methods"].as_array().unwrap().iter().any(|m| m == "focus"));
        // Nothing polled yet and no project: not an empty answer.
        let none = request(&mut stream, &mut reader, 1, "focus");
        assert_eq!(none["error"]["code"], code::FAILED);
        {
            let mut snapshot = lab.snapshot.lock().unwrap();
            snapshot.focus = json!({"epoch": "e-1", "view": "arrange", "changed": 4});
            snapshot.focus_at = Some(Instant::now() - Duration::from_millis(120));
        }
        let focus = request(&mut stream, &mut reader, 2, "focus")["result"].clone();
        assert_eq!((&focus["epoch"], &focus["view"], &focus["changed"]), (&json!("e-1"), &json!("arrange"), &json!(4)));
        assert!(focus["ageMs"].as_u64().unwrap() >= 120);
        assert_eq!(focus.get("busy"), None);
        // The main thread stops ticking: the last focus still answers, and says why it is old.
        lab.snapshot.lock().unwrap().last_tick = Some(Instant::now() - Duration::from_secs(5));
        let focus = request(&mut stream, &mut reader, 3, "focus")["result"].clone();
        assert_eq!(focus["busy"], "modal");
        assert_eq!(focus["view"], "arrange");
    }

    #[test]
    fn a_dialog_over_the_main_window_refuses_requests_at_once_while_the_main_thread_still_ticks() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        // The extension ticks (REAPER's timer runs inside the dialog's loop) and says a dialog has the window.
        lab.snapshot.lock().unwrap().busy = Some(Busy::Modal);
        let started = Instant::now();
        let response = request(&mut stream, &mut reader, 1, "quit");
        assert_eq!(response["error"]["code"], code::BUSY);
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.contains("dialog") && message.contains("nothing changed"), "{message}");
        assert!(started.elapsed() < Duration::from_millis(500), "queued behind the dialog instead of refused");
        // status says why, and goes on answering.
        let status = request(&mut stream, &mut reader, 2, "status")["result"].clone();
        assert_eq!((&status["busy"], status["busyReason"].as_str().is_some_and(|r| r.contains("dialog"))), (&json!("modal"), true));
        // A load, a save and a render hold work the same way, and each says its own reason: not "a dialog may be open".
        for (n, (busy, reason)) in [(Busy::Loading, "loading or saving"), (Busy::Rendering, "rendering")].into_iter().enumerate() {
            lab.snapshot.lock().unwrap().busy = Some(busy);
            let started = Instant::now();
            // The host's own wait, far longer than the test: a read that waited it out would end as a deadline.
            let frame = Request {
                id: 3 + n as u64,
                method: "read".into(),
                params: json!({"kind": "track"}),
                deadline_ms: Some(unix_ms() + 15_000),
            };
            stream.write_all(&Frame::Request(frame).encode()).unwrap();
            let response = read_frame(&mut reader);
            assert_eq!(response["error"]["code"], code::BUSY, "{response}");
            let message = response["error"]["message"].as_str().unwrap();
            assert!(message.contains(reason) && !message.contains("dialog") && message.contains("nothing changed"), "{message}");
            assert!(started.elapsed() < Duration::from_millis(500), "{busy:?}: queued instead of refused");
            let status = request(&mut stream, &mut reader, 10 + n as u64, "status")["result"].clone();
            assert_eq!(status["busy"], busy.code());
        }
    }

    fn send(stream: &mut TcpStream, id: u64, method: &str, params: Value) {
        stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
    }

    #[test]
    fn a_plans_step_asked_closed_while_reaper_is_busy_is_taken_for_the_first_free_tick() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        let jobs = std::mem::replace(&mut lab.jobs, mpsc::channel().1);
        let main = thread::spawn(move || {
            let job = jobs.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(job.claim());
            assert_eq!(job.method, "undo_step_begin");
            job.answer(Ok(json!({"stepId": "s1"})));
            jobs
        });
        send(&mut stream, 1, "undo_step_begin", json!({"label": "Kumi: Kumi (k1111)"}));
        assert_eq!(read_frame(&mut reader)["result"], json!({"stepId": "s1"}));
        let jobs = main.join().unwrap();

        // REAPER goes busy: a change is refused, the step's close is not.
        lab.snapshot.lock().unwrap().busy = Some(Busy::Rendering);
        send(&mut stream, 2, "change_apply", json!({}));
        assert_eq!(read_frame(&mut reader)["error"]["code"], code::BUSY);
        let started = Instant::now();
        send(&mut stream, 3, "undo_step_end", json!({"stepId": "s1"}));
        let response = read_frame(&mut reader);
        assert_eq!(response["result"], json!({"deferred": true, "desc": "Kumi: Kumi (k1111)"}), "{response}");
        assert!(started.elapsed() < Duration::from_millis(500), "waited for REAPER instead of taking the close");
        // It is in the queue, for the first tick that can run it.
        let job = jobs.try_recv().expect("the close is queued");
        assert_eq!((job.method.as_str(), job.params["stepId"].as_str()), ("undo_step_end", Some("s1")));
        // Asked again, there is no step to close; nor is there one it never began.
        send(&mut stream, 4, "undo_step_end", json!({"stepId": "s1"}));
        assert_eq!(read_frame(&mut reader)["error"]["code"], code::FAILED);
        send(&mut stream, 5, "undo_step_end", json!({"stepId": "s9"}));
        assert_eq!(read_frame(&mut reader)["error"]["code"], code::FAILED);
        assert!(jobs.try_recv().is_err(), "nothing else was queued");
    }

    #[test]
    fn a_close_with_a_new_label_is_deferred_under_that_label() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        let jobs = std::mem::replace(&mut lab.jobs, mpsc::channel().1);
        let main = thread::spawn(move || {
            let job = jobs.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(job.claim());
            job.answer(Ok(json!({"stepId": "s7"})));
            jobs
        });
        send(&mut stream, 1, "undo_step_begin", json!({"label": "Kumi: Kumi (k2222)"}));
        read_frame(&mut reader);
        let jobs = main.join().unwrap();
        lab.snapshot.lock().unwrap().busy = Some(Busy::Modal);
        send(&mut stream, 2, "undo_step_end", json!({"stepId": "s7", "label": "Kumi: Vocal volume −4.5 dB and 1 more (k2222)"}));
        assert_eq!(read_frame(&mut reader)["result"], json!({"deferred": true, "desc": "Kumi: Vocal volume −4.5 dB and 1 more (k2222)"}));
        assert_eq!(jobs.try_recv().unwrap().params["label"], "Kumi: Vocal volume −4.5 dB and 1 more (k2222)");
    }

    #[test]
    fn a_plans_step_whose_connection_goes_is_closed_and_one_another_connection_began_isnt() {
        // A host that dies (or closes) mid-plan leaves nobody to end its step: REAPER's block stayed open for two minutes,
        // folding the producer's edits into Kumi's point.
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        let (mut other, mut other_reader, _) = lab.authenticated();
        let jobs = std::mem::replace(&mut lab.jobs, mpsc::channel().1);
        let main = thread::spawn(move || {
            let job = jobs.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(job.claim());
            assert_eq!(job.method, "undo_step_begin");
            job.answer(Ok(json!({"stepId": "s3"})));
            jobs
        });
        send(&mut stream, 1, "undo_step_begin", json!({"label": "Kumi: Kumi (k3333)"}));
        assert_eq!(read_frame(&mut reader)["result"], json!({"stepId": "s3"}));
        let jobs = main.join().unwrap();
        // The other connection goes: the step isn't its own.
        assert_eq!(request(&mut other, &mut other_reader, 2, "ping")["result"], json!({}));
        drop((other, other_reader));
        thread::sleep(Duration::from_millis(300));
        assert!(jobs.try_recv().is_err(), "nothing closed for a connection that began no step");
        // The one that began it goes: its close is queued for the first free tick.
        drop((stream, reader));
        let job = jobs.recv_timeout(Duration::from_secs(5)).expect("the step's close is queued");
        assert_eq!((job.method.as_str(), job.params["stepId"].as_str()), ("undo_step_end", Some("s3")));
        assert!(jobs.try_recv().is_err(), "once");
    }

    #[test]
    fn the_main_thread_running_a_slow_request_is_working_and_that_request_is_answered_while_others_are_told_busy() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        let jobs = std::mem::replace(&mut lab.jobs, mpsc::channel().1);
        let snapshot = lab.snapshot.clone();
        let main = thread::spawn(move || {
            let job = jobs.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(job.claim());
            // A plug-in loads, or the request opened a licence prompt: the main thread doesn't tick, and says it is working.
            {
                let mut snapshot = snapshot.lock().unwrap();
                snapshot.working = Some(Instant::now());
                snapshot.last_tick = Some(Instant::now() - Duration::from_secs(5));
            }
            thread::sleep(Duration::from_millis(2600));
            {
                let mut snapshot = snapshot.lock().unwrap();
                snapshot.working = None;
                snapshot.last_tick = Some(Instant::now());
            }
            job.answer(Ok(json!({"added": true})));
        });
        let frame = Request { id: 1, method: "change_apply".into(), params: json!({}), deadline_ms: Some(unix_ms() + 15_000) };
        stream.write_all(&Frame::Request(frame).encode()).unwrap();
        // While it is young, status (on another connection) says it works and not that REAPER is stuck.
        thread::sleep(Duration::from_millis(800));
        let (mut other, mut other_reader, _) = lab.authenticated();
        let status = request(&mut other, &mut other_reader, 2, "status")["result"].clone();
        assert_eq!((&status["busy"], &status["working"]), (&json!(false), &json!(true)));
        // Past the stall limit, what else comes is answered busy at once, not after its deadline.
        thread::sleep(Duration::from_millis(1000));
        let status = request(&mut other, &mut other_reader, 3, "status")["result"].clone();
        assert_eq!((&status["busy"], &status["working"]), (&json!("working"), &json!(true)));
        assert!(status["workingMs"].as_u64().unwrap() >= 1500, "{status}");
        let started = Instant::now();
        let read = Request { id: 4, method: "read".into(), params: json!({"kind": "track"}), deadline_ms: Some(unix_ms() + 15_000) };
        other.write_all(&Frame::Request(read).encode()).unwrap();
        let refused = read_frame(&mut other_reader);
        assert_eq!(refused["error"]["code"], code::BUSY, "{refused}");
        assert!(refused["error"]["message"].as_str().unwrap().contains("long request"), "{refused}");
        assert!(started.elapsed() < Duration::from_millis(500), "waited {:?}", started.elapsed());
        // The request that runs is answered when it ends.
        let response = read_frame(&mut reader);
        assert_eq!(response["result"], json!({"added": true}), "{response}");
        main.join().unwrap();
        // The main thread that stops ticking without a job to run is stalled, as before.
        lab.snapshot.lock().unwrap().last_tick = Some(Instant::now() - Duration::from_secs(5));
        let status = request(&mut other, &mut other_reader, 5, "status")["result"].clone();
        assert_eq!(status["busy"], "modal");
    }

    #[test]
    fn a_request_waiting_when_a_render_starts_or_the_main_thread_stops_ticking_is_answered_busy_at_once() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        for (id, why) in [(1u64, "rendering"), (2, "dialog")] {
            let snapshot = lab.snapshot.clone();
            let later = thread::spawn(move || {
                thread::sleep(Duration::from_millis(150));
                let mut snapshot = snapshot.lock().unwrap();
                if id == 1 {
                    snapshot.busy = Some(Busy::Rendering);
                } else {
                    // A long load or a plug-in scan: no more ticks, and nobody has said why.
                    snapshot.busy = None;
                    snapshot.last_tick = Some(Instant::now() - Duration::from_secs(5));
                }
            });
            let started = Instant::now();
            let frame = Request { id, method: "read".into(), params: json!({"kind": "track"}), deadline_ms: Some(unix_ms() + 20_000) };
            stream.write_all(&Frame::Request(frame).encode()).unwrap();
            let response = read_frame(&mut reader);
            later.join().unwrap();
            assert_eq!(response["error"]["code"], code::BUSY, "{response}");
            assert!(response["error"]["message"].as_str().unwrap().contains(why), "{response}");
            assert!(started.elapsed() < Duration::from_secs(3), "waited {:?} for a deadline 20 s away", started.elapsed());
            // Back to normal for the next round.
            let mut snapshot = lab.snapshot.lock().unwrap();
            snapshot.busy = None;
            snapshot.last_tick = Some(Instant::now());
        }
    }

    #[test]
    fn a_request_waiting_when_a_dialog_opens_is_answered_busy_at_once() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        // Nothing drains the queue. The request waits for a far deadline; the main thread finds the dialog a moment later.
        let snapshot = lab.snapshot.clone();
        let later = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            snapshot.lock().unwrap().busy = Some(Busy::Modal);
        });
        let started = Instant::now();
        let request_frame =
            Request { id: 1, method: "read".into(), params: json!({"kind": "track"}), deadline_ms: Some(unix_ms() + 20_000) };
        stream.write_all(&Frame::Request(request_frame).encode()).unwrap();
        let response = read_frame(&mut reader);
        later.join().unwrap();
        assert_eq!(response["error"]["code"], code::BUSY);
        assert!(response["error"]["message"].as_str().unwrap().contains("dialog"));
        assert!(started.elapsed() < Duration::from_secs(3), "waited {:?} for a deadline 20 s away", started.elapsed());
    }

    #[test]
    fn status_answers_off_the_main_thread_and_says_when_it_is_held() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        let status = request(&mut stream, &mut reader, 1, "status")["result"].clone();
        assert_eq!(status["busy"], false);
        assert_eq!(status["tracks"], 3);
        assert_eq!(status["dawVersion"], "7.82/macOS-arm64");
        assert_eq!(status["connections"], 1);

        // No tick for a while: a dialog holds the main thread. Queued work is refused as busy, nothing changed.
        lab.snapshot.lock().unwrap().last_tick = Some(Instant::now() - Duration::from_secs(5));
        let status = request(&mut stream, &mut reader, 2, "status")["result"].clone();
        assert_eq!(status["busy"], "modal");
        assert!(status["busyReason"].as_str().unwrap().contains("dialog"));
        // A request is refused at once, "busy": it would only sit in the queue.
        let started = Instant::now();
        let response = request(&mut stream, &mut reader, 3, "quit");
        assert_eq!(response["error"]["code"], code::BUSY);
        assert!(response["error"]["message"].as_str().unwrap().contains("nothing changed"));
        assert!(started.elapsed() < Duration::from_millis(500));
        // A deadline that has passed is told so first.
        let past = Request { id: 4, method: "quit".into(), params: json!({}), deadline_ms: Some(unix_ms().saturating_sub(5)) };
        stream.write_all(&Frame::Request(past).encode()).unwrap();
        assert_eq!(read_frame(&mut reader)["error"]["code"], code::DEADLINE);
        // And a method the bridge doesn't have needs no tick to be refused.
        assert_eq!(request(&mut stream, &mut reader, 5, "explode")["error"]["code"], code::UNKNOWN_METHOD);
    }

    #[test]
    fn a_discovery_file_that_cant_be_written_is_tried_again_in_a_while_not_every_tick() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let snapshot = lab.snapshot.lock().unwrap().clone();
        let good = lab.dir.path().join("bridges/reaper/discovery");
        // A file where the folder should be.
        let blocked = lab.dir.path().join("blocked");
        std::fs::write(&blocked, "").unwrap();
        lab.server().config.discovery_dir = blocked.join("discovery");
        lab.server().publish(&snapshot);
        assert!(lab.server().discovery.is_none() && lab.server().discovery_failure.is_some());
        // The problem goes away, but the next tick doesn't try yet.
        lab.server().config.discovery_dir = good.clone();
        lab.server().publish(&snapshot);
        assert!(files::scan_discovery(&good, Daw::Reaper).is_empty());
        lab.server().discovery_failure.as_mut().unwrap().0 -= DISCOVERY_RETRY;
        lab.server().publish(&snapshot);
        assert_eq!(files::scan_discovery(&good, Daw::Reaper).len(), 1);
        assert!(lab.server().discovery_failure.is_none());
    }

    #[test]
    fn the_discovery_file_follows_the_project_and_goes_with_the_server() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let snapshot = lab.snapshot.lock().unwrap().clone();
        lab.server().publish(&snapshot);
        let dir = lab.dir.path().join("bridges/reaper/discovery");
        let found = files::scan_discovery(&dir, Daw::Reaper);
        assert_eq!(found.len(), 1);
        let discovery = found[0].discovery.as_ref().unwrap();
        assert_eq!(discovery.port, lab.server().port());
        assert_eq!(discovery.project.as_ref().unwrap().name.as_deref(), Some("Song.rpp"));

        let mut moved = snapshot.clone();
        moved.project = Some(ProjectSummary::default());
        lab.server().publish(&moved);
        let found = files::scan_discovery(&dir, Daw::Reaper);
        assert_eq!(found[0].discovery.as_ref().unwrap().project, Some(Project::default()));

        let (_stream, mut reader, _) = lab.authenticated();
        lab.server.take().unwrap().stop("quit");
        assert_eq!(read_frame(&mut reader), json!({"type": "bye", "reason": "quit"}));
        assert_eq!(read_line(&mut reader), None);
        assert!(files::scan_discovery(&dir, Daw::Reaper).is_empty());
    }

    #[test]
    fn a_long_project_title_is_cut_in_the_discovery_file_and_the_welcome() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let mut snapshot = lab.snapshot.lock().unwrap().clone();
        snapshot.project = Some(ProjectSummary { name: "é".repeat(100_000), path: format!("/m/{}", "p".repeat(100_000)) });
        *lab.snapshot.lock().unwrap() = snapshot.clone();
        lab.server().publish(&snapshot);
        let dir = lab.dir.path().join("bridges/reaper/discovery");
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(files.len(), 1);
        // Far under the 64 KiB a host will read, and still found.
        assert!(files[0].metadata().unwrap().len() < 8 * 1024, "{} bytes", files[0].metadata().unwrap().len());
        let found = files::scan_discovery(&dir, Daw::Reaper);
        let project = found[0].discovery.as_ref().unwrap().project.clone().unwrap();
        let name = project.name.unwrap();
        assert_eq!(name.chars().count(), MAX_NAME);
        assert!(name.ends_with('…') && name.starts_with("éé"));
        assert_eq!(project.path.unwrap().chars().count(), MAX_PATH);
        // The welcome carries the same, so a long title can't hold up the handshake either.
        let (_stream, _reader, welcome) = lab.authenticated();
        assert_eq!(welcome["project"]["name"].as_str().unwrap().chars().count(), MAX_NAME);
        // A name that fits is left alone.
        assert_eq!(cut("Song.rpp", MAX_NAME), "Song.rpp");
        assert_eq!(cut(&"a".repeat(MAX_NAME), MAX_NAME).chars().count(), MAX_NAME);
    }

    #[test]
    fn stopping_answers_a_queued_request_and_never_waits_for_the_main_thread() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        // Nothing drains the queue: the main thread is the one inside stop(). The host picked a far deadline.
        let request = Request { id: 1, method: "action".into(), params: json!({"command": 40001}), deadline_ms: Some(unix_ms() + 25_000) };
        stream.write_all(&Frame::Request(request).encode()).unwrap();
        thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        lab.server.take().unwrap().stop("quit");
        assert!(started.elapsed() < Duration::from_secs(2), "stop took {:?}", started.elapsed());
        let response = read_frame(&mut reader);
        assert_eq!((response["id"].as_u64(), response["error"]["code"].as_str()), (Some(1), Some(code::FAILED)));
        assert!(response["error"]["message"].as_str().unwrap().contains("closing"));
        assert_eq!(read_frame(&mut reader), json!({"type": "bye", "reason": "quit"}));
        assert_eq!(read_line(&mut reader), None);
    }

    #[test]
    fn a_host_whose_reader_left_before_the_stop_took_its_list_is_still_told_goodbye() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader, _) = lab.authenticated();
        let request = Request { id: 1, method: "action".into(), params: json!({"command": 40001}), deadline_ms: None };
        stream.write_all(&Frame::Request(request).encode()).unwrap();
        // The request is waiting for the main thread (it is in the queue, put back for the server's sake).
        let job = lab.jobs.recv_timeout(Duration::from_secs(5)).unwrap();
        lab.tx.send(job).unwrap();
        let mut server = lab.server.take().unwrap();
        let closing = server.begin_stop();
        // The race, made certain: the reader answers "closing" and leaves the hub before the stop goes any further.
        let until = Instant::now() + Duration::from_secs(5);
        while server.hub.connections() > 0 && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.hub.connections(), 0, "the reader should have left");
        server.finish_stop(closing, "quit");
        let response = read_frame(&mut reader);
        assert_eq!((response["id"].as_u64(), response["error"]["code"].as_str()), (Some(1), Some(code::FAILED)));
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
            lab.hub.event("tracks", json!({"pad": big}));
        }
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        lab.server.take().unwrap().stop("quit");
        assert!(started.elapsed() < Duration::from_secs(3), "stop took {:?}", started.elapsed());
    }

    #[test]
    fn parsing_a_big_frame_never_holds_up_the_main_threads_events() {
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
            lab.hub.event("tracks", json!({"call": calls}));
            worst = worst.max(started.elapsed());
            calls += 1;
            thread::sleep(Duration::from_millis(1));
        }
        let _stream = host.join().unwrap();
        // The reader was parsing for most of that time; an event from the main thread must not have waited for it.
        assert!(worst < Duration::from_millis(300), "an event took {worst:?} (of {calls} calls)");
    }

    #[test]
    fn an_event_nested_past_the_limit_is_not_sent_and_takes_no_number() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (_stream, mut reader, _) = lab.authenticated();
        let deep = (0..wire::MAX_PAYLOAD_DEPTH + 1).fold(json!(1), |inner, _| json!([inner]));
        lab.hub.event("tracks", deep);
        lab.hub.event("tracks", json!({"count": 2}));
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
            lab.hub.event("tracks", json!({"round": round}));
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
    // this fails if the extension passes a stale one.
    #[test]
    fn a_peer_that_keeps_sending_is_refused_at_the_deadline_by_the_real_clock() {
        let mut lab = Lab::start(VECTOR_SECRET, ServerConnection::new);
        let (mut stream, mut reader) = lab.connect();
        let accepted = Instant::now();
        let refused_at = Arc::new(Mutex::new(None));
        let seen = refused_at.clone();
        // Read as it comes: a write after the bridge closed resets the connection, and a refusal not yet read is lost
        // on Windows.
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
