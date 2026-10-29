//! The session's protocol in this process: each request reaches the controller's method it was and answers in its own
//! type, an aside's words come as events with its id and stopping it stops it, a request nobody waits for runs at once
//! as far as it goes, and /music's, /map's and /slots' are answered from the session's own music, map and slots (a /slots
//! fetch says what it's doing, and a stop ends it, even one that comes first).
use async_trait::async_trait;
use kumi_agent::core::{
    contracts::*,
    palette::{Gathered, PaletteView},
    protocol::{op, Capabilities, InProcess, Op, SessionHandle},
    search_memory::SearchMemoryStatus,
};
use kumi_common::abort::Signal;
use kumi_types::contracts::{ChangeRecord, JsonObject};
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
        let signal = signal.expect("an aside can be stopped");
        self.call(format!("aside:{question}{}", if signal.is_cancelled() { ":stopped already" } else { "" }));
        on_text("It's ");
        on_text("the low shelf.");
        signal.cancelled().await;
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
    session.op(op::Submit { input: "louder".into(), pinned: None, attachments: vec![], references: vec![] }).await.unwrap();
    session.op(op::Submit { input: "like this".into(), pinned: None, attachments: vec![attachment()], references: vec![] }).await.unwrap();
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

#[tokio::test(flavor = "current_thread")]
async fn a_stop_that_comes_before_its_aside_starts_still_stops_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fake = Rc::new(Fake::default());
            let session: Rc<dyn SessionHandle> = InProcess::new(fake.clone());
            // Closed (its panel, or Kumi) before the task that asks it ran.
            session.tell(op::StopAside { id: 1 }.into());
            let answer =
                tokio::time::timeout(std::time::Duration::from_secs(5), session.op(op::Aside { id: 1, question: "why the shelf?".into() }))
                    .await
                    .expect("a stopped aside ends at once");
            assert!(answer.is_err());
            assert_eq!(fake.calls(), ["aside:why the shelf?:stopped already"]);
        })
        .await;
}

/// The method each request reached, by its name (and, where two of its arguments have one type, with them), answering
/// as an absent method would.
#[derive(Default)]
struct Reached(RefCell<Vec<String>>);

impl Reached {
    fn reach(&self, method: impl Into<String>) {
        self.0.borrow_mut().push(method.into());
    }
}

#[async_trait(?Send)]
impl SessionController for Reached {
    async fn start(&self) -> Result<(), RuntimeError> {
        self.reach("start");
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.reach("submit");
        Ok(())
    }
    async fn submit_with(&self, _: &str, _: Option<PinnedNode>, _: Vec<Attachment>) -> Result<(), RuntimeError> {
        self.reach("submit_with");
        Ok(())
    }
    async fn submit_with_references(
        &self,
        _: &str,
        _: Option<PinnedNode>,
        _: Vec<Attachment>,
        _: Vec<Reference>,
    ) -> Result<(), RuntimeError> {
        self.reach("submit_with_references");
        Ok(())
    }
    async fn references(&self, _: &str, _: u32) -> Result<Vec<kumi_agent::core::references::Candidate>, RuntimeError> {
        self.reach("references");
        Ok(vec![])
    }
    fn steer(&self, _: &str) -> bool {
        self.reach("steer");
        false
    }
    async fn aside(&self, _: &str, _: OnText, _: Option<Signal>) -> Result<String, RuntimeError> {
        self.reach("aside");
        Ok(String::new())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        self.reach("refresh");
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        self.reach("new_conversation");
        Ok(())
    }
    async fn reconnect(&self) -> Result<(), RuntimeError> {
        self.reach("reconnect");
        Ok(())
    }
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        self.reach("conversations");
        Ok(vec![])
    }
    async fn resume_conversation(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("resume_conversation");
        Ok(false)
    }
    async fn all_conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        self.reach("all_conversations");
        Ok(vec![])
    }
    async fn find_conversation(&self, _: &str) -> Result<ConversationMatch, RuntimeError> {
        self.reach("find_conversation");
        Ok(ConversationMatch::None)
    }
    async fn rename_conversation(&self, id: Option<&str>, name: Option<&str>) -> Result<bool, RuntimeError> {
        self.reach(format!("rename_conversation:{id:?}:{name:?}"));
        Ok(false)
    }
    async fn pin_conversation(&self, _: &str, _: bool) -> Result<bool, RuntimeError> {
        self.reach("pin_conversation");
        Ok(false)
    }
    async fn delete_conversation(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("delete_conversation");
        Ok(false)
    }
    async fn fork_conversation(&self, _: Option<&str>) -> Result<Option<String>, RuntimeError> {
        self.reach("fork_conversation");
        Ok(None)
    }
    async fn answer_set(&self, _: SetChoice) -> Result<(), RuntimeError> {
        self.reach("answer_set");
        Ok(())
    }
    async fn conversation_now(&self) -> Option<ConversationNow> {
        self.reach("conversation_now");
        None
    }
    async fn conversation_lines(&self, _: &str) -> Result<Vec<TranscriptLine>, RuntimeError> {
        self.reach("conversation_lines");
        Ok(vec![])
    }
    async fn conversation_to_export(&self) -> Option<ConversationToExport> {
        self.reach("conversation_to_export");
        None
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        self.reach("cancel");
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.reach("close");
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus { state: TurnState::Idle, connection: ConnectionState::Disconnected, turns: 0, max_turns: None, observation: None }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        self.reach("undo");
        Ok(None)
    }
    async fn memory(&self) -> Result<Option<MemoryView>, RuntimeError> {
        self.reach("memory");
        Ok(None)
    }
    async fn remembered(&self) -> Option<SearchMemoryStatus> {
        self.reach("remembered");
        None
    }
    async fn taste(&self) -> Result<Vec<KeptLine>, RuntimeError> {
        self.reach("taste");
        Ok(vec![])
    }
    async fn forget_taste(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("forget_taste");
        Ok(false)
    }
    async fn forget(&self, _: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        self.reach("forget");
        Ok(None)
    }
    async fn change_note(&self, _: &str, _: NoteChange) -> Result<Option<MemoryNote>, RuntimeError> {
        self.reach("change_note");
        Ok(None)
    }
    async fn recipes(&self) -> Result<Vec<RecipeSummary>, RuntimeError> {
        self.reach("recipes");
        Ok(vec![])
    }
    async fn instant(&self, _: &str) -> Result<Option<InstantOutcome>, RuntimeError> {
        self.reach("instant");
        Ok(None)
    }
    fn pin_for_recipe(&self, _: &PinnedNode) -> Option<PinnedNode> {
        self.reach("pin_for_recipe");
        None
    }
    async fn run_recipe(&self, _: &str, _: JsonObject) -> Result<RecipeOutcome, RuntimeError> {
        self.reach("run_recipe");
        Ok(RecipeOutcome { text: String::new(), is_error: false })
    }
    async fn forget_recipe(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("forget_recipe");
        Ok(false)
    }
    async fn techniques(&self) -> Result<Vec<TechniqueSummary>, RuntimeError> {
        self.reach("techniques");
        Ok(vec![])
    }
    async fn forget_technique(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("forget_technique");
        Ok(false)
    }
    async fn answer_technique(&self, _: bool) -> Result<bool, RuntimeError> {
        self.reach("answer_technique");
        Ok(false)
    }
    fn picked(&self, _: Picked) {
        self.reach("picked");
    }
    async fn goal(&self, _: Option<&str>) -> Result<(), RuntimeError> {
        self.reach("goal");
        Ok(())
    }
    async fn stop_goal(&self) -> Result<bool, RuntimeError> {
        self.reach("stop_goal");
        Ok(false)
    }
    async fn stop_loop(&self) -> Result<bool, RuntimeError> {
        self.reach("stop_loop");
        Ok(false)
    }
    async fn lessons(&self) -> Result<Vec<LessonEntry>, RuntimeError> {
        self.reach("lessons");
        Ok(vec![])
    }
    async fn forget_lesson(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("forget_lesson");
        Ok(false)
    }
    async fn stop_live(&self) -> Result<bool, RuntimeError> {
        self.reach("stop_live");
        Ok(false)
    }
    async fn daw_message(&self, _: &str) -> Result<bool, RuntimeError> {
        self.reach("daw_message");
        Ok(false)
    }
    async fn daw_recording(&self) -> Result<Option<bool>, RuntimeError> {
        self.reach("daw_recording");
        Ok(None)
    }
    fn stop_job(&self, _: u32) -> bool {
        self.reach("stop_job");
        false
    }
    fn run_job_again(&self, _: u32) -> Result<u32, String> {
        self.reach("run_job_again");
        Err(String::new())
    }
    fn background(&self) -> Result<u32, String> {
        self.reach("background");
        Err(String::new())
    }
    fn carry_on(&self, _: u32) -> bool {
        self.reach("carry_on");
        false
    }
    async fn palette(&self) -> Result<Option<PaletteView>, RuntimeError> {
        self.reach("palette");
        Ok(None)
    }
    async fn palette_gather(&self, _: Vec<Gathered>) -> Result<String, RuntimeError> {
        self.reach("palette_gather");
        Ok(String::new())
    }
    async fn palette_remove(&self, path: &str, size: i64, mtime: i64) -> Result<bool, RuntimeError> {
        self.reach(format!("palette_remove:{path}:{size}:{mtime}"));
        Ok(false)
    }
    async fn palette_command(&self, _: &str) -> Result<String, RuntimeError> {
        self.reach("palette_command");
        Ok(String::new())
    }
    async fn focus_body(&self, _: &BodyRequest) -> Result<Option<FocusBody>, RuntimeError> {
        self.reach("focus_body");
        Ok(None)
    }
    async fn reconfigure(&self) -> Result<(), RuntimeError> {
        self.reach("reconfigure");
        Ok(())
    }
    // Asked for, and absent: what's answered from them isn't available.
    fn music(&self) -> Option<Rc<kumi_listening::music::learning::Music>> {
        self.reach("music");
        None
    }
    fn slots_context(&self) -> Option<Rc<kumi_listening::slots::SlotsContext>> {
        self.reach("slots_context");
        None
    }
    fn map_loader(&self) -> Option<kumi_library::map::LoadMap> {
        self.reach("map_loader");
        None
    }
}

/// The controller's method each request was before the protocol, with the arguments that share a type in the order it
/// takes them; the part it's answered from for /music's, /map's and /slots' (the producer's music, the map's loader,
/// the slots' context); None for the handle's own (stopping an aside stops the signal it gave it).
fn method(request: &Op) -> Option<String> {
    Some(
        match request {
            Op::Start(_) => "start",
            Op::Submit(op::Submit { references, .. }) if !references.is_empty() => "submit_with_references",
            Op::Submit(op::Submit { attachments, .. }) if attachments.is_empty() => "submit",
            Op::Submit(_) => "submit_with",
            Op::References(_) => "references",
            Op::Steer(_) => "steer",
            Op::Aside(_) => "aside",
            Op::StopAside(_) => return None,
            Op::Cancel(_) => "cancel",
            Op::Close(_) => "close",
            Op::Undo(_) => "undo",
            Op::Refresh(_) => "refresh",
            Op::NewConversation(_) => "new_conversation",
            Op::Reconnect(_) => "reconnect",
            Op::Reconfigure(_) => "reconfigure",
            Op::Instant(_) => "instant",
            Op::Pick(_) => "picked",
            Op::Conversations(_) => "conversations",
            Op::AllConversations(_) => "all_conversations",
            Op::FindConversation(_) => "find_conversation",
            Op::ResumeConversation(_) => "resume_conversation",
            Op::RenameConversation(op::RenameConversation { id, name }) => {
                return Some(format!("rename_conversation:{:?}:{:?}", id.as_deref(), name.as_deref()))
            }
            Op::PinConversation(_) => "pin_conversation",
            Op::DeleteConversation(_) => "delete_conversation",
            Op::ForkConversation(_) => "fork_conversation",
            Op::AnswerSet(_) => "answer_set",
            Op::ConversationNow(_) => "conversation_now",
            Op::ConversationLines(_) => "conversation_lines",
            Op::ConversationToExport(_) => "conversation_to_export",
            Op::Memory(_) => "memory",
            Op::Remembered(_) => "remembered",
            Op::Taste(_) => "taste",
            Op::ForgetTaste(_) => "forget_taste",
            Op::Forget(_) => "forget",
            Op::ChangeNote(_) => "change_note",
            Op::Recipes(_) => "recipes",
            Op::PinForRecipe(_) => "pin_for_recipe",
            Op::RunRecipe(_) => "run_recipe",
            Op::ForgetRecipe(_) => "forget_recipe",
            Op::Techniques(_) => "techniques",
            Op::ForgetTechnique(_) => "forget_technique",
            Op::AnswerTechnique(_) => "answer_technique",
            Op::Lessons(_) => "lessons",
            Op::ForgetLesson(_) => "forget_lesson",
            Op::Goal(_) => "goal",
            Op::StopGoal(_) => "stop_goal",
            Op::StopLoop(_) => "stop_loop",
            Op::StopDaw(_) => "stop_live",
            Op::DawMessage(_) => "daw_message",
            Op::DawRecording(_) => "daw_recording",
            Op::StopJob(_) => "stop_job",
            Op::RunJobAgain(_) => "run_job_again",
            Op::Background(_) => "background",
            Op::CarryOn(_) => "carry_on",
            Op::Palette(_) => "palette",
            Op::PaletteGather(_) => "palette_gather",
            Op::PaletteRemove(op::PaletteRemove { path, size, mtime }) => return Some(format!("palette_remove:{path}:{size}:{mtime}")),
            Op::PaletteCommand(_) => "palette_command",
            Op::Music(_) | Op::MusicFolders(_) | Op::LearnMusic(_) | Op::SetMusicFolders(_) | Op::HoldMusic(_) | Op::ForgetMusic(_) => {
                "music"
            }
            Op::Map(_) => "map_loader",
            Op::Slots(_) => "slots_context",
            Op::StopSlots(_) => return None,
            Op::FocusBody(_) => "focus_body",
        }
        .into(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn every_request_reaches_the_method_it_was() {
    let reached = Rc::new(Reached::default());
    let session: Rc<dyn SessionHandle> = InProcess::new(reached.clone());
    // One of each, as their wire pins have them.
    for request in crate::pin_wire::requests() {
        let (name, wanted) = (request.name(), method(&request));
        reached.0.borrow_mut().clear();
        let answered = session.send(request).await;
        assert_eq!(*reached.0.borrow(), wanted.iter().cloned().collect::<Vec<_>>(), "{name}");
        // Each is answered, but what's answered from a part this session hasn't (its music, its map, its slots).
        let from_a_part = wanted.as_deref().is_some_and(|method| ["music", "map_loader", "slots_context"].contains(&method));
        match (answered, from_a_part) {
            (Err(error), true) => assert_eq!(error.message(), "Not available", "{name}"),
            (Ok(_), true) => panic!("{name}: answered without its part"),
            (Err(error), false) => panic!("{name}: {error}"),
            (Ok(_), false) => {}
        }
    }
}

/// A session with the producer's music, a map and /slots, and nothing else.
struct Parts {
    music: Rc<kumi_listening::music::learning::Music>,
    slots: Rc<kumi_listening::slots::SlotsContext>,
    map: kumi_library::map::LoadMap,
}

#[async_trait(?Send)]
impl SessionController for Parts {
    async fn start(&self) -> Result<(), RuntimeError> {
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
        SessionStatus { state: TurnState::Idle, connection: ConnectionState::Connected, turns: 0, max_turns: None, observation: None }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    fn music(&self) -> Option<Rc<kumi_listening::music::learning::Music>> {
        Some(self.music.clone())
    }
    fn slots_context(&self) -> Option<Rc<kumi_listening::slots::SlotsContext>> {
        Some(self.slots.clone())
    }
    fn map_loader(&self) -> Option<kumi_library::map::LoadMap> {
        Some(self.map.clone())
    }
}

/// A session in this process with only the producer's music (nowhere to find songs, nothing learned while a test runs),
/// a map that says it has nothing on it yet, and /slots, all kept in `folder`.
fn with_parts(folder: &std::path::Path) -> (Rc<kumi_listening::music::learning::Music>, Rc<dyn SessionHandle>) {
    use futures::FutureExt;
    use kumi_library::map::{Kind, Layer, MapData};
    use kumi_listening::{music::learning, slots};
    let mut options = learning::MusicOptions::new(folder.join("music"), None, Default::default());
    (options.places, options.fork, options.delay_ms) = (Some(vec![]), false, 3_600_000);
    let music = learning::create_music(options);
    let store = kumi_ai::auth::store::open_credential_store(folder.join("auth.json"));
    let slots =
        Rc::new(slots::SlotsContext { file: slots::file_in(folder), store: Rc::new(store), env: Default::default(), profile: None });
    let map: kumi_library::map::LoadMap = Rc::new(|| {
        async { MapData { sounds: Layer::empty(Kind::Sounds, "No sounds yet."), songs: Layer::empty(Kind::Songs, "No songs yet.") } }
            .boxed_local()
    });
    let session: Rc<dyn SessionHandle> = InProcess::new(Rc::new(Parts { music: music.clone(), slots, map }));
    (music, session)
}

#[tokio::test(flavor = "current_thread")]
async fn music_the_map_and_slots_are_answered_from_the_sessions_own() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let (music, session) = with_parts(folder.path());
            let can = session.capabilities();
            assert!(can.music && can.map && can.slots);
            // /music: nothing agreed, held or found yet; paused once asked.
            let view = session.op(op::Music {}).await.unwrap();
            assert!(!view.agreed && !view.held && view.found.is_none(), "{view:?}");
            assert!(session.op(op::MusicFolders {}).await.unwrap().is_empty());
            session.op(op::HoldMusic { held: true }).await.unwrap();
            assert!(music.held() && session.op(op::Music {}).await.unwrap().held);
            // /map: what its loader reads.
            assert_eq!(session.op(op::Map {}).await.unwrap().sounds.empty.as_deref(), Some("No sounds yet."));
            // /slots: the slots, a line each.
            let said = session.op(op::Slots { id: 1, words: String::new() }).await.unwrap();
            assert!(matches!(&said, kumi_listening::slots::Said::Slots { lines, .. } if !lines.is_empty()), "{said:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_slots_fetch_says_what_it_does_and_a_stop_ends_it_even_one_that_came_first() {
    use kumi_listening::slots::Said;
    use std::time::Duration;
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            // Kumi's models folder is here: what a link fetches would be kept in it.
            std::env::set_var("KUMI_HOME", folder.path());
            // A server on this computer that takes the connection and never answers: the fetch waits until it's stopped.
            let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let link = format!("https://127.0.0.1:{}/clap.onnx", silent.local_addr().unwrap().port());
            let words = format!("embeddings {link}");
            let (_, session) = with_parts(folder.path());
            let heard = Rc::new(RefCell::new(Vec::new()));
            session.listen({
                let heard = heard.clone();
                Rc::new(move |event| {
                    if let SessionEvent::SlotsProgress { id, line } = event {
                        heard.borrow_mut().push((id, line));
                    }
                })
            });
            let stopped = |said: &Said| matches!(said, Said::Refused(why) if why.ends_with(": stopped"));
            // Stopped while it fetches, which it said it was doing.
            let fetching = tokio::task::spawn_local({
                let (session, words) = (session.clone(), words.clone());
                async move { session.op(op::Slots { id: 1, words }).await }
            });
            for _ in 0..1_000 {
                if !heard.borrow().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(*heard.borrow(), [(1, format!("Fetching {link} (Esc stops it)…"))]);
            session.op(op::StopSlots { id: 1 }).await.unwrap();
            let said = tokio::time::timeout(Duration::from_secs(5), fetching).await.expect("stopped").unwrap().unwrap();
            assert!(stopped(&said), "{said:?}");
            // A stop that comes before its request starts still stops it, as the fetch begins.
            session.op(op::StopSlots { id: 2 }).await.unwrap();
            let said =
                tokio::time::timeout(Duration::from_secs(5), session.op(op::Slots { id: 2, words })).await.expect("stopped").unwrap();
            assert!(stopped(&said), "{said:?}");
            drop(silent);
        })
        .await;
}
