//! Live-domain contract and deterministic simulator.
//!
//! The simulator is deliberately an adapter test double: it models stable
//! references and state transitions without claiming that Ableton Live is
//! installed or connected. A Remote Script/Extension can implement the same
//! contract at the protocol boundary.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use async_trait::async_trait;
use kumi_common::abort::Signal;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::registry::{live_registry_hash, live_registry_operations, RegistryError};

pub const LIVE_PROTOCOL_VERSION: &str = "ableton-live/v1";
// SHA-256 of canonical sorted-key JSON, so negotiation is invariant to the
// checkout's LF/CRLF policy on macOS and Windows.
pub static LIVE_REGISTRY_HASH: LazyLock<&'static str> = LazyLock::new(live_registry_hash);
pub static LIVE_REGISTRY_OPERATIONS: LazyLock<&'static [String]> = LazyLock::new(live_registry_operations);

/// A string-valued enumeration, as the TypeScript's string unions: its text on the wire and in files.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $text)] $variant),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(&self) -> &'static str { match self { $($name::$variant => $text),+ } }
            pub fn parse(text: &str) -> Option<$name> { match text { $($text => Some($name::$variant),)+ _ => None } }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

string_enum! {
    /// What a connected Live can do, as the bridge advertises it.
    LiveCapability {
        SessionRead = "session.read", Tracks = "tracks", Scenes = "scenes", Clips = "clips", Notes = "notes",
        SessionDiscovery = "session.discovery", SessionStructure = "session.structure", SessionMidiClipCreate = "session.midi_clip.create", SessionMidiClipDelete = "session.midi_clip.delete", SessionMidiNoteRead = "session.midi_note.read", SessionMidiNoteWrite = "session.midi_note.write",
        ArrangementRead = "arrangement.read", ArrangementWrite = "arrangement.write", Audio = "audio", AudioCaptureResampling = "audio.capture.resampling", Warp = "warp", Takes = "takes",
        Automation = "automation", Devices = "devices", Racks = "racks", Chains = "chains", Parameters = "parameters", Browser = "browser",
        DeviceParameterWrite = "device.parameter.write",
        Routing = "routing", Recording = "recording", Projects = "projects", Mixing = "mixing", Transport = "transport", Max = "max", Osc = "osc", View = "view", Tuning = "tuning", Groove = "groove",
        RealtimeEvents = "realtime.events", Plugins = "plugins", Subscriptions = "subscriptions", Reconnect = "reconnect",
    }
}

pub const LIVE_CAPABILITIES: &[LiveCapability] = LiveCapability::ALL;

pub const LIVE_UNAVAILABLE_CAPABILITIES: &[LiveCapability] = &[
    LiveCapability::ArrangementRead,
    LiveCapability::ArrangementWrite,
    LiveCapability::Audio,
    LiveCapability::AudioCaptureResampling,
    LiveCapability::Warp,
    LiveCapability::Takes,
    LiveCapability::Automation,
    LiveCapability::Devices,
    LiveCapability::Racks,
    LiveCapability::Chains,
    LiveCapability::Parameters,
    LiveCapability::Browser,
    LiveCapability::Routing,
    LiveCapability::Recording,
    LiveCapability::Projects,
    LiveCapability::Mixing,
    LiveCapability::Max,
    LiveCapability::Osc,
    LiveCapability::RealtimeEvents,
    LiveCapability::Plugins,
];

pub const SIMULATOR_CAPABILITIES: &[LiveCapability] = &[
    LiveCapability::SessionRead,
    LiveCapability::Tracks,
    LiveCapability::Scenes,
    LiveCapability::Clips,
    LiveCapability::Notes,
    LiveCapability::SessionDiscovery,
    LiveCapability::SessionStructure,
    LiveCapability::SessionMidiClipCreate,
    LiveCapability::SessionMidiClipDelete,
    LiveCapability::SessionMidiNoteRead,
    LiveCapability::SessionMidiNoteWrite,
    LiveCapability::ArrangementRead,
    LiveCapability::ArrangementWrite,
    LiveCapability::Transport,
    LiveCapability::Devices,
    LiveCapability::Parameters,
    LiveCapability::DeviceParameterWrite,
    LiveCapability::Subscriptions,
    LiveCapability::Reconnect,
    LiveCapability::View,
    LiveCapability::Warp,
    LiveCapability::Takes,
    LiveCapability::Tuning,
    LiveCapability::Groove,
];

/// Capability derivation shared by the remote adapter's status validation and
/// the simulator's advertisement: a capability is advertised only when its
/// exact negotiated operations are present.
pub fn live_capabilities_for_operations<S: AsRef<str>>(operations: &[S]) -> Vec<LiveCapability> {
    let has = |operation: &str| operations.iter().any(|item| item.as_ref() == operation);
    let all = |required: &[&str]| required.iter().all(|operation| has(operation));
    let any = |required: &[&str]| required.iter().any(|operation| has(operation));
    let readable_hierarchy = all(&["snapshot", "discover", "get"]);
    let requirement = |capability: LiveCapability| -> bool {
        match capability {
            LiveCapability::SessionRead => readable_hierarchy && all(&["session.playback"]),
            LiveCapability::Tracks | LiveCapability::Scenes | LiveCapability::Clips | LiveCapability::Notes => readable_hierarchy,
            LiveCapability::SessionDiscovery => all(&["discover"]),
            LiveCapability::SessionStructure => any(&["track.create", "track.delete", "scene.create", "scene.delete"]),
            LiveCapability::SessionMidiClipCreate => all(&["clip.create"]),
            LiveCapability::SessionMidiClipDelete => all(&["clip.delete"]),
            LiveCapability::SessionMidiNoteRead => readable_hierarchy,
            LiveCapability::SessionMidiNoteWrite => all(&["note.add", "note.add-batch"]),
            LiveCapability::ArrangementRead => any(&["locator.add", "arrangement.clip.delete", "arrangement.automation.read"]),
            LiveCapability::ArrangementWrite => any(&[
                "locator.add",
                "locator.delete",
                "arrangement.clip.create",
                "arrangement.audio-clip.create",
                "arrangement.clip.delete",
            ]),
            LiveCapability::Audio => all(&["audio.clip.set"]),
            LiveCapability::AudioCaptureResampling => {
                all(&["audio.capture.inspect", "audio.capture.start", "audio.capture.stop", "audio.capture.cleanup"])
            }
            LiveCapability::Warp => all(&["audio.warp-marker.read"]),
            LiveCapability::Takes => all(&["audio.take-lane.read"]),
            LiveCapability::Automation => all(&["automation.envelope.read"]),
            LiveCapability::Devices | LiveCapability::Racks | LiveCapability::Chains | LiveCapability::Parameters => readable_hierarchy,
            LiveCapability::Browser => all(&["browser.search"]),
            LiveCapability::DeviceParameterWrite => all(&["device.parameter.set"]),
            LiveCapability::Routing => all(&["routing.set"]),
            LiveCapability::Recording => any(&["recording.session", "recording.arrangement"]),
            LiveCapability::Projects => all(&["snapshot"]),
            LiveCapability::Mixing => all(&["mixer.set"]),
            LiveCapability::Transport => all(&["transport.set", "tempo.set"]),
            LiveCapability::Tuning => any(&["tuning.read", "tuning.set"]),
            LiveCapability::Groove => all(&["groove.read"]),
            LiveCapability::Max => false,
            LiveCapability::View => any(&["view.set", "view.control"]),
            LiveCapability::Osc | LiveCapability::RealtimeEvents => all(&["realtime.arm", "realtime.disarm", "realtime.stats"]),
            LiveCapability::Plugins => readable_hierarchy,
            LiveCapability::Subscriptions => all(&["subscribe"]),
            LiveCapability::Reconnect => all(&["reconnect"]),
        }
    };
    LIVE_CAPABILITIES.iter().copied().filter(|capability| requirement(*capability)).collect()
}

string_enum! {
    /// The kinds of simulator-local references (`kind:key`).
    LiveObjectKind {
        Set = "set", Track = "track", Scene = "scene", Clip = "clip", ClipSlot = "clip-slot", SessionPlayback = "session-playback", ArrangementClip = "arrangement-clip",
        TakeLane = "take-lane", TakeLaneClip = "take-lane-clip", Groove = "groove", Device = "device", Parameter = "parameter", Note = "note", Automation = "automation",
        Locator = "locator", Chain = "chain", DrumPad = "drum_pad",
    }
}

/// Opaque references are simulator-local (`kind:key`) or production mapper
/// references (`epoch:wire_kind:key`, the wire kinds being the object kinds above plus `clip_slot`,
/// `arrangement_clip`, `take_lane`, `take_lane_clip`, `groove`, `routing_choice`, `return_track`,
/// `main_track` and `browser_item`). Callers must never parse authority from either form; the adapter
/// performs epoch and object-identity checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveRef(pub String);

impl LiveRef {
    /// A simulator reference: `${kind}:${id}`.
    pub fn new(kind: LiveObjectKind, id: &str) -> LiveRef {
        LiveRef(format!("{}:{}", kind.as_str(), id))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The kind this reference names (see [`ref_kind`]).
    pub fn kind(&self) -> Option<&str> {
        ref_kind(&self.0)
    }
    /// The track a positional Remote Script reference sits under (see [`track_index_of_ref`]).
    pub fn track_index(&self) -> Option<usize> {
        track_index_of_ref(&self.0)
    }
}

impl std::ops::Deref for LiveRef {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl AsRef<str> for LiveRef {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl std::borrow::Borrow<str> for LiveRef {
    fn borrow(&self) -> &str {
        &self.0
    }
}
impl From<&str> for LiveRef {
    fn from(text: &str) -> LiveRef {
        LiveRef(text.to_string())
    }
}
impl From<String> for LiveRef {
    fn from(text: String) -> LiveRef {
        LiveRef(text)
    }
}
impl From<LiveRef> for String {
    fn from(reference: LiveRef) -> String {
        reference.0
    }
}
impl PartialEq<str> for LiveRef {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}
impl PartialEq<&str> for LiveRef {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}
impl PartialEq<String> for LiveRef {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}
impl PartialEq<LiveRef> for str {
    fn eq(&self, other: &LiveRef) -> bool {
        self == other.0
    }
}
impl PartialEq<LiveRef> for &str {
    fn eq(&self, other: &LiveRef) -> bool {
        *self == other.0
    }
}
impl std::fmt::Display for LiveRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

string_enum! {
    LiveMonitoringState { In = "in", Auto = "auto", Off = "off" }
}

string_enum! {
    LiveDiscoveryKind {
        Set = "set", Track = "track", ReturnTrack = "return-track", MainTrack = "main-track", Scene = "scene", ClipSlot = "clip-slot", SessionClip = "session-clip",
        ArrangementClip = "arrangement-clip", Note = "note", Locator = "locator", Device = "device", Parameter = "parameter", Selection = "selection",
        RoutingChoice = "routing-choice", SessionPlayback = "session-playback",
    }
}

/// A `foo?: T | null` field: absent, null, or a value. The three are kept apart because rows from Live
/// are fingerprinted as they come, and a key that is there with `null` hashes differently from one that
/// isn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Maybe<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}

impl<T> Maybe<T> {
    /// `value ?? null` read as an option: the value, or none whether absent or null.
    pub fn value(&self) -> Option<&T> {
        match self {
            Maybe::Value(value) => Some(value),
            _ => None,
        }
    }
    pub fn is_absent(&self) -> bool {
        matches!(self, Maybe::Absent)
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Maybe::Null)
    }
    /// `x ?? null` written back: a value, or null.
    pub fn null_or(value: Option<T>) -> Maybe<T> {
        match value {
            Some(value) => Maybe::Value(value),
            None => Maybe::Null,
        }
    }
    pub fn some(value: T) -> Maybe<T> {
        Maybe::Value(value)
    }
    pub fn into_option(self) -> Option<T> {
        match self {
            Maybe::Value(value) => Some(value),
            _ => None,
        }
    }
    pub fn cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        self.value().cloned()
    }
    /// `x ?? null` as JSON: the value, or null.
    pub fn to_json(&self) -> Value
    where
        T: Serialize,
    {
        self.value().map(|value| serde_json::to_value(value).unwrap_or(Value::Null)).unwrap_or(Value::Null)
    }
}

impl<T: Serialize> Serialize for Maybe<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Maybe::Value(value) => value.serialize(serializer),
            Maybe::Absent | Maybe::Null => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Maybe<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Maybe<T>, D::Error> {
        Ok(Maybe::null_or(Option::<T>::deserialize(deserializer)?))
    }
}

/// `x ?? null` as JSON for a plain optional.
pub fn null_or_json<T: Serialize>(value: &Option<T>) -> Value {
    value.as_ref().map(|value| serde_json::to_value(value).unwrap_or(Value::Null)).unwrap_or(Value::Null)
}

/// What an operation runs under: its cancellation, deadline and transaction identity.
#[derive(Debug, Clone, Default)]
pub struct LiveOperationContext {
    pub signal: Option<Signal>,
    pub deadline_ms: Option<f64>,
    /// Stable host transaction authority; the remote adapter derives per-operation replay keys from it.
    pub idempotency_key: Option<String>,
    pub transaction_id: Option<String>,
}

impl LiveOperationContext {
    pub fn with_deadline(deadline_ms: f64) -> LiveOperationContext {
        LiveOperationContext { deadline_ms: Some(deadline_ms), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDiscoveryRequest {
    pub kind: LiveDiscoveryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl LiveDiscoveryRequest {
    pub fn of(kind: LiveDiscoveryKind) -> LiveDiscoveryRequest {
        LiveDiscoveryRequest { kind, parent: None, filter: None, fields: None, budget: None, limit: None, cursor: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDiscoveryResult {
    pub epoch: i64,
    pub items: Vec<Map<String, Value>>,
    pub truncated: bool,
    pub revision: String,
    pub kind: LiveDiscoveryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl LiveDiscoveryResult {
    /// What `serde_json::to_value` makes of it, moving the rows instead of copying them (a page can
    /// hold megabytes of tracks).
    pub fn into_value(self) -> Value {
        let mut row = Map::new();
        row.insert("epoch".into(), self.epoch.into());
        row.insert("items".into(), Value::Array(self.items.into_iter().map(Value::Object).collect()));
        row.insert("truncated".into(), self.truncated.into());
        row.insert("revision".into(), self.revision.into());
        row.insert("kind".into(), self.kind.as_str().into());
        if let Some(cursor) = self.next_cursor {
            row.insert("nextCursor".into(), cursor.into());
        }
        Value::Object(row)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlaybackTarget {
    pub track_ref: LiveRef,
    pub clip_slot_ref: LiveRef,
    pub scene_ref: LiveRef,
    pub scene_index: usize,
    pub clip_ref: Option<LiveRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchQuantization {
    /// A string, a number or null.
    pub raw: Value,
    pub normalized: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportLoop {
    pub enabled: Option<bool>,
    pub start: Option<f64>,
    pub length: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTransport {
    pub playing: Option<bool>,
    pub arrangement_record: Option<bool>,
    pub session_record: Option<bool>,
    pub position: Option<f64>,
    pub launch_quantization: LaunchQuantization,
    #[serde(rename = "loop")]
    pub loop_: TransportLoop,
    pub punch_in: Option<bool>,
    pub punch_out: Option<bool>,
    pub metronome: Option<bool>,
    pub count_in: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlaybackState {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    pub epoch: i64,
    pub revision: String,
    pub transport: SessionTransport,
    pub fired_targets: Vec<SessionPlaybackTarget>,
    pub playing_targets: Vec<SessionPlaybackTarget>,
}

string_enum! {
    LiveAdapterKind { Simulator = "simulator", RemoteScript = "remote-script", Extension = "extension", Unavailable = "unavailable", OfflineFile = "offline-file" }
}

string_enum! {
    LiveProvenance { RealLive = "real-live", FakeLive = "fake-live", Simulator = "simulator", Unknown = "unknown" }
}

/// Best-effort read-only runtime evidence reported by the adapter; every field may be unprobed (null).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEnvironment {
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub live_version: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub live_edition: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub os: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub api: Maybe<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub connected: bool,
    pub adapter: LiveAdapterKind,
    pub epoch: Option<i64>,
    pub protocol: String,
    pub capabilities: Vec<LiveCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<LiveProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub willington_kinds: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<LiveEnvironment>,
    /// Channel-specific evidence and future negotiated status fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl LiveStatus {
    /// Whether `operation` was negotiated (`status.operations?.includes(operation)`).
    pub fn has_operation(&self, operation: &str) -> bool {
        self.operations.as_ref().map(|operations| operations.iter().any(|item| item == operation)).unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub pitch: f64,
    pub start: f64,
    pub duration: f64,
    pub velocity: f64,
    pub channel: f64,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub id: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub mute: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub probability: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub velocity_deviation: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub release_velocity: Maybe<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Note {
    pub fn new(pitch: f64, start: f64, duration: f64, velocity: f64, channel: f64) -> Note {
        Note {
            pitch,
            start,
            duration,
            velocity,
            channel,
            id: Maybe::Absent,
            mute: Maybe::Absent,
            probability: Maybe::Absent,
            velocity_deviation: Maybe::Absent,
            release_velocity: Maybe::Absent,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPoint {
    pub time: f64,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<LiveRef>,
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub automatable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub default_value: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub original_name: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub state: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub value_items: Maybe<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A rack chain's own mixer.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainMixer {
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub sends: Vec<Option<f64>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub volume_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub panning_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_refs: Option<Vec<LiveRef>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub chain_activator_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixer_identity: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceChain {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    pub parent_ref: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    /// Absent on a chain the simulator makes for a pad's sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub mute: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub solo: Maybe<bool>,
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub color_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub auto_color: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub has_audio_input: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub has_audio_output: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub has_midi_input: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub has_midi_output: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub muted_via_solo: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub in_note: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub out_note: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub choke_group: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixer: Option<ChainMixer>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrumPad {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    pub parent_ref: LiveRef,
    pub index: usize,
    pub name: String,
    pub mute: Option<bool>,
    pub chains: Vec<DeviceChain>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub note: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub solo: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

string_enum! {
    DeviceKind { Instrument = "instrument", AudioEffect = "audio-effect", MidiEffect = "midi-effect", Plugin = "plugin", Rack = "rack", Device = "device" }
}

/// A rack macro row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Macro {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    // Observed values reach the host unchanged so it can decide whether restoration is possible.
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_collapsed: Maybe<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceComparison {
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub capability: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub active_side: Maybe<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RackView {
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub selected_chain_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub selected_pad_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub pad_scroll_position: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub show_chain_devices: Maybe<bool>,
}

/// A device row. The device-family rows (`drift`, `eq8`, `hybridReverb`, `meld`, `drumCell`, `looper`,
/// `plugin`, `maxDevice`) are kept as the JSON objects Live sends; the TypeScript declared their fields as
/// `drift: { modSources?, modTargets?, pitchBendRange?, voiceCount?, voiceMode?, voiceCountList?, voiceModeList? }`,
/// `eq8: { editMode?, globalMode?, oversample?, selectedBand? }`, `hybridReverb: { irCategory?, irFile?,
/// irCategoryList?, irFileList?, attack?, decay?, size? }`, `meld: { engine?, unison?, monoPoly?, polyphony? }`,
/// `drumCell: { gain? }`, `looper: { overdubAfterRecord?, recordLengthIndex?, loopLength?, tempo?, state? }`,
/// `plugin: { presets?, selectedPresetIndex?, isEditorOpen? }` and `maxDevice: { audioIns?, audioOuts?, midiIns?,
/// midiOuts? }`, every one nullable. Other device-specific rows (`sample`, `wavetable`, `roar`, `deviceIo`...)
/// come in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<LiveRef>,
    pub name: String,
    pub kind: DeviceKind,
    pub parameters: Vec<Parameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub can_have_chains: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub can_have_drum_pads: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<DeviceChain>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drum_pads: Option<Vec<DrumPad>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macros: Option<Vec<Macro>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub chain_selector: Maybe<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<DeviceView>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub latency_samples: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub latency_ms: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub parameter_bank: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<DeviceComparison>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_chains: Option<Vec<LiveRef>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub visible_macro_count: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq8: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hybrid_reverb: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meld: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drum_cell: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looper: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_device: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub selected_variation_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub macro_mapped: Maybe<Vec<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack_view: Option<RackView>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Device {
    /// A bare device row: `{ ref, name, kind, parameters: [] }` and nothing else set.
    pub fn new(ref_: LiveRef, name: &str, kind: DeviceKind) -> Device {
        Device {
            ref_,
            parent_ref: None,
            name: name.to_string(),
            kind,
            parameters: Vec::new(),
            object_identity: None,
            enabled: None,
            class_name: None,
            can_have_chains: Maybe::Absent,
            can_have_drum_pads: Maybe::Absent,
            chains: None,
            drum_pads: None,
            macros: None,
            variation_count: None,
            chain_selector: Maybe::Absent,
            view: None,
            latency_samples: Maybe::Absent,
            latency_ms: Maybe::Absent,
            parameter_bank: Maybe::Absent,
            comparison: None,
            return_chains: None,
            visible_macro_count: Maybe::Absent,
            drift: None,
            eq8: None,
            hybrid_reverb: None,
            meld: None,
            drum_cell: None,
            looper: None,
            plugin: None,
            max_device: None,
            selected_variation_index: Maybe::Absent,
            macro_mapped: Maybe::Absent,
            rack_view: None,
            extra: Map::new(),
        }
    }

    /// A device-family row by its key: a declared family (`drift`, `looper`...) or one kept in `extra`.
    pub fn family_row(&self, key: &str) -> Option<&Map<String, Value>> {
        match key {
            "drift" => self.drift.as_ref(),
            "eq8" => self.eq8.as_ref(),
            "hybridReverb" => self.hybrid_reverb.as_ref(),
            "meld" => self.meld.as_ref(),
            "drumCell" => self.drum_cell.as_ref(),
            "looper" => self.looper.as_ref(),
            "plugin" => self.plugin.as_ref(),
            "maxDevice" => self.max_device.as_ref(),
            other => self.extra.get(other).and_then(Value::as_object),
        }
    }

    pub fn family_row_mut(&mut self, key: &str) -> Option<&mut Map<String, Value>> {
        match key {
            "drift" => self.drift.as_mut(),
            "eq8" => self.eq8.as_mut(),
            "hybridReverb" => self.hybrid_reverb.as_mut(),
            "meld" => self.meld.as_mut(),
            "drumCell" => self.drum_cell.as_mut(),
            "looper" => self.looper.as_mut(),
            "plugin" => self.plugin.as_mut(),
            "maxDevice" => self.max_device.as_mut(),
            other => self.extra.get_mut(other).and_then(Value::as_object_mut),
        }
    }

    /// `device[key] ??= {}` for a declared family row.
    pub fn family_row_or_insert(&mut self, key: &str) -> &mut Map<String, Value> {
        match key {
            "drift" => self.drift.get_or_insert_with(Map::new),
            "eq8" => self.eq8.get_or_insert_with(Map::new),
            "hybridReverb" => self.hybrid_reverb.get_or_insert_with(Map::new),
            "meld" => self.meld.get_or_insert_with(Map::new),
            "drumCell" => self.drum_cell.get_or_insert_with(Map::new),
            "looper" => self.looper.get_or_insert_with(Map::new),
            "plugin" => self.plugin.get_or_insert_with(Map::new),
            "maxDevice" => self.max_device.get_or_insert_with(Map::new),
            other => {
                let entry = self.extra.entry(other.to_string()).or_insert_with(|| Value::Object(Map::new()));
                if !entry.is_object() {
                    *entry = Value::Object(Map::new());
                }
                entry.as_object_mut().expect("an object")
            }
        }
    }
}

string_enum! {
    ClipKind { Midi = "midi", Audio = "audio" }
}

pub type ClipGroove = ObservedObject<ClipGrooveFields>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipGrooveFields {
    #[serde(rename = "ref", default, skip_serializing_if = "Maybe::is_absent")]
    pub ref_: Maybe<Value>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub name: Maybe<Value>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarpMarker {
    pub beat_time: f64,
    pub sample_time: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipView {
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub grid_quantization: Maybe<Value>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub grid_is_triplet: Maybe<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub name: String,
    pub kind: ClipKind,
    pub start: f64,
    pub length: f64,
    pub notes: Vec<ObservedObject<Note>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_revision: Option<String>,
    // The Remote Script doesn't send these three; only the simulator does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warp: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<Vec<AutomationPoint>>,
    /// Envelopes by parameter reference, each a list of automation points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelopes: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_audio: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub gain: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub pitch_coarse: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub pitch_fine: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub warp_mode: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub warping: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fade_in_length: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fade_out_length: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_audio_fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub loop_start: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub loop_end: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub file_path: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub muted: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub color_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub looping: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_take_lane_clip: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub groove: Maybe<ClipGroove>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub has_groove: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub launch_mode: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub launch_quantization: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub legato: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub playing_position: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_playing: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_triggered: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_recording: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub ram_mode: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub signature_numerator: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub signature_denominator: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub velocity_amount: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub will_record_on_start: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fire_button_state: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub end_time: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub available_warp_modes: Maybe<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub sample_length: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub warp_markers: Maybe<Vec<WarpMarker>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_view: Option<ClipView>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Clip {
    /// A bare clip row: `{ ref, name, kind, start, length, notes: [], warp, takes: [], automation: [] }`.
    pub fn new(ref_: LiveRef, name: &str, kind: ClipKind, start: f64, length: f64) -> Clip {
        Clip {
            ref_,
            object_identity: None,
            name: name.to_string(),
            kind,
            start,
            length,
            notes: Vec::new(),
            notes_revision: None,
            warp: Some(false),
            takes: Some(Vec::new()),
            automation: Some(Vec::new()),
            envelopes: None,
            is_audio: Maybe::Absent,
            gain: Maybe::Absent,
            pitch_coarse: Maybe::Absent,
            pitch_fine: Maybe::Absent,
            warp_mode: Maybe::Absent,
            warping: Maybe::Absent,
            fade_in_length: Maybe::Absent,
            fade_out_length: Maybe::Absent,
            available_audio_fields: None,
            loop_start: Maybe::Absent,
            loop_end: Maybe::Absent,
            file_path: Maybe::Absent,
            muted: Maybe::Absent,
            color_index: Maybe::Absent,
            looping: Maybe::Absent,
            is_take_lane_clip: Maybe::Absent,
            groove: Maybe::Absent,
            has_groove: Maybe::Absent,
            launch_mode: Maybe::Absent,
            launch_quantization: Maybe::Absent,
            legato: Maybe::Absent,
            playing_position: Maybe::Absent,
            is_playing: Maybe::Absent,
            is_triggered: Maybe::Absent,
            is_recording: Maybe::Absent,
            ram_mode: Maybe::Absent,
            signature_numerator: Maybe::Absent,
            signature_denominator: Maybe::Absent,
            velocity_amount: Maybe::Absent,
            will_record_on_start: Maybe::Absent,
            fire_button_state: Maybe::Absent,
            end_time: Maybe::Absent,
            available_warp_modes: Maybe::Absent,
            sample_length: Maybe::Absent,
            warp_markers: Maybe::Absent,
            clip_view: None,
            extra: Map::new(),
        }
    }

    /// The envelope at `parameter_ref`, if the clip has one (its points, parsed).
    pub fn envelope(&self, parameter_ref: &str) -> Option<Vec<AutomationPoint>> {
        self.envelopes.as_ref()?.get(parameter_ref).map(|points| serde_json::from_value(points.clone()).unwrap_or_default())
    }

    /// `clip.envelopes[parameterRef] = points`.
    pub fn set_envelope(&mut self, parameter_ref: &str, points: &[AutomationPoint]) {
        self.envelopes
            .get_or_insert_with(Map::new)
            .insert(parameter_ref.to_string(), serde_json::to_value(points).unwrap_or(Value::Array(Vec::new())));
    }

    /// The clip as a JSON row.
    pub fn to_row(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// `(clip as Record<string, unknown>)[field]`: a field by its wire name, absent when unset.
    pub fn field(&self, name: &str) -> Option<Value> {
        self.to_row().as_object().and_then(|row| row.get(name).cloned())
    }
}

/// Typed access to an observed JSON object, retaining its original property order and explicit nulls.
/// Audio diagnosis binds the complete mixer/routing evidence with JavaScript JSON.stringify.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedObject<T> {
    value: T,
    original: Map<String, Value>,
    preserved_nulls: HashSet<String>,
}
impl<T> std::ops::Deref for ObservedObject<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T> std::ops::DerefMut for ObservedObject<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}
impl<'de, T: serde::de::DeserializeOwned + Serialize> Deserialize<'de> for ObservedObject<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let original = Map::<String, Value>::deserialize(deserializer)?;
        let value: T = serde_json::from_value(Value::Object(original.clone())).map_err(serde::de::Error::custom)?;
        let represented = serde_json::to_value(&value).map_err(serde::de::Error::custom)?;
        let preserved_nulls =
            original.iter().filter(|(key, value)| value.is_null() && represented.get(*key).is_none()).map(|(key, _)| key.clone()).collect();
        Ok(Self { value, original, preserved_nulls })
    }
}
impl<T: Serialize> Serialize for ObservedObject<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let Value::Object(mut current) = serde_json::to_value(&self.value).map_err(serde::ser::Error::custom)? else {
            return Err(serde::ser::Error::custom("observed evidence must be an object"));
        };
        let mut map = serializer.serialize_map(None)?;
        for (key, original) in &self.original {
            if let Some(value) = current.shift_remove(key) {
                map.serialize_entry(key, &value)?;
            } else if original.is_null() && self.preserved_nulls.contains(key) {
                map.serialize_entry(key, original)?;
            }
        }
        for (key, value) in current {
            // An absent optional field deserializes to None, which must not invent a null field.
            if !value.is_null() {
                map.serialize_entry(&key, &value)?;
            }
        }
        map.end()
    }
}

pub type RoutingState = ObservedObject<RoutingStateFields>;
pub type MixerState = ObservedObject<MixerStateFields>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingStateFields {
    pub input_type: Option<String>,
    pub input_sub_routing: Option<String>,
    pub output_type: Option<String>,
    pub output_sub_routing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_input_types: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_input_channels: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_output_types: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_output_channels: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixerStateFields {
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub cue_volume: Option<f64>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
    pub sends: Vec<Option<f64>>,
    /// Live's own text for each value ("-3.2 dB", "25L"), when the adapter provides it.
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub volume_display: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub pan_display: Maybe<String>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub cue_volume_display: Maybe<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_displays: Option<Vec<Option<String>>>,
    pub volume_ref: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub volume_identity: Maybe<String>,
    pub pan_ref: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub pan_identity: Maybe<String>,
    pub cue_ref: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub cue_identity: Maybe<String>,
    pub send_refs: Vec<LiveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_identities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixer_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_activator: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crossfader: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panning_left: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panning_right: Option<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub track_activator_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub crossfader_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub crossfade_assign: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub panning_mode: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub panning_left_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub panning_right_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub song_tempo_ref: Maybe<LiveRef>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipSlot {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    pub parent_ref: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub scene_index: usize,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub clip_ref: Maybe<LiveRef>,
    pub empty: bool,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub color_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub controls_other_clips: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub has_stop_button: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_group_slot: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub playing_status: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub will_record_on_start: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fire_button_state: Maybe<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ClipSlot {
    /// An empty slot: `{ ref, parentRef, objectIdentity, sceneIndex, clipRef: null, empty: true }`.
    pub fn empty(ref_: LiveRef, parent_ref: LiveRef, object_identity: &str, scene_index: usize) -> ClipSlot {
        ClipSlot {
            ref_,
            parent_ref,
            object_identity: Some(object_identity.to_string()),
            scene_index,
            clip_ref: Maybe::Null,
            empty: true,
            color_index: Maybe::Absent,
            controls_other_clips: Maybe::Absent,
            has_stop_button: Maybe::Absent,
            is_group_slot: Maybe::Absent,
            playing_status: Maybe::Absent,
            will_record_on_start: Maybe::Absent,
            fire_button_state: Maybe::Absent,
            extra: Map::new(),
        }
    }

    /// The clip in the slot, if any (`slot.clipRef`, null or absent otherwise).
    pub fn clip(&self) -> Option<&LiveRef> {
        self.clip_ref.value()
    }
}

string_enum! {
    TrackKind { Audio = "audio", Midi = "midi", Group = "group", Return = "return", Main = "main", Master = "master", Regular = "regular" }
}

string_enum! {
    TrackMedia { Midi = "midi", Audio = "audio" }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackView {
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub selected_device_ref: Maybe<Value>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub device_insert_mode: Maybe<Value>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_collapsed: Maybe<Value>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_showing_chains: Maybe<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A track row. A focused read lists the tracks outside its focus as light rows (`light: true`): identity,
/// name, kind, arm, colour and group only, with empty clips, slots, devices and lanes and no mixer or routing.
/// `volume`, `pan`, `mute`, `solo` and `sends` are absent on a light row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_kind: Option<TrackMedia>,
    pub name: String,
    pub kind: TrackKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
    pub armed: Option<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub monitoring_state: Maybe<LiveMonitoringState>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub playing_slot_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fired_slot_index: Maybe<i64>,
    pub clips: Vec<Clip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_slots: Option<Vec<ClipSlot>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub mixer: Maybe<MixerState>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub routing: Maybe<RoutingState>,
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sends: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_lanes: Option<Vec<TakeLane>>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub group_track_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_visible: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_selected: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_frozen: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fold_state: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub implicit_arm: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub back_to_arranger: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub muted_via_solo: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub color_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub color: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub input_meter_left: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub input_meter_right: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub input_meter_level: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub output_meter_left: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub output_meter_right: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub output_meter_level: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub performance_impact: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<TrackView>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Track {
    /// A bare track row: `{ ref, name, kind, volume, pan, mute, solo, armed, clips: [], devices: [], sends }`.
    pub fn new(ref_: LiveRef, name: &str, kind: TrackKind) -> Track {
        Track {
            ref_,
            object_identity: None,
            light: None,
            parent_ref: None,
            media_kind: None,
            name: name.to_string(),
            kind,
            volume: Some(0.85),
            pan: Some(0.0),
            mute: Some(false),
            solo: Some(false),
            armed: Some(false),
            monitoring_state: Maybe::Absent,
            playing_slot_index: Maybe::Absent,
            fired_slot_index: Maybe::Absent,
            clips: Vec::new(),
            clip_slots: None,
            mixer: Maybe::Absent,
            routing: Maybe::Absent,
            devices: Vec::new(),
            sends: Some(vec![0.0, 0.0]),
            input: None,
            output: None,
            take_lanes: None,
            group_track_ref: Maybe::Absent,
            is_visible: Maybe::Absent,
            is_selected: Maybe::Absent,
            is_frozen: Maybe::Absent,
            fold_state: Maybe::Absent,
            implicit_arm: Maybe::Absent,
            back_to_arranger: Maybe::Absent,
            muted_via_solo: Maybe::Absent,
            color_index: Maybe::Absent,
            color: Maybe::Absent,
            input_meter_left: Maybe::Absent,
            input_meter_right: Maybe::Absent,
            input_meter_level: Maybe::Absent,
            output_meter_left: Maybe::Absent,
            output_meter_right: Maybe::Absent,
            output_meter_level: Maybe::Absent,
            performance_impact: Maybe::Absent,
            view: None,
            extra: Map::new(),
        }
    }

    pub fn is_light(&self) -> bool {
        self.light == Some(true)
    }

    /// `track.clipSlots ?? []`.
    pub fn slots(&self) -> &[ClipSlot] {
        self.clip_slots.as_deref().unwrap_or(&[])
    }

    /// `track.takeLanes ?? []`.
    pub fn lanes(&self) -> &[TakeLane] {
        self.take_lanes.as_deref().unwrap_or(&[])
    }

    /// The track as a JSON row.
    pub fn to_row(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeLane {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_ref: Option<LiveRef>,
    pub name: String,
    pub index: usize,
    pub clips: Vec<Clip>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub name: String,
    pub index: usize,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub color_index: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_empty: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub is_triggered: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub tempo: Maybe<f64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub tempo_enabled: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub signature_numerator: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub signature_denominator: Maybe<i64>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub time_signature_enabled: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub fire_button_state: Maybe<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggerable: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Scene {
    /// A bare scene row: `{ ref, objectIdentity, name, index }`.
    pub fn new(ref_: LiveRef, object_identity: &str, name: &str, index: usize) -> Scene {
        Scene {
            ref_,
            object_identity: Some(object_identity.to_string()),
            name: name.to_string(),
            index,
            color_index: Maybe::Absent,
            is_empty: Maybe::Absent,
            is_triggered: Maybe::Absent,
            tempo: Maybe::Absent,
            tempo_enabled: Maybe::Absent,
            signature_numerator: Maybe::Absent,
            signature_denominator: Maybe::Absent,
            time_signature_enabled: Maybe::Absent,
            fire_button_state: Maybe::Absent,
            triggerable: None,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLoop {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
}

/// The Set row (`snapshot.set`), with whatever else Live says of it in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSet {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "loop")]
    pub loop_: Option<SetLoop>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Locator {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub name: String,
    pub position: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrangement {
    // The Remote Script's snapshot has no Arrangement length; only the simulator sends one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator_revision: Option<String>,
    pub locators: Vec<Locator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<Map<String, Value>>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrangementClipEntry {
    pub clip: Clip,
    pub track_ref: LiveRef,
}

string_enum! {
    BrowserEntryKind { Device = "device", Sample = "sample", Preset = "preset" }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEntry {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    pub name: String,
    pub kind: BrowserEntryKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveView {
    pub visible_view: Option<String>,
    pub follow: Option<bool>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub draw_mode: Maybe<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub track_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub scene_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub slot_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub detail_clip_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub device_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub parameter_ref: Maybe<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub chain_ref: Maybe<LiveRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quantization {
    pub name: String,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSongState {
    pub visible_tracks: Vec<LiveRef>,
    pub appointed_device: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Maybe::is_absent")]
    pub select_on_launch: Maybe<bool>,
    pub song_length: Option<f64>,
    pub start_time: Option<f64>,
    pub signature_numerator: Option<i64>,
    pub signature_denominator: Option<i64>,
    pub swing_amount: Option<f64>,
    pub overdub: Option<bool>,
    pub arrangement_overdub: Option<bool>,
    pub back_to_arranger: Option<bool>,
    pub can_capture_midi: Option<bool>,
    pub can_undo: Option<bool>,
    pub can_redo: Option<bool>,
    pub exclusive_arm: Option<bool>,
    pub exclusive_solo: Option<bool>,
    pub is_counting_in: Option<bool>,
    pub tempo_follower_enabled: Option<bool>,
    pub re_enable_automation_enabled: Option<bool>,
    pub session_record: Option<bool>,
    pub session_automation_record: Option<bool>,
    pub clip_trigger_quantization: Option<Quantization>,
    pub midi_recording_quantization: Option<Quantization>,
    pub is_ableton_link_enabled: Option<bool>,
    pub is_ableton_link_start_stop_sync_enabled: Option<bool>,
    pub tempo_follower: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTuning {
    pub note: i64,
    pub deviation: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TuningSystem {
    pub name: String,
    pub lowest_note: Option<Map<String, Value>>,
    pub highest_note: Option<Map<String, Value>>,
    pub reference_pitch: Option<Map<String, Value>>,
    pub pseudo_octave_in_cents: Option<f64>,
    pub note_tunings: Vec<NoteTuning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scale {
    pub root_note: Option<i64>,
    pub scale_name: Option<String>,
    pub scale_mode: Option<bool>,
    pub scale_intervals: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tuning {
    pub system: TuningSystem,
    pub scale: Scale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Groove {
    #[serde(rename = "ref")]
    pub ref_: LiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<String>,
    pub name: String,
    pub base: Option<f64>,
    pub quantization_amount: Option<f64>,
    pub random_amount: Option<f64>,
    pub timing_amount: Option<f64>,
    pub velocity_amount: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroovePool {
    pub amount: Option<f64>,
    pub grooves: Vec<Groove>,
}

string_enum! {
    /// The top-level parts a snapshot read can be limited to (the epoch always comes).
    LiveSnapshotPart { Set = "set", Tracks = "tracks", Scenes = "scenes", Arrangement = "arrangement", Playback = "playback", Selection = "selection" }
}

pub const LIVE_SNAPSHOT_PARTS: &[LiveSnapshotPart] = LiveSnapshotPart::ALL;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshotWindow {
    pub from: usize,
    pub count: usize,
}

/// What one snapshot read builds, so that a read costs what an operation touches instead of the whole Set.
/// Empty: the whole Set. `tracks`/`scenes`: only those whole rows, the track index running over regular and
/// group tracks, then returns, then main. `focus`: every track in order, whole for the listed indices and
/// light for the rest, with the Arrangement clips of the focus tracks only. `parts`: only those top-level
/// parts; the others are absent. The result's `window` says what was honoured. A Remote Script from before
/// these arguments answers with the whole Set whatever is asked (and no `window`), so a request only ever
/// makes a read cheaper: nothing may rely on a row being light.
///
/// The same shape is a snapshot's `window`: what the Remote Script honoured of the read's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshotRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<LiveSnapshotWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenes: Option<LiveSnapshotWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<LiveSnapshotPart>>,
}

impl LiveSnapshotRequest {
    /// `Object.keys(request).length === 0`: a read of the whole Set.
    pub fn is_empty(&self) -> bool {
        self.tracks.is_none() && self.scenes.is_none() && self.focus.is_none() && self.parts.is_none()
    }
    pub fn focused(focus: Vec<usize>) -> LiveSnapshotRequest {
        LiveSnapshotRequest { focus: Some(focus), ..Default::default() }
    }
    pub fn of_parts(parts: Vec<LiveSnapshotPart>) -> LiveSnapshotRequest {
        LiveSnapshotRequest { parts: Some(parts), ..Default::default() }
    }
    pub fn track_window(from: usize, count: usize) -> LiveSnapshotRequest {
        LiveSnapshotRequest { tracks: Some(LiveSnapshotWindow { from, count }), ..Default::default() }
    }
}

/// The whole Set as the simulator keeps it and as a read returns it (every part optional: a read limited to
/// `parts` leaves the others out, and `epoch`, `trackCount`, `sceneCount` and `window` come with a read).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<LiveSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<Track>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenes: Option<Vec<Scene>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<Arrangement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement_clips: Option<Vec<ArrangementClipEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<Vec<BrowserEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<SessionPlaybackState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<LiveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<LiveView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song: Option<LiveSongState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groove_pool: Option<GroovePool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i64>,
    /// How many tracks and scenes the Set holds, whatever the read returned of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_count: Option<usize>,
    /// What the Remote Script honoured of the read's arguments; absent, the read was of the whole Set (what a
    /// Remote Script from before the arguments returns whatever it is asked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<LiveSnapshotRequest>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl LiveSnapshot {
    /// `snapshot.tracks`, or none when the read left them out.
    pub fn tracks(&self) -> &[Track] {
        self.tracks.as_deref().unwrap_or(&[])
    }
    pub fn tracks_mut(&mut self) -> &mut Vec<Track> {
        self.tracks.get_or_insert_with(Vec::new)
    }
    pub fn scenes(&self) -> &[Scene] {
        self.scenes.as_deref().unwrap_or(&[])
    }
    pub fn scenes_mut(&mut self) -> &mut Vec<Scene> {
        self.scenes.get_or_insert_with(Vec::new)
    }
    /// `snapshot.arrangementClips ?? []`.
    pub fn arrangement_clips(&self) -> &[ArrangementClipEntry] {
        self.arrangement_clips.as_deref().unwrap_or(&[])
    }
    /// `snapshot.arrangement?.clips ?? []`.
    pub fn arrangement_clip_rows(&self) -> &[Map<String, Value>] {
        self.arrangement.as_ref().and_then(|arrangement| arrangement.clips.as_deref()).unwrap_or(&[])
    }
    /// Whether the answer holds `part` (`part in value`).
    pub fn has_part(&self, part: LiveSnapshotPart) -> bool {
        match part {
            LiveSnapshotPart::Set => self.set.is_some(),
            LiveSnapshotPart::Tracks => self.tracks.is_some(),
            LiveSnapshotPart::Scenes => self.scenes.is_some(),
            LiveSnapshotPart::Arrangement => self.arrangement.is_some(),
            LiveSnapshotPart::Playback => self.playback.is_some(),
            LiveSnapshotPart::Selection => self.selection.is_some(),
        }
    }
}

/// The largest track or scene index a read may name (the registry's bound).
pub const MAX_SNAPSHOT_INDEX: usize = 100_000;

/// A request as the registry bounds it; anything else is refused before it is read.
pub fn validate_snapshot_request(request: &LiveSnapshotRequest) -> Result<(), LiveError> {
    let index = |value: usize| value <= MAX_SNAPSHOT_INDEX;
    for window in [&request.tracks, &request.scenes].into_iter().flatten() {
        if !index(window.from) || !index(window.count) || window.count < 1 {
            return Err(LiveError::RangeError("snapshot window is invalid".into()));
        }
    }
    if let Some(focus) = &request.focus {
        if !focus.iter().all(|value| index(*value)) || focus.iter().collect::<HashSet<_>>().len() != focus.len() {
            return Err(LiveError::RangeError("snapshot focus is invalid".into()));
        }
    }
    if let Some(parts) = &request.parts {
        if parts.len() > LIVE_SNAPSHOT_PARTS.len() || parts.iter().collect::<HashSet<_>>().len() != parts.len() {
            return Err(LiveError::RangeError("snapshot parts are invalid".into()));
        }
    }
    Ok(())
}

/// A request given as JSON (a tool's arguments, a wire frame), checked as the TypeScript checked the raw
/// object: only the four keys, integer indices within bounds, known parts.
pub fn snapshot_request_from_value(value: &Value) -> Result<LiveSnapshotRequest, LiveError> {
    let object = match value {
        Value::Object(object) if object.keys().all(|key| ["tracks", "scenes", "focus", "parts"].contains(&key.as_str())) => object,
        _ => return Err(LiveError::RangeError("snapshot request is invalid".into())),
    };
    let index = |value: Option<&Value>| -> Option<usize> {
        let number = value?.as_f64()?;
        (number.fract() == 0.0 && number >= 0.0 && number <= MAX_SNAPSHOT_INDEX as f64).then_some(number as usize)
    };
    let mut request = LiveSnapshotRequest::default();
    for (key, slot) in [("tracks", &mut request.tracks), ("scenes", &mut request.scenes)] {
        if let Some(window) = object.get(key) {
            let parsed = window
                .as_object()
                .filter(|window| window.keys().all(|key| key == "from" || key == "count"))
                .and_then(|window| Some(LiveSnapshotWindow { from: index(window.get("from"))?, count: index(window.get("count"))? }))
                .filter(|window| window.count >= 1);
            match parsed {
                Some(window) => *slot = Some(window),
                None => return Err(LiveError::RangeError("snapshot window is invalid".into())),
            }
        }
    }
    if let Some(focus) = object.get("focus") {
        let parsed = focus.as_array().and_then(|items| items.iter().map(|item| index(Some(item))).collect::<Option<Vec<usize>>>());
        match parsed {
            Some(items) if items.iter().collect::<HashSet<_>>().len() == items.len() => request.focus = Some(items),
            _ => return Err(LiveError::RangeError("snapshot focus is invalid".into())),
        }
    }
    if let Some(parts) = object.get("parts") {
        let parsed = parts
            .as_array()
            .and_then(|items| items.iter().map(|item| item.as_str().and_then(LiveSnapshotPart::parse)).collect::<Option<Vec<_>>>());
        match parsed {
            Some(items) if items.len() <= LIVE_SNAPSHOT_PARTS.len() && items.iter().collect::<HashSet<_>>().len() == items.len() => {
                request.parts = Some(items)
            }
            _ => return Err(LiveError::RangeError("snapshot parts are invalid".into())),
        }
    }
    Ok(request)
}

/// The parts a whole-Set answer holds.
const WHOLE_SET_PARTS: [LiveSnapshotPart; 5] =
    [LiveSnapshotPart::Set, LiveSnapshotPart::Tracks, LiveSnapshotPart::Scenes, LiveSnapshotPart::Arrangement, LiveSnapshotPart::Playback];

/// A snapshot answer checked against what was asked, now that the protocol leaves every part optional.
/// Without a `window` it is the whole Set: every part, every row whole (what a Remote Script from before
/// snapshot arguments answers to anything). With one, the window may only echo what was asked; the answer
/// holds exactly the parts it names (all of them when it names none), a focus lists every track with only
/// the focus tracks whole, and a window holds only its rows. Anything else is refused here, never passed on
/// as a snapshot.
pub fn check_snapshot_answer(answer: LiveSnapshot, request: &LiveSnapshotRequest) -> Result<LiveSnapshot, LiveError> {
    let err = |text: String| Err(LiveError::Error(text));
    let rows = answer.tracks.as_deref();
    let Some(window) = &answer.window else {
        let missing: Vec<&str> = WHOLE_SET_PARTS.iter().filter(|part| !answer.has_part(**part)).map(|part| part.as_str()).collect();
        if !missing.is_empty() {
            return err(format!("snapshot answer without a window isn't the whole Set: it lacks {}", missing.join(", ")));
        }
        if rows.map(|rows| rows.iter().any(Track::is_light)).unwrap_or(true) {
            return err("snapshot answer without a window isn't the whole Set: its tracks aren't all whole".into());
        }
        return Ok(answer);
    };
    for (key, honoured, asked) in [
        ("tracks", window.tracks.is_some(), request.tracks.is_some()),
        ("scenes", window.scenes.is_some(), request.scenes.is_some()),
        ("focus", window.focus.is_some(), request.focus.is_some()),
        ("parts", window.parts.is_some(), request.parts.is_some()),
    ] {
        if honoured && !asked {
            return err(format!("snapshot window says it honoured {key}, which wasn't asked"));
        }
    }
    if let Some(parts) = &window.parts {
        let asked = request.parts.as_deref().unwrap_or(&[]);
        if parts.len() != asked.len() || !parts.iter().all(|part| asked.contains(part)) {
            return err("snapshot window's parts aren't the parts asked".into());
        }
    }
    if let Some(focus) = &window.focus {
        let asked = request.focus.as_deref().unwrap_or(&[]);
        if !focus.iter().all(|index| asked.contains(index)) {
            return err("snapshot window's focus isn't the focus asked".into());
        }
    }
    for (key, honoured, asked) in [("tracks", &window.tracks, &request.tracks), ("scenes", &window.scenes, &request.scenes)] {
        if let Some(honoured) = honoured {
            let asked = asked.as_ref().copied().unwrap_or(LiveSnapshotWindow { from: usize::MAX, count: 0 });
            if honoured.from != asked.from || !(honoured.count >= 1 && honoured.count <= asked.count) {
                return err(format!("snapshot window's {key} aren't the {key} asked"));
            }
        }
    }
    for part in LIVE_SNAPSHOT_PARTS {
        let listed = match &window.parts {
            Some(parts) => parts.contains(part),
            None => *part != LiveSnapshotPart::Selection,
        };
        if listed && !answer.has_part(*part) {
            return err(format!("snapshot answer lacks its {part}"));
        }
        if window.parts.is_some() && !listed && answer.has_part(*part) {
            return err(format!("snapshot answer holds {part}, which wasn't asked"));
        }
    }
    let counted =
        |count: Option<usize>, from: usize, size: usize| -> Option<usize> { count.map(|count| count.saturating_sub(from).min(size)) };
    if let Some(rows) = rows {
        let from = window.tracks.map(|window| window.from).unwrap_or(0);
        let focus: Option<HashSet<usize>> = window.focus.as_ref().map(|focus| focus.iter().copied().collect());
        let expected = match (&window.tracks, &focus) {
            (Some(tracks), _) => counted(answer.track_count, from, tracks.count),
            (None, Some(_)) => answer.track_count,
            (None, None) => None,
        };
        if window.tracks.map(|tracks| rows.len() > tracks.count).unwrap_or(false)
            || expected.map(|expected| rows.len() != expected).unwrap_or(false)
        {
            return err("snapshot answer doesn't hold the track rows asked".into());
        }
        for (position, row) in rows.iter().enumerate() {
            let whole = focus.as_ref().map(|focus| focus.contains(&(from + position))).unwrap_or(true);
            if whole == row.is_light() {
                return err(format!(
                    "snapshot track {} is {}",
                    from + position,
                    if whole { "light, but was asked whole" } else { "whole, but was asked light" }
                ));
            }
        }
    }
    if let (Some(scenes), Some(rows)) = (&window.scenes, &answer.scenes) {
        let expected = counted(answer.scene_count, scenes.from, scenes.count);
        if rows.len() > scenes.count || expected.map(|expected| rows.len() != expected).unwrap_or(false) {
            return err("snapshot answer doesn't hold the scene rows asked".into());
        }
    }
    Ok(answer)
}

/// Whether a track plays audio or MIDI. The Remote Script's rows say what a track is (`kind`: regular, group,
/// return, main) apart from what it plays (`mediaKind`: audio, midi); older simulated rows said the second as
/// `kind`. None for a track that plays neither (a group).
pub fn track_media(track: &Track) -> Option<TrackMedia> {
    if track.kind == TrackKind::Group {
        return None;
    }
    if let Some(media) = track.media_kind {
        return Some(media);
    }
    match track.kind {
        TrackKind::Audio => Some(TrackMedia::Audio),
        TrackKind::Midi => Some(TrackMedia::Midi),
        _ => None,
    }
}

/// A track as a focused read lists it outside its focus: who it is, none of what it holds.
pub fn light_track_row(track: &Track, set_ref: Option<&LiveRef>) -> Track {
    Track {
        ref_: track.ref_.clone(),
        parent_ref: track.parent_ref.clone().or_else(|| set_ref.cloned()),
        object_identity: track.object_identity.clone(),
        name: track.name.clone(),
        kind: track.kind,
        media_kind: Some(track.media_kind.unwrap_or(if track.kind == TrackKind::Midi { TrackMedia::Midi } else { TrackMedia::Audio })),
        light: Some(true),
        armed: track.armed,
        color_index: Maybe::null_or(track.color_index.cloned()),
        group_track_ref: Maybe::null_or(track.group_track_ref.cloned()),
        clips: Vec::new(),
        clip_slots: Some(Vec::new()),
        devices: Vec::new(),
        take_lanes: Some(Vec::new()),
        mixer: Maybe::Null,
        routing: Maybe::Null,
        volume: None,
        pan: None,
        mute: None,
        solo: None,
        sends: None,
        monitoring_state: Maybe::Absent,
        playing_slot_index: Maybe::Absent,
        fired_slot_index: Maybe::Absent,
        input: None,
        output: None,
        is_visible: Maybe::Absent,
        is_selected: Maybe::Absent,
        is_frozen: Maybe::Absent,
        fold_state: Maybe::Absent,
        implicit_arm: Maybe::Absent,
        back_to_arranger: Maybe::Absent,
        muted_via_solo: Maybe::Absent,
        color: Maybe::Absent,
        input_meter_left: Maybe::Absent,
        input_meter_right: Maybe::Absent,
        input_meter_level: Maybe::Absent,
        output_meter_left: Maybe::Absent,
        output_meter_right: Maybe::Absent,
        output_meter_level: Maybe::Absent,
        performance_impact: Maybe::Absent,
        view: None,
        extra: Map::new(),
    }
}

string_enum! {
    /// Every event type: the Remote Script's (its _Subscription's, as the registry's subscribe types name them),
    /// Kumi's Live extension's `pointed` (a right-click on an object), and the simulator's own.
    LiveEventType {
        Transport = "transport", Object = "object", Reset = "reset", Selection = "selection", Name = "name", Mixer = "mixer", Parameter = "parameter", Structure = "structure",
        Pointed = "pointed", State = "state", Meter = "meter", Max = "max", Osc = "osc",
    }
}

/// What the Remote Script pushes to a subscription (its _Subscription), as the registry's subscribe types name them.
pub const REMOTE_SCRIPT_EVENT_TYPES: &[LiveEventType] = &[
    LiveEventType::Transport,
    LiveEventType::Object,
    LiveEventType::Reset,
    LiveEventType::Selection,
    LiveEventType::Name,
    LiveEventType::Mixer,
    LiveEventType::Parameter,
    LiveEventType::Structure,
];
/// Every event type: the Remote Script's, Kumi's Live extension's `pointed` (a right-click on an object), and the simulator's own.
pub const LIVE_EVENT_TYPES: &[LiveEventType] = LiveEventType::ALL;

string_enum! {
    LiveEventChannel { RemoteScript = "remote-script", Extension = "extension" }
}

/// One event from Live, numbered in its channel's own sequence; `coalesced` counts the events it stands for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEvent {
    pub epoch: i64,
    pub sequence: u64,
    #[serde(rename = "type")]
    pub event_type: LiveEventType,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ref")]
    pub ref_: Option<LiveRef>,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<LiveEventChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coalesced: Option<u64>,
}

/// Every operation the bridge invokes on Live (the registry's invoke operations, and the simulator's older ones).
pub const LIVE_OPERATIONS: &[&str] = &[
    "willington.device.read",
    "willington.device.set",
    "clip.follow-actions.set",
    "arrangement.clip.create",
    "arrangement.clip.delete",
    "arrangement.clip.move",
    "arrangement.audio-clip.create",
    "arrangement.automation.read",
    "arrangement.automation.create",
    "arrangement.automation.delete",
    "arrangement.automation.point.insert",
    "arrangement.automation.point.delete",
    "audio.capture.cleanup",
    "audio.capture.emergency-stop",
    "audio.capture.inspect",
    "audio.capture.start",
    "audio.capture.status",
    "audio.capture.stop",
    "audio.clip.set",
    "audio.warp-marker.read",
    "audio.warp-marker.add",
    "audio.warp-marker.move",
    "audio.warp-marker.delete",
    "audio.take-lane.read",
    "audio.comp.read",
    "automation.envelope.clear",
    "automation.envelope.create",
    "automation.envelope.delete",
    "automation.envelope.read",
    "automation.point.delete",
    "automation.point.insert",
    "browser.inspect",
    "browser.load",
    "ownership.settle",
    "browser.roots",
    "browser.search",
    "browser.preview.start",
    "browser.preview.stop",
    "chain.set",
    "clip.action",
    "clip.create",
    "drum-pad.delete-all-chains",
    "drum-pad.load-sample",
    "drum-pad.load-samples",
    "device.parameters.set",
    "drum-pad.set",
    "rack.action",
    "rack.set",
    "rack.view.set",
    "clip.delete",
    "clip.duplicate",
    "clip.move",
    "clip.rename",
    "clip.set",
    "application.dialog",
    "clip.view.set",
    "device.bank.set",
    "drift.set",
    "drum-cell.set",
    "eq8.set",
    "hybrid-reverb.set",
    "looper.action",
    "looper.set",
    "meld.set",
    "plugin.set",
    "simpler.replace-sample",
    "device.comparison.save-to-slot",
    "device.delete",
    "device.enable",
    "device.insert",
    "device.move",
    "device.parameter.set",
    "device.rename",
    "device.view.set",
    "observe.poll",
    "observe.subscribe",
    "observe.unsubscribe",
    "parameter.re-enable-automation",
    "selection.set",
    "song.view.set",
    "chain-mixer.set",
    "compressor.sidechain.set",
    "device-io.set",
    "locator.add",
    "locator.delete",
    "locator.jump",
    "locator.jump-to",
    "locator.rename",
    "mixer.extended.set",
    "mixer.set",
    "note.add",
    "note.add-batch",
    "note.delete",
    "note.duplicate",
    "note.quantize",
    "note.read-by-id",
    "note.read-selected",
    "note.update",
    "project.bounce",
    "project.collect",
    "project.export",
    "project.new",
    "project.open",
    "project.save",
    "project.save-as",
    "authority.digest",
    "dev.lom-audit",
    "undo.step.begin",
    "undo.step.end",
    "song.undo",
    "song.redo",
    "render.offline",
    "arrangement.midi-clip.create",
    "clip.clear-range",
    "device.duplicate",
    "drum-pad.sample-chain",
    "project.import",
    "transaction.group",
    "performance.read",
    "realtime.arm",
    "realtime.disarm",
    "realtime.stats",
    "recording.arrangement",
    "recording.session",
    "routing.set",
    "scene.capture",
    "scene.create",
    "scene.delete",
    "scene.fire-selected",
    "scene.rename",
    "scene.set",
    "session.audio-clip.create",
    "session.audition-launch",
    "session.audition-stop",
    "session.capture-midi",
    "session.clip-launch",
    "session.clip-stop",
    "session.discover",
    "session.emergency-stop",
    "song.read",
    "song.set",
    "song.time-convert",
    "scene.duplicate",
    "tempo.set",
    "track.create",
    "track.create-return",
    "track.delete",
    "track.delete-return",
    "track.duplicate",
    "track.rename",
    "track.select-instrument",
    "track.set",
    "track.view.set",
    "transport.action",
    "transport.set",
    "groove.edit",
    "groove.read",
    "groove.set",
    "take-lane.create",
    "take-lane.rename",
    "take-lane.clip.create",
    "take-lane.audio-clip.create",
    "tuning.read",
    "tuning.set",
    "view.control",
    "view.set",
    "subscribe",
    "application.message",
    "automation.step.insert",
    "automation.value-at",
    "clip.time-convert",
    "data.get",
    "data.set",
    "device.action",
    "device.banks.read",
    "device.property.set",
    "fire-button.set",
    "note.delete-range",
    "note.select",
    "plugin.parameter-names",
    "sample.set",
    "sample.slice",
    "track.action",
    "wavetable.modulation.set",
    "wavetable.set",
    "python.run",
];

/// An operation's name (one of [`LIVE_OPERATIONS`] in production; the simulator refuses any other).
pub type LiveOperation = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveInvocation {
    pub operation: LiveOperation,
    pub args: Map<String, Value>,
}

impl LiveInvocation {
    pub fn new(operation: &str, args: Value) -> LiveInvocation {
        LiveInvocation { operation: operation.to_string(), args: args.as_object().cloned().unwrap_or_default() }
    }
}

/// What the Live domain throws: an `Error`, a `TypeError` or a `RangeError` (the host tells the last apart
/// for capture failures), or a refusal before dispatch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveError {
    #[error("{0}")]
    Error(String),
    #[error("{0}")]
    TypeError(String),
    #[error("{0}")]
    RangeError(String),
    /// A mutation the bridge refused before dispatching it to Live (at its authority preflight or
    /// prepare, or for lacking cleanup ownership): nothing in Live changed.
    #[error("{0}")]
    MutationNotDispatched(String),
}

impl LiveError {
    pub fn message(&self) -> &str {
        match self {
            LiveError::Error(text) | LiveError::TypeError(text) | LiveError::RangeError(text) | LiveError::MutationNotDispatched(text) => {
                text
            }
        }
    }
    /// The JavaScript error's `name`.
    pub fn name(&self) -> &'static str {
        match self {
            LiveError::Error(_) => "Error",
            LiveError::TypeError(_) => "TypeError",
            LiveError::RangeError(_) => "RangeError",
            LiveError::MutationNotDispatched(_) => "LiveMutationNotDispatchedError",
        }
    }
    pub fn error(text: impl Into<String>) -> LiveError {
        LiveError::Error(text.into())
    }
    pub fn type_error(text: impl Into<String>) -> LiveError {
        LiveError::TypeError(text.into())
    }
    pub fn range_error(text: impl Into<String>) -> LiveError {
        LiveError::RangeError(text.into())
    }
}

impl From<RegistryError> for LiveError {
    fn from(error: RegistryError) -> LiveError {
        LiveError::Error(error.0)
    }
}
impl From<crate::follow_actions::FollowActionError> for LiveError {
    fn from(error: crate::follow_actions::FollowActionError) -> LiveError {
        LiveError::Error(error.0)
    }
}
impl From<serde_json::Error> for LiveError {
    fn from(error: serde_json::Error) -> LiveError {
        LiveError::Error(error.to_string())
    }
}
impl From<kumi_common::abort::Aborted> for LiveError {
    fn from(error: kumi_common::abort::Aborted) -> LiveError {
        LiveError::Error(error.to_string())
    }
}

pub type LiveListener = Rc<dyn Fn(&LiveEvent)>;
pub type Unsubscribe = Box<dyn Fn()>;
pub type StatusListener = Rc<dyn Fn(Option<&LiveStatus>)>;

pub trait LiveAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError>;
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError>;
    /// The object at `object_ref` as JSON, or none (`undefined`) when nothing is there.
    fn get(&self, object_ref: &LiveRef) -> Result<Option<Value>, LiveError>;
    fn invoke(&self, invocation: &LiveInvocation) -> Result<Value, LiveError>;
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError>;
    fn reconnect(&self) -> Result<LiveStatus, LiveError>;
}

/// Promise-based boundary used by process-backed adapters. Synchronous methods
/// remain available for deterministic in-process compatibility tests.
///
/// The methods past `close` are the ones the TypeScript adapters offered optionally (the host looks for
/// them at runtime): each has a `has_*` flag that says whether the adapter really implements it.
#[async_trait(?Send)]
pub trait AsyncLiveAdapter: LiveAdapter {
    /// A snapshot of the Set; `request` limits what is built (see LiveSnapshotRequest).
    async fn snapshot_async(
        &self,
        context: Option<&LiveOperationContext>,
        request: Option<&LiveSnapshotRequest>,
    ) -> Result<LiveSnapshot, LiveError>;
    async fn discover_async(
        &self,
        request: &LiveDiscoveryRequest,
        context: Option<&LiveOperationContext>,
    ) -> Result<LiveDiscoveryResult, LiveError>;
    async fn get_async(&self, object_ref: &LiveRef, context: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError>;
    async fn invoke_async(&self, invocation: &LiveInvocation, context: Option<&LiveOperationContext>) -> Result<Value, LiveError>;
    async fn reconnect_async(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError>;
    async fn close(&self) -> Result<(), LiveError>;

    /// `refreshStatusAsync`: a status read again from Live (the remote adapter and the router).
    fn has_refresh_status_async(&self) -> bool {
        false
    }
    async fn refresh_status_async(&self, _context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    /// `subscribeStatus`: told when the adapter's status changes shape.
    fn has_subscribe_status(&self) -> bool {
        false
    }
    fn subscribe_status(&self, _listener: StatusListener) -> Unsubscribe {
        Box::new(|| {})
    }
    /// `retireTransactionAsync(transactionId, context?, terminal = false)`: frees a transaction's replay ledger
    /// in the Remote Script; the result is `{ retired: number }`.
    fn has_retire_transaction_async(&self) -> bool {
        false
    }
    async fn retire_transaction_async(
        &self,
        _transaction_id: &str,
        _context: Option<&LiveOperationContext>,
        _terminal: bool,
    ) -> Result<Value, LiveError> {
        Err(LiveError::Error("retireTransactionAsync is unavailable".into()))
    }
    /// `retiresOnItsOwn`: the adapter retires changed transactions itself (single-tick mutations).
    fn retires_on_its_own(&self) -> bool {
        false
    }
    /// `expectStateDigest(transactionId, invocation)`: the state digest a preview fenced, kept for its change.
    fn has_expect_state_digest(&self) -> bool {
        false
    }
    fn expect_state_digest(&self, _transaction_id: &str, _invocation: &LiveInvocation) {}
}

/// Kinds of reference that name something outside every track (the Set, a scene, a locator...): reading
/// one needs no track rows.
const OUTSIDE_TRACK_KINDS: [&str; 9] =
    ["set", "scene", "locator", "groove", "session_playback", "session-playback", "selection", "browser_item", "browser-item"];
/// Kinds whose Remote Script path starts with their track's index.
const TRACK_PATH_KINDS: [&str; 11] = [
    "track",
    "clip_slot",
    "clip",
    "device",
    "chain",
    "drum_pad",
    "take_lane",
    "take_lane_clip",
    "arrangement_clip",
    "routing_choice",
    "parameter",
];

static REF_KIND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[0-9]+:)?([a-z_-]+):").expect("a valid pattern"));
static POSITIONAL_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:([a-z_]+):(.+)$").expect("a valid pattern"));
static NESTED_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:[a-z_]+:").expect("a valid pattern"));
static SHORT_INDEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]{1,6}$").expect("a valid pattern"));

/// The kind a reference names: `{epoch}:{kind}:{path}` from the Remote Script, `{kind}:{id}` from the simulator.
pub fn ref_kind(reference: &str) -> Option<&str> {
    REF_KIND.captures(reference).and_then(|captures| captures.get(1)).map(|kind| kind.as_str())
}

/// The track a positional Remote Script reference sits under, by the combined index (regular and group
/// tracks, then returns, then main): `{e}:track:3`, `{e}:clip_slot:3:5`, `{e}:clip:3:5`, `{e}:device:3:0:2`,
/// `{e}:chain:3:0`, `{e}:drum_pad:3:0:36`, `{e}:take_lane:3:1`, `{e}:take_lane_clip:3:1:0`,
/// `{e}:arrangement_clip:3:7`, `{e}:parameter:mixer:3:volume`, and a parameter or chain that names its
/// owner's reference (`{e}:parameter:{e}:device:3:0:2:5`, `{e}:parameter:{e}:chain:3:0:volume`).
/// None when the reference doesn't place itself on a track: the Set, a scene, a group-track or view
/// alias (`{e}:track:group:3`, `{e}:device:view:3`), a Set-level Arrangement clip, a simulator reference.
pub fn track_index_of_ref(reference: &str) -> Option<usize> {
    track_index_of_ref_at(reference, 0)
}

fn track_index_of_ref_at(reference: &str, depth: usize) -> Option<usize> {
    let captures = POSITIONAL_REF.captures(reference)?;
    if depth > 8 {
        return None;
    }
    let kind = captures.get(1)?.as_str();
    let path = captures.get(2)?.as_str();
    if NESTED_REF.is_match(path) {
        return if kind == "parameter" || kind == "chain" { track_index_of_ref_at(path, depth + 1) } else { None };
    }
    if !TRACK_PATH_KINDS.contains(&kind) {
        return None;
    }
    let parts: Vec<&str> = path.split(':').collect();
    let numeric = if kind == "parameter" {
        if parts.first() == Some(&"mixer") {
            parts.get(1).copied()
        } else {
            None
        }
    } else {
        parts.first().copied()
    };
    let numeric = numeric?;
    if !SHORT_INDEX.is_match(numeric) || (kind == "arrangement_clip" && parts.len() < 2) {
        return None;
    }
    let index: usize = numeric.parse().ok()?;
    (index <= MAX_SNAPSHOT_INDEX).then_some(index)
}

/// Every reference a whole track row owns: its own, its clips', slots', lanes', devices', parameters',
/// chains' and pads', and its mixers' parameters. References to other objects (a parent, a group) aren't.
pub fn refs_owned_by_track(track: &Value, owned: &mut dyn FnMut(&str)) {
    fn visit(value: &Value, mixer: bool, depth: usize, owned: &mut dyn FnMut(&str)) {
        if depth > 256 {
            return;
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, mixer, depth + 1, owned);
                }
            }
            Value::Object(object) => {
                for (key, item) in object {
                    if key == "ref" {
                        if let Value::String(text) = item {
                            owned(text);
                        }
                    } else if mixer && (key.ends_with("Ref") || key.ends_with("Refs")) && (item.is_string() || item.is_array()) {
                        let entries: Vec<&Value> = match item {
                            Value::Array(items) => items.iter().collect(),
                            other => vec![other],
                        };
                        for entry in entries {
                            if let Value::String(text) = entry {
                                owned(text);
                            }
                        }
                    } else if item.is_object() || item.is_array() {
                        visit(item, key == "mixer", depth + 1, owned);
                    }
                }
            }
            _ => {}
        }
    }
    visit(track, false, 0, owned);
}

/// Which tracks a view reads whole: the listed ones (every other one comes light), or all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveViewScope {
    Indices(Vec<usize>),
    All,
}

/// A whole-Set read is paged by this many tracks, so that no one request builds a whole big Set on Live's
/// UI thread; the page is what keeps Live responsive, not a bound on the Set.
pub const WHOLE_SET_PAGE_TRACKS: usize = 16;
const MAX_REMEMBERED_OWNERS: usize = 250_000;
/// How many pages one discovery may take to its end (a page holds one item at least).
const MAX_DISCOVERY_PAGES: usize = 1_000_000;

// --- views ---

/// Reads of Live shaped to what an operation touches, assembling bounded track windows when needed.
pub struct LiveViews {
    pub track_count: Cell<usize>,
    owners: RefCell<HashMap<String, usize>>,
    adapter: Rc<dyn Fn() -> Rc<dyn AsyncLiveAdapter>>,
}

struct ViewAssembly {
    tracks: Vec<Track>,
    clips: Vec<Map<String, Value>>,
    held: Vec<ArrangementClipEntry>,
}

enum FillResult {
    Complete,
    Retry,
    Whole(LiveSnapshot),
}

impl ViewAssembly {
    fn new(first: &LiveSnapshot) -> Self {
        let mut assembly = Self { tracks: first.tracks().to_vec(), clips: Vec::new(), held: Vec::new() };
        assembly.keep(first);
        assembly
    }
    fn keep(&mut self, snapshot: &LiveSnapshot) {
        for clip in snapshot.arrangement.as_ref().and_then(|a| a.clips.as_ref()).into_iter().flatten() {
            if !self.clips.iter().any(|known| known.get("ref") == clip.get("ref")) {
                self.clips.push(clip.clone());
            }
        }
        for item in snapshot.arrangement_clips.iter().flatten() {
            if !self.held.iter().any(|known| known.clip.ref_ == item.clip.ref_) {
                self.held.push(item.clone());
            }
        }
    }
    fn assembled(mut self, mut first: LiveSnapshot, wanted: Option<&[LiveSnapshotPart]>, focus: Option<&[usize]>) -> LiveSnapshot {
        let order: HashMap<_, _> = self.tracks.iter().enumerate().map(|(i, t)| (t.ref_.as_str(), i)).collect();
        self.clips
            .sort_by_key(|clip| clip.get("trackRef").and_then(Value::as_str).and_then(|key| order.get(key)).copied().unwrap_or(usize::MAX));
        self.held.sort_by_key(|item| order.get(item.track_ref.as_str()).copied().unwrap_or(usize::MAX));
        if wanted.is_none_or(|parts| parts.contains(&LiveSnapshotPart::Arrangement)) {
            if let Some(arrangement) = &mut first.arrangement {
                arrangement.clips = Some(self.clips);
            }
        }
        if first.arrangement_clips.is_some() {
            first.arrangement_clips = Some(self.held);
        }
        first.tracks = Some(self.tracks);
        if let (Some(focus), Some(window)) = (focus, &mut first.window) {
            window.focus = Some(focus.to_vec());
        } else {
            first.window = None;
        }
        first
    }
}

impl LiveViews {
    pub fn new(adapter: impl Fn() -> Rc<dyn AsyncLiveAdapter> + 'static) -> Self {
        Self { track_count: Cell::new(0), owners: RefCell::new(HashMap::new()), adapter: Rc::new(adapter) }
    }
    pub async fn view(
        &self,
        context: Option<&LiveOperationContext>,
        scope: LiveViewScope,
        parts: Option<&[LiveSnapshotPart]>,
    ) -> Result<LiveSnapshot, LiveError> {
        let LiveViewScope::Indices(mut focus) = scope else { return self.whole_set(context, parts).await };
        focus.retain(|index| *index <= MAX_SNAPSHOT_INDEX);
        focus.sort_unstable();
        focus.dedup();
        let wanted = parts.map(unique_parts);
        let adapter = (self.adapter)();
        if wanted
            .as_ref()
            .is_some_and(|parts| !parts.contains(&LiveSnapshotPart::Tracks) && !parts.contains(&LiveSnapshotPart::Arrangement))
        {
            return self.read(&*adapter, context, LiveSnapshotRequest { parts: wanted, ..Default::default() }).await;
        }
        for _ in 0..3 {
            let first = self
                .read(&*adapter, context, LiveSnapshotRequest { focus: Some(focus.clone()), parts: wanted.clone(), ..Default::default() })
                .await?;
            let honoured = first.window.as_ref().and_then(|window| window.focus.as_ref());
            if honoured.is_none()
                || first.tracks.is_none()
                || focus.iter().all(|index| honoured.unwrap().contains(index) || *index >= first.tracks().len())
            {
                return Ok(first);
            }
            let mut assembly = ViewAssembly::new(&first);
            match self.fill(&*adapter, context, &first, &mut assembly, &focus, &Self::page_parts(wanted.as_deref())).await? {
                FillResult::Retry => continue,
                FillResult::Complete => return Ok(assembly.assembled(first, wanted.as_deref(), Some(&focus))),
                FillResult::Whole(whole) => return Ok(whole),
            }
        }
        Err(LiveError::error("the Set's tracks kept changing while they were read; read them again"))
    }
    pub async fn view_for(
        &self,
        context: Option<&LiveOperationContext>,
        refs: &[Value],
        parts: Option<&[LiveSnapshotPart]>,
        indices: &[usize],
    ) -> Result<LiveSnapshot, LiveError> {
        if parts.is_some_and(|parts| !parts.contains(&LiveSnapshotPart::Tracks)) {
            return self.view(context, LiveViewScope::Indices(vec![]), parts).await;
        }
        let mut focus = indices.to_vec();
        let mut unplaced = Vec::new();
        let mut unknown = false;
        for reference in refs.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()) {
            if ref_kind(reference).is_some_and(|kind| OUTSIDE_TRACK_KINDS.contains(&kind)) {
                continue;
            }
            if let Some(index) = track_index_of_ref(reference) {
                focus.push(index);
                continue;
            }
            if let Some(index) = self.owners.borrow().get(reference) {
                focus.push(*index);
            } else {
                unknown = true;
            }
            unplaced.push(reference.to_string());
        }
        if unknown && focus.is_empty() {
            return self.whole_set(context, parts).await;
        }
        let snapshot = self.view(context, LiveViewScope::Indices(focus), parts).await?;
        if unplaced.is_empty() || Self::whole_rows_hold(&snapshot, &unplaced) {
            Ok(snapshot)
        } else {
            self.whole_set(context, parts).await
        }
    }
    pub async fn whole_set(
        &self,
        context: Option<&LiveOperationContext>,
        parts: Option<&[LiveSnapshotPart]>,
    ) -> Result<LiveSnapshot, LiveError> {
        let wanted = parts.map(unique_parts);
        let adapter = (self.adapter)();
        if wanted.as_ref().is_some_and(|parts| !parts.contains(&LiveSnapshotPart::Tracks)) {
            return self.read(&*adapter, context, LiveSnapshotRequest { parts: wanted, ..Default::default() }).await;
        }
        for _ in 0..3 {
            let first = self
                .read(
                    &*adapter,
                    context,
                    LiveSnapshotRequest { focus: Some((0..WHOLE_SET_PAGE_TRACKS).collect()), parts: wanted.clone(), ..Default::default() },
                )
                .await?;
            if first.window.as_ref().and_then(|window| window.focus.as_ref()).is_none() || first.tracks.is_none() {
                return Ok(first);
            }
            if first.track_count.is_some_and(|count| count != first.tracks().len()) {
                continue;
            }
            let mut assembly = ViewAssembly::new(&first);
            let indices = (0..assembly.tracks.len()).collect::<Vec<_>>();
            match self.fill(&*adapter, context, &first, &mut assembly, &indices, &Self::page_parts(wanted.as_deref())).await? {
                FillResult::Retry => continue,
                FillResult::Complete => return Ok(assembly.assembled(first, wanted.as_deref(), None)),
                FillResult::Whole(whole) => return Ok(whole),
            }
        }
        Err(LiveError::error("the Set's tracks kept changing while it was read; read it again"))
    }
    pub async fn discover_all(
        &self,
        request: &LiveDiscoveryRequest,
        context: Option<&LiveOperationContext>,
        most: Option<usize>,
    ) -> Result<Vec<Map<String, Value>>, LiveError> {
        let adapter = (self.adapter)();
        let mut request = request.clone();
        let mut revision = None;
        let mut items = Vec::new();
        for _ in 0..MAX_DISCOVERY_PAGES {
            let page = adapter.discover_async(&request, context).await?;
            if revision.as_ref().is_some_and(|revision| revision != &page.revision) {
                return Err(LiveError::error(format!("the {} list changed while it was read; read it again", request.kind)));
            }
            revision = Some(page.revision);
            items.extend(page.items);
            if most.is_some_and(|most| items.len() >= most) {
                items.truncate(most.unwrap());
                return Ok(items);
            }
            if page.next_cursor.as_ref().is_none_or(String::is_empty) {
                return Ok(items);
            }
            if page.next_cursor == request.cursor {
                return Err(LiveError::error(format!("the {} list's cursor didn't move on", request.kind)));
            }
            request.cursor = page.next_cursor;
        }
        Err(LiveError::error(format!("the {} list didn't end", request.kind)))
    }
    async fn read(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        context: Option<&LiveOperationContext>,
        request: LiveSnapshotRequest,
    ) -> Result<LiveSnapshot, LiveError> {
        let snapshot = adapter.snapshot_async(context, Some(&request)).await?;
        self.note(&snapshot);
        Ok(snapshot)
    }
    fn page_parts(wanted: Option<&[LiveSnapshotPart]>) -> Vec<LiveSnapshotPart> {
        wanted
            .unwrap_or(LIVE_SNAPSHOT_PARTS)
            .iter()
            .copied()
            .filter(|part| matches!(part, LiveSnapshotPart::Tracks | LiveSnapshotPart::Arrangement))
            .collect()
    }
    async fn fill(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        context: Option<&LiveOperationContext>,
        first: &LiveSnapshot,
        assembly: &mut ViewAssembly,
        indices: &[usize],
        parts: &[LiveSnapshotPart],
    ) -> Result<FillResult, LiveError> {
        let mut pending = indices.iter().copied().filter(|i| assembly.tracks.get(*i).is_some_and(Track::is_light)).collect::<Vec<_>>();
        pending.sort_unstable();
        pending.dedup();
        let mut at = 0;
        while at < pending.len() {
            let from = pending[at];
            let mut count = 1;
            while count < WHOLE_SET_PAGE_TRACKS && pending.get(at + count) == Some(&(from + count)) {
                count += 1;
            }
            let page = self
                .read(
                    adapter,
                    context,
                    LiveSnapshotRequest {
                        tracks: Some(LiveSnapshotWindow { from, count }),
                        parts: Some(parts.to_vec()),
                        ..Default::default()
                    },
                )
                .await?;
            let Some(window) = &page.window else {
                return Ok(FillResult::Whole(page));
            };
            let delivered = window.tracks.map(|window| window.count).unwrap_or(0);
            if window.tracks.map(|window| window.from) != Some(from)
                || page.tracks.is_none()
                || delivered < 1
                || delivered > count
                || page.tracks().len() != delivered
                || page.epoch != first.epoch
                || first.track_count.is_some_and(|n| page.track_count != Some(n))
                || page.scene_count != first.scene_count
            {
                return Ok(FillResult::Retry);
            }
            for (offset, row) in page.tracks().iter().enumerate() {
                let Some(listed) = assembly.tracks.get_mut(from + offset) else {
                    return Ok(FillResult::Retry);
                };
                if row.is_light() || listed.ref_ != row.ref_ || listed.object_identity != row.object_identity {
                    return Ok(FillResult::Retry);
                }
                *listed = row.clone();
            }
            assembly.keep(&page);
            at += delivered;
        }
        Ok(FillResult::Complete)
    }
    pub async fn playback(&self, context: Option<&LiveOperationContext>) -> Result<SessionPlaybackState, LiveError> {
        let result = (self.adapter)().discover_async(&LiveDiscoveryRequest::of(LiveDiscoveryKind::SessionPlayback), context).await?;
        result
            .items
            .first()
            .cloned()
            .and_then(|row| serde_json::from_value(Value::Object(row)).ok())
            .ok_or_else(|| LiveError::error("authoritative Session playback is unavailable"))
    }
    fn note(&self, snapshot: &LiveSnapshot) {
        if let Some(count) = snapshot.track_count {
            self.track_count.set(count);
        } else if snapshot.window.is_none() && !snapshot.tracks().is_empty() {
            self.track_count.set(snapshot.tracks().len());
        }
        let mut owners = self.owners.borrow_mut();
        if owners.len() > MAX_REMEMBERED_OWNERS {
            owners.clear();
        }
        let offset = snapshot.window.as_ref().and_then(|window| window.tracks).map(|window| window.from).unwrap_or(0);
        let mut index_of = HashMap::new();
        for (position, track) in snapshot.tracks().iter().enumerate() {
            if track_index_of_ref(&track.ref_).is_some() {
                continue;
            }
            let index = offset + position;
            index_of.insert(track.ref_.as_str(), index);
            owners.insert(track.ref_.to_string(), index);
            if !track.is_light() {
                refs_owned_by_track(&track.to_row(), &mut |reference| {
                    owners.insert(reference.to_string(), index);
                });
            }
        }
        for clip in snapshot.arrangement.as_ref().and_then(|a| a.clips.as_ref()).into_iter().flatten() {
            if let (Some(index), Some(reference)) =
                (clip.get("trackRef").and_then(Value::as_str).and_then(|key| index_of.get(key)), clip.get("ref").and_then(Value::as_str))
            {
                owners.insert(reference.to_string(), *index);
            }
        }
        for item in snapshot.arrangement_clips.iter().flatten() {
            if let Some(index) = index_of.get(item.track_ref.as_str()) {
                owners.insert(item.clip.ref_.to_string(), *index);
            }
        }
    }
    fn whole_rows_hold(snapshot: &LiveSnapshot, refs: &[String]) -> bool {
        let mut owned = HashSet::new();
        let mut whole = HashSet::new();
        for track in snapshot.tracks().iter().filter(|track| !track.is_light()) {
            whole.insert(track.ref_.as_str());
            refs_owned_by_track(&track.to_row(), &mut |reference| {
                owned.insert(reference.to_string());
            });
        }
        for clip in snapshot.arrangement.as_ref().and_then(|a| a.clips.as_ref()).into_iter().flatten() {
            if clip.get("trackRef").and_then(Value::as_str).is_some_and(|reference| whole.contains(reference)) {
                if let Some(reference) = clip.get("ref").and_then(Value::as_str) {
                    owned.insert(reference.to_string());
                }
            }
        }
        for item in snapshot.arrangement_clips.iter().flatten() {
            if whole.contains(item.track_ref.as_str()) {
                owned.insert(item.clip.ref_.to_string());
            }
        }
        refs.iter().all(|reference| owned.contains(reference))
    }
}

fn unique_parts(parts: &[LiveSnapshotPart]) -> Vec<LiveSnapshotPart> {
    let mut unique = Vec::new();
    for part in parts {
        if !unique.contains(part) {
            unique.push(*part);
        }
    }
    unique
}

#[derive(Debug, Clone, Default)]
pub struct UnavailableLiveAdapter;
impl LiveAdapter for UnavailableLiveAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(LiveStatus {
            connected: false,
            adapter: LiveAdapterKind::Unavailable,
            epoch: None,
            protocol: LIVE_PROTOCOL_VERSION.into(),
            capabilities: vec![],
            reason: Some("live-adapter-not-installed".into()),
            registry_hash: None,
            operations: None,
            provenance: None,
            willington_kinds: None,
            environment: None,
            extra: Map::new(),
        })
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        Err(LiveError::error("Live adapter unavailable"))
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        Err(LiveError::error("Live adapter unavailable"))
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("Live adapter unavailable"))
    }
    fn subscribe(&self, _: LiveListener) -> Result<Unsubscribe, LiveError> {
        Ok(Box::new(|| {}))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait(?Send)]
impl AsyncLiveAdapter for UnavailableLiveAdapter {
    async fn snapshot_async(&self, _: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.snapshot()
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("Live adapter unavailable"))
    }
    async fn get_async(&self, reference: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(reference)
    }
    async fn invoke_async(&self, invocation: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke(invocation)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}

const VOLATILE_TRACK_FIELDS: &[&str] = &[
    "armed",
    "implicitArm",
    "isSelected",
    "isVisible",
    "foldState",
    "view",
    "playingSlotIndex",
    "firedSlotIndex",
    "backToArranger",
    "mutedViaSolo",
    "performanceImpact",
    "inputMeterLeft",
    "inputMeterRight",
    "inputMeterLevel",
    "outputMeterLeft",
    "outputMeterRight",
    "outputMeterLevel",
];
const VOLATILE_SLOT_FIELDS: &[&str] = &["playingStatus", "willRecordOnStart", "fireButtonState"];
const VOLATILE_ROUTING_FIELDS: &[&str] =
    &["availableInputTypes", "availableInputChannels", "availableOutputTypes", "availableOutputChannels"];
const VOLATILE_CLIP_FIELDS: &[&str] = &["playingPosition", "isPlaying", "isTriggered", "fireButtonState", "willRecordOnStart"];

pub fn without_playback_state(value: &Value) -> Value {
    fn without(value: &Value, depth: usize) -> Value {
        if depth > 64 {
            return value.clone();
        }
        match value {
            Value::Array(items) => Value::Array(items.iter().map(|item| without(item, depth + 1)).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| !VOLATILE_CLIP_FIELDS.contains(&key.as_str()))
                    .map(|(key, item)| (key.clone(), without(item, depth + 1)))
                    .collect(),
            ),
            _ => value.clone(),
        }
    }
    without(value, 0)
}
fn keep_fields(value: &Value, volatile: &[&str]) -> Value {
    Value::Object(
        value
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| !volatile.contains(&key.as_str()))
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect(),
    )
}
pub fn owned_track_fingerprint_row(track: &Track) -> Value {
    let source = track.to_row();
    let mut row = keep_fields(&source, VOLATILE_TRACK_FIELDS);
    if source["routing"].is_object() {
        row["routing"] = keep_fields(&source["routing"], VOLATILE_ROUTING_FIELDS);
    }
    row["clipSlots"] = Value::Array(
        source["clipSlots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|slot| slot["empty"] != true || !slot["clipRef"].is_null())
            .map(|slot| keep_fields(slot, VOLATILE_SLOT_FIELDS))
            .collect(),
    );
    without_playback_state(&row)
}
pub fn owned_device_fingerprint_row(row: &Value) -> Value {
    match row {
        Value::Array(items) => Value::Array(items.iter().map(owned_device_fingerprint_row).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(key, _)| key.as_str() != "revision" && !(key.as_str() == "view" && fields.contains_key("canHaveChains")))
                .map(|(key, value)| (key.clone(), owned_device_fingerprint_row(value)))
                .collect(),
        ),
        _ => row.clone(),
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProperty {
    pub row_key: String,
    pub field: String,
    pub choices: Option<String>,
}

pub const SIMULATOR_OPERATIONS: &[&str] = &[
    "status",
    "snapshot",
    "discover",
    "get",
    "reconnect",
    "session.playback",
    "transport.set",
    "tempo.set",
    "session.audition-launch",
    "session.audition-stop",
    "session.emergency-stop",
    "session.clip-launch",
    "session.clip-stop",
    "clip.create",
    "clip.delete",
    "track.create",
    "track.delete",
    "track.rename",
    "track.create-return",
    "track.delete-return",
    "track.duplicate",
    "scene.duplicate",
    "track.view.set",
    "track.select-instrument",
    "track.set",
    "scene.create",
    "scene.delete",
    "scene.rename",
    "scene.set",
    "scene.fire-selected",
    "clip.rename",
    "device.rename",
    "locator.rename",
    "scene.capture",
    "note.add",
    "note.add-batch",
    "note.update",
    "note.delete",
    "note.duplicate",
    "note.quantize",
    "note.read-by-id",
    "note.read-selected",
    "locator.add",
    "locator.delete",
    "locator.jump",
    "locator.jump-to",
    "song.read",
    "song.set",
    "song.time-convert",
    "transport.action",
    "session.capture-midi",
    "device.parameter.set",
    "clip.duplicate",
    "clip.move",
    "clip.set",
    "clip.action",
    "arrangement.clip.create",
    "arrangement.clip.delete",
    "arrangement.clip.move",
    "arrangement.audio-clip.create",
    "session.audio-clip.create",
    "take-lane.create",
    "take-lane.rename",
    "take-lane.clip.create",
    "take-lane.audio-clip.create",
    "audio.take-lane.read",
    "audio.comp.read",
    "arrangement.automation.read",
    "tuning.read",
    "tuning.set",
    "groove.read",
    "groove.set",
    "groove.edit",
    "chain.set",
    "drum-pad.set",
    "drum-pad.delete-all-chains",
    "drum-pad.load-sample",
    "drum-pad.load-samples",
    "device.parameters.set",
    "rack.set",
    "rack.action",
    "rack.view.set",
    "audio.clip.set",
    "audio.warp-marker.read",
    "audio.warp-marker.add",
    "audio.warp-marker.move",
    "audio.warp-marker.delete",
    "mixer.set",
    "mixer.extended.set",
    "chain-mixer.set",
    "device-io.set",
    "compressor.sidechain.set",
    "automation.envelope.read",
    "automation.envelope.create",
    "automation.envelope.delete",
    "automation.envelope.clear",
    "automation.point.insert",
    "automation.point.delete",
    "device.insert",
    "device.delete",
    "device.enable",
    "device.move",
    "device.bank.set",
    "parameter.re-enable-automation",
    "device.comparison.save-to-slot",
    "drift.set",
    "drum-cell.set",
    "eq8.set",
    "hybrid-reverb.set",
    "looper.action",
    "looper.set",
    "meld.set",
    "plugin.set",
    "simpler.replace-sample",
    "observe.subscribe",
    "observe.poll",
    "observe.unsubscribe",
    "selection.set",
    "song.view.set",
    "clip.view.set",
    "device.view.set",
    "application.dialog",
    "browser.search",
    "browser.inspect",
    "browser.load",
    "ownership.settle",
    "browser.roots",
    "routing.set",
    "recording.session",
    "recording.arrangement",
    "performance.read",
    "view.set",
    "view.control",
    "undo.step.begin",
    "undo.step.end",
    "song.undo",
    "song.redo",
    "render.offline",
    "arrangement.midi-clip.create",
    "clip.clear-range",
    "device.duplicate",
    "drum-pad.sample-chain",
    "project.import",
    "transaction.group",
    "data.get",
    "data.set",
    "note.select",
    "note.delete-range",
    "fire-button.set",
    "track.action",
    "automation.step.insert",
    "automation.value-at",
    "device.property.set",
    "device.action",
    "sample.set",
    "sample.slice",
    "wavetable.set",
    "wavetable.modulation.set",
    "plugin.parameter-names",
    "device.banks.read",
    "clip.time-convert",
    "application.message",
    "browser.preview.start",
    "browser.preview.stop",
];
pub const LOM_GAP_OPERATIONS: &[&str] = &[
    "data.get",
    "data.set",
    "note.select",
    "note.delete-range",
    "fire-button.set",
    "track.action",
    "automation.step.insert",
    "automation.value-at",
    "device.property.set",
    "device.action",
    "sample.set",
    "sample.slice",
    "wavetable.set",
    "wavetable.modulation.set",
    "plugin.parameter-names",
    "device.banks.read",
    "clip.time-convert",
    "application.message",
    "browser.preview.start",
    "browser.preview.stop",
];
pub const EXTENSION_OPERATIONS: &[&str] = &[
    "render.offline",
    "arrangement.midi-clip.create",
    "clip.clear-range",
    "device.duplicate",
    "drum-pad.sample-chain",
    "project.import",
    "transaction.group",
];
pub const SAMPLE_FIELDS: &[&str] = &[
    "beatsGranulationResolution",
    "beatsTransientEnvelope",
    "beatsTransientLoopMode",
    "complexProEnvelope",
    "complexProFormants",
    "textureFlux",
    "textureGrainSize",
    "tonesGrainSize",
    "slicingStyle",
    "slicingBeatDivision",
    "slicingRegionCount",
    "slicingSensitivity",
];
pub const WAVETABLE_FIELDS: &[&str] = &[
    "oscillator1WavetableCategory",
    "oscillator1WavetableIndex",
    "oscillator2WavetableCategory",
    "oscillator2WavetableIndex",
    "oscillator1EffectMode",
    "oscillator2EffectMode",
    "filterRouting",
    "unisonMode",
    "unisonVoiceCount",
];
pub static DEVICE_PROPERTIES: LazyLock<HashMap<String, DeviceProperty>> = LazyLock::new(|| {
    serde_json::from_str("{\"roar.routing_mode_index\":{\"rowKey\":\"roar\",\"field\":\"routingModeIndex\",\"choices\":\"routingModeList\"},\"roar.env_listen\":{\"rowKey\":\"roar\",\"field\":\"envListen\",\"choices\":null},\"shifter.pitch_mode_index\":{\"rowKey\":\"shifter\",\"field\":\"pitchModeIndex\",\"choices\":\"pitchModeList\"},\"spectral_resonator.frequency_dial_mode\":{\"rowKey\":\"spectralResonator\",\"field\":\"frequencyDialMode\",\"choices\":\"frequencyDialModeList\"},\"spectral_resonator.midi_gate\":{\"rowKey\":\"spectralResonator\",\"field\":\"midiGate\",\"choices\":\"midiGateList\"},\"spectral_resonator.mod_mode\":{\"rowKey\":\"spectralResonator\",\"field\":\"modMode\",\"choices\":\"modModeList\"},\"spectral_resonator.mono_poly\":{\"rowKey\":\"spectralResonator\",\"field\":\"monoPoly\",\"choices\":\"monoPolyList\"},\"spectral_resonator.pitch_mode\":{\"rowKey\":\"spectralResonator\",\"field\":\"pitchMode\",\"choices\":\"pitchModeList\"},\"hybrid_reverb.ir_time_shaping_on\":{\"rowKey\":\"hybridReverb\",\"field\":\"irTimeShapingOn\",\"choices\":null},\"cc_control.custom_bool_target\":{\"rowKey\":\"ccControl\",\"field\":\"customBoolTarget\",\"choices\":\"customBoolTargetList\"},\"cc_control.custom_float_target_0\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget0\",\"choices\":\"customFloatTarget0List\"},\"cc_control.custom_float_target_1\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget1\",\"choices\":\"customFloatTarget1List\"},\"cc_control.custom_float_target_2\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget2\",\"choices\":\"customFloatTarget2List\"},\"cc_control.custom_float_target_3\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget3\",\"choices\":\"customFloatTarget3List\"},\"cc_control.custom_float_target_4\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget4\",\"choices\":\"customFloatTarget4List\"},\"cc_control.custom_float_target_5\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget5\",\"choices\":\"customFloatTarget5List\"},\"cc_control.custom_float_target_6\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget6\",\"choices\":\"customFloatTarget6List\"},\"cc_control.custom_float_target_7\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget7\",\"choices\":\"customFloatTarget7List\"},\"cc_control.custom_float_target_8\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget8\",\"choices\":\"customFloatTarget8List\"},\"cc_control.custom_float_target_9\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget9\",\"choices\":\"customFloatTarget9List\"},\"cc_control.custom_float_target_10\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget10\",\"choices\":\"customFloatTarget10List\"},\"cc_control.custom_float_target_11\":{\"rowKey\":\"ccControl\",\"field\":\"customFloatTarget11\",\"choices\":\"customFloatTarget11List\"},\"simpler.playback_mode\":{\"rowKey\":\"simpler\",\"field\":\"playbackMode\",\"choices\":null},\"simpler.retrigger\":{\"rowKey\":\"simpler\",\"field\":\"retrigger\",\"choices\":null},\"simpler.slicing_playback_mode\":{\"rowKey\":\"simpler\",\"field\":\"slicingPlaybackMode\",\"choices\":null},\"simpler.voices\":{\"rowKey\":\"simpler\",\"field\":\"voices\",\"choices\":null},\"simpler.pad_slicing\":{\"rowKey\":\"simpler\",\"field\":\"padSlicing\",\"choices\":null},\"simpler.note_pitch_bend_range\":{\"rowKey\":\"simpler\",\"field\":\"notePitchBendRange\",\"choices\":null}}").expect("device property definitions")
});

fn simulator_revision(value: &Value) -> String {
    crate::registry::sha256_hex(
        &crate::registry::canonical_json(value, &crate::registry::UNBOUNDED_CANONICAL_LIMITS).expect("JSON has a canonical form"),
    )
}
fn live_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("Live value is JSON")
}
fn from_live_json<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, LiveError> {
    serde_json::from_value(value).map_err(|error| LiveError::error(error.to_string()))
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string_arg<'a>(args: &'a Map<String, Value>, name: &str) -> Result<&'a str, LiveError> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 256)
        .ok_or_else(|| LiveError::type_error(format!("{name} must be a non-empty string")))
}
fn find_live_path(state: &Value, reference: &str) -> Option<String> {
    for (i, track) in array(&state["tracks"]).iter().enumerate() {
        let path = format!("/tracks/{i}");
        if track["ref"] == reference {
            return Some(path);
        }
        for (j, clip) in array(&track["clips"]).iter().enumerate() {
            if clip["ref"] == reference {
                return Some(format!("{path}/clips/{j}"));
            }
        }
        for (j, lane) in array(&track["takeLanes"]).iter().enumerate() {
            for (k, clip) in array(&lane["clips"]).iter().enumerate() {
                if clip["ref"] == reference {
                    return Some(format!("{path}/takeLanes/{j}/clips/{k}"));
                }
            }
        }
        for (j, device) in array(&track["devices"]).iter().enumerate() {
            if device["ref"] == reference {
                return Some(format!("{path}/devices/{j}"));
            }
            for (k, parameter) in array(&device["parameters"]).iter().enumerate() {
                if parameter["ref"] == reference {
                    return Some(format!("{path}/devices/{j}/parameters/{k}"));
                }
            }
        }
    }
    None
}
fn clip_path(state: &Value, reference: &str) -> Result<String, LiveError> {
    find_live_path(state, reference)
        .filter(|path| state.pointer(path).is_some_and(|value| value.get("notes").is_some()))
        .ok_or_else(|| LiveError::error(format!("unknown clip reference: {reference}")))
}
fn all_device_rows(state: &Value) -> Vec<Value> {
    fn visit(devices: &Value, depth: usize, rows: &mut Vec<Value>) {
        for device in array(devices) {
            rows.push(device.clone());
            if depth < 8 {
                for chain in array(&device["chains"]) {
                    visit(&chain["devices"], depth + 1, rows);
                }
            }
        }
    }
    let mut rows = Vec::new();
    for track in array(&state["tracks"]) {
        visit(&track["devices"], 0, &mut rows);
    }
    rows
}

/// An adapter test double with stable references, explicit authority fences and deterministic state.
pub struct DeterministicLiveSimulator {
    pub state: RefCell<Value>,
    sequence: Cell<u64>,
    epoch: Cell<i64>,
    listeners: Rc<RefCell<Vec<(u64, LiveListener)>>>,
    listener_number: Cell<u64>,
    pub read_budget_rows: Cell<Option<usize>>,
    pub discovery_budget_items: Cell<Option<usize>>,
    next_note_id: Cell<i64>,
    pub undo_step: RefCell<Option<Value>>,
    pub closed_undo_steps: RefCell<Vec<String>>,
    pub live_history: RefCell<(u64, u64)>,
    stored_data: RefCell<HashMap<String, String>>,
    pub selected_notes: RefCell<HashMap<String, Vec<Value>>>,
    pub held_fire_buttons: RefCell<std::collections::HashSet<String>>,
    modulation_amounts: RefCell<HashMap<String, HashMap<String, f64>>>,
    pub shown_messages: RefCell<Vec<Value>>,
    browser_preview: RefCell<Option<String>>,
    observe_sequence: Cell<u64>,
    observe_subscriptions: RefCell<HashMap<String, simulator_observe::ObserveSubscription>>,
}
impl Default for DeterministicLiveSimulator {
    fn default() -> Self {
        Self::new()
    }
}
impl DeterministicLiveSimulator {
    pub fn new() -> Self {
        let mut state: Value = serde_json::from_str(include_str!("live_simulator.json")).expect("simulator initial state");
        state["tuning"]["system"]["noteTunings"] =
            Value::Array((0..128).map(|note| serde_json::json!({"note":note,"deviation":0})).collect());
        Self {
            state: RefCell::new(state),
            sequence: Cell::new(0),
            epoch: Cell::new(1),
            listeners: Rc::new(RefCell::new(vec![])),
            listener_number: Cell::new(0),
            read_budget_rows: Cell::new(None),
            discovery_budget_items: Cell::new(None),
            next_note_id: Cell::new(2),
            undo_step: RefCell::new(None),
            closed_undo_steps: RefCell::new(vec![]),
            live_history: RefCell::new((1, 0)),
            stored_data: RefCell::new(HashMap::new()),
            selected_notes: RefCell::new(HashMap::new()),
            held_fire_buttons: RefCell::new(std::collections::HashSet::new()),
            modulation_amounts: RefCell::new(HashMap::new()),
            shown_messages: RefCell::new(vec![]),
            browser_preview: RefCell::new(None),
            observe_sequence: Cell::new(0),
            observe_subscriptions: RefCell::new(HashMap::new()),
        }
    }
    fn next_sequence(&self) -> u64 {
        let sequence = self.sequence.get() + 1;
        self.sequence.set(sequence);
        sequence
    }
    fn emit(&self, event_type: LiveEventType, reference: Option<LiveRef>, payload: Value) {
        let event = LiveEvent {
            epoch: self.epoch.get(),
            sequence: self.next_sequence(),
            event_type,
            ref_: reference,
            payload,
            channel: None,
            coalesced: None,
        };
        let mut last = 0;
        loop {
            let next = self.listeners.borrow().iter().find(|(id, _)| *id > last).cloned();
            let Some((id, listener)) = next else {
                break;
            };
            last = id;
            listener(&event);
        }
    }
    fn snapshot_value(&self) -> Value {
        let mut value = self.state.borrow().clone();
        let clips = array(&value["arrangementClips"])
            .iter()
            .map(|item| {
                let clip = &item["clip"];
                let mut row = Map::new();
                for key in ["ref", "objectIdentity"] {
                    if let Some(v) = clip.get(key) {
                        row.insert(key.into(), v.clone());
                    }
                }
                row.insert("parentRef".into(), item["trackRef"].clone());
                row.insert("trackRef".into(), item["trackRef"].clone());
                for key in ["name", "kind", "start", "length"] {
                    if let Some(v) = clip.get(key) {
                        row.insert(key.into(), v.clone());
                    }
                }
                for key in ["muted", "colorIndex", "looping", "loopStart", "loopEnd", "filePath"] {
                    row.insert(key.into(), clip[key].clone());
                }
                row.insert(
                    "isAudio".into(),
                    clip.get("isAudio").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| (clip["kind"] == "audio").into()),
                );
                row.insert(
                    "endTime".into(),
                    clip.get("endTime")
                        .filter(|v| !v.is_null())
                        .cloned()
                        .unwrap_or_else(|| (clip["start"].as_f64().unwrap_or(0.0) + clip["length"].as_f64().unwrap_or(0.0)).into()),
                );
                row.insert("noteCount".into(), array(&clip["notes"]).len().into());
                Value::Object(row)
            })
            .collect();
        value["arrangement"]["clips"] = Value::Array(clips);
        value
    }
    pub fn snapshot_view(&self, request: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        let request = request.cloned().unwrap_or_default();
        validate_snapshot_request(&request)?;
        let full = self.snapshot()?;
        let mut value = full.clone();
        value.epoch = Some(self.epoch.get());
        value.track_count = Some(full.tracks().len());
        value.scene_count = Some(full.scenes().len());
        let mut window = LiveSnapshotRequest::default();
        if let Some(tracks) = request.tracks {
            value.tracks = Some(full.tracks().iter().skip(tracks.from).take(tracks.count).cloned().collect());
            window.tracks = Some(tracks);
        }
        if let Some(scenes) = request.scenes {
            value.scenes = Some(full.scenes().iter().skip(scenes.from).take(scenes.count).cloned().collect());
            window.scenes = Some(scenes);
        }
        let set_ref = full.set.as_ref().map(|set| &set.ref_);
        let offset = request.tracks.map(|w| w.from).unwrap_or(0);
        if let Some(focus) = &request.focus {
            value.tracks = Some(
                value
                    .tracks()
                    .iter()
                    .enumerate()
                    .map(
                        |(position, track)| {
                            if focus.contains(&(offset + position)) {
                                track.clone()
                            } else {
                                light_track_row(track, set_ref)
                            }
                        },
                    )
                    .collect(),
            );
            window.focus = Some(focus.clone());
        }
        if let Some(budget) = self
            .read_budget_rows
            .get()
            .filter(|_| !request.is_empty() && request.parts.as_ref().is_none_or(|parts| parts.contains(&LiveSnapshotPart::Tracks)))
        {
            let mut rows = Vec::new();
            let mut built = 0;
            let mut cut = false;
            for track in value.tracks() {
                if track.is_light() {
                    rows.push(track.clone());
                    continue;
                }
                if built >= budget.max(1) {
                    cut = true;
                    if request.focus.is_none() {
                        break;
                    }
                    rows.push(light_track_row(track, set_ref));
                    continue;
                }
                built += 1;
                rows.push(track.clone());
            }
            if cut && request.focus.is_none() && request.tracks.is_some() {
                window.tracks = Some(LiveSnapshotWindow { from: offset, count: rows.len() });
            }
            if cut {
                if let Some(focus) = &request.focus {
                    window.focus = Some(
                        focus
                            .iter()
                            .copied()
                            .filter(|index| index.checked_sub(offset).and_then(|i| rows.get(i)).is_some_and(|row| !row.is_light()))
                            .collect(),
                    );
                }
            }
            value.tracks = Some(rows);
        }
        if request.tracks.is_some() || request.focus.is_some() {
            let whole = value.tracks().iter().filter(|track| !track.is_light()).map(|track| track.ref_.clone()).collect::<HashSet<_>>();
            if let Some(arrangement) = &mut value.arrangement {
                arrangement.clips = Some(
                    full.arrangement
                        .as_ref()
                        .and_then(|a| a.clips.as_ref())
                        .into_iter()
                        .flatten()
                        .filter(|clip| clip.get("trackRef").and_then(Value::as_str).is_some_and(|r| whole.contains(r)))
                        .cloned()
                        .collect(),
                );
            }
            value.arrangement_clips =
                Some(full.arrangement_clips.iter().flatten().filter(|item| whole.contains(&item.track_ref)).cloned().collect());
        }
        if let Some(parts) = &request.parts {
            window.parts = Some(parts.clone());
            if !parts.contains(&LiveSnapshotPart::Set) {
                value.set = None;
                value.song = None;
                value.view = None;
                value.tuning = None;
                value.groove_pool = None;
                value.browser = None;
            }
            if !parts.contains(&LiveSnapshotPart::Tracks) {
                value.tracks = None;
            }
            if !parts.contains(&LiveSnapshotPart::Scenes) {
                value.scenes = None;
            }
            if !parts.contains(&LiveSnapshotPart::Arrangement) {
                value.arrangement = None;
                value.arrangement_clips = None;
            }
            if !parts.contains(&LiveSnapshotPart::Playback) {
                value.playback = None;
            }
            if !parts.contains(&LiveSnapshotPart::Selection) {
                value.selection = None;
                value.selected = None;
            }
        }
        if !request.is_empty() {
            value.window = Some(window);
        }
        check_snapshot_answer(value, &request)
    }
    fn discovery_rows(&self, request: &LiveDiscoveryRequest) -> Result<Vec<Value>, LiveError> {
        let state = self.state.borrow();
        let parent = request.parent.as_deref();
        if request.kind == LiveDiscoveryKind::Note {
            let parent = parent.ok_or_else(|| LiveError::error("a kind-specific parent reference is required"))?;
            let clip =
                array(&state["tracks"]).iter().flat_map(|t| array(&t["clips"])).find(|clip| clip["ref"] == parent).or_else(|| {
                    array(&state["arrangementClips"]).iter().find(|item| item["clip"]["ref"] == parent).map(|item| &item["clip"])
                });
            return Ok(clip
                .map(|clip| array(&clip["notes"]))
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(index, note)| {
                    let mut row = note.clone();
                    row["ref"] = format!("{parent}:note:{index}").into();
                    row["parentRef"] = parent.into();
                    row
                })
                .collect());
        }
        Ok(match request.kind {
            LiveDiscoveryKind::Set => vec![state["set"].clone()],
            LiveDiscoveryKind::Track => array(&state["tracks"]).to_vec(),
            LiveDiscoveryKind::Scene => array(&state["scenes"]).to_vec(),
            LiveDiscoveryKind::SessionClip => array(&state["tracks"]).iter().flat_map(|t| array(&t["clips"]).iter().cloned()).collect(),
            LiveDiscoveryKind::ArrangementClip => array(&state["arrangementClips"])
                .iter()
                .filter(|item| parent.is_none_or(|p| item["trackRef"] == p))
                .map(|item| {
                    let clip = &item["clip"];
                    let mut row = serde_json::json!({"ref":clip["ref"]});
                    if let Some(v) = clip.get("objectIdentity") {
                        row["objectIdentity"] = v.clone();
                    }
                    row["parentRef"] = item["trackRef"].clone();
                    row["trackRef"] = item["trackRef"].clone();
                    for key in ["name", "kind", "start", "length"] {
                        row[key] = clip[key].clone();
                    }
                    row["notes"] = array(&clip["notes"]).len().into();
                    row
                })
                .collect(),
            LiveDiscoveryKind::Locator => array(&state["arrangement"]["locators"]).to_vec(),
            LiveDiscoveryKind::Device => all_device_rows(&state)
                .into_iter()
                .map(|mut device| {
                    if !array(&device["chains"]).is_empty() {
                        device["chainList"] = Value::Array(
                            array(&device["chains"])
                                .iter()
                                .map(|chain| serde_json::json!({"ref":chain["ref"],"name":chain["name"]}))
                                .collect(),
                        );
                    }
                    device
                })
                .collect(),
            LiveDiscoveryKind::Parameter => {
                if let Some(parent) = parent {
                    all_device_rows(&state)
                        .into_iter()
                        .find(|device| device["ref"] == parent)
                        .map(|device| {
                            array(&device["parameters"])
                                .iter()
                                .map(|p| {
                                    let mut p = p.clone();
                                    p["parentRef"] = parent.into();
                                    p
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    array(&state["tracks"])
                        .iter()
                        .flat_map(|t| array(&t["devices"]))
                        .flat_map(|d| array(&d["parameters"]))
                        .cloned()
                        .collect()
                }
            }
            LiveDiscoveryKind::SessionPlayback => vec![state["playback"].clone()],
            _ => vec![],
        })
    }
    pub fn discover(&self, request: &LiveDiscoveryRequest) -> Result<LiveDiscoveryResult, LiveError> {
        use base64::Engine;
        let rows = self
            .discovery_rows(request)?
            .into_iter()
            .filter(|row| request.filter.as_ref().is_none_or(|filter| filter.iter().all(|(key, value)| row.get(key) == Some(value))))
            .collect::<Vec<_>>();
        let revision = format!("{}:{}:{}:{}", self.epoch.get(), request.kind, request.parent.as_deref().unwrap_or(""), rows.len());
        let listed = |count: usize| {
            if request.kind == LiveDiscoveryKind::Note {
                simulator_revision(&Value::Array(
                    rows.iter()
                        .take(count)
                        .map(|row| {
                            row.get("id")
                                .filter(|v| !v.is_null())
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!([row["pitch"], row["start"], row["duration"]]))
                        })
                        .collect(),
                ))
            } else {
                String::new()
            }
        };
        let offset = if let Some(cursor) = &request.cursor {
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(cursor.trim_end_matches('='))
                .map_err(|_| LiveError::error("invalid discovery cursor"))?;
            let position: Value = serde_json::from_slice(&bytes).map_err(|_| LiveError::error("invalid discovery cursor"))?;
            let offset = position["offset"].as_u64().and_then(|n| usize::try_from(n).ok());
            if position["revision"] != revision
                || offset.is_none_or(|n| n > rows.len())
                || position.get("listed").filter(|v| !v.is_null()).unwrap_or(&Value::String(String::new()))
                    != &Value::String(listed(offset.unwrap_or(0)))
            {
                return Err(LiveError::error("stale discovery cursor"));
            }
            offset.unwrap()
        } else {
            0
        };
        let limit = request.limit.unwrap_or(50).min(self.discovery_budget_items.get().unwrap_or(usize::MAX)).max(1);
        let page = rows.iter().skip(offset).take(limit).cloned().collect::<Vec<_>>();
        let next_offset = offset + page.len();
        let next_cursor = if next_offset < rows.len() {
            let mut cursor = serde_json::json!({"revision":revision,"offset":next_offset});
            if request.kind == LiveDiscoveryKind::Note {
                cursor["listed"] = listed(next_offset).into();
            }
            Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(kumi_common::js::json::stringify(&cursor)))
        } else {
            None
        };
        let items = page
            .into_iter()
            .map(|row| {
                row.as_object()
                    .unwrap()
                    .iter()
                    .filter(|(key, _)| {
                        request
                            .fields
                            .as_ref()
                            .is_none_or(|fields| key.as_str() == "ref" || key.as_str() == "parentRef" || fields.contains(key))
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .collect();
        Ok(LiveDiscoveryResult {
            epoch: self.epoch.get(),
            items,
            truncated: next_cursor.is_some(),
            revision,
            kind: request.kind,
            next_cursor,
        })
    }
}

impl DeterministicLiveSimulator {
    pub fn simulate_external_edit(&self, reference: &LiveRef, property: &str, value: Value) -> Result<(), LiveError> {
        let mut state = self.state.borrow_mut();
        let path = if state["set"]["ref"] == reference.as_str() { Some("/set".to_string()) } else { find_live_path(&state, reference) };
        let path = path
            .filter(|p| state.pointer(p).is_some_and(|row| row.get(property).is_some()))
            .ok_or_else(|| LiveError::error(format!("unknown Live property: {reference}.{property}")))?;
        let target = state.pointer_mut(&path).unwrap();
        match property {
            "tempo" => {
                let v = value
                    .as_f64()
                    .filter(|v| v.is_finite() && path == "/set")
                    .ok_or_else(|| LiveError::type_error("tempo must be finite"))?;
                target[property] = v.clamp(20.0, 999.0).into();
            }
            "volume" | "pan" => {
                let v =
                    value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| LiveError::type_error(format!("{property} must be finite")))?;
                target[property] = v.clamp(if property == "pan" { -1.0 } else { 0.0 }, 1.0).into();
            }
            "playing" | "mute" | "solo" | "armed" => {
                if !value.is_boolean() {
                    return Err(LiveError::type_error(format!("{property} must be boolean")));
                }
                target[property] = value;
            }
            "position" if path == "/set" => {
                if !value.as_f64().is_some_and(|v| v.is_finite() && v >= 0.0) {
                    return Err(LiveError::type_error("position must be a non-negative finite number"));
                }
                target[property] = value;
            }
            "name" => {
                if !value.as_str().is_some_and(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 256) {
                    return Err(LiveError::type_error("name must be 1-256 characters"));
                }
                target[property] = value;
            }
            "value" if target.get("min").is_some() && target.get("max").is_some() => {
                let value =
                    value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| LiveError::type_error("parameter value must be finite"))?;
                if target["enabled"] == false {
                    return Err(LiveError::error("parameter is disabled"));
                }
                let min = target["min"].as_f64().unwrap();
                let max = target["max"].as_f64().unwrap();
                let quantum = target["quantization"].as_f64().unwrap_or(0.0);
                let clamped = value.clamp(min, max);
                let next = if quantum > 0.0 { kumi_common::js::number::round((clamped - min) / quantum) * quantum + min } else { clamped };
                target["value"] = next.into();
                target["revision"] = (target["revision"].as_u64().unwrap_or(0) + 1).into();
                target["displayValue"] = kumi_common::js::number::to_string(next).into();
            }
            _ => return Err(LiveError::error(format!("property is not writable: {property}"))),
        }
        let applied = target[property].clone();
        drop(state);
        self.emit(
            if property == "playing" || property == "tempo" { LiveEventType::Transport } else { LiveEventType::Object },
            Some(reference.clone()),
            serde_json::json!({"property":property,"value":applied}),
        );
        Ok(())
    }
    fn validate_note_for_clip(clip: &Value, note: &Note) -> Result<(), LiveError> {
        if clip["kind"] != "midi" {
            return Err(LiveError::error("notes require a MIDI clip"));
        }
        if note.pitch.fract() != 0.0
            || !note.pitch.is_finite()
            || !(0.0..=127.0).contains(&note.pitch)
            || !note.start.is_finite()
            || note.start < 0.0
            || !note.duration.is_finite()
            || note.duration <= 0.0
            || !note.velocity.is_finite()
            || !(1.0..=127.0).contains(&note.velocity)
            || !note.channel.is_finite()
            || note.channel.fract() != 0.0
            || !(1.0..=16.0).contains(&note.channel)
        {
            return Err(LiveError::range_error("invalid MIDI note"));
        }
        for (value, min, max, message) in [
            (note.probability.value(), 0.0, 1.0, "note probability is invalid"),
            (note.velocity_deviation.value(), -127.0, 127.0, "note velocity deviation is invalid"),
            (note.release_velocity.value(), 0.0, 127.0, "note release velocity is invalid"),
        ] {
            if value.is_some_and(|v| !v.is_finite() || *v < min || *v > max) {
                return Err(LiveError::range_error(message));
            }
        }
        Ok(())
    }
    pub fn add_note(&self, reference: &LiveRef, note: &Note) -> Result<Value, LiveError> {
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        let clip = state.pointer_mut(&path).unwrap();
        Self::validate_note_for_clip(clip, note)?;
        let id = self.next_note_id.get();
        self.next_note_id.set(id + 1);
        let mut added = note.clone();
        added.id = Maybe::Value(id);
        added.mute = Maybe::Value(note.mute.cloned().unwrap_or(false));
        added.probability = Maybe::Value(note.probability.cloned().unwrap_or(1.0));
        added.velocity_deviation = Maybe::Value(note.velocity_deviation.cloned().unwrap_or(0.0));
        added.release_velocity = Maybe::Value(note.release_velocity.cloned().unwrap_or(64.0));
        clip["notes"].as_array_mut().unwrap().push(live_json(&added));
        clip["notesRevision"] = simulator_revision(&clip["notes"]).into();
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.clone()), serde_json::json!({"operation":"note.add","note":note}));
        Ok(serde_json::json!({"added":true,"noteId":id}))
    }
    pub fn set_automation(&self, reference: &LiveRef, point: &AutomationPoint) -> Result<(), LiveError> {
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        let clip = state.pointer_mut(&path).unwrap();
        if !point.time.is_finite()
            || point.time < 0.0
            || point.time > clip["length"].as_f64().unwrap()
            || !point.value.is_finite()
            || point.curve.is_some_and(|v| !v.is_finite())
        {
            return Err(LiveError::range_error("automation point is outside the clip"));
        }
        clip["automation"].as_array_mut().unwrap().push(live_json(point));
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.clone()), serde_json::json!({"operation":"automation.add","point":point}));
        Ok(())
    }
    pub fn set_warp(&self, reference: &LiveRef, enabled: bool) -> Result<(), LiveError> {
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        let clip = state.pointer_mut(&path).unwrap();
        if clip["kind"] != "audio" {
            return Err(LiveError::error("warp requires an audio clip"));
        }
        clip["warp"] = enabled.into();
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.clone()), serde_json::json!({"operation":"warp.set","enabled":enabled}));
        Ok(())
    }
    pub fn add_take(&self, reference: &LiveRef, take: &str) -> Result<(), LiveError> {
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        let clip = state.pointer_mut(&path).unwrap();
        if take.is_empty() || kumi_common::js::string::utf16_len(take) > 256 || array(&clip["takes"]).iter().any(|v| v == take) {
            return Err(LiveError::error("invalid or duplicate take"));
        }
        clip["takes"].as_array_mut().unwrap().push(take.into());
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.clone()), serde_json::json!({"operation":"take.add","take":take}));
        Ok(())
    }
    fn session_clip_authority(state: &Value, reference: &str) -> Result<Value, LiveError> {
        for track in array(&state["tracks"]) {
            if let Some(clip) = array(&track["clips"]).iter().find(|clip| clip["ref"] == reference) {
                let slot = array(&track["clipSlots"]).iter().find(|slot| slot["clipRef"] == reference);
                let scene = slot.and_then(|slot| array(&state["scenes"]).iter().find(|scene| scene["index"] == slot["sceneIndex"]));
                if let (Some(slot), Some(scene)) = (slot, scene) {
                    if [clip, track, slot, scene].iter().all(|row| row["objectIdentity"].is_string()) {
                        return Ok(
                            serde_json::json!({"expectedObjectIdentity":clip["objectIdentity"],"expectedTrackRef":track["ref"],"expectedTrackIdentity":track["objectIdentity"],"expectedSlotRef":slot["ref"],"expectedSlotIdentity":slot["objectIdentity"],"expectedSceneRef":scene["ref"],"expectedSceneIdentity":scene["objectIdentity"]}),
                        );
                    }
                }
            }
        }
        Err(LiveError::error("clip hierarchy identity is unavailable"))
    }
    fn assert_note_authority(&self, args: &Map<String, Value>, reference: &str) -> Result<(), LiveError> {
        let state = self.state.borrow();
        let path = clip_path(&state, reference)?;
        let clip = state.pointer(&path).unwrap();
        if !args.contains_key("expectedClipAuthority") {
            return Err(LiveError::error("unsupported simulator authority value"));
        }
        if args.get("expectedClipAuthority") != Some(&Self::session_clip_authority(&state, reference)?)
            || args.get("expectedNotesRevision") != clip.get("notesRevision")
        {
            return Err(LiveError::error("clip identity or notes changed since preview"));
        }
        Ok(())
    }
    fn parameter_authority(state: &Value, reference: &str) -> Result<Value, LiveError> {
        for track in array(&state["tracks"]) {
            let mixer = &track["mixer"];
            if mixer.is_object() && track["objectIdentity"].is_string() {
                let mut rows = Vec::new();
                for (ref_key, id_key) in [("volumeRef", "volumeIdentity"), ("panRef", "panIdentity"), ("cueRef", "cueIdentity")] {
                    if mixer[ref_key].is_string() && mixer[id_key].is_string() {
                        rows.push(serde_json::json!({"ref":mixer[ref_key],"objectIdentity":mixer[id_key]}));
                    }
                }
                for (i, r) in array(&mixer["sendRefs"]).iter().enumerate() {
                    let identity = &mixer["sendIdentities"][i];
                    if r.is_string() && identity.is_string() {
                        rows.push(serde_json::json!({"ref":r,"objectIdentity":identity}));
                    }
                }
                if let Some(target) = rows.iter().find(|row| row["ref"] == reference) {
                    return Ok(
                        serde_json::json!({"ref":target["ref"],"parameterIdentity":target["objectIdentity"],"ownerRef":track["ref"],"ownerIdentity":track["objectIdentity"],"trackRef":track["ref"],"trackIdentity":track["objectIdentity"],"siblings":rows}),
                    );
                }
            }
            for device in array(&track["devices"]) {
                if let Some(parameter) = array(&device["parameters"]).iter().find(|parameter| parameter["ref"] == reference) {
                    if [parameter, device, track].iter().all(|row| row["objectIdentity"].is_string()) {
                        return Ok(
                            serde_json::json!({"ref":parameter["ref"],"parameterIdentity":parameter["objectIdentity"],"ownerRef":device["ref"],"ownerIdentity":device["objectIdentity"],"trackRef":track["ref"],"trackIdentity":track["objectIdentity"],"siblings":array(&device["parameters"]).iter().map(|p|serde_json::json!({"ref":p["ref"],"objectIdentity":p["objectIdentity"]})).collect::<Vec<_>>()}),
                        );
                    }
                }
            }
        }
        Err(LiveError::error("parameter authority is unavailable"))
    }
    fn checked_parameter(&self, args: &Map<String, Value>) -> Result<LiveRef, LiveError> {
        let reference = string_arg(args, "ref")?;
        let state = self.state.borrow();
        let target = find_live_path(&state, reference).and_then(|path| state.pointer(&path));
        let requested = args.get("value").and_then(Value::as_f64);
        if target.is_none()
            || requested.is_none_or(|value| {
                !value.is_finite()
                    || target.unwrap()["min"].as_f64().is_some_and(|min| value < min)
                    || target.unwrap()["max"].as_f64().is_some_and(|max| value > max)
            })
        {
            return Err(LiveError::range_error("parameter value is outside numeric bounds"));
        }
        let target = target.unwrap();
        let requested = requested.unwrap();
        let lean = args.get("expectedSiblings").and_then(Value::as_array).is_some_and(Vec::is_empty);
        let mut current = Self::parameter_authority(&state, reference)?;
        if lean {
            current.as_object_mut().unwrap().remove("siblings");
        }
        if ["expectedObjectIdentity", "expectedOwnerRef", "expectedOwnerIdentity", "expectedTrackRef", "expectedTrackIdentity"]
            .iter()
            .any(|key| !args.contains_key(*key))
            || (!lean && !args.contains_key("expectedSiblings"))
        {
            return Err(LiveError::error("unsupported simulator authority value"));
        }
        let mut expected = serde_json::json!({"ref":reference});
        for (key, arg) in [
            ("parameterIdentity", "expectedObjectIdentity"),
            ("ownerRef", "expectedOwnerRef"),
            ("ownerIdentity", "expectedOwnerIdentity"),
            ("trackRef", "expectedTrackRef"),
            ("trackIdentity", "expectedTrackIdentity"),
        ] {
            if let Some(value) = args.get(arg) {
                expected[key] = value.clone();
            }
        }
        if !lean {
            if let Some(siblings) = args.get("expectedSiblings") {
                expected["siblings"] = siblings.clone();
            }
        }
        if current != expected {
            return Err(LiveError::error("parameter identity or hierarchy changed since preview"));
        }
        if target["enabled"] == false {
            return Err(LiveError::error("parameter is greyed out in Live right now"));
        }
        let quantum = target["quantization"].as_f64().unwrap_or(0.0);
        let min = target["min"].as_f64().unwrap_or(f64::NAN);
        if quantum > 0.0 {
            let scaled = (requested - min) / quantum;
            if (scaled - kumi_common::js::number::round(scaled)).abs() > 1e-9 {
                return Err(LiveError::range_error("parameter value violates quantization"));
            }
        }
        let revision = target.get("revision").filter(|v| !v.is_null()).cloned().unwrap_or(1.into());
        let expected_revision = args.get("expectedRevision").cloned().unwrap_or(Value::Null);
        let same_revision = if revision.is_number() && expected_revision.is_number() {
            revision.as_f64() == expected_revision.as_f64()
        } else {
            revision == expected_revision
        };
        if !same_revision {
            return Err(LiveError::error("parameter revision changed since preview"));
        }
        Ok(reference.into())
    }
    fn browser_catalog() -> Vec<Value> {
        [("instruments","Drum Rack",true),("instruments","Analog",true),("instruments","Collision",true),("instruments","Instrument Rack",true),("audio_effects","Audio Effect Rack",true),("audio_effects","Utility",true),("audio_effects","Echo",true),("midi_effects","Arpeggiator",true),("drums","Kick Core",false)].into_iter().map(|(category,name,is_device)|{let path=format!("{category}/{name}");serde_json::json!({"id":path,"objectIdentity":format!("simulator:browser:{path}"),"name":name,"category":category,"path":path,"isDevice":is_device})}).collect()
    }
}
impl LiveAdapter for DeterministicLiveSimulator {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(LiveStatus {
            connected: true,
            adapter: LiveAdapterKind::Simulator,
            epoch: Some(self.epoch.get()),
            protocol: LIVE_PROTOCOL_VERSION.into(),
            capabilities: live_capabilities_for_operations(SIMULATOR_OPERATIONS),
            operations: Some(SIMULATOR_OPERATIONS.iter().map(|s| s.to_string()).collect()),
            reason: None,
            registry_hash: None,
            provenance: None,
            willington_kinds: None,
            environment: None,
            extra: Map::new(),
        })
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        from_live_json(self.snapshot_value())
    }
    fn get(&self, reference: &LiveRef) -> Result<Option<Value>, LiveError> {
        let state = self.state.borrow();
        if state["set"]["ref"] == reference.as_str() {
            return Ok(Some(state["set"].clone()));
        }
        if let Some(scene) = array(&state["scenes"]).iter().find(|row| row["ref"] == reference.as_str()) {
            return Ok(Some(scene.clone()));
        }
        if let Some(path) = find_live_path(&state, reference) {
            return Ok(state.pointer(&path).cloned());
        }
        for track in array(&state["tracks"]) {
            if let Some(lane) = array(&track["takeLanes"]).iter().find(|lane| lane["ref"] == reference.as_str()) {
                return Ok(Some(lane.clone()));
            }
        }
        Ok(array(&state["arrangement"]["locators"]).iter().find(|row| row["ref"] == reference.as_str()).cloned())
    }
    fn invoke(&self, invocation: &LiveInvocation) -> Result<Value, LiveError> {
        self.invoke_operation(&invocation.operation, &invocation.args)
    }
    fn subscribe(&self, listener: LiveListener) -> Result<Unsubscribe, LiveError> {
        let existing = self.listeners.borrow().iter().find(|(_, callback)| Rc::ptr_eq(callback, &listener)).map(|(id, _)| *id);
        let id = existing.unwrap_or_else(|| {
            let id = self.listener_number.get() + 1;
            self.listener_number.set(id);
            self.listeners.borrow_mut().push((id, listener));
            id
        });
        let listeners = self.listeners.clone();
        Ok(Box::new(move || listeners.borrow_mut().retain(|(key, _)| *key != id)))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.epoch.set(self.epoch.get() + 1);
        {
            let mut state = self.state.borrow_mut();
            state["playback"]["epoch"] = self.epoch.get().into();
            state["playback"]["revision"] = format!("{}:reconnected", self.epoch.get()).into();
        }
        self.emit(LiveEventType::State, None, serde_json::json!({"epoch":self.epoch.get(),"snapshot":self.snapshot_value()}));
        self.status()
    }
}
#[async_trait(?Send)]
impl AsyncLiveAdapter for DeterministicLiveSimulator {
    async fn snapshot_async(
        &self,
        _: Option<&LiveOperationContext>,
        request: Option<&LiveSnapshotRequest>,
    ) -> Result<LiveSnapshot, LiveError> {
        self.snapshot_view(request)
    }
    async fn discover_async(
        &self,
        request: &LiveDiscoveryRequest,
        _: Option<&LiveOperationContext>,
    ) -> Result<LiveDiscoveryResult, LiveError> {
        self.discover(request)
    }
    async fn get_async(&self, reference: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(reference)
    }
    async fn invoke_async(&self, invocation: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke(invocation)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        self.listeners.borrow_mut().clear();
        Ok(())
    }
}

impl DeterministicLiveSimulator {
    fn invoke_operation(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        use serde_json::json;
        match operation {
            "render.offline"
            | "arrangement.midi-clip.create"
            | "clip.clear-range"
            | "device.duplicate"
            | "drum-pad.sample-chain"
            | "project.import"
            | "transaction.group" => self.invoke_extension(operation, args),
            "observe.subscribe" | "observe.poll" | "observe.unsubscribe" => self.invoke_observe(operation, args),
            "session.capture-midi" | "scene.capture" | "session.audition-launch" | "session.audition-stop" | "session.emergency-stop" => {
                self.invoke_session(operation, args)
            }
            "data.get"
            | "data.set"
            | "note.select"
            | "note.delete-range"
            | "fire-button.set"
            | "track.action"
            | "device.property.set"
            | "device.action"
            | "sample.set"
            | "sample.slice"
            | "wavetable.set"
            | "wavetable.modulation.set"
            | "plugin.parameter-names"
            | "device.banks.read"
            | "clip.time-convert"
            | "application.message"
            | "browser.preview.start"
            | "browser.preview.stop" => self.invoke_lom(operation, args),
            "drum-pad.set"
            | "drum-pad.load-sample"
            | "drum-pad.load-samples"
            | "drum-pad.delete-all-chains"
            | "rack.set"
            | "rack.action"
            | "rack.view.set" => self.invoke_racks(operation, args),
            "chain.set"
            | "chain-mixer.set"
            | "parameter.re-enable-automation"
            | "device-io.set"
            | "compressor.sidechain.set"
            | "device.bank.set"
            | "device.comparison.save-to-slot"
            | "simpler.replace-sample"
            | "drift.set"
            | "drum-cell.set"
            | "eq8.set"
            | "hybrid-reverb.set"
            | "looper.set"
            | "meld.set"
            | "plugin.set"
            | "looper.action" => self.invoke_device_state(operation, args),
            "locator.jump" | "view.set" | "view.control" => self.invoke_view_control(operation, args),
            "automation.step.insert" | "automation.value-at" => self.invoke_gap_automation(operation, args),
            "automation.envelope.clear" => self.clear_envelopes(args),
            "clip.set"
            | "clip.action"
            | "clip.follow-actions.set"
            | "audio.clip.set"
            | "audio.warp-marker.read"
            | "audio.warp-marker.add"
            | "audio.warp-marker.move"
            | "audio.warp-marker.delete" => self.invoke_clip_properties(operation, args),
            "arrangement.clip.create"
            | "arrangement.audio-clip.create"
            | "arrangement.clip.delete"
            | "arrangement.clip.move"
            | "clip.duplicate"
            | "clip.move"
            | "session.audio-clip.create" => self.invoke_clips(operation, args),
            "track.create-return"
            | "track.delete-return"
            | "track.duplicate"
            | "scene.duplicate"
            | "audio.take-lane.read"
            | "take-lane.create"
            | "take-lane.rename"
            | "take-lane.clip.create"
            | "take-lane.audio-clip.create" => self.invoke_structure(operation, args),
            "scene.set"
            | "scene.fire-selected"
            | "track.view.set"
            | "track.set"
            | "track.select-instrument"
            | "mixer.extended.set"
            | "selection.set"
            | "song.view.set"
            | "clip.view.set"
            | "device.view.set"
            | "locator.jump-to"
            | "application.dialog"
            | "performance.read" => self.invoke_views(operation, args),
            "recording.session"
            | "recording.arrangement"
            | "arrangement.automation.read"
            | "audio.comp.read"
            | "note.read-by-id"
            | "note.read-selected"
            | "note.duplicate"
            | "note.quantize"
            | "automation.envelope.read"
            | "automation.envelope.create"
            | "automation.envelope.delete"
            | "automation.point.insert"
            | "automation.point.delete" => self.invoke_automation(operation, args),
            "device.insert" | "device.delete" | "device.enable" | "device.move" | "browser.load" => self.invoke_devices(operation, args),
            "tuning.read" | "tuning.set" | "groove.read" | "groove.set" | "groove.edit" | "song.read" | "song.set"
            | "song.time-convert" | "transport.action" => self.invoke_set_state(operation, args),
            "undo.step.begin" => {
                let previous = self.undo_step.borrow_mut().take();
                if let Some(step) = &previous {
                    self.closed_undo_steps.borrow_mut().push(step["stepId"].as_str().unwrap().into());
                }
                let timeout = args.get("timeoutMs").and_then(Value::as_f64).unwrap_or(120_000.0);
                let step = json!({"stepId":format!("undo_simulator_{}",self.next_sequence()),"expiresAt":kumi_common::time::now_ms() as f64+timeout});
                *self.undo_step.borrow_mut() = Some(step.clone());
                Ok(json!({"open":true,"stepId":step["stepId"],"expiresAt":step["expiresAt"],"closedPrevious":previous.is_some()}))
            }
            "undo.step.end" => {
                let step = self.undo_step.borrow().clone();
                let Some(step) = step else {
                    return Ok(json!({"closed":false,"stepId":null,"reason":"not-open"}));
                };
                if args.get("stepId").is_some_and(|v| v != &step["stepId"]) {
                    return Ok(json!({"closed":false,"stepId":step["stepId"],"reason":"other-step"}));
                }
                self.undo_step.borrow_mut().take();
                self.closed_undo_steps.borrow_mut().push(step["stepId"].as_str().unwrap().into());
                Ok(json!({"closed":true,"stepId":step["stepId"],"reason":"ended"}))
            }
            "song.undo" | "song.redo" => {
                let undoing = operation == "song.undo";
                let mut history = self.live_history.borrow_mut();
                let done = if undoing { history.0 > 0 } else { history.1 > 0 };
                if done {
                    if let Some(step) = self.undo_step.borrow_mut().take() {
                        self.closed_undo_steps.borrow_mut().push(step["stepId"].as_str().unwrap().into());
                    }
                    if undoing {
                        history.0 -= 1;
                        history.1 += 1;
                    } else {
                        history.1 -= 1;
                        history.0 += 1;
                    }
                }
                let result = json!({"done":done,"canUndo":history.0>0,"canRedo":history.1>0});
                drop(history);
                if done {
                    self.emit(LiveEventType::Reset, None, json!({"operation":operation}));
                }
                Ok(result)
            }
            "tempo.set" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let set = &mut state["set"];
                if set["ref"] != reference
                    || args.get("expectedObjectIdentity") != set.get("objectIdentity")
                    || !args.get("value").is_some_and(Value::is_number)
                    || !args.get("expectedTempo").is_some_and(Value::is_number)
                    || args.get("expectedTempo").and_then(Value::as_f64) != set["tempo"].as_f64()
                {
                    return Err(LiveError::error("Set identity or tempo state changed since preview"));
                }
                let tempo = args["value"].as_f64().unwrap().clamp(20.0, 999.0);
                set["tempo"] = tempo.into();
                let revision = self.next_sequence();
                drop(state);
                self.emit(LiveEventType::Transport, Some(reference.into()), json!({"property":"tempo","value":tempo}));
                Ok(json!({"changed":true,"tempo":tempo,"revision":revision}))
            }
            "transport.set" => {
                let mut state = self.state.borrow_mut();
                if args.get("setRef") != state["set"].get("ref")
                    || args.get("expectedObjectIdentity") != state["set"].get("objectIdentity")
                    || !args.get("expectedRevision").is_some_and(Value::is_string)
                    || args.get("expectedRevision") != state["playback"].get("revision")
                {
                    return Err(LiveError::error("transport Set identity or state changed since preview"));
                }
                for key in ["position", "loopStart", "loopLength"] {
                    if let Some(value) = args.get(key).filter(|v| !v.is_null()) {
                        if !value.as_f64().is_some_and(|v| v.is_finite() && v >= 0.0) {
                            return Err(LiveError::type_error(format!("{key} is invalid")));
                        }
                    }
                }
                for key in ["loopEnabled", "metronome", "punchIn", "punchOut"] {
                    if let Some(value) = args.get(key).filter(|v| !v.is_null()) {
                        if !value.is_boolean() {
                            return Err(LiveError::type_error(format!("{key} is invalid")));
                        }
                    }
                }
                if args.get("loopLength").and_then(Value::as_f64).is_some_and(|v| v <= 0.0) {
                    return Err(LiveError::range_error("loopLength is invalid"));
                }
                for key in ["position", "metronome", "punchIn", "punchOut"] {
                    if let Some(value) = args.get(key).filter(|v| !v.is_null()) {
                        state["playback"]["transport"][key] = value.clone();
                        if key == "position" {
                            state["set"][key] = value.clone();
                        }
                    }
                }
                for (key, field) in [("loopEnabled", "enabled"), ("loopStart", "start"), ("loopLength", "length")] {
                    if let Some(value) = args.get(key).filter(|v| !v.is_null()) {
                        state["playback"]["transport"]["loop"][field] = value.clone();
                    }
                }
                let revision = format!("{}:transport:{}", self.epoch.get(), self.next_sequence());
                state["playback"]["revision"] = revision.clone().into();
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":revision}))
            }
            "session.clip-launch" | "session.clip-stop" => self.session_clip_playback(operation, args),
            "track.create" => {
                self.require_structure_revision(args)?;
                let kind = args
                    .get("kind")
                    .and_then(Value::as_str)
                    .filter(|kind| ["midi", "audio"].contains(kind))
                    .ok_or_else(|| LiveError::type_error("track kind must be audio or midi"))?;
                let name = string_arg(args, "name")?;
                let mut state = self.state.borrow_mut();
                let size = array(&state["tracks"]).len();
                let index = if let Some(value) = args.get("index") {
                    value
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .filter(|v| *v <= size)
                        .ok_or_else(|| LiveError::range_error("track index is invalid"))?
                } else {
                    size
                };
                let number = size as u64 + self.sequence.get() + 1;
                let reference = format!("track:track-{number}");
                let slots=array(&state["scenes"]).iter().map(|scene|json!({"ref":format!("clip-slot:{number}:{}",scene["index"]),"parentRef":reference,"objectIdentity":format!("simulator:clip-slot:{number}:{}",scene["index"]),"sceneIndex":scene["index"],"clipRef":null,"empty":true})).collect::<Vec<_>>();
                let track = json!({"ref":reference,"objectIdentity":format!("simulator:track:{number}"),"name":name,"kind":"regular","mediaKind":kind,"volume":0.85,"pan":0,"mute":false,"solo":false,"armed":false,"clips":[],"clipSlots":slots,"devices":[],"sends":[0,0]});
                state["tracks"].as_array_mut().unwrap().insert(index, track.clone());
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.clone().into()), json!({"operation":operation,"track":track}));
                let mut result = track;
                result["kind"] = kind.into();
                result["index"] = index.into();
                result["createdFingerprint"] = self.structure_created_fingerprint("track", &reference)?.into();
                Ok(result)
            }
            "track.delete" => {
                self.require_structure_revision(args)?;
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let index = array(&state["tracks"])
                    .iter()
                    .position(|track| track["ref"] == reference)
                    .ok_or_else(|| LiveError::error(format!("unknown track reference: {reference}")))?;
                if args.get("expectedObjectIdentity") != state["tracks"][index].get("objectIdentity") {
                    return Err(LiveError::error("track object identity changed; deletion refused"));
                }
                let mut members = HashSet::new();
                if state["tracks"][index]["kind"] == "group" {
                    let mut pending = vec![reference.to_string()];
                    while let Some(group) = pending.pop() {
                        for track in array(&state["tracks"]) {
                            if track["groupTrackRef"] == group {
                                if let Some(r) = track["ref"].as_str() {
                                    if members.insert(r.to_string()) {
                                        pending.push(r.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                let deleted = state["tracks"].as_array_mut().unwrap().remove(index);
                state["tracks"].as_array_mut().unwrap().retain(|track| track["ref"].as_str().is_none_or(|r| !members.contains(r)));
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"track":deleted}));
                Ok(json!({"deleted":reference}))
            }
            "scene.create" => {
                self.require_structure_revision(args)?;
                let name = string_arg(args, "name")?;
                let mut state = self.state.borrow_mut();
                let size = array(&state["scenes"]).len();
                let index = if let Some(value) = args.get("index") {
                    value
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .filter(|v| *v <= size)
                        .ok_or_else(|| LiveError::range_error("scene index is invalid"))?
                } else {
                    size
                };
                let number = size as u64 + self.sequence.get() + 1;
                let reference = format!("scene:scene-{number}");
                let scene = json!({"ref":reference,"objectIdentity":format!("sim-object:scene:{number}"),"name":name,"index":index});
                state["scenes"].as_array_mut().unwrap().insert(index, scene.clone());
                for (i, scene) in state["scenes"].as_array_mut().unwrap().iter_mut().enumerate() {
                    scene["index"] = i.into();
                }
                for track in state["tracks"].as_array_mut().unwrap() {
                    let mut slots = array(&track["clipSlots"]).to_vec();
                    for slot in &mut slots {
                        if slot["sceneIndex"].as_u64().unwrap() >= index as u64 {
                            slot["sceneIndex"] = (slot["sceneIndex"].as_u64().unwrap() + 1).into();
                        }
                    }
                    let track_ref = track["ref"].as_str().unwrap();
                    slots.push(json!({"ref":format!("clip-slot:{track_ref}:{reference}"),"parentRef":track_ref,"objectIdentity":format!("simulator:clip-slot:{track_ref}:{reference}"),"sceneIndex":index,"clipRef":null,"empty":true}));
                    slots.sort_by_key(|slot| slot["sceneIndex"].as_u64().unwrap());
                    track["clipSlots"] = Value::Array(slots);
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.clone().into()), json!({"operation":operation,"scene":scene}));
                let mut result = self.get(&reference.clone().into())?.unwrap();
                result["createdFingerprint"] = self.structure_created_fingerprint("scene", &reference)?.into();
                Ok(result)
            }
            "scene.delete" => {
                self.require_structure_revision(args)?;
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let index = array(&state["scenes"])
                    .iter()
                    .position(|scene| scene["ref"] == reference)
                    .ok_or_else(|| LiveError::error(format!("unknown scene reference: {reference}")))?;
                if args.get("expectedObjectIdentity") != state["scenes"][index].get("objectIdentity") {
                    return Err(LiveError::error("scene object identity changed; deletion refused"));
                }
                state["scenes"].as_array_mut().unwrap().remove(index);
                for (i, scene) in state["scenes"].as_array_mut().unwrap().iter_mut().enumerate() {
                    scene["index"] = i.into();
                }
                for track in state["tracks"].as_array_mut().unwrap() {
                    track["clipSlots"] = Value::Array(
                        array(&track["clipSlots"])
                            .iter()
                            .filter(|slot| slot["sceneIndex"].as_u64() != Some(index as u64))
                            .cloned()
                            .map(|mut slot| {
                                let old = slot["sceneIndex"].as_u64().unwrap();
                                if old > index as u64 {
                                    slot["sceneIndex"] = (old - 1).into();
                                }
                                slot
                            })
                            .collect(),
                    );
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"ref":reference}));
                Ok(json!({"deleted":reference}))
            }
            "track.rename" | "scene.rename" | "clip.rename" | "device.rename" | "locator.rename" => {
                let reference = string_arg(args, "ref")?;
                let name = string_arg(args, "name")?;
                let mut state = self.state.borrow_mut();
                let path = match operation {
                    "track.rename" => {
                        array(&state["tracks"]).iter().position(|row| row["ref"] == reference).map(|i| format!("/tracks/{i}"))
                    }
                    "scene.rename" => {
                        array(&state["scenes"]).iter().position(|row| row["ref"] == reference).map(|i| format!("/scenes/{i}"))
                    }
                    "locator.rename" => array(&state["arrangement"]["locators"])
                        .iter()
                        .position(|row| row["ref"] == reference)
                        .map(|i| format!("/arrangement/locators/{i}")),
                    "clip.rename" => array(&state["tracks"])
                        .iter()
                        .enumerate()
                        .find_map(|(i, t)| {
                            array(&t["clips"]).iter().position(|row| row["ref"] == reference).map(|j| format!("/tracks/{i}/clips/{j}"))
                        })
                        .or_else(|| {
                            array(&state["arrangementClips"])
                                .iter()
                                .position(|item| item["clip"]["ref"] == reference)
                                .map(|i| format!("/arrangementClips/{i}/clip"))
                        }),
                    _ => array(&state["tracks"]).iter().enumerate().find_map(|(i, t)| {
                        array(&t["devices"]).iter().position(|row| row["ref"] == reference).map(|j| format!("/tracks/{i}/devices/{j}"))
                    }),
                };
                let target = path.as_ref().and_then(|path| state.pointer(path));
                let authority=match operation{
                    "track.rename"|"scene.rename"=>target.map(|target|simulator_revision(&json!({"ref":target["ref"],"objectIdentity":target["objectIdentity"],"name":target["name"]}))),
                    "locator.rename"=>state["arrangement"]["locatorRevision"].as_str().map(String::from),
                    "clip.rename"=>Some(if reference.starts_with("arrangement-clip:"){simulator_revision(&json!({"expectedObjectIdentity":target.map(|t|&t["objectIdentity"]),"expectedAuthorityRevision":Self::arrangement_authority_revision(&state,reference)?}))}else{simulator_revision(&Self::session_clip_authority(&state,reference)?)}),
                    _=>array(&state["tracks"]).iter().find_map(|track|array(&track["devices"]).iter().find(|device|device["ref"]==reference).map(|device|simulator_revision(&json!({"ref":device["ref"],"objectIdentity":device["objectIdentity"],"trackRef":track["ref"],"trackIdentity":track["objectIdentity"],"ownerRef":track["ref"],"ownerIdentity":track["objectIdentity"],"siblings":array(&track["devices"]).iter().map(|d|json!({"ref":d["ref"],"objectIdentity":d["objectIdentity"]})).collect::<Vec<_>>()})))),
                };
                if target.is_none()
                    || target.unwrap().get("objectIdentity") != args.get("expectedObjectIdentity")
                    || target.unwrap().get("name") != args.get("expectedName")
                    || authority.as_deref() != args.get("expectedAuthorityRevision").and_then(Value::as_str)
                {
                    return Err(LiveError::error("rename target identity, hierarchy, or name changed since preview"));
                }
                state.pointer_mut(&path.unwrap()).unwrap()["name"] = name.into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"name":name}));
                Ok(json!({"renamed":reference,"name":name}))
            }
            "clip.create" => {
                let reference = string_arg(args, "trackRef")?;
                let mut state = self.state.borrow_mut();
                let track_index = array(&state["tracks"])
                    .iter()
                    .position(|track| track["ref"] == reference && reference.starts_with("track:"))
                    .ok_or_else(|| LiveError::error("unknown track reference"))?;
                let kind = args
                    .get("kind")
                    .and_then(Value::as_str)
                    .filter(|s| ["midi", "audio"].contains(s))
                    .ok_or_else(|| LiveError::type_error("kind must be midi or audio"))?;
                let start = args
                    .get("start")
                    .filter(|v| !v.is_null())
                    .and_then(Value::as_f64)
                    .or_else(|| args.get("sceneIndex").and_then(Value::as_f64).map(|v| v * 4.0));
                let length = args.get("length").and_then(Value::as_f64);
                if start.is_none_or(|s| !s.is_finite() || s < 0.0) || length.is_none_or(|s| !s.is_finite() || s <= 0.0) {
                    return Err(LiveError::range_error("clip bounds are invalid"));
                }
                let start = start.unwrap();
                let length = length.unwrap();
                let track = &state["tracks"][track_index];
                let index = args.get("sceneIndex").and_then(Value::as_f64);
                let slot_index = array(&track["clipSlots"]).iter().position(|slot| index.is_some() && slot["sceneIndex"].as_f64() == index);
                let scene = array(&state["scenes"]).iter().find(|scene| index.is_some() && scene["index"].as_f64() == index);
                let valid = slot_index.zip(scene).is_some_and(|(i, scene)| {
                    let slot = &track["clipSlots"][i];
                    args.get("expectedTrackIdentity") == track.get("objectIdentity")
                        && args.get("expectedSlotRef") == slot.get("ref")
                        && args.get("expectedSlotIdentity") == slot.get("objectIdentity")
                        && args.get("expectedSceneRef") == scene.get("ref")
                        && args.get("expectedSceneIdentity") == scene.get("objectIdentity")
                        && slot["clipRef"].is_null()
                });
                if !valid {
                    return Err(LiveError::error("clip creation target identity changed since preview"));
                }
                let track = &mut state["tracks"][track_index];
                let suffix = format!("clip-{}-{}", array(&track["clips"]).len() + 1, self.sequence.get() + 1);
                let audio = kind == "audio";
                let clip = json!({"ref":format!("clip:{suffix}"),"objectIdentity":format!("simulator:clip:{suffix}"),"name":args.get("name").and_then(Value::as_str).filter(|s|!s.is_empty()).unwrap_or("New Clip"),"kind":kind,"start":start,"length":length,"notes":[],"notesRevision":simulator_revision(&json!([])),"warp":false,"takes":[],"automation":[],"isAudio":audio,"gain":if audio{json!(1)}else{Value::Null},"pitchCoarse":if audio{json!(0)}else{Value::Null},"pitchFine":if audio{json!(0)}else{Value::Null},"warpMode":if audio{json!(0)}else{Value::Null},"loopStart":if audio{json!(start)}else{Value::Null},"loopEnd":if audio{json!(start+length)}else{Value::Null},"warping":if audio{json!(true)}else{Value::Null},"fadeInLength":if audio{json!(0)}else{Value::Null},"fadeOutLength":if audio{json!(0)}else{Value::Null},"availableAudioFields":if audio{json!(["gain","pitchCoarse","pitchFine","warpMode","warping","fadeInLength","fadeOutLength","loopStart","loopEnd"])}else{json!([])}});
                track["clips"].as_array_mut().unwrap().push(clip.clone());
                let slot = &mut track["clipSlots"][slot_index.unwrap()];
                slot["clipRef"] = clip["ref"].clone();
                slot["empty"] = false.into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"clip":clip}));
                Ok(
                    json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"name":clip["name"],"length":clip["length"],"createdFingerprint":simulator_revision(&without_playback_state(&clip))}),
                )
            }
            "clip.delete" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let found = array(&state["tracks"])
                    .iter()
                    .enumerate()
                    .find_map(|(i, track)| array(&track["clips"]).iter().position(|clip| clip["ref"] == reference).map(|j| (i, j)))
                    .ok_or_else(|| LiveError::error(format!("unknown clip reference: {reference}")))?;
                let authority = Self::session_clip_authority(&state, reference)
                    .map_err(|_| LiveError::error("clip hierarchy identity changed; deletion refused"))?;
                if authority.as_object().unwrap().iter().any(|(key, value)| args.get(key) != Some(value)) {
                    return Err(LiveError::error("clip hierarchy identity changed; deletion refused"));
                }
                let (ti, ci) = found;
                let slot_index = array(&state["tracks"][ti]["clipSlots"]).iter().position(|slot| slot["clipRef"] == reference).unwrap();
                let scene_index = state["tracks"][ti]["clipSlots"][slot_index]["sceneIndex"].as_u64().unwrap() as usize;
                let remove_scene = reference.starts_with("clip:captured-") && state["scenes"][scene_index]["name"] == "Capture Target";
                let track = &mut state["tracks"][ti];
                let track_ref = track["ref"].as_str().unwrap().to_string();
                track["clips"].as_array_mut().unwrap().remove(ci);
                track["clipSlots"][slot_index]["clipRef"] = Value::Null;
                track["clipSlots"][slot_index]["empty"] = true.into();
                if remove_scene {
                    track["clipSlots"].as_array_mut().unwrap().remove(slot_index);
                    state["scenes"].as_array_mut().unwrap().remove(scene_index);
                    for (i, scene) in state["scenes"].as_array_mut().unwrap().iter_mut().enumerate() {
                        scene["index"] = i.into();
                    }
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(track_ref.into()), json!({"operation":operation,"ref":reference}));
                Ok(json!({"deleted":reference}))
            }
            "routing.set" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let track = state["tracks"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|track| track["ref"] == reference)
                    .filter(|track| args.get("expectedObjectIdentity") == track.get("objectIdentity"))
                    .ok_or_else(|| LiveError::error("routing track identity changed since preview"))?;
                let revision = simulator_revision(
                    &json!({"inputType":track["routing"]["inputType"],"inputSubRouting":track["routing"]["inputSubRouting"],"outputType":track["routing"]["outputType"],"outputSubRouting":track["routing"]["outputSubRouting"],"arm":track["armed"],"monitoring":track["monitoringState"]}),
                );
                if args.get("expectedStateRevision") != Some(&Value::String(revision)) {
                    return Err(LiveError::error("routing state changed since preview"));
                }
                for key in ["inputType", "inputSubRouting", "outputType", "outputSubRouting"] {
                    if let Some(value) = args.get(key) {
                        if !track["routing"].is_object() {
                            track["routing"] = json!({});
                        }
                        track["routing"][key] = value.clone();
                        if key == "inputType" && value == "No Input" {
                            track["routing"]["inputSubRouting"] = Value::Null;
                        }
                    }
                }
                if let Some(value) = args.get("arm") {
                    if !value.is_boolean() {
                        return Err(LiveError::type_error("arm is invalid"));
                    }
                    track["armed"] = value.clone();
                }
                if let Some(value) = args.get("monitoring") {
                    if !value.as_str().is_some_and(|s| ["in", "auto", "off"].contains(&s)) {
                        return Err(LiveError::range_error("monitoring is invalid"));
                    }
                    track["monitoringState"] = value.clone();
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":self.next_sequence()}))
            }
            "mixer.set" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let track = state["tracks"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|track| track["ref"] == reference)
                    .filter(|track| track["mixer"].is_object())
                    .ok_or_else(|| LiveError::error("mixer is unavailable"))?;
                let mixer = &track["mixer"];
                let row: Map<String, Value> = ["volume", "pan", "mute", "solo", "cueVolume", "sends"]
                    .into_iter()
                    .map(|key| (key.into(), mixer[key].clone()))
                    .collect();
                if args.get("expectedObjectIdentity") == track.get("objectIdentity")
                    && args.get("expectedVolumeIdentity") == mixer.get("volumeIdentity")
                    && args.get("expectedPanIdentity") == mixer.get("panIdentity")
                    && args.get("expectedCueIdentity") == mixer.get("cueIdentity")
                    && !args.contains_key("expectedSendIdentities")
                {
                    return Err(LiveError::error("unsupported simulator authority value"));
                }
                if args.get("expectedObjectIdentity") != track.get("objectIdentity")
                    || args.get("expectedVolumeIdentity") != mixer.get("volumeIdentity")
                    || args.get("expectedPanIdentity") != mixer.get("panIdentity")
                    || args.get("expectedCueIdentity") != mixer.get("cueIdentity")
                    || args.get("expectedSendIdentities") != mixer.get("sendIdentities")
                    || args.get("expectedStateRevision") != Some(&Value::String(simulator_revision(&Value::Object(row))))
                {
                    return Err(LiveError::error("mixer track or parameter identity changed since preview"));
                }
                for key in ["volume", "pan", "mute", "solo", "cueVolume"] {
                    if let Some(value) = args.get(key) {
                        if key == "mute" || key == "solo" {
                            if !value.is_boolean() {
                                return Err(LiveError::type_error(format!("{key} is invalid")));
                            }
                        } else if !value.as_f64().is_some_and(|v| v.is_finite() && v >= if key == "pan" { -1.0 } else { 0.0 } && v <= 1.0) {
                            return Err(LiveError::range_error(format!("{key} is invalid")));
                        }
                        track["mixer"][key] = value.clone();
                        if key != "cueVolume" {
                            track[key] = value.clone();
                        }
                    }
                }
                if let Some(sends) = args.get("sends") {
                    if !sends.as_array().is_some_and(|s| {
                        s.len() <= array(&track["mixer"]["sends"]).len()
                            && s.iter().all(|v| v.as_f64().is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v)))
                    }) {
                        return Err(LiveError::range_error("sends are invalid"));
                    }
                    for (i, value) in array(sends).iter().enumerate() {
                        track["mixer"]["sends"][i] = value.clone();
                    }
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":self.next_sequence()}))
            }
            "note.add" => {
                let reference = string_arg(args, "ref")?;
                self.assert_note_authority(args, reference)?;
                let note: Note = from_live_json(args.get("note").cloned().unwrap_or(Value::Null))?;
                self.add_note(&reference.into(), &note)
            }
            "note.add-batch" => {
                let reference = string_arg(args, "ref")?;
                let input = args
                    .get("notes")
                    .and_then(Value::as_array)
                    .filter(|notes| !notes.is_empty() && notes.len() <= 512)
                    .ok_or_else(|| LiveError::range_error("note batch is invalid"))?;
                self.assert_note_authority(args, reference)?;
                let notes: Vec<Note> = from_live_json(Value::Array(input.clone()))?;
                {
                    let state = self.state.borrow();
                    let path = clip_path(&state, reference)?;
                    for note in &notes {
                        Self::validate_note_for_clip(state.pointer(&path).unwrap(), note)?;
                    }
                }
                let mut ids = Vec::new();
                for note in &notes {
                    ids.push(self.add_note(&reference.into(), note)?["noteId"].clone());
                }
                let state = self.state.borrow();
                let path = clip_path(&state, reference)?;
                Ok(json!({"added":ids.len(),"noteIds":ids,"notesRevision":state.pointer(&path).unwrap()["notesRevision"]}))
            }
            "note.update" => {
                let reference = string_arg(args, "ref")?;
                self.assert_note_authority(args, reference)?;
                let patches = args
                    .get("notes")
                    .and_then(Value::as_array)
                    .filter(|patches| !patches.is_empty() && patches.len() <= 512)
                    .ok_or_else(|| LiveError::range_error("note patches are invalid"))?;
                let mut seen = HashSet::new();
                for patch in patches {
                    let id = patch["id"]
                        .as_f64()
                        .filter(|id| id.is_finite() && id.fract() == 0.0 && *id >= 0.0)
                        .ok_or_else(|| LiveError::range_error("note patch id is invalid"))? as i64;
                    if !seen.insert(id) {
                        return Err(LiveError::range_error("duplicate note patch id"));
                    }
                }
                let mut state = self.state.borrow_mut();
                let path = clip_path(&state, reference)?;
                let clip = state.pointer_mut(&path).unwrap();
                for id in &seen {
                    if !array(&clip["notes"]).iter().any(|note| note["id"].as_i64() == Some(*id)) {
                        return Err(LiveError::error("note id is not present in the clip"));
                    }
                }
                for patch in patches {
                    let note =
                        clip["notes"].as_array_mut().unwrap().iter_mut().find(|note| note["id"].as_f64() == patch["id"].as_f64()).unwrap();
                    for key in ["pitch", "start", "duration", "velocity", "mute", "probability", "velocityDeviation", "releaseVelocity"] {
                        if let Some(value) = patch.get(key) {
                            note[key] = value.clone();
                        }
                    }
                }
                clip["notesRevision"] = simulator_revision(&clip["notes"]).into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"updated":seen.len()}))
            }
            "note.delete" => {
                let reference = string_arg(args, "ref")?;
                self.assert_note_authority(args, reference)?;
                let ids = args
                    .get("noteIds")
                    .and_then(Value::as_array)
                    .filter(|ids| !ids.is_empty() && ids.len() <= 512)
                    .ok_or_else(|| LiveError::range_error("note ids are invalid"))?;
                let mut unique = HashSet::new();
                for id in ids {
                    let id = id
                        .as_f64()
                        .filter(|id| id.is_finite() && id.fract() == 0.0 && *id >= 0.0)
                        .ok_or_else(|| LiveError::range_error("note ids are invalid"))? as i64;
                    if !unique.insert(id) {
                        return Err(LiveError::range_error("note ids are invalid"));
                    }
                }
                let mut state = self.state.borrow_mut();
                let path = clip_path(&state, reference)?;
                let clip = state.pointer_mut(&path).unwrap();
                for id in ids {
                    if !array(&clip["notes"]).iter().any(|note| &note["id"] == id) {
                        return Err(LiveError::error("note id is not present in the clip"));
                    }
                }
                clip["notes"].as_array_mut().unwrap().retain(|note| !ids.contains(&note["id"]));
                clip["notesRevision"] = simulator_revision(&clip["notes"]).into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"deleted":ids.len()}))
            }
            "device.parameter.set" => {
                let reference = self.checked_parameter(args)?;
                self.simulate_external_edit(&reference, "value", (args["value"].as_f64().unwrap() as f32 as f64).into())?;
                let target = self.get(&reference)?.unwrap();
                Ok(
                    json!({"changed":true,"ref":reference,"property":"value","value":target["value"],"revision":target.get("revision").unwrap_or(&json!(1))}),
                )
            }
            "device.parameters.set" => {
                let items = args
                    .get("parameters")
                    .and_then(Value::as_array)
                    .filter(|items| !items.is_empty() && items.len() <= 64)
                    .ok_or_else(|| LiveError::error("parameter authority is invalid"))?;
                let mut shared = Map::new();
                for key in ["expectedOwnerRef", "expectedOwnerIdentity", "expectedTrackRef", "expectedTrackIdentity", "expectedSiblings"] {
                    if let Some(value) = args.get(key) {
                        shared.insert(key.into(), value.clone());
                    }
                }
                let mut targets = Vec::new();
                for item in items {
                    let mut merged = shared.clone();
                    if let Some(item) = item.as_object() {
                        merged.extend(item.clone());
                    }
                    targets.push(self.checked_parameter(&merged)?);
                }
                if targets.iter().collect::<HashSet<_>>().len() != targets.len() {
                    return Err(LiveError::error("parameter changes name the same parameter twice"));
                }
                let mut priors = Vec::new();
                for (index, target) in targets.iter().enumerate() {
                    let prior = self.get(target)?.unwrap()["value"].clone();
                    if let Err(error) =
                        self.simulate_external_edit(target, "value", (items[index]["value"].as_f64().unwrap() as f32 as f64).into())
                    {
                        for (reference, value) in priors.into_iter().rev() {
                            self.simulate_external_edit(&reference, "value", value)?;
                        }
                        return Err(LiveError::error(format!("parameter {} of {}: {error}", index + 1, items.len())));
                    }
                    priors.push((target.clone(), prior));
                }
                let parameters = targets
                    .iter()
                    .map(|reference| {
                        let target = self.get(reference).unwrap().unwrap();
                        json!({"ref":reference,"value":target["value"],"revision":target.get("revision").unwrap_or(&json!(1))})
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"parameters":parameters}))
            }
            "browser.search" => {
                let query = args.get("query").and_then(Value::as_str).unwrap_or("").to_lowercase();
                let category = args.get("category").and_then(Value::as_str);
                let limit = args.get("limit").and_then(Value::as_u64).filter(|v| (1..=10_000).contains(v)).unwrap_or(50) as usize;
                let items = Self::browser_catalog()
                    .into_iter()
                    .filter(|item| {
                        category.is_none_or(|c| c.is_empty() || item["category"] == c)
                            && (query.is_empty()
                                || item["name"].as_str().unwrap().to_lowercase().contains(&query)
                                || item["path"].as_str().unwrap().contains(&query))
                    })
                    .take(limit)
                    .collect::<Vec<_>>();
                Ok(json!({"items":items}))
            }
            "browser.inspect" => {
                let id = string_arg(args, "itemId")?;
                if let Some(item) = Self::browser_catalog().into_iter().find(|item| item["id"] == id) {
                    return Ok(item);
                }
                if id.starts_with("user_library/") {
                    return Ok(
                        json!({"id":id,"objectIdentity":format!("simulator:browser:{id}"),"name":id.split('/').next_back().unwrap(),"category":"user_library","path":id,"isDevice":false}),
                    );
                }
                Err(LiveError::error("browser item is not present"))
            }
            "browser.roots" => {
                let mut state = json!({"roots":(["instruments","sounds","samples","user_library","current_project","legacy_libraries","tunings"].into_iter().map(|name|json!({"name":name,"binding":"unofficial-internal","searchable":!["legacy_libraries","tunings"].contains(&name)})).collect::<Vec<_>>()),"previewAvailable":false,"bindingEvidence":"shape-probed on the connected build (Live simulator); undocumented Remote Script internals, version-specific"});
                state["revision"] = simulator_revision(&state).into();
                Ok(state)
            }
            "ownership.settle" => {
                let reference = string_arg(args, "ref")?;
                let state = self.state.borrow();
                let device = all_device_rows(&state).into_iter().find(|device| device["ref"] == reference);
                if device.is_none()
                    || device.unwrap().get("objectIdentity") != args.get("expectedObjectIdentity")
                    || !args.get("expectedFingerprint").is_some_and(Value::is_string)
                {
                    return Err(LiveError::error("ownership settle identity changed"));
                }
                Ok(json!({"settled":true,"fingerprint":args["expectedFingerprint"]}))
            }
            "locator.add" => {
                let name = string_arg(args, "name")?;
                let position = args.get("position").and_then(Value::as_f64);
                let mut state = self.state.borrow_mut();
                let arrangement = &mut state["arrangement"];
                if args.get("expectedCollectionRevision") != arrangement.get("locatorRevision") {
                    return Err(LiveError::error("locator collection changed since preview"));
                }
                let position =
                    position.filter(|p| p.is_finite() && *p >= 0.0).ok_or_else(|| LiveError::range_error("locator position is invalid"))?;
                let index = array(&arrangement["locators"]).len() + 1;
                let locator = json!({"ref":format!("locator:locator-{index}"),"objectIdentity":format!("simulator:locator:{index}"),"name":name,"position":position});
                arrangement["locators"].as_array_mut().unwrap().push(locator.clone());
                arrangement["locatorRevision"] = simulator_revision(&arrangement["locators"]).into();
                drop(state);
                self.emit(
                    LiveEventType::Object,
                    Some(locator["ref"].as_str().unwrap().into()),
                    json!({"operation":operation,"locator":locator}),
                );
                let mut result = locator;
                result["createdFingerprint"] = simulator_revision(&result).into();
                Ok(result)
            }
            "locator.delete" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let arrangement = &mut state["arrangement"];
                let index = array(&arrangement["locators"]).iter().position(|item| item["ref"] == reference);
                if index.is_none()
                    || args.get("expectedCollectionRevision") != arrangement.get("locatorRevision")
                    || args.get("expectedObjectIdentity") != arrangement["locators"][index.unwrap()].get("objectIdentity")
                {
                    return Err(LiveError::error("locator identity or collection changed since preview"));
                }
                let deleted = arrangement["locators"].as_array_mut().unwrap().remove(index.unwrap());
                arrangement["locatorRevision"] = simulator_revision(&arrangement["locators"]).into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"locator":deleted}));
                Ok(json!({"deleted":reference}))
            }
            _ => Err(LiveError::error(format!("unknown operation: {operation}"))),
        }
    }
    fn session_clip_playback(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        use serde_json::json;
        let launching = operation == "session.clip-launch";
        let reference = string_arg(args, if launching { "slotRef" } else { "trackRef" })?;
        let mut state = self.state.borrow_mut();
        if launching && args.get("playbackRevision") != state["playback"].get("revision") {
            return Err(LiveError::error("playback state changed since preview"));
        }
        if !launching {
            for active in array(&state["playback"]["firedTargets"]).iter().chain(array(&state["playback"]["playingTargets"])) {
                if active["trackRef"] == reference
                    && ["trackRef", "clipSlotRef", "sceneRef", "clipRef", "sceneIndex"]
                        .into_iter()
                        .any(|key| args.get(if key == "clipSlotRef" { "slotRef" } else { key }) != active.get(key))
                {
                    return Err(LiveError::error("track has foreign playback targets"));
                }
            }
        }
        let found = array(&state["tracks"]).iter().enumerate().find_map(|(i, track)| {
            array(&track["clipSlots"])
                .iter()
                .enumerate()
                .find(|(_, slot)| {
                    args.get("slotRef") == slot.get("ref")
                        && args.get("trackRef") == track.get("ref")
                        && args.get("clipRef") == slot.get("clipRef")
                        && !slot["clipRef"].is_null()
                })
                .map(|(j, _)| (i, j))
        });
        let Some((ti, si)) = found else {
            return Err(LiveError::error(if launching {
                "clip slot with a clip is required"
            } else {
                "clip-stop object identity changed"
            }));
        };
        let track = &state["tracks"][ti];
        let slot = &track["clipSlots"][si];
        let scene = array(&state["scenes"]).iter().find(|scene| scene["index"] == slot["sceneIndex"]);
        let clip = array(&track["clips"]).iter().find(|clip| clip["ref"] == slot["clipRef"]);
        let valid = scene.zip(clip).is_some_and(|(scene, clip)| {
            args.get("sceneRef") == scene.get("ref")
                && args.get("sceneIndex") == scene.get("index")
                && args.get("trackIdentity") == track.get("objectIdentity")
                && args.get("slotIdentity") == slot.get("objectIdentity")
                && args.get("sceneIdentity") == scene.get("objectIdentity")
                && args.get("clipIdentity") == clip.get("objectIdentity")
        });
        if !valid {
            return Err(LiveError::error(if launching {
                "clip-launch object identity changed"
            } else {
                "clip-stop object identity changed"
            }));
        }
        let target = json!({"trackRef":track["ref"],"clipSlotRef":slot["ref"],"sceneRef":scene.unwrap()["ref"],"sceneIndex":slot["sceneIndex"],"clipRef":slot["clipRef"]});
        if launching {
            state["set"]["playing"] = true.into();
            state["playback"]["transport"]["playing"] = true.into();
            for key in ["firedTargets", "playingTargets"] {
                let targets = state["playback"][key].as_array_mut().unwrap();
                targets.retain(|item| item["clipSlotRef"] != target["clipSlotRef"]);
                targets.push(target.clone());
            }
            for key in ["firedSlotIndex", "playingSlotIndex"] {
                state["tracks"][ti][key] = target["sceneIndex"].clone();
            }
            state["playback"]["revision"] = format!("{}:clip:{}", self.epoch.get(), target["sceneRef"].as_str().unwrap()).into();
        } else {
            for key in ["firedTargets", "playingTargets"] {
                state["playback"][key].as_array_mut().unwrap().retain(|item| item["trackRef"] != target["trackRef"]);
            }
            for key in ["firedSlotIndex", "playingSlotIndex"] {
                state["tracks"][ti][key] = Value::Null;
            }
            if array(&state["playback"]["firedTargets"]).is_empty() && array(&state["playback"]["playingTargets"]).is_empty() {
                state["set"]["playing"] = false.into();
                state["playback"]["transport"]["playing"] = false.into();
            }
            state["playback"]["revision"] = format!("{}:track-stop:{reference}", self.epoch.get()).into();
        }
        drop(state);
        let payload = if launching { json!({"operation":operation,"slot":reference}) } else { json!({"operation":operation}) };
        self.emit(LiveEventType::Transport, Some(reference.into()), payload);
        Ok(if launching { json!({"launched":reference,"targets":[target]}) } else { json!({"stopped":true}) })
    }
}

impl DeterministicLiveSimulator {
    fn require_structure_revision(&self, args: &Map<String, Value>) -> Result<(), LiveError> {
        let state = self.state.borrow();
        let revision = crate::registry::sha256_hex(&kumi_common::js::json::stringify(
            &serde_json::json!({"tracks":array(&state["tracks"]).iter().enumerate().map(|(index,item)|serde_json::json!([item["ref"],item["objectIdentity"],item["name"],item["kind"],index])).collect::<Vec<_>>(),"scenes":array(&state["scenes"]).iter().enumerate().map(|(index,item)|serde_json::json!([item["ref"],item["objectIdentity"],item["name"],index])).collect::<Vec<_>>()}),
        ));
        if args.get("expectedStructureRevision") != Some(&Value::String(revision)) {
            Err(LiveError::error("Session structure changed since preview"))
        } else {
            Ok(())
        }
    }
    fn arrangement_authority_revision(state: &Value, reference: &str) -> Result<String, LiveError> {
        let item = array(&state["arrangementClips"])
            .iter()
            .find(|item| item["clip"]["ref"] == reference)
            .ok_or_else(|| LiveError::error("Arrangement clip hierarchy is unavailable"))?;
        let track = array(&state["tracks"])
            .iter()
            .find(|track| track["ref"] == item["trackRef"])
            .ok_or_else(|| LiveError::error("Arrangement clip hierarchy is unavailable"))?;
        Ok(simulator_revision(
            &serde_json::json!({"clip":{"ref":reference,"objectIdentity":item["clip"]["objectIdentity"]},"owner":{"ref":track["ref"],"objectIdentity":track["objectIdentity"]},"siblings":array(&state["arrangementClips"]).iter().filter(|item|item["trackRef"]==track["ref"]).map(|item|serde_json::json!({"ref":item["clip"]["ref"],"objectIdentity":item["clip"]["objectIdentity"]})).collect::<Vec<_>>()}),
        ))
    }
    fn structure_created_fingerprint(&self, kind: &str, reference: &str) -> Result<String, LiveError> {
        use serde_json::json;
        let snapshot = self.snapshot_value();
        if kind == "track" {
            let track = array(&snapshot["tracks"])
                .iter()
                .find(|track| track["ref"] == reference)
                .ok_or_else(|| LiveError::error("created track fingerprint is unavailable"))?;
            let typed: Track = from_live_json(track.clone())?;
            let owned = owned_track_fingerprint_row(&typed);
            let clips = array(&snapshot["arrangement"]["clips"])
                .iter()
                .filter(|clip| clip["trackRef"] == reference || clip["parentRef"] == reference)
                .cloned()
                .collect::<Vec<_>>();
            return Ok(simulator_revision(&json!({"track":owned,"arrangementClips":clips})));
        }
        let scene = array(&snapshot["scenes"])
            .iter()
            .find(|scene| scene["ref"] == reference)
            .ok_or_else(|| LiveError::error("created scene fingerprint is unavailable"))?;
        let scene_identity = json!({"ref":scene["ref"],"parentRef":scene["parentRef"],"objectIdentity":scene["objectIdentity"],"name":scene["name"],"triggerable":scene["triggerable"]});
        let contents=array(&snapshot["tracks"]).iter().map(|track|{let slot=array(&track["clipSlots"]).iter().find(|slot|slot["sceneIndex"]==scene["index"]);let clip=slot.and_then(|slot|array(&track["clips"]).iter().find(|clip|clip["ref"]==slot["clipRef"]));let owned=slot.map(|slot|json!({"ref":slot["ref"],"parentRef":slot["parentRef"],"trackRef":slot["trackRef"],"objectIdentity":slot["objectIdentity"],"clipRef":slot["clipRef"],"empty":slot["empty"]}));json!({"trackRef":track["ref"],"trackIdentity":track["objectIdentity"],"slot":owned,"clip":clip})}).collect::<Vec<_>>();
        Ok(simulator_revision(&json!({"scene":scene_identity,"contents":contents})))
    }
}

fn ranged_number(value: &Value, min: f64, max: f64, integer: bool, message: &str) -> Result<f64, LiveError> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && *v >= min && *v <= max && (!integer || v.fract() == 0.0))
        .ok_or_else(|| LiveError::range_error(message))
}
fn bounded_text(value: &Value, max: usize, message: &str) -> Result<String, LiveError> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= max)
        .map(String::from)
        .ok_or_else(|| LiveError::range_error(message))
}
fn song_settings_state(song: &Value) -> Value {
    serde_json::json!({"selectOnLaunch":song["selectOnLaunch"],"signatureNumerator":song["signatureNumerator"],"signatureDenominator":song["signatureDenominator"],"swingAmount":song["swingAmount"],"clipTriggerQuantization":song["clipTriggerQuantization"]["value"],"midiRecordingQuantization":song["midiRecordingQuantization"]["value"]})
}
impl DeterministicLiveSimulator {
    fn invoke_set_state(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        use serde_json::json;
        let mut state = self.state.borrow_mut();
        match operation {
            "tuning.read" => Ok(
                json!({"tuningSystem":state["tuning"]["system"],"scale":state["tuning"]["scale"],"revision":simulator_revision(&state["tuning"])}),
            ),
            "groove.read" => Ok(
                json!({"grooveAmount":state["groovePool"]["amount"],"grooves":state["groovePool"]["grooves"],"revision":simulator_revision(&state["groovePool"])}),
            ),
            "song.read" => {
                let mut result = state["song"].clone();
                result["revision"] = simulator_revision(&result).into();
                Ok(result)
            }
            "tuning.set" | "groove.set" | "song.set" => {
                if args.get("setRef") != state["set"].get("ref") || args.get("expectedObjectIdentity") != state["set"].get("objectIdentity")
                {
                    return Err(LiveError::error("Set identity changed since preview"));
                }
                let field = if operation == "tuning.set" {
                    "tuning"
                } else if operation == "groove.set" {
                    "groovePool"
                } else {
                    "song"
                };
                let prior = if field == "song" { song_settings_state(&state[field]) } else { state[field].clone() };
                let expected = if field == "song" { "expectedStateRevision" } else { "expectedRevision" };
                if args.get(expected) != Some(&Value::String(simulator_revision(&prior))) {
                    return Err(LiveError::error(match field {
                        "tuning" => "tuning or scale state changed since preview",
                        "groovePool" => "groove state changed since preview",
                        _ => "song settings state changed since preview",
                    }));
                }
                let target = &mut state[field];
                if field == "tuning" {
                    if let Some(value) = args.get("name") {
                        target["system"]["name"] = bounded_text(value, 256, "name is invalid")?.into();
                    }
                    for key in ["lowestNote", "highestNote", "referencePitch"] {
                        if let Some(value) = args.get(key) {
                            let valid = value.as_object().is_some_and(|fields| {
                                fields.len() <= 8
                                    && fields.iter().all(|(key, value)| {
                                        let length = kumi_common::js::string::utf16_len(key);
                                        length > 0
                                            && length <= 64
                                            && (value.is_null()
                                                || value.is_boolean()
                                                || value.as_f64().is_some_and(|v| v.is_finite() && v.abs() <= 1e9)
                                                || value.as_str().is_some_and(|s| kumi_common::js::string::utf16_len(s) <= 256))
                                    })
                            });
                            if !valid {
                                return Err(LiveError::range_error("tuning setting dictionaries are invalid"));
                            }
                            target["system"][key] = value.clone();
                        }
                    }
                    if let Some(value) = args.get("noteTunings") {
                        if !value.as_array().is_some_and(|rows| {
                            rows.len() == 128
                                && rows.iter().all(|row| {
                                    row["note"].as_f64().is_some_and(|n| n.is_finite() && n.fract() == 0.0 && (0.0..=127.0).contains(&n))
                                        && row["deviation"].as_f64().is_some_and(|n| n.is_finite() && n.abs() <= 1200.0)
                                })
                        }) {
                            return Err(LiveError::range_error("noteTunings must contain exactly 128 valid entries"));
                        }
                        if array(value).iter().map(|row| row["note"].as_f64().unwrap() as i64).collect::<HashSet<_>>().len() != 128 {
                            return Err(LiveError::range_error("noteTunings notes are invalid"));
                        }
                        target["system"]["noteTunings"] = value.clone();
                    }
                    if let Some(value) = args.get("rootNote") {
                        ranged_number(value, 0.0, 11.0, true, "rootNote is invalid")?;
                        target["scale"]["rootNote"] = value.clone();
                    }
                    if let Some(value) = args.get("scaleName") {
                        target["scale"]["scaleName"] = bounded_text(value, 256, "scaleName is invalid")?.into();
                    }
                    if let Some(value) = args.get("scaleMode") {
                        if !value.is_boolean() {
                            return Err(LiveError::range_error("scaleMode is invalid"));
                        }
                        target["scale"]["scaleMode"] = value.clone();
                    }
                    if args.contains_key("scaleIntervals") {
                        return Err(LiveError::range_error("scaleIntervals is read-only and cannot be assigned"));
                    }
                } else if field == "groovePool" {
                    target["amount"] =
                        ranged_number(args.get("grooveAmount").unwrap_or(&Value::Null), 0.0, 1.3, false, "grooveAmount is invalid")?.into();
                } else {
                    for key in ["signatureNumerator", "signatureDenominator"] {
                        if let Some(value) = args.get(key) {
                            ranged_number(value, 1.0, 99.0, true, &format!("{key} is invalid"))?;
                            target[key] = value.clone();
                        }
                    }
                    if let Some(value) = args.get("swingAmount") {
                        ranged_number(value, 0.0, 1.0, false, "swingAmount is invalid")?;
                        target["swingAmount"] = value.clone();
                    }
                    if let Some(value) = args.get("clipTriggerQuantization") {
                        let index = ranged_number(value, 0.0, 13.0, true, "clipTriggerQuantization is invalid")? as usize;
                        let names = [
                            "none", "8-bars", "4-bars", "2-bars", "1-bar", "1/2", "1/2T", "1/4", "1/4T", "1/8", "1/8T", "1/16", "1/16T",
                            "1/32",
                        ];
                        target["clipTriggerQuantization"] = json!({"name":names[index],"value":value});
                    }
                    if let Some(value) = args.get("selectOnLaunch") {
                        if !value.is_boolean() {
                            return Err(LiveError::type_error("selectOnLaunch is invalid"));
                        }
                        target["selectOnLaunch"] = value.clone();
                    }
                    if let Some(value) = args.get("midiRecordingQuantization") {
                        let index = ranged_number(value, 0.0, 8.0, true, "midiRecordingQuantization is invalid")? as usize;
                        target["midiRecordingQuantization"] = json!({"name":format!("rec_quantisation_{index}"),"value":value});
                    }
                }
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                let state = self.state.borrow();
                let next = if field == "song" { song_settings_state(&state[field]) } else { state[field].clone() };
                Ok(json!({"changed":true,"revision":simulator_revision(&next)}))
            }
            "groove.edit" => {
                let reference = string_arg(args, "ref")?;
                let index = array(&state["groovePool"]["grooves"])
                    .iter()
                    .position(|g| g["ref"] == reference)
                    .ok_or_else(|| LiveError::error("groove reference is stale or invalid"))?;
                if args.get("expectedObjectIdentity") != state["groovePool"]["grooves"][index].get("objectIdentity") {
                    return Err(LiveError::error("groove identity changed since preview"));
                }
                if args.get("expectedRevision") != Some(&Value::String(simulator_revision(&state["groovePool"]))) {
                    return Err(LiveError::error("groove state changed since preview"));
                }
                let groove = &mut state["groovePool"]["grooves"][index];
                if let Some(value) = args.get("name") {
                    groove["name"] = bounded_text(value, 256, "name is invalid")?.into();
                }
                if let Some(value) = args.get("base") {
                    ranged_number(value, 0.0, 16.0, true, "base is invalid")?;
                    groove["base"] = value.clone();
                }
                for key in ["quantizationAmount", "randomAmount", "timingAmount", "velocityAmount"] {
                    if let Some(value) = args.get(key) {
                        ranged_number(value, 0.0, 1.0, false, &format!("{key} is invalid"))?;
                        groove[key] = value.clone();
                    }
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":simulator_revision(&self.state.borrow()["groovePool"])}))
            }
            "song.time-convert" => {
                if args.get("setRef") != state["set"].get("ref") {
                    return Err(LiveError::error("set reference is stale or invalid"));
                }
                match args.get("query").and_then(Value::as_str) {
                    Some("beats-loop") => Ok(
                        json!({"available":true,"loopStart":state["playback"]["transport"]["loop"].get("start").filter(|v|!v.is_null()).unwrap_or(&json!(0)),"loopLength":state["playback"]["transport"]["loop"].get("length").filter(|v|!v.is_null()).unwrap_or(&json!(4)),"smpte":null}),
                    ),
                    Some("current-smpte") => {
                        let format = match args.get("smpteFormat").filter(|v| !v.is_null()) {
                            Some(value) => value.as_str().ok_or_else(|| LiveError::range_error("smpteFormat is invalid"))?,
                            None => "smpte-25",
                        };
                        if !["smpte-24", "smpte-25", "smpte-29", "smpte-30", "smpte-30-drop"].contains(&format) {
                            return Err(LiveError::range_error("smpteFormat is invalid"));
                        }
                        let tempo = state["set"]["tempo"]
                            .as_f64()
                            .filter(|v| v.is_finite() && (20.0..=999.0).contains(v))
                            .ok_or_else(|| LiveError::error("tempo is unavailable for time conversion"))?;
                        let total = (state["playback"]["transport"]["position"].as_f64().unwrap_or(0.0) * 60.0 / tempo).max(0.0);
                        let fps = if format == "smpte-24" {
                            24.0
                        } else if format == "smpte-30" || format == "smpte-30-drop" {
                            30.0
                        } else {
                            25.0
                        };
                        Ok(
                            json!({"available":true,"loopStart":null,"loopLength":null,"smpte":{"hours":(total/3600.0).floor(),"minutes":((total%3600.0)/60.0).floor(),"seconds":(total%60.0).floor(),"frames":((total-total.floor())*fps).floor()}}),
                        )
                    }
                    _ => Err(LiveError::range_error("time-convert query is invalid")),
                }
            }
            "transport.action" => {
                if args.get("setRef") != state["set"].get("ref") || args.get("expectedObjectIdentity") != state["set"].get("objectIdentity")
                {
                    return Err(LiveError::error("Set identity changed since preview"));
                }
                if args.get("expectedRevision") != state["playback"].get("revision") {
                    return Err(LiveError::error("transport state changed since preview"));
                }
                let finite = |key: &str, message: &str| {
                    args.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| LiveError::range_error(message))
                };
                match args.get("action").and_then(Value::as_str) {
                    Some("start" | "continue" | "play-selection") => {
                        state["playback"]["transport"]["playing"] = true.into();
                        state["set"]["playing"] = true.into();
                    }
                    Some("stop") => {
                        state["playback"]["transport"]["playing"] = false.into();
                        state["set"]["playing"] = false.into();
                    }
                    Some("force-link-beat-time") => {
                        let beat = finite("beatTime", "beatTime is required for force-link-beat-time")?;
                        state["playback"]["transport"]["position"] = beat.into();
                        state["set"]["position"] = beat.into();
                    }
                    Some("stop-all-clips") => {
                        for track in state["tracks"].as_array_mut().unwrap() {
                            track["playingSlotIndex"] = Value::Null;
                            track["firedSlotIndex"] = Value::Null;
                        }
                        state["playback"]["transport"]["playing"] = false.into();
                        state["set"]["playing"] = false.into();
                    }
                    Some("back-to-arrangement") => {
                        for track in state["tracks"].as_array_mut().unwrap() {
                            track["backToArranger"] = false.into();
                        }
                        if state["song"].is_object() {
                            state["song"]["backToArranger"] = false.into();
                        }
                    }
                    Some("scrub") => {
                        let beat = finite("beatTime", "beatTime distance is required for scrub")?;
                        state["playback"]["transport"]["position"] =
                            (state["playback"]["transport"]["position"].as_f64().unwrap_or(0.0) + beat).into();
                    }
                    Some("jump-by") => {
                        let beats = finite("beats", "beats is required to jump")?;
                        state["playback"]["transport"]["position"] =
                            (state["playback"]["transport"]["position"].as_f64().unwrap_or(0.0) + beats).max(0.0).into();
                        state["set"]["position"] = state["playback"]["transport"]["position"].clone();
                    }
                    Some("tap-tempo" | "nudge-up" | "nudge-down" | "re-enable-automation" | "trigger-session-record") => {}
                    _ => return Err(LiveError::range_error("transport action is invalid")),
                }
                let revision = format!("{}:transport:{}", self.epoch.get(), self.next_sequence());
                state["playback"]["revision"] = revision.clone().into();
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
                Ok(json!({"done":true,"revision":revision}))
            }
            _ => unreachable!("set-state dispatcher only receives explicit operation names"),
        }
    }
}

#[path = "live_simulator_devices.rs"]
mod simulator_devices;

#[path = "live_simulator_automation.rs"]
mod simulator_automation;

#[path = "live_simulator_views.rs"]
mod simulator_views;

#[path = "live_simulator_structure.rs"]
mod simulator_structure;

#[path = "live_simulator_clips.rs"]
mod simulator_clips;

#[path = "live_simulator_device_state.rs"]
mod simulator_device_state;

#[path = "live_simulator_racks.rs"]
mod simulator_racks;

#[path = "live_simulator_lom.rs"]
mod simulator_lom;

#[path = "live_simulator_session.rs"]
mod simulator_session;

#[path = "live_simulator_observe.rs"]
mod simulator_observe;

#[path = "live_simulator_extensions.rs"]
mod simulator_extensions;
