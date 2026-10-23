//! A track's fixed lanes (REAPER 7) and razor areas (M4): its item positioning (fixed lanes, free, or normal), which lanes
//! play, lane names, comping a lane's span into the comp lane, and the razor areas themselves.
//!
//! Lanes and razor areas are a track's values: put back by value. A comp area has no API (it is a `LINKEDLANE` line of
//! the track's chunk, made by REAPER's own "Razor edit: Create fixed lane comp area"), so a comp keeps the track's chunk
//! from before and puts it back whole, only while the track is exactly as the comp left it.

use serde_json::{json, Value};

use super::items::{changed, gone, lane_count, track_by_guid};
use super::{chunks, quoted, track_name, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, Track};
use crate::refs::{self, EnvelopeId, Ref, TrackId};

/// REAPER's actions a comp is made with, and their names in 7.82.
const COMP_INTO_NEW_LANE: (i32, &str) = (42797, "Track lanes: Comp into new empty lane");
const COMP_AREA_FROM_RAZOR: (i32, &str) = (42475, "Razor edit: Create fixed lane comp area");
/// The most razor areas one change sets on a track.
const MOST_AREAS: usize = 64;
/// The most lanes a track has (REAPER 7).
pub(super) const MOST_LANES: i64 = 256;

struct TrackFound {
    track: Track,
    guid: String,
}

fn find(ctx: &Ctx, value: &Value, what: &str) -> Done<TrackFound> {
    let text = value.as_str().ok_or_else(|| bad(format!("{what}: ref must be a track ref")))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::Track(TrackId::Guid(guid)) => {
            Ok(TrackFound { track: ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.clone()))?, guid })
        }
        Ref::Track(TrackId::Master) => Err(bad(format!("{what}: the master track has no lanes or razor areas"))),
        other => Err(bad(format!("{what}: ref must be a track ref, not r:{}", other.kind()))),
    }
}

fn find_for_apply(ctx: &Ctx, value: &Value, what: &str) -> Done<TrackFound> {
    find(ctx, value, what).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })
}

fn find_for_restore(ctx: &Ctx, value: &Value, what: &str) -> Done<TrackFound> {
    find(ctx, value, what).map_err(|(code, message)| if code == "failed" { gone(message) } else { (code, message) })
}

fn target(guid: &str) -> Value {
    json!({"ref": refs::track_ref(&TrackId::Guid(guid.to_string())), "kind": "track"})
}

fn named(ctx: &Ctx, track: Track) -> String {
    track_name(ctx.reaper, track)
}

/// The lane a request names, from 0, on a track with fixed lanes.
fn lane_of(ctx: &Ctx, input: &Value, track: Track) -> Done<i64> {
    let count = lane_count(ctx.reaper, track).ok_or_else(|| bad("that track has no fixed lanes: turn them on with track_lanes first"))?;
    let lane = input.get("lane").and_then(Value::as_i64).ok_or_else(|| bad("lane must be a whole number from 0"))?;
    if !(0..count).contains(&lane) {
        return Err(bad(format!("lane must be from 0 to {} on that track (it has {count} lanes)", count - 1)));
    }
    Ok(lane)
}

// ----- lanes on or off -----

const MODES: [(&str, i64); 3] = [("normal", 0), ("free", 1), ("lanes", 2)];

/// How a track positions its items and, with fixed lanes, how many lanes it has, how each plays and which lane each item is
/// in (by GUID). Without fixed lanes, its items by GUID alone: an undo that turns lanes on again puts those items on their
/// lanes, and one the producer added or took away since is theirs.
fn lanes_state(reaper: &Reaper, track: Track) -> Value {
    let mode = reaper.track_value(track, "I_FREEMODE") as i64;
    let items = (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n));
    if mode != 2 {
        let items: Vec<Value> = items.map(|item| json!([reaper.item_guid(item), null])).collect();
        return json!({"mode": mode, "lanes": null, "plays": null, "items": items});
    }
    let count = (reaper.track_value(track, "I_NUMFIXEDLANES") as i64).max(1);
    let items: Vec<Value> = items.map(|item| json!([reaper.item_guid(item), reaper.item_value(item, "I_FIXEDLANE") as i64])).collect();
    json!({"mode": mode, "lanes": count, "plays": plays_of(reaper, track, count), "all": all_of(reaper, track), "items": items})
}

fn plays_of(reaper: &Reaper, track: Track, count: i64) -> Vec<i64> {
    (0..count).map(|n| reaper.track_value(track, &format!("C_LANEPLAYS:{n}")) as i64).collect()
}

/// `C_ALLLANESPLAY`: 1 when no lane is picked to play (all play), 0 when none plays, 2 when lanes are picked, even every
/// one of them (lab, 7.82). The plays alone can't tell the first from the last.
fn all_of(reaper: &Reaper, track: Track) -> i64 {
    reaper.track_value(track, "C_ALLLANESPLAY") as i64
}

/// A state's items, by GUID, with their lanes (none without fixed lanes), in GUID order.
fn lane_items(state: &Value) -> Vec<(String, Option<i64>)> {
    let mut list: Vec<(String, Option<i64>)> =
        state["items"].as_array().into_iter().flatten().filter_map(|e| Some((e[0].as_str()?.to_string(), e[1].as_i64()))).collect();
    list.sort();
    list
}

/// Items compared by GUID, in any order (REAPER keeps a track's items in time order, which a move changes).
fn same_lanes(a: &Value, b: &Value) -> bool {
    a["mode"] == b["mode"] && a["lanes"] == b["lanes"] && a["plays"] == b["plays"] && a["all"] == b["all"] && lane_items(a) == lane_items(b)
}

/// Whether the lanes are as `state` says for what an undo writes back: the mode, the lanes and how they play, and the lane
/// of each of its items that is there. (Told to undo anyway, an item the producer added since is left on the lane REAPER
/// gives it.)
fn put_as(now: &Value, state: &Value) -> bool {
    let kept: std::collections::HashMap<String, Option<i64>> = lane_items(state).into_iter().collect();
    now["mode"] == state["mode"]
        && now["lanes"] == state["lanes"]
        && now["plays"] == state["plays"]
        && now["all"] == state["all"]
        && lane_items(now).iter().all(|(guid, lane)| kept.get(guid).is_none_or(|kept| kept == lane))
}

/// The mode asked for. A lane count isn't asked: REAPER keeps as many lanes as its items use (it drops lanes left empty at
/// the bottom whenever items change, lab 7.82), so lanes come from items put on them.
fn lanes_wanted(input: &Value) -> Done<i64> {
    if input.get("lanes").is_some_and(|v| !v.is_null()) {
        return Err(bad("lanes isn't asked: REAPER keeps as many lanes as items use; put an item on the lane after the last (lane = the lane count) to add one"));
    }
    let name = input.get("mode").and_then(Value::as_str).unwrap_or_default();
    MODES
        .iter()
        .find(|(word, _)| *word == name)
        .map(|(_, mode)| *mode)
        .ok_or_else(|| bad("mode must be lanes (fixed lanes), free (free item positioning) or normal"))
}

fn mode_word(mode: i64) -> &'static str {
    MODES.iter().find(|(_, m)| *m == mode).map_or("normal", |(word, _)| word)
}

fn lanes_title(ctx: &Ctx, track: Track, mode: i64) -> String {
    let name = named(ctx, track);
    match mode {
        2 => format!("{name}: fixed lanes"),
        1 => format!("{name}: free item positioning"),
        _ => format!("{name}: no lanes"),
    }
}

pub fn lanes_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "track-lanes")?;
    let mode = lanes_wanted(input)?;
    let now = lanes_state(ctx.reaper, found.track);
    let mut display = json!({"title": lanes_title(ctx, found.track, mode)});
    if now["mode"] == mode {
        display["unchanged"] = true.into();
    }
    // Lanes going off leave every item on one lane (REAPER keeps their lanes for when lanes come back).
    if mode != 2 && now["mode"] == 2 {
        let others = now["items"].as_array().into_iter().flatten().filter(|e| e[1].as_i64().unwrap_or(0) != 0).count();
        if others > 0 {
            display["note"] = format!("{others} item(s) on other lanes then play together on one lane; REAPER keeps their lanes for when fixed lanes are on again.").into();
        }
    }
    Ok(Preview { target: target(&found.guid), prior: now.clone(), fence: now, display })
}

pub fn lanes_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "track-lanes")?;
    let reaper = ctx.reaper;
    let now = lanes_state(reaper, found.track);
    if !same_lanes(&now, fence) {
        return Err(changed(format!("{}'s lanes changed since the preview", named(ctx, found.track))));
    }
    let mode = lanes_wanted(input)?;
    let title = lanes_title(ctx, found.track, mode);
    if now["mode"] == mode {
        return Ok(Applied { applied: now, display: json!({"title": title}), unwritten: true, ..Default::default() });
    }
    reaper.set_track_value(found.track, "I_FREEMODE", mode as f64);
    reaper.update_timeline();
    // REAPER settles a track's lanes later on its own (lab, 7.82: an empty track reads 2 lanes, then 1): settled now, in
    // this step, what is kept as applied is what stays.
    if mode == 2 {
        reaper.update_item_lanes(ctx.proj);
    }
    let after = lanes_state(reaper, found.track);
    if after["mode"] != mode {
        put_lanes(ctx, found.track, &now);
        return Err(if same_lanes(&lanes_state(reaper, found.track), &now) {
            failed("REAPER didn't take that lane setting; the track is as it was")
        } else {
            ("uncertain".into(), "REAPER didn't take that lane setting, and Kumi could not put the track's lanes back: read it".into())
        });
    }
    Ok(Applied { applied: after, display: json!({"title": title}), ..Default::default() })
}

/// Puts a track's lanes as `state` says: its mode, then (with fixed lanes) the lane count, how each plays and the lanes of
/// the items still there.
fn put_lanes(ctx: &Ctx, track: Track, state: &Value) {
    let reaper = ctx.reaper;
    let mode = state["mode"].as_i64().unwrap_or(0);
    if reaper.track_value(track, "I_FREEMODE") as i64 != mode {
        reaper.set_track_value(track, "I_FREEMODE", mode as f64);
        reaper.update_timeline();
    }
    if mode != 2 {
        return;
    }
    reaper.update_item_lanes(ctx.proj);
    // (only when REAPER didn't bring the lanes back itself: setting the count leaves lane names in the track's state)
    if let Some(n) = state["lanes"].as_i64().filter(|n| *n != reaper.track_value(track, "I_NUMFIXEDLANES") as i64) {
        reaper.set_track_value(track, "I_NUMFIXEDLANES", n as f64);
    }
    let lanes: std::collections::HashMap<String, i64> =
        state["items"].as_array().into_iter().flatten().filter_map(|e| Some((e[0].as_str()?.to_string(), e[1].as_i64()?))).collect();
    for item in (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n)) {
        if let Some(lane) = lanes.get(&reaper.item_guid(item)) {
            if reaper.item_value(item, "I_FIXEDLANE") as i64 != *lane {
                reaper.set_item_value(item, "I_FIXEDLANE", *lane as f64);
            }
        }
    }
    reaper.update_item_lanes(ctx.proj);
    if let Some(plays) = state["plays"].as_array() {
        write_plays(reaper, track, &plays.iter().filter_map(Value::as_i64).collect::<Vec<_>>(), state["all"].as_i64());
    }
}

/// Which lanes play, as `plays` and `all` (C_ALLLANESPLAY) say. REAPER 7.82 (lab): no lane picked or none playing is
/// `C_ALLLANESPLAY` 1 or 0; picked lanes are one set to play alone (the others go silent), then each other one set to play
/// with others. Lane by lane from no lanes picked would leave every lane picked, which plays the same until a lane is
/// added.
fn write_plays(reaper: &Reaper, track: Track, plays: &[i64], all: Option<i64>) {
    let playing: Vec<usize> = (0..plays.len()).filter(|&lane| plays[lane] != 0).collect();
    match (all, playing.split_first()) {
        (Some(1), _) => {
            reaper.set_track_value(track, "C_ALLLANESPLAY", 1.0);
        }
        (_, None) => {
            reaper.set_track_value(track, "C_ALLLANESPLAY", 0.0);
        }
        (_, Some((first, others))) => {
            reaper.set_track_value(track, &format!("C_LANEPLAYS:{first}"), 1.0);
            for lane in others {
                reaper.set_track_value(track, &format!("C_LANEPLAYS:{lane}"), 2.0);
            }
        }
    }
}

pub fn lanes_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "track-lanes")?;
    let now = lanes_state(ctx.reaper, found.track);
    let name = named(ctx, found.track);
    if same_lanes(&now, restore.prior) {
        return Ok(Restored { restored: now, display: json!({"title": format!("{name}'s lanes are as they were")}), unwritten: true });
    }
    // (its items too: with lanes off, items the producer added or took away since are theirs)
    if !restore.discard && !same_lanes(&now, restore.applied) {
        return Err(changed(format!("{name}'s lanes or items have been changed in REAPER since Kumi set its lanes")));
    }
    put_lanes(ctx, found.track, restore.prior);
    let after = lanes_state(ctx.reaper, found.track);
    if !put_as(&after, restore.prior) {
        // Written, so never a plain failure: put as Kumi left them, the lanes are as before this undo; else it's uncertain.
        put_lanes(ctx, found.track, &now);
        return Err(if put_as(&lanes_state(ctx.reaper, found.track), &now) {
            failed(format!("REAPER didn't put {name}'s lanes back exactly; they are as Kumi left them"))
        } else {
            (
                "uncertain".into(),
                format!("REAPER didn't put {name}'s lanes back exactly, and Kumi could not leave them as they were: read it"),
            )
        });
    }
    Ok(Restored {
        restored: after,
        display: json!({"title": format!("{name}: {} again", mode_word(restore.prior["mode"].as_i64().unwrap_or(0)))}),
        unwritten: false,
    })
}

// ----- which lanes play -----

const PLAYS: [(&str, i64); 3] = [("no", 0), ("only", 1), ("with-others", 2)];

fn plays_wanted(input: &Value) -> Done<i64> {
    let name = input.get("plays").and_then(Value::as_str).unwrap_or_default();
    PLAYS.iter().find(|(word, _)| *word == name).map(|(_, v)| *v).ok_or_else(|| bad("plays must be no, only or with-others"))
}

/// Whether a lane plays as asked: one asked to play with others reads "alone" when no other lane plays.
fn plays_as_asked(plays: &[i64], lane: usize, wanted: i64) -> bool {
    match wanted {
        2 => plays.get(lane).is_some_and(|v| *v == 2 || (*v == 1 && plays.iter().filter(|p| **p != 0).count() == 1)),
        _ => plays.get(lane) == Some(&wanted),
    }
}

fn plays_title(ctx: &Ctx, track: Track, lane: i64, wanted: i64) -> String {
    let name = named(ctx, track);
    match wanted {
        0 => format!("{name}: lane {} silent", lane + 1),
        1 => format!("{name}: only lane {} plays", lane + 1),
        _ => format!("{name}: lane {} plays with the others", lane + 1),
    }
}

pub fn plays_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "lane-plays")?;
    let lane = lane_of(ctx, input, found.track)?;
    let wanted = plays_wanted(input)?;
    let count = lane_count(ctx.reaper, found.track).unwrap_or(1);
    let now = plays_of(ctx.reaper, found.track, count);
    let mut display = json!({"title": plays_title(ctx, found.track, lane, wanted)});
    if plays_as_asked(&now, lane as usize, wanted) {
        display["unchanged"] = true.into();
    }
    let state = json!({"plays": now, "all": all_of(ctx.reaper, found.track)});
    Ok(Preview { target: target(&found.guid), prior: state.clone(), fence: state, display })
}

pub fn plays_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "lane-plays")?;
    let reaper = ctx.reaper;
    let lane = lane_of(ctx, input, found.track)?;
    let wanted = plays_wanted(input)?;
    let count = lane_count(reaper, found.track).unwrap_or(1);
    let now = json!({"plays": plays_of(reaper, found.track, count), "all": all_of(reaper, found.track)});
    if &now != fence {
        return Err(changed(format!("which of {}'s lanes play changed since the preview", named(ctx, found.track))));
    }
    let plays_now: Vec<i64> = now["plays"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    if plays_as_asked(&plays_now, lane as usize, wanted) {
        return Ok(Applied {
            applied: now,
            display: json!({"title": plays_title(ctx, found.track, lane, wanted)}),
            unwritten: true,
            ..Default::default()
        });
    }
    reaper.set_track_value(found.track, &format!("C_LANEPLAYS:{lane}"), wanted as f64);
    let after = plays_of(reaper, found.track, count);
    if !plays_as_asked(&after, lane as usize, wanted) {
        write_plays(reaper, found.track, &plays_now, now["all"].as_i64());
        return Err(failed("REAPER didn't take that; the lanes play as they did"));
    }
    Ok(Applied {
        applied: json!({"plays": after, "all": all_of(reaper, found.track)}),
        display: json!({"title": plays_title(ctx, found.track, lane, wanted)}),
        ..Default::default()
    })
}

pub fn plays_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "lane-plays")?;
    let reaper = ctx.reaper;
    let prior: Vec<i64> = restore.prior["plays"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    let count = lane_count(reaper, found.track).ok_or_else(|| changed("the track has no fixed lanes now"))?;
    let state = |reaper: &Reaper| json!({"plays": plays_of(reaper, found.track, count), "all": all_of(reaper, found.track)});
    let now = state(reaper);
    let name = named(ctx, found.track);
    if &now == restore.prior {
        return Ok(Restored { restored: now, display: json!({"title": format!("{name}'s lanes play as they did")}), unwritten: true });
    }
    if !restore.discard && &now != restore.applied {
        return Err(changed(format!("which of {name}'s lanes play has been changed in REAPER since")));
    }
    if prior.len() as i64 != count {
        return Err(changed(format!("{name} has another number of lanes now")));
    }
    write_plays(reaper, found.track, &prior, restore.prior["all"].as_i64());
    let after = state(reaper);
    if &after != restore.prior {
        return Err(failed("REAPER didn't make the lanes play as they did"));
    }
    Ok(Restored { restored: after, display: json!({"title": format!("{name}'s lanes play as they did again")}), unwritten: false })
}

// ----- lane names -----

fn lane_name(reaper: &Reaper, track: Track, lane: i64) -> String {
    reaper.track_string(track, &format!("P_LANENAME:{lane}")).unwrap_or_default()
}

fn name_wanted(input: &Value) -> Done<String> {
    match input.get("name") {
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Ok(name.clone()),
        _ => Err(bad("name must be one line of at most 256 characters")),
    }
}

/// Every lane's name, top first: a lane is told by its place, which a lane added or taken away above it moves.
pub(super) fn lane_names(reaper: &Reaper, track: Track) -> Vec<String> {
    (0..lane_count(reaper, track).unwrap_or(0)).map(|lane| lane_name(reaper, track, lane)).collect()
}

pub fn name_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "lane-name")?;
    let lane = lane_of(ctx, input, found.track)?;
    let wanted = name_wanted(input)?;
    let now = lane_name(ctx.reaper, found.track, lane);
    let mut display = json!({"title": format!("{}: lane {} named {}", named(ctx, found.track), lane + 1, quoted(&wanted))});
    if now == wanted {
        display["unchanged"] = true.into();
    }
    let state = json!({"lane": lane, "name": now, "names": lane_names(ctx.reaper, found.track)});
    Ok(Preview { target: target(&found.guid), prior: state.clone(), fence: state, display })
}

pub fn name_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "lane-name")?;
    let lane = lane_of(ctx, input, found.track)?;
    let wanted = name_wanted(input)?;
    let now = lane_name(ctx.reaper, found.track, lane);
    let names_moved = fence.get("names").is_some_and(|names| *names != json!(lane_names(ctx.reaper, found.track)));
    if fence["name"].as_str() != Some(now.as_str()) || fence["lane"].as_i64() != Some(lane) || names_moved {
        return Err(changed(format!("{}'s lanes were added, taken away or renamed since the preview", named(ctx, found.track))));
    }
    let title = format!("{}: lane {} named {}", named(ctx, found.track), lane + 1, quoted(&wanted));
    if now == wanted {
        return Ok(Applied {
            applied: json!({"lane": lane, "name": now, "names": lane_names(ctx.reaper, found.track)}),
            display: json!({"title": title}),
            unwritten: true,
            ..Default::default()
        });
    }
    if !ctx.reaper.set_track_string(found.track, &format!("P_LANENAME:{lane}"), &wanted)
        || lane_name(ctx.reaper, found.track, lane) != wanted
    {
        return Err(failed("REAPER didn't take that lane name"));
    }
    Ok(Applied {
        applied: json!({"lane": lane, "name": wanted, "names": lane_names(ctx.reaper, found.track)}),
        display: json!({"title": title}),
        ..Default::default()
    })
}

pub fn name_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "lane-name")?;
    let lane = restore.prior["lane"].as_i64().ok_or_else(|| bad("prior: the lane"))?;
    if lane_count(ctx.reaper, found.track).is_none_or(|count| lane >= count) {
        return Err(gone("that lane isn't on the track any more"));
    }
    let now = lane_name(ctx.reaper, found.track, lane);
    let prior = restore.prior["name"].as_str().unwrap_or_default();
    let name = named(ctx, found.track);
    // The lanes by their names: a lane added or taken away above it (a comp lane on top) moves every lane down or up, and
    // the lane at its number is another one (as are "back already" and a rename there). It is still Kumi's while the lanes
    // above it read as Kumi left them, or while there are as many lanes as then (others renamed); lanes below don't count
    // (REAPER drops empty ones at the bottom). Kept since lanes were.
    let names = lane_names(ctx.reaper, found.track);
    if let Some(then) = restore.applied.get("names").and_then(Value::as_array) {
        let then: Vec<&str> = then.iter().filter_map(Value::as_str).collect();
        let at = lane as usize;
        let above_same = names.len() > at && then.len() > at && names[..at].iter().zip(&then[..at]).all(|(a, b)| a == b);
        if !(above_same || names.len() == then.len()) {
            return Err(changed(format!(
                "{name}'s lanes have been added or taken away above lane {} in REAPER since: it isn't the lane Kumi named, so Kumi \
                 leaves them, even with discard",
                lane + 1
            )));
        }
    }
    if now == prior {
        return Ok(Restored {
            restored: restore.prior.clone(),
            display: json!({"title": format!("{name}'s lane {} has its name back already", lane + 1)}),
            unwritten: true,
        });
    }
    if !restore.discard && Some(now.as_str()) != restore.applied["name"].as_str() {
        return Err(changed(format!("{name}'s lane {} has been renamed in REAPER since", lane + 1)));
    }
    if !ctx.reaper.set_track_string(found.track, &format!("P_LANENAME:{lane}"), prior) || lane_name(ctx.reaper, found.track, lane) != prior
    {
        return Err(failed("REAPER didn't take the lane's name back"));
    }
    Ok(Restored {
        restored: restore.prior.clone(),
        display: json!({"title": format!("{name}: lane {} named {} again", lane + 1, quoted(prior))}),
        unwritten: false,
    })
}

// ----- razor areas -----

/// One razor area: seconds, the envelope's GUID ("" for the media lane), and the share of the track's height it covers.
#[derive(Debug, Clone, PartialEq)]
struct Area {
    start: f64,
    end: f64,
    envelope: String,
    span: Option<(f64, f64)>,
}

/// `P_RAZOREDITS_EXT`: areas apart by commas, each `start end "envelope" [top bottom]`.
fn areas_of(text: &str) -> Vec<Area> {
    text.split(',')
        .filter_map(|area| {
            let parts: Vec<&str> = area.split_whitespace().collect();
            let number = |at: usize| parts.get(at).and_then(|p| p.parse::<f64>().ok());
            let (start, end) = (number(0)?, number(1)?);
            let envelope = parts.get(2).map(|e| e.trim_matches('"').to_string()).unwrap_or_default();
            let span = number(3).zip(number(4));
            Some(Area { start, end, envelope, span })
        })
        .collect()
}

fn same_areas(a: &[Area], b: &[Area]) -> bool {
    // A media-lane area with no y-range is the whole track: REAPER writes 0 and 1 for it.
    let span = |area: &Area| area.span.unwrap_or((0.0, 1.0));
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let (sx, sy) = (span(x), span(y));
            (x.start - y.start).abs() < 1e-6
                && (x.end - y.end).abs() < 1e-6
                && x.envelope == y.envelope
                && (sx.0 - sy.0).abs() < 1e-5
                && (sx.1 - sy.1).abs() < 1e-5
        })
}

fn text_of(areas: &[Area]) -> String {
    areas
        .iter()
        .map(|area| match area.span {
            Some((top, bottom)) => format!("{:.14} {:.14} \"{}\" {top:.6} {bottom:.6}", area.start, area.end, area.envelope),
            None => format!("{:.14} {:.14} \"{}\"", area.start, area.end, area.envelope),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn razor_text(reaper: &Reaper, track: Track) -> String {
    reaper.track_string(track, "P_RAZOREDITS_EXT").unwrap_or_default()
}

fn areas_wanted(ctx: &Ctx, input: &Value, found: &TrackFound) -> Done<Vec<Area>> {
    let list = input.get("areas").and_then(Value::as_array).ok_or_else(|| bad("areas must be a list ([] clears them)"))?;
    if list.len() > MOST_AREAS {
        return Err(bad(format!("areas holds at most {MOST_AREAS} areas")));
    }
    let reaper = ctx.reaper;
    let lanes = lane_count(reaper, found.track);
    let mut areas = Vec::new();
    for spec in list {
        let start = super::marks::moment(ctx, spec, "startBeats", "start")?.ok_or_else(|| bad("every area needs startBeats or start"))?;
        let end = super::marks::moment(ctx, spec, "endBeats", "end")?.ok_or_else(|| bad("every area needs endBeats or end"))?;
        if end <= start + 1e-9 {
            return Err(bad("an area must end after it starts"));
        }
        let envelope = match spec.get("envelopeRef").filter(|v| !v.is_null()) {
            None => String::new(),
            Some(value) => {
                let text = value.as_str().ok_or_else(|| bad("envelopeRef must be an envelope ref"))?;
                match Ref::parse(text).map_err(bad)? {
                    Ref::Envelope { track, which: EnvelopeId::Guid(guid) } if track == TrackId::Guid(found.guid.clone()) => {
                        let envelope = reaper
                            .track_envelope_by_chunk_name(found.track, &guid)
                            .ok_or_else(|| failed("that envelope isn't on the track"))?;
                        reaper.envelope_guid(envelope).unwrap_or(guid)
                    }
                    Ref::Envelope { .. } => {
                        return Err(bad("envelopeRef must be one of this track's envelopes, by its GUID ref as the reads give it"))
                    }
                    // m4-r-fxenv's take envelope refs: REAPER has no razor areas on a take's envelopes.
                    Ref::TakeEnvelope { .. } => {
                        return Err(bad("a take's envelope has no razor areas in REAPER: give one of this track's envelopes"))
                    }
                    other => return Err(bad(format!("envelopeRef must be an envelope ref, not r:{}", other.kind()))),
                }
            }
        };
        let span = match spec.get("lane").filter(|v| !v.is_null()) {
            None if envelope.is_empty() => Some((0.0, 1.0)),
            None => None,
            Some(_) if !envelope.is_empty() => return Err(bad("an envelope's area has no lane")),
            Some(value) => {
                let count = lanes.ok_or_else(|| bad("lane: that track has no fixed lanes"))?;
                let lane = value
                    .as_i64()
                    .filter(|n| (0..count).contains(n))
                    .ok_or_else(|| bad(format!("lane must be from 0 to {} on that track", count - 1)))?;
                Some((lane as f64 / count as f64, (lane + 1) as f64 / count as f64))
            }
        };
        areas.push(Area { start, end, envelope, span });
    }
    Ok(areas)
}

fn razor_title(ctx: &Ctx, track: Track, areas: &[Area]) -> String {
    let name = named(ctx, track);
    match areas.len() {
        0 => format!("{name}: razor areas cleared"),
        1 => format!(
            "{name}: a razor area from {} to {}",
            super::marks::bar_text(ctx.reaper, ctx.proj, areas[0].start),
            super::marks::bar_text(ctx.reaper, ctx.proj, areas[0].end)
        ),
        n => format!("{name}: {n} razor areas"),
    }
}

pub fn razor_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], "razor-set")?;
    let wanted = areas_wanted(ctx, input, &found)?;
    let now = razor_text(ctx.reaper, found.track);
    let mut display = json!({"title": razor_title(ctx, found.track, &wanted)});
    if same_areas(&areas_of(&now), &wanted) {
        display["unchanged"] = true.into();
    }
    let state = json!({"razor": now});
    Ok(Preview { target: target(&found.guid), prior: state.clone(), fence: state, display })
}

pub fn razor_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, &input["ref"], "razor-set")?;
    let reaper = ctx.reaper;
    let now = razor_text(reaper, found.track);
    if !same_areas(&areas_of(&now), &areas_of(fence["razor"].as_str().unwrap_or_default())) {
        return Err(changed(format!("{}'s razor areas changed since the preview", named(ctx, found.track))));
    }
    let wanted = areas_wanted(ctx, input, &found)?;
    let title = razor_title(ctx, found.track, &wanted);
    if same_areas(&areas_of(&now), &wanted) {
        return Ok(Applied { applied: json!({"razor": now}), display: json!({"title": title}), unwritten: true, ..Default::default() });
    }
    reaper.set_track_string(found.track, "P_RAZOREDITS_EXT", &text_of(&wanted));
    let after = razor_text(reaper, found.track);
    if !same_areas(&areas_of(&after), &wanted) {
        reaper.set_track_string(found.track, "P_RAZOREDITS_EXT", &now);
        return Err(failed("REAPER didn't take those razor areas; the track's are as they were"));
    }
    Ok(Applied { applied: json!({"razor": after}), display: json!({"title": title}), ..Default::default() })
}

pub fn razor_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, &restore.target["ref"], "razor-set")?;
    let reaper = ctx.reaper;
    let now = areas_of(&razor_text(reaper, found.track));
    let prior_text = restore.prior["razor"].as_str().unwrap_or_default();
    let name = named(ctx, found.track);
    if same_areas(&now, &areas_of(prior_text)) {
        return Ok(Restored {
            restored: restore.prior.clone(),
            display: json!({"title": format!("{name}'s razor areas are as they were")}),
            unwritten: true,
        });
    }
    if !restore.discard && !same_areas(&now, &areas_of(restore.applied["razor"].as_str().unwrap_or_default())) {
        return Err(changed(format!("{name}'s razor areas have been changed in REAPER since")));
    }
    reaper.set_track_string(found.track, "P_RAZOREDITS_EXT", prior_text);
    if !same_areas(&areas_of(&razor_text(reaper, found.track)), &areas_of(prior_text)) {
        return Err(failed("REAPER didn't take the razor areas back"));
    }
    Ok(Restored { restored: restore.prior.clone(), display: json!({"title": format!("{name}'s razor areas put back")}), unwritten: false })
}

// ----- comping -----

/// The comp lane a track records its comp into, from its chunk's `LANEREC` line (lab, 7.82: its second number is the comp
/// lane, -1 while the track isn't comping).
fn comp_lane(chunk: &str) -> Option<i64> {
    chunk
        .lines()
        .take_while(|line| !line.starts_with("<ITEM"))
        .find_map(|line| line.strip_prefix("LANEREC "))
        .and_then(|rest| rest.split_whitespace().nth(1)?.parse::<i64>().ok())
        .filter(|lane| *lane >= 0)
}

fn track_chunk(reaper: &Reaper, track: Track) -> Done<String> {
    let chunk = reaper.track_chunk(track).ok_or_else(|| failed("REAPER wouldn't give the track's state"))?;
    if chunk.len() > chunks::MOST_KEPT {
        return Err(failed(format!(
            "this track's state is {} MB, more than the {} MB Kumi keeps to put it back: Kumi won't comp what it can't put back",
            chunk.len() >> 20,
            chunks::MOST_KEPT >> 20
        )));
    }
    Ok(chunk)
}

struct Comp {
    found: TrackFound,
    lane: i64,
    start: f64,
    end: f64,
}

fn comp_plan(ctx: &Ctx, input: &Value) -> Done<Comp> {
    let found = find(ctx, &input["ref"], "lane-comp")?;
    let lane = lane_of(ctx, input, found.track)?;
    let start = super::marks::moment(ctx, input, "startBeats", "start")?
        .ok_or_else(|| bad("pass startBeats or start: where the comp area begins"))?;
    let end = super::marks::moment(ctx, input, "endBeats", "end")?.ok_or_else(|| bad("pass endBeats or end: where the comp area ends"))?;
    if end <= start + 1e-6 {
        return Err(bad("the comp area must end after it starts"));
    }
    Ok(Comp { found, lane, start, end })
}

fn comp_title(ctx: &Ctx, comp: &Comp) -> String {
    format!(
        "{}: comped lane {} from {} to {}",
        named(ctx, comp.found.track),
        comp.lane + 1,
        super::marks::bar_text(ctx.reaper, ctx.proj, comp.start),
        super::marks::bar_text(ctx.reaper, ctx.proj, comp.end)
    )
}

pub fn comp_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let comp = comp_plan(ctx, input)?;
    let chunk = track_chunk(ctx.reaper, comp.found.track)?;
    let mut display = json!({"title": comp_title(ctx, &comp)});
    if comp_lane(&chunk).is_none() {
        display["note"] = "The track isn't comping yet: REAPER makes a comp lane on top first (the other lanes move down one).".into();
    }
    if comp_lane(&chunk) == Some(comp.lane) {
        return Err(bad("that lane is the comp lane: comp another lane into it"));
    }
    let print = chunks::print_of(&chunk);
    Ok(Preview { target: target(&comp.found.guid), prior: json!({"print": print}), fence: json!({"print": print}), display })
}

pub fn comp_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let comp = comp_plan(ctx, input).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    let reaper = ctx.reaper;
    let track = comp.found.track;
    let chunk = track_chunk(reaper, track)?;
    if Some(chunks::print_of(&chunk).as_str()) != fence["print"].as_str() {
        return Err(changed(format!("{} changed since the preview", named(ctx, track))));
    }
    // Both actions are checked by name before anything is written.
    for (command, name) in [COMP_INTO_NEW_LANE, COMP_AREA_FROM_RAZOR] {
        if reaper.action_name(command).as_deref() != Some(name) {
            return Err(failed(format!("this REAPER has no action {command} named “{name}”, so Kumi won't comp")));
        }
    }
    let items_before: Vec<String> =
        (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n)).map(|item| reaper.item_guid(item)).collect();
    // The producer's selection and razor areas, on every track: the comp action works on all razor areas there are.
    let selected: Vec<String> = (0..reaper.track_count(ctx.proj))
        .filter_map(|n| reaper.track_at(ctx.proj, n))
        .filter(|t| reaper.track_value(*t, "I_SELECTED") != 0.0)
        .map(|t| reaper.track_guid(t))
        .collect();
    let razors: Vec<(String, String)> = (0..reaper.track_count(ctx.proj))
        .filter_map(|n| reaper.track_at(ctx.proj, n))
        .map(|t| (reaper.track_guid(t), razor_text(reaper, t)))
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    reaper.prevent_ui_refresh(1);
    let mut lane = comp.lane;
    if comp_lane(&chunk).is_none() {
        reaper.select_only(track);
        reaper.run_action(COMP_INTO_NEW_LANE.0);
        // The comp lane is on top now: every other lane moved down one.
        lane += 1;
    }
    for (guid, _) in &razors {
        if let Some(other) = track_by_guid(ctx, guid) {
            reaper.set_track_string(other, "P_RAZOREDITS_EXT", "");
        }
    }
    let count = lane_count(reaper, track).unwrap_or(1) as f64;
    let area =
        Area { start: comp.start, end: comp.end, envelope: String::new(), span: Some((lane as f64 / count, (lane + 1) as f64 / count)) };
    reaper.set_track_string(track, "P_RAZOREDITS_EXT", &text_of(&[area]));
    reaper.run_action(COMP_AREA_FROM_RAZOR.0);
    // The producer's razor areas and selection, as they were.
    reaper.set_track_string(track, "P_RAZOREDITS_EXT", "");
    for (guid, text) in &razors {
        if let Some(other) = track_by_guid(ctx, guid) {
            reaper.set_track_string(other, "P_RAZOREDITS_EXT", text);
        }
    }
    for other in (0..reaper.track_count(ctx.proj)).filter_map(|n| reaper.track_at(ctx.proj, n)) {
        let was = selected.contains(&reaper.track_guid(other));
        if (reaper.track_value(other, "I_SELECTED") != 0.0) != was {
            reaper.set_track_selected(other, was);
        }
    }
    reaper.prevent_ui_refresh(-1);
    let made: Vec<String> = (0..reaper.track_item_count(track))
        .filter_map(|n| reaper.track_item(track, n))
        .map(|item| (reaper.item_guid(item), item))
        .filter(|(guid, _)| !items_before.contains(guid))
        .map(|(guid, item)| {
            ctx.reads.registry().note_item(ctx.proj, &guid, item);
            guid
        })
        .collect();
    let after = reaper
        .track_chunk(track)
        .ok_or_else(|| ("uncertain".to_string(), "REAPER comped but wouldn't give the track's state: read it".to_string()))?;
    if made.is_empty() {
        // Nothing was comped (no media in that span of the lane): the track goes back as it was.
        let back = reaper.set_track_chunk(track, &chunk);
        return Err(if back {
            bad(format!("there is no media on lane {} between those times to comp: the track is as it was", comp.lane + 1))
        } else {
            ("uncertain".into(), "REAPER comped nothing, and Kumi could not put the track back: read it".into())
        });
    }
    Ok(Applied {
        applied: json!({"print": chunks::print_of(&after)}),
        created: Some(json!({"ref": refs::item_ref(&made[0]), "kind": "item"})),
        material: Some(json!({"chunk": chunk})),
        display: json!({"title": comp_title(ctx, &comp)}),
        ..Default::default()
    })
}

pub fn comp_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the track from before the comp"))?;
    let chunk = material["chunk"].as_str().ok_or_else(|| bad("material: the track's state"))?;
    let found = find_for_restore(ctx, &restore.target["ref"], "lane-comp")?;
    let reaper = ctx.reaper;
    let print = chunks::print_of(&track_chunk(reaper, found.track)?);
    let name = named(ctx, found.track);
    if Some(print.as_str()) == restore.prior["print"].as_str() {
        return Ok(Restored {
            restored: json!({"ref": refs::track_ref(&TrackId::Guid(found.guid.clone()))}),
            display: json!({"title": format!("{name} is as it was before the comp")}),
            unwritten: true,
        });
    }
    // (discard too: the undo writes the whole track back, which would take the producer's edits since with it)
    if Some(print.as_str()) != restore.applied["print"].as_str() {
        return Err(changed(format!(
            "{name} has been changed in REAPER since Kumi comped it: putting the track back as it was before the comp would \
             write over those changes too (its lanes, items, FX, automation and routing), so Kumi leaves it, even with discard"
        )));
    }
    let selected = reaper.track_value(found.track, "I_SELECTED") != 0.0;
    if !reaper.set_track_chunk(found.track, chunk) {
        return Err(failed("REAPER wouldn't take the track's state back"));
    }
    reaper.set_track_selected(found.track, selected);
    Ok(Restored {
        restored: json!({"ref": refs::track_ref(&TrackId::Guid(found.guid))}),
        display: json!({"title": format!("{name} as it was before the comp")}),
        unwritten: false,
    })
}
