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
    attach_existing: true,
    program: "lynx-kumi",
    app_variable: "KUMI_LYNX_APP",
    bridge_folder: "bridges/lynx",
    launch_record: "launched.json",
    create: crate::create_lynx_integration,
    instructions: "LYNX SHARE is disconnected, so Kumi cannot inspect or change this project. Say that before offering general advice.",
    not_found: "Open a compatible LYNX Remote Control build and enable Remote Control in General settings",
    restart_why: "Kumi attaches to LYNX Remote Control when you enable it in General settings; no application restart is required.",
    watch_words: WatchWords {
        starting: "Waiting for LYNX's native control connection.",
        unreachable: "Enable LYNX Remote Control in General settings in a compatible build, then run kumi --daw lynx bridge.",
        answered: "Connected to the current LYNX project through Kumi's bridge.",
    },
    used: |env| detected(env, detect::detect_lynx).is_some(),
    here: |env| detected(env, detect::detect_lynx).is_some(),
    app: |env| detected(env, detect::detect_lynx).map(|found| found.path),
    app_version: |env, is| detected(env, detect::detect_lynx).filter(|found| is(&found.path)).and_then(|found| found.version),
    last_session: |env| {
        let home = daw_bridge::paths::kumi_home_from(&|name| env.get(name).cloned(), env.get("HOME").map(std::path::PathBuf::from))?;
        let path = home.join("bridges/lynx/launched.json");
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 65536 {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let path = value["path"].as_str()?;
        (std::path::Path::new(path).is_absolute() && std::path::Path::new(path).is_file()).then(|| path.to_owned())
    },
};
