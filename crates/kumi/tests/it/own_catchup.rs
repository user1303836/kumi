//! Catch-up drafting with REAPER's and Renoise's own words (from own_catchup's tests: the app is where both DAWs'
//! integrations meet).
use kumi_runtime::integrations::own_catchup::*;
use serde_json::{json, Value};

#[test]
fn effects_put_on_a_track_added_while_watching_are_drafted_on_it() {
    use daw_mcp_server::semantic::{diff, Snapshot};
    let reaper = |records: Vec<Value>| Snapshot {
        records: records.into_iter().map(|r| serde_json::from_value(r).unwrap()).collect(),
        ..Default::default()
    };
    let before = reaper(vec![json!({"k": "track", "id": "{A}", "n": "Vox", "o": 1})]);
    let after = reaper(vec![
        json!({"k": "track", "id": "{A}", "n": "Vox", "o": 1}),
        json!({"k": "track", "id": "{B}", "n": "Vox Bus", "o": 2}),
        json!({"k": "FX", "id": "{F1}", "n": "ReaComp", "o": 0, "p": "{B}", "v": [0.5, 0.2], "f": {"plugin": "VST: ReaComp (Cockos)"}}),
    ]);
    let mut said = serde_json::to_value(diff(&before, &after, true, &Default::default()).changes).unwrap();
    // (the parameter the host names for it, as its name_moves does)
    said[1]["madeOf"]["parameters"] = json!([{"name": "Threshold", "value": {"value": 0.2, "shows": "-20 dB"}}]);
    let drafted = draft(&json!({"changes": said}), &kumi_daw_reaper::CATCH_UP).unwrap();
    assert_eq!(
        drafted["steps"],
        json!([
            {"tool": "track_add", "input": {"name": "Vox Bus"}, "as": "t1"},
            {"tool": "fx_add", "input": {"name": "VST: ReaComp (Cockos)", "ref": "@t1"}, "as": "fx1"},
            {"tool": "fx_params", "input": {"values": [{"param": "Threshold", "normalized": 0.2}], "ref": "@fx1"}},
        ])
    );
    let renoise = json!({"changes": [
        {"change": "added", "kind": "track", "item": {"name": "Reverb", "order": 9}, "madeOf": {"name": "Reverb", "kind": "send"}},
        {"change": "added", "kind": "device", "item": {"name": "Reverb", "order": 2, "on": "Reverb"}, "madeOf": {"path": "Audio/Effects/Native/Reverb"}},
        {"change": "added", "kind": "track", "item": {"name": "Lead", "order": 3}, "madeOf": {"name": "Lead", "kind": "sequencer"}},
    ]});
    let drafted = draft(&renoise, &kumi_runtime::integrations::renoise::CATCH_UP).unwrap();
    assert_eq!(
        drafted["steps"],
        json!([
            {"tool": "track_add", "input": {"name": "Reverb", "type": "send"}, "as": "t1"},
            {"tool": "device_add", "input": {"path": "Audio/Effects/Native/Reverb", "ref": "@t1"}, "as": "fx1"},
            {"tool": "track_add", "input": {"name": "Lead"}, "as": "t2"},
        ])
    );
}
