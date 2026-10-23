//! Live's change records and its ways back: the revert script, the bridge's transactions, material made again,
//! Live's own undo and the emergency stop. HISTORY itself is core's (`core::history`).
use super::{
    changes::{undo_note, EMERGENCY_STOP},
    connection::LiveConnection,
    context,
    fast::revert_script,
    observation::ObservedChange,
    references::{LiveRefs, References},
    remember::Remember,
    snapshots::{self, Material},
    views::ViewHost,
};
pub use crate::core::history::{CurveRun, OnShift, Restore, UndoResult, MAX_ENTRIES, PICTURES_KEPT};
use crate::{
    core::{
        contracts::*,
        daw::DawProfile,
        errors::RuntimeError,
        history::{cant_reach_to_undo, ChangeHost},
    },
    mcp::types::{CallToolResult, ContentBlock},
};
use async_trait::async_trait;
use indexmap::IndexSet;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, number::to_string, string::head},
};
use serde_json::{json, Value};
use std::{cell::RefCell, future::Future, ops::Deref, rc::Rc};

/// The most each call to Live of Kumi's undo of a cut waits.
const RESTORE_MS: u64 = 30_000;
/// How Kumi's Live script says a kept audio clip's file is gone, before its path (snapshots.py).
const MISSING_FILE: &str = "audio file isn't there any more (";

/// HISTORY over Live.
pub type History = crate::core::history::History<LiveChanges>;
/// A change as Live's HISTORY keeps it, with the clips it cut.
pub type Applied = crate::core::history::Applied<Material>;

impl Applied {
    /// Whether the change can change what's heard: not one that only names, colours or marks things, sets the
    /// transport, or adds empty tracks and scenes (or captures one). The scale can: tunings and devices follow it.
    pub fn audible(&self) -> bool {
        const SILENT: [&str; 7] =
            ["rename", "set_track_color", "set_locators", "delete_locator", "set_transport", "add_tracks_and_scenes", "capture_scene"];
        match &self.tool {
            Some(tool) => !SILENT.contains(&tool.as_str()),
            None => !matches!(self.record.family, ChangeFamily::Rename | ChangeFamily::Color | ChangeFamily::Locators),
        }
    }
}
/// The Remote Script's python.run error type for a failure after the code ran (its result, the deadline after it,
/// its undo step): Live may have changed.
pub const PYTHON_RAN: &str = "RanResultUnavailable";
pub enum FastResult {
    Result(Value),
    Error { error: String, sent: bool },
}

/// What HISTORY asks of Live: its connection, and Kumi's memory of the Set.
pub struct LiveChanges {
    pub connection: Rc<LiveConnection>,
    pub remember: Rc<Remember>,
}
impl Deref for History {
    type Target = LiveChanges;
    fn deref(&self) -> &LiveChanges {
        &self.host
    }
}
#[async_trait(?Send)]
impl ChangeHost for LiveChanges {
    type Refs = LiveRefs;
    type Material = Material;
    fn profile(&self) -> &DawProfile {
        &DawProfile::ABLETON
    }
    fn book(&self) -> &RefCell<References> {
        &self.connection.references
    }
    fn lifetime(&self) -> Signal {
        self.connection.lifetime.clone()
    }
    fn now_ms(&self) -> i64 {
        self.connection.now().timestamp_millis()
    }
    fn keep_soon(&self) {
        self.remember.schedule_save(20_000);
    }
    fn release(&self, ids: Vec<String>) {
        if !self.connection.has("live_transaction_release") {
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
    async fn ready(&self, signal: Signal) -> bool {
        self.connection.ensure_catalog(signal).await.is_ok()
    }
    fn reachable(&self) -> bool {
        self.connection.available.get() && !self.connection.lost.get() && self.connection.has("live_undo")
    }
    fn plan_steps(&self) -> bool {
        self.connection.has("live_undo_step_begin") && self.connection.has("live_undo_step_end")
    }
    async fn step_begin(&self) -> Option<String> {
        let signal = abort::any([self.connection.lifetime.clone(), abort::timeout(10_000)]);
        let opened =
            self.connection.call("live_undo_step_begin", object(json!({"label":"Kumi","timeoutMs":600_000})), signal).await.ok()?;
        context::payload(&opened).ok().and_then(|p| p.get("stepId").and_then(Value::as_str).map(str::to_owned))
    }
    async fn step_end(&self, id: String) {
        let signal = abort::any([self.connection.lifetime.clone(), abort::timeout(10_000)]);
        let _ = self.connection.call("live_undo_step_end", object(json!({"stepId":id})), signal).await;
    }
    async fn stop_transport(&self, signal: Signal) -> bool {
        self.try_stop_everything(signal).await.unwrap_or(false)
    }
    async fn revert(
        &self,
        history: &History,
        entry: &Rc<RefCell<Applied>>,
        revert: Vec<Value>,
        signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        history.revert_in_live(entry, revert, signal).await
    }
    async fn undo_material(
        &self,
        history: &History,
        entry: &Rc<RefCell<Applied>>,
        snapshot: &Applied,
        material: Material,
        undo_key: &str,
        discard: bool,
        signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        history.undo_material(entry, snapshot, material, undo_key, discard, signal).await
    }
    async fn undo_transaction(
        &self,
        history: &History,
        entry: &Rc<RefCell<Applied>>,
        snapshot: &Applied,
        undo_key: &str,
        discard: bool,
    ) -> Result<Option<UndoResult>, RuntimeError> {
        history.bridge_undo(entry, snapshot, undo_key, discard).await
    }
}

impl History {
    pub fn new(
        connection: Rc<LiveConnection>,
        remember: Rc<Remember>,
        timeout_ms: Option<u64>,
        on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
    ) -> Self {
        Self::over(LiveChanges { connection, remember }, timeout_ms, on_change)
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
    /// Undo a change by the parameters its revert script puts back, unless Live changed them since.
    async fn revert_in_live(&self, entry: &Rc<RefCell<Applied>>, revert: Vec<Value>, signal: Signal) -> Result<UndoResult, RuntimeError> {
        if !self.connection.available.get() || self.connection.lost.get() || !self.connection.has("live_run_python") {
            return Ok(UndoResult::error(cant_reach_to_undo(self.host.profile())));
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
                    self.update(entry, ChangeState::Unsure, Some("Live didn't confirm the undo; try again.".into())),
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
            return Ok(self.undone(entry));
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
        Ok(UndoResult::with(self.update(entry, ChangeState::Kept, Some(note.clone())), note, true))
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
}
impl LiveChanges {
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
