//! The Set model: built from the observation's track and device rows, found by id, ref or name, and diffed
//! between builds.
use kumi_runtime::{
    core::contracts::JsonObject,
    integrations::ableton::set_model::{normalize, Diff, SetModel},
};
use serde_json::{json, Value};

const BASS: &str = "01J9ZQ3V6M8K2D7X4N5P0R1S2A";
const DRUMS: &str = "01J9ZQ3V6M8K2D7X4N5P0R1S2B";
const PAD: &str = "01J9ZQ3V6M8K2D7X4N5P0R1S2C";

fn rows(values: Value) -> Vec<JsonObject> {
    values.as_array().unwrap().iter().map(|v| v.as_object().unwrap().clone()).collect()
}
fn tracks(list: &[(&str, &str, Option<&str>)]) -> Vec<JsonObject> {
    rows(json!(list
        .iter()
        .map(|(reference, name, id)| json!({"ref":reference,"name":name,"kind":"regular","mediaKind":"midi","kumiTrack":id}))
        .collect::<Vec<_>>()))
}

#[test]
fn a_set_is_built_from_the_rows_the_observation_reads_with_devices_one_rack_level_down() {
    let tracks = rows(json!([
        {"ref":"1:track:0","name":"Drums","kind":"group","mediaKind":"midi","kumiTrack":DRUMS},
        {"ref":"1:track:1","name":"Bass","kind":"regular","mediaKind":"midi","groupTrackRef":"1:track:0","kumiTrack":BASS},
        {"ref":"1:track:2","name":"Pad","kind":"regular","mediaKind":"audio","kumiTrack":"not an id"}
    ]));
    let devices = rows(json!([
        {"ref":"1:device:1:0","parentRef":"1:track:1","name":"Rack","className":"InstrumentGroupDevice","chainList":[{"ref":"1:chain:1:0:0","name":"Low"}]},
        {"ref":"1:device:1:0:0:0","parentRef":"1:chain:1:0:0","name":"Operator","className":"Operator"},
        {"ref":"1:device:1:1","parentRef":"1:track:1","name":"EQ Eight","className":"Eq8"}
    ]));
    let model = SetModel::next(&SetModel::default(), &tracks, &devices, true);
    assert_eq!((model.generation, model.complete, model.tracks.len()), (1, true, 3));
    let bass = model.track(BASS).unwrap();
    assert_eq!((bass.index, bass.group.as_deref(), bass.devices.len()), (1, Some(DRUMS), 2), "its group by the group's id");
    let rack = &bass.devices[0];
    assert_eq!((rack.name.as_str(), rack.class.as_deref(), rack.chains[0].name.as_str()), ("Rack", Some("InstrumentGroupDevice"), "Low"));
    assert_eq!(
        rack.chains[0].devices.iter().map(|d| (d.name.as_str(), d.class.clone())).collect::<Vec<_>>(),
        [("Operator", None)],
        "a class the same as the name isn't kept"
    );
    // Found by Kumi's id, by its ref now, or by name however it's written; text that isn't an id is no id.
    assert_eq!(model.track("1:track:1").map(|t| t.name.as_str()), Some("Bass"));
    assert_eq!(model.tracks_named(" BASS! ").iter().map(|t| t.key()).collect::<Vec<_>>(), [BASS]);
    let pad = model.track("1:track:2").unwrap();
    assert_eq!((pad.id.as_deref(), pad.key()), (None, "1:track:2"));
    assert!(model.tracks_named("bassline").is_empty());
    // A row without a ref or a name can't be found again: it isn't in the model.
    let odd = rows(json!([{"ref":null,"name":"Ghost"},{"ref":"1:track:9","name":null},{"ref":"1:track:1","name":"Bass"}]));
    assert_eq!(SetModel::next(&model, &odd, &[], true).tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Bass"]);
    assert_eq!(normalize("Kick 2 (Main)"), "kick2main");
}

#[test]
fn a_track_inserted_moves_the_refs_but_not_the_ids_and_a_build_says_what_changed() {
    let first =
        SetModel::next(&SetModel::default(), &tracks(&[("1:track:0", "Drums", Some(DRUMS)), ("1:track:1", "Bass", Some(BASS))]), &[], true);
    // A pad above the bass: its ref and index move, its id and generation stay.
    let second = SetModel::next(
        &first,
        &tracks(&[("1:track:0", "Drums", Some(DRUMS)), ("1:track:1", "Pad", Some(PAD)), ("1:track:2", "Bass", Some(BASS))]),
        &[],
        true,
    );
    let bass = second.track(BASS).unwrap();
    assert_eq!((bass.reference.as_str(), bass.index, bass.generation), ("1:track:2", 2, 1));
    assert_eq!(second.changed_since(1).iter().map(|t| t.key()).collect::<Vec<_>>(), [PAD]);
    assert_eq!(first.diff(&second), Diff { added: vec![PAD.into()], moved: vec![(BASS.into(), 1, 2)], ..Default::default() });
    // Renamed and one removed: the name lookups follow.
    let third = SetModel::next(&second, &tracks(&[("1:track:0", "Drums", Some(DRUMS)), ("1:track:1", "Sub Bass", Some(BASS))]), &[], true);
    assert_eq!(
        second.diff(&third),
        Diff {
            removed: vec![PAD.into()],
            renamed: vec![(BASS.into(), "Bass".into(), "Sub Bass".into())],
            moved: vec![(BASS.into(), 2, 1)],
            ..Default::default()
        }
    );
    assert!(third.tracks_named("bass").is_empty() && third.tracks_named("sub bass")[0].generation == 3);
    assert!(third.diff(&third).is_empty());
}

#[test]
fn a_track_inserted_above_a_group_changes_nothing_of_the_tracks_inside_it() {
    let grouped = |above: bool| {
        let mut list = vec![json!({"ref":"1:track:0","name":"Drums","kind":"group","kumiTrack":DRUMS})];
        if above {
            list.insert(0, json!({"ref":"1:track:0","name":"Pad","kind":"regular","kumiTrack":PAD}));
            list[1]["ref"] = json!("1:track:1");
        }
        let group = list.last().unwrap()["ref"].clone();
        list.push(json!({"ref":format!("1:track:{}", list.len()),"name":"Bass","kind":"regular","groupTrackRef":group,"kumiTrack":BASS}));
        rows(json!(list))
    };
    let first = SetModel::next(&SetModel::default(), &grouped(false), &[], true);
    let second = SetModel::next(&first, &grouped(true), &[], true);
    // The group is known by its id: the bass inside it moved, but nothing of it changed.
    assert_eq!(second.track(BASS).unwrap().group.as_deref(), Some(DRUMS));
    assert_eq!(second.changed_since(1).iter().map(|t| t.key()).collect::<Vec<_>>(), [PAD]);
}

#[test]
fn without_ids_from_live_tracks_are_known_by_place_and_a_cut_short_read_says_so() {
    // A bridge from before track ids: the model knows a track by its ref, so an insert reads as a rename.
    let first = SetModel::next(&SetModel::default(), &tracks(&[("1:track:0", "Drums", None), ("1:track:1", "Bass", None)]), &[], false);
    assert!(!first.complete, "a page was cut short");
    let second = SetModel::next(
        &first,
        &tracks(&[("1:track:0", "Drums", None), ("1:track:1", "Pad", None), ("1:track:2", "Bass", None)]),
        &[],
        true,
    );
    assert_eq!(
        first.diff(&second),
        Diff { added: vec!["1:track:2".into()], renamed: vec![("1:track:1".into(), "Bass".into(), "Pad".into())], ..Default::default() }
    );
}
