//! Kumi's own ears for REAPER and Renoise: their steps fit each DAW's kinds (from own_ears' tests: the app is where both
//! DAWs' integrations meet).
use kumi_agent::core::to_midi::MidiNote;
use kumi_agent::kernel::schema::problem;
use kumi_daw_own::ears::*;
use kumi_daw_renoise::changes::CHANGES as RENOISE;
use serde_json::{json, Value};
use std::path::Path;

fn slot(number: u64, pattern: &str, at: f64, beats: f64) -> Slot {
    Slot { number, pattern: pattern.into(), at, beats }
}

fn note(pitch: u8, start: f64, length: f64) -> MidiNote {
    MidiNote { pitch, start, length, velocity: 127 }
}

#[test]
fn notes_land_on_their_slots_lines_with_a_delay_and_pitched_ones_end_with_an_off() {
    // (two slots of 16 lines at LPB 4, from beat 4: the stretch starts at beat 6)
    let slots = vec![slot(2, "1:pattern:1", 4., 4.), slot(3, "1:pattern:2", 8., 4.)];
    let (patterns, left_out) =
        renoise_cells(&[note(60, 0., 1.), note(64, 0.1, 0.5), note(67, 2.5, 1.)], false, 6., 4., 4., &slots, "1:instrument:1").unwrap();
    assert_eq!(left_out, 0);
    let first = &patterns[0];
    assert_eq!(first.0, "1:pattern:1");
    // (beat 6 is line 8 of the first slot; beat 6.1 is line 8 at 0.4 of a line: delay 102)
    assert_eq!(first.1[0]["line"], 8);
    assert_eq!(first.1[0]["notes"][0], json!({"col": 1, "note": "C-5", "instrumentRef": "1:instrument:1", "volume": "80"}));
    assert_eq!(first.1[0]["notes"][1]["col"], 2);
    assert_eq!(first.1[0]["notes"][1]["delay"], 102);
    assert!(first.1.iter().any(|line| line["line"] == 12 && line["notes"][0]["note"] == "OFF"), "C-5 ends a beat on: {first:?}");
    // (beat 8.5 is line 2 of the second slot)
    assert_eq!(patterns[1].1[0]["line"], 2);
    for (pattern, lines) in &patterns {
        let input = json!({"patternRef": pattern, "trackRef": "1:track:9", "lines": lines});
        let kind = RENOISE.kinds.iter().find(|kind| kind.tool == "notes_write").unwrap();
        assert_eq!(problem(&Value::Object(kind.input_schema()), &input), None, "{input}");
    }
}

#[test]
fn stems_go_on_new_tracks_in_reaper_and_play_as_instruments_in_renoise() {
    use kumi_daw_reaper::changes::CHANGES as REAPER;
    let fits = |kinds: &[kumi_daw_own::changes::OwnKind], step: &Value| {
        let tool = step["tool"].as_str().unwrap();
        let kind = kinds.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}"));
        assert_eq!(problem(&Value::Object(kind.input_schema()), &step["input"]), None, "{step}");
    };
    let (drums, bass) = (Path::new("/tmp/Loop drums.wav"), Path::new("/tmp/Loop bass.wav"));
    let stems = [("Loop drums".to_string(), drums), ("Loop bass".to_string(), bass)];
    let reaper = reaper_stem_steps(&stems, 3, 8., 4.);
    assert_eq!(reaper.len(), 4);
    assert_eq!(reaper[3]["input"]["ref"], "@s1");
    reaper.iter().for_each(|step| fits(REAPER.kinds, step));
    // (the stretch from beat 6: line 8 of the slot from beat 4)
    let renoise = renoise_stem_steps(&stems, "Loop", 3, 6., 4., &[slot(2, "1:pattern:1", 4., 4.)]).unwrap();
    assert_eq!(renoise.len(), 6);
    assert_eq!(renoise[1]["input"], json!({"ref": "@i0", "name": "Loop drums"}));
    assert_eq!(renoise[5]["input"]["lines"][0]["line"], 8);
    assert_eq!(renoise[5]["input"]["lines"][0]["notes"][1], json!({"col": 2, "note": "C-4", "instrumentRef": "@i1"}));
    renoise.iter().for_each(|step| fits(RENOISE.kinds, step));
    assert!(renoise_stem_steps(&stems, "Loop", 3, 20., 4., &[slot(2, "1:pattern:1", 4., 4.)]).is_err(), "past the song");
}

#[test]
fn reapers_plan_and_renoises_track_fit_their_kinds() {
    use kumi_daw_reaper::changes::CHANGES as REAPER;
    let fits = |kinds: &[kumi_daw_own::changes::OwnKind], step: &Value| {
        let tool = step["tool"].as_str().unwrap();
        let kind = kinds.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}"));
        assert_eq!(problem(&Value::Object(kind.input_schema()), &step["input"]), None, "{step}");
    };
    for step in reaper_steps("Amen drums", 3, &[note(36, 0., 0.25), note(38, 1., 0.25)], 8., 4.) {
        fits(REAPER.kinds, &step);
    }
    fits(RENOISE.kinds, &json!({"tool": "track_add", "input": {"type": "sequencer", "name": "Amen drums", "index": 3}}));
}

#[test]
fn a_note_starting_while_another_sounds_takes_another_column() {
    let slots = vec![slot(1, "1:pattern:1", 0., 4.)];
    // (C-5 for two beats; E-5 starts a beat in, under it: column 2, and C-5's OFF stays in column 1)
    let (patterns, _) = renoise_cells(&[note(60, 0., 2.), note(64, 1., 2.)], false, 0., 4., 4., &slots, "1:instrument:1").unwrap();
    let lines = &patterns[0].1;
    let at = |line: u64| lines.iter().find(|row| row["line"] == line).cloned().unwrap_or(Value::Null);
    assert_eq!(at(4)["notes"][0]["col"], 2, "{lines:?}");
    assert_eq!(at(8)["notes"][0], json!({"col": 1, "note": "OFF"}), "{lines:?}");
    assert_eq!(at(12)["notes"][0], json!({"col": 2, "note": "OFF"}), "{lines:?}");
}

#[test]
fn a_stretch_over_two_slots_of_one_pattern_is_refused_and_drums_play_out() {
    let slots = vec![slot(1, "1:pattern:1", 0., 4.), slot(2, "1:pattern:1", 4., 4.)];
    let refused = renoise_cells(&[note(36, 0., 0.25)], true, 0., 8., 4., &slots, "1:instrument:1").unwrap_err();
    assert!(refused.starts_with("slots 01 and 02 play the same pattern"), "{refused}");
    let (patterns, _) =
        renoise_cells(&[note(36, 0., 0.25), note(42, 0., 0.25), note(38, 1., 0.25)], true, 0., 4., 4., &slots[..1], "1:instrument:1")
            .unwrap();
    assert!(!Value::Array(patterns[0].1.clone()).to_string().contains("OFF"), "drums play out: {:?}", patterns[0]);
    // (a column a key: kick 1, hat 3 on line 0; snare 2 on line 4)
    assert_eq!(
        patterns[0].1[0]["notes"].as_array().unwrap().iter().map(|cell| cell["col"].clone()).collect::<Vec<_>>(),
        [json!(1), json!(3)]
    );
    assert_eq!(patterns[0].1[1]["notes"][0]["col"], 2);
}

#[test]
fn a_pitched_note_ends_where_it_does_across_patterns_and_at_the_stretchs_end() {
    // (two whole patterns of 16 lines at LPB 4: the stretch is beats 0 to 8)
    let slots = vec![slot(1, "1:pattern:1", 0., 4.), slot(2, "1:pattern:2", 4., 4.)];
    let notes = [note(60, 3., 2.), note(62, 5., 0.1), note(64, 6., 2.)];
    let (patterns, _) = renoise_cells(&notes, false, 0., 8., 4., &slots, "1:instrument:1").unwrap();
    let lines = |pattern: &str| patterns.iter().find(|(p, _)| p == pattern).map(|(_, lines)| lines.clone()).unwrap_or_default();
    let at = |pattern: &str, line: u64| lines(pattern).into_iter().find(|row| row["line"] == line).unwrap_or(Value::Null);
    // (C-5 from line 12 of the first into the second: its OFF on the second's line 4, and the next notes keep off its column)
    assert_eq!(at("1:pattern:2", 4)["notes"][0], json!({"col": 1, "note": "OFF"}), "{patterns:?}");
    assert_eq!(at("1:pattern:2", 4)["notes"][1]["col"], 2, "D-5 starts while C-5 sounds: {patterns:?}");
    // (D-5 shorter than a line: its OFF on the next line)
    assert_eq!(at("1:pattern:2", 5)["notes"][0], json!({"col": 2, "note": "OFF"}), "{patterns:?}");
    // (E-5 sounds to the stretch's end: an OFF on its last line, delayed to the end)
    assert_eq!(at("1:pattern:2", 15)["notes"][0], json!({"col": 1, "note": "OFF", "delay": 255}), "{patterns:?}");
}
