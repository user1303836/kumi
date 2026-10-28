//! In-process stand-ins for the bridges inside REAPER and Renoise: the same wire through daw-bridge's
//! [`ServerConnection`], a scripted identity, and a transcript of every line, for the host's tests.
use daw_bridge::auth::Secret;
use daw_bridge::files::{write_discovery, Discovery, DISCOVERY_SCHEMA};
use daw_bridge::frame::Framer;
use daw_bridge::handshake::{Offer, ServerAction, ServerConnection};
use daw_bridge::wire::{code, event, Frame, Identity, Project, Request, CREDIT_FLOOR_BYTES, PROTOCOL};
use daw_bridge::Daw;
use serde_json::{json, Map, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

/// One line on a simulated connection: `>` from the host, `<` from the bridge, with the connection's number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub connection: usize,
    pub from_host: bool,
    pub text: String,
}

pub mod listening;

/// What a simulated bridge answers beyond `ping` and `status`: `Some(Ok(result))`, `Some(Err((code, message)))`, or None
/// for a method it doesn't have.
pub type Respond = Arc<dyn Fn(&str, &Value) -> Option<Result<Value, (String, String)>> + Send + Sync>;

/// `status` fields read as each status is answered (what listening the bridge can do now, M5).
pub type StatusMore = Arc<dyn Fn() -> Map<String, Value> + Send + Sync>;

#[derive(Clone)]
pub struct SimulatorConfig {
    pub secret: Secret,
    pub identity: Identity,
    pub offer: Offer,
    /// Extra `status` fields (Renoise's `demo`).
    pub status_extra: Map<String, Value>,
    /// Where to write the discovery file, if anywhere.
    pub discovery_dir: Option<PathBuf>,
    /// The methods beyond ping and status.
    pub respond: Option<Respond>,
    /// `status` fields read at each status.
    pub status_more: Option<StatusMore>,
}

impl std::fmt::Debug for SimulatorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatorConfig")
            .field("identity", &self.identity)
            .field("respond", &self.respond.is_some())
            .finish_non_exhaustive()
    }
}

impl SimulatorConfig {
    /// REAPER 7.82 on a portable resource folder, with a saved project.
    pub fn reaper(secret: Secret, resource: &str) -> Self {
        Self {
            secret,
            identity: Identity {
                preset_files: Vec::new(),
                daw: Daw::Reaper.id().into(),
                bridge: "0.1.0".into(),
                daw_version: "7.82/macOS-arm64".into(),
                pid: std::process::id(),
                resource: resource.into(),
                project: Some(Project {
                    name: Some("Song.rpp".into()),
                    path: Some(format!("{resource}/Projects/Song.rpp")),
                    ..Project::default()
                }),
            },
            offer: Offer {
                events: vec![event::PROJECT.into(), event::TRANSPORT.into(), event::TRACKS.into(), event::SELECTION.into()],
                ..Offer::default()
            },
            status_extra: Map::new(),
            discovery_dir: None,
            respond: None,
            status_more: None,
        }
    }

    /// Adds methods, answered by `respond`, to what the bridge offers.
    pub fn with_methods(mut self, methods: &[&str], respond: Respond) -> Self {
        self.offer.methods.extend(methods.iter().map(|m| m.to_string()));
        self.respond = Some(respond);
        self
    }

    /// Renoise 3.5.4 (licensed) with an unsaved song.
    pub fn renoise(secret: Secret, resource: &str) -> Self {
        let mut status_extra = Map::new();
        status_extra.insert("demo".into(), false.into());
        Self {
            secret,
            identity: Identity {
                preset_files: Vec::new(),
                daw: Daw::Renoise.id().into(),
                bridge: "0.1.0".into(),
                daw_version: "3.5.4".into(),
                pid: std::process::id(),
                resource: resource.into(),
                project: Some(Project { name: Some("Untitled".into()), ..Project::default() }),
            },
            offer: Offer {
                events: vec![event::PROJECT.into(), event::TRANSPORT.into(), event::TRACKS.into(), event::SELECTION.into()],
                // Like the Renoise tool: it sends only within the credit the host grants.
                credit_bytes: Some(CREDIT_FLOOR_BYTES),
                ..Offer::default()
            },
            status_extra,
            discovery_dir: None,
            respond: None,
            status_more: None,
        }
    }
}

struct State {
    identity: Identity,
    busy: Option<String>,
    /// Methods whose requests get no answer at all (a bridge stuck in the middle of one).
    held: Vec<String>,
    transcript: Vec<Line>,
    connections: usize,
    flow: Flow,
    /// Connections numbered up to this have been let go of without a word: they answer nothing, send nothing.
    silent_upto: usize,
    /// The whole DAW is frozen (a plug-in loading, a long script): every connection, new ones too, answers nothing.
    frozen: bool,
    /// Connections numbered up to this are retired as the Renoise tool retires a host: the line part sent is finished, a bye
    /// follows it, and only credit frames are taken from then on.
    retired_upto: usize,
}

/// What a bridge that sends within credit has sent and held back, over every connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flow {
    /// Bytes sent that credit had to cover (the handshake and the bye are not counted).
    pub sent: u64,
    /// Bytes waiting for credit.
    pub waiting: u64,
}

/// A running simulated bridge on 127.0.0.1 (an ephemeral port).
pub struct Simulator {
    address: SocketAddr,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<(String, Value)>,
    stop: watch::Sender<bool>,
    /// Raised to wake the connections when `silent_upto` or `retired_upto` moves.
    wake: watch::Sender<u64>,
    config: SimulatorConfig,
    started_ms: u64,
}

impl Simulator {
    pub async fn start(config: SimulatorConfig) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State {
            identity: config.identity.clone(),
            busy: None,
            held: vec![],
            transcript: vec![],
            connections: 0,
            flow: Flow::default(),
            silent_upto: 0,
            frozen: false,
            retired_upto: 0,
        }));
        let (events, _) = broadcast::channel(256);
        let (stop, stopped) = watch::channel(false);
        let (wake, woken) = watch::channel(0u64);
        let started_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let simulator = Self { address, state: state.clone(), events: events.clone(), stop, wake, config: config.clone(), started_ms };
        simulator.write_discovery()?;
        tokio::spawn(async move {
            let mut stopped_accepting = stopped.clone();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, peer)) = accepted else { continue };
                        if !daw_bridge::loopback::peer_allowed(&peer) {
                            continue;
                        }
                        let number = {
                            let mut state = state.lock().unwrap();
                            state.connections += 1;
                            state.connections
                        };
                        tokio::spawn(serve(stream, number, config.clone(), state.clone(), events.subscribe(), stopped.clone(), woken.clone()));
                    }
                    _ = stopped_accepting.changed() => return,
                }
            }
        });
        Ok(simulator)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn discovery(&self) -> Discovery {
        let identity = self.state.lock().unwrap().identity.clone();
        Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: identity.daw,
            pid: identity.pid,
            host: "127.0.0.1".into(),
            port: self.address.port(),
            protocol: PROTOCOL.into(),
            version: identity.bridge,
            daw_version: identity.daw_version,
            resource: identity.resource,
            project: identity.project,
            started_ms: self.started_ms,
        }
    }

    fn write_discovery(&self) -> std::io::Result<()> {
        if let Some(dir) = &self.config.discovery_dir {
            write_discovery(dir, &self.discovery())?;
        }
        Ok(())
    }

    /// Sends an event to every authenticated connection.
    pub fn emit(&self, name: &str, data: Value) {
        let _ = self.events.send((name.into(), data));
    }

    /// Opens another project: the discovery file and `status` follow, and a `project` event goes out.
    pub fn open_project(&self, project: Option<Project>) -> std::io::Result<()> {
        self.state.lock().unwrap().identity.project = project.clone();
        self.write_discovery()?;
        self.emit(event::PROJECT, serde_json::to_value(project).unwrap());
        Ok(())
    }

    /// Busy (`Some("rendering")`): requests are refused as busy and heartbeats stop, as in a modal dialog.
    pub fn set_busy(&self, busy: Option<&str>) {
        self.state.lock().unwrap().busy = busy.map(str::to_string);
    }

    /// Requests for this method are taken and never answered (until `release`), the rest go on as they do.
    pub fn hold(&self, method: &str) {
        self.state.lock().unwrap().held.push(method.to_string());
    }

    pub fn release(&self) {
        self.state.lock().unwrap().held.clear();
    }

    /// The bridge lets go of every connection made so far without a word, as a bridge that lost track of them: they answer
    /// nothing and send nothing (a new connection is served as before).
    pub fn go_silent(&self) {
        let mut state = self.state.lock().unwrap();
        state.silent_upto = state.connections;
        drop(state);
        self.wake.send_modify(|n| *n += 1);
    }

    /// The DAW freezes: the socket still takes connections, but nothing is answered on any, new ones included (the
    /// handshake never completes), until `thaw`.
    pub fn freeze(&self) {
        self.state.lock().unwrap().frozen = true;
        self.wake.send_modify(|n| *n += 1);
    }

    pub fn thaw(&self) {
        self.state.lock().unwrap().frozen = false;
        self.wake.send_modify(|n| *n += 1);
    }

    /// The bridge retires every connection made so far as the Renoise tool retires a host that stopped reading: the line it
    /// was part way through sending is finished (as the host grants credit), a bye follows it, nothing else is sent, and
    /// only credit is taken.
    pub fn retire_with_bye(&self) {
        let mut state = self.state.lock().unwrap();
        state.retired_upto = state.connections;
        drop(state);
        self.wake.send_modify(|n| *n += 1);
    }

    /// What the bridge has sent within credit, and what waits for more.
    pub fn flow(&self) -> Flow {
        self.state.lock().unwrap().flow
    }

    /// Every line so far, in order.
    pub fn transcript(&self) -> Vec<Line> {
        self.state.lock().unwrap().transcript.clone()
    }

    /// Says bye to every connection, closes them and removes the discovery file, as a DAW quitting does.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
        if let Some(dir) = &self.config.discovery_dir {
            let _ = std::fs::remove_file(dir.join(self.discovery().file_name()));
        }
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

fn record(state: &Mutex<State>, connection: usize, from_host: bool, bytes: &[u8]) {
    let mut state = state.lock().unwrap();
    for line in String::from_utf8_lossy(bytes).split('\n').filter(|line| !line.trim().is_empty()) {
        state.transcript.push(Line { connection, from_host, text: line.trim_end_matches('\r').to_string() });
    }
}

fn answer(request: &Request, state: &Mutex<State>, config: &SimulatorConfig) -> Frame {
    let state = state.lock().unwrap();
    // Busy, as the bridges are: status and focus still answer (saying why), the rest is refused.
    if let Some(busy) = state.busy.clone().filter(|_| !matches!(request.method.as_str(), "status" | "focus")) {
        return Frame::error(request.id, code::BUSY, format!("{} is {busy}", state.identity.daw));
    }
    let busy = state.busy.clone();
    match request.method.as_str() {
        "ping" => Frame::ok(request.id, json!({})),
        "focus" if busy.is_some() => {
            let respond = config.respond.clone();
            drop(state);
            match respond.and_then(|respond| respond("focus", &request.params)) {
                Some(Ok(mut focus)) => {
                    focus["busy"] = busy.into();
                    Frame::ok(request.id, focus)
                }
                _ => Frame::error(request.id, code::BUSY, "busy; nothing changed"),
            }
        }
        "status" => {
            let mut status = serde_json::to_value(&state.identity).unwrap();
            status["busy"] = busy.map_or(Value::Bool(false), Value::from);
            for (key, value) in &config.status_extra {
                status[key] = value.clone();
            }
            if let Some(more) = &config.status_more {
                for (key, value) in more() {
                    status[key] = value;
                }
            }
            Frame::ok(request.id, status)
        }
        other => match config.respond.as_ref().and_then(|respond| respond(other, &request.params)) {
            Some(Ok(result)) => Frame::ok(request.id, result),
            Some(Err((code, message))) => Frame::error(request.id, &code, message),
            None => Frame::error(request.id, code::UNKNOWN_METHOD, format!("no method {other}")),
        },
    }
}

/// The most credit a bridge holds (the Renoise tool's cap).
const MAX_CREDIT: u64 = 4 * 1024 * 1024;

/// Whether a frame is sent whatever the credit: the handshake, and the bye (the tool ignores credit on unload).
fn uncounted(bytes: &[u8]) -> bool {
    let line = String::from_utf8_lossy(bytes);
    let first = line.split('\n').next().unwrap_or("");
    matches!(
        Frame::parse(first.trim_end_matches('\r')),
        Ok(Frame::Challenge { .. } | Frame::Welcome(_) | Frame::Refused { .. } | Frame::Bye { .. })
    )
}

/// Where a frame goes: out at once (the handshake, the bye), or out within the credit.
fn put(direct: &mut Vec<u8>, counted: &mut Vec<u8>, bytes: Vec<u8>) {
    if uncounted(&bytes) {
        direct.extend(bytes);
    } else {
        counted.extend(bytes);
    }
}

async fn serve(
    stream: TcpStream,
    number: usize,
    config: SimulatorConfig,
    state: Arc<Mutex<State>>,
    mut events: broadcast::Receiver<(String, Value)>,
    mut stopped: watch::Receiver<bool>,
    mut woken: watch::Receiver<u64>,
) {
    let (mut reader, mut writer) = stream.into_split();
    let clock = Instant::now();
    let mut connection = ServerConnection::new(config.secret.clone(), 0);
    let mut chunk = vec![0u8; 64 * 1024];
    // A bridge that names `creditBytes` sends within the credit the host grants, as the Renoise tool does: what it may
    // not send yet waits in `queue`, and a `credit` frame lets more go.
    let limited = config.offer.credit_bytes.is_some();
    let mut credit = config.offer.credit_bytes.unwrap_or(0);
    let mut queue: Vec<u8> = vec![];
    let mut host_lines = Framer::new(1 << 20);
    // Set by `retire_with_bye`: nothing is sent but what is held, and then a bye.
    let mut retiring = false;
    let mut bye_pending = false;
    let mut heartbeat = tokio::time::interval(Duration::from_millis(config.offer.heartbeat_ms.max(1)));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let mut out: Vec<u8> = vec![];
        let mut direct: Vec<u8> = vec![];
        let mut close = false;
        tokio::select! {
            read = reader.read(&mut chunk) => {
                let count = match read { Ok(0) | Err(_) => return, Ok(count) => count };
                record(&state, number, true, &chunk[..count]);
                host_lines.feed(&chunk[..count]);
                while let Some(Ok(line)) = host_lines.next_line() {
                    if let Ok(Frame::Credit { bytes }) = Frame::parse(&line) {
                        credit = (credit + bytes).min(MAX_CREDIT);
                    }
                }
                let mut identity = || (state.lock().unwrap().identity.clone(), config.offer.clone());
                for action in connection.on_bytes(&chunk[..count], clock.elapsed().as_millis() as u64, &mut identity) {
                    match action {
                        ServerAction::Send(bytes) => put(&mut direct, &mut out, bytes),
                        ServerAction::Close(_) => close = true,
                        ServerAction::Authenticated => {}
                        ServerAction::Request(request) => {
                            if !state.lock().unwrap().held.contains(&request.method) {
                                put(&mut direct, &mut out, answer(&request, &state, &config).encode());
                            }
                        }
                    }
                }
            }
            _ = heartbeat.tick() => {
                for action in connection.on_tick(clock.elapsed().as_millis() as u64) {
                    match action {
                        ServerAction::Send(bytes) => put(&mut direct, &mut out, bytes),
                        ServerAction::Close(_) => close = true,
                        _ => {}
                    }
                }
                // (Not behind output that waits for credit, as in the tool.)
                if connection.is_authenticated() && state.lock().unwrap().busy.is_none() && queue.is_empty() {
                    put(&mut direct, &mut out, Frame::Heartbeat { ms: clock.elapsed().as_millis() as u64 }.encode());
                }
            }
            sent = events.recv() => {
                if let Ok((name, data)) = sent {
                    if let Some(bytes) = connection.event(&name, data) {
                        put(&mut direct, &mut out, bytes);
                    }
                }
            }
            _ = woken.changed() => {}
            _ = stopped.changed() => {
                if connection.is_authenticated() {
                    put(&mut direct, &mut out, Frame::Bye { reason: "quit".into() }.encode());
                }
                close = true;
            }
        }
        let (silent, retire) = {
            let state = state.lock().unwrap();
            (number <= state.silent_upto || state.frozen, number <= state.retired_upto)
        };
        if silent {
            // Let go of: nothing it was asked or told reaches the host.
            out.clear();
            direct.clear();
            close = false;
        }
        if retire && !retiring && connection.is_authenticated() {
            // The line part sent is kept (up to its newline), and the bye comes after it.
            retiring = true;
            bye_pending = true;
            if let Some(end) = queue.iter().position(|b| *b == b'\n') {
                queue.truncate(end + 1);
            } else {
                queue.clear();
            }
        }
        if retiring {
            out.clear();
            direct.clear();
        }
        if !out.is_empty() || !direct.is_empty() {
            // Heartbeats are left out of the transcript: their timing isn't the test's.
            let mut produced = direct.clone();
            produced.extend_from_slice(&out);
            let kept: Vec<u8> = String::from_utf8_lossy(&produced)
                .split_inclusive('\n')
                .filter(|line| !line.starts_with("{\"type\":\"heartbeat\""))
                .collect::<String>()
                .into_bytes();
            record(&state, number, false, &kept);
        }
        let mut send = direct;
        if limited {
            queue.extend(out);
            let allowed = (queue.len() as u64).min(credit) as usize;
            credit -= allowed as u64;
            send.extend(queue.drain(..allowed));
            let mut state = state.lock().unwrap();
            state.flow.sent += allowed as u64;
            state.flow.waiting = queue.len() as u64;
        } else {
            send.extend(out);
        }
        // (a retired host's bye follows what was held, whatever credit is left)
        if retiring && bye_pending && queue.is_empty() {
            let bye = Frame::Bye { reason: "overload".into() }.encode();
            record(&state, number, false, &bye);
            send.extend(bye);
            bye_pending = false;
        }
        if !send.is_empty() && writer.write_all(&send).await.is_err() {
            return;
        }
        if close {
            let _ = writer.shutdown().await;
            return;
        }
    }
}
