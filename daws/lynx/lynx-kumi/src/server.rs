//! The standard own-bridge server. Cached FOCUS/transport never waits for a plugin.
use crate::{
    native,
    project::{self, Bridge},
    unix_ms, Error, Result,
};
use daw_bridge::{
    files::{self, Discovery, DISCOVERY_SCHEMA},
    handshake::{refuse_connection, Offer, ServerAction, ServerConnection},
    loopback::peer_allowed,
    wire::{code, event, Frame, Identity, Project, Request, HEARTBEAT_MS, MAX_AUTHENTICATED, MAX_UNAUTHENTICATED, PROTOCOL},
};
use serde_json::{json, Value};
use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{broadcast, watch, Semaphore},
};

pub const DAW: &str = "lynx";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub struct Config {
    pub native_config: PathBuf,
    pub secret_file: PathBuf,
    pub discovery_dir: PathBuf,
    pub poll: Duration,
    pub serving: Option<std::fs::File>,
}
impl Config {
    pub fn new(native_config: PathBuf, secret_file: PathBuf, discovery_dir: PathBuf) -> Self {
        Self { native_config, secret_file, discovery_dir, poll: Duration::from_millis(500), serving: None }
    }
}

#[derive(Clone)]
struct Seen {
    session: Value,
    identity: Identity,
    at: Instant,
    busy: bool,
}
fn seen_lock(seen: &Mutex<Seen>) -> std::sync::MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Notice(PathBuf);
impl Drop for Notice {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Serves one native process until the host's stdin ends, a stop arrives or LYNX quits.
pub async fn serve(config: Config, stop: impl Future<Output = ()>) -> Result<()> {
    files::read_secret(&config.secret_file).map_err(|_| Error::bad("The own-bridge secret is missing or not owner-only"))?;
    let found = native::discovery(&config.native_config)?;
    let native_pid = found.pid;
    let mut bridge = Bridge::new(native::Client::for_pid(config.native_config.clone(), native_pid));
    let session = bridge.session().await?;
    let identity = identity(&session, native_pid, found.app_version, config.native_config.to_string_lossy().into_owned());
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| Error::new(code::FAILED, "Cannot listen on loopback for Kumi's host"))?;
    let port = listener.local_addr().map_err(|_| Error::new(code::FAILED, "Cannot identify the own-bridge port"))?.port();
    let started_ms = unix_ms();
    let discovery = notice(&identity, port, started_ms);
    files::write_discovery(&config.discovery_dir, &discovery)
        .map_err(|_| Error::new(code::FAILED, "Cannot publish private own-bridge discovery"))?;
    let notice_guard = Notice(config.discovery_dir.join(discovery.file_name()));
    let seen = Arc::new(Mutex::new(Seen { session, identity, at: Instant::now(), busy: false }));
    let work = Arc::new(tokio::sync::Mutex::new(bridge));
    let (events, _) = broadcast::channel(64);
    let (ending, ended) = watch::channel(false);
    let accepting = tokio::spawn(accept(listener, config.secret_file.clone(), work.clone(), seen.clone(), events.clone(), ended));
    let watching = look(work, seen, events, config.poll, config.discovery_dir.clone(), port, started_ms);
    tokio::select! {()=stop=>{},()=watching=>{}}
    drop(notice_guard);
    drop(config.serving);
    accepting.abort();
    let _ = ending.send(true);
    tokio::task::yield_now().await;
    Ok(())
}

fn identity(session: &Value, pid: u32, version: String, resource: String) -> Identity {
    Identity {
        preset_files: vec![],
        daw: DAW.into(),
        bridge: VERSION.into(),
        daw_version: session["appVersion"].as_str().unwrap_or(&version).into(),
        pid,
        resource,
        project: Some(native_project(session)),
    }
}
fn native_project(session: &Value) -> Project {
    Project {
        id: session.pointer("/project/id").and_then(Value::as_str).map(str::to_owned),
        name: session.pointer("/project/name").and_then(Value::as_str).map(str::to_owned),
        path: session.pointer("/project/path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned),
        extra: serde_json::from_value(json!({"epoch":project::epoch(session)})).unwrap_or_default(),
    }
}
fn notice(identity: &Identity, port: u16, started_ms: u64) -> Discovery {
    Discovery {
        schema: DISCOVERY_SCHEMA.into(),
        daw: DAW.into(),
        pid: identity.pid,
        host: "127.0.0.1".into(),
        port,
        protocol: PROTOCOL.into(),
        version: VERSION.into(),
        daw_version: identity.daw_version.clone(),
        resource: identity.resource.clone(),
        project: identity.project.clone(),
        started_ms,
    }
}

async fn look(
    work: Arc<tokio::sync::Mutex<Bridge>>,
    seen: Arc<Mutex<Seen>>,
    events: broadcast::Sender<(String, Value)>,
    every: Duration,
    dir: PathBuf,
    port: u16,
    started: u64,
) {
    let mut tick = tokio::time::interval(every.max(Duration::from_millis(50)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if !files::process_alive(seen_lock(&seen).identity.pid) {
            break;
        }
        let Ok(mut work) = work.try_lock() else { continue };
        let session = match work.session().await {
            Ok(s) => s,
            Err(_) => {
                seen_lock(&seen).busy = true;
                continue;
            }
        };
        let mut old = seen_lock(&seen);
        let project_changed = old.session["controlEpoch"] != session["controlEpoch"]
            || ["id", "name", "path"].iter().any(|field| old.session["project"][*field] != session["project"][*field]);
        let changed = old.session["revision"] != session["revision"];
        let transport = project::transport(&session);
        if project::transport(&old.session) != transport {
            let _ = events.send((event::TRANSPORT.into(), transport));
        }
        if changed {
            let _ = events.send((event::TRACKS.into(), json!({"epoch":project::epoch(&session),"changed":session["revision"]})));
        }
        if old.session["currentPattern"] != session["currentPattern"] {
            let _ = events.send((event::SELECTION.into(), project::selection(&session)));
        }
        old.identity.project = Some(native_project(&session));
        old.session = session;
        old.at = Instant::now();
        old.busy = false;
        if project_changed {
            let _ = events.send((event::PROJECT.into(), serde_json::to_value(&old.identity.project).unwrap_or(Value::Null)));
        }
        if project_changed || changed {
            let _ = files::write_discovery(&dir, &notice(&old.identity, port, started));
        }
    }
}

fn offer() -> Offer {
    Offer {
        methods: [
            "ping",
            "status",
            "focus",
            "read",
            "snapshot",
            "focus_body",
            "digest",
            "change_preview",
            "change_apply",
            "change_restore",
            "transport_play",
            "transport_stop",
        ]
        .iter()
        .map(|s| (*s).into())
        .collect(),
        events: [event::PROJECT, event::TRANSPORT, event::TRACKS, event::SELECTION].iter().map(|s| (*s).into()).collect(),
        heartbeat_ms: HEARTBEAT_MS,
        credit_bytes: None,
    }
}

async fn accept(
    listener: TcpListener,
    secret_file: PathBuf,
    work: Arc<tokio::sync::Mutex<Bridge>>,
    seen: Arc<Mutex<Seen>>,
    events: broadcast::Sender<(String, Value)>,
    ended: watch::Receiver<bool>,
) {
    let waiting = Arc::new(Semaphore::new(MAX_UNAUTHENTICATED));
    let authenticated = Arc::new(Semaphore::new(MAX_AUTHENTICATED));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(a) => a,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        if !peer_allowed(&peer) {
            continue;
        }
        let permit = match waiting.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                tokio::spawn(refuse(stream));
                continue;
            }
        };
        if authenticated.available_permits() == 0 {
            tokio::spawn(refuse(stream));
            continue;
        }
        let Ok(secret) = files::read_secret(&secret_file) else { continue };
        let (work, seen, events, ended, authenticated) =
            (work.clone(), seen.clone(), events.subscribe(), ended.clone(), authenticated.clone());
        tokio::spawn(connection(stream, secret, Admission { waiting: permit, authenticated }, work, seen, events, ended));
    }
}
async fn refuse(mut stream: TcpStream) {
    let _ = stream.write_all(&refuse_connection()).await;
    let _ = stream.shutdown().await;
}

struct Admission {
    waiting: tokio::sync::OwnedSemaphorePermit,
    authenticated: Arc<Semaphore>,
}
async fn connection(
    stream: TcpStream,
    secret: daw_bridge::auth::Secret,
    admission: Admission,
    work: Arc<tokio::sync::Mutex<Bridge>>,
    seen: Arc<Mutex<Seen>>,
    mut events: broadcast::Receiver<(String, Value)>,
    mut ended: watch::Receiver<bool>,
) {
    let Admission { waiting: permit, authenticated } = admission;
    let (mut reader, mut writer) = stream.into_split();
    let clock = Instant::now();
    let mut protocol = ServerConnection::new(secret, 0);
    let (mut pending_auth, mut active) = (Some(permit), None);
    let mut bytes = vec![0; 64 * 1024];
    let mut heartbeat = tokio::time::interval(Duration::from_millis(HEARTBEAT_MS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (answered, mut answers) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let requests = Arc::new(Semaphore::new(8));
    let stops = Arc::new(Semaphore::new(1));
    loop {
        let (mut out, mut close) = (Vec::new(), false);
        tokio::select! {
            got=reader.read(&mut bytes)=>{
                let count=match got{Ok(0)|Err(_)=>break,Ok(n)=>n};
                let mut identity=||(seen_lock(&seen).identity.clone(),offer());
                for action in protocol.on_bytes(&bytes[..count],clock.elapsed().as_millis() as u64,&mut identity){match action{
                    ServerAction::Send(bytes)=>out.extend(bytes),ServerAction::Close(_)=>close=true,
                    ServerAction::Authenticated=>match authenticated.clone().try_acquire_owned(){Ok(p)=>{active=Some(p);pending_auth.take();},Err(_)=>close=true},
                    ServerAction::Request(request)=>{
                        if let Some(frame)=cached(&request,&seen){out.extend(frame.encode());continue;}
                        if request.params.to_string().len()>native::MAX_MESSAGE{out.extend(Frame::error(request.id,"limit","Request params exceed the native message bound").encode());continue;}
                        let emergency=request.method=="transport_stop";
                        let pool=if emergency{stops.clone()}else{requests.clone()};
                        let Ok(permit)=pool.try_acquire_owned()else{out.extend(Frame::error(request.id,code::BUSY,"This connection already has its allowed outstanding requests").encode());continue};
                        let stop_client=if emergency{let seen=seen_lock(&seen);Some(native::Client::for_pid(PathBuf::from(&seen.identity.resource),seen.identity.pid))}else{None};
                        let(work,answered)=(work.clone(),answered.clone());
                        tokio::spawn(async move{
                            let frame=run(request,work,stop_client).await;
                            let _=answered.send(frame.encode()).await;drop(permit);
                        });
                    }
                }}
            }
            Some(answer)=answers.recv()=>out.extend(answer),
            _=heartbeat.tick()=>{
                for action in protocol.on_tick(clock.elapsed().as_millis() as u64){match action{ServerAction::Send(bytes)=>out.extend(bytes),ServerAction::Close(_)=>close=true,_=>{}}}
                if active.is_some()&&!seen_lock(&seen).busy{out.extend(Frame::Heartbeat{ms:clock.elapsed().as_millis() as u64}.encode());}
            }
            event=events.recv()=>{match event{Ok((name,data))=>out.extend(protocol.event(&name,data).unwrap_or_default()),Err(broadcast::error::RecvError::Lagged(_))=>{out.extend(protocol.event(event::TRACKS,json!({"gap":true})).unwrap_or_default());},Err(_)=>{}}}
            _=ended.changed()=>{if *ended.borrow(){out.extend(Frame::Bye{reason:"the LYNX bridge stopped".into()}.encode());close=true;}}
        }
        if !out.is_empty() && writer.write_all(&out).await.is_err() {
            break;
        }
        if close {
            break;
        }
    }
}

fn cached(request: &Request, seen: &Mutex<Seen>) -> Option<Frame> {
    if request.deadline_ms.is_some_and(|until| until <= unix_ms()) {
        return Some(Frame::error(request.id, code::DEADLINE, "The request deadline passed before work began"));
    }
    let seen = seen_lock(seen);
    Some(match request.method.as_str() {
        "ping" => Frame::ok(request.id, json!({})),
        "status" => {
            let mut status = serde_json::to_value(&seen.identity).unwrap_or(json!({}));
            status["busy"] = seen.busy.into();
            Frame::ok(request.id, status)
        }
        "focus" => {
            let mut transport = project::transport(&seen.session);
            if transport["playing"] == true {
                transport["positionBeats"] = (transport["positionBeats"].as_f64().unwrap_or(0.)
                    + seen.at.elapsed().as_secs_f64() * transport["tempo"].as_f64().unwrap_or(120.) / 60.)
                    .into();
            }
            let mut focus = json!({"epoch":project::epoch(&seen.session),"layout":seen.session["revision"],"view":if seen.session["songMode"]==true{"playlist"}else{"pattern"},
                "selection":project::selection(&seen.session),"transport":transport,"changed":seen.session["revision"],"ageMs":seen.at.elapsed().as_millis() as u64});
            if seen.busy {
                focus["busy"] = "stalled".into();
            }
            Frame::ok(request.id, focus)
        }
        _ => return None,
    })
}

async fn run(request: Request, work: Arc<tokio::sync::Mutex<Bridge>>, stop_client: Option<native::Client>) -> Frame {
    let until = request.deadline_ms.unwrap_or_else(|| unix_ms() + 65_000);
    if until <= unix_ms() {
        return Frame::error(request.id, code::DEADLINE, "The request deadline passed before work began");
    }
    let writing = matches!(request.method.as_str(), "change_apply" | "change_restore" | "transport_play" | "transport_stop");
    let future = async {
        // Emergency stop has no project/revision fence and bypasses the companion
        // work queue, so a queued inspection or plugin job cannot hold it here.
        if let Some(client) = stop_client {
            return client.call("transport.stop", json!({})).await;
        }
        let mut bridge = work.lock().await;
        if until <= unix_ms() {
            return Err(Error::new(code::DEADLINE, "The request expired in the work queue; nothing was sent"));
        }
        match request.method.as_str() {
            "read" => bridge.read(&request.params).await,
            "snapshot" => bridge.snapshot().await,
            "focus_body" => bridge.focus_body(&request.params).await,
            "digest" => bridge.digest(&request.params).await,
            "change_preview" => bridge.preview(&request.params).await,
            "change_apply" => bridge.apply(&request.params).await,
            "change_restore" => bridge.restore(&request.params).await,
            "transport_play" => bridge.transport_action(true).await,
            "transport_stop" => bridge.transport_action(false).await,
            _ => Err(Error::new(code::UNKNOWN_METHOD, "This method is not implemented by the LYNX own bridge")),
        }
    };
    let result = tokio::time::timeout(Duration::from_millis(until.saturating_sub(unix_ms())), future).await;
    match result {
        Ok(Ok(answer)) => Frame::ok(request.id, answer),
        Ok(Err(error)) => Frame::error(request.id, error.code, error.message),
        Err(_) => Frame::error(
            request.id,
            if writing { code::UNCERTAIN } else { code::BUSY },
            if writing {
                "The deadline passed after work started; a sent change may have run. Inspect before retrying"
            } else {
                "The native read did not finish before its deadline"
            },
        ),
    }
}
