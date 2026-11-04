//! A DAW whose bridge is a program Kumi ships beside its own executable (SoundCraft's `soundcraft-kumi`): nothing goes
//! into the DAW. Kumi's host starts the program, and the program reaches a DAW that Kumi opened with its way in on,
//! through the program's own `launch`. The DAW counts as set up for Kumi once Kumi has opened it that way, as the
//! program's launch record says, and its integration is the shared one, through Kumi's host, as REAPER's and Renoise's
//! is. Nothing is put in, brought up to date, rolled back or taken out of the DAW: the program comes and goes with Kumi.
use crate::bridge::OwnOptions;
use crate::provider::{answers, bridge_log, closed, host_check, kumi_home, open_now, probe_host, running_app, unknown_to, OwnHosts};
use async_trait::async_trait;
use daw_bridge::{daw::Daw, detect, install::min_version};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::core::{
    context::IntegrationContext,
    contracts::{ConnectionState, Integration},
    presentation::ToolPresentation,
    view::ViewPresentation,
};
use kumi_common::abort::Signal;
use kumi_daw::{
    check::{Check, Probe, Probes},
    io::{Answers, DawIo},
    provider::{
        BridgeCommand, BridgeRollbackStep, BridgeState, DawProvider, DawSession, EnableStep, Installed, RemovedBridge, Toggle, ToggleWords,
        WatchWords,
    },
    setup::{DawSetup, SetupSession},
};
use kumi_library::daws::LibrarySources;
use kumi_types::{
    daw::{DawId, DawProfile},
    errors::RuntimeError,
    system::Env,
    version::KUMI_VERSION,
};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::Duration,
};

/// The version of the DAW's app a path picks out (else of any one found), from Kumi's environment.
pub type AppVersion = fn(&Env, &dyn Fn(&Path) -> bool) -> Option<String>;

/// One DAW whose bridge is a program beside Kumi, as its crate gives it: what the app and the library take from it,
/// its words, and what's its alone about finding the DAW and the session the producer had open.
#[derive(Clone, Copy)]
pub struct Beside {
    pub id: DawId,
    pub daw: Daw,
    pub profile: &'static DawProfile,
    pub presentation: &'static ToolPresentation,
    pub view: &'static ViewPresentation,
    pub library: &'static dyn LibrarySources,
    /// Where the DAW is downloaded from, as setup names it.
    pub site: &'static str,
    /// The program, by its name beside Kumi's executable: "soundcraft-kumi".
    pub program: &'static str,
    /// Kumi's folder of the bridge's state, under Kumi's home: "bridges/soundcraft".
    pub bridge_folder: &'static str,
    /// The file there that says Kumi has opened the DAW for its bridge: the program's launch record.
    pub launch_record: &'static str,
    pub create: fn(OwnOptions) -> Rc<dyn Integration>,
    /// What chatting without the DAW says it is.
    pub instructions: &'static str,
    /// Why the DAW isn't found.
    pub not_found: &'static str,
    /// Why a DAW the producer opened has to be opened again by Kumi, as setup says it.
    pub restart_why: &'static str,
    pub watch_words: WatchWords,
    /// What `kumi uninstall --dry-run` says it would also do with the bridge's state.
    pub removal_question: &'static str,
    /// Whether this user has used the DAW.
    pub used: fn(&Env) -> bool,
    /// Whether the DAW is on this computer, from files alone.
    pub here: fn(&Env) -> bool,
    /// The app Kumi opens, when it finds one.
    pub app: fn(&Env) -> Option<PathBuf>,
    /// The version of the app a path picks out (else of any one found).
    pub app_version: AppVersion,
    /// The session the producer has open, from the DAW's own files (its newest recent), for Kumi to open again once
    /// they've quit it.
    pub last_session: fn(&Env) -> Option<String>,
}

/// The program called `name` beside the Kumi whose executable is in `beside`, when it's there.
pub fn program_beside(name: &str, beside: &Path) -> Option<PathBuf> {
    let program = beside.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    program.is_file().then_some(program)
}

/// Whether Kumi has opened the DAW for its bridge: the program's launch record (`record`) is in the bridge's `folder`
/// under Kumi's `home`.
pub fn opened_by_kumi(home: &Path, folder: &str, record: &str) -> bool {
    home.join(folder).join(record).is_file()
}

/// Opens the DAW with its way in on, through `program launch` for the Kumi home `home`, with `session` when there is
/// one: whether it opened. The program opens the DAW apart from Kumi and returns.
pub fn launch(program: &Path, home: &Path, session: Option<&str>) -> bool {
    use std::process::Stdio;
    let mut command = std::process::Command::new(program);
    command.arg("launch").arg("--kumi-home").arg(home);
    if let Some(session) = session.filter(|session| !session.is_empty()) {
        command.arg(session);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command.status().is_ok_and(|status| status.success())
}

impl Beside {
    fn name(&self) -> &'static str {
        self.profile.words.app
    }

    /// The program beside this Kumi.
    fn program(&self) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        program_beside(self.program, exe.parent()?)
    }

    fn opened(&self, env: &Env) -> bool {
        opened_by_kumi(&kumi_home(env), self.bridge_folder, self.launch_record)
    }

    /// The bridge, once the DAW is set up for Kumi: the program beside this Kumi, once Kumi has opened the DAW through it.
    fn set_up(&self, env: &Env) -> Option<PathBuf> {
        self.program().filter(|_| self.opened(env))
    }

    /// Whether this user has used the DAW, or Kumi has opened it: else the doctor has nothing to say of it.
    fn present(&self, env: &Env) -> bool {
        self.opened(env) || (self.used)(env)
    }

    fn not_carried(&self) -> String {
        format!("this Kumi doesn't carry {} for {}", self.program, self.name())
    }
}

impl DawProvider for Beside {
    fn id(&self) -> DawId {
        self.id
    }
    fn profile(&self) -> &'static DawProfile {
        self.profile
    }
    fn presentation(&self) -> &'static ToolPresentation {
        self.presentation
    }
    fn view(&self) -> &'static ViewPresentation {
        self.view
    }
    fn library(&self) -> &'static dyn LibrarySources {
        self.library
    }
    fn site(&self) -> &'static str {
        self.site
    }
    fn bridge_config(&self, env: &Env) -> Option<String> {
        self.set_up(env).map(|program| program.to_string_lossy().into_owned())
    }
    /// The program comes with Kumi, at Kumi's version.
    fn bundled_bridge_version(&self) -> Option<String> {
        self.program().map(|_| KUMI_VERSION.to_string())
    }
    /// The program in use is the one beside this Kumi.
    fn installed_bridge_version(&self, _bridge_config: &str, _env: &Env) -> Option<String> {
        self.bundled_bridge_version()
    }
    fn bridge_answers(&self, _bridge_config: &str, env: &Env) -> LocalBoxFuture<'static, bool> {
        answers(self.daw, kumi_home(env))
    }
    fn here(&self, env: &Env) -> bool {
        (self.here)(env)
    }
    fn running_now(&self, processes: &[(u32, String)]) -> bool {
        processes.iter().any(|(_, name)| detect::process_is(self.daw, name))
    }
    fn app_open(&self, env: &Env) -> LocalBoxFuture<'static, bool> {
        let (daw, env) = (self.daw, env.clone());
        async move { tokio::task::spawn_blocking(move || open_now(daw, &env)).await.unwrap_or(false) }.boxed_local()
    }
    fn watch_words(&self) -> WatchWords {
        self.watch_words
    }
    fn enable_step(&self) -> Option<EnableStep> {
        None
    }
    fn min_version(&self) -> &'static str {
        min_version(self.daw)
    }
    fn integration(&self, context: IntegrationContext, session: &DawSession) -> Rc<dyn Integration> {
        let connect = session.answers.get::<OwnHosts>().map(|hosts| (hosts.0)(self.id));
        (self.create)(OwnOptions { context, kumi_home: kumi_home(&session.env), connect })
    }
    fn probe(&self, _bridge_config: String, answers: &Answers, env: &Env) -> LocalBoxFuture<'static, Probe> {
        probe_host(self.create, kumi_home(env), answers.get::<OwnHosts>().map(|hosts| (hosts.0)(self.id)))
    }
    fn bridge_state(&self, _env: &Env, _found: bool, missing: bool, _bundled: Option<&str>, command: &str) -> BridgeState {
        if !missing {
            return BridgeState::default();
        }
        if self.program().is_none() {
            let said = self.not_carried();
            return BridgeState {
                notice: Some(format!("{said}; chatting without it. Run {} to repair Kumi.", *kumi_types::command::KUMI_REPAIR)),
                why: Some(format!("{said}.")),
            };
        }
        let app = self.name();
        BridgeState {
            notice: Some(format!(
                "Kumi hasn't opened {app} yet; chatting without it. Run {command} --daw {} to open it.",
                self.id.as_str()
            )),
            why: Some(format!("Kumi opens {app} for you, so that it can reach it.")),
        }
    }
    fn setup(
        &self,
        env: &Env,
        bridge: Rc<RefCell<Option<String>>>,
        given: bool,
        session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
    ) -> Rc<dyn DawSetup> {
        Rc::new(BesideSetup { beside: *self, env: env.clone(), bridge, given, session })
    }
    fn doctor_checks<'a>(&'a self, io: &'a DawIo, _node: &'a Check) -> LocalBoxFuture<'a, Vec<Check>> {
        if !self.present(&io.env) {
            return async { vec![] }.boxed_local();
        }
        let (id, name) = (self.id, self.name());
        let (program, opened, probe) = (self.program(), self.opened(&io.env), io.answers.get::<Probes>().cloned());
        let not_carried = self.not_carried();
        async move {
            let mut checks = vec![match program {
                Some(program) => Check::ok(format!("Kumi's bridge for {name} is beside Kumi ({})", program.display())),
                None => Check::fix(not_carried, Some(format!("Run {} to repair Kumi", *kumi_types::command::KUMI_REPAIR))),
            }];
            if !opened {
                let open = format!("{} --daw {} opens it", *kumi_types::command::KUMI, id.as_str());
                checks.push(Check::note(format!("Kumi hasn't opened {name} yet"), Some(open)));
            }
            if let Some(fix) = host_check(probe, id, name).await {
                checks.push(fix);
            }
            checks
        }
        .boxed_local()
    }
    fn installation_checks<'a>(&'a self, io: &'a DawIo, _platform: &'a str) -> LocalBoxFuture<'a, Vec<Check>> {
        if !self.present(&io.env) {
            return async { vec![] }.boxed_local();
        }
        let check = match (self.app)(&io.env) {
            Some(_) => Check::ok(format!("{} is installed here.", self.name())),
            None => Check::note(format!("{}: {}.", self.name(), self.not_found), None),
        };
        async move { vec![check] }.boxed_local()
    }
    fn extra_checks<'a>(&'a self, _io: &'a DawIo) -> LocalBoxFuture<'a, Result<Vec<Check>, RuntimeError>> {
        async { Ok(vec![]) }.boxed_local()
    }
    fn report_log<'a>(&'a self, io: &'a DawIo, _home: &'a str) -> LocalBoxFuture<'a, (String, String)> {
        bridge_log(self.name(), self.daw, &io.env)
    }
    /// The program came with the new Kumi: nothing in the DAW to bring up to date.
    fn after_update<'a>(&'a self, _io: &'a DawIo, _home: &'a str, _app: &'a str, _back_to: Option<&'a str>) -> LocalBoxFuture<'a, i32> {
        async { 0 }.boxed_local()
    }
    /// The checkout's own build of the program is the one beside its Kumi.
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
        async move { Ok(Box::new(BesideRollback(*self)) as Box<dyn BridgeRollbackStep>) }.boxed_local()
    }
    fn finish_transition<'a>(&'a self, _io: &'a DawIo) -> LocalBoxFuture<'a, ()> {
        async {}.boxed_local()
    }
    /// `kumi --daw <daw> bridge`: nothing goes into the DAW, and that's said, with what opens it for Kumi instead.
    fn setup_bridge(&self, io: DawIo, _command: BridgeCommand) -> LocalBoxFuture<'static, Result<i32, RuntimeError>> {
        let app = self.name();
        let said = match self.program() {
            Some(_) => format!(
                "Kumi's bridge for {app} comes with Kumi, and nothing goes into {app}: {} --daw {} opens {app} so that Kumi can reach it.\n",
                *kumi_types::command::KUMI,
                self.id.as_str()
            ),
            None => format!("{}: run {} to repair it.\n", self.not_carried(), *kumi_types::command::KUMI_REPAIR),
        };
        let code = if self.program().is_some() { 0 } else { 1 };
        async move {
            io.out.write(&said);
            Ok(code)
        }
        .boxed_local()
    }
    fn bridge_installed(&self, env: &Env) -> bool {
        self.set_up(env).is_some()
    }
    fn bridge_folder(&self) -> &'static str {
        self.bridge_folder
    }
    fn bridge_files_in_use(&self, _folder: &str, _env: &Env) -> bool {
        false
    }
    /// Nothing of Kumi's is in the DAW: the program goes with Kumi's own folder.
    fn remove_bridge<'a>(&'a self, _io: &'a DawIo) -> LocalBoxFuture<'a, Result<RemovedBridge, RuntimeError>> {
        async { Ok(RemovedBridge::None) }.boxed_local()
    }
    fn removal_question(&self) -> &'static str {
        self.removal_question
    }
    fn files_stay(&self, folder: &str) -> String {
        format!("What Kumi's bridge for {} keeps stays in {folder}.", self.name())
    }
    fn toggle(&self, _env: &Env) -> Option<Toggle> {
        None
    }
    fn toggle_words(&self) -> Option<ToggleWords> {
        None
    }
    fn without_daw(&self, on_connection: Rc<dyn Fn(ConnectionState)>) -> Rc<dyn Integration> {
        kumi_agent::integrations::none::create_no_daw_integration(*self.profile, self.instructions, on_connection)
    }
}

/// A rollback's step for a DAW whose bridge is a program beside Kumi: the program goes back with the Kumi it came with,
/// so there's nothing to move; an earlier Kumi that doesn't carry it doesn't know the DAW, and says so while KUMI_DAW
/// names it.
struct BesideRollback(Beside);

#[async_trait(?Send)]
impl BridgeRollbackStep for BesideRollback {
    async fn before_swap(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn swap_failed(&self) -> Option<String> {
        None
    }
    async fn after_swap(&self, io: &DawIo, _home: &str, app: &str, version: &str, _native: bool) -> i32 {
        let beside = &self.0;
        if program_beside(beside.program, Path::new(app)).is_none() {
            if let Some(said) = unknown_to(&io.env, beside.id, beside.name(), version) {
                io.out.write(&said);
            }
        }
        0
    }
}

/// Setup's DAW side for a DAW whose bridge is a program beside Kumi: nothing goes in, and Kumi opens the DAW itself,
/// through the program, so that the program can reach it. One the producer opened can't be reached, so they quit it, and
/// Kumi opens it again with the session they had open.
struct BesideSetup {
    beside: Beside,
    env: Env,
    /// The session's bridge slot: once Kumi has opened the DAW, it names the program, so the session's next look connects
    /// through it instead of chatting without the DAW.
    bridge: Rc<RefCell<Option<String>>>,
    /// A bridge configuration given on the command line stays.
    given: bool,
    session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
}

#[async_trait(?Send)]
impl DawSetup for BesideSetup {
    /// The DAW's app when it's open: the one running, else the one Kumi would open.
    async fn open_live(&self) -> Option<String> {
        let running = running_app(self.beside.daw, &self.env).await?;
        running.or_else(|| (self.beside.app)(&self.env)).map(|app| app.to_string_lossy().into_owned())
    }
    async fn ask_to_quit(&self) -> bool {
        // Kumi never scripts the DAW's menus: the producer quits it, and saves first if it asks.
        false
    }
    fn can_ask_to_quit(&self) -> bool {
        false
    }
    async fn closed(&self, stop: &Signal) -> bool {
        closed(self.beside.daw, &self.env, stop).await
    }
    /// Nothing goes into the DAW: what's said is the program's version, Kumi's.
    async fn install(&self) -> Result<String, String> {
        match self.beside.program() {
            Some(_) => Ok(KUMI_VERSION.to_string()),
            None => Err(format!("{}: run {} to repair it", self.beside.not_carried(), *kumi_types::command::KUMI_REPAIR)),
        }
    }
    /// Opens the DAW through the program's `launch`, with `set` (the session the producer had open) when Kumi knows it.
    async fn start(&self, _app: Option<String>, set: Option<String>) -> bool {
        let Some(program) = self.beside.program() else { return false };
        let home = kumi_home(&self.env);
        tokio::task::spawn_blocking(move || launch(&program, &home, set.as_deref())).await.unwrap_or(false)
    }
    /// The DAW is opening for Kumi: the session takes the program as its bridge and reconnects; one still starting is
    /// asked again shortly.
    async fn connect(&self) {
        if !self.given {
            if let Some(found) = self.beside.bridge_config(&self.env) {
                *self.bridge.borrow_mut() = Some(found);
            }
        }
        for _ in 0..40 {
            let session = self.session.borrow().as_ref().and_then(Weak::upgrade);
            let Some(session) = session else { return };
            if session.reconnect().await.is_ok() {
                return;
            }
            drop(session);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    /// The version of `app` (else of any one found).
    async fn version(&self, app: Option<String>) -> Option<String> {
        let app = app.filter(|app| !app.is_empty()).map(PathBuf::from);
        let is = |path: &Path| app.as_deref().is_none_or(|app| app == path);
        (self.beside.app_version)(&self.env, &is)
    }
    async fn installed(&self) -> Installed {
        if (self.beside.app)(&self.env).is_some() {
            Installed::Ready
        } else {
            Installed::Missing
        }
    }
    /// The session the producer has open, for Kumi to open again once they've quit the DAW.
    fn open_set(&self) -> Option<String> {
        (self.beside.last_session)(&self.env)
    }
    fn restart_why(&self, _app: &str) -> String {
        self.beside.restart_why.to_string()
    }
    fn once_closed(&self, app: &str) -> String {
        format!("Kumi then opens {app} again, with your session.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::OwnBridge;

    #[test]
    fn the_program_beside_kumi_is_the_bridge_once_kumi_has_opened_the_daw_through_it() {
        let (kumi, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert_eq!(program_beside("soundcraft-kumi", kumi.path()), None, "no program beside this Kumi");
        let program = kumi.path().join(format!("soundcraft-kumi{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&program, b"").unwrap();
        assert_eq!(program_beside("soundcraft-kumi", kumi.path()), Some(program));
        assert!(!opened_by_kumi(home.path(), "bridges/soundcraft", "launched.json"), "Kumi hasn't opened the DAW yet");
        std::fs::create_dir_all(home.path().join("bridges/soundcraft")).unwrap();
        std::fs::write(home.path().join("bridges/soundcraft/launched.json"), b"{}").unwrap();
        assert!(opened_by_kumi(home.path(), "bridges/soundcraft", "launched.json"));
    }

    #[cfg(unix)]
    #[test]
    fn kumi_opens_the_daw_through_the_programs_launch_with_its_home_and_the_session() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (program, said) = (dir.path().join("program"), dir.path().join("said"));
        std::fs::write(&program, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", said.display())).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.path().join("kumi home");
        assert!(launch(&program, &home, Some("/sessions/Groove.scraft")));
        let args = std::fs::read_to_string(&said).unwrap();
        assert_eq!(args.lines().collect::<Vec<_>>(), ["launch", "--kumi-home", home.to_str().unwrap(), "/sessions/Groove.scraft"]);
        assert!(launch(&program, &home, None));
        assert_eq!(std::fs::read_to_string(&said).unwrap().lines().count(), 3, "no session when there's none");
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(!launch(&program, &home, None), "a launch that fails says so");
    }

    #[test]
    fn a_bridge_beside_kumi_is_never_said_to_need_an_update_or_a_restart() {
        for said in [
            OwnBridge::Beside.cant_change("SoundCraft", "soundcraft"),
            OwnBridge::Beside.cant_listen("SoundCraft"),
            OwnBridge::Beside.cant_keep("SoundCraft"),
        ] {
            assert!(!said.contains("update") && !said.contains("restart") && !said.contains("bridge, then"), "{said}");
        }
        assert_eq!(
            OwnBridge::Loaded("tool").cant_change("Renoise", "renoise"),
            "Renoise's tool is older than this Kumi and can't make changes yet: run kumi --daw renoise bridge, then restart Renoise"
        );
    }
}
