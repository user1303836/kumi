//! The timeline: the project tempo, tempo and time signature markers, the loop and whether it repeats. Each is read back after
//! it is written, and put back by value if the producer has not touched it since.

use serde_json::{json, Value};

use super::marks::{bar_text, moment, trimmed};
use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::TempoMarker;

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

fn number(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(f64::NAN)
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

// ----- the project's tempo -----

/// The tempo of a project with no tempo markers; with a tempo map the tempo is a marker's, at a place.
fn base_tempo(ctx: &Ctx) -> Done<f64> {
    if ctx.reaper.tempo_marker_count(ctx.proj) > 0 {
        return Err(bad("this project has tempo markers: change one at its place (tempo-marker-set), or add one at bar 1"));
    }
    ctx.reaper.tempo().ok_or_else(|| failed("this REAPER can't say the project's tempo"))
}

fn wanted_bpm(input: &Value) -> Done<f64> {
    input
        .get("bpm")
        .and_then(Value::as_f64)
        .filter(|bpm| bpm.is_finite() && (1.0..=960.0).contains(bpm))
        .ok_or_else(|| bad("bpm must be a number from 1 to 960"))
}

pub fn tempo_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = wanted_bpm(input)?;
    let now = json!({"bpm": base_tempo(ctx)?});
    let mut display = json!({"title": format!("Tempo {} BPM", trimmed(wanted)), "before": now, "after": {"bpm": wanted}});
    if close(number(&now, "bpm"), wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn tempo_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = wanted_bpm(input)?;
    let now = base_tempo(ctx)?;
    if !close(now, number(fence, "bpm")) {
        return Err(changed("the tempo changed since the preview"));
    }
    if !close(now, wanted) && !ctx.reaper.set_current_bpm(ctx.proj, wanted) {
        return Err(failed("REAPER wouldn't take the tempo"));
    }
    let after = base_tempo(ctx)?;
    if !close(after, wanted) {
        return Err(failed("REAPER kept the tempo as it was"));
    }
    Ok(Applied {
        applied: json!({"bpm": after}),
        created: None,
        material: None,
        display: json!({"title": format!("Tempo {} BPM", trimmed(after))}),
        unwritten: close(now, wanted),
    })
}

pub fn tempo_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (prior, applied) = (number(restore.prior, "bpm"), number(restore.applied, "bpm"));
    let now = base_tempo(ctx)?;
    if !close(now, prior) {
        if !restore.discard && !close(now, applied) {
            return Err(changed("the tempo has been changed in REAPER since Kumi set it"));
        }
        if !ctx.reaper.set_current_bpm(ctx.proj, prior) || !close(base_tempo(ctx)?, prior) {
            return Err(failed("REAPER wouldn't put the tempo back"));
        }
    }
    Ok(Restored {
        restored: json!({"bpm": prior}),
        display: json!({"title": format!("Tempo back to {} BPM", trimmed(prior))}),
        unwritten: close(now, prior),
    })
}

// ----- tempo and time signature markers -----

/// The marker at a time, with its index.
fn marker_at(ctx: &Ctx, time: f64) -> Option<(i32, TempoMarker)> {
    (0..ctx.reaper.tempo_marker_count(ctx.proj))
        .filter_map(|i| ctx.reaper.tempo_marker(ctx.proj, i).map(|m| (i, m)))
        .find(|(_, m)| (m.time - time).abs() < 1e-6)
}

fn marker_state(marker: Option<&TempoMarker>) -> Value {
    match marker {
        Some(m) => json!({"exists": true, "bpm": m.bpm, "num": m.num.max(0), "den": m.denom.max(0), "linear": m.linear}),
        None => json!({"exists": false}),
    }
}

fn same_marker(a: &Value, b: &Value) -> bool {
    if a["exists"] != b["exists"] {
        return false;
    }
    a["exists"] == false
        || (close(number(a, "bpm"), number(b, "bpm")) && a["num"] == b["num"] && a["den"] == b["den"] && a["linear"] == b["linear"])
}

fn signature_input(input: &Value) -> Done<Option<(i32, i32)>> {
    let Some(value) = input.get("timeSignature").filter(|v| !v.is_null()) else { return Ok(None) };
    let (num, den) = match value {
        Value::Array(pair) if pair.len() == 2 => (pair[0].as_i64(), pair[1].as_i64()),
        Value::Object(_) => (value["num"].as_i64(), value["den"].as_i64()),
        _ => (None, None),
    };
    match (num, den) {
        (Some(num), Some(den)) if (1..=64).contains(&num) && [1, 2, 4, 8, 16, 32, 64].contains(&den) => Ok(Some((num as i32, den as i32))),
        _ => Err(bad("timeSignature must be [numerator, denominator] or {num, den}: 1 to 64 over 1, 2, 4, 8, 16, 32 or 64")),
    }
}

struct MarkerPlan {
    time: f64,
    bpm: Option<f64>,
    signature: Option<(i32, i32)>,
    linear: Option<bool>,
}

fn marker_plan(ctx: &Ctx, input: &Value) -> Done<MarkerPlan> {
    let time = moment(ctx, input, "positionBeats", "position")?
        .ok_or_else(|| bad("pass positionBeats (quarter notes from the start) or position (seconds)"))?;
    let bpm = match input.get("bpm").filter(|v| !v.is_null()) {
        Some(_) => Some(wanted_bpm(input)?),
        None => None,
    };
    let signature = signature_input(input)?;
    let linear = match input.get("linear") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(linear)) => Some(*linear),
        Some(_) => return Err(bad("linear must be true or false")),
    };
    if bpm.is_none() && signature.is_none() {
        return Err(bad("pass bpm, timeSignature or both"));
    }
    Ok(MarkerPlan { time, bpm, signature, linear })
}

fn marker_title(ctx: &Ctx, plan: &MarkerPlan) -> String {
    let at = bar_text(ctx.reaper, ctx.proj, plan.time);
    match (plan.bpm, plan.signature) {
        (Some(bpm), None) => format!("Tempo {} BPM at {at}", trimmed(bpm)),
        (None, Some((n, d))) => format!("Time signature {n}/{d} at {at}"),
        (Some(bpm), Some((n, d))) => format!("Tempo {} BPM and time signature {n}/{d} at {at}", trimmed(bpm)),
        (None, None) => format!("Tempo marker at {at}"),
    }
}

pub fn marker_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let plan = marker_plan(ctx, input)?;
    let now = marker_state(marker_at(ctx, plan.time).as_ref().map(|(_, m)| m));
    Ok(Preview {
        target: json!({"ref": null, "kind": "tempo-marker", "time": plan.time}),
        prior: now.clone(),
        fence: now.clone(),
        display: json!({"title": marker_title(ctx, &plan), "before": now}),
    })
}

pub fn marker_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = marker_plan(ctx, input)?;
    let found = marker_at(ctx, plan.time);
    let now = marker_state(found.as_ref().map(|(_, m)| m));
    if !same_marker(&now, fence) {
        return Err(changed("the tempo markers there changed since the preview"));
    }
    let reaper = ctx.reaper;
    let in_force = reaper.time_signature_at(ctx.proj, plan.time).map(|s| s.bpm).or_else(|| reaper.tempo()).unwrap_or(120.0);
    let bpm = plan.bpm.unwrap_or_else(|| found.as_ref().map_or(in_force, |(_, m)| m.bpm));
    // A marker with no time signature of its own passes 0 and 0: the one in force stays.
    let (num, den) = plan.signature.unwrap_or_else(|| found.as_ref().map_or((0, 0), |(_, m)| (m.num.max(0), m.denom.max(0))));
    let linear = plan.linear.unwrap_or_else(|| found.as_ref().is_some_and(|(_, m)| m.linear));
    let index = found.as_ref().map_or(-1, |(i, _)| *i);
    let count_before = reaper.tempo_marker_count(ctx.proj);
    if !reaper.set_tempo_marker(ctx.proj, index, plan.time, bpm, num, den, linear) {
        return Err(failed("REAPER wouldn't set the tempo marker"));
    }
    let after = marker_at(ctx, plan.time).ok_or_else(|| failed("the tempo marker isn't there after it was set"))?.1;
    // The first marker in a project brings one at the start with it, to hold the tempo and time signature that were there.
    let base = (count_before == 0 && plan.time > 1e-6).then(|| marker_at(ctx, 0.0)).flatten().map(|(_, m)| marker_state(Some(&m)));
    if plan.bpm.is_some_and(|wanted| !close(after.bpm, wanted)) || plan.signature.is_some_and(|(n, d)| (after.num, after.denom) != (n, d)) {
        return Err(failed("REAPER kept the tempo marker as it was"));
    }
    let mut applied = marker_state(Some(&after));
    if let Some(base) = base {
        applied["base"] = base;
    }
    // Where it is in beats: in beats timebase an earlier tempo change moves it in seconds, and its undo finds it there.
    if let Some(qn) = reaper.quarter_notes_at(ctx.proj, plan.time) {
        applied["qn"] = qn.into();
    }
    Ok(Applied { applied, created: None, material: None, display: json!({"title": marker_title(ctx, &plan)}), ..Default::default() })
}

/// Kumi's marker for an undo: at its time, else where its quarter note is now when the marker there is as Kumi left it
/// (an earlier tempo change moved it in seconds, in beats timebase). `changed` when it is in neither place but a marker
/// as Kumi left it is elsewhere: the producer moved it, and the undo can't tell it apart, discard or not.
fn kumis_marker(ctx: &Ctx, restore: &Restore, time: f64) -> Done<(f64, Option<(i32, TempoMarker)>)> {
    if let Some(found) = marker_at(ctx, time) {
        return Ok((time, Some(found)));
    }
    if restore.applied["exists"] != true {
        return Ok((time, None));
    }
    if let Some(moved) = restore.applied["qn"].as_f64().and_then(|qn| ctx.reaper.time_at_quarter_notes(ctx.proj, qn)) {
        if let Some(found) = marker_at(ctx, moved).filter(|(_, m)| same_marker(&marker_state(Some(m)), restore.applied)) {
            return Ok((moved, Some(found)));
        }
    }
    let elsewhere = (0..ctx.reaper.tempo_marker_count(ctx.proj))
        .filter_map(|i| ctx.reaper.tempo_marker(ctx.proj, i))
        .any(|m| same_marker(&marker_state(Some(&m)), restore.applied));
    if elsewhere {
        return Err(changed(format!(
            "the tempo marker Kumi set at {} isn't there any more (moved or deleted in REAPER since), and one like it is \
             elsewhere: Kumi can't tell whether that one is it, so it leaves the tempo map as it is, even with discard",
            bar_text(ctx.reaper, ctx.proj, time)
        )));
    }
    Ok((time, None))
}

pub fn marker_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let time = restore.target["time"].as_f64().ok_or_else(|| bad("target: the marker's time"))?;
    let (time, found) = kumis_marker(ctx, restore, time)?;
    let now = marker_state(found.as_ref().map(|(_, m)| m));
    let title = format!("Tempo marker at {} put back", bar_text(ctx.reaper, ctx.proj, time));
    if same_marker(&now, restore.prior) {
        return Ok(Restored { restored: now, display: json!({"title": title}), unwritten: true });
    }
    if !restore.discard && !same_marker(&now, restore.applied) {
        return Err(changed("the tempo markers there have been changed in REAPER since Kumi set them"));
    }
    let reaper = ctx.reaper;
    if restore.prior["exists"] == true {
        let index = found.as_ref().map_or(-1, |(i, _)| *i);
        if !put_marker(ctx, index, time, restore.prior) {
            return Err(failed("REAPER wouldn't put the tempo marker back"));
        }
    } else if let Some((index, _)) = found {
        if !reaper.delete_tempo_marker(ctx.proj, index) {
            return Err(failed("REAPER wouldn't take the tempo marker away again"));
        }
        // The one REAPER made at the start with it goes too, if it is as it made it and the only one left.
        if let Some(base) = restore.applied.get("base").filter(|b| b["exists"] == true) {
            let left = reaper.tempo_marker_count(ctx.proj);
            if let Some((start, marker)) = marker_at(ctx, 0.0).filter(|_| left == 1) {
                if same_marker(&marker_state(Some(&marker)), base) && !reaper.delete_tempo_marker(ctx.proj, start) {
                    return Err(failed("REAPER wouldn't take the tempo marker at the start away again"));
                }
            }
        }
    }
    let after = marker_state(marker_at(ctx, time).as_ref().map(|(_, m)| m));
    if !same_marker(&after, restore.prior) {
        return Err(failed("REAPER kept the tempo marker as it was"));
    }
    Ok(Restored { restored: after, display: json!({"title": title}), ..Default::default() })
}

/// A marker made, or written over, to hold what `state` says.
fn put_marker(ctx: &Ctx, index: i32, time: f64, state: &Value) -> bool {
    let (num, den) = (state["num"].as_i64().unwrap_or(0) as i32, state["den"].as_i64().unwrap_or(0) as i32);
    ctx.reaper.set_tempo_marker(ctx.proj, index, time, number(state, "bpm"), num, den, state["linear"] == true)
}

fn marker_delete_time(ctx: &Ctx, input: &Value) -> Done<f64> {
    moment(ctx, input, "positionBeats", "position")?.ok_or_else(|| bad("pass positionBeats or position: where the marker is"))
}

pub fn marker_delete_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let time = marker_delete_time(ctx, input)?;
    let (_, marker) = marker_at(ctx, time).ok_or_else(|| failed("there is no tempo marker there"))?;
    let now = marker_state(Some(&marker));
    Ok(Preview {
        target: json!({"ref": null, "kind": "tempo-marker", "time": marker.time}),
        prior: now.clone(),
        fence: now,
        display: json!({"title": format!("Deleted tempo marker at {}", bar_text(ctx.reaper, ctx.proj, marker.time))}),
    })
}

pub fn marker_delete_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let time = marker_delete_time(ctx, input)?;
    let (index, marker) = marker_at(ctx, time).ok_or_else(|| changed("the tempo marker went since the preview"))?;
    let now = marker_state(Some(&marker));
    if !same_marker(&now, fence) {
        return Err(changed("the tempo marker changed since the preview"));
    }
    let count = ctx.reaper.tempo_marker_count(ctx.proj);
    if !ctx.reaper.delete_tempo_marker(ctx.proj, index) || ctx.reaper.tempo_marker_count(ctx.proj) >= count {
        return Err(failed("REAPER wouldn't delete the tempo marker"));
    }
    // The tempo map as the delete left it: changed since, the marker made again at its old time would sit at another bar.
    let tempo = super::marks::tempo_print(ctx.reaper, ctx.proj);
    Ok(Applied {
        applied: json!({"deleted": true}),
        created: None,
        material: Some(json!({"time": marker.time, "state": now, "tempo": tempo})),
        display: json!({"title": format!("Deleted tempo marker at {}", bar_text(ctx.reaper, ctx.proj, marker.time))}),
        ..Default::default()
    })
}

pub fn marker_delete_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let time = material["time"].as_f64().ok_or_else(|| bad("material: the marker's time"))?;
    let saved = &material["state"];
    let title = format!("Tempo marker at {} put back", bar_text(ctx.reaper, ctx.proj, time));
    if let Some((_, existing)) = marker_at(ctx, time) {
        let now = marker_state(Some(&existing));
        // The one Kumi deleted is back (a retry): nothing is made twice. Another one there is the producer's, which an undo
        // never writes over, even one told to put things back anyway.
        return if same_marker(&now, saved) {
            Ok(Restored { restored: now, display: json!({"title": title}), unwritten: true })
        } else {
            Err(changed(format!(
                "a different tempo marker is at {} now (not the one Kumi deleted): nothing was written",
                bar_text(ctx.reaper, ctx.proj, time)
            )))
        };
    }
    super::marks::tempo_unmoved(ctx, material, restore.discard, "the tempo marker")?;
    if !put_marker(ctx, -1, time, saved) {
        return Err(failed("REAPER wouldn't make the tempo marker again"));
    }
    let after = marker_state(marker_at(ctx, time).as_ref().map(|(_, m)| m));
    if !same_marker(&after, saved) {
        return Err(failed("REAPER made the tempo marker differently"));
    }
    Ok(Restored { restored: after, display: json!({"title": title}), ..Default::default() })
}

// ----- the loop -----

fn loop_state(ctx: &Ctx) -> Value {
    let (start, end) = ctx.reaper.loop_points(ctx.proj);
    json!({"start": start, "end": end})
}

fn same_loop(a: &Value, b: &Value) -> bool {
    close(number(a, "start"), number(b, "start")) && close(number(a, "end"), number(b, "end"))
}

fn loop_wanted(ctx: &Ctx, input: &Value) -> Done<Value> {
    let start = moment(ctx, input, "startBeats", "start")?
        .ok_or_else(|| bad("pass startBeats (quarter notes from the start) or start (seconds)"))?;
    let end = moment(ctx, input, "endBeats", "end")?.ok_or_else(|| bad("pass endBeats or end"))?;
    if end <= start && !(start == 0.0 && end == 0.0) {
        return Err(bad("the loop must end after it starts (0 and 0 clears it)"));
    }
    Ok(json!({"start": start, "end": end}))
}

fn loop_title(ctx: &Ctx, state: &Value) -> String {
    let (start, end) = (number(state, "start"), number(state, "end"));
    if start == 0.0 && end == 0.0 {
        return "Loop cleared".into();
    }
    // (m6-places) Whole bars as a producer counts them: from bar 9's start to bar 17's is bars 9–16.
    let bar = |time: f64| ctx.reaper.bar_and_beat_at(ctx.proj, time).filter(|(_, beat)| beat.abs() < 0.005).map(|(measure, _)| measure + 1);
    match (bar(start), bar(end)) {
        (Some(first), Some(after)) if after == first + 1 => format!("Loop bar {first}"),
        (Some(first), Some(after)) if after > first + 1 => format!("Loop bars {first}–{}", after - 1),
        _ => format!("Loop {} to {}", bar_text(ctx.reaper, ctx.proj, start), bar_text(ctx.reaper, ctx.proj, end)),
    }
}

fn put_loop(ctx: &Ctx, state: &Value) -> Done<Value> {
    if !ctx.reaper.set_loop_points(ctx.proj, number(state, "start"), number(state, "end")) {
        return Err(failed("REAPER wouldn't set the loop"));
    }
    let after = loop_state(ctx);
    if same_loop(&after, state) {
        Ok(after)
    } else {
        Err(failed("REAPER kept the loop as it was"))
    }
}

pub fn loop_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = loop_wanted(ctx, input)?;
    let now = loop_state(ctx);
    let mut display = json!({"title": loop_title(ctx, &wanted), "before": now, "after": wanted});
    if same_loop(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn loop_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = loop_wanted(ctx, input)?;
    let now = loop_state(ctx);
    if !same_loop(&now, fence) {
        return Err(changed("the loop changed since the preview"));
    }
    let after = if same_loop(&now, &wanted) { now.clone() } else { put_loop(ctx, &wanted)? };
    Ok(Applied {
        applied: after.clone(),
        created: None,
        material: None,
        display: json!({"title": loop_title(ctx, &after), "before": now, "after": after}),
        unwritten: same_loop(&now, &wanted),
    })
}

pub fn loop_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let now = loop_state(ctx);
    let already = same_loop(&now, restore.prior);
    let after = if already {
        now
    } else if restore.discard || same_loop(&now, restore.applied) {
        put_loop(ctx, restore.prior)?
    } else {
        return Err(changed("the loop has been changed in REAPER since Kumi set it"));
    };
    Ok(Restored {
        restored: after.clone(),
        display: json!({"title": format!("{} (put back)", loop_title(ctx, &after))}),
        unwritten: already,
    })
}

// ----- repeat -----

fn wanted_repeat(input: &Value) -> Done<bool> {
    input.get("on").and_then(Value::as_bool).ok_or_else(|| bad("on must be true or false"))
}

pub fn repeat_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let on = wanted_repeat(input)?;
    let now = json!({"on": ctx.reaper.repeat(ctx.proj)});
    let mut display = json!({"title": format!("Repeat {}", on_off(on)), "before": now, "after": {"on": on}});
    if now["on"] == on {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn repeat_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let on = wanted_repeat(input)?;
    let now = ctx.reaper.repeat(ctx.proj);
    if now != (fence["on"] == true) {
        return Err(changed("repeat changed since the preview"));
    }
    if now != on && !ctx.reaper.set_repeat(ctx.proj, on) {
        return Err(failed("REAPER wouldn't set repeat"));
    }
    if ctx.reaper.repeat(ctx.proj) != on {
        return Err(failed("REAPER kept repeat as it was"));
    }
    Ok(Applied {
        applied: json!({"on": on}),
        created: None,
        material: None,
        display: json!({"title": format!("Repeat {}", on_off(on))}),
        unwritten: now == on,
    })
}

pub fn repeat_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (prior, applied) = (restore.prior["on"] == true, restore.applied["on"] == true);
    let now = ctx.reaper.repeat(ctx.proj);
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed("repeat has been changed in REAPER since Kumi set it"));
        }
        if !ctx.reaper.set_repeat(ctx.proj, prior) || ctx.reaper.repeat(ctx.proj) != prior {
            return Err(failed("REAPER kept repeat as it was"));
        }
    }
    Ok(Restored {
        restored: json!({"on": prior}),
        display: json!({"title": format!("Repeat back {}", on_off(prior))}),
        unwritten: now == prior,
    })
}

// ----- the metronome (m6-places) -----

/// The project's metronome options (`projmetroen`): 1 is the metronome on; the other bits are its count-in and recording
/// options, which a switch leaves as they are.
fn metronome_flags(ctx: &Ctx) -> Done<i64> {
    ctx.reaper
        .config_text("projmetroen")
        .and_then(|text| text.trim().parse::<i64>().ok())
        .ok_or_else(|| failed("this REAPER can't say whether its metronome is on"))
}

fn metronome_on(ctx: &Ctx) -> Done<bool> {
    Ok(metronome_flags(ctx)? & 1 != 0)
}

/// Switches the metronome, then reads it back: REAPER keeps it in the project's settings, outside its undo history.
fn set_metronome(ctx: &Ctx, on: bool) -> Done<()> {
    let flags = metronome_flags(ctx)?;
    let wanted = if on { flags | 1 } else { flags & !1 };
    if ctx.reaper.set_config_text("projmetroen", &wanted.to_string()) == 0 {
        return Err(failed("REAPER wouldn't switch its metronome"));
    }
    if metronome_on(ctx)? != on {
        return Err(failed("REAPER kept its metronome as it was"));
    }
    Ok(())
}

pub fn metronome_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let on = wanted_repeat(input)?;
    let now = json!({"on": metronome_on(ctx)?});
    let mut display = json!({"title": format!("Metronome {}", on_off(on)), "before": now, "after": {"on": on}});
    if now["on"] == on {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn metronome_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let on = wanted_repeat(input)?;
    let now = metronome_on(ctx)?;
    if now != (fence["on"] == true) {
        return Err(changed("the metronome was switched since the preview"));
    }
    if now != on {
        set_metronome(ctx, on)?;
    }
    Ok(Applied {
        applied: json!({"on": on}),
        created: None,
        material: None,
        display: json!({"title": format!("Metronome {}", on_off(on))}),
        unwritten: now == on,
    })
}

pub fn metronome_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (prior, applied) = (restore.prior["on"] == true, restore.applied["on"] == true);
    let now = metronome_on(ctx)?;
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed("the metronome has been switched in REAPER since Kumi switched it"));
        }
        set_metronome(ctx, prior)?;
    }
    Ok(Restored {
        restored: json!({"on": prior}),
        display: json!({"title": format!("Metronome back {}", on_off(prior))}),
        unwritten: now == prior,
    })
}
