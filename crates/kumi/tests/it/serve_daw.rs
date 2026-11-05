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

/// A runtime for `bridge`'s DAW with Kumi's part in it (as its receipt says, when `installed`) and `simulator`
/// answering as the DAW behind Kumi's host.
struct Lab {
    home: tempfile::TempDir,
    ctx: Rc<Ctx>,
    _simulator: Option<Rc<Simulator>>,
}

impl Lab {
    async fn new(daw: DawId, bridge: Daw, config: Option<Box<dyn FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig>>) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let transport = home.path().join("transport");
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
        Lab { home, ctx, _simulator: simulator }
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
    assert!(key.starts_with("p-") && first["epoch"] == 1, "{first}");

    let seen = lab.call("observe", json!({"scope": "project"})).await.unwrap();
    assert_eq!((seen["project_key"].as_str(), seen["epoch"].clone()), (Some(key.as_str()), json!(1)), "the same project, the same epoch");
    assert!(!seen["text"].as_str().unwrap().is_empty());
    let refs = seen["refs"].as_array().unwrap();
    assert!(refs.iter().filter(|candidate| candidate["kind"] == "track").count() >= 3, "{refs:?}");
    assert!(refs.iter().all(|candidate| candidate["reference"]["kind"] == "daw"), "only the project's things: {refs:?}");
    let focus = lab.call("observe", json!({"scope": "focus", "max_chars": 200})).await.unwrap();
    assert!(focus["text"].as_str().unwrap().contains("(epoch 1)"), "{focus}");
    assert!(lab.ctx.link().expect(&key, 1).is_ok());

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
    assert_eq!(other["epoch"], 2);
    let Err(OpError::Refused { code, .. }) = lab.ctx.link().expect(&key, 1) else { panic!("a plan made on the first project") };
    assert_eq!(code, "stale");
    let Err(OpError::Refused { code, .. }) = lab.ctx.link().expect(other["project_key"].as_str().unwrap(), 1) else {
        panic!("a plan made at an earlier epoch")
    };
    assert_eq!(code, "stale");

    let focus_file = lab.state("focus.json");
    assert_eq!((focus_file["project"]["key"].clone(), focus_file["project"]["epoch"].clone()), (other["project_key"].clone(), json!(2)));
    let status = lab.state("status.json");
    assert_eq!((status["connected"].clone(), status["project"]["epoch"].clone()), (json!(true), json!(2)));
    let Err(OpError::Refused { code, .. }) = lab.call("connect", json!({"daw": "renoise"})).await else { panic!("another DAW") };
    assert_eq!(code, "other_daw");
    lab.ctx.link().close().await;
});

case!(serve_wont_connect_before_kumis_part_is_in_the_daw, async {
    let lab = Lab::new(DawId::Reaper, Daw::Reaper, None).await;
    let Err(OpError::Refused { code, reason, .. }) = lab.call("connect", json!({})).await else { panic!("connected with nothing set up") };
    assert_eq!(code, "not_set_up");
    assert!(reason.contains("REAPER"), "{reason}");
    assert_eq!(lab.call("status", json!({})).await.unwrap()["connected"], false);
});
