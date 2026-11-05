//! `kumi-daw/1` beside LYNX, with a mutually authenticated native control connection.
//!
//! Kumi reaches this program through its normal own-bridge MCP host. Only this
//! program reads LYNX's private discovery and speaks its native endpoint.
mod changes;
pub mod launch;
pub mod native;
pub mod project;
pub mod server;

use serde_json::Value;

/// A bounded error whose code preserves uncertain writes across the shared host.
#[derive(Debug, Clone)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}
impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(daw_bridge::wire::code::BAD_PARAMS, message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Unix milliseconds used only for request deadlines and discovery timestamps.
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |n| n.as_millis() as u64)
}

pub fn integer(value: &Value, name: &str) -> Result<u64> {
    value
        .get(name)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0. && n.fract() == 0. && *n <= daw_bridge::wire::MAX_SAFE_INTEGER as f64)
        .map(|n| n as u64)
        .ok_or_else(|| Error::bad(format!("{name} must be a nonnegative safe integer")))
}
