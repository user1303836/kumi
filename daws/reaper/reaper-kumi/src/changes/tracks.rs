//! Changes to a track: its volume, pan, mute, solo, name and colour; and deleting it, with what undo needs to make it
//! again.

use serde_json::{json, Map, Value};

use super::{folders, track_ref_of, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, Track};
use crate::refs::{self, TrackId};
use crate::rows;

/// A track attribute Kumi changes: how it is read, parsed from the model's input, written, compared and worded. The state
/// of an attribute is an object whose first key is the exact value (what is compared); the rest is for the model.
pub struct Prop {
    pub kind: &'static str,
    /// What the master track has none of (a name, a solo).
    master: bool,
    read: fn(&Reaper, Track) -> Value,
    /// The state the input asks for.
    wanted: fn(&Reaper, &Value) -> Done<Value>,
    write: fn(&Reaper, Track, &Value) -> bool,
    same: fn(&Value, &Value) -> bool,
    /// The title of the change, with the track's name: "Bass volume −6 dB".
    title: fn(&str, &Value, &Value) -> String,
    /// What was changed, for the error when REAPER keeps the old value.
    noun: &'static str,
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// dB with the true minus sign, as the titles show it: `−6`, `+3`, `0`, `−6.5`.
pub fn db_text(db: f64) -> String {
    if db <= rows::MIN_DB + 0.5 {
        return "−∞".into();
    }
    let rounded = (db * 10.0).round() / 10.0;
    if rounded == 0.0 {
        "0".into()
    } else if rounded < 0.0 {
        format!("−{}", trimmed(-rounded))
    } else {
        format!("+{}", trimmed(rounded))
    }
}

fn trimmed(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
}

pub fn pan_text(pan: f64) -> String {
    let percent = (pan.abs() * 100.0).round() as i64;
    match percent {
        0 => "centre".into(),
        n if pan < 0.0 => format!("{n}% left"),
        n => format!("{n}% right"),
    }
}

// ----- the attributes -----

fn volume_state(gain: f64) -> Value {
    json!({"volume": gain, "volumeDb": rows::db(gain)})
}

fn color_state(native: i32) -> Value {
    json!({"color": native})
}

/// The colour a state shows: none (0) unless REAPER's flag says one is set. A track with none can still hold a number below
/// the flag, which means nothing and is put back as it was.
pub fn colour_of(state: &Value) -> i64 {
    let native = state["color"].as_i64().unwrap_or(0);
    if native & 0x100_0000 == 0 {
        0
    } else {
        native & 0x1ff_ffff
    }
}

pub static PROPS: [Prop; 7] = [
    Prop {
        kind: "track-volume",
        master: true,
        read: |reaper, track| volume_state(reaper.track_value(track, "D_VOL")),
        wanted: |_, input| {
            let db = input
                .get("volumeDb")
                .and_then(Value::as_f64)
                .filter(|db| db.is_finite())
                .ok_or_else(|| bad("volumeDb must be a number"))?;
            if !(-150.0..=24.0).contains(&db) {
                return Err(bad("volumeDb must be from -150 (silent) to 24"));
            }
            Ok(volume_state(if db <= -150.0 { 0.0 } else { 10f64.powf(db / 20.0) }))
        },
        write: |reaper, track, state| reaper.set_track_value(track, "D_VOL", number(state, "volume")),
        same: |a, b| close(number(a, "volume"), number(b, "volume")),
        title: |name, _, to| format!("{name} volume {} dB", db_text(number(to, "volumeDb"))),
        noun: "volume",
    },
    Prop {
        kind: "track-pan",
        master: true,
        read: |reaper, track| json!({"pan": reaper.track_value(track, "D_PAN")}),
        wanted: |_, input| {
            let pan = input
                .get("pan")
                .and_then(Value::as_f64)
                .filter(|pan| (-1.0..=1.0).contains(pan))
                .ok_or_else(|| bad("pan must be a number from -1 (left) to 1 (right)"))?;
            Ok(json!({"pan": pan}))
        },
        write: |reaper, track, state| reaper.set_track_value(track, "D_PAN", number(state, "pan")),
        same: |a, b| close(number(a, "pan"), number(b, "pan")),
        title: |name, _, to| format!("{name} pan {}", pan_text(number(to, "pan"))),
        noun: "pan",
    },
    Prop {
        kind: "track-mute",
        master: true,
        read: |reaper, track| json!({"mute": reaper.track_value(track, "B_MUTE") != 0.0}),
        wanted: |_, input| {
            let mute = input.get("mute").and_then(Value::as_bool).ok_or_else(|| bad("mute must be true or false"))?;
            Ok(json!({"mute": mute}))
        },
        write: |reaper, track, state| reaper.set_track_value(track, "B_MUTE", if state["mute"] == true { 1.0 } else { 0.0 }),
        same: |a, b| a["mute"] == b["mute"],
        title: |name, _, to| format!("{name} {}", if to["mute"] == true { "muted" } else { "unmuted" }),
        noun: "mute",
    },
    Prop {
        kind: "track-channels",
        master: true,
        read: |reaper, track| json!({"channels": reaper.track_value(track, "I_NCHAN") as i64}),
        wanted: |_, input| {
            let channels = input
                .get("channels")
                .and_then(Value::as_i64)
                .filter(|n| (2..=128).contains(n) && n % 2 == 0)
                .ok_or_else(|| bad("channels must be an even number from 2 to 128"))?;
            Ok(json!({"channels": channels}))
        },
        write: |reaper, track, state| reaper.set_track_value(track, "I_NCHAN", number(state, "channels")),
        same: |a, b| a["channels"].as_i64() == b["channels"].as_i64(),
        title: |name, _, to| format!("{name} {} channels", to["channels"]),
        noun: "channels",
    },
    Prop {
        kind: "track-solo",
        master: false,
        // REAPER's solo is a number: 0 off, 1 solo, 2 solo in place, and the safe states above. What is soloed in any way
        // counts as soloed; the number is kept exactly for undo.
        read: |reaper, track| {
            let solo = reaper.track_value(track, "I_SOLO") as i64;
            json!({"solo": solo, "soloed": solo != 0})
        },
        wanted: |_, input| {
            let soloed = input.get("solo").and_then(Value::as_bool).ok_or_else(|| bad("solo must be true or false"))?;
            Ok(json!({"solo": if soloed { 1 } else { 0 }, "soloed": soloed}))
        },
        write: |reaper, track, state| reaper.set_track_value(track, "I_SOLO", state["solo"].as_i64().unwrap_or(0) as f64),
        same: |a, b| a["solo"] == b["solo"],
        title: |name, _, to| format!("{name} {}", if to["soloed"] == true { "soloed" } else { "unsoloed" }),
        noun: "solo",
    },
    Prop {
        kind: "track-rename",
        master: false,
        read: |reaper, track| json!({"name": reaper.track_name(track)}),
        wanted: |_, input| {
            let name = input.get("name").and_then(Value::as_str).ok_or_else(|| bad("name must be text"))?;
            if name.chars().count() > 256 || name.chars().any(char::is_control) {
                return Err(bad("name must be one line of at most 256 characters"));
            }
            Ok(json!({"name": name}))
        },
        write: |reaper, track, state| reaper.set_track_string(track, "P_NAME", state["name"].as_str().unwrap_or_default()),
        same: |a, b| a["name"] == b["name"],
        title: |_, from, to| {
            let (from, to) = (from["name"].as_str().unwrap_or_default(), to["name"].as_str().unwrap_or_default());
            if from.is_empty() {
                format!("Named track {}", super::quoted(to))
            } else {
                format!("Renamed track {} → {}", super::quoted(from), super::quoted(to))
            }
        },
        noun: "name",
    },
    Prop {
        kind: "track-color",
        master: true,
        read: |reaper, track| color_state(reaper.track_value(track, "I_CUSTOMCOLOR") as i32),
        wanted: |reaper, input| match input.get("color") {
            Some(Value::Null) => Ok(color_state(0)),
            Some(Value::String(hex)) => {
                let digits = hex.strip_prefix('#').filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()));
                let digits = digits.ok_or_else(|| bad("color must be #rrggbb, or null for no colour"))?;
                let channel = |at: usize| i32::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
                reaper
                    .color_to_native(channel(0), channel(2), channel(4))
                    .map(|native| color_state(native | 0x100_0000))
                    .ok_or_else(|| failed("this REAPER can't make a colour"))
            }
            _ => Err(bad("color must be #rrggbb, or null for no colour")),
        },
        write: |reaper, track, state| reaper.set_track_value(track, "I_CUSTOMCOLOR", number(state, "color")),
        same: |a, b| colour_of(a) == colour_of(b),
        title: |name, _, to| {
            if colour_of(to) == 0 {
                format!("{name} colour cleared")
            } else {
                format!("{name} colour changed")
            }
        },
        noun: "colour",
    },
];

/// A track a request names: its handle, its name, and whether it is the master.
struct Found {
    track: Track,
    id: TrackId,
    name: String,
}

fn find(ctx: &Ctx, reference: &Value, what: &str) -> Done<Found> {
    let id = track_ref_of(&json!({"ref": reference}), what)?;
    let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id)?;
    let name = if id == TrackId::Master { "Master".to_string() } else { rows::clip(&ctx.reaper.track_name(track)) };
    Ok(Found { track, id, name })
}

fn target(id: &TrackId) -> Value {
    json!({"ref": refs::track_ref(id), "kind": "track"})
}

/// A title with the producer's name for the track: a track with no name is the number REAPER shows.
fn named(ctx: &Ctx, found: &Found) -> String {
    if found.id == TrackId::Master {
        return "Master".into();
    }
    if found.name.is_empty() {
        return format!("Track {}", ctx.reaper.track_value(found.track, "IP_TRACKNUMBER") as i64);
    }
    found.name.clone()
}

pub fn prop_preview(ctx: &Ctx, prop: &Prop, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"], prop.kind)?;
    if found.id == TrackId::Master && !prop.master {
        return Err(bad(format!("the master track has no {}", prop.noun)));
    }
    let now = (prop.read)(ctx.reaper, found.track);
    let wanted = (prop.wanted)(ctx.reaper, input)?;
    let title = (prop.title)(&named(ctx, &found), &now, &wanted);
    let unchanged = (prop.same)(&now, &wanted);
    let mut display = json!({"title": title, "before": now, "after": wanted});
    if unchanged {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(&found.id), prior: now.clone(), fence: now, display })
}

pub fn prop_apply(ctx: &Ctx, prop: &Prop, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find(ctx, &input["ref"], prop.kind).map_err(gone_is_changed)?;
    let now = (prop.read)(ctx.reaper, found.track);
    if !(prop.same)(&now, fence) {
        return Err(("changed".into(), format!("{}'s {} changed since the preview", named(ctx, &found), prop.noun)));
    }
    let wanted = (prop.wanted)(ctx.reaper, input)?;
    if !(prop.same)(&now, &wanted) && !(prop.write)(ctx.reaper, found.track, &wanted) {
        return Err(failed(format!("REAPER wouldn't take the {}", prop.noun)));
    }
    let after = (prop.read)(ctx.reaper, found.track);
    if !(prop.same)(&after, &wanted) {
        return Err(failed(format!("REAPER kept {}'s {} as it was", named(ctx, &found), prop.noun)));
    }
    let title = (prop.title)(&named(ctx, &found), &now, &after);
    Ok(Applied {
        applied: after.clone(),
        created: None,
        material: None,
        display: json!({"title": title, "before": now, "after": after}),
        unwritten: (prop.same)(&now, &wanted),
    })
}

pub fn prop_restore(ctx: &Ctx, prop: &Prop, restore: &Restore) -> Done<Restored> {
    let reference = restore.target.get("ref").ok_or_else(|| bad("target: the track's ref"))?;
    let found = find(ctx, reference, prop.kind)
        .map_err(|(code, message)| if code == "failed" { ("gone".to_string(), message) } else { (code, message) })?;
    let now = (prop.read)(ctx.reaper, found.track);
    let name = named(ctx, &found);
    // Already as it was: a retry of a restore that went through.
    let already = (prop.same)(&now, restore.prior);
    if !already {
        if !restore.discard && !(prop.same)(&now, restore.applied) {
            return Err(("changed".into(), format!("{name}'s {} changed in REAPER since Kumi set it", prop.noun)));
        }
        if !(prop.write)(ctx.reaper, found.track, restore.prior) {
            return Err(failed(format!("REAPER wouldn't put back the {}", prop.noun)));
        }
        let after = (prop.read)(ctx.reaper, found.track);
        if !(prop.same)(&after, restore.prior) {
            return Err(failed(format!("REAPER kept {name}'s {} as it was", prop.noun)));
        }
    }
    let after = (prop.read)(ctx.reaper, found.track);
    let title = (prop.title)(&name, &now, &after);
    Ok(Restored {
        restored: after.clone(),
        display: json!({"title": format!("{title} (put back)"), "before": now, "after": after}),
        unwritten: already,
    })
}

/// An object that was there for the preview and isn't for the apply has changed since.
fn gone_is_changed((code, message): super::Fault) -> super::Fault {
    if code == "failed" {
        ("changed".into(), format!("{message}: it went since the preview"))
    } else {
        (code, message)
    }
}

// ----- deleting a track -----

/// What tells a track apart from what the preview saw: its place, its name and how much it holds.
fn signature(reaper: &Reaper, track: Track) -> Value {
    json!({
        "name": reaper.track_name(track),
        "index": reaper.track_value(track, "IP_TRACKNUMBER") as i64,
        "items": reaper.track_item_count(track),
        "fx": reaper.fx_count(crate::reaper::FxOwner::Track(track)),
        "sends": reaper.send_count(track, 0),
        "receives": reaper.send_count(track, -1),
        "folderDepth": reaper.track_value(track, "I_FOLDERDEPTH") as i64,
    })
}

fn find_to_delete(ctx: &Ctx, input: &Value) -> Done<Found> {
    let found = find(ctx, &input["ref"], "track-delete")?;
    if found.id == TrackId::Master {
        return Err(bad("the master track can't be deleted"));
    }
    Ok(found)
}

pub fn delete_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find_to_delete(ctx, input)?;
    let now = signature(ctx.reaper, found.track);
    let held = now["items"].as_i64().unwrap_or(0) + now["fx"].as_i64().unwrap_or(0);
    let mut display = json!({"title": format!("Deleted track {}", super::quoted(&named(ctx, &found)))});
    let mut notes = Vec::new();
    if held > 0 {
        notes.push(format!("It holds {} items and {} FX; Kumi keeps them to put back.", now["items"], now["fx"]));
    }
    if let Some(note) = folder_note(ctx, &found) {
        notes.push(note);
    }
    if !notes.is_empty() {
        display["note"] = notes.join(" ").into();
    }
    Ok(Preview { target: target(&found.id), prior: now.clone(), fence: now, display })
}

/// What a delete does to folders, when it changes any track's folder: the tracks of a folder that goes stay where they are,
/// one level up.
fn folder_note(ctx: &Ctx, found: &Found) -> Option<String> {
    let tracks = folder_depths(ctx);
    let guid = ctx.reaper.track_guid(found.track);
    let at = tracks.iter().position(|(id, _)| *id == guid)?;
    let layout = folders::layout(&tracks.iter().map(|(_, depth)| *depth).collect::<Vec<_>>());
    let inside: Vec<String> =
        (0..tracks.len()).filter(|other| layout.parent[*other] == Some(at)).map(|other| tracks[other].0.clone()).collect();
    if inside.is_empty() {
        return None;
    }
    let names: Vec<String> = inside.iter().take(3).map(|id| super::quoted(&name_of(ctx, id))).collect();
    let more = if inside.len() > 3 { format!(" and {} more", inside.len() - 3) } else { String::new() };
    let up = match layout.parent[at] {
        Some(parent) => format!("the folder {}", super::quoted(&name_of(ctx, &tracks[parent].0))),
        None => "the top level".to_string(),
    };
    Some(format!(
        "{}{} {} in this folder: they stay where they are and move up into {up}.",
        names.join(", "),
        more,
        if inside.len() == 1 { "is" } else { "are" }
    ))
}

/// A track's name by its id, for what is said of it.
fn name_of(ctx: &Ctx, id: &str) -> String {
    match ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(id.to_string())) {
        Ok(track) => super::track_name(ctx.reaper, track),
        Err(_) => "a track".to_string(),
    }
}

/// A track's id: the master by name, any other by GUID.
pub(super) fn track_id_of(ctx: &Ctx, track: Track) -> TrackId {
    rows::track_id(ctx.reaper, track)
}

/// The most of a deleted track's state Kumi keeps to put it back: the chunk goes in a frame and sits in the host's journal.
const MOST_MATERIAL: usize = 24 << 20;

/// Every track's folder depth, by id, in order.
pub(super) fn folder_depths(ctx: &Ctx) -> Vec<(String, i64)> {
    (0..ctx.reaper.track_count(ctx.proj))
        .filter_map(|at| ctx.reaper.track_at(ctx.proj, at))
        .map(|track| (ctx.reaper.track_guid(track), ctx.reaper.track_value(track, "I_FOLDERDEPTH") as i64))
        .collect()
}

/// Sets each track's folder depth back to what it was (`before`, from `folder_depths`), for a change that takes away again
/// the tracks it made inside the call that made them: REAPER's deletes (and moves) mend some folders and not others.
pub(super) fn put_folder_depths_back(ctx: &Ctx, before: &[(String, i64)]) {
    let was: std::collections::HashMap<&str, i64> = before.iter().map(|(id, depth)| (id.as_str(), *depth)).collect();
    for track in (0..ctx.reaper.track_count(ctx.proj)).filter_map(|at| ctx.reaper.track_at(ctx.proj, at)) {
        let Some(depth) = was.get(ctx.reaper.track_guid(track).as_str()) else { continue };
        if ctx.reaper.track_value(track, "I_FOLDERDEPTH") as i64 != *depth {
            ctx.reaper.set_track_value(track, "I_FOLDERDEPTH", *depth as f64);
        }
    }
}

/// Each track's folder parent, by id.
fn parents_by_id(ids: &[String], depths: &[i64]) -> std::collections::HashMap<String, Option<String>> {
    let layout = folders::layout(depths);
    ids.iter().enumerate().map(|(place, id)| (id.clone(), layout.parent[place].map(|parent| ids[parent].clone()))).collect()
}

/// Deletes tracks Kumi made, for an undo that takes them away again, keeping every other track in the folder it is in.
/// REAPER's own delete mends some folders and not others (7.82 lab: a track that closes a folder takes its closing with it,
/// so the tracks after it fall into that folder), so the depths are worked out before, as a track delete's are, and set
/// after. The tracks of a folder that goes move up into the folder around it, and only those in `may_move_up` may (an
/// added folder's own run; `None`, any, when Kumi is told to undo anyway): when another track would move out of its
/// folder, nothing is deleted and the answer is `changed`.
pub(super) fn delete_keeping_folders(ctx: &Ctx, gone: &[Track], may_move_up: Option<&[&str]>) -> Done<()> {
    let reaper = ctx.reaper;
    let before = folder_depths(ctx);
    let mut ids: Vec<String> = before.iter().map(|(id, _)| id.clone()).collect();
    let mut depths: Vec<i64> = before.iter().map(|(_, depth)| *depth).collect();
    let parents_before = parents_by_id(&ids, &depths);
    let mut places = Vec::with_capacity(gone.len());
    for track in gone {
        let id = reaper.track_guid(*track);
        let place =
            ids.iter().position(|other| *other == id).ok_or_else(|| failed("a track to delete isn't in the project's track list"))?;
        places.push((place, *track));
    }
    places.sort_by_key(|(place, _)| std::cmp::Reverse(*place));
    places.dedup_by_key(|(place, _)| *place);
    // Last first: every place before it is still the one it was.
    for (place, _) in &places {
        depths = folders::after_delete(&depths, *place);
        ids.remove(*place);
    }
    let parents_after = parents_by_id(&ids, &depths);
    let may_move = |id: &str| may_move_up.is_none_or(|ids| ids.contains(&id));
    if let Some(moved) = ids.iter().find(|id| parents_before[*id] != parents_after[*id] && !may_move(id)) {
        let name = super::quoted(&name_of(ctx, moved));
        return Err((
            "changed".into(),
            format!("taking it away would move {name} out of its folder: {name} went into a folder of Kumi's in REAPER since, so Kumi leaves both"),
        ));
    }
    for (n, (_, track)) in places.iter().enumerate() {
        if !reaper.delete_track(*track) {
            return Err(if n == 0 {
                failed("REAPER wouldn't delete the track")
            } else {
                ("uncertain".into(), "REAPER wouldn't delete one of the tracks: read the tracks".into())
            });
        }
    }
    // What REAPER left of the folders is made what they should be.
    let left = folder_depths(ctx);
    if left.len() != ids.len() || left.iter().zip(&ids).any(|((now, _), id)| now != id) {
        return Err(("uncertain".into(), "REAPER deleted the tracks, but the track list isn't what Kumi expected: read the tracks".into()));
    }
    for (place, ((_, now), want)) in left.iter().zip(&depths).enumerate() {
        if now != want {
            if let Some(other) = reaper.track_at(ctx.proj, place as i32) {
                reaper.set_track_value(other, "I_FOLDERDEPTH", *want as f64);
            }
        }
    }
    if folder_depths(ctx).iter().zip(&depths).any(|((_, now), want)| now != want) {
        return Err(("uncertain".into(), "Kumi deleted the tracks but REAPER kept some folders as they were: read the tracks".into()));
    }
    Ok(())
}

/// A send or receive of the track, with everything that makes it the same one again.
fn route(ctx: &Ctx, track: Track, category: i32, index: i32) -> Option<Value> {
    let other = ctx.reaper.send_peer(track, category, index)?;
    let value = |parm: &str| ctx.reaper.send_value(track, category, index, parm);
    // A send's envelopes (volume, pan, mute) are the sender's, and go with the send; making them again is M4's.
    let envelopes = if category == 0 {
        ["<VOLENV", "<PANENV", "<MUTEENV"]
            .into_iter()
            .filter_map(|chunk| ctx.reaper.send_envelope(track, index, chunk))
            .filter(|envelope| ctx.reaper.envelope_flag(*envelope, "ACTIVE"))
            .count()
    } else {
        0
    };
    Some(json!({
        "category": category,
        "envelopes": envelopes,
        "other": ctx.reaper.track_guid(other),
        "stamp": ctx.reaper.send_text(track, category, index, super::sends::STAMP),
        "volume": value("D_VOL"),
        "pan": value("D_PAN"),
        "mute": value("B_MUTE"),
        "mode": value("I_SENDMODE"),
        "srcChan": value("I_SRCCHAN"),
        "dstChan": value("I_DSTCHAN"),
        "midiFlags": value("I_MIDIFLAGS"),
        "phase": value("B_PHASE"),
        "mono": value("B_MONO"),
        "autoMode": value("I_AUTOMODE"),
        // (r-tracks, N10) Its pan law, and its slot in REAPER's send list (the UI's own order, -1 unhinted): REAPER puts a
        // route made again in its place by the tracks' order itself (7.82 lab), and the hint keeps a producer's own order.
        "panLaw": value("D_PANLAW"),
        "slotHint": value("I_SLOT_HINT"),
    }))
}

pub fn delete_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_to_delete(ctx, input).map_err(gone_is_changed)?;
    let now = signature(ctx.reaper, found.track);
    if &now != fence {
        return Err(("changed".into(), format!("{} changed since the preview", named(ctx, &found))));
    }
    let reaper = ctx.reaper;
    let chunk = reaper
        .track_chunk(found.track)
        .ok_or_else(|| failed("REAPER wouldn't give the track's state, so Kumi won't delete what it could not put back"))?;
    let guid = reaper.track_guid(found.track);
    let index = (reaper.track_value(found.track, "IP_TRACKNUMBER") as i64 - 1).max(0);
    // Too much to keep: the chunk travels in a frame and sits in the journal.
    if chunk.len() > MOST_MATERIAL {
        return Err(failed(format!(
            "this track's state is {} MB, more than the {} MB Kumi can keep of a deleted track: Kumi won't delete what it can't put back",
            chunk.len() >> 20,
            MOST_MATERIAL >> 20
        )));
    }
    // Every track stays in the folder it is in, but for the tracks of a folder that goes, which move up: the depths that make
    // it so, worked out before REAPER deletes (it mends some folders and not others).
    let depths_before = folder_depths(ctx);
    let at = depths_before.iter().position(|(id, _)| *id == guid).ok_or_else(|| failed("the track isn't in the project's track list"))?;
    let all: Vec<i64> = depths_before.iter().map(|(_, depth)| *depth).collect();
    let layout = folders::layout(&all);
    let wanted = folders::after_delete(&all, at);
    let parent = layout.parent[at].map(|parent| depths_before[parent].0.clone());
    let children: Vec<String> =
        (0..all.len()).filter(|other| layout.parent[*other] == Some(at)).map(|other| depths_before[other].0.clone()).collect();
    // Its neighbours, by id: the undo puts it back beside them, wherever the producer has moved them since.
    let above = at.checked_sub(1).map(|place| depths_before[place].0.clone());
    let below = depths_before.get(at + 1).map(|(id, _)| id.clone());
    let routes: Vec<Value> = [0, -1]
        .into_iter()
        .flat_map(|category| (0..reaper.send_count(found.track, category)).filter_map(move |at| route(ctx, found.track, category, at)))
        .collect();
    let selected = reaper.track_value(found.track, "I_SELECTED") != 0.0;
    let lost_envelopes = routes.iter().filter_map(|route| route["envelopes"].as_u64()).sum::<u64>() + without_receives(&chunk).1 as u64;
    let name = named(ctx, &found);
    // (what the delete does to folders is said with the change too, not only before it)
    let folders = folder_note(ctx, &found);
    if !reaper.delete_track(found.track) {
        return Err(failed("REAPER wouldn't delete the track"));
    }
    // What REAPER left of the folders is made what they should be.
    let left = folder_depths(ctx);
    if left.len() == wanted.len() {
        for (place, ((_, now), want)) in left.iter().zip(&wanted).enumerate() {
            if now != want {
                if let Some(other) = reaper.track_at(ctx.proj, place as i32) {
                    reaper.set_track_value(other, "I_FOLDERDEPTH", *want as f64);
                }
            }
        }
    }
    // Exactly the folder depths the deletion changed, by id, with what each was and became: an undo puts back only those
    // that are still as the deletion left them.
    let depths_after: std::collections::HashMap<String, i64> = folder_depths(ctx).into_iter().collect();
    let depths: Vec<Value> = depths_before
        .iter()
        .filter(|(track, _)| *track != guid)
        .filter_map(|(track, before)| {
            depths_after.get(track).filter(|after| *after != before).map(|after| json!({"guid": track, "before": before, "after": after}))
        })
        .collect();
    let mended = folder_depths(ctx);
    if mended.len() != wanted.len() || mended.iter().zip(&wanted).any(|((_, now), want)| now != want) {
        return Err(("uncertain".into(), format!("Kumi deleted {name} but REAPER kept some folders as they were: read the tracks")));
    }
    let material = json!({
        "guid": guid, "index": index, "name": now["name"], "chunk": chunk,
        "depths": depths, "parent": parent, "children": children, "above": above, "below": below, "routes": routes, "selected": selected,
        "folderDepth": now["folderDepth"], "lostEnvelopes": lost_envelopes,
    });
    let mut display = json!({"title": format!("Deleted track {}", super::quoted(&name))});
    if let Some(note) = folders {
        display["note"] = note.into();
    }
    Ok(Applied { applied: json!({"deleted": true}), created: None, material: Some(material), display, ..Default::default() })
}

/// A chunk's lines without the sends the track receives: those name their sources by position, which has moved on, and
/// are made again from the routes by GUID. Nothing else of the chunk is touched.
pub fn without_receives(chunk: &str) -> (String, usize) {
    let mut kept = String::with_capacity(chunk.len());
    let mut dropped = 0usize;
    let mut depth = 0i32;
    let mut skipping: Option<i32> = None;
    for line in chunk.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let opens = trimmed.starts_with('<');
        let closes = trimmed.trim_end() == ">";
        if let Some(level) = skipping {
            if opens {
                depth += 1;
            } else if closes {
                depth -= 1;
                if depth < level {
                    skipping = None;
                }
            }
            continue;
        }
        if depth == 1 && trimmed.starts_with("AUXRECV ") {
            continue;
        }
        if depth == 1 && opens && ["<AUXVOLENV", "<AUXPANENV", "<AUXMUTEENV"].iter().any(|name| trimmed.starts_with(name)) {
            skipping = Some(depth + 1);
            depth += 1;
            dropped += 1;
            continue;
        }
        if opens {
            depth += 1;
        } else if closes {
            depth -= 1;
        }
        kept.push_str(line);
    }
    (kept, dropped)
}

/// Where a deleted track goes back: after the track that was above it, else before the track that was below it, wherever the
/// producer has moved them since; at its old place only when neither is in the project any more.
fn back_at(ctx: &Ctx, material: &Value) -> i32 {
    let place = |key: &str| {
        let id = material[key].as_str()?;
        let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(id.to_string())).ok()?;
        Some(ctx.reaper.track_value(track, "IP_TRACKNUMBER") as i32 - 1)
    };
    if let Some(above) = place("above") {
        return above + 1;
    }
    if let Some(below) = place("below") {
        return below;
    }
    material["index"].as_i64().unwrap_or(0).max(0) as i32
}

/// Whether putting the track back would change which folder another track is in, as the folders are now: what the producer
/// has done since is theirs, and a track put back in the middle of it would pull tracks in or out of folders. The tracks of a
/// folder that went come back into it. `None` when it would not; else what it would do.
fn would_refold(ctx: &Ctx, material: &Value, guid: &str, index: i32) -> Option<String> {
    let now = folder_depths(ctx);
    let mut then = now.clone();
    // The depths the deletion changed are put back, and the track's own comes with its state.
    for change in material["depths"].as_array().into_iter().flatten() {
        let (Some(other), Some(before)) = (change["guid"].as_str(), change["before"].as_i64()) else { continue };
        if let Some(entry) = then.iter_mut().find(|(id, _)| id == other) {
            entry.1 = before;
        }
    }
    let at = (index.max(0) as usize).min(then.len());
    then.insert(at, (guid.to_string(), material["folderDepth"].as_i64().unwrap_or(0)));
    let parents = |tracks: &[(String, i64)]| {
        let layout = folders::layout(&tracks.iter().map(|(_, depth)| *depth).collect::<Vec<_>>());
        tracks
            .iter()
            .enumerate()
            .map(|(place, (id, _))| (id.clone(), layout.parent[place].map(|parent| tracks[parent].0.clone())))
            .collect::<std::collections::HashMap<String, Option<String>>>()
    };
    let (parents_now, parents_then) = (parents(&now), parents(&then));
    let inside: Vec<&str> = material["children"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut moved: Vec<&String> = parents_now
        .iter()
        .filter(|(id, parent)| {
            let after = &parents_then[*id];
            after != *parent && !(inside.contains(&id.as_str()) && after.as_deref() == Some(guid))
        })
        .map(|(id, _)| id)
        .collect();
    moved.sort();
    if let Some(id) = moved.first() {
        return Some(format!(
            "putting {} back would move {} into or out of a folder: the folders have been rearranged in REAPER since it was deleted",
            super::quoted(material["name"].as_str().unwrap_or("the track")),
            super::quoted(&name_of(ctx, id))
        ));
    }
    // And the folder it was in, if that is still there, is the one it goes back into.
    let was_in = material["parent"].as_str();
    let now_in = parents_then[guid].as_deref();
    if material.get("parent").is_some() && was_in != now_in && was_in.is_none_or(|parent| parents_now.contains_key(parent)) {
        return Some(format!(
            "{} would not go back into the folder it was in: the folders have been rearranged in REAPER since it was deleted",
            super::quoted(material["name"].as_str().unwrap_or("the track")),
        ));
    }
    None
}

pub fn delete_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let guid = material["guid"].as_str().ok_or_else(|| bad("material: the track's id"))?;
    let chunk = material["chunk"].as_str().ok_or_else(|| bad("material: the track's state"))?;
    let reaper = ctx.reaper;
    let id = TrackId::Guid(guid.to_string());
    // A track with that id is there: it came back some other way, or it was never gone.
    if ctx.reads.track_by_id(reaper, ctx.proj, &id).is_ok() {
        return if restore.discard {
            Ok(Restored {
                restored: json!({"ref": refs::track_ref(&id)}),
                display: json!({"title": "The track is there already"}),
                unwritten: true,
            })
        } else {
            Err(("changed".into(), "a track with that id is in the project again".into()))
        };
    }
    // The folders around it as the deletion left them: else the producer has rearranged them since, and putting the old
    // depths back would break their folders.
    if !restore.discard {
        for change in material["depths"].as_array().into_iter().flatten() {
            let Some(other) = change["guid"].as_str() else { continue };
            let Ok(other) = ctx.reads.track_by_id(reaper, ctx.proj, &TrackId::Guid(other.to_string())) else { continue };
            if reaper.track_value(other, "I_FOLDERDEPTH") as i64 != change["after"].as_i64().unwrap_or(0) {
                return Err(("changed".into(), "the folders around the deleted track have been rearranged in REAPER since".into()));
            }
        }
    }
    let index = back_at(ctx, material);
    if !restore.discard {
        if let Some(reason) = would_refold(ctx, material, guid, index) {
            return Err(("changed".into(), reason));
        }
    }
    let track = reaper.insert_track(ctx.proj, index).ok_or_else(|| failed("REAPER wouldn't make a track to put it back in"))?;
    if !reaper.set_track_chunk(track, &without_receives(chunk).0) || reaper.track_guid(track) != guid {
        // Not what was deleted: take away what was made, so nothing half-done stays.
        reaper.delete_track(track);
        return Err(failed("REAPER wouldn't put the track's state back"));
    }
    for change in material["depths"].as_array().into_iter().flatten() {
        let Some(other) = change["guid"].as_str() else { continue };
        if let Ok(other) = ctx.reads.track_by_id(reaper, ctx.proj, &TrackId::Guid(other.to_string())) {
            reaper.set_track_value(other, "I_FOLDERDEPTH", change["before"].as_f64().unwrap_or(0.0));
        }
    }
    let mut remade = 0usize;
    for route in material["routes"].as_array().into_iter().flatten() {
        let Some(other) = route["other"].as_str() else { continue };
        let Ok(other) = ctx.reads.track_by_id(reaper, ctx.proj, &TrackId::Guid(other.to_string())) else { continue };
        let (from, to, category) = if route["category"] == -1 { (other, track, 0) } else { (track, other, 0) };
        let Some(at) = reaper.create_send(from, Some(to)) else { continue };
        // A send of Kumi's comes back with its mark.
        if let Some(stamp) = route["stamp"].as_str() {
            reaper.set_send_text(from, 0, at, super::sends::STAMP, stamp);
        }
        let set = |parm: &str, key: &str| reaper.set_send_value(from, category, at, parm, route[key].as_f64().unwrap_or(0.0));
        for (parm, key) in [
            ("D_VOL", "volume"),
            ("D_PAN", "pan"),
            ("B_MUTE", "mute"),
            ("I_SENDMODE", "mode"),
            ("I_SRCCHAN", "srcChan"),
            ("I_DSTCHAN", "dstChan"),
            ("I_MIDIFLAGS", "midiFlags"),
            ("B_PHASE", "phase"),
            ("B_MONO", "mono"),
            ("I_AUTOMODE", "autoMode"),
        ] {
            set(parm, key);
        }
        // (r-tracks, N10) Kept since M4: a delete Kumi made before has none, and its route takes REAPER's defaults.
        for (parm, key) in [("D_PANLAW", "panLaw"), ("I_SLOT_HINT", "slotHint")] {
            if route.get(key).is_some_and(Value::is_number) {
                set(parm, key);
            }
        }
        remade += 1;
    }
    // The producer's selection stands: the track joins it only to be the selection again where nothing else is selected
    // (the track's own state may bring a selection with it, which is not the producer's now).
    let others_selected = reaper.master_track(ctx.proj).is_some_and(|master| reaper.track_value(master, "I_SELECTED") != 0.0)
        || (0..reaper.track_count(ctx.proj))
            .filter_map(|at| reaper.track_at(ctx.proj, at))
            .any(|other| other != track && reaper.track_value(other, "I_SELECTED") != 0.0);
    let select = material["selected"] == true && !others_selected;
    reaper.set_track_value(track, "I_SELECTED", if select { 1.0 } else { 0.0 });
    let name = super::track_name(reaper, track);
    let mut restored = Map::new();
    restored.insert("ref".into(), refs::track_ref(&id).into());
    restored.insert("routes".into(), remade.into());
    let mut display = json!({"title": format!("{} put back", super::quoted(&name))});
    if let Some(lost) = material["lostEnvelopes"].as_u64().filter(|lost| *lost > 0) {
        display["note"] = format!("Its {lost} send envelope(s) were not put back.").into();
    }
    Ok(Restored { restored: Value::Object(restored), display, ..Default::default() })
}
