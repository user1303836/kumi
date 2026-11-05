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
        // A file goes in only through the import step: not in this runtime yet, whatever the field holds.
        json!([{"tool": "item_import", "input": {"ref": reference, "path": {"sound_id": "s-r1-1", "map_revision": "r1"}}}]),
        json!([{"tool": "item_import", "input": {"ref": reference, "path": {"folders": ["/Users/me/Private"], "random": true}}}]),
    ]
    .into_iter()
    .enumerate()
    {
        let (refused, _) = wire.call("change", plan(&key, epoch, steps), Some(&format!("tc-refused-{n}"))).await;
        assert_eq!((refused["state"].clone(), refused["code"].clone()), (json!("refused"), json!("not_allowed")), "{refused}");
    }

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

    // HISTORY's row: its family, its project, and the host call that made it with what it asked (C5-3).
    let (history, _) = wire.call("history", json!({}), None).await;
    let row = &history["items"][0];
    assert_eq!(
        (row["id"].clone(), row["family"].clone(), row["project_key"].clone()),
        (first["history_ids"][0].clone(), json!("mixer"), key.clone())
    );
    assert_eq!(row["tool_call_id"], "tc-1");
    assert_eq!(row["digest"].as_str().map(str::len), Some(64));

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

case!(serve_reads_reaper_through_its_reader_and_gives_a_steps_input_as_a_plugins_plan_fills_it, async {
    let script = ReaperScript::new(3, 0);
    let tab = script.clone();
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, Some(Box::new(move |secret| script.config(secret, "/lab/REAPER")))).await;
    let Err(OpError::NotConnected { .. }) = lab.call("read", json!({"action": "schema"})).await else { panic!("read unconnected") };
    let mut wire = Wire::start(lab.ctx.clone()).await;
    let (key, epoch, _, label) = looked(&mut wire).await;

    // What a change may use (the allowlist's steps the plan offers), with their families, and the readers. A step that
    // places a file isn't one until the import step is in the runtime.
    let (offered, _) = wire.call("read", json!({"action": "schema", "max_chars": 20000}), None).await;
    let names = |list: &str| -> Vec<String> {
        offered[list].as_array().unwrap().iter().map(|entry| entry["tool"].as_str().unwrap().to_string()).collect()
    };
    let (steps, readers) = (names("steps"), names("readers"));
    assert!(steps.iter().any(|step| step == "track_volume"), "{steps:?}");
    for unlisted in ["run_lua", "take_add", "wait", "track_delete", "save_project", "item_import"] {
        assert!(!steps.iter().any(|step| step == unlisted), "{unlisted}: {steps:?}");
    }
    assert_eq!(readers, ["reaper_read"]);
    let volume = offered["steps"].as_array().unwrap().iter().find(|entry| entry["tool"] == "track_volume").unwrap().clone();
    assert_eq!(volume["family"], "mixer");
    let (whole, _) = wire.call("read", json!({"action": "schema", "tool": "track_volume"}), None).await;
    assert_eq!((whole["kind"].clone(), whole["input_schema"]["required"].clone()), (json!("step"), json!(["ref", "volumeDb"])));
    let (import, error) = wire.call("read", json!({"action": "schema", "tool": "item_import"}), None).await;
    assert!(error && import["error"]["reason"].as_str().unwrap().contains("import step"), "{import}");
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

    // A look whose epoch Kumi couldn't pin (REAPER busy as it read, say): a read isn't taken on trust.
    lab.ctx.link().saw_page_epoch(None);
    let (unpinned, error) = wire.call("read", json!({"action": "data", "tool": "reaper_read", "query": {"kind": "track"}}), None).await;
    assert!(error && unpinned["error"]["code"] == "stale", "{unpinned}");
    wire.call("observe", json!({"scope": "focus"}), None).await;

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
