//! The host's tools that read the DAW (`read`, `snapshot`, `focus`, `focus_body`) against a Logic Pro project replayed
//! from the bridge's recorded answers: a golden MCP transcript of a whole session, the refs named the contract's way, a
//! Logic away from its Space, a bridge without Accessibility, and the words when Kumi's bridge for Logic isn't installed.
mod common;

use common::{golden, Session};
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::simulator::logic::LogicScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};

const BUNDLE: &str = "com.apple.logic10";

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    script: LogicScript,
    simulator: Simulator,
    next_id: u64,
}

impl Lab {
    /// A host with a Logic running behind it, its bridge answering already.
    async fn start() -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Logic);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = LogicScript::new();
        let mut config = script.config(secret, BUNDLE);
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let mut session = Session::start(Daw::Logic, home.path());
        session.hide_port(simulator.address().port(), "<port>");
        Lab { _home: home, session, script, simulator, next_id: 100 }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next_id += 1;
        let id = self.next_id;
        self.session.tool_full(id, name, arguments, false).await
    }
}

#[tokio::test]
async fn a_whole_session_of_reads_against_a_logic_project() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Logic);
    let mut session = Session::start(Daw::Logic, home.path());
    session.call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"kumi","version":"test"}}})).await;
    session.call(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    session.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;

    // Not set up, then set up with nothing running: the host would start the bridge, which isn't installed; each tool says so.
    let (answer, is_error) = session.tool_full(3, "read", json!({"kind": "track"}), false).await;
    assert!(is_error && answer["connected"] == false, "{answer}");
    let secret = ensure_secret(&paths.secret()).unwrap();
    for (id, tool, arguments) in [(4, "snapshot", json!({})), (5, "focus", json!({})), (6, "focus_body", json!({"kind": "timeline"}))] {
        let (answer, is_error) = session.tool_full(id, tool, arguments, false).await;
        assert!(
            is_error && answer["connected"] == false && answer["error"].as_str().unwrap().contains("kumi --daw logic bridge"),
            "{answer}"
        );
    }

    // A project behind the bridge.
    let script = LogicScript::new();
    let mut config = script.config(secret, BUNDLE);
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    session.hide_port(simulator.address().port(), "<port>");
    let status = session.tool(7, "status", json!({})).await;
    assert_eq!(
        (status["connected"].clone(), status["permission"].clone(), status["busy"].clone()),
        (json!(true), Value::Null, json!(false))
    );
    let (first, _) = session.tool_full(8, "read", json!({"kind": "track", "limit": 1}), false).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let (second, _) = session.tool_full(9, "read", json!({"kind": "track", "cursor": first["nextCursor"]}), false).await;
    assert_eq!((second["items"][0]["ref"].clone(), second.get("nextCursor").cloned()), (json!("1:track:2"), None));
    session.tool(10, "read", json!({"kind": "region", "parent": "1:track:2"})).await;
    session.tool(11, "read", json!({"kind": "note", "parent": "1:region:1"})).await;
    session.tool(12, "read", json!({"kind": "plugin", "parent": "1:track:1"})).await;
    session.tool(13, "read", json!({"ref": "1:track:2"})).await;
    session.tool(14, "snapshot", json!({})).await;
    session.tool(15, "focus", json!({})).await;
    session.tool(16, "focus_body", json!({"kind": "tree"})).await;
    session.tool(17, "focus_body", json!({"kind": "notes", "ref": "1:region:1"})).await;
    session.tool(18, "focus_body", json!({"kind": "timeline"})).await;
    let around = session
        .tool(19, "focus_body", json!({"kind": "timeline", "limit": 1, "around": "1:track:2", "window": {"from": 0, "to": 16}}))
        .await;
    assert_eq!((around["from"].clone(), around["trackCount"].clone()), (json!(1), json!(2)));
    session.tool(20, "focus_body", json!({"kind": "routing", "ref": "1:track:1"})).await;

    // What the host refuses before it asks, and what the bridge refuses.
    for (id, tool, arguments) in [
        (21, "read", json!({})),
        (22, "read", json!({"kind": "track", "limit": 0})),
        (23, "focus_body", json!({"kind": "timeline", "window": {"from": 4, "to": 4}})),
        (24, "focus_body", json!({})),
        (25, "read", json!({"kind": "bogus"})),
        (26, "read", json!({"kind": "note"})),
        (27, "focus_body", json!({"kind": "mixer"})),
    ] {
        let (answer, is_error) = session.tool_full(id, tool, arguments, false).await;
        assert!(is_error && answer["error"].is_string(), "{answer}");
    }
    // A ref from before a track changed is refused in the bridge's words and code.
    script.change_tracks();
    let (answer, is_error) = session.tool_full(28, "read", json!({"ref": "1:track:1"}), false).await;
    assert!(is_error && answer["code"] == "stale" && answer["error"].as_str().unwrap().contains("earlier epoch"), "{answer}");
    // Logic's windows on another Space: the focus says so, the reads are refused as busy.
    simulator.set_busy(Some("away"));
    let (focus, is_error) = session.tool_full(29, "focus", json!({}), false).await;
    assert!(!is_error && focus["busy"] == "away", "{focus}");
    let (answer, is_error) = session.tool_full(30, "read", json!({"kind": "track"}), false).await;
    assert!(is_error && answer["busy"] == true && answer["code"] == "busy", "{answer}");
    simulator.set_busy(None);
    // Without Accessibility: the status names it, and a read fails, never busy.
    simulator.set_permission(Some("accessibility"));
    assert_eq!(session.tool(31, "status", json!({})).await["permission"], "accessibility");
    let (answer, is_error) = session.tool_full(32, "read", json!({"kind": "track"}), false).await;
    assert!(is_error && answer["code"] == "failed" && answer["error"].as_str().unwrap().contains("Accessibility"), "{answer}");
    assert!(answer.get("busy").is_none(), "{answer}");
    simulator.set_permission(None);
    // And the bridge quitting is said.
    simulator.stop();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let (answer, is_error) = session.tool_full(33, "snapshot", json!({}), false).await;
    assert!(is_error && answer["connected"] == false, "{answer}");

    golden("logic-tools-mcp.ndjson", &session.finish().await);
}

#[tokio::test]
async fn every_answer_the_host_hands_over_names_refs_the_way_the_contract_says() {
    let mut lab = Lab::start().await;
    let mut answers = Vec::new();
    for (tool, arguments) in [
        ("read", json!({"kind": "project"})),
        ("read", json!({"kind": "track"})),
        ("read", json!({"kind": "region"})),
        ("read", json!({"kind": "note", "parent": "1:region:1"})),
        ("read", json!({"kind": "plugin", "parent": "1:track:1"})),
        ("read", json!({"kind": "send", "parent": "1:track:1"})),
        ("read", json!({"kind": "marker"})),
        ("read", json!({"kind": "tempo"})),
        ("read", json!({"ref": "1:track:2"})),
        ("snapshot", json!({})),
        ("focus", json!({})),
        ("focus_body", json!({"kind": "tree"})),
        ("focus_body", json!({"kind": "notes"})),
        ("focus_body", json!({"kind": "timeline"})),
        ("focus_body", json!({"kind": "routing", "ref": "1:track:1"})),
        // (M4b) the Stereo Out's plug-ins (none), a strip's plug-ins, a plug-in's parameters and one of them, Smart
        // Controls, automation
        ("read", json!({"kind": "plugin", "parent": "1:output:1"})),
        ("read", json!({"kind": "plugin", "parent": "1:track:2"})),
        ("read", json!({"kind": "param", "parent": "1:plugin:2.3"})),
        ("read", json!({"ref": "1:param:2.3.1"})),
        ("read", json!({"kind": "smart", "parent": "1:track:2"})),
        ("read", json!({"kind": "automation", "parent": "1:track:1"})),
        ("focus_body", json!({"kind": "tree", "ref": "1:track:2"})),
    ] {
        let (answer, is_error) = lab.tool(tool, arguments.clone()).await;
        assert!(!is_error, "{tool} {arguments}: {answer}");
        answers.push((tool, answer));
    }
    for (tool, answer) in &answers {
        let misplaced = daw_bridge::contract::misplaced_refs(answer);
        assert!(misplaced.is_empty(), "{tool}: {misplaced:?}");
    }
    assert_eq!(lab.script.asked().len(), answers.len());
}

#[tokio::test]
async fn only_known_arguments_reach_the_bridge() {
    let mut lab = Lab::start().await;
    let (refused, is_error) = lab.tool("read", json!({"kind": "track", "limit": 5000})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("from 1 to 200"), "{refused}");
    assert!(lab.script.asked().is_empty(), "refused before the bridge is asked");
    lab.tool("read", json!({"kind": "note", "parent": "1:region:1", "limit": 2, "pid": std::process::id(), "mood": "loud"})).await;
    lab.tool("focus_body", json!({"kind": "timeline", "window": {"from": 0, "to": 8}, "extra": 1})).await;
    let asked = lab.script.asked();
    assert_eq!(asked[0], ("read".to_string(), json!({"kind": "note", "parent": "1:region:1", "limit": 2})));
    assert_eq!(asked[1], ("focus_body".to_string(), json!({"kind": "timeline", "window": {"from": 0.0, "to": 8.0}})));
    lab.simulator.stop();
}
