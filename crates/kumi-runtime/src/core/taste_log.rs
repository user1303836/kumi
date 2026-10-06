//! What the producer does in answer to Kumi, kept in Kumi's database to learn their taste from later: their own words
//! about what Kumi did or what they like (tagged by the model, quietly, with `reaction`), a pick among the options an
//! answer ended on, yes or no to keeping a technique, and undoing one of Kumi's changes.
//!
//! Logging only: nothing here is read back into a prompt. Only real reactions are kept, never silence: not work
//! toward a goal or a match run, not Kumi's own clean-ups, not a change undone in the turn that made it, not a default
//! taken with Enter. Each row keeps its kind's default weight and every fact behind it, so a learner can weigh it
//! again. Each also says where it happened: the conversation, the Set's project and fingerprint, and the producer's
//! latest requests.
use super::{
    contracts::{ChangeRecord, JsonObject, KernelTool, Picked, ToolResult},
    errors::RuntimeError,
    memory::suspect_note,
    store_client::StoreClient,
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{abort::Signal, js::json::stringify, time::now_ms};
use kumi_store::observations::{self, Kind, Observation};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

pub const REACTION_TOOL: &str = "reaction";
/// An undo this soon after the change was first heard is a clear no.
const HEARD_UNDO_MS: i64 = 60_000;
/// Kumi's changes remembered for an undo of them.
const KEEP_CHANGES: usize = 2_000;
/// The producer's latest requests kept with each row.
const REQUESTS: usize = 3;
const REQUEST_CHARS: usize = 300;
const QUOTE_CHARS: usize = 200;
const DESCRIPTION: &str = concat!(
    "When the producer's message reacts to what you did, or says what they like or don't in their music (\"too bright\", ",
    "\"love that groove\", \"never put reverb on the kick\", \"more like Burial\"), note it here in the same reply, ",
    "quoting their own words, even when you also remember it. The producer doesn't see it, and it changes nothing now. ",
    "Not for a request with no opinion in it (\"add a kick\"), and never for your own judgement."
);
/// Kept in place of a request that reads as a secret (a pasted key, say): rows are kept for good.
const LEFT_OUT: &str = "(left out: it read as a secret)";
/// A request that asks for an undo, aimed at what Kumi did: "undo that", "revert c3", "take it back", in the languages
/// Kumi speaks. A change's id counts only inside such words ("tune the 808 to c1" asks for nothing).
static UNDO_ASKED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(undo|revert|take (it|that|this|them|c\d+) (back|out)|put (it|that|this|them|c\d+) back|scrap (it|that|this))\b|元に戻|取り消|撤销|撤消|还原",
    )
    .unwrap()
});

/// Where an observation happens, from the session: its conversation, the Set's project, and the Set's fingerprint.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Whereabouts {
    pub session: String,
    pub project: Option<String>,
    pub set: Option<Value>,
}

/// Who undid one of Kumi's changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoneBy {
    /// The producer, with Kumi's undo key or /undo.
    Producer,
    /// Kumi's undo tool, in a turn.
    Tool,
}

#[derive(Default)]
struct LogState {
    turn: u64,
    /// This turn is the producer's own request: not work toward a goal, nor a match run.
    attended: bool,
    /// This turn's request, as the quote check reads it.
    request: String,
    /// The producer's latest requests, oldest first.
    requests: VecDeque<String>,
    /// Kumi's changes by id, each with the turn it was made in.
    changes: IndexMap<String, (u64, ChangeRecord)>,
}

pub struct TasteLog {
    store: StoreClient,
    whereabouts: Rc<dyn Fn() -> Whereabouts>,
    /// When the producer first heard Live play at or after a time (ms).
    heard: Rc<dyn Fn(i64) -> Option<i64>>,
    now: Rc<dyn Fn() -> i64>,
    state: RefCell<LogState>,
}

impl TasteLog {
    pub fn new(store: StoreClient, whereabouts: Rc<dyn Fn() -> Whereabouts>, heard: Rc<dyn Fn(i64) -> Option<i64>>) -> Rc<TasteLog> {
        TasteLog::with_clock(store, whereabouts, heard, Rc::new(now_ms))
    }
    pub fn with_clock(
        store: StoreClient,
        whereabouts: Rc<dyn Fn() -> Whereabouts>,
        heard: Rc<dyn Fn(i64) -> Option<i64>>,
        now: Rc<dyn Fn() -> i64>,
    ) -> Rc<TasteLog> {
        Rc::new(TasteLog { store, whereabouts, heard, now, state: RefCell::new(LogState::default()) })
    }
    /// The quiet tool the model tags the producer's words with.
    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(ReactionTool { log: self.clone() })
    }
    /// Kumi's own undo tool, watched for the producer's undos it makes.
    pub fn watch_undo(self: &Rc<Self>, tool: Rc<dyn KernelTool>) -> Rc<dyn KernelTool> {
        Rc::new(UndoWatch { tool, log: self.clone() })
    }
    /// A turn begins: the producer's own request (`attended`), or work toward a goal or a match run's.
    pub fn turn_started(&self, request: &str, attended: bool) {
        let mut state = self.state.borrow_mut();
        state.turn += 1;
        state.attended = attended;
        state.request = if attended { request.to_owned() } else { String::new() };
        if attended {
            state.requests.push_back(kept_request(request));
            while state.requests.len() > REQUESTS {
                state.requests.pop_front();
            }
        }
    }
    /// The producer's words steering the turn under way: theirs to quote too.
    pub fn steered(&self, text: &str) {
        let mut state = self.state.borrow_mut();
        if !state.attended {
            return;
        }
        state.request = format!("{}\n{text}", state.request);
        let joined = state.requests.back().map(|last| format!("{last}\n{text}"));
        if let (Some(last), Some(joined)) = (state.requests.back_mut(), joined) {
            *last = if *last == LEFT_OUT { LEFT_OUT.to_owned() } else { kept_request(&joined) };
        }
    }
    /// One of Kumi's changes, as it is now: tied to the turn it was first seen in.
    pub fn change(&self, record: &ChangeRecord) {
        let mut state = self.state.borrow_mut();
        let turn = state.changes.get(&record.id).map(|(turn, _)| *turn).unwrap_or(state.turn);
        state.changes.insert(record.id.clone(), (turn, record.clone()));
        while state.changes.len() > KEEP_CHANGES {
            state.changes.shift_remove_index(0);
        }
    }
    /// One of Kumi's changes was undone. Kept when the producer did it: with Kumi's undo key or /undo, or by asking
    /// Kumi in a later turn of their own (not in the turn that made it, nor work toward a goal). It counts most when
    /// it came soon after they first heard the change.
    pub fn undone(&self, id: &str, by: UndoneBy) {
        let (record, asked) = {
            let state = self.state.borrow();
            let Some((turn, record)) = state.changes.get(id).cloned() else { return };
            if by == UndoneBy::Tool && (!state.attended || turn >= state.turn) {
                return;
            }
            // Kumi's undo tool on a turn whose request doesn't ask for one is the model's choice, not the producer's.
            let asked = by == UndoneBy::Producer || UNDO_ASKED.is_match(&state.request);
            (record, asked)
        };
        let now = (self.now)();
        let heard = (self.heard)(record.at);
        let since_heard = heard.map(|at| now - at);
        let weight = if !asked {
            None
        } else if since_heard.is_some_and(|ms| ms <= HEARD_UNDO_MS) {
            Some(-2.0)
        } else {
            Some(-0.5)
        };
        let mut facts = json!({
            "by":match (by, asked) {
                (UndoneBy::Producer, _) => "producer",
                (UndoneBy::Tool, true) => "asked",
                (UndoneBy::Tool, false) => "model",
            },
            "sinceChange":now - record.at,
            "sinceHeard":since_heard,
        });
        for (key, value) in [("from", record.from), ("to", record.to)] {
            if let Some(value) = value {
                facts[key] = json!(value);
            }
        }
        if let Some(range) = record.range {
            facts["range"] = json!(range);
        }
        self.log(Kind::Undo, weight, Some(heard.is_some()), subject(&record), facts);
    }
    /// The producer's own words, tagged by the model. Err is why it wasn't kept, for the model.
    pub fn reaction(&self, input: &JsonObject) -> Result<(), String> {
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(|text| text.trim().to_owned()).filter(|text| !text.is_empty());
        let (attended, request) = {
            let state = self.state.borrow();
            (state.attended, state.request.clone())
        };
        if !attended {
            return Err("Only the producer's own messages are noted: work toward a goal or a match has none.".into());
        }
        let quote: String = text("quote").unwrap_or_default().chars().take(QUOTE_CHARS).collect();
        if plain(&quote).chars().count() < 2 || !plain(&request).contains(&plain(&quote)) {
            return Err("Quote the producer's own words from this message, exactly.".into());
        }
        if [quote.as_str(), text("about").as_deref().unwrap_or(""), text("reference").as_deref().unwrap_or("")]
            .into_iter()
            .any(suspect_note)
        {
            return Err("That reads as instructions or a secret, not a reaction to the music, so it wasn't noted.".into());
        }
        let lean = text("lean").unwrap_or_default();
        let weight = match lean.as_str() {
            "like" | "more" => Some(3.0),
            "dislike" | "less" => Some(-3.0),
            "like_reference" => Some(2.0),
            "never" | "always" => None,
            _ => return Err("Give its lean: like, dislike, more, less, never, always or like_reference.".into()),
        };
        let mut subject = json!({});
        for key in ["about", "reference"] {
            if let Some(value) = text(key) {
                subject[key] = json!(value.chars().take(80).collect::<String>());
            }
        }
        let mut heard = None;
        if let Some(change) = text("change") {
            let found = self.state.borrow().changes.get(&change).map(|(_, record)| record.clone());
            if let Some(record) = found {
                heard = Some((self.heard)(record.at).is_some());
                subject["change"] = json!(change);
                for (key, value) in self::subject(&record).as_object().into_iter().flatten() {
                    subject[key.as_str()] = value.clone();
                }
            }
        }
        self.log(Kind::Words, weight, heard, subject, json!({"quote":quote,"lean":lean}));
        Ok(())
    }
    /// The producer picked one of Kumi's options with a key.
    pub fn picked(&self, pick: &Picked) {
        match pick {
            Picked::Answer { question, options, index } => {
                let facts = json!({"picked":index + 1,"of":options.len(),"option":options.get(*index)});
                self.log(Kind::Pick, Some(1.0), None, json!({"question":question,"options":options}), facts);
            }
            Picked::Technique { name, keep } => {
                let request = self.state.borrow().requests.back().cloned();
                let weight = if *keep { 1.0 } else { -0.5 };
                self.log(Kind::TechniqueOffer, Some(weight), None, json!({"technique":name,"request":request}), json!({"keep":keep}));
            }
        }
    }
    fn log(&self, kind: Kind, weight: Option<f64>, heard: Option<bool>, subject: Value, facts: Value) {
        let place = (self.whereabouts)();
        let mut observation = Observation::new((self.now)(), &place.session, place.project, kind);
        observation.weight = weight;
        observation.heard = heard;
        observation.subject = subject;
        observation.facts = facts;
        observation.context = json!({"set":place.set,"requests":self.state.borrow().requests});
        // Best effort: a row that can't be written is a reaction not learned from, nothing more.
        self.store.store().write(move |connection| observations::append(connection, &observation), |_| {});
    }
}

/// A request as rows keep it: its first characters, or a mark in place of one that reads as a secret.
fn kept_request(request: &str) -> String {
    if suspect_note(request) {
        return LEFT_OUT.to_owned();
    }
    request.chars().take(REQUEST_CHARS).collect()
}

/// What a change was, as a row says it.
fn subject(record: &ChangeRecord) -> Value {
    let mut subject = json!({"change":record.id,"family":record.family,"title":record.title});
    if let Some(track) = &record.track {
        subject["track"] = json!(track.name);
    }
    subject
}

/// Text as the quote check compares it: lower case, one kind of quote mark, spaces collapsed, no quotes around it.
fn plain(text: &str) -> String {
    let unified: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201b}' | '`' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{00ab}' | '\u{00bb}' => '"',
            c => c,
        })
        .collect();
    unified.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c: char| c == '"' || c == '\'').trim().to_owned()
}

struct ReactionTool {
    log: Rc<TasteLog>,
}
#[async_trait(?Send)]
impl KernelTool for ReactionTool {
    fn name(&self) -> &str {
        REACTION_TOOL
    }
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["quote","lean"],"properties":{
            "quote":{"type":"string","minLength":2,"maxLength":QUOTE_CHARS,"description":"The producer's own words from this message, exactly as they wrote them"},
            "lean":{"type":"string","enum":["like","dislike","more","less","never","always","like_reference"],
                "description":"like or dislike what it's about; more or less of it; never or always (a rule they state); like_reference: they want it more like an artist, track or sound they name"},
            "about":{"type":"string","maxLength":80,"description":"What it's about, in a few words: \"the drum reverb\", \"the pad's brightness\""},
            "change":{"type":"string","pattern":"^c\\d{1,6}$","description":"The change of yours it reacts to, when it's one (such as c3)"},
            "reference":{"type":"string","maxLength":80,"description":"The artist, track or sound they name"}
        }})
        .as_object()
        .unwrap()
        .clone()
    }
    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.log.reaction(&input) {
            // Not quiet yet: a quiet call ends the answer, and words beside it ("Making it darker now.") would end
            // the turn before its change. It gets `final` when quiet calls take one.
            Ok(()) => ToolResult { text: stringify(&json!({"noted":true})), ..Default::default() },
            Err(why) => ToolResult::error(why),
        })
    }
}

/// Kumi's undo tool, as it was, with the change it undid passed on as the producer's undo.
struct UndoWatch {
    tool: Rc<dyn KernelTool>,
    log: Rc<TasteLog>,
}
#[async_trait(?Send)]
impl KernelTool for UndoWatch {
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn input_schema(&self) -> JsonObject {
        self.tool.input_schema()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let result = self.tool.execute(input, signal).await?;
        if !result.is_error {
            let undone: Value = serde_json::from_str(&result.text).unwrap_or(Value::Null);
            if let (Some(change), None) = (undone["change"].as_str(), undone.get("already")) {
                self.log.undone(change, UndoneBy::Tool);
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::{ChangeFamily, ChangeState, TrackChip};
    use std::cell::Cell;

    fn record(id: &str, at: i64) -> ChangeRecord {
        ChangeRecord {
            id: id.into(),
            family: ChangeFamily::Parameter,
            title: "Reverb Dry/Wet 20 → 45 %".into(),
            track: Some(TrackChip { name: "Drums".into(), color: None }),
            from: Some(0.2),
            to: Some(0.45),
            range: Some([0.0, 1.0]),
            colors: None,
            clip: None,
            devices: None,
            state: ChangeState::Applied,
            score: None,
            note: None,
            at,
        }
    }
    struct Fixture {
        log: Rc<TasteLog>,
        store: StoreClient,
        clock: Rc<Cell<i64>>,
        _folder: tempfile::TempDir,
    }
    fn fixture(heard_at: Option<i64>) -> Fixture {
        let folder = tempfile::tempdir().unwrap();
        let store = StoreClient::new(kumi_store::Store::open(folder.path().join("kumi.db")).unwrap());
        let clock = Rc::new(Cell::new(1_000_000));
        let now = clock.clone();
        let log = TasteLog::with_clock(
            store.clone(),
            Rc::new(|| Whereabouts {
                session: "20261006-120000-abcd".into(),
                project: Some("0123456789abcdef0123456789abcdef".into()),
                set: Some(json!({"tempo":172.0})),
            }),
            Rc::new(move |since| heard_at.filter(|at| *at >= since)),
            Rc::new(move || now.get()),
        );
        Fixture { log, store, clock, _folder: folder }
    }
    impl Fixture {
        fn rows(&self) -> Vec<Observation> {
            // A write is queued: wait for it with one of its own.
            self.store.store().write_wait(|_| Ok(())).unwrap();
            let mut rows = self.store.store().read(|c| observations::recent(c, 100)).unwrap();
            rows.reverse();
            rows
        }
    }

    #[test]
    fn the_producers_words_are_kept_only_as_they_wrote_them() {
        let f = fixture(None);
        f.log.turn_started("Hmm, that's “too bright” — never put reverb on the kick", true);
        let input = |quote: &str, lean: &str| json!({"quote":quote,"lean":lean,"about":"the kick's reverb"}).as_object().unwrap().clone();
        assert!(f.log.reaction(&input("never put reverb on the kick", "never")).is_ok());
        // Quotes and case as the model may write them.
        assert!(f.log.reaction(&input("\"Too bright\"", "less")).is_ok());
        assert!(f.log.reaction(&input("the drums sound great", "like")).is_err(), "not the producer's words");
        assert!(f.log.reaction(&input("never put reverb on the kick", "maybe")).is_err());
        let rows = f.rows();
        assert_eq!(rows.iter().map(|row| (row.kind, row.weight)).collect::<Vec<_>>(), [(Kind::Words, None), (Kind::Words, Some(-3.0))]);
        assert_eq!(rows[0].facts, json!({"quote":"never put reverb on the kick","lean":"never"}));
        assert_eq!(rows[0].project.as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(rows[0].context, json!({"set":{"tempo":172.0},"requests":["Hmm, that's “too bright” — never put reverb on the kick"]}));
        // Work toward a goal has no producer's words.
        f.log.turn_started("", false);
        assert!(f.log.reaction(&input("never put reverb on the kick", "never")).is_err());
        assert_eq!(f.rows().len(), 2);
    }

    #[test]
    fn an_undo_counts_when_the_producer_made_it_and_most_soon_after_they_heard_the_change() {
        // Heard 10 s after the change.
        let f = fixture(Some(1_010_000));
        f.log.turn_started("wetter drums", true);
        f.log.change(&record("c1", 1_000_000));
        // Kumi taking its own change back in the turn that made it isn't the producer's.
        f.log.undone("c1", UndoneBy::Tool);
        assert!(f.rows().is_empty());
        // Asked for in their next turn, 30 s after hearing it: a clear no.
        f.log.turn_started("no, undo that", true);
        f.clock.set(1_040_000);
        f.log.undone("c1", UndoneBy::Tool);
        // With the undo key, never heard: counted, less.
        f.log.change(&record("c2", 1_050_000));
        f.clock.set(1_500_000);
        f.log.undone("c2", UndoneBy::Producer);
        let rows = f.rows();
        assert_eq!(
            rows.iter().map(|row| (row.kind, row.weight, row.heard)).collect::<Vec<_>>(),
            [(Kind::Undo, Some(-2.0), Some(true)), (Kind::Undo, Some(-0.5), Some(false)),]
        );
        assert_eq!(rows[0].facts, json!({"by":"asked","sinceChange":40000,"sinceHeard":30000,"from":0.2,"to":0.45,"range":[0.0,1.0]}));
        assert_eq!(rows[0].subject, json!({"change":"c1","family":"parameter","title":"Reverb Dry/Wet 20 → 45 %","track":"Drums"}));
        // Work toward a goal undoing an earlier change isn't the producer's either.
        f.log.change(&record("c3", 1_600_000));
        f.log.turn_started("", false);
        f.log.undone("c3", UndoneBy::Tool);
        assert_eq!(f.rows().len(), 2);
    }

    #[test]
    fn picks_are_kept_with_their_options_and_a_technique_offer_with_its_answer() {
        let f = fixture(None);
        f.log.turn_started("make a pad", true);
        f.log.picked(&Picked::Answer { question: "Which pad?".into(), options: vec!["Warm".into(), "Glassy".into()], index: 1 });
        f.log.picked(&Picked::Technique { name: "Shimmer pad".into(), keep: false });
        let rows = f.rows();
        assert_eq!(
            rows.iter().map(|row| (row.kind, row.weight)).collect::<Vec<_>>(),
            [(Kind::Pick, Some(1.0)), (Kind::TechniqueOffer, Some(-0.5))]
        );
        assert_eq!(rows[0].facts, json!({"picked":2,"of":2,"option":"Glassy"}));
        assert_eq!(rows[1].subject, json!({"technique":"Shimmer pad","request":"make a pad"}));
    }

    #[test]
    fn a_request_that_reads_as_a_secret_is_left_out_of_every_row() {
        let f = fixture(None);
        f.log.turn_started("use my key api_key=sk-test-0123456789abcdefghijklmnopqrstuv", true);
        f.log.picked(&Picked::Answer { question: "Which?".into(), options: vec!["A".into(), "B".into()], index: 0 });
        assert_eq!(f.rows()[0].context["requests"], json!([LEFT_OUT]));
    }

    #[test]
    fn words_steering_a_turn_are_the_producers_to_quote() {
        let f = fixture(None);
        f.log.turn_started("make the pad brighter", true);
        f.log.steered("too bright!");
        let input = json!({"quote":"too bright","lean":"less"}).as_object().unwrap().clone();
        assert!(f.log.reaction(&input).is_ok());
        assert_eq!(f.rows()[0].context["requests"], json!(["make the pad brighter\ntoo bright!"]));
    }

    #[test]
    fn an_undo_the_model_chose_is_kept_without_weight() {
        let f = fixture(Some(1_010_000));
        f.log.turn_started("add a test clip", true);
        f.log.change(&record("c1", 1_000_000));
        // The next request doesn't ask for an undo: the model took the change back on its own.
        f.log.turn_started("now make the drums swing", true);
        f.log.undone("c1", UndoneBy::Tool);
        // One that asks for it does: heard at 1 010 000, undone 20 s later.
        f.log.change(&record("c2", 1_005_000));
        f.log.turn_started("take c2 out", true);
        f.clock.set(1_030_000);
        f.log.undone("c2", UndoneBy::Tool);
        // A note name, or going back to a part of the song, isn't asking for an undo.
        for (id, request) in [("c3", "tune the 808 to c1"), ("c4", "go back to the verse")] {
            f.log.change(&record(id, 1_031_000));
            f.log.turn_started(request, true);
            f.log.undone(id, UndoneBy::Tool);
        }
        let rows = f.rows();
        assert_eq!(
            rows.iter().map(|row| (row.facts["by"].clone(), row.weight)).collect::<Vec<_>>(),
            [(json!("model"), None), (json!("asked"), Some(-2.0)), (json!("model"), None), (json!("model"), None)]
        );
    }
}
