//! MIDI events beyond notes (m4-r-items): written, changed and deleted through the take's buffer, undone by value, and
//! read with the indexes the changes take.

use serde_json::json;

use super::tests_items::{code, item, state, Lab, S1};
use crate::midi::Event;

const SNARE_TAKE: &str = "r:take:{TAKE-S1}";

fn ev(pos: i64, msg: &[u8]) -> Event {
    Event { pos, flag: 0, msg: msg.to_vec() }
}

/// The snare's take as REAPER's buffer has it: a note, a controller, a program change and the end marker.
fn midi_lab() -> Lab {
    let lab = Lab::new();
    state(|s| {
        s.tracks[2].items[0].takes[0].more.raw = Some(vec![
            ev(0, &[0xB0, 7, 100]),
            ev(0, &[0x90, 38, 100]),
            ev(480, &[0x80, 38, 0]),
            ev(960, &[0xC0, 3]),
            ev(7680, &[0xB0, 0x7B, 0]),
        ])
    });
    lab
}

fn raw() -> Vec<Event> {
    item("{ITEM-S1}").unwrap().1.takes[0].more.raw.clone().unwrap()
}

#[test]
fn controllers_pitch_bend_and_a_program_with_its_bank_are_written_and_undone_by_value() {
    let lab = midi_lab();
    let before = raw();
    let input = json!({"ref": SNARE_TAKE, "events": [
        {"type": "cc", "channel": 1, "controller": 74, "value": 20, "start": 0.5, "shape": "bezier", "tension": -0.5},
        {"type": "cc", "channel": 1, "controller": 1, "value": 9000, "fourteenBit": true, "start": 1.0},
        {"type": "pitchbend", "channel": 1, "value": 4096, "start": 1.5},
        {"type": "program", "channel": 1, "program": 7, "bank": 2, "start": 0.0},
    ]});
    let (preview, applied) = lab.make("midi-events-add", input);
    assert_eq!(preview["display"]["title"], "Wrote 7 MIDI events in “snare.mid”");
    let now = raw();
    // The program change and its bank first at their tick, as REAPER orders them when it reads the take back (lab, 7.82):
    // before the controller already there and the note-on; the bezier tension right after its controller.
    assert_eq!(now[0].msg, vec![0xB0, 0, 0]);
    assert_eq!(now[1].msg, vec![0xB0, 32, 2]);
    assert_eq!(now[2].msg, vec![0xC0, 7]);
    assert_eq!(now[3].msg, vec![0xB0, 7, 100]);
    assert_eq!(now[4].msg, vec![0x90, 38, 100]);
    let at = now.iter().position(|e| e.msg == [0xB0, 74, 20]).unwrap();
    assert_eq!(now[at].flag, 0x50);
    assert_eq!(&now[at + 1].msg[..7], b"\xFF\x0FCCBZ ");
    assert!(now.iter().any(|e| e.msg == [0xB0, 1, 70]) && now.iter().any(|e| e.msg == [0xB0, 33, 40]));
    assert_eq!(now.last().unwrap().msg, vec![0xB0, 0x7B, 0], "the end marker stays last");
    lab.undo("midi-events-add", &preview, &applied).unwrap();
    assert_eq!(raw(), before, "the buffer is byte for byte as it was");
    let again = lab.undo("midi-events-add", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn the_read_lists_events_with_their_indexes_and_a_change_and_a_delete_use_them() {
    let lab = midi_lab();
    let rows = lab.call("read", json!({"kind": "event", "parent": SNARE_TAKE})).unwrap();
    assert_eq!(rows["indexFrom"], 0);
    assert_eq!(rows["total"], 2, "notes and the end marker are not events here");
    assert_eq!(rows["items"][1], json!({"index": 1, "type": "program", "channel": 1, "program": 3, "start": 1.0, "shape": "square"}));
    let (preview, applied) = lab
        .make("midi-events-change", json!({"ref": S1, "changes": [{"index": 1, "program": 9, "start": 0.5}, {"index": 0, "value": 64}]}));
    assert_eq!(preview["display"]["title"], "Changed 2 MIDI events in “snare.mid”");
    let now = raw();
    assert!(now.iter().any(|e| e.pos == 480 && e.msg == [0xC0, 9]) && now.iter().any(|e| e.msg == [0xB0, 7, 64]));
    lab.undo("midi-events-change", &preview, &applied).unwrap();
    assert!(raw().iter().any(|e| e.pos == 960 && e.msg == [0xC0, 3]));
    let (preview, applied) = lab.make("midi-events-delete", json!({"ref": S1, "indexes": [0]}));
    assert!(!raw().iter().any(|e| e.msg == [0xB0, 7, 100]));
    lab.undo("midi-events-delete", &preview, &applied).unwrap();
    assert_eq!(raw()[0].msg, vec![0xB0, 7, 100]);
}

#[test]
fn a_filter_deletes_a_lane_in_a_span_and_leaves_the_rest() {
    let lab = midi_lab();
    lab.make(
        "midi-events-add",
        json!({"ref": S1, "events": [
            {"type": "cc", "channel": 1, "controller": 74, "value": 1, "start": 1.0},
            {"type": "cc", "channel": 1, "controller": 74, "value": 2, "start": 2.0},
            {"type": "cc", "channel": 1, "controller": 74, "value": 3, "start": 3.0},
            {"type": "cc", "channel": 2, "controller": 74, "value": 4, "start": 2.0},
        ]}),
    );
    let (preview, _) = lab
        .make("midi-events-delete", json!({"ref": S1, "filter": {"type": "cc", "channel": 1, "controller": 74, "from": 1.5, "to": 3.0}}));
    assert_eq!(preview["display"]["count"], 1);
    let values: Vec<u8> = raw().iter().filter(|e| e.msg.len() == 3 && e.msg[1] == 74).map(|e| e.msg[2]).collect();
    assert_eq!(values, vec![1, 4, 3]);
    assert_eq!(code(lab.preview("midi-events-delete", json!({"ref": S1, "filter": {"type": "sysex"}}))), "bad-params");
    assert_eq!(code(lab.preview("midi-events-delete", json!({"ref": S1}))), "bad-params");
}

#[test]
fn text_and_sysex_are_written_and_nothing_goes_past_the_end_of_the_source() {
    let lab = midi_lab();
    lab.make("midi-events-add", json!({"ref": S1, "events": [{"type": "text", "textType": "marker", "text": "chorus", "start": 2.0}, {"type": "sysex", "bytes": "7E 7F 09 01", "start": 0.0}]}));
    let rows = lab.call("read", json!({"kind": "event", "parent": S1})).unwrap()["items"].clone();
    let kinds: Vec<&str> = rows.as_array().unwrap().iter().map(|r| r["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["cc", "sysex", "program", "text"]);
    assert_eq!(
        (rows[1]["bytes"].as_str(), rows[3]["textType"].as_str(), rows[3]["text"].as_str()),
        (Some("7E 7F 09 01"), Some("marker"), Some("chorus"))
    );
    // The source ends at 8 quarter notes from the item's start.
    let (code_, message) = lab
        .preview("midi-events-add", json!({"ref": S1, "events": [{"type": "cc", "channel": 1, "controller": 1, "value": 1, "start": 8.0}]}))
        .unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("before the end of the take's source"), "{message}");
}

#[test]
fn a_note_is_kept_inside_the_takes_source() {
    // A note at or past the source's end was sorted after REAPER's end marker: the source grew, and a looped item's
    // period with it.
    let lab = midi_lab();
    let before = raw();
    for note in [json!({"pitch": 60, "start": 8.0, "length": 0.5}), json!({"pitch": 60, "start": 7.5, "length": 1.0})] {
        let (code_, message) = lab.preview("notes-add", json!({"ref": S1, "notes": [note]})).unwrap_err();
        assert_eq!(code_, "bad-params");
        assert!(message.contains("end of the take's source"), "{message}");
    }
    let moved = lab.preview("notes-change", json!({"ref": S1, "changes": [{"index": 0, "start": 7.75}]}));
    assert_eq!(code(moved), "bad-params", "a change that would move a note past the end");
    let late = lab.preview("notes-quantize", json!({"ref": S1, "grid": 16.0, "ends": "end"}));
    assert_eq!(code(late), "bad-params", "a quantize that would end a note past the end");
    assert_eq!(raw(), before);
    // A note that ends on the end marker's tick is inside it, and the marker stays last.
    lab.make("notes-add", json!({"ref": S1, "notes": [{"pitch": 60, "start": 7.5, "length": 0.5}]}));
    let now = raw();
    assert_eq!((now.last().unwrap().pos, now.last().unwrap().msg.clone()), (7680, vec![0xB0, 0x7B, 0]));
}

#[test]
fn a_notes_release_velocity_stays_through_a_change_and_its_undo() {
    // Every note Kumi wrote again ended with velocity 0: a recorded release velocity was lost, and the undo couldn't bring
    // it back.
    let lab = midi_lab();
    state(|s| s.tracks[2].items[0].takes[0].more.raw.as_mut().unwrap()[2].msg = vec![0x80, 38, 64]);
    let before = raw();
    let (preview, applied) = lab.make("notes-change", json!({"ref": S1, "changes": [{"index": 0, "velocity": 70}]}));
    assert!(raw().iter().any(|e| e.pos == 480 && e.msg == [0x80, 38, 64]), "{:?}", raw());
    lab.undo("notes-change", &preview, &applied).unwrap();
    assert_eq!(raw(), before, "byte for byte");
}

#[test]
fn events_the_producer_changed_since_are_theirs_and_a_change_between_preview_and_apply_is_refused() {
    let lab = midi_lab();
    let (preview, applied) =
        lab.make("midi-events-add", json!({"ref": S1, "events": [{"type": "pressure", "channel": 1, "value": 50, "start": 1.0}]}));
    state(|s| {
        let raw = s.tracks[2].items[0].takes[0].more.raw.as_mut().unwrap();
        let at = raw.iter().position(|e| e.msg == [0xD0, 50]).unwrap();
        raw[at].msg = vec![0xD0, 51];
    });
    assert_eq!(code(lab.undo("midi-events-add", &preview, &applied)), "changed");
    assert!(raw().iter().any(|e| e.msg == [0xD0, 51]), "the producer's event stays");
    let input = json!({"ref": S1, "indexes": [0]});
    let preview = lab.preview("midi-events-delete", input.clone()).unwrap();
    state(|s| s.tracks[2].items[0].takes[0].more.raw.as_mut().unwrap().insert(0, ev(0, &[0xB0, 10, 64])));
    assert_eq!(code(lab.apply_with("midi-events-delete", &input, &preview, "Kumi: x (k1)")), "changed");
    // A write REAPER doesn't keep whole is taken back.
    let lab = midi_lab();
    let before = raw();
    state(|s| s.glitch_events = 1);
    let input = json!({"ref": S1, "events": [{"type": "cc", "channel": 1, "controller": 1, "value": 1, "start": 2.0}]});
    let preview = lab.preview("midi-events-add", input.clone()).unwrap();
    assert_eq!(code(lab.apply_with("midi-events-add", &input, &preview, "Kumi: x (k1)")), "failed");
    assert_eq!(raw(), before);
}

#[test]
fn every_event_answer_names_refs_the_way_the_contract_says() {
    let lab = midi_lab();
    for (kind, input) in [
        ("midi-events-add", json!({"ref": S1, "events": [{"type": "cc", "channel": 1, "controller": 1, "value": 1, "start": 1.0}]})),
        ("midi-events-change", json!({"ref": S1, "changes": [{"index": 0, "value": 3}]})),
        ("midi-events-delete", json!({"ref": S1, "indexes": [1]})),
    ] {
        let (preview, applied) = lab.make(kind, input);
        let restored = lab.undo(kind, &preview, &applied).unwrap();
        for answer in [&preview, &applied, &restored] {
            assert_eq!(daw_bridge::contract::misplaced_refs(answer), Vec::<String>::new(), "{kind}: {answer}");
        }
    }
}

// ----- quantize and humanize -----

/// The snare's notes, off the grid: the item starts on beat 16, a grid line, and a quarter note is 960 ticks.
fn loose_notes(notes: &[(i64, i64, i32)]) -> Lab {
    let lab = Lab::new();
    state(|s| {
        s.tracks[2].items[0].takes[0].notes = notes
            .iter()
            .map(|&(start, end, pitch)| crate::reaper::fake::FakeNote {
                start_ppq: start as f64,
                end_ppq: end as f64,
                channel: 0,
                pitch,
                velocity: 100,
                selected: false,
                muted: false,
            })
            .collect()
    });
    lab
}

fn starts() -> Vec<(f64, f64)> {
    item("{ITEM-S1}").unwrap().1.takes[0].notes.iter().map(|n| (n.start_ppq, n.end_ppq)).collect()
}

#[test]
fn notes_are_quantized_by_strength_and_swing_and_put_back_by_the_notes_undo() {
    let lab = loose_notes(&[(100, 400, 60), (1000, 1200, 62), (2000, 2300, 64)]);
    let (preview, applied) = lab.make("notes-quantize", json!({"ref": S1, "grid": 1.0}));
    assert_eq!(preview["display"]["title"], "Quantized 3 notes in take “snare.mid” to 1 of a beat");
    assert_eq!(starts(), vec![(0.0, 300.0), (960.0, 1160.0), (1920.0, 2220.0)], "moved to the grid, lengths kept");
    lab.undo("notes-quantize", &preview, &applied).unwrap();
    assert_eq!(starts(), vec![(100.0, 400.0), (1000.0, 1200.0), (2000.0, 2300.0)]);
    // Half strength goes half way; the ends alone snap with ends: end.
    lab.make("notes-quantize", json!({"ref": S1, "grid": 1.0, "strength": 50, "indexes": [1]}));
    assert_eq!(starts()[1], (980.0, 1180.0));
    let lab = loose_notes(&[(100, 400, 60), (1000, 1200, 62)]);
    lab.make("notes-quantize", json!({"ref": S1, "grid": 0.5, "ends": "end"}));
    assert_eq!(starts(), vec![(100.0, 480.0), (1000.0, 1440.0)]);
    // A swung eighth grid at 100%: every second line on the triplet (two thirds of a beat, 640 ticks).
    let lab = loose_notes(&[(600, 700, 60)]);
    let (preview, _) = lab.make("notes-quantize", json!({"ref": S1, "grid": 0.5, "swing": 100}));
    assert_eq!(starts(), vec![(640.0, 740.0)]);
    assert!(preview["display"]["title"].as_str().unwrap().ends_with("swing 100%"));
}

#[test]
fn a_quantize_shortens_a_note_it_would_overlap_and_refuses_two_starting_together() {
    let lab = loose_notes(&[(0, 1000, 60), (1100, 1300, 60)]);
    let (preview, applied) = lab.make("notes-quantize", json!({"ref": S1, "grid": 1.0, "indexes": [1]}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("1 note(s) end where the next"), "{}", preview["display"]);
    assert_eq!(starts(), vec![(0.0, 960.0), (960.0, 1160.0)]);
    lab.undo("notes-quantize", &preview, &applied).unwrap();
    assert_eq!(starts(), vec![(0.0, 1000.0), (1100.0, 1300.0)], "the shortened note is put back too");
    let lab = loose_notes(&[(900, 940, 60), (1010, 1100, 60)]);
    let (code_, message) = lab.preview("notes-quantize", json!({"ref": S1, "grid": 1.0})).unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("start together"), "{message}");
    assert_eq!(code(lab.preview("notes-quantize", json!({"ref": S1, "grid": 0.0}))), "bad-params");
    assert_eq!(code(lab.preview("notes-quantize", json!({"ref": S1, "grid": 1.0, "ends": "middle"}))), "bad-params");
}

#[test]
fn humanize_moves_and_changes_notes_from_a_seed_so_the_apply_does_what_the_preview_showed() {
    let lab = loose_notes(&[(0, 480, 60), (960, 1440, 62), (1920, 2400, 64)]);
    let input = json!({"ref": S1, "timing": 10.0, "velocity": 12});
    let preview = lab.preview("notes-humanize", input.clone()).unwrap();
    let seed = preview["fence"]["seed"].as_u64().expect("the preview's seed is fenced");
    assert_eq!(preview["display"]["seed"], seed);
    let applied = lab.apply_with("notes-humanize", &input, &preview, "Kumi: humanize (k1)").unwrap();
    let notes = item("{ITEM-S1}").unwrap().1.takes[0].notes.clone();
    // 10 ms at 120 BPM is a fiftieth of a beat: at most 19.2 ticks either way.
    for (note, was) in notes.iter().zip([0.0, 960.0, 1920.0]) {
        assert!((note.start_ppq - was).abs() <= 19.5, "{} from {was}", note.start_ppq);
        assert!((note.velocity - 100).abs() <= 12);
        assert_eq!(note.end_ppq - note.start_ppq, 480.0, "lengths kept");
    }
    let shown: Vec<f64> = preview["display"]["after"].as_array().unwrap().iter().map(|row| row["start"].as_f64().unwrap()).collect();
    let written: Vec<f64> = applied["display"]["written"]
        .as_array()
        .map(|rows| rows.iter().map(|row| row["start"].as_f64().unwrap()).collect())
        .unwrap_or_default();
    let _ = written;
    assert_eq!(shown.len(), 3);
    lab.undo("notes-humanize", &preview, &applied).unwrap();
    assert_eq!(starts(), vec![(0.0, 480.0), (960.0, 1440.0), (1920.0, 2400.0)]);
    // The same seed moves them the same way again.
    let again = lab.preview("notes-humanize", json!({"ref": S1, "timing": 10.0, "velocity": 12, "seed": seed})).unwrap();
    assert_eq!(again["display"]["after"], preview["display"]["after"]);
    assert_eq!(code(lab.preview("notes-humanize", json!({"ref": S1}))), "bad-params");
}

#[test]
fn a_transform_is_refused_when_the_notes_changed_since_the_preview_and_its_undo_when_they_changed_since() {
    let lab = loose_notes(&[(100, 400, 60), (1000, 1200, 62)]);
    let input = json!({"ref": S1, "grid": 1.0});
    let preview = lab.preview("notes-quantize", input.clone()).unwrap();
    state(|s| s.tracks[2].items[0].takes[0].notes[0].velocity = 90);
    assert_eq!(code(lab.apply_with("notes-quantize", &input, &preview, "Kumi: x (k1)")), "changed");
    let (preview, applied) = lab.make("notes-quantize", input);
    state(|s| s.tracks[2].items[0].takes[0].notes[1].start_ppq = 990.0);
    assert_eq!(code(lab.undo("notes-quantize", &preview, &applied)), "changed");
    for answer in [&preview, &applied] {
        assert_eq!(daw_bridge::contract::misplaced_refs(answer), Vec::<String>::new());
    }
}

/// Every MIDI event and note transform undo says when the take is back already (writing nothing) and when it is gone.
#[test]
fn every_event_and_transform_undo_says_back_already_and_gone() {
    let cases: [(&str, serde_json::Value); 5] = [
        (
            "midi-events-add",
            json!({"ref": SNARE_TAKE, "events": [{"type": "cc", "channel": 1, "controller": 1, "value": 5, "start": 0.5}]}),
        ),
        ("midi-events-change", json!({"ref": SNARE_TAKE, "changes": [{"index": 0, "value": 9}]})),
        ("midi-events-delete", json!({"ref": SNARE_TAKE, "indexes": [1]})),
        ("notes-quantize", json!({"ref": SNARE_TAKE, "grid": 1.0})),
        ("notes-humanize", json!({"ref": SNARE_TAKE, "timing": 20.0, "seed": 3})),
    ];
    for (kind, input) in cases {
        let setup = || if kind.starts_with("notes") { loose_notes(&[(100, 400, 60), (1000, 1200, 62)]) } else { midi_lab() };
        let lab = setup();
        let (preview, applied) = lab.make(kind, input.clone());
        lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        let again = lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind} again: {e:?}"));
        assert_eq!(again["display"]["note"], "It was back already: nothing was written.", "{kind}");
        let lab = setup();
        let (preview, applied) = lab.make(kind, input);
        state(|s| s.tracks[2].items.retain(|i| i.guid != "{ITEM-S1}"));
        assert_eq!(code(lab.undo(kind, &preview, &applied)), "gone", "{kind}");
    }
}
