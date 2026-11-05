use kumi_agent::core::{
    refs::{Moved, RefScheme, Shift},
    view::*,
};
use kumi_daw_logic::{LogicRefs, DAW};
use serde_json::json;

#[test]
fn a_track_inserted_moves_the_tracks_after_it_with_their_plugins_and_sends() {
    let refs = LogicRefs;
    let shift = Shift { tracks_made: vec![2], ..Default::default() };
    assert_eq!(refs.moved(&shift, "7:track:1"), Moved::Same);
    assert_eq!(refs.moved(&shift, "7:track:2"), Moved::To("7:track:3".into()));
    assert_eq!(refs.moved(&shift, "7:plugin:4.2"), Moved::To("7:plugin:5.2".into()));
    assert_eq!(refs.moved(&shift, "7:send:2.1"), Moved::To("7:send:3.1".into()));
    // A region counted through the project, and a note under it, sit on no position a track shift moves.
    assert_eq!(refs.moved(&shift, "7:region:2"), Moved::Same);
    assert_eq!(refs.moved(&shift, "7:note:2.3"), Moved::Same);
    // Nested under their track, they follow it.
    assert_eq!(refs.moved(&shift, "7:region:2.1"), Moved::To("7:region:3.1".into()));
    assert_eq!(refs.moved(&shift, "7:note:2.1.3"), Moved::To("7:note:3.1.3".into()));
    let gone = Shift { tracks_gone: vec![2], ..Default::default() };
    assert_eq!(refs.moved(&gone, "7:plugin:2.1"), Moved::Gone);
    assert_eq!(refs.track_of("7:plugin:3.1"), Some("7:track:3".into()));
    assert_eq!(refs.track_of("7:send:3.2"), Some("7:track:3".into()));
    assert_eq!(refs.track_of("7:region:2.1"), Some("7:track:2".into()));
    assert_eq!(refs.track_of("7:region:1"), None);
    assert_eq!(refs.track_of("7:note:1.3"), None);
    assert_eq!(refs.track_of("7:marker:1"), None);
    assert_eq!(refs.kind_of("7:note:1.3"), Some("note"));
    assert_eq!(refs.kind_of("track:3"), None);
    assert!(refs.positional());
    assert_eq!(refs.refs_in("see 7:track:2 and 7:note:1.3 then 7:marker:1"), ["7:track:2", "7:note:1.3", "7:marker:1"]);
    assert_eq!(refs.parents("note"), Some(vec!["region".to_string()]));
    assert_eq!(refs.parents("region"), Some(vec!["track".to_string()]));
    assert_eq!(refs.parents("marker"), None);
}

#[test]
fn the_focus_is_the_tracks_area_with_the_editor_over_it_and_the_selected_region() {
    let focus = json!({
        "view": "tracks", "editor": "pianoRoll",
        "selection": {"trackRef": "1:track:2", "track": {"name": "Keys", "kind": "instrument", "color": "#5b8def"}, "regionRef": "1:region:1", "regionName": "Keys", "pluginRef": "1:plugin:2.1", "pluginName": "Alchemy"},
    });
    let filled = fill_focus(&focus);
    assert_eq!(filled.main_view.as_ref().map(|v| (v.kind, v.label.as_str())), Some((ViewKind::Timeline, "Tracks")));
    assert_eq!(filled.editor_view.as_ref().map(|v| (v.kind, v.label.as_str())), Some((ViewKind::NoteEditor, "Piano Roll")));
    let track = filled.track.as_ref().expect("the selected track");
    assert_eq!((track.name.as_str(), track.kind, track.color.as_deref()), ("Keys", Some(TrackKind::Midi), Some("#5b8def")));
    assert_eq!(filled.track_ref.as_deref(), Some("1:track:2"));
    let item = filled.item.as_ref().expect("the selected region");
    assert_eq!((item.kind, item.item_ref.as_deref(), item.name.as_deref()), (ItemKind::Item, Some("1:region:1"), Some("Keys")));
    assert_eq!((filled.device_ref.as_deref(), filled.device.as_deref()), (Some("1:plugin:2.1"), Some("Alchemy")));
    let mixer = fill_focus(&json!({"view": "mixer", "selection": {}}));
    assert_eq!(mixer.main_view.as_ref().map(|v| v.kind), Some(ViewKind::Mixer));
    assert!(mixer.editor_view.is_none() && mixer.item.is_none() && mixer.track.is_none());
}

#[test]
fn a_strip_is_a_tree_of_its_inserts_and_its_routing_its_inputs_sends_and_output() {
    let body = json!({"kind": "tree", "trackRef": "1:track:1", "strip": {"name": "Vocal", "inserts": [
        {"slot": 1, "name": "Channel EQ", "bypassed": false, "pluginRef": "1:plugin:1.1", "kind": "effect"},
        {"slot": 2, "name": "Alchemy", "bypassed": false, "pluginRef": "1:plugin:1.2", "kind": "instrument"}
    ], "sends": []}});
    let tree = strip_tree("1:track:1", &body);
    assert_eq!(tree.track_ref, "1:track:1");
    assert_eq!(
        tree.devices.iter().map(|d| (d.r#ref.as_str(), d.name.as_str())).collect::<Vec<_>>(),
        [("1:plugin:1.1", "Channel EQ"), ("1:plugin:1.2", "Alchemy")]
    );
    assert_eq!(tree.devices[0].role, Some(DeviceRole { signal: DeviceSignal::Audio, form: DeviceForm::Plugin }));
    assert_eq!(tree.devices[1].role.map(|role| role.signal), Some(DeviceSignal::Instrument));
    let routing = strip_routing(
        "1:track:1",
        &json!({"trackRef": "1:track:1", "inputs": ["Input 1"], "output": "Stereo Out", "sends": [{"slot": 1, "destination": "Bus 1", "level": -6.0, "sendRef": "1:send:1.1"}]}),
    );
    let routes: Vec<(RouteKind, &str, Option<f64>)> =
        routing.routes.iter().map(|route| (route.kind, route.other.as_str(), route.volume_db)).collect();
    assert_eq!(
        routes,
        [(RouteKind::Receive, "Input 1", None), (RouteKind::Send, "Bus 1", Some(-6.0)), (RouteKind::Hardware, "Stereo Out", None)]
    );
}

#[test]
fn a_regions_notes_are_read_from_logics_positions_and_the_tracks_area_from_its_lanes() {
    let position = |bar: u32, beat: u32, division: u32, tick: u32| json!({"bar": bar, "beat": beat, "division": division, "tick": tick});
    let body = json!({"kind": "notes", "regionRef": "1:region:1", "startBeats": 4.0, "endBeats": 12.0, "timeSignature": "3/4", "notes": [
        {"ref": "1:note:1.1", "position": position(2, 2, 1, 1), "pitch": 60, "velocity": 100, "lengthTicks": 240, "channel": 1},
        {"ref": "1:note:1.2", "position": position(2, 2, 3, 121), "pitch": 64, "velocity": 96, "lengthTicks": 960, "channel": 1, "selected": true},
        {"ref": "1:note:1.3", "position": position(1, 1, 1, 1), "pitch": 67, "velocity": 92, "lengthTicks": 480, "channel": 1}
    ]});
    let notes = region_notes("1:region:1", &body);
    assert_eq!((notes.slot_ref.as_str(), notes.length, notes.beats_per_bar), ("1:region:1", 8.0, Some(3.0)));
    let seen: Vec<(f64, f64, f64, Option<bool>)> =
        notes.notes.iter().map(|n| (n.note.pitch, n.note.start, n.note.duration, n.selected)).collect();
    // Bar 2 beat 2 of 3/4 is beat 4 of the project: the region's start. A division is a quarter beat, a tick 1/960.
    assert_eq!(seen[0], (60.0, 0.0, 0.25, None));
    assert_eq!(seen[1], (64.0, 0.5 + 120.0 / 960.0, 1.0, Some(true)));
    assert_eq!(seen[2], (67.0, 0.0, 0.5, None), "a note before the region's start isn't drawn before it");
    let timeline = lanes_timeline(&json!({"kind": "timeline", "window": {"from": 0.0, "to": 16.0}, "trackCount": 2, "from": 0, "lanes": [
        {"trackRef": "1:track:1", "name": "Vocal", "kind": "audio", "regions": []},
        {"trackRef": "1:track:2", "name": "Keys", "kind": "instrument", "regions": [{"ref": "1:region:1", "name": "Keys", "startBeats": 8.0, "endBeats": 24.0, "kind": "midi", "selected": true}]}
    ]}));
    let lanes = timeline.lanes.as_ref().unwrap();
    assert_eq!(
        lanes.iter().map(|lane| (lane.name.as_str(), lane.track_ref.as_deref(), lane.items.len())).collect::<Vec<_>>(),
        [("Vocal", Some("1:track:1"), 0), ("Keys", Some("1:track:2"), 1)]
    );
    assert_eq!((lanes[1].items[0].start, lanes[1].items[0].length, lanes[1].items[0].selected), (8.0, 16.0, true));
    assert_eq!((timeline.length, timeline.track_count, timeline.lanes_from), (24.0, Some(2), Some(0)), "as long as the furthest region");
}

fn fill_focus(value: &serde_json::Value) -> DawFocus {
    (DAW.fill_focus)(value)
}
fn strip_tree(track: &str, value: &serde_json::Value) -> DeviceTree {
    let Some(FocusBody::Tree(body)) = (DAW.to_body)(&BodyRequest::Tree { track_ref: track.into() }, value) else { panic!("tree") };
    body
}
fn region_notes(region: &str, value: &serde_json::Value) -> NoteView {
    let Some(FocusBody::Notes(body)) = (DAW.to_body)(&BodyRequest::Notes { item_ref: region.into() }, value) else { panic!("notes") };
    body
}
fn lanes_timeline(value: &serde_json::Value) -> TimelineStrip {
    let Some(FocusBody::Timeline(body)) = (DAW.to_body)(&BodyRequest::Timeline, value) else { panic!("timeline") };
    body
}
fn strip_routing(track: &str, value: &serde_json::Value) -> RoutingView {
    let Some(FocusBody::Routing(body)) = (DAW.to_body)(&BodyRequest::Routing { track_ref: track.into() }, value) else { panic!("routing") };
    body
}
