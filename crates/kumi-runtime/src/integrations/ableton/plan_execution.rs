//! Plans run in arrival order, batching adjacent changes and settling Live when interrupted.
use super::{
    actions::{ActionSummary, ACTIONS},
    changes::{ChangeKind, CHANGES},
    context::{object, payload},
    mutations::{track_index_of, Mutations},
    plan_stream::{step_scanner, StepScanner},
    views::ViewHost,
};
use crate::core::{
    contracts::{ChangeFamily, JsonObject, StreamingCall, ToolResult},
    errors::RuntimeError,
};
use async_trait::async_trait;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
        string::head,
    },
};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};
use tokio::sync::Notify;

const MAX_CHANGES: usize = 5_000;
const STEP_COUNT: &str = "Give 1 to 5000 steps in all.";
const REFUSED_NOTE: &str = "Nothing changed for the refused steps. The held-back ones needed one of them (its @name, its track, or the Set's tracks and scenes), so they didn't run; every other step did. Fix what was refused and send only those steps, with the held-back ones, in one more make_changes: the @names this answer made still work there.";
fn row(value: &Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(a)) => {
            a.iter().map(|v| if v.is_null() { String::new() } else { js_string(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(v) => stringify(v),
    }
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
pub fn expand_step(raw: &Value, index: usize) -> Result<Vec<Value>, String> {
    let item = row(raw);
    let each = item.get("each").and_then(Value::as_object);
    let Some(each) = each.filter(|e| !e.is_empty()) else { return Ok(vec![raw.clone()]) };
    let runs = each.values().next().and_then(Value::as_array).map(Vec::len);
    let Some(runs) = runs.filter(|n| each.values().all(|v| v.as_array().is_some_and(|v| v.len() == *n))) else {
        return Err(format!("Step {}: each gives input fields lists of one length, one value per run.", index + 1));
    };
    let base = item.get("input").map(row).unwrap_or_default();
    Ok((0..runs.min(MAX_CHANGES + 1))
        .map(|run| {
            let mut input = base.clone();
            for (field, values) in each {
                input.insert(field.clone(), values[run].clone());
            }
            let mut step = JsonObject::new();
            if let Some(tool) = item.get("tool") {
                step.insert("tool".into(), tool.clone());
            }
            step.insert("input".into(), json!(input));
            json!(step)
        })
        .collect())
}
impl Mutations {
    pub async fn make_changes(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let mut steps = Vec::new();
        for (index, raw) in input.get("steps").and_then(Value::as_array).into_iter().flatten().enumerate() {
            match expand_step(raw, index) {
                Ok(expanded) => steps.extend(expanded),
                Err(error) => return Ok(ToolResult::error(error)),
            }
            if steps.len() > MAX_CHANGES {
                break;
            }
        }
        if steps.is_empty() || steps.len() > MAX_CHANGES {
            return Ok(ToolResult::error(STEP_COUNT));
        }
        let plan = Plan::new(self.clone(), signal, None);
        plan.add(steps);
        plan.close();
        plan.result(input.get("final") == Some(&json!(true))).await
    }
    pub fn stream_changes(self: &Rc<Self>, signal: Signal, on_start: Rc<dyn Fn()>) -> Box<dyn StreamingCall> {
        let plan = Plan::new(self.clone(), signal.clone(), Some(on_start));
        let received = Rc::new(RefCell::new(Vec::<Value>::new()));
        let taking = plan.clone();
        let arrivals = received.clone();
        let take: Rc<dyn Fn(Value)> = Rc::new(move |raw| {
            let index = arrivals.borrow().len();
            arrivals.borrow_mut().push(raw.clone());
            if taking.failed() {
                return;
            }
            match expand_step(&raw, index) {
                Err(error) => taking.fail(error),
                Ok(expanded) => {
                    if taking.count() + expanded.len() > MAX_CHANGES {
                        taking.fail(STEP_COUNT.into());
                    } else {
                        taking.add(expanded);
                    }
                }
            }
        });
        let scan_take = take.clone();
        let callback: Box<dyn FnMut(Value)> = Box::new(move |value| scan_take(value));
        Box::new(StreamingPlan { mutations: self.clone(), signal, plan, received, take, scanner: RefCell::new(step_scanner(callback)) })
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Stop {
    pub step: usize,
    pub tool: Value,
    pub error: String,
}
#[derive(Clone)]
struct Failure {
    at: usize,
    error: String,
}
#[derive(Default)]
struct State {
    made: IndexMap<String, String>,
    done: Vec<Value>,
    missed: Vec<Value>,
    /// Steps refused with nothing changed, and the later ones held back with them (#259): the plan went on with the
    /// rest, so the model resends a few steps, not the whole plan.
    refused: Vec<Value>,
    dependents: Vec<Value>,
    blocked: Blocked,
    /// What Kumi said of stopping Live's playback or recording after a plan with refused steps.
    quieted: Option<String>,
    parameters_on: IndexMap<String, Value>,
    copy: Option<String>,
    copy_checked: bool,
    playing: bool,
    recording: Option<String>,
    undo_step: Option<String>,
}
/// What refused steps leave for the steps after them, each with the step it hangs on: the `@names` they'd have made,
/// the tracks they'd have changed (by the tracks' short names, which follow them through a restructure), and whether
/// they'd have changed the Set's tracks and scenes or its transport. A later step that needs any of it is held back:
/// it may have counted on the refused one (a track's clip on its sample, the Sub track added after deleting the 808).
#[derive(Default)]
struct Blocked {
    names: IndexMap<String, usize>,
    tracks: IndexMap<String, usize>,
    structure: Option<usize>,
    transport: Option<usize>,
}
/// Actions that start, stop or move what Live plays: a `wait` or a stop after a refused one waits for nothing.
const TRANSPORT: [&str; 5] = ["play", "fire_scene", "launch_clip", "record", "jump_to_locator"];
type Settled = Shared<LocalBoxFuture<'static, Result<Option<Stop>, RuntimeError>>>;
pub struct Plan {
    mutations: Rc<Mutations>,
    signal: Signal,
    on_start: Option<Rc<dyn Fn()>>,
    steps: RefCell<Vec<Value>>,
    closed: Cell<bool>,
    abandoned: Cell<bool>,
    started: Cell<bool>,
    failure: RefCell<Option<Failure>>,
    notify: Notify,
    settled: RefCell<Option<Settled>>,
    state: RefCell<State>,
}
impl Plan {
    pub fn new(mutations: Rc<Mutations>, signal: Signal, on_start: Option<Rc<dyn Fn()>>) -> Rc<Self> {
        let this = Rc::new(Self {
            mutations,
            signal,
            on_start,
            steps: RefCell::new(Vec::new()),
            closed: Cell::new(false),
            abandoned: Cell::new(false),
            started: Cell::new(false),
            failure: RefCell::new(None),
            notify: Notify::new(),
            settled: RefCell::new(None),
            state: RefCell::new(State::default()),
        });
        let running = this.clone();
        let settled = async move { running.run().await }.boxed_local().shared();
        *this.settled.borrow_mut() = Some(settled.clone());
        tokio::task::spawn_local(async move {
            let _ = settled.await;
        });
        this
    }
    pub fn started(&self) -> bool {
        self.started.get()
    }
    pub fn failed(&self) -> bool {
        self.failure.borrow().is_some()
    }
    pub fn count(&self) -> usize {
        self.steps.borrow().len()
    }
    pub fn add(&self, more: Vec<Value>) {
        if !self.closed.get() && !self.failed() {
            self.steps.borrow_mut().extend(more);
            self.notify.notify_waiters();
        }
    }
    pub fn fail(&self, error: String) {
        if !self.failed() {
            *self.failure.borrow_mut() = Some(Failure { at: self.count(), error });
            self.notify.notify_waiters();
        }
    }
    pub fn close(&self) {
        self.closed.set(true);
        self.notify.notify_waiters();
    }
    pub async fn abandon(&self) {
        self.set_aside();
        let _ = self.pending().await;
    }
    /// Start nothing more (without waiting for what's under way).
    fn set_aside(&self) {
        self.abandoned.set(true);
        self.notify.notify_waiters();
    }
    fn pending(&self) -> Settled {
        self.settled.borrow().as_ref().unwrap().clone()
    }
    fn known(&self, index: usize) -> bool {
        self.count() > index || self.closed.get() || self.failed() || self.abandoned.get() || self.signal.is_cancelled()
    }
    async fn until_known(&self, index: usize) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.known(index) {
                return;
            }
            tokio::select! {_=notified=>{},_=self.signal.cancelled()=>{}}
        }
    }
    /// The refused step a step hangs on, if it does: it uses an `@name` one would have made, changes a track one would
    /// have changed, or reshapes the Set's tracks and scenes, or its transport, after one that would have.
    fn hangs_on(&self, item: &JsonObject, state: &State) -> Option<usize> {
        let blocked = &state.blocked;
        let input = item.get("input").cloned().unwrap_or(Value::Null);
        let mut names = Vec::new();
        names_in(&input, &mut names, 0);
        if let Some(on) = names.iter().find_map(|name| blocked.names.get(name)) {
            return Some(*on);
        }
        if let Some(on) = self.tracks_in(&input, &state.made).iter().find_map(|track| blocked.tracks.get(track)) {
            return Some(*on);
        }
        let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
        if let Some(on) = blocked.structure.filter(|_| structural(tool)) {
            return Some(on);
        }
        blocked.transport.filter(|_| tool == "wait" || TRANSPORT.contains(&tool))
    }
    /// Steps refused with nothing changed (`run` of them from `at`, a batch as one): reported with why, and what they'd
    /// have made or changed holds back the later steps that need it. The rest of the plan goes on.
    fn refuse(&self, state: &mut State, at: usize, run: usize, error: String) {
        let step = at + 1;
        let items: Vec<JsonObject> = self.steps.borrow()[at..at + run].iter().map(row).collect();
        let tool = items[0].get("tool").cloned().unwrap_or(Value::Null);
        let error = head(&error, 600);
        state.refused.push(if run > 1 {
            json!({"steps":format!("{step}–{}", step + run - 1),"tool":tool,"error":error})
        } else {
            json!({"step":step,"tool":tool,"error":error})
        });
        for item in &items {
            if let Some(name) = item.get("as").and_then(Value::as_str) {
                state.blocked.names.insert(format!("@{name}"), step);
            }
            for track in self.tracks_in(item.get("input").unwrap_or(&Value::Null), &state.made) {
                state.blocked.tracks.entry(track).or_insert(step);
            }
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
            if structural(tool) {
                state.blocked.structure.get_or_insert(step);
            }
            if TRANSPORT.contains(&tool) {
                state.blocked.transport.get_or_insert(step);
            }
        }
    }
    /// The tracks a step's refs are on, by the tracks' short names (`@names` read through what the plan made).
    fn tracks_in(&self, input: &Value, made: &IndexMap<String, String>) -> Vec<String> {
        fn walk(value: &Value, key: &str, out: &mut Vec<String>, depth: usize) {
            if depth > 8 {
                return;
            }
            match value {
                Value::String(text) if key == "ref" || key == "parent" || key.ends_with("Ref") || key.ends_with("Refs") => {
                    out.push(text.clone())
                }
                Value::Array(items) => items.iter().for_each(|item| walk(item, key, out, depth + 1)),
                Value::Object(row) => row.iter().for_each(|(key, value)| walk(value, key, out, depth + 1)),
                _ => {}
            }
        }
        let mut references = Vec::new();
        walk(input, "", &mut references, 0);
        let mut book = self.mutations.parameters.history.connection.references.borrow_mut();
        let mut tracks = Vec::new();
        for reference in references {
            let reference = match reference.strip_prefix('@') {
                Some(name) => match made.get(name) {
                    Some(made) => made.clone(),
                    None => continue,
                },
                None => reference,
            };
            let long = book.lengthen(&json!({"ref": reference}))["ref"].as_str().unwrap_or_default().to_owned();
            let (Some(epoch), Some(index)) = (long.split(':').next().filter(|e| e.parse::<u64>().is_ok()), track_index_of(&long)) else {
                continue;
            };
            let track = book.short_ref(&format!("{epoch}:track:{}", to_string(index)));
            if !tracks.contains(&track) {
                tracks.push(track);
            }
        }
        tracks
    }
    async fn run(&self) -> Result<Option<Stop>, RuntimeError> {
        let mut state = State::default();
        // What earlier plans in this answer made: a plan resending refused steps uses their @names.
        state.made = self.mutations.names.borrow().clone();
        let mut outcome = self.steps(&mut state).await;
        // A plan with refused steps didn't finish either: what it started playing or recording may have had its stop
        // among them.
        let finished = matches!(outcome, Ok(None)) && !self.abandoned.get() && state.refused.is_empty();
        if let Some(id) = state.undo_step.take().filter(|s| !s.is_empty()) {
            let connection = &self.mutations.parameters.history.connection;
            let signal = abort::any([connection.lifetime.clone(), abort::timeout(10_000)]);
            let _ = connection.call("live_undo_step_end", object(&json!({"stepId":id}))?, signal).await;
        }
        if !finished {
            if let Some(note) = self.quiet(&state).await {
                match &mut outcome {
                    Ok(Some(stopped)) => stopped.error = head(&format!("{} {note}", stopped.error), 800),
                    _ => state.quieted = Some(note),
                }
            }
        }
        *self.state.borrow_mut() = state;
        outcome
    }
    async fn open_undo_step(&self, state: &mut State) {
        let connection = &self.mutations.parameters.history.connection;
        if state.undo_step.is_some() || !connection.has("live_undo_step_begin") || !connection.has("live_undo_step_end") {
            return;
        }
        state.undo_step = Some(String::new());
        let signal = abort::any([connection.lifetime.clone(), abort::timeout(10_000)]);
        if let Ok(opened) = connection.call("live_undo_step_begin", row(&json!({"label":"Kumi","timeoutMs":600_000})), signal).await {
            if let Some(id) = payload(&opened).ok().and_then(|p| p.get("stepId").and_then(Value::as_str).map(str::to_owned)) {
                state.undo_step = Some(id);
            }
        }
    }
    async fn quiet(&self, state: &State) -> Option<String> {
        let recording = state.recording.as_ref().is_some_and(|s| !s.is_empty());
        if !recording && !state.playing {
            return None;
        }
        let history = &self.mutations.parameters.history;
        if !history.stop_everything(history.change_signal()).await {
            let doing = if recording { "recording" } else { "playing" };
            self.mutations.emit_action(&ActionSummary {
                title: format!("Live may still be {doing}: press space in Live, or /stop"),
                playing: None,
                recording: None,
            });
            return Some(format!("The plan didn't finish and Live may still be {doing}; tell the producer."));
        }
        self.mutations.emit_action(&ActionSummary {
            title: if recording { "Recording stopped" } else { "Stopped" }.into(),
            playing: Some(false),
            recording: Some(false),
        });
        Some(format!("Kumi stopped {}, since the plan didn't finish.", if recording { "the recording and playback" } else { "playback" }))
    }
    async fn steps(&self, state: &mut State) -> Result<Option<Stop>, RuntimeError> {
        let mut index = 0;
        loop {
            self.until_known(index).await;
            if self.abandoned.get() {
                return Ok(None);
            }
            self.signal.check()?;
            let Some(raw) = self.steps.borrow().get(index).cloned() else {
                return Ok(self.failure.borrow().as_ref().filter(|f| f.at == index).map(|f| Stop {
                    step: index + 1,
                    tool: Value::Null,
                    error: f.error.clone(),
                }));
            };
            let item = row(&raw);
            let step = index + 1;
            if let Some(on) = self.hangs_on(&item, state) {
                state.held_back(step, &item, on);
                index += 1;
                continue;
            }
            let stop =
                |error: String| Some(Stop { step, tool: item.get("tool").cloned().unwrap_or(Value::Null), error: head(&error, 600) });
            let confirmed = !state.done.is_empty();
            let device = item.get("input").and_then(|v| v.get("deviceRef"));
            let mut run = 1;
            if Batch::ALL.iter().any(|b| b.matches(&raw, None)) {
                self.until_known(index + 1).await;
                if self.abandoned.get() {
                    return Ok(None);
                }
                self.signal.check()?;
            }
            let next = self.steps.borrow().get(index + 1).cloned().unwrap_or(Value::Null);
            let batch = Batch::ALL.iter().find(|b| b.matches(&raw, None) && b.matches(&next, device));
            if let Some(batch) = batch {
                let connection = &self.mutations.parameters.history.connection;
                if connection.ensure_catalog(self.signal.clone()).await.is_err() {
                    self.signal.check()?;
                }
                if batch.offered(self.mutations.as_ref()) {
                    while run < batch.most {
                        self.until_known(index + run).await;
                        if self.abandoned.get() {
                            return Ok(None);
                        }
                        self.signal.check()?;
                        if self.steps.borrow().get(index + run).is_some_and(|v| batch.matches(v, device)) {
                            run += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            // Set aside while it waited on Live (the catalog above): nothing starts after that.
            if self.abandoned.get() {
                return Ok(None);
            }
            if !self.started.replace(true) {
                if let Some(on_start) = &self.on_start {
                    on_start();
                }
            }
            let tool = js_string(item.get("tool"));
            // Before a step that can lose the producer's material: a delete, a cleared range, or a move,
            // which replaces what's in its new place.
            if !state.copy_checked && (index >= 2 || tool.starts_with("delete_") || ["clear_range", "move_clip"].contains(&tool.as_str())) {
                state.copy_checked = true;
                state.copy = self.mutations.keep_copy(self.signal.clone()).await?;
            }
            self.open_undo_step(state).await;
            if let Some(batch) = batch.filter(|_| run > 1) {
                let inputs: Result<Vec<_>, _> = self.steps.borrow()[index..index + run]
                    .iter()
                    .enumerate()
                    .map(|(offset, v)| resolve(v.get("input").unwrap_or(&Value::Null), step + offset, &state.made).map(|v| row(&v)))
                    .collect();
                let inputs = match inputs {
                    Ok(v) => v,
                    Err(error) => {
                        self.refuse(state, index, run, error);
                        index += run;
                        continue;
                    }
                };
                let outcome = self.mutations.change(batch.kind(), batch.input(&inputs), self.signal.clone(), confirmed).await;
                let reply = serde_json::from_str::<Value>(&outcome.text).map(|v| row(&v)).unwrap_or_default();
                if outcome.is_error && outcome.missed.unwrap_or(0) == 0 {
                    let error = format!("{} {step}–{}, as one change: {}", batch.what, step + run - 1, outcome.text);
                    if outcome.stops {
                        return Ok(stop(error));
                    }
                    self.refuse(state, index, run, error);
                    index += run;
                    continue;
                }
                if outcome.missed.unwrap_or(0) > 0 {
                    record_miss(state, format!("{step}–{}", step + run - 1), inputs[0].get("deviceRef"), &reply);
                }
                for offset in 0..run {
                    let changed = reply
                        .get("lines")
                        .and_then(Value::as_array)
                        .and_then(|a| a.get(offset))
                        .filter(|v| v.is_string())
                        .or_else(|| reply.get("changed"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    state.done.push(json!({"step":step+offset,"changed":changed,"change":reply.get("change").unwrap_or(&Value::Null)}));
                }
                index += run;
                continue;
            }
            let tool = item.get("tool").and_then(Value::as_str);
            let kind = CHANGES.iter().find(|k| Some(k.tool.as_str()) == tool && k.internal != Some(true));
            let action = if kind.is_none() { ACTIONS.iter().find(|k| Some(k.tool.as_str()) == tool) } else { None };
            if kind.is_none() && action.is_none() && tool != Some("wait") {
                self.refuse(state, index, 1, format!("{} isn't one of Kumi's change tools", head(&js_string(item.get("tool")), 64)));
                index += 1;
                continue;
            }
            let since = kind.and_then(|k| k.since.as_deref()).or_else(|| action.and_then(|k| k.since.as_deref()));
            if !self.mutations.supported(since) {
                self.refuse(state, index, 1, self.mutations.too_old(since));
                index += 1;
                continue;
            }
            let input = match resolve(&json!(item.get("input").map(row).unwrap_or_default()), step, &state.made) {
                Ok(v) => row(&v),
                Err(error) => {
                    self.refuse(state, index, 1, error);
                    index += 1;
                    continue;
                }
            };
            if tool == Some("wait") {
                let seconds = input
                    .get("seconds")
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        input
                            .get("beats")
                            .and_then(Value::as_f64)
                            .and_then(|beats| self.mutations.observer.tempo.get().filter(|t| *t != 0.0).map(|tempo| beats * 60.0 / tempo))
                    })
                    .unwrap_or(f64::NAN);
                if !(seconds > 0.0 && seconds <= 1800.0) {
                    self.refuse(state, index, 1, "wait takes seconds (up to 1800) or beats".into());
                    index += 1;
                    continue;
                }
                // Node's timer floors fractional milliseconds and uses one millisecond for values below one.
                let milliseconds = (seconds * 1000.0).floor().max(1.0) as u64;
                tokio::select! {biased;_=self.signal.cancelled()=>return Err(RuntimeError::Aborted),_=tokio::time::sleep(Duration::from_millis(milliseconds))=>{}}
                state
                    .done
                    .push(json!({"step":step,"changed":format!("waited {} s",to_string(round(seconds*10.0)/10.0)),"change":Value::Null}));
                index += 1;
                continue;
            }
            if let Some(action) = action {
                let outcome = self.mutations.act(action, input.clone(), self.signal.clone(), false).await;
                if !outcome.is_error || outcome.maybe == Some(true) {
                    if let Some(done) = &outcome.done {
                        if let Some(playing) = done.playing {
                            state.playing = playing || (state.playing && outcome.maybe == Some(true));
                        }
                        if let Some(recording) = done.recording {
                            if recording {
                                state.recording =
                                    Some(js_string(input.get("lane").filter(|v| !v.is_null()).or(Some(&json!("arrangement")))));
                            } else if outcome.maybe != Some(true) {
                                state.recording = None;
                            }
                        }
                    }
                }
                if outcome.is_error {
                    // Live may have done it: what comes after can't be sure where Live is.
                    if outcome.maybe == Some(true) {
                        return Ok(stop(outcome.text));
                    }
                    self.refuse(state, index, 1, outcome.text);
                    index += 1;
                    continue;
                }
                state.done.push(json!({"step":step,"changed":outcome.done.map(|d|d.title),"change":Value::Null}));
                index += 1;
                continue;
            }
            let outcome = self.mutations.change(kind.unwrap(), input.clone(), self.signal.clone(), confirmed).await;
            let reply = serde_json::from_str::<Value>(&outcome.text).map(|v| row(&v)).unwrap_or_default();
            if outcome.is_error && outcome.missed.unwrap_or(0) == 0 {
                let text = reply.get("changed").and_then(Value::as_str).map(|s| format!("{s}: {}", outcome.text)).unwrap_or(outcome.text);
                if outcome.stops {
                    return Ok(stop(text));
                }
                self.refuse(state, index, 1, text);
                index += 1;
                continue;
            }
            if outcome.missed.unwrap_or(0) > 0 {
                record_miss(state, step.to_string(), input.get("deviceRef"), &reply);
            }
            if let (Some(name), Some(reference)) = (item.get("as").and_then(Value::as_str), reply.get("ref").and_then(Value::as_str)) {
                state.made.insert(name.into(), reference.into());
                self.mutations.names.borrow_mut().insert(name.into(), reference.into());
            }
            let mut done = row(
                &json!({"step":step,"changed":reply.get("changed").unwrap_or(&Value::Null),"change":reply.get("change").unwrap_or(&Value::Null)}),
            );
            if let Some(reference) = reply.get("ref").filter(|v| v.is_string()) {
                done.insert("ref".into(), reference.clone());
            }
            if let Some(lines) = reply.get("lines").filter(|v| v.is_array()) {
                done.insert("lines".into(), lines.clone());
            }
            state.done.push(json!(done));
            index += 1;
        }
    }
    pub async fn result(&self, final_: bool) -> Result<ToolResult, RuntimeError> {
        let stopped = self.pending().await?;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed.get() || self.abandoned.get() {
                break;
            }
            notified.await;
        }
        let state = self.state.borrow();
        if stopped.is_some() && state.done.is_empty() {
            if let Some(failure) = self.failure.borrow().as_ref().filter(|f| f.at == 0) {
                return Ok(ToolResult::error(&failure.error));
            }
        }
        let mut reply = row(&json!({"done":state.done}));
        if !state.refused.is_empty() {
            reply.insert("refused".into(), json!(state.refused));
            if !state.dependents.is_empty() {
                reply.insert("heldBack".into(), json!(state.dependents));
            }
            reply.insert("refusedNote".into(), json!(REFUSED_NOTE));
            if let Some(note) = &state.quieted {
                reply.insert("playback".into(), json!(note));
            }
        }
        if let Some(stopped) = &stopped {
            reply.insert("stopped".into(), json!(stopped));
            if self.count() > stopped.step {
                reply.insert("skipped".into(), json!(self.count() - stopped.step));
            }
        }
        if !state.missed.is_empty() {
            reply.insert("missed".into(), json!(state.missed));
            reply.insert(
                "parametersOnDevice".into(),
                Value::Object(state.parameters_on.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            );
            reply.insert("missedNote".into(),json!("These parameters weren't set (the rest of the plan was): name them as the device has them (parametersOnDevice), or give values it can take, in one more make_changes."));
        }
        if let Some(copy) = state.copy.as_ref().filter(|s| !s.is_empty()) {
            reply.insert("copy".into(), json!(copy));
            reply.insert("copyNote".into(),json!("Before this, Kumi kept a copy of the Set as last saved, next to it. Tell the producer in a few words, with the file's name."));
        }
        let text = stringify(&json!(reply));
        if stopped.is_some() || !state.refused.is_empty() {
            return Ok(ToolResult::error(text));
        }
        if state.done.is_empty() {
            return Ok(ToolResult::error(STEP_COUNT));
        }
        let reply = if final_ && state.missed.is_empty() {
            let lines: Vec<_> = state
                .done
                .iter()
                .flat_map(|item| {
                    item.get("lines")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_else(|| vec![item.get("changed").cloned().unwrap_or(Value::Null)])
                })
                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                .collect();
            let mut text = if lines.len() == 1 {
                format!("Done: {}.", lines[0])
            } else {
                format!("Done:\n{}", lines.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"))
            };
            if let Some(copy) = state.copy.as_ref().filter(|s| !s.is_empty()) {
                text.push_str(&format!(
                    "\n\nFirst, Kumi kept a copy of your Set as last saved, next to it: {}",
                    Path::new(copy).file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            Some(text)
        } else {
            None
        };
        Ok(ToolResult { text, reply, ..Default::default() })
    }
}
impl State {
    /// A step held back because it needed a refused one: what it would have made holds back its own dependents.
    fn held_back(&mut self, step: usize, item: &JsonObject, on: usize) {
        self.dependents.push(json!({"step":step,"tool":item.get("tool").cloned().unwrap_or(Value::Null),"after":on}));
        if let Some(name) = item.get("as").and_then(Value::as_str) {
            self.blocked.names.insert(format!("@{name}"), on);
        }
        let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
        if structural(tool) {
            self.blocked.structure.get_or_insert(on);
        }
    }
}
/// The `@names` a step's input uses.
fn names_in(value: &Value, out: &mut Vec<String>, depth: usize) {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^@[A-Za-z][A-Za-z0-9_]{0,31}$").unwrap());
    if depth > 8 {
        return;
    }
    match value {
        Value::String(text) if NAMED.is_match(text) => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| names_in(item, out, depth + 1)),
        Value::Object(row) => row.values().for_each(|item| names_in(item, out, depth + 1)),
        _ => {}
    }
}
/// Whether a tool adds, deletes or moves the Set's tracks or scenes.
fn structural(tool: &str) -> bool {
    CHANGES.iter().any(|kind| kind.tool == tool && kind.family == ChangeFamily::Structure)
}
fn record_miss(state: &mut State, steps: String, device: Option<&Value>, reply: &JsonObject) {
    state
        .missed
        .push(json!({"steps":steps,"deviceRef":device.unwrap_or(&Value::Null),"missed":reply.get("missed").unwrap_or(&Value::Null)}));
    if let (Some(device), Some(parameters)) = (device.and_then(Value::as_str), reply.get("parametersOnDevice").filter(|v| truthy(v))) {
        state.parameters_on.insert(device.into(), parameters.clone());
    }
}
fn resolve(value: &Value, step: usize, made: &IndexMap<String, String>) -> Result<Value, String> {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^@[A-Za-z][A-Za-z0-9_]{0,31}$").unwrap());
    match value {
        Value::String(s) if NAMED.is_match(s) => made
            .get(&s[1..])
            .filter(|s| !s.is_empty())
            .map(|s| json!(s))
            .ok_or_else(|| format!("step {step} refers to {s}, which no earlier step made")),
        Value::Array(a) => a.iter().map(|v| resolve(v, step, made)).collect::<Result<Vec<_>, _>>().map(Value::Array),
        Value::Object(o) => {
            o.iter().map(|(k, v)| Ok((k.clone(), resolve(v, step, made)?))).collect::<Result<JsonObject, String>>().map(Value::Object)
        }
        v => Ok(v.clone()),
    }
}
struct Batch {
    tool: &'static str,
    kind: &'static str,
    most: usize,
    what: &'static str,
}
impl Batch {
    const ALL: [Self; 2] = [
        Self { tool: "load_sample_to_pad", kind: "load_samples_to_pads", most: 16, what: "pads" },
        Self { tool: "set_device_parameter", kind: "set_device_parameters", most: 64, what: "parameters" },
    ];
    fn kind(&self) -> &'static ChangeKind {
        CHANGES.iter().find(|kind| kind.tool == self.kind).unwrap()
    }
    fn matches(&self, value: &Value, device: Option<&Value>) -> bool {
        let item = row(value);
        let input = item.get("input").map(row).unwrap_or_default();
        item.get("tool").and_then(Value::as_str) == Some(self.tool)
            && !item.contains_key("as")
            && input.get("deviceRef").is_some_and(|v| v.is_string() && device.is_none_or(|device| v == device))
    }
    fn offered(&self, mutations: &Mutations) -> bool {
        let properties = mutations
            .parameters
            .history
            .connection
            .tools()
            .and_then(|tools| tools.tool(&self.kind().preview))
            .and_then(|tool| tool.input_schema.properties)
            .unwrap_or_default();
        if self.tool == "load_sample_to_pad" {
            properties
                .get("action")
                .and_then(|v| v.get("enum"))
                .and_then(Value::as_array)
                .is_some_and(|a| a.contains(&json!("load-samples")))
        } else {
            properties.contains_key("values")
        }
    }
    fn input(&self, group: &[JsonObject]) -> JsonObject {
        let mut input = row(&json!({"deviceRef":group[0].get("deviceRef").unwrap_or(&Value::Null)}));
        let values: Vec<_> = group
            .iter()
            .map(|item| {
                if self.tool == "load_sample_to_pad" {
                    let mut pad =
                        row(&json!({"note":item.get("note").unwrap_or(&Value::Null),"sample":item.get("sample").unwrap_or(&Value::Null)}));
                    if item.get("instrument").and_then(Value::as_str) == Some("Drum Sampler") {
                        pad.insert("instrument".into(), json!("Drum Sampler"));
                    }
                    json!(pad)
                } else {
                    let mut param = JsonObject::new();
                    if item.get("parameter").is_some_and(Value::is_string) && !item.contains_key("parameterRef") {
                        param.insert("parameter".into(), item["parameter"].clone());
                    } else {
                        param.insert("parameterRef".into(), item.get("parameterRef").cloned().unwrap_or(Value::Null));
                    }
                    param.insert("value".into(), item.get("value").cloned().unwrap_or(Value::Null));
                    json!(param)
                }
            })
            .collect();
        input.insert(if self.tool == "load_sample_to_pad" { "pads" } else { "values" }.into(), json!(values));
        input
    }
}
struct StreamingPlan {
    mutations: Rc<Mutations>,
    signal: Signal,
    plan: Rc<Plan>,
    received: Rc<RefCell<Vec<Value>>>,
    take: Rc<dyn Fn(Value)>,
    scanner: RefCell<StepScanner<Box<dyn FnMut(Value)>>>,
}
/// A plan dropped unfinished (its reply set aside) starts nothing more: its task would otherwise wait for steps that
/// never come, or start one it already had. After `finish` the plan has settled, so this changes nothing.
impl Drop for StreamingPlan {
    fn drop(&mut self) {
        self.plan.set_aside();
    }
}
#[async_trait(?Send)]
impl StreamingCall for StreamingPlan {
    fn push(&self, delta: &str) {
        self.scanner.borrow_mut().push(delta);
    }
    fn started(&self) -> bool {
        self.plan.started()
    }
    async fn abandon(&self) {
        self.plan.abandon().await;
    }
    async fn finish(&self, input: Option<JsonObject>) -> Result<ToolResult, RuntimeError> {
        if self.received.borrow().is_empty() {
            let plan = self.plan.clone();
            tokio::task::spawn_local(async move {
                plan.abandon().await;
            });
            return match input {
                Some(input) => self.mutations.make_changes(input, self.signal.clone()).await,
                None => Ok(ToolResult::error("Tool arguments must be a JSON object.")),
            };
        }
        if let Some(input) = &input {
            let all = input.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
            let changed =
                self.received.borrow().iter().enumerate().any(|(i, raw)| all.get(i).is_none_or(|v| stringify(raw) != stringify(v)));
            if changed {
                self.plan.fail("The plan's steps changed as they were written, so it stopped there.".into());
            } else {
                let received = self.received.borrow().len();
                for raw in all.into_iter().skip(received) {
                    (self.take)(raw);
                }
            }
        } else {
            self.plan.fail("The rest of the plan wasn't valid JSON, so it stopped there.".into());
        }
        self.plan.close();
        self.plan.result(input.as_ref().and_then(|v| v.get("final")) == Some(&json!(true))).await
    }
}
