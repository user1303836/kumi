//! Logic's bridge is Kumi's own process, started by the host when no discovery file answers: here through a spawner
//! that starts a simulator in its place (writing the discovery file as the bridge does), and through the host's own
//! spawner against what it finds installed.
mod common;

use common::Session;
use daw_bridge::files::{ensure_secret, scan_discovery, write_discovery};
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::{Host, SpawnFuture, SpawnedBridge, Spawner};
use daw_mcp_server::simulator::logic::LogicScript;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BUNDLE: &str = "com.apple.logic10";

/// A simulator standing in for the started bridge: stopped (its discovery file removed) when the host stops it.
struct SimBridge {
    simulator: Option<Simulator>,
    /// How it ended, once it has.
    exit: Option<String>,
}

impl SpawnedBridge for SimBridge {
    fn exited(&mut self) -> Option<String> {
        self.exit.clone()
    }

    fn stop(&mut self) {
        if let Some(simulator) = self.simulator.take() {
            simulator.stop();
        }
        self.exit.get_or_insert_with(|| "exit status: 0".into());
    }

    fn end(&mut self) {
        self.stop();
    }
}

/// What the test's spawner did: how many times it was asked, and the homes it was given.
#[derive(Default)]
struct Spawns {
    count: AtomicUsize,
    homes: std::sync::Mutex<Vec<PathBuf>>,
}

/// A spawner that starts a simulated Logic bridge, serving `script`, with its discovery file where the host looks.
fn simulated(script: LogicScript, spawns: Arc<Spawns>) -> Spawner {
    Box::new(move |home: PathBuf| -> SpawnFuture {
        let script = script.clone();
        let spawns = spawns.clone();
        Box::pin(async move {
            spawns.count.fetch_add(1, Ordering::SeqCst);
            spawns.homes.lock().unwrap().push(home.clone());
            let paths = BridgePaths::new(&home, Daw::Logic);
            let secret = daw_bridge::files::read_secret(&paths.secret()).map_err(|error| error.to_string())?;
            let mut config = script.config(secret, BUNDLE);
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.map_err(|error| error.to_string())?;
            Ok(Box::new(SimBridge { simulator: Some(simulator), exit: None }) as Box<dyn SpawnedBridge>)
        })
    })
}

fn logic_files(home: &Path) -> Vec<PathBuf> {
    scan_discovery(&BridgePaths::new(home, Daw::Logic).discovery(), Daw::Logic).into_iter().map(|found| found.path).collect()
}

#[tokio::test]
async fn with_no_discovery_file_the_host_starts_the_bridge_and_stops_it_when_it_closes() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Logic);
    ensure_secret(&paths.secret()).unwrap();
    let spawns = Arc::new(Spawns::default());
    let script = LogicScript::new();
    let host = Host::new(Daw::Logic, home.path()).with_spawner(simulated(script.clone(), spawns.clone()));
    let mut session = Session::start_with(Arc::new(host), home.path());
    assert!(logic_files(home.path()).is_empty());

    let status = session.tool(1, "status", json!({})).await;
    assert_eq!(
        (status["connected"].clone(), status["resource"].clone(), status["dawVersion"].clone()),
        (json!(true), json!(BUNDLE), json!("12.3.1"))
    );
    assert_eq!(status["permission"], serde_json::Value::Null);
    assert_eq!(spawns.count.load(Ordering::SeqCst), 1);
    assert_eq!(spawns.homes.lock().unwrap()[0], home.path());
    assert_eq!(logic_files(home.path()).len(), 1, "the started bridge wrote its discovery file");
    // The connection, and the bridge, are kept for the next calls.
    let (tracks, is_error) = session.tool_full(2, "read", json!({"kind": "track"}), false).await;
    assert!(!is_error && tracks["total"] == 2, "{tracks}");
    assert_eq!(session.tool(3, "focus", json!({})).await["epoch"], "1");
    assert_eq!(spawns.count.load(Ordering::SeqCst), 1);
    assert_eq!(script.asked().len(), 2);

    // The session ends: the host stops the bridge it started, which removes its file.
    session.finish().await;
    assert!(logic_files(home.path()).is_empty(), "the bridge was stopped");
}

#[tokio::test]
async fn a_refused_file_of_a_bridge_the_host_starts_is_removed_and_the_bridge_started_again() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Logic);
    ensure_secret(&paths.secret()).unwrap();
    // A file left by a bridge that is gone while Logic (this process, here) runs on: its port answers nothing.
    let port = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap().local_addr().unwrap().port();
    let mut stale = SimulatorConfig::logic(daw_bridge::auth::Secret::new("x".repeat(40)).unwrap(), "com.apple.mobilelogic");
    stale.identity.bridge = "0.0.9".into();
    let stale = daw_bridge::files::Discovery {
        schema: daw_bridge::files::DISCOVERY_SCHEMA.into(),
        daw: "logic".into(),
        pid: std::process::id(),
        host: "127.0.0.1".into(),
        port,
        protocol: daw_bridge::wire::PROTOCOL.into(),
        version: stale.identity.bridge,
        daw_version: stale.identity.daw_version,
        resource: stale.identity.resource,
        project: None,
        started_ms: 1,
    };
    let stale_file = write_discovery(&paths.discovery(), &stale).unwrap();
    let spawns = Arc::new(Spawns::default());
    let host = Host::new(Daw::Logic, home.path()).with_spawner(simulated(LogicScript::new(), spawns.clone()));
    let mut session = Session::start_with(Arc::new(host), home.path());
    let status = session.tool(1, "status", json!({})).await;
    assert_eq!((status["connected"].clone(), status["bridge"].clone()), (json!(true), json!("0.1.0")), "{status}");
    assert!(!stale_file.exists(), "the refused file is gone");
    assert_eq!(spawns.count.load(Ordering::SeqCst), 1);
    assert_eq!(logic_files(home.path()).len(), 1);
    session.finish().await;
}

#[tokio::test]
async fn a_bridge_that_ends_before_it_serves_is_said_with_its_status_and_logic_to_open() {
    let home = tempfile::tempdir().unwrap();
    ensure_secret(&BridgePaths::new(home.path(), Daw::Logic).secret()).unwrap();
    let ended: Spawner = Box::new(|_| {
        Box::pin(async { Ok(Box::new(SimBridge { simulator: None, exit: Some("exit status: 2".into()) }) as Box<dyn SpawnedBridge>) })
    });
    let host = Host::new(Daw::Logic, home.path()).with_spawner(ended);
    let mut session = Session::start_with(Arc::new(host), home.path());
    let status = session.tool(1, "status", json!({})).await;
    assert_eq!(status["connected"], false);
    let reason = status["reason"].as_str().unwrap();
    assert!(reason.contains("ended before it served (exit status: 2)") && reason.contains("open Logic Pro"), "{reason}");
    // And a tool's error says the same.
    let (answer, is_error) = session.tool_full(2, "read", json!({"kind": "track"}), false).await;
    assert!(is_error && answer["error"].as_str().unwrap().contains("open Logic Pro"), "{answer}");
    session.finish().await;
}

#[tokio::test]
async fn a_bridge_that_never_serves_is_stopped_after_the_wait() {
    let home = tempfile::tempdir().unwrap();
    ensure_secret(&BridgePaths::new(home.path(), Daw::Logic).secret()).unwrap();
    let stopped = Arc::new(AtomicUsize::new(0));
    let counted = stopped.clone();
    let silent: Spawner = Box::new(move |_| {
        let counted = counted.clone();
        Box::pin(async move {
            struct Silent(Arc<AtomicUsize>, bool);
            impl SpawnedBridge for Silent {
                fn exited(&mut self) -> Option<String> {
                    self.1.then(|| "signal: 15".to_string())
                }
                fn stop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    self.1 = true;
                }
                fn end(&mut self) {
                    self.stop()
                }
            }
            Ok(Box::new(Silent(counted, false)) as Box<dyn SpawnedBridge>)
        })
    });
    let host = Host::new(Daw::Logic, home.path()).with_spawner(silent).with_spawn_wait(Duration::from_millis(300));
    let mut session = Session::start_with(Arc::new(host), home.path());
    let status = session.tool(1, "status", json!({})).await;
    let reason = status["reason"].as_str().unwrap();
    assert!(reason.contains("didn't start serving within 0.3 s") && reason.contains("open Logic Pro"), "{reason}");
    assert_eq!(stopped.load(Ordering::SeqCst), 1, "the bridge that never served was stopped");
    session.finish().await;
}

#[tokio::test]
async fn without_the_bridge_installed_the_error_names_the_command_that_installs_it() {
    let home = tempfile::tempdir().unwrap();
    ensure_secret(&BridgePaths::new(home.path(), Daw::Logic).secret()).unwrap();
    let host = Host::new(Daw::Logic, home.path());
    let (answer, is_error) = host.read(&json!({"kind": "track"})).await;
    assert!(is_error, "{answer}");
    assert_eq!(answer["error"], "Kumi's Logic Pro bridge isn't installed: run `kumi --daw logic bridge`");
    assert_eq!(answer["connected"], false);
    // A binary named instead of the installed one has to be there.
    let host = Host::new(Daw::Logic, home.path()).with_bridge_binary(home.path().join("nowhere").join("logic-kumi"));
    let (answer, _) = host.snapshot(&json!({})).await;
    assert!(answer["error"].as_str().unwrap().contains("KUMI_LOGIC_BRIDGE names"), "{answer}");
    // discover never starts a bridge: it says what to do.
    let found = host.discover().await;
    assert_eq!(found["instances"], json!([]));
    assert!(found["hint"].as_str().unwrap().contains("open Logic Pro"), "{found}");
}

/// The host's own spawner runs the binary with `--kumi-home`: a script standing in for the bridge, which ends at once.
#[cfg(unix)]
#[tokio::test]
async fn the_bridge_binary_is_run_with_kumi_home_and_its_exit_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    ensure_secret(&BridgePaths::new(home.path(), Daw::Logic).secret()).unwrap();
    let script = home.path().join("logic-kumi");
    std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$2/args.txt\"\nexit 3\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let host = Host::new(Daw::Logic, home.path()).with_bridge_binary(script);
    let (answer, is_error) = host.focus(&json!({})).await;
    assert!(is_error, "{answer}");
    let error = answer["error"].as_str().unwrap();
    assert!(error.contains("ended before it served (exit status: 3)") && error.contains("open Logic Pro"), "{error}");
    let args = std::fs::read_to_string(home.path().join("args.txt")).unwrap();
    assert_eq!(args, format!("--kumi-home\n{}\n", home.path().display()));
    host.close().await;
}
