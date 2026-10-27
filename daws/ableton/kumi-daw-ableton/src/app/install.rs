//! Live's part of installing, updating, rolling back and removing Kumi: Kumi's bridge in Live after an update (and the
//! receipt-bound switch from a JavaScript bridge), the bridge going back with a rollback, and the bridge taken out of
//! Live when Kumi is removed.
use super::folders::{find_bridge_config, read_bridge_server, remote_scripts_dir};
use super::{
    answers::{BridgeAfterUpdate, LiveRunning},
    bridge_setup::{bridge_version, is_live_running, kept_generation, BridgeRollback},
    live_extension::{
        extension_data_dir, former_extensions_dir, live_extensions_dir, remove_extension, remove_former_extension, KUMI_EXTENSION_ID,
    },
};
use kumi_daw::{files::remove, provider::newer, provider::newer as newer_version};

fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
use crate::ears::device::EARS_NAME;
use kumi_daw::{
    io::{kumi_home, step, DawIo, Kept},
    process::executable_name,
    provider::{BridgeRollbackStep, RemovedBridge},
};
use kumi_library::sources::{basename, dirname, join, resolve};
use kumi_types::{
    command::{CONNECT_LIVE, KUMI},
    errors::RuntimeError,
    system::{self, Env},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path, process::Stdio, rc::Rc};

pub(crate) async fn live_open(io: &DawIo) -> bool {
    step(
        io.out.as_ref(),
        "Checking whether Live is open…",
        async {
            if let Some(live) = io.answers.get::<LiveRunning>() {
                (live.0)().await
            } else {
                is_live_running(io.run.clone()).await
            }
        },
        false,
    )
    .await
}
/// After an update: going back (`back_to`, the earlier Kumi), a bridge in Live as new as the earlier Kumi's stays, as a
/// newer one serves it too and Live may have it loaded; otherwise the bridge as after any update.
pub(crate) async fn after_update(io: &DawIo, home: &str, app: &str, back_to: Option<&str>) -> i32 {
    // The bundle carries the bridge as the lifecycle's tarball: its package is unpacked beside it, where a session
    // starts the bridge from. (An app from before has the package already.)
    if let Err(why) = super::bridge_setup::unpack_prepared_bridge(&join(app, "bridge")) {
        io.out.write(&format!("Kumi couldn't unpack Live's bridge in {app}: {why}. Run kumi doctor.\n"));
        return 1;
    }
    if let Some(kumi) = back_to {
        let installed = find_bridge_config(&io.env).and_then(|config| read_bridge_server(&config).ok()?.version);
        if let Some(installed) = installed.filter(|installed| bridge_version(app).is_none_or(|bundled| !newer_version(&bundled, installed)))
        {
            io.out.write(&format!("The bridge in Live stays {installed}, which works with {kumi}.\n"));
            return 0;
        }
    }
    bridge_after(io, home, app).await
}
pub(crate) async fn bridge_after(io: &DawIo, home: &str, app: &str) -> i32 {
    let say = |text: String| io.out.write(&format!("{text}\n"));
    // Kumi was never put in Live: the producer may work in another DAW, so nothing is said of Live (the app says how to
    // set Kumi up when it's in no DAW at all).
    let Some(config) = find_bridge_config(&io.env) else { return 0 };
    let server = read_bridge_server(&config).ok();
    let runtime_migration = server.as_ref().is_some_and(|s| !s.native());
    let installed = server.and_then(|s| s.version);
    let bundled = bridge_version(app);
    let Some((installed, bundled)) = installed
        .zip(bundled)
        .filter(|(installed, bundled)| newer_version(bundled, installed) || (runtime_migration && bundled == installed))
    else {
        say("The bridge in Live is up to date.".into());
        return 0;
    };
    say(if runtime_migration {
        format!("The bridge in Live uses JavaScript ({installed}); this Kumi includes the native bridge ({bundled}).")
    } else {
        format!("The bridge in Live is {installed}; this Kumi's is {bundled}.")
    });
    if live_open(io).await {
        say(format!("{}.", *CONNECT_LIVE));
        return 0;
    }
    if let Some(update) = io.answers.get::<BridgeAfterUpdate>() {
        return (update.0)(app.into()).await;
    }
    bridge_command(app, home).status().await.ok().and_then(|s| s.code()).unwrap_or(1)
}
/// `kumi bridge` as an update's last step, Live just found closed: it asks nothing more (its lifecycle looks for
/// Live again before changing anything) and doesn't wait for a Live nobody has opened.
pub fn bridge_command(app: &str, home: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(join(app, &executable_name("kumi")));
    command
        .args(["bridge", "--yes"])
        .current_dir(home)
        .env("KUMI_INSTALLED", "1")
        .env("KUMI_BRIDGE_AFTER", "1")
        .env("KUMI_BRIDGE_WAIT_SECONDS", "0")
        .env("KUMI_HOME", home)
        // Live's bridge, whatever DAW the producer chose for sessions (a Kumi of before #361 ignores it; one of #361's
        // took it).
        .env_remove("KUMI_DAW")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}
pub(crate) async fn remove_bridge(io: &DawIo) -> Result<RemovedBridge, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let Some(config) = find_bridge_config(&io.env) else { return Ok(RemovedBridge::None) };
    let extensions = live_extensions_dir(&io.env, system::platform());
    let by_hand = format!(
        "AbletonMcpBridge from Live's Remote Scripts folder{}",
        extensions
            .as_ref()
            .map(|ext| format!(", and {KUMI_EXTENSION_ID} from {ext} and from {}", dirname(&extension_data_dir(ext))))
            .unwrap_or_default()
    );
    if !io.ask.yes_no("Remove the Ableton bridge from Live too?", None).await {
        say(format!("The bridge stays in Live, and so do its files. To take it out later, remove {by_hand}."));
        return Ok(RemovedBridge::Kept);
    }
    if live_open(io).await {
        say(format!("Live is open, so the bridge stays, and so do its files. Quit Live, then remove {by_hand}."));
        return Ok(RemovedBridge::Kept);
    }
    let root = read_bridge_server(&config).ok().and_then(|server| server.package_root());
    let lifecycle = root
        .as_ref()
        .map(|root| join(root, &executable_name("ableton-mcp-server")))
        .filter(|file| Path::new(file).is_file())
        .or_else(super::bridge_setup::bundled_bridge_binary);
    let Some((root, lifecycle)) = root.zip(lifecycle) else {
        say(format!("Kumi couldn't find the bridge's own uninstaller; remove {by_hand} by hand."));
        return Ok(RemovedBridge::Kept);
    };
    let ran = step(
        io.out.as_ref(),
        "Taking the bridge out of Live…",
        (io.run)(
            lifecycle,
            vec![
                "lifecycle".into(),
                "uninstall".into(),
                "--remote-scripts-dir".into(),
                remote_scripts_dir(&io.env),
                "--state-dir".into(),
                dirname(&config),
                "--package-root".into(),
                root,
                "--apply".into(),
                "--confirm-live-stopped".into(),
            ],
            None,
        ),
        false,
    )
    .await;
    if ran.code != 0 {
        say(format!("The bridge's uninstaller refused; remove {by_hand} by hand."));
        return Ok(RemovedBridge::Kept);
    }
    // The bridge is out: an extension that won't come out is said with where it is, and the rest goes on.
    let removed_extension = match extensions.as_ref().map(|ext| (ext, remove_extension(ext))) {
        Some((_, Ok(removed))) => removed,
        Some((ext, Err(why))) => {
            say(format!(
                "Kumi's extension couldn't be taken out of Live ({why}): delete {KUMI_EXTENSION_ID} from {ext} and from {} by hand.",
                dirname(&extension_data_dir(ext))
            ));
            false
        }
        None => false,
    };
    let removed_former = remove_former_extension(&io.env, system::platform()).unwrap_or_else(|why| {
        let folder = former_extensions_dir(&io.env, system::platform()).unwrap_or_default();
        say(format!("Kumi's earlier extension couldn't be taken out of Live ({why}): delete {KUMI_EXTENSION_ID} from {folder} by hand."));
        false
    });
    say(if removed_extension || removed_former {
        "The bridge and Kumi's extension are out of Live."
    } else {
        "The bridge is out of Live."
    }
    .into());
    let scripts = remote_scripts_dir(&io.env);
    // An Ears device that won't come out is said with where it is, as the extension is: the bridge is out already.
    if basename(&scripts) == "Remote Scripts" {
        let ears = join(&join(&dirname(&scripts), "Kumi"), &format!("{EARS_NAME}.amxd"));
        if let Err(why) = remove(&ears) {
            say(format!("Kumi's Ears device couldn't be taken out of Live's User Library ({why}): delete {ears} by hand."));
        }
    }
    Ok(RemovedBridge::Removed)
}
pub(crate) fn bridge_uses(folder: &str, env: &Env) -> bool {
    let Some(config) = find_bridge_config(env) else { return false };
    let inside = |path: &str| {
        let mut folder = resolve(folder);
        let mut path = resolve(path);
        if cfg!(windows) {
            folder = folder.to_lowercase();
            path = path.to_lowercase();
        }
        Path::new(&path)
            .strip_prefix(&folder)
            .is_ok_and(|relative| !relative.as_os_str().is_empty() && !relative.to_string_lossy().starts_with(".."))
    };
    inside(&config) || read_bridge_server(&config).ok().and_then(|s| s.entry).is_some_and(|entry| inside(&entry))
}

/// Complete a receipt-bound bridge transition after the old updater's application swap. Best effort:
/// Kumi opens whatever happens here, and keeps chatting through the existing bridge until it switches.
pub async fn finish_legacy_transition(io: &DawIo) {
    let home = kumi_home(&io.env);
    let app = join(&home, "app");
    let native = fs::read(join(&app, "package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|metadata| metadata["runtime"] == "rust-native");
    if !native {
        return;
    }
    let Some(config) = find_bridge_config(&io.env) else { return };
    let Some(server) = read_bridge_server(&config).ok().filter(|s| !s.native()) else { return };
    let Some(bundled) = bridge_version(&app) else { return };
    if server.version.as_deref().is_none_or(|version| version != bundled && !newer_version(&bundled, version)) {
        return;
    }
    let unchanged = remote_script_unchanged(&io.env, &app, &config, &server);
    // Live has the existing Remote Script loaded: wait, quietly; the folder Live loaded is never moved
    // under it. A connect is instant; asking the system about Live is not (Windows). With the same
    // Remote Script nothing is missing meanwhile (Kumi runs its own native host), so the session
    // doesn't mention it either.
    if super::bridge_setup::remote_script_answers(&config).await || live_open(io).await {
        return;
    }
    // Byte for byte the same Remote Script (1.7.5's 1.0.74 for this release's): only the host and its
    // receipt change, so the switch is quiet, failing included (a later start tries again).
    let said = Rc::new(Kept::default());
    let mut setup = super::bridge_setup::BridgeSetupIo::new(if unchanged { said.clone() } else { io.out.clone() }, io.env.clone());
    setup.ask = io.ask.clone();
    setup.bridge_dir = Some(app.clone());
    setup.prepared = Some(join(&app, "bridge"));
    setup.yes = true;
    setup.wait_ms = Some(0);
    setup.run = Some(io.run.clone());
    setup.live_running = io.answers.get::<LiveRunning>().map(|running| running.0.clone());
    match super::bridge_setup::setup_bridge(setup).await {
        Ok(0) => {}
        _ if unchanged => {}
        Ok(_) => io.out.write("Kumi can still open. To finish switching the bridge, close Live and run: kumi bridge\n"),
        Err(reason) => io.out.write(&format!(
            "Kumi can still open; the bridge couldn't switch ({reason}). To finish switching it, close Live and run: kumi bridge\n"
        )),
    }
}

/// Whether the Remote Script installed for a JavaScript bridge is, byte for byte, the one the app at
/// `app` brings: then switching changes only the host, which native Kumi already runs.
fn remote_script_unchanged(env: &Env, app: &str, config: &str, server: &super::folders::BridgeServer) -> bool {
    let scripts = server
        .package_root()
        .and_then(|package| super::bridge_setup::owner_paths(config, &package, &kumi_home(env)))
        .map_or_else(|| super::folders::remote_scripts_dir(env), |(_, _, scripts)| scripts);
    super::bridge_setup::remote_script_unchanged(&join(app, "bridge/package"), &scripts)
}

/// Whether a pending switch from a JavaScript bridge would change only its host (the installed Remote
/// Script is the app's own); the session then has nothing to ask of the producer.
pub fn only_the_host_differs(env: &Env) -> bool {
    let app = join(&kumi_home(env), "app");
    let Some(config) = find_bridge_config(env) else { return false };
    let Some(server) = read_bridge_server(&config).ok().filter(|s| !s.native()) else { return false };
    remote_script_unchanged(env, &app, &config, &server)
}

pub(crate) async fn prepare_legacy_rollback(io: &DawIo, home: &str) -> Result<Option<BridgeRollback>, RuntimeError> {
    let Some(config) = find_bridge_config(&io.env) else { return Ok(None) };
    let server = read_bridge_server(&config)?;
    if !server.native() {
        return Ok(None);
    }
    let package = server.package_root().ok_or_else(|| RuntimeError::plain("The bridge's package could not be found."))?;
    let Some((state, secret, scripts)) = super::bridge_setup::owner_paths(&config, &package, home) else {
        return Err(RuntimeError::plain(
            "The earlier Kumi needs its JavaScript bridge, but its owner receipt could not be found. Nothing was changed.",
        ));
    };
    let receipt: Value = serde_json::from_slice(&fs::read(join(&state, "install-receipt.json")).map_err(error)?).map_err(error)?;
    let previous = receipt["previous"]["packageRoot"].as_str().filter(|path| Path::new(path).is_absolute());
    let schema = |root: &str| {
        fs::read(join(root, "release-manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value["schema"].as_str().map(str::to_string))
    };
    if schema(&package).as_deref() != Some("ableton-mcp-native-release/v1")
        || !previous.and_then(schema).is_some_and(|schema| matches!(schema.as_str(), "ableton-mcp-release/v1" | "ableton-mcp-release/v2"))
    {
        return Err(RuntimeError::plain(
            "The earlier Kumi needs its JavaScript bridge, but no retained JavaScript bridge generation is available. Nothing was changed.",
        ));
    }
    if live_open(io).await {
        return Err(RuntimeError::plain("Quit Live (save your work first), then run: kumi update --rollback. Restoring this Kumi also restores its previous bridge; nothing was changed."));
    }
    Ok(Some(BridgeRollback::new(
        server.command.unwrap(),
        package,
        (state, secret, scripts),
        config,
        previous.unwrap().into(),
        io.run.clone(),
    )))
}

/// What a rollback to a native Kumi does with the bridge in Live.
pub(crate) struct EarlierBridge {
    /// The installed bridge's own rollback to the earlier Kumi's, when that goes back too.
    pub(crate) rollback: Option<BridgeRollback>,
    /// What to say once Kumi is back, in place of checking the bridge again.
    pub(crate) said: Option<String>,
    /// With a rollback: what to say if it can't run after all (Live opened meanwhile), which is how to finish later.
    pub(crate) later: Option<String>,
}
/// A rollback to a native Kumi whose own bridge is older than the one in Live. The bridge goes back too when its
/// receipt kept that Kumi's and Live is closed; in a terminal, the producer can close Live first. Otherwise the newer
/// bridge stays: it serves the earlier Kumi too. Kumi never quits Live itself here.
pub(crate) async fn earlier_bridge(io: &DawIo, home: &str, previous: &str, earlier: &str) -> EarlierBridge {
    let unchanged = || EarlierBridge { rollback: None, said: None, later: None };
    let Some(config) = find_bridge_config(&io.env) else { return unchanged() };
    let Ok(server) = read_bridge_server(&config) else { return unchanged() };
    let (Some(installed), Some(bundled)) = (server.version.clone().filter(|_| server.native()), bridge_version(previous)) else {
        return unchanged();
    };
    if !newer_version(&installed, &bundled) {
        return unchanged();
    }
    let stays = format!("The bridge in Live stays {installed}, which works with {earlier}.");
    let twice = format!("{stays} To put back bridge {bundled} too, quit Live, then run {} update --rollback twice.", *KUMI);
    // The bridge goes back after the swap, run from its own folders: one inside the app's folders, which the swap
    // moves, stays.
    let in_app = server.command.as_deref().is_some_and(|command| {
        ["app", "app.previous", "app.rollback"].iter().any(|folder| Path::new(command).starts_with(join(home, folder)))
    });
    let package = server.package_root();
    let owner = package.as_deref().and_then(|package| super::bridge_setup::owner_paths(&config, package, home));
    // That Kumi's own bridge: its version and, when its bundle lists it, its artifact.
    let artifact = super::bridge_setup::prepared_sha256(&join(previous, "bridge"));
    let kept = owner.as_ref().and_then(|(state, _, _)| kept_generation(state)).filter(|kept| kept.is(&bundled, artifact.as_deref()));
    let (false, Some(command), Some(package), Some(owner), Some(kept)) = (in_app, server.command.clone(), package, owner, kept) else {
        return EarlierBridge { rollback: None, said: Some(stays), later: None };
    };
    let back = EarlierBridge {
        rollback: Some(BridgeRollback::new(command, package, owner, config, kept.root, io.run.clone())),
        said: Some(format!("The bridge went back to {bundled} with it; Live loads it when it starts.")),
        later: Some(twice.clone()),
    };
    if !live_open(io).await {
        return back;
    }
    let mut question = format!(
        "Live is open. Going back to {earlier} can also put back its bridge {bundled} once Live is closed (save your work, then quit Live). Put it back too?"
    );
    for _ in 0..3 {
        if !io.ask.yes_no(&question, None).await {
            break;
        }
        if !live_open(io).await {
            return back;
        }
        question = format!("Live is still open. Put back bridge {bundled} too, once it's closed?");
    }
    EarlierBridge { rollback: None, said: Some(twice), later: None }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OlderBridge {
    pub installed: String,
    pub bundled: String,
    #[serde(default)]
    pub runtime_migration: bool,
}
pub fn older_bridge(env: &Env, bundled: Option<&str>) -> Option<OlderBridge> {
    let config = find_bridge_config(env)?;
    let bundled = bundled.filter(|s| !s.is_empty())?;
    let server = read_bridge_server(&config).ok()?;
    let runtime_migration = !server.native();
    let installed = server.version?;
    (newer(bundled, &installed) || (runtime_migration && bundled == installed)).then(|| OlderBridge {
        installed,
        bundled: bundled.into(),
        runtime_migration,
    })
}
pub async fn checkout_bridge(repo: &str) -> i32 {
    let target = std::env::var("CARGO_TARGET_DIR")
        .ok()
        .map(|dir| if Path::new(&dir).is_absolute() { dir } else { join(repo, &dir) })
        .unwrap_or_else(|| join(repo, "target"));
    let mut command = tokio::process::Command::new(join(&target, &format!("release/{}", executable_name("kumi"))));
    // As an installed Kumi's update does: Live was just found closed, so nothing more is asked or waited for.
    command
        .args(["bridge", "--yes"])
        .env("KUMI_BRIDGE_WAIT_SECONDS", "0")
        .env_remove("KUMI_DAW")
        .current_dir(repo)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command.status().await.ok().and_then(|s| s.code()).unwrap_or(1)
}

/// After a checkout's update: Live's bridge brought up to date with the checkout's own build, where Live is closed.
pub(crate) async fn after_checkout_update(io: &DawIo, repo: &str) -> i32 {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let bundled = kumi_daw::process::read_cargo_version(&join(repo, "daws/ableton/ableton-mcp-server/Cargo.toml"));
    let bundled = bundled.as_deref();
    let bridge = older_bridge(&io.env, bundled);
    // Never put in Live: nothing to say of it, as after an installed Kumi's update.
    if find_bridge_config(&io.env).is_none() {
        return 0;
    }
    let Some(bridge) = bridge else {
        say("The bridge in Live is up to date.".into());
        return 0;
    };
    say(if bridge.runtime_migration {
        format!("The bridge in Live uses JavaScript ({}); this Kumi includes the native bridge ({}).", bridge.installed, bridge.bundled)
    } else {
        format!("The bridge in Live is {}; this Kumi's is {}.", bridge.installed, bridge.bundled)
    });
    if live_open(io).await {
        say(format!("{}.", *CONNECT_LIVE));
        return 0;
    }
    if let Some(bridge) = io.answers.get::<BridgeAfterUpdate>() {
        (bridge.0)(repo.into()).await
    } else {
        checkout_bridge(repo).await
    }
}

/// Live's bridge in a rollback of Kumi. A Node Kumi can't run without its own bridge: that goes back first, and
/// forward again if the swap fails. A native one runs with a newer bridge too, so its own goes back after the swap,
/// when it can.
struct LiveRollback {
    bridge_rollback: Option<BridgeRollback>,
    earlier: std::cell::RefCell<Option<EarlierBridge>>,
}

pub(crate) async fn prepare_rollback(
    io: &DawIo,
    home: &str,
    previous: &str,
    version: &str,
    legacy: bool,
) -> Result<Box<dyn BridgeRollbackStep>, RuntimeError> {
    let (bridge_rollback, earlier) = if legacy {
        (prepare_legacy_rollback(io, home).await?, None)
    } else {
        (None, Some(earlier_bridge(io, home, previous, version).await))
    };
    Ok(Box::new(LiveRollback { bridge_rollback, earlier: std::cell::RefCell::new(earlier) }))
}

#[async_trait::async_trait(?Send)]
impl BridgeRollbackStep for LiveRollback {
    async fn before_swap(&self) -> Result<(), RuntimeError> {
        if let Some(rollback) = &self.bridge_rollback {
            rollback.apply().await?;
        }
        Ok(())
    }
    async fn swap_failed(&self) -> Option<String> {
        let rollback = self.bridge_rollback.as_ref()?;
        rollback.apply().await.err().map(|error| {
            format!(
                "The application swap failed, and restoring its bridge also failed: {}. Keep Live closed and run: kumi bridge",
                error.message()
            )
        })
    }
    async fn after_swap(&self, io: &DawIo, home: &str, app: &str, version: &str, native: bool) -> i32 {
        let say = |text: String| io.out.write(&format!("{text}\n"));
        let earlier = self.earlier.borrow_mut().take();
        if native {
            let Some(EarlierBridge { rollback, said: Some(said), later }) = earlier else {
                return bridge_after(io, home, app).await;
            };
            let Some(rollback) = rollback else {
                say(said);
                return 0;
            };
            // This process is still the newer Kumi: the swap only renamed its folder, which nothing from here on reads
            // (on Windows too, where the running executable stays in use). The bridge's own rollback and the kept
            // extension come from the bridge's folders, which the swap doesn't move.
            // Live may have opened while the folders moved (a rename can wait on a held folder): the switch runs with
            // it closed, or not at all.
            if live_open(io).await {
                say(later.unwrap_or_default());
                return 0;
            }
            match step(io.out.as_ref(), "Putting back the bridge…", rollback.apply(), true).await {
                Ok(()) => {
                    say(said);
                    if let Some(placed) = rollback.place_extension(&io.env) {
                        say(placed);
                    }
                    0
                }
                // The lifecycle couldn't put its own changes back: the bridge is neither one nor the other.
                Err(reason) if reason.message().contains("compensation was incomplete") => {
                    say(format!("The bridge's rollback stopped partway ({}). {} doctor says how to repair it.", reason.message(), *KUMI));
                    1
                }
                Err(reason) => {
                    say(format!(
                        "The bridge couldn't go back ({}): the one in Live stays, and works with {version}. To try again, quit Live, then run {} update --rollback twice.",
                        reason.message(),
                        *KUMI
                    ));
                    0
                }
            }
        } else {
            // The receipt-bound bridge rollback above restored the legacy command/config as well; Live's extension goes
            // back with it.
            if let Some(said) = self.bridge_rollback.as_ref().and_then(|rollback| rollback.place_extension(&io.env)) {
                say(said);
            }
            0
        }
    }
}
