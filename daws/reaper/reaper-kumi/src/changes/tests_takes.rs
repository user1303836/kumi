//! Takes, stretch markers and take markers (m4-r-items): each kind applied, undone by Kumi, refused when the producer
//! changed it in between, gone and back already.

use serde_json::json;

use super::tests_items::{code, guid_in, item, kumi_render, state, top, Lab, K1};
use crate::reaper::fake::{self, FakeTake};

const T1: &str = "r:take:{TAKE-K1}";

/// Kick's first item with three takes: kick.wav (playing), "room" and an unnamed one.
fn three_takes() -> Lab {
    let lab = Lab::new();
    state(|s| {
        let item = &mut s.tracks[1].items[0];
        item.takes.push(FakeTake::wave("{TAKE-K1B}", "room"));
        item.takes.push(FakeTake::wave("{TAKE-K1C}", ""));
    });
    lab
}

// ----- properties -----

#[test]
fn take_properties_are_set_together_and_each_put_back() {
    let lab = Lab::new();
    let input = json!({"ref": T1, "volumeDb": -6.0, "pan": -0.5, "pitch": 2.0, "playrate": 1.5, "preservePitch": true, "channelMode": "mono", "name": "kick tight", "color": "#00ff00"});
    let (preview, applied) = lab.make("take-props", input);
    assert_eq!(
        preview["display"]["title"],
        "kick.wav: volume −6 dB, pan 50% left, pitch +2 st, rate 1.5×, pitch kept with the rate, channels mono, named “kick tight”, colour changed"
    );
    assert_eq!(preview["display"]["before"]["volumeDb"], 0.0);
    assert_eq!(
        (&preview["display"]["after"]["volumeDb"], &preview["display"]["after"]["channelMode"], &preview["display"]["after"]["color"]),
        (&json!(-6.0), &json!("mono"), &json!("#00ff00"))
    );
    let take = item("{ITEM-K1}").unwrap().1.takes[0].clone();
    assert!((take.extra["D_VOL"] - 0.501187).abs() < 1e-5);
    assert_eq!(
        (take.extra["D_PAN"], take.extra["D_PITCH"], take.extra["D_PLAYRATE"], take.extra["B_PPITCH"], take.extra["I_CHANMODE"]),
        (-0.5, 2.0, 1.5, 1.0, 2.0)
    );
    assert_eq!(take.name, "kick tight");
    lab.undo("take-props", &preview, &applied).unwrap();
    let back = item("{ITEM-K1}").unwrap().1.takes[0].clone();
    assert_eq!(
        (back.extra["D_VOL"], back.extra["D_PAN"], back.extra["D_PITCH"], back.extra["D_PLAYRATE"], back.extra["B_PPITCH"]),
        (1.0, 0.0, 0.0, 1.0, 0.0)
    );
    assert_eq!((back.name.as_str(), back.extra["I_CHANMODE"], back.extra["I_CUSTOMCOLOR"]), ("kick.wav", 0.0, 0.0));
}

#[test]
fn a_takes_volume_and_its_polarity_are_one_number_in_reaper_and_each_changes_alone() {
    let lab = Lab::new();
    lab.make("take-props", json!({"ref": T1, "polarity": "inverted"}));
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].extra["D_VOL"], -1.0);
    let (preview, applied) = lab.make("take-props", json!({"ref": T1, "volumeDb": -6.0}));
    let vol = item("{ITEM-K1}").unwrap().1.takes[0].extra["D_VOL"];
    assert!(vol < 0.0 && (vol + 0.501187).abs() < 1e-5, "the polarity stays inverted: {vol}");
    lab.undo("take-props", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].extra["D_VOL"], -1.0);
}

#[test]
fn a_take_property_the_producer_changed_since_stays_theirs() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("take-props", json!({"ref": T1, "pitch": 3.0}));
    state(|s| s.tracks[1].items[0].takes[0].extra.insert("D_PITCH".into(), -1.0));
    assert_eq!(code(lab.undo("take-props", &preview, &applied)), "changed");
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].extra["D_PITCH"], -1.0);
    // Between the preview and the apply too.
    let input = json!({"ref": T1, "pitch": 3.0});
    let preview = lab.preview("take-props", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].takes[0].extra.insert("D_PITCH".into(), 5.0));
    assert_eq!(code(lab.apply_with("take-props", &input, &preview, "Kumi: x (k1)")), "changed");
    // The take deleted since: gone.
    let (preview, applied) = lab.make("take-props", json!({"ref": T1, "pan": 0.5}));
    state(|s| s.tracks[1].items.remove(0));
    assert_eq!(code(lab.undo("take-props", &preview, &applied)), "gone");
}

#[test]
fn take_properties_are_checked_before_anything_is_written() {
    let lab = Lab::new();
    for (input, needle) in [
        (json!({"ref": T1}), "pass one or more"),
        (json!({"ref": T1, "playrate": 0.0}), "playrate"),
        (json!({"ref": T1, "pitch": 200.0}), "pitch"),
        (json!({"ref": T1, "channelMode": "surround"}), "channelMode"),
        (json!({"ref": T1, "polarity": true}), "polarity"),
        (json!({"ref": "r:track:{BBBB}", "pan": 0.0}), "take ref"),
    ] {
        let (code, message) = lab.preview("take-props", input.clone()).unwrap_err();
        assert_eq!(code, "bad-params", "{input}");
        assert!(message.contains(needle), "{input}: {message}");
    }
    // An item's ref names its active take.
    assert_eq!(lab.preview("take-props", json!({"ref": K1, "pan": 0.2})).unwrap()["target"]["ref"], T1);
}

// ----- the active take -----

#[test]
fn another_take_plays_and_the_undo_makes_the_one_that_played_play_again() {
    let lab = three_takes();
    let (preview, applied) = lab.make("take-active", json!({"ref": "r:take:{TAKE-K1B}"}));
    assert_eq!(preview["display"]["title"], "room plays in “kick.wav”");
    assert_eq!(item("{ITEM-K1}").unwrap().1.active, 1);
    lab.undo("take-active", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-K1}").unwrap().1.active, 0);
    // The producer picks another take meanwhile: theirs.
    let (preview, applied) = lab.make("take-active", json!({"ref": "r:take:{TAKE-K1B}"}));
    state(|s| s.tracks[1].items[0].active = 2);
    assert_eq!(code(lab.undo("take-active", &preview, &applied)), "changed");
    // The take that played is deleted since (Kumi's still plays): gone.
    state(|s| {
        s.tracks[1].items[0].takes.remove(0);
        s.tracks[1].items[0].active = 0;
    });
    assert_eq!(code(lab.undo("take-active", &preview, &applied)), "gone");
}

// ----- adding a take -----

#[test]
fn an_empty_take_is_added_and_plays_and_the_undo_puts_the_item_back_whole() {
    let lab = Lab::new();
    let before = item("{ITEM-K1}").unwrap().1;
    let (preview, applied) = lab.make("take-add", json!({"ref": K1, "name": "idea"}));
    assert_eq!(preview["display"]["title"], "Added the empty take “idea” to “kick.wav”");
    assert_eq!(applied["display"]["title"], preview["display"]["title"], "said of the item as it was, not by its new take");
    let now = item("{ITEM-K1}").unwrap().1;
    assert_eq!((now.takes.len(), now.active, now.takes[1].name.as_str()), (2, 1, "idea"));
    assert_eq!(guid_in(&applied["created"]["ref"]), now.takes[1].guid);
    let undone = lab.undo("take-add", &preview, &applied).unwrap();
    assert_eq!(undone["display"]["title"], "kick.wav as it was before");
    assert!(item("{ITEM-K1}").unwrap().1 == before);
    let again = lab.undo("take-add", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn a_take_of_a_render_in_kumis_own_folder_plays_a_copy_in_the_projects_media_folder() {
    let lab = Lab::new();
    let (_kumi, media, render) = kumi_render(&lab, "idea.wav");
    let copy = media.path().join("idea.wav").to_string_lossy().into_owned();
    fake::items::add_media(&copy, "WAVE", 2.0, false);
    let (preview, applied) = lab.make("take-add", json!({"ref": K1, "path": render}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("a copy of it"), "{}", preview["display"]);
    let now = item("{ITEM-K1}").unwrap().1;
    assert_eq!(now.takes[1].more.file.as_deref(), Some(copy.as_str()));
    assert!(applied["display"]["note"].as_str().unwrap().contains("Kumi clears"));
}

#[test]
fn a_take_playing_a_file_is_added_without_playing_when_asked() {
    let lab = Lab::new();
    let path = std::env::temp_dir().join(format!("kumi-takes-{}-alt.wav", std::process::id()));
    std::fs::write(&path, b"RIFF").unwrap();
    let path = path.to_string_lossy().into_owned();
    fake::items::add_media(&path, "WAVE", 2.0, false);
    let (_, applied) = lab.make("take-add", json!({"ref": K1, "path": path, "active": false}));
    let now = item("{ITEM-K1}").unwrap().1;
    assert_eq!((now.takes.len(), now.active), (2, 0), "the take that played still plays");
    assert_eq!(now.takes[1].more.file.as_deref(), Some(path.as_str()));
    assert!(now.takes[1].name.ends_with("alt.wav"));
    assert!(applied["display"]["note"].as_str().unwrap().contains("stays where it is"));
    assert_eq!(code(lab.preview("take-add", json!({"ref": K1, "path": "/no/such.wav"}))), "bad-params");
}

#[test]
fn an_added_take_is_kept_once_the_producer_has_changed_the_item_even_with_discard() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("take-add", json!({"ref": K1}));
    state(|s| s.tracks[1].items[0].position = 0.25);
    assert_eq!(code(lab.undo("take-add", &preview, &applied)), "changed");
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes.len(), 2);
    // The item's whole state from before would move it back too (review R4).
    let (code_, why) = lab.undo_with("take-add", &preview, &applied, true).unwrap_err();
    assert!(code_ == "changed" && why.contains("even with discard"), "{why}");
    assert_eq!((item("{ITEM-K1}").unwrap().1.takes.len(), item("{ITEM-K1}").unwrap().1.position), (2, 0.25));
}

// ----- deleting a take -----

#[test]
fn a_take_is_deleted_with_reapers_own_action_and_the_producers_selection_and_active_take_stay() {
    let lab = three_takes();
    // The producer has the other kick item selected, not this one.
    state(|s| s.tracks[1].items[1].selected = true);
    let before = item("{ITEM-K1}").unwrap().1;
    let (preview, applied) = lab.make("take-delete", json!({"ref": "r:take:{TAKE-K1B}"}));
    assert_eq!(preview["display"]["title"], "Deleted “room” from “kick.wav”");
    let now = item("{ITEM-K1}").unwrap().1;
    assert_eq!(now.takes.iter().map(|t| t.guid.as_str()).collect::<Vec<_>>(), ["{TAKE-K1}", "{TAKE-K1C}"]);
    assert_eq!(now.active, 0, "the take that played plays");
    assert!(!now.selected && state(|s| s.tracks[1].items[1].selected), "the producer's selection is as it was");
    assert_eq!(state(|s| s.actions.clone()), vec![40129]);
    assert_eq!(top().as_deref(), Some("Kumi: take-delete (k0001)"));
    lab.undo("take-delete", &preview, &applied).unwrap();
    assert!(item("{ITEM-K1}").unwrap().1 == before, "the item is as it was, the take in its place");
}

#[test]
fn the_take_that_plays_can_be_deleted_and_an_only_take_is_not() {
    let lab = three_takes();
    let (preview, applied) = lab.make("take-delete", json!({"ref": T1}));
    // The item is named after the take that plays, so it is named by its place here.
    assert_eq!(preview["display"]["title"], "Deleted “kick.wav” from the item on “Kick” at bar 1");
    assert_eq!(applied["display"]["title"], preview["display"]["title"]);
    let now = item("{ITEM-K1}").unwrap().1;
    assert_eq!(now.takes.len(), 2);
    assert_eq!(now.takes[now.active].guid, "{TAKE-K1B}", "REAPER plays the next one");
    let (code_, message) = lab.preview("take-delete", json!({"ref": "r:take:{TAKE-K2}"})).unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("only take"), "{message}");
}

#[test]
fn a_take_delete_is_refused_when_the_item_changed_and_its_undo_when_it_changed_since() {
    let lab = three_takes();
    let input = json!({"ref": "r:take:{TAKE-K1C}"});
    let preview = lab.preview("take-delete", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].takes[1].name = "room 2".into());
    assert_eq!(code(lab.apply_with("take-delete", &input, &preview, "Kumi: x (k1)")), "changed");
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes.len(), 3, "nothing written");
    assert!(state(|s| s.actions.is_empty()));
    let (preview, applied) = lab.make("take-delete", input);
    state(|s| s.tracks[1].items[0].mute = true);
    assert_eq!(code(lab.undo("take-delete", &preview, &applied)), "changed");
}

// ----- stretch markers -----

#[test]
fn stretch_markers_are_added_moved_and_deleted_and_each_change_undone_by_value() {
    let lab = Lab::new();
    let (preview, applied) =
        lab.make("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.5}, {"position": 0.5, "sourcePosition": 0.4}]}));
    assert_eq!(preview["display"]["title"], "Added 2 stretch markers in “kick.wav”");
    let markers = || item("{ITEM-K1}").unwrap().1.takes[0].more.stretch.clone();
    assert_eq!(markers(), vec![(0.5, 0.4, 0.0), (1.5, 1.5, 0.0)]);
    // The producer adds one of their own: Kumi's undo takes only its own.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch.insert(1, (1.0, 1.0, 0.0)));
    lab.undo("stretch-markers-add", &preview, &applied).unwrap();
    assert_eq!(markers(), vec![(1.0, 1.0, 0.0)]);
    // A move in place and a slope; then its undo.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(0.5, 0.5, 0.0), (1.0, 1.0, 0.0), (1.5, 1.5, 0.0)]);
    let (preview, applied) =
        lab.make("stretch-markers-change", json!({"ref": T1, "changes": [{"index": 1, "position": 1.2, "slope": 0.25}]}));
    assert_eq!(markers()[1], (1.2, 1.0, 0.25));
    lab.undo("stretch-markers-change", &preview, &applied).unwrap();
    assert_eq!(markers(), vec![(0.5, 0.5, 0.0), (1.0, 1.0, 0.0), (1.5, 1.5, 0.0)]);
    let (preview, applied) = lab.make("stretch-markers-delete", json!({"ref": T1, "indexes": [0, 2]}));
    assert_eq!(markers(), vec![(1.0, 1.0, 0.0)]);
    lab.undo("stretch-markers-delete", &preview, &applied).unwrap();
    assert_eq!(markers(), vec![(0.5, 0.5, 0.0), (1.0, 1.0, 0.0), (1.5, 1.5, 0.0)]);
    let again = lab.undo("stretch-markers-delete", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn a_stretch_marker_added_without_a_source_pins_what_plays_there_now_after_the_producers_markers_moved() {
    let lab = Lab::new();
    // The producer's markers: the second dragged so that source 0.8 plays at 1.0 s.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(0.5, 0.5, 0.0), (1.0, 0.8, 0.0)]);
    let markers = || item("{ITEM-K1}").unwrap().1.takes[0].more.stretch.clone();
    let (preview, applied) = lab.make("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.5}]}));
    // What plays at 1.5 s now is 0.8 + 0.5 into the source, not 1.5: the audio around it isn't re-timed.
    assert_eq!(markers()[2], (1.5, 1.3, 0.0));
    assert_eq!(applied["applied"]["added"][0][1], 1.3, "what REAPER wrote is kept");
    lab.undo("stretch-markers-add", &preview, &applied).unwrap();
    assert_eq!(markers(), vec![(0.5, 0.5, 0.0), (1.0, 0.8, 0.0)]);
}

#[test]
fn stretch_markers_keep_reapers_rules_and_their_undo_refuses_what_the_producer_moved() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(0.5, 0.5, 0.0), (1.0, 1.0, 0.0)]);
    for (kind, input, needle) in [
        ("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.0}]}), "one position"),
        ("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 3.0}]}), "inside the item"),
        ("stretch-markers-change", json!({"ref": T1, "changes": [{"index": 0, "position": 1.5}]}), "between its neighbours"),
        ("stretch-markers-delete", json!({"ref": T1, "indexes": [5]}), "read it again"),
        ("stretch-markers-change", json!({"ref": T1, "changes": [{"index": 0, "slope": 3.0}]}), "slope"),
    ] {
        let (code, message) = lab.preview(kind, input.clone()).unwrap_err();
        assert_eq!(code, "bad-params", "{input}");
        assert!(message.contains(needle), "{input}: {message}");
    }
    let (preview, applied) = lab.make("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.5}]}));
    state(|s| s.tracks[1].items[0].takes[0].more.stretch[2].0 = 1.6);
    assert_eq!(code(lab.undo("stretch-markers-add", &preview, &applied)), "changed");
    // The list changed between the preview and the apply: its indexes may name others.
    let input = json!({"ref": T1, "indexes": [0]});
    let preview = lab.preview("stretch-markers-delete", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].takes[0].more.stretch.insert(0, (0.2, 0.2, 0.0)));
    assert_eq!(code(lab.apply_with("stretch-markers-delete", &input, &preview, "Kumi: x (k1)")), "changed");
}

// ----- take markers -----

#[test]
fn take_markers_are_added_changed_and_deleted_and_the_producers_own_stay() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].takes[0].more.markers = vec![(0.3, "theirs".into(), 0)]);
    let (preview, applied) =
        lab.make("take-markers-add", json!({"ref": T1, "markers": [{"sourcePosition": 1.0, "name": "drop", "color": "#ff0000"}]}));
    assert_eq!(preview["display"]["title"], "Added the take marker “drop” to “kick.wav”");
    let markers = || item("{ITEM-K1}").unwrap().1.takes[0].more.markers.clone();
    assert_eq!(markers(), vec![(0.3, "theirs".into(), 0), (1.0, "drop".into(), fake::native_color(255, 0, 0))]);
    lab.undo("take-markers-add", &preview, &applied).unwrap();
    assert_eq!(markers(), vec![(0.3, "theirs".into(), 0)]);
    // A change moves and renames one marker; its undo puts that one back.
    let (preview, applied) =
        lab.make("take-markers-change", json!({"ref": T1, "changes": [{"index": 0, "name": "verse", "position": 1.5}]}));
    assert_eq!(markers(), vec![(1.5, "verse".into(), 0)]);
    lab.undo("take-markers-change", &preview, &applied).unwrap();
    assert_eq!(markers(), vec![(0.3, "theirs".into(), 0)]);
    let (preview, applied) = lab.make("take-markers-delete", json!({"ref": T1, "indexes": [0]}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("length"));
    assert!(markers().is_empty());
    let restored = lab.undo("take-markers-delete", &preview, &applied).unwrap();
    assert!(restored["display"]["note"].as_str().unwrap().contains("length"));
    assert_eq!(markers(), vec![(0.3, "theirs".into(), 0)]);
}

#[test]
fn a_take_marker_by_position_follows_the_takes_offset_and_rate_and_its_undo_is_fenced() {
    let lab = Lab::new();
    state(|s| {
        let take = &mut s.tracks[1].items[0].takes[0];
        take.extra.insert("D_STARTOFFS".into(), 0.5);
        take.extra.insert("D_PLAYRATE".into(), 2.0);
    });
    let (preview, applied) = lab.make("take-markers-add", json!({"ref": T1, "markers": [{"position": 0.25}]}));
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].more.markers, vec![(1.0, String::new(), 0)]);
    state(|s| s.tracks[1].items[0].takes[0].more.markers[0].1 = "renamed".into());
    assert_eq!(code(lab.undo("take-markers-add", &preview, &applied)), "changed");
    // Under stretch markers a place in the item doesn't say where the source plays.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(0.5, 0.5, 0.0)]);
    let (_, message) = lab.preview("take-markers-add", json!({"ref": T1, "markers": [{"position": 0.25}]})).unwrap_err();
    assert!(message.contains("sourcePosition"), "{message}");
}

#[test]
fn every_take_answer_names_refs_the_way_the_contract_says() {
    let lab = three_takes();
    for (kind, input) in [
        ("take-props", json!({"ref": T1, "pan": 0.3})),
        ("take-active", json!({"ref": "r:take:{TAKE-K1B}"})),
        ("take-add", json!({"ref": K1})),
        ("take-delete", json!({"ref": "r:take:{TAKE-K1C}"})),
        ("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.0}]})),
        ("take-markers-add", json!({"ref": T1, "markers": [{"sourcePosition": 1.0, "name": "x"}]})),
    ] {
        let (preview, applied) = lab.make(kind, input);
        let restored = lab.undo(kind, &preview, &applied).unwrap();
        for answer in [&preview, &applied, &restored] {
            assert_eq!(daw_bridge::contract::misplaced_refs(answer), Vec::<String>::new(), "{kind}: {answer}");
        }
    }
}

// ----- what the reads say -----

#[test]
fn a_takes_own_read_lists_its_markers_with_the_indexes_the_changes_take() {
    let lab = Lab::new();
    state(|s| {
        let take = &mut s.tracks[1].items[0].takes[0];
        take.more.stretch = vec![(0.5, 0.4, 0.1)];
        take.more.markers = vec![(1.0, "drop".into(), fake::native_color(255, 0, 0)), (1.5, "end".into(), 0)];
        take.extra.insert("D_PITCH".into(), 2.0);
        take.extra.insert("D_VOL".into(), -0.5);
    });
    let row = lab.call("read", json!({"ref": T1})).unwrap()["items"][0].clone();
    assert_eq!(
        (row["pitch"].as_f64(), row["polarity"].as_str(), row["channelMode"].as_str()),
        (Some(2.0), Some("inverted"), Some("normal"))
    );
    assert!((row["volumeDb"].as_f64().unwrap() + 6.0206).abs() < 1e-3);
    assert_eq!(row["stretchMarkers"][0], json!({"index": 0, "position": 0.5, "sourcePosition": 0.4, "slope": 0.1}));
    assert_eq!(row["takeMarkers"][0], json!({"index": 0, "sourcePosition": 1.0, "name": "drop", "color": "#ff0000"}));
    assert_eq!(row["takeMarkers"][1]["color"], serde_json::Value::Null);
    // A list of an item's takes counts the markers only.
    let rows = lab.call("read", json!({"kind": "take", "parent": K1})).unwrap()["items"].clone();
    assert_eq!((rows[0]["stretchMarkerCount"].as_u64(), rows[0]["takeMarkerCount"].as_u64()), (Some(1), Some(2)));
    assert!(rows[0].get("stretchMarkers").is_none());
}

#[test]
fn an_items_row_says_its_gain_lock_loop_group_and_fades_and_a_track_its_razor_areas() {
    let lab = Lab::new();
    let plain = lab.call("read", json!({"ref": K1})).unwrap()["items"][0].clone();
    assert_eq!((plain["locked"].as_bool(), plain["loop"].as_bool()), (Some(false), Some(false)));
    for key in ["volumeDb", "group", "fadeIn", "fadeOut", "timebase", "snapOffset"] {
        assert!(plain.get(key).is_none(), "{key} only when it isn't REAPER's default: {plain}");
    }
    state(|s| {
        let item = &mut s.tracks[1].items[0];
        for (key, value) in [
            ("C_LOCK", 1.0),
            ("B_LOOPSRC", 1.0),
            ("D_VOL", 0.5),
            ("I_GROUPID", 3.0),
            ("D_FADEINLEN", 0.25),
            ("C_FADEINSHAPE", 2.0),
            ("C_BEATATTACHMODE", 1.0),
        ] {
            item.extra.insert(key.into(), value);
        }
    });
    let row = lab.call("read", json!({"ref": K1})).unwrap()["items"][0].clone();
    assert_eq!(
        (row["locked"].as_bool(), row["loop"].as_bool(), row["group"].as_i64(), row["timebase"].as_str()),
        (Some(true), Some(true), Some(3), Some("beats"))
    );
    assert_eq!((row["fadeIn"]["length"].as_f64(), row["fadeIn"]["shape"].as_i64()), (Some(0.25), Some(2)));
    assert!((row["volumeDb"].as_f64().unwrap() + 6.0206).abs() < 1e-3);
    // Razor areas on a track, the media lane's and an envelope's.
    lab.make("razor-set", json!({"ref": "r:track:{AAAA}", "areas": [{"start": 1.0, "end": 2.0}, {"start": 3.0, "end": 4.0, "envelopeRef": "r:env:{AAAA}:{ENV-VOLENV2}"}]}));
    let track = lab.call("read", json!({"ref": "r:track:{AAAA}"})).unwrap()["items"][0].clone();
    assert_eq!(track["razorAreas"][0], json!({"start": 1.0, "end": 2.0, "envelopeRef": null, "top": 0.0, "bottom": 1.0}));
    assert_eq!(track["razorAreas"][1]["envelopeRef"], "r:env:{AAAA}:{ENV-VOLENV2}");
    assert_eq!(daw_bridge::contract::misplaced_refs(&track), Vec::<String>::new());
}

// ----- what the producer changed between the preview and the apply -----

#[test]
fn a_take_apply_is_refused_when_the_producer_changed_the_take_or_item_since_the_preview() {
    type Edit = fn(&mut fake::State);
    let cases: [(&str, serde_json::Value, Edit); 3] = [
        ("take-active", json!({"ref": "r:take:{TAKE-K1B}"}), |s| s.tracks[1].items[0].active = 2),
        ("take-add", json!({"ref": K1, "name": "idea"}), |s| s.tracks[1].items[0].position = 0.25),
        ("take-markers-add", json!({"ref": T1, "markers": [{"sourcePosition": 0.5, "name": "a"}]}), |s| {
            s.tracks[1].items[0].takes[0].more.markers.push((0.2, "theirs".into(), 0));
        }),
    ];
    for (kind, input, edit) in cases {
        let lab = three_takes();
        let preview = lab.preview(kind, input.clone()).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        state(edit);
        let before = state(|s| s.tracks.clone());
        let answer = lab.apply_with(kind, &input, &preview, "Kumi: x (k1)");
        assert_eq!(answer.as_ref().err().map(|e| e.0.as_str()), Some("changed"), "{kind}: {answer:?}");
        assert!(state(|s| s.tracks.clone()) == before, "{kind}: nothing written");
    }
}

#[test]
fn a_stretch_marker_the_producer_put_where_one_goes_back_stops_the_undo() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(0.5, 0.5, 0.0), (1.0, 1.0, 0.0), (1.5, 1.5, 0.0)]);
    let (preview, applied) = lab.make("stretch-markers-delete", json!({"ref": T1, "indexes": [1]}));
    // The producer puts a marker of their own at 1.0, from another place in the source.
    state(|s| s.tracks[1].items[0].takes[0].more.stretch.insert(1, (1.0, 1.2, 0.0)));
    let (code_, message) = lab.undo("stretch-markers-delete", &preview, &applied).unwrap_err();
    assert_eq!(code_, "changed");
    assert!(message.contains("can't all be put back"), "{message}");
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].more.stretch, vec![(0.5, 0.5, 0.0), (1.0, 1.2, 0.0), (1.5, 1.5, 0.0)]);
}

#[test]
fn a_take_and_its_stretch_markers_back_from_reapers_undo_and_redo_can_still_be_undone() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("take-props", json!({"ref": T1, "pan": -0.123456789}));
    // REAPER's saved state keeps a pan to 6 decimals (lab, 7.82).
    state(|s| s.tracks[1].items[0].takes[0].extra.insert("D_PAN".into(), -0.123457));
    lab.undo("take-props", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-K1}").unwrap().1.takes[0].extra["D_PAN"], 0.0);
    // And a stretch marker to 9 significant digits.
    let (preview, applied) =
        lab.make("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.2345678912, "sourcePosition": 1.1111111111}]}));
    state(|s| s.tracks[1].items[0].takes[0].more.stretch = vec![(1.23456789, 1.11111111, 0.0)]);
    lab.undo("stretch-markers-add", &preview, &applied).unwrap();
    assert!(item("{ITEM-K1}").unwrap().1.takes[0].more.stretch.is_empty());
}

/// Every take and marker kind's undo says when it is back already (writing nothing) and when the take's item is gone.
#[test]
fn every_take_undo_says_back_already_and_gone() {
    let marked = || {
        let lab = three_takes();
        state(|s| {
            let take = &mut s.tracks[1].items[0].takes[0];
            take.more.stretch = vec![(0.5, 0.5, 0.0), (1.0, 1.0, 0.0)];
            take.more.markers = vec![(0.3, "m".into(), 0)];
        });
        lab
    };
    let cases: [(&str, serde_json::Value); 10] = [
        ("take-props", json!({"ref": T1, "pitch": 2.0})),
        ("take-active", json!({"ref": "r:take:{TAKE-K1B}"})),
        ("take-add", json!({"ref": K1})),
        ("take-delete", json!({"ref": "r:take:{TAKE-K1B}"})),
        ("stretch-markers-add", json!({"ref": T1, "markers": [{"position": 1.5}]})),
        ("stretch-markers-change", json!({"ref": T1, "changes": [{"index": 1, "position": 1.2}]})),
        ("stretch-markers-delete", json!({"ref": T1, "indexes": [0]})),
        ("take-markers-add", json!({"ref": T1, "markers": [{"sourcePosition": 0.6, "name": "n"}]})),
        ("take-markers-change", json!({"ref": T1, "changes": [{"index": 0, "name": "x"}]})),
        ("take-markers-delete", json!({"ref": T1, "indexes": [0]})),
    ];
    for (kind, input) in cases {
        let lab = marked();
        let (preview, applied) = lab.make(kind, input.clone());
        lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        let again = lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind} again: {e:?}"));
        assert_eq!(again["display"]["note"], "It was back already: nothing was written.", "{kind}");
        let lab = marked();
        let (preview, applied) = lab.make(kind, input);
        state(|s| s.tracks[1].items.retain(|i| i.guid != "{ITEM-K1}"));
        assert_eq!(code(lab.undo(kind, &preview, &applied)), "gone", "{kind}");
    }
}
