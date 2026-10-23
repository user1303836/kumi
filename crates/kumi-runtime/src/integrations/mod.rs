//! What the integrations of the DAWs whose bridges are Kumi's own share (`own_*`), what every DAW's integration shares,
//! and the DAW-neutral ones (none, the fallback). Live's is kumi-daw-ableton's, REAPER's kumi-daw-reaper's and Renoise's
//! kumi-daw-renoise's; the registry of the DAWs this build integrates is the app's.
pub mod fallback;
pub mod none;
pub mod own_bridge;
pub mod own_catchup;
pub mod own_changes;
pub mod own_ears;
mod own_goal;
mod own_groove;
pub mod own_library;
pub mod own_listening;
pub mod own_phrases;
pub mod own_pictures;
mod own_scripts;
pub mod own_spec;
pub mod own_views;
pub mod shared;

pub use crate::core::project::{create_project_store, Baseline, ProjectStore};
