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
use kumi_daw::provider::DawSession;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::Duration;

/// How many of the project's things (tracks, devices, clips) `observe` lists with their refs.
const MOST_REFS: usize = 200;
/// How long the integration gets to close when the runtime stops.
const CLOSING: Duration = Duration::from_secs(5);

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

    pub fn focus(&self) -> Option<DawFocus> {
        self.reported.focus.borrow().clone()
    }

    pub fn transport(&self) -> Option<Transport> {
        self.reported.transport.borrow().clone()
    }

    /// What changed while no one looked, since the last time this was asked: bounded (see [`bounded_catch_up`]).
    pub fn take_catch_up(&self) -> Vec<CatchUp> {
        bounded_catch_up(std::mem::take(&mut *self.reported.catch_up.borrow_mut()))
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
}

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
        }
    }

    pub fn connection(&self) -> Option<Rc<Connection>> {
        self.connection.borrow().clone()
    }

    pub fn project(&self) -> Option<Project> {
        self.project.borrow().clone()
    }

    /// The project `observation` is about, as the runtime keys it (the integration's key with the DAW's identity for
    /// the open project, so a copy open in another tab, or the same file opened again, is another): the epoch moves
    /// on when it's another project than last seen, or the DAW came back since.
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
        let project = Project { key, name, epoch: self.epoch.get(), seen_at: kumi_common::time::now_ms() };
        *self.project.borrow_mut() = Some(project.clone());
        project
    }

    /// Moves the epoch on (a write's outcome is unsure): refs from earlier looks no longer hold.
    pub fn advance(&self) {
        self.epoch.set(self.epoch.get() + 1);
        if let Some(project) = self.project.borrow_mut().as_mut() {
            project.epoch = self.epoch.get();
        }
    }

    /// The write's plan was made on `key` at `epoch`: refused unless that's the project and epoch now, the DAW answers,
    /// and it hasn't gone and come back since the last look.
    pub fn expect(&self, key: &str, epoch: u64) -> Result<Project, OpError> {
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
            return stale("That plan was made for another project than the one open now: observe again.");
        }
        if project.epoch != epoch {
            return stale("The project has changed since that plan was made (its refs no longer hold): observe again.");
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
            reason: format!("Kumi doesn't work with {} here.", ctx.daw().as_str()),
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
        let reason = format!(
            "Kumi's part in {app} belongs to another Kumi on this computer: set {app} up for Hermes, or adopt that installation, first."
        );
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
    context.policy = HostPolicy::plugin(ctx.paths().state_dir.join("scratch").join("listen"));
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
        let reported = Rc::downgrade(&reported);
        Rc::new(move |catch_up| {
            if let Some(reported) = reported.upgrade() {
                let mut kept = reported.catch_up.borrow_mut();
                kept.push(catch_up);
                if kept.len() > CATCH_UPS {
                    let older = kept.len() - CATCH_UPS;
                    kept.drain(..older);
                }
            }
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
/// while no one looked (as it stands, not taken). A DAW that doesn't answer has no project, focus or transport here.
pub fn publish_focus(ctx: &Weak<Ctx>) {
    let Some(ctx) = ctx.upgrade() else { return };
    let Some(connection) = ctx.link().connection() else { return };
    let connected = connection.answering();
    let project = ctx
        .link()
        .project()
        .filter(|_| connected)
        .map(|project| json!({"name": project.name, "key": project.key, "epoch": project.epoch, "seen_at": project.seen_at}));
    let body = json!({
        "connected": connected,
        "project": project,
        "focus": connection.focus().filter(|_| connected),
        "transport": connection.transport().filter(|_| connected).as_ref().map(transport_value),
        "catch_up": bounded_catch_up(connection.reported.catch_up.borrow().clone()),
    });
    if let Err(error) = ctx.snapshots().write("focus", &body) {
        say(&format!("kumi serve: focus.json not written: {error}"));
    }
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

/// Whether `a` and `b` are the same folder on disk (links resolved); false when either isn't there.
fn same_folder(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// Whether `path` is inside `folder` on disk (links resolved); false when either isn't there.
fn inside(path: &Path, folder: &Path) -> bool {
    matches!((std::fs::canonicalize(path), std::fs::canonicalize(folder)), (Ok(path), Ok(folder)) if path.starts_with(&folder))
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
}
