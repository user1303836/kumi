//! Live 12.4's modulators (LFO, Shaper, Envelope Follower, Expression Control) mapped to a parameter as a typed change,
//! in one trip of Kumi's own Python. A plan loads a modulator and maps it in one go: the model used to find
//! map_modulation for itself with run_python, a model call or three later, with no HISTORY row and no undo (#264).
use super::{
    changes::{new_record, ChangeKind},
    connection::ReadError,
    context,
    fast::with_args,
    history::FastResult,
    mutations::Mutations,
    parameters::ChangeOutcome,
};
use crate::core::contracts::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const MAP_MODULATOR: &str = "map_modulator";
/// Max for Live starts its engine with the first such device a session loads, and until it has, a modulator can't map:
/// tried again for this long.
const READY: Duration = Duration::from_secs(8);
/// What Live says of a modulator Max hasn't started yet.
const NOT_READY: &str = "Max bridge is not initialized";

impl Mutations {
    pub(super) async fn map_modulator(&self, kind: &ChangeKind, input: &JsonObject, signal: Signal) -> Result<ChangeOutcome, ReadError> {
        let history = &self.parameters.history;
        let connection = &history.connection;
        let clear = input.get("clear") == Some(&Value::Bool(true));
        let named = input.get("parameterRef").is_some() || (input.get("targetRef").is_some() && input.get("parameter").is_some());
        if !clear && !named {
            return Ok(ChangeOutcome::error(
                "map_modulator takes the parameter to modulate: parameterRef, or targetRef (its device) with parameter (its name); or clear: true to empty the slot",
            ));
        }
        let slot = input.get("slot").and_then(Value::as_u64).unwrap_or(0);
        let args = json!({
            "device": input.get("deviceRef"),
            "slot": slot,
            "clear": clear,
            "parameterRef": input.get("parameterRef"),
            "target": input.get("targetRef"),
            "parameter": input.get("parameter"),
        });
        let script = with_args("map-modulator", &args, include_str!("assets/map-modulator.py"));
        let started = Instant::now();
        history.changes_this_turn.set(history.changes_this_turn.get() + 1);
        let result = loop {
            signal.check()?;
            match history.run_fast(script.clone(), history.change_signal()).await? {
                FastResult::Result(value) => break context::object(&value)?,
                // Refused before anything changed: a modulator Live loaded a moment ago waits for Max.
                FastResult::Error { error, sent: false } if error.contains(NOT_READY) && started.elapsed() < READY => {
                    // Stopped meanwhile, the loop's check says so.
                    tokio::select! { _ = signal.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(250)) => {} }
                }
                FastResult::Error { error, sent: false } => {
                    let error = if error.contains(NOT_READY) {
                        "Max for Live didn't start in Live in time, so the modulator can't be mapped yet: try again in a moment".to_owned()
                    } else {
                        error
                    };
                    return Ok(ChangeOutcome::error(error));
                }
                FastResult::Error { error, sent: true } => {
                    history.remember(
                        new_record(
                            kind,
                            super::changes::ChangeSummary::title("Mapped a modulator (unconfirmed)"),
                            ChangeState::Unsure,
                            connection.now().timestamp_millis(),
                        ),
                        String::new(),
                        None,
                    );
                    return Ok(ChangeOutcome::stop(format!(
                        "Live didn't confirm the mapping ({error}), so it may or may not have happened. Tell the producer to check Live."
                    )));
                }
            }
        };
        let known = |reference: &Value| reference.as_str().and_then(|r| connection.references.borrow().known.get(r).cloned());
        let summary = kind.summarize(&JsonObject::new(), input, &known, Some(&result));
        let now = result.get("now").filter(|now| now.is_object());
        let prior = result.get("prior").filter(|prior| prior.is_object());
        // Kumi's undo empties a slot it filled; what a slot held before isn't Kumi's to put back.
        let revertible = now.is_some() && prior.is_none();
        let mut record = new_record(
            kind,
            summary,
            if revertible || (now.is_none() && prior.is_none()) { ChangeState::Applied } else { ChangeState::Kept },
            connection.now().timestamp_millis(),
        );
        if record.state == ChangeState::Kept {
            record.note = Some("Kumi can't put back what this slot was mapped to before; Live's own undo (Cmd-Z in Live) can.".into());
        }
        history.remember(record.clone(), String::new(), None);
        if revertible {
            let now = now.unwrap();
            let revert = json!({
                "kind": "modulation",
                "device": args["device"],
                "slot": slot,
                "name": result.get("modulator"),
                "applied": now.get("identity"),
                "appliedName": now.get("name"),
            });
            if let Some(entry) = history.entries.borrow().get(&record.id) {
                entry.borrow_mut().revert = Some(vec![revert]);
            }
        }
        let mut reply = context::object(&json!({"changed":record.title,"change":record.id,"state":record.state}))?;
        if let Some(note) = &record.note {
            reply.insert("note".into(), json!(note));
        }
        let mut live = result.clone();
        live.remove("track");
        reply.insert("live".into(), connection.references.borrow_mut().shorten(&Value::Object(live)));
        Ok(ChangeOutcome { text: stringify(&Value::Object(reply)), is_error: false, missed: None, stops: false })
    }
}
