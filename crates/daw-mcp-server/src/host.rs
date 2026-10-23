//! What the tools do: find the running bridges of one DAW through their discovery files, connect, and report. A DAW
//! nothing can be loaded into (Logic) has a bridge of Kumi's own, which the host starts when no discovery file answers.
use crate::client::{BridgeClient, ConnectError, RequestError};
use daw_bridge::files::{read_secret, scan_discovery, Discovery, FileError};
use daw_bridge::install::{logic_target, LOGIC_BINARY};
use daw_bridge::lifecycle::Lifecycle;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use serde_json::{json, Value};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

pub const CLIENT_NAME: &str = concat!("daw-mcp-server/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
/// No answer of a bridge is longer than this. A bridge that sends more is asked for a smaller page, if the call has one.
pub const MAX_ANSWER_BYTES: usize = 256 * 1024;
/// How long a silent connection gets to answer a ping before it is called dead.
const PING_PROBE: Duration = Duration::from_millis(1500);
/// How long a bridge the host starts gets to listen and write its discovery file, looked for this often.
pub const SPAWN_WAIT: Duration = Duration::from_secs(10);
const SPAWN_POLL: Duration = Duration::from_millis(100);
/// How long a bridge the host stops gets to end by itself (removing its discovery file) before it is killed.
const STOP_WAIT: Duration = Duration::from_secs(1);
/// Names the bridge binary to start instead of the installed one; the command line reads it into
/// [`Host::with_bridge_binary`].
pub const LOGIC_BRIDGE_VAR: &str = "KUMI_LOGIC_BRIDGE";

struct Current {
    pid: u32,
    resource: String,
    port: u16,
    client: BridgeClient,
}

/// The host's connection to one instance as work beside the calls follows it: a connection the host replaces (or makes
/// again) is the one used from then on.
pub(crate) struct Following {
    current: std::sync::Arc<tokio::sync::Mutex<Option<Current>>>,
    instance: Pick,
}

impl Following {
    /// The connection the host has open to the instance now, if it has one: the host makes them, this only follows.
    pub(crate) async fn client(&self) -> Option<BridgeClient> {
        let current = self.current.lock().await;
        let open = current.as_ref().filter(|open| {
            !open.client.is_closed()
                && self.instance.pid.is_none_or(|pid| pid == open.pid)
                && self.instance.resource.as_ref().is_none_or(|resource| *resource == open.resource)
        })?;
        Some(open.client.clone())
    }
}

/// Why a call to the bridge has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// There is no bridge to ask: not set up, not running, or the pick matches nothing.
    NotConnected(String),
    /// The connection went while the bridge was asked.
    Closed(String),
    /// The bridge answered with an error (a code from `daw_bridge::wire::code`).
    Bridge { code: String, message: String },
    /// The bridge didn't answer in time: the DAW is busy (a dialog, a render).
    Timeout,
    /// The bridge's answer passed the frame bound, and asking for less didn't help.
    TooBig,
}

/// Whether the DAW's bridge is a process of Kumi's own that the host starts (Logic's: nothing can be loaded into
/// Logic), rather than one the DAW loads and runs.
pub fn spawned_by_host(daw: Daw) -> bool {
    daw == Daw::Logic
}

/// A bridge the host started, or what a test runs in its place (a simulator).
pub trait SpawnedBridge: Send {
    /// How it ended (its exit status), once it has; None while it runs.
    fn exited(&mut self) -> Option<String>;
    /// Asks it to end, so it can remove its discovery file first.
    fn stop(&mut self);
    /// Ends it now.
    fn end(&mut self);
}

impl SpawnedBridge for tokio::process::Child {
    fn exited(&mut self) -> Option<String> {
        match self.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        }
    }

    fn stop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.id().and_then(|pid| i32::try_from(pid).ok()) {
            // SIGTERM: the bridge ends as it does when its DAW quits, removing its discovery file.
            let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
            return;
        }
        self.end();
    }

    fn end(&mut self) {
        let _ = self.start_kill();
    }
}

/// What a spawner answers: the bridge it started, or why it couldn't.
pub type SpawnFuture = Pin<Box<dyn Future<Output = Result<Box<dyn SpawnedBridge>, String>> + Send>>;
/// Starts a bridge for the host, given Kumi's folder (what the bridge gets as `--kumi-home`): a future, so a test can
/// start a simulator in its place.
pub type Spawner = Box<dyn Fn(PathBuf) -> SpawnFuture + Send + Sync>;
/// What the host asks again on a fresh connection when the old one was lost: the reads (and a preview, which writes
/// nothing). A write is never sent twice.
const RESENT: [&str; 7] = ["read", "snapshot", "focus", "focus_body", "status", "change_preview", "digest"];

/// The host for one DAW.
pub struct Host {
    daw: Daw,
    kumi_home: PathBuf,
    paths: BridgePaths,
    current: std::sync::Arc<tokio::sync::Mutex<Option<Current>>>,
    /// The bridge's events (`{name, data}`), for the MCP client: whichever instance the host is connected to.
    events: tokio::sync::broadcast::Sender<Value>,
    /// What starts the bridge of a DAW whose bridge the host starts, in place of the installed binary (a test's).
    spawner: Option<Spawner>,
    /// The bridge binary to start instead of the installed one ([`LOGIC_BRIDGE_VAR`]).
    bridge_binary: Option<PathBuf>,
    spawn_wait: Duration,
    /// The bridge the host started, while it runs it.
    spawned: tokio::sync::Mutex<Option<Box<dyn SpawnedBridge>>>,
    /// The changes previewed, applied and undone through this host (M3).
    journal: std::sync::Arc<crate::changes::Journal>,
    /// The projects' snapshots, compared on connect and while watching (M6).
    catchup: crate::catchup::Catchup,
    /// The listening jobs started through this host (M5).
    jobs: crate::listening::Jobs,
}

/// Which running instance a tool means, when several run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pick {
    pub pid: Option<u32>,
    pub resource: Option<String>,
}

impl Pick {
    pub fn from_arguments(arguments: &Value) -> Result<Self, String> {
        let pid = match arguments.get("pid") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_u64().and_then(|pid| u32::try_from(pid).ok()).ok_or("pid must be a process id")?),
        };
        let resource = match arguments.get("resource") {
            None | Some(Value::Null) => None,
            Some(Value::String(resource)) => Some(resource.clone()),
            Some(_) => return Err("resource must be a folder path".into()),
        };
        Ok(Self { pid, resource })
    }

    fn matches(&self, discovery: &Discovery) -> bool {
        self.pid.is_none_or(|pid| pid == discovery.pid) && self.resource.as_ref().is_none_or(|resource| *resource == discovery.resource)
    }
}

fn connect_error(daw: Daw, error: &ConnectError) -> String {
    match error {
        ConnectError::Handshake(daw_bridge::handshake::HandshakeError::Refused { reason, protocol: Some(protocol) })
            if reason == "protocol" =>
        {
            let protocol = daw_bridge::handshake::known_protocol(protocol);
            match daw {
                Daw::Logic => format!(
                    "Kumi's bridge for Logic Pro speaks {protocol}, another version than this Kumi's: set up Kumi's Logic Pro bridge again (kumi --daw logic bridge)"
                ),
                _ => format!("the bridge in {daw} speaks {protocol}, another version than this Kumi's: restart {daw} to load the installed one"),
            }
        }
        ConnectError::Handshake(daw_bridge::handshake::HandshakeError::Refused { reason, .. }) if reason == "auth" => match daw {
            Daw::Logic => {
                "Kumi's bridge for Logic Pro was set up with another secret: set up Kumi's Logic Pro bridge again (kumi --daw logic bridge)"
                    .to_string()
            }
            _ => format!("the bridge in {daw} was set up with another secret: restart {daw}, or set up Kumi's bridge again"),
        },
        ConnectError::Handshake(daw_bridge::handshake::HandshakeError::Refused { reason, .. }) if reason == "connections" => {
            let bridge = match daw {
                Daw::Logic => "Kumi's bridge for Logic Pro".to_string(),
                _ => format!("the bridge in {daw}"),
            };
            format!("{bridge} already has as many connections as it takes: close another Kumi or MCP app connected to it")
        }
        ConnectError::Handshake(daw_bridge::handshake::HandshakeError::BadProof) => {
            format!("whatever listens there isn't Kumi's bridge, or {daw} was set up with another secret")
        }
        other => other.to_string(),
    }
}

/// A connect that nothing answers: refused, or (Windows retries the SYN to a closed loopback port for about 2 s before
/// it refuses) not accepted in time. A running bridge accepts at once, even while its DAW is busy, so either means
/// the process is gone.
fn no_listener(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut)
}

impl Host {
    pub fn new(daw: Daw, kumi_home: &Path) -> Self {
        Self {
            daw,
            kumi_home: kumi_home.to_path_buf(),
            paths: BridgePaths::new(kumi_home, daw),
            current: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            events: tokio::sync::broadcast::channel(1024).0,
            spawner: None,
            bridge_binary: None,
            spawn_wait: SPAWN_WAIT,
            spawned: tokio::sync::Mutex::new(None),
            journal: std::sync::Arc::new(crate::changes::Journal::for_daw(daw)),
            catchup: crate::catchup::Catchup::new(BridgePaths::new(kumi_home, daw).snapshots()),
            jobs: crate::listening::Jobs::default(),
        }
    }

    /// Starts the bridge through `spawner` instead of the installed binary (a test's simulator).
    pub fn with_spawner(mut self, spawner: Spawner) -> Self {
        self.spawner = Some(spawner);
        self
    }

    /// Starts this binary instead of the installed one ([`LOGIC_BRIDGE_VAR`]).
    pub fn with_bridge_binary(mut self, binary: PathBuf) -> Self {
        self.bridge_binary = Some(binary);
        self
    }

    /// How long a started bridge gets to write its discovery file ([`SPAWN_WAIT`] unless a test shortens it).
    pub fn with_spawn_wait(mut self, wait: Duration) -> Self {
        self.spawn_wait = wait;
        self
    }

    pub(crate) fn catchup(&self) -> &crate::catchup::Catchup {
        &self.catchup
    }

    /// The host's connection to one instance, followed by work beside the calls (watch_me's looks).
    pub(crate) fn following(&self, instance: Pick) -> Following {
        Following { current: self.current.clone(), instance }
    }

    pub(crate) fn journal(&self) -> &crate::changes::Journal {
        &self.journal
    }

    pub(crate) fn jobs(&self) -> &crate::listening::Jobs {
        &self.jobs
    }

    pub(crate) fn paths(&self) -> &BridgePaths {
        &self.paths
    }

    /// The bridge's events from now on, as `{name, data}` (a `tracks` shift, a project opened…).
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub fn daw(&self) -> Daw {
        self.daw
    }

    /// The running instances (their pids alive), newest first, and how many stale files were removed.
    fn instances(&self) -> (Vec<Discovery>, usize, Vec<String>) {
        let mut stale = 0;
        let mut invalid = vec![];
        let mut alive = vec![];
        for found in scan_discovery(&self.paths.discovery(), self.daw) {
            match (found.discovery, found.alive) {
                (_, false) => {
                    if std::fs::remove_file(&found.path).is_ok() {
                        stale += 1;
                    }
                }
                (Ok(discovery), true) => alive.push(discovery),
                (Err(why), true) => invalid.push(format!("{}: {why}", found.path.file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        (alive, stale, invalid)
    }

    fn secret(&self) -> Result<daw_bridge::auth::Secret, String> {
        read_secret(&self.paths.secret()).map_err(|error| match error {
            FileError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                format!("Kumi's bridge for {} isn't set up yet (no secret)", self.daw)
            }
            FileError::NotOwnerOnly => format!("the secret for Kumi's {} bridge is readable by other accounts", self.daw),
            other => format!("the secret for Kumi's {} bridge can't be read: {other}", self.daw),
        })
    }

    /// Why nothing can be asked: no instance runs. Logic's bridge is the host's to start, so what is missing is Logic.
    fn none_running(&self) -> String {
        match self.daw {
            Daw::Logic => "no Logic Pro with Kumi's bridge is running: open Logic Pro".into(),
            daw => format!("no {daw} with Kumi's bridge is running"),
        }
    }

    /// Connects to the instance a discovery file names (Logic's names Logic's pid and its bundle id as the resource). A
    /// file whose port nobody answers is stale when it names no pid (Renoise on Windows), or when its bridge is one the
    /// host starts (Logic's: it ended with the session that started it, or crashed, while Logic runs on): it's removed,
    /// and the result is None.
    async fn connect(&self, discovery: &Discovery, secret: &daw_bridge::auth::Secret) -> Option<Result<BridgeClient, String>> {
        let address = SocketAddr::from(([127, 0, 0, 1], discovery.port));
        let client = match BridgeClient::connect_within(address, secret.clone(), CLIENT_NAME, CONNECT_TIMEOUT).await {
            Ok(client) => client,
            Err(ConnectError::Io(error)) if (discovery.pid == 0 || spawned_by_host(self.daw)) && no_listener(&error) => {
                let _ = std::fs::remove_file(self.paths.discovery().join(discovery.file_name()));
                return None;
            }
            Err(error) => return Some(Err(connect_error(self.daw, &error))),
        };
        let identity = &client.welcome().identity;
        if identity.daw != self.daw.id() || identity.pid != discovery.pid || identity.resource != discovery.resource {
            client.close().await;
            let daw = daw_bridge::handshake::tidy(&identity.daw, 16);
            return Some(Err(format!("port {} answered as another instance ({daw} pid {})", discovery.port, identity.pid)));
        }
        Some(Ok(client))
    }

    /// `discover`: every running instance's bridge, connected and asked for its status.
    pub async fn discover(&self) -> Value {
        let (alive, mut stale, invalid) = self.instances();
        let secret = self.secret();
        let mut instances = vec![];
        for discovery in &alive {
            let mut row = json!({
                "pid": discovery.pid,
                "port": discovery.port,
                "resource": discovery.resource,
                "bridge": discovery.version,
                "dawVersion": discovery.daw_version,
                "project": discovery.project,
            });
            match &secret {
                Err(why) => {
                    row["connected"] = false.into();
                    row["error"] = why.clone().into();
                }
                Ok(secret) => match self.connect(discovery, secret).await {
                    None => {
                        stale += 1;
                        continue;
                    }
                    Some(Err(why)) => {
                        row["connected"] = false.into();
                        row["error"] = why.into();
                    }
                    Some(Ok(client)) => {
                        row["connected"] = true.into();
                        row["bridge"] = client.welcome().identity.bridge.clone().into();
                        match client.request("status", json!({}), STATUS_TIMEOUT).await {
                            Ok(status) => {
                                row["project"] = status["project"].clone();
                                row["busy"] = status["busy"].clone();
                                for key in ["demo", "listening"] {
                                    if let Some(value) = status.get(key) {
                                        row[key] = value.clone();
                                    }
                                }
                                // What Logic's bridge lacks (`accessibility`), or null.
                                if let Some(permission) = status.get("permission") {
                                    row["permission"] = permission.clone();
                                }
                            }
                            Err(error) => row["busy"] = busy_word(&error).into(),
                        }
                        client.close().await;
                    }
                },
            }
            instances.push(row);
        }
        let mut result = json!({"daw": self.daw.id(), "setUp": secret.is_ok(), "instances": instances});
        if stale > 0 {
            result["staleRemoved"] = stale.into();
        }
        if !invalid.is_empty() {
            result["invalid"] = invalid.into();
        }
        if instances.is_empty() {
            result["hint"] = match &secret {
                Err(why) => why.clone(),
                Ok(_) => match self.daw {
                    Daw::Logic => {
                        "no Logic Pro with Kumi's bridge is running: open Logic Pro (Kumi starts its bridge when a session begins)"
                            .to_string()
                    }
                    daw => format!(
                        "no {daw} with Kumi's bridge is running: open {daw}, or restart it if it was open before Kumi set up its bridge"
                    ),
                },
            }
            .into();
        }
        result
    }

    /// Makes `current` the chosen instance's connection: the one already connected if it fits the pick, else the only
    /// running one, the one the pick names, or the newest. A DAW whose bridge the host starts (Logic) has it started
    /// when none runs. Returns how many instances run, or why none can be used.
    async fn ensure_connected(&self, current: &mut Option<Current>, pick: &Pick) -> Result<usize, String> {
        let reusable = current.as_ref().is_some_and(|open| {
            !open.client.is_closed()
                && pick.pid.is_none_or(|pid| pid == open.pid)
                && pick.resource.as_ref().is_none_or(|r| *r == open.resource)
        });
        let (mut alive, _, _) = self.instances();
        if !reusable {
            let secret = self.secret()?;
            if alive.is_empty() && spawned_by_host(self.daw) {
                alive = self.start_bridge().await?;
            }
            // (a pick that matches nothing leaves the connection there is: the calls without a pick go on using it)
            let mut discovery = self.pick_from(&alive, pick)?;
            if let Some(open) = current.take() {
                open.client.close().await;
            }
            let mut connected = self.connect(&discovery, &secret).await;
            if connected.is_none() && spawned_by_host(self.daw) {
                // The file was a gone bridge's (ended with the session that started it, or crashed), removed just now:
                // the bridge is started again, once.
                alive = self.start_bridge().await?;
                discovery = self.pick_from(&alive, pick)?;
                connected = self.connect(&discovery, &secret).await;
            }
            match connected {
                Some(Ok(client)) => {
                    self.forward_events(&client);
                    *current = Some(Current { pid: discovery.pid, resource: discovery.resource.clone(), port: discovery.port, client });
                }
                Some(Err(why)) => return Err(why),
                None => return Err(self.none_running()),
            }
        }
        Ok(alive.len())
    }

    /// The instance the pick means among those running, or why none does.
    fn pick_from(&self, alive: &[Discovery], pick: &Pick) -> Result<Discovery, String> {
        alive.iter().find(|discovery| pick.matches(discovery)).cloned().ok_or_else(|| {
            if alive.is_empty() {
                self.none_running()
            } else {
                format!("no running {} matches that pid or folder", self.daw)
            }
        })
    }

    /// Starts the DAW's bridge (one the host starts: Logic's), any it started before stopped first, and waits for its
    /// discovery file: the running instances once there is one. A bridge that ends first, or hasn't listened within
    /// the wait, is said, with what to do.
    async fn start_bridge(&self) -> Result<Vec<Discovery>, String> {
        let mut spawned = self.spawned.lock().await;
        if let Some(earlier) = spawned.take() {
            stop_bridge(earlier).await;
        }
        let mut bridge = match &self.spawner {
            Some(spawner) => spawner(self.kumi_home.clone()).await?,
            None => start_bridge_process(self.daw, &self.kumi_home, self.bridge_binary.as_deref()).await?,
        };
        let deadline = tokio::time::Instant::now() + self.spawn_wait;
        loop {
            let (alive, _, _) = self.instances();
            if !alive.is_empty() {
                *spawned = Some(bridge);
                return Ok(alive);
            }
            if let Some(how) = bridge.exited() {
                return Err(format!("Kumi's {0} bridge ended before it served ({how}): open {0}, then ask again", self.daw));
            }
            if tokio::time::Instant::now() >= deadline {
                stop_bridge(bridge).await;
                return Err(format!(
                    "Kumi's {0} bridge didn't start serving within {1} s: open {0}, then ask again",
                    self.daw,
                    self.spawn_wait.as_secs_f64()
                ));
            }
            tokio::time::sleep(SPAWN_POLL).await;
        }
    }

    /// The bridge's events go on to the MCP client while this connection lasts.
    fn forward_events(&self, client: &BridgeClient) {
        tokio::spawn(pass_on_events(client.events(), self.events.clone(), self.journal.clone()));
    }

    /// A connection that is silent and answers no ping is dead, or its DAW is frozen (a plug-in loading, a long script):
    /// from here the two look the same, and a frozen DAW takes a new connection no better. So the instance is asked afresh
    /// first. If it answers, the old connection goes and the new one is kept; if not, nothing is dropped (the DAW is busy).
    async fn fresh_connection(&self, silent: &BridgeClient, pid: u32, resource: &str) -> Option<BridgeClient> {
        let secret = self.secret().ok()?;
        let (alive, _, _) = self.instances();
        let discovery = alive.iter().find(|discovery| discovery.pid == pid && discovery.resource == resource)?;
        let Some(Ok(client)) = self.connect(discovery, &secret).await else { return None };
        let mut current = self.current.lock().await;
        match current.as_ref() {
            // Someone else replaced it while this one connected: use theirs.
            Some(open) if !open.client.same_connection(silent) => {
                let theirs = open.client.clone();
                drop(current);
                client.close().await;
                Some(theirs)
            }
            _ => {
                if let Some(old) = current.take() {
                    old.client.close().await;
                }
                self.forward_events(&client);
                *current = Some(Current { pid, resource: resource.to_string(), port: discovery.port, client: client.clone() });
                Some(client)
            }
        }
    }

    /// `status`: the chosen instance's bridge (the one already connected, else the only one or the newest).
    pub async fn status(&self, pick: &Pick) -> Value {
        let mut current = self.current.lock().await;
        let alive = match self.ensure_connected(&mut current, pick).await {
            Ok(alive) => alive,
            Err(why) => return self.not_connected(why),
        };
        let open = current.as_ref().expect("connected above");
        let identity = &open.client.welcome().identity;
        let mut result = json!({
            "connected": true,
            "daw": self.daw.id(),
            "bridge": identity.bridge,
            "dawVersion": identity.daw_version,
            "pid": open.pid,
            "port": open.port,
            "resource": open.resource,
            "project": identity.project,
            "busy": false,
            "instances": alive.max(1),
        });
        match open.client.request("status", json!({}), STATUS_TIMEOUT).await {
            Ok(status) => {
                result["project"] = status["project"].clone();
                result["busy"] = status.get("busy").cloned().unwrap_or(Value::Bool(false));
                // (Renoise's demo; what listening each bridge can do now, M5)
                for key in ["demo", "listening"] {
                    if let Some(value) = status.get(key) {
                        result[key] = value.clone();
                    }
                }
                // What Logic's bridge lacks (`accessibility`), or null.
                if let Some(permission) = status.get("permission") {
                    result["permission"] = permission.clone();
                }
            }
            Err(RequestError::Closed) => {
                current.take();
                return self.not_connected(format!("{} closed the connection", self.daw));
            }
            Err(error) => result["busy"] = busy_word(&error).into(),
        }
        result
    }

    /// The instance a pick means now (connected to, if it wasn't), as a pick of that one alone: what a change is previewed
    /// in, so its apply and undo go there and never to another one.
    pub(crate) async fn pin(&self, pick: &Pick) -> Result<Pick, CallError> {
        let mut current = self.current.lock().await;
        self.ensure_connected(&mut current, pick).await.map_err(CallError::NotConnected)?;
        let open = current.as_ref().expect("connected above");
        Ok(Pick { pid: Some(open.pid), resource: Some(open.resource.clone()) })
    }

    /// The refusal for a change whose instance isn't running any more (another one may be: the change isn't made there).
    pub(crate) fn elsewhere(&self, instance: &Pick) -> Option<Value> {
        let pid = instance.pid?;
        if self.instances().0.iter().any(|discovery| instance.matches(discovery)) {
            return None;
        }
        let folder = instance.resource.as_deref().map(|resource| format!(", {resource}")).unwrap_or_default();
        Some(json!({
            "error": format!("this change was made in another {} (pid {pid}{folder}), which isn't running now: preview it again", self.daw),
            "code": "stale",
        }))
    }

    /// Asks the chosen instance's bridge for one of its answers (`read`, `snapshot`, `focus`, `focus_body`). The
    /// connection is made if there isn't one, and isn't held while the bridge thinks: other calls go ahead beside it.
    pub async fn call(&self, method: &str, params: Value, pick: &Pick, timeout: Duration) -> Result<Value, CallError> {
        let (client, pid, resource) = {
            let mut current = self.current.lock().await;
            self.ensure_connected(&mut current, pick).await.map_err(CallError::NotConnected)?;
            let open = current.as_ref().expect("connected above");
            (open.client.clone(), open.pid, open.resource.clone())
        };
        match client.request(method, params.clone(), timeout).await {
            Ok(answer) => Ok(answer),
            Err(RequestError::Bridge { code, message }) => Err(CallError::Bridge { code, message }),
            Err(RequestError::Timeout) => {
                // A DAW behind a dialog is silent but still answers a ping (a socket callback needs no timer); a connection the
                // bridge has let go of, or lost, answers nothing for ever, and so does a frozen DAW. A new connection tells these
                // apart: if the instance answers it, the old one was lost and is replaced (and the call is asked again on the
                // new one); if not, the DAW is busy and the old connection is kept, as it was.
                if client.busy() && client.request("ping", json!({}), PING_PROBE).await == Err(RequestError::Timeout) {
                    let Some(fresh) = self.fresh_connection(&client, pid, &resource).await else { return Err(CallError::Timeout) };
                    return self.again(&fresh, method, params, timeout).await;
                }
                Err(CallError::Timeout)
            }
            Err(RequestError::Closed) => {
                let mut current = self.current.lock().await;
                // The host let this connection go for a fresh one to the same instance while the call waited on it (another
                // call's timeout, above): the DAW is still there, and the call is never told it closed.
                let replaced =
                    current.as_ref().filter(|open| open.pid == pid && !open.client.same_connection(&client) && !open.client.is_closed());
                if let Some(fresh) = replaced.map(|open| open.client.clone()) {
                    drop(current);
                    return self.again(&fresh, method, params, timeout).await;
                }
                if current.as_ref().is_some_and(|open| open.pid == pid && open.client.is_closed()) {
                    current.take();
                }
                Err(CallError::Closed(format!("{} closed the connection", self.daw)))
            }
        }
    }

    /// A call whose connection was let go of for a fresh one. Only a read is asked again: a write the old connection carried
    /// may have been made, and making it twice (or refusing it as changed) would be wrong; its caller is told it timed out
    /// (uncertain).
    async fn again(&self, fresh: &BridgeClient, method: &str, params: Value, timeout: Duration) -> Result<Value, CallError> {
        if !RESENT.contains(&method) {
            return Err(CallError::Timeout);
        }
        match fresh.request(method, params, timeout).await {
            Ok(answer) => Ok(answer),
            Err(RequestError::Bridge { code, message }) => Err(CallError::Bridge { code, message }),
            Err(RequestError::Timeout) => Err(CallError::Timeout),
            Err(RequestError::Closed) => Err(CallError::Closed(format!("{} closed the connection", self.daw))),
        }
    }

    /// The JSON a tool answers when a call has no result.
    pub fn call_error(&self, error: &CallError) -> Value {
        let daw = self.daw;
        match error {
            CallError::NotConnected(reason) => json!({"error": reason, "connected": false, "daw": daw.id()}),
            CallError::Closed(reason) => json!({"error": reason, "connected": false, "daw": daw.id()}),
            CallError::Bridge { code, message } => {
                let mut answer = json!({"error": message, "code": code, "connected": true, "daw": daw.id()});
                if code == daw_bridge::wire::code::BUSY {
                    answer["busy"] = true.into();
                }
                answer
            }
            CallError::Timeout => json!({
                "error": format!("{daw} didn't answer in time (a dialog may be open)"),
                "busy": true,
                "connected": true,
                "daw": daw.id(),
            }),
            CallError::TooBig => json!({
                "error": format!("{daw} answered more than {} KiB even for a smaller page", MAX_ANSWER_BYTES / 1024),
                "connected": true,
                "daw": daw.id(),
            }),
        }
    }

    fn not_connected(&self, reason: String) -> Value {
        json!({"connected": false, "daw": self.daw.id(), "reason": reason})
    }

    /// Closes the connection, and stops the bridge the host started, if it started one.
    pub async fn close(&self) {
        // A render or a capture still running is stopped, and what it changed put back, before the connection goes.
        self.cancel_running_jobs().await;
        if let Some(open) = self.current.lock().await.take() {
            open.client.close().await;
        }
        if let Some(bridge) = self.spawned.lock().await.take() {
            stop_bridge(bridge).await;
        }
    }
}

/// Starts the installed bridge binary (or `binary`, given instead of it) with `--kumi-home`: its stdin and stdout
/// closed (the host's own stdout is the MCP channel), its stderr the host's, and killed when the host lets go of it.
async fn start_bridge_process(daw: Daw, kumi_home: &Path, binary: Option<&Path>) -> Result<Box<dyn SpawnedBridge>, String> {
    let binary = match binary {
        Some(binary) if binary.is_file() => binary.to_path_buf(),
        Some(binary) => return Err(format!("{LOGIC_BRIDGE_VAR} names {}, which isn't there", binary.display())),
        None => installed_bridge_binary(daw, kumi_home)?,
    };
    let child = tokio::process::Command::new(&binary)
        .arg("--kumi-home")
        .arg(kumi_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Kumi's {daw} bridge at {} couldn't start: {error}", binary.display()))?;
    Ok(Box::new(child))
}

/// The bridge binary the lifecycle installed, from its receipt: Logic's `logic-kumi` inside its folder.
fn installed_bridge_binary(daw: Daw, kumi_home: &Path) -> Result<PathBuf, String> {
    let target = match daw {
        Daw::Logic => logic_target(&BridgePaths::new(kumi_home, daw)),
        other => return Err(format!("{other}'s bridge isn't one Kumi starts")),
    };
    let setup = format!("kumi --daw {} bridge", daw.id());
    let receipt = Lifecycle::new(kumi_home, target)
        .receipt()
        .map_err(|error| format!("Kumi's {daw} bridge receipt can't be read ({error}): run `{setup}` to set it up again"))?
        .filter(|receipt| receipt.status == "installed")
        .ok_or_else(|| format!("Kumi's {daw} bridge isn't installed: run `{setup}`"))?;
    let binary = Path::new(&receipt.target).join(LOGIC_BINARY);
    if !binary.is_file() {
        return Err(format!("Kumi's {daw} bridge is missing from {}: run `{setup}` to set it up again", receipt.target));
    }
    Ok(binary)
}

/// Stops a bridge the host started: asked to end, so it removes its discovery file; killed if it hasn't within a second.
async fn stop_bridge(mut bridge: Box<dyn SpawnedBridge>) {
    bridge.stop();
    let deadline = tokio::time::Instant::now() + STOP_WAIT;
    while bridge.exited().is_none() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if bridge.exited().is_none() {
        bridge.end();
    }
}

/// The bridge's events, on to the MCP client as `{name, data}`. Events dropped on the way (the host fell behind) are
/// said, never lost silently: a track shift among them would leave positional refs one off, so Kumi reads them again.
pub async fn pass_on_events(
    mut from: tokio::sync::broadcast::Receiver<daw_bridge::wire::Event>,
    to: tokio::sync::broadcast::Sender<Value>,
    journal: std::sync::Arc<crate::changes::Journal>,
) {
    loop {
        match from.recv().await {
            Ok(event) => {
                // (the DAW's own undo of a Kumi change names the transactions it took back)
                let data = crate::changes::undone_in_daw(&journal, &event.name, event.data);
                let _ = to.send(json!({"name": event.name, "data": data}));
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                let _ = to.send(json!({"name": "events_lost", "data": {}}));
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn busy_word(error: &RequestError) -> String {
    match error {
        RequestError::Bridge { code, message } if code == daw_bridge::wire::code::BUSY => message.clone(),
        RequestError::Timeout => "not answering (a dialog may be open)".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[tokio::test]
    async fn events_dropped_on_the_way_are_said_not_lost_silently() {
        let (bridge, from) = tokio::sync::broadcast::channel(2);
        let (to, mut client) = tokio::sync::broadcast::channel(16);
        for n in 0..5 {
            bridge.send(daw_bridge::wire::Event { seq: n + 1, name: "tracks".into(), data: json!({"layout": n}) }).unwrap();
        }
        drop(bridge);
        pass_on_events(from, to, Default::default()).await;
        let mut names = vec![];
        while let Ok(event) = client.try_recv() {
            names.push(event["name"].as_str().unwrap().to_string());
        }
        assert_eq!(names, ["events_lost", "tracks", "tracks"], "the three dropped are said; the last two come through");
    }

    #[test]
    fn a_refused_or_unanswered_connect_means_nothing_listens() {
        assert!(no_listener(&Error::from(ErrorKind::ConnectionRefused)));
        assert!(no_listener(&Error::from(ErrorKind::TimedOut)));
        assert!(!no_listener(&Error::from(ErrorKind::ConnectionReset)));
        assert!(!no_listener(&Error::from(ErrorKind::PermissionDenied)));
    }

    #[test]
    fn only_logics_bridge_is_the_hosts_to_start() {
        assert!(spawned_by_host(Daw::Logic) && !spawned_by_host(Daw::Reaper) && !spawned_by_host(Daw::Renoise));
    }

    #[test]
    fn the_installed_bridge_binary_comes_from_the_receipt() {
        let home = tempfile::tempdir().unwrap();
        let missing = installed_bridge_binary(Daw::Logic, home.path()).unwrap_err();
        assert_eq!(missing, "Kumi's Logic Pro bridge isn't installed: run `kumi --daw logic bridge`");
        assert!(installed_bridge_binary(Daw::Reaper, home.path()).is_err());
        // Installed through the lifecycle: the binary inside the folder.
        let target = logic_target(&BridgePaths::new(home.path(), Daw::Logic));
        let source = home.path().join("built").join("logic-kumi");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("logic-kumi"), b"#!/bin/sh\n").unwrap();
        std::fs::write(source.join("version"), "0.1.0\n").unwrap();
        Lifecycle::new(home.path(), target.clone()).install(&source, "0.1.0").unwrap();
        assert_eq!(installed_bridge_binary(Daw::Logic, home.path()).unwrap(), target.path().join("logic-kumi"));
        // Removed behind Kumi's back: said, with the command that puts it back.
        std::fs::remove_file(target.path().join("logic-kumi")).unwrap();
        let gone = installed_bridge_binary(Daw::Logic, home.path()).unwrap_err();
        assert!(gone.contains("missing from") && gone.contains("kumi --daw logic bridge"), "{gone}");
    }
}
