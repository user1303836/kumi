use ableton_mcp_server::{
    live::*,
    registry::{canonical_json, UNBOUNDED_CANONICAL_LIMITS},
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
/// Live never holds two Arrangement clips with one ref or identity, so the simulator mustn't either.
fn clips_are_unique(state: &Value, label: &str) {
    for field in ["ref", "objectIdentity"] {
        let mut seen = std::collections::HashSet::new();
        for row in state["arrangementClips"].as_array().into_iter().flatten() {
            assert!(seen.insert(row["clip"][field].to_string()), "{label}: two Arrangement clips with {field} {}", row["clip"][field]);
        }
    }
}
fn canonical(value: Value) -> String {
    canonical_json(&value, &UNBOUNDED_CANONICAL_LIMITS).unwrap()
}
fn normalize_paths(value: &mut Value) {
    match value {
        Value::Array(rows) => rows.iter_mut().for_each(normalize_paths),
        Value::Object(rows) => {
            for (key, value) in rows {
                if key == "path" {
                    if let Some(path) = value.as_str() {
                        let root = std::env::temp_dir().to_string_lossy().trim_end_matches(std::path::MAIN_SEPARATOR).to_owned();
                        if let Some(relative) = path.strip_prefix(&root) {
                            let suffix =
                                regex::Regex::new(r"kumi-simulated-render-\d+-").unwrap().replace(relative, "kumi-simulated-render-$$pid-");
                            *value = json!(format!("$tmp{suffix}").replace('\\', "/"));
                        }
                    }
                } else {
                    normalize_paths(value);
                }
            }
        }
        _ => {}
    }
}
#[test]
fn native_simulator_matches_typescript_state_results_events_and_authority_errors() {
    let scenarios: Vec<Value> = serde_json::from_str(include_str!("fixtures/simulator-oracle.json")).unwrap();
    for scenario in scenarios {
        let live = DeterministicLiveSimulator::new();
        let events = Rc::new(RefCell::new(vec![]));
        let sink = events.clone();
        let _unsubscribe = live.subscribe(Rc::new(move |event| sink.borrow_mut().push(event.clone()))).unwrap();
        let mut last_subscription: Option<String> = None;
        for step in scenario["steps"].as_array().unwrap() {
            let reference = LiveRef::from(step["ref"].as_str().unwrap_or(""));
            let before = kumi_common::time::now_ms();
            let mut result = match step["method"].as_str().unwrap() {
                "invoke" => {
                    let mut invocation = step["invocation"].clone();
                    if invocation["args"]["subscriptionId"] == "$last" {
                        invocation["args"]["subscriptionId"] = json!(last_subscription);
                    }
                    live.invoke(&serde_json::from_value(invocation).unwrap()).map(Some)
                }
                "addNote" => live.add_note(&reference, &serde_json::from_value(step["note"].clone()).unwrap()).map(Some),
                "external" => {
                    live.simulate_external_edit(&reference, step["property"].as_str().unwrap(), step["value"].clone()).map(|_| None)
                }
                "automation" => live.set_automation(&reference, &serde_json::from_value(step["point"].clone()).unwrap()).map(|_| None),
                "take" => live.add_take(&reference, step["take"].as_str().unwrap()).map(|_| None),
                "replaceState" => {
                    *live.state.borrow_mut() = step["value"].clone();
                    Ok(None)
                }
                "reconnect" => live.reconnect().map(|status| Some(serde_json::to_value(status).unwrap())),
                method => panic!("unknown test action {method}"),
            };
            if let Some(hash) = step["renderSha256"].as_str() {
                let output = result.as_ref().unwrap().as_ref().unwrap();
                let path = output["path"].as_str().unwrap();
                let bytes = std::fs::read(path).unwrap();
                use sha2::Digest;
                assert_eq!(hex::encode(sha2::Sha256::digest(&bytes)), hash);
                assert_eq!(bytes.len() as u64, output["bytes"].as_u64().unwrap());
                std::fs::remove_file(path).unwrap();
            }
            if let Ok(Some(value)) = &mut result {
                normalize_paths(value);
            }
            if step["invocation"]["operation"] == "observe.subscribe" && step.get("error").is_none() {
                let result = result.as_mut().unwrap().as_mut().unwrap();
                let id = result["subscriptionId"].as_str().unwrap();
                assert!(id.starts_with("obs_") && id.len() < 40);
                last_subscription = Some(id.to_owned());
                result["subscriptionId"] = step["result"]["subscriptionId"].clone();
            }
            if step["invocation"]["operation"] == "performance.read" {
                let sampled = result.as_mut().unwrap().as_mut().unwrap();
                let timestamp = sampled["sampledAt"].as_i64().unwrap();
                assert!(timestamp >= before && timestamp <= kumi_common::time::now_ms());
                sampled["sampledAt"] = step["result"]["sampledAt"].clone();
            }
            if let Some(error) = step["error"].as_str() {
                assert_eq!(result.unwrap_err().to_string(), error, "{}", scenario["name"]);
            } else {
                assert_eq!(
                    result.unwrap().map(canonical),
                    step.get("result").cloned().map(canonical),
                    "{}: {}",
                    scenario["name"],
                    step["method"]
                );
            }
        }
        let mut held = live.held_fire_buttons.borrow().iter().cloned().collect::<Vec<_>>();
        held.sort();
        assert_eq!(
            canonical(
                json!({"selectedNotes":*live.selected_notes.borrow(),"heldFireButtons":held,"shownMessages":*live.shown_messages.borrow()})
            ),
            canonical(scenario["effects"].clone()),
            "{} effects",
            scenario["name"]
        );
        assert_eq!(
            canonical(serde_json::to_value(live.snapshot().unwrap_or_else(|error| panic!("{}: {error}", scenario["name"]))).unwrap()),
            canonical(scenario["snapshot"].clone()),
            "{} state",
            scenario["name"]
        );
        clips_are_unique(&serde_json::to_value(live.snapshot().unwrap()).unwrap(), scenario["name"].as_str().unwrap());
        assert_eq!(
            canonical(serde_json::to_value(events.borrow().clone()).unwrap()),
            canonical(scenario["events"].clone()),
            "{} events",
            scenario["name"]
        );
        assert_eq!(
            canonical(serde_json::to_value(live.status().unwrap()).unwrap()),
            canonical(scenario["status"].clone()),
            "{} status",
            scenario["name"]
        );
    }
}
#[tokio::test]
async fn simulator_windows_parts_and_budgeted_views_match_whole_reads() {
    let live = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut state = live.state.borrow_mut();
        let first = state["tracks"][0].clone();
        for i in 1..20 {
            let mut row = first.clone();
            row["ref"] = format!("track:extra-{i}").into();
            row["objectIdentity"] = format!("simulator:track:extra-{i}").into();
            for key in ["clips", "clipSlots", "devices", "takeLanes"] {
                row[key] = json!([]);
            }
            state["tracks"].as_array_mut().unwrap().push(row);
        }
    }
    let full = live.snapshot_async(None, None).await.unwrap();
    assert_eq!(full.track_count, Some(20));
    assert_eq!(full.scene_count, Some(1));
    assert!(full.window.is_none());
    let focused = live.snapshot_view(Some(&LiveSnapshotRequest::focused(vec![0, 2]))).unwrap();
    assert_eq!(focused.tracks().iter().filter(|row| !row.is_light()).count(), 2);
    let parts = live.snapshot_view(Some(&LiveSnapshotRequest::of_parts(vec![LiveSnapshotPart::Set, LiveSnapshotPart::Playback]))).unwrap();
    assert!(parts.tracks.is_none());
    assert!(parts.scenes.is_none());
    assert!(parts.arrangement.is_none());
    assert!(parts.selection.is_none());
    assert_eq!(parts.set.unwrap().tempo, Some(120.0));
    live.read_budget_rows.set(Some(2));
    let adapter = live.clone();
    let views = LiveViews::new(move || adapter.clone());
    let paged = views.whole_set(None, None).await.unwrap();
    assert_eq!(paged.tracks, full.tracks);
    let focused = views.view(None, LiveViewScope::Indices(vec![0, 2, 4, 6]), None).await.unwrap();
    assert_eq!(focused.tracks().iter().filter(|row| !row.is_light()).count(), 4);
}
#[tokio::test]
async fn simulator_discovery_pages_are_bound_to_the_note_prefix() {
    let live = DeterministicLiveSimulator::new();
    for pitch in 40..44 {
        live.add_note(&"clip:clip-1".into(), &Note::new(pitch as f64, 1.0, 0.25, 90.0, 1.0)).unwrap();
    }
    live.discovery_budget_items.set(Some(2));
    let request = LiveDiscoveryRequest {
        parent: Some("clip:clip-1".into()),
        fields: Some(vec!["pitch".into()]),
        ..LiveDiscoveryRequest::of(LiveDiscoveryKind::Note)
    };
    let first = live.discover(&request).unwrap();
    assert!(first.truncated);
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items[0].keys().map(String::as_str).collect::<Vec<_>>(), vec!["pitch", "ref", "parentRef"]);
    let mut next = request.clone();
    next.cursor = first.next_cursor;
    let second = live.discover(&next).unwrap();
    assert_eq!(second.items.len(), 2);
    live.state.borrow_mut()["tracks"][0]["clips"][0]["notes"][0]["id"] = 999.into();
    assert_eq!(live.discover(&next).unwrap_err().to_string(), "stale discovery cursor");
}

#[test]
fn simulator_subscriptions_preserve_set_identity_and_reentrant_removal() {
    let live = Rc::new(DeterministicLiveSimulator::new());
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    let listener: LiveListener = Rc::new(move |_| sink.borrow_mut().push(1));
    let first = live.subscribe(listener.clone()).unwrap();
    let second = live.subscribe(listener).unwrap();
    live.reconnect().unwrap();
    assert_eq!(*seen.borrow(), vec![1]);
    second();
    live.reconnect().unwrap();
    assert_eq!(*seen.borrow(), vec![1]);
    first();
    let next = Rc::new(RefCell::new(None::<Unsubscribe>));
    let remove = next.clone();
    let _a = live
        .subscribe(Rc::new(move |_| {
            if let Some(unsubscribe) = remove.borrow_mut().take() {
                unsubscribe();
            }
        }))
        .unwrap();
    let sink = seen.clone();
    *next.borrow_mut() = Some(live.subscribe(Rc::new(move |_| sink.borrow_mut().push(2))).unwrap());
    live.reconnect().unwrap();
    assert_eq!(*seen.borrow(), vec![1]);
}

#[test]
fn simulator_invalid_authority_fields_match_typescript_errors() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/simulator-errors.json")).unwrap();
    let scenarios: Vec<Value> = serde_json::from_str(include_str!("fixtures/simulator-oracle.json")).unwrap();
    let mut mismatches = Vec::new();
    for case in cases {
        let live = DeterministicLiveSimulator::new();
        if let Some(name) = case["initialStateCase"].as_str() {
            *live.state.borrow_mut() = scenarios.iter().find(|scenario| scenario["name"] == name).unwrap()["steps"][0]["value"].clone();
        }
        let result = live.invoke(&serde_json::from_value(case["invocation"].clone()).unwrap());
        let actual = result.err().map(|e| e.to_string());
        if actual.as_deref() != case["error"].as_str() {
            mismatches.push(format!("{}: got {:?}, expected {}", case["name"], actual, case["error"]));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
