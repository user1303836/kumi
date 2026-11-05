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
use kumi_daw_own::provider::OwnHosts;
use kumi_types::contracts::JsonObject;
use kumi_types::system::Env;
use kumi_types::{daw::DawId, errors::RuntimeError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::{cell::Cell, rc::Rc, sync::Arc};

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
        let host_home = transport.clone();
        let own_host = OwnHosts(Rc::new(move |_| {
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(bridge, &host_home)), id: Cell::new(0) });
            Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            })
        }));
        let answers = kumi_daw::io::Answers::default()
            .with(AbletonFactory(Rc::new(|_| panic!("a REAPER runtime makes no Ableton integration"))))
            .with(own_host);
        let args = Args {
            state_dir: home.path().join("state"),
            kumi_home: transport,
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
    let home = tempfile::tempdir().unwrap();
    let transport = home.path().join("kumi-home");
    let config = owner.unwrap_or(&transport).join("bridge/state/bridge-config.json");
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

case!(serve_connects_to_live_through_the_bridge_its_own_home_installed, async {
    let (connected, started) = live_connect(None).await;
    let status = connected.expect("its own bridge");
    assert!(started, "Live's integration started");
    assert_eq!((status["daw"].clone(), status["project"].clone()), (json!("ableton"), Value::Null), "no look yet: Live isn't open");
});
