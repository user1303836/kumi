//! Fixed lanes, lane plays and names, razor areas and comping (m4-r-items): applied, undone by Kumi, refused when the
//! producer changed them in between, gone and back already.

use serde_json::json;

use super::tests_items::{code, item, state, Lab, KICK, SNARE};

/// Kick with fixed lanes: two lanes, kick.wav on lane 0 and the second kick item moved under it on lane 1.
fn lanes_lab() -> Lab {
    let lab = Lab::new();
    state(|s| {
        let kick = &mut s.tracks[1];
        kick.extra.insert("I_FREEMODE".into(), 2.0);
        kick.extra.insert("I_NUMFIXEDLANES".into(), 2.0);
        kick.items[1].position = 0.0;
        kick.items[1].lane = 1;
        kick.lane_names = vec!["1".into(), "2".into()];
    });
    lab
}

fn plays() -> Vec<f64> {
    state(|s| {
        (0..s.tracks[1].extra.get("I_NUMFIXEDLANES").copied().unwrap_or(1.0) as usize)
            .map(|n| s.tracks[1].extra.get(&format!("C_LANEPLAYS:{n}")).copied().unwrap_or(2.0))
            .collect()
    })
}

// ----- lanes on and off -----

fn picked() -> bool {
    state(|s| s.tracks[1].extra.keys().any(|key| key.starts_with("C_LANEPLAYS:")))
}

fn lane_count() -> f64 {
    state(|s| s.tracks[1].extra.get("I_NUMFIXEDLANES").copied().unwrap_or(1.0))
}

#[test]
fn fixed_lanes_go_on_and_their_undo_puts_the_track_back() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-lanes", json!({"ref": KICK, "mode": "lanes"}));
    assert_eq!(preview["display"]["title"], "Kick: fixed lanes");
    assert_eq!(state(|s| s.tracks[1].extra["I_FREEMODE"]), 2.0);
    // A lane count isn't asked: REAPER keeps as many lanes as items use.
    let (code_, message) = lab.preview("track-lanes", json!({"ref": KICK, "mode": "lanes", "lanes": 3})).unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("lane = the lane count"), "{message}");
    lab.undo("track-lanes", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].extra["I_FREEMODE"]), 0.0);
    let again = lab.undo("track-lanes", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn a_lane_names_undo_is_refused_once_a_lane_went_in_above_it() {
    let lab = lanes_lab();
    let (preview, applied) = lab.make("lane-name", json!({"ref": KICK, "lane": 1, "name": "Take 2"}));
    // A comp lane goes in on top: every lane moves down one, and lane 2 is the unnamed one now.
    state(|s| s.tracks[1].lane_names.insert(0, "C1".into()));
    state(|s| s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 3.0));
    for discard in [false, true] {
        let (code_, message) = lab.undo_with("lane-name", &preview, &applied, discard).unwrap_err();
        assert_eq!(code_, "changed", "discard {discard}");
        assert!(message.contains("added or taken away"), "{message}");
    }
    assert_eq!(state(|s| s.tracks[1].lane_names.clone()), ["C1", "1", "Take 2"], "no lane renamed");
}

#[test]
fn undoing_a_move_off_the_bottom_lane_puts_it_back_after_reaper_dropped_that_lane() {
    let lab = lanes_lab();
    let (preview, applied) = lab.make("item-move", json!({"ref": "r:item:{ITEM-K2}", "toRef": SNARE}));
    // The second lane held only that item: REAPER drops it.
    state(|s| {
        s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 1.0);
        s.tracks[1].lane_names.truncate(1);
    });
    lab.undo("item-move", &preview, &applied).unwrap();
    let (track, back) = item("{ITEM-K2}").unwrap();
    assert_eq!((track, back.lane), (1, 1), "back on Kick, on a second lane again");
}

#[test]
fn undoing_a_move_finds_its_lane_by_the_items_on_it_after_a_lane_went_in_above() {
    let lab = lanes_lab();
    // Another item shares the second lane with the one Kumi moves.
    state(|s| {
        let take = crate::reaper::fake::FakeTake::wave("{TAKE-K3}", "kick.wav");
        let mut third = crate::reaper::fake::FakeItem::new("{ITEM-K3}", 4.0, 1.0, vec![take]);
        third.lane = 1;
        s.tracks[1].items.push(third);
    });
    let (preview, applied) = lab.make("item-move", json!({"ref": "r:item:{ITEM-K2}", "toRef": SNARE}));
    // A comp lane goes in on top: every lane moves down one.
    state(|s| {
        s.tracks[1].lane_names.insert(0, "C1".into());
        s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 3.0);
        for item in s.tracks[1].items.iter_mut() {
            item.lane += 1;
        }
    });
    lab.undo("item-move", &preview, &applied).unwrap();
    assert_eq!(item("{ITEM-K2}").unwrap().1.lane, 2, "beside the item it shared its lane with");
    // Alone on its lane, with lanes moved above it, it is refused rather than put on another lane.
    let lab = lanes_lab();
    let (preview, applied) = lab.make("item-move", json!({"ref": "r:item:{ITEM-K2}", "toRef": SNARE}));
    state(|s| {
        s.tracks[1].lane_names.insert(0, "C1".into());
        s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 3.0);
    });
    assert_eq!(code(lab.undo("item-move", &preview, &applied)), "changed");
}

#[test]
fn a_lane_names_undo_goes_through_another_lanes_rename_and_an_empty_lane_dropped_below() {
    let lab = lanes_lab();
    let names = || state(|s| s.tracks[1].lane_names.clone());
    let (preview, applied) = lab.make("lane-name", json!({"ref": KICK, "lane": 0, "name": "Lead"}));
    // The producer renames the other lane.
    state(|s| s.tracks[1].lane_names[1] = "Old".into());
    lab.undo("lane-name", &preview, &applied).unwrap();
    assert_eq!(names(), ["1", "Old"]);
    // REAPER drops the empty lane below the one Kumi named.
    let (preview, applied) = lab.make("lane-name", json!({"ref": KICK, "lane": 0, "name": "Lead"}));
    state(|s| {
        s.tracks[1].lane_names.truncate(1);
        s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 1.0);
    });
    lab.undo("lane-name", &preview, &applied).unwrap();
    assert_eq!(names(), ["1"]);
}

#[test]
fn undoing_lanes_off_leaves_the_track_once_the_producer_added_an_item_while_they_were_off_unless_told_to_discard() {
    let lab = lanes_lab();
    let (preview, applied) = lab.make("track-lanes", json!({"ref": KICK, "mode": "normal"}));
    assert_eq!(state(|s| s.tracks[1].extra["I_FREEMODE"]), 0.0);
    // With lanes off, the producer records a new take on Kick.
    state(|s| {
        let take = crate::reaper::fake::FakeTake::wave("{TAKE-NEW}", "new.wav");
        s.tracks[1].items.push(crate::reaper::fake::FakeItem::new("{ITEM-NEW}", 8.0, 2.0, vec![take]));
    });
    let before = state(|s| s.tracks[1].clone());
    let (code_, message) = lab.undo("track-lanes", &preview, &applied).unwrap_err();
    assert_eq!(code_, "changed", "{message}");
    assert!(state(|s| s.tracks[1].clone()) == before, "nothing written");
    // Told to undo anyway: lanes are back, Kumi's items on their lanes and the producer's item kept.
    lab.undo_with("track-lanes", &preview, &applied, true).unwrap();
    assert_eq!(state(|s| s.tracks[1].extra["I_FREEMODE"]), 2.0);
    assert_eq!(item("{ITEM-K2}").unwrap().1.lane, 1);
    assert!(item("{ITEM-NEW}").is_some());
}

#[test]
fn undoing_a_move_off_a_track_whose_lanes_went_off_since_leaves_the_item_where_kumi_put_it() {
    let lab = lanes_lab();
    // Kick's second item, on lane 2, moved to Snare.
    let (preview, applied) = lab.make("item-move", json!({"ref": "r:item:{ITEM-K2}", "toRef": SNARE}));
    assert_eq!(item("{ITEM-K2}").unwrap().0, 2);
    // The producer turns Kick's fixed lanes off: lane 2 is no place to go back to.
    state(|s| {
        s.tracks[1].extra.insert("I_FREEMODE".into(), 0.0);
    });
    let (code_, message) = lab.undo("item-move", &preview, &applied).unwrap_err();
    assert_eq!(code_, "changed", "{message}");
    assert_eq!(item("{ITEM-K2}").unwrap().0, 2, "still on Snare: nothing written");
    // Told to undo anyway, it goes back to its place on Kick.
    lab.undo_with("item-move", &preview, &applied, true).unwrap();
    assert_eq!(item("{ITEM-K2}").unwrap().0, 1);
}

#[test]
fn lanes_going_off_say_what_then_plays_together_and_the_undo_brings_the_lanes_back() {
    let lab = lanes_lab();
    state(|s| s.tracks[1].extra.insert("C_LANEPLAYS:1".into(), 1.0));
    state(|s| s.tracks[1].extra.insert("C_LANEPLAYS:0".into(), 0.0));
    let (preview, applied) = lab.make("track-lanes", json!({"ref": KICK, "mode": "normal"}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("1 item(s) on other lanes"), "{}", preview["display"]);
    // (REAPER puts every item on one lane while lanes are off)
    state(|s| {
        for item in s.tracks[1].items.iter_mut() {
            item.lane = 0;
        }
    });
    lab.undo("track-lanes", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].extra["I_FREEMODE"]), 2.0);
    assert_eq!(item("{ITEM-K2}").unwrap().1.lane, 1, "the item is on its lane again");
    assert_eq!(plays(), vec![0.0, 1.0], "the lanes play as they did");
}

#[test]
fn lanes_the_producer_changed_since_stay_theirs() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("track-lanes", json!({"ref": KICK, "mode": "lanes"}));
    // The producer puts the second kick item on a lane of its own.
    state(|s| {
        s.tracks[1].items[1].lane = 1;
        s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 2.0);
    });
    assert_eq!(code(lab.undo("track-lanes", &preview, &applied)), "changed");
    assert_eq!(code(lab.preview("track-lanes", json!({"ref": KICK, "mode": "free", "lanes": 2}))), "bad-params");
    assert_eq!(code(lab.preview("track-lanes", json!({"ref": "r:track:master", "mode": "lanes"}))), "bad-params");
}

// ----- lane plays -----

#[test]
fn a_lane_plays_alone_and_the_undo_makes_the_lanes_play_as_they_did() {
    let lab = lanes_lab();
    let (preview, applied) = lab.make("lane-plays", json!({"ref": KICK, "lane": 1, "plays": "only"}));
    assert_eq!(preview["display"]["title"], "Kick: only lane 2 plays");
    assert_eq!(plays(), vec![0.0, 1.0]);
    lab.undo("lane-plays", &preview, &applied).unwrap();
    assert_eq!(plays(), vec![2.0, 2.0]);
    // No lane was picked to play before (all played): none is after, so a lane added later plays too.
    assert!(!picked(), "the lanes are unpicked, not each picked");
    // Silent, then the producer picks another lane: theirs.
    let (preview, applied) = lab.make("lane-plays", json!({"ref": KICK, "lane": 0, "plays": "no"}));
    assert_eq!(plays(), vec![0.0, 1.0], "the lane left playing plays alone");
    state(|s| {
        s.tracks[1].extra.insert("C_LANEPLAYS:0".into(), 1.0);
        s.tracks[1].extra.insert("C_LANEPLAYS:1".into(), 0.0);
    });
    assert_eq!(code(lab.undo("lane-plays", &preview, &applied)), "changed");
    assert_eq!(code(lab.preview("lane-plays", json!({"ref": KICK, "lane": 2, "plays": "only"}))), "bad-params");
    assert_eq!(code(lab.preview("lane-plays", json!({"ref": "r:track:{CCCC}", "lane": 0, "plays": "only"}))), "bad-params");
}

// ----- lane names -----

#[test]
fn a_lane_is_named_and_its_name_put_back() {
    let lab = lanes_lab();
    let (preview, applied) = lab.make("lane-name", json!({"ref": KICK, "lane": 1, "name": "Take B"}));
    assert_eq!(preview["display"]["title"], "Kick: lane 2 named “Take B”");
    assert_eq!(state(|s| s.tracks[1].lane_names[1].clone()), "Take B");
    lab.undo("lane-name", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].lane_names[1].clone()), "2");
    let (preview, applied) = lab.make("lane-name", json!({"ref": KICK, "lane": 1, "name": "Take B"}));
    state(|s| s.tracks[1].lane_names[1] = "mine".into());
    assert_eq!(code(lab.undo("lane-name", &preview, &applied)), "changed");
    // The lane went: gone.
    state(|s| s.tracks[1].extra.insert("I_NUMFIXEDLANES".into(), 1.0));
    assert_eq!(code(lab.undo("lane-name", &preview, &applied)), "gone");
}

// ----- razor areas -----

#[test]
fn razor_areas_are_set_on_a_lane_or_an_envelope_and_put_back_exactly() {
    let lab = lanes_lab();
    state(|s| s.tracks[1].envelopes = vec![crate::reaper::fake::FakeEnvelope::own("Volume", "<VOLENV2", 2)]);
    let input = json!({"ref": KICK, "areas": [{"start": 0.5, "end": 1.5, "lane": 1}, {"startBeats": 4.0, "endBeats": 6.0, "envelopeRef": "r:env:{BBBB}:{ENV-VOLENV2}"}]});
    let (preview, applied) = lab.make("razor-set", input);
    assert_eq!(preview["display"]["title"], "Kick: 2 razor areas");
    let razor = state(|s| s.items.razor.get("{BBBB}").cloned().unwrap());
    assert!(
        razor.starts_with("0.50000000000000 1.50000000000000 \"\" 0.500000 1.000000,2.00000000000000 3.00000000000000 \"{ENV-VOLENV2}\""),
        "{razor}"
    );
    // The producer's areas elsewhere are theirs; the undo clears Kumi's.
    lab.undo("razor-set", &preview, &applied).unwrap();
    assert!(state(|s| !s.items.razor.contains_key("{BBBB}")));
    // An area the producer redrew since stays theirs.
    let (preview, applied) = lab.make("razor-set", json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0}]}));
    state(|s| s.items.razor.insert("{BBBB}".into(), "0.00000000000000 2.00000000000000 \"\" 0.000000 1.000000".into()));
    assert_eq!(code(lab.undo("razor-set", &preview, &applied)), "changed");
    // Clearing them, then putting them back.
    let (preview, applied) = lab.make("razor-set", json!({"ref": KICK, "areas": []}));
    assert!(state(|s| !s.items.razor.contains_key("{BBBB}")));
    lab.undo("razor-set", &preview, &applied).unwrap();
    assert!(state(|s| s.items.razor.get("{BBBB}").unwrap().starts_with("0.00000000000000 2.00000000000000")));
}

#[test]
fn razor_areas_are_checked_before_anything_is_written() {
    let lab = lanes_lab();
    for (input, needle) in [
        (json!({"ref": KICK}), "areas must be a list"),
        (json!({"ref": KICK, "areas": [{"start": 2.0, "end": 1.0}]}), "end after"),
        (json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0, "lane": 2}]}), "lane must be"),
        (json!({"ref": "r:track:{CCCC}", "areas": [{"start": 0.0, "end": 1.0, "lane": 0}]}), "no fixed lanes"),
        (
            json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0, "envelopeRef": "r:env:{AAAA}:{ENV-VOLENV2}"}]}),
            "this track's envelopes",
        ),
        (json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0, "envelopeRef": "r:env:take:{TAKE-K1}:{ENV-T}"}]}), "a take's envelope"),
    ] {
        let (code, message) = lab.preview("razor-set", input.clone()).unwrap_err();
        assert_eq!(code, "bad-params", "{input}");
        assert!(message.contains(needle), "{input}: {message}");
    }
}

// ----- comping -----

#[test]
fn a_comp_makes_the_comp_lane_and_area_and_its_undo_puts_the_whole_track_back() {
    let lab = lanes_lab();
    // The producer's razor area on another track and their track selection stay as they are.
    state(|s| {
        s.items.razor.insert("{DDDD}".into(), "9.00000000000000 10.00000000000000 \"\" 0.000000 1.000000".into());
        s.tracks[3].selected = true;
    });
    let before = state(|s| s.tracks[1].clone());
    let (preview, applied) = lab.make("lane-comp", json!({"ref": KICK, "lane": 1, "start": 0.5, "end": 1.5}));
    assert_eq!(preview["display"]["title"], "Kick: comped lane 2 from bar 1 beat 2 to bar 1 beat 4");
    assert!(preview["display"]["note"].as_str().unwrap().contains("comp lane on top"));
    let kick = state(|s| s.tracks[1].clone());
    assert_eq!(kick.lane_names[0], "C1");
    assert_eq!(kick.items.len(), 3, "the comp area's media is a new item on the comp lane");
    let made = kick.items.iter().find(|i| i.lane == 0 && i.guid != "{ITEM-K1}" && i.guid != "{ITEM-K2}").unwrap();
    assert_eq!((made.position, made.length), (0.5, 1.0));
    assert_eq!(applied["created"]["ref"], format!("r:item:{}", made.guid));
    assert_eq!(
        state(|s| s.items.razor.get("{DDDD}").cloned()).as_deref(),
        Some("9.00000000000000 10.00000000000000 \"\" 0.000000 1.000000")
    );
    assert!(state(|s| !s.items.razor.contains_key("{BBBB}")), "Kumi's razor area is gone");
    assert!(state(|s| s.tracks[3].selected && !s.tracks[1].selected), "the producer's selection is as it was");
    lab.undo("lane-comp", &preview, &applied).unwrap();
    assert!(state(|s| s.tracks[1].clone()) == before, "the track is exactly as before the comp");
    // A second comp on a comping track uses its comp lane.
    lab.make("lane-comp", json!({"ref": KICK, "lane": 1, "start": 0.0, "end": 0.5}));
    let (preview2, _) = lab.make("lane-comp", json!({"ref": KICK, "lane": 2, "start": 1.0, "end": 1.5}));
    assert!(preview2["display"].get("note").is_none());
    assert_eq!(state(|s| s.tracks[1].lane_names.len()), 3, "no second comp lane");
}

#[test]
fn a_comps_undo_puts_the_track_back_after_the_producer_only_selected_or_resized_it() {
    let lab = lanes_lab();
    let before = state(|s| s.tracks[1].clone());
    let (preview, applied) = lab.make("lane-comp", json!({"ref": KICK, "lane": 1, "start": 0.5, "end": 1.5}));
    // The producer clicks Kick and makes it taller: nothing they would want kept.
    state(|s| {
        s.tracks[1].selected = true;
        s.tracks[1].extra.insert("I_HEIGHTOVERRIDE".into(), 120.0);
    });
    lab.undo("lane-comp", &preview, &applied).unwrap();
    let kick = state(|s| s.tracks[1].clone());
    assert_eq!((kick.items.len(), kick.lane_names.len()), (before.items.len(), before.lane_names.len()), "the comp is gone");
    assert!(kick.selected, "the producer's selection stands");
}

#[test]
fn a_comp_is_refused_on_a_changed_track_and_with_nothing_to_comp_and_its_undo_fenced() {
    let lab = lanes_lab();
    let before = state(|s| s.tracks[1].clone());
    // Nothing on lane 2 from 3 to 3.5 s: the preview can't know; the apply finds nothing comped and puts the track back.
    let input = json!({"ref": KICK, "lane": 1, "start": 3.0, "end": 3.5});
    let preview = lab.preview("lane-comp", input.clone()).unwrap();
    let (code_, message) = lab.apply_with("lane-comp", &input, &preview, "Kumi: x (k1)").unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("no media"), "{message}");
    assert!(state(|s| s.tracks[1].clone()) == before, "nothing comped stays");
    // The track changed between the preview and the apply.
    let input = json!({"ref": KICK, "lane": 1, "start": 0.5, "end": 1.5});
    let preview = lab.preview("lane-comp", input.clone()).unwrap();
    state(|s| s.tracks[1].volume = 0.5);
    assert_eq!(code(lab.apply_with("lane-comp", &input, &preview, "Kumi: x (k1)")), "changed");
    // And since the comp.
    let (preview, applied) = lab.make("lane-comp", json!({"ref": KICK, "lane": 1, "start": 0.5, "end": 1.5}));
    state(|s| s.tracks[1].mute = true);
    assert_eq!(code(lab.undo("lane-comp", &preview, &applied)), "changed");
    // Discard too: the whole track from before the comp would write over the producer's edits (review R4).
    let lanes = state(|s| s.tracks[1].lane_names.len());
    let (code_, why) = lab.undo_with("lane-comp", &preview, &applied, true).unwrap_err();
    assert!(code_ == "changed" && why.contains("even with discard"), "{why}");
    assert_eq!(state(|s| s.tracks[1].lane_names.len()), lanes, "the track is left as the producer has it");
}

#[test]
fn every_lane_answer_names_refs_the_way_the_contract_says() {
    let lab = lanes_lab();
    for (kind, input) in [
        ("track-lanes", json!({"ref": KICK, "mode": "normal"})),
        ("lane-plays", json!({"ref": KICK, "lane": 0, "plays": "only"})),
        ("lane-name", json!({"ref": KICK, "lane": 0, "name": "x"})),
        ("razor-set", json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0}]})),
        ("lane-comp", json!({"ref": KICK, "lane": 1, "start": 0.5, "end": 1.5})),
    ] {
        let (preview, applied) = lab.make(kind, input);
        let restored = lab.undo(kind, &preview, &applied).unwrap();
        for answer in [&preview, &applied, &restored] {
            assert_eq!(daw_bridge::contract::misplaced_refs(answer), Vec::<String>::new(), "{kind}: {answer}");
        }
    }
}

// ----- what the producer changed between the preview and the apply -----

#[test]
fn a_lane_apply_is_refused_when_the_producer_changed_the_lanes_since_the_preview() {
    type Edit = fn(&mut crate::reaper::fake::State);
    let cases: [(&str, serde_json::Value, Edit); 4] = [
        ("track-lanes", json!({"ref": KICK, "mode": "normal"}), |s| {
            s.tracks[1].extra.insert("C_LANEPLAYS:1".into(), 1.0);
        }),
        ("lane-plays", json!({"ref": KICK, "lane": 0, "plays": "only"}), |s| {
            s.tracks[1].extra.insert("C_LANEPLAYS:1".into(), 0.0);
        }),
        ("lane-name", json!({"ref": KICK, "lane": 0, "name": "A"}), |s| s.tracks[1].lane_names[0] = "B".into()),
        ("razor-set", json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0}]}), |s| {
            s.items.razor.insert("{BBBB}".into(), "2.00000000000000 3.00000000000000 \"\" 0.000000 1.000000".into());
        }),
    ];
    for (kind, input, edit) in cases {
        let lab = lanes_lab();
        let preview = lab.preview(kind, input.clone()).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        state(edit);
        let before = state(|s| (s.tracks.clone(), s.items.razor.clone()));
        let answer = lab.apply_with(kind, &input, &preview, "Kumi: x (k1)");
        assert_eq!(answer.as_ref().err().map(|e| e.0.as_str()), Some("changed"), "{kind}: {answer:?}");
        assert!(state(|s| (s.tracks.clone(), s.items.razor.clone())) == before, "{kind}: nothing written");
    }
}

// ----- lanes come from items -----

#[test]
fn an_item_put_on_the_lane_after_the_last_adds_it_and_its_undo_drops_it() {
    let lab = lanes_lab();
    let (preview, applied) = lab.make("item-move", json!({"ref": "r:item:{ITEM-K1}", "lane": 2}));
    assert_eq!((item("{ITEM-K1}").unwrap().1.lane, lane_count()), (2, 3.0), "REAPER adds the lane for it");
    lab.undo("item-move", &preview, &applied).unwrap();
    assert_eq!((item("{ITEM-K1}").unwrap().1.lane, lane_count()), (0, 2.0), "and drops it once it is empty");
    let (preview, applied) = lab.make("item-add", json!({"ref": KICK, "position": 6.0, "length": 1.0, "lane": 2}));
    assert_eq!(lane_count(), 3.0);
    lab.undo("item-add", &preview, &applied).unwrap();
    assert_eq!(lane_count(), 2.0);
    let (code_, message) = lab.preview("item-move", json!({"ref": "r:item:{ITEM-K1}", "lane": 3})).unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("2 adds one at the bottom"), "{message}");
}

#[test]
fn picked_lanes_come_back_picked_and_a_track_whose_lanes_all_played_comes_back_unpicked() {
    let lab = lanes_lab();
    // The producer has both lanes picked to play (REAPER's LANESOLO with both): each picked, not all playing unpicked.
    state(|s| {
        s.tracks[1].extra.insert("C_LANEPLAYS:0".into(), 2.0);
        s.tracks[1].extra.insert("C_LANEPLAYS:1".into(), 2.0);
    });
    let (preview, applied) = lab.make("lane-plays", json!({"ref": KICK, "lane": 0, "plays": "only"}));
    assert_eq!(preview["prior"]["all"], 2);
    lab.undo("lane-plays", &preview, &applied).unwrap();
    assert!(picked() && plays() == vec![2.0, 2.0], "both picked again");
    // Lanes turned off and on again by Kumi: unpicked lanes stay unpicked.
    let lab = lanes_lab();
    let (preview, applied) = lab.make("track-lanes", json!({"ref": KICK, "mode": "normal"}));
    assert_eq!(preview["prior"]["all"], 1);
    lab.undo("track-lanes", &preview, &applied).unwrap();
    assert!(!picked());
}

/// Every lane kind's undo says when the track is back already (writing nothing) and when it is gone.
#[test]
fn every_lane_undo_says_back_already_and_gone() {
    let cases: [(&str, serde_json::Value); 5] = [
        ("track-lanes", json!({"ref": KICK, "mode": "normal"})),
        ("lane-plays", json!({"ref": KICK, "lane": 1, "plays": "only"})),
        ("lane-name", json!({"ref": KICK, "lane": 1, "name": "B"})),
        ("razor-set", json!({"ref": KICK, "areas": [{"start": 0.0, "end": 1.0}]})),
        ("lane-comp", json!({"ref": KICK, "lane": 1, "start": 0.5, "end": 1.5})),
    ];
    for (kind, input) in cases {
        let lab = lanes_lab();
        let (preview, applied) = lab.make(kind, input.clone());
        lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        let again = lab.undo(kind, &preview, &applied).unwrap_or_else(|e| panic!("{kind} again: {e:?}"));
        assert_eq!(again["display"]["note"], "It was back already: nothing was written.", "{kind}");
        let lab = lanes_lab();
        let (preview, applied) = lab.make(kind, input);
        state(|s| {
            s.tracks.remove(1);
        });
        assert_eq!(code(lab.undo(kind, &preview, &applied)), "gone", "{kind}");
    }
}
