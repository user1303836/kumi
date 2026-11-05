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

    /// What changed while no one looked, since the last time this was asked.
    pub fn take_catch_up(&self) -> Vec<CatchUp> {
        std::mem::take(&mut *self.reported.catch_up.borrow_mut())
    }
}

/// The runtime's side of the connection: none until `connect`, then the connection and the project last seen.
#[derive(Default)]
pub struct Link {
    connection: RefCell<Option<Rc<Connection>>>,
    project: RefCell<Option<Project>>,
    /// The connection's returns when the project was last seen: a return since moves the epoch on.
    returns_seen: Cell<u64>,
    epoch: Cell<u64>,
}

impl Link {
    pub fn connection(&self) -> Option<Rc<Connection>> {
        self.connection.borrow().clone()
    }

    pub fn project(&self) -> Option<Project> {
        self.project.borrow().clone()
    }

    /// The project `observation` is about, as the runtime keys it: the epoch moves on when it's another project than
    /// last seen, or the DAW came back since.
    pub fn saw(&self, observation: &Observation) -> Project {
        let key = project_key(&observation.key);
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

    /// The write's plan was made on `key` at `epoch`: refused unless that's the project and epoch now.
    pub fn expect(&self, key: &str, epoch: u64) -> Result<Project, OpError> {
        let Some(project) = self.project() else {
            return Err(OpError::NotConnected { reason: "Kumi hasn't looked at the project yet: observe first.".into() });
        };
        if project.key != key {
            let reason = "That plan was made for another project than the one open now: observe again.";
            return Err(OpError::Refused { code: "stale", reason: reason.into(), owner: None });
        }
        if project.epoch != epoch {
            let reason = "The project has changed since that plan was made (its refs no longer hold): observe again.";
            return Err(OpError::Refused { code: "stale", reason: reason.into(), owner: None });
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

/// `observation.key` as the runtime gives it out: short, and saying nothing of the project's file or name.
fn project_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    format!("p-{}", digest[..8].iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

/// Connects to the DAW: its integration made with the plugin's host policy and started. Refused when Kumi's part
/// isn't in the DAW.
pub async fn connect(ctx: &Ctx, signal: kumi_common::abort::Signal) -> Result<Rc<Connection>, OpError> {
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
    let Some(bridge_config) = provider.bridge_config(ctx.env()) else {
        let reason = format!("Kumi's part isn't in {} yet: set it up first.", provider.profile().words.app);
        return Err(OpError::Refused { code: "not_set_up", reason, owner: None });
    };
    let reported = Rc::new(Reported::default());
    let on_connection = {
        let reported = Rc::downgrade(&reported);
        Rc::new(move |state: ConnectionState, _cause| {
            let Some(reported) = reported.upgrade() else { return };
            let answering = state == ConnectionState::Connected;
            if answering && !reported.connected.get() {
                if reported.answered.get() {
                    reported.returns.set(reported.returns.get() + 1);
                }
                reported.answered.set(true);
            }
            reported.connected.set(answering);
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
        let reported = Rc::downgrade(&reported);
        Rc::new(move |transport| {
            if let Some(reported) = reported.upgrade() {
                *reported.transport.borrow_mut() = transport;
            }
        })
    });
    context.on_catch_up = Some({
        let reported = Rc::downgrade(&reported);
        Rc::new(move |catch_up| {
            if let Some(reported) = reported.upgrade() {
                reported.catch_up.borrow_mut().push(catch_up);
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

/// `focus.json` (C3): the project as last seen, the focus, the transport and what changed while no one looked (as it
/// stands, not taken).
pub fn publish_focus(ctx: &Weak<Ctx>) {
    let Some(ctx) = ctx.upgrade() else { return };
    let Some(connection) = ctx.link().connection() else { return };
    let project = ctx.link().project().map(|project| json!({"name": project.name, "key": project.key, "epoch": project.epoch}));
    let body = json!({
        "project": project,
        "focus": connection.focus(),
        "transport": connection.transport(),
        "catch_up": *connection.reported.catch_up.borrow(),
    });
    if let Err(error) = ctx.snapshots().write("focus", &body) {
        say(&format!("kumi serve: focus.json not written: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_key_is_the_same_for_the_same_project_and_says_nothing_of_it() {
        let key = project_key("[\"g1\",1,\"/Users/x/Music/Night Drive.als\"]");
        assert_eq!(key, project_key("[\"g1\",1,\"/Users/x/Music/Night Drive.als\"]"));
        assert_ne!(key, project_key("[\"g2\",1,\"/Users/x/Music/Night Drive.als\"]"));
        assert!(key.starts_with("p-") && key.len() == 18 && !key.contains("Night"));
    }
}
