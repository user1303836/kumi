//! An FX parameter's modulation, as REAPER's parameter modulation window sets it (`param.N.mod.*`, `.lfo.*`, `.acs.*`,
//! `.plink.*`, `.learn.*`): one kind per family, changing only the fields given. The prior holds every field the change
//! touched, and what REAPER took is read back (it clamps some: an LFO's phase, the audio control signal's attack).
//!
//! Lab facts (7.82): every key reads false until the parameter has a modulation block; `lfo.active` and `plink.active`
//! make one and turn `mod.active` on, the other keys don't; REAPER follows a link's source FX when either moves. A
//! modulation change that only writes values makes no undo point by itself, so each change nudges the FX's wet control
//! (`fxchain::mark_fx`), and REAPER then keeps it.

use serde_json::{json, Map, Value};

use super::fx::{self, fx_label, place_phrase, ParamAt};
use super::fxchain::{self, changed, mark_fx, went, Holder};
use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::Fx;
use crate::refs::Ref;
use crate::rows;

#[derive(Clone, Copy)]
enum Field {
    Bool,
    Number {
        min: f64,
        max: f64,
    },
    Choice(&'static [(&'static str, i64)]),
    Int {
        min: i64,
        max: i64,
    },
    Text,
    /// A channel counted from 1 (REAPER keeps it from 0; -1 is the track's first pair).
    Channel,
    /// A link's source: another parameter of an FX in the same chain (REAPER keeps the FX's place and the parameter).
    Source,
}

pub struct Family {
    pub kind: &'static str,
    noun: &'static str,
    /// (the model's name, REAPER's key after `param.N.`, what it holds, REAPER's value once the block is made)
    fields: &'static [(&'static str, &'static str, Field, &'static str)],
}

const SHAPES: &[(&str, i64)] = &[("sine", 0), ("square", 1), ("saw-left", 2), ("saw-right", 3), ("triangle", 4), ("random", 5)];
const DIRECTIONS: &[(&str, i64)] = &[("up", 1), ("centered", 0), ("down", -1)];
const MIDI_MESSAGES: &[(&str, i64)] =
    &[("cc", 0xB0), ("note", 0x90), ("pitch", 0xE0), ("aftertouch", 0xD0), ("poly-aftertouch", 0xA0), ("program", 0xC0)];

pub static FAMILIES: [Family; 5] = [
    Family {
        kind: "fx-param-mod",
        noun: "modulation",
        fields: &[
            ("active", "mod.active", Field::Bool, "0"),
            ("baseline", "mod.baseline", Field::Number { min: 0.0, max: 1.0 }, "0"),
            ("visible", "mod.visible", Field::Bool, "0"),
        ],
    },
    Family {
        kind: "fx-param-lfo",
        noun: "LFO",
        fields: &[
            ("active", "lfo.active", Field::Bool, "0"),
            ("shape", "lfo.shape", Field::Choice(SHAPES), "0"),
            ("speed", "lfo.speed", Field::Number { min: 0.0039, max: 8.0 }, "1"),
            ("strength", "lfo.strength", Field::Number { min: 0.0, max: 1.0 }, "1"),
            ("phase", "lfo.phase", Field::Number { min: 0.0, max: 1.0 }, "0"),
            ("tempoSync", "lfo.temposync", Field::Bool, "0"),
            ("direction", "lfo.dir", Field::Choice(DIRECTIONS), "1"),
            ("free", "lfo.free", Field::Bool, "0"),
        ],
    },
    Family {
        kind: "fx-param-acs",
        noun: "audio control signal",
        fields: &[
            ("active", "acs.active", Field::Bool, "0"),
            ("strength", "acs.strength", Field::Number { min: 0.0, max: 1.0 }, "1"),
            ("direction", "acs.dir", Field::Choice(DIRECTIONS), "1"),
            ("attack", "acs.attack", Field::Number { min: 0.0, max: 1000.0 }, "300"),
            ("release", "acs.release", Field::Number { min: 0.0, max: 1000.0 }, "300"),
            ("minDb", "acs.dblo", Field::Number { min: -60.0, max: 0.0 }, "-24"),
            ("maxDb", "acs.dbhi", Field::Number { min: -60.0, max: 12.0 }, "0"),
            ("channel", "acs.chan", Field::Channel, "-1"),
            ("stereo", "acs.stereo", Field::Bool, "0"),
            ("curveX", "acs.x2", Field::Number { min: 0.0, max: 1.0 }, "0.5"),
            ("curveY", "acs.y2", Field::Number { min: 0.0, max: 1.0 }, "0.5"),
        ],
    },
    Family {
        kind: "fx-param-link",
        noun: "link",
        fields: &[
            ("active", "plink.active", Field::Bool, "0"),
            ("sourceRef", "plink.effect", Field::Source, "-1"),
            ("scale", "plink.scale", Field::Number { min: -10.0, max: 10.0 }, "1"),
            ("offset", "plink.offset", Field::Number { min: -1.0, max: 1.0 }, "0"),
            ("midiBus", "plink.midi_bus", Field::Int { min: 0, max: 127 }, "0"),
            ("midiChannel", "plink.midi_chan", Field::Int { min: 0, max: 16 }, "0"),
            ("midiMessage", "plink.midi_msg", Field::Choice(MIDI_MESSAGES), "0"),
            ("midiNumber", "plink.midi_msg2", Field::Int { min: 0, max: 127 }, "0"),
        ],
    },
    Family {
        kind: "fx-param-learn",
        noun: "MIDI learn",
        fields: &[
            ("midiStatus", "learn.midi1", Field::Int { min: 0, max: 255 }, "0"),
            ("midiNumber", "learn.midi2", Field::Int { min: 0, max: 127 }, "0"),
            ("mode", "learn.mode", Field::Int { min: 0, max: 15 }, "0"),
            ("flags", "learn.flags", Field::Int { min: 0, max: 0xFFFF }, "0"),
            ("osc", "learn.osc", Field::Text, ""),
        ],
    },
];

/// One REAPER key a change touches, with the text it writes.
struct Touch {
    key: String,
    write: String,
}

fn key(param: i32, suffix: &str) -> String {
    format!("param.{param}.{suffix}")
}

/// What the model asked for, as the REAPER keys and texts it writes (a link's source writes its FX's place and the
/// parameter; a MIDI link writes the MIDI source's -100).
fn touches(ctx: &Ctx, family: &Family, at: &ParamAt, input: &Value) -> Done<Vec<Touch>> {
    let mut out = Vec::new();
    for (name, suffix, field, _) in family.fields {
        let Some(value) = input.get(*name).filter(|v| !v.is_null()) else { continue };
        let write = match field {
            Field::Bool => match value.as_bool() {
                Some(b) => if b { "1" } else { "0" }.to_string(),
                None => return Err(bad(format!("{name} must be true or false"))),
            },
            Field::Number { min, max } => match value.as_f64().filter(|v| v.is_finite() && (*min..=*max).contains(v)) {
                Some(v) => format!("{v}"),
                None => return Err(bad(format!("{name} must be a number from {min} to {max}"))),
            },
            Field::Choice(choices) => match value.as_str().and_then(|text| choices.iter().find(|(word, _)| *word == text)) {
                Some((_, code)) => code.to_string(),
                None => return Err(bad(format!("{name} must be one of {}", choices.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")))),
            },
            Field::Int { min, max } => match value.as_i64().filter(|v| (*min..=*max).contains(v)) {
                Some(v) => v.to_string(),
                None => return Err(bad(format!("{name} must be a whole number from {min} to {max}"))),
            },
            Field::Text => match value.as_str().filter(|t| t.len() <= 256 && !t.chars().any(char::is_control)) {
                Some(t) => t.to_string(),
                None => return Err(bad(format!("{name} must be one line of text"))),
            },
            Field::Channel => match value.as_i64().filter(|c| (1..=64).contains(c)) {
                Some(c) => (c - 1).to_string(),
                None => return Err(bad(format!("{name} must be the first channel of the pair, from 1 to 64 (3 for 3/4)"))),
            },
            Field::Source => {
                let (place, param) = source_place(ctx, at, value)?;
                out.push(Touch { key: key(at.param, "plink.param"), write: param.to_string() });
                place.to_string()
            }
        };
        out.push(Touch { key: key(at.param, suffix), write });
    }
    if family.kind == "fx-param-link" {
        let midi = ["midiBus", "midiChannel", "midiMessage", "midiNumber"].iter().any(|k| input.get(*k).is_some_and(|v| !v.is_null()));
        if midi && input.get("sourceRef").is_some_and(|v| !v.is_null()) {
            return Err(bad("a link follows either sourceRef (another parameter) or MIDI, not both"));
        }
        if midi {
            out.push(Touch { key: key(at.param, "plink.effect"), write: "-100".into() });
        }
    }
    if out.is_empty() {
        return Err(bad(format!("pass at least one of {}", family.fields.iter().map(|f| f.0).collect::<Vec<_>>().join(", "))));
    }
    // An LFO, a signal or a link does nothing while the parameter's modulation is off: it goes on with them.
    let turns_on = out
        .iter()
        .any(|t| (t.key.ends_with(".lfo.active") || t.key.ends_with(".acs.active") || t.key.ends_with(".plink.active")) && t.write == "1");
    let mod_on = ctx.reaper.fx_config(at.at.fx, &key(at.param, "mod.active")).is_some_and(|v| v.trim() == "1");
    if turns_on && !mod_on && !out.iter().any(|t| t.key.ends_with(".mod.active")) {
        out.insert(0, Touch { key: key(at.param, "mod.active"), write: "1".into() });
    }
    // A baseline alone on a parameter with no modulation isn't kept by REAPER: the modulation goes on with it.
    if family.kind == "fx-param-mod" && !mod_on && !out.iter().any(|t| t.key.ends_with(".mod.active")) {
        out.insert(0, Touch { key: key(at.param, "mod.active"), write: "1".into() });
    }
    Ok(out)
}

/// The source a link's `sourceRef` names: another FX's parameter, in the same chain as the linked FX (REAPER keeps the
/// source by its place in the chain, and follows it as FX move).
fn source_place(ctx: &Ctx, at: &ParamAt, value: &Value) -> Done<(usize, i32)> {
    if matches!(at.at.holder, Holder::Container(_)) {
        return Err(bad("Kumi doesn't link parameters of FX inside a container: REAPER's link addresses there aren't known"));
    }
    let source = fx::locate_param(ctx, &json!({"ref": value}))?;
    if source.at.guid.eq_ignore_ascii_case(&at.at.guid) {
        return Err(bad("sourceRef must be another FX's parameter"));
    }
    let chain = fxchain::guids(ctx.reaper, at.at.holder);
    let place =
        fxchain::position_of(&chain, &source.at.guid).ok_or_else(|| bad("sourceRef must be a parameter of an FX in the same chain"))?;
    Ok((place, source.param))
}

/// The keys a change touches, as they read now (null where the parameter has no modulation block), with a link's source
/// told as its FX's id (a place in the chain moves; the id doesn't).
fn state(ctx: &Ctx, fx: Fx, holder: Holder, keys: &[String]) -> Value {
    let chain = fxchain::guids(ctx.reaper, holder);
    let mut state = Map::new();
    for key in keys {
        let read = ctx.reaper.fx_config(fx, key).map(|v| v.trim().to_string());
        let value = match read {
            Some(text) if key.ends_with(".plink.effect") => match text.parse::<i64>().ok() {
                Some(place) if place >= 0 => chain.get(place as usize).map_or(Value::from(text.clone()), |guid| json!({"fx": guid})),
                _ => Value::from(text),
            },
            Some(text) => Value::from(text),
            None => Value::Null,
        };
        state.insert(key.clone(), value);
    }
    Value::Object(state)
}

/// The text to write to put a key back as `value` reads: a link's source FX by its id, found in the chain now.
fn write_back(ctx: &Ctx, holder: Holder, value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(source) => {
            let guid = source.get("fx")?.as_str()?;
            fxchain::position_of(&fxchain::guids(ctx.reaper, holder), guid).map(|place| place.to_string())
        }
        _ => None,
    }
}

/// What REAPER holds once a modulation block is made, for a key whose prior was "no block".
fn default_of(family: &Family, key: &str) -> &'static str {
    let suffix = key.splitn(3, '.').nth(2).unwrap_or_default();
    FAMILIES
        .iter()
        .chain(std::iter::once(family))
        .flat_map(|f| f.fields.iter())
        .find(|field| field.1 == suffix)
        .map_or("0", |field| field.3)
}

/// Two readings of one key agree: numbers by value (REAPER prints "0.5" for "0.50").
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => {
            x == y || matches!((x.parse::<f64>(), y.parse::<f64>()), (Ok(p), Ok(q)) if (p - q).abs() <= 1e-9 * p.abs().max(1.0))
        }
        _ => a == b,
    }
}

fn same_state(a: &Value, b: &Value) -> bool {
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return false };
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same_value(v, w)))
}

fn title(ctx: &Ctx, family: &Family, at: &ParamAt, input: &Value) -> String {
    let mut parts = Vec::new();
    for (name, _, _, _) in family.fields {
        if let Some(value) = input.get(*name).filter(|v| !v.is_null()) {
            let shown = match value {
                Value::Bool(true) if *name == "active" => "on".to_string(),
                Value::Bool(false) if *name == "active" => "off".to_string(),
                Value::String(text) if *name == "sourceRef" => {
                    let _ = text;
                    "from another parameter".to_string()
                }
                Value::String(text) => format!("{name} {text}"),
                other => format!("{name} {other}"),
            };
            parts.push(shown);
        }
    }
    format!(
        "{} {} {} {}: {}",
        fx_label(ctx, at.at.fx),
        rows::clip(&ctx.reaper.param_name(at.at.fx, at.param)),
        place_phrase(ctx, at.at.holder),
        family.noun,
        parts.join(", ")
    )
}

fn target(ctx: &Ctx, at: &ParamAt) -> Value {
    fx::param_target(ctx, at)
}

pub fn preview(ctx: &Ctx, family: &Family, input: &Value) -> Done<Preview> {
    let at = fx::locate_param(ctx, input)?;
    let touched = touches(ctx, family, &at, input)?;
    let keys: Vec<String> = touched.iter().map(|t| t.key.clone()).collect();
    let now = state(ctx, at.at.fx, at.at.holder, &keys);
    let mut display = json!({"title": title(ctx, family, &at, input)});
    let mut notes = Vec::new();
    if keys.iter().any(|k| k.ends_with(".mod.active")) && input.get("active").is_none()
        || (family.kind != "fx-param-mod" && keys.first().is_some_and(|k| k.ends_with(".mod.active")))
    {
        notes.push("The parameter's modulation is turned on with it (it does nothing while off).");
    }
    if family.kind == "fx-param-mod" && !sourced(ctx, at.at.fx, at.param) {
        notes.push("With no LFO, audio control signal or link on, REAPER's undo doesn't keep this; Kumi's undo does.");
    }
    if !notes.is_empty() {
        display["note"] = notes.join(" ").into();
    }
    Ok(Preview { target: target(ctx, &at), prior: json!({"keys": keys, "state": now}), fence: json!({"state": now}), display })
}

/// Whether a source of the parameter's modulation is on: REAPER's undo state holds its modulation only then (lab 7.82: a
/// write with none on makes no point).
fn sourced(ctx: &Ctx, fx: Fx, param: i32) -> bool {
    ["lfo.active", "acs.active", "plink.active"].iter().any(|k| ctx.reaper.fx_config(fx, &key(param, k)).is_some_and(|v| v.trim() == "1"))
}

fn write_all(ctx: &Ctx, fx: Fx, writes: &[(String, String)]) {
    for (key, text) in writes {
        ctx.reaper.fx_set_config(fx, key, text);
    }
    mark_fx(ctx.reaper, fx);
}

pub fn apply(ctx: &Ctx, family: &Family, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fx::locate_param(ctx, input).map_err(went)?;
    let touched = touches(ctx, family, &at, input)?;
    let keys: Vec<String> = touched.iter().map(|t| t.key.clone()).collect();
    let now = state(ctx, at.at.fx, at.at.holder, &keys);
    if !same_state(&now, &fence["state"]) {
        return Err(changed(format!("the parameter's {} changed since the preview", family.noun)));
    }
    let writes: Vec<(String, String)> = touched.into_iter().map(|t| (t.key, t.write)).collect();
    let unwritten = writes.iter().all(|(key, text)| now.get(key).is_some_and(|v| same_value(v, &Value::from(text.clone()))));
    let sourced_before = sourced(ctx, at.at.fx, at.param);
    if !unwritten {
        write_all(ctx, at.at.fx, &writes);
    }
    // (MIDI learn is in REAPER's undo state whatever the modulation holds)
    if family.kind != "fx-param-learn" && !sourced_before && !sourced(ctx, at.at.fx, at.param) {
        ctx.unkept.set(true);
    }
    let after = state(ctx, at.at.fx, at.at.holder, &keys);
    if after.as_object().is_some_and(|a| a.values().all(Value::is_null)) {
        return Err(failed(format!("REAPER took none of the {}'s values", family.noun)));
    }
    // What REAPER took instead of what was asked (it clamps some values): said, not refused.
    let clamped: Vec<String> = writes
        .iter()
        .filter(|(key, text)| !key.ends_with(".plink.effect") && !after.get(key).is_some_and(|v| same_value(v, &Value::from(text.clone()))))
        .map(|(key, text)| format!("{} {} (asked {text})", key.splitn(3, '.').nth(2).unwrap_or(key), after[key].as_str().unwrap_or("?")))
        .collect();
    let mut display = json!({"title": title(ctx, family, &at, input)});
    if !clamped.is_empty() {
        display["note"] = format!("REAPER took {}.", clamped.join(", ")).into();
    }
    Ok(Applied { applied: json!({"state": after}), display, unwritten, ..Default::default() })
}

pub fn restore(ctx: &Ctx, family: &Family, restore: &Restore) -> Done<Restored> {
    let guid = match Ref::parse(restore.target["fxRef"].as_str().unwrap_or_default()) {
        Ok(Ref::Fx(guid)) => guid,
        _ => return Err(bad("target.fxRef")),
    };
    let at = fxchain::locate_guid(ctx, &guid).map_err(fxchain::gone_if_missing)?;
    let param = restore.target["param"].as_i64().ok_or_else(|| bad("target.param"))? as i32;
    let keys: Vec<String> = restore.prior["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let prior = &restore.prior["state"];
    let now = state(ctx, at.fx, at.holder, &keys);
    // A key that had no block before reads its default once one is made: put back as that.
    let wanted: Value = Value::Object(
        keys.iter()
            .map(|key| {
                let value = match prior.get(key) {
                    Some(Value::Null) | None => Value::from(default_of(family, key)),
                    Some(value) => value.clone(),
                };
                (key.clone(), value)
            })
            .collect(),
    );
    if same_state(&now, &wanted) || same_state(&now, prior) {
        return Ok(Restored { restored: json!({"state": now}), display: json!({"title": "It was back already"}), unwritten: true });
    }
    if !restore.discard && !same_state(&now, &restore.applied["state"]) {
        return Err(changed(format!("the parameter's {} has been changed in REAPER since Kumi set it", family.noun)));
    }
    let writes: Vec<(String, String)> =
        keys.iter().filter_map(|key| write_back(ctx, at.holder, &wanted[key]).map(|text| (key.clone(), text))).collect();
    if writes.len() != keys.len() {
        return Err(changed("the link's source FX is no longer in the chain"));
    }
    write_all(ctx, at.fx, &writes);
    let after = state(ctx, at.fx, at.holder, &keys);
    if !same_state(&after, &wanted) {
        return Err(failed(format!("REAPER wouldn't put the parameter's {} back", family.noun)));
    }
    let name = rows::clip(&ctx.reaper.param_name(at.fx, param));
    let mut display =
        json!({"title": format!("{} {name} {}: {} put back", fx_label(ctx, at.fx), place_phrase(ctx, at.holder), family.noun)});
    if prior.as_object().is_some_and(|p| p.values().all(Value::is_null)) {
        display["note"] = "REAPER keeps the parameter's modulation settings once made; they are turned off, as before.".into();
    }
    Ok(Restored { restored: json!({"state": after}), display, unwritten: false })
}
