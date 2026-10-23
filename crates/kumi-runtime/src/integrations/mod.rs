//! What the integrations of the DAWs whose bridges are Kumi's own share (`own_*`), Renoise's integration, what every
//! DAW's integration shares, and the DAW-neutral ones (none, the fallback). Live's is kumi-daw-ableton's and REAPER's
//! kumi-daw-reaper's; the registry of the DAWs this build integrates is the app's.
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
pub mod renoise;
pub mod renoise_changes;
mod renoise_chop;
mod renoise_jam;
mod renoise_layers;
mod renoise_lines;
mod renoise_mapping;
mod renoise_variations;
pub mod shared;

pub use crate::core::project::{create_project_store, Baseline, ProjectStore};
