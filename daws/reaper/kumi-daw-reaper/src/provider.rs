//! REAPER as Kumi's app works with it, through Kumi's own bridges' provider (kumi-daw-own's): its words, its
//! integration, and what's REAPER's alone about finding it and its extension (REAPER's installs, a portable one Kumi put
//! its extension in, a REAPER that can't load the extension, the library beside the executable that a checkout builds).
use daw_bridge::{
    daw::Daw,
    detect::{self, Os},
    install::{reaper_extension_name, reaper_install, reaper_target, unloadable},
};
use kumi_daw::provider::WatchWords;
use kumi_daw_own::provider::{detected, Own};
use kumi_types::{
    daw::{DawId, DawProfile},
    system::Env,
};
use std::path::PathBuf;

/// REAPER, through Kumi's own bridge: its extension, a file in the UserPlugins folder of the REAPER it goes in.
pub static PROVIDER: Own = Own {
    id: DawId::Reaper,
    daw: Daw::Reaper,
    profile: &DawProfile::REAPER,
    presentation: &crate::PRESENTATION,
    view: &crate::VIEW,
    library: &crate::library::LIBRARY,
    site: "reaper.fm",
    bridge: "Kumi's extension",
    bridge_is_folder: false,
    bridge_folder: "bridges/reaper",
    create: crate::create_reaper_integration,
    instructions: "REAPER isn't connected, so Kumi can't see your project: answer from what you know, and say so.",
    not_found: "REAPER isn't where Kumi looks (open it, and Kumi finds it; or set KUMI_REAPER_RESOURCE_DIR to its resource folder)",
    watch_words: WatchWords {
        starting: "REAPER is open; Kumi connects once its extension answers.",
        unreachable: "Kumi couldn't reach REAPER, so this is chat without it. Open REAPER, or restart it if it's open, so it loads Kumi's extension; Kumi connects by itself once it answers.",
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

/// The REAPERs here, the one Kumi's extension was put in (a portable one Kumi doesn't find by itself), and the ones
/// running now, first: the one the producer has open is the one the extension goes in, wherever it is.
fn reapers(own: &Own, env: &Env) -> Vec<detect::ReaperInstall> {
    let placed = own.receipt(env).map(|receipt| PathBuf::from(receipt.target));
    detected(env, |found| {
        let installs = detect::with_placed(found, detect::detect_reaper(found), placed.as_deref());
        detect::with_running(found, installs, &detect::running_programs(Daw::Reaper, found))
    })
}
