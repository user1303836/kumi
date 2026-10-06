//! Public construction options for the native Ableton integration.
use super::{
    connection::{Connect, ConnectionOptions},
    project::ProjectStore,
};
use crate::{
    core::{contracts::*, errors::RuntimeError},
    ears::link::EarsLink,
    hands::Hands,
};
use chrono::{DateTime, Utc};
use futures::future::LocalBoxFuture;
use std::rc::Rc;

pub enum EarsSetup {
    Disabled,
    Open(Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Rc<dyn EarsLink>, RuntimeError>>>),
}
pub enum HandsSetup {
    Disabled,
    Open(Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Option<Rc<dyn Hands>>, RuntimeError>>>),
}
pub type LowDisk = Rc<dyn Fn(String, f64, String) -> LocalBoxFuture<'static, Option<String>>>;

pub struct AbletonOptions {
    pub on_connection: ConnectionListener,
    pub bridge_config: Option<String>,
    pub connect: Option<Connect>,
    pub now: Option<Rc<dyn Fn() -> DateTime<Utc>>>,
    pub generation: Option<String>,
    pub on_dispatch: Option<Rc<dyn Fn(String)>>,
    pub on_focus: Option<Rc<dyn Fn(Option<LiveFocus>)>>,
    pub on_pointed: Option<Rc<dyn Fn(PinnedNode)>>,
    pub on_transport: Option<Rc<dyn Fn(Option<LiveTransport>)>>,
    pub focus_interval_ms: Option<u64>,
    pub on_change: Option<Rc<dyn Fn(ChangeRecord)>>,
    pub on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    pub low_disk: Option<LowDisk>,
    pub on_watch: Option<Rc<dyn Fn(bool)>>,
    pub change_timeout_ms: Option<u64>,
    pub project_store: Option<Rc<dyn ProjectStore>>,
    pub on_catch_up: Option<Rc<dyn Fn(CatchUp)>>,
    pub reconnect_interval_ms: Option<u64>,
    pub user_library: Option<String>,
    pub restore_file: Option<String>,
    pub on_audition: Option<Rc<dyn Fn(AuditionEvent)>>,
    pub ears: Option<EarsSetup>,
    pub hands: Option<HandsSetup>,
    pub fast: Option<bool>,
    /// Whether Live's process is running, to tell Live closing from Live opening another Set.
    pub live_running: Option<super::connection::LiveRunning>,
}
impl AbletonOptions {
    pub fn new(on_connection: ConnectionListener) -> Self {
        Self {
            on_connection,
            bridge_config: None,
            connect: None,
            now: None,
            generation: None,
            on_dispatch: None,
            on_focus: None,
            on_pointed: None,
            on_transport: None,
            focus_interval_ms: None,
            on_change: None,
            on_action: None,
            low_disk: None,
            on_watch: None,
            change_timeout_ms: None,
            project_store: None,
            on_catch_up: None,
            reconnect_interval_ms: None,
            user_library: None,
            restore_file: None,
            on_audition: None,
            ears: None,
            hands: None,
            fast: None,
            live_running: None,
        }
    }
    pub fn connection_options(&self) -> ConnectionOptions {
        let mut options = ConnectionOptions::new(self.on_connection.clone());
        options.bridge_config = self.bridge_config.clone();
        options.connect = self.connect.clone();
        options.now = self.now.clone();
        options.generation = self.generation.clone();
        options.on_dispatch = self.on_dispatch.clone();
        options.on_focus = self.on_focus.clone();
        options.on_pointed = self.on_pointed.clone();
        options.on_transport = self.on_transport.clone();
        options.focus_interval_ms = self.focus_interval_ms;
        options.reconnect_interval_ms = self.reconnect_interval_ms;
        options.live_running = self.live_running.clone();
        options
    }
}
