//! The runtime's connection to its DAW (`connect`) and what it has seen of the open project (`observe`): the project's
//! key and epoch every write is checked against, the focus and transport as the DAW reports them, and what changed
//! while no one looked.
//!
//! The integration is the one Kumi's app uses, built with the plugin's host policy: nothing of Kumi's identity is
//! written into the project, listening keeps to the runtime's scratch, and no crash restoration runs (there's no
//! restore journal). Kumi-side records (Live's baselines) go under the runtime's state folder.

use super::ctx::{Ctx, OpError};
use super::say;
use kumi_agent::core::context::{HostPolicy, IntegrationContext};
use kumi_agent::core::contracts::{CatchUp, ConnectionState, Integration, Observation, Reference};
use kumi_agent::core::view::{DawFocus, Transport};
use kumi_daw::component::same_folder;
use kumi_daw::provider::DawSession;
use kumi_types::contracts::{JsonObject, KernelTool, ToolRole};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::Duration;

/// How many of the project's things (tracks, devices, clips) `observe` lists with their refs.
const MOST_REFS: usize = 200;
/// How long the integration gets to close when the runtime stops.
const CLOSING: Duration = Duration::from_secs(5);

/// Each `{ref, name}` a look's context shows, by its ref: a track by its name, and what's inside one (a device, a chain,
/// a clip) as "<name> (on <track>)", as a look's `refs` say it. A name with its track wins over one without it (a
/// selection shows a device alone).
pub(super) fn look_names(context: &JsonObject) -> HashMap<String, String> {
    fn walk(value: &Value, on: Option<&str>, names: &mut HashMap<String, (String, bool)>) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| walk(item, on, names)),
            Value::Object(fields) => {
                let here = fields.get("ref").and_then(Value::as_str).zip(fields.get("name").and_then(Value::as_str));
                if let Some((reference, name)) = here {
                    if names.get(reference).is_none_or(|(_, placed)| on.is_some() && !placed) {
                        let named = on.map_or_else(|| name.to_string(), |track| format!("{name} (on {track})"));
                        names.insert(reference.to_string(), (named, on.is_some()));
                    }
                }
                let inside = on.or(here.map(|(_, name)| name));
                fields.values().for_each(|item| walk(item, inside, names));
            }
            _ => {}
        }
    }
    let mut names = HashMap::new();
    context.values().for_each(|value| walk(value, None, &mut names));
    names.into_iter().map(|(reference, (named, _))| (reference, named)).collect()
}

/// Each track's fader as a look's context shows it, by its ref: Live's track rows carry `volume`, a display string such
/// as "-6.0 dB", while the Set has few enough tracks for the mixer fields. What a fader change's approval says it
/// changes from, never read from the DAW for the preview.
pub(super) fn look_volumes(context: &JsonObject) -> HashMap<String, String> {
    fn walk(value: &Value, volumes: &mut HashMap<String, String>) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| walk(item, volumes)),
            Value::Object(fields) => {
                let here = fields.get("ref").and_then(Value::as_str).zip(fields.get("volume").and_then(Value::as_str));
                if let Some((reference, volume)) = here {
                    volumes.entry(reference.to_string()).or_insert_with(|| volume.to_string());
                }
                fields.values().for_each(|item| walk(item, volumes));
            }
            _ => {}
        }
    }
    let mut volumes = HashMap::new();
    context.values().for_each(|value| walk(value, &mut volumes));
    volumes
}

/// The project as the runtime last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The open project's key (`p-` and 16 hex digits): a different project, or the same one opened again, has another.
    pub key: String,
    pub name: String,
    /// Which look at this project the refs from `observe` belong to: it moves on when the project changes, the DAW
    /// reconnects, or a write's outcome is unsure. A write names the epoch it was planned on, and a stale one is refused.
    pub epoch: u64,
    /// When it was seen (ms).
    pub seen_at: i64,
    /// The DAW's own epoch the look read it at (an own bridge's snapshot epoch, a Live look's): with the project, so a
    /// caller that checks it ([`Link::expect`]) has the epoch of that same look, with no second read between.
    pub native_epoch: Option<String>,
}

/// What the integration reports between looks, kept for `observe` and `focus.json`.
#[derive(Default)]
struct Reported {
    connected: Cell<bool>,
    /// The DAW has answered once: an answer after it went away is a return.
    answered: Cell<bool>,
    /// How many times the connection came back after it went.
    returns: Cell<u64>,
    focus: RefCell<Option<DawFocus>>,
    transport: RefCell<Option<Transport>>,
    catch_up: RefCell<Vec<CatchUp>>,
}

/// The runtime's live connection to the DAW.
pub struct Connection {
    pub integration: Rc<dyn Integration>,
    reported: Rc<Reported>,
}

impl Connection {
    /// Whether the DAW answers now, as the integration last said.
    pub fn answering(&self) -> bool {
        self.reported.connected.get()
    }

    /// The focus as the model is shown it ([`Integration::model_focus`]): its refs in the form the look and a plan's
    /// steps take, so FOCUS and the look number a track alike (Main is `track:7` in both).
    pub fn focus(&self) -> Option<DawFocus> {
        self.reported.focus.borrow().as_ref().map(|focus| self.integration.model_focus(focus))
    }

    pub fn transport(&self) -> Option<Transport> {
        self.reported.transport.borrow().clone()
    }

    /// What changed while no one looked, since the last time this was asked: bounded (see [`bounded_catch_up`]).
    pub fn take_catch_up(&self) -> Vec<CatchUp> {
        bounded_catch_up(std::mem::take(&mut *self.reported.catch_up.borrow_mut()))
    }

    /// The same, kept for the look that asks for it.
    pub fn catch_up(&self) -> Vec<CatchUp> {
        bounded_catch_up(self.reported.catch_up.borrow().clone())
    }
}

/// The most catch-ups (projects) kept and given, the newest; and the most lines given for each (the rest counted in
/// `more`).
const CATCH_UPS: usize = 5;
const CATCH_UP_LINES: usize = 20;

/// `catch_up` as the runtime gives it (CC-7): the newest [`CATCH_UPS`], each with at most [`CATCH_UP_LINES`] lines.
pub fn bounded_catch_up(mut catch_up: Vec<CatchUp>) -> Vec<CatchUp> {
    if catch_up.len() > CATCH_UPS {
        catch_up.drain(..catch_up.len() - CATCH_UPS);
    }
    for seen in &mut catch_up {
        if seen.lines.len() > CATCH_UP_LINES {
            seen.more += seen.lines.len() - CATCH_UP_LINES;
            seen.lines.truncate(CATCH_UP_LINES);
        }
    }
    catch_up
}

/// The runtime's side of the connection: none until `connect`, then the connection and the project last seen.
pub struct Link {
    connection: RefCell<Option<Rc<Connection>>>,
    /// Held while a `connect` starts the integration, so a second one waits for it rather than starting another.
    connecting: tokio::sync::Mutex<()>,
    project: RefCell<Option<Project>>,
    /// The connection's returns when the project was last seen: a return since makes every plan stale.
    returns_seen: Cell<u64>,
    epoch: Cell<u64>,
    /// The tools the last look offered (the plan's among them).
    tools: RefCell<Vec<Rc<dyn KernelTool>>>,
    /// The names the last look gave its refs, for saying what a plan's steps are on.
    ref_names: RefCell<HashMap<String, String>>,
    /// Each track's fader as the last look showed it ([`look_volumes`]), for saying what a fader change changes from.
    ref_volumes: RefCell<HashMap<String, String>>,
    /// A write whose outcome is unsure, until the next look (CC-2's R6): `{at, op, reason, cleanup}`.
    unsure: RefCell<Option<Value>>,
    /// The DAW's own epoch the last look read, taken with that look: an own bridge's page of another epoch (a tab
    /// switched since) isn't the looked project's. Never read again after the look, so a switch between the two can't
    /// pair the looked project's key with another's epoch.
    native_epoch: RefCell<Option<String>>,
    /// What each previewed plan with files approved (CC-11's 11b), by its host call (or, a call without one, by its
    /// plan): its dispatch must be the same plan, placing the same files. The newest [`PINNED`].
    pinned: RefCell<std::collections::VecDeque<Pin>>,
}

/// A preview's approval of a plan with files: the call it's for, the plan as approved (digested as the host's digest
/// reads it) and each file's identity.
struct Pin {
    key: String,
    approved: String,
    files: Vec<Value>,
}

/// The most previewed plans whose files are kept for their dispatch.
const PINNED: usize = 64;

impl Link {
    /// No connection yet. Epochs count up from `base` (the runtime's start, ms), so no plan made in an earlier
    /// runtime carries an epoch this one gives out.
    pub fn new(base: u64) -> Link {
        Link {
            connection: RefCell::new(None),
            connecting: tokio::sync::Mutex::new(()),
            project: RefCell::new(None),
            returns_seen: Cell::new(0),
            epoch: Cell::new(base),
            tools: RefCell::new(Vec::new()),
            ref_names: RefCell::new(HashMap::new()),
            ref_volumes: RefCell::new(HashMap::new()),
            unsure: RefCell::new(None),
            native_epoch: RefCell::new(None),
            pinned: RefCell::new(std::collections::VecDeque::new()),
        }
    }

    pub fn connection(&self) -> Option<Rc<Connection>> {
        self.connection.borrow().clone()
    }

    /// A connection to `integration` that answers, as `connect` makes one: for an operation's tests.
    #[cfg(test)]
    pub(super) fn connected_to(&self, integration: Rc<dyn Integration>) {
        let reported = Rc::new(Reported::default());
        reported.connected.set(true);
        reported.answered.set(true);
        *self.connection.borrow_mut() = Some(Rc::new(Connection { integration, reported }));
    }

    pub fn project(&self) -> Option<Project> {
        self.project.borrow().clone()
    }

    /// A connection to `integration` that answers: a test's own DAW.
    #[cfg(test)]
    pub(crate) fn connect_for_test(&self, integration: Rc<dyn Integration>) {
        let reported = Rc::new(Reported::default());
        reported.connected.set(true);
        reported.answered.set(true);
        *self.connection.borrow_mut() = Some(Rc::new(Connection { integration, reported }));
    }

    /// The DAW's plan tool, as the last look offered it (`make_changes`).
    pub fn plan_tool(&self) -> Option<Rc<dyn KernelTool>> {
        self.tools.borrow().iter().find(|tool| tool.role() == Some(ToolRole::Plan)).cloned()
    }

    /// The tools the last look offered.
    pub fn tools(&self) -> Vec<Rc<dyn KernelTool>> {
        self.tools.borrow().clone()
    }

    /// The names the last look gave its refs, for saying what a plan's steps are on ([`Link::keep_refs`]).
    pub fn ref_names(&self) -> HashMap<String, String> {
        self.ref_names.borrow().clone()
    }

    /// Each track's fader as the last look showed it, by its ref ([`look_volumes`]).
    pub fn ref_volumes(&self) -> HashMap<String, String> {
        self.ref_volumes.borrow().clone()
    }

    /// Keeps each track's fader as a look showed it, replacing the last look's.
    pub fn keep_volumes(&self, volumes: HashMap<String, String>) {
        *self.ref_volumes.borrow_mut() = volumes;
    }

    /// The names a look gave its refs, kept for naming what a plan is on: `names`, what its context shows by the refs a
    /// plan's steps take ([`look_names`]), then its `refs`' labels with their detail. A DAW whose `refs` are in another
    /// form than its steps take (Live's long refs) is still named by what the look showed.
    pub fn keep_refs(&self, refs: &[Value], mut names: HashMap<String, String>) {
        names.extend(refs.iter().filter_map(|candidate| {
            let reference = candidate["reference"]["node"]["ref"].as_str()?;
            let label = candidate["label"].as_str()?;
            let named = match candidate["detail"].as_str() {
                Some(detail) => format!("{label} ({detail})"),
                None => label.to_string(),
            };
            Some((reference.to_string(), named))
        }));
        *self.ref_names.borrow_mut() = names;
    }

    /// A write ended unsure: said in `status.json` until the next completed `observe` (R6).
    pub fn note_unsure(&self, op: &str, reason: &str, cleanup: &str) {
        *self.unsure.borrow_mut() = Some(json!({"at": kumi_common::time::now_ms(), "op": op, "reason": reason, "cleanup": cleanup}));
    }

    pub fn unsure(&self) -> Option<Value> {
        self.unsure.borrow().clone()
    }

    /// A preview of the call `key` names approved the plan `approved` digests, which places these files: its dispatch
    /// must be that plan, with the same files.
    pub fn pin_files(&self, key: &str, approved: &str, files: Vec<Value>) {
        let mut pinned = self.pinned.borrow_mut();
        pinned.retain(|pin| pin.key != key);
        pinned.push_back(Pin { key: key.to_string(), approved: approved.to_string(), files });
        while pinned.len() > PINNED {
            pinned.pop_front();
        }
    }

    /// What the preview of the call `key` names approved, taken: a dispatch holds to it once.
    pub fn take_pin(&self, key: &str) -> Option<(String, Vec<Value>)> {
        let mut pinned = self.pinned.borrow_mut();
        let at = pinned.iter().position(|pin| pin.key == key)?;
        pinned.remove(at).map(|pin| (pin.approved, pin.files))
    }

    /// An `observe` completed: the model sees the project as it is now, so nothing is unsure any more (R6).
    pub fn clear_unsure(&self) {
        *self.unsure.borrow_mut() = None;
    }

    /// The project `observation` is about, as the runtime keys it (the integration's key with the DAW's identity for
    /// the open project, so a copy open in another tab, or the same file opened again, is another): the epoch moves
    /// on when it's another project than last seen, or the DAW came back since. The DAW's own epoch is kept from the
    /// same observation, so the looked page epoch is always the looked project's.
    pub fn saw(&self, observation: &Observation) -> Project {
        let open =
            observation.set.as_deref().or_else(|| observation.identity.as_ref().and_then(|identity| identity.open_identity.as_deref()));
        let key = project_key(&observation.key, open);
        let returns = self.connection().map_or(0, |connection| connection.reported.returns.get());
        let changed = self.project.borrow().as_ref().is_none_or(|last| last.key != key) || returns != self.returns_seen.get();
        if changed {
            self.epoch.set(self.epoch.get() + 1);
        }
        self.returns_seen.set(returns);
        let name = observation.identity.as_ref().map_or_else(|| observation.label.clone(), |identity| identity.name.clone());
        let native_epoch = observation.native_epoch.clone();
        let project =
            Project { key, name, epoch: self.epoch.get(), seen_at: kumi_common::time::now_ms(), native_epoch: native_epoch.clone() };
        *self.project.borrow_mut() = Some(project.clone());
        *self.tools.borrow_mut() = observation.tools.clone();
        *self.native_epoch.borrow_mut() = native_epoch;
        project
    }

    /// The DAW's own epoch the last look read, taken with that look (an own bridge's from its snapshot): None when the
    /// DAW gave none or no look read the project. A capture names it, so another project's state is never taken for the
    /// looked one. Where the project is checked too, take it from [`Link::expect`]'s [`Project`], the same look's.
    pub fn observed_native_epoch(&self) -> Option<String> {
        self.native_epoch.borrow().clone()
    }

    /// Whether an own bridge's page with this epoch is of the state of the project the last look saw: both known, and
    /// the same.
    pub fn page_is_looked(&self, epoch: Option<&str>) -> bool {
        matches!((self.native_epoch.borrow().as_deref(), epoch), (Some(looked), Some(now)) if looked == now)
    }

    /// The epoch the refs of the last look are of, moved on since by any write's unsure outcome.
    pub fn epoch(&self) -> u64 {
        self.epoch.get()
    }

    /// Moves the epoch on (a write's outcome is unsure): refs from earlier looks no longer hold, and neither do the faders
    /// the last look showed, which a fader change's preview would otherwise say it changes from.
    pub fn advance(&self) {
        self.epoch.set(self.epoch.get() + 1);
        if let Some(project) = self.project.borrow_mut().as_mut() {
            project.epoch = self.epoch.get();
        }
        self.ref_volumes.borrow_mut().clear();
    }

    /// The write's plan was made on `key` at `epoch`: refused unless that's the project and epoch now, the DAW answers,
    /// and it hasn't gone and come back since the last look.
    pub fn expect(&self, key: &str, epoch: u64) -> Result<Project, OpError> {
        let project = self.expect_project(key)?;
        if project.epoch != epoch {
            let reason = "The project has changed since that plan was made (its refs no longer hold): observe again.";
            return Err(OpError::Refused { code: "stale", reason: reason.into(), owner: None });
        }
        Ok(project)
    }

    /// A write on `key`'s project that no epoch binds (an undo): refused unless it's the project open now, the DAW
    /// answers, and it hasn't gone and come back since the last look.
    pub fn expect_project(&self, key: &str) -> Result<Project, OpError> {
        let connection = self.connection().filter(|connection| connection.answering());
        let (Some(connection), Some(project)) = (connection, self.project()) else {
            return Err(OpError::NotConnected {
                reason: "Kumi isn't connected to the DAW, or hasn't looked at the project yet: connect and observe first.".into(),
            });
        };
        let stale =
            |reason: &str| -> Result<Project, OpError> { Err(OpError::Refused { code: "stale", reason: reason.into(), owner: None }) };
        if connection.reported.returns.get() != self.returns_seen.get() {
            return stale("The DAW went away and came back since the last look, so its refs may not hold: observe again.");
        }
        if project.key != key {
            return stale("That was made on another project than the one open now: observe again.");
        }
        Ok(project)
    }

    /// Ends the connection (the runtime is stopping): the integration closes, a few seconds at most.
    pub async fn close(&self) {
        let Some(connection) = self.connection.borrow_mut().take() else { return };
        match tokio::time::timeout(CLOSING, connection.integration.close()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => say(&format!("kumi serve: the DAW connection closed with: {error}")),
            Err(_) => say("kumi serve: the DAW connection didn't close in time"),
        }
    }
}

/// The project's key as the runtime gives it out: short, and saying nothing of the project's file or name.
fn project_key(key: &str, open: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(key.as_bytes());
    digest.update([0]);
    digest.update(open.unwrap_or_default().as_bytes());
    format!("p-{}", digest.finalize()[..8].iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

/// Why Kumi's part in the DAW, another Kumi's, isn't used, and what the producer does: for Live, set up again and
/// approve using that installation (setup adopts it).
pub fn owned_elsewhere(daw: kumi_types::daw::DawId) -> String {
    let app = daw.profile().words.app;
    match daw {
        kumi_types::daw::DawId::Ableton => format!(
            "Kumi's part in {app} belongs to another Kumi on this computer. Ask Hermes to set Kumi up again, and approve using that installation."
        ),
        _ => format!("Kumi's part in {app} belongs to another Kumi on this computer: set {app} up for Hermes, or adopt that installation, first."),
    }
}

/// The plugin's host policy for a runtime whose state is `state_dir`: listening keeps to its scratch and quarantine
/// (#1035), and the variations tool keeps its sets under its state (`<state>/variations/`), never the Kumi home, so the
/// plugin's V-numbering and kept takes never mix with standalone Kumi's on the same project (CC-24's W1-e).
pub fn policy(state_dir: &Path) -> HostPolicy {
    HostPolicy::plugin(state_dir.join("scratch").join("listen"), state_dir.join("quarantine"), state_dir.to_path_buf())
}

/// Connects to the DAW: its integration made with the plugin's host policy and started. One start at a time: a second
/// `connect` waits for the first and takes its connection. Refused when Kumi's part isn't in the DAW, when the part in
/// the DAW is another Kumi's (its home isn't `--kumi-home`), and, for REAPER and Renoise, when `--kumi-home` is a home
/// setup adopted (their integrations still keep the project's places and baselines under it).
pub async fn connect(ctx: &Ctx, signal: kumi_common::abort::Signal) -> Result<Rc<Connection>, OpError> {
    let _starting = ctx.link().connecting.lock().await;
    if let Some(connection) = ctx.link().connection() {
        return Ok(connection);
    }
    let Some(provider) = crate::daw::provider(ctx.daw()) else {
        return Err(OpError::Refused {
            code: "unsupported",
            reason: format!("Kumi doesn't work with {} here.", ctx.daw().profile().words.app),
            owner: None,
        });
    };
    let app = provider.profile().words.app;
    let Some(bridge_config) = provider.bridge_config(ctx.env()) else {
        let reason = format!("Kumi's part isn't in {app} yet: set it up first.");
        return Err(OpError::Refused { code: "not_set_up", reason, owner: None });
    };
    let home = &ctx.paths().kumi_home;
    let owner = provider.bridge_owner(&bridge_config, ctx.env());
    if !owner.as_deref().is_some_and(|owner| same_folder(Path::new(owner), home)) {
        let reason = owned_elsewhere(ctx.daw());
        return Err(OpError::Refused { code: "owned_elsewhere", reason, owner: Some(json!({"kumi_home": owner})) });
    }
    let plugins = ctx.paths().state_dir.parent().map(Path::to_path_buf).unwrap_or_default();
    if matches!(ctx.daw(), kumi_types::daw::DawId::Reaper | kumi_types::daw::DawId::Renoise) && !inside(home, &plugins) {
        let reason = format!("Kumi for Hermes can't use {app} through an adopted installation yet: set {app} up for Hermes instead.");
        return Err(OpError::Refused { code: "adopted_home", reason, owner: Some(json!({"kumi_home": home})) });
    }
    let reported = Rc::new(Reported::default());
    let on_connection = {
        let (reported, ctx) = (Rc::downgrade(&reported), ctx.handle());
        Rc::new(move |state: ConnectionState, _cause| {
            let Some(reported) = reported.upgrade() else { return };
            let answering = state == ConnectionState::Connected;
            if answering && !reported.connected.get() {
                if reported.answered.get() {
                    reported.returns.set(reported.returns.get() + 1);
                }
                reported.answered.set(true);
            }
            let changed = reported.connected.get() != answering;
            reported.connected.set(answering);
            if changed {
                // What the host's UI shows follows at once: a DAW gone isn't shown as current.
                let ctx = ctx.clone();
                tokio::task::spawn_local(async move {
                    publish_focus(&ctx);
                    if let Some(ctx) = ctx.upgrade() {
                        super::core::publish_status(&ctx, false).await;
                    }
                });
            }
        })
    };
    let mut context = IntegrationContext::new(on_connection);
    context.policy = policy(ctx.paths().state_dir.as_path());
    // Polled about once a second: nothing draws it as it moves.
    context.focus_drawn = false;
    context.on_focus = Some({
        let (reported, ctx) = (Rc::downgrade(&reported), ctx.handle());
        Rc::new(move |focus| {
            if let Some(reported) = reported.upgrade() {
                *reported.focus.borrow_mut() = focus;
            }
            publish_focus(&ctx);
        })
    });
    context.on_transport = Some({
        let (reported, ctx) = (Rc::downgrade(&reported), ctx.handle());
        Rc::new(move |transport: Option<Transport>| {
            let Some(reported) = reported.upgrade() else { return };
            // What a reader shows: playing or not, the tempo and the meter; the beat moves on all the time.
            let shown = |transport: &Option<Transport>| {
                transport.as_ref().map(|transport| (transport.playing, transport.tempo, transport.beats_per_bar, transport.meter.clone()))
            };
            let changed = shown(&reported.transport.borrow()) != shown(&transport);
            *reported.transport.borrow_mut() = transport;
            if changed {
                publish_focus(&ctx);
            }
        })
    });
    context.on_catch_up = Some({
        let (reported, ctx) = (Rc::downgrade(&reported), ctx.handle());
        Rc::new(move |catch_up| {
            let Some(reported) = reported.upgrade() else { return };
            {
                let mut kept = reported.catch_up.borrow_mut();
                kept.push(catch_up);
                if kept.len() > CATCH_UPS {
                    let older = kept.len() - CATCH_UPS;
                    kept.drain(..older);
                }
            }
            // focus.json carries it too, so the note the chat gets before its next turn tells it.
            publish_focus(&ctx);
        })
    });
    // Each change Kumi records (made, undone, kept, unsure) becomes or updates a HISTORY row, on the project open now.
    context.on_change = Some({
        let ctx = ctx.handle();
        Rc::new(move |record| {
            let Some(ctx) = ctx.upgrade() else { return };
            let key = ctx.link().project().map(|project| project.key).unwrap_or_default();
            ctx.history().record(&record, &key, ctx.snapshots());
        })
    });
    let session = DawSession {
        env: ctx.env().clone(),
        bridge_config,
        project_store: kumi_agent::core::project::create_project_store(ctx.paths().state_dir.join("projects")),
        answers: ctx.answers().clone(),
    };
    let integration = provider.integration(context, &session);
    if let Err(error) = integration.start(signal).await {
        let _ = tokio::time::timeout(CLOSING, integration.close()).await;
        return Err(OpError::Failed { reason: format!("Kumi couldn't reach {}: {error}", provider.profile().words.app) });
    }
    let connection = Rc::new(Connection { integration, reported });
    *ctx.link().connection.borrow_mut() = Some(connection.clone());
    Ok(connection)
}

/// The project's things the model can name in a change, with their refs (tracks, devices, clips): epoch-scoped.
pub async fn refs(connection: &Connection, signal: kumi_common::abort::Signal) -> Vec<Value> {
    let candidates = connection.integration.reference_candidates(signal).await.unwrap_or_default();
    candidates
        .into_iter()
        .filter(|candidate| matches!(candidate.reference, Reference::Daw { .. }))
        .take(MOST_REFS)
        .filter_map(|candidate| serde_json::to_value(candidate).ok())
        .collect()
}

/// `focus.json` (C3, as CC-7 words it): whether the DAW answers (`connected`), and while it does, the project as of the
/// last look (`seen_at`: the focus may have moved to another since), the focus and the transport; and what changed
/// while no one looked (as it stands, not taken). A DAW that doesn't answer, or no connection yet, has no project,
/// focus or transport here.
pub fn publish_focus(ctx: &Weak<Ctx>) {
    let Some(ctx) = ctx.upgrade() else { return };
    let connection = ctx.link().connection();
    let connected = connection.as_ref().is_some_and(|connection| connection.answering());
    let project = ctx
        .link()
        .project()
        .filter(|_| connected)
        .map(|project| json!({"name": project.name, "key": project.key, "epoch": project.epoch, "seen_at": project.seen_at}));
    let answering = connection.as_ref().filter(|_| connected);
    let body = json!({
        "connected": connected,
        "project": project,
        "focus": answering.and_then(|connection| connection.focus()).map(|focus| focus_value(&focus)),
        "transport": answering.and_then(|connection| connection.transport()).as_ref().map(transport_value),
        "catch_up": connection.as_ref().map(|connection| bounded_catch_up(connection.reported.catch_up.borrow().clone())).unwrap_or_default(),
    });
    if let Err(error) = ctx.snapshots().write("focus", &body) {
        say(&format!("kumi serve: focus.json not written: {error}"));
    }
}

/// The focus as `focus.json` and `observe` give it (CC-7): Kumi's `DawFocus`, its own fields only, with any name that
/// is a whole path (a take or sample named by its file) cut to the file's name, so no folder of the producer's leaves
/// the runtime. Refs (`…Ref`) are as [`Connection::focus`] gives them: the form a plan's steps take.
pub fn focus_value(focus: &DawFocus) -> Value {
    let mut value = serde_json::to_value(focus).unwrap_or(Value::Null);
    names_only(&mut value);
    value
}

/// A whole path: an absolute one, a drive's or a share's.
/// A path with a folder in it: a drive's (`C:\\…`), a share's (`\\\\server\\…`), or a rooted one with at least two parts
/// (`/Users/…/x.wav`). A name that only starts with a slash ("/Bus") or a lone one ("Drums / Perc") isn't.
fn whole_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive = bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    let share = text.strip_prefix("\\\\").is_some_and(|rest| rest.trim_end_matches(['\\', '/']).contains(['\\', '/']));
    let rooted = text.strip_prefix('/').is_some_and(|rest| rest.trim_end_matches('/').contains('/'));
    drive || share || rooted
}

/// Every string in `value` that's a whole path cut to its file name, the DAW's refs aside: what the runtime shows of
/// the producer's files is their names, never their folders.
pub(super) fn names_only(value: &mut Value) {
    match value {
        Value::String(text) if whole_path(text) => {
            let name = text.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default().to_string();
            *text = name;
        }
        Value::Array(items) => items.iter_mut().for_each(names_only),
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                if !(key == "ref" || key.ends_with("Ref") || key.ends_with("Refs")) {
                    names_only(field);
                }
            }
        }
        _ => {}
    }
}

/// What the DAW said of a plan's steps (`changed`, `error`, `reason`, as `change` answers them): [`names_only`], and
/// each path inside those sentences ("Couldn't load /Users/…/x.wav") cut to its file name too.
pub(super) fn names_only_said(value: &mut Value) {
    names_only(value);
    fn said(value: &mut Value) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(said),
            Value::Object(fields) => {
                for (key, field) in fields.iter_mut() {
                    match field {
                        Value::String(text) if matches!(key.as_str(), "changed" | "error" | "reason") => *text = names_only_text(text),
                        _ => said(field),
                    }
                }
            }
            _ => {}
        }
    }
    said(value);
}

/// The same for text that isn't JSON: each whole path in it (as far as a space or a quote) cut to its file name.
pub(super) fn names_only_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(|c: char| c.is_whitespace() || c == '"' || c == '\'').unwrap_or(rest.len());
        let (word, after) = rest.split_at(end);
        if whole_path(word) {
            out.push_str(word.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default());
        } else {
            out.push_str(word);
        }
        let mut chars = after.chars();
        if let Some(stop) = chars.next() {
            out.push(stop);
        }
        rest = chars.as_str();
    }
    out
}

/// The transport as C3 names it (`playing`, `tempo`, `position` in beats), with the bar's beats, the meter and a
/// tracker's line when the DAW says them. Not the integration's own clock (`at`), which means nothing outside it.
pub fn transport_value(transport: &Transport) -> Value {
    json!({
        "playing": transport.playing,
        "tempo": transport.tempo,
        "position": transport.beat,
        "beatsPerBar": transport.beats_per_bar,
        "meter": transport.meter,
        "line": transport.line,
    })
}

/// Whether `path` is inside `folder` on disk (links resolved); false when either isn't there.
pub(super) fn inside(path: &Path, folder: &Path) -> bool {
    matches!((std::fs::canonicalize(path), std::fs::canonicalize(folder)), (Ok(path), Ok(folder)) if path.starts_with(&folder))
}

/// A connection to `integration` that answers, with `observation` seen on it: as a `connect` and an `observe` leave the
/// link, for an operation's tests.
#[cfg(test)]
impl Link {
    pub(super) fn connected_for_test(&self, integration: Rc<dyn Integration>, observation: &Observation) -> Project {
        let reported = Rc::new(Reported::default());
        reported.connected.set(true);
        reported.answered.set(true);
        *self.connection.borrow_mut() = Some(Rc::new(Connection { integration, reported }));
        self.saw(observation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kumi_types::errors::RuntimeError;

    #[test]
    fn a_project_key_is_the_same_for_the_same_open_project_and_says_nothing_of_it() {
        let saved = "reaper:0123456789abcdef";
        let key = project_key(saved, Some("4242:1"));
        assert_eq!(key, project_key(saved, Some("4242:1")));
        assert_ne!(key, project_key(saved, Some("4242:2")), "a copy in another tab, or the file opened again, is another");
        assert_ne!(key, project_key("reaper:fedcba9876543210", Some("4242:1")));
        assert!(key.starts_with("p-") && key.len() == 18 && !key.contains("reaper"));
    }

    struct Nothing;

    #[async_trait(?Send)]
    impl Integration for Nothing {
        async fn start(&self, _: kumi_common::abort::Signal) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn observe(
            &self,
            _: kumi_common::abort::Signal,
            _: Option<kumi_agent::core::contracts::ObserveHints>,
        ) -> Result<Observation, RuntimeError> {
            unreachable!("these tests give the link their own observations")
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// What a DAW says when its saved project `id` is open in tab `open`.
    fn seen(id: &str, open: &str) -> Observation {
        Observation {
            key: format!("reaper:{id}"),
            revision: None,
            label: "Night Drive".into(),
            context: String::new(),
            instructions: String::new(),
            tools: vec![],
            project: None,
            set: Some(open.into()),
            tracks: None,
            saved_at: None,
            identity: None,
            native_epoch: None,
        }
    }

    fn connected(link: &Link) -> Rc<Reported> {
        let reported = Rc::new(Reported::default());
        reported.connected.set(true);
        reported.answered.set(true);
        *link.connection.borrow_mut() = Some(Rc::new(Connection { integration: Rc::new(Nothing), reported: reported.clone() }));
        reported
    }

    #[test]
    fn focus_names_no_folder_of_the_producers_and_keeps_the_daws_refs() {
        let focus: DawFocus = serde_json::from_value(json!({
            "track": {"name": "Drums"},
            "trackRef": "track:3",
            "clip": "/Users/producer/Samples/Breaks/Amen 170.wav",
            "item": {"kind": "take", "itemRef": "item:7/take:1", "name": "C:\\Users\\producer\\Music\\Vox take 2.wav"},
        }))
        .unwrap();
        let value = focus_value(&focus);
        assert_eq!(value["clip"], "Amen 170.wav");
        assert_eq!(value["item"]["name"], "Vox take 2.wav");
        assert_eq!(
            (value["trackRef"].clone(), value["item"]["itemRef"].clone()),
            (json!("track:3"), json!("item:7/take:1")),
            "refs are as the integration gives them (this one, as the DAW's)"
        );
        let text = value.to_string();
        assert!(!text.contains("/Users/") && !text.contains("\\Users\\"), "{text}");
        let known: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(known, ["track", "trackRef", "clip", "item"], "nothing but DawFocus's own fields");
    }

    #[test]
    fn a_path_in_what_the_daw_says_of_a_step_is_its_file_name_and_a_name_with_a_slash_stays() {
        let mut said = json!({"refused": [{"step": 2, "error": "Couldn't load /Users/producer/Samples/kick.wav: too long"}],
        "done": [
            {"changed": "Simpler now plays C:\\Users\\producer\\snare.wav", "trackRef": "7:track:0"},
            {"changed": "Drums / Perc volume -6.0 dB → -2.0 dB", "track": "/Bus"},
        ]});
        names_only_said(&mut said);
        assert_eq!(said["refused"][0]["error"], "Couldn't load kick.wav: too long");
        assert_eq!(said["done"][0]["changed"], "Simpler now plays snare.wav");
        assert_eq!(said["done"][0]["trackRef"], "7:track:0", "refs are the DAW's");
        assert_eq!(
            (said["done"][1]["changed"].clone(), said["done"][1]["track"].clone()),
            (json!("Drums / Perc volume -6.0 dB → -2.0 dB"), json!("/Bus"))
        );
        // Names elsewhere (focus, observe, read) aren't rewritten as sentences, nor taken for paths.
        let mut focus = json!({"track": {"name": "Drums / Perc"}, "group": "/Bus", "clip": "/Users/producer/Samples/Amen 170.wav"});
        names_only(&mut focus);
        assert_eq!(focus, json!({"track": {"name": "Drums / Perc"}, "group": "/Bus", "clip": "Amen 170.wav"}));
    }

    #[test]
    fn the_transport_is_c3s_without_the_integrations_clock() {
        let transport = Transport {
            playing: true,
            tempo: Some(124.0),
            beat: Some(33.5),
            at: 9_876.0,
            beats_per_bar: Some(4.0),
            meter: None,
            line: None,
        };
        let value = transport_value(&transport);
        let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["playing", "tempo", "position", "beatsPerBar", "meter", "line"]);
        assert_eq!((value["position"].clone(), value["playing"].clone()), (json!(33.5), json!(true)));
    }

    #[test]
    fn catch_up_is_the_newest_five_projects_with_twenty_lines_each_at_most() {
        let seen = |n: usize, lines: usize| CatchUp {
            set: format!("Set {n}"),
            last_seen_at: n as i64,
            lines: (0..lines).map(|line| format!("line {line}")).collect(),
            more: 3,
            after_reconnect: None,
        };
        let bounded = bounded_catch_up((0..7).map(|n| seen(n, if n == 6 { 25 } else { 2 })).collect());
        assert_eq!(bounded.iter().map(|seen| seen.set.as_str()).collect::<Vec<_>>(), ["Set 2", "Set 3", "Set 4", "Set 5", "Set 6"]);
        let last = bounded.last().unwrap();
        assert_eq!((last.lines.len(), last.more), (20, 3 + 5), "the lines past 20 are counted with the rest");
    }

    #[test]
    fn a_plan_holds_only_for_the_project_open_in_front_at_the_epoch_it_was_made() {
        let link = Link::new(1_700_000_000_000);
        let reported = connected(&link);
        let first = link.saw(&seen("0123", "4242:1"));
        assert!(first.epoch > 1_700_000_000_000, "past every epoch an earlier runtime gave out");
        assert!(link.expect(&first.key, first.epoch).is_ok());
        assert_eq!(
            link.saw(&seen("0123", "4242:1")),
            Project { seen_at: link.project().unwrap().seen_at, ..first.clone() },
            "the same look"
        );
        // Its Save-As copy, in another tab: the same saved id, another project.
        let copy = link.saw(&seen("0123", "4242:2"));
        assert_ne!(copy.key, first.key);
        assert_eq!(copy.epoch, first.epoch + 1);
        let Err(OpError::Refused { code: "stale", .. }) = link.expect(&first.key, first.epoch) else {
            panic!("a plan made on the first tab")
        };
        // The DAW went away and came back: nothing holds until the next look.
        reported.returns.set(reported.returns.get() + 1);
        let Err(OpError::Refused { code: "stale", .. }) = link.expect(&copy.key, copy.epoch) else {
            panic!("a plan from before the return")
        };
        let again = link.saw(&seen("0123", "4242:2"));
        assert_eq!(again.epoch, copy.epoch + 1);
        assert!(link.expect(&again.key, again.epoch).is_ok());
        // While it doesn't answer, nothing is written.
        reported.connected.set(false);
        let Err(OpError::NotConnected { .. }) = link.expect(&again.key, again.epoch) else { panic!("a plan while the DAW is away") };
    }

    /// The looked page epoch is the look's own: an A→B switch after the look leaves A's key with A's epoch, so B's page
    /// is never the looked project's; a look that gave none pins nothing.
    #[test]
    fn a_look_keeps_its_own_epoch_so_another_projects_page_is_never_the_looked_ones() {
        let link = Link::new(1_700_000_000_000);
        connected(&link);
        let looked = |open: &str, epoch: Option<&str>| Observation { native_epoch: epoch.map(str::to_owned), ..seen("0123", open) };
        assert_eq!(link.observed_native_epoch(), None, "no look yet");
        let a = link.saw(&looked("4242:1", Some("eA")));
        assert_eq!(link.observed_native_epoch().as_deref(), Some("eA"));
        assert!(link.page_is_looked(Some("eA")));
        assert!(!link.page_is_looked(Some("eB")), "B's page isn't A's");
        assert!(!link.page_is_looked(None));
        // The next look is B's: its key and its epoch together.
        let b = link.saw(&looked("4242:2", Some("eB")));
        assert_ne!(a.key, b.key);
        assert_eq!(link.observed_native_epoch().as_deref(), Some("eB"));
        assert!(!link.page_is_looked(Some("eA")));
        // A look that gave no epoch (one that couldn't read the project): no page is taken on trust.
        link.saw(&looked("4242:2", None));
        assert_eq!(link.observed_native_epoch(), None);
        assert!(!link.page_is_looked(Some("eB")));
    }
}
