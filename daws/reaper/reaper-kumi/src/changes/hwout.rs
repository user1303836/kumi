//! A track's hardware outputs (r-tracks, M4): adding and removing one, and changing its volume, pan, mute, mode, channels,
//! polarity and mono. REAPER gives an output no identity of its own: its ref names it by the output it goes to, and Kumi
//! writes its mark (`P_EXT:kumi_id`) on one at Kumi's first write, as on a send, so its undo finds it wherever it sits in
//! the track's list and whatever output it goes to now. Two outputs of one track to the same output can't be told apart by
//! a ref, so Kumi refuses to pick one.
//!
//! An output must go to outputs the audio device has (REAPER stores any, unheard: 7.82 lab), and takes no pre-receive mode
//! (REAPER's header: not for hardware outputs). REAPER lists a track's outputs in the order they were made, so one put back
//! after its removal comes last: said when it wasn't.

use serde_json::{json, Value};

use super::sends::{self, MODES};
use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, Track};
use crate::refs::{self, Ref, TrackId};
use crate::rows;

/// Hardware outputs, as REAPER's send calls name them.
const HW: i32 = 1;
const STAMP: &str = "P_EXT:kumi_id";

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

struct Found {
    track: Track,
    id: TrackId,
    index: i32,
}

fn name_of(reaper: &Reaper, track: Track) -> String {
    super::track_name(reaper, track)
}

/// "output 3/4" or "output 3 (mono)", from REAPER's `I_DSTCHAN` and the source's width.
fn output_text(dst: i64, src: i64) -> String {
    let first = (dst & 1023) + 1;
    if dst & 1024 != 0 {
        return format!("output {first} (mono)");
    }
    let count = sends::src_count(src).max(1);
    format!("output {}", sends::span_text(first, count))
}

/// The output's ends as HISTORY's picture names them: the track, and the output ("output 3–4").
fn ends(ctx: &Ctx, found: &Found) -> (String, String) {
    let dst = ctx.reaper.send_value(found.track, HW, found.index, "I_DSTCHAN") as i64;
    let src = ctx.reaper.send_value(found.track, HW, found.index, "I_SRCCHAN") as i64;
    (name_of(ctx.reaper, found.track), output_text(dst, src))
}

fn label(ctx: &Ctx, found: &Found) -> String {
    let dst = ctx.reaper.send_value(found.track, HW, found.index, "I_DSTCHAN") as i64;
    let src = ctx.reaper.send_value(found.track, HW, found.index, "I_SRCCHAN") as i64;
    format!("{} \u{2192} {}", super::quoted(&name_of(ctx.reaper, found.track)), output_text(dst, src))
}

fn stamp_of(reaper: &Reaper, track: Track, index: i32) -> Option<String> {
    reaper.send_text(track, HW, index, STAMP)
}

/// The output a ref names, by the output it goes to; refused when two of the track's outputs go there.
fn find(ctx: &Ctx, reference: &str) -> Done<Found> {
    let Ref::Send { track, category, other, repeat, of, .. } = Ref::parse(reference).map_err(bad)? else {
        return Err(bad("ref must be a hardware output's ref (r:send:{track}:1:hw…)"));
    };
    if category != HW {
        return Err(bad("ref must be a hardware output's ref (r:send:{track}:1:hw…): a send or receive has kinds of its own"));
    }
    let owner = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &track)?;
    let keys = rows::send_keys(ctx.reaper, owner, HW);
    let same: Vec<usize> = keys.iter().enumerate().filter(|(_, key)| key.other == other).map(|(at, _)| at).collect();
    if same.len() >= 2 || of >= 2 {
        return Err(changed(format!(
            "{} has {} hardware outputs to that output: Kumi can't tell which one this ref names (remove one, or change one in REAPER)",
            super::quoted(&name_of(ctx.reaper, owner)),
            same.len().max(of as usize)
        )));
    }
    let index = *same.first().ok_or_else(|| failed("that hardware output isn't there (removed, or it goes to another output now)"))?;
    let _ = repeat;
    Ok(Found { track: owner, id: track, index: index as i32 })
}

/// The output Kumi marked, on this track: none, or refused when two carry the mark (one was copied).
fn find_by_stamp(ctx: &Ctx, track: Track, stamp: &str) -> Done<Option<i32>> {
    let marked: Vec<i32> =
        (0..ctx.reaper.send_count(track, HW)).filter(|&index| stamp_of(ctx.reaper, track, index).as_deref() == Some(stamp)).collect();
    if marked.len() >= 2 {
        return Err(changed(format!(
            "two hardware outputs of {} carry the mark of Kumi's output (one was copied): Kumi can't tell which is its own",
            super::quoted(&name_of(ctx.reaper, track))
        )));
    }
    Ok(marked.first().copied())
}

/// The output a change is held to: by its mark wherever it sits; else by its ref, in the state the seal saw (`check`).
fn find_sealed(ctx: &Ctx, reference: &str, seal: &Value, check: bool) -> Done<Found> {
    if let Some(stamp) = seal["stamp"].as_str() {
        let Ref::Send { track, .. } = Ref::parse(reference).map_err(bad)? else { return Err(bad("target: a hardware output's ref")) };
        let owner = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &track)?;
        let index =
            find_by_stamp(ctx, owner, stamp)?.ok_or_else(|| failed("Kumi's hardware output isn't there (removed in REAPER since)"))?;
        return Ok(Found { track: owner, id: track, index });
    }
    let found = find(ctx, reference)?;
    if check && seal.is_object() && !same_state(&state(ctx.reaper, found.track, found.index), &seal["state"]) {
        return Err(changed(format!("the hardware output {} is not as it was: it may be another output now", label(ctx, &found))));
    }
    Ok(found)
}

fn for_apply(fault: Fault) -> Fault {
    if fault.0 == "failed" {
        changed(format!("{}: it changed since the preview", fault.1))
    } else {
        fault
    }
}

fn for_restore(fault: Fault) -> Fault {
    if fault.0 == "failed" {
        ("gone".to_string(), fault.1)
    } else {
        fault
    }
}

/// All of an output's settings, as a state to compare and to make again. (Not `B_MONO`: on an output it reads back what was
/// written but REAPER neither saves nor undoes it, lab 7.82; an output's mono is its `I_DSTCHAN`'s 1024 bit.)
const STATE_KEYS: [(&str, &str); 9] = [
    ("D_VOL", "volume"),
    ("D_PAN", "pan"),
    ("B_MUTE", "mute"),
    ("I_SENDMODE", "mode"),
    ("I_SRCCHAN", "srcChan"),
    ("I_DSTCHAN", "dstChan"),
    ("B_PHASE", "phase"),
    ("I_AUTOMODE", "autoMode"),
    ("D_PANLAW", "panLaw"),
];

fn state(reaper: &Reaper, track: Track, index: i32) -> Value {
    let mut state = json!({});
    for (parm, key) in STATE_KEYS {
        state[key] = reaper.send_value(track, HW, index, parm).into();
    }
    state["slotHint"] = reaper.send_value(track, HW, index, "I_SLOT_HINT").into();
    state
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn same_state(a: &Value, b: &Value) -> bool {
    STATE_KEYS.iter().all(|(_, key)| close(number(a, key), number(b, key)))
}

fn write_state(reaper: &Reaper, track: Track, index: i32, state: &Value) {
    for (parm, key) in STATE_KEYS.iter().copied().chain([("I_SLOT_HINT", "slotHint")]) {
        if let Some(value) = state.get(key).and_then(Value::as_f64) {
            reaper.set_send_value(track, HW, index, parm, value);
        }
    }
}

fn seal(reaper: &Reaper, found: &Found) -> Value {
    json!({"stamp": stamp_of(reaper, found.track, found.index), "state": state(reaper, found.track, found.index)})
}

/// The outputs the audio device has, in words, for a refusal.
fn outputs_words(ctx: &Ctx) -> String {
    match ctx.reaper.audio_outputs() {
        0 => "REAPER's audio device has no outputs (or none is open)".to_string(),
        n => {
            let names: Vec<String> =
                (0..n.min(8)).map(|i| format!("{} {}", i + 1, super::quoted(&ctx.reaper.audio_output_name(i)))).collect();
            let more = if n > 8 { format!(" and {} more", n - 8) } else { String::new() };
            format!("REAPER's audio device has {n} output(s): {}{more}", names.join(", "))
        }
    }
}

/// The output a state goes to must be one the device has.
fn reaches(ctx: &Ctx, src: i64, dst: i64) -> Done<()> {
    let count = if dst & 1024 != 0 { 1 } else { sends::src_count(src).max(1) };
    let last = (dst & 1023) + count;
    if last > ctx.reaper.audio_outputs() as i64 {
        return Err(bad(format!("{}: there is no output {last}", outputs_words(ctx))));
    }
    Ok(())
}

/// The source channels must be the track's own.
fn takes(ctx: &Ctx, track: Track, src: i64) -> Done<()> {
    if src < 0 {
        return Err(bad("a hardware output takes audio: srcChannels can't be \"none\""));
    }
    let channels = ctx.reaper.track_value(track, "I_NCHAN").max(2.0) as i64;
    let last = (src & 1023) + sends::src_count(src);
    if last > channels {
        return Err(bad(format!(
            "{} has {channels} channels, and the output would take from channel {last}: raise its channels first (track_channels)",
            super::quoted(&name_of(ctx.reaper, track))
        )));
    }
    Ok(())
}

/// REAPER's `I_DSTCHAN` for {first (from 1), mono?}.
fn output_code(value: &Value) -> Done<i64> {
    let first = value
        .get("first")
        .and_then(Value::as_i64)
        .filter(|n| (1..=512).contains(n))
        .ok_or_else(|| bad("output must be {first (the device's output, from 1), mono?}"))?;
    let mono = match value.get("mono") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("output.mono must be true or false")),
    };
    Ok((first - 1) | if mono { 1024 } else { 0 })
}

fn mode_of(input: &Value) -> Done<Option<i64>> {
    let modes = "mode must be post-fader, pre-fader or pre-fx (a hardware output has no pre-receive)";
    match input.get("mode") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(word)) if word != "pre-receive" => {
            MODES.iter().find(|(name, _)| name == word).map(|(_, mode)| Some(*mode)).ok_or_else(|| bad(modes))
        }
        Some(_) => Err(bad(modes)),
    }
}

fn db_gain(input: &Value) -> Done<Option<f64>> {
    match input.get("volumeDb") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let db = v
                .as_f64()
                .filter(|db| db.is_finite() && (-150.0..=24.0).contains(db))
                .ok_or_else(|| bad("volumeDb must be from -150 (silent) to 24"))?;
            Ok(Some(if db <= -150.0 { 0.0 } else { 10f64.powf(db / 20.0) }))
        }
    }
}

fn pan_of(input: &Value) -> Done<Option<f64>> {
    match input.get("pan") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_f64().filter(|p| (-1.0..=1.0).contains(p)).map(Some).ok_or_else(|| bad("pan must be a number from -1 (left) to 1 (right)"))
        }
    }
}

// ----- an output's settings -----

pub struct Prop {
    pub kind: &'static str,
    read: fn(&Reaper, Track, i32) -> Value,
    /// The fields the input asks for, with the output's settings now (a channel change keeps what it isn't given).
    wanted: fn(&Ctx, Track, &Value, &Value) -> Done<Value>,
    title: fn(&str, &Value) -> String,
    noun: &'static str,
}

fn write_fields(reaper: &Reaper, track: Track, index: i32, state: &Value) -> bool {
    STATE_KEYS
        .iter()
        .all(|(parm, key)| state.get(key).and_then(Value::as_f64).is_none_or(|value| reaper.set_send_value(track, HW, index, parm, value)))
}

/// Whether `a` holds what `b` says, for every field `b` names.
fn holds(a: &Value, b: &Value) -> bool {
    b.as_object().is_some_and(|fields| fields.iter().all(|(key, want)| close(number(a, key), want.as_f64().unwrap_or(f64::NAN))))
}

fn touched(state: &Value, of: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    for key in of.as_object().into_iter().flatten().map(|(key, _)| key) {
        if let Some(value) = state.get(key) {
            fields.insert(key.clone(), value.clone());
        }
    }
    Value::Object(fields)
}

pub static PROPS: [Prop; 7] = [
    Prop {
        kind: "hw-out-volume",
        read: |reaper, track, index| state(reaper, track, index),
        wanted: |_, _, _, input| Ok(json!({"volume": db_gain(input)?.ok_or_else(|| bad("volumeDb must be a number"))?})),
        title: |route, to| format!("Hardware output {route} {} dB", super::tracks::db_text(rows::db(number(to, "volume")))),
        noun: "volume",
    },
    Prop {
        kind: "hw-out-pan",
        read: |reaper, track, index| state(reaper, track, index),
        wanted: |_, _, _, input| Ok(json!({"pan": pan_of(input)?.ok_or_else(|| bad("pan must be a number from -1 (left) to 1 (right)"))?})),
        title: |route, to| format!("Hardware output {route} pan {}", super::tracks::pan_text(number(to, "pan"))),
        noun: "pan",
    },
    Prop {
        kind: "hw-out-mute",
        read: |reaper, track, index| state(reaper, track, index),
        wanted: |_, _, _, input| {
            let mute = input.get("mute").and_then(Value::as_bool).ok_or_else(|| bad("mute must be true or false"))?;
            Ok(json!({"mute": if mute { 1.0 } else { 0.0 }}))
        },
        title: |route, to| format!("Hardware output {route} {}", if number(to, "mute") != 0.0 { "muted" } else { "unmuted" }),
        noun: "mute",
    },
    Prop {
        kind: "hw-out-mode",
        read: |reaper, track, index| state(reaper, track, index),
        wanted: |_, _, _, input| {
            Ok(json!({"mode": mode_of(input)?.ok_or_else(|| bad("mode must be post-fader, pre-fader or pre-fx"))? as f64}))
        },
        title: |route, to| format!("Hardware output {route} {}", rows::send_mode_name(number(to, "mode") as i64)),
        noun: "mode",
    },
    Prop {
        kind: "hw-out-channels",
        read: |reaper, track, index| state(reaper, track, index),
        wanted: |ctx, track, now, input| {
            let mut wanted = json!({});
            if let Some(src) = input.get("srcChannels").filter(|v| !v.is_null()) {
                wanted["srcChan"] = (sends::src_code(src)? as f64).into();
            }
            if let Some(output) = input.get("output").filter(|v| !v.is_null()) {
                wanted["dstChan"] = (output_code(output)? as f64).into();
            }
            if wanted.as_object().is_some_and(|w| w.is_empty()) {
                return Err(bad("give srcChannels (the track's channels it takes) or output (the device's output it goes to)"));
            }
            let src = wanted.get("srcChan").map_or(number(now, "srcChan"), |v| v.as_f64().unwrap_or(0.0)) as i64;
            let dst = wanted.get("dstChan").map_or(number(now, "dstChan"), |v| v.as_f64().unwrap_or(0.0)) as i64;
            takes(ctx, track, src)?;
            reaches(ctx, src, dst)?;
            Ok(wanted)
        },
        title: |route, to| {
            let src = to.get("srcChan").and_then(Value::as_f64).map(|src| {
                let src = src as i64;
                format!("from channels {}", sends::span_text((src & 1023) + 1, sends::src_count(src)))
            });
            let dst = to
                .get("dstChan")
                .and_then(Value::as_f64)
                .map(|dst| format!("to {}", output_text(dst as i64, to.get("srcChan").and_then(Value::as_f64).unwrap_or(0.0) as i64)));
            format!("Hardware output {route} {}", [src, dst].into_iter().flatten().collect::<Vec<_>>().join(" "))
        },
        noun: "channels",
    },
    Prop {
        kind: "hw-out-phase",
        read: |reaper, track, index| state(reaper, track, index),
        wanted: |_, _, _, input| {
            let inverted = input.get("inverted").and_then(Value::as_bool).ok_or_else(|| bad("inverted must be true or false"))?;
            Ok(json!({"phase": if inverted { 1.0 } else { 0.0 }}))
        },
        title: |route, to| {
            format!("Hardware output {route} polarity {}", if number(to, "phase") != 0.0 { "inverted" } else { "back to normal" })
        },
        noun: "polarity",
    },
    Prop {
        kind: "hw-out-mono",
        read: |reaper, track, index| state(reaper, track, index),
        // (the output's mono is its I_DSTCHAN's 1024 bit, as REAPER saves it)
        wanted: |ctx, _, now, input| {
            let mono = input.get("mono").and_then(Value::as_bool).ok_or_else(|| bad("mono must be true or false"))?;
            let dst = number(now, "dstChan") as i64;
            let dst = if mono { dst | 1024 } else { dst & !1024 };
            reaches(ctx, number(now, "srcChan") as i64, dst)?;
            Ok(json!({"dstChan": dst as f64}))
        },
        title: |route, to| {
            format!("Hardware output {route} {}", if number(to, "dstChan") as i64 & 1024 != 0 { "summed to mono" } else { "in stereo" })
        },
        noun: "mono",
    },
];

fn target(reference: &str) -> Value {
    json!({"ref": reference, "kind": "send"})
}

pub fn prop_preview(ctx: &Ctx, prop: &Prop, input: &Value) -> Done<Preview> {
    let reference = input["ref"].as_str().unwrap_or_default();
    let found = find(ctx, reference)?;
    let now = (prop.read)(ctx.reaper, found.track, found.index);
    let wanted = (prop.wanted)(ctx, found.track, &now, input)?;
    let route = label(ctx, &found);
    let mut display = json!({"title": (prop.title)(&route, &wanted), "before": touched(&now, &wanted), "after": wanted});
    if holds(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    let fence = json!({"field": touched(&now, &wanted), "output": seal(ctx.reaper, &found)});
    Ok(Preview { target: target(reference), prior: touched(&now, &wanted), fence, display })
}

pub fn prop_apply(ctx: &Ctx, prop: &Prop, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_sealed(ctx, input["ref"].as_str().unwrap_or_default(), &fence["output"], true).map_err(for_apply)?;
    let reaper = ctx.reaper;
    let now = (prop.read)(reaper, found.track, found.index);
    let route = label(ctx, &found);
    // What the change touches must be as the preview saw it (its identity is the seal's: its mark, or its whole state).
    if !holds(&now, &fence["field"]) {
        return Err(changed(format!("{route}'s {} changed since the preview", prop.noun)));
    }
    let wanted = (prop.wanted)(ctx, found.track, &now, input)?;
    let unchanged = holds(&now, &wanted);
    if !unchanged {
        if !write_fields(reaper, found.track, found.index, &wanted) {
            return Err(failed(format!("REAPER wouldn't take the hardware output's {}", prop.noun)));
        }
        if !holds(&(prop.read)(reaper, found.track, found.index), &wanted) {
            write_fields(reaper, found.track, found.index, &touched(&now, &wanted));
            return Err(failed(format!("REAPER kept the hardware output's {} as it was", prop.noun)));
        }
        // Kumi's mark, at its first write to this output, in the same step.
        if stamp_of(reaper, found.track, found.index).is_none() {
            reaper.set_send_text(found.track, HW, found.index, STAMP, &sends::fresh_stamp());
        }
    }
    let after = (prop.read)(reaper, found.track, found.index);
    let (from, to) = ends(ctx, &found);
    let drawn = sends::drawn_route(&from, &to, Some(&touched(&now, &wanted)), Some(&touched(&after, &wanted)));
    let mut applied = touched(&after, &wanted);
    applied["output"] = seal(reaper, &found);
    Ok(Applied {
        applied,
        created: None,
        material: None,
        display: json!({
            "title": (prop.title)(&route, &wanted),
            "before": touched(&now, &wanted),
            "after": touched(&after, &wanted),
            "route": drawn,
        }),
        unwritten: unchanged,
    })
}

pub fn prop_restore(ctx: &Ctx, prop: &Prop, restore: &Restore) -> Done<Restored> {
    let reference = restore.target["ref"].as_str().unwrap_or_default();
    let found = find_sealed(ctx, reference, &restore.applied["output"], false).map_err(for_restore)?;
    let reaper = ctx.reaper;
    let route = label(ctx, &found);
    let now = (prop.read)(reaper, found.track, found.index);
    let mut prior = restore.prior.clone();
    let mut applied = restore.applied.clone();
    if let Some(fields) = applied.as_object_mut() {
        fields.remove("output");
    }
    if let Some(fields) = prior.as_object_mut() {
        fields.remove("output");
    }
    // An output without Kumi's mark (REAPER refused it) is told only by all it is: as Kumi left it, or as it was before
    // (back already). Anything else is another output, or this one changed past knowing, and is never written, even told
    // to put things back anyway.
    if restore.applied["output"]["stamp"].is_null() {
        let left = &restore.applied["output"]["state"];
        let mut before = left.clone();
        for (key, value) in prior.as_object().into_iter().flatten() {
            before[key] = value.clone();
        }
        if !same_state(&now, left) && !same_state(&now, &before) {
            return Err(changed(format!(
                "the hardware output {route} isn't as Kumi left it, and it has no mark of Kumi's to know it by: Kumi won't write to what may be another output"
            )));
        }
    }
    let already = holds(&now, &prior);
    if !already {
        if !restore.discard && !holds(&now, &applied) {
            return Err(changed(format!("{route}'s {} changed in REAPER since Kumi set it", prop.noun)));
        }
        if !write_fields(reaper, found.track, found.index, &prior) || !holds(&(prop.read)(reaper, found.track, found.index), &prior) {
            return Err(failed(format!("REAPER wouldn't put back the hardware output's {}", prop.noun)));
        }
    }
    let after = touched(&(prop.read)(reaper, found.track, found.index), &prior);
    Ok(Restored {
        restored: after.clone(),
        display: json!({"title": format!("{} (put back)", (prop.title)(&label(ctx, &found), &prior)), "before": touched(&now, &prior), "after": after}),
        unwritten: already,
    })
}

// ----- adding one -----

struct Add {
    track: Track,
    id: TrackId,
    src: i64,
    dst: i64,
    changes: Vec<(&'static str, f64)>,
}

fn add_plan(ctx: &Ctx, input: &Value) -> Done<Add> {
    let id = super::track_ref_of(input, "hw-out-add")?;
    let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id)?;
    let output =
        input.get("output").filter(|v| v.is_object()).ok_or_else(|| bad("output must be {first (the device's output, from 1), mono?}"))?;
    let dst = output_code(output)?;
    let src = match input.get("srcChannels").filter(|v| !v.is_null()) {
        Some(src) => sends::src_code(src)?,
        None => 0,
    };
    takes(ctx, track, src)?;
    reaches(ctx, src, dst)?;
    let mut changes = vec![("I_DSTCHAN", dst as f64), ("I_SRCCHAN", src as f64)];
    if let Some(gain) = db_gain(input)? {
        changes.push(("D_VOL", gain));
    }
    if let Some(pan) = pan_of(input)? {
        changes.push(("D_PAN", pan));
    }
    if let Some(mode) = mode_of(input)? {
        changes.push(("I_SENDMODE", mode as f64));
    }
    if let Some(mute) = input.get("mute").filter(|v| !v.is_null()) {
        changes.push(("B_MUTE", if mute.as_bool().ok_or_else(|| bad("mute must be true or false"))? { 1.0 } else { 0.0 }));
    }
    Ok(Add { track, id, src, dst, changes })
}

fn add_title(ctx: &Ctx, plan: &Add) -> String {
    format!("Added hardware output {} \u{2192} {}", super::quoted(&name_of(ctx.reaper, plan.track)), output_text(plan.dst, plan.src))
}

pub fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let plan = add_plan(ctx, input)?;
    Ok(Preview {
        target: json!({"ref": refs::track_ref(&plan.id), "kind": "track"}),
        prior: json!({"outputs": ctx.reaper.send_count(plan.track, HW)}),
        fence: json!({"outputs": ctx.reaper.send_count(plan.track, HW)}),
        display: json!({"title": add_title(ctx, &plan)}),
    })
}

pub fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = add_plan(ctx, input).map_err(for_apply)?;
    let reaper = ctx.reaper;
    if json!(reaper.send_count(plan.track, HW)) != fence["outputs"] {
        return Err(changed("the track's hardware outputs changed since the preview"));
    }
    let index = reaper.create_send(plan.track, None).ok_or_else(|| failed("REAPER wouldn't make the hardware output"))?;
    let stamp = sends::fresh_stamp();
    reaper.set_send_text(plan.track, HW, index, STAMP, &stamp);
    for (parm, value) in &plan.changes {
        reaper.set_send_value(plan.track, HW, index, parm, *value);
    }
    let key = rows::send_keys(reaper, plan.track, HW)
        .get(index as usize)
        .cloned()
        .ok_or_else(|| failed("the new output isn't in the track's list"))?;
    let reference = refs::send_ref(&plan.id, HW, &key.other, key.repeat, key.of, None);
    let mut created = target(&reference);
    if stamp_of(reaper, plan.track, index).as_deref() == Some(stamp.as_str()) {
        created["stamp"] = stamp.into();
    }
    let found = Found { track: plan.track, id: plan.id.clone(), index };
    let mut applied = state(reaper, plan.track, index);
    let (from, to) = ends(ctx, &found);
    let display = json!({"title": add_title(ctx, &plan), "route": sends::drawn_route(&from, &to, None, Some(&applied))});
    applied["output"] = seal(reaper, &found);
    Ok(Applied { applied, created: Some(created), material: None, display, ..Default::default() })
}

pub fn add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the output that was added"))?;
    let reference = created["ref"].as_str().ok_or_else(|| bad("created: the output's ref"))?;
    // Without Kumi's mark the output is told only by its state: told to undo anyway or not, another output is never removed.
    let stamped = restore.applied["output"]["stamp"].is_string();
    let check = !restore.discard || !stamped;
    let found = find_sealed(ctx, reference, &restore.applied["output"], check).map_err(for_restore)?;
    let now = state(ctx.reaper, found.track, found.index);
    if check && !same_state(&now, restore.applied) {
        return Err(changed(format!("the hardware output {} has been changed in REAPER since Kumi added it", label(ctx, &found))));
    }
    // Its envelopes go with it, and nothing brings them back: drawn since, they are the producer's.
    let drawn = drawn_envelopes(ctx.reaper, found.track, found.index);
    if !restore.discard && drawn > 0 {
        return Err(changed(format!(
            "the hardware output {} has {drawn} envelope(s) the producer added since Kumi made it, which would go with it",
            label(ctx, &found)
        )));
    }
    let route = label(ctx, &found);
    if !ctx.reaper.remove_send(found.track, HW, found.index) {
        return Err(failed("REAPER wouldn't remove the hardware output"));
    }
    Ok(Restored {
        restored: json!({"removed": reference}),
        display: json!({"title": format!("Hardware output {route} taken away again")}),
        ..Default::default()
    })
}

// ----- removing one -----

/// The output's volume, pan and mute envelopes that are on or hold points (one bypassed is still drawn): REAPER drops
/// them with it, and undo makes the output again without them.
fn drawn_envelopes(reaper: &Reaper, track: Track, index: i32) -> usize {
    ["<VOLENV", "<PANENV", "<MUTEENV"]
        .into_iter()
        .filter_map(|chunk| reaper.send_envelope_in(track, HW, index, chunk))
        .filter(|envelope| reaper.envelope_flag(*envelope, "ACTIVE") || reaper.envelope_point_count(*envelope) > 0)
        .count()
}

pub fn remove_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let reference = input["ref"].as_str().unwrap_or_default();
    let found = find(ctx, reference)?;
    let now = state(ctx.reaper, found.track, found.index);
    let mut display = json!({"title": format!("Removed hardware output {}", label(ctx, &found)), "before": now});
    let envelopes = drawn_envelopes(ctx.reaper, found.track, found.index);
    if envelopes > 0 {
        display["note"] = format!("Its {envelopes} envelope(s) can't be put back by undo.").into();
    }
    Ok(Preview { target: target(reference), prior: now.clone(), fence: json!({"output": seal(ctx.reaper, &found)}), display })
}

pub fn remove_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let reference = input["ref"].as_str().unwrap_or_default();
    let found = find_sealed(ctx, reference, &fence["output"], true).map_err(for_apply)?;
    let reaper = ctx.reaper;
    let now = state(reaper, found.track, found.index);
    let route = label(ctx, &found);
    let (from, to) = ends(ctx, &found);
    let last = found.index == reaper.send_count(found.track, HW) - 1;
    let stamp = stamp_of(reaper, found.track, found.index);
    let lost = drawn_envelopes(reaper, found.track, found.index);
    if !reaper.remove_send(found.track, HW, found.index) {
        return Err(failed("REAPER wouldn't remove the hardware output"));
    }
    Ok(Applied {
        applied: json!({"removed": true}),
        created: None,
        material: Some(json!({"state": now, "stamp": stamp, "last": last, "lostEnvelopes": lost, "trackRef": refs::track_ref(&found.id)})),
        display: json!({"title": format!("Removed hardware output {route}"), "route": sends::drawn_route(&from, &to, Some(&now), None)}),
        ..Default::default()
    })
}

pub fn remove_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the removal took away"))?;
    let reference = restore.target["ref"].as_str().unwrap_or_default();
    let Ref::Send { track, .. } = Ref::parse(reference).map_err(bad)? else { return Err(bad("target: a hardware output's ref")) };
    let reaper = ctx.reaper;
    let owner = ctx.reads.track_by_id(reaper, ctx.proj, &track).map_err(|(_, message)| ("gone".to_string(), message))?;
    // Back already (a retry, or the producer made it again): its mark, or one in exactly its state.
    let saved = &material["state"];
    let back = match material["stamp"].as_str() {
        Some(stamp) => find_by_stamp(ctx, owner, stamp)?,
        None => (0..reaper.send_count(owner, HW)).find(|&index| same_state(&state(reaper, owner, index), saved)),
    };
    if let Some(index) = back {
        return Ok(Restored {
            restored: state(reaper, owner, index),
            display: json!({"title": "The hardware output is back already"}),
            unwritten: true,
        });
    }
    let index = reaper.create_send(owner, None).ok_or_else(|| failed("REAPER wouldn't make the hardware output again"))?;
    write_state(reaper, owner, index, saved);
    if let Some(stamp) = material["stamp"].as_str() {
        reaper.set_send_text(owner, HW, index, STAMP, stamp);
    }
    let found = Found { track: owner, id: track, index };
    let mut display = json!({"title": format!("Hardware output {} put back", label(ctx, &found))});
    let mut notes = Vec::new();
    if material["last"] == false {
        notes.push("REAPER lists it after the track's other hardware outputs now.".to_string());
    }
    if let Some(lost) = material["lostEnvelopes"].as_u64().filter(|lost| *lost > 0) {
        notes.push(format!("Its {lost} envelope(s) were not put back."));
    }
    if !notes.is_empty() {
        display["note"] = notes.join(" ").into();
    }
    Ok(Restored { restored: json!({"state": state(reaper, owner, index)}), display, ..Default::default() })
}

/// The output a ref names, for the kind table.
pub fn prop(kind: &str) -> Option<&'static Prop> {
    PROPS.iter().find(|prop| prop.kind == kind)
}
