//! The variations tool's takes (#401, SFX variations): an item copied with its settings, rewritten, and several dropped,
//! each one change that Kumi's undo takes back whole, refused once the producer changed it, gone, or back already.

use serde_json::json;

use super::tests_items::{code, guid_in, item, state, Lab, K1};

const S1: &str = "r:item:{ITEM-S1}";

#[test]
fn an_audio_take_is_its_items_copy_with_its_own_settings_and_its_undo_takes_it_away_while_as_made() {
    let lab = Lab::new();
    let input = json!({"ref": K1, "position": 30.0, "length": 1.5, "startOffset": 0.02, "pitch": -1.5, "playrate": 1.1,
        "preservePitch": false, "volumeDb": -3.0, "fadeIn": 0.005, "fadeOut": 0.4, "name": "V1 · kick"});
    let (preview, made) = lab.make("item-variation", input);
    assert_eq!(preview["display"]["title"], "Added “V1 · kick” at bar 16");
    let copy = guid_in(&made["created"]["ref"]);
    let (track, take) = item(&copy).expect("the take is made");
    assert_eq!((track, take.position, take.length), (1, 30.0, 1.5));
    assert_ne!(take.takes[0].guid, "{TAKE-K1}", "it shares no ids with its source");
    let value = |key: &str| take.takes[0].extra.get(key).copied().unwrap_or(f64::NAN);
    assert_eq!((value("D_PITCH"), value("D_PLAYRATE"), value("B_PPITCH"), value("D_STARTOFFS")), (-1.5, 1.1, 0.0, 0.02));
    assert!((value("D_VOL") - 10f64.powf(-3.0 / 20.0)).abs() < 1e-9);
    assert_eq!((take.extra["D_FADEINLEN"], take.extra["D_FADEOUTLEN"]), (0.005, 0.4));
    assert_eq!(take.takes[0].name, "V1 · kick");
    // The producer moved it since: the undo is refused; moved back, it takes it away; then it's gone.
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == copy).unwrap().position = 31.0);
    assert_eq!(code(lab.undo("item-variation", &preview, &made)), "changed");
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == copy).unwrap().position = 30.0);
    lab.undo("item-variation", &preview, &made).unwrap();
    assert!(item(&copy).is_none());
    assert_eq!(code(lab.undo("item-variation", &preview, &made)), "gone");
    // Refused before anything is made: no position, a rate out of range, notes on an audio take.
    for wrong in
        [json!({"ref": K1}), json!({"ref": K1, "position": 5.0, "playrate": 0.0}), json!({"ref": K1, "position": 5.0, "name": "a\nb"})]
    {
        assert_eq!(code(lab.preview("item-variation", wrong.clone())), "bad-params", "{wrong}");
    }
    let notes = json!({"ref": K1, "position": 5.0, "notes": [{"pitch": 36, "start": 0.0, "length": 0.5}]});
    let preview = lab.preview("item-variation", notes.clone()).unwrap();
    let before = state(|s| s.tracks.clone());
    assert_eq!(code(lab.apply_with("item-variation", &notes, &preview, "Kumi: x (kv1)")), "bad-params");
    assert!(state(|s| s.tracks.clone()) == before, "nothing half-made stays");
}

#[test]
fn a_midi_take_has_its_own_notes_and_a_rewrite_is_put_back_whole() {
    let lab = Lab::new();
    let input = json!({"ref": S1, "position": 20.0, "name": "V2 · snare", "notes": [
        {"pitch": 38, "start": 0.0, "length": 0.5, "velocity": 90},
        {"pitch": 38, "start": 1.5, "length": 0.25, "velocity": 40}
    ]});
    let (_, made) = lab.make("item-variation", input);
    let copy = guid_in(&made["created"]["ref"]);
    let (_, take) = item(&copy).unwrap();
    let notes: Vec<(i32, i32)> = take.takes[0].notes.iter().map(|n| (n.pitch, n.velocity)).collect();
    assert_eq!(notes, [(38, 90), (38, 40)], "the copy's notes are the take's, its source's left as they were");
    assert_eq!(item("{ITEM-S1}").unwrap().1.takes[0].notes.len(), 2);
    // Rewritten: other notes and a quieter take; its undo puts back the whole take as it was.
    let reference = made["created"]["ref"].clone();
    let rewrite = json!({"ref": reference, "volumeDb": -6.0, "notes": [{"pitch": 40, "start": 0.0, "length": 1.0, "velocity": 70}]});
    let (preview, written) = lab.make("item-variation-set", rewrite.clone());
    assert_eq!(preview["display"]["title"], "Rewrote “V2 · snare”");
    let (_, now) = item(&copy).unwrap();
    assert_eq!(now.takes[0].notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [40]);
    // The producer changed it since: refused; told to anyway, put back.
    state(|s| s.tracks[2].items.iter_mut().find(|i| i.guid == copy).unwrap().takes[0].notes[0].velocity = 127);
    assert_eq!(code(lab.undo("item-variation-set", &preview, &written)), "changed");
    lab.undo_with("item-variation-set", &preview, &written, true).unwrap();
    let (_, back) = item(&copy).unwrap();
    assert_eq!(back.takes[0].notes.iter().map(|n| (n.pitch, n.velocity)).collect::<Vec<_>>(), [(38, 90), (38, 40)]);
    // As it was already: nothing written.
    let again = lab.undo("item-variation-set", &preview, &written).unwrap();
    assert!(again["display"]["title"].as_str().unwrap().ends_with("is as it was already"), "{again}");
}

#[test]
fn takes_dropped_together_are_one_change_and_its_undo_makes_each_again_with_its_ids() {
    let lab = Lab::new();
    let (_, a) = lab.make("item-variation", json!({"ref": K1, "position": 30.0, "name": "V1"}));
    let (_, b) = lab.make("item-variation", json!({"ref": K1, "position": 33.0, "name": "V2"}));
    let (ga, gb) = (guid_in(&a["created"]["ref"]), guid_in(&b["created"]["ref"]));
    let drop = json!({"refs": [a["created"]["ref"], b["created"]["ref"]]});
    let (preview, dropped) = lab.make("items-delete", drop.clone());
    assert_eq!(preview["display"]["title"], "Deleted 2 items");
    assert!(item(&ga).is_none() && item(&gb).is_none());
    let undone = lab.undo("items-delete", &preview, &dropped).unwrap();
    assert_eq!(undone["display"]["title"], "2 items put back");
    assert!(item(&ga).is_some() && item(&gb).is_some(), "each again, with its id");
    let again = lab.undo("items-delete", &preview, &dropped).unwrap();
    assert_eq!(again["display"]["title"], "The items are back already");
    // One changed since the preview: nothing is deleted.
    let preview = lab.preview("items-delete", drop.clone()).unwrap();
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == gb).unwrap().position = 34.0);
    assert_eq!(code(lab.apply_with("items-delete", &drop, &preview, "Kumi: x (kv2)")), "changed");
    assert!(item(&ga).is_some(), "nothing deleted");
    assert_eq!(code(lab.preview("items-delete", json!({"refs": [a["created"]["ref"], a["created"]["ref"]]}))), "bad-params");
}
