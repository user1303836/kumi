//! Shared execution services for plans, arrangement, and listening in Live.
use super::{
    actions::{ActionSummary, ACTIONS},
    arrange::{ArrangeHost, Built, Made, QuietBuilt, UndoStep},
    changes::CHANGES,
    context::{object, payload, ObservationError},
    mutations::Mutations,
    views::{self, ViewHost},
};
use crate::core::{contracts::JsonObject, errors::RuntimeError};
use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        number::to_string,
        string::{head, trim},
    },
};
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::Cell, path::Path, rc::Rc, sync::LazyLock, time::UNIX_EPOCH};

impl Mutations {
    pub async fn step(&self, tool: &str, input: JsonObject, signal: Signal) -> Result<JsonObject, RuntimeError> {
        let (text, failed) = if let Some(kind) = CHANGES.iter().find(|kind| kind.tool == tool) {
            let result = self.change(kind, input, signal, true).await;
            (result.text, result.is_error)
        } else {
            let action = ACTIONS.iter().find(|kind| kind.tool == tool).ok_or_else(|| RuntimeError::plain("Unknown Live step"))?;
            let result = self.act(action, input, signal, false).await;
            (result.text, result.is_error)
        };
        if failed {
            return Err(ObservationError(format!("{tool}: {}", head(&text, 400))).into());
        }
        Ok(serde_json::from_str(&text).ok().and_then(|v: Value| v.as_object().cloned()).unwrap_or_default())
    }
    pub async fn clip_file(&self, named: &str, original: Signal) -> Result<Option<String>, RuntimeError> {
        static ARRANGEMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):arrangement_clip:([0-9]+):[0-9]+$").unwrap());
        static SESSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):clip:([0-9]+):([0-9]+)$").unwrap());
        static SHORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:arrangement_clip|clip):[A-Za-z0-9_:]+$").unwrap());
        let connection = &self.parameters.history.connection;
        let given = trim(named);
        let reference = connection.references.borrow().lengthen(&json!({"ref":given}))["ref"].as_str().unwrap_or(given).to_owned();
        let arrangement = ARRANGEMENT.captures(&reference);
        let session = SESSION.captures(&reference);
        if arrangement.is_none() && session.is_none() {
            if SHORT.is_match(given) {
                return Err(ObservationError("That clip isn't one from this turn's discovery; discover it again.".into()).into());
            }
            return Ok(None);
        }
        if !connection.available.get() || connection.lost.get() || connection.tools().is_none() {
            return Err(ObservationError("Live isn't connected, so Kumi can't find that clip's file.".into()).into());
        }
        let signal = abort::any([original, connection.lifetime.clone(), abort::timeout(15_000)]);
        let (kind, parent) = if let Some(m) = arrangement {
            ("arrangement-clip", format!("{}:track:{}", &m[1], &m[2]))
        } else {
            let m = session.unwrap();
            ("session-clip", format!("{}:clip_slot:{}:{}", &m[1], &m[2], &m[3]))
        };
        let read=views::pages(connection.as_ref(),object(&json!({"kind":kind,"parent":parent,"fields":["ref","name","isAudio","filePath"],"limit":connection.page_limit(),"budget":connection.whole_budget()}))?,signal).await?;
        let rows = if read.is_error == Some(true) {
            Vec::new()
        } else {
            payload(&read)?.get("items").and_then(Value::as_array).cloned().unwrap_or_default()
        };
        let rows = rows.iter().map(object).collect::<Result<Vec<_>, _>>()?;
        let clip = rows
            .iter()
            .find(|row| row.get("ref").and_then(Value::as_str) == Some(&reference))
            .ok_or_else(|| ObservationError("That clip isn't in the Set any more; discover it again.".into()))?;
        if clip.get("isAudio") == Some(&json!(false)) {
            return Err(ObservationError("That's a MIDI clip, which has no sound of its own: record its track to audio first (resampling), then listen to the recording.".into()).into());
        }
        let file = clip
            .get("filePath")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ObservationError("Live didn't say which file that clip plays.".into()))?;
        Ok(Some(file.into()))
    }
    pub async fn keep_copy(&self, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let history = &self.parameters.history;
        let connection = &history.connection;
        let Some(path) = history.remember.current().and_then(|project| project.path.clone()).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if !["live_project_backup_preview", "live_project_backup_apply"].iter().all(|name| connection.has(name)) {
            return Ok(None);
        }
        let Ok(stats) = std::fs::metadata(&path) else { return Ok(None) };
        let modified = match stats.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
            Some(t) => t.as_secs() as f64 * 1000.0 + t.subsec_nanos() as f64 / 1_000_000.0,
            None => return Ok(None),
        };
        let version = format!("{path}:{}:{}", stats.len(), to_string(modified));
        if self.copied.borrow().contains(&version) {
            return Ok(None);
        }
        let result: Result<Option<String>, RuntimeError> = async {
            let folder = Path::new(&path).parent().unwrap_or(Path::new(".")).to_string_lossy();
            let previewed = connection
                .call("live_project_backup_preview", object(&json!({"confirmation":"backup","allowedRoot":folder}))?, signal.clone())
                .await?;
            if previewed.is_error == Some(true) {
                return Ok(None);
            }
            let preview = payload(&previewed)?;
            let Some(transaction) = preview.get("transactionId").and_then(Value::as_str) else { return Ok(None) };
            let applied = connection
                .call(
                    "live_project_backup_apply",
                    object(&json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":uuid::Uuid::new_v4().to_string()}))?,
                    signal.clone(),
                )
                .await?;
            if applied.is_error == Some(true) {
                return Ok(None);
            }
            let applied = payload(&applied)?;
            Ok(applied.get("backup").and_then(Value::as_str).map(str::to_owned))
        }
        .await;
        match result {
            Ok(backup) => {
                if backup.is_some() {
                    self.copied.borrow_mut().insert(version);
                }
                Ok(backup)
            }
            Err(_) => {
                signal.check()?;
                Ok(None)
            }
        }
    }
    pub fn arrange_host(self: &Rc<Self>) -> ArrangementSession {
        ArrangementSession { mutations: self.clone(), confirmed: Cell::new(false) }
    }
}
pub struct ArrangementSession {
    mutations: Rc<Mutations>,
    confirmed: Cell<bool>,
}
#[async_trait(?Send)]
impl ArrangeHost for ArrangementSession {
    fn tempo(&self) -> Option<f64> {
        self.mutations.observer.tempo.get()
    }
    fn beats_per_bar(&self) -> f64 {
        self.mutations.observer.beats_per_bar.get()
    }
    async fn read(&self, kind: &str, extra: JsonObject, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        Ok(self.mutations.parameters.history.connection.rows(kind, extra, signal).await?)
    }
    fn offers(&self, tool: &str) -> bool {
        let connection = &self.mutations.parameters.history.connection;
        CHANGES
            .iter()
            .find(|kind| kind.tool == tool)
            .is_some_and(|kind| self.mutations.supported(kind.since.as_deref()) && kind.available(|tool| connection.has(tool)))
    }
    async fn change(&self, tool: &str, input: JsonObject, signal: Signal) -> Result<Made, RuntimeError> {
        let kind = CHANGES.iter().find(|kind| kind.tool == tool).ok_or_else(|| RuntimeError::plain("Unknown Live change"))?;
        let outcome = self.mutations.change(kind, input, signal, self.confirmed.get()).await;
        let reply: Value = serde_json::from_str(&outcome.text).unwrap_or(json!({}));
        let id = reply.get("change").and_then(Value::as_str);
        if outcome.is_error || id.is_none() {
            let changed = reply.get("changed").and_then(Value::as_str).map(|s| format!("{s}: ")).unwrap_or_default();
            return Err(ObservationError(format!("{changed}{}", head(&outcome.text, 400))).into());
        }
        self.confirmed.set(true);
        Ok(Made { id: id.unwrap().into(), reference: reply.get("ref").and_then(Value::as_str).map(str::to_owned) })
    }
    async fn undo(&self, id: &str, signal: Signal) -> Result<bool, RuntimeError> {
        Ok(!self.mutations.parameters.history.undo(id, signal, false).await?.is_error)
    }
    async fn quietly(&self, work: LocalBoxFuture<'_, Result<Built, RuntimeError>>) -> Result<QuietBuilt, RuntimeError> {
        let mut ids = Vec::new();
        let value = self.mutations.parameters.history.quietly(Some(&mut ids), work).await?;
        Ok(QuietBuilt { value, ids })
    }
    fn record(&self, title: &str, ids: &[String], apart: &[String]) -> Option<String> {
        self.mutations.parameters.history.grouped(title, ids, apart)
    }
    async fn undo_step(&self) -> Result<UndoStep, RuntimeError> {
        let connection = self.mutations.parameters.history.connection.clone();
        if !connection.has("live_undo_step_begin") || !connection.has("live_undo_step_end") {
            return Ok(UndoStep { opened: false, close: Box::new(|| async { Ok(()) }.boxed_local()) });
        }
        let lasting = || abort::any([connection.lifetime.clone(), abort::timeout(10_000)]);
        let opened = connection.call("live_undo_step_begin", object(&json!({"label":"Kumi","timeoutMs":600_000}))?, lasting()).await;
        let id = opened.ok().and_then(|r| payload(&r).ok()).and_then(|p| p.get("stepId").and_then(Value::as_str).map(str::to_owned));
        Ok(UndoStep {
            opened: id.is_some(),
            close: Box::new(move || {
                async move {
                    if let Some(id) = id.filter(|s| !s.is_empty()) {
                        let signal = abort::any([connection.lifetime.clone(), abort::timeout(10_000)]);
                        let _ = connection.call("live_undo_step_end", object(&json!({"stepId":id}))?, signal).await;
                    }
                    Ok(())
                }
                .boxed_local()
            }),
        })
    }
    async fn keep_copy(&self, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.mutations.keep_copy(signal).await
    }
    fn tell(&self, title: &str) {
        self.mutations.emit_action(&ActionSummary { title: title.into(), playing: None, recording: None });
    }
}
