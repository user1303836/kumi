//! What the app needs of a DAW it works with: finding Kumi's bridge for it, making the session's integration with the
//! app's neutral context, watching for the DAW to answer, and chatting without it in its words.
use futures::future::LocalBoxFuture;
use kumi_agent::{
    core::{
        context::IntegrationContext,
        contracts::{ConnectionState, Integration, SessionController},
        presentation::ToolPresentation,
        view::ViewPresentation,
    },
    integrations::ProjectStore,
};
use kumi_types::{
    daw::{DawId, DawProfile},
    system::Env,
};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use super::ableton::AbletonFactory;
use crate::doctor::{Check, DoctorIo, InstallationIo};
use crate::install::InstalledIo;
use crate::process::Run;
use crate::report::LogFiles;
use crate::tui::app::DawSetup;
use crate::tui::tty::TtyOutput;
use crate::update::UpdateIo;
use kumi_types::errors::RuntimeError;

/// How a DAW whose bridge is Kumi's own reaches Kumi's host, by DAW: a test's host in place of `daw-mcp-server`.
pub type OwnHostFactory = Rc<dyn Fn(DawId) -> kumi_daw_own::bridge::Connect>;

/// Makes each registered DAW's integration from its options: the app's own, or a test's stand-in.
#[derive(Clone)]
pub struct DawFactory {
    pub ableton: AbletonFactory,
    /// REAPER's and Renoise's host: none in production (Kumi starts `daw-mcp-server`).
    pub own_host: Option<OwnHostFactory>,
}

impl DawFactory {
    /// Each DAW's production integration.
    pub fn native() -> Self {
        DawFactory { ableton: Rc::new(kumi_daw_ableton::create_ableton_integration), own_host: None }
    }
}

impl From<AbletonFactory> for DawFactory {
    fn from(ableton: AbletonFactory) -> Self {
        DawFactory { ableton, own_host: None }
    }
}

/// What the app hands a DAW's provider for the session's integration, beside the neutral context.
#[derive(Clone)]
pub struct DawSession {
    pub env: Env,
    /// Where the DAW's bridge says how to reach it.
    pub bridge_config: String,
    /// Where saved projects' ids and baselines are kept.
    pub project_store: Rc<dyn ProjectStore>,
    pub factory: DawFactory,
}

/// Whether `version` ("11.3.21", "3.4.4", "6.83") is older than `min` ("12", "3.5"), number by number; a version that
/// doesn't start with a number isn't.
pub fn older_than(version: &str, min: &str) -> bool {
    let numbers = |text: &str| -> Vec<u32> {
        let word = text.split_whitespace().next().unwrap_or("");
        word.split('.').map_while(|part| part.parse().ok()).collect()
    };
    let version = numbers(version);
    !version.is_empty() && version < numbers(min)
}

/// What of the DAW is on this computer, read from its files without opening it: whether it can take Kumi's bridge yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// A version Kumi works with has been opened here, so it has the folders the bridge goes in.
    Ready,
    /// None at all.
    Missing,
    /// Only an older one: the newest one's version ("11.3.13").
    Older(String),
    /// One Kumi works with is installed (this app), but hasn't been opened yet.
    NeverOpened(String),
}

/// The step setup shows when the DAW has to be told to use Kumi's bridge, by the producer, in the DAW (Live's
/// Control Surface): its name, what to do, what it says while it waits, and what it says when put off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnableStep {
    pub label: &'static str,
    pub say: &'static str,
    pub waiting: &'static str,
    pub later: &'static str,
    /// What Kumi says when the producer puts it off.
    pub put_off: &'static str,
}

/// What the session says of the DAW's bridge as it starts: in plain mode (`notice`, with the way by hand), and as
/// setup's reason to put it in place (`why`); None for each when the bridge is current.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeState {
    pub notice: Option<String>,
    pub why: Option<String>,
}

/// What became of the DAW's bridge when Kumi was removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovedBridge {
    Removed,
    Kept,
    None,
}

/// `kumi bridge`'s options: answer yes to its questions, allow a checkout with changes, and how long to wait for the
/// DAW to answer once the bridge is in place.
#[derive(Clone, Default)]
pub struct BridgeCommand {
    pub input: Option<Rc<dyn crate::input::TerminalInput>>,
    pub yes: bool,
    pub allow_dirty: bool,
    pub wait_ms: Option<u64>,
}

/// A DAW's bridge in a rollback of Kumi: put back before Kumi's folders swap when the earlier Kumi needs it first,
/// restored if the swap fails, and finished once Kumi is back.
#[async_trait::async_trait(?Send)]
pub trait BridgeRollbackStep {
    /// Before the swap.
    async fn before_swap(&self) -> Result<(), RuntimeError>;
    /// The swap failed: what to say when the bridge couldn't be restored either.
    async fn swap_failed(&self) -> Option<String>;
    /// Kumi is back at `version` (`native`: a native Kumi); the exit code.
    async fn after_swap(&self, io: &InstalledIo, home: &str, app: &str, version: &str, native: bool) -> i32;
}

/// A switch of the DAW's that the producer flips with a command (Live: /willington, for Willington's bindings): whether
/// it's on (None while the DAW's bridge doesn't carry it), and flipping it, which ends with what to tell the producer.
#[derive(Clone)]
pub struct Toggle {
    pub on: Rc<dyn Fn() -> Option<bool>>,
    pub set: Rc<dyn Fn(bool) -> LocalBoxFuture<'static, Result<&'static str, String>>>,
}

/// What the app says of the DAW's switch: plain mode's tag for it, and the note when it's off as Kumi starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToggleWords {
    pub tag: &'static str,
    pub off_at_start: &'static str,
}

/// What the app says while it watches for the DAW to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchWords {
    /// The DAW is open but hasn't answered yet.
    pub starting: &'static str,
    /// Kumi's bridge can't reach the DAW: how to make it answer.
    pub unreachable: &'static str,
    /// It answered, and the session connected.
    pub answered: &'static str,
}

pub trait DawProvider {
    fn id(&self) -> DawId;
    fn profile(&self) -> &'static DawProfile {
        crate::daw::registry::profile(self.id()).expect("a provider's DAW is registered")
    }
    fn presentation(&self) -> &'static ToolPresentation {
        crate::daw::registry::presentation(self.id()).expect("a provider's DAW is registered")
    }
    fn view(&self) -> &'static ViewPresentation {
        crate::daw::registry::view(self.id()).expect("a provider's DAW is registered")
    }
    /// Where Kumi's bridge for the DAW says how to reach it, when the bridge is installed.
    fn bridge_config(&self, env: &Env) -> Option<String>;
    /// The version of the bridge this Kumi brings for the DAW.
    fn bundled_bridge_version(&self) -> Option<String>;
    /// The version of the bridge installed where a configuration says.
    fn installed_bridge_version(&self, bridge_config: &str, env: &Env) -> Option<String>;
    /// Whether the DAW's bridge answers now.
    fn bridge_answers(&self, bridge_config: &str, env: &Env) -> LocalBoxFuture<'static, bool>;
    /// Whether the DAW is open.
    fn app_open(&self, env: &Env) -> LocalBoxFuture<'static, bool>;
    /// Whether the DAW runs now, from one process listing: for choosing among several DAWs whose bridges are installed.
    fn running_now(&self) -> bool {
        false
    }
    /// Whether the DAW is on this computer, from files alone (installed, or opened once): with no bridge installed yet,
    /// the DAW a first run sets up.
    fn here(&self, env: &Env) -> bool {
        let _ = env;
        false
    }
    fn watch_words(&self) -> WatchWords;
    /// The step setup shows for the producer to tell the DAW to use Kumi's bridge; None when it needs none.
    fn enable_step(&self) -> Option<EnableStep>;
    /// The oldest version of the DAW Kumi works with, as people write it ("12", "3.5").
    fn min_version(&self) -> &'static str;
    /// The session's integration with the DAW: the app's neutral context, and what the DAW finds for itself.
    fn integration(&self, context: IntegrationContext, session: &DawSession) -> Rc<dyn Integration>;
    /// For the doctor: whether Kumi reaches the DAW through a bridge configuration, and what it says.
    fn probe(&self, bridge_config: String, factory: &DawFactory, env: &Env) -> LocalBoxFuture<'static, crate::doctor::LiveProbe>;
    /// What the session says of the DAW's bridge: `missing` when it isn't installed; `found` when a configuration was,
    /// which may be older than this Kumi's (`bundled`); `command` starts Kumi in the way by hand.
    fn bridge_state(&self, env: &Env, found: bool, missing: bool, bundled: Option<&str>, command: &str) -> BridgeState;
    /// The DAW's side of first-run setup: `bridge` is the configuration the session's integration starts from, which
    /// setup sets once the bridge is in place unless it was `given`.
    fn setup(
        &self,
        env: &Env,
        bridge: Rc<RefCell<Option<String>>>,
        given: bool,
        controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
    ) -> Rc<dyn DawSetup>;
    /// The DAW's part of `kumi doctor`: its bridge, and whether Kumi reaches the DAW through it (`node` is the doctor's
    /// check of Kumi itself).
    fn doctor_checks<'a>(&'a self, io: &'a DoctorIo, node: &'a Check) -> LocalBoxFuture<'a, Vec<Check>>;
    /// What the DAW needs from this computer, for the doctor's Installation section.
    fn installation_checks<'a>(&'a self, env: &'a Env, io: &'a InstallationIo, platform: &'a str) -> LocalBoxFuture<'a, Vec<Check>>;
    /// What the DAW needs beyond its bridge, later in the doctor (Live: Kumi's hands in its menus).
    fn extra_checks<'a>(&'a self, io: &'a DoctorIo) -> LocalBoxFuture<'a, Result<Vec<Check>, RuntimeError>>;
    /// The DAW's log for `kumi report`: its heading, and its bridge's lines and errors (`logs`: a test's log files in
    /// place of the DAW's own).
    fn report_log<'a>(&'a self, env: &'a Env, home: &'a str, logs: Option<&'a LogFiles>) -> LocalBoxFuture<'a, (String, String)>;
    /// After an installed Kumi's update (`back_to`: to the earlier Kumi it names), its bridge brought up to date where it
    /// can be, or how to: the exit code.
    fn after_update<'a>(&'a self, io: &'a InstalledIo, home: &'a str, app: &'a str, back_to: Option<&'a str>) -> LocalBoxFuture<'a, i32>;
    /// After a checkout's update, the same, with the checkout's own build of the bridge.
    fn after_checkout_update<'a>(&'a self, io: &'a UpdateIo, run: Run, repo: &'a str) -> LocalBoxFuture<'a, i32>;
    /// Ready the bridge for a rollback to the Kumi at `previous` (`version`; `legacy`: a JavaScript Kumi).
    fn prepare_rollback<'a>(
        &'a self,
        io: &'a InstalledIo,
        home: &'a str,
        previous: &'a str,
        version: &'a str,
        legacy: bool,
    ) -> LocalBoxFuture<'a, Result<Box<dyn BridgeRollbackStep>, RuntimeError>>;
    /// Finishes a switch of the bridge an earlier updater began, before a session starts. Best effort.
    fn finish_transition<'a>(&'a self, io: &'a InstalledIo) -> LocalBoxFuture<'a, ()>;
    /// `kumi bridge`: puts Kumi's bridge in the DAW, or brings it up to date.
    fn setup_bridge(&self, out: Rc<dyn TtyOutput>, env: &Env, command: BridgeCommand)
        -> LocalBoxFuture<'static, Result<i32, RuntimeError>>;
    /// Whether Kumi's bridge is in the DAW, for removing Kumi.
    fn bridge_installed(&self, env: &Env) -> bool;
    /// Kumi's folder of the DAW's bridge files, under Kumi's home ("bridge").
    fn bridge_folder(&self) -> &'static str;
    /// Whether the DAW uses the bridge files in `folder`, so they stay when Kumi goes.
    fn bridge_files_in_use(&self, folder: &str, env: &Env) -> bool;
    /// Takes Kumi's bridge out of the DAW, asking first.
    fn remove_bridge<'a>(&'a self, io: &'a InstalledIo, run: Run) -> LocalBoxFuture<'a, Result<RemovedBridge, RuntimeError>>;
    /// What `kumi uninstall --dry-run` says it would also do with the bridge.
    fn removal_question(&self) -> &'static str;
    /// What removing Kumi says when the bridge's files stay in `folder`.
    fn files_stay(&self, folder: &str) -> String;
    /// The DAW's switch the producer flips with a command, for a DAW that has one (Live: Willington's bindings).
    fn toggle(&self, env: &Env) -> Option<Toggle>;
    /// What the app says of that switch.
    fn toggle_words(&self) -> Option<ToggleWords>;
    /// Chatting without the DAW, in its words.
    fn without_daw(&self, on_connection: Rc<dyn Fn(ConnectionState)>) -> Rc<dyn Integration>;
}
