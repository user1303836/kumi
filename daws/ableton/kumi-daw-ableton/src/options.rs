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
pub use crate::core::context::{IntegrationContext, ListenerSource, LowDisk};

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
    /// Each round the judge logs (`judge`): its target, change, numbers before and after, keep or revert, and what's next.
    pub on_judge: Option<Rc<dyn Fn(crate::listening::round::Round)>>,
    /// Finds an audio-capable model among the producer's providers to listen beside the meters (asked once).
    pub listener: Option<ListenerSource>,
    /// Measured references the judge can work toward by name.
    pub references: Option<Rc<crate::references::store::ReferenceStore>>,
    pub ears: Option<EarsSetup>,
    pub hands: Option<HandsSetup>,
    pub fast: Option<bool>,
    /// Whether Live's process is running, to tell Live closing from Live opening another Set.
    pub live_running: Option<super::connection::LiveRunning>,
    /// Opens a Set's file in Live (the one running, or Live itself), as the system opens a file: true when it was handed
    /// over. A kept conversation's Set opens this way when the producer asks.
    pub open_file: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, bool>>>,
    /// How Willington's bindings stand, None while Kumi's bridge in Live doesn't carry them: asked each time the model's
    /// session is made, since /willington switches them while Kumi runs.
    pub willington: Option<Rc<dyn Fn() -> Option<super::willington::WillingtonSwitch>>>,
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
            on_judge: None,
            listener: None,
            references: None,
            ears: None,
            hands: None,
            fast: None,
            live_running: None,
            open_file: None,
            willington: None,
        }
    }
    /// Live's options from the app's neutral half; what only Live has is set after.
    pub fn with_context(context: IntegrationContext) -> Self {
        let mut options = Self::new(context.on_connection);
        options.now = context.now;
        options.generation = context.generation;
        options.on_dispatch = context.on_dispatch;
        options.on_focus = context.on_focus;
        options.on_pointed = context.on_pointed;
        options.on_transport = context.on_transport;
        options.focus_interval_ms = context.focus_interval_ms;
        options.on_change = context.on_change;
        options.on_action = context.on_action;
        options.low_disk = context.low_disk;
        options.on_watch = context.on_watch;
        options.change_timeout_ms = context.change_timeout_ms;
        options.on_catch_up = context.on_catch_up;
        options.reconnect_interval_ms = context.reconnect_interval_ms;
        options.restore_file = context.restore_file;
        options.on_audition = context.on_audition;
        options.on_judge = context.on_judge;
        options.listener = context.listener;
        options.references = context.references;
        options
    }
    /// The neutral half of these options.
    pub fn context(&self) -> IntegrationContext {
        IntegrationContext {
            on_connection: self.on_connection.clone(),
            now: self.now.clone(),
            generation: self.generation.clone(),
            on_dispatch: self.on_dispatch.clone(),
            on_focus: self.on_focus.clone(),
            on_pointed: self.on_pointed.clone(),
            on_transport: self.on_transport.clone(),
            focus_interval_ms: self.focus_interval_ms,
            focus_drawn: true,
            on_change: self.on_change.clone(),
            on_action: self.on_action.clone(),
            low_disk: self.low_disk.clone(),
            on_watch: self.on_watch.clone(),
            on_look_again: None,
            change_timeout_ms: self.change_timeout_ms,
            on_catch_up: self.on_catch_up.clone(),
            reconnect_interval_ms: self.reconnect_interval_ms,
            restore_file: self.restore_file.clone(),
            on_audition: self.on_audition.clone(),
            on_judge: self.on_judge.clone(),
            listener: self.listener.clone(),
            references: self.references.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every neutral field goes into Live's options and comes back out the same (the same callbacks, not copies).
    #[test]
    fn the_neutral_half_goes_in_and_comes_back_whole() {
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        context.now = Some(Rc::new(Utc::now));
        context.generation = Some("g".into());
        context.on_dispatch = Some(Rc::new(|_| {}));
        context.on_focus = Some(Rc::new(|_| {}));
        context.on_pointed = Some(Rc::new(|_| {}));
        context.on_transport = Some(Rc::new(|_| {}));
        context.focus_interval_ms = Some(7);
        context.on_change = Some(Rc::new(|_| {}));
        context.on_action = Some(Rc::new(|_| {}));
        context.low_disk = Some(Rc::new(|_, _, _| Box::pin(async { None })));
        context.on_watch = Some(Rc::new(|_| {}));
        context.change_timeout_ms = Some(8);
        context.on_catch_up = Some(Rc::new(|_| {}));
        context.reconnect_interval_ms = Some(9);
        context.restore_file = Some("/r".into());
        context.on_audition = Some(Rc::new(|_| {}));
        context.on_judge = Some(Rc::new(|_| {}));
        context.listener = Some(Rc::new(|_| Box::pin(async { Ok(None) })));
        let options = AbletonOptions::with_context(context.clone());
        let back = options.context();
        fn same<T: ?Sized>(a: &Option<Rc<T>>, b: &Option<Rc<T>>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        }
        assert!(Rc::ptr_eq(&context.on_connection, &back.on_connection));
        assert!(same(&context.now, &back.now) && same(&context.on_dispatch, &back.on_dispatch) && same(&context.on_focus, &back.on_focus));
        assert!(same(&context.on_pointed, &back.on_pointed) && same(&context.on_transport, &back.on_transport));
        assert!(
            same(&context.on_change, &back.on_change)
                && same(&context.on_action, &back.on_action)
                && same(&context.low_disk, &back.low_disk)
        );
        assert!(same(&context.on_watch, &back.on_watch) && same(&context.on_catch_up, &back.on_catch_up));
        assert!(same(&context.on_audition, &back.on_audition) && same(&context.on_judge, &back.on_judge));
        assert!(same(&context.listener, &back.listener) && same(&context.references, &back.references));
        assert_eq!(
            (back.generation, back.focus_interval_ms, back.change_timeout_ms, back.reconnect_interval_ms, back.restore_file),
            (Some("g".into()), Some(7), Some(8), Some(9), Some("/r".into()))
        );
        // What only Live has stays unset.
        assert!(options.bridge_config.is_none() && options.connect.is_none() && options.project_store.is_none());
    }
}
