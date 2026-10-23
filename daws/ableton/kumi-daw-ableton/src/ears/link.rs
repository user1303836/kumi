//! Kumi's end of its listening devices: a UDP socket on loopback, requests answered by token.

use super::{
    device::{EARS_VERSION, KUMI_PORTS},
    osc::{decode_osc_packet, encode_osc, OscArg, OscMessage, OscValue},
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{abort::Signal, time::now_ms};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::{broadcast, mpsc},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tap {
    pub id: f64,
    pub port: u16,
    pub path: String,
    pub version: f64,
    pub sample_rate: f64,
    pub seen_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Armed {
    pub beats: f64,
    pub running: bool,
    pub sample_rate: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    pub file: String,
    pub sample_rate: f64,
    pub channels: usize,
    pub beats: f64,
    pub running: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transport {
    pub beats: f64,
    pub running: bool,
}
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct EarsError(pub String);
#[async_trait(?Send)]
pub trait EarsLink {
    fn port(&self) -> u16;
    fn taps(&self) -> Vec<Tap>;
    async fn wait_for(&self, accept: Rc<dyn for<'a> Fn(&'a Tap) -> bool>, timeout_ms: u64, signal: Option<Signal>) -> Option<Tap>;
    /// Starts a recording of up to `seconds`. `quiet` holds the track's sound back meanwhile (a candidate only Kumi
    /// hears): the device lets it pass again when the recording is written or stopped, or its time is up.
    async fn arm(&self, tap: &Tap, seconds: f64, quiet: bool, signal: Option<Signal>) -> Result<Armed, EarsError>;
    async fn write(&self, tap: &Tap, file: &str, signal: Option<Signal>) -> Result<Written, EarsError>;
    fn stop(&self, tap: &Tap);
    async fn ping(&self, tap: &Tap, signal: Option<Signal>) -> Option<Tap>;
    async fn transport(&self, tap: &Tap, signal: Option<Signal>) -> Result<Option<Transport>, EarsError>;
    async fn close(&self);
}
#[derive(Debug, Clone, Default)]
pub struct EarsOptions {
    pub port: Option<u16>,
    pub ports: Option<Vec<u16>>,
}
const GONE_MS: i64 = 7000;
const REPLY_MS: u64 = 3000;
struct State {
    known: RefCell<IndexMap<u64, Tap>>,
    replies: RefCell<HashMap<String, mpsc::UnboundedSender<OscMessage>>>,
    heard: broadcast::Sender<Tap>,
    closed: Cell<bool>,
    stop: Signal,
}
struct SocketLink {
    port: u16,
    socket: RefCell<Option<Rc<UdpSocket>>>,
    state: Rc<State>,
    task: RefCell<Option<tokio::task::JoinHandle<()>>>,
}
/// Bind the first free port, with a local receiver task matching Node's callback ordering.
pub async fn open_ears_link(options: EarsOptions) -> Result<Rc<dyn EarsLink>, EarsError> {
    let candidates = options.port.map(|p| vec![p]).unwrap_or_else(|| options.ports.unwrap_or_else(|| KUMI_PORTS.to_vec()));
    let mut bound = None;
    for port in &candidates {
        if let Ok(socket) = UdpSocket::bind(("127.0.0.1", *port)).await {
            bound = Some(socket);
            break;
        }
    }
    let socket = Rc::new(bound.ok_or_else(|| {
        EarsError(format!(
            "Kumi couldn't listen for its listening devices: ports {} are all taken.",
            candidates.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
        ))
    })?);
    let port = socket.local_addr().map_err(|e| EarsError(e.to_string()))?.port();
    let state = Rc::new(State {
        known: RefCell::new(IndexMap::new()),
        replies: RefCell::new(HashMap::new()),
        heard: broadcast::channel(64).0,
        closed: Cell::new(false),
        stop: Signal::new(),
    });
    let receiving = socket.clone();
    let listening = state.clone();
    let task = tokio::task::spawn_local(async move {
        let mut packet = vec![0u8; 65536];
        loop {
            tokio::select! {_ = listening.stop.cancelled()=>break,result=receiving.recv_from(&mut packet)=>{let Ok((length,_))=result else{continue;};for message in decode_osc_packet(&packet[..length]){
                if message.address=="/kumi/ears/hello"{heard(&listening,&message.args,0);continue;}
                if message.address=="/kumi/ears/pong"&&heard(&listening,&message.args,1).is_none(){continue;}
                let token=message.args.first().map(OscValue::to_js_string).unwrap_or_default();let answer=listening.replies.borrow().get(&token).cloned();if let Some(answer)=answer{let _=answer.send(message);}
            }}}
        }
    });
    Ok(Rc::new(SocketLink { port, socket: RefCell::new(Some(socket)), state, task: RefCell::new(Some(task)) }))
}
fn number(args: &[OscValue], at: usize) -> Option<f64> {
    args.get(at).and_then(OscValue::as_number)
}
fn heard(state: &State, args: &[OscValue], from: usize) -> Option<Tap> {
    let port = number(args, from)?;
    let id = number(args, from + 1)?;
    let path = args.get(from + 4)?.as_text()?;
    let seen_at = now_ms();
    let version = number(args, from + 2)
        .or_else(|| args.get(from + 2).and_then(|v| kumi_common::js::number::parse(&v.to_js_string())))
        .unwrap_or(0.0);
    let tap = Tap {
        id,
        port: port as u16,
        path: kumi_common::js::string::trim(path).into(),
        version,
        sample_rate: number(args, from + 3).filter(|v| *v > 0.0).unwrap_or(44100.0),
        seen_at,
        loaded_at: number(args, from + 5).filter(|v| *v >= 0.0).map(|age| seen_at as f64 - age),
    };
    state.known.borrow_mut().insert(id.to_bits(), tap.clone());
    let _ = state.heard.send(tap.clone());
    Some(tap)
}
struct Pending {
    state: Rc<State>,
    token: String,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.state.replies.borrow_mut().remove(&self.token);
    }
}
impl SocketLink {
    fn send(&self, tap: &Tap, address: &str, args: &[OscArg]) {
        if self.state.closed.get() {
            return;
        }
        if let Some(socket) = self.socket.borrow().clone() {
            let bytes = encode_osc(address, args);
            let port = tap.port;
            tokio::task::spawn_local(async move {
                let _ = socket.send_to(&bytes, ("127.0.0.1", port)).await;
            });
        }
    }
    /// Sends (what, the port to answer on, a token, then `more`) and waits for the answer that carries the token.
    async fn ask(
        &self,
        tap: &Tap,
        address: &str,
        what: OscArg,
        more: &[OscArg],
        expect: &str,
        signal: Option<Signal>,
    ) -> Result<OscMessage, EarsError> {
        let token = uuid::Uuid::new_v4().to_string()[..12].to_string();
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.state.replies.borrow_mut().insert(token.clone(), tx);
        let _pending = Pending { state: self.state.clone(), token: token.clone() };
        let signal = signal.unwrap_or_default();
        if signal.is_cancelled() {
            return Err(EarsError("This operation was aborted".into()));
        }
        let mut args = vec![what, self.port.into(), token.into()];
        args.extend_from_slice(more);
        self.send(tap, address, &args);
        let deadline = tokio::time::sleep(Duration::from_millis(REPLY_MS));
        tokio::pin!(deadline);
        loop {
            tokio::select! {_ = signal.cancelled()=>return Err(EarsError("This operation was aborted".into())),_ = &mut deadline=>return Err(EarsError(format!("Kumi's listening device on {} didn't answer.",describe(&tap.path)))),message=rx.recv()=>if let Some(message)=message{if message.address==expect{return Ok(message);}}else{return Err(EarsError(format!("Kumi's listening device on {} didn't answer.",describe(&tap.path))));}}
        }
    }
}
#[async_trait(?Send)]
impl EarsLink for SocketLink {
    fn port(&self) -> u16 {
        self.port
    }
    fn taps(&self) -> Vec<Tap> {
        let now = now_ms();
        self.state
            .known
            .borrow()
            .values()
            .filter(|tap| now - tap.seen_at < GONE_MS && tap.version == EARS_VERSION as f64)
            .cloned()
            .collect()
    }
    async fn wait_for(&self, accept: Rc<dyn for<'a> Fn(&'a Tap) -> bool>, timeout_ms: u64, signal: Option<Signal>) -> Option<Tap> {
        if let Some(found) = self.taps().into_iter().find(|tap| accept(tap)) {
            return Some(found);
        }
        let mut heard = self.state.heard.subscribe();
        let signal = signal.unwrap_or_default();
        let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(timeout);
        loop {
            tokio::select! {_ = &mut timeout=>return None,_=signal.cancelled()=>return None,message=heard.recv()=>match message{Ok(tap)if tap.version==EARS_VERSION as f64&&accept(&tap)=>return Some(tap),Err(broadcast::error::RecvError::Closed)=>return None,_=>{}}}
        }
    }
    async fn arm(&self, tap: &Tap, seconds: f64, quiet: bool, signal: Option<Signal>) -> Result<Armed, EarsError> {
        let reply = self.ask(tap, "/kumi/ears/arm", OscArg::Float(seconds), &[i32::from(quiet).into()], "/kumi/ears/armed", signal).await?;
        Ok(Armed {
            beats: number(&reply.args, 2).unwrap_or(0.0),
            running: number(&reply.args, 3) == Some(1.0),
            sample_rate: number(&reply.args, 4).filter(|v| *v > 0.0).unwrap_or(tap.sample_rate),
        })
    }
    async fn write(&self, tap: &Tap, file: &str, signal: Option<Signal>) -> Result<Written, EarsError> {
        let reply = self.ask(tap, "/kumi/ears/write", file.into(), &[], "/kumi/ears/written", signal).await?;
        Ok(Written {
            file: reply.args.get(2).and_then(OscValue::as_text).filter(|v| !v.is_empty()).unwrap_or(file).into(),
            sample_rate: number(&reply.args, 3).filter(|v| *v > 0.0).unwrap_or(tap.sample_rate),
            channels: number(&reply.args, 4).filter(|v| *v > 0.0).unwrap_or(3.0) as usize,
            beats: number(&reply.args, 5).unwrap_or(0.0),
            running: number(&reply.args, 6) == Some(1.0),
        })
    }
    fn stop(&self, tap: &Tap) {
        self.send(tap, "/kumi/ears/stop", &["".into(), self.port.into(), "".into()]);
    }
    async fn ping(&self, tap: &Tap, signal: Option<Signal>) -> Option<Tap> {
        self.ask(tap, "/kumi/ears/ping", "".into(), &[], "/kumi/ears/pong", signal).await.ok()?;
        self.state.known.borrow().get(&tap.id.to_bits()).cloned()
    }
    async fn transport(&self, tap: &Tap, signal: Option<Signal>) -> Result<Option<Transport>, EarsError> {
        match self.ask(tap, "/kumi/ears/ping", "".into(), &[], "/kumi/ears/pong", signal.clone()).await {
            Ok(reply) => Ok(number(&reply.args, 7).map(|beats| Transport { beats, running: number(&reply.args, 8) == Some(1.0) })),
            Err(error) => {
                if signal.is_some_and(|s| s.is_cancelled()) {
                    Err(error)
                } else {
                    Ok(None)
                }
            }
        }
    }
    async fn close(&self) {
        if self.state.closed.replace(true) {
            return;
        }
        self.state.stop.cancel();
        let task = self.task.borrow_mut().take();
        if let Some(task) = task {
            let _ = task.await;
        }
        self.socket.borrow_mut().take();
    }
}
impl Drop for SocketLink {
    fn drop(&mut self) {
        self.state.stop.cancel();
    }
}
/// A device's place in plain words, for messages.
pub fn describe(path: &str) -> String {
    static TRACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"live_set tracks ([0-9]+)").unwrap());
    static RETURN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"live_set return_tracks ([0-9]+)").unwrap());
    if let Some(found) = TRACK.captures(path) {
        return format!("track {}", found[1].parse::<u64>().unwrap_or(0) + 1);
    }
    if let Some(found) = RETURN.captures(path) {
        let code = 65u32.wrapping_add(found[1].parse::<u32>().unwrap_or(0));
        return format!("return {}", String::from_utf16_lossy(&[code as u16]));
    }
    if path.contains("live_set master_track") {
        "Main".into()
    } else {
        "a track".into()
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub kind: String,
    pub index: usize,
    pub device: usize,
}
pub fn place_of(path: &str) -> Option<Place> {
    static PLACE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^live_set (tracks|return_tracks|master_track)(?: ([0-9]+))? devices ([0-9]+)$").unwrap());
    let found = PLACE.captures(kumi_common::js::string::trim(path))?;
    Some(Place {
        kind: match &found[1] {
            "tracks" => "track",
            "return_tracks" => "return",
            _ => "main",
        }
        .into(),
        index: found.get(2).and_then(|s| s.as_str().parse().ok()).unwrap_or(0),
        device: found[3].parse().ok()?,
    })
}
