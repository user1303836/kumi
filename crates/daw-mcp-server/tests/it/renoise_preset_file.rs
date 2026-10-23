//! device-preset-data with a preset file through the host's tools (M4 n-devices): Kumi's host reads an .xrdp from the
//! User Library and sends the tool its text and name; a file outside Renoise's libraries is refused before the tool hears
//! of it.
#[path = "../common/mod.rs"]
mod common;

use common::Session;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const PRESET: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FilterDevicePreset doc_version=\"14\">\n  <DeviceSlot type=\"CompressorDevice\">\n    <IsMaximized>true</IsMaximized>\n  </DeviceSlot>\n</FilterDevicePreset>\n";

#[tokio::test]
async fn a_preset_file_in_the_user_library_is_read_by_the_host_and_one_elsewhere_is_refused() {
    let home = tempfile::tempdir().unwrap();
    // (the libraries are found from the home folder, as on the producer's computer)
    std::env::set_var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }, home.path());
    let folder = home.path().join("Documents").join("Renoise").join("User Library").join("Effect Presets").join("Compressor");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Drums Hard.xrdp");
    std::fs::write(&file, PRESET).unwrap();
    let outside = home.path().join("Drums Hard.xrdp");
    std::fs::write(&outside, PRESET).unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let asked: Arc<Mutex<Vec<Value>>> = Arc::default();
    let into = asked.clone();
    let respond: daw_mcp_server::simulator::Respond = Arc::new(move |method, params| {
        if method != "change_preview" {
            return None;
        }
        into.lock().unwrap().push(params.clone());
        Some(Ok(json!({
            "epoch": "e1", "layout": 0, "target": {"kind": "device", "track": 1, "device": 2}, "prior": {"print": "p"},
            "fence": {"id": "track:Kick|Audio/Effects/Native/Compressor", "state": "s"},
            "display": {"title": format!("Loaded the preset “{}” into “Compressor on Kick”", params["input"]["presetName"].as_str().unwrap_or_default())},
        })))
    });
    let mut config = SimulatorConfig::renoise(secret, "/lab/Renoise").with_methods(&["change_preview"], respond);
    config.discovery_dir = Some(paths.discovery());
    let _simulator = Simulator::start(config).await.unwrap();
    let mut session = Session::start(Daw::Renoise, home.path());
    let preview = |path: &std::path::Path| json!({"kind": "device-preset-data", "input": {"ref": "e1:device:1.2", "path": path.to_string_lossy()}, "epoch": "e1", "layout": 0});
    let (answer, is_error) = session.tool_full(1, "change_preview", preview(&file), false).await;
    assert!(!is_error, "{answer}");
    assert_eq!(answer["display"]["title"], "Loaded the preset “Drums Hard” into “Compressor on Kick”");
    let sent = asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(sent["input"]["presetData"], PRESET, "the file's text");
    assert_eq!(sent["input"]["presetName"], "Drums Hard");
    assert!(sent["input"].get("path").is_none(), "{sent}");
    let (refused, is_error) = session.tool_full(2, "change_preview", preview(&outside), false).await;
    assert!(is_error, "{refused}");
    assert_eq!(refused["code"], "bad-params");
    assert_eq!(asked.lock().unwrap().len(), 1, "the tool never heard of the file outside the libraries");
    // A loader's file: checked here, and sent on by its resolved path.
    let chains = home.path().join("Documents").join("Renoise").join("User Library").join("Effect Chains");
    std::fs::create_dir_all(&chains).unwrap();
    let chain = chains.join("Bright Vox.xrnt");
    std::fs::write(&chain, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RenoiseDeviceChain doc_version=\"21\">\n</RenoiseDeviceChain>\n")
        .unwrap();
    let load = json!({"kind": "device-chain-load", "input": {"trackRef": "e1:track:1", "path": chain.to_string_lossy()}, "epoch": "e1", "layout": 0});
    let (answer, is_error) = session.tool_full(3, "change_preview", load, false).await;
    assert!(!is_error, "{answer}");
    let sent = asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(sent["kind"], "device-chain-load");
    assert_eq!(sent["input"]["file"], std::fs::canonicalize(&chain).unwrap().to_string_lossy().as_ref());
    assert_eq!(sent["input"]["fileName"], "Bright Vox");
    // One the caller names itself never reaches the tool.
    let named =
        json!({"kind": "device-chain-load", "input": {"trackRef": "e1:track:1", "file": "/tmp/x.xrnt"}, "epoch": "e1", "layout": 0});
    let (refused, is_error) = session.tool_full(4, "change_preview", named, false).await;
    assert!(is_error && refused["code"] == "bad-params", "{refused}");
    assert_eq!(asked.lock().unwrap().len(), 2);
    // A phrase script file: its text goes to the tool as the code.
    let scripts = home.path().join("Documents").join("Renoise").join("User Library").join("Phrase Scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("Arp.lua"), "return cycle(\"c4 e4 g4\")\n").unwrap();
    let script = json!({"kind": "phrase-script", "input": {"ref": "e1:phrase:1.1", "path": scripts.join("Arp.lua").to_string_lossy()}, "epoch": "e1", "layout": 0});
    let (answer, is_error) = session.tool_full(5, "change_preview", script, false).await;
    assert!(!is_error, "{answer}");
    let sent = asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!((&sent["input"]["code"], &sent["input"]["codeName"]), (&json!("return cycle(\"c4 e4 g4\")\n"), &json!("Arp")));
}
