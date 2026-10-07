use kumi_runtime::{
    hands::MenuItem,
    integrations::ableton::{audition::*, live_command::*, plugin_tool::*},
};
use serde_json::{json, Value};
fn oracle() -> Value {
    serde_json::from_str(include_str!("support/audition-menu-oracle.json")).unwrap()
}
fn equivalent(a: &Value, b: &Value) {
    if let (Some(a), Some(b)) = (a.as_f64(), b.as_f64()) {
        assert!((a - b).abs() < 1e-10, "{a} != {b}");
    } else if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
        assert_eq!(a.len(), b.len());
        for (k, v) in a {
            equivalent(v, &b[k]);
        }
    } else if let (Some(a), Some(b)) = (a.as_array(), b.as_array()) {
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            equivalent(a, b);
        }
    } else {
        assert_eq!(a, b);
    }
}
#[test]
fn audition_requests_and_render_spans_match_source() {
    let data = oracle();
    for case in data["requests"].as_array().unwrap() {
        let result = match audition_request(case["input"].as_object().unwrap()) {
            Ok(request) => serde_json::to_value(request).unwrap(),
            Err(why) => json!(why),
        };
        equivalent(&result, &case["value"]);
    }
    for case in data["spans"].as_array().unwrap() {
        let args = case["args"].as_array().unwrap();
        let span = render_span(
            args[0].as_f64().unwrap(),
            args[1].as_f64().unwrap(),
            args[2].as_f64().unwrap(),
            args[3].as_f64().unwrap(),
            args[4].as_bool().unwrap(),
            args[5].as_f64(),
        );
        equivalent(&serde_json::to_value(span).unwrap(), &case["value"]);
    }
}
#[test]
fn menu_matching_shortcuts_and_model_schemas_match_source() {
    let data = oracle();
    let items: Vec<MenuItem> = serde_json::from_value(data["items"].clone()).unwrap();
    for case in data["menus"].as_array().unwrap() {
        let titles = serde_json::from_value::<Vec<String>>(case["titles"].clone()).unwrap();
        equivalent(&serde_json::to_value(find_item(&items, &titles)).unwrap(), &case["value"]);
    }
    for case in data["shortcuts"].as_array().unwrap() {
        let item = serde_json::from_value(case["item"].clone()).unwrap();
        assert_eq!(serde_json::to_value(shortcut(&item)).unwrap(), case["value"]);
    }
    let assets = &data["assets"];
    assert_eq!(*AUDITION_DESCRIPTION, assets["audition"]["description"]);
    assert_eq!(*AUDITION_SCHEMA, assets["audition"]["schema"].as_object().unwrap().clone());
    assert_eq!(*RENDER_DESCRIPTION, assets["audition"]["renderDescription"]);
    assert_eq!(*RENDER_SCHEMA, assets["audition"]["renderSchema"].as_object().unwrap().clone());
    assert_eq!(*LIVE_COMMAND_DESCRIPTION, assets["commands"]["description"]);
    assert_eq!(*LIVE_COMMAND_SCHEMA, assets["commands"]["schema"].as_object().unwrap().clone());
    let commands: serde_json::Map<String, Value> = COMMANDS.iter().map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap())).collect();
    assert_eq!(commands, assets["commands"]["commands"].as_object().unwrap().clone());
    assert_eq!(*PLUGIN_DESCRIPTION, assets["plugin"]["description"]);
    assert_eq!(*PLUGIN_SCHEMA, assets["plugin"]["schema"].as_object().unwrap().clone());
}
#[test]
fn crash_restore_journal_is_private_validated_and_best_effort() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("nested/main-restore.json");
    let store = restore_store(&file);
    assert!(store.load().is_none());
    let record = json!({"set":"opaque","volume":0.75,"at":100,"scratch":["one","two"],"extra":"kept"});
    assert!(store.save(&record));
    assert_eq!(Value::Object(store.load().unwrap()), record);
    // A write that can't finish (here its file beside the journal can't be made) says so, and leaves the journal
    // before it whole, with nothing beside it.
    let beside = std::path::PathBuf::from(format!("{}.{}.tmp", file.display(), std::process::id()));
    std::fs::create_dir(&beside).unwrap();
    assert!(!store.save(&json!({"set":"other","volume":0.1,"at":200})));
    assert_eq!(Value::Object(store.load().unwrap()), record);
    std::fs::remove_dir(&beside).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), kumi_common::js::json::file_text(&record));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    for value in [
        json!({"set":"x","volume":-1,"at":100}),
        json!({"set":"x","volume":1.1,"at":100}),
        json!({"set":1,"volume":0.5,"at":100}),
        json!({"set":"x","volume":0.5,"at":"100"}),
    ] {
        store.save(&value);
        assert!(store.load().is_none());
    }
    store.clear();
    store.clear();
    assert!(!file.exists());
    let invalid = restore_store(root.path());
    assert!(!invalid.save(&record));
    invalid.clear();
    assert!(root.path().exists());
    assert!(!std::path::PathBuf::from(format!("{}.{}.tmp", root.path().display(), std::process::id())).exists());
}
