//! A DAW whose bridge is a program Kumi ships beside its own executable (SoundCraft's `soundcraft-kumi`): nothing goes
//! into the DAW. Kumi's host starts the program, and the program reaches a copy of the DAW that Kumi opened with its way
//! in on, through the program's own `launch`. The DAW counts as set up for Kumi once Kumi has opened it that way, as the
//! program's launch record says; a copy open another way (from the Dock, say) can't be reached, so whenever the DAW isn't
//! open through Kumi, setup opens it (having the producer quit one open another way first), and so does
//! `kumi --daw <daw> bridge`. Its integration is the shared one, through Kumi's host, as REAPER's and Renoise's is.
//! Nothing is put in, brought up to date, rolled back or taken out of the DAW: the program comes and goes with Kumi.
use crate::bridge::OwnOptions;
use crate::provider::{
    answers, bridge_log, closed, detected, host_check, kumi_home, open_now, probe_host, running_app, unknown_to, OwnHosts,
};
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
        older_than, BridgeCommand, BridgeRollbackStep, BridgeState, DawProvider, DawSession, EnableStep, Installed, RemovedBridge, Toggle,
        ToggleWords, WatchWords,
    },
    setup::{DawSetup, SetupSession},
};
use kumi_library::daws::LibrarySources;
use kumi_types::{
    command::{KUMI, KUMI_REPAIR},
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
    /// Whether the DAW is still experimental: only `--daw`, `KUMI_DAW` and `/daw` reach it.
    pub experimental: bool,
    /// Whether Kumi only reads the DAW: its integration has no change kinds yet.
    pub reads_only: bool,
    /// The program, by its name beside Kumi's executable: "soundcraft-kumi".
    pub program: &'static str,
    /// The variable the program takes for the app to open ("KUMI_SOUNDCRAFT_APP"): Kumi sets it to the app it found, so
    /// the program opens that one.
    pub app_variable: &'static str,
    /// Kumi's folder of the bridge's state, under Kumi's home: "bridges/soundcraft".
    pub bridge_folder: &'static str,
    /// The file there that says Kumi has opened the DAW for its bridge: the program's launch record, which names the copy
    /// it opened by its `pid`.
    pub launch_record: &'static str,
    pub create: fn(OwnOptions) -> Rc<dyn Integration>,
    /// What chatting without the DAW says it is.
    pub instructions: &'static str,
    /// Why the DAW isn't found.
    pub not_found: &'static str,
    /// Why a DAW the producer opened has to be opened again by Kumi, as setup says it.
    pub restart_why: &'static str,
    pub watch_words: WatchWords,
    /// Whether this user has used the DAW.
    pub used: fn(&Env) -> bool,
    /// Whether the DAW is on this computer, from files alone.
    pub here: fn(&Env) -> bool,
    /// The app Kumi opens, when it finds one.
    pub app: fn(&Env) -> Option<PathBuf>,
    /// The version of the app a path picks out (else of any one found).
    pub app_version: AppVersion,
    /// The session the producer had open last, from the DAW's own files (its newest recent), for Kumi to open the DAW
    /// with.
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

/// The most a launch record holds, as the program keeps it; and the most of what a launch said that Kumi reads.
const READ_MOST: u64 = 64 * 1024;

/// Up to [`READ_MOST`] bytes of a file.
fn read_capped(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(READ_MOST).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Whether the copy Kumi opened runs: the launch record's `pid` is, in `listing` (pid and program), a copy of `daw`. The
/// record is read only as the program keeps it: a file of its own (not a link, a pipe or a folder) of at most 64 KiB.
pub fn open_as_launched(home: &Path, folder: &str, record: &str, daw: Daw, listing: &[(u32, String)]) -> bool {
    let path = home.join(folder).join(record);
    let kept = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() <= READ_MOST);
    let read = kept.then(|| read_capped(&path)).flatten();
    let pid = read.and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()?.get("pid")?.as_u64());
    pid.is_some_and(|pid| listing.iter().any(|(running, name)| u64::from(*running) == pid && detect::process_is(daw, name)))
}

/// What the program's `launch` came to: whether the DAW opened (or the copy Kumi opened was open still), and what it
/// said, a line each, without its name before them: why it didn't open, or what's worth knowing (a recovery the DAW will
/// offer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    pub opened: bool,
    pub said: Vec<String>,
}

/// Opens the DAW with its way in on, through `program launch` for the Kumi home `home`: the app setup found when there
/// is one (named in the program's variable), with `session` when there is one. The program opens the DAW apart from Kumi
/// and returns; the copy Kumi opened, still open, is found again rather than opened twice.
pub fn launch(program: &Path, home: &Path, app: Option<(&str, &Path)>, session: Option<&str>) -> Launched {
    use std::process::Stdio;
    let mut command = std::process::Command::new(program);
    command.arg("launch").arg("--kumi-home").arg(home);
    if let Some(session) = session.filter(|session| !session.is_empty()) {
        command.arg(session);
    }
    if let Some((variable, app)) = app {
        command.env(variable, app);
    }
    // What the program says goes to a file, never a pipe: what it starts can hold its stderr (on Windows the DAW
    // inherits it, rust-lang/rust#54760), and a pipe would keep the launch waiting until the DAW quits.
    let (said_to, stderr) = match said_file() {
        Some((path, file)) => (Some(path), Stdio::from(file)),
        None => (None, Stdio::null()),
    };
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(stderr);
    let status = command.status();
    // (Kumi's own handle on the file goes with the command)
    drop(command);
    let said = said_to
        .map(|path| {
            let bytes = read_capped(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            said_by(program, &bytes)
        })
        .unwrap_or_default();
    match status {
        Ok(status) => Launched { opened: status.success(), said },
        Err(error) => Launched { opened: false, said: vec![format!("{} couldn't run: {error}", program.display())] },
    }
}

/// A fresh file for what a launch says, readable only by this user: its path, and the file.
fn said_file() -> Option<(PathBuf, std::fs::File)> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    let path = std::env::temp_dir().join(format!("kumi-launch-{}-{stamp}.txt", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&path).ok().map(|file| (path, file))
}

/// A program's lines on stderr, without its name before each (`soundcraft-kumi: …`).
fn said_by(program: &Path, stderr: &[u8]) -> Vec<String> {
    let name = program.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    let prefix = format!("{name}: ");
    String::from_utf8_lossy(stderr)
        .lines()
        .map(|line| line.strip_prefix(prefix.as_str()).unwrap_or(line).trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// A log of launches that has grown past this starts again.
const LAUNCH_LOG_MOST: u64 = 64 * 1024;

/// Keeps what a launch came to in the bridge's `logs` (`folder`'s, where `kumi report` reads the bridge's logs), a line a
/// launch: a failure, or a recovery the DAW offered, is still there once setup has moved on. Only in a folder the program
/// made (owner-only, for the record it keeps there): Kumi never makes that folder itself.
fn keep_said(folder: &Path, launched: &Launched) {
    use std::io::Write;
    if !folder.is_dir() {
        return;
    }
    let logs = folder.join("logs");
    if std::fs::create_dir_all(&logs).is_err() {
        return;
    }
    let file = logs.join("launch.log");
    let mut options = std::fs::OpenOptions::new();
    if std::fs::metadata(&file).is_ok_and(|meta| meta.len() > LAUNCH_LOG_MOST) {
        options.create(true).write(true).truncate(true);
    } else {
        options.create(true).append(true);
    }
    let Ok(mut log) = options.open(&file) else { return };
    let came = if launched.opened { "opened" } else { "didn't open" };
    let said = if launched.said.is_empty() { String::new() } else { format!(": {}", launched.said.join(" | ")) };
    let _ = writeln!(log, "{} launch {came}{said}", chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
}

/// A line as a sentence ends: with its stop.
fn sentence(line: &str) -> String {
    let line = line.trim();
    if line.ends_with(['.', '!', '?']) {
        line.to_owned()
    } else {
        format!("{line}.")
    }
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

    /// Whether the copy Kumi opened through the program runs now, from a listing made now (KUMI_PROCESSES in the
    /// session's environment stands in for it): only that copy can be reached.
    fn open_through_kumi(&self, env: &Env) -> bool {
        let listing = detected(env, |found| detect::list_processes(found.var));
        open_as_launched(&kumi_home(env), self.bridge_folder, self.launch_record, self.daw, &listing)
    }

    /// Whether this user has used the DAW, or Kumi has opened it: else the doctor has nothing to say of it.
    fn present(&self, env: &Env) -> bool {
        self.opened(env) || (self.used)(env)
    }

    fn not_carried(&self) -> String {
        format!("This Kumi doesn't carry {} for {}", self.program, self.name())
    }

    /// The command that opens the DAW for Kumi, `kumi` being how the producer runs Kumi: `kumi --daw <daw> bridge`.
    fn open_command(&self, kumi: &str) -> String {
        format!("{kumi} --daw {} bridge", self.id.as_str())
    }

    /// The version of `app`, when it's older than Kumi works with.
    fn older(&self, env: &Env, app: &Path) -> Option<String> {
        (self.app_version)(env, &|path: &Path| path == app).filter(|version| older_than(version, min_version(self.daw)))
    }

    /// What a session says of the DAW, by whether the program is beside Kumi, Kumi has opened the DAW yet (`missing`
    /// when not) and the copy it opened runs now: in plain mode, how to open it; in the app, why setup opens it (`why`),
    /// whenever the DAW isn't open through Kumi.
    fn state(&self, program: bool, missing: bool, through_kumi: bool, command: &str) -> BridgeState {
        if !program {
            let said = self.not_carried();
            return BridgeState {
                notice: Some(format!("{said}; chatting without it. Run {} to repair Kumi.", *KUMI_REPAIR)),
                why: Some(format!("{said}.")),
            };
        }
        if through_kumi && !missing {
            return BridgeState::default();
        }
        let (app, open) = (self.name(), self.open_command(command));
        let (notice, why) = if missing {
            (format!("Kumi hasn't opened {app} yet"), format!("Kumi opens {app} for you, so that it can reach it."))
        } else {
            (format!("{app} isn't open through Kumi"), format!("Kumi reaches the {app} it opens itself."))
        };
        BridgeState { notice: Some(format!("{notice}; chatting without it. Run {open} to open it.")), why: Some(why) }
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
    fn experimental(&self) -> bool {
        self.experimental
    }
    fn reads_only(&self) -> bool {
        self.reads_only
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
    /// Missing, or set up but not open through Kumi (closed, or opened another way), the session says how to open it, and
    /// the app's setup opens it.
    fn bridge_state(&self, env: &Env, _found: bool, missing: bool, _bundled: Option<&str>, command: &str) -> BridgeState {
        let program = self.program().is_some();
        let through_kumi = program && !missing && self.open_through_kumi(env);
        self.state(program, missing, through_kumi, command)
    }
    fn setup(
        &self,
        env: &Env,
        bridge: Rc<RefCell<Option<String>>>,
        given: bool,
        session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
    ) -> Rc<dyn DawSetup> {
        Rc::new(BesideSetup { beside: *self, env: env.clone(), bridge, given, session, launched: RefCell::default() })
    }
    fn doctor_checks<'a>(&'a self, io: &'a DawIo, _node: &'a Check) -> LocalBoxFuture<'a, Vec<Check>> {
        if !self.present(&io.env) {
            return async { vec![] }.boxed_local();
        }
        let (name, open) = (self.name(), self.open_command(&KUMI));
        let (program, opened, probe) = (self.program(), self.opened(&io.env), io.answers.get::<Probes>().cloned());
        // Open, but not the copy Kumi opened: Kumi can't reach it until it's quit and opened again.
        let elsewhere = program.is_some() && opened && open_now(self.daw, &io.env) && !self.open_through_kumi(&io.env);
        let not_carried = self.not_carried();
        let id = self.id;
        async move {
            let carried = program.is_some();
            let mut checks = vec![match program {
                Some(program) => Check::ok(format!("Kumi's bridge for {name} is beside Kumi ({})", program.display())),
                None => Check::fix(not_carried, Some(format!("Run {} to repair Kumi", *KUMI_REPAIR))),
            }];
            if carried && !opened {
                checks.push(Check::note(format!("Kumi hasn't opened {name} yet"), Some(format!("{open} opens it"))));
            } else if elsewhere {
                let fix = format!("Save your {}, quit {name}, then {open} opens it again", self.profile.words.place);
                checks.push(Check::note(format!("{name} is open, but not through Kumi, so Kumi can't reach it"), Some(fix)));
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
        let name = self.name();
        let check = match (self.app)(&io.env) {
            Some(app) => match self.older(&io.env, &app) {
                Some(version) => {
                    let min = min_version(self.daw);
                    Check::fix(
                        format!("This is {name} {version}; Kumi needs {name} {min} or newer"),
                        Some(format!("Install {name} {min} or newer ({})", self.site)),
                    )
                }
                None => Check::ok(format!("{name} is installed here.")),
            },
            None => Check::note(format!("{name}: {}.", self.not_found), None),
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
    /// `kumi --daw <daw> bridge`: nothing goes into the DAW, so this opens it for Kumi, through the program, with the
    /// session the producer had open last. Open through Kumi already, there's nothing to do; open another way, Kumi can't
    /// reach it, so the producer saves and quits it first.
    fn setup_bridge(&self, io: DawIo, _command: BridgeCommand) -> LocalBoxFuture<'static, Result<i32, RuntimeError>> {
        let beside = *self;
        async move {
            let (app, place) = (beside.name(), beside.profile.words.place);
            let Some(program) = beside.program() else {
                io.out.write(&format!("{}: run {} to repair it.\n", beside.not_carried(), *KUMI_REPAIR));
                return Ok(1);
            };
            let env = io.env.clone();
            let (through_kumi, open) = tokio::task::spawn_blocking(move || (beside.open_through_kumi(&env), open_now(beside.daw, &env)))
                .await
                .unwrap_or((false, false));
            if through_kumi {
                io.out.write(&format!("{app} is open through Kumi already, so Kumi reaches it.\n"));
                return Ok(0);
            }
            if open {
                io.out.write(&format!(
                    "{app} is open, but not through Kumi, so Kumi can't reach it: save your {place}, quit {app}, and run this again. Kumi then opens {app} with it.\n"
                ));
                return Ok(1);
            }
            let session = (beside.last_session)(&io.env);
            // (only an app that's there: a path that's gone would stop the program's own search)
            let found = (beside.app)(&io.env).filter(|found| found.exists());
            let (home, shown) = (kumi_home(&io.env), session.clone());
            let launched = tokio::task::spawn_blocking(move || {
                launch(&program, &home, found.as_deref().map(|found| (beside.app_variable, found)), shown.as_deref())
            })
            .await
            .unwrap_or_else(|_| Launched { opened: false, said: vec![] });
            keep_said(&kumi_home(&io.env).join(beside.bridge_folder), &launched);
            if !launched.opened {
                let why = launched.said.last().map_or(String::new(), |why| format!(": {}", sentence(why)));
                io.out.write(&format!("Kumi couldn't open {app}{}\n", if why.is_empty() { ".".into() } else { why }));
                return Ok(1);
            }
            let with = session.as_deref().and_then(|session| Path::new(session).file_name()).map(|name| name.to_string_lossy().into_owned());
            let with = with.map_or(String::new(), |name| format!(" with {name}"));
            let said: Vec<String> = launched.said.iter().map(|line| sentence(line)).collect();
            let notes = if said.is_empty() { String::new() } else { format!(" {}", said.join(" ")) };
            io.out.write(&format!("Kumi opened {app}{with}; {} --daw {} works in it.{notes}\n", *KUMI, beside.id.as_str()));
            Ok(0)
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
    /// Nothing of Kumi's went into the DAW, and its state goes with Kumi's folder: the dry run has nothing more to say.
    fn removal_question(&self) -> &'static str {
        ""
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

/// What a rollback says while KUMI_DAW names the DAW: an earlier Kumi without the program doesn't know the DAW; one
/// with it may still not (the program can ship before Kumi works with the DAW), so that's said as a may. Nothing while
/// KUMI_DAW names another DAW, or none.
fn rollback_words(env: &Env, id: DawId, name: &str, version: &str, carried: bool) -> Option<String> {
    if !carried {
        return unknown_to(env, id, name, version);
    }
    env.get("KUMI_DAW").filter(|daw| daw.eq_ignore_ascii_case(id.as_str()))?;
    Some(format!(
        "KUMI_DAW is set to {} where you run Kumi: if Kumi {version} doesn't know {name} yet, it won't start while it's set, so unset KUMI_DAW then.\n",
        id.as_str()
    ))
}

/// A rollback's step for a DAW whose bridge is a program beside Kumi: the program goes back with the Kumi it came with,
/// so there's nothing to move; what's said is whether the earlier Kumi knows the DAW, while KUMI_DAW names it.
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
        let carried = program_beside(beside.program, Path::new(app)).is_some();
        if let Some(said) = rollback_words(&io.env, beside.id, beside.name(), version, carried) {
            io.out.write(&said);
        }
        0
    }
}

/// Setup's DAW side for a DAW whose bridge is a program beside Kumi: nothing goes in, and Kumi opens the DAW itself,
/// through the program, so that the program can reach it. One the producer opened can't be reached, so they quit it, and
/// Kumi opens it again with the session they had open; one that's closed opens with the session they had open last.
struct BesideSetup {
    beside: Beside,
    env: Env,
    /// The session's bridge slot: once Kumi has opened the DAW, it names the program, so the session's next look connects
    /// through it instead of chatting without the DAW.
    bridge: Rc<RefCell<Option<String>>>,
    /// A bridge configuration given on the command line stays.
    given: bool,
    session: Rc<RefCell<Option<Weak<dyn SetupSession>>>>,
    /// What the last launch came to, for setup to say.
    launched: RefCell<Option<Launched>>,
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
            None => Err(format!("{}: run {} to repair it", self.beside.not_carried(), *KUMI_REPAIR)),
        }
    }
    /// Opens the DAW through the program's `launch`: `app` (the one that was open, else the one Kumi finds), with `set`
    /// when there's one. The DAW has closed by then, so the session it had open is read again: one first saved as it quit
    /// is its newest recent now.
    async fn start(&self, app: Option<String>, set: Option<String>) -> bool {
        let Some(program) = self.beside.program() else { return false };
        let home = kumi_home(&self.env);
        let set = set.map(|set| (self.beside.last_session)(&self.env).unwrap_or(set));
        // The app that was open, else the one Kumi finds; one gone since (moved by its own update, an AppImage's mount)
        // is left to the program's own search.
        let app = app.filter(|app| !app.is_empty()).map(PathBuf::from).or_else(|| (self.beside.app)(&self.env)).filter(|app| app.exists());
        let variable = self.beside.app_variable;
        let launched =
            tokio::task::spawn_blocking(move || launch(&program, &home, app.as_deref().map(|app| (variable, app)), set.as_deref()))
                .await
                .unwrap_or_else(|_| Launched { opened: false, said: vec![] });
        keep_said(&kumi_home(&self.env).join(self.beside.bridge_folder), &launched);
        let opened = launched.opened;
        *self.launched.borrow_mut() = Some(launched);
        opened
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
    /// The app Kumi opens, unless it's older than Kumi works with.
    async fn installed(&self) -> Installed {
        let Some(app) = (self.beside.app)(&self.env) else { return Installed::Missing };
        match self.beside.older(&self.env, &app) {
            Some(version) => Installed::Older(version),
            None => Installed::Ready,
        }
    }
    /// The session the producer has open, for Kumi to open again once they've quit the DAW.
    fn open_set(&self) -> Option<String> {
        (self.beside.last_session)(&self.env)
    }
    fn restart_why(&self, _app: &str) -> String {
        self.beside.restart_why.to_string()
    }
    /// The session is promised only when Kumi knows one to open.
    fn once_closed(&self, app: &str) -> String {
        match (self.beside.last_session)(&self.env) {
            Some(_) => format!("Kumi then opens {app} again, with your session."),
            None => format!("Kumi then opens {app} again."),
        }
    }
    /// Kumi only opens the DAW to reach it: never unasked.
    fn asks_to_open(&self) -> bool {
        true
    }
    /// A DAW that wasn't open opens with the session the producer had open last.
    fn closed_set(&self) -> Option<String> {
        (self.beside.last_session)(&self.env)
    }
    /// Why the program couldn't open the DAW, as it said, and how to try again.
    fn open_failed(&self) -> Option<String> {
        let launched = self.launched.borrow();
        let launched = launched.as_ref().filter(|launched| !launched.opened)?;
        let (app, again) = (self.beside.name(), self.beside.open_command(&KUMI));
        Some(match launched.said.last() {
            Some(why) => format!("Kumi couldn't open {app}: {} {again} tries again.", sentence(why)),
            None => format!("Kumi couldn't open {app}. {again} tries again."),
        })
    }
    /// What the program said as the DAW opened (a recovery it will offer).
    fn opened_note(&self) -> Option<String> {
        let launched = self.launched.borrow();
        let launched = launched.as_ref().filter(|launched| launched.opened && !launched.said.is_empty())?;
        Some(launched.said.iter().map(|line| sentence(line)).collect::<Vec<_>>().join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::OwnBridge;

    struct NoLibrary;
    impl LibrarySources for NoLibrary {
        fn profile(&self) -> &'static DawProfile {
            &DawProfile::RENOISE
        }
    }
    static NO_LIBRARY: NoLibrary = NoLibrary;
    const NO_TOOLS: ToolPresentation =
        ToolPresentation { label: |_| None, doing: |_| None, activity: |_| None, plan: |_| false, acts: |_| false, read_prefix: None };
    fn no_integration(_: OwnOptions) -> Rc<dyn Integration> {
        unreachable!("these tests make no integration")
    }

    /// A DAW beside Kumi as these tests have it: Renoise's names, and its app, the app's version and the session it had
    /// open last from the environment.
    const TESTED: Beside = Beside {
        id: DawId::Renoise,
        daw: Daw::Renoise,
        profile: &DawProfile::RENOISE,
        presentation: &NO_TOOLS,
        view: &ViewPresentation::NONE,
        library: &NO_LIBRARY,
        site: "renoise.com",
        experimental: false,
        reads_only: false,
        program: "program",
        app_variable: "TEST_APP",
        bridge_folder: "bridges/renoise",
        launch_record: "launched.json",
        create: no_integration,
        instructions: "",
        not_found: "",
        restart_why: "",
        watch_words: WatchWords { starting: "", unreachable: "", answered: "" },
        used: |_| true,
        here: |_| true,
        app: |env| env.get("TEST_APP").map(PathBuf::from),
        app_version: |env, _| env.get("TEST_VERSION").cloned(),
        last_session: |env| env.get("TEST_SESSION").cloned(),
    };

    fn env(vars: &[(&str, &str)]) -> Env {
        vars.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }

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

    #[test]
    fn the_copy_kumi_opened_is_the_launch_records_pid_running_as_the_daw() {
        let home = tempfile::tempdir().unwrap();
        let renoise = "/Applications/Renoise.app/Contents/MacOS/Renoise";
        let open =
            |pid: u32, name: &str| open_as_launched(home.path(), "bridges/renoise", "launched.json", Daw::Renoise, &[(pid, name.into())]);
        assert!(!open(4242, renoise), "no launch record");
        std::fs::create_dir_all(home.path().join("bridges/renoise")).unwrap();
        std::fs::write(home.path().join("bridges/renoise/launched.json"), br#"{"pid":4242,"port":51234}"#).unwrap();
        assert!(open(4242, renoise));
        assert!(!open(4243, renoise), "another copy, opened another way");
        assert!(!open(4242, "/bin/zsh"), "its pid, another program's now");
        // Read only as the program keeps it: a file of its own, of at most 64 KiB.
        let record = home.path().join("bridges/renoise/launched.json");
        let mut big = br#"{"pid":4242}"#.to_vec();
        big.resize(65 * 1024, b' ');
        std::fs::write(&record, &big).unwrap();
        assert!(!open(4242, renoise), "a record bigger than the program writes");
        #[cfg(unix)]
        {
            let elsewhere = home.path().join("elsewhere.json");
            std::fs::write(&elsewhere, br#"{"pid":4242}"#).unwrap();
            std::fs::remove_file(&record).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &record).unwrap();
            assert!(!open(4242, renoise), "a link in the record's place");
        }
    }

    #[test]
    fn setup_opens_the_daw_whenever_it_isnt_open_through_kumi_and_plain_mode_names_the_command_that_does() {
        let open = |state: &BridgeState| state.notice.as_deref().is_some_and(|notice| notice.contains("kumi --daw renoise bridge"));
        let never = TESTED.state(true, true, false, "kumi");
        assert!(never.why.is_some() && open(&never), "{never:?}");
        // Opened once, then quit, or opened again from the Dock: setup opens it again.
        let elsewhere = TESTED.state(true, false, false, "kumi");
        assert!(elsewhere.why.is_some() && open(&elsewhere), "{elsewhere:?}");
        let through = TESTED.state(true, false, true, "kumi");
        assert!(through.why.is_none() && through.notice.is_none(), "open through Kumi: it connects");
        let carried = TESTED.state(false, true, false, "kumi");
        assert!(carried.why.as_deref().is_some_and(|why| why.starts_with("This Kumi doesn't carry program")), "{carried:?}");
    }

    #[cfg(unix)]
    #[test]
    fn kumi_opens_the_daw_through_the_programs_launch_with_its_home_the_app_and_the_session() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = (dir.path().join("program"), dir.path().join("args"));
        let write = |script: &str| {
            std::fs::write(&program, format!("#!/bin/sh\n{script}")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write(&format!(
            "printf '%s\\n' \"$@\" \"app=$TEST_APP\" > '{}'\necho 'program: Renoise will offer to recover what it was doing' >&2\n",
            args.display()
        ));
        let home = dir.path().join("kumi home");
        let launched = launch(&program, &home, Some(("TEST_APP", Path::new("/Apps/Renoise.app"))), Some("/songs/Groove.xrns"));
        assert_eq!(launched, Launched { opened: true, said: vec!["Renoise will offer to recover what it was doing".into()] });
        let given = std::fs::read_to_string(&args).unwrap();
        assert_eq!(
            given.lines().collect::<Vec<_>>(),
            ["launch", "--kumi-home", home.to_str().unwrap(), "/songs/Groove.xrns", "app=/Apps/Renoise.app"]
        );
        assert!(launch(&program, &home, None, None).opened);
        assert_eq!(std::fs::read_to_string(&args).unwrap().lines().collect::<Vec<_>>()[3..], ["app="], "no session, no app");
        write("echo 'program: Renoise is open without its control channel (pid 7). Save, quit it.' >&2\nexit 1\n");
        let failed = launch(&program, &home, None, None);
        assert_eq!(
            failed,
            Launched { opened: false, said: vec!["Renoise is open without its control channel (pid 7). Save, quit it.".into()] }
        );
        // Kept where `kumi report` reads the bridge's logs, once the program has made the bridge's folder: Kumi never
        // makes it.
        let folder = home.join("bridges/renoise");
        keep_said(&folder, &failed);
        assert!(!folder.exists(), "no folder made for the log");
        std::fs::create_dir_all(&folder).unwrap();
        keep_said(&folder, &failed);
        let log = std::fs::read_to_string(folder.join("logs/launch.log")).unwrap();
        assert!(log.contains("launch didn't open: Renoise is open without its control channel"), "{log}");
    }

    /// What the program starts can hold its stderr, as the DAW holds it on Windows: the launch returns as the program
    /// does all the same, with what it said.
    #[cfg(unix)]
    #[test]
    fn a_launch_returns_as_the_program_does_whatever_still_holds_its_stderr() {
        use std::io::{Read, Write};
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        use std::process::{Child, Command, Stdio};
        struct Peer(Child);
        impl Drop for Peer {
            fn drop(&mut self) {
                self.0.stdin.take();
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("peer.sock");
        let ready = socket.with_extension("ready");
        let name = std::ffi::CString::new(ready.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is a live C string; this creates only the fixture's FIFO in its temporary directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut ready = std::fs::OpenOptions::new().read(true).write(true).open(ready).unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut peer = Peer(
            Command::new(&executable)
                .args(["--exact", "beside::tests::launch_stderr_fixture", "--nocapture"])
                .env("KUMI_TEST_LAUNCH_SOCKET", &socket)
                .env("KUMI_TEST_LAUNCH_ROLE", "hold")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut state = [0];
        ready.read_exact(&mut state).unwrap();
        assert_eq!(&state, b"R", "the owned peer is listening");
        let program = dir.path().join("program");
        // The launcher gives the owned peer its actual stderr descriptor, then exits. The peer keeps that descriptor
        // until explicitly released (or its parent's stdin pipe closes), so no scheduler deadline stands in for it.
        let executable = executable.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &program,
            format!("#!/bin/sh\nKUMI_TEST_LAUNCH_ROLE=send exec '{executable}' --exact beside::tests::launch_stderr_fixture --nocapture\n"),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launched = launch(&program, dir.path(), Some(("KUMI_TEST_LAUNCH_SOCKET", &socket)), None);
        ready.read_exact(&mut state).unwrap();
        assert_eq!(&state, b"H", "the peer holds the launcher's stderr");
        assert!(peer.0.try_wait().unwrap().is_none(), "launch returned while its stderr was still held");
        assert_eq!(launched, Launched { opened: true, said: vec!["Renoise opened".into()] });
        peer.0.stdin.take().unwrap().write_all(b"release").unwrap();
        assert!(peer.0.wait().unwrap().success());
    }

    /// Runs as a separate test process only for the launch fixture above. The holder is a direct owned Child, not an
    /// orphan grandchild of the launch script, and exits on release or parent EOF even while awaiting a connection.
    #[cfg(unix)]
    #[test]
    fn launch_stderr_fixture() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        let Some(socket) = std::env::var_os("KUMI_TEST_LAUNCH_SOCKET").map(PathBuf::from) else { return };
        if std::env::var("KUMI_TEST_LAUNCH_ROLE").as_deref() == Ok("hold") {
            std::thread::spawn(|| {
                let _ = std::io::stdin().read(&mut [0]);
                std::process::exit(0);
            });
            let listener = UnixListener::bind(&socket).unwrap();
            let mut ready = std::fs::OpenOptions::new().write(true).open(socket.with_extension("ready")).unwrap();
            ready.write_all(b"R").unwrap();
            let (connection, _) = listener.accept().unwrap();
            let _stderr = launch_fixture_descriptor(&connection, false).unwrap();
            ready.write_all(b"H").unwrap();
            loop {
                std::thread::park();
            }
        } else {
            let connection = UnixStream::connect(socket).unwrap();
            launch_fixture_descriptor(&connection, true);
            eprintln!("program: Renoise opened");
        }
    }

    /// Passes the launcher's stderr over the fixture's Unix socket; receives an owned copy on the other side.
    #[cfg(unix)]
    fn launch_fixture_descriptor(connection: &std::os::unix::net::UnixStream, send: bool) -> Option<std::fs::File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let mut byte = [0u8];
        let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
        let mut control = [0usize; 8];
        // SAFETY: zero is a valid empty msghdr; its buffers below stay alive across the synchronous socket call.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        // SAFETY: the ancillary buffer is aligned and larger than the space required for one descriptor.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as _) } as _;
        if send {
            // SAFETY: the live message has enough ancillary space; SCM_RIGHTS copies the sender's descriptor.
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as _) as _;
                std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<libc::c_int>(), libc::STDERR_FILENO);
                assert_eq!(libc::sendmsg(connection.as_raw_fd(), &message, 0), 1);
            }
            None
        } else {
            // SAFETY: recvmsg writes only within the live buffers. A successful SCM_RIGHTS message transfers ownership
            // of a new descriptor, which the returned File closes when the peer is released or exits.
            unsafe {
                assert_eq!(libc::recvmsg(connection.as_raw_fd(), &mut message, 0), 1);
                assert_eq!(message.msg_flags & libc::MSG_CTRUNC, 0);
                let header = libc::CMSG_FIRSTHDR(&message);
                assert!(!header.is_null());
                assert_eq!((*header).cmsg_level, libc::SOL_SOCKET);
                assert_eq!((*header).cmsg_type, libc::SCM_RIGHTS);
                assert!((*header).cmsg_len as usize >= libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as _) as usize);
                let descriptor = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::c_int>());
                Some(std::fs::File::from_raw_fd(descriptor))
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn setup_opens_a_closed_daw_only_in_a_version_kumi_works_with_and_with_the_session_it_had_open_last() {
        let setup = |vars: &[(&str, &str)]| TESTED.setup(&env(vars), Rc::default(), false, Rc::default());
        assert_eq!(setup(&[]).installed().await, Installed::Missing);
        let older = setup(&[("TEST_APP", "/Apps/Renoise.app"), ("TEST_VERSION", "3.4.4")]);
        assert_eq!(older.installed().await, Installed::Older("3.4.4".into()));
        assert_eq!(setup(&[("TEST_APP", "/Apps/Renoise.app"), ("TEST_VERSION", "3.5.4")]).installed().await, Installed::Ready);
        assert_eq!(setup(&[("TEST_APP", "/Apps/Renoise.app")]).installed().await, Installed::Ready, "a version it can't tell");
        // The session is promised, and a closed DAW opens with it, only when Kumi knows one.
        let known = setup(&[("TEST_SESSION", "/songs/Groove.xrns")]);
        assert_eq!(known.closed_set().as_deref(), Some("/songs/Groove.xrns"));
        assert!(known.once_closed("Renoise").contains("with your session"));
        let unknown = setup(&[]);
        assert_eq!(unknown.closed_set(), None);
        assert!(!unknown.once_closed("Renoise").contains("session"));
        assert_eq!((unknown.open_failed(), unknown.opened_note()), (None, None), "nothing launched, nothing said");
    }

    #[test]
    fn a_rollback_names_the_daw_kumi_daw_sets_as_unknown_or_maybe_unknown_by_whether_that_kumi_carries_the_program() {
        let named = env(&[("KUMI_DAW", "renoise")]);
        let without = rollback_words(&named, DawId::Renoise, "Renoise", "1.20.0", false).unwrap();
        assert!(without.contains("Kumi 1.20.0 doesn't know Renoise"), "{without}");
        let with = rollback_words(&named, DawId::Renoise, "Renoise", "1.20.0", true).unwrap();
        assert!(with.contains("if Kumi 1.20.0 doesn't know Renoise yet"), "{with}");
        assert_eq!(rollback_words(&env(&[]), DawId::Renoise, "Renoise", "1.20.0", true), None, "KUMI_DAW unset");
        assert_eq!(rollback_words(&env(&[("KUMI_DAW", "reaper")]), DawId::Renoise, "Renoise", "1.20.0", false), None);
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
