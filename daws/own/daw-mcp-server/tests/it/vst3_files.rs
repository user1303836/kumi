//! Preset-file changes require an explicitly capable running bridge, including REAPER's older named-preset route.
#[path = "../common/mod.rs"]
mod common;

use common::Session;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Map, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn old_bridges_never_receive_preset_files_even_through_reapers_named_preset_field() {
    for (daw, kind, field) in [
        (Daw::Reaper, "fx-preset", "path"),
        (Daw::Reaper, "fx-preset", "preset"),
        (Daw::Renoise, "device-preset-data", "path"),
        (Daw::Renoise, "plugin-preset-data", "path"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), daw);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let capable = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let respond = Arc::new(move |method: &str, _: &Value| {
            if !matches!(method, "change_preview" | "change_apply" | "change_restore") {
                return None;
            }
            count.fetch_add(1, Ordering::SeqCst);
            Some(Ok(match method {
                "change_preview" => json!({"epoch":"e1","target":{"kind":"device","ref":"e1:device:1.2"},"prior":{"hash":"before"},
                    "fence":{"hash":"before"},"display":{"title":"Loaded preset"}}),
                "change_apply" => {
                    json!({"epoch":"e1","applied":{"hash":"after"},"display":{"title":"Loaded preset"},"undo":{"entry":true}})
                }
                _ => json!({"epoch":"e1","restored":{"hash":"before"},"display":{"title":"Restored preset"},"undo":{"entry":true}}),
            }))
        });
        let mut config = match daw {
            Daw::Reaper => SimulatorConfig::reaper(secret, "/lab/reaper"),
            _ => SimulatorConfig::renoise(secret, "/lab/renoise"),
        }
        .with_methods(&["change_preview", "change_apply", "change_restore"], respond);
        let advertised = capable.clone();
        config.status_more = Some(Arc::new(move || {
            if advertised.load(Ordering::SeqCst) {
                Map::from_iter([("presetFiles".into(), json!(["vst3"]))])
            } else {
                Map::new()
            }
        }));
        config.discovery_dir = Some(paths.discovery());
        let _simulator = Simulator::start(config).await.unwrap();
        let mut session = Session::start(daw, home.path());
        let file = home.path().join("Created.vstpreset");
        let mut bytes = b"VST3".to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(b"00112233445566778899AABBCCDDEEFF");
        bytes.extend(51u64.to_le_bytes());
        bytes.extend(b"abcList");
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(b"Comp");
        bytes.extend(48u64.to_le_bytes());
        bytes.extend(3u64.to_le_bytes());
        std::fs::write(&file, bytes).unwrap();
        let mut input = json!({"ref":"e1:device:1.2"});
        input[field] = file.to_string_lossy().into_owned().into();
        let preview = json!({"kind":kind,"input":input});
        let (answer, failed) = session.tool_full(1, "change_preview", preview.clone(), false).await;
        assert!(failed && answer["code"] == "bridge-update-required", "{daw}: {answer}");
        assert!(answer["error"].as_str().unwrap().contains("restart"));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        capable.store(true, Ordering::SeqCst);
        let (record, failed) = session.tool_full(2, "change_preview", preview, false).await;
        assert!(!failed, "{record}");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        capable.store(false, Ordering::SeqCst);
        let (answer, failed) = session
            .tool_full(
                3,
                "change_apply",
                json!({"transactionId":record["transactionId"],
            "confirmation":record["confirmation"],"idempotencyKey":"attempt"}),
                false,
            )
            .await;
        assert!(failed && answer["code"] == "bridge-update-required", "{answer}");
        assert_eq!(requests.load(Ordering::SeqCst), 1, "a downgraded/restarted bridge receives no mutation");
    }
}

#[tokio::test]
async fn network_preset_names_are_refused_before_connecting_to_any_bridge() {
    let home = tempfile::tempdir().unwrap();
    let mut session = Session::start(Daw::Reaper, home.path());
    for (n, field) in ["path", "preset"].iter().enumerate() {
        let mut input = json!({"ref":"r:fx:{test}"});
        input[*field] = r"\\untrusted\share\preset.vstpreset".into();
        let (answer, failed) = session.tool_full(n as u64 + 1, "change_preview", json!({"kind":"fx-preset","input":input}), false).await;
        assert!(failed && answer["code"] == "bad-params", "{answer}");
        assert!(answer["error"].as_str().unwrap().contains("local"));
    }
}
