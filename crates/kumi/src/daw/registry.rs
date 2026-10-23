//! The DAW table: the DAWs this build integrates, each by its provider, which says what the app and the library take
//! from it (its profile, how its own tools, its focus and its devices show, the places its conversations and notes are
//! kept in, where it keeps the producer's library, where it's downloaded from) and finds, installs and checks Kumi's
//! bridge in it. The DAWs' own crates (Live's kumi-daw-ableton, and the integrations of the DAWs whose bridges are
//! Kumi's own) don't name each other; this table names them all.

use kumi_agent::core::presentation::ToolPresentation;
use kumi_agent::core::view::ViewPresentation;
use kumi_daw::provider::DawProvider;
use kumi_daw_ableton::app::provider::Ableton;
use kumi_daw_reaper as reaper;
use kumi_daw_renoise as renoise;
use kumi_library::daws::{Daws, LibrarySources};
use kumi_library::sources::{Source, SourceOptions};
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::place::Places;
use std::sync::LazyLock;

/// The DAWs this build integrates, Live's first, then the DAWs whose bridges are Kumi's own.
pub static DAWS: &[&(dyn DawProvider + Sync)] = &[&Ableton, &reaper::PROVIDER, &renoise::PROVIDER];

/// The DAWs this build integrates, by profile.
pub fn registered() -> impl Iterator<Item = &'static DawProfile> {
    DAWS.iter().map(|daw| daw.profile())
}

/// The places a store takes: every registered DAW's.
pub fn places() -> Places {
    static IDS: LazyLock<Vec<DawId>> = LazyLock::new(|| registered().filter_map(|profile| profile.id).collect());
    let ids: &'static [DawId] = &IDS;
    Places(ids)
}

/// A registered DAW's profile; None for one this build doesn't integrate.
pub fn profile(id: DawId) -> Option<&'static DawProfile> {
    registered().find(|profile| profile.id == Some(id))
}

/// How the app shows a registered DAW's own tools.
pub fn presentation(id: DawId) -> Option<&'static ToolPresentation> {
    super::provider(id).map(|daw| daw.presentation())
}

/// How the app reads a registered DAW's focus and devices.
pub fn view(id: DawId) -> Option<&'static ViewPresentation> {
    super::provider(id).map(|daw| daw.view())
}

/// What each registered DAW tells the library (its places, its own library, the projects it opened lately, its
/// manual), in the table's order.
pub fn libraries() -> Daws {
    static LIBRARIES: LazyLock<Vec<&'static dyn LibrarySources>> = LazyLock::new(|| DAWS.iter().map(|daw| daw.library()).collect());
    let libraries: &'static [&'static dyn LibrarySources] = &LIBRARIES;
    Daws(libraries)
}

/// Where the DAWs keep the producer's sounds, presets and saved projects, beside the folders named: Live's layout (its
/// User Library, Places, packs and Core Library), Splice's folder, and REAPER's and Renoise's own libraries.
pub fn library_sources(options: &SourceOptions) -> Vec<Source> {
    debug_assert!(profile(options.daw.unwrap_or(DawId::Ableton)).is_some(), "a registered DAW's places");
    kumi_library::sources::library_sources(libraries(), options)
}

/// The DAWs' saved projects they opened lately (Live's Sets, from its log).
pub fn recent_projects(options: &SourceOptions) -> Vec<String> {
    debug_assert!(profile(options.daw.unwrap_or(DawId::Ableton)).is_some(), "a registered DAW's projects");
    kumi_library::sources::recent_projects(libraries(), options)
}

/// REAPER's and Renoise's instructions, packs included, keep within the budget Live's instructions keep. (Here, because
/// only the registry names Live's integration beside the others.)
#[cfg(test)]
#[test]
fn each_own_daws_instructions_keep_within_lives() {
    let live = kumi_daw_ableton::context::INSTRUCTIONS.len();
    for daw in [&reaper::DAW, &renoise::DAW] {
        let text = kumi_daw_own::bridge::instructions_of(daw);
        assert!(text.len() <= live, "{}'s instructions are {} bytes, over Live's {live}", daw.profile.words.app, text.len());
    }
}
