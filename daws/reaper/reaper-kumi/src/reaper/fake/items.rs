//! The fake REAPER's items and takes (M4): what the bridge writes to them, their chunks, stretch and take markers, media
//! sources, the MIDI buffer of every kind of event, fixed lanes and razor areas, and the item actions Kumi runs (glue,
//! render to a take, delete the active take, comping). Its table is looked up before the fake's own: a function here
//! stands in for the reads-only one there and hands on what it doesn't know, as REAPER 7.82 behaves in the lab.

use super::*;
use crate::midi::Event;

/// What a fake take holds besides what the reads used before M4.
#[derive(Clone, Default, PartialEq)]
pub struct TakeMore {
    /// Stretch markers: position in the item (seconds), source position, slope; in position order.
    pub stretch: Vec<(f64, f64, f64)>,
    /// Take markers: source position, name, colour; in source order.
    pub markers: Vec<(f64, String, i32)>,
    /// The media file a take plays, and its length (seconds, or quarter notes for MIDI).
    pub file: Option<String>,
    pub length: f64,
    /// Every MIDI event of the take as REAPER's buffer holds it (notes, controllers, sysex, text, the end marker), when the
    /// test gives one; else the take's notes and controllers are its MIDI.
    pub raw: Option<Vec<Event>>,
}

/// A media source made from a file, not yet a take's.
#[derive(Clone)]
pub struct NewSource {
    pub path: String,
    pub kind: String,
    pub length: f64,
    pub beats: bool,
    pub freed: bool,
}

/// The fake REAPER's state for items: what `fake::State` keeps of them.
#[derive(Clone, Default)]
pub struct ItemsState {
    /// Item states handed out by `GetItemStateChunk`, by the number in the chunk text.
    pub chunks: Vec<FakeItem>,
    pub sources: Vec<NewSource>,
    /// Files REAPER can read as media, by path: kind ("WAVE", "MIDI"), length, and whether the length is in beats.
    pub media: HashMap<String, (String, f64, bool)>,
    /// A GUID counter for items, takes and FX made here.
    pub made: u32,
    /// Razor areas by track GUID, as `P_RAZOREDITS_EXT` holds them.
    pub razor: HashMap<String, String>,
    /// The next split fails (REAPER answers no item).
    pub fail_split: bool,
}

const NEW_SOURCE: u64 = 10;
/// The CC that marks a MIDI take's source end: all notes off, channel 1.
const END_MARKER: [u8; 3] = [0xB0, 0x7B, 0x00];

fn new_id(s: &mut State, what: &str) -> String {
    s.items.made += 1;
    format!("{{{what}-NEW-{}}}", s.items.made)
}

fn item_mut(s: &mut State, handle: u64) -> Option<&mut FakeItem> {
    let (kind, a, b, _) = unpack(handle);
    if kind != ITEM {
        return None;
    }
    s.tracks.get_mut(a as usize)?.items.get_mut(b as usize)
}

fn take_handle_of(s: &State, take: &str) -> Option<u64> {
    for (t, track) in s.tracks.iter().enumerate() {
        for (i, item) in track.items.iter().enumerate() {
            if let Some(k) = item.takes.iter().position(|tk| tk.guid == take) {
                return Some(take_ptr(t, i, k) as u64);
            }
        }
    }
    None
}

/// A track's fixed lanes: whether it has them, how many, and how each plays (0 not, 1 alone, 2 with others).
/// Whether lanes are picked to play (REAPER's `LANESOLO` line, here any `C_LANEPLAYS:n` kept): without, every lane plays
/// and `C_ALLLANESPLAY` reads 1 (lab, 7.82).
fn picked(t: &FakeTrack) -> bool {
    t.extra.keys().any(|key| key.starts_with("C_LANEPLAYS:"))
}

fn lanes_of(t: &FakeTrack) -> (bool, usize, Vec<i32>) {
    let fixed = t.extra.get("I_FREEMODE").copied().unwrap_or(0.0) as i32 == 2;
    let count = t.extra.get("I_NUMFIXEDLANES").copied().unwrap_or(1.0).max(1.0) as usize;
    let plays = if picked(t) {
        (0..count).map(|n| t.extra.get(&format!("C_LANEPLAYS:{n}")).copied().unwrap_or(2.0) as i32).collect()
    } else {
        vec![if count == 1 { 1 } else { 2 }; count]
    };
    (fixed, count, plays)
}

fn set_plays(t: &mut FakeTrack, plays: &[i32]) {
    for (n, v) in plays.iter().enumerate() {
        t.extra.insert(format!("C_LANEPLAYS:{n}"), *v as f64);
    }
}

fn unpick(t: &mut FakeTrack) {
    t.extra.retain(|key, _| !key.starts_with("C_LANEPLAYS:"));
}

// ----- items -----

pub(super) unsafe extern "C" fn get_media_item_info_value(item: *mut MediaItem, parm: *const c_char) -> f64 {
    let parm = name_of(parm);
    let handle = item as u64;
    let own = counted(|s| {
        let i = item_of(s, handle)?;
        if let Some(v) = i.extra.get(&parm) {
            return Some(*v);
        }
        Some(match parm.as_str() {
            "D_VOL" => 1.0,
            "C_BEATATTACHMODE" => -1.0,
            "C_LANEPLAYS" => {
                let track = track_of(s, pack(TRACK, unpack(handle).1, 0, 0))?;
                let (fixed, _, plays) = lanes_of(track);
                if !fixed {
                    return Some(0.0);
                }
                plays.get(i.lane.max(0) as usize).copied().unwrap_or(0) as f64
            }
            key if key.starts_with("D_FADE") || key.starts_with("C_FADE") => 0.0,
            "B_LOOPSRC" | "C_LOCK" | "I_GROUPID" | "D_SNAPOFFSET" => 0.0,
            _ => return None,
        })
    });
    own.unwrap_or_else(|| unsafe { super::get_media_item_info_value(item, c_name(&parm).as_ptr()) })
}

fn c_name(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

/// A fade's curve keys follow each other as REAPER 7.82's do: a shape sets its curve and S values, the legacy curve makes
/// a shape of its own (-1), and the 7.81 curve and S values make the shape the nearest one.
fn set_fade(i: &mut FakeItem, side: &str, key: &str, value: f64) {
    let shape_key = format!("C_FADE{side}SHAPE");
    let dir_key = format!("D_FADE{side}DIR");
    let new_key = format!("D_FADE{side}DIR_NEW");
    let new2_key = format!("D_FADE{side}DIR2_NEW");
    let get = |i: &FakeItem, key: &str| i.extra.get(key).copied().unwrap_or(0.0);
    let classify = |new: f64, new2: f64| -> f64 {
        if new2 != 0.0 {
            5.0
        } else if new > 0.0 {
            1.0
        } else if new < 0.0 {
            2.0
        } else {
            0.0
        }
    };
    let derived_dir = |new: f64, new2: f64| -> f64 {
        if new2 == 0.0 {
            (2.0 * new).clamp(-1.0, 1.0)
        } else {
            -(new + new2) / 2.0
        }
    };
    if key == shape_key {
        let canonical =
            [(0.0, 0.0, 0.0), (0.0, 0.5, 0.0), (-1.0, -0.5, 0.0), (1.0, 1.0, 0.0), (-1.0, -1.0, 0.0), (0.0, 0.0, 0.5), (0.0, 0.0, 1.0)];
        let (dir, new, new2) = canonical.get(value as usize).copied().unwrap_or((0.0, 0.0, 0.0));
        i.extra.insert(shape_key, value);
        i.extra.insert(dir_key, dir);
        i.extra.insert(new_key, new);
        i.extra.insert(new2_key, new2);
    } else if key == dir_key {
        i.extra.insert(shape_key, -1.0);
        i.extra.insert(dir_key, value);
        i.extra.insert(new_key, value * 0.78);
        i.extra.insert(new2_key, 0.0);
    } else if key == new_key {
        let new2 = get(i, &new2_key);
        i.extra.insert(new_key, value);
        i.extra.insert(shape_key, classify(value, new2));
        i.extra.insert(dir_key, derived_dir(value, new2));
    } else if key == new2_key {
        let new = get(i, &new_key);
        i.extra.insert(new2_key, value);
        i.extra.insert(shape_key, classify(new, value));
        i.extra.insert(dir_key, derived_dir(new, value));
    } else {
        i.extra.insert(key.to_string(), value);
    }
}

pub(super) unsafe extern "C" fn set_media_item_info_value(item: *mut MediaItem, parm: *const c_char, value: f64) -> bool {
    let parm = name_of(parm);
    counted(|s| {
        let Some(i) = item_mut(s, item as u64) else { return false };
        match parm.as_str() {
            "D_POSITION" => i.position = value,
            "D_LENGTH" => i.length = value.max(0.0),
            "B_MUTE" => i.mute = value != 0.0,
            "I_FIXEDLANE" => i.lane = value.max(0.0) as i32,
            "I_CUSTOMCOLOR" => i.color = value as i32,
            "B_UISEL" => i.selected = value != 0.0,
            "I_CURTAKE" => {
                if (value as usize) < i.takes.len() {
                    i.active = value as usize;
                }
            }
            key if key.starts_with("C_FADEIN") || key.starts_with("D_FADEINDIR") => set_fade(i, "IN", key, value),
            key if key.starts_with("C_FADEOUT") || key.starts_with("D_FADEOUTDIR") => set_fade(i, "OUT", key, value),
            "IP_ITEMNUMBER" | "C_LANEPLAYS" | "P_TRACK" => return false,
            other => {
                i.extra.insert(other.to_string(), value);
            }
        }
        s.undo_dirty = true;
        true
    })
}

pub(super) unsafe extern "C" fn create_new_midi_item_in_proj(
    track: *mut MediaTrack,
    start: f64,
    end: f64,
    qn: *const bool,
) -> *mut MediaItem {
    counted(|s| {
        let in_beats = !qn.is_null() && unsafe { *qn };
        let (start, end) = if in_beats { (start * 60.0 / s.bpm, end * 60.0 / s.bpm) } else { (start, end) };
        let at = unpack(track as u64).1;
        if at == MASTER || track_of(s, track as u64).is_none() || end <= start {
            return ptr::null_mut();
        }
        let (item_id, take_id) = (new_id(s, "ITEM"), new_id(s, "TAKE"));
        let ticks = ((end - start) * s.bpm / 60.0 * 960.0).round() as i64;
        let mut take = FakeTake::midi(&take_id, "");
        take.more.raw = Some(vec![Event { pos: ticks, flag: 0, msg: END_MARKER.to_vec() }]);
        take.more.length = (end - start) * s.bpm / 60.0;
        let t = &mut s.tracks[at as usize];
        t.items.push(FakeItem::new(&item_id, start, end - start, vec![take]));
        // (REAPER's undo doesn't notice an item made by this call: MarkTrackItemsDirty makes it, lab 7.82)
        item_ptr(at as usize, t.items.len() - 1)
    })
}

pub(super) unsafe extern "C" fn mark_track_items_dirty(track: *mut MediaTrack, _item: *mut MediaItem) {
    counted(|s| {
        if track_of(s, track as u64).is_some() {
            s.undo_dirty = true;
        }
    })
}

pub(super) unsafe extern "C" fn add_media_item_to_track(track: *mut MediaTrack) -> *mut MediaItem {
    // As REAPER 7.82's: the new-item fades unless the producer turned them off (splitautoxfade &8), deffadelen long.
    let fades = super::extra(|e| {
        let number = |name: &str| e.config.get(name).and_then(|value| value.trim().parse::<f64>().ok());
        match (number("splitautoxfade"), number("deffadelen")) {
            (Some(flags), Some(length)) if flags as i64 & 8 == 0 => length,
            _ => 0.0,
        }
    });
    counted(|s| {
        let at = unpack(track as u64).1;
        if at == MASTER || track_of(s, track as u64).is_none() {
            return ptr::null_mut();
        }
        let id = new_id(s, "ITEM");
        let t = &mut s.tracks[at as usize];
        let mut item = FakeItem::new(&id, 0.0, 0.0, vec![]);
        // As REAPER 7.82's: an item this makes loops its source whatever the preferences say, and has the new-item fades
        // (m7 KP1 lab).
        item.extra.insert("B_LOOPSRC".into(), 1.0);
        if fades > 0.0 {
            item.extra.insert("D_FADEINLEN".into(), fades);
            item.extra.insert("D_FADEOUTLEN".into(), fades);
        }
        t.items.push(item);
        s.undo_dirty = true;
        item_ptr(at as usize, t.items.len() - 1)
    })
}

pub(super) unsafe extern "C" fn delete_track_media_item(track: *mut MediaTrack, item: *mut MediaItem) -> bool {
    counted(|s| {
        let (_, t, i, _) = unpack(item as u64);
        if item_of(s, item as u64).is_none() || unpack(track as u64).1 != t || s.item_deletes_left == Some(0) {
            return false;
        }
        if let Some(left) = s.item_deletes_left.as_mut() {
            *left -= 1;
        }
        s.tracks[t as usize].items.remove(i as usize);
        s.undo_dirty = true;
        true
    })
}

pub(super) unsafe extern "C" fn move_media_item_to_track(item: *mut MediaItem, track: *mut MediaTrack) -> bool {
    counted(|s| {
        let (_, t, i, _) = unpack(item as u64);
        let dest = unpack(track as u64).1;
        if item_of(s, item as u64).is_none() || dest == MASTER || track_of(s, track as u64).is_none() {
            return false;
        }
        if dest == t {
            return true;
        }
        let moved = s.tracks[t as usize].items.remove(i as usize);
        s.tracks[dest as usize].items.push(moved);
        s.undo_dirty = true;
        true
    })
}

/// REAPER's split: the item keeps the left part with a short fade out, and a new item (new GUIDs) holds the right part with
/// a short fade in, its takes' source offsets moved on; stretch markers go with both (the right's moved), take markers
/// stay with both; a MIDI take's events are cut at the split.
pub(super) unsafe extern "C" fn split_media_item(item: *mut MediaItem, position: f64) -> *mut MediaItem {
    counted(|s| {
        if std::mem::take(&mut s.items.fail_split) {
            return ptr::null_mut();
        }
        let (_, t, i, _) = unpack(item as u64);
        let Some(original) = item_of(s, item as u64).cloned() else { return ptr::null_mut() };
        let offset = position - original.position;
        if offset <= 0.0 || offset >= original.length {
            return ptr::null_mut();
        }
        let bpm = s.bpm;
        let split_tick = (offset * bpm / 60.0 * 960.0).round() as i64;
        let mut right = original.clone();
        right.guid = new_id(s, "ITEM");
        right.position = position;
        right.length = original.length - offset;
        right.selected = false;
        right.extra.insert("D_FADEINLEN".into(), 0.01);
        for take in right.takes.iter_mut() {
            take.guid = new_id(s, "TAKE");
            let rate = take.extra.get("D_PLAYRATE").copied().unwrap_or(1.0);
            let offs = take.extra.get("D_STARTOFFS").copied().unwrap_or(0.0);
            take.extra.insert("D_STARTOFFS".into(), offs + offset * rate);
            for marker in take.more.stretch.iter_mut() {
                marker.0 -= offset;
            }
            for fx in take.fx.iter_mut() {
                fx.guid = format!("{{FX-SPLIT-{}}}", fx.guid.trim_matches(|c| c == '{' || c == '}'));
            }
            if let Some(raw) = take.more.raw.as_mut() {
                let end = raw.last().map_or(0, |e| e.pos);
                *raw = raw
                    .iter()
                    .filter(|e| e.pos >= split_tick && e.msg != END_MARKER)
                    .map(|e| Event { pos: e.pos - split_tick, ..e.clone() })
                    .collect();
                raw.push(Event { pos: end - split_tick, flag: 0, msg: END_MARKER.to_vec() });
            }
        }
        let left = &mut s.tracks[t as usize].items[i as usize];
        left.length = offset;
        left.extra.insert("D_FADEOUTLEN".into(), 0.01);
        for take in left.takes.iter_mut() {
            if let Some(raw) = take.more.raw.as_mut() {
                raw.retain(|e| e.pos < split_tick && e.msg != END_MARKER);
                raw.push(Event { pos: split_tick, flag: 0, msg: END_MARKER.to_vec() });
            }
        }
        s.tracks[t as usize].items.insert(i as usize + 1, right);
        s.undo_dirty = true;
        item_ptr(t as usize, i as usize + 1)
    })
}

/// The chunk of a fake item names the state kept for it (equal states share a number, so a chunk reads the same until
/// the item changes), with the lines a real one has that the bridge reads: the selection, the GUIDs, a MIDI take's pool
/// and the take FX's ids.
pub(super) unsafe extern "C" fn get_item_state_chunk(item: *mut MediaItem, buf: *mut c_char, size: c_int, _isundo: bool) -> bool {
    counted(|s| {
        let Some(i) = item_of(s, item as u64).cloned() else { return false };
        let mut state = i.clone();
        state.selected = false;
        let n = match s.items.chunks.iter().position(|kept| *kept == state) {
            Some(n) => n,
            None => {
                s.items.chunks.push(state);
                s.items.chunks.len() - 1
            }
        };
        let mut text = format!("<ITEM\nFAKEITEM {n}\nSEL {}\nIGUID {}\n", i.selected as i32, i.guid);
        for (k, take) in i.takes.iter().enumerate() {
            if k > 0 {
                text += if k == i.active { "TAKE SEL\n" } else { "TAKE\n" };
            }
            text += &format!("NAME \"{}\"\nGUID {}\n", take.name, take.guid);
            if take.source.as_deref() == Some("MIDI") {
                text += &format!(
                    "<SOURCE MIDI\nPOOLEDEVTS {{POOL-{}}}\nCFGEDITVIEW 0 0\n>\n",
                    take.guid.trim_matches(|c| c == '{' || c == '}')
                );
            }
            // The take's FX as m4-r-fxenv writes their entries (its take FX kinds read them back): one FXID line per FX,
            // and no GUID line inside, so the take GUIDs above stay the only ones.
            let (_, track, at, _) = unpack(item as u64);
            text += &super::fxenv::chain_block(s, take_ptr(track as usize, at as usize, k) as u64, "TAKEFX", &take.fx);
        }
        text += ">\n";
        write(buf, size, &text);
        true
    })
}

pub(super) unsafe extern "C" fn set_item_state_chunk(item: *mut MediaItem, chunk: *const c_char, _isundo: bool) -> bool {
    let chunk = name_of(chunk);
    counted(|s| {
        let Some(n) = chunk.lines().find_map(|line| line.strip_prefix("FAKEITEM ")).and_then(|n| n.trim().parse::<usize>().ok()) else {
            return false;
        };
        let Some(mut state) = s.items.chunks.get(n).cloned() else { return false };
        state.selected = chunk.lines().any(|line| line.trim() == "SEL 1");
        // A line that isn't there makes REAPER give a new id: an item copied as text without them is a new item.
        match chunk.lines().find_map(|line| line.strip_prefix("IGUID ")) {
            Some(guid) => state.guid = guid.trim().to_string(),
            None => state.guid = new_id(s, "ITEM"),
        }
        let guids: Vec<String> = chunk.lines().filter_map(|line| line.strip_prefix("GUID ")).map(|g| g.trim().to_string()).collect();
        let fxids = chunk.lines().filter(|line| line.starts_with("FXID ")).count();
        let pools = chunk.lines().filter(|line| line.starts_with("POOLEDEVTS ")).count();
        let midi = state.takes.iter().filter(|t| t.source.as_deref() == Some("MIDI")).count();
        let (takes, all_fx) = (state.takes.len(), state.takes.iter().map(|t| fx_count(&t.fx)).sum::<usize>());
        for (k, take) in state.takes.iter_mut().enumerate() {
            take.guid = match guids.get(k) {
                Some(guid) if guids.len() == takes => guid.clone(),
                _ => {
                    s.items.made += 1;
                    format!("{{TAKE-NEW-{}}}", s.items.made)
                }
            };
            if fxids < all_fx {
                renew_fx_ids(&mut s.items.made, &mut take.fx);
            }
        }
        let _ = (pools, midi);
        let Some(slot) = item_mut(s, item as u64) else { return false };
        *slot = state;
        s.undo_dirty = true;
        true
    })
}

/// FX and, in containers, the FX in them: one `FXID` line each in a chunk.
fn fx_count(fx: &[FakeFx]) -> usize {
    fx.iter().map(|f| 1 + f.children.as_deref().map_or(0, fx_count)).sum()
}

/// New ids for every FX of a chain copied without its `FXID` lines, those in containers too, as REAPER gives (lab, 7.82).
fn renew_fx_ids(made: &mut u32, fx: &mut [FakeFx]) {
    for f in fx {
        *made += 1;
        f.guid = format!("{{FX-NEW-{made}}}");
        if let Some(children) = f.children.as_mut() {
            renew_fx_ids(made, children);
        }
    }
}

pub(super) unsafe extern "C" fn update_item_in_project(_item: *mut MediaItem) {
    with(|s| s.redraws += 1);
}
/// As REAPER 7.82 does with a track's default lane flags (lab): each track with fixed lanes keeps as many lanes as its items
/// use, adding lanes an item is on and dropping those left empty at the bottom.
pub(super) unsafe extern "C" fn update_item_lanes(_proj: *mut ReaProject) -> bool {
    counted(|s| {
        for t in s.tracks.iter_mut() {
            let (fixed, count, plays) = lanes_of(t);
            if !fixed {
                continue;
            }
            let needed = t.items.iter().map(|item| item.lane.max(0) as usize + 1).max().unwrap_or(1);
            if needed != count {
                t.extra.insert("I_NUMFIXEDLANES".into(), needed as f64);
                if picked(t) {
                    let mut plays = plays;
                    plays.resize(needed, 0);
                    unpick(t);
                    set_plays(t, &plays);
                }
                // (a lane REAPER adds is named by its number)
                t.lane_names.truncate(needed);
                while t.lane_names.len() < needed {
                    let number = t.lane_names.len() + 1;
                    t.lane_names.push(number.to_string());
                }
            }
        }
    });
    true
}
pub(super) unsafe extern "C" fn update_timeline() {
    with(|s| s.redraws += 1);
}
pub(super) unsafe extern "C" fn set_media_item_selected(item: *mut MediaItem, selected: bool) {
    counted(|s| {
        if let Some(i) = item_mut(s, item as u64) {
            i.selected = selected;
        }
    })
}
pub(super) unsafe extern "C" fn select_all_media_items(_proj: *mut ReaProject, selected: bool) {
    counted(|s| {
        for track in s.tracks.iter_mut() {
            for item in track.items.iter_mut() {
                item.selected = selected;
            }
        }
    })
}

// ----- takes -----

pub(super) unsafe extern "C" fn add_take_to_media_item(item: *mut MediaItem) -> *mut MediaItem_Take {
    counted(|s| {
        if item_of(s, item as u64).is_none() {
            return ptr::null_mut();
        }
        let id = new_id(s, "TAKE");
        let (_, t, i, _) = unpack(item as u64);
        let it = &mut s.tracks[t as usize].items[i as usize];
        let mut take = FakeTake::wave(&id, "");
        take.source = Some("EMPTY".into());
        it.takes.push(take);
        if it.takes.len() == 1 {
            it.active = 0;
        }
        s.undo_dirty = true;
        take_ptr(t as usize, i as usize, it.takes.len() - 1)
    })
}

pub(super) unsafe extern "C" fn set_active_take(take: *mut MediaItem_Take) {
    counted(|s| {
        let (_, t, i, k) = unpack(take as u64);
        if take_of(s, take as u64).is_some() {
            s.tracks[t as usize].items[i as usize].active = k as usize;
            s.undo_dirty = true;
        }
    })
}

pub(super) unsafe extern "C" fn get_media_item_take_info_value(take: *mut MediaItem_Take, parm: *const c_char) -> f64 {
    let parm = name_of(parm);
    counted(|s| {
        let Some(t) = take_of(s, take as u64) else { return 0.0 };
        t.extra.get(&parm).copied().unwrap_or(match parm.as_str() {
            "D_VOL" | "D_PLAYRATE" => 1.0,
            "I_PITCHMODE" | "D_PANLAW" => -1.0,
            "IP_TAKENUMBER" => unpack(take as u64).3 as f64,
            _ => 0.0,
        })
    })
}

pub(super) unsafe extern "C" fn set_media_item_take_info_value(take: *mut MediaItem_Take, parm: *const c_char, value: f64) -> bool {
    let parm = name_of(parm);
    counted(|s| {
        let Some(t) = take_mut(s, take as u64) else { return false };
        if parm == "IP_TAKENUMBER" {
            return false;
        }
        t.extra.insert(parm, value);
        s.undo_dirty = true;
        true
    })
}

pub(super) unsafe extern "C" fn get_set_media_item_take_info_string(
    take: *mut MediaItem_Take,
    parm: *const c_char,
    buf: *mut c_char,
    set: bool,
) -> bool {
    let key = name_of(parm);
    if !set {
        return unsafe { super::get_set_media_item_take_info_string(take, parm, buf, false) };
    }
    let value = name_of(buf);
    counted(|s| match (take_mut(s, take as u64), key.as_str()) {
        (Some(t), "P_NAME") => {
            t.name = value;
            s.undo_dirty = true;
            true
        }
        _ => false,
    })
}

pub(super) unsafe extern "C" fn get_set_media_item_take_info(
    take: *mut MediaItem_Take,
    parm: *const c_char,
    set: *mut c_void,
) -> *mut c_void {
    let parm = name_of(parm);
    counted(|s| {
        if parm != "P_SOURCE" || set.is_null() {
            return ptr::null_mut();
        }
        let (kind, index, _, _) = unpack(set as u64);
        let Some(source) = (kind == NEW_SOURCE).then(|| s.items.sources.get(index as usize).cloned()).flatten() else {
            return ptr::null_mut();
        };
        let bpm = s.bpm;
        let Some(t) = take_mut(s, take as u64) else { return ptr::null_mut() };
        t.source = Some(source.kind.clone());
        t.more.file = Some(source.path.clone());
        t.more.length = source.length;
        if source.kind == "MIDI" {
            let ticks = if source.beats { source.length * 960.0 } else { source.length * bpm / 60.0 * 960.0 };
            t.more.raw = Some(vec![Event { pos: ticks.round() as i64, flag: 0, msg: END_MARKER.to_vec() }]);
        }
        // The take holds it now: it is no longer the caller's to free.
        s.items.sources[index as usize].freed = true;
        s.undo_dirty = true;
        set
    })
}

pub(super) unsafe extern "C" fn pcm_source_create_from_file(path: *const c_char) -> *mut api::PCM_source {
    let path = name_of(path);
    counted(|s| {
        // REAPER makes a source of anything, an empty one (WAVE, length 0) for a missing or unreadable file.
        let (kind, length, beats) = s.items.media.get(&path).cloned().unwrap_or(("WAVE".into(), 0.0, false));
        s.items.sources.push(NewSource { path, kind, length, beats, freed: false });
        pack(NEW_SOURCE, s.items.sources.len() as u64 - 1, 0, 0) as *mut api::PCM_source
    })
}

pub(super) unsafe extern "C" fn pcm_source_destroy(source: *mut api::PCM_source) {
    counted(|s| {
        let (kind, index, _, _) = unpack(source as u64);
        if kind == NEW_SOURCE {
            if let Some(source) = s.items.sources.get_mut(index as usize) {
                source.freed = true;
            }
        }
    })
}

pub(super) unsafe extern "C" fn get_media_source_length(source: *mut api::PCM_source, beats: *mut bool) -> f64 {
    counted(|s| {
        let (kind, index, _, _) = unpack(source as u64);
        let (length, in_beats) = if kind == NEW_SOURCE {
            s.items.sources.get(index as usize).map_or((0.0, false), |src| (src.length, src.beats))
        } else {
            take_of(s, source as u64).map_or((0.0, false), |t| (t.more.length, t.source.as_deref() == Some("MIDI")))
        };
        if !beats.is_null() {
            unsafe { *beats = in_beats };
        }
        length
    })
}

pub(super) unsafe extern "C" fn get_media_source_type(source: *mut api::PCM_source, buf: *mut c_char, size: c_int) {
    let (kind, index, _, _) = unpack(source as u64);
    if kind != NEW_SOURCE {
        return unsafe { super::get_media_source_type(source, buf, size) };
    }
    counted(|s| {
        if let Some(src) = s.items.sources.get(index as usize) {
            write(buf, size, &src.kind);
        }
    })
}

pub(super) unsafe extern "C" fn get_media_source_file_name(source: *mut api::PCM_source, buf: *mut c_char, size: c_int) {
    counted(|s| {
        let (kind, index, _, _) = unpack(source as u64);
        let name = if kind == NEW_SOURCE {
            s.items.sources.get(index as usize).map(|src| src.path.clone())
        } else {
            take_of(s, source as u64).and_then(|t| t.more.file.clone())
        };
        write(buf, size, &name.unwrap_or_default());
    })
}

fn fnv(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016X}")
}

pub(super) unsafe extern "C" fn midi_get_hash(take: *mut MediaItem_Take, notes_only: bool, buf: *mut c_char, size: c_int) -> bool {
    counted(|s| {
        let Some(t) = take_of(s, take as u64) else { return false };
        let events = events_of(t);
        let kept: Vec<Event> = if notes_only {
            events.into_iter().filter(|e| matches!(e.msg.first(), Some(status) if status & 0xE0 == 0x80)).collect()
        } else {
            events
        };
        write(buf, size, &fnv(&crate::midi::encode(&kept)));
        true
    })
}

// ----- stretch markers -----

pub(super) unsafe extern "C" fn get_take_num_stretch_markers(take: *mut MediaItem_Take) -> c_int {
    counted(|s| take_of(s, take as u64).map_or(0, |t| t.more.stretch.len() as c_int))
}

pub(super) unsafe extern "C" fn get_take_stretch_marker(take: *mut MediaItem_Take, index: c_int, pos: *mut f64, src: *mut f64) -> c_int {
    counted(|s| match take_of(s, take as u64).and_then(|t| t.more.stretch.get(index.max(0) as usize).filter(|_| index >= 0)) {
        Some(&(p, sp, _)) => {
            unsafe {
                *pos = p;
                if !src.is_null() {
                    *src = sp;
                }
            }
            index
        }
        None => -1,
    })
}

/// Adds (index < 0) or moves a stretch marker as REAPER does: none is added where one is, a move stays between its
/// neighbours, and the list stays in position order.
pub(super) unsafe extern "C" fn set_take_stretch_marker(take: *mut MediaItem_Take, index: c_int, pos: f64, src: *const f64) -> c_int {
    counted(|s| {
        let Some(t) = take_mut(s, take as u64) else { return -1 };
        let rate = t.extra.get("D_PLAYRATE").copied().unwrap_or(1.0);
        let offs = t.extra.get("D_STARTOFFS").copied().unwrap_or(0.0);
        let list = &mut t.more.stretch;
        let given = (!src.is_null()).then(|| unsafe { *src });
        let result = if index < 0 {
            if list.iter().any(|m| (m.0 - pos).abs() < 1e-9) {
                -1
            } else {
                let at = list.iter().position(|m| m.0 > pos).unwrap_or(list.len());
                // Without a source, the point that plays there now: between two markers, in proportion; past the
                // last (or before the first), from it at the take's rate; with none, the offset plus the position.
                let source = given.unwrap_or_else(|| match (at.checked_sub(1).map(|i| list[i]), list.get(at).copied()) {
                    (Some(a), Some(b)) if b.0 > a.0 => a.1 + (pos - a.0) * (b.1 - a.1) / (b.0 - a.0),
                    (Some(a), _) => a.1 + (pos - a.0) * rate,
                    (None, Some(b)) => b.1 - (b.0 - pos) * rate,
                    (None, None) => pos * rate + offs,
                });
                list.insert(at, (pos, source, 0.0));
                at as c_int
            }
        } else if let Some(marker) = list.get(index as usize).copied() {
            let low = if index > 0 { list[index as usize - 1].0 } else { f64::MIN };
            let high = list.get(index as usize + 1).map_or(f64::MAX, |m| m.0);
            list[index as usize] = (pos.clamp(low, high), given.unwrap_or(marker.1), marker.2);
            index
        } else {
            -1
        };
        if result >= 0 {
            s.undo_dirty = true;
        }
        result
    })
}

pub(super) unsafe extern "C" fn delete_take_stretch_markers(take: *mut MediaItem_Take, index: c_int, count: *const c_int) -> c_int {
    counted(|s| {
        let Some(t) = take_mut(s, take as u64) else { return 0 };
        let count = if count.is_null() { 1 } else { unsafe { *count } };
        let list = &mut t.more.stretch;
        if index < 0 || index as usize >= list.len() || count <= 0 {
            return 0;
        }
        let end = (index as usize + count as usize).min(list.len());
        list.drain(index as usize..end);
        s.undo_dirty = true;
        (end - index as usize) as c_int
    })
}

pub(super) unsafe extern "C" fn get_take_stretch_marker_slope(take: *mut MediaItem_Take, index: c_int) -> f64 {
    counted(|s| take_of(s, take as u64).and_then(|t| t.more.stretch.get(index.max(0) as usize)).map_or(0.0, |m| m.2))
}

pub(super) unsafe extern "C" fn set_take_stretch_marker_slope(take: *mut MediaItem_Take, index: c_int, slope: f64) -> bool {
    counted(|s| match take_mut(s, take as u64).and_then(|t| t.more.stretch.get_mut(index.max(0) as usize)) {
        Some(marker) if index >= 0 => {
            marker.2 = slope;
            s.undo_dirty = true;
            true
        }
        _ => false,
    })
}

// ----- take markers -----

pub(super) unsafe extern "C" fn get_num_take_markers(take: *mut MediaItem_Take) -> c_int {
    counted(|s| take_of(s, take as u64).map_or(0, |t| t.more.markers.len() as c_int))
}

pub(super) unsafe extern "C" fn get_take_marker(
    take: *mut MediaItem_Take,
    index: c_int,
    name: *mut c_char,
    size: c_int,
    color: *mut c_int,
) -> f64 {
    counted(|s| match take_of(s, take as u64).and_then(|t| t.more.markers.get(index.max(0) as usize).filter(|_| index >= 0)) {
        Some((src, label, col)) => {
            write(name, size, label);
            if !color.is_null() {
                unsafe { *color = *col };
            }
            *src
        }
        None => -1.0,
    })
}

/// Adds (index < 0) or changes a take marker; what isn't given stays; the list stays in source order (equal positions in
/// the order they came).
pub(super) unsafe extern "C" fn set_take_marker(
    take: *mut MediaItem_Take,
    index: c_int,
    name: *const c_char,
    src: *mut f64,
    color: *mut c_int,
) -> c_int {
    let label = if name.is_null() { None } else { Some(name_of(name)) };
    counted(|s| {
        let Some(t) = take_mut(s, take as u64) else { return -1 };
        let list = &mut t.more.markers;
        let given_src = (!src.is_null()).then(|| unsafe { *src });
        let given_color = (!color.is_null()).then(|| unsafe { *color });
        let marker = if index < 0 {
            let Some(at) = given_src else { return -1 };
            (at, label.clone().unwrap_or_default(), given_color.unwrap_or(0))
        } else {
            let Some(old) = list.get(index as usize).cloned() else { return -1 };
            list.remove(index as usize);
            (given_src.unwrap_or(old.0), label.clone().unwrap_or(old.1), given_color.unwrap_or(old.2))
        };
        let at = list.iter().position(|m| m.0 > marker.0).unwrap_or(list.len());
        list.insert(at, marker);
        s.undo_dirty = true;
        at as c_int
    })
}

pub(super) unsafe extern "C" fn delete_take_marker(take: *mut MediaItem_Take, index: c_int) -> bool {
    counted(|s| {
        let Some(t) = take_mut(s, take as u64) else { return false };
        if index < 0 || index as usize >= t.more.markers.len() {
            return false;
        }
        t.more.markers.remove(index as usize);
        s.undo_dirty = true;
        true
    })
}

// ----- MIDI: every event, when a take has its own buffer -----

/// A take's MIDI as REAPER's buffer holds it: its own when it has one, else its notes and controllers.
pub(super) fn events_of(t: &FakeTake) -> Vec<Event> {
    match &t.more.raw {
        Some(raw) => raw.clone(),
        None => super::midi_events_of(t),
    }
}

fn is_end(events: &[Event], at: usize) -> bool {
    at + 1 == events.len() && events[at].msg == END_MARKER
}

/// The controller-lane events REAPER counts as CCs: everything on a channel but notes, less the end marker.
fn ccs_of(events: &[Event]) -> Vec<(usize, &Event)> {
    events
        .iter()
        .enumerate()
        .filter(|(at, e)| matches!(e.msg.first(), Some(status) if (0xA0..0xF0).contains(status)) && !is_end(events, *at))
        .collect()
}

fn texts_of(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| match e.msg.as_slice() {
            [0xF0, ..] => true,
            [0xFF, 0x0F, rest @ ..] => !rest.starts_with(b"CCBZ"),
            [0xFF, ..] => true,
            _ => false,
        })
        .count()
}

pub(super) unsafe extern "C" fn midi_count_evts(take: *mut MediaItem_Take, notes: *mut c_int, ccs: *mut c_int, texts: *mut c_int) -> c_int {
    let raw = with(|s| take_of(s, take as u64).and_then(|t| t.more.raw.clone()));
    let Some(raw) = raw else { return unsafe { super::midi_count_evts(take, notes, ccs, texts) } };
    counted(|_| {
        let (n, c, t) = (crate::midi::notes(&raw).len(), ccs_of(&raw).len(), texts_of(&raw));
        unsafe {
            *notes = n as c_int;
            *ccs = c as c_int;
            *texts = t as c_int;
        }
        (n + c + t) as c_int
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "C" fn midi_get_note(
    take: *mut MediaItem_Take,
    index: c_int,
    selected: *mut bool,
    muted: *mut bool,
    start: *mut f64,
    end: *mut f64,
    channel: *mut c_int,
    pitch: *mut c_int,
    velocity: *mut c_int,
) -> bool {
    let raw = with(|s| take_of(s, take as u64).and_then(|t| t.more.raw.clone()));
    let Some(raw) = raw else {
        return unsafe { super::midi_get_note(take, index, selected, muted, start, end, channel, pitch, velocity) };
    };
    counted(|_| match crate::midi::notes(&raw).get(index.max(0) as usize).filter(|_| index >= 0) {
        Some(n) => {
            unsafe {
                *selected = n.selected;
                *muted = n.muted;
                *start = n.start as f64;
                *end = n.end as f64;
                *channel = n.channel as c_int;
                *pitch = n.pitch as c_int;
                *velocity = n.velocity as c_int;
            }
            true
        }
        None => false,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "C" fn midi_get_cc(
    take: *mut MediaItem_Take,
    index: c_int,
    selected: *mut bool,
    muted: *mut bool,
    ppq: *mut f64,
    kind: *mut c_int,
    channel: *mut c_int,
    data1: *mut c_int,
    data2: *mut c_int,
) -> bool {
    let raw = with(|s| take_of(s, take as u64).and_then(|t| t.more.raw.clone()));
    let Some(raw) = raw else {
        return unsafe { super::midi_get_cc(take, index, selected, muted, ppq, kind, channel, data1, data2) };
    };
    counted(|_| match ccs_of(&raw).get(index.max(0) as usize).filter(|_| index >= 0) {
        Some((_, e)) => {
            unsafe {
                *selected = e.flag & 1 != 0;
                *muted = e.flag & 2 != 0;
                *ppq = e.pos as f64;
                *kind = (e.msg[0] & 0xF0) as c_int;
                *channel = (e.msg[0] & 0x0F) as c_int;
                *data1 = e.msg.get(1).copied().unwrap_or(0) as c_int;
                *data2 = e.msg.get(2).copied().unwrap_or(0) as c_int;
            }
            true
        }
        None => false,
    })
}

pub(super) unsafe extern "C" fn midi_get_all_evts(take: *mut MediaItem_Take, buf: *mut c_char, size: *mut c_int) -> bool {
    let raw = with(|s| take_of(s, take as u64).and_then(|t| t.more.raw.clone()));
    let Some(raw) = raw else { return unsafe { super::midi_get_all_evts(take, buf, size) } };
    counted(|_| {
        let bytes = crate::midi::encode(&raw);
        let room = unsafe { *size } as usize;
        unsafe { *size = bytes.len() as c_int };
        if bytes.len() > room {
            return false;
        }
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast(), bytes.len()) };
        true
    })
}

pub(super) unsafe extern "C" fn midi_set_all_evts(take: *mut MediaItem_Take, buf: *const c_char, size: c_int) -> bool {
    let has_raw = with(|s| take_of(s, take as u64).is_some_and(|t| t.more.raw.is_some()));
    if !has_raw {
        return unsafe { super::midi_set_all_evts(take, buf, size) };
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), size.max(0) as usize) }.to_vec();
    counted(|s| {
        let Some(events) = crate::midi::decode(&bytes) else { return false };
        let glitch = s.glitch_events > 0;
        s.glitch_events = s.glitch_events.saturating_sub(1);
        let Some(t) = take_mut(s, take as u64) else { return false };
        let mut events = events;
        if glitch {
            // A write that goes wrong: the last event before the end marker is lost.
            let at = events.len().saturating_sub(2);
            if at < events.len() {
                events.remove(at);
            }
        }
        t.more.raw = Some(events);
        s.undo_dirty = true;
        true
    })
}

/// 960 ticks to the quarter note; the take's source starts where its item does, less its start offset.
fn source_start_qn(s: &State, take: u64) -> f64 {
    let (_, t, i, k) = unpack(take);
    let Some(item) = s.tracks.get(t as usize).and_then(|t| t.items.get(i as usize)) else { return 0.0 };
    let offs = item.takes.get(k as usize).and_then(|tk| tk.extra.get("D_STARTOFFS").copied()).unwrap_or(0.0);
    (item.position - offs) * s.bpm / 60.0
}

pub(super) unsafe extern "C" fn midi_get_proj_qn_from_ppq_pos(take: *mut MediaItem_Take, ppq: f64) -> f64 {
    counted(|s| source_start_qn(s, take as u64) + ppq / 960.0)
}

pub(super) unsafe extern "C" fn midi_get_ppq_pos_from_proj_qn(take: *mut MediaItem_Take, qn: f64) -> f64 {
    counted(|s| (qn - source_start_qn(s, take as u64)) * 960.0)
}

// ----- tracks: fixed lanes and razor areas -----

pub(super) unsafe extern "C" fn get_media_track_info_value(track: *mut MediaTrack, parm: *const c_char) -> f64 {
    let key = name_of(parm);
    let own = counted(|s| {
        let t = track_of(s, track as u64)?;
        let (fixed, count, plays) = lanes_of(t);
        match key.as_str() {
            "I_NUMFIXEDLANES" => Some(if fixed { count as f64 } else { t.extra.get(&key).copied().unwrap_or(0.0) }),
            // 1: no lanes picked, all play; 0: none plays; 2: lanes picked (even every one of them).
            "C_ALLLANESPLAY" if fixed => Some(if !picked(t) {
                1.0
            } else if plays.iter().all(|p| *p == 0) {
                0.0
            } else {
                2.0
            }),
            lane if lane.starts_with("C_LANEPLAYS:") => {
                let n: usize = lane["C_LANEPLAYS:".len()..].parse().ok()?;
                Some(if fixed { plays.get(n).copied().unwrap_or(0) as f64 } else { 0.0 })
            }
            _ => None,
        }
    });
    own.unwrap_or_else(|| unsafe { super::get_media_track_info_value(track, parm) })
}

/// Lane plays as REAPER 7.82 sets them: a lane set to play alone silences the others; one set to play with others leaves
/// the lanes that play playing with it (a lane that played alone now plays with others).
pub(super) unsafe extern "C" fn set_media_track_info_value(track: *mut MediaTrack, parm: *const c_char, value: f64) -> bool {
    let key = name_of(parm);
    let mine = key.starts_with("C_LANEPLAYS:") || key == "C_ALLLANESPLAY" || key == "I_NUMFIXEDLANES" || key == "I_FREEMODE";
    if !mine {
        return unsafe { super::set_media_track_info_value(track, parm, value) };
    }
    counted(|s| {
        let Some(t) = track_mut(s, track as u64) else { return false };
        let (fixed, count, mut plays) = lanes_of(t);
        match key.as_str() {
            "I_FREEMODE" => {
                t.extra.insert(key, value);
            }
            "I_NUMFIXEDLANES" => {
                let n = value.max(1.0) as usize;
                let was_picked = picked(t);
                plays.resize(n, 2);
                t.extra.insert(key, n as f64);
                unpick(t);
                if was_picked {
                    set_plays(t, &plays);
                }
            }
            // 1 unpicks every lane (all play), 0 picks none (none plays).
            "C_ALLLANESPLAY" => {
                if !fixed {
                    return false;
                }
                unpick(t);
                if value == 0.0 {
                    set_plays(t, &vec![0; count]);
                }
            }
            lane => {
                let Some(n) = lane["C_LANEPLAYS:".len()..].parse::<usize>().ok().filter(|n| *n < count && fixed) else { return false };
                // With no lanes picked, a lane that plays set to play with others stays as it is.
                if !picked(t) && value as i32 == 2 {
                    return true;
                }
                match value as i32 {
                    1 => plays = (0..count).map(|m| if m == n { 1 } else { 0 }).collect(),
                    2 => {
                        for p in plays.iter_mut() {
                            if *p == 1 {
                                *p = 2;
                            }
                        }
                        plays[n] = 2;
                    }
                    _ => plays[n] = 0,
                }
                // A lane that plays alone reads "alone" (lab, 7.82).
                let playing: Vec<usize> = (0..plays.len()).filter(|&m| plays[m] != 0).collect();
                if let [only] = playing[..] {
                    plays[only] = 1;
                }
                set_plays(t, &plays);
            }
        }
        s.undo_dirty = true;
        true
    })
}

pub(super) unsafe extern "C" fn get_set_media_track_info_string(
    track: *mut MediaTrack,
    parm: *const c_char,
    buf: *mut c_char,
    set: bool,
) -> bool {
    let key = name_of(parm);
    let mine = key == "P_RAZOREDITS" || key == "P_RAZOREDITS_EXT" || (set && key.starts_with("P_LANENAME:"));
    if !mine {
        return unsafe { super::get_set_media_track_info_string(track, parm, buf, set) };
    }
    let value = if set { name_of(buf) } else { String::new() };
    counted(|s| {
        let Some(t) = track_of(s, track as u64) else { return false };
        let guid = t.guid.clone();
        let (fixed, count, _) = lanes_of(t);
        if let Some(n) = key.strip_prefix("P_LANENAME:") {
            let Some(n) = n.parse::<usize>().ok().filter(|n| fixed && *n < count) else { return false };
            let t = track_mut(s, track as u64).expect("the track");
            if t.lane_names.len() <= n {
                t.lane_names.resize(n + 1, String::new());
            }
            t.lane_names[n] = value;
            s.undo_dirty = true;
            return true;
        }
        if set {
            // REAPER writes each area with its lane's y-range (the whole track when none is given).
            let areas: Vec<String> = value
                .split(',')
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(|area| {
                    let parts: Vec<&str> = area.split(' ').collect();
                    let number = |at: usize| parts.get(at).and_then(|p| p.parse::<f64>().ok()).unwrap_or(0.0);
                    let envelope = parts.get(2).copied().unwrap_or("\"\"");
                    let (top, bottom) = if parts.len() >= 5 { (number(3), number(4)) } else { (0.0, 1.0) };
                    format!("{:.14} {:.14} {envelope} {top:.6} {bottom:.6}", number(0), number(1))
                })
                .collect();
            let text = areas.join(",");
            if text.is_empty() {
                s.items.razor.remove(&guid);
            } else {
                s.items.razor.insert(guid, text);
            }
            s.undo_dirty = true;
            return true;
        }
        let ext = s.items.razor.get(&guid).cloned().unwrap_or_default();
        let text = if key == "P_RAZOREDITS" {
            ext.split(',')
                .filter(|a| !a.is_empty())
                .map(|a| a.split(' ').take(3).collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            ext
        };
        write(buf, 4096, &text);
        true
    })
}

// ----- the item actions Kumi runs -----

/// Takes out every selected item's active take.
fn delete_active_takes(s: &mut State) {
    for track in s.tracks.iter_mut() {
        for item in track.items.iter_mut().filter(|i| i.selected && !i.takes.is_empty()) {
            item.takes.remove(item.active);
            item.active = item.active.min(item.takes.len().saturating_sub(1));
        }
    }
}

/// Glue: each track's selected items become one item playing a new file, placed where the first was.
fn glue(s: &mut State) {
    for t in 0..s.tracks.len() {
        let selected: Vec<usize> = (0..s.tracks[t].items.len()).filter(|&i| s.tracks[t].items[i].selected).collect();
        let Some(&first) = selected.first() else { continue };
        let start = selected.iter().map(|&i| s.tracks[t].items[i].position).fold(f64::MAX, f64::min);
        let end = selected.iter().map(|&i| s.tracks[t].items[i].position + s.tracks[t].items[i].length).fold(f64::MIN, f64::max);
        let (item_id, take_id) = (new_id(s, "ITEM"), new_id(s, "TAKE"));
        let mut take = FakeTake::wave(&take_id, &format!("glued-{}.wav", s.items.made));
        take.more.file = Some(format!("/lab/Recordings/glued-{}.wav", s.items.made));
        take.more.length = end - start;
        let mut glued = FakeItem::new(&item_id, start, end - start, vec![take]);
        glued.selected = true;
        let track = &mut s.tracks[t];
        for &i in selected.iter().rev() {
            track.items.remove(i);
        }
        track.items.insert(first.min(track.items.len()), glued);
    }
}

/// Render to a new take: each selected item gains a take playing a new file, made the active one.
fn render_to_take(s: &mut State) {
    for t in 0..s.tracks.len() {
        for i in 0..s.tracks[t].items.len() {
            if !s.tracks[t].items[i].selected || s.tracks[t].items[i].takes.is_empty() {
                continue;
            }
            let id = new_id(s, "TAKE");
            let n = s.items.made;
            let item = &mut s.tracks[t].items[i];
            let mut take = FakeTake::wave(&id, &format!("render-{n}.wav"));
            take.more.file = Some(format!("/lab/Recordings/render-{n}.wav"));
            take.more.length = item.length;
            item.takes.push(take);
            item.active = item.takes.len() - 1;
        }
    }
}

/// Comp into a new empty lane: each selected track with fixed lanes gets a lane on top, named C1, playing alone.
fn comp_lane(s: &mut State) {
    for t in s.tracks.iter_mut().filter(|t| t.selected) {
        let (fixed, count, _) = lanes_of(t);
        if !fixed {
            continue;
        }
        for item in t.items.iter_mut() {
            item.lane += 1;
        }
        t.lane_names.insert(0, "C1".into());
        t.extra.insert("I_NUMFIXEDLANES".into(), count as f64 + 1.0);
        let plays: Vec<i32> = (0..=count).map(|n| if n == 0 { 1 } else { 0 }).collect();
        set_plays(t, &plays);
        t.extra.insert("COMPLANE".into(), 1.0);
    }
}

/// A comp area from each razor area on a lane of a comping track: the media of that lane in the area is copied into the
/// comp lane (the top one), and the razor area is cleared.
fn comp_area(s: &mut State) {
    for t in 0..s.tracks.len() {
        let guid = s.tracks[t].guid.clone();
        if s.tracks[t].extra.get("COMPLANE").copied().unwrap_or(0.0) == 0.0 {
            continue;
        }
        let Some(ext) = s.items.razor.get(&guid).cloned() else { continue };
        let (_, count, _) = lanes_of(&s.tracks[t]);
        for area in ext.split(',') {
            let parts: Vec<f64> = area.split(' ').filter_map(|p| p.parse::<f64>().ok()).collect();
            let [start, end, top, ..] = parts[..] else { continue };
            let lane = (top * count as f64).round() as i32;
            let Some(source) =
                s.tracks[t].items.iter().find(|i| i.lane == lane && i.position < end && i.position + i.length > start).cloned()
            else {
                continue;
            };
            let mut copy = source.clone();
            copy.guid = new_id(s, "ITEM");
            for take in copy.takes.iter_mut() {
                take.guid = new_id(s, "TAKE");
            }
            copy.position = start.max(source.position);
            copy.length = end.min(source.position + source.length) - copy.position;
            copy.lane = 0;
            copy.selected = false;
            s.tracks[t].items.push(copy);
            let areas = s.tracks[t].extra.get("COMPAREAS").copied().unwrap_or(0.0);
            s.tracks[t].extra.insert("COMPAREAS".into(), areas + 1.0);
        }
        s.items.razor.remove(&guid);
    }
}

pub(super) unsafe extern "C" fn main_on_command_ex(command: c_int, flag: c_int, proj: *mut ReaProject) {
    with(|s| {
        match command {
            40129 => delete_active_takes(s),
            40362 => glue(s),
            41999 | 40209 => render_to_take(s),
            42797 => comp_lane(s),
            42475 => comp_area(s),
            42406 => s.items.razor.clear(),
            _ => return,
        }
        s.undo_dirty = true;
    });
    // On to m4-r-fxenv's (take envelope actions), which hands every id to the base once.
    unsafe { super::fxenv::main_on_command_ex(command, flag, proj) }
}

pub(super) unsafe extern "C" fn kbd_get_text_from_cmd(command: c_int, section: *mut c_void) -> *const c_char {
    match command {
        40129 => c"Take: Delete active take from items".as_ptr(),
        40362 => c"Item: Glue items, ignoring time selection".as_ptr(),
        41999 => c"Item: Render items to new take".as_ptr(),
        40209 => c"Item: Apply track/take FX to items".as_ptr(),
        42797 => c"Track lanes: Comp into new empty lane".as_ptr(),
        42475 => c"Razor edit: Create fixed lane comp area".as_ptr(),
        42406 => c"Razor edit: Clear all areas".as_ptr(),
        _ => unsafe { super::kbd_get_text_from_cmd(command, section) },
    }
}

/// A track's chunk as the fake's own (`FAKECHUNK n` names the state kept), with equal states sharing a number so a chunk
/// reads the same until the track changes, and the `LANEREC` line that says which lane a comping track comps into.
pub(super) unsafe extern "C" fn get_track_state_chunk(track: *mut MediaTrack, buf: *mut c_char, size: c_int, _isundo: bool) -> bool {
    counted(|s| {
        let Some(mut t) = track_of(s, track as u64).cloned() else { return false };
        // The FX chains as m4-r-fxenv writes their entries (its FX kinds read them back), in stable text.
        let chains = super::fxenv::chain_block(s, track as u64, "FXCHAIN", &t.fx)
            + &super::fxenv::chain_block(s, track as u64, "FXCHAIN_REC", &t.input_fx);
        // (r-tracks) Chunks that read the same while the track is the same, for the tests that compare them.
        if let Some(text) = super::m4::stable_chunk(&t, &chains) {
            write(buf, size, &text);
            return true;
        }
        // A track's own sends are lines of the tracks they go to (AUXRECV), not of its chunk. Its selection isn't in it
        // either, and its height is a line of its own (7.82 lab: selecting the track changes nothing in its chunk, a
        // height change moves `TRACKHEIGHT`).
        t.sends.clear();
        t.selected = false;
        let height = t.extra.remove("I_HEIGHTOVERRIDE").unwrap_or(0.0);
        let key = super::chunk_key(&t);
        let n = match s.chunks.iter().position(|kept| *kept == key) {
            Some(n) => n,
            None => {
                s.chunks.push(key);
                s.chunks.len() - 1
            }
        };
        let comping = t.extra.get("COMPLANE").copied().unwrap_or(0.0) != 0.0;
        let lanerec = if comping { "LANEREC -1 0 -1 0\n" } else { "LANEREC -1 -1 -1 0\n" };
        // (r-tracks) A track whose controls are locked says so in its state.
        let lock = if t.extra.get("LOCK") == Some(&1.0) { "LOCK 1\n" } else { "" };
        let text =
            format!("<TRACK {}\nNAME \"{}\"\n{lock}{lanerec}TRACKHEIGHT {height} 0 0 0 0 0 0\nFAKECHUNK {n}\n{chains}>\n", t.guid, t.name);
        write(buf, size, &text);
        true
    })
}

/// The functions above, by name: the fake's table asks here first.
pub(super) fn lookup(name: &CStr) -> *mut c_void {
    macro_rules! table {
        ($($name:literal => $f:expr),* $(,)?) => {
            match name.to_bytes() {
                $($name => $f as *mut c_void,)*
                _ => ptr::null_mut(),
            }
        };
    }
    table! {
        b"GetMediaItemInfo_Value" => get_media_item_info_value as unsafe extern "C" fn(*mut MediaItem, *const c_char) -> f64,
        b"SetMediaItemInfo_Value" => set_media_item_info_value as unsafe extern "C" fn(*mut MediaItem, *const c_char, f64) -> bool,
        b"CreateNewMIDIItemInProj" => create_new_midi_item_in_proj as unsafe extern "C" fn(*mut MediaTrack, f64, f64, *const bool) -> *mut MediaItem,
        b"MarkTrackItemsDirty" => mark_track_items_dirty as unsafe extern "C" fn(*mut MediaTrack, *mut MediaItem),
        b"AddMediaItemToTrack" => add_media_item_to_track as unsafe extern "C" fn(*mut MediaTrack) -> *mut MediaItem,
        b"DeleteTrackMediaItem" => delete_track_media_item as unsafe extern "C" fn(*mut MediaTrack, *mut MediaItem) -> bool,
        b"MoveMediaItemToTrack" => move_media_item_to_track as unsafe extern "C" fn(*mut MediaItem, *mut MediaTrack) -> bool,
        b"SplitMediaItem" => split_media_item as unsafe extern "C" fn(*mut MediaItem, f64) -> *mut MediaItem,
        b"GetItemStateChunk" => get_item_state_chunk as unsafe extern "C" fn(*mut MediaItem, *mut c_char, c_int, bool) -> bool,
        b"SetItemStateChunk" => set_item_state_chunk as unsafe extern "C" fn(*mut MediaItem, *const c_char, bool) -> bool,
        b"UpdateItemInProject" => update_item_in_project as unsafe extern "C" fn(*mut MediaItem),
        b"UpdateItemLanes" => update_item_lanes as unsafe extern "C" fn(*mut ReaProject) -> bool,
        b"UpdateTimeline" => update_timeline as unsafe extern "C" fn(),
        b"SetMediaItemSelected" => set_media_item_selected as unsafe extern "C" fn(*mut MediaItem, bool),
        b"SelectAllMediaItems" => select_all_media_items as unsafe extern "C" fn(*mut ReaProject, bool),
        b"AddTakeToMediaItem" => add_take_to_media_item as unsafe extern "C" fn(*mut MediaItem) -> *mut MediaItem_Take,
        b"SetActiveTake" => set_active_take as unsafe extern "C" fn(*mut MediaItem_Take),
        b"GetMediaItemTakeInfo_Value" => get_media_item_take_info_value as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char) -> f64,
        b"SetMediaItemTakeInfo_Value" => set_media_item_take_info_value as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char, f64) -> bool,
        b"GetSetMediaItemTakeInfo_String" => get_set_media_item_take_info_string as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char, *mut c_char, bool) -> bool,
        b"GetSetMediaItemTakeInfo" => get_set_media_item_take_info as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char, *mut c_void) -> *mut c_void,
        b"PCM_Source_CreateFromFile" => pcm_source_create_from_file as unsafe extern "C" fn(*const c_char) -> *mut api::PCM_source,
        b"PCM_Source_Destroy" => pcm_source_destroy as unsafe extern "C" fn(*mut api::PCM_source),
        b"GetMediaSourceLength" => get_media_source_length as unsafe extern "C" fn(*mut api::PCM_source, *mut bool) -> f64,
        b"GetMediaSourceType" => get_media_source_type as unsafe extern "C" fn(*mut api::PCM_source, *mut c_char, c_int),
        b"GetMediaSourceFileName" => get_media_source_file_name as unsafe extern "C" fn(*mut api::PCM_source, *mut c_char, c_int),
        b"MIDI_GetHash" => midi_get_hash as unsafe extern "C" fn(*mut MediaItem_Take, bool, *mut c_char, c_int) -> bool,
        b"GetTakeNumStretchMarkers" => get_take_num_stretch_markers as unsafe extern "C" fn(*mut MediaItem_Take) -> c_int,
        b"GetTakeStretchMarker" => get_take_stretch_marker as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut f64, *mut f64) -> c_int,
        b"SetTakeStretchMarker" => set_take_stretch_marker as unsafe extern "C" fn(*mut MediaItem_Take, c_int, f64, *const f64) -> c_int,
        b"DeleteTakeStretchMarkers" => delete_take_stretch_markers as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_int) -> c_int,
        b"GetTakeStretchMarkerSlope" => get_take_stretch_marker_slope as unsafe extern "C" fn(*mut MediaItem_Take, c_int) -> f64,
        b"SetTakeStretchMarkerSlope" => set_take_stretch_marker_slope as unsafe extern "C" fn(*mut MediaItem_Take, c_int, f64) -> bool,
        b"GetNumTakeMarkers" => get_num_take_markers as unsafe extern "C" fn(*mut MediaItem_Take) -> c_int,
        b"GetTakeMarker" => get_take_marker as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut c_char, c_int, *mut c_int) -> f64,
        b"SetTakeMarker" => set_take_marker as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char, *mut f64, *mut c_int) -> c_int,
        b"DeleteTakeMarker" => delete_take_marker as unsafe extern "C" fn(*mut MediaItem_Take, c_int) -> bool,
        b"MIDI_CountEvts" => midi_count_evts as unsafe extern "C" fn(*mut MediaItem_Take, *mut c_int, *mut c_int, *mut c_int) -> c_int,
        b"MIDI_GetNote" => midi_get_note as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut bool, *mut bool, *mut f64, *mut f64, *mut c_int, *mut c_int, *mut c_int) -> bool,
        b"MIDI_GetCC" => midi_get_cc as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut bool, *mut bool, *mut f64, *mut c_int, *mut c_int, *mut c_int, *mut c_int) -> bool,
        b"MIDI_GetAllEvts" => midi_get_all_evts as unsafe extern "C" fn(*mut MediaItem_Take, *mut c_char, *mut c_int) -> bool,
        b"MIDI_SetAllEvts" => midi_set_all_evts as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char, c_int) -> bool,
        b"MIDI_GetProjQNFromPPQPos" => midi_get_proj_qn_from_ppq_pos as unsafe extern "C" fn(*mut MediaItem_Take, f64) -> f64,
        b"MIDI_GetPPQPosFromProjQN" => midi_get_ppq_pos_from_proj_qn as unsafe extern "C" fn(*mut MediaItem_Take, f64) -> f64,
        b"GetMediaTrackInfo_Value" => get_media_track_info_value as unsafe extern "C" fn(*mut MediaTrack, *const c_char) -> f64,
        b"SetMediaTrackInfo_Value" => set_media_track_info_value as unsafe extern "C" fn(*mut MediaTrack, *const c_char, f64) -> bool,
        b"GetSetMediaTrackInfo_String" => get_set_media_track_info_string as unsafe extern "C" fn(*mut MediaTrack, *const c_char, *mut c_char, bool) -> bool,
        b"Main_OnCommandEx" => main_on_command_ex as unsafe extern "C" fn(c_int, c_int, *mut ReaProject),
        b"GetTrackStateChunk" => get_track_state_chunk as unsafe extern "C" fn(*mut MediaTrack, *mut c_char, c_int, bool) -> bool,
        b"kbd_getTextFromCmd" => kbd_get_text_from_cmd as unsafe extern "C" fn(c_int, *mut c_void) -> *const c_char,
    }
}

/// Registers a file REAPER can read as media in the fake: its kind and length (quarter notes when `beats`).
pub fn add_media(path: &str, kind: &str, length: f64, beats: bool) {
    with(|s| s.items.media.insert(path.to_string(), (kind.to_string(), length, beats)));
}

/// The handle of the take with this GUID, for tests.
pub fn take_handle(guid: &str) -> Option<u64> {
    with(|s| take_handle_of(s, guid))
}
