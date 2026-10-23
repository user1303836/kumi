//! Parameter discovery, Live's display units, and atomic fast parameter changes.
use super::{
    bridge_version::{at_least, PYTHON_BRIDGE},
    changes::{new_record, pictures, ChangeKind, ChangeSummary},
    connection::LiveConnection,
    context,
    display::{value_for_display, DisplayMap, DISPLAY_MAP_SCRIPT},
    fast::{find_script, set_script},
    history::{FastResult, History},
    views::ViewHost,
};
use crate::{
    command::KUMI,
    core::{contracts::*, errors::RuntimeError, timing},
};
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number,
        string::{head, trim},
    },
    time::perf_now,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};

pub use crate::core::plan::ChangeOutcome;
#[derive(Clone, Serialize)]
pub struct Found {
    pub index: f64,
    pub name: String,
    pub min: f64,
    pub max: f64,
}
pub struct Parameters {
    pub history: Rc<History>,
    pub display_maps: RefCell<IndexMap<String, DisplayMap>>,
    pub fast_found: RefCell<IndexMap<String, Found>>,
    pub fast_generation: Cell<Option<u64>>,
    /// The lease `display_maps` were read in.
    maps_generation: Cell<Option<u64>>,
    fast: Option<bool>,
}
impl Parameters {
    pub fn new(history: Rc<History>, fast: Option<bool>) -> Self {
        Self {
            history,
            fast,
            display_maps: RefCell::new(IndexMap::new()),
            fast_found: RefCell::new(IndexMap::new()),
            fast_generation: Cell::new(None),
            maps_generation: Cell::new(None),
        }
    }
    /// What Kumi found of parameters by place goes when the place may mean something else: a new lease (each
    /// observation, or Live changed outside a change Kumi follows), or a device Kumi moved, deleted or replaced. A
    /// parameter ref is a position, so a device put in another's place answers to the same ref.
    pub fn forget(&self) {
        self.fast_found.borrow_mut().clear();
        self.display_maps.borrow_mut().clear();
    }
    fn forget_stale(&self) {
        let lease = self.history.connection.lease.get();
        if self.fast_generation.get() != Some(lease) {
            self.fast_found.borrow_mut().clear();
            self.fast_generation.set(Some(lease));
        }
        self.forget_stale_maps();
    }
    fn forget_stale_maps(&self) {
        let lease = self.history.connection.lease.get();
        if self.maps_generation.get() != Some(lease) {
            self.display_maps.borrow_mut().clear();
            self.maps_generation.set(Some(lease));
        }
    }
    pub fn fast_on(&self) -> bool {
        self.fast != Some(false)
            && std::env::var("KUMI_FAST").as_deref() != Ok("0")
            && at_least(self.history.connection.version().as_deref(), PYTHON_BRIDGE)
            && self.history.connection.has("live_run_python")
    }
    pub async fn device_parameters(&self, device_ref: Value, fields: Vec<String>, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        read_parameters(&self.history.connection, device_ref, fields, signal).await
    }
    pub async fn value_for_text(&self, parameter_ref: &str, text: &str, signal: Signal) -> Result<Result<f64, String>, RuntimeError> {
        let connection = &self.history.connection;
        self.forget_stale_maps();
        let named = connection.references.borrow().lengthen(&json!({"parameterRef":parameter_ref}));
        let long = named["parameterRef"].as_str().unwrap_or(parameter_ref).to_owned();
        let cached = self.display_maps.borrow().get(&long).cloned();
        let map = if let Some(map) = cached {
            map
        } else {
            if !at_least(connection.version().as_deref(), PYTHON_BRIDGE) || !connection.has("live_run_python") {
                return Ok(Err(format!("Give {} as a number in the parameter's range: this bridge can't read the parameter's units (update it with {} bridge).",stringify(&json!(text)),*KUMI)));
            }
            let read = connection
                .call(
                    "live_run_python",
                    object(json!({"code":DISPLAY_MAP_SCRIPT,"mode":"exec","ref":long,"timeoutMs":5000})),
                    abort::any([signal, connection.lifetime.clone()]),
                )
                .await?;
            let done = if read.is_error == Some(true) { None } else { Some(context::payload(&read)?) };
            let result = if let Some(done) = done.filter(|v| v.get("ok") == Some(&Value::Bool(true))) {
                Some(context::object(done.get("result").filter(|v| !v.is_null()).unwrap_or(&json!({})))?)
            } else {
                None
            };
            let Some(map) = result.as_ref().and_then(display_map) else {
                return Ok(Err("Kumi couldn't read how this parameter shows its values; give a number in its range.".into()));
            };
            self.display_maps.borrow_mut().insert(long, map.clone());
            if self.display_maps.borrow().len() > 4096 {
                self.display_maps.borrow_mut().shift_remove_index(0);
            }
            map
        };
        Ok(value_for_display(&map, text))
    }
    pub async fn fast_parameters(&self, kind: &ChangeKind, input: &JsonObject, signal: Signal) -> Result<ChangeOutcome, RuntimeError> {
        let Some(device) = input.get("deviceRef").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            return Ok(ChangeOutcome::error("Name the device (deviceRef) whose parameter this is."));
        };
        let connection = &self.history.connection;
        self.forget_stale();
        let several = input.get("values").is_some_and(Value::is_array);
        let asked = if several {
            input["values"].as_array().unwrap().iter().map(context::object).collect::<Result<Vec<_>, _>>()?
        } else {
            vec![input.clone()]
        };
        if asked.is_empty() {
            return Ok(ChangeOutcome::error("Give at least one parameter and its value."));
        }
        struct Step {
            reference: Option<String>,
            name: Option<String>,
            number: Option<f64>,
            text: Option<String>,
        }
        impl Step {
            fn key(&self, device: &str) -> String {
                self.reference.clone().unwrap_or_else(|| format!("{device}\0{}", trim(self.name.as_deref().unwrap_or("")).to_lowercase()))
            }
            fn map_key(&self, device: &str, index: Option<f64>) -> String {
                self.reference
                    .clone()
                    .unwrap_or_else(|| format!("{device}\0#{}", index.map(number::to_string).unwrap_or_else(|| "undefined".into())))
            }
        }
        let steps: Vec<_> = asked
            .iter()
            .map(|row| {
                let value = row.get("value");
                let numeric = value.and_then(Value::as_f64).or_else(|| {
                    value.and_then(Value::as_str).filter(|s| !trim(s).is_empty()).and_then(number::parse).filter(|n| n.is_finite())
                });
                Step {
                    reference: row.get("parameterRef").and_then(Value::as_str).map(str::to_owned),
                    name: row.get("parameter").and_then(Value::as_str).map(str::to_owned),
                    number: numeric,
                    text: if numeric.is_none() { value.and_then(Value::as_str).map(str::to_owned) } else { None },
                }
            })
            .collect();
        for step in &steps {
            if step.reference.as_ref().is_none_or(|s| s.is_empty()) && step.name.as_ref().is_none_or(|s| s.is_empty()) {
                return Ok(ChangeOutcome::error(
                    "Each parameter needs its parameterRef from discovery, or its name as parameter (\"Drive\").",
                ));
            }
            if step.number.is_none() && step.text.is_none() {
                return Ok(ChangeOutcome::error(format!(
                    "Give {} a value: a number in its range, or what Live shows (\"2 dB\").",
                    step.name.as_deref().unwrap_or("the parameter")
                )));
            }
        }
        let mut missed = Vec::new();
        let mut dropped = HashSet::new();
        let mut on_device = None;
        let unknown: Vec<_> = steps
            .iter()
            .enumerate()
            .filter(|(_, step)| {
                let key = step.key(device);
                let found = self.fast_found.borrow().get(&key).cloned();
                (step.name.as_ref().is_some_and(|s| !s.is_empty())
                    && step.reference.as_ref().is_none_or(|s| s.is_empty())
                    && found.is_none())
                    || (step.text.is_some() && !self.display_maps.borrow().contains_key(&step.map_key(device, found.map(|f| f.index))))
            })
            .map(|(i, _)| i)
            .collect();
        if !unknown.is_empty() {
            let query: Vec<_> = unknown
                .iter()
                .map(|i| {
                    let step = &steps[*i];
                    if let Some(reference) = step.reference.as_ref().filter(|s| !s.is_empty()) {
                        json!({"ref":reference,"map":step.text.is_some()})
                    } else {
                        json!({"device":device,"parameter":step.name,"map":step.text.is_some()})
                    }
                })
                .collect();
            let found = self.history.run_fast(find_script(&json!(query)), signal.clone()).await?;
            let found = match found {
                FastResult::Result(v) => v,
                FastResult::Error { error, .. } => {
                    return Ok(ChangeOutcome::error(format!("Kumi couldn't find those parameters on the device: {error}")))
                }
            };
            let rows = found.as_array().into_iter().flatten().map(context::object).collect::<Result<Vec<_>, _>>()?;
            for (position, index) in unknown.iter().enumerate() {
                let step = &steps[*index];
                let empty = JsonObject::new();
                let row = rows.get(position).unwrap_or(&empty);
                if let Some(missing) = row.get("missing").and_then(Value::as_array) {
                    let name = step.name.as_ref().ok_or_else(|| RuntimeError::plain("Missing parameter name in Live reply"))?;
                    missed.push(json!({"parameter":head(name,64),"why":"the device has no parameter by that name"}));
                    dropped.insert(*index);
                    on_device = Some(missing.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>());
                    continue;
                }
                if row.get("error").is_some_and(Value::is_string)
                    || !row.get("name").is_some_and(Value::is_string)
                    || !row.get("min").is_some_and(Value::is_number)
                    || !row.get("max").is_some_and(Value::is_number)
                {
                    let why = row.get("error").filter(|v| !v.is_null()).map(js_string).unwrap_or_else(|| "Live didn't answer".into());
                    return Ok(ChangeOutcome::error(format!(
                        "Kumi couldn't read {}: {why}{}.",
                        step.name.as_deref().unwrap_or("that parameter"),
                        if why.contains("discover") { "" } else { "; discover the device again" }
                    )));
                }
                if let Some(index) = row.get("index").and_then(Value::as_f64) {
                    self.fast_found.borrow_mut().insert(
                        step.key(device),
                        Found {
                            index,
                            name: row["name"].as_str().unwrap().into(),
                            min: row["min"].as_f64().unwrap(),
                            max: row["max"].as_f64().unwrap(),
                        },
                    );
                }
                if let Some(map) = display_map(row) {
                    self.display_maps.borrow_mut().insert(step.map_key(device, row.get("index").and_then(Value::as_f64)), map);
                }
            }
        }
        let mut targets = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            if dropped.contains(&index) {
                continue;
            }
            let reference = step.reference.as_ref().filter(|s| !s.is_empty());
            let place = if reference.is_some() { None } else { self.fast_found.borrow().get(&step.key(device)).cloned() };
            let mut target = if let Some(reference) = reference {
                object(json!({"ref":reference}))
            } else {
                let found = place.as_ref().ok_or_else(|| RuntimeError::plain("Live did not identify the named parameter"))?;
                object(json!({"device":device,"index":found.index,"name":found.name}))
            };
            let value = if let Some(number) = step.number {
                number
            } else {
                let map = self
                    .display_maps
                    .borrow()
                    .get(&step.map_key(device, place.as_ref().map(|f| f.index)))
                    .cloned()
                    .ok_or_else(|| RuntimeError::plain("Live did not provide the parameter's display map"))?;
                match value_for_display(&map, step.text.as_deref().unwrap()) {
                    Ok(value) => value,
                    Err(why) => {
                        missed.push(json!({"parameter":place.as_ref().map(|f|f.name.as_str()).or(step.name.as_deref()).unwrap_or("a parameter"),"why":why}));
                        continue;
                    }
                }
            };
            target.insert("value".into(), json!(value));
            targets.push(target);
        }
        let count = missed.len();
        let mut misses = JsonObject::new();
        if count > 0 {
            misses.insert("missed".into(), json!(missed));
            if let Some(names) = on_device {
                misses.insert("parametersOnDevice".into(), json!(names));
            }
        }
        if targets.is_empty() {
            let mut reply = object(json!({"changed":null}));
            reply.extend(misses);
            return Ok(ChangeOutcome { text: stringify(&json!(reply)), is_error: true, missed: Some(count), stops: false, applied: None });
        }
        signal.check()?;
        self.history.changes_this_turn.set(self.history.changes_this_turn.get() + 1);
        let set = self.history.run_fast(set_script(&json!(targets)), self.history.change_signal()).await?;
        let result = match set {
            FastResult::Result(value) => context::object(&value)?,
            FastResult::Error { error, sent } => {
                if !sent {
                    return Ok(ChangeOutcome::error(format!(
                        "Live didn't change {}: {error}",
                        if targets.len() == 1 { "it" } else { "them" }
                    )));
                }
                let name = if targets.len() == 1 {
                    targets[0].get("name").and_then(Value::as_str).unwrap_or("A parameter").to_owned()
                } else {
                    format!("{} parameters", targets.len())
                };
                self.history.remember(
                    new_record(
                        kind,
                        ChangeSummary::title(format!("{name} (unconfirmed)")),
                        ChangeState::Unsure,
                        connection.now().timestamp_millis(),
                    ),
                    String::new(),
                    None,
                );
                return Ok(ChangeOutcome::stop("Live didn't confirm this change, so it may or may not have happened. Tell the producer to check Live; discover again before more changes."));
            }
        };
        let rows =
            result.get("items").and_then(Value::as_array).into_iter().flatten().map(context::object).collect::<Result<Vec<_>, _>>()?;
        let mut device_row = object(json!({"ref":device}));
        if let Some(name) = result.get("device").and_then(Value::as_str) {
            device_row.insert("name".into(), json!(name));
        }
        // A track Live couldn't say (none, or unreadable once the change was made) leaves the change as it is.
        let track = result.get("track").and_then(Value::as_object).cloned().unwrap_or_default();
        if let Some(reference) = track.get("ref") {
            device_row.insert("trackRef".into(), reference.clone());
        }
        let mut shown = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let target = targets.get(index).ok_or_else(|| RuntimeError::plain("Live returned an unexpected parameter"))?;
            let reference = target.get("ref").cloned().unwrap_or_else(|| json!(format!("{device}#{}", js_string(&target["index"]))));
            let mut shown_row = object(json!({"ref":reference}));
            for (from, to) in [
                ("name", "name"),
                ("prior", "currentValue"),
                ("value", "proposedValue"),
                ("min", "min"),
                ("max", "max"),
                ("priorDisplay", "displayValue"),
            ] {
                if let Some(value) = row.get(from) {
                    shown_row.insert(to.into(), value.clone());
                }
            }
            shown.push(shown_row);
        }
        let mut preview = object(json!({"device":device_row}));
        let mut applied = JsonObject::new();
        if several || rows.len() > 1 {
            preview.insert("parameters".into(), json!(shown));
            applied.insert(
                "parameters".into(),
                json!(shown
                    .iter()
                    .zip(&rows)
                    .map(|(shown, row)| {
                        let mut p = object(json!({"ref":shown["ref"]}));
                        if let Some(value) = row.get("display") {
                            p.insert("displayValue".into(), value.clone());
                        }
                        p
                    })
                    .collect::<Vec<_>>()),
            );
        } else {
            if let Some(row) = shown.first() {
                preview.insert("parameter".into(), json!(row));
            }
            if let Some(value) = rows.first().and_then(|r| r.get("display")) {
                applied.insert("displayValue".into(), value.clone());
            }
        }
        let summary = kind.summarize(
            &preview,
            input,
            &|reference| reference.as_str().and_then(|r| connection.references.borrow().known.get(r).cloned()),
            Some(&applied),
        );
        let record = new_record(kind, summary.clone(), ChangeState::Applied, connection.now().timestamp_millis());
        self.history.remember(record.clone(), String::new(), None);
        curve_later(&self.history, kind, Some(&json!(device)), &record.id, summary.picture.as_ref());
        let mut revert = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            let row = rows.get(index).ok_or_else(|| RuntimeError::plain("Live omitted a changed parameter"))?;
            let mut back = if let Some(reference) = target.get("ref") {
                object(json!({"ref":reference}))
            } else {
                object(json!({"device":target["device"],"index":target["index"]}))
            };
            for (from, to) in [("name", "name"), ("prior", "prior")] {
                if let Some(value) = row.get(from) {
                    back.insert(to.into(), value.clone());
                }
            }
            // What the target's own set left (two targets can be one parameter: the undo puts each back in turn),
            // else what Live kept in the end.
            if let Some(value) = row.get("applied").or_else(|| row.get("value")) {
                back.insert("applied".into(), value.clone());
            }
            revert.push(Value::Object(back));
        }
        if let Some(entry) = self.history.entries.borrow().get(&record.id) {
            entry.borrow_mut().revert = Some(revert);
        }
        let mut reply = object(json!({"changed":record.title,"change":record.id,"state":record.state}));
        if let Some(lines) = summary.lines.filter(|lines| !lines.is_empty()) {
            reply.insert("lines".into(), json!(lines));
        }
        reply.insert("live".into(),json!({"parameters":rows.iter().map(|row|{let mut result=JsonObject::new();for (from,to) in [("name","name"),("value","value"),("display","displayValue")]{if let Some(value)=row.get(from){result.insert(to.into(),value.clone());}}result}).collect::<Vec<_>>()}));
        reply.extend(misses);
        Ok(ChangeOutcome {
            text: stringify(&json!(reply)),
            is_error: false,
            missed: (count > 0).then_some(count),
            stops: false,
            applied: None,
        })
    }
}
/// A device's parameters (`fields` of each), page by page.
pub async fn read_parameters(
    connection: &LiveConnection,
    device_ref: Value,
    fields: Vec<String>,
    signal: Signal,
) -> Result<Vec<JsonObject>, RuntimeError> {
    let mut rows = Vec::new();
    let mut cursor = None;
    for _ in 0..10_000 {
        let mut args = object(json!({"kind":"parameter","parent":device_ref,"fields":fields,"limit":connection.page_limit()}));
        if let Some(cursor) = &cursor {
            args.insert("cursor".into(), json!(cursor));
        }
        let read = context::payload(&connection.call("live_discover", args, signal.clone()).await?)?;
        for item in read.get("items").and_then(Value::as_array).into_iter().flatten() {
            rows.push(context::object(item)?);
        }
        let next = read.get("nextCursor").and_then(Value::as_str).filter(|next| Some(*next) != cursor.as_deref()).map(str::to_owned);
        cursor = next;
        if cursor.as_ref().is_none_or(|s| s.is_empty()) {
            break;
        }
    }
    Ok(rows)
}
/// A plan's steps on one device come close together: the device is read once this long after the last of them.
const CURVE_QUIET_MS: f64 = 300.;
/// The longest a run of changes waits for its read.
const CURVE_WAIT_MS: f64 = 3_000.;

/// An equaliser's or a compressor's curve for a change to its parameters. The device is read once the change has
/// been told, so the change isn't kept waiting, and once a run of changes to it has ended; each change of the run
/// then gets its own before and after (`pictures::curves`), put on its record. `picture` is the change's own (every
/// value it moved), not its record's shortened one.
///
/// The read goes around Kumi's Live lock, as FOCUS's reads do: it reads parameters through the bridge without
/// registering references or taking a lease, so it can't come between a turn's own reads and changes. What else moved
/// on the device while the run waited (a preset, the producer's own hand) shows in its newest change's after.
pub fn curve_later(history: &Rc<History>, kind: &ChangeKind, device: Option<&Value>, id: &str, picture: Option<&ChangePicture>) {
    if kind.family != ChangeFamily::Parameter || history.is_quiet() {
        return;
    }
    let Some(ChangePicture::Values { values }) = picture.cloned() else { return };
    let Some(device) = device.and_then(Value::as_str).map(str::to_owned) else { return };
    if !values.iter().any(|value| pictures::curve_parameter(&value.name)) {
        return;
    }
    let first = {
        let mut runs = history.curve_runs.borrow_mut();
        let run = runs.entry(device.clone()).or_default();
        run.waiting.push((id.to_owned(), values));
        run.last = perf_now();
        run.waiting.len() == 1 && run.reading.is_empty()
    };
    if !first {
        return;
    }
    let history = history.clone();
    tokio::task::spawn_local(timing::background(async move {
        // One task a device: it reads again for changes that came while it read.
        loop {
            let began = perf_now();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let Some(last) = history.curve_runs.borrow().get(&device).map(|run| run.last) else { return };
                if perf_now() - last >= CURVE_QUIET_MS || perf_now() - began >= CURVE_WAIT_MS {
                    break;
                }
            }
            let changes = {
                let mut runs = history.curve_runs.borrow_mut();
                let Some(run) = runs.get_mut(&device) else { return };
                run.reading = std::mem::take(&mut run.waiting);
                run.reading.clone()
            };
            let connection = &history.connection;
            let signal = abort::any([connection.lifetime.clone(), abort::timeout(5_000)]);
            let fields = ["name", "value", "min", "max", "displayValue"].map(str::to_owned).to_vec();
            let read = read_parameters(connection, json!(device), fields, signal).await;
            let newer = {
                let mut runs = history.curve_runs.borrow_mut();
                let Some(run) = runs.get_mut(&device) else { return };
                run.reading.clear();
                run.waiting.clone()
            };
            if let Ok(rows) = read {
                let rows: Vec<pictures::ParameterRow> = rows.iter().filter_map(pictures::parameter_row).collect();
                let applied =
                    |id: &str| history.entries.borrow().get(id).is_some_and(|entry| entry.borrow().record.state == ChangeState::Applied);
                for (id, curve) in pictures::curves_for(&rows, &changes, &newer, applied) {
                    if let Some(curve) = curve.bounded() {
                        history.set_picture(&id, curve);
                    }
                }
            }
            let mut runs = history.curve_runs.borrow_mut();
            if runs.get(&device).is_none_or(|run| run.waiting.is_empty()) {
                runs.remove(&device);
                return;
            }
        }
    }));
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
fn display_map(row: &JsonObject) -> Option<DisplayMap> {
    Some(DisplayMap {
        min: row.get("min")?.as_f64()?,
        max: row.get("max")?.as_f64()?,
        items: Some(
            row.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
        ),
        grid: row
            .get("grid")?
            .as_array()?
            .iter()
            .filter_map(|v| {
                let a = v.as_array()?;
                Some((a.first()?.as_f64()?, a.get(1)?.as_str()?.to_owned()))
            })
            .collect(),
    })
}
fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(a) => a.iter().map(|v| if v.is_null() { String::new() } else { js_string(v) }).collect::<Vec<_>>().join(","),
        _ => stringify(value),
    }
}
