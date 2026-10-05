use ableton_mcp_server::midi_transforms::*;
use ableton_mcp_server::registry::{canonical_json, UNBOUNDED_CANONICAL_LIMITS};
use serde_json::Value;
fn canonical(value: Value) -> String {
    canonical_json(&value, &UNBOUNDED_CANONICAL_LIMITS).unwrap()
}

#[test]
fn all_transform_families_match_typescript_outcomes_errors_digests_and_diffs() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/midi-transform-oracle.json")).unwrap();
    for case in cases {
        let notes: Vec<Note> = serde_json::from_value(case["notes"].clone()).unwrap();
        let spec = MidiTransformSpec {
            r#type: case["spec"]["type"].as_str().unwrap().into(),
            params: case["spec"]["params"].as_object().unwrap().clone(),
        };
        let result = apply_midi_transform(&notes, &spec, case["clipLength"].as_f64());
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err().to_string(), error, "{}", spec.r#type);
            continue;
        }
        let result = result.unwrap();
        assert_eq!(canonical(serde_json::to_value(&result).unwrap()), canonical(case["expected"].clone()), "{}", spec.r#type);
        if let Some(digest) = case["contentDigest"].as_str() {
            assert_eq!(note_content_digest(&result.notes).unwrap(), digest, "{} content digest", spec.r#type);
            assert_eq!(note_identity_digest(&result.notes).unwrap(), case["identityDigest"], "{} identity digest", spec.r#type);
            assert_eq!(
                canonical(serde_json::to_value(diff_notes(&notes, &result.notes)).unwrap()),
                canonical(case["diff"].clone()),
                "{} diff",
                spec.r#type
            );
        }
    }
}

#[test]
fn stochastic_transforms_without_a_seed_draw_one_from_the_request() {
    let notes = vec![Note::new(60.0, 0.0, 1.0, 100.0, 1.0), Note::new(64.0, 1.0, 1.0, 100.0, 1.0)];
    for (kind, params) in [
        ("humanize-velocity", serde_json::json!({"maxDelta":12})),
        ("humanize-timing", serde_json::json!({"maxOffset":0.125})),
        ("ratchet", serde_json::json!({"subdivisions":4,"probability":0.6})),
        ("seeded-variation", serde_json::json!({"velocityMax":10,"timingMax":0.05,"probabilityDepth":0.4})),
    ] {
        let mut spec = MidiTransformSpec { r#type: kind.into(), params: params.as_object().unwrap().clone() };
        let first = apply_midi_transform(&notes, &spec, None).unwrap();
        assert!(first.seed.as_ref().unwrap().starts_with("auto-"));
        assert_eq!(first, apply_midi_transform(&notes, &spec, None).unwrap());
        spec.params.insert("seed".into(), Value::String(String::new()));
        assert!(apply_midi_transform(&notes, &spec, None).unwrap_err().to_string().contains("seed"));
    }
}

#[test]
fn chord_progression_takes_chords_as_its_tool_text_says() {
    let progression = |params: Value| {
        apply_midi_transform(
            &[],
            &MidiTransformSpec { r#type: "chord-progression".into(), params: params.as_object().unwrap().clone() },
            None,
        )
    };
    let symbols = progression(serde_json::json!({"symbols":["Cm","Ab","Eb","Bb"]})).unwrap();
    assert_eq!(progression(serde_json::json!({"chords":["Cm","Ab","Eb","Bb"]})).unwrap(), symbols);
    let numerals = progression(serde_json::json!({"numerals":["i","VI","III","VII"],"root":0,"scale":"minor"})).unwrap();
    assert_eq!(progression(serde_json::json!({"chords":["i","VI","III","VII"],"root":0,"scale":"minor"})).unwrap(), numerals);
    let both = progression(serde_json::json!({"chords":["Cm"],"symbols":["Cm"]})).unwrap_err();
    assert_eq!(both.to_string(), "exactly one of chords, numerals or symbols is required");
}
