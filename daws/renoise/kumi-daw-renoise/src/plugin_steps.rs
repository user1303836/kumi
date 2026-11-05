//! The C2 allowlist of Renoise's change steps that a Kumi for Hermes plugin's plans are checked against, with
//! [`kumi_daw::plugin::check`]: the steps a plan may use (`PLUGIN_STEPS`) and every other make_changes step with why
//! it's refused (`PLUGIN_REFUSED`).
use kumi_daw::plugin::PluginStep;

/// The make_changes steps a plugin's plan may use in Renoise, each with its HISTORY family.
pub static PLUGIN_STEPS: &[PluginStep] = &[
    PluginStep::new("track_volume", "mixer"),
    PluginStep::new("track_pan", "mixer"),
    PluginStep::new("track_mute", "mixer"),
    PluginStep::new("track_solo", "mixer"),
    PluginStep::new("track_width", "mixer"),
    PluginStep::new("track_output", "mixer"),
    PluginStep::new("track_delay", "mixer"),
    PluginStep::new("song_headroom", "mixer"),
    PluginStep::new("track_rename", "rename"),
    PluginStep::new("instrument_rename", "rename"),
    PluginStep::new("pattern_name", "rename"),
    PluginStep::new("song_info", "rename"),
    PluginStep::new("track_color", "color"),
    PluginStep::new("slot_color", "color"),
    PluginStep::new("track_add", "structure"),
    PluginStep::new("track_move", "structure"),
    PluginStep::new("track_group", "structure"),
    PluginStep::new("instrument_add", "structure"),
    PluginStep::new("instrument_duplicate", "structure"),
    PluginStep::new("instrument_move", "structure"),
    // It may add, never clear.
    PluginStep::new("phrase_add", "structure").refusing(&[("clear", &["*"])]),
    // Replacing a sample's audio in place makes it a `.deletes()` kind, but Kumi keeps the audio it replaces and
    // undo_change puts it back. The import step fills `path` from an approved source.
    PluginStep::new("sample_load", "structure").file(
        "path",
        "Load an audio file as a new sample of an instrument (ref: the instrument; at index, its number from 0, the end \
         by default), or into a sample in place of its audio (ref: the sample; Kumi keeps the audio it replaces to put \
         back), named name. path is where the file comes from: a sound from the map, a file Kumi made, or an approved \
         file, never a path. Renoise takes up to 96 MB; an approved file, which Kumi copies first, is at most 50 MB.",
    ),
    PluginStep::new("song_bpm", "tempo"),
    PluginStep::new("song_lpb", "tempo"),
    PluginStep::new("song_tpl", "tempo"),
    PluginStep::new("song_groove", "tempo"),
    // `path` names a device in Renoise's browser ("Audio/Effects/Native/EQ 10"), never a file.
    PluginStep::new("device_add", "device").browser("path"),
    PluginStep::new("device_move", "device"),
    PluginStep::new("device_bypass", "device"),
    PluginStep::new("device_set", "device"),
    PluginStep::new("device_preset", "device"),
    PluginStep::new("modset_add", "device"),
    PluginStep::new("modset_set", "device"),
    // `path` names a device in Renoise's browser ("Audio/Effects/Native/EQ 10"), never a file.
    PluginStep::new("moddevice_add", "device").browser("path"),
    PluginStep::new("moddevice_set", "device"),
    PluginStep::new("chain_add", "device"),
    PluginStep::new("chain_set", "device"),
    // `path` names a device in Renoise's browser ("Audio/Effects/Native/EQ 10"), never a file.
    PluginStep::new("chaindevice_add", "device").browser("path"),
    PluginStep::new("chaindevice_move", "device"),
    PluginStep::new("chaindevice_bypass", "device"),
    PluginStep::new("plugin_preset", "device"),
    PluginStep::new("plugin_suspend", "device"),
    // `path` names a device in Renoise's browser ("Audio/Effects/Native/EQ 10"), never a file.
    PluginStep::new("doofer_device_add", "device").browser("path"),
    PluginStep::new("doofer_device_move", "device"),
    PluginStep::new("splitter_set", "device"),
    PluginStep::new("notepad_write", "device"),
    PluginStep::new("meta_destination", "device"),
    PluginStep::new("doofer_make", "device"),
    PluginStep::new("device_param", "parameter"),
    PluginStep::new("device_params", "parameter"),
    // Its points may be written; removing points or clearing or deleting the automation may not.
    PluginStep::new("automation_write", "parameter").refusing(&[("remove", &["*"]), ("clear", &["*"]), ("delete", &["*"])]),
    PluginStep::new("instrument_macro", "parameter"),
    PluginStep::new("instrument_macro_map", "parameter"),
    PluginStep::new("instrument_set", "parameter"),
    PluginStep::new("sample_set", "parameter"),
    PluginStep::new("sample_invert", "parameter"),
    PluginStep::new("keyzone_set", "parameter"),
    PluginStep::new("keyzone_overlap", "parameter"),
    PluginStep::new("keyzone_drumkit", "parameter"),
    PluginStep::new("keyzone_map", "parameter"),
    PluginStep::new("moddevice_param", "parameter"),
    PluginStep::new("chaindevice_param", "parameter"),
    PluginStep::new("phrase_set", "parameter"),
    PluginStep::new("phrase_map", "parameter"),
    PluginStep::new("phrase_playback", "parameter"),
    PluginStep::new("plugin_param", "parameter"),
    PluginStep::new("plugin_set", "parameter"),
    PluginStep::new("midi_set", "parameter"),
    PluginStep::new("doofer_macro", "parameter"),
    PluginStep::new("doofer_device_set", "parameter"),
    // Its lines may be written, never cleared.
    PluginStep::new("notes_write", "clip").refusing(&[("clear", &["*"])]),
    PluginStep::new("notes_transpose", "clip"),
    PluginStep::new("notes_humanize", "clip"),
    // Its lines may be written, never cleared.
    PluginStep::new("phrase_write", "clip").refusing(&[("clear", &["*"])]),
    PluginStep::new("track_column", "clip"),
    PluginStep::new("pattern_length", "clip"),
    PluginStep::new("slot_insert", "locators"),
    PluginStep::new("slot_pattern", "locators"),
    PluginStep::new("slot_section", "locators"),
    PluginStep::new("slot_mute", "locators"),
    PluginStep::new("slot_clone", "locators"),
    PluginStep::new("slot_move", "locators"),
    PluginStep::new("slot_unique", "locators"),
    // It may add, never clear.
    PluginStep::new("slot_variation", "locators").refusing(&[("clear", &["*"])]),
];

/// Every other make_changes step in Renoise, with why a plugin's plan can't use it.
pub static PLUGIN_REFUSED: &[(&str, &str)] = &[
    ("wait", "Pauses the plan while it holds the write lease."),
    ("run_lua", "Runs Lua, and only Renoise's own undo takes it back."),
    ("phrase_script", "Writes Lua code or loads a script file by path."),
    ("phrase_render", "Renders a script over the phrase's lines (a .deletes() kind)."),
    ("plugin_preset_data", "Loads a .vstpreset by absolute path into an instrument's plug-in."),
    ("device_preset_data", "Loads a preset file by path or writes raw presetData XML."),
    ("instrument_load", "Loads an instrument or sample file by path."),
    ("instrument_kit", "Builds an instrument from sample files by path."),
    ("device_chain_load", "Loads a device chain file by path (a .deletes() kind)."),
    ("phrase_load", "Loads a phrase file by path."),
    ("modset_load", "Loads a modulation set file by path."),
    ("plugin_load", "Replaces an instrument's plug-in by path or name (a .deletes() kind)."),
    ("track_delete", "Deletes a track."),
    ("device_delete", "Deletes a DSP device."),
    ("notes_clear", "Clears pattern lines."),
    ("slot_delete", "Deletes a sequence slot."),
    ("slots_drop", "Deletes sequence slots."),
    ("pattern_alias", "Can leave a pattern track empty (a .deletes() kind)."),
    ("lines_copy", "Overwrites the destination lines and empties the source on move (a .deletes() kind)."),
    ("instrument_delete", "Deletes an instrument and the notes that play it."),
    ("sample_delete", "Deletes a sample."),
    ("sample_slices", "Deletes slice markers, and the slices' samples with them."),
    ("modset_delete", "Deletes a modulation set."),
    ("moddevice_delete", "Deletes a modulation device."),
    ("moddevice_move", "Deletes the old modulation device and makes a copy."),
    ("chain_delete", "Deletes a sample FX chain."),
    ("chaindevice_delete", "Deletes a sample FX chain's device."),
    ("doofer_device_delete", "Deletes a device inside a Doofer or Splitter."),
    ("phrase_delete", "Deletes a phrase."),
    ("phrases_drop", "Deletes phrases."),
    ("phrase_clear", "Clears a phrase's lines."),
    ("loop_range", "Sets the transport's loop, a playback setting."),
    ("loop_pattern", "Turns pattern looping on or off, a playback setting."),
    ("metronome_switch", "Turns the metronome and precount on or off, a playback setting."),
    ("song_metronome", "Changes the metronome, a playback setting."),
    ("edit_settings", "Changes Renoise's editing settings and preferences."),
    ("track_collapse", "Collapses or expands a track in the pattern editor, a view change."),
    ("track_columns", "Shows or hides a track's columns, a view change."),
    ("scratch_keep", "Keeps one of Kumi's scratch renders as a track: a judged round's work, which a plugin's plan doesn't make."),
    ("play", "Plays Renoise."),
    ("stop", "Stops Renoise's playback."),
    ("audition_note", "Plays a note."),
    ("select", "Changes Renoise's selection, a view change."),
    ("save_song", "Saves the song's file."),
    ("save_instrument", "Saves an instrument file."),
    ("plugin_editor", "Opens or closes a plug-in's editor window."),
    ("save_device_preset", "Saves a device preset file."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::CHANGES;
    use kumi_daw::plugin::check;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// Every step make_changes offers: Renoise's change kinds, its actions, and `wait`.
    fn plan_steps() -> BTreeSet<&'static str> {
        CHANGES.kinds.iter().map(|kind| kind.tool).chain(CHANGES.actions.iter().map(|action| action.tool)).chain(["wait"]).collect()
    }

    #[test]
    fn plugin_steps_and_refusals_partition_the_plan_steps() {
        let allowed: Vec<&str> = PLUGIN_STEPS.iter().map(|step| step.tool).collect();
        let refused: Vec<&str> = PLUGIN_REFUSED.iter().map(|(tool, _)| *tool).collect();
        let mut listed = BTreeSet::new();
        for tool in allowed.iter().chain(&refused) {
            assert!(listed.insert(*tool), "{tool} is listed twice");
        }
        assert_eq!(listed, plan_steps());
    }

    #[test]
    fn plugin_steps_keep_their_registry_family() {
        for step in PLUGIN_STEPS {
            let kind = CHANGES.kinds.iter().find(|kind| kind.tool == step.tool).unwrap();
            assert_eq!(serde_json::to_value(kind.family).unwrap(), json!(step.family), "{}", step.tool);
        }
    }

    #[test]
    fn plugin_steps_never_delete() {
        for step in PLUGIN_STEPS {
            let kind = CHANGES.kinds.iter().find(|kind| kind.tool == step.tool).unwrap();
            // sample_load deletes only when it replaces a sample's audio, which Kumi keeps and undo_change puts back.
            assert!(!kind.deletes || step.tool == "sample_load", "{} deletes", step.tool);
        }
    }

    #[test]
    fn plugin_steps_refuse_what_c2_forbids() {
        for (tool, input) in [
            ("plugin_preset_data", json!({"ref": "i:0", "path": "/Users/me/Presets/lead.vstpreset"})),
            ("device_preset_data", json!({"ref": "d:1:2", "presetData": "<?xml version=\"1.0\"?><FilterDevicePreset/>"})),
            ("device_preset_data", json!({"ref": "d:1:2", "path": "/Users/me/Presets/verb.vstpreset"})),
            ("wait", json!({"seconds": 4})),
            ("run_lua", json!({"code": "song:delete_track_at(1)"})),
            ("phrase_script", json!({"ref": "p:0:1", "code": "return pattern{}"})),
            ("instrument_load", json!({"path": "library:Instruments/Piano.xrni"})),
            ("track_delete", json!({"ref": "t:1"})),
        ] {
            assert!(check(PLUGIN_STEPS, tool, &input).is_err(), "{tool}");
        }
        assert_eq!(check(PLUGIN_STEPS, "track_volume", &json!({"ref": "t:1", "volumeDb": -6})).unwrap().family, "mixer");
        let source = json!({"ref": "i:0", "path": {"sound_id": "s-r1-1", "map_revision": "r1"}});
        assert!(check(PLUGIN_STEPS, "sample_load", &source).is_ok());
        assert!(check(PLUGIN_STEPS, "sample_load", &json!({"ref": "i:0", "path": "/Users/me/kick.wav"})).is_err());
    }
}
