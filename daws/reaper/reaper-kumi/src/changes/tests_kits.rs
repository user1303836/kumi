//! A sampler kit made from sample files (m6-library, M6): a folder with a MIDI track and a ReaSamplOmatic5000 track per
//! pad, or one track with its pads in a container; the hi-hats choked through JS: MIDI Choke; one undo point, taken away by
//! Kumi's undo only while the kit is as it was made; refused when the project or a file changed after the preview, and
//! when a file isn't a sound; nothing left of a kit REAPER wouldn't make.

use serde_json::{json, Value};

use super::tests_lab::*;
use crate::reaper::{fake, Fx, FxOwner, Reaper};

/// A folder of sound files: a WAVE header and some bytes each.
fn sounds(names: &[&str]) -> (tempfile::TempDir, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let paths = names
        .iter()
        .map(|name| {
            let path = dir.path().join(format!("{name}.wav"));
            std::fs::write(&path, [b"RIFF\0\0\0\0WAVEfmt ".as_slice(), &[0u8; 64]].concat()).unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect();
    (dir, paths)
}

fn names() -> Vec<String> {
    state(|s| s.tracks.iter().map(|t| t.name.clone()).collect())
}

fn depths() -> Vec<i32> {
    state(|s| s.tracks.iter().map(|t| t.folder).collect())
}

/// What the fake's REAPER reads of a track's FX at `index`: its name, file, note range and note-offs (as 0 to 127, 0 or 1).
fn sampler(track: usize, fx: Option<i32>) -> (String, Option<String>, Option<String>, i64, i64, f64) {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    let owner = FxOwner::Track(reaper.track_at(proj, track as i32).unwrap());
    let fx = Fx { owner, index: fx.unwrap_or(0) };
    (
        reaper.fx_name(fx),
        reaper.fx_config(fx, "FILE0"),
        reaper.fx_config(fx, "MODE"),
        (reaper.param_normalized(fx, 3) * 127.0).round() as i64,
        (reaper.param_normalized(fx, 4) * 127.0).round() as i64,
        reaper.param_normalized(fx, 11),
    )
}

/// The raw sliders of a JS FX on a track.
fn sliders(track: usize, index: i32) -> Vec<f64> {
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    let fx = Fx { owner: FxOwner::Track(reaper.track_at(proj, track as i32).unwrap()), index };
    (0..reaper.fx_param_count(fx)).map(|p| reaper.fx_param_raw(fx, p).map(|(v, _, _)| v).unwrap_or(f64::NAN)).collect()
}

fn kit(paths: &[String], extra: Value) -> Value {
    let pads: Vec<Value> = paths.iter().map(|path| json!({"path": path})).collect();
    let mut input = json!({"pads": pads});
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    input
}

#[test]
fn a_pads_file_in_kumis_own_folder_plays_from_a_copy_in_the_projects_media_folder() {
    let lab = Lab::new();
    let kumi = tempfile::tempdir().unwrap();
    let media = tempfile::tempdir().unwrap();
    lab.reads.set_kumi_folder(kumi.path().to_path_buf());
    fake::with(|s| s.media = media.path().to_string_lossy().into_owned());
    let (_dir, theirs) = sounds(&["Kick"]);
    // (a render Kumi made, in its listening folder)
    let listening = kumi.path().join("bridges/reaper/listening");
    std::fs::create_dir_all(&listening).unwrap();
    let render = listening.join("Snare.wav");
    std::fs::copy(&theirs[0], &render).unwrap();
    let paths = vec![theirs[0].clone(), render.to_string_lossy().into_owned()];
    let (preview, made) = lab.make("drum-kit", kit(&paths, json!({})), "Kumi: kit (kk001)");
    assert!(preview["display"]["note"].as_str().unwrap().contains("play copies in the project's media folder"), "{}", preview["display"]);
    let copy = media.path().join("Snare.wav").to_string_lossy().into_owned();
    assert_eq!(sampler(6, None).1.as_deref(), Some(paths[0].as_str()), "the producer's own file stays where it is");
    assert_eq!(sampler(7, None).1.as_deref(), Some(copy.as_str()), "Kumi's render plays from its copy");
    assert_eq!(made["applied"]["pads"][1]["file"], copy.as_str());
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&render).unwrap());
}

#[test]
fn a_drum_kit_records_no_automation_under_a_global_override_that_writes() {
    // The pads' and the choke's parameter writes would be recorded under a global override in latch (review R5).
    let lab = Lab::new();
    let (_dir, paths) = sounds(&["Kick", "Closed Hat", "Open Hat"]);
    state(|s| s.automation_override = 4);
    let tracks = state(|s| s.tracks.len());
    lab.make("drum-kit", kit(&paths, json!({})), "Kumi: kit (kq001)");
    let recorded: usize = state(|s| s.tracks[tracks..].iter().map(|t| t.envelopes.len()).sum());
    assert_eq!((recorded, state(|s| s.automation_override)), (0, 4), "nothing recorded, the override put back");
}

#[test]
fn a_drum_kit_is_a_folder_a_midi_track_and_a_sampler_per_pad_its_hats_choking_and_one_undo_point() {
    let lab = Lab::new();
    fake::extra(|e| e.stable_chunks = true);
    let (_dir, paths) = sounds(&["Kick", "Snare", "Closed Hat", "Open Hat"]);
    let before = entries();
    let (preview, made) = lab.make("drum-kit", kit(&paths, json!({})), "Kumi: kit (kd001)");
    assert_eq!(made["display"]["title"], "Made the drum kit “Drums” with 4 pads; its hi-hats choke each other");
    assert_eq!(
        preview["display"]["note"],
        "Notes go on “Drums MIDI” (the track this made); pads: C2 Kick, C#2 Snare, D2 Closed Hat, D#2 Open Hat."
    );
    assert_eq!(entries(), before + 1, "one undo point");
    assert_eq!(top().as_deref(), Some("Kumi: kit (kd001)"));
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb", "Drums", "Drums MIDI", "Kick", "Snare", "Closed Hat", "Open Hat"]);
    assert_eq!(depths(), [1, 0, -1, 0, 1, 0, 0, 0, 0, -1], "the producer's folder as it was; the kit's closed");
    // Each pad: its file, played as it is, on its note; only the open hat obeys note-offs (the choke ends it).
    for (n, (note, obey)) in [(36, 0.0), (37, 0.0), (38, 0.0), (39, 1.0)].into_iter().enumerate() {
        let (name, file, mode, start, end, offs) = sampler(6 + n, None);
        assert_eq!(name, "VSTi: ReaSamplOmatic5000 (Cockos)");
        assert_eq!(file.as_deref(), Some(paths[n].as_str()), "the file, taken with DONE");
        assert_eq!(mode.as_deref(), Some("1"));
        assert_eq!((start, end, offs), (note, note, obey), "pad {n}");
    }
    // MIDI only, every channel, from the MIDI track to each pad; the choke ahead of the sends.
    state(|s| {
        let midi = &s.tracks[5];
        assert_eq!(midi.sends.iter().map(|send| send.dest).collect::<Vec<_>>(), [6, 7, 8, 9]);
        assert!(midi.sends.iter().all(|send| send.src_chan == -1 && send.extra.get("I_MIDIFLAGS").copied().unwrap_or(0.0) == 0.0));
        assert_eq!(midi.fx.len(), 1);
        assert_eq!(midi.fx[0].name, "JS: MIDI Choke");
    });
    assert_eq!(&sliders(5, 0)[..5], [1.0, 38.0, 1.0, 39.0, 1.0], "channel 1; Closed Hat cuts Open Hat");
    assert_eq!(made["created"]["ref"], json!(format!("r:track:{}", state(|s| s.tracks[5].guid.clone()))), "the track the notes go on");
    assert_eq!(made["created"]["trackRefs"].as_array().unwrap().len(), 6);
    assert!(fake::extra(|e| e.note_names.iter().any(|(_, pitch, _, name)| *pitch == 39 && name == "Open Hat")), "its notes named");
    lab.undo("drum-kit", &preview, &made, "Kumi: undo (kd002)").unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 0, -1, 0]);
    assert_eq!(lab.undo("drum-kit", &preview, &made, "Kumi: undo (kd003)").unwrap_err().0, "gone");
}

#[test]
fn a_kit_in_gm_notes_chokes_the_open_hat_from_the_closed_and_pedal_ones_and_goes_where_asked() {
    let lab = Lab::new();
    fake::extra(|e| e.stable_chunks = true);
    let (_dir, paths) = sounds(&["BD", "SD", "HH Closed", "HH Pedal", "HH Open"]);
    let mut input = kit(&paths, json!({"name": "GM Kit", "index": 2, "channel": 10}));
    for (n, note) in [36, 38, 42, 44, 46].into_iter().enumerate() {
        input["pads"][n]["note"] = note.into();
    }
    let (preview, made) = lab.make("drum-kit", input, "Kumi: kit (kg001)");
    // Inside the producer's folder, after its first track: their tracks keep their folder.
    assert_eq!(names(), ["Drums", "GM Kit", "GM Kit MIDI", "BD", "SD", "HH Closed", "HH Pedal", "HH Open", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 1, 0, 0, 0, 0, 0, -1, 0, -1, 0]);
    let choke = sliders(2, 0);
    assert_eq!(&choke[..5], [10.0, 42.0, 1.0, 46.0, 1.0], "closed at 42 chokes, the open at 46 is cut");
    assert_eq!(choke[6], 44.0, "the pedal hat chokes too, as one more choke note");
    assert_eq!(sampler(7, None).5, 1.0, "the open hat obeys note-offs");
    lab.undo("drum-kit", &preview, &made, "Kumi: undo (kg002)").unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 0, -1, 0]);
}

#[test]
fn a_kit_at_the_end_of_a_folder_in_a_folder_is_taken_away_leaving_every_track_in_its_folder() {
    let lab = Lab::new();
    fake::extra(|e| e.stable_chunks = true);
    drums_in_rhythm();
    let (_dir, paths) = sounds(&["Kick", "Snare"]);
    let (preview, made) = lab.make("drum-kit", kit(&paths, json!({"name": "Perc", "index": 4})), "Kumi: kit (kn001)");
    assert_eq!(names(), ["Rhythm", "Drums", "Kick", "Perc", "Perc MIDI", "Kick", "Snare", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 1, 0, 1, 0, 0, -1, -2, 0]);
    lab.undo("drum-kit", &preview, &made, "Kumi: undo (kn002)").unwrap();
    assert_eq!(names(), ["Rhythm", "Drums", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 1, 0, -2, 0], "Snare still closes both folders, and Reverb is in neither");
}

#[test]
fn a_container_kit_is_one_track_its_pads_in_one_container_behind_the_choke() {
    let lab = Lab::new();
    fake::extra(|e| e.stable_chunks = true);
    let (_dir, paths) = sounds(&["Kick", "Snare", "Closed Hat", "Open Hat"]);
    let (preview, made) = lab.make("drum-kit", kit(&paths, json!({"layout": "container", "name": "Kit"})), "Kumi: kit (kc001)");
    assert_eq!(made["display"]["title"], "Made the drum kit container “Kit” with 4 pads; its hi-hats choke each other");
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb", "Kit"]);
    state(|s| {
        let track = &s.tracks[4];
        assert_eq!(track.fx.len(), 2);
        assert_eq!(track.fx[0].name, "JS: MIDI Choke");
        let pads = track.fx[1].children.as_ref().expect("a container");
        assert_eq!(track.fx[1].renamed, "Kit");
        assert_eq!(pads.len(), 4);
        assert!(pads.iter().all(|pad| pad.name == "VSTi: ReaSamplOmatic5000 (Cockos)"));
        assert_eq!(pads[3].config.get("FILE0").map(String::as_str), Some(paths[3].as_str()));
        assert_eq!((pads[2].params[3].value * 127.0).round(), 38.0);
        assert_eq!(pads[3].params[11].value, 1.0);
    });
    assert_eq!(made["created"]["ref"], json!(format!("r:track:{}", state(|s| s.tracks[4].guid.clone()))));
    lab.undo("drum-kit", &preview, &made, "Kumi: undo (kc002)").unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

#[test]
fn a_container_kits_undo_leaves_it_once_the_producer_turned_a_pad_inside_the_container() {
    let lab = Lab::new();
    fake::extra(|e| e.stable_chunks = true);
    let (_dir, paths) = sounds(&["Kick", "Snare"]);
    let (preview, made) = lab.make("drum-kit", kit(&paths, json!({"layout": "container", "name": "Kit"})), "Kumi: kit (kc011)");
    // (no hi-hats, no choke: the container is the track's only FX)
    fn pads(s: &mut fake::State) -> &mut Vec<fake::FakeFx> {
        s.tracks[4].fx[0].children.as_mut().expect("the kit's container")
    }
    // A pad's sampler inside the container, its volume turned down.
    let was = state(|s| std::mem::replace(&mut pads(s)[0].params[0].value, 0.1));
    let (code, message) = lab.undo("drum-kit", &preview, &made, "Kumi: undo (kc012)").unwrap_err();
    assert_eq!(code, "changed", "{message}");
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb", "Kit"]);
    // And a plug-in the producer dropped into the container.
    state(|s| {
        pads(s)[0].params[0].value = was;
        pads(s).push(fake::FakeFx::new("{THEIRS}", "VST: ReaEQ (Cockos)"));
    });
    assert_eq!(lab.undo("drum-kit", &preview, &made, "Kumi: undo (kc013)").unwrap_err().0, "changed");
    state(|s| pads(s).pop());
    lab.undo("drum-kit", &preview, &made, "Kumi: undo (kc014)").unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

#[test]
fn hats_choke_only_when_some_close_and_some_open_and_a_reaper_without_the_choke_still_makes_the_kit() {
    let lab = Lab::new();
    let (_dir, paths) = sounds(&["Kick", "Closed Hat", "Open Hat", "Hat"]);
    // Turned off.
    lab.make("drum-kit", kit(&paths[..3], json!({"chokeHats": false})), "Kumi: kit (kh001)");
    assert!(state(|s| s.tracks[5].fx.is_empty()), "no choke");
    assert_eq!(sampler(8, None).5, 0.0, "the open hat obeys no note-off then");
    // Only closed hats: nothing to cut.
    let lab = Lab::new();
    lab.make("drum-kit", kit(&[paths[0].clone(), paths[3].clone()], json!({})), "Kumi: kit (kh002)");
    assert!(state(|s| s.tracks[5].fx.is_empty()));
    // A pad that names its part: the names' guess is left.
    let lab = Lab::new();
    let mut input = kit(&paths[..3], json!({}));
    input["pads"][0]["choke"] = "closes".into();
    input["pads"][2]["choke"] = "open".into();
    lab.make("drum-kit", input, "Kumi: kit (kh003)");
    assert_eq!(&sliders(5, 0)[1..5], [36.0, 1.0, 38.0, 1.0], "the kick chokes the open hat, as asked; the closed hat doesn't");
    // No JS: MIDI Choke in this REAPER.
    let lab = Lab::new();
    fake::extra(|e| e.no_choke = true);
    let (_, made) = lab.make("drum-kit", kit(&paths[..3], json!({})), "Kumi: kit (kh004)");
    assert!(made["display"]["note"].as_str().unwrap().ends_with("REAPER has no JS: MIDI Choke here, so the hi-hats don't choke."));
    assert!(state(|s| s.tracks[5].fx.is_empty()));
    assert_eq!(names().len(), 9, "the kit is made");
}

#[test]
fn a_kit_refuses_files_that_arent_sounds_and_requests_it_cant_make_with_nothing_made() {
    let lab = Lab::new();
    let (dir, paths) = sounds(&["Kick"]);
    let text = dir.path().join("notes.wav");
    std::fs::write(&text, "this is no wave at all, just words").unwrap();
    let refused = |input: Value| lab.preview("drum-kit", input).unwrap_err().0;
    assert_eq!(refused(json!({"pads": [{"path": text.to_string_lossy()}]})), "bad-params");
    assert_eq!(refused(json!({"pads": [{"path": "/nowhere/kick.wav"}]})), "bad-params");
    assert_eq!(refused(json!({"pads": [{"path": "kick.wav"}]})), "bad-params");
    assert_eq!(refused(json!({"pads": [{"path": dir.path().join("../x/kick.wav").to_string_lossy()}]})), "bad-params");
    assert_eq!(refused(json!({"pads": []})), "bad-params");
    assert_eq!(refused(kit(&vec![paths[0].clone(); 65], json!({}))), "bad-params");
    assert_eq!(refused(kit(&vec![paths[0].clone(); 3], json!({"first": 126}))), "bad-params");
    assert_eq!(refused(json!({"pads": [{"path": paths[0], "volumeDb": -6}], "layout": "container"})), "bad-params");
    assert_eq!(refused(kit(&paths, json!({"index": 9}))), "bad-params");
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

#[test]
fn a_kit_is_refused_when_the_tracks_or_a_file_changed_after_the_preview() {
    let lab = Lab::new();
    let (dir, paths) = sounds(&["Kick", "Snare"]);
    let input = kit(&paths, json!({"index": 2}));
    let preview = lab.preview("drum-kit", input.clone()).unwrap();
    state(|s| s.tracks.insert(1, fake::FakeTrack::new("{NEW1}", "Mine")));
    assert_eq!(lab.apply("drum-kit", &input, &preview, "Kumi: kit (kf001)").unwrap_err().0, "changed");
    assert_eq!(names(), ["Drums", "Mine", "Kick", "Snare", "Reverb"], "nothing made");
    // A file replaced after the preview.
    let preview = lab.preview("drum-kit", input.clone()).unwrap();
    std::fs::write(&paths[1], [b"RIFF\0\0\0\0WAVEfmt ".as_slice(), &[1u8; 200]].concat()).unwrap();
    assert_eq!(lab.apply("drum-kit", &input, &preview, "Kumi: kit (kf002)").unwrap_err().0, "changed");
    // One gone after the preview.
    let preview = lab.preview("drum-kit", input.clone()).unwrap();
    std::fs::remove_file(&paths[0]).unwrap();
    assert_eq!(lab.apply("drum-kit", &input, &preview, "Kumi: kit (kf003)").unwrap_err().0, "changed");
    assert_eq!(names().len(), 5);
    drop(dir);
}

#[test]
fn a_kit_isnt_undone_once_the_producer_mixed_renamed_or_reloaded_a_pad_or_put_a_track_in_its_folder() {
    let lab = Lab::new();
    fake::extra(|e| e.stable_chunks = true);
    let (_dir, paths) = sounds(&["Kick", "Snare", "Other"]);
    let (preview, made) = lab.make("drum-kit", kit(&paths[..2], json!({})), "Kumi: kit (kr001)");
    let refused = |what: &str| {
        let (code, message) = lab.undo("drum-kit", &preview, &made, "Kumi: undo (kr002)").unwrap_err();
        assert_eq!(code, "changed", "{what}: {message}");
        assert_eq!(names().len(), 8, "{what}: kept");
        message
    };
    state(|s| s.tracks[6].volume = 0.5);
    assert!(refused("a pad's fader").contains("“Kick” has been changed"));
    state(|s| s.tracks[6].volume = 1.0);
    state(|s| s.tracks[7].name = "My Snare".into());
    refused("a pad renamed");
    state(|s| s.tracks[7].name = "Snare".into());
    // Another sound loaded into a pad's sampler.
    state(|s| {
        s.tracks[6].fx[0].config.insert("FILE0".into(), paths[2].clone());
    });
    let message = refused("a pad's sound");
    assert!(message.contains("another sound"), "{message}");
    state(|s| {
        s.tracks[6].fx[0].config.insert("FILE0".into(), paths[0].clone());
    });
    // A track of the producer's dragged into the kit's folder: not even discard deletes the folder around it.
    state(|s| {
        let reverb = s.tracks.remove(3);
        s.tracks.insert(5, reverb);
    });
    let (code, message) = lab.undo_anyway("drum-kit", &preview, &made, "Kumi: undo (kr003)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("inside the kit's folder"), "{message}");
    state(|s| {
        let reverb = s.tracks.remove(5);
        s.tracks.insert(3, reverb);
    });
    // Discard takes it whatever was mixed.
    state(|s| s.tracks[6].volume = 0.5);
    lab.undo_anyway("drum-kit", &preview, &made, "Kumi: undo (kr004)").unwrap();
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
}

#[test]
fn a_kit_reaper_wont_finish_leaves_nothing_behind() {
    let lab = Lab::new();
    let (_dir, paths) = sounds(&["Kick", "Snare"]);
    fake::extra(|e| e.sampler_refuses_files = true);
    let (code, message) = lab.make_err("drum-kit", kit(&paths, json!({})));
    assert_eq!(code, "failed");
    assert!(message.contains("what Kumi made is taken away again"), "{message}");
    assert_eq!(names(), ["Drums", "Kick", "Snare", "Reverb"]);
    // At the end of a folder in a folder too, every track keeps its folder.
    drums_in_rhythm();
    let (code, _) = lab.make_err("drum-kit", kit(&paths, json!({"index": 4})));
    assert_eq!(code, "failed");
    assert_eq!(names(), ["Rhythm", "Drums", "Kick", "Snare", "Reverb"]);
    assert_eq!(depths(), [1, 1, 0, -2, 0]);
}

#[test]
fn an_fx_whose_track_reaper_replaced_is_never_read_through_the_old_track() {
    // (lab 2026-10-19: REAPER's undo and redo of a kit put its tracks back as new objects while the extension's watch
    // followed the samplers; reading the old track's chain crashed REAPER)
    let lab = Lab::new();
    let (_dir, paths) = sounds(&["Kick", "Snare"]);
    let (_, made) = lab.make("drum-kit", kit(&paths, json!({})), "Kumi: kit (kx001)");
    let sampler = made["applied"]["pads"][1]["fx"].as_str().unwrap().to_string();
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    assert!(lab.reads.fx_by_guid(&reaper, proj, &sampler).is_ok());
    // The last pad's track goes: the handle the registry kept is no track of REAPER's any more.
    state(|s| s.tracks.pop());
    assert!(lab.reads.fx_by_guid(&reaper, proj, &sampler).is_err());
    assert_eq!(state(|s| s.dead_handle_reads), 0, "no chain read through the track REAPER no longer has");
}
