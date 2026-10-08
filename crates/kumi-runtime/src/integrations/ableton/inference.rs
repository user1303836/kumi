//! The explicit inference-only integration, including observations while Live is away.
use crate::core::{contracts::*, errors::RuntimeError};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use serde_json::json;
use std::{cell::Cell, rc::Rc};
pub(crate) fn no_access(key: &str, now: DateTime<Utc>, project: Option<ProjectRef>) -> Observation {
    Observation {
        key: key.into(),
        revision: Some("no-live".into()),
        label: "Inference-only — No Live access".into(),
        instructions: super::context::INSTRUCTIONS.into(),
        tools: vec![],
        project,
        set: None,
        tracks: None,
        saved_at: None,
        context: stringify(
            &json!({"observedAt":now.to_rfc3339_opts(SecondsFormat::Millis,true),"mode":"inference-only","access":"No Live access; do not describe remembered Set data as current. Kumi reconnects on its own when Live is back, and the conversation carries on."}),
        ),
    }
}
struct InferenceOnly {
    closed: Cell<bool>,
    on_connection: Rc<dyn Fn(ConnectionState)>,
}
pub fn create_inference_only_integration(on_connection: Rc<dyn Fn(ConnectionState)>) -> Rc<dyn Integration> {
    Rc::new(InferenceOnly { closed: Cell::new(false), on_connection })
}
#[async_trait(?Send)]
impl Integration for InferenceOnly {
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        signal.check()?;
        if self.closed.get() {
            return Err(RuntimeError::plain("Integration is closed"));
        }
        (self.on_connection)(ConnectionState::Disconnected);
        Ok(())
    }
    async fn observe(&self, signal: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        signal.check()?;
        if self.closed.get() {
            return Err(RuntimeError::plain("Integration is closed"));
        }
        Ok(no_access("inference-only", Utc::now(), None))
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closed.set(true);
        Ok(())
    }
}
