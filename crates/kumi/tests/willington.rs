//! /willington's switch: finding Willington in the bridge in Live, and turning its bindings on and off.
use ableton_mcp_server::delivery::{secret_permissions, SecretPermissions};
use kumi::willington::{Willington, WillingtonControl, TURNED_OFF, TURNED_OFF_FOLLOW_STAYS, TURNED_ON, TURNED_ON_WITH_FOLLOW};
use kumi_runtime::integrations::ableton::willington::WillingtonSwitch;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

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
    assert_eq!((control.set)(true).await.unwrap(), TURNED_ON);
    assert_eq!((control.on)(), Some(true));
    // Exactly the keys the bridge accepts, and only an owner-only file is one it reads. Without a self-test
    // receipt Follow Action edits stay off, so their bindings, which Live can't unload, aren't loaded.
    let written: Value = serde_json::from_slice(&fs::read(&switch).unwrap()).unwrap();
    assert_eq!(written, json!({"version":1,"followActions":false,"deviceTools":true,"rackZones":true,"enableWrites":true}));
    assert_eq!(secret_permissions(&switch), SecretPermissions::OwnerOnly);
    // Staged beside the bridge: nothing of the write is left among the bridge's files, or beside them.
    for folder in [scripts, &scripts.join("AbletonMcpBridge")] {
        assert!(folder.read_dir().unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".ableton-mcp-")));
    }
    assert_eq!((control.set)(false).await.unwrap(), TURNED_OFF);
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
async fn follow_actions_come_on_with_a_passing_self_test_for_the_bindings_the_bridge_loads() {
    let folder = tempfile::tempdir().unwrap();
    let scripts = folder.path();
    put(&scripts.join("AbletonMcpBridge/__init__.py"), "");
    put(&scripts.join("AbletonMcpBridge/willington/WillingtonRuntime/__init__.py"), "");
    let control = WillingtonControl::new(scripts_env(scripts));
    let switch = scripts.join("AbletonMcpBridge/willington.json");
    let follow = || serde_json::from_slice::<Value>(&fs::read(&switch).unwrap()).unwrap()["followActions"].clone();
    let receipt = |status: &str, digest: &str| format!(r#"{{"status": "{status}", "library_sha256": "{digest}"}}"#);
    // The bridge's copy, with a library for one Live build, as Willington's matrix lays it out.
    let library = b"Follow Actions library, b5";
    let digest = hex::encode(Sha256::digest(library));
    let bindings = scripts.join("AbletonMcpBridge/willington/WillingtonBindings");
    put(&bindings.join("build/live-12.4.15b5-arm64/libwillington.dylib"), std::str::from_utf8(library).unwrap());
    // A receipt that didn't pass is no receipt.
    put(&bindings.join("self-test.json"), &receipt("failed", &digest));
    assert_eq!((control.set)(true).await.unwrap(), TURNED_ON);
    assert_eq!(follow(), false);
    // Passed, but for a library that isn't there (one an update replaced since), or with no digest the bridge
    // could match: the bridge would leave Follow Action edits off, so their bindings aren't loaded.
    let replaced = hex::encode(Sha256::digest(b"Follow Actions library, an older build"));
    for stale in [replaced.as_str(), "0", &digest.to_uppercase()] {
        put(&bindings.join("self-test.json"), &receipt("passed", stale));
        assert_eq!((control.set)(true).await.unwrap(), TURNED_ON, "{stale}");
        assert_eq!(follow(), false, "{stale}");
    }
    put(&bindings.join("self-test.json"), &receipt("passed", &digest));
    assert_eq!((control.set)(true).await.unwrap(), TURNED_ON_WITH_FOLLOW);
    assert_eq!(follow(), true);
    // Turned off, Live keeps Follow Actions' bindings until it restarts: the producer is told.
    assert_eq!((control.set)(false).await.unwrap(), TURNED_OFF_FOLLOW_STAYS);
    // WillingtonBindings installed beside the bridge comes first on Python's path: its receipt and its library.
    let beside = scripts.join("WillingtonBindings");
    put(&beside.join("__init__.py"), "");
    assert_eq!((control.set)(true).await.unwrap(), TURNED_ON);
    assert_eq!(follow(), false);
    put(&beside.join("self-test.json"), &receipt("passed", &digest));
    assert_eq!((control.set)(true).await.unwrap(), TURNED_ON, "the receipt names a library only the bridge's copy has");
    let windows = b"Follow Actions library, Windows b5";
    put(&beside.join("willington_bindings.pyd"), std::str::from_utf8(windows).unwrap());
    put(&beside.join("self-test.json"), &receipt("passed", &hex::encode(Sha256::digest(windows))));
    assert_eq!((control.set)(true).await.unwrap(), TURNED_ON_WITH_FOLLOW);
    assert_eq!((control.set)(false).await.unwrap(), TURNED_OFF_FOLLOW_STAYS);
}

#[tokio::test(flavor = "current_thread")]
async fn the_switch_is_read_again_only_when_its_file_changes_and_is_just_on_while_the_bridge_loads() {
    let folder = tempfile::tempdir().unwrap();
    let scripts = folder.path();
    put(&scripts.join("AbletonMcpBridge/__init__.py"), "");
    put(&scripts.join("AbletonMcpBridge/willington/WillingtonRuntime/__init__.py"), "");
    let willington = Willington::in_remote_scripts(scripts).unwrap();
    let control = WillingtonControl::new(scripts_env(scripts));
    assert_eq!(willington.switch(), WillingtonSwitch::Off);
    (control.set)(true).await.unwrap();
    // Just written: the bridge may still be loading the bindings.
    assert_eq!(willington.switch(), WillingtonSwitch::JustOn);
    assert_eq!((control.on)(), Some(true));
    let switch = scripts.join("AbletonMcpBridge/willington.json");
    let set_time = |at: SystemTime| fs::File::options().write(true).open(&switch).unwrap().set_modified(at).unwrap();
    let earlier = SystemTime::now() - Duration::from_secs(60);
    set_time(earlier);
    assert_eq!(willington.switch(), WillingtonSwitch::On);
    // A new time: read again, and kept.
    assert_eq!((control.on)(), Some(true));
    // The same size and time, though changed: the menu, which asks every frame, keeps what it read.
    let text = fs::read_to_string(&switch).unwrap();
    let off = text.replace("\"enableWrites\": true", "\"enableWrites\": 0   ");
    assert_eq!(off.len(), text.len());
    fs::write(&switch, &off).unwrap();
    set_time(earlier);
    assert_eq!((control.on)(), Some(true));
    // A new time is a change: read again.
    set_time(SystemTime::now());
    assert_eq!((control.on)(), Some(false));
    assert_eq!(willington.switch(), WillingtonSwitch::Off);
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
