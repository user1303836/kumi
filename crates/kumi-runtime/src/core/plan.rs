//! Plans run in arrival order, batching adjacent changes and settling the DAW when interrupted, over the DAW's
//! PlanHost: its change kinds and actions, each step's change, its batches and its safety copy.
use crate::core::{
    contracts::{JsonObject, StreamingCall, ToolResult},
    errors::RuntimeError,
    history::{ChangeHost, History},
    plan_stream::{step_scanner, StepScanner},
    refs::RefScheme,
};
use async_trait::async_trait;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use indexmap::IndexMap;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
        string::head,
    },
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};
use tokio::sync::Notify;

/// The most changes one answer makes, however they come (a plan's steps, change tools; Live's and the other DAWs').
pub const MAX_CHANGES: usize = 5_000;
/// What a change past [`MAX_CHANGES`] in one answer says.
pub fn too_many_changes() -> String {
    format!("That's {MAX_CHANGES} changes in one answer; carry on in the next one")
}
const STEP_COUNT: &str = "Give 1 to 5000 steps in all.";
crate::daw_text!(
    /// Said with a plan's refused steps.
    pub REFUSED_NOTE, refused_note: "Nothing changed for the refused steps. The held-back ones needed one of them (its @name, its track, or the ", place, "'s ", structure, "), so they didn't run; every other step did. Fix what was refused and send only those steps, with the held-back ones, in one more ", plan_tool, ": the @names this answer made still work there."
);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeOutcome {
    pub text: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missed: Option<usize>,
    /// The DAW may have made the change, or Kumi can't read it to know: a plan stops here. An error without it is a
    /// refusal, with nothing changed, and the plan goes on with the steps that don't depend on it (#259).
    #[serde(skip)]
    pub stops: bool,
    /// What the DAW answered when it applied the change (the place it made, its rows): a backend composing on it reads it
    /// here (#401, no cell shared between changes).
    #[serde(skip)]
    pub applied: Option<Value>,
}
impl ChangeOutcome {
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true, missed: None, stops: false, applied: None }
    }
    /// An error after which nothing more should be tried: the DAW may have changed, or can't be read.
    pub fn stop(text: impl Into<String>) -> Self {
        Self { stops: true, ..Self::error(text) }
    }
    pub fn tool_result(self) -> ToolResult {
        ToolResult { text: self.text, is_error: self.is_error, ..Default::default() }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSummary {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording: Option<bool>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOutcome {
    pub text: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maybe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<ActionSummary>,
}
impl ActionOutcome {
    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true, maybe: None, done: None }
    }
}

/// One of the DAW's change kinds or actions, as a step names it.
pub trait StepKind {
    /// The bridge version it needs, if it needs a newer one than the first.
    fn since(&self) -> Option<&str>;
}
/// Adjacent steps the DAW takes as one change: up to `most` steps of `tool` on one `key` (one device's parameters),
/// made as one `kind`; `what` names them.
pub struct Batch {
    pub tool: &'static str,
    pub kind: &'static str,
    pub key: &'static str,
    pub most: usize,
    pub what: &'static str,
}
/// What a plan asks of a DAW for its steps.
#[async_trait(?Send)]
pub trait PlanHost: 'static {
    /// The DAW's HISTORY host: its words, refs, undo steps and emergency stop.
    type Changes: ChangeHost;
    type Change: StepKind + 'static;
    type Action: StepKind + 'static;
    fn history(&self) -> &History<Self::Changes>;
    /// The `@names` this answer's plans made (each the short ref of what its step made): a later plan in the answer
    /// uses them, so steps a plan refused are resent alone, not with everything they hang on (#259).
    fn names(&self) -> &RefCell<IndexMap<String, String>>;
    /// The change kind a step's tool names (one a step can name).
    fn change_kind(&self, tool: &str) -> Option<&'static Self::Change>;
    /// The action a step's tool names.
    fn action_kind(&self, tool: &str) -> Option<&'static Self::Action>;
    /// Whether a tool adds, deletes or moves the project's tracks.
    fn structural(&self, tool: &str) -> bool;
    /// Whether an action starts, stops or moves what the DAW plays: a `wait` or a stop after a refused one waits for
    /// nothing.
    fn moves_transport(&self, tool: &str) -> bool;
    /// Whether a step can lose the producer's material (a delete, a cleared range, a move onto what's there): the
    /// project's last saved file is copied before it.
    fn may_lose_material(&self, tool: &str) -> bool;
    /// Whether the bridge is new enough for what needs `since`.
    fn supported(&self, since: Option<&str>) -> bool;
    /// What Kumi says when it isn't.
    fn too_old(&self, since: Option<&str>) -> String;
    /// The tempo as last read, for a `wait` in beats.
    fn tempo(&self) -> Option<f64>;
    fn batches(&self) -> &[Batch];
    /// The change a batch makes.
    fn batch_kind(&self, batch: &Batch) -> &'static Self::Change;
    /// Whether the bridge takes a batch as one change.
    fn batch_offered(&self, batch: &Batch) -> bool;
    /// The one change's input for a batch's steps' inputs.
    fn batch_input(&self, batch: &Batch, group: &[JsonObject]) -> JsonObject;
    async fn change(&self, kind: &'static Self::Change, input: JsonObject, signal: Signal, confirmed: bool) -> ChangeOutcome;
    async fn act(&self, action: &'static Self::Action, input: JsonObject, signal: Signal) -> ActionOutcome;
    /// A copy of the project as last saved, next to it: its path, when one was made.
    async fn safety_copy(&self, signal: Signal) -> Result<Option<String>, RuntimeError>;
    /// Whether a plan past its second step keeps that copy first whatever the step (Live's rule); else only before a
    /// step that can lose material.
    fn copies_long_plans(&self) -> bool {
        true
    }
    /// Tells the producer what a plan did to the transport.
    fn emit_action(&self, done: &ActionSummary);
}

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
pub async fn make_changes<H: PlanHost>(host: &Rc<H>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
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
    let plan = Plan::new(host.clone(), signal, None);
    plan.add(steps);
    plan.close();
    plan.result(input.get("final") == Some(&json!(true))).await
}
pub fn stream_changes<H: PlanHost>(host: &Rc<H>, signal: Signal, on_start: Rc<dyn Fn()>) -> Box<dyn StreamingCall> {
    let plan = Plan::new(host.clone(), signal.clone(), Some(on_start));
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
    Box::new(StreamingPlan { host: host.clone(), signal, plan, received, take, scanner: RefCell::new(step_scanner(callback)) })
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
    /// What Kumi said of stopping the DAW's playback or recording after a plan with refused steps.
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
/// they'd have changed the project's tracks or its transport. A later step that needs any of it is held back: it may
/// have counted on the refused one (a track's clip on its sample, the Sub track added after deleting the 808).
#[derive(Default)]
struct Blocked {
    names: IndexMap<String, usize>,
    tracks: IndexMap<String, usize>,
    structure: Option<usize>,
    transport: Option<usize>,
}
type Settled = Shared<LocalBoxFuture<'static, Result<Option<Stop>, RuntimeError>>>;
pub struct Plan<H: PlanHost> {
    host: Rc<H>,
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
impl<H: PlanHost> Plan<H> {
    pub fn new(host: Rc<H>, signal: Signal, on_start: Option<Rc<dyn Fn()>>) -> Rc<Self> {
        let this = Rc::new(Self {
            host,
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
    /// have changed, or reshapes the project's tracks, or its transport, after one that would have.
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
        if let Some(on) = blocked.structure.filter(|_| self.host.structural(tool)) {
            return Some(on);
        }
        blocked.transport.filter(|_| tool == "wait" || self.host.moves_transport(tool))
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
            if self.host.structural(tool) {
                state.blocked.structure.get_or_insert(step);
            }
            if self.host.moves_transport(tool) {
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
        let mut book = self.host.history().host.book().borrow_mut();
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
            let Some(track) = book.scheme.track_of(&long) else {
                continue;
            };
            let track = book.short_ref(&track);
            if !tracks.contains(&track) {
                tracks.push(track);
            }
        }
        tracks
    }
    async fn run(&self) -> Result<Option<Stop>, RuntimeError> {
        // What earlier plans in this answer made: a plan resending refused steps uses their @names.
        let mut state = State { made: self.host.names().borrow().clone(), ..Default::default() };
        let mut outcome = self.steps(&mut state).await;
        // A plan with refused steps didn't finish either: what it started playing or recording may have had its stop
        // among them.
        let finished = matches!(outcome, Ok(None)) && !self.abandoned.get() && state.refused.is_empty();
        if let Some(id) = state.undo_step.take().filter(|s| !s.is_empty()) {
            self.host.history().host.step_end(id).await;
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
        let host = &self.host.history().host;
        if state.undo_step.is_some() || !host.plan_steps() {
            return;
        }
        state.undo_step = Some(String::new());
        if let Some(id) = host.step_begin().await {
            state.undo_step = Some(id);
        }
    }
    async fn quiet(&self, state: &State) -> Option<String> {
        let recording = state.recording.as_ref().is_some_and(|s| !s.is_empty());
        if !recording && !state.playing {
            return None;
        }
        let history = self.host.history();
        if !history.stop_everything(history.change_signal()).await {
            let doing = if recording { "recording" } else { "playing" };
            let app = history.host.profile().words.app;
            self.host.emit_action(&ActionSummary {
                title: format!("{app} may still be {doing}: press space in {app}, or /stop"),
                playing: None,
                recording: None,
            });
            return Some(format!("The plan didn't finish and {app} may still be {doing}; tell the producer."));
        }
        self.host.emit_action(&ActionSummary {
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
                let tracks = self.tracks_in(item.get("input").unwrap_or(&Value::Null), &state.made);
                state.held_back(step, &item, on, tracks, self.host.as_ref());
                index += 1;
                continue;
            }
            let stop =
                |error: String| Some(Stop { step, tool: item.get("tool").cloned().unwrap_or(Value::Null), error: head(&error, 600) });
            let confirmed = !state.done.is_empty();
            let key = |batch: &Batch| item.get("input").and_then(|v| v.get(batch.key));
            let mut run = 1;
            let batches = self.host.batches();
            if batches.iter().any(|b| b.matches(&raw, None)) {
                self.until_known(index + 1).await;
                if self.abandoned.get() {
                    return Ok(None);
                }
                self.signal.check()?;
            }
            let next = self.steps.borrow().get(index + 1).cloned().unwrap_or(Value::Null);
            let batch = batches.iter().find(|b| b.matches(&raw, None) && b.matches(&next, key(b)));
            if let Some(batch) = batch {
                if !self.host.history().host.ready(self.signal.clone()).await {
                    self.signal.check()?;
                }
                if self.host.batch_offered(batch) {
                    while run < batch.most {
                        self.until_known(index + run).await;
                        if self.abandoned.get() {
                            return Ok(None);
                        }
                        self.signal.check()?;
                        if self.steps.borrow().get(index + run).is_some_and(|v| batch.matches(v, key(batch))) {
                            run += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            // Set aside while it waited on the DAW (its tools above): nothing starts after that.
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
            if !state.copy_checked && ((index >= 2 && self.host.copies_long_plans()) || self.host.may_lose_material(&tool)) {
                state.copy_checked = true;
                state.copy = self.host.safety_copy(self.signal.clone()).await?;
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
                let outcome = self
                    .host
                    .change(self.host.batch_kind(batch), self.host.batch_input(batch, &inputs), self.signal.clone(), confirmed)
                    .await;
                let reply = serde_json::from_str::<Value>(&outcome.text).map(|v| row(&v)).unwrap_or_default();
                if outcome.stops || (outcome.is_error && outcome.missed.unwrap_or(0) == 0) {
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
            let kind = tool.and_then(|tool| self.host.change_kind(tool));
            let action = if kind.is_none() { tool.and_then(|tool| self.host.action_kind(tool)) } else { None };
            if kind.is_none() && action.is_none() && tool != Some("wait") {
                self.refuse(state, index, 1, format!("{} isn't one of Kumi's change tools", head(&js_string(item.get("tool")), 64)));
                index += 1;
                continue;
            }
            let since = kind.and_then(|k| k.since()).or_else(|| action.and_then(|k| k.since()));
            if !self.host.supported(since) {
                self.refuse(state, index, 1, self.host.too_old(since));
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
                            .and_then(|beats| self.host.tempo().filter(|t| *t != 0.0).map(|tempo| beats * 60.0 / tempo))
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
                let outcome = self.host.act(action, input.clone(), self.signal.clone()).await;
                if !outcome.is_error || outcome.maybe == Some(true) {
                    if let Some(done) = &outcome.done {
                        if let Some(playing) = done.playing {
                            state.playing = playing || (state.playing && outcome.maybe == Some(true));
                        }
                        if let Some(recording) = done.recording {
                            if recording {
                                state.recording = Some(js_string(input.get("lane").filter(|v| !v.is_null()).or(Some(&json!("timeline")))));
                            } else if outcome.maybe != Some(true) {
                                state.recording = None;
                            }
                        }
                    }
                }
                if outcome.is_error {
                    // The DAW may have done it: what comes after can't be sure where it is.
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
            let outcome = self.host.change(kind.unwrap(), input.clone(), self.signal.clone(), confirmed).await;
            let reply = serde_json::from_str::<Value>(&outcome.text).map(|v| row(&v)).unwrap_or_default();
            if outcome.stops || (outcome.is_error && outcome.missed.unwrap_or(0) == 0) {
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
                self.host.names().borrow_mut().insert(name.into(), reference.into());
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
            // (a step that found it so already: left out of the producer's summary when others changed something; an action
            // the DAW keeps no point of, `unkept`, was done and is said)
            if reply.get("unchanged") == Some(&Value::Bool(true)) {
                done.insert("unchanged".into(), Value::Bool(true));
            }
            // What Kumi read for itself in the step's notation, or left to the DAW (a sidechain's channel, an audio
            // clip's looping): the model says it. And what Lua or a call through the DAW's API answered.
            for key in ["notation", "channel", "looping", "result", "printed", "out", "failed"] {
                if let Some(said) = reply.get(key) {
                    done.insert(key.into(), said.clone());
                }
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
        let profile = self.host.history().host.profile();
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
            reply.insert("refusedNote".into(), json!(refused_note(profile)));
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
            // Parameters a device didn't have, or clips whose notation had mistakes (#257).
            let parameters = state.missed.iter().all(|miss| miss.get("deviceRef").is_some_and(|device| !device.is_null()));
            let plan_tool = profile.vocab.plan_tool;
            reply.insert("missedNote".into(),json!(if parameters {format!("These parameters weren't set (the rest of the plan was): name them as the device has them (parametersOnDevice), or give values it can take, in one more {plan_tool}.")} else {format!("Parts of these steps weren't made (missed says which, and why), and the rest of the plan was: send only those parts, fixed, in one more {plan_tool}.")}));
        }
        if let Some(copy) = state.copy.as_ref().filter(|s| !s.is_empty()) {
            reply.insert("copy".into(), json!(copy));
            reply.insert("copyNote".into(),json!(format!("Before this, Kumi kept a copy of the {} as last saved, next to it. Tell the producer in a few words, with the file's name.", profile.words.place)));
        }
        let text = stringify(&json!(reply));
        if stopped.is_some() || !state.refused.is_empty() {
            return Ok(ToolResult::error(text));
        }
        if state.done.is_empty() {
            return Ok(ToolResult::error(STEP_COUNT));
        }
        let reply = if final_ && state.missed.is_empty() {
            let mut text = done_summary(&state.done);
            if let Some(copy) = state.copy.as_ref().filter(|s| !s.is_empty()) {
                text.push_str(&format!(
                    "\n\nFirst, Kumi kept a copy of your {} as last saved, next to it: {}",
                    profile.words.place,
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
    /// A step held back because it needed a refused one holds back what a refused step would: what uses its `@name`,
    /// the later steps on its tracks, the restructures after one, the transport after one (a `wait` after a held-back
    /// launch would wait in silence).
    fn held_back(&mut self, step: usize, item: &JsonObject, on: usize, tracks: Vec<String>, host: &impl PlanHost) {
        self.dependents.push(json!({"step":step,"tool":item.get("tool").cloned().unwrap_or(Value::Null),"after":on}));
        if let Some(name) = item.get("as").and_then(Value::as_str) {
            self.blocked.names.insert(format!("@{name}"), on);
        }
        for track in tracks {
            self.blocked.tracks.entry(track).or_insert(on);
        }
        let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
        if host.structural(tool) {
            self.blocked.structure.get_or_insert(on);
        }
        if host.moves_transport(tool) {
            self.blocked.transport.get_or_insert(on);
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
impl Batch {
    fn matches(&self, value: &Value, key: Option<&Value>) -> bool {
        let item = row(value);
        let input = item.get("input").map(row).unwrap_or_default();
        item.get("tool").and_then(Value::as_str) == Some(self.tool)
            && !item.contains_key("as")
            && input.get(self.key).is_some_and(|v| v.is_string() && key.is_none_or(|key| v == key))
    }
}
struct StreamingPlan<H: PlanHost> {
    host: Rc<H>,
    signal: Signal,
    plan: Rc<Plan<H>>,
    received: Rc<RefCell<Vec<Value>>>,
    take: Rc<dyn Fn(Value)>,
    scanner: RefCell<StepScanner<Box<dyn FnMut(Value)>>>,
}
/// A plan dropped unfinished (its reply set aside) starts nothing more: its task would otherwise wait for steps that
/// never come, or start one it already had. After `finish` the plan has settled, so this changes nothing.
impl<H: PlanHost> Drop for StreamingPlan<H> {
    fn drop(&mut self) {
        self.plan.set_aside();
    }
}
#[async_trait(?Send)]
impl<H: PlanHost> StreamingCall for StreamingPlan<H> {
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
                Some(input) => make_changes(&self.host, input, self.signal.clone()).await,
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

/// The producer's summary of a plan's steps, a line each: a step that found it so already is left out when others changed
/// something (#401), and when every step did, their lines are what's said.
fn done_summary(done: &[Value]) -> String {
    // (each step that says something: whether it found it so already, and its lines)
    let said: Vec<(bool, Vec<String>)> = done
        .iter()
        .map(|item| {
            let lines = item
                .get("lines")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![item.get("changed").cloned().unwrap_or(Value::Null)])
                .into_iter()
                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                .collect::<Vec<_>>();
            (item.get("unchanged") == Some(&Value::Bool(true)), lines)
        })
        .filter(|(_, lines)| !lines.is_empty())
        .collect();
    let all_noops = said.iter().all(|(noop, _)| *noop);
    let lines: Vec<String> = said.into_iter().filter(|(noop, _)| all_noops || !noop).flat_map(|(_, lines)| lines).collect();
    let list = || lines.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n");
    match (all_noops, lines.len()) {
        (true, 1) => format!("{}.", lines[0]),
        (true, _) => format!("Nothing changed:\n{}", list()),
        (false, 1) => format!("Done: {}.", lines[0]),
        (false, _) => format!("Done:\n{}", list()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// #401: a step that found it so already isn't said among what was done; when every step did, that's what's said.
    #[test]
    fn the_summary_leaves_out_steps_that_were_so_already() {
        let noop = json!({"step": 2, "changed": "Bass volume -6 dB: it was so already, so nothing changed", "unchanged": true});
        let tempo = json!({"step": 1, "changed": "Tempo 120 → 124 BPM"});
        let pads = json!({"step": 3, "changed": "Renamed Pad to Pads"});
        assert_eq!(super::done_summary(&[tempo.clone(), noop.clone()]), "Done: Tempo 120 → 124 BPM.");
        assert_eq!(super::done_summary(&[tempo, noop.clone(), pads]), "Done:\n- Tempo 120 → 124 BPM\n- Renamed Pad to Pads");
        assert_eq!(super::done_summary(std::slice::from_ref(&noop)), "Bass volume -6 dB: it was so already, so nothing changed.");
        // An action the DAW keeps no point of (play, a selection) was done: it's said.
        let play = json!({"step": 5, "changed": "Play from bar 5: it changed nothing the project keeps", "unkept": true});
        let select = json!({"step": 6, "changed": "Select Kick: it changed nothing the project keeps", "unkept": true});
        assert_eq!(
            super::done_summary(&[json!({"step": 1, "changed": "Tempo 120 → 124 BPM"}), noop.clone(), play.clone()]),
            "Done:\n- Tempo 120 → 124 BPM\n- Play from bar 5: it changed nothing the project keeps"
        );
        assert_eq!(super::done_summary(&[select, noop.clone(), play]).lines().next(), Some("Done:"));
        let lines = json!({"step": 4, "lines": ["A: it was so already, so nothing changed"], "unchanged": true});
        assert_eq!(
            super::done_summary(&[noop, lines]),
            "Nothing changed:\n- Bass volume -6 dB: it was so already, so nothing changed\n- A: it was so already, so nothing changed"
        );
    }

    /// Review K40: one cap for every DAW's changes, in the words Live's always said.
    #[test]
    fn the_cap_on_an_answers_changes_says_what_it_always_said() {
        assert_eq!(super::too_many_changes(), "That's 5000 changes in one answer; carry on in the next one");
    }
}
