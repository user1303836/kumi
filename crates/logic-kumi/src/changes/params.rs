//! A plug-in's parameters, through its window's Controls View ([`super::window`]): `fx-param` sets one, `fx-params`
//! several in one change, and the read kind `param` lists them (`read {kind: param, parent: <plug-in>}`). A
//! parameter is named by its ref (`<epoch>:param:<plug-in's numbers>.<n>`), or by the plug-in's ref and `param`, its
//! name as the Controls View shows it or its place there (from 1). The value is `normalized` (0 to 1 over the
//! control's range) or `value`, what the Controls View shows: a number in the parameter's own units ("-6" for
//! "-6.0 dB"), found by setting the slider and reading its text, or an option's name for a choice. Read back from
//! the view. Logic keeps no step of its project undo for a parameter (the plug-in window's own undo has it), so a
//! restore is by value. A value the view shows with no control (the Compressor's Knee) is read, with its text and no
//! value, and a change of it is refused.

use serde_json::{json, Value};

use super::strips::{self, Owner};
use super::window::{self, Kind, Param, Shown};
use super::{changed, gone, quoted, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::refs::{bad, failed};

/// The most parameters one change sets.
const MOST: usize = 64;

/// The plug-in a request names, and the parameter it names in its ref, if it does.
fn plugin_of(ctx: &Ctx, input: &Value, epoch: Epoch) -> Done<(Owner, usize, Option<usize>)> {
    let text = input.get("ref").and_then(Value::as_str).unwrap_or_default();
    if crate::refs::Ref::parse(text).is_some_and(|r| r.kind == "param") {
        let (owner, slot, index) = strips::param(ctx, input, "ref", epoch)?;
        return Ok((owner, slot, Some(index)));
    }
    let (owner, slot) = strips::plugin(ctx, input, "ref", epoch)?;
    Ok((owner, slot, None))
}

/// A parameter by its name (as the Controls View shows it, any case) or its place (from 1).
fn find<'a>(params: &'a [Param], which: &Value, plugin: &str) -> Done<&'a Param> {
    match which {
        Value::Number(n) => n
            .as_u64()
            .and_then(|i| params.get((i as usize).wrapping_sub(1)))
            .ok_or_else(|| bad(format!("param must be a parameter's place from 1 to {}, or its name", params.len()))),
        Value::String(name) => {
            let found: Vec<&Param> = params.iter().filter(|p| p.name.eq_ignore_ascii_case(name.trim())).collect();
            match found.as_slice() {
                [one] => Ok(one),
                [] => Err(bad(format!(
                    "{} has no parameter named {name:?}: read its parameters (read kind param) for their names",
                    quoted(plugin)
                ))),
                _ => Err(bad(format!("several parameters of {} are named {name:?}: give its place or its ref", quoted(plugin)))),
            }
        }
        _ => Err(bad("param must be the parameter's name or its place (from 1)")),
    }
}

/// What a value asked for is: a share of the range, a number in the parameter's units, or an option's name.
#[derive(Debug, Clone, PartialEq)]
enum Wanted {
    Normalized(f64),
    Reading(f64, f64),
    Option(String),
}

fn wanted_of(item: &Value) -> Done<Wanted> {
    if let Some(n) = item.get("normalized").filter(|v| !v.is_null()) {
        let n = n.as_f64().filter(|n| (0.0..=1.0).contains(n)).ok_or_else(|| bad("normalized must be a number from 0 to 1"))?;
        return Ok(Wanted::Normalized(n));
    }
    match item.get("value") {
        Some(Value::Number(n)) => {
            let n = n.as_f64().filter(|n| n.is_finite()).ok_or_else(|| bad("value must be a finite number"))?;
            Ok(Wanted::Reading(n, tolerance(&n.to_string())))
        }
        Some(Value::String(text)) => {
            match window::reading(text).filter(|_| text.trim().starts_with(|c: char| c.is_ascii_digit() || "-+.−".contains(c))) {
                Some(n) => Ok(Wanted::Reading(n, tolerance(text))),
                None if !text.trim().is_empty() => Ok(Wanted::Option(text.trim().to_string())),
                None => Err(bad("value must say what the parameter should show")),
            }
        }
        _ => Err(bad("give normalized (0 to 1) or value (what the Controls View should show, such as -6 or \"-6.0 dB\")")),
    }
}

/// Half a unit of the last decimal given: "-6" may read -6.4 at worst, "-6.0" -6.05.
fn tolerance(text: &str) -> f64 {
    let decimals = text.split_once('.').map_or(0, |(_, rest)| rest.chars().take_while(char::is_ascii_digit).count());
    0.5 * 10f64.powi(-(decimals as i32)) + 1e-9
}

/// The raw value a wanted value is for this parameter: computed for a share, found by reading for a number (with the
/// slider's resolution there when no value reads the number exactly), the option's place for a name.
fn raw_for(ctx: &mut Ctx, shown: &Shown, param: &Param, wanted: &Wanted) -> Done<(f64, Option<f64>)> {
    if let (Wanted::Reading(n, tolerance), Kind::Slider) = (wanted, param.kind) {
        let found = window::find_raw(ctx, shown, param, *n, *tolerance)?.ok_or_else(|| {
            failed(format!(
                "no value of {}'s {} reads {n} (it reads {} now; read its parameters for the range)",
                quoted(&shown.plugin),
                param.name,
                param.text
            ))
        })?;
        return Ok((found.raw, found.step));
    }
    raw_of(param, wanted).map(|raw| (raw, None))
}

/// The raw value of a share, an option or a switch's word.
fn raw_of(param: &Param, wanted: &Wanted) -> Done<f64> {
    match (wanted, param.kind) {
        (Wanted::Normalized(n), Kind::Slider) => {
            let (min, max) = (param.min.unwrap_or(0.0), param.max.unwrap_or(1.0));
            Ok((min + n * (max - min)).round())
        }
        (Wanted::Normalized(n), Kind::Choice) => Ok((n * (param.choices.len().saturating_sub(1)) as f64).round()),
        (Wanted::Normalized(n), Kind::Switch) => Ok(if *n >= 0.5 { 1.0 } else { 0.0 }),
        (Wanted::Option(name), Kind::Choice) => {
            param.choices.iter().position(|(title, _)| title.eq_ignore_ascii_case(name)).map(|i| i as f64).ok_or_else(|| {
                let options: Vec<&str> = param.choices.iter().map(|(t, _)| t.as_str()).collect();
                bad(format!("{} is one of {}", param.name, options.join(", ")))
            })
        }
        (Wanted::Option(name), Kind::Switch) if ["on", "off"].contains(&name.to_lowercase().as_str()) => {
            Ok(if name.eq_ignore_ascii_case("on") { 1.0 } else { 0.0 })
        }
        _ => Err(bad(format!("{} doesn't take that value (it shows {})", param.name, param.text))),
    }
}

/// A parameter's state as a change keeps it.
fn state(param: &Param) -> Value {
    json!({"index": param.index, "name": param.name, "value": param.value, "text": param.text})
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a["value"].as_f64(), b["value"].as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-6,
        _ => a["text"] == b["text"],
    }
}

/// The asks of a change: `fx-param`'s one (its `param` or the ref's), `fx-params`' list of `{param, normalized | value}`.
fn asks(input: &Value, from_ref: Option<usize>, many: bool) -> Done<Vec<(Value, Wanted)>> {
    if !many {
        let which = match (from_ref, input.get("param").filter(|v| !v.is_null())) {
            (Some(index), _) => json!(index),
            (None, Some(param)) => param.clone(),
            (None, None) => return Err(bad("pass the parameter's ref as ref, or the plug-in's ref as ref and param (its name or place)")),
        };
        return Ok(vec![(which, wanted_of(input)?)]);
    }
    let list = input
        .get("values")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or_else(|| bad("values: a list of {param, normalized | value}"))?;
    if list.len() > MOST {
        return Err(bad(format!("at most {MOST} values at once ({} given)", list.len())));
    }
    list.iter()
        .map(|item| {
            let which =
                item.get("param").filter(|v| !v.is_null()).cloned().ok_or_else(|| bad("each value needs its param (name or place)"))?;
            Ok((which, wanted_of(item)?))
        })
        .collect()
}

/// The parameters the asks name, read now, each once.
fn named<'a>(params: &'a [Param], asks: &[(Value, Wanted)], plugin: &str) -> Done<Vec<&'a Param>> {
    let mut out: Vec<&Param> = Vec::new();
    for (which, _) in asks {
        let param = find(params, which, plugin)?;
        if out.iter().any(|p| p.index == param.index) {
            return Err(bad(format!("{} is given twice", param.name)));
        }
        out.push(param);
    }
    Ok(out)
}

fn record(params: &[&Param]) -> Value {
    Value::Array(params.iter().map(|p| state(p)).collect())
}

fn title(plugin: &str, strip: &str, params: &[Value]) -> String {
    match params {
        [one] => format!(
            "{}'s {} → {} on {}",
            quoted(plugin),
            one["name"].as_str().unwrap_or_default(),
            one["text"].as_str().unwrap_or_default(),
            quoted(strip)
        ),
        many => format!("{} {} parameters on {}", quoted(plugin), many.len(), quoted(strip)),
    }
}

fn lines(params: &[Value]) -> Vec<String> {
    params.iter().map(|p| format!("{} → {}", p["name"].as_str().unwrap_or_default(), p["text"].as_str().unwrap_or_default())).collect()
}

pub fn preview(ctx: &mut Ctx, input: &Value, many: bool) -> Done<Preview> {
    let (owner, slot, from_ref) = plugin_of(ctx, input, Epoch::Current)?;
    let asks = asks(input, from_ref, many)?;
    let shown = window::show(ctx, owner, slot)?;
    let result = (|| {
        let params = window::params(ctx, &shown)?;
        let chosen = named(&params, &asks, &shown.plugin)?;
        for ((_, wanted), param) in asks.iter().zip(&chosen) {
            if let (Wanted::Option(_), Kind::Slider) = (wanted, param.kind) {
                return Err(bad(format!("{} is a slider: give normalized or a number", param.name)));
            }
            if param.kind == Kind::Readout {
                return Err(bad(window::readout_words(&shown.plugin, &param.name)));
            }
        }
        let prior = record(&chosen);
        let fence = json!({"plugin": shown.plugin, "params": prior});
        let mut display = json!({
            "title": format!("{} {} on {}", quoted(&shown.plugin), if chosen.len() == 1 { chosen[0].name.clone() } else { format!("{} parameters", chosen.len()) }, quoted(&shown.strip)),
            "params": chosen.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        });
        display["note"] = "A parameter moved in the plug-in window is a step of that window's own undo, not of the project's: Kumi puts it back by value.".into();
        Ok(Preview {
            target: json!({"ref": owner.plugin(ctx.project.epoch, slot), "kind": "plugin", "track": owner.text(ctx.project.epoch)}),
            prior: json!({"plugin": shown.plugin, "params": fence["params"]}),
            fence,
            display,
        })
    })();
    window::hide(ctx, shown);
    result
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value, many: bool) -> Done<Applied> {
    let (owner, slot, from_ref) = plugin_of(ctx, input, Epoch::Current).map_err(strips::went)?;
    let asks = asks(input, from_ref, many)?;
    let shown = window::show(ctx, owner, slot)?;
    let result = (|| {
        let params = window::params(ctx, &shown)?;
        let chosen: Vec<Param> = named(&params, &asks, &shown.plugin)?.into_iter().cloned().collect();
        let now = record(&chosen.iter().collect::<Vec<_>>());
        if json!({"plugin": shown.plugin, "params": now}) != *fence {
            return Err(changed(format!("{} changed in Logic Pro since the preview", quoted(&shown.plugin))));
        }
        let mut after: Vec<Value> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for ((_, wanted), param) in asks.iter().zip(&chosen) {
            let (raw, step) = raw_for(ctx, &shown, param, wanted)?;
            let read = window::write(ctx, &shown, param, raw)?;
            let shows = match wanted {
                Wanted::Option(name) => read.text.eq_ignore_ascii_case(name) || (read.kind == Kind::Switch && read.value == Some(raw)),
                _ => read.value.is_some_and(|v| (v - raw).abs() < 1e-6),
            };
            if !shows {
                // What was written so far goes back: nothing half-done stays.
                write_back(ctx, &shown, &chosen, &after);
                return Err(super::uncertain(format!(
                    "Logic Pro shows {}'s {} as {} after Kumi set it",
                    quoted(&shown.plugin),
                    param.name,
                    read.text
                )));
            }
            if let (Some(step), Wanted::Reading(n, _)) = (step, wanted) {
                notes.push(format!(
                    "{} moves {} a step here, so {} is the nearest it shows to {n}.",
                    param.name,
                    (step * 1000.0).round() / 1000.0,
                    read.text
                ));
            }
            after.push(state(&read));
        }
        let mut display = json!({"title": title(&shown.plugin, &shown.strip, &after), "lines": lines(&after)});
        if !notes.is_empty() {
            display["note"] = notes.join(" ").into();
        }
        Ok(Applied { applied: json!({"plugin": shown.plugin, "params": after}), display, ..Applied::default() })
    })();
    window::hide(ctx, shown);
    result
}

/// Puts the parameters written so far back to the values they had.
fn write_back(ctx: &mut Ctx, shown: &Shown, chosen: &[Param], written: &[Value]) {
    for (param, _) in chosen.iter().zip(written) {
        if let Some(raw) = param.value {
            if let Ok(now) = window::param(ctx, shown, param.index) {
                let _ = window::write(ctx, shown, &now, raw);
            }
        }
    }
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let target = restore.target;
    let (owner, slot) = strips::plugin(ctx, target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let prior: Vec<Value> = restore.prior["params"].as_array().cloned().ok_or_else(|| bad("prior.params"))?;
    let applied: Vec<Value> = restore.applied["params"].as_array().cloned().unwrap_or_default();
    let plugin = restore.prior["plugin"].as_str().unwrap_or_default().to_string();
    let shown = window::show(ctx, owner, slot)?;
    let result = (|| {
        if shown.plugin != plugin {
            return Err(changed(format!("the slot holds {} now, not {}", quoted(&shown.plugin), quoted(&plugin))));
        }
        let mut back = Vec::new();
        for was in &prior {
            let index = was["index"].as_u64().unwrap_or(0) as usize;
            let now = window::param(ctx, &shown, index)?;
            if now.name != was["name"].as_str().unwrap_or_default() {
                return Err(changed(format!("{}'s parameters aren't as Kumi left them", quoted(&plugin))));
            }
            if same_value(&state(&now), was) {
                back.push(state(&now));
                continue;
            }
            let kumi = applied.iter().find(|a| a["index"] == was["index"]);
            if !restore.discard && !kumi.is_some_and(|a| same_value(&state(&now), a)) {
                return Err(changed(format!("{}'s {} has been changed in Logic Pro since Kumi set it", quoted(&plugin), now.name)));
            }
            let raw = was["value"].as_f64().ok_or_else(|| failed(format!("{} was a menu Kumi can't set back", now.name)))?;
            let read = window::write(ctx, &shown, &now, raw)?;
            if !same_value(&state(&read), was) {
                return Err(super::uncertain(format!(
                    "Logic Pro shows {}'s {} as {} after Kumi put it back",
                    quoted(&plugin),
                    read.name,
                    read.text
                )));
            }
            back.push(state(&read));
        }
        Ok(Restored::by_value(
            json!({"plugin": plugin, "params": back}),
            json!({"title": format!("{} (put back)", title(&plugin, &shown.strip, &back))}),
        ))
    })();
    window::hide(ctx, shown);
    result
}

/// `read {kind: param, parent: <plug-in>}` (or `ref` a parameter): the Controls View's parameters as rows.
pub fn rows(ctx: &mut Ctx, owner: Owner, slot: usize) -> Done<Vec<Value>> {
    let shown = window::show(ctx, owner, slot)?;
    let result = window::params(ctx, &shown).map(|params| {
        let epoch = ctx.project.epoch;
        let plugin = owner.plugin(epoch, slot);
        params.iter().map(|param| param.row(&owner.param(epoch, slot, param.index), &plugin)).collect()
    });
    window::hide(ctx, shown);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_a_share_a_reading_or_an_option() {
        assert_eq!(wanted_of(&json!({"normalized": 0.5})).unwrap(), Wanted::Normalized(0.5));
        assert!(wanted_of(&json!({"normalized": 2})).is_err());
        match wanted_of(&json!({"value": "-6.0 dB"})).unwrap() {
            Wanted::Reading(n, t) => assert!((n + 6.0).abs() < 1e-9 && (t - 0.05).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
        match wanted_of(&json!({"value": -6})).unwrap() {
            Wanted::Reading(n, t) => assert!((n + 6.0).abs() < 1e-9 && (t - 0.5).abs() < 1e-6),
            other => panic!("{other:?}"),
        }
        assert_eq!(wanted_of(&json!({"value": "Inv"})).unwrap(), Wanted::Option("Inv".into()));
        assert!(wanted_of(&json!({})).is_err());
        let params = vec![
            window::test_param(1, "Gain", Kind::Slider, Some(960.0), "0.0 dB", Some((0.0, 1200.0)), vec![]),
            window::test_param(
                2,
                "Phase Invert",
                Kind::Choice,
                Some(0.0),
                "Norm",
                Some((0.0, 1.0)),
                vec![("Norm".into(), "b".into()), ("Inv".into(), "c".into())],
            ),
        ];
        assert_eq!(find(&params, &json!("gain"), "Gain").unwrap().index, 1);
        assert_eq!(find(&params, &json!(2), "Gain").unwrap().name, "Phase Invert");
        assert!(find(&params, &json!("Drive"), "Gain").is_err());
        let asked = asks(&json!({"values": [{"param": "Gain", "value": -6}, {"param": 2, "value": "Inv"}]}), None, true).unwrap();
        assert_eq!(named(&params, &asked, "Gain").unwrap().len(), 2);
        assert!(named(
            &params,
            &asks(&json!({"values": [{"param": 1, "normalized": 0.1}, {"param": "Gain", "normalized": 0.2}]}), None, true).unwrap(),
            "Gain"
        )
        .is_err());
        assert_eq!(asks(&json!({"normalized": 0.5}), Some(1), false).unwrap()[0].0, json!(1));
        assert!(asks(&json!({"normalized": 0.5}), None, false).is_err());
    }
}
