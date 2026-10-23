//! The doctor's model: a check (fine, a note, or something to fix, and what to do next), and what its look through a
//! DAW's bridge found. Each DAW's checks are these; the app's doctor groups, writes and counts them.
use futures::future::LocalBoxFuture;
use kumi_types::{daw::DawId, errors::RuntimeError};
use serde::{Deserialize, Serialize};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Note,
    Fix,
}

impl CheckStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Ok => "ok",
            Self::Note => "note",
            Self::Fix => "fix",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub status: CheckStatus,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl Check {
    pub fn ok(text: impl Into<String>) -> Self {
        Self { status: CheckStatus::Ok, text: text.into(), next: None }
    }
    pub fn note(text: impl Into<String>, next: Option<String>) -> Self {
        Self { status: CheckStatus::Note, text: text.into(), next }
    }
    pub fn fix(text: impl Into<String>, next: Option<String>) -> Self {
        Self { status: CheckStatus::Fix, text: text.into(), next }
    }
}

/// What the doctor's look through a DAW's bridge found: whether the DAW's integration started (and what it said when it
/// didn't), whether it reached the DAW, and what the DAW said of itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_live: Option<bool>,
    /// Why the bridge didn't start, in Kumi's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
}

/// The doctor's look through each DAW's bridge (on a bridge configuration), as an answer a DAW's checks read: the
/// app's (each DAW's own probe), or a test's. Without it, the checks look through no bridge.
#[derive(Clone)]
pub struct Probes(pub Rc<dyn Fn(DawId, String) -> LocalBoxFuture<'static, Result<Probe, RuntimeError>>>);

/// The version of the bridge this Kumi brings for each DAW, which the doctor compares the installed one with, as an
/// answer a DAW's checks read: the app's (each DAW's provider), or a test's. Without it, nothing is compared.
#[derive(Clone)]
pub struct Bundled(pub Rc<dyn Fn(DawId) -> Option<String>>);
