//! The DAW table: the DAWs this build integrates, each by its provider, which says what the app and the library take
//! from it (its profile, how its own tools, its focus and its devices show, the places its conversations and notes are
//! kept in, where it keeps the producer's library, where it's downloaded from) and finds, installs and checks Kumi's
//! bridge in it. The DAWs' own crates (Live's kumi-daw-ableton, and the integrations of the DAWs whose bridges are
//! Kumi's own) don't name each other; this table names them all.

use super::ableton::provider::Ableton;
use super::own::{self, Own};
use daw_bridge::{
    daw::Daw,
    detect::{self, Os},
    install::{reaper_extension_name, reaper_install, reaper_target, renoise_target, renoise_targets, unloadable, RENOISE_TOOL_FOLDER},
};
use kumi_agent::core::presentation::ToolPresentation;
use kumi_agent::core::view::ViewPresentation;
use kumi_daw::provider::{DawProvider, WatchWords};
use kumi_daw_reaper as reaper;
use kumi_daw_renoise as renoise;
use kumi_library::daws::{Daws, LibrarySources};
use kumi_library::sources::{Source, SourceOptions};
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::place::Places;
use kumi_types::system::Env;
use std::path::PathBuf;
use std::sync::LazyLock;

/// The DAWs this build integrates, Live's first, then the DAWs whose bridges are Kumi's own.
pub static DAWS: &[&(dyn DawProvider + Sync)] = &[&Ableton, &REAPER, &RENOISE];

/// REAPER, through Kumi's own bridge: its extension, a file in the UserPlugins folder of the REAPER it goes in.
pub static REAPER: Own = Own {
    id: DawId::Reaper,
    daw: Daw::Reaper,
    profile: &DawProfile::REAPER,
    presentation: &reaper::PRESENTATION,
    view: &reaper::VIEW,
    library: &reaper::library::LIBRARY,
    site: "reaper.fm",
    bridge: "Kumi's extension",
    bridge_is_folder: false,
    bridge_folder: "bridges/reaper",
    create: reaper::create_reaper_integration,
    instructions: "REAPER isn't connected, so Kumi can't see your project: answer from what you know, and say so.",
    not_found: "REAPER isn't installed (or not where Kumi looks: set KUMI_REAPER_RESOURCE_DIR to its resource folder)",
    watch_words: WatchWords {
        starting: "REAPER is open; Kumi connects once its extension answers.",
        unreachable: "Kumi couldn't reach REAPER, so this is chat without it. Open REAPER with Kumi's extension installed (kumi --daw reaper bridge); Kumi connects by itself once it answers.",
        answered: "REAPER answered, so Kumi is connected to it now.",
    },
    removal_question: "take Kumi's extension out of REAPER too",
    checkout_build: Some(("reaper-kumi", "Building REAPER's extension…")),
    carried: || reaper_extension_name(Os::current()),
    built: |beside| {
        let name = reaper_extension_name(Os::current());
        Some(beside.join(if cfg!(windows) { name.to_string() } else { format!("lib{name}") }))
    },
    version_file: |source| source.with_extension("version"),
    placed: |own, env| reaper_target(reaper_install(&reapers(own, env))?, Os::current()).map_err(Some),
    elsewhere: |_| vec![],
    unloadable: |own, env, source| reaper_install(&reapers(own, env)).ok().and_then(|install| unloadable(source, install)),
    used: |own, env| reapers(own, env).iter().any(|install| install.opened),
    here: |own, env| !reapers(own, env).is_empty(),
    // The REAPER Kumi's extension goes in.
    app: |own, env| {
        let installs = reapers(own, env);
        let chosen = reaper_install(&installs).ok().and_then(|install| install.app.clone());
        chosen.or_else(|| installs.into_iter().find_map(|install| install.app))
    },
    app_version: |own, env, is| {
        reapers(own, env).into_iter().filter(|install| install.app.as_deref().is_some_and(is)).find_map(|install| install.version)
    },
};

/// The REAPERs here, and the one Kumi's extension was put in (a portable one Kumi doesn't find by itself).
fn reapers(own: &Own, env: &Env) -> Vec<detect::ReaperInstall> {
    let placed = own.receipt(env).map(|receipt| PathBuf::from(receipt.target));
    own::detected(env, |found| detect::with_placed(found, detect::detect_reaper(found), placed.as_deref()))
}

/// Renoise, through Kumi's own bridge: its tool, a folder in the preferences of the Renoise it goes in.
pub static RENOISE: Own = Own {
    id: DawId::Renoise,
    daw: Daw::Renoise,
    profile: &DawProfile::RENOISE,
    presentation: &renoise::PRESENTATION,
    view: &renoise::VIEW,
    library: &renoise::library::LIBRARY,
    site: "renoise.com",
    bridge: "Kumi's tool",
    bridge_is_folder: true,
    bridge_folder: "bridges/renoise",
    create: renoise::create_renoise_integration,
    instructions: "Renoise isn't connected, so Kumi can't see your song: answer from what you know, and say so.",
    not_found: "Renoise isn't installed (or not where Kumi looks: set KUMI_RENOISE_PREFS_DIR to its preferences folder)",
    watch_words: WatchWords {
        starting: "Renoise is open; Kumi connects once its tool answers.",
        unreachable: "Kumi couldn't reach Renoise, so this is chat without it. Open Renoise with Kumi's tool installed (kumi --daw renoise bridge); Kumi connects by itself once it answers.",
        answered: "Renoise answered, so Kumi is connected to it now.",
    },
    removal_question: "take Kumi's tool out of Renoise too",
    checkout_build: None,
    carried: || RENOISE_TOOL_FOLDER,
    built: |_| Some(kumi_common::dev::checkout()?.join("daws").join("renoise").join(RENOISE_TOOL_FOLDER)),
    version_file: |source| source.join("payload").join("version.lua"),
    placed: |_, env| renoise_target(&own::detected(env, detect::detect_renoise)).map_err(Some),
    // Renoise's version folders from 3.5, where a newer Renoise may have copied Kumi's tool.
    elsewhere: |env| renoise_targets(&own::detected(env, detect::detect_renoise)),
    unloadable: |_, _, _| None,
    used: |_, env| !own::detected(env, detect::detect_renoise).prefs.is_empty(),
    here: |_, env| {
        let found = own::detected(env, detect::detect_renoise);
        !found.apps.is_empty() || !found.prefs.is_empty()
    },
    // The newest Renoise.
    app: |_, env| {
        own::detected(env, detect::detect_renoise)
            .apps
            .into_iter()
            .max_by_key(|app| app.version.as_deref().and_then(detect::Version::of_app))
            .map(|app| app.path)
    },
    app_version: |_, env, is| {
        own::detected(env, detect::detect_renoise).apps.into_iter().filter(|found| is(&found.path)).find_map(|found| found.version)
    },
};

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
