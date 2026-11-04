//! Preset-file changes require an explicitly capable running bridge, including REAPER's older named-preset route.
#[path = "../common/mod.rs"]
mod common;

use base64::Engine;
use common::Session;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Map, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[tokio::test]
async fn fabfilter_processor_and_controller_state_survives_each_guarded_daw_route() {
    let equalizer = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/fabfilter-pro-q-4/format-1/fixtures/init.vstpreset"));
    let synth = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/fabfilter-twin-3/format-1/fixtures/init.vstpreset"));
    for (daw, kind, bytes) in [
        (Daw::Reaper, "fx-preset", equalizer.as_slice()),
        (Daw::Reaper, "fx-preset", synth.as_slice()),
        (Daw::Renoise, "device-preset-data", equalizer.as_slice()),
        (Daw::Renoise, "plugin-preset-data", synth.as_slice()),
    ] {
        let expected = daw_bridge::vst3::read(bytes).unwrap();
        assert!(expected.controller.is_some(), "the fixture must exercise the separate controller");
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), daw);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let target_kind = if kind == "plugin-preset-data" { "instrument" } else { "device" };
        let target_ref = if kind == "plugin-preset-data" { "e1:instrument:1" } else { "e1:device:1.2" };
        let respond = Arc::new(move |method: &str, params: &Value| {
            if !matches!(method, "change_preview" | "change_apply" | "change_restore") {
                return None;
            }
            captured.lock().unwrap().push((method.to_owned(), params.clone()));
            Some(Ok(match method {
                "change_preview" => json!({"epoch":"e1","target":{"kind":target_kind,"ref":target_ref},
                    "prior":{"hash":"before","retained":"original processor and controller"},"fence":{"hash":"before"},
                    "display":{"title":"Load FabFilter preset"}}),
                "change_apply" => json!({"epoch":"e1","applied":{"hash":"after"},
                    "display":{"title":"Loaded FabFilter preset"},"undo":{"entry":true}}),
                _ => json!({"epoch":"e1","restored":{"hash":"before"},
                    "display":{"title":"Restored FabFilter preset"},"undo":{"entry":true}}),
            }))
        });
        let mut config = match daw {
            Daw::Reaper => SimulatorConfig::reaper(secret, "/lab/reaper"),
            _ => SimulatorConfig::renoise(secret, "/lab/renoise"),
        }
        .with_methods(&["change_preview", "change_apply", "change_restore"], respond);
        config.status_more = Some(Arc::new(|| Map::from_iter([("presetFiles".into(), json!(["vst3"]))])));
        config.discovery_dir = Some(paths.discovery());
        let _simulator = Simulator::start(config).await.unwrap();
        let mut session = Session::start(daw, home.path());
        let source = home.path().join("FabFilter.vstpreset");
        std::fs::write(&source, bytes).unwrap();
        let (record, failed) =
            session.tool_full(1, "change_preview", json!({"kind":kind,"input":{"ref":target_ref,"path":source}}), false).await;
        assert!(!failed, "{daw}/{kind}: {record}");
        // Apply must use the checked snapshot, even after the producer replaces
        // the source file with unrelated bytes.
        std::fs::write(&source, b"replaced after preview").unwrap();
        let (applied, failed) = session
            .tool_full(
                2,
                "change_apply",
                json!({"transactionId":record["transactionId"],
                "confirmation":record["confirmation"],"idempotencyKey":"apply"}),
                false,
            )
            .await;
        assert!(!failed && applied["state"] == "applied", "{daw}/{kind}: {applied}");
        let (undone, failed) = session
            .tool_full(
                3,
                "change_undo",
                json!({"transactionId":record["transactionId"],
                "confirmation":"undo","idempotencyKey":"undo"}),
                false,
            )
            .await;
        assert!(!failed && undone["state"] == "undone", "{daw}/{kind}: {undone}");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "preview, apply and guarded restore must reach the same bridge");
        for (_, request) in requests.iter().take(2) {
            assert_eq!(request["kind"], kind);
            let input = &request["input"];
            assert!(input.get("path").is_none());
            let delivered = base64::engine::general_purpose::STANDARD.decode(input["vst3Data"].as_str().unwrap()).unwrap();
            assert_eq!(delivered, bytes);
            let actual = daw_bridge::vst3::read(&delivered).unwrap();
            assert_eq!(actual.class_id, expected.class_id);
            assert_eq!(actual.component, expected.component);
            assert_eq!(actual.controller, expected.controller);
        }
        assert_eq!(requests[2].0, "change_restore");
        assert_eq!(requests[2].1["prior"]["retained"], "original processor and controller");
    }
}

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
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let sent = inputs.clone();
        let respond = Arc::new(move |method: &str, params: &Value| {
            if !matches!(method, "change_preview" | "change_apply" | "change_restore") {
                return None;
            }
            count.fetch_add(1, Ordering::SeqCst);
            if matches!(method, "change_preview" | "change_apply") {
                sent.lock().unwrap().push(params["input"].clone());
            }
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
        let file = home.path().join(if field == "preset" { "X.VSTPRESET" } else { "Created.vstpreset" });
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
        for (n, path) in [r"C:x.vstpreset", r"\x.vstpreset"].into_iter().enumerate() {
            let mut invalid = json!({"ref":"e1:device:1.2"});
            invalid[field] = path.into();
            let (answer, failed) = session.tool_full(20 + n as u64, "change_preview", json!({"kind":kind,"input":invalid}), false).await;
            assert!(failed && answer["code"] == "bad-params", "{daw}/{field}/{path}: {answer}");
            assert_eq!(requests.load(Ordering::SeqCst), 0, "relative/rooted paths never reach the DAW");
        }
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
        // Source files may change or disappear after preview. The stored checked bytes are the apply input.
        capable.store(true, Ordering::SeqCst);
        std::fs::remove_file(&file).unwrap();
        let (answer, failed) = session
            .tool_full(
                4,
                "change_apply",
                json!({"transactionId":record["transactionId"],
            "confirmation":record["confirmation"],"idempotencyKey":"snapshot"}),
                false,
            )
            .await;
        assert!(!failed, "{daw}: {answer}");
        let inputs = inputs.lock().unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0], inputs[1], "the exact validated snapshot reaches apply");
        assert!(inputs[0].get("path").is_none() && inputs[0].get("preset").is_none());
        assert!(inputs[0]["vst3Data"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(
            inputs[0]["presetName"],
            if daw == Daw::Reaper { file.file_name() } else { file.file_stem() }.unwrap().to_string_lossy().as_ref()
        );
    }
}

#[tokio::test]
async fn network_preset_names_are_refused_before_connecting_to_any_bridge() {
    let home = tempfile::tempdir().unwrap();
    let mut session = Session::start(Daw::Reaper, home.path());
    let mut request_id = 1;
    for field in ["path", "preset"] {
        for name in [
            r"\\untrusted\share\preset.vstpreset",
            "\0\\\\untrusted\\share\\preset.vstpreset\0",
            "C:\\P\\preset.vstpreset\0",
            "preset\nname",
            "C:\\P\\CON.vstpreset",
            "C:\\P\\COM9.vstpreset",
            "/tmp/x\u{7f}.vstpreset",
        ] {
            if !cfg!(windows) && (name.contains("CON.vstpreset") || name.contains("COM9.vstpreset")) {
                continue;
            }
            let mut input = json!({"ref":"r:fx:{test}"});
            input[field] = name.into();
            let (answer, failed) = session.tool_full(request_id, "change_preview", json!({"kind":"fx-preset","input":input}), false).await;
            request_id += 1;
            assert!(failed && answer["code"] == "bad-params", "{answer}");
            assert!(answer["error"].as_str().unwrap().contains("local"));
        }
    }
}

#[tokio::test]
async fn file_names_cannot_bypass_checked_loading_as_ordinary_program_names() {
    let home = tempfile::tempdir().unwrap();
    let mut session = Session::start(Daw::Reaper, home.path());
    for (n, name) in
        ["C:x", "C:\\P\\x.fxp", "/tmp/plugin.fxp", "x.vstpreset.", "x.vstpreset ", "C:x.vstpreset. ", ".\\x.vstpreset.", "./x.fxp"]
            .into_iter()
            .enumerate()
    {
        if !cfg!(windows) && matches!(name, "C:x" | "C:\\P\\x.fxp") {
            continue;
        }
        let (answer, failed) = session
            .tool_full(n as u64 + 1, "change_preview", json!({"kind":"fx-preset","input":{"ref":"r:fx:{test}","preset":name}}), false)
            .await;
        assert!(failed && answer["code"] == "bad-params", "{name}: {answer}");
        assert!(answer["error"].as_str().unwrap().contains("named program"), "{answer}");
    }
}

#[tokio::test]
async fn undo_labels_with_controls_are_refused_before_connecting() {
    let home = tempfile::tempdir().unwrap();
    let mut session = Session::start(Daw::Reaper, home.path());
    let mut id = 100;
    for label in ["Warm\0pad", "Warm\npad", "Warm\u{7f}pad"] {
        id += 1;
        let (answer, failed) = session.tool_full(id, "undo_step_begin", json!({"label":label}), false).await;
        assert!(failed && answer["code"] == "bad-params", "{answer}");
    }
}
