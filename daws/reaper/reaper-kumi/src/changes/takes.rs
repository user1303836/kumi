//! Takes (M4): a take's properties (volume, polarity, pan, pitch, playrate, preserve pitch, channel mode, name, colour),
//! which of an item's takes plays, adding a take (empty, or playing a media file in place) and deleting one. A take is
//! found by its GUID, through REAPER's own lookup.
//!
//! No API call deletes a take: REAPER's own "Take: Delete active take from items" does, run on exactly that item with the
//! producer's selection put back after. Adding and deleting a take keep the item's chunk from before and put it back
//! whole, only while the item is exactly as the change left it.

use serde_json::{json, Map, Value};

use super::items::{self, changed, close, gone, Field, Found};
use super::{chunks, quoted, Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Item, Reaper, Take};
use crate::refs::{self, Ref};
use crate::rows;

/// REAPER's action that deletes the selected items' active takes, and the name it has in 7.82 (40130 asks first).
const DELETE_ACTIVE_TAKE: (i32, &str) = (40129, "Take: Delete active take from items");

/// A take a request names, with its item.
pub(super) struct TakeFound {
    pub take: Take,
    pub guid: String,
    pub item: Item,
    pub item_guid: String,
}

/// The take a ref names: a take's ref, or an item's (its active take).
pub(super) fn find(ctx: &Ctx, value: &Value, what: &str) -> Done<TakeFound> {
    let text = value.as_str().ok_or_else(|| bad(format!("{what}: ref must be a take ref")))?;
    let take = match Ref::parse(text).map_err(bad)? {
        Ref::Take(guid) => ctx.reads.take_by_ref(ctx.reaper, ctx.proj, &guid)?,
        Ref::Item(guid) => {
            let item = ctx.reads.item_by_ref(ctx.reaper, ctx.proj, &guid)?;
            ctx.reaper.active_take(item).ok_or_else(|| bad("that item has no take"))?
        }
        other => return Err(bad(format!("{what}: ref must be a take ref, not r:{}", other.kind()))),
    };
    let item = ctx.reaper.take_item(take).ok_or_else(|| failed("that take's item isn't in the project"))?;
    Ok(TakeFound { take, guid: ctx.reaper.take_guid(take), item, item_guid: ctx.reaper.item_guid(item) })
}

fn find_for_apply(ctx: &Ctx, value: &Value, what: &str) -> Done<TakeFound> {
    find(ctx, value, what).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })
}

fn find_for_restore(ctx: &Ctx, value: &Value, what: &str) -> Done<TakeFound> {
    find(ctx, value, what).map_err(|(code, message)| if code == "failed" { gone(message) } else { (code, message) })
}

pub(super) fn target(guid: &str) -> Value {
    json!({"ref": refs::take_ref(guid), "kind": "take"})
}

/// A take of an item, by GUID.
pub(super) fn take_in(reaper: &Reaper, item: Item, guid: &str) -> Option<Take> {
    (0..reaper.take_count(item)).filter_map(|n| reaper.take_at(item, n)).find(|take| reaper.take_guid(*take) == guid)
}

/// The take as the producer knows it: its name in quotes, else its place in its item ("take 2 of “vox.wav”").
pub(super) fn label(ctx: &Ctx, take: Take) -> String {
    let name = rows::clip(&ctx.reaper.take_name(take));
    if !name.is_empty() {
        return quoted(&name);
    }
    let Some(item) = ctx.reaper.take_item(take) else { return "a take".into() };
    let place = (0..ctx.reaper.take_count(item)).position(|n| ctx.reaper.take_at(item, n) == Some(take)).unwrap_or(0) + 1;
    format!("take {place} of {}", items::label(ctx, item))
}

fn label_opening(ctx: &Ctx, take: Take) -> String {
    let name = rows::clip(&ctx.reaper.take_name(take));
    if !name.is_empty() {
        return name;
    }
    let mut words = label(ctx, take);
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}

fn refuse_locked(ctx: &Ctx, item: Item) -> Done<()> {
    if items::is_locked(ctx.reaper, item) {
        return Err(bad(format!("{} is locked in REAPER: unlock it first (item_props lock: false)", items::label(ctx, item))));
    }
    Ok(())
}

// ----- properties -----

fn flag(value: &Value, key: &str) -> Done<Value> {
    value.as_bool().map(Value::Bool).ok_or_else(|| bad(format!("{key} must be true or false")))
}

fn bounded(value: &Value, key: &str, low: f64, high: f64, unit: &str) -> Done<Value> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && (low..=high).contains(n))
        .map(|n| json!(n))
        .ok_or_else(|| bad(format!("{key} must be from {low} to {high}{unit}")))
}

fn same_number(a: &Value, b: &Value) -> bool {
    close(a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN))
}

fn same_exact(a: &Value, b: &Value) -> bool {
    a == b
}

/// A number as a title says it: no trailing zeros, a true minus.
fn plain(value: f64) -> String {
    let text = format!("{:.2}", value.abs());
    let text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    if value < 0.0 {
        format!("−{text}")
    } else {
        text
    }
}

const CHANNEL_MODES: [(&str, i64); 5] = [("normal", 0), ("reverse", 1), ("mono", 2), ("left", 3), ("right", 4)];

/// A take's volume and its polarity are one number in REAPER (negative: flipped): each is changed without the other.
fn volume_magnitude(reaper: &Reaper, take: Take) -> f64 {
    reaper.take_value(take, "D_VOL").abs()
}

pub(super) static TAKE_FIELDS: [Field<Take>; 9] = [
    Field {
        key: "volumeDb",
        read: |reaper, take| json!(volume_magnitude(reaper, take)),
        parse: |_, value| items::gain_of(value, "volumeDb").map(|gain| json!(gain)),
        write: |reaper, take, value| {
            let sign = if reaper.take_value(take, "D_VOL") < 0.0 { -1.0 } else { 1.0 };
            reaper.set_take_value(take, "D_VOL", sign * value.as_f64().unwrap_or(1.0).abs())
        },
        same: items::same_number6,
        words: |value| format!("volume {} dB", super::tracks::db_text(rows::db(value.as_f64().unwrap_or(1.0)))),
        show: items::show_db,
    },
    Field {
        key: "polarity",
        read: |reaper, take| json!(if reaper.take_value(take, "D_VOL") < 0.0 { "inverted" } else { "normal" }),
        parse: |_, value| match value.as_str() {
            Some("inverted" | "normal") => Ok(value.clone()),
            _ => Err(bad("polarity must be inverted or normal")),
        },
        write: |reaper, take, value| {
            let magnitude = volume_magnitude(reaper, take);
            reaper.set_take_value(take, "D_VOL", if value.as_str() == Some("inverted") { -magnitude } else { magnitude })
        },
        same: same_exact,
        words: |value| format!("polarity {}", value.as_str().unwrap_or("normal")),
        show: items::show_same,
    },
    Field {
        key: "pan",
        read: |reaper, take| json!(reaper.take_value(take, "D_PAN")),
        parse: |_, value| bounded(value, "pan", -1.0, 1.0, " (-1 left, 1 right)"),
        write: |reaper, take, value| reaper.set_take_value(take, "D_PAN", value.as_f64().unwrap_or(0.0)),
        same: items::same_number6,
        words: |value| format!("pan {}", super::tracks::pan_text(value.as_f64().unwrap_or(0.0))),
        show: items::show_same,
    },
    Field {
        key: "pitch",
        read: |reaper, take| json!(reaper.take_value(take, "D_PITCH")),
        parse: |_, value| bounded(value, "pitch", -96.0, 96.0, " semitones"),
        write: |reaper, take, value| reaper.set_take_value(take, "D_PITCH", value.as_f64().unwrap_or(0.0)),
        same: same_number,
        words: |value| {
            let semitones = value.as_f64().unwrap_or(0.0);
            format!("pitch {}{} st", if semitones > 0.0 { "+" } else { "" }, plain(semitones))
        },
        show: items::show_same,
    },
    Field {
        key: "playrate",
        read: |reaper, take| json!(reaper.take_value(take, "D_PLAYRATE")),
        parse: |_, value| bounded(value, "playrate", 0.01, 100.0, " (1 is normal speed)"),
        write: |reaper, take, value| reaper.set_take_value(take, "D_PLAYRATE", value.as_f64().unwrap_or(1.0)),
        same: same_number,
        words: |value| format!("rate {}×", plain(value.as_f64().unwrap_or(1.0))),
        show: items::show_same,
    },
    Field {
        key: "preservePitch",
        read: |reaper, take| json!(reaper.take_value(take, "B_PPITCH") != 0.0),
        parse: |_, value| flag(value, "preservePitch"),
        write: |reaper, take, value| reaper.set_take_value(take, "B_PPITCH", if value == &Value::Bool(true) { 1.0 } else { 0.0 }),
        same: same_exact,
        words: |value| if value == &Value::Bool(true) { "pitch kept with the rate".into() } else { "pitch follows the rate".into() },
        show: items::show_same,
    },
    Field {
        key: "channelMode",
        read: |reaper, take| json!(reaper.take_value(take, "I_CHANMODE") as i64),
        parse: |_, value| {
            let name = value.as_str().unwrap_or_default();
            CHANNEL_MODES
                .iter()
                .find(|(word, _)| *word == name)
                .map(|(_, mode)| json!(mode))
                .ok_or_else(|| bad("channelMode must be normal, reverse, mono, left or right"))
        },
        write: |reaper, take, value| reaper.set_take_value(take, "I_CHANMODE", value.as_f64().unwrap_or(0.0)),
        same: same_exact,
        words: |value| {
            let name = CHANNEL_MODES.iter().find(|(_, mode)| Some(*mode) == value.as_i64()).map_or("its own", |(word, _)| word);
            format!("channels {name}")
        },
        show: |_, value| CHANNEL_MODES.iter().find(|(_, mode)| Some(*mode) == value.as_i64()).map_or(Value::Null, |(word, _)| json!(word)),
    },
    Field {
        key: "name",
        read: |reaper, take| json!(reaper.take_name(take)),
        parse: |_, value| match value.as_str() {
            Some(name) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Ok(value.clone()),
            _ => Err(bad("name must be one line of at most 256 characters")),
        },
        write: |reaper, take, value| reaper.set_take_string(take, "P_NAME", value.as_str().unwrap_or_default()),
        same: same_exact,
        words: |value| match value.as_str().unwrap_or_default() {
            "" => "name cleared".into(),
            name => format!("named {}", quoted(&rows::clip(name))),
        },
        show: items::show_same,
    },
    Field {
        key: "color",
        read: |reaper, take| json!(reaper.take_value(take, "I_CUSTOMCOLOR") as i64),
        parse: |ctx, value| items::color_of(ctx, value, "color"),
        write: |reaper, take, value| reaper.set_take_value(take, "I_CUSTOMCOLOR", value.as_f64().unwrap_or(0.0)),
        same: items::same_color,
        words: |value| {
            if super::tracks::colour_of(&json!({"color": value})) == 0 {
                "colour cleared".into()
            } else {
                "colour changed".into()
            }
        },
        show: items::show_color,
    },
];

fn props_plan(ctx: &Ctx, input: &Value) -> Done<(Vec<&'static Field<Take>>, Value)> {
    let asked = items::asked_fields(ctx, input, &TAKE_FIELDS, "take-props")?;
    let mut wanted = Map::new();
    for (field, value) in &asked {
        wanted.insert(field.key.into(), value.clone());
    }
    Ok((asked.iter().map(|(field, _)| *field).collect(), Value::Object(wanted)))
}

fn props_title(ctx: &Ctx, take: Take, fields: &[&Field<Take>], state: &Value) -> String {
    format!("{}: {}", label_opening(ctx, take), items::words_of(fields, state))
}

pub fn props_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "take-props")?;
    let (fields, wanted) = props_plan(ctx, input)?;
    let now = items::state_of(ctx.reaper, found.take, &fields);
    let mut display = json!({
        "title": props_title(ctx, found.take, &fields, &wanted),
        "before": items::shown(ctx.reaper, &fields, &now),
        "after": items::shown(ctx.reaper, &fields, &wanted),
    });
    if items::same_state(&fields, &now, &wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(&found.guid), prior: now.clone(), fence: now, display })
}

pub fn props_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "take-props")?;
    let (fields, wanted) = props_plan(ctx, input)?;
    let now = items::state_of(ctx.reaper, found.take, &fields);
    if !items::same_state(&fields, &now, fence) {
        return Err(changed(format!("{} changed since the preview", label_opening(ctx, found.take))));
    }
    refuse_locked(ctx, found.item)?;
    // (named before a new name is written)
    let opening = label_opening(ctx, found.take);
    items::write_state(ctx.reaper, found.take, &fields, &wanted);
    ctx.reaper.update_item(found.item);
    let after = items::state_of(ctx.reaper, found.take, &fields);
    if !items::same_state(&fields, &after, &wanted) {
        items::write_state(ctx.reaper, found.take, &fields, &now);
        return Err(failed("REAPER didn't take all of that; the take is as it was"));
    }
    Ok(Applied {
        applied: after.clone(),
        display: json!({
            "title": format!("{opening}: {}", items::words_of(&fields, &after)),
            "before": items::shown(ctx.reaper, &fields, &now),
            "after": items::shown(ctx.reaper, &fields, &after),
        }),
        unwritten: items::same_state(&fields, &now, &wanted),
        ..Default::default()
    })
}

pub fn props_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "take-props")?;
    let fields = items::fields_in(&TAKE_FIELDS, restore.prior);
    if fields.is_empty() {
        return Err(bad("prior: the take's properties before the change"));
    }
    let now = items::state_of(ctx.reaper, found.take, &fields);
    let name = label_opening(ctx, found.take);
    if items::same_state(&fields, &now, restore.prior) {
        return Ok(Restored { restored: now, display: json!({"title": format!("{name} is as it was")}), unwritten: true });
    }
    if !restore.discard && !items::same_state(&fields, &now, restore.applied) {
        return Err(changed(format!("{name} has been changed in REAPER since Kumi set it")));
    }
    items::write_state(ctx.reaper, found.take, &fields, restore.prior);
    ctx.reaper.update_item(found.item);
    let after = items::state_of(ctx.reaper, found.take, &fields);
    if !items::same_state(&fields, &after, restore.prior) {
        return Err(failed(format!("REAPER kept some of {name}'s properties as Kumi had set them")));
    }
    Ok(Restored {
        restored: after,
        display: json!({"title": format!("{}: {} (put back)", label_opening(ctx, found.take), items::words_of(&fields, restore.prior))}),
        unwritten: false,
    })
}

// ----- which take plays -----

fn active_of(reaper: &Reaper, item: Item) -> Value {
    json!({"active": reaper.active_take(item).map(|take| reaper.take_guid(take))})
}

pub fn active_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "take-active")?;
    let now = active_of(ctx.reaper, found.item);
    let mut display = json!({"title": format!("{} plays in {}", label_opening(ctx, found.take), items::label(ctx, found.item))});
    if now["active"].as_str() == Some(found.guid.as_str()) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: items::target(&found.item_guid), prior: now.clone(), fence: now, display })
}

pub fn active_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "take-active")?;
    let now = active_of(ctx.reaper, found.item);
    if &now != fence {
        return Err(changed(format!("the take that plays in {} changed since the preview", items::label(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    let title = format!("{} plays in {}", label_opening(ctx, found.take), items::label(ctx, found.item));
    if now["active"].as_str() == Some(found.guid.as_str()) {
        return Ok(Applied { applied: now, display: json!({"title": title}), unwritten: true, ..Default::default() });
    }
    if !ctx.reaper.set_active_take(found.take) {
        return Err(failed("REAPER wouldn't make that take the active one"));
    }
    ctx.reaper.update_item(found.item);
    let after = active_of(ctx.reaper, found.item);
    if after["active"].as_str() != Some(found.guid.as_str()) {
        return Err(failed("REAPER kept another take active"));
    }
    Ok(Applied { applied: after, display: json!({"title": title}), ..Default::default() })
}

pub fn active_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = items::find_for_restore(ctx, &restore.target["ref"], "take-active")?;
    let now = active_of(ctx.reaper, found.item);
    if &now == restore.prior {
        return Ok(Restored { restored: now, display: json!({"title": "The take that played plays again already"}), unwritten: true });
    }
    if !restore.discard && &now != restore.applied {
        return Err(changed(format!("another take has been made active in {} since", items::label(ctx, found.item))));
    }
    let prior = restore.prior["active"].as_str().ok_or_else(|| bad("prior: the take that played"))?;
    let take = take_in(ctx.reaper, found.item, prior).ok_or_else(|| gone("the take that played isn't in the item any more"))?;
    ctx.reaper.set_active_take(take);
    ctx.reaper.update_item(found.item);
    if active_of(ctx.reaper, found.item) != *restore.prior {
        return Err(failed("REAPER kept another take active"));
    }
    Ok(Restored {
        restored: restore.prior.clone(),
        display: json!({"title": format!("{} plays again", label_opening(ctx, take))}),
        unwritten: false,
    })
}

// ----- adding and deleting a take: the item put back whole -----

/// The item's chunk, if it is small enough to keep.
fn keepable_chunk(ctx: &Ctx, item: Item, verb: &str) -> Done<String> {
    let chunk = ctx.reaper.item_chunk(item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    if chunk.len() > chunks::MOST_KEPT {
        return Err(failed(format!(
            "this item's state is {} MB, more than the {} MB Kumi keeps to put it back: Kumi won't {verb} what it can't put back",
            chunk.len() >> 20,
            chunks::MOST_KEPT >> 20
        )));
    }
    Ok(chunk)
}

/// Puts an item back as it was before a change, while it is as the change left it (`applied.print`), discard or not:
/// the item's whole state goes back, which would take the producer's edits since with it. Back already when its print
/// is `prior.print`.
pub(super) fn snapshot_restore(ctx: &Ctx, restore: &Restore, words: &str) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the item from before the change"))?;
    let chunk = material["chunk"].as_str().ok_or_else(|| bad("material: the item's state"))?;
    let found = items::find_for_restore(ctx, &restore.target["ref"], words)?;
    let print = chunks::item_print(ctx.reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    let name = items::label_opening(ctx, found.item);
    if Some(print.as_str()) == restore.prior["print"].as_str() {
        return Ok(Restored {
            restored: json!({"ref": refs::item_ref(&found.guid)}),
            display: json!({"title": format!("{name} is as it was")}),
            unwritten: true,
        });
    }
    if Some(print.as_str()) != restore.applied["print"].as_str() {
        return Err(changed(format!(
            "{} has been changed in REAPER since Kumi {words}: putting the item back as it was would write over those changes \
             too (its takes, their FX and envelopes), so Kumi leaves it, even with discard",
            items::label(ctx, found.item)
        )));
    }
    items::put_chunk(ctx, found.item, chunk)?;
    // (named as it is put back: its active take may be another again)
    let name = items::label_opening(ctx, found.item);
    Ok(Restored {
        restored: json!({"ref": refs::item_ref(&found.guid)}),
        display: json!({"title": format!("{name} as it was before")}),
        unwritten: false,
    })
}

struct Add {
    found: Found,
    path: Option<String>,
    name: Option<String>,
    active: bool,
}

fn add_plan(ctx: &Ctx, input: &Value) -> Done<Add> {
    let found = items::find(ctx, &input["ref"], "take-add")?;
    let path = match input.get("path").filter(|v| !v.is_null()) {
        None => None,
        Some(value) => {
            let path = value.as_str().filter(|p| !p.is_empty()).ok_or_else(|| bad("path: the media file's full path"))?;
            if !std::path::Path::new(path).is_absolute() {
                return Err(bad("path must be a full path (from /, or a drive on Windows)"));
            }
            if !std::fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
                return Err(bad("there is no file at that path (or it is a folder)"));
            }
            ctx.reaper
                .media(path)
                .ok_or_else(|| bad("REAPER reads nothing from that file: it is no audio or MIDI REAPER knows, or it is empty"))?;
            Some(path.to_string())
        }
    };
    let name = match input.get("name").filter(|v| !v.is_null()) {
        None => None,
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Some(name.clone()),
        Some(_) => return Err(bad("name must be one line of at most 256 characters")),
    };
    let active = match input.get("active").filter(|v| !v.is_null()) {
        None => true,
        Some(value) => value.as_bool().ok_or_else(|| bad("active must be true or false"))?,
    };
    Ok(Add { found, path, name, active })
}

fn add_title(ctx: &Ctx, add: &Add) -> String {
    let item = items::label(ctx, add.found.item);
    match (&add.path, &add.name) {
        (Some(path), _) => {
            let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            format!("Added {} as a take of {item}", quoted(&rows::clip(&file)))
        }
        (None, Some(name)) => format!("Added the empty take {} to {item}", quoted(name)),
        (None, None) => format!("Added an empty take to {item}"),
    }
}

pub fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let add = add_plan(ctx, input)?;
    let print = chunks::item_print(ctx.reaper, add.found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    let mut display = json!({"title": add_title(ctx, &add)});
    if let Some(path) = &add.path {
        display["note"] = super::kumi_files::note(ctx, path).into();
    }
    Ok(Preview { target: items::target(&add.found.guid), prior: json!({"print": print}), fence: json!({"print": print}), display })
}

pub fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let add = add_plan(ctx, input).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    let item = add.found.item;
    let chunk = keepable_chunk(ctx, item, "add a take to")?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() {
        return Err(changed(format!("{} changed since the preview", items::label(ctx, item))));
    }
    refuse_locked(ctx, item)?;
    // (a file in Kumi's own folder plays from a copy in the project's media folder)
    let played = add.path.as_deref().map(|path| super::kumi_files::playable(ctx, path)).transpose()?;
    // (said before the take is added: a new active take gives the item its name)
    let title = add_title(ctx, &add);
    let reaper = ctx.reaper;
    let take = reaper.add_take(item).ok_or_else(|| failed("REAPER wouldn't add a take"))?;
    // Nothing half-made stays: the item goes back as it was if any of it fails.
    let undo = |message: &str| -> Fault {
        match items::put_chunk(ctx, item, &chunk) {
            Ok(()) => failed(format!("{message}: the item is as it was")),
            Err(_) => ("uncertain".into(), format!("{message}, and Kumi could not put the item back: read it")),
        }
    };
    if let Some(path) = &played {
        if reaper.set_take_file(take, path).is_none() {
            return Err(undo("REAPER wouldn't read that file into the take"));
        }
    }
    let name = add
        .name
        .clone()
        .or_else(|| add.path.as_ref().and_then(|p| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned())));
    if let Some(name) = name {
        reaper.set_take_string(take, "P_NAME", &name);
    }
    if add.active && !reaper.set_active_take(take) {
        return Err(undo("REAPER wouldn't make the new take the active one"));
    }
    reaper.update_item(item);
    let guid = reaper.take_guid(take);
    let print = chunks::item_print(reaper, item).ok_or_else(|| undo("REAPER wouldn't give the item's state"))?;
    let mut display = json!({"title": title});
    if let Some(path) = &add.path {
        display["note"] = super::kumi_files::note(ctx, path).into();
    }
    Ok(Applied {
        applied: json!({"print": print}),
        created: Some(json!({"ref": refs::take_ref(&guid), "kind": "take"})),
        material: Some(json!({"chunk": chunk})),
        display,
        ..Default::default()
    })
}

pub fn add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    snapshot_restore(ctx, restore, "added the take")
}

pub fn delete_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "take-delete")?;
    if ctx.reaper.take_count(found.item) < 2 {
        return Err(bad(format!(
            "{} is the only take of {}: delete the item instead (item_delete)",
            label(ctx, found.take),
            items::label(ctx, found.item)
        )));
    }
    let print = chunks::item_print(ctx.reaper, found.item).ok_or_else(|| failed("REAPER wouldn't give the item's state"))?;
    Ok(Preview {
        target: items::target(&found.item_guid),
        prior: json!({"print": print}),
        fence: json!({"print": print, "take": found.guid}),
        display: json!({"title": delete_title(ctx, &found)}),
    })
}

/// "Deleted “take 2” from “vox.wav”"; the item by its place when its name is the take's own.
fn delete_title(ctx: &Ctx, found: &TakeFound) -> String {
    let item = if ctx.reaper.active_take(found.item) == Some(found.take) {
        items::place_label(ctx, found.item)
    } else {
        items::label(ctx, found.item)
    };
    format!("Deleted {} from {item}", label(ctx, found.take))
}

pub fn delete_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "take-delete")?;
    let reaper = ctx.reaper;
    let chunk = keepable_chunk(ctx, found.item, "delete a take of")?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() || fence["take"].as_str() != Some(found.guid.as_str()) {
        return Err(changed(format!("{} changed since the preview", items::label(ctx, found.item))));
    }
    refuse_locked(ctx, found.item)?;
    if reaper.take_count(found.item) < 2 {
        return Err(bad("that is the item's only take: delete the item instead (item_delete)"));
    }
    let title = delete_title(ctx, &found);
    let count = reaper.take_count(found.item);
    // The take that plays now, by GUID: it plays again after, unless it is the one deleted.
    let playing = reaper.active_take(found.item).map(|take| reaper.take_guid(take));
    // (the action is checked before anything is written: an action of another name under its number isn't run)
    if reaper.action_name(DELETE_ACTIVE_TAKE.0).as_deref() != Some(DELETE_ACTIVE_TAKE.1) {
        return Err(failed(format!(
            "this REAPER has no action {} named “{}”, so Kumi won't delete a take",
            DELETE_ACTIVE_TAKE.0, DELETE_ACTIVE_TAKE.1
        )));
    }
    reaper.set_active_take(found.take);
    items::run_on_items(ctx, &[found.item], DELETE_ACTIVE_TAKE.0, DELETE_ACTIVE_TAKE.1)?;
    let item = items::item_by_guid(ctx, &found.item_guid)
        .ok_or_else(|| ("uncertain".to_string(), "the item went while its take was deleted: read it".to_string()))?;
    if let Some(take) = playing.filter(|guid| *guid != found.guid).and_then(|guid| take_in(reaper, item, &guid)) {
        reaper.set_active_take(take);
    }
    if take_in(reaper, item, &found.guid).is_some() || reaper.take_count(item) != count - 1 {
        return Err(match items::put_chunk(ctx, item, &chunk) {
            Ok(()) => failed("REAPER didn't delete that take alone: the item is as it was"),
            Err(_) => ("uncertain".into(), "REAPER didn't delete that take alone, and Kumi could not put the item back: read it".into()),
        });
    }
    reaper.update_item(item);
    let print = chunks::item_print(reaper, item)
        .ok_or_else(|| ("uncertain".to_string(), "REAPER wouldn't give the item's state after the delete".to_string()))?;
    Ok(Applied {
        applied: json!({"print": print}),
        material: Some(json!({"chunk": chunk})),
        display: json!({"title": title}),
        ..Default::default()
    })
}

pub fn delete_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    snapshot_restore(ctx, restore, "deleted its take")
}
