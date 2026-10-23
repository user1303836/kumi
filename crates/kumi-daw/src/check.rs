//! The doctor's model: a check (fine, a note, or something to fix, and what to do next), and what its look through a
//! DAW's bridge found. Each DAW's checks are these; the app's doctor groups, writes and counts them.
use futures::future::LocalBoxFuture;
use kumi_common::js::{
    number,
    string::{head, trim},
};
use kumi_library::sources::dirname;
use kumi_types::{daw::DawId, errors::RuntimeError};
use serde::{Deserialize, Serialize};
use std::{rc::Rc, time::Duration};

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

/// `path` as the producer reads it: their home folder as ~.
pub fn tilde(path: &str) -> String {
    let home = home::home_dir().unwrap_or_default().display().to_string();
    if !home.is_empty() {
        path.strip_prefix(&home).map(|tail| format!("~{tail}")).unwrap_or_else(|| path.into())
    } else {
        path.into()
    }
}
pub fn major(version: &str) -> f64 {
    number::parse(version.strip_prefix('v').unwrap_or(version).split('.').next().unwrap_or("")).unwrap_or(f64::NAN)
}
pub fn newer(left: &str, right: &str) -> bool {
    let a: Vec<_> = left.split('.').map(|s| number::parse(s).unwrap_or(f64::NAN)).collect();
    let b: Vec<_> = right.split('.').map(|s| number::parse(s).unwrap_or(f64::NAN)).collect();
    for i in 0..3 {
        let x = a.get(i).copied().unwrap_or(0.);
        let y = b.get(i).copied().unwrap_or(0.);
        if x != y {
            return x > y;
        }
    }
    false
}
pub fn access(path: &str, execute: bool) -> bool {
    #[cfg(unix)]
    {
        let Ok(path) = std::ffi::CString::new(path) else { return false };
        unsafe { libc::access(path.as_ptr(), if execute { libc::X_OK } else { libc::W_OK }) == 0 }
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path).is_ok_and(|m| execute || !m.permissions().readonly())
    }
}
pub async fn node_version(command: &str) -> Option<String> {
    let mut process = tokio::process::Command::new(command);
    process.arg("--version").env_clear().env("PATH", dirname(command)).kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);
    let result = tokio::time::timeout(Duration::from_secs(5), process.output()).await.ok()?.ok()?;
    if !result.status.success() {
        return None;
    }
    let text = head(trim(&String::from_utf8_lossy(&result.stdout)), 32);
    (!text.is_empty()).then_some(text)
}
