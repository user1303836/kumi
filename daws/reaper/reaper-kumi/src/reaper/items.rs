//! Items, takes and what is in them (M4): what the bridge reads and writes of them, one call each, as REAPER's own. A
//! function this REAPER lacks reads as empty and writes as false, and the change that needed it is refused before it
//! begins.

use std::ffi::{c_char, c_int, CString};
use std::ptr;

use super::{Item, Project, Reaper, Take, Track};
use crate::api;

/// The most an item's chunk may grow to when it is read: a MIDI take's events and the take FX's states take the room.
const MAX_CHUNK: usize = 256 << 20;

fn cstring(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

/// A stretch marker as REAPER keeps it: where it sits in the item (seconds from the item's start), the source position it
/// pins there (seconds), and the rate's slope to the next one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StretchMarker {
    pub position: f64,
    pub source: f64,
    pub slope: f64,
}

/// A take marker: a source position (seconds), a name and a colour (REAPER's native one with 0x1000000, 0 for the default).
#[derive(Debug, Clone, PartialEq)]
pub struct TakeMarker {
    pub source: f64,
    pub name: String,
    pub color: i32,
}

/// A media file as REAPER reads it: its kind ("WAVE", "MIDI"…) and length (quarter notes for MIDI, else seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub kind: String,
    pub length: f64,
    pub length_is_beats: bool,
}

impl Reaper<'_> {
    // ----- items -----

    pub fn set_item_value(&self, item: Item, parm: &str, value: f64) -> bool {
        let parm = cstring(parm);
        self.api.SetMediaItemInfo_Value().is_some_and(|f| unsafe { f(item.0, parm.as_ptr(), value) })
    }

    /// An empty MIDI item on `track` from `start` to `end` (seconds). REAPER's undo doesn't notice an item this call makes
    /// (lab 7.82: a block with only it makes no point, and REAPER's undo then takes the step before): the track's items are
    /// marked dirty, and REAPER keeps a point of it.
    pub fn create_midi_item(&self, track: Track, start: f64, end: f64) -> Option<Item> {
        let in_beats = false;
        let item = unsafe { self.api.CreateNewMIDIItemInProj()?(track.0, start, end, &in_beats) };
        if item.is_null() {
            return None;
        }
        if let Some(mark) = self.api.MarkTrackItemsDirty() {
            unsafe { mark(track.0, item) };
        }
        Some(Item(item))
    }

    /// A new empty item (no take) at the end of the track's list.
    pub fn add_item(&self, track: Track) -> Option<Item> {
        let item = unsafe { self.api.AddMediaItemToTrack()?(track.0) };
        (!item.is_null()).then_some(Item(item))
    }

    pub fn delete_item(&self, track: Track, item: Item) -> bool {
        self.api.DeleteTrackMediaItem().is_some_and(|f| unsafe { f(track.0, item.0) })
    }

    pub fn move_item_to_track(&self, item: Item, track: Track) -> bool {
        self.api.MoveMediaItemToTrack().is_some_and(|f| unsafe { f(item.0, track.0) })
    }

    /// Splits the item at `position` (seconds): the item keeps the left part, and the right part is a new item.
    pub fn split_item(&self, item: Item, position: f64) -> Option<Item> {
        let right = unsafe { self.api.SplitMediaItem()?(item.0, position) };
        (!right.is_null()).then_some(Item(right))
    }

    /// The item's whole state as REAPER writes it into a project file: its takes, their sources (a MIDI take's events),
    /// take FX, markers and GUIDs.
    pub fn item_chunk(&self, item: Item) -> Option<String> {
        let f = self.api.GetItemStateChunk()?;
        let mut size = 1usize << 16;
        loop {
            let mut buf = vec![0u8; size];
            let ok = unsafe { f(item.0, buf.as_mut_ptr().cast::<c_char>(), size as c_int, false) };
            let len = buf.iter().position(|&b| b == 0).unwrap_or(size);
            if !ok {
                return None;
            }
            if len + 1 < size {
                buf.truncate(len);
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
            if size >= MAX_CHUNK {
                return None;
            }
            size *= 4;
        }
    }

    pub fn set_item_chunk(&self, item: Item, chunk: &str) -> bool {
        let chunk = cstring(chunk);
        self.api.SetItemStateChunk().is_some_and(|f| unsafe { f(item.0, chunk.as_ptr(), false) })
    }

    /// Tells REAPER an item changed by script (its place among the track's items, its drawing).
    pub fn update_item(&self, item: Item) {
        if let Some(f) = self.api.UpdateItemInProject() {
            unsafe { f(item.0) }
        }
    }

    /// Lays out the fixed lanes again after items changed lanes.
    pub fn update_item_lanes(&self, proj: Project) {
        if let Some(f) = self.api.UpdateItemLanes() {
            unsafe { f(proj.0) };
        }
    }

    /// Redraws the timeline (after a track's lanes are turned on or off, as the API asks).
    pub fn update_timeline(&self) {
        if let Some(f) = self.api.UpdateTimeline() {
            unsafe { f() }
        }
    }

    pub fn set_item_selected(&self, item: Item, selected: bool) {
        if let Some(f) = self.api.SetMediaItemSelected() {
            unsafe { f(item.0, selected) }
        }
    }

    pub fn select_all_items(&self, proj: Project, selected: bool) {
        if let Some(f) = self.api.SelectAllMediaItems() {
            unsafe { f(proj.0, selected) }
        }
    }

    // ----- takes -----

    /// A new empty take in the item (REAPER leaves the active take as it was, unless the item had none).
    pub fn add_take(&self, item: Item) -> Option<Take> {
        let take = unsafe { self.api.AddTakeToMediaItem()?(item.0) };
        (!take.is_null()).then_some(Take(take))
    }

    pub fn set_active_take(&self, take: Take) -> bool {
        match self.api.SetActiveTake() {
            Some(f) => {
                unsafe { f(take.0) };
                true
            }
            None => false,
        }
    }

    pub fn set_take_value(&self, take: Take, parm: &str, value: f64) -> bool {
        let parm = cstring(parm);
        self.api.SetMediaItemTakeInfo_Value().is_some_and(|f| unsafe { f(take.0, parm.as_ptr(), value) })
    }

    /// A take's text attribute (`P_NAME`).
    pub fn set_take_string(&self, take: Take, parm: &str, value: &str) -> bool {
        let (parm, value) = (cstring(parm), cstring(value));
        let Some(f) = self.api.GetSetMediaItemTakeInfo_String() else { return false };
        let mut buf = value.into_bytes_with_nul();
        unsafe { f(take.0, parm.as_ptr(), buf.as_mut_ptr().cast::<c_char>(), true) }
    }

    /// What REAPER reads of a media file, without putting it anywhere: None when it reads nothing (a missing or
    /// unreadable file reads as an empty source of length 0).
    pub fn media(&self, path: &str) -> Option<Media> {
        let (create, destroy) = (self.api.PCM_Source_CreateFromFile()?, self.api.PCM_Source_Destroy()?);
        let path = cstring(path);
        let source = unsafe { create(path.as_ptr()) };
        if source.is_null() {
            return None;
        }
        let media = self.source_media(source);
        unsafe { destroy(source) };
        media.filter(|media| media.length > 0.0)
    }

    fn source_media(&self, source: *mut api::PCM_source) -> Option<Media> {
        let mut in_beats = false;
        let length = unsafe { self.api.GetMediaSourceLength()?(source, &mut in_beats) };
        let kind = match self.api.GetMediaSourceType() {
            Some(f) => api::read_buffer(|buf, size| unsafe { f(source, buf, size) }),
            None => String::new(),
        };
        Some(Media { kind, length, length_is_beats: in_beats })
    }

    /// Gives the take the media file at `path` as its source, in place (nothing is copied), and frees the source it had.
    pub fn set_take_file(&self, take: Take, path: &str) -> Option<Media> {
        let (create, destroy, set) =
            (self.api.PCM_Source_CreateFromFile()?, self.api.PCM_Source_Destroy()?, self.api.GetSetMediaItemTakeInfo()?);
        let path = cstring(path);
        let source = unsafe { create(path.as_ptr()) };
        if source.is_null() {
            return None;
        }
        let Some(media) = self.source_media(source).filter(|media| media.length > 0.0) else {
            unsafe { destroy(source) };
            return None;
        };
        // REAPER's own order: the old source is read, the new one set, then the old one freed.
        let old = self.api.GetMediaItemTake_Source().map_or(ptr::null_mut(), |f| unsafe { f(take.0) });
        unsafe { set(take.0, c"P_SOURCE".as_ptr(), source.cast()) };
        if !old.is_null() && old != source {
            unsafe { destroy(old) };
        }
        Some(media)
    }

    /// The file a take plays, if its source is one (an in-project MIDI take has none).
    pub fn take_file(&self, take: Take) -> Option<String> {
        let source = unsafe { self.api.GetMediaItemTake_Source()?(take.0) };
        if source.is_null() {
            return None;
        }
        let f = self.api.GetMediaSourceFileName()?;
        let name = api::read_buffer(|buf, size| unsafe { f(source, buf, size) });
        (!name.is_empty()).then_some(name)
    }

    /// A string that changes only when the take's MIDI does (`notes_only`: only its notes).
    pub fn midi_hash(&self, take: Take, notes_only: bool) -> Option<String> {
        let f = self.api.MIDI_GetHash()?;
        let mut ok = false;
        let hash = api::read_buffer(|buf, size| ok = unsafe { f(take.0, notes_only, buf, size) });
        ok.then_some(hash)
    }

    // ----- stretch markers -----

    pub fn stretch_markers(&self, take: Take) -> Vec<StretchMarker> {
        let (Some(count), Some(get)) = (self.api.GetTakeNumStretchMarkers(), self.api.GetTakeStretchMarker()) else { return vec![] };
        let slope = self.api.GetTakeStretchMarkerSlope();
        let n = unsafe { count(take.0) }.max(0);
        (0..n)
            .filter_map(|index| {
                let (mut position, mut source) = (0.0, 0.0);
                let found = unsafe { get(take.0, index, &mut position, &mut source) };
                let slope = slope.map_or(0.0, |f| unsafe { f(take.0, index) });
                (found >= 0).then_some(StretchMarker { position, source, slope })
            })
            .collect()
    }

    /// Adds a stretch marker (index -1) or moves one; REAPER keeps one between its neighbours. Its index, or -1 when none
    /// was made (one is at that position already).
    pub fn set_stretch_marker(&self, take: Take, index: i32, position: f64, source: Option<f64>) -> i32 {
        let Some(f) = self.api.SetTakeStretchMarker() else { return -1 };
        match source {
            Some(source) => unsafe { f(take.0, index, position, &source) },
            None => unsafe { f(take.0, index, position, ptr::null()) },
        }
    }

    pub fn set_stretch_slope(&self, take: Take, index: i32, slope: f64) -> bool {
        self.api.SetTakeStretchMarkerSlope().is_some_and(|f| unsafe { f(take.0, index, slope) })
    }

    /// Deletes `count` stretch markers from `index`; how many went.
    pub fn delete_stretch_markers(&self, take: Take, index: i32, count: i32) -> i32 {
        self.api.DeleteTakeStretchMarkers().map_or(0, |f| unsafe { f(take.0, index, &count) })
    }

    // ----- take markers -----

    pub fn take_markers(&self, take: Take) -> Vec<TakeMarker> {
        let (Some(count), Some(get)) = (self.api.GetNumTakeMarkers(), self.api.GetTakeMarker()) else { return vec![] };
        let n = unsafe { count(take.0) }.max(0);
        (0..n)
            .filter_map(|index| {
                let mut color = 0;
                let mut source = -1.0;
                let name = api::read_buffer(|buf, size| source = unsafe { get(take.0, index, buf, size, &mut color) });
                (source >= 0.0).then_some(TakeMarker { source, name, color })
            })
            .collect()
    }

    /// Adds a take marker (index -1) or changes one; what isn't given stays. Its index after (REAPER keeps them in source
    /// order), or -1.
    pub fn set_take_marker(&self, take: Take, index: i32, name: &str, source: Option<f64>, color: Option<i32>) -> i32 {
        let Some(f) = self.api.SetTakeMarker() else { return -1 };
        let name = cstring(name);
        let mut source = source;
        let mut color = color;
        let source_ptr = source.as_mut().map_or(ptr::null_mut(), |s| s as *mut f64);
        let color_ptr = color.as_mut().map_or(ptr::null_mut(), |c| c as *mut c_int);
        unsafe { f(take.0, index, name.as_ptr(), source_ptr, color_ptr) }
    }

    pub fn delete_take_marker(&self, take: Take, index: i32) -> bool {
        self.api.DeleteTakeMarker().is_some_and(|f| unsafe { f(take.0, index) })
    }
}
