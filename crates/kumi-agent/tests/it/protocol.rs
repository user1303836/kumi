//! The session's protocol in this process: each request reaches the controller's method it was and answers in its own
//! type, an aside's words come as events with its id and stopping it stops it, and a request nobody waits for runs at
//! once as far as it goes.
use async_trait::async_trait;
use kumi_agent::core::{
    contracts::*,
    protocol::{op, Capabilities, InProcess, SessionHandle},
};
use kumi_common::abort::Signal;
use kumi_types::contracts::ChangeRecord;
use kumi_types::errors::RuntimeError;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<String>>,
}

impl Fake {
    fn call(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

#[async_trait(?Send)]
impl SessionController for Fake {
    async fn start(&self) -> Result<(), RuntimeError> {
        self.call("start");
        Ok(())
    }
    async fn submit(&self, input: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.call(format!("submit:{input}"));
        Ok(())
    }
    async fn submit_with(&self, input: &str, _: Option<PinnedNode>, attachments: Vec<Attachment>) -> Result<(), RuntimeError> {
        self.call(format!("submit_with:{input}:{}", attachments.len()));
        Ok(())
    }
    fn has_aside(&self) -> bool {
        true
    }
    /// Two pieces of words, then it waits until it's stopped.
    async fn aside(&self, question: &str, on_text: OnText, signal: Option<Signal>) -> Result<String, RuntimeError> {
        self.call(format!("aside:{question}"));
        on_text("It's ");
        on_text("the low shelf.");
        signal.expect("an aside can be stopped").cancelled().await;
        Err(RuntimeError::plain("stopped"))
    }
    /// What waits on something before it's done.
    async fn refresh(&self) -> Result<(), RuntimeError> {
        self.call("refresh:started");
        tokio::task::yield_now().await;
        self.call("refresh:done");
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
        SessionStatus { state: TurnState::Idle, connection: ConnectionState::Connected, turns: 2, max_turns: None, observation: None }
    }
    async fn undo(&self, id: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        self.call(format!("undo:{}", id.unwrap_or("latest")));
        Ok(None)
    }
    fn picked(&self, pick: Picked) {
        if let Picked::Technique { name, keep } = pick {
            self.call(format!("picked:{name}:{keep}"));
        }
    }
    fn stop_job(&self, id: u32) -> bool {
        self.call(format!("stop_job:{id}"));
        id == 3
    }
}

fn attachment() -> Attachment {
    Attachment { path: "/Users/me/reference.png".into(), name: "reference.png".into(), media_type: "image/png".into(), bytes: 48_210 }
}

#[tokio::test(flavor = "current_thread")]
async fn each_request_reaches_the_method_it_was_and_answers_in_its_own_type() {
    let fake = Rc::new(Fake::default());
    let session: Rc<dyn SessionHandle> = InProcess::new(fake.clone());
    session.op(op::Start {}).await.unwrap();
    // A message with files goes as it did: with them, and without them otherwise.
    session.op(op::Submit { input: "louder".into(), pinned: None, attachments: vec![] }).await.unwrap();
    session.op(op::Submit { input: "like this".into(), pinned: None, attachments: vec![attachment()] }).await.unwrap();
    let undone: Option<ChangeRecord> = session.op(op::Undo { id: Some("c2".into()) }).await.unwrap();
    assert_eq!(undone, None);
    let stopping: bool = session.op(op::StopJob { id: 3 }).await.unwrap();
    assert!(stopping);
    assert!(!session.op(op::StopJob { id: 4 }).await.unwrap());
    assert_eq!(fake.calls(), ["start", "submit:louder", "submit_with:like this:1", "undo:c2", "stop_job:3", "stop_job:4"]);
    // What the session is like now, read from it as it's asked for.
    assert_eq!(session.status().turns, 2);
    assert_eq!(session.capabilities(), Capabilities::of(&*fake));
}

#[tokio::test(flavor = "current_thread")]
async fn an_asides_words_come_as_events_with_its_id_and_stopping_it_stops_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fake = Rc::new(Fake::default());
            let session: Rc<dyn SessionHandle> = InProcess::new(fake.clone());
            let heard = Rc::new(RefCell::new(vec![]));
            let h = heard.clone();
            session.listen(Rc::new(move |event| h.borrow_mut().push(event)));
            let asking = tokio::task::spawn_local({
                let session = session.clone();
                async move { session.op(op::Aside { id: 4, question: "why the shelf?".into() }).await }
            });
            tokio::task::yield_now().await;
            assert_eq!(
                *heard.borrow(),
                [SessionEvent::Aside { id: 4, text: "It's ".into() }, SessionEvent::Aside { id: 4, text: "the low shelf.".into() }]
            );
            // Another aside's id stops nothing.
            session.tell(op::StopAside { id: 5 }.into());
            tokio::task::yield_now().await;
            assert!(!asking.is_finished(), "only its own id stops it");
            session.tell(op::StopAside { id: 4 }.into());
            assert!(asking.await.unwrap().is_err(), "a stopped aside ends");
            assert_eq!(fake.calls(), ["aside:why the shelf?"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_nobody_waits_for_runs_at_once_as_far_as_it_goes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fake = Rc::new(Fake::default());
            let session: Rc<dyn SessionHandle> = InProcess::new(fake.clone());
            session.tell(op::Pick { pick: Picked::Technique { name: "Drum bus".into(), keep: true } }.into());
            assert_eq!(fake.calls(), ["picked:Drum bus:true"], "what the controller does at once is done at once");
            // One that waits on something goes on in a task of its own.
            session.tell(op::Refresh {}.into());
            assert_eq!(fake.calls(), ["picked:Drum bus:true", "refresh:started"]);
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            assert_eq!(fake.calls(), ["picked:Drum bus:true", "refresh:started", "refresh:done"]);
        })
        .await;
}

#[test]
fn capabilities_are_the_controllers_flags() {
    let can = Capabilities::of(&Fake::default());
    assert!(can.aside);
    assert!(!can.steer && !can.palette && !can.jobs && !can.conversations);
}
