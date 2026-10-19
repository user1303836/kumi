//! The authenticated channel to the Remote Script.
use super::wire;
use crate::live::*;
use serde_json::{json, Value};
use std::collections::HashSet;

pub const READ_ONLY_INVOKES: &[&str] = &[
    "session.playback",
    "willington.device.read",
    "willington.editing.read",
    "automation.envelope.read",
    "arrangement.automation.read",
    "audio.take-lane.read",
    "audio.warp-marker.read",
    "browser.search",
    "browser.inspect",
    "browser.roots",
    "audio.capture.inspect",
    "audio.capture.status",
    "realtime.stats",
    "session.reconnect",
    "song.read",
    "song.time-convert",
    "tuning.read",
    "groove.read",
    "note.read-by-id",
    "note.read-selected",
    "performance.read",
    "authority.digest",
    "dev.lom-audit",
    "data.get",
    "automation.value-at",
    "plugin.parameter-names",
    "device.banks.read",
    "clip.time-convert",
];
pub const AUTHORITY_FREE_INVOKES: &[&str] = &["undo.step.begin", "undo.step.end", "application.message", "python.run"];
pub const TRANSACTION_CREATIONS: &[&str] = &[
    "track.create",
    "track.create-return",
    "track.duplicate",
    "scene.create",
    "scene.duplicate",
    "clip.create",
    "clip.duplicate",
    "arrangement.clip.create",
    "arrangement.audio-clip.create",
    "session.audio-clip.create",
    "browser.load",
    "device.insert",
    "device.duplicate",
    "session.capture-midi",
    "scene.capture",
    "locator.add",
];
pub const TRANSACTION_DELETIONS: &[&str] =
    &["track.delete", "track.delete-return", "scene.delete", "clip.delete", "arrangement.clip.delete", "device.delete", "locator.delete"];
pub const EXPLICIT_DELETIONS: &[&str] =
    &["device.delete", "track.delete-return", "clip.delete", "arrangement.clip.delete", "scene.delete", "track.delete", "locator.delete"];

pub fn mutation_authority_required(operation: &str) -> bool {
    !READ_ONLY_INVOKES.contains(&operation) && !AUTHORITY_FREE_INVOKES.contains(&operation)
}

/// The exact reference-valued fields the Remote Script's authority digest collects.
pub fn digest_references(value: &Value) -> HashSet<String> {
    fn collect(value: &Value, key: &str, into: &mut HashSet<String>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    collect(item, key, into);
                }
            }
            Value::Object(row) => {
                for (key, item) in row {
                    collect(item, key, into);
                }
            }
            Value::String(text) if key == "ref" || key.ends_with("Ref") || key.ends_with("Refs") => {
                into.insert(text.clone());
            }
            _ => {}
        }
    }
    let mut into = HashSet::new();
    collect(value, "", &mut into);
    into
}
fn references_key(value: &Value) -> String {
    let mut refs: Vec<_> = digest_references(value).into_iter().collect();
    refs.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    kumi_common::js::json::stringify(&json!(refs))
}

/// Expand the wire's compact pad-chain references into their rack's complete rows.
/// Rust JSON owns its children; the observable JSON agrees with the shared JS rows.
pub fn expand_pad_chains(value: &mut Value) {
    fn expand(value: &mut Value, depth: usize) {
        if depth > 48 {
            return;
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    expand(item, depth + 1);
                }
            }
            Value::Object(row) => {
                for child in row.values_mut() {
                    expand(child, depth + 1);
                }
                // Each pad chain the rack lists, with the rack's row for it (the last with its identity), found
                // first and copied alone: a rack without pads, or a big one, copies none of its chains (#174).
                let Some(pads) = row.get("drumPads").and_then(Value::as_array) else { return };
                let chains = row.get("chains").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
                let mut listed = Vec::new();
                for (at, pad) in pads.iter().enumerate() {
                    for (place, chain) in pad.get("chains").and_then(Value::as_array).into_iter().flatten().enumerate() {
                        if chain.get("listedOnRack") == Some(&Value::Bool(true)) {
                            let full = chains
                                .iter()
                                .rev()
                                .find(|full| full.is_object() && full.get("objectIdentity") == chain.get("objectIdentity"));
                            listed.push((at, place, full.cloned()));
                        }
                    }
                }
                let Some(pads) = row.get_mut("drumPads").and_then(Value::as_array_mut) else { return };
                for (at, place, full) in listed {
                    let chain = &mut pads[at]["chains"][place];
                    match full {
                        Some(full) => *chain = full,
                        None => chain["devices"] = json!([]),
                    }
                }
            }
            _ => {}
        }
    }
    expand(value, 0);
}

fn valid_status(value: &Value) -> bool {
    let Some(row) = value.as_object() else {
        return false;
    };
    if !row.get("connected").is_some_and(Value::is_boolean)
        || !matches!(value["adapter"].as_str(), Some("remote-script" | "simulator" | "extension" | "unavailable"))
        || !(row.get("epoch") == Some(&Value::Null) || wire::safe_integer(&value["epoch"]).is_some_and(|n| n >= 1.0))
        || value["protocol"] != LIVE_PROTOCOL_VERSION
        || value["registryHash"] != *LIVE_REGISTRY_HASH
    {
        return false;
    }
    let Some(caps) = value["capabilities"].as_array() else {
        return false;
    };
    let Some(ops) = value["operations"].as_array() else {
        return false;
    };
    let valid_list = |items: &[Value]| {
        items.len() <= 4096
            && items.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 128))
            && items.iter().map(|v| v.as_str()).collect::<HashSet<_>>().len() == items.len()
    };
    if !valid_list(caps) || !valid_list(ops) {
        return false;
    }
    if !caps.iter().all(|v| LiveCapability::parse(v.as_str().unwrap()).is_some())
        || !ops.iter().all(|v| LIVE_REGISTRY_OPERATIONS.iter().any(|op| v == op))
    {
        return false;
    }
    if !["status", "snapshot", "discover", "get", "reconnect", "session.playback"].iter().all(|required| ops.iter().any(|v| v == required))
    {
        return false;
    }
    let operations: Vec<_> = ops.iter().map(|v| v.as_str().unwrap()).collect();
    let derived = live_capabilities_for_operations(&operations);
    caps.iter().all(|v| derived.iter().any(|cap| v.as_str() == Some(cap.as_str())))
}

fn registry_request(operation: &str, fields: &Value) -> Result<Value, LiveError> {
    Ok(match operation {
        "status" | "reconnect" | "session.playback" => json!({}),
        "get" => json!({"ref":fields["ref"]}),
        "authority.preflight" | "authority.prepare" => {
            let mut args = json!({"operation":fields["operation"],"argsDigest":wire::digest(fields.get("args").unwrap_or(&json!({})))?,"transactionId":fields["transactionId"]});
            if operation == "authority.prepare" {
                for key in ["preflightToken", "confirmation", "idempotencyKey"] {
                    if let Some(value) = fields.get(key) {
                        args[key] = value.clone();
                    }
                }
            }
            args
        }
        "authority.retire" => {
            let mut args = json!({"transactionId":fields["transactionId"]});
            if let Some(terminal) = fields.get("terminal") {
                args["terminal"] = terminal.clone();
            }
            args
        }
        _ => fields.get("args").cloned().unwrap_or_else(|| json!({})),
    })
}
use super::listeners::Listeners;
use crate::loopback::LOOPBACK_PROTOCOL_VERSION;
use crate::registry::{validate_live_operation_request, validate_live_operation_result};
use async_trait::async_trait;
use futures::future::{LocalBoxFuture, Shared};
use futures::FutureExt;
use kumi_common::{abort::Signal, time::now_ms};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{tcp::OwnedWriteHalf, TcpStream};
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MutationPath {
    #[default]
    Mutate,
    Authority,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteScriptEndpoint {
    pub host: String,
    pub port: f64,
    pub secret: String,
    pub timeout_ms: Option<f64>,
    pub mutation_path: Option<MutationPath>,
    pub retire_after: Option<usize>,
}
impl RemoteScriptEndpoint {
    pub fn new(host: impl Into<String>, port: u16, secret: impl Into<String>) -> Self {
        Self { host: host.into(), port: f64::from(port), secret: secret.into(), timeout_ms: None, mutation_path: None, retire_after: None }
    }
    fn validate(&self) -> Result<(), LiveError> {
        if !matches!(self.host.as_str(), "127.0.0.1" | "::1")
            || self.port.fract() != 0.0
            || !(1.0..=65535.0).contains(&self.port)
            || kumi_common::js::string::utf16_len(&self.secret) < 32
        {
            return Err(LiveError::error("remote script endpoint must use exact loopback address 127.0.0.1 or ::1 with a strong secret"));
        }
        Ok(())
    }
    fn timeout(&self) -> f64 {
        self.timeout_ms.unwrap_or(5000.0)
    }
}
struct Pending {
    operation: String,
    response: oneshot::Sender<Result<Value, LiveError>>,
}
type SharedResult = Shared<LocalBoxFuture<'static, Result<(), LiveError>>>;
struct Expectation {
    operation: String,
    references: String,
    digest: Shared<LocalBoxFuture<'static, Option<String>>>,
    expires_at: f64,
}
struct RemoteInner {
    endpoint: RemoteScriptEndpoint,
    writer: RefCell<Option<Rc<Mutex<OwnedWriteHalf>>>>,
    connection: RefCell<Signal>,
    generation: Cell<u64>,
    sequence: Cell<u64>,
    epoch: Cell<Option<i64>>,
    bridge_epoch: RefCell<Option<String>>,
    challenge: RefCell<Option<String>>,
    hello: RefCell<Option<oneshot::Sender<Result<(), LiveError>>>>,
    cached: RefCell<LiveStatus>,
    pending: RefCell<HashMap<String, Pending>>,
    listeners: Listeners<dyn Fn(&LiveEvent)>,
    status_listeners: Listeners<dyn Fn(Option<&LiveStatus>)>,
    last_event_epoch: Cell<Option<i64>>,
    last_event_sequence: Cell<u64>,
    reopening: RefCell<Option<SharedResult>>,
    explicitly_closed: Cell<bool>,
    poisoned: Cell<bool>,
    subscription: RefCell<Option<Value>>,
    cleanup_ownership: RefCell<HashMap<String, HashMap<String, String>>>,
    expected_digests: RefCell<VecDeque<(String, Expectation)>>,
    unretired: RefCell<VecDeque<String>>,
    changing: RefCell<HashMap<String, usize>>,
    retiring: Cell<bool>,
}
/// Background readers run on a Tokio `LocalSet`, matching the host's single-threaded callback model.
#[derive(Clone)]
pub struct RemoteScriptLiveAdapter(Rc<RemoteInner>);
/// What the bridge says when Live runs another version of Kumi's Remote Script than this bridge's.
pub use kumi_common::bridge::ANOTHER_BRIDGE;
impl RemoteScriptLiveAdapter {
    pub async fn connect(endpoint: RemoteScriptEndpoint) -> Result<Self, LiveError> {
        endpoint.validate()?;
        let adapter = Self(Rc::new(RemoteInner {
            endpoint,
            writer: RefCell::new(None),
            connection: RefCell::new(Signal::new()),
            generation: Cell::new(0),
            sequence: Cell::new(0),
            epoch: Cell::new(None),
            bridge_epoch: RefCell::new(None),
            challenge: RefCell::new(None),
            hello: RefCell::new(None),
            cached: RefCell::new(serde_json::from_value(
                json!({"connected":false,"adapter":"unavailable","epoch":null,"protocol":LIVE_PROTOCOL_VERSION,"capabilities":[],"reason":"not-connected"}),
            )?),
            pending: RefCell::new(HashMap::new()),
            listeners: Listeners::default(),
            status_listeners: Listeners::default(),
            last_event_epoch: Cell::new(None),
            last_event_sequence: Cell::new(0),
            reopening: RefCell::new(None),
            explicitly_closed: Cell::new(false),
            poisoned: Cell::new(false),
            subscription: RefCell::new(None),
            cleanup_ownership: RefCell::new(HashMap::new()),
            expected_digests: RefCell::new(VecDeque::new()),
            unretired: RefCell::new(VecDeque::new()),
            changing: RefCell::new(HashMap::new()),
            retiring: Cell::new(false),
        }));
        adapter.open().await?;
        let value = adapter.request(json!({"method":"status"}), "status", None).await?;
        if !valid_status(&value) || value["connected"] != true || value["adapter"] != "remote-script" || value["epoch"].is_null() {
            adapter.close().await?;
            if value["registryHash"].is_string() && value["registryHash"] != *LIVE_REGISTRY_HASH {
                return Err(LiveError::error(ANOTHER_BRIDGE));
            }
            return Err(LiveError::error("remote script handshake or negotiation failed"));
        }
        let status: LiveStatus = serde_json::from_value(value)?;
        adapter.0.epoch.set(status.epoch);
        *adapter.0.cached.borrow_mut() = status;
        Ok(adapter)
    }
    fn live_socket(&self) -> bool {
        self.0.writer.borrow().is_some() && !self.0.connection.borrow().is_cancelled()
    }
    fn emit_status(&self) {
        let status = self.0.cached.borrow().clone();
        self.0.status_listeners.each(|listener| listener(Some(&status)));
    }
    fn shape_changed(prior: &LiveStatus, next: &LiveStatus) -> bool {
        let mut a: Vec<_> = prior.capabilities.iter().map(LiveCapability::as_str).collect();
        a.sort();
        let mut b: Vec<_> = next.capabilities.iter().map(LiveCapability::as_str).collect();
        b.sort();
        prior.connected != next.connected
            || prior.epoch != next.epoch
            || prior.willington_kinds.as_deref().unwrap_or_default() != next.willington_kinds.as_deref().unwrap_or_default()
            || prior.operations.as_deref().unwrap_or_default() != next.operations.as_deref().unwrap_or_default()
            || a != b
    }
    fn fail_pending(&self, error: LiveError) {
        for (_, pending) in self.0.pending.take() {
            let _ = pending.response.send(Err(error.clone()));
        }
    }
    fn disconnect(&self, error: LiveError) {
        self.0.connection.borrow().cancel();
        self.0.writer.take();
        let was_connected = self.0.cached.borrow().connected;
        {
            let mut status = self.0.cached.borrow_mut();
            status.connected = false;
            status.reason = Some("remote-adapter-disconnected".into());
        }
        if self.0.bridge_epoch.borrow().is_some() && was_connected {
            self.emit_status();
        }
        if let Some(hello) = self.0.hello.take() {
            let _ = hello.send(Err(error.clone()));
        }
        self.fail_pending(error);
    }
    async fn open(&self) -> Result<(), LiveError> {
        let timeout = self.0.endpoint.timeout();
        if !timeout.is_finite() || timeout <= 0.0 {
            return Err(LiveError::error("remote adapter reconnect deadline expired"));
        }
        let (hello_send, hello_recv) = oneshot::channel();
        *self.0.hello.borrow_mut() = Some(hello_send);
        let this = self.clone();
        let opening = async move {
            let stream = TcpStream::connect((this.0.endpoint.host.as_str(), this.0.endpoint.port as u16))
                .await
                .map_err(|e| LiveError::error(e.to_string()))?;
            stream.set_nodelay(true).map_err(|e| LiveError::error(e.to_string()))?;
            let (mut reader, writer) = stream.into_split();
            this.0.connection.borrow().cancel();
            let cancel = Signal::new();
            *this.0.connection.borrow_mut() = cancel.clone();
            let generation = this.0.generation.get() + 1;
            this.0.generation.set(generation);
            *this.0.writer.borrow_mut() = Some(Rc::new(Mutex::new(writer)));
            let weak = Rc::downgrade(&this.0);
            tokio::task::spawn_local(async move {
                let mut buffer = bytes::BytesMut::new();
                // How far the buffer is known to hold no newline: each byte is searched once, so a large
                // frame arriving in 64 KiB reads isn't rescanned from its start after each one.
                let mut scanned = 0;
                let mut chunk = vec![0; 65536];
                loop {
                    let read = tokio::select! { biased; _=cancel.cancelled()=>break, read=reader.read(&mut chunk)=>read };
                    let Some(inner) = weak.upgrade() else { break };
                    let adapter = Self(inner);
                    if adapter.0.generation.get() != generation {
                        break;
                    }
                    let count = match read {
                        Ok(0) => {
                            adapter.disconnect(LiveError::error("remote adapter disconnected"));
                            break;
                        }
                        Ok(n) => n,
                        Err(e) => {
                            adapter.disconnect(LiveError::error(e.to_string()));
                            break;
                        }
                    };
                    buffer.extend_from_slice(&chunk[..count]);
                    let mut failure = None;
                    if !chunk[..count].contains(&b'\n') && buffer.len() > wire::MAX_FRAME_BYTES {
                        failure = Some(LiveError::error("remote frame exceeds limit"));
                    }
                    while failure.is_none() {
                        let Some(found) = memchr::memchr(b'\n', &buffer[scanned..]) else {
                            scanned = buffer.len();
                            break;
                        };
                        let index = scanned + found;
                        scanned = 0;
                        let line = buffer.split_to(index + 1);
                        if index == 0 {
                            continue;
                        }
                        if index > wire::MAX_FRAME_BYTES {
                            failure = Some(LiveError::error("remote frame exceeds limit"));
                            break;
                        }
                        let parsed = wire::parse(&line[..index]).and_then(|frame| adapter.on_response(frame));
                        if let Err(error) = parsed {
                            failure = Some(error);
                        }
                    }
                    if let Some(error) = failure {
                        adapter.disconnect(error);
                        break;
                    }
                }
            });
            hello_recv.await.unwrap_or_else(|_| Err(LiveError::error("remote adapter disconnected")))
        };
        match tokio::time::timeout(duration(timeout), opening).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.disconnect(error.clone());
                Err(error)
            }
            Err(_) => {
                let error = LiveError::error("remote adapter connection timed out");
                self.disconnect(error.clone());
                Err(error)
            }
        }
    }
    fn on_response(&self, mut response: Value) -> Result<(), LiveError> {
        if response["version"] != LOOPBACK_PROTOCOL_VERSION
            || !["id", "mac", "bridgeEpoch", "connectionChallenge"].iter().all(|key| response[*key].is_string())
        {
            return Err(LiveError::error("invalid remote response"));
        }
        if !wire::verify(&self.0.endpoint.secret, &mut response, false)? {
            return Err(LiveError::error("remote response authentication failed"));
        }
        let id = response["id"].as_str().unwrap();
        if id == "hello" {
            if self.0.bridge_epoch.borrow().is_some() || !truthy(&response["ok"]) || !response["result"].is_object() {
                return Err(LiveError::error("invalid or duplicate remote hello"));
            }
            let hello = &response["result"];
            // Live loads Kumi's Remote Script when it starts: one with another registry is another bridge version's,
            // Live left running through an update (or another Kumi's). Restarting Live loads the installed one.
            if hello["protocol"] != LIVE_PROTOCOL_VERSION || hello["registryHash"] != *LIVE_REGISTRY_HASH {
                return Err(LiveError::error(ANOTHER_BRIDGE));
            }
            if !wire::safe_integer(&hello["maxDeadlineMs"]).is_some_and(|n| n >= 100.0)
                || kumi_common::js::string::utf16_len(response["bridgeEpoch"].as_str().unwrap()) < 16
                || kumi_common::js::string::utf16_len(response["connectionChallenge"].as_str().unwrap()) < 16
            {
                return Err(LiveError::error("remote hello negotiation failed"));
            }
            *self.0.bridge_epoch.borrow_mut() = response["bridgeEpoch"].as_str().map(str::to_owned);
            *self.0.challenge.borrow_mut() = response["connectionChallenge"].as_str().map(str::to_owned);
            if let Some(hello) = self.0.hello.take() {
                let _ = hello.send(Ok(()));
            }
            return Ok(());
        }
        if response["bridgeEpoch"].as_str() != self.0.bridge_epoch.borrow().as_deref()
            || response["connectionChallenge"].as_str() != self.0.challenge.borrow().as_deref()
        {
            return Err(LiveError::error("remote response channel binding failed"));
        }
        if let Some(event) = response.get("result").and_then(|v| v.get("event")) {
            if !event.is_object()
                || !wire::safe_integer(&event["epoch"]).is_some_and(|v| v > 0.0)
                || !wire::safe_integer(&event["sequence"]).is_some_and(|v| v > 0.0)
                || !event["type"].as_str().is_some_and(|v| REMOTE_SCRIPT_EVENT_TYPES.iter().any(|e| e.as_str() == v))
            {
                return Err(LiveError::error("invalid remote event"));
            }
            let mut event = event.clone();
            if event.get("payload").is_none() {
                event["payload"] = Value::Null;
            }
            let event: LiveEvent = serde_json::from_value(event)?;
            if Some(event.epoch) != self.0.epoch.get() {
                return Err(LiveError::error("remote event epoch does not match the current connection"));
            }
            if self.0.last_event_epoch.get() != Some(event.epoch) {
                self.0.last_event_epoch.set(Some(event.epoch));
                self.0.last_event_sequence.set(0);
            }
            if event.sequence <= self.0.last_event_sequence.get()
                || (event.event_type != LiveEventType::Reset && event.sequence != self.0.last_event_sequence.get() + 1)
            {
                return Err(LiveError::error("remote event sequence gap or replay requires reset"));
            }
            self.0.last_event_sequence.set(event.sequence);
            self.0.listeners.each(|listener| listener(&event));
            return Ok(());
        }
        let pending = self.0.pending.borrow_mut().remove(id).ok_or_else(|| LiveError::error("unknown or duplicate remote response"))?;
        if truthy(&response["ok"]) {
            let result = response["result"].take();
            if let Err(error) = validate_live_operation_result(&pending.operation, &result) {
                let error = LiveError::from(error);
                let _ = pending.response.send(Err(error.clone()));
                self.0.cached.borrow_mut().connected = false;
                self.0.cached.borrow_mut().reason = Some("registry-result-validation-failed".into());
                return Err(error);
            }
            if pending.operation == "subscribe" {
                self.0.last_event_epoch.set(self.0.epoch.get());
                self.0.last_event_sequence.set(0);
            }
            if pending.operation == "reconnect" && valid_status(&result) {
                self.0.epoch.set(result["epoch"].as_i64());
                self.0.last_event_epoch.set(self.0.epoch.get());
                self.0.last_event_sequence.set(0);
            }
            let _ = pending.response.send(Ok(result));
        } else {
            let _ = pending.response.send(Err(LiveError::error(response["error"].as_str().unwrap_or("remote request failed"))));
        }
        Ok(())
    }
    async fn request(&self, mut fields: Value, operation: &str, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        if !self.live_socket() || self.0.bridge_epoch.borrow().is_none() || self.0.challenge.borrow().is_none() {
            return Err(LiveError::error("remote adapter is disconnected"));
        }
        if self.0.pending.borrow().len() >= 4096 {
            return Err(LiveError::error("remote adapter queue is full"));
        }
        if self.0.sequence.get() >= 9_007_199_254_740_991 {
            return Err(LiveError::error("remote adapter sequence exhausted"));
        }
        if cancelled(context) {
            return Err(LiveError::error("remote adapter request cancelled before dispatch"));
        }
        let configured = self.0.endpoint.timeout();
        let timeout =
            if matches!(fields["method"].as_str(), Some("snapshot" | "discover")) { (configured * 6.0).min(60000.0) } else { configured };
        let deadline = context.and_then(|c| c.deadline_ms).unwrap_or_else(|| now_ms() as f64 + timeout);
        if !kumi_common::js::number::is_safe_integer(deadline) || deadline <= now_ms() as f64 || deadline > now_ms() as f64 + 60000.0 {
            return Err(LiveError::error("remote adapter deadline is invalid or expired"));
        }
        validate_live_operation_request(operation, &registry_request(operation, &fields)?)?;
        let sequence = self.0.sequence.get() + 1;
        self.0.sequence.set(sequence);
        let id = format!("async-{sequence}");
        let row = fields.as_object_mut().expect("internal request fields");
        row.insert("version".into(), LOOPBACK_PROTOCOL_VERSION.into());
        row.insert("id".into(), id.clone().into());
        row.insert("nonce".into(), wire::random_id().into());
        row.insert("sequence".into(), sequence.into());
        row.insert("bridgeEpoch".into(), self.0.bridge_epoch.borrow().clone().unwrap().into());
        row.insert("connectionChallenge".into(), self.0.challenge.borrow().clone().unwrap().into());
        row.insert("deadlineMs".into(), json!(deadline));
        let request = wire::signed(&self.0.endpoint.secret, fields, false)?;
        let mut encoded = kumi_common::js::json::stringify(&request).into_bytes();
        encoded.push(b'\n');
        let (send, recv) = oneshot::channel();
        self.0.pending.borrow_mut().insert(id.clone(), Pending { operation: operation.into(), response: send });
        let writer = self.0.writer.borrow().clone().unwrap();
        let remaining = if context.and_then(|c| c.deadline_ms).is_some() { deadline - now_ms() as f64 } else { timeout };
        let signal = context.and_then(|c| c.signal.clone()).unwrap_or_default();
        let exchange = async {
            let mut recv = recv;
            let write = async { writer.lock().await.write_all(&encoded).await };
            tokio::select! {
                result = write => if let Err(error) = result {
                    self.0.pending.borrow_mut().remove(&id);
                    let error = LiveError::error(error.to_string());
                    self.disconnect(error.clone());
                    return Err(error);
                },
                result = &mut recv => return result.unwrap_or_else(|_|Err(LiveError::error("remote adapter disconnected"))),
            }
            recv.await.unwrap_or_else(|_| Err(LiveError::error("remote adapter disconnected")))
        };
        tokio::select! {
            result=exchange=>result,
            _=signal.cancelled()=>{let error=LiveError::error("remote adapter request state uncertain after dispatch cancellation");self.0.cached.borrow_mut().connected=false;self.fail_pending(error.clone());self.disconnect(error.clone());Err(error)},
            _=tokio::time::sleep(duration(remaining.max(1.0)))=>{let error=LiveError::error("remote adapter request state uncertain after dispatch timeout");self.0.cached.borrow_mut().connected=false;self.fail_pending(error.clone());self.disconnect(error.clone());Err(error)},
        }
    }

    async fn ensure_connected(&self, context: Option<&LiveOperationContext>) -> Result<(), LiveError> {
        if self.0.explicitly_closed.get() {
            return Err(LiveError::error("remote adapter is closed"));
        }
        if self.0.poisoned.get() {
            return Err(LiveError::error("remote adapter reconciliation channel is poisoned by a bridge or Live epoch change"));
        }
        let existing = self.0.reopening.borrow().clone();
        let reopening = if let Some(reopening) = existing {
            reopening
        } else {
            if self.live_socket() && self.0.bridge_epoch.borrow().is_some() && self.0.challenge.borrow().is_some() {
                return Ok(());
            }
            let prior_bridge = self.0.bridge_epoch.take();
            let prior_live = self.0.epoch.get();
            self.0.challenge.take();
            self.0.sequence.set(0);
            let this = self.clone();
            let future = async move {
                let result = this.reopen(prior_bridge, prior_live).await;
                if result.is_err() {
                    let mut status = this.0.cached.borrow_mut();
                    status.connected = false;
                    status.reason = Some("remote-reconnect-failed".into());
                    drop(status);
                    this.0.connection.borrow().cancel();
                    this.0.writer.take();
                }
                this.0.reopening.take();
                result
            }
            .boxed_local()
            .shared();
            *self.0.reopening.borrow_mut() = Some(future.clone());
            let background = future.clone();
            tokio::task::spawn_local(async move {
                let _ = background.await;
            });
            future
        };
        let Some(context) = context else { return reopening.await };
        if context.signal.as_ref().is_some_and(Signal::is_cancelled) {
            return Err(LiveError::error("remote adapter reconnect cancelled"));
        }
        let deadline = context.deadline_ms.unwrap_or_else(|| now_ms() as f64 + self.0.endpoint.timeout());
        if !kumi_common::js::number::is_safe_integer(deadline) || deadline <= now_ms() as f64 || deadline > now_ms() as f64 + 60000.0 {
            return Err(LiveError::error("remote adapter reconnect deadline is invalid or expired"));
        }
        let signal = context.signal.clone().unwrap_or_default();
        tokio::select! {result=reopening=>result,_=signal.cancelled()=>Err(LiveError::error("remote adapter reconnect cancelled")),_=tokio::time::sleep(duration((deadline-now_ms() as f64).max(1.0)))=>Err(LiveError::error("remote adapter reconnect deadline expired"))}
    }
    async fn reopen(&self, prior_bridge: Option<String>, prior_live: Option<i64>) -> Result<(), LiveError> {
        self.open().await?;
        let value = self.request(json!({"method":"status"}), "status", None).await?;
        if !valid_status(&value) || value["connected"] != true || value["adapter"] != "remote-script" || value["epoch"].is_null() {
            return Err(LiveError::error("remote adapter recovery negotiation failed"));
        }
        let status: LiveStatus = serde_json::from_value(value)?;
        if (prior_bridge.is_some() && *self.0.bridge_epoch.borrow() != prior_bridge) || (prior_live.is_some() && status.epoch != prior_live)
        {
            self.0.cached.borrow_mut().connected = false;
            self.0.cached.borrow_mut().reason = Some("remote-bridge-or-live-epoch-changed".into());
            self.0.poisoned.set(true);
            self.0.bridge_epoch.take();
            self.0.challenge.take();
            self.0.connection.borrow().cancel();
            self.0.writer.take();
            return Err(LiveError::error("remote bridge or Live epoch changed; mutation reconciliation is unavailable"));
        }
        let changed = Self::shape_changed(&self.0.cached.borrow(), &status);
        self.0.epoch.set(status.epoch);
        *self.0.cached.borrow_mut() = status;
        self.0.last_event_epoch.set(self.0.epoch.get());
        self.0.last_event_sequence.set(0);
        if changed {
            self.emit_status();
        }
        let args = self.0.subscription.borrow().clone();
        if let Some(args) = args {
            let result = self.request(json!({"method":"subscribe","args":args}), "subscribe", None).await?;
            if result["subscribed"] != true {
                return Err(LiveError::error("remote adapter subscription restoration failed"));
            }
        }
        Ok(())
    }
}
fn cancelled(context: Option<&LiveOperationContext>) -> bool {
    context.and_then(|c| c.signal.as_ref()).is_some_and(Signal::is_cancelled)
}
fn duration(ms: f64) -> Duration {
    Duration::from_secs_f64(if ms.is_finite() { ms.max(0.0) / 1000.0 } else { 0.001 })
}
impl RemoteScriptLiveAdapter {
    async fn invoke_once(&self, invocation: &LiveInvocation, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.ensure_connected(context).await?;
        let operation = invocation.operation.as_str();
        let args = Value::Object(invocation.args.clone());
        if !self.0.cached.borrow().has_operation(operation) {
            return Err(LiveError::error(format!("remote operation is not negotiated: {operation}")));
        }
        if operation == "subscribe" {
            let result = self.request(json!({"method":"subscribe","args":args}), "subscribe", context).await?;
            *self.0.subscription.borrow_mut() = if result["subscribed"] == true { Some(args) } else { None };
            return Ok(result);
        }
        if !mutation_authority_required(operation) {
            let result = self.request(json!({"method":"invoke","operation":operation,"args":args}), operation, context).await?;
            if operation == "session.reconnect" {
                self.0.cleanup_ownership.borrow_mut().clear();
            }
            return Ok(result);
        }
        let args_digest = wire::digest(&args)
            .and_then(|digest| {
                validate_live_operation_request(operation, &args)?;
                Ok(digest)
            })
            .map_err(|e| LiveError::MutationNotDispatched(e.to_string()))?;
        let base = context.and_then(|c| c.idempotency_key.clone()).unwrap_or_else(wire::random_id);
        let transaction = context.and_then(|c| c.transaction_id.clone()).unwrap_or_else(wire::random_id);
        if !(8..=128).contains(&kumi_common::js::string::utf16_len(&base))
            || !(8..=128).contains(&kumi_common::js::string::utf16_len(&transaction))
        {
            return Err(LiveError::error("remote mutation idempotency authority is invalid"));
        }
        let owned_count: usize = self.0.cleanup_ownership.borrow().values().map(HashMap::len).sum();
        let reserve = if operation == "session.capture-midi" { 256 } else { 1 };
        if TRANSACTION_CREATIONS.contains(&operation) && owned_count + reserve > 4096 {
            return Err(LiveError::error("remote cleanup ownership ledger is full"));
        }
        let explicit = EXPLICIT_DELETIONS.contains(&operation) && args["explicitDeletion"] == true;
        let owned = (TRANSACTION_DELETIONS.contains(&operation) && !explicit) || operation == "ownership.settle";
        let reference = args["ref"].as_str();
        let mut ownership = if owned {
            reference.and_then(|r| self.0.cleanup_ownership.borrow().get(&transaction).and_then(|m| m.get(r)).cloned())
        } else {
            None
        };
        if owned && ownership.is_none() {
            return Err(LiveError::MutationNotDispatched("remote destructive cleanup lacks transaction-owned authority".into()));
        }
        let mut consumed = None;
        if matches!(operation, "clip.move" | "arrangement.clip.move") {
            if let Some(reference) = reference {
                let ledger = self.0.cleanup_ownership.borrow();
                let matches: Vec<_> = ledger.iter().filter(|(_, rows)| rows.contains_key(reference)).collect();
                if matches.len() > 1 {
                    return Err(LiveError::error("remote transaction-owned move authority is ambiguous"));
                }
                if let Some((transaction, rows)) = matches.first() {
                    ownership = rows.get(reference).cloned();
                    consumed = Some((transaction.to_string(), reference.to_owned()));
                }
            }
        }
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(format!("{transaction}\0{base}\0{operation}\0{args_digest}").as_bytes()));
        let result = if self.retires_on_its_own() {
            self.mutate(invocation, &transaction, &key, ownership.as_deref(), context).await
        } else {
            self.authorized_invoke(invocation, &transaction, &key, ownership.as_deref(), context).await
        };
        let mut result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some((transaction, reference)) = &consumed {
                    if !matches!(error, LiveError::MutationNotDispatched(_)) && self.live_socket() {
                        if let Ok(observed) = self.request(json!({"method":"get","ref":reference}), "get", context).await {
                            if !observed.is_object()
                                || !args["expectedObjectIdentity"].is_string()
                                || observed["objectIdentity"] != args["expectedObjectIdentity"]
                            {
                                self.remove_ownership(transaction, reference);
                            }
                        }
                    }
                }
                return Err(error);
            }
        };
        if TRANSACTION_CREATIONS.contains(&operation) {
            let mut tokens = vec![];
            let mut take_token = |row: &mut Value| -> Result<(), LiveError> {
                if !row.is_object() {
                    return Err(LiveError::error("remote creation ownership evidence is malformed"));
                }
                let reference = row[if operation == "browser.load" { "deviceRef" } else { "ref" }].as_str();
                let token = row["ownershipToken"].as_str();
                if reference.is_none() || !token.is_some_and(|t| kumi_common::js::string::utf16_len(t) >= 32) {
                    return Err(LiveError::error("remote creation ownership token is missing"));
                }
                tokens.push((reference.unwrap().to_owned(), token.unwrap().to_owned()));
                row.as_object_mut().unwrap().remove("ownershipToken");
                Ok(())
            };
            if operation == "session.capture-midi" {
                let rows = result["clipIdentities"]
                    .as_array_mut()
                    .ok_or_else(|| LiveError::error("remote creation ownership result is malformed"))?;
                for row in rows {
                    take_token(row)?;
                }
            } else {
                take_token(&mut result)?;
            }
            if !tokens.is_empty() {
                self.0.cleanup_ownership.borrow_mut().entry(transaction).or_default().extend(tokens);
            }
            return Ok(result);
        }
        if let Some((transaction, reference)) = consumed {
            self.remove_ownership(&transaction, &reference);
        }
        if TRANSACTION_DELETIONS.contains(&operation) {
            if let Some(reference) = reference {
                self.remove_ownership(&transaction, reference);
            }
        }
        Ok(result)
    }
    fn remove_ownership(&self, transaction: &str, reference: &str) {
        let mut ledger = self.0.cleanup_ownership.borrow_mut();
        if let Some(rows) = ledger.get_mut(transaction) {
            rows.remove(reference);
            if rows.is_empty() {
                ledger.remove(transaction);
            }
        }
    }
    async fn authorized_invoke(
        &self,
        invocation: &LiveInvocation,
        transaction: &str,
        key: &str,
        ownership: Option<&str>,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        let prepare = async {
            let mut fields =
                json!({"method":"preflight","operation":invocation.operation,"args":invocation.args,"transactionId":transaction});
            if let Some(token) = ownership {
                fields["ownershipToken"] = token.into();
            }
            let preflight = self.request(fields.clone(), "authority.preflight", context).await?;
            if !preflight["preflightToken"].is_string()
                || !preflight["confirmation"].is_string()
                || preflight["operation"] != invocation.operation
                || !preflight["argsDigest"].is_string()
                || !preflight["expiresAt"].as_f64().is_some_and(|v| v > now_ms() as f64)
            {
                return Err(LiveError::error("remote mutation authority preflight failed"));
            }
            fields["method"] = "prepare".into();
            fields["preflightToken"] = preflight["preflightToken"].clone();
            fields["confirmation"] = preflight["confirmation"].clone();
            fields["idempotencyKey"] = key.into();
            let prepared = self.request(fields, "authority.prepare", context).await?;
            if !prepared["authorityToken"].is_string()
                || prepared["operation"] != invocation.operation
                || prepared["argsDigest"] != preflight["argsDigest"]
                || !prepared["expiresAt"].as_f64().is_some_and(|v| v > now_ms() as f64)
            {
                return Err(LiveError::error("remote mutation authority preparation failed"));
            }
            Ok(prepared)
        }
        .await
        .map_err(|e: LiveError| LiveError::MutationNotDispatched(e.to_string()))?;
        let mut fields = json!({"method":"invoke","operation":invocation.operation,"args":invocation.args,"authorityToken":prepare["authorityToken"],"transactionId":transaction});
        if let Some(token) = ownership {
            fields["ownershipToken"] = token.into();
        }
        self.request(fields, &invocation.operation, context).await.map_err(classify_refusal)
    }
    async fn mutate(
        &self,
        invocation: &LiveInvocation,
        transaction: &str,
        key: &str,
        ownership: Option<&str>,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        let expectation = {
            let mut ledger = self.0.expected_digests.borrow_mut();
            let index = ledger.iter().position(|(id, _)| id == transaction);
            index
                .filter(|i| {
                    ledger[*i].1.operation == invocation.operation
                        && ledger[*i].1.references == references_key(&Value::Object(invocation.args.clone()))
                })
                .and_then(|i| ledger.remove(i))
        };
        let mut digest = None;
        if let Some((_, expectation)) = expectation {
            if expectation.expires_at > now_ms() as f64 {
                digest = expectation.digest.await;
            }
        }
        *self.0.changing.borrow_mut().entry(transaction.into()).or_default() += 1;
        let mut fields = json!({"method":"mutate","operation":invocation.operation,"args":invocation.args,"transactionId":transaction,"idempotencyKey":key});
        if let Some(token) = ownership {
            fields["ownershipToken"] = token.into();
        }
        if let Some(digest) = digest.filter(|d| !d.is_empty()) {
            fields["stateDigest"] = digest.into();
        }
        let result = self.request(fields, &invocation.operation, context).await.map_err(classify_refusal);
        if result.is_ok() {
            let mut rows = self.0.unretired.borrow_mut();
            rows.retain(|id| id != transaction);
            rows.push_back(transaction.into());
        }
        {
            let mut changing = self.0.changing.borrow_mut();
            let count = changing.get(transaction).copied().unwrap_or(1) - 1;
            if count > 0 {
                changing.insert(transaction.into(), count);
            } else {
                changing.remove(transaction);
            }
        }
        if self.0.unretired.borrow().len() > self.0.endpoint.retire_after.unwrap_or(4096).max(2) {
            self.retire_oldest_soon();
        }
        result
    }
    fn retire_oldest_soon(&self) {
        if self.0.retiring.replace(true) {
            return;
        }
        let this = self.clone();
        tokio::task::spawn_local(async move {
            let ids: Vec<_> = this.0.unretired.borrow().iter().cloned().collect();
            for id in ids {
                if this.0.unretired.borrow().len() <= this.0.endpoint.retire_after.unwrap_or(4096).max(2) / 2 {
                    break;
                }
                if this.0.changing.borrow().contains_key(&id) {
                    continue;
                }
                this.0.unretired.borrow_mut().retain(|item| item != &id);
                let context = LiveOperationContext::with_deadline(now_ms() as f64 + this.0.endpoint.timeout());
                let _ = this.retire_transaction_async(&id, Some(&context), false).await;
            }
            this.0.retiring.set(false);
        });
    }
}
fn classify_refusal(error: LiveError) -> LiveError {
    static REFUSED: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"; nothing changed\b|Live state changed since the preview$").unwrap());
    if REFUSED.is_match(error.message()) {
        LiveError::MutationNotDispatched(error.to_string())
    } else {
        error
    }
}
impl LiveAdapter for RemoteScriptLiveAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.0.cached.borrow().clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        Err(LiveError::error("remote adapter is asynchronous; use snapshotAsync"))
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        Err(LiveError::error("remote adapter is asynchronous; use getAsync"))
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("remote adapter is asynchronous; use invokeAsync"))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        Err(LiveError::error("remote adapter is asynchronous; use reconnectAsync"))
    }
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.0.listeners.add(listener.clone());
        let weak = Rc::downgrade(&self.0);
        Ok(Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.listeners.remove(&listener);
            }
        }))
    }
}
#[async_trait(?Send)]
impl AsyncLiveAdapter for RemoteScriptLiveAdapter {
    async fn snapshot_async(
        &self,
        context: Option<&LiveOperationContext>,
        request: Option<&LiveSnapshotRequest>,
    ) -> Result<LiveSnapshot, LiveError> {
        self.ensure_connected(context).await?;
        let request = request.cloned().unwrap_or_default();
        let args = serde_json::to_value(&request)?;
        let mut fields = json!({"method":"snapshot"});
        if args.as_object().is_some_and(|row| !row.is_empty()) {
            fields["args"] = args;
        }
        let mut result = self.request(fields, "snapshot", context).await?;
        expand_pad_chains(&mut result);
        check_snapshot_answer(serde_json::from_value(result)?, &request)
    }
    async fn discover_async(
        &self,
        request: &LiveDiscoveryRequest,
        context: Option<&LiveOperationContext>,
    ) -> Result<LiveDiscoveryResult, LiveError> {
        self.ensure_connected(context).await?;
        let mut args = json!({"kind":request.kind.as_str().replace('-',"_")});
        args["kind"] = request.kind.as_str().replace('-', "_").into();
        if let Some(v) = &request.parent {
            args["parent"] = json!(v);
        }
        if let Some(v) = &request.filter {
            args["filters"] = json!(v);
        }
        if let Some(v) = &request.fields {
            args["requestedFields"] = json!(v);
        }
        if let Some(v) = request.budget {
            args["traversalBudget"] = json!(v);
        }
        if let Some(v) = request.limit {
            args["limit"] = json!(v);
        }
        if let Some(v) = &request.cursor {
            args["cursor"] = json!(v);
        }
        let operation = if request.kind == LiveDiscoveryKind::SessionPlayback { "session.playback" } else { "discover" };
        let mut result = self.request(json!({"method":"discover","args":args}), operation, context).await?;
        expand_pad_chains(&mut result);
        if request.kind == LiveDiscoveryKind::SessionPlayback {
            return Ok(serde_json::from_value(
                json!({"epoch":result["epoch"],"items":[result.clone()],"truncated":false,"revision":result["revision"],"kind":request.kind}),
            )?);
        }
        let translated = result["kind"].as_str().unwrap_or_default().replace('_', "-");
        if LiveDiscoveryKind::parse(&translated) != Some(request.kind) {
            return Err(LiveError::error("remote discovery returned an unexpected kind"));
        }
        result["kind"] = translated.into();
        // The page's rows move into the result as they are: from_value would rebuild every row's map.
        let rows = result.get_mut("items").map(Value::take);
        if rows.is_some() {
            result["items"] = Value::Array(Vec::new());
        }
        let mut page: LiveDiscoveryResult = serde_json::from_value(result)?;
        page.items = match rows {
            Some(Value::Array(rows)) => rows
                .into_iter()
                .map(|row| match row {
                    Value::Object(row) => Ok(row),
                    other => Err(<serde_json::Error as serde::de::Error>::invalid_type(unexpected(&other), &"a map")),
                })
                .collect::<Result<_, _>>()?,
            Some(other) => return Err(<serde_json::Error as serde::de::Error>::invalid_type(unexpected(&other), &"a sequence").into()),
            None => return Err(<serde_json::Error as serde::de::Error>::missing_field("items").into()),
        };
        Ok(page)
    }
    async fn get_async(&self, reference: &LiveRef, context: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.ensure_connected(context).await?;
        let mut result = self.request(json!({"method":"get","ref":reference}), "get", context).await?;
        expand_pad_chains(&mut result);
        Ok(Some(result))
    }
    async fn invoke_async(&self, invocation: &LiveInvocation, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        for attempt in 1..=10 {
            match self.invoke_once(invocation, context).await {
                Err(error)
                    if error.message().ends_with("retry shortly")
                        && attempt < 10
                        && !cancelled(context)
                        && context.and_then(|c| c.deadline_ms).is_none_or(|deadline| deadline - now_ms() as f64 > 250.0) =>
                {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
                result => return result,
            }
        }
        unreachable!()
    }
    async fn reconnect_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.ensure_connected(context).await?;
        let value = self.request(json!({"method":"reconnect"}), "reconnect", context).await?;
        if !valid_status(&value) {
            return Err(LiveError::error("invalid reconnect status"));
        }
        let status: LiveStatus = serde_json::from_value(value)?;
        let changed = Self::shape_changed(&self.0.cached.borrow(), &status);
        self.0.epoch.set(status.epoch);
        *self.0.cached.borrow_mut() = status.clone();
        self.0.last_event_epoch.set(status.epoch);
        self.0.last_event_sequence.set(0);
        self.0.cleanup_ownership.borrow_mut().clear();
        self.0.unretired.borrow_mut().clear();
        self.0.expected_digests.borrow_mut().clear();
        if changed {
            self.emit_status();
        }
        Ok(status)
    }
    async fn close(&self) -> Result<(), LiveError> {
        self.0.explicitly_closed.set(true);
        let error = LiveError::error("remote adapter disconnected");
        self.fail_pending(error.clone());
        if let Some(hello) = self.0.hello.take() {
            let _ = hello.send(Err(error));
        }
        self.0.connection.borrow().cancel();
        self.0.writer.take();
        let mut status = self.0.cached.borrow_mut();
        status.connected = false;
        status.reason = Some("closed".into());
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.ensure_connected(context).await?;
        let value = self.request(json!({"method":"status"}), "status", context).await?;
        if !valid_status(&value) {
            return Err(LiveError::error("invalid refreshed status"));
        }
        let status: LiveStatus = serde_json::from_value(value)?;
        let changed = Self::shape_changed(&self.0.cached.borrow(), &status);
        *self.0.cached.borrow_mut() = status.clone();
        if changed {
            self.emit_status();
        }
        Ok(status)
    }
    fn has_subscribe_status(&self) -> bool {
        true
    }
    fn subscribe_status(&self, listener: StatusListener) -> Unsubscribe {
        self.0.status_listeners.add(listener.clone());
        let weak = Rc::downgrade(&self.0);
        Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.status_listeners.remove(&listener);
            }
        })
    }
    fn has_retire_transaction_async(&self) -> bool {
        true
    }
    async fn retire_transaction_async(
        &self,
        transaction: &str,
        context: Option<&LiveOperationContext>,
        terminal: bool,
    ) -> Result<Value, LiveError> {
        if !(8..=128).contains(&kumi_common::js::string::utf16_len(transaction)) {
            return Err(LiveError::error("remote retirement transaction id is invalid"));
        }
        self.0.unretired.borrow_mut().retain(|id| id != transaction);
        self.ensure_connected(context).await?;
        let mut fields = json!({"method":"retire","transactionId":transaction});
        if terminal {
            fields["terminal"] = true.into();
        }
        let result = self.request(fields, "authority.retire", context).await?;
        if terminal {
            self.0.cleanup_ownership.borrow_mut().remove(transaction);
        }
        Ok(result)
    }
    fn retires_on_its_own(&self) -> bool {
        self.0.endpoint.mutation_path.unwrap_or_default() == MutationPath::Mutate
    }
    fn has_expect_state_digest(&self) -> bool {
        true
    }
    fn expect_state_digest(&self, transaction: &str, invocation: &LiveInvocation) {
        if !self.retires_on_its_own()
            || !self.0.cached.borrow().has_operation("authority.digest")
            || !mutation_authority_required(&invocation.operation)
        {
            return;
        }
        let now = now_ms() as f64;
        let mut ledger = self.0.expected_digests.borrow_mut();
        ledger.retain(|(_, row)| row.expires_at > now);
        while ledger.len() >= 4096 {
            ledger.pop_front();
        }
        let this = self.clone();
        let invocation = invocation.clone();
        let operation = invocation.operation.clone();
        let references = references_key(&Value::Object(invocation.args.clone()));
        let digest = async move {
            this.request(
                json!({"method":"invoke","operation":"authority.digest","args":{"operation":invocation.operation,"args":invocation.args}}),
                "authority.digest",
                None,
            )
            .await
            .ok()
            .and_then(|value| value["stateDigest"].as_str().map(str::to_owned))
        }
        .boxed_local()
        .shared();
        // JS starts this request before returning from expectStateDigest.
        let _ = digest.clone().now_or_never();
        let background = digest.clone();
        tokio::task::spawn_local(async move {
            let _ = background.await;
        });
        let row = Expectation { operation, references, digest, expires_at: now + 600000.0 };
        if let Some((_, prior)) = ledger.iter_mut().find(|(id, _)| id == transaction) {
            *prior = row;
        } else {
            ledger.push_back((transaction.into(), row));
        }
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|v| v != 0.0),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}

/// How serde names a value's type in an "invalid type" error.
fn unexpected(value: &Value) -> serde::de::Unexpected<'_> {
    use serde::de::Unexpected;
    match value {
        Value::Null => Unexpected::Unit,
        Value::Bool(flag) => Unexpected::Bool(*flag),
        Value::Number(number) => number
            .as_i64()
            .map(Unexpected::Signed)
            .or_else(|| number.as_u64().map(Unexpected::Unsigned))
            .unwrap_or_else(|| Unexpected::Float(number.as_f64().unwrap_or_default())),
        Value::String(text) => Unexpected::Str(text),
        Value::Array(_) => Unexpected::Seq,
        Value::Object(_) => Unexpected::Map,
    }
}
