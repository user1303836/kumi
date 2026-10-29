//! The screen on a session on a thread of its own: a key still draws at once while the session's thread is busy, and
//! what's asked of the session once until it answers (words for the answer under way, ctrl-b's move) is asked once.
#[path = "../support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::{
    contracts::*,
    protocol::SessionHandle,
    threaded::{Build, Built, Threaded},
};
use kumi_types::{contracts::ChangeRecord, errors::RuntimeError};
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

/// How long the session's answer keeps its thread busy, yielding nothing.
const WORK: Duration = Duration::from_millis(200);
/// How soon a key typed meanwhile draws: well inside the session's work, so a screen that waited on it couldn't pass.
/// (A frame's own budget is 16 ms; a test machine under load can be later than that without anything waiting.)
const DRAWN_WITHIN: Duration = Duration::from_millis(100);

/// A session whose answer keeps its thread busy for `WORK`: `busy` says while it does.
struct Busy {
    busy: Arc<AtomicBool>,
}

#[async_trait(?Send)]
impl SessionController for Busy {
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

#[async_trait(?Send)]
impl SessionController for Steers {
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
    fn has_steer(&self) -> bool {
        true
    }
    fn steer(&self, _: &str) -> bool {
        self.steers.fetch_add(1, Ordering::SeqCst);
        self.said.get()
    }
    fn has_jobs(&self) -> bool {
        true
    }
    fn background(&self) -> Result<u32, String> {
        self.moves.fetch_add(1, Ordering::SeqCst);
        Ok(7)
    }
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
