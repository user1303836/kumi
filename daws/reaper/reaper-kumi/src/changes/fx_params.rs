//! `fx-params`: up to 256 parameter values of one FX set in one undo point (what M5's listening tunes at once). Each value
//! is found and read back as `fx-param` finds and reads one (`normalized`, `value` text, or `units`); the FX by its GUID,
//! in any chain (a track's, its input chain, a take's, a container's).

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use super::fx::{self, fx_label, fx_target, near, place_phrase, shown, ParamAt, Wanted};
use super::fxchain::{self, changed, gone_if_missing, went, FxAt, QuietAutomation};
use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::rows;

/// The most values one change sets.
const MAX_VALUES: usize = 256;

/// One value asked for: its parameter, what it becomes, and that as a normalized value.
struct Ask {
    param: i32,
    wanted: Wanted,
    normalized: f64,
}

fn at_param(at: &FxAt, param: i32) -> ParamAt {
    ParamAt { at: at.clone(), param }
}

fn asks(ctx: &Ctx, at: &FxAt, input: &Value) -> Done<Vec<Ask>> {
    let list = input
        .get("values")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or_else(|| bad("values: a list of {param, normalized | value | units}"))?;
    if list.len() > MAX_VALUES {
        return Err(bad(format!("at most {MAX_VALUES} values at once ({} given)", list.len())));
    }
    let mut seen = BTreeSet::new();
    list.iter()
        .map(|item| {
            let param = fx::param_index(ctx, at, item.get("param"))?;
            if !seen.insert(param) {
                return Err(bad(format!("parameter {param} is given twice")));
            }
            let place = at_param(at, param);
            let wanted = fx::wanted_value(ctx, &place, item)?;
            let normalized = fx::wanted_normalized(ctx, &place, &wanted);
            Ok(Ask { param, wanted, normalized })
        })
        .collect()
}

/// Each parameter's normalized value now, by its index as text (the record's keys).
fn values_of(ctx: &Ctx, at: &FxAt, params: impl Iterator<Item = i32>) -> Map<String, Value> {
    params.map(|param| (param.to_string(), ctx.reaper.param_normalized(at.fx, param).into())).collect()
}

fn same_values(now: &Map<String, Value>, kept: &Value) -> bool {
    now.iter().all(|(param, value)| near(value.as_f64().unwrap_or(f64::NAN), kept[param].as_f64().unwrap_or(f64::NAN)))
}

fn identity(ctx: &Ctx, at: &FxAt) -> Value {
    json!({"guid": at.guid, "chain": fxchain::holder_value(ctx, at.holder)})
}

fn record(ctx: &Ctx, at: &FxAt, params: Map<String, Value>) -> Value {
    let mut record = identity(ctx, at);
    record["params"] = Value::Object(params);
    record
}

fn title(ctx: &Ctx, at: &FxAt, count: usize) -> String {
    let noun = if count == 1 { "parameter" } else { "parameters" };
    format!("{} {count} {noun} {}", fx_label(ctx, at.fx), place_phrase(ctx, at.holder))
}

/// "Freq → 120 Hz": a parameter and what it shows at a value.
fn line(ctx: &Ctx, at: &FxAt, param: i32, normalized: f64) -> String {
    format!("{} → {}", rows::clip(&ctx.reaper.param_name(at.fx, param)), shown(ctx, &at_param(at, param), normalized))
}

/// Each parameter's move as HISTORY draws it (for the display only: no undo reads it): its name, where it was and is
/// (normalized), and what the plug-in shows at each.
fn moves(ctx: &Ctx, at: &FxAt, before: &Map<String, Value>, after: &Map<String, Value>) -> Vec<Value> {
    after
        .iter()
        .filter_map(|(param, to)| {
            let (index, to) = (param.parse::<i32>().ok()?, to.as_f64()?);
            let from = before.get(param).and_then(Value::as_f64);
            Some(json!({
                "name": rows::clip(&ctx.reaper.param_name(at.fx, index)),
                "from": from,
                "to": to,
                "min": 0.0,
                "max": 1.0,
                "fromText": from.map(|from| shown(ctx, &at_param(at, index), from)),
                "toText": shown(ctx, &at_param(at, index), to),
            }))
        })
        .collect()
}

fn lines(ctx: &Ctx, at: &FxAt, values: &Map<String, Value>) -> Vec<String> {
    values.iter().filter_map(|(param, value)| Some(line(ctx, at, param.parse().ok()?, value.as_f64()?))).collect()
}

/// Whether a value is where it was asked to be already (a JSFX slider by its own value).
fn there(ctx: &Ctx, at: &FxAt, ask: &Ask, now: f64) -> bool {
    match ask.wanted {
        Wanted::Normalized(n) => near(n, now),
        Wanted::Raw(raw) => ctx.reaper.fx_param_raw(at.fx, ask.param).is_some_and(|(value, _, _)| (value - raw).abs() <= 1e-9),
    }
}

/// Writes values back (normalized), quietly where the track would record.
fn write_back(ctx: &Ctx, at: &FxAt, values: &Map<String, Value>) {
    let _quiet = QuietAutomation::around(ctx.reaper, at.holder.track(ctx.reaper));
    for (param, value) in values {
        if let (Ok(param), Some(value)) = (param.parse::<i32>(), value.as_f64()) {
            ctx.reaper.fx_set_param(at.fx, param, value);
        }
    }
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let asks = asks(ctx, &at, input)?;
    let now = values_of(ctx, &at, asks.iter().map(|ask| ask.param));
    let mut display = json!({
        "title": title(ctx, &at, asks.len()),
        "lines": asks.iter().map(|ask| line(ctx, &at, ask.param, ask.normalized)).collect::<Vec<_>>(),
    });
    if asks.iter().all(|ask| near(ask.normalized, now[&ask.param.to_string()].as_f64().unwrap_or(f64::NAN))) {
        display["unchanged"] = true.into();
    }
    let prior = record(ctx, &at, now);
    Ok(Preview { target: fx_target(ctx, &at), fence: prior.clone(), prior, display })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let asks = asks(ctx, &at, input)?;
    let now = values_of(ctx, &at, asks.iter().map(|ask| ask.param));
    if fxchain::holder_value(ctx, at.holder) != fence["chain"] || !same_values(&now, &fence["params"]) {
        return Err(changed("the FX's parameters, or the chain it is in, changed since the preview"));
    }
    let before = |param: i32| now[&param.to_string()].as_f64().unwrap_or(f64::NAN);
    let todo: Vec<&Ask> = asks.iter().filter(|ask| !there(ctx, &at, ask, before(ask.param))).collect();
    if !todo.is_empty() {
        let _quiet = QuietAutomation::around(ctx.reaper, at.holder.track(ctx.reaper));
        for ask in &todo {
            let wrote = match ask.wanted {
                Wanted::Normalized(n) => ctx.reaper.fx_set_param(at.fx, ask.param, n),
                Wanted::Raw(raw) => ctx.reaper.fx_set_param_raw(at.fx, ask.param, raw),
            };
            if !wrote {
                write_back(ctx, &at, &now);
                let name = rows::clip(&ctx.reaper.param_name(at.fx, ask.param));
                return Err(failed(format!("REAPER wouldn't set {name}; the values written are put back")));
            }
        }
    }
    let after = values_of(ctx, &at, asks.iter().map(|ask| ask.param));
    // A value the plug-in kept as it was: everything goes back, nothing half-done stays.
    let kept: Vec<String> = todo
        .iter()
        .filter(|ask| {
            let read = after[&ask.param.to_string()].as_f64().unwrap_or(f64::NAN);
            (read - ask.normalized).abs() > 1e-3 && near(read, before(ask.param))
        })
        .map(|ask| rows::clip(&ctx.reaper.param_name(at.fx, ask.param)))
        .collect();
    if !kept.is_empty() {
        write_back(ctx, &at, &now);
        return Err(failed(format!("the plug-in kept {} as it was; nothing is changed", kept.join(", "))));
    }
    let display = json!({"title": title(ctx, &at, asks.len()), "lines": lines(ctx, &at, &after), "values": moves(ctx, &at, &now, &after)});
    Ok(Applied { applied: record(ctx, &at, after), display, unwritten: todo.is_empty(), ..Default::default() })
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = restore.prior["guid"].as_str().ok_or_else(|| bad("prior.guid"))?;
    let at = fxchain::locate_guid(ctx, guid).map_err(gone_if_missing)?;
    let prior = restore.prior["params"].as_object().ok_or_else(|| bad("prior.params"))?;
    let count = ctx.reaper.fx_param_count(at.fx);
    let params: Vec<i32> = prior.keys().filter_map(|param| param.parse().ok()).collect();
    if params.len() != prior.len() || params.iter().any(|param| *param >= count) {
        return Err(changed(format!("{} has fewer parameters than when Kumi set them", fx_label(ctx, at.fx))));
    }
    let now = values_of(ctx, &at, params.iter().copied());
    let n = params.len();
    if same_values(&now, &restore.prior["params"]) {
        let display = json!({"title": format!("{} (as they were)", title(ctx, &at, n))});
        return Ok(Restored { restored: record(ctx, &at, now), display, unwritten: true });
    }
    if !restore.discard {
        let moved: Vec<String> = now
            .iter()
            .filter(|(param, value)| {
                !near(value.as_f64().unwrap_or(f64::NAN), restore.applied["params"][param.as_str()].as_f64().unwrap_or(f64::NAN))
            })
            .filter_map(|(param, _)| param.parse().ok().map(|param| rows::clip(&ctx.reaper.param_name(at.fx, param))))
            .collect();
        if !moved.is_empty() {
            return Err(changed(format!("{} changed in REAPER since Kumi set it", moved.join(", "))));
        }
    }
    let wanted: Map<String, Value> = prior
        .iter()
        .filter(|(param, value)| !near(now[param.as_str()].as_f64().unwrap_or(f64::NAN), value.as_f64().unwrap_or(f64::NAN)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    write_back(ctx, &at, &wanted);
    let after = values_of(ctx, &at, params.iter().copied());
    let kept: Vec<String> = wanted
        .iter()
        .filter(|(param, value)| (after[param.as_str()].as_f64().unwrap_or(f64::NAN) - value.as_f64().unwrap_or(f64::NAN)).abs() > 1e-3)
        .filter_map(|(param, _)| param.parse().ok().map(|param| rows::clip(&ctx.reaper.param_name(at.fx, param))))
        .collect();
    if !kept.is_empty() {
        return Err(failed(format!("the plug-in kept {} as it was", kept.join(", "))));
    }
    let display = json!({"title": format!("{} put back", title(ctx, &at, n)), "lines": lines(ctx, &at, &after)});
    Ok(Restored { restored: record(ctx, &at, after), display, unwritten: false })
}
