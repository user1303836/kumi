use kumi_daw_ableton::{
    actions::ACTIONS,
    more_changes::{bars, set_meter, span},
};
use kumi_types::contracts::TrackChip;
use serde_json::Value;
#[test]
fn transport_action_preparation_summaries_and_metadata_match_source() {
    let data: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/actions-oracle.json")).unwrap();
    assert_eq!(serde_json::to_value(&*ACTIONS).unwrap(), data["kinds"]);
    let track = |reference: &Value| {
        reference
            .as_str()
            .filter(|s| ["7:track:2", "track:1"].contains(s))
            .map(|_| TrackChip { name: "Bass".into(), color: Some("#00aaff".into()) })
    };
    for case in data["cases"].as_array().unwrap() {
        let kind = ACTIONS.iter().find(|action| action.tool == case["tool"]).unwrap();
        let input = case["input"].as_object().unwrap();
        assert_eq!(Value::Object(kind.prepare(input).unwrap()), case["prepared"], "{case}");
        let summary = kind.summarize(case["preview"].as_object().unwrap(), input, &track);
        assert_eq!(serde_json::to_value(summary).unwrap(), case["summary"], "{case}");
    }
}
#[test]
fn meter_aware_bar_positions_and_spans_keep_last_valid_meter() {
    let data: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/actions-oracle.json")).unwrap();
    for case in data["positions"].as_array().unwrap() {
        set_meter(case["numerator"].as_f64().unwrap(), case["denominator"].as_f64().unwrap());
        let beats = case["beats"].as_f64().unwrap();
        assert_eq!(bars(beats), case["bars"], "{case}");
        assert_eq!(span(beats), case["span"], "{case}");
    }
}
