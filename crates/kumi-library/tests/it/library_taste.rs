//! Producer habits from the source library tests and broader TypeScript fixtures.
use kumi_library::{
    sets::{SetSummary, SetTrack},
    taste::{build_taste, colour_name, taste_instructions, track_role},
};
use serde_json::{json, Value};
#[test]
fn habits_chains_counts_names_and_untrusted_instructions_match_typescript() {
    let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-library/library-taste-oracle.json")).unwrap();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let sets: Vec<SetSummary> = serde_json::from_value(case["sets"].clone()).unwrap();
        let taste = build_taste(&sets, Some(123456789));
        assert_eq!(serde_json::to_value(&taste).unwrap(), case["expected"], "case {index}");
        assert_eq!(taste_instructions(&taste, &Default::default()), case["instructions"], "case {index}");
        assert_eq!(
            taste_instructions(&taste, &["tempo".into(), "keys".into(), "names".into()].into()),
            case["forgotten"],
            "forgotten case {index}"
        );
    }
}
#[test]
fn every_palette_colour_and_track_role_matches_typescript() {
    let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-library/library-taste-oracle.json")).unwrap();
    for case in fixture["colours"].as_array().unwrap() {
        assert_eq!(json!(colour_name(case["index"].as_f64().unwrap())), case["expected"], "colour {}", case["index"]);
    }
    for case in fixture["roles"].as_array().unwrap() {
        let track: SetTrack = serde_json::from_value(case["track"].clone()).unwrap();
        assert_eq!(json!(track_role(&track)), case["expected"], "role {}", track.name);
    }
    for invalid in [f64::NAN, f64::INFINITY, 0.5] {
        assert_eq!(colour_name(invalid), None);
    }
}
