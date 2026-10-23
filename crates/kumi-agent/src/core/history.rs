//! Kumi's change records and its guarded undo, over the DAW's ChangeHost: HISTORY's lifecycle (records, groups,
//! quiet audition steps, what changed outside it, refusals) is the same for every DAW; each DAW's ways back are its own.
use crate::core::refs::{Book, Moved, RefScheme, Shift};
use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, string::head},
};
use kumi_listening::notation::thousands;
use kumi_types::contracts::*;
use kumi_types::picture::PictureValue;
use kumi_types::{daw::DawProfile, errors::RuntimeError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

/// The most changes a session's history keeps, the oldest going first.
pub const MAX_ENTRIES: usize = 20_000;
/// The newest changes whose pictures the history keeps.
pub const PICTURES_KEPT: usize = 500;

pub fn next_change_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("c{}", COUNTER.fetch_add(1, Ordering::Relaxed) + 1)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Restore {
    #[serde(rename = "ref")]
    pub reference: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound(serialize = "M: Serialize", deserialize = "M: Deserialize<'de>"))]
pub struct Applied<M> {
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
    /// The DAW's own way back, kept with the change: each target's `ref` and `device` follow restructures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert: Option<Vec<Value>>,
    /// What the change cut or deleted, which Kumi's undo makes again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<M>,
    /// A group's first steps that were past the most changes Kumi keeps when it was recorded: only the DAW's own undo
    /// takes them back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trimmed: Option<usize>,
    /// Where the change moved the project's tracks, which the refs that followed it go back from once it's undone.
    /// Not kept past this connection to the DAW: its refs aren't either.
    #[serde(skip)]
    pub shift: Option<Shift>,
    /// The DAW's identity for the device the change made (a load or a duplicate), when the bridge said: what may be
    /// removed in its place if the DAW won't undo it. Not kept past this connection: identities aren't either.
    #[serde(skip)]
    pub created: Option<String>,
    /// The tool that made the change: whether it can change what's heard (a rename, the transport or an empty track
    /// can't). Not kept past this Kumi: a change read back from disk goes by its family.
    #[serde(skip)]
    pub tool: Option<String>,
    /// The clip a note edit changed, by its long ref: a groove round takes back its own clip's edits only. Not kept
    /// past this connection: refs aren't either.
    #[serde(skip)]
    pub clip: Option<String>,
}
impl<M> Applied<M> {
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
            clip: None,
        }
    }
}
/// A change as HISTORY holds it.
pub type Entry<M> = Rc<RefCell<Applied<M>>>;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<ChangeRecord>,
    pub text: String,
    pub is_error: bool,
    /// Kumi's refs were retired with it (a restructure's undo the DAW didn't confirm). The model is told so with the
    /// undo (`for_model`); the producer reads `text` as it is (a render's cleanup notice, /undo).
    #[serde(skip)]
    pub retired: bool,
    /// What the model is told when they were: the DAW's words.
    #[serde(skip)]
    refs_retired: &'static str,
}
impl UndoResult {
    pub fn error(text: impl Into<String>) -> Self {
        Self { record: None, text: text.into(), is_error: true, retired: false, refs_retired: "" }
    }
    pub fn with(record: ChangeRecord, text: impl Into<String>, is_error: bool) -> Self {
        Self { record: Some(record), text: text.into(), is_error, retired: false, refs_retired: "" }
    }
    /// The undo's text as the model reads it: with what to do about Kumi's refs when they were retired with it.
    pub fn for_model(&self) -> String {
        if !self.retired {
            return self.text.clone();
        }
        let text = self.text.trim_end();
        let stop = if text.ends_with(['.', '!', '?']) { "" } else { "." };
        format!("{text}{stop} {}", self.refs_retired)
    }
}
/// A device's changes waiting for its curve to be read (`parameters::curve_later`): each change's id and every value
/// it moved (not the record's shortened list).
#[derive(Default)]
pub struct CurveRun {
    /// Waiting for the next read.
    pub waiting: Vec<(String, Vec<PictureValue>)>,
    /// Being read now: listed until the read lands, so changes made meanwhile walk back over them.
    pub reading: Vec<(String, Vec<PictureValue>)>,
    /// When the last change came (performance time).
    pub last: f64,
}
/// Told where a restructure Kumi's undo took back moved the project's tracks, or None when it can't be known.
pub type OnShift = Rc<dyn Fn(Option<&Shift>)>;

/// What HISTORY asks of a DAW: its refs, its clock and memory, and its ways back for one change.
#[async_trait(?Send)]
pub trait ChangeHost: Sized {
    /// The DAW's refs, which a restructure's undo moves back.
    type Refs: RefScheme;
    /// What a change cut or deleted, kept with its record for Kumi's undo to make again.
    type Material: Clone + Serialize + DeserializeOwned;
    /// The DAW's words, for what the undo says.
    fn profile(&self) -> &DawProfile;
    /// The turn's refs.
    fn book(&self) -> &RefCell<Book<Self::Refs>>;
    /// How long the connection to the DAW lasts.
    fn lifetime(&self) -> Signal;
    /// The time now, in milliseconds since the epoch.
    fn now_ms(&self) -> i64;
    /// HISTORY changed: the project's memory keeps it soon.
    fn keep_soon(&self);
    /// Changes Kumi won't undo any more: the DAW lets go of their transactions (never empty).
    fn release(&self, transactions: Vec<String>);
    /// The DAW's tools are known (before an undo, or a batch): whether they are.
    async fn ready(&self, signal: Signal) -> bool;
    /// Whether the DAW can take a change back through its bridge now.
    fn reachable(&self) -> bool;
    /// Whether the DAW gives a plan one undo step of its own now, so one press of its undo takes the plan back.
    fn plan_steps(&self) -> bool {
        false
    }
    /// Opens a plan's undo step: its id, to close it by.
    async fn step_begin(&self) -> Option<String> {
        None
    }
    /// Closes a plan's undo step.
    async fn step_end(&self, _id: String) {}
    /// Stops the DAW's playback and recording (the emergency stop): whether it's stopped.
    async fn stop_transport(&self, _signal: Signal) -> bool {
        false
    }
    /// Undo a change by the way back kept with it (`Applied::revert`).
    async fn revert(
        &self,
        history: &History<Self>,
        entry: &Entry<Self::Material>,
        revert: Vec<Value>,
        signal: Signal,
    ) -> Result<UndoResult, RuntimeError>;
    /// Undo a change that cut or deleted material, making it again.
    async fn undo_material(
        &self,
        history: &History<Self>,
        entry: &Entry<Self::Material>,
        snapshot: &Applied<Self::Material>,
        material: Self::Material,
        undo_key: &str,
        discard: bool,
        signal: Signal,
    ) -> Result<UndoResult, RuntimeError>;
    /// The change's own undo, through the bridge's transaction: None once it's undone, else what stopped it.
    async fn undo_transaction(
        &self,
        history: &History<Self>,
        entry: &Entry<Self::Material>,
        snapshot: &Applied<Self::Material>,
        undo_key: &str,
        discard: bool,
    ) -> Result<Option<UndoResult>, RuntimeError>;
}

pub struct History<H: ChangeHost> {
    pub host: H,
    pub entries: RefCell<IndexMap<String, Entry<H::Material>>>,
    /// Changes to a device's curve waiting for one read after the last of them, by device: each change's id and the
    /// values it moved, and when the last came (`parameters::curve_later`).
    pub curve_runs: RefCell<HashMap<String, CurveRun>>,
    pub changes_this_turn: Cell<usize>,
    quiet: RefCell<Option<Vec<String>>>,
    pub timeout_ms: u64,
    on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
    /// What else follows a restructure Kumi's undo takes back (what the turns showed of the project's devices).
    pub on_shift: RefCell<Option<OnShift>>,
    /// What changed the DAW outside HISTORY, oldest first (code run in it, a command, its own undo): nothing a judged
    /// round can take back.
    pub outside: RefCell<Vec<&'static str>>,
}
/// A HISTORY reads as its host: Live's connection and its memory of the Set, or the own bridges' host.
impl<H: ChangeHost> std::ops::Deref for History<H> {
    type Target = H;
    fn deref(&self) -> &H {
        &self.host
    }
}
impl<H: ChangeHost> History<H> {
    pub fn change_signal(&self) -> Signal {
        abort::any([self.host.lifetime(), abort::timeout(self.timeout_ms)])
    }
    /// A change's time in the DAW, sized to the change: a change of many tracks, pads or clips takes the DAW a while
    /// for each, and each costs more in a big project (64 tracks took Live 28 s, and were called unconfirmed at a flat
    /// 30 s, #260). The bridge bounds each of the DAW's steps itself, so this only needs to outlast them; at most ten
    /// minutes.
    pub fn change_signal_for(&self, items: usize) -> Signal {
        let more = items.saturating_sub(8) as u64 * (self.timeout_ms / 15);
        abort::any([self.host.lifetime(), abort::timeout((self.timeout_ms + more).min(600_000.max(self.timeout_ms)))])
    }
    pub fn over(host: H, timeout_ms: Option<u64>, on_change: Option<Rc<dyn Fn(ChangeRecord)>>) -> Self {
        Self {
            host,
            timeout_ms: timeout_ms.unwrap_or(30_000),
            on_change,
            entries: RefCell::new(IndexMap::new()),
            curve_runs: RefCell::default(),
            changes_this_turn: Cell::new(0),
            quiet: RefCell::new(None),
            on_shift: RefCell::new(None),
            outside: RefCell::new(vec![]),
        }
    }
    /// What changed the DAW outside HISTORY since `mark` (a count of `outside`), each said once.
    pub fn outside_since(&self, mark: usize) -> Vec<&'static str> {
        let mut said = vec![];
        for what in self.outside.borrow().iter().skip(mark) {
            if !said.contains(what) {
                said.push(*what);
            }
        }
        said
    }
    /// A restructure this change made, which its undo moves the refs that followed it back from.
    pub fn restructured(&self, id: &str, shift: &Shift) {
        if let Some(entry) = self.entries.borrow().get(id) {
            entry.borrow_mut().shift = Some(shift.clone());
        }
    }
    /// A restructure undone: what followed it goes back, the book's refs (which the DAW reads again at their places
    /// before code uses them), HISTORY's, and what the turns showed.
    fn unshift(&self, shift: &Shift) {
        let back = shift.inverse();
        let moved = self.shifted(&back);
        let mut book = self.host.book().borrow_mut();
        book.shift(&back);
        book.mark_moved(moved);
        drop(book);
        let on_shift = self.on_shift.borrow().clone();
        if let Some(on_shift) = on_shift {
            on_shift(Some(&back));
        }
    }
    /// A restructure's undo the DAW didn't confirm: whether the tracks moved back can't be known, so the refs that
    /// followed it are retired, as after a restructure Kumi can't read exactly, and what the turns showed with them.
    fn unknown_shift(&self) {
        let mut book = self.host.book().borrow_mut();
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
        // Only the newest changes keep their pictures here (each up to 4 KB, and the history keeps thousands).
        if let Some((_, older)) = self.entries.borrow().iter().rev().nth(PICTURES_KEPT) {
            older.borrow_mut().record.picture = None;
        }
        if let Some(quiet) = self.quiet.borrow_mut().as_mut() {
            quiet.push(record.id.clone());
            return;
        }
        self.emit(&record);
        self.host.keep_soon();
    }
    /// A picture made after the change was told (a device's curve, read once it was applied): kept on its record, and
    /// the record told again, unless it's part of a group or Kumi is keeping changes quiet.
    pub fn set_picture(&self, id: &str, picture: ChangePicture) {
        let record = {
            let entries = self.entries.borrow();
            let Some(entry) = entries.get(id) else { return };
            let mut entry = entry.borrow_mut();
            // Undone, expired or unsure since: what was read isn't its after.
            if entry.record.state != ChangeState::Applied {
                return;
            }
            entry.record.picture = Some(picture);
            if entry.within.is_some() {
                return;
            }
            entry.record.clone()
        };
        if !self.is_quiet() {
            self.emit(&record);
        }
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
    pub fn update(&self, entry: &Entry<H::Material>, state: ChangeState, note: Option<String>) -> ChangeRecord {
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
        struct Guard<'a, H: ChangeHost> {
            history: &'a History<H>,
            outer: Option<Vec<String>>,
            counted: usize,
            into: Option<&'a mut Vec<String>>,
        }
        impl<H: ChangeHost> Drop for Guard<'_, H> {
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
    /// Drops changes from HISTORY, and from the quiet list they're on, once what they made is gone some other way (a
    /// rig's device deleted by its identity): there's nothing left for them to take back. Their transactions are
    /// released.
    pub fn forget(&self, ids: &[String]) {
        if let Some(quiet) = self.quiet.borrow_mut().as_mut() {
            quiet.retain(|id| !ids.contains(id));
        }
        let released: Vec<String> = ids
            .iter()
            .filter_map(|id| self.entries.borrow_mut().shift_remove(id))
            .map(|entry| entry.borrow().transaction_id.clone())
            .collect();
        self.release(&released);
    }
    pub fn release(&self, given: &[String]) {
        let ids: Vec<_> = given.iter().filter(|s| !s.is_empty()).cloned().collect();
        if ids.is_empty() {
            return;
        }
        self.host.release(ids);
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
        let record: ChangeRecord = serde_json::from_value(
            json!({"id":next_change_id(),"family":"clip","title":head(title,160),"state":"applied","at":self.host.now_ms()}),
        )
        .unwrap();
        for id in &members {
            self.entries.borrow()[id].borrow_mut().within = Some(record.id.clone());
        }
        let mut entry = Applied::new(record.clone(), String::new(), None);
        entry.members = Some(members);
        entry.trimmed = (trimmed > 0).then_some(trimmed);
        self.entries.borrow_mut().insert(record.id.clone(), Rc::new(RefCell::new(entry)));
        self.emit(&record);
        self.host.keep_soon();
        Some(record.id)
    }
    /// After a restructure the refs Kumi's undo names follow what they named, as the book's do, so an undo puts back
    /// the parameter it set, not what moved into its place; one on what was deleted names nothing, and its undo says
    /// so. Returns the refs moved, which the DAW reads again before code uses them.
    pub fn shifted(&self, shift: &Shift) -> Vec<String> {
        let scheme = H::Refs::default();
        // Curves waiting to be read go: the device each names may have moved, and another may sit in its place.
        self.curve_runs.borrow_mut().clear();
        let mut moved = Vec::new();
        for entry in self.entries.borrow().values() {
            let mut entry = entry.borrow_mut();
            for target in entry.revert.iter_mut().flatten() {
                for key in ["ref", "device"] {
                    let Some(reference) = target.get(key).and_then(Value::as_str) else { continue };
                    match scheme.moved(shift, reference) {
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
                if let Moved::To(now) = scheme.moved(shift, &restore.reference) {
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
    pub fn undo<'a>(&'a self, target: &'a str, signal: Signal, discard: bool) -> LocalBoxFuture<'a, Result<UndoResult, RuntimeError>> {
        async move {
            let profile = self.host.profile();
            // The latest change, confirmed or not: one the DAW didn't confirm (64 tracks added past the time limit) is
            // the one the producer means, and undoing the one before it instead took back the wrong change (#260).
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
                    snapshot.record.note.unwrap_or_else(|| cant_take_back(profile)),
                    true,
                ));
            }
            self.host.ready(signal.clone()).await;
            if let Some(revert) = snapshot.revert {
                return self.host.revert(self, &entry, revert, signal).await;
            }
            if !self.host.reachable() {
                return Ok(UndoResult::error(cant_reach_to_undo(profile)));
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
                        format!("the rest changed in {} since, so Kumi left them", profile.words.app)
                    } else {
                        format!("{} changed in {} since, so Kumi left {}", thousands(left), profile.words.app, them(left))
                    });
                }
                if past > 0 {
                    why.push(format!(
                        "{} past the {} changes Kumi keeps, so only {} can take {} back",
                        if past == 1 { "the first is".to_owned() } else { format!("the first {} are", thousands(past)) },
                        thousands(MAX_ENTRIES),
                        profile.vocab.own_undo,
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
                return self.host.undo_material(self, &entry, &snapshot, material, &undo_key, discard, signal).await;
            }
            if snapshot.transaction_id.is_empty() {
                return Ok(UndoResult::with(snapshot.record, cant_take_back(profile), true));
            }
            if let Some(mut stopped) = self.host.undo_transaction(self, &entry, &snapshot, &undo_key, discard).await? {
                if snapshot.shift.is_some() && stopped.record.as_ref().is_some_and(|record| record.state == ChangeState::Unsure) {
                    self.unknown_shift();
                    stopped.retired = true;
                    stopped.refs_retired = profile.vocab.refs_retired;
                }
                return Ok(stopped);
            }
            if let Some(shift) = &snapshot.shift {
                self.unshift(shift);
            }
            self.host.keep_soon();
            if let Some(restore) = snapshot.restore {
                let mut book = self.host.book().borrow_mut();
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
    /// Keep what a change cut or deleted with its record, for Kumi's undo to make again.
    /// Notes the DAW's identity for the device a change made.
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
    /// Notes the clip a change edited (its long ref).
    pub fn made_on(&self, change: &str, clip: &str) {
        if let Some(entry) = self.entries.borrow().get(change) {
            entry.borrow_mut().clip = Some(clip.into());
        }
    }
    pub fn attach_material(&self, change: &str, material: H::Material) {
        if let Some(entry) = self.entries.borrow().get(change) {
            entry.borrow_mut().material = Some(material);
        }
    }
    /// Stops the DAW's playback and recording: whether it's stopped.
    pub async fn stop_everything(&self, signal: Signal) -> bool {
        self.host.stop_transport(signal).await
    }
    pub fn undone(&self, entry: &Entry<H::Material>) -> UndoResult {
        let record = self.update(entry, ChangeState::Undone, None);
        let text = stringify(&json!({"undone":record.title,"change":record.id}));
        UndoResult::with(record, text, false)
    }
}
/// What Kumi says of a change only the DAW's own undo can take back.
pub fn cant_take_back(profile: &DawProfile) -> String {
    format!("Kumi can't take this back; {} can.", profile.vocab.own_undo)
}
/// What Kumi says when the DAW isn't there to take a change back.
pub fn cant_reach_to_undo(profile: &DawProfile) -> String {
    format!("Kumi can't reach {} right now, so it can't undo.", profile.words.app)
}
