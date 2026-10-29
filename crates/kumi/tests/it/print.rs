//! `kumi -p`'s front end: one request, its answer's words on stdout (or each event as a JSON line with `--json`), what
//! went wrong on stderr, and an exit code that says how it ended, over a session in this thread or one of its own.
#[path = "../support/absent.rs"]
mod absent;
use async_trait::async_trait;
use futures::FutureExt;
use kumi::{
    print::{Print, PrintOptions, ANSWERED, FAILED, STEP_LIMIT, STOPPED},
    terminal::Terminal,
    tui::tty::TtyOutput,
};
use kumi_agent::core::{
    contracts::*,
    jobs::{JobDetail, JobStatus},
    protocol::{cloud_provider, op, provider_name, tell_now, Capabilities, ModelsView, Op, Reply, SessionHandle},
    threaded::{Build, Built, Threaded},
};
use kumi_ai::providers::ProviderId;
use kumi_types::contracts::{ChangeRecord, LibraryStatus};
use kumi_types::errors::{FailureKind, RuntimeError};
use std::{cell::RefCell, rc::Rc};

const SECRET: &str = "sk-test-private-0000000000";

#[derive(Default)]
struct Out(RefCell<String>);

impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, s: &str) {
        self.0.borrow_mut().push_str(s);
    }
}

/// A session that answers as it's told: the events of its answer, then (when it waits) the end ctrl+c brings, unless
/// it's stubborn and goes on whatever it's told.
#[derive(Default)]
struct Fake {
    emit: RefCell<Option<Rc<dyn Fn(SessionEvent)>>>,
    answer: Vec<SessionEvent>,
    waits: bool,
    stubborn: bool,
    cancelled: tokio::sync::Notify,
    calls: RefCell<Vec<String>>,
    /// Itself, for a request told it to run on after `tell` returns, as InProcess's does.
    this: RefCell<std::rc::Weak<Fake>>,
}

/// `fake`, shared, knowing itself.
fn made(fake: Fake) -> Rc<Fake> {
    let fake = Rc::new(fake);
    *fake.this.borrow_mut() = Rc::downgrade(&fake);
    fake
}

impl Fake {
    fn call(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }
    fn emit(&self, event: SessionEvent) {
        let emit = self.emit.borrow().clone();
        if let Some(emit) = emit {
            emit(event);
        }
    }
}

/// What the session does, asked through its handle below.
impl Fake {
    async fn start(&self) -> Result<(), RuntimeError> {
        self.call("start");
        Ok(())
    }
    async fn submit(&self, input: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.call(format!("submit:{input}"));
        for event in self.answer.clone() {
            self.emit(event);
        }
        if self.waits {
            self.cancelled.notified().await;
            self.emit(ended(StopReason::Cancelled));
        }
        Ok(())
    }
    async fn instant(&self, text: &str) -> Result<Option<InstantOutcome>, RuntimeError> {
        self.call(format!("instant:{text}"));
        Ok((text == "tempo 124").then(|| InstantOutcome { text: "Tempo 120 → 124 BPM".into(), is_error: false }))
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        self.call("cancel");
        if !self.stubborn {
            self.cancelled.notify_one();
        }
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.call("close");
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus { state: TurnState::Idle, connection: ConnectionState::Connected, turns: 0, max_turns: None, observation: None }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
}

/// `kumi -p`'s session: its own requests through its methods above, and anything else as a session without it.
#[async_trait(?Send)]
impl SessionHandle for Fake {
    fn capabilities(&self) -> Capabilities {
        Capabilities { instant: true, ..Capabilities::default() }
    }
    fn can_show(&self, _: BodyKind) -> bool {
        false
    }
    fn status(&self) -> SessionStatus {
        Fake::status(self)
    }
    fn answering_fallback(&self) -> Option<String> {
        None
    }
    fn library(&self) -> Option<LibraryStatus> {
        None
    }
    fn open_set(&self) -> Option<String> {
        None
    }
    fn jobs(&self) -> Vec<JobStatus> {
        vec![]
    }
    fn job(&self, _: u32) -> Option<JobDetail> {
        None
    }
    fn can_background(&self) -> bool {
        false
    }
    fn models(&self) -> Option<ModelsView> {
        None
    }
    fn provider_name(&self, id: &str) -> String {
        provider_name(None, id)
    }
    fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        cloud_provider(None, id)
    }
    fn context_window(&self, _: &str) -> Option<f64> {
        None
    }
    fn listen(&self, _: Rc<dyn Fn(SessionEvent)>) {}
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        Ok(match op {
            Op::Start(_) => Reply::Start(self.start().await?),
            Op::Submit(op::Submit { input, pinned, .. }) => Reply::Submit(self.submit(&input, pinned).await?),
            Op::Instant(op::Instant { text }) => Reply::Instant(self.instant(&text).await?),
            Op::Refresh(_) => Reply::Refresh(self.refresh().await?),
            Op::NewConversation(_) => Reply::NewConversation(self.new_conversation().await?),
            Op::Cancel(_) => Reply::Cancel(self.cancel().await?),
            Op::Close(_) => Reply::Close(self.close().await?),
            Op::Undo(op::Undo { id }) => Reply::Undo(self.undo(id.as_deref()).await?),
            op => return absent::answer(op),
        })
    }
    fn tell(&self, op: Op) {
        let Some(this) = self.this.borrow().upgrade() else { return };
        tell_now(
            async move {
                let _ = this.send(op).await;
            }
            .boxed_local(),
        );
    }
}

fn text(words: &str) -> SessionEvent {
    SessionEvent::Kernel(KernelEvent::Text { text: words.into() })
}

fn ended(stop: StopReason) -> SessionEvent {
    SessionEvent::TurnComplete { result: TurnResult { stop_reason: stop, usage: None }, elapsed_ms: 1_200 }
}

fn state(state: TurnState) -> SessionEvent {
    SessionEvent::State { state }
}

/// `kumi -p <request>` over `fake`: the front end, and what it wrote to stdout and stderr.
fn print(fake: &Rc<Fake>, request: &str, json: bool) -> (Print, Rc<Out>, Rc<Out>) {
    let (out, err) = (Rc::new(Out::default()), Rc::new(Out::default()));
    let print = Print::new(PrintOptions {
        session: fake.clone(),
        request: request.into(),
        json,
        out: out.clone(),
        err: err.clone(),
        secrets: vec![SECRET.into()],
        instant: true,
        startup_notice: None,
    });
    let front = print.clone();
    *fake.emit.borrow_mut() = Some(Rc::new(move |event| front.handle_event(event)));
    (print, out, err)
}

fn answered(words: &[&str], stop: StopReason) -> Rc<Fake> {
    let mut answer = vec![state(TurnState::Running)];
    answer.extend(words.iter().map(|words| text(words)));
    answer.extend([ended(stop), state(TurnState::Idle)]);
    made(Fake { answer, ..Default::default() })
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_goes_to_stdout_as_it_comes_and_ends_with_0() {
    let fake = answered(&["A low shelf ", "lifts the lows, ", "and the key ", SECRET, " stays out."], StopReason::Completed);
    let (print, out, err) = print(&fake, "What does a low shelf do?", false);
    assert_eq!(print.run().await, ANSWERED);
    assert_eq!(*out.0.borrow(), "A low shelf lifts the lows, and the key [redacted] stays out.\n");
    assert!(err.0.borrow().is_empty(), "{}", err.0.borrow());
    assert_eq!(*fake.calls.borrow(), ["start", "submit:What does a low shelf do?", "close"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_fixed_phrase_changes_the_daw_at_once_without_the_model() {
    let fake = answered(&["never asked"], StopReason::Completed);
    let (print, out, _) = print(&fake, "tempo 124", false);
    assert_eq!(print.run().await, ANSWERED);
    assert_eq!(*out.0.borrow(), "Tempo 120 → 124 BPM\n");
    assert_eq!(*fake.calls.borrow(), ["start", "instant:tempo 124", "close"]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_cut_short_at_its_step_limit_ends_with_3() {
    let fake = answered(&["Two of the four tracks are done"], StopReason::MaxSteps);
    let (print, out, err) = print(&fake, "set up the drum bus", false);
    assert_eq!(print.run().await, STEP_LIMIT);
    assert_eq!(*out.0.borrow(), "Two of the four tracks are done\n");
    assert!(err.0.borrow().contains("step limit"), "{}", err.0.borrow());
}

#[tokio::test(flavor = "current_thread")]
async fn what_went_wrong_goes_to_stderr_and_ends_with_1() {
    let failed = SessionEvent::Error {
        message: format!("Not signed in to Anthropic (key {SECRET}): run kumi login anthropic"),
        kind: Some(FailureKind::Auth),
        provider: Some("anthropic".into()),
    };
    let fake = made(Fake { answer: vec![state(TurnState::Running), failed, state(TurnState::Idle)], ..Default::default() });
    let (print, out, err) = print(&fake, "hello", false);
    assert_eq!(print.run().await, FAILED);
    assert!(out.0.borrow().is_empty(), "{}", out.0.borrow());
    let err = err.0.borrow();
    assert!(err.contains("Not signed in to Anthropic") && !err.contains(SECRET), "{err}");
}

#[tokio::test(flavor = "current_thread")]
async fn with_json_each_event_is_a_line_of_json() {
    // A key the model writes across two pieces of words is a key all the same.
    let (first, rest) = SECRET.split_at(12);
    let fake = answered(&["A low shelf ", first, rest, " here"], StopReason::Completed);
    let (print, out, _) = print(&fake, "What does a low shelf do?", true);
    assert_eq!(print.run().await, ANSWERED);
    let out = out.0.borrow();
    assert!(!out.contains(first) && !out.contains(rest), "{out}");
    let events: Vec<SessionEvent> = out.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let words: String = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Kernel(KernelEvent::Text { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(words, "A low shelf [redacted] here");
    let others: Vec<&SessionEvent> =
        events.iter().filter(|event| !matches!(event, SessionEvent::Kernel(KernelEvent::Text { .. }))).collect();
    assert_eq!(others, [&state(TurnState::Running), &ended(StopReason::Completed), &state(TurnState::Idle)]);
}

#[tokio::test(flavor = "current_thread")]
async fn ctrl_c_stops_the_answer_and_ends_with_130() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fake = made(Fake { answer: vec![state(TurnState::Running), text("Listening to ")], waits: true, ..Default::default() });
            let (print, out, _) = print(&fake, "listen to the mix", false);
            let running = tokio::task::spawn_local(print.run());
            tokio::task::yield_now().await;
            print.interrupt();
            assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(5), running).await.unwrap().unwrap(), STOPPED);
            assert_eq!(*out.0.borrow(), "Listening to \n");
            assert_eq!(*fake.calls.borrow(), ["start", "submit:listen to the mix", "cancel", "close"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_ctrl_c_or_being_told_to_close_ends_it_while_the_answer_goes_on() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for told_to_close in [false, true] {
                let fake = made(Fake {
                    answer: vec![state(TurnState::Running), text("Rendering ")],
                    waits: true,
                    stubborn: true,
                    ..Default::default()
                });
                let (print, _, _) = print(&fake, "render the mix", false);
                let running = tokio::task::spawn_local(print.run());
                tokio::task::yield_now().await;
                print.interrupt();
                tokio::task::yield_now().await;
                assert!(!running.is_finished(), "the first ctrl+c only asks the answer to stop");
                if told_to_close {
                    assert_eq!(print.close().await, STOPPED);
                } else {
                    print.interrupt();
                }
                assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(5), running).await.unwrap().unwrap(), STOPPED);
                assert_eq!(*fake.calls.borrow(), ["start", "submit:render the mix", "cancel", "close"], "told to close: {told_to_close}");
            }
        })
        .await;
}

/// Over a session on a thread of its own, the answer's end (turn-complete, sent just before its Submit returns) is heard
/// before the Submit's reply, so the exit code says it came in full. Many times: a reply that overtook the events sent
/// before it would lose only sometimes.
#[tokio::test(flavor = "current_thread")]
async fn over_a_session_on_its_own_thread_an_answer_that_came_in_full_ends_with_0() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for _ in 0..200 {
                let build: Build = Box::new(|emit| {
                    async move {
                        let fake = answered(&["done"], StopReason::Completed);
                        *fake.emit.borrow_mut() = Some(emit);
                        Ok(Built::new(fake))
                    }
                    .boxed_local()
                });
                let session = Threaded::spawn(build).await.unwrap();
                let (out, err) = (Rc::new(Out::default()), Rc::new(Out::default()));
                let print = Print::new(PrintOptions {
                    session: session.clone(),
                    request: "make it louder".into(),
                    json: false,
                    out: out.clone(),
                    err: err.clone(),
                    secrets: vec![],
                    instant: false,
                    startup_notice: None,
                });
                assert_eq!(print.run().await, ANSWERED, "{}", err.0.borrow());
                assert_eq!(*out.0.borrow(), "done\n");
                assert!(session.close_thread().await);
            }
        })
        .await;
}
