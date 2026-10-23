//! The integrations of the DAWs whose bridges are Kumi's own (REAPER's and Renoise's, and what they share), what every
//! DAW's integration shares, and the DAW-neutral ones (none, the fallback). Live's is kumi-daw-ableton's; the registry of
//! the DAWs this build integrates is the app's.
#[cfg(test)]
mod daws_doc;
pub mod fallback;
pub mod none;
pub mod own_bridge;
pub mod own_catchup;
pub mod own_changes;
mod own_ears;
mod own_goal;
mod own_groove;
pub mod own_library;
pub mod own_listening;
pub mod own_phrases;
pub mod own_pictures;
mod own_scripts;
mod own_spec;
pub mod own_views;
#[cfg(test)]
mod pack_tests;
pub mod reaper;
pub mod reaper_changes;
mod reaper_delivery;
mod reaper_routing;
mod reaper_variations;
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
