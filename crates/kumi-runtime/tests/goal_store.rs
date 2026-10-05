//! Goal persistence and prompts.
use kumi_runtime::{core::contracts::StructuralMove as Structural, core::goal::*};
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!("support/goal-oracle.json")).unwrap()
}
#[test]
fn setup_and_structural_leap_prompts_match_typescript_exactly() {
    let data = fixture();
    for case in data["setup"].as_array().unwrap() {
        assert_eq!(goal_setup(case["goal"].as_str().unwrap()), case["text"]);
    }
    for case in data["leaps"].as_array().unwrap() {
        let state: GoalState = serde_json::from_value(case["state"].clone()).unwrap();
        let best: Option<Best> = serde_json::from_value(case["best"].clone()).unwrap();
        let gaps: Vec<String> = serde_json::from_value(case["gaps"].clone()).unwrap();
        let structural = case
            .get("structural")
            .map(|s| Structural { gap: s["gap"].as_str().unwrap().into(), r#move: s["move"].as_str().unwrap().into() });
        assert_eq!(goal_leap(&state, best.as_ref(), &gaps, case["stalled"].as_bool().unwrap(), structural.as_ref()), case["text"]);
    }
}
#[tokio::test(flavor = "current_thread")]
async fn goal_state_survives_restart_is_private_and_clear_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("goals");
    let store = create_goal_store(&folder);
    let state: GoalState = serde_json::from_value(fixture()["state"].clone()).unwrap();
    assert!(store.load("Set").await.unwrap().is_none());
    store.save("Set", &state).await.unwrap();
    assert_eq!(create_goal_store(&folder).load("Set").await.unwrap(), Some(state.clone()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(folder.join("Set.json")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(
        std::fs::read_to_string(folder.join("Set.json")).unwrap(),
        format!("{}\n", kumi_common::js::json::stringify(&serde_json::to_value(&state).unwrap()))
    );
    for (place, name) in [("", "unsaved"), ("../a", ".._a"), ("😀", "__"), ("é", "_")] {
        store.save(place, &state).await.unwrap();
        assert!(folder.join(format!("{name}.json")).exists());
        assert_eq!(store.load(place).await.unwrap(), Some(state.clone()));
    }
    store.save(&"a".repeat(200), &state).await.unwrap();
    assert!(folder.join(format!("{}.json", "a".repeat(120))).exists());
    store.clear("Set").await.unwrap();
    store.clear("Set").await.unwrap();
    assert!(store.load("Set").await.unwrap().is_none());
    assert!(std::fs::read_dir(&folder).unwrap().all(|p| !p.unwrap().file_name().to_string_lossy().starts_with(".goal-")));
}
#[tokio::test(flavor = "current_thread")]
async fn shallow_validation_matches_the_reference_and_corrupt_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_goal_store(dir.path());
    let file = dir.path().join("Set.json");
    for value in [
        json!({}),
        json!({"version":2,"goal":"x","request":{},"slots":[]}),
        json!({"version":1,"goal":4,"request":{},"slots":[]}),
        json!({"version":1,"goal":"x","request":false,"slots":[]}),
        json!({"version":1,"goal":"x","request":{},"slots":{}}),
    ] {
        std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.load_value("Set").await.is_none());
    }
    let minimal = json!({"version":1,"goal":"x","request":{},"slots":[],"extra":"preserved"});
    std::fs::write(&file, serde_json::to_vec(&minimal).unwrap()).unwrap();
    assert_eq!(store.load_value("Set").await, Some(minimal));
    std::fs::write(&file, "broken").unwrap();
    assert!(store.load("Set").await.unwrap().is_none());
}
