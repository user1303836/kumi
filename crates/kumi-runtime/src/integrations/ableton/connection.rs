//! Bridge connection, reconnection, catalog refresh, and observation leases.
use super::{
    context::{self, ObservationError},
    focus::{start_focus_feed, FocusFeed, FocusFeedOptions},
    pins::pointed_pin,
    references::References,
    views::{self, ViewHost},
    BRIDGE_TOOLS,
};
use crate::{
    command::KUMI,
    core::{
        contracts::*,
        errors::{FailureKind, KumiError, RuntimeError},
    },
    mcp::{
        allowed_tools::{AllowedTools, CallOptions},
        client::{self, McpEndpoint},
        types::CallToolResult,
    },
};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, number::to_string},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::{Rc, Weak},
    time::Duration,
};
use tokio::time::sleep;

/// How many changes of Live's playing a connection keeps.
const HEARD_KEPT: usize = 512;

/// The first time at or after `since` in `heard` (each change of playing, oldest first) that Live played.
pub fn first_heard(heard: &VecDeque<(i64, bool)>, since: i64) -> Option<i64> {
    if heard.iter().rev().find(|(at, _)| *at <= since).is_some_and(|(_, playing)| *playing) {
        return Some(since);
    }
    heard.iter().find(|(at, playing)| *at > since && *playing).map(|(at, _)| *at)
}

pub const NO_CURRENT_LIVE:&str="Kumi has no current view of Live: it disconnected, or the Set changed. Kumi reconnects on its own when Live is back; tell the producer, and don't describe earlier readings as current.";
pub type Connect = Rc<dyn Fn(Signal) -> LocalBoxFuture<'static, Result<Rc<dyn McpEndpoint>, RuntimeError>>>;
pub struct ConnectionOptions {
    pub on_connection: ConnectionListener,
    pub bridge_config: Option<String>,
    pub connect: Option<Connect>,
    pub now: Option<Rc<dyn Fn() -> DateTime<Utc>>>,
    pub generation: Option<String>,
    pub on_dispatch: Option<Rc<dyn Fn(String)>>,
    pub on_focus: Option<Rc<dyn Fn(Option<LiveFocus>)>>,
    pub on_pointed: Option<Rc<dyn Fn(PinnedNode)>>,
    pub on_transport: Option<Rc<dyn Fn(Option<LiveTransport>)>>,
    pub focus_interval_ms: Option<u64>,
    pub reconnect_interval_ms: Option<u64>,
    /// The owner retires HISTORY entries whose transactions belonged to the old bridge or Live epoch.
    pub on_retire: Option<Rc<dyn Fn(&str)>>,
    /// A Live restart lets the owner retry its Ears device setup.
    pub on_live_lost: Option<Rc<dyn Fn()>>,
}
impl ConnectionOptions {
    pub fn new(on_connection: ConnectionListener) -> Self {
        Self {
            on_connection,
            bridge_config: None,
            connect: None,
            now: None,
            generation: None,
            on_dispatch: None,
            on_focus: None,
            on_pointed: None,
            on_transport: None,
            focus_interval_ms: None,
            reconnect_interval_ms: None,
            on_retire: None,
            on_live_lost: None,
        }
    }
}
#[derive(Debug)]
pub enum ReadError {
    Observation(ObservationError),
    Other(RuntimeError),
}
impl From<ObservationError> for ReadError {
    fn from(e: ObservationError) -> Self {
        Self::Observation(e)
    }
}
impl From<RuntimeError> for ReadError {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::Observation(message) => Self::Observation(ObservationError(message)),
            e => Self::Other(e),
        }
    }
}
impl From<kumi_common::abort::Aborted> for ReadError {
    fn from(_: kumi_common::abort::Aborted) -> Self {
        Self::Other(RuntimeError::Aborted)
    }
}
impl From<ReadError> for RuntimeError {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::Observation(e) => e.into(),
            ReadError::Other(e) => e,
        }
    }
}
fn observation(message: &str) -> ReadError {
    ReadError::Observation(ObservationError(message.into()))
}
type Closing = Shared<LocalBoxFuture<'static, Result<(), RuntimeError>>>;
pub struct LiveConnection {
    options: ConnectionOptions,
    weak: Weak<Self>,
    pub generation: String,
    pub lifetime: Signal,
    pub references: RefCell<References>,
    endpoint: RefCell<Option<Rc<dyn McpEndpoint>>>,
    tools: RefCell<Option<AllowedTools>>,
    unlisten: RefCell<Vec<Box<dyn Fn()>>>,
    focus: RefCell<Option<FocusFeed>>,
    pub closed: Cell<bool>,
    pub closing_started: Cell<bool>,
    pub started: Cell<bool>,
    pub available: Cell<bool>,
    pub lost: Cell<bool>,
    pub lease: Cell<u64>,
    pub epoch: Cell<Option<f64>>,
    pub set: RefCell<Option<String>>,
    pub last_epoch: Cell<Option<f64>>,
    pub reconnected: Cell<bool>,
    /// How many structure changes Live has told of (tracks, returns or scenes added, removed or moved).
    pub structure_events: Cell<u64>,
    /// Endpoints attached so far: Live's events between two of them reach no one.
    pub attachments: Cell<u64>,
    /// Whether the endpoint attached passes Live's events on, and whether Live took the subscription.
    listening: Cell<bool>,
    hearing: Cell<bool>,
    lost_epoch: Cell<Option<f64>>,
    looking: Cell<bool>,
    watcher: RefCell<Option<Signal>>,
    last_fresh_bridge: Cell<i64>,
    closing: RefCell<Option<Closing>>,
    subscribed: Cell<bool>,
    transport_events: Cell<bool>,
    transport_reading: Cell<bool>,
    transport_again: Cell<bool>,
    transport_timer: RefCell<Option<Signal>>,
    bar_beats: Cell<Option<f64>>,
    transport_reads: Cell<u64>,
    last_transport: RefCell<String>,
    /// When Live was heard playing: each time that changed (ms, and whether it was), newest last.
    heard: RefCell<VecDeque<(i64, bool)>>,
    /// Kumi is playing the Set for itself, Main down (a silent render): playing then isn't heard.
    pub rendering: Cell<bool>,
}
impl LiveConnection {
    pub fn new(options: ConnectionOptions) -> Rc<Self> {
        Rc::new_cyclic(|weak| Self {
            generation: options.generation.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            options,
            weak: weak.clone(),
            lifetime: Signal::new(),
            references: RefCell::new(References::default()),
            endpoint: RefCell::new(None),
            tools: RefCell::new(None),
            unlisten: RefCell::new(Vec::new()),
            focus: RefCell::new(None),
            closed: Cell::new(false),
            closing_started: Cell::new(false),
            started: Cell::new(false),
            available: Cell::new(false),
            lost: Cell::new(false),
            lease: Cell::new(0),
            epoch: Cell::new(None),
            set: RefCell::new(None),
            last_epoch: Cell::new(None),
            reconnected: Cell::new(false),
            structure_events: Cell::new(0),
            attachments: Cell::new(0),
            listening: Cell::new(false),
            hearing: Cell::new(false),
            lost_epoch: Cell::new(None),
            looking: Cell::new(false),
            watcher: RefCell::new(None),
            last_fresh_bridge: Cell::new(0),
            closing: RefCell::new(None),
            subscribed: Cell::new(false),
            transport_events: Cell::new(false),
            transport_reading: Cell::new(false),
            transport_again: Cell::new(false),
            transport_timer: RefCell::new(None),
            bar_beats: Cell::new(None),
            transport_reads: Cell::new(0),
            last_transport: RefCell::new(String::new()),
            heard: RefCell::new(VecDeque::new()),
            rendering: Cell::new(false),
        })
    }
    pub fn now(&self) -> DateTime<Utc> {
        self.options.now.as_ref().map(|f| f()).unwrap_or_else(Utc::now)
    }
    pub fn iso_now(&self) -> String {
        self.now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }
    pub fn tools(&self) -> Option<AllowedTools> {
        self.tools.borrow().clone()
    }
    pub fn invalidate(&self) {
        self.references.borrow_mut().invalidate();
        self.epoch.set(None);
        self.lease.set(self.lease.get() + 1);
    }
    pub fn discard_reads(&self) {
        let mut book = self.references.borrow_mut();
        book.refs.clear();
        book.cursors.clear();
    }
    pub fn assert_lease(&self, lease: u64, signal: &Signal) -> Result<(), ReadError> {
        signal.check()?;
        self.lifetime.check()?;
        if self.closed.get() || lease != self.lease.get() {
            return Err(observation("Observation changed; late result discarded"));
        }
        Ok(())
    }
    pub fn changed(&self) -> ReadError {
        self.invalidate();
        (self.options.on_connection)(ConnectionState::Error, None);
        observation("Live epoch or Set identity changed; result discarded. Refresh before continuing.")
    }
    pub fn assert_epoch(&self, actual: Option<&Value>, expected: f64) -> Result<(), ReadError> {
        if actual.and_then(Value::as_f64) != Some(expected) {
            Err(self.changed())
        } else {
            Ok(())
        }
    }
    /// Whether Live's events reach Kumi now: Live took the subscription, through an endpoint that passes them on.
    pub fn hears_live(&self) -> bool {
        self.hearing.get() && self.listening.get()
    }
    pub fn register_rows(&self, kind: &str, rows: &[JsonObject], args: &JsonObject, next: Option<&str>) -> Result<(), ReadError> {
        let result = self.references.borrow_mut().register_rows(kind, rows, args, next);
        if let Err(error) = result {
            if error.invalidate {
                self.invalidate();
            }
            return Err(error.error.into());
        }
        Ok(())
    }
    async fn open_endpoint(&self, signal: Signal) -> Result<Rc<dyn McpEndpoint>, RuntimeError> {
        if let Some(connect) = &self.options.connect {
            return connect(signal).await;
        }
        let path = self
            .options
            .bridge_config
            .as_ref()
            .ok_or_else(|| RuntimeError::plain("Bridge configuration is required; choose explicit inference-only mode otherwise"))?;
        let dispatch = self.options.on_dispatch.clone().map(|f| Rc::new(move |name: &str| f(name.into())) as Rc<dyn Fn(&str)>);
        client::connect_mcp(client::Options {
            signal,
            bridge_config: Some(path.into()),
            allow_tools: BRIDGE_TOOLS.clone(),
            on_dispatch: dispatch,
            ..Default::default()
        })
        .await
    }
    pub async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        if self.closed.get() || self.started.get() {
            return Err(RuntimeError::plain("Integration cannot be started again"));
        }
        self.started.set(true);
        (self.options.on_connection)(ConnectionState::Connecting, None);
        let signal = abort::any([signal, self.lifetime.clone()]);
        let result = async {
            let fresh = self.open_endpoint(signal.clone()).await?;
            if signal.aborted() || self.closed.get() {
                fresh.close().await?;
                signal.check()?;
                return Err(RuntimeError::plain("Connection closed"));
            }
            self.attach(fresh);
            self.available.set(true);
            Ok::<_, RuntimeError>(())
        }
        .await;
        if let Err(error) = result {
            (self.options.on_connection)(ConnectionState::Error, None);
            // Live still running the bridge it loaded before an update: restart Live, not another update, in the
            // bridge's own words. Anything else gets the usual way back after an update, with what the bridge said.
            let said = error.to_string();
            let message = if said.contains(kumi_common::bridge::ANOTHER_BRIDGE) {
                said
            } else {
                let cause = said.strip_prefix("Kumi's bridge didn't start: ").map(|cause| format!(" ({cause})")).unwrap_or_default();
                format!("Kumi couldn't start its bridge to Live{cause}. After updating Kumi, Live's part needs updating too: quit Live, then run {} bridge. Otherwise: {} doctor",*KUMI,*KUMI)
            };
            return Err(KumiError::new(FailureKind::Live, message).into());
        }
        Ok(())
    }
    fn attach(&self, endpoint: Rc<dyn McpEndpoint>) {
        *self.endpoint.borrow_mut() = Some(endpoint.clone());
        *self.tools.borrow_mut() = Some(AllowedTools::new(endpoint.clone(), BRIDGE_TOOLS.iter().cloned().collect::<HashSet<_>>()));
        let weak = self.weak.clone();
        self.unlisten.borrow_mut().push(endpoint.on_disconnect(Rc::new(move || {
            if let Some(this) = weak.upgrade() {
                this.lose_access();
            }
        })));
        if let Some(feed) = self.focus.borrow_mut().take() {
            feed.stop();
        }
        if let Some(on_focus) = &self.options.on_focus {
            let read_endpoint = endpoint.clone();
            let weak = self.weak.clone();
            let on_failure = Rc::new(move || {
                let Some(this) = weak.upgrade() else { return };
                if this.lost.get() {
                    return;
                }
                tokio::task::spawn_local(async move {
                    if let Ok(status) = this.read_status(abort::timeout(1500)).await {
                        if status.get("connected") == Some(&Value::Bool(false)) {
                            this.lose_live();
                        }
                    }
                });
            });
            *self.focus.borrow_mut() = Some(start_focus_feed(FocusFeedOptions {
                read: Rc::new(move |signal| {
                    let endpoint = read_endpoint.clone();
                    async move { endpoint.call("live_discover", views::object(json!({"kind":"selection","limit":1})), signal).await }
                        .boxed_local()
                }),
                on_focus: on_focus.clone(),
                on_failure: Some(on_failure),
                interval_ms: self.options.focus_interval_ms.filter(|n| *n != 0),
                timeout_ms: None,
            }));
        }
        self.subscribed.set(false);
        self.hearing.set(false);
        self.attachments.set(self.attachments.get() + 1);
        self.listening.set(endpoint.has_on_live_event());
        if endpoint.has_on_live_event() {
            let weak = self.weak.clone();
            self.unlisten.borrow_mut().push(endpoint.on_live_event(Rc::new(move |event| {
                if let Some(this) = weak.upgrade() {
                    this.live_event(event);
                }
            })));
        }
    }
    pub async fn ensure_catalog(&self, signal: Signal) -> Result<(), ReadError> {
        let Some(tools) = self.tools() else { return Ok(()) };
        if tools.is_valid() {
            return Ok(());
        }
        if tools.refresh(signal.clone()).await.is_err() {
            signal.check()?;
            tools.refresh(signal).await?;
        }
        Ok(())
    }
    pub async fn read_status(&self, signal: Signal) -> Result<JsonObject, ReadError> {
        self.ensure_catalog(signal.clone()).await?;
        if !self.has("live_status") {
            return Err(observation("Live status capability is unavailable"));
        }
        Ok(context::status_payload(&self.call("live_status", JsonObject::new(), signal).await?)?)
    }
    pub async fn guard_epoch(&self, signal: Signal, epoch: f64, lease: u64) -> Result<JsonObject, ReadError> {
        let status = self.read_status(signal.clone()).await?;
        self.assert_lease(lease, &signal)?;
        if status.get("connected") != Some(&Value::Bool(true)) {
            self.lose_live();
            return Err(observation("No Live access; current observations were discarded"));
        }
        self.assert_epoch(status.get("epoch"), epoch)?;
        Ok(status)
    }
    fn stop_timer(timer: &RefCell<Option<Signal>>) {
        if let Some(signal) = timer.borrow_mut().take() {
            signal.cancel();
        }
    }
    fn stop_focus(&self) {
        if let Some(feed) = self.focus.borrow().as_ref() {
            feed.stop();
        }
    }
    pub fn lose_access(&self) {
        if self.closed.get() || self.closing_started.get() || (self.lost.get() && !self.available.get()) {
            return;
        }
        self.available.set(false);
        self.stop_focus();
        Self::stop_timer(&self.transport_timer);
        self.report_transport(None);
        if !self.lost.replace(true) {
            self.lost_epoch.set(self.last_epoch.get());
            self.invalidate();
            (self.options.on_connection)(ConnectionState::Disconnected, Some(DisconnectCause::Bridge));
        }
        self.keep_looking();
    }
    pub fn lose_live(&self) {
        if self.closed.get() || self.closing_started.get() || self.lost.replace(true) {
            return;
        }
        self.lost_epoch.set(self.last_epoch.get());
        self.invalidate();
        (self.options.on_connection)(ConnectionState::Disconnected, Some(DisconnectCause::Live));
        if let Some(callback) = &self.options.on_live_lost {
            callback();
        }
        Self::stop_timer(&self.transport_timer);
        self.report_transport(None);
        self.keep_looking();
    }
    fn keep_looking(&self) {
        Self::stop_timer(&self.watcher);
        let stop = Signal::new();
        *self.watcher.borrow_mut() = Some(stop.clone());
        let weak = self.weak.clone();
        let interval = self.options.reconnect_interval_ms.unwrap_or(2000).max(1);
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {biased;_=stop.cancelled()=>break,_=sleep(Duration::from_millis(interval))=>{let Some(this)=weak.upgrade() else{break};tokio::task::spawn_local(async move{this.look_for_live().await;});}}
            }
        });
    }
    async fn remote_script_listening(&self) -> bool {
        let Some(path) = &self.options.bridge_config else { return false };
        let Ok(bytes) = std::fs::read(path) else { return false };
        let Ok(config) = serde_json::from_slice::<Value>(&bytes) else { return false };
        let Some(host) = config["bridge"]["host"].as_str().filter(|host| ["127.0.0.1", "localhost", "::1"].contains(host)) else {
            return false;
        };
        let Some(port) = config["bridge"]["port"].as_f64().filter(|n| n.fract() == 0. && *n >= 0. && *n <= 65535.) else { return false };
        matches!(tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect((host, port as u16))).await, Ok(Ok(_)))
    }
    pub async fn look_for_live(&self) {
        if self.closed.get() || !self.lost.get() || self.looking.replace(true) {
            return;
        }
        let _ = async {
            let answering = if self.available.get() {
                let status = self.read_status(abort::any([self.lifetime.clone(), abort::timeout(1500)])).await?;
                if status.get("connected") == Some(&Value::Bool(true)) {
                    self.back(status.get("epoch").and_then(Value::as_f64) != self.lost_epoch.get(), false);
                    return Ok::<_, ReadError>(());
                }
                status.get("reason").and_then(Value::as_str) == Some("remote-bridge-or-live-epoch-changed")
                    || self.remote_script_listening().await
            } else {
                self.remote_script_listening().await
            };
            let now = Utc::now().timestamp_millis();
            if !answering && now - self.last_fresh_bridge.get() < 30_000 {
                return Ok(());
            }
            self.last_fresh_bridge.set(now);
            let signal = abort::any([self.lifetime.clone(), abort::timeout(20_000)]);
            let fresh = self.open_endpoint(signal.clone()).await?;
            let status = match fresh.call("live_status", JsonObject::new(), signal).await {
                Ok(read) => context::status_payload(&read).ok(),
                Err(_) => None,
            };
            let ready = status.as_ref().is_some_and(|s| s.get("connected") == Some(&Value::Bool(true)));
            if !ready || self.closed.get() || !self.lost.get() {
                let _ = fresh.close().await;
                return Ok(());
            }
            let old = self.tools();
            for remove in self.unlisten.borrow_mut().drain(..) {
                remove();
            }
            self.attach(fresh);
            self.available.set(true);
            if let Some(old) = old {
                tokio::task::spawn_local(async move {
                    let _ = old.close().await;
                });
            }
            self.back(status.as_ref().and_then(|s| s.get("epoch")).and_then(Value::as_f64) != self.lost_epoch.get(), true);
            Ok(())
        }
        .await;
        self.looking.set(false);
    }
    fn back(&self, restarted: bool, fresh_bridge: bool) {
        Self::stop_timer(&self.watcher);
        if let Some(retire) = &self.options.on_retire {
            if restarted {
                retire("Live restarted since, so Kumi can't undo this; it's in the Set only if the Set was saved.");
            } else if fresh_bridge {
                retire("Kumi's link to Live restarted since, so Kumi can't undo this; Live's own undo still can.");
            }
        }
        self.lost.set(false);
        self.reconnected.set(true);
        (self.options.on_connection)(ConnectionState::Connected, None);
    }
    pub fn notify_connected(&self) {
        (self.options.on_connection)(ConnectionState::Connected, None);
    }
    /// Stop reconnecting while the integration writes its final bounded project snapshot.
    pub fn prepare_close(&self) {
        self.closing_started.set(true);
        Self::stop_timer(&self.watcher);
    }
    pub fn close(&self) -> Closing {
        if let Some(closing) = self.closing.borrow().clone() {
            return closing;
        }
        self.prepare_close();
        self.closed.set(true);
        self.available.set(false);
        self.lifetime.cancel();
        self.invalidate();
        self.stop_focus();
        Self::stop_timer(&self.transport_timer);
        for remove in self.unlisten.borrow_mut().drain(..) {
            remove();
        }
        let tools = self.tools();
        let endpoint = self.endpoint.borrow().clone();
        let closing = async move {
            if let Some(tools) = tools {
                tools.close().await
            } else if let Some(endpoint) = endpoint {
                endpoint.close().await
            } else {
                Ok(())
            }
        }
        .boxed_local()
        .shared();
        *self.closing.borrow_mut() = Some(closing.clone());
        closing
    }
    pub async fn subscribe(&self, signal: Signal) {
        if self.subscribed.get() || !self.has("live_subscribe") {
            self.spawn_transport();
            return;
        }
        self.subscribed.set(true);
        let result = async {
            let mut result = if self.options.on_transport.is_some() {
                Some(
                    self.call("live_subscribe", views::object(json!({"types":["selection","structure","transport"]})), signal.clone())
                        .await?,
                )
            } else {
                None
            };
            self.transport_events.set(result.as_ref().is_some_and(|r| r.is_error != Some(true)));
            if !self.transport_events.get() {
                result = Some(self.call("live_subscribe", views::object(json!({"types":["selection","structure"]})), signal).await?);
            }
            let heard = result.as_ref().is_some_and(|r| r.is_error != Some(true));
            if heard {
                if let Some(focus) = self.focus.borrow().as_ref() {
                    focus.slow();
                }
            }
            Ok::<_, RuntimeError>(heard)
        }
        .await;
        // Heard only once Live said yes. A refusal, or a call that failed, is asked again on the next turn.
        let heard = matches!(result, Ok(true));
        self.hearing.set(heard);
        if !heard {
            self.subscribed.set(false);
            self.transport_events.set(false);
        }
        self.spawn_transport();
    }
    fn spawn_transport(&self) {
        if let Some(this) = self.weak.upgrade() {
            // The transport clock reads Live on a timer, for the producer's view, not for a turn.
            tokio::task::spawn_local(crate::core::timing::background(async move {
                this.read_transport().await;
            }));
        }
    }
    /// What a transport read found: Live playing is heard, unless Kumi is playing the Set for itself.
    pub fn note_transport(&self, playing: bool) {
        self.note_heard(playing && !self.rendering.get());
    }
    /// Whether Live is heard playing now, kept when that changes.
    fn note_heard(&self, playing: bool) {
        let mut heard = self.heard.borrow_mut();
        if heard.back().map(|(_, was)| *was) != Some(playing) {
            heard.push_back((self.now().timestamp_millis(), playing));
            while heard.len() > HEARD_KEPT {
                heard.pop_front();
            }
        }
    }
    /// The first time at or after `since` that Live was heard playing: at once when it was playing then.
    pub fn first_heard(&self, since: i64) -> Option<i64> {
        first_heard(&self.heard.borrow(), since)
    }
    fn report_transport(&self, transport: Option<LiveTransport>) {
        let key = transport
            .as_ref()
            .map(|t| {
                format!(
                    "{}:{}:{}:{}",
                    t.playing,
                    t.tempo.map(to_string).unwrap_or_else(|| "undefined".into()),
                    t.beats_per_bar.map(to_string).unwrap_or_else(|| "undefined".into()),
                    if t.playing { t.beat.map(to_string).unwrap_or_else(|| "undefined".into()) } else { String::new() }
                )
            })
            .unwrap_or_else(|| "null".into());
        if *self.last_transport.borrow() == key {
            return;
        }
        *self.last_transport.borrow_mut() = key;
        if let Some(callback) = &self.options.on_transport {
            let _ = catch_unwind(AssertUnwindSafe(|| callback(transport)));
        }
    }
    pub async fn read_transport(&self) {
        if self.options.on_transport.is_none() || self.closed.get() || !self.available.get() || self.lost.get() {
            return;
        }
        if self.transport_reading.replace(true) {
            self.transport_again.set(true);
            return;
        }
        Self::stop_timer(&self.transport_timer);
        let mut playing = self.last_transport.borrow().starts_with("true");
        let mut soon = false;
        let _ = async {
            if !self.has("live_discover") {
                soon = true;
                return Ok::<_, RuntimeError>(());
            }
            let signal = abort::any([self.lifetime.clone(), abort::timeout(3000)]);
            let count = self.transport_reads.get();
            self.transport_reads.set(count + 1);
            if count % 8 == 0 && self.has("live_song_state") {
                let song = self.call("live_song_state", JsonObject::new(), signal.clone()).await.ok();
                let state = match song.filter(|s| s.is_error != Some(true)) {
                    Some(song) => context::payload(&song)?,
                    None => JsonObject::new(),
                };
                let numerator = number(state.get("signatureNumerator"));
                let denominator = number(state.get("signatureDenominator"));
                if numerator > 0. && denominator > 0. {
                    self.bar_beats.set(Some(numerator * 4. / denominator));
                }
            }
            let sent = kumi_common::time::perf_now();
            let read = self
                .call("live_discover", views::object(json!({"kind":"set","fields":["tempo","position","playing"],"limit":1})), signal)
                .await?;
            let at = (sent + kumi_common::time::perf_now()) / 2.;
            let row = if read.is_error == Some(true) { None } else { views::rows(&context::payload(&read)?).first().cloned() };
            if let Some(row) = row.filter(|_| !self.closed.get()) {
                playing = row.get("playing") == Some(&Value::Bool(true));
                self.note_transport(playing);
                self.report_transport(Some(LiveTransport {
                    playing,
                    at,
                    tempo: row.get("tempo").and_then(Value::as_f64),
                    beat: row.get("position").and_then(Value::as_f64),
                    beats_per_bar: self.bar_beats.get().filter(|b| *b != 0. && !b.is_nan()),
                }));
            }
            Ok(())
        }
        .await;
        self.transport_reading.set(false);
        if self.transport_again.replace(false) {
            self.spawn_transport();
        } else if !self.closed.get() && self.available.get() && !self.lost.get() && (soon || playing || !self.transport_events.get()) {
            let stop = Signal::new();
            *self.transport_timer.borrow_mut() = Some(stop.clone());
            let weak = self.weak.clone();
            let ms = if soon {
                500
            } else if playing {
                4000
            } else {
                2500
            };
            tokio::task::spawn_local(async move {
                tokio::select! {biased;_=stop.cancelled()=>{},_=sleep(Duration::from_millis(ms))=>{if let Some(this)=weak.upgrade(){this.spawn_transport();}}}
            });
        }
    }
    fn live_event(&self, event: JsonObject) {
        if event.get("type").and_then(Value::as_str) == Some("structure") {
            self.structure_events.set(self.structure_events.get() + 1);
        }
        if matches!(event.get("type").and_then(Value::as_str), Some("selection" | "structure")) {
            if let Some(focus) = self.focus.borrow().as_ref() {
                focus.poke();
            }
        }
        if event.get("type").and_then(Value::as_str) == Some("transport") {
            self.spawn_transport();
        }
        if event.get("type").and_then(Value::as_str) == Some("pointed") {
            if let (Some(callback), Some(pin)) = (&self.options.on_pointed, pointed_pin(&event)) {
                let _ = catch_unwind(AssertUnwindSafe(|| callback(pin)));
            }
        }
    }
    /// The model's bounded reads, with references authorized by this observation alone.
    pub async fn invoke(&self, name: &str, input: JsonObject, original: Signal) -> ToolResult {
        let signal = abort::any([original, self.lifetime.clone()]);
        let lease = self.lease.get();
        let mut reading = false;
        let result: Result<ToolResult, ReadError> = async {
            signal.check()?;
            if !self.available.get() || self.lost.get() || self.epoch.get().is_none() || self.tools().is_none() {
                return Err(observation(NO_CURRENT_LIVE));
            }
            let epoch = self.epoch.get().unwrap();
            self.ensure_catalog(signal.clone()).await?;
            self.assert_lease(lease, &signal)?;
            if !self.has(name) {
                return Err(observation("That read isn't available for the open Set right now"));
            }
            let named = self.references.borrow().lengthen(&Value::Object(input));
            let named = context::object(&named)?;
            let args = if name == "live_discover" { context::discovery_args(&named)? } else { named };
            if name == "live_discover" {
                self.references.borrow().validate_parent_and_cursor(&args)?;
            } else {
                self.references.borrow().require_fresh_references(&args)?;
            }
            reading = true;
            if name != "live_discover" {
                self.guard_epoch(signal.clone(), epoch, lease).await?;
            }
            let mut result = self.call(name, args.clone(), signal.clone()).await?;
            self.assert_lease(lease, &signal)?;
            if result_size(&result) > 64 * 1024 {
                self.discard_reads();
                return Ok(ToolResult::error("Result too large; narrow fields/parent/page instead of requesting a whole Set dump."));
            }
            if result.is_error != Some(true) && name == "live_discover" {
                self.assert_epoch(context::payload(&result)?.get("epoch"), epoch)?;
                let kind = args["kind"].as_str().unwrap();
                let first = context::discovery_payload(&result, kind, epoch)?;
                let mut items = views::rows(&first);
                if kind == "set" && (items.len() != 1 || Some(context::set_identity(&context::object(&items[0])?)?) != *self.set.borrow()) {
                    return Err(self.changed());
                }
                let mut next = next_cursor(&first);
                self.register_rows(kind, &object_rows(&items)?, &args, next.as_deref())?;
                let mut pages = 1;
                while next.is_some() && pages < 3 && stringify(&json!(items)).len() < 32 * 1024 {
                    let mut page_args = args.clone();
                    page_args.insert("cursor".into(), json!(next));
                    let more = self.call(name, page_args.clone(), signal.clone()).await?;
                    self.assert_lease(lease, &signal)?;
                    if more.is_error == Some(true) || result_size(&more) > 64 * 1024 {
                        break;
                    }
                    let page = (|| -> Result<JsonObject, ReadError> {
                        self.assert_epoch(context::payload(&more)?.get("epoch"), epoch)?;
                        let page = context::discovery_payload(&more, kind, epoch)?;
                        self.register_rows(kind, &object_rows(&views::rows(&page))?, &page_args, next_cursor(&page).as_deref())?;
                        Ok(page)
                    })();
                    let Ok(page) = page else { break };
                    items.extend(views::rows(&page));
                    next = next_cursor(&page);
                    pages += 1;
                }
                if pages > 1 {
                    let mut merged = context::payload(&result)?;
                    merged.shift_remove("nextCursor");
                    merged.insert("items".into(), json!(items));
                    merged.insert("truncated".into(), json!(next.is_some()));
                    if let Some(next) = next {
                        merged.insert("nextCursor".into(), json!(next));
                    }
                    result = views::wrapped(merged);
                }
            }
            self.guard_epoch(signal.clone(), epoch, lease).await?;
            if result.is_error != Some(true) {
                if name == "live_snapshot" {
                    let data = context::payload(&result)?;
                    self.assert_epoch(data.get("epoch"), epoch)?;
                    let snapshot = context::object(data.get("snapshot").unwrap_or(&Value::Null))?;
                    if Some(context::set_identity(&context::object(snapshot.get("set").unwrap_or(&Value::Null))?)?) != *self.set.borrow() {
                        return Err(self.changed());
                    }
                } else if name == "live_status" {
                    let data = context::status_payload(&result)?;
                    if data.get("connected") != Some(&Value::Bool(true)) {
                        self.lose_live();
                    }
                    self.assert_epoch(data.get("epoch"), epoch)?;
                }
            }
            let encoded = self.references.borrow_mut().encode(&result, epoch, name == "live_discover", &self.iso_now(), &self.generation);
            if encoded.is_error {
                self.discard_reads();
            }
            Ok(encoded)
        }
        .await;
        match result {
            Ok(result) => result,
            Err(error) => {
                if reading && lease == self.lease.get() {
                    self.discard_reads();
                }
                ToolResult::error(match error {
                    ReadError::Observation(error) => error.0,
                    ReadError::Other(_) => "Live read failed; refresh current observations and narrow the request before retrying.".into(),
                })
            }
        }
    }
    pub async fn rows(&self, kind: &str, extra: JsonObject, signal: Signal) -> Result<Vec<JsonObject>, ReadError> {
        if !self.available.get() || self.lost.get() || self.epoch.get().is_none() || self.tools().is_none() {
            return Err(observation(NO_CURRENT_LIVE));
        }
        let epoch = self.epoch.get().unwrap();
        let mut input = views::object(json!({"kind":kind,"limit":self.page_limit(),"budget":self.whole_budget()}));
        input.extend(context::object(&self.references.borrow().lengthen(&Value::Object(extra)))?);
        let args = context::discovery_args(&input)?;
        let read = views::pages(self, args.clone(), abort::any([signal, self.lifetime.clone()])).await?;
        if read.is_error == Some(true) {
            use crate::mcp::types::ContentBlock;
            let text = read
                .content
                .iter()
                .filter_map(|b| if let ContentBlock::Text { text, .. } = b { Some(text.as_str()) } else { None })
                .collect::<String>();
            return Err(observation(if text.is_empty() { "Live read failed" } else { &text }));
        }
        let page = context::discovery_payload(&read, kind, epoch)?;
        self.register_rows(kind, &object_rows(&views::rows(&page))?, &args, next_cursor(&page).as_deref())?;
        let slim = super::references::slim_mixers(&views::object(json!({"items":page["items"]})));
        let shown = self.references.borrow_mut().shorten(&Value::Object(slim));
        Ok(object_rows(shown["items"].as_array().unwrap())?)
    }
}
#[async_trait(?Send)]
impl ViewHost for LiveConnection {
    fn available(&self) -> bool {
        self.available.get() && !self.lost.get()
    }
    fn has(&self, name: &str) -> bool {
        self.tools().is_some_and(|t| t.has(name))
    }
    fn version(&self) -> Option<String> {
        self.endpoint.borrow().as_ref().and_then(|e| e.server_info()).map(|s| s.version)
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        let tools = self.tools().ok_or_else(|| RuntimeError::plain(NO_CURRENT_LIVE))?;
        tools.call(name, args, signal, CallOptions { host: true }).await
    }
}
fn result_size(result: &CallToolResult) -> usize {
    stringify(&serde_json::to_value(result).unwrap()).len()
}
fn next_cursor(body: &JsonObject) -> Option<String> {
    body.get("nextCursor").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}
fn object_rows(rows: &[Value]) -> Result<Vec<JsonObject>, ObservationError> {
    rows.iter().map(context::object).collect()
}
fn number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.,
        Some(Value::Bool(v)) => {
            if *v {
                1.
            } else {
                0.
            }
        }
        Some(Value::Number(v)) => v.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(v)) => kumi_common::js::number::parse(v).unwrap_or(f64::NAN),
        Some(Value::Array(v)) => {
            fn array_text(v: &Value) -> String {
                match v {
                    Value::Null => String::new(),
                    Value::Array(a) => a.iter().map(array_text).collect::<Vec<_>>().join(","),
                    Value::Object(_) => "[object Object]".into(),
                    Value::String(s) => s.clone(),
                    v => stringify(v),
                }
            }
            kumi_common::js::number::parse(&v.iter().map(array_text).collect::<Vec<_>>().join(",")).unwrap_or(f64::NAN)
        }
        _ => f64::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_heard_when_live_plays_after_it_or_was_playing_then() {
        let heard = VecDeque::from([(1_000, true), (5_000, false), (9_000, true), (12_000, false)]);
        // Made while it played: heard at once.
        assert_eq!(first_heard(&heard, 2_000), Some(2_000));
        // Made while stopped: heard when it played next.
        assert_eq!(first_heard(&heard, 6_000), Some(9_000));
        // Never played since.
        assert_eq!(first_heard(&heard, 13_000), None);
        // Before anything was read.
        assert_eq!(first_heard(&heard, 500), Some(1_000));
        assert_eq!(first_heard(&VecDeque::new(), 500), None);
    }
}
