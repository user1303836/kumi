//! An answer past the frame bound that asking for fewer rows doesn't make smaller is refused after the second try, not the fifth.
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn a_body_that_ignores_the_limit_is_asked_twice_not_five_times() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let asked = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = asked.clone();
    let respond: daw_mcp_server::simulator::Respond = Arc::new(move |_, params| {
        log.lock().unwrap().push(params.clone());
        Some(Ok(json!({"epoch": "1", "blob": "z".repeat(300 * 1024)})))
    });
    let mut config = SimulatorConfig::renoise(secret, "/lab/Renoise").with_methods(&["focus_body"], respond);
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let host = Host::new(Daw::Renoise, home.path());
    let (answer, is_error) = host.focus_body(&json!({"kind": "dsp", "ref": "1:track:1"})).await;
    assert!(is_error, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("answered more than"), "{answer}");
    assert_eq!(asked.lock().unwrap().len(), 2, "asked again once with fewer rows, and no more");
    simulator.stop();
}
