//! The host's change tools (M3 contract §3a) against the simulated Logic Pro, which answers as the bridge does: a
//! header's control and the tempo put back by value with no step of Logic's undo, a rename, a track added or deleted
//! and a marker made, moved or deleted each a step of it (it has no undo block: a plan's step is counted), a track
//! added below the selected one moving the epoch with a restore's ref carried past it, a deleted track and a deleted
//! marker left for the producer's manual Logic undo, a write Logic doesn't answer after left uncertain
//! and never sent twice (an apply's and an undo's), the kinds the bridge can't make refused saying why, the transport
//! and the playhead, and what the bridge receives for every method, as a golden of the wire
//! (tests/golden/logic-changes-bridge.ndjson).
#[path = "../common/mod.rs"]
mod common;

use common::{golden, normalized};
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use daw_mcp_server::mcp;
use daw_mcp_server::simulator::logic::LogicScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};
use std::sync::Arc;

const BUNDLE: &str = "com.apple.logic10";

struct Lab {
    _home: tempfile::TempDir,
    host: Arc<Host>,
    script: LogicScript,
    simulator: Simulator,
    /// The bridge's events as the host passes them on to its client, the journal's transaction ids filled in.
    events: tokio::sync::broadcast::Receiver<Value>,
    next_id: u64,
}

impl Lab {
    async fn start() -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Logic);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = LogicScript::new();
        let mut config = script.config(secret, BUNDLE);
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let host = Arc::new(Host::new(Daw::Logic, home.path()));
        let events = host.events();
        Lab { _home: home, host, script, simulator, events, next_id: 100 }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next_id += 1;
        let id = self.next_id;
        let answer =
            mcp::handle(&self.host, &json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}))
                .await
                .unwrap();
        assert!(answer.get("error").is_none(), "{name}: {answer}");
        (answer["result"]["structuredContent"].clone(), answer["result"]["isError"] == true)
    }

    /// A change previewed (in the epoch the project is in): the preview's answer.
    async fn preview(&mut self, kind: &str, input: Value) -> Value {
        let (preview, is_error) = self.try_preview(kind, input).await;
        assert!(!is_error, "{kind}: {preview}");
        preview
    }

    async fn try_preview(&mut self, kind: &str, input: Value) -> (Value, bool) {
        let epoch = self.script.epoch().to_string();
        self.tool("change_preview", json!({"kind": kind, "input": input, "epoch": epoch})).await
    }

    async fn apply(&mut self, preview: &Value, key: &str) -> (Value, bool) {
        let (id, confirmation) = (preview["transactionId"].clone(), preview["confirmation"].clone());
        self.tool("change_apply", json!({"transactionId": id, "confirmation": confirmation, "idempotencyKey": key})).await
    }

    async fn undo(&mut self, preview: &Value, key: &str, discard: bool) -> (Value, bool) {
        let id = preview["transactionId"].clone();
        self.tool("change_undo", json!({"transactionId": id, "confirmation": "undo", "idempotencyKey": key, "discard": discard})).await
    }

    /// A change made: its preview and its apply's answer (which must have gone through).
    async fn make(&mut self, kind: &str, input: Value, key: &str) -> (Value, Value) {
        let preview = self.preview(kind, input).await;
        let (applied, is_error) = self.apply(&preview, key).await;
        assert!(!is_error && applied["state"] == "applied", "{kind}: {applied}");
        (preview, applied)
    }

    /// The next `undone_in_daw` the host passes on.
    async fn undone_in_daw(&mut self) -> Value {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), self.events.recv()).await.unwrap().unwrap();
            if event["name"] == "undone_in_daw" {
                return event["data"].clone();
            }
        }
    }
}

/// A desc's nonce (`k` and four base-36 characters, in parentheses) made the same in every run.
fn without_nonces(transcript: &str) -> String {
    let transcript = transcript.replace(daw_mcp_server::host::CLIENT_NAME, "daw-mcp-server/<version>");
    let mut out = String::with_capacity(transcript.len());
    let mut rest = transcript.as_str();
    while let Some(at) = rest.find("(k") {
        let after = &rest[at + 2..];
        let nonce = after.get(..4).filter(|n| n.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()));
        match nonce.filter(|_| after.as_bytes().get(4) == Some(&b')')) {
            Some(_) => {
                out.push_str(&rest[..at]);
                out.push_str("(k<nonce>)");
                rest = &after[5..];
            }
            None => {
                out.push_str(&rest[..at + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn position(bar: i64, beat: i64) -> Value {
    json!({"bar": bar, "beat": beat, "division": 1, "tick": 1})
}

/// A header's control: no step of Logic's undo for the change nor for its undo (the Edit menu stays "Can't Undo");
/// both are writes, read back, and each key is answered once.
#[tokio::test]
async fn a_change_is_previewed_applied_and_undone_by_value_with_no_step_of_logics_undo() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("track-volume", json!({"ref": "1:track:1", "volumeDb": -6})).await;
    assert_eq!(
        (&preview["prior"], &preview["target"]["ref"], &preview["epoch"]),
        (&json!({"volumeDb": 0.0}), &json!("1:track:1"), &json!("1"))
    );
    assert_eq!(preview["display"]["title"], "Vocal volume −6 dB");
    assert_eq!(lab.script.volume(1), 0.0, "a preview changes nothing");

    let (wrong, is_error) =
        lab.tool("change_apply", json!({"transactionId": preview["transactionId"], "confirmation": "c0000", "idempotencyKey": "a1"})).await;
    assert!(is_error && wrong["code"] == "confirmation", "{wrong}");

    let (applied, is_error) = lab.apply(&preview, "a1").await;
    assert!(!is_error && applied["state"] == "applied", "{applied}");
    assert_eq!(applied["applied"], json!({"volumeDb": -6.0}), "the value read back");
    let desc = applied["undo"]["desc"].as_str().unwrap().to_string();
    assert!(desc.starts_with("Kumi: Vocal volume −6 dB (k") && desc.ends_with(')'), "{desc}");
    assert_eq!(applied["undo"], json!({"desc": desc, "entry": false}), "no step of Logic's undo, so no title");
    assert_eq!(lab.script.volume(1), -6.0);
    // The same key again: the same answer, and Logic changed once.
    assert_eq!(lab.apply(&preview, "a1").await, (applied.clone(), false));
    assert!(lab.script.undo_titles().is_empty(), "{:?}", lab.script.undo_titles());
    // The value shows in a read after the change.
    let (row, _) = lab.tool("read", json!({"ref": "1:track:1"})).await;
    assert_eq!(row["items"][0]["volumeDb"], -6.0, "{row}");

    let (undone, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(!is_error && undone["state"] == "undone", "{undone}");
    assert_eq!(
        (undone["restored"].clone(), undone["display"]["title"].clone()),
        (json!({"volumeDb": 0.0}), json!("Vocal volume 0 dB (put back)"))
    );
    assert_eq!(lab.script.volume(1), 0.0);
    assert!(lab.script.undo_titles().is_empty(), "put back by value: Logic's Edit menu still says Can't Undo");
    assert_eq!(lab.undo(&preview, "u1", false).await, (undone, false));
    let (again, is_error) = lab.undo(&preview, "u2", false).await;
    assert!(is_error && again["code"] == "undone", "{again}");
    lab.simulator.stop();
}

/// Logic has no undo block: a plan's step is counted, a mute in it is no step of Logic's undo and a rename is one
/// ("Renaming"), which the producer's Cmd-Z takes back and the bridge names to the host by the desc it kept.
#[tokio::test]
async fn a_plan_is_counted_and_only_its_rename_is_a_step_of_logics_undo_which_cmd_z_takes_back() {
    let mut lab = Lab::start().await;
    let (begun, is_error) = lab.tool("undo_step_begin", json!({"label": "Tighten the keys"})).await;
    assert!(!is_error && begun["stepId"] == "s1", "{begun}");
    let (_, muted) = lab.make("track-mute", json!({"ref": "1:track:2", "mute": true}), "a1").await;
    assert_eq!(muted["undo"]["entry"], false);
    let (_, renamed) = lab.make("track-rename", json!({"ref": "1:track:2", "name": "Lead"}), "a2").await;
    assert_eq!(renamed["display"]["title"], "Renamed track “Keys” → “Lead”");
    assert_eq!((renamed["undo"]["entry"].clone(), renamed["undo"]["title"].clone()), (json!(true), json!("Undo Renaming")));
    assert_eq!(renamed["epoch"], "2", "a renamed header moves the epoch, as the observers count it");
    assert_eq!((lab.script.track(2)["mute"].clone(), lab.script.track(2)["name"].clone()), (json!(true), json!("Lead")));
    let history = lab.script.undo_history();
    assert_eq!(history.len(), 1, "the rename alone is a step: {history:?}");
    assert!(history[0].starts_with("Kumi: Renamed track “Keys” → “Lead” (k"), "{history:?}");
    assert_eq!(lab.script.undo_titles(), ["Undo Renaming"]);
    let (ended, is_error) = lab.tool("undo_step_end", json!({"stepId": "s1", "label": "Keys muted and 1 more"})).await;
    assert!(!is_error, "{ended}");
    assert!(ended["desc"].as_str().unwrap().starts_with("Kumi: Keys muted and 1 more (k"), "{ended}");
    assert_eq!(
        (ended["entry"].clone(), ended["changes"].clone()),
        (json!(false), json!(2)),
        "counted, no step of Logic's undo for the plan"
    );

    // Explicit producer fixture: the renamed value is back, providing positive evidence independent of the Undo title.
    let (title, desc) = lab.script.undo_last().expect("a step");
    assert_eq!(title, "Undo Renaming");
    lab.script.set_track(2, "name", json!("Keys"));
    lab.simulator.emit("undone_in_daw", json!({"descs": [desc.clone().unwrap()], "how": "undo", "epoch": "2"}));
    let named = lab.undone_in_daw().await;
    assert_eq!((named["transactionIds"].clone(), named["how"].clone()), (json!([renamed["transactionId"]]), json!("undo")));
    assert_ne!(renamed["transactionId"], muted["transactionId"]);
    lab.simulator.stop();
}

#[tokio::test]
async fn what_the_producer_changed_since_is_never_overwritten_unless_kumi_is_told_to() {
    let mut lab = Lab::start().await;
    // Moved between the preview and the apply: nothing is written.
    let preview = lab.preview("track-volume", json!({"ref": "1:track:1", "volumeDb": -6})).await;
    lab.script.set_volume(1, -3.0);
    let (refused, is_error) = lab.apply(&preview, "a1").await;
    assert!(is_error && refused["code"] == "changed" && refused["error"] == "Vocal's volume changed since the preview", "{refused}");
    assert_eq!(lab.script.volume(1), -3.0);

    // Moved after the apply: the undo is refused and the change stays undoable.
    let preview = lab.preview("track-volume", json!({"ref": "1:track:1", "volumeDb": -6})).await;
    lab.apply(&preview, "a1").await;
    lab.script.set_volume(1, -9.0);
    let (refused, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(
        is_error && refused["code"] == "changed" && refused["error"] == "Vocal's volume changed in Logic Pro since Kumi set it",
        "{refused}"
    );
    assert_eq!(lab.script.volume(1), -9.0);
    // Put back by hand, the same undo goes through.
    lab.script.set_volume(1, -6.0);
    let (undone, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(!is_error && undone["state"] == "undone", "{undone}");
    assert_eq!(lab.script.volume(1), -3.0, "back to what the preview saw");

    // Told to discard: the values go back whatever happened since.
    let preview = lab.preview("track-volume", json!({"ref": "1:track:1", "volumeDb": -12})).await;
    lab.apply(&preview, "a1").await;
    lab.script.set_volume(1, 1.0);
    let (undone, is_error) = lab.undo(&preview, "u1", true).await;
    assert!(!is_error && undone["state"] == "undone", "{undone}");
    assert_eq!(lab.script.volume(1), -3.0);
    lab.simulator.stop();
}

/// A track added or deleted (and a header renamed) moves Logic's epoch: a change previewed before is refused as
/// stale, the reads carry the new epoch, and a change made before is still undone, its ref carried to where its track
/// is now. A new track goes below the selected one, so `index` counts from 1. A deleted track comes back by Logic's
/// own undo manually: its title cannot prove step ownership.
#[tokio::test]
async fn a_track_added_below_a_track_moves_the_epoch_and_changes_before_it_are_still_undone_by_their_place() {
    let mut lab = Lab::start().await;
    let (rename, renamed) = lab.make("track-rename", json!({"ref": "1:track:2", "name": "Lead"}), "a1").await;
    assert_eq!(renamed["epoch"], "2");
    let (volume, _) = lab.make("track-volume", json!({"ref": "2:track:1", "volumeDb": -6}), "a2").await;
    let stale = lab.preview("track-mute", json!({"ref": "2:track:2", "mute": true})).await;

    let (add, added) = lab.make("track-add", json!({"name": "Pad", "kind": "instrument", "index": 1}), "a3").await;
    assert_eq!((add["prior"].clone(), add["target"].clone()), (json!({"tracks": 2}), json!({"ref": null, "kind": "track", "index": 1})));
    assert_eq!(add["display"], json!({"title": "Added instrument track “Pad”", "index": 1}));
    assert_eq!((added["epoch"].clone(), added["created"].clone()), (json!("3"), json!({"ref": "3:track:2", "kind": "track"})));
    assert_eq!(
        (added["applied"].clone(), added["undo"]["title"].clone()),
        (json!({"index": 2, "name": "Pad", "kind": "instrument"}), json!("Undo Create Track"))
    );
    assert_eq!(lab.script.track_count(), 3);
    let (page, _) = lab.tool("read", json!({"kind": "track"})).await;
    assert_eq!(
        (page["epoch"].clone(), page["total"].clone(), page["items"][1]["name"].clone(), page["items"][2]["name"].clone()),
        (json!("3"), json!(3), json!("Pad"), json!("Lead"))
    );
    // Previewed before the add: refused as stale, nothing written.
    let (refused, is_error) = lab.apply(&stale, "a4").await;
    assert!(is_error && refused["code"] == "stale", "{refused}");
    assert_eq!(lab.script.track(3)["mute"], json!(false));
    // Index 0 (above every track) isn't reachable; a kind Logic's menu hasn't is refused.
    let (refused, is_error) = lab.try_preview("track-add", json!({"index": 0})).await;
    assert!(
        is_error && refused["code"] == "bad-params" && refused["error"].as_str().unwrap().contains("below the selected one"),
        "{refused}"
    );
    let (refused, is_error) = lab.try_preview("track-add", json!({"kind": "drummer"})).await;
    assert!(is_error && refused["error"] == "kind must be audio, instrument or external", "{refused}");

    // Each undone, the ones from before the add by the place their track had then: the rename by value (a fresh
    // step of Logic's undo), the added track left for manual Undo, the fader by value.
    let (undone, is_error) = lab.undo(&rename, "u1", false).await;
    assert!(!is_error && undone["state"] == "undone" && undone["epoch"] == "4", "{undone}");
    assert_eq!(lab.script.track(3)["name"], json!("Keys"));
    let (refused, is_error) = lab.undo(&add, "u2", false).await;
    assert!(
        is_error && refused["code"] == "failed" && refused["error"].as_str().unwrap().contains("doesn't expose all of its contents"),
        "{refused}"
    );
    assert_eq!((lab.script.epoch(), lab.script.track_count()), (4, 3));
    let (undone, is_error) = lab.undo(&volume, "u3", false).await;
    assert!(!is_error && undone["state"] == "undone", "{undone}");
    assert_eq!(lab.script.volume(1), 0.0);
    assert_eq!(lab.script.undo_titles(), ["Undo Renaming", "Undo Create Track", "Undo Renaming"]);

    // Deletion cannot be restored automatically: even a matching top title proves no step identity.
    let (delete, deleted) = lab.make("track-delete", json!({"ref": "4:track:2"}), "a5").await;
    assert_eq!(deleted["epoch"], "5");
    let titles = lab.script.undo_titles();
    let before = lab.script.asked().len();
    let (refused, is_error) = lab.undo(&delete, "u4", false).await;
    assert!(is_error && refused["code"] == "failed", "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("Undo title cannot identify Kumi's specific step"), "{refused}");
    assert_eq!(lab.script.track_count(), 2);
    assert_eq!(lab.script.epoch(), 5);
    assert_eq!(lab.script.undo_titles(), titles, "no Undo was pressed");
    assert_eq!(lab.script.asked()[before..].iter().filter(|(method, _)| method == "change_restore").count(), 1, "no automatic resend");
    // The journal still knows the delete applied; asking for its apply answer never deletes again.
    let applies = lab.script.asked().iter().filter(|(method, _)| method == "change_apply").count();
    assert_eq!(lab.apply(&delete, "a5").await, (deleted, false));
    assert_eq!(lab.script.asked().iter().filter(|(method, _)| method == "change_apply").count(), applies);
    lab.make("track-rename", json!({"ref": "5:track:1", "name": "Vox"}), "a6").await;
    let (again, is_error) = lab.undo(&delete, "u5", true).await;
    assert!(is_error && again["code"] == "failed", "discard cannot establish undo identity: {again}");
    assert_eq!(lab.script.track_count(), 2);
    lab.simulator.stop();
}

/// A write Logic doesn't answer after: the bridge answers uncertain, the journal keeps the change uncertain (never
/// applied again, never undone blindly), and an undo that went the same way is never sent twice.
#[tokio::test]
async fn a_write_logic_doesnt_answer_after_is_uncertain_and_is_never_sent_twice() {
    let mut lab = Lab::start().await;
    lab.script.uncertain_once();
    let preview = lab.preview("track-volume", json!({"ref": "1:track:1", "volumeDb": -6})).await;
    let (unsure, is_error) = lab.apply(&preview, "a1").await;
    assert!(is_error && unsure["state"] == "uncertain" && unsure["code"] == "uncertain", "{unsure}");
    assert_eq!(unsure["transactionId"], preview["transactionId"]);
    assert!(unsure["error"].as_str().unwrap().contains("didn't answer after the change was sent"), "{unsure}");
    assert_eq!(lab.script.volume(1), -6.0, "Logic took the write");
    assert!(lab.script.undo_titles().is_empty(), "a header's control makes no step");
    // The same key: the same answer; another: the change was uncertain already; its undo: refused, not sent.
    assert_eq!(lab.apply(&preview, "a1").await, (unsure.clone(), true));
    let (again, is_error) = lab.apply(&preview, "a2").await;
    assert!(is_error && again["code"] == "applied" && again["error"].as_str().unwrap().contains("uncertain"), "{again}");
    let asked = lab.script.asked().len();
    let (refused, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(
        is_error && refused["code"] == "uncertain" && refused["error"].as_str().unwrap().contains("won't undo it blindly"),
        "{refused}"
    );
    assert_eq!(lab.script.asked().len(), asked, "nothing reached the bridge");

    // An undo Logic doesn't answer after: the restore may have run, so the change is uncertain and the undo is never
    // sent again, under any key.
    let (pan, applied) = lab.make("track-pan", json!({"ref": "1:track:1", "pan": 0.25}), "a3").await;
    assert_eq!(
        (applied["display"]["title"].clone(), applied["applied"].clone()),
        (json!("Vocal pan 25% right"), json!({"pan": 0.25, "steps": 16}))
    );
    lab.script.uncertain_once();
    let (unsure, is_error) = lab.undo(&pan, "u2", false).await;
    assert!(is_error && unsure["state"] == "uncertain" && unsure["code"] == "undo-uncertain" && unsure["answered"] == true, "{unsure}");
    assert!(unsure["error"].as_str().unwrap().contains("didn't answer after the change was sent"), "{unsure}");
    assert_eq!(lab.script.track(1)["pan"], json!(0), "Logic put the pan back");
    let asked = lab.script.asked().len();
    for key in ["u2", "u3"] {
        let (refused, is_error) = lab.undo(&pan, key, false).await;
        assert!(
            is_error && refused["code"] == "undo-uncertain" && refused["error"].as_str().unwrap().contains("won't undo it again"),
            "{key}: {refused}"
        );
    }
    assert_eq!(lab.script.asked().len(), asked, "nothing reached the bridge");
    lab.simulator.stop();
}

/// Markers by their place in the Marker List, numbered by Logic, each a step of its undo; a deleted one back through
/// manual Logic Undo; the tempo in whole BPM, no step; and the kinds the bridge can't
/// make, refused saying why.
#[tokio::test]
async fn markers_and_the_tempo_change_and_come_back_in_logics_words() {
    let mut lab = Lab::start().await;
    let (tempo, applied) = lab.make("tempo-set", json!({"bpm": 128}), "a1").await;
    assert_eq!((tempo["prior"].clone(), tempo["target"].clone()), (json!({"bpm": 160.0}), json!({"ref": null, "kind": "project"})));
    assert_eq!((applied["display"]["title"].clone(), applied["undo"]["entry"].clone()), (json!("Tempo 128 BPM"), json!(false)));
    let (read, _) = lab.tool("read", json!({"kind": "tempo"})).await;
    let (focus, _) = lab.tool("focus", json!({})).await;
    assert_eq!((read["items"][0]["bpm"].clone(), focus["transport"]["tempo"].clone()), (json!(128.0), json!(128.0)));
    let (undone, _) = lab.undo(&tempo, "u1", false).await;
    assert_eq!((undone["display"]["title"].clone(), lab.script.tempo()), (json!("Tempo back to 160 BPM"), 160.0));
    let rounded = lab.preview("tempo-set", json!({"bpm": 120.4})).await;
    assert_eq!(rounded["display"]["title"], "Tempo 120 BPM");
    assert!(rounded["display"]["note"].as_str().unwrap().contains("was rounded to 120"), "{rounded}");

    // A marker can't be named (Logic numbers it) nor placed in seconds.
    let (refused, is_error) = lab.try_preview("marker-add", json!({"positionBeats": 16, "name": "Verse"})).await;
    assert!(
        is_error && refused["code"] == "bad-params" && refused["error"].as_str().unwrap().contains("names markers itself"),
        "{refused}"
    );
    let (refused, is_error) = lab.try_preview("marker-add", json!({"position": 6})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("not seconds"), "{refused}");
    // Positive value-restore fixture explicitly exposes Text before the first preview; it is not the original recording.
    expose_marker_text(&lab.script);
    // One added before the recorded one is first in the list; a move re-sorts it; a delete and the undos, each a step.
    let (add, added) = lab.make("marker-add", json!({"positionBeats": 16}), "a2").await;
    assert_eq!((add["prior"].clone(), add["display"]["title"].clone()), (json!({"markers": 1}), json!("Marker at bar 5")));
    assert_eq!(
        (added["created"].clone(), added["applied"]["position"].clone()),
        (json!({"ref": "1:marker:1", "kind": "marker"}), position(5, 1))
    );
    assert_eq!(
        (added["display"]["title"].clone(), added["undo"]["title"].clone()),
        (json!("Marker “Marker 2” at bar 5"), json!("Undo Create Marker"))
    );
    let (page, _) = lab.tool("read", json!({"kind": "marker"})).await;
    assert_eq!(page["items"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Marker 2", "Marker 1"]);
    assert_eq!(page["items"][0]["position"], position(5, 1));
    let (moved, applied) = lab.make("marker-move", json!({"ref": "1:marker:1", "positionBeats": 40}), "a3").await;
    assert_eq!(
        (moved["prior"]["position"].clone(), applied["display"]["title"].clone()),
        (position(5, 1), json!("Moved marker “Marker 2” to bar 11"))
    );
    assert_eq!(applied["undo"]["title"], "Undo Value Change");
    assert_eq!(lab.script.markers(), [("Marker 1".to_string(), 32.0), ("Marker 2".to_string(), 40.0)]);
    let (undone, is_error) = lab.undo(&moved, "u3", false).await;
    assert!(!is_error && undone["display"]["title"] == "marker “Marker 2” back at bar 5", "{undone}");
    assert_eq!(lab.script.markers(), [("Marker 2".to_string(), 16.0), ("Marker 1".to_string(), 32.0)]);
    let (gone, is_error) = lab.undo(&add, "u4", false).await;
    assert!(
        !is_error
            && gone["restored"] == json!({"deleted": "1:marker:1"})
            && gone["display"]["title"] == "marker “Marker 2” taken away again",
        "{gone}"
    );
    assert_eq!(lab.script.markers(), [("Marker 1".to_string(), 32.0)]);
    assert_eq!(lab.script.undo_titles(), ["Undo Create Marker", "Undo Value Change", "Undo Value Change", "Undo Delete"]);
    assert_eq!(lab.script.epoch(), 1, "markers and the tempo never move the epoch");

    let (delete, deleted) = lab.make("marker-delete", json!({"ref": "1:marker:1"}), "a4").await;
    assert_eq!(deleted["applied"], json!({"deleted": true}));
    let titles = lab.script.undo_titles();
    let asked = lab.script.asked().len();
    let (refused, is_error) = lab.undo(&delete, "u5", false).await;
    assert!(is_error && refused["code"] == "failed", "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("Undo title cannot identify Kumi's specific step"), "{refused}");
    assert_eq!(lab.script.markers().len(), 0, "no renamed approximation was created");
    assert_eq!(lab.script.undo_titles(), titles, "no possibly unrelated undo was pressed");
    assert_eq!(lab.script.asked()[asked..].iter().filter(|(method, _)| method == "change_restore").count(), 1);
    assert_eq!(lab.apply(&delete, "a4").await, (deleted, false), "the journal keeps the applied deletion");

    // What the bridge can't make yet is refused saying why; what it makes beyond the simulator says so too.
    for (kind, why) in [
        ("loop-range", "the cycle locators have no field Accessibility can set"),
        ("marker-rename", "a marker's name cell in the Marker List isn't a text field"),
        ("fx-add", "the insert slot's pop-up menu can't be driven from outside"),
        ("send-volume", "the send slot's pop-up menu can't be driven from outside"),
    ] {
        let (refused, is_error) = lab.try_preview(kind, json!({"ref": "1:marker:1"})).await;
        assert!(is_error && refused["code"] == "bad-params", "{kind}: {refused}");
        assert_eq!(refused["error"], format!("Logic Pro's bridge can't make a {kind} change yet: {why}"));
    }
    for (kind, input) in [
        ("fx-bypass", json!({"ref":"1:plugin:1.1","bypass":true})),
        ("notes-add", json!({"ref":"1:region:1","notes":[]})),
        ("notes-change", json!({"ref":"1:region:1","notes":[]})),
        ("notes-delete", json!({"ref":"1:region:1","notes":[]})),
    ] {
        let (refused, error) = lab.try_preview(kind, input).await;
        assert!(error && refused["error"].as_str().unwrap().starts_with("the simulated Logic Pro makes only"), "{kind}: {refused}");
    }
    let (refused, _) = lab.try_preview("bogus", json!({})).await;
    assert_eq!(refused["error"], "Logic Pro has no change kind \"bogus\"");
    lab.simulator.stop();
}

#[tokio::test]
async fn a_dialog_stops_a_change_before_anything_is_written_and_the_transport_and_playhead_act() {
    let mut lab = Lab::start().await;
    lab.script.busy_once();
    let (busy, is_error) = lab.try_preview("track-mute", json!({"ref": "1:track:1", "mute": true})).await;
    assert!(
        is_error && busy["code"] == "busy" && busy["busy"] == true && busy["error"] == "a dialog is up in Logic Pro; nothing changed",
        "{busy}"
    );
    assert_eq!(lab.script.track(1)["mute"], json!(false));

    let (playing, is_error) = lab.tool("transport_play", json!({})).await;
    assert!(!is_error && playing == json!({"started": true, "playing": true}), "{playing}");
    let (focus, _) = lab.tool("focus", json!({})).await;
    assert_eq!(focus["transport"]["playing"], true);
    // The playhead, in beats or in seconds (160 BPM: a beat is 0.375 s), to the bar and beat.
    let (moved, is_error) = lab.tool("cursor_set", json!({"positionBeats": 8})).await;
    assert!(
        !is_error
            && moved == json!({"epoch": "1", "positionBeats": 8.0, "position": 3.0, "playhead": {"bar": 3, "beat": 1}, "playing": true}),
        "{moved}"
    );
    let (moved, _) = lab.tool("cursor_set", json!({"position": 6})).await;
    assert_eq!(
        (moved["positionBeats"].clone(), moved["playhead"].clone(), lab.script.playhead()),
        (json!(16.0), json!({"bar": 5, "beat": 1}), 16.0)
    );
    let (focus, _) = lab.tool("focus", json!({})).await;
    assert_eq!((focus["playhead"].clone(), focus["transport"]["beat"].clone()), (json!({"bar": 5, "beat": 1}), json!(16.0)));
    let (stopped, _) = lab.tool("transport_stop", json!({})).await;
    assert_eq!(stopped, json!({"stopped": true, "wasRecording": false}));
    let (stopped, _) = lab.tool("transport_stop", json!({})).await;
    assert_eq!(stopped["stopped"], false, "stopped already");

    // Released changes are forgotten.
    let (preview, _) = lab.make("track-solo", json!({"ref": "1:track:2", "solo": true}), "a1").await;
    let (released, is_error) = lab.tool("change_release", json!({"transactionIds": [preview["transactionId"], "t-unknown"]})).await;
    assert_eq!((released, is_error), (json!({"released": 1}), false));
    let (gone, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(is_error && gone["code"] == "unknown", "{gone}");
    // A connected bridge without its grant cannot preview or write through any M3 entry point.
    lab.simulator.set_permission(Some("accessibility"));
    let before = lab.script.asked().len();
    for (tool, arguments) in [
        ("change_preview", json!({"kind":"track-mute","input":{"ref":"1:track:1","mute":true},"epoch":"1"})),
        ("transport_play", json!({})),
        ("cursor_set", json!({"positionBeats":4})),
        ("undo_step_begin", json!({"label":"A blocked plan"})),
    ] {
        let (refused, error) = lab.tool(tool, arguments).await;
        assert!(error && refused["code"] == "failed" && refused["error"].as_str().unwrap().contains("Accessibility"), "{tool}: {refused}");
        assert!(refused.get("busy").is_none(), "{refused}");
    }
    assert_eq!(lab.script.asked().len(), before, "the native permission refusal precedes changes");
    lab.simulator.stop();
}

/// Every change method once, over the wire between the host and the bridge: what Logic's bridge receives for a preview,
/// an apply, a restore, a plan's counted step, the transport and the playhead, and what it answers, as the golden
/// tests/golden/logic-changes-bridge.ndjson (the descs' nonces made the same in every run).
#[tokio::test]
async fn the_change_methods_on_the_wire() {
    let original = include_bytes!("../fixtures/logic-m3/logic-changes-bridge.original.ndjson");
    assert_eq!(daw_bridge::auth::sha256_hex(original), "e443804b394da96e9caee12c32de4735465ff2df7116ae1b46c45e2655609508");
    let mut lab = Lab::start().await;
    let (volume, _) = lab.make("track-volume", json!({"ref": "1:track:1", "volumeDb": -6}), "a1").await;
    lab.undo(&volume, "u1", false).await;
    lab.script.busy_once();
    lab.try_preview("track-pan", json!({"ref": "1:track:1", "pan": -0.5})).await;
    lab.script.uncertain_once();
    let preview = lab.preview("track-arm", json!({"ref": "1:track:1", "arm": true})).await;
    lab.apply(&preview, "a2").await;
    lab.tool("undo_step_begin", json!({"label": "A plan"})).await;
    lab.make("track-mute", json!({"ref": "1:track:2", "mute": true}), "a3").await;
    lab.make("track-rename", json!({"ref": "1:track:2", "name": "Lead"}), "a4").await;
    lab.tool("undo_step_end", json!({"stepId": "s1", "label": "Keys muted and 1 more"})).await;
    let (add, _) = lab.make("track-add", json!({"name": "Pad", "kind": "instrument"}), "a5").await;
    // Rename restore is value-backed; the subsequent track removal request is refused manually.
    let (pads, _) = lab.make("track-rename", json!({"ref": "3:track:3", "name": "Pads"}), "a6").await;
    lab.undo(&pads, "u2", false).await;
    lab.undo(&add, "u3", false).await;
    let (marker, _) = lab.make("marker-add", json!({"positionBeats": 16}), "a7").await;
    let (moved, _) = lab.make("marker-move", json!({"ref": "5:marker:1", "positionBeats": 20}), "a8").await;
    lab.undo(&moved, "u4", false).await;
    lab.undo(&marker, "u5", false).await;
    let (tempo, _) = lab.make("tempo-set", json!({"bpm": 128}), "a9").await;
    lab.undo(&tempo, "u6", false).await;
    lab.tool("transport_play", json!({})).await;
    lab.tool("cursor_set", json!({"positionBeats": 8})).await;
    lab.tool("transport_stop", json!({})).await;
    let (delete, _) = lab.make("track-delete", json!({"ref": "5:track:2"}), "a10").await;
    lab.undo(&delete, "u7", false).await;
    assert_eq!(
        (lab.script.epoch(), lab.script.track_count()),
        (6, 2),
        "value undos went through; structural and unknown-Text marker restores require manual Logic undo"
    );
    let asked: Vec<String> = lab.script.asked().into_iter().map(|(method, _)| method).collect();
    for method in [
        "change_preview",
        "change_apply",
        "change_restore",
        "undo_step_begin",
        "undo_step_end",
        "transport_play",
        "cursor_set",
        "transport_stop",
    ] {
        assert!(asked.iter().any(|m| m == method), "{method} never reached the bridge");
    }
    golden("logic-changes-bridge.ndjson", &without_nonces(&normalized(&lab.simulator.transcript())));
    lab.simulator.stop();
}

fn marker_identity_edits() -> Vec<(&'static str, Value)> {
    vec![
        ("name", json!(format!("{}producer replacement", "A".repeat(300)))),
        ("length", json!(24.0)),
        ("length", json!("infinite")),
        ("text", json!("producer's visible marker note")),
        ("columns", json!(["Lock", "Position", "Length", "Name", "Text"])),
        ("ax", json!({"role":"AXRow", "subrole":"AXTableRow", "id":"replacement-row"})),
        ("other", json!({"Lock":{"role":"AXCell", "children":[], "value":"producer locked marker"}})),
    ]
}

fn expose_marker_text(script: &LogicScript) {
    script.set_marker(1, "columns", json!(["Lock", "Position", "Name", "Length", "Text"]));
    script.set_marker(1, "text", json!("original visible text"));
}

#[tokio::test]
async fn an_added_marker_is_never_deleted_after_a_same_position_identity_or_list_change() {
    for (field, value) in marker_identity_edits() {
        let mut lab = Lab::start().await;
        expose_marker_text(&lab.script);
        let (add, applied) = lab.make("marker-add", json!({"positionBeats":16}), "add").await;
        assert_eq!(applied["undo"]["restore"]["mode"], "value");
        assert!(applied.get("material").is_none(), "private identities stay in the host journal");
        lab.script.set_marker(1, field, value);
        let identity = lab.script.marker_identity(1);
        let titles = lab.script.undo_titles();
        for (key, discard) in [("undo", false), ("discard", true)] {
            let (refused, error) = lab.undo(&add, key, discard).await;
            assert!(error && refused["code"] == "changed", "{field}, discard={discard}: {refused}");
            assert_eq!(lab.script.marker_identity(1), identity);
            assert_eq!(lab.script.markers().len(), 2);
            assert_eq!(lab.script.undo_titles(), titles, "refusal made no native undo step");
        }
        lab.simulator.stop();
    }
    // An unrelated row changing also invalidates the captured surrounding list.
    let mut lab = Lab::start().await;
    expose_marker_text(&lab.script);
    let (add, _) = lab.make("marker-add", json!({"positionBeats":16}), "add").await;
    lab.script.set_marker(2, "name", json!("a producer's unrelated marker"));
    let (refused, error) = lab.undo(&add, "undo", true).await;
    assert!(error && refused["code"] == "changed", "{refused}");
    assert_eq!(lab.script.markers().len(), 2);
    lab.simulator.stop();
}

#[tokio::test]
async fn marker_move_fences_and_restores_bind_raw_row_and_complete_list_identity() {
    for (field, value) in marker_identity_edits() {
        let mut lab = Lab::start().await;
        expose_marker_text(&lab.script);
        let preview = lab.preview("marker-move", json!({"ref":"1:marker:1", "positionBeats":40})).await;
        let before = lab.script.marker_identity(1);
        lab.script.set_marker(1, field, value.clone());
        let changed = lab.script.marker_identity(1);
        let (refused, error) = lab.apply(&preview, "apply").await;
        assert!(error && refused["code"] == "changed", "{field}: {refused}");
        assert_eq!(lab.script.marker_identity(1), changed);
        assert!(lab.script.undo_titles().is_empty());
        lab.simulator.stop();

        let mut lab = Lab::start().await;
        expose_marker_text(&lab.script);
        let (moved, applied) = lab.make("marker-move", json!({"ref":"1:marker:1", "positionBeats":40}), "move").await;
        assert_eq!(moved["prior"]["identity"][0], before.unwrap());
        assert_eq!(applied["undo"]["restore"]["mode"], "value");
        lab.script.set_marker(1, field, value);
        let identity = lab.script.marker_identity(1);
        let titles = lab.script.undo_titles();
        for (key, discard) in [("undo", false), ("discard", true)] {
            let (refused, error) = lab.undo(&moved, key, discard).await;
            assert!(error && refused["code"] == "changed", "{field}, discard={discard}: {refused}");
            assert_eq!(lab.script.marker_identity(1), identity);
            assert_eq!(lab.script.markers()[0].1, 40.0);
            assert_eq!(lab.script.undo_titles(), titles);
        }
        lab.simulator.stop();
    }
}

#[tokio::test]
async fn marker_already_restored_requires_the_full_prior_state_and_unknown_values_never_become_empty() {
    let mut lab = Lab::start().await;
    expose_marker_text(&lab.script);
    lab.script.set_marker(1, "length", json!("infinite"));
    let (moved, _) = lab.make("marker-move", json!({"ref":"1:marker:1", "positionBeats":40}), "move").await;
    lab.script.set_marker(1, "positionBeats", json!(32));
    let titles = lab.script.undo_titles();
    let (back, error) = lab.undo(&moved, "already", false).await;
    assert!(!error && back["display"]["title"] == "The marker is where it was", "{back}");
    assert_eq!(lab.script.undo_titles(), titles, "the exact prior state needs no write");
    lab.simulator.stop();

    let mut lab = Lab::start().await;
    expose_marker_text(&lab.script);
    let (moved, _) = lab.make("marker-move", json!({"ref":"1:marker:1", "positionBeats":40}), "move").await;
    lab.script.set_marker(1, "positionBeats", json!(32));
    lab.script.set_marker(1, "name", json!("different marker at the old position"));
    let titles = lab.script.undo_titles();
    let (refused, error) = lab.undo(&moved, "not-already", true).await;
    assert!(error && refused["code"] == "changed", "{refused}");
    assert_eq!(lab.script.undo_titles(), titles);
    assert_eq!(lab.script.markers()[0].0, "different marker at the old position");
    lab.simulator.stop();

    // Unavailable Text (null) is not visible empty Text, even though M2's old read row says "" for both.
    let mut lab = Lab::start().await;
    let (moved, applied) = lab.make("marker-move", json!({"ref":"1:marker:1", "positionBeats":40}), "move").await;
    assert_eq!(applied["undo"]["restore"]["mode"], "manual");
    assert!(lab.script.marker_identity(1).unwrap()["text"].is_null());
    expose_marker_text(&lab.script);
    lab.script.set_marker(1, "text", json!(""));
    let (refused, error) = lab.undo(&moved, "text-visibility", true).await;
    assert!(error && refused["code"] == "failed", "historically unknown Text cannot become known: {refused}");
    lab.script.set_marker(1, "length", Value::Null);
    let (refused, error) = lab.undo(&moved, "unreadable-length", true).await;
    assert!(error && refused["code"] == "failed", "{refused}");
    assert_eq!(lab.script.markers()[0].1, 40.0);
    lab.simulator.stop();
}

#[tokio::test]
async fn a_created_track_is_never_deleted_even_when_every_visible_field_appears_unchanged() {
    for (field, value) in [
        ("kind", json!("audio")),
        ("name", json!("producer's new name")),
        ("volumeDb", json!(-3.0)),
        ("color", json!("#123456")),
        ("monitoring", json!(true)),
        ("inputs", json!(["Input 2"])),
        ("output", json!("Bus 7")),
        ("plugins", json!([{"name":"a producer's instrument", "state":"changed"}])),
        ("sends", json!([{"destination":"Bus 4", "level":-6.0}])),
        ("regions", json!([{"name":"producer's recording"}])),
        ("monitoring", Value::Null),
        ("automation", json!([{"hidden":true, "value":0.5}])),
    ] {
        let mut lab = Lab::start().await;
        let (add, applied) = lab.make("track-add", json!({"name":"Pad", "kind":"instrument"}), "add").await;
        assert_eq!(applied["undo"]["restore"]["mode"], "manual");
        lab.script.set_track(3, field, value);
        let material = lab.script.track(3);
        let titles = lab.script.undo_titles();
        for (key, discard) in [("undo", false), ("discard", true)] {
            let (refused, error) = lab.undo(&add, key, discard).await;
            assert!(error && matches!(refused["code"].as_str(), Some("changed" | "failed")), "{field}, discard={discard}: {refused}");
            assert_eq!(lab.script.track_count(), 3);
            assert_eq!(lab.script.track(3), material);
            assert_eq!(lab.script.undo_titles(), titles);
        }
        lab.simulator.stop();
    }
}

#[tokio::test]
async fn an_unchanged_created_track_still_requires_manual_undo_including_discard() {
    for kind in ["audio", "instrument", "external"] {
        let mut lab = Lab::start().await;
        let (add, applied) = lab.make("track-add", json!({"name":"Pad", "kind":kind}), "add").await;
        assert_eq!(applied["undo"]["restore"]["mode"], "manual");
        for field in ["selected", "focused", "enabled"] {
            lab.script.set_track(3, field, json!(true));
        }
        assert_eq!(applied["undo"]["restore"]["mode"], "manual");
        let (undone, error) = lab.undo(&add, "undo", true).await;
        assert!(error && undone["code"] == "failed", "{kind}: {undone}");
        assert_eq!((lab.script.epoch(), lab.script.track_count()), (2, 3));
        lab.simulator.stop();
    }
}

#[tokio::test]
async fn a_created_tracks_missing_or_incomplete_undo_material_never_becomes_permission_to_delete() {
    let mut lab = Lab::start().await;
    let (add, applied) = lab.make("track-add", json!({"name":"Pad", "kind":"instrument"}), "add").await;
    let before = lab.script.track(3);
    let titles = lab.script.undo_titles();
    for material in [Value::Null, json!({"track": {"name":"Pad", "kind":"instrument", "header":{}, "regions":[]}})] {
        let response = (lab.script.respond())(
            "change_restore",
            &json!({
                "kind":"track-add", "target":add["target"], "prior":add["prior"], "applied":applied["applied"],
                "created":applied["created"], "material":material, "desc":"an explicit direct-client undo", "discard":true
            }),
        )
        .unwrap();
        assert!(matches!(response, Err((code, _)) if code == "failed"));
        assert_eq!(lab.script.track_count(), 3);
        assert_eq!(lab.script.track(3), before);
        assert_eq!(lab.script.undo_titles(), titles);
    }
    lab.simulator.stop();
}

#[tokio::test]
async fn showing_text_later_never_recovers_unknown_original_marker_contents_even_when_the_position_is_back() {
    for kind in ["marker-add", "marker-move"] {
        let mut lab = Lab::start().await;
        let asked = if kind == "marker-add" { json!({"positionBeats":16}) } else { json!({"ref":"1:marker:1","positionBeats":40}) };
        let (preview, applied) = lab.make(kind, asked, "apply").await;
        assert_eq!(applied["undo"]["restore"]["mode"], "manual");
        expose_marker_text(&lab.script);
        for index in 1..=lab.script.markers().len() {
            lab.script.set_marker(index, "text", json!(""));
        }
        if kind == "marker-move" {
            lab.script.set_marker(1, "positionBeats", json!(32));
        }
        let markers = lab.script.markers();
        let titles = lab.script.undo_titles();
        for (key, discard) in [("normal", false), ("discard", true)] {
            let (refused, error) = lab.undo(&preview, key, discard).await;
            assert!(error && refused["code"] == "failed", "{kind}: {refused}");
            assert_eq!(lab.script.markers(), markers);
            assert_eq!(lab.script.undo_titles(), titles);
        }
        lab.simulator.stop();
    }
}
