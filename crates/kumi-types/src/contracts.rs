//! The contracts between the agent and its tools: what a tool is (`KernelTool`) and returns (`ToolResult`), how far it
//! reaches, and what the tools that hear, audition, render, read the web, watch videos and learn the library report;
//! and what a turn's model calls used (`Usage`), which the model clients count.
//! The rest of the agent's contracts (the kernel, the session and its events, the stores, the DAW integrations) are
//! kumi-runtime's `core::contracts`, which re-exports these.

use std::rc::Rc;

use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::RuntimeError;
pub use crate::matching::{Closeness, Focus};
pub use crate::picture::ChangePicture;

pub type JsonObject = serde_json::Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// All prompt tokens, including cache reads and writes.
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_read_tokens: f64,
    pub cache_write_tokens: f64,
}

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

/// What a tool does to the DAW: nothing (it works away from it: the web, the producer's files, Kumi's own records),
/// reads it, or may change it. Background jobs use only tools away from the DAW.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    AwayFromDaw,
    DawRead,
    DawWrite,
}

/// A tool the kernel offers the model. `stream` is optional: it returns None for a tool whose calls
/// can't start before they're whole.
/// What a tool is for, when the session needs that tool and not any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolRole {
    /// Makes several changes as one plan (recipes run through it; a technique is drafted beside it).
    Plan,
    /// Undoes one of Kumi's changes.
    Undo,
    /// Judges a change by ear (a judged run's rounds).
    Judge,
    /// Tunes knobs toward a target by ear.
    Tune,
    /// Finds sounds on this computer (the library's own, when Kumi has one, takes its place).
    FindSounds,
}

impl ToolRole {
    /// The role of one of Kumi's own tools, by its name.
    pub fn named(name: &str) -> Option<ToolRole> {
        match name {
            "make_changes" => Some(ToolRole::Plan),
            "undo_change" => Some(ToolRole::Undo),
            "judge" => Some(ToolRole::Judge),
            "tune" => Some(ToolRole::Tune),
            "find_sounds" => Some(ToolRole::FindSounds),
            _ => None,
        }
    }
}

/// Kumi's own tools that only read away from the DAW: several calls to them run at once.
pub const CONCURRENT: &[&str] =
    &["search_web", "read_web", "find_sounds", "find_presets", "my_sets", "live_manual", "search_conversations"];

#[async_trait(?Send)]
pub trait KernelTool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> JsonObject;
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError>;
    /// What it does to the DAW. A tool that doesn't say is taken to change it.
    fn reach(&self) -> Reach {
        Reach::DawWrite
    }
    /// Start while the model is still writing the call: a plan's first steps run as its later ones are
    /// written. Offered for a model reply's first call only, since calls run in order. `on_start` says
    /// the work has begun (the call then shows as running).
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        let _ = (signal, on_start);
        None
    }
    /// Rarely needed, so not sent with every request: the model finds it with find_tools and runs it with
    /// call_tool (or by its name), and every request stays small. Whoever makes the tool decides.
    fn deferred(&self) -> bool {
        false
    }
    /// The tool whose own description already lists this one with its input (a plan tool whose steps run
    /// it): find_tools' list points there instead of repeating this one.
    fn listed_by(&self) -> Option<&str> {
        None
    }
    /// What the tool is for, for the session to find it by (the plan tool, the undo, the judge…): by its name by
    /// default, as Kumi names its own; an integration whose tools are named otherwise says.
    fn role(&self) -> Option<ToolRole> {
        ToolRole::named(self.name())
    }
    /// Whether it only reads away from the DAW, so several calls to it and tools like it can run at once: by its name
    /// by default, for Kumi's own.
    fn concurrent(&self) -> bool {
        CONCURRENT.contains(&self.name())
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

/// What to hear in the Set: tracks by reference or name (or the whole mix), and when.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HearRequest {
    pub tracks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix: Option<bool>,
    /// A stretch of the Arrangement to play, in beats; left out, what's playing now (or, stopped, the loop or the playhead's part).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_beat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats: Option<f64>,
    /// How long to hear what's playing now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// The whole song, from the Arrangement's start to its end, in one listen.
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

/// A gap no knob closes, and the structural change that closes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralMove {
    pub gap: String,
    pub r#move: String,
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
    /// Anything the producer should know (a render track that stayed, a track left disarmed).
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
    /// What it looks like, before and after (absent on records saved before Kumi drew them, and read as absent when a
    /// later Kumi drew one this one can't).
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::picture::lenient")]
    pub picture: Option<ChangePicture>,
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
