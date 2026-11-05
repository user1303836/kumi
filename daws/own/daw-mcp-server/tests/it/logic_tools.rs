//! The current MCP host over Logic's original M2 recording: bodies, short-ref keys, paging, refusals and arguments.
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::{Host, Pick};
use daw_mcp_server::mcp;
use daw_mcp_server::simulator::logic::LogicScript;
use daw_mcp_server::simulator::Simulator;
use daw_mcp_server::spawn::Spawn;
use serde_json::{json, Value};
use std::sync::Arc;

struct Lab {
    _home: tempfile::TempDir,
    host: Arc<Host>,
    script: LogicScript,
    simulator: Simulator,
}

impl Lab {
    async fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Logic);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = LogicScript::new();
        let mut config = script.config(secret, "com.apple.logic10");
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let host = Arc::new(Host::new(Daw::Logic, home.path()).with_spawn(Spawn::new(
            || panic!("the existing bridge is attached to"),
            || panic!("the existing bridge is never launched again"),
        )));
        Self { _home: home, host, script, simulator }
    }

    async fn tool(&self, name: &str, arguments: Value) -> (Value, bool) {
        let answer =
            mcp::handle(&self.host, &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name,"arguments":arguments}}))
                .await
                .unwrap();
        assert!(answer.get("error").is_none(), "{answer}");
        (answer["result"]["structuredContent"].clone(), answer["result"]["isError"] == true)
    }
}

#[tokio::test]
async fn every_logic_answer_names_refs_the_way_the_contract_says() {
    let lab = Lab::start().await;
    for (tool, arguments) in [
        ("read", json!({"kind":"project"})),
        ("read", json!({"kind":"track"})),
        ("read", json!({"kind":"region"})),
        ("read", json!({"kind":"note","parent":"1:region:1"})),
        ("read", json!({"kind":"plugin","parent":"1:track:1"})),
        ("read", json!({"kind":"send","parent":"1:track:1"})),
        ("read", json!({"kind":"marker"})),
        ("read", json!({"kind":"tempo"})),
        ("read", json!({"ref":"1:track:2"})),
        ("snapshot", json!({})),
        ("focus", json!({})),
        ("focus_body", json!({"kind":"tree"})),
        ("focus_body", json!({"kind":"notes"})),
        ("focus_body", json!({"kind":"timeline"})),
        ("focus_body", json!({"kind":"routing","ref":"1:track:1"})),
    ] {
        let (answer, is_error) = lab.tool(tool, arguments.clone()).await;
        assert!(!is_error, "{tool} {arguments}: {answer}");
        let misplaced = daw_bridge::contract::misplaced_refs(&answer);
        assert!(misplaced.is_empty(), "{tool}: {misplaced:?}");
    }
    assert_eq!(lab.script.asked().len(), 15);
    lab.host.close().await;
}

#[tokio::test]
async fn logic_pages_and_views_keep_the_bridges_stale_and_permission_refusals() {
    let lab = Lab::start().await;
    let (first, error) = lab.tool("read", json!({"kind":"track","limit":1})).await;
    assert!(!error, "{first}");
    let (second, error) = lab.tool("read", json!({"kind":"track","cursor":first["nextCursor"]})).await;
    assert!(!error && second["items"][0]["ref"] == "1:track:2" && second.get("nextCursor").is_none(), "{second}");
    let (around, error) =
        lab.tool("focus_body", json!({"kind":"timeline","around":"1:track:2","window":{"from":0,"to":16},"limit":1})).await;
    assert!(!error && around["from"] == 1 && around["trackCount"] == 2, "{around}");
    // Notes need their region; a deleted object is a different refusal from a stale ref.
    for (arguments, code) in [(json!({"kind":"note"}), "bad-params"), (json!({"ref":"1:track:9"}), "failed")] {
        let (answer, error) = lab.tool("read", arguments).await;
        assert!(error && answer["code"] == code, "{answer}");
    }
    lab.script.change_tracks();
    for (tool, arguments) in [
        ("read", json!({"ref":"1:track:1"})),
        ("read", json!({"kind":"track","cursor":first["nextCursor"]})),
        ("focus_body", json!({"kind":"routing","ref":"1:track:1"})),
    ] {
        let (answer, error) = lab.tool(tool, arguments).await;
        assert!(error && answer["code"] == "stale" && answer["error"].as_str().unwrap().contains("earlier epoch"), "{answer}");
    }
    let (again, error) = lab.tool("read", json!({"kind":"track","limit":1})).await;
    assert!(!error && again["items"][0]["ref"] == "2:track:1", "{again}");
    lab.simulator.set_busy(Some("away"));
    let (focus, error) = lab.tool("focus", json!({})).await;
    assert!(!error && focus["busy"] == "away", "{focus}");
    let (read, error) = lab.tool("read", json!({"kind":"track"})).await;
    assert!(error && read["code"] == "busy" && read["busy"] == true, "{read}");
    lab.simulator.set_permission(Some("accessibility"));
    assert_eq!(lab.tool("status", json!({})).await.0["permission"], "accessibility");
    for (tool, arguments) in
        [("read", json!({"kind":"track"})), ("snapshot", json!({})), ("focus", json!({})), ("focus_body", json!({"kind":"timeline"}))]
    {
        let (answer, error) = lab.tool(tool, arguments).await;
        assert!(error && answer["code"] == "failed" && answer["error"].as_str().unwrap().contains("Accessibility"), "{answer}");
        assert!(answer.get("busy").is_none(), "a grant is needed, not a busy retry: {answer}");
    }
    lab.host.close().await;
}

#[tokio::test]
async fn only_known_logic_arguments_reach_the_bridge() {
    let lab = Lab::start().await;
    for (tool, arguments) in [
        ("read", json!({})),
        ("read", json!({"kind":"track","limit":0})),
        ("read", json!({"kind":"track","limit":5000})),
        ("read", json!({"kind":"track","cursor":[]})),
        ("read", json!({"kind":"track","pid":0})),
        ("read", json!({"kind":"param"})),
        ("read", json!({"kind":"automation"})),
        ("focus_body", json!({})),
        ("focus_body", json!({"kind":"mixer"})),
        ("focus_body", json!({"kind":"timeline","around":5})),
        ("focus_body", json!({"kind":"timeline","window":{"from":4,"to":4}})),
    ] {
        let (answer, error) = lab.tool(tool, arguments).await;
        assert!(error && answer["error"].is_string(), "{answer}");
    }
    assert!(lab.script.asked().is_empty(), "invalid arguments are refused before dispatch");
    let (answer, error) = lab.tool("read", json!({"kind":"track","limit":1,"pid":std::process::id(),"mood":"loud"})).await;
    assert!(!error, "{answer}");
    let (answer, error) = lab.tool("focus_body", json!({"kind":"timeline","window":{"from":0,"to":8},"extra":1})).await;
    assert!(!error, "{answer}");
    assert_eq!(
        lab.script.asked(),
        [
            ("read".into(), json!({"kind":"track","limit":1})),
            ("focus_body".into(), json!({"kind":"timeline","window":{"from":0.0,"to":8.0}})),
        ]
    );
    lab.host.close().await;
}

#[tokio::test]
async fn a_logic_read_waiting_for_native_work_does_not_hold_up_status_or_focus() {
    let lab = Lab::start().await;
    assert_eq!(lab.host.status(&Pick::default()).await["connected"], true);
    lab.simulator.hold("read");
    let host = lab.host.clone();
    let read = tokio::spawn(async move { host.read(&json!({"kind":"track"})).await });
    loop {
        if lab
            .simulator
            .transcript()
            .iter()
            .any(|line| line.from_host && serde_json::from_str::<Value>(&line.text).ok().is_some_and(|frame| frame["method"] == "read"))
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(!read.is_finished());
    assert_eq!(lab.tool("status", json!({})).await.0["connected"], true);
    let (focus, error) = lab.tool("focus", json!({})).await;
    assert!(!error && focus["epoch"] == "1", "{focus}");
    assert_eq!(lab.host.call("ping", json!({}), &Pick::default(), std::time::Duration::from_secs(5)).await.unwrap(), json!({}));
    assert!(!read.is_finished(), "the other answers arrived while the read was still waiting");
    // End the simulated native peer: the pending call is observed and joined, never left to a timeout.
    lab.simulator.stop();
    assert!(read.await.unwrap().1);
    lab.host.close().await;
}
