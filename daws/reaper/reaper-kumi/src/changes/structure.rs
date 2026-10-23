//! Adding a track (alone, or as the folder of a run of tracks) and moving one. Both are undone by value: an added track is
//! deleted by its id, a moved one goes back to its place and the folder depths around it are set as they were.

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, Track};
use crate::refs::{self, Ref, TrackId};
use crate::rows;

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

fn track_of(ctx: &Ctx, guid: &str) -> Option<Track> {
    ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string())).ok()
}

/// A track's place among the tracks (0-based).
fn place(reaper: &Reaper, track: Track) -> i64 {
    reaper.track_value(track, "IP_TRACKNUMBER") as i64 - 1
}

fn depth(reaper: &Reaper, track: Track) -> i64 {
    reaper.track_value(track, "I_FOLDERDEPTH") as i64
}

fn parse_track_ref(text: &str, what: &str) -> Done<String> {
    match Ref::parse(text).map_err(bad)? {
        Ref::Track(TrackId::Guid(guid)) => Ok(guid),
        Ref::Track(TrackId::Master) => Err(bad(format!("{what}: the master track can't be placed among the others"))),
        other => Err(bad(format!("{what}: a track ref, not r:{}", other.kind()))),
    }
}

fn name_text(input: &Value) -> Done<Option<String>> {
    match input.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Ok(Some(name.clone())),
        Some(_) => Err(bad("name must be one line of at most 256 characters")),
    }
}

// ----- adding a track -----

/// The tracks a new one is to be the folder of: contiguous, in order, none the master.
fn adopted(ctx: &Ctx, input: &Value) -> Done<Vec<(Track, String)>> {
    let Some(list) = input.get("asFolderOf").filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let refs =
        list.as_array().filter(|a| !a.is_empty() && a.len() <= 500).ok_or_else(|| bad("asFolderOf must list 1 to 500 track refs"))?;
    let mut found: Vec<(Track, String)> = Vec::new();
    for reference in refs {
        let guid = parse_track_ref(reference.as_str().unwrap_or_default(), "asFolderOf")?;
        let track = track_of(ctx, &guid).ok_or_else(|| failed("a track in asFolderOf isn't in the project (deleted, or another tab)"))?;
        found.push((track, guid));
    }
    let first = place(ctx.reaper, found[0].0);
    if found.iter().enumerate().any(|(n, (track, _))| place(ctx.reaper, *track) != first + n as i64) {
        return Err(bad("asFolderOf must be tracks next to each other, in the order they have"));
    }
    // One folder's worth: tracks in the same folder, each folder among them with all of its tracks. A folder adopted
    // alone would hand its tracks to the new folder, and a run past a folder's end would pull the next track in.
    let reaper = ctx.reaper;
    let run: Vec<Track> = found.iter().map(|(track, _)| *track).collect();
    let outer = reaper.track_parent(run[0]);
    if run.iter().any(|track| {
        let parent = reaper.track_parent(*track);
        parent != outer && !parent.is_some_and(|parent| run.contains(&parent))
    }) {
        return Err(bad("asFolderOf must be tracks of one folder (or none): these are in different folders"));
    }
    let mut open = 0i64;
    for (n, track) in run.iter().enumerate() {
        let d = depth(reaper, *track);
        if d > 0 {
            open += d;
        } else if d < 0 {
            let closes = -d;
            // (only the last may close the folder they are all in, and the ones around it)
            if closes > open && n + 1 < run.len() {
                return Err(bad("asFolderOf must be tracks of one folder (or none): these run past the end of their folder"));
            }
            open -= closes.min(open);
        }
    }
    if open > 0 {
        return Err(bad(
            "asFolderOf must hold each folder in it with all of its tracks: a folder's tracks would go into the new folder without it",
        ));
    }
    Ok(found)
}

struct Add {
    at: i64,
    name: Option<String>,
    color: Option<i32>,
    adopted: Vec<(Track, String)>,
}

fn add_plan(ctx: &Ctx, input: &Value) -> Done<Add> {
    let count = ctx.reaper.track_count(ctx.proj) as i64;
    let adopted = adopted(ctx, input)?;
    let at = match (input.get("index").filter(|v| !v.is_null()), adopted.first()) {
        (Some(_), Some(_)) => return Err(bad("pass index or asFolderOf, not both: a folder goes where its tracks are")),
        // The place the new track will have, counted from 1 as a read's track rows count (index).
        (Some(index), None) => {
            index.as_i64().filter(|i| (1..=count + 1).contains(i)).ok_or_else(|| {
                bad(format!(
                    "index must be the new track's place, a whole number from 1 to {} (counted from 1, as a read shows)",
                    count + 1
                ))
            })? - 1
        }
        (None, Some((track, _))) => place(ctx.reaper, *track),
        (None, None) => count,
    };
    let color = match input.get("color") {
        None | Some(Value::Null) => None,
        Some(Value::String(hex)) => {
            let digits = hex
                .strip_prefix('#')
                .filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| bad("color must be #rrggbb"))?;
            let channel = |at: usize| i32::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
            let native =
                ctx.reaper.color_to_native(channel(0), channel(2), channel(4)).ok_or_else(|| failed("this REAPER can't make a colour"))?;
            Some(native | 0x100_0000)
        }
        Some(_) => return Err(bad("color must be #rrggbb")),
    };
    Ok(Add { at, name: name_text(input)?, color, adopted })
}

/// What stands where the new track goes: the track now at that place (it moves down), and the last adopted track's depth.
fn add_fence(ctx: &Ctx, plan: &Add) -> Value {
    let count = ctx.reaper.track_count(ctx.proj);
    let at_guid = ctx.reaper.track_at(ctx.proj, plan.at as i32).map(|track| ctx.reaper.track_guid(track));
    let last = plan.adopted.last().map(|(track, guid)| json!({"guid": guid, "folderDepth": depth(ctx.reaper, *track)}));
    json!({"tracks": count, "at": at_guid, "lastAdopted": last})
}

pub fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let plan = add_plan(ctx, input)?;
    let words = match (&plan.name, plan.adopted.is_empty()) {
        (Some(name), true) => format!("Added track {}", super::quoted(name)),
        (None, true) => "Added a track".to_string(),
        (Some(name), false) => format!("Added folder {}", super::quoted(name)),
        (None, false) => "Added a folder".to_string(),
    };
    let fence = add_fence(ctx, &plan);
    let mut display = json!({"title": words, "index": plan.at + 1});
    if !plan.adopted.is_empty() {
        display["note"] = format!("It holds the {} track(s) after it.", plan.adopted.len()).into();
    }
    Ok(Preview {
        target: json!({"ref": null, "kind": "track", "index": plan.at}),
        prior: json!({"tracks": ctx.reaper.track_count(ctx.proj)}),
        fence,
        display,
    })
}

pub fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = add_plan(ctx, input).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    if &add_fence(ctx, &plan) != fence {
        return Err(changed("the tracks changed since the preview"));
    }
    let reaper = ctx.reaper;
    let track = reaper.insert_track(ctx.proj, plan.at as i32).ok_or_else(|| failed("REAPER wouldn't add a track"))?;
    if let Some(name) = &plan.name {
        reaper.set_track_string(track, "P_NAME", name);
    }
    if let Some(color) = plan.color {
        reaper.set_track_value(track, "I_CUSTOMCOLOR", color as f64);
    }
    // The folder opens at the new track and closes after the last track it holds.
    let mut closed = Value::Null;
    if let Some((_, guid)) = plan.adopted.last() {
        // Found again by its id: what was held before the new track went in may not be where it was.
        let last = track_of(ctx, guid).ok_or_else(|| failed("the folder's last track went while the folder was made"))?;
        let before = depth(reaper, last);
        reaper.set_track_value(track, "I_FOLDERDEPTH", 1.0);
        reaper.set_track_value(last, "I_FOLDERDEPTH", (before - 1) as f64);
        closed = json!({"guid": guid, "before": before, "after": before - 1});
    }
    let guid = reaper.track_guid(track);
    let id = TrackId::Guid(guid.clone());
    let name = rows::clip(&reaper.track_name(track));
    let label = super::track_name(reaper, track);
    let adopted: Vec<&str> = plan.adopted.iter().map(|(_, guid)| guid.as_str()).collect();
    Ok(Applied {
        applied: json!({"index": place(reaper, track), "name": name, "folder": !plan.adopted.is_empty(), "closedAt": closed, "adopted": adopted}),
        created: Some(json!({"ref": refs::track_ref(&id), "kind": "track"})),
        material: None,
        display: json!({"title": if plan.adopted.is_empty() { format!("Added track {}", super::quoted(&label)) } else { format!("Added folder {}", super::quoted(&label)) }}),
        ..Default::default()
    })
}

pub fn add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the track that was added"))?;
    let reference = created["ref"].as_str().ok_or_else(|| bad("created: the track's ref"))?;
    let guid = parse_track_ref(reference, "created")?;
    let reaper = ctx.reaper;
    let Some(track) = track_of(ctx, &guid) else {
        return Err(("gone".into(), "the track Kumi added isn't in the project any more".into()));
    };
    // The producer may have put something in it since: deleting it would lose that.
    let held = reaper.track_item_count(track)
        + reaper.fx_count(crate::reaper::FxOwner::Track(track))
        + reaper.send_count(track, 0)
        + reaper.send_count(track, -1);
    if held > 0 && !restore.discard {
        return Err(changed("the track Kumi added has items, FX or sends in it now"));
    }
    // Nor its automation, nor the tracks the producer has put under it.
    if !restore.discard {
        let drawn = (0..reaper.track_envelope_count(track))
            .filter_map(|i| reaper.track_envelope(track, i))
            .any(|envelope| reaper.envelope_point_count(envelope) > 0);
        if drawn {
            return Err(changed("the track Kumi added has automation in it now"));
        }
        let next = reaper.track_at(ctx.proj, place(reaper, track) as i32 + 1);
        let opens = restore.applied["folder"] != true
            && (depth(reaper, track) > 0 || next.is_some_and(|next| reaper.track_parent(next) == Some(track)));
        if opens {
            return Err(changed("the track Kumi added is a folder of other tracks now"));
        }
    }
    let closed = &restore.applied["closedAt"];
    if let (Some(last), false) = (closed["guid"].as_str().and_then(|g| track_of(ctx, g)), closed.is_null()) {
        let now = depth(reaper, last);
        if now != closed["after"].as_i64().unwrap_or(now) && !restore.discard {
            return Err(changed("the folder's last track has been changed since"));
        }
    }
    // The tracks the folder was made of move up out of it; any other track keeps its folder (REAPER's bare delete would
    // drop a closing the track has taken over since, and pull the tracks after it into that folder).
    let children: Vec<String> = if restore.applied["folder"] != true {
        vec![]
    } else if let Some(adopted) = restore.applied["adopted"].as_array() {
        adopted.iter().filter_map(Value::as_str).map(str::to_string).collect()
    } else {
        // (a folder made before the change kept its tracks' ids: those in it now are taken for its own)
        (0..reaper.track_count(ctx.proj))
            .filter_map(|at| reaper.track_at(ctx.proj, at))
            .filter(|other| reaper.track_parent(*other) == Some(track))
            .map(|other| reaper.track_guid(other))
            .collect()
    };
    let children: Vec<&str> = children.iter().map(String::as_str).collect();
    let name = super::track_name(reaper, track);
    super::tracks::delete_keeping_folders(ctx, &[track], if restore.discard { None } else { Some(&children) })?;
    Ok(Restored {
        restored: json!({"deleted": reference}),
        display: json!({"title": format!("{} taken away again", super::quoted(&name))}),
        ..Default::default()
    })
}

// ----- moving a track -----

/// The tracks around a move, by id and folder depth: what it may change.
fn window(reaper: &Reaper, ctx: &Ctx, low: i64, high: i64) -> Value {
    let count = reaper.track_count(ctx.proj) as i64;
    let tracks: Vec<Value> = (low.max(0)..=(high + 1).min(count - 1))
        .filter_map(|at| reaper.track_at(ctx.proj, at as i32))
        .map(|track| json!({"guid": reaper.track_guid(track), "folderDepth": depth(reaper, track)}))
        .collect();
    json!({"from": low.max(0), "tracks": tracks})
}

struct Move {
    track: Track,
    guid: String,
    from: i64,
    to: i64,
}

fn move_plan(ctx: &Ctx, input: &Value) -> Done<Move> {
    let guid = parse_track_ref(input["ref"].as_str().unwrap_or_default(), "ref")?;
    let track = track_of(ctx, &guid).ok_or_else(|| failed("that track isn't in the project (deleted, or another tab)"))?;
    let count = ctx.reaper.track_count(ctx.proj) as i64;
    // The place the track will have once moved, counted from 1 as a read's track rows count (index).
    let to = input.get("index").and_then(Value::as_i64).filter(|i| (1..=count).contains(i)).ok_or_else(|| {
        bad(format!("index must be the track's new place, a whole number from 1 to {count} (counted from 1, as a read shows)"))
    })? - 1;
    // A folder moved alone would leave its tracks to the folder above them; moving it with them is a different change.
    if depth(ctx.reaper, track) > 0 {
        return Err(bad("that track is a folder: moving it alone would leave its tracks in another folder; move the tracks in or out of it instead, or move them all"));
    }
    Ok(Move { track, guid, from: place(ctx.reaper, track), to })
}

fn move_window(ctx: &Ctx, plan: &Move) -> Value {
    window(ctx.reaper, ctx, plan.from.min(plan.to) - 1, plan.from.max(plan.to))
}

pub fn move_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let plan = move_plan(ctx, input)?;
    let name = super::track_name(ctx.reaper, plan.track);
    let state = json!({"index": plan.from, "window": move_window(ctx, &plan)});
    let mut display = json!({"title": format!("Moved track {} to position {}", super::quoted(&name), plan.to + 1), "before": plan.from + 1, "after": plan.to + 1});
    if plan.from == plan.to {
        display["unchanged"] = true.into();
    }
    Ok(Preview {
        target: json!({"ref": refs::track_ref(&TrackId::Guid(plan.guid)), "kind": "track"}),
        prior: state.clone(),
        fence: state,
        display,
    })
}

/// Moves one track to a place, leaving the producer's selection as it was.
fn reorder(ctx: &Ctx, track: Track, to: i64, from: i64) -> bool {
    let reaper = ctx.reaper;
    // The producer's selection, by id, to put back: selecting the one track to move is how REAPER is asked to move it.
    let selected: Vec<String> = (0..reaper.track_count(ctx.proj))
        .filter_map(|at| reaper.track_at(ctx.proj, at))
        .filter(|t| reaper.track_value(*t, "I_SELECTED") != 0.0)
        .map(|t| reaper.track_guid(t))
        .collect();
    let master = reaper.master_track(ctx.proj).filter(|m| reaper.track_value(*m, "I_SELECTED") != 0.0);
    reaper.select_only(track);
    // REAPER puts the track above the one at `before`, counted before the move.
    let before = if to > from { to + 1 } else { to };
    let moved = reaper.reorder_selected(before as i32, 0);
    for at in 0..reaper.track_count(ctx.proj) {
        if let Some(t) = reaper.track_at(ctx.proj, at) {
            reaper.set_track_selected(t, selected.contains(&reaper.track_guid(t)));
        }
    }
    if let Some(m) = reaper.master_track(ctx.proj) {
        reaper.set_track_selected(m, master.is_some());
    }
    moved
}

pub fn move_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = move_plan(ctx, input).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    let now = json!({"index": plan.from, "window": move_window(ctx, &plan)});
    if &now != fence {
        return Err(changed("the tracks changed since the preview"));
    }
    if plan.from != plan.to && !reorder(ctx, plan.track, plan.to, plan.from) {
        return Err(failed("REAPER wouldn't move the track"));
    }
    let moved = track_of(ctx, &plan.guid).ok_or_else(|| failed("the track went while it was moved"))?;
    let at = place(ctx.reaper, moved);
    if at != plan.to {
        // Put back where it was, inside this call; else nobody knows how the tracks stand.
        let back = reorder(ctx, moved, plan.from, at) && place(ctx.reaper, moved) == plan.from;
        let message = format!("REAPER put the track at {} rather than {}", at + 1, plan.to + 1);
        return Err(if back {
            failed(format!("{message}; the track is moved back"))
        } else {
            ("uncertain".into(), format!("{message}, and Kumi could not move it back: read the tracks"))
        });
    }
    let name = super::track_name(ctx.reaper, moved);
    let after = json!({"index": at, "window": window(ctx.reaper, ctx, plan.from.min(plan.to) - 1, plan.from.max(plan.to))});
    Ok(Applied {
        applied: after,
        created: None,
        material: None,
        display: json!({"title": format!("Moved track {} to position {}", super::quoted(&name), at + 1)}),
        unwritten: plan.from == plan.to,
    })
}

pub fn move_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = parse_track_ref(restore.target["ref"].as_str().unwrap_or_default(), "target")?;
    let reaper = ctx.reaper;
    let track = track_of(ctx, &guid).ok_or_else(|| ("gone".to_string(), "that track isn't in the project any more".to_string()))?;
    let name = super::track_name(reaper, track);
    let (from, to) = (restore.prior["index"].as_i64().unwrap_or(0), restore.applied["index"].as_i64().unwrap_or(0));
    let (low, high) = (from.min(to) - 1, from.max(to));
    let now_window = window(reaper, ctx, low, high);
    if now_window != restore.applied["window"] && !restore.discard {
        if now_window == restore.prior["window"] {
            return Ok(Restored {
                restored: restore.prior.clone(),
                display: json!({"title": "Track already back in its place"}),
                unwritten: true,
            });
        }
        return Err(changed("the tracks around it have been moved or changed in REAPER since Kumi moved it"));
    }
    let at = place(reaper, track);
    if at != from && !reorder(ctx, track, from, at) {
        return Err(failed("REAPER wouldn't move the track back"));
    }
    // The depths the neighbours had, by id.
    for entry in restore.prior["window"]["tracks"].as_array().into_iter().flatten() {
        if let Some(other) = entry["guid"].as_str().and_then(|g| track_of(ctx, g)) {
            if depth(reaper, other) != entry["folderDepth"].as_i64().unwrap_or(0) {
                reaper.set_track_value(other, "I_FOLDERDEPTH", entry["folderDepth"].as_f64().unwrap_or(0.0));
            }
        }
    }
    Ok(Restored {
        restored: json!({"index": place(reaper, track)}),
        display: json!({"title": format!("{} back at position {}", super::quoted(&name), from + 1)}),
        ..Default::default()
    })
}
