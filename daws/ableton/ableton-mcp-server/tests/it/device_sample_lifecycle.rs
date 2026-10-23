//! The initial sample of an inserted Simpler follows the verified import lifecycle.
kumi_testkit::fixture_paths!();
use ableton_mcp_server::{
    host::{mutations::result_body, McpHost, McpHostOptions},
    live::*,
};
use serde_json::{json, Value};
use std::{fs, rc::Rc};
fn wave() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(44u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(44100u32.to_le_bytes());
    bytes.extend(88200u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(8u32.to_le_bytes());
    bytes.extend([0u8; 8]);
    bytes
}
fn body(result: &Value) -> Value {
    assert_eq!(result["result"]["isError"], false, "{result}");
    result_body(result).unwrap()
}
#[tokio::test]
async fn initial_sample_uses_verified_copy_and_undo_cleans_only_owned_media() {
    for change in ["none", "source-edit", "staged-edit", "track-replaced"] {
        let temp = tempfile::tempdir().unwrap();
        let root = fixture_paths::native_path(&temp.path().canonicalize().unwrap());
        let original = root.join("original.wav");
        let managed = root.join("managed");
        let wave = wave();
        fs::write(&original, &wave).unwrap();
        let sim = Rc::new(DeterministicLiveSimulator::new());
        let host =
            McpHost::new(sim.clone(), McpHostOptions { import_staging_dir: Some(managed.to_string_lossy().into()), ..Default::default() })
                .unwrap();
        let preview = body(
            &host
                .live_device_preview_async(
                    &json!(1),
                    &json!({"action":"insert","deviceName":"Simpler","trackRef":"track:track-1","filePath":original,"allowedRoot":root}),
                )
                .await,
        );
        let transaction = preview["transactionId"].as_str().unwrap();
        let staged = std::path::PathBuf::from(preview["payload"]["samplePath"].as_str().unwrap());
        assert_ne!(staged, original);
        assert_eq!(fs::read(&staged).unwrap(), wave);
        match change {
            "source-edit" => fs::write(&original, b"later source edit").unwrap(),
            "staged-edit" => {
                let mut permissions = fs::metadata(&staged).unwrap().permissions();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    permissions.set_mode(0o600);
                }
                #[cfg(not(unix))]
                permissions.set_readonly(false);
                fs::set_permissions(&staged, permissions).unwrap();
                fs::write(&staged, b"tampered staging").unwrap();
            }
            "track-replaced" => sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("replacement-track"),
            _ => {}
        }
        let applied = host
            .live_device_apply_async(
                &json!(2),
                &json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":"apply-sample"}),
                None,
            )
            .await
            .unwrap();
        if ["staged-edit", "track-replaced"].contains(&change) {
            assert_eq!(applied["result"]["isError"], true, "{change}: {applied}");
            assert_eq!(sim.state.borrow()["tracks"][0]["devices"].as_array().unwrap().len(), 1);
            if change == "track-replaced" {
                assert!(!staged.exists());
            }
            assert!(original.exists());
            continue;
        }
        let applied = body(&applied);
        let reference = applied["result"]["ref"].as_str().unwrap();
        let device = sim.get(&LiveRef::from(reference)).unwrap().unwrap();
        assert_eq!(device["samplePath"], json!(staged));
        assert_eq!(fs::read(&staged).unwrap(), wave);
        let params = json!({"transactionId":transaction,"confirmation":"undo","idempotencyKey":"undo-sample"});
        body(
            &host
                .with_undo_watch(&json!(3), &params, async { Ok(host.undo_device_basic_async(&json!(3), &params, None).await) })
                .await
                .unwrap(),
        );
        assert!(!staged.exists());
        assert_eq!(sim.state.borrow()["tracks"][0]["devices"].as_array().unwrap().len(), 1);
        assert_eq!(fs::read(&original).unwrap(), if change == "source-edit" { b"later source edit".to_vec() } else { wave });
    }
}
