//! A track template (`.RTrackTemplate`) inserted as tracks (r-tracks, M4): by its name in REAPER's TrackTemplates folder
//! or its path, at a place. REAPER inserts a template's tracks at the end of the track list (whatever is selected),
//! selected, with new GUIDs and their folders (7.82 lab); Kumi then moves them where they were asked to go and puts the
//! producer's selection back. The tracks made are the change's (`created`, by GUID); its undo deletes them, only while
//! each is as the insert left it (its items and their MIDI, automation, mix, routing, plug-ins and their parameters) and,
//! whatever it is told, no track of the producer's has gone into their folders.
//!
//! A template file that isn't there would make REAPER show an error dialog: it is checked first.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Fx, FxOwner, Reaper, Track};
use crate::refs::{self, TrackId};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

/// The most of a template Kumi reads to say what it holds.
const MOST_TEMPLATE: u64 = 64 << 20;

fn template_path(ctx: &Ctx, input: &Value) -> Done<PathBuf> {
    let path = match (input.get("name").and_then(Value::as_str), input.get("path").and_then(Value::as_str)) {
        (Some(name), None) => {
            let name = name.trim().trim_end_matches(".RTrackTemplate");
            let relative = Path::new(name);
            if name.is_empty() || relative.is_absolute() || relative.components().any(|c| !matches!(c, Component::Normal(_))) {
                return Err(bad(
                    "name must be a template's name in REAPER's TrackTemplates folder (folders inside allowed, \"Drums/Kit\")",
                ));
            }
            // (a folder at a time, so the path has the system's separators throughout)
            Path::new(&ctx.reaper.resource_path())
                .join("TrackTemplates")
                .join(Path::new(&format!("{name}.RTrackTemplate")).components().collect::<PathBuf>())
        }
        (None, Some(path)) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
                return Err(bad("path must be a whole path, with no \"..\" in it"));
            }
            path
        }
        _ => return Err(bad("give name (a template in REAPER's TrackTemplates folder) or path, not both")),
    };
    if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("RTrackTemplate")) {
        return Err(bad("a track template is a .RTrackTemplate file"));
    }
    let meta = std::fs::metadata(&path).map_err(|_| failed(format!("there is no track template at {}", path.display())))?;
    if !meta.is_file() || meta.len() > MOST_TEMPLATE {
        return Err(failed(format!("{} isn't a track template Kumi can insert", path.display())));
    }
    Ok(path)
}

/// What a template holds: its tracks' names, and the hash of its text (what the preview saw).
struct Template {
    names: Vec<String>,
    hash: String,
}

fn read_template(path: &Path) -> Done<Template> {
    let text = std::fs::read_to_string(path).map_err(|e| failed(format!("Kumi couldn't read {}: {e}", path.display())))?;
    let mut names = Vec::new();
    let mut depth = 0i32;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('<') {
            depth += 1;
            if depth == 1 && trimmed.starts_with("<TRACK") {
                names.push(String::new());
            }
            continue;
        }
        if trimmed == ">" {
            depth -= 1;
            continue;
        }
        if depth == 1 {
            if let (Some(rest), Some(last)) = (trimmed.strip_prefix("NAME "), names.last_mut()) {
                *last = rest.trim().trim_matches('"').to_string();
            }
        }
    }
    if names.is_empty() {
        return Err(failed(format!("{} holds no tracks", path.display())));
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(Template { names, hash: format!("{hash:016x}") })
}

/// Where the tracks go (0-based), from `index` counted from 1 as the track rows count; the end by default.
fn place_of(ctx: &Ctx, input: &Value) -> Done<i64> {
    let count = ctx.reaper.track_count(ctx.proj) as i64;
    match input.get("index") {
        None | Some(Value::Null) => Ok(count),
        Some(v) => v.as_i64().filter(|i| (1..=count + 1).contains(i)).map(|i| i - 1).ok_or_else(|| {
            bad(format!("index must be the first track's place, from 1 to {} (counted from 1, as a read shows)", count + 1))
        }),
    }
}

/// What stands where the tracks go: the track count, and the track at that place now.
fn fence_of(ctx: &Ctx, at: i64, template: &Template) -> Value {
    let reaper = ctx.reaper;
    json!({
        "tracks": reaper.track_count(ctx.proj),
        "at": reaper.track_at(ctx.proj, at as i32).map(|t| reaper.track_guid(t)),
        "template": template.hash,
    })
}

fn label(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn title(path: &Path, count: usize, at: i64) -> String {
    format!(
        "Inserted track template {} ({count} track{}) at position {}",
        super::quoted(&label(path)),
        if count == 1 { "" } else { "s" },
        at + 1
    )
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let path = template_path(ctx, input)?;
    let template = read_template(&path)?;
    let at = place_of(ctx, input)?;
    let names: Vec<String> = template.names.iter().map(|n| if n.is_empty() { "(unnamed)".to_string() } else { n.clone() }).collect();
    Ok(Preview {
        target: json!({"ref": null, "kind": "track", "index": at, "template": path.to_string_lossy()}),
        prior: json!({"tracks": ctx.reaper.track_count(ctx.proj)}),
        fence: fence_of(ctx, at, &template),
        display: json!({"title": title(&path, template.names.len(), at), "note": format!("Its tracks: {}.", names.join(", "))}),
    })
}

/// The first words of a track's state lines that only show or select it: selection, heights, window places, the MIDI
/// editor's view, an envelope's lane (7.82 lab: these move when the producer only looks or selects).
const VIEW_WORDS: [&str; 20] = [
    "SEL",
    "TRACKHEIGHT",
    "VU",
    "SHOWINMIX",
    "BUSCOMP",
    "IID",
    "SHOW",
    "LASTSEL",
    "DOCKED",
    "FLOATPOS",
    "FLOAT",
    "WNDRECT",
    "CFGEDITVIEW",
    "CFGEDIT",
    "EVTFILTER",
    "KEYSNAP",
    "TRACKSEL",
    "VELLANE",
    "VIS",
    "LANEHEIGHT",
];

/// A track's lane-recording settings as REAPER first writes them.
const DEFAULT_LANEREC: &str = "LANEREC -1 -1 -1 0";

/// A plug-in's block in an FX chain: what it holds is the plug-in's own data, its parameters among it; those are its FX
/// signature's (less what moves by itself), so the block's lines aren't hashed.
const PLUGIN_WORDS: [&str; 7] = ["<VST", "<AU", "<JS", "<CLAP", "<LV2", "<DX", "<VIDEO_EFFECT"];

/// A hash of a track's state as the producer can change it: its state chunk less the lines that only show or select
/// (`VIEW_WORDS`; a MIDI event or envelope point told the same selected or not), less its receives (REAPER writes them by
/// the sending track's number, which moves when tracks do: they are hashed by id apart), less its plug-ins' data (but not
/// its input FX's, which have no signature here).
pub(super) fn content_hash(chunk: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |text: &str| {
        for byte in text.bytes().chain([b'\n']) {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let (mut depth, mut plugin, mut input_chain) = (0i32, None::<i32>, None::<i32>);
    let lines: Vec<&str> = chunk.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let mut skip_to = 0;
    for (at, &line) in lines.iter().enumerate() {
        if at < skip_to {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or_default();
        // REAPER writes a track anew after its own undo or redo: a track the API made then gains its lane-recording
        // defaults and an FX chain block with nothing in it (7.82 lab, 2026-10-19). Neither is the producer's.
        if line == DEFAULT_LANEREC {
            continue;
        }
        if first == "<FXCHAIN" {
            let end = lines[at + 1..].iter().position(|l| *l == ">" || l.starts_with('<')).map(|n| at + 1 + n);
            if let Some(end) = end.filter(|end| lines[*end] == ">") {
                if lines[at + 1..end].iter().all(|l| VIEW_WORDS.contains(&l.split_whitespace().next().unwrap_or_default())) {
                    skip_to = end + 1;
                    continue;
                }
            }
        }
        if line == ">" {
            if plugin == Some(depth) {
                plugin = None;
            } else if plugin.is_none() {
                feed(line);
            }
            if input_chain == Some(depth) {
                input_chain = None;
            }
            depth -= 1;
            continue;
        }
        if plugin.is_some() {
            depth += i32::from(line.starts_with('<'));
            continue;
        }
        if line.starts_with('<') {
            depth += 1;
            if first == "<FXCHAIN_REC" {
                input_chain = Some(depth);
            } else if input_chain.is_none() && PLUGIN_WORDS.contains(&first) {
                plugin = Some(depth);
            }
            feed(&unselected(line));
            continue;
        }
        if VIEW_WORDS.contains(&first) || first == "AUXRECV" {
            continue;
        }
        feed(&unselected(line));
    }
    format!("{hash:016x}")
}

/// A line as it reads unselected: a selected MIDI event is written `e`/`x` (`E`/`X` not), a selected envelope point has a
/// fifth number 1 (`PT 2 0.5 0 0 1`; one at no tension and not selected has three).
fn unselected(line: &str) -> String {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    let Some(first) = words.first().copied() else { return String::new() };
    let upper: String;
    match first {
        "e" | "em" | "x" | "xm" | "<x" | "<xm" => {
            upper = first.replacen('e', "E", 1).replacen('x', "X", 1);
            words[0] = upper.as_str();
            words.join(" ")
        }
        "PT" => {
            if words.len() == 4 {
                words.push("0");
            } else if words.len() >= 6 {
                words.remove(5);
            }
            words.join(" ")
        }
        _ => words.join(" "),
    }
}

/// A track's sends and receives, by the track at the other end: their settings, and the points of a send's envelopes (REAPER
/// keeps those on the receiving track).
fn routes_hash(reaper: &Reaper, track: Track) -> String {
    let mut text = String::new();
    for category in [0, -1] {
        for index in 0..reaper.send_count(track, category) {
            let peer = reaper.send_peer(track, category, index).map(|t| reaper.track_guid(t)).unwrap_or_default();
            text += &format!("{category} {peer}");
            for parm in crate::rows::SEND_STATE {
                text += &format!(" {}", reaper.send_value(track, category, index, parm));
            }
            if category == 0 {
                for chunk in ["<VOLENV", "<PANENV", "<MUTEENV"] {
                    let points = reaper.send_envelope(track, index, chunk).map(|e| reaper.envelope_point_count(e));
                    text += &format!(" {chunk}:{points:?}");
                }
            }
            text.push('\n');
        }
    }
    content_hash(&text)
}

/// The track's FX and its takes' FX, in order, each container followed by the FX inside it (at any depth).
fn all_fx(reaper: &Reaper, track: Track) -> Vec<Fx> {
    fn with_inside(reaper: &Reaper, fx: Fx, all: &mut Vec<Fx>) {
        all.push(fx);
        for child in reaper.fx_children(fx) {
            with_inside(reaper, child, all);
        }
    }
    let mut owners = vec![FxOwner::Track(track)];
    for item in (0..reaper.track_item_count(track)).filter_map(|n| reaper.track_item(track, n)) {
        owners.extend((0..reaper.take_count(item)).filter_map(|n| reaper.take_at(item, n)).map(FxOwner::Take));
    }
    let mut all = Vec::new();
    for owner in owners {
        for index in 0..reaper.fx_count(owner) {
            with_inside(reaper, Fx { owner, index }, &mut all);
        }
    }
    all
}

/// What tells a track made from the template from how the insert left it: its state as the producer can change it (items
/// and their MIDI, envelopes and their points, mix, routing, plug-ins in and out), its sends and receives, and each FX's
/// signature (its parameters, less those that move by themselves).
pub(super) fn signature(ctx: &Ctx, track: Track) -> Value {
    let reaper = ctx.reaper;
    json!({
        "state": reaper.track_chunk(track).map(|chunk| content_hash(&chunk)),
        "routes": routes_hash(reaper, track),
        "fx": all_fx(reaper, track).into_iter().map(|fx| super::fx::signature(ctx, fx)).collect::<Vec<_>>(),
    })
}

/// Whether the track is as the insert left it, by its recorded signature.
pub(super) fn as_left(ctx: &Ctx, track: Track, recorded: &Value) -> bool {
    let reaper = ctx.reaper;
    let fx = all_fx(reaper, track);
    let signatures = recorded["fx"].as_array().map(Vec::as_slice).unwrap_or_default();
    recorded["state"].is_string()
        && reaper.track_chunk(track).map(|chunk| content_hash(&chunk)).as_deref() == recorded["state"].as_str()
        && recorded["routes"].as_str() == Some(routes_hash(reaper, track).as_str())
        && fx.len() == signatures.len()
        && fx.iter().zip(signatures).all(|(fx, recorded)| super::fx::signature_as(ctx, *fx, recorded) == *recorded)
}

fn selection(ctx: &Ctx) -> (Vec<String>, bool) {
    let reaper = ctx.reaper;
    let tracks = (0..reaper.track_count(ctx.proj))
        .filter_map(|at| reaper.track_at(ctx.proj, at))
        .filter(|t| reaper.track_value(*t, "I_SELECTED") != 0.0)
        .map(|t| reaper.track_guid(t))
        .collect();
    (tracks, reaper.master_track(ctx.proj).is_some_and(|m| reaper.track_value(m, "I_SELECTED") != 0.0))
}

/// REAPER puts a template's tracks after the last touched track, not at the end (lab 7.82): the last track is made the last
/// touched one first, so they come at the end; the producer's own (its GUID) is put back by `put_touched`.
const SET_LAST_TOUCHED: i32 = 40914;

fn touch_last(ctx: &Ctx) -> Option<String> {
    let reaper = ctx.reaper;
    let touched = reaper.last_touched_track().map(|t| reaper.track_guid(t));
    if let Some(last) = reaper.track_at(ctx.proj, reaper.track_count(ctx.proj) - 1) {
        select_only(ctx, Some(last));
        reaper.run_action(SET_LAST_TOUCHED);
    }
    touched
}

fn put_touched(ctx: &Ctx, touched: &Option<String>) {
    if let Some(track) = touched.as_deref().and_then(|guid| by_guid(ctx, guid)) {
        select_only(ctx, Some(track));
        ctx.reaper.run_action(SET_LAST_TOUCHED);
    }
}

fn select_only(ctx: &Ctx, only: Option<Track>) {
    let reaper = ctx.reaper;
    for at in 0..reaper.track_count(ctx.proj) {
        if let Some(t) = reaper.track_at(ctx.proj, at) {
            reaper.set_track_selected(t, Some(t) == only);
        }
    }
}

fn put_selection(ctx: &Ctx, (tracks, master): &(Vec<String>, bool)) {
    let reaper = ctx.reaper;
    for at in 0..reaper.track_count(ctx.proj) {
        if let Some(t) = reaper.track_at(ctx.proj, at) {
            reaper.set_track_selected(t, tracks.contains(&reaper.track_guid(t)));
        }
    }
    if let Some(m) = reaper.master_track(ctx.proj) {
        reaper.set_track_selected(m, *master);
    }
}

fn guids(ctx: &Ctx) -> Vec<String> {
    (0..ctx.reaper.track_count(ctx.proj)).filter_map(|at| ctx.reaper.track_at(ctx.proj, at)).map(|t| ctx.reaper.track_guid(t)).collect()
}

fn by_guid(ctx: &Ctx, guid: &str) -> Option<Track> {
    ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string())).ok()
}

/// Deletes these tracks, last first (what was made, taken away again inside the call that made it), and puts the other
/// tracks' folders back as they were (`depths`).
fn delete_all(ctx: &Ctx, made: &[String], depths: &[(String, i64)]) {
    for guid in made.iter().rev() {
        if let Some(track) = by_guid(ctx, guid) {
            ctx.reaper.delete_track(track);
        }
    }
    super::tracks::put_folder_depths_back(ctx, depths);
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let path = template_path(ctx, input).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    let template = read_template(&path)?;
    let at = place_of(ctx, input)?;
    if &fence_of(ctx, at, &template) != fence {
        return Err(changed("the tracks, or the template file, changed since the preview"));
    }
    let reaper = ctx.reaper;
    let kept = selection(ctx);
    let before = guids(ctx);
    let depths = super::tracks::folder_depths(ctx);
    let touched = touch_last(ctx);
    let opened = reaper.open_project(&path.to_string_lossy());
    put_touched(ctx, &touched);
    if !opened {
        put_selection(ctx, &kept);
        return Err(failed("this REAPER can't insert track templates"));
    }
    // The tracks REAPER made: those it didn't have, in its order (at the end).
    let made: Vec<String> = guids(ctx).into_iter().filter(|guid| !before.contains(guid)).collect();
    if made.is_empty() {
        put_selection(ctx, &kept);
        return Err(failed("REAPER made no tracks from the template"));
    }
    // A template whose tracks open a folder they don't close would pull the tracks after them into it: those go at the end.
    let net: i64 = made.iter().filter_map(|guid| by_guid(ctx, guid)).map(|t| reaper.track_value(t, "I_FOLDERDEPTH") as i64).sum();
    let count = reaper.track_count(ctx.proj) as i64;
    let first_at = count - made.len() as i64;
    if at < first_at {
        if net != 0 {
            delete_all(ctx, &made, &depths);
            put_selection(ctx, &kept);
            return Err(bad("the template's tracks open a folder they don't close: insert it at the end (no index)"));
        }
        // Moved as one block, just above the track at the asked place.
        for t in 0..reaper.track_count(ctx.proj) {
            if let Some(track) = reaper.track_at(ctx.proj, t) {
                reaper.set_track_selected(track, made.contains(&reaper.track_guid(track)));
            }
        }
        if !reaper.reorder_selected(at as i32, 0) {
            delete_all(ctx, &made, &depths);
            put_selection(ctx, &kept);
            return Err(failed("REAPER wouldn't move the template's tracks to their place: they are taken away again"));
        }
    }
    put_selection(ctx, &kept);
    let placed: Vec<i64> =
        made.iter().filter_map(|guid| by_guid(ctx, guid)).map(|t| reaper.track_value(t, "IP_TRACKNUMBER") as i64 - 1).collect();
    let in_place = placed.len() == made.len() && placed.iter().enumerate().all(|(n, p)| *p == at + n as i64);
    if !in_place {
        delete_all(ctx, &made, &depths);
        return Err(failed("REAPER put the template's tracks elsewhere than asked: they are taken away again"));
    }
    let tracks: Vec<Value> =
        made.iter().filter_map(|guid| by_guid(ctx, guid).map(|t| json!({"guid": guid, "signature": signature(ctx, t)}))).collect();
    let track_refs: Vec<String> = made.iter().map(|guid| refs::track_ref(&TrackId::Guid(guid.clone()))).collect();
    Ok(Applied {
        applied: json!({"index": at, "tracks": tracks}),
        created: Some(json!({"ref": track_refs[0], "kind": "track", "trackRefs": track_refs})),
        material: None,
        display: json!({"title": title(&path, made.len(), at)}),
        ..Default::default()
    })
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let reaper = ctx.reaper;
    let tracks = restore.applied["tracks"].as_array().ok_or_else(|| bad("applied: the tracks made"))?;
    let made: Vec<String> = tracks.iter().filter_map(|t| t["guid"].as_str().map(str::to_string)).collect();
    let present: Vec<(String, Track)> = made.iter().filter_map(|guid| by_guid(ctx, guid).map(|t| (guid.clone(), t))).collect();
    if present.is_empty() {
        return Err(("gone".into(), "the tracks the template made aren't in the project any more".into()));
    }
    if !restore.discard {
        if present.len() != made.len() {
            return Err(changed("some of the tracks the template made were deleted in REAPER since: the others stay"));
        }
        for entry in tracks {
            let Some((_, track)) = present.iter().find(|(guid, _)| entry["guid"].as_str() == Some(guid.as_str())) else { continue };
            if !as_left(ctx, *track, &entry["signature"]) {
                return Err(changed(format!(
                    "{} has been changed in REAPER since the template made it (its items, automation, mix, routing or plug-ins)",
                    super::quoted(&super::track_name(reaper, *track))
                )));
            }
        }
        // Still one block.
        let places: Vec<i64> = present.iter().map(|(_, t)| reaper.track_value(*t, "IP_TRACKNUMBER") as i64 - 1).collect();
        if places.windows(2).any(|w| w[1] != w[0] + 1) {
            return Err(changed("the template's tracks aren't together any more: other tracks were moved among them"));
        }
    }
    // No track of the producer's inside its folders, even told to undo anyway: deleting the folder would move that track
    // into another folder (or out of all), and it isn't the template's.
    let inside = (0..reaper.track_count(ctx.proj)).filter_map(|at| reaper.track_at(ctx.proj, at)).any(|other| {
        !present.iter().any(|(_, t)| *t == other)
            && reaper.track_parent(other).is_some_and(|parent| present.iter().any(|(_, t)| *t == parent))
    });
    if inside {
        return Err(changed("a track of the producer's is inside the template's folder now: Kumi won't delete the folder around it"));
    }
    let names: Vec<String> = present.iter().map(|(_, t)| super::track_name(reaper, *t)).collect();
    // Every other track stays in the folder it is in (template tracks that close a folder take that closing with them in a
    // bare delete).
    let tracks_now: Vec<Track> = present.iter().map(|(_, t)| *t).collect();
    super::tracks::delete_keeping_folders(ctx, &tracks_now, Some(&[]))?;
    Ok(Restored {
        restored: json!({"deleted": made.len()}),
        display: json!({"title": format!("Template tracks {} taken away again", names.iter().map(|n| super::quoted(n)).collect::<Vec<_>>().join(", "))}),
        ..Default::default()
    })
}
