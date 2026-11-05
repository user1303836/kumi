//! The C2 allowlist for REAPER: the plan steps a Kumi for Hermes plugin's plan may hold, which
//! `kumi_daw::plugin::check` checks each step against, and the reason every other step is refused.
use kumi_daw::plugin::PluginStep;

const fn step(tool: &'static str, family: &'static str) -> PluginStep {
    PluginStep::new(tool, family)
}

/// The steps a plugin's plan may hold, each with its HISTORY family.
pub static PLUGIN_STEPS: &[PluginStep] = &[
    step("track_volume", "mixer"),
    step("track_pan", "mixer"),
    step("track_mute", "mixer"),
    step("track_solo", "mixer"),
    step("track_channels", "mixer"),
    step("track_rename", "rename"),
    step("track_color", "color"),
    step("track_add", "structure"),
    step("track_move", "structure"),
    step("send_add", "mixer"),
    step("send_volume", "mixer"),
    step("send_pan", "mixer"),
    step("send_mute", "mixer"),
    step("send_channels", "mixer"),
    step("send_mode", "mixer"),
    step("marker_add", "locators"),
    step("regions_add", "locators"),
    step("region_add", "locators"),
    step("marker_move", "locators"),
    step("marker_rename", "rename"),
    step("marker_color", "color"),
    step("tempo_set", "tempo"),
    step("tempo_marker_set", "tempo"),
    step("loop_range", "locators"),
    step("fx_add", "device"),
    step("fx_move", "device"),
    step("fx_bypass", "device"),
    step("fx_pins", "device"),
    step("fx_param", "parameter"),
    step("notes_add", "clip"),
    step("notes_change", "clip"),
    step("fx_params", "parameter"),
    step("fx_offline", "device"),
    step("fx_wet", "parameter"),
    step("fx_eq_band", "parameter"),
    // By preset name, index or step only: the generic check refuses its `path` field.
    step("fx_preset", "device"),
    step("fx_copy", "device"),
    step("fx_container_add", "device"),
    step("fx_container_move", "device"),
    step("fx_parallel", "device"),
    step("fx_container_map", "parameter"),
    step("fx_param_lfo", "parameter"),
    step("fx_param_acs", "parameter"),
    step("fx_param_link", "parameter"),
    step("fx_param_learn", "parameter"),
    step("envelope_points_add", "parameter"),
    step("envelope_points_change", "parameter"),
    step("automation_item_add", "parameter"),
    step("automation_item_set", "parameter"),
    step("track_automation_mode", "mixer"),
    step("send_automation_mode", "mixer"),
    step("item_add", "clip"),
    step("item_import", "clip").file(
        "path",
        "Put an audio file on a track (ref) at positionBeats or position: it plays whole unless lengthBeats or length is \
         given. path is where the file comes from: a sound from the map, a file Kumi made, or an approved file, never a \
         path.",
    ),
    step("item_move", "clip"),
    step("item_split", "clip"),
    step("item_trim", "clip"),
    step("item_fade", "clip"),
    step("item_props", "clip"),
    step("item_group", "clip"),
    step("item_variation", "clip"),
    step("item_variation_set", "clip"),
    step("item_duplicate", "clip"),
    step("take_props", "clip"),
    step("take_active", "clip"),
    step("stretch_markers_add", "clip"),
    step("stretch_markers_change", "clip"),
    step("take_markers_add", "clip"),
    step("take_markers_change", "clip"),
    step("track_lanes", "structure"),
    step("lane_plays", "clip"),
    step("lane_name", "rename"),
    step("lane_comp", "clip"),
    step("midi_events_add", "clip"),
    step("midi_events_change", "clip"),
    step("notes_quantize", "clip"),
    step("notes_humanize", "clip"),
    step("track_phase", "mixer"),
    step("track_main_send", "mixer"),
    step("track_fx_enabled", "device"),
    step("track_pan_mode", "mixer"),
    step("track_width", "mixer"),
    step("track_dual_pan", "mixer"),
    step("track_pan_law", "mixer"),
    step("track_lock", "mixer"),
    step("track_solo_defeat", "mixer"),
    step("track_midi_out", "mixer"),
    step("receive_add", "mixer"),
    step("send_phase", "mixer"),
    step("send_mono", "mixer"),
    step("hw_out_add", "mixer"),
    step("hw_out_volume", "mixer"),
    step("hw_out_pan", "mixer"),
    step("hw_out_mute", "mixer"),
    step("hw_out_mode", "mixer"),
    step("hw_out_channels", "mixer"),
    step("hw_out_phase", "mixer"),
    step("hw_out_mono", "mixer"),
    step("project_notes", "rename"),
    step("project_info", "rename"),
    step("project_timebase", "tempo"),
    step("ruler_lane", "locators"),
    step("ruler_lane_add", "locators"),
    step("marker_lane", "locators"),
    step("region_render", "locators"),
    step("track_group", "mixer"),
    step("track_group_name", "rename"),
];

/// Every other plan step, with why a plugin's plan may not hold it.
pub static PLUGIN_REFUSED: &[(&str, &str)] = &[
    ("wait", "It holds the write lease for up to 1800 s a step."),
    ("track_delete", "It deletes a track with its items, FX and sends."),
    ("send_remove", "It deletes a send."),
    ("marker_delete", "It deletes a marker or region."),
    ("tempo_marker_delete", "It deletes a tempo marker."),
    ("loop_repeat", "It switches looped playback, a transport setting."),
    ("metronome_switch", "It switches the metronome, a playback setting."),
    ("fx_delete", "It deletes an FX and everything in a container."),
    ("notes_delete", "It deletes MIDI notes."),
    ("run_lua", "It runs Lua code in REAPER."),
    ("api_call", "It calls a REAPER API function by name."),
    ("fx_state", "Its input is a raw plug-in state chunk."),
    ("fx_chain_load", "It loads an FX chain file by name or absolute path."),
    ("fx_container_unmap", "It deletes a container macro and the automation on it."),
    ("fx_param_mod", "It can open the parameter modulation window (visible)."),
    ("envelope_show", "It shows, hides or arms envelopes: a view and recording change."),
    ("envelope_points_delete", "It deletes envelope points."),
    ("envelope_range_clear", "It clears an envelope's points over a range."),
    ("automation_item_delete", "It deletes an automation item."),
    ("automation_override", "It changes REAPER's global automation override, a setting outside the project's undo."),
    ("items_delete", "It deletes items with their takes."),
    ("item_delete", "It deletes an item with its takes."),
    ("take_add", "It plays a media file given by its full path."),
    ("take_delete", "It deletes a take."),
    ("stretch_markers_delete", "It deletes stretch markers."),
    ("take_markers_delete", "It deletes take markers."),
    ("razor_set", "It replaces the razor areas, an edit selection."),
    ("items_glue", "It glues items into a new file and deletes the originals."),
    ("items_render", "It renders items to new audio files."),
    ("midi_events_delete", "It deletes MIDI events."),
    ("track_arm", "It arms a track for recording."),
    ("track_record_input", "It chooses a track's recording input."),
    ("track_monitoring", "It changes record monitoring."),
    ("track_record_mode", "It chooses what an armed track records."),
    ("track_visible", "It shows or hides a track, a view change."),
    ("track_folder_state", "It opens or collapses a folder, a view change."),
    ("hw_out_remove", "It deletes a hardware output."),
    ("time_selection", "It changes the time selection."),
    ("project_grid", "It changes the arrange view's grid, a view setting."),
    ("run_action", "It runs a REAPER action by its id or name."),
    ("track_template_insert", "It loads a track template file by name or path."),
    ("drum_kit", "It loads sound files by path."),
    ("scratch_keep", "It keeps one of Kumi's scratch renders as a track: a judged round's work, which a plugin's plan doesn't make."),
    ("play", "It plays REAPER."),
    ("stop", "It stops REAPER's playback and recording."),
    ("set_cursor", "It moves the edit cursor."),
    ("fx_chain_save", "It saves an FX chain file."),
    ("save_project", "It saves the project's file."),
    ("save_track_template", "It saves a track template file."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::CHANGES;
    use kumi_daw::plugin::check;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// Every step make_changes offers: REAPER's change kinds, its actions, and `wait`.
    fn plan_steps() -> BTreeSet<&'static str> {
        CHANGES.kinds.iter().map(|kind| kind.tool).chain(CHANGES.actions.iter().map(|action| action.tool)).chain(["wait"]).collect()
    }

    #[test]
    fn plugin_steps_partition_the_plan_enum() {
        let allowed: Vec<_> = PLUGIN_STEPS.iter().map(|step| step.tool).collect();
        let refused: Vec<_> = PLUGIN_REFUSED.iter().map(|(tool, _)| *tool).collect();
        let listed: Vec<_> = allowed.iter().chain(&refused).copied().collect();
        let unique: BTreeSet<_> = listed.iter().copied().collect();
        assert_eq!(unique.len(), listed.len(), "a step is listed twice");
        assert_eq!(unique, plan_steps());
    }

    #[test]
    fn plugin_steps_keep_their_families_and_delete_nothing() {
        for step in PLUGIN_STEPS {
            let kind = CHANGES.kinds.iter().find(|kind| kind.tool == step.tool).unwrap();
            assert_eq!(serde_json::to_value(kind.family).unwrap(), json!(step.family), "{}", step.tool);
            assert!(!kind.deletes, "{} deletes", step.tool);
        }
    }

    #[test]
    fn plugin_steps_refuse_the_reviewers_cases() {
        let refused = |tool, input| assert!(check(PLUGIN_STEPS, tool, &input).is_err(), "{tool} {input}");
        refused("take_add", json!({"ref": "7:item:1", "path": "/Users/me/Audio/kick.wav"}));
        refused("fx_preset", json!({"ref": "7:fx:1", "path": "/Users/me/Presets/Lead.vstpreset"}));
        refused("wait", json!({"beats": 8}));
        refused("run_lua", json!({"code": "reaper.Main_OnCommand(40001, 0)"}));
        refused("api_call", json!({"fn": "DeleteTrack", "args": []}));
        refused("run_action", json!({"action": "40001"}));
        refused("fx_state", json!({"ref": "7:fx:1", "state": "AAAA"}));
        refused("fx_chain_load", json!({"ref": "7:track:1", "file": "Vocals/Lead"}));
        refused("track_template_insert", json!({"name": "Drums/Kit"}));
        refused("items_glue", json!({"itemRefs": ["7:item:1"]}));
        refused("track_delete", json!({"ref": "7:track:1"}));
        refused("item_import", json!({"ref": "7:track:1", "path": "/Users/me/Audio/kick.wav"}));
    }

    #[test]
    fn plugin_steps_allow_project_edits() {
        let allowed = |tool, input| assert_eq!(check(PLUGIN_STEPS, tool, &input).map(|step| step.tool), Ok(tool));
        allowed("track_volume", json!({"ref": "7:track:1", "volumeDb": -6}));
        allowed("fx_preset", json!({"ref": "7:fx:1", "preset": "Warm Lead"}));
        allowed("item_import", json!({"ref": "7:track:1", "path": {"sound_id": "s-r1-1", "map_revision": "r1"}}));
    }
}
