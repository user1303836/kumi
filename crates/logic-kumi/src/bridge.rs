//! One run of the bridge: find Logic through the helper, attach to it, serve on loopback, follow its project, read it
//! for the hosts, and stop when Logic quits or the process is asked to. The run loop alone talks to the helper: every
//! 150 ms it refreshes the focus cache with targeted reads (the control bar, the track headers, the regions), every
//! 500 ms it reads Logic's windows for the project and whether Logic can take work, and between those it answers the
//! requests the socket threads queued (`read`, `snapshot`, `focus_body`) through the project reader, pumps the
//! Mackie Control's bytes and sends the events a change deserves.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use daw_bridge::files::{self, FileError, Reference};
use daw_bridge::handshake::ServerConnection;
use daw_bridge::paths::BridgePaths;
use daw_bridge::wire::{self, HEARTBEAT_MS};
use daw_bridge::Daw;
use serde_json::{json, Value};

use crate::ax::strings::{self, Strings};
use crate::ax::{Ax, AxError};
use crate::changes::Changes;

/// The methods that write to Logic: the focus is refreshed before their answer leaves.
const WRITES: [&str; 11] = [
    "change_apply",
    "change_restore",
    "transport_stop",
    "transport_play",
    "cursor_set",
    "project_save",
    "bounce",
    "export_tracks",
    "hear",
    "audition",
    "render",
];
use crate::focus::{self, Focus};
use crate::helper::Helper;
use crate::log;
use crate::logic::{self, App, Model, BUNDLES, POLL};
use crate::mcu::Mcu;
use crate::project::Project;
use crate::queue::{Answer, Job, Queue};
use crate::server::{self, Hub, Server, Shared2};

/// How often the running apps are asked for: while waiting for Logic, and once attached as the check that it still
/// runs (its `terminated` event is the quick way; this is the sure one).
pub const APPS_EVERY: Duration = Duration::from_secs(2);
/// How often the focus cache is refreshed.
pub const REFRESH: Duration = Duration::from_millis(focus::REFRESH_MS);
/// The most queued requests answered in one tick before the focus is refreshed again.
const JOBS_PER_TICK: usize = 4;
/// How long the Mackie Control gets to install after its ports open, answering Logic's queries meanwhile.
const MCU_INSTALL: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone)]
pub struct Config {
    pub server: server::Config,
    /// How often Logic's windows are read.
    pub poll: Duration,
    pub apps_every: Duration,
    /// How often the focus is refreshed.
    pub refresh: Duration,
    /// Whether to open the Mackie Control's ports (tests over a mock helper don't).
    pub mcu: bool,
    /// Where Logic's app bundle is, for its string tables; found from the running app when None.
    pub bundle: Option<std::path::PathBuf>,
}

impl Config {
    pub fn from_reference(reference: &Reference) -> Config {
        Config {
            server: server::Config::from_reference(reference),
            poll: POLL,
            apps_every: APPS_EVERY,
            refresh: REFRESH,
            mcu: true,
            bundle: None,
        }
    }

    /// A development run without a reference file: the secret in Kumi's folder (made when missing), the discovery and
    /// log folders beside it, Logic's ports from the first.
    pub fn development(kumi_home: &Path) -> Result<Config, FileError> {
        let paths = BridgePaths::new(kumi_home, Daw::Logic);
        files::ensure_secret(&paths.secret())?;
        Ok(Config {
            server: server::Config {
                secret_file: paths.secret(),
                discovery_dir: paths.discovery(),
                port: None,
                log_dir: Some(paths.logs()),
                connection: ServerConnection::new,
            },
            poll: POLL,
            apps_every: APPS_EVERY,
            refresh: REFRESH,
            mcu: true,
            bundle: None,
        })
    }

    /// Other intervals than the defaults, for tests.
    pub fn with_timing(mut self, poll: Duration, apps_every: Duration) -> Config {
        self.poll = poll;
        self.apps_every = apps_every;
        self.refresh = poll;
        self
    }

    /// No Mackie Control ports: tests, and a Logic whose control surfaces mustn't be touched.
    pub fn without_mcu(mut self) -> Config {
        self.mcu = false;
        self
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Logic quit (or was no longer running): the hosts were told `bye` with reason `quit`.
    LogicQuit,
    /// The process was asked to stop (a signal): reason `shutdown`.
    Stopped,
}

impl Outcome {
    pub fn bye(self) -> &'static str {
        match self {
            Outcome::LogicQuit => "quit",
            Outcome::Stopped => "shutdown",
        }
    }
}

/// The Logic attached to, the server for it, the hub to its connections, and what the run loop reads with.
struct Attached {
    pid: u32,
    model: Arc<Mutex<Model>>,
    focus: Arc<Mutex<Focus>>,
    queue: Queue,
    server: Server,
    hub: Hub,
    project: Project,
    /// The change side: the undo watch and the counted plan steps.
    changes: Changes,
    mcu: Mcu,
    /// When the Mackie Control's ports were opened, and whether the install was logged.
    mcu_opened: Option<Instant>,
    mcu_said: bool,
    refreshed_at: Option<Instant>,
    /// The last selection and transport sent as events.
    last_selection: Option<Value>,
    last_transport: Option<Value>,
    last_epoch: u32,
    /// When the bridge attached and when the first complete epoch (headers and regions read) came: the lab's number.
    attached_at: Instant,
    first_epoch_at: Option<Instant>,
    /// When the selected track's strip was last read for the undo watch's strip probes.
    strip_read_at: Option<Instant>,
}

/// How often the Inspector's strip is read for a strip probe (a plug-in or a send Kumi put on the selected track).
const STRIP_EVERY: Duration = Duration::from_secs(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lines logged when they change, not on every tick.
#[derive(Default)]
struct Notes {
    last: HashMap<&'static str, String>,
}

impl Notes {
    fn say(&mut self, topic: &'static str, message: String) {
        if self.last.get(topic) != Some(&message) {
            log::line(&message);
            self.last.insert(topic, message);
        }
    }

    fn clear(&mut self, topic: &'static str) {
        self.last.remove(topic);
    }
}

enum Signal {
    /// The Logic attached to is gone.
    Quit,
    /// A Logic launched while none was attached: look for it now.
    Launched,
}

fn on_app_event(event: &Value, attached: Option<&Attached>) -> Option<Signal> {
    if event["event"] != "app" {
        return None;
    }
    let pid = event["pid"].as_u64();
    match event["state"].as_str() {
        Some("terminated") if attached.is_some_and(|attached| Some(u64::from(attached.pid)) == pid) => {
            log::line("Logic Pro terminated");
            Some(Signal::Quit)
        }
        Some("launched") if attached.is_none() => Some(Signal::Launched),
        _ => None,
    }
}

/// Logic's app bundle: the one named, else the running app's, else the standard place.
fn bundle_of(app: &App, config: &Config) -> std::path::PathBuf {
    config
        .bundle
        .clone()
        .or_else(|| {
            app.path.as_deref().and_then(|path| {
                if path.ends_with(".app") {
                    Some(path.into())
                } else {
                    logic::bundle_of_executable(path).map(Into::into)
                }
            })
        })
        .unwrap_or_else(|| Path::new("/Applications/Logic Pro.app").to_path_buf())
}

/// Logic's string tables for the language it runs in, from its bundle (the app's path from the helper, else the
/// standard place).
fn strings_for(app: &App, config: &Config) -> Strings {
    let bundle = bundle_of(app, config);
    let language = strings::language_of(&strings::preferred_languages(&app.bundle));
    let strings = Strings::load(&bundle, &language);
    if language != "en" {
        log::line(&format!("Logic Pro runs in {language}: {} labels translated from {}", strings.len(), bundle.display()));
    }
    strings
}

fn attach(config: &Config, app: &App, notes: &mut Notes) -> Option<Attached> {
    let version = logic::app_version(app);
    let listener = match config.server.bind() {
        Ok(listener) => listener,
        Err(error) => {
            notes.say("listen", format!("couldn't listen on Logic's ports: {error}"));
            return None;
        }
    };
    let model = Arc::new(Mutex::new(Model::attach(app, &version)));
    let hub = Hub::default();
    let state = Shared2::new(model.clone());
    let (focus, queue) = (state.focus.clone(), state.queue.clone());
    match Server::start(listener, config.server.clone(), hub.clone(), state) {
        Ok(server) => {
            notes.clear("listen");
            notes.clear("waiting");
            log::line(&format!(
                "attached to Logic Pro {} (pid {}, {}); serving on 127.0.0.1:{}",
                if version.is_empty() { "of an unknown version" } else { version.as_str() },
                app.pid,
                app.bundle,
                server.port()
            ));
            // The producer's key command set: their preferences over the preset they chose, cached for the run.
            let mut changes = Changes::default();
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            changes.keys = crate::keys::KeyCommands::load(&bundle_of(app, config), home.as_deref());
            let (names, bindings) = changes.keys.counts();
            log::line(&format!("key commands: {names} names, {bindings} bindings from {:?}", changes.keys.source));
            // Where Logic's playhead stands within its beat isn't known yet: the Mackie Control's timecode will say.
            let mut project = Project::new(strings_for(app, config));
            project.playhead_exact = false;
            Some(Attached {
                pid: app.pid,
                model,
                focus,
                queue,
                server,
                hub,
                project,
                changes,
                mcu: Mcu::default(),
                mcu_opened: None,
                mcu_said: false,
                refreshed_at: None,
                last_selection: None,
                last_transport: None,
                last_epoch: 1,
                attached_at: Instant::now(),
                first_epoch_at: None,
                strip_read_at: None,
            })
        }
        Err(error) => {
            notes.say("listen", format!("couldn't start the server: {error}"));
            None
        }
    }
}

fn describe(project: &Option<wire::Project>) -> String {
    match project {
        None => "no project".into(),
        Some(project) => match (&project.path, project.extra.get("identity").and_then(Value::as_str)) {
            (Some(path), _) => format!("project {} ({path})", project.name.as_deref().unwrap_or("")),
            (None, Some(identity)) => format!("unsaved project {identity:?}"),
            (None, None) => "a project".into(),
        },
    }
}

impl Attached {
    /// A look at Logic's windows: the project and whether Logic can take work.
    fn poll_windows(&mut self, helper: &mut dyn Helper, notes: &mut Notes) {
        match helper.call("windows", json!({"pid": self.pid})) {
            Ok(reply) => {
                let (changed, busy, project) = {
                    let mut model = lock(&self.model);
                    let changed = model.on_windows(&reply);
                    (changed, model.busy, model.project.clone())
                };
                notes.say("busy", busy.map_or("Logic Pro can take work".into(), |busy| format!("busy: {}", busy.reason())));
                if changed {
                    log::line(&describe(&project));
                    self.server.publish(&lock(&self.model).clone());
                    self.hub.event(wire::event::PROJECT, serde_json::to_value(&project).unwrap_or(Value::Null));
                    let (id, path, name) =
                        project.as_ref().map(|p| (p.id.clone(), p.path.clone(), p.name.clone().unwrap_or_default())).unwrap_or_default();
                    self.project.set_identity(id.as_deref(), path.as_deref(), &name);
                    // Another project: its epoch starts afresh, and so does what was read.
                    if self.first_epoch_at.is_some() {
                        self.project.epoch += 1;
                    }
                    self.project.new_project();
                }
            }
            Err(error) => notes.say("helper", format!("the helper didn't answer `windows`: {error}")),
        }
    }

    /// One refresh of the focus cache, and the events it deserves.
    fn refresh(&mut self, helper: &mut dyn Helper, notes: &mut Notes) {
        let busy = lock(&self.model).busy;
        if busy.is_some() {
            // Nothing of the window can be read: the cache keeps what it had, and says why.
            lock(&self.focus).busy = busy.map(logic::Busy::code);
            return;
        }
        let root = lock(&self.model).window_root();
        self.project.set_root(root);
        let mut ax = Ax::new(helper, self.pid);
        let (look, headers) = match self.project.look(&mut ax, None) {
            Ok((look, headers, _)) => (look, headers),
            Err(error) => {
                if matches!(error, AxError::Missing(_)) {
                    // The window changed shape (a pane opened, a project loaded): find its parts again next time.
                    self.project.layout = Default::default();
                }
                notes.say("read", format!("couldn't refresh the focus: {error}"));
                return;
            }
        };
        notes.clear("read");
        if look.transport.playing || look.transport.recording {
            self.project.playhead_exact = false;
        }
        if self.first_epoch_at.is_none() {
            self.first_epoch_at = Some(Instant::now());
            log::line(&format!("the project read in {} ms after attaching", self.attached_at.elapsed().as_millis()));
        }
        self.refreshed_at = Some(Instant::now());
        let epoch = look.epoch;
        let (selection, transport) = (look.selection.to_json(epoch), look.transport.to_json());
        // A strip probe watches the selected track: its strip read from the Inspector, at most once a second.
        if let Some((place, _)) = &look.selection.track {
            if self.changes.watch.strip_wanted(*place) && self.strip_read_at.is_none_or(|at| at.elapsed() >= STRIP_EVERY) {
                self.strip_read_at = Some(Instant::now());
                if let Ok(Some(strip)) = self.project.inspector_strip(&mut ax) {
                    self.changes.watch.saw_strip(crate::changes::undo::Seen {
                        place: *place,
                        plugins: strip.inserts.iter().filter(|(_, _, kind)| *kind == "effect").map(|(name, _, _)| name.clone()).collect(),
                        sends: strip.sends.iter().map(|(bus, _)| bus.clone()).collect(),
                    });
                }
            }
        }
        // The producer's own undo of Kumi's changes, named back: the probes against this look, the Edit menu's titles.
        {
            let hub = &self.hub;
            let project = &self.project;
            self.changes.watch.poll(
                &mut ax,
                &project.strings,
                &look,
                &headers,
                &mut |data| hub.event(wire::event::UNDONE_IN_DAW, data),
                &|from, place| project.carry_track(from, place),
            );
        }
        lock(&self.focus).update(look);
        if epoch != self.last_epoch {
            self.last_epoch = epoch;
            self.hub.event(wire::event::TRACKS, json!({"epoch": epoch.to_string()}));
        }
        if self.last_selection.as_ref() != Some(&selection) {
            let mut data = selection.clone();
            data["epoch"] = epoch.to_string().into();
            if self.last_selection.is_some() {
                self.hub.event(wire::event::SELECTION, data);
            }
            self.last_selection = Some(selection);
        }
        if self.last_transport.as_ref() != Some(&transport) {
            if self.last_transport.is_some() {
                self.hub.event(wire::event::TRANSPORT, transport.clone());
            }
            self.last_transport = Some(transport);
        }
    }

    /// Answers one queued request through the project reader. A write (a change applied or put back, the transport,
    /// the playhead, a save) refreshes the focus before its answer leaves, so the host's next `focus` reads what the
    /// write left and not what the tick before it saw: the M4 gate read the cycle one write behind otherwise.
    fn serve(&mut self, job: Job, helper: &mut dyn Helper, notes: &mut Notes) {
        let answer = self.answer(&job.method, &job.params, helper);
        if WRITES.contains(&job.method.as_str()) {
            self.refresh(helper, notes);
        }
        job.answer(answer);
    }

    fn answer(&mut self, method: &str, params: &Value, helper: &mut dyn Helper) -> Answer {
        if let Some(busy) = lock(&self.model).busy {
            return Err((wire::code::BUSY.into(), format!("{}; nothing changed", busy.reason())));
        }
        let root = lock(&self.model).window_root();
        self.project.set_root(root);
        if crate::serve::CHANGE_METHODS.contains(&method) {
            return crate::serve::answer_change(&mut self.project, &mut self.changes, helper, self.pid, method, params);
        }
        if crate::serve::LISTEN_METHODS.contains(&method) {
            // Logic plays for a while: the hosts keep hearing the bridge's heartbeats meanwhile.
            let hub = &self.hub;
            let mcu = if self.mcu.open { Some(&mut self.mcu) } else { None };
            return crate::serve::answer_listen(&mut self.project, mcu, helper, self.pid, method, params, &mut || hub.heartbeat());
        }
        crate::serve::answer_keyed(&mut self.project, helper, self.pid, method, params, &self.changes.keys)
    }

    /// The Mackie Control: opened once Logic can be read, its bytes answered on every event.
    fn drive_mcu(&mut self, helper: &mut dyn Helper, notes: &mut Notes) {
        if self.mcu_opened.is_none() {
            match self.mcu.open(helper) {
                Ok(()) => {
                    self.mcu_opened = Some(Instant::now());
                    log::line(&format!(
                        "Mackie Control ports \"{}\" opened; waiting for Logic Pro to install the surface",
                        crate::mcu::PORT_NAME
                    ));
                    self.mcu.pump(helper, MCU_INSTALL);
                }
                Err(error) => notes.say("mcu", format!("couldn't open the Mackie Control ports: {error}")),
            }
        }
        if !self.mcu_said && self.mcu.surface.state.installed {
            self.mcu_said = true;
            log::line(&format!(
                "Logic Pro installed the Mackie Control {} ms after its first device query; the LCD says {:?}",
                self.mcu.install_ms().unwrap_or(0),
                self.mcu.surface.state.names()
            ));
        }
    }

    fn on_midi(&mut self, event: &Value, helper: &mut dyn Helper) {
        let reaction = self.mcu.on_event(event, helper);
        // The timecode says where the playhead stands within its beat, which the LCD doesn't: placing it from a
        // division or a tick past 1 goes to the project's start first.
        let state = &self.mcu.surface.state;
        if state.installed && state.beats_mode() {
            if let Some([_, _, division, tick]) = state.position() {
                self.project.playhead_exact = !state.playing && !state.recording && division == 1 && tick == 1;
            }
        }
        if reaction.transport_changed {
            // The LEDs say before the control bar does: the next refresh carries the detail; the event goes now.
            let state = &self.mcu.surface.state;
            let mut transport = self.last_transport.clone().unwrap_or_else(|| json!({}));
            transport["playing"] = state.playing.into();
            transport["recording"] = state.recording.into();
            transport["cycle"] = state.cycle.into();
            self.hub.event(wire::event::TRANSPORT, transport);
        }
    }
}

/// Runs until Logic quits or `stop` is set: the hosts are told goodbye, the discovery file removed, the helper ended.
pub fn run(config: Config, mut helper: Box<dyn Helper>, stop: Arc<AtomicBool>) -> Outcome {
    let heartbeat = Duration::from_millis(HEARTBEAT_MS);
    let mut attached: Option<Attached> = None;
    let mut apps_at: Option<Instant> = None;
    let mut windows_at: Option<Instant> = None;
    let mut heartbeat_at = Instant::now();
    let mut prompted = false;
    let mut notes = Notes::default();
    let outcome = 'run: loop {
        if stop.load(Ordering::Acquire) {
            break Outcome::Stopped;
        }
        let tick = Instant::now();
        // Events first: a Logic that terminated ends the run before anything else is asked.
        while let Some(event) = helper.event(Duration::ZERO) {
            match on_app_event(&event, attached.as_ref()) {
                Some(Signal::Quit) => break 'run Outcome::LogicQuit,
                Some(Signal::Launched) => apps_at = None,
                None => {
                    if let Some(attached) = attached.as_mut() {
                        attached.on_midi(&event, helper.as_mut());
                    }
                }
            }
        }
        if apps_at.is_none_or(|at| at.elapsed() >= config.apps_every) {
            match helper.call("apps", json!({"bundles": BUNDLES})) {
                Ok(reply) => {
                    apps_at = Some(Instant::now());
                    notes.clear("helper");
                    let apps = logic::parse_apps(&reply);
                    match &attached {
                        Some(attached) if !apps.iter().any(|app| app.pid == attached.pid) => {
                            log::line("Logic Pro is no longer running");
                            break Outcome::LogicQuit;
                        }
                        Some(_) => {}
                        None => match logic::newest(&apps) {
                            Some(app) => attached = attach(&config, app, &mut notes),
                            None => notes.say("waiting", "waiting for Logic Pro to run".into()),
                        },
                    }
                }
                Err(error) => notes.say("helper", format!("the helper didn't answer `apps`: {error}")),
            }
        }
        if let Some(attached) = attached.as_mut() {
            if windows_at.is_none_or(|at| at.elapsed() >= config.poll) {
                windows_at = Some(Instant::now());
                // The first check may prompt for Accessibility (macOS shows its dialog once); the later ones only ask.
                let prompt = !std::mem::replace(&mut prompted, true);
                match helper.call("trusted", json!({"prompt": prompt})) {
                    Ok(reply) => {
                        notes.clear("helper");
                        let trusted = reply["trusted"] == Value::Bool(true);
                        if lock(&attached.model).on_trusted(trusted) {
                            log::line(if trusted {
                                "Accessibility is granted"
                            } else {
                                "Accessibility isn't granted to the helper: nothing of Logic Pro can be read until it is"
                            });
                        }
                        if trusted {
                            attached.poll_windows(helper.as_mut(), &mut notes);
                        }
                    }
                    Err(error) => notes.say("helper", format!("the helper didn't answer `trusted`: {error}")),
                }
                // Written once attached, again when the project changed, and tried again in a while after a failure.
                let snapshot = lock(&attached.model).clone();
                attached.server.publish(&snapshot);
            }
            let trusted = lock(&attached.model).permission.is_none();
            if trusted {
                if attached.refreshed_at.is_none_or(|at| at.elapsed() >= config.refresh) {
                    attached.refresh(helper.as_mut(), &mut notes);
                }
                if config.mcu && attached.refreshed_at.is_some() {
                    attached.drive_mcu(helper.as_mut(), &mut notes);
                }
            }
            for _ in 0..JOBS_PER_TICK {
                let Some(job) = attached.queue.take() else { break };
                if !trusted {
                    job.answer(Err((wire::code::FAILED.into(), server::UNTRUSTED.into())));
                    continue;
                }
                attached.serve(job, helper.as_mut(), &mut notes);
            }
        }
        if heartbeat_at.elapsed() >= heartbeat {
            if let Some(attached) = &attached {
                attached.hub.heartbeat();
            }
            heartbeat_at = Instant::now();
        }
        // The rest of the tick, spent waiting for an event rather than asleep, in short waits so a queued request
        // is answered soon.
        let period = if attached.is_some() { config.refresh } else { config.poll };
        let left = period.saturating_sub(tick.elapsed()).min(Duration::from_millis(20));
        if let Some(event) = helper.event(left) {
            match on_app_event(&event, attached.as_ref()) {
                Some(Signal::Quit) => break Outcome::LogicQuit,
                Some(Signal::Launched) => apps_at = None,
                None => {
                    if let Some(attached) = attached.as_mut() {
                        attached.on_midi(&event, helper.as_mut());
                    }
                }
            }
        }
    };
    if let Some(mut attached) = attached.take() {
        attached.changes.stop();
        attached.mcu.close(helper.as_mut());
        attached.server.stop(outcome.bye());
    }
    helper.quit();
    outcome
}
