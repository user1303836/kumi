//! One run of the bridge: find Logic through the helper, attach to it, serve on loopback, follow its project, and stop
//! when Logic quits or the process is asked to.

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

use crate::helper::Helper;
use crate::log;
use crate::logic::{self, App, Model, BUNDLES, POLL};
use crate::server::{self, Hub, Server};

/// How often the running apps are asked for: while waiting for Logic, and once attached as the check that it still
/// runs (its `terminated` event is the quick way; this is the sure one).
pub const APPS_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct Config {
    pub server: server::Config,
    /// How often Logic's windows are read.
    pub poll: Duration,
    pub apps_every: Duration,
}

impl Config {
    pub fn from_reference(reference: &Reference) -> Config {
        Config { server: server::Config::from_reference(reference), poll: POLL, apps_every: APPS_EVERY }
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
        })
    }

    /// Other intervals than the defaults, for tests.
    pub fn with_timing(mut self, poll: Duration, apps_every: Duration) -> Config {
        self.poll = poll;
        self.apps_every = apps_every;
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

/// The Logic attached to, the server for it and the hub to its connections.
struct Attached {
    pid: u32,
    model: Arc<Mutex<Model>>,
    server: Server,
    hub: Hub,
}

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

fn on_event(event: &Value, attached: Option<&Attached>) -> Option<Signal> {
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
    match Server::start(listener, config.server.clone(), hub.clone(), model.clone()) {
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
            Some(Attached { pid: app.pid, model, server, hub })
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

/// Runs until Logic quits or `stop` is set: the hosts are told goodbye, the discovery file removed, the helper ended.
pub fn run(config: Config, mut helper: Box<dyn Helper>, stop: Arc<AtomicBool>) -> Outcome {
    let heartbeat = Duration::from_millis(HEARTBEAT_MS);
    let mut attached: Option<Attached> = None;
    let mut apps_at: Option<Instant> = None;
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
            match on_event(&event, attached.as_ref()) {
                Some(Signal::Quit) => break 'run Outcome::LogicQuit,
                Some(Signal::Launched) => apps_at = None,
                None => {}
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
        if stop.load(Ordering::Acquire) {
            break Outcome::Stopped;
        }
        if let Some(attached) = attached.as_mut() {
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
                    if trusted && !stop.load(Ordering::Acquire) {
                        match helper.call("windows", json!({"pid": attached.pid})) {
                            Ok(reply) => {
                                let (changed, busy, project) = {
                                    let mut model = lock(&attached.model);
                                    let changed = model.on_windows(&reply);
                                    (changed, model.busy, model.project.clone())
                                };
                                notes.say("busy", busy.map_or("Logic Pro can take work".into(), |busy| format!("busy: {}", busy.reason())));
                                if changed {
                                    log::line(&describe(&project));
                                    attached.server.publish(&lock(&attached.model).clone());
                                    attached.hub.event(wire::event::PROJECT, serde_json::to_value(&project).unwrap_or(Value::Null));
                                }
                            }
                            Err(error) => notes.say("helper", format!("the helper didn't answer `windows`: {error}")),
                        }
                    }
                }
                Err(error) => notes.say("helper", format!("the helper didn't answer `trusted`: {error}")),
            }
            // Written once attached, again when the project changed, and tried again in a while after a failure.
            let snapshot = lock(&attached.model).clone();
            attached.server.publish(&snapshot);
        }
        if stop.load(Ordering::Acquire) {
            break Outcome::Stopped;
        }
        if heartbeat_at.elapsed() >= heartbeat {
            if let Some(attached) = &attached {
                attached.hub.heartbeat();
            }
            heartbeat_at = Instant::now();
        }
        // The rest of the tick, spent waiting for an event rather than asleep.
        let left = config.poll.saturating_sub(tick.elapsed());
        if let Some(event) = helper.event(left) {
            match on_event(&event, attached.as_ref()) {
                Some(Signal::Quit) => break Outcome::LogicQuit,
                Some(Signal::Launched) => apps_at = None,
                None => {}
            }
        }
    };
    // End the native helper before socket peers get their bounded chance to finish. A stuck peer must never keep
    // the helper alive until the host kills the bridge after closing stdin.
    helper.quit();
    if let Some(attached) = attached.take() {
        attached.server.stop(outcome.bye());
    }
    outcome
}
