//! The fake REAPER's side for M4's track, routing, project and action changes (r-tracks): the audio and MIDI devices,
//! track groups, project information, notes and grid, config variables, the action list, track templates and saving.
//! Its own state sits beside the fake's (`reset` clears both); tracks are found through the fake's handles.

use super::*;
use std::ffi::c_uint;

pub struct Extra {
    pub audio_inputs: Vec<String>,
    pub audio_outputs: Vec<String>,
    /// MIDI devices present, by REAPER's device number.
    pub midi_inputs: Vec<(i32, String)>,
    pub midi_outputs: Vec<(i32, String)>,
    /// Group membership bits by (track GUID, role, high).
    pub groups: HashMap<(String, String, bool), u32>,
    pub project_info: HashMap<String, f64>,
    pub project_text: HashMap<String, String>,
    pub notes: String,
    pub grid: (f64, i32, f64),
    pub config: HashMap<String, String>,
    /// The main section's actions: (command id, name).
    pub actions: Vec<(i32, String)>,
    /// REAPER marked a change of its own inside the open block (an action that makes an undo point): a block ended with
    /// flags 0 makes one.
    pub marked: bool,
    /// Track chunks that read the same while the track is the same (a template's undo compares them), with no token to
    /// put back by: for the tests of what compares chunks.
    pub stable_chunks: bool,
    /// A file system whose times are coarser than two saves (Windows' can be): a save over a file keeps its time.
    pub coarse_file_times: bool,
    /// Files saved through `Main_SaveProjectEx`: (path, options).
    pub saved_as: Vec<(String, i32)>,
    pub made_from_templates: u32,
    /// (m6-library) A REAPER without JS: MIDI Choke (its Effects folder lacks it).
    pub no_choke: bool,
    /// (m6-library) A sampler that takes no file (DONE commits nothing).
    pub sampler_refuses_files: bool,
    /// (m6-library) Note names a track was given (`SetTrackMIDINoteNameEx`): (track GUID, pitch, channel, name).
    pub note_names: Vec<(String, i32, i32, String)>,
    /// The track REAPER takes as last touched once an action made it so (40914, by GUID); else the first selected one.
    pub last_touched: Option<String>,
    /// (m7) API functions extensions registered (SWS's `CF_GetSWSVersion`…), for `APIExists`.
    pub extension_apis: Vec<String>,
}

impl Default for Extra {
    fn default() -> Self {
        let config = [
            ("panmode", "3"),
            ("panlaw", "1"),
            ("itemtimelock", "1"),
            ("tempoenvtimelock", "1"),
            ("defsavepath", "/lab/Projects"),
            ("projmetroen", "0"),
            // (m7) REAPER 7.82's factory defaults for new items and tracks (KP1, lab 2026-10-21).
            ("deffadelen", "0.01000000000000"),
            ("splitautoxfade", "0"),
            ("loopnewitems", "14"),
            ("deftrackvol", "1.00000000000000"),
        ];
        let project_info = [("RULER_LANE_COUNT", 2.0), ("RULER_LANE_DEFAULT:0", 1.0), ("RULER_LANE_DEFAULT:1", 2.0)];
        let actions = [
            (1007, "Transport: Play"),
            (1016, "Transport: Stop"),
            (1068, "Transport: Toggle repeat"),
            (40001, "Track: Insert new track"),
            (40004, "File: Quit REAPER"),
            (40016, "Options: Preferences..."),
            (40022, "File: Save project as..."),
            (40023, "File: New project"),
            (40026, "File: Save project"),
            (40042, "Transport: Go to start of project"),
            (40280, "Track: Mute/unmute tracks"),
            (40296, "Track: Select all tracks"),
            (40297, "Track: Unselect (clear selection of) all tracks"),
            (40392, "Track: Save tracks as track template..."),
            (40860, "Close current project tab"),
            (40886, "File: Close all projects"),
            (41312, "Track: Lock track controls"),
            (41313, "Track: Unlock track controls"),
            (41824, "File: Render project, using the most recent render settings"),
            (41895, "File: Save new version of project (automatically increment project name)"),
            (42230, "File: Render project, using the most recent render settings, auto-close render dialog"),
            (40029, "Edit: Undo"),
            (40030, "Edit: Redo"),
            (54000, "Script: kumi.lua"),
            (54001, "Script: tidy up.lua"),
        ];
        Extra {
            audio_inputs: vec!["Input 1".into(), "Input 2".into()],
            audio_outputs: vec!["Output 1".into(), "Output 2".into()],
            midi_inputs: vec![(0, "Fake Keys".into()), (62, "Virtual MIDI Keyboard".into())],
            midi_outputs: vec![(0, "Fake Synth".into())],
            groups: HashMap::new(),
            project_info: project_info.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            project_text: HashMap::new(),
            notes: String::new(),
            grid: (0.25, 0, 0.0),
            config: config.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            actions: actions.into_iter().map(|(id, name)| (id, name.to_string())).collect(),
            marked: false,
            stable_chunks: false,
            coarse_file_times: false,
            saved_as: vec![],
            made_from_templates: 0,
            no_choke: false,
            sampler_refuses_files: false,
            note_names: vec![],
            last_touched: None,
            extension_apis: vec![],
        }
    }
}

thread_local! {
    pub static EXTRA: RefCell<Extra> = RefCell::new(Extra::default());
}

/// The test's view of this state.
pub fn extra<T>(f: impl FnOnce(&mut Extra) -> T) -> T {
    EXTRA.with(|e| f(&mut e.borrow_mut()))
}

pub fn reset() {
    EXTRA.with(|e| *e.borrow_mut() = Extra::default());
}

/// REAPER marked a change of its own (an action that makes an undo point).
pub fn mark_changed() {
    extra(|e| e.marked = true);
    with(|s| s.undo_dirty = true);
}

/// Whether REAPER marked a change since the last block ended, and forgets it.
pub fn take_marked() -> bool {
    extra(|e| std::mem::take(&mut e.marked))
}

/// A track's chunk as it reads with `stable_chunks`: its state in the shape of a real one, items with their selection.
pub fn stable_chunk(t: &FakeTrack, fx_chain: &str) -> Option<String> {
    if !extra(|e| e.stable_chunks) {
        return None;
    }
    let lock = if t.extra.get("LOCK") == Some(&1.0) { "LOCK 1\n" } else { "" };
    let mut items = String::new();
    for item in &t.items {
        items += &format!(
            "<ITEM\nPOSITION {}\nLENGTH {}\nMUTE {}\nSEL {}\nIGUID {}\n",
            item.position,
            item.length,
            u8::from(item.mute),
            u8::from(item.selected),
            item.guid
        );
        for take in &item.takes {
            items += &format!("TAKE\nGUID {}\nNOTES {}\n", take.guid, take.notes.len());
        }
        items += ">\n";
    }
    Some(format!(
        "<TRACK {}\nNAME \"{}\"\nVOLPAN {} {}\nMUTESOLO {} {}\nSEL {}\n{lock}{fx_chain}{items}>\n",
        t.guid,
        t.name,
        t.volume,
        t.pan,
        u8::from(t.mute),
        t.solo,
        u8::from(t.selected)
    ))
}

/// The action's name, for the fake's `kbd_getTextFromCmd`.
pub fn action_name(command: c_int) -> Option<String> {
    extra(|e| e.actions.iter().find(|(id, _)| *id == command).map(|(_, name)| name.clone()))
}

fn guid_of(track: *mut MediaTrack) -> Option<String> {
    with(|s| track_of(s, track as u64).map(|t| t.guid.clone()))
}

// ----- devices -----

unsafe extern "C" fn get_num_audio_inputs() -> c_int {
    extra(|e| e.audio_inputs.len() as c_int)
}
unsafe extern "C" fn get_num_audio_outputs() -> c_int {
    extra(|e| e.audio_outputs.len() as c_int)
}
unsafe extern "C" fn get_input_channel_name(index: c_int) -> *const c_char {
    match extra(|e| e.audio_inputs.get(index.max(0) as usize).cloned()) {
        Some(name) => keep(name),
        None => ptr::null(),
    }
}
unsafe extern "C" fn get_output_channel_name(index: c_int) -> *const c_char {
    match extra(|e| e.audio_outputs.get(index.max(0) as usize).cloned()) {
        Some(name) => keep(name),
        None => ptr::null(),
    }
}
unsafe extern "C" fn get_num_midi_inputs() -> c_int {
    130
}
unsafe extern "C" fn get_num_midi_outputs() -> c_int {
    128
}
unsafe extern "C" fn get_midi_input_name(dev: c_int, buf: *mut c_char, size: c_int) -> bool {
    match extra(|e| e.midi_inputs.iter().find(|(d, _)| *d == dev).map(|(_, n)| n.clone())) {
        Some(name) => {
            write(buf, size, &name);
            true
        }
        None => {
            write(buf, size, "");
            false
        }
    }
}
unsafe extern "C" fn get_midi_output_name(dev: c_int, buf: *mut c_char, size: c_int) -> bool {
    match extra(|e| e.midi_outputs.iter().find(|(d, _)| *d == dev).map(|(_, n)| n.clone())) {
        Some(name) => {
            write(buf, size, &name);
            true
        }
        None => {
            write(buf, size, "");
            false
        }
    }
}

// ----- groups -----

fn membership(track: *mut MediaTrack, role: *const c_char, high: bool, mask: c_uint, value: c_uint) -> c_uint {
    // (one REAPER call, counted as a read's cost like the others)
    counted(|_| ());
    let role = name_of(role);
    let Some(guid) = guid_of(track) else { return 0 };
    let before = extra(|e| {
        let bits = e.groups.entry((guid, role, high)).or_insert(0);
        let before = *bits;
        *bits = (*bits & !mask) | (value & mask);
        before
    });
    if mask != 0 {
        with(|s| s.undo_dirty = true);
    }
    before
}
unsafe extern "C" fn get_set_track_group_membership(track: *mut MediaTrack, role: *const c_char, mask: c_uint, value: c_uint) -> c_uint {
    membership(track, role, false, mask, value)
}
unsafe extern "C" fn get_set_track_group_membership_high(
    track: *mut MediaTrack,
    role: *const c_char,
    mask: c_uint,
    value: c_uint,
) -> c_uint {
    membership(track, role, true, mask, value)
}

// ----- the project -----

unsafe extern "C" fn get_set_project_info(_proj: *mut ReaProject, key: *const c_char, value: f64, set: bool) -> f64 {
    let key = name_of(key);
    if key.starts_with("PROJECT_TIMEBASE") {
        return extra(|e| e.config.get("itemtimelock").and_then(|v| v.parse().ok()).unwrap_or(1.0));
    }
    // A lane's number by its GUID: a lane's GUID is `{LANE-n}` from its first number unless a test moved them
    // (`RULER_LANE_GUID:n` set), as when the producer reorders lanes.
    if let Some(guid) = key.strip_prefix("RULER_LANE_FROM_GUID:") {
        let count = extra(|e| e.project_info.get("RULER_LANE_COUNT").copied().unwrap_or(0.0)) as i64;
        let lane = (0..count).find(|n| lane_guid(*n) == guid);
        return lane.map_or(-1.0, |n| n as f64);
    }
    let out = extra(|e| {
        if set {
            e.project_info.insert(key.clone(), value);
        }
        e.project_info.get(&key).copied().unwrap_or(0.0)
    });
    if set {
        with(|s| s.undo_dirty = true);
    }
    out
}
unsafe extern "C" fn get_set_project_info_string(_proj: *mut ReaProject, key: *const c_char, buf: *mut c_char, set: bool) -> bool {
    let key = name_of(key);
    if set {
        let value = name_of(buf);
        extra(|e| e.project_text.insert(key, value));
        with(|s| s.undo_dirty = true);
        return true;
    }
    let text =
        extra(|e| e.project_text.get(&key).cloned().or_else(|| key.strip_prefix("RULER_LANE_GUID:").map(|n| format!("{{LANE-{n}}}"))));
    write(buf, 4096, text.as_deref().unwrap_or(""));
    true
}
fn lane_guid(lane: i64) -> String {
    extra(|e| e.project_text.get(&format!("RULER_LANE_GUID:{lane}")).cloned()).unwrap_or_else(|| format!("{{LANE-{lane}}}"))
}
unsafe extern "C" fn get_set_project_notes(_proj: *mut ReaProject, set: bool, buf: *mut c_char, size: c_int) {
    if set {
        let notes = name_of(buf);
        extra(|e| e.notes = notes);
        return;
    }
    let notes = extra(|e| e.notes.clone());
    write(buf, size, &notes);
}
unsafe extern "C" fn get_set_project_grid(
    _proj: *mut ReaProject,
    set: bool,
    division: *mut f64,
    mode: *mut c_int,
    amount: *mut f64,
) -> c_int {
    extra(|e| {
        if set {
            unsafe {
                if !division.is_null() {
                    e.grid.0 = *division;
                }
                if !mode.is_null() {
                    e.grid.1 = *mode;
                }
                if !amount.is_null() {
                    e.grid.2 = *amount;
                }
            }
        } else {
            unsafe {
                if !division.is_null() {
                    *division = e.grid.0;
                }
                if !mode.is_null() {
                    *mode = e.grid.1;
                }
                if !amount.is_null() {
                    *amount = e.grid.2;
                }
            }
        }
        0
    })
}
unsafe extern "C" fn get_config_var_string(name: *const c_char, buf: *mut c_char, size: c_int) -> bool {
    let name = name_of(name);
    match extra(|e| e.config.get(&name).cloned()) {
        Some(value) => {
            write(buf, size, &value);
            true
        }
        None => false,
    }
}
/// REAPER's own functions (the fake's table) and those an extension registered.
unsafe extern "C" fn api_exists(name: *const c_char) -> bool {
    let name = name_of(name);
    let c_name = CString::new(name.as_str()).unwrap_or_default();
    !super::lookup(&c_name).is_null() || extra(|e| e.extension_apis.contains(&name))
}
unsafe extern "C" fn set_config_var_string(name: *const c_char, value: *const c_char, _persist: c_int) -> c_int {
    let (name, value) = (name_of(name), name_of(value));
    extra(|e| match e.config.get_mut(&name) {
        Some(slot) => {
            *slot = value;
            2
        }
        None => 0,
    })
}

// ----- actions -----

const MAIN_SECTION: u64 = 0x5EC0;

unsafe extern "C" fn section_from_unique_id(id: c_int) -> *mut api::KbdSectionInfo {
    if id == 0 {
        MAIN_SECTION as *mut api::KbdSectionInfo
    } else {
        ptr::null_mut()
    }
}
unsafe extern "C" fn kbd_enumerate_actions(section: *mut api::KbdSectionInfo, index: c_int, name: *mut *const c_char) -> c_int {
    if section as u64 != MAIN_SECTION {
        return 0;
    }
    match extra(|e| e.actions.get(index.max(0) as usize).cloned()) {
        Some((id, text)) => {
            if !name.is_null() {
                unsafe { *name = keep(text) };
            }
            id
        }
        None => 0,
    }
}

// ----- templates and saving -----

/// A template's tracks, as the fake writes them: a block per track with its NAME and ISBUS (the folder depth second).
unsafe extern "C" fn main_open_project(path: *const c_char) {
    let path = name_of(path);
    let path = path.strip_prefix("noprompt:").unwrap_or(&path).to_string();
    if !path.to_ascii_lowercase().ends_with(".rtracktemplate") {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let mut made = Vec::new();
    for block in text.split("<TRACK").skip(1) {
        let name = block.lines().find_map(|l| l.trim().strip_prefix("NAME ")).unwrap_or("").trim_matches('"').to_string();
        let depth = block
            .lines()
            .find_map(|l| l.trim().strip_prefix("ISBUS "))
            .and_then(|rest| rest.split_whitespace().nth(1)?.parse::<i32>().ok());
        // A plug-in in it (`<VST "name"`), with one parameter at 0.5.
        let fx: Vec<String> = block
            .lines()
            .filter_map(|l| l.trim().strip_prefix("<VST \"").and_then(|rest| rest.split('"').next()).map(str::to_string))
            .collect();
        made.push((name, depth.unwrap_or(0), fx));
    }
    let first = extra(|e| {
        let first = e.made_from_templates;
        e.made_from_templates += made.len() as u32;
        first
    });
    // REAPER puts the template's tracks after the last touched track, the end only when that's the last (lab 7.82).
    let touched = extra(|e| e.last_touched.clone());
    with(|s| {
        let mut at = touched
            .and_then(|guid| s.tracks.iter().position(|t| t.guid == guid))
            .or_else(|| s.tracks.iter().position(|t| t.selected))
            .map_or(s.tracks.len(), |at| at + 1);
        for t in s.tracks.iter_mut() {
            t.selected = false;
        }
        s.master.selected = false;
        for (n, (name, depth, fx)) in made.into_iter().enumerate() {
            let guid = format!("{{TPL-{}}}", first + n as u32 + 1);
            let mut track = FakeTrack::new(&guid, &name);
            track.folder = depth;
            track.selected = true;
            track.fx = fx
                .iter()
                .enumerate()
                .map(|(at, fx_name)| {
                    let mut fx = FakeFx::new(&format!("{{TPL-FX-{}-{at}}}", first + n as u32 + 1), fx_name);
                    fx.params = vec![FakeParam::new("Gain", 0.5, "0.0 dB")];
                    fx
                })
                .collect();
            s.tracks.insert(at, track);
            at += 1;
        }
        s.undo_dirty = true;
    });
}
unsafe extern "C" fn main_save_project_ex(_proj: *mut ReaProject, path: *const c_char, options: c_int) {
    let path = name_of(path);
    let text = if options & 1 != 0 {
        with(|s| {
            s.tracks
                .iter()
                .filter(|t| t.selected)
                .map(|t| format!("<TRACK\n  NAME \"{}\"\n  ISBUS 0 {}\n>\n", t.name, t.folder))
                .collect::<String>()
        })
    } else {
        "<REAPER_PROJECT 0.1 \"7.82/fake\"\n>\n".to_string()
    };
    let kept_time = std::fs::metadata(&path).and_then(|m| m.modified()).ok().filter(|_| extra(|e| e.coarse_file_times));
    let _ = std::fs::write(&path, text);
    if let Some(time) = kept_time {
        let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(time));
    }
    extra(|e| e.saved_as.push((path.clone(), options)));
    if options & 1 == 0 && options & 8 != 0 {
        with(|s| {
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            s.project = Some((path, name));
            s.dirty = false;
        });
    }
}

// ----- item selection -----

unsafe extern "C" fn set_media_item_selected(item: *mut MediaItem, selected: bool) {
    with(|s| {
        let (kind, a, b, _) = unpack(item as u64);
        if kind == ITEM {
            if let Some(i) = s.tracks.get_mut(a as usize).and_then(|t| t.items.get_mut(b as usize)) {
                i.selected = selected;
            }
        }
    });
}
unsafe extern "C" fn select_all_media_items(_proj: *mut ReaProject, selected: bool) {
    with(|s| {
        for t in s.tracks.iter_mut() {
            for i in t.items.iter_mut() {
                i.selected = selected;
            }
        }
    });
}

/// What this module adds to the fake's table.
pub fn lookup(name: &CStr) -> *mut c_void {
    macro_rules! table {
        ($($name:literal => $f:expr),* $(,)?) => {
            match name.to_bytes() {
                $($name => $f as *mut c_void,)*
                _ => ptr::null_mut(),
            }
        };
    }
    table! {
        b"GetNumAudioInputs" => get_num_audio_inputs as unsafe extern "C" fn() -> c_int,
        b"GetNumAudioOutputs" => get_num_audio_outputs as unsafe extern "C" fn() -> c_int,
        b"GetInputChannelName" => get_input_channel_name as unsafe extern "C" fn(c_int) -> *const c_char,
        b"GetOutputChannelName" => get_output_channel_name as unsafe extern "C" fn(c_int) -> *const c_char,
        b"GetNumMIDIInputs" => get_num_midi_inputs as unsafe extern "C" fn() -> c_int,
        b"GetNumMIDIOutputs" => get_num_midi_outputs as unsafe extern "C" fn() -> c_int,
        b"GetMIDIInputName" => get_midi_input_name as unsafe extern "C" fn(c_int, *mut c_char, c_int) -> bool,
        b"GetMIDIOutputName" => get_midi_output_name as unsafe extern "C" fn(c_int, *mut c_char, c_int) -> bool,
        b"GetSetTrackGroupMembership" => get_set_track_group_membership as unsafe extern "C" fn(*mut MediaTrack, *const c_char, c_uint, c_uint) -> c_uint,
        b"GetSetTrackGroupMembershipHigh" => get_set_track_group_membership_high as unsafe extern "C" fn(*mut MediaTrack, *const c_char, c_uint, c_uint) -> c_uint,
        b"GetSetProjectInfo" => get_set_project_info as unsafe extern "C" fn(*mut ReaProject, *const c_char, f64, bool) -> f64,
        b"GetSetProjectInfo_String" => get_set_project_info_string as unsafe extern "C" fn(*mut ReaProject, *const c_char, *mut c_char, bool) -> bool,
        b"GetSetProjectNotes" => get_set_project_notes as unsafe extern "C" fn(*mut ReaProject, bool, *mut c_char, c_int),
        b"GetSetProjectGrid" => get_set_project_grid as unsafe extern "C" fn(*mut ReaProject, bool, *mut f64, *mut c_int, *mut f64) -> c_int,
        b"get_config_var_string" => get_config_var_string as unsafe extern "C" fn(*const c_char, *mut c_char, c_int) -> bool,
        b"APIExists" => api_exists as unsafe extern "C" fn(*const c_char) -> bool,
        b"set_config_var_string" => set_config_var_string as unsafe extern "C" fn(*const c_char, *const c_char, c_int) -> c_int,
        b"SectionFromUniqueID" => section_from_unique_id as unsafe extern "C" fn(c_int) -> *mut api::KbdSectionInfo,
        b"kbd_enumerateActions" => kbd_enumerate_actions as unsafe extern "C" fn(*mut api::KbdSectionInfo, c_int, *mut *const c_char) -> c_int,
        b"Main_openProject" => main_open_project as unsafe extern "C" fn(*const c_char),
        b"Main_SaveProjectEx" => main_save_project_ex as unsafe extern "C" fn(*mut ReaProject, *const c_char, c_int),
        b"SetMediaItemSelected" => set_media_item_selected as unsafe extern "C" fn(*mut MediaItem, bool),
        b"SelectAllMediaItems" => select_all_media_items as unsafe extern "C" fn(*mut ReaProject, bool),
        b"SetTrackMIDINoteNameEx" => set_track_midi_note_name_ex as unsafe extern "C" fn(*mut ReaProject, *mut MediaTrack, c_int, c_int, *const c_char) -> bool,
    }
}

unsafe extern "C" fn set_track_midi_note_name_ex(
    _: *mut ReaProject,
    track: *mut MediaTrack,
    pitch: c_int,
    channel: c_int,
    name: *const c_char,
) -> bool {
    let name = name_of(name);
    let Some(guid) = with(|s| track_of(s, track as u64).map(|t| t.guid.clone())) else { return false };
    extra(|e| {
        e.note_names.retain(|(g, p, c, _)| !(g == &guid && *p == pitch && *c == channel));
        e.note_names.push((guid, pitch, channel, name));
    });
    true
}
