//! An FX's pins: which of its track's channels each input pin takes from and each output pin gives to. A sidechain's
//! channels (a compressor's inputs 3 and 4) are met here. Pins and channels count from 1, as REAPER's pin connector shows
//! them; the mapping is kept whole (every channel a pin has) and put back whole.
//!
//! REAPER doesn't notice a pin mapping written through its API (lab 7.82: a block with only that write makes no undo
//! point, and REAPER's undo of the step before it takes that step instead): each write ends with the FX's wet nudged
//! there and back (`mark_fx`), and REAPER then keeps a point whose undo and redo bring the pins back exactly.

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::Fx;
use crate::refs::Ref;

/// The FX an fx-pins change names, in any chain (a track's, its input chain, a take's, a container's).
fn fx_of(ctx: &Ctx, input: &Value) -> Done<(Fx, String)> {
    let reference = input.get("ref").and_then(Value::as_str).ok_or_else(|| bad("ref: the FX"))?;
    let Ref::Fx(guid) = Ref::parse(reference).map_err(bad)? else { return Err(bad("ref must be an FX ref (r:fx:…)")) };
    let at = super::fxchain::locate_guid(ctx, &guid)?;
    let name = format!("{} {}", super::quoted(&super::fx::fx_label(ctx, at.fx)), super::fx::place_phrase(ctx, at.holder));
    Ok((at.fx, name))
}

/// The pins asked for: (output, pin from 0, channels as bits), each pin once.
fn wanted(ctx: &Ctx, fx: Fx, input: &Value) -> Done<Vec<(bool, i32, u64)>> {
    let (ins, outs) = ctx.reaper.fx_io(fx).ok_or_else(|| failed("REAPER doesn't say how many pins this FX has"))?;
    let list = input
        .get("pins")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| bad("pins: a list of {pin, output?, channels}"))?;
    let mut out: Vec<(bool, i32, u64)> = Vec::new();
    for entry in list {
        let output = match entry.get("output") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(bad("pins[].output must be true or false")),
        };
        let most = if output { outs } else { ins };
        let pin = entry
            .get("pin")
            .and_then(Value::as_i64)
            .filter(|p| *p >= 1 && *p <= most as i64)
            .ok_or_else(|| bad(format!("pins[].pin is a {} pin from 1 to {most}", if output { "output" } else { "input" })))?;
        let channels = entry
            .get("channels")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("pins[].channels: a list of channels (from 1 to 64), or []"))?;
        let mut mask = 0u64;
        for channel in channels {
            let c = channel.as_i64().filter(|c| (1..=64).contains(c)).ok_or_else(|| bad("pins[].channels are channels from 1 to 64"))?;
            mask |= 1 << (c - 1);
        }
        let pin = (pin - 1) as i32;
        if out.iter().any(|(o, p, _)| *o == output && *p == pin) {
            return Err(bad(format!("{} pin {} is given twice", if output { "output" } else { "input" }, pin + 1)));
        }
        out.push((output, pin, mask));
    }
    Ok(out)
}

fn read(ctx: &Ctx, fx: Fx, pins: &[(bool, i32, u64)]) -> Done<Value> {
    pins.iter()
        .map(|(output, pin, _)| {
            let mask = ctx.reaper.fx_pin(fx, *output, *pin).ok_or_else(|| failed("REAPER doesn't say how this FX's pins are mapped"))?;
            Ok(state(*output, *pin, mask))
        })
        .collect::<Done<Vec<Value>>>()
        .map(Value::Array)
}

fn state(output: bool, pin: i32, mask: u64) -> Value {
    let channels: Vec<i64> = (0..64).filter(|b| mask & (1 << b) != 0).map(|b| b + 1).collect();
    json!({"pin": pin + 1, "output": output, "channels": channels, "mask": mask.to_string()})
}

fn mask_of(state: &Value) -> Option<u64> {
    state["mask"].as_str()?.parse().ok()
}

fn title(name: &str, pins: &[(bool, i32, u64)]) -> String {
    let parts: Vec<String> = pins
        .iter()
        .map(|(output, pin, mask)| {
            let channels: Vec<String> = (0..64).filter(|b| mask & (1 << b) != 0).map(|b| (b + 1).to_string()).collect();
            let channels = if channels.is_empty() { "none".to_string() } else { channels.join("+") };
            if *output {
                format!("out {} \u{2192} {channels}", pin + 1)
            } else {
                format!("in {} \u{2190} {channels}", pin + 1)
            }
        })
        .collect();
    format!("Pins of {name}: {}", parts.join(", "))
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let (fx, name) = fx_of(ctx, input)?;
    let pins = wanted(ctx, fx, input)?;
    let now = read(ctx, fx, &pins)?;
    let reference = input["ref"].as_str().unwrap_or_default();
    let mut display = json!({"title": title(&name, &pins)});
    if now.as_array().into_iter().flatten().zip(&pins).all(|(n, (_, _, mask))| mask_of(n) == Some(*mask)) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": reference, "kind": "fx"}), prior: json!({"pins": now}), fence: json!({"pins": now}), display })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (fx, name) = fx_of(ctx, input)?;
    let pins = wanted(ctx, fx, input)?;
    let now = read(ctx, fx, &pins)?;
    if now != fence["pins"] {
        return Err(("changed".into(), format!("the pins of {name} changed since the preview")));
    }
    let mut wrote = false;
    for (output, pin, mask) in &pins {
        if ctx.reaper.fx_pin(fx, *output, *pin) != Some(*mask) {
            if !ctx.reaper.set_fx_pin(fx, *output, *pin, *mask) {
                // (what was written goes back: nothing half-done stays)
                for prior in now.as_array().into_iter().flatten() {
                    if let (Some(p), Some(m)) = (prior["pin"].as_i64(), mask_of(prior)) {
                        ctx.reaper.set_fx_pin(fx, prior["output"] == true, (p - 1) as i32, m);
                    }
                }
                return Err(failed(format!("REAPER wouldn't map the pins of {name}")));
            }
            wrote = true;
        }
    }
    if wrote {
        super::fxchain::mark_fx(ctx.reaper, fx);
    }
    let after = read(ctx, fx, &pins)?;
    Ok(Applied { applied: json!({"pins": after}), display: json!({"title": title(&name, &pins)}), unwritten: !wrote, ..Default::default() })
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let reference = restore.target["ref"].as_str().unwrap_or_default();
    let (fx, name) = fx_of(ctx, &json!({"ref": reference})).map_err(|e| if e.0 == "failed" { ("gone".to_string(), e.1) } else { e })?;
    let prior: Vec<(bool, i32, u64)> = restore.prior["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["output"] == true, (p["pin"].as_i64()? - 1) as i32, mask_of(p)?)))
        .collect();
    let applied: Vec<u64> = restore.applied["pins"].as_array().into_iter().flatten().filter_map(mask_of).collect();
    if prior.is_empty() || prior.len() != applied.len() {
        return Err(bad("prior and applied: the pins the change mapped"));
    }
    let mut wrote = false;
    for ((output, pin, mask), set) in prior.iter().zip(&applied) {
        let now = ctx.reaper.fx_pin(fx, *output, *pin);
        if now == Some(*mask) {
            continue;
        }
        if !restore.discard && now != Some(*set) {
            return Err(("changed".into(), format!("the pins of {name} were changed in REAPER since Kumi mapped them")));
        }
    }
    for (output, pin, mask) in &prior {
        if ctx.reaper.fx_pin(fx, *output, *pin) != Some(*mask) {
            if !ctx.reaper.set_fx_pin(fx, *output, *pin, *mask) {
                return Err(failed(format!("REAPER wouldn't map the pins of {name} back")));
            }
            wrote = true;
        }
    }
    if wrote {
        super::fxchain::mark_fx(ctx.reaper, fx);
    }
    Ok(Restored {
        restored: json!({"pins": read(ctx, fx, &prior)?}),
        display: json!({"title": format!("{} (put back)", title(&name, &prior))}),
        unwritten: !wrote,
    })
}
