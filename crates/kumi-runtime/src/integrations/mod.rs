//! The DAWs Kumi integrates, and what their integrations share. Outside Live's own integration, only this registry
//! and the crate root name it: the crate root's constructors for Live come through here.
pub mod ableton;
#[cfg(test)]
mod daws_doc;
pub mod fallback;
pub mod none;
pub mod own_bridge;
pub mod own_catchup;
pub mod own_changes;
mod own_goal;
mod own_groove;
pub mod own_listening;
pub mod own_phrases;
pub mod own_pictures;
mod own_scripts;
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
mod renoise_lines;
mod renoise_mapping;
mod renoise_variations;
pub mod shared;

use crate::core::daw::{DawId, DawProfile};
use crate::core::presentation::ToolPresentation;
use crate::core::view::ViewPresentation;

pub use crate::core::project::{create_project_store, Baseline, ProjectStore};
pub use ableton::devices as live_devices;
pub use ableton::focus::parse_focus;
pub use ableton::live_set;
pub use ableton::manual as live_manual;
pub use ableton::rendering::main_chain as live_main_chain;
pub use ableton::{create_ableton_integration, create_inference_only_integration, BRIDGE_TOOLS};
pub use ableton::{ears, hands};

/// The DAWs this build integrates, by profile: Live, and the DAWs whose bridges are Kumi's own.
pub const REGISTERED: &[&DawProfile] = &[&DawProfile::ABLETON, &DawProfile::REAPER, &DawProfile::RENOISE];

/// The places a store takes: every registered DAW's.
pub const PLACES: crate::core::place::Places = crate::core::place::Places(&[DawId::Ableton, DawId::Reaper, DawId::Renoise]);

/// A registered DAW's profile; None for one this build doesn't integrate.
pub fn profile(id: DawId) -> Option<&'static DawProfile> {
    REGISTERED.iter().copied().find(|profile| profile.id == Some(id))
}

/// How the app shows a registered DAW's own tools.
pub fn presentation(id: DawId) -> Option<&'static ToolPresentation> {
    match id {
        DawId::Ableton => Some(&ableton::presentation::PRESENTATION),
        DawId::Reaper => Some(&reaper::PRESENTATION),
        DawId::Renoise => Some(&renoise::PRESENTATION),
    }
}

/// How the app reads a registered DAW's focus and devices.
pub fn view(id: DawId) -> Option<&'static ViewPresentation> {
    match id {
        DawId::Ableton => Some(&ableton::presentation::VIEW),
        DawId::Reaper => Some(&reaper::VIEW),
        DawId::Renoise => Some(&renoise::VIEW),
    }
}

/// A registered DAW's manual as a tool, for a DAW whose manual Kumi reads (Live's).
pub fn manual_tool(id: DawId, options: live_manual::ManualOptions) -> Option<std::rc::Rc<dyn crate::core::contracts::KernelTool>> {
    match id {
        DawId::Ableton => Some(live_manual::manual_tool(options)),
        _ => None,
    }
}

/// Where the DAW keeps the producer's sounds, presets and saved projects, beside the folders named: Live's layout (its
/// User Library, Places, packs and Core Library) and Splice's folder. Live is the only DAW registered; another DAW's
/// places come with it.
pub fn library_sources(options: &crate::library::sources::SourceOptions) -> Vec<crate::library::sources::Source> {
    debug_assert!(profile(options.daw.unwrap_or(DawId::Ableton)).is_some(), "a registered DAW's places");
    crate::library::sources::library_sources(options)
}

/// The DAW's saved projects it opened lately (Live's Sets, from its log).
pub fn recent_projects(options: &crate::library::sources::SourceOptions) -> Vec<String> {
    debug_assert!(profile(options.daw.unwrap_or(DawId::Ableton)).is_some(), "a registered DAW's projects");
    crate::library::sources::recent_sets(options)
}

/// REAPER's and Renoise's instructions, packs included, keep within the budget Live's instructions keep. (Here, because
/// only the registry may name Live's integration outside it.)
#[cfg(test)]
#[test]
fn each_own_daws_instructions_keep_within_lives() {
    let live = ableton::context::INSTRUCTIONS.len();
    for daw in [&reaper::DAW, &renoise::DAW] {
        let text = own_bridge::instructions_of(daw);
        assert!(text.len() <= live, "{}'s instructions are {} bytes, over Live's {live}", daw.profile.words.app, text.len());
    }
}
