//! Kumi's changes in a DAW whose bridge is its own (REAPER's extension, Renoise's tool): each one previewed and applied
//! through the host's journal (`change_preview`, `change_apply`), kept in HISTORY, and undone as a new step of the
//! DAW's (`change_undo`), never by the DAW's own undo. Plans run through core's plan engine over the DAW's kinds.
use crate::bridge::{
    answer_of, follow_epoch, layout_of, names_tracks, note_epoch, stopped_or, Layout, OwnDaw, Refused, CALL_MS, SHORT_REF,
};
use async_trait::async_trait;
use chrono::Utc;
use indexmap::IndexMap;
use kumi_agent::{
    core::{
        contracts::*,
        history::{cant_reach_to_undo, cant_take_back, Applied, ChangeHost, Entry, History, UndoResult},
        plan::{make_changes, stream_changes, ActionOutcome, ActionSummary, Batch, ChangeOutcome, PlanHost, StepKind},
        refs::{Book, RefScheme},
    },
    integrations::shared::{
        changes::{new_record, ChangeSummary},
        steps::{own_step_line, plan_tool},
        undo::undo_change_with,
    },
    kernel::schema,
    mcp::client::McpEndpoint,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::json::stringify,
};
use kumi_types::contracts::*;
use kumi_types::{daw::DawProfile, errors::RuntimeError};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::Path,
    rc::Rc,
};

/// How often a plan's step is asked to close while the DAW is busy (twice a second, two minutes in all).
const STEP_END_TRIES: u32 = 240;
/// The most changes one release names (the host takes 64 at a time).
const RELEASE_CHUNK: usize = 64;
/// How many of Kumi's latest changes the observation lists, and the most of a title each shows.
const CHANGES_SHOWN: usize = 12;
const TITLE_SHOWN: usize = 160;
/// The most changes the host keeps for Kumi's undo (its journal holds 512): older ones are let go.
pub const UNDO_WINDOW: usize = 400;

/// One of a DAW's change kinds: the step (and tool) that names it, the host's kind, and what HISTORY and the model are
/// told of it.
pub struct OwnKind {
    /// The step's name in make_changes, and the tool's when it has one: "track_volume".
    pub tool: &'static str,
    /// The host's (and the bridge's) name for it: "track-volume".
    pub kind: &'static str,
    pub family: ChangeFamily,
    pub description: &'static str,
    /// Its input, as JSON Schema.
    pub schema: &'static str,
    /// A tool of its own on every request; the others are make_changes steps.
    pub core: bool,
    /// Adds, deletes or moves tracks: later steps on tracks wait for it.
    pub structural: bool,
    /// Deletes what the producer made: the project's saved file is copied first.
    pub deletes: bool,
    /// Input fields the bridge names otherwise (the model's name, the bridge's): every ref field the model sees is named
    /// as refs are (`ref`, `…Ref`, `…Refs`), so Kumi shortens and lengthens it.
    pub wire: &'static [(&'static str, &'static str)],
    /// What HISTORY draws of it, from the values it found and left.
    pub picture: Picture,
    /// The DAW's own undo never takes it back (Renoise's mute, solo and loop; REAPER's repeat): only Kumi's undo does.
    pub skipped_by_daw_undo: bool,
}

/// What HISTORY draws of a change: a value moved within its span (a fader, a pan), a colour's swatches, or one picture
/// per family of kinds (`pictures`): a tracker's lines, a lane's points, the parameters that moved, a route, a
/// keyzone, items on the timeline, the settings that moved, a track's chain of devices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Picture {
    None,
    Meter {
        field: &'static str,
        min: f64,
        max: f64,
    },
    Colors {
        field: &'static str,
    },
    Lines,
    Curve,
    Params,
    Route,
    /// A keyzone, its keys in its DAW's names.
    Zone {
        profile: &'static kumi_types::daw::DawProfile,
    },
    Items,
    Settings,
    Chain,
    Midi,
}

impl OwnKind {
    pub const fn new(
        tool: &'static str,
        kind: &'static str,
        family: ChangeFamily,
        description: &'static str,
        schema: &'static str,
    ) -> Self {
        Self {
            tool,
            kind,
            family,
            description,
            schema,
            core: false,
            structural: false,
            deletes: false,
            wire: &[],
            picture: Picture::None,
            skipped_by_daw_undo: false,
        }
    }
    pub const fn skipped_by_daw_undo(mut self) -> Self {
        self.skipped_by_daw_undo = true;
        self
    }
    pub const fn meter(mut self, field: &'static str, min: f64, max: f64) -> Self {
        self.picture = Picture::Meter { field, min, max };
        self
    }
    pub const fn colors(mut self, field: &'static str) -> Self {
        self.picture = Picture::Colors { field };
        self
    }
    /// HISTORY draws it as one of the families' pictures (`pictures`).
    pub const fn drawn(mut self, picture: Picture) -> Self {
        self.picture = picture;
        self
    }
    /// HISTORY's summary of the change: its title, and its picture from what its preview found (`prior`, `display`) and
    /// its apply left (`applied`, `display`).
    pub fn summary(&self, title: &str, preview: Option<&Value>, applied: Option<&Value>) -> ChangeSummary {
        let mut summary = ChangeSummary::title(title);
        let (Some(preview), Some(applied)) = (preview, applied) else { return summary };
        let answers = crate::pictures::Answers { preview, applied };
        summary.picture = match self.picture {
            Picture::Lines => crate::pictures::lines(&answers),
            Picture::Curve => crate::pictures::curve(&answers),
            Picture::Params => crate::pictures::params(&answers),
            Picture::Route => crate::pictures::route(&answers),
            Picture::Zone { profile } => crate::pictures::zone(&answers, profile),
            Picture::Items => crate::pictures::items(&answers),
            Picture::Settings => crate::pictures::settings(&answers),
            Picture::Midi => crate::pictures::midi(&answers),
            _ => None,
        };
        if self.picture == Picture::Chain {
            summary.devices = crate::pictures::chain(&answers);
        }
        let (prior, after) = (preview.get("prior"), applied.get("applied"));
        let (Some(prior), Some(after)) = (prior.filter(|v| v.is_object()), after.filter(|v| v.is_object())) else { return summary };
        match self.picture {
            Picture::Meter { field, min, max } => {
                // A value the DAW leaves out (Renoise's volume at silence) is the bottom of its span.
                let value = |values: &Value| values.get(field).and_then(Value::as_f64).unwrap_or(min).clamp(min, max);
                if after.get(field).is_some() {
                    (summary.from, summary.to, summary.range) = (Some(value(prior)), Some(value(after)), Some([min, max]));
                }
            }
            Picture::Colors { field } => {
                if let Some(to) = after.get(field).and_then(Value::as_str) {
                    let from = prior.get(field).and_then(Value::as_str).map(str::to_owned);
                    summary.colors = Some(ColorChange { from, to: to.to_owned() });
                }
            }
            _ => {}
        }
        summary
    }
    pub const fn wire(mut self, renamed: &'static [(&'static str, &'static str)]) -> Self {
        self.wire = renamed;
        self
    }
    pub const fn core(mut self) -> Self {
        self.core = true;
        self
    }
    pub const fn structural(mut self) -> Self {
        self.structural = true;
        self
    }
    pub const fn deletes(mut self) -> Self {
        self.deletes = true;
        self
    }
    pub fn input_schema(&self) -> JsonObject {
        serde_json::from_str(self.schema).expect("a change kind's schema is JSON")
    }
}

impl StepKind for OwnKind {
    fn since(&self) -> Option<&str> {
        None
    }
}

/// One of a DAW's actions: what a plan's step or a tool of its own asks the DAW to do that no HISTORY row keeps (play,
/// stop, move the cursor), through one of the host's tools.
pub struct OwnAction {
    pub tool: &'static str,
    /// The host's tool for it.
    pub method: &'static str,
    pub description: &'static str,
    /// Its input, as JSON Schema.
    pub schema: &'static str,
    /// A tool of its own on every request; the others are make_changes steps.
    pub core: bool,
    /// What it leaves the transport doing: playing, stopped, or as it was.
    pub playing: Option<bool>,
    /// What the producer is told it did.
    pub title: &'static str,
}

impl OwnAction {
    pub fn input_schema(&self) -> JsonObject {
        serde_json::from_str(self.schema).expect("an action's schema is JSON")
    }
}

impl StepKind for OwnAction {
    fn since(&self) -> Option<&str> {
        None
    }
}

/// What a DAW of Kumi's own can change, and how its plans and undo are described.
pub struct OwnChangeSet {
    pub kinds: &'static [OwnKind],
    pub actions: &'static [OwnAction],
    /// make_changes' description before the lines of the steps that aren't tools of their own.
    pub plan_description: &'static str,
    pub undo_description: &'static str,
    /// What the model is told of changing the DAW, after the DAW's instructions.
    pub instructions: &'static str,
    /// Whether the DAW holds one undo step open across calls, so one press of its undo takes a whole plan back.
    pub plan_steps: bool,
    /// The most tracks one answer may add, where each costs the DAW a lot in a big project (Renoise).
    pub most_track_adds: Option<usize>,
}

/// make_changes' input: the same shape for every DAW (its steps' tool enum is filled in for the DAW).
const PLAN_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["steps"],"properties":{"steps":{"type":"array","minItems":1,"maxItems":5000,"items":{"type":"object","additionalProperties":false,"required":["tool","input"],"properties":{"tool":{"type":"string"},"input":{"type":"object","description":"What that step takes; \"@name\" for what an earlier step made; wait takes {\"beats\": 8} or {\"seconds\": 4}"},"as":{"type":"string","pattern":"^[a-zA-Z][a-zA-Z0-9_]{0,31}$","description":"Name what this step makes (a new track, an added effect) for later steps"},"each":{"type":"object","description":"Repeat this step: each field's list gives that input field its value run by run, e.g. {\"volumeDb\": [-6, -3]}, or several lists of one length","additionalProperties":{"type":"array","maxItems":5000}}}}},"final":{"type":"boolean","description":"These changes complete the request: Kumi says what changed and you aren't called again. Leave it out to see the results and carry on."}}}"#;
const UNDO_SCHEMA: &str = r#"{"type":"object","properties":{"change":{"type":"string","minLength":1,"maxLength":32,"description":"A change id such as c3, or \"last\""},"discard":{"type":"boolean","description":"Put the change's values back even though the producer changed them since (only when they asked for that)"},"final":{"type":"boolean","description":"true when the undo completes the request: Kumi tells the producer, and the answer ends there"}},"required":["change"],"additionalProperties":false}"#;

/// Kumi's link to the DAW's host, shared by the integration and its changes.
pub struct Link<S: RefScheme> {
    pub daw: &'static OwnDaw,
    pub endpoint: Rc<RefCell<Option<Rc<dyn McpEndpoint>>>>,
    pub book: Rc<RefCell<Book<S>>>,
    pub epoch: Rc<RefCell<Option<String>>>,
    pub layout: Rc<Cell<Option<u64>>>,
    pub seen_in: Rc<RefCell<HashMap<String, String>>>,
    pub connected: Rc<Cell<bool>>,
    /// Cancelled when the integration closes.
    pub lifetime: Signal,
    /// The tempo as last read, for a wait in beats.
    pub tempo: Rc<Cell<Option<f64>>>,
    /// The project's saved file as last read, for the safety copy.
    pub file: Rc<RefCell<Option<String>>>,
    /// While a plan's undo step is open: the titles of the changes made in it, which name the step when it closes.
    pub step_titles: RefCell<Option<Vec<String>>>,
    /// Changes the DAW's own undo doesn't take back (`undo.entry` or `undo.covered` false, or a kind it always skips),
    /// by transaction.
    pub outside_daw_undo: RefCell<std::collections::HashSet<String>>,
    /// Changes whose undo by Kumi was sent and never answered (or the DAW couldn't say how it ended), by transaction.
    pub undo_unanswered: RefCell<std::collections::HashSet<String>>,
    /// Changes made in a plan's step of the DAW's undo, by transaction.
    pub in_plan: RefCell<std::collections::HashSet<String>>,
    /// Changes Kumi undid that the DAW's undo or redo may have put back (it can't say which step it took), by
    /// transaction: Unsure until the DAW's undo or redo, or Kumi's undo, settles them.
    pub maybe_redone: RefCell<std::collections::HashSet<String>>,
    /// Changes the DAW's own undo or redo put back (a redo of the change, an undo of Kumi's undo step), by transaction: what
    /// Kumi made may be a new object to the DAW's bridge since, so a refusal of Kumi's undo says that, not that it moved or
    /// went (`undo_transaction`).
    pub put_back_by_daw: RefCell<std::collections::HashSet<String>>,
    /// The open project's identity as Kumi last saw it (the integration's), and the one each change was made in, by
    /// transaction: a refusal of Kumi's undo in another project says that, not that something moved.
    pub project: Rc<RefCell<Option<String>>>,
    pub made_in: RefCell<HashMap<String, String>>,
}

/// How Kumi's undo of a change came to an end for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Past the changes Kumi keeps for its undo.
    Window,
    /// Kumi's host started again, with a new journal.
    Restart,
    /// The host doesn't know the change any more.
    Unknown,
    /// Past the changes only Kumi can take back that Kumi keeps for its undo (half the window).
    Crowded,
}

impl<S: RefScheme> Link<S> {
    /// A host tool's answer, Ok with its JSON or Err with why; Err(RuntimeError) when the host isn't there.
    pub async fn call(&self, tool: &str, args: Value, signal: Signal) -> Result<Result<Value, Refused>, RuntimeError> {
        let Some(endpoint) = self.endpoint.borrow().clone() else {
            return Err(RuntimeError::plain(self.daw.profile.vocab.cant_reach));
        };
        let args = args.as_object().cloned().unwrap_or_default();
        let bounded = abort::any([signal, self.lifetime.clone(), abort::timeout(CALL_MS)]);
        stopped_or(endpoint.call(tool, args, bounded.clone()).await.map(|result| answer_of(&result)), &bounded)
    }

    fn words(&self) -> &'static DawProfile {
        self.daw.profile
    }

    /// What a row says once Kumi can't undo it anymore: how that came, and who can still take it back, from what the row
    /// knows. Kumi's undo of it was never answered: it's checked, then changed by hand. An Unsure row stays unsure: the
    /// DAW may have redone it, or Kumi never heard whether it went through. Only Kumi could take it back: nobody can
    /// now, so by hand. Else the DAW's own undo may (with the plan's other changes, for a plan's step).
    pub fn ending_note(&self, transaction: &str, state: ChangeState, ending: Ending) -> String {
        let (app, own_undo) = (self.words().words.app, self.words().vocab.own_undo);
        let why = match ending {
            Ending::Window => format!("Past the {UNDO_WINDOW} changes Kumi keeps for its undo"),
            Ending::Restart => format!("Kumi's link to {app} started again, so Kumi can't undo this anymore"),
            Ending::Unknown => "Kumi can't undo this anymore".to_string(),
            Ending::Crowded => format!("Past the {} changes only Kumi can take back that Kumi keeps for its undo", UNDO_WINDOW / 2),
        };
        let (only_kumi, in_plan) = (self.outside_daw_undo.borrow().contains(transaction), self.in_plan.borrow().contains(transaction));
        if self.undo_unanswered.borrow().contains(transaction) {
            format!("{why}, and whether its undo went through isn't known: check it in {app}, and change it back by hand if it's still as Kumi set it.")
        } else if state == ChangeState::Unsure && self.maybe_redone.borrow().contains(transaction) {
            format!("{why}, and {app} may have redone it (Kumi can't tell): check it in {app}, and change it back by hand if it's there.")
        } else if state == ChangeState::Unsure && only_kumi {
            format!("{why}, and Kumi isn't sure it went through: check it in {app}, and change it back by hand if it did ({own_undo} doesn't take this back).")
        } else if state == ChangeState::Unsure && in_plan {
            format!("{why}, and Kumi isn't sure it went through: check it in {app}; if it did, {own_undo} takes it back only with the plan's other changes.")
        } else if state == ChangeState::Unsure {
            format!("{why}, and Kumi isn't sure it went through: check it in {app}; if it did, {own_undo} may still take it back.")
        } else if only_kumi {
            format!("{why}, and {own_undo} doesn't take this back: neither can now, so change it by hand if needed.")
        } else if in_plan {
            // (true before the plan's undo and after it: the step goes with the plan, never alone)
            format!("{why}; {own_undo} takes it back only with the plan's other changes.")
        } else {
            format!("{why}; {own_undo} may still take it back.")
        }
    }

    /// What an Undone row says once Kumi can't follow the DAW's undo any more: the producer's undo of Kumi's undo puts
    /// the change back, unseen.
    fn undone_note(&self, ending: Ending) -> String {
        let (app, own_undo) = (self.words().words.app, self.words().vocab.own_undo);
        let why = match ending {
            Ending::Restart => format!("Kumi's link to {app} started again"),
            _ => "Kumi can't follow this anymore".to_string(),
        };
        format!("{why}: if {own_undo} took back Kumi's undo, this is in effect again, and Kumi can't see it.")
    }

    /// The DAW's words for a refusal, with its long refs in this turn's short ones.
    fn in_short_refs(&self, why: &str) -> String {
        let mut why = why.to_owned();
        let mut book = self.book.borrow_mut();
        for reference in book.scheme.refs_in(&why.clone()) {
            let short = book.short_ref(reference);
            why = why.replace(reference, &short);
        }
        why
    }
}

/// What HISTORY asks of a DAW of Kumi's own: its refs, its clock, and its way back for one change (the host's undo).
pub struct OwnChanges<S: RefScheme> {
    pub link: Rc<Link<S>>,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> ChangeHost for OwnChanges<S> {
    type Refs = S;
    /// Nothing: the host keeps what a delete removed, with the change's record in its journal.
    type Material = Value;
    fn profile(&self) -> &DawProfile {
        self.link.daw.profile
    }
    fn book(&self) -> &RefCell<Book<S>> {
        &self.link.book
    }
    fn lifetime(&self) -> Signal {
        self.link.lifetime.clone()
    }
    fn now_ms(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
    fn keep_soon(&self) {}
    fn release(&self, transactions: Vec<String>) {
        let link = self.link.clone();
        tokio::task::spawn_local(async move {
            for chunk in transactions.chunks(RELEASE_CHUNK) {
                let signal = abort::timeout(10_000);
                if !matches!(link.call("change_release", json!({"transactionIds": chunk}), signal).await, Ok(Ok(_))) {
                    break;
                }
            }
        });
    }
    async fn ready(&self, _signal: Signal) -> bool {
        self.link.endpoint.borrow().is_some()
    }
    fn reachable(&self) -> bool {
        self.link.connected.get() && self.link.endpoint.borrow().is_some()
    }
    fn plan_steps(&self) -> bool {
        self.link.daw.changes.plan_steps
    }
    async fn step_begin(&self) -> Option<String> {
        let opened = self.link.call("undo_step_begin", json!({"label": "Kumi"}), abort::timeout(10_000)).await.ok()?.ok()?;
        let id = opened.get("stepId").and_then(Value::as_str).map(str::to_owned)?;
        *self.link.step_titles.borrow_mut() = Some(Vec::new());
        Some(id)
    }
    async fn step_end(&self, id: String) {
        // The DAW's undo menu names the plan by what it did: its first change, and how many more.
        let titles = self.link.step_titles.borrow_mut().take().unwrap_or_default();
        let mut args = json!({"stepId": id});
        if let Some(first) = titles.first() {
            args["label"] = match titles.len() {
                1 => first.clone(),
                n => format!("{first} and {} more", n - 1),
            }
            .into();
        }
        // Too busy to close it now (a render, a dialog: why the plan stopped): asked again until it closes, so the
        // producer's edits meanwhile don't land in Kumi's step.
        let busy = |answer: &Result<Result<Value, Refused>, RuntimeError>| match answer {
            Ok(Err(refused)) => refused.busy,
            Err(error) => error.is_aborted(),
            Ok(Ok(_)) => false,
        };
        if !busy(&self.link.call("undo_step_end", args.clone(), abort::timeout(10_000)).await) {
            return;
        }
        let link = self.link.clone();
        tokio::task::spawn_local(async move {
            for _ in 0..STEP_END_TRIES {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if link.lifetime.is_cancelled() || !busy(&link.call("undo_step_end", args.clone(), abort::timeout(10_000)).await) {
                    return;
                }
            }
        });
    }
    async fn stop_transport(&self, signal: Signal) -> bool {
        matches!(self.link.call("transport_stop", json!({}), signal).await, Ok(Ok(answer)) if answer.get("stopped") != Some(&Value::Bool(false)))
    }
    async fn revert(
        &self,
        history: &History<Self>,
        entry: &Entry<Value>,
        _revert: Vec<Value>,
        _signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        // Own DAWs keep no revert scripts: every change is undone through the host.
        Ok(UndoResult::with(entry.borrow().record.clone(), cant_take_back(history.host.profile()), true))
    }
    async fn undo_material(
        &self,
        history: &History<Self>,
        entry: &Entry<Value>,
        snapshot: &Applied<Value>,
        _material: Value,
        undo_key: &str,
        discard: bool,
        _signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        match self.undo_transaction(history, entry, snapshot, undo_key, discard).await? {
            Some(stopped) => Ok(stopped),
            None => Ok(history.undone(entry)),
        }
    }
    async fn undo_transaction(
        &self,
        history: &History<Self>,
        entry: &Entry<Value>,
        snapshot: &Applied<Value>,
        undo_key: &str,
        discard: bool,
    ) -> Result<Option<UndoResult>, RuntimeError> {
        let words = self.link.words().words;
        let mut args = json!({"transactionId": snapshot.transaction_id, "confirmation": "undo", "idempotencyKey": undo_key});
        if discard {
            args["discard"] = true.into();
        }
        let answer = match self.link.call("change_undo", args, history.change_signal()).await {
            Ok(answer) => answer,
            // No answer in time: whether the undo went through isn't known (the host won't send it twice either).
            Err(error) if error.is_aborted() && !self.link.lifetime.is_cancelled() => {
                self.link.undo_unanswered.borrow_mut().insert(snapshot.transaction_id.clone());
                let note = unsure_undo_note(self.link.words(), &snapshot.record.title);
                let text = format!("{} didn't answer Kumi's undo, so whether it went through isn't known: {note}", words.app);
                return Ok(Some(UndoResult::with(history.update(entry, ChangeState::Unsure, Some(note)), text, true)));
            }
            Err(_) => return Ok(Some(UndoResult::error(cant_reach_to_undo(self.link.words())))),
        };
        // (an answer settles whether the DAW redid it: the row now says what Kumi's undo found)
        let settled = |link: &Link<S>| {
            link.maybe_redone.borrow_mut().remove(&snapshot.transaction_id);
            link.put_back_by_daw.borrow_mut().remove(&snapshot.transaction_id)
        };
        let refused = match answer {
            Ok(done) if done.get("state").and_then(Value::as_str) == Some("undone") => {
                settled(&self.link);
                // A restore that made devices again moves Renoise's epoch: the book follows.
                follow_epoch(&self.link.book, &self.link.epoch, &self.link.layout, &done);
                note_epoch(&self.link.seen_in, &self.link.book.borrow().scheme, &done);
                // What the DAW couldn't put back (a mute in a slot, lines in a pattern, a routing), said on the row and
                // in the answer: an undo made in part never reads as whole.
                let Some(left_out) = done.pointer("/display/note").and_then(Value::as_str).filter(|note| !note.is_empty()) else {
                    return Ok(None);
                };
                let note = self.link.in_short_refs(left_out);
                let record = history.update(entry, ChangeState::Undone, Some(note.clone()));
                let text = stringify(&json!({"undone": record.title, "change": record.id, "note": note}));
                return Ok(Some(UndoResult::with(record, text, false)));
            }
            Ok(other) => {
                let note = format!("{} didn't confirm the undo; try again.", words.app);
                return Ok(Some(UndoResult::with(history.update(entry, ChangeState::Unsure, Some(note)), stringify(&other), true)));
            }
            Err(refused) => refused,
        };
        let why = self.link.in_short_refs(&refused.why);
        let put_back = matches!(refused.code.as_deref(), Some("changed" | "gone" | "stale" | "undone" | "daw-only" | "unknown"))
            && settled(&self.link);
        // (made in a project that isn't the one open now: Renoise's new song, REAPER's other tab)
        let elsewhere = self
            .link
            .made_in
            .borrow()
            .get(&snapshot.transaction_id)
            .is_some_and(|made| self.link.project.borrow().as_ref().is_some_and(|now| now != made));
        let (state, note) = match refused.code.as_deref() {
            Some("changed" | "gone" | "stale") if elsewhere => (
                ChangeState::Kept,
                format!("Another {} is open in {} now, not the one Kumi made this in, so Kumi left it.", words.place, words.app),
            ),
            // The DAW's own undo or redo put it back since: to its bridge that may be another object, or lines another hand
            // wrote, so it refuses; it's there, and the DAW's undo is what takes it back now.
            Some("changed" | "gone" | "stale") if put_back => (
                ChangeState::Kept,
                format!(
                    "{} put it back with its own undo or redo, and Kumi can't tell it's still what Kumi made, so it left it as it is; {} can take it back.",
                    words.app,
                    self.link.words().vocab.own_undo
                ),
            ),
            Some("changed") => (ChangeState::Kept, format!("It changed in {} since, so Kumi left it as it is.", words.app)),
            Some("gone") => (ChangeState::Kept, format!("What it changed is gone from the {}, so there's nothing to undo.", words.place)),
            Some("stale") if self.link.book.borrow().scheme.positional() => (
                ChangeState::Kept,
                format!("What it changed moved in the {} since (something was added, removed or moved), so Kumi left it.", words.place),
            ),
            Some("stale") => (
                ChangeState::Kept,
                format!("The {} changed since (another one is open, or it was reloaded), so Kumi left it.", words.place),
            ),
            Some("undone") => return Ok(Some(history.undone(entry))),
            // (what Lua or a call through the DAW's API did: Kumi doesn't know what it changed, the DAW's own undo does)
            Some("daw-only") => (ChangeState::Kept, why.clone()),
            Some("unknown") => {
                (ChangeState::Expired, self.link.ending_note(&snapshot.transaction_id, snapshot.record.state, Ending::Unknown))
            }
            Some("not-applied" | "uncertain")
                if snapshot.record.state == ChangeState::Unsure || refused.code.as_deref() == Some("uncertain") =>
            {
                let only_kumi = self.link.outside_daw_undo.borrow().contains(&snapshot.transaction_id);
                let in_plan = self.link.in_plan.borrow().contains(&snapshot.transaction_id);
                (ChangeState::Unsure, unsure_note(self.link.words(), &snapshot.record.title, only_kumi, in_plan))
            }
            // Kumi's undo was sent and never answered, or the DAW couldn't say how it ended: the change was made; whether
            // it's undone isn't known. The DAW's own words say what to look for.
            Some("undo-uncertain") => {
                let again = !self.link.undo_unanswered.borrow_mut().insert(snapshot.transaction_id.clone());
                self.link.maybe_redone.borrow_mut().remove(&snapshot.transaction_id);
                let note = unsure_undo_note(self.link.words(), &snapshot.record.title);
                let (note, text) = if again {
                    // (refused before anything was sent: the row keeps what it says of the undo that went before)
                    let kept = snapshot.record.note.clone().unwrap_or_else(|| note.clone());
                    (kept, format!("Whether Kumi's undo of this went through isn't known, so Kumi won't undo it again: {note}"))
                } else if refused.answered {
                    // The DAW answered, and said how far its undo got (a sliced restore stopped part way, a job past its
                    // time): its words lead, on the row too.
                    (
                        format!("Kumi's undo didn't finish: {why}"),
                        format!("{} couldn't finish Kumi's undo: {why}. Kumi won't undo it again.", words.app),
                    )
                } else {
                    let text = format!("{} didn't answer Kumi's undo, so whether it went through isn't known: {note}", words.app);
                    (note, text)
                };
                return Ok(Some(UndoResult::with(history.update(entry, ChangeState::Unsure, Some(note)), text, true)));
            }
            // Busy (a dialog, a render) or another refusal with nothing written: the change stays as it was, undoable.
            Some("busy") => {
                return Ok(Some(UndoResult::with(
                    snapshot.record.clone(),
                    format!("{} is busy ({why}): nothing was undone; try again in a moment.", words.app),
                    true,
                )))
            }
            _ => return Ok(Some(UndoResult::with(snapshot.record.clone(), why, true))),
        };
        Ok(Some(UndoResult::with(history.update(entry, state, Some(note)), why, true)))
    }
}

/// The DAW's change engine: what a plan, a change tool and an undo ask of it.
pub struct OwnPlan<S: RefScheme + 'static> {
    pub history: History<OwnChanges<S>>,
    names: RefCell<IndexMap<String, String>>,
    on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    /// Tracks added this answer (`OwnChangeSet::most_track_adds`).
    pub added: Cell<usize>,
    /// Why the saved project wasn't copied before the step about to run (too big, or the copy failed).
    pub not_copied: RefCell<Option<Copied>>,
    /// Kumi's track renames, oldest first: the change, the track's ref and the name it had and was given (a judged run
    /// follows its track by them).
    pub renamed: RefCell<Vec<(String, String, String, String)>>,
    /// (#401) The chunks run_lua ran this session without stopping at an error (their hashes, the latest kept): a script is
    /// kept only once it ran so.
    ran: RefCell<Vec<u64>>,
}

/// The most clean runs remembered.
const MOST_RAN: usize = 64;

/// A chunk's hash, its lines' trailing spaces and its ends left out (the same chunk sent again a little reformatted).
fn chunk_hash(code: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in code.trim().lines() {
        line.trim_end().hash(&mut hasher);
    }
    hasher.finish()
}

impl<S: RefScheme + 'static> OwnPlan<S> {
    pub fn new(
        link: Rc<Link<S>>,
        timeout_ms: Option<u64>,
        on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
        on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    ) -> Rc<Self> {
        Rc::new(Self {
            history: History::over(OwnChanges { link }, timeout_ms, on_change),
            names: RefCell::default(),
            on_action,
            added: Cell::new(0),
            not_copied: RefCell::new(None),
            renamed: RefCell::default(),
            ran: RefCell::default(),
        })
    }

    pub fn link(&self) -> &Link<S> {
        &self.history.host.link
    }

    /// One change of the DAW's by its step's name (the listening tools' own: a gain stage turned, knobs set), made and
    /// kept as the model's are; refused in words when the DAW has no such kind.
    pub async fn change_by(&self, tool: &str, input: JsonObject, signal: Signal) -> ChangeOutcome {
        match self.link().daw.changes.kinds.iter().find(|kind| kind.tool == tool) {
            Some(kind) => self.one_change(kind, input, signal).await,
            None => ChangeOutcome::error(format!("{} has no {tool} change", self.link().words().words.app)),
        }
    }

    /// The epoch (and, where refs are places, the layout) a request about these refs is asked from, as a change's is.
    pub fn place_request(&self, args: &mut Value, input: &JsonObject) {
        self.place(args, input)
    }

    /// (#401) Whether run_lua ran this chunk this session without stopping at an error.
    pub fn ran_cleanly(&self, code: &str) -> bool {
        self.ran.borrow().contains(&chunk_hash(code))
    }

    /// A ref the model gave, long (as the bridge takes it), or why it isn't one of this turn's.
    pub fn lengthen(&self, reference: &str) -> Result<String, String> {
        let long = self.link().book.borrow().lengthen(&json!({"ref": reference}));
        let long = long["ref"].as_str().unwrap_or(reference).to_string();
        if SHORT_REF.is_match(&long) {
            return Err(format!("{reference} isn't one of this turn's refs (the song may have changed): read again for a current one"));
        }
        Ok(long)
    }

    /// (#401) Whether the DAW plays now, as its focus says; None when it couldn't say (a render then waits).
    pub async fn playing(&self, signal: &Signal) -> Option<bool> {
        let focus = self.link().call("focus", json!({}), signal.clone()).await.ok()?.ok()?;
        focus.pointer("/transport/playing").and_then(Value::as_bool)
    }

    /// A read as the read tool makes it, from the book's epoch when it names refs (and `layout`, when they are from then):
    /// its answer, the book following the epoch it gives. Core tools' backends read through it (the variations, the sample map).
    pub async fn read_through(&self, mut args: Value, layout: Option<u64>, signal: &Signal) -> Result<Value, String> {
        let link = self.link();
        let input = args.as_object().cloned().unwrap_or_default();
        if ["parent", "track", "items", "ref"].iter().any(|key| input.contains_key(*key)) {
            self.place(&mut args, &input);
        }
        if let Some(layout) = layout {
            args["layout"] = layout.into();
        }
        match link.call("read", args, signal.clone()).await {
            Ok(Ok(answer)) => {
                follow_epoch(&link.book, &link.epoch, &link.layout, &answer);
                Ok(answer)
            }
            Ok(Err(refused)) => Err(refused.why),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Every row of a read, page after page (at most `pages` of them).
    pub async fn rows_through(&self, args: Value, pages: usize, signal: &Signal) -> Result<Vec<Value>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..pages {
            let mut asked = args.clone();
            if let Some(cursor) = &cursor {
                asked["cursor"] = cursor.clone().into();
            }
            let page = self.read_through(asked, None, signal).await?;
            out.extend(page.get("items").and_then(Value::as_array).cloned().unwrap_or_default());
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(next) => cursor = Some(next.into()),
                None => return Ok(out),
            }
        }
        Ok(out)
    }

    /// The tools for this DAW's changes: each core kind's and action's, make_changes with every kind and action as a step,
    /// and undo_change.
    pub fn tools(self: &Rc<Self>) -> Vec<Rc<dyn KernelTool>> {
        let set = self.link().daw.changes;
        let mut tools: Vec<Rc<dyn KernelTool>> = Vec::new();
        let mut lines = Vec::new();
        // Every kind and action is a tool: the core ones go with every request, the others wait behind find_tools (which
        // gives their whole input) and get a line in make_changes' description.
        for kind in set.kinds {
            tools.push(Rc::new(ChangeTool { engine: self.clone(), kind }));
            if !kind.core {
                lines.push(own_step_line(kind.tool, kind.description, &kind.input_schema()));
            }
        }
        for action in set.actions {
            tools.push(Rc::new(ActionTool { engine: self.clone(), action }));
            if !action.core {
                lines.push(own_step_line(action.tool, action.description, &action.input_schema()));
            }
        }
        let steps = set.kinds.iter().map(|kind| kind.tool).chain(set.actions.iter().map(|action| action.tool)).map(str::to_owned).collect();
        let (description, schema) = plan_tool(set.plan_description, &serde_json::from_str(PLAN_SCHEMA).expect("JSON"), steps, &lines);
        tools.push(Rc::new(PlanTool { engine: self.clone(), description, schema }));
        tools.push(Rc::new(UndoTool { engine: self.clone() }));
        tools
    }

    /// A copy of the saved project before a step that can lose material: its path. One too big to copy, or a copy that
    /// failed, is said with the step's answer (`not_copied`).
    pub async fn copy_before_delete(&self) -> Option<String> {
        // (what an earlier delete couldn't copy isn't this one's to say)
        *self.not_copied.borrow_mut() = None;
        let path = self.link().file.borrow().clone()?;
        // A big song is a big copy: off Kumi's thread.
        match tokio::task::spawn_blocking(move || copy_beside(Path::new(&path))).await.ok()? {
            Copied::Made(copy) => Some(copy),
            Copied::Nothing => None,
            not_copied => {
                *self.not_copied.borrow_mut() = Some(not_copied);
                None
            }
        }
    }

    /// A new answer: its counts of changes and added tracks start again.
    pub fn new_turn(&self) {
        self.history.changes_this_turn.set(0);
        self.added.set(0);
    }

    /// The observation's list of Kumi's latest changes and where each stands.
    pub fn latest(&self) -> Option<Value> {
        let entries = self.history.entries.borrow();
        let rows: Vec<Value> = entries
            .values()
            .rev()
            .take(CHANGES_SHOWN)
            .map(|entry| {
                let record = &entry.borrow().record;
                let title = kumi_common::js::string::head(&record.title, TITLE_SHOWN);
                let mut row = json!({"change": record.id, "title": title, "state": record.state});
                if let Some(note) = &record.note {
                    row["note"] = json!(note);
                }
                row
            })
            .collect();
        (!rows.is_empty()).then(|| Value::Array(rows.into_iter().rev().collect()))
    }

    /// The producer took Kumi's changes back (or put them back) with the DAW's own undo: HISTORY follows.
    pub fn undone_in_daw(&self, data: &Value) {
        let ids = |key: &str| -> Vec<String> {
            data.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
        };
        // Which changes the DAW's undo took back and which it put back, as the host's journal followed it (its own step,
        // or Kumi's undo of it, undone or redone); an older host names only the changes' own steps.
        let (undone, redone) = if data.get("undone").is_some() || data.get("redone").is_some() {
            (ids("undone"), ids("redone"))
        } else if data.get("how").and_then(Value::as_str) == Some("redo") {
            (vec![], ids("transactionIds"))
        } else {
            (ids("transactionIds"), vec![])
        };
        // (what the DAW may have put back: it can't say whether its redo did, or its undo of something older)
        let unsure = ids("unsure");
        if undone.is_empty() && redone.is_empty() && unsure.is_empty() {
            return;
        }
        let profile = self.link().words();
        let outside = self.link().outside_daw_undo.borrow();
        let entries: Vec<_> = self.history.entries.borrow().values().cloned().collect();
        for entry in entries {
            let (transaction, state) = {
                let entry = entry.borrow();
                (entry.transaction_id.clone(), entry.record.state)
            };
            if transaction.is_empty() {
                continue;
            }
            // A step the DAW's undo doesn't take back (REAPER's repeat, Renoise's mute) is in a plan's step, not undone by
            // it. (What the DAW puts back is exact: its undo of Kumi's own undo step.)
            let only_kumi = outside.contains(&transaction);
            // (the DAW's undo or redo settles the row: what Kumi didn't know of it before is known now)
            let settle = |link: &Link<S>| {
                link.maybe_redone.borrow_mut().remove(&transaction);
                link.undo_unanswered.borrow_mut().remove(&transaction);
                link.put_back_by_daw.borrow_mut().remove(&transaction);
            };
            match state {
                // (Kept: Kumi's undo was refused, as for Lua the DAW's own undo takes back, or the producer changed it since;
                // the DAW's undo of the change's own step still takes it back)
                ChangeState::Applied | ChangeState::Unsure | ChangeState::Kept if undone.contains(&transaction) && !only_kumi => {
                    settle(self.link());
                    self.history.update(&entry, ChangeState::Undone, Some(format!("Undone with {}.", profile.vocab.own_undo)));
                }
                // (Unsure: Kumi's undo of it was never answered, and the DAW's undo took that undo's step back)
                ChangeState::Undone | ChangeState::Unsure if redone.contains(&transaction) => {
                    settle(self.link());
                    self.link().put_back_by_daw.borrow_mut().insert(transaction.clone());
                    // Kumi may undo it again: with a fresh undo, not the one that answered before.
                    entry.borrow_mut().undo_key = None;
                    self.history.update(&entry, ChangeState::Applied, Some(format!("Put back with {}.", profile.words.app)));
                }
                ChangeState::Undone if unsure.contains(&transaction) => {
                    settle(self.link());
                    self.link().maybe_redone.borrow_mut().insert(transaction.clone());
                    self.link().put_back_by_daw.borrow_mut().insert(transaction.clone());
                    // (Kumi's undo of it reads back first, and writes only if what Kumi wrote is where it wrote it, untouched)
                    entry.borrow_mut().undo_key = None;
                    let note = format!("{} may have redone it; Kumi can't tell.", profile.words.app);
                    self.history.update(&entry, ChangeState::Unsure, Some(note));
                }
                _ => {}
            }
        }
        self.history.outside.borrow_mut().push(profile.vocab.own_undo);
    }

    /// The input with this turn's short refs made long, or what to say of a short ref the turn doesn't know.
    fn lengthened(&self, input: &JsonObject) -> Result<JsonObject, String> {
        let book = self.link().book.borrow();
        let mut long = book.lengthen(&Value::Object(input.clone()));
        // (a call's arguments hold refs under no ref-named field)
        if let Some(args) = long.get("args").cloned() {
            long["args"] = crate::bridge::lengthen_anywhere(&book, &args);
        }
        let mut unknown = None;
        find_unknown(&long, "", &mut unknown, 0);
        if let Some(asked) = unknown {
            let place = self.link().words().words.place;
            return Err(format!(
                "{asked} isn't one of this turn's refs (it may be from before the {place} changed): read again for a current one"
            ));
        }
        Ok(long.as_object().cloned().unwrap_or_default())
    }

    /// Where a change is asked from: the epoch its refs were read in, and (refs that are positions) their layout.
    fn place(&self, args: &mut Value, input: &JsonObject) {
        let link = self.link();
        let book = link.book.borrow();
        let positional = book.scheme.positional();
        let primary = ["ref", "parent", "trackRef", "track"].iter().find_map(|key| input.get(*key).and_then(Value::as_str));
        let epoch = match primary {
            Some(reference) if !positional => link.seen_in.borrow().get(reference).cloned().or_else(|| link.epoch.borrow().clone()),
            _ => link.epoch.borrow().clone(),
        };
        if let Some(epoch) = epoch {
            args["epoch"] = epoch.into();
        }
        if let Some(layout) = link.layout.get().filter(|_| names_tracks(&book.scheme, input)) {
            args["layout"] = layout.into();
        }
    }

    /// A refusal before anything was written, in Kumi's words (`asked`: the input as the model gave it, in short refs).
    fn refusal(&self, refused: &Refused, asked: &JsonObject) -> String {
        let profile = self.link().words();
        let words = profile.words;
        let why = self.link().in_short_refs(&refused.why);
        let what = asked.get("ref").and_then(Value::as_str).unwrap_or("That ref");
        match refused.code.as_deref() {
            Some("stale") if self.link().book.borrow().scheme.positional() => {
                format!("{what} is from before the {} changed (something was added, removed or opened): read again for current refs; nothing changed", words.place)
            }
            Some("stale") => format!(
                "{what} was read in another {} than the one {} has open now: read again for current refs; nothing changed",
                words.place, words.app
            ),
            Some("changed") => format!(
                "It changed in {} between Kumi's look and the change ({why}), so nothing was written: read it again and retry",
                words.app
            ),
            Some("gone") => format!("{what} is gone from the {}: nothing changed", words.place),
            Some("unknown-method") => format!(
                "{}'s {} is older than this Kumi and can't make changes yet: run kumi --daw {} bridge, then restart {}",
                words.app,
                if self.link().daw.host_flag == "renoise" { "tool" } else { "extension" },
                self.link().daw.host_flag,
                words.app
            ),
            _ => why,
        }
    }

    /// One change: previewed, applied in one named step of the DAW's undo, and kept in HISTORY.
    pub async fn one_change(&self, kind: &'static OwnKind, asked: JsonObject, signal: Signal) -> ChangeOutcome {
        let words = self.link().words().words;
        if self.history.changes_this_turn.get() >= kumi_agent::core::plan::MAX_CHANGES {
            return ChangeOutcome::stop(kumi_agent::core::plan::too_many_changes());
        }
        // Each track a DAW like Renoise adds costs it more in a big song (and it stalls near 600): an answer adds a few.
        let most = self.link().daw.changes.most_track_adds.filter(|_| kind.kind == "track-add");
        if let Some(most) = most.filter(|most| self.added.get() >= *most) {
            return ChangeOutcome::stop(format!(
                "That's {most} tracks added in one answer, the most {} takes at once; carry on in the next one",
                self.link().words().words.app
            ));
        }
        let mut input = match self.lengthened(&asked) {
            Ok(input) => input,
            Err(why) => return ChangeOutcome::error(why),
        };
        for (ours, theirs) in kind.wire {
            if let Some(value) = input.remove(*ours) {
                input.insert((*theirs).into(), value);
            }
        }
        let mut args = json!({"kind": kind.kind, "input": input});
        self.place(&mut args, &input);
        let preview = match self.link().call("change_preview", args, signal.clone()).await {
            Ok(Ok(preview)) => preview,
            // Busy (a dialog): the plan stops here, so nothing after it is made out of order once the dialog closes.
            Ok(Err(refused)) if refused.busy => {
                return ChangeOutcome::stop(format!(
                    "{} is busy ({}), so this wasn't made and nothing of it was written; in a plan, the steps after it weren't run and the ones before it stand, each undoable. Try again once it's free",
                    words.app, refused.why
                ))
            }
            Ok(Err(refused)) => return ChangeOutcome::error(self.refusal(&refused, &asked)),
            // The producer stopped it: the plan is stopping anyway.
            Err(error) if error.is_aborted() && signal.is_cancelled() => {
                return ChangeOutcome::error(format!("Stopped before {} was asked: nothing changed", words.app))
            }
            // No answer in time (a dialog may be open): a preview writes nothing, and the plan stops here.
            Err(error) if error.is_aborted() => {
                return ChangeOutcome::stop(format!(
                    "{} didn't answer in time (a dialog may be open), so this wasn't made and nothing of it was written; in a plan, the steps after it weren't run and the ones before it stand, each undoable",
                    words.app
                ))
            }
            Err(_) => return ChangeOutcome::error(self.link().words().vocab.cant_reach),
        };
        follow_epoch(&self.link().book, &self.link().epoch, &self.link().layout, &preview);
        let title = preview.pointer("/display/title").and_then(Value::as_str).unwrap_or(kind.kind).to_owned();
        // Nothing to change (already so): no step in the DAW's undo, no row in HISTORY.
        if preview.pointer("/display/unchanged") == Some(&Value::Bool(true)) {
            let id = preview.get("transactionId").and_then(Value::as_str).map(str::to_owned);
            self.history.release(&id.into_iter().collect::<Vec<_>>());
            // (A plan's answer keeps only a step's `changed`: the reason goes there.)
            return ChangeOutcome {
                text: stringify(&json!({"changed": format!("{title}: it was so already, so nothing changed"), "unchanged": true})),
                is_error: false,
                missed: None,
                stops: false,
                applied: None,
            };
        }
        let (Some(transaction), Some(confirmation)) =
            (preview.get("transactionId").and_then(Value::as_str), preview.get("confirmation").and_then(Value::as_str))
        else {
            return ChangeOutcome::error(format!("{} answered the preview without a transaction: nothing changed", words.app));
        };
        let transaction = transaction.to_owned();
        let apply = json!({"transactionId": transaction, "confirmation": confirmation, "idempotencyKey": uuid::Uuid::new_v4().to_string()});
        // Once asked, the apply runs to its answer: an Esc mid-way would leave the change's outcome unknown.
        let applied = match self.link().call("change_apply", apply, self.history.change_signal()).await {
            Ok(Ok(applied)) => applied,
            Ok(Err(refused)) if refused.code.as_deref() == Some("uncertain") || refused.why.contains("uncertain") => {
                self.remember(
                    kind,
                    kind.summary(&title, None, None),
                    &transaction,
                    ChangeState::Unsure,
                    Some(self.unsure_note(kind, &title, &transaction)),
                );
                return ChangeOutcome::stop(format!(
                    "{} didn't confirm this change ({title}), so it may or may not have happened: read it before going on",
                    words.app
                ));
            }
            Ok(Err(refused)) if refused.busy => {
                self.history.release(std::slice::from_ref(&transaction));
                return ChangeOutcome::stop(format!(
                    "{} went busy ({}) before this was written, so it wasn't made and nothing of it was written; in a plan, the steps after it weren't run and the ones before it stand, each undoable. Try again once it's free",
                    words.app, refused.why
                ));
            }
            // Refused before anything was written (changed, failed…): the host forgets the preview.
            Ok(Err(refused)) => {
                self.history.release(std::slice::from_ref(&transaction));
                return ChangeOutcome::error(self.refusal(&refused, &asked));
            }
            Err(_) => {
                self.remember(
                    kind,
                    kind.summary(&title, None, None),
                    &transaction,
                    ChangeState::Unsure,
                    Some(self.unsure_note(kind, &title, &transaction)),
                );
                return ChangeOutcome::stop(format!(
                    "{} didn't answer this change ({title}), so it may or may not have happened: read it before going on",
                    words.app
                ));
            }
        };
        follow_epoch(&self.link().book, &self.link().epoch, &self.link().layout, &applied);
        note_epoch(&self.link().seen_in, &self.link().book.borrow().scheme, &applied);
        let title = applied.pointer("/display/title").and_then(Value::as_str).map(str::to_owned).unwrap_or(title);
        // (r-tracks) Done, and nothing the DAW keeps changed (an action the DAW made no undo point of: a view, selection or
        // transport one): no row in HISTORY, and the host forgets it.
        if applied.pointer("/display/unchanged") == Some(&Value::Bool(true)) {
            self.history.release(std::slice::from_ref(&transaction));
            let why = applied.pointer("/display/note").and_then(Value::as_str).unwrap_or("it changed nothing the project keeps");
            return ChangeOutcome {
                // ("unkept", not "unchanged": it was done, and a plan's summary says it, unlike a step that was so already)
                text: stringify(&json!({"changed": format!("{title}: {}", why.trim_end_matches('.')), "unkept": true})),
                is_error: false,
                missed: None,
                stops: false,
                applied: Some(applied.clone()),
            };
        }
        // What the producer should know of it (a step the DAW's own undo skips), on its row and in its answer.
        let own_undo = self.link().words().vocab.own_undo;
        // Not taken back by the DAW's own undo: its own step makes no undo point, or a plan's point doesn't cover it.
        let skipped = kind.skipped_by_daw_undo
            || applied.pointer("/undo/entry") == Some(&Value::Bool(false))
            || applied.pointer("/undo/covered") == Some(&Value::Bool(false));
        let note = applied
            .pointer("/display/note")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| skipped.then(|| format!("{own_undo} can't take this back; Kumi can.")));
        if kind.kind == "track-add" {
            self.added.set(self.added.get() + 1);
        }
        // (#401) a chunk that ran to its end: one a script may keep
        if kind.kind == "run-lua" && applied.pointer("/applied/failed").is_none() {
            if let Some(code) = input.get("code").and_then(Value::as_str) {
                let mut ran = self.ran.borrow_mut();
                ran.push(chunk_hash(code));
                if ran.len() > MOST_RAN {
                    ran.remove(0);
                }
            }
        }
        // Code run in the DAW may have changed anything, outside what HISTORY can take back: a judged run hears again.
        if matches!(kind.kind, "run-lua" | "api-call") {
            self.history.outside.borrow_mut().push(self.link().daw.outside_code);
        }
        if skipped {
            self.link().outside_daw_undo.borrow_mut().insert(transaction.clone());
        }
        let summary = kind.summary(&title, Some(&preview), Some(&applied));
        let id = self.remember(kind, summary, &transaction, ChangeState::Applied, note.clone());
        // What the listening tools read back: the part a note edit changed (groove takes back its own part's edits), and a
        // track's rename (a judged run follows its track by it).
        if let Some(target) = preview.pointer("/target/ref").and_then(Value::as_str) {
            if kind.kind.starts_with("notes-") {
                self.history.made_on(&id, target);
            }
            let from = preview.pointer("/prior/name").and_then(Value::as_str);
            if let (true, Some(from), Some(to)) = (kind.kind == "track-rename", from, input.get("name").and_then(Value::as_str)) {
                self.renamed.borrow_mut().push((id.clone(), target.to_owned(), from.to_owned(), to.to_owned()));
            }
        }
        // No copy kept first (the saved project was too big, or copying it failed): said with what it deleted.
        let skipped = match self.not_copied.take().filter(|_| kind.deletes) {
            Some(Copied::TooBig(size)) => Some(format!(
                "no copy of the {} was kept first: its saved file is {} MB, past the {} MB Kumi copies",
                words.place,
                size / (1024 * 1024),
                MOST_COPIED / (1024 * 1024)
            )),
            Some(Copied::Failed(why)) => {
                Some(format!("no copy of the {} was kept first: copying its saved file failed ({why})", words.place))
            }
            _ => None,
        };
        let note = match (skipped, note) {
            (Some(skipped), note) => Some(note.map_or(skipped.clone(), |note| format!("{note}; {skipped}"))),
            (None, note) => note,
        };
        // A plan's answer keeps a step's `changed` and not its other fields, so the note goes in it.
        let changed = match &note {
            Some(note) => format!("{title} ({})", note.trim_end_matches('.')),
            None => title,
        };
        let mut reply = json!({"changed": changed, "change": id});
        // The ref it made is named once the book is at the answer's layout (its tracks event may still be on its way); an
        // answer the book has moved past since would name it from before a shift, so it isn't named (a read names it).
        if let Some(made) = applied.pointer("/created/ref").and_then(Value::as_str) {
            let layout = applied.get("layout").and_then(Value::as_u64);
            if layout_of(&self.link().book, &self.link().layout, layout).await != Layout::Older {
                reply["ref"] = json!(self.link().book.borrow_mut().short_ref(made));
            } else {
                reply["refNote"] = json!(format!("Tracks moved in {} meanwhile: read again for the ref of what this made", words.app));
            }
        }
        if let Some(restored) = applied.pointer("/applied/base") {
            reply["base"] = self.link().book.borrow_mut().shorten(restored);
        }
        // What Lua or a call through the DAW's API answered: its value, what it printed, its out-values, or the error it
        // stopped at (what it changed before that stays), with the DAW's refs in them short.
        for key in ["result", "printed", "out", "failed"] {
            if let Some(value) = applied.pointer(&format!("/applied/{key}")) {
                reply[key] = crate::bridge::shorten_anywhere(&mut self.link().book.borrow_mut(), value);
            }
        }
        ChangeOutcome { text: stringify(&reply), is_error: false, missed: None, stops: false, applied: Some(applied) }
    }

    /// One action through the host's tool: what the producer is told it did, or why it didn't.
    pub async fn one_action(&self, action: &'static OwnAction, asked: JsonObject, signal: Signal) -> ActionOutcome {
        let words = self.link().words().words;
        // (an action's refs are this turn's, as a change's are: the tracks a template is saved from, what Renoise auditions or
        // selects, the device whose editor or preset it is)
        let input = match self.lengthened(&asked) {
            Ok(input) => input,
            Err(why) => return ActionOutcome::error(why),
        };
        let mut args = Value::Object(input.clone());
        self.place(&mut args, &input);
        match self.link().call(action.method, args, signal).await {
            Ok(Ok(answer)) => {
                follow_epoch(&self.link().book, &self.link().epoch, &self.link().layout, &answer);
                // The file a save wrote is the project's from now on: a delete later in this answer copies that one first
                // (accept-reaper lab: a delete right after the project's first save kept no copy).
                if let Some(saved) = answer.get("saved").and_then(Value::as_str).filter(|_| action.method == "project_save") {
                    *self.link().file.borrow_mut() = Some(saved.to_owned());
                }
                let playing = action.playing.or_else(|| answer.get("playing").and_then(Value::as_bool));
                let recording = (action.playing == Some(false)).then_some(false);
                // What the DAW says it did, in its words, when it says (Renoise's actions: the notes, what was selected, the file).
                let title = answer.pointer("/display/title").and_then(Value::as_str).unwrap_or(action.title).to_owned();
                let mut text = json!({"done": title});
                // (r-tracks) `saved`: the file a save wrote; (m4-r-fxenv) `file`, `count`: an FX chain saved; (n-devices)
                // `visible`, `listed`: a device's editor, a preset saved.
                for field in
                    ["playing", "positionBeats", "position", "wasRecording", "stopped", "saved", "file", "count", "visible", "listed"]
                {
                    if let Some(value) = answer.get(field) {
                        text[field] = value.clone();
                    }
                }
                for field in ["auditioning", "selected"] {
                    if let Some(value) = answer.get(field) {
                        text[field] = crate::bridge::shorten_anywhere(&mut self.link().book.borrow_mut(), value);
                    }
                }
                if let Some(note) = answer.pointer("/display/note").and_then(Value::as_str) {
                    text["note"] = note.into();
                }
                let done = ActionSummary { title, playing, recording };
                ActionOutcome { text: stringify(&text), is_error: false, maybe: None, done: Some(done) }
            }
            Ok(Err(refused)) if refused.busy => {
                ActionOutcome::error(format!("{} is busy ({}): try again in a moment", words.app, refused.why))
            }
            Ok(Err(refused)) => ActionOutcome::error(self.refusal(&refused, &asked)),
            Err(_) => ActionOutcome {
                text: format!("{} didn't answer, so it may or may not have done it", words.app),
                is_error: true,
                maybe: Some(true),
                done: None,
            },
        }
    }

    /// A change kept in HISTORY under its transaction: its id.
    pub fn remember(&self, kind: &OwnKind, summary: ChangeSummary, transaction: &str, state: ChangeState, note: Option<String>) -> String {
        if let Some(titles) = self.link().step_titles.borrow_mut().as_mut() {
            titles.push(summary.title.clone());
            self.link().in_plan.borrow_mut().insert(transaction.to_owned());
        }
        let mut record = new_record(kind.family, summary, state, self.history.host.now_ms());
        record.note = note;
        let id = record.id.clone();
        if let Some(project) = self.link().project.borrow().clone() {
            self.link().made_in.borrow_mut().insert(transaction.to_owned(), project);
        }
        self.history.remember(record, transaction.into(), None);
        self.history.made_by(&id, kind.tool);
        self.history.changes_this_turn.set(self.history.changes_this_turn.get() + 1);
        self.keep_window();
        id
    }

    /// Kumi's changes the host keeps stay within its journal: past `UNDO_WINDOW` of them, some are let go (one that could
    /// still be undone is then too old for Kumi to undo, and says who can), so the host never runs out of room for a new
    /// one. The change just made stays whatever it takes. What nothing can bring back goes first (ended, or undone by Kumi
    /// where its undo made no step of the DAW's); then changes only Kumi can take back past half the window, so they never
    /// crowd out the rest; then the oldest of the others (one Kumi undid stays as long, since the DAW's undo of Kumi's undo
    /// step brings it back, and the host must be there to follow); the rest of the changes only Kumi can take back go last.
    fn keep_window(&self) {
        let held: Vec<_> =
            self.history.entries.borrow().values().filter(|entry| !entry.borrow().transaction_id.is_empty()).cloned().collect();
        let over = held.len().saturating_sub(UNDO_WINDOW);
        let Some((_, older)) = held.split_last().filter(|_| over > 0) else { return };
        let outside = self.link().outside_daw_undo.borrow().clone();
        let only_kumi = |entry: &Entry<Value>| outside.contains(&entry.borrow().transaction_id);
        let for_good = |entry: &Entry<Value>| match entry.borrow().record.state {
            ChangeState::Expired => true,
            ChangeState::Undone => only_kumi(entry),
            _ => false,
        };
        let (ours, theirs): (Vec<_>, Vec<_>) = older.iter().filter(|entry| !for_good(entry)).partition(|entry| only_kumi(entry));
        let crowding = ours.len().saturating_sub(UNDO_WINDOW / 2);
        let crowded: std::collections::HashSet<String> =
            ours[..crowding].iter().map(|entry| entry.borrow().transaction_id.clone()).collect();
        let order = older
            .iter()
            .filter(|entry| for_good(entry))
            .chain(ours[..crowding].iter().copied())
            .chain(theirs)
            .chain(ours[crowding..].iter().copied());
        let mut released = vec![];
        for entry in order.take(over) {
            let (state, transaction) = {
                let entry = entry.borrow();
                (entry.record.state, entry.transaction_id.clone())
            };
            if matches!(state, ChangeState::Applied | ChangeState::Unsure | ChangeState::Kept) {
                let ending = if crowded.contains(&transaction) { Ending::Crowded } else { Ending::Window };
                let note = self.link().ending_note(&transaction, state, ending);
                self.history.update(entry, ChangeState::Expired, Some(note));
            }
            released.push(std::mem::take(&mut entry.borrow_mut().transaction_id));
        }
        self.history.release(&released);
    }

    /// Kumi's host started again, with a new journal: no change before it can be undone through it, and each row says who
    /// can still take it back.
    pub fn retire_all(&self, ending: Ending) {
        let entries: Vec<_> = self.history.entries.borrow().values().cloned().collect();
        for entry in entries {
            let (state, transaction) = {
                let entry = entry.borrow();
                (entry.record.state, entry.transaction_id.clone())
            };
            if matches!(state, ChangeState::Applied | ChangeState::Unsure) {
                let note = self.link().ending_note(&transaction, state, ending);
                self.history.update(&entry, ChangeState::Expired, Some(note));
            } else if state == ChangeState::Undone && !self.link().outside_daw_undo.borrow().contains(&transaction) {
                // The DAW's undo of Kumi's undo would put it back, and Kumi can't see that any more.
                let note = self.link().undone_note(ending);
                self.history.update(&entry, ChangeState::Undone, Some(note));
            }
        }
    }

    /// What an Unsure row tells the producer: Kumi won't undo blindly what may not have happened, so where to look, and how
    /// to take it back if it did (the DAW's own undo, unless it skips this kind; with the rest of a plan's step in it).
    pub fn unsure_note(&self, kind: &OwnKind, title: &str, transaction: &str) -> String {
        if kind.skipped_by_daw_undo {
            self.link().outside_daw_undo.borrow_mut().insert(transaction.to_owned());
        }
        let in_plan = self.link().step_titles.borrow().is_some();
        unsure_note(self.link().words(), title, kind.skipped_by_daw_undo, in_plan)
    }
}

/// The most of a saved file Kumi copies before a delete, and the copies of one project it keeps.
pub const MOST_COPIED: u64 = 512 * 1024 * 1024;
pub const COPIES_KEPT: usize = 5;

/// When a copy was saved, from its name ("<stem> (Kumi copy 2026-10-20 14.05.31 +0900)<extension>", `prefix` and
/// `suffix` around the time): its local time and offset, or UTC for a copy named without an offset; None for a name
/// that isn't one.
fn copy_saved_at(name: &str, prefix: &str, suffix: &str) -> Option<i64> {
    let when = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    chrono::DateTime::parse_from_str(when, "%Y-%m-%d %H.%M.%S %z")
        .map(|at| at.timestamp())
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(when, "%Y-%m-%d %H.%M.%S").map(|at| at.and_utc().timestamp()))
        .ok()
}

/// A copy of the project's saved file beside it, in its own folder (so a REAPER project's media, which it names
/// relative to that folder, is found from the copy too), named by when it was saved in the producer's local time (with
/// its offset, so copies made in another time zone still sort by when they were saved): one saved version is copied once
/// whatever the session. Its path; nothing when there's nothing to copy (never saved, copied already); or why it wasn't
/// copied (over `MOST_COPIED`, or the copy failed). The oldest copies of the project past `COPIES_KEPT` go.
pub fn copy_beside(path: &Path) -> Copied {
    let Ok(stats) = std::fs::metadata(path) else { return Copied::Nothing };
    if stats.len() > MOST_COPIED {
        return Copied::TooBig(stats.len());
    }
    let made = || -> Result<Option<String>, String> {
        let unnamed = || "the project has no name to copy it by".to_string();
        let saved: chrono::DateTime<chrono::Local> = stats.modified().map_err(|why| why.to_string())?.into();
        let folder = path.parent().ok_or_else(unnamed)?;
        let stem = path.file_stem().ok_or_else(unnamed)?.to_string_lossy().into_owned();
        let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let (prefix, suffix) = (format!("{stem} (Kumi copy "), format!("){extension}"));
        let name = format!("{prefix}{}{suffix}", saved.format("%Y-%m-%d %H.%M.%S %z"));
        let copy = folder.join(&name);
        if copy.exists() {
            return Ok(None);
        }
        // Copied under another name and renamed once whole: a copy that stopped part-way (a full disk) is never taken
        // for this version's.
        let part = folder.join(format!(".{name}.part"));
        if let Err(why) = std::fs::copy(path, &part).and_then(|_| std::fs::rename(&part, &copy)) {
            let _ = std::fs::remove_file(&part);
            return Err(why.to_string());
        }
        // This project's copies only (another with the same name in the folder, Song.rpp and Song.xrns, keeps its own),
        // and never the one just made (an older save than those kept is copied too, and named in the answer).
        let mut copies: Vec<_> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|kept| {
                *kept != copy && kept.file_name().is_some_and(|name| copy_saved_at(&name.to_string_lossy(), &prefix, &suffix).is_some())
            })
            .collect();
        // Oldest first, by when each was saved (its name's time and offset), not by how its name sorts.
        copies.sort_by_key(|kept| {
            let name = kept.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            (copy_saved_at(&name, &prefix, &suffix).unwrap_or(i64::MIN), name)
        });
        for old in copies.iter().take((copies.len() + 1).saturating_sub(COPIES_KEPT)) {
            let _ = std::fs::remove_file(old);
        }
        Ok(Some(copy.to_string_lossy().into_owned()))
    };
    match made() {
        Ok(Some(copy)) => Copied::Made(copy),
        Ok(None) => Copied::Nothing,
        Err(why) => Copied::Failed(why),
    }
}

/// What a copy before a delete came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Copied {
    Made(String),
    /// The saved file is past `MOST_COPIED` (its size): nothing copied, and the producer is told.
    TooBig(u64),
    /// Copying it failed (a folder Kumi can't write in, a full disk): why. Nothing copied, and the producer is told.
    Failed(String),
    /// Never saved, or this saved version is copied already.
    Nothing,
}

/// What an Unsure row tells the producer: Kumi won't undo blindly what may not have happened, so where to look, and how
/// to take it back if it did.
pub fn unsure_note(profile: &DawProfile, title: &str, only_kumi: bool, in_plan: bool) -> String {
    let (app, own_undo) = (profile.words.app, profile.vocab.own_undo);
    if only_kumi {
        format!("Kumi isn't sure this went through ({title}): check it in {app}, and change it back by hand if it did ({own_undo} doesn't take this back).")
    } else if in_plan {
        format!("Kumi isn't sure this went through ({title}): check it in {app}; if it did, {own_undo} takes it back together with the plan's other changes.")
    } else {
        format!("Kumi isn't sure this went through ({title}): check it in {app}, and {own_undo} takes it back if it did.")
    }
}

/// What an Unsure row says when Kumi's undo of it was sent and never answered: the change was made; whether the undo was
/// isn't known.
pub fn unsure_undo_note(profile: &DawProfile, title: &str) -> String {
    format!(
        "Kumi isn't sure its undo went through ({title}): check it in {}, and change it back by hand if it's still as Kumi set it.",
        profile.words.app
    )
}

/// The first short ref a request names that lengthening left short: one this turn doesn't know.
fn find_unknown(value: &Value, key: &str, out: &mut Option<String>, depth: usize) {
    if out.is_some() || depth > 16 {
        return;
    }
    match value {
        Value::String(text)
            if (key == "ref" || key == "parent" || key.ends_with("Ref") || key.ends_with("Refs")) && SHORT_REF.is_match(text) =>
        {
            *out = Some(text.clone())
        }
        Value::Array(items) => items.iter().for_each(|item| find_unknown(item, key, out, depth + 1)),
        Value::Object(row) => row.iter().for_each(|(key, item)| find_unknown(item, key, out, depth + 1)),
        _ => {}
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> PlanHost for OwnPlan<S> {
    type Changes = OwnChanges<S>;
    type Change = OwnKind;
    type Action = OwnAction;
    fn history(&self) -> &History<OwnChanges<S>> {
        &self.history
    }
    fn names(&self) -> &RefCell<IndexMap<String, String>> {
        &self.names
    }
    fn change_kind(&self, tool: &str) -> Option<&'static OwnKind> {
        self.link().daw.changes.kinds.iter().find(|kind| kind.tool == tool)
    }
    fn action_kind(&self, tool: &str) -> Option<&'static OwnAction> {
        self.link().daw.changes.actions.iter().find(|action| action.tool == tool)
    }
    fn structural(&self, tool: &str) -> bool {
        self.change_kind(tool).is_some_and(|kind| kind.structural)
    }
    fn moves_transport(&self, tool: &str) -> bool {
        self.action_kind(tool).is_some()
    }
    fn may_lose_material(&self, tool: &str) -> bool {
        self.change_kind(tool).is_some_and(|kind| kind.deletes)
    }
    fn supported(&self, _since: Option<&str>) -> bool {
        true
    }
    fn too_old(&self, _since: Option<&str>) -> String {
        String::new()
    }
    fn tempo(&self) -> Option<f64> {
        self.link().tempo.get()
    }
    fn batches(&self) -> &[Batch] {
        &[]
    }
    fn batch_kind(&self, _batch: &Batch) -> &'static OwnKind {
        unreachable!("own DAWs offer no batches")
    }
    fn batch_offered(&self, _batch: &Batch) -> bool {
        false
    }
    fn batch_input(&self, _batch: &Batch, _group: &[JsonObject]) -> JsonObject {
        JsonObject::new()
    }
    /// A step is held to its kind's schema, as a call of its own tool is: a field the kind doesn't take, one it needs
    /// that's missing, or one of the wrong type is refused in words, never sent to the bridge to be dropped or guessed.
    async fn change(&self, kind: &'static OwnKind, input: JsonObject, signal: Signal, _confirmed: bool) -> ChangeOutcome {
        if let Some(problem) = schema::problem(&Value::Object(kind.input_schema()), &Value::Object(input.clone())) {
            return ChangeOutcome::error(format!("{}: {problem}", kind.tool));
        }
        self.one_change(kind, input, signal).await
    }
    async fn act(&self, action: &'static OwnAction, input: JsonObject, signal: Signal) -> ActionOutcome {
        if let Some(problem) = schema::problem(&Value::Object(action.input_schema()), &Value::Object(input.clone())) {
            return ActionOutcome::error(format!("{}: {problem}", action.tool));
        }
        self.one_action(action, input, signal).await
    }
    async fn safety_copy(&self, _signal: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(self.copy_before_delete().await)
    }
    /// A copy only before a step that can lose material: a whole song (samples and all) per long plan is too much.
    fn copies_long_plans(&self) -> bool {
        false
    }
    fn emit_action(&self, done: &ActionSummary) {
        if let Some(on_action) = &self.on_action {
            on_action(ActionEvent { title: done.title.clone(), playing: done.playing, recording: done.recording });
        }
    }
}

/// One change kind's own tool.
struct ChangeTool<S: RefScheme + 'static> {
    engine: Rc<OwnPlan<S>>,
    kind: &'static OwnKind,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> KernelTool for ChangeTool<S> {
    fn name(&self) -> &str {
        self.kind.tool
    }
    fn description(&self) -> &str {
        self.kind.description
    }
    fn input_schema(&self) -> JsonObject {
        self.kind.input_schema()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        // A delete on its own keeps the copy a plan keeps before one.
        let copy = match self.kind.deletes {
            true => self.engine.copy_before_delete().await,
            false => None,
        };
        let outcome = self.engine.one_change(self.kind, input, signal).await;
        let mut text = outcome.text;
        if let (Some(copy), false) = (copy, outcome.is_error) {
            if let Ok(Value::Object(mut reply)) = serde_json::from_str::<Value>(&text) {
                reply.insert("copy".into(), json!(copy));
                text = stringify(&Value::Object(reply));
            }
        }
        Ok(ToolResult { text, is_error: outcome.is_error, ..Default::default() })
    }
    fn deferred(&self) -> bool {
        !self.kind.core
    }
    fn listed_by(&self) -> Option<&str> {
        Some("make_changes")
    }
}

/// One action's own tool.
struct ActionTool<S: RefScheme + 'static> {
    engine: Rc<OwnPlan<S>>,
    action: &'static OwnAction,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> KernelTool for ActionTool<S> {
    fn name(&self) -> &str {
        self.action.tool
    }
    fn description(&self) -> &str {
        self.action.description
    }
    fn input_schema(&self) -> JsonObject {
        self.action.input_schema()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        let outcome = self.engine.one_action(self.action, input, signal).await;
        if let Some(done) = outcome.done.as_ref().filter(|_| !outcome.is_error) {
            self.engine.emit_action(done);
        }
        Ok(ToolResult { text: outcome.text, is_error: outcome.is_error, ..Default::default() })
    }
    fn deferred(&self) -> bool {
        !self.action.core
    }
    fn listed_by(&self) -> Option<&str> {
        Some("make_changes")
    }
}

/// make_changes: the DAW's changes as steps of one plan.
struct PlanTool<S: RefScheme + 'static> {
    engine: Rc<OwnPlan<S>>,
    description: String,
    schema: JsonObject,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> KernelTool for PlanTool<S> {
    fn name(&self) -> &str {
        "make_changes"
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        self.schema.clone()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        make_changes(&self.engine, input, signal).await
    }
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        Some(stream_changes(&self.engine, signal, on_start))
    }
}

/// undo_change: one of Kumi's changes taken back as a new step of the DAW's undo.
struct UndoTool<S: RefScheme + 'static> {
    engine: Rc<OwnPlan<S>>,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> KernelTool for UndoTool<S> {
    fn name(&self) -> &str {
        "undo_change"
    }
    fn description(&self) -> &str {
        self.engine.link().daw.changes.undo_description
    }
    fn input_schema(&self) -> JsonObject {
        serde_json::from_str(UNDO_SCHEMA).expect("JSON")
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        // Discard is the own DAWs' (their schema offers it); Live's undo_change never reads it.
        let discard = input.get("discard") == Some(&Value::Bool(true));
        undo_change_with(&self.engine.history, &input, signal, discard).await
    }
}
