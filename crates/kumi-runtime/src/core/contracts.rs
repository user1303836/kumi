use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::errors::{FailureKind, RuntimeError};
use super::evolve::Knob;
use super::goal::GoalStatus;
use super::match_run::MatchStatus;
use crate::audio::matching::{Closeness, Focus};

pub type JsonObject = serde_json::Map<String, Value>;

/// `reply`, from a tool that finished what the producer asked, is the answer: when every call in
/// the step succeeded and no guidance is waiting, the turn ends there without another model call.
/// An empty reply is quiet (a note kept): when every call in the step is, and says `final` (see
/// `with_final`), the turn ends as is.
/// `images` go to the model after the text, each after its caption, for the rest of the turn.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImage>,
}

/// `schema` with `final`, for a tool whose result is quiet (a note kept, a recipe saved). A reply of
/// the model's words and nothing but such calls ends the answer only when they say `final: true`;
/// otherwise the model carries on, since its words may be a step on the way ("removing the test clip
/// now") rather than its answer (#180).
pub fn with_final(mut schema: JsonObject) -> JsonObject {
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties.insert(
            "final".into(),
            serde_json::json!({"type":"boolean","description":"true when this goes with your finished answer, in the same reply: the answer ends there. Leave it out while there's more to do, and you're called again."}),
        );
    }
    schema
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true, ..Self::default() }
    }
}

/// A picture a tool shows the model (a video's frame, say): the model sees it until the turn ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolImage {
    pub data: Vec<u8>,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// A tool the kernel offers the model. `stream` is optional: it returns None for a tool whose calls
/// can't start before they're whole.
#[async_trait(?Send)]
pub trait KernelTool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> JsonObject;
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError>;
    /// Start while the model is still writing the call: a plan's first steps run as its later ones are
    /// written. Offered for a model reply's first call only, since calls run in order. `on_start` says
    /// the work has begun (the call then shows as running).
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        let _ = (signal, on_start);
        None
    }
}

/// A tool call under way while its input streams in.
#[async_trait(?Send)]
pub trait StreamingCall {
    /// The next piece of the input, as the model writes it.
    fn push(&self, delta: &str);
    /// The whole input (None when it isn't a JSON object): settles as `execute` would.
    async fn finish(&self, input: Option<JsonObject>) -> Result<ToolResult, RuntimeError>;
    /// The model's reply broke off: start nothing more; settles once the work under way has.
    async fn abandon(&self);
    /// Some of the work has begun, so the reply can't be asked for again.
    fn started(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum KernelEvent {
    Text {
        text: String,
    },
    /// The model began writing a call (a plan, say); "tool-start" follows once it runs.
    ToolInput {
        id: String,
        name: String,
    },
    ToolStart {
        id: String,
        name: String,
    },
    ToolEnd {
        id: String,
        name: String,
        is_error: bool,
        elapsed_ms: u64,
    },
    /// Guidance accepted mid-turn; it entered the conversation at a model boundary.
    Steer {
        text: String,
    },
    /// A model call failed in a way worth trying again: why, and how long Kumi waits first (the turn can
    /// still be stopped meanwhile).
    Retry {
        reason: String,
        wait_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// All prompt tokens, including cache reads and writes.
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_read_tokens: f64,
    pub cache_write_tokens: f64,
}

/// "max-steps": the model kept calling tools past the per-turn step bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    Completed,
    Cancelled,
    MaxSteps,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnResult {
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// A kernel's settled conversation as plain JSON; opaque outside the kernel that made it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelCheckpoint {
    pub version: u32,
    pub messages: Vec<Value>,
    /// The model that wrote it ("<provider>/<model>"; a provider alone in older saves). Reasoning is
    /// the writer's own: another model continues from a portable copy (words, tool calls, results).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The instructions and tools it was made with, as a fingerprint: some models' reasoning is bound to them too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRole {
    User,
    Assistant,
}

/// A line of a conversation as the producer saw it; an answer's line names the tools it called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptLine {
    pub role: TranscriptRole,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

/// `(event: KernelEvent) => void`: a turn's listener. One that fails (threw, in the TypeScript) ends the turn.
pub type KernelEmit = Rc<dyn Fn(KernelEvent) -> Result<(), RuntimeError>>;
/// `(text: string) => void`: words as they stream.
pub type OnText = Rc<dyn Fn(&str)>;

fn absent() -> RuntimeError {
    RuntimeError::plain("Not available")
}

/// Optional methods (`checkpoint?()` and so on) have a `has_…` flag, false by default, that says
/// whether this kernel offers them.
#[async_trait(?Send)]
pub trait Kernel {
    async fn run(&self, input: &str, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError>;
    /// `run` with pictures the model sees beside the words.
    async fn run_with(&self, input: &str, pictures: Vec<Picture>, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError> {
        let _ = pictures;
        self.run(input, signal, emit).await
    }
    async fn close(&self);
    fn has_checkpoint(&self) -> bool {
        false
    }
    /// The settled conversation, so a kernel with other tools can carry it on.
    fn checkpoint(&self) -> Result<KernelCheckpoint, RuntimeError> {
        Err(absent())
    }
    fn has_transcript(&self) -> bool {
        false
    }
    /// The settled conversation's words, for showing a resumed conversation.
    fn transcript(&self) -> Vec<TranscriptLine> {
        Vec::new()
    }
    fn has_steer(&self) -> bool {
        false
    }
    /// Guidance for the turn under way, entering at its next step; false when no turn can take it.
    fn steer(&self, text: &str) -> bool {
        let _ = text;
        false
    }
    fn has_aside(&self) -> bool {
        false
    }
    /// A side question about the conversation so far, answered without tools and never kept in it.
    async fn aside(&self, question: &str, signal: Signal, on_text: OnText) -> Result<String, RuntimeError> {
        let _ = (question, signal, on_text);
        Err(absent())
    }
}

#[derive(Clone)]
pub struct KernelOptions {
    pub instructions: String,
    pub tools: Vec<Rc<dyn KernelTool>>,
    pub signal: Signal,
    /// Continue this conversation instead of starting empty.
    pub checkpoint: Option<KernelCheckpoint>,
    /// The conversation's own id, which stays the same across restarts: the provider keeps its prompt
    /// cache for it.
    pub conversation: Option<String>,
}

pub type KernelFactory = Rc<dyn Fn(KernelOptions) -> LocalBoxFuture<'static, Result<Rc<dyn Kernel>, RuntimeError>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// Why Live is disconnected: it went away (closed or crashed), Kumi's bridge to it dropped, or Live is
/// still running and opening another Set, which Kumi asked for (`AskedSet`) or not (`Set`). A Set
/// opening reloads Live's Remote Script, so Kumi loses Live for a moment however it happens (#188).
/// `Busy`: Live is running and its Remote Script still listening, only too slow to answer (a big Set's
/// changes, Live's own undo of a big step, a job left running in it): the request carries on once it
/// answers (#256).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisconnectCause {
    Live,
    Bridge,
    Set,
    AskedSet,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnState {
    Idle,
    Running,
    Cancelling,
    Closed,
}

/// The saved Set an observation is about (an opaque id), so its conversation can be kept between sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone)]
pub struct Observation {
    /// What the conversation is about (the open Set in this Live session). A new key starts a new conversation. Not a durable project ID.
    pub key: String,
    /// Changes that keep the conversation, such as the tool catalog: the kernel is rebuilt with the same history.
    pub revision: Option<String>,
    pub label: String,
    pub context: String,
    pub instructions: String,
    pub tools: Vec<Rc<dyn KernelTool>>,
    /// The saved Set this is about (an opaque id), so its conversation can be kept between sessions.
    pub project: Option<ProjectRef>,
    /// The Set's track names, when all of them were read (names are data).
    pub tracks: Option<Vec<String>>,
    /// When the saved Set's file was last written: a later time means the producer saved it.
    pub saved_at: Option<f64>,
}

/// A conversation, kept between sessions: `changes` are what Kumi changed during it (for HISTORY),
/// `first` the producer's first request and `turns` how many they made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConversation {
    pub saved_at: i64,
    pub checkpoint: KernelCheckpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<ChangeRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
}

/// A kept conversation, as /conversations lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub saved_at: i64,
    pub first: String,
    pub turns: u32,
    pub current: bool,
}

/// The conversation a place carries on with: its id and the conversation itself.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentConversation {
    pub id: String,
    pub conversation: SavedConversation,
}

/// Each Set's conversations. `place` is a saved Set's project id, or "unsaved" for Sets without a
/// file (their conversations move to the Set's own place when it's first saved). The one saved last
/// is the place's current conversation, until the place starts afresh.
#[async_trait(?Send)]
pub trait ConversationStore {
    /// The conversation `place` carries on with, if any.
    async fn current(&self, place: &str) -> Result<Option<CurrentConversation>, RuntimeError>;
    async fn load(&self, place: &str, id: &str) -> Result<Option<SavedConversation>, RuntimeError>;
    /// Keep `id` as `place`'s current conversation; each place keeps its latest 20.
    async fn save(&self, place: &str, id: &str, conversation: &SavedConversation) -> Result<(), RuntimeError>;
    /// `place` starts afresh; its conversations stay listed.
    async fn fresh(&self, place: &str) -> Result<(), RuntimeError>;
    /// `place`'s conversations, newest first.
    async fn list(&self, place: &str) -> Result<Vec<ConversationSummary>, RuntimeError>;
    /// A conversation moves with its Set (an unsaved Set's, when the Set is first saved).
    async fn move_conversation(&self, id: &str, from: &str, to: &str) -> Result<(), RuntimeError>;
    /// Exchanges in every place's kept conversations that hold at least `needed` of `words`
    /// (lowercase), most words first, then newest; at most `limit`. `skip` (place, id) is left out:
    /// the conversation going on now.
    async fn search(
        &self,
        words: &[String],
        needed: usize,
        limit: usize,
        skip: Option<(&str, &str)>,
    ) -> Result<Vec<FoundExchange>, RuntimeError> {
        let _ = (words, needed, limit, skip);
        Ok(vec![])
    }
}

/// An earlier exchange a search found: where and when, what the producer said, and Kumi's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundExchange {
    pub place: String,
    /// The Set's name, when Kumi has seen it saved.
    pub set: Option<String>,
    pub conversation: String,
    pub saved_at: i64,
    pub said: String,
    pub answer: String,
    /// The tools Kumi used in its answer, in order.
    pub tools: Vec<String>,
    /// How many of the searched words it holds.
    pub matched: usize,
}

/// What `observe` is told besides the signal.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObserveHints {
    /// What the producer pointed at; it's checked against Live and given to the model.
    pub pinned: Option<PinnedNode>,
    /// The same answer goes on (a match run's next round): its counts carry on.
    pub continuing: Option<bool>,
}

/// Optional methods (`undo?()` and so on) have a `has_…` flag, false by default.
#[async_trait(?Send)]
pub trait Integration {
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError>;
    /// What's in Live now; `pinned` (what the producer pointed at) is checked against it and given to the model.
    async fn observe(&self, signal: Signal, hints: Option<ObserveHints>) -> Result<Observation, RuntimeError>;
    async fn close(&self) -> Result<(), RuntimeError>;
    fn has_undo(&self) -> bool {
        false
    }
    /// Undo one of Kumi's changes (the latest undoable one when `id` is omitted).
    async fn undo(&self, id: Option<&str>, signal: Signal) -> Result<ChangeRecord, RuntimeError> {
        let _ = (id, signal);
        Err(absent())
    }
    /// The open Set's fingerprint, for what Kumi logs of the producer's reactions: its name, tempo, meter and scale,
    /// the roles its tracks play, its devices and the words in its track names. None before Kumi has read the Set.
    fn fingerprint(&self) -> Option<Value> {
        None
    }
    /// The first time at or after `since` (ms) that Live was heard playing (the producer's playing, not Kumi's own
    /// silent renders), for whether they heard one of Kumi's changes. None when it hasn't played since.
    fn first_heard(&self, since: i64) -> Option<i64> {
        let _ = since;
        None
    }
    fn has_audio_file(&self) -> bool {
        false
    }
    /// The audio file behind something in the Set the model names (a clip, say); None when it isn't one.
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let _ = (named, signal);
        Ok(None)
    }
    fn has_stop_live(&self) -> bool {
        false
    }
    /// Stop clips, the transport and recording in Live at once; true when Live is stopped afterwards.
    async fn stop_live(&self, signal: Signal) -> Result<bool, RuntimeError> {
        let _ = signal;
        Ok(false)
    }
    fn has_device_tree(&self) -> bool {
        false
    }
    /// A track's devices, racks' chains and what's in them, for FOCUS; None when Live can't say.
    async fn device_tree(&self, track_ref: &str, signal: Signal) -> Result<Option<DeviceTree>, RuntimeError> {
        let _ = (track_ref, signal);
        Ok(None)
    }
    fn has_clip_view(&self) -> bool {
        false
    }
    /// The MIDI clip in a Session slot, its notes and which are selected, for FOCUS; None for none (or audio).
    async fn clip_view(&self, slot_ref: &str, signal: Signal) -> Result<Option<ClipView>, RuntimeError> {
        let _ = (slot_ref, signal);
        Ok(None)
    }
    fn has_session_strip(&self) -> bool {
        false
    }
    /// A track's Session slots around a scene, for FOCUS.
    async fn session_strip(&self, track_ref: &str, scene: f64, signal: Signal) -> Result<Option<SessionStrip>, RuntimeError> {
        let _ = (track_ref, scene, signal);
        Ok(None)
    }
    fn has_arrangement_strip(&self) -> bool {
        false
    }
    /// The Arrangement at a glance (length, playhead, loop, locators), for FOCUS.
    async fn arrangement_strip(&self, signal: Signal) -> Result<Option<ArrangementStrip>, RuntimeError> {
        let _ = signal;
        Ok(None)
    }
    fn has_audition(&self) -> bool {
        false
    }
    /// Render candidates quietly, hear them and set them against a reference (the audition tool's work); `Err(why)` inside says why not.
    async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let _ = (request, signal);
        Err(absent())
    }
    fn has_goal(&self) -> bool {
        false
    }
    /// A goal's render rig over these candidates (kept open across generations), or why not.
    async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        let _ = (request, signal);
        Err(absent())
    }
    fn has_hear(&self) -> bool {
        false
    }
    /// Hear tracks (or the mix) in the Set directly: as they play now, or quietly over a stretch of the Arrangement; a file each, or why not.
    async fn hear(&self, request: &HearRequest, signal: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let _ = (request, signal);
        Err(absent())
    }
}

/// What to hear in the Set: tracks by reference or name (or the whole mix), and when.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HearRequest {
    pub tracks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<bool>,
    /// A stretch of the Arrangement to play quietly, in beats; left out, what's playing now (or, stopped, the loop or the playhead's part).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_beat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats: Option<f64>,
    /// How long to hear what's playing now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// The whole song, from the Arrangement's start to its end, quietly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whole: Option<bool>,
}

/// One thing heard: its name, its file, where the part starts in it, and whether it was heard as it played.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeardTake {
    pub label: String,
    pub file: String,
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    pub live: bool,
    /// What went short of the request (Live stopped before the part's end, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A goal's candidate chain in Live: its track, what it is, and the knobs a search may move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalSlotInfo {
    pub name: String,
    pub label: String,
    pub chain: String,
    pub knobs: Vec<Knob>,
}

/// One trial of a generation: a slot's knobs set to these values.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerationTrial {
    pub slot: String,
    pub knobs: Vec<Knob>,
    pub values: Vec<f64>,
    /// Hear it again, not from the cache.
    pub fresh: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenerationOptions {
    pub screen: Option<bool>,
}

/// A gap no knob closes, and the structural change that closes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralMove {
    pub gap: String,
    pub r#move: String,
}

/// What a generation's render says of each trial.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Generation {
    pub scores: HashMap<String, f64>,
    pub gaps: HashMap<String, Vec<String>>,
    pub silent: Vec<String>,
    pub frozen: HashMap<String, HashSet<String>>,
    /// Each slot's gap no knob closes, when it has one.
    pub structural: HashMap<String, StructuralMove>,
    /// Rendered on the short window, and how many trials were heard from the cache.
    pub screened: bool,
    pub cached: usize,
}

/// A render rig a goal keeps open: its slots, a generation rendered and scored in one pass, the best kept, and closing.
#[async_trait(?Send)]
pub trait GoalRig {
    fn slots(&self) -> Vec<GoalSlotInfo>;
    /// The part is long enough to screen: early generations can render a short, characteristic window of it.
    fn screens(&self) -> bool;
    /// A candidate the model built mid-search joins (with a safety limiter at the end of its chain).
    async fn add(&self, candidate: &AuditionCandidate, signal: Signal) -> Result<Result<GoalSlotInfo, String>, RuntimeError>;
    /// Each trial's values set on its slot, all rendered in one silent pass, each scored against the reference.
    async fn generation(
        &self,
        trials: &[GenerationTrial],
        signal: Signal,
        options: Option<GenerationOptions>,
    ) -> Result<Generation, RuntimeError>;
    /// The best so far on a track of its own ("Kumi · Goal best"), replacing the last copy; its name, or why not.
    async fn keep_best(&self, slot: &str, knobs: &[Knob], values: &[f64], signal: Signal) -> Result<String, RuntimeError>;
    /// A slot left where it is at these values, its safety limiter's input back at 0 dB (a match's winner, tuned); why not, if it couldn't be.
    async fn settle(&self, slot: &str, knobs: &[Knob], values: &[f64], signal: Signal) -> Result<Option<String>, RuntimeError>;
    /// A finished goal's candidates: the top ones muted for the producer to A/B, the rest removed; what it did.
    async fn tidy(&self, top: &[String], signal: Signal) -> Result<Vec<String>, RuntimeError>;
    /// The rig's scratch tracks go and the transport comes back; anything the producer should know.
    async fn close(&self) -> Result<Vec<String>, RuntimeError>;
}

/// An audition: the candidate tracks, where the part is, and what to match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditionCandidate {
    pub track: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The whole mix (what Main plays, recorded through Resampling) rather than a track; `track` is then MIX_CANDIDATE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<bool>,
}

/// The track name a mix candidate goes by (no track of the Set is a candidate then).
pub const MIX_CANDIDATE: &str = "the whole mix";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditionRequest {
    pub candidates: Vec<AuditionCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_beat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Focus>,
}

/// Where a take is in names that last from turn to turn: its track's name, and its clip ("scene:2") when it played one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Where {
    pub track: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
}

/// A render's file, and where the part starts in it (seconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Render {
    pub file: String,
    pub start: f64,
    /// How long the part heard is, in seconds, when Live stopped short of what was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}

/// What a take sounded like: its loudness (null when it couldn't be measured) and a summary line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub lufs: Option<f64>,
    pub summary: String,
}

/// One candidate's render, heard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditionTake {
    pub label: String,
    pub track: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<Where>,
    /// The render was (nearly) silent: nothing to compare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closeness: Option<Closeness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<Render>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<Heard>,
}

/// The reference as heard: its file and a summary line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceHeard {
    pub file: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditionResult {
    pub takes: Vec<AuditionTake>,
    /// The best take's label, when anything was compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ReferenceHeard>,
    /// How long the render and listening took.
    pub seconds: f64,
    /// Anything the producer should know (Main couldn't be put back, a scratch track stayed).
    pub notes: Vec<String>,
}

/// The best take of an audition: its label and score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BestTake {
    pub label: String,
    pub score: f64,
}

/// A candidate's score in an audition event; silent ones say so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeScore {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<Where>,
}

/// One audition, for the conversation: the round, the best score and the one before, and what differs most (a `{ type: "auditioned" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditionEvent {
    pub round: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<BestTake>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<f64>,
    /// Each candidate's score, best first; silent ones say so.
    pub takes: Vec<TakeScore>,
    pub gaps: Vec<String>,
    /// What was auditioned, so a match run can audition it again after changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<AuditionRequest>,
    /// The best's gap no knob closes, and the structural change that closes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural: Option<StructuralMove>,
    /// The reference as heard ("C2 · bright, rich · attack 15 ms"), for what a lesson says was matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// A clip in a Session slot: its name, and whether it's audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotClip {
    pub name: String,
    pub audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSlot {
    pub index: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<SlotClip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<bool>,
}

/// A track's Session slots around the selected scene: what's in each, and what's playing or queued. Names are data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStrip {
    pub track_ref: String,
    pub scene: f64,
    pub slots: Vec<SessionSlot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrangementLoop {
    pub start: f64,
    pub length: f64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    pub name: String,
    pub position: f64,
}

/// The Arrangement at a glance, in beats. Names are data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrangementStrip {
    pub length: f64,
    pub position: f64,
    pub playing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#loop: Option<ArrangementLoop>,
    pub locators: Vec<Locator>,
}

/// A note of a clip as FOCUS draws it, selected ones marked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipViewNote {
    #[serde(flatten)]
    pub note: ClipNote,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
}

/// A MIDI clip as FOCUS draws it: its length in beats, and its notes (the first 512), selected ones marked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipView {
    pub slot_ref: String,
    pub name: String,
    pub length: f64,
    pub notes: Vec<ClipViewNote>,
}

/// Live's device type, when the bridge sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Instrument,
    AudioEffect,
    MidiEffect,
}

/// A device in a track's tree: what the bridge says it is, and a rack's chains. Names are data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceNode {
    pub r#ref: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_have_chains: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_have_drum_pads: Option<bool>,
    /// Live's device type, when the bridge sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<DeviceType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<ChainNode>>,
}

/// A rack's chain (a Drum Rack's, a pad's), and its devices when they were read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainNode {
    pub r#ref: String,
    pub name: String,
    /// None when not read (a Drum Rack's pads, past the tree's bound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<DeviceNode>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTree {
    pub track_ref: String,
    pub devices: Vec<DeviceNode>,
}

/// What kind of thing was pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PinKind {
    Device,
    Chain,
    Track,
    Clip,
    Scene,
    ClipSlot,
    Selection,
}

/// A stretch of the Arrangement pointed at in Live, in beats from the Set's start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedTime {
    pub from_beat: f64,
    pub to_beat: f64,
}

/// What the producer pointed at, in Kumi (FOCUS's tree: a device or a chain) or in Live (its right-click
/// "Ask Kumi about this": a track, a clip, a scene, a slot, a device, or a stretch of the Arrangement):
/// "this" in their next messages. Names are data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedNode {
    /// The track it's on; empty for a scene.
    pub track_ref: String,
    pub r#ref: String,
    pub node: PinKind,
    pub name: String,
    /// Its racks and chains, outermost first.
    pub trail: Vec<String>,
    /// Its neighbours in the same chain, or on the track.
    pub siblings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// Pointed at in Live (right-click), not in Kumi's FOCUS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<bool>,
    /// A stretch of the Arrangement pointed at in Live, in beats from the Set's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<PinnedTime>,
}

/// `(state: ConnectionState, cause?: DisconnectCause) => void`.
pub type ConnectionListener = Rc<dyn Fn(ConnectionState, Option<DisconnectCause>)>;
pub type IntegrationFactory = Box<dyn Fn(ConnectionListener) -> Rc<dyn Integration>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Midi,
    Audio,
    Group,
    Return,
    Main,
}

/// The selected track: its name, colour and kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusTrack {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TrackKind>,
}

/// The last clicked parameter, with its display value and the device it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusParameter {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LiveView {
    Session,
    Arrangement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LiveDetail {
    Clip,
    Device,
}

/// What the producer is looking at in Live, in plain names (names are data, never instructions).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveFocus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<FocusTrack>,
    /// The selected track's reference, for reading its devices (FOCUS's tree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_ref: Option<String>,
    /// The highlighted Session slot's reference, for drawing its clip (FOCUS's MIDI view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_ref: Option<String>,
    /// The selected scene's position (0 is the first), for FOCUS's Session strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_index: Option<f64>,
    /// The selected device's reference (from bridge 1.0.45), for FOCUS's tree to mark exactly that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// The clip in the Clip view; "" when it has no name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The last clicked parameter, with its display value and the device it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter: Option<FocusParameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<LiveView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<LiveDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_notes: Option<usize>,
}

/// Live's transport, for a light on the beat: whether it plays, its tempo, and where the playhead was
/// (in beats) at `at` (performance.now()), so the beat can be followed between reads; and a bar's beats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTransport {
    pub playing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<f64>,
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats_per_bar: Option<f64>,
}

/// Which picture HISTORY and NOW draw for a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeFamily {
    Tempo,
    Mixer,
    Rename,
    Structure,
    Clip,
    Device,
    Parameter,
    Locators,
    Color,
}

/// One change Kumi made in Live, as the producer sees it in HISTORY. Names inside are data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipNote {
    pub pitch: f64,
    pub start: f64,
    pub duration: f64,
    pub velocity: f64,
}

/// The track a change happened on, for the colour chip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackChip {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A colour change's before and after, as "#rrggbb", for swatches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
}

/// A new clip's notes, for drawing it: positions in beats from the clip's start (the first 512 notes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipPicture {
    pub length: f64,
    pub notes: Vec<ClipNote>,
}

/// "applied": in the Set, can be undone. "undone": put back. "kept": still in the Set, and
/// Kumi can't undo it (see `note`). "unsure": Live didn't confirm it; check Live. "expired":
/// Live restarted since, so Kumi can't undo it; whether it's still in the Set depends on
/// whether the Set was saved. "heard": Kumi listened to it (an audition); nothing in the Set changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeState {
    Applied,
    Undone,
    Kept,
    Unsure,
    Expired,
    Heard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeRecord {
    /// Unique for this Kumi process: "c1", "c2", …
    pub id: String,
    pub family: ChangeFamily,
    /// Plain words, such as "Tempo 120 → 124 BPM".
    pub title: String,
    /// The track it happened on, for the colour chip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackChip>,
    /// Before and after, for the picture (a fader position, a value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    /// The span `from` and `to` move in, for drawing them as positions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// A colour change's before and after, as "#rrggbb", for swatches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<ColorChange>,
    /// A new clip's notes, for drawing it: positions in beats from the clip's start (the first 512 notes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipPicture>,
    /// Where a loaded device or a new chain sits, for drawing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<DevicePlacement>,
    pub state: ChangeState,
    /// An audition's best closeness to its reference, 0–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Why an undo didn't happen, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: i64,
}

/// A rack's chain, for NOW's picture: its name and its devices in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainPlacement {
    pub name: String,
    pub devices: Vec<String>,
}

/// A device chain by name, for NOW's picture: a track's or a chain's devices in order with the new
/// one's place, and, inside a rack, the rack's chains side by side and which one it's in.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DevicePlacement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<ChainPlacement>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<f64>,
}

/// What changed in a saved Set while Kumi wasn't running, in plain words. Names are data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatchUp {
    pub set: String,
    /// When Kumi last saw the Set, in ms since the epoch.
    pub last_seen_at: i64,
    pub lines: Vec<String>,
    /// Changes not listed.
    pub more: usize,
    /// Live went away and came back while Kumi was running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_reconnect: Option<bool>,
}

/// A lesson Kumi learned from a match run (or forgot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LessonAction {
    Learned,
    Updated,
    Forgot,
}

/// Something Kumi did in Live that isn't a change to the Set: playing, launching, recording, showing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionEvent {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<bool>,
}

/// What the producer picked with a key among the options Kumi offered.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    /// One of the options Kumi's answer ended on: the question, the options, and the one picked (from 0).
    Answer { question: String, options: Vec<String>, index: usize },
    /// Whether to keep the technique Kumi offered after an answer.
    Technique { name: String, keep: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum SessionEvent {
    Focus {
        focus: Option<LiveFocus>,
    },
    /// The producer pointed at something in Live (right-click "Ask Kumi about this"): the app pins it.
    Pointed {
        pin: PinnedNode,
    },
    Change {
        change: ChangeRecord,
    },
    CatchUp {
        catch_up: CatchUp,
    },
    /// A kept conversation continues (`lines` are its recent exchanges): the Set's own, one chosen in
    /// /conversations (`chosen`), or one this model couldn't continue (`unreadable`, shown only).
    /// `changes` are its HISTORY, which Kumi can't undo any more.
    Resumed {
        saved_at: i64,
        lines: Vec<TranscriptLine>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changes: Option<Vec<ChangeRecord>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chosen: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unreadable: Option<bool>,
    },
    /// Live is back after stopping a request: the app offers to send `text` again.
    Resend {
        text: String,
    },
    State {
        state: TurnState,
    },
    Connection {
        state: ConnectionState,
    },
    Observation {
        label: String,
    },
    Notice {
        message: String,
    },
    /// `kind` and `provider` say what failed and where, so an app can offer the fix (sign in, choose a model).
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<FailureKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    TurnComplete {
        result: TurnResult,
        elapsed_ms: u64,
    },
    /// A lesson Kumi learned from a match run (or forgot): its line, as /memory shows it.
    Lesson {
        action: LessonAction,
        id: String,
        line: String,
    },
    Match(MatchStatus),
    Goal(GoalStatus),
    /// A /goal objective's status: where it is, turns and time against its budget, and the last check.
    Objective(super::goal_mode::ObjectiveStatus),
    /// No goal to show: there's none here, or another Set is open.
    ObjectiveCleared,
    Heard(HeardEvent),
    Auditioned(AuditionEvent),
    /// A round of a judged run: its target, change, numbers before and after, keep or revert, and what's next.
    Judged(crate::listening::round::Round),
    /// Where the loop is: rounds, kept and taken back, listens, time, the next target.
    Loop(super::loop_run::LoopStatus),
    Watched(WatchedEvent),
    Web(WebEvent),
    Library(LibraryEvent),
    Recipe(RecipeEvent),
    Technique(TechniqueEvent),
    /// What a tool at work is doing now ("looking at 2:05"), for NOW; it ends with the tool.
    Doing {
        text: String,
    },
    /// Something Kumi did in Live that isn't a change to the Set: playing, launching, recording, showing.
    Action(ActionEvent),
    /// Live's transport as it is now (None: not known, Live gone), for the beat light.
    Transport {
        transport: Option<LiveTransport>,
    },
    /// Kumi started or stopped watching the producer work in Live (watch_me).
    Watching {
        on: bool,
    },
    #[serde(untagged)]
    Kernel(KernelEvent),
    #[serde(untagged)]
    Memory(MemoryEvent),
}

impl From<KernelEvent> for SessionEvent {
    fn from(event: KernelEvent) -> Self {
        Self::Kernel(event)
    }
}

impl From<MemoryEvent> for SessionEvent {
    fn from(event: MemoryEvent) -> Self {
        Self::Memory(event)
    }
}

impl From<MatchStatus> for SessionEvent {
    fn from(status: MatchStatus) -> Self {
        Self::Match(status)
    }
}

impl From<GoalStatus> for SessionEvent {
    fn from(status: GoalStatus) -> Self {
        Self::Goal(status)
    }
}

impl From<AuditionEvent> for SessionEvent {
    fn from(event: AuditionEvent) -> Self {
        Self::Auditioned(event)
    }
}

/// "producer": true of them in any project; "set": about one saved Set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    Producer,
    Set,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryNote {
    /// "p3" (about the producer) or "s3" (about the Set): what the model and /memory name it by.
    pub id: String,
    pub text: String,
    /// Epoch milliseconds it was written.
    pub at: i64,
    /// The producer pinned it: a full store never drops it to make room.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

/// What the producer changes about a note in /memory, without the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteChange {
    Text(String),
    Pinned(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Memory {
    pub producer: Vec<MemoryNote>,
    pub set: Vec<MemoryNote>,
}

/// How keeping a note went: kept (with the note it replaced, or that made room), or not, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remembering {
    Kept {
        note: MemoryNote,
        replaced: Option<MemoryNote>,
    },
    /// `replaces` names no note in the scope.
    NoSuchNote,
    /// The scope is full and every note in it is pinned.
    AllPinned,
}

#[async_trait(?Send)]
pub trait MemoryStore {
    /// Notes about the producer, and about the saved Set `project` when there is one.
    async fn load(&self, project: Option<&str>) -> Result<Memory, RuntimeError>;
    async fn save(&self, scope: MemoryScope, project: Option<&str>, notes: &[MemoryNote]) -> Result<(), RuntimeError>;
    /// Keep a note: in place of `replaces` (keeping its pin), or added with the next id, the oldest
    /// unpinned making room when the scope is full.
    async fn remember(
        &self,
        scope: MemoryScope,
        project: Option<&str>,
        text: &str,
        replaces: Option<&str>,
        at: i64,
    ) -> Result<Remembering, RuntimeError> {
        let memory = self.load(project).await?;
        let mut notes = if scope == MemoryScope::Set { memory.set } else { memory.producer };
        let remembering = super::memory::remember_in(&mut notes, scope, text, replaces, at);
        if matches!(remembering, Remembering::Kept { .. }) {
            self.save(scope, project, &notes).await?;
        }
        Ok(remembering)
    }
    /// The producer's change to a note (its words already checked): the note as changed, if there is one.
    async fn change(
        &self,
        scope: MemoryScope,
        project: Option<&str>,
        id: &str,
        change: NoteChange,
        at: i64,
    ) -> Result<Option<MemoryNote>, RuntimeError> {
        let memory = self.load(project).await?;
        let mut notes = if scope == MemoryScope::Set { memory.set } else { memory.producer };
        let Some(note) = notes.iter_mut().find(|n| n.id == id) else {
            return Ok(None);
        };
        match change {
            NoteChange::Text(text) => {
                note.text = text;
                note.at = at;
            }
            NoteChange::Pinned(pinned) => note.pinned = pinned,
        }
        let note = note.clone();
        self.save(scope, project, &notes).await?;
        Ok(Some(note))
    }
    /// Notes kept while a Set was unsaved, added now that it's saved, while there's room.
    async fn add(&self, scope: MemoryScope, project: Option<&str>, texts: &[String], at: i64) -> Result<(), RuntimeError> {
        let memory = self.load(project).await?;
        let mut notes = if scope == MemoryScope::Set { memory.set } else { memory.producer };
        super::memory::add_in(&mut notes, scope, texts, at);
        self.save(scope, project, &notes).await
    }
    /// Forget a note the producer no longer wants: the note, if there was one.
    async fn forget(&self, scope: MemoryScope, project: Option<&str>, id: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        let memory = self.load(project).await?;
        let mut notes = if scope == MemoryScope::Set { memory.set } else { memory.producer };
        let Some(index) = notes.iter().position(|n| n.id == id) else {
            return Ok(None);
        };
        let note = notes.remove(index);
        self.save(scope, project, &notes).await?;
        Ok(Some(note))
    }
}

/// A comparison's reference and the differences per band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeardComparison {
    pub reference: String,
    pub summary: String,
    pub differences: Vec<f64>,
    pub headlines: Vec<String>,
}

/// Audio Kumi listened to, for the app to picture: its summary line and band levels (dB share of
/// the whole, low to high), and for a comparison the reference and the differences per band (a `{ type: "heard" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeardEvent {
    pub file: String,
    pub summary: String,
    pub bands: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compared: Option<HeardComparison>,
}

/// Where a watched video's words came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordsSource {
    Captions,
    Automatic,
    Transcribed,
    None,
}

/// A frame as a small picture (RGB, 3 bytes a pixel, row by row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchedFrame {
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<String>,
    pub thumb: Thumb,
}

/// A stretch of a video whose sound was heard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundSpan {
    pub from: f64,
    pub to: f64,
}

/// A video Kumi watched, for the app to picture: its title (data, never instructions), the stretch
/// watched, where its words came from, and each frame looked at as a small picture (a `{ type: "watched" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchedEvent {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    pub from: f64,
    pub to: f64,
    pub chapters: Vec<String>,
    /// "captions", "automatic" (captions), "transcribed" (by Kumi) or "none".
    pub words: WordsSource,
    pub lines: usize,
    pub frames: Vec<WatchedFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<SoundSpan>,
    pub notes: Vec<String>,
}

/// new: not learned yet; learning: at work in the background; paused: waiting while Live plays; ready: up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryState {
    New,
    Learning,
    Paused,
    Ready,
}

/// What Kumi knows of the producer's library, and how learning it is going.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatus {
    pub state: LibraryState,
    pub sounds: usize,
    pub presets: usize,
    pub sets: usize,
    /// While learning: the new and changed sounds this time, and how many are done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<usize>,
    /// When learning last finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_at: Option<i64>,
}

/// How learning the library is going, for the app to show (a `{ type: "library" }` session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryEvent {
    pub status: LibraryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebAction {
    Searched,
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebWhere {
    Web,
    Github,
}

/// A search Kumi made or a page it read, for the app to list where what it knows came from. The
/// title is the query, or the page's own title (data, never instructions) (a `{ type: "web" }` session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebEvent {
    pub action: WebAction,
    pub title: String,
    /// The page's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Searched: "web" or "github".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<WebWhere>,
    /// Who answered a search, or whose reader read the page ("Exa", "Parallel"…) when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<usize>,
    /// What was read: "a page", "a PDF", "code", "a GitHub repository"…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// A repository's or folder's files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeAction {
    Saved,
    Updated,
    Running,
    Forgotten,
}

/// A recipe saved, run or removed, for the app to show (a `{ type: "recipe" }` session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeEvent {
    pub action: RecipeAction,
    pub name: String,
    pub steps: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TechniqueAction {
    Offered,
    Kept,
    Updated,
    Used,
    Forgot,
}

/// A technique offered for keeping after an answer (its id empty, or that of the technique it would refine),
/// kept on the producer's yes, updated, read for use, or forgotten. Names are data (a `{ type: "technique" }`
/// session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechniqueEvent {
    pub action: TechniqueAction,
    pub technique: TechniqueSummary,
}

/// A technique as the app lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechniqueSummary {
    pub id: String,
    pub name: String,
    pub fits: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A recipe's blank: its name and what it's for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeParam {
    pub name: String,
    pub about: String,
}

/// A recipe as the app lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeSummary {
    pub name: String,
    pub about: String,
    pub params: Vec<RecipeParam>,
    pub steps: usize,
    pub used: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<f64>,
    pub created: f64,
}

/// A note written or removed, for the app to show; `pending` while the Set isn't saved yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum MemoryEvent {
    Remembered {
        scope: MemoryScope,
        note: MemoryNote,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaced: Option<MemoryNote>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending: Option<bool>,
    },
    Forgot {
        scope: MemoryScope,
        note: MemoryNote,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub state: TurnState,
    pub connection: ConnectionState,
    pub turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<String>,
}

/// What the integration reports outside a turn's own events: a change Kumi made (kept with the
/// conversation, for HISTORY), an action in Live, or an audition.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchEvent {
    Change(ChangeRecord),
    Action(ActionEvent),
    Audition(AuditionEvent),
    Judged(crate::listening::round::Round),
}

/// What Kumi remembers now: about the producer, and about the open Set when it's saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    #[serde(flatten)]
    pub memory: Memory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_name: Option<String>,
    pub saved: bool,
}

/// Something Kumi learned, as one line with an id to forget it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptLine {
    pub id: String,
    pub line: String,
}

/// A lesson from matching sounds: its line, and when it was learned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LessonEntry {
    pub id: String,
    pub line: String,
    pub at: f64,
}

/// A file the producer added to a request, pasted or dragged in: a picture the model sees, or any
/// file, which the model works with by its path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
}

/// A picture the model sees with the producer's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub name: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

/// What running a recipe did, in words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeOutcome {
    pub text: String,
    pub is_error: bool,
}

/// Optional methods (`steer?()` and so on) have a `has_…` flag, false by default, that says whether
/// this controller offers them; their default bodies answer as an absent method's caller would read.
#[async_trait(?Send)]
pub trait SessionController {
    async fn start(&self) -> Result<(), RuntimeError>;
    /// `pinned`: what the producer points at in Kumi, which "this" means in the message.
    async fn submit(&self, input: &str, pinned: Option<PinnedNode>) -> Result<(), RuntimeError>;
    fn has_attachments(&self) -> bool {
        false
    }
    /// `submit` with files the producer added: pictures go to the model to see, and every file by its path.
    async fn submit_with(&self, input: &str, pinned: Option<PinnedNode>, attachments: Vec<Attachment>) -> Result<(), RuntimeError> {
        if attachments.is_empty() {
            self.submit(input, pinned).await
        } else {
            Err(RuntimeError::plain("Kumi can't take files here."))
        }
    }
    fn has_steer(&self) -> bool {
        false
    }
    /// More from the producer for the answer under way: it enters the conversation at the answer's next
    /// step (a "steer" event says when). False when no answer is at a point to take it; send it later.
    fn steer(&self, text: &str) -> bool {
        let _ = text;
        false
    }
    fn has_aside(&self) -> bool {
        false
    }
    /// A side question (/btw) about the conversation so far, answered while Kumi works or not, without
    /// tools; neither the question nor the answer joins the conversation. Its words stream to `on_text`.
    async fn aside(&self, question: &str, on_text: OnText, signal: Option<Signal>) -> Result<String, RuntimeError> {
        let _ = (question, on_text, signal);
        Err(absent())
    }
    async fn refresh(&self) -> Result<(), RuntimeError>;
    /// Forget this conversation and start afresh; it stays in the Set's kept conversations.
    async fn new_conversation(&self) -> Result<(), RuntimeError>;
    fn has_reconnect(&self) -> bool {
        false
    }
    /// A fresh bridge to Live, carrying the conversation over.
    async fn reconnect(&self) -> Result<(), RuntimeError> {
        Err(absent())
    }
    fn has_conversations(&self) -> bool {
        false
    }
    /// The open Set's kept conversations, newest first.
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(Vec::new())
    }
    fn has_resume_conversation(&self) -> bool {
        false
    }
    /// Continue a kept conversation instead of this one (which stays kept); false when it's gone.
    async fn resume_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        let _ = id;
        Ok(false)
    }
    fn has_watch(&self) -> bool {
        false
    }
    /// What the integration reports outside a turn's own events: a change Kumi made (kept with the
    /// conversation, for HISTORY), or an action in Live. Undoing a build withdraws the technique
    /// Kumi offered for it.
    fn watch(&self, event: WatchEvent) {
        let _ = event;
    }
    async fn cancel(&self) -> Result<(), RuntimeError>;
    async fn close(&self) -> Result<(), RuntimeError>;
    fn status(&self) -> SessionStatus;
    /// Undo one of Kumi's changes (the latest undoable one when `id` is omitted); not during a turn.
    /// Resolves with the change as it now stands, or None when there was nothing to undo (an
    /// error event says why).
    async fn undo(&self, id: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError>;
    fn has_memory(&self) -> bool {
        false
    }
    /// What Kumi remembers now: about the producer, and about the open Set when it's saved.
    async fn memory(&self) -> Result<Option<MemoryView>, RuntimeError> {
        Ok(None)
    }
    fn has_library(&self) -> bool {
        false
    }
    /// The producer's library: what Kumi knows of it, and how learning it is going.
    fn library(&self) -> Option<LibraryStatus> {
        None
    }
    fn has_taste(&self) -> bool {
        false
    }
    /// What Kumi learned from the producer's own Sets, a line each, and forgetting one by id.
    async fn taste(&self) -> Result<Vec<KeptLine>, RuntimeError> {
        Ok(Vec::new())
    }
    fn has_forget_taste(&self) -> bool {
        false
    }
    async fn forget_taste(&self, id: &str) -> Result<bool, RuntimeError> {
        let _ = id;
        Ok(false)
    }
    fn has_forget(&self) -> bool {
        false
    }
    /// Remove a note by id; None when there's none.
    async fn forget(&self, id: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        let _ = id;
        Ok(None)
    }
    fn has_change_note(&self) -> bool {
        false
    }
    /// Change a note's words or pin it, as the producer asks in /memory; None when there's no such note.
    async fn change_note(&self, id: &str, change: NoteChange) -> Result<Option<MemoryNote>, RuntimeError> {
        let _ = (id, change);
        Ok(None)
    }
    fn has_recipes(&self) -> bool {
        false
    }
    /// The producer's saved recipes, most recently used first.
    async fn recipes(&self) -> Result<Vec<RecipeSummary>, RuntimeError> {
        Ok(Vec::new())
    }
    fn has_run_recipe(&self) -> bool {
        false
    }
    /// Run a recipe straight away (no model involved), `with` a value for each of its blanks; what it did,
    /// in words.
    async fn run_recipe(&self, name: &str, with: JsonObject) -> Result<RecipeOutcome, RuntimeError> {
        let _ = (name, with);
        Err(absent())
    }
    fn has_forget_recipe(&self) -> bool {
        false
    }
    async fn forget_recipe(&self, name: &str) -> Result<bool, RuntimeError> {
        let _ = name;
        Ok(false)
    }
    fn has_techniques(&self) -> bool {
        false
    }
    /// The techniques Kumi learned, and forgetting one by id.
    async fn techniques(&self) -> Result<Vec<TechniqueSummary>, RuntimeError> {
        Ok(Vec::new())
    }
    fn has_forget_technique(&self) -> bool {
        false
    }
    async fn forget_technique(&self, id: &str) -> Result<bool, RuntimeError> {
        let _ = id;
        Ok(false)
    }
    fn has_answer_technique(&self) -> bool {
        false
    }
    /// The producer's answer to the technique Kumi offered after an answer: kept on yes. False when
    /// nothing was waiting (they had moved on, or undid the build).
    async fn answer_technique(&self, keep: bool) -> Result<bool, RuntimeError> {
        let _ = keep;
        Ok(false)
    }
    /// The producer picked one of the options Kumi offered, with a key: one an answer ended on, or keeping a
    /// technique. Never a default taken with Enter, or Esc. Kept to learn their taste from.
    fn picked(&self, pick: Picked) {
        let _ = pick;
    }
    fn has_goal(&self) -> bool {
        false
    }
    /// /goal: pursue one (with what to reach), or pick a paused one up (without); stop ends it; the dashboard's numbers.
    async fn goal(&self, text: Option<&str>) -> Result<(), RuntimeError> {
        let _ = text;
        Err(absent())
    }
    fn has_stop_goal(&self) -> bool {
        false
    }
    async fn stop_goal(&self) -> Result<bool, RuntimeError> {
        Ok(false)
    }
    fn has_stop_loop(&self) -> bool {
        false
    }
    /// /loop stop: ends a sound-match search, running or paused, keeping its best (a judged loop ends as Esc ends it).
    async fn stop_loop(&self) -> Result<bool, RuntimeError> {
        Ok(false)
    }
    fn has_goal_status(&self) -> bool {
        false
    }
    fn goal_status(&self) -> Option<GoalStatus> {
        None
    }
    fn has_lessons(&self) -> bool {
        false
    }
    /// What Kumi learned matching sounds, newest first, and forgetting one.
    async fn lessons(&self) -> Result<Vec<LessonEntry>, RuntimeError> {
        Ok(Vec::new())
    }
    fn has_forget_lesson(&self) -> bool {
        false
    }
    async fn forget_lesson(&self, id: &str) -> Result<bool, RuntimeError> {
        let _ = id;
        Ok(false)
    }
    fn has_stop_live(&self) -> bool {
        false
    }
    /// Stop Live (clips, the transport and recording), any time, even during a turn; false when it couldn't.
    async fn stop_live(&self) -> Result<bool, RuntimeError> {
        Ok(false)
    }
    fn has_device_tree(&self) -> bool {
        false
    }
    /// A track's device tree, for FOCUS (while connected).
    async fn device_tree(&self, track_ref: &str) -> Result<Option<DeviceTree>, RuntimeError> {
        let _ = track_ref;
        Ok(None)
    }
    fn has_clip_view(&self) -> bool {
        false
    }
    /// The clip in a Session slot, for FOCUS (while connected).
    async fn clip_view(&self, slot_ref: &str) -> Result<Option<ClipView>, RuntimeError> {
        let _ = slot_ref;
        Ok(None)
    }
    fn has_session_strip(&self) -> bool {
        false
    }
    /// A track's Session slots around a scene, and the Arrangement at a glance, for FOCUS (while connected).
    async fn session_strip(&self, track_ref: &str, scene: f64) -> Result<Option<SessionStrip>, RuntimeError> {
        let _ = (track_ref, scene);
        Ok(None)
    }
    fn has_arrangement_strip(&self) -> bool {
        false
    }
    async fn arrangement_strip(&self) -> Result<Option<ArrangementStrip>, RuntimeError> {
        Ok(None)
    }
    fn has_reconfigure(&self) -> bool {
        false
    }
    /// The model changed (a new one chosen, a sign-in, a new effort): the next turn or refresh builds
    /// the kernel afresh through the factory, continuing this conversation. Safe during a turn.
    async fn reconfigure(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
