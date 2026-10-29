//! The screen on a session on a thread of its own: a key still draws at once while the session's thread is busy, and
//! what's asked of the session once until it answers (words for the answer under way, ctrl-b's move) is asked once.
#[path = "../support/absent.rs"]
mod absent;
#[path = "../support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::{
    contracts::*,
    jobs::{JobDetail, JobStatus},
    protocol::{cloud_provider, op, provider_name, Capabilities, ModelsView, Op, Reply, SessionHandle},
    threaded::{Build, Built, Threaded},
};
use kumi_ai::providers::ProviderId;
use kumi_types::{
    contracts::{ChangeRecord, LibraryStatus},
    errors::RuntimeError,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use support::*;

/// The handle's reads every session here gives alike: none of what they're about, and nothing told to them (the core's
/// thread sends every request it's given). Each session answers its own requests in its `send`.
macro_rules! alike {
    () => {
        fn can_show(&self, _: BodyKind) -> bool {
            false
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
        fn tell(&self, op: Op) {
            let _ = self.send(op).now_or_never();
        }
    };
}

/// A session's own requests, as each of these sessions answers them; anything else as a session with nothing for it.
macro_rules! answered {
    ($session:expr, $op:expr, { $($arm:tt)* }) => {
        Ok(match $op {
            Op::Start(_) => Reply::Start($session.start().await?),
            Op::Submit(op::Submit { input, pinned, .. }) => Reply::Submit($session.submit(&input, pinned).await?),
            Op::Refresh(_) => Reply::Refresh($session.refresh().await?),
            Op::NewConversation(_) => Reply::NewConversation($session.new_conversation().await?),
            Op::Cancel(_) => Reply::Cancel($session.cancel().await?),
            Op::Close(_) => Reply::Close($session.close().await?),
            Op::Undo(op::Undo { id }) => Reply::Undo($session.undo(id.as_deref()).await?),
            $($arm)*
            op => return absent::answer(op),
        })
    };
}

/// How long the session's answer keeps its thread busy, yielding nothing.
const WORK: Duration = Duration::from_millis(200);
/// How soon a key typed meanwhile draws: well inside the session's work, so a screen that waited on it couldn't pass.
/// (A frame's own budget is 16 ms; a test machine under load can be later than that without anything waiting.)
const DRAWN_WITHIN: Duration = Duration::from_millis(100);

/// A session whose answer keeps its thread busy for `WORK`: `busy` says while it does.
struct Busy {
    busy: Arc<AtomicBool>,
}

/// What the session does, asked through its handle below.
impl Busy {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.busy.store(true, Ordering::SeqCst);
        std::thread::sleep(WORK);
        self.busy.store(false, Ordering::SeqCst);
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus { state: TurnState::Idle, connection: ConnectionState::Connected, turns: 0, max_turns: None, observation: None }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
}

#[async_trait(?Send)]
impl SessionHandle for Busy {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn status(&self) -> SessionStatus {
        Busy::status(self)
    }
    fn jobs(&self) -> Vec<JobStatus> {
        vec![]
    }
    fn job(&self, _: u32) -> Option<JobDetail> {
        None
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        answered!(self, op, {})
    }
    alike!();
}

#[tokio::test(flavor = "current_thread")]
async fn a_key_draws_at_once_while_the_sessions_thread_is_busy() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let busy = Arc::new(AtomicBool::new(false));
            let working = busy.clone();
            let build: Build = Box::new(move |_| async move { Ok(Built::new(Rc::new(Busy { busy: working }))) }.boxed_local());
            let h = Harness::on(120, 36, Threaded::spawn(build).await.unwrap(), |_| {});
            h.start().await;
            h.input.write("make the bass darker\r");
            let asked = Instant::now();
            while !busy.load(Ordering::SeqCst) {
                assert!(asked.elapsed() < Duration::from_secs(5), "the session never started its work");
                delay(1).await;
            }
            // The session's thread is in its work now: a key typed is drawn all the same.
            let typed = Instant::now();
            h.input.write("zq");
            while !has(&h.screen(), "zq") {
                assert!(typed.elapsed() < WORK, "the key waited on the session: {:?}", typed.elapsed());
                delay(1).await;
            }
            let drawn = typed.elapsed();
            assert!(busy.load(Ordering::SeqCst), "the key drew while the session's thread was still busy");
            assert!(drawn < DRAWN_WITHIN, "{drawn:?}");
            h.close().await;
        })
        .await;
}

fn idle() -> SessionStatus {
    SessionStatus { state: TurnState::Idle, connection: ConnectionState::Connected, turns: 0, max_turns: None, observation: None }
}

/// Until `done`, within five seconds.
async fn until(what: &str, done: impl Fn() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < Duration::from_secs(5), "{what} never happened");
        delay(1).await;
    }
}

/// A session whose answer waits until it's told to go on (`go`), then says a burst of words all at once and works on
/// until Kumi closes. It takes what the producer says meanwhile only once the burst is said (`steers` counts each ask),
/// and moves a step to the background each time it's asked (`moves` counts them).
struct Steers {
    emit: Rc<dyn Fn(SessionEvent)>,
    status: RefCell<SessionStatus>,
    go: Arc<AtomicBool>,
    steers: Arc<AtomicUsize>,
    moves: Arc<AtomicUsize>,
    said: Cell<bool>,
    closed: Cell<bool>,
}

/// What the session does, asked through its handle below.
impl Steers {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.status.borrow_mut().state = TurnState::Running;
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        while !self.go.load(Ordering::SeqCst) && !self.closed.get() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // All at once: every word is on its way to the screen before what it asks meanwhile is read here.
        for _ in 0..50 {
            (self.emit)(SessionEvent::Kernel(KernelEvent::Text { text: "w".into() }));
        }
        self.said.set(true);
        while !self.closed.get() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closed.set(true);
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        self.status.borrow().clone()
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    fn steer(&self, _: &str) -> bool {
        self.steers.fetch_add(1, Ordering::SeqCst);
        self.said.get()
    }
    fn background(&self) -> Result<u32, String> {
        self.moves.fetch_add(1, Ordering::SeqCst);
        Ok(7)
    }
}

#[async_trait(?Send)]
impl SessionHandle for Steers {
    fn capabilities(&self) -> Capabilities {
        Capabilities { steer: true, jobs: true, ..Capabilities::default() }
    }
    fn status(&self) -> SessionStatus {
        Steers::status(self)
    }
    fn jobs(&self) -> Vec<JobStatus> {
        vec![]
    }
    fn job(&self, _: u32) -> Option<JobDetail> {
        None
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        answered!(self, op, { Op::Steer(op::Steer { text }) => Reply::Steer(self.steer(&text)), Op::Background(_) => Reply::Background(self.background()), })
    }
    alike!();
}

/// The screen on a [`Steers`] on a thread of its own, and its counts: what it was asked to take, and to move.
async fn steering(go: &Arc<AtomicBool>) -> (Harness, Rc<Threaded>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (steers, moves) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (going, asked, moved) = (go.clone(), steers.clone(), moves.clone());
    let build: Build = Box::new(move |emit| {
        async move {
            Ok(Built::new(Rc::new(Steers {
                emit,
                status: RefCell::new(idle()),
                go: going,
                steers: asked,
                moves: moved,
                said: Cell::new(false),
                closed: Cell::new(false),
            })))
        }
        .boxed_local()
    });
    let threaded = Threaded::spawn(build).await.unwrap();
    let h = Harness::on(120, 36, threaded.clone(), |_| {});
    h.start().await;
    (h, threaded, steers, moves)
}

/// What the producer says while the answer works is asked of it once until it answers. A session on a thread of its own
/// answers after the events already on their way, and each of those would otherwise ask again.
#[tokio::test(flavor = "current_thread")]
async fn words_for_the_answer_under_way_are_asked_once_until_it_answers() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let go = Arc::new(AtomicBool::new(false));
            let (h, threaded, steers, _) = steering(&go).await;
            h.input.write("make the bass darker\r");
            until("the answer", || threaded.status().state == TurnState::Running).await;
            // Said while it works: asked at once, and not taken yet.
            h.input.write("and the kick\r");
            until("the first ask", || steers.load(Ordering::SeqCst) == 1).await;
            // A burst of the answer's words: the first asks again, and the rest come before its answer.
            go.store(true, Ordering::SeqCst);
            until("the second ask", || steers.load(Ordering::SeqCst) >= 2).await;
            delay(100).await;
            assert_eq!(steers.load(Ordering::SeqCst), 2, "asked again before the answer came");
            h.close().await;
        })
        .await;
}

/// Ctrl-b twice before its answer moves the step once.
#[tokio::test(flavor = "current_thread")]
async fn ctrl_b_twice_before_its_answer_moves_once() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let go = Arc::new(AtomicBool::new(false));
            let (h, threaded, _, moves) = steering(&go).await;
            h.input.write("make the bass darker\r");
            until("the answer", || threaded.status().state == TurnState::Running).await;
            h.input.write("\x02\x02");
            until("the move", || moves.load(Ordering::SeqCst) >= 1).await;
            delay(100).await;
            assert_eq!(moves.load(Ordering::SeqCst), 1);
            h.has("Moved to the background as job 7");
            h.close().await;
        })
        .await;
}

/// A session that, as it takes a request, first says how an operation before it ended (Running, then Idle), then
/// answers. On a thread of its own, an earlier operation's end can reach the screen after the request went.
struct Late {
    emit: Rc<dyn Fn(SessionEvent)>,
    status: RefCell<SessionStatus>,
}

impl Late {
    fn answer(&self) {
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        (self.emit)(SessionEvent::State { state: TurnState::Idle });
        self.status.borrow_mut().state = TurnState::Running;
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        (self.emit)(SessionEvent::Kernel(KernelEvent::Text { text: "Lowered the bass by 3 dB.".into() }));
        (self.emit)(SessionEvent::TurnComplete { result: TurnResult { stop_reason: StopReason::Completed, usage: None }, elapsed_ms: 1 });
        self.status.borrow_mut().state = TurnState::Idle;
        (self.emit)(SessionEvent::State { state: TurnState::Idle });
    }
}

/// What the session does, asked through its handle below.
impl Late {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.answer();
        Ok(())
    }
    async fn goal(&self, _: Option<&str>) -> Result<(), RuntimeError> {
        self.answer();
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        self.status.borrow().clone()
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
}

#[async_trait(?Send)]
impl SessionHandle for Late {
    fn capabilities(&self) -> Capabilities {
        Capabilities { goal: true, ..Capabilities::default() }
    }
    fn status(&self) -> SessionStatus {
        Late::status(self)
    }
    fn jobs(&self) -> Vec<JobStatus> {
        vec![]
    }
    fn job(&self, _: u32) -> Option<JobDetail> {
        None
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        answered!(self, op, { Op::Goal(op::Goal { text }) => Reply::Goal(self.goal(text.as_deref()).await?), })
    }
    alike!();
}

/// An answer, or a goal's, shows whole when an earlier operation's end reaches the screen after it was asked for.
#[tokio::test(flavor = "current_thread")]
async fn an_answer_shows_whole_when_an_earlier_operations_end_reaches_the_screen_after_the_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for asked in ["make the bass darker\r", "/goal make the bass darker\r"] {
                let build: Build =
                    Box::new(|emit| async move { Ok(Built::new(Rc::new(Late { emit, status: RefCell::new(idle()) }))) }.boxed_local());
                let h = Harness::on(120, 36, Threaded::spawn(build).await.unwrap(), |_| {});
                h.start().await;
                h.input.write(asked);
                until("the answer", || has(&h.screen(), "Lowered the bass by 3 dB.")).await;
                assert!(!has(&h.screen(), "stopped"), "{asked}:\n{}", h.screen().join("\n"));
                h.close().await;
            }
        })
        .await;
}

/// A session whose answer keeps its thread busy a moment before it says the answer has begun, takes into the answer
/// what's said meanwhile once it has, and turns away a second request while it answers. `submits` and `steers` count
/// the two.
struct Slow {
    emit: Rc<dyn Fn(SessionEvent)>,
    status: RefCell<SessionStatus>,
    at_work: Arc<AtomicBool>,
    submits: Arc<AtomicUsize>,
    steers: Arc<AtomicUsize>,
    closed: Cell<bool>,
}

/// What the session does, asked through its handle below.
impl Slow {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        if self.status.borrow().state == TurnState::Running {
            return Err(RuntimeError::plain("Session is busy; cancel first"));
        }
        self.submits.fetch_add(1, Ordering::SeqCst);
        self.at_work.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        self.status.borrow_mut().state = TurnState::Running;
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        while !self.closed.get() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closed.set(true);
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        self.status.borrow().clone()
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    fn steer(&self, _: &str) -> bool {
        self.steers.fetch_add(1, Ordering::SeqCst);
        true
    }
}

#[async_trait(?Send)]
impl SessionHandle for Slow {
    fn capabilities(&self) -> Capabilities {
        Capabilities { steer: true, ..Capabilities::default() }
    }
    fn status(&self) -> SessionStatus {
        Slow::status(self)
    }
    fn jobs(&self) -> Vec<JobStatus> {
        vec![]
    }
    fn job(&self, _: u32) -> Option<JobDetail> {
        None
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        answered!(self, op, { Op::Steer(op::Steer { text }) => Reply::Steer(self.steer(&text)), })
    }
    alike!();
}

/// A second message while the first's answer hasn't reached the screen yet (the session's thread busy in it) goes to
/// that answer, rather than as a second request the session turns away, its words lost.
#[tokio::test(flavor = "current_thread")]
async fn a_second_message_before_the_answer_has_begun_goes_to_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (at_work, submits, steers) =
                (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
            let (working, answered, asked) = (at_work.clone(), submits.clone(), steers.clone());
            let build: Build = Box::new(move |emit| {
                async move {
                    Ok(Built::new(Rc::new(Slow {
                        emit,
                        status: RefCell::new(idle()),
                        at_work: working,
                        submits: answered,
                        steers: asked,
                        closed: Cell::new(false),
                    })))
                }
                .boxed_local()
            });
            let h = Harness::on(120, 36, Threaded::spawn(build).await.unwrap(), |_| {});
            h.start().await;
            h.input.write("make the bass darker\r");
            until("the session at work", || at_work.load(Ordering::SeqCst)).await;
            h.input.write("and the kick\r");
            until("the second message asked of the answer", || steers.load(Ordering::SeqCst) >= 1).await;
            assert_eq!(submits.load(Ordering::SeqCst), 1);
            assert!(!has(&h.screen(), "Session is busy"), "{}", h.screen().join("\n"));
            h.close().await;
        })
        .await;
}

/// A session with a background job done, whose details it reads on its own thread.
struct Jobbed {
    emit: Rc<dyn Fn(SessionEvent)>,
}

fn done_job() -> serde_json::Value {
    serde_json::json!({"id": 3, "kind": "worker", "title": "Hear the mix", "state": "done", "whenDone": "tell", "startedAt": 1,
                       "endedAt": 2, "steps": 0, "sources": 0})
}

/// What the session does, asked through its handle below.
impl Jobbed {
    async fn start(&self) -> Result<(), RuntimeError> {
        let _ = &self.emit;
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        idle()
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    fn jobs(&self) -> Vec<kumi_agent::core::jobs::JobStatus> {
        vec![serde_json::from_value(done_job()).unwrap()]
    }
    fn job(&self, id: u32) -> Option<kumi_agent::core::jobs::JobDetail> {
        (id == 3).then(|| {
            serde_json::from_value(serde_json::json!({"status": done_job(), "asked": "Hear the mix in mono", "steps": [],
                                                      "report": "It holds up in mono."}))
            .unwrap()
        })
    }
}

#[async_trait(?Send)]
impl SessionHandle for Jobbed {
    fn capabilities(&self) -> Capabilities {
        Capabilities { jobs: true, ..Capabilities::default() }
    }
    fn status(&self) -> SessionStatus {
        Jobbed::status(self)
    }
    fn jobs(&self) -> Vec<JobStatus> {
        Jobbed::jobs(self)
    }
    fn job(&self, id: u32) -> Option<JobDetail> {
        Jobbed::job(self, id)
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        answered!(self, op, {})
    }
    alike!();
}

/// A job's panel, opened while Kumi is idle, shows the job's details as they come, with no key to draw them.
#[tokio::test(flavor = "current_thread")]
async fn a_jobs_details_show_as_they_come_without_a_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let build: Build = Box::new(|emit| async move { Ok(Built::new(Rc::new(Jobbed { emit }))) }.boxed_local());
            let h = Harness::on(120, 36, Threaded::spawn(build).await.unwrap(), |_| {});
            h.start().await;
            h.input.write("/jobs\r");
            until("the jobs", || has(&h.screen(), "Background jobs")).await;
            h.input.write("\r");
            until("the job's details", || has(&h.screen(), "Asked: Hear the mix in mono")).await;
            h.close().await;
        })
        .await;
}

/// A session whose start takes a moment (its thread free meanwhile), which turns away a request before it has started
/// and while it starts, as the session does, and answers one once it has. `submits` counts the answers.
struct Starting {
    emit: Rc<dyn Fn(SessionEvent)>,
    status: RefCell<SessionStatus>,
    starting: Cell<bool>,
    started: Cell<bool>,
    submits: Arc<AtomicUsize>,
}

/// What the session does, asked through its handle below.
impl Starting {
    async fn start(&self) -> Result<(), RuntimeError> {
        self.starting.set(true);
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.started.set(true);
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        (self.emit)(SessionEvent::State { state: TurnState::Idle });
        self.starting.set(false);
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        if self.starting.get() {
            return Err(RuntimeError::plain("Session is busy; cancel first"));
        }
        if !self.started.get() {
            return Err(RuntimeError::plain("Session is not started"));
        }
        self.submits.fetch_add(1, Ordering::SeqCst);
        self.status.borrow_mut().state = TurnState::Running;
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        (self.emit)(SessionEvent::Kernel(KernelEvent::Text { text: "Lowered the bass by 3 dB.".into() }));
        (self.emit)(SessionEvent::TurnComplete { result: TurnResult { stop_reason: StopReason::Completed, usage: None }, elapsed_ms: 1 });
        self.status.borrow_mut().state = TurnState::Idle;
        (self.emit)(SessionEvent::State { state: TurnState::Idle });
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        self.status.borrow().clone()
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
}

#[async_trait(?Send)]
impl SessionHandle for Starting {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn status(&self) -> SessionStatus {
        Starting::status(self)
    }
    fn jobs(&self) -> Vec<JobStatus> {
        vec![]
    }
    fn job(&self, _: u32) -> Option<JobDetail> {
        None
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        answered!(self, op, {})
    }
    alike!();
}

/// A message sent while the session starts, typed then or ahead of the screen, waits for the session and is answered,
/// rather than going as a request the session turns away.
#[tokio::test(flavor = "current_thread")]
async fn a_message_sent_while_the_session_starts_waits_for_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for ahead in [false, true] {
                let submits = Arc::new(AtomicUsize::new(0));
                let answered = submits.clone();
                let build: Build = Box::new(move |emit| {
                    async move {
                        Ok(Built::new(Rc::new(Starting {
                            emit,
                            status: RefCell::new(idle()),
                            starting: Cell::new(false),
                            started: Cell::new(false),
                            submits: answered,
                        })))
                    }
                    .boxed_local()
                });
                let h = Harness::on(120, 36, Threaded::spawn(build).await.unwrap(), |options| {
                    if ahead {
                        options.typeahead = b"make the bass darker\r".to_vec();
                    }
                });
                h.start().await;
                if !ahead {
                    h.input.write("make the bass darker\r");
                }
                until("the answer", || has(&h.screen(), "Lowered the bass by 3 dB.")).await;
                assert_eq!(submits.load(Ordering::SeqCst), 1, "typed ahead: {ahead}");
                assert!(!has(&h.screen(), "Session is"), "typed ahead: {ahead}\n{}", h.screen().join("\n"));
                h.close().await;
            }
        })
        .await;
}
