//! Items (M4): making an empty MIDI item or one that plays a media file, moving, splitting, trimming and fading one, its
//! properties (mute, volume, colour, loop, lock, timebase, snap offset), grouping, deleting and copying. An item is found
//! by its GUID each time (REAPER has no lookup by GUID: the registry walks the project's items and keeps the map).
//!
//! What a change touches is put back by value where REAPER can set it (a position, a fade, a group id). Where it can't (a
//! split's left item, a deleted item), the item's chunk from before is kept and put back whole, and only while the item
//! is exactly as the change left it (`chunks::print_of`), so nothing of the producer's is overwritten.

use serde_json::{json, Map, Value};

use super::{chunks, marks, quoted, Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Item, Reaper, Take, Track};
use crate::refs::{self, Ref, TrackId};
use crate::rows;

pub(super) fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

pub(super) fn gone(message: impl Into<String>) -> Fault {
    ("gone".into(), message.into())
}

/// Two numbers REAPER gave back for one Kumi set, as far as its saved state keeps them: 14 decimals for most, 9 significant
/// digits for stretch markers (lab, 7.82). A value back from REAPER's undo or redo is still the one Kumi wrote.
pub(super) fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0)
}

/// The same for a volume, a pan or a fade's curve, which REAPER's saved state keeps to 6 decimals (lab, 7.82).
pub(super) fn close6(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

// ----- finding an item -----

/// An item a request names, with the track it is on.
pub(super) struct Found {
    pub item: Item,
    pub guid: String,
    pub track: Track,
}

/// The item a ref names: an item's ref, or a take's (its item).
pub(super) fn find(ctx: &Ctx, value: &Value, what: &str) -> Done<Found> {
    let text = value.as_str().ok_or_else(|| bad(format!("{what}: ref must be an item ref")))?;
    let item = match Ref::parse(text).map_err(bad)? {
        Ref::Item(guid) => ctx.reads.item_by_ref(ctx.reaper, ctx.proj, &guid)?,
        Ref::Take(guid) => {
            let take = ctx.reads.take_by_ref(ctx.reaper, ctx.proj, &guid)?;
            ctx.reaper.take_item(take).ok_or_else(|| failed("that take's item isn't in the project"))?
        }
        other => return Err(bad(format!("{what}: ref must be an item ref, not r:{}", other.kind()))),
    };
    let track = ctx.reaper.item_track(item).ok_or_else(|| failed("that item isn't on a track"))?;
    Ok(Found { item, guid: ctx.reaper.item_guid(item), track })
}

/// As `find`, for an apply: an item that was there for the preview and isn't now has changed since.
pub(super) fn find_for_apply(ctx: &Ctx, value: &Value, what: &str) -> Done<Found> {
    find(ctx, value, what).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })
}

/// As `find`, for a restore: an item that isn't there is gone.
pub(super) fn find_for_restore(ctx: &Ctx, value: &Value, what: &str) -> Done<Found> {
    find(ctx, value, what).map_err(|(code, message)| if code == "failed" { gone(message) } else { (code, message) })
}

/// The item with this GUID, if it is in the project.
pub(super) fn item_by_guid(ctx: &Ctx, guid: &str) -> Option<Item> {
    ctx.reads.item_by_ref(ctx.reaper, ctx.proj, guid).ok()
}

/// The track a request's track ref names (not the master: items aren't on it).
pub(super) fn track_of(ctx: &Ctx, value: &Value, what: &str) -> Done<(Track, String)> {
    let text = value.as_str().ok_or_else(|| bad(format!("{what} must be a track ref")))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::Track(TrackId::Guid(guid)) => Ok((ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.clone()))?, guid)),
        Ref::Track(TrackId::Master) => Err(bad(format!("{what}: the master track holds no items"))),
        other => Err(bad(format!("{what} must be a track ref, not r:{}", other.kind()))),
    }
}

pub(super) fn track_by_guid(ctx: &Ctx, guid: &str) -> Option<Track> {
    ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string())).ok()
}

pub(super) fn target(guid: &str) -> Value {
    json!({"ref": refs::item_ref(guid), "kind": "item"})
}

pub(super) fn guid_of_ref(value: &Value, what: &str) -> Done<String> {
    match Ref::parse(value.as_str().unwrap_or_default()).map_err(bad)? {
        Ref::Item(guid) => Ok(guid),
        other => Err(bad(format!("{what}: an item ref, not r:{}", other.kind()))),
    }
}

// ----- words -----

/// The item as the producer knows it: its active take's name in quotes, else where it is ("the item on “Bass” at bar 5").
pub(super) fn label(ctx: &Ctx, item: Item) -> String {
    let name = ctx.reaper.active_take(item).map(|take| rows::clip(&ctx.reaper.take_name(take))).unwrap_or_default();
    if !name.is_empty() {
        return quoted(&name);
    }
    place_label(ctx, item)
}

/// The item by where it is: "the item on “Bass” at bar 5" (for when its name is the take a change is about).
pub(super) fn place_label(ctx: &Ctx, item: Item) -> String {
    let track = ctx.reaper.item_track(item).map(|track| super::track_name(ctx.reaper, track)).unwrap_or_else(|| "a track".into());
    format!("the item on {} at {}", quoted(&track), marks::bar_text(ctx.reaper, ctx.proj, ctx.reaper.item_value(item, "D_POSITION")))
}

/// An item as HISTORY draws it (for the display only: no undo reads it): its active take's name, where it starts and how
/// long it is (seconds), its fades, whether it's muted, and where it starts in the ruler's words.
pub(super) fn drawn(ctx: &Ctx, item: Item) -> Value {
    let reaper = ctx.reaper;
    let position = reaper.item_value(item, "D_POSITION");
    json!({
        "name": reaper.active_take(item).map(|take| rows::clip(&reaper.take_name(take))).unwrap_or_default(),
        "start": position,
        "length": reaper.item_value(item, "D_LENGTH"),
        "fadeIn": reaper.item_value(item, "D_FADEINLEN"),
        "fadeOut": reaper.item_value(item, "D_FADEOUTLEN"),
        "muted": reaper.item_value(item, "B_MUTE") != 0.0,
        "at": marks::bar_text(reaper, ctx.proj, position),
    })
}

/// The items a change leaves before and after, for HISTORY's picture.
pub(super) fn drawn_items(before: Vec<Value>, after: Vec<Value>) -> Value {
    json!({"before": before, "after": after})
}

/// The same, to open a title with: a name stands bare there.
pub(super) fn label_opening(ctx: &Ctx, item: Item) -> String {
    let name = ctx.reaper.active_take(item).map(|take| rows::clip(&ctx.reaper.take_name(take))).unwrap_or_default();
    if !name.is_empty() {
        return name;
    }
    let mut words = label(ctx, item);
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}

fn seconds_text(seconds: f64) -> String {
    let text = format!("{seconds:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text} s")
}

// ----- what a request gives -----

/// A length given as `…Beats` (quarter notes) or in seconds, as seconds from `start`.
pub(super) fn length_from(ctx: &Ctx, input: &Value, start: f64, beats: &str, seconds: &str) -> Done<Option<f64>> {
    let number = |key: &str| -> Done<Option<f64>> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1.0e6)
                .map(Some)
                .ok_or_else(|| bad(format!("{key} must be a number from 0"))),
        }
    };
    match (number(beats)?, number(seconds)?) {
        (Some(_), Some(_)) => Err(bad(format!("pass {beats} or {seconds}, not both"))),
        (Some(qn), None) => {
            let from = ctx.reaper.quarter_notes_at(ctx.proj, start).ok_or_else(|| failed("this REAPER can't turn time into beats"))?;
            let end =
                ctx.reaper.time_at_quarter_notes(ctx.proj, from + qn).ok_or_else(|| failed("this REAPER can't turn beats into time"))?;
            Ok(Some(end - start))
        }
        (None, seconds) => Ok(seconds),
    }
}

fn text_input(input: &Value, key: &str) -> Done<Option<String>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.chars().count() <= 256 && !text.chars().any(char::is_control) => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{key} must be one line of at most 256 characters"))),
    }
}

/// How many fixed lanes a track has; None when it has none (free or plain item positioning).
pub(super) fn lane_count(reaper: &Reaper, track: Track) -> Option<i64> {
    (reaper.track_value(track, "I_FREEMODE") as i64 == 2).then(|| (reaper.track_value(track, "I_NUMFIXEDLANES") as i64).max(1))
}

/// A lane the input names (`lane`, from 0 as the reads count), checked against the track's lanes: the lane count itself is
/// a new lane at the bottom, which REAPER adds for the item.
fn lane_input(ctx: &Ctx, input: &Value, track: Track) -> Done<Option<i64>> {
    let Some(value) = input.get("lane").filter(|v| !v.is_null()) else { return Ok(None) };
    let lane = value.as_i64().filter(|n| *n >= 0).ok_or_else(|| bad("lane must be a whole number from 0"))?;
    match lane_count(ctx.reaper, track) {
        None => Err(bad("lane: that track has no fixed lanes (turn them on with track_lanes first)")),
        Some(count) if lane > count || lane >= super::lanes::MOST_LANES => {
            Err(bad(format!("lane must be from 0 to {count} on that track: it has {count} lanes, and {count} adds one at the bottom")))
        }
        Some(_) => Ok(Some(lane)),
    }
}

/// After items come, go or change lanes on tracks with fixed lanes: REAPER adds the lanes an item needs and drops those left
/// empty at the bottom (lab, 7.82), as its own edits do.
pub(super) fn settle_lanes(ctx: &Ctx, tracks: &[Option<Track>]) {
    if tracks.iter().flatten().any(|track| lane_count(ctx.reaper, *track).is_some()) {
        ctx.reaper.update_item_lanes(ctx.proj);
    }
}

pub(super) fn is_locked(reaper: &Reaper, item: Item) -> bool {
    reaper.item_value(item, "C_LOCK") as i64 & 1 != 0
}

fn refuse_locked(ctx: &Ctx, item: Item) -> Done<()> {
    if is_locked(ctx.reaper, item) {
        return Err(bad(format!("{} is locked in REAPER: unlock it first (item_props lock: false)", label(ctx, item))));
    }
    Ok(())
}

// ----- making an item -----

struct Make {
    track: Track,
    start: f64,
    end: f64,
    name: Option<String>,
    lane: Option<i64>,
    /// The media file an import plays, and what REAPER reads of it.
    file: Option<(String, crate::reaper::Media)>,
}

fn make_plan(ctx: &Ctx, input: &Value, import: bool) -> Done<Make> {
    let what = if import { "item-import" } else { "item-add" };
    let (track, _) = track_of(ctx, &input["ref"], &format!("{what}: ref"))?;
    let start = marks::moment(ctx, input, "positionBeats", "position")?
        .ok_or_else(|| bad("pass positionBeats (quarter notes from the start) or position (seconds)"))?;
    let file = if import {
        let path =
            input.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).ok_or_else(|| bad("path: the media file's full path"))?;
        if !std::path::Path::new(path).is_absolute() {
            return Err(bad("path must be a full path (from /, or a drive on Windows)"));
        }
        if !std::fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
            return Err(bad("there is no file at that path (or it is a folder)"));
        }
        let media = ctx
            .reaper
            .media(path)
            .ok_or_else(|| bad("REAPER reads nothing from that file: it is no audio or MIDI REAPER knows, or it is empty"))?;
        Some((path.to_string(), media))
    } else {
        None
    };
    let end = match (length_from(ctx, input, start, "lengthBeats", "length")?, marks::moment(ctx, input, "endBeats", "end")?) {
        (Some(_), Some(_)) => return Err(bad("pass a length or an end, not both")),
        (Some(length), None) => start + length,
        (None, Some(end)) => end,
        (None, None) => match &file {
            // A file plays whole by default.
            Some((_, media)) if media.length_is_beats => {
                let from = ctx.reaper.quarter_notes_at(ctx.proj, start).ok_or_else(|| failed("this REAPER can't turn time into beats"))?;
                ctx.reaper
                    .time_at_quarter_notes(ctx.proj, from + media.length)
                    .ok_or_else(|| failed("this REAPER can't turn beats into time"))?
            }
            Some((_, media)) => start + media.length,
            None => return Err(bad("pass lengthBeats or length (or endBeats or end): how long the item is")),
        },
    };
    if end <= start + 1e-6 {
        return Err(bad("the item must end after it starts"));
    }
    Ok(Make { track, start, end, name: text_input(input, "name")?, lane: lane_input(ctx, input, track)?, file })
}

fn make_title(ctx: &Ctx, make: &Make, import: bool) -> String {
    let track = super::track_name(ctx.reaper, make.track);
    let at = marks::bar_text(ctx.reaper, ctx.proj, make.start);
    match (&make.file, &make.name) {
        // (by the take's name when it's given one: a file of Kumi's own is named for Kumi, a stem's with its time)
        (Some(_), Some(name)) if import => format!("Added {} on {} at {at}", quoted(&rows::clip(name)), quoted(&track)),
        (Some((path, _)), _) if import => {
            let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            format!("Added {} on {} at {at}", quoted(&rows::clip(&file)), quoted(&track))
        }
        (_, Some(name)) => format!("Added the MIDI item {} on {} at {at}", quoted(name), quoted(&track)),
        _ => format!("Added a MIDI item on {} at {at}", quoted(&track)),
    }
}

pub fn make_preview(ctx: &Ctx, input: &Value, import: bool) -> Done<Preview> {
    let make = make_plan(ctx, input, import)?;
    let mut display = json!({"title": make_title(ctx, &make, import), "length": rows::seconds(make.end - make.start)});
    if let Some((path, _)) = &make.file {
        display["note"] = super::kumi_files::note(ctx, path).into();
    }
    Ok(Preview {
        target: json!({"ref": null, "kind": "item"}),
        prior: json!({"items": ctx.reaper.track_item_count(make.track)}),
        // (nothing the producer changes on the track makes adding the item wrong: its undo is what is fenced)
        fence: json!({}),
        display,
    })
}

pub fn make_apply(ctx: &Ctx, input: &Value, import: bool) -> Done<Applied> {
    let make = make_plan(ctx, input, import).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    let reaper = ctx.reaper;
    let item = match &make.file {
        None => {
            reaper.create_midi_item(make.track, make.start, make.end).ok_or_else(|| failed("REAPER wouldn't make a MIDI item there"))?
        }
        Some((path, _)) => {
            // (a file in Kumi's own folder plays from a copy in the project's media folder)
            let played = super::kumi_files::playable(ctx, path)?;
            let item = reaper.add_item(make.track).ok_or_else(|| failed("REAPER wouldn't make an item"))?;
            let take = reaper.add_take(item);
            let media = take.and_then(|take| reaper.set_take_file(take, &played));
            if media.is_none() {
                // Nothing half-made stays.
                reaper.delete_item(make.track, item);
                return Err(failed("REAPER wouldn't read that file into a take: nothing was made"));
            }
            reaper.set_item_value(item, "D_POSITION", make.start);
            reaper.set_item_value(item, "D_LENGTH", make.end - make.start);
            // (m7) As the producer's own import (Insert Media) makes it: looped as their REAPER loops imported media, and
            // with no fades (an item AddMediaItemToTrack makes always loops and has the new-item fades; KP1 lab: Insert
            // Media gave none whatever the fade settings).
            if let Some(loops) = crate::setup::loops_imported_media(reaper) {
                reaper.set_item_value(item, "B_LOOPSRC", if loops { 1.0 } else { 0.0 });
            }
            reaper.set_item_value(item, "D_FADEINLEN", 0.0);
            reaper.set_item_value(item, "D_FADEOUTLEN", 0.0);
            if let Some(take) = take {
                let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                reaper.set_take_string(take, "P_NAME", make.name.as_deref().unwrap_or(&file));
            }
            item
        }
    };
    if make.file.is_none() {
        if let (Some(name), Some(take)) = (&make.name, reaper.active_take(item)) {
            reaper.set_take_string(take, "P_NAME", name);
        }
    }
    if let Some(lane) = make.lane {
        reaper.set_item_value(item, "I_FIXEDLANE", lane as f64);
        reaper.update_item_lanes(ctx.proj);
    }
    reaper.update_item(item);
    let guid = reaper.item_guid(item);
    ctx.reads.registry().note_item(ctx.proj, &guid, item);
    let print = chunks::item_print(reaper, item).ok_or_else(|| failed("REAPER wouldn't give the item it made"))?;
    let take_ref = reaper.active_take(item).map(|take| refs::take_ref(&reaper.take_guid(take)));
    Ok(Applied {
        applied: json!({"print": print, "position": reaper.item_value(item, "D_POSITION"), "length": reaper.item_value(item, "D_LENGTH")}),
        created: Some(json!({"ref": refs::item_ref(&guid), "kind": "item", "takeRef": take_ref})),
        material: None,
        display: {
            let mut display = json!({"title": make_title(ctx, &make, import), "items": drawn_items(vec![], vec![drawn(ctx, item)])});
            if let Some((path, _)) = &make.file {
                display["note"] = super::kumi_files::note(ctx, path).into();
            }
            display
        },
        ..Default::default()
    })
}

/// Takes away an item a change made, while it is as the change left it.
pub fn created_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the item that was made"))?;
    let guid = guid_of_ref(&created["ref"], "created")?;
    let Some(item) = item_by_guid(ctx, &guid) else {
        return Err(gone("the item Kumi made isn't in the project any more"));
    };
    let words = label(ctx, item);
    let print = chunks::item_print(ctx.reaper, item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    if !restore.discard && Some(print.as_str()) != restore.applied["print"].as_str() {
        return Err(changed(format!("{words} has been changed in REAPER since Kumi made it (edited, moved or filled)")));
    }
    let track = ctx.reaper.item_track(item).ok_or_else(|| failed("the item isn't on a track"))?;
    if !ctx.reaper.delete_item(track, item) {
        return Err(failed("REAPER wouldn't delete the item"));
    }
    settle_lanes(ctx, &[Some(track)]);
    let mut title = format!("{words} taken away again");
    if let Some(first) = title.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    Ok(Restored { restored: json!({"ref": refs::item_ref(&guid), "deleted": true}), display: json!({"title": title}), unwritten: false })
}

// ----- moving -----

/// Where an item is: its position, its track and, on a track with fixed lanes, its lane.
fn place_of(reaper: &Reaper, item: Item) -> Value {
    let track = reaper.item_track(item);
    let lane = track.and_then(|track| lane_count(reaper, track)).map(|_| reaper.item_value(item, "I_FIXEDLANE") as i64);
    json!({
        "position": reaper.item_value(item, "D_POSITION"),
        "track": track.map(|track| reaper.track_guid(track)),
        "lane": lane,
    })
}

fn same_place(a: &Value, b: &Value) -> bool {
    close(number(a, "position"), number(b, "position")) && a["track"] == b["track"] && a["lane"] == b["lane"]
}

/// Puts an item where `place` says, as far as REAPER lets; the item as REAPER has it after (found again by its GUID).
fn put_place(ctx: &Ctx, guid: &str, place: &Value) -> Done<Item> {
    let reaper = ctx.reaper;
    let mut item = item_by_guid(ctx, guid).ok_or_else(|| gone("the item isn't in the project any more"))?;
    let from = reaper.item_track(item);
    if let Some(dest) = place["track"].as_str() {
        let track = track_by_guid(ctx, dest).ok_or_else(|| gone("the track it goes on isn't in the project any more"))?;
        if reaper.item_track(item) != Some(track) {
            if !reaper.move_item_to_track(item, track) {
                return Err(failed("REAPER wouldn't move the item to that track"));
            }
            item = item_by_guid(ctx, guid)
                .ok_or_else(|| ("uncertain".to_string(), "the item went while it was moved: read the items".to_string()))?;
        }
    }
    reaper.set_item_value(item, "D_POSITION", number(place, "position"));
    if let Some(lane) = place["lane"].as_i64() {
        reaper.set_item_value(item, "I_FIXEDLANE", lane as f64);
    }
    settle_lanes(ctx, &[from, reaper.item_track(item)]);
    reaper.update_item(item);
    item_by_guid(ctx, guid).ok_or_else(|| failed("the item went while it was moved"))
}

fn move_wanted(ctx: &Ctx, input: &Value, found: &Found) -> Done<Value> {
    let reaper = ctx.reaper;
    let position = marks::moment(ctx, input, "positionBeats", "position")?;
    let dest = match input.get("toRef").filter(|v| !v.is_null()) {
        Some(value) => Some(track_of(ctx, value, "toRef")?),
        None => None,
    };
    if position.is_none() && dest.is_none() && input.get("lane").is_none_or(Value::is_null) {
        return Err(bad("pass where it goes: positionBeats or position, toRef (another track), lane, or several"));
    }
    let (track, track_guid) = dest.unwrap_or((found.track, reaper.track_guid(found.track)));
    let lane = match lane_input(ctx, input, track)? {
        Some(lane) => Some(lane),
        // Onto a track with fixed lanes: the lane it is in, if that track has it, else the first.
        None => lane_count(reaper, track).map(|count| {
            let now = reaper.item_value(found.item, "I_FIXEDLANE") as i64;
            if now < count {
                now
            } else {
                0
            }
        }),
    };
    Ok(json!({"position": position.unwrap_or(reaper.item_value(found.item, "D_POSITION")), "track": track_guid, "lane": lane}))
}

fn move_title(ctx: &Ctx, item: Item, to: &Value, from: &Value) -> String {
    let at = marks::bar_text(ctx.reaper, ctx.proj, number(to, "position"));
    let mut title = format!("Moved {} to {at}", label(ctx, item));
    if to["track"] != from["track"] {
        if let Some(track) = to["track"].as_str().and_then(|guid| track_by_guid(ctx, guid)) {
            title += &format!(" on {}", quoted(&super::track_name(ctx.reaper, track)));
        }
    }
    if to["lane"] != from["lane"] {
        if let Some(lane) = to["lane"].as_i64() {
            title += &format!(", lane {}", lane + 1);
        }
    }
    title
}

pub fn move_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-move")?;
    let now = place_of(ctx.reaper, found.item);
    let wanted = move_wanted(ctx, input, &found)?;
    let mut display = json!({"title": move_title(ctx, found.item, &wanted, &now), "before": now, "after": wanted});
    if same_place(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    if is_locked(ctx.reaper, found.item) {
        display["note"] = format!("{} is locked in REAPER: unlock it first (item_props lock: false).", label(ctx, found.item)).into();
    }
    Ok(Preview { target: target(&found.guid), prior: now.clone(), fence: now, display })
}

pub fn move_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-move")?;
    let now = place_of(ctx.reaper, found.item);
    if !same_place(&now, fence) {
        return Err(changed(format!("{} moved since the preview", label(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    let wanted = move_wanted(ctx, input, &found)?;
    // (said before the move: an item without a name is named by where it is)
    let title = move_title(ctx, found.item, &wanted, &now);
    if same_place(&now, &wanted) {
        return Ok(Applied { applied: now.clone(), display: json!({"title": title}), unwritten: true, ..Default::default() });
    }
    let was = drawn(ctx, found.item);
    // Its lane, told by what is on it with it (lane numbers move when a lane goes in above): the other items on it.
    let mates: Vec<String> = match now["lane"].as_i64() {
        Some(lane) => (0..ctx.reaper.track_item_count(found.track))
            .filter_map(|n| ctx.reaper.track_item(found.track, n))
            .filter(|other| *other != found.item && ctx.reaper.item_value(*other, "I_FIXEDLANE") as i64 == lane)
            .map(|other| ctx.reaper.item_guid(other))
            .collect(),
        None => vec![],
    };
    let item = put_place(ctx, &found.guid, &wanted)?;
    let after = place_of(ctx.reaper, item);
    if !same_place(&after, &wanted) {
        // Half a move is no move: it goes back where it was.
        let back = put_place(ctx, &found.guid, &now).is_ok_and(|item| same_place(&place_of(ctx.reaper, item), &now));
        return Err(if back {
            failed("REAPER didn't put the item where it was asked to; it is back where it was")
        } else {
            ("uncertain".into(), "REAPER didn't put the item where it was asked to, and Kumi could not put it back: read it".into())
        });
    }
    let items = drawn_items(vec![was], vec![drawn(ctx, item)]);
    let mut applied = after.clone();
    if now["lane"].is_i64() {
        // The lanes of the track it left as the move left them (REAPER drops a bottom lane left empty).
        applied["from"] = json!({"lanes": super::lanes::lane_names(ctx.reaper, found.track), "mates": mates});
    }
    Ok(Applied { applied, display: json!({"title": title, "before": now, "after": after, "items": items}), ..Default::default() })
}

/// The lane an item goes back to on the track it left (`from`, kept by the move): the lane its old lane mates are on, or,
/// with none left, its number while no lane went in or out above it since. None when that can't be told.
fn lane_back(ctx: &Ctx, track: Track, lane: i64, from: &Value) -> Option<i64> {
    let reaper = ctx.reaper;
    let mates: Vec<&str> = from["mates"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let lanes_now: Vec<i64> = (0..reaper.track_item_count(track))
        .filter_map(|n| reaper.track_item(track, n))
        .filter(|item| mates.contains(&reaper.item_guid(*item).as_str()))
        .map(|item| reaper.item_value(item, "I_FIXEDLANE") as i64)
        .collect();
    if let Some(first) = lanes_now.first() {
        return lanes_now.iter().all(|l| l == first).then_some(*first);
    }
    let Some(then) = from["lanes"].as_array() else { return Some(lane) };
    let now = super::lanes::lane_names(reaper, track);
    let kept = then.len().min(now.len()).min(lane as usize + 1);
    then.iter().take(kept).zip(now.iter()).all(|(a, b)| a.as_str() == Some(b.as_str())).then_some(lane)
}

pub fn move_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "item-move")?;
    let now = place_of(ctx.reaper, found.item);
    if same_place(&now, restore.prior) {
        return Ok(Restored {
            restored: now,
            display: json!({"title": format!("{} is back where it was", label(ctx, found.item))}),
            unwritten: true,
        });
    }
    if !restore.discard && !same_place(&now, restore.applied) {
        return Err(changed(format!("{} has been moved in REAPER since Kumi moved it", label(ctx, found.item))));
    }
    // The lane it goes back to, as the track it goes back to has lanes now: one whose fixed lanes went off or on since
    // can't hold it as it was, nor one where a lane went in or out above its lane (its number is another lane's now),
    // which is the producer's change, found before anything is written. Told to undo anyway, it goes back to its place
    // there: on its first lane, or at its lane's number. (A bottom lane REAPER dropped when the move emptied it is made
    // again: its number is the lane after the last.)
    let mut back = restore.prior.clone();
    if let Some(track) = restore.prior["track"].as_str().and_then(|guid| track_by_guid(ctx, guid)) {
        let lanes = lane_count(ctx.reaper, track);
        let refused = |why: &str| {
            changed(format!(
                "{} {why} since Kumi moved {} off it: it can't go back to its lane",
                quoted(&super::track_name(ctx.reaper, track)),
                label(ctx, found.item)
            ))
        };
        match (restore.prior["lane"].as_i64(), lanes) {
            (Some(_), None) | (None, Some(_)) => {
                if !restore.discard {
                    return Err(refused("has had its fixed lanes turned off or on in REAPER"));
                }
                back["lane"] = lanes.map(|_| 0).into();
            }
            (Some(lane), Some(count)) => match lane_back(ctx, track, lane, &restore.applied["from"]) {
                Some(at) => back["lane"] = at.min(count).into(),
                None if restore.discard => back["lane"] = lane.min(count).into(),
                None => return Err(refused("has had lanes added or taken away above its lane in REAPER")),
            },
            (None, None) => {}
        }
    }
    let item = put_place(ctx, &found.guid, &back)?;
    let after = place_of(ctx.reaper, item);
    if !same_place(&after, &back) {
        // Written, so never a plain failure: put where Kumi left it, it is as before this undo; else it's uncertain.
        let left = put_place(ctx, &found.guid, &now).is_ok_and(|item| same_place(&place_of(ctx.reaper, item), &now));
        return Err(if left {
            failed("REAPER didn't put the item back where it was; it is where Kumi left it")
        } else {
            ("uncertain".into(), "REAPER didn't put the item back where it was, and Kumi could not leave it as it was: read it".into())
        });
    }
    Ok(Restored { restored: after, display: json!({"title": format!("{} moved back", label_opening(ctx, item))}), unwritten: false })
}

// ----- splitting -----

fn split_at(ctx: &Ctx, input: &Value, found: &Found) -> Done<f64> {
    let at = marks::moment(ctx, input, "positionBeats", "position")?
        .ok_or_else(|| bad("pass positionBeats (quarter notes from the start) or position (seconds): where to split"))?;
    let (start, length) = (ctx.reaper.item_value(found.item, "D_POSITION"), ctx.reaper.item_value(found.item, "D_LENGTH"));
    if at <= start + 1e-6 || at >= start + length - 1e-6 {
        return Err(bad(format!(
            "that's not inside the item: it runs from {} to {}",
            marks::bar_text(ctx.reaper, ctx.proj, start),
            marks::bar_text(ctx.reaper, ctx.proj, start + length)
        )));
    }
    Ok(at)
}

pub fn split_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-split")?;
    let at = split_at(ctx, input, &found)?;
    let print = chunks::item_print(ctx.reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    let title = format!("Split {} at {}", label(ctx, found.item), marks::bar_text(ctx.reaper, ctx.proj, at));
    Ok(Preview {
        target: target(&found.guid),
        prior: json!({"print": print}),
        fence: json!({"print": print}),
        display: json!({"title": title}),
    })
}

pub fn split_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-split")?;
    let reaper = ctx.reaper;
    let chunk = reaper.item_chunk(found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() {
        return Err(changed(format!("{} changed since the preview", label(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    let at = split_at(ctx, input, &found)?;
    if chunk.len() > chunks::MOST_KEPT {
        return Err(failed(format!(
            "this item's state is {} MB, more than the {} MB Kumi keeps to put it back: Kumi won't split what it can't put back",
            chunk.len() >> 20,
            chunks::MOST_KEPT >> 20
        )));
    }
    let title = format!("Split {} at {}", label(ctx, found.item), marks::bar_text(reaper, ctx.proj, at));
    let whole = drawn(ctx, found.item);
    let right = reaper.split_item(found.item, at).ok_or_else(|| failed("REAPER wouldn't split the item there"))?;
    let right_guid = reaper.item_guid(right);
    ctx.reads.registry().note_item(ctx.proj, &right_guid, right);
    let left = item_by_guid(ctx, &found.guid)
        .ok_or_else(|| ("uncertain".to_string(), "the item went while it was split: read the track".to_string()))?;
    let (left_print, right_print) = (chunks::item_print(reaper, left), chunks::item_print(reaper, right));
    let (Some(left_print), Some(right_print)) = (left_print, right_print) else {
        return Err(("uncertain".into(), "REAPER split the item but wouldn't give the halves' state: read the track".into()));
    };
    Ok(Applied {
        applied: json!({"left": left_print, "right": right_print}),
        created: Some(json!({"ref": refs::item_ref(&right_guid), "kind": "item"})),
        material: Some(json!({"chunk": chunk})),
        display: json!({"title": title, "items": drawn_items(vec![whole], vec![drawn(ctx, left), drawn(ctx, right)])}),
        ..Default::default()
    })
}

/// Puts back an item's chunk from before a change, keeping the producer's selection of it.
pub(super) fn put_chunk(ctx: &Ctx, item: Item, chunk: &str) -> Done<()> {
    let selected = ctx.reaper.item_value(item, "B_UISEL") != 0.0;
    if !ctx.reaper.set_item_chunk(item, chunk) {
        return Err(failed("REAPER wouldn't take the item's state back"));
    }
    ctx.reaper.set_item_value(item, "B_UISEL", if selected { 1.0 } else { 0.0 });
    ctx.reaper.update_item(item);
    Ok(())
}

pub fn split_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the item from before the split"))?;
    let chunk = material["chunk"].as_str().ok_or_else(|| bad("material: the item's state"))?;
    let left = find_for_restore(ctx, &restore.target["ref"], "item-split")?;
    let right_guid = restore.created.map(|created| guid_of_ref(&created["ref"], "created")).transpose()?;
    let right = right_guid.as_deref().and_then(|guid| item_by_guid(ctx, guid));
    let reaper = ctx.reaper;
    let left_print = chunks::item_print(reaper, left.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    let words = label(ctx, left.item);
    if right.is_none() && Some(left_print.as_str()) == restore.prior["print"].as_str() {
        return Ok(Restored {
            restored: json!({"ref": refs::item_ref(&left.guid)}),
            display: json!({"title": format!("{words} is whole again")}),
            unwritten: true,
        });
    }
    // (discard too: the item's whole state from before goes back over both parts, which would take the producer's edits
    // since with it, or bring back a part they deleted)
    let whole = "putting the item back whole would write over that too, so Kumi leaves it, even with discard";
    if Some(left_print.as_str()) != restore.applied["left"].as_str() {
        return Err(changed(format!("{words} has been changed in REAPER since Kumi split it: {whole}")));
    }
    match right {
        None => return Err(changed(format!("the right part of the split has been deleted in REAPER since: {whole}"))),
        Some(right) if chunks::item_print(reaper, right).as_deref() != restore.applied["right"].as_str() => {
            return Err(changed(format!("the right part of the split has been changed in REAPER since Kumi split it: {whole}")));
        }
        Some(_) => {}
    }
    if let Some(right) = right {
        let track = reaper.item_track(right).ok_or_else(|| failed("the right part isn't on a track"))?;
        if !reaper.delete_item(track, right) {
            return Err(failed("REAPER wouldn't take the right part away"));
        }
    }
    // (found again: the left's pointer is REAPER's own, but the list it is in changed)
    let left_item = item_by_guid(ctx, &left.guid)
        .ok_or_else(|| ("uncertain".to_string(), "the item went while it was put back: read the track".to_string()))?;
    put_chunk(ctx, left_item, chunk)?;
    Ok(Restored {
        restored: json!({"ref": refs::item_ref(&left.guid)}),
        display: json!({"title": format!("{} whole again", label_opening(ctx, left_item))}),
        unwritten: false,
    })
}

// ----- trimming -----

/// An item's edges and its takes' source offsets: what a trim moves.
fn edges_of(reaper: &Reaper, item: Item) -> Value {
    let takes: Vec<Value> = (0..reaper.take_count(item))
        .filter_map(|n| reaper.take_at(item, n))
        .map(|take| json!([reaper.take_guid(take), reaper.take_value(take, "D_STARTOFFS")]))
        .collect();
    json!({"position": reaper.item_value(item, "D_POSITION"), "length": reaper.item_value(item, "D_LENGTH"), "offsets": takes})
}

fn same_edges(a: &Value, b: &Value) -> bool {
    let offsets = |v: &Value| v["offsets"].as_array().cloned().unwrap_or_default();
    let (oa, ob) = (offsets(a), offsets(b));
    close(number(a, "position"), number(b, "position"))
        && close(number(a, "length"), number(b, "length"))
        && oa.len() == ob.len()
        && oa.iter().zip(&ob).all(|(x, y)| x[0] == y[0] && close(x[1].as_f64().unwrap_or(f64::NAN), y[1].as_f64().unwrap_or(f64::NAN)))
}

fn trim_wanted(ctx: &Ctx, input: &Value, found: &Found) -> Done<Value> {
    let reaper = ctx.reaper;
    let (start, length) = (reaper.item_value(found.item, "D_POSITION"), reaper.item_value(found.item, "D_LENGTH"));
    let new_start = marks::moment(ctx, input, "startBeats", "start")?;
    let new_end = marks::moment(ctx, input, "endBeats", "end")?;
    if new_start.is_none() && new_end.is_none() {
        return Err(bad("pass the new start (startBeats or start), the new end (endBeats or end), or both"));
    }
    let (s, e) = (new_start.unwrap_or(start), new_end.unwrap_or(start + length));
    if e <= s + 1e-6 {
        return Err(bad("the item must end after it starts"));
    }
    let delta = s - start;
    let takes: Vec<Take> = (0..reaper.take_count(found.item)).filter_map(|n| reaper.take_at(found.item, n)).collect();
    if delta.abs() > 1e-12 {
        // REAPER doesn't move stretch markers with a take's offset: the audio under them would slide.
        if takes.iter().any(|take| !reaper.stretch_markers(*take).is_empty()) {
            return Err(bad(
                "this item has stretch markers, which REAPER doesn't move with its start: trim its end, or delete the markers first",
            ));
        }
        let loops = reaper.item_value(found.item, "B_LOOPSRC") != 0.0;
        let before_media =
            takes.iter().any(|take| reaper.take_value(*take, "D_STARTOFFS") + delta * reaper.take_value(*take, "D_PLAYRATE") < -1e-9);
        if before_media && !loops {
            return Err(bad("that start is before the start of its media: turn on loop source (item_props loop: true) or trim less"));
        }
    }
    let offsets: Vec<Value> = takes
        .iter()
        .map(|take| {
            json!([reaper.take_guid(*take), reaper.take_value(*take, "D_STARTOFFS") + delta * reaper.take_value(*take, "D_PLAYRATE")])
        })
        .collect();
    Ok(json!({"position": s, "length": e - s, "offsets": offsets}))
}

fn put_edges(ctx: &Ctx, item: Item, edges: &Value) -> bool {
    let reaper = ctx.reaper;
    let mut ok = reaper.set_item_value(item, "D_POSITION", number(edges, "position"));
    ok &= reaper.set_item_value(item, "D_LENGTH", number(edges, "length"));
    for entry in edges["offsets"].as_array().into_iter().flatten() {
        let (Some(guid), Some(offset)) = (entry[0].as_str(), entry[1].as_f64()) else { continue };
        let take = (0..reaper.take_count(item)).filter_map(|n| reaper.take_at(item, n)).find(|take| reaper.take_guid(*take) == guid);
        ok &= take.is_some_and(|take| reaper.set_take_value(take, "D_STARTOFFS", offset));
    }
    reaper.update_item(item);
    ok
}

fn trim_title(ctx: &Ctx, item: Item, edges: &Value) -> String {
    let (start, length) = (number(edges, "position"), number(edges, "length"));
    format!(
        "Trimmed {} to {}–{}",
        label(ctx, item),
        marks::bar_text(ctx.reaper, ctx.proj, start),
        marks::bar_text(ctx.reaper, ctx.proj, start + length)
    )
}

pub fn trim_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-trim")?;
    let now = edges_of(ctx.reaper, found.item);
    let wanted = trim_wanted(ctx, input, &found)?;
    let mut display = json!({"title": trim_title(ctx, found.item, &wanted)});
    if same_edges(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(&found.guid), prior: now.clone(), fence: now, display })
}

pub fn trim_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-trim")?;
    let now = edges_of(ctx.reaper, found.item);
    if !same_edges(&now, fence) {
        return Err(changed(format!("{} changed since the preview", label(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    let wanted = trim_wanted(ctx, input, &found)?;
    let title = trim_title(ctx, found.item, &wanted);
    if same_edges(&now, &wanted) {
        return Ok(Applied { applied: now, display: json!({"title": title}), unwritten: true, ..Default::default() });
    }
    let was = drawn(ctx, found.item);
    put_edges(ctx, found.item, &wanted);
    let item = item_by_guid(ctx, &found.guid)
        .ok_or_else(|| ("uncertain".to_string(), "the item went while it was trimmed: read it".to_string()))?;
    let after = edges_of(ctx.reaper, item);
    if !same_edges(&after, &wanted) {
        let back = put_edges(ctx, item, &now) && same_edges(&edges_of(ctx.reaper, item), &now);
        return Err(if back {
            failed("REAPER didn't trim the item as asked; it is as it was")
        } else {
            ("uncertain".into(), "REAPER didn't trim the item as asked, and Kumi could not put it back: read it".into())
        });
    }
    Ok(Applied {
        applied: after,
        display: json!({"title": title, "items": drawn_items(vec![was], vec![drawn(ctx, item)])}),
        ..Default::default()
    })
}

pub fn trim_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "item-trim")?;
    let now = edges_of(ctx.reaper, found.item);
    if same_edges(&now, restore.prior) {
        return Ok(Restored {
            restored: now,
            display: json!({"title": format!("{} is as it was", label_opening(ctx, found.item))}),
            unwritten: true,
        });
    }
    if !restore.discard && !same_edges(&now, restore.applied) {
        return Err(changed(format!("{} has been changed in REAPER since Kumi trimmed it", label(ctx, found.item))));
    }
    // Putting the start back moves its takes' offsets, and REAPER doesn't move stretch markers with them: markers the
    // producer added since (the trim had none to move) would slide off the audio they hold, discard or not.
    let reaper = ctx.reaper;
    let offset_of = |state: &Value, guid: &str| {
        state["offsets"].as_array().into_iter().flatten().find(|e| e[0].as_str() == Some(guid)).and_then(|e| e[1].as_f64())
    };
    let slid = (0..reaper.take_count(found.item)).filter_map(|n| reaper.take_at(found.item, n)).any(|take| {
        let guid = reaper.take_guid(take);
        let moves = match (offset_of(&now, &guid), offset_of(restore.prior, &guid)) {
            (Some(now), Some(then)) => !close(now, then),
            _ => false,
        };
        moves && !reaper.stretch_markers(take).is_empty()
    });
    if slid {
        return Err(changed(format!(
            "{} has stretch markers now, which REAPER doesn't move with its start: putting the start back would slide them off \
             the audio they hold, so Kumi leaves it, even with discard",
            label(ctx, found.item)
        )));
    }
    put_edges(ctx, found.item, restore.prior);
    let after = edges_of(ctx.reaper, found.item);
    if !same_edges(&after, restore.prior) {
        // Written, so never a plain failure: put as Kumi left it, it is as before this undo; else it's uncertain.
        let left = put_edges(ctx, found.item, &now) && same_edges(&edges_of(ctx.reaper, found.item), &now);
        return Err(if left {
            failed("REAPER didn't put the item's edges back; it is as Kumi left it")
        } else {
            ("uncertain".into(), "REAPER didn't put the item's edges back, and Kumi could not leave it as it was: read it".into())
        });
    }
    Ok(Restored {
        restored: after,
        display: json!({"title": format!("{} back to its length", label_opening(ctx, found.item))}),
        unwritten: false,
    })
}

// ----- fades -----

/// A fade's keys, in the order a fade is put back: the length, then the shape (a preset of the curve and S values that
/// sets them), the 7.81 curve and S values, and REAPER's legacy curve, which only a custom shape (-1) keeps apart (lab,
/// 7.82: that order brings back every state exactly, the chunk's FADEIN line too).
const FADE_KEYS: [&str; 5] = ["LEN", "SHAPE", "DIR_NEW", "DIR2_NEW", "DIR"];

fn fade_key(side: &str, key: &str) -> String {
    match key {
        "LEN" => format!("D_FADE{side}LEN"),
        "SHAPE" => format!("C_FADE{side}SHAPE"),
        "DIR" => format!("D_FADE{side}DIR"),
        "DIR_NEW" => format!("D_FADE{side}DIR_NEW"),
        _ => format!("D_FADE{side}DIR2_NEW"),
    }
}

fn fade_of(reaper: &Reaper, item: Item, side: &str) -> Value {
    json!({
        "length": reaper.item_value(item, &fade_key(side, "LEN")),
        "shape": reaper.item_value(item, &fade_key(side, "SHAPE")) as i64,
        "curve": reaper.item_value(item, &fade_key(side, "DIR_NEW")),
        "s": reaper.item_value(item, &fade_key(side, "DIR2_NEW")),
        "legacy": reaper.item_value(item, &fade_key(side, "DIR")),
    })
}

fn fades_of(reaper: &Reaper, item: Item) -> Value {
    json!({"fadeIn": fade_of(reaper, item, "IN"), "fadeOut": fade_of(reaper, item, "OUT")})
}

fn same_fade(a: &Value, b: &Value) -> bool {
    close(number(a, "length"), number(b, "length"))
        && ["curve", "s", "legacy"].iter().all(|key| close6(number(a, key), number(b, key)))
        && a["shape"] == b["shape"]
}

fn same_fades(a: &Value, b: &Value) -> bool {
    same_fade(&a["fadeIn"], &b["fadeIn"]) && same_fade(&a["fadeOut"], &b["fadeOut"])
}

/// Puts a fade back exactly: the keys in `FADE_KEYS` order, the shape only when it is a preset, the legacy curve only for
/// a custom shape.
fn put_fade(reaper: &Reaper, item: Item, side: &str, fade: &Value) {
    let shape = fade["shape"].as_i64().unwrap_or(0);
    for key in FADE_KEYS {
        let (field, write) = match key {
            "LEN" => ("length", true),
            "SHAPE" => ("shape", shape >= 0),
            "DIR_NEW" => ("curve", true),
            "DIR2_NEW" => ("s", true),
            _ => ("legacy", shape < 0),
        };
        if write {
            reaper.set_item_value(item, &fade_key(side, key), fade[field].as_f64().unwrap_or(0.0));
        }
    }
}

/// A fade's length as seconds: given in seconds, or in beats counted from the edge it is at (forward from the start for a
/// fade in, back from the end for a fade out), under the tempo there.
fn fade_span(ctx: &Ctx, value: &Value, side: &str, start: f64, length: f64) -> Done<Option<f64>> {
    let get = |key: &str| -> Done<Option<f64>> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .map(Some)
                .ok_or_else(|| bad(format!("{side}.{key} must be a number from 0"))),
        }
    };
    match (get("lengthBeats")?, get("length")?) {
        (Some(_), Some(_)) => Err(bad(format!("{side}: pass lengthBeats or length, not both"))),
        (None, seconds) => Ok(seconds),
        (Some(qn), None) => {
            let reaper = ctx.reaper;
            let qn_at = |time: f64| reaper.quarter_notes_at(ctx.proj, time).ok_or_else(|| failed("this REAPER can't turn time into beats"));
            let time_at =
                |qn: f64| reaper.time_at_quarter_notes(ctx.proj, qn).ok_or_else(|| failed("this REAPER can't turn beats into time"));
            if side == "fadeIn" {
                Ok(Some(time_at(qn_at(start)? + qn)? - start))
            } else {
                let end = start + length;
                Ok(Some(end - time_at(qn_at(end)? - qn)?))
            }
        }
    }
}

struct FadeAsk {
    length: Option<f64>,
    shape: Option<i64>,
    curve: Option<f64>,
    s: Option<f64>,
}

fn fade_ask(ctx: &Ctx, value: &Value, side: &str, item: Item) -> Done<Option<FadeAsk>> {
    if value.is_null() {
        return Ok(None);
    }
    if !value.is_object() {
        return Err(bad(format!("{side} must be {{length or lengthBeats, shape, curve, s}}")));
    }
    let reaper = ctx.reaper;
    let (start, length) = (reaper.item_value(item, "D_POSITION"), reaper.item_value(item, "D_LENGTH"));
    let span = fade_span(ctx, value, side, start, length)?;
    if span.is_some_and(|span| span > length + 1e-9) {
        return Err(bad(format!("{side}: a fade can't be longer than the item ({})", seconds_text(length))));
    }
    let shape = match value.get("shape").filter(|v| !v.is_null()) {
        None => None,
        Some(shape) => Some(shape.as_i64().filter(|n| (0..=6).contains(n)).ok_or_else(|| bad(format!("{side}.shape must be 0 to 6")))?),
    };
    let unit = |key: &str| -> Done<Option<f64>> {
        match value.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => {
                v.as_f64().filter(|n| (-1.0..=1.0).contains(n)).map(Some).ok_or_else(|| bad(format!("{side}.{key} must be from -1 to 1")))
            }
        }
    };
    let ask = FadeAsk { length: span, shape, curve: unit("curve")?, s: unit("s")? };
    if ask.length.is_none() && ask.shape.is_none() && ask.curve.is_none() && ask.s.is_none() {
        return Err(bad(format!("{side}: pass its length (or lengthBeats), shape, curve or s")));
    }
    Ok(Some(ask))
}

fn fade_asks(ctx: &Ctx, input: &Value, item: Item) -> Done<Vec<(&'static str, FadeAsk)>> {
    let mut asks = Vec::new();
    for (key, side) in [("fadeIn", "IN"), ("fadeOut", "OUT")] {
        if let Some(ask) = fade_ask(ctx, input.get(key).unwrap_or(&Value::Null), key, item)? {
            asks.push((side, ask));
        }
    }
    if asks.is_empty() {
        return Err(bad("pass fadeIn, fadeOut or both: {length or lengthBeats, shape (0-6), curve, s}"));
    }
    Ok(asks)
}

fn write_asks(reaper: &Reaper, item: Item, asks: &[(&'static str, FadeAsk)]) {
    for (side, ask) in asks {
        if let Some(length) = ask.length {
            reaper.set_item_value(item, &fade_key(side, "LEN"), length);
        }
        if let Some(shape) = ask.shape {
            reaper.set_item_value(item, &fade_key(side, "SHAPE"), shape as f64);
        }
        if let Some(curve) = ask.curve {
            reaper.set_item_value(item, &fade_key(side, "DIR_NEW"), curve);
        }
        if let Some(s) = ask.s {
            reaper.set_item_value(item, &fade_key(side, "DIR2_NEW"), s);
        }
    }
    reaper.update_item(item);
}

fn asks_hold(after: &Value, asks: &[(&'static str, FadeAsk)]) -> bool {
    asks.iter().all(|(side, ask)| {
        let fade = &after[if *side == "IN" { "fadeIn" } else { "fadeOut" }];
        ask.length.is_none_or(|length| close(number(fade, "length"), length))
            // (a shape given with its own curve or S values is the nearest preset of them, as REAPER names it)
            && ask.shape.is_none_or(|shape| ask.curve.is_some() || ask.s.is_some() || fade["shape"].as_i64() == Some(shape))
            && ask.curve.is_none_or(|curve| close6(number(fade, "curve"), curve))
            && ask.s.is_none_or(|s| close6(number(fade, "s"), s))
    })
}

fn fade_title(ctx: &Ctx, item: Item, asks: &[(&'static str, FadeAsk)], after: Option<&Value>) -> String {
    let parts: Vec<String> = asks
        .iter()
        .map(|(side, ask)| {
            let word = if *side == "IN" { "fade in" } else { "fade out" };
            let length = ask.length.or_else(|| after.map(|a| number(&a[if *side == "IN" { "fadeIn" } else { "fadeOut" }], "length")));
            match length {
                Some(length) if length <= 0.0 => format!("no {word}"),
                Some(length) => format!("{word} {}", seconds_text(length)),
                None => format!("{word} reshaped"),
            }
        })
        .collect();
    format!("{}: {}", label_opening(ctx, item), parts.join(", "))
}

pub fn fade_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-fade")?;
    let asks = fade_asks(ctx, input, found.item)?;
    let now = fades_of(ctx.reaper, found.item);
    Ok(Preview {
        target: target(&found.guid),
        prior: now.clone(),
        fence: now,
        display: json!({"title": fade_title(ctx, found.item, &asks, None)}),
    })
}

pub fn fade_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-fade")?;
    let now = fades_of(ctx.reaper, found.item);
    if !same_fades(&now, fence) {
        return Err(changed(format!("{}'s fades changed since the preview", label_opening(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    let asks = fade_asks(ctx, input, found.item)?;
    let was = drawn(ctx, found.item);
    write_asks(ctx.reaper, found.item, &asks);
    let after = fades_of(ctx.reaper, found.item);
    if !asks_hold(&after, &asks) {
        put_fade(ctx.reaper, found.item, "IN", &now["fadeIn"]);
        put_fade(ctx.reaper, found.item, "OUT", &now["fadeOut"]);
        return Err(failed("REAPER didn't take that fade; the item's fades are as they were"));
    }
    let title = fade_title(ctx, found.item, &asks, Some(&after));
    let items = drawn_items(vec![was], vec![drawn(ctx, found.item)]);
    Ok(Applied {
        applied: after.clone(),
        display: json!({"title": title, "items": items}),
        unwritten: same_fades(&now, &after),
        ..Default::default()
    })
}

pub fn fade_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "item-fade")?;
    let now = fades_of(ctx.reaper, found.item);
    if same_fades(&now, restore.prior) {
        return Ok(Restored {
            restored: now,
            display: json!({"title": format!("{}'s fades are as they were", label_opening(ctx, found.item))}),
            unwritten: true,
        });
    }
    if !restore.discard && !same_fades(&now, restore.applied) {
        return Err(changed(format!("{}'s fades have been changed in REAPER since Kumi set them", label_opening(ctx, found.item))));
    }
    put_fade(ctx.reaper, found.item, "IN", &restore.prior["fadeIn"]);
    put_fade(ctx.reaper, found.item, "OUT", &restore.prior["fadeOut"]);
    ctx.reaper.update_item(found.item);
    let after = fades_of(ctx.reaper, found.item);
    if !same_fades(&after, restore.prior) {
        return Err(failed("REAPER didn't put the fades back exactly"));
    }
    Ok(Restored {
        restored: after,
        display: json!({"title": format!("{}'s fades put back", label_opening(ctx, found.item))}),
        unwritten: false,
    })
}

// ----- properties -----

/// One property of an item or a take that a change sets: how it is read, asked for, written, compared and said.
pub(super) struct Field<O: Copy> {
    pub key: &'static str,
    pub read: fn(&Reaper, O) -> Value,
    pub parse: fn(&Ctx, &Value) -> Done<Value>,
    pub write: fn(&Reaper, O, &Value) -> bool,
    pub same: fn(&Value, &Value) -> bool,
    /// What the title says of the value it is set to: "muted", "volume −3 dB".
    pub words: fn(&Value) -> String,
    /// The value as a request gives it, for the display's before and after: dB, not a gain; #rrggbb, not REAPER's number.
    pub show: fn(&Reaper, &Value) -> Value,
}

pub(super) fn show_same(_: &Reaper, value: &Value) -> Value {
    value.clone()
}

pub(super) fn show_db(_: &Reaper, gain: &Value) -> Value {
    json!(rows::db(gain.as_f64().unwrap_or(1.0)))
}

/// A colour stored as REAPER's number with its "set" flag: #rrggbb, or null for none.
pub(super) fn show_color(reaper: &Reaper, value: &Value) -> Value {
    let native = value.as_i64().unwrap_or(0) as i32;
    (native & 0x100_0000 != 0).then(|| reaper.color_hex(native)).flatten().map_or(Value::Null, Value::from)
}

/// A state's values as a request gives them.
pub(super) fn shown<O: Copy>(reaper: &Reaper, fields: &[&Field<O>], state: &Value) -> Value {
    let mut out = Map::new();
    for field in fields {
        out.insert(field.key.into(), (field.show)(reaper, &state[field.key]));
    }
    Value::Object(out)
}

fn same_number(a: &Value, b: &Value) -> bool {
    close(a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN))
}

/// A volume or a pan: 6 decimals in REAPER's saved state.
pub(super) fn same_number6(a: &Value, b: &Value) -> bool {
    close6(a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN))
}

fn same_exact(a: &Value, b: &Value) -> bool {
    a == b
}

/// A gain from dB, -150 or less being silence.
pub(super) fn gain_of(db: &Value, key: &str) -> Done<f64> {
    let db = db
        .as_f64()
        .filter(|db| db.is_finite() && (-150.0..=24.0).contains(db))
        .ok_or_else(|| bad(format!("{key} must be from -150 (silent) to 24")))?;
    Ok(if db <= -150.0 { 0.0 } else { 10f64.powf(db / 20.0) })
}

pub(super) fn color_of(ctx: &Ctx, value: &Value, key: &str) -> Done<Value> {
    match value {
        Value::Null => Ok(json!(0)),
        Value::String(hex) => {
            let digits = hex
                .strip_prefix('#')
                .filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| bad(format!("{key} must be #rrggbb, or null for none")))?;
            let channel = |at: usize| i32::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
            let native =
                ctx.reaper.color_to_native(channel(0), channel(2), channel(4)).ok_or_else(|| failed("this REAPER can't make a colour"))?;
            Ok(json!(native | 0x100_0000))
        }
        _ => Err(bad(format!("{key} must be #rrggbb, or null for none"))),
    }
}

pub(super) fn same_color(a: &Value, b: &Value) -> bool {
    super::tracks::colour_of(&json!({"color": a})) == super::tracks::colour_of(&json!({"color": b}))
}

fn flag(value: &Value, key: &str) -> Done<Value> {
    value.as_bool().map(Value::Bool).ok_or_else(|| bad(format!("{key} must be true or false")))
}

const TIMEBASES: [(&str, i64); 4] = [("project", -1), ("time", 0), ("beats", 1), ("beats-position", 2)];

pub(super) static ITEM_FIELDS: [Field<Item>; 7] = [
    Field {
        key: "mute",
        read: |reaper, item| json!(reaper.item_value(item, "B_MUTE") != 0.0),
        parse: |_, value| flag(value, "mute"),
        write: |reaper, item, value| reaper.set_item_value(item, "B_MUTE", if value == &Value::Bool(true) { 1.0 } else { 0.0 }),
        same: same_exact,
        words: |value| if value == &Value::Bool(true) { "muted".into() } else { "unmuted".into() },
        show: show_same,
    },
    Field {
        key: "volumeDb",
        read: |reaper, item| json!(reaper.item_value(item, "D_VOL")),
        parse: |_, value| gain_of(value, "volumeDb").map(|gain| json!(gain)),
        write: |reaper, item, value| reaper.set_item_value(item, "D_VOL", value.as_f64().unwrap_or(1.0)),
        same: same_number6,
        words: |value| format!("volume {} dB", super::tracks::db_text(rows::db(value.as_f64().unwrap_or(1.0)))),
        show: show_db,
    },
    Field {
        key: "color",
        read: |reaper, item| json!(reaper.item_value(item, "I_CUSTOMCOLOR") as i64),
        parse: |ctx, value| color_of(ctx, value, "color"),
        write: |reaper, item, value| reaper.set_item_value(item, "I_CUSTOMCOLOR", value.as_f64().unwrap_or(0.0)),
        same: same_color,
        words: |value| {
            if super::tracks::colour_of(&json!({"color": value})) == 0 {
                "colour cleared".into()
            } else {
                "colour changed".into()
            }
        },
        show: show_color,
    },
    Field {
        key: "loop",
        read: |reaper, item| json!(reaper.item_value(item, "B_LOOPSRC") != 0.0),
        parse: |_, value| flag(value, "loop"),
        write: |reaper, item, value| reaper.set_item_value(item, "B_LOOPSRC", if value == &Value::Bool(true) { 1.0 } else { 0.0 }),
        same: same_exact,
        words: |value| if value == &Value::Bool(true) { "loops its source".into() } else { "doesn't loop its source".into() },
        show: show_same,
    },
    Field {
        key: "lock",
        read: |reaper, item| json!(is_locked(reaper, item)),
        parse: |_, value| flag(value, "lock"),
        // Only the lock bit: REAPER keeps other flags beside it.
        write: |reaper, item, value| {
            let now = reaper.item_value(item, "C_LOCK") as i64;
            let wanted = if value == &Value::Bool(true) { now | 1 } else { now & !1 };
            reaper.set_item_value(item, "C_LOCK", wanted as f64)
        },
        same: same_exact,
        words: |value| if value == &Value::Bool(true) { "locked".into() } else { "unlocked".into() },
        show: show_same,
    },
    Field {
        key: "timebase",
        read: |reaper, item| json!(reaper.item_value(item, "C_BEATATTACHMODE") as i64),
        parse: |_, value| {
            let name = value.as_str().unwrap_or_default();
            TIMEBASES
                .iter()
                .find(|(word, _)| *word == name)
                .map(|(_, mode)| json!(mode))
                .ok_or_else(|| bad("timebase must be project, time, beats or beats-position"))
        },
        write: |reaper, item, value| reaper.set_item_value(item, "C_BEATATTACHMODE", value.as_f64().unwrap_or(-1.0)),
        same: same_exact,
        words: |value| {
            let name = TIMEBASES.iter().find(|(_, mode)| Some(*mode) == value.as_i64()).map_or("its own", |(word, _)| word);
            format!("timebase {name}")
        },
        show: |_, value| TIMEBASES.iter().find(|(_, mode)| Some(*mode) == value.as_i64()).map_or(Value::Null, |(word, _)| json!(word)),
    },
    Field {
        key: "snapOffset",
        read: |reaper, item| json!(reaper.item_value(item, "D_SNAPOFFSET")),
        parse: |_, value| {
            value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .map(|n| json!(n))
                .ok_or_else(|| bad("snapOffset must be seconds from the item's start, from 0"))
        },
        write: |reaper, item, value| reaper.set_item_value(item, "D_SNAPOFFSET", value.as_f64().unwrap_or(0.0)),
        same: same_number,
        words: |value| format!("snap offset {}", seconds_text(value.as_f64().unwrap_or(0.0))),
        show: show_same,
    },
];

/// The fields a request names, with what each is asked to become.
pub(super) fn asked_fields<'f, O: Copy>(ctx: &Ctx, input: &Value, fields: &'f [Field<O>], what: &str) -> Done<Vec<(&'f Field<O>, Value)>> {
    let asked: Vec<(&Field<O>, Value)> = fields
        .iter()
        .filter_map(|field| input.get(field.key).map(|value| (field, value)))
        .map(|(field, value)| Ok((field, (field.parse)(ctx, value)?)))
        .collect::<Done<_>>()?;
    if asked.is_empty() {
        let keys: Vec<&str> = fields.iter().map(|field| field.key).collect();
        return Err(bad(format!("{what}: pass one or more of {}", keys.join(", "))));
    }
    Ok(asked)
}

pub(super) fn state_of<O: Copy>(reaper: &Reaper, object: O, fields: &[&Field<O>]) -> Value {
    let mut state = Map::new();
    for field in fields {
        state.insert(field.key.into(), (field.read)(reaper, object));
    }
    Value::Object(state)
}

pub(super) fn same_state<O: Copy>(fields: &[&Field<O>], a: &Value, b: &Value) -> bool {
    fields.iter().all(|field| (field.same)(&a[field.key], &b[field.key]))
}

/// Writes each field's value from `state`; whether REAPER took them all.
pub(super) fn write_state<O: Copy>(reaper: &Reaper, object: O, fields: &[&Field<O>], state: &Value) -> bool {
    fields.iter().all(|field| (field.write)(reaper, object, &state[field.key]))
}

pub(super) fn words_of<O: Copy>(fields: &[&Field<O>], state: &Value) -> String {
    fields.iter().map(|field| (field.words)(&state[field.key])).collect::<Vec<_>>().join(", ")
}

fn props_plan(ctx: &Ctx, input: &Value, found: &Found) -> Done<(Vec<&'static Field<Item>>, Value)> {
    let asked = asked_fields(ctx, input, &ITEM_FIELDS, "item-props")?;
    let fields: Vec<&Field<Item>> = asked.iter().map(|(field, _)| *field).collect();
    let mut wanted = Map::new();
    for (field, value) in &asked {
        wanted.insert(field.key.into(), value.clone());
    }
    // A locked item stays as it is, but for unlocking it (with other changes in the same step).
    let unlocks = wanted.get("lock") == Some(&Value::Bool(false));
    if is_locked(ctx.reaper, found.item) && !unlocks && fields.iter().any(|field| field.key != "lock") {
        return Err(bad(format!(
            "{} is locked in REAPER: unlock it first (lock: false, with the other changes or before them)",
            label(ctx, found.item)
        )));
    }
    if let Some(snap) = wanted.get("snapOffset").and_then(Value::as_f64) {
        if snap > ctx.reaper.item_value(found.item, "D_LENGTH") + 1e-9 {
            return Err(bad("snapOffset must be inside the item (seconds from its start)"));
        }
    }
    Ok((fields, Value::Object(wanted)))
}

pub fn props_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-props")?;
    let (fields, wanted) = props_plan(ctx, input, &found)?;
    let now = state_of(ctx.reaper, found.item, &fields);
    let mut display = json!({
        "title": format!("{}: {}", label_opening(ctx, found.item), words_of(&fields, &wanted)),
        "before": shown(ctx.reaper, &fields, &now),
        "after": shown(ctx.reaper, &fields, &wanted),
    });
    if same_state(&fields, &now, &wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(&found.guid), prior: now.clone(), fence: now, display })
}

pub fn props_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-props")?;
    let (fields, wanted) = props_plan(ctx, input, &found)?;
    let now = state_of(ctx.reaper, found.item, &fields);
    if !same_state(&fields, &now, fence) {
        return Err(changed(format!("{} changed since the preview", label_opening(ctx, found.item))));
    }
    // The lock first when it opens, last when it closes: nothing else is written to a locked item.
    let mut order = fields.clone();
    order.sort_by_key(|field| match (field.key, &wanted["lock"]) {
        ("lock", Value::Bool(false)) => 0,
        ("lock", _) => 2,
        _ => 1,
    });
    write_state(ctx.reaper, found.item, &order, &wanted);
    ctx.reaper.update_item(found.item);
    let after = state_of(ctx.reaper, found.item, &fields);
    if !same_state(&fields, &after, &wanted) {
        write_state(ctx.reaper, found.item, &order, &now);
        return Err(failed("REAPER didn't take all of that; the item is as it was"));
    }
    let title = format!("{}: {}", label_opening(ctx, found.item), words_of(&fields, &after));
    Ok(Applied {
        applied: after.clone(),
        display: json!({"title": title, "before": shown(ctx.reaper, &fields, &now), "after": shown(ctx.reaper, &fields, &after)}),
        unwritten: same_state(&fields, &now, &wanted),
        ..Default::default()
    })
}

/// The fields a journalled state holds (those the change set).
pub(super) fn fields_in<'f, O: Copy>(fields: &'f [Field<O>], state: &Value) -> Vec<&'f Field<O>> {
    fields.iter().filter(|field| state.get(field.key).is_some()).collect()
}

pub fn props_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "item-props")?;
    let fields = fields_in(&ITEM_FIELDS, restore.prior);
    if fields.is_empty() {
        return Err(bad("prior: the item's properties before the change"));
    }
    let now = state_of(ctx.reaper, found.item, &fields);
    let name = label_opening(ctx, found.item);
    if same_state(&fields, &now, restore.prior) {
        return Ok(Restored { restored: now, display: json!({"title": format!("{name} is as it was")}), unwritten: true });
    }
    if !restore.discard && !same_state(&fields, &now, restore.applied) {
        return Err(changed(format!("{name} has been changed in REAPER since Kumi set it")));
    }
    let mut order = fields.clone();
    order.sort_by_key(|field| match (field.key, &restore.prior["lock"]) {
        ("lock", Value::Bool(false)) => 0,
        ("lock", _) => 2,
        _ => 1,
    });
    // A lock is opened first, so the rest can be put back; one that isn't Kumi's to put back (the producer's, set since)
    // is closed again after.
    let lock = ctx.reaper.item_value(found.item, "C_LOCK");
    let locked = is_locked(ctx.reaper, found.item);
    if locked {
        ctx.reaper.set_item_value(found.item, "C_LOCK", (lock as i64 & !1) as f64);
    }
    write_state(ctx.reaper, found.item, &order, restore.prior);
    if locked && !fields.iter().any(|field| field.key == "lock") {
        ctx.reaper.set_item_value(found.item, "C_LOCK", lock);
    }
    ctx.reaper.update_item(found.item);
    let after = state_of(ctx.reaper, found.item, &fields);
    if !same_state(&fields, &after, restore.prior) {
        return Err(failed(format!("REAPER kept some of {name}'s properties as Kumi had set them")));
    }
    Ok(Restored {
        restored: after,
        display: json!({"title": format!("{name}: {} (put back)", words_of(&fields, restore.prior))}),
        unwritten: false,
    })
}

// ----- groups -----

fn group_items(ctx: &Ctx, input: &Value) -> Done<Vec<Found>> {
    let list = input
        .get("itemRefs")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty() && l.len() <= 500)
        .ok_or_else(|| bad("itemRefs must list 1 to 500 items"))?;
    let mut found: Vec<Found> = Vec::new();
    for value in list {
        let item = find(ctx, value, "itemRefs")?;
        if found.iter().any(|f| f.guid == item.guid) {
            return Err(bad("an item is named twice in itemRefs"));
        }
        found.push(item);
    }
    Ok(found)
}

fn groups_of(ctx: &Ctx, items: &[Found]) -> Value {
    items.iter().map(|f| json!([f.guid, ctx.reaper.item_value(f.item, "I_GROUPID") as i64])).collect::<Vec<_>>().into()
}

/// A group id no item of the project has: one past the highest, as REAPER's own Group items makes.
fn new_group(ctx: &Ctx) -> i64 {
    let reaper = ctx.reaper;
    let mut highest = 0i64;
    for track in (0..reaper.track_count(ctx.proj)).filter_map(|n| reaper.track_at(ctx.proj, n)) {
        for item in (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n)) {
            highest = highest.max(reaper.item_value(item, "I_GROUPID") as i64);
        }
    }
    highest + 1
}

fn group_title(count: usize, group: Option<i64>) -> String {
    match group {
        Some(id) => format!("Grouped {count} items (group {id})"),
        None => format!("Took {count} item{} out of {} group", if count == 1 { "" } else { "s" }, if count == 1 { "its" } else { "their" }),
    }
}

fn group_wanted(input: &Value, count: usize) -> Done<bool> {
    let grouped = input
        .get("grouped")
        .and_then(Value::as_bool)
        .ok_or_else(|| bad("grouped must be true (one new group) or false (out of their groups)"))?;
    if grouped && count < 2 {
        return Err(bad("a group needs two items or more"));
    }
    Ok(grouped)
}

pub fn group_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let items = group_items(ctx, input)?;
    let grouped = group_wanted(input, items.len())?;
    let now = groups_of(ctx, &items);
    let mut display = json!({"title": group_title(items.len(), grouped.then(|| new_group(ctx)))});
    if !grouped && now.as_array().is_some_and(|list| list.iter().all(|entry| entry[1] == 0)) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(&items[0].guid), prior: json!({"groups": now.clone()}), fence: json!({"groups": now}), display })
}

pub fn group_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let items = group_items(ctx, input).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    let grouped = group_wanted(input, items.len())?;
    if groups_of(ctx, &items) != fence["groups"] {
        return Err(changed("those items' groups changed since the preview"));
    }
    for found in &items {
        refuse_locked(ctx, found.item)?;
    }
    let id = if grouped { new_group(ctx) } else { 0 };
    for found in &items {
        ctx.reaper.set_item_value(found.item, "I_GROUPID", id as f64);
    }
    let after = groups_of(ctx, &items);
    if after.as_array().is_some_and(|list| list.iter().any(|entry| entry[1].as_i64() != Some(id))) {
        for (found, entry) in items.iter().zip(fence["groups"].as_array().into_iter().flatten()) {
            ctx.reaper.set_item_value(found.item, "I_GROUPID", entry[1].as_f64().unwrap_or(0.0));
        }
        return Err(failed("REAPER didn't take the group; the items are as they were"));
    }
    Ok(Applied {
        applied: json!({"group": id, "groups": after}),
        display: json!({"title": group_title(items.len(), grouped.then_some(id))}),
        ..Default::default()
    })
}

pub fn group_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let prior = restore.prior["groups"].as_array().ok_or_else(|| bad("prior: the items' groups before"))?;
    let id = restore.applied["group"].as_i64().unwrap_or(0);
    let mut found = Vec::new();
    for entry in prior {
        let guid = entry[0].as_str().unwrap_or_default();
        match item_by_guid(ctx, guid) {
            Some(item) => found.push((item, entry[1].as_i64().unwrap_or(0))),
            None if restore.discard => {}
            None => return Err(gone("an item of that group isn't in the project any more")),
        }
    }
    let back = found.iter().all(|(item, before)| ctx.reaper.item_value(*item, "I_GROUPID") as i64 == *before);
    if back {
        return Ok(Restored {
            restored: json!({"groups": prior}),
            display: json!({"title": "The items' groups are as they were"}),
            unwritten: true,
        });
    }
    if !restore.discard && found.iter().any(|(item, _)| ctx.reaper.item_value(*item, "I_GROUPID") as i64 != id) {
        return Err(changed("those items' groups have been changed in REAPER since Kumi set them"));
    }
    for (item, before) in &found {
        ctx.reaper.set_item_value(*item, "I_GROUPID", *before as f64);
    }
    Ok(Restored {
        restored: json!({"groups": prior}),
        display: json!({"title": format!("{} items' groups put back", found.len())}),
        unwritten: false,
    })
}

// ----- deleting -----

pub fn delete_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-delete")?;
    let print = chunks::item_print(ctx.reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    let mut display = json!({"title": format!("Deleted {}", label(ctx, found.item))});
    if is_locked(ctx.reaper, found.item) {
        display["note"] = "It is locked in REAPER: unlock it first (item_props lock: false).".into();
    }
    Ok(Preview { target: target(&found.guid), prior: json!({"print": print}), fence: json!({"print": print}), display })
}

pub fn delete_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-delete")?;
    let reaper = ctx.reaper;
    let chunk = reaper
        .item_chunk(found.item)
        .ok_or_else(|| failed("REAPER wouldn't give the item's state, so Kumi won't delete what it could not put back"))?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() {
        return Err(changed(format!("{} changed since the preview", label(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    if chunk.len() > chunks::MOST_KEPT {
        return Err(failed(format!(
            "this item's state is {} MB, more than the {} MB Kumi keeps of a deleted item: Kumi won't delete what it can't put back",
            chunk.len() >> 20,
            chunks::MOST_KEPT >> 20
        )));
    }
    let title = format!("Deleted {}", label(ctx, found.item));
    let was = drawn(ctx, found.item);
    let material = json!({
        "chunk": chunk,
        "track": reaper.track_guid(found.track),
        "selected": reaper.item_value(found.item, "B_UISEL") != 0.0,
        "tempo": marks::tempo_print(reaper, ctx.proj),
    });
    if !reaper.delete_item(found.track, found.item) {
        return Err(failed("REAPER wouldn't delete the item"));
    }
    settle_lanes(ctx, &[Some(found.track)]);
    let display = json!({"title": title, "items": drawn_items(vec![was], vec![])});
    Ok(Applied { applied: json!({"deleted": true}), material: Some(material), display, ..Default::default() })
}

/// Makes an item again from its chunk (its GUIDs with it) on the track with this GUID.
pub(super) fn remake(ctx: &Ctx, track_guid: &str, chunk: &str, guid: &str) -> Done<Item> {
    let reaper = ctx.reaper;
    let track = track_by_guid(ctx, track_guid).ok_or_else(|| gone("the track the item was on isn't in the project any more"))?;
    let item = reaper.add_item(track).ok_or_else(|| failed("REAPER wouldn't make an item to put it back in"))?;
    if !reaper.set_item_chunk(item, chunk) || reaper.item_guid(item) != guid {
        // Not what was there: nothing half-made stays.
        reaper.delete_item(track, item);
        return Err(failed("REAPER wouldn't put the item's state back"));
    }
    settle_lanes(ctx, &[Some(track)]);
    reaper.update_item(item);
    ctx.reads.registry().note_item(ctx.proj, guid, item);
    Ok(item)
}

/// Whether anything but `except` is selected among the project's items.
pub(super) fn other_items_selected(ctx: &Ctx, except: Item) -> bool {
    let (count, items) = ctx.reaper.selected_items_up_to(ctx.proj, 2);
    count > 1 || items.iter().any(|item| *item != except)
}

pub fn delete_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let chunk = material["chunk"].as_str().ok_or_else(|| bad("material: the item's state"))?;
    let track_guid = material["track"].as_str().ok_or_else(|| bad("material: the item's track"))?;
    let guid = guid_of_ref(&restore.target["ref"], "target")?;
    if let Some(item) = item_by_guid(ctx, &guid) {
        let back = chunks::item_print(ctx.reaper, item).as_deref() == restore.prior["print"].as_str();
        if back {
            return Ok(Restored {
                restored: json!({"ref": refs::item_ref(&guid)}),
                display: json!({"title": format!("{} is back already", label_opening(ctx, item))}),
                unwritten: true,
            });
        }
        // (with discard: it is there, but not as it was; nothing is written over it)
        if restore.discard {
            let name = label_opening(ctx, item);
            return Ok(Restored {
                restored: json!({"ref": refs::item_ref(&guid)}),
                display: json!({
                    "title": format!("{name} is in the project again, changed since"),
                    "note": "It is in the project again, changed since it was deleted: Kumi left it as it is.",
                }),
                unwritten: true,
            });
        }
        return Err(changed("an item with that id is in the project again"));
    }
    marks::tempo_unmoved(ctx, material, restore.discard, "the item")?;
    let item = remake(ctx, track_guid, chunk, &guid)?;
    // The producer's selection stands: the item is selected again only where nothing else is.
    let select = material["selected"] == true && !other_items_selected(ctx, item);
    ctx.reaper.set_item_value(item, "B_UISEL", if select { 1.0 } else { 0.0 });
    Ok(Restored {
        restored: json!({"ref": refs::item_ref(&guid)}),
        display: json!({"title": format!("{} put back", label_opening(ctx, item))}),
        unwritten: false,
    })
}

// ----- copying -----

struct CopyPlan {
    found: Found,
    track: Track,
    track_guid: String,
    position: f64,
    lane: Option<i64>,
}

fn copy_plan(ctx: &Ctx, input: &Value) -> Done<CopyPlan> {
    let found = find(ctx, &input["ref"], "item-duplicate")?;
    let reaper = ctx.reaper;
    let (track, track_guid) = match input.get("toRef").filter(|v| !v.is_null()) {
        Some(value) => track_of(ctx, value, "toRef")?,
        None => (found.track, reaper.track_guid(found.track)),
    };
    // Right after the item by default.
    let position = marks::moment(ctx, input, "positionBeats", "position")?
        .unwrap_or(reaper.item_value(found.item, "D_POSITION") + reaper.item_value(found.item, "D_LENGTH"));
    let lane = match lane_input(ctx, input, track)? {
        Some(lane) => Some(lane),
        None => lane_count(reaper, track).map(|count| (reaper.item_value(found.item, "I_FIXEDLANE") as i64).min(count - 1).max(0)),
    };
    Ok(CopyPlan { found, track, track_guid, position, lane })
}

fn copy_title(ctx: &Ctx, copy: &CopyPlan) -> String {
    let mut title = format!("Copied {} to {}", label(ctx, copy.found.item), marks::bar_text(ctx.reaper, ctx.proj, copy.position));
    if copy.track != copy.found.track {
        title += &format!(" on {}", quoted(&super::track_name(ctx.reaper, copy.track)));
    }
    title
}

pub fn copy_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let copy = copy_plan(ctx, input)?;
    let print = chunks::item_print(ctx.reaper, copy.found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    Ok(Preview {
        target: json!({"ref": null, "kind": "item"}),
        prior: json!({"items": ctx.reaper.track_item_count(copy.track)}),
        fence: json!({"print": print, "track": copy.track_guid}),
        display: json!({"title": copy_title(ctx, &copy)}),
    })
}

pub fn copy_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let copy = copy_plan(ctx, input).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    let reaper = ctx.reaper;
    let chunk = reaper.item_chunk(copy.found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() || fence["track"].as_str() != Some(copy.track_guid.as_str()) {
        return Err(changed(format!("{} changed since the preview", label(ctx, copy.found.item))));
    }
    let item = reaper.add_item(copy.track).ok_or_else(|| failed("REAPER wouldn't make an item for the copy"))?;
    if !reaper.set_item_chunk(item, &chunks::without_ids(&chunk)) {
        reaper.delete_item(copy.track, item);
        return Err(failed("REAPER wouldn't take the copy's state: nothing was made"));
    }
    // (a chunk without ids gets new ones from REAPER: lab, 7.82, the takes', their sources' and FX's too)
    let guid = reaper.item_guid(item);
    if guid.is_empty() || guid == copy.found.guid {
        reaper.delete_item(copy.track, item);
        return Err(failed("REAPER gave the copy the original's id: nothing was made"));
    }
    reaper.set_item_value(item, "D_POSITION", copy.position);
    reaper.set_item_value(item, "B_UISEL", 0.0);
    if let Some(lane) = copy.lane {
        reaper.set_item_value(item, "I_FIXEDLANE", lane as f64);
        reaper.update_item_lanes(ctx.proj);
    }
    reaper.update_item(item);
    ctx.reads.registry().note_item(ctx.proj, &guid, item);
    let item =
        item_by_guid(ctx, &guid).ok_or_else(|| ("uncertain".to_string(), "the copy went as it was made: read the track".to_string()))?;
    let print = chunks::item_print(reaper, item).ok_or_else(|| failed("REAPER wouldn't give the copy's state"))?;
    Ok(Applied {
        applied: json!({"print": print, "position": reaper.item_value(item, "D_POSITION")}),
        created: Some(json!({"ref": refs::item_ref(&guid), "kind": "item"})),
        display: json!({
            "title": copy_title(ctx, &copy),
            "items": drawn_items(vec![drawn(ctx, copy.found.item)], vec![drawn(ctx, copy.found.item), drawn(ctx, item)]),
        }),
        ..Default::default()
    })
}

// ----- the producer's selection, around REAPER's actions on selected items -----

/// The items the producer has selected, by GUID: what an action on selected items is run between.
pub(super) fn selected_items(ctx: &Ctx) -> Vec<String> {
    let reaper = ctx.reaper;
    let mut guids = Vec::new();
    for track in (0..reaper.track_count(ctx.proj)).filter_map(|n| reaper.track_at(ctx.proj, n)) {
        for item in (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n)) {
            if reaper.item_value(item, "B_UISEL") != 0.0 {
                guids.push(reaper.item_guid(item));
            }
        }
    }
    guids
}

/// Selects exactly these items.
pub(super) fn select_only(ctx: &Ctx, items: &[Item]) {
    ctx.reaper.select_all_items(ctx.proj, false);
    for item in items {
        ctx.reaper.set_item_selected(*item, true);
    }
}

/// Selects again the items the producer had selected (those still there), and nothing else.
pub(super) fn reselect(ctx: &Ctx, guids: &[String]) {
    ctx.reaper.select_all_items(ctx.proj, false);
    for guid in guids {
        if let Some(item) = item_by_guid(ctx, guid) {
            ctx.reaper.set_item_selected(item, true);
        }
    }
}

/// Runs one of REAPER's main actions on exactly `items`, leaving the producer's selection as it was. The action is
/// checked by the name it has in this REAPER first: an action of another name under that number is not run.
pub(super) fn run_on_items(ctx: &Ctx, items: &[Item], command: i32, name: &str) -> Done<()> {
    let reaper = ctx.reaper;
    if reaper.action_name(command).as_deref() != Some(name) {
        return Err(failed(format!("this REAPER has no action {command} named “{name}”, so Kumi won't run it")));
    }
    let selected = selected_items(ctx);
    reaper.prevent_ui_refresh(1);
    select_only(ctx, items);
    let ran = reaper.run_action(command);
    reselect(ctx, &selected);
    reaper.prevent_ui_refresh(-1);
    if !ran {
        return Err(failed(format!("REAPER wouldn't run “{name}”")));
    }
    Ok(())
}
