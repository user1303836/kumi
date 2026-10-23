//! A DAW that isn't Live, held in memory: core's engines run over it (HISTORY and its undo, the plan executor, the
//! describers, refs, the instant planner, folding, pictures, the project rules). It answers each optional question
//! both ways (a plan's own undo step, refs that move or stay, the emergency stop, batches, the safety copy, being
//! there to undo), and nothing core tells the producer or the model over it is in Live's words. Core's own texts are
//! checked for Live's words too: each one left is listed below with why.
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use kumi_runtime::core::{
    contracts::{ChangeRecord, ChangeState, JsonObject, ToolResult},
    daw::{DawProfile, DawWords, PromptVocab},
    describe::{describe_changes, describe_watch_changes, Detail, ProjectChange, ProjectItem, ProjectWords},
    errors::RuntimeError,
    fold::fold_tracks_with,
    history::{next_change_id, Applied, ChangeHost, Entry, History, Restore, UndoResult},
    phrases::{instant_steps, parse, InstantVocab, PhraseTrack, Switch},
    picture::{curves, CurveReader, Dynamics, EqBand, ParameterRow, PictureValue},
    plan::{make_changes, ActionOutcome, ActionSummary, Batch, ChangeOutcome, PlanHost, StepKind},
    project::{decide_project_with, ProjectRules},
    refs::{moved_index, Book, Moved, RefScheme, Shift},
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::LazyLock,
};

/// Live's words, whole: none may reach the producer or the model from core over another DAW.
static LIVE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|[Rr]acks?|[Cc]lip slots?|[Ss]cenes?)\b")
        .unwrap()
});

const FAKE: DawProfile = DawProfile {
    id: None,
    words: DawWords { app: "Fakeloop", place: "song", places: "songs", master: "the master track", timeline: "timeline" },
    caps: DawProfile::UNKNOWN.caps,
    vocab: PromptVocab {
        own_undo: "Fakeloop's own undo (Ctrl-Z)",
        refs_retired: "Fakeloop may have taken the tracks back, so Kumi's references are retired: discover again before using any.",
        structure: "tracks",
        ..DawProfile::UNKNOWN.vocab
    },
};

/// Each optional answer the DAW gives.
#[derive(Debug, Clone, Copy)]
struct Answers {
    /// A plan gets one undo step of the DAW's own.
    plan_steps: bool,
    /// The emergency stop stops it.
    stops: bool,
    /// It's there to take a change back.
    reachable: bool,
    /// The bridge takes a batch as one change.
    batches: bool,
    /// The song was saved, so a copy is kept before a risky step.
    copies: bool,
}
impl Answers {
    fn all() -> Vec<Answers> {
        (0..32)
            .map(|bits: u32| Answers {
                plan_steps: bits & 1 != 0,
                stops: bits & 2 != 0,
                reachable: bits & 4 != 0,
                batches: bits & 8 != 0,
                copies: bits & 16 != 0,
            })
            .collect()
    }
}

/// Refs that are positions (`1:track:3`), which a restructure moves, or stable ids (`1:track:g3`), which it doesn't.
#[derive(Default)]
struct Refs<const POSITIONAL: bool>;
impl<const POSITIONAL: bool> RefScheme for Refs<POSITIONAL> {
    fn positional(&self) -> bool {
        POSITIONAL
    }
    fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str> {
        reference.split(':').nth(1)
    }
    fn moved(&self, shift: &Shift, reference: &str) -> Moved {
        let parts: Vec<&str> = reference.split(':').collect();
        let Some(index) = parts.get(2).and_then(|index| index.parse::<usize>().ok()) else { return Moved::Same };
        match moved_index(index, &shift.tracks_gone, &shift.tracks_made) {
            Some(now) if now == index => Moved::Same,
            Some(now) => Moved::To(format!("{}:{}:{now}", parts[0], parts[1])),
            None => Moved::Gone,
        }
    }
    fn place_of(&self, reference: &str) -> String {
        reference.into()
    }
    fn refs_in<'a>(&self, _text: &'a str) -> Vec<&'a str> {
        vec![]
    }
    fn reference_fields(&self) -> &[String] {
        &[]
    }
    fn parents(&self, _kind: &str) -> Option<Vec<String>> {
        None
    }
    fn parent_hint(&self, _kind: &str, _parent: &str) -> &'static str {
        ""
    }
    fn row_refs(&self, _kind: &str, _row: &JsonObject) -> Vec<(String, String, bool)> {
        vec![]
    }
    fn track_of(&self, reference: &str) -> Option<String> {
        let parts: Vec<&str> = reference.split(':').collect();
        (parts.len() >= 3 && parts[1] == "track").then(|| parts[..3].join(":"))
    }
}

/// What HISTORY asks of the fake DAW.
struct FakeChanges<const P: bool> {
    answers: Answers,
    book: RefCell<Book<Refs<P>>>,
    calls: RefCell<Vec<String>>,
    /// Transactions whose undo the DAW won't confirm.
    unsure: RefCell<Vec<String>>,
    /// Whether each undo was told to discard, in order.
    discards: RefCell<Vec<bool>>,
}
#[async_trait(?Send)]
impl<const P: bool> ChangeHost for FakeChanges<P> {
    type Refs = Refs<P>;
    type Material = Vec<String>;
    fn profile(&self) -> &DawProfile {
        &FAKE
    }
    fn book(&self) -> &RefCell<Book<Refs<P>>> {
        &self.book
    }
    fn lifetime(&self) -> Signal {
        Signal::new()
    }
    fn now_ms(&self) -> i64 {
        1_700_000_000_000
    }
    fn keep_soon(&self) {}
    fn release(&self, transactions: Vec<String>) {
        self.calls.borrow_mut().push(format!("release {}", transactions.join(",")));
    }
    async fn ready(&self, _signal: Signal) -> bool {
        true
    }
    fn reachable(&self) -> bool {
        self.answers.reachable
    }
    fn plan_steps(&self) -> bool {
        self.answers.plan_steps
    }
    async fn step_begin(&self) -> Option<String> {
        self.calls.borrow_mut().push("step begin".into());
        Some("s1".into())
    }
    async fn step_end(&self, id: String) {
        self.calls.borrow_mut().push(format!("step end {id}"));
    }
    async fn stop_transport(&self, _signal: Signal) -> bool {
        self.calls.borrow_mut().push("stop".into());
        self.answers.stops
    }
    async fn revert(
        &self,
        history: &History<Self>,
        entry: &Entry<Vec<String>>,
        _revert: Vec<Value>,
        _signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        Ok(history.undone(entry))
    }
    async fn undo_material(
        &self,
        history: &History<Self>,
        entry: &Entry<Vec<String>>,
        _snapshot: &Applied<Vec<String>>,
        material: Vec<String>,
        _undo_key: &str,
        _discard: bool,
        _signal: Signal,
    ) -> Result<UndoResult, RuntimeError> {
        let record = history.update(entry, ChangeState::Undone, None);
        Ok(UndoResult::with(record, format!("Brought back {}.", material.join(", ")), false))
    }
    async fn undo_transaction(
        &self,
        history: &History<Self>,
        entry: &Entry<Vec<String>>,
        snapshot: &Applied<Vec<String>>,
        _undo_key: &str,
        discard: bool,
    ) -> Result<Option<UndoResult>, RuntimeError> {
        self.discards.borrow_mut().push(discard);
        if !self.unsure.borrow().contains(&snapshot.transaction_id) {
            return Ok(None);
        }
        let record = history.update(entry, ChangeState::Unsure, Some("Fakeloop didn't confirm the undo; try again.".into()));
        Ok(Some(UndoResult::with(record, "Fakeloop didn't answer the undo; it can be retried.", true)))
    }
}

struct Kind {
    tool: &'static str,
    since: Option<&'static str>,
}
impl StepKind for Kind {
    fn since(&self) -> Option<&str> {
        self.since
    }
}
const fn kind(tool: &'static str) -> Kind {
    Kind { tool, since: None }
}
static CHANGES: [Kind; 9] = [
    kind("set_tempo"),
    kind("add_track"),
    kind("rename_track"),
    kind("delete_clip"),
    kind("set_param"),
    kind("refuse_me"),
    kind("risky"),
    kind("untracked"),
    Kind { tool: "newer", since: Some("9") },
];
static SET_PARAMS: Kind = kind("set_params");
static ACTIONS: [Kind; 1] = [kind("play")];
const BATCHES: [Batch; 1] = [Batch { tool: "set_param", kind: "set_params", key: "deviceRef", most: 4, what: "parameters" }];

/// The fake DAW's change engine: what a plan asks of it.
struct FakeDaw<const P: bool> {
    history: History<FakeChanges<P>>,
    names: RefCell<IndexMap<String, String>>,
    tracks: Cell<usize>,
    /// What core told the producer: HISTORY's records and the plan's transport notes.
    said: Rc<RefCell<Vec<String>>>,
}
impl<const P: bool> FakeDaw<P> {
    fn new(answers: Answers) -> Rc<Self> {
        let said = Rc::new(RefCell::new(Vec::<String>::new()));
        let heard = said.clone();
        let host = FakeChanges {
            answers,
            book: RefCell::default(),
            calls: RefCell::default(),
            unsure: RefCell::default(),
            discards: RefCell::default(),
        };
        let told = move |record: ChangeRecord| {
            let mut heard = heard.borrow_mut();
            heard.push(record.title);
            heard.extend(record.note);
        };
        let history = History::over(host, Some(1_000), Some(Rc::new(told)));
        Rc::new(Self { history, names: RefCell::default(), tracks: Cell::new(0), said })
    }
    fn track_ref(&self, index: usize) -> String {
        if P {
            format!("1:track:{index}")
        } else {
            format!("1:track:g{index}")
        }
    }
    fn record(&self, family: &str, title: &str, transaction: &str, restore: Option<Restore>) -> String {
        let id = next_change_id();
        let record: ChangeRecord = serde_json::from_value(json!({"id":id,"family":family,"title":title,"state":"applied","at":0})).unwrap();
        self.history.remember(record, transaction.into(), restore);
        id
    }
}
#[async_trait(?Send)]
impl<const P: bool> PlanHost for FakeDaw<P> {
    type Changes = FakeChanges<P>;
    type Change = Kind;
    type Action = Kind;
    fn history(&self) -> &History<FakeChanges<P>> {
        &self.history
    }
    fn names(&self) -> &RefCell<IndexMap<String, String>> {
        &self.names
    }
    fn change_kind(&self, tool: &str) -> Option<&'static Kind> {
        CHANGES.iter().find(|kind| kind.tool == tool)
    }
    fn action_kind(&self, tool: &str) -> Option<&'static Kind> {
        ACTIONS.iter().find(|kind| kind.tool == tool)
    }
    fn structural(&self, tool: &str) -> bool {
        tool == "add_track"
    }
    fn moves_transport(&self, tool: &str) -> bool {
        tool == "play"
    }
    fn may_lose_material(&self, tool: &str) -> bool {
        tool == "delete_clip"
    }
    fn supported(&self, since: Option<&str>) -> bool {
        since.is_none()
    }
    fn too_old(&self, since: Option<&str>) -> String {
        format!("That needs Fakeloop's bridge {} or later.", since.unwrap_or_default())
    }
    fn tempo(&self) -> Option<f64> {
        Some(120.0)
    }
    fn batches(&self) -> &[Batch] {
        &BATCHES
    }
    fn batch_kind(&self, _batch: &Batch) -> &'static Kind {
        &SET_PARAMS
    }
    fn batch_offered(&self, _batch: &Batch) -> bool {
        self.history.host.answers.batches
    }
    fn batch_input(&self, _batch: &Batch, group: &[JsonObject]) -> JsonObject {
        let values: Vec<_> = group.iter().map(|item| json!({"parameter":item["parameter"],"value":item["value"]})).collect();
        json!({"deviceRef":group[0]["deviceRef"],"values":values}).as_object().unwrap().clone()
    }
    async fn change(&self, kind: &'static Kind, input: JsonObject, _signal: Signal, _confirmed: bool) -> ChangeOutcome {
        let done = |reply: Value| ChangeOutcome { text: reply.to_string(), is_error: false, missed: None, stops: false, applied: None };
        match kind.tool {
            "set_tempo" => {
                let title = format!("Tempo 120 → {} BPM", input["bpm"]);
                let id = self.record("tempo", &title, "t-tempo", None);
                done(json!({"changed":title,"change":id}))
            }
            "add_track" => {
                // New tracks go first, so a positional DAW's other tracks move along.
                let made = self.track_ref(if P { 0 } else { self.tracks.get() + 100 });
                self.tracks.set(self.tracks.get() + 1);
                let title = format!("Added track “{}”", input["name"].as_str().unwrap_or_default());
                let id = self.record("structure", &title, &format!("t-add-{}", self.tracks.get()), None);
                let shift = Shift { tracks_made: vec![0], ..Default::default() };
                if P {
                    self.history.host.book.borrow_mut().shift(&shift);
                    self.history.restructured(&id, &shift);
                }
                let short = self.history.host.book.borrow_mut().short_ref(&made);
                done(json!({"changed":title,"change":id,"ref":short}))
            }
            "rename_track" => {
                let reference = input["trackRef"].as_str().unwrap_or_default().to_owned();
                let restore = Restore { reference: reference.clone(), field: "name".into(), value: Some("Pad".into()) };
                let title = format!("Renamed to “{}”", input["name"].as_str().unwrap_or_default());
                let id = self.record("rename", &title, "t-rename", Some(restore));
                done(json!({"changed":title,"change":id}))
            }
            "delete_clip" => {
                let id = self.record("clip", "Deleted clip “Hook”", "t-delete", None);
                self.history.attach_material(&id, vec!["clip “Hook”".into()]);
                done(json!({"changed":"Deleted clip “Hook”","change":id}))
            }
            "set_param" => {
                let title = format!("{} → {}", input["parameter"].as_str().unwrap_or_default(), input["value"]);
                let id = self.record("parameter", &title, "t-param", None);
                done(json!({"changed":title,"change":id}))
            }
            "set_params" => {
                let lines: Vec<_> = input["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| format!("{} → {}", value["parameter"].as_str().unwrap_or_default(), value["value"]))
                    .collect();
                let id = self.record("parameter", &format!("{} parameters on Fakeverb", lines.len()), "t-params", None);
                done(json!({"changed":"Fakeverb","change":id,"lines":lines}))
            }
            "refuse_me" => ChangeOutcome::error("Fakeloop refused it: nothing changed."),
            "risky" => ChangeOutcome::stop("Fakeloop didn't confirm this change, so it may or may not have happened."),
            "untracked" => {
                let id = self.record("device", "Loaded Fakeverb", "", None);
                done(json!({"changed":"Loaded Fakeverb","change":id}))
            }
            other => ChangeOutcome::error(format!("no {other}")),
        }
    }
    async fn act(&self, _action: &'static Kind, _input: JsonObject, _signal: Signal) -> ActionOutcome {
        let done = ActionSummary { title: "Playing".into(), playing: Some(true), recording: None };
        ActionOutcome { text: "Playing".into(), is_error: false, maybe: None, done: Some(done) }
    }
    async fn safety_copy(&self, _signal: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(self.history.host.answers.copies.then(|| "/songs/Beat copy.fsong".into()))
    }
    fn emit_action(&self, done: &ActionSummary) {
        self.said.borrow_mut().push(done.title.clone());
    }
}

fn steps(steps: Value) -> JsonObject {
    json!({"steps":steps,"final":true}).as_object().unwrap().clone()
}
fn said(result: &ToolResult) -> Vec<String> {
    let mut said = vec![result.text.clone()];
    said.extend(result.reply.clone());
    said
}
fn id_of(history: &History<impl ChangeHost>, title: &str) -> String {
    history.entries.borrow().values().find(|entry| entry.borrow().record.title == title).unwrap().borrow().record.id.clone()
}

/// A session over the fake DAW, with these answers: what core said to the producer and the model.
async fn session<const P: bool>(answers: Answers) -> Vec<String> {
    let daw = FakeDaw::<P>::new(answers);
    let mut out = vec![];
    // Hand the DAW a track it had, for a restructure to move.
    let before = daw.track_ref(if P { 0 } else { 7 });
    daw.history.host.book.borrow_mut().short_ref(&before);

    // A plan that runs: the safety copy before its third step, one undo step, a batch when offered, a wait in beats.
    let result = make_changes(
        &daw,
        steps(json!([
            {"tool":"set_tempo","input":{"bpm":124}},
            {"tool":"add_track","as":"pad","input":{"name":"Pad"}},
            {"tool":"rename_track","input":{"trackRef":"@pad","name":"Pads"}},
            {"tool":"set_param","input":{"deviceRef":"1:device:1","parameter":"Drive","value":0.5}},
            {"tool":"set_param","input":{"deviceRef":"1:device:1","parameter":"Tone","value":0.2}},
            {"tool":"set_param","input":{"deviceRef":"1:device:1","parameter":"Mix","value":0.8}},
            {"tool":"wait","input":{"beats":0.01}},
            {"tool":"delete_clip","input":{"clipRef":"1:clip:3"}},
        ])),
        Signal::new(),
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{}", result.text);
    let reply = result.reply.clone().unwrap();
    assert!(reply.starts_with("Done:\n- Tempo 120 → 124 BPM"), "{reply}");
    assert_eq!(
        reply.contains("First, Kumi kept a copy of your song as last saved, next to it: Beat copy.fsong"),
        answers.copies,
        "{reply}"
    );
    let calls = daw.history.host.calls.borrow().clone();
    assert_eq!(calls.contains(&"step begin".to_owned()), answers.plan_steps, "{calls:?}");
    assert_eq!(calls.contains(&"step end s1".to_owned()), answers.plan_steps, "{calls:?}");
    let batched = daw.history.entries.borrow().values().any(|entry| entry.borrow().record.title == "3 parameters on Fakeverb");
    assert_eq!(batched, answers.batches);
    // The track the DAW had moved along when the new one went first, if its refs are positions.
    let now = daw.history.host.book.borrow().lengthen(&json!({"ref":"track:1"}))["ref"].clone();
    assert_eq!(now, json!(if P { "1:track:1".to_owned() } else { before.clone() }));
    out.extend(said(&result));

    // Refused steps, the ones held back with them, a step the bridge is too old for, a tool that isn't one, and
    // playback left going when the plan didn't finish.
    let result = make_changes(
        &daw,
        steps(json!([
            {"tool":"play","input":{}},
            {"tool":"refuse_me","as":"x","input":{}},
            {"tool":"rename_track","input":{"trackRef":"@x","name":"Y"}},
            {"tool":"newer","input":{}},
            {"tool":"bogus","input":{}},
        ])),
        Signal::new(),
    )
    .await
    .unwrap();
    assert!(result.is_error);
    let reply: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(reply["refusedNote"], json!(kumi_runtime::core::plan::refused_note(&FAKE)));
    assert!(reply["refusedNote"].as_str().unwrap().contains("or the song's tracks), so"), "{}", reply["refusedNote"]);
    assert_eq!(reply["heldBack"][0]["step"], json!(3));
    assert_eq!(reply["refused"][1]["error"], json!("That needs Fakeloop's bridge 9 or later."));
    let stopped = if answers.stops {
        "Kumi stopped playback, since the plan didn't finish."
    } else {
        "The plan didn't finish and Fakeloop may still be playing; tell the producer."
    };
    assert_eq!(reply["playback"], json!(stopped));
    if !answers.stops {
        assert!(daw.said.borrow().contains(&"Fakeloop may still be playing: press space in Fakeloop, or /stop".to_owned()));
    }
    out.extend(said(&result));

    // A change the DAW may have made stops the plan.
    let result = make_changes(&daw, steps(json!([{"tool":"risky","input":{}},{"tool":"set_tempo","input":{"bpm":90}}])), Signal::new())
        .await
        .unwrap();
    assert!(result.is_error && result.text.contains("\"skipped\":1"), "{}", result.text);
    out.extend(said(&result));

    // Undo: material made again, a rename's name put back in the book, a change only the DAW's own undo takes back,
    // one that's gone, and one that's already undone.
    let deleted = id_of(&daw.history, "Deleted clip “Hook”");
    let undone = daw.history.undo(&deleted, Signal::new(), false).await.unwrap();
    if answers.reachable {
        assert_eq!(undone.text, "Brought back clip “Hook”.");
        let again = daw.history.undo(&deleted, Signal::new(), false).await.unwrap();
        assert!(again.text.contains("\"already\":true"), "{}", again.text);
        out.push(again.text);
    } else {
        assert_eq!(undone.text, "Kumi can't reach Fakeloop right now, so it can't undo.");
    }
    out.push(undone.for_model());
    let result = make_changes(&daw, steps(json!([{"tool":"untracked","input":{}}])), Signal::new()).await.unwrap();
    out.extend(said(&result));
    let untracked = daw.history.undo("last", Signal::new(), false).await.unwrap();
    if answers.reachable {
        assert_eq!(untracked.text, "Kumi can't take this back; Fakeloop's own undo (Ctrl-Z) can.");
    }
    out.push(untracked.for_model());
    out.push(daw.history.undo("c999999", Signal::new(), false).await.unwrap().text);

    // A group taken back partway: one of its changes the DAW won't confirm.
    let members: Vec<String> = daw
        .history
        .entries
        .borrow()
        .values()
        .filter(|entry| entry.borrow().record.state == ChangeState::Applied)
        .map(|entry| entry.borrow().record.id.clone())
        .collect();
    let group = daw.history.grouped("Built a pad", &members, &[]).unwrap();
    daw.history.host.unsure.borrow_mut().push("t-tempo".into());
    let partly = daw.history.undo(&group, Signal::new(), false).await.unwrap();
    if answers.reachable {
        assert!(partly.text.starts_with("Kumi took back "), "{}", partly.text);
        assert!(partly.text.contains("changed in Fakeloop since, so Kumi left"), "{}", partly.text);
    }
    out.push(partly.for_model());

    // A restructure whose undo the DAW didn't confirm: Kumi's refs are retired, in the DAW's words.
    let result = make_changes(&daw, steps(json!([{"tool":"add_track","input":{"name":"Keys"}}])), Signal::new()).await.unwrap();
    out.extend(said(&result));
    let added = id_of(&daw.history, "Added track “Keys”");
    let transaction = daw.history.entries.borrow()[&added].borrow().transaction_id.clone();
    daw.history.host.unsure.borrow_mut().push(transaction);
    let retired = daw.history.undo(&added, Signal::new(), false).await.unwrap();
    if answers.reachable {
        assert_eq!(retired.retired, P, "only moved refs are retired");
        if P {
            assert!(retired.for_model().ends_with(FAKE.vocab.refs_retired), "{}", retired.for_model());
            assert!(daw.history.host.book.borrow().named_references().is_empty());
        }
    }
    out.push(retired.for_model());

    out.extend(daw.said.borrow().iter().cloned());
    out.extend(daw.history.entries.borrow().values().flat_map(|entry| {
        let record = entry.borrow().record.clone();
        [Some(record.title), record.note].into_iter().flatten()
    }));
    out
}

fn leaks(texts: &[String]) -> Vec<String> {
    texts.iter().filter(|text| LIVE_WORDS.is_match(text)).cloned().collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_session_over_another_daw_says_nothing_in_lives_words_whatever_it_answers() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for answers in Answers::all() {
                for (positional, said) in [(true, session::<true>(answers).await), (false, session::<false>(answers).await)] {
                    let found = leaks(&said);
                    assert!(found.is_empty(), "{answers:?}, positional refs {positional}: {found:#?}");
                }
            }
        })
        .await;
}

/// The fake DAW's project words and phrase steps.
const WORDS: ProjectWords = ProjectWords {
    kinds: &[("track", "tracks"), ("pattern", "patterns"), ("device", "devices")],
    project: "song",
    rank: &["song", "track", "pattern", "device"],
    opaque: &["device"],
};
struct Phrases;
impl InstantVocab for Phrases {
    fn tempo(&self, bpm: f64) -> Value {
        json!({"tool":"set_tempo","input":{"bpm":bpm}})
    }
    fn track(&self, reference: &str, switch: Switch, on: bool) -> Value {
        json!({"tool":"switch_track","input":{"trackRef":reference,"switch":format!("{switch:?}"),"on":on}})
    }
    fn loop_on(&self, start: f64, length: f64) -> Value {
        json!({"tool":"set_loop","input":{"start":start,"length":length}})
    }
    fn loop_off(&self) -> Value {
        json!({"tool":"set_loop","input":{"on":false}})
    }
    fn metronome(&self, on: bool) -> Value {
        json!({"tool":"set_click","input":{"on":on}})
    }
}
struct Curves;
impl CurveReader for Curves {
    fn eq_bands(&self, _rows: &[ParameterRow]) -> Option<Vec<EqBand>> {
        None
    }
    fn dynamics(&self, rows: &[ParameterRow]) -> Option<Dynamics> {
        let value = |name: &str| rows.iter().find(|row| row.name == name).map(|row| row.value);
        Some(Dynamics { threshold_db: value("Threshold")?, ratio: value("Ratio")?, knee_db: 0.0, makeup_db: 0.0 })
    }
}
struct Rules {
    templates: bool,
}
impl ProjectRules for Rules {
    fn template_location(&self, _path: &str) -> bool {
        self.templates
    }
    fn project_folder(&self, path: &str) -> Option<PathBuf> {
        Path::new(path).parent().map(Path::to_path_buf)
    }
}

#[test]
fn the_describers_folding_phrases_pictures_and_project_rules_speak_the_daws_words() {
    let item = |name: &str, order: u64| ProjectItem { name: Some(json!(name)), order: Some(json!(order)), on: Some("Bass".into()) };
    let changes = vec![
        ProjectChange::Added { kind: "track".into(), item: item("Pads", 3), made_of: JsonObject::new() },
        ProjectChange::Removed { kind: "pattern".into(), item: item("Intro", 1) },
        ProjectChange::Changed {
            kind: "device".into(),
            before: Some(item("Fakeverb", 0)),
            after: Some(item("Fakeverb", 0)),
            renamed: false,
            details: vec![Detail {
                facet: "mixer".into(),
                what: "mixer.volume".into(),
                from: json!(0.5),
                to: json!(0.7),
                fingerprint: false,
            }],
            lines: None,
        },
        ProjectChange::Changed {
            kind: "song".into(),
            before: None,
            after: None,
            renamed: false,
            details: vec![],
            lines: Some(vec!["Tempo 120 → 124 BPM".into()]),
        },
        ProjectChange::Unclear { kind: "pattern".into(), before: vec![item("A", 1), item("B", 2)], after: vec![item("C", 1)] },
    ];
    let mut said = describe_changes(&changes, &WORDS, None).lines;
    assert!(!said.is_empty());
    said.extend(describe_watch_changes(&changes, &WORDS, None).changes.iter().map(|change| Value::Object(change.clone()).to_string()));

    let tracks: Vec<JsonObject> = (0..400)
        .map(|n| json!({"ref":format!("track:{n}"),"name":format!("Track {n} with a long name")}).as_object().unwrap().clone())
        .collect();
    let folded = fold_tracks_with(
        &tracks,
        |track| track["ref"] == json!("track:1"),
        Some(2_000),
        "Some tracks are folded.",
        |left| format!("{left} more tracks aren't listed; ask for them by page"),
    );
    said.extend(folded.folded.clone());
    said.extend(folded.more_tracks.clone());
    assert!(folded.more_tracks.is_some());

    let named = [PhraseTrack { reference: "1:track:0", name: "Bass", own: true, master: false }];
    for phrase in ["tempo 124", "mute bass", "loop 1-4", "metronome on", "track 1"] {
        let Some(intent) = parse(phrase) else { continue };
        if let Some(plan) = instant_steps(&Phrases, &intent, &named, 4.0) {
            said.push(plan.to_string());
        }
    }

    let read = [
        ParameterRow { name: "Threshold".into(), value: -18.0, min: -60.0, max: 0.0, text: Some("-18 dB".into()) },
        ParameterRow { name: "Ratio".into(), value: 4.0, min: 1.0, max: 20.0, text: Some("4:1".into()) },
    ];
    let moved = vec![PictureValue { name: "Threshold".into(), from: Some(-10.0), to: Some(-18.0), ..Default::default() }];
    let drawn = curves(&Curves, &read, &[("c1".into(), moved)]);
    assert_eq!(drawn.len(), 1);
    said.extend(drawn.iter().map(|(_, picture)| serde_json::to_string(picture).unwrap()));

    for templates in [true, false] {
        let (id, keep) = decide_project_with(&Rules { templates }, None, "/songs/Beat.fsong", None, true);
        assert_eq!((id.len(), keep), (32, !templates));
    }
    let found = leaks(&said);
    assert!(found.is_empty(), "{found:#?}");
}

/// Core's own texts that still name Live's things, each with why. Live's words themselves are spelled once, in
/// daw.rs' `live_word!`, and aren't listed.
const ALLOWED: &[(&str, &str, &str)] = &[
    ("session.rs", "Session is ", "Kumi's own session with the model, not the DAW's"),
    ("session.rs", "Session operation ended unexpectedly", "Kumi's own session with the model, not the DAW's"),
    ("session.rs", "Session already started", "Kumi's own session with the model, not the DAW's"),
    ("goal.rs", "Set the search up", "the verb"),
    ("goal.rs", "a Max for Live device via make_device", "only a DAW with Max for Live devices gets it"),
];

/// Core's string literals outside its unit tests, by file (`live_word!`'s left out), each with its line.
fn core_texts() -> Vec<(String, usize, String)> {
    static LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""((?:[^"\\]|\\.)*)""#).unwrap());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core");
    let mut texts = vec![];
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().path()).collect();
    files.sort();
    for file in files.into_iter().filter(|file| file.extension().is_some_and(|ext| ext == "rs")) {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&file).unwrap();
        let mut in_live_words = false;
        for (n, line) in text.lines().enumerate() {
            if line.trim() == "#[cfg(test)]" && text.lines().nth(n + 1).is_some_and(|next| next.trim_start().starts_with("mod ")) {
                break;
            }
            if line.starts_with("macro_rules! live_word") {
                in_live_words = true;
            } else if in_live_words && line == "}" {
                in_live_words = false;
                continue;
            }
            if in_live_words || line.trim_start().starts_with("//") {
                continue;
            }
            for found in LITERAL.captures_iter(line) {
                texts.push((name.clone(), n + 1, found[1].to_owned()));
            }
        }
    }
    texts
}

#[test]
fn cores_own_texts_name_none_of_lives_things_but_those_listed() {
    let found: Vec<String> = core_texts()
        .into_iter()
        .filter(|(file, _, text)| LIVE_WORDS.is_match(text) && !ALLOWED.iter().any(|(at, phrase, _)| file == at && text.contains(phrase)))
        .map(|(file, line, text)| format!("core/{file}:{line}: {text}"))
        .collect();
    assert!(
        found.is_empty(),
        "these say Live's words in core; take them from the DAW's profile, or list them with why:\n{}",
        found.join("\n")
    );
}

#[test]
fn each_allowed_text_is_still_there() {
    let texts = core_texts();
    for (file, phrase, why) in ALLOWED {
        assert!(
            texts.iter().any(|(at, _, text)| at == file && text.contains(phrase)),
            "core/{file} no longer says {phrase} ({why}): take it off the list"
        );
    }
}

/// The shared undo_change is Live's as it was: HISTORY's undo of the change named (or the last), never discarding,
/// whatever the input holds (a model can send a field the schema doesn't offer), and "Undone: …" for the producer when it
/// ends the answer. Only a DAW whose undo takes discard passes it, through undo_change_with.
#[tokio::test]
async fn the_shared_undo_change_never_discards_unless_its_daw_says_so() {
    use kumi_runtime::integrations::shared::undo::{undo_change, undo_change_with};
    let daw = FakeDaw::<false>::new(Answers { plan_steps: false, stops: false, reachable: true, batches: false, copies: false });
    let first = daw.record("tempo", "Tempo 120 → 124 BPM", "t-1", None);
    daw.record("tempo", "Tempo 124 → 126 BPM", "t-2", None);
    let input = |value: Value| value.as_object().unwrap().clone();

    let undone = undo_change(&daw.history, &input(json!({"change": "last", "final": true, "discard": true})), Signal::new()).await.unwrap();
    assert_eq!((undone.is_error, undone.reply.as_deref()), (false, Some("Undone: Tempo 124 → 126 BPM.")));
    assert_eq!(undone.text, json!({"undone": "Tempo 124 → 126 BPM", "change": id_of(&daw.history, "Tempo 124 → 126 BPM")}).to_string());
    let undone = undo_change_with(&daw.history, &input(json!({"change": first})), Signal::new(), true).await.unwrap();
    assert_eq!((undone.is_error, undone.reply), (false, None), "{}", undone.text);
    assert_eq!(*daw.history.host.discards.borrow(), [false, true]);
}
