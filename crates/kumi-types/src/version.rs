//! Kumi's release, as providers and the bridges see it: each program Kumi ships sets it first thing, from its own
//! package's version, which a release moves. This crate's number stays put, so a release rebuilds only the programs,
//! not every crate above this one.
use std::sync::OnceLock;

/// What [`kumi`] says before the program sets it (a crate's own tests): no release is numbered so.
pub const UNSET: &str = "0.0.0-unset";

static KUMI: OnceLock<&'static str> = OnceLock::new();

/// Sets Kumi's release for this process: the first thing each program Kumi ships does. A second call changes nothing.
pub fn set(version: &'static str) {
    let _ = KUMI.set(version);
}

/// Kumi's release, as the program set it; [`UNSET`] before.
pub fn kumi() -> &'static str {
    KUMI.get().copied().unwrap_or(UNSET)
}
