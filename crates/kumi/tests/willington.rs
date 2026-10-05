//! /willington's switch: finding Willington in the bridge in Live, and turning its bindings on and off.
use ableton_mcp_server::delivery::{secret_permissions, SecretPermissions};
use kumi::willington::{Willington, WillingtonControl};
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::Path};

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn scripts_env(scripts: &Path) -> HashMap<String, String> {
    HashMap::from([("KUMI_REMOTE_SCRIPTS_DIR".to_string(), scripts.display().to_string())])
}

#[test]
fn willington_is_found_only_with_a_bridge_that_carries_it() {
    let folder = tempfile::tempdir().unwrap();
    let scripts = folder.path();
    assert_eq!(Willington::in_remote_scripts(scripts), None);
    put(&scripts.join("AbletonMcpBridge/__init__.py"), "");
    assert_eq!(Willington::in_remote_scripts(scripts), None);
    put(&scripts.join("AbletonMcpBridge/willington/WillingtonRuntime/__init__.py"), "");
    assert!(Willington::in_remote_scripts(scripts).is_some());
    // Installed beside the bridge by hand, as before Kumi carried it.
    fs::remove_dir_all(scripts.join("AbletonMcpBridge/willington")).unwrap();
    put(&scripts.join("WillingtonRuntime/__init__.py"), "");
    assert!(Willington::in_remote_scripts(scripts).is_some());
    fs::remove_file(scripts.join("AbletonMcpBridge/__init__.py")).unwrap();
    assert_eq!(Willington::in_remote_scripts(scripts), None, "Willington alone, without Kumi's bridge, isn't Kumi's to switch");
}

#[tokio::test(flavor = "current_thread")]
async fn the_switch_turns_every_binding_on_owner_only_and_off_by_going() {
    let folder = tempfile::tempdir().unwrap();
    let scripts = folder.path();
    put(&scripts.join("AbletonMcpBridge/__init__.py"), "");
    put(&scripts.join("AbletonMcpBridge/willington/WillingtonRuntime/__init__.py"), "");
    let control = WillingtonControl::new(scripts_env(scripts));
    let switch = scripts.join("AbletonMcpBridge/willington.json");
    assert_eq!((control.on)(), Some(false));
    (control.set)(true).await.unwrap();
    assert_eq!((control.on)(), Some(true));
    // Exactly the keys the bridge accepts, and only an owner-only file is one it reads.
    let written: Value = serde_json::from_slice(&fs::read(&switch).unwrap()).unwrap();
    assert_eq!(written, json!({"version":1,"followActions":true,"deviceTools":true,"rackZones":true,"enableWrites":true}));
    assert_eq!(secret_permissions(&switch), SecretPermissions::OwnerOnly);
    (control.set)(false).await.unwrap();
    assert_eq!((control.on)(), Some(false));
    assert!(!switch.exists());
    (control.set)(false).await.unwrap();
    // A switch that asks for no edits, or isn't JSON, is off; turning on replaces it.
    for text in [r#"{"version":1,"followActions":true,"deviceTools":true,"enableWrites":false}"#, "not json"] {
        fs::write(&switch, text).unwrap();
        assert_eq!((control.on)(), Some(false), "{text}");
        (control.set)(true).await.unwrap();
        assert_eq!((control.on)(), Some(true), "{text}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn without_willington_in_the_bridge_there_is_nothing_to_switch() {
    let folder = tempfile::tempdir().unwrap();
    put(&folder.path().join("AbletonMcpBridge/__init__.py"), "");
    let control = WillingtonControl::new(scripts_env(folder.path()));
    assert_eq!((control.on)(), None);
    assert!((control.set)(true).await.unwrap_err().contains("doesn't carry Willington"));
    assert!(!folder.path().join("AbletonMcpBridge/willington.json").exists());
}
