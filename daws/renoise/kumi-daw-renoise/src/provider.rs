//! Renoise as Kumi's app works with it, through Kumi's own bridges' provider (kumi-daw-own's): its words, its
//! integration, and what's Renoise's alone about finding it and its tool (its preferences, its version folders from 3.5
//! where a newer Renoise may copy the tool, its apps, and the tool this checkout holds).
use daw_bridge::{
    daw::Daw,
    detect,
    install::{renoise_target, renoise_targets, RENOISE_TOOL_FOLDER},
};
use kumi_daw::{provider::WatchWords, setup::Reopens};
use kumi_daw_own::provider::{detected, Own};
use kumi_types::daw::{DawId, DawProfile};

/// Renoise, through Kumi's own bridge: its tool, a folder in the preferences of the Renoise it goes in.
pub static PROVIDER: Own = Own {
    id: DawId::Renoise,
    daw: Daw::Renoise,
    profile: &DawProfile::RENOISE,
    presentation: &crate::PRESENTATION,
    view: &crate::VIEW,
    library: &crate::library::LIBRARY,
    site: "renoise.com",
    bridge: "Kumi's tool",
    bridge_is_folder: true,
    bridge_folder: "bridges/renoise",
    create: crate::create_renoise_integration,
    instructions: "Renoise isn't connected, so Kumi can't see your song: answer from what you know, and say so.",
    not_found: "Renoise isn't where Kumi looks (open it once, so it makes its preferences folder; or set KUMI_RENOISE_PREFS_DIR to that folder)",
    watch_words: WatchWords {
        starting: "Renoise is open; Kumi connects once its tool answers.",
        unreachable: "Kumi couldn't reach Renoise, so this is chat without it. Open Renoise, or restart it if it's open, so it loads Kumi's tool; Kumi connects by itself once it answers.",
        answered: "Renoise answered, so Kumi is connected to it now.",
    },
    removal_question: "take Kumi's tool out of Renoise too",
    checkout_build: None,
    carried: || RENOISE_TOOL_FOLDER,
    built: |_| Some(kumi_common::dev::checkout()?.join("daws").join("renoise").join(RENOISE_TOOL_FOLDER)),
    version_file: |source| source.join("payload").join("version.lua"),
    placed: |_, env| renoise_target(&detected(env, detect::detect_renoise)).map_err(Some),
    // Renoise's version folders from 3.5, where a newer Renoise may have copied Kumi's tool.
    elsewhere: |env| renoise_targets(&detected(env, detect::detect_renoise)),
    reopens: |_| Reopens::Unknown,
    unloadable: |_, _, _| None,
    used: |_, env| !detected(env, detect::detect_renoise).prefs.is_empty(),
    here: |_, env| {
        let found = detected(env, detect::detect_renoise);
        !found.apps.is_empty() || !found.prefs.is_empty()
    },
    // The newest Renoise.
    app: |_, env| {
        detected(env, detect::detect_renoise)
            .apps
            .into_iter()
            .max_by_key(|app| app.version.as_deref().and_then(detect::Version::of_app))
            .map(|app| app.path)
    },
    app_version: |_, env, is| {
        detected(env, detect::detect_renoise).apps.into_iter().filter(|found| is(&found.path)).find_map(|found| found.version)
    },
};
