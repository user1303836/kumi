//! Logic's M2 bridge attaches beside an already open Logic. Its binary, helper and reference live in Kumi's own
//! folder; Logic loads nothing, and setup never asks the producer to quit or restart it.
mod detection;
mod source;

pub use detection::{detect_logic, LogicInstall, LOGIC_BUNDLES};
pub use source::LOGIC_BRIDGE_SOURCE_VAR;

use daw_bridge::{daw::Daw, files, install, paths::BridgePaths};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_daw::provider::{EnableStep, WatchWords};
use kumi_daw_own::provider::{Own, OwnLifecycle};
use kumi_types::{
    daw::{DawId, DawProfile},
    system::Env,
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub static PROVIDER: Own = Own {
    experimental: true,
    id: DawId::Logic,
    daw: Daw::Logic,
    profile: &DawProfile::LOGIC,
    presentation: &crate::PRESENTATION,
    view: &crate::VIEW,
    plugin_steps: &[],
    library: &crate::library::LIBRARY,
    site: "apple.com/logic-pro",
    bridge: "Kumi's Logic Pro bridge",
    bridge_is_folder: true,
    bridge_folder: "bridges/logic",
    lifecycle: OwnLifecycle::Attached,
    reads_only: false,
    enable_step: Some(EnableStep {
        label: "Accessibility",
        say: "Allow Kumi's helper under System Settings > Privacy & Security > Accessibility: macOS asks the first time Kumi starts it, and lists the app Kumi runs in (your terminal).",
        waiting: "waiting for Accessibility",
        later: "when Accessibility is allowed",
        put_off: "Kumi reads Logic Pro by itself once Accessibility is allowed: in System Settings, open Privacy & Security > Accessibility and turn on the app Kumi runs in.",
    }),
    enabled: Some(enabled),
    source: Some(source::source),
    create: crate::create_logic_integration,
    instructions: "Logic Pro isn't connected, so Kumi can't see your project: answer from what you know, and say so.",
    not_found: "Logic Pro isn't installed (or not where Kumi looks: set KUMI_LOGIC_APP to its app)",
    watch_words: WatchWords {
        starting: "Logic Pro is open; Kumi connects once its bridge attaches.",
        unreachable: "Kumi couldn't reach Logic Pro, so this is chat without it. Open Logic Pro with Kumi's bridge installed (kumi --daw logic bridge); Kumi connects by itself once it answers.",
        answered: "Logic Pro answered, so Kumi is connected to it now.",
    },
    removal_question: "take Kumi's bridge for Logic Pro out too",
    checkout_build: None,
    carried: || install::LOGIC_BRIDGE_FOLDER,
    built: source::built,
    version_file: |path| path.join(install::LOGIC_VERSION_FILE),
    identity_file: |path| path.join(install::LOGIC_BINARY),
    placed: |_, env| Ok(install::logic_target(Path::new(&kumi_daw::io::kumi_home(env)))),
    elsewhere: |_| Vec::new(),
    reopens: |_| kumi_daw::setup::Reopens::Unknown,
    unloadable: |_, _, _| None,
    used: |_, env| best_logic(env).is_some(),
    here: |_, env| best_logic(env).is_some(),
    app: |_, env| best_logic(env).map(|found| found.app),
    app_version: |_, env, is| detect_logic(env).into_iter().filter(|found| is(&found.app)).filter_map(|found| found.version).max().map(|version| version.to_string()),
};

fn best_logic(env: &Env) -> Option<LogicInstall> {
    detect_logic(env).into_iter().max_by_key(|found| found.version)
}

fn enabled(_: &Own, env: &Env) -> LocalBoxFuture<'static, Option<bool>> {
    let home = PathBuf::from(kumi_daw::io::kumi_home(env));
    async move { Some(tokio::task::spawn_blocking(move || accessibility(&home)).await.ok().flatten().unwrap_or(false)) }.boxed_local()
}

/// Only an authenticated status can say the helper has permission. Busy (a dialog or another Space) is independent
/// of Accessibility; an absent status or a bridge that has not attached yet leaves setup waiting.
fn accessibility(home: &Path) -> Option<bool> {
    let paths = BridgePaths::new(home, Daw::Logic);
    let secret = files::read_secret(&paths.secret()).ok()?;
    files::scan_discovery(&paths.discovery(), Daw::Logic).into_iter().filter(|found| found.alive).find_map(|found| {
        let discovery = found.discovery.ok()?;
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], discovery.port));
        let probe = daw_bridge::probe::probe(address, &secret, "kumi", Duration::from_secs(2)).ok()?;
        Some(probe.status.as_ref().is_some_and(|status| status.get("permission").is_some_and(serde_json::Value::is_null)))
    })
}

/// Opens a package with the attached Logic's known bundle id, not with an arbitrary app named by bridge data. A
/// package unreadable to Kumi (for example ~/Music without its grant) is left to Logic; only NotFound proves it gone.
pub(crate) fn open_project(resource: Option<String>, path: String) -> LocalBoxFuture<'static, Result<(), String>> {
    async move {
        if matches!(std::fs::metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
            return Err(format!("That project isn't at {path} any more."));
        }
        if !cfg!(target_os = "macos") {
            return Err("Kumi can't open projects in Logic Pro from here.".into());
        }
        let bundle = resource.filter(|resource| !resource.is_empty()).unwrap_or_else(|| LOGIC_BUNDLES[0].into());
        if !LOGIC_BUNDLES.contains(&bundle.as_str()) {
            return Err("That app isn't a Logic Pro bundle Kumi knows.".into());
        }
        let opened = tokio::process::Command::new("/usr/bin/open")
            .args(["-b", &bundle, "--", &path])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .is_ok_and(|status| status.success());
        if opened {
            Ok(())
        } else {
            Err(format!("Logic Pro didn't take {path}."))
        }
    }
    .boxed_local()
}
