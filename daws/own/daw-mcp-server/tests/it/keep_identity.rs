//! The hello's `keepIdentity`: a host made to keep the project's id (standalone Kumi's, `KUMI_KEEP_IDENTITY=1`) asks on its
//! connections and another (the plugin's runtime's) never does; the simulated bridges write the id into their project
//! only while a connection that asked is open, and say in `status` that they work that way.
use daw_bridge::auth::Secret;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::client::BridgeClient;
use daw_mcp_server::host::{Host, ListeningMode, Pick};
use daw_mcp_server::reaper::simulator::ReaperScript;
use daw_mcp_server::renoise::simulator::RenoiseScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// What each hello the simulator was sent said of `keepIdentity`, in order.
fn hellos(simulator: &Simulator) -> Vec<Option<Value>> {
    simulator
        .transcript()
        .into_iter()
        .filter(|line| line.from_host)
        .filter_map(|line| serde_json::from_str::<Value>(&line.text).ok())
        .filter(|frame| frame["type"] == "hello")
        .map(|frame| frame.get("keepIdentity").cloned())
        .collect()
}

/// Whether the simulator comes to `want` (a closed connection ends on the bridge's side a moment later).
async fn settles(simulator: &Simulator, want: bool) -> bool {
    let until = tokio::time::Instant::now() + WAIT;
    while simulator.keeps_identity() != want && tokio::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    simulator.keeps_identity() == want
}

#[tokio::test]
async fn reaper_has_its_projects_id_written_only_while_a_host_made_to_keep_it_is_connected() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Reaper);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let script = ReaperScript::new(2, 0);
    let mut config = script.config(secret, "/lab/REAPER");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();

    // The plugin's runtime's host: no ask, nothing written, and the status says what the bridge can do.
    let plain = Host::new(Daw::Reaper, home.path(), ListeningMode::Restricted);
    let status = plain.status(&Pick::default()).await;
    assert_eq!((&status["connected"], &status["keep_identity"]), (&json!(true), &json!(true)), "{status}");
    assert!(!simulator.keeps_identity());
    assert_eq!(script.ids_written(), Vec::<String>::new());

    // Standalone Kumi's host asks: the project has its id written while it is connected.
    let keeping = Host::new(Daw::Reaper, home.path(), ListeningMode::Standalone).with_keep_identity(true);
    assert_eq!(keeping.status(&Pick::default()).await["connected"], true);
    assert!(simulator.keeps_identity());
    assert_eq!(script.ids_written(), ["/lab/Song.rpp"]);
    assert_eq!(hellos(&simulator), [None, Some(json!(true))]);

    // It goes: the plain host, still connected, asks nothing, and nothing more is written.
    keeping.close().await;
    assert!(settles(&simulator, false).await, "the asking host has gone");
    script.set_project(None, "/lab/Other.rpp", "sim:other", "Other.rpp");
    assert_eq!(plain.status(&Pick::default()).await["connected"], true);
    assert_eq!(script.ids_written(), ["/lab/Song.rpp"]);
    plain.close().await;
}

#[tokio::test]
async fn renoise_has_its_songs_id_written_as_it_is_saved_only_while_a_connection_that_asked_is_open() {
    let secret = Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap();
    let script = RenoiseScript::new();
    let simulator = Simulator::start(script.config_with_changes(secret.clone(), "/lab/Renoise")).await.unwrap();
    let save = |client: BridgeClient| async move { client.request("project_save", json!({"epoch": "1"}), WAIT).await.unwrap() };

    let plain = BridgeClient::connect_within(simulator.address(), secret.clone(), "test", WAIT).await.unwrap();
    assert_eq!(plain.request("status", json!({}), WAIT).await.unwrap()["keep_identity"], true);
    save(plain.clone()).await;
    assert_eq!(script.ids_written(), Vec::<String>::new(), "a host that didn't ask");

    let asking = BridgeClient::connect_asking(simulator.address(), secret.clone(), "test", true, WAIT).await.unwrap();
    save(asking.clone()).await;
    assert_eq!(script.ids_written(), ["/lab/Renoise/Sim.xrns"]);

    asking.close().await;
    assert!(settles(&simulator, false).await, "the asking host has gone");
    save(plain.clone()).await;
    assert_eq!(script.ids_written(), ["/lab/Renoise/Sim.xrns"], "nothing more once it went");
    plain.close().await;
}
