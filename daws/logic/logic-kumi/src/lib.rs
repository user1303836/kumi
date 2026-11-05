//! Kumi's Logic Pro bridge. Logic has no plug-in API, so the bridge is a process beside Kumi: it reaches Logic through
//! the `kumi-logic-hands` helper, a child speaking JSON lines that holds the macOS channels (Accessibility, key events,
//! MIDI), and serves Kumi's bridge wire (`kumi-daw/1`, daw-bridge) on loopback as the REAPER extension and the Renoise
//! tool do from inside their DAWs.
//!
//! M2 adds native Accessibility readers, cached focus, snapshots and the four focus bodies. Only read methods are
//! exposed on the wire; project changes, listening and application registration are later stages.

pub mod ax;
pub mod bodies;
pub mod bridge;
pub mod fixtures;
pub mod focus;
pub mod helper;
pub mod log;
pub mod logic;
pub mod mcu;
pub mod project;
pub mod queue;
pub mod read;
pub mod refs;
pub mod rows;
pub mod scenes;
pub mod serve;
pub mod server;

pub const BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs `f`, turning a panic into `fallback` and a log line: for every thread the bridge starts, so one connection's
/// trouble never takes the process down.
pub fn guard<T>(edge: &str, fallback: T, f: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let why = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned());
            log::line(&format!("panic in {edge}: {}", why.unwrap_or_default()));
            fallback
        }
    }
}
