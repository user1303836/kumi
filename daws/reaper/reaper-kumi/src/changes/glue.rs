//! Glue and render-in-place (M4), only when the producer asks: REAPER's own actions, run on exactly the items asked with
//! the producer's selection put back, and checked by the name they have in this REAPER first.
//!
//! Glue replaces the items with one that plays a new file: Kumi keeps each item's chunk, and its undo deletes what the
//! glue made (while unchanged) and makes the items again from their chunks, GUIDs and all. Render-in-place gives each
//! item a new take that plays its rendered audio: the item's old take stays, and the undo puts each item back as it was
//! (while unchanged). The files REAPER writes stay where it wrote them.

use serde_json::{json, Value};

use super::items::{self, changed, gone, Found};
use super::{chunks, quoted, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Item, Reaper};
use crate::refs;

const GLUE: (i32, &str) = (40362, "Item: Glue items, ignoring time selection");
const RENDER_TAKE_FX: (i32, &str) = (41999, "Item: Render items to new take");
const RENDER_ALL_FX: (i32, &str) = (40209, "Item: Apply track/take FX to items");
/// The most audio one glue or render renders, in seconds: REAPER renders it before it answers anything else (lab, 7.82:
/// about 30 ms a second with two plug-ins), and a change must answer well inside the bridge's deadline.
const MOST_RENDERED: f64 = 30.0;
const FILES_NOTE: &str = "REAPER wrote the new audio to a file beside the project's media; undo_change leaves the file there.";

fn listed(ctx: &Ctx, input: &Value, what: &str) -> Done<Vec<Found>> {
    let list = input
        .get("itemRefs")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty() && l.len() <= 500)
        .ok_or_else(|| bad("itemRefs must list 1 to 500 items"))?;
    let mut found: Vec<Found> = Vec::new();
    for value in list {
        let item = items::find(ctx, value, what)?;
        if found.iter().any(|f| f.guid == item.guid) {
            return Err(bad("an item is named twice in itemRefs"));
        }
        found.push(item);
    }
    Ok(found)
}

fn listed_for_apply(ctx: &Ctx, input: &Value, what: &str) -> Done<Vec<Found>> {
    listed(ctx, input, what).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })
}

/// Seconds of audio REAPER renders for these items: a MIDI item without take FX glues as MIDI and renders nothing.
fn rendered_seconds(reaper: &Reaper, items: &[Found], midi_counts: bool) -> f64 {
    items
        .iter()
        .filter(|f| {
            let take = reaper.active_take(f.item);
            midi_counts || !take.is_some_and(|take| reaper.take_is_midi(take) && reaper.fx_count(crate::reaper::FxOwner::Take(take)) == 0)
        })
        .map(|f| reaper.item_value(f.item, "D_LENGTH"))
        .sum()
}

fn too_long(seconds: f64, verb: &str) -> Done<()> {
    if seconds > MOST_RENDERED {
        return Err(bad(format!(
            "that's {seconds:.0} s of audio to {verb}: Kumi {verb}s up to {MOST_RENDERED:.0} s in one step (REAPER answers nothing else while it renders); do fewer items at a time"
        )));
    }
    Ok(())
}

fn prints(reaper: &Reaper, items: &[Found]) -> Done<Value> {
    items
        .iter()
        .map(|f| {
            chunks::item_print(reaper, f.item)
                .map(|print| json!([f.guid, print]))
                .ok_or_else(|| failed("REAPER wouldn't give an item's state"))
        })
        .collect::<Done<Vec<Value>>>()
        .map(Value::from)
}

fn refuse_locked(ctx: &Ctx, items: &[Found]) -> Done<()> {
    if let Some(locked) = items.iter().find(|f| items::is_locked(ctx.reaper, f.item)) {
        return Err(bad(format!("{} is locked in REAPER: unlock it first (item_props lock: false)", items::label(ctx, locked.item))));
    }
    Ok(())
}

/// Each item's chunk (by GUID, with its track's GUID), if all of them together are small enough to keep.
fn keep(ctx: &Ctx, items: &[Found], verb: &str) -> Done<Vec<Value>> {
    let mut total = 0usize;
    let mut kept = Vec::new();
    for found in items {
        let chunk = ctx.reaper.item_chunk(found.item).ok_or_else(|| failed("REAPER wouldn't give an item's state"))?;
        total += chunk.len();
        if total > chunks::MOST_KEPT {
            return Err(failed(format!(
                "these items' state is over the {} MB Kumi keeps to put them back: Kumi won't {verb} what it can't put back",
                chunks::MOST_KEPT >> 20
            )));
        }
        kept.push(json!({"guid": found.guid, "track": ctx.reaper.track_guid(found.track), "chunk": chunk}));
    }
    Ok(kept)
}

fn where_words(ctx: &Ctx, items: &[Found]) -> String {
    let mut tracks: Vec<crate::reaper::Track> = Vec::new();
    for found in items {
        if !tracks.contains(&found.track) {
            tracks.push(found.track);
        }
    }
    match tracks.as_slice() {
        [one] => format!("on {}", quoted(&super::track_name(ctx.reaper, *one))),
        many => format!("on {} tracks", many.len()),
    }
}

fn count_words(count: usize) -> String {
    format!("{count} item{}", if count == 1 { "" } else { "s" })
}

// ----- glue -----

pub fn glue_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = listed(ctx, input, "items-glue")?;
    too_long(rendered_seconds(ctx.reaper, &found, false), "glue")?;
    let now = prints(ctx.reaper, &found)?;
    Ok(Preview {
        target: items::target(&found[0].guid),
        prior: json!({"prints": now.clone()}),
        fence: json!({"prints": now}),
        display: json!({"title": format!("Glued {} {}", count_words(found.len()), where_words(ctx, &found)), "note": FILES_NOTE}),
    })
}

pub fn glue_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = listed_for_apply(ctx, input, "items-glue")?;
    let reaper = ctx.reaper;
    if prints(reaper, &found)? != fence["prints"] {
        return Err(changed("those items changed since the preview"));
    }
    refuse_locked(ctx, &found)?;
    too_long(rendered_seconds(reaper, &found, false), "glue")?;
    let originals = keep(ctx, &found, "glue")?;
    let title = format!("Glued {} {}", count_words(found.len()), where_words(ctx, &found));
    // What each track holds before: what is new after is the glue's.
    let mut tracks: Vec<crate::reaper::Track> = Vec::new();
    for f in &found {
        if !tracks.contains(&f.track) {
            tracks.push(f.track);
        }
    }
    let held = |track: crate::reaper::Track| -> Vec<(String, Item)> {
        (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n)).map(|item| (reaper.item_guid(item), item)).collect()
    };
    let before: Vec<String> = tracks.iter().flat_map(|t| held(*t)).map(|(guid, _)| guid).collect();
    let items: Vec<Item> = found.iter().map(|f| f.item).collect();
    items::run_on_items(ctx, &items, GLUE.0, GLUE.1)?;
    let after: Vec<(String, Item)> = tracks.iter().flat_map(|t| held(*t)).collect();
    let made: Vec<(String, Item)> = after.iter().filter(|(guid, _)| !before.contains(guid)).cloned().collect();
    let left = found.iter().filter(|f| after.iter().any(|(guid, _)| *guid == f.guid)).count();
    if made.is_empty() && left == found.len() {
        return Err(failed("REAPER glued nothing: the items are as they were"));
    }
    if made.is_empty() || left > 0 {
        return Err(("uncertain".into(), "REAPER glued only some of those items: read the tracks".into()));
    }
    // What REAPER made is known by its GUID from now on (the project's map of items is from before it).
    for (guid, item) in &made {
        ctx.reads.registry().note_item(ctx.proj, guid, *item);
    }
    let glued: Vec<Value> = made.iter().map(|(guid, item)| json!([guid, chunks::item_print(reaper, *item)])).collect();
    Ok(Applied {
        applied: json!({"glued": glued}),
        created: Some(json!({"ref": refs::item_ref(&made[0].0), "kind": "item"})),
        material: Some(json!({"originals": originals})),
        display: json!({"title": title, "note": FILES_NOTE}),
        ..Default::default()
    })
}

pub fn glue_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the items before the glue"))?;
    let originals = material["originals"].as_array().ok_or_else(|| bad("material: the items before the glue"))?;
    let glued = restore.applied["glued"].as_array().ok_or_else(|| bad("applied: what the glue made"))?;
    let reaper = ctx.reaper;
    let present: Vec<(Item, &Value)> =
        glued.iter().filter_map(|entry| Some((items::item_by_guid(ctx, entry[0].as_str()?)?, entry))).collect();
    let originals_back = originals.iter().all(|o| o["guid"].as_str().is_some_and(|guid| items::item_by_guid(ctx, guid).is_some()));
    if present.is_empty() && originals_back {
        return Ok(Restored {
            restored: json!({"items": originals.len()}),
            display: json!({"title": "The glued items are back already"}),
            unwritten: true,
        });
    }
    // (discard too: the glued item goes and the originals come back whole, which would take the producer's edits since)
    if present.len() < glued.len() {
        return Err(changed(
            "what the glue made has been deleted in REAPER since: the items aren't brought back over the producer's edits, even \
             with discard",
        ));
    }
    if present.iter().any(|(item, entry)| chunks::item_print(reaper, *item).as_deref() != entry[1].as_str()) {
        return Err(changed(
            "what the glue made has been changed in REAPER since Kumi glued: taking it away would take those changes too, so \
             Kumi leaves it, even with discard",
        ));
    }
    for (item, _) in &present {
        if let Some(track) = reaper.item_track(*item) {
            reaper.delete_item(track, *item);
        }
    }
    let mut remade = 0usize;
    for original in originals {
        let (Some(guid), Some(track), Some(chunk)) = (original["guid"].as_str(), original["track"].as_str(), original["chunk"].as_str())
        else {
            continue;
        };
        if items::item_by_guid(ctx, guid).is_some() {
            continue;
        }
        match items::remake(ctx, track, chunk, guid) {
            Ok(item) => {
                reaper.set_item_value(item, "B_UISEL", 0.0);
                remade += 1;
            }
            Err((code, _)) if code == "gone" => {}
            Err(fault) => {
                return Err((
                    "uncertain".into(),
                    format!("Kumi took the glued item away but couldn't make every item again ({}): read the tracks", fault.1),
                ))
            }
        }
    }
    let mut display = json!({"title": format!("{} back from the glue", count_words(remade))});
    if remade < originals.len() {
        display["note"] = format!("{} item(s) didn't come back: the tracks they were on are gone.", originals.len() - remade).into();
    }
    Ok(Restored { restored: json!({"items": remade}), display, unwritten: false })
}

// ----- render in place -----

fn render_action(input: &Value) -> Done<((i32, &'static str), &'static str)> {
    match input.get("fx").and_then(Value::as_str).unwrap_or("take") {
        "take" => Ok((RENDER_TAKE_FX, "take FX")),
        "track" => Ok((RENDER_ALL_FX, "track and take FX")),
        _ => Err(bad("fx must be take (its take FX) or track (the track's FX too)")),
    }
}

pub fn render_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = listed(ctx, input, "items-render")?;
    let (_, words) = render_action(input)?;
    too_long(rendered_seconds(ctx.reaper, &found, true), "render")?;
    let now = prints(ctx.reaper, &found)?;
    let mut note = FILES_NOTE.to_string();
    if words.starts_with("track") {
        note += " The track's FX still play on the new take: bypass them if it shouldn't go through them twice.";
    }
    Ok(Preview {
        target: items::target(&found[0].guid),
        prior: json!({"prints": now.clone()}),
        fence: json!({"prints": now}),
        display: json!({"title": format!("Rendered {} to new takes ({words})", count_words(found.len())), "note": note}),
    })
}

pub fn render_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = listed_for_apply(ctx, input, "items-render")?;
    let reaper = ctx.reaper;
    if prints(reaper, &found)? != fence["prints"] {
        return Err(changed("those items changed since the preview"));
    }
    refuse_locked(ctx, &found)?;
    let ((command, name), words) = render_action(input)?;
    too_long(rendered_seconds(reaper, &found, true), "render")?;
    let kept = keep(ctx, &found, "render")?;
    let counts: Vec<i32> = found.iter().map(|f| reaper.take_count(f.item)).collect();
    let items: Vec<Item> = found.iter().map(|f| f.item).collect();
    items::run_on_items(ctx, &items, command, name)?;
    // Each item has one take more, and it plays: else every item goes back as it was.
    let whole = found.iter().zip(&counts).all(|(f, count)| {
        items::item_by_guid(ctx, &f.guid)
            .is_some_and(|item| reaper.take_count(item) == count + 1 && reaper.active_take(item) == reaper.take_at(item, *count))
    });
    if !whole {
        let back = kept.iter().all(|k| {
            let item = k["guid"].as_str().and_then(|guid| items::item_by_guid(ctx, guid));
            item.is_some_and(|item| items::put_chunk(ctx, item, k["chunk"].as_str().unwrap_or_default()).is_ok())
        });
        return Err(if back {
            failed("REAPER didn't render every item to a new take: the items are as they were")
        } else {
            ("uncertain".into(), "REAPER didn't render every item to a new take, and Kumi couldn't put them all back: read them".into())
        });
    }
    let after = prints(reaper, &found)?;
    let first = items::item_by_guid(ctx, &found[0].guid)
        .and_then(|item| reaper.active_take(item))
        .map(|take| reaper.take_guid(take))
        .unwrap_or_default();
    let mut note = FILES_NOTE.to_string();
    if words.starts_with("track") {
        note += " The track's FX still play on the new take: bypass them if it shouldn't go through them twice.";
    }
    Ok(Applied {
        applied: json!({"prints": after}),
        created: Some(json!({"ref": refs::take_ref(&first), "kind": "take"})),
        material: Some(json!({"items": kept})),
        display: json!({"title": format!("Rendered {} to new takes ({words})", count_words(found.len())), "note": note}),
        ..Default::default()
    })
}

pub fn render_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the items before the render"))?;
    let kept = material["items"].as_array().ok_or_else(|| bad("material: the items before the render"))?;
    let reaper = ctx.reaper;
    let print_in = |list: &Value, guid: &str| {
        list.as_array().into_iter().flatten().find(|e| e[0] == guid).and_then(|e| e[1].as_str().map(str::to_string))
    };
    let mut found = Vec::new();
    for k in kept {
        let guid = k["guid"].as_str().unwrap_or_default();
        match items::item_by_guid(ctx, guid) {
            Some(item) => found.push((item, guid.to_string(), k["chunk"].as_str().unwrap_or_default())),
            None => return Err(gone("an item that was rendered isn't in the project any more")),
        }
    }
    let now: Vec<Option<String>> = found.iter().map(|(item, _, _)| chunks::item_print(reaper, *item)).collect();
    let back = found.iter().zip(&now).all(|((_, guid, _), print)| *print == print_in(&restore.prior["prints"], guid));
    if back {
        return Ok(Restored {
            restored: json!({"items": found.len()}),
            display: json!({"title": "The items are as they were before the render"}),
            unwritten: true,
        });
    }
    // (discard too: each item's whole state from before goes back, which would take the producer's edits since with it)
    if found.iter().zip(&now).any(|((_, guid, _), print)| *print != print_in(&restore.applied["prints"], guid)) {
        return Err(changed(
            "an item that was rendered has been changed in REAPER since: putting it back as it was would write over those \
             changes too, so Kumi leaves it, even with discard",
        ));
    }
    for (item, _, chunk) in &found {
        items::put_chunk(ctx, *item, chunk)?;
    }
    Ok(Restored {
        restored: json!({"items": found.len()}),
        display: json!({"title": format!("{} as they were before the render", count_words(found.len()))}),
        unwritten: false,
    })
}
