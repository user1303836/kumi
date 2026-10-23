//! What the session says, word for word, as Live comes and goes, and what Kumi says it can't do without Live.
use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::conversations::create_conversation_store,
    core::{
        contracts::*,
        errors::{FailureKind, KumiError, RuntimeError},
        evolve::Knob,
        goal::{GoalBudget, GOAL_BUDGET},
        match_run::{MatchBudget, MATCH_BUDGET},
        session::*,
    },
    create_inference_only_integration,
    kernel::budget::{transcript_of, OBSERVATION_MARKER},
    with_fallback,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    time::Duration,
};

fn golden(name: &str, actual: &str) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/session").join(name);
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    if actual != expected {
        panic!("{}", difference(&file, &expected, actual));
    }
}

/// Where a golden and what a test made first part: the line before, the golden's lines and the new ones from there,
/// and, for a long line, the column where they part.
fn difference(file: &std::path::Path, expected: &str, actual: &str) -> String {
    let (old, new): (Vec<&str>, Vec<&str>) = (expected.split('\n').collect(), actual.split('\n').collect());
    let at = old.iter().zip(&new).position(|(a, b)| a != b).unwrap_or(old.len().min(new.len()));
    let mut said = format!(
        "{} changed at line {} ({} lines now, {} before): check the difference, then run this test with KUMI_BLESS=1\n",
        file.display(),
        at + 1,
        new.len(),
        old.len()
    );
    let shown = |line: &str, from: usize| -> String {
        let chars: Vec<char> = line.chars().collect();
        let start = from.saturating_sub(60);
        let end = (from + 100).min(chars.len());
        let cut: String = chars[start.min(end)..end].iter().collect();
        format!("{}{cut}{}", if start > 0 { "…" } else { "" }, if end < chars.len() { "…" } else { "" })
    };
    let column = match (old.get(at), new.get(at)) {
        (Some(a), Some(b)) => a.chars().zip(b.chars()).position(|(x, y)| x != y).unwrap_or(a.chars().count().min(b.chars().count())),
        _ => usize::MAX,
    };
    if column != usize::MAX {
        said.push_str(&format!("(the line parts at column {})\n", column + 1));
    }
    let column = if column == usize::MAX { 0 } else { column };
    let before = at.saturating_sub(2);
    for (n, line) in old.iter().enumerate().skip(before).take(at - before) {
        said.push_str(&format!("  {:>5} | {}\n", n + 1, shown(line, column)));
    }
    for (n, line) in old.iter().enumerate().skip(at).take(3) {
        said.push_str(&format!("- {:>5} | {}\n", n + 1, shown(line, column)));
    }
    for (n, line) in new.iter().enumerate().skip(at).take(3) {
        said.push_str(&format!("+ {:>5} | {}\n", n + 1, shown(line, column)));
    }
    said
}
/// Texts by name, keys sorted, as one pretty JSON file.
fn golden_texts(name: &str, texts: BTreeMap<String, Value>) {
    golden(name, &format!("{}\n", serde_json::to_string_pretty(&texts).unwrap()));
}

type Run = Rc<dyn Fn(String, Signal, KernelEmit) -> LocalBoxFuture<'static, Result<TurnResult, RuntimeError>>>;
fn complete() -> TurnResult {
    TurnResult { stop_reason: StopReason::Completed, usage: None }
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
    listeners: RefCell<Vec<ConnectionListener>>,
    /// The integration offers an undo, which doesn't finish.
    undo_fails: Cell<bool>,
    goal_rig: RefCell<Option<Rc<dyn GoalRig>>>,
    /// The integration comes wrapped in a fallback, as the app has it.
    wrapped: Cell<bool>,
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
    async fn close(&self) {}
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
}
struct TestIntegration {
    record: Rc<Record>,
    observation: Rc<RefCell<Observation>>,
    listener: ConnectionListener,
}
#[async_trait(?Send)]
impl Integration for TestIntegration {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        (self.listener)(ConnectionState::Connecting, None);
        (self.listener)(ConnectionState::Connected, None);
        Ok(())
    }
    async fn observe(&self, _: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        Ok(self.observation.borrow().clone())
    }
    fn words(&self) -> (&'static str, &'static str) {
        ("Live", "Set")
    }
    fn has_undo(&self) -> bool {
        self.record.undo_fails.get()
    }
    async fn undo(&self, _: Option<&str>, _: Signal) -> Result<ChangeRecord, RuntimeError> {
        Err(RuntimeError::plain("secret-token-must-not-escape"))
    }
    fn has_goal(&self) -> bool {
        self.record.goal_rig.borrow().is_some()
    }
    async fn goal(&self, _: &AuditionRequest, _: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        Ok(self.record.goal_rig.borrow().clone().ok_or_else(|| "no rig".into()))
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
struct Harness {
    session: Session,
    record: Rc<Record>,
    events: Rc<RefCell<Vec<SessionEvent>>>,
    observation: Rc<RefCell<Observation>>,
}
fn harness(run: Option<Run>, config: impl FnOnce(&mut SessionOptions)) -> Harness {
    harness_with(Rc::new(Record::default()), run, config)
}
fn harness_with(record: Rc<Record>, run: Option<Run>, config: impl FnOnce(&mut SessionOptions)) -> Harness {
    let observation = Rc::new(RefCell::new(Observation {
        key: "epoch:1".into(),
        revision: None,
        label: "Fixture Set".into(),
        context: "fresh fixture context".into(),
        instructions: "fixture instructions".into(),
        tools: vec![],
        project: None,
        set: None,
        tracks: None,
        saved_at: None,
        identity: None,
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
        let bare = Rc::new(TestIntegration { record: r.clone(), observation: obs.clone(), listener });
        if r.wrapped.get() {
            with_fallback(bare, Rc::new(|| panic!("no fallback")), Rc::new(|_| {})) as Rc<dyn Integration>
        } else {
            bare
        }
    });
    let mut options = SessionOptions::new(factory, integration, Rc::new(move |e| out.borrow_mut().push(e)));
    options.timeout_ms = Some(5000);
    options.cancel_grace_ms = Some(10);
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
    fn notices(&self) -> Vec<String> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Notice { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
    fn errors(&self) -> Vec<String> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Error { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
    async fn wait_notice(&self, text: &str) {
        for _ in 0..2000 {
            if self.notices().iter().any(|n| n == text) {
                return;
            }
            delay(2).await;
        }
        panic!("no notice {text:?}: {:?}", self.notices());
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
const STILL_MISSING: &str =
    "Kumi can't reach Live. Is it open, with AbletonMcpBridge chosen as a Control Surface (Settings → Link, Tempo & MIDI)?";

/// Live goes (for each cause, while Kumi works and while it's idle), is still missing a moment later, then comes back:
/// every notice, in order.
async fn comes_and_goes(cause: Option<DisconnectCause>, working: bool, back: Option<DisconnectCause>) -> Vec<String> {
    let h = harness(Some(hang_once()), |_| {});
    h.session.start().await.unwrap();
    let running = working.then(|| spawned(&h.session, "widen the pads"));
    settle().await;
    // Working: the model is answering.
    for _ in 0..2500 {
        if !working || !h.record.calls.borrow().is_empty() {
            break;
        }
        delay(2).await;
    }
    let before = h.notices().len();
    h.connection(ConnectionState::Disconnected, cause);
    settle().await;
    h.wait_notice(STILL_MISSING).await;
    h.connection(ConnectionState::Connected, back);
    settle().await;
    // Once back, Kumi reads Live again (a request that carries on keeps going instead): its notices come by then.
    let carries_on = working && matches!(cause, Some(DisconnectCause::Busy | DisconnectCause::AskedSet)) && back.is_none();
    if !carries_on {
        for _ in 0..2500 {
            if h.session.status().state == TurnState::Idle {
                break;
            }
            delay(2).await;
        }
        assert_eq!(h.session.status().state, TurnState::Idle);
    }
    let said = h.notices()[before..].to_vec();
    if let Some(running) = running {
        running.abort();
    }
    h.session.close().await.unwrap();
    said
}
local_test!(live_coming_and_going_says_these_notices, {
    let mut texts = BTreeMap::new();
    let causes = [
        ("live", Some(DisconnectCause::Live)),
        ("bridge", Some(DisconnectCause::Bridge)),
        ("set", Some(DisconnectCause::Set)),
        ("askedset", Some(DisconnectCause::AskedSet)),
        ("busy", Some(DisconnectCause::Busy)),
        ("none", None),
    ];
    for (name, cause) in causes {
        for working in [true, false] {
            let said = comes_and_goes(cause, working, None).await;
            texts.insert(format!("{name}, {}", if working { "working" } else { "idle" }), json!(said));
        }
    }
    // Taken for busy, Live comes back with another Set.
    texts.insert(
        "busy, working, back with another set".into(),
        json!(comes_and_goes(Some(DisconnectCause::Busy), true, Some(DisconnectCause::Set)).await),
    );
    texts.insert(
        "busy, idle, back with another set".into(),
        json!(comes_and_goes(Some(DisconnectCause::Busy), false, Some(DisconnectCause::Set)).await),
    );
    texts.insert("still missing".into(), json!(STILL_MISSING));
    golden_texts("connection.json", texts);
});

const SET_A: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const SET_B: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
const KEPT: &str = "mv66me0m3fa91c";
fn live(h: &Harness, place: Option<&str>) {
    let mut o = h.observation.borrow_mut();
    o.key = place.map(|p| format!("set:{p}")).unwrap_or_else(|| "no-live".into());
    o.project = place.filter(|p| *p != "unsaved").map(|p| ProjectRef { id: p.into(), name: format!("Song {}", &p[..2]) });
    o.set = place.map(|p| format!("identity:{p}"));
}
fn kept_about(words: &str) -> SavedConversation {
    serde_json::from_value(json!({"savedAt":1,"checkpoint":{"version":1,"messages":[
        {"role":"user","content":format!("{words}{OBSERVATION_MARKER}\n{{\"tracks\":[\"Bass\"]}}\n</current_observation_untrusted>")},
        {"role":"assistant","content":"done"}
    ]}}))
    .unwrap()
}
local_test!(another_set_opening_says_these_notices, {
    let mut texts = BTreeMap::new();
    // No conversation kept for the Set that opens: a fresh one.
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    h.session.submit("one", None).await.unwrap();
    let before = h.notices().len();
    h.observation.borrow_mut().key = "epoch:2".into();
    h.session.submit("two", None).await.unwrap();
    texts.insert("fresh".into(), json!(h.notices()[before..]));
    h.session.close().await.unwrap();
    // The Set that opens has its own conversation: it carries on.
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(SET_A, KEPT, &kept_about("make the bass wider")).await.unwrap();
    let h = harness(None, |o| o.conversations = Some(store.clone()));
    live(&h, Some(SET_B));
    h.session.start().await.unwrap();
    h.session.submit("make the pads wider", None).await.unwrap();
    let before = h.notices().len();
    live(&h, Some(SET_A));
    h.session.submit("and the bass", None).await.unwrap();
    texts.insert("continuing".into(), json!(h.notices()[before..]));
    h.session.close().await.unwrap();
    golden_texts("set_changed.json", texts);
});

local_test!(undo_reconnect_and_listening_without_live_say_these_texts, {
    let mut texts = BTreeMap::new();
    // The integration's undo doesn't finish, with no word of its own.
    let record = Rc::new(Record::default());
    record.undo_fails.set(true);
    let h = harness_with(record, None, |_| {});
    h.session.start().await.unwrap();
    let before = h.errors().len();
    let _ = h.session.undo(None).await;
    texts.insert("undo didn't finish".into(), json!(h.errors()[before..]));
    h.session.close().await.unwrap();
    // As the app has it, in a fallback, with an integration that can't undo.
    let record = Rc::new(Record::default());
    record.wrapped.set(true);
    let h = harness_with(record, None, |_| {});
    h.session.start().await.unwrap();
    let before = h.errors().len();
    let _ = h.session.undo(None).await;
    texts.insert("undo without live".into(), json!(h.errors()[before..]));
    h.session.close().await.unwrap();
    // /reconnect while Live is there.
    let h = harness(None, |_| {});
    h.session.start().await.unwrap();
    let before = h.notices().len();
    h.session.reconnect().await.unwrap();
    texts.insert("reconnected".into(), json!(h.notices()[before..]));
    h.session.close().await.unwrap();
    // The listen tool asked to hear the Set, with an integration that can't.
    let h = harness(None, |o| o.listen = true);
    h.session.start().await.unwrap();
    let listen = h.record.created.borrow()[0].tools.iter().find(|t| t.name() == kumi_runtime::LISTEN_TOOL).cloned().unwrap();
    let heard = listen.execute(json!({"mix":true}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    texts.insert("listen without live".into(), serde_json::to_value(heard).unwrap());
    h.session.close().await.unwrap();
    golden_texts("session_texts.json", texts);
});

/// An integration without Live's abilities, for the fallback's own words.
struct Bare {
    error: Option<RuntimeError>,
}
#[async_trait(?Send)]
impl Integration for Bare {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        self.error.clone().map_or(Ok(()), Err)
    }
    async fn observe(&self, _: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        Err(RuntimeError::plain("not read"))
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
#[tokio::test(flavor = "current_thread")]
async fn without_live_the_fallback_says_these_texts() {
    let said = Rc::new(RefCell::new(vec![]));
    let notes = said.clone();
    let primary = Rc::new(Bare { error: Some(KumiError::new(FailureKind::Live, "Live isn't open.").into()) });
    let fallen = with_fallback(
        primary,
        Rc::new(|| Rc::new(Bare { error: None }) as Rc<dyn Integration>),
        Rc::new(move |s| notes.borrow_mut().push(s)),
    );
    fallen.start(Signal::new()).await.unwrap();
    let request: AuditionRequest = serde_json::from_value(json!({"candidates":[]})).unwrap();
    let mut texts = BTreeMap::new();
    texts.insert("fell back with".into(), json!(*said.borrow()));
    texts.insert("undo".into(), json!(fallen.undo(None, Signal::new()).await.unwrap_err().message()));
    texts.insert("audition".into(), json!(fallen.audition(&request, Signal::new()).await.unwrap().unwrap_err()));
    texts.insert("goal".into(), json!(fallen.goal(&request, Signal::new()).await.unwrap().err().unwrap()));
    texts.insert("hear".into(), json!(fallen.hear(&HearRequest::default(), Signal::new()).await.unwrap().unwrap_err()));
    golden_texts("fallback.json", texts);
}

/// The inference-only integration, as the app has it without a bridge and as the fallback: its whole observation (the
/// moment it was read set aside), its words and how it says it's connected.
#[tokio::test(flavor = "current_thread")]
async fn the_inference_only_integration_observes_exactly_this() {
    let states = Rc::new(RefCell::new(vec![]));
    let seen = states.clone();
    let integration = create_inference_only_integration(Rc::new(move |state| seen.borrow_mut().push(state)));
    integration.start(Signal::new()).await.unwrap();
    let observation = integration.observe(Signal::new(), None).await.unwrap();
    let at = regex::Regex::new(r#"^\{"observedAt":"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z)","#).unwrap();
    let observed = at.captures(&observation.context).expect("observedAt comes first").get(1).unwrap().as_str().to_owned();
    assert!(chrono::DateTime::parse_from_rfc3339(&observed).is_ok());
    let context = observation.context.replacen(&observed, "<observed at>", 1);
    let (daw, noun) = integration.words();
    let whole = json!({
        "key": observation.key,
        "revision": observation.revision,
        "label": observation.label,
        "context": context,
        "instructions": {
            "bytes": observation.instructions.len(),
            "sha256": hex::encode(Sha256::digest(observation.instructions.as_bytes())),
        },
        "tools": observation.tools.iter().map(|t| t.name().to_owned()).collect::<Vec<_>>(),
        "project": observation.project.as_ref().map(|p| json!({"id": p.id, "name": p.name})),
        "set": observation.set,
        "tracks": observation.tracks,
        "savedAt": observation.saved_at,
        "words": [daw, noun],
        "startedAs": *states.borrow(),
    });
    golden("inference_only.json", &format!("{}\n", serde_json::to_string_pretty(&whole).unwrap()));
}

fn audition(score: f64, label: &str) -> AuditionEvent {
    serde_json::from_value(json!({"round":1,"best":{"label":label,"score":score},"takes":[{"label":label,"score":score,"where":{"track":"Candidate"}},{"label":"Other","score":score-10.}],"gaps":["attack too slow"],"request":{"candidates":[{"track":"track:1","label":"Drift"}],"fromBeat":16,"beats":4,"reference":"~/ref.wav"}})).unwrap()
}
/// A rig whose render tracks can't be removed.
struct StuckRig {
    scores: Vec<f64>,
    values_score: bool,
    calls: Cell<usize>,
}
impl StuckRig {
    fn knob() -> Knob {
        Knob { r#ref: "knob:1".into(), device: "1:Operator".into(), name: "Filter Freq".into(), min: 0., max: 1., step: None, value: 0.2 }
    }
}
#[async_trait(?Send)]
impl GoalRig for StuckRig {
    fn slots(&self) -> Vec<GoalSlotInfo> {
        vec![GoalSlotInfo { name: "Candidate".into(), label: "Drift".into(), chain: "Operator".into(), knobs: vec![Self::knob()] }]
    }
    fn screens(&self) -> bool {
        false
    }
    async fn add(&self, _: &AuditionCandidate, _: Signal) -> Result<Result<GoalSlotInfo, String>, RuntimeError> {
        Ok(Ok(self.slots()[0].clone()))
    }
    async fn generation(
        &self,
        trials: &[GenerationTrial],
        signal: Signal,
        _: Option<GenerationOptions>,
    ) -> Result<Generation, RuntimeError> {
        let at = self.calls.get();
        self.calls.set(at + 1);
        tokio::select! {_=signal.cancelled()=>return Err(RuntimeError::Aborted),_=delay(1)=>{}}
        let mut result = Generation::default();
        for trial in trials {
            let score = if self.values_score {
                trial.values[0] * 100.
            } else {
                *self.scores.get(at).or_else(|| self.scores.last()).unwrap_or(&50.)
            };
            result.scores.insert(trial.slot.clone(), score);
            result.gaps.insert(trial.slot.clone(), vec!["attack too slow".into()]);
        }
        Ok(result)
    }
    async fn keep_best(&self, _: &str, _: &[Knob], _: &[f64], _: Signal) -> Result<String, RuntimeError> {
        Ok("Kumi · Goal best".into())
    }
    async fn settle(&self, _: &str, _: &[Knob], _: &[f64], _: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
    }
    async fn tidy(&self, _: &[String], _: Signal) -> Result<Vec<String>, RuntimeError> {
        Ok(vec![])
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        Err(RuntimeError::plain("the render tracks stayed"))
    }
}
fn seeded(o: &mut SessionOptions) {
    let random = Rc::new(RefCell::new(kumi_runtime::core::evolve::seeded(123)));
    o.goal_random = Some(Rc::new(move || (random.borrow_mut())()));
}
local_test!(a_knob_search_whose_render_tracks_stay_says_these_texts, {
    // The match's knob search, which keeps a better setting or puts the old one back: the wrap-up the model reads.
    tokio::time::pause();
    let mut texts = BTreeMap::new();
    // What the search found depends on its random draws and the platform's float maths: the numbers are set aside.
    let numbers = [
        (regex::Regex::new(r"tried \d+ settings").unwrap(), "tried <n> settings"),
        (regex::Regex::new(r"→ \d+% at full length").unwrap(), "→ <to>% at full length"),
        (regex::Regex::new(r"\(50% → \d+%\)").unwrap(), "(50% → <to>%)"),
        (regex::Regex::new(r"Filter Freq 0\.2 → \d+(\.\d+)?").unwrap(), "Filter Freq 0.2 → <value>"),
    ];
    for improves in [false, true] {
        let rounds = Rc::new(Cell::new(0));
        let session = Rc::new(RefCell::new(None::<Session>));
        let active = session.clone();
        let run: Run = Rc::new(move |_, signal, emit| {
            let n = rounds.get();
            rounds.set(n + 1);
            let session = active.borrow().clone().unwrap();
            async move {
                if n == 0 {
                    session.watch(WatchEvent::Audition(audition(50., "Drift")));
                }
                if signal.is_cancelled() {
                    return Err(RuntimeError::Aborted);
                }
                emit(KernelEvent::Text { text: format!("answer {}", n + 1) })?;
                Ok(complete())
            }
            .boxed_local()
        });
        let h = harness(Some(run), |o| {
            o.match_budget = Some(MatchBudget { rounds: 0, polish_ms: Some(50), ..MATCH_BUDGET });
            seeded(o);
        });
        *session.borrow_mut() = Some(h.session.clone());
        *h.record.goal_rig.borrow_mut() = Some(Rc::new(StuckRig { scores: vec![50.], values_score: improves, calls: Cell::new(0) }));
        h.session.start().await.unwrap();
        h.session.submit("match this reference", None).await.unwrap();
        let calls = h.record.calls.borrow().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        let mut said = calls[1].split(OBSERVATION_MARKER).next().unwrap().to_owned();
        for (pattern, with) in numbers.iter().filter(|_| improves) {
            said = pattern.replace_all(&said, *with).into_owned();
        }
        texts.insert(format!("match polish, {}", if improves { "kept" } else { "none beat it" }), json!(said));
        h.session.close().await.unwrap();
    }
    golden_texts("knob_search.json", texts);
});
local_test!(a_goal_whose_render_tracks_stay_says_this_notice, {
    let session = Rc::new(RefCell::new(None::<Session>));
    let active = session.clone();
    let run: Run = Rc::new(move |_, _, emit| {
        let session = active.borrow().clone().unwrap();
        async move {
            let mut heard = audition(50., "Drift");
            heard.request.as_mut().unwrap().candidates[0].track = "Candidate".into();
            heard.request.as_mut().unwrap().candidates[0].clip = Some("first".into());
            session.watch(WatchEvent::Audition(heard));
            emit(KernelEvent::Text { text: "Tried: a brighter filter".into() })?;
            Ok(complete())
        }
        .boxed_local()
    });
    let h = harness(Some(run), |o| {
        o.goal_budget = Some(GoalBudget { ms: 2000, ..GOAL_BUDGET });
        seeded(o);
    });
    *session.borrow_mut() = Some(h.session.clone());
    *h.record.goal_rig.borrow_mut() = Some(Rc::new(StuckRig { scores: vec![60., 75., 96.], values_score: false, calls: Cell::new(0) }));
    h.session.start().await.unwrap();
    let before = h.notices().len();
    h.session.submit("/loop make my pad sound like the reference", None).await.unwrap();
    let said: Vec<_> = h.notices()[before..].iter().filter(|n| n.starts_with("Goal ")).cloned().collect();
    let mut texts = BTreeMap::new();
    texts.insert("goal done".into(), json!(said));
    h.session.close().await.unwrap();
    golden_texts("goal.json", texts);
});
