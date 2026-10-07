//! Session behavior.
use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::{
        contracts::*,
        errors::{FailureKind, KumiError, RuntimeError},
        session::*,
    },
    integrations::ableton::project::create_conversation_store,
    kernel::budget::{transcript_of, OBSERVATION_MARKER},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
use tokio::sync::{oneshot, Notify};

type Run = Rc<dyn Fn(String, Signal, KernelEmit) -> LocalBoxFuture<'static, Result<TurnResult, RuntimeError>>>;
fn complete() -> TurnResult {
    TurnResult { stop_reason: StopReason::Completed, usage: None }
}
fn cancelled() -> TurnResult {
    TurnResult { stop_reason: StopReason::Cancelled, usage: Some(Usage { input_tokens: 4., output_tokens: 3., ..Default::default() }) }
}
async fn delay(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
async fn settle() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}
#[derive(Default)]
struct Record {
    calls: RefCell<Vec<String>>,
    created: RefCell<Vec<KernelOptions>>,
    closes: Cell<usize>,
    integration_closes: Cell<usize>,
    observations: Cell<usize>,
    stops: Cell<usize>,
    stop_works: Cell<bool>,
    listeners: RefCell<Vec<ConnectionListener>>,
    refresh_error: Cell<bool>,
    audition: RefCell<Option<Rc<dyn Fn(&AuditionRequest)>>>,
    goal_rig: RefCell<Option<Rc<dyn GoalRig>>>,
    goal_requests: RefCell<Vec<AuditionRequest>>,
    /// Setting up a goal's search waits until the goal is stopped.
    goal_holds: Cell<bool>,
    /// Reading the Set waits until the operation is stopped.
    observe_holds: Cell<bool>,
    audio_resolved: RefCell<Vec<String>>,
}
struct TestKernel {
    record: Rc<Record>,
    history: RefCell<Vec<Value>>,
    run: Option<Run>,
}
#[async_trait(?Send)]
impl Kernel for TestKernel {
    async fn run(&self, input: &str, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError> {
        self.record.calls.borrow_mut().push(input.into());
        self.history.borrow_mut().push(json!({"role":"user","content":input.split(OBSERVATION_MARKER).next().unwrap()}));
        if let Some(run) = &self.run {
            run(input.into(), signal, emit).await
        } else {
            emit(KernelEvent::Text { text: "answer".into() })?;
            Ok(complete())
        }
    }
    async fn run_with(&self, input: &str, pictures: Vec<Picture>, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError> {
        let names: Vec<_> = pictures.iter().map(|p| format!("{} {} {}", p.name, p.media_type, p.data.len())).collect();
        self.record.calls.borrow_mut().push(format!("pictures: {}", names.join(", ")));
        self.run(input, signal, emit).await
    }
    async fn close(&self) {
        self.record.closes.set(self.record.closes.get() + 1);
    }
    fn has_checkpoint(&self) -> bool {
        true
    }
    fn checkpoint(&self) -> Result<KernelCheckpoint, RuntimeError> {
        Ok(KernelCheckpoint { version: 1, messages: self.history.borrow().clone(), origin: None, tools: None })
    }
    fn has_transcript(&self) -> bool {
        true
    }
    fn transcript(&self) -> Vec<TranscriptLine> {
        transcript_of(&self.history.borrow())
    }
    fn has_aside(&self) -> bool {
        true
    }
    async fn aside(&self, _: &str, _: Signal, _: OnText) -> Result<String, RuntimeError> {
        use kumi_runtime::core::timing;
        drop(timing::model_call("openai/a-side-question"));
        timing::sent(500_000);
        Ok("aside".into())
    }
}
struct TestIntegration {
    record: Rc<Record>,
    observation: Rc<RefCell<Observation>>,
    listener: ConnectionListener,
}
#[async_trait(?Send)]
impl Integration for TestIntegration {
    fn has_audio_file(&self) -> bool {
        true
    }
    async fn audio_file(&self, named: &str, _: Signal) -> Result<Option<String>, RuntimeError> {
        self.record.audio_resolved.borrow_mut().push(named.into());
        Ok(None)
    }
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        (self.listener)(ConnectionState::Connecting, None);
        (self.listener)(ConnectionState::Connected, None);
        Ok(())
    }
    async fn observe(&self, signal: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        self.record.observations.set(self.record.observations.get() + 1);
        if self.record.observe_holds.get() {
            signal.cancelled().await;
            return Err(RuntimeError::Aborted);
        }
        if self.record.refresh_error.get() {
            return Err(RuntimeError::plain("secret-token-must-not-escape"));
        }
        Ok(self.observation.borrow().clone())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.record.integration_closes.set(self.record.integration_closes.get() + 1);
        Ok(())
    }
    fn has_audition(&self) -> bool {
        self.record.audition.borrow().is_some()
    }
    async fn audition(&self, request: &AuditionRequest, _: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        if let Some(callback) = self.record.audition.borrow().clone() {
            callback(request);
        }
        Ok(Ok(AuditionResult { takes: vec![], seconds: 1., notes: vec![], best: None, reference: None }))
    }
    fn has_goal(&self) -> bool {
        self.record.goal_rig.borrow().is_some()
    }
    async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        self.record.goal_requests.borrow_mut().push(request.clone());
        if self.record.goal_holds.get() {
            signal.cancelled().await;
            return Err(RuntimeError::Aborted);
        }
        Ok(self.record.goal_rig.borrow().clone().ok_or_else(|| "no rig".into()))
    }
    fn has_stop_live(&self) -> bool {
        true
    }
    async fn stop_live(&self, _: Signal) -> Result<bool, RuntimeError> {
        self.record.stops.set(self.record.stops.get() + 1);
        Ok(self.record.stop_works.get())
    }
}
struct Harness {
    session: Session,
    record: Rc<Record>,
    events: Rc<RefCell<Vec<SessionEvent>>>,
    observation: Rc<RefCell<Observation>>,
}
fn harness(run: Option<Run>, config: impl FnOnce(&mut SessionOptions)) -> Harness {
    let record = Rc::new(Record::default());
    record.stop_works.set(true);
    let observation = Rc::new(RefCell::new(Observation {
        key: "epoch:1".into(),
        revision: None,
        label: "Fixture Set".into(),
        context: "fresh fixture context".into(),
        instructions: "fixture instructions".into(),
        tools: vec![],
        project: None,
        tracks: None,
        saved_at: None,
    }));
    let events = Rc::new(RefCell::new(vec![]));
    let out = events.clone();
    let r = record.clone();
    let factory: KernelFactory = Rc::new(move |options| {
        r.created.borrow_mut().push(options.clone());
        let record = r.clone();
        let run = run.clone();
        async move {
            Ok(Rc::new(TestKernel { record, history: RefCell::new(options.checkpoint.map(|c| c.messages).unwrap_or_default()), run })
                as Rc<dyn Kernel>)
        }
        .boxed_local()
    });
    let r = record.clone();
    let obs = observation.clone();
    let integration: IntegrationFactory = Box::new(move |listener| {
        r.listeners.borrow_mut().push(listener.clone());
        Rc::new(TestIntegration { record: r.clone(), observation: obs.clone(), listener })
    });
    let mut options = SessionOptions::new(factory, integration, Rc::new(move |e| out.borrow_mut().push(e)));
    options.timeout_ms = Some(5000);
    options.cancel_grace_ms = Some(10);
    // Long enough for a slow disk to finish the saves close waits for; a test of a stuck close sets its own.
    options.close_timeout_ms = Some(2000);
    options.missing_after_ms = Some(30);
    config(&mut options);
    Harness { session: create_session(options).unwrap(), record, events, observation }
}
impl Harness {
    fn connection(&self, state: ConnectionState, cause: Option<DisconnectCause>) {
        let listener = self.record.listeners.borrow().last().unwrap().clone();
        listener(state, cause);
    }
    fn error(&self, text: &str) -> bool {
        self.events.borrow().iter().any(|e| matches!(e, SessionEvent::Error{message,..} if message.contains(text)))
    }
    fn notice(&self, text: &str) -> bool {
        self.events.borrow().iter().any(|e| matches!(e, SessionEvent::Notice{message} if message.contains(text)))
    }
    fn words(&self, index: usize) -> Option<Vec<String>> {
        self.record.created.borrow()[index]
            .checkpoint
            .as_ref()
            .map(|c| c.messages.iter().map(|m| m["content"].as_str().unwrap().to_owned()).collect())
    }
}
macro_rules! local_test { ($name:ident,$body:block) => { #[tokio::test(flavor="current_thread")] async fn $name() { tokio::task::LocalSet::new().run_until(async $body).await; } }; }
fn hang_once() -> Run {
    let n = Rc::new(Cell::new(0));
    Rc::new(move |_, _, _| {
        let hang = n.get() == 0;
        n.set(n.get() + 1);
        async move {
            if hang {
                futures::future::pending::<()>().await;
            }
            Ok(complete())
        }
        .boxed_local()
    })
}
fn spawned(session: &Session, text: &str) -> tokio::task::JoinHandle<Result<(), RuntimeError>> {
    let session = session.clone();
    let text = text.to_owned();
    tokio::task::spawn_local(async move { session.submit(&text, None).await })
}

local_test!(fresh_context_events_concurrency_and_stop_live, {
    let gate = Rc::new(Notify::new());
    let hold = gate.clone();
    let h = harness(
        Some(Rc::new(move |_, _, emit| {
            let gate = hold.clone();
            async move {
                emit(KernelEvent::Text { text: "hello".into() })?;
                emit(KernelEvent::ToolStart { id: "t1".into(), name: "read_fixture".into() })?;
                emit(KernelEvent::ToolEnd { id: "t1".into(), name: "read_fixture".into(), elapsed_ms: 3, is_error: false })?;
                gate.notified().await;
                Ok(TurnResult { usage: Some(Usage { input_tokens: 5., output_tokens: 2., ..Default::default() }), ..complete() })
            }
            .boxed_local()
        })),
        |_| {},
    );
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "question");
    settle().await;
    assert!(h.session.submit("second", None).await.unwrap_err().to_string().contains("busy"));
    assert_eq!(h.session.status().state, TurnState::Running);
    assert!(h.record.calls.borrow()[0].contains("fresh fixture context"));
    assert!(h.session.stop_live().await.unwrap());
    assert_eq!(h.record.stops.get(), 1);
    assert_eq!(h.session.status().state, TurnState::Running);
    gate.notify_one();
    running.await.unwrap().unwrap();
    assert_eq!(h.record.observations.get(), 2);
    assert_eq!(h.events.borrow().iter().filter(|e| matches!(e, SessionEvent::Kernel(KernelEvent::Text { .. }))).count(), 1);
    assert!(h
        .events
        .borrow()
        .iter()
        .any(|e| matches!(e,SessionEvent::TurnComplete{result,..} if result.usage.as_ref().is_some_and(|u|u.input_tokens==5.))));
    h.record.stop_works.set(false);
    assert!(!h.session.stop_live().await.unwrap());
    assert_eq!(h.events.borrow().iter().filter(|e| matches!(e, SessionEvent::Action(_))).count(), 1);
    assert_eq!(h.session.status().state, TurnState::Idle);
    h.session.close().await.unwrap();
});
local_test!(uncooperative_cancel_fences_late_events_and_recovers, {
    let emit = Rc::new(RefCell::new(None::<KernelEmit>));
    let old = emit.clone();
    let gate = Rc::new(Notify::new());
    let hold = gate.clone();
    let calls = Rc::new(Cell::new(0));
    let h = harness(
        Some(Rc::new(move |_, _, out| {
            let first = calls.get() == 0;
            calls.set(calls.get() + 1);
            let hold = hold.clone();
            if first {
                *old.borrow_mut() = Some(out.clone());
            }
            async move {
                if first {
                    hold.notified().await;
                } else {
                    out(KernelEvent::Text { text: "fresh".into() })?;
                }
                Ok(complete())
            }
            .boxed_local()
        })),
        |_| {},
    );
    h.session.start().await.unwrap();
    h.session.cancel().await.unwrap();
    let running = spawned(&h.session, "cancel me");
    settle().await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    h.session.submit("next", None).await.unwrap();
    emit.borrow().as_ref().unwrap()(KernelEvent::Text { text: "stale".into() }).unwrap();
    gate.notify_one();
    settle().await;
    assert_eq!(h.record.created.borrow().len(), 2);
    assert!(h.notice("Cancelled work was discarded"));
    assert_eq!(
        h.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Kernel(KernelEvent::Text { text }) => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["fresh"]
    );
    h.session.close().await.unwrap();
});
local_test!(cooperative_cancel_keeps_kernel_usage_and_finished_steps, {
    let n = Rc::new(Cell::new(0));
    let h = harness(
        Some(Rc::new(move |_, signal, _| {
            let first = n.get() == 0;
            n.set(n.get() + 1);
            async move {
                if first {
                    signal.cancelled().await;
                    Ok(cancelled())
                } else {
                    Ok(complete())
                }
            }
            .boxed_local()
        })),
        |_| {},
    );
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "cancel");
    settle().await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    h.session.submit("followup", None).await.unwrap();
    assert_eq!(h.record.created.borrow().len(), 1);
    assert_eq!(h.record.closes.get(), 0);
    assert!(h.events.borrow().iter().any(|e|matches!(e,SessionEvent::TurnComplete{result,..}if result.stop_reason==StopReason::Cancelled&&result.usage.as_ref().is_some_and(|u|u.input_tokens==4.))));
    h.session.close().await.unwrap();
    assert_eq!(h.session.status().connection, ConnectionState::Disconnected);
});
local_test!(uncooperative_timeout_is_bounded_and_quarantines_kernel, {
    let h = harness(Some(hang_once()), |o| o.timeout_ms = Some(15));
    h.session.start().await.unwrap();
    h.session.submit("timeout", None).await.unwrap();
    assert!(h.error("without progress"));
    h.session.submit("recovered", None).await.unwrap();
    assert_eq!(h.record.created.borrow().len(), 2);
    h.session.close().await.unwrap();
});
local_test!(progress_refreshes_quiet_timer_and_tool_work_suspends_it, {
    for tool in [false, true] {
        let h = harness(
            Some(Rc::new(move |_, _, emit| {
                async move {
                    if tool {
                        emit(KernelEvent::ToolStart { id: "t1".into(), name: "make_changes".into() })?;
                        delay(120).await;
                        emit(KernelEvent::ToolEnd { id: "t1".into(), name: "make_changes".into(), elapsed_ms: 120, is_error: false })?;
                    } else {
                        for _ in 0..8 {
                            delay(15).await;
                            emit(KernelEvent::Text { text: ".".into() })?;
                        }
                    }
                    Ok(complete())
                }
                .boxed_local()
            })),
            |o| o.idle_timeout_ms = Some(40),
        );
        h.session.start().await.unwrap();
        h.session.submit("work", None).await.unwrap();
        assert!(!h.error(""), "{:?}", h.events.borrow());
        h.session.close().await.unwrap();
    }
    let h = harness(
        Some(Rc::new(|_, signal, _| {
            async move {
                signal.cancelled().await;
                Ok(cancelled())
            }
            .boxed_local()
        })),
        |o| o.idle_timeout_ms = Some(40),
    );
    h.session.start().await.unwrap();
    h.session.submit("think", None).await.unwrap();
    assert!(h.error("Kumi stopped after 1 second without progress."));
    assert_eq!(h.record.created.borrow().len(), 1);
    h.session.close().await.unwrap();
});
local_test!(busy_turn_still_stops_at_hard_limit, {
    let h = harness(
        Some(Rc::new(|_, signal, emit| {
            async move {
                loop {
                    tokio::select! {_=signal.cancelled()=>break,_=delay(5)=>{emit(KernelEvent::Text{text:".".into()})?;}}
                }
                Ok(cancelled())
            }
            .boxed_local()
        })),
        |o| {
            o.idle_timeout_ms = Some(1000);
            o.turn_limit_ms = Some(60);
        },
    );
    h.session.start().await.unwrap();
    h.session.submit("forever", None).await.unwrap();
    assert!(h.error("this answer had run for"));
    h.session.close().await.unwrap();
});
local_test!(refresh_failure_never_calls_inference_or_exposes_private_errors, {
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    h.record.refresh_error.set(true);
    h.session.submit("current state?", None).await.unwrap();
    assert!(h.record.calls.borrow().is_empty());
    assert!(h.error("Context refresh failed"));
    assert!(!format!("{:?}", h.events.borrow()).contains("secret-token"));
    h.session.close().await.unwrap();
});
local_test!(set_identity_resets_but_label_and_tools_keep_context, {
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    h.session.submit("one", None).await.unwrap();
    h.observation.borrow_mut().label = "Renamed Set".into();
    h.session.refresh().await.unwrap();
    assert_eq!(h.session.status().observation.as_deref(), Some("Renamed Set"));
    assert_eq!(h.record.created.borrow().len(), 1);
    h.observation.borrow_mut().revision = Some("2".into());
    h.session.submit("two", None).await.unwrap();
    assert_eq!(h.words(1).unwrap(), ["one"]);
    assert!(!h.notice("fresh conversation"));
    h.observation.borrow_mut().key = "epoch:2".into();
    h.session.submit("three", None).await.unwrap();
    assert!(h.words(2).is_none());
    assert!(h.notice("open Set changed"));
    h.session.close().await.unwrap();
});
local_test!(model_reconfiguration_waits_for_running_answer_and_keeps_history, {
    let n = Rc::new(Cell::new(0));
    let gate = Rc::new(Notify::new());
    let hold = gate.clone();
    let h = harness(
        Some(Rc::new(move |_, _, _| {
            n.set(n.get() + 1);
            let wait = n.get() == 2;
            let hold = hold.clone();
            async move {
                if wait {
                    hold.notified().await;
                }
                Ok(complete())
            }
            .boxed_local()
        })),
        |_| {},
    );
    h.session.start().await.unwrap();
    h.session.submit("one", None).await.unwrap();
    let running = spawned(&h.session, "two");
    settle().await;
    h.session.reconfigure().await.unwrap();
    assert_eq!(h.record.created.borrow().len(), 1);
    gate.notify_one();
    running.await.unwrap().unwrap();
    h.session.submit("three", None).await.unwrap();
    assert_eq!(h.words(1).unwrap(), ["one", "two"]);
    h.session.close().await.unwrap();
});
local_test!(actionable_errors_keep_kind_and_provider, {
    let h = harness(
        Some(Rc::new(|_, _, _| {
            async { Err(KumiError::with_provider(FailureKind::Auth, "Not signed in to Anthropic.", "anthropic").into()) }.boxed_local()
        })),
        |_| {},
    );
    h.session.start().await.unwrap();
    h.session.submit("hello", None).await.unwrap();
    assert!(h.events.borrow().iter().any(|e|matches!(e,SessionEvent::Error{message,kind:Some(FailureKind::Auth),provider:Some(provider)}if message=="Not signed in to Anthropic."&&provider=="anthropic")));
    h.session.close().await.unwrap();
});
local_test!(disconnect_cancels_invalidates_and_offers_resend_once, {
    let h = harness(Some(hang_once()), |_| {});
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "live");
    settle().await;
    h.connection(ConnectionState::Disconnected, Some(DisconnectCause::Live));
    running.await.unwrap().unwrap();
    assert_eq!(h.session.status().observation, None);
    assert!(h.notice("Live closed"));
    // The note comes missing_after_ms (30) after the drop; a busy runner's timer fires later than that.
    for _ in 0..1000 {
        if h.notice("Is it open") {
            break;
        }
        delay(2).await;
    }
    assert!(h.notice("Is it open"));
    h.connection(ConnectionState::Connected, None);
    settle().await;
    assert!(h.events.borrow().iter().any(|e| matches!(e,SessionEvent::Resend{text}if text=="live")));
    h.session.submit("music theory", None).await.unwrap();
    h.connection(ConnectionState::Disconnected, Some(DisconnectCause::Bridge));
    h.connection(ConnectionState::Connected, None);
    settle().await;
    assert!(h.notice("link to Live dropped"));
    assert_eq!(h.events.borrow().iter().filter(|e| matches!(e, SessionEvent::Resend { .. })).count(), 1);
    h.session.close().await.unwrap();
});
local_test!(idle_disconnect_reobserves_without_replacing_kernel, {
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    h.session.submit("first", None).await.unwrap();
    h.connection(ConnectionState::Disconnected, None);
    let before = h.record.observations.get();
    h.connection(ConnectionState::Connected, None);
    settle().await;
    assert_eq!(h.record.observations.get(), before + 1);
    assert_eq!(h.record.created.borrow().len(), 1);
    assert!(h.notice("Live is back."));
    h.session.close().await.unwrap();
});
local_test!(new_conversation_keeps_bridge_and_resets_configured_limit, {
    let h = harness(None, |o| o.max_turns = Some(1));
    h.session.start().await.unwrap();
    h.session.submit("one", None).await.unwrap();
    assert!(h.session.submit("two", None).await.unwrap_err().to_string().contains("limit"));
    h.session.new_conversation().await.unwrap();
    assert_eq!(h.record.integration_closes.get(), 0);
    assert_eq!(h.record.closes.get(), 1);
    assert_eq!(h.session.status().turns, 0);
    h.session.submit("two", None).await.unwrap();
    h.session.close().await.unwrap();
});
local_test!(default_session_has_no_turn_limit_and_validates_prompts, {
    let h = harness(None, |_| {});
    assert!(h.session.submit("x", None).await.is_err());
    h.session.start().await.unwrap();
    assert!(h.session.submit(" \u{feff}", None).await.is_err());
    assert!(h.session.submit(&"x".repeat(16385), None).await.is_err());
    for n in 0..30 {
        h.session.submit(&n.to_string(), None).await.unwrap();
    }
    assert_eq!(h.session.status().turns, 30);
    h.session.close().await.unwrap();
    assert!(h.session.submit("x", None).await.is_err());
});
local_test!(startup_failure_cleans_integration_and_late_kernel_is_closed, {
    let h = harness(None, |o| o.kernel_factory = Rc::new(|_| async { Err(RuntimeError::plain("private startup failure")) }.boxed_local()));
    assert!(h.session.start().await.is_err());
    assert_eq!(h.record.integration_closes.get(), 1);
    assert!(!format!("{:?}", h.events.borrow()).contains("private"));
    h.session.close().await.unwrap();
    let (tx, rx) = oneshot::channel::<Rc<dyn Kernel>>();
    let held = Rc::new(RefCell::new(Some(rx)));
    let h = harness(None, |o| {
        o.kernel_factory = Rc::new(move |_| {
            let rx = held.borrow_mut().take().unwrap();
            async move { Ok(rx.await.unwrap()) }.boxed_local()
        })
    });
    let s = h.session.clone();
    let starting = tokio::task::spawn_local(async move { s.start().await });
    settle().await;
    h.session.close().await.unwrap();
    let kernel = Rc::new(TestKernel { record: h.record.clone(), history: RefCell::new(vec![]), run: None });
    assert!(tx.send(kernel).is_ok());
    let _ = starting.await;
    settle().await;
    assert_eq!(h.record.closes.get(), 1);
    assert_eq!(h.session.status().state, TurnState::Closed);
});
local_test!(a_first_start_stopped_before_it_finished_finishes_once_live_is_there, {
    async fn started(h: &Harness) -> bool {
        for _ in 0..500 {
            match h.session.submit("hello", None).await {
                Ok(()) => return true,
                Err(error) if error.message().contains("busy") => delay(2).await,
                Err(_) => return false,
            }
        }
        false
    }
    // Live drops while the first start reads the Set, then comes back.
    let h = harness(None, |_| {});
    h.record.observe_holds.set(true);
    let s = h.session.clone();
    let starting = tokio::task::spawn_local(async move { s.start().await });
    for _ in 0..500 {
        if h.record.observations.get() > 0 {
            break;
        }
        delay(1).await;
    }
    let first = h.record.listeners.borrow()[0].clone();
    first(ConnectionState::Disconnected, Some(DisconnectCause::Live));
    let _ = starting.await.unwrap();
    h.record.observe_holds.set(false);
    first(ConnectionState::Connected, None);
    assert!(started(&h).await, "the session started once Live was back");
    h.session.close().await.unwrap();
    // The first start's wait runs out before Live is there; then Live connects.
    let h = harness(None, |o| o.timeout_ms = Some(30));
    h.record.observe_holds.set(true);
    let _ = h.session.start().await;
    h.record.observe_holds.set(false);
    let first = h.record.listeners.borrow()[0].clone();
    first(ConnectionState::Connected, None);
    assert!(started(&h).await, "the session started once Live connected");
    h.session.close().await.unwrap();
});
local_test!(reconnect_carries_same_set_but_other_saved_set_starts_fresh, {
    let h = harness(None, |_| {});
    h.observation.borrow_mut().project = Some(ProjectRef { id: "p1".into(), name: "Set A".into() });
    h.session.start().await.unwrap();
    h.session.submit("one", None).await.unwrap();
    h.session.reconnect().await.unwrap();
    assert_eq!(h.words(1).unwrap(), ["one"]);
    assert_eq!(h.record.integration_closes.get(), 1);
    // A closed integration's late callback cannot change the new connection.
    let old = h.record.listeners.borrow()[0].clone();
    old(ConnectionState::Disconnected, None);
    assert_eq!(h.session.status().connection, ConnectionState::Connected);
    h.observation.borrow_mut().project = Some(ProjectRef { id: "p2".into(), name: "Set B".into() });
    h.session.reconnect().await.unwrap();
    assert!(h.words(2).is_none());
    h.session.close().await.unwrap();
});

local_test!(added_files_go_with_the_words_and_pictures_to_the_model, {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: usize| {
        let path = dir.path().join(name);
        std::fs::write(&path, vec![7u8; bytes]).unwrap();
        path
    };
    let attach = |path: &std::path::Path, media_type: &str| Attachment {
        path: path.to_string_lossy().into_owned(),
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        media_type: media_type.into(),
        bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
    };
    let (picture, reference) = (file("synth.png", 1500), file("reference.wav", 3 * 1024 * 1024 + 1));
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    h.session.submit_with("make this", None, vec![attach(&picture, "image/png"), attach(&reference, "audio/wav")]).await.unwrap();
    {
        let calls = h.record.calls.borrow();
        assert_eq!(calls[0], "pictures: synth.png image/png 1500");
        let said = format!(
            "make this\n\n[The producer added: synth.png (image/png, 2 KB) at {}; reference.wav (audio/wav, 3.0 MB) at {}]",
            picture.display(),
            reference.display()
        );
        assert!(calls[1].starts_with(&format!("{said}{OBSERVATION_MARKER}")), "{}", calls[1]);
    }
    // What can't go is refused before anything is sent, with what to do instead.
    let missing = dir.path().join("gone.png");
    let cases = [
        (attach(&missing, "image/png"), "gone.png isn't there any more; add it again."),
        (attach(&file("scan.tiff", 10), "image/tiff"), "The model sees PNG, JPEG, GIF and WebP pictures; save scan.tiff as one of those"),
        (attach(&file("huge.png", 4 * 1024 * 1024), "image/png"), "huge.png is 4.0 MB; the model takes pictures up to 3.75 MB."),
        (attach(dir.path(), "application/octet-stream"), "is a folder; add the files in it instead."),
    ];
    for (attachment, refusal) in cases {
        let error = h.session.submit_with("make this", None, vec![attachment]).await.unwrap_err();
        assert!(error.message().contains(refusal), "{}", error.message());
    }
    let big = |name: &str| attach(&file(name, 3 * 1024 * 1024), "image/png");
    let error = h.session.submit_with("make this", None, (0..7).map(|i| big(&format!("shot{i}.png"))).collect()).await.unwrap_err();
    assert!(error.message().contains("one message takes up to 20 MB of them"), "{}", error.message());
    let many = vec![attach(&reference, "audio/wav"); 11];
    assert!(h.session.submit_with("make this", None, many).await.unwrap_err().message().contains("at most 10 files"));
    assert_eq!(h.record.calls.borrow().len(), 2);
    h.session.close().await.unwrap();
});
async fn saved(store: &dyn ConversationStore, place: &str, turns: u32) -> CurrentConversation {
    // Up to 2 s: a busy Windows runner writes slowly, and close waits for the save only so long.
    for _ in 0..1000 {
        if let Some(kept) = store.current(place).await.unwrap() {
            if kept.conversation.turns == Some(turns) {
                return kept;
            }
        }
        delay(2).await;
    }
    panic!("conversation was not saved");
}
fn project(h: &Harness, id: &str) {
    let mut o = h.observation.borrow_mut();
    o.key = id.into();
    o.project = Some(ProjectRef { id: id.into(), name: id.into() });
}
local_test!(saved_conversation_restart_new_and_explicit_resume, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let one = harness(None, |o| o.conversations = Some(store.clone()));
    project(&one, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    one.session.start().await.unwrap();
    one.session.submit("remember 42", None).await.unwrap();
    one.session.close().await.unwrap();
    let kept = saved(store.as_ref(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 1).await;
    assert_eq!(kept.conversation.turns, Some(1));
    let two = harness(None, |o| o.conversations = Some(store.clone()));
    project(&two, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    two.session.start().await.unwrap();
    assert_eq!(two.words(0).unwrap(), ["remember 42"]);
    assert!(two.events.borrow().iter().any(|e| matches!(e,SessionEvent::Resumed{lines,..}if lines[0].text=="remember 42")));
    two.session.new_conversation().await.unwrap();
    assert_eq!(two.record.integration_closes.get(), 0);
    assert!(two.words(1).is_none());
    assert!(two.notice("The last one is kept"));
    two.session.submit("a new idea", None).await.unwrap();
    // The kept conversation already has a turn, so waiting for one says nothing about the new one:
    // wait until both are listed (up to 2 s; a busy Windows runner writes slowly).
    let mut listed = vec![];
    for _ in 0..1000 {
        listed = two.session.conversations().await.unwrap();
        if listed.len() == 2 {
            break;
        }
        delay(2).await;
    }
    assert_eq!(listed.len(), 2);
    let original = listed.iter().find(|r| r.first == "remember 42").unwrap();
    assert!(two.session.resume_conversation(&original.id).await.unwrap());
    assert_eq!(two.words(2).unwrap(), ["remember 42"]);
    assert!(two.events.borrow().iter().any(|e| matches!(e, SessionEvent::Resumed { chosen: Some(true), .. })));
    two.session.close().await.unwrap();
    let mut current = None;
    for _ in 0..1000 {
        current = store.current("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").await.unwrap().map(|kept| kept.id);
        if current.as_deref() == Some(original.id.as_str()) {
            break;
        }
        delay(2).await;
    }
    assert_eq!(current.as_deref(), Some(original.id.as_str()));
});
local_test!(unsaved_conversation_moves_with_first_save, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let h = harness(None, |o| o.conversations = Some(store.clone()));
    h.session.start().await.unwrap();
    h.session.submit("sketch a progression", None).await.unwrap();
    saved(store.as_ref(), "unsaved", 1).await;
    h.observation.borrow_mut().project = Some(ProjectRef { id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(), name: "Sketch".into() });
    h.session.submit("now add drums", None).await.unwrap();
    h.session.close().await.unwrap();
    assert!(store.list("unsaved").await.unwrap().is_empty());
    let kept = store.current("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").await.unwrap().unwrap();
    assert_eq!(
        kept.conversation.checkpoint.messages.iter().map(|v| v["content"].as_str().unwrap()).collect::<Vec<_>>(),
        ["sketch a progression", "now add drums"]
    );
});
local_test!(unreadable_kept_history_is_shown_and_retained, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store
        .save(
            "cccccccccccccccccccccccccccccccc",
            "old001",
            &SavedConversation {
                saved_at: 1,
                checkpoint: KernelCheckpoint {
                    version: 1,
                    messages: vec![json!({"role":"user","content":"from another model"}), json!({"role":"assistant","content":"sure"})],
                    origin: None,
                    tools: None,
                },
                changes: None,
                first: None,
                turns: None,
            },
        )
        .await
        .unwrap();
    let h = harness(None, |o| {
        o.conversations = Some(store.clone());
        let original = o.kernel_factory.clone();
        o.kernel_factory = Rc::new(move |options| {
            let original = original.clone();
            async move {
                if options.checkpoint.is_some() {
                    Err(RuntimeError::plain("Unsupported checkpoint version."))
                } else {
                    original(options).await
                }
            }
            .boxed_local()
        });
    });
    project(&h, "cccccccccccccccccccccccccccccccc");
    h.session.start().await.unwrap();
    assert!(h.events.borrow().iter().any(|e|matches!(e,SessionEvent::Resumed{unreadable:Some(true),lines,..}if lines.iter().map(|l|l.text.as_str()).collect::<Vec<_>>()==["from another model","sure"])));
    assert!(h.notice("couldn't continue that conversation"));
    assert_eq!(store.list("cccccccccccccccccccccccccccccccc").await.unwrap().len(), 1);
    h.session.close().await.unwrap();
});
fn change(id: &str, state: &str, at: i64) -> ChangeRecord {
    serde_json::from_value(
        json!({"id":id,"family":"tempo","title":"Tempo 120 → 124 BPM","state":state,"at":at,"clip":{"length":4,"notes":[]}}),
    )
    .unwrap()
}
local_test!(earlier_process_changes_expire_without_pictures_and_same_process_keeps_undo, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let one = harness(None, |o| o.conversations = Some(store.clone()));
    project(&one, "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    one.session.start().await.unwrap();
    one.session.watch(WatchEvent::Change(change("c1", "applied", 1)));
    one.session.watch(WatchEvent::Change(change("c2", "undone", 2)));
    one.session.submit("turn bass down", None).await.unwrap();
    saved(store.as_ref(), "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", 1).await;
    project(&one, "ffffffffffffffffffffffffffffffff");
    one.session.submit("hello F", None).await.unwrap();
    saved(store.as_ref(), "ffffffffffffffffffffffffffffffff", 1).await;
    project(&one, "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    one.session.submit("back to E", None).await.unwrap();
    assert!(one.events.borrow().iter().any(|e| matches!(e,SessionEvent::Resumed{lines,changes:None,..}if lines[0].text=="turn bass down")));
    one.session.close().await.unwrap();
    let two = harness(None, |o| o.conversations = Some(store.clone()));
    project(&two, "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    two.session.start().await.unwrap();
    let events = two.events.borrow();
    let changes = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::Resumed { changes: Some(c), .. } => Some(c),
            _ => None,
        })
        .unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].state, ChangeState::Expired);
    assert_eq!(changes[1].state, ChangeState::Undone);
    assert!(changes.iter().all(|c| c.id.contains(':') && c.clip.is_none()));
    drop(events);
    two.session.close().await.unwrap();
});
local_test!(settled_history_is_recovered_after_uncooperative_work, {
    let n = Rc::new(Cell::new(0));
    let h = harness(
        Some(Rc::new(move |_, _, _| {
            n.set(n.get() + 1);
            let hang = n.get() == 2;
            async move {
                if hang {
                    futures::future::pending::<()>().await;
                }
                Ok(complete())
            }
            .boxed_local()
        })),
        |_| {},
    );
    h.session.start().await.unwrap();
    h.session.submit("finished", None).await.unwrap();
    let running = spawned(&h.session, "unfinished");
    settle().await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    h.session.submit("next", None).await.unwrap();
    assert_eq!(h.words(1).unwrap(), ["finished"]);
    h.session.close().await.unwrap();
});
local_test!(memory_instructions_tools_and_forgetting, {
    use kumi_runtime::core::memory::{create_memory_store, MemoryStoreOptions};
    let dir = tempfile::tempdir().unwrap();
    let store = create_memory_store(MemoryStoreOptions {
        projects_dir: dir.path().join("projects"),
        producer_file: dir.path().join("producer.json"),
    });
    store
        .save(MemoryScope::Producer, None, &[MemoryNote { id: "p1".into(), text: "Prefers short reverbs".into(), at: 1, pinned: false }])
        .await
        .unwrap();
    let h = harness(None, |o| o.memory = Some(store.clone()));
    h.session.start().await.unwrap();
    let tools = {
        let created = h.record.created.borrow();
        assert!(created[0].instructions.contains("[p1] Prefers short reverbs"));
        created[0].tools.clone()
    };
    assert_eq!(tools.iter().map(|t| t.name()).collect::<Vec<_>>(), ["remember", "forget"]);
    tools[0]
        .execute(json!({"note":"The Reese is the main bass","about":"producer"}).as_object().unwrap().clone(), Signal::new())
        .await
        .unwrap();
    assert_eq!(h.session.memory().await.unwrap().unwrap().memory.producer.len(), 2);
    assert_eq!(h.session.forget("p1").await.unwrap().unwrap().text, "Prefers short reverbs");
    h.session.close().await.unwrap();
});

local_test!(willington_instructions_follow_the_switch_each_time_the_kernel_is_made, {
    use kumi_runtime::integrations::ableton::willington::WillingtonSwitch;
    let switch = Rc::new(Cell::new(WillingtonSwitch::Off));
    let read = switch.clone();
    let h = harness(None, move |o| o.willington = Some(Rc::new(move || Some(read.get()))));
    // Kumi can change the Set (make_changes): the instructions say what the bindings would add.
    h.observation.borrow_mut().tools = vec![Rc::new(Plan { received: Rc::new(RefCell::new(vec![])) })];
    h.session.start().await.unwrap();
    assert!(h.record.created.borrow()[0].instructions.contains("/willington turns the bindings on"));
    // Turned on a moment ago, before the bridge offers their tools: nothing said about them yet.
    switch.set(WillingtonSwitch::JustOn);
    h.session.reconfigure().await.unwrap();
    h.session.refresh().await.unwrap();
    let instructions = h.record.created.borrow().last().unwrap().instructions.clone();
    assert!(!instructions.contains("Willington"), "{instructions}");
    // On for a while, for a Live they have no bindings for: the next kernel says so, without pointing at /willington.
    switch.set(WillingtonSwitch::On);
    h.session.reconfigure().await.unwrap();
    h.session.refresh().await.unwrap();
    let instructions = h.record.created.borrow().last().unwrap().instructions.clone();
    assert!(instructions.contains("none fit the Live that's open") && !instructions.contains("/willington"));
    // On, with Willington's mapping and Python in Live offered: the next kernel says modulators map, and how,
    // rather than that they can't (a tutorial's LFOs were rebuilt with automation when it said so).
    {
        let tools = &mut h.observation.borrow_mut().tools;
        tools.push(Rc::new(Offered("edit_rack_mapping")));
        tools.push(Rc::new(Offered("run_python")));
    }
    h.session.reconfigure().await.unwrap();
    h.session.refresh().await.unwrap();
    let instructions = h.record.created.borrow().last().unwrap().instructions.clone();
    assert!(instructions.contains("map with map_modulator") && !instructions.contains("can't be mapped"), "{instructions}");
    h.session.close().await.unwrap();
});

/// A tool the integration offers, by name only: what the instructions say depends on which are offered.
struct Offered(&'static str);
#[async_trait(?Send)]
impl KernelTool for Offered {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "Offered."
    }
    fn input_schema(&self) -> JsonObject {
        JsonObject::new()
    }
    async fn execute(&self, _: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        panic!("only offered")
    }
}

#[derive(Clone)]
struct Plan {
    received: Rc<RefCell<Vec<JsonObject>>>,
}
#[async_trait(?Send)]
impl KernelTool for Plan {
    fn name(&self) -> &str {
        "make_changes"
    }
    fn description(&self) -> &str {
        "Make changes."
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","properties":{"steps":{"type":"array"}},"additionalProperties":false}).as_object().unwrap().clone()
    }
    async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        self.received.borrow_mut().push(input);
        Ok(ToolResult::text("done"))
    }
    fn stream(&self, _: Signal, _: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        Some(Box::new(PlanStream { plan: self.clone(), started: Cell::new(false) }))
    }
}
struct PlanStream {
    plan: Plan,
    started: Cell<bool>,
}
#[async_trait(?Send)]
impl StreamingCall for PlanStream {
    fn push(&self, _: &str) {
        self.started.set(true);
    }
    async fn finish(&self, input: Option<JsonObject>) -> Result<ToolResult, RuntimeError> {
        self.plan.execute(input.unwrap(), Signal::new()).await
    }
    async fn abandon(&self) {}
    fn started(&self) -> bool {
        self.started.get()
    }
}
fn draft() -> Value {
    json!({"name":"Reese stack","fits":"wide Reese basses","idea":"Two detuned saws with glide, then saturation and a low cut."})
}
local_test!(technique_is_taken_out_of_whole_and_streamed_plans_and_offered_after_the_answer, {
    use kumi_runtime::core::techniques::{create_technique_store, TechniqueStore};
    let dir = tempfile::tempdir().unwrap();
    let store = create_technique_store(dir.path().join("techniques.json"));
    let received = Rc::new(RefCell::new(vec![]));
    let tools = Rc::new(RefCell::new(Vec::<Rc<dyn KernelTool>>::new()));
    let streamed = Rc::new(Cell::new(false));
    let session = Rc::new(RefCell::new(None::<Session>));
    let use_tools = tools.clone();
    let way = streamed.clone();
    let active = session.clone();
    let run: Run = Rc::new(move |_, signal, _| {
        let plan = use_tools.borrow().iter().find(|t| t.name() == "make_changes").unwrap().clone();
        let streamed = way.get();
        let session = active.borrow().clone().unwrap();
        async move {
            let input = json!({"steps":[{"tool":"load_device"}],"technique":draft()}).as_object().unwrap().clone();
            if streamed {
                let call = plan.stream(signal, Rc::new(|| {})).unwrap();
                assert!(!call.started());
                call.push("{");
                assert!(call.started());
                call.finish(Some(input)).await?;
            } else {
                plan.execute(input, signal).await?;
            }
            session.watch(WatchEvent::Change(change("c1", "applied", 1)));
            Ok(complete())
        }
        .boxed_local()
    });
    let h = harness(Some(run), |o| {
        o.techniques = Some(store.clone());
        o.gaps = Some(dir.path().join("gaps.jsonl").to_string_lossy().into_owned());
        let factory = o.kernel_factory.clone();
        o.kernel_factory = Rc::new(move |options| {
            *tools.borrow_mut() = options.tools.clone();
            factory(options)
        });
    });
    *session.borrow_mut() = Some(h.session.clone());
    h.observation.borrow_mut().tools.push(Rc::new(Plan { received: received.clone() }));
    h.session.start().await.unwrap();
    let plan = h.record.created.borrow()[0].tools[0].clone();
    assert!(plan.description().starts_with("Make changes. When the plan builds"));
    assert!(plan.input_schema()["properties"]["technique"].is_object());
    let actions = || {
        h.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Technique(t) => Some(t.action),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    h.session.submit("build a Reese", None).await.unwrap();
    assert_eq!(received.borrow().last().unwrap(), json!({"steps":[{"tool":"load_device"}]}).as_object().unwrap());
    assert_eq!(actions(), [TechniqueAction::Offered]);
    let order = h
        .events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            SessionEvent::TurnComplete { .. } => Some("answered"),
            SessionEvent::Technique(_) => Some("offered"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(order, ["answered", "offered"], "the offer comes after the answer, so the answer's own question comes first");
    // Playing it isn't a yes; the producer's answer is.
    h.session.watch(WatchEvent::Action(ActionEvent { title: "Playing".into(), playing: Some(true), recording: None }));
    assert!(store.list().await.unwrap().is_empty());
    assert!(h.session.answer_technique(true).await.unwrap());
    assert_eq!(actions(), [TechniqueAction::Offered, TechniqueAction::Kept]);
    assert_eq!(h.session.techniques().await.unwrap()[0].name, "Reese stack");
    assert_eq!(store.list().await.unwrap()[0].request.as_deref(), Some("build a Reese"));
    assert!(!h.session.answer_technique(true).await.unwrap(), "answered once, nothing waits");
    // A streamed plan loses its technique too; right after an offer, Kumi doesn't ask again.
    streamed.set(true);
    h.session.submit("build another Reese", None).await.unwrap();
    assert_eq!(received.borrow().last().unwrap(), json!({"steps":[{"tool":"load_device"}]}).as_object().unwrap());
    assert_eq!(actions(), [TechniqueAction::Offered, TechniqueAction::Kept]);
    let id = store.list().await.unwrap()[0].id.clone();
    assert!(h.session.forget_technique(&id).await.unwrap());
    assert!(store.list().await.unwrap().is_empty());
    let result = plan
        .execute(json!({"steps":[{"tool":"load_device"},{"tool":"load_device"}]}).as_object().unwrap().clone(), Signal::new())
        .await
        .unwrap();
    assert!(!result.text.contains("technique"), "nothing nudges the model to draft one");
    h.session.close().await.unwrap();
});
local_test!(a_save_or_silence_keeps_nothing_a_failed_answer_offers_nothing_and_a_yes_in_words_keeps_it, {
    use kumi_runtime::core::techniques::{create_technique_store, TechniqueStore, TECHNIQUE_TOOL};
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = create_technique_store(dir.path().join("techniques.json"));
        let tools = Rc::new(RefCell::new(Vec::<Rc<dyn KernelTool>>::new()));
        let session = Rc::new(RefCell::new(None::<Session>));
        let use_tools = tools.clone();
        let active = session.clone();
        let count = Rc::new(Cell::new(0));
        let run: Run = Rc::new(move |input, signal, _| {
            let turn = count.get();
            count.set(turn + 1);
            let tool = use_tools.borrow().iter().find(|t| t.name() == TECHNIQUE_TOOL).unwrap().clone();
            let session = active.borrow().clone().unwrap();
            async move {
                if turn == 0 {
                    session.watch(WatchEvent::Change(change("c1", "applied", 1)));
                    let mut input = draft().as_object().unwrap().clone();
                    input.insert("action".into(), json!("draft"));
                    tool.execute(input, signal).await?;
                    if fail {
                        return Err(RuntimeError::plain("provider went away"));
                    }
                } else if input.contains("whether to keep “Reese stack” as a technique") {
                    // The producer's words say yes, so the model keeps it.
                    tool.execute(json!({"action":"keep"}).as_object().unwrap().clone(), signal).await?;
                }
                Ok(complete())
            }
            .boxed_local()
        });
        let h = harness(Some(run), |o| {
            o.techniques = Some(store.clone());
            let factory = o.kernel_factory.clone();
            o.kernel_factory = Rc::new(move |options| {
                *tools.borrow_mut() = options.tools.clone();
                factory(options)
            });
        });
        *session.borrow_mut() = Some(h.session.clone());
        project(&h, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        h.observation.borrow_mut().saved_at = Some(1000.);
        h.session.start().await.unwrap();
        h.session.submit("build a Reese", None).await.unwrap();
        // Saving the Set isn't a yes.
        h.observation.borrow_mut().saved_at = Some(2000.);
        h.session.refresh().await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
        h.session.submit("keep the technique, then add a riser", None).await.unwrap();
        let input = h.record.calls.borrow().last().unwrap().clone();
        let noted = input.find("whether to keep “Reese stack” as a technique");
        assert_eq!(noted.is_some(), !fail, "the next turn hears of the offer only when there was one");
        if let Some(noted) = noted {
            assert!(noted > input.find("</current_observation_untrusted>").unwrap(), "Kumi's note comes after the observation");
            assert!(transcript_of(&[json!({"role":"user","content":input})]).iter().all(|line| !line.text.contains("[Kumi]")));
        }
        h.session.close().await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), usize::from(!fail));
        assert_eq!(h.error("Inference failed"), fail);
    }
});
local_test!(a_new_or_resumed_conversation_lets_a_waiting_offer_go, {
    use kumi_runtime::core::techniques::{create_technique_store, TechniqueStore, TECHNIQUE_TOOL};
    let dir = tempfile::tempdir().unwrap();
    let store = create_technique_store(dir.path().join("techniques.json"));
    let tools = Rc::new(RefCell::new(Vec::<Rc<dyn KernelTool>>::new()));
    let session = Rc::new(RefCell::new(None::<Session>));
    let use_tools = tools.clone();
    let active = session.clone();
    let run: Run = Rc::new(move |input, signal, _| {
        let tool = use_tools.borrow().iter().find(|t| t.name() == TECHNIQUE_TOOL).unwrap().clone();
        let session = active.borrow().clone().unwrap();
        async move {
            if input.starts_with("build a Reese") {
                session.watch(WatchEvent::Change(change("c1", "applied", 1)));
                let mut input = draft().as_object().unwrap().clone();
                input.insert("action".into(), json!("draft"));
                tool.execute(input, signal).await?;
            }
            Ok(complete())
        }
        .boxed_local()
    });
    let h = harness(Some(run), |o| {
        o.techniques = Some(store.clone());
        let factory = o.kernel_factory.clone();
        o.kernel_factory = Rc::new(move |options| {
            *tools.borrow_mut() = options.tools.clone();
            factory(options)
        });
    });
    *session.borrow_mut() = Some(h.session.clone());
    h.session.start().await.unwrap();
    h.session.submit("build a Reese", None).await.unwrap();
    h.session.new_conversation().await.unwrap();
    h.session.submit("keep the technique", None).await.unwrap();
    assert!(
        !h.record.calls.borrow().last().unwrap().contains("[Kumi] After your last answer"),
        "the offer stayed with the old conversation"
    );
    assert!(!h.session.answer_technique(true).await.unwrap());
    h.session.close().await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
});

use kumi_runtime::core::match_run::{MatchBudget, MatchState, MatchStop, MATCH_BUDGET};
fn audition(score: f64, label: &str) -> AuditionEvent {
    serde_json::from_value(json!({"round":1,"best":{"label":label,"score":score},"takes":[{"label":label,"score":score,"where":{"track":"Candidate"}},{"label":"Other","score":score-10.}],"gaps":["attack too slow"],"request":{"candidates":[{"track":"track:1","label":"Drift"}],"fromBeat":16,"beats":4,"reference":"~/ref.wav"}})).unwrap()
}
fn match_harness(rounds: Vec<Option<(&str, f64)>>, config: impl FnOnce(&mut SessionOptions)) -> Harness {
    let rounds = rounds.into_iter().map(|r| r.map(|(l, s)| (l.to_owned(), s))).collect::<Vec<_>>();
    let session = Rc::new(RefCell::new(None::<Session>));
    let active = session.clone();
    let count = Rc::new(Cell::new(0));
    let run: Run = Rc::new(move |_, signal, emit| {
        let n = count.get();
        count.set(n + 1);
        let event = rounds.get(n).cloned().flatten();
        let session = active.borrow().clone().unwrap();
        async move {
            if let Some((label, score)) = event {
                session.watch(WatchEvent::Audition(audition(score, &label)));
            }
            if signal.is_cancelled() {
                return Err(RuntimeError::Aborted);
            }
            emit(KernelEvent::Text { text: format!("answer {}", n + 1) })?;
            Ok(TurnResult {
                stop_reason: StopReason::Completed,
                usage: Some(Usage { input_tokens: 10., output_tokens: 5., ..Default::default() }),
            })
        }
        .boxed_local()
    });
    let h = harness(Some(run), config);
    *session.borrow_mut() = Some(h.session.clone());
    h
}
local_test!(match_rounds_reach_target_wrap_up_once_and_sum_usage, {
    let h = match_harness(vec![Some(("Drift", 50.)), Some(("Drift", 60.)), Some(("Drift", 75.)), Some(("Drift", 93.)), None], |_| {});
    h.session.start().await.unwrap();
    h.session.submit("make my pad sound like this reference", None).await.unwrap();
    let calls = h.record.calls.borrow();
    assert_eq!(calls.len(), 5);
    assert!(calls[1].starts_with(
        "[Kumi] Score 50% (best: Drift). Budget left: 12 rounds, about 45 minutes. Biggest gaps: attack too slow. Keep going"
    ));
    assert!(calls[2].starts_with("[Kumi] Score 50% → 60%"));
    assert!(calls[4].contains("That reaches 93%"));
    drop(calls);
    let statuses =
        h.events.borrow().iter().filter_map(|e| if let SessionEvent::Match(m) = e { Some(m.clone()) } else { None }).collect::<Vec<_>>();
    let last = statuses.last().unwrap();
    assert_eq!(last.state, MatchState::Done);
    assert_eq!(last.stop, Some(MatchStop::Reached));
    assert_eq!(last.first, Some(50.));
    let complete = h
        .events
        .borrow()
        .iter()
        .filter_map(|e| if let SessionEvent::TurnComplete { result, .. } = e { Some(result.clone()) } else { None })
        .collect::<Vec<_>>();
    assert_eq!(complete.len(), 1);
    assert_eq!(complete[0].usage.as_ref().unwrap().input_tokens, 50.);
    h.session.close().await.unwrap();
});
local_test!(match_plateau_requires_new_ideas_and_carry_on_ends_after_other_request, {
    let h = match_harness(
        vec![
            Some(("Drift", 50.)),
            Some(("Drift", 51.)),
            Some(("Drift", 51.)),
            Some(("Drift", 51.)),
            Some(("Drift", 52.)),
            Some(("Drift", 52.)),
            None,
        ],
        |_| {},
    );
    h.session.start().await.unwrap();
    h.session.submit("recreate this sound", None).await.unwrap();
    assert!(h.record.calls.borrow()[3].contains("Try something genuinely different now"));
    assert!(h.record.calls.borrow().last().unwrap().contains("Refining and new ideas both stopped gaining"));
    h.session.close().await.unwrap();
    let h = match_harness(
        vec![Some(("Drift", 50.)), Some(("Drift", 54.)), Some(("Drift", 58.)), None, Some(("Drift", 70.)), None, None, None, None, None],
        |o| o.match_budget = Some(MatchBudget { rounds: 2, ..MATCH_BUDGET }),
    );
    h.session.start().await.unwrap();
    h.session.submit("match this reference", None).await.unwrap();
    assert!(h.record.calls.borrow()[3].contains("That's the run's budget spent"));
    h.session.submit("keep going", None).await.unwrap();
    assert!(h.record.calls.borrow()[5].starts_with("[Kumi] Score 70% (best: Drift)"));
    let before = h.record.calls.borrow().len();
    h.session.submit("make a bass", None).await.unwrap();
    h.session.submit("keep going", None).await.unwrap();
    assert_eq!(h.record.calls.borrow().len(), before + 2);
    h.session.close().await.unwrap();
});
local_test!(match_reauditions_changed_candidate_before_deciding_and_can_cancel, {
    let session = Rc::new(RefCell::new(None::<Session>));
    let active = session.clone();
    let n = Rc::new(Cell::new(0));
    let h = harness(
        Some(Rc::new(move |_, _, _| {
            let n0 = n.get();
            n.set(n0 + 1);
            let session = active.borrow().clone().unwrap();
            async move {
                if n0 == 0 {
                    session.watch(WatchEvent::Audition(audition(50., "Drift")));
                    session.watch(WatchEvent::Change(change("c1", "applied", 1)));
                }
                Ok(complete())
            }
            .boxed_local()
        })),
        |_| {},
    );
    *session.borrow_mut() = Some(h.session.clone());
    let active = h.session.clone();
    let checked = Rc::new(Cell::new(0));
    let counts = checked.clone();
    *h.record.audition.borrow_mut() = Some(Rc::new(move |_| {
        counts.set(counts.get() + 1);
        active.watch(WatchEvent::Audition(audition(94., "Drift")));
    }));
    h.session.start().await.unwrap();
    h.session.submit("make it sound like this", None).await.unwrap();
    assert_eq!(checked.get(), 1);
    assert_eq!(h.record.calls.borrow().len(), 1);
    assert!(h.events.borrow().iter().any(|e| matches!(e, SessionEvent::Match(m) if m.stop == Some(MatchStop::Reached))));
    h.session.close().await.unwrap();
    let session = Rc::new(RefCell::new(None::<Session>));
    let active = session.clone();
    let n = Rc::new(Cell::new(0));
    let h = harness(
        Some(Rc::new(move |_, signal, _| {
            let first = n.get() == 0;
            n.set(n.get() + 1);
            let session = active.borrow().clone().unwrap();
            async move {
                if first {
                    session.watch(WatchEvent::Audition(audition(50., "Drift")));
                } else {
                    signal.cancelled().await;
                    return Err(RuntimeError::Aborted);
                }
                Ok(complete())
            }
            .boxed_local()
        })),
        |_| {},
    );
    *session.borrow_mut() = Some(h.session.clone());
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "make it sound like the reference");
    settle().await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(h.session.status().state, TurnState::Idle);
    h.session.close().await.unwrap();
});
local_test!(match_lessons_are_learned_updated_judged_read_and_forgotten, {
    use kumi_runtime::core::playbook::{create_playbook_store, PlaybookStore, Reaction};
    let dir = tempfile::tempdir().unwrap();
    let store = create_playbook_store(dir.path().join("lessons.json"));
    let h = match_harness(
        vec![
            Some(("Operator FM", 52.)),
            Some(("Collision", 64.)),
            Some(("Collision + parallel delays", 73.)),
            Some(("Collision + parallel delays", 73.)),
            Some(("Collision + parallel delays", 74.)),
            Some(("Collision + parallel delays", 74.)),
            Some(("Collision + parallel delays", 74.)),
            None,
            Some(("Collision, brighter", 93.)),
            None,
            None,
        ],
        |o| o.playbook = Some(store.clone()),
    );
    h.session.start().await.unwrap();
    h.session.submit("make my plucked metallic percussion sound like this reference", None).await.unwrap();
    let listed = h.session.lessons().await.unwrap();
    assert_eq!(listed.len(), 1);
    let lessons = store.list().await.unwrap();
    assert_eq!(lessons[0].matched, "plucked metallic percussion");
    assert_eq!((lessons[0].from, lessons[0].to), (52., 74.));
    h.session.submit("keep going", None).await.unwrap();
    h.session.lessons().await.unwrap();
    assert_eq!(store.list().await.unwrap()[0].to, 93.);
    h.session.submit("love it, thanks", None).await.unwrap();
    h.session.lessons().await.unwrap();
    assert_eq!(store.list().await.unwrap()[0].reaction, Some(Reaction::Liked));
    let two = match_harness(vec![Some(("Collision", 94.)), None], |o| o.playbook = Some(store.clone()));
    two.session.start().await.unwrap();
    two.session.submit("make this metallic percussion hit sound like the reference", None).await.unwrap();
    assert!(two.record.calls.borrow()[0].contains("Collision, brighter"));
    assert!(two.session.forget_lesson(&listed[0].id).await.unwrap());
    h.session.close().await.unwrap();
    two.session.close().await.unwrap();
});

use kumi_runtime::core::{
    evolve::Knob,
    goal::{create_goal_store, GoalBudget, GoalPhase, GoalRun, GoalStore, GOAL_BUDGET},
};
struct SearchRig {
    calls: RefCell<Vec<(Vec<GenerationTrial>, Option<GenerationOptions>)>>,
    cleanup: RefCell<Vec<String>>,
    scores: Vec<f64>,
    screens: bool,
    silent: bool,
    structural: bool,
    ms: u64,
    values_score: bool,
    settled: RefCell<Vec<f64>>,
}
impl SearchRig {
    fn new(scores: Vec<f64>) -> Self {
        Self {
            calls: RefCell::new(vec![]),
            cleanup: RefCell::new(vec![]),
            scores,
            screens: false,
            silent: false,
            structural: false,
            ms: 1,
            values_score: false,
            settled: RefCell::new(vec![]),
        }
    }
    fn knob() -> Knob {
        Knob { r#ref: "knob:1".into(), device: "1:Operator".into(), name: "Filter Freq".into(), min: 0., max: 1., step: None, value: 0.2 }
    }
}
#[async_trait(?Send)]
impl GoalRig for SearchRig {
    fn slots(&self) -> Vec<GoalSlotInfo> {
        vec![GoalSlotInfo { name: "Candidate".into(), label: "Drift".into(), chain: "Operator".into(), knobs: vec![Self::knob()] }]
    }
    fn screens(&self) -> bool {
        self.screens
    }
    async fn add(&self, _: &AuditionCandidate, _: Signal) -> Result<Result<GoalSlotInfo, String>, RuntimeError> {
        Ok(Ok(self.slots()[0].clone()))
    }
    async fn generation(
        &self,
        trials: &[GenerationTrial],
        signal: Signal,
        options: Option<GenerationOptions>,
    ) -> Result<Generation, RuntimeError> {
        let at = self.calls.borrow().len();
        self.calls.borrow_mut().push((trials.to_vec(), options));
        tokio::select! {_=signal.cancelled()=>return Err(RuntimeError::Aborted),_=delay(self.ms)=>{}}
        let mut result = Generation::default();
        for trial in trials {
            if !self.silent {
                let score = if self.values_score {
                    trial.values[0] * 100.
                } else {
                    *self.scores.get(at).or_else(|| self.scores.last()).unwrap_or(&50.)
                };
                result.scores.insert(trial.slot.clone(), score);
            }
            result.gaps.insert(trial.slot.clone(), vec!["attack too slow".into()]);
            if self.structural {
                result.structural.insert(trial.slot.clone(), StructuralMove { gap: "sub missing".into(), r#move: "add a sub".into() });
            }
        }
        Ok(result)
    }
    async fn keep_best(&self, slot: &str, _: &[Knob], _: &[f64], signal: Signal) -> Result<String, RuntimeError> {
        assert!(!signal.is_cancelled());
        self.cleanup.borrow_mut().push(format!("keep:{slot}"));
        Ok("Kumi · Goal best".into())
    }
    async fn settle(&self, _: &str, _: &[Knob], values: &[f64], signal: Signal) -> Result<Option<String>, RuntimeError> {
        assert!(!signal.is_cancelled());
        *self.settled.borrow_mut() = values.to_vec();
        self.cleanup.borrow_mut().push("settle".into());
        Ok(None)
    }
    async fn tidy(&self, top: &[String], signal: Signal) -> Result<Vec<String>, RuntimeError> {
        assert!(!signal.is_cancelled());
        self.cleanup.borrow_mut().push(format!("tidy:{}", top.join(",")));
        Ok(vec![])
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        self.cleanup.borrow_mut().push("close".into());
        Ok(vec![])
    }
}
fn goal_harness(rig: Rc<SearchRig>, reference: bool, config: impl FnOnce(&mut SessionOptions)) -> Harness {
    let session = Rc::new(RefCell::new(None::<Session>));
    let active = session.clone();
    let run: Run = Rc::new(move |_, _, emit| {
        let session = active.borrow().clone().unwrap();
        async move {
            if reference {
                let mut heard = audition(50., "Drift");
                heard.request.as_mut().unwrap().candidates[0].track = "Candidate".into();
                heard.request.as_mut().unwrap().candidates[0].clip = Some("first".into());
                session.watch(WatchEvent::Audition(heard));
            }
            emit(KernelEvent::Text { text: "Tried: a brighter filter".into() })?;
            Ok(TurnResult { usage: Some(Usage { input_tokens: 10., output_tokens: 5., ..Default::default() }), ..complete() })
        }
        .boxed_local()
    });
    let h = harness(Some(run), |o| {
        o.goal_budget = Some(GoalBudget { ms: 2000, ..GOAL_BUDGET });
        config(o)
    });
    *session.borrow_mut() = Some(h.session.clone());
    *h.record.goal_rig.borrow_mut() = Some(rig);
    h
}
async fn generation(session: &Session, n: u32) {
    for _ in 0..500 {
        if session.goal_status().is_some_and(|s| s.generation >= n) {
            return;
        }
        delay(1).await;
    }
    panic!("goal did not reach generation {n}: {:?}", session.goal_status());
}
local_test!(goal_reaches_target_cleans_in_order_keeps_best_and_lesson, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_goal_store(dir.path().join("goals"));
    let playbook = kumi_runtime::core::playbook::create_playbook_store(dir.path().join("lessons.json"));
    let rig = Rc::new(SearchRig::new(vec![60., 75., 96.]));
    let h = goal_harness(rig.clone(), true, |o| {
        o.goals = Some(store.clone());
        o.playbook = Some(playbook.clone());
    });
    h.session.start().await.unwrap();
    h.session.goal(Some("make my pad sound like the reference")).await.unwrap();
    let status = h.session.goal_status().unwrap();
    assert_eq!(status.state, GoalPhase::Done);
    assert_eq!(status.best.as_ref().unwrap().score, 96.);
    assert_eq!(status.generation, 3);
    assert_eq!(status.why.as_deref(), Some("reached 96%"));
    assert_eq!(*rig.cleanup.borrow(), ["close", "tidy:Candidate", "keep:Candidate"]);
    assert_eq!(h.record.calls.borrow().len(), 1);
    assert_eq!(h.session.lessons().await.unwrap().len(), 1);
    h.session.close().await.unwrap();
    assert_eq!(store.load("unsaved").await.unwrap().unwrap().status, GoalRun::Done);
});
local_test!(goal_without_reference_finishes_regular_request_and_missing_rig_explains, {
    let rig = Rc::new(SearchRig::new(vec![]));
    let h = goal_harness(rig.clone(), false, |_| {});
    h.session.start().await.unwrap();
    h.session.goal(Some("build a complex rack")).await.unwrap();
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Done);
    assert!(h.notice("regular request"));
    assert!(rig.calls.borrow().is_empty());
    h.session.close().await.unwrap();
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    h.session.goal(Some("build a rack")).await.unwrap();
    assert!(h.error("Context refresh failed"));
    assert!(h.events.borrow().iter().any(|e| matches!(e, SessionEvent::Error { kind: Some(FailureKind::Request), .. })));
    h.session.close().await.unwrap();
});
local_test!(goal_pauses_persists_resumes_without_setup_and_stop_finishes, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_goal_store(dir.path().join("goals"));
    let mut raw = SearchRig::new(vec![55.]);
    raw.ms = 25;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |o| {
        o.goals = Some(store.clone());
        o.idle_timeout_ms = Some(10);
    });
    h.session.start().await.unwrap();
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(Some("match my reference")).await });
    generation(&h.session, 2).await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Paused);
    assert!(!h.error("without progress"));
    assert!(!rig.cleanup.borrow().iter().any(|s| s.starts_with("tidy:")));
    h.session.close().await.unwrap();
    let kept = store.load("unsaved").await.unwrap().unwrap();
    assert_eq!(kept.status, GoalRun::Paused);
    let mut raw = SearchRig::new(vec![65.]);
    raw.ms = 10;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |o| o.goals = Some(store.clone()));
    h.session.start().await.unwrap();
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(None).await });
    generation(&h.session, kept.generation + 1).await;
    assert!(h.record.calls.borrow().is_empty());
    assert!(h.session.stop_goal().await.unwrap());
    running.await.unwrap().unwrap();
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Done);
    assert_eq!(h.session.goal_status().unwrap().why.as_deref(), Some("stopped"));
    assert!(rig.cleanup.borrow().iter().any(|s| s.starts_with("tidy:")));
    h.session.close().await.unwrap();
    // close waits for the goal's save only 25 ms here; a busy runner writes slower.
    let mut status = None;
    for _ in 0..1000 {
        status = store.load("unsaved").await.unwrap().map(|kept| kept.status);
        if status == Some(GoalRun::Done) {
            break;
        }
        delay(2).await;
    }
    assert_eq!(status, Some(GoalRun::Done));
    assert!(!h.session.stop_goal().await.unwrap());
});
local_test!(a_goal_stopped_while_its_search_is_set_up_is_done_and_the_next_goals_escape_still_pauses_it, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_goal_store(dir.path().join("goals"));
    let mut raw = SearchRig::new(vec![55.]);
    raw.ms = 25;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |o| o.goals = Some(store.clone()));
    h.record.goal_holds.set(true);
    h.session.start().await.unwrap();
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(Some("match my reference")).await });
    for _ in 0..500 {
        if !h.record.goal_requests.borrow().is_empty() {
            break;
        }
        delay(1).await;
    }
    assert!(h.session.stop_goal().await.unwrap());
    let _ = running.await.unwrap();
    let stopped = h.session.goal_status().unwrap();
    // The next goal is its own: Esc pauses it, as Esc does, and its candidates stay.
    h.record.goal_holds.set(false);
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(Some("match my reference again")).await });
    generation(&h.session, 2).await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Paused);
    assert!(!rig.cleanup.borrow().iter().any(|s| s.starts_with("tidy:")));
    // And the stopped one said so.
    assert_eq!((stopped.state, stopped.why.as_deref()), (GoalPhase::Done, Some("stopped")));
    h.session.close().await.unwrap();
});
local_test!(a_goal_stopped_during_its_setup_turn_stops_that_turn_and_leaves_an_older_paused_goal_paused, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_goal_store(dir.path().join("goals"));
    let mut raw = SearchRig::new(vec![55.]);
    raw.ms = 25;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |o| o.goals = Some(store.clone()));
    h.session.start().await.unwrap();
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(Some("match my reference")).await });
    generation(&h.session, 2).await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    h.session.close().await.unwrap();
    assert_eq!(store.load("unsaved").await.unwrap().unwrap().status, GoalRun::Paused);
    // A new goal whose setup turn is still being written when the producer says /goal stop.
    let writing: Run = Rc::new(|_, signal, _| {
        async move {
            signal.cancelled().await;
            Err(RuntimeError::Aborted)
        }
        .boxed_local()
    });
    let h = harness(Some(writing), |o| o.goals = Some(store.clone()));
    *h.record.goal_rig.borrow_mut() = Some(rig.clone());
    h.session.start().await.unwrap();
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(Some("a new goal")).await });
    for _ in 0..500 {
        if !h.record.calls.borrow().is_empty() {
            break;
        }
        delay(1).await;
    }
    assert!(h.session.stop_goal().await.unwrap());
    tokio::time::timeout(Duration::from_secs(5), running).await.expect("the setup turn stopped").unwrap().ok();
    assert_eq!(h.session.goal_status().unwrap().why.as_deref(), Some("stopped"));
    h.session.close().await.unwrap();
    assert_eq!(store.load("unsaved").await.unwrap().unwrap().status, GoalRun::Paused, "the older goal is still there to pick up");
});
local_test!(a_paused_goal_stays_with_its_set, {
    let dir = tempfile::tempdir().unwrap();
    let store = create_goal_store(dir.path().join("goals"));
    let mut raw = SearchRig::new(vec![55.]);
    raw.ms = 25;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |o| o.goals = Some(store.clone()));
    h.observation.borrow_mut().project = Some(ProjectRef { id: "set-a".into(), name: "Set A".into() });
    h.session.start().await.unwrap();
    let s = h.session.clone();
    let running = tokio::task::spawn_local(async move { s.goal(Some("match my reference")).await });
    generation(&h.session, 2).await;
    h.session.cancel().await.unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Paused);
    let rendered = rig.calls.borrow().len();
    // The producer opens Set B, and Kumi sees it on the next turn.
    h.observation.borrow_mut().project = Some(ProjectRef { id: "set-b".into(), name: "Set B".into() });
    h.session.submit("what's in this Set?", None).await.unwrap();
    // Set A's goal doesn't carry on in Set B, and stopping there doesn't touch it.
    let opened = h.record.goal_requests.borrow().len();
    h.session.goal(None).await.unwrap();
    assert_eq!(h.record.goal_requests.borrow().len(), opened, "no search set up in Set B");
    assert_eq!(rig.calls.borrow().len(), rendered, "nothing rendered in Set B");
    assert!(!h.session.stop_goal().await.unwrap());
    h.session.close().await.unwrap();
    assert!(store.load("set-b").await.unwrap().is_none());
    assert_eq!(store.load("set-a").await.unwrap().unwrap().status, GoalRun::Paused);
});
local_test!(goal_silent_renders_pause_and_structural_gap_prompts_leap, {
    let mut raw = SearchRig::new(vec![]);
    raw.silent = true;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |_| {});
    h.session.start().await.unwrap();
    h.session.goal(Some("match the reference")).await.unwrap();
    assert_eq!(rig.calls.borrow().len(), 2);
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Paused);
    assert!(h.session.goal_status().unwrap().why.unwrap().contains("nothing came through"));
    h.session.close().await.unwrap();
    let mut raw = SearchRig::new(vec![55., 60., 96.]);
    raw.structural = true;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |_| {});
    h.session.start().await.unwrap();
    h.session.goal(Some("match the reference")).await.unwrap();
    assert_eq!(h.record.calls.borrow().len(), 2);
    assert!(h.record.calls.borrow()[1].contains("Make a structural leap"));
    assert!(h.record.calls.borrow()[1].contains("sub missing"));
    assert!(h.record.calls.borrow()[1].contains("add a sub"));
    assert_eq!(h.record.goal_requests.borrow().len(), 2);
    assert!(h.record.goal_requests.borrow()[1].candidates.iter().all(|c| c.clip.as_deref() == Some("first")));
    assert_eq!(h.session.goal_status().unwrap().idea.as_deref(), Some("a brighter filter"));
    assert_eq!(rig.cleanup.borrow().iter().filter(|s| s.as_str() == "close").count(), 2);
    h.session.close().await.unwrap();
});
local_test!(goal_full_length_checks_control_reported_scores_and_time_cap, {
    let mut raw = SearchRig::new(vec![50., 55., 60., 65., 96.]);
    raw.screens = true;
    let rig = Rc::new(raw);
    let h = goal_harness(rig.clone(), true, |_| {});
    h.session.start().await.unwrap();
    h.session.goal(Some("match the reference")).await.unwrap();
    assert_eq!(h.session.goal_status().unwrap().best.unwrap().score, 96.);
    assert_eq!(h.session.goal_status().unwrap().rendered, 5);
    assert_eq!(rig.calls.borrow()[4].1.as_ref().unwrap().screen, Some(false));
    h.session.close().await.unwrap();
    let rig = Rc::new(SearchRig::new(vec![50.]));
    let h = goal_harness(rig.clone(), true, |o| o.goal_budget = Some(GoalBudget { ms: 1, ..GOAL_BUDGET }));
    h.session.start().await.unwrap();
    h.session.goal(Some("match the reference")).await.unwrap();
    assert_eq!(h.session.goal_status().unwrap().state, GoalPhase::Done);
    assert_eq!(h.session.goal_status().unwrap().why.as_deref(), Some("the safety cap on its time"));
    h.session.close().await.unwrap();
});
local_test!(match_polish_confirms_improvement_at_full_length_and_restores_if_not_better, {
    // Exercise the full search budget independently of the host timer resolution.
    tokio::time::pause();
    for improves in [false, true] {
        let mut raw = SearchRig::new(vec![50.]);
        raw.values_score = improves;
        let rig = Rc::new(raw);
        let h = match_harness(vec![Some(("Drift", 50.)), None], |o| {
            o.match_budget = Some(MatchBudget { rounds: 0, polish_ms: Some(50), ..MATCH_BUDGET });
            let random = Rc::new(RefCell::new(kumi_runtime::core::evolve::seeded(123)));
            o.goal_random = Some(Rc::new(move || (random.borrow_mut())()));
        });
        *h.record.goal_rig.borrow_mut() = Some(rig.clone());
        h.session.start().await.unwrap();
        h.session.submit("match this reference", None).await.unwrap();
        assert_eq!(h.record.calls.borrow().len(), 2);
        assert_eq!(rig.cleanup.borrow().as_slice(), ["close", "settle"]);
        if improves {
            assert!(h.record.calls.borrow()[1].contains("kept on the track"), "{}", h.record.calls.borrow()[1]);
            assert!(rig.settled.borrow()[0] > 0.2);
        } else {
            assert!(h.record.calls.borrow()[1].contains("none beat it at full length"));
            assert_eq!(*rig.settled.borrow(), [0.2]);
        }
        h.session.close().await.unwrap();
    }
});

struct NamesOnly;
#[async_trait(?Send)]
impl KernelTool for NamesOnly {
    fn name(&self) -> &str {
        "find_sounds"
    }
    fn description(&self) -> &str {
        "Names only"
    }
    fn input_schema(&self) -> JsonObject {
        JsonObject::new()
    }
    async fn execute(&self, _: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        panic!("replaced by library")
    }
}
local_test!(library_tools_preferences_status_and_forgetting_follow_session_lifecycle, {
    use kumi_runtime::library::{create_library, sources::SourceOptions, LibraryOptions};
    let temp = tempfile::tempdir().unwrap();
    let samples = temp.path().join("samples");
    std::fs::create_dir(&samples).unwrap();
    kumi_runtime::library::library_logs(&temp.path().to_string_lossy())
        .sounds
        .append(&[json!({"path":samples.join("Kick.wav"),"size":100,"mtime":1,"seconds":1,"features":1})])
        .await
        .unwrap();
    std::fs::write(
        temp.path().join("taste.json"),
        serde_json::to_vec(&json!({"sets":2,"at":1,"lines":[{"id":"tempo","line":"Tempo: usually 124–126 BPM"}]})).unwrap(),
    )
    .unwrap();
    let library = create_library(LibraryOptions {
        dir: temp.path().to_string_lossy().into(),
        folders: Some(vec![samples.to_string_lossy().into()]),
        find_sets: Some(false),
        fork: Some(false),
        sources: Some(SourceOptions { home: Some(temp.path().to_string_lossy().into()), ..Default::default() }),
        ..Default::default()
    });
    let h = harness(None, |o| o.library = Some(library.clone()));
    h.observation.borrow_mut().tools = vec![Rc::new(NamesOnly), Rc::new(Plan { received: Rc::new(RefCell::new(vec![])) })];
    assert!(h.session.has_library() && h.session.has_taste() && h.session.has_forget_taste());
    assert_eq!(h.session.library(), Some(library.status()));
    h.session.start().await.unwrap();
    let tools = h.record.created.borrow()[0].tools.clone();
    assert_eq!(
        tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
        ["make_changes", "find_sounds", "find_presets", "my_sets", "live_manual"]
    );
    assert!(h.record.created.borrow()[0].instructions.contains("Tempo: usually 124–126 BPM"));
    let sounds = tools.iter().find(|t| t.name() == "find_sounds").unwrap();
    let result = sounds.execute(json!({"like":"the kick in this Set"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(result.is_error);
    assert_eq!(*h.record.audio_resolved.borrow(), ["the kick in this Set"]);
    assert_eq!(h.session.taste().await.unwrap()[0].id, "tempo");
    assert!(h.session.forget_taste("tempo").await.unwrap());
    assert!(!h.session.forget_taste("tempo").await.unwrap());
    assert!(h.session.taste().await.unwrap().is_empty());
    // The existing kernel keeps its cached preferences until it is rebuilt.
    h.session.refresh().await.unwrap();
    assert_eq!(h.record.created.borrow().len(), 1);
    h.session.reconfigure().await.unwrap();
    h.session.refresh().await.unwrap();
    assert!(!h.record.created.borrow().last().unwrap().instructions.contains("Tempo: usually"));
    h.events.borrow_mut().clear();
    library.pause();
    delay(450).await;
    assert!(h.events.borrow().iter().any(|e| matches!(e, SessionEvent::Library(LibraryEvent { status }) if *status == library.status())));
    h.session.close().await.unwrap();
    let count = h.events.borrow().len();
    let independent = Rc::new(Cell::new(0));
    let seen = independent.clone();
    let unlisten = library.on_status(Rc::new(move |_| seen.set(seen.get() + 1)));
    library.resume();
    delay(450).await;
    assert_eq!(h.events.borrow().len(), count);
    assert!(independent.get() > 0, "the session does not own the library lifetime");
    unlisten();
    library.close().await;
    let without = harness(None, |_| {});
    assert!(!without.session.has_library() && !without.session.has_taste() && !without.session.has_forget_taste());
    assert!(without.session.library().is_none());
    without.session.close().await.unwrap();
});

local_test!(each_turn_logs_where_its_time_went, {
    use kumi_runtime::core::timing;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("timings.jsonl");
    let fail = Rc::new(Cell::new(false));
    let failing = fail.clone();
    let h = harness(
        Some(Rc::new(move |_, _, _| {
            let fail = failing.get();
            async move {
                drop(timing::model_call("openai/gpt-test"));
                timing::tool(40);
                timing::live_request();
                timing::background(async { timing::live_request() }).await;
                timing::sent(1000);
                if fail {
                    return Err(RuntimeError::plain("model went away"));
                }
                Ok(TurnResult {
                    usage: Some(Usage { input_tokens: 900., cache_read_tokens: 600., output_tokens: 20., ..Default::default() }),
                    ..complete()
                })
            }
            .boxed_local()
        })),
        |o| o.timings = Some(file.to_string_lossy().into_owned()),
    );
    h.session.start().await.unwrap();
    h.session.submit("make it louder", None).await.unwrap();
    fail.set(true);
    let _ = h.session.submit("and again", None).await;
    settle().await;
    let lines: Vec<Value> = std::fs::read_to_string(&file).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(lines.len(), 2, "one line a turn; starting the session isn't a turn");
    let turn = &lines[0];
    assert_eq!(
        [&turn["stop"], &turn["model"], &turn["modelCalls"], &turn["tools"], &turn["toolMs"], &turn["liveRequests"], &turn["sentBytes"]],
        [&json!("completed"), &json!("openai/gpt-test"), &json!(1), &json!(1), &json!(40), &json!(1), &json!(1000)]
    );
    assert_eq!([&turn["inputTokens"], &turn["cachedTokens"], &turn["outputTokens"]], [&json!(900), &json!(600), &json!(20)]);
    assert_eq!(lines[1]["stop"], "error");
    h.session.close().await.unwrap();
});

local_test!(a_side_question_asked_during_an_answer_isnt_part_of_its_timing, {
    use kumi_runtime::core::timing;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("timings.jsonl");
    let gate = Rc::new(Notify::new());
    let hold = gate.clone();
    let h = harness(
        Some(Rc::new(move |_, _, _| {
            let gate = hold.clone();
            async move {
                drop(timing::model_call("openai/gpt-test"));
                timing::sent(1000);
                gate.notified().await;
                Ok(complete())
            }
            .boxed_local()
        })),
        |o| o.timings = Some(file.to_string_lossy().into_owned()),
    );
    h.session.start().await.unwrap();
    let answer = spawned(&h.session, "make it louder");
    delay(10).await;
    assert_eq!(h.session.aside("what did you change?", Rc::new(|_| {}), None).await.unwrap(), "aside");
    gate.notify_one();
    answer.await.unwrap().unwrap();
    let line: Value = serde_json::from_str(std::fs::read_to_string(&file).unwrap().trim()).unwrap();
    assert_eq!([&line["modelCalls"], &line["sentBytes"], &line["model"]], [&json!(1), &json!(1000), &json!("openai/gpt-test")]);
    h.session.close().await.unwrap();
});
local_test!(a_turn_brings_in_what_an_older_kumi_changed_beside_it_and_never_waits_for_it, {
    use kumi_runtime::core::{file_sync::FileSync, store_client::StoreClient, store_import::JsonFiles};
    let dir = tempfile::tempdir().unwrap();
    let files = JsonFiles {
        memory: dir.path().join("memory.json"),
        projects: dir.path().join("projects"),
        techniques: dir.path().join("techniques.json"),
        playbook: dir.path().join("playbook.json"),
        gaps: dir.path().join("gaps.jsonl"),
    };
    std::fs::write(&files.memory, r#"{"version":1,"notes":[{"id":"p1","text":"Likes short reverbs","at":100}]}"#).unwrap();
    let db = dir.path().join("kumi.db");
    let (client, _) = StoreClient::open(db.clone(), files.clone(), 1).await.unwrap();
    let h = harness(None, |options| options.files = Some(FileSync::new(client, files.clone(), &[])));
    h.session.start().await.unwrap();
    // An older Kumi open beside this one keeps a note, while another Kumi holds the database's write lock.
    std::fs::write(
        &files.memory,
        r#"{"version":1,"notes":[{"id":"p1","text":"Likes short reverbs","at":100},{"id":"p2","text":"Works at 140","at":200}]}"#,
    )
    .unwrap();
    let other = kumi_store::Connection::open(&db).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let began = std::time::Instant::now();
    h.session.submit("hello", None).await.unwrap();
    assert!(began.elapsed() < Duration::from_secs(2), "the turn didn't wait for the look: {:?}", began.elapsed());
    assert!(!h.notice("Brought in"));
    other.execute_batch("ROLLBACK").unwrap();
    let said = tokio::time::timeout(Duration::from_secs(10), async {
        while !h.notice("Brought in changes made with the older Kumi: 1 note added.") {
            delay(10).await;
        }
    })
    .await;
    assert!(said.is_ok(), "{:?}", h.events.borrow());
});
local_test!(a_set_kumi_asked_for_keeps_the_request_and_one_opened_by_hand_stops_it_without_offering_it_again, {
    // #188: "start a new Set and build X" was cancelled by the switch it asked for, and called "Live closed".
    let h = harness(Some(hang_once()), |_| {});
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "make a new set and add a track");
    settle().await;
    h.connection(ConnectionState::Disconnected, Some(DisconnectCause::AskedSet));
    settle().await;
    assert!(h.notice("Live is opening the Set; Kumi carries on once it's open."));
    assert!(!running.is_finished(), "the request carries on");
    h.connection(ConnectionState::Connected, None);
    settle().await;
    assert!(h.notice("Live has the other Set open."));
    assert!(!h.events.borrow().iter().any(|e| matches!(e, SessionEvent::Resend { .. })));
    running.abort();
    h.session.close().await.unwrap();

    // Opened by hand while Kumi worked: the request stops (nothing lands in the other Set), and isn't
    // offered again there.
    let h = harness(Some(hang_once()), |_| {});
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "widen the pads");
    settle().await;
    h.connection(ConnectionState::Disconnected, Some(DisconnectCause::Set));
    running.await.unwrap().unwrap();
    assert!(h.notice("Live is opening another Set, so Kumi stopped what it was doing"));
    h.connection(ConnectionState::Connected, None);
    settle().await;
    assert!(h.notice("Live has the other Set open."));
    assert!(!h.events.borrow().iter().any(|e| matches!(e, SessionEvent::Resend { .. })));
    h.session.close().await.unwrap();
});
local_test!(live_closing_mid_request_says_it_may_have_crashed_and_warns_before_sending_it_again, {
    // #195: Live crashed under a script Kumi ran, and Kumi said "Live closed", then offered the request
    // again as if nothing happened.
    let h = harness(Some(hang_once()), |_| {});
    h.session.start().await.unwrap();
    let running = spawned(&h.session, "record the mix");
    settle().await;
    h.connection(ConnectionState::Disconnected, Some(DisconnectCause::Live));
    running.await.unwrap().unwrap();
    assert!(h.notice("Live closed while Kumi was working in it. If it crashed, your unsaved changes may be lost"));
    h.connection(ConnectionState::Connected, None);
    settle().await;
    assert!(h.notice("Your last request stopped when Live closed, which it may have caused: check the Set before you send it again"));
    assert!(h.events.borrow().iter().any(|e| matches!(e, SessionEvent::Resend { text } if text == "record the mix")));
    h.session.close().await.unwrap();
});
/// Kumi's undo tool, as the integration offers it: it answers with the change it undid.
struct UndoTool;
#[async_trait(?Send)]
impl KernelTool for UndoTool {
    fn name(&self) -> &str {
        "undo_change"
    }
    fn description(&self) -> &str {
        "Undo one of your changes."
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object"}).as_object().unwrap().clone()
    }
    async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(ToolResult { text: json!({"undone":"Tempo 120 → 124 BPM","change":input["change"]}).to_string(), ..Default::default() })
    }
}
local_test!(the_producers_reactions_are_kept_in_kumis_database_and_kumis_own_steps_are_not, {
    use kumi_runtime::core::{store_client::StoreClient, store_import::JsonFiles};
    use kumi_store::observations::{self, Kind};
    let dir = tempfile::tempdir().unwrap();
    let files = JsonFiles {
        memory: dir.path().join("memory.json"),
        projects: dir.path().join("projects"),
        techniques: dir.path().join("techniques.json"),
        playbook: dir.path().join("playbook.json"),
        gaps: dir.path().join("gaps.jsonl"),
    };
    let (client, _) = StoreClient::open(dir.path().join("kumi.db"), files, 1).await.unwrap();
    let tools = Rc::new(RefCell::new(Vec::<Rc<dyn KernelTool>>::new()));
    let session = Rc::new(RefCell::new(None::<Session>));
    let turns = Rc::new(Cell::new(0));
    let (use_tools, active, count) = (tools.clone(), session.clone(), turns.clone());
    let run: Run = Rc::new(move |_, signal, _| {
        let tools = use_tools.borrow().clone();
        let session = active.borrow().clone().unwrap();
        count.set(count.get() + 1);
        let turn = count.get();
        async move {
            let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap().clone();
            let input = |value: Value| value.as_object().unwrap().clone();
            if turn == 1 {
                // Kumi takes its own change back in the turn that made it: not the producer's.
                session.watch(WatchEvent::Change(change("c1", "applied", 1)));
                tool("undo_change").execute(input(json!({"change":"c1"})), signal.clone()).await?;
                session.watch(WatchEvent::Change(change("c2", "applied", 2)));
            } else {
                let noted = tool("reaction").execute(input(json!({"quote":"too wet","lean":"less","change":"c2"})), signal.clone()).await?;
                assert_eq!((noted.is_error, noted.reply.as_deref()), (false, Some("")), "noted quietly: without final, the turn goes on");
                tool("undo_change").execute(input(json!({"change":"c2"})), signal).await?;
            }
            Ok(complete())
        }
        .boxed_local()
    });
    let h = harness(Some(run), |o| {
        o.store = Some(client.clone());
        let factory = o.kernel_factory.clone();
        o.kernel_factory = Rc::new(move |options| {
            *tools.borrow_mut() = options.tools.clone();
            factory(options)
        });
    });
    *session.borrow_mut() = Some(h.session.clone());
    h.observation.borrow_mut().tools.push(Rc::new(UndoTool));
    h.session.start().await.unwrap();
    h.session.submit("make it wetter", None).await.unwrap();
    h.session.submit("too wet, undo that", None).await.unwrap();
    h.session.picked(Picked::Answer { question: "Which pad?".into(), options: vec!["Warm".into(), "Glassy".into()], index: 0 });
    client.store().write_wait(|_| Ok(())).unwrap();
    let mut rows = client.store().read(|c| observations::recent(c, 10)).unwrap();
    rows.reverse();
    assert_eq!(rows.iter().map(|row| row.kind).collect::<Vec<_>>(), [Kind::Words, Kind::Undo, Kind::Pick]);
    assert_eq!(rows[0].facts, json!({"quote":"too wet","lean":"less"}));
    assert_eq!(rows[0].subject["change"], "c2");
    assert_eq!((rows[1].subject["change"].clone(), rows[1].facts["by"].clone(), rows[1].weight), (json!("c2"), json!("asked"), Some(-0.5)));
    assert_eq!(rows[2].context["requests"], json!(["make it wetter", "too wet, undo that"]));
    h.session.close().await.unwrap();
});
