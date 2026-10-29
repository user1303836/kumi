//! The screen on a session on a thread of its own: a key still draws at once while the session's thread is busy.
#[path = "../support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::{
    contracts::*,
    threaded::{Build, Threaded},
};
use kumi_types::{contracts::ChangeRecord, errors::RuntimeError};
use std::{
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
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
            let build: Build =
                Box::new(move |_| async move { Ok(Rc::new(Busy { busy: working }) as Rc<dyn SessionController>) }.boxed_local());
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
