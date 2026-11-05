//! The C2 allowlist of Live's `make_changes` steps that the Kumi for Hermes plugin's plans are checked against, with
//! `kumi_daw::plugin::check`. Every step `make_changes` can offer is in exactly one of [`PLUGIN_STEPS`] (allowed, with
//! the action values each refuses) and [`PLUGIN_REFUSED`] (refused, with the reason).

use kumi_daw::plugin::PluginStep;

/// The steps a plugin's plan may use in Live, each with its HISTORY family as changes.json gives it.
pub static PLUGIN_STEPS: &[PluginStep] = &[
    PluginStep::new("set_tempo", "tempo"),
    PluginStep::new("set_mixer", "mixer"),
    PluginStep::new("rename", "rename"),
    PluginStep::new("add_tracks_and_scenes", "structure"),
    PluginStep::new("write_midi_clip", "clip").noting(crate::context::NOTATION),
    PluginStep::new("load_device", "device"),
    PluginStep::new("set_device_parameter", "parameter"),
    // add-chain is the step's own word for insert-chain: Live gives Kumi no way to take a chain away again.
    PluginStep::new("edit_rack", "device").refusing(&[("action", &["remove-macro", "delete-variation", "insert-chain", "add-chain"])]),
    PluginStep::new("set_chain_mixer", "mixer"),
    PluginStep::new("set_locators", "locators"),
    PluginStep::new("set_track_color", "color"),
    PluginStep::new("set_mixer_options", "mixer"),
    // Where a track's sound comes from and goes, only: arming and monitoring are for recording, and Live crashes when a
    // track's input is Main (Live's host refuses that too).
    PluginStep::new("set_routing", "mixer")
        .refusing(&[("arm", &["*"]), ("monitoring", &["*"]), ("inputType", &["Main", "Master"])])
        .because(&[
            ("arm", "arming a track is for recording, which Kumi for Hermes doesn't do."),
            ("monitoring", "a track's monitoring is for recording, which Kumi for Hermes doesn't do."),
            ("inputType", "Live crashes when a track's input is Main (Live 11 called it Master); \"Resampling\" takes the mix."),
        ])
        .described(
            "Route a track: its input (inputType and inputSubRouting, such as another track and \"Post FX\", or \
             \"Resampling\" for the main output; never \"Main\", which crashes Live) and its output (outputType, \
             outputSubRouting), exactly as Live names them (live_discover kind routing-choice with the track as parent \
             lists them). Kumi for Hermes doesn't arm tracks or change their monitoring.",
        ),
    PluginStep::new("edit_rack_mapping", "device"),
    PluginStep::new("set_clip_follow_actions", "clip"),
    PluginStep::new("set_clip", "clip"),
    PluginStep::new("set_audio_clip", "clip"),
    PluginStep::new("duplicate_clip", "clip"),
    PluginStep::new("change_notes", "clip"),
    // select selects notes as a click would.
    PluginStep::new("edit_notes", "clip").refusing(&[("action", &["select", "delete-range"])]),
    PluginStep::new("transform_midi", "clip"),
    PluginStep::new("set_automation", "clip").refusing(&[("action", &["delete-range", "delete-envelope"])]),
    // Live gives Kumi no undo for delete-return.
    PluginStep::new("change_structure", "structure").refusing(&[("action", &["delete-return"])]),
    PluginStep::new("set_scene", "structure"),
    PluginStep::new("switch_device", "device"),
    PluginStep::new("move_device", "device"),
    PluginStep::new("move_device_to", "device"),
    PluginStep::new("set_chain", "device"),
    PluginStep::new("set_song", "tempo"),
    PluginStep::new("set_scale", "tempo"),
    PluginStep::new("set_groove", "tempo"),
    PluginStep::new("replace_sample", "device").file(
        "sample",
        "Swap the sample in a Simpler for another: deviceRef (the Simpler), and sample, where the file comes from: a \
         sound from the map, a file Kumi made, or an approved file, never a path.",
    ),
    PluginStep::new("import_audio", "clip").file(
        "sample",
        "Put an audio file into the Set as a clip, in an empty Session slot: trackRef (an audio track) and sceneIndex. \
         sample is where the file comes from: a sound from the map, a file Kumi made, or an approved file, never a path.",
    ),
    PluginStep::new("set_warp_markers", "clip").refusing(&[("action", &["delete"])]),
    PluginStep::new("write_arrangement_clip", "clip").noting(crate::context::NOTATION),
    // Slices removed or cleared, and the slices reset, don't come back as they were; resend's effect isn't described.
    PluginStep::new("edit_device", "device").refusing(&[(
        "action",
        &["slice-remove", "slice-clear", "slice-reset", "resend", "warp-as", "warp-double", "warp-half", "modulate"],
    )]),
    PluginStep::new("duplicate_device", "device"),
    PluginStep::new("set_sidechain", "device"),
    PluginStep::new("set_scene_follow_actions", "clip"),
    PluginStep::new("set_global_follow_actions", "clip"),
    PluginStep::new("set_note_expression", "clip"),
    // The action sits inside its edit object.
    PluginStep::new("edit_arrangement_automation", "clip").refusing(&[("action", &["delete"])]),
    PluginStep::new("map_modulator", "device"),
];

/// The other steps `make_changes` can offer, each with why a plugin's plan may not use it.
pub static PLUGIN_REFUSED: &[(&str, &str)] = &[
    ("load_sample", "It loads an audio file by path into a new Simpler."),
    ("load_sample_to_pad", "It loads an audio file by path onto a Drum Rack pad."),
    ("set_transport", "It moves the playhead and changes the metronome and punch settings."),
    ("edit_clip", "Live gives Kumi no undo for any of its edits, and crop destroys material."),
    ("move_clip", "It replaces clips where it drops one, and a copy or a move to another track has no Kumi undo."),
    ("add_arrangement_clip", "It puts an audio file in by path."),
    ("add_take_lane", "Live's API can't delete a take lane, so Kumi's undo can't take it back."),
    ("delete_notes", "It deletes notes."),
    ("capture_scene", "It captures what is playing now."),
    ("delete_device", "It deletes a device, and Kumi's undo can't bring it back."),
    ("capture_midi", "It captures what was played on armed tracks."),
    (
        "set_device_details",
        "Its settings are whatever the bridge describes per device, plug-in specifics included, so they can't be checked.",
    ),
    ("use_looper", "It records, plays, stops and clears a Looper."),
    ("delete_clip", "It deletes a clip."),
    ("delete_scene", "It deletes a scene and its clips, and Kumi's undo can't bring it back."),
    ("delete_track", "It deletes a track with its clips and devices, and Kumi's undo can't bring it back."),
    ("delete_locator", "It deletes a locator, and Kumi's undo can't bring it back."),
    ("clear_range", "It clears clips from a stretch of the Arrangement and cuts those crossing it."),
    ("group_tracks", "Live's API can't group tracks without Kumi's native editing; select the tracks and press Cmd+G (Ctrl+G on Windows)."),
    ("play", "It plays and stops Live."),
    ("fire_scene", "It launches a scene."),
    ("launch_clip", "It launches a clip."),
    ("record", "It records."),
    ("jump_to_locator", "It moves the playhead."),
    ("select", "It changes Live's selection."),
    ("show", "It changes Live's view."),
    ("wait", "It holds the write lease while nothing changes."),
];

#[cfg(test)]
mod tests {
    use super::{PLUGIN_REFUSED, PLUGIN_STEPS};
    use crate::{actions::ACTIONS, changes::CHANGES};
    use kumi_daw::plugin::check;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// Every step make_changes can offer: the CHANGES that aren't internal, every ACTIONS, and wait.
    fn plan_steps() -> BTreeSet<String> {
        CHANGES
            .iter()
            .filter(|kind| kind.internal != Some(true))
            .map(|kind| kind.tool.clone())
            .chain(ACTIONS.iter().map(|kind| kind.tool.clone()))
            .chain(["wait".to_owned()])
            .collect()
    }

    #[test]
    fn plugin_steps_partition_the_plan_enum() {
        let allowed: Vec<&str> = PLUGIN_STEPS.iter().map(|step| step.tool).collect();
        let refused: Vec<&str> = PLUGIN_REFUSED.iter().map(|(tool, _)| *tool).collect();
        let mut listed = BTreeSet::new();
        for tool in allowed.iter().chain(&refused) {
            assert!(listed.insert((*tool).to_owned()), "{tool} is listed twice");
        }
        assert_eq!(listed, plan_steps());
    }

    #[test]
    fn plugin_steps_have_their_registry_family() {
        for step in PLUGIN_STEPS {
            let kind = CHANGES.iter().find(|kind| kind.tool == step.tool).unwrap_or_else(|| panic!("{} isn't a change", step.tool));
            assert_eq!(serde_json::to_value(kind.family).unwrap(), json!(step.family), "{}", step.tool);
        }
    }

    #[test]
    fn plugin_steps_refuse_the_reviewers_examples() {
        assert!(check(PLUGIN_STEPS, "change_structure", &json!({"action":"delete-return","ref":"t1"})).is_err());
        assert!(check(PLUGIN_STEPS, "change_structure", &json!({"action":"create-return","name":"Verb"})).is_ok());
        assert!(check(PLUGIN_STEPS, "edit_rack", &json!({"rackRef":"d1","action":"delete-variation","index":0})).is_err());
        assert!(check(PLUGIN_STEPS, "edit_rack", &json!({"rackRef":"d1","action":"remove-macro"})).is_err());
        assert!(check(PLUGIN_STEPS, "edit_rack", &json!({"rackRef":"d1","action":"add-chain"})).is_err(), "no undo takes a chain away");
        assert!(check(PLUGIN_STEPS, "edit_device", &json!({"deviceRef":"d1","action":"slice-clear"})).is_err());
        assert!(check(PLUGIN_STEPS, "edit_device", &json!({"deviceRef":"d1","action":"resend"})).is_err());
        assert!(check(PLUGIN_STEPS, "edit_clip", &json!({"clipRef":"c1","action":"duplicate-loop"})).is_err());
        assert!(check(PLUGIN_STEPS, "wait", &json!({"seconds":10})).is_err());
        for tool in ["move_clip", "load_sample_to_pad", "capture_midi", "use_looper", "delete_track", "record"] {
            assert!(check(PLUGIN_STEPS, tool, &json!({})).is_err(), "{tool}");
        }
        // Routing, but nothing that records, and never Main as an input: Live crashes.
        for input in [
            json!({"trackRef":"t1","arm":true}),
            json!({"trackRef":"t1","arm":false}),
            json!({"trackRef":"t1","outputType":"Drum Bus","monitoring":"in"}),
            json!({"trackRef":"t1","inputType":"Main"}),
            json!({"trackRef":"t1","inputType":"Master"}),
        ] {
            assert!(check(PLUGIN_STEPS, "set_routing", &input).is_err(), "{input}");
        }
    }

    #[test]
    fn a_file_field_takes_only_a_source_never_a_search_of_folders() {
        for sample in [json!({"folders": ["/Users/x/Private"], "random": true}), json!({"words": ["kick"]}), json!("/Users/x/kick.wav")] {
            assert!(check(PLUGIN_STEPS, "replace_sample", &json!({"deviceRef": "1:device:0:0", "sample": sample})).is_err(), "{sample}");
        }
    }

    #[test]
    fn what_live_keeps_no_undo_for_is_refused_through_the_provider() {
        use kumi_daw::provider::DawProvider;
        let live = crate::app::provider::Ableton;
        for (tool, input) in [
            ("edit_rack", json!({"rackRef": "d1", "action": "insert-chain"})),
            (
                "import_audio",
                json!({"sample": {"sound_id": "s-r1-1", "map_revision": "r1"}, "takeLaneRef": "1:take_lane:0:0", "position": 4}),
            ),
            ("edit_device", json!({"deviceRef": "d1", "action": "warp-as"})),
            ("edit_device", json!({"deviceRef": "d1", "action": "warp-double"})),
            ("edit_device", json!({"deviceRef": "d1", "action": "warp-half"})),
            ("edit_device", json!({"deviceRef": "d1", "action": "modulate"})),
            ("edit_notes", json!({"clipRef": "c1", "action": "select"})),
        ] {
            assert!(live.plugin_refuses(tool, &input).is_some(), "{tool} {input}");
        }
        assert!(live
            .plugin_refuses("import_audio", &json!({"sample": {"artifact_id": "a-1"}, "trackRef": "1:track:0", "sceneIndex": 0}))
            .is_none());
        assert!(live.plugin_refuses("set_mixer", &json!({"trackRef": "1:track:0", "volume": 0.8})).is_none());
        // Every allowed step whose action Live marks permanent is refused by the list itself too.
        for step in PLUGIN_STEPS {
            for action in ["insert-chain", "warp-as", "warp-double", "warp-half", "modulate", "delete-variation", "delete-return"] {
                let input = json!({"action": action});
                if live.plugin_refuses(step.tool, &input).is_some() {
                    assert!(check(PLUGIN_STEPS, step.tool, &input).is_err(), "{} {action}", step.tool);
                }
            }
        }
    }

    #[test]
    fn plugin_steps_refuse_their_destructive_actions() {
        assert!(check(PLUGIN_STEPS, "edit_notes", &json!({"clipRef":"c1","action":"delete-range"})).is_err());
        assert!(check(PLUGIN_STEPS, "edit_notes", &json!({"clipRef":"c1","action":"select","all":true})).is_err());
        assert!(check(PLUGIN_STEPS, "edit_notes", &json!({"clipRef":"c1","action":"quantize","grid":0.25})).is_ok());
        assert!(check(PLUGIN_STEPS, "set_automation", &json!({"clipRef":"c1","action":"delete-envelope"})).is_err());
        assert!(check(PLUGIN_STEPS, "set_warp_markers", &json!({"clipRef":"c1","action":"delete","beatTime":4})).is_err());
        assert!(check(PLUGIN_STEPS, "set_warp_markers", &json!({"clipRef":"c1","action":"add","beatTime":4})).is_ok());
        assert!(
            check(PLUGIN_STEPS, "edit_arrangement_automation", &json!({"ref":"t1","edit":{"action":"delete","start":0,"end":4}})).is_err()
        );
    }

    #[test]
    fn plugin_steps_allow_a_mixer_change_a_route_and_sourced_files() {
        assert!(check(PLUGIN_STEPS, "set_mixer", &json!({"trackRef":"t1","volume":0.7,"pan":-0.2})).is_ok());
        for route in [
            json!({"trackRef":"t1","outputType":"Drum Bus"}),
            json!({"trackRef":"t2","inputType":"Bass","inputSubRouting":"Post FX"}),
            json!({"trackRef":"t2","inputType":"Resampling"}),
        ] {
            assert!(check(PLUGIN_STEPS, "set_routing", &route).is_ok(), "{route}");
        }
        let source = json!({"sound_id":"s-r1-1","map_revision":"r1"});
        for tool in ["replace_sample", "import_audio"] {
            assert!(check(PLUGIN_STEPS, tool, &json!({"deviceRef":"d1","trackRef":"t1","sample":source})).is_ok(), "{tool}");
            assert!(check(PLUGIN_STEPS, tool, &json!({"deviceRef":"d1","sample":"/Users/me/kick.wav"})).is_err(), "{tool}");
        }
    }
}
