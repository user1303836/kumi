use std::fmt;

use kumi_common::abort::Aborted;
use serde::{Deserialize, Serialize};

/// What failed, so an app can offer the fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    Auth,
    Billing,
    Model,
    Config,
    /// The DAW or Kumi's bridge to it (its name in JSON stays "live", as before other DAWs).
    #[serde(rename = "live")]
    Daw,
    RateLimit,
    /// A plan's usage limit, reached until it resets (ChatGPT's five-hour or weekly window).
    UsageLimit,
    Request,
    Provider,
    Network,
    Protocol,
    Output,
}

impl FailureKind {
    /// The DAW's kind by the name it had before other DAWs.
    #[allow(non_upper_case_globals)]
    pub const Live: FailureKind = FailureKind::Daw;
}

/// A DAW setup failure with a fix of its own, beyond its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupFailure {
    /// The DAW still runs the copy of Kumi's bridge it loaded before an update: restarting the DAW is the fix.
    RestartDaw,
}

/// A failure whose message Kumi wrote itself: safe to display, never a credential or raw provider
/// payload. `provider` says which provider it concerns, so an app can offer the fix (sign in there,
/// choose another model); `setup`, a DAW setup failure's own fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KumiError {
    pub kind: FailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupFailure>,
}

impl KumiError {
    pub const NAME: &'static str = "KumiError";

    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), provider: None, setup: None }
    }

    pub fn with_provider(kind: FailureKind, message: impl Into<String>, provider: impl Into<String>) -> Self {
        Self { kind, message: message.into(), provider: Some(provider.into()), setup: None }
    }

    /// The same failure, with the setup fix it has.
    pub fn with_setup(self, setup: SetupFailure) -> Self {
        Self { setup: Some(setup), ..self }
    }
}

impl fmt::Display for KumiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for KumiError {}

/// An error as the TypeScript threw it: one of Kumi's own, a plain `Error(message)`, or the abort
/// `signal.throwIfAborted()` throws. Its `Display` is `error.message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    Kumi(KumiError),
    /// `new Error(message)`: not in Kumi's own words, so apps don't show it as such.
    Plain(String),
    /// A Live observation error, whose own message may be shown by integration tools.
    Observation(String),
    /// The work was cancelled.
    Aborted,
}

impl RuntimeError {
    pub fn plain(message: impl Into<String>) -> Self {
        Self::Plain(message.into())
    }

    /// `error.message`.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// `error instanceof KumiError ? error : undefined`.
    pub fn kumi(&self) -> Option<&KumiError> {
        match self {
            Self::Kumi(error) => Some(error),
            _ => None,
        }
    }

    pub fn is_aborted(&self) -> bool {
        matches!(self, Self::Aborted)
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kumi(error) => f.write_str(&error.message),
            Self::Plain(message) | Self::Observation(message) => f.write_str(message),
            Self::Aborted => f.write_str("This operation was aborted"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<KumiError> for RuntimeError {
    fn from(error: KumiError) -> Self {
        Self::Kumi(error)
    }
}

impl From<Aborted> for RuntimeError {
    fn from(_: Aborted) -> Self {
        Self::Aborted
    }
}
