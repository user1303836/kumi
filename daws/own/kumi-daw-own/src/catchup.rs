//! Catch-up and watch_me for the DAWs whose bridges are Kumi's own (REAPER, Renoise): what Kumi's host answers
//! (`catch_up`, `watch_stop`) said in core's words (`describe_changes`, `describe_watch_changes`) with the DAW's names for
//! its things, and watch_me's draft of a recipe in the DAW's change kinds. The host keeps the snapshots and compares
//! them; the integration (`own_bridge`) decides when.
use super::own_bridge::{answer_of, stopped_or, CALL_MS};
use crate::core::contracts::CatchUp;
use crate::core::{
    contracts::{JsonObject, KernelTool, Reach, ToolResult},
    describe::{describe_changes, describe_watch_changes, DescribedDiff, Detail, ProjectChange, ProjectItem, ProjectWords},
    errors::RuntimeError,
};
use crate::mcp::client::McpEndpoint;
use async_trait::async_trait;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::json::stringify,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    time::Duration,
};
use tokio::time::Instant;

/// The project's snapshot is kept again once the producer's (or Kumi's) edits have settled this long…
const SETTLE: Duration = Duration::from_secs(2);
/// …and no sooner than this after the last time, or fifty times as long as the last took (a big project's digest): a DAW
/// quit soon after an edit then reads back what changed while it was away, not the edits made while Kumi watched.
const MOST_OFTEN: Duration = Duration::from_secs(5);
/// A look this long after the last snapshot keeps one again, whatever the counter says (Live's rule).
const LOOK_STALE: Duration = Duration::from_secs(5 * 60);
/// How long Kumi's close waits for the last snapshot.
pub const CLOSE_MS: u64 = 2_500;

/// What a DAW says of its project's changes, beside core's describers.
pub struct CatchUpWords {
    /// Its names for the things in a project.
    pub words: ProjectWords,
    /// The project's own values that changed (the host's `project` details), as catch-up lines: "Tempo 120 → 124 BPM".
    pub facts: fn(&[Value]) -> Vec<String>,
    /// How watch_me drafts a recipe in its change kinds.
    pub recipe: RecipeKinds,
    /// watch_me's description, in the DAW's words.
    pub watch_description: &'static str,
}

/// A step of a draft: the kind and its input.
pub type DraftStep = (&'static str, JsonObject);

/// The change kinds a draft recipe is made of.
pub struct RecipeKinds {
    /// The kind that adds a track (`{name}`), and the kinds of track (the host's `kind` facet) it makes by its `type`
    /// besides its default one.
    pub track_add: &'static str,
    pub track_types: &'static [&'static str],
    /// The kind of record an effect is ("FX", "device"), and the step that adds one to a track (without its `ref`), from
    /// what it's made of and its name.
    pub effect: &'static str,
    pub effect_add: fn(&JsonObject, &str) -> Option<DraftStep>,
    /// The step that sets an effect's parameters by name (without its `ref`), from (name, value) pairs as watch_me read
    /// them.
    pub effect_values: fn(&[(String, Value)]) -> DraftStep,
    /// A track's mixer values by what the host calls them, with the kind that sets each.
    pub mixer: &'static [Mixer],
}

/// One of a track's mixer values (`mixer.volume`), the kind that sets it (`track_volume`) and its field (`volumeDb`), the
/// value the kind takes from what the host read (None when the kind can't set it), and what else the step always takes
/// (Renoise's `stage`).
pub struct Mixer {
    pub what: &'static str,
    pub tool: &'static str,
    pub field: &'static str,
    pub value: fn(&Value) -> Option<Value>,
    pub with: &'static [(&'static str, &'static str)],
}

/// A mixer value as the host read it, the kind takes it as it is.
pub fn as_read(value: &Value) -> Option<Value> {
    (!value.is_null()).then(|| value.clone())
}

/// A thing as a change names it, read from the host's answer.
fn item(value: &Value) -> ProjectItem {
    ProjectItem {
        name: value.get("name").cloned(),
        order: value.get("order").cloned(),
        on: value.get("on").and_then(Value::as_str).map(str::to_owned),
    }
}

fn detail(value: &Value) -> Detail {
    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    Detail {
        facet: text("facet"),
        what: text("what"),
        from: value.get("from").cloned().unwrap_or(Value::Null),
        to: value.get("to").cloned().unwrap_or(Value::Null),
        fingerprint: value.get("fingerprint") == Some(&Value::Bool(true)),
    }
}

/// The host's changes (`catch_up`, `watch_stop`) as core's, the project's own values first, in the DAW's words.
pub fn changes_of(answer: &Value, words: &CatchUpWords) -> Vec<ProjectChange> {
    let mut changes = vec![];
    let facts = (words.facts)(answer.get("project").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
    if !facts.is_empty() {
        changes.push(ProjectChange::Changed {
            kind: words.words.project.into(),
            before: None,
            after: None,
            renamed: false,
            details: vec![],
            lines: Some(facts),
        });
    }
    for change in answer.get("changes").and_then(Value::as_array).into_iter().flatten() {
        let kind = change.get("kind").and_then(Value::as_str).unwrap_or("").to_owned();
        changes.push(match change.get("change").and_then(Value::as_str) {
            Some("added") => ProjectChange::Added {
                kind,
                item: item(&change["item"]),
                made_of: change.get("madeOf").and_then(Value::as_object).cloned().unwrap_or_default(),
            },
            Some("removed") => ProjectChange::Removed { kind, item: item(&change["item"]) },
            Some("changed") => ProjectChange::Changed {
                kind,
                before: change.get("before").map(item),
                after: change.get("after").map(item),
                renamed: change.get("renamed") == Some(&Value::Bool(true)),
                details: change.get("details").and_then(Value::as_array).into_iter().flatten().map(detail).collect(),
                lines: None,
            },
            _ => continue,
        });
    }
    changes
}

/// When Kumi keeps the open project's snapshot again, and what catch-up told of it, for one integration. The host keeps
/// the snapshots and compares them (`catch_up`, `baseline_save`); this decides when to ask.
#[derive(Default)]
pub struct Remembered {
    /// The open project (as this run of the DAW names it) catch-up last ran for, and what it told of it.
    caught: RefCell<Option<String>>,
    /// Every project catch-up ran for in this run: one seen again (a tab switched back to) is told only what changed.
    seen: RefCell<std::collections::HashSet<String>>,
    context: RefCell<Option<(String, JsonObject)>>,
    /// The DAW went away after a project was seen: the next catch-up tells what changed while it was away.
    away: Cell<bool>,
    running: Cell<bool>,
    saving: Cell<bool>,
    /// The host can't compare (its bridge has no digest): nothing more is asked of it.
    unsupported: Cell<bool>,
    /// The host wouldn't keep the snapshot of the project open now before comparing it (another tab, another run of the
    /// DAW, or the host started again): nothing is kept until catch-up runs at the next look.
    uncompared: Cell<bool>,
    /// Kumi's host started again while the session went on: its next catch-up compares, and tells nothing (what changed
    /// meanwhile, Kumi's changes among it, the looks saw), unless the DAW went away too.
    quiet: Cell<bool>,
    /// When the snapshot was last kept, how long that took, and the DAW's change counter then.
    saved_at: Cell<Option<Instant>>,
    took_ms: Cell<u64>,
    saved_counter: RefCell<Option<Value>>,
    /// Saves in a row that the host answered with an error, or that got no answer: each waits twice as long as the last.
    failed: Cell<u32>,
    /// The change counter as the focus last showed it, and when something last changed (the counter moved, or Kumi made a
    /// change).
    counter: RefCell<Option<Value>>,
    moved_at: Cell<Option<Instant>>,
}

impl Remembered {
    /// What catch-up told of the open project, for its observation (`sinceLastTime`).
    pub fn since(&self, identity: &str) -> Option<JsonObject> {
        self.context.borrow().as_ref().filter(|(of, _)| of == identity).map(|(_, context)| context.clone())
    }

    /// Whether catch-up should run for the open project: it hasn't for this one (or the DAW went away since), nor is it
    /// running.
    pub fn should_catch_up(&self, identity: &str) -> bool {
        !self.unsupported.get()
            && !self.running.get()
            && (self.away.get() || self.uncompared.get() || self.caught.borrow().as_deref() != Some(identity))
    }

    /// Kumi's host started again: it has compared nothing yet, so it keeps no snapshot before the next catch-up.
    pub fn host_restarted(&self) {
        if self.caught.borrow().is_some() {
            self.uncompared.set(true);
            self.quiet.set(true);
        }
    }

    /// The DAW is gone (it quit, or its bridge did): what changes before it's back is told as changed while it was away.
    pub fn went_away(&self) {
        if self.caught.borrow().is_some() {
            self.away.set(true);
        }
    }

    /// The change counter the focus shows now.
    pub fn noted(&self, counter: Option<&Value>) {
        let Some(counter) = counter.filter(|c| !c.is_null()) else { return };
        if self.counter.borrow().as_ref() != Some(counter) {
            let first = self.counter.replace(Some(counter.clone())).is_none();
            if !first {
                self.moved_at.set(Some(Instant::now()));
            }
        }
    }

    /// Kumi made a change (or undid one): the snapshot is kept again once things settle.
    pub fn kumi_changed(&self) {
        self.moved_at.set(Some(Instant::now()));
    }

    /// Whether to keep the snapshot again now: catch-up has run for the open project, something changed since and has
    /// settled, and the last time was long enough ago.
    pub fn due(&self) -> bool {
        if self.caught.borrow().is_none() || self.unsupported.get() || self.uncompared.get() || self.saving.get() || self.running.get() {
            return false;
        }
        let Some(moved) = self.moved_at.get() else { return false };
        let failed = self.failed.get();
        let backoff = if failed == 0 { Duration::ZERO } else { MOST_OFTEN * (1u32 << failed.min(6)) };
        let gap = MOST_OFTEN.max(Duration::from_millis(self.took_ms.get().saturating_mul(50))).max(backoff);
        moved.elapsed() >= SETTLE && self.saved_at.get().is_none_or(|at| at.elapsed() >= gap)
    }

    /// Whether a look should keep the snapshot again: the last time was long ago.
    pub fn stale(&self) -> bool {
        self.caught.borrow().is_some()
            && !self.unsupported.get()
            && !self.uncompared.get()
            && !self.saving.get()
            && !self.running.get()
            && self.saved_at.get().is_none_or(|at| at.elapsed() >= LOOK_STALE)
    }

    /// Keeps the open project's snapshot again through the host. Refused by the host until it has compared the project
    /// (it says so): nothing is asked again until catch-up has run. A refusal still read the whole project, so the next
    /// try waits as long as after a kept one.
    pub async fn save(self: Rc<Self>, host: Rc<dyn McpEndpoint>, signal: Signal) {
        if self.saving.replace(true) {
            return;
        }
        let began = Instant::now();
        let answer = host.call("baseline_save", JsonObject::new(), signal).await.ok().map(|result| answer_of(&result));
        if !matches!(answer, Some(Ok(_))) {
            // An error (a full disk where the host keeps the snapshots, a digest past its time) or no answer: asked again
            // later, and later each time, never every second (each try reads the whole project).
            self.saved_at.set(Some(Instant::now()));
            self.failed.set(self.failed.get().saturating_add(1));
        }
        if let Some(Ok(answer)) = answer {
            self.failed.set(0);
            self.saved_at.set(Some(Instant::now()));
            self.took_ms.set(answer.get("ms").and_then(Value::as_u64).unwrap_or(began.elapsed().as_millis() as u64));
            if answer.get("saved") == Some(&Value::Bool(true)) {
                *self.saved_counter.borrow_mut() = answer.get("changed").cloned();
                // (what changed while it was kept is kept next time)
                if self.moved_at.get().is_some_and(|moved| moved <= began) {
                    self.moved_at.set(None);
                }
            } else if answer.get("compared") == Some(&Value::Bool(false)) {
                self.uncompared.set(true);
            }
        }
        self.saving.set(false);
    }

    /// Asks the host what changed in the open project since its last snapshot (the host keeps it again after). What it
    /// tells goes to `on_catch_up` and, while the project stays open, to its observations. A busy or failed catch-up is
    /// asked again at the next look; a bridge that can't compare, never again.
    pub async fn catch_up(
        self: Rc<Self>,
        host: Rc<dyn McpEndpoint>,
        identity: String,
        name: String,
        words: &'static CatchUpWords,
        on_catch_up: Option<Rc<dyn Fn(CatchUp)>>,
        signal: Signal,
    ) {
        if self.running.replace(true) {
            return;
        }
        let after_reconnect = self.away.replace(false);
        let quiet = self.quiet.replace(false);
        let answer = host.call("catch_up", JsonObject::new(), signal).await.ok().map(|result| answer_of(&result));
        self.running.set(false);
        let answer = match answer {
            Some(Ok(answer)) => answer,
            Some(Err(refused)) if refused.code.as_deref() == Some("unsupported") => {
                self.unsupported.set(true);
                return;
            }
            _ => {
                self.away.set(after_reconnect);
                self.quiet.set(quiet);
                return;
            }
        };
        *self.caught.borrow_mut() = Some(identity.clone());
        self.uncompared.set(false);
        let again = !self.seen.borrow_mut().insert(identity.clone());
        self.saved_at.set(Some(Instant::now()));
        self.took_ms.set(answer.get("ms").and_then(Value::as_u64).unwrap_or(0));
        self.moved_at.set(None);
        if quiet && !after_reconnect {
            return;
        }
        *self.context.borrow_mut() = None;
        let Some(described) = caught_up(&answer, words) else { return };
        // (back to a project seen in this run, unchanged: a tab switched back to, nothing to say)
        if again && described.lines.is_empty() && !after_reconnect {
            return;
        }
        let last_seen_at = answer.get("lastSeenAt").and_then(Value::as_i64).unwrap_or(0);
        let mut context = JsonObject::new();
        context.insert("lastSeen".into(), kumi_common::time::since(last_seen_at as f64, kumi_common::time::now_ms() as f64).into());
        context.insert("changes".into(), json!(described.lines));
        if described.more != 0 {
            context.insert("more".into(), described.more.into());
        }
        *self.context.borrow_mut() = Some((identity, context));
        let summary = CatchUp {
            set: name,
            last_seen_at,
            lines: described.lines,
            more: described.more,
            after_reconnect: after_reconnect.then_some(true),
        };
        if let Some(callback) = on_catch_up {
            let _ = catch_unwind(AssertUnwindSafe(|| callback(summary)));
        }
    }
}

/// What catch-up tells of the host's `catch_up` answer: nothing for a project first seen here, no lines when nothing
/// changed, else core's lines (and, past them, how many more); None as well when what changed has no words.
pub fn caught_up(answer: &Value, words: &CatchUpWords) -> Option<DescribedDiff> {
    if answer.get("first") == Some(&Value::Bool(true)) {
        return None;
    }
    if answer.get("same") == Some(&Value::Bool(true)) {
        return Some(DescribedDiff { lines: vec![], more: 0 });
    }
    let changes = changes_of(answer, words);
    let mut described = describe_changes(&changes, &words.words, None);
    if described.lines.is_empty() {
        return None;
    }
    described.more += answer.get("more").and_then(Value::as_u64).unwrap_or(0) as usize;
    Some(described)
}

/// A recipe blank's name from a track's name: "$bass", "$lead_vox"; never one already used.
fn blank_name(name: &str, taken: &[String]) -> String {
    let mut base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base = format!("t_{base}");
    }
    base.truncate(28);
    let base = base.trim_end_matches('_').to_string();
    let mut candidate = base.clone();
    let mut n = 2;
    while taken.contains(&candidate) {
        candidate = format!("{base}{n}");
        n += 1;
    }
    candidate
}

/// The steps a draft is made of, the blanks they use and what they are.
#[derive(Default)]
struct Draft {
    steps: Vec<Value>,
    params: Vec<(String, String)>,
    /// A track's name → what a step names it by: "@t1" (made by a step) or "$bass" (chosen when it runs).
    tracks: Vec<(String, String)>,
    made: usize,
    effects: usize,
    left_out: Vec<String>,
}

impl Draft {
    /// The name a step gives a track: a mark when a step made it, else a blank: $track for the one the producer selected
    /// (or, with none selected, the first they worked on), the others' after their names.
    fn track(&mut self, name: &str, selected: Option<&str>) -> String {
        if let Some((_, by)) = self.tracks.iter().find(|(track, _)| track == name) {
            return by.clone();
        }
        let taken: Vec<String> = self.params.iter().map(|(blank, _)| blank.clone()).collect();
        let main = selected.is_none_or(|selected| selected == name);
        let blank = if main && !taken.iter().any(|t| t == "track") { "track".to_string() } else { blank_name(name, &taken) };
        self.params.push((blank.clone(), format!("the track to work on (it was “{name}”)")));
        let by = format!("${blank}");
        self.tracks.push((name.to_owned(), by.clone()));
        by
    }

    fn step(&mut self, tool: &str, input: JsonObject, mark: Option<String>) {
        let mut step = json!({"tool": tool, "input": input});
        if let Some(mark) = mark {
            step["as"] = mark.into();
        }
        self.steps.push(step);
    }
}

/// A watched value as the parameter's own (`{value, shows}` → value).
fn plain(value: &Value) -> Value {
    value.get("value").cloned().unwrap_or_else(|| value.clone())
}

/// watch_me's draft of a recipe: the steps that would make what the producer did again, in the DAW's change kinds, with
/// a blank for each existing track they worked on (the one they selected is $track) and marks for what the steps make.
/// What no change kind here makes (items, notes, markers) is listed as left out.
pub fn draft(answer: &Value, words: &CatchUpWords) -> Option<Value> {
    let kinds = &words.recipe;
    let selected = answer
        .get("selected")
        .and_then(Value::as_array)
        .and_then(|tracks| tracks.iter().rev().find_map(|track| track.get("name").and_then(Value::as_str)))
        .map(str::to_owned);
    // (a track selected that the routine didn't touch says nothing of which one is the track)
    let touched = |name: &str| {
        answer.get("changes").and_then(Value::as_array).into_iter().flatten().any(|change| {
            let thing = change.get("item").or_else(|| change.get("after")).unwrap_or(&Value::Null);
            thing.get("on").and_then(Value::as_str) == Some(name)
                || (change.get("kind").and_then(Value::as_str) == Some("track") && thing.get("name").and_then(Value::as_str) == Some(name))
        })
    };
    let selected = selected.as_deref().filter(|name| touched(name));
    let mut draft = Draft::default();
    let mixer_step = |draft: &mut Draft, by: &str, what: &str, value: &Value| {
        let Some(mixer) = kinds.mixer.iter().find(|mixer| mixer.what == what) else { return false };
        let Some(value) = (mixer.value)(value) else { return false };
        let mut input = JsonObject::new();
        input.insert("ref".into(), by.into());
        input.insert(mixer.field.into(), value);
        for (key, value) in mixer.with {
            input.insert((*key).into(), (*value).into());
        }
        draft.step(mixer.tool, input, None);
        true
    };
    for change in answer.get("changes").and_then(Value::as_array).into_iter().flatten() {
        let kind = change.get("kind").and_then(Value::as_str).unwrap_or("");
        let what = change.get("change").and_then(Value::as_str).unwrap_or("");
        let thing = change.get("item").or_else(|| change.get("after")).cloned().unwrap_or(Value::Null);
        let name = thing.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        let on = thing.get("on").and_then(Value::as_str).map(str::to_owned);
        match (kind, what) {
            ("track", "added") => {
                draft.made += 1;
                let mark = format!("t{}", draft.made);
                let made = change.get("madeOf").cloned().unwrap_or(Value::Null);
                let mut input = JsonObject::new();
                if !name.is_empty() {
                    input.insert("name".into(), name.clone().into());
                }
                // (a send or group track made as one; what's put on it comes as its own changes, after it)
                if let Some(kind) = made.get("kind").and_then(Value::as_str).filter(|kind| kinds.track_types.contains(kind)) {
                    input.insert("type".into(), kind.into());
                }
                draft.step(kinds.track_add, input, Some(mark.clone()));
                draft.tracks.push((name.clone(), format!("@{mark}")));
                for (leaf, value) in made.get("mixer").and_then(Value::as_object).into_iter().flatten() {
                    let neutral = matches!(value, Value::Bool(false)) || value.as_f64() == Some(0.0) || value.is_null();
                    if !neutral {
                        mixer_step(&mut draft, &format!("@{mark}"), &format!("mixer.{leaf}"), value);
                    }
                }
            }
            ("track", "changed") => {
                let by = draft.track(&name, selected);
                for detail in change.get("details").and_then(Value::as_array).into_iter().flatten() {
                    let what = detail.get("what").and_then(Value::as_str).unwrap_or("");
                    if !mixer_step(&mut draft, &by, what, &detail["to"]) && detail.get("fingerprint") != Some(&Value::Bool(true)) {
                        draft.left_out.push(format!("track “{name}”: {what}"));
                    }
                }
            }
            (effect, "added") if effect == kinds.effect => {
                let Some(track) = on else { continue };
                let made = change.get("madeOf").and_then(Value::as_object).cloned().unwrap_or_default();
                let Some((tool, mut input)) = (kinds.effect_add)(&made, &name) else {
                    draft.left_out.push(format!("{kind} “{name}”"));
                    continue;
                };
                let by = draft.track(&track, selected);
                input.insert("ref".into(), by.into());
                draft.effects += 1;
                let mark = format!("fx{}", draft.effects);
                draft.step(tool, input, Some(mark.clone()));
                let values: Vec<(String, Value)> = made
                    .get("parameters")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Some((p.get("name")?.as_str()?.to_owned(), plain(&p["value"]))))
                    .collect();
                if !values.is_empty() {
                    let (tool, mut input) = (kinds.effect_values)(&values);
                    input.insert("ref".into(), format!("@{mark}").into());
                    draft.step(tool, input, None);
                }
                // (past the host's limit it names none of them, only how many moved: the effect is drafted as it comes)
                if let Some(moved) = made.get("parametersMoved").and_then(Value::as_u64).filter(|moved| *moved > 0) {
                    let plural = if moved == 1 { "" } else { "s" };
                    draft.left_out.push(format!("{kind} “{name}”: {moved} parameter{plural} moved, not drafted"));
                }
            }
            (effect, "changed") if effect == kinds.effect => {
                let values: Vec<(String, Value)> = change
                    .get("details")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    // (a `…Hash` leaf is the parameters' fingerprint, never one of them)
                    .filter(|d| d.get("fingerprint") != Some(&Value::Bool(true)))
                    .filter_map(|d| Some((d.get("what")?.as_str()?.strip_prefix("parameters.")?.to_owned(), plain(&d["to"]))))
                    .collect();
                if values.is_empty() {
                    draft.left_out.push(format!("{kind} “{name}”"));
                    continue;
                }
                // An effect that was there before: a blank, chosen when the recipe runs.
                let taken: Vec<String> = draft.params.iter().map(|(blank, _)| blank.clone()).collect();
                let blank = blank_name(&name, &taken);
                let on = on.map(|track| format!(" on “{track}”")).unwrap_or_default();
                draft.params.push((blank.clone(), format!("the {kind} to set (it was “{name}”{on})")));
                let (tool, mut input) = (kinds.effect_values)(&values);
                input.insert("ref".into(), format!("${blank}").into());
                draft.step(tool, input, None);
            }
            (_, what) => draft.left_out.push(format!("{kind} “{name}” {what}")),
        }
    }
    if draft.steps.is_empty() {
        return None;
    }
    let mut out = json!({
        "params": draft.params.iter().map(|(name, about)| json!({"name": name, "about": about})).collect::<Vec<_>>(),
        "steps": draft.steps,
    });
    if !draft.left_out.is_empty() {
        draft.left_out.truncate(12);
        out["leftOut"] = draft.left_out.into();
    }
    Some(out)
}

/// watch_me's answer from the host's `watch_stop`: core's changes in the DAW's words, and the draft recipe.
pub fn watched(answer: &Value, words: &CatchUpWords) -> JsonObject {
    let described = describe_watch_changes(&changes_of(answer, words), &words.words, None);
    let mut reply = JsonObject::new();
    reply.insert("watched".into(), answer.get("watched").cloned().unwrap_or(Value::Null));
    reply.insert("changes".into(), json!(described.changes));
    let more = described.more + answer.get("more").and_then(Value::as_u64).unwrap_or(0) as usize;
    if more > 0 {
        reply.insert("more".into(), more.into());
    }
    match draft(answer, words) {
        Some(steps) => {
            reply.insert("recipeSteps".into(), steps);
        }
        None if !described.changes.is_empty() => {
            reply.insert("note".into(), "No change kind makes these again: describe them in the recipe's about, or leave them out.".into());
        }
        None => {}
    }
    if described.changes.is_empty() {
        let place = words.words.project;
        reply.insert("note".into(), format!("Nothing in the {place} changed while Kumi watched.").into());
    }
    reply
}

/// watch_me in a DAW whose bridge is Kumi's own: the host keeps the project as it was at start and compares at stop.
pub struct WatchTool {
    pub endpoint: Rc<RefCell<Option<Rc<dyn McpEndpoint>>>>,
    pub words: &'static CatchUpWords,
    pub app: &'static str,
    pub on_watch: Option<Rc<dyn Fn(bool)>>,
}

impl WatchTool {
    fn watching_now(&self, on: bool) {
        if let Some(listener) = &self.on_watch {
            let _ = catch_unwind(AssertUnwindSafe(|| listener(on)));
        }
    }
}

#[async_trait(?Send)]
impl KernelTool for WatchTool {
    fn name(&self) -> &str {
        "watch_me"
    }
    fn description(&self) -> &str {
        self.words.watch_description
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{"action":{"type":"string","enum":["start","stop"]}}})
            .as_object()
            .cloned()
            .unwrap_or_default()
    }
    fn reach(&self) -> Reach {
        Reach::DawRead
    }
    fn deferred(&self) -> bool {
        true
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        let Some(endpoint) = self.endpoint.borrow().clone() else {
            return Ok(ToolResult::error(format!("{} isn't connected, so Kumi can't watch it.", self.app)));
        };
        let start = input.get("action").and_then(Value::as_str) == Some("start");
        let bounded = abort::any([signal.clone(), abort::timeout(CALL_MS * 2)]);
        let tool = if start { "watch_start" } else { "watch_stop" };
        let answer =
            match stopped_or(endpoint.call(tool, JsonObject::new(), bounded.clone()).await.map(|result| answer_of(&result)), &bounded) {
                Ok(answer) => answer,
                Err(error) if error.is_aborted() && signal.is_cancelled() => return Err(error),
                Err(_) => {
                    return Ok(ToolResult::error(format!("Kumi couldn't compare the {} just now; try again.", self.words.words.project)))
                }
            };
        let answer = match answer {
            Ok(answer) => answer,
            Err(refused) => {
                let place = self.words.words.project;
                // (busy at stop: the host goes on watching, to stop once the DAW answers)
                if refused.busy && !start {
                    let app = self.app;
                    return Ok(ToolResult::error(format!(
                        "{app} is busy just now ({}): Kumi is still watching; stop again once {app} answers.",
                        refused.why
                    )));
                }
                if !start {
                    self.watching_now(false);
                }
                let text = match refused.code.as_deref() {
                    Some("not-watching") => "Kumi isn't watching yet: start first, before the producer does it.".to_string(),
                    Some("other-project") => format!("A different {place} is open now, so there's nothing to compare; start again."),
                    Some("unsupported") => {
                        format!("This {} bridge can't compare the {place}'s states, so Kumi can't learn routines by watching.", self.app)
                    }
                    _ => format!("Kumi couldn't compare the {place} just now ({}); try again.", refused.why),
                };
                return Ok(ToolResult::error(text));
            }
        };
        if start {
            self.watching_now(true);
            let app = self.app;
            return Ok(ToolResult::text(stringify(
                &json!({"watching": true, "note": format!("Tell the producer to go ahead in {app} and to say when they're done.")}),
            )));
        }
        self.watching_now(false);
        Ok(ToolResult::text(stringify(&Value::Object(watched(&answer, self.words)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORDS: CatchUpWords = CatchUpWords {
        words: ProjectWords {
            kinds: &[("track", "tracks"), ("FX", "FX"), ("item", "items")],
            project: "project",
            rank: &["project", "track", "FX", "item"],
            opaque: &["FX"],
        },
        facts: |details| {
            details
                .iter()
                .filter(|d| d["what"] == "tempo")
                .map(|d| format!("Tempo {} → {} BPM", stringify(&d["from"]), stringify(&d["to"])))
                .collect()
        },
        recipe: RecipeKinds {
            track_add: "track_add",
            track_types: &[],
            effect: "FX",
            effect_add: |made, name| {
                let plugin = made.get("plugin").and_then(Value::as_str).unwrap_or(name);
                Some(("fx_add", json!({"name": plugin}).as_object().cloned().unwrap()))
            },
            effect_values: |values| {
                let values: Vec<Value> = values.iter().map(|(name, value)| json!({"param": name, "normalized": value})).collect();
                ("fx_params", json!({"values": values}).as_object().cloned().unwrap())
            },
            mixer: &[
                Mixer { what: "mixer.volume", tool: "track_volume", field: "volumeDb", value: as_read, with: &[] },
                Mixer { what: "mixer.pan", tool: "track_pan", field: "pan", value: as_read, with: &[] },
            ],
        },
        watch_description: "",
    };

    #[test]
    fn catch_up_says_the_projects_values_first_then_each_kind_in_the_daws_words() {
        let answer = json!({
            "project": [{"facet": "tempo", "what": "tempo", "from": 120.0, "to": 124.0}],
            "changes": [
                {"change": "changed", "kind": "track", "before": {"name": "Bass", "order": 2}, "after": {"name": "Bass", "order": 2}, "renamed": false, "details": [{"facet": "mixer", "what": "mixer.volume", "from": null, "to": null}]},
                {"change": "added", "kind": "FX", "item": {"name": "ReaComp", "order": 0, "on": "Bass"}},
                {"change": "added", "kind": "FX", "item": {"name": "ReaEQ", "order": 1, "on": "Bass"}},
                {"change": "removed", "kind": "item", "item": {"name": "Kick", "order": 3}},
            ],
            "more": 2,
        });
        let said = caught_up(&answer, &WORDS).unwrap();
        assert_eq!(
            said.lines,
            ["Tempo 120 → 124 BPM", "Changed track “Bass” (mixer)", "Added FX “ReaComp”, “ReaEQ”", "Removed item “Kick”"]
        );
        assert_eq!(said.more, 2, "what the host didn't list is counted");
        assert_eq!(caught_up(&json!({"first": true}), &WORDS), None, "a project first seen here: nothing to tell");
        assert_eq!(caught_up(&json!({"same": true}), &WORDS), Some(DescribedDiff { lines: vec![], more: 0 }));
    }

    #[test]
    fn a_watched_routine_is_drafted_as_the_daws_steps_with_blanks_for_the_tracks_worked_on() {
        let answer = json!({
            "watched": "40 s",
            "changes": [
                {"change": "added", "kind": "track", "item": {"name": "Vox Bus", "order": 5}, "madeOf": {"name": "Vox Bus", "mixer": {"volume": -3.0, "pan": 0.0}}},
                {"change": "changed", "kind": "track", "before": {"name": "Lead Vox", "order": 2}, "after": {"name": "Lead Vox", "order": 2}, "details": [{"facet": "mixer", "what": "mixer.volume", "from": 0.0, "to": -6.0}, {"facet": "sends", "what": "sends.sendsHash", "from": "a", "to": "b", "fingerprint": true}]},
                {"change": "added", "kind": "FX", "item": {"name": "My Comp", "order": 0, "on": "Lead Vox"}, "madeOf": {"plugin": "VST: ReaComp (Cockos)", "parameters": [{"name": "Threshold", "value": {"value": 0.3, "shows": "-18 dB"}}]}},
                {"change": "changed", "kind": "FX", "after": {"name": "ReaEQ", "order": 0, "on": "Drums"}, "details": [{"facet": "parameters", "what": "parameters.Gain", "from": 0.5, "to": {"value": 0.6, "shows": "+2 dB"}}]},
                {"change": "added", "kind": "item", "item": {"name": "take 1", "order": 0, "on": "Lead Vox"}},
            ],
            "selected": [{"name": "Lead Vox", "key": "{A}"}],
        });
        let draft = draft(&answer, &WORDS).unwrap();
        assert_eq!(
            draft["steps"],
            json!([
                {"tool": "track_add", "input": {"name": "Vox Bus"}, "as": "t1"},
                {"tool": "track_volume", "input": {"ref": "@t1", "volumeDb": -3.0}},
                {"tool": "track_volume", "input": {"ref": "$track", "volumeDb": -6.0}},
                {"tool": "fx_add", "input": {"name": "VST: ReaComp (Cockos)", "ref": "$track"}, "as": "fx1"},
                {"tool": "fx_params", "input": {"values": [{"param": "Threshold", "normalized": 0.3}], "ref": "@fx1"}},
                {"tool": "fx_params", "input": {"values": [{"param": "Gain", "normalized": 0.6}], "ref": "$reaeq"}},
            ])
        );
        assert_eq!(draft["params"][0], json!({"name": "track", "about": "the track to work on (it was “Lead Vox”)"}));
        assert_eq!(draft["params"][1], json!({"name": "reaeq", "about": "the FX to set (it was “ReaEQ” on “Drums”)"}));
        assert_eq!(draft["leftOut"], json!(["item “take 1” added"]));
        // An added effect whose moved parameters the host didn't name (past its limit) is drafted as it comes, and said.
        let mut many = answer.clone();
        many["changes"][2]["madeOf"] = json!({"plugin": "VST: ReaComp (Cockos)", "parametersMoved": 14});
        let drafted = super::draft(&many, &WORDS).unwrap();
        assert_eq!(drafted["leftOut"], json!(["FX “My Comp”: 14 parameters moved, not drafted", "item “take 1” added"]));
        let reply = watched(&answer, &WORDS);
        assert_eq!(reply["watched"], "40 s");
        assert!(reply["changes"].as_array().unwrap().iter().any(|c| c["added"] == "FX" && c["on"] == "Lead Vox"), "{reply:?}");
        // The draft's blanks are the recipe's own names: what save_recipe takes.
        let blank = regex::Regex::new(r"^[a-z][a-z0-9_]{0,31}$").unwrap();
        for param in draft["params"].as_array().unwrap() {
            assert!(blank.is_match(param["name"].as_str().unwrap()), "{param}");
        }
    }

    /// A host that answers catch_up from a list, in turn.
    struct Answers(RefCell<Vec<Value>>);
    #[async_trait(?Send)]
    impl McpEndpoint for Answers {
        fn pid(&self) -> Option<u32> {
            None
        }
        fn server_info(&self) -> Option<crate::mcp::types::Implementation> {
            None
        }
        async fn list(&self, _: Option<&str>, _: Signal) -> Result<crate::mcp::types::ListToolsResult, RuntimeError> {
            Ok(serde_json::from_value(json!({"tools": []})).unwrap())
        }
        async fn call(&self, _: &str, _: JsonObject, _: Signal) -> Result<crate::mcp::types::CallToolResult, RuntimeError> {
            let answer = self.0.borrow_mut().remove(0);
            Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": answer.to_string()}], "isError": false})).unwrap())
        }
        fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
            Box::new(|| {})
        }
        fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
            Box::new(|| {})
        }
        fn stderr_status(&self) -> crate::mcp::client::StderrStatus {
            crate::mcp::client::StderrStatus { bytes: 0, truncated: false }
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_project_seen_again_unchanged_is_not_told_again() {
        let host: Rc<dyn McpEndpoint> = Rc::new(Answers(RefCell::new(vec![
            json!({"same": true, "lastSeenAt": 1}),
            json!({"first": true}),
            json!({"same": true, "lastSeenAt": 1}),
            json!({"project": [{"what": "tempo", "from": 120, "to": 121}], "lastSeenAt": 1}),
        ])));
        let told: Rc<RefCell<Vec<CatchUp>>> = Rc::default();
        let into = told.clone();
        let on: Option<Rc<dyn Fn(CatchUp)>> = Some(Rc::new(move |c| into.borrow_mut().push(c)));
        let remembered = Rc::new(Remembered::default());
        for identity in ["A", "B", "A", "A"] {
            remembered.clone().catch_up(host.clone(), identity.into(), "Song".into(), &WORDS, on.clone(), Signal::new()).await;
        }
        // A first: nothing changed, said as Live says it at a start; B new; A again unchanged: quiet; A changed: told.
        let lines: Vec<Vec<String>> = told.borrow().iter().map(|c| c.lines.clone()).collect();
        assert_eq!(lines, [vec![], vec!["Tempo 120 → 121 BPM".to_string()]]);
        assert!(remembered.since("A").is_some() && remembered.since("B").is_none());
    }

    /// What the host says of an FX whose parameters moved: the diff's details as watch_stop sends them, with the one
    /// moved parameter it names (as its `name_moves` does) when `named`.
    fn host_said(named: bool) -> Value {
        use daw_mcp_server::semantic::{diff, Change, Detail, Snapshot};
        let snapshot = |hash: &str, ratio: f64| {
            Snapshot {
            records: vec![
                serde_json::from_value(json!({"k": "track", "id": "{B}", "n": "Bass", "o": 1})).unwrap(),
                serde_json::from_value(json!({"k": "FX", "id": "{F}", "n": "ReaComp", "o": 0, "p": "{B}", "f": {"parameters": {"parametersHash": hash}}, "v": [0.5, ratio]})).unwrap(),
            ],
            ..Default::default()
        }
        };
        let mut said = diff(&snapshot("77aa", 0.1), &snapshot("77ab", 0.4), true, &Default::default());
        if named {
            if let Change::Changed { details, .. } = &mut said.changes[0] {
                details.push(Detail {
                    facet: "parameters".into(),
                    what: "parameters.Ratio".into(),
                    from: json!(0.1),
                    to: json!({"value": 0.4, "shows": "0.4"}),
                    fingerprint: false,
                });
            }
        }
        json!({"changes": said.changes})
    }

    // Review K7: the parameters' fingerprint is never drafted as a parameter to set.
    #[test]
    fn a_tweaked_effects_fingerprint_is_never_drafted_as_one_of_its_parameters() {
        let answer = host_said(true);
        assert_eq!(answer["changes"][0]["details"][0]["what"], "parameters.parametersHash", "{answer}");
        let drafted = draft(&answer, &WORDS).unwrap();
        assert_eq!(
            drafted["steps"],
            json!([{"tool": "fx_params", "input": {"values": [{"param": "Ratio", "normalized": 0.4}], "ref": "$reacomp"}}])
        );
        // Its names not read (past what the host reads): nothing to set, and it's left out.
        let answer = host_said(false);
        assert!(draft(&answer, &WORDS).is_none(), "{answer}");
    }

    // Review K8: what the producer put on a track they added is drafted on that track, with the parameters they moved;
    // Renoise's send track is made as one.

    #[test]
    fn with_no_track_selected_the_first_one_worked_on_is_the_track() {
        let answer = json!({
            "changes": [
                {"change": "changed", "kind": "track", "after": {"name": "Bass", "order": 2}, "details": [{"facet": "mixer", "what": "mixer.pan", "from": 0.0, "to": 0.5}]},
                {"change": "changed", "kind": "track", "after": {"name": "Keys", "order": 3}, "details": [{"facet": "mixer", "what": "mixer.pan", "from": 0.0, "to": -0.5}]},
            ],
            // (selected, but the routine didn't touch it)
            "selected": [{"name": "Drums"}],
        });
        let draft = draft(&answer, &WORDS).unwrap();
        assert_eq!(draft["steps"][0]["input"]["ref"], "$track");
        assert_eq!(draft["steps"][1]["input"]["ref"], "$keys");
    }

    /// A host that answers catch_up and baseline_save as the test says, and remembers what it was asked.
    struct Told {
        catch_up: RefCell<Value>,
        save: RefCell<Value>,
        asked: RefCell<Vec<String>>,
        /// baseline_save answers its text as an error.
        save_fails: Cell<bool>,
    }
    #[async_trait(?Send)]
    impl McpEndpoint for Told {
        fn pid(&self) -> Option<u32> {
            None
        }
        fn server_info(&self) -> Option<crate::mcp::types::Implementation> {
            None
        }
        async fn list(&self, _: Option<&str>, _: Signal) -> Result<crate::mcp::types::ListToolsResult, RuntimeError> {
            Ok(serde_json::from_value(json!({"tools": []})).unwrap())
        }
        async fn call(&self, name: &str, _: JsonObject, _: Signal) -> Result<crate::mcp::types::CallToolResult, RuntimeError> {
            self.asked.borrow_mut().push(name.into());
            let answer = if name == "catch_up" { self.catch_up.borrow().clone() } else { self.save.borrow().clone() };
            let error = name == "baseline_save" && self.save_fails.get();
            Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": answer.to_string()}], "isError": error})).unwrap())
        }
        fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
            Box::new(|| {})
        }
        fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
            Box::new(|| {})
        }
        fn stderr_status(&self) -> crate::mcp::client::StderrStatus {
            crate::mcp::client::StderrStatus { bytes: 0, truncated: false }
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    fn answers(catch_up: Value, save: Value) -> Rc<Told> {
        Rc::new(Told {
            catch_up: RefCell::new(catch_up),
            save: RefCell::new(save),
            asked: RefCell::default(),
            save_fails: Cell::new(false),
        })
    }

    const REFUSED: &str = r#"{"saved": false, "compared": false, "why": "not compared yet: catch_up first"}"#;

    /// The keeper's loop for a minute: each second, the snapshot is kept when it's due. How many times it was asked.
    async fn keep_for_a_minute(remembered: &Rc<Remembered>, host: &Rc<Told>) -> usize {
        let before = host.asked.borrow().len();
        for _ in 0..60 {
            tokio::time::advance(Duration::from_secs(1)).await;
            if remembered.due() {
                remembered.clone().save(host.clone(), Signal::new()).await;
            }
        }
        host.asked.borrow()[before..].iter().filter(|name| *name == "baseline_save").count()
    }

    /// (#361 review K15) A save the host refuses (another tab, not compared yet) isn't asked again each second, each a
    /// whole project's digest: the next look's catch-up compares it, and then it's kept again.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_refused_save_waits_for_the_next_catch_up() {
        let host = answers(json!({"first": true}), serde_json::from_str(REFUSED).unwrap());
        let remembered = Rc::new(Remembered::default());
        remembered.clone().catch_up(host.clone(), "A".into(), "Song".into(), &WORDS, None, Signal::new()).await;
        assert!(!remembered.should_catch_up("A"));
        remembered.kumi_changed();
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 1, "{:?}", host.asked.borrow());
        assert!(remembered.should_catch_up("B"), "the project open now is compared at the next look");
        remembered.clone().catch_up(host.clone(), "B".into(), "Other".into(), &WORDS, None, Signal::new()).await;
        *host.save.borrow_mut() = json!({"saved": true, "ms": 20});
        remembered.kumi_changed();
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 1);
        assert!(!remembered.due(), "kept: nothing changed since");
    }

    /// A save the host answers with an error (its snapshots' folder is full, the digest ran out of time) isn't asked again
    /// each second, each a whole project's digest: each try waits twice as long as the last, until one is kept.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_failed_save_is_asked_again_later_each_time_not_every_second() {
        let host = answers(json!({"first": true}), json!("the disk is full"));
        host.save_fails.set(true);
        let remembered = Rc::new(Remembered::default());
        remembered.clone().catch_up(host.clone(), "A".into(), "Song".into(), &WORDS, None, Signal::new()).await;
        remembered.kumi_changed();
        // (after 2 s, 10 s later, then 20 s later: the next is 40 s on)
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 3, "{:?}", host.asked.borrow());
        host.save_fails.set(false);
        *host.save.borrow_mut() = json!({"saved": true, "ms": 20});
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 1, "kept once the host can");
        remembered.kumi_changed();
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 1, "and back to its usual pace");
    }

    /// (#361 review K15) Kumi's host started again compares the project before it keeps a snapshot: the next look's
    /// catch-up, which tells nothing (the session went on), and the snapshots are kept again after it.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_restarted_host_is_caught_up_quietly_before_snapshots_are_kept_again() {
        let host = answers(json!({"first": true}), serde_json::from_str(REFUSED).unwrap());
        let remembered = Rc::new(Remembered::default());
        let told = Rc::new(RefCell::new(vec![]));
        let tell: Rc<dyn Fn(CatchUp)> = {
            let told = told.clone();
            Rc::new(move |caught: CatchUp| told.borrow_mut().push(caught.lines))
        };
        remembered.clone().catch_up(host.clone(), "A".into(), "Song".into(), &WORDS, Some(tell.clone()), Signal::new()).await;
        remembered.host_restarted();
        assert!(remembered.should_catch_up("A"));
        remembered.kumi_changed();
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 0, "nothing kept before the host compares");
        *host.catch_up.borrow_mut() = json!({"changes": [{"change": "added", "kind": "FX", "item": {"name": "ReaComp", "order": 0}}]});
        remembered.clone().catch_up(host.clone(), "A".into(), "Song".into(), &WORDS, Some(tell.clone()), Signal::new()).await;
        assert!(told.borrow().is_empty(), "Kumi's own change isn't news: {:?}", told.borrow());
        assert_eq!(remembered.since("A"), None);
        *host.save.borrow_mut() = json!({"saved": true, "ms": 20});
        remembered.kumi_changed();
        assert_eq!(keep_for_a_minute(&remembered, &host).await, 1);
        // The DAW went away too: what changed meanwhile is told.
        remembered.went_away();
        remembered.host_restarted();
        remembered.clone().catch_up(host.clone(), "A".into(), "Song".into(), &WORDS, Some(tell), Signal::new()).await;
        assert_eq!(*told.borrow(), [vec!["Added FX “ReaComp”".to_string()]]);
    }

    #[test]
    fn blanks_from_names_are_valid_and_never_twice() {
        assert_eq!(blank_name("Lead Vox", &[]), "lead_vox");
        assert_eq!(blank_name("808 Sub!", &[]), "t_808_sub");
        assert_eq!(blank_name("Lead Vox", &["lead_vox".into()]), "lead_vox2");
        assert_eq!(blank_name("", &[]), "t");
        let long = blank_name(&"x".repeat(80), &[]);
        assert!(long.len() <= 32, "{long}");
    }
}
