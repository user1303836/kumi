//! The Doofer, Splitter and Notepad changes the host makes from a device's preset data (M4 n-devices), through its tools
//! against a simulated Renoise tool that does what the real one does with them: reads the preset data (`preset_read`),
//! writes what the host composed while the device's shape is the one the host edited from, and puts the host's restore
//! back the same way.
#[path = "../common/mod.rs"]
mod common;

use common::Session;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::presets::shape_of;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const DOOFER: &str = include_str!("../fixtures/renoise/doofer_eq5_gainer.xml");

/// The Doofer on track 3 of a simulated song: its preset data, and every write the host asked for.
#[derive(Default)]
struct Song {
    xml: String,
    writes: Vec<(String, String)>,
    /// Reads Renoise fails to answer after its next apply (busy, say).
    reads_failing_after_apply: usize,
    failing: usize,
}

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    song: Arc<Mutex<Song>>,
    _simulator: Simulator,
    next: u64,
}

fn refusal(code: &str, message: &str) -> Option<Result<Value, (String, String)>> {
    Some(Err((code.into(), message.into())))
}

impl Lab {
    async fn start() -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Renoise);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let song = Arc::new(Mutex::new(Song { xml: DOOFER.into(), ..Song::default() }));
        let state = song.clone();
        let respond: daw_mcp_server::simulator::Respond = Arc::new(move |method, params| {
            let mut song = state.lock().unwrap();
            let at = json!({"track": 3, "dev": {"n": 2, "mark": "d1:4", "print": {"path": "Audio/Effects/Native/Doofer", "name": "Doofer"}}, "label": "Doofer"});
            match method {
                "preset_read" if song.failing > 0 => {
                    song.failing -= 1;
                    refusal("busy", "Renoise is busy")
                }
                "preset_read" => Some(Ok(json!({
                    "epoch": "e1", "layout": 0, "ref": "e1:device:3.2", "target": {"kind": "device", "track": 3, "device": 2}, "at": at,
                    "id": "track:Bass|Audio/Effects/Native/Doofer", "path": "Audio/Effects/Native/Doofer", "name": "Doofer", "label": "Doofer",
                    "trackName": "Bass", "presetData": song.xml, "shape": shape_of(&song.xml),
                }))),
                "change_apply" => {
                    let input = &params["input"];
                    if input["shape"].as_str() != Some(&shape_of(&song.xml)) {
                        return refusal("changed", "“Doofer on Bass” changed in Renoise since Kumi read it: read it again");
                    }
                    let xml = input["presetData"].as_str().unwrap().to_string();
                    song.writes.push((params["desc"].as_str().unwrap().into(), xml.clone()));
                    song.xml = xml;
                    song.failing = std::mem::take(&mut song.reads_failing_after_apply);
                    Some(Ok(json!({
                        "epoch": "e1", "layout": 0, "applied": {"shape": shape_of(&song.xml), "edit": input["edit"]}, "material": at,
                        "target": {"kind": "device", "track": 3, "device": 2}, "display": input["display"],
                        "undo": {"desc": params["desc"], "entry": true},
                    })))
                }
                "change_restore" => {
                    let write = &params["material"]["write"];
                    if write["shape"].as_str() != Some(&shape_of(&song.xml)) {
                        return refusal("changed", "“Doofer on Bass” changed in Renoise since Kumi read it");
                    }
                    let xml = write["presetData"].as_str().unwrap().to_string();
                    song.writes.push((params["desc"].as_str().unwrap().into(), xml.clone()));
                    song.xml = xml;
                    Some(Ok(
                        json!({"epoch": "e1", "layout": 0, "restored": {"shape": shape_of(&song.xml)}, "display": write["display"], "undo": {"desc": params["desc"], "entry": true}}),
                    ))
                }
                "read" => Some(Ok(json!({"epoch": "e1", "layout": 0, "items": [{
                    "ref": "e1:device:3.2", "index": 2, "name": "Doofer", "displayName": "Doofer", "path": "Audio/Effects/Native/Doofer",
                    "active": true, "preset": null, "paramCount": 8, "plugin": false, "isDoofer": true, "isSplitter": false,
                }]}))),
                "focus_body" => Some(Ok(json!({"epoch": "e1", "layout": 0, "ref": "e1:track:3", "name": "Bass", "nodes": [
                    {"ref": "e1:device:3.1", "name": "TrackVolPan", "enabled": true, "plugin": false, "container": false, "childNodes": []},
                    {"ref": "e1:device:3.2", "name": "Doofer", "enabled": true, "plugin": false, "container": true, "childNodes": []},
                ]}))),
                _ => None,
            }
        });
        let mut config = SimulatorConfig::renoise(secret, "/lab/Renoise")
            .with_methods(&["preset_read", "change_apply", "change_restore", "read", "focus_body"], respond);
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let session = Session::start(Daw::Renoise, home.path());
        Lab { _home: home, session, song, _simulator: simulator, next: 100 }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next += 1;
        self.session.tool_full(self.next, name, arguments, false).await
    }

    async fn preview(&mut self, kind: &str, input: Value) -> Value {
        let (preview, is_error) = self.tool("change_preview", json!({"kind": kind, "input": input, "epoch": "e1", "layout": 0})).await;
        assert!(!is_error, "{preview}");
        preview
    }

    async fn apply(&mut self, preview: &Value, key: &str) -> (Value, bool) {
        self.tool(
            "change_apply",
            json!({"transactionId": preview["transactionId"], "confirmation": preview["confirmation"], "idempotencyKey": key}),
        )
        .await
    }

    async fn undo(&mut self, preview: &Value, key: &str, discard: bool) -> (Value, bool) {
        self.tool(
            "change_undo",
            json!({"transactionId": preview["transactionId"], "confirmation": "undo", "idempotencyKey": key, "discard": discard}),
        )
        .await
    }

    fn xml(&self) -> String {
        self.song.lock().unwrap().xml.clone()
    }

    fn writes(&self) -> usize {
        self.song.lock().unwrap().writes.len()
    }

    fn edit_xml(&self, from: &str, to: &str) {
        let mut song = self.song.lock().unwrap();
        song.xml = song.xml.replacen(from, to, 1);
    }
}

#[tokio::test]
async fn a_macro_is_named_and_mapped_in_one_write_and_kumis_undo_writes_the_preset_data_back_exactly() {
    let mut lab = Lab::start().await;
    let preview = lab
        .preview("doofer-macro", json!({"ref": "e1:device:3.2", "macro": 1, "name": "Drive", "mappings": [{"device": 2, "param": "Gain"}]}))
        .await;
    assert_eq!(
        preview["display"]["title"],
        "Doofer on Bass: macro 1 named \u{201c}Drive\u{201d}, mapped to \u{201c}Gain\u{201d} of \u{201c}Gainer\u{201d}"
    );
    assert_eq!(lab.writes(), 0, "a preview writes nothing");
    let (applied, is_error) = lab.apply(&preview, "a1").await;
    assert!(!is_error && applied["state"] == "applied", "{applied}");
    assert!(lab.xml().contains("<Name>Drive</Name>") && lab.xml().contains("<DestChainIndex>0</DestChainIndex>"));
    let desc = &lab.song.lock().unwrap().writes[0].0.clone();
    assert!(desc.starts_with("Kumi: Doofer on Bass: macro 1 named") && desc.ends_with(')'), "{desc}");
    // The same key: once.
    assert_eq!(lab.apply(&preview, "a1").await, (applied, false));
    assert_eq!(lab.writes(), 1);
    // A knob moved by the producer (a value) since: the undo still puts back only what Kumi changed.
    lab.edit_xml("<Value>1000</Value>", "<Value>1500</Value>");
    let (undone, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(!is_error && undone["state"] == "undone", "{undone}");
    assert_eq!(lab.xml(), DOOFER.replacen("<Value>1000</Value>", "<Value>1500</Value>", 1));
    // Asked again: the same answer, nothing written.
    assert_eq!(lab.undo(&preview, "u1", false).await.0, undone);
    assert_eq!(lab.writes(), 2);
}

#[tokio::test]
async fn a_producers_edit_between_preview_and_apply_refuses_the_apply_and_nothing_is_written() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-macro", json!({"ref": "e1:device:3.2", "macro": 1, "name": "Drive"})).await;
    lab.edit_xml("<Name>Macro 1</Name>", "<Name>Theirs</Name>");
    let (refused, is_error) = lab.apply(&preview, "a1").await;
    assert!(is_error && refused["code"] == "changed", "{refused}");
    assert_eq!(lab.writes(), 0);
    assert!(lab.xml().contains("<Name>Theirs</Name>"));
    // Previewed again, it goes through.
    let again = lab.preview("doofer-macro", json!({"ref": "e1:device:3.2", "macro": 1, "name": "Drive"})).await;
    assert!(!lab.apply(&again, "a2").await.1);
}

#[tokio::test]
async fn a_device_added_inside_is_taken_out_by_kumis_undo_unless_the_devices_or_mappings_changed_since() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-device-add", json!({"ref": "e1:device:3.2", "path": "*LFO", "at": [1]})).await;
    assert_eq!(preview["display"]["title"], "Added \u{201c}*LFO\u{201d} to \u{201c}Doofer on Bass\u{201d} (place 1)");
    assert!(!lab.apply(&preview, "a1").await.1);
    assert!(lab.xml().contains("<LfoDevice type=\"LfoDevice\""));
    // The producer adds a mapping in Renoise since: structural, so Kumi can't tell what to take back.
    let after = lab.xml();
    lab.edit_xml("<Name>Macro 2</Name>", "<Name>Macro 2</Name>\n      <Mappings>\n        <Mapping>\n          <DestDeviceIndex>0</DestDeviceIndex>\n        </Mapping>\n      </Mappings>");
    let (refused, is_error) = lab.undo(&preview, "u1", true).await;
    assert!(is_error && refused["code"] == "changed", "{refused}");
    assert_eq!(lab.writes(), 1, "nothing written, discard or not");
    // Taken back by the producer: Kumi's undo goes through, and gives the Doofer as it was.
    lab.song.lock().unwrap().xml = after;
    let (undone, is_error) = lab.undo(&preview, "u2", false).await;
    assert!(!is_error, "{undone}");
    assert_eq!(lab.xml(), DOOFER);
}

#[tokio::test]
async fn an_undo_that_finds_it_back_already_writes_nothing_and_a_preview_never_sends_kept_xml_to_kumi() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-device-delete", json!({"ref": "e1:device:3.2", "at": [2]})).await;
    assert!(preview["prior"].get("xml").is_none(), "Kumi isn't sent the kept device: {}", preview["prior"]);
    assert!(!lab.apply(&preview, "a1").await.1);
    assert!(!lab.xml().contains("GainerDevice"));
    // The producer's Cmd-Z brought it back: Kumi's undo writes nothing and says so.
    lab.song.lock().unwrap().xml = DOOFER.into();
    let set = lab.preview("doofer-device-set", json!({"ref": "e1:device:3.2", "at": [2], "bypass": true})).await;
    assert!(!lab.apply(&set, "a2").await.1);
    lab.edit_xml(
        "<Value>0.0</Value>\n            <Visualization>Device only</Visualization>\n          </IsActive>\n          <Volume>",
        "<Value>1.0</Value>\n            <Visualization>Device only</Visualization>\n          </IsActive>\n          <Volume>",
    );
    let writes = lab.writes();
    let (back, is_error) = lab.undo(&set, "u1", false).await;
    assert!(!is_error && back["state"] == "undone", "{back}");
    assert!(back["display"]["note"].as_str().unwrap().contains("back already"), "{back}");
    assert_eq!(lab.writes(), writes, "nothing written");
}

#[tokio::test]
async fn a_doofer_read_by_ref_says_what_it_holds_its_preset_data_on_ask_and_the_dsp_body_gets_its_branches() {
    let mut lab = Lab::start().await;
    let (read, is_error) = lab.tool("read", json!({"ref": "e1:device:3.2", "epoch": "e1", "layout": 0})).await;
    assert!(!is_error, "{read}");
    let inside = &read["items"][0]["inside"];
    assert_eq!(inside["type"], "doofer");
    assert_eq!(inside["devices"][1]["name"], "Gainer");
    assert!(read["items"][0].get("presetData").is_none(), "only on ask");
    let (with_data, _) = lab.tool("read", json!({"ref": "e1:device:3.2", "epoch": "e1", "layout": 0, "presetData": true})).await;
    assert_eq!(with_data["items"][0]["presetData"], DOOFER);
    let (body, is_error) = lab.tool("focus_body", json!({"kind": "dsp", "ref": "e1:track:3", "layout": 0})).await;
    assert!(!is_error, "{body}");
    assert_eq!(body["nodes"][1]["childNodes"][0]["name"], "EQ 5");
    assert_eq!(body["nodes"][0]["childNodes"], json!([]));
}

#[tokio::test]
async fn kumis_undo_of_a_device_delete_puts_it_back_as_the_apply_found_it() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-device-delete", json!({"ref": "e1:device:3.2", "at": [2]})).await;
    // The producer turns the Gainer's volume between the preview and the apply (a value: the apply goes on).
    lab.edit_xml("<Volume>\n            <Value>1.0</Value>", "<Volume>\n            <Value>1.5</Value>");
    let turned = lab.xml();
    assert!(!lab.apply(&preview, "a1").await.1);
    let (undone, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(!is_error, "{undone}");
    assert_eq!(lab.xml(), turned, "the Gainer back as the apply took it out, at 1.5");
}

#[tokio::test]
async fn a_device_kumi_added_is_kept_as_renoise_wrote_it_for_kumis_undo() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-device-add", json!({"ref": "e1:device:3.2", "path": "Gainer", "at": [3]})).await;
    let (applied, is_error) = lab.apply(&preview, "a1").await;
    assert!(!is_error, "{applied}");
    assert!(applied["applied"]["edit"]["content"].as_str().is_some_and(|print| print.contains(':')), "{applied}");
}

#[tokio::test]
async fn a_device_kumi_added_whose_write_cant_be_read_back_is_one_kumis_undo_cant_tell_about() {
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-device-add", json!({"ref": "e1:device:3.2", "path": "Gainer", "at": [3]})).await;
    // Renoise answers neither read back after the apply.
    lab.song.lock().unwrap().reads_failing_after_apply = 2;
    let (applied, is_error) = lab.apply(&preview, "a1").await;
    assert!(!is_error, "{applied}");
    assert_eq!(applied["applied"]["edit"]["contentUnknown"], true, "{applied}");
    let (refused, is_error) = lab.undo(&preview, "u1", false).await;
    assert!(is_error && refused["code"] == "changed", "{refused}");
    // Read back on the second try: kept as usual.
    let mut lab = Lab::start().await;
    let preview = lab.preview("doofer-device-add", json!({"ref": "e1:device:3.2", "path": "Gainer", "at": [3]})).await;
    lab.song.lock().unwrap().reads_failing_after_apply = 1;
    let (applied, _) = lab.apply(&preview, "a1").await;
    assert!(applied["applied"]["edit"]["content"].is_string(), "{applied}");
}
