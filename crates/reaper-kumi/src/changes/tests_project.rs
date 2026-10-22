//! The project beyond its tracks (r-tracks, M4): the time selection, notes, title and author, grid, timebase, ruler lanes,
//! a marker's lane and a region's render matrix.

use serde_json::json;

use super::tests_lab::*;
use crate::reaper::fake;

fn time_selection() -> (f64, f64) {
    state(|s| s.time_selection)
}

#[test]
fn the_time_selection_is_made_cleared_and_put_back_and_refused_when_it_moved() {
    let lab = Lab::new();
    let (preview, made) = lab.make("time-selection", json!({"start": 2.0, "end": 6.0}), "Kumi: x (kt001)");
    assert_eq!(time_selection(), (2.0, 6.0));
    assert_eq!(made["undo"]["entry"], true);
    assert!(made["display"]["title"].as_str().unwrap().starts_with("Time selection bar"), "{}", made["display"]["title"]);
    lab.undo("time-selection", &preview, &made, "Kumi: undo (kt002)").unwrap();
    assert_eq!(time_selection(), (0.0, 0.0));
    // Moved by the producer after the preview: refused. After the apply: the undo is refused, discard puts it back.
    let input = json!({"start": 1.0, "end": 3.0});
    let preview = lab.preview("time-selection", input.clone()).unwrap();
    state(|s| s.time_selection = (4.0, 5.0));
    assert_eq!(lab.apply("time-selection", &input, &preview, "Kumi: x (kt003)").unwrap_err().0, "changed");
    assert_eq!(time_selection(), (4.0, 5.0));
    let (preview, made) = lab.make("time-selection", json!({"start": 0.0, "end": 0.0}), "Kumi: x (kt004)");
    assert_eq!(made["display"]["title"], "Time selection cleared");
    state(|s| s.time_selection = (7.0, 8.0));
    assert_eq!(lab.undo("time-selection", &preview, &made, "Kumi: undo (kt005)").unwrap_err().0, "changed");
    lab.undo_anyway("time-selection", &preview, &made, "Kumi: undo (kt006)").unwrap();
    assert_eq!(time_selection(), (4.0, 5.0));
    assert_eq!(lab.preview("time-selection", json!({"start": 3.0, "end": 1.0})).unwrap_err().0, "bad-params");
}

#[test]
fn the_notes_title_and_author_are_written_and_put_back_and_the_notes_say_reaper_wont() {
    let lab = Lab::new();
    fake::extra(|e| e.notes = "Mix notes".into());
    let (preview, made) = lab.make("project-notes", json!({"notes": "Bridge needs a riser\nVocal comp: take 3"}), "Kumi: x (kn001)");
    assert_eq!(fake::extra(|e| e.notes.clone()), "Bridge needs a riser\nVocal comp: take 3");
    assert_eq!(made["display"]["title"], "Project notes changed");
    assert_eq!(made["display"]["note"], "REAPER's own undo won't take back the project's notes; Kumi can.");
    lab.undo("project-notes", &preview, &made, "Kumi: undo (kn002)").unwrap();
    assert_eq!(fake::extra(|e| e.notes.clone()), "Mix notes");
    // The producer edits them after the apply: refused.
    let (preview, made) = lab.make("project-notes", json!({"notes": ""}), "Kumi: x (kn003)");
    assert_eq!(made["display"]["title"], "Project notes cleared");
    fake::extra(|e| e.notes = "Mine".into());
    assert_eq!(lab.undo("project-notes", &preview, &made, "Kumi: undo (kn004)").unwrap_err().0, "changed");
    // Title and author.
    let (preview, made) = lab.make("project-info", json!({"title": "Night Drive", "author": "K."}), "Kumi: x (kn005)");
    assert_eq!(made["display"]["title"], "Project title “Night Drive”, author “K.”");
    // (7.82 lab: REAPER makes no undo point of them)
    assert_eq!(made["display"]["note"], "REAPER's own undo won't take back the project's title and author; Kumi can.");
    assert_eq!(fake::extra(|e| e.project_text.get("PROJECT_TITLE").cloned()).as_deref(), Some("Night Drive"));
    lab.undo("project-info", &preview, &made, "Kumi: undo (kn006)").unwrap();
    assert_eq!(fake::extra(|e| e.project_text.get("PROJECT_TITLE").cloned()).as_deref(), Some(""));
    assert_eq!(lab.preview("project-info", json!({"title": "two\nlines"})).unwrap_err().0, "bad-params");
}

#[test]
fn the_grid_takes_note_values_swing_and_the_measure_grid() {
    let lab = Lab::new();
    let grid = || fake::extra(|e| e.grid);
    let (preview, made) = lab.make("project-grid", json!({"division": "1/16", "swing": 0.3}), "Kumi: x (kg001)");
    assert_eq!(grid(), (0.0625, 1, 0.3));
    assert_eq!(made["display"]["title"], "Grid 1/16, swing 30%");
    assert_eq!(made["display"]["note"], "REAPER's own undo won't take back the grid; Kumi can.");
    lab.undo("project-grid", &preview, &made, "Kumi: undo (kg002)").unwrap();
    assert_eq!(grid(), (0.25, 0, 0.0));
    let (_, made) = lab.make("project-grid", json!({"division": "1/8T"}), "Kumi: x (kg003)");
    assert!((grid().0 - 0.125 * 2.0 / 3.0).abs() < 1e-12);
    assert_eq!(made["display"]["title"], "Grid 1/8 triplet");
    let (_, made) = lab.make("project-grid", json!({"division": "1/4.", "measure": true}), "Kumi: x (kg004)");
    assert_eq!((grid().0, grid().1), (0.375, 3));
    assert_eq!(made["display"]["title"], "Grid 1/4 dotted, the measure grid");
    for wrong in [json!({"division": "fast"}), json!({"division": 0}), json!({"swing": 2.0}), json!({})] {
        assert_eq!(lab.preview("project-grid", wrong.clone()).unwrap_err().0, "bad-params", "{wrong}");
    }
}

#[test]
fn the_projects_timebase_is_a_project_setting_put_back_by_kumi() {
    let lab = Lab::new();
    let config = |name: &str| fake::extra(|e| e.config.get(name).cloned()).unwrap();
    let (preview, made) = lab.make("project-timebase", json!({"items": "time", "tempoEnvelopes": "time"}), "Kumi: x (kb001)");
    assert_eq!((config("itemtimelock"), config("tempoenvtimelock")), ("0".into(), "0".into()));
    assert_eq!(made["display"]["title"], "Project timebase time, tempo envelope timebase time");
    lab.undo("project-timebase", &preview, &made, "Kumi: undo (kb002)").unwrap();
    assert_eq!((config("itemtimelock"), config("tempoenvtimelock")), ("1".into(), "1".into()));
    assert_eq!(lab.preview("project-timebase", json!({"tempoEnvelopes": "beats-position"})).unwrap_err().0, "bad-params");
}

#[test]
fn a_ruler_lane_is_named_hidden_and_put_back_by_its_id() {
    let lab = Lab::new();
    let (preview, made) = lab.make("ruler-lane", json!({"lane": 1, "name": "Sections", "hidden": true}), "Kumi: x (kl001)");
    assert_eq!(preview["target"]["guid"], "{LANE-1}");
    assert_eq!(made["display"]["title"], "Ruler lane 1 named “Sections”, hidden");
    assert_eq!(fake::extra(|e| e.project_text.get("RULER_LANE_NAME:1").cloned()).as_deref(), Some("Sections"));
    assert_eq!(fake::extra(|e| e.project_info.get("RULER_LANE_HIDDEN:1").copied()), Some(1.0));
    lab.undo("ruler-lane", &preview, &made, "Kumi: undo (kl002)").unwrap();
    assert_eq!(fake::extra(|e| e.project_text.get("RULER_LANE_NAME:1").cloned()).as_deref(), Some(""));
    // A lane that isn't there any more: gone.
    let (preview, made) = lab.make("ruler-lane", json!({"lane": 1, "locked": true}), "Kumi: x (kl003)");
    fake::extra(|e| e.project_info.insert("RULER_LANE_COUNT".into(), 1.0));
    assert_eq!(lab.undo("ruler-lane", &preview, &made, "Kumi: undo (kl004)").unwrap_err().0, "gone");
    assert_eq!(lab.preview("ruler-lane", json!({"lane": 4, "name": "x"})).unwrap_err().0, "bad-params");
}

#[test]
fn a_ruler_lane_is_added_and_taken_away_again_only_while_nothing_is_on_it() {
    let lab = Lab::new();
    let lanes = || fake::extra(|e| e.project_info.get("RULER_LANE_COUNT").copied()).unwrap();
    let (preview, made) = lab.make("ruler-lane-add", json!({"name": "Hits"}), "Kumi: x (ka001)");
    assert_eq!((lanes(), made["created"]["guid"].as_str()), (3.0, Some("{LANE-2}")));
    assert_eq!(made["display"]["title"], "Added ruler lane 2 named “Hits”");
    lab.undo("ruler-lane-add", &preview, &made, "Kumi: undo (ka002)").unwrap();
    assert_eq!(lanes(), 2.0);
    // A marker the producer put on it keeps it.
    let (preview, made) = lab.make("ruler-lane-add", json!({}), "Kumi: x (ka003)");
    state(|s| s.marker_lanes.insert("{MARK-1}".into(), 2));
    let (code, message) = lab.undo("ruler-lane-add", &preview, &made, "Kumi: undo (ka004)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("markers or regions on it"), "{message}");
    assert_eq!(lanes(), 3.0);
    // The markers are the producer's: not even told to undo anyway.
    let (code, _) = lab.undo_anyway("ruler-lane-add", &preview, &made, "Kumi: undo (ka005)").unwrap_err();
    assert_eq!((code.as_str(), lanes()), ("changed", 3.0));
}

/// The producer reorders the first two ruler lanes: each lane keeps its id, at the other's number.
fn swap_first_two_lanes() {
    fake::extra(|e| {
        e.project_text.insert("RULER_LANE_GUID:0".into(), "{LANE-1}".into());
        e.project_text.insert("RULER_LANE_GUID:1".into(), "{LANE-0}".into());
    });
}

#[test]
fn a_ruler_lane_change_writes_the_lane_the_preview_named_or_none() {
    let lab = Lab::new();
    let input = json!({"lane": 1, "name": "Cues"});
    let preview = lab.preview("ruler-lane", input.clone()).unwrap();
    swap_first_two_lanes();
    let (code, message) = lab.apply("ruler-lane", &input, &preview, "Kumi: x (kr001)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("another lane now"), "{message}");
    assert_eq!(fake::extra(|e| e.project_text.get("RULER_LANE_NAME:1").cloned()), None, "nothing written");
}

#[test]
fn a_marker_goes_back_to_its_lane_by_the_lanes_id_after_the_lanes_are_reordered() {
    let lab = Lab::new();
    fake::extra(|e| e.project_info.insert("RULER_LANE_COUNT".into(), 3.0));
    let lane = || state(|s| s.marker_lanes.get("{MARK-1}").copied().unwrap_or(0));
    state(|s| s.marker_lanes.insert("{MARK-1}".into(), 1));
    let (preview, made) = lab.make("marker-lane", json!({"ref": "r:marker:{MARK-1}", "lane": 2}), "Kumi: x (kg001)");
    assert_eq!((lane(), made["applied"]["laneGuid"].as_str()), (2, Some("{LANE-2}")));
    // The lane it came from is first now.
    swap_first_two_lanes();
    lab.undo("marker-lane", &preview, &made, "Kumi: undo (kg002)").unwrap();
    assert_eq!(lane(), 0, "back in its lane, wherever that is now");
    // Its lane gone: nowhere to put it back to.
    let (preview, made) = lab.make("marker-lane", json!({"ref": "r:marker:{MARK-1}", "lane": 2}), "Kumi: x (kg003)");
    fake::extra(|e| {
        e.project_text.insert("RULER_LANE_GUID:0".into(), "{LANE-9}".into());
    });
    let (code, message) = lab.undo_anyway("marker-lane", &preview, &made, "Kumi: undo (kg004)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("isn't in the project any more"), "{message}");
    assert_eq!(lane(), 2);
}

#[test]
fn a_marker_moves_to_another_ruler_lane_and_back() {
    let lab = Lab::new();
    let lane = || state(|s| s.marker_lanes.get("{MARK-1}").copied().unwrap_or(0));
    fake::extra(|e| e.project_text.insert("RULER_LANE_NAME:1".into(), "Cues".into()));
    let (preview, made) = lab.make("marker-lane", json!({"ref": "r:marker:{MARK-1}", "lane": 1}), "Kumi: x (km001)");
    assert_eq!(lane(), 1);
    assert_eq!(made["display"]["title"], "Moved marker “Drop” to ruler lane 1 (“Cues”)");
    lab.undo("marker-lane", &preview, &made, "Kumi: undo (km002)").unwrap();
    assert_eq!(lane(), 0);
    let (code, message) = lab.preview("marker-lane", json!({"ref": "r:marker:{MARK-1}", "lane": 5})).unwrap_err();
    assert_eq!(code, "bad-params");
    assert!(message.contains("2 ruler lanes"), "{message}");
}

#[test]
fn a_regions_render_matrix_gains_and_loses_tracks_and_only_those_are_put_back() {
    let lab = Lab::new();
    let matrix = || state(|s| s.render_matrix.get(&1).cloned().unwrap_or_default());
    state(|s| s.render_matrix.insert(1, vec!["{CCCC}".into()]));
    let input = json!({"ref": "r:region:{REG-1}", "addRefs": [KICK, "r:track:master"], "removeRefs": [SNARE]});
    let (preview, made) = lab.make("region-render", input, "Kumi: x (kx001)");
    assert_eq!(matrix(), ["{BBBB}", "master"]);
    assert_eq!(made["display"]["title"], "Region “Verse” renders “Kick”, the master; no longer renders “Snare”");
    // The producer adds Reverb meanwhile: not Kumi's, and it stays.
    state(|s| s.render_matrix.get_mut(&1).unwrap().push("{DDDD}".into()));
    lab.undo("region-render", &preview, &made, "Kumi: undo (kx002)").unwrap();
    assert_eq!(matrix(), ["{DDDD}", "{CCCC}"]);
    // The producer took out the track Kumi added: it is back as it was, and nothing is written.
    let (preview, made) = lab.make("region-render", json!({"ref": "r:region:{REG-1}", "addRefs": [KICK]}), "Kumi: x (kx003)");
    state(|s| s.render_matrix.get_mut(&1).unwrap().retain(|id| id != "{BBBB}"));
    let again = lab.undo("region-render", &preview, &made, "Kumi: undo (kx004)").unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
    // A track Kumi added that was deleted since: gone (discard puts back the rest).
    let (preview, made) =
        lab.make("region-render", json!({"ref": "r:region:{REG-1}", "addRefs": [KICK, "r:track:master"]}), "Kumi: x (kx005)");
    state(|s| s.tracks.remove(1));
    assert_eq!(lab.undo("region-render", &preview, &made, "Kumi: undo (kx006)").unwrap_err().0, "gone");
    lab.undo_anyway("region-render", &preview, &made, "Kumi: undo (kx007)").unwrap();
    assert!(!matrix().contains(&"master".to_string()));
    for wrong in [
        json!({"ref": "r:marker:{MARK-1}", "addRefs": [KICK]}),
        json!({"ref": "r:region:{REG-1}"}),
        json!({"ref": "r:region:{REG-1}", "addRefs": [SNARE], "removeRefs": [SNARE]}),
    ] {
        assert_eq!(lab.preview("region-render", wrong.clone()).unwrap_err().0, "bad-params", "{wrong}");
    }
}

#[test]
fn the_project_row_says_its_lanes_named_groups_grid_timebase_title_and_notes() {
    let lab = Lab::new();
    let project = |lab: &Lab| lab.call("read", json!({"kind": "project"})).unwrap()["items"][0].clone();
    let row = project(&lab);
    assert_eq!(
        row["rulerLanes"],
        json!([{"lane": 0, "name": "", "defaultFor": "regions"}, {"lane": 1, "name": "", "defaultFor": "markers"}])
    );
    assert_eq!(row["grid"], json!({"division": 0.25, "swing": 0.0, "measure": false}));
    assert_eq!(row["timebase"], json!({"items": "beats", "tempoEnvelopes": "beats"}));
    for absent in ["trackGroups", "title", "author", "notes"] {
        assert!(row.get(absent).is_none(), "{absent} only when set");
    }
    lab.make("ruler-lane", json!({"lane": 1, "name": "Cues", "hidden": true}), "Kumi: x (kr001)");
    lab.make("track-group-name", json!({"groupNumber": 2, "name": "Drums"}), "Kumi: x (kr002)");
    lab.make("project-info", json!({"title": "Night Drive"}), "Kumi: x (kr003)");
    lab.make("project-notes", json!({"notes": "n".repeat(2500)}), "Kumi: x (kr004)");
    lab.make("project-grid", json!({"division": "1/16", "swing": 0.25}), "Kumi: x (kr005)");
    let row = project(&lab);
    assert_eq!(row["rulerLanes"][1], json!({"lane": 1, "name": "Cues", "defaultFor": "markers", "hidden": true}));
    assert_eq!(row["trackGroups"], json!([{"group": 2, "name": "Drums"}]));
    assert_eq!((row["title"].as_str(), row["notesClipped"].as_bool()), (Some("Night Drive"), Some(true)));
    assert_eq!(row["notes"].as_str().unwrap().len(), 2000);
    assert_eq!(row["grid"], json!({"division": 0.0625, "swing": 0.25, "measure": false}));
}

#[test]
fn a_ruler_lane_added_and_then_taken_away_by_the_producer_is_gone_to_undo() {
    let lab = Lab::new();
    let (preview, made) = lab.make("ruler-lane-add", json!({}), "Kumi: x (kg001)");
    fake::extra(|e| e.project_info.insert("RULER_LANE_COUNT".into(), 2.0));
    assert_eq!(lab.undo("ruler-lane-add", &preview, &made, "Kumi: undo (kg002)").unwrap_err().0, "gone");
    // A grid set back by hand: back already, nothing written.
    let (preview, made) = lab.make("project-grid", json!({"division": "1/8"}), "Kumi: x (kg003)");
    fake::extra(|e| e.grid = (0.25, 0, 0.0));
    let again = lab.undo("project-grid", &preview, &made, "Kumi: undo (kg004)").unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

// ----- the metronome (m6-places) -----

fn metronome_flags() -> String {
    fake::extra(|e| e.config["projmetroen"].clone())
}

fn set_metronome_flags(flags: &str) {
    fake::extra(|e| e.config.insert("projmetroen".into(), flags.into()));
}

#[test]
fn the_metronome_is_switched_and_put_back_keeping_its_other_options() {
    let lab = Lab::new();
    let project = |lab: &Lab| lab.call("read", json!({"kind": "project"})).unwrap()["items"][0].clone();
    assert_eq!(project(&lab)["metronome"], false);
    // Count-in options set (2 and 4): a switch keeps them.
    set_metronome_flags("6");
    let (preview, made) = lab.make("metronome-switch", json!({"on": true}), "Kumi: x (km001)");
    assert_eq!((preview["prior"].clone(), metronome_flags()), (json!({"on": false}), "7".to_string()));
    assert_eq!(made["display"]["title"], "Metronome on");
    assert_eq!(project(&lab)["metronome"], true, "the project row says it");
    lab.undo("metronome-switch", &preview, &made, "Kumi: undo (km002)").unwrap();
    assert_eq!(metronome_flags(), "6");
    // Already on: nothing to write.
    set_metronome_flags("1");
    assert_eq!(lab.preview("metronome-switch", json!({"on": true})).unwrap()["display"]["unchanged"], true);
    for wrong in [json!({}), json!({"on": "yes"})] {
        assert_eq!(lab.preview("metronome-switch", wrong.clone()).unwrap_err().0, "bad-params", "{wrong}");
    }
}

#[test]
fn a_metronome_switched_by_the_producer_since_is_refused_or_back_already() {
    let lab = Lab::new();
    // Switched in REAPER after the preview: the apply is refused, nothing written.
    let preview = lab.preview("metronome-switch", json!({"on": true})).unwrap();
    set_metronome_flags("1");
    assert_eq!(lab.apply("metronome-switch", &json!({"on": true}), &preview, "Kumi: x (km003)").unwrap_err().0, "changed");
    assert_eq!(metronome_flags(), "1");
    // Switched back off in REAPER after Kumi turned it on: back already, nothing written.
    set_metronome_flags("0");
    let (preview, made) = lab.make("metronome-switch", json!({"on": true}), "Kumi: x (km004)");
    set_metronome_flags("0");
    let again = lab.undo("metronome-switch", &preview, &made, "Kumi: undo (km005)").unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
    // The producer turned the count-in on since Kumi turned the metronome on: the undo turns it off and keeps theirs.
    let (preview, made) = lab.make("metronome-switch", json!({"on": true}), "Kumi: x (km006)");
    set_metronome_flags("9");
    assert_eq!(lab.undo("metronome-switch", &preview, &made, "Kumi: undo (km007)").unwrap()["restored"], json!({"on": false}));
    assert_eq!(metronome_flags(), "8");
}

#[test]
fn many_regions_are_added_in_one_step_with_their_matrix_and_the_undo_takes_away_only_those_as_added() {
    let lab = Lab::new();
    let before = state(|s| s.markers.len());
    let input = json!({"regions": [
        {"position": 20.0, "end": 21.5, "name": "Footstep_Gravel_01", "renderRefs": [KICK]},
        {"position": 22.0, "end": 23.25, "name": "Footstep_Gravel_02", "renderRefs": [KICK, "r:track:master"]},
        {"positionBeats": 50.0, "endBeats": 52.0, "name": "Footstep_Gravel_03"}
    ]});
    let (preview, made) = lab.make("regions-add", input, "Kumi: x (krs01)");
    assert_eq!(preview["display"]["title"], "Added 3 regions, “Footstep_Gravel_01” to “Footstep_Gravel_03”");
    assert_eq!(state(|s| s.markers.len()), before + 3);
    let refs = made["created"]["regionRefs"].as_array().unwrap().clone();
    assert_eq!(refs.len(), 3);
    assert_eq!(made["created"]["ref"], refs[0]);
    // Each region renders the tracks named in the matrix.
    let numbers: Vec<i32> = state(|s| s.markers.iter().filter(|m| m.name.starts_with("Footstep")).map(|m| m.number).collect());
    let matrix = |n: i32| state(|s| s.render_matrix.get(&n).cloned().unwrap_or_default());
    assert_eq!(matrix(numbers[0]), ["{BBBB}"]);
    assert_eq!(matrix(numbers[1]), ["{BBBB}", "master"]);
    assert!(matrix(numbers[2]).is_empty());
    // The producer renamed one since: the undo is refused, nothing deleted; told to anyway, all three go.
    state(|s| s.markers.iter_mut().find(|m| m.name == "Footstep_Gravel_02").unwrap().name = "Mine".into());
    assert_eq!(lab.undo("regions-add", &preview, &made, "Kumi: undo (krs02)").unwrap_err().0, "changed");
    assert_eq!(state(|s| s.markers.len()), before + 3);
    state(|s| s.markers.iter_mut().find(|m| m.name == "Mine").unwrap().name = "Footstep_Gravel_02".into());
    // A track the producer put in one's matrix since is their work too.
    state(|s| s.render_matrix.get_mut(&numbers[0]).unwrap().push("{AAAA}".into()));
    assert_eq!(lab.undo("regions-add", &preview, &made, "Kumi: undo (krs03)").unwrap_err().0, "changed");
    state(|s| s.render_matrix.get_mut(&numbers[0]).unwrap().retain(|id| id != "{AAAA}"));
    // One deleted in REAPER since: the others go, it isn't missed.
    state(|s| s.markers.retain(|m| m.name != "Footstep_Gravel_03"));
    let undone = lab.undo("regions-add", &preview, &made, "Kumi: undo (krs04)").unwrap();
    assert_eq!(undone["display"]["title"], "2 regions taken away again");
    assert_eq!(state(|s| s.markers.len()), before);
    // All gone already: nothing written.
    let again = lab.undo("regions-add", &preview, &made, "Kumi: undo (krs05)").unwrap();
    assert_eq!(again["display"]["title"], "The regions are gone already");
    for wrong in [
        json!({"regions": []}),
        json!({"regions": [{"position": 2.0}]}),
        json!({"regions": [{"position": 2.0, "end": 1.0}]}),
        json!({"regions": [{"position": 1.0, "end": 2.0, "renderRefs": ["r:region:{REG-1}"]}]}),
    ] {
        assert_eq!(lab.preview("regions-add", wrong.clone()).unwrap_err().0, "bad-params", "{wrong}");
    }
    assert_eq!(
        lab.preview("regions-add", json!({"regions": [{"position": 1.0, "end": 2.0, "renderRefs": ["r:track:{NOPE}"]}]})).unwrap_err().0,
        "failed"
    );
}
