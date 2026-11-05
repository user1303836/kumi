//! `kumi serve` connected to a DAW: REAPER simulated behind Kumi's host, called in process (as the app's own session
//! tests do), driven through the runtime's operations: connect, observe, and the project's key and epoch.

use async_trait::async_trait;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    reaper::simulator::ReaperScript,
    simulator::{Simulator, SimulatorConfig},
};
use futures::FutureExt;
use kumi::serve::ctx::{Call, Ctx, Host as HostCall, OpError, Progress};
use kumi::serve::{registry, Args};
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_common::abort::Signal;
use kumi_daw_ableton::app::AbletonFactory;
use kumi_daw_own::bridge::{HostStart, OwnOptions};
use kumi_daw_own::provider::OwnHosts;
use kumi_types::contracts::JsonObject;
use kumi_types::system::Env;
use kumi_types::{daw::DawId, errors::RuntimeError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::{cell::Cell, rc::Rc, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Kumi's host, called in process.
struct InProcess {
    host: Arc<Host>,
    id: Cell<u64>,
}

#[async_trait(?Send)]
impl McpEndpoint for InProcess {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools": daw_mcp_server::mcp::tools(&self.host)})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.id.set(self.id.get() + 1);
        let message = json!({"jsonrpc": "2.0", "id": self.id.get(), "method": "tools/call", "params": {"name": name, "arguments": args}});
        let answer = daw_mcp_server::mcp::handle(&self.host, &message).await.expect("an answer");
        Ok(serde_json::from_value(answer["result"].clone()).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.host.close().await;
        Ok(())
    }
}

/// A runtime for `bridge`'s DAW with Kumi's part in it (as its receipt says, when `config` gives a simulator) and that
/// simulator answering as the DAW behind Kumi's host. Its transport home is the plugin's own, beside its state folder,
/// or (`adopted`) one elsewhere, as setup's adoption gives it.
struct Lab {
    home: tempfile::TempDir,
    _elsewhere: tempfile::TempDir,
    ctx: Rc<Ctx>,
    simulator: Option<Rc<Simulator>>,
}

impl Lab {
    async fn new(daw: DawId, bridge: Daw, config: Option<Box<dyn FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig>>) -> Lab {
        Lab::with(daw, bridge, config, false).await
    }

    async fn with(
        daw: DawId,
        bridge: Daw,
        config: Option<Box<dyn FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig>>,
        adopted: bool,
    ) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let transport = if adopted { elsewhere.path().join(".kumi") } else { home.path().join("kumi-home") };
        let paths = BridgePaths::new(&transport, bridge);
        let simulator = match config {
            Some(config) => {
                let secret = ensure_secret(&paths.secret()).unwrap();
                let mut config = config(secret);
                config.discovery_dir = Some(paths.discovery());
                let receipt = daw_bridge::lifecycle::Receipt {
                    schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
                    daw: bridge.id().into(),
                    status: "installed".into(),
                    target: home.path().join("bridge").display().to_string(),
                    reference: home.path().join("bridge-reference.json").display().to_string(),
                    generation: Some("g1".into()),
                    version: Some("1.0.0".into()),
                    sha256: None,
                    previous: vec![],
                    copies: vec![],
                    updated_ms: 0,
                    history: vec![],
                };
                std::fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
                Some(Rc::new(Simulator::start(config).await.unwrap()))
            }
            None => None,
        };
        let env = Env::from([
            ("HOME".into(), home.path().display().to_string()),
            ("KUMI_HOME".into(), transport.display().to_string()),
            ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
            ("KUMI_PROCESSES".into(), "none".into()),
            ("KUMI_REAPER_RESOURCE_DIR".into(), home.path().join("REAPER").display().to_string()),
            ("KUMI_RENOISE_PREFS_DIR".into(), home.path().join("Renoise").display().to_string()),
        ]);
        // (made as the integration starts it: its ask to keep the project's id is the host's, as KUMI_KEEP_IDENTITY is)
        let host_home = transport.clone();
        let own_host = OwnHosts(Rc::new(move |_| {
            let host_home = host_home.clone();
            Rc::new(move |start: HostStart| {
                let host = Host::new(bridge, &host_home, daw_mcp_server::host::ListeningMode::Restricted).with_keep_identity(start.keep_identity);
                let endpoint = Rc::new(InProcess { host: Arc::new(host), id: Cell::new(0) });
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            })
        }));
        let answers = kumi_daw::io::Answers::default()
            .with(AbletonFactory(Rc::new(|_| panic!("a REAPER runtime makes no Ableton integration"))))
            .with(own_host);
        let args = Args {
            state_dir: home.path().join("state"),
            kumi_home: transport,
            hermes_home: None,
            hermes_root: None,
            daw,
            library_roots: vec![],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        let ctx = Ctx::new(args, env, answers).unwrap();
        Lab { home, _elsewhere: elsewhere, ctx, simulator }
    }

    /// The methods Kumi's host sent the DAW, each once.
    fn methods_sent(&self) -> Vec<String> {
        let mut methods: Vec<String> = self
            .simulator
            .as_ref()
            .map(|simulator| simulator.transcript())
            .unwrap_or_default()
            .into_iter()
            .filter(|line| line.from_host)
            .filter_map(|line| serde_json::from_str::<Value>(&line.text).ok()?.get("method")?.as_str().map(str::to_string))
            .collect();
        methods.sort();
        methods.dedup();
        methods
    }

    /// The runtime's operation `name` with these arguments.
    async fn call(&self, name: &str, args: Value) -> Result<Value, OpError> {
        let operation = registry::OPERATIONS.iter().find(|operation| operation.name == name).unwrap();
        let Value::Object(args) = args else { panic!("arguments are an object") };
        let call = Call { args, host: HostCall::default(), signal: Signal::new(), progress: Progress::none() };
        (operation.run)(&self.ctx, call).await
    }

    fn state(&self, file: &str) -> Value {
        self.ctx.snapshots().flush();
        serde_json::from_slice(&std::fs::read(self.home.path().join("state").join(file)).unwrap()).unwrap()
    }
}

macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

case!(serve_connects_to_reaper_and_gives_each_project_its_key_and_epoch, async {
    let script = ReaperScript::new(3, 0);
    let tab = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| script.config(secret, "/lab/REAPER")))).await;
    let before = lab.call("status", json!({})).await.unwrap();
    assert_eq!((before["connected"].clone(), before["project"].clone()), (json!(false), Value::Null));
    let Err(OpError::NotConnected { .. }) = lab.call("observe", json!({"scope": "project"})).await else {
        panic!("observe before connect")
    };

    let connected = lab.call("connect", json!({"daw": "reaper"})).await.unwrap();
    assert_eq!(connected["connected"], true, "{connected}");
    let first = connected["project"].clone();
    let key = first["key"].as_str().unwrap().to_string();
    let epoch = first["epoch"].as_u64().unwrap();
    assert!(key.starts_with("p-") && epoch > 1_000_000_000_000, "an epoch no earlier runtime gave out: {first}");

    let seen = lab.call("observe", json!({"scope": "project"})).await.unwrap();
    assert_eq!(
        (seen["project_key"].as_str(), seen["epoch"].as_u64()),
        (Some(key.as_str()), Some(epoch)),
        "the same project, the same epoch"
    );
    assert!(!seen["text"].as_str().unwrap().is_empty());
    let refs = seen["refs"].as_array().unwrap();
    assert!(refs.iter().filter(|candidate| candidate["kind"] == "track").count() >= 3, "{refs:?}");
    assert!(refs.iter().all(|candidate| candidate["reference"]["kind"] == "daw"), "only the project's things: {refs:?}");
    let focus = lab.call("observe", json!({"scope": "focus", "max_chars": 200})).await.unwrap();
    assert!(focus["text"].as_str().unwrap().contains(&format!("(epoch {epoch})")), "{focus}");
    assert!(lab.ctx.link().expect(&key, epoch).is_ok());

    // Another project in REAPER's tab: another key and the next epoch; a plan made on the first is stale.
    tab.switch_tab();
    let mut other = Value::Null;
    for _ in 0..200 {
        other = lab.call("observe", json!({"scope": "project", "max_chars": 200})).await.unwrap();
        if other["project_key"] != key {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_ne!(other["project_key"], key, "{other}");
    assert_eq!(other["epoch"].as_u64(), Some(epoch + 1));
    let Err(OpError::Refused { code, .. }) = lab.ctx.link().expect(&key, epoch) else { panic!("a plan made on the first project") };
    assert_eq!(code, "stale");
    let Err(OpError::Refused { code, .. }) = lab.ctx.link().expect(other["project_key"].as_str().unwrap(), epoch) else {
        panic!("a plan made at an earlier epoch")
    };
    assert_eq!(code, "stale");

    let focus_file = lab.state("focus.json");
    assert_eq!(
        (focus_file["project"]["key"].clone(), focus_file["project"]["epoch"].as_u64()),
        (other["project_key"].clone(), Some(epoch + 1))
    );
    let status = lab.state("status.json");
    assert_eq!((status["connected"].clone(), status["project"]["epoch"].as_u64()), (json!(true), Some(epoch + 1)));
    let Err(OpError::Refused { code, .. }) = lab.call("connect", json!({"daw": "renoise"})).await else { panic!("another DAW") };
    assert_eq!(code, "other_daw");
    // REAPER goes away: status and focus say so at once, and no plan holds.
    lab.simulator.as_ref().unwrap().stop();
    let (mut gone, mut status) = (Value::Null, Value::Null);
    for _ in 0..1000 {
        (gone, status) = (lab.state("focus.json"), lab.state("status.json"));
        if gone["connected"] == false && status["connected"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        (gone["connected"].clone(), gone["project"].clone(), gone["focus"].clone()),
        (json!(false), Value::Null, Value::Null),
        "{gone}"
    );
    assert_eq!(status["connected"], false, "{status}");
    let Err(OpError::NotConnected { .. }) = lab.ctx.link().expect(other["project_key"].as_str().unwrap(), epoch + 1) else {
        panic!("a plan while REAPER is away")
    };
    // Nothing but reads reached REAPER: connecting and looking write nothing into the project.
    let reads = ["digest", "focus", "focus_body", "read", "snapshot", "status"];
    let sent = lab.methods_sent();
    assert!(!sent.is_empty() && sent.iter().all(|method| reads.contains(&method.as_str())), "{sent:?}");
    lab.ctx.link().close().await;
});

/// What each hello the simulated bridge was sent said of keeping the project's id (`keepIdentity`), in order.
fn identity_asks(simulator: &Simulator) -> Vec<Option<Value>> {
    simulator
        .transcript()
        .into_iter()
        .filter(|line| line.from_host)
        .filter_map(|line| serde_json::from_str::<Value>(&line.text).ok())
        .filter(|frame| frame["type"] == "hello")
        .map(|frame| frame.get("keepIdentity").cloned())
        .collect()
}

// The plugin's runtime only observes: its host never asks REAPER to keep the project's id, so nothing is written into the
// project (contract C2), while REAPER says it would keep one for a client that asked.
case!(serve_never_asks_reaper_to_keep_the_projects_id_and_nothing_is_written_into_it, async {
    let script = ReaperScript::new(3, 0);
    let written = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| script.config(secret, "/lab/REAPER")))).await;
    let connected = lab.call("connect", json!({"daw": "reaper"})).await.unwrap();
    assert_eq!(connected["connected"], true, "{connected}");
    lab.call("observe", json!({"scope": "project"})).await.unwrap();
    let simulator = lab.simulator.as_ref().unwrap();
    let asks = identity_asks(simulator);
    assert!(!asks.is_empty() && asks.iter().all(Option::is_none), "{asks:?}");
    assert!(!simulator.keeps_identity());
    assert_eq!(written.ids_written(), Vec::<String>::new(), "nothing written into the project");
    let status = lab.state("status.json");
    assert_eq!(status["connected"], true, "{status}");
    lab.ctx.link().close().await;
});

// Standalone Kumi (its integration with the standalone policy) asks, and REAPER writes the project's id into it while it
// is connected.
case!(serve_daw_standalone_kumi_asks_reaper_to_keep_the_projects_id_and_it_is_written, async {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Reaper);
    let script = ReaperScript::new(3, 0);
    let mut config = script.config(ensure_secret(&paths.secret()).unwrap(), "/lab/REAPER");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let host_home = home.path().to_path_buf();
    let connect: kumi_daw_own::bridge::Connect = Rc::new(move |start: HostStart| {
        let host = Host::new(Daw::Reaper, &host_home, daw_mcp_server::host::ListeningMode::Standalone).with_keep_identity(start.keep_identity);
        let endpoint = Rc::new(InProcess { host: Arc::new(host), id: Cell::new(0) });
        async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
    });
    let context = kumi_agent::core::context::IntegrationContext::new(Rc::new(|_, _| {}));
    assert!(context.policy.persist_identity, "standalone Kumi's policy");
    let reaper = kumi_daw_reaper::create_reaper_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
    reaper.start(Signal::new()).await.unwrap();
    reaper.observe(Signal::new(), None).await.unwrap();
    let asks = identity_asks(&simulator);
    assert!(!asks.is_empty() && asks.iter().all(|ask| *ask == Some(json!(true))), "{asks:?}");
    assert!(simulator.keeps_identity());
    assert_eq!(script.ids_written(), ["/lab/Song.rpp"], "the project's id written into it");
    reaper.close().await.unwrap();
});

case!(serve_wont_connect_before_kumis_part_is_in_the_daw, async {
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, None).await;
    let Err(OpError::Refused { code, reason, .. }) = lab.call("connect", json!({})).await else { panic!("connected with nothing set up") };
    assert_eq!(code, "not_set_up");
    assert!(reason.contains("REAPER"), "{reason}");
    assert_eq!(lab.call("status", json!({})).await.unwrap()["connected"], false);
});

case!(serve_wont_use_reaper_through_an_adopted_home_yet, async {
    let script = ReaperScript::new(3, 0);
    let lab = Lab::with(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| script.config(secret, "/lab/REAPER"))), true).await;
    let Err(OpError::Refused { code, .. }) = lab.call("connect", json!({})).await else { panic!("connected through an adopted home") };
    assert_eq!(code, "adopted_home");
    assert!(lab.methods_sent().is_empty(), "nothing reached REAPER");
});

/// Live with nothing in it but what a Kumi start reads.
struct Silent;

#[async_trait(?Send)]
impl kumi_agent::core::contracts::Integration for Silent {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn observe(
        &self,
        _: Signal,
        _: Option<kumi_agent::core::contracts::ObserveHints>,
    ) -> Result<kumi_agent::core::contracts::Observation, RuntimeError> {
        Err(RuntimeError::plain("Live isn't open"))
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// A runtime for Live whose Remote Script's reference names a configuration in `owner`'s home (the runtime's own when
/// None); and whether it started Live's integration.
async fn live_connect(owner: Option<&std::path::Path>) -> (Result<Value, OpError>, bool) {
    live_connect_with(owner, false).await
}

/// [`live_connect`], with the configuration where an earlier install keeps it (`legacy`): in a folder of its own, naming
/// the server in its owner's `bridge/<version>/package`.
async fn live_connect_with(owner: Option<&std::path::Path>, legacy: bool) -> (Result<Value, OpError>, bool) {
    let home = tempfile::tempdir().unwrap();
    let transport = home.path().join("kumi-home");
    let owner = owner.unwrap_or(&transport);
    let config = if legacy {
        let server = owner.join("bridge/1.0.104-1793822317831/package/ableton-mcp-server");
        std::fs::create_dir_all(server.parent().unwrap()).unwrap();
        std::fs::write(&server, "").unwrap();
        let config = home.path().join(".config/ableton-mcp-release-a8a291e/bridge-config.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let body = json!({"version": 2, "server": {"command": server, "args": ["--config", config]}, "bridge": {"host": "127.0.0.1", "port": 9765}});
        std::fs::write(&config, body.to_string()).unwrap();
        config
    } else {
        let config = owner.join("bridge/state/bridge-config.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        config
    };
    std::fs::create_dir_all(&transport).unwrap();
    let scripts = home.path().join("Remote Scripts");
    std::fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
    std::fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config": config}).to_string()).unwrap();
    let env = Env::from([
        ("HOME".into(), home.path().display().to_string()),
        ("KUMI_HOME".into(), transport.display().to_string()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts.display().to_string()),
        ("KUMI_PROCESSES".into(), "none".into()),
    ]);
    let started = Rc::new(Cell::new(false));
    let factory = {
        let started = started.clone();
        AbletonFactory(Rc::new(move |_| {
            started.set(true);
            Rc::new(Silent) as Rc<dyn kumi_agent::core::contracts::Integration>
        }))
    };
    let args = Args {
        state_dir: home.path().join("state"),
        kumi_home: transport,
        hermes_home: None,
        hermes_root: None,
        daw: DawId::Ableton,
        library_roots: vec![],
        media_roots: vec![],
        tools: BTreeMap::new(),
    };
    let ctx = Ctx::new(args, env, kumi_daw::io::Answers::default().with(factory)).unwrap();
    let connect = registry::OPERATIONS.iter().find(|operation| operation.name == "connect").unwrap();
    let call = Call { args: Default::default(), host: HostCall::default(), signal: Signal::new(), progress: Progress::none() };
    let connected = (connect.run)(&ctx, call).await;
    ctx.link().close().await;
    (connected, started.get())
}

case!(serve_wont_take_over_lives_bridge_when_another_kumi_installed_it, async {
    // The producer's own Kumi put the bridge in Live: the Remote Script's reference names a configuration in its home.
    let other = tempfile::tempdir().unwrap();
    let (connected, started) = live_connect(Some(other.path())).await;
    let Err(OpError::Refused { code, owner, .. }) = connected else { panic!("took over another Kumi's bridge") };
    assert_eq!(code, "owned_elsewhere");
    assert_eq!(owner.unwrap()["kumi_home"], std::fs::canonicalize(other.path()).unwrap().display().to_string());
    assert!(!started, "its bridge isn't started");
});

case!(serve_knows_the_home_an_earlier_live_install_belongs_to_by_the_server_it_starts, async {
    // The producer's own Kumi installed the bridge before state lived in its home: the configuration is in a folder of
    // its own, and names the server in that Kumi's bridge folder.
    let (connected, started) = live_connect_with(None, true).await;
    assert!(connected.is_ok() && started, "the runtime's own home's install: {connected:?}");
    let other = tempfile::tempdir().unwrap();
    let (connected, started) = live_connect_with(Some(other.path()), true).await;
    let Err(OpError::Refused { code, owner, .. }) = connected else { panic!("took over another Kumi's bridge") };
    assert_eq!((code, started), ("owned_elsewhere", false));
    assert_eq!(owner.unwrap()["kumi_home"], std::fs::canonicalize(other.path()).unwrap().display().to_string());
});

case!(serve_connects_to_live_through_the_bridge_its_own_home_installed, async {
    let (connected, started) = live_connect(None).await;
    let status = connected.expect("its own bridge");
    assert!(started, "Live's integration started");
    assert_eq!((status["daw"].clone(), status["project"].clone()), (json!("ableton"), Value::Null), "no look yet: Live isn't open");
});

/// The runtime served over MCP on in-memory pipes, as Hermes's host drives it: `_host` and all.
struct Wire {
    to: Option<tokio::io::DuplexStream>,
    from: BufReader<tokio::io::DuplexStream>,
    served: tokio::task::JoinHandle<std::io::Result<()>>,
    asked: u64,
}

impl Wire {
    async fn start(ctx: Rc<Ctx>) -> Wire {
        let (to, runtime_in) = tokio::io::duplex(1 << 20);
        let (runtime_out, from) = tokio::io::duplex(1 << 20);
        let served = tokio::task::spawn_local(kumi::serve::mcp::serve_with(
            ctx,
            registry::OPERATIONS,
            runtime_in,
            runtime_out,
            std::future::pending(),
        ));
        let mut wire = Wire { to: Some(to), from: BufReader::new(from), served, asked: 0 };
        wire.request("initialize", json!({"protocolVersion": "2025-06-18"})).await;
        wire.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        wire
    }

    async fn send(&mut self, message: Value) {
        let to = self.to.as_mut().unwrap();
        to.write_all(format!("{message}\n").as_bytes()).await.unwrap();
        to.flush().await.unwrap();
    }

    /// A request's answer; what comes out meanwhile (progress) is passed over.
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.asked += 1;
        let id = self.asked;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await;
        loop {
            let mut line = String::new();
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(30), self.from.read_line(&mut line)).await.expect("an answer in time");
            assert!(read.unwrap() > 0, "the runtime stopped before answering {method}");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message;
            }
        }
    }

    /// A tool call as Hermes's host makes it (with its tool call's id, when it has one): the result's structured
    /// content, and whether it's an error.
    async fn call(&mut self, name: &str, mut arguments: Value, tool_call_id: Option<&str>) -> (Value, bool) {
        if let Some(id) = tool_call_id {
            arguments["_host"] = json!({"tool_call_id": id});
        }
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments})).await;
        (answer["result"]["structuredContent"].clone(), answer["result"]["isError"] == true)
    }

    /// [`Wire::call`] by a host call of the chat `session`.
    async fn call_in(&mut self, name: &str, mut arguments: Value, tool_call_id: Option<&str>, session: &str) -> (Value, bool) {
        arguments["_host"] = json!({"tool_call_id": tool_call_id, "session_id": session});
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments})).await;
        (answer["result"]["structuredContent"].clone(), answer["result"]["isError"] == true)
    }

    async fn end(mut self) {
        drop(self.to.take());
        let mut rest = String::new();
        while self.from.read_line(&mut rest).await.unwrap_or(0) > 0 {}
        tokio::time::timeout(std::time::Duration::from_secs(30), self.served).await.unwrap().unwrap().unwrap();
    }
}

/// A connected runtime's first look: the project's key and epoch, and a track's ref and label.
async fn looked(wire: &mut Wire) -> (Value, u64, String, String) {
    let (connected, refused) = wire.call("connect", json!({}), None).await;
    assert!(!refused, "{connected}");
    let (seen, _) = wire.call("observe", json!({"scope": "project"}), None).await;
    let track = seen["refs"].as_array().unwrap().iter().find(|candidate| candidate["kind"] == "track").unwrap().clone();
    let reference = track["reference"]["node"]["ref"].as_str().unwrap().to_string();
    (seen["project_key"].clone(), seen["epoch"].as_u64().unwrap(), reference, track["label"].as_str().unwrap().to_string())
}

/// A plan as the host sends it: the epoch as its digits (CC-2).
fn plan(key: &Value, epoch: u64, steps: Value) -> Value {
    json!({"title": "Bring the track down", "steps": steps, "expected_project_key": key, "expected_epoch": epoch.to_string()})
}

case!(serve_changes_reaper_once_per_host_call_over_mcp_and_undoes_it, async {
    let script = ReaperScript::new(3, 0);
    let sim = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| sim.config(secret, "/lab/REAPER")))).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, reference, label) = looked(&mut wire).await;
    let volumes = || (0..4).map(|n| script.volume(n)).collect::<Vec<f64>>();
    let before = volumes();
    let down = json!([{"tool": "track_volume", "input": {"ref": reference, "volumeDb": -6}}]);

    // The approval's preview (CC-3), sent with the call's id as the hook does: it reads, and records nothing.
    let mut asked = plan(&key, epoch, down.clone());
    asked["preview"] = json!(true);
    let (preview, _) = wire.call("change", asked, Some("tc-1")).await;
    assert_eq!((preview["state"].clone(), preview["steps"][0]["family"].clone()), (json!("preview"), json!("mixer")), "{preview}");
    let text = preview["steps"][0]["text"].as_str().unwrap();
    assert!(text.contains(&label) && text.contains("-6"), "{text}");
    let reads = ["digest", "focus", "focus_body", "read", "snapshot", "status"];
    let sent = lab.methods_sent();
    assert!(sent.iter().all(|method| reads.contains(&method.as_str())), "a preview sends REAPER nothing but reads: {sent:?}");
    assert_eq!(volumes(), before);

    // What a plugin's plan may not hold is refused before anything runs: code, a file by path, a wait.
    for (n, steps) in [
        json!([{"tool": "run_lua", "input": {"code": "x = 1"}}]),
        json!([{"tool": "take_add", "input": {"ref": reference, "path": "/Users/me/Samples/kick.wav"}}]),
        json!([{"tool": "wait", "input": {"seconds": 1800}}]),
        // A file step's field takes only a source: never a path, nor a search of folders.
        json!([{"tool": "item_import", "input": {"ref": reference, "path": "/Users/me/Samples/kick.wav"}}]),
        json!([{"tool": "item_import", "input": {"ref": reference, "path": {"folders": ["/Users/me/Private"], "random": true}}}]),
    ]
    .into_iter()
    .enumerate()
    {
        let (refused, _) = wire.call("change", plan(&key, epoch, steps), Some(&format!("tc-refused-{n}"))).await;
        assert_eq!((refused["state"].clone(), refused["code"].clone()), (json!("refused"), json!("not_allowed")), "{refused}");
    }

    // A sound from a map this runtime hasn't made: stale at its preview, and nothing runs.
    let sound = json!([{"tool": "item_import", "input": {"ref": reference, "path": {"sound_id": "s-r1-1", "map_revision": "r1"}}}]);
    let mut asked = plan(&key, epoch, sound);
    asked["preview"] = json!(true);
    let (unmapped, _) = wire.call("change", asked, Some("tc-unmapped")).await;
    assert_eq!((unmapped["state"].clone(), unmapped["code"].clone()), (json!("refused"), json!("stale")), "{unmapped}");

    // Once per host call: it runs, and the same call asked again gets that answer without running again.
    let (first, _) = wire.call("change", plan(&key, epoch, down.clone()), Some("tc-1")).await;
    assert_eq!(first["state"], "applied", "{first}");
    assert_eq!((first["history_ids"].as_array().unwrap().len(), first["epoch"].as_u64()), (1, Some(epoch + 1)), "refs move on with it");
    assert!(volumes().contains(&-6.0), "{:?}", volumes());
    let steps_made = script.undo_history().len();
    let (again, _) = wire.call("change", plan(&key, epoch, down.clone()), Some("tc-1")).await;
    assert_eq!(again, first);
    assert_eq!(script.undo_history().len(), steps_made, "not run again");
    let other = plan(&key, epoch + 1, json!([{"tool": "track_volume", "input": {"ref": reference, "volumeDb": -3}}]));
    let (duplicate, error) = wire.call("change", other, Some("tc-1")).await;
    assert_eq!((error, duplicate["error"]["code"].clone()), (true, json!("duplicate")));
    // The plan made on the old epoch, asked by another call: stale, and nothing runs.
    let (stale, _) = wire.call("change", plan(&key, epoch, down), Some("tc-2")).await;
    assert_eq!((stale["state"].clone(), stale["code"].clone()), (json!("refused"), json!("stale")));

    // HISTORY's row: its family and its project; the host call that made it, with what it asked (C5-3), in
    // history.json for the runtime and the plugin, not in what the model reads.
    let (history, _) = wire.call("history", json!({}), None).await;
    let row = &history["items"][0];
    assert_eq!(
        (row["id"].clone(), row["family"].clone(), row["project_key"].clone()),
        (first["history_ids"][0].clone(), json!("mixer"), key.clone())
    );
    assert!(row.get("tool_call_id").is_none() && row.get("digest").is_none(), "{row}");
    let kept = lab.state("history.json");
    assert_eq!(kept["items"][0]["tool_call_id"], "tc-1");
    assert_eq!(kept["items"][0]["digest"].as_str().map(str::len), Some(64));

    // Undo, previewed then made once.
    let id = first["history_ids"][0].clone();
    let (preview, _) = wire.call("undo", json!({"history_id": id, "preview": true}), Some("tc-3")).await;
    assert_eq!((preview["state"].clone(), preview["undo_available"].clone()), (json!("preview"), json!(true)), "{preview}");
    let (undone, _) = wire.call("undo", json!({"history_id": id}), Some("tc-3")).await;
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(volumes(), before);
    let kept = lab.state("history.json");
    assert_eq!((kept["items"][0]["state"].clone(), kept["items"][0]["undo_available"].clone()), (json!("undone"), json!(false)));
    let (twice, _) = wire.call("undo", json!({"history_id": id}), Some("tc-4")).await;
    assert_eq!(twice["state"], "refused", "{twice}");
    wire.end().await;
});

case!(serve_runs_nothing_of_a_plan_made_before_reaper_switched_project_tabs, async {
    let script = ReaperScript::new(3, 0);
    let sim = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| sim.config(secret, "/lab/REAPER")))).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, reference, _) = looked(&mut wire).await;
    let before: Vec<f64> = (0..4).map(|n| script.volume(n)).collect();
    // The producer switches tabs while the approval waits: refs from the look before no longer hold.
    script.switch_tab();
    let down = json!([{"tool": "track_volume", "input": {"ref": reference, "volumeDb": -6}}]);
    let (changed, _) = wire.call("change", plan(&key, epoch, down), Some("tc-1")).await;
    assert_eq!(changed["state"], "refused", "{changed}");
    assert_eq!((0..4).map(|n| script.volume(n)).collect::<Vec<f64>>(), before, "nothing changed");
    assert_eq!(changed["history_ids"], json!([]));
    wire.end().await;
});

case!(serve_says_a_change_reaper_didnt_confirm_is_unsure_until_the_next_look, async {
    let script = ReaperScript::new(3, 0);
    let sim = script.clone();
    let lab = Lab::new(
        DawId::Reaper,
        Daw::Reaper,
        Some(Box::new(move |secret| {
            let mut config = sim.config(secret, "/lab/REAPER");
            let answer = config.respond.take().unwrap();
            // REAPER makes the change, but its answer is lost: whether it happened isn't known.
            config.respond = Some(Arc::new(move |method, params| {
                let answered = answer(method, params);
                if method == "change_apply" {
                    Some(Err(("uncertain".into(), "REAPER stopped answering".into())))
                } else {
                    answered
                }
            }));
            config
        })),
    )
    .await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, reference, _) = looked(&mut wire).await;
    let down = json!([{"tool": "track_volume", "input": {"ref": reference, "volumeDb": -6}}]);
    let (changed, error) = wire.call("change", plan(&key, epoch, down.clone()), Some("tc-1")).await;
    assert_eq!((error, changed["state"].clone(), changed["undo_available"].clone()), (false, json!("unsure"), json!(false)), "{changed}");
    assert_eq!(changed["epoch"].as_u64(), Some(epoch + 1), "refs from before may not hold");
    let kept = lab.state("history.json");
    let audit = kept["items"].as_array().unwrap().iter().find(|row| row["family"] == "unsure").unwrap().clone();
    assert_eq!((audit["undo_available"].clone(), audit["tool_call_id"].clone()), (json!(false), json!("tc-1")));
    assert!(changed["history_ids"].as_array().unwrap().contains(&audit["id"]));
    assert_eq!(lab.state("status.json")["unsure"]["op"], "change");
    let (stale, _) = wire.call("change", plan(&key, epoch, down), Some("tc-2")).await;
    assert_eq!(stale["code"], "stale");
    // The next look sees the project as it is: nothing is unsure any more.
    wire.call("observe", json!({"scope": "focus"}), None).await;
    assert_eq!(lab.state("status.json")["unsure"], Value::Null);
    wire.end().await;
});

case!(serve_audits_an_undo_reaper_didnt_confirm_once_and_moves_the_epoch_on_once, async {
    let script = ReaperScript::new(3, 0);
    let sim = script.clone();
    let lab = Lab::new(
        DawId::Reaper,
        Daw::Reaper,
        Some(Box::new(move |secret| {
            let mut config = sim.config(secret, "/lab/REAPER");
            let answer = config.respond.take().unwrap();
            // The change goes through; the undo's answer is lost, so whether it happened isn't known.
            config.respond = Some(Arc::new(move |method, params| {
                if method == "change_restore" {
                    Some(Err(("uncertain".into(), "REAPER stopped answering".into())))
                } else {
                    answer(method, params)
                }
            }));
            config
        })),
    )
    .await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, reference, _) = looked(&mut wire).await;
    let down = json!([{"tool": "track_volume", "input": {"ref": reference, "volumeDb": -6}}]);
    let (changed, _) = wire.call("change", plan(&key, epoch, down), Some("tc-1")).await;
    assert_eq!(changed["state"], "applied", "{changed}");
    let rows = |lab: &Lab| lab.state("history.json")["items"].as_array().map(Vec::len).unwrap_or_default();
    let (before_rows, before_epoch) = (rows(&lab), lab.ctx.link().epoch());
    let (undone, _) = wire.call("undo", json!({"history_id": changed["history_ids"][0]}), Some("tc-2")).await;
    assert_eq!(undone["state"], "unsure", "{undone}");
    assert_eq!(rows(&lab), before_rows + 1, "one audit row");
    assert_eq!(lab.ctx.link().epoch(), before_epoch + 1, "the epoch moved on once");
    let ids = undone["history_ids"].as_array().cloned().unwrap_or_default();
    let kept = lab.state("history.json");
    assert_eq!((ids.len(), kept["items"][0]["id"].clone()), (1, ids[0].clone()), "the answer names its audit row");
    wire.end().await;
});

case!(serve_reads_reaper_through_its_reader_and_gives_a_steps_input_as_a_plugins_plan_fills_it, async {
    let script = ReaperScript::new(3, 0);
    let tab = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| script.config(secret, "/lab/REAPER")))).await;
    let Err(OpError::NotConnected { .. }) = lab.call("read", json!({"action": "schema"})).await else { panic!("read unconnected") };
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, label) = looked(&mut wire).await;

    // What a change may use (the allowlist's steps the plan offers), with their families, and the readers.
    let (offered, _) = wire.call("read", json!({"action": "schema", "max_chars": 20000}), None).await;
    let names = |list: &str| -> Vec<String> {
        offered[list].as_array().unwrap().iter().map(|entry| entry["tool"].as_str().unwrap().to_string()).collect()
    };
    let (steps, readers) = (names("steps"), names("readers"));
    assert!(steps.iter().any(|step| step == "track_volume"), "{steps:?}");
    assert!(steps.iter().any(|step| step == "item_import"), "the import step's: {steps:?}");
    for unlisted in ["run_lua", "take_add", "wait", "track_delete", "save_project"] {
        assert!(!steps.iter().any(|step| step == unlisted), "{unlisted}: {steps:?}");
    }
    assert_eq!(readers, ["reaper_read"]);
    let volume = offered["steps"].as_array().unwrap().iter().find(|entry| entry["tool"] == "track_volume").unwrap().clone();
    assert_eq!(volume["family"], "mixer");
    let (whole, _) = wire.call("read", json!({"action": "schema", "tool": "track_volume"}), None).await;
    assert_eq!((whole["kind"].clone(), whole["input_schema"]["required"].clone()), (json!("step"), json!(["ref", "volumeDb"])));
    // A file step's field takes a source (CC-11), never a path.
    let (import, _) = wire.call("read", json!({"action": "schema", "tool": "item_import"}), None).await;
    assert_eq!((import["kind"].clone(), import["file_field"]["field"].clone()), (json!("step"), json!("path")), "{import}");
    assert!(import["input_schema"]["properties"]["path"]["anyOf"].is_array(), "{import}");
    let (lua, error) = wire.call("read", json!({"action": "schema", "tool": "run_lua"}), None).await;
    assert!(error && lua["error"]["code"] == "invalid", "{lua}");

    // The reader's rows, as REAPER gives them, names and not folders; never an API function by name, a query its
    // schema doesn't take, nor a tool that isn't a reader.
    let (tracks, _) = wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": {"kind": "track"}}), None).await;
    assert!(tracks["text"].as_str().unwrap().contains(&label), "{tracks}");
    assert_eq!((tracks["project_key"].clone(), tracks["epoch"].clone()), (key.clone(), json!(epoch)));
    let (project, _) = wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": {"kind": "project"}}), None).await;
    assert!(!project["text"].as_str().unwrap().contains("/lab/"), "{project}");
    for query in [json!({"kind": "call", "fn": "GetTrackName", "args": []}), json!({"kind": "track", "fn": "GetTrackName"})] {
        let (refused, _) = wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": query}), None).await;
        assert_eq!(refused["error"]["code"], "not_allowed", "{refused}");
    }
    let (odd, error) =
        wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": {"kind": "track", "bogus": 1}}), None).await;
    assert!(error && odd["error"]["code"] == "invalid", "{odd}");
    let (plan, error) = wire.call("read", json!({"action": "data", "tool": "make_changes", "query": {}}), None).await;
    assert!(error && plan["error"]["code"] == "invalid", "{plan}");
    let reads = ["digest", "focus", "focus_body", "read", "snapshot", "status"];
    let sent = lab.methods_sent();
    assert!(sent.iter().all(|method| reads.contains(&method.as_str())), "{sent:?}");

    // REAPER switches tabs after the look: its rows aren't the looked project's, so a read is stale, not stamped with it.
    tab.switch_tab();
    let (stale, error) = wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": {"kind": "track"}}), None).await;
    assert!(error && stale["error"]["code"] == "stale", "{stale}");
    // The next look is the new tab's, and reads hold again.
    wire.call("observe", json!({"scope": "focus"}), None).await;
    let (again, error) = wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": {"kind": "track"}}), None).await;
    assert!(!error && again["project_key"] != key, "{again}");
    wire.end().await;
});

/// (#980) The look's own snapshot gives the epoch a read is held to: REAPER switches from project A to B right after
/// it answers the look's snapshot (where a second read of the epoch, after the look, would have read B's), and B's
/// page is never taken for A's: A's key stays with A's epoch, a read is stale, and the next look is B's with B's epoch.
case!(serve_holds_a_read_to_the_epoch_of_the_snapshot_the_look_read_so_a_switch_after_it_is_never_the_looked_project, async {
    let script = ReaperScript::new(3, 0);
    // What REAPER does as it answers the next look's snapshot: nothing (0), switch to another project right after (1),
    // or answer without its epoch (2), as a look Kumi can't pin.
    let next_look = Arc::new(AtomicU8::new(0));
    let armed = next_look.clone();
    let lab = Lab::new(
        DawId::Reaper,
        Daw::Reaper,
        Some(Box::new(move |secret| {
            let mut config = script.config(secret, "/lab/REAPER");
            let answer = script.respond();
            config.respond = Some(Arc::new(move |method, params| {
                let mut answered = answer(method, params);
                if method == "snapshot" {
                    match armed.swap(0, Ordering::SeqCst) {
                        1 => script.switch_tab(),
                        2 => {
                            if let Some(Ok(Value::Object(snapshot))) = answered.as_mut() {
                                snapshot.remove("epoch");
                            }
                        }
                        _ => {}
                    }
                }
                answered
            }));
            config
        })),
    )
    .await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (a, _, _, _) = looked(&mut wire).await;
    assert_eq!(lab.ctx.link().observed_native_epoch().as_deref(), Some("sim-1"), "A's epoch, from the look's snapshot");
    let track = json!({"action": "data", "tool": "reaper_read", "query": {"kind": "track"}});
    let (rows, error) = wire.call("read", track.clone(), None).await;
    assert!(!error && rows["project_key"] == a, "{rows}");

    // A→B between the look's snapshot and anything after it.
    next_look.store(1, Ordering::SeqCst);
    let (seen, _) = wire.call("observe", json!({"scope": "focus"}), None).await;
    assert_eq!(seen["project_key"], a, "the look read A: {seen}");
    assert_eq!(lab.ctx.link().observed_native_epoch().as_deref(), Some("sim-1"), "A's key keeps A's epoch, never B's");
    assert!(!lab.ctx.link().page_is_looked(Some("sim-2")));
    let (stale, error) = wire.call("read", track.clone(), None).await;
    assert!(error && stale["error"]["code"] == "stale", "B's page passed as A's: {stale}");
    // Refused for its epoch, not for anything else that's stale (a return the focus poll saw, say).
    let said = stale["error"]["reason"].as_str().unwrap_or_default();
    assert!(said.contains("can't tell these rows are the looked project's"), "{stale}");

    // The next look is B's, with B's epoch: B's rows are B's.
    let (b, _) = wire.call("observe", json!({"scope": "focus"}), None).await;
    assert_ne!(b["project_key"], a, "{b}");
    assert_eq!(lab.ctx.link().observed_native_epoch().as_deref(), Some("sim-2"));
    let (rows, error) = wire.call("read", track.clone(), None).await;
    assert!(!error && rows["project_key"] == b["project_key"], "{rows}");

    // A look whose snapshot gave no epoch pins nothing: a read isn't taken on trust.
    next_look.store(2, Ordering::SeqCst);
    wire.call("observe", json!({"scope": "focus"}), None).await;
    assert_eq!(lab.ctx.link().observed_native_epoch(), None);
    let (unpinned, error) = wire.call("read", track, None).await;
    assert!(error && unpinned["error"]["code"] == "stale", "{unpinned}");
    wire.end().await;
});

/// Live as far as a plan needs it: a look that offers the plan tool and its `replace_sample` step. The plan tool records
/// every plan it's given and Kumi's change record for it, as Live's would.
struct PlanLive {
    on_connection: kumi_agent::core::contracts::ConnectionListener,
    on_change: Option<Rc<dyn Fn(kumi_types::contracts::ChangeRecord)>>,
    given: Rc<std::cell::RefCell<Vec<Value>>>,
}

struct PlanTool {
    on_change: Option<Rc<dyn Fn(kumi_types::contracts::ChangeRecord)>>,
    given: Rc<std::cell::RefCell<Vec<Value>>>,
}

struct SampleStep;

#[async_trait(?Send)]
impl kumi_types::contracts::KernelTool for PlanTool {
    fn name(&self) -> &str {
        "make_changes"
    }
    fn description(&self) -> &str {
        "Make changes in one call."
    }
    fn input_schema(&self) -> JsonObject {
        let schema = json!({"type": "object", "required": ["steps"], "properties": {"steps": {"type": "array", "items": {
            "type": "object", "properties": {"tool": {"type": "string", "enum": ["replace_sample", "wait"]}, "input": {"type": "object"}}
        }}}});
        schema.as_object().unwrap().clone()
    }
    async fn execute(&self, input: JsonObject, _: Signal) -> Result<kumi_types::contracts::ToolResult, RuntimeError> {
        self.given.borrow_mut().push(Value::Object(input));
        let n = self.given.borrow().len();
        if let Some(on_change) = &self.on_change {
            on_change(kumi_types::contracts::ChangeRecord {
                id: format!("c{n}"),
                family: kumi_types::contracts::ChangeFamily::Device,
                title: "Simpler sample replaced".into(),
                track: None,
                from: None,
                to: None,
                range: None,
                colors: None,
                clip: None,
                devices: None,
                state: kumi_types::contracts::ChangeState::Applied,
                score: None,
                note: None,
                at: 1_700_000_000_000,
                picture: None,
                batch: None,
            });
        }
        Ok(kumi_types::contracts::ToolResult::text(json!({"done": [{"step": 1, "changed": true, "change": format!("c{n}")}]}).to_string()))
    }
}

#[async_trait(?Send)]
impl kumi_types::contracts::KernelTool for SampleStep {
    fn name(&self) -> &str {
        "replace_sample"
    }
    fn description(&self) -> &str {
        "Swap the sample in a Simpler for another."
    }
    fn input_schema(&self) -> JsonObject {
        let schema = json!({"type": "object", "additionalProperties": false, "required": ["deviceRef", "sample"], "properties": {
            "deviceRef": {"type": "string", "minLength": 1}, "sample": {"type": "string", "minLength": 1}
        }});
        schema.as_object().unwrap().clone()
    }
    async fn execute(&self, _: JsonObject, _: Signal) -> Result<kumi_types::contracts::ToolResult, RuntimeError> {
        Ok(kumi_types::contracts::ToolResult::error("run through make_changes"))
    }
}

#[async_trait(?Send)]
impl kumi_agent::core::contracts::Integration for PlanLive {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        (self.on_connection)(kumi_agent::core::contracts::ConnectionState::Connected, None);
        Ok(())
    }
    async fn observe(
        &self,
        _: Signal,
        _: Option<kumi_agent::core::contracts::ObserveHints>,
    ) -> Result<kumi_agent::core::contracts::Observation, RuntimeError> {
        let plan = PlanTool { on_change: self.on_change.clone(), given: self.given.clone() };
        let tools: Vec<Rc<dyn kumi_types::contracts::KernelTool>> = vec![Rc::new(plan), Rc::new(SampleStep)];
        Ok(kumi_agent::core::contracts::Observation {
            native_epoch: None,
            key: "live:demo".into(),
            revision: None,
            label: "Live — Demo".into(),
            context: "Drums: Drum Rack with a Simpler".into(),
            instructions: String::new(),
            tools,
            project: None,
            set: Some("set-1".into()),
            tracks: None,
            saved_at: None,
            identity: None,
        })
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// A runtime for Live (its own home's bridge) whose look offers [`PlanLive`]'s plan, with a media root holding a sound,
/// a note and another sound; and what the plan tool was given.
/// A decaying tone, as a 16-bit mono WAV: audio the library's scan measures.
fn tone_wav(hz: f64, seconds: f64) -> Vec<u8> {
    let rate = 44_100u32;
    let frames = (f64::from(rate) * seconds) as u32;
    let mut data = Vec::with_capacity(frames as usize * 2);
    for n in 0..frames {
        let at = f64::from(n) / f64::from(rate);
        let level = (2.0 * std::f64::consts::PI * hz * at).sin() * (-at * 12.0).exp() * 0.8;
        data.extend(((level * f64::from(i16::MAX)) as i16).to_le_bytes());
    }
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    wav
}

struct ImportLab {
    _home: tempfile::TempDir,
    media: tempfile::TempDir,
    library: tempfile::TempDir,
    ctx: Rc<Ctx>,
    given: Rc<std::cell::RefCell<Vec<Value>>>,
}

impl ImportLab {
    fn new() -> ImportLab {
        let home = tempfile::tempdir().unwrap();
        let media = tempfile::tempdir().unwrap();
        let wav = |bytes: usize| {
            let mut data =
                b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x44\xac\x00\x00\x88\x58\x01\x00\x02\x00\x10\x00data"
                    .to_vec();
            data.extend(std::iter::repeat_n(0u8, bytes));
            data
        };
        std::fs::write(media.path().join("Dusty Snare 04.wav"), wav(64)).unwrap();
        std::fs::write(media.path().join("Kick.wav"), wav(32)).unwrap();
        std::fs::write(media.path().join("notes.txt"), b"not audio").unwrap();
        let library = tempfile::tempdir().unwrap();
        std::fs::write(library.path().join("Snare Tone.wav"), tone_wav(190.0, 0.4)).unwrap();
        let transport = home.path().join("kumi-home");
        let config = transport.join("bridge/state/bridge-config.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        let scripts = home.path().join("Remote Scripts");
        std::fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
        std::fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config": config}).to_string()).unwrap();
        let env = Env::from([
            ("HOME".into(), home.path().display().to_string()),
            ("KUMI_HOME".into(), transport.display().to_string()),
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts.display().to_string()),
            ("KUMI_PROCESSES".into(), "none".into()),
        ]);
        let given = Rc::new(std::cell::RefCell::new(Vec::new()));
        let factory = {
            let given = given.clone();
            AbletonFactory(Rc::new(move |options| {
                Rc::new(PlanLive {
                    on_connection: options.on_connection.clone(),
                    on_change: options.on_change.clone(),
                    given: given.clone(),
                }) as Rc<dyn kumi_agent::core::contracts::Integration>
            }))
        };
        let args = Args {
            state_dir: home.path().join("state"),
            kumi_home: transport,
            daw: DawId::Ableton,
            library_roots: vec![library.path().to_path_buf()],
            media_roots: vec![media.path().to_path_buf()],
            tools: BTreeMap::new(),
            hermes_home: None,
            hermes_root: None,
        };
        let ctx = Ctx::new(args, env, kumi_daw::io::Answers::default().with(factory)).unwrap();
        ImportLab { _home: home, media, library, ctx, given }
    }
}

/// A plan placing `sample` in the Simpler.
/// A canonical path as a DAW gets it: on Windows without the verbatim `\\?\` prefix canonicalizing gives.
fn daw_path(path: std::path::PathBuf) -> std::path::PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}").into(),
        None => text.strip_prefix(r"\\?\").map_or(path, std::path::PathBuf::from),
    }
}

fn sample_plan(key: &Value, epoch: u64, sample: Value) -> Value {
    plan(key, epoch, json!([{"tool": "replace_sample", "input": {"deviceRef": "7:device:0:0", "sample": sample}}]))
}

case!(serve_places_a_file_in_lives_simpler_only_from_a_source_it_resolves_and_pins, async {
    let lab = ImportLab::new();
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (connected, refused) = wire.call("connect", json!({}), None).await;
    assert!(!refused, "{connected}");
    let (seen, _) = wire.call("observe", json!({"scope": "project"}), None).await;
    let (key, epoch) = (seen["project_key"].clone(), seen["epoch"].as_u64().unwrap());
    let media = std::fs::canonicalize(lab.media.path()).unwrap();
    let provenance = json!({"host_tool": "elevenlabs_sound_effect", "tool_call_id": "call-9"});

    // Refused before anything runs: a path, a search of folders, a file outside the approved roots, a file that isn't
    // audio, a map sound with no map.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("Private.wav"), b"RIFF").unwrap();
    for (n, sample) in [
        json!(media.join("Dusty Snare 04.wav").display().to_string()),
        json!({"folders": [media.display().to_string()], "random": true}),
        json!({"file": outside.path().join("Private.wav").display().to_string(), "provenance": provenance}),
        json!({"file": media.join("notes.txt").display().to_string(), "provenance": provenance}),
        json!({"sound_id": "s-r1-1", "map_revision": "r1"}),
    ]
    .into_iter()
    .enumerate()
    {
        let (refused, _) = wire.call("change", sample_plan(&key, epoch, sample.clone()), Some(&format!("tc-no-{n}"))).await;
        assert_eq!(refused["state"], "refused", "{sample}: {refused}");
    }
    assert!(lab.given.borrow().is_empty(), "nothing ran");

    // A host's file: the approval names it, and the DAW gets a copy in Kumi's artifacts, never the host's folder.
    let file = json!({"file": media.join("Dusty Snare 04.wav").display().to_string(), "provenance": provenance});
    let mut asked = sample_plan(&key, epoch, file.clone());
    asked["preview"] = json!(true);
    let (preview, _) = wire.call("change", asked, Some("tc-1")).await;
    let text = preview["steps"][0]["text"].as_str().unwrap_or_else(|| panic!("{preview}"));
    assert!(text.contains("\"Dusty Snare 04.wav\"") && !text.contains(&media.display().to_string()), "{text}");
    let (placed, _) = wire.call("change", sample_plan(&key, epoch, file), Some("tc-1")).await;
    assert_eq!(placed["state"], "applied", "{placed}");
    let sent = lab.given.borrow()[0]["steps"][0]["input"]["sample"].as_str().unwrap().to_string();
    let artifacts = daw_path(std::fs::canonicalize(lab.ctx.paths().state_dir.join("artifacts")).unwrap());
    assert!(std::path::Path::new(&sent).starts_with(&artifacts), "{sent}");
    assert!(!sent.starts_with(r"\\?\"), "the DAW gets a plain path: {sent}");
    let row = lab.ctx.history().row(placed["history_ids"][0].as_str().unwrap()).unwrap();
    let source = &row.sources.unwrap()[0];
    assert_eq!(
        (source["field"].clone(), source["file"].clone(), source["host_tool"].clone()),
        (json!("sample"), json!("Dusty Snare 04.wav"), json!("elevenlabs_sound_effect"))
    );
    let artifact = source["artifact_id"].as_str().unwrap().to_string();
    assert!(source["identity"]["size"].as_u64().is_some());

    // The artifact it became places again, as it is. The hook previews the model's arguments as written (the epoch as
    // digits) and the handler dispatches them coerced (a number): the same plan, by the same call.
    let epoch = placed["epoch"].as_u64().unwrap();
    let artifact_plan = sample_plan(&key, epoch, json!({"artifact_id": artifact}));
    let mut written = artifact_plan.clone();
    written["expected_epoch"] = json!(epoch.to_string());
    written["preview"] = json!(true);
    wire.call("change", written, Some("tc-2")).await;
    let (again, _) = wire.call("change", artifact_plan.clone(), Some("tc-2")).await;
    assert_eq!(again["state"], "applied", "{again}");
    assert_eq!(lab.given.borrow()[1]["steps"][0]["input"]["sample"], json!(sent));

    // A plan with a file is held to its approval: one never previewed, or not the one previewed by the same call, is
    // stale, and nothing runs.
    let epoch = again["epoch"].as_u64().unwrap();
    let unapproved = sample_plan(&key, epoch, json!({"artifact_id": artifact}));
    let (unasked, _) = wire.call("change", unapproved.clone(), Some("tc-4")).await;
    assert_eq!((unasked["state"].clone(), unasked["code"].clone()), (json!("refused"), json!("stale")), "{unasked}");
    let mut previewed = unapproved.clone();
    previewed["preview"] = json!(true);
    wire.call("change", previewed, Some("tc-5")).await;
    let mut other = unapproved.clone();
    other["title"] = json!("Something else");
    let (switched, _) = wire.call("change", other, Some("tc-5")).await;
    assert_eq!((switched["state"].clone(), switched["code"].clone()), (json!("refused"), json!("stale")), "{switched}");
    assert_eq!(lab.given.borrow().len(), 2);
    // A preview made by one call binds neither a dispatch with no call id nor the same id in another chat.
    let mut previewed = unapproved.clone();
    previewed["preview"] = json!(true);
    wire.call("change", previewed.clone(), Some("tc-7")).await;
    let (unnamed_dispatch, _) = wire.call("change", unapproved.clone(), None).await;
    assert_eq!(unnamed_dispatch["code"], "stale", "{unnamed_dispatch}");
    wire.call_in("change", previewed, Some("tc-8"), "chat-1").await;
    let (other_chat, _) = wire.call_in("change", unapproved.clone(), Some("tc-8"), "chat-2").await;
    assert_eq!(other_chat["code"], "stale", "{other_chat}");
    assert_eq!(lab.given.borrow().len(), 2);
    // A preview that came with no call id (a hook that had none) binds its dispatch by the plan alone.
    let mut unnamed = unapproved.clone();
    unnamed["preview"] = json!(true);
    wire.call("change", unnamed, None).await;
    let (bound, _) = wire.call("change", unapproved, Some("tc-6")).await;
    assert_eq!(bound["state"], "applied", "{bound}");
    assert_eq!(lab.given.borrow().len(), 3);
    let epoch = bound["epoch"].as_u64().unwrap();

    // 11b: the file changes between the approval and the dispatch: stale, and nothing runs.
    let kick = json!({"file": media.join("Kick.wav").display().to_string(), "provenance": provenance});
    let mut asked = sample_plan(&key, epoch, kick.clone());
    asked["preview"] = json!(true);
    wire.call("change", asked, Some("tc-3")).await;
    std::fs::write(media.join("Kick.wav"), b"RIFF and something else entirely").unwrap();
    let (stale, _) = wire.call("change", sample_plan(&key, epoch, kick), Some("tc-3")).await;
    assert_eq!((stale["state"].clone(), stale["code"].clone()), (json!("refused"), json!("stale")), "{stale}");
    assert_eq!(lab.given.borrow().len(), 3);

    // A plan with two host's files keeps neither when the second can't be copied (over the artifacts' 50 MiB): every
    // file is staged before any is kept, so the first isn't left in the artifacts for nothing.
    let big = media.join("Long Take.wav");
    std::fs::File::create(&big).unwrap().set_len(51 * 1024 * 1024).unwrap();
    let artifacts = lab.ctx.paths().state_dir.join("artifacts");
    let kept_before = std::fs::read_dir(&artifacts).unwrap().count();
    let two = |n: u64| {
        json!({"title": "Two samples", "expected_project_key": key, "expected_epoch": n, "steps": [
            {"tool": "replace_sample", "input": {"deviceRef": "7:device:0:0", "sample": {"file": media.join("Kick.wav").display().to_string(), "provenance": provenance}}},
            {"tool": "replace_sample", "input": {"deviceRef": "7:device:0:0", "sample": {"file": big.display().to_string(), "provenance": provenance}}},
        ]})
    };
    let mut asked = two(epoch);
    asked["preview"] = json!(true);
    wire.call("change", asked, Some("tc-two")).await;
    let (too_long, _) = wire.call("change", two(epoch), Some("tc-two")).await;
    assert_eq!(too_long["state"], "refused", "{too_long}");
    assert_eq!(std::fs::read_dir(&artifacts).unwrap().count(), kept_before, "neither file kept");
    assert_eq!(lab.given.borrow().len(), 3);

    // 11d, the demo's step: a sound from a scanned library folder, found by its words, placed as it is.
    let (scan, _) = wire.call("library", json!({"action": "scan"}), Some("tc-scan")).await;
    let job = scan["job_id"].as_str().unwrap_or_else(|| panic!("{scan}")).to_string();
    let scanned = loop {
        let (state, _) = wire.call("job", json!({"job_id": job}), None).await;
        if state["state"] != "running" {
            break state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(scanned["state"], "done", "{scanned}");
    let (found, _) = wire.call("sounds", json!({"query": "snare"}), None).await;
    let sound = found["items"].as_array().and_then(|items| items.first()).unwrap_or_else(|| panic!("{found}")).clone();
    let from_map = json!({"sound_id": sound["sound_id"], "map_revision": found["map_revision"]});
    let mut asked = sample_plan(&key, epoch, from_map.clone());
    asked["preview"] = json!(true);
    let (preview, _) = wire.call("change", asked, Some("tc-map")).await;
    assert!(preview["steps"][0]["text"].as_str().is_some_and(|text| text.contains("\"Snare Tone.wav\"")), "{preview}");
    let (from_library, _) = wire.call("change", sample_plan(&key, epoch, from_map), Some("tc-map")).await;
    assert_eq!(from_library["state"], "applied", "{from_library}");
    let library_file = daw_path(std::fs::canonicalize(lab.library.path().join("Snare Tone.wav")).unwrap());
    assert_eq!(lab.given.borrow()[3]["steps"][0]["input"]["sample"], json!(library_file.display().to_string()), "in place");
    let row = lab.ctx.history().row(from_library["history_ids"][0].as_str().unwrap()).unwrap();
    let source = &row.sources.unwrap()[0];
    assert_eq!((source["sound_id"].clone(), source["step"].clone()), (sound["sound_id"].clone(), json!(1)));
    // What the model reads of HISTORY names the file, never the call or the file's identity.
    let (history, _) = wire.call("history", json!({}), None).await;
    let shown = &history["items"][0];
    assert!(shown.get("tool_call_id").is_none() && shown["sources"][0].get("identity").is_none(), "{shown}");
    wire.end().await;
});

case!(serve_refuses_a_path_in_renoises_file_field_before_anything_runs, async {
    let lab = Lab::new(DawId::Renoise, Daw::Renoise, Some(Box::new(|secret| SimulatorConfig::renoise(secret, "/lab/Renoise")))).await;
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (connected, refused) = wire.call("connect", json!({}), None).await;
    assert!(!refused, "{connected}");
    let (seen, _) = wire.call("observe", json!({"scope": "project"}), None).await;
    let key = seen["project_key"].clone();
    let epoch = seen["epoch"].as_u64().unwrap_or_else(|| panic!("{seen}"));
    let sent = lab.methods_sent();
    for (n, path) in ["/Users/me/Samples/kick.wav", "C:\\Samples\\kick.wav", "library:Drums/kick.wav"].into_iter().enumerate() {
        let steps = json!([{"tool": "sample_load", "input": {"ref": "i1", "path": path}}]);
        let (refused, _) = wire.call("change", plan(&key, epoch, steps), Some(&format!("tc-path-{n}"))).await;
        assert_eq!((refused["state"].clone(), refused["code"].clone()), (json!("refused"), json!("not_allowed")), "{path}: {refused}");
    }
    assert_eq!(lab.methods_sent(), sent, "nothing reached Renoise");
    wire.end().await;
});

/// Live whose undo is guarded, as Live's integration is on the plugin host: it says what an undo would do from fresh
/// reads, and refuses one whose values changed since (`refuse`), counting the undos it lets through. Whether the
/// runtime's write lease was held as it checked goes to `held`.
struct Guarded {
    on_connection: kumi_agent::core::contracts::ConnectionListener,
    state: Rc<GuardedState>,
}

struct GuardedState {
    refuse: Cell<bool>,
    undos: Cell<usize>,
    lease: std::cell::RefCell<std::rc::Weak<Ctx>>,
    held: Cell<Option<bool>>,
}

#[async_trait(?Send)]
impl kumi_agent::core::contracts::Integration for Guarded {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        (self.on_connection)(kumi_agent::core::contracts::ConnectionState::Connected, None);
        Ok(())
    }
    async fn observe(
        &self,
        _: Signal,
        _: Option<kumi_agent::core::contracts::ObserveHints>,
    ) -> Result<kumi_agent::core::contracts::Observation, RuntimeError> {
        Ok(kumi_agent::core::contracts::Observation {
            native_epoch: None,
            key: "live:demo".into(),
            revision: None,
            label: "Live — Demo".into(),
            context: "Drums".into(),
            instructions: String::new(),
            tools: vec![],
            project: None,
            set: Some("set-1".into()),
            tracks: None,
            saved_at: None,
            identity: None,
        })
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn has_undo(&self) -> bool {
        true
    }
    async fn guarded_undo(
        &self,
        id: &str,
        _: Signal,
    ) -> Result<Result<kumi_types::contracts::ChangeRecord, kumi_agent::core::contracts::UndoRefusal>, RuntimeError> {
        let state = &self.state;
        state.held.set(state.lease.borrow().upgrade().map(|ctx| ctx.lease().held()));
        if state.refuse.get() {
            let reason = "volume changed in Live since Kumi set it, so Kumi left the change as it is.".into();
            return Ok(Err(kumi_agent::core::contracts::UndoRefusal { code: "changed_since", reason }));
        }
        state.undos.set(state.undos.get() + 1);
        Ok(Ok(serde_json::from_value(json!({"id": id, "family": "mixer", "title": "Drums volume", "state": "undone", "at": 2})).unwrap()))
    }
    async fn undo_preview(&self, _: &str, _: Signal) -> Result<Option<kumi_agent::core::contracts::UndoPreview>, RuntimeError> {
        let refusal = self.state.refuse.get().then(|| kumi_agent::core::contracts::UndoRefusal {
            code: "changed_since",
            reason: "volume changed in Live since Kumi set it, so Kumi left the change as it is.".into(),
        });
        Ok(Some(kumi_agent::core::contracts::UndoPreview { text: "Undo volume -4.0 dB → -6.0 dB".into(), refusal }))
    }
}

case!(serve_answers_a_guarded_undos_refusal_with_nothing_recorded_audited_or_moved_on, async {
    let home = tempfile::tempdir().unwrap();
    let transport = home.path().join("kumi-home");
    let config = transport.join("bridge/state/bridge-config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "{}").unwrap();
    let scripts = home.path().join("Remote Scripts");
    std::fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
    std::fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config": config}).to_string()).unwrap();
    let env = Env::from([
        ("HOME".into(), home.path().display().to_string()),
        ("KUMI_HOME".into(), transport.display().to_string()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts.display().to_string()),
        ("KUMI_PROCESSES".into(), "none".into()),
    ]);
    let live = Rc::new(GuardedState { refuse: Cell::new(true), undos: Cell::new(0), lease: Default::default(), held: Cell::new(None) });
    let factory = {
        let live = live.clone();
        AbletonFactory(Rc::new(move |options| {
            Rc::new(Guarded { on_connection: options.on_connection.clone(), state: live.clone() })
                as Rc<dyn kumi_agent::core::contracts::Integration>
        }))
    };
    let args = Args {
        state_dir: home.path().join("state"),
        kumi_home: transport,
        hermes_home: None,
        hermes_root: None,
        daw: DawId::Ableton,
        library_roots: vec![],
        media_roots: vec![],
        tools: BTreeMap::new(),
    };
    let ctx = Ctx::new(args, env, kumi_daw::io::Answers::default().with(factory)).unwrap();
    *live.lease.borrow_mut() = Rc::downgrade(&ctx);
    let mut wire = Wire::start(ctx.clone()).await;
    let (connected, refused) = wire.call("connect", json!({}), None).await;
    assert!(!refused, "{connected}");
    let (seen, _) = wire.call("observe", json!({"scope": "focus"}), None).await;
    let key = seen["project_key"].as_str().unwrap().to_owned();
    // A change of Kumi's to that mixer, as the integration's record of it reaches HISTORY.
    let made = json!({"id": "c1", "family": "mixer", "title": "Drums volume -6.0 dB → -4.0 dB", "state": "applied", "at": 1});
    let row = ctx.history().record(&serde_json::from_value(made).unwrap(), &key, ctx.snapshots());
    let before = (wire.call("history", json!({}), None).await.0, wire.call("status", json!({}), None).await.0["project"].clone());
    // Its preview is the integration's fresh read, with why it won't go: never the title the change was made with.
    let (preview, _) = wire.call("undo", json!({"history_id": row.id, "preview": true}), None).await;
    assert_eq!((preview["undo_available"].clone(), preview["reason"].clone()), (json!(false), json!("changed_since")), "{preview}");
    assert!(preview["text"].as_str().unwrap().contains("-4.0 dB → -6.0 dB"), "{preview}");
    // The undo: refused, and nothing recorded, audited, or moved on.
    let (undone, error) = wire.call("undo", json!({"history_id": row.id}), Some("tc-undo")).await;
    assert!(!error, "{undone}");
    assert_eq!(
        (undone["state"].clone(), undone["code"].clone(), undone.get("history_ids")),
        (json!("refused"), json!("changed_since"), None)
    );
    let after = (wire.call("history", json!({}), None).await.0, wire.call("status", json!({}), None).await.0["project"].clone());
    assert_eq!(after, before, "HISTORY and the epoch as they were");
    assert_eq!((live.undos.get(), live.held.get()), (0, Some(true)), "checked under the endpoint's write lease");
    // Untouched since: undone.
    live.refuse.set(false);
    let (undone, _) = wire.call("undo", json!({"history_id": row.id}), Some("tc-undo-2")).await;
    assert_eq!((undone["state"].clone(), live.undos.get()), (json!("undone"), 1), "{undone}");
    wire.end().await;
});
#[path = "serve_listen.rs"]
mod track_listening;
