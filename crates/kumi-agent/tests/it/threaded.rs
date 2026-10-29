//! A session on a thread of its own: a reply comes after every event sent before it, the reads are sent before what
//! changed them, a session that can't start (or panics as it starts) says why, a panic later ends the front end with
//! what it said, closing has its bound, the thread ends with its handle, and a stop before its aside still stops it.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::{
    contracts::*,
    jobs::JobDetail,
    protocol::{op, SessionHandle},
    threaded::{session_panicked, Build, Threaded},
};
use kumi_common::abort::Signal;
use kumi_types::{contracts::ChangeRecord, errors::RuntimeError};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

/// How the fake session behaves.
#[derive(Clone, Default)]
struct Ways {
    /// Its refresh panics: the DAW's read went wrong in a way nothing caught.
    refresh_panics: bool,
    /// Its close never finishes.
    close_hangs: bool,
    /// Set as it's dropped: its thread is ending.
    gone: Option<Arc<AtomicBool>>,
}

/// A session that answers a message with a turn's events, built on the session's thread.
struct Answers {
    emit: Rc<dyn Fn(SessionEvent)>,
    status: RefCell<SessionStatus>,
    ways: Ways,
}

impl Drop for Answers {
    fn drop(&mut self) {
        if let Some(gone) = &self.ways.gone {
            gone.store(true, Ordering::SeqCst);
        }
    }
}

fn idle() -> SessionStatus {
    SessionStatus { state: TurnState::Idle, connection: ConnectionState::Connected, turns: 0, max_turns: None, observation: None }
}

#[async_trait(?Send)]
impl SessionController for Answers {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, input: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.status.borrow_mut().state = TurnState::Running;
        (self.emit)(SessionEvent::State { state: TurnState::Running });
        (self.emit)(SessionEvent::Kernel(KernelEvent::Text { text: input.into() }));
        {
            let mut status = self.status.borrow_mut();
            status.state = TurnState::Idle;
            status.turns += 1;
        }
        (self.emit)(SessionEvent::TurnComplete { result: TurnResult { stop_reason: StopReason::Completed, usage: None }, elapsed_ms: 1 });
        (self.emit)(SessionEvent::State { state: TurnState::Idle });
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        if self.ways.refresh_panics {
            panic!("the DAW's read went wrong");
        }
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        if self.ways.close_hangs {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        self.status.borrow().clone()
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    fn has_aside(&self) -> bool {
        true
    }
    /// Until it's stopped.
    async fn aside(&self, _: &str, _: OnText, signal: Option<Signal>) -> Result<String, RuntimeError> {
        if let Some(signal) = signal {
            signal.cancelled().await;
        }
        Ok("stopped".into())
    }
    fn has_jobs(&self) -> bool {
        true
    }
    fn job(&self, id: u32) -> Option<JobDetail> {
        (id == 3).then(|| {
            serde_json::from_value(serde_json::json!({
                "status": {"id": 3, "kind": "worker", "title": "Hear the mix", "state": "running", "whenDone": "tell",
                           "startedAt": 1, "steps": 0, "sources": 0},
                "asked": "Hear the mix",
                "steps": [],
            }))
            .unwrap()
        })
    }
}

fn answers(ways: Ways) -> Build {
    Box::new(move |emit| {
        async move { Ok(Rc::new(Answers { emit, status: RefCell::new(idle()), ways }) as Rc<dyn SessionController>) }.boxed_local()
    })
}

async fn spawned(ways: Ways) -> Rc<dyn SessionHandle> {
    Threaded::spawn(answers(ways)).await.unwrap()
}

/// Panics on the session's thread go where the app's hook sends them; the rest as before.
fn hooked() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !session_panicked(info) {
            previous(info);
        }
    }));
}

fn local<F: std::future::Future<Output = ()>>(test: F) -> impl std::future::Future<Output = ()> {
    async move { tokio::task::LocalSet::new().run_until(test).await }
}

#[tokio::test(flavor = "current_thread")]
async fn a_reply_comes_after_every_event_sent_before_it() {
    local(async {
        // Many times: two channels, one for replies and one for events, would lose this race only sometimes.
        for _ in 0..200 {
            let session = spawned(Ways::default()).await;
            let seen = Rc::new(RefCell::new(vec![]));
            session.listen({
                let seen = seen.clone();
                Rc::new(move |event| seen.borrow_mut().push(event))
            });
            session.op(op::Submit { input: "louder".into(), pinned: None, attachments: vec![], references: vec![] }).await.unwrap();
            let seen = seen.borrow();
            assert!(seen.iter().any(|event| matches!(event, SessionEvent::TurnComplete { .. })), "{seen:?}");
            assert!(matches!(seen.last(), Some(SessionEvent::State { state: TurnState::Idle })), "{seen:?}");
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_reads_are_sent_before_what_changed_them() {
    local(async {
        let session = spawned(Ways::default()).await;
        // At the event that said the answer began, the status read says so too, as it would in this thread.
        let read = Rc::new(RefCell::new(vec![]));
        let weak = Rc::downgrade(&session);
        session.listen({
            let read = read.clone();
            Rc::new(move |event| {
                if let (SessionEvent::State { state }, Some(session)) = (event, weak.upgrade()) {
                    read.borrow_mut().push((state, session.status().state));
                }
            })
        });
        session.op(op::Submit { input: "louder".into(), pinned: None, attachments: vec![], references: vec![] }).await.unwrap();
        assert_eq!(*read.borrow(), [(TurnState::Running, TurnState::Running), (TurnState::Idle, TurnState::Idle)]);
        // And once the reply came, what the request changed.
        assert_eq!(session.status().turns, 1);
        assert!(session.capabilities().aside && session.capabilities().jobs);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_session_that_cant_be_built_says_why() {
    local(async {
        let build: Build = Box::new(|_| async { Err(RuntimeError::plain("Kumi's database couldn't open")) }.boxed_local());
        let error = Threaded::spawn(build).await.err().expect("no session");
        assert_eq!(error.message(), "Kumi's database couldn't open");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_session_that_panics_as_it_is_built_says_so() {
    hooked();
    local(async {
        let build: Build = Box::new(|_| async { panic!("no room for the store") }.boxed_local());
        let error = Threaded::spawn(build).await.err().expect("no session");
        assert!(error.message().starts_with("no room for the store"), "{}", error.message());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_panic_on_the_sessions_thread_ends_the_front_end_with_what_it_said() {
    hooked();
    local(async {
        let threaded = Threaded::spawn(answers(Ways { refresh_panics: true, ..Default::default() })).await.unwrap();
        let ended = Rc::new(RefCell::new(None));
        threaded.on_fatal({
            let ended = ended.clone();
            Rc::new(move |why| *ended.borrow_mut() = Some(why))
        });
        let session: Rc<dyn SessionHandle> = threaded;
        session.tell(op::Refresh {}.into());
        for _ in 0..500 {
            if ended.borrow().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let ended = ended.borrow().clone().expect("the front end heard it ended");
        assert!(ended.starts_with("the DAW's read went wrong"), "{ended}");
        // What's asked after says so.
        let error = session.op(op::Refresh {}).await.err().expect("ended");
        assert!(error.message().starts_with("the DAW's read went wrong"), "{}", error.message());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn closing_waits_no_longer_than_its_bound() {
    local(async {
        tokio::time::pause();
        let session = spawned(Ways { close_hangs: true, ..Default::default() }).await;
        // The screen's close gives a session six seconds, whatever it's doing.
        let closed = tokio::time::timeout(Duration::from_secs(6), session.op(op::Close {})).await;
        assert!(closed.is_err(), "a close that never finishes is left at its bound");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_sessions_thread_ends_when_its_handle_goes() {
    local(async {
        let gone = Arc::new(AtomicBool::new(false));
        let session = spawned(Ways { gone: Some(gone.clone()), ..Default::default() }).await;
        session.op(op::Close {}).await.unwrap();
        drop(session);
        for _ in 0..500 {
            if gone.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("the session's thread didn't end with its handle");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_that_comes_before_its_aside_still_stops_it_across_the_thread() {
    local(async {
        let session = spawned(Ways::default()).await;
        session.tell(op::StopAside { id: 7 }.into());
        let answer = tokio::time::timeout(Duration::from_secs(5), session.op(op::Aside { id: 7, question: "why?".into() })).await;
        assert_eq!(answer.expect("stopped").unwrap(), "stopped");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_jobs_details_come_for_the_one_watched() {
    local(async {
        let session = spawned(Ways::default()).await;
        // Asked for first, it's watched: its details come once its thread has read them.
        assert!(session.job(3).is_none());
        for _ in 0..500 {
            if let Some(detail) = session.job(3) {
                assert_eq!(detail.asked, "Hear the mix");
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("the watched job's details never came");
    })
    .await;
}
