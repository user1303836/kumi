//! The bridge's loopback server: `kumi-daw/1` to Kumi's host, with daw-bridge's handshake and framing.
//!
//! `ping`, `status` and `focus` are answered from what the bridge last saw of SoundCraft, so they answer at once even
//! while the app is busy. The look loop asks the app, on its [`Schedule`]:
//! - for `ui.inspect` each second, four times a second while it plays: its windows, the playhead, a dialog;
//! - for `engine.history` each second: an edit, an undo or a redo moves it, and another session empties it;
//! - for the session (`session.inspect`'s summary) at ten times what its last look cost, from half a second to five, and
//!   sooner, as far as that cost allows, when the history moves, a plug-in window opens or closes, or the app starts
//!   playing: what's open and selected, and whether it records. The reads' lives learn from it too, so a project event
//!   goes out for another session even under the same name.
//!
//! Events go out when what they tell changed between two looks (the first look tells nothing): `project`, `transport`
//! (whether the app plays, from `ui.inspect`; whether it records and its tempo at the playhead, from the session),
//! `tracks` (the set of its tracks' ids, from the session) and `selection` (its selected tracks, from the session).
//!
//! FOCUS's `changed` moves with the history, and with the session's content when that changed with no move of the
//! history between two looks to tell it: a gesture SoundCraft merges into its last step (a fader's, an automation pass),
//! or an edit whose history reads as it did. A plug-in's parameters, which no summary shows, move neither.
//!
//! A dialog open in the app, or three looks in a row that miss their second, make the bridge busy; from the first miss
//! it asks only `ui.inspect`. A busy bridge sends no heartbeats, as the other bridges don't. The discovery file names
//! the app's pid and settings folder. It goes when the bridge ends: when the host that started it goes (its stdin ends),
//! or when SoundCraft quits.
use crate::control::{Control, Error};
use crate::launch;
use crate::read::Reads;
use crate::timebase::Timebase;
use daw_bridge::auth::Secret;
use daw_bridge::daw::Daw;
use daw_bridge::files::{process_alive, read_secret, write_discovery, Discovery, Launched, DISCOVERY_SCHEMA};
use daw_bridge::handshake::{refuse_connection, Offer, ServerAction, ServerConnection};
use daw_bridge::loopback::{self, peer_allowed};
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

/// The bridge's own version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How often the bridge looks at the app.
pub const LOOK: Duration = Duration::from_secs(1);
/// Looks in a row that miss their second before the app counts as stalled.
const MISSES_BEFORE_STALLED: u32 = 3;
/// `ui.inspect`s to a look while the app plays: the playhead FOCUS's beat light follows.
const UI_LOOKS_WHILE_PLAYING: u32 = 4;
/// A look at the session waits ten times what the last one cost (on a big session a `session.inspect` costs the app
/// frames), from half a look to five; something it would show changing brings it forward as far as that cost allows.
const SESSION_WAIT_PER_COST: u32 = 10;
/// Looks a look at the session may take, so a summary slower than a look is still seen.
const SESSION_LOOKS: u32 = 5;
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
    /// The last `ui.inspect`, and when its answer came: the windows and the playhead.
    ui: Option<(Value, Instant)>,
    /// What the last look at the session saw for FOCUS.
    glance: Option<Glance>,
    /// How many times the session changed since the bridge started, as the looks saw it: each move of the undo history,
    /// and each change of the session's content that no move told between two looks.
    changed: u64,
}

/// What a look at the session saw for FOCUS, in the refs of its epoch.
struct Glance {
    epoch: String,
    selection: Value,
    /// The plug-in window it named the plug-in of (`[track, slot]`, the last opened then), with its `fxRef`, `fxName`,
    /// `fxTrackRef` and `fxTrackName`.
    plugin: Option<(Value, Value)>,
    /// The ref of the clip the MIDI Editor shows ([`Reads::midi_editor_clip`]).
    midi_clip: Option<String>,
    /// Every selected track, by its ref and name.
    selected: Vec<(String, Value)>,
    /// The summary itself: the rate and the tempo and meter maps the playhead is read through, and whether it records.
    session: Arc<Value>,
}

fn lock(seen: &Mutex<Seen>) -> MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The listener the bridge serves on: 127.0.0.1, the first free port of SoundCraft's range.
pub fn bind() -> std::io::Result<std::net::TcpListener> {
    loopback::bind(Daw::SoundCraft, None)
}

/// Serves on `listener` ([`bind`]'s, or a test's) until `stop` comes or SoundCraft quits, and says which. The discovery
/// file is written before it serves and removed when it ends.
pub async fn serve(listener: std::net::TcpListener, config: Config, stop: impl Future<Output = ()>) -> Result<End, String> {
    let control = launch::control(&config.launched);
    let listening = |error: std::io::Error| format!("the bridge can't listen on loopback: {error}");
    listener.set_nonblocking(true).map_err(listening)?;
    let listener = TcpListener::from_std(listener).map_err(listening)?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();
    let reads = Reads::new(control.clone(), config.launched.version.clone());
    let epochs = reads.epochs();
    let reads = Arc::new(tokio::sync::Mutex::new(reads));
    let (project, glance) = match look_at_session(&control, &reads, &epochs, None, config.look * SESSION_LOOKS, config.look).await {
        Some(looked) => (Some(looked.project), looked.glance),
        None => (None, None),
    };
    let identity = Identity {
        preset_files: vec![],
        daw: Daw::SoundCraft.id().into(),
        bridge: VERSION.into(),
        daw_version: config.launched.version.clone().unwrap_or_default(),
        pid: config.launched.pid,
        resource: config.resource.clone(),
        project,
    };
    let started_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
    let notice = move |identity: &Identity| Discovery {
        schema: DISCOVERY_SCHEMA.into(),
        daw: Daw::SoundCraft.id().into(),
        pid: identity.pid,
        host: loopback::BIND_ADDRESS.to_string(),
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
    let seen = Arc::new(Mutex::new(Seen { identity, busy: None, ui: None, glance, changed: 0 }));
    let (events, _) = broadcast::channel::<(String, Value)>(64);
    let (ending, ended) = watch::channel(None::<&'static str>);
    let offer = Offer {
        methods: vec!["ping".into(), "status".into(), "focus".into(), "read".into(), "snapshot".into(), "focus_body".into()],
        events: vec![event::PROJECT.into(), event::TRANSPORT.into(), event::TRACKS.into(), event::SELECTION.into()],
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

/// What a look at the session saw.
struct Looked {
    project: Project,
    /// What FOCUS shows of it, unless a read held the reads (that read looks itself, and FOCUS keeps what it saw).
    glance: Option<Glance>,
    /// Its content ([`crate::read::content`]), for `changed`.
    content: u64,
}

/// A look at the session (`session.inspect`, its summary, within `deadline`): what's open now, with the epoch of its
/// objects' refs, its content, and what FOCUS shows of it, the plug-in in `window` among it (named within `naming`). The
/// reads' lives learn from it, unless a read holds them; the epoch is told either way. Nothing while the app doesn't
/// answer.
async fn look_at_session(
    control: &Control,
    reads: &tokio::sync::Mutex<Reads>,
    epochs: &watch::Receiver<String>,
    window: Option<Value>,
    deadline: Duration,
    naming: Duration,
) -> Option<Looked> {
    let session = Arc::new(control.call("session.inspect", json!({}), deadline).await.ok()?);
    let content = crate::read::content(&session);
    let text = |key: &str| session.get(key).and_then(Value::as_str).map(str::to_string);
    let (name, path) = (text("name"), text("path"));
    let glance = match reads.try_lock() {
        Ok(mut reads) => {
            reads.looked_at(Arc::clone(&session), content);
            let plugin = match window {
                Some(window) => reads.plugin_window(&session, &window, Instant::now() + naming).await.map(|named| (window, named)),
                None => None,
            };
            let midi_clip = reads.midi_editor_clip(&session);
            let (epoch, selection, selected) = (reads.epoch().into(), reads.selection(&session), reads.selected_tracks(&session));
            Some(Glance { epoch, selection, plugin, midi_clip, selected, session })
        }
        Err(_) => None,
    };
    let mut extra = Map::new();
    extra.insert("epoch".into(), epochs.borrow().as_str().into());
    Some(Looked { project: Project { name, path, extra, ..Project::default() }, glance, content })
}

/// When the look loop next asks the app for each thing: `ui.inspect` each look (each quarter of one while the app plays),
/// `engine.history` each look, and the session ten times what its last look cost after it, from half a look to five, or
/// sooner, as far as that cost allows, when something it would show changed. Its clock is the caller's.
#[derive(Debug, Clone)]
pub struct Schedule {
    every: Duration,
    ui: Instant,
    history: Instant,
    session: Instant,
    /// The last look at the session: when it was asked, and what it cost.
    last: Option<(Instant, Duration)>,
}

impl Schedule {
    /// Each look `every` apart, all of them due at `now`.
    pub fn new(every: Duration, now: Instant) -> Self {
        Self { every, ui: now, history: now, session: now, last: None }
    }

    /// How often the loop wakes: a quarter of a look, the playhead's pace while the app plays.
    pub fn tick(&self) -> Duration {
        self.every / UI_LOOKS_WHILE_PLAYING
    }

    pub fn ui_due(&self, now: Instant) -> bool {
        now >= self.ui
    }

    pub fn history_due(&self, now: Instant) -> bool {
        now >= self.history
    }

    pub fn session_due(&self, now: Instant) -> bool {
        now >= self.session
    }

    /// A `ui.inspect` asked at `asked`: the next a look on, or a quarter of one while the app plays.
    pub fn looked_at_ui(&mut self, asked: Instant, playing: bool) {
        self.ui = asked + if playing { self.tick() } else { self.every };
    }

    /// An `engine.history` asked at `asked`: the next a look on.
    pub fn looked_at_history(&mut self, asked: Instant) {
        self.history = asked + self.every;
    }

    /// A look at the session asked at `asked` and over (answered or given up) at `ended`: the next ten times its cost on,
    /// from half a look to five.
    pub fn looked_at_session(&mut self, asked: Instant, ended: Instant) {
        let cost = ended.saturating_duration_since(asked);
        self.last = Some((asked, cost));
        self.session = ended + (cost * SESSION_WAIT_PER_COST).clamp(self.every / 2, self.every * SESSION_LOOKS);
    }

    /// Something a look at the session would show changed at `now` (the history moved, a plug-in window opened or
    /// closed, the app started playing): the next look as soon as its cost allows, ten times the last one's cost after it
    /// was asked, with no half-look's floor.
    pub fn soon(&mut self, now: Instant) {
        let allowed = self.last.map_or(now, |(asked, cost)| now.max(asked + cost * SESSION_WAIT_PER_COST));
        self.session = self.session.min(allowed);
    }

    /// How long a look at the session may take: ten times what the last one cost, from a look to five (five before any
    /// cost is known). So a stall a look at the session meets first is given up on within a look, as a `ui.inspect` is,
    /// and a summary slower than a look is still seen from its second try.
    pub fn session_deadline(&self) -> Duration {
        let wait = |cost: Duration| (cost * SESSION_WAIT_PER_COST).clamp(self.every, self.every * SESSION_LOOKS);
        self.last.map_or(self.every * SESSION_LOOKS, |(_, cost)| wait(cost))
    }
}

/// What the events last told, from the looks that saw it: the transport, the set of the tracks' ids, and the selected
/// tracks. Each is told when a look sees it otherwise than the last did; the first look at it tells nothing.
#[derive(Default)]
struct Told {
    /// Whether the session plays and records, and its tempo at the playhead: all from one look at the session, so a take
    /// started from a stop is told recording from the first. A `ui.inspect` tells only a stop, at once.
    transport: Option<Value>,
    /// The tracks' ids and the selected tracks, each with the epoch it was seen in: another session on the same ids
    /// (SoundCraft numbers a new session's objects from 1 again) is news, and never named in the last one's words.
    tracks: Option<(String, Vec<u64>)>,
    selection: Option<(String, Vec<String>)>,
}

impl Told {
    /// A `ui.inspect` saw the app playing or not: a stop is told at once. A start waits for the look at the session,
    /// which says whether it records.
    fn played(&mut self, playing: bool) -> Vec<(&'static str, Value)> {
        let Some(told) = self.transport.as_ref().filter(|told| !playing && told["playing"] == true) else { return vec![] };
        let now = json!({"playing": false, "recording": false, "bpm": told["bpm"].clone()});
        self.transport = Some(now.clone());
        vec![(event::TRANSPORT, now)]
    }

    /// A look at the session saw `glance`, with the playhead at `position` samples: the transport, the tracks and the
    /// selection, each if it's news.
    fn looked(&mut self, glance: &Glance, position: &Value) -> Vec<(&'static str, Value)> {
        let session = &*glance.session;
        let bpm = match (Timebase::of(session), position.as_f64()) {
            (Some(timebase), Some(samples)) => timebase.tempo(timebase.beats(samples)).into(),
            _ => session["tempo"][0]["bpm"].clone(),
        };
        // (a stopped take doesn't record)
        let playing = session["transport"]["playing"] == true;
        let transport = json!({"playing": playing, "recording": playing && session["transport"]["recording"] == true, "bpm": bpm});
        let mut news = Vec::new();
        if changed(&mut self.transport, transport.clone()) {
            news.push((event::TRANSPORT, transport));
        }
        let mut ids: Vec<u64> = session["tracks"].as_array().into_iter().flatten().filter_map(|track| track["id"].as_u64()).collect();
        ids.sort_unstable();
        let count = ids.len();
        if changed(&mut self.tracks, (glance.epoch.clone(), ids)) {
            news.push((event::TRACKS, json!({"count": count, "epoch": glance.epoch})));
        }
        let refs = glance.selected.iter().map(|(key, _)| key.clone()).collect();
        if changed(&mut self.selection, (glance.epoch.clone(), refs)) {
            let tracks: Vec<Value> = glance.selected.iter().map(|(key, name)| json!({"guid": key, "name": name})).collect();
            news.push((event::SELECTION, json!({"tracks": tracks})));
        }
        news
    }
}

/// Keeps `now` as what was told, and says whether it's news: something was told before, and not this.
fn changed<T: PartialEq>(told: &mut Option<T>, now: T) -> bool {
    let news = told.as_ref().is_some_and(|before| *before != now);
    *told = Some(now);
    news
}

/// Looks at the app on its [`Schedule`] until it quits, keeping what was seen and telling the connections what changed.
/// From a missed `ui.inspect` or while the app is busy, it asks nothing else.
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
    let mut schedule = Schedule::new(every, Instant::now());
    let mut tick = tokio::time::interval(schedule.tick());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // (a look is due at its tick, though the tick comes a little early)
    let at_tick = |schedule: &Schedule| Instant::now() + schedule.tick() / 2;
    let (mut misses, mut playing, mut windows) = (0u32, false, Value::Null);
    // (the history the last poll saw; the content the last look at the session saw, and whether a move was counted since)
    let (mut history, mut content, mut moved) = (None::<Value>, None::<u64>, false);
    let mut told = Told::default();
    let tell = |news: Vec<(&'static str, Value)>| {
        for (name, data) in news {
            let _ = events.send((name.into(), data));
        }
    };
    loop {
        tick.tick().await;
        if schedule.ui_due(at_tick(&schedule)) {
            let asked = Instant::now();
            let busy = match control.call("ui.inspect", json!({}), every).await {
                Ok(ui) => {
                    misses = 0;
                    // A plug-in window opened or closed, a take may have started, or a dialog closed (a save's, which
                    // names the session after its file: seen before the next edit, it's the same session): the session is
                    // looked at soon, for the plug-in FOCUS shows, whether it records, and what the dialog did.
                    let (now_playing, busy) = (ui["playing"] == true, dialog(&ui));
                    let closed = busy.is_none() && lock(seen).busy.is_some();
                    if ui["ui"]["plugin_windows"] != windows || (now_playing && !playing) || closed {
                        schedule.soon(asked);
                    }
                    (windows, playing) = (ui["ui"]["plugin_windows"].clone(), now_playing);
                    lock(seen).ui = Some((ui, Instant::now()));
                    tell(told.played(now_playing));
                    busy
                }
                Err(Error::Unreachable(_) | Error::Closed) if !process_alive(pid) => return End::AppQuit,
                Err(_) => {
                    misses += 1;
                    playing = false;
                    match misses >= MISSES_BEFORE_STALLED {
                        true => Some(("stalled", "SoundCraft isn't answering".to_string())),
                        false => lock(seen).busy.clone(),
                    }
                }
            };
            lock(seen).busy = busy;
            schedule.looked_at_ui(asked, playing);
        }
        if misses > 0 || lock(seen).busy.is_some() {
            continue;
        }
        if schedule.history_due(at_tick(&schedule)) {
            schedule.looked_at_history(Instant::now());
            if let Ok(now) = control.execute("engine.history", json!({}), every).await {
                if history.as_ref().is_some_and(|before| *before != now) {
                    lock(seen).changed += 1;
                    moved = true;
                    schedule.soon(Instant::now());
                }
                history = Some(now);
            }
        }
        if !schedule.session_due(at_tick(&schedule)) {
            continue;
        }
        let asked = Instant::now();
        let window = windows.as_array().and_then(|open| open.last()).cloned();
        let looked = look_at_session(control, reads, epochs, window, schedule.session_deadline(), every).await;
        schedule.looked_at_session(asked, Instant::now());
        let Some(looked) = looked else { continue };
        let position = lock(seen).ui.as_ref().map_or(Value::Null, |(ui, _)| ui["position"].clone());
        let news = looked.glance.as_ref().map(|glance| told.looked(glance, &position)).unwrap_or_default();
        let mut guard = lock(seen);
        // (the session changed with no move of the history to tell it: a merged gesture, or an edit whose history reads
        // as it did)
        if content.is_some_and(|before| before != looked.content) && !moved {
            guard.changed += 1;
        }
        (content, moved) = (Some(looked.content), false);
        if looked.glance.is_some() {
            guard.glance = looked.glance;
        }
        // (another session first: the tracks and selection after it are its)
        if guard.identity.project.as_ref() != Some(&looked.project) {
            guard.identity.project = Some(looked.project.clone());
            let identity = guard.identity.clone();
            drop(guard);
            rewrite(&identity);
            let _ = events.send((event::PROJECT.into(), serde_json::to_value(&looked.project).unwrap_or(Value::Null)));
        } else {
            drop(guard);
        }
        tell(news);
    }
}

/// A dialog open in the app, as the bridge's reason to be busy.
fn dialog(ui: &Value) -> Option<(&'static str, String)> {
    ui.get("dialog").filter(|dialog| !dialog.is_null()).map(|dialog| {
        let named = dialog.as_str().map(str::to_string).unwrap_or_else(|| dialog.to_string());
        ("modal", format!("a dialog is open in SoundCraft ({named})"))
    })
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
                        ServerAction::Request(request) if matches!(request.method.as_str(), "read" | "snapshot" | "focus_body") => {
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

/// A `read`, a `snapshot` or a `focus_body`: answered when the app has, in a task of its own (one read at a time), so the
/// connection goes on meanwhile. While the app is busy it's refused at once, nothing read, and one whose deadline has
/// passed isn't started. A body is drawn with the app's windows as the last look saw them, and its refs, which come with
/// no epoch from FOCUS (it takes them from `focus`), are read in the epoch the last look saw: a ref from before another
/// session opened is stale, not read as the new session's.
fn read_request(mut request: Request, serving: &Serving, answers: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) -> Option<Vec<u8>> {
    if let Some((_, reason)) = &lock(&serving.seen).busy {
        return Some(Frame::error(request.id, code::BUSY, format!("{reason}; nothing was read")).encode());
    }
    let Some(until) = deadline(request.deadline_ms) else {
        return Some(Frame::error(request.id, code::DEADLINE, "the request's deadline passed before the read started").encode());
    };
    let mut ui = None;
    if request.method == "focus_body" {
        let seen = lock(&serving.seen);
        ui = seen.ui.as_ref().map(|(ui, _)| ui.clone());
        let given = |key: &str| request.params.get(key).is_some_and(|value| !value.is_null());
        if !given("epoch") && (given("ref") || given("around")) {
            if let Some(glance) = &seen.glance {
                request.params["epoch"] = glance.epoch.as_str().into();
            }
        }
    }
    let reads = serving.reads.clone();
    tokio::spawn(async move {
        let mut reads = reads.lock().await;
        let read = match request.method.as_str() {
            "snapshot" => reads.snapshot(until).await,
            "focus_body" => reads.focus_body(&request.params, ui.as_ref(), until).await,
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

/// A request's answer: `ping`, `status` and `focus` from what the bridge last saw; nothing else yet.
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
        "focus" => focus(request.id, &lock(seen)),
        other => Frame::error(request.id, code::UNKNOWN_METHOD, format!("no method {other}")),
    }
}

/// FOCUS as the last looks saw it:
/// - `view`, from the app's windows;
/// - `selection`, in the refs of the epoch the last look at the session saw; in a plug-in's window that plug-in, and in
///   the MIDI Editor the clip it shows (`midiClipRef`);
/// - `transport` where the playhead is now: while the app plays, it's moved on by the time since the look;
/// - `changed`, which moves when the session changes (the module's doc says how the looks see it), and `ageMs`, how old
///   the look is;
/// - `busy` only while the app is (Kumi takes any `busy` there for busy).
fn focus(id: u64, seen: &Seen) -> Frame {
    let (Some((ui, at)), Some(glance)) = (&seen.ui, &seen.glance) else {
        return Frame::error(id, code::BUSY, "the bridge hasn't seen SoundCraft yet");
    };
    let view = view(ui);
    let mut selection = glance.selection.clone();
    let window = ui["ui"]["plugin_windows"].as_array().and_then(|open| open.last());
    if let Some((_, named)) = glance.plugin.as_ref().filter(|(seen, _)| view == "fx" && Some(seen) == window) {
        for key in ["fxRef", "fxName", "fxTrackRef", "fxTrackName"] {
            selection[key] = named[key].clone();
        }
    }
    if let Some(clip) = glance.midi_clip.as_ref().filter(|_| view == "midi") {
        selection["midiClipRef"] = clip.as_str().into();
    }
    let playing = ui["playing"] == true;
    let mut position = ui["position"].clone();
    if let (true, Some(samples), Some(rate)) = (playing, position.as_f64(), glance.session["sample_rate"].as_f64()) {
        position = (samples + at.elapsed().as_secs_f64() * rate).round().into();
    }
    // (the session's look says whether it records, the UI's whether it plays: a recording that has stopped doesn't)
    let recording = playing && glance.session["transport"]["recording"] == true;
    let mut answer = json!({
        "epoch": glance.epoch,
        "view": view,
        "selection": selection,
        "transport": crate::read::transport(&glance.session, &position, &playing.into(), &recording.into()),
        "changed": seen.changed,
        "ageMs": at.elapsed().as_millis() as u64,
    });
    if let Some((code, _)) = &seen.busy {
        answer["busy"] = (*code).into();
    }
    Frame::ok(id, answer)
}

/// What the producer looks at, from `ui.inspect`: a plug-in's window (`fx`); the MIDI Editor (`midi`), a panel of the
/// Edit window, so only while the main window draws Edit (it always does once Mix has a window of its own); else the
/// Mix window (`mixer`) or the Edit window (`edit`).
fn view(ui: &Value) -> &'static str {
    let shown = &ui["ui"];
    let mix_in_main = shown["window"] == "Mix" && shown["mix_separate"] != true;
    if shown["plugin_windows"].as_array().is_some_and(|open| !open.is_empty()) {
        "fx"
    } else if !mix_in_main && shown["show_midi_editor"] == true {
        "midi"
    } else if shown["window"] == "Mix" {
        "mixer"
    } else {
        "edit"
    }
}
