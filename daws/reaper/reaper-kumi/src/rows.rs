//! The rows `read`, `snapshot` and the FOCUS bodies answer with: one JSON object per REAPER object, named in camelCase
//! (the shapes are in the M2 contract). Times are seconds, positions in MIDI and sends in quarter notes, gains in dB.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::reaper::{Envelope, Fx, FxOwner, Item, Marker, MidiNote, PlayState, Project, ProjectInfo, Reaper, Take, Track};
use crate::refs::{self, EnvelopeId, SendEnd, TrackId};

/// Longest text a row carries from REAPER (a name, a preset): a pasted essay of a track name mustn't fill an answer.
pub const TEXT_LIMIT: usize = 1024;
/// REAPER's fader floor, which stands for minus infinity.
pub const MIN_DB: f64 = -150.0;

pub fn clip(text: &str) -> String {
    if text.chars().count() <= TEXT_LIMIT {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(TEXT_LIMIT - 1).collect();
    clipped.push('…');
    clipped
}

fn round(value: f64, places: i32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// A time in seconds, to the microsecond.
pub fn seconds(value: f64) -> f64 {
    round(value, 6)
}

/// A gain factor in dB, floored at REAPER's fader bottom.
pub fn db(gain: f64) -> f64 {
    if gain <= 1e-7 {
        MIN_DB
    } else {
        round((20.0 * gain.log10()).max(MIN_DB), 2)
    }
}

pub fn track_id(reaper: &Reaper, track: Track) -> TrackId {
    if reaper.track_value(track, "IP_TRACKNUMBER") < 0.0 {
        TrackId::Master
    } else {
        TrackId::Guid(reaper.track_guid(track))
    }
}

fn color(reaper: &Reaper, native: Option<i32>) -> Value {
    native.and_then(|c| reaper.color_hex(c)).map_or(Value::Null, Value::from)
}

/// A colour field stored as an int in `I_CUSTOMCOLOR`: set only when the 0x1000000 flag is.
fn custom_color(reaper: &Reaper, value: f64) -> Value {
    let native = value as i32;
    color(reaper, (native & 0x100_0000 != 0).then_some(native))
}

pub fn play_state_name(state: PlayState) -> &'static str {
    match (state.recording, state.playing, state.paused) {
        (true, _, true) => "recording-paused",
        (true, _, false) => "recording",
        (false, true, _) => "playing",
        (false, false, true) => "paused",
        _ => "stopped",
    }
}

/// What the transport shows. `beat` counts quarter notes from the project's start, and the loop is in the same beats.
pub fn transport(reaper: &Reaper, proj: Project) -> Value {
    let state = reaper.play_state(proj);
    let now = if state.playing || state.recording { reaper.play_position(proj) } else { reaper.cursor(proj) };
    let (loop_start, loop_end) = reaper.loop_points(proj);
    let signature = reaper.time_signature_at(proj, now);
    let beats = |time: f64| reaper.quarter_notes_at(proj, time).map(seconds);
    let mut transport = json!({
        "playing": state.playing,
        "paused": state.paused,
        "recording": state.recording,
        "state": play_state_name(state),
        "seconds": seconds(now),
        "beat": beats(now),
        "tempo": signature.map(|s| round(s.bpm, 3)).or_else(|| reaper.tempo()),
        "timeSignature": signature.map(|s| json!([s.num, s.denom])),
        "beatsPerBar": signature.map(|s| round(s.num as f64 * 4.0 / s.denom.max(1) as f64, 4)),
        "loop": {"on": reaper.repeat(proj), "start": beats(loop_start), "end": beats(loop_end)},
        "loopSeconds": {"start": seconds(loop_start), "end": seconds(loop_end)},
    });
    if let Some((bar, beat)) = reaper.bar_and_beat_at(proj, now) {
        transport["bar"] = (bar + 1).into();
        transport["beatInBar"] = round(beat + 1.0, 3).into();
    }
    transport
}

/// The time selection, or null when there is none.
pub fn time_selection(reaper: &Reaper, proj: Project) -> Value {
    let (start, end) = reaper.time_selection(proj);
    if end > start {
        json!({"start": seconds(start), "end": seconds(end)})
    } else {
        Value::Null
    }
}

/// The project's row. `identity` is what names this open project while it stays open in this REAPER; `id` is the Kumi
/// project id the project itself carries (see `Reads::project_id`), null before it has one of its own.
pub fn project_row(reaper: &Reaper, proj: Project, info: &ProjectInfo, identity: &str, id: Option<&str>) -> Value {
    let cursor = reaper.cursor(proj);
    let state = reaper.play_state(proj);
    let (loop_start, loop_end) = reaper.loop_points(proj);
    let signature = reaper.time_signature_at(proj, cursor);
    let mut row = json!({
        "id": id,
        "identity": identity,
        "name": clip(&info.name),
        "path": clip(&info.path),
        "saved": !info.path.is_empty(),
        "dirty": reaper.is_dirty(proj),
        "tempo": signature.map(|s| round(s.bpm, 3)).or_else(|| reaper.tempo()),
        "timeSignature": signature.map(|s| json!([s.num, s.denom])),
        "length": seconds(reaper.project_length(proj)),
        "playState": play_state_name(state),
        "cursor": seconds(cursor),
        "loop": {"on": reaper.repeat(proj), "start": seconds(loop_start), "end": seconds(loop_end)},
        "timeSelection": time_selection(reaper, proj),
        "tabCount": reaper.tab_count(),
    });
    add_project_settings(reaper, proj, &mut row);
    row
}

/// (r-tracks) What the project's own settings hold, for the changes that set them: its ruler lanes, its named track
/// groups, the grid and timebase, and its title, author and notes (when it has them; the notes clipped).
fn add_project_settings(reaper: &Reaper, proj: Project, row: &mut Value) {
    const NOTES_SHOWN: usize = 2000;
    let lanes = reaper.project_info(proj, "RULER_LANE_COUNT").unwrap_or(0.0).max(0.0) as i64;
    let lane_rows: Vec<Value> = (0..lanes.min(64))
        .map(|lane| {
            let info = |key: &str| reaper.project_info(proj, &format!("{key}:{lane}")).unwrap_or(0.0);
            let mut lane_row =
                json!({"lane": lane, "name": clip(&reaper.project_text(proj, &format!("RULER_LANE_NAME:{lane}")).unwrap_or_default())});
            match info("RULER_LANE_DEFAULT") as i64 {
                1 => lane_row["defaultFor"] = "regions".into(),
                2 => lane_row["defaultFor"] = "markers".into(),
                3 => lane_row["defaultFor"] = "both".into(),
                _ => {}
            }
            for (key, field) in [("RULER_LANE_HIDDEN", "hidden"), ("RULER_LANE_LOCKED", "locked")] {
                if info(key) != 0.0 {
                    lane_row[field] = true.into();
                }
            }
            lane_row
        })
        .collect();
    if !lane_rows.is_empty() {
        row["rulerLanes"] = lane_rows.into();
    }
    let groups: Vec<Value> = (1..=64)
        .filter_map(|group| {
            let name = reaper.project_text(proj, &format!("TRACK_GROUP_NAME:{group}")).unwrap_or_default();
            (!name.is_empty()).then(|| json!({"group": group, "name": clip(&name)}))
        })
        .collect();
    if !groups.is_empty() {
        row["trackGroups"] = groups.into();
    }
    if let Some((division, mode, swing)) = reaper.grid(proj) {
        row["grid"] = json!({"division": round(division, 6), "swing": if mode == 1 { round(swing, 4) } else { 0.0 }, "measure": mode == 3});
    }
    let timebase = |name: &str| reaper.config_text(name).and_then(|text| text.trim().parse::<i64>().ok());
    let words = |code: i64| match code {
        0 => "time",
        1 => "beats",
        2 => "beats-position",
        _ => "other",
    };
    if let (Some(items), Some(tempo)) = (timebase("itemtimelock"), timebase("tempoenvtimelock")) {
        row["timebase"] = json!({"items": words(items), "tempoEnvelopes": words(tempo)});
    }
    // (m6-places) Whether the metronome is on, and how many tempo and time signature markers there are (a tempo map makes
    // a bar's length and "the tempo" depend on where).
    if let Some(flags) = timebase("projmetroen") {
        row["metronome"] = (flags & 1 != 0).into();
    }
    let markers = reaper.tempo_marker_count(proj);
    if markers > 0 {
        row["tempoMarkers"] = markers.into();
    }
    for (key, field) in [("PROJECT_TITLE", "title"), ("PROJECT_AUTHOR", "author")] {
        if let Some(text) = reaper.project_text(proj, key).filter(|text| !text.is_empty()) {
            row[field] = clip(&text).into();
        }
    }
    if let Some(notes) = reaper.project_notes(proj).filter(|notes| !notes.is_empty()) {
        let shown: String = notes.chars().take(NOTES_SHOWN).collect();
        if shown.len() < notes.len() {
            row["notesClipped"] = true.into();
        }
        row["notes"] = shown.into();
    }
}

pub fn track_row(reaper: &Reaper, track: Track) -> Value {
    let id = track_id(reaper, track);
    let master = id == TrackId::Master;
    let number = reaper.track_value(track, "IP_TRACKNUMBER");
    let mut row = json!({
        "ref": refs::track_ref(&id),
        "index": if master { 0 } else { number as i64 },
        "name": clip(&reaper.track_name(track)),
        "depth": if master { 0 } else { reaper.track_depth(track) },
        "folder": reaper.track_value(track, "I_FOLDERDEPTH") as i64,
        "color": color(reaper, reaper.track_color(track)),
        "mute": reaper.track_value(track, "B_MUTE") != 0.0,
        "solo": reaper.track_value(track, "I_SOLO") != 0.0,
        "arm": reaper.track_value(track, "I_RECARM") != 0.0,
        "volumeDb": db(reaper.track_value(track, "D_VOL")),
        "pan": round(reaper.track_value(track, "D_PAN"), 4),
        "fxCount": reaper.fx_count(FxOwner::Track(track)),
        "itemCount": reaper.track_item_count(track),
        "sends": reaper.send_count(track, 0),
        "receives": reaper.send_count(track, -1),
        "selected": reaper.track_value(track, "I_SELECTED") != 0.0,
    });
    if master {
        row["master"] = true.into();
    }
    add_track_state(reaper, track, master, &mut row);
    crate::read::item_rows::razor_areas(reaper, track, &mut row);
    row
}

/// -1, which every new track has, follows the project's default pan mode.
fn pan_mode_name(mode: i64) -> &'static str {
    match mode {
        -1 => "default",
        0 => "classic",
        3 => "balance",
        5 => "stereo",
        6 => "dual",
        _ => "other",
    }
}

fn record_mode_name(mode: i64) -> &'static str {
    match mode {
        0 => "input",
        1 => "stereo-out",
        2 => "none",
        3 => "stereo-out-compensated",
        4 => "midi-out",
        5 => "mono-out",
        6 => "mono-out-compensated",
        7 => "midi-overdub",
        8 => "midi-replace",
        _ => "other",
    }
}

fn monitoring_name(mode: i64) -> &'static str {
    match mode {
        0 => "off",
        1 => "on",
        2 => "auto",
        _ => "other",
    }
}

/// -1 on a send is the track's own mode.
pub fn automation_mode_name(mode: i64) -> &'static str {
    match mode {
        -1 => "track",
        0 => "trim",
        1 => "read",
        2 => "touch",
        3 => "write",
        4 => "latch",
        _ => "other",
    }
}

/// `I_RECINPUT` as the header decodes it: none (below 0); MIDI from a device (63 all, 62 the virtual keyboard) and a
/// channel (0 all); or audio from a first channel, mono, stereo or as many as the track has.
fn record_input(value: i64) -> Value {
    if value < 0 {
        return json!({"kind": "none"});
    }
    if value & 4096 != 0 {
        let channel = value & 31;
        let device = (value >> 5) & 63;
        return json!({
            "kind": "midi",
            "channel": channel,
            "device": match device { 63 => "all".to_string(), 62 => "virtual-keyboard".to_string(), n => n.to_string() },
        });
    }
    let layout = if value & 2048 != 0 {
        "multichannel"
    } else if value & 1024 != 0 {
        "stereo"
    } else {
        "mono"
    };
    json!({"kind": "audio", "first": (value & 1023) + 1, "layout": layout})
}

/// Whether a folder is open or collapsed; null for a track that is not a folder.
pub fn folder_state(reaper: &Reaper, track: Track) -> Value {
    if reaper.track_value(track, "I_FOLDERDEPTH") as i64 != 1 {
        return Value::Null;
    }
    match reaper.track_value(track, "I_FOLDERCOMPACT") as i64 {
        0 => "open",
        1 => "collapsed",
        _ => "fully-collapsed",
    }
    .into()
}

/// What a mixer strip shows beyond the fader: channels, how it pans, whether it is shown, what it records, and the lanes
/// of a track with fixed lanes. A strip reads the same in any view.
fn add_track_state(reaper: &Reaper, track: Track, master: bool, row: &mut Value) {
    let value = |key: &str| reaper.track_value(track, key);
    let mode = value("I_PANMODE") as i64;
    row["channels"] = (value("I_NCHAN") as i64).into();
    row["panMode"] = pan_mode_name(mode).into();
    row["width"] = round(value("D_WIDTH"), 4).into();
    row["panLaw"] = (value("D_PANLAW") >= 0.0).then(|| round(value("D_PANLAW"), 4)).into();
    row["dualPan"] =
        if mode == 6 { json!({"left": round(value("D_DUALPANL"), 4), "right": round(value("D_DUALPANR"), 4)}) } else { Value::Null };
    row["phaseInverted"] = (value("B_PHASE") != 0.0).into();
    row["fxEnabled"] = (value("I_FXEN") != 0.0).into();
    row["automationMode"] = automation_mode_name(value("I_AUTOMODE") as i64).into();
    if master {
        return;
    }
    row["parentRef"] = reaper.track_parent(track).map(|parent| refs::track_ref(&track_id(reaper, parent))).into();
    row["shownInArrange"] = (value("B_SHOWINTCP") != 0.0).into();
    row["shownInMixer"] = (value("B_SHOWINMIXER") != 0.0).into();
    row["folderState"] = folder_state(reaper, track);
    row["recordInput"] = record_input(value("I_RECINPUT") as i64);
    row["recordMode"] = record_mode_name(value("I_RECMODE") as i64).into();
    row["monitoring"] = monitoring_name(value("I_RECMON") as i64).into();
    // (r-tracks) The send to its parent (its folder, or the master): on, the parent's first channel (from 1), how many.
    let channels = value("C_MAINSEND_NCH") as i64;
    row["mainSend"] = json!({
        "on": value("B_MAINSEND") != 0.0,
        "offset": value("C_MAINSEND_OFFS") as i64 + 1,
        "channels": if channels == 0 { json!("all") } else { json!(channels) },
    });
    // What a power user set away from the defaults, only when set: an output's tap, items monitored while recording, solo
    // defeat, the MIDI hardware output.
    if matches!(value("I_RECMODE") as i64, 1 | 3 | 4 | 5 | 6) {
        row["recordTap"] = ["post-fader", "pre-fx", "post-fx", "post-fx"][(value("I_RECMODE_FLAGS") as i64 & 3) as usize].into();
    }
    if value("I_RECMONITEMS") != 0.0 {
        row["monitorItems"] = true.into();
    }
    if value("B_SOLO_DEFEAT") != 0.0 {
        row["soloDefeat"] = true.into();
    }
    let midi_out = value("I_MIDIHWOUT") as i64;
    if midi_out >= 0 {
        row["midiOut"] = json!({"device": (midi_out >> 5) & 31, "channel": midi_out & 31});
    }
    let groups = track_groups(reaper, track);
    if !groups.is_empty() {
        row["groups"] = groups.into();
    }
    // Fixed lanes (comping): how many, what each is called and which play.
    let free_mode = value("I_FREEMODE") as i64;
    row["itemLayout"] = match free_mode {
        1 => "free",
        2 => "lanes",
        _ => "normal",
    }
    .into();
    if free_mode == 2 {
        let lanes = (0..value("I_NUMFIXEDLANES") as i64)
            .map(|n| {
                let plays = match reaper.track_value(track, &format!("C_LANEPLAYS:{n}")) as i64 {
                    0 => "no",
                    1 => "only",
                    _ => "with-others",
                };
                json!({"index": n, "name": clip(&reaper.track_string(track, &format!("P_LANENAME:{n}")).unwrap_or_default()), "plays": plays})
            })
            .collect::<Vec<_>>();
        row["lanes"] = lanes.into();
    }
}

/// (r-tracks) A track's roles in REAPER's track groups, as Kumi names them and REAPER does (`GetSetTrackGroupMembership`).
pub const GROUP_ROLES: [(&str, &str); 25] = [
    ("volume-lead", "VOLUME_LEAD"),
    ("volume-follow", "VOLUME_FOLLOW"),
    ("vca-lead", "VOLUME_VCA_LEAD"),
    ("vca-follow", "VOLUME_VCA_FOLLOW"),
    ("pan-lead", "PAN_LEAD"),
    ("pan-follow", "PAN_FOLLOW"),
    ("width-lead", "WIDTH_LEAD"),
    ("width-follow", "WIDTH_FOLLOW"),
    ("mute-lead", "MUTE_LEAD"),
    ("mute-follow", "MUTE_FOLLOW"),
    ("solo-lead", "SOLO_LEAD"),
    ("solo-follow", "SOLO_FOLLOW"),
    ("record-arm-lead", "RECARM_LEAD"),
    ("record-arm-follow", "RECARM_FOLLOW"),
    ("polarity-lead", "POLARITY_LEAD"),
    ("polarity-follow", "POLARITY_FOLLOW"),
    ("automation-mode-lead", "AUTOMODE_LEAD"),
    ("automation-mode-follow", "AUTOMODE_FOLLOW"),
    ("media-edit-lead", "MEDIA_EDIT_LEAD"),
    ("media-edit-follow", "MEDIA_EDIT_FOLLOW"),
    ("volume-reverse", "VOLUME_REVERSE"),
    ("pan-reverse", "PAN_REVERSE"),
    ("width-reverse", "WIDTH_REVERSE"),
    ("no-lead-when-follow", "NO_LEAD_WHEN_FOLLOW"),
    ("vca-follow-pre-fx", "VOLUME_VCA_FOLLOW_ISPREFX"),
];

/// The groups a track is in, with its roles in each (groups 1-64): empty when it is in none.
pub fn track_groups(reaper: &Reaper, track: Track) -> Vec<Value> {
    let mut found: Vec<(i64, Vec<&str>)> = Vec::new();
    for (role, name) in GROUP_ROLES {
        for high in [false, true] {
            let Some(mask) = reaper.group_membership(track, name, high).filter(|mask| *mask != 0) else { continue };
            for bit in (0..32).filter(|bit| mask & (1u32 << bit) != 0) {
                let group = bit as i64 + 1 + if high { 32 } else { 0 };
                match found.iter_mut().find(|(g, _)| *g == group) {
                    Some((_, roles)) => roles.push(role),
                    None => found.push((group, vec![role])),
                }
            }
        }
    }
    found.sort_by_key(|(group, _)| *group);
    found.into_iter().map(|(group, roles)| json!({"group": group, "roles": roles})).collect()
}

/// What a take's source is called in a row: "wav", "mid", other types in lower case, "empty" for no take or source.
pub fn source_word(reaper: &Reaper, take: Option<Take>) -> String {
    match take.and_then(|t| reaper.take_source_type(t)) {
        None => "empty".into(),
        Some(kind) => match kind.to_ascii_lowercase().as_str() {
            "wave" => "wav".into(),
            "midi" => "mid".into(),
            other => other.to_string(),
        },
    }
}

pub fn item_row(reaper: &Reaper, proj: Project, item: Item) -> Value {
    let guid = reaper.item_guid(item);
    let take = reaper.active_take(item);
    let (position, length) = (reaper.item_value(item, "D_POSITION"), reaper.item_value(item, "D_LENGTH"));
    let start = reaper.quarter_notes_at(proj, position);
    let end = reaper.quarter_notes_at(proj, position + length);
    let mut row = json!({
        "ref": refs::item_ref(&guid),
        "trackRef": reaper.item_track(item).map(|t| refs::track_ref(&track_id(reaper, t))),
        "position": seconds(position),
        "length": seconds(length),
        "positionBeats": start.map(seconds),
        "lengthBeats": start.zip(end).map(|(start, end)| seconds(end - start)),
        "name": take.map(|t| clip(&reaper.take_name(t))).unwrap_or_default(),
        "takeRef": take.map(|t| refs::take_ref(&reaper.take_guid(t))),
        "takes": reaper.take_count(item),
        "source": source_word(reaper, take),
        "muted": reaper.item_value(item, "B_MUTE") != 0.0,
        "lane": reaper.item_value(item, "I_FIXEDLANE") as i64,
        "lanePlays": lane_plays(reaper, item),
        "color": item_color(reaper, item, take),
        "selected": reaper.item_value(item, "B_UISEL") != 0.0,
    });
    crate::read::item_rows::item_details(reaper, item, &mut row);
    row
}

/// The colour REAPER draws an item in: its active take's own, else the item's.
pub fn item_color(reaper: &Reaper, item: Item, take: Option<Take>) -> Value {
    let own = take.map(|take| reaper.take_value(take, "I_CUSTOMCOLOR") as i32).filter(|native| native & 0x100_0000 != 0);
    match own {
        Some(native) => color(reaper, Some(native)),
        None => custom_color(reaper, reaper.item_value(item, "I_CUSTOMCOLOR")),
    }
}

/// Whether the item's fixed lane plays: only on a track with fixed lanes, else null.
pub fn lane_plays(reaper: &Reaper, item: Item) -> Value {
    let on_lanes = reaper.item_track(item).is_some_and(|track| reaper.track_value(track, "I_FREEMODE") as i64 == 2);
    if !on_lanes {
        return Value::Null;
    }
    match reaper.item_value(item, "C_LANEPLAYS") as i64 {
        0 => "no",
        1 => "only",
        2 => "with-others",
        _ => "hidden",
    }
    .into()
}

/// An FX in a chain: `position` is its place there; a container lists its children's refs.
pub fn fx_row(reaper: &Reaper, fx: Fx, position: usize) -> Value {
    let guid = reaper.fx_guid(fx);
    let renamed = reaper.fx_config(fx, "renamed_name").unwrap_or_default();
    let name = if renamed.is_empty() { reaper.fx_name(fx) } else { renamed };
    let children = reaper.fx_config(fx, "container_count").is_some().then(|| reaper.fx_children(fx));
    let mut row = json!({
        "ref": refs::fx_ref(&guid),
        "index": position,
        "name": clip(&name),
        "enabled": reaper.fx_enabled(fx),
        "offline": reaper.fx_offline(fx),
        "preset": clip(&reaper.fx_preset(fx)),
        "paramCount": reaper.fx_param_count(fx),
        "container": children.is_some(),
    });
    if let Some(children) = children {
        row["childRefs"] = children.into_iter().map(|child| refs::fx_ref(&reaper.fx_guid(child))).collect::<Vec<_>>().into();
    }
    if let Some(parallel) = reaper.fx_config(fx, "parallel").and_then(|p| p.trim().parse::<i32>().ok()).filter(|&p| p != 0) {
        row["parallel"] = parallel.into();
    }
    if fx.index & crate::reaper::INPUT_FX != 0 {
        row["input"] = true.into();
    }
    // M5 listening (m5-reaper): the FX's track (and take), whether it is a plug-in from outside REAPER, its own name.
    crate::listening::reads::add_fx_place(reaper, fx, &mut row);
    row
}

pub fn param_row(reaper: &Reaper, fx: Fx, fx_guid: &str, index: i32) -> Value {
    let (min, max) = reaper.param_range(fx, index).unwrap_or((0.0, 1.0));
    json!({
        "ref": refs::param_ref(fx_guid, index),
        "index": index,
        "name": clip(&reaper.param_name(fx, index)),
        "value": round(reaper.param_normalized(fx, index), 6),
        "formatted": clip(&reaper.param_formatted(fx, index)),
        "min": min,
        "max": max,
    })
}

pub fn send_mode_name(mode: i64) -> &'static str {
    match mode {
        0 => "post-fader",
        1 => "pre-fx",
        // 2 is the deprecated spelling of 3.
        2 | 3 => "pre-fader",
        8 => "pre-receive",
        _ => "other",
    }
}

/// Channels of a send as `{first, count}`: the source's count comes from the width REAPER packs above the low 10 bits.
fn channels(reaper: &Reaper, track: Track, category: i32, index: i32) -> Value {
    let source = reaper.send_value(track, category, index, "I_SRCCHAN") as i64;
    let destination = reaper.send_value(track, category, index, "I_DSTCHAN") as i64;
    let source = if source < 0 {
        Value::Null
    } else {
        let width = source >> 10;
        json!({"first": source & 1023, "count": match width { 0 => 2, 1 => 1, n => 2 * n }})
    };
    json!({"src": source, "dst": {"first": destination & 1023, "mono": destination & 1024 != 0}})
}

/// What names a send in its ref: the other end, and which of several to the same one (`repeat`, from 1) among how
/// many (`of`). REAPER gives a send no identity, so a ref to one of several reads nothing once their number changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendKey {
    pub other: SendEnd,
    pub repeat: u32,
    pub of: u32,
}

/// The keys of all of a track's sends (`category` 0), receives (-1) or hardware outputs (1), by position.
pub fn send_keys(reaper: &Reaper, track: Track, category: i32) -> Vec<SendKey> {
    let ends: Vec<SendEnd> = (0..reaper.send_count(track, category))
        .map(|index| match reaper.send_peer(track, category, index) {
            Some(peer) => SendEnd::Track(track_id(reaper, peer)),
            // The output it goes to, not its place in the list: removing another output must not rename this one.
            None => SendEnd::Hardware(reaper.send_value(track, category, index, "I_DSTCHAN") as i32),
        })
        .collect();
    let mut totals: HashMap<String, u32> = HashMap::new();
    for end in &ends {
        *totals.entry(end.text()).or_insert(0) += 1;
    }
    let mut seen: HashMap<String, u32> = HashMap::new();
    ends.into_iter()
        .map(|other| {
            let text = other.text();
            let repeat = seen.entry(text.clone()).or_insert(0);
            *repeat += 1;
            SendKey { repeat: *repeat, of: totals[&text], other }
        })
        .collect()
}

/// A send's attributes that make its state, as REAPER names them.
pub const SEND_STATE: [&str; 10] =
    ["D_VOL", "D_PAN", "B_MUTE", "I_SENDMODE", "I_SRCCHAN", "I_DSTCHAN", "I_MIDIFLAGS", "B_PHASE", "B_MONO", "I_AUTOMODE"];

/// A send's envelopes, as what tells it from a send with the same settings: which of them it has, and on. (Not their points:
/// the producer draws automation on a send all the time, and that is no other send.)
pub fn send_envelope_print(reaper: &Reaper, track: Track, index: i32) -> String {
    ["<VOLENV", "<PANENV", "<MUTEENV"]
        .into_iter()
        .map(|chunk| match reaper.send_envelope(track, index, chunk) {
            Some(envelope) => u8::from(reaper.envelope_flag(envelope, "ACTIVE")).to_string(),
            None => "-".to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The states of all the sends from one track to another, in order, with their envelopes, as one hash: it moves when one
/// is added, taken away, or changed.
pub fn pair_hash(reaper: &Reaper, owner: Track, receiver: Track) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for index in 0..reaper.send_count(owner, 0) {
        if reaper.send_peer(owner, 0, index) != Some(receiver) {
            continue;
        }
        for parm in SEND_STATE {
            for byte in reaper.send_value(owner, 0, index, parm).to_bits().to_le_bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        for byte in send_envelope_print(reaper, owner, index).bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash = (hash ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// What a send or receive ref carries of the sends between the same two tracks, when there are two or more of them: their
/// state as one short hash. None for one alone between its tracks, or a hardware output.
pub fn pair_seen(reaper: &Reaper, owner: Track, category: i32, index: i32, key: &SendKey) -> Option<String> {
    if key.of < 2 {
        return None;
    }
    match category {
        0 => {
            let receiver = reaper.send_peer(owner, 0, index)?;
            Some(pair_hash(reaper, owner, receiver)[..8].to_string())
        }
        // (r-tracks) A receive is the send seen from the other end: its ref carries the pair's state from the sending
        // side, as that send's ref does, and Kumi changes it through that send.
        c if c < 0 => {
            let sender = reaper.send_peer(owner, c, index)?;
            Some(pair_hash(reaper, sender, owner)[..8].to_string())
        }
        _ => None,
    }
}

/// A send (`category` 0), receive (-1) or hardware output (1) of `track`, with the key `send_keys` gave it.
pub fn send_row(reaper: &Reaper, track: Track, category: i32, index: i32, key: &SendKey) -> Value {
    let this = track_id(reaper, track);
    // The other track: where a send goes, where a receive comes from; none for a hardware output.
    let other = match &key.other {
        SendEnd::Track(id) => Some(refs::track_ref(id)),
        SendEnd::Hardware(_) => None,
    };
    let mode = reaper.send_value(track, category, index, "I_SENDMODE") as i64;
    let mut row = json!({
        "ref": refs::send_ref(&this, category, &key.other, key.repeat, key.of, pair_seen(reaper, track, category, index, key).as_deref()),
        "category": category,
        "direction": match category { c if c < 0 => "receive", 0 => "send", _ => "hardware" },
        "index": index,
        "trackRef": refs::track_ref(&this),
        "destRef": other,
        "volumeDb": db(reaper.send_value(track, category, index, "D_VOL")),
        "pan": round(reaper.send_value(track, category, index, "D_PAN"), 4),
        "mute": reaper.send_value(track, category, index, "B_MUTE") != 0.0,
        "mode": mode,
        "modeName": send_mode_name(mode),
        "channels": channels(reaper, track, category, index),
        "phaseInverted": reaper.send_value(track, category, index, "B_PHASE") != 0.0,
        // (a hardware output's mono is its I_DSTCHAN's 1024 bit: B_MONO on one isn't kept by REAPER)
        "mono": if category == 1 {
            reaper.send_value(track, category, index, "I_DSTCHAN") as i64 & 1024 != 0
        } else {
            reaper.send_value(track, category, index, "B_MONO") != 0.0
        },
        "automationMode": automation_mode_name(reaper.send_value(track, category, index, "I_AUTOMODE") as i64),
    });
    if category <= 0 {
        row["midi"] = midi_flags(reaper.send_value(track, category, index, "I_MIDIFLAGS") as i64);
    }
    // The envelopes of a send (volume, pan, mute) are the sending track's own. REAPER hands out one of each kind for every
    // send, active or not, and an inactive one has no GUID to keep: a send lists the ones that are in use.
    if category == 0 {
        let envelopes: Vec<Value> = ["<VOLENV", "<PANENV", "<MUTEENV"]
            .into_iter()
            .filter_map(|chunk| reaper.send_envelope(track, index, chunk))
            .filter(|envelope| reaper.envelope_flag(*envelope, "ACTIVE"))
            .filter_map(|envelope| reaper.envelope_guid(envelope))
            .map(|guid| refs::envelope_ref(&this, &EnvelopeId::Guid(guid)).into())
            .collect();
        row["envelopeRefs"] = envelopes.into();
    }
    row
}

/// `I_MIDIFLAGS` of a send: whether MIDI goes (low 5 bits 31 turn it off), from which channel (0 all) to which (0 the
/// original), on which buses (0 all), and whether the faders send MIDI volume and pan.
fn midi_flags(flags: i64) -> Value {
    let source = flags & 31;
    json!({
        "enabled": source != 31,
        "srcChannel": if source == 31 { 0 } else { source },
        "dstChannel": (flags >> 5) & 31,
        "srcBus": (flags >> 14) & 255,
        "dstBus": (flags >> 22) & 255,
        "faders": flags & 1024 != 0,
    })
}

pub fn marker_row(reaper: &Reaper, proj: Project, marker: &Marker) -> Value {
    let beats = |time: f64| reaper.quarter_notes_at(proj, time).map(seconds);
    let mut row = json!({
        "ref": if marker.is_region { refs::region_ref(&marker.guid) } else { refs::marker_ref(&marker.guid) },
        "number": marker.number,
        "name": clip(&marker.name),
        "position": seconds(marker.start),
        "positionBeats": beats(marker.start),
        "color": custom_color(reaper, marker.color as f64),
        // (the ruler lane it sits in, which marker-lane moves it to)
        "lane": marker.lane,
    });
    if marker.is_region {
        row["end"] = seconds(marker.end).into();
        row["endBeats"] = beats(marker.end).into();
    }
    row
}

/// A note in quarter notes from the start of its item.
pub fn note_row(reaper: &Reaper, take: Take, item_start_qn: f64, index: i32, note: &MidiNote) -> Value {
    let start = reaper.ppq_to_quarter_notes(take, note.start_ppq).unwrap_or(0.0);
    let end = reaper.ppq_to_quarter_notes(take, note.end_ppq).unwrap_or(start);
    json!({
        "index": index,
        "pitch": note.pitch,
        "start": round(start - item_start_qn, 4),
        "length": round(end - start, 4),
        "velocity": note.velocity,
        "channel": note.channel + 1,
        "selected": note.selected,
        "muted": note.muted,
    })
}

/// The envelope's row, named by its GUID. An FX parameter's envelope also says which FX and parameter it is for.
pub fn envelope_row(reaper: &Reaper, track: &TrackId, envelope: Envelope) -> Value {
    let reference = reaper.envelope_guid(envelope).map(|guid| refs::envelope_ref(track, &EnvelopeId::Guid(guid)));
    let (fx_ref, param) = match reaper.envelope_owner(envelope) {
        Some((owner, index, param)) if index >= 0 => {
            (Some(refs::fx_ref(&reaper.fx_guid(Fx { owner: FxOwner::Track(owner), index }))), Some(param))
        }
        _ => (None, None),
    };
    json!({
        "ref": reference,
        "name": clip(&reaper.envelope_name(envelope)),
        "pointCount": reaper.envelope_point_count(envelope),
        "armed": reaper.envelope_flag(envelope, "ARM"),
        "visible": reaper.envelope_flag(envelope, "VISIBLE"),
        "active": reaper.envelope_flag(envelope, "ACTIVE"),
        "fxRef": fx_ref,
        "param": param,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sends_mode_has_a_name_for_every_spelling_reaper_has() {
        assert_eq!([0, 1, 2, 3, 8, 5].map(send_mode_name), ["post-fader", "pre-fx", "pre-fader", "pre-fader", "pre-receive", "other"]);
    }

    #[test]
    fn gains_are_decibels_with_a_floor() {
        assert_eq!(db(1.0), 0.0);
        assert_eq!(db(0.5), -6.02);
        assert_eq!(db(2.0), 6.02);
        assert_eq!(db(0.0), MIN_DB);
        assert_eq!(db(1e-12), MIN_DB);
    }

    #[test]
    fn long_text_is_clipped_with_an_ellipsis() {
        assert_eq!(clip("short"), "short");
        let clipped = clip(&"é".repeat(TEXT_LIMIT + 50));
        assert_eq!(clipped.chars().count(), TEXT_LIMIT);
        assert!(clipped.ends_with('…'));
    }
}
