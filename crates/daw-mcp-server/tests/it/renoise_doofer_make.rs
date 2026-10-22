//! doofer-make through the host's tools (M4 n-devices): Kumi's host reads each device's preset data, composes the
//! container's, and sends the tool the devices with the shapes it read; the tool's undo takes it apart (the host sends the
//! record on as it is).
mod common;

use common::Session;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::presets::shape_of;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const EQ5: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FilterDevicePreset doc_version=\"14\">\n  <DeviceSlot type=\"Eq5Device\">\n    <IsMaximized>true</IsMaximized>\n    <Gain0>\n      <Value>-5.0</Value>\n    </Gain0>\n  </DeviceSlot>\n</FilterDevicePreset>\n";
const GAINER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FilterDevicePreset doc_version=\"14\">\n  <DeviceSlot type=\"GainerDevice\">\n    <IsMaximized>true</IsMaximized>\n    <Volume>\n      <Value>0.5</Value>\n    </Volume>\n  </DeviceSlot>\n</FilterDevicePreset>\n";

fn read_of(reference: &str, d: u32, name: &str, xml: &str) -> Value {
    json!({
        "epoch": "e1", "layout": 0, "ref": reference, "target": {"kind": "device", "track": 1, "device": d},
        "at": {"track": 1, "dev": {"n": d, "mark": "d1:0"}, "label": name}, "id": format!("track:Bass|Audio/Effects/Native/{name}"),
        "path": format!("Audio/Effects/Native/{name}"), "name": name, "label": name, "trackName": "Bass", "active": true,
        "presetData": xml, "shape": shape_of(xml), "print": "p", "parameters": [],
    })
}

#[tokio::test]
async fn a_doofer_is_made_through_the_host_from_the_devices_preset_data_and_undone_by_the_tool() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let asked: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let into = asked.clone();
    let respond: daw_mcp_server::simulator::Respond = Arc::new(move |method, params| {
        into.lock().unwrap().push((method.to_string(), params.clone()));
        match method {
            "preset_read" => match params["ref"].as_str() {
                Some("e1:device:1.2") => Some(Ok(read_of("e1:device:1.2", 2, "EQ 5", EQ5))),
                Some("e1:device:1.3") => Some(Ok(read_of("e1:device:1.3", 3, "Gainer", GAINER))),
                _ => Some(Err(("gone".into(), "that device isn't on the track any more".into()))),
            },
            "change_apply" => Some(Ok(json!({
                "epoch": "e2", "layout": 0, "applied": {"shape": "s", "count": 2}, "created": {"ref": "e2:device:1.2", "kind": "device"},
                "material": {"track": 1, "dev": {"n": 2, "mark": "d1:2"}, "label": "Tone", "devices": []},
                "target": {"kind": "device", "track": 1, "device": 2}, "display": params["input"]["display"], "undo": {"desc": params["desc"], "entry": true},
            }))),
            "change_restore" => Some(Ok(
                json!({"epoch": "e3", "layout": 0, "restored": {"count": 2}, "display": {"title": "Took “Tone on Bass” apart (put back)"}, "undo": {"desc": params["desc"], "entry": true}}),
            )),
            _ => None,
        }
    });
    let mut config =
        SimulatorConfig::renoise(secret, "/lab/Renoise").with_methods(&["preset_read", "change_apply", "change_restore"], respond);
    config.discovery_dir = Some(paths.discovery());
    let _simulator = Simulator::start(config).await.unwrap();
    let mut session = Session::start(Daw::Renoise, home.path());
    let input = json!({"trackRef": "e1:track:1", "deviceRefs": ["e1:device:1.2", "e1:device:1.3"], "name": "Tone", "macros": [{"name": "Level", "mappings": [{"device": 2, "param": 1}]}]});
    let (preview, is_error) =
        session.tool_full(1, "change_preview", json!({"kind": "doofer-make", "input": input, "epoch": "e1", "layout": 0}), false).await;
    assert!(!is_error, "{preview}");
    assert_eq!(preview["display"]["title"], "Made a Doofer of “EQ 5” and “Gainer” on “Bass”");
    assert!(asked.lock().unwrap().iter().all(|(method, _)| method == "preset_read"), "a preview writes nothing");
    let (applied, is_error) = session
        .tool_full(
            2,
            "change_apply",
            json!({"transactionId": preview["transactionId"], "confirmation": preview["confirmation"], "idempotencyKey": "k1"}),
            false,
        )
        .await;
    assert!(!is_error, "{applied}");
    let (_, sent) = asked.lock().unwrap().iter().rev().find(|(method, _)| method == "change_apply").cloned().unwrap();
    assert_eq!(sent["kind"], "doofer-make");
    let data = sent["input"]["presetData"].as_str().unwrap();
    assert!(data.contains("<Eq5Device type=\"Eq5Device\">") && data.contains("<GainerDevice type=\"GainerDevice\">"), "{data}");
    assert!(data.contains("<Name>Level</Name>"), "{data}");
    assert_eq!(sent["input"]["shapes"], json!([shape_of(EQ5), shape_of(GAINER)]), "the shapes it composed from, to fence the tool's write");
    assert_eq!(sent["input"]["name"], "Tone");
    let (undone, is_error) = session
        .tool_full(
            3,
            "change_undo",
            json!({"transactionId": preview["transactionId"], "confirmation": "undo", "idempotencyKey": "k2"}),
            false,
        )
        .await;
    assert!(!is_error, "{undone}");
    let (_, restore) = asked.lock().unwrap().iter().rev().find(|(method, _)| method == "change_restore").cloned().unwrap();
    assert_eq!(
        (&restore["kind"], &restore["material"]["label"]),
        (&json!("doofer-make"), &json!("Tone")),
        "the tool's own undo, with the record"
    );
}
