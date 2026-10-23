//! The host's tools that read the DAW (`read`, `snapshot`, `focus`, `focus_body`) against a scripted Renoise song: a golden
//! MCP transcript of a whole session, a pattern too big for one page, the frame bound, and what reaches the tool.
#[path = "../common/mod.rs"]
mod common;

use common::{golden, Session};
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::renoise::simulator::RenoiseScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};

const FRAME_BOUND: usize = 256 * 1024;

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    script: RenoiseScript,
    simulator: Simulator,
    next_id: u64,
}

impl Lab {
    /// A host with a Renoise running behind it; `full_lines` makes track 1 of pattern 1 that many full lines.
    async fn start(full_lines: Option<usize>) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Renoise);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = full_lines.map_or_else(RenoiseScript::new, RenoiseScript::with_full_pattern);
        let mut config = script.config(secret, "/lab/Renoise");
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let mut session = Session::start(Daw::Renoise, home.path());
        session.hide_port(simulator.address().port(), "<port>");
        Lab { _home: home, session, script, simulator, next_id: 100 }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next_id += 1;
        let id = self.next_id;
        self.session.tool_full(id, name, arguments, false).await
    }

    /// Every row of a kind, following the cursors; each page checked against the frame bound.
    async fn read_all(&mut self, arguments: Value) -> Vec<Value> {
        let mut rows = Vec::new();
        let mut arguments = arguments;
        loop {
            let (page, is_error) = self.tool("read", arguments.clone()).await;
            assert!(!is_error, "{page}");
            assert!(serde_json::to_string(&page).unwrap().len() <= FRAME_BOUND);
            rows.extend(page["items"].as_array().unwrap().iter().cloned());
            match page.get("nextCursor") {
                Some(cursor) => arguments["cursor"] = cursor.clone(),
                None => return rows,
            }
        }
    }
}

#[tokio::test]
async fn a_whole_session_of_reads_against_a_renoise_song() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let mut session = Session::start(Daw::Renoise, home.path());
    session.call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"kumi","version":"test"}}})).await;
    session.call(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    session.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;

    // Not set up, then set up with nothing running: each tool says so, as an error.
    let (answer, is_error) = session.tool_full(3, "read", json!({"kind": "track"}), false).await;
    assert!(is_error && answer["connected"] == false, "{answer}");
    let secret = ensure_secret(&paths.secret()).unwrap();
    for (id, tool, arguments) in [(4, "snapshot", json!({})), (5, "focus", json!({})), (6, "focus_body", json!({"kind": "pattern"}))] {
        let (answer, is_error) = session.tool_full(id, tool, arguments, false).await;
        assert!(
            is_error
                && answer["connected"] == false
                && answer["error"].as_str().unwrap().contains("no Renoise with Kumi's bridge is running"),
            "{answer}"
        );
    }

    // A song behind the bridge.
    let script = RenoiseScript::new();
    let mut config = script.config(secret, "/lab/Renoise");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    session.hide_port(simulator.address().port(), "<port>");
    let (first, _) = session.tool_full(7, "read", json!({"kind": "track", "limit": 2}), false).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let (second, _) = session.tool_full(8, "read", json!({"kind": "track", "limit": 2, "cursor": first["nextCursor"]}), false).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    session.tool(9, "read", json!({"kind": "line", "parent": "1:pattern:1", "track": 1})).await;
    session.tool(10, "read", json!({"ref": "1:track:3"})).await;
    session.tool(11, "snapshot", json!({})).await;
    session.tool(12, "focus", json!({})).await;
    session.tool(13, "focus_body", json!({"kind": "pattern"})).await;
    session.tool(14, "focus_body", json!({"kind": "pattern", "window": {"from": 3, "to": 6}})).await;
    session.tool(15, "focus_body", json!({"kind": "matrix"})).await;
    session.tool(16, "focus_body", json!({"kind": "dsp", "ref": "1:track:1", "limit": 10})).await;

    // What the host refuses before it asks, and what the tool refuses.
    for (id, tool, arguments) in [
        (17, "read", json!({})),
        (18, "read", json!({"kind": "track", "limit": 0})),
        (19, "focus_body", json!({"kind": "pattern", "window": {"from": 4, "to": 4}})),
        (20, "focus_body", json!({})),
        (21, "read", json!({"kind": "nonsense"})),
        (22, "read", json!({"kind": "line", "track": 1})),
        (23, "focus_body", json!({"kind": "arrange"})),
    ] {
        let (answer, is_error) = session.tool_full(id, tool, arguments, false).await;
        assert!(is_error && answer["error"].is_string(), "{answer}");
    }
    // A ref or cursor from an earlier epoch is refused in the tool's words and code.
    let (answer, is_error) = session.tool_full(24, "read", json!({"ref": "0:track:1"}), false).await;
    assert!(is_error && answer["code"] == "stale" && answer["error"].as_str().unwrap().contains("earlier epoch"), "{answer}");
    let (answer, is_error) = session.tool_full(25, "read", json!({"kind": "track", "cursor": "0.2"}), false).await;
    assert!(is_error && answer["code"] == "stale", "{answer}");
    // And Renoise quitting is said.
    simulator.stop();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let (answer, is_error) = session.tool_full(26, "snapshot", json!({}), false).await;
    assert!(is_error && answer["connected"] == false, "{answer}");

    golden("renoise-tools-mcp.ndjson", &session.finish().await);
}

#[tokio::test]
async fn a_pattern_too_big_for_one_page_is_read_to_the_end_inside_the_frame_bound() {
    // 300 lines of twelve note columns and eight effect columns, all full: about 480 KB, read twice, which the bridge
    // sends only as the host gives credit back.
    let mut lab = Lab::start(Some(300)).await;
    let mut rows = Vec::new();
    for _ in 0..2 {
        rows = lab.read_all(json!({"kind": "line", "parent": "1:pattern:1", "track": 1, "limit": 200})).await;
        assert_eq!(rows.len(), 300);
    }
    let lines: Vec<u64> = rows.iter().map(|row| row["line"].as_u64().unwrap()).collect();
    assert_eq!(lines, (1..=300).collect::<Vec<u64>>());
    let pages = lab.script.asked().iter().filter(|(method, _)| method == "read").count();
    assert!(pages >= 4, "{pages} pages for two reads");
}

#[tokio::test]
async fn a_page_a_bridge_made_too_big_is_asked_for_again_with_fewer_rows() {
    // Lines of 1.6 KB: 200 of them are 320 KB, which the simulator, like a bridge that doesn't know the bound, sends.
    let mut lab = Lab::start(Some(300)).await;
    lab.script.ignore_the_frame_bound();
    let (page, is_error) = lab.tool("read", json!({"kind": "line", "parent": "1:pattern:1", "track": 1, "limit": 200})).await;
    assert!(!is_error);
    assert_eq!(page["items"].as_array().unwrap().len(), 100);
    assert_eq!(page["nextCursor"], "1.0.101");
    let limits: Vec<Option<u64>> = lab.script.asked().iter().map(|(_, params)| params["limit"].as_u64()).collect();
    assert_eq!(limits, [Some(200), Some(100)]);
}

#[tokio::test]
async fn only_known_arguments_reach_the_tool_and_a_window_is_a_range() {
    let mut lab = Lab::start(None).await;
    // More rows than a page holds is refused before the bridge is asked, as the bridges refuse it.
    let (refused, is_error) = lab.tool("read", json!({"kind": "track", "limit": 5000})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("from 1 to 200"), "{refused}");
    assert!(lab.script.asked().is_empty());
    lab.tool("read", json!({"kind": "track", "limit": 200, "pid": std::process::id(), "mood": "loud"})).await;
    lab.tool("focus_body", json!({"kind": "pattern", "window": {"from": 3, "to": 6}, "extra": 1})).await;
    lab.tool("read", json!({"ref": "1:track:3", "kind": "ignored"})).await;
    let asked = lab.script.asked();
    assert_eq!(asked[0], ("read".to_string(), json!({"kind": "track", "limit": 200})));
    assert_eq!(asked[1], ("focus_body".to_string(), json!({"kind": "pattern", "window": {"from": 3.0, "to": 6.0}})));
    assert_eq!(asked[2], ("read".to_string(), json!({"kind": "ignored", "ref": "1:track:3"})));
}

#[tokio::test]
async fn a_renoise_behind_a_dialog_is_said_busy_and_a_quit_one_is_gone() {
    let mut lab = Lab::start(None).await;
    assert!(!lab.tool("focus", json!({})).await.1);
    lab.simulator.set_busy(Some("rendering"));
    // The focus still answers, saying why it may be stale; the work is refused.
    let (focus, is_error) = lab.tool("focus", json!({})).await;
    assert!(!is_error && focus["busy"] == "rendering", "{focus}");
    for (tool, arguments) in [("read", json!({"kind": "track"})), ("snapshot", json!({})), ("focus_body", json!({"kind": "matrix"}))] {
        let (answer, is_error) = lab.tool(tool, arguments).await;
        assert!(is_error, "{tool}");
        assert_eq!(
            (&answer["busy"], &answer["code"], &answer["connected"]),
            (&json!(true), &json!("busy"), &json!(true)),
            "{tool}: {answer}"
        );
    }
    lab.simulator.set_busy(None);
    assert!(!lab.tool("focus", json!({})).await.1);
    lab.simulator.stop();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let (answer, is_error) = lab.tool("read", json!({"kind": "track"})).await;
    assert!(is_error && answer["connected"] == false, "{answer}");
}

#[tokio::test]
async fn renoises_actions_are_its_own_and_another_daws_host_refuses_them() {
    // (M4) The Renoise host lists audition_note and select; REAPER's has neither, and answers so if asked.
    let mut lab = Lab::start(None).await;
    let listed = lab.session.call_as(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {}}), false).await;
    let names: Vec<&str> = listed["result"]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
    for name in ["audition_note", "select"] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    let home = tempfile::tempdir().unwrap();
    let mut reaper = Session::start(Daw::Reaper, home.path());
    let listed = reaper.call_as(json!({"jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": {}}), false).await;
    assert!(!listed["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "audition_note"));
    let answer = reaper
        .call(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "audition_note", "arguments": {"instrumentRef": "1:instrument:1"}}}))
        .await;
    assert_eq!(answer["error"]["message"], "Unknown tool: audition_note", "REAPER's host has no Renoise action: {answer}");
}

#[tokio::test]
async fn an_instrument_saves_through_renoises_host_only() {
    // (M4, n-instruments) The Renoise host lists instrument_save; REAPER's doesn't, and answers so if asked.
    let mut lab = Lab::start(None).await;
    let listed = lab.session.call_as(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {}}), false).await;
    let save = listed["result"]["tools"].as_array().unwrap().iter().find(|tool| tool["name"] == "instrument_save").cloned();
    let save = save.expect("instrument_save is listed");
    assert_eq!(save["inputSchema"]["required"], json!(["path"]));
    assert!(save["inputSchema"]["properties"].get("overwrite").is_some(), "{save}");
    let home = tempfile::tempdir().unwrap();
    let mut reaper = Session::start(Daw::Reaper, home.path());
    let listed = reaper.call_as(json!({"jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": {}}), false).await;
    assert!(!listed["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "instrument_save"));
    let answer = reaper
        .call(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "instrument_save", "arguments": {"instrumentRef": "1:instrument:1", "path": "/tmp/a.xrni"}}}))
        .await;
    assert_eq!(answer["error"]["message"], "Unknown tool: instrument_save", "{answer}");
}
