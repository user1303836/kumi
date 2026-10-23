//! Live as the app works with it: its bridge (the Remote Script) and its configuration, the session's integration
//! with what only Live has (its User Library, the Set opener, whether Live runs, Willington's switch), and the words
//! for watching it answer.
use super::{
    answers::{AbletonFactory, LiveLogs},
    bridge_setup, live_app,
    setup::{live_here, LiveSide},
    willington::Willington,
};
use crate::AbletonOptions;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::core::contracts::SessionController;
use kumi_agent::core::{
    context::IntegrationContext,
    contracts::{ConnectionState, Integration},
    presentation::ToolPresentation,
    view::ViewPresentation,
};
use kumi_common::abort;
use kumi_daw::{
    check::{Check, Probe},
    io::{Answers, DawIo},
    provider::{
        BridgeCommand, BridgeRollbackStep, BridgeState, DawProvider, DawSession, EnableStep, Installed, RemovedBridge, Toggle, ToggleWords,
        WatchWords,
    },
    setup::DawSetup,
};
use kumi_library::daws::LibrarySources;
use kumi_types::errors::RuntimeError;
use kumi_types::{
    daw::{DawId, DawProfile},
    system::Env,
};
use std::rc::Rc;
use std::{cell::RefCell, rc::Weak};

/// Live's provider.
pub struct Ableton;

/// Setup's Control Surface step: what the producer does in Live.
const SURFACE: &str = "In Live, open Settings › Link, Tempo & MIDI and set a Control Surface to AbletonMcpBridge. Kumi notices by itself.";
/// What Kumi says while an open Live hasn't answered yet.
pub(crate) const LIVE_STARTING: &str = "Live is open; Kumi connects once it answers.";
/// What Kumi says when its bridge can't reach Live.
pub(crate) const NO_LIVE: &str = "Kumi's bridge couldn't reach Live, so this is chat without Live. Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI); if Live is showing a dialog, answer it. Kumi connects by itself once Live answers.";

impl DawProvider for Ableton {
    fn id(&self) -> DawId {
        DawId::Ableton
    }
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::ABLETON
    }
    fn presentation(&self) -> &'static ToolPresentation {
        &crate::presentation::PRESENTATION
    }
    fn view(&self) -> &'static ViewPresentation {
        &crate::presentation::VIEW
    }
    fn library(&self) -> &'static dyn LibrarySources {
        &crate::library::LIBRARY
    }
    fn site(&self) -> &'static str {
        "ableton.com"
    }
    fn bridge_config(&self, env: &Env) -> Option<String> {
        super::folders::find_bridge_config(env)
    }
    fn bundled_bridge_version(&self) -> Option<String> {
        bridge_setup::bundled_bridge_version()
    }
    fn installed_bridge_version(&self, bridge_config: &str, _env: &Env) -> Option<String> {
        super::folders::read_bridge_server(bridge_config).ok().and_then(|s| s.version)
    }
    fn bridge_answers(&self, bridge_config: &str, _env: &Env) -> LocalBoxFuture<'static, bool> {
        let config = bridge_config.to_string();
        async move { bridge_setup::remote_script_answers(&config).await }.boxed_local()
    }
    fn app_open(&self, env: &Env) -> LocalBoxFuture<'static, bool> {
        let env = env.clone();
        async move { live_app::open_live(&kumi_daw::process::default_run(), kumi_types::system::platform(), &env).await.is_some() }
            .boxed_local()
    }
    fn here(&self, env: &Env) -> bool {
        live_here() && live_app::installed(env, kumi_types::system::platform()) != Installed::Missing
    }
    fn running_now(&self, processes: &[(u32, String)]) -> bool {
        processes.iter().any(|(_, name)| {
            let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
            name == "Live" || (name.starts_with("Ableton Live") && name.ends_with(".exe"))
        })
    }
    fn watch_words(&self) -> WatchWords {
        WatchWords { starting: LIVE_STARTING, unreachable: NO_LIVE, answered: "Live answered, so Kumi is connected to it now." }
    }
    fn enable_step(&self) -> Option<EnableStep> {
        Some(EnableStep {
            label: "Control Surface",
            say: SURFACE,
            waiting: "waiting for Live",
            later: "when Live opens",
            put_off: "Kumi connects to Live by itself once Live answers: in Live, open Settings › Link, Tempo & MIDI and set a Control Surface to AbletonMcpBridge.",
        })
    }
    fn min_version(&self) -> &'static str {
        "12"
    }
    fn integration(&self, context: IntegrationContext, session: &DawSession) -> Rc<dyn Integration> {
        let env = session.env.clone();
        let mut options = AbletonOptions::with_context(context);
        options.bridge_config = Some(session.bridge_config.clone());
        options.project_store = Some(session.project_store.clone());
        options.user_library = super::folders::live_user_library(&env);
        // A kept conversation's Set opens in Live as a double-click would.
        options.open_file = Some({
            let env = env.clone();
            Rc::new(move |set| {
                let env = env.clone();
                async move { live_app::open_set_file(&kumi_daw::process::default_run(), kumi_types::system::platform(), &env, &set).await }
                    .boxed_local()
            })
        });
        // With Live still running, losing it means it's opening another Set, not that it closed (#188).
        options.live_running =
            Some(Rc::new(|| async { bridge_setup::is_live_running(kumi_daw::process::default_run()).await }.boxed_local()));
        // How Willington's bindings stand, read again for each new model session.
        options.willington = Some(Rc::new(move || Willington::find(&env).map(|willington| willington.switch())));
        integration(session.answers.get::<AbletonFactory>(), options)
    }
    fn probe(&self, bridge_config: String, answers: &Answers, _env: &Env) -> LocalBoxFuture<'static, Probe> {
        probe_live(bridge_config, answers.get::<AbletonFactory>().cloned()).boxed_local()
    }
    fn bridge_state(&self, env: &Env, found: bool, missing: bool, bundled: Option<&str>, command: &str) -> BridgeState {
        // A switch from a JavaScript bridge with the same Remote Script changes only the host, which Kumi
        // already runs natively: nothing to ask of the producer (a start with Live closed finishes it).
        let stale = found
            .then(|| super::install::older_bridge(env, bundled))
            .flatten()
            .filter(|stale| !(stale.runtime_migration && super::install::only_the_host_differs(env)));
        // Plain mode has no setup to put the bridge in: the way by hand comes first.
        let connect = kumi_types::command::connect_live_by_hand(command);
        let notice = if missing {
            live_here()
                .then(|| format!("The Ableton bridge isn't in Live yet, so Kumi can't see your Set; chatting without it. {connect}."))
        } else {
            stale.as_ref().map(|stale| {
                if stale.runtime_migration {
                    format!("The bridge in Live still uses JavaScript. {connect}.")
                } else {
                    format!(
                        "The bridge in Live is {}, older than this Kumi's ({}), so some changes aren't offered. {connect}.",
                        stale.installed, stale.bundled
                    )
                }
            })
        };
        // In the app, first-run setup puts the bridge in place instead, with Live restarting around it (where Live runs).
        let why = if missing {
            live_here().then(|| "The Ableton bridge isn't in Live yet, so Kumi can't see your Set.".to_string())
        } else {
            stale.as_ref().map(|stale| {
                if stale.runtime_migration {
                    "The bridge in Live still runs on JavaScript.".to_string()
                } else {
                    format!(
                        "The bridge in Live is {}, older than Kumi's {}, so some changes aren't offered.",
                        stale.installed, stale.bundled
                    )
                }
            })
        };
        BridgeState { notice, why }
    }
    fn setup(
        &self,
        env: &Env,
        bridge: Rc<RefCell<Option<String>>>,
        given: bool,
        controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
    ) -> Rc<dyn DawSetup> {
        Rc::new(LiveSide { env: env.clone(), run: kumi_daw::process::default_run(), bridge, given, controller })
    }
    fn doctor_checks<'a>(&'a self, io: &'a DawIo, node: &'a Check) -> LocalBoxFuture<'a, Vec<Check>> {
        super::doctor::bridge_checks(io, node).boxed_local()
    }
    fn installation_checks<'a>(&'a self, io: &'a DawIo, platform: &'a str) -> LocalBoxFuture<'a, Vec<Check>> {
        super::doctor::installation_checks(&io.env, io, platform).boxed_local()
    }
    fn extra_checks<'a>(&'a self, io: &'a DawIo) -> LocalBoxFuture<'a, Result<Vec<Check>, RuntimeError>> {
        super::doctor::extra_checks(io).boxed_local()
    }
    fn report_log<'a>(&'a self, io: &'a DawIo, home: &'a str) -> LocalBoxFuture<'a, (String, String)> {
        async move {
            let logs = match io.answers.get::<LiveLogs>() {
                Some(logs) => Some((logs.0)().await.unwrap_or_default()),
                None => None,
            };
            super::report::log_section(&io.env, home, logs).await
        }
        .boxed_local()
    }
    fn after_update<'a>(&'a self, io: &'a DawIo, home: &'a str, app: &'a str, back_to: Option<&'a str>) -> LocalBoxFuture<'a, i32> {
        super::install::after_update(io, home, app, back_to).boxed_local()
    }
    fn after_checkout_update<'a>(&'a self, io: &'a DawIo, repo: &'a str) -> LocalBoxFuture<'a, i32> {
        super::install::after_checkout_update(io, repo).boxed_local()
    }
    fn prepare_rollback<'a>(
        &'a self,
        io: &'a DawIo,
        home: &'a str,
        previous: &'a str,
        version: &'a str,
        legacy: bool,
    ) -> LocalBoxFuture<'a, Result<Box<dyn BridgeRollbackStep>, RuntimeError>> {
        super::install::prepare_rollback(io, home, previous, version, legacy).boxed_local()
    }
    fn finish_transition<'a>(&'a self, io: &'a DawIo) -> LocalBoxFuture<'a, ()> {
        super::install::finish_legacy_transition(io).boxed_local()
    }
    fn setup_bridge(&self, io: DawIo, command: BridgeCommand) -> LocalBoxFuture<'static, Result<i32, RuntimeError>> {
        let mut options = bridge_setup::BridgeSetupIo::new(io.out.clone(), io.env.clone());
        options.ask = io.ask.clone();
        options.run = Some(io.run.clone());
        options.live_running = io.answers.get::<super::answers::LiveRunning>().map(|running| running.0.clone());
        options.yes = command.yes;
        options.allow_dirty = command.allow_dirty;
        options.wait_ms = command.wait_ms;
        bridge_setup::setup_bridge(options).boxed_local()
    }
    fn bridge_installed(&self, env: &Env) -> bool {
        super::folders::find_bridge_config(env).is_some()
    }
    fn bridge_folder(&self) -> &'static str {
        "bridge"
    }
    fn bridge_files_in_use(&self, folder: &str, env: &Env) -> bool {
        super::install::bridge_uses(folder, env)
    }
    fn remove_bridge<'a>(&'a self, io: &'a DawIo) -> LocalBoxFuture<'a, Result<RemovedBridge, RuntimeError>> {
        super::install::remove_bridge(io).boxed_local()
    }
    fn removal_question(&self) -> &'static str {
        "ask whether to take the bridge out of Live too (its files stay while Live uses it)"
    }
    fn files_stay(&self, folder: &str) -> String {
        format!("The bridge's files stay in {folder} while Live uses it.")
    }
    fn toggle(&self, env: &Env) -> Option<Toggle> {
        self.profile().caps.willington.then(|| super::willington::control(env.clone()))
    }
    fn toggle_words(&self) -> Option<ToggleWords> {
        Some(ToggleWords { tag: "willington", off_at_start: super::willington::OFF_AT_START })
    }
    fn without_daw(&self, on_connection: Rc<dyn Fn(ConnectionState)>) -> Rc<dyn Integration> {
        crate::create_inference_only_integration(on_connection)
    }
}

/// Live's integration: a test's stand-in, or kumi-daw-ableton's.
fn integration(stand_in: Option<&AbletonFactory>, options: AbletonOptions) -> Rc<dyn Integration> {
    match stand_in {
        Some(factory) => (factory.0)(options),
        None => crate::create_ableton_integration(options),
    }
}

/// Starts Live's integration on a bridge configuration and looks once, for the doctor.
async fn probe_live(config: String, stand_in: Option<AbletonFactory>) -> Probe {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(config);
    let integration = integration(stand_in.as_ref(), options);
    if let Err(error) = integration.start(abort::timeout(20000)).await {
        let _ = integration.close().await;
        return Probe { started: false, said: Some(error.to_string()), ..Default::default() };
    }
    let result = async {
        let observation = integration.observe(abort::timeout(20000), None).await?;
        let context: serde_json::Value = serde_json::from_str(&observation.context).map_err(|e| RuntimeError::plain(e.to_string()))?;
        if context["mode"] == "inference-only" || observation.tools.is_empty() {
            return Ok::<_, RuntimeError>(Probe { started: true, connected: Some(false), ..Default::default() });
        }
        Ok(Probe {
            started: true,
            connected: Some(true),
            live_version: context["liveVersion"].as_str().map(str::to_string),
            set: context["set"]["name"].as_str().map(str::to_string),
            real_live: Some(context["provenance"] == "real-live"),
            said: None,
        })
    }
    .await;
    let _ = integration.close().await;
    result.unwrap_or(Probe { started: true, connected: Some(false), ..Default::default() })
}
