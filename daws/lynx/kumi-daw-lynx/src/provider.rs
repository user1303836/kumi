//! Native control is supplied by the custom LYNX app, not installed into other DAWs.
use crate::{
    client::{config_path, Client},
    integration::LynxIntegration,
};
use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::{
    core::{
        context::IntegrationContext,
        contracts::{ConnectionState, Integration},
        presentation::{ToolActivity, ToolPresentation},
        view::ViewPresentation,
    },
    integrations::none::create_no_daw_integration,
};
use kumi_common::abort::Signal;
use kumi_daw::{
    check::{Check, Probe},
    io::{Answers, DawIo},
    provider::*,
    setup::{DawSetup, SetupSession},
};
use kumi_library::daws::LibrarySources;
use kumi_types::{
    daw::{DawId, DawProfile},
    errors::RuntimeError,
    system::Env,
};
use serde_json::json;
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

pub struct Lynx;
pub static PROVIDER: Lynx = Lynx;
struct Library;
static LIBRARY: Library = Library;
impl LibrarySources for Library {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::LYNX
    }
}

static PRESENTATION: ToolPresentation = ToolPresentation {
    label: |tool| match tool {
        "lynx_inspect" | "lynx_pattern_notes" | "lynx_plugin_inspect" | "lynx_effect_catalog" | "lynx_playlist_inspect" => {
            Some("looked at LYNX")
        }
        _ => None,
    },
    doing: |tool| match tool {
        "lynx_inspect" | "lynx_pattern_notes" | "lynx_plugin_inspect" | "lynx_effect_catalog" | "lynx_playlist_inspect" => {
            Some("looking at LYNX")
        }
        _ => Some("changing LYNX"),
    },
    activity: |tool| {
        Some(
            if matches!(
                tool,
                "lynx_inspect" | "lynx_pattern_notes" | "lynx_plugin_inspect" | "lynx_effect_catalog" | "lynx_playlist_inspect"
            ) {
                ToolActivity::Read
            } else {
                ToolActivity::Change
            },
        )
    },
    plan: |_| false,
    acts: |_| false,
    read_prefix: None,
};
const NEXT: &str =
    "Start a LYNX build with lynx-kumi/1 native control enabled, then run kumi --daw lynx. See the LYNX setup in docs/en/DAWS.md; a standard build without the endpoint cannot connect.";

fn ping(env: &Env) -> LocalBoxFuture<'static, bool> {
    let client = Client::new(config_path(env));
    async move { client.call("session.inspect", json!({}), &Signal::new()).await.is_ok() }.boxed_local()
}

impl DawProvider for Lynx {
    fn id(&self) -> DawId {
        DawId::Lynx
    }
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::LYNX
    }
    fn presentation(&self) -> &'static ToolPresentation {
        &PRESENTATION
    }
    fn view(&self) -> &'static ViewPresentation {
        &ViewPresentation::NONE
    }
    fn library(&self) -> &'static dyn LibrarySources {
        &LIBRARY
    }
    fn site(&self) -> &'static str {
        "github.com/Ka1-S/LYNX-SHARE"
    }
    fn experimental(&self) -> bool {
        true
    }
    fn here(&self, env: &Env) -> bool {
        config_path(env).is_file()
    }
    fn bridge_config(&self, env: &Env) -> Option<String> {
        Some(config_path(env).to_string_lossy().into_owned())
    }
    fn bundled_bridge_version(&self) -> Option<String> {
        Some("lynx-kumi/1".into())
    }
    fn installed_bridge_version(&self, _config: &str, _env: &Env) -> Option<String> {
        Some("lynx-kumi/1".into())
    }
    fn bridge_answers(&self, _config: &str, env: &Env) -> LocalBoxFuture<'static, bool> {
        ping(env)
    }
    fn app_open(&self, env: &Env) -> LocalBoxFuture<'static, bool> {
        ping(env)
    }
    fn watch_words(&self) -> WatchWords {
        WatchWords {
            starting: "Waiting for LYNX's native control endpoint.",
            unreachable: NEXT,
            answered: "Connected to the live LYNX SHARE project.",
        }
    }
    fn enable_step(&self) -> Option<EnableStep> {
        None
    }
    fn min_version(&self) -> &'static str {
        "0.1.23"
    }
    fn integration(&self, context: IntegrationContext, session: &DawSession) -> Rc<dyn Integration> {
        Rc::new(LynxIntegration::new(Client::new(session.bridge_config.clone().into()), context))
    }
    fn probe(&self, config: String, _answers: &Answers, _env: &Env) -> LocalBoxFuture<'static, Probe> {
        async move {
            match Client::new(config.into()).call("session.inspect", json!({}), &Signal::new()).await {
                Ok(state) => Probe {
                    started: true,
                    connected: Some(true),
                    set: state.pointer("/project/name").and_then(serde_json::Value::as_str).map(str::to_owned),
                    real_live: Some(true),
                    ..Default::default()
                },
                Err(error) => Probe { started: true, connected: Some(false), said: Some(error.to_string()), ..Default::default() },
            }
        }
        .boxed_local()
    }
    fn bridge_state(&self, _env: &Env, _found: bool, _missing: bool, _bundled: Option<&str>, _command: &str) -> BridgeState {
        BridgeState::default()
    }
    fn setup(
        &self,
        env: &Env,
        bridge: Rc<RefCell<Option<String>>>,
        given: bool,
        session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
    ) -> Rc<dyn DawSetup> {
        Rc::new(Setup { env: env.clone(), bridge, given, session })
    }
    fn doctor_checks<'a>(&'a self, io: &'a DawIo, _node: &'a Check) -> LocalBoxFuture<'a, Vec<Check>> {
        async move {
            vec![if ping(&io.env).await {
                Check::ok("LYNX's native control endpoint answers")
            } else {
                Check::fix("LYNX's native control endpoint is unavailable", Some(NEXT.into()))
            }]
        }
        .boxed_local()
    }
    fn installation_checks<'a>(&'a self, _io: &'a DawIo, _platform: &'a str) -> LocalBoxFuture<'a, Vec<Check>> {
        async { vec![Check::note("Experimental LYNX adapter; requires a matching native-control build", None)] }.boxed_local()
    }
    fn extra_checks<'a>(&'a self, _io: &'a DawIo) -> LocalBoxFuture<'a, Result<Vec<Check>, RuntimeError>> {
        async { Ok(vec![]) }.boxed_local()
    }
    fn report_log<'a>(&'a self, _io: &'a DawIo, _home: &'a str) -> LocalBoxFuture<'a, (String, String)> {
        async { ("LYNX native control".into(), "Discovery credentials are omitted.".into()) }.boxed_local()
    }
    fn after_update<'a>(&'a self, _io: &'a DawIo, _home: &'a str, _app: &'a str, _back: Option<&'a str>) -> LocalBoxFuture<'a, i32> {
        async { 0 }.boxed_local()
    }
    fn after_checkout_update<'a>(&'a self, _io: &'a DawIo, _repo: &'a str) -> LocalBoxFuture<'a, i32> {
        async { 0 }.boxed_local()
    }
    fn prepare_rollback<'a>(
        &'a self,
        _io: &'a DawIo,
        _home: &'a str,
        _previous: &'a str,
        _version: &'a str,
        _legacy: bool,
    ) -> LocalBoxFuture<'a, Result<Box<dyn BridgeRollbackStep>, RuntimeError>> {
        async { Ok(Box::new(Rollback) as Box<dyn BridgeRollbackStep>) }.boxed_local()
    }
    fn finish_transition<'a>(&'a self, _io: &'a DawIo) -> LocalBoxFuture<'a, ()> {
        async {}.boxed_local()
    }
    fn setup_bridge(&self, io: DawIo, _command: BridgeCommand) -> LocalBoxFuture<'static, Result<i32, RuntimeError>> {
        async move {
            let connected = ping(&io.env).await;
            if !connected {
                io.out.write(NEXT);
            }
            Ok(if connected { 0 } else { 1 })
        }
        .boxed_local()
    }
    fn bridge_installed(&self, _env: &Env) -> bool {
        false
    }
    fn bridge_folder(&self) -> &'static str {
        "bridges/lynx"
    }
    fn bridge_files_in_use(&self, _folder: &str, _env: &Env) -> bool {
        false
    }
    fn remove_bridge<'a>(&'a self, _io: &'a DawIo) -> LocalBoxFuture<'a, Result<RemovedBridge, RuntimeError>> {
        async { Ok(RemovedBridge::None) }.boxed_local()
    }
    fn removal_question(&self) -> &'static str {
        ""
    }
    fn files_stay(&self, _folder: &str) -> String {
        "The LYNX app is managed separately from Kumi.".into()
    }
    fn toggle(&self, _env: &Env) -> Option<Toggle> {
        None
    }
    fn toggle_words(&self) -> Option<ToggleWords> {
        None
    }
    fn without_daw(&self, on_connection: Rc<dyn Fn(ConnectionState)>) -> Rc<dyn Integration> {
        create_no_daw_integration(DawProfile::LYNX, NEXT, on_connection)
    }
}

struct Setup {
    env: Env,
    bridge: Rc<RefCell<Option<String>>>,
    given: bool,
    session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
}
#[async_trait(?Send)]
impl DawSetup for Setup {
    async fn open_live(&self) -> Option<String> {
        None
    }
    async fn ask_to_quit(&self) -> bool {
        false
    }
    async fn closed(&self, stop: &Signal) -> bool {
        !stop.is_cancelled()
    }
    async fn install(&self) -> Result<String, String> {
        if !self.given {
            *self.bridge.borrow_mut() = Some(config_path(&self.env).to_string_lossy().into_owned());
        }
        Ok("lynx-kumi/1".into())
    }
    async fn start(&self, _app: Option<String>, _set: Option<String>) -> bool {
        ping(&self.env).await
    }
    async fn connect(&self) {
        let session = self.session.borrow().as_ref().and_then(Weak::upgrade);
        if let Some(session) = session {
            let _ = session.reconnect().await;
        }
    }
    async fn version(&self, _app: Option<String>) -> Option<String> {
        Some("0.1.23".into())
    }
    fn can_ask_to_quit(&self) -> bool {
        false
    }
    fn open_failed(&self) -> Option<String> {
        Some(NEXT.into())
    }
}
struct Rollback;
#[async_trait(?Send)]
impl BridgeRollbackStep for Rollback {
    async fn before_swap(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn swap_failed(&self) -> Option<String> {
        None
    }
    async fn after_swap(&self, _io: &DawIo, _home: &str, _app: &str, _version: &str, _native: bool) -> i32 {
        0
    }
}
