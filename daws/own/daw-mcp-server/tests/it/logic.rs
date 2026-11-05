//! Logic M1 attaches beside an already running app and offers status only. Simulated future reads must not turn
//! into advertised product tools; only a bridge this host started is stopped when its session ends.
use daw_bridge::files::{ensure_secret, read_secret, scan_discovery};
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::{Host, Pick};
use daw_mcp_server::mcp;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use daw_mcp_server::spawn::{Spawn, SpawnedBridge};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Started {
    simulator: Option<Simulator>,
    stopped: Arc<AtomicUsize>,
}

impl SpawnedBridge for Started {
    fn exited(&mut self) -> Option<String> {
        self.simulator.is_none().then(|| "asked to end".into())
    }
    fn stop(&mut self) {
        if let Some(simulator) = self.simulator.take() {
            simulator.stop();
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn kill(&mut self) {
        self.stop();
    }
}

#[tokio::test]
async fn an_existing_logic_bridge_attaches_without_starting_or_stopping_another_and_exposes_only_status() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Logic);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let asked = reads.clone();
    // These fake future operations deliberately exist in the stand-in. M1's host must still expose only what its
    // real bridge implements, rather than confusing a richer simulator with a shipping Logic capability.
    let mut config = SimulatorConfig::logic(secret, "com.apple.mobilelogic").with_methods(
        &["read", "snapshot", "focus", "focus_body", "change_preview", "listen_render"],
        Arc::new(move |_, _| {
            asked.fetch_add(1, Ordering::SeqCst);
            Some(Ok(json!({"items":[]})))
        }),
    );
    config.status_extra.insert("permission".into(), json!("accessibility"));
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let host = Host::new(Daw::Logic, home.path()).with_spawn(Spawn::new(
        || panic!("a serving Logic bridge never needs a process-launch check"),
        || panic!("a serving Logic bridge is attached to, never started again"),
    ));
    let status = host.status(&Pick::default()).await;
    assert_eq!(status["connected"], true, "{status}");
    assert_eq!(status["resource"], "com.apple.mobilelogic");
    assert_eq!(status["dawVersion"], "12.3.1");
    assert_eq!(status["permission"], "accessibility", "the missing permission isn't hidden by a connected socket");
    assert!(status["listening"].is_null());
    let discovered = host.discover().await;
    assert_eq!(discovered["instances"][0]["permission"], "accessibility");
    let tools = mcp::tools(&host);
    assert_eq!(tools.as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect::<Vec<_>>(), ["status", "discover"]);
    assert!(tools[0]["description"].as_str().unwrap().contains("bridge beside Logic Pro"));
    for name in ["read", "snapshot", "focus", "focus_body", "change_preview", "listen_render", "scratch", "project_save"] {
        let answer =
            mcp::handle(&host, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":{}}})).await.unwrap();
        assert_eq!(answer["error"]["code"], -32602, "{name}: {answer}");
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0, "unimplemented product tools never reach a simulator's extra methods");
    host.close().await;
    assert_eq!(scan_discovery(&paths.discovery(), Daw::Logic).len(), 1, "an attached external bridge stays with its owner");
    simulator.stop();
}

#[tokio::test]
async fn logic_status_calls_share_one_started_bridge_and_closing_removes_its_discovery_file() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Logic);
    ensure_secret(&paths.secret()).unwrap();
    let (started, stopped) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (starts, stops) = (started.clone(), stopped.clone());
    let spawn_paths = paths.clone();
    let spawn = Spawn::new(
        || true,
        move || {
            let (paths, starts, stops) = (spawn_paths.clone(), starts.clone(), stops.clone());
            Box::pin(async move {
                starts.fetch_add(1, Ordering::SeqCst);
                let mut config = SimulatorConfig::logic(read_secret(&paths.secret()).unwrap(), "com.apple.logic10");
                config.discovery_dir = Some(paths.discovery());
                let simulator = Simulator::start(config).await.map_err(|error| error.to_string())?;
                Ok(Box::new(Started { simulator: Some(simulator), stopped: stops }) as Box<dyn SpawnedBridge>)
            })
        },
    );
    let host = Host::new(Daw::Logic, home.path()).with_spawn(spawn);
    let pick = Pick::default();
    let (one, two) = tokio::join!(host.status(&pick), host.status(&pick));
    for status in [one, two] {
        assert_eq!(status["connected"], true, "{status}");
        assert_eq!(status["resource"], "com.apple.logic10");
        assert_eq!(status.get("permission"), Some(&Value::Null));
    }
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(scan_discovery(&paths.discovery(), Daw::Logic).len(), 1);
    host.close().await;
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert!(scan_discovery(&paths.discovery(), Daw::Logic).is_empty());
}
