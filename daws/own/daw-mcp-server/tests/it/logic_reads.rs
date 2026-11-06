//! Logic M2's fixed requests and read behavior over the current host wire, replayed from the original recording.
use daw_bridge::auth::{sha256_hex, Secret};
use daw_mcp_server::client::{BridgeClient, RequestError};
use daw_mcp_server::simulator::logic::LogicScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

/// The original M2 request set in its order and with its exact parameters, independently replayed by the native
/// readers over the recorded Scratch scenes and by this host stand-in over the preserved vector.
fn fixed_request_set() -> Vec<(&'static str, Value)> {
    vec![
        ("status", json!({})),
        ("read", json!({"kind": "project"})),
        ("read", json!({"kind": "track"})),
        ("read", json!({"kind": "track", "limit": 1})),
        ("read", json!({"kind": "track", "cursor": "1.1"})),
        ("read", json!({"kind": "region"})),
        ("read", json!({"kind": "region", "parent": "1:track:2"})),
        ("read", json!({"kind": "note"})),
        ("read", json!({"kind": "note", "parent": "1:region:1"})),
        ("read", json!({"kind": "plugin", "parent": "1:track:1"})),
        ("read", json!({"kind": "send", "parent": "1:track:1"})),
        ("read", json!({"kind": "marker"})),
        ("read", json!({"kind": "tempo"})),
        ("read", json!({"ref": "1:track:2"})),
        ("read", json!({"ref": "1:track:9"})),
        ("read", json!({"kind": "bogus"})),
        ("read", json!({"kind": "region", "parent": "0:track:2"})),
        ("snapshot", json!({})),
        ("focus", json!({})),
        ("focus_body", json!({"kind": "tree", "ref": "1:track:1"})),
        ("focus_body", json!({"kind": "tree"})),
        ("focus_body", json!({"kind": "notes", "ref": "1:region:1"})),
        ("focus_body", json!({"kind": "notes"})),
        ("focus_body", json!({"kind": "timeline"})),
        ("focus_body", json!({"kind": "timeline", "around": "1:track:2", "window": {"from": 0, "to": 16}, "limit": 1})),
        ("focus_body", json!({"kind": "routing", "ref": "1:track:1"})),
    ]
}

/// The vector is copied byte-for-byte from d821e669, also retained by the native reader's scene replay.
/// Every fixed request is asked over the wire and compared with that independent expected response.
#[tokio::test]
async fn the_fixed_request_set_is_what_the_vectors_record() {
    let secret = Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap();
    let script = LogicScript::new();
    let simulator = Simulator::start(script.config(secret.clone(), "com.apple.logic10")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret, "daw-mcp-server/test").await.unwrap();
    let mut cases = Vec::new();
    for (method, params) in fixed_request_set() {
        let response = match client.request(method, params.clone(), WAIT).await {
            Ok(result) => json!({"ok": true, "result": result}),
            Err(RequestError::Bridge { code, message }) => json!({"ok": false, "error": {"code": code, "message": message}}),
            Err(other) => panic!("{method} {params}: {other:?}"),
        };
        cases.push(json!({"request": {"method": method, "params": params}, "response": response}));
    }
    simulator.stop();
    let made = json!({"cases": cases, "epoch": "1"});
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("vectors").join("logic-reads.json");
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing", path.display()));
    assert_eq!(
        kept.as_bytes(),
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../logic/logic-kumi/tests/fixtures/logic-reads.json")),
        "host and native tests share the original recording"
    );
    assert_eq!(
        sha256_hex(kept.as_bytes()),
        "72beee747c7e5f0e29c5be4da79da09bbdbdbbdf7684830085adf9a599ad6a32",
        "the original d821e669 vector is never blessed"
    );
    let kept: Value = serde_json::from_str(&kept).expect("the vectors are JSON");
    for (made, kept) in made["cases"].as_array().unwrap().iter().zip(kept["cases"].as_array().unwrap()) {
        assert_eq!(made, kept, "{} {} is answered otherwise than recorded", made["request"]["method"], made["request"]["params"]);
    }
    assert_eq!(made["cases"].as_array().unwrap().len(), kept["cases"].as_array().unwrap().len(), "the recorded set differs in length");
}

fn refused_as(result: Result<serde_json::Value, RequestError>, wanted: &str) -> bool {
    matches!(result, Err(RequestError::Bridge { code, .. }) if code == wanted)
}

#[tokio::test]
async fn the_read_methods_on_the_wire() {
    let secret = Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap();
    let script = LogicScript::new();
    let simulator = Simulator::start(script.config(secret.clone(), "com.apple.logic10")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret, "daw-mcp-server/test").await.unwrap();
    assert_eq!(
        client.welcome().methods,
        [
            "ping",
            "status",
            "focus",
            "read",
            "snapshot",
            "focus_body",
            "change_preview",
            "change_apply",
            "change_restore",
            "transport_stop",
            "transport_play",
            "cursor_set",
            "undo_step_begin",
            "undo_step_end"
        ]
    );
    assert_eq!(client.welcome().events, ["project", "transport", "tracks", "selection", "undone_in_daw"]);
    assert_eq!(client.welcome().identity.daw_version, "12.3.1");

    // Every kind of the contract: the project, the tracks (a page at a time), the children under their parent, one
    // object by its ref.
    let project = client.request("read", json!({"kind": "project"}), WAIT).await.unwrap();
    assert_eq!((project["items"][0]["tempo"].clone(), project["items"][0]["trackCount"].clone()), (json!(160.0), json!(2)));
    let tracks = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap();
    assert_eq!(tracks["items"].as_array().unwrap().len(), 2);
    assert_eq!((tracks["items"][0]["ref"].clone(), tracks["epoch"].clone()), (json!("1:track:1"), json!("1")));
    let first = client.request("read", json!({"kind": "track", "limit": 1}), WAIT).await.unwrap();
    assert_eq!((first["items"].as_array().unwrap().len(), first["nextCursor"].clone()), (1, json!("1.1")));
    let rest = client.request("read", json!({"kind": "track", "cursor": first["nextCursor"]}), WAIT).await.unwrap();
    assert_eq!((rest["items"][0]["ref"].clone(), rest.get("nextCursor").cloned()), (json!("1:track:2"), None));
    let regions = client.request("read", json!({"kind": "region", "parent": "1:track:2"}), WAIT).await.unwrap();
    assert_eq!((regions["items"][0]["kind"].clone(), regions["total"].clone()), (json!("midi"), json!(1)));
    let notes = client.request("read", json!({"kind": "note", "parent": "1:region:1"}), WAIT).await.unwrap();
    assert_eq!(notes["items"].as_array().unwrap().len(), 4);
    assert_eq!((notes["items"][0]["pitch"].clone(), notes["items"][3]["pitch"].clone()), (json!(60), json!(72)));
    let plugins = client.request("read", json!({"kind": "plugin", "parent": "1:track:1"}), WAIT).await.unwrap();
    assert_eq!(plugins["items"][0]["name"], "Channel EQ");
    // Sends as the recording has them (the Scratch project carries none: a send can't be set up from outside yet).
    let sends = client.request("read", json!({"kind": "send", "parent": "1:track:1"}), WAIT).await.unwrap();
    assert_eq!(sends["total"].as_u64().map(|n| n as usize), Some(sends["items"].as_array().unwrap().len()));
    assert!(sends["items"].as_array().unwrap().iter().all(|send| send["destination"].is_string() && send["level"].is_number()), "{sends}");
    let markers = client.request("read", json!({"kind": "marker"}), WAIT).await.unwrap();
    assert!(
        markers["items"][0]["name"].as_str().is_some_and(|name| !name.is_empty()) && markers["items"][0]["position"]["bar"] == 9,
        "{markers}"
    );
    assert_eq!(client.request("read", json!({"kind": "tempo"}), WAIT).await.unwrap()["items"][0]["bpm"], 160.0);
    let one = client.request("read", json!({"ref": "1:track:2"}), WAIT).await.unwrap();
    assert_eq!(one["items"][0]["name"], "Keys");
    let snapshot = client.request("snapshot", json!({}), WAIT).await.unwrap();
    assert_eq!((snapshot["tracks"].as_array().unwrap().len(), snapshot["markers"].as_array().unwrap().len()), (2, 1));
    let focus = client.request("focus", json!({}), WAIT).await.unwrap();
    assert_eq!(focus["selection"]["regionRef"], "1:region:1");
    assert!(focus.get("editor").is_some_and(|editor| editor.is_null() || editor.is_string()), "the open editor, or none: {focus}");
    // The bodies: the selected strip, a region's notes, the Tracks area, a strip's routing.
    let tree = client.request("focus_body", json!({"kind": "tree"}), WAIT).await.unwrap();
    assert_eq!((tree["trackRef"].clone(), tree["strip"]["inserts"][0]["pluginRef"].clone()), (json!("1:track:1"), json!("1:plugin:1.1")));
    let notes_body = client.request("focus_body", json!({"kind": "notes", "ref": "1:region:1"}), WAIT).await.unwrap();
    assert_eq!((notes_body["notes"].as_array().unwrap().len(), notes_body["regionRef"].clone()), (4, json!("1:region:1")));
    let timeline = client.request("focus_body", json!({"kind": "timeline"}), WAIT).await.unwrap();
    assert_eq!(timeline["lanes"][1]["regions"][0]["ref"], "1:region:1");
    let routing = client.request("focus_body", json!({"kind": "routing", "ref": "1:track:1"}), WAIT).await.unwrap();
    assert_eq!((routing["trackRef"].clone(), routing["sends"].is_array()), (json!("1:track:1"), true));

    // What the bridge refuses: a kind it doesn't have, notes without their region, a ref that isn't in the project, a
    // ref from before the epoch.
    assert!(refused_as(client.request("read", json!({"kind": "bogus"}), WAIT).await, "bad-params"));
    assert!(refused_as(client.request("read", json!({"kind": "note"}), WAIT).await, "bad-params"));
    assert!(refused_as(client.request("read", json!({"ref": "1:track:9"}), WAIT).await, "failed"));
    assert!(refused_as(client.request("read", json!({"kind": "region", "parent": "0:track:2"}), WAIT).await, "stale"));

    // A track changed: the epoch moves, refs and cursors from before are stale, and the new ones read the same.
    script.change_tracks();
    let stale = client.request("read", json!({"ref": "1:track:2"}), WAIT).await.unwrap_err();
    assert!(matches!(&stale, RequestError::Bridge { code, message } if code == "stale" && message.contains("earlier epoch")), "{stale:?}");
    assert!(refused_as(client.request("read", json!({"kind": "track", "cursor": "1.1"}), WAIT).await, "stale"));
    assert!(refused_as(client.request("focus_body", json!({"kind": "routing", "ref": "1:track:1"}), WAIT).await, "stale"));
    let again = client.request("read", json!({"kind": "track", "limit": 1}), WAIT).await.unwrap();
    assert_eq!(
        (again["epoch"].clone(), again["items"][0]["ref"].clone(), again["nextCursor"].clone()),
        (json!("2"), json!("2:track:1"), json!("2.1"))
    );
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["selection"]["trackRef"], "2:track:1");

    // Without Accessibility the status says what is missing, and every read fails in words.
    simulator.set_busy(Some("modal"));
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}));
    assert_eq!(client.request("status", json!({}), WAIT).await.unwrap()["busy"], "modal");
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["busy"], "modal");
    simulator.set_permission(Some("accessibility"));
    assert_eq!(client.request("status", json!({}), WAIT).await.unwrap()["permission"], "accessibility");
    let refused = client.request("focus", json!({}), WAIT).await.unwrap_err();
    assert!(
        matches!(&refused, RequestError::Bridge { code, message } if code == "failed" && message.contains("Accessibility")),
        "{refused:?}"
    );
    for (method, params) in [("read", json!({"kind":"track"})), ("snapshot", json!({})), ("focus_body", json!({"kind":"timeline"}))] {
        let refused = client.request(method, params, WAIT).await.unwrap_err();
        assert!(matches!(refused, RequestError::Bridge { code, message } if code == "failed" && message.contains("Accessibility")));
    }
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}));
    simulator.set_permission(None);
    simulator.set_busy(None);
    assert_eq!(script.asked().len(), 27, "{:?}", script.asked());

    simulator.stop();
}
