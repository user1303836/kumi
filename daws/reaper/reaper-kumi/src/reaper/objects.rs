//! What the bridge reads of REAPER's objects: the project's timeline, tracks, items, takes, FX, sends, envelopes, MIDI and
//! markers. Each call is REAPER's own, one for one; a function this REAPER lacks reads as empty.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;

use super::ctext;
use super::{Envelope, Fx, FxOwner, Item, Project, Reaper, Take, Track};
use crate::api::{self, ProjectMarker};

/// The size told to REAPER's string getters that take one (the most of a text they write).
const BIG: usize = 4096;
/// The buffer of REAPER's string getters that take no size: REAPER writes the whole text whatever the buffer holds
/// (lab 7.82: more than 96 KiB of a track's 200 KB P_EXT), so it is as big as a buffer gets. Its memory is taken only as far as
/// REAPER writes, once: REAPER's main thread reads one text at a time.
const UNSIZED: usize = 64 * 1024 * 1024;

thread_local! {
    static UNSIZED_BUF: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// Added to an FX index to address the track's input chain.
pub const INPUT_FX: i32 = 0x100_0000;
/// Added to an FX index to address a container's child.
pub const CONTAINER_FX: i32 = 0x200_0000;
/// `GetTrackColor`'s flag: the colour is set.
const COLOR_SET: i32 = 0x100_0000;

/// Reads a string a getter writes into a big buffer: one it isn't told the size of (`…NeedBig`), which holds a text of any
/// length REAPER gives (a name, a P_EXT value, razor edits), or one it is told `BIG` of.
pub(super) fn big_string(fill: impl FnOnce(*mut c_char) -> bool) -> Option<String> {
    UNSIZED_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        if buf.len() < UNSIZED {
            *buf = vec![0u8; UNSIZED];
        }
        buf[0] = 0;
        if !fill(buf.as_mut_ptr().cast()) {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(UNSIZED - 1);
        let text = String::from_utf8_lossy(&buf[..len]).into_owned();
        // (a long text's memory is given back; the next read takes a fresh buffer)
        if len > 1 << 20 {
            *buf = Vec::new();
        }
        Some(text)
    })
}

/// Reads a string from a getter that takes the buffer and its size.
fn text(mut fill: impl FnMut(*mut c_char, c_int) -> bool) -> String {
    api::read_buffer(|buf, size| {
        fill(buf, size);
    })
}

/// A marker or region as `EnumProjectMarkers3` gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// REAPER's own id for it: the shown number can be edited and repeat.
    pub guid: String,
    pub is_region: bool,
    pub start: f64,
    pub end: f64,
    pub name: String,
    /// The number REAPER shows (editable, and may repeat).
    pub number: i32,
    pub color: i32,
    /// The ruler lane it sits in (0 is the first).
    pub lane: i32,
}

/// A tempo or time signature marker.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMarker {
    pub time: f64,
    pub bpm: f64,
    pub num: i32,
    pub denom: i32,
    pub linear: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeSignature {
    pub num: i32,
    pub denom: i32,
    pub bpm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiNote {
    pub selected: bool,
    pub muted: bool,
    pub start_ppq: f64,
    pub end_ppq: f64,
    pub channel: i32,
    pub pitch: i32,
    pub velocity: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiCc {
    pub selected: bool,
    pub muted: bool,
    pub ppq: f64,
    /// The status byte without the channel: 0xB0 control change, 0xE0 pitch bend, 0xC0 program change…
    pub kind: i32,
    pub channel: i32,
    pub data1: i32,
    pub data2: i32,
}

/// Where `GetTouchedOrFocusedFX` says an FX is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxHit {
    /// -1 for the master.
    pub track: i32,
    /// -1 when the FX isn't a take's.
    pub item: i32,
    pub take: i32,
    pub fx: i32,
    /// The parameter (mode 0), or in mode 1 whether the FX is open but no longer focused (1).
    pub param: i32,
}

impl Reaper<'_> {
    // ----- the project's timeline -----

    pub fn project_length(&self, proj: Project) -> f64 {
        self.api.GetProjectLength().map(|f| unsafe { f(proj.0) }).unwrap_or(0.0)
    }

    pub fn cursor(&self, proj: Project) -> f64 {
        self.api.GetCursorPositionEx().map(|f| unsafe { f(proj.0) }).unwrap_or(0.0)
    }

    /// Where the project is playing; the edit cursor's when it isn't.
    pub fn play_position(&self, proj: Project) -> f64 {
        self.api.GetPlayPosition2Ex().map(|f| unsafe { f(proj.0) }).unwrap_or(0.0)
    }

    fn loop_time_range(&self, proj: Project, is_loop: bool) -> (f64, f64) {
        let (mut start, mut end) = (0.0, 0.0);
        if let Some(f) = self.api.GetSet_LoopTimeRange2() {
            unsafe { f(proj.0, false, is_loop, &mut start, &mut end, false) };
        }
        (start, end)
    }

    /// The loop points.
    pub fn loop_points(&self, proj: Project) -> (f64, f64) {
        self.loop_time_range(proj, true)
    }

    /// The time selection; start equals end when there is none.
    pub fn time_selection(&self, proj: Project) -> (f64, f64) {
        self.loop_time_range(proj, false)
    }

    /// Whether repeat (the loop button) is on.
    pub fn repeat(&self, proj: Project) -> bool {
        self.api.GetSetRepeatEx().is_some_and(|f| unsafe { f(proj.0, -1) } != 0)
    }

    pub fn time_signature_at(&self, proj: Project, time: f64) -> Option<TimeSignature> {
        let f = self.api.TimeMap_GetTimeSigAtTime()?;
        let (mut num, mut denom, mut bpm) = (4, 4, 0.0);
        unsafe { f(proj.0, time, &mut num, &mut denom, &mut bpm) };
        Some(TimeSignature { num, denom, bpm })
    }

    /// The time in quarter notes from the project start.
    pub fn quarter_notes_at(&self, proj: Project, time: f64) -> Option<f64> {
        self.api.TimeMap2_timeToQN().map(|f| unsafe { f(proj.0, time) })
    }

    /// The time of a position given in quarter notes from the project start.
    pub fn time_at_quarter_notes(&self, proj: Project, qn: f64) -> Option<f64> {
        self.api.TimeMap2_QNToTime().map(|f| unsafe { f(proj.0, qn) })
    }

    /// The 0-based measure and the 0-based beat in it.
    pub fn bar_and_beat_at(&self, proj: Project, time: f64) -> Option<(i32, f64)> {
        let f = self.api.TimeMap2_timeToBeats()?;
        let (mut measures, mut beats_in_measure, mut full_beats, mut denom) = (0, 0, 0.0, 0);
        let beat = unsafe { f(proj.0, time, &mut measures, &mut beats_in_measure, &mut full_beats, &mut denom) };
        // REAPER counts the beats of the time signature (eighths in 6/8); everything else here counts quarter notes.
        let denominator = if denom > 0 { denom as f64 } else { 4.0 };
        Some((measures, beat * 4.0 / denominator))
    }

    /// The time range the arrange view shows.
    pub fn arrange_view(&self, proj: Project) -> Option<(f64, f64)> {
        let f = self.api.GetSet_ArrangeView2()?;
        let (mut start, mut end) = (0.0, 0.0);
        unsafe { f(proj.0, false, 0, 0, &mut start, &mut end) };
        (end > start).then_some((start, end))
    }

    /// The open project tabs.
    pub fn tab_count(&self) -> usize {
        self.projects().len()
    }

    pub fn project_valid(&self, proj: Project) -> bool {
        self.projects().iter().any(|p| p.ptr == proj)
    }

    // ----- markers, regions and the tempo map -----

    /// Markers and regions together (REAPER 7.62's object API: each has a GUID).
    pub fn marker_count(&self, proj: Project) -> i32 {
        self.api.GetNumRegionsOrMarkers().map(|f| unsafe { f(proj.0) }).unwrap_or(0)
    }

    /// The marker or region at an internal index.
    pub fn marker(&self, proj: Project, index: i32) -> Option<Marker> {
        let marker = unsafe { self.api.GetRegionOrMarker()?(proj.0, index, ptr::null()) };
        self.marker_at(proj, marker)
    }

    /// The marker or region with this GUID.
    pub fn marker_by_guid(&self, proj: Project, guid: &str) -> Option<Marker> {
        let Some(guid) = ctext(guid) else { return None };
        let marker = unsafe { self.api.GetRegionOrMarker()?(proj.0, -1, guid.as_ptr()) };
        self.marker_at(proj, marker)
    }

    fn marker_at(&self, proj: Project, marker: *mut ProjectMarker) -> Option<Marker> {
        if marker.is_null() {
            return None;
        }
        let value = |parm: &std::ffi::CStr| {
            self.api.GetRegionOrMarkerInfo_Value().map(|f| unsafe { f(proj.0, marker, parm.as_ptr()) }).unwrap_or(0.0)
        };
        let string = |parm: &std::ffi::CStr| {
            let f = self.api.GetSetRegionOrMarkerInfo_String();
            f.and_then(|f| big_string(|buf| unsafe { f(proj.0, marker, parm.as_ptr(), buf, false) })).unwrap_or_default()
        };
        let is_region = value(c"B_ISREGION") != 0.0;
        let start = value(c"D_STARTPOS");
        Some(Marker {
            guid: string(c"GUID"),
            is_region,
            start,
            end: if is_region { value(c"D_ENDPOS") } else { start },
            name: string(c"P_NAME"),
            number: value(c"I_NUMBER") as i32,
            color: value(c"I_CUSTOMCOLOR") as i32,
            lane: value(c"I_LANENUMBER") as i32,
        })
    }

    /// The tracks (the master among them) a region renders in the region render matrix, by the region's number.
    pub fn region_render_tracks(&self, proj: Project, number: i32) -> Vec<Track> {
        let Some(f) = self.api.EnumRegionRenderMatrix() else { return vec![] };
        (0..4096).map_while(|n| Some(unsafe { f(proj.0, number, n) }).filter(|t| !t.is_null())).map(Track).collect()
    }

    pub fn tempo_marker_count(&self, proj: Project) -> i32 {
        self.api.CountTempoTimeSigMarkers().map(|f| unsafe { f(proj.0) }).unwrap_or(0)
    }

    pub fn tempo_marker(&self, proj: Project, index: i32) -> Option<TempoMarker> {
        let f = self.api.GetTempoTimeSigMarker()?;
        let (mut time, mut measure, mut beat, mut bpm, mut num, mut denom, mut linear) = (0.0, 0, 0.0, 0.0, 0, 0, false);
        let ok = unsafe { f(proj.0, index, &mut time, &mut measure, &mut beat, &mut bpm, &mut num, &mut denom, &mut linear) };
        ok.then_some(TempoMarker { time, bpm, num, denom, linear })
    }

    // ----- tracks -----

    pub fn track_at(&self, proj: Project, index: i32) -> Option<Track> {
        let track = unsafe { self.api.GetTrack()?(proj.0, index) };
        (!track.is_null()).then_some(Track(track))
    }

    pub fn master_track(&self, proj: Project) -> Option<Track> {
        let track = unsafe { self.api.GetMasterTrack()?(proj.0) };
        (!track.is_null()).then_some(Track(track))
    }

    /// A value the project carries for an extension (`kumi`/`project` is the Kumi project id).
    pub fn project_ext_state(&self, proj: Project, section: &str, key: &str) -> Option<String> {
        let (Some(section), Some(key)) = (ctext(section), ctext(key)) else { return None };
        let f = self.api.GetProjExtState()?;
        let mut found = 0;
        let value = big_string(|buf| {
            found = unsafe { f(proj.0, section.as_ptr(), key.as_ptr(), buf, BIG as c_int) };
            true
        })?;
        (found > 0).then_some(value)
    }

    /// Sets a value the project carries for an extension; false when this REAPER can't. It marks nothing changed and makes no
    /// undo point (7.82 lab): REAPER writes it into the file with the producer's next save.
    pub fn set_project_ext_state(&self, proj: Project, section: &str, key: &str, value: &str) -> bool {
        super::wrote();
        let (Some(section), Some(key), Some(value)) = (ctext(section), ctext(key), ctext(value)) else { return false };
        let Some(f) = self.api.SetProjExtState() else { return false };
        unsafe { f(proj.0, section.as_ptr(), key.as_ptr(), value.as_ptr()) };
        true
    }

    /// How many entries the undo history holds (REAPER 7.79): one after a project is loaded, more with each edit.
    pub fn undo_entries(&self, proj: Project) -> Option<i32> {
        self.api.Undo_GetNumEntries().map(|f| unsafe { f(proj.0) })
    }

    /// Whether the track has an instrument plug-in.
    pub fn track_has_instrument(&self, track: Track) -> bool {
        self.api.TrackFX_GetInstrument().is_some_and(|f| unsafe { f(track.0) } >= 0)
    }

    /// Whether REAPER still has this track in the project (a track deleted since may have been freed).
    pub fn track_valid(&self, proj: Project, track: Track) -> bool {
        self.valid(proj, track.0.cast(), c"MediaTrack*")
    }

    pub fn item_valid(&self, proj: Project, item: Item) -> bool {
        self.valid(proj, item.0.cast(), c"MediaItem*")
    }

    pub fn take_valid(&self, proj: Project, take: Take) -> bool {
        self.valid(proj, take.0.cast(), c"MediaItem_Take*")
    }

    pub fn envelope_valid(&self, proj: Project, envelope: Envelope) -> bool {
        self.valid(proj, envelope.0.cast(), c"TrackEnvelope*")
    }

    fn valid(&self, proj: Project, pointer: *mut c_void, class: &std::ffi::CStr) -> bool {
        self.api.ValidatePtr2().is_some_and(|f| unsafe { f(proj.0, pointer, class.as_ptr()) })
    }

    /// A track's peak level now on a channel (0 left, 1 right), as an amplitude: REAPER's meter. None where REAPER doesn't
    /// give it.
    pub fn track_peak(&self, track: Track, channel: i32) -> Option<f64> {
        self.api.Track_GetPeakInfo().map(|f| unsafe { f(track.0, channel) }).filter(|peak| peak.is_finite())
    }

    pub fn track_value(&self, track: Track, parm: &str) -> f64 {
        let Some(parm) = ctext(parm) else { return 0.0 };
        self.api.GetMediaTrackInfo_Value().map(|f| unsafe { f(track.0, parm.as_ptr()) }).unwrap_or(0.0)
    }

    /// The folder track this track is inside, if it is inside one.
    pub fn track_parent(&self, track: Track) -> Option<Track> {
        let f = self.api.GetSetMediaTrackInfo()?;
        let parent = unsafe { f(track.0, c"P_PARTRACK".as_ptr(), ptr::null_mut()) };
        (!parent.is_null()).then_some(Track(parent.cast()))
    }

    /// A track's text attribute (`P_LANENAME:2`…), None when REAPER has none.
    pub fn track_string(&self, track: Track, parm: &str) -> Option<String> {
        let Some(parm) = ctext(parm) else { return None };
        let f = self.api.GetSetMediaTrackInfo_String()?;
        big_string(|buf| unsafe { f(track.0, parm.as_ptr(), buf, false) })
    }

    /// How many folders deep the track sits (0 at the top level).
    pub fn track_depth(&self, track: Track) -> i32 {
        self.api.GetTrackDepth().map(|f| unsafe { f(track.0) }).unwrap_or(0)
    }

    pub fn track_name(&self, track: Track) -> String {
        match self.api.GetTrackName() {
            Some(f) => text(|buf, size| unsafe { f(track.0, buf, size) }),
            None => String::new(),
        }
    }

    pub fn track_guid(&self, track: Track) -> String {
        let f = self.api.GetSetMediaTrackInfo_String();
        f.and_then(|f| big_string(|buf| unsafe { f(track.0, c"GUID".as_ptr(), buf, false) })).unwrap_or_default()
    }

    /// The track's colour as REAPER stores it (the OS's order, 0x1000000 set), or None when it has none.
    pub fn track_color(&self, track: Track) -> Option<i32> {
        let color = self.api.GetTrackColor().map(|f| unsafe { f(track.0) })?;
        (color & COLOR_SET != 0).then_some(color)
    }

    /// The selected tracks (the master included): how many there are, and the first `limit` of them.
    pub fn selected_tracks_up_to(&self, proj: Project, limit: usize) -> (usize, Vec<Track>) {
        let (Some(count), Some(get)) = (self.api.CountSelectedTracks2(), self.api.GetSelectedTrack2()) else { return (0, vec![]) };
        let total = unsafe { count(proj.0, true) }.max(0) as usize;
        let tracks = (0..total.min(limit)).map(|i| unsafe { get(proj.0, i as c_int, true) }).filter(|t| !t.is_null()).map(Track).collect();
        (total, tracks)
    }

    /// The track the user touched last.
    pub fn last_touched_track(&self) -> Option<Track> {
        let track = unsafe { self.api.GetLastTouchedTrack()?() };
        (!track.is_null()).then_some(Track(track))
    }

    // ----- items and takes -----

    pub fn track_item_count(&self, track: Track) -> i32 {
        self.api.CountTrackMediaItems().map(|f| unsafe { f(track.0) }).unwrap_or(0)
    }

    pub fn track_item(&self, track: Track, index: i32) -> Option<Item> {
        let item = unsafe { self.api.GetTrackMediaItem()?(track.0, index) };
        (!item.is_null()).then_some(Item(item))
    }

    pub fn item_track(&self, item: Item) -> Option<Track> {
        let track = unsafe { self.api.GetMediaItemTrack()?(item.0) };
        (!track.is_null()).then_some(Track(track))
    }

    pub fn item_value(&self, item: Item, parm: &str) -> f64 {
        let Some(parm) = ctext(parm) else { return 0.0 };
        self.api.GetMediaItemInfo_Value().map(|f| unsafe { f(item.0, parm.as_ptr()) }).unwrap_or(0.0)
    }

    pub fn item_guid(&self, item: Item) -> String {
        let f = self.api.GetSetMediaItemInfo_String();
        f.and_then(|f| big_string(|buf| unsafe { f(item.0, c"GUID".as_ptr(), buf, false) })).unwrap_or_default()
    }

    /// The selected items: how many there are, and the first `limit` of them. REAPER walks the project for each one it is
    /// asked for, so `limit` stays small.
    pub fn selected_items_up_to(&self, proj: Project, limit: usize) -> (usize, Vec<Item>) {
        let (Some(count), Some(get)) = (self.api.CountSelectedMediaItems(), self.api.GetSelectedMediaItem()) else { return (0, vec![]) };
        let total = unsafe { count(proj.0) }.max(0) as usize;
        let items = (0..total.min(limit)).map(|i| unsafe { get(proj.0, i as c_int) }).filter(|i| !i.is_null()).map(Item).collect();
        (total, items)
    }

    pub fn take_count(&self, item: Item) -> i32 {
        self.api.CountTakes().map(|f| unsafe { f(item.0) }).unwrap_or(0)
    }

    pub fn take_at(&self, item: Item, index: i32) -> Option<Take> {
        let take = unsafe { self.api.GetMediaItemTake()?(item.0, index) };
        (!take.is_null()).then_some(Take(take))
    }

    pub fn active_take(&self, item: Item) -> Option<Take> {
        let take = unsafe { self.api.GetActiveTake()?(item.0) };
        (!take.is_null()).then_some(Take(take))
    }

    pub fn take_item(&self, take: Take) -> Option<Item> {
        let item = unsafe { self.api.GetMediaItemTake_Item()?(take.0) };
        (!item.is_null()).then_some(Item(item))
    }

    pub fn take_guid(&self, take: Take) -> String {
        let f = self.api.GetSetMediaItemTakeInfo_String();
        f.and_then(|f| big_string(|buf| unsafe { f(take.0, c"GUID".as_ptr(), buf, false) })).unwrap_or_default()
    }

    /// REAPER has no lookup by GUID except for takes.
    pub fn take_by_guid(&self, proj: Project, guid: &str) -> Option<Take> {
        let (parse, find) = (self.api.stringToGuid()?, self.api.GetMediaItemTakeByGUID()?);
        let Some(text) = ctext(guid) else { return None };
        let mut bytes = [0u8; 16];
        unsafe { parse(text.as_ptr(), bytes.as_mut_ptr().cast()) };
        let take = unsafe { find(proj.0, bytes.as_ptr().cast()) };
        (!take.is_null()).then_some(Take(take))
    }

    pub fn take_name(&self, take: Take) -> String {
        match self.api.GetTakeName() {
            Some(f) => unsafe { api::string(f(take.0)) },
            None => String::new(),
        }
    }

    pub fn take_value(&self, take: Take, parm: &str) -> f64 {
        let Some(parm) = ctext(parm) else { return 0.0 };
        self.api.GetMediaItemTakeInfo_Value().map(|f| unsafe { f(take.0, parm.as_ptr()) }).unwrap_or(0.0)
    }

    /// The take's source type as REAPER names it ("WAVE", "MIDI", "MP3"…); None when it has no source.
    pub fn take_source_type(&self, take: Take) -> Option<String> {
        let source = unsafe { self.api.GetMediaItemTake_Source()?(take.0) };
        if source.is_null() {
            return None;
        }
        let f = self.api.GetMediaSourceType()?;
        let name = api::read_buffer(|buf, size| unsafe { f(source, buf, size) });
        (!name.is_empty()).then_some(name)
    }

    pub fn take_is_midi(&self, take: Take) -> bool {
        // (a pooled one's source is MIDIPOOL: lab, 7.82)
        self.take_source_type(take).is_some_and(|t| t.eq_ignore_ascii_case("MIDI") || t.eq_ignore_ascii_case("MIDIPOOL"))
    }

    // ----- FX -----

    /// The FX in a chain (a container's children aren't counted here).
    pub fn fx_count(&self, owner: FxOwner) -> i32 {
        match owner {
            FxOwner::Track(t) => self.api.TrackFX_GetCount().map(|f| unsafe { f(t.0) }),
            FxOwner::Take(t) => self.api.TakeFX_GetCount().map(|f| unsafe { f(t.0) }),
        }
        .unwrap_or(0)
    }

    /// The FX in a track's input chain (the master's monitoring FX).
    pub fn input_fx_count(&self, track: Track) -> i32 {
        self.api.TrackFX_GetRecCount().map(|f| unsafe { f(track.0) }).unwrap_or(0)
    }

    fn fx_guid_pointer(&self, fx: Fx) -> *mut api::GUID {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetFXGUID().map(|f| unsafe { f(t.0, fx.index) }),
            FxOwner::Take(t) => self.api.TakeFX_GetFXGUID().map(|f| unsafe { f(t.0, fx.index) }),
        }
        .unwrap_or(ptr::null_mut())
    }

    /// The FX's GUID, which keeps its identity across reorders; empty when the address holds no FX.
    pub fn fx_guid(&self, fx: Fx) -> String {
        let guid = self.fx_guid_pointer(fx);
        let Some(to_string) = self.api.guidToString() else { return String::new() };
        if guid.is_null() {
            return String::new();
        }
        let mut buf = [0 as c_char; 64];
        unsafe { to_string(guid, buf.as_mut_ptr()) };
        buf[63] = 0;
        unsafe { api::string(buf.as_ptr()) }
    }

    pub fn fx_name(&self, fx: Fx) -> String {
        let name = match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetFXName().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, buf, size) })),
            FxOwner::Take(t) => self.api.TakeFX_GetFXName().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, buf, size) })),
        };
        name.unwrap_or_default()
    }

    pub fn fx_enabled(&self, fx: Fx) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetEnabled().is_some_and(|f| unsafe { f(t.0, fx.index) }),
            FxOwner::Take(t) => self.api.TakeFX_GetEnabled().is_some_and(|f| unsafe { f(t.0, fx.index) }),
        }
    }

    pub fn fx_offline(&self, fx: Fx) -> bool {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetOffline().is_some_and(|f| unsafe { f(t.0, fx.index) }),
            FxOwner::Take(t) => self.api.TakeFX_GetOffline().is_some_and(|f| unsafe { f(t.0, fx.index) }),
        }
    }

    /// The preset's name; empty when the FX has none loaded.
    pub fn fx_preset(&self, fx: Fx) -> String {
        let preset = match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetPreset().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, buf, size) })),
            FxOwner::Take(t) => self.api.TakeFX_GetPreset().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, buf, size) })),
        };
        let preset = preset.unwrap_or_default();
        // REAPER retains the pathname after reading Kumi's short-lived checked copy. It is an
        // implementation detail, not a user-selectable preset path, in reads and change history.
        let mut parts = preset.rsplit(['/', '\\']);
        let name = parts.next().unwrap_or_default();
        if parts.next().is_some_and(|parent| parent.starts_with("kumi-vst3-") && parent.len() > "kumi-vst3-".len())
            && name.to_ascii_lowercase().ends_with(".vstpreset")
        {
            return name.to_string();
        }
        preset
    }

    pub fn fx_param_count(&self, fx: Fx) -> i32 {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetNumParams().map(|f| unsafe { f(t.0, fx.index) }),
            FxOwner::Take(t) => self.api.TakeFX_GetNumParams().map(|f| unsafe { f(t.0, fx.index) }),
        }
        .unwrap_or(0)
    }

    /// A named configuration value (`parallel`, `container_count`, `fx_type`…); None when this FX has no such key.
    pub fn fx_config(&self, fx: Fx, key: &str) -> Option<String> {
        let Some(key) = ctext(key) else { return None };
        match fx.owner {
            FxOwner::Track(t) => {
                let f = self.api.TrackFX_GetNamedConfigParm()?;
                big_string(|buf| unsafe { f(t.0, fx.index, key.as_ptr(), buf, BIG as c_int) })
            }
            FxOwner::Take(t) => {
                let f = self.api.TakeFX_GetNamedConfigParm()?;
                big_string(|buf| unsafe { f(t.0, fx.index, key.as_ptr(), buf, BIG as c_int) })
            }
        }
    }

    /// A container's children, in order; empty for an FX that isn't a container.
    pub fn fx_children(&self, fx: Fx) -> Vec<Fx> {
        let count = self.fx_config(fx, "container_count").and_then(|c| c.trim().parse::<i32>().ok()).unwrap_or(0);
        (0..count)
            .filter_map(|i| self.fx_config(fx, &format!("container_item.{i}")))
            .filter_map(|index| index.trim().parse::<i32>().ok())
            .map(|index| Fx { owner: fx.owner, index })
            .collect()
    }

    pub fn param_name(&self, fx: Fx, param: i32) -> String {
        let name = match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetParamName().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, param, buf, size) })),
            FxOwner::Take(t) => self.api.TakeFX_GetParamName().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, param, buf, size) })),
        };
        name.unwrap_or_default()
    }

    /// The parameter's value on its 0 to 1 scale.
    pub fn param_normalized(&self, fx: Fx, param: i32) -> f64 {
        match fx.owner {
            FxOwner::Track(t) => self.api.TrackFX_GetParamNormalized().map(|f| unsafe { f(t.0, fx.index, param) }),
            FxOwner::Take(t) => self.api.TakeFX_GetParamNormalized().map(|f| unsafe { f(t.0, fx.index, param) }),
        }
        .unwrap_or(0.0)
    }

    /// The value as the plug-in prints it ("-6.0 dB").
    pub fn param_formatted(&self, fx: Fx, param: i32) -> String {
        let text = match fx.owner {
            FxOwner::Track(t) => {
                self.api.TrackFX_GetFormattedParamValue().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, param, buf, size) }))
            }
            FxOwner::Take(t) => {
                self.api.TakeFX_GetFormattedParamValue().map(|f| text(|buf, size| unsafe { f(t.0, fx.index, param, buf, size) }))
            }
        };
        text.unwrap_or_default()
    }

    /// The parameter's own range.
    pub fn param_range(&self, fx: Fx, param: i32) -> Option<(f64, f64)> {
        let (mut min, mut max, mut mid) = (0.0, 0.0, 0.0);
        match fx.owner {
            FxOwner::Track(t) => unsafe { self.api.TrackFX_GetParamEx()?(t.0, fx.index, param, &mut min, &mut max, &mut mid) },
            FxOwner::Take(t) => unsafe { self.api.TakeFX_GetParamEx()?(t.0, fx.index, param, &mut min, &mut max, &mut mid) },
        };
        Some((min, max))
    }

    // ----- sends, receives and hardware outputs -----

    /// How many: `category` -1 receives, 0 sends, 1 hardware outputs.
    pub fn send_count(&self, track: Track, category: i32) -> i32 {
        self.api.GetTrackNumSends().map(|f| unsafe { f(track.0, category) }).unwrap_or(0)
    }

    pub fn send_value(&self, track: Track, category: i32, index: i32, parm: &str) -> f64 {
        let Some(parm) = ctext(parm) else { return 0.0 };
        self.api.GetTrackSendInfo_Value().map(|f| unsafe { f(track.0, category, index, parm.as_ptr()) }).unwrap_or(0.0)
    }

    /// A send's own envelope of this kind (`<VOLENV`, `<PANENV`, `<MUTEENV`), if it has one.
    pub fn send_envelope(&self, track: Track, index: i32, chunk: &str) -> Option<Envelope> {
        let Some(parm) = ctext(&format!("P_ENV:{chunk}")) else { return None };
        let envelope = unsafe { self.api.GetSetTrackSendInfo()?(track.0, 0, index, parm.as_ptr(), ptr::null_mut()) };
        (!envelope.is_null()).then_some(Envelope(envelope.cast()))
    }

    /// The track on the other end: `P_DESTTRACK` of a send, `P_SRCTRACK` of a receive.
    pub fn send_peer(&self, track: Track, category: i32, index: i32) -> Option<Track> {
        let parm = if category < 0 { c"P_SRCTRACK" } else { c"P_DESTTRACK" };
        let peer = unsafe { self.api.GetSetTrackSendInfo()?(track.0, category, index, parm.as_ptr(), ptr::null_mut()) };
        (!peer.is_null()).then_some(Track(peer.cast()))
    }

    // ----- envelopes -----

    pub fn track_envelope_count(&self, track: Track) -> i32 {
        self.api.CountTrackEnvelopes().map(|f| unsafe { f(track.0) }).unwrap_or(0)
    }

    pub fn track_envelope(&self, track: Track, index: i32) -> Option<Envelope> {
        let envelope = unsafe { self.api.GetTrackEnvelope()?(track.0, index) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    /// A track's envelope by its chunk name (`<VOLENV2`, `<PANENV`…) or its GUID.
    pub fn track_envelope_by_chunk_name(&self, track: Track, name: &str) -> Option<Envelope> {
        let Some(name) = ctext(name) else { return None };
        let envelope = unsafe { self.api.GetTrackEnvelopeByChunkName()?(track.0, name.as_ptr()) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    /// An FX parameter's envelope, if it has one (nothing is created).
    pub fn fx_param_envelope(&self, track: Track, fx_index: i32, param: i32) -> Option<Envelope> {
        let envelope = unsafe { self.api.GetFXEnvelope()?(track.0, fx_index, param, false) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    pub fn take_envelope_count(&self, take: Take) -> i32 {
        self.api.CountTakeEnvelopes().map(|f| unsafe { f(take.0) }).unwrap_or(0)
    }

    pub fn take_envelope(&self, take: Take, index: i32) -> Option<Envelope> {
        let envelope = unsafe { self.api.GetTakeEnvelope()?(take.0, index) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    pub fn envelope_name(&self, envelope: Envelope) -> String {
        match self.api.GetEnvelopeName() {
            Some(f) => text(|buf, size| unsafe { f(envelope.0, buf, size) }),
            None => String::new(),
        }
    }

    /// An envelope's GUID, which stays with it through a save, an undo and the others being added or removed.
    pub fn envelope_guid(&self, envelope: Envelope) -> Option<String> {
        let Some(key) = ctext("GUID") else { return None };
        let f = self.api.GetSetEnvelopeInfo_String()?;
        big_string(|buf| unsafe { f(envelope.0, key.as_ptr(), buf, false) })
            .map(|guid| guid.trim().to_string())
            .filter(|guid| !guid.is_empty())
    }

    /// What this REAPER is too old for: the features the bridge reads with functions it lacks, and the version that has them.
    /// Nothing else says why a project then shows no markers, or why a project loaded over another goes unnoticed.
    pub fn missing_features(&self) -> Vec<(&'static str, &'static str)> {
        let mut missing = Vec::new();
        if self.api.GetRegionOrMarker().is_none() || self.api.GetNumRegionsOrMarkers().is_none() {
            missing.push(("markers and regions", "7.62"));
        }
        if self.api.Undo_GetNumEntries().is_none() {
            missing.push(("noticing that a project was loaded over another in its tab, without REAPER's load signal", "7.79"));
        }
        missing
    }

    /// The track an envelope is on and, for an FX parameter's envelope, the FX (as `TrackFX_` addresses it) and the
    /// parameter; -1 and -1 for any other.
    pub fn envelope_owner(&self, envelope: Envelope) -> Option<(Track, i32, i32)> {
        let f = self.api.Envelope_GetParentTrack()?;
        let (mut fx, mut param) = (-1, -1);
        let track = unsafe { f(envelope.0, &mut fx, &mut param) };
        (!track.is_null()).then_some((Track(track.cast()), fx, param))
    }

    pub fn envelope_point_count(&self, envelope: Envelope) -> i32 {
        self.api.CountEnvelopePoints().map(|f| unsafe { f(envelope.0) }).unwrap_or(0)
    }

    /// An envelope's yes/no attribute: `ACTIVE`, `ARM` or `VISIBLE`.
    pub fn envelope_flag(&self, envelope: Envelope, key: &str) -> bool {
        let Some(key) = ctext(key) else { return false };
        let f = self.api.GetSetEnvelopeInfo_String();
        f.and_then(|f| big_string(|buf| unsafe { f(envelope.0, key.as_ptr(), buf, false) })).is_some_and(|text| text.trim() == "1")
    }

    // ----- MIDI -----

    /// A take's note, CC and text/sysex event counts.
    /// A MIDI take's notes, CC events and text/sysex events. (REAPER answers their total: 0 for a take with no MIDI in it
    /// yet, which isn't a failure. Whether a take is MIDI at all is `take_is_midi`.)
    pub fn midi_counts(&self, take: Take) -> Option<(i32, i32, i32)> {
        let f = self.api.MIDI_CountEvts()?;
        let (mut notes, mut ccs, mut texts) = (0, 0, 0);
        unsafe { f(take.0, &mut notes, &mut ccs, &mut texts) };
        Some((notes, ccs, texts))
    }

    pub fn midi_note(&self, take: Take, index: i32) -> Option<MidiNote> {
        let f = self.api.MIDI_GetNote()?;
        let (mut selected, mut muted, mut start_ppq, mut end_ppq, mut channel, mut pitch, mut velocity) = (false, false, 0.0, 0.0, 0, 0, 0);
        let ok =
            unsafe { f(take.0, index, &mut selected, &mut muted, &mut start_ppq, &mut end_ppq, &mut channel, &mut pitch, &mut velocity) };
        ok.then_some(MidiNote { selected, muted, start_ppq, end_ppq, channel, pitch, velocity })
    }

    pub fn midi_cc(&self, take: Take, index: i32) -> Option<MidiCc> {
        let f = self.api.MIDI_GetCC()?;
        let (mut selected, mut muted, mut ppq, mut kind, mut channel, mut data1, mut data2) = (false, false, 0.0, 0, 0, 0, 0);
        let ok = unsafe { f(take.0, index, &mut selected, &mut muted, &mut ppq, &mut kind, &mut channel, &mut data1, &mut data2) };
        ok.then_some(MidiCc { selected, muted, ppq, kind, channel, data1, data2 })
    }

    /// Quarter notes from the project start as a position in the take's MIDI ticks.
    pub fn quarter_notes_to_ppq(&self, take: Take, qn: f64) -> Option<f64> {
        self.api.MIDI_GetPPQPosFromProjQN().map(|f| unsafe { f(take.0, qn) })
    }

    /// A position in the take's MIDI ticks as quarter notes from the project start.
    pub fn ppq_to_quarter_notes(&self, take: Take, ppq: f64) -> Option<f64> {
        self.api.MIDI_GetProjQNFromPPQPos().map(|f| unsafe { f(take.0, ppq) })
    }

    // ----- focus -----

    /// The take of the MIDI editor that has the keyboard focus, if one does.
    pub fn focused_midi_take(&self) -> Option<Take> {
        let editor = unsafe { self.api.MIDIEditor_GetActive()?() };
        if editor.is_null() {
            return None;
        }
        let take = unsafe { self.api.MIDIEditor_GetTake()?(editor) };
        (!take.is_null()).then_some(Take(take))
    }

    /// Whether a MIDI editor has the focus, with or without a take.
    pub fn midi_editor_focused(&self) -> bool {
        self.api.MIDIEditor_GetActive().is_some_and(|f| !unsafe { f() }.is_null())
    }

    /// The last touched parameter's FX (`mode` 0) or the focused FX (`mode` 1).
    pub fn touched_or_focused_fx(&self, mode: i32) -> Option<FxHit> {
        let f = self.api.GetTouchedOrFocusedFX()?;
        let (mut track, mut item, mut take, mut fx, mut param) = (0, 0, 0, 0, 0);
        let ok = unsafe { f(mode, &mut track, &mut item, &mut take, &mut fx, &mut param) };
        ok.then_some(FxHit { track, item, take, fx, param })
    }

    /// The envelope selected in the arrange view.
    pub fn selected_envelope(&self, proj: Project) -> Option<Envelope> {
        let envelope = unsafe { self.api.GetSelectedEnvelope()?(proj.0) };
        (!envelope.is_null()).then_some(Envelope(envelope))
    }

    /// 0 when the keyboard acts on tracks, 1 on items, 2 on envelopes.
    pub fn cursor_context(&self) -> Option<i32> {
        self.api.GetCursorContext2().map(|f| unsafe { f(true) })
    }

    /// A colour in the OS's native order as `#rrggbb`.
    pub fn color_hex(&self, native: i32) -> Option<String> {
        let f = self.api.ColorFromNative()?;
        let (mut r, mut g, mut b) = (0, 0, 0);
        unsafe { f(native & !COLOR_SET, &mut r, &mut g, &mut b) };
        Some(format!("#{:02x}{:02x}{:02x}", r & 255, g & 255, b & 255))
    }
}
