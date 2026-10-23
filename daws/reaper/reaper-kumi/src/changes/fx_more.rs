//! The rest of what an FX takes (M4): offline, its wet and delta controls, a ReaEQ band's type and switch, presets, its
//! state chunk, FX chain files loaded into a chain, and copies of an FX in another chain. Every FX is found by its GUID,
//! in any chain (see `fxchain`).

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use super::fx::{self, fx_label, fx_target, place_phrase, signature, signature_as, target_fx};
use super::fxchain::{self, changed, gone, gone_if_missing, mark_fx, went, ChainFile, FxAt, Holder, QuietAutomation};
use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Fx, FxOwner};
use crate::refs;

/// The most a state chunk (or a preset's prior state) may be for Kumi to keep it for an undo.
const STATE_LIMIT: usize = 4 << 20;
/// The most an FX chain file may be.
const CHAIN_FILE_LIMIT: u64 = 32 << 20;

fn flag(input: &Value, key: &str) -> Done<Option<bool>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("{key} must be true or false"))),
    }
}

fn label(ctx: &Ctx, at: &FxAt) -> String {
    format!("{} {}", super::quoted(&fx_label(ctx, at.fx)), place_phrase(ctx, at.holder))
}

// ----- offline -----

fn offline_title(ctx: &Ctx, at: &FxAt, offline: bool) -> String {
    if offline {
        format!("Took {} offline", label(ctx, at))
    } else {
        format!("Put {} back online", label(ctx, at))
    }
}

pub fn offline_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let offline = flag(input, "offline")?.ok_or_else(|| bad("offline must be true or false"))?;
    let now = json!({"offline": ctx.reaper.fx_offline(at.fx)});
    let mut display = json!({"title": offline_title(ctx, &at, offline)});
    if now["offline"] == offline {
        display["unchanged"] = true.into();
    }
    if offline {
        display["note"] = "An FX offline runs no audio and shows no parameters until it is back online; its state is kept.".into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: now.clone(), fence: now, display })
}

pub fn offline_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let offline = flag(input, "offline")?.ok_or_else(|| bad("offline must be true or false"))?;
    let now = ctx.reaper.fx_offline(at.fx);
    if json!({"offline": now}) != *fence {
        return Err(changed("the FX was taken offline or put online in REAPER since the preview"));
    }
    if now != offline && !ctx.reaper.fx_set_offline(at.fx, offline) {
        return Err(failed("REAPER wouldn't take the FX offline"));
    }
    if ctx.reaper.fx_offline(at.fx) != offline {
        return Err(failed("REAPER kept the FX as it was"));
    }
    Ok(Applied {
        applied: json!({"offline": offline}),
        display: json!({"title": offline_title(ctx, &at, offline)}),
        unwritten: now == offline,
        ..Default::default()
    })
}

pub fn offline_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let (prior, applied) = (restore.prior["offline"] == true, restore.applied["offline"] == true);
    let now = ctx.reaper.fx_offline(at.fx);
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed("the FX was taken offline or put online in REAPER since Kumi did"));
        }
        if !ctx.reaper.fx_set_offline(at.fx, prior) || ctx.reaper.fx_offline(at.fx) != prior {
            return Err(failed("REAPER wouldn't put the FX back as it was"));
        }
    }
    Ok(Restored { restored: json!({"offline": prior}), display: json!({"title": offline_title(ctx, &at, prior)}), unwritten: now == prior })
}

// ----- wet and delta -----

/// The wet and delta controls asked for: each with its parameter's index.
type WetDelta = (Option<(i32, f64)>, Option<(i32, bool)>);

/// The wet and delta controls REAPER gives every FX (`:wet`, `:delta`).
fn wet_params(ctx: &Ctx, at: &FxAt, input: &Value) -> Done<WetDelta> {
    let wet = match input.get("wet") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let wet = value.as_f64().filter(|w| (0.0..=1.0).contains(w)).ok_or_else(|| bad("wet must be from 0 (dry) to 1 (all wet)"))?;
            let param = ctx.reaper.fx_ident(at.fx, ":wet").ok_or_else(|| failed("REAPER gives this FX no wet control"))?;
            Some((param, wet))
        }
    };
    let delta = match flag(input, "delta")? {
        None => None,
        Some(on) => Some((ctx.reaper.fx_ident(at.fx, ":delta").ok_or_else(|| failed("REAPER gives this FX no delta control"))?, on)),
    };
    if wet.is_none() && delta.is_none() {
        return Err(bad("pass wet (0 to 1), delta (true or false), or both"));
    }
    Ok((wet, delta))
}

fn wet_state(ctx: &Ctx, at: &FxAt, wet: Option<i32>, delta: Option<i32>) -> Value {
    let mut state = json!({});
    if let Some(param) = wet {
        let value = ctx.reaper.param_normalized(at.fx, param);
        state["wet"] = value.into();
        state["wetDisplay"] = ctx.reaper.fx_format(at.fx, param, value).unwrap_or_default().into();
    }
    if let Some(param) = delta {
        state["delta"] = (ctx.reaper.param_normalized(at.fx, param) >= 0.5).into();
    }
    state
}

fn same_wet(a: &Value, b: &Value) -> bool {
    let wet = match (a.get("wet").and_then(Value::as_f64), b.get("wet").and_then(Value::as_f64)) {
        (Some(x), Some(y)) => fx::near(x, y),
        (None, None) => true,
        _ => false,
    };
    wet && a.get("delta") == b.get("delta")
}

fn wet_title(ctx: &Ctx, at: &FxAt, state: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(wet) = state.get("wet").and_then(Value::as_f64) {
        parts.push(format!("wet {}%", (wet * 100.0).round() as i64));
    }
    if let Some(delta) = state.get("delta").and_then(Value::as_bool) {
        parts.push(format!("delta {}", if delta { "on" } else { "off" }));
    }
    format!("{} {}", label(ctx, at), parts.join(", "))
}

fn wet_write(ctx: &Ctx, at: &FxAt, wet: Option<i32>, delta: Option<i32>, state: &Value) -> bool {
    let _quiet = QuietAutomation::around(ctx.reaper, at.holder.track(ctx.reaper));
    let mut ok = true;
    if let (Some(param), Some(value)) = (wet, state.get("wet").and_then(Value::as_f64)) {
        ok &= ctx.reaper.fx_set_param(at.fx, param, value);
    }
    if let (Some(param), Some(on)) = (delta, state.get("delta").and_then(Value::as_bool)) {
        ok &= ctx.reaper.fx_set_param(at.fx, param, if on { 1.0 } else { 0.0 });
    }
    ok
}

pub fn wet_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let (wet, delta) = wet_params(ctx, &at, input)?;
    let now = wet_state(ctx, &at, wet.map(|w| w.0), delta.map(|d| d.0));
    let mut wanted = now.clone();
    if let Some((_, value)) = wet {
        wanted["wet"] = value.into();
    }
    if let Some((_, on)) = delta {
        wanted["delta"] = on.into();
    }
    let mut display = json!({"title": wet_title(ctx, &at, &wanted), "before": now});
    if same_wet(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: now.clone(), fence: now, display })
}

pub fn wet_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let (wet, delta) = wet_params(ctx, &at, input)?;
    let (wet_param, delta_param) = (wet.map(|w| w.0), delta.map(|d| d.0));
    let now = wet_state(ctx, &at, wet_param, delta_param);
    if !same_wet(&now, fence) {
        return Err(changed("the FX's wet or delta changed since the preview"));
    }
    let mut wanted = now.clone();
    if let Some((_, value)) = wet {
        wanted["wet"] = value.into();
    }
    if let Some((_, on)) = delta {
        wanted["delta"] = on.into();
    }
    let unwritten = same_wet(&now, &wanted);
    if !unwritten && !wet_write(ctx, &at, wet_param, delta_param, &wanted) {
        return Err(failed("REAPER wouldn't set the FX's wet or delta"));
    }
    let after = wet_state(ctx, &at, wet_param, delta_param);
    if !same_wet(&after, &wanted) {
        return Err(failed("REAPER kept the FX's wet or delta as it was"));
    }
    Ok(Applied { applied: after.clone(), display: json!({"title": wet_title(ctx, &at, &after)}), unwritten, ..Default::default() })
}

pub fn wet_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let (wet, delta) = (
        restore.prior.get("wet").and_then(|_| ctx.reaper.fx_ident(at.fx, ":wet")),
        restore.prior.get("delta").and_then(|_| ctx.reaper.fx_ident(at.fx, ":delta")),
    );
    let now = wet_state(ctx, &at, wet, delta);
    let already = same_wet(&now, restore.prior);
    if !already {
        if !restore.discard && !same_wet(&now, restore.applied) {
            return Err(changed("the FX's wet or delta has been changed in REAPER since Kumi set it"));
        }
        if !wet_write(ctx, &at, wet, delta, restore.prior) || !same_wet(&wet_state(ctx, &at, wet, delta), restore.prior) {
            return Err(failed("REAPER wouldn't put the FX's wet or delta back"));
        }
    }
    let after = wet_state(ctx, &at, wet, delta);
    Ok(Restored {
        restored: after.clone(),
        display: json!({"title": format!("{} (put back)", wet_title(ctx, &at, &after))}),
        unwritten: already,
    })
}

// ----- a ReaEQ band -----

/// ReaEQ's band types, as its `BANDTYPEn` codes them (measured in 7.82).
const BAND_TYPES: [(&str, i64, &str); 11] = [
    ("low-shelf", 0, "low shelf"),
    ("high-shelf", 1, "high shelf"),
    ("band-alt-2", 2, "band (alt 2)"),
    ("low-pass", 3, "low pass"),
    ("high-pass", 4, "high pass"),
    ("all-pass", 5, "all pass"),
    ("notch", 6, "notch"),
    ("band-pass", 7, "band pass"),
    ("band", 8, "band"),
    ("band-alt", 9, "band (alt)"),
    ("parallel-band-pass", 10, "parallel band pass"),
];

fn band_count(ctx: &Ctx, fx: Fx) -> usize {
    (0..64).take_while(|n| ctx.reaper.fx_config(fx, &format!("BANDTYPE{n}")).is_some()).count()
}

fn band_state(ctx: &Ctx, fx: Fx, band: usize, which: (bool, bool)) -> Value {
    let mut state = json!({});
    let read = |key: &str| ctx.reaper.fx_config(fx, &format!("{key}{band}")).and_then(|v| v.trim().parse::<i64>().ok());
    if which.0 {
        state["type"] = read("BANDTYPE").into();
    }
    if which.1 {
        state["enabled"] = read("BANDENABLED").map(|v| v != 0).into();
    }
    state
}

fn band_input(ctx: &Ctx, at: &FxAt, input: &Value) -> Done<(usize, Option<i64>, Option<bool>)> {
    let count = band_count(ctx, at.fx);
    if count == 0 {
        return Err(bad(format!("{} isn't a ReaEQ: fx_eq_band changes ReaEQ's bands", fx_label(ctx, at.fx))));
    }
    let band = input
        .get("band")
        .and_then(Value::as_u64)
        .map(|b| b as usize)
        .filter(|b| *b < count)
        .ok_or_else(|| bad(format!("band must be one of the EQ's bands, from 0 to {}", count - 1)))?;
    let kind = match input.get("type") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(
            BAND_TYPES
                .iter()
                .find(|(key, _, _)| key.eq_ignore_ascii_case(name.trim()))
                .map(|(_, code, _)| *code)
                .ok_or_else(|| bad(format!("type must be one of {}", BAND_TYPES.map(|(key, _, _)| key).join(", "))))?,
        ),
        Some(_) => return Err(bad("type must be a band type's name")),
    };
    let enabled = flag(input, "enabled")?;
    if kind.is_none() && enabled.is_none() {
        return Err(bad("pass type, enabled, or both"));
    }
    Ok((band, kind, enabled))
}

fn band_title(ctx: &Ctx, at: &FxAt, band: usize, state: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(code) = state["type"].as_i64() {
        let name = BAND_TYPES.iter().find(|(_, c, _)| *c == code).map_or("another type", |(_, _, words)| words);
        parts.push(format!("a {name}"));
    }
    if let Some(on) = state["enabled"].as_bool() {
        parts.push(if on { "on".to_string() } else { "off".to_string() });
    }
    format!("{} band {} {}", label(ctx, at), band + 1, parts.join(", "))
}

/// A band's type and switch are ReaEQ's own state, not parameters: REAPER's undo neither notices nor keeps them (lab 7.82:
/// a wet nudge there and back makes no point, one left makes a point whose undo takes back only the nudge). So no point is
/// forced: the change is outside REAPER's undo (`skipped_by_daw_undo`), and Kumi's undo puts the band back.
fn band_write(ctx: &Ctx, fx: Fx, band: usize, state: &Value) {
    // (REAPER answers false for these and takes them all the same: the change reads them back.)
    if let Some(code) = state["type"].as_i64() {
        ctx.reaper.fx_set_config(fx, &format!("BANDTYPE{band}"), &code.to_string());
    }
    if let Some(on) = state["enabled"].as_bool() {
        ctx.reaper.fx_set_config(fx, &format!("BANDENABLED{band}"), if on { "1" } else { "0" });
    }
}

pub fn eq_band_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let (band, kind, enabled) = band_input(ctx, &at, input)?;
    let now = band_state(ctx, at.fx, band, (kind.is_some(), enabled.is_some()));
    let wanted =
        json!({"type": kind.map_or(now["type"].clone(), Value::from), "enabled": enabled.map_or(now["enabled"].clone(), Value::from)});
    let wanted = band_only(&wanted, kind.is_some(), enabled.is_some());
    let mut display = json!({
        "title": band_title(ctx, &at, band, &wanted),
        "note": "REAPER's undo doesn't take a band's type or switch back (they aren't parameters); Kumi's undo does.",
    });
    if now == wanted {
        display["unchanged"] = true.into();
    }
    let mut target = fx_target(ctx, &at);
    target["band"] = band.into();
    Ok(Preview { target, prior: now.clone(), fence: now, display })
}

fn band_only(state: &Value, kind: bool, enabled: bool) -> Value {
    let mut out = json!({});
    if kind {
        out["type"] = state["type"].clone();
    }
    if enabled {
        out["enabled"] = state["enabled"].clone();
    }
    out
}

pub fn eq_band_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let (band, kind, enabled) = band_input(ctx, &at, input)?;
    let which = (kind.is_some(), enabled.is_some());
    let now = band_state(ctx, at.fx, band, which);
    if now != *fence {
        return Err(changed("the EQ band changed since the preview"));
    }
    let mut wanted = now.clone();
    if let Some(code) = kind {
        wanted["type"] = code.into();
    }
    if let Some(on) = enabled {
        wanted["enabled"] = on.into();
    }
    let unwritten = wanted == now;
    if !unwritten {
        band_write(ctx, at.fx, band, &wanted);
    }
    let after = band_state(ctx, at.fx, band, which);
    if after != wanted {
        // (what was written goes back: nothing half-done stays)
        band_write(ctx, at.fx, band, &now);
        return Err(failed("ReaEQ didn't take that band change; it is put back"));
    }
    // (REAPER's undo state doesn't hold a band's type or switch, lab 7.82)
    ctx.unkept.set(true);
    Ok(Applied { applied: after.clone(), display: json!({"title": band_title(ctx, &at, band, &after)}), unwritten, ..Default::default() })
}

pub fn eq_band_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let band = restore.target["band"].as_u64().ok_or_else(|| bad("target.band"))? as usize;
    let which = (restore.prior.get("type").is_some(), restore.prior.get("enabled").is_some());
    let now = band_state(ctx, at.fx, band, which);
    let already = now == *restore.prior;
    if !already {
        if !restore.discard && now != *restore.applied {
            return Err(changed("the EQ band has been changed in REAPER since Kumi changed it"));
        }
        band_write(ctx, at.fx, band, restore.prior);
        if band_state(ctx, at.fx, band, which) != *restore.prior {
            return Err(failed("ReaEQ wouldn't put the band back"));
        }
    }
    Ok(Restored {
        restored: restore.prior.clone(),
        display: json!({"title": format!("{} (put back)", band_title(ctx, &at, band, restore.prior))}),
        unwritten: already,
    })
}

// ----- presets -----

/// The values of an FX's parameters, as an undo of a preset puts them back: each one, and a hash of them (a meter that
/// moves by itself is left out of the hash, as the FX's signature leaves it).
fn values(ctx: &Ctx, fx: Fx, volatile: &[i64]) -> (Vec<f64>, String) {
    let count = ctx.reaper.fx_param_count(fx).min(4096);
    let list: Vec<f64> = (0..count).map(|p| ctx.reaper.param_normalized(fx, p)).collect();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (p, value) in list.iter().enumerate().filter(|(p, _)| !volatile.contains(&(*p as i64))) {
        let _ = p;
        for byte in (*value as f32).to_bits().to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    (list, format!("{hash:016x}-{count}"))
}

/// The state chunk of a plug-in that has one (a VST's `vst_chunk`, a CLAP's `clap_chunk`): its key and text.
fn state_chunk(ctx: &Ctx, fx: Fx) -> Option<(&'static str, String)> {
    ["vst_chunk", "clap_chunk"]
        .into_iter()
        .find_map(|key| ctx.reaper.fx_config_big(fx, key).filter(|text| !text.is_empty()).map(|text| (key, text)))
}

fn preset_state(ctx: &Ctx, at: &FxAt, volatile: &[i64], keep: bool) -> Done<Value> {
    let (index, count) = ctx.reaper.fx_preset_index(at.fx).unwrap_or((-1, 0));
    let (list, hash) = values(ctx, at.fx, volatile);
    let mut state = json!({"preset": ctx.reaper.fx_preset(at.fx), "index": index, "count": count, "hash": hash, "volatile": volatile});
    if keep {
        state["params"] = list.into();
        if let Some((key, text)) = state_chunk(ctx, at.fx) {
            if text.len() > STATE_LIMIT {
                return Err(failed(format!("the plug-in's state is {} MB: Kumi won't change what it can't put back", text.len() >> 20)));
            }
            state["stateKey"] = key.into();
            state["state"] = text.into();
        }
    }
    Ok(state)
}

fn volatile_of(recorded: &Value) -> Vec<i64> {
    recorded["volatile"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect()
}

/// What a preset request asks for: a name, a place in the plug-in's list, or a step to the next or previous.
enum PresetAsk {
    Name(String),
    Index(i32),
    Step(i32),
}

fn preset_ask(input: &Value) -> Done<PresetAsk> {
    let name = input.get("preset").filter(|v| !v.is_null());
    let index = input.get("index").filter(|v| !v.is_null());
    let step = input.get("step").filter(|v| !v.is_null());
    match (name, index, step) {
        (Some(Value::String(name)), None, None) if !name.trim().is_empty() => Ok(PresetAsk::Name(name.trim().to_string())),
        (None, Some(index), None) => {
            index.as_u64().map(|i| PresetAsk::Index(i as i32)).ok_or_else(|| bad("index must be a preset's place, from 0"))
        }
        (None, None, Some(step)) => match step.as_i64() {
            Some(1) => Ok(PresetAsk::Step(1)),
            Some(-1) => Ok(PresetAsk::Step(-1)),
            _ => Err(bad("step must be 1 (the next preset) or -1 (the previous one)")),
        },
        _ => Err(bad("pass one of preset (its name), index (its place in the plug-in's list) or step (1 or -1)")),
    }
}

fn preset_title(ctx: &Ctx, at: &FxAt, ask: Option<&PresetAsk>, loaded: Option<&str>) -> String {
    let what = match (loaded, ask) {
        (Some(name), _) if !name.is_empty() => format!("preset {}", super::quoted(name)),
        (_, Some(PresetAsk::Name(name))) => format!("preset {}", super::quoted(name)),
        (_, Some(PresetAsk::Index(i))) => format!("preset {}", i + 1),
        (_, Some(PresetAsk::Step(1))) => "the next preset".into(),
        (_, Some(_)) => "the previous preset".into(),
        _ => "its preset".into(),
    };
    format!("{} {what}", label(ctx, at))
}

pub fn preset_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let ask = preset_ask(input)?;
    let volatile: Vec<i64> = signature(ctx, at.fx)["volatile"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    let now = preset_state(ctx, &at, &volatile, true)?;
    if let PresetAsk::Index(i) = ask {
        let count = now["count"].as_i64().unwrap_or(0);
        if i as i64 >= count {
            return Err(bad(format!("that plug-in has {count} presets (index from 0)")));
        }
    }
    let fence = json!({"preset": now["preset"], "hash": now["hash"], "volatile": volatile});
    let mut display = json!({"title": preset_title(ctx, &at, Some(&ask), None)});
    if matches!(&ask, PresetAsk::Name(name) if *name == now["preset"].as_str().unwrap_or_default()) {
        display["note"] = "That preset is loaded already: loading it again puts back its stored values.".into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: now, fence, display })
}

pub fn preset_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let ask = preset_ask(input)?;
    let volatile = volatile_of(fence);
    let now = preset_state(ctx, &at, &volatile, true)?;
    if now["preset"] != fence["preset"] || now["hash"] != fence["hash"] {
        return Err(changed("the FX's preset or values changed since the preview"));
    }
    let loaded = {
        let _quiet = QuietAutomation::around(ctx.reaper, at.holder.track(ctx.reaper));
        match &ask {
            PresetAsk::Name(name) => ctx.reaper.fx_set_preset(at.fx, name),
            PresetAsk::Index(i) => ctx.reaper.fx_set_preset_index(at.fx, *i),
            PresetAsk::Step(step) => ctx.reaper.fx_step_preset(at.fx, *step),
        }
    };
    let after = preset_state(ctx, &at, &volatile, false)?;
    if !loaded {
        if after["hash"] != now["hash"] {
            // REAPER said no and changed something: what it changed goes back.
            put_back_preset(ctx, &at, &now);
        }
        return Err(failed(match ask {
            PresetAsk::Name(name) => {
                format!("{} has no preset named \"{name}\": read the FX for its presets' count, or step through them", fx_label(ctx, at.fx))
            }
            _ => "REAPER wouldn't load that preset".into(),
        }));
    }
    let name = after["preset"].as_str().unwrap_or_default().to_string();
    Ok(Applied {
        applied: json!({"preset": after["preset"], "index": after["index"], "hash": after["hash"]}),
        display: json!({"title": preset_title(ctx, &at, Some(&ask), Some(&name))}),
        ..Default::default()
    })
}

/// Puts an FX back to a state a preset change took it from: the preset loaded again (its name), then the exact values (its
/// state chunk, or each parameter). A chunk clears the preset's name in REAPER (measured), so the name stays only where
/// the preset alone gives the values back.
fn put_back_preset(ctx: &Ctx, at: &FxAt, prior: &Value) -> Option<String> {
    let reaper = ctx.reaper;
    let _quiet = QuietAutomation::around(reaper, at.holder.track(reaper));
    let name = prior["preset"].as_str().unwrap_or_default();
    if !name.is_empty() {
        let by_name = reaper.fx_set_preset(at.fx, name);
        if !by_name {
            if let Some(index) = prior["index"].as_i64().filter(|i| *i >= 0) {
                reaper.fx_set_preset_index(at.fx, index as i32);
            }
        }
    } else {
        // No preset was loaded: the plug-in's factory state clears the name, and the values follow.
        reaper.fx_set_preset_index(at.fx, -2);
    }
    let volatile = volatile_of(prior);
    if values(ctx, at.fx, &volatile).1 != prior["hash"].as_str().unwrap_or_default() {
        match (prior["stateKey"].as_str(), prior["state"].as_str()) {
            (Some(key), Some(state)) => {
                reaper.fx_set_config(at.fx, key, state);
                mark_fx(reaper, at.fx);
            }
            _ => {
                for (p, value) in prior["params"].as_array().into_iter().flatten().enumerate() {
                    if let Some(value) = value.as_f64() {
                        if !fx::near(reaper.param_normalized(at.fx, p as i32), value) {
                            reaper.fx_set_param(at.fx, p as i32, value);
                        }
                    }
                }
            }
        }
    }
    let shown = reaper.fx_preset(at.fx);
    (shown != name).then_some(shown)
}

pub fn preset_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let volatile = volatile_of(restore.prior);
    let now = preset_state(ctx, &at, &volatile, false)?;
    let already = now["hash"] == restore.prior["hash"] && now["preset"] == restore.prior["preset"];
    if already {
        return Ok(Restored { restored: now, display: json!({"title": format!("{} is back already", label(ctx, &at))}), unwritten: true });
    }
    if !restore.discard && now["hash"] != restore.applied["hash"] {
        return Err(changed("the FX's values have been changed in REAPER since Kumi loaded the preset"));
    }
    let other_name = put_back_preset(ctx, &at, restore.prior);
    let after = preset_state(ctx, &at, &volatile, false)?;
    if after["hash"] != restore.prior["hash"] {
        return Err(failed("the plug-in didn't take its earlier values back"));
    }
    let mut display = json!({"title": format!("{} back as it was", label(ctx, &at))});
    if let Some(shown) = other_name {
        display["note"] = if shown.is_empty() {
            "Its values are back exactly; REAPER shows no preset name for it now.".into()
        } else {
            format!("Its values are back exactly; REAPER still shows the preset name {}.", super::quoted(&shown)).into()
        };
    }
    Ok(Restored { restored: after, display, unwritten: false })
}

// ----- the state chunk -----

fn state_input(input: &Value) -> Done<(String, Option<&'static str>)> {
    let state = input
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("state: the plug-in's state, base64"))?;
    if state.len() > STATE_LIMIT {
        return Err(bad(format!("state must be at most {} MB", STATE_LIMIT >> 20)));
    }
    if !state.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'\n' | b'\r')) {
        return Err(bad("state must be base64, as the FX's read gives it"));
    }
    let key = match input.get("format").and_then(Value::as_str) {
        None => None,
        Some("vst") => Some("vst_chunk"),
        Some("clap") => Some("clap_chunk"),
        Some(_) => return Err(bad("format must be \"vst\" or \"clap\"")),
    };
    Ok((state.to_string(), key))
}

fn text_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}-{}", text.len())
}

fn current_state(ctx: &Ctx, at: &FxAt, key: Option<&str>) -> Done<(&'static str, String)> {
    let found = match key {
        Some("vst_chunk") => ctx.reaper.fx_config_big(at.fx, "vst_chunk").filter(|t| !t.is_empty()).map(|t| ("vst_chunk", t)),
        Some(_) => ctx.reaper.fx_config_big(at.fx, "clap_chunk").filter(|t| !t.is_empty()).map(|t| ("clap_chunk", t)),
        None => state_chunk(ctx, at.fx),
    };
    found.ok_or_else(|| {
        bad(format!("{} gives REAPER no state chunk: change its parameters (fx_param) or load a preset", fx_label(ctx, at.fx)))
    })
}

pub fn state_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let (wanted, key) = state_input(input)?;
    let (key, now) = current_state(ctx, &at, key)?;
    if now.len() > STATE_LIMIT {
        return Err(failed(format!("the plug-in's state is {} MB: Kumi won't change what it can't put back", now.len() >> 20)));
    }
    let volatile: Vec<i64> = signature(ctx, at.fx)["volatile"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    let prior = json!({"key": key, "state": now, "hash": text_hash(&now), "values": values(ctx, at.fx, &volatile).1, "volatile": volatile});
    let mut display = json!({"title": format!("{} state set ({} bytes)", label(ctx, &at), wanted.len())});
    if text_hash(&wanted) == prior["hash"] {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: fx_target(ctx, &at), fence: json!({"key": key, "hash": prior["hash"]}), prior, display })
}

pub fn state_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let (wanted, _) = state_input(input)?;
    let key = fence["key"].as_str().ok_or_else(|| bad("fence.key"))?;
    let (_, now) = current_state(ctx, &at, Some(key))?;
    if text_hash(&now) != fence["hash"].as_str().unwrap_or_default() {
        return Err(changed("the plug-in's state changed since the preview"));
    }
    let unwritten = now == wanted;
    if !unwritten {
        if !ctx.reaper.fx_set_config(at.fx, key, &wanted) {
            return Err(failed("REAPER wouldn't give the plug-in that state"));
        }
        mark_fx(ctx.reaper, at.fx);
    }
    let (_, after) = current_state(ctx, &at, Some(key))?;
    Ok(Applied {
        applied: json!({"hash": text_hash(&after)}),
        display: json!({"title": format!("{} state set ({} bytes)", label(ctx, &at), after.len())}),
        unwritten,
        ..Default::default()
    })
}

pub fn state_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let key = restore.prior["key"].as_str().ok_or_else(|| bad("prior.key"))?;
    let prior = restore.prior["state"].as_str().ok_or_else(|| bad("prior.state"))?;
    let (_, now) = current_state(ctx, &at, Some(key)).map_err(gone_if_missing)?;
    let already = text_hash(&now) == restore.prior["hash"].as_str().unwrap_or_default();
    if !already {
        if !restore.discard && text_hash(&now) != restore.applied["hash"].as_str().unwrap_or_default() {
            return Err(changed("the plug-in's state has been changed in REAPER since Kumi set it"));
        }
        ctx.reaper.fx_set_config(at.fx, key, prior);
        mark_fx(ctx.reaper, at.fx);
        let (_, after) = current_state(ctx, &at, Some(key))?;
        // A plug-in may write its state back in other words: its values say whether it took it.
        let volatile = volatile_of(restore.prior);
        if after != prior && values(ctx, at.fx, &volatile).1 != restore.prior["values"].as_str().unwrap_or_default() {
            return Err(failed("the plug-in didn't take its earlier state back"));
        }
    }
    Ok(Restored {
        restored: json!({"hash": restore.prior["hash"]}),
        display: json!({"title": format!("{} state put back", label(ctx, &at))}),
        unwritten: already,
    })
}

// ----- FX chain files -----

/// The FX chain file a request names: a name in REAPER's FXChains folder (".RfxChain" added when missing, sub-folders
/// allowed) or an absolute path; its text.
fn chain_file(ctx: &Ctx, input: &Value) -> Done<(String, String)> {
    let file = input
        .get("file")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| bad("file: the FX chain's name"))?;
    let path = Path::new(file);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        if path.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
            return Err(bad("file: a name in REAPER's FXChains folder (sub-folders allowed, no ..), or an absolute path"));
        }
        Path::new(&ctx.reaper.resource_path()).join("FXChains").join(path.components().collect::<PathBuf>())
    };
    let full = with_chain_extension(full);
    let size = std::fs::metadata(&full)
        .map_err(|_| failed(format!("there is no FX chain {} (looked for {})", super::quoted(file), full.display())))?
        .len();
    if size > CHAIN_FILE_LIMIT {
        return Err(failed(format!("that FX chain file is {} MB, past the {} MB Kumi loads", size >> 20, CHAIN_FILE_LIMIT >> 20)));
    }
    let text = std::fs::read_to_string(&full).map_err(|e| failed(format!("couldn't read that FX chain file: {e}")))?;
    let name = full.file_stem().map_or_else(|| file.to_string(), |s| s.to_string_lossy().into_owned());
    Ok((name, text))
}

/// How many FX a chain file adds: its entries at the top level.
fn top_entries(text: &str) -> usize {
    let mut depth = 0i32;
    let mut count = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if depth == 0 && line.starts_with("BYPASS ") {
            count += 1;
        }
        if line.starts_with('<') {
            depth += 1;
        } else if line == ">" {
            depth -= 1;
        }
    }
    count
}

pub fn chain_load_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let holder = fx::add_holder(ctx, input, "fx-chain-load")?;
    let (name, text) = chain_file(ctx, input)?;
    let count = top_entries(&text);
    if count == 0 {
        return Err(failed(format!("{} holds no FX", super::quoted(&name))));
    }
    let before = fxchain::guids(ctx.reaper, holder);
    let place = fx::add_position(input, before.len())?.unwrap_or(before.len());
    fxchain::add_place(ctx.reaper, holder, place)?;
    Ok(Preview {
        target: fxchain::holder_target(ctx, holder),
        prior: json!({"chain": before}),
        // (the file too: one rewritten since the preview isn't what the preview said it loads)
        fence: json!({"chain": before, "file": text_hash(&text)}),
        display: json!({"title": format!("Loaded the FX chain {} into {}", super::quoted(&name), fxchain::holder_label(ctx, holder)), "count": count, "position": place}),
    })
}

pub fn chain_load_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let holder = fx::add_holder(ctx, input, "fx-chain-load").map_err(went)?;
    let (name, text) = chain_file(ctx, input)?;
    let count = top_entries(&text);
    let before = fxchain::guids(ctx.reaper, holder);
    if json!({"chain": before}) != json!({"chain": fence["chain"]}) {
        return Err(changed("the FX in that chain changed since the preview"));
    }
    if fence["file"] != text_hash(&text) || count == 0 {
        return Err(changed(format!("the FX chain file {} changed since the preview", super::quoted(&name))));
    }
    let place = fx::add_position(input, before.len())?.unwrap_or(before.len());
    let (address, rec) = fxchain::add_place(ctx.reaper, holder, place)?;
    // Loaded without its ids: REAPER gives each FX its own (one kept from a file could name an FX elsewhere).
    let file = ChainFile::write(ctx.reaper, &fxchain::without_ids(&text), "load")?;
    ctx.reaper.fx_add(holder.owner(), &file.name, rec, Some(address));
    drop(file);
    let after = fxchain::guids(ctx.reaper, holder);
    let made: Vec<String> = after.iter().filter(|g| fxchain::position_of(&before, g).is_none()).cloned().collect();
    let in_place = after.get(place..place + made.len()).is_some_and(|slice| slice == made.as_slice());
    if made.len() != count || after.len() != before.len() + count || !in_place {
        return Err(fx::take_back_added(ctx, holder, &before, "the chain isn't what it should be after loading the file"));
    }
    let signatures: Vec<Value> =
        made.iter().map(|g| fxchain::adopt(ctx, holder, g).map(|at| fx::deep_signature(ctx, at.fx)).unwrap_or(Value::Null)).collect();
    let fx_refs: Vec<String> = made.iter().map(|g| refs::fx_ref(g)).collect();
    Ok(Applied {
        applied: json!({"guids": made, "index": place, "signatures": signatures}),
        created: Some(json!({"ref": fx_refs[0], "kind": "fx", "fxRefs": fx_refs})),
        display: json!({"title": format!("Loaded the FX chain {} into {} ({} FX)", super::quoted(&name), fxchain::holder_label(ctx, holder), count), "fxRefs": fx_refs}),
        ..Default::default()
    })
}

/// Takes off FX Kumi added (a chain loaded, a copy), refusing when the producer has changed or automated any since, or any
/// FX inside a container of them (the container takes those with it).
fn take_off(ctx: &Ctx, guids: &[String], signatures: &[Value], discard: bool) -> Done<(usize, String)> {
    let found: Vec<(FxAt, &Value)> = guids
        .iter()
        .zip(signatures.iter().chain(std::iter::repeat(&Value::Null)))
        .filter_map(|(guid, sig)| fxchain::locate_guid(ctx, guid).ok().map(|at| (at, sig)))
        .collect();
    if found.is_empty() {
        return Err(gone("the FX Kumi added are gone"));
    }
    if !discard {
        if found.len() != guids.len() {
            return Err(changed("some of the FX Kumi added are gone: the rest stay (discard takes them off too)"));
        }
        for (at, sig) in &found {
            if fxchain::fx_envelopes_deep(ctx.reaper, at) > 0 {
                return Err(changed(format!("{} has automation in it now", super::quoted(&fx_label(ctx, at.fx)))));
            }
            if !sig.is_null() && !fx::deep_as_recorded(ctx, at.fx, sig) {
                return Err(changed(format!(
                    "{} (or an FX inside it) has been changed in REAPER since",
                    super::quoted(&fx_label(ctx, at.fx))
                )));
            }
        }
    }
    let where_ = fxchain::holder_label(ctx, found[0].0.holder);
    let mut deleted = 0;
    for (at, _) in found.iter().rev() {
        let at = fxchain::locate_guid(ctx, &at.guid)?;
        if !ctx.reaper.fx_delete(at.fx) {
            return Err(if deleted == 0 {
                failed("REAPER wouldn't delete the FX")
            } else {
                ("uncertain".into(), "REAPER deleted some of the FX and not the rest: read the chain".into())
            });
        }
        deleted += 1;
    }
    Ok((deleted, where_))
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

pub fn chain_load_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guids = strings(&restore.applied["guids"]);
    let signatures: Vec<Value> = restore.applied["signatures"].as_array().cloned().unwrap_or_default();
    let (deleted, where_) = take_off(ctx, &guids, &signatures, restore.discard)?;
    Ok(Restored {
        restored: json!({"deleted": guids}),
        display: json!({"title": format!("Took the loaded chain's {deleted} FX off {where_}")}),
        ..Default::default()
    })
}

// ----- a copy in another chain -----

fn copy_holder(ctx: &Ctx, input: &Value) -> Done<Holder> {
    let input_chain = flag(input, "input")?.unwrap_or(false);
    fxchain::holder_named(ctx, input.get("toRef").unwrap_or(&Value::Null), input_chain, "toRef")
}

pub fn copy_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let holder = copy_holder(ctx, input)?;
    if let Holder::Container(container) = holder {
        if container == at.fx || is_inside(ctx, container, at.fx) {
            return Err(bad("a container can't be copied into itself"));
        }
    }
    let before = fxchain::guids(ctx.reaper, holder);
    let place = fx::add_position(input, before.len())?.unwrap_or(before.len());
    fxchain::add_place(ctx.reaper, holder, place)?;
    let fence = json!({"chain": before, "source": signature(ctx, at.fx)});
    Ok(Preview {
        target: fxchain::holder_target(ctx, holder),
        prior: json!({"chain": before}),
        fence,
        display: json!({"title": format!("Copied {} to {}", label(ctx, &at), fxchain::holder_label(ctx, holder)), "position": place}),
    })
}

/// Whether `fx` sits (at any depth) inside `container`.
fn is_inside(ctx: &Ctx, container: Fx, fx: Fx) -> bool {
    let mut holder = fxchain::holder_of(ctx.reaper, container);
    loop {
        match holder {
            Holder::Container(parent) if parent == fx => return true,
            Holder::Container(parent) => holder = fxchain::holder_of(ctx.reaper, parent),
            _ => return false,
        }
    }
}

pub fn copy_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let holder = copy_holder(ctx, input).map_err(went)?;
    let before = fxchain::guids(ctx.reaper, holder);
    if before != fx::chain_of_value(fence)? || signature_as(ctx, at.fx, &fence["source"]) != fence["source"] {
        return Err(changed("the FX, or the chain it goes to, changed since the preview"));
    }
    let place = fx::add_position(input, before.len())?.unwrap_or(before.len());
    let made = match holder {
        // REAPER's own copy into a container hung REAPER (lab): the FX's entry goes in as an FX chain file instead.
        Holder::Container(_) => {
            let chunk = fxchain::owner_chunk(ctx.reaper, at.fx.owner)?;
            let entry = fxchain::fx_entry(&chunk, matches!(at.fx.owner, FxOwner::Take(_)), &at.guid)
                .ok_or_else(|| failed("the FX isn't in its owner's state (a plug-in REAPER can't describe)"))?;
            fx::add_entry(ctx, holder, place, &fxchain::without_ids(&entry), &at.guid)?
        }
        _ => {
            let destination = fxchain::move_address(ctx.reaper, holder, place)?;
            if !ctx.reaper.fx_copy(at.fx, holder.owner(), destination, false) {
                return Err(failed("REAPER wouldn't copy the FX"));
            }
            let after = fxchain::guids(ctx.reaper, holder);
            let made = after.get(place).cloned().unwrap_or_default();
            if after.len() != before.len() + 1 || made.is_empty() || fxchain::position_of(&before, &made).is_some() {
                return Err(fx::take_back_added(ctx, holder, &before, "the chain isn't what it should be after the copy"));
            }
            fxchain::adopt(ctx, holder, &made)?
        }
    };
    Ok(Applied {
        applied: json!({"guids": [made.guid], "index": place, "signatures": [fx::deep_signature(ctx, made.fx)]}),
        created: Some(json!({"ref": refs::fx_ref(&made.guid), "kind": "fx"})),
        display: json!({"title": format!("Copied {} to {}", label(ctx, &at), fxchain::holder_label(ctx, holder)), "ref": refs::fx_ref(&made.guid)}),
        ..Default::default()
    })
}

pub fn copy_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guids = strings(&restore.applied["guids"]);
    let signatures: Vec<Value> = restore.applied["signatures"].as_array().cloned().unwrap_or_default();
    let (_, where_) = take_off(ctx, &guids, &signatures, restore.discard)?;
    Ok(Restored {
        restored: json!({"deleted": guids}),
        display: json!({"title": format!("Took the copy off {where_}")}),
        ..Default::default()
    })
}

// ----- FX chain files saved -----

/// A chain file's path with REAPER's extension (added, never put in place of a dot in the name: "Lead 2.1").
fn with_chain_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("rfxchain")) {
        return path;
    }
    let mut text = path.into_os_string();
    text.push(".RfxChain");
    PathBuf::from(text)
}

/// Where a chain is saved: a name in REAPER's FXChains folder, sub-folders allowed.
fn save_path(ctx: &Ctx, params: &Value) -> Done<PathBuf> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| bad("name: the FX chain file's name"))?;
    if name.chars().count() > 256 || name.chars().any(char::is_control) {
        return Err(bad("name must be one line of at most 256 characters"));
    }
    let path = Path::new(name);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(bad("name: a name in REAPER's FXChains folder (sub-folders as \"Vocals/Lead\"; no .., no absolute path)"));
    }
    // (a folder at a time, so the path has the system's separators throughout: "Vocals\\Lead.RfxChain" on Windows)
    Ok(with_chain_extension(Path::new(&ctx.reaper.resource_path()).join("FXChains").join(path.components().collect::<PathBuf>())))
}

/// An FX's entry without its automation (its `PARMENV` blocks): a chain file is the FX, not the project's timeline.
/// Modulation (`PROGRAMENV`) stays.
fn without_automation(entry: &str) -> String {
    let mut kept = Vec::new();
    let mut depth = 0usize;
    for line in entry.lines() {
        if depth > 0 {
            if line.starts_with('<') {
                depth += 1;
            } else if line == ">" {
                depth -= 1;
            }
        } else if line.starts_with("<PARMENV") {
            depth = 1;
        } else {
            kept.push(line);
        }
    }
    kept.join("\n")
}

/// A link from a saved FX's parameter to an FX left out of the file: REAPER keeps a link by the source's place in the
/// chain, so in the file it would follow whatever FX lands there.
fn link_left_out(ctx: &Ctx, chain: &[Fx], chosen: &[usize]) -> Option<String> {
    let reaper = ctx.reaper;
    for &place in chosen {
        let fx = chain[place];
        for p in 0..reaper.fx_param_count(fx).min(4096) {
            if reaper.fx_config(fx, &format!("param.{p}.plink.active")).is_none_or(|v| v.trim() != "1") {
                continue;
            }
            let source = reaper.fx_config(fx, &format!("param.{p}.plink.effect")).and_then(|v| v.trim().parse::<usize>().ok());
            if let Some(source) = source.filter(|s| *s < chain.len() && !chosen.contains(s)) {
                return Some(format!(
                    "{}'s parameters link to {}, which isn't saved with it: save both, or the whole chain",
                    super::quoted(&fx_label(ctx, fx)),
                    super::quoted(&fx_label(ctx, chain[source]))
                ));
            }
        }
    }
    None
}

/// `fx_chain_save {ref, fxRefs?, name, input?, overwrite?}`: a chain's FX (or some of them, in the chain's order) saved
/// as an FX chain file in REAPER's FXChains folder: a file, not the project, so no undo point and no HISTORY row. The
/// file holds no FX ids (each load gives its FX ids of their own) and no automation. A file of that name is replaced
/// only when asked (`overwrite`), and then whole or not at all.
pub fn chain_save(ctx: &Ctx, params: &Value) -> Done<Value> {
    let input_chain = flag(params, "input")?.unwrap_or(false);
    let holder = fxchain::holder_named(ctx, &params["ref"], input_chain, "ref")?;
    let chain = fxchain::members(ctx.reaper, holder);
    let guids: Vec<String> = chain.iter().map(|fx| ctx.reaper.fx_guid(*fx)).collect();
    let chosen: Vec<usize> = match params.get("fxRefs").filter(|v| !v.is_null()) {
        None => (0..chain.len()).collect(),
        Some(Value::Array(list)) if !list.is_empty() => {
            let mut places = Vec::new();
            for item in list {
                let text = item.as_str().ok_or_else(|| bad("fxRefs: FX refs"))?;
                let refs::Ref::Fx(guid) = refs::Ref::parse(text).map_err(bad)? else { return Err(bad("fxRefs: FX refs")) };
                let place = fxchain::position_of(&guids, &guid)
                    .ok_or_else(|| bad(format!("{text} isn't directly in {}", fxchain::holder_label(ctx, holder))))?;
                places.push(place);
            }
            places.sort_unstable();
            places.dedup();
            places
        }
        Some(_) => return Err(bad("fxRefs: a list of FX in that chain")),
    };
    if chosen.is_empty() {
        return Err(failed(format!("{} has no FX to save", fxchain::holder_label(ctx, holder))));
    }
    let overwrite = flag(params, "overwrite")?.unwrap_or(false);
    let path = save_path(ctx, params)?;
    if chosen.len() < chain.len() {
        if let Some(why) = link_left_out(ctx, &chain, &chosen) {
            return Err(bad(why));
        }
    }
    let chunk = fxchain::owner_chunk(ctx.reaper, holder.owner())?;
    let take = matches!(holder.owner(), FxOwner::Take(_));
    let mut text = String::new();
    for (n, place) in chosen.iter().enumerate() {
        let entry = fxchain::fx_entry(&chunk, take, &guids[*place])
            .ok_or_else(|| failed("REAPER doesn't give that FX's state (the master's monitoring FX are kept outside the project)"))?;
        let mut lines: Vec<String> = without_automation(&fxchain::without_ids(&entry)).lines().map(str::to_string).collect();
        // (the first FX of a file runs in parallel with nothing)
        if n == 0 && lines.last().is_some_and(|l| l.starts_with("PARALLEL ")) {
            lines.pop();
        }
        text.push_str(&lines.join("\n"));
        text.push('\n');
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let exists = |path: &Path| fs::symlink_metadata(path).is_ok();
    let existed = exists(&path);
    if existed && !overwrite {
        return Err(failed(format!(
            "there is an FX chain {} already ({}): pass overwrite: true to replace it, or another name",
            super::quoted(&stem),
            path.display()
        )));
    }
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|e| failed(format!("couldn't make the folder for that FX chain: {e}")))?;
    }
    let written = if existed {
        // Beside it first, then over it: a failed write leaves the old file whole.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let temp = path.with_file_name(format!(".kumi-{stamp}.RfxChain"));
        let done = fs::write(&temp, &text).and_then(|_| fs::rename(&temp, &path));
        if done.is_err() {
            let _ = fs::remove_file(&temp);
        }
        done
    } else {
        // (made new or not at all: a file that appeared since is never written over)
        fs::OpenOptions::new().write(true).create_new(true).open(&path).and_then(|mut file| file.write_all(text.as_bytes()))
    };
    written.map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => {
            failed(format!("an FX chain {} appeared meanwhile: nothing was written", super::quoted(&stem)))
        }
        _ => failed(format!("couldn't write that FX chain file: {e}")),
    })?;
    Ok(json!({"file": path.display().to_string(), "name": stem, "count": chosen.len(), "replaced": existed}))
}
