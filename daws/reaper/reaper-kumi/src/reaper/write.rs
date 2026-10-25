//! What the bridge writes to REAPER, and the undo history it names its writes in. Each call is REAPER's own, one for one;
//! a function this REAPER lacks answers false or None, and the change that needed it is refused before it begins.

use std::ffi::{c_char, c_int};
use std::ptr;

use super::ctext;
use super::{Fx, FxOwner, Project, Reaper, Take, Track};
use crate::api::{self, ProjectMarker};

/// The most a track's chunk may grow to when it is read: the plug-in states in it are what take the room.
const MAX_CHUNK: usize = 256 << 20;

thread_local! {
    /// What the last track chunk read needed (`grown_text` starts the next from twice it).
    static TRACK_CHUNK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Reaper<'_> {
    // ----- tracks -----

    pub fn set_track_value(&self, track: Track, parm: &str, value: f64) -> bool {
        super::wrote();
        let parm = ctext(parm);
        self.api.SetMediaTrackInfo_Value().is_some_and(|f| unsafe { f(track.0, parm.as_ptr(), value) })
    }

    /// A track's text attribute (`P_NAME`).
    pub fn set_track_string(&self, track: Track, parm: &str, value: &str) -> bool {
        super::wrote();
        let (parm, value) = (ctext(parm), ctext(value));
        let Some(f) = self.api.GetSetMediaTrackInfo_String() else { return false };
        // The buffer is read, not written, when setting; REAPER wants it writable all the same.
        let mut buf = value.into_bytes_with_nul();
        unsafe { f(track.0, parm.as_ptr(), buf.as_mut_ptr().cast::<c_char>(), true) }
    }

    /// A new track at `index` (0-based; at the end when past the last), without the project's default FX and sends.
    pub fn insert_track(&self, proj: Project, index: i32) -> Option<Track> {
        super::wrote();
        let at = index.clamp(0, self.track_count(proj));
        unsafe { self.api.InsertTrackAtIndex()?(at, false) };
        self.track_at(proj, at)
    }

    /// Names a note of a track (`SetTrackMIDINoteNameEx`; channel -1 for all): what the MIDI editor shows for its row.
    pub fn set_note_name(&self, proj: Project, track: Track, pitch: i32, channel: i32, name: &str) -> bool {
        super::wrote();
        let name = ctext(name);
        self.api.SetTrackMIDINoteNameEx().is_some_and(|f| unsafe { f(proj.0, track.0, pitch, channel, name.as_ptr()) })
    }

    pub fn delete_track(&self, track: Track) -> bool {
        super::wrote();
        match self.api.DeleteTrack() {
            Some(f) => {
                unsafe { f(track.0) };
                true
            }
            None => false,
        }
    }

    /// The track's whole state as REAPER writes it into a project file, plug-in states and GUIDs included.
    pub fn track_chunk(&self, track: Track) -> Option<String> {
        let f = self.api.GetTrackStateChunk()?;
        super::grown_text(&TRACK_CHUNK, 1 << 16, MAX_CHUNK, |buf, size| unsafe { f(track.0, buf, size, false) })
    }

    pub fn set_track_chunk(&self, track: Track, chunk: &str) -> bool {
        super::wrote();
        let chunk = ctext(chunk);
        self.api.SetTrackStateChunk().is_some_and(|f| unsafe { f(track.0, chunk.as_ptr(), false) })
    }

    /// A colour in the order REAPER stores it (without the flag that says it is set).
    pub fn color_to_native(&self, r: i32, g: i32, b: i32) -> Option<i32> {
        self.api.ColorToNative().map(|f| unsafe { f(r, g, b) })
    }

    /// Selects exactly this track (the master included).
    pub fn select_only(&self, track: Track) {
        super::wrote();
        if let Some(f) = self.api.SetOnlyTrackSelected() {
            unsafe { f(track.0) }
        }
    }

    pub fn set_track_selected(&self, track: Track, selected: bool) {
        super::wrote();
        if let Some(f) = self.api.SetTrackSelected() {
            unsafe { f(track.0, selected) }
        }
    }

    /// Moves the selected tracks to just above the track at `before` (counted before the move). `folder` 0 leaves their
    /// folder depths as they are.
    pub fn reorder_selected(&self, before: i32, folder: i32) -> bool {
        super::wrote();
        self.api.ReorderSelectedTracks().is_some_and(|f| unsafe { f(before, folder) })
    }

    // ----- sends -----

    /// A send from `track` to `dest`, or a hardware output when there is none: its index among the sends of its kind.
    pub fn create_send(&self, track: Track, dest: Option<Track>) -> Option<i32> {
        super::wrote();
        let index = unsafe { self.api.CreateTrackSend()?(track.0, dest.map_or(ptr::null_mut(), |d| d.0)) };
        (index >= 0).then_some(index)
    }

    pub fn remove_send(&self, track: Track, category: i32, index: i32) -> bool {
        super::wrote();
        self.api.RemoveTrackSend().is_some_and(|f| unsafe { f(track.0, category, index) })
    }

    /// A send's text attribute (`P_EXT:…`): None when it has none.
    pub fn send_text(&self, track: Track, category: i32, index: i32, parm: &str) -> Option<String> {
        let parm = ctext(parm);
        let f = self.api.GetSetTrackSendInfo_String()?;
        // Asked for a read, REAPER writes the text without being told the buffer's size: the big buffer of its other such reads.
        let text = super::objects::big_string(|buf| unsafe { f(track.0, category, index, parm.as_ptr(), buf, false) })?;
        (!text.is_empty()).then_some(text)
    }

    pub fn set_send_text(&self, track: Track, category: i32, index: i32, parm: &str, value: &str) -> bool {
        super::wrote();
        let (parm, value) = (ctext(parm), ctext(value));
        let Some(f) = self.api.GetSetTrackSendInfo_String() else { return false };
        let mut buf = value.into_bytes_with_nul();
        unsafe { f(track.0, category, index, parm.as_ptr(), buf.as_mut_ptr().cast(), true) }
    }

    /// The channels an FX pin takes from (input) or gives to (output), as bits 0 to 63: a track's FX or a take's.
    pub fn fx_pin(&self, fx: Fx, output: bool, pin: i32) -> Option<u64> {
        let mut high = 0;
        let low = match fx.owner {
            FxOwner::Track(track) => unsafe { self.api.TrackFX_GetPinMappings()?(track.0, fx.index, i32::from(output), pin, &mut high) },
            FxOwner::Take(take) => unsafe { self.api.TakeFX_GetPinMappings()?(take.0, fx.index, i32::from(output), pin, &mut high) },
        };
        Some((low as u32 as u64) | ((high as u32 as u64) << 32))
    }

    pub fn set_fx_pin(&self, fx: Fx, output: bool, pin: i32, mask: u64) -> bool {
        super::wrote();
        let (low, high) = (mask as u32 as i32, (mask >> 32) as u32 as i32);
        match fx.owner {
            FxOwner::Track(track) => {
                self.api.TrackFX_SetPinMappings().is_some_and(|f| unsafe { f(track.0, fx.index, i32::from(output), pin, low, high) })
            }
            FxOwner::Take(take) => {
                self.api.TakeFX_SetPinMappings().is_some_and(|f| unsafe { f(take.0, fx.index, i32::from(output), pin, low, high) })
            }
        }
    }

    /// How many input and output pins an FX has.
    pub fn fx_io(&self, fx: Fx) -> Option<(i32, i32)> {
        let (mut ins, mut outs) = (0, 0);
        let kind = match fx.owner {
            FxOwner::Track(track) => unsafe { self.api.TrackFX_GetIOSize()?(track.0, fx.index, &mut ins, &mut outs) },
            FxOwner::Take(take) => unsafe { self.api.TakeFX_GetIOSize()?(take.0, fx.index, &mut ins, &mut outs) },
        };
        (kind >= 0).then_some((ins, outs))
    }

    pub fn set_send_value(&self, track: Track, category: i32, index: i32, parm: &str, value: f64) -> bool {
        super::wrote();
        let parm = ctext(parm);
        self.api.SetTrackSendInfo_Value().is_some_and(|f| unsafe { f(track.0, category, index, parm.as_ptr(), value) })
    }

    // ----- the screen -----

    /// Redraws the arrange view and the track list after a change made by script.
    pub fn refresh_after_change(&self) {
        if let Some(f) = self.api.TrackList_AdjustWindows() {
            unsafe { f(false) }
        }
        if let Some(f) = self.api.UpdateArrange() {
            unsafe { f() }
        }
    }

    // ----- the undo history -----

    pub fn undo_begin(&self, proj: Project) -> bool {
        match self.api.Undo_BeginBlock2() {
            Some(f) => {
                unsafe { f(proj.0) };
                true
            }
            None => false,
        }
    }

    /// Ends the block as one undo point named `desc`. `flags` -1 covers every kind of change; 0 makes a point only if
    /// something REAPER itself marked changed.
    pub fn undo_end(&self, proj: Project, desc: &str, flags: i32) -> bool {
        let desc = ctext(desc);
        match self.api.Undo_EndBlock2() {
            Some(f) => {
                unsafe { f(proj.0, desc.as_ptr(), flags) };
                true
            }
            None => false,
        }
    }

    /// The description of the point Undo would take back.
    pub fn undo_can_undo(&self, proj: Project) -> Option<String> {
        let text = unsafe { api::string(self.api.Undo_CanUndo2()?(proj.0)) };
        (!text.is_empty()).then_some(text)
    }

    /// The description of the point Redo would bring back.
    pub fn undo_can_redo(&self, proj: Project) -> Option<String> {
        let text = unsafe { api::string(self.api.Undo_CanRedo2()?(proj.0)) };
        (!text.is_empty()).then_some(text)
    }

    /// The history's entries, and the one the project is at; entry 0 is "--- Initial state ---".
    pub fn undo_position(&self, proj: Project) -> Option<(i32, i32)> {
        let entries = unsafe { self.api.Undo_GetNumEntries()?(proj.0) };
        let at = unsafe { self.api.Undo_GetCurEntry()?(proj.0) };
        Some((entries, at))
    }

    pub fn undo_entry(&self, proj: Project, index: i32) -> Option<String> {
        let text = unsafe { api::string(self.api.Undo_GetEntryDesc()?(proj.0, index)) };
        (!text.is_empty()).then_some(text)
    }
}

/// Markers and regions, which REAPER hands out by pointer and names by GUID.
impl Reaper<'_> {
    fn marker_pointer(&self, proj: Project, guid: &str) -> Option<*mut ProjectMarker> {
        let guid = ctext(guid);
        let marker = unsafe { self.api.GetRegionOrMarker()?(proj.0, -1, guid.as_ptr()) };
        (!marker.is_null()).then_some(marker)
    }

    /// A marker or region made at `start` (seconds), and its GUID. `end` is the region's.
    pub fn add_marker(&self, proj: Project, region: bool, start: f64, end: f64, name: &str, color: i32) -> Option<String> {
        super::wrote();
        let name = ctext(name);
        let marker = unsafe { self.api.AddRegionOrMarker()?(proj.0, region, start, end, name.as_ptr(), -1, color) };
        if marker.is_null() {
            return None;
        }
        let f = self.api.GetSetRegionOrMarkerInfo_String()?;
        let mut buf = vec![0u8; 256];
        let ok = unsafe { f(proj.0, marker, c"GUID".as_ptr(), buf.as_mut_ptr().cast(), false) };
        let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned())
    }

    /// A marker's number attribute: `D_STARTPOS`, `D_ENDPOS`, `I_NUMBER`, `I_CUSTOMCOLOR`.
    pub fn set_marker_value(&self, proj: Project, guid: &str, parm: &str, value: f64) -> bool {
        super::wrote();
        let (Some(marker), Some(f)) = (self.marker_pointer(proj, guid), self.api.SetRegionOrMarkerInfo_Value()) else { return false };
        let parm = ctext(parm);
        // What it returns is not documented as a success flag: the change reads the marker again.
        unsafe { f(proj.0, marker, parm.as_ptr(), value) };
        true
    }

    /// Puts a track in the region render matrix of the region with this number.
    pub fn add_to_region_render(&self, proj: Project, number: i32, track: Track) -> bool {
        super::wrote();
        match self.api.SetRegionRenderMatrix() {
            Some(f) => {
                unsafe { f(proj.0, number, track.0, 1) };
                true
            }
            None => false,
        }
    }

    pub fn set_marker_name(&self, proj: Project, guid: &str, name: &str) -> bool {
        super::wrote();
        let (Some(marker), Some(f)) = (self.marker_pointer(proj, guid), self.api.GetSetRegionOrMarkerInfo_String()) else { return false };
        let mut buf = ctext(name).into_bytes_with_nul();
        unsafe { f(proj.0, marker, c"P_NAME".as_ptr(), buf.as_mut_ptr().cast::<c_char>(), true) }
    }

    pub fn delete_marker(&self, proj: Project, guid: &str) -> bool {
        super::wrote();
        let (Some(marker), Some(value), Some(delete)) =
            (self.marker_pointer(proj, guid), self.api.GetRegionOrMarkerInfo_Value(), self.api.DeleteProjectMarkerByIndex())
        else {
            return false;
        };
        let index = unsafe { value(proj.0, marker, c"I_INDEX".as_ptr()) } as i32;
        unsafe { delete(proj.0, index) }
    }
}

/// The timeline: tempo, time signature, loop, cursor.
impl Reaper<'_> {
    pub fn set_current_bpm(&self, proj: Project, bpm: f64) -> bool {
        super::wrote();
        match self.api.SetCurrentBPM() {
            Some(f) => {
                unsafe { f(proj.0, bpm, false) };
                true
            }
            None => false,
        }
    }

    /// A tempo or time signature marker: the existing one at `index`, or a new one for -1. `num` and `denom` 0 leave the
    /// time signature alone; `bpm` 0 keeps the tempo in force.
    #[allow(clippy::too_many_arguments)]
    pub fn set_tempo_marker(&self, proj: Project, index: i32, time: f64, bpm: f64, num: i32, denom: i32, linear: bool) -> bool {
        super::wrote();
        self.api.SetTempoTimeSigMarker().is_some_and(|f| unsafe { f(proj.0, index, time, -1, -1.0, bpm, num, denom, linear) })
    }

    pub fn delete_tempo_marker(&self, proj: Project, index: i32) -> bool {
        super::wrote();
        self.api.DeleteTempoTimeSigMarker().is_some_and(|f| unsafe { f(proj.0, index) })
    }

    pub fn set_loop_points(&self, proj: Project, start: f64, end: f64) -> bool {
        super::wrote();
        let Some(f) = self.api.GetSet_LoopTimeRange2() else { return false };
        let (mut start, mut end) = (start, end);
        unsafe { f(proj.0, true, true, &mut start, &mut end, false) };
        true
    }

    pub fn set_repeat(&self, proj: Project, on: bool) -> bool {
        super::wrote();
        match self.api.GetSetRepeatEx() {
            Some(f) => {
                unsafe { f(proj.0, on as c_int) };
                true
            }
            None => false,
        }
    }

    /// The edit cursor, without moving the view; the play position follows it when `seek` is set.
    pub fn set_cursor(&self, proj: Project, time: f64, seek: bool) -> bool {
        super::wrote();
        match self.api.SetEditCurPos2() {
            Some(f) => {
                unsafe { f(proj.0, time, false, seek) };
                true
            }
            None => false,
        }
    }
}

/// A MIDI take's events, all at once.
impl Reaper<'_> {
    pub fn midi_events(&self, take: Take) -> Option<Vec<crate::midi::Event>> {
        let f = self.api.MIDI_GetAllEvts()?;
        let (notes, ccs, texts) = self.midi_counts(take)?;
        let mut size = ((notes.max(0) as usize * 2 + ccs.max(0) as usize) * 12 + texts.max(0) as usize * 64 + 4096).max(4096);
        loop {
            let mut buf = vec![0u8; size];
            let mut written = size as c_int;
            let ok = unsafe { f(take.0, buf.as_mut_ptr().cast(), &mut written) };
            if ok && written >= 0 && written as usize <= size {
                buf.truncate(written as usize);
                return crate::midi::decode(&buf);
            }
            // Too small: REAPER says how much it needs, or not, and then the buffer doubles.
            size = (written.max(0) as usize).max(size * 2);
            if size > 1 << 30 {
                return None;
            }
        }
    }

    /// Replaces all the take's MIDI with these events, which must be in order.
    pub fn set_midi_events(&self, take: Take, events: &[crate::midi::Event]) -> bool {
        super::wrote();
        let buf = crate::midi::encode(events);
        self.api.MIDI_SetAllEvts().is_some_and(|f| unsafe { f(take.0, buf.as_ptr().cast(), buf.len() as c_int) })
    }
}

/// FX in a track's chain. An index is the FX's place in the main chain.
impl Reaper<'_> {
    /// Adds the plug-in (or the FX chain file) `name` names, at `position` or at the end; the index of the first FX made,
    /// or -1 when REAPER has none by that name.
    pub fn add_fx_by_name(&self, track: Track, name: &str, position: Option<i32>) -> i32 {
        super::wrote();
        let name = ctext(name);
        let instantiate = position.map_or(-1, |p| -1000 - p);
        self.api.TrackFX_AddByName().map_or(-1, |f| unsafe { f(track.0, name.as_ptr(), false, instantiate) })
    }

    pub fn delete_fx(&self, track: Track, index: i32) -> bool {
        super::wrote();
        self.api.TrackFX_Delete().is_some_and(|f| unsafe { f(track.0, index) })
    }

    /// Moves an FX within its track's chain, so that it ends up at `to`.
    pub fn move_fx(&self, track: Track, from: i32, to: i32) -> bool {
        super::wrote();
        match self.api.TrackFX_CopyToTrack() {
            Some(f) => {
                unsafe { f(track.0, from, track.0, to, true) };
                true
            }
            None => false,
        }
    }

    pub fn set_fx_enabled(&self, track: Track, index: i32, enabled: bool) -> bool {
        super::wrote();
        match self.api.TrackFX_SetEnabled() {
            Some(f) => {
                unsafe { f(track.0, index, enabled) };
                true
            }
            None => false,
        }
    }

    pub fn set_fx_param_normalized(&self, track: Track, index: i32, param: i32, value: f64) -> bool {
        super::wrote();
        self.api.TrackFX_SetParamNormalized().is_some_and(|f| unsafe { f(track.0, index, param, value) })
    }

    /// What the plug-in would print for a normalized value, without setting it.
    pub fn format_fx_param(&self, track: Track, index: i32, param: i32, value: f64) -> Option<String> {
        let f = self.api.TrackFX_FormatParamValueNormalized()?;
        let mut ok = false;
        let text = api::read_buffer(|buf, size| ok = unsafe { f(track.0, index, param, value, buf, size) });
        ok.then_some(text)
    }
}
