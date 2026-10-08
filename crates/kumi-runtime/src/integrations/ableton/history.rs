//! Live change records, guarded undo, grouped HISTORY, and quiet audition steps.
use super::{
    changes::{next_change_id, undo_note, EMERGENCY_STOP},
    connection::LiveConnection,
    context,
    fast::revert_script,
    observation::ObservedChange,
    references::{Moved, Shift},
    remember::Remember,
    snapshots::{self, Material},
    views::ViewHost,
};
use crate::{
    core::{contracts::*, errors::RuntimeError},
    mcp::types::{CallToolResult, ContentBlock},
    notation::thousands,
};
use futures::{future::LocalBoxFuture, FutureExt};
use indexmap::{IndexMap, IndexSet};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, number::to_string, string::head},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

/// The most each call to Live of Kumi's undo of a cut waits.
const RESTORE_MS: u64 = 30_000;
/// The most changes a session's history keeps, the oldest going first.
pub const MAX_ENTRIES: usize = 20_000;
/// How Kumi's Live script says a kept audio clip's file is gone, before its path (snapshots.py).
const MISSING_FILE: &str = "audio file isn't there any more (";

#[derive(Clone, Serialize, Deserialize)]
pub struct Restore {
    #[serde(rename = "ref")]
    pub reference: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub record: ChangeRecord,
    pub transaction_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<Restore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert: Option<Vec<Value>>,
    /// What the change cut or deleted, which Kumi's undo makes again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<Material>,
    /// A group's first steps that were past the most changes Kumi keeps when it was recorded: only Live's own undo
    /// takes them back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trimmed: Option<usize>,
    /// Where the change moved the Set's tracks and scenes, which the refs that followed it go back from once it's
    /// undone. Not kept past this Live connection: its refs aren't either.
    #[serde(skip)]
    pub shift: Option<Shift>,
    /// Live's identity for the device the change made (a load or a duplicate), when the bridge said: what may be
    /// removed in its place if Live won't undo it. Not kept past this Live connection: identities aren't either.
    #[serde(skip)]
    pub created: Option<String>,
    /// The tool that made the change: whether it can change what's heard (a rename, a scale, the transport or an empty
    /// track can't). Not kept past this Kumi: a change read back from disk goes by its family.
    #[serde(skip)]
    pub tool: Option<String>,
}
impl Applied {
    pub fn new(record: ChangeRecord, transaction_id: String, restore: Option<Restore>) -> Self {
        let permanent = (record.state == ChangeState::Kept).then_some(true);
        Self {
            record,
            transaction_id,
            restore,
            permanent,
            undo_key: None,
            members: None,
            within: None,
            revert: None,
            material: None,
            trimmed: None,
            shift: None,
            created: None,
            tool: None,
        }
    }
    /// Whether the change can change what's heard: not one that only names, colours or marks things, sets the
    /// scale or the transport, or adds empty tracks and scenes (or captures one).
    pub fn audible(&self) -> bool {
        const SILENT: [&str; 8] = [
            "rename",
            "set_track_color",
            "set_locators",
            "delete_locator",
            "set_scale",
            "set_transport",
            "add_tracks_and_scenes",
            "capture_scene",
        ];
        match &self.tool {
            Some(tool) => !SILENT.contains(&tool.as_str()),
            None => !matches!(self.record.family, ChangeFamily::Rename | ChangeFamily::Color | ChangeFamily::Locators),
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<ChangeRecord>,
    pub text: String,
    pub is_error: bool,
    /// Kumi's refs were retired with it (a restructure's undo Live didn't confirm). The model is told so with the
    /// undo (`for_model`); the producer reads `text` as it is (a render's cleanup notice, /undo).
    #[serde(skip)]
    pub retired: bool,
}
impl UndoResult {
    fn error(text: impl Into<String>) -> Self {
        Self { record: None, text: text.into(), is_error: true, retired: false }
    }
    fn with(record: ChangeRecord, text: impl Into<String>, is_error: bool) -> Self {
        Self { record: Some(record), text: text.into(), is_error, retired: false }
    }
    /// The undo's text as the model reads it: with what to do about Kumi's refs when they were retired with it.
    pub fn for_model(&self) -> String {
        if !self.retired {
            return self.text.clone();
        }
        let text = self.text.trim_end();
        let stop = if text.ends_with(['.', '!', '?']) { "" } else { "." };
        format!("{text}{stop} {REFS_RETIRED}")
    }
}
/// Said to the model after a restructure's undo Live didn't confirm.
const REFS_RETIRED: &str =
    "Live may have taken the tracks or scenes back, so Kumi's references are retired: discover again before using any.";
/// The Remote Script's python.run error type for a failure after the code ran (its result, the deadline after it,
/// its undo step): Live may have changed.
pub const PYTHON_RAN: &str = "RanResultUnavailable";
pub enum FastResult {
    Result(Value),
    Error { error: String, sent: bool },
}
pub struct History {
    pub connection: Rc<LiveConnection>,
    pub remember: Rc<Remember>,
    pub entries: RefCell<IndexMap<String, Rc<RefCell<Applied>>>>,
    pub changes_this_turn: Cell<usize>,
    quiet: RefCell<Option<Vec<String>>>,
    timeout_ms: u64,
    on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
    /// What else follows a restructure Kumi's undo takes back (what the turns showed of the Set's devices).
    pub on_shift: RefCell<Option<OnShift>>,
}
/// Told where a restructure Kumi's undo took back moved the Set's tracks and scenes, or None when it can't be known.
pub type OnShift = Rc<dyn Fn(Option<&Shift>)>;
impl History {
    pub fn change_signal(&self) -> Signal {
        abort::any([self.connection.lifetime.clone(), abort::timeout(self.timeout_ms)])
    }
    /// A change's time in Live, sized to the change: a change of many tracks, scenes, pads or clips takes Live a while
    /// for each, and each costs more in a big Set (64 tracks took 28 s, and were called unconfirmed at a flat 30 s,
    /// #260). The bridge bounds each of Live's steps itself, so this only needs to outlast them; at most ten minutes.
    pub fn change_signal_for(&self, items: usize) -> Signal {
        let more = items.saturating_sub(8) as u64 * (self.timeout_ms / 15);
        abort::any([self.connection.lifetime.clone(), abort::timeout((self.timeout_ms + more).min(600_000.max(self.timeout_ms)))])
    }
    pub fn new(
        connection: Rc<LiveConnection>,
        remember: Rc<Remember>,
        timeout_ms: Option<u64>,
        on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
    ) -> Self {
        Self {
            connection,
            remember,
            timeout_ms: timeout_ms.unwrap_or(30_000),
            on_change,
            entries: RefCell::new(IndexMap::new()),
            changes_this_turn: Cell::new(0),
            quiet: RefCell::new(None),
            on_shift: RefCell::new(None),
        }
    }
    /// A restructure this change made, which its undo moves the refs that followed it back from.
    pub fn restructured(&self, id: &str, shift: &Shift) {
        if let Some(entry) = self.entries.borrow().get(id) {
            entry.borrow_mut().shift = Some(shift.clone());
        }
    }
    /// A restructure undone: what followed it goes back, the book's refs (which Live reads again at their places before
    /// Python uses them), HISTORY's, and what the turns showed.
    fn unshift(&self, shift: &Shift) {
        let back = shift.inverse();
        let moved = self.shifted(&back);
        let mut book = self.connection.references.borrow_mut();
        book.shift(&back);
        book.mark_moved(moved);
        drop(book);
        let on_shift = self.on_shift.borrow().clone();
        if let Some(on_shift) = on_shift {
            on_shift(Some(&back));
        }
    }
    /// A restructure's undo Live didn't confirm: whether the tracks and scenes moved back can't be known, so the refs
    /// that followed it are retired, as after a restructure Kumi can't read exactly, and what the turns showed with
    /// them.
    fn unknown_shift(&self) {
        let mut book = self.connection.references.borrow_mut();
        book.refs.clear();
        book.known.clear();
        book.cursors.clear();
        book.clear_names();
        drop(book);
        let on_shift = self.on_shift.borrow().clone();
        if let Some(on_shift) = on_shift {
            on_shift(None);
        }
    }
    pub fn observed(&self) -> Vec<ObservedChange> {
        self.entries
            .borrow()
            .values()
            .map(|entry| {
                let entry = entry.borrow();
                ObservedChange { record: entry.record.clone(), within: entry.within.as_ref().is_some_and(|s| !s.is_empty()) }
            })
            .collect()
    }
    pub fn emit(&self, record: &ChangeRecord) {
        if self.quiet.borrow().is_some() {
            return;
        }
        if let Some(listener) = &self.on_change {
            let _ = catch_unwind(AssertUnwindSafe(|| listener(record.clone())));
        }
    }
    pub fn is_quiet(&self) -> bool {
        self.quiet.borrow().is_some()
    }
    /// At most `most` changes are kept, the oldest going first, however they came in (quiet ones, groups). The
    /// history keeps MAX_ENTRIES.
    fn keep_within(&self, most: usize) {
        let mut entries = self.entries.borrow_mut();
        let over = entries.len().saturating_sub(most);
        if over > 0 {
            entries.drain(..over);
        }
    }
    pub fn remember(&self, record: ChangeRecord, transaction_id: String, restore: Option<Restore>) {
        self.entries.borrow_mut().insert(record.id.clone(), Rc::new(RefCell::new(Applied::new(record.clone(), transaction_id, restore))));
        self.keep_within(MAX_ENTRIES);
        if let Some(quiet) = self.quiet.borrow_mut().as_mut() {
            quiet.push(record.id.clone());
            return;
        }
        self.emit(&record);
        self.remember.schedule_save(20_000);
    }
    pub fn retire(&self, note: &str) {
        let entries: Vec<_> = self.entries.borrow().values().cloned().collect();
        for entry in entries {
            let record = {
                let mut entry = entry.borrow_mut();
                if !matches!(entry.record.state, ChangeState::Applied | ChangeState::Unsure) {
                    continue;
                }
                entry.record.state = ChangeState::Expired;
                entry.record.note = Some(note.into());
                entry.record.clone()
            };
            self.emit(&record);
        }
    }
    fn update(&self, entry: &Rc<RefCell<Applied>>, state: ChangeState, note: Option<String>) -> ChangeRecord {
        let record = {
            let mut entry = entry.borrow_mut();
            entry.record.state = state;
            entry.record.note = note;
            entry.record.clone()
        };
        self.emit(&record);
        record
    }
    pub async fn quietly<T>(&self, into: Option<&mut Vec<String>>, work: impl Future<Output = T>) -> T {
        struct Guard<'a> {
            history: &'a History,
            outer: Option<Vec<String>>,
            counted: usize,
            into: Option<&'a mut Vec<String>>,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                let quiet = self.history.quiet.borrow_mut().take().unwrap_or_default();
                if let Some(into) = self.into.as_mut() {
                    into.extend(quiet);
                } else {
                    let released: Vec<_> = quiet
                        .iter()
                        .filter_map(|id| self.history.entries.borrow_mut().shift_remove(id))
                        .filter_map(|entry| {
                            let entry = entry.borrow();
                            (entry.record.state == ChangeState::Applied).then(|| entry.transaction_id.clone())
                        })
                        .collect();
                    self.history.release(&released);
                }
                *self.history.quiet.borrow_mut() = self.outer.take();
                self.history.changes_this_turn.set(self.counted);
            }
        }
        let _guard = Guard { history: self, outer: self.quiet.replace(Some(Vec::new())), counted: self.changes_this_turn.get(), into };
        work.await
    }
    pub fn release(&self, given: &[String]) {
        let ids: Vec<_> = given.iter().filter(|s| !s.is_empty()).cloned().collect();
        if ids.is_empty() || !self.connection.has("live_transaction_release") {
            return;
        }
        for chunk in ids.chunks(64) {
            let connection = self.connection.clone();
            let args = object(json!({"transactionIds":chunk}));
            let signal = abort::any([connection.lifetime.clone(), abort::timeout(10_000)]);
            start_background(async move {
                let _ = connection.call("live_transaction_release", args, signal).await;
            });
        }
    }
    pub fn grouped(&self, title: &str, ids: &[String], apart: &[String]) -> Option<String> {
        let standing = |id: &String| {
            !apart.contains(id)
                && self.entries.borrow().get(id).is_some_and(|entry| {
                    matches!(entry.borrow().record.state, ChangeState::Applied | ChangeState::Unsure | ChangeState::Kept)
                })
        };
        // The build's first steps that are already past the most changes Kumi keeps (the oldest go first).
        let mut trimmed = ids.iter().filter(|id| !apart.contains(id)).take_while(|id| !self.entries.borrow().contains_key(*id)).count();
        let members: Vec<_> = ids.iter().filter(|id| standing(id)).cloned().collect();
        let gone: Vec<_> = ids.iter().filter(|id| !standing(id)).collect();
        let released: Vec<_> = gone
            .iter()
            .filter_map(|id| self.entries.borrow().get(*id).cloned())
            .filter_map(|entry| {
                let entry = entry.borrow();
                (entry.record.state == ChangeState::Applied).then(|| entry.transaction_id.clone())
            })
            .collect();
        self.release(&released);
        for id in gone {
            self.entries.borrow_mut().shift_remove(id);
        }
        // Room for the group's own record before its members are chosen, so keeping to the cap can't drop one of them.
        self.keep_within(MAX_ENTRIES - 1);
        let count = members.len();
        let members: Vec<_> = members.into_iter().filter(|id| self.entries.borrow().contains_key(id)).collect();
        trimmed += count - members.len();
        if members.is_empty() {
            return None;
        }
        let record:ChangeRecord=serde_json::from_value(json!({"id":next_change_id(),"family":"clip","title":head(title,160),"state":"applied","at":self.connection.now().timestamp_millis()})).unwrap();
        for id in &members {
            self.entries.borrow()[id].borrow_mut().within = Some(record.id.clone());
        }
        let mut entry = Applied::new(record.clone(), String::new(), None);
        entry.members = Some(members);
        entry.trimmed = (trimmed > 0).then_some(trimmed);
        self.entries.borrow_mut().insert(record.id.clone(), Rc::new(RefCell::new(entry)));
        self.emit(&record);
        self.remember.schedule_save(20_000);
        Some(record.id)
    }
    /// After a restructure the refs Kumi's undo names follow what they named, as the book's do, so an undo puts back
    /// the parameter it set, not what moved into its place; one on what was deleted names nothing, and its undo says
    /// so. Returns the refs moved, which Live reads again before Python uses them.
    pub fn shifted(&self, shift: &Shift) -> Vec<String> {
        let mut moved = Vec::new();
        for entry in self.entries.borrow().values() {
            let mut entry = entry.borrow_mut();
            for target in entry.revert.iter_mut().flatten() {
                for key in ["ref", "device"] {
                    let Some(reference) = target.get(key).and_then(Value::as_str) else { continue };
                    match shift.reference(reference) {
                        Moved::Same => {}
                        Moved::To(now) => {
                            target[key] = json!(now);
                            moved.push(now);
                        }
                        Moved::Gone => target[key] = json!("gone"),
                    }
                }
            }
            if let Some(restore) = entry.restore.as_mut() {
                if let Moved::To(now) = shift.reference(&restore.reference) {
                    restore.reference = now;
                }
            }
            // What an earlier restructure made is where this one moved it, for that one's undo to delete. (Kumi's
            // undo makes nothing deleted again, so only what was made needs following.)
            if let Some(earlier) = entry.shift.as_mut() {
                earlier.tracks_made = earlier.tracks_made.iter().filter_map(|index| shift.track(*index)).collect();
                earlier.scenes_made = earlier.scenes_made.iter().filter_map(|index| shift.scene(*index)).collect();
            }
        }
        moved
    }
    pub async fn run_fast(&self, code: String, signal: Signal) -> Result<FastResult, RuntimeError> {
        let called =
            match self.connection.call("live_run_python", object(json!({"code":code,"mode":"exec","timeoutMs":10000})), signal).await {
                Ok(v) => v,
                Err(_) => return Ok(FastResult::Error { error: "Live didn't answer".into(), sent: true }),
            };
        if called.is_error == Some(true) {
            return Ok(FastResult::Error { error: head(&result_text(&called), 600), sent: uncertain(&called) });
        }
        let body = match context::payload(&called) {
            Ok(v) => v,
            Err(_) => return Ok(FastResult::Error { error: "Kumi couldn't read Live's answer".into(), sent: true }),
        };
        if body.get("ok") != Some(&Value::Bool(true)) {
            let error = context::object(body.get("error").filter(|v| !v.is_null()).unwrap_or(&json!({})))?;
            let message =
                error.get("message").filter(|v| !v.is_null()).map(|v| js_string(Some(v))).unwrap_or_else(|| "Live refused it".into());
            // The code ran and only its answer failed (or its undo step): Live may have changed.
            let sent = error.get("type").and_then(Value::as_str) == Some(PYTHON_RAN);
            return Ok(FastResult::Error { error: head(&message, 600), sent });
        }
        Ok(FastResult::Result(body.get("result").cloned().unwrap_or(Value::Null)))
    }
    pub fn undo<'a>(&'a self, target: &'a str, signal: Signal, discard: bool) -> LocalBoxFuture<'a, Result<UndoResult, RuntimeError>> {
        async move {
            // The latest change, confirmed or not: one Live didn't confirm (64 tracks added past the time limit) is the
            // one the producer means, and undoing the one before it instead took back the wrong change (#260).
            let entry = if target == "last" {
                self.entries
                    .borrow()
                    .values()
                    .rev()
                    .find(|entry| {
                        let entry = entry.borrow();
                        matches!(entry.record.state, ChangeState::Applied | ChangeState::Unsure)
                            && entry.within.as_ref().is_none_or(|s| s.is_empty())
                    })
                    .cloned()
            } else {
                self.entries.borrow().get(target).cloned()
            };
            let Some(entry) = entry else {
                return Ok(UndoResult::error(if target == "last" {
                    "There's no change of Kumi's left to undo.".into()
                } else {
                    format!("There's no change {} in this session.", head(target, 32))
                }));
            };
            let snapshot = entry.borrow().clone();
            if snapshot.record.state == ChangeState::Undone {
                return Ok(UndoResult::with(
                    snapshot.record.clone(),
                    stringify(&json!({"undone":snapshot.record.title,"change":snapshot.record.id,"already":true})),
                    false,
                ));
            }
            if snapshot.record.state == ChangeState::Expired {
                return Ok(UndoResult::with(
                    snapshot.record.clone(),
                    snapshot.record.note.unwrap_or_else(|| "Kumi can't undo this anymore.".into()),
                    true,
                ));
            }
            if snapshot.permanent == Some(true) {
                return Ok(UndoResult::with(
                    snapshot.record.clone(),
                    snapshot.record.note.unwrap_or_else(|| "Kumi can't take this back; Live's own undo (Cmd-Z in Live) can.".into()),
                    true,
                ));
            }
            let _ = self.connection.ensure_catalog(signal.clone()).await;
            if let Some(revert) = snapshot.revert {
                if !self.connection.available.get() || self.connection.lost.get() || !self.connection.has("live_run_python") {
                    return Ok(UndoResult::error("Kumi can't reach Live right now, so it can't undo."));
                }
                signal.check()?;
                let done = self
                    .run_fast(
                        revert_script(&json!(revert)),
                        abort::any([signal, self.connection.lifetime.clone(), abort::timeout(self.timeout_ms)]),
                    )
                    .await?;
                let result = match done {
                    FastResult::Result(v) => context::object(&v)?,
                    FastResult::Error { error, .. } => {
                        return Ok(UndoResult::with(
                            self.update(&entry, ChangeState::Unsure, Some("Live didn't confirm the undo; try again.".into())),
                            format!("Live didn't confirm the undo: {error}"),
                            true,
                        ))
                    }
                };
                let back = result.get("back").and_then(Value::as_f64).unwrap_or(0.);
                let names = |key: &str| {
                    result
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .take(16)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                };
                let moved = names("moved");
                let gone = names("gone");
                self.remember.schedule_save(20_000);
                if moved.is_empty() && gone.is_empty() {
                    return Ok(self.undone(&entry));
                }
                if back != 0. {
                    entry.borrow_mut().revert = None;
                }
                let mut parts = Vec::new();
                if back != 0. {
                    parts.push(format!("Kumi put back {} of its parameters", to_string(back)));
                }
                let them = if moved.len() == 1 { "it" } else { "them" };
                if !moved.is_empty() {
                    parts.push(if back != 0. {
                        format!("{} changed in Live since, so Kumi left {them}", moved.join(", "))
                    } else {
                        format!(
                            "{} changed in Live since Kumi set {them}, so Kumi left {them} as {}",
                            moved.join(", "),
                            if moved.len() == 1 { "it is" } else { "they are" }
                        )
                    });
                }
                if !gone.is_empty() {
                    parts.push(format!("{} {} in Live any more", gone.join(", "), if gone.len() == 1 { "isn't" } else { "aren't" }));
                }
                let note = format!("{}.", parts.join("; "));
                return Ok(UndoResult::with(self.update(&entry, ChangeState::Kept, Some(note.clone())), note, true));
            }
            if !self.connection.available.get() || self.connection.lost.get() || !self.connection.has("live_undo") {
                return Ok(UndoResult::error("Kumi can't reach Live right now, so it can't undo."));
            }
            signal.check()?;
            let undo_key = entry.borrow_mut().undo_key.get_or_insert_with(|| uuid::Uuid::new_v4().to_string()).clone();
            if let Some(members) = snapshot.members {
                let mut hidden = Vec::new();
                let standing =
                    |id: &String| self.entries.borrow().get(id).is_some_and(|entry| entry.borrow().record.state != ChangeState::Undone);
                self.quietly(Some(&mut hidden), async {
                    for id in members.iter().rev() {
                        if standing(id) {
                            let _ = self.undo(id, signal.clone(), false).await;
                        }
                    }
                })
                .await;
                // Its first steps past the most changes Kumi keeps, when it was recorded or since (the oldest go first).
                let past = snapshot.trimmed.unwrap_or(0) + members.iter().filter(|id| !self.entries.borrow().contains_key(*id)).count();
                let left = members.iter().filter(|id| standing(id)).count();
                if left == 0 && past == 0 {
                    return Ok(self.undone(&entry));
                }
                let total = members.len() + snapshot.trimmed.unwrap_or(0);
                let them = |count: usize| if count == 1 { "it" } else { "them" };
                let mut why = Vec::new();
                if left > 0 {
                    why.push(if past == 0 {
                        "the rest changed in Live since, so Kumi left them".to_owned()
                    } else {
                        format!("{} changed in Live since, so Kumi left {}", thousands(left), them(left))
                    });
                }
                if past > 0 {
                    why.push(format!(
                        "{} past the {} changes Kumi keeps, so only Live's own undo (Cmd-Z in Live) can take {} back",
                        if past == 1 { "the first is".to_owned() } else { format!("the first {} are", thousands(past)) },
                        thousands(MAX_ENTRIES),
                        them(past)
                    ));
                }
                let note = format!(
                    "Kumi took back {} of its {} changes; {}.",
                    thousands(total - past - left),
                    thousands(total),
                    why.join(", and ")
                );
                return Ok(UndoResult::with(self.update(&entry, ChangeState::Kept, Some(note.clone())), note, true));
            }
            if let Some(material) = snapshot.material.clone() {
                return self.undo_material(&entry, &snapshot, material, &undo_key, discard, signal).await;
            }
            if snapshot.transaction_id.is_empty() {
                return Ok(UndoResult::with(snapshot.record, "Kumi can't take this back; Live's own undo (Cmd-Z in Live) can.", true));
            }
            if let Some(mut stopped) = self.bridge_undo(&entry, &snapshot, &undo_key, discard).await? {
                if snapshot.shift.is_some() && stopped.record.as_ref().is_some_and(|record| record.state == ChangeState::Unsure) {
                    self.unknown_shift();
                    stopped.retired = true;
                }
                return Ok(stopped);
            }
            if let Some(shift) = &snapshot.shift {
                self.unshift(shift);
            }
            self.remember.schedule_save(20_000);
            if let Some(restore) = snapshot.restore {
                let mut book = self.connection.references.borrow_mut();
                if let Some(current) = book.known.get_mut(&restore.reference) {
                    if restore.field == "name" {
                        if let Some(value) = restore.value {
                            current.name = value;
                        }
                    } else {
                        current.color = restore.value.filter(|s| !s.is_empty());
                    }
                }
            }
            Ok(self.undone(&entry))
        }
        .boxed_local()
    }
    /// The change's own undo, through the bridge: None once it's undone, else what stopped it.
    async fn bridge_undo(
        &self,
        entry: &Rc<RefCell<Applied>>,
        snapshot: &Applied,
        undo_key: &str,
        discard: bool,
    ) -> Result<Option<UndoResult>, RuntimeError> {
        let mut args = object(json!({"transactionId":snapshot.transaction_id,"confirmation":"undo","idempotencyKey":undo_key}));
        if discard {
            args.insert("discard".into(), json!(true));
        }
        let bound = abort::any([self.connection.lifetime.clone(), abort::timeout(self.timeout_ms)]);
        let result = match self.connection.call("live_undo", args, bound).await {
            Ok(v) => v,
            Err(_) => {
                return Ok(Some(UndoResult::with(
                    self.update(entry, ChangeState::Unsure, Some("Live didn't answer the undo; try again.".into())),
                    "Live didn't answer the undo; it can be retried.",
                    true,
                )))
            }
        };
        if result.is_error == Some(true) {
            let message = head(&result_text(&result), 2048);
            let lower = message.to_ascii_lowercase();
            let refused = ["modified after apply", "changed before deletion", "undo refused"].iter().any(|s| lower.contains(s));
            let record = if uncertain(&result) && !refused {
                self.update(entry, ChangeState::Unsure, Some("Live didn't confirm the undo; try again.".into()))
            } else {
                self.update(entry, ChangeState::Kept, Some(undo_note(&message)))
            };
            return Ok(Some(UndoResult::with(record, message, true)));
        }
        let body = context::payload(&result)?;
        if body.get("state").and_then(Value::as_str) != Some("undone") {
            return Ok(Some(UndoResult::with(
                self.update(entry, ChangeState::Unsure, Some("Live didn't confirm the undo; try again.".into())),
                stringify(&json!(body)),
                true,
            )));
        }
        Ok(None)
    }
    /// Keep what a change cut or deleted with its record, for Kumi's undo to make again.
    /// Notes Live's identity for the device a change made.
    pub fn made(&self, change: &str, identity: String) {
        if let Some(entry) = self.entries.borrow().get(change) {
            entry.borrow_mut().created = Some(identity);
        }
    }
    /// Notes the tool that made a change.
    pub fn made_by(&self, change: &str, tool: &str) {
        if let Some(entry) = self.entries.borrow().get(change) {
            entry.borrow_mut().tool = Some(tool.into());
        }
    }
    pub fn attach_material(&self, change: &str, material: Material) {
        if let Some(entry) = self.entries.borrow().get(change) {
            entry.borrow_mut().material = Some(material);
        }
    }
    /// Undo a change that cut or deleted clips. What it cut is checked first, so nothing changes when it can't come
    /// back. Then, inside one Live undo step, the change's own undo (when it has one) and the clips made again whole.
    /// Live is asked on the connection's lifetime and a time limit, not the turn's, so an Esc doesn't stop it halfway.
    /// If Live stops partway anyway, the step is taken back when what the track holds shows it's Live's last.
    async fn undo_material(
        &self,
        entry: &Rc<RefCell<Applied>>,
        snapshot: &Applied,
        material: Material,
        undo_key: &str,
        discard: bool,
        _signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        let names = material.names();
        let remember = &self.remember;
        let mut leaves = vec![];
        for clip in &material.clips {
            match remember.history.kept_leaf(&clip.object, remember.store.as_ref(), remember.current().as_deref()).await {
                Some(leaf) => leaves.push(leaf),
                None => return Ok(self.left(entry, &names, "Kumi no longer has them")),
            }
        }
        if !super::cuts::can_snapshot(&self.connection) {
            return Ok(UndoResult::error(format!(
                "Kumi can't run its Python in this Live, so it can't bring back {names}; Live's own undo (Cmd-Z in Live) can take the change back."
            )));
        }
        let bound = || abort::any([self.connection.lifetime.clone(), abort::timeout(RESTORE_MS)]);
        if let Err(why) = snapshots::check(self, &material, &leaves, &bound).await {
            return Ok(self.refused(entry, &material, &names, &why));
        }
        // What the track holds before: a rollback is only ever made back to it.
        let before = snapshots::state(self, &material, bound()).await;
        let Some(step) = self.open_undo_step(bound()).await else {
            return Ok(UndoResult::error(format!(
                "Kumi couldn't open one Live undo step to bring back {names}, so it left the change as it is: try again, or Live's own undo (Cmd-Z in Live) can take it back."
            )));
        };
        let outcome = async {
            if material.host_undo {
                if let Some(stopped) = self.bridge_undo(entry, snapshot, undo_key, discard).await? {
                    return Ok(Err(stopped));
                }
            }
            Ok::<_, RuntimeError>(Ok(snapshots::restore(self, &material, &leaves, &bound).await))
        }
        .await;
        self.close_undo_step(Some(step)).await;
        self.remember.schedule_save(20_000);
        Ok(match outcome? {
            // The change's own undo didn't happen, and nothing else did: its record says why.
            Err(stopped) => stopped,
            Ok(Ok(restored)) => {
                let record = self.update(entry, ChangeState::Undone, None);
                let mut text = json!({"undone":record.title,"change":record.id,"broughtBack":names});
                if let Some(short) = restored.short_of() {
                    text["notExactly"] = json!(format!(
                        "{short}: Live doesn't give Kumi those, or take them back. Cmd-Z in Live twice, right away, takes this undo back and then the change, bringing all of it back."
                    ));
                }
                UndoResult::with(record, stringify(&text), false)
            }
            Ok(Err(snapshots::Stopped::Nothing(why))) if !material.host_undo => self.refused(entry, &material, &names, &why),
            Ok(Err(stopped)) => self.roll_back(entry, &material, &names, before, stopped, bound).await,
        })
    }
    /// Kumi can't bring the clips back: the change stays, Live's undo's to take back.
    fn left(&self, entry: &Rc<RefCell<Applied>>, names: &str, why: &str) -> UndoResult {
        let note =
            format!("Kumi can't bring back {names}: {why}. It left the change as it is; Live's own undo (Cmd-Z in Live) can take it back.");
        UndoResult::with(self.update(entry, ChangeState::Kept, Some(note.clone())), note, true)
    }
    /// A restore's checks refused, so nothing changed. What the producer can clear (a clip in the way, a file moved, a
    /// frozen or renamed track) leaves the change for Kumi's undo once they have; anything else leaves it to Live's.
    fn refused(&self, entry: &Rc<RefCell<Applied>>, material: &Material, names: &str, why: &str) -> UndoResult {
        if why.contains("again at its length") {
            // Its clips were made aside and deleted: Live's history keeps that empty step of Kumi's.
            let note = format!(
                "Kumi can't bring back {names}: {why}. It left the change as it is; Live's own undo (Cmd-Z in Live) can take it back, one press later than before: Kumi's try left an empty step on top of Live's undo history."
            );
            return UndoResult::with(self.update(entry, ChangeState::Kept, Some(note.clone())), note, true);
        }
        let fix = if why.contains("isn't the track it was") {
            Some(format!(
                "If it's \u{201c}{0}\u{201d} renamed, name it \u{201c}{0}\u{201d} again, then undo again; if another track took its place, Live's own undo (Cmd-Z in Live) can take the change back.",
                material.track_name
            ))
        } else if why.contains("'s place now") {
            why.find('\u{201d}').map(|end| format!("Move or delete {}, then undo again.", &why[..end + '\u{201d}'.len_utf8()]))
        } else if why.contains("'s slot now") {
            Some("Move or delete the clip in its slot, then undo again.".to_owned())
        } else if let Some(at) = why.find(MISSING_FILE) {
            // The path is all that follows the fixed words, less their closing parenthesis: it may hold " (" and ")".
            let file = &why[at + MISSING_FILE.len()..];
            Some(format!("Put the file back at {}, then undo again.", file.strip_suffix(')').unwrap_or(file)))
        } else if why.ends_with(" is frozen") {
            Some(format!("Unfreeze {}, then undo again.", why.trim_end_matches(" is frozen")))
        } else {
            None
        };
        match fix {
            Some(fix) => {
                let record = entry.borrow().record.clone();
                UndoResult::with(record, format!("Kumi can't bring back {names} yet: {why}. {fix}"), true)
            }
            None => self.left(entry, names, why),
        }
    }
    /// Kumi's undo stopped partway, its step closed by now. One Live undo takes all of the step back: made right away
    /// when Kumi knows its step changed something (its answer says what it removed or made, or the change's own undo
    /// ran), or else once the track shows it changed. Then the track has to be back as it was before. If the producer
    /// changed something in Live in that moment, Live's last step is theirs and the track isn't back: Kumi says so.
    /// Without what the track held before, Kumi doesn't touch Live's undo, and says what the producer can do.
    async fn roll_back(
        &self,
        entry: &Rc<RefCell<Applied>>,
        material: &Material,
        names: &str,
        before: Option<Value>,
        stopped: snapshots::Stopped,
        bound: impl Fn() -> Signal,
    ) -> UndoResult {
        let (why, known) = match stopped {
            snapshots::Stopped::Nothing(why) => (why, material.host_undo),
            snapshots::Stopped::Partway(why, done) => (why, material.host_undo || done.changed()),
        };
        if let Some(before) = &before {
            let unread = if known {
                false
            } else {
                match snapshots::state(self, material, bound()).await {
                    Some(now) if now == *before => {
                        let record = entry.borrow().record.clone();
                        return UndoResult::with(
                            record,
                            format!("Kumi couldn't bring back {names} ({why}); nothing changed. Undo again, or Live's own undo (Cmd-Z in Live) can take it back."),
                            true,
                        );
                    }
                    Some(_) => false,
                    // Unread, the track can't say whether Kumi's step changed anything: Live's undo might take back
                    // the producer's own last step instead, so it's left to them.
                    None => true,
                }
            };
            if !unread && self.live_undo_once(bound()).await {
                if snapshots::state(self, material, bound()).await.as_ref() == Some(before) {
                    if material.host_undo {
                        // The change's own undo went with it, and the bridge counts it done: only Live's undo is left.
                        let note = format!("Kumi's undo didn't finish ({why}), so it took back what it did: the change is as it was, and Live's own undo (Cmd-Z in Live) can take it back.");
                        return UndoResult::with(self.update(entry, ChangeState::Kept, Some(note.clone())), note, true);
                    }
                    let record = entry.borrow().record.clone();
                    return UndoResult::with(
                        record,
                        format!("Kumi's undo didn't finish ({why}), so it took back what it did: the change is as it was. Undo again, or Live's own undo (Cmd-Z in Live) can take it back."),
                        true,
                    );
                }
                let note = format!("Kumi's undo didn't finish ({why}), and taking it back didn't leave things as they were: something else may have changed in Live meanwhile. Check Live, where Cmd-Z and Shift-Cmd-Z step through it.");
                return UndoResult::with(self.update(entry, ChangeState::Unsure, Some(note.clone())), note, true);
            }
        }
        let note = format!("Kumi's undo didn't finish ({why}). Cmd-Z in Live once puts back what it changed.");
        UndoResult::with(self.update(entry, ChangeState::Unsure, Some(note.clone())), note, true)
    }
    /// Live's own undo, once: whether Live did one.
    async fn live_undo_once(&self, signal: Signal) -> bool {
        let args = object(json!({"confirmation":"undo-in-live","idempotencyKey":uuid::Uuid::new_v4().to_string()}));
        match self.connection.call("live_song_undo", args, signal).await {
            Ok(result) if result.is_error != Some(true) => {
                context::payload(&result).ok().is_some_and(|done| done.get("done") == Some(&json!(true)))
            }
            _ => false,
        }
    }
    /// One Live undo step around what follows, when the bridge offers it: its id, to close it by.
    async fn open_undo_step(&self, signal: Signal) -> Option<String> {
        if !self.connection.has("live_undo_step_begin") || !self.connection.has("live_undo_step_end") {
            return None;
        }
        let opened =
            self.connection.call("live_undo_step_begin", object(json!({"label":"Kumi: undo","timeoutMs":120_000})), signal).await.ok()?;
        context::payload(&opened).ok()?.get("stepId").and_then(Value::as_str).map(str::to_owned)
    }
    async fn close_undo_step(&self, step: Option<String>) {
        if let Some(step) = step {
            let bound = abort::any([self.connection.lifetime.clone(), abort::timeout(10_000)]);
            let _ = self.connection.call("live_undo_step_end", object(json!({"stepId":step})), bound).await;
        }
    }
    fn undone(&self, entry: &Rc<RefCell<Applied>>) -> UndoResult {
        let record = self.update(entry, ChangeState::Undone, None);
        let text = stringify(&json!({"undone":record.title,"change":record.id}));
        UndoResult::with(record, text, false)
    }
    pub async fn stop_everything(&self, signal: Signal) -> bool {
        self.try_stop_everything(signal).await.unwrap_or(false)
    }
    async fn try_stop_everything(&self, signal: Signal) -> Result<bool, RuntimeError> {
        if !self.connection.available.get() || self.connection.lost.get() || self.connection.tools().is_none() {
            return Ok(false);
        }
        self.connection.ensure_catalog(signal.clone()).await?;
        if !self.connection.has(EMERGENCY_STOP) || !self.connection.has("live_discover") {
            return Ok(false);
        }
        for _ in 0..2 {
            let read = context::payload(
                &self.connection.call("live_discover", object(json!({"kind":"session-playback","limit":1})), signal.clone()).await?,
            )?;
            let playback = context::object(
                read.get("items").and_then(Value::as_array).and_then(|a| a.first()).filter(|v| !v.is_null()).unwrap_or(&json!({})),
            )?;
            let transport = context::object(playback.get("transport").filter(|v| !v.is_null()).unwrap_or(&json!({})))?;
            let mut targets = IndexSet::new();
            for key in ["firedTargets", "playingTargets"] {
                for target in playback.get(key).and_then(Value::as_array).into_iter().flatten() {
                    let target = context::object(target)?;
                    targets.insert(format!(
                        "{}|{}|{}",
                        js_string(target.get("trackRef")),
                        js_string(target.get("clipSlotRef")),
                        js_string(target.get("sceneRef"))
                    ));
                }
            }
            let mut targets: Vec<_> = targets.into_iter().collect();
            targets.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            let recording = match (
                transport.get("sessionRecord") == Some(&Value::Bool(true)),
                transport.get("arrangementRecord") == Some(&Value::Bool(true)),
            ) {
                (true, true) => "both",
                (true, false) => "session",
                (false, true) => "arrangement",
                _ => "stopped",
            };
            if transport.get("playing") != Some(&Value::Bool(true)) && targets.is_empty() && recording == "stopped" {
                return Ok(true);
            }
            let result=self.connection.call(EMERGENCY_STOP,object(json!({"confirmation":"emergency-stop","expectedTargets":targets,"expectedRecording":recording,"idempotencyKey":uuid::Uuid::new_v4().to_string()})),signal.clone()).await?;
            if result.is_error != Some(true) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
pub fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            ContentBlock::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn uncertain(result: &CallToolResult) -> bool {
    result.structured_content.as_ref().and_then(|v| v.get("state")).and_then(Value::as_str) == Some("uncertain")
        || result_text(result).to_ascii_lowercase().contains("uncertain")
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(a)) => {
            a.iter().map(|v| if v.is_null() { String::new() } else { js_string(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(v) => stringify(v),
    }
}
fn start_background(work: impl Future<Output = ()> + 'static) {
    let mut work = Box::pin(work);
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    if work.as_mut().poll(&mut cx).is_pending() {
        tokio::task::spawn_local(work);
    }
}
