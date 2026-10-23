//! Scripts kept for a repeated chore (#401), built once for every DAW: a chunk written against the live session and run once
//! as one step of the DAW's undo, then, once the producer says yes, kept to run again by name (`store`: Kumi's own copy) and
//! installed as the DAW's own action (the backend's: REAPER's Action List, Renoise's Tools menu). `lint` says what would stop
//! it working in another project; the tool (`tool`) keeps, lists, runs and removes them.
pub mod backend;
pub mod lint;
pub mod store;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;
