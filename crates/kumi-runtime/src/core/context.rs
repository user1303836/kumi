//! What a DAW's integration tells the app and asks of it, whichever DAW it is: the neutral half of a DAW's options
//! (Live's are `AbletonOptions`, built from this plus what only Live has: its bridge, User Library, Ears, hands).
use super::contracts::{ActionEvent, AuditionEvent, CatchUp, ChangeRecord, ConnectionListener, PinnedNode};
use super::project::ProjectStore;
use super::view::{DawFocus, Transport};
use chrono::{DateTime, Utc};
use futures::future::LocalBoxFuture;
use std::rc::Rc;

/// Asks the producer to free disk space before a recording: what's recorded, the bytes needed, where.
pub type LowDisk = Rc<dyn Fn(String, f64, String) -> LocalBoxFuture<'static, Option<String>>>;

/// Finds the listening model: one, none (no provider has one), or why it couldn't tell (the network, Esc).
pub type ListenerSource = Rc<
    dyn Fn(
        kumi_common::abort::Signal,
    ) -> futures::future::LocalBoxFuture<'static, Result<Option<Rc<dyn crate::listening::listener::Listener>>, String>>,
>;

/// The sinks a DAW's integration reports to, and the services and timings the app gives it.
#[derive(Clone)]
pub struct IntegrationContext {
    pub on_connection: ConnectionListener,
    pub now: Option<Rc<dyn Fn() -> DateTime<Utc>>>,
    pub generation: Option<String>,
    pub on_dispatch: Option<Rc<dyn Fn(String)>>,
    pub on_focus: Option<Rc<dyn Fn(Option<DawFocus>)>>,
    pub on_pointed: Option<Rc<dyn Fn(PinnedNode)>>,
    pub on_transport: Option<Rc<dyn Fn(Option<Transport>)>>,
    pub focus_interval_ms: Option<u64>,
    /// Whether something draws the focus as it moves (the full screen's FOCUS). Without it, a DAW whose focus is
    /// polled is asked about once a second: enough to see it go, come back or turn busy.
    pub focus_drawn: bool,
    pub on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
    pub on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    pub low_disk: Option<LowDisk>,
    pub on_watch: Option<Rc<dyn Fn(bool)>>,
    /// Asks the session to look at the DAW again: a look failed while the DAW was there (it was loading a file), and no
    /// turn may come to look again.
    pub on_look_again: Option<Rc<dyn Fn()>>,
    pub change_timeout_ms: Option<u64>,
    pub on_catch_up: Option<Rc<dyn Fn(CatchUp)>>,
    /// (M6) Told the projects the DAW opened lately, as its bridge reads them once connected (Logic's File > Open Recent,
    /// the ones whose place it knew), for the library to learn.
    pub on_recent_projects: Option<Rc<dyn Fn(Vec<String>)>>,
    /// Where saved projects' baselines are kept (the session's store, each project under its place), for a DAW Kumi
    /// catches up on; None keeps none.
    pub project_store: Option<Rc<dyn ProjectStore>>,
    pub reconnect_interval_ms: Option<u64>,
    pub restore_file: Option<String>,
    pub on_audition: Option<Rc<dyn Fn(AuditionEvent)>>,
    /// Each round the judge logs (`judge`): its target, change, numbers before and after, keep or revert, and what's next.
    pub on_judge: Option<Rc<dyn Fn(crate::listening::round::Round)>>,
    /// Finds an audio-capable model among the producer's providers to listen beside the meters (asked once).
    pub listener: Option<ListenerSource>,
    /// Measured references the judge can work toward by name.
    pub references: Option<Rc<crate::references::store::ReferenceStore>>,
}

impl IntegrationContext {
    pub fn new(on_connection: ConnectionListener) -> Self {
        Self {
            on_connection,
            now: None,
            generation: None,
            on_dispatch: None,
            on_focus: None,
            on_pointed: None,
            on_transport: None,
            focus_interval_ms: None,
            focus_drawn: true,
            on_change: None,
            on_action: None,
            low_disk: None,
            on_watch: None,
            on_look_again: None,
            change_timeout_ms: None,
            on_catch_up: None,
            on_recent_projects: None,
            project_store: None,
            reconnect_interval_ms: None,
            restore_file: None,
            on_audition: None,
            on_judge: None,
            listener: None,
            references: None,
        }
    }
}
