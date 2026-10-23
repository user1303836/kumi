//! Running without a DAW, explicitly or while the DAW is away: the observation says there's no access, in the DAW's
//! own words, and offers no tools. A DAW's integration gives its profile and its instructions.
use crate::core::{contracts::*, errors::RuntimeError};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use serde_json::json;
use std::{cell::Cell, rc::Rc};

/// What a turn sees without access to the DAW: who it's about (`key`, `project`), and that nothing in it is current.
pub fn no_access(profile: &DawProfile, instructions: &str, key: &str, now: DateTime<Utc>, project: Option<ProjectRef>) -> Observation {
    let DawWords { app, place, .. } = profile.words;
    Observation {
        key: key.into(),
        revision: Some("no-live".into()),
        label: format!("Inference-only — No {app} access"),
        instructions: instructions.into(),
        tools: vec![],
        project,
        set: None,
        tracks: None,
        saved_at: None,
        identity: None,
        context: stringify(&json!({
            "observedAt": now.to_rfc3339_opts(SecondsFormat::Millis, true),
            "mode": "inference-only",
            "access": format!("No {app} access; do not describe remembered {place} data as current. Kumi reconnects on its own when {app} is back, and the conversation carries on."),
        })),
    }
}

struct NoDaw {
    profile: DawProfile,
    instructions: &'static str,
    closed: Cell<bool>,
    on_connection: Rc<dyn Fn(ConnectionState)>,
}

/// Kumi without the DAW: the DAW's words and instructions, nothing it can do.
pub fn create_no_daw_integration(
    profile: DawProfile,
    instructions: &'static str,
    on_connection: Rc<dyn Fn(ConnectionState)>,
) -> Rc<dyn Integration> {
    Rc::new(NoDaw { profile: profile.without_caps(), instructions, closed: Cell::new(false), on_connection })
}

#[async_trait(?Send)]
impl Integration for NoDaw {
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
        Ok(no_access(&self.profile, self.instructions, "inference-only", Utc::now(), None))
    }
    /// The DAW's words, with nothing it does: it isn't here.
    fn profile(&self) -> &DawProfile {
        &self.profile
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closed.set(true);
        Ok(())
    }
}
