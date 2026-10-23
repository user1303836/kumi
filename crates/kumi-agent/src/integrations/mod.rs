//! What every DAW's integration shares, and the DAW-neutral ones (none, the fallback). Live's is kumi-daw-ableton's,
//! REAPER's kumi-daw-reaper's and Renoise's kumi-daw-renoise's, and what the two of Kumi's own bridges share is
//! kumi-daw-own's; the registry of the DAWs this build integrates is the app's.
pub mod fallback;
pub mod none;
pub mod shared;

pub use crate::core::project::{create_project_store, Baseline, ProjectStore};
