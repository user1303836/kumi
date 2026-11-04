//! The bridge's loopback server: `kumi-daw/1` to Kumi's host, with daw-bridge's handshake and framing.
//!
//! `ping` and `status` are answered from what the bridge last saw of SoundCraft, so they answer at once even while the
//! app is busy. The look loop asks the app for `ui.inspect` each second, and for the session every few seconds: what's
//! open, which the reads' lives learn from too, so a project event goes out for another session even under the same name.
//! A dialog open in the app, or three looks in a row that miss their second, make the bridge busy, and a busy
//! bridge sends no heartbeats, as the other bridges don't. The discovery file names the app's pid and settings folder. It
//! goes when the bridge ends: when the host that started it goes (its stdin ends), or when SoundCraft quits.
use crate::control::{Control, Error};
use crate::launch::Launched;
use crate::read::Reads;
use daw_bridge::auth::Secret;
use daw_bridge::files::{process_alive, read_secret, write_discovery, Discovery, DISCOVERY_SCHEMA};
use daw_bridge::handshake::{refuse_connection, Offer, ServerAction, ServerConnection};
use daw_bridge::loopback::peer_allowed;
use daw_bridge::wire::{code, event, Frame, Identity, Project, Request, HEARTBEAT_MS, MAX_AUTHENTICATED, MAX_UNAUTHENTICATED, PROTOCOL};
use serde_json::{json, Map, Value};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

/// The DAW's id on the wire and in file names.
pub const DAW: &str = "soundcraft";
/// The bridge's own version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How often the bridge looks at the app.
pub const LOOK: Duration = Duration::from_secs(1);
/// Looks in a row that miss their second before the app counts as stalled.
const MISSES_BEFORE_STALLED: u32 = 3;
/// Looks between two reads of the session's name and path: each is a `session.inspect`, which costs the app a few frames
/// on a big session.
const LOOKS_PER_SESSION: u32 = 5;
/// How long the connections get to say bye when the bridge ends.
const BYE_GRACE: Duration = Duration::from_millis(300);
/// How long the listener rests after an accept fails (a connection reset before it was taken, descriptors run out).
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
/// How long a refused connection's input is read before it's dropped.
const TURN_AWAY: Duration = Duration::from_millis(100);

/// What the bridge serves, and for which copy of SoundCraft.
pub struct Config {
    /// The bridge's secret, read for each connection, as kumi-daw/1 asks: one replaced while the bridge runs is used.
    pub secret_file: PathBuf,
    pub discovery_dir: PathBuf,
    pub launched: Launched,
    /// SoundCraft's settings folder: what tells two copies apart.
    pub resource: String,
    pub look: Duration,
    pub heartbeat_ms: u64,
    /// What makes this bridge the copy's only one (`serving-<pid>.lock`, held): let go with the discovery file, so the
    /// bridge a host starts once that file has gone finds the copy free, though this one is still saying bye.
    pub serving: Option<std::fs::File>,
}

impl Config {
    /// A look each second, a heartbeat each second.
    pub fn new(secret_file: PathBuf, discovery_dir: PathBuf, launched: Launched, resource: String) -> Self {
        Self { secret_file, discovery_dir, launched, resource, look: LOOK, heartbeat_ms: HEARTBEAT_MS, serving: None }
    }
}

/// Why the bridge ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The host that started it went (its stdin ended), or something else asked it to stop.
    Stopped,
    /// SoundCraft quit.
    AppQuit,
}

/// What the bridge last saw of the app.
struct Seen {
    identity: Identity,
    /// Why the app can't take work now: a short code (`modal`, `stalled`) and the words for it.
    busy: Option<(&'static str, String)>,
}

fn lock(seen: &Mutex<Seen>) -> MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serves until `stop` comes or SoundCraft quits, and says which. The discovery file is written before it serves and
/// removed when it ends.
pub async fn serve(config: Config, stop: impl Future<Output = ()>) -> Result<End, String> {
    let control = config.launched.control();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|error| format!("the bridge can't listen on loopback: {error}"))?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();
    let reads = Reads::new(control.clone());
    let epochs = reads.epochs();
    let reads = Arc::new(tokio::sync::Mutex::new(reads));
    let identity = Identity {
        preset_files: vec![],
        daw: DAW.into(),
        bridge: VERSION.into(),
        daw_version: config.launched.version.clone().unwrap_or_default(),
        pid: config.launched.pid,
        resource: config.resource.clone(),
        project: look_at_session(&control, &reads, &epochs, config.look).await,
    };
    let started_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
    let notice = move |identity: &Identity| Discovery {
        schema: DISCOVERY_SCHEMA.into(),
        daw: DAW.into(),
        pid: identity.pid,
        host: "127.0.0.1".into(),
        port,
        protocol: PROTOCOL.into(),
        version: identity.bridge.clone(),
        daw_version: identity.daw_version.clone(),
        resource: identity.resource.clone(),
        project: identity.project.clone(),
        started_ms,
    };
    let discovery = notice(&identity);
    let file = config.discovery_dir.join(discovery.file_name());
    write_discovery(&config.discovery_dir, &discovery).map_err(|error| format!("the bridge can't write its discovery file: {error}"))?;
    let seen = Arc::new(Mutex::new(Seen { identity, busy: None }));
    let (events, _) = broadcast::channel::<(String, Value)>(64);
    let (ending, ended) = watch::channel(None::<&'static str>);
    let offer = Offer {
        methods: vec!["ping".into(), "status".into(), "read".into(), "snapshot".into()],
        events: vec![event::PROJECT.into()],
        heartbeat_ms: config.heartbeat_ms,
        credit_bytes: None,
    };
    let serving = Serving { seen: seen.clone(), offer, reads: reads.clone() };
    let accepting = tokio::spawn(accept(listener, config.secret_file.clone(), serving, events.clone(), ended));
    let rewrite = |identity: &Identity| {
        let _ = write_discovery(&config.discovery_dir, &notice(identity));
    };
    let end = tokio::select! {
        () = stop => End::Stopped,
        end = look(&control, &seen, &reads, &epochs, &events, &rewrite, config.look) => end,
    };
    // The file and the lock go and the listener closes first, so no host finds a bridge that's ending, and the next one
    // finds the copy free; then the connections say bye.
    let _ = std::fs::remove_file(&file);
    drop(config.serving);
    accepting.abort();
    let _ = ending.send(Some(match end {
        End::Stopped => "the bridge stopped",
        End::AppQuit => "SoundCraft quit",
    }));
    tokio::time::sleep(BYE_GRACE).await;
    Ok(end)
}

/// A look at the session (`session.inspect`, its summary): what's open now, with the epoch of its objects' refs. The reads'
/// lives learn from it, unless a read holds them (that read looks itself), and the epoch is told either way. Nothing while
/// the app doesn't answer.
async fn look_at_session(
    control: &Control,
    reads: &tokio::sync::Mutex<Reads>,
    epochs: &watch::Receiver<String>,
    deadline: Duration,
) -> Option<Project> {
    let session = control.call("session.inspect", json!({}), deadline).await.ok()?;
    if let Ok(mut reads) = reads.try_lock() {
        reads.saw(&session);
    }
    let text = |key: &str| session.get(key).and_then(Value::as_str).map(str::to_string);
    let mut extra = Map::new();
    extra.insert("epoch".into(), epochs.borrow().as_str().into());
    Some(Project { name: text("name"), path: text("path"), extra, ..Project::default() })
}

/// Looks at the app each `every`, keeping what was seen and telling the connections what changed, until the app quits.
async fn look(
    control: &Control,
    seen: &Mutex<Seen>,
    reads: &tokio::sync::Mutex<Reads>,
    epochs: &watch::Receiver<String>,
    events: &broadcast::Sender<(String, Value)>,
    rewrite: &(dyn Fn(&Identity) + Sync),
    every: Duration,
) -> End {
    let pid = lock(seen).identity.pid;
    let (mut misses, mut looks) = (0u32, 0u32);
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        looks += 1;
        let busy = match control.call("ui.inspect", json!({}), every).await {
            Ok(ui) => {
                misses = 0;
                ui.get("dialog").filter(|dialog| !dialog.is_null()).map(|dialog| {
                    let named = dialog.as_str().map(str::to_string).unwrap_or_else(|| dialog.to_string());
                    ("modal", format!("a dialog is open in SoundCraft ({named})"))
                })
            }
            Err(Error::Unreachable(_) | Error::Closed) if !process_alive(pid) => return End::AppQuit,
            Err(_) => {
                misses += 1;
                if misses >= MISSES_BEFORE_STALLED {
                    Some(("stalled", "SoundCraft isn't answering".to_string()))
                } else {
                    lock(seen).busy.clone()
                }
            }
        };
        lock(seen).busy = busy;
        if looks % LOOKS_PER_SESSION != 0 || lock(seen).busy.is_some() {
            continue;
        }
        let Some(now) = look_at_session(control, reads, epochs, every).await else { continue };
        let mut guard = lock(seen);
        if guard.identity.project.as_ref() == Some(&now) {
            continue;
        }
        guard.identity.project = Some(now.clone());
        let identity = guard.identity.clone();
        drop(guard);
        rewrite(&identity);
        let _ = events.send((event::PROJECT.into(), serde_json::to_value(&now).unwrap_or(Value::Null)));
    }
}

/// Connections waiting to authenticate, and authenticated ones.
#[derive(Default)]
struct Counts {
    waiting: AtomicUsize,
    authenticated: AtomicUsize,
}

/// What every connection shares: what the bridge last saw, what it offers, and its reads of the app (one at a time).
#[derive(Clone)]
struct Serving {
    seen: Arc<Mutex<Seen>>,
    offer: Offer,
    reads: Arc<tokio::sync::Mutex<Reads>>,
}

/// Takes connections from loopback only, as many as the protocol allows (the one past that is refused and closed). An
/// accept that fails is one connection's failure, or a moment out of descriptors: the listener rests and goes on.
async fn accept(
    listener: TcpListener,
    secret_file: PathBuf,
    serving: Serving,
    events: broadcast::Sender<(String, Value)>,
    ended: watch::Receiver<Option<&'static str>>,
) {
    let counts = Arc::new(Counts::default());
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(_) => {
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        if !peer_allowed(&peer) {
            continue;
        }
        if counts.waiting.load(Ordering::SeqCst) >= MAX_UNAUTHENTICATED || counts.authenticated.load(Ordering::SeqCst) >= MAX_AUTHENTICATED
        {
            tokio::spawn(turn_away(stream));
            continue;
        }
        // (a secret that can't be read closes the connection, as REAPER's bridge does)
        let Ok(secret) = read_secret(&secret_file) else { continue };
        counts.waiting.fetch_add(1, Ordering::SeqCst);
        let (serving, events, ended, counts) = (serving.clone(), events.subscribe(), ended.clone(), counts.clone());
        tokio::spawn(async move {
            let authenticated = connection(stream, secret, &serving, events, ended, &counts).await;
            let count = if authenticated { &counts.authenticated } else { &counts.waiting };
            count.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Tells a connection over the limits why, and closes it without a reset: what the host sent is read for a moment
/// first, since closing a socket with unread input resets the connection, and Windows then drops the refusal unread on
/// the other side.
async fn turn_away(mut stream: TcpStream) {
    let _ = stream.write_all(&refuse_connection()).await;
    let _ = stream.shutdown().await;
    let mut unread = [0u8; 4096];
    let _ = tokio::time::timeout(TURN_AWAY, async { while matches!(stream.read(&mut unread).await, Ok(1..)) {} }).await;
}

/// One connection, until it closes or the bridge ends: whether it had authenticated (and so counts among the
/// authenticated ones), however it ended.
async fn connection(
    stream: TcpStream,
    secret: Secret,
    serving: &Serving,
    mut events: broadcast::Receiver<(String, Value)>,
    mut ended: watch::Receiver<Option<&'static str>>,
    counts: &Counts,
) -> bool {
    let (seen, offer) = (&*serving.seen, &serving.offer);
    let (mut reader, mut writer) = stream.into_split();
    let clock = Instant::now();
    let now = || clock.elapsed().as_millis() as u64;
    let mut connection = ServerConnection::new(secret, 0);
    let mut authenticated = false;
    let mut chunk = vec![0u8; 64 * 1024];
    let mut heartbeat = tokio::time::interval(Duration::from_millis(offer.heartbeat_ms.max(1)));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // (answers that come later, from work in a task of its own: reads)
    let (answers, mut answered) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    loop {
        let (mut out, mut close) = (Vec::new(), false);
        tokio::select! {
            read = reader.read(&mut chunk) => {
                let count = match read {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let mut identity = || (lock(seen).identity.clone(), offer.clone());
                for action in connection.on_bytes(&chunk[..count], now(), &mut identity) {
                    match action {
                        ServerAction::Send(bytes) => out.extend(bytes),
                        ServerAction::Close(_) => close = true,
                        ServerAction::Authenticated => {
                            authenticated = true;
                            counts.waiting.fetch_sub(1, Ordering::SeqCst);
                            counts.authenticated.fetch_add(1, Ordering::SeqCst);
                        }
                        ServerAction::Request(request) if matches!(request.method.as_str(), "read" | "snapshot") => {
                            out.extend(read_request(request, serving, answers.clone()).unwrap_or_default());
                        }
                        ServerAction::Request(request) => out.extend(answer(&request, seen).encode()),
                    }
                }
            }
            Some(bytes) = answered.recv() => out.extend(bytes),
            _ = heartbeat.tick() => {
                for action in connection.on_tick(now()) {
                    match action {
                        ServerAction::Send(bytes) => out.extend(bytes),
                        ServerAction::Close(_) => close = true,
                        _ => {}
                    }
                }
                if connection.is_authenticated() && lock(seen).busy.is_none() {
                    out.extend(Frame::Heartbeat { ms: now() }.encode());
                }
            }
            sent = events.recv() => {
                if let Ok((name, data)) = sent {
                    out.extend(connection.event(&name, data).unwrap_or_default());
                }
            }
            _ = ended.changed() => {
                if let Some(reason) = *ended.borrow() {
                    if connection.is_authenticated() {
                        out.extend(Frame::Bye { reason: reason.into() }.encode());
                    }
                    close = true;
                }
            }
        }
        if !out.is_empty() && writer.write_all(&out).await.is_err() {
            break;
        }
        if close {
            break;
        }
    }
    // (not the connection's state: one closed for a bad line after its handshake is no longer "authenticated" there,
    // and it would be taken from the waiting ones a second time)
    authenticated
}

/// A `read` or a `snapshot`: answered when the app has, in a task of its own (one read at a time), so the connection goes
/// on meanwhile. While the app is busy it's refused at once, nothing read, and one whose deadline has passed isn't started.
fn read_request(request: Request, serving: &Serving, answers: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) -> Option<Vec<u8>> {
    if let Some((_, reason)) = &lock(&serving.seen).busy {
        return Some(Frame::error(request.id, code::BUSY, format!("{reason}; nothing was read")).encode());
    }
    let Some(until) = deadline(request.deadline_ms) else {
        return Some(Frame::error(request.id, code::DEADLINE, "the request's deadline passed before the read started").encode());
    };
    let reads = serving.reads.clone();
    tokio::spawn(async move {
        let mut reads = reads.lock().await;
        let read = match request.method.as_str() {
            "snapshot" => reads.snapshot(until).await,
            _ => reads.read(&request.params, until).await,
        };
        let frame = match read {
            Ok(result) => Frame::ok(request.id, result),
            Err((code, why)) => Frame::error(request.id, code, why),
        };
        let _ = answers.send(frame.encode());
    });
    None
}

/// When a request's deadline (Unix milliseconds) falls: None once it has passed; a quarter of a minute on when it names
/// none.
fn deadline(deadline_ms: Option<u64>) -> Option<Instant> {
    let Some(deadline_ms) = deadline_ms else { return Some(Instant::now() + Duration::from_secs(15)) };
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
    (deadline_ms > now_ms).then(|| Instant::now() + Duration::from_millis(deadline_ms - now_ms))
}

/// A request's answer: `ping` and `status` from what the bridge last saw; nothing else yet.
fn answer(request: &Request, seen: &Mutex<Seen>) -> Frame {
    match request.method.as_str() {
        "ping" => Frame::ok(request.id, json!({})),
        "status" => {
            let seen = lock(seen);
            let mut status = serde_json::to_value(&seen.identity).unwrap_or_else(|_| json!({}));
            status["busy"] = seen.busy.as_ref().map_or(Value::Bool(false), |(code, _)| (*code).into());
            if let Some((_, reason)) = &seen.busy {
                status["busyReason"] = reason.as_str().into();
            }
            Frame::ok(request.id, status)
        }
        other => Frame::error(request.id, code::UNKNOWN_METHOD, format!("no method {other}")),
    }
}
