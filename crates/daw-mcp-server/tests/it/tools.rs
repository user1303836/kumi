//! The host's tools that read the DAW (`read`, `snapshot`, `focus`, `focus_body`) against a scripted REAPER project:
//! a golden MCP transcript of a whole session, paging, the frame bound, a busy DAW, a gone one, and a cursor from
//! before the tab changed.
mod common;

use common::{golden, Session};
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::simulator::reaper::ReaperScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};

const FRAME_BOUND: usize = 256 * 1024;

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    script: ReaperScript,
    simulator: Simulator,
    next_id: u64,
}

impl Lab {
    /// A host with a REAPER running behind it: `tracks` tracks, names `name_len` characters long.
    async fn start(tracks: usize, name_len: usize) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Reaper);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = ReaperScript::new(tracks, name_len);
        let mut config = script.config(secret, "/lab/REAPER");
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let mut session = Session::start(Daw::Reaper, home.path());
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
async fn a_whole_session_of_reads_against_a_reaper_project() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Reaper);
    let mut session = Session::start(Daw::Reaper, home.path());
    session.call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"kumi","version":"test"}}})).await;
    session.call(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    session.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;

    // Not set up, then set up with nothing running: each tool says so, as an error.
    let (answer, is_error) = session.tool_full(3, "read", json!({"kind": "track"}), false).await;
    assert!(is_error && answer["connected"] == false, "{answer}");
    let secret = ensure_secret(&paths.secret()).unwrap();
    for (id, tool, arguments) in [(4, "snapshot", json!({})), (5, "focus", json!({})), (6, "focus_body", json!({"kind": "arrange"}))] {
        let (answer, is_error) = session.tool_full(id, tool, arguments, false).await;
        assert!(
            is_error
                && answer["connected"] == false
                && answer["error"].as_str().unwrap().contains("no REAPER with Kumi's bridge is running"),
            "{answer}"
        );
    }

    // A project with three tracks behind the bridge.
    let script = ReaperScript::new(3, 0);
    let mut config = script.config(secret, "/lab/REAPER");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    session.hide_port(simulator.address().port(), "<port>");
    let (first, _) = session.tool_full(7, "read", json!({"kind": "track", "limit": 2}), false).await.into_pair();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let (second, _) =
        session.tool_full(8, "read", json!({"kind": "track", "limit": 2, "cursor": first["nextCursor"]}), false).await.into_pair();
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second.get("nextCursor"), None);
    session.tool(9, "read", json!({"kind": "item", "parent": "r:track:{T0002}"})).await;
    session.tool(10, "read", json!({"ref": "r:track:{T0001}"})).await;
    session.tool(11, "snapshot", json!({})).await;
    session.tool(12, "focus", json!({})).await;
    session.tool(13, "focus_body", json!({"kind": "arrange", "window": {"from": 0, "to": 32}})).await;
    session.tool(14, "focus_body", json!({"kind": "midi", "ref": "r:take:{T-1}", "limit": 10})).await;
    // The arrange page centred on a track, and the page after it.
    let around = session.tool(24, "focus_body", json!({"kind": "arrange", "limit": 2, "around": "r:track:{T0003}"})).await;
    assert_eq!((around["from"].clone(), around["trackCount"].clone()), (json!(1), json!(3)));
    session.tool(25, "focus_body", json!({"kind": "arrange", "limit": 2, "around": "r:track:{T0001}"})).await;

    // What the host refuses before it asks, and what the bridge refuses.
    for (id, tool, arguments) in [
        (15, "read", json!({})),
        (16, "read", json!({"kind": "track", "limit": 0})),
        (17, "focus_body", json!({"kind": "arrange", "window": {"from": 4, "to": 4}})),
        (18, "focus_body", json!({})),
        (19, "read", json!({"kind": "pattern"})),
        (20, "read", json!({"kind": "item"})),
        (21, "focus_body", json!({"kind": "console"})),
    ] {
        let (answer, is_error) = session.tool_full(id, tool, arguments, false).await;
        assert!(is_error && answer["error"].is_string(), "{answer}");
    }
    // A cursor from before the tab changed is refused in words.
    script.switch_tab();
    let (answer, is_error) =
        session.tool_full(22, "read", json!({"kind": "track", "limit": 2, "cursor": first["nextCursor"]}), false).await;
    assert!(is_error && answer["error"].as_str().unwrap().contains("before the project tab changed"), "{answer}");
    // And REAPER quitting is said.
    simulator.stop();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let (answer, is_error) = session.tool_full(23, "snapshot", json!({}), false).await;
    assert!(is_error && answer["connected"] == false, "{answer}");

    golden("reaper-tools-mcp.ndjson", &session.finish().await);
}

trait IntoPair {
    fn into_pair(self) -> (Value, bool);
}

impl IntoPair for (Value, bool) {
    fn into_pair(self) -> (Value, bool) {
        self
    }
}

#[tokio::test]
async fn a_long_project_is_read_to_the_end_inside_the_frame_bound() {
    // 450 tracks with names of 1.2 KB: 540 KB of rows, over four pages of 200 at most.
    let mut lab = Lab::start(450, 1200).await;
    let rows = lab.read_all(json!({"kind": "track", "limit": 200})).await;
    assert_eq!(rows.len(), 451);
    let refs: std::collections::BTreeSet<&str> = rows.iter().map(|r| r["ref"].as_str().unwrap()).collect();
    assert_eq!(refs.len(), 451);
    let pages = lab.script.asked().iter().filter(|(method, _)| method == "read").count();
    assert!(pages >= 3, "{pages} pages");
}

#[tokio::test]
async fn a_page_a_bridge_made_too_big_is_asked_for_again_with_fewer_rows() {
    // 8 KB names: 50 rows are 400 KB; the sim, like a bridge that doesn't know the bound, sends them all.
    let mut lab = Lab::start(120, 8000).await;
    lab.script.ignore_the_frame_bound();
    let (page, is_error) = lab.tool("read", json!({"kind": "track"})).await;
    assert!(!is_error);
    assert_eq!(page["items"].as_array().unwrap().len(), 25);
    assert!(page["nextCursor"].is_string());
    let limits: Vec<Option<u64>> = lab.script.asked().iter().map(|(_, params)| params["limit"].as_u64()).collect();
    // The first ask names no limit (the bridge's default is 50); the second halves it.
    assert_eq!(limits, [None, Some(25)]);
    // The cursor goes on from where the smaller page stopped.
    let (rest, _) = lab.tool("read", json!({"kind": "track", "limit": 25, "cursor": page["nextCursor"]})).await;
    assert_eq!(rest["items"][0]["index"], 25);
}

#[tokio::test]
async fn rows_too_big_even_one_at_a_time_are_refused_in_words() {
    let mut lab = Lab::start(3, 300_000).await;
    lab.script.ignore_the_frame_bound();
    let (answer, is_error) = lab.tool("read", json!({"kind": "track"})).await;
    assert!(is_error);
    assert!(answer["error"].as_str().unwrap().contains("256 KiB"), "{answer}");
    // A snapshot has no page to shrink: it is refused, not passed on.
    let (answer, is_error) = lab.tool("snapshot", json!({})).await;
    assert!(is_error && answer["error"].as_str().unwrap().contains("256 KiB"), "{answer}");
}

#[tokio::test]
async fn a_busy_reaper_is_said_so_and_a_quit_one_is_gone() {
    let mut lab = Lab::start(3, 0).await;
    assert!(!lab.tool("read", json!({"kind": "track"})).await.1);
    lab.simulator.set_busy(Some("rendering"));
    // The focus still answers, saying why it may be stale; the work is refused.
    let (focus, is_error) = lab.tool("focus", json!({})).await;
    assert!(!is_error && focus["busy"] == "rendering", "{focus}");
    for (tool, arguments) in
        [("read", json!({"kind": "track"})), ("snapshot", json!({})), ("focus_body", json!({"kind": "fx", "ref": "r:track:master"}))]
    {
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
async fn arguments_are_checked_before_the_bridge_is_asked() {
    let mut lab = Lab::start(3, 0).await;
    for (tool, arguments) in [
        ("read", json!({})),
        ("read", json!({"kind": "track", "limit": 1.5})),
        ("read", json!({"kind": "track", "parent": 7})),
        ("read", json!({"kind": "track", "pid": "x"})),
        ("focus_body", json!({"kind": "arrange", "window": [0, 4]})),
        ("focus_body", json!({"kind": ["arrange"]})),
        ("focus_body", json!({"kind": "arrange", "around": 12})),
        ("focus_body", json!({"kind": "arrange", "around": ""})),
        ("focus", json!({"resource": 4})),
    ] {
        let (answer, is_error) = lab.tool(tool, arguments.clone()).await;
        assert!(is_error && answer["error"].is_string(), "{tool} {arguments}: {answer}");
    }
    assert!(lab.script.asked().is_empty(), "{:?}", lab.script.asked());
}

#[tokio::test]
async fn only_known_arguments_reach_the_bridge_and_a_limit_past_a_page_is_refused() {
    let mut lab = Lab::start(3, 0).await;
    let (refused, is_error) = lab.tool("read", json!({"kind": "track", "limit": 5000})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("from 1 to 200"), "{refused}");
    assert!(lab.script.asked().is_empty(), "refused before the bridge is asked");
    lab.tool("read", json!({"kind": "track", "limit": 200, "pid": std::process::id(), "mood": "loud"})).await;
    lab.tool("focus_body", json!({"kind": "arrange", "window": {"from": 0, "to": 8}, "extra": 1})).await;
    let asked = lab.script.asked();
    assert_eq!(asked[0], ("read".to_string(), json!({"kind": "track", "limit": 200})));
    assert_eq!(asked[1], ("focus_body".to_string(), json!({"kind": "arrange", "window": {"from": 0.0, "to": 8.0}})));
}

#[tokio::test]
async fn the_pick_chooses_the_reaper_a_read_goes_to() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Reaper);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let mut started = Vec::new();
    for (tracks, resource) in [(2, "/lab/REAPER"), (5, "/lab/REAPER portable")] {
        let script = ReaperScript::new(tracks, 0);
        let mut config = script.config(secret.clone(), resource);
        config.discovery_dir = Some(paths.discovery());
        started.push((script, Simulator::start(config).await.unwrap()));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let mut session = Session::start(Daw::Reaper, home.path());
    let total = |answer: &Value| answer["total"].as_u64().unwrap();
    // No pick: the newest. A pick by resource: that one, and the next call without a pick stays with it.
    assert_eq!(total(&session.tool(1, "read", json!({"kind": "track"})).await), 6);
    assert_eq!(total(&session.tool(2, "read", json!({"kind": "track", "resource": "/lab/REAPER"})).await), 3);
    assert_eq!(total(&session.tool(3, "read", json!({"kind": "track"})).await), 3);
    let (answer, is_error) = session.tool_full(4, "read", json!({"kind": "track", "resource": "/nowhere"}), false).await;
    assert!(is_error && answer["error"].as_str().unwrap().contains("matches"), "{answer}");
    session.finish().await;
    drop(started);
}

#[tokio::test]
async fn every_answer_the_host_hands_over_names_refs_the_way_the_contract_says() {
    let mut lab = Lab::start(5, 0).await;
    let mut answers = Vec::new();
    for (tool, arguments) in [
        ("read", json!({"kind": "track"})),
        ("read", json!({"kind": "item", "parent": "r:track:{T0002}"})),
        ("read", json!({"ref": "r:track:{T0001}"})),
        ("snapshot", json!({})),
        ("focus", json!({})),
        ("focus_body", json!({"kind": "arrange"})),
        ("focus_body", json!({"kind": "routing", "ref": "r:track:{T0001}"})),
    ] {
        let (answer, is_error) = lab.tool(tool, arguments.clone()).await;
        assert!(!is_error, "{tool} {arguments}: {answer}");
        answers.push((tool, answer));
    }
    for (tool, answer) in &answers {
        let misplaced = daw_bridge::contract::misplaced_refs(answer);
        assert!(misplaced.is_empty(), "{tool}: {misplaced:?}");
    }
}

#[tokio::test]
async fn an_arrange_page_centred_on_a_track_starts_half_a_page_above_it() {
    let mut lab = Lab::start(60, 0).await;
    let (page, is_error) = lab.tool("focus_body", json!({"kind": "arrange", "limit": 10, "around": "r:track:{T0040}"})).await;
    assert!(!is_error, "{page}");
    // Track 40 is the 40th of the 60 (the master isn't one): its place is 39, so the page starts at 34.
    assert_eq!((page["from"].clone(), page["trackCount"].clone()), (json!(34), json!(60)));
    let tracks = page["tracks"].as_array().unwrap();
    assert_eq!((tracks.len(), tracks[0]["ref"].clone()), (10, json!("r:track:{T0035}")));
    assert!(tracks.iter().any(|t| t["ref"] == "r:track:{T0040}"));
    assert!(page["nextCursor"].is_string());
    // The last page ends at the last track; a track that isn't there is refused by the bridge in words.
    let (end, _) = lab.tool("focus_body", json!({"kind": "arrange", "limit": 10, "around": "r:track:{T0060}"})).await;
    assert_eq!((end["from"].clone(), end.get("nextCursor").cloned()), (json!(50), None));
    let (gone, is_error) = lab.tool("focus_body", json!({"kind": "arrange", "around": "r:track:{NOPE}"})).await;
    assert!(is_error && gone["code"] == "failed", "{gone}");
    // `around` reached the bridge as asked.
    assert_eq!(lab.script.asked()[0], ("focus_body".to_string(), json!({"kind": "arrange", "around": "r:track:{T0040}", "limit": 10})));
}
