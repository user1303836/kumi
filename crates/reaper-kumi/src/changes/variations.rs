//! Takes of the variations tool (#401, SFX variations): an item copied with its own settings, rewritten, or dropped with
//! others, each one change, so Kumi's undo takes a take back whole.
//!
//! `item-variation` copies an item (sharing nothing with it) to a position, with what makes it a take: its length and start
//! offset, its take's pitch, rate, volume and name, its fades, and for a MIDI take its whole note list. Its undo deletes the
//! copy while it is as made. `item-variation-set` writes those settings over a take; its undo puts the take's whole state back
//! while it is as written. `items-delete` deletes several items; its undo makes each again, with its ids.

use serde_json::{json, Value};

use super::items::{
    changed, drawn, drawn_items, find, find_for_apply, gone, guid_of_ref, item_by_guid, label, label_opening, put_chunk, remake,
    settle_lanes, target,
};
use super::{chunks, marks, notes, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed, take_start_qn};
use crate::reaper::{Item, Take};
use crate::refs;

/// The most items one drop deletes.
const MOST_DROPPED: usize = 500;

/// A take's settings as asked: each only when given.
#[derive(Default)]
struct Settings {
    length: Option<f64>,
    offset: Option<f64>,
    pitch: Option<f64>,
    playrate: Option<f64>,
    preserve: Option<bool>,
    volume_db: Option<f64>,
    fade_in: Option<f64>,
    fade_out: Option<f64>,
    name: Option<String>,
    notes: Option<Vec<Value>>,
}

fn number(input: &Value, key: &str, low: f64, high: f64) -> Done<Option<f64>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && (low..=high).contains(n))
            .map(Some)
            .ok_or_else(|| bad(format!("{key}: a number from {low} to {high}"))),
    }
}

fn settings_of(input: &Value) -> Done<Settings> {
    let name = match input.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Some(name.clone()),
        Some(_) => return Err(bad("name: one line of at most 256 characters")),
    };
    let notes = match input.get("notes") {
        None | Some(Value::Null) => None,
        Some(Value::Array(list)) if list.len() <= 10_000 => Some(list.clone()),
        Some(_) => return Err(bad("notes: the take's whole note list, at most 10000")),
    };
    Ok(Settings {
        length: number(input, "length", 0.001, 86_400.0)?,
        offset: number(input, "startOffset", 0.0, 86_400.0)?,
        pitch: number(input, "pitch", -96.0, 96.0)?,
        playrate: number(input, "playrate", 0.01, 100.0)?,
        preserve: input.get("preservePitch").and_then(Value::as_bool),
        volume_db: number(input, "volumeDb", -150.0, 24.0)?,
        fade_in: number(input, "fadeIn", 0.0, 86_400.0)?,
        fade_out: number(input, "fadeOut", 0.0, 86_400.0)?,
        name,
        notes,
    })
}

/// Writes the settings on an item and its active take: what REAPER refused, if anything.
fn write(ctx: &Ctx, item: Item, take: Option<Take>, settings: &Settings) -> Done<()> {
    let reaper = ctx.reaper;
    if let Some(length) = settings.length {
        reaper.set_item_value(item, "D_LENGTH", length);
    }
    let length = reaper.item_value(item, "D_LENGTH");
    for (key, value) in [("D_FADEINLEN", settings.fade_in), ("D_FADEOUTLEN", settings.fade_out)] {
        if let Some(value) = value {
            reaper.set_item_value(item, key, value.min(length));
        }
    }
    let Some(take) = take else {
        return if settings.notes.is_some() || settings.pitch.is_some() || settings.name.is_some() {
            Err(failed("the item has no take to set"))
        } else {
            Ok(())
        };
    };
    for (key, value) in [("D_STARTOFFS", settings.offset), ("D_PITCH", settings.pitch), ("D_PLAYRATE", settings.playrate)] {
        if let Some(value) = value {
            reaper.set_take_value(take, key, value);
        }
    }
    if let Some(preserve) = settings.preserve {
        reaper.set_take_value(take, "B_PPITCH", if preserve { 1.0 } else { 0.0 });
    }
    if let Some(db) = settings.volume_db {
        reaper.set_take_value(take, "D_VOL", 10f64.powf(db / 20.0));
    }
    if let Some(name) = &settings.name {
        reaper.set_take_string(take, "P_NAME", name);
    }
    if let Some(list) = &settings.notes {
        if !reaper.take_is_midi(take) {
            return Err(bad("notes: the item's take isn't MIDI"));
        }
        let item_qn = take_start_qn(reaper, ctx.proj, take);
        let wanted = list.iter().map(|spec| notes::new_note(ctx, take, item_qn, spec)).collect::<Done<Vec<_>>>()?;
        let (now, events) = notes::read_all(ctx, take)?;
        notes::check_inside(ctx, take, item_qn, &events, &wanted)?;
        notes::write_notes(ctx, take, &events, &now, &wanted)?;
    }
    reaper.update_item(item);
    Ok(())
}

// ----- a take made -----

fn add_title(ctx: &Ctx, input: &Value, source: Item, position: f64) -> String {
    let name = input.get("name").and_then(Value::as_str).filter(|name| !name.is_empty());
    let what = name.map_or_else(|| format!("a variation of {}", label(ctx, source)), super::quoted);
    format!("Added {what} at {}", marks::bar_text(ctx.reaper, ctx.proj, position))
}

fn position_of(ctx: &Ctx, input: &Value) -> Done<f64> {
    marks::moment(ctx, input, "positionBeats", "position")?.ok_or_else(|| bad("position (seconds) or positionBeats: where the take goes"))
}

pub fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-variation")?;
    let position = position_of(ctx, input)?;
    settings_of(input)?;
    let print = chunks::item_print(ctx.reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    Ok(Preview {
        target: json!({"ref": null, "kind": "item"}),
        prior: json!({"items": ctx.reaper.track_item_count(found.track)}),
        fence: json!({"print": print}),
        display: json!({"title": add_title(ctx, input, found.item, position)}),
    })
}

pub fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-variation")?;
    let position = position_of(ctx, input)?;
    let settings = settings_of(input)?;
    let reaper = ctx.reaper;
    let chunk = reaper.item_chunk(found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() {
        return Err(changed(format!("{} changed since the preview", label(ctx, found.item))));
    }
    let item = reaper.add_item(found.track).ok_or_else(|| failed("REAPER wouldn't make an item for the take"))?;
    // (a chunk without ids gets new ones from REAPER: the copy shares nothing with its source, a MIDI copy isn't pooled)
    let guid = if reaper.set_item_chunk(item, &chunks::without_ids(&chunk)) { reaper.item_guid(item) } else { String::new() };
    if guid.is_empty() || guid == found.guid {
        reaper.delete_item(found.track, item);
        return Err(failed("REAPER wouldn't make the copy: nothing was made"));
    }
    reaper.set_item_value(item, "D_POSITION", position);
    reaper.set_item_value(item, "B_UISEL", 0.0);
    if let Err(fault) = write(ctx, item, reaper.active_take(item), &settings) {
        // Nothing half-made stays.
        reaper.delete_item(found.track, item);
        return Err(fault);
    }
    ctx.reads.registry().note_item(ctx.proj, &guid, item);
    let item =
        item_by_guid(ctx, &guid).ok_or_else(|| ("uncertain".to_string(), "the take went as it was made: read the track".to_string()))?;
    let print = chunks::item_print(reaper, item).ok_or_else(|| failed("REAPER wouldn't give the take's state"))?;
    let take_ref = reaper.active_take(item).map(|take| refs::take_ref(&reaper.take_guid(take)));
    Ok(Applied {
        applied: json!({"print": print, "position": reaper.item_value(item, "D_POSITION")}),
        created: Some(json!({"ref": refs::item_ref(&guid), "kind": "item", "takeRef": take_ref})),
        display: json!({
            "title": add_title(ctx, input, found.item, position),
            "items": drawn_items(vec![drawn(ctx, found.item)], vec![drawn(ctx, found.item), drawn(ctx, item)]),
        }),
        ..Default::default()
    })
}

// ----- a take rewritten -----

pub fn set_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "item-variation-set")?;
    settings_of(input)?;
    let print = chunks::item_print(ctx.reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    Ok(Preview {
        target: target(&found.guid),
        prior: json!({"print": print}),
        fence: json!({"print": print}),
        display: json!({"title": format!("Rewrote {}", label(ctx, found.item))}),
    })
}

pub fn set_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "item-variation-set")?;
    let settings = settings_of(input)?;
    let reaper = ctx.reaper;
    let chunk = reaper.item_chunk(found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() {
        return Err(changed(format!("{} changed since the preview", label(ctx, found.item))));
    }
    if chunk.len() > chunks::MOST_KEPT {
        return Err(failed("this take's state is too big for Kumi to keep, so Kumi won't write what it couldn't put back"));
    }
    let before = drawn(ctx, found.item);
    if let Err(fault) = write(ctx, found.item, reaper.active_take(found.item), &settings) {
        // (what was written before the refusal goes back with the state it had)
        let _ = put_chunk(ctx, found.item, &chunk);
        return Err(fault);
    }
    let print = chunks::item_print(reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the take's state"))?;
    let unwritten = Some(print.as_str()) == fence["print"].as_str();
    Ok(Applied {
        applied: json!({"print": print}),
        material: Some(json!({"chunk": chunk})),
        display: json!({
            "title": format!("Rewrote {}", label(ctx, found.item)),
            "items": drawn_items(vec![before], vec![drawn(ctx, found.item)]),
        }),
        unwritten,
        ..Default::default()
    })
}

pub fn set_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = guid_of_ref(&restore.target["ref"], "target")?;
    let item = item_by_guid(ctx, &guid).ok_or_else(|| gone("the take isn't in the project any more"))?;
    let print = chunks::item_print(ctx.reaper, item).ok_or_else(|| failed("REAPER wouldn't give the take's state"))?;
    if Some(print.as_str()) == restore.prior["print"].as_str() {
        return Ok(Restored {
            restored: json!({"ref": refs::item_ref(&guid)}),
            display: json!({"title": format!("{} is as it was already", label_opening(ctx, item))}),
            unwritten: true,
        });
    }
    if !restore.discard && Some(print.as_str()) != restore.applied["print"].as_str() {
        return Err(changed(format!("{} has been changed in REAPER since Kumi rewrote it", label(ctx, item))));
    }
    let chunk = restore.material.and_then(|material| material["chunk"].as_str()).ok_or_else(|| bad("material: the take's state"))?;
    put_chunk(ctx, item, chunk)?;
    Ok(Restored {
        restored: json!({"ref": refs::item_ref(&guid)}),
        display: json!({"title": format!("{} put back as it was", label_opening(ctx, item))}),
        unwritten: false,
    })
}

// ----- takes dropped -----

/// The items a drop names, each found, none twice.
fn dropped(ctx: &Ctx, input: &Value) -> Done<Vec<super::items::Found>> {
    let list = input.get("refs").and_then(Value::as_array).ok_or_else(|| bad("refs: the items to delete"))?;
    if list.is_empty() || list.len() > MOST_DROPPED {
        return Err(bad(format!("refs: 1 to {MOST_DROPPED} items")));
    }
    let mut out: Vec<super::items::Found> = Vec::new();
    for reference in list {
        let found = find(ctx, reference, "items-delete")?;
        if out.iter().any(|other| other.guid == found.guid) {
            return Err(bad("refs: an item named twice"));
        }
        out.push(found);
    }
    Ok(out)
}

pub fn drop_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = dropped(ctx, input)?;
    let prints: Vec<Value> = found.iter().map(|f| json!(chunks::item_print(ctx.reaper, f.item))).collect();
    let guids: Vec<&str> = found.iter().map(|f| f.guid.as_str()).collect();
    Ok(Preview {
        target: json!({"ref": null, "kind": "item", "guids": guids}),
        prior: json!({"prints": prints}),
        fence: json!({"prints": prints}),
        display: json!({"title": format!("Deleted {} item{}", found.len(), if found.len() == 1 { "" } else { "s" })}),
    })
}

pub fn drop_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = dropped(ctx, input).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    let reaper = ctx.reaper;
    let mut kept = Vec::new();
    for (k, f) in found.iter().enumerate() {
        let chunk = reaper
            .item_chunk(f.item)
            .ok_or_else(|| failed("REAPER wouldn't give an item's state, so Kumi won't delete what it could not put back"))?;
        if Some(chunks::print_of(&chunk).as_str()) != fence["prints"][k].as_str() {
            return Err(changed(format!("{} changed since the preview", label(ctx, f.item))));
        }
        if chunk.len() > chunks::MOST_KEPT {
            return Err(failed(format!(
                "{}'s state is too big for Kumi to keep, so Kumi won't delete what it couldn't put back",
                label(ctx, f.item)
            )));
        }
        kept.push(json!({"guid": f.guid, "chunk": chunk, "track": reaper.track_guid(f.track), "selected": reaper.item_value(f.item, "B_UISEL") != 0.0}));
    }
    let before: Vec<Value> = found.iter().map(|f| drawn(ctx, f.item)).collect();
    // (each found again by its id as it goes: a pointer taken before another item went may not be that item any more)
    for f in &found {
        let deleted = item_by_guid(ctx, &f.guid).is_some_and(|item| reaper.delete_item(f.track, item));
        if !deleted {
            return Err(failed("REAPER wouldn't delete an item"));
        }
    }
    let tracks: Vec<Option<crate::reaper::Track>> = found.iter().map(|f| Some(f.track)).collect();
    settle_lanes(ctx, &tracks);
    Ok(Applied {
        applied: json!({"deleted": found.len()}),
        material: Some(json!({"items": kept, "tempo": marks::tempo_print(reaper, ctx.proj)})),
        display: json!({"title": format!("Deleted {} item{}", found.len(), if found.len() == 1 { "" } else { "s" }), "items": drawn_items(before, vec![])}),
        ..Default::default()
    })
}

pub fn drop_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the drop deleted"))?;
    let items = material["items"].as_array().ok_or_else(|| bad("material: the items' states"))?;
    // (one there again with its id: as it was, it is back already; otherwise nothing is made over it)
    let mut missing = Vec::new();
    for (k, kept) in items.iter().enumerate() {
        let guid = kept["guid"].as_str().unwrap_or_default();
        match item_by_guid(ctx, guid) {
            Some(item) if restore.discard || chunks::item_print(ctx.reaper, item).as_deref() == restore.prior["prints"][k].as_str() => {}
            Some(_) => return Err(changed("an item with one of those ids is in the project again")),
            None => missing.push(kept),
        }
    }
    if missing.is_empty() {
        return Ok(Restored {
            restored: json!({"back": items.len()}),
            display: json!({"title": "The items are back already"}),
            unwritten: true,
        });
    }
    marks::tempo_unmoved(ctx, material, restore.discard, "the items")?;
    for kept in &missing {
        let guid = kept["guid"].as_str().unwrap_or_default();
        let item = remake(ctx, kept["track"].as_str().unwrap_or_default(), kept["chunk"].as_str().unwrap_or_default(), guid)?;
        ctx.reaper.set_item_value(item, "B_UISEL", 0.0);
    }
    Ok(Restored {
        restored: json!({"back": missing.len()}),
        display: json!({"title": format!("{} item{} put back", missing.len(), if missing.len() == 1 { "" } else { "s" })}),
        unwritten: false,
    })
}
