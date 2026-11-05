//! Shared companion lifecycle; no installation into another DAW.
use daw_bridge::{daw::Daw, detect};
use kumi_daw::provider::WatchWords;
use kumi_daw_own::{beside::Beside, provider::detected};
use kumi_types::daw::{DawId, DawProfile};

pub static PROVIDER: Beside = Beside {
    id: DawId::Lynx,
    daw: Daw::Lynx,
    profile: &DawProfile::LYNX,
    presentation: &crate::PRESENTATION,
    view: &crate::VIEW,
    library: &crate::library::LIBRARY,
    site: "github.com/Ka1-S/LYNX-SHARE",
    experimental: true,
    reads_only: false,
    program: "lynx-kumi",
    app_variable: "KUMI_LYNX_APP",
    bridge_folder: "bridges/lynx",
    launch_record: "launched.json",
    create: crate::create_lynx_integration,
    instructions: "LYNX SHARE is disconnected, so Kumi cannot inspect or change this project. Say that before offering general advice.",
    not_found: "A compatible custom LYNX build was not found; set KUMI_LYNX_APP to its application path",
    restart_why: "LYNX must provide the opt-in authenticated native control endpoint used by lynx-kumi.",
    watch_words: WatchWords {
        starting: "Waiting for LYNX's native control connection.",
        unreachable: "LYNX is unavailable. Use the matching custom native-control build and run kumi --daw lynx bridge to connect it.",
        answered: "Connected to the current LYNX project through Kumi's bridge.",
    },
    used: |env| detected(env, detect::detect_lynx).is_some(),
    here: |env| detected(env, detect::detect_lynx).is_some(),
    app: |env| detected(env, detect::detect_lynx).map(|found| found.path),
    app_version: |env, is| detected(env, detect::detect_lynx).filter(|found| is(&found.path)).and_then(|found| found.version),
    last_session: |_| None,
};
