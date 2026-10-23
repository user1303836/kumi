//! Glue and render-in-place (m4-r-items): applied, undone by Kumi, refused when the producer changed what they made, and
//! the producer's selection left as it was.

use serde_json::json;

use super::tests_items::{code, guid_in, item, state, Lab, K1};

const K2: &str = "r:item:{ITEM-K2}";

#[test]
fn glued_items_come_back_from_their_chunks_and_the_glued_one_goes() {
    let lab = Lab::new();
    // The producer has the snare's item selected.
    state(|s| s.tracks[2].items[0].selected = true);
    let (before1, before2) = (item("{ITEM-K1}").unwrap().1, item("{ITEM-K2}").unwrap().1);
    let (preview, applied) = lab.make("items-glue", json!({"itemRefs": [K1, K2]}));
    assert_eq!(preview["display"]["title"], "Glued 2 items on “Kick”");
    assert!(preview["display"]["note"].as_str().unwrap().contains("undo_change leaves the file"));
    let glued = guid_in(&applied["created"]["ref"]);
    let (track, made) = item(&glued).unwrap();
    assert_eq!((track, made.position, made.length), (1, 0.0, 4.0));
    assert!(item("{ITEM-K1}").is_none() && item("{ITEM-K2}").is_none());
    assert!(state(|s| s.tracks[2].items[0].selected && !s.tracks[1].items[0].selected), "the producer's selection stays");
    lab.undo("items-glue", &preview, &applied).unwrap();
    assert!(item(&glued).is_none());
    assert!(item("{ITEM-K1}").unwrap().1 == before1 && item("{ITEM-K2}").unwrap().1 == before2, "both items are back as they were");
    let again = lab.undo("items-glue", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn a_glue_is_undone_only_while_what_it_made_is_unchanged() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("items-glue", json!({"itemRefs": [K1, K2]}));
    let glued = guid_in(&applied["created"]["ref"]);
    state(|s| s.tracks[1].items.iter_mut().find(|i| i.guid == glued).unwrap().mute = true);
    assert_eq!(code(lab.undo("items-glue", &preview, &applied)), "changed");
    assert!(item("{ITEM-K1}").is_none(), "nothing brought back");
    // Discard too: taking the glued item away would take the producer's change with it (review R4).
    let (code_, why) = lab.undo_with("items-glue", &preview, &applied, true).unwrap_err();
    assert!(code_ == "changed" && why.contains("even with discard"), "{why}");
    assert!(item("{ITEM-K1}").is_none() && item(&glued).is_some_and(|(_, made)| made.mute), "the producer's glued item stays");
    // An item changed between the preview and the apply.
    let lab = Lab::new();
    let input = json!({"itemRefs": [K1, K2]});
    let preview = lab.preview("items-glue", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].length = 1.5);
    let actions = state(|s| s.actions.len());
    assert_eq!(code(lab.apply_with("items-glue", &input, &preview, "Kumi: x (k1)")), "changed");
    assert_eq!(state(|s| s.actions.len()), actions, "nothing run");
}

#[test]
fn too_much_audio_to_render_in_one_step_is_refused() {
    let lab = Lab::new();
    state(|s| s.tracks[1].items[0].length = 40.0);
    let (code_, message) = lab.preview("items-glue", json!({"itemRefs": [K1]})).unwrap_err();
    assert_eq!(code_, "bad-params");
    assert!(message.contains("up to 30 s"), "{message}");
    assert_eq!(code(lab.preview("items-render", json!({"itemRefs": [K1]}))), "bad-params");
    // A MIDI item without take FX glues as MIDI: nothing is rendered.
    state(|s| s.tracks[2].items[0].length = 40.0);
    assert!(lab.preview("items-glue", json!({"itemRefs": ["r:item:{ITEM-S1}"]})).is_ok());
}

#[test]
fn a_render_gives_each_item_a_new_playing_take_and_its_undo_puts_them_back() {
    let lab = Lab::new();
    let before = item("{ITEM-K1}").unwrap().1;
    let (preview, applied) = lab.make("items-render", json!({"itemRefs": [K1, K2], "fx": "track"}));
    assert_eq!(preview["display"]["title"], "Rendered 2 items to new takes (track and take FX)");
    assert!(preview["display"]["note"].as_str().unwrap().contains("bypass them"));
    let now = item("{ITEM-K1}").unwrap().1;
    assert_eq!((now.takes.len(), now.active), (2, 1));
    assert_eq!(guid_in(&applied["created"]["ref"]), now.takes[1].guid);
    assert_eq!(state(|s| s.actions.clone()), vec![40209]);
    lab.undo("items-render", &preview, &applied).unwrap();
    assert!(item("{ITEM-K1}").unwrap().1 == before);
    assert_eq!(item("{ITEM-K2}").unwrap().1.takes.len(), 1);
    // The producer edits one of them since: theirs.
    let (preview, applied) = lab.make("items-render", json!({"itemRefs": [K1]}));
    assert_eq!(state(|s| *s.actions.last().unwrap()), 41999);
    state(|s| s.tracks[1].items[0].active = 0);
    assert_eq!(code(lab.undo("items-render", &preview, &applied)), "changed");
    // Discard too: the item from before would write over the take the producer picked (review R4).
    let (code_, why) = lab.undo_with("items-render", &preview, &applied, true).unwrap_err();
    assert!(code_ == "changed" && why.contains("even with discard"), "{why}");
    assert_eq!((item("{ITEM-K1}").unwrap().1.active, item("{ITEM-K1}").unwrap().1.takes.len()), (0, 2));
    assert_eq!(code(lab.preview("items-render", json!({"itemRefs": [K1], "fx": "all"}))), "bad-params");
}

#[test]
fn every_glue_answer_names_refs_the_way_the_contract_says() {
    let lab = Lab::new();
    for (kind, input) in [("items-glue", json!({"itemRefs": [K1, K2]})), ("items-render", json!({"itemRefs": ["r:item:{ITEM-S1}"]}))] {
        let (preview, applied) = lab.make(kind, input);
        let restored = lab.undo(kind, &preview, &applied).unwrap();
        for answer in [&preview, &applied, &restored] {
            assert_eq!(daw_bridge::contract::misplaced_refs(answer), Vec::<String>::new(), "{kind}: {answer}");
        }
    }
}

#[test]
fn a_render_is_refused_when_an_item_changed_since_the_preview() {
    let lab = Lab::new();
    let input = json!({"itemRefs": [K1]});
    let preview = lab.preview("items-render", input.clone()).unwrap();
    state(|s| s.tracks[1].items[0].length = 1.5);
    let actions = state(|s| s.actions.len());
    assert_eq!(code(lab.apply_with("items-render", &input, &preview, "Kumi: x (k1)")), "changed");
    assert_eq!(state(|s| s.actions.len()), actions, "nothing run");
}

#[test]
fn a_glue_whose_item_the_producer_deleted_stays_undone_by_them() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("items-glue", json!({"itemRefs": [K1, K2]}));
    let glued = guid_in(&applied["created"]["ref"]);
    // The producer deletes what the glue made: bringing the items back would undo their delete.
    state(|s| s.tracks[1].items.retain(|i| i.guid != glued));
    let (code_, message) = lab.undo("items-glue", &preview, &applied).unwrap_err();
    assert_eq!(code_, "changed", "{message}");
    assert!(item("{ITEM-K1}").is_none() && item("{ITEM-K2}").is_none(), "nothing brought back");
}

#[test]
fn a_render_undo_says_back_already_and_gone() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("items-render", json!({"itemRefs": [K1]}));
    lab.undo("items-render", &preview, &applied).unwrap();
    let again = lab.undo("items-render", &preview, &applied).unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
    let lab = Lab::new();
    let (preview, applied) = lab.make("items-render", json!({"itemRefs": [K1]}));
    state(|s| s.tracks[1].items.retain(|i| i.guid != "{ITEM-K1}"));
    assert_eq!(code(lab.undo("items-render", &preview, &applied)), "gone");
    // A glue whose originals' track went: the glued item went with it, so it is the producer's (changed), not gone.
    let lab = Lab::new();
    let (preview, applied) = lab.make("items-glue", json!({"itemRefs": [K1, K2]}));
    state(|s| {
        s.tracks.remove(1);
    });
    assert_eq!(code(lab.undo("items-glue", &preview, &applied)), "changed");
}
