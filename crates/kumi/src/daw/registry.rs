//! The DAWs this build integrates, by profile, and what each tells the app and the library: how its own tools, its
//! focus and its devices show, the places its conversations and notes are kept in, and where it keeps the producer's
//! library. The DAWs' own crates (Live's kumi-daw-ableton, and the integrations of the DAWs whose bridges are Kumi's
//! own) don't name each other; this registry names them all.

use kumi_daw_ableton as ableton;
use kumi_daw_reaper as reaper;
use kumi_daw_renoise as renoise;
use kumi_runtime::core::daw::{DawId, DawProfile};
use kumi_runtime::core::place::Places;
use kumi_runtime::core::presentation::ToolPresentation;
use kumi_runtime::core::view::ViewPresentation;
use kumi_runtime::library::daws::Daws;
use kumi_runtime::library::sources::{Source, SourceOptions};

/// The DAWs this build integrates, by profile: Live, and the DAWs whose bridges are Kumi's own.
pub const REGISTERED: &[&DawProfile] = &[&DawProfile::ABLETON, &DawProfile::REAPER, &DawProfile::RENOISE];

/// The places a store takes: every registered DAW's.
pub const PLACES: Places = Places(&[DawId::Ableton, DawId::Reaper, DawId::Renoise]);

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

/// What each registered DAW tells the library (its places, its own library, the projects it opened lately, its
/// manual), in the registry's order.
pub static LIBRARIES: Daws = Daws(&[&ableton::library::LIBRARY, &reaper::library::LIBRARY, &renoise::library::LIBRARY]);

/// Where the DAWs keep the producer's sounds, presets and saved projects, beside the folders named: Live's layout (its
/// User Library, Places, packs and Core Library), Splice's folder, and REAPER's and Renoise's own libraries.
pub fn library_sources(options: &SourceOptions) -> Vec<Source> {
    debug_assert!(profile(options.daw.unwrap_or(DawId::Ableton)).is_some(), "a registered DAW's places");
    kumi_runtime::library::sources::library_sources(LIBRARIES, options)
}

/// The DAWs' saved projects they opened lately (Live's Sets, from its log).
pub fn recent_projects(options: &SourceOptions) -> Vec<String> {
    debug_assert!(profile(options.daw.unwrap_or(DawId::Ableton)).is_some(), "a registered DAW's projects");
    kumi_runtime::library::sources::recent_projects(LIBRARIES, options)
}

/// REAPER's and Renoise's instructions, packs included, keep within the budget Live's instructions keep. (Here, because
/// only the registry names Live's integration beside the others.)
#[cfg(test)]
#[test]
fn each_own_daws_instructions_keep_within_lives() {
    let live = ableton::context::INSTRUCTIONS.len();
    for daw in [&reaper::DAW, &renoise::DAW] {
        let text = kumi_daw_own::bridge::instructions_of(daw);
        assert!(text.len() <= live, "{}'s instructions are {} bytes, over Live's {live}", daw.profile.words.app, text.len());
    }
}
