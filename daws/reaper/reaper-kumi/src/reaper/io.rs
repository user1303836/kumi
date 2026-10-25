//! The REAPER calls M4's track, routing, project and action changes make (r-tracks): the audio device's inputs and outputs,
//! MIDI devices, track groups, project information, notes and grid, config variables, the action list, track templates
//! and saving. Each is REAPER's own call, one for one; a function this REAPER lacks answers None or false.

use std::ffi::{c_char, c_int};
use std::ptr;

use super::ctext;
use super::{Item, Project, Reaper, Track};
use crate::api;

/// The most of a project string REAPER is asked for (notes, names).
const STRING_LIMIT: usize = 1 << 20;

impl Reaper<'_> {
    // ----- the audio device and MIDI devices -----

    /// How many inputs the audio device has (0 when none is open).
    pub fn audio_inputs(&self) -> i32 {
        self.api.GetNumAudioInputs().map_or(0, |f| unsafe { f() }).max(0)
    }

    /// How many outputs the audio device has.
    pub fn audio_outputs(&self) -> i32 {
        self.api.GetNumAudioOutputs().map_or(0, |f| unsafe { f() }).max(0)
    }

    /// An audio input's name as REAPER shows it (from 0).
    pub fn audio_input_name(&self, index: i32) -> String {
        self.api.GetInputChannelName().map(|f| unsafe { api::string(f(index)) }).unwrap_or_default()
    }

    /// An audio output's name as REAPER shows it (from 0).
    pub fn audio_output_name(&self, index: i32) -> String {
        self.api.GetOutputChannelName().map(|f| unsafe { api::string(f(index)) }).unwrap_or_default()
    }

    /// The MIDI inputs present, by REAPER's device number, with their names (the virtual keyboard is 62).
    pub fn midi_inputs(&self) -> Vec<(i32, String)> {
        let (Some(count), Some(name)) = (self.api.GetNumMIDIInputs(), self.api.GetMIDIInputName()) else { return vec![] };
        (0..unsafe { count() }.clamp(0, 256))
            .filter_map(|dev| {
                let mut present = false;
                let text = api::read_buffer(|buf, size| present = unsafe { name(dev, buf, size) });
                present.then_some((dev, text))
            })
            .collect()
    }

    /// The MIDI outputs present, by REAPER's device number, with their names.
    pub fn midi_outputs(&self) -> Vec<(i32, String)> {
        let (Some(count), Some(name)) = (self.api.GetNumMIDIOutputs(), self.api.GetMIDIOutputName()) else { return vec![] };
        (0..unsafe { count() }.clamp(0, 256))
            .filter_map(|dev| {
                let mut present = false;
                let text = api::read_buffer(|buf, size| present = unsafe { name(dev, buf, size) });
                present.then_some((dev, text))
            })
            .collect()
    }

    /// A send's, receive's (`category` below 0) or hardware output's (above 0) envelope, by its chunk name (`<VOLENV`…).
    pub fn send_envelope_in(&self, track: Track, category: i32, index: i32, chunk: &str) -> Option<super::Envelope> {
        let parm = ctext(&format!("P_ENV:{chunk}"));
        let f = self.api.GetSetTrackSendInfo()?;
        let envelope = unsafe { f(track.0, category, index, parm.as_ptr(), ptr::null_mut()) };
        (!envelope.is_null()).then_some(super::Envelope(envelope.cast()))
    }

    // ----- track groups -----

    /// A track's membership of one group role (`VOLUME_LEAD`…) in groups 1-32 (`high` false) or 33-64, one bit a group.
    pub fn group_membership(&self, track: Track, role: &str, high: bool) -> Option<u32> {
        let role = ctext(role);
        let f = if high { self.api.GetSetTrackGroupMembershipHigh()? } else { self.api.GetSetTrackGroupMembership()? };
        Some(unsafe { f(track.0, role.as_ptr(), 0, 0) })
    }

    /// Sets the bits of `mask` to those of `value`; what it was before.
    pub fn set_group_membership(&self, track: Track, role: &str, high: bool, mask: u32, value: u32) -> Option<u32> {
        let role = ctext(role);
        let f = if high { self.api.GetSetTrackGroupMembershipHigh()? } else { self.api.GetSetTrackGroupMembership()? };
        Some(unsafe { f(track.0, role.as_ptr(), mask, value) })
    }

    // ----- project information -----

    /// A number of `GetSetProjectInfo` (`RULER_LANE_COUNT`, `RULER_LANE_HIDDEN:2`…).
    pub fn project_info(&self, proj: Project, key: &str) -> Option<f64> {
        let key = ctext(key);
        self.api.GetSetProjectInfo().map(|f| unsafe { f(proj.0, key.as_ptr(), 0.0, false) })
    }

    pub fn set_project_info(&self, proj: Project, key: &str, value: f64) -> bool {
        let key = ctext(key);
        match self.api.GetSetProjectInfo() {
            Some(f) => {
                unsafe { f(proj.0, key.as_ptr(), value, true) };
                true
            }
            None => false,
        }
    }

    /// A string of `GetSetProjectInfo_String` (`TRACK_GROUP_NAME:3`, `RULER_LANE_NAME:0`…); None when REAPER has none.
    pub fn project_text(&self, proj: Project, key: &str) -> Option<String> {
        let key = ctext(key);
        let f = self.api.GetSetProjectInfo_String()?;
        super::objects::big_string(|buf| unsafe { f(proj.0, key.as_ptr(), buf, false) })
    }

    pub fn set_project_text(&self, proj: Project, key: &str, value: &str) -> bool {
        let key = ctext(key);
        let Some(f) = self.api.GetSetProjectInfo_String() else { return false };
        let mut buf = ctext(value).into_bytes_with_nul();
        unsafe { f(proj.0, key.as_ptr(), buf.as_mut_ptr().cast::<c_char>(), true) }
    }

    /// The project's notes (Project Settings > Notes).
    pub fn project_notes(&self, proj: Project) -> Option<String> {
        let f = self.api.GetSetProjectNotes()?;
        let mut size = 1usize << 14;
        loop {
            let mut buf = vec![0u8; size];
            unsafe { f(proj.0, false, buf.as_mut_ptr().cast(), size as c_int) };
            let len = buf.iter().position(|&b| b == 0).unwrap_or(size);
            if len + 1 < size || size >= STRING_LIMIT {
                buf.truncate(len);
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
            size *= 4;
        }
    }

    pub fn set_project_notes(&self, proj: Project, notes: &str) -> bool {
        let Some(f) = self.api.GetSetProjectNotes() else { return false };
        let mut buf = ctext(notes).into_bytes_with_nul();
        let size = buf.len() as c_int;
        unsafe { f(proj.0, true, buf.as_mut_ptr().cast(), size) };
        true
    }

    /// The arrange view's grid: its division (in whole notes: 0.25 a quarter), the swing mode (0 off, 1 swing, 3 the
    /// measure grid) and the swing amount (-1 to 1).
    pub fn grid(&self, proj: Project) -> Option<(f64, i32, f64)> {
        let f = self.api.GetSetProjectGrid()?;
        let (mut division, mut mode, mut amount) = (0.0, 0, 0.0);
        unsafe { f(proj.0, false, &mut division, &mut mode, &mut amount) };
        Some((division, mode, amount))
    }

    pub fn set_grid(&self, proj: Project, division: f64, mode: i32, amount: f64) -> bool {
        let Some(f) = self.api.GetSetProjectGrid() else { return false };
        let (mut division, mut mode, mut amount) = (division, mode, amount);
        unsafe { f(proj.0, true, &mut division, &mut mode, &mut amount) };
        true
    }

    /// A configuration variable as text (`panmode`, `itemtimelock`, `defsavepath`…); for a project setting, the open
    /// project's.
    pub fn config_text(&self, name: &str) -> Option<String> {
        let name = ctext(name);
        let f = self.api.get_config_var_string()?;
        let mut ok = false;
        let text = api::read_buffer(|buf, size| ok = unsafe { f(name.as_ptr(), buf, size) });
        ok.then_some(text)
    }

    /// Whether an API function is registered (`APIExists`): REAPER's own, or one an extension adds (SWS's `CF_…`).
    pub fn api_exists(&self, name: &str) -> bool {
        let name = ctext(name);
        self.api.APIExists().is_some_and(|f| unsafe { f(name.as_ptr()) })
    }

    /// Sets one in memory (never in reaper.ini): 2 when it is a project setting, 1 a global one, 0 when REAPER refused.
    pub fn set_config_text(&self, name: &str, value: &str) -> i32 {
        let (name, value) = (ctext(name), ctext(value));
        self.api.set_config_var_string().map_or(0, |f| unsafe { f(name.as_ptr(), value.as_ptr(), 0) })
    }

    /// The time selection (not the loop): start and end in seconds, equal when there is none.
    pub fn set_time_selection(&self, proj: Project, start: f64, end: f64) -> bool {
        let Some(f) = self.api.GetSet_LoopTimeRange2() else { return false };
        let (mut start, mut end) = (start, end);
        unsafe { f(proj.0, true, false, &mut start, &mut end, false) };
        true
    }

    /// Takes a track out of the region render matrix of the region with this number.
    pub fn remove_from_region_render(&self, proj: Project, number: i32, track: Track) -> bool {
        match self.api.SetRegionRenderMatrix() {
            Some(f) => {
                unsafe { f(proj.0, number, track.0, -1) };
                true
            }
            None => false,
        }
    }

    // ----- the action list -----

    /// Every main-section action: its command id and name, in REAPER's order.
    pub fn actions(&self) -> Vec<(i32, String)> {
        let (Some(section), Some(enumerate)) = (self.api.SectionFromUniqueID(), self.api.kbd_enumerateActions()) else {
            return vec![];
        };
        let main = unsafe { section(0) };
        let mut found = Vec::new();
        for index in 0..100_000 {
            let mut name: *const c_char = ptr::null();
            let id = unsafe { enumerate(main, index, &mut name) };
            if id <= 0 {
                break;
            }
            found.push((id, unsafe { api::string(name) }));
        }
        found
    }

    // ----- item selection -----
    // (`set_item_selected` and `select_all_items`: m4-r-items' in `reaper/items.rs`, the same calls)

    /// Every item of the project, track by track.
    pub fn all_items(&self, proj: Project) -> Vec<Item> {
        (0..self.track_count(proj))
            .filter_map(|at| self.track_at(proj, at))
            .flat_map(|track| (0..self.track_item_count(track)).filter_map(move |n| self.track_item(track, n)))
            .collect()
    }

    // ----- templates and saving -----

    /// Opens a file in the active tab as REAPER's File > Open does; a `.RTrackTemplate` adds its tracks at the end.
    pub fn open_project(&self, path: &str) -> bool {
        let path = ctext(path);
        match self.api.Main_openProject() {
            Some(f) => {
                unsafe { f(path.as_ptr()) };
                true
            }
            None => false,
        }
    }

    /// `Main_SaveProjectEx`: options &1 the selected tracks as a track template (&2 with their items, &4 with their
    /// envelopes), &8 the project under this name from now on.
    pub fn save_project_as(&self, proj: Project, path: &str, options: i32) -> bool {
        let path = ctext(path);
        match self.api.Main_SaveProjectEx() {
            Some(f) => {
                unsafe { f(proj.0, path.as_ptr(), options) };
                true
            }
            None => false,
        }
    }

    /// A track's state as REAPER keeps it for undo (lighter than the file's: no plug-in states).
    pub fn track_undo_chunk(&self, track: Track) -> Option<String> {
        let f = self.api.GetTrackStateChunk()?;
        let mut size = 1usize << 14;
        loop {
            let mut buf = vec![0u8; size];
            let ok = unsafe { f(track.0, buf.as_mut_ptr().cast::<c_char>(), size as c_int, true) };
            if !ok {
                return None;
            }
            let len = buf.iter().position(|&b| b == 0).unwrap_or(size);
            if len + 1 < size {
                buf.truncate(len);
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
            if size >= 256 << 20 {
                return None;
            }
            size *= 4;
        }
    }
}
