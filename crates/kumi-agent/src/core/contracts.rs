use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::evolve::Knob;
use crate::core::goal::GoalStatus;
use crate::core::match_run::MatchStatus;
pub use crate::core::view::*;
use kumi_types::contracts::*;
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::errors::{FailureKind, RuntimeError};

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
    /// A model call failed in a way worth trying again: why, how long Kumi waits first (the turn can
    /// still be stopped meanwhile), and what it waits for when that isn't a busy provider.
    Retry {
        reason: String,
        wait_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<WaitFor>,
        /// What refused the call on the way, when that's what it waits out as offline: a certificate (TLS) or a
        /// proxy. The wait is the same; the words say which.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<kumi_net::cause::NetCause>,
        /// A proxy `cause` that isn't answering (its address refused, its name not found) rather than one that answered
        /// no: the words say to start it again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        proxy_down: bool,
    },
    /// Another model answers from here: the producer's fallback, while their model can't (why: its plan's
    /// limit, an outage), or their own model back again (`back`).
    ModelSwitch {
        model: String,
        reason: String,
        #[serde(default)]
        back: bool,
    },
}

/// What a long wait for a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitFor {
    /// A plan's usage limit to reset: the answer carries on then.
    Limit,
    /// The connection to come back.
    Offline,
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
    /// The fallback model answering while the producer's can't (`<provider>/<model>`); None while theirs does.
    fn answering_fallback(&self) -> Option<String> {
        None
    }
    /// The DAW the conversation is about, for the words the kernel's own notes use; Live's until it's told.
    fn set_profile(&self, profile: DawProfile) {
        let _ = profile;
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

/// The project an observation is about, as the DAW knows it: which DAW, its durable id once saved, its name, its file,
/// when it was last saved, and the DAW's identity for it while it stays open. Names are data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdentity {
    pub daw: DawId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_identity: Option<String>,
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
    /// Live's identity for the open Set: it holds while the Set stays open in this run of Live, through a Kumi restart
    /// and a new conversation (an unsaved Set's goal is kept by it). None when Live wasn't read.
    pub set: Option<String>,
    /// The Set's track names, when all of them were read (names are data).
    pub tracks: Option<Vec<String>>,
    /// When the saved Set's file was last written: a later time means the producer saved it.
    pub saved_at: Option<f64>,
    /// The project as the DAW knows it, its name for the app to show; None when the DAW wasn't read.
    pub identity: Option<ProjectIdentity>,
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

/// A kept conversation, as /resume lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub saved_at: i64,
    pub first: String,
    pub turns: u32,
    /// From a store: its place's latest. From a session: the conversation going on now.
    pub current: bool,
    /// Where it's kept: its Set's place (a saved Set's project id, or "unsaved").
    #[serde(default)]
    pub place: String,
    /// The producer's name for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its Set's name, when Kumi has seen the Set saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    /// Kept however old it gets.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// The conversation it was copied from: a fork, or one carried on in another Set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    /// When it began (ms since the epoch).
    #[serde(default)]
    pub created: i64,
    /// It came from a file (kumi import), or is a copy of one: its earlier words are the file's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub imported: bool,
}

impl ConversationSummary {
    /// What it's called: the producer's name for it, else its first request.
    pub fn title(&self) -> String {
        crate::core::conversations::title(self.name.as_deref(), &self.first)
    }
}

/// What the producer's word for a conversation (`kumi -r`, /resume) names among the kept ones.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationMatch {
    One(ConversationSummary),
    /// More than one fits, newest first: the producer picks.
    Several(Vec<ConversationSummary>),
    /// One Kumi kept once and has let go.
    Gone,
    None,
}

/// What a DAW and a place in it are called when the integration doesn't say.
pub const DAW_WORDS: (&str, &str) = DawProfile::UNKNOWN.word_pair();

/// Whether this Kumi has a conversation to itself, or another Kumi that's still open has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Ours,
    Taken,
}

/// The conversation going on now: its id and what it's called, whether it's kept yet, and whether this run made a request
/// in it or took it up (the exit names it then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationNow {
    pub id: String,
    pub title: String,
    pub kept: bool,
    pub touched: bool,
}

/// The conversation going on now as an export reads it: its id, what it's called, its Set's name, when it began (0 when
/// it isn't kept yet), what was said up to its last answer, and what the producer calls the DAW and a place in it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationToExport {
    pub id: String,
    pub title: String,
    pub set: Option<String>,
    pub created: i64,
    pub conversation: SavedConversation,
    pub words: (&'static str, &'static str),
}

/// The conversation the producer chose belongs to another Set than the open one: Kumi asks what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAsk {
    pub id: String,
    pub title: String,
    /// Its Set's name, when Kumi has seen that Set saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    /// The open Set's name; none for one not saved yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    /// What the producer calls the DAW and a place in it ("Live", "Set").
    pub daw: String,
    pub noun: String,
    /// Kumi can't open places in this DAW: the producer opens it, and the conversation carries on there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_hand: bool,
}

/// The producer's answer when a conversation belongs to another Set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetChoice {
    /// Open its Set in the DAW, and carry on there.
    Open,
    /// Carry it on in the open Set, as a copy.
    Copy,
    /// Leave it: the open Set's own conversation goes on.
    Cancel,
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
    /// Keep `id` as `place`'s current conversation. A place keeps its 20 newest, and others while they're pinned or
    /// were used lately.
    async fn save(&self, place: &str, id: &str, conversation: &SavedConversation) -> Result<(), RuntimeError>;
    /// `place` starts afresh; its conversations stay listed.
    async fn fresh(&self, place: &str) -> Result<(), RuntimeError>;
    /// Kept conversation `id` is `place`'s current one from here, as it's kept: nothing in it changes.
    async fn set_current(&self, place: &str, id: &str) -> Result<(), RuntimeError> {
        let _ = (place, id);
        Ok(())
    }
    /// `place`'s conversations, newest first.
    async fn list(&self, place: &str) -> Result<Vec<ConversationSummary>, RuntimeError>;
    /// A conversation moves with its Set (an unsaved Set's, when the Set is first saved).
    async fn move_conversation(&self, id: &str, from: &str, to: &str) -> Result<(), RuntimeError>;
    /// What a saved place is called, by the DAW it's in: kept beside its conversations, so a list can name it.
    async fn note_place(&self, place: &str, daw: DawId, name: &str) -> Result<(), RuntimeError> {
        let _ = (place, daw, name);
        Ok(())
    }
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
    /// Every place's conversations, newest first.
    async fn all(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(vec![])
    }
    /// What `word` names: a conversation's id, the start of one (4 characters or more), or the producer's name for it.
    async fn find(&self, word: &str) -> Result<ConversationMatch, RuntimeError> {
        let _ = word;
        Ok(ConversationMatch::None)
    }
    /// An id no kept conversation has.
    async fn new_id(&self) -> String {
        crate::core::conversations::new_conversation_id(kumi_common::time::now_ms())
    }
    /// The producer's name for a kept conversation (None takes it away); false when it isn't kept.
    async fn rename(&self, place: &str, id: &str, name: Option<&str>) -> Result<bool, RuntimeError> {
        let _ = (place, id, name);
        Ok(false)
    }
    /// A pinned conversation is kept however old it gets; false when it isn't kept.
    async fn pin(&self, place: &str, id: &str, pinned: bool) -> Result<bool, RuntimeError> {
        let _ = (place, id, pinned);
        Ok(false)
    }
    /// The conversation goes for good; false when it wasn't kept.
    async fn delete(&self, place: &str, id: &str) -> Result<bool, RuntimeError> {
        let _ = (place, id);
        Ok(false)
    }
    /// Keep `conversation` as a new one, `id`, in `place` (the place's current), with the producer's name for it and the
    /// conversation it was copied from, when it has them: all there by the time it's listed.
    async fn create(
        &self,
        place: &str,
        id: &str,
        conversation: &SavedConversation,
        name: Option<&str>,
        from: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let _ = (name, from);
        self.save(place, id, conversation).await
    }
    /// This Kumi takes the conversation, unless another Kumi that's still open has it.
    fn claim(&self, place: &str, id: &str) -> Claim {
        let _ = (place, id);
        Claim::Ours
    }
    /// This Kumi is done with the conversation (it took another, or is closing).
    fn release(&self, place: &str, id: &str) {
        let _ = (place, id);
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

/// Optional methods (`undo?()` and so on) have a `has_…` flag, which the profile's capabilities answer by default.
#[async_trait(?Send)]
pub trait Integration {
    /// The DAW this drives, its words and what it can ever do; no DAW known by default.
    fn profile(&self) -> &DawProfile {
        &DawProfile::UNKNOWN
    }
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError>;
    /// What's in Live now; `pinned` (what the producer pointed at) is checked against it and given to the model.
    async fn observe(&self, signal: Signal, hints: Option<ObserveHints>) -> Result<Observation, RuntimeError>;
    async fn close(&self) -> Result<(), RuntimeError>;
    /// Resolves once no tool call is still at work in the app (a stopped answer's, putting it back): a stopped answer
    /// ends only then, so the next one doesn't run beside it.
    async fn settled(&self) {}
    /// The producer's words came into the answer under way (a steer): what it changes from here may be theirs.
    fn steered(&self) {}
    /// Words the integration adds at the end of the system text, read again each time the model's session is made:
    /// Live's say what Willington's bindings allow, as their switch stands now. `offered` says whether the model has a
    /// tool by a name. None by default.
    fn instructions_addendum(&self, offered: &dyn Fn(&str) -> bool) -> String {
        let _ = offered;
        String::new()
    }
    /// The conversation the turns from here belong to: what's kept for a conversation (the devices it made) is this one's.
    fn conversation(&self, id: &str) {
        let _ = id;
    }
    /// Tools a background job may use, which never wait on Live (make_device writes its file and doesn't wait for
    /// Live's Browser to list it).
    fn worker_tools(&self) -> Vec<Rc<dyn KernelTool>> {
        Vec::new()
    }
    fn has_undo(&self) -> bool {
        self.profile().caps.undo
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
    /// The first time at or after `since` (ms) that Live was heard playing (the producer's playing, or Kumi's own
    /// listens, which play through Main), for whether they heard one of Kumi's changes. None if it hasn't played since.
    fn first_heard(&self, since: i64) -> Option<i64> {
        let _ = since;
        None
    }
    fn has_audio_file(&self) -> bool {
        self.profile().caps.audio_file
    }
    /// The audio file behind something in the Set the model names (a clip, say); None when it isn't one.
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let _ = (named, signal);
        Ok(None)
    }
    fn has_stop_live(&self) -> bool {
        self.profile().caps.stop_transport
    }
    /// Stop clips, the transport and recording in Live at once; true when Live is stopped afterwards.
    async fn stop_live(&self, signal: Signal) -> Result<bool, RuntimeError> {
        let _ = signal;
        Ok(false)
    }
    /// The saved Set open in the DAW, by its file, once Kumi has read it: what a restart of the DAW opens again.
    fn open_set(&self) -> Option<String> {
        None
    }
    /// What the producer calls the DAW and a place in it (Live's integration: "Live", "Set"); plain words otherwise.
    fn words(&self) -> (&'static str, &'static str) {
        self.profile().word_pair()
    }
    /// Whether Kumi can open a saved place in the DAW ([`Integration::open_place`]); else the producer opens it.
    fn has_open_place(&self) -> bool {
        false
    }
    /// Open the saved place (a Set, a project) a kept conversation belongs to in the DAW, which may ask first whether to
    /// save the open one. `Err(why)` inside when it can't.
    async fn open_place(&self, place: &str, signal: Signal) -> Result<Result<(), String>, RuntimeError> {
        let _ = (place, signal);
        Ok(Err("Kumi can't open it from here.".into()))
    }
    fn has_show_message(&self) -> bool {
        self.profile().caps.status_line
    }
    /// A short line in the DAW's own window (Live's status bar), never a dialog, for a producer working there; true
    /// when it was shown. A DAW without such a place says false.
    async fn show_message(&self, text: &str, signal: Signal) -> Result<bool, RuntimeError> {
        let _ = (text, signal);
        Ok(false)
    }
    /// Whether the DAW is recording now; None when it can't say.
    async fn recording(&self, signal: Signal) -> Result<Option<bool>, RuntimeError> {
        let _ = signal;
        Ok(None)
    }
    fn has_instant(&self) -> bool {
        self.profile().caps.instant
    }
    /// A fixed phrase's intent (core/phrases.rs) made as this DAW's change, with no model call: what changed, in
    /// words. None when it can't run as asked here (a track the Set doesn't have, or two that fit): the model takes it.
    async fn instant(&self, intent: &crate::core::phrases::Intent, signal: Signal) -> Result<Option<InstantOutcome>, RuntimeError> {
        let _ = (intent, signal);
        Ok(None)
    }
    fn has_device_tree(&self) -> bool {
        self.profile().caps.device_tree
    }
    /// A track's devices, racks' chains and what's in them, for FOCUS; None when Live can't say.
    async fn device_tree(&self, track_ref: &str, signal: Signal) -> Result<Option<DeviceTree>, RuntimeError> {
        let _ = (track_ref, signal);
        Ok(None)
    }
    fn has_clip_view(&self) -> bool {
        self.profile().caps.note_view
    }
    /// The MIDI clip in a Session slot, its notes and which are selected, for FOCUS; None for none (or audio).
    async fn clip_view(&self, slot_ref: &str, signal: Signal) -> Result<Option<ClipView>, RuntimeError> {
        let _ = (slot_ref, signal);
        Ok(None)
    }
    fn has_session_strip(&self) -> bool {
        self.profile().caps.launcher_grid
    }
    /// A track's Session slots around a scene, for FOCUS.
    async fn session_strip(&self, track_ref: &str, scene: f64, signal: Signal) -> Result<Option<SessionStrip>, RuntimeError> {
        let _ = (track_ref, scene, signal);
        Ok(None)
    }
    fn has_arrangement_strip(&self) -> bool {
        self.profile().caps.timeline_strip
    }
    /// The Arrangement at a glance (length, playhead, loop, locators), for FOCUS.
    async fn arrangement_strip(&self, signal: Signal) -> Result<Option<ArrangementStrip>, RuntimeError> {
        let _ = signal;
        Ok(None)
    }
    /// Whether the DAW can give FOCUS this kind of body: Live's views by their flags, unless the integration says.
    fn can_show(&self, kind: BodyKind) -> bool {
        match kind {
            BodyKind::Tree => self.has_device_tree(),
            BodyKind::Notes => self.has_clip_view(),
            BodyKind::Grid => self.has_session_strip(),
            BodyKind::Timeline => self.has_arrangement_strip(),
            _ => false,
        }
    }
    /// A pin as a recipe's blanks take it now: what was pinned where it is now, in the turn's short refs (a DAW whose refs
    /// are places moves them with its tracks); None when Kumi can't tell. Live's and REAPER's refs hold: as the pin was made.
    fn pin_for_recipe(&self, pin: &PinnedNode) -> Option<PinnedNode> {
        Some(pin.clone())
    }
    /// A body for FOCUS, read by the DAW; None when it can't say. Live's integration reads its views.
    async fn focus_body(&self, request: &BodyRequest, signal: Signal) -> Result<Option<FocusBody>, RuntimeError> {
        Ok(match request {
            BodyRequest::Tree { track_ref } => self.device_tree(track_ref, signal).await?.map(FocusBody::Tree),
            BodyRequest::Notes { item_ref } => self.clip_view(item_ref, signal).await?.map(FocusBody::Notes),
            BodyRequest::Grid { track_ref, index } => self.session_strip(track_ref, *index, signal).await?.map(FocusBody::Grid),
            BodyRequest::Timeline => self.arrangement_strip(signal).await?.map(FocusBody::Timeline),
            _ => None,
        })
    }
    fn has_audition(&self) -> bool {
        self.profile().caps.audition
    }
    /// Render candidates, hear them and set them against a reference (the audition tool's work); `Err(why)` inside says why not.
    async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let _ = (request, signal);
        Err(absent())
    }
    fn has_goal(&self) -> bool {
        self.profile().caps.goal_rig
    }
    /// A goal's render rig over these candidates (kept open across generations), or why not.
    async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        let _ = (request, signal);
        Err(absent())
    }
    fn has_hear(&self) -> bool {
        self.profile().caps.hear
    }
    /// Hear tracks (or the mix) in the Set directly: as they play now, or by playing a stretch of the Arrangement; a file each, or why not.
    async fn hear(&self, request: &HearRequest, signal: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let _ = (request, signal);
        Err(absent())
    }
}

/// A goal's candidate chain in Live: its track, what it is, and the knobs a search may move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalSlotInfo {
    pub name: String,
    pub label: String,
    pub chain: String,
    pub knobs: Vec<Knob>,
    /// Who each knob's device is (`Knob::device` to the device's identity: a built-in device's own name, a plug-in's
    /// format, maker and name as far as the DAW says them), for what Kumi remembers of the search. A device the rig
    /// doesn't name is known by its name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub devices: std::collections::BTreeMap<String, String>,
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
    /// The layout its refs were read at, where refs are places (Renoise's): it's carried from there to where it is now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<u64>,
}

/// `(state: ConnectionState, cause?: DisconnectCause) => void`.
pub type ConnectionListener = Rc<dyn Fn(ConnectionState, Option<DisconnectCause>)>;
pub type IntegrationFactory = Box<dyn Fn(ConnectionListener) -> Rc<dyn Integration>>;

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
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
    /// The conversation the producer chose belongs to another Set: open it there, carry on here in a copy, or leave it.
    /// `ask` None: the question is no longer open.
    SetAsked {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ask: Option<SetAsk>,
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
        /// The project's name, as the DAW gives it (the label says it in words).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
    Objective(crate::core::goal_mode::ObjectiveStatus),
    /// No goal to show: there's none here, or another Set is open.
    ObjectiveCleared,
    Heard(HeardEvent),
    Auditioned(AuditionEvent),
    /// A round of a judged run: its target, change, numbers before and after, keep or revert, and what's next.
    Judged(kumi_listening::round::Round),
    /// Where the loop is: rounds, kept and taken back, listens, time, the next target.
    Loop(crate::core::loop_run::LoopStatus),
    Watched(WatchedEvent),
    Web(WebEvent),
    Library(LibraryEvent),
    /// How learning the producer's own music is going.
    Music(MusicEvent),
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
    /// A background job started, moved on or ended: its whole status, by its id.
    Job(crate::core::jobs::JobStatus),
    /// Kumi carries on by itself with a job's result, as the request it was started for asked: an answer follows.
    CarryOn {
        job: u32,
        title: String,
    },
    /// The session's palette as it is now, after a change (None: it has none).
    Palette {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        palette: Option<crate::core::palette::PaletteView>,
    },
    /// Words of a side question's answer as they're written (the protocol's `aside` request with this `id`).
    Aside {
        id: u32,
        text: String,
    },
    #[serde(untagged)]
    Kernel(KernelEvent),
    #[serde(untagged)]
    Memory(MemoryEvent),
}

/// A tool's event, as the session's own.
impl From<ToolEvent> for SessionEvent {
    fn from(event: ToolEvent) -> Self {
        match event {
            ToolEvent::Doing { text } => SessionEvent::Doing { text },
            ToolEvent::Notice { message } => SessionEvent::Notice { message },
            ToolEvent::Heard(heard) => SessionEvent::Heard(heard),
            ToolEvent::Watched(watched) => SessionEvent::Watched(watched),
            ToolEvent::Web(web) => SessionEvent::Web(web),
        }
    }
}

/// Where a tool's events go: on to `emit`, as the session's own.
pub fn tool_events(emit: &Rc<dyn Fn(SessionEvent)>) -> Rc<dyn Fn(ToolEvent)> {
    let emit = emit.clone();
    Rc::new(move |event| emit(event.into()))
}

impl SessionEvent {
    /// What the app is told of an observation: its label, and the project's name as the DAW gives it.
    pub fn observed(observation: &Observation) -> SessionEvent {
        SessionEvent::Observation {
            label: observation.label.clone(),
            name: observation.identity.as_ref().map(|identity| identity.name.clone()),
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
        let remembering = crate::core::memory::remember_in(&mut notes, scope, text, replaces, at);
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
        crate::core::memory::add_in(&mut notes, scope, texts, at);
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

/// How learning the library is going, for the app to show (a `{ type: "library" }` session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryEvent {
    pub status: LibraryStatus,
}

/// How learning the producer's own music is going, for the app to show (a `{ type: "music" }` session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicEvent {
    pub status: kumi_listening::music::learning::MusicStatus,
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
    Judged(kumi_listening::round::Round),
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

/// A fixed phrase run with no model call ("tempo 124"): what changed, in words, as Kumi says it.
#[derive(Debug, Clone, PartialEq)]
pub struct InstantOutcome {
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
    /// The fallback model answering for the producer's now (`<provider>/<model>`); None while theirs does.
    fn answering_fallback(&self) -> Option<String> {
        None
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
    /// Every Set's kept conversations, newest first; the one going on now is `current` (here and in this Set's list
    /// and what a word finds).
    async fn all_conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(vec![])
    }
    /// What the producer's word names among the kept conversations: an id, the start of one, or a name.
    async fn find_conversation(&self, word: &str) -> Result<ConversationMatch, RuntimeError> {
        let _ = word;
        Ok(ConversationMatch::None)
    }
    /// The producer's name for a kept conversation (`id`), or for the one going on now; None takes it away. False when
    /// there's no such conversation.
    async fn rename_conversation(&self, id: Option<&str>, name: Option<&str>) -> Result<bool, RuntimeError> {
        let _ = (id, name);
        Ok(false)
    }
    async fn pin_conversation(&self, id: &str, pinned: bool) -> Result<bool, RuntimeError> {
        let _ = (id, pinned);
        Ok(false)
    }
    /// A kept conversation goes for good (never the one going on now); false when it wasn't kept.
    async fn delete_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        let _ = id;
        Ok(false)
    }
    /// A copy of the conversation going on now, with a new id, to carry on in; the original stays. Its id, or None with
    /// nothing to copy yet.
    async fn fork_conversation(&self, name: Option<&str>) -> Result<Option<String>, RuntimeError> {
        let _ = name;
        Ok(None)
    }
    /// The producer's answer to `SetAsked`.
    async fn answer_set(&self, choice: SetChoice) -> Result<(), RuntimeError> {
        let _ = choice;
        Ok(())
    }
    /// The conversation going on now.
    async fn conversation_now(&self) -> Option<ConversationNow> {
        None
    }
    /// A kept conversation's words, for a look before going back to it.
    async fn conversation_lines(&self, id: &str) -> Result<Vec<TranscriptLine>, RuntimeError> {
        let _ = id;
        Ok(vec![])
    }
    /// The conversation going on now, to export (/export, /share): None before its first answer.
    async fn conversation_to_export(&self) -> Option<ConversationToExport> {
        None
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
    /// What Kumi's searches remember (settings heard, their devices, how often one won the round it was tried in): None
    /// without its database.
    async fn remembered(&self) -> Option<crate::core::search_memory::SearchMemoryStatus> {
        None
    }
    /// What Kumi remembers of searches, for a DAW's listening to hand what it hears to and start from: None without its
    /// database.
    fn settings_memory(&self) -> Option<Rc<dyn kumi_listening::heard_settings::SettingsMemory>> {
        None
    }
    /// The saved Set open in the DAW, by its file, once the session has read it.
    fn open_set(&self) -> Option<String> {
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
    fn has_instant(&self) -> bool {
        false
    }
    /// A message that is a fixed phrase ("tempo 124", "mute drums", "loop 9-16"), run with no model call: what
    /// changed. None when it isn't one, or can't run as asked: then it goes to the model as written.
    async fn instant(&self, text: &str) -> Result<Option<InstantOutcome>, RuntimeError> {
        let _ = text;
        Ok(None)
    }
    /// A pin as a recipe's blanks take it now (the DAW's integration says); as it was made when nothing says otherwise.
    fn pin_for_recipe(&self, pin: &PinnedNode) -> Option<PinnedNode> {
        Some(pin.clone())
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
    fn has_daw_message(&self) -> bool {
        false
    }
    /// A short line in the DAW's own window (Live's status bar), any time, even during a turn; false when it couldn't.
    async fn daw_message(&self, text: &str) -> Result<bool, RuntimeError> {
        let _ = text;
        Ok(false)
    }
    /// Whether the DAW is recording now; None when it can't say (no DAW connected, or one that doesn't tell).
    async fn daw_recording(&self) -> Result<Option<bool>, RuntimeError> {
        Ok(None)
    }
    fn has_jobs(&self) -> bool {
        false
    }
    /// The background jobs, oldest first (the latest that ended, and every one running).
    fn jobs(&self) -> Vec<crate::core::jobs::JobStatus> {
        Vec::new()
    }
    /// One job with its steps and report.
    fn job(&self, id: u32) -> Option<crate::core::jobs::JobDetail> {
        let _ = id;
        None
    }
    /// Stop a running job; false when it isn't running.
    fn stop_job(&self, id: u32) -> bool {
        let _ = id;
        false
    }
    /// Run a job that ended (one interrupted, say) again: the new job's number, or why it can't.
    fn run_job_again(&self, id: u32) -> Result<u32, String> {
        let _ = id;
        Err("Kumi can't run background jobs here.".into())
    }
    /// Ctrl-b: the answer's steps at work away from the DAW go on as a job and the answer carries on without them; the
    /// job's number, or why nothing moved.
    fn background(&self) -> Result<u32, String> {
        Err("Kumi can't run background jobs here.".into())
    }
    /// Whether ctrl-b has a step to move right now.
    fn can_background(&self) -> bool {
        false
    }
    /// Carry on with a job that ended whose request needs it (`when_done` continue): the app calls this when the producer
    /// isn't in the middle of something (a pick, a panel, typing). Kumi then answers as itself. False when it can't now
    /// (Kumi is answering) or the job isn't waiting to carry on.
    fn carry_on(&self, job: u32) -> bool {
        let _ = job;
        false
    }
    fn has_palette(&self) -> bool {
        false
    }
    /// The session's palette, with its files: None before one is made or chosen.
    async fn palette(&self) -> Result<Option<crate::core::palette::PaletteView>, RuntimeError> {
        Ok(None)
    }
    /// Sounds or songs gathered (on the map, in the browser's map) into the session's palette, a new one when it has none
    /// yet: what happened, in words.
    async fn palette_gather(&self, files: Vec<crate::core::palette::Gathered>) -> Result<String, RuntimeError> {
        let _ = files;
        Err(absent())
    }
    /// A file out of the session's palette (by its path, size and modification time): whether it had it.
    async fn palette_remove(&self, path: &str, size: i64, mtime: i64) -> Result<bool, RuntimeError> {
        let _ = (path, size, mtime);
        Ok(false)
    }
    /// /palette and its words (list, new, use, rename, forget, strict, loose): its answer, in words.
    async fn palette_command(&self, words: &str) -> Result<String, RuntimeError> {
        let _ = words;
        Err(absent())
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
    /// Whether FOCUS can show this kind of body (while connected).
    fn can_show(&self, kind: BodyKind) -> bool {
        match kind {
            BodyKind::Tree => self.has_device_tree(),
            BodyKind::Notes => self.has_clip_view(),
            BodyKind::Grid => self.has_session_strip(),
            BodyKind::Timeline => self.has_arrangement_strip(),
            _ => false,
        }
    }
    /// A body for FOCUS (while connected).
    async fn focus_body(&self, request: &BodyRequest) -> Result<Option<FocusBody>, RuntimeError> {
        Ok(match request {
            BodyRequest::Tree { track_ref } => self.device_tree(track_ref).await?.map(FocusBody::Tree),
            BodyRequest::Notes { item_ref } => self.clip_view(item_ref).await?.map(FocusBody::Notes),
            BodyRequest::Grid { track_ref, index } => self.session_strip(track_ref, *index).await?.map(FocusBody::Grid),
            BodyRequest::Timeline => self.arrangement_strip().await?.map(FocusBody::Timeline),
            _ => None,
        })
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
