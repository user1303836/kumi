//! How the app shows Live's own tools (each step's words, what NOW says while it runs and its kind; Kumi's own tools
//! are shown alike in every DAW, core/presentation.rs), and how it reads Live's focus and devices.
use crate::core::presentation::{ToolActivity, ToolPresentation};
use crate::core::view::{
    BodyKind, DawFocus, DeviceForm, DeviceRole, DeviceRow, DeviceSignal, FocusItem, FocusSection, FocusView, ItemKind, LiveDetail,
    LiveView, SectionKind, ViewKind, ViewPresentation,
};

/// Live's tools as the app shows them.
pub const PRESENTATION: ToolPresentation =
    ToolPresentation { label, doing, activity, plan: |tool| tool == "make_changes", acts, read_prefix: Some("live_") };

fn label(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "server_status" => "checked the Ableton bridge",
        "live_status" => "checked Live",
        "live_discover" => "looked at your Set",
        "live_snapshot" => "read your whole Set",
        "live_browser_search" => "searched the Browser",
        "live_note_read" => "read notes",
        "set_tempo" => "changed the tempo",
        "set_mixer" => "changed the mixer",
        "rename" => "renamed",
        "add_tracks_and_scenes" => "added tracks or scenes",
        "write_midi_clip" => "wrote a MIDI clip",
        "load_device" => "loaded a device",
        "set_device_parameter" => "moved a device control",
        "set_locators" => "set locators",
        "set_track_color" => "changed a track colour",
        "undo_change" => "undid a change",
        "run_python" => "ran Python in Live",
        "make_changes" => "made changes",
        "arrange" => "arranged",
        "audition" => "listened to it",
        "judge" => "judged it by ear",
        "tune" => "tuned it",
        "groove" => "compared the feel",
        "sound" => "measured the sounds",
        "form" => "laid out the form",
        "play" => "played or stopped",
        "fire_scene" => "launched a scene",
        "launch_clip" => "launched a clip",
        "record" => "recorded",
        "jump_to_locator" => "moved the playhead",
        "select" => "showed you in Live",
        "show" => "changed the view",
        "set_transport" => "changed the transport",
        "set_routing" => "changed routing",
        "transform_midi" => "transformed notes",
        "set_automation" => "drew automation",
        "change_structure" => "changed tracks",
        "set_song" => "changed song settings",
        "live_manual" => "read Live's manual",
        "load_sample" => "loaded a sample",
        "load_sample_to_pad" => "loaded a pad",
        "edit_rack" => "edited a rack",
        "set_chain_mixer" => "changed a rack chain",
        "set_mixer_options" => "changed mixer options",
        "set_clip" => "changed a clip",
        "set_audio_clip" => "changed an audio clip",
        "edit_clip" => "edited a clip",
        "duplicate_clip" => "copied a clip",
        "move_clip" => "moved a clip",
        "add_arrangement_clip" => "added an Arrangement clip",
        "change_notes" => "changed notes",
        "delete_notes" => "deleted notes",
        "edit_notes" => "edited notes",
        "set_scene" => "changed a scene",
        "capture_scene" => "captured a scene",
        "switch_device" => "switched a device",
        "move_device" => "moved a device",
        "move_device_to" => "moved a device",
        "delete_device" => "deleted a device",
        "set_chain" => "changed a chain",
        "set_scale" => "set the scale",
        "set_groove" => "changed the groove",
        "replace_sample" => "swapped a sample",
        "import_audio" => "imported audio",
        "set_warp_markers" => "moved warp markers",
        "capture_midi" => "captured MIDI",
        "set_device_details" => "changed device settings",
        "use_looper" => "used the Looper",
        "set_sidechain" => "set a sidechain",
        "live_song_state" => "read the song's settings",
        "live_performance_read" => "checked Live's load",
        "live_key_estimate" => "estimated the key",
        "live_take_lane_read" => "read take lanes",
        "live_warp_marker_read" => "read warp markers",
        "live_arrangement_automation_read" => "read automation",
        "live_browser_roots" => "looked in the Browser",
        "live_browser_inspect" => "looked in the Browser",
        "watch_me" => "watched you work",
        "make_device" => "made a device",
        _ => return None,
    })
}

fn doing(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "live_status" => "checking Live",
        "live_discover" => "looking at your Set",
        "live_snapshot" => "reading your whole Set",
        "live_browser_search" => "searching the Browser",
        "live_note_read" => "reading notes",
        "make_changes" => "making changes",
        "arrange" => "arranging",
        "audition" => "listening to it",
        "judge" => "judging it by ear",
        "tune" => "tuning it",
        "groove" => "comparing the feel",
        "sound" => "measuring the sounds",
        "form" => "laying out the form",
        "play" => "playing",
        "record" => "recording",
        "select" => "showing you",
        "live_manual" => "reading Live's manual",
        "undo_change" => "undoing a change",
        "run_python" => "running Python in Live",
        "watch_me" => "comparing your Set",
        "live_key_estimate" => "estimating the key",
        "make_device" => "making a device",
        _ => return None,
    })
}

fn activity(tool: &str) -> Option<ToolActivity> {
    use ToolActivity::*;
    Some(match tool {
        "live_browser_search" => Search,
        "live_browser_roots" => Search,
        "live_browser_inspect" => Search,
        "live_manual" => Read,
        "server_status" => Look,
        "live_status" => Look,
        "live_discover" => Look,
        "live_snapshot" => Look,
        "live_note_read" => Look,
        "live_song_state" => Look,
        "live_performance_read" => Look,
        "live_key_estimate" => Look,
        "live_take_lane_read" => Look,
        "live_warp_marker_read" => Look,
        "live_arrangement_automation_read" => Look,
        "watch_me" => Look,
        "select" => Look,
        "show" => Look,
        "groove" => Look,
        "make_device" => Build,
        "arrange" => Build,
        "audition" => Listen,
        "judge" => Listen,
        "tune" => Listen,
        "sound" => Listen,
        "form" => Listen,
        "play" => Play,
        "fire_scene" => Play,
        "launch_clip" => Play,
        "jump_to_locator" => Play,
        "record" => Record,
        "capture_midi" => Record,
        "run_python" => Code,
        _ => return None,
    })
}

/// What NOW shows Live doing while the tool runs: a plan's or an arrangement's actions, and Live's own actions.
fn acts(tool: &str) -> bool {
    matches!(tool, "make_changes" | "arrange" | "play" | "fire_scene" | "launch_clip" | "record" | "jump_to_locator" | "select" | "show")
}

/// How the app reads Live's focus and devices.
pub const VIEW: ViewPresentation = ViewPresentation { fill_focus, device_role, heading, timeline_notes: false, open_containers: false };

/// Live's word over each body FOCUS draws.
fn heading(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::Tree => "Device",
        BodyKind::Notes => "Clip",
        BodyKind::Grid => "Session",
        BodyKind::Timeline => "Arrangement",
        _ => "",
    }
}

/// Fills a focus's neutral fields from Live's wire form, one for one: the Session and Arrangement views, the Clip and
/// Device views, the highlighted slot's clip, the selected scene. A neutral field already there must agree.
pub fn fill_focus(focus: &mut DawFocus) {
    let view = |kind, label: &str| FocusView { kind, label: label.into() };
    let main = focus.view.map(|main| match main {
        LiveView::Session => view(ViewKind::Launcher, "Session"),
        LiveView::Arrangement => view(ViewKind::Timeline, "Arrangement"),
    });
    let editor = focus.detail.map(|detail| match detail {
        LiveDetail::Clip => view(ViewKind::NoteEditor, "Clip view"),
        LiveDetail::Device => view(ViewKind::DeviceChain, "Device view"),
    });
    let item = (focus.slot_ref.is_some() || focus.clip.is_some()).then(|| FocusItem {
        kind: ItemKind::Clip,
        item_ref: focus.slot_ref.clone(),
        name: focus.clip.clone(),
    });
    let section = (focus.scene_index.is_some() || focus.scene.is_some()).then(|| FocusSection {
        kind: SectionKind::Scene,
        index: focus.scene_index,
        name: focus.scene.clone(),
    });
    fill(&mut focus.main_view, main);
    fill(&mut focus.editor_view, editor);
    fill(&mut focus.item, item);
    fill(&mut focus.section, section);
}

/// A neutral field from Live's: kept when Live's says nothing, and the same as Live's when both say.
fn fill<T: PartialEq + std::fmt::Debug>(field: &mut Option<T>, live: Option<T>) {
    match (field.as_ref(), live) {
        (_, None) => {}
        (None, live) => *field = live,
        (Some(kept), Some(live)) => debug_assert_eq!(*kept, live, "a focus's neutral field disagrees with Live's"),
    }
}

/// What a Live device is, from its class, whether it holds chains or pads, and Live's device type when sent.
pub fn device_role(row: &DeviceRow) -> DeviceRole {
    let cls = row.class_name.unwrap_or("");
    let typed = match row.device_type {
        Some("instrument") => DeviceSignal::Instrument,
        Some("audio_effect") => DeviceSignal::Audio,
        Some("midi_effect") => DeviceSignal::Midi,
        _ => DeviceSignal::Unknown,
    };
    let role = |signal, form| DeviceRole { signal, form };
    if row.can_have_drum_pads == Some(true) || cls == "DrumGroupDevice" {
        return role(DeviceSignal::Instrument, DeviceForm::DrumContainer);
    }
    if row.can_have_chains == Some(true) || cls.ends_with("GroupDevice") {
        let signal = if cls.starts_with("MidiEffect") || typed == DeviceSignal::Midi {
            DeviceSignal::Midi
        } else if cls.starts_with("Instrument") || typed == DeviceSignal::Instrument {
            DeviceSignal::Instrument
        } else {
            DeviceSignal::Audio
        };
        return role(signal, DeviceForm::Container);
    }
    if cls.starts_with("MxDevice") {
        let signal = if cls.contains("MidiEffect") {
            DeviceSignal::Midi
        } else if cls.contains("Instrument") {
            DeviceSignal::Instrument
        } else {
            DeviceSignal::Audio
        };
        return role(signal, DeviceForm::Script);
    }
    if cls.contains("Plugin") {
        return role(typed, DeviceForm::Plugin);
    }
    if cls == "DrumCell" || cls == "DrumSampler" {
        return role(DeviceSignal::Instrument, DeviceForm::DrumInstrument);
    }
    role(typed, DeviceForm::Native)
}
