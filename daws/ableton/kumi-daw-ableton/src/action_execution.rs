//! Guarded transport, recording, and view actions, including recording cleanup.
use super::{
    actions::{ActionKind, ActionSummary},
    changes::CHANGES,
    connection::{ReadError, NO_CURRENT_LIVE},
    context::{self, ObservationError},
    history::uncertain,
    mutations::Mutations,
    views::ViewHost,
};
use kumi_agent::core::contracts::ActionEvent;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, string::head},
};
use kumi_library::sources::homedir;
use kumi_types::contracts::JsonObject;
use kumi_types::disk;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
};

pub use kumi_agent::core::plan::ActionOutcome;
impl Mutations {
    pub fn emit_action(&self, done: &ActionSummary) {
        if let Some(listener) = &self.options.on_action {
            let event = ActionEvent { title: done.title.clone(), playing: done.playing, recording: done.recording };
            let _ = catch_unwind(AssertUnwindSafe(|| listener(event)));
        }
    }
    pub async fn act(&self, kind: &ActionKind, named: JsonObject, original: Signal, cleanup: bool) -> ActionOutcome {
        let mut disarmed = Vec::new();
        if kind.tool == "record" && named.get("action").and_then(Value::as_str) == Some("start") && !cleanup {
            if let Some(destination) = named.get("destinationTrackRef").and_then(Value::as_str) {
                let mut keep = vec![destination.to_owned()];
                keep.extend(
                    named.get("alsoTrackRefs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned),
                );
                disarmed = match self.disarm_others(&keep, original.clone()).await {
                    Ok(disarmed) => disarmed,
                    Err(text) => return ActionOutcome::error(text),
                };
            }
        }
        let mut outcome = self.act_once(kind, &named, original.clone(), cleanup).await;
        if !disarmed.is_empty() && !outcome.is_error {
            if let Ok(Value::Object(mut reply)) = serde_json::from_str(&outcome.text) {
                reply.insert("disarmedFirst".into(), json!(disarmed));
                outcome.text = stringify(&json!(reply));
            }
            if let Some(done) = &mut outcome.done {
                done.title.push_str(&format!(", after disarming {}", disarmed.join(", ")));
            }
        }
        let stopping = matches!(kind.tool.as_str(), "play" | "record") && named.get("action").and_then(Value::as_str) == Some("stop");
        if !outcome.is_error || !stopping || original.is_cancelled() {
            return outcome;
        }
        let history = &self.parameters.history;
        if !history.stop_everything(history.change_signal()).await {
            return outcome;
        }
        let done = ActionSummary {
            title: if kind.tool == "record" { "Recording stopped" } else { "Stopped" }.into(),
            playing: Some(false),
            recording: (kind.tool == "record").then_some(false),
        };
        self.emit_action(&done);
        ActionOutcome {
            text: stringify(
                &json!({"done":done.title,"note":"Live's ordinary stop was refused, so Kumi stopped clips, the transport and recording together."}),
            ),
            is_error: false,
            maybe: None,
            done: Some(done),
        }
    }
    async fn disarm_others(&self, keep: &[String], signal: Signal) -> Result<Vec<String>, String> {
        let connection = &self.parameters.history.connection;
        let kept: HashSet<_> = keep
            .iter()
            .flat_map(|reference| {
                [reference.clone(), connection.references.borrow().lengthen(&json!(reference)).as_str().unwrap_or(reference).to_owned()]
            })
            .collect();
        let read = connection
            .invoke(
                "live_discover",
                object(json!({"kind":"track","fields":["ref","name","armed"],"limit":connection.page_limit()})),
                signal.clone(),
            )
            .await;
        if read.is_error {
            return Ok(Vec::new());
        }
        let Ok(read) = serde_json::from_str::<Value>(&read.text) else { return Ok(Vec::new()) };
        let routing = CHANGES.iter().find(|kind| kind.tool == "set_routing");
        let mut disarmed = Vec::new();
        for item in read["live"]["items"].as_array().into_iter().flatten() {
            let Some(reference) = item.get("ref").and_then(Value::as_str).filter(|r| !kept.contains(*r)) else { continue };
            if item.get("armed") != Some(&json!(true)) {
                continue;
            }
            let name = item.get("name").and_then(Value::as_str).map(|s| head(s, 80)).unwrap_or_else(|| "a track".into());
            let outcome = match routing {
                Some(routing) => self.change(routing, object(json!({"trackRef":reference,"arm":false})), signal.clone(), false).await,
                None => super::parameters::ChangeOutcome::error("Live doesn't offer disarming here"),
            };
            if outcome.is_error {
                return Err(format!(
                    "{name} is armed too, and Live records onto one armed track only; Kumi couldn't disarm it: {}",
                    head(&outcome.text, 200)
                ));
            }
            disarmed.push(name);
        }
        Ok(disarmed)
    }
    async fn act_once(&self, kind: &ActionKind, named: &JsonObject, original: Signal, cleanup: bool) -> ActionOutcome {
        match self.try_act_once(kind, named, original, cleanup).await {
            Ok(outcome) => outcome,
            Err(ReadError::Observation(error)) => ActionOutcome::error(error.0),
            Err(_) => ActionOutcome::error("That didn't happen in Live; discover again, then retry."),
        }
    }
    async fn try_act_once(
        &self,
        kind: &ActionKind,
        named: &JsonObject,
        original: Signal,
        cleanup: bool,
    ) -> Result<ActionOutcome, ReadError> {
        let history = &self.parameters.history;
        let connection = &history.connection;
        let signal = abort::any([original, connection.lifetime.clone()]);
        let input = context::object(&connection.references.borrow().lengthen(&json!(named)))?;
        let lease = connection.lease.get();
        signal.check()?;
        if !connection.available.get() || connection.lost.get() || connection.epoch.get().is_none() || connection.tools().is_none() {
            return Err(observation(NO_CURRENT_LIVE));
        }
        connection.ensure_catalog(signal.clone()).await?;
        if !cleanup {
            connection.assert_lease(lease, &signal)?;
        }
        if !connection.has(&kind.preview) || !connection.has(&kind.apply) {
            return Err(observation("Live doesn't offer that for the open Set right now"));
        }
        if !cleanup && !self.supported(kind.since.as_deref()) {
            return Err(observation(&self.too_old(kind.since.as_deref())));
        }
        // `newer` is keyed only by known action strings; other input shapes have no entry.
        if let Some(since) = input.get("action").and_then(Value::as_str).and_then(|action| kind.newer.as_ref()?.get(action)?.as_str()) {
            if !cleanup && !self.supported(Some(since)) {
                return Err(observation(&self.too_old(Some(since))));
            }
        }
        if !cleanup {
            connection.references.borrow().require_fresh_references(&input)?;
        }
        let prepared = match kind.prepare(&input) {
            Ok(input) => input,
            Err(text) => return Ok(ActionOutcome::error(text)),
        };
        if kind.tool == "record" && input.get("action").and_then(Value::as_str) == Some("start") && !cleanup {
            let folder = history
                .remember
                .current()
                .and_then(|project| {
                    project
                        .path
                        .as_ref()
                        .filter(|s| !s.is_empty())
                        .map(|p| Path::new(p).parent().unwrap_or(Path::new(".")).to_string_lossy().into_owned())
                })
                .unwrap_or_else(homedir);
            let full = if let Some(low_disk) = &self.options.low_disk {
                low_disk(folder, 100.0 * disk::MB, "Live records to".into()).await
            } else {
                disk::low_disk(&folder, 100.0 * disk::MB, "Live records to").await
            };
            if let Some(full) = full.filter(|s| !s.is_empty()) {
                return Ok(ActionOutcome::error(format!("{full} Nothing was recorded.")));
            }
        }
        let previewed = connection.call(&kind.preview, prepared.clone(), signal.clone()).await?;
        if !cleanup {
            connection.assert_lease(lease, &signal)?;
        }
        if previewed.is_error == Some(true) {
            return Ok(ActionOutcome::error(stringify(&serde_json::to_value(previewed).unwrap())));
        }
        let preview = context::payload(&previewed)?;
        let transaction = preview.get("transactionId").and_then(Value::as_str).filter(|s| !s.is_empty());
        let confirmation = preview.get("confirmation").and_then(Value::as_str).filter(|s| !s.is_empty());
        let (Some(transaction), Some(confirmation)) = (transaction, confirmation) else {
            return Err(observation("The bridge's preview was malformed; nothing happened"));
        };
        let known = |reference: &Value| reference.as_str().and_then(|r| connection.references.borrow().known.get(r).cloned());
        let unsure = ActionOutcome {
            text: "Live didn't confirm this, so it may have happened: check Live (/stop stops it) before trying again.".into(),
            is_error: true,
            maybe: Some(true),
            done: Some(kind.summarize(&preview, &prepared, &known)),
        };
        let applied = match connection
            .call(
                &kind.apply,
                object(json!({"transactionId":transaction,"confirmation":confirmation,"idempotencyKey":uuid::Uuid::new_v4().to_string()})),
                history.change_signal(),
            )
            .await
        {
            Ok(applied) => applied,
            Err(_) => return Ok(unsure),
        };
        if applied.is_error == Some(true) {
            return Ok(if uncertain(&applied) { unsure } else { ActionOutcome::error(stringify(&serde_json::to_value(applied).unwrap())) });
        }
        let done = kind.summarize(&preview, &prepared, &known);
        if !history.is_quiet() {
            self.emit_action(&done);
        }
        Ok(ActionOutcome {
            text: stringify(
                &json!({"done":done.title,"live":connection.references.borrow_mut().shorten(&json!(context::payload(&applied)?))}),
            ),
            is_error: false,
            maybe: None,
            done: Some(done),
        })
    }
}
fn observation(message: &str) -> ReadError {
    ReadError::Observation(ObservationError(message.into()))
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
