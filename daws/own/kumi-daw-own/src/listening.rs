//! Listening in a DAW whose bridge is Kumi's own (REAPER's extension, Renoise's tool): the shared listening tools
//! (`Listening`) over the host's listening tools (M5 contract). A part is rendered offline, silently and faster than it
//! plays, and the main thing heard is played out loud after, so the producer hears what Kumi heard; REAPER also hears
//! what it plays as it plays. What each DAW's sources hold, its gain stage, its equaliser and its master chain are its
//! table here; the rest is the same for both.
use crate::bridge::{names_tracks, Refused};
use crate::changes::{Link, OwnPlan};
use crate::groove::Part;
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_agent::core::{
    context::{IntegrationContext, ListeningPolicy},
    contracts::*,
    refs::RefScheme,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::string::head,
};
use kumi_library::plugins::roles::Seen;
use kumi_listening::{
    checklist::MainState,
    engine::Listening,
    fit::{Band, Shape},
    heard_settings::DeviceIdentity,
    judging::Placed,
    knobs::Scale,
    notes::Kit,
    renderer::{
        bars_of, AuditionOptions, AuditionPass, AuditionRender, AuditionSource, Captured, ClipNotes, Copies, DeviceKnob, EqFit, ListenFlag,
        NotePatch, Pass, Renderer, Window,
    },
    tools::{audition_tool, form_tool, groove_tool, judge_tool, render_tool, sound_tool, tune_tool},
};
use kumi_types::contracts::*;
use kumi_types::display::parse_display;
use kumi_types::{
    daw::{DawId, DawProfile},
    errors::{HearFailure, RuntimeError},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

/// How long a listen played out loud after its render may run, at most (the rest is there to play with audition).
const PLAYED_SECONDS: f64 = 60.;
/// How long the track list read for one listen stays good.
const TRACKS_FRESH: Duration = Duration::from_secs(1);
/// The most tracks read to find one by name (pages of 200).
const TRACK_PAGES: usize = 50;
/// The points a parameter's text is read at across its range, as Live's scan reads them.
const GRID: u64 = 49;
/// The longest a job is waited for before Kumi stops it (a whole song with heavy plug-ins).
const JOB_LIMIT: Duration = Duration::from_secs(30 * 60);
/// How long a render the DAW answers busy is asked again, and how often: Renoise's tool answers busy while it reads a dense
/// song's tempo commands, a slice each time it's asked and more between, so a moment later it goes on from there.
const BUSY_WAIT: Duration = Duration::from_secs(20);
const BUSY_PACE: Duration = Duration::from_secs(1);

fn hear_refused(code: &'static str, reason: impl Into<String>) -> RuntimeError {
    HearFailure::Refused { code, reason: reason.into(), marked_modified: false }.into()
}

fn after_restoration(mut error: RuntimeError, marked_modified: bool) -> RuntimeError {
    match &mut error {
        RuntimeError::Hear(HearFailure::Unsure { code, restored, marked_modified: marked, .. }) => {
            // A later matching snapshot cannot erase the native restorer's explicit failure.
            *restored = native_tempo_failure(*code, Some(DawId::Renoise)).is_none();
            *marked = marked_modified;
        }
        RuntimeError::Hear(HearFailure::Refused { marked_modified: marked, .. }) => *marked = marked_modified,
        RuntimeError::Aborted => return HearFailure::Cancelled { marked_modified }.into(),
        _ => {}
    }
    error
}

fn native_tempo_failure(code: Option<&str>, daw: Option<DawId>) -> Option<(&'static str, &'static str)> {
    if daw != Some(DawId::Renoise) {
        return None;
    }
    match code {
        Some("tempo_restore_state_changed") => Some((
            "tempo_restore_state_changed",
            "Renoise's document or transport changed; tempo restoration could not be verified. Observe again.",
        )),
        Some("tempo_restore_ownership_uncertain") => Some((
            "tempo_restore_ownership_uncertain",
            "Renoise's BPM/LPB pair could not be attributed to this render and was left unchanged. Observe again.",
        )),
        Some("tempo_restore_failed") => Some(("tempo_restore_failed", "Renoise could not verify its BPM/LPB restoration. Observe again.")),
        _ => None,
    }
}

/// Keep the native cause even when the independent full-state comparison also fails.
/// Native prose and unrecognized codes never cross this boundary.
fn keep_native_tempo_failure(mut error: RuntimeError, native: Option<&str>, daw: Option<DawId>) -> RuntimeError {
    if let RuntimeError::Hear(HearFailure::Unsure { code, reason, restored, .. }) = &mut error {
        if let Some((known, message)) = native_tempo_failure(native, daw) {
            *code = Some(known);
            *restored = false;
            reason.clear();
            reason.push_str(message);
        }
    }
    error
}

/// Native error text can contain cache paths and producer-controlled strings. Only its typed
/// code crosses the restricted boundary; standalone keeps its detailed bridge diagnostics.
fn native_hear_refused(code: Option<&str>, fallback: &'static str, daw: Option<DawId>) -> RuntimeError {
    if let Some((code, reason)) = native_tempo_failure(code, daw) {
        return HearFailure::Unsure { code: Some(code), reason: reason.into(), restored: false, marked_modified: false }.into();
    }
    if code == Some("uncertain") {
        return HearFailure::Unsure {
            code: None,
            reason: "The native renderer could not confirm its result. Observe again before another action.".into(),
            restored: false,
            marked_modified: false,
        }
        .into();
    }
    let (code, reason) = match code {
        Some("stale" | "changed") => ("stale", "The project or approved window changed; observe and preview again."),
        Some("gone") => ("stale_ref", "The observed source is no longer available; observe again."),
        Some("busy") if daw == Some(DawId::Renoise) => (
            "render_busy",
            "Renoise is not ready to render. Stop playback and rendering, turn off Edit Mode, and wait for tempo scanning to finish.",
        ),
        Some("busy") => ("render_busy", "The DAW is busy or its transport is running; stop playback and recording and try again."),
        Some("bad-params") => ("invalid_window", "The bridge cannot render the requested window exactly; choose a valid beat interval."),
        Some("unsupported" | "unknown-method") => ("unsupported", "The bridge does not support the required bounded rendering operation."),
        _ => match fallback {
            "transport_unknown" => ("transport_unknown", "The bridge could not establish the current transport state."),
            "stale_ref" => ("stale_ref", "The observed track could not be read; observe again."),
            "render_failed" => ("render_failed", "The offline render failed after its touched state was read back."),
            "render_unavailable" => ("render_unavailable", "The native renderer is unavailable; no render was started."),
            _ => ("unsupported", "The bridge could not establish the required bounded render preflight."),
        },
    };
    hear_refused(code, reason)
}

fn start_refused_before_mutation(daw: Option<DawId>, code: Option<&str>) -> bool {
    matches!(code, Some("stale" | "busy" | "bad-params"))
        || (daw == Some(DawId::Reaper) && matches!(code, Some("failed" | "gone" | "unsupported" | "unknown-method")))
}

/// The restricted facade never inherits standalone's implicit source, loop, playhead or duration.
fn bounded_hear_window(request: &HearRequest) -> Result<Window, RuntimeError> {
    let mix = request.mix == Some(true);
    if (mix && !request.tracks.is_empty()) || (!mix && request.tracks.len() != 1) {
        return Err(hear_refused("invalid_source", "Give exactly one fresh track ref or mix: true, not both."));
    }
    if request.whole == Some(true) || request.seconds.is_some() {
        return Err(hear_refused("invalid_window", "Restricted listening needs an explicit beat window, not whole-song or seconds."));
    }
    match (request.from_beat, request.beats) {
        (Some(from), Some(beats))
            if from.is_finite() && from >= 0. && beats.is_finite() && beats > 0. && beats <= 256. && (from + beats).is_finite() =>
        {
            Ok(Window { from, beats })
        }
        _ => Err(hear_refused("invalid_window", "Give finite from_beat >= 0 and 0 < beats <= 256 with a finite end.")),
    }
}

fn bounded_hear_transport(transport: &Value) -> Result<(), RuntimeError> {
    let playing = transport.get("playing").and_then(Value::as_bool);
    let recording = transport.get("recording").and_then(Value::as_bool);
    if playing == Some(true) || recording == Some(true) || transport.get("paused") == Some(&Value::Bool(true)) {
        return Err(hear_refused("transport_running", "Stop playback and recording before listening; Kumi will not stop them for you."));
    }
    if playing != Some(false) || recording != Some(false) {
        return Err(hear_refused("transport_unknown", "The bridge did not establish that playback and recording are stopped."));
    }
    transport
        .get("tempo")
        .and_then(Value::as_f64)
        .filter(|tempo| tempo.is_finite() && *tempo > 0.)
        .ok_or_else(|| hear_refused("tempo_unknown", "The bridge did not report a finite positive current tempo."))?;
    Ok(())
}

struct RestrictedPreflight {
    preview: HearPreview,
    window: Window,
    source: Value,
    epoch: String,
    state: Value,
    focus: Value,
}

const RENDER_NUMBERS: &[&str] = &[
    "RENDER_SETTINGS",
    "RENDER_BOUNDSFLAG",
    "RENDER_CHANNELS",
    "RENDER_SRATE",
    "RENDER_STARTPOS",
    "RENDER_ENDPOS",
    "RENDER_TAILFLAG",
    "RENDER_TAILMS",
    "RENDER_ADDTOPROJ",
    "RENDER_DITHER",
    "RENDER_NORMALIZE",
    "RENDER_NORMALIZE_TARGET",
    "RENDER_BRICKWALL",
    "RENDER_FADEIN",
    "RENDER_FADEOUT",
    "RENDER_FADEINSHAPE",
    "RENDER_FADEOUTSHAPE",
    "RENDER_FADELPF",
    "RENDER_PADSTART",
    "RENDER_PADEND",
    "RENDER_TRIMSTART",
    "RENDER_TRIMEND",
    "RENDER_DELAY",
];
const RENDER_TEXTS: &[&str] = &["RENDER_FILE", "RENDER_PATTERN", "RENDER_FORMAT", "RENDER_FORMAT2", "RENDER_EXTRAFILEDIR"];

fn finite(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}

fn render_state_complete(daw: Option<DawId>, state: &Value) -> bool {
    let unique_refs = |rows: &[Value]| {
        let mut seen = std::collections::HashSet::with_capacity(rows.len());
        rows.iter()
            .all(|row| row.get("ref").and_then(Value::as_str).is_some_and(|reference| !reference.is_empty() && seen.insert(reference)))
    };
    match daw {
        Some(DawId::Reaper) => {
            let (Some(selection), Some(mutes), Some(time)) =
                (state["selection"].as_array(), state["mutes"].as_array(), state["time_selection"].as_array())
            else {
                return false;
            };
            state["numbers"].as_object().is_some_and(|numbers| {
                numbers.len() == RENDER_NUMBERS.len() && RENDER_NUMBERS.iter().all(|key| numbers.get(*key).is_some_and(finite))
            }) && state["texts"].as_object().is_some_and(|texts| {
                texts.len() == RENDER_TEXTS.len() && RENDER_TEXTS.iter().all(|key| texts.get(*key).is_some_and(Value::is_string))
            }) && state["projrenderlimit"].as_f64().is_some_and(|speed| {
                speed.is_finite() && speed.fract() == 0. && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&speed)
            }) && state["dirty"].is_boolean()
                && finite(&state["master_fx"])
                && time.len() == 2
                && time.iter().all(finite)
                && !selection.is_empty()
                && selection.len() == mutes.len()
                && unique_refs(selection)
                && selection.iter().zip(mutes).all(|(a, b)| a["ref"] == b["ref"] && finite(&a["value"]) && finite(&b["value"]))
        }
        Some(DawId::Renoise) => {
            state["bpm"].as_f64().is_some_and(|bpm| bpm.is_finite() && bpm > 0.)
                && state["lpb"].as_f64().is_some_and(|lpb| lpb.is_finite() && (1.0..=256.0).contains(&lpb) && lpb.fract() == 0.)
                && state["tracks"].as_array().is_some_and(|tracks| {
                    unique_refs(tracks)
                        && tracks
                            .iter()
                            .all(|track| track["mute"].as_u64().is_some_and(|mute| (1..=3).contains(&mute)) && track["solo"].is_boolean())
                })
        }
        _ => false,
    }
}

fn focus_state(mut focus: Value) -> Result<Value, RuntimeError> {
    let transport = &focus["transport"];
    if !focus["selection"].is_object()
        || !finite(&transport["beat"])
        || !transport["loop"]["on"].is_boolean()
        || !finite(&transport["loop"]["start"])
        || !finite(&transport["loop"]["end"])
    {
        return Err(hear_refused("unsupported", "The bridge did not expose a complete transport and selection snapshot."));
    }
    focus.as_object_mut().expect("validated focus snapshot").retain(|key, _| matches!(key.as_str(), "epoch" | "transport" | "selection"));
    Ok(focus)
}

fn render_deadline_seconds(seconds: f64) -> u64 {
    (seconds * 4.).max(120.).ceil() as u64 + 120
}

fn exact_render_seconds(read: &Value, approval: Option<HearApproval>) -> Result<f64, RuntimeError> {
    if read["schema"].as_u64() != Some(1)
        || !read["aligned"].is_boolean()
        || !read["irregular"].is_boolean()
        || !read["clipped"].is_boolean()
        || !finite(&read["offset"])
    {
        return Err(hear_refused("unsupported", "The bridge did not return render preflight schema 1."));
    }
    if read["aligned"] != true || read["irregular"] != false || read["clipped"] != false || read["offset"].as_f64() != Some(0.) {
        return Err(hear_refused(
            "window_unrenderable",
            "This interval cannot be rendered exactly without extra, clipped or irregular material.",
        ));
    }
    let seconds = read["seconds"]
        .as_f64()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.)
        .ok_or_else(|| hear_refused("unsupported", "The bridge did not provide a finite positive exact render duration."))?;
    if approval.is_some_and(|approved| !approved.covers(seconds, render_deadline_seconds(seconds))) {
        return Err(hear_refused("stale", "The current window exceeds its approved duration or deadline; preview again before listening."));
    }
    if seconds > 60. {
        return Err(hear_refused("window_too_long", "The requested interval exceeds 60 seconds on the current tempo map."));
    }
    Ok(seconds)
}

fn restored_state(daw: Option<DawId>, before: &Value, after: &Value) -> Result<bool, RuntimeError> {
    if !render_state_complete(daw, before) || !render_state_complete(daw, after) {
        return Err(HearFailure::Unsure {
            code: None,
            reason: "The render's touched state could not be read completely after cleanup.".into(),
            restored: false,
            marked_modified: false,
        }
        .into());
    }
    let expected = before.as_object().expect("validated render state");
    let observed = after.as_object().expect("validated render state");
    let marked_modified = daw == Some(DawId::Reaper) && before["dirty"] == false && after["dirty"] == true;
    if expected.len() != observed.len()
        || expected.iter().any(|(key, value)| !(daw == Some(DawId::Reaper) && key == "dirty") && observed.get(key) != Some(value))
    {
        return Err(HearFailure::Unsure {
            code: None,
            reason: "The state read after rendering did not match the state before it. Observe again before another action.".into(),
            restored: false,
            marked_modified: false,
        }
        .into());
    }
    Ok(marked_modified)
}

/// What Kumi knows of one DAW's listening: what its sources hold, the master's devices that don't make masking unfair,
/// its gain stage, its equaliser, and the change kinds the listening tools use.
pub struct Table {
    /// A track renders on its own after its fader and through the master's devices (Renoise's solo), not before its
    /// fader as Live's Ears hears it: its fader and the master's pure gains are in it already.
    pub post_fader: bool,
    /// Why the mix as it comes into the master's chain can't be heard, when it can't.
    pub no_before_chain: Option<&'static str>,
    /// The master's fader is in the master's render (REAPER's master mix), so masking adds it to the focus.
    pub master_fader: bool,
    /// The master track's ref when it's fixed (REAPER's); None: the track row whose type is "master".
    pub master_ref: Option<&'static str>,
    /// Devices that change nothing heard (analyzers), by a word in their name.
    pub analyzers: &'static [&'static str],
    /// Devices that only turn the level, by a word in their name, with the knob that turns it.
    pub gains: &'static [(&'static str, &'static str)],
    /// Devices that act on one track as they do on the mix (equalisers, filters, gains): with a track rendered through the
    /// master, only the others make masking unfair.
    pub linear: &'static [&'static str],
    /// Limiters (a word in their name, the knob that turns the level into them, whether lowering it makes it louder).
    pub limiters: &'static [(&'static str, &'static str, bool)],
    /// The gain device rebalancing adds when there's no gain stage: what the add takes, and its name once there.
    pub gain_device: (&'static str, &'static str),
    /// The limiter a goal's candidate gets last in its chain for safety: what the add takes, and its name once there.
    pub safety_limiter: (&'static str, &'static str),
    /// The read kind of a track's devices ("fx", "device").
    pub device_kind: &'static str,
    /// The change kinds: a parameter, several of one device's parameters, a device added, a device deleted.
    pub param_tool: &'static str,
    pub params_tool: &'static str,
    pub add_tool: &'static str,
    pub delete_tool: &'static str,
    /// The add's field for what to add ("name", "path"), and a parameter's fields for a raw value and for its text.
    pub add_field: &'static str,
    pub raw_field: &'static str,
    pub text_field: &'static str,
    /// Parameters' raw values are 0 to 1 (REAPER's normalized), whatever range the row names.
    pub normalized: bool,
    /// A track's own items render before its FX (REAPER's audio accessor); why not, when they can't.
    pub no_items: Option<&'static str>,
    /// What a scratch copy's values are set by in `scratch set`.
    pub scratch_field: &'static str,
    /// The equaliser fit draws on, by a word in its name, and whether its first and last bands are shelves.
    pub eq: &'static str,
    pub eq_shelves_at_ends: bool,
    /// Whether the bridge can hear what the DAW plays (REAPER's audio hook; Renoise's Lua can't).
    pub captures: bool,
    /// The name prefixes of a plug-in's device where the rows don't say (REAPER's "VST3: …").
    pub plugin_prefixes: &'static [&'static str],
    /// How a render's notes start that say it stopped what was playing: of Kumi's own play, said once a run.
    pub stop_notes: &'static [&'static str],
    /// Parameters the DAW lists last on every device that aren't the sound's (REAPER's Bypass, Wet and Delta): a search
    /// never moves them.
    pub pseudo_params: &'static [&'static str],
    /// Whether a device's parameter aims it somewhere rather than shaping its sound (Renoise's meta devices' Dest. Track,
    /// Effect and Parameter, a #Send's Receiver), by the device's own name and the parameter's: a search never moves it.
    pub aims: fn(&str, &str) -> bool,
}

/// What the bridge can do for listening now, as its `status` says (`listening`), and whether it's the Renoise demo.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Flags {
    /// The bridge says what it can do (a bridge older than M5 says nothing).
    pub known: bool,
    pub render: bool,
    pub capture: bool,
    pub audition: bool,
    pub grid: bool,
    pub scratch: bool,
    pub demo: bool,
}

impl Flags {
    /// From a `status` answer.
    pub fn of(status: &Value) -> Flags {
        let listening = status.get("listening");
        let flag = |key: &str| listening.and_then(|flags| flags.get(key)) == Some(&Value::Bool(true));
        Flags {
            known: listening.is_some_and(Value::is_object),
            render: flag("render"),
            capture: flag("capture"),
            audition: flag("audition"),
            grid: flag("grid"),
            scratch: flag("scratch"),
            demo: status.get("demo") == Some(&Value::Bool(true)),
        }
    }
}

/// A track as a listen finds it: its ref, name and type, and its fader.
#[derive(Debug, Clone)]
pub struct TrackRow {
    pub reference: String,
    pub name: String,
    kind: String,
    volume_db: Option<f64>,
    /// Muted (REAPER), or not active (Renoise's MUTED and OFF).
    muted: bool,
}

/// One of a track's devices as listening reads it.
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub reference: String,
    /// What the producer calls it: for display only.
    pub name: String,
    /// Its own name, whatever the producer named it: REAPER's FX as the plug-in is called (`originalName` once renamed),
    /// Renoise's device by its path's last part ("Audio/Effects/Native/Maximizer"). The tables' words are matched to it.
    pub own_name: String,
    /// Who it is in every session (`identity_of`), for what Kumi remembers of a search on it.
    pub identity: String,
    pub on: bool,
    pub plugin: bool,
    /// An instrument (REAPER's VSTi and the like): the track's sound starts there.
    pub instrument: bool,
    pub index: u64,
    /// The track's own mixer (Renoise's first device: its fader and pan, not something on the track).
    pub mixer: bool,
}

/// Only host identity, never the producer's display name, establishes a VST3 limiter.
pub(crate) fn verified_plugin_limiter(device: &DeviceRow) -> bool {
    if !device.on || !device.plugin || device.instrument {
        return false;
    }
    if let Some(name) = device.identity.strip_prefix("VST3: ") {
        return kumi_library::plugins::inventory::is_limiter_name(&plugin_name(name));
    }
    device.identity.rsplit_once("/VST3/").is_some_and(|(_, id)| kumi_library::plugins::inventory::is_limiter_class_id(id))
}

/// What a scratch set keeps to set and drop its copies.
struct Scratch {
    id: String,
    /// The copied device's place in its track's chain, and its knobs' places by name.
    device: u64,
    knobs: IndexMap<String, u64>,
    /// The steps setting the copies (and dropping them) left in the DAW's undo (Renoise's: one a set), told when they go.
    steps: Cell<u64>,
}

/// The DAW as the listening tools meet it, through Kumi's host.
pub struct OwnRenderer<S: RefScheme + 'static> {
    pub link: Rc<Link<S>>,
    pub plan: Rc<OwnPlan<S>>,
    pub table: &'static Table,
    on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    change_timeout_ms: u64,
    folder: PathBuf,
    flag: ListenFlag,
    flags: Cell<Flags>,
    beats_per_bar: Cell<f64>,
    /// Scratch copies standing now: each name's ref.
    pub copies: RefCell<IndexMap<String, String>>,
    tracks: RefCell<Option<(Instant, Vec<TrackRow>)>>,
    /// The parts groove read, by the name the model gave each.
    pub parts: RefCell<HashMap<String, Part>>,
    /// Kumi's listens played out loud (from, to, in ms): what the producer heard of Kumi's work.
    pub played: RefCell<std::collections::VecDeque<(i64, i64)>>,
    /// The changes that kept the last goal's best on a track of its own (its keep, then the mute a paused goal put on it):
    /// taken back, the last first, when a better one replaces it, by this goal or a resumed one.
    pub goal_best: RefCell<Vec<String>>,
    /// The track that best is on (its short ref, which the book follows): muted again when its mute was taken back and its
    /// keep's undo then refused.
    pub goal_best_track: RefCell<Option<String>>,
    /// A render stopping Kumi's own play was said in this run (a new answer's run says it again).
    pub stop_told: Cell<bool>,
    /// The last render's notes, told as it ended: the engine passing them on doesn't tell them again.
    told_notes: RefCell<Vec<String>>,
    /// The tempo renders played into the transport (Renoise's play the song's tempo commands), still to be set back: the
    /// producer's and the transport's now. Set back once, before the DAW plays the song out loud or as the call or the
    /// goal ends: each setting is a step of the DAW's undo.
    tempo_back: RefCell<Option<TempoBack>>,
}

/// The producer's tempo (BPM, LPB) and the transport's as the last render left it.
#[derive(Clone, Copy)]
struct TempoBack {
    bpm: f64,
    lpb: u64,
    now_bpm: f64,
    now_lpb: u64,
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// What the bridge noted with an answer (`notes`), in order, each a sentence (REAPER's come without their full stop).
pub fn notes_of(value: &Value) -> Vec<String> {
    let sentence = |note: &str| {
        let note = note.trim();
        if note.ends_with(['.', '!', '?']) {
            note.to_owned()
        } else {
            format!("{note}.")
        }
    };
    value.get("notes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(sentence).collect()
}

/// How far a listening job is, in the producer's words: a render's, or a capture of what the DAW plays.
fn progress_words(app: &str, kind: Option<&str>, percent: i64) -> String {
    match kind {
        Some("capture") => format!("Hearing what {app} plays · {percent}%"),
        _ => format!("Rendering in {app} · {percent}%"),
    }
}

/// A reason with what the bridge noted after it, in parentheses.
fn with_notes(said: String, notes: &[String]) -> String {
    if notes.is_empty() {
        return said;
    }
    format!("{said} ({})", notes.iter().map(|note| note.trim_end_matches('.')).collect::<Vec<_>>().join("; "))
}

/// How many values a stepped parameter has: the texts of its steps (a grid read's `items`), or its own count (`steps`).
pub fn steps_of(row: &Value) -> Option<u64> {
    let items = row.get("items").and_then(Value::as_array).map_or(0, Vec::len) as u64;
    row.get("steps").and_then(Value::as_u64).or((items >= 2).then_some(items)).filter(|steps| *steps >= 2)
}

/// The stretches of a capture the playhead played straight through, from its marks (`[frame, seconds, beats, playing]` at
/// the first frame and at each jump: a loop, a seek, a stop): each one's start and length in the file, in seconds, and
/// what it was ("The mix from bar 5", a pass number when the loop played it again). Stretches while stopped, and those
/// under half a second, are left out.
fn stretches(marks: Option<&Value>, rate: f64, held: Option<f64>, beats_per_bar: f64) -> Vec<(f64, f64, String)> {
    let marks: Vec<(f64, Option<f64>, bool)> = marks
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mark| {
            Some((mark.get(0)?.as_f64()?, mark.get(2).and_then(Value::as_f64), mark.get(3).and_then(Value::as_bool).unwrap_or(true)))
        })
        .collect();
    let end = held.map(|seconds| seconds * rate);
    let mut found: Vec<(f64, f64, String)> = vec![];
    let mut seen: IndexMap<String, usize> = IndexMap::new();
    for (at, (frame, beat, playing)) in marks.iter().enumerate() {
        let Some(next) = marks.get(at + 1).map(|(frame, ..)| *frame).or(end) else { continue };
        let seconds = (next - frame) / rate;
        if !playing || seconds < 0.5 {
            continue;
        }
        let base = match beat {
            Some(beat) => format!("The mix from bar {}", (beat / beats_per_bar.max(1.)).floor() as i64 + 1),
            None => "The mix".to_string(),
        };
        let pass = seen.entry(base.clone()).or_insert(0);
        *pass += 1;
        let label = if *pass == 1 { base } else { format!("{base} (pass {pass})") };
        found.push((frame / rate, seconds, label));
    }
    found
}

impl<S: RefScheme + 'static> OwnRenderer<S> {
    pub fn new(link: Rc<Link<S>>, plan: Rc<OwnPlan<S>>, context: &IntegrationContext, kumi_home: &std::path::Path) -> Self {
        let profile = link.daw.profile;
        let folder = kumi_home.join("bridges").join(profile.id.map_or("daw", |id| id.as_str())).join("listening");
        OwnRenderer {
            table: link.daw.listening,
            link,
            plan,
            on_action: context.on_action.clone(),
            change_timeout_ms: context.change_timeout_ms.unwrap_or(30_000),
            folder,
            flag: ListenFlag::default(),
            flags: Cell::new(Flags::default()),
            beats_per_bar: Cell::new(4.),
            copies: RefCell::default(),
            tracks: RefCell::default(),
            parts: RefCell::default(),
            played: RefCell::default(),
            goal_best: RefCell::default(),
            goal_best_track: RefCell::default(),
            stop_told: Cell::new(false),
            told_notes: RefCell::default(),
            tempo_back: RefCell::default(),
        }
    }

    pub fn profile(&self) -> &'static DawProfile {
        self.link.daw.profile
    }

    pub fn app(&self) -> &'static str {
        self.profile().words.app
    }

    pub fn set_flags(&self, flags: Flags) {
        self.flags.set(flags);
    }

    pub fn flags(&self) -> Flags {
        self.flags.get()
    }

    pub fn set_beats_per_bar(&self, beats: f64) {
        if beats.is_finite() && beats > 0. {
            self.beats_per_bar.set(beats);
        }
    }

    pub fn cleanup_signal(&self) -> Signal {
        abort::any([self.link.lifetime.clone(), abort::timeout(self.change_timeout_ms.saturating_mul(3))])
    }

    pub fn tell_app(&self, title: &str) {
        if let Some(on_action) = &self.on_action {
            on_action(ActionEvent { title: title.into(), playing: None, recording: None });
        }
    }

    /// Why nothing can be heard now, in words: the DAW isn't there, its bridge is older than listening, or it's the demo.
    /// (M7) Kumi is listening to something else now (a judged run, a listen): another check waits its turn.
    /// Kumi's listening folder for this bridge (one the DAW reads).
    pub fn folder(&self) -> &std::path::Path {
        &self.folder
    }

    /// (#401) A track rendered alone over `window` at `stage` ("items": REAPER's items before the track's FX; "pre-fader"):
    /// its file, or why not.
    pub async fn render_track(&self, long: &str, stage: &str, window: Window, signal: &Signal) -> Result<Render, String> {
        if self.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        if let Some(why) = self.why_not() {
            return Err(why);
        }
        let sources = vec![json!({"id": "t", "what": "track", "ref": long, "stage": stage})];
        let _listening = self.begin_listen();
        let rendered = self.render_sources(sources, window, signal).await;
        self.put_tempo_back().await;
        match rendered {
            Ok(Ok((files, _))) => files.get("t").map(|(render, _)| render.clone()).ok_or_else(|| "the render made no file".to_string()),
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn listening_now(&self) -> bool {
        self.flag.is_on()
    }

    /// (M7) Kumi listens now, until the answer is dropped.
    pub fn begin_listen(&self) -> kumi_listening::renderer::ListenNow<'_> {
        self.flag.begin()
    }

    pub fn why_not(&self) -> Option<String> {
        let app = self.app();
        if !self.link.connected.get() || self.link.endpoint.borrow().is_none() {
            return Some(self.profile().vocab.cant_reach.into());
        }
        let flags = self.flags.get();
        if !flags.known {
            return Some(self.link.daw.bridge.cant_listen(app));
        }
        if flags.demo || !flags.render {
            return Some(if flags.demo {
                format!("The {app} demo can't render to a file, so Kumi can't hear the song: a licensed {app} can.")
            } else {
                format!("{app} can't render for Kumi just now.")
            });
        }
        None
    }

    /// A host tool's answer, with the request placed where its refs came from (their epoch, and Renoise's layout).
    pub async fn ask(&self, tool: &str, args: Value, signal: Signal) -> Result<Result<Value, Refused>, RuntimeError> {
        let mut args = args;
        let input = args.as_object().cloned().unwrap_or_default();
        self.plan.place_request(&mut args, &input);
        self.link.call(tool, args, signal).await
    }

    /// `ask`, of a request that reads a song's tempo commands (a render, a part's audio, scratch copies), asked again on
    /// `asked_on`'s signal while the DAW answers busy, until `signal` stops the wait. The status line says once what it's
    /// waiting for.
    async fn ask_while_busy(
        &self,
        tool: &str,
        args: Value,
        signal: &Signal,
        asked_on: impl Fn() -> Signal,
    ) -> Result<Result<Value, Refused>, RuntimeError> {
        let waiting = |why: &str| self.tell_app(&format!("{}: Kumi is waiting for it", why.trim_end_matches(": ask again in a moment")));
        while_busy(signal, waiting, || self.ask(tool, args.clone(), asked_on())).await
    }

    /// A read's arguments placed as the model's read places them: the layout Renoise's track refs were made in, and the
    /// epoch a lasting ref (REAPER's) was read in.
    pub fn read_placed(&self, mut args: Value) -> Value {
        let book = self.link.book.borrow();
        let object = args.as_object().cloned().unwrap_or_default();
        if let Some(layout) = self.link.layout.get().filter(|_| names_tracks(&book.scheme, &object)) {
            args["layout"] = layout.into();
        }
        if !book.scheme.positional() {
            let primary = object.get("ref").or(object.get("parent")).and_then(Value::as_str);
            if let Some(epoch) = primary.and_then(|reference| self.link.seen_in.borrow().get(reference).cloned()) {
                args["epoch"] = epoch.into();
            }
        }
        args
    }

    /// Every row of a read, following its pages (at most `pages`).
    pub async fn rows(&self, args: Value, pages: usize, signal: &Signal) -> Result<Result<Vec<Value>, String>, RuntimeError> {
        let mut args = self.read_placed(args);
        let mut rows = vec![];
        for _ in 0..pages {
            let page = match self.link.call("read", args.clone(), signal.clone()).await? {
                Ok(page) => page,
                Err(refused) => return Ok(Err(refused.why)),
            };
            rows.extend(page.get("items").and_then(Value::as_array).cloned().unwrap_or_default());
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(cursor) => args["cursor"] = cursor.into(),
                None => break,
            }
        }
        Ok(Ok(rows))
    }

    /// The tracks are read again next time (they just changed).
    pub fn forget_tracks(&self) {
        self.tracks.borrow_mut().take();
    }

    /// The project's tracks (read once a second at most).
    pub async fn track_rows(&self, signal: &Signal) -> Result<Result<Vec<TrackRow>, String>, RuntimeError> {
        if let Some((at, rows)) = self.tracks.borrow().as_ref() {
            if at.elapsed() < TRACKS_FRESH {
                return Ok(Ok(rows.clone()));
            }
        }
        let rows = match self.rows(json!({"kind": "track", "limit": 200}), TRACK_PAGES, signal).await? {
            Ok(rows) => rows,
            Err(why) => return Ok(Err(why)),
        };
        let rows: Vec<TrackRow> = rows
            .iter()
            .filter_map(|row| {
                Some(TrackRow {
                    reference: text(row, "ref")?,
                    name: text(row, "name").unwrap_or_default(),
                    kind: text(row, "type").unwrap_or_default(),
                    volume_db: row.get("volumeDb").and_then(Value::as_f64),
                    muted: match row.get("mute") {
                        Some(Value::Bool(muted)) => *muted,
                        Some(Value::String(state)) => state != "active",
                        _ => false,
                    },
                })
            })
            .collect();
        *self.tracks.borrow_mut() = Some((Instant::now(), rows.clone()));
        Ok(Ok(rows))
    }

    /// A track's fader (dB, -120 for -inf), by its long ref.
    async fn fader_at(&self, reference: &str, signal: &Signal) -> Option<f64> {
        let tracks = self.track_rows(signal).await.ok()?.ok()?;
        let row = tracks.iter().find(|row| self.same_place(&row.reference, reference))?;
        Some(row.volume_db.unwrap_or(-120.).max(-120.))
    }

    /// Whether two refs name the same thing: Renoise's positional refs by their place, whatever epoch each was read in (a
    /// device added moves the epoch and keeps the tracks); REAPER's as they are.
    pub fn same_place(&self, a: &str, b: &str) -> bool {
        if self.link.book.borrow().scheme.positional() {
            a.split_once(':').map(|(_, rest)| rest) == b.split_once(':').map(|(_, rest)| rest)
        } else {
            a == b
        }
    }

    /// A ref as a run follows it (a rename's): Renoise's without its epoch.
    pub fn place_key(&self, long: &str) -> String {
        match long.split_once(':') {
            Some((_, rest)) if self.link.book.borrow().scheme.positional() => rest.to_owned(),
            _ => long.to_owned(),
        }
    }

    /// Keeps a scratch copy by its name, its ref registered in the book so it follows what moves the tracks (Renoise's
    /// refs are places); its ref as the book names it.
    pub fn keep_copy(&self, name: String, long: &str) -> String {
        let short = self.link.book.borrow_mut().short_ref(long);
        self.copies.borrow_mut().insert(name, short.clone());
        short
    }

    /// A scratch copy's ref now, by its name.
    pub fn copy_ref(&self, name: &str) -> Option<String> {
        let short = self.copies.borrow().get(name).cloned()?;
        Some(self.link.book.borrow().lengthen(&json!({"ref": short}))["ref"].as_str().unwrap_or(&short).to_owned())
    }

    /// Says what scratch copies left in the DAW's undo, when they left anything.
    pub fn tell_scratch_steps(&self, steps: u64) {
        if steps > 0 {
            self.tell_app(&format!(
                "Kumi's scratch copies left {steps} step{} in {}'s undo",
                if steps == 1 { "" } else { "s" },
                self.app()
            ));
        }
    }

    /// Drops a make's copies: whether all went; what the bridge noted, or why some stayed, is told. A make the bridge
    /// doesn't know (dropped already, or forgotten after a crash) has nothing to drop.
    pub async fn drop_make(&self, scratch: &str, steps: u64) -> bool {
        let dropped = self.link.call("scratch", json!({"action": "drop", "scratchId": scratch}), self.cleanup_signal()).await;
        let (all, notes, more) = match &dropped {
            Ok(Ok(done)) => (
                done.get("left").and_then(Value::as_u64).unwrap_or(0) == 0,
                notes_of(done),
                done.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0),
            ),
            Ok(Err(refused)) if refused.code.as_deref() == Some("bad-params") => (true, vec![], 0),
            _ => (false, vec![], 0),
        };
        self.tell_scratch_steps(steps + more);
        for note in &notes {
            self.tell_app(note);
        }
        if !all && notes.is_empty() {
            self.tell_app(self.profile().vocab.render_leftovers);
        }
        all
    }

    /// A track by what the model or a round names it (a ref of this turn, a long ref, a scratch copy's or a track's
    /// name): its long ref and its name now.
    pub async fn resolve(&self, named: &str, signal: &Signal) -> Result<Result<(String, String), String>, RuntimeError> {
        if let Some(reference) = self.copy_ref(named) {
            return Ok(Ok((reference, named.to_owned())));
        }
        let long = self.link.book.borrow().lengthen(&json!({"ref": named}))["ref"].as_str().unwrap_or(named).to_owned();
        let tracks = match self.track_rows(signal).await? {
            Ok(tracks) => tracks,
            Err(why) => return Ok(Err(why)),
        };
        let place = self.profile().words.place;
        if self.link.book.borrow().scheme.kind_of(&long) == Some("track") {
            return Ok(match tracks.iter().find(|track| self.same_place(&track.reference, &long)) {
                Some(track) => Ok((long, track.name.clone())),
                None => Err(format!("{named} isn't a track in the {place} now: read again for its ref")),
            });
        }
        let found =
            tracks.iter().find(|track| track.name == named).or_else(|| tracks.iter().find(|track| track.name.eq_ignore_ascii_case(named)));
        Ok(match found {
            Some(track) => Ok((track.reference.clone(), track.name.clone())),
            None => Err(format!("“{named}” isn't a track in the {place}: give a track's ref or its name")),
        })
    }

    /// The run's track (the master without one): its long ref and name.
    async fn scope(&self, track: Option<&str>, signal: &Signal) -> Result<Result<(String, String), String>, RuntimeError> {
        if let Some(track) = track {
            return self.resolve(track, signal).await;
        }
        let master = self.profile().words.master;
        if let Some(reference) = self.table.master_ref {
            return Ok(Ok((reference.to_owned(), master.to_owned())));
        }
        let tracks = match self.track_rows(signal).await? {
            Ok(tracks) => tracks,
            Err(why) => return Ok(Err(why)),
        };
        Ok(tracks
            .iter()
            .find(|track| track.kind == "master")
            .map(|track| (track.reference.clone(), master.to_owned()))
            .ok_or_else(|| format!("Kumi couldn't find {master}")))
    }

    /// A track's devices, in order.
    pub async fn devices(&self, track: &str, signal: &Signal) -> Result<Result<Vec<DeviceRow>, String>, RuntimeError> {
        let rows = match self.rows(json!({"kind": self.table.device_kind, "parent": track}), 8, signal).await? {
            Ok(rows) => rows,
            Err(why) => return Ok(Err(why)),
        };
        Ok(Ok(rows
            .iter()
            .enumerate()
            // (REAPER's input FX, and the master's monitoring FX, are in no render)
            .filter(|(_, row)| row.get("input") != Some(&Value::Bool(true)))
            .filter_map(|(at, row)| {
                Some(DeviceRow {
                    mixer: text(row, "path").is_some_and(|path| path.ends_with("TrackVolPan"))
                        || matches!(text(row, "name").as_deref(), Some("TrackVolPan" | "Track Vol/Pan" | "Mixer")),
                    reference: text(row, "ref")?,
                    name: text(row, "name").unwrap_or_default(),
                    own_name: text(row, "originalName")
                        .or_else(|| text(row, "path").and_then(|path| path.rsplit('/').next().map(str::to_owned)))
                        .or_else(|| text(row, "name"))
                        .unwrap_or_default(),
                    identity: identity_of(row).unwrap_or_default(),
                    // REAPER says enabled and offline; Renoise says active.
                    on: row.get("enabled").or(row.get("active")).and_then(Value::as_bool).unwrap_or(true)
                        && row.get("offline") != Some(&Value::Bool(true)),
                    plugin: row.get("plugin").and_then(Value::as_bool).unwrap_or_else(|| {
                        let name = text(row, "name").unwrap_or_default();
                        self.table.plugin_prefixes.iter().any(|prefix| name.starts_with(prefix))
                    }),
                    instrument: row.get("instrument") == Some(&Value::Bool(true)),
                    index: row.get("index").and_then(Value::as_u64).unwrap_or(at as u64),
                })
            })
            .collect()))
    }

    /// A device's parameters (with their text across the range when `grid`).
    pub async fn parameters(&self, device: &str, grid: bool, signal: &Signal) -> Result<Result<Vec<Value>, String>, RuntimeError> {
        let mut args = json!({"kind": "param", "parent": device});
        if grid {
            args["grid"] = GRID.into();
        }
        self.rows(args, 16, signal).await
    }

    /// Follows a job to its end: its last status, or why it didn't finish. Stopping the call stops the job, and what it
    /// changed is put back by the bridge.
    pub async fn wait(&self, started: &Value, signal: &Signal) -> Result<Result<Value, String>, RuntimeError> {
        let Some(id) = started.get("jobId").and_then(Value::as_str).map(str::to_owned) else {
            return Ok(Err(format!("{} started no job", self.app())));
        };
        let began = Instant::now();
        let mut told = None;
        loop {
            if began.elapsed() > JOB_LIMIT {
                self.cancel_job(&id).await;
                return Ok(Err(format!("{} took longer than {} minutes to render; Kumi stopped it", self.app(), JOB_LIMIT.as_secs() / 60)));
            }
            let asked = tokio::select! {
                biased;
                _ = signal.cancelled() => {
                    self.cancel_job(&id).await;
                    return Err(RuntimeError::Aborted);
                }
                asked = self.link.call("listen_job", json!({"jobId": id, "waitMs": 1000}), self.cleanup_signal()) => asked,
            };
            let status = match asked {
                Ok(Ok(status)) => status,
                // (the job may run on: it's stopped, so the next listen doesn't find the DAW busy with it)
                Ok(Err(refused)) => {
                    self.cancel_job(&id).await;
                    return Ok(Err(refused.why));
                }
                Err(_) if signal.is_cancelled() => {
                    self.cancel_job(&id).await;
                    return Err(RuntimeError::Aborted);
                }
                Err(error) => return Err(error),
            };
            match status.get("state").and_then(Value::as_str) {
                Some("done") => return Ok(Ok(status)),
                Some("failed") => {
                    let why = text(&status, "error").unwrap_or_else(|| format!("{} couldn't render it", self.app()));
                    return Ok(Err(with_notes(why, &notes_of(&status))));
                }
                Some("cancelled") => return Ok(Err(with_notes(format!("{} stopped the render", self.app()), &notes_of(&status)))),
                _ => {
                    // A long render says how far it is (a short one is over before it would).
                    let progress = status.get("progress").and_then(Value::as_f64).unwrap_or(0.);
                    let percent = (progress * 100.).round() as i64;
                    if began.elapsed() > Duration::from_secs(3) && told != Some(percent) {
                        told = Some(percent);
                        self.tell_app(&progress_words(self.app(), status.get("kind").and_then(Value::as_str), percent));
                    }
                }
            }
        }
    }

    /// Stops a job, and waits (in the clean-up's own time) until the bridge has put back what it changed: a render in
    /// flight runs to its end in Renoise, and REAPER may take a moment to stop. The next listening call waits for this.
    async fn cancel_job(&self, id: &str) {
        let cleanup = self.cleanup_signal();
        let ended = |answer: &Value| matches!(answer.get("state").and_then(Value::as_str), Some("done" | "failed" | "cancelled"));
        match self.link.call("listen_cancel", json!({"jobId": id}), cleanup.clone()).await {
            Ok(Ok(answer)) if !ended(&answer) => {}
            _ => return,
        }
        self.tell_app(&format!("{} is finishing the render it was asked to stop", self.app()));
        while !cleanup.is_cancelled() {
            match self.link.call("listen_job", json!({"jobId": id, "waitMs": 1000}), cleanup.clone()).await {
                Ok(Ok(status)) if !ended(&status) => continue,
                _ => return,
            }
        }
    }

    /// Renders these sources over `window` (with a bar before it so effects are warm): the job's files by source id, with
    /// its notes.
    pub async fn render_sources(
        &self,
        sources: Vec<Value>,
        window: Window,
        signal: &Signal,
    ) -> Result<Result<(IndexMap<String, (Render, Value)>, Vec<String>), String>, RuntimeError> {
        let pre_roll = self.beats_per_bar.get().min(window.from).max(0.);
        let args = json!({"sources": sources, "range": {"fromBeat": window.from, "beats": window.beats}, "preRollBeats": pre_roll});
        let started = match self.ask_while_busy("render", args, signal, || signal.clone()).await? {
            Ok(started) => started,
            Err(refused) => return Ok(Err(refused.why)),
        };
        let own_playing = self.own_playing();
        let done = match self.wait(&started, signal).await? {
            Ok(done) => done,
            Err(why) => return Ok(Err(why)),
        };
        let notes = self.finished(&done, own_playing);
        let mut files = IndexMap::new();
        for file in done.get("files").and_then(Value::as_array).into_iter().flatten() {
            let (Some(id), Some(path)) = (text(file, "id"), text(file, "file")) else { continue };
            let start = file.get("start").and_then(Value::as_f64).unwrap_or(0.);
            let seconds = file.get("seconds").and_then(Value::as_f64).map(|seconds| (seconds - start).max(0.));
            files.insert(id, (Render { file: path, start, seconds }, file.clone()));
        }
        Ok(Ok((files, notes)))
    }

    /// Kumi's own listen still playing out loud: a render stops it, which a run says once (each listen of a judged run or a
    /// goal stops the last one). (A second's slack: the DAW started it a moment before Kumi heard so.)
    fn own_playing(&self) -> bool {
        self.played.borrow().back().is_some_and(|(_, to)| *to + 1000 > chrono::Utc::now().timestamp_millis())
    }

    /// A finished render job (a render, a part's audio rendered, a track's items): its notes, told as it ends (a stop of
    /// Kumi's own play said once a run), and the tempo it played into the transport, kept to set back once.
    fn finished(&self, done: &Value, own_playing: bool) -> Vec<String> {
        let mut notes = notes_of(done);
        if own_playing {
            let stops = |note: &String| self.table.stop_notes.iter().any(|start| note.starts_with(start));
            if self.stop_told.get() {
                notes.retain(|note| !stops(note));
            } else if notes.iter().any(stops) {
                self.stop_told.set(true);
            }
        }
        // A render that played the song's tempo commands into its transport (Renoise's does): the tempo goes back to what
        // the producer had, once for the call or the goal (`put_tempo_back`).
        if let (Some(bpm), Some(lpb)) =
            (done.pointer("/tempo/before/bpm").and_then(Value::as_f64), done.pointer("/tempo/before/lpb").and_then(Value::as_u64))
        {
            let now_bpm = done.pointer("/tempo/bpm").and_then(Value::as_f64).unwrap_or(bpm);
            let now_lpb = done.pointer("/tempo/lpb").and_then(Value::as_u64).unwrap_or(lpb);
            let mut back = self.tempo_back.borrow_mut();
            match back.as_mut() {
                // (the transport as the last render left it: the producer's tempo is still the one to set back)
                Some(kept) if (kept.now_bpm - bpm).abs() <= 1e-6 && kept.now_lpb == lpb => {
                    (kept.now_bpm, kept.now_lpb) = (now_bpm, now_lpb);
                }
                // (the producer set a tempo since, or the first render to move it)
                _ => *back = Some(TempoBack { bpm, lpb, now_bpm, now_lpb }),
            }
        }
        // (told as well: not every tool's reply carries a render's notes)
        for note in &notes {
            self.tell_app(note);
        }
        *self.told_notes.borrow_mut() = notes.clone();
        notes
    }

    /// Sets back the tempo renders played into the transport, as one setting of each that moved (quietly: Kumi's undo
    /// never takes it back): what to tell, with the steps it left in the DAW's undo. Nothing when no render moved it.
    pub async fn put_tempo_back(&self) -> Option<String> {
        let back = self.tempo_back.borrow_mut().take()?;
        let mut done = vec![];
        if (back.now_bpm - back.bpm).abs() > 1e-6 {
            done.push(
                self.plan.history.quietly(None, self.change("song_bpm", json!({"bpm": back.bpm}), self.cleanup_signal())).await.is_ok(),
            );
        }
        if back.now_lpb != back.lpb {
            done.push(
                self.plan.history.quietly(None, self.change("song_lpb", json!({"lpb": back.lpb}), self.cleanup_signal())).await.is_ok(),
            );
        }
        if done.is_empty() || !done.iter().all(|done| *done) {
            return None;
        }
        let note = format!(
            "Kumi set the tempo back to {} BPM, LPB {} ({} step{} in {}'s undo).",
            kumi_common::js::number::to_string((back.bpm * 100.).round() / 100.),
            back.lpb,
            done.len(),
            if done.len() == 1 { "" } else { "s" },
            self.app()
        );
        self.tell_app(&note);
        Some(note)
    }

    /// Kumi's play out loud stops if `signal` (a listening call's, the answer's) is cancelled before it ends: Esc stops
    /// what Kumi plays as it stops the answer, whether the call is still at work or has answered.
    pub fn stop_play_on(self: &Rc<Self>, signal: &Signal) {
        let now = chrono::Utc::now().timestamp_millis();
        let Some(until) = self.played.borrow().back().map(|(_, to)| *to).filter(|to| *to > now) else { return };
        let (renderer, signal) = (Rc::downgrade(self), signal.clone());
        tokio::task::spawn_local(async move {
            tokio::select! {
                biased;
                _ = signal.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis((until - now) as u64)) => return,
            }
            // (the same play, still playing: a later one is its own call's)
            let Some(renderer) = renderer.upgrade() else { return };
            let playing =
                renderer.played.borrow().back().is_some_and(|(_, to)| *to == until && until > chrono::Utc::now().timestamp_millis());
            if playing {
                let _ = renderer.link.call("audition_stop", json!({}), renderer.cleanup_signal()).await;
            }
        });
    }

    /// Plays a render out loud where the producer listens (its part, at most `PLAYED_SECONDS`), without waiting: whether
    /// it's playing, and what the DAW says of how (Renoise plays the song over that part), or why it isn't. A track's
    /// render from before its fader (`track`, its ref) plays at its fader's level, as Kumi measured it, not louder.
    pub async fn play(&self, render: &Render, track: Option<&str>) -> (bool, Option<String>) {
        // (the song plays at the producer's tempo)
        self.put_tempo_back().await;
        // (tried whatever the flags said last: REAPER opens its audio device for a preview itself, and says why not)
        let seconds = render.seconds.unwrap_or(PLAYED_SECONDS).min(PLAYED_SECONDS);
        let mut args = json!({"file": render.file, "fromSeconds": render.start, "seconds": seconds});
        if let Some(track) = track.filter(|_| !self.table.post_fader) {
            let fader = self.fader_at(track, &self.cleanup_signal()).await.unwrap_or(0.);
            if fader != 0. {
                args["gainDb"] = json!(fader.clamp(-60., 12.));
            }
        }
        match self.link.call("audition", args, self.cleanup_signal()).await {
            Ok(Ok(answer)) => {
                let playing = answer.get("playing") == Some(&Value::Bool(true));
                if playing {
                    // (as long as the DAW says it plays it)
                    let seconds = answer.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(seconds);
                    let now = chrono::Utc::now().timestamp_millis();
                    let mut played = self.played.borrow_mut();
                    played.push_back((now, now + (seconds * 1000.) as i64));
                    if played.len() > 128 {
                        played.pop_front();
                    }
                }
                (playing, text(&answer, "note"))
            }
            Ok(Err(refused)) => (false, Some(refused.why)),
            Err(_) => (false, None),
        }
    }

    /// The bars a window covers, as the producer reads them.
    fn bars(&self, window: Window) -> String {
        bars_of(self.beats_per_bar.get(), window)
    }

    /// Hears these candidates over `window` in one render: each by the name it goes by now, the mix by MIX_CANDIDATE.
    /// The first that isn't a scratch copy is played out loud after.
    async fn hear_candidates(&self, candidates: &[AuditionCandidate], window: Window, before_main: bool, signal: &Signal) -> Pass {
        self.hear_candidates_aloud(candidates, window, before_main, true, signal).await
    }

    /// The same, the first source played out loud only when `aloud` (a round heard in several renders plays one).
    async fn hear_candidates_aloud(
        &self,
        candidates: &[AuditionCandidate],
        window: Window,
        before_main: bool,
        aloud: bool,
        signal: &Signal,
    ) -> Pass {
        let mut sources = vec![];
        // Each source's id, the name its file goes by, and its track (None for the mix).
        let mut named: Vec<(String, String, Option<String>)> = vec![];
        for (at, candidate) in candidates.iter().enumerate() {
            if candidate.mix == Some(true) || candidate.track == MIX_CANDIDATE {
                if !named.iter().any(|(_, _, track)| track.is_none()) {
                    sources.push(json!({"id": "mix", "what": "master"}));
                    named.push(("mix".into(), MIX_CANDIDATE.into(), None));
                }
                continue;
            }
            match self.resolve(&candidate.track, signal).await {
                Ok(Ok((reference, name))) => {
                    let id = format!("t{at}");
                    sources.push(json!({"id": id, "what": "track", "ref": reference, "stage": "pre-fader"}));
                    named.push((id, name, Some(reference)));
                }
                Ok(Err(why)) => return pass(Err(RuntimeError::Observation(why)), vec![], None, false),
                Err(error) => return pass(Err(error), vec![], None, false),
            }
        }
        let mut tried = None;
        if before_main {
            match self.table.no_before_chain {
                None => sources.push(json!({"id": "premix", "what": "master", "beforeChain": true})),
                Some(why) => tried = Some(why.to_owned()),
            }
        }
        let (files, notes) = match self.render_sources(sources, window, signal).await {
            Ok(Ok(rendered)) => rendered,
            Ok(Err(why)) => return pass(Err(RuntimeError::plain(why)), vec![], tried, false),
            Err(error) => return pass(Err(error), vec![], tried, false),
        };
        let mut captured = IndexMap::new();
        for (id, name, _) in &named {
            if let Some((render, _)) = files.get(id) {
                captured.insert(name.clone(), render.clone());
            }
        }
        let before_master = files.get("premix").map(|(render, _)| render.clone());
        let mut notes = notes;
        // What the producer hears: the first thing heard that isn't a copy only Kumi compares.
        let first = named.iter().filter(|_| aloud).find(|(_, name, _)| !self.copies.borrow().contains_key(name));
        let audible = match first.and_then(|(_, name, track)| Some((captured.get(name)?, track.as_deref()))) {
            Some((render, track)) => {
                notes.extend(self.put_tempo_back().await);
                let (played, said) = self.play(render, track).await;
                if played {
                    self.tell_app(&format!("Playing {} out loud", self.bars(window)));
                    notes.extend(said);
                } else {
                    let why = said.map(|why| format!(" ({})", why.trim_end_matches('.'))).unwrap_or_default();
                    let unheard = format!("{} couldn't play it out loud{why}, so the producer didn't hear it.", self.app());
                    // (told as well: not every tool's reply carries a pass's notes)
                    self.tell_app(&unheard);
                    notes.push(unheard);
                }
                played
            }
            None => false,
        };
        let sources = named.iter().map(|(_, name, track)| (name.clone(), track.is_none())).collect();
        pass(Ok(Captured { files: captured, sources, before_master }), notes, tried, audible)
    }

    /// Each candidate's stretch: its own part's (an item, a pattern) from its start, else the first part's, else from_beat;
    /// all one length (beats, else the parts', at most `MAX_PART_BEATS`). Parts of different lengths are refused in words.
    pub async fn part_windows(
        &self,
        candidates: &[AuditionCandidate],
        from_beat: Option<f64>,
        beats: Option<f64>,
        signal: &Signal,
    ) -> Result<Result<Vec<Window>, String>, RuntimeError> {
        let given = Window { from: from_beat.unwrap_or(0.), beats: beats.unwrap_or(8.) };
        let mut parts: Vec<Option<Window>> = vec![];
        for candidate in candidates {
            match self.part_of(candidate, from_beat, signal).await? {
                Ok(part) => parts.push(part),
                Err(why) => return Ok(Err(why)),
            }
        }
        let lengths: Vec<f64> = parts.iter().flatten().map(|part| part.beats.min(MAX_PART_BEATS)).collect();
        if beats.is_none() && lengths.iter().any(|length| (length - lengths[0]).abs() > 1e-6) {
            let said: Vec<String> = candidates
                .iter()
                .zip(&parts)
                .filter_map(|(candidate, part)| {
                    let name = candidate.label.clone().unwrap_or_else(|| candidate.track.clone());
                    part.map(|part| format!("“{name}” {} beats", kumi_common::js::number::to_string(part.beats.min(MAX_PART_BEATS))))
                })
                .collect();
            return Ok(Err(format!(
                "The candidates' parts are different lengths ({}): give beats, and Kumi hears each that long from its own start.",
                said.join(", ")
            )));
        }
        let length = beats.or(lengths.first().copied()).unwrap_or(given.beats);
        let first = parts.iter().flatten().next().map(|part| part.from);
        Ok(Ok(parts
            .iter()
            .map(|part| Window { from: part.map(|part| part.from).or(first).unwrap_or(given.from), beats: length })
            .collect()))
    }

    /// Where a candidate's own part (an item, a pattern) plays: None when it names none or from_beat is given. "first" (a
    /// goal heard again names its tracks' parts so) is where the track first plays.
    pub async fn part_of(
        &self,
        candidate: &AuditionCandidate,
        from_beat: Option<f64>,
        signal: &Signal,
    ) -> Result<Result<Option<Window>, String>, RuntimeError> {
        let Some(part) = candidate.clip.as_deref().filter(|part| !part.is_empty() && from_beat.is_none()) else {
            return Ok(Ok(None));
        };
        if part != "first" {
            return Ok(self.window_of(part, signal).await?.map(Some));
        }
        let track = match self.resolve(&candidate.track, signal).await? {
            Ok((track, _)) => track,
            Err(why) => return Ok(Err(why)),
        };
        let rows = match self.rows(json!({"kind": "spans", "limit": 200}), 8, signal).await? {
            Ok(rows) => rows,
            Err(why) => return Ok(Err(why)),
        };
        let span = rows
            .iter()
            .find(|row| text(row, "ref").is_some_and(|reference| self.same_place(&reference, &track)))
            .and_then(|row| row.pointer("/spans/0").and_then(Value::as_array).cloned())
            .and_then(|span| Some((span.first()?.as_f64()?, span.get(1)?.as_f64()?)));
        Ok(Ok(span.filter(|(from, to)| to > from).map(|(from, to)| Window { from, beats: to - from })))
    }

    /// The window an item (REAPER) or a pattern (Renoise) plays over, from its row.
    async fn window_of(&self, part: &str, signal: &Signal) -> Result<Result<Window, String>, RuntimeError> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": part}))["ref"].as_str().unwrap_or(part).to_owned();
        // A Renoise pattern (or pattern track) plays where its first slot in the sequence is.
        if let Some(window) = self.pattern_window(&long, signal).await {
            return Ok(Ok(window));
        }
        let read = match self.link.call("read", self.read_placed(json!({"ref": long})), signal.clone()).await? {
            Ok(read) => read,
            Err(refused) => return Ok(Err(refused.why)),
        };
        let row = read.pointer("/items/0").cloned().unwrap_or(Value::Null);
        let (from, beats) = (row.get("positionBeats").and_then(Value::as_f64), row.get("lengthBeats").and_then(Value::as_f64));
        Ok(match (from, beats) {
            (Some(from), Some(beats)) if beats > 0. => Ok(Window { from, beats }),
            _ => Err(format!("Kumi couldn't tell where {part} plays: give from_beat and beats")),
        })
    }

    /// Where rebalancing turns the level on a chain, without adding anything: the last limiter that's on (its knob, and
    /// whether lowering it makes it louder), else the last gain device that's on.
    fn gain_stage(&self, devices: &[DeviceRow]) -> Option<(DeviceRow, &'static str, bool, bool)> {
        let on = || devices.iter().rev().filter(|device| device.on);
        if let Some((device, (_, knob, inverted))) = on()
            .find_map(|device| self.table.limiters.iter().find(|(word, _, _)| device.own_name.contains(word)).map(|found| (device, found)))
        {
            return Some((device.clone(), knob, *inverted, true));
        }
        on().find_map(|device| {
            self.table.gains.iter().find(|(word, _)| device.own_name.contains(word)).map(|(_, knob)| (device.clone(), *knob, false, false))
        })
    }

    /// A change of the DAW's for listening (a gain stage turned, a device added or deleted): its id, or why not.
    pub async fn change(&self, tool: &str, input: Value, signal: Signal) -> Result<(String, Value), String> {
        let outcome = self.plan.change_by(tool, object(input), signal).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or(Value::String(outcome.text.clone()));
        if outcome.is_error || outcome.stops {
            return Err(head(answer.as_str().unwrap_or(&outcome.text), 200));
        }
        Ok((text(&answer, "change").unwrap_or_default(), answer))
    }

    /// A parameter's place in its device, from its ref (REAPER `r:param:{FX}:3`, Renoise `E:param:T.D.P`).
    fn param_index(reference: &str) -> Option<u64> {
        reference.rsplit([':', '.']).next()?.parse().ok()
    }

    /// The knobs a fit sets on the DAW's equaliser: its bands by their knobs' names (`Freq-Band 2`, `Gain 3`, …).
    fn eq_bands_of<'a>(&self, knobs: &'a [DeviceKnob]) -> Vec<EqBandKnobs<'a>> {
        eq_bands(knobs, self.table.eq_shelves_at_ends)
    }
}

/// The device a parameter's ref is of, where refs are ids (REAPER's `r:param:{FX GUID}:{index}` is of `r:fx:{FX GUID}`).
pub fn device_of_param(reference: &str) -> Option<String> {
    let (guid, _) = reference.strip_prefix("r:param:")?.rsplit_once(':')?;
    Some(format!("r:fx:{guid}"))
}

/// The longest part a candidate is heard over (as Live caps a clip), in beats.
pub const MAX_PART_BEATS: f64 = 32.;

/// One knob's change: the device as it is now with the knob's number (a knob's own ref can be from before a search's copies
/// moved Renoise's refs), or the knob's ref where it has no number.
fn knob_change(device: &str, knob: &str, field: &str, raw: f64) -> Value {
    let mut input = match knob.rsplit([':', '.']).next().and_then(|n| n.parse::<u64>().ok()) {
        Some(index) => json!({"ref": device, "param": index}),
        None => json!({"ref": knob}),
    };
    input[field] = json!(raw);
    input
}

/// A level a knob shows, in dB: its text's number in dB, or a bare number where the knob is named in dB (a JSFX's
/// "Adjustment (dB)" shows "6.0").
fn shown_db(knob: &str, shown: &str) -> Option<f64> {
    let reading = parse_display(shown)?;
    let db = reading.unit == "db" || (reading.unit.is_empty() && knob.contains("dB"));
    db.then_some(reading.value)
}

/// A parameter's text across its range where the DAW can't say it (Renoise's rows give their current text only), when
/// its value is its text's number (a native device's dB, Hz or ms, as Renoise's are): read off its range, so it scales as
/// one read across the range would. None for one whose text is another number (a linear gain shown in dB).
fn grid_from_units(value: f64, shown: &str, min: f64, max: f64) -> Option<Vec<(f64, String)>> {
    let reading = parse_display(shown)?;
    let near = (reading.value - value).abs() <= ((max - min).abs() * 1e-3).max(1e-6);
    if !near || !(max > min) {
        return None;
    }
    // (a bare number is the value itself: an EQ's bandwidth "2.00")
    let unit = match reading.unit.as_str() {
        "" => "",
        "db" => " dB",
        "hz" => " Hz",
        "ms" => " ms",
        "s" | "sec" => " s",
        "%" => " %",
        _ => return None,
    };
    Some((0..=8).map(|step| min + (max - min) * step as f64 / 8.).map(|at| (at, format!("{at:.4}{unit}"))).collect())
}

/// A listen's pass, from what it captured.
fn pass(rendered: Result<Captured, RuntimeError>, notes: Vec<String>, tried: Option<String>, audible: bool) -> Pass {
    Pass { rendered, notes, tried, lead: 0., audible }
}

/// One band of an equaliser by its knobs: its label, shape, and the knobs of its frequency, gain and width.
#[derive(Debug)]
pub struct EqBandKnobs<'a> {
    pub label: String,
    pub shape: Shape,
    pub frequency: &'a DeviceKnob,
    pub gain: &'a DeviceKnob,
    pub width: Option<&'a DeviceKnob>,
    /// Its width is a bandwidth in octaves, not a Q.
    pub octaves: bool,
}

/// An equaliser's bands from its knobs' names: ReaEQ's "Freq-Low Shelf", "Gain-Band 2", "Q-Band 2"; Renoise's "Gain 3",
/// "Frequency 3", "Bandwidth 3" (shelves at the ends when `shelves_at_ends`).
pub fn eq_bands(knobs: &[DeviceKnob], shelves_at_ends: bool) -> Vec<EqBandKnobs<'_>> {
    let mut labels: Vec<String> = vec![];
    let split = |name: &str| -> Option<(String, String)> {
        let name = name.trim();
        for (what, words) in [
            ("freq", &["Freq-", "Freq ", "Frequency ", "Frequency-"][..]),
            ("gain", &["Gain-", "Gain "][..]),
            ("width", &["Q-", "Q ", "BW-", "BW ", "Bandwidth ", "Bandwidth-"][..]),
        ] {
            for word in words {
                if let Some(label) = name.strip_prefix(word).filter(|label| !label.is_empty()) {
                    // (Renoise names a gain by its band's frequency too: "Gain 1 (50 Hz)")
                    let label = label.split(" (").next().unwrap_or(label).trim();
                    return Some((what.to_owned(), label.to_owned()));
                }
            }
        }
        None
    };
    for knob in knobs {
        if let Some((_, label)) = split(&knob.name) {
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
    }
    let count = labels.len();
    labels
        .iter()
        .enumerate()
        .filter_map(|(at, label)| {
            let knob = |what: &str| knobs.iter().find(|knob| split(&knob.name).is_some_and(|(kind, of)| kind == what && of == *label));
            let (frequency, gain) = (knob("freq")?, knob("gain")?);
            let width = knob("width");
            let lower = label.to_lowercase();
            // (a pass, notch or band-pass band cuts, it doesn't boost or cut around a frequency: no fit goes there)
            if lower.contains("pass") || lower.contains("notch") {
                return None;
            }
            let shape = if lower.contains("low shelf") || (shelves_at_ends && at == 0) {
                Shape::LowShelf
            } else if lower.contains("high shelf") || (shelves_at_ends && at + 1 == count) {
                Shape::HighShelf
            } else {
                Shape::Bell
            };
            let octaves = width.is_some_and(|width| width.name.starts_with("BW") || width.name.starts_with("Bandwidth"));
            Some(EqBandKnobs { label: label.clone(), shape, frequency, gain, width, octaves })
        })
        .collect()
}

/// A Q as a bandwidth in octaves (a bell's, as ReaEQ's bandwidth knobs read it).
fn octaves_of(q: f64) -> f64 {
    2. / std::f64::consts::LN_2 * (1. / (2. * q.max(0.05))).asinh()
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> Renderer for OwnRenderer<S> {
    fn profile(&self) -> &DawProfile {
        self.link.daw.profile
    }
    fn refusal(&self) -> Option<String> {
        self.why_not()
    }
    fn tempo(&self) -> Option<f64> {
        self.link.tempo.get()
    }
    fn beats_per_bar(&self) -> f64 {
        self.beats_per_bar.get()
    }
    fn lifetime(&self) -> Signal {
        self.link.lifetime.clone()
    }
    fn cleanup(&self) -> Signal {
        self.cleanup_signal()
    }
    fn tell(&self, title: &str, _playing: Option<bool>) {
        // (a render's note was told as it ended: once is enough)
        let mut told = self.told_notes.borrow_mut();
        if let Some(at) = told.iter().position(|note| note == title) {
            told.remove(at);
            return;
        }
        drop(told);
        // (the DAW doesn't play for a render: what plays out loud after is said on its own)
        self.tell_app(title);
    }
    fn flag(&self) -> &ListenFlag {
        &self.flag
    }
    fn kept_folder(&self) -> PathBuf {
        self.folder.join("kept")
    }

    async fn song_end(&self, signal: Signal) -> Option<f64> {
        let rows = self.rows(json!({"kind": "spans", "limit": 200}), 8, &signal).await.ok()?.ok()?;
        let end = rows
            .iter()
            .flat_map(|row| row.get("spans").and_then(Value::as_array).cloned().unwrap_or_default())
            .filter_map(|span| span.get(1).and_then(Value::as_f64))
            .fold(0., f64::max);
        (end > 0.).then_some(end)
    }
    async fn track_name(&self, named: &str, signal: Signal) -> Option<String> {
        self.resolve(named, &signal).await.ok()?.ok().map(|(_, name)| name)
    }
    async fn track_ref(&self, named: &str, signal: Signal) -> Option<String> {
        // (only a run's renames follow it: by its place)
        self.resolve(named, &signal).await.ok()?.ok().map(|(reference, _)| self.place_key(&reference))
    }
    async fn clip_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let named = named.trim();
        let long = self.link.book.borrow().lengthen(&json!({"ref": named}))["ref"].as_str().unwrap_or(named).to_owned();
        // A file names itself; only something of the project's, by its ref, has its audio found.
        if self.link.book.borrow().scheme.kind_of(&long).is_none() {
            if crate::bridge::SHORT_REF.is_match(named) {
                let place = self.profile().words.place;
                return Err(RuntimeError::Observation(format!(
                    "{named} isn't one of this turn's refs (it may be from before the {place} changed): read again"
                )));
            }
            return Ok(None);
        }
        let answer = match self.ask_while_busy("audio_file", json!({"ref": long}), &signal, || signal.clone()).await? {
            Ok(answer) => answer,
            Err(refused) => return Err(RuntimeError::Observation(refused.why)),
        };
        let own_playing = self.own_playing();
        // (a part rendered, as Renoise renders a pattern: its notes and the tempo it moved, as every render's are)
        if answer.get("jobId").is_some() {
            return match self.wait(&answer, &signal).await? {
                Ok(done) => {
                    self.finished(&done, own_playing);
                    Ok(done.pointer("/files/0/file").and_then(Value::as_str).map(str::to_owned))
                }
                Err(why) => Err(RuntimeError::Observation(why)),
            };
        }
        let Some(file) = text(&answer, "file") else { return Ok(None) };
        let (offset, length) = (answer.get("offset").and_then(Value::as_f64), answer.get("length").and_then(Value::as_f64));
        let speed = answer.get("playRate").and_then(Value::as_f64).unwrap_or(1.);
        let reversed = answer.get("reversed") == Some(&Value::Bool(true));
        // An item played at another speed is heard as REAPER plays it: its track's own audio rendered over the item.
        if (speed - 1.).abs() > 1e-6 && self.table.no_items.is_none() {
            if let Ok(item) = self.item_row(&long, &signal).await {
                let (track, from, beats) = (
                    text(&item, "trackRef"),
                    item.get("positionBeats").and_then(Value::as_f64),
                    item.get("lengthBeats").and_then(Value::as_f64),
                );
                if let (Some(track), Some(from), Some(beats)) = (track, from, beats) {
                    if let Ok(rendered) = self.render(&track, from, beats, signal.clone()).await? {
                        if let Some(path) = rendered.get("file").and_then(Value::as_str) {
                            return Ok(Some(path.to_owned()));
                        }
                    }
                }
            }
        }
        if offset.is_none() && length.is_none() && !reversed {
            return Ok(Some(file));
        }
        // Only what the item (or the slice) plays of its file, the way it plays it; the whole file if it can't be cut.
        Ok(Some(self.cut(&file, offset.unwrap_or(0.), length, reversed, &signal).await.unwrap_or(file)))
    }

    async fn hear_pass(&self, candidates: &[AuditionCandidate], window: Window, before_main: bool, signal: Signal) -> Pass {
        self.hear_candidates(candidates, window, before_main, &signal).await
    }
    async fn master_chain(&self, signal: Signal) -> MainState {
        // A chain Kumi can't read counts as one masking can't be read through.
        let unreadable =
            || MainState { unfair: vec![format!("{} (Kumi couldn't read its devices)", self.profile().words.master)], gain: 0. };
        let Ok(Ok((master, _))) = self.scope(None, &signal).await else { return unreadable() };
        let Ok(Ok(devices)) = self.devices(&master, &signal).await else { return unreadable() };
        let mut state = MainState::default();
        for device in devices.iter().filter(|device| device.on) {
            if self.table.analyzers.iter().any(|word| device.own_name.contains(word)) {
                continue;
            }
            // With tracks rendered through the master, what acts on one track as on the mix is in both already.
            if self.table.post_fader {
                if !device.mixer && !self.table.linear.iter().any(|word| device.own_name.contains(word)) {
                    state.unfair.push(device.name.clone());
                }
                continue;
            }
            let gain = match self.table.gains.iter().find(|(word, _)| device.own_name.contains(word)) {
                Some((_, knob)) => match self.parameters(&device.reference, false, &signal).await {
                    Ok(Ok(rows)) => rows
                        .iter()
                        .find(|row| text(row, "name").as_deref() == Some(*knob))
                        .and_then(|row| text(row, "formatted").or_else(|| text(row, "valueString")))
                        .and_then(|shown| shown_db(knob, &shown)),
                    _ => None,
                },
                None => None,
            };
            match gain {
                Some(gain) if gain.is_finite() => state.gain += gain,
                _ => state.unfair.push(device.name.clone()),
            }
        }
        if self.table.master_fader {
            let fader = match self.track_rows(&signal).await {
                Ok(Ok(tracks)) => {
                    tracks.iter().find(|track| Some(track.reference.as_str()) == self.table.master_ref).and_then(|track| track.volume_db)
                }
                _ => None,
            };
            state.gain += fader.filter(|db| db.is_finite()).unwrap_or(0.);
        }
        state.gain = (state.gain * 10.).round() / 10.;
        state
    }
    async fn fader_of(&self, track: &str, signal: Signal) -> Option<f64> {
        if self.table.post_fader {
            // (a track renders after its fader already)
            return Some(0.);
        }
        let (reference, _) = self.resolve(track, &signal).await.ok()?.ok()?;
        self.fader_at(&reference, &signal).await
    }

    fn applied(&self) -> Vec<String> {
        self.plan
            .history
            .entries
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, _)| id.clone())
            .collect()
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        self.plan
            .history
            .entries
            .borrow()
            .iter()
            .filter(|(id, entry)| !mark.contains(id) && entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, entry)| (id.clone(), entry.borrow().record.title.clone()))
            .collect()
    }
    fn audible(&self, id: &str) -> Option<bool> {
        /// What changes nothing heard: names, colours, marks, the loop, an empty track added.
        const SILENT: [&str; 14] = [
            "track_rename",
            "track_color",
            "track_add",
            "marker_add",
            "region_add",
            "marker_move",
            "marker_rename",
            "marker_color",
            "marker_delete",
            "loop_range",
            "loop_repeat",
            "loop_pattern",
            "slot_section",
            "instrument_rename",
        ];
        let entries = self.plan.history.entries.borrow();
        let entry = entries.get(id)?.borrow();
        Some(match &entry.tool {
            Some(tool) => !SILENT.contains(&tool.as_str()),
            None => !matches!(entry.record.family, ChangeFamily::Rename | ChangeFamily::Color | ChangeFamily::Locators),
        })
    }
    fn outside(&self) -> usize {
        self.plan.history.outside.borrow().len()
    }
    fn outside_since(&self, mark: usize) -> Vec<&'static str> {
        self.plan.history.outside_since(mark)
    }
    async fn undo(&self, id: &str) -> Result<(), String> {
        match self.plan.history.undo(id, self.cleanup_signal(), false).await {
            Ok(undone) if !undone.is_error => Ok(()),
            Ok(undone) => Err(head(&undone.text, 120)),
            Err(error) => Err(head(&error.to_string(), 120)),
        }
    }
    // `made` stays empty: a change Kumi's undo refused (the producer changed what it made since) is said, and what it made
    // is never deleted in its place.

    async fn turn_gain(&self, gain: f64, track: Option<&str>, signal: Signal) -> Result<(String, bool), String> {
        let (scope, _) = self.scope(track, &signal).await.map_err(|error| error.to_string())??;
        let mut devices = self.devices(&scope, &signal).await.map_err(|error| error.to_string())??;
        // A gain device Kumi adds isn't in what was heard: its knob is set to the turn itself, whatever it starts at (REAPER
        // adds a JS Volume Adjustment at +6 dB, measured).
        let mut fresh = false;
        let stage = match self.gain_stage(&devices) {
            Some(stage) => stage,
            None => {
                fresh = true;
                let (add, name) = self.table.gain_device;
                let mut input = json!({"ref": scope});
                input[self.table.add_field] = add.into();
                // Before the chain's last device that shapes the sound when it may hold the ceiling there (a limiter, or a
                // plug-in effect Kumi doesn't know): turned up after it, the level would go past it. After anything else (an
                // instrument the sound starts at, the DAW's own effects), so the turn is heard as it is.
                let last = devices
                    .iter()
                    .rev()
                    .find(|device| !device.mixer && !self.table.analyzers.iter().any(|word| device.own_name.contains(word)));
                let ceiling = |device: &&DeviceRow| {
                    !device.instrument && (device.plugin || self.table.limiters.iter().any(|(word, _, _)| device.own_name.contains(word)))
                };
                if let Some(last) = last.filter(ceiling) {
                    input["index"] = last.index.into();
                }
                self.change(self.table.add_tool, input, signal.clone()).await?;
                devices = self.devices(&scope, &signal).await.map_err(|error| error.to_string())??;
                let added = devices.iter().rev().find(|device| device.own_name.contains(name)).cloned();
                let added = added.ok_or_else(|| format!("the {name} Kumi added didn't appear"))?;
                let knob = self.table.gains.iter().find(|(word, _)| name.contains(word)).map_or("Gain", |(_, knob)| *knob);
                (added, knob, false, false)
            }
        };
        let (device, knob, inverted, limited) = stage;
        let turn = if inverted { -gain } else { gain };
        let (_, wanted) = self.turn_knob(&device, knob, turn, fresh, &signal).await?;
        // Said by the level it turns: "Volume Adjustment gain", "ReaLimit level (its Threshold)".
        let named = plugin_name(&device.name);
        let mut label = if inverted { format!("{named} level (its {knob})") } else { format!("{named} gain") };
        // What a limiter's knob couldn't take (a Maximizer's Boost stops at 0 dB, ReaLimit's Threshold at 0 dB) goes on a
        // gain device after it.
        let got = self.knob_db(&device, knob, &signal).await.ok().filter(|_| limited);
        let left = got.map_or(0., |got| if inverted { got - wanted } else { wanted - got });
        if left.abs() >= 0.2 {
            let (add, name) = self.table.gain_device;
            let chain = self.devices(&scope, &signal).await.map_err(|error| error.to_string())??;
            let found =
                chain.iter().rev().find(|row| row.on && self.table.gains.iter().any(|(word, _)| row.own_name.contains(word))).cloned();
            let (gainer, fresh) = match found {
                Some(found) => (found, false),
                None => {
                    let mut input = json!({"ref": scope});
                    input[self.table.add_field] = add.into();
                    self.change(self.table.add_tool, input, signal.clone()).await?;
                    let chain = self.devices(&scope, &signal).await.map_err(|error| error.to_string())??;
                    let added = chain
                        .iter()
                        .rev()
                        .find(|row| row.own_name.contains(name))
                        .cloned()
                        .ok_or_else(|| format!("the {name} Kumi added didn't appear"))?;
                    (added, true)
                }
            };
            let gain_knob = self.table.gains.iter().find(|(word, _)| gainer.own_name.contains(word)).map_or("Gain", |(_, knob)| *knob);
            self.turn_knob(&gainer, gain_knob, left, fresh, &signal).await?;
            label = format!("{label} and {} gain", plugin_name(&gainer.name));
        }
        Ok((label, limited))
    }
    async fn limited(&self, track: Option<&str>, signal: Signal) -> bool {
        let Ok(Ok((scope, _))) = self.scope(track, &signal).await else { return false };
        let Ok(Ok(devices)) = self.devices(&scope, &signal).await else { return false };
        devices.last().is_some_and(verified_plugin_limiter) || self.gain_stage(&devices).is_some_and(|(_, _, _, limited)| limited)
    }
    async fn chain(&self, track: Option<&str>) -> Result<Vec<Placed>, String> {
        let signal = self.cleanup_signal();
        let (scope, _) = self.scope(track, &signal).await.map_err(|error| error.to_string())??;
        let devices = self.devices(&scope, &signal).await.map_err(|error| error.to_string())??;
        // A device's identity is its ref where refs hold (REAPER's FX GUIDs); places (Renoise's) are no identity.
        let lasting = !self.link.book.borrow().scheme.positional();
        Ok(devices
            .into_iter()
            .map(|device| Placed {
                identity: if lasting { device.reference.clone() } else { String::new() },
                name: device.name,
                reference: device.reference,
            })
            .collect())
    }
    async fn delete_device(&self, reference: &str) -> bool {
        self.change(self.table.delete_tool, json!({"ref": reference}), self.cleanup_signal()).await.is_ok()
    }

    async fn knobs(&self, device: &str, signal: Signal) -> Result<Result<Vec<DeviceKnob>, String>, RuntimeError> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        let rows = match self.parameters(&long, true, &signal).await? {
            Ok(rows) => rows,
            Err(why) => return Ok(Err(why)),
        };
        let number = |row: &Value, key: &str| row.get(key).and_then(Value::as_f64);
        let knobs: Vec<DeviceKnob> = rows
            .iter()
            .filter_map(|row| {
                let items: Vec<String> =
                    row.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                let grid: Vec<(f64, String)> = row
                    .get("grid")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|point| Some((point.get(0)?.as_f64()?, point.get(1)?.as_str()?.to_owned())))
                    .collect();
                let (min, max) =
                    if self.table.normalized { (0., 1.) } else { (number(row, "min").unwrap_or(0.), number(row, "max").unwrap_or(1.)) };
                let short = row.get("gridShort") == Some(&Value::Bool(true)) || grid.len() < 2;
                let grid = match (short, number(row, "value"), text(row, "valueString").or_else(|| text(row, "formatted"))) {
                    (true, Some(value), Some(shown)) if items.is_empty() => grid_from_units(value, &shown, min, max).unwrap_or(grid),
                    _ => grid,
                };
                Some(DeviceKnob {
                    reference: text(row, "ref")?,
                    name: text(row, "name")?,
                    raw: number(row, "value").unwrap_or(min),
                    min,
                    max,
                    scale: if items.is_empty() && grid.len() > 1 { Scale::read(&grid) } else { None },
                    items,
                })
            })
            .collect();
        if knobs.is_empty() {
            return Ok(Err("Kumi couldn't read the device's knobs; is it still there? Read it again.".into()));
        }
        Ok(Ok(knobs))
    }
    async fn set_knobs(&self, device: &str, values: &[(&str, f64)], signal: Signal) -> Result<(), RuntimeError> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        // Several of one device's knobs in one change, where the DAW makes such a change.
        if self.link.daw.changes.kinds.iter().any(|kind| kind.tool == self.table.params_tool) {
            let set: Vec<Value> = values
                .iter()
                .map(|(reference, raw)| {
                    let mut value = json!({"param": Self::param_index(reference)});
                    value[self.table.raw_field] = json!(raw);
                    value
                })
                .collect();
            return self
                .change(self.table.params_tool, json!({"ref": long, "values": set}), signal)
                .await
                .map(|_| ())
                .map_err(RuntimeError::plain);
        }
        // Otherwise one change each, kept in HISTORY as one.
        let mut ids = vec![];
        for (reference, raw) in values {
            let input = knob_change(&long, reference, self.table.raw_field, *raw);
            match self.change(self.table.param_tool, input, signal.clone()).await {
                Ok((id, _)) if !id.is_empty() => ids.push(id),
                Ok(_) => {}
                Err(why) => return Err(RuntimeError::plain(why)),
            }
        }
        if ids.len() > 1 {
            self.plan.history.grouped(&format!("{} knobs set", ids.len()), &ids, &[]);
        }
        Ok(())
    }
    async fn device_identity(&self, device: &str, signal: Signal) -> Option<DeviceIdentity> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        let read = self.link.call("read", self.read_placed(json!({"ref": long})), signal).await.ok()?.ok()?;
        identity_of(read.pointer("/items/0")?).map(DeviceIdentity::named)
    }
    async fn device_kind(&self, device: &str, signal: Signal) -> Option<String> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        let read = self.link.call("read", self.read_placed(json!({"ref": long})), signal).await.ok()?.ok()?;
        let row = read.pointer("/items/0")?;
        // (by the plug-in's own name, whatever the producer renamed the FX)
        let name = text(row, "originalName").or_else(|| text(row, "name"))?;
        // REAPER: "VST3: Pro-Q 3 (FabFilter)" is Pro-Q 3; Renoise: a native device by its path's last part.
        let kind = match text(row, "path").filter(|path| path.contains("/Native/")) {
            Some(path) => path.rsplit('/').next().unwrap_or(&name).to_owned(),
            None => plugin_name(&name),
        };
        (!kind.is_empty()).then_some(kind)
    }
    async fn fit_eq(
        &self,
        device: &str,
        knobs: &[DeviceKnob],
        bands: &[Band],
        signal: Signal,
    ) -> Result<Result<EqFit, String>, RuntimeError> {
        let eq = self.table.eq;
        let kind = self.device_kind(device, signal).await.unwrap_or_default();
        if !kind.contains(eq) && !kind.starts_with("EQ") {
            return Ok(Err(format!(
                "fit sets {}'s bands: put {} where the fix belongs, then tune it.",
                kumi_types::daw::with_article(eq),
                kumi_types::daw::with_article(eq)
            )));
        }
        let all = self.eq_bands_of(knobs);
        // A band that does nothing (its gain at 0 dB) takes a fitted one of its shape.
        let free: Vec<&EqBandKnobs> =
            all.iter().filter(|band| band.gain.scale.as_ref().is_some_and(|scale| scale.shown(band.gain.raw).abs() < 0.05)).collect();
        let mut taken: Vec<usize> = vec![];
        let mut values = vec![];
        for band in bands {
            let slot = free.iter().enumerate().find(|(at, slot)| !taken.contains(at) && slot.shape == band.shape);
            let Some((at, slot)) = slot else {
                return Ok(Err(format!(
                    "{} has {} free bands of the shapes the fit needs, and it needs {}; add another {eq}.",
                    kind,
                    free.len(),
                    bands.len()
                )));
            };
            taken.push(at);
            let (Some(hz), Some(db)) = (slot.frequency.scale.as_ref(), slot.gain.scale.as_ref()) else {
                return Ok(Err(format!("Kumi couldn't read the units of {kind}'s {}.", slot.label)));
            };
            values.push((slot.frequency.reference.clone(), hz.raw(band.hz)));
            values.push((slot.gain.reference.clone(), db.raw(band.db)));
            if let Some(width) = slot.width {
                let Some(scale) = width.scale.as_ref() else {
                    return Ok(Err(format!("Kumi couldn't read the width of {kind}'s {}.", slot.label)));
                };
                values.push((width.reference.clone(), scale.raw(if slot.octaves { octaves_of(band.q) } else { band.q })));
            }
        }
        Ok(Ok(EqFit { values, settings: vec![] }))
    }
    async fn on_track(&self, device: &str, track: &str, signal: Signal) -> bool {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        let Ok(Ok((scope, _))) = self.resolve(track, &signal).await else { return false };
        matches!(self.devices(&scope, &signal).await, Ok(Ok(devices)) if devices.iter().any(|row| self.same_place(&row.reference, &long)))
    }
    async fn open_copies(
        &self,
        device: &str,
        count: usize,
        label: &str,
        _passes: u32,
        window: Window,
        signal: Signal,
    ) -> Result<Result<Copies, String>, RuntimeError> {
        if !self.flags.get().scratch {
            return Ok(Err(format!("{} can't make scratch copies of a track for Kumi just now.", self.app())));
        }
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        let (track, chain) = match self.track_of_device(&long, &signal).await? {
            Ok(found) => found,
            Err(why) => return Ok(Err(why)),
        };
        let Some(place) = chain.iter().find(|row| self.same_place(&row.reference, &long)).map(|row| row.index) else {
            return Ok(Err("Kumi couldn't find the device on its track; read it again.".into()));
        };
        let knobs = match self.parameters(&long, false, &signal).await? {
            Ok(rows) => rows
                .iter()
                .filter_map(|row| {
                    Some((text(row, "name")?, row.get("index").and_then(Value::as_u64).or_else(|| Self::param_index(&text(row, "ref")?))?))
                })
                .collect(),
            Err(why) => return Ok(Err(why)),
        };
        let args = json!({"action": "make", "ref": track, "count": count, "label": label, "range": {"fromBeat": window.from, "beats": window.beats}});
        // Asked in the clean-up's own time: copies made while the producer stopped are dropped, never left behind. (A stop
        // while Kumi waits for a busy DAW has nothing to drop: a refusal made nothing.)
        let made = match self.ask_while_busy("scratch", args, &signal, || self.cleanup_signal()).await? {
            Ok(made) => made,
            Err(refused) => return Ok(Err(refused.why)),
        };
        let Some(id) = text(&made, "scratchId") else { return Ok(Err(format!("{} made no scratch copies", self.app()))) };
        if signal.is_cancelled() {
            self.drop_make(&id, made.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0)).await;
            return Err(RuntimeError::Aborted);
        }
        let mut names = vec![];
        for copy in made.get("copies").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(reference), Some(name)) = (text(copy, "ref"), text(copy, "name")) {
                self.keep_copy(name.clone(), &reference);
                names.push(name);
            }
        }
        self.tell_scratch_steps(made.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0));
        for note in notes_of(&made) {
            self.tell_app(&note);
        }
        Ok(Ok(Copies { names, held: Box::new(Scratch { id, device: place, knobs, steps: Cell::new(0) }) }))
    }
    async fn set_copies(
        &self,
        copies: &Copies,
        values: serde_json::Map<String, Value>,
        signal: Signal,
    ) -> Result<Result<(), String>, RuntimeError> {
        let Some(held) = copies.held.downcast_ref::<Scratch>() else { return Ok(Err("those copies aren't Kumi's scratch copies".into())) };
        let mut set = vec![];
        for (name, knobs) in &values {
            let Some(reference) = self.copy_ref(name) else { continue };
            for (knob, raw) in knobs.as_object().into_iter().flatten() {
                let Some(param) = held.knobs.get(knob) else { return Ok(Err(format!("the device has no knob “{knob}”"))) };
                let mut value = json!({"ref": reference, "device": held.device, "param": param});
                value[self.table.scratch_field] = raw.clone();
                set.push(value);
            }
        }
        if set.is_empty() {
            return Ok(Ok(()));
        }
        Ok(match self.ask("scratch", json!({"action": "set", "scratchId": held.id, "values": set}), signal).await? {
            Ok(done) => {
                held.steps.set(held.steps.get() + done.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0));
                Ok(())
            }
            Err(refused) => Err(refused.why),
        })
    }
    async fn close_copies(&self, copies: Copies) {
        let Ok(held) = copies.held.downcast::<Scratch>() else { return };
        self.drop_make(&held.id, held.steps.get()).await;
        self.copies.borrow_mut().retain(|name, _| !copies.names.contains(name));
    }

    async fn track_devices(&self, device: &str, signal: Signal) -> Result<(Vec<Seen>, usize), RuntimeError> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        let (_, chain) = self.track_of_device(&long, &signal).await?.map_err(RuntimeError::Observation)?;
        let at = chain.iter().position(|row| self.same_place(&row.reference, &long)).unwrap_or(0);
        Ok((chain.iter().map(seen).collect(), at))
    }
    async fn listed_names(&self, device: &str, signal: Signal) -> Result<Option<Vec<String>>, RuntimeError> {
        let long = self.link.book.borrow().lengthen(&json!({"ref": device}))["ref"].as_str().unwrap_or(device).to_owned();
        Ok(self.parameters(&long, false, &signal).await?.ok().map(|rows| rows.iter().filter_map(|row| text(row, "name")).collect()))
    }
    async fn read_seen(&self, device: &mut Seen, signal: Signal) {
        // (a grid of two points: a stepped parameter answers its steps' texts)
        let Ok(Ok(rows)) = self.rows(json!({"kind": "param", "parent": device.reference, "grid": 2}), 16, &signal).await else { return };
        device.turnable = rows.iter().filter_map(|row| text(row, "name")).collect();
        // A switch: a parameter of two steps (or 0 to 1), on at its top.
        device.switched_on = rows
            .iter()
            .filter(|row| {
                let max = row.get("max").and_then(Value::as_f64).unwrap_or(1.);
                let value = row.get("value").and_then(Value::as_f64).unwrap_or(0.);
                steps_of(row) == Some(2) && value >= max * 0.5
            })
            .filter_map(|row| text(row, "name"))
            .collect();
        if device.name.contains(self.table.eq) {
            let knobs: Vec<DeviceKnob> = rows
                .iter()
                .filter_map(|row| {
                    let shown = text(row, "formatted").or_else(|| text(row, "valueString"))?;
                    let value = parse_display(&shown)?.value;
                    Some(DeviceKnob {
                        reference: text(row, "ref")?,
                        name: text(row, "name")?,
                        raw: value,
                        min: value,
                        max: value,
                        items: vec![],
                        scale: None,
                    })
                })
                .collect();
            device.bands = self.eq_bands(&knobs);
        }
        if device.adapter().is_some() {
            device.listed = Some(device.turnable.clone());
        }
    }
    fn eq_bands(&self, knobs: &[DeviceKnob]) -> Vec<(u8, f64, bool, bool)> {
        self.eq_bands_of(knobs)
            .iter()
            .enumerate()
            .map(|(at, band)| {
                let hz = band.frequency.scale.as_ref().map_or(band.frequency.raw, |scale| scale.shown(band.frequency.raw));
                (at as u8 + 1, hz, true, true)
            })
            .collect()
    }
    async fn devices_seen(&self, track: Option<&str>, signal: Signal) -> Option<Vec<Seen>> {
        let (scope, _) = self.scope(track, &signal).await.ok()?.ok()?;
        let devices = self.devices(&scope, &signal).await.ok()?.ok()?;
        Some(devices.iter().map(seen).collect())
    }

    fn render_refusal(&self) -> Option<String> {
        self.why_not()
    }
    async fn audition_pass(
        &self,
        candidates: &[AuditionCandidate],
        from_beat: Option<f64>,
        beats: Option<f64>,
        options: AuditionOptions,
        signal: Signal,
    ) -> AuditionPass {
        let failed = |error: RuntimeError, opened: bool| AuditionPass { rendered: Err(error), notes: vec![], opened };
        // Each candidate is heard over its own part (an item, a pattern), from its start for one length (beats, else the
        // part's, at most MAX_PART_BEATS); a candidate without one over the first part's stretch, or from_beat for beats.
        let windows = match self.part_windows(candidates, from_beat, beats, &signal).await {
            Ok(Ok(windows)) => windows,
            Ok(Err(why)) => return failed(RuntimeError::Observation(why), false),
            Err(error) => return failed(error, false),
        };
        let window = windows[0];
        let first =
            candidates.first().map(|candidate| candidate.label.clone().unwrap_or_else(|| candidate.track.clone())).unwrap_or_default();
        self.tell_app(&format!(
            "Round {}: rendering {}{}",
            options.round,
            first,
            match candidates.len() {
                1 => String::new(),
                2 => " and 1 more".into(),
                n => format!(" and {} more", n - 1),
            }
        ));
        // One render for each stretch the candidates are heard over (most rounds: one), the first played out loud.
        let mut froms: Vec<f64> = vec![];
        for window in &windows {
            if !froms.iter().any(|from| (from - window.from).abs() < 1e-9) {
                froms.push(window.from);
            }
        }
        let mut notes = vec![];
        let mut audible = false;
        let mut names: Vec<Option<String>> = vec![None; candidates.len()];
        let mut files = IndexMap::new();
        for (n, from) in froms.iter().enumerate() {
            let at: Vec<usize> = (0..candidates.len()).filter(|i| (windows[*i].from - from).abs() < 1e-9).collect();
            let group: Vec<AuditionCandidate> = at.iter().map(|i| candidates[*i].clone()).collect();
            let heard = self.hear_candidates_aloud(&group, Window { from: *from, beats: window.beats }, false, n == 0, &signal).await;
            notes.extend(heard.notes);
            audible |= n == 0 && heard.audible;
            let captured = match heard.rendered {
                Ok(captured) => captured,
                Err(error) => return AuditionPass { rendered: Err(error), notes, opened: true },
            };
            for (i, (name, _)) in at.iter().zip(captured.sources) {
                // (the same track over two parts: the second goes by "name (2)")
                let mut unique = name.clone();
                let mut k = 2;
                while files.contains_key(&unique) {
                    unique = format!("{name} ({k})");
                    k += 1;
                }
                if let Some(render) = captured.files.get(&name) {
                    files.insert(unique.clone(), render.clone());
                }
                names[*i] = Some(unique);
            }
        }
        let captured =
            Captured { sources: names.into_iter().map(|name| (name.unwrap_or_default(), false)).collect(), files, before_master: None };
        let sources: Vec<AuditionSource> = candidates
            .iter()
            .enumerate()
            .zip(captured.sources.iter())
            .map(|((at, candidate), (name, _))| AuditionSource {
                track: candidate.track.clone(),
                name: name.clone(),
                scene: None,
                audible: at == 0 && audible,
            })
            .collect();
        if audible && candidates.len() > 1 {
            notes.push(format!("{} rendered the round offline; the producer hears the first candidate out loud now.", self.app()));
        }
        AuditionPass { rendered: Ok(AuditionRender { sources, files: captured.files, beats: window.beats }), notes, opened: true }
    }
    async fn render(&self, track: &str, from: f64, beats: f64, signal: Signal) -> Result<Result<JsonObject, String>, RuntimeError> {
        if let Some(why) = self.table.no_items {
            return Ok(Err(why.into()));
        }
        let (reference, _) = match self.resolve(track, &signal).await? {
            Ok(found) => found,
            Err(why) => return Ok(Err(why)),
        };
        let source = json!({"id": "items", "what": "track", "ref": reference, "stage": "items"});
        let args = json!({"sources": [source], "range": {"fromBeat": from, "beats": beats}, "preRollBeats": 0});
        let started = match self.ask_while_busy("render", args, &signal, || signal.clone()).await? {
            Ok(started) => started,
            Err(refused) => return Ok(Err(refused.why)),
        };
        let own_playing = self.own_playing();
        let done = match self.wait(&started, &signal).await? {
            Ok(done) => done,
            Err(why) => return Ok(Err(why)),
        };
        let notes = self.finished(&done, own_playing);
        let Some(file) = done.pointer("/files/0") else { return Ok(Err(format!("{} rendered nothing", self.app()))) };
        let mut reply = JsonObject::new();
        for key in ["file", "seconds", "channels", "sampleRate"] {
            if let Some(value) = file.get(key) {
                reply.insert(key.into(), value.clone());
            }
        }
        if !notes.is_empty() {
            reply.insert("notes".into(), json!(notes));
        }
        Ok(Ok(reply))
    }
    fn short_ref(&self, long: &str) -> String {
        self.link.book.borrow_mut().short_ref(long)
    }
    fn emit(&self, record: ChangeRecord) {
        self.plan.history.emit(&record)
    }

    async fn clip_notes(&self, clip: &str, signal: Signal) -> Result<ClipNotes, String> {
        self.part_notes(clip, signal).await
    }
    async fn lanes_of(&self, clip: &str, signal: Signal) -> Kit {
        self.part_lanes(clip, signal).await
    }
    async fn change_notes(&self, clip: &str, patches: &[NotePatch], signal: Signal) -> Result<(), RuntimeError> {
        self.move_notes(clip, patches, signal).await
    }
    fn clip_id(&self, clip: &str) -> Option<String> {
        self.part_id(clip)
    }
    fn note_changes(&self, checkpoint: &[String], clip: Option<&str>) -> (Vec<(String, String)>, Vec<(String, String)>) {
        self.part_changes(checkpoint, clip)
    }
    async fn stem_track(&self, audio: &str, signal: Signal) -> Result<String, String> {
        self.stem_track_of(audio, signal).await
    }
    fn devices_made(&self, changes: &[(String, String)]) -> usize {
        self.devices_added(changes)
    }
    fn renames(&self, before: &[String]) -> Vec<(String, String, String)> {
        self.renames_since(before)
    }

    async fn clips_by_track(&self, signal: Signal) -> Vec<(String, Vec<(f64, f64)>)> {
        let Ok(Ok(rows)) = self.rows(json!({"kind": "spans", "limit": 64}), 1, &signal).await else { return vec![] };
        // What plays nowhere, or isn't one of the song's parts: a muted track (the row says so, or the track's own row),
        // the master, a send, and Kumi's scratch copies.
        let tracks = self.track_rows(&signal).await.ok().and_then(Result::ok).unwrap_or_default();
        let silent = |row: &Value| {
            let reference = text(row, "ref").unwrap_or_default();
            let track = tracks.iter().find(|track| self.same_place(&track.reference, &reference));
            row.get("mute") == Some(&Value::Bool(true))
                || track.is_some_and(|track| track.muted || matches!(track.kind.as_str(), "master" | "send"))
                || text(row, "name").is_some_and(|name| self.copies.borrow().contains_key(&name))
        };
        rows.iter()
            .take(64)
            .filter(|row| !silent(row))
            .filter_map(|row| {
                let spans = row
                    .get("spans")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|span| Some((span.get(0)?.as_f64()?, span.get(1)?.as_f64()?)))
                    .collect();
                Some((text(row, "name")?, spans))
            })
            .collect()
    }
}

impl<S: RefScheme + 'static> OwnRenderer<S> {
    /// What a device's knob shows now, in dB.
    async fn knob_db(&self, device: &DeviceRow, knob: &str, signal: &Signal) -> Result<f64, String> {
        let rows = self.parameters(&device.reference, false, signal).await.map_err(|error| error.to_string())??;
        let row = rows
            .iter()
            .find(|row| text(row, "name").as_deref() == Some(knob))
            .ok_or_else(|| format!("Kumi couldn't find {}'s {knob}", device.name))?;
        let shown = text(row, "formatted").or_else(|| text(row, "valueString")).unwrap_or_default();
        shown_db(knob, &shown).ok_or_else(|| format!("{}'s {knob} doesn't show dB", device.name))
    }

    /// Turns a device's knob by `turn` dB, by its text (a change of the DAW's): the knob's ref and the dB it was asked for.
    /// A device just added (`fresh`) is set to `turn` dB itself: what was heard didn't have it.
    async fn turn_knob(&self, device: &DeviceRow, knob: &str, turn: f64, fresh: bool, signal: &Signal) -> Result<(String, f64), String> {
        let rows = self.parameters(&device.reference, false, signal).await.map_err(|error| error.to_string())??;
        let row = rows
            .iter()
            .find(|row| text(row, "name").as_deref() == Some(knob))
            .ok_or_else(|| format!("Kumi couldn't find {}'s {knob}", device.name))?;
        let reference = text(row, "ref").ok_or("a parameter without a ref")?;
        let shown = text(row, "formatted").or_else(|| text(row, "valueString")).unwrap_or_default();
        let now = shown_db(knob, &shown).ok_or_else(|| format!("{}'s {knob} doesn't show dB", device.name))?;
        let wanted = if now.is_finite() && !fresh { now + turn } else { turn };
        let mut input = json!({"ref": reference});
        input[self.table.text_field] = format!("{:.1} dB", (wanted * 10.).round() / 10.).into();
        self.change(self.table.param_tool, input, signal.clone()).await?;
        Ok((reference, wanted))
    }

    /// What a file holds from `offset` for `length` seconds (to its end without one), reversed if asked, written as a WAV
    /// in Kumi's listening folder (the host prunes it with the renders): its path.
    async fn cut(&self, file: &str, offset: f64, length: Option<f64>, reversed: bool, signal: &Signal) -> Result<String, String> {
        let mut source = kumi_media::audio::decode::open_audio(file, Some(signal.clone())).await.map_err(|error| error.0)?;
        let rate = source.sample_rate;
        source.seek(offset.max(0.) * rate);
        let frames = length.map_or(usize::MAX, |length| (length.max(0.) * rate) as usize);
        let mut channels: Vec<Vec<f32>> = vec![];
        let mut read = 0;
        while read < frames {
            let Some(block) = source.read(65_536.min(frames - read)).await.map_err(|error| error.0)? else { break };
            let Some(first) = block.first() else { break };
            if first.is_empty() {
                break;
            }
            read += first.len();
            channels.resize(channels.len().max(block.len()), vec![]);
            for (channel, samples) in channels.iter_mut().zip(&block) {
                channel.extend_from_slice(samples);
            }
        }
        let _ = source.close().await;
        if channels.is_empty() {
            return Err("nothing to cut".into());
        }
        if reversed {
            channels.iter_mut().for_each(|channel| channel.reverse());
        }
        let stem = std::path::Path::new(file).file_stem().and_then(|stem| stem.to_str()).unwrap_or("audio");
        let stamp = chrono::Utc::now().timestamp_millis();
        let path = self.folder.join(format!(
            "{stem} · {:.3}+{:.3}{} · {stamp}.wav",
            offset,
            read as f64 / rate,
            if reversed { " reversed" } else { "" }
        ));
        std::fs::create_dir_all(&self.folder).map_err(|error| error.to_string())?;
        std::fs::write(&path, kumi_media::audio::decode::float_wav(&channels, rate as u32)).map_err(|error| error.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// The track a device is on, and that track's chain: by the device's own ref where it names its track (Renoise's
    /// places), else by its row (REAPER's FX row says the track whose chain it's on, and the take for a take's FX). Why
    /// not, in words.
    async fn track_of_device(&self, device: &str, signal: &Signal) -> Result<Result<(String, Vec<DeviceRow>), String>, RuntimeError> {
        const NOT_FOUND: &str = "Kumi couldn't find the device's track; read it again.";
        let named = self.link.book.borrow().scheme.track_of(device);
        let track = match named {
            Some(track) => track,
            None => {
                let row = match self.link.call("read", self.read_placed(json!({"ref": device})), signal.clone()).await? {
                    Ok(read) => read.pointer("/items/0").cloned().unwrap_or(Value::Null),
                    Err(_) => return Ok(Err(NOT_FOUND.into())),
                };
                if row.get("takeRef").and_then(Value::as_str).is_some() {
                    return Ok(Err("That FX is on an item's take: Kumi tunes the FX in a track's chain, not a take's.".into()));
                }
                match text(&row, "trackRef") {
                    Some(track) => track,
                    None => return Ok(Err(NOT_FOUND.into())),
                }
            }
        };
        match self.devices(&track, signal).await? {
            Ok(devices) if devices.iter().any(|row| self.same_place(&row.reference, device)) => Ok(Ok((track, devices))),
            _ => Ok(Err(NOT_FOUND.into())),
        }
    }
}

/// A device as tune sees it: by its name, a plug-in as Live's class for one (so the plug-in map knows it), a DAW's own
/// device by its name.
fn seen(device: &DeviceRow) -> Seen {
    Seen {
        reference: device.reference.clone(),
        name: plugin_name(&device.own_name),
        class: if device.plugin { "PluginDevice".into() } else { plugin_name(&device.own_name) },
        off: !device.on,
        ..Default::default()
    }
}

/// A plug-in's own name from REAPER's FX name ("VST3: Pro-Q 3 (FabFilter)" is "Pro-Q 3"; "JS: Volume Adjustment" is
/// "Volume Adjustment"), or the name as it is.
/// Who a device is, the same in every session and wherever it sits, from its row: REAPER's FX as the plug-in is called,
/// with its format and maker ("VST3: Pro-Q 3 (FabFilter)", its `originalName` once the producer renamed it), Renoise's
/// device by its path ("Audio/Effects/Native/Maximizer", a plug-in's naming its format too). Neither says a version.
pub fn identity_of(row: &Value) -> Option<String> {
    text(row, "originalName").or_else(|| text(row, "path")).or_else(|| text(row, "name")).filter(|identity| !identity.trim().is_empty())
}

pub fn plugin_name(name: &str) -> String {
    let name = name.trim();
    let name = ["VST3i: ", "VST3: ", "VSTi: ", "VST: ", "AUi: ", "AU: ", "CLAPi: ", "CLAP: ", "JS: ", "LV2i: ", "LV2: "]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name);
    match name.rfind(" (") {
        Some(at) if name.ends_with(')') && at > 0 => name[..at].to_owned(),
        _ => name.to_owned(),
    }
}

/// The listening tools over a DAW of Kumi's own: the shared engines and state (`Listening`) over its renderer, one tool
/// call at a time.
pub struct OwnListening<S: RefScheme + 'static> {
    pub renderer: Rc<OwnRenderer<S>>,
    pub listening: Rc<Listening>,
    /// Held for each listening call: a stopped answer's clean-up runs to its end before the next one starts.
    busy: tokio::sync::Mutex<()>,
    /// The scratch copies an earlier session left that the producer was told the bridge removed.
    swept: RefCell<std::collections::HashSet<String>>,
    policy: ListeningPolicy,
}

impl<S: RefScheme + 'static> OwnListening<S> {
    pub fn new(link: Rc<Link<S>>, plan: Rc<OwnPlan<S>>, context: &IntegrationContext, kumi_home: &std::path::Path) -> Rc<Self> {
        let renderer = Rc::new(OwnRenderer::new(link, plan, context, kumi_home));
        let host: Rc<dyn Renderer> = renderer.clone();
        let listening = Listening::new(
            host,
            context.on_judge.clone(),
            context.on_audition.clone(),
            context.listener.clone(),
            context.references.clone(),
            context.search_memory.clone(),
        );
        Rc::new(Self {
            renderer,
            listening,
            busy: tokio::sync::Mutex::new(()),
            swept: RefCell::default(),
            policy: context.policy.listening.clone(),
        })
    }

    /// The bridge removed scratch copies an earlier session left behind (a crash, a quit mid-search): the producer is
    /// told once which, as the project changed without them asking (`scratch_swept`, REAPER's `listening.swept`).
    pub fn swept(&self, data: &Value) {
        let names: Vec<String> = data
            .get("names")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|name| self.swept.borrow_mut().insert((*name).to_owned()))
            .map(|name| format!("“{name}”"))
            .collect();
        if names.is_empty() {
            return;
        }
        let place = self.renderer.profile().words.place;
        let (one, app) = (names.len() == 1, self.renderer.app());
        // (REAPER's undo or redo of the producer's point can bring back copies Kumi had removed: removed again)
        if data.get("why").and_then(Value::as_str) == Some("undo") {
            self.renderer.tell_app(&format!(
                "{app}'s undo brought back Kumi's scratch cop{} {}, which Kumi had removed: removed {} again",
                if one { "y" } else { "ies" },
                names.join(", "),
                if one { "it" } else { "them" }
            ));
            return;
        }
        self.renderer.tell_app(&format!(
            "{app} removed Kumi's scratch cop{} an earlier session left in the {place}: {}",
            if one { "y" } else { "ies" },
            names.join(", ")
        ));
    }

    /// The DAW, for one listening call: once a stopped one has put the DAW back.
    async fn hold(&self, signal: &Signal) -> Result<tokio::sync::MutexGuard<'_, ()>, RuntimeError> {
        if let Ok(held) = self.busy.try_lock() {
            return Ok(held);
        }
        self.renderer.tell_app("Finishing the stopped answer's listening first");
        tokio::select! {
            biased;
            _ = signal.cancelled() => Err(RuntimeError::Aborted),
            held = self.busy.lock() => Ok(held),
        }
    }

    /// Resolves once no listening call runs.
    pub async fn settled(&self) {
        drop(self.busy.lock().await);
    }

    async fn restricted_cancel(&self, id: &str) {
        // An acknowledgment is not a restoration observation. The waiter still polls the job,
        // and the caller must read the touched state after its terminal status.
        let signal = abort::timeout(10_000);
        let _ = tokio::time::timeout(Duration::from_secs(10), self.renderer.link.call("listen_cancel", json!({"jobId": id}), signal)).await;
    }

    async fn restricted_wait(&self, started: &Value, seconds: f64, original: &Signal) -> Result<(Value, bool), RuntimeError> {
        let Some(id) = started.get("jobId").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
            return Err(HearFailure::Unsure {
                code: None,
                reason: "The render may have started, but the bridge returned no job identity.".into(),
                restored: false,
                marked_modified: false,
            }
            .into());
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs_f64((seconds * 4.).max(120.).min(240.));
        let mut cancelled = false;
        loop {
            if original.is_cancelled() && !cancelled {
                cancelled = true;
                self.restricted_cancel(id).await;
            }
            let answer = tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => {
                    self.restricted_cancel(id).await;
                    return Err(HearFailure::Unsure {
                        code: None,
                        reason: "The offline render exceeded its bounded wait; stopping and restoration could not be verified. Observe again.".into(),
                        restored: false, marked_modified: false,
                    }.into());
                }
                _ = original.cancelled(), if !cancelled => continue,
                answer = self.renderer.link.call("listen_job", json!({"jobId": id, "waitMs": 1000}), abort::timeout(5_000)) => answer,
            };
            match answer {
                Ok(Ok(status)) => match status.get("state").and_then(Value::as_str) {
                    Some("done" | "failed" | "cancelled") => return Ok((status, cancelled)),
                    Some("running" | "queued") => {}
                    _ => {
                        self.restricted_cancel(id).await;
                        return Err(HearFailure::Unsure {
                            code: None,
                            reason: "The bridge returned an unknown render state; restoration is unverified.".into(),
                            restored: false,
                            marked_modified: false,
                        }
                        .into());
                    }
                },
                _ => {
                    self.restricted_cancel(id).await;
                    return Err(HearFailure::Unsure {
                        code: None,
                        reason: "The render could not be observed through completion; restoration is unverified. Observe again.".into(),
                        restored: false,
                        marked_modified: false,
                    }
                    .into());
                }
            }
        }
    }

    /// The listening tools the DAW offers now (none while it can't render, or its bridge is older than listening).
    pub fn tools(self: &Rc<Self>) -> Vec<Rc<dyn KernelTool>> {
        let flags = self.renderer.flags();
        if !flags.known || !flags.render || flags.demo {
            return vec![];
        }
        let profile = self.renderer.profile();
        let mut defs =
            vec![("audition", audition_tool(profile), false), ("judge", judge_tool(profile), true), ("tune", tune_tool(profile), true)];
        defs.push(("sound", sound_tool(profile), true));
        defs.push(("form", form_tool(profile), true));
        defs.push(("groove", groove_tool(profile), true));
        if self.renderer.table.no_items.is_none() {
            defs.push(("render", render_tool(profile), true));
        }
        defs.into_iter()
            .map(|(name, def, deferred)| {
                Rc::new(ListeningTool { owner: self.clone(), name, description: def.description, schema: def.schema, deferred })
                    as Rc<dyn KernelTool>
            })
            .collect()
    }

    /// A new answer, not the match run's next round: a judged run's changes count from what's applied now.
    pub fn new_turn(&self, continuing: bool) {
        if !continuing {
            self.listening.new_answer(self.renderer.applied());
            self.renderer.stop_told.set(false);
        }
    }

    /// The producer's words came into the answer under way.
    pub fn steered(&self) {
        self.listening.steered(self.renderer.applied());
    }

    /// An audition (the audition tool's work), as the session's match run asks for it.
    pub async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let _held = self.hold(&signal).await?;
        let auditioned = self.listening.audition(request, signal.clone()).await;
        self.renderer.put_tempo_back().await;
        self.renderer.stop_play_on(&signal);
        auditioned
    }

    /// The audio file a ref stands for (an item's, a sample's, a pattern rendered); None for a file.
    pub async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let file = self.renderer.clip_file(named, signal).await;
        // (a part's audio is a render in Renoise: what it moved goes back as the call ends, as a listen's does)
        self.renderer.put_tempo_back().await;
        file
    }

    /// Whether a goal's rig can be opened now (the bridge renders and makes scratch copies).
    pub fn goal_ready(&self) -> bool {
        let flags = self.renderer.flags();
        flags.known && flags.render && flags.scratch && !flags.demo
    }

    /// A goal's render rig over these candidates (`/goal`, a match run's search), or why not.
    pub async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        let _held = self.hold(&signal).await?;
        crate::goal::OwnGoalRig::open(self.renderer.clone(), &self.listening, request, signal).await
    }

    /// Pure preflight: no listening hold, job, scratch access, audition or mutation.
    pub async fn preview_hear(&self, request: &HearRequest, signal: Signal) -> Result<HearPreview, RuntimeError> {
        self.restricted_preflight(request, signal).await.map(|checked| checked.preview)
    }

    async fn render_preflight_read(&self, window: Window, epoch: &str, signal: Signal) -> Result<Value, RuntimeError> {
        let read = self
            .renderer
            .link
            .call(
                "read",
                json!({
                    "kind": "render_preflight", "range": {"fromBeat": window.from, "beats": window.beats}, "epoch": epoch,
                }),
                signal,
            )
            .await?
            .map_err(|refused| native_hear_refused(refused.code.as_deref(), "unsupported", self.renderer.profile().id))?;
        if read["schema"].as_u64() != Some(1) || !render_state_complete(self.renderer.profile().id, &read["render_state"]) {
            return Err(hear_refused("unsupported", "The bridge did not provide a complete schema-1 render-state snapshot."));
        }
        Ok(read)
    }

    async fn restricted_preflight(&self, request: &HearRequest, signal: Signal) -> Result<RestrictedPreflight, RuntimeError> {
        signal.check()?;
        let window = bounded_hear_window(request)?;
        let renderer = &self.renderer;
        let status = renderer
            .link
            .call("status", json!({}), signal.clone())
            .await?
            .map_err(|refused| native_hear_refused(refused.code.as_deref(), "render_unavailable", renderer.profile().id))?;
        let flags = Flags::of(&status);
        if !flags.known || !flags.render || flags.demo {
            return Err(hear_refused("render_unavailable", "The bridge does not support offline rendering now."));
        }
        if status["busy"] != false && !status["busy"].is_null() {
            return Err(hear_refused("render_busy", "The DAW is busy; no render was started."));
        }
        let focus = renderer
            .link
            .call("focus", json!({}), signal.clone())
            .await?
            .map_err(|refused| native_hear_refused(refused.code.as_deref(), "transport_unknown", renderer.profile().id))?;
        bounded_hear_transport(&focus["transport"])?;
        let epoch = focus["epoch"]
            .as_str()
            .filter(|epoch| !epoch.is_empty())
            .ok_or_else(|| hear_refused("stale_ref", "The bridge did not establish the current project epoch."))?
            .to_owned();
        if matches!(self.policy, ListeningPolicy::Restricted { .. }) && request.expected_native_epoch.as_deref() != Some(epoch.as_str()) {
            return Err(hear_refused("stale", "The capture is not bound to this observed project epoch; observe again before listening."));
        }
        let (label, source) = if let Some(reference) = request.tracks.first() {
            let long = renderer
                .plan
                .lengthen(reference)
                .map_err(|_| hear_refused("stale_ref", "The track reference is not current; observe again."))?;
            if renderer.link.book.borrow().scheme.kind_of(&long) != Some("track") {
                return Err(hear_refused("invalid_source", "Restricted listening accepts a fresh track ref, never a track name."));
            }
            if renderer.link.seen_in.borrow().get(&long).map(String::as_str) != Some(epoch.as_str()) {
                return Err(hear_refused("stale_ref", "The track was not observed in the current project epoch; observe again."));
            }
            let read = renderer
                .link
                .call("read", renderer.read_placed(json!({"ref": long})), signal.clone())
                .await?
                .map_err(|refused| native_hear_refused(refused.code.as_deref(), "stale_ref", renderer.profile().id))?;
            let row = read["items"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["ref"].as_str() == Some(long.as_str())))
                .ok_or_else(|| hear_refused("stale_ref", "The track is no longer present; observe again."))?;
            if row["type"] == "master" || row["master"] == true {
                return Err(hear_refused("invalid_source", "Use mix: true for the master output."));
            }
            let label =
                row["name"].as_str().ok_or_else(|| hear_refused("unsupported", "The bridge did not provide the current track name."))?;
            (label.to_owned(), json!({"id": "bounded", "what": "track", "ref": long, "stage": "pre-fader"}))
        } else {
            ("The whole mix".to_owned(), json!({"id": "bounded", "what": "master"}))
        };
        if status.pointer("/listening/render_preflight") != Some(&Value::Bool(true)) {
            return Err(hear_refused("unsupported", format!(
                "This {} bridge does not expose read(kind=render_preflight) schema 1, required to prove bounded duration and restoration.", renderer.app()
            )));
        }
        let focus = focus_state(focus)?;
        let mut read = self.render_preflight_read(window, &epoch, signal.clone()).await?;
        let seconds = exact_render_seconds(&read, request.approval)?;
        signal.check()?;
        Ok(RestrictedPreflight {
            preview: HearPreview {
                label,
                seconds,
                method: HearMethod::Offline,
                deadline_seconds: render_deadline_seconds(seconds),
                disclosure: match renderer.profile().id {
                    Some(DawId::Reaper) => "REAPER may show the project as modified after a listen.",
                    Some(DawId::Renoise) => "Renoise may change BPM/LPB. It attempts restoration only from this render's planned end pair; a different pair is left unchanged. An unverified restoration, including cancellation, can leave tempo changed.",
                    _ => "",
                },
            },
            window,
            source,
            epoch,
            state: read.get_mut("render_state").expect("validated preflight state").take(),
            focus,
        })
    }

    /// A terminal job is not restoration evidence. Native completion owns restoration;
    /// the adapter only reads it back and never overwrites a newer producer tempo.
    async fn restricted_restore(&self, before: &RestrictedPreflight) -> Result<bool, RuntimeError> {
        let cleanup = async {
            let renderer = &self.renderer;
            // A temporary LPB change can move the approved interval past the song's end.
            // This pure state-only probe always fits its first line; it renders nothing.
            let probe = Window { from: 0., beats: 1. / 256. };
            let after = self.render_preflight_read(probe, &before.epoch, abort::timeout(15_000)).await?;
            let marked_modified = restored_state(renderer.profile().id, &before.state, &after["render_state"])?;
            let status = renderer
                .link
                .call("status", json!({}), abort::timeout(5_000))
                .await?
                .map_err(|_| hear_refused("render_unavailable", "The DAW's rendering state could not be read."))?;
            if status["busy"] != false {
                return Err(HearFailure::Unsure {
                    code: None,
                    reason: "The DAW still reports busy after the render; restoration is unverified.".into(),
                    restored: false,
                    marked_modified: false,
                }
                .into());
            }
            let focus = renderer
                .link
                .call("focus", json!({}), abort::timeout(5_000))
                .await?
                .map_err(|_| hear_refused("transport_unknown", "The transport could not be read after rendering."))?;
            bounded_hear_transport(&focus["transport"])?;
            if focus_state(focus)? != before.focus {
                return Err(HearFailure::Unsure {
                    code: None,
                    reason: "Transport or selection differs from the before-render snapshot. Observe again.".into(),
                    restored: false,
                    marked_modified: false,
                }
                .into());
            }
            Ok::<bool, RuntimeError>(marked_modified)
        };
        match tokio::time::timeout(Duration::from_secs(60), cleanup).await {
            Ok(Ok(marked)) => Ok(marked),
            _ => Err(HearFailure::Unsure {
                code: None,
                reason: "The render ended, but complete restoration could not be verified. Observe again before another action.".into(),
                restored: false,
                marked_modified: false,
            }
            .into()),
        }
    }

    async fn restricted_discard(&self, done: &Value, scratch: &std::path::Path, quarantine_root: &std::path::Path) {
        let Some(files) = done["files"].as_array() else { return };
        if files.is_empty() {
            return;
        }
        if files.len() != 1 || files[0]["id"] != "bounded" {
            self.renderer.tell_app("The bridge returned unexpected staging files; they were left untouched.");
            return;
        }
        let Some(path) = files[0]["file"].as_str() else { return };
        #[cfg(windows)]
        let result = {
            let (cache, scratch, quarantine, returned) =
                (self.renderer.folder.clone(), scratch.to_path_buf(), quarantine_root.to_path_buf(), PathBuf::from(path));
            tokio::task::spawn_blocking(move || restricted_staging::discard(&cache, &scratch, &quarantine, &returned))
                .await
                .unwrap_or_else(|_| Err(hear_refused("unsafe_render_file", "The staging cleanup worker did not finish.")))
        };
        #[cfg(not(windows))]
        let result = restricted_staging::discard(&self.renderer.folder, scratch, quarantine_root, std::path::Path::new(path));
        match result {
            Ok(Some(warning)) => self.renderer.tell_app(&warning),
            Ok(None) => {}
            Err(_) => self.renderer.tell_app("This call's bridge staging file could not be safely discarded; it was left untouched."),
        }
    }

    async fn restricted_hear(&self, request: &HearRequest, original: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let _held = self.hold(&original).await?;
        let before = self.restricted_preflight(request, original.clone()).await?;
        let approval = request
            .approval
            .ok_or_else(|| hear_refused("not_approved", "A restricted render needs the host's approved window and native deadline."))?;
        let scratch = self.policy.scratch_dir(request).await?;
        let ListeningPolicy::Restricted { quarantine_root, .. } = &self.policy else {
            return Err(hear_refused("unsupported", "Restricted listening requires an explicit host policy."));
        };
        #[cfg(windows)]
        {
            let cache = self.renderer.folder.clone();
            tokio::task::spawn_blocking(move || restricted_staging::check_cache(&cache))
                .await
                .map_err(|_| hear_refused("unsafe_render_file", "The staging directory check did not finish."))??;
        }
        #[cfg(not(windows))]
        restricted_staging::check_cache(&self.renderer.folder)?;
        if self.renderer.flag.is_on() {
            return Err(hear_refused("listening_busy", "Kumi is already listening; wait for it to settle."));
        }
        original.check()?;
        let _listening = self.renderer.flag.begin();
        // Do not cancel the start RPC with the caller: retain its job identity so a cancellation
        // during dispatch can still stop and settle that exact job. There is no busy retry.
        let mut args = json!({
            "sources": [before.source], "range": {"fromBeat": before.window.from, "beats": before.window.beats}, "preRollBeats": 0,
            "maxSeconds": approval.seconds,
        });
        if self.renderer.profile().id == Some(DawId::Renoise) {
            if let Some(deadline) = request.native_deadline_unix_ms {
                args["deadlineUnixMs"] = deadline.into();
            }
        }
        // Preserve the normal layout placement, but never let the focus watcher's mutable
        // bookkeeping replace the exact observation epoch that this capture validated.
        let input = args.as_object().expect("render arguments are an object").clone();
        self.renderer.plan.place_request(&mut args, &input);
        args["epoch"] = json!(before.epoch);
        let started = match self.renderer.link.call("render", args, abort::timeout(10_000)).await {
            Ok(Ok(started)) => started,
            Ok(Err(refused)) if start_refused_before_mutation(self.renderer.profile().id, refused.code.as_deref()) => {
                return Err(native_hear_refused(refused.code.as_deref(), "render_unavailable", self.renderer.profile().id));
            }
            failed => {
                let native = match &failed {
                    Ok(Err(refused)) => refused.code.as_deref(),
                    _ => None,
                };
                return Err(keep_native_tempo_failure(
                    HearFailure::Unsure {
                        code: None,
                        reason: "The render start could not be resolved; its effects and restoration are unknown. Its staging file may remain in the owning Kumi listening cache. Observe again.".into(),
                        restored: false, marked_modified: false,
                    }.into(),
                    native,
                    self.renderer.profile().id,
                ));
            }
        };
        let (done, cancelled) = match self.restricted_wait(&started, before.preview.seconds, &original).await {
            Ok(ended) => ended,
            Err(mut error) => {
                // One independent, bounded observation after cancellation may now name the
                // completed file. Never infer a filename from the start response or sweep.
                let observed = if let Some(id) = started["jobId"].as_str() {
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        self.renderer.link.call("listen_job", json!({"jobId": id, "waitMs": 1000}), abort::timeout(5_000)),
                    )
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .and_then(Result::ok)
                } else {
                    None
                };
                if let Some(done) = observed.filter(|done| matches!(done["state"].as_str(), Some("done" | "failed" | "cancelled"))) {
                    self.restricted_discard(&done, &scratch, quarantine_root).await;
                    if let Ok(marked_modified) = self.restricted_restore(&before).await {
                        return Err(keep_native_tempo_failure(
                            HearFailure::Unsure {
                                code: None,
                                reason: "The render could not be observed normally; a later read-back confirmed restoration.".into(),
                                restored: true,
                                marked_modified,
                            }
                            .into(),
                            done["code"].as_str(),
                            self.renderer.profile().id,
                        ));
                    }
                    error = keep_native_tempo_failure(error, done["code"].as_str(), self.renderer.profile().id);
                } else {
                    self.renderer.tell_app("The render's terminal state remains unknown. Its per-call staging file may remain in the owning Kumi listening cache; no cache sweep was performed.");
                    if let RuntimeError::Hear(HearFailure::Unsure { reason, .. }) = &mut error {
                        reason.push_str(
                            " This call's staging file may remain in the owning Kumi listening cache; no cache sweep was performed.",
                        );
                    }
                }
                return Err(error);
            }
        };
        // REAPER's post-FX render reports `stale` only from its pre-execution guards, before touching DAW
        // state. Comparing its old focus now would mistake the producer's new tempo for
        // restoration residue. Renoise can also report stale after a render has started.
        if self.renderer.profile().id == Some(DawId::Reaper) && done["state"] == "failed" && done["code"] == "stale" {
            // This refusal precedes native mutation, but only a fresh touched-state read can
            // establish cleanup. Do not compare or restore the producer's newer tempo/focus.
            let after = self
                .render_preflight_read(Window { from: 0., beats: 1. / 256. }, &before.epoch, abort::timeout(15_000))
                .await
                .map_err(|_| {
                    RuntimeError::from(HearFailure::Unsure {
                        code: None,
                        reason: "The queued render was refused, but its touched state could not be read back. Observe again.".into(),
                        restored: false,
                        marked_modified: false,
                    })
                })?;
            let marked_modified = restored_state(self.renderer.profile().id, &before.state, &after["render_state"])?;
            return Err(after_restoration(
                hear_refused("stale", "The queued render's window or project changed before it started; preview again."),
                marked_modified,
            ));
        }
        let restoration = self
            .restricted_restore(&before)
            .await
            .and_then(|marked_modified| {
                if native_tempo_failure(done["code"].as_str(), self.renderer.profile().id).is_some() {
                    Err(after_restoration(
                        native_hear_refused(done["code"].as_str(), "render_failed", self.renderer.profile().id),
                        marked_modified,
                    ))
                } else {
                    Ok(marked_modified)
                }
            })
            .map_err(|error| keep_native_tempo_failure(error, done["code"].as_str(), self.renderer.profile().id));
        if restoration.is_err() || cancelled || original.is_cancelled() || done["state"] == "cancelled" {
            self.restricted_discard(&done, &scratch, quarantine_root).await;
            let marked_modified = restoration?;
            return Err(HearFailure::Cancelled { marked_modified }.into());
        }
        let marked_modified = restoration?;
        if done["state"] != "done" {
            self.restricted_discard(&done, &scratch, quarantine_root).await;
            return Err(after_restoration(
                native_hear_refused(done["code"].as_str(), "render_failed", self.renderer.profile().id),
                marked_modified,
            ));
        }
        let files = done["files"].as_array().filter(|files| files.len() == 1).ok_or_else(|| {
            after_restoration(
                hear_refused("invalid_render_result", "The completed render did not return exactly this call's one source."),
                marked_modified,
            )
        })?;
        let file = &files[0];
        if file["id"] != "bounded" || file.get("start").is_some_and(|start| start.as_f64() != Some(0.)) {
            return Err(after_restoration(
                hear_refused("invalid_render_result", "The render returned a different source or an unapproved leading interval."),
                marked_modified,
            ));
        }
        let returned = file["file"]
            .as_str()
            .ok_or_else(|| after_restoration(hear_refused("invalid_render_result", "The render returned no file."), marked_modified))?;
        #[cfg(windows)]
        let staged = {
            let (cache, scratch, quarantine, returned, signal) =
                (self.renderer.folder.clone(), scratch.clone(), quarantine_root.clone(), PathBuf::from(returned), original.clone());
            tokio::task::spawn_blocking(move || restricted_staging::copy(&cache, &scratch, &quarantine, &returned, &signal))
                .await
                .unwrap_or_else(|_| Err(hear_refused("unsafe_render_file", "The staging copy worker did not finish.")))
        };
        #[cfg(not(windows))]
        let staged = restricted_staging::copy(&self.renderer.folder, &scratch, quarantine_root, std::path::Path::new(returned), &original);
        let staged = staged.map_err(|error| after_restoration(error, marked_modified))?;
        original.check().map_err(|error| after_restoration(error.into(), marked_modified))?;
        let mut notes = vec![];
        if staged.seconds + 0.001 < before.preview.seconds {
            notes.push(format!("The render is short: {:.3} of {:.3} requested seconds.", staged.seconds, before.preview.seconds));
        }
        if let Some(warning) = staged.warning {
            self.renderer.tell_app(&warning);
            notes.push(warning);
        }
        Ok(Ok(vec![HeardTake {
            label: before.preview.label,
            file: staged.path.to_string_lossy().into_owned(),
            start: 0.,
            seconds: Some(staged.seconds),
            live: false,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
            marked_modified,
        }]))
    }

    /// The listen tool's hearing of the project: what plays now (REAPER, while it plays), or a stretch rendered offline
    /// and played out loud after.
    pub async fn hear(&self, request: &HearRequest, original: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        if matches!(self.policy, ListeningPolicy::Restricted { .. }) {
            return self.restricted_hear(request, original).await;
        }
        let heard = self.hear_now(request, original.clone()).await;
        self.renderer.put_tempo_back().await;
        self.renderer.stop_play_on(&original);
        heard
    }

    async fn hear_now(&self, request: &HearRequest, original: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let renderer = &self.renderer;
        if let Some(why) = renderer.why_not() {
            return Ok(Err(why));
        }
        let Some(tempo) = renderer.tempo().filter(|tempo| *tempo > 0.) else {
            return Ok(Err(format!("Kumi doesn't know the {}'s tempo yet; try again.", renderer.profile().words.place)));
        };
        let _held = self.hold(&original).await?;
        let signal = abort::any([original, renderer.lifetime()]);
        if renderer.flag.is_on() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let _listening = renderer.flag.begin();
        let focus = renderer.link.call("focus", json!({}), signal.clone()).await?.ok().unwrap_or(Value::Null);
        let transport = focus.get("transport").cloned().unwrap_or(Value::Null);
        let playing = transport.get("playing") == Some(&Value::Bool(true));
        let app = renderer.app();
        // While REAPER plays, a listen with no place hears what it plays (the master output).
        if playing && request.from_beat.is_none() && request.whole != Some(true) && renderer.flags().capture {
            if request.mix != Some(true) {
                return Ok(Err(format!(
                    "While {app} plays, Kumi hears the master output as it plays (mix: true); give from_beat and beats to hear tracks on their own (Kumi renders them)."
                )));
            }
            return self.capture(request.seconds.unwrap_or(8.).clamp(1., 60.), &signal).await;
        }
        // A DAW that can't hear what it plays stops playing to render (Renoise does): a listen with no place doesn't stop
        // the producer's playback.
        if playing && request.from_beat.is_none() && request.whole != Some(true) {
            let why = if renderer.table.captures {
                format!("Kumi can't hear what {app} plays just now (its audio isn't running for Kumi)")
            } else {
                format!("{app}'s scripts can't hear what it plays")
            };
            return Ok(Err(format!(
                "{why}, and a render stops its playback: give from_beat and beats (or stop {app}), and Kumi renders the part."
            )));
        }
        let looped = transport.get("loop").filter(|lp| lp.get("on") == Some(&Value::Bool(true)));
        let window = if request.whole == Some(true) {
            let Some(end) = renderer.song_end(signal.clone()).await else {
                return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into()));
            };
            Window { from: 0., beats: end }
        } else {
            let (loop_from, loop_to) = (
                looped.and_then(|lp| lp.get("start")).and_then(Value::as_f64),
                looped.and_then(|lp| lp.get("end")).and_then(Value::as_f64),
            );
            let from = request.from_beat.or(loop_from).or_else(|| transport.get("beat").and_then(Value::as_f64)).unwrap_or(0.).max(0.);
            let looped_beats = loop_from.zip(loop_to).map(|(start, end)| end - start).filter(|beats| *beats > 0.);
            let beats = request.beats.or(looped_beats).unwrap_or(4. * renderer.beats_per_bar.get());
            Window { from, beats }
        };
        if let Some(why) = kumi_listening::engine::too_long(window.beats, tempo, "listen in parts") {
            return Ok(Err(why));
        }
        let candidates: Vec<AuditionCandidate> = if request.mix == Some(true) || request.tracks.is_empty() {
            vec![AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label: Some("The whole mix".into()), clip: None }]
        } else {
            request.tracks.iter().map(|track| AuditionCandidate { track: track.clone(), mix: None, label: None, clip: None }).collect()
        };
        // Said by the tracks' names, whatever named them (a ref, mostly).
        let mut heard: Vec<String> = vec![];
        for candidate in &candidates {
            heard.push(match candidate.mix {
                Some(true) => "the mix".into(),
                _ => renderer.resolve(&candidate.track, &signal).await?.map_or_else(|_| candidate.track.clone(), |(_, name)| name),
            });
        }
        renderer.tell_app(&format!("Rendering {} · listening to {}", renderer.bars(window), heard.join(", ")));
        let pass = renderer.hear_candidates(&candidates, window, false, &signal).await;
        renderer.tell_app("Listened");
        let note = |notes: &[String]| (!notes.is_empty()).then(|| notes.join(" "));
        match pass.rendered {
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
            Ok(captured) => {
                let mut notes = pass.notes;
                if pass.audible {
                    notes.push(format!("{app} rendered it offline; Kumi plays it out loud now."));
                }
                // Only the first is played out loud: the others say so.
                let label = |name: &String, mix: bool| if mix { "The whole mix".to_string() } else { name.clone() };
                let first =
                    captured.sources.iter().find(|(name, _)| captured.files.contains_key(name)).map(|(name, mix)| label(name, *mix));
                let takes: Vec<HeardTake> = captured
                    .sources
                    .iter()
                    .filter_map(|(name, mix)| {
                        let label = label(name, *mix);
                        let said = match &first {
                            Some(first) if *first != label && pass.audible => {
                                Some(format!("{app} rendered it offline; the producer heard only {first} out loud."))
                            }
                            _ => note(&notes),
                        };
                        captured.files.get(name).map(|render| HeardTake {
                            label,
                            file: render.file.clone(),
                            start: render.start,
                            seconds: Some(render.seconds.unwrap_or(window.beats * 60. / tempo)),
                            live: false,
                            note: said,
                            marked_modified: false,
                        })
                    })
                    .collect();
                if takes.is_empty() {
                    let timeline = renderer.profile().words.timeline;
                    return Ok(Err(format!("Nothing came through. Is something playing there in the {timeline}?")));
                }
                Ok(Ok(takes))
            }
        }
    }

    /// What REAPER plays, captured as it plays for `seconds`.
    async fn capture(&self, seconds: f64, signal: &Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let renderer = &self.renderer;
        renderer.tell_app(&format!("Listening to the mix as it plays ({} s)", seconds.round()));
        let started = match renderer.link.call("hear", json!({"seconds": seconds}), signal.clone()).await? {
            Ok(started) => started,
            Err(refused) => return Ok(Err(refused.why)),
        };
        let done = match renderer.wait(&started, signal).await? {
            Ok(done) => done,
            Err(why) => return Ok(Err(why)),
        };
        renderer.tell_app("Listened");
        let Some(file) = done.pointer("/files/0") else { return Ok(Err(format!("{} captured nothing", renderer.app()))) };
        let held = file.get("seconds").and_then(Value::as_f64);
        let partial = file.get("partial") == Some(&Value::Bool(true));
        let mut notes = notes_of(&done);
        // (the bridge says itself when it stopped early)
        if partial && notes.is_empty() {
            notes.push(format!(
                "{} stopped before the end: {} s were heard.",
                renderer.app(),
                held.map_or("a few".into(), |s| format!("{:.1}", s))
            ));
        }
        let path = text(file, "file").unwrap_or_default();
        let rate = file.get("sampleRate").and_then(Value::as_f64).filter(|rate| *rate > 0.).unwrap_or(48_000.);
        let stretches = stretches(done.get("marks"), rate, held, renderer.beats_per_bar.get());
        if stretches.len() <= 1 {
            let (start, seconds) = stretches.first().map_or((0., held), |(start, seconds, _)| (*start, Some(*seconds)));
            let note = (!notes.is_empty()).then(|| notes.join(" "));
            return Ok(Ok(vec![HeardTake {
                label: "The whole mix".into(),
                file: path,
                start,
                seconds,
                live: true,
                note,
                marked_modified: false,
            }]));
        }
        // The playhead jumped (a loop, a seek): what's measured is the longest stretch it played straight through (the
        // stretches aren't parts heard at once).
        // (the first of the longest)
        let (start, seconds, label) =
            stretches.iter().cloned().reduce(|best, next| if next.1 > best.1 + 1e-9 { next } else { best }).unwrap();
        notes.push(format!(
            "{} jumped while Kumi listened ({}): this is the longest stretch it played straight through, {:.1} s.",
            renderer.app(),
            stretches.iter().map(|(_, _, label)| label.as_str()).collect::<Vec<_>>().join(", "),
            seconds
        ));
        Ok(Ok(vec![HeardTake {
            label,
            file: path,
            start,
            seconds: Some(seconds),
            live: true,
            note: Some(notes.join(" ")),
            marked_modified: false,
        }]))
    }
}

/// One listening tool (audition, judge, tune, sound, form, render) as the model calls it.
struct ListeningTool<S: RefScheme + 'static> {
    owner: Rc<OwnListening<S>>,
    name: &'static str,
    description: String,
    schema: JsonObject,
    deferred: bool,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> KernelTool for ListeningTool<S> {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        self.schema.clone()
    }
    fn deferred(&self) -> bool {
        self.deferred
    }
    fn reach(&self) -> Reach {
        Reach::DawWrite
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let _held = self.owner.hold(&signal).await?;
        let result = match self.owner.listening.call(self.name, input, signal.clone()).await {
            Some(result) => result,
            None => Ok(ToolResult::error(format!("{} isn't a listening tool", self.name))),
        };
        self.owner.renderer.put_tempo_back().await;
        self.owner.renderer.stop_play_on(&signal);
        result
    }
}

/// `ask`'s answer, asked again while the DAW refuses as busy, up to `BUSY_WAIT`: still busy then, that's the answer. `waiting`
/// hears why, once. Any other answer comes at once, and a stop ends the wait. That includes the host's own word that the DAW
/// didn't answer in time, which is busy with no code: that's no refusal, and what was asked may have started.
async fn while_busy<F>(signal: &Signal, waiting: impl Fn(&str), mut ask: impl FnMut() -> F) -> Result<Result<Value, Refused>, RuntimeError>
where
    F: std::future::Future<Output = Result<Result<Value, Refused>, RuntimeError>>,
{
    let began = tokio::time::Instant::now();
    let told = Cell::new(false);
    loop {
        match ask().await? {
            Err(refused) if refused.there && refused.code.as_deref() == Some("busy") && began.elapsed() < BUSY_WAIT => {
                if !told.replace(true) {
                    waiting(&refused.why);
                }
                tokio::select! {
                    biased;
                    _ = signal.cancelled() => return Err(RuntimeError::Aborted),
                    _ = tokio::time::sleep(BUSY_PACE) => {}
                }
            }
            answer => return Ok(answer),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn restricted_windows_require_one_source_and_explicit_finite_bounds() {
        use super::bounded_hear_window;
        use kumi_types::contracts::HearRequest;
        let request = HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() };
        assert!(bounded_hear_window(&request).is_ok());
        for (from_beat, beats) in [
            (None, Some(8.)),
            (Some(0.), None),
            (Some(-1.), Some(8.)),
            (Some(f64::NAN), Some(8.)),
            (Some(f64::INFINITY), Some(8.)),
            (Some(0.), Some(f64::NAN)),
            (Some(0.), Some(f64::INFINITY)),
            (Some(0.), Some(0.)),
            (Some(0.), Some(-1.)),
            (Some(0.), Some(257.)),
        ] {
            assert!(bounded_hear_window(&HearRequest { from_beat, beats, ..request.clone() }).is_err(), "{from_beat:?} {beats:?}");
        }
        for invalid in [
            HearRequest { mix: None, ..request.clone() },
            HearRequest { tracks: vec!["t1".into()], ..request.clone() },
            HearRequest { mix: None, tracks: vec!["t1".into(), "t2".into()], ..request.clone() },
            HearRequest { whole: Some(true), ..request.clone() },
            HearRequest { seconds: Some(1.), ..request.clone() },
        ] {
            assert!(bounded_hear_window(&invalid).is_err(), "{invalid:?}");
        }
        assert!(bounded_hear_window(&HearRequest { mix: None, tracks: vec!["t1".into()], ..request }).is_ok());
    }

    #[test]
    fn restricted_transport_requires_known_stopped_state_and_positive_tempo() {
        use super::bounded_hear_transport;
        let stopped = json!({"playing": false, "recording": false, "tempo": 120.});
        assert!(bounded_hear_transport(&stopped).is_ok());
        // Duration comes from the native tempo map, not this instantaneous BPM.
        assert!(bounded_hear_transport(&json!({"playing": false, "recording": false, "tempo": 1.})).is_ok());
        for field in ["playing", "recording", "paused"] {
            let mut transport = stopped.clone();
            transport[field] = true.into();
            assert!(bounded_hear_transport(&transport).is_err(), "{field}");
        }
        for field in ["playing", "recording", "tempo"] {
            let mut transport = stopped.clone();
            transport.as_object_mut().unwrap().remove(field);
            assert!(bounded_hear_transport(&transport).is_err(), "{field}");
        }
        for tempo in [0., -1.] {
            let mut transport = stopped.clone();
            transport["tempo"] = tempo.into();
            assert!(bounded_hear_transport(&transport).is_err(), "{tempo}");
        }
    }

    #[test]
    fn restricted_exact_duration_rejects_hidden_preroll_irregular_clipped_and_long_audio() {
        use super::exact_render_seconds;
        let exact = json!({"schema": 1, "seconds": 60., "offset": 0., "aligned": true, "irregular": false, "clipped": false});
        assert_eq!(exact_render_seconds(&exact, None).unwrap(), 60.);
        for (key, value) in [
            ("seconds", json!(60.001)),
            ("seconds", json!(0.)),
            ("seconds", Value::Null),
            ("offset", json!(0.001)),
            ("aligned", json!(false)),
            ("irregular", json!(true)),
            ("clipped", json!(true)),
            ("schema", json!(2)),
        ] {
            let mut invalid = exact.clone();
            invalid[key] = value;
            assert!(exact_render_seconds(&invalid, None).is_err(), "{invalid}");
        }
        for key in ["schema", "seconds", "offset", "aligned", "irregular", "clipped"] {
            let mut incomplete = exact.clone();
            incomplete.as_object_mut().unwrap().remove(key);
            assert!(exact_render_seconds(&incomplete, None).is_err(), "{key}");
        }
    }

    #[test]
    fn verified_restoration_preserves_cleanup_and_modified_facts_on_non_success() {
        use super::{after_restoration, hear_refused, HearFailure, RuntimeError};
        for marked_modified in [false, true] {
            assert!(matches!(
                after_restoration(RuntimeError::Aborted, marked_modified),
                RuntimeError::Hear(HearFailure::Cancelled { marked_modified: marked }) if marked == marked_modified
            ));
            assert!(matches!(
                after_restoration(hear_refused("render_failed", "The render failed."), marked_modified),
                RuntimeError::Hear(HearFailure::Refused { marked_modified: marked, .. }) if marked == marked_modified
            ));
            let overrun = HearFailure::Unsure {
                code: None,
                reason: "The rendered audio exceeded its cap.".into(),
                restored: false,
                marked_modified: false,
            }
            .into();
            assert!(matches!(
                after_restoration(overrun, marked_modified),
                RuntimeError::Hear(HearFailure::Unsure { restored: true, marked_modified: marked, .. }) if marked == marked_modified
            ));
        }
    }

    #[test]
    fn a_matching_later_snapshot_does_not_override_native_restoration_failure() {
        use super::{after_restoration, native_hear_refused, DawId, HearFailure, RuntimeError};
        let error = native_hear_refused(Some("tempo_restore_failed"), "render_failed", Some(DawId::Renoise));
        assert!(matches!(
            after_restoration(error, false),
            RuntimeError::Hear(HearFailure::Unsure { code: Some("tempo_restore_failed"), restored: false, .. })
        ));
    }

    #[test]
    fn restricted_native_failures_keep_typed_codes_without_native_text() {
        use super::{native_hear_refused, start_refused_before_mutation, DawId, HearFailure, RuntimeError};
        for (native, expected) in [
            ("stale", "stale"),
            ("changed", "stale"),
            ("gone", "stale_ref"),
            ("busy", "render_busy"),
            ("bad-params", "invalid_window"),
            ("failed", "render_failed"),
            ("/private/producer/cache.wav", "render_failed"),
        ] {
            for daw in [DawId::Reaper, DawId::Renoise] {
                let RuntimeError::Hear(HearFailure::Refused { code, reason, .. }) =
                    native_hear_refused(Some(native), "render_failed", Some(daw))
                else {
                    panic!("typed refusal")
                };
                assert_eq!(code, expected);
                assert!(!reason.contains("/private/"));
            }
        }
        for code in ["stale", "busy", "bad-params", "failed", "gone"] {
            assert!(start_refused_before_mutation(Some(DawId::Reaper), Some(code)));
        }
        for daw in [DawId::Reaper, DawId::Renoise] {
            for code in [None, Some("timeout"), Some("uncertain"), Some("disconnected")] {
                assert!(!start_refused_before_mutation(Some(daw), code));
            }
        }
        assert!(!start_refused_before_mutation(Some(DawId::Renoise), Some("failed")));
        for code in ["tempo_restore_state_changed", "tempo_restore_ownership_uncertain", "tempo_restore_failed"] {
            assert!(
                matches!(
                    native_hear_refused(Some(code), "render_failed", Some(DawId::Renoise)),
                    RuntimeError::Hear(HearFailure::Unsure { code: Some(known), restored: false, .. }) if known == code
                ),
                "a tempo restoration failure is uncertainty, not a safe refusal"
            );
            assert!(
                matches!(
                    native_hear_refused(Some(code), "render_failed", Some(DawId::Reaper)),
                    RuntimeError::Hear(HearFailure::Refused { code: "render_failed", .. })
                ),
                "another DAW cannot opt into Renoise's failure meanings"
            );
        }
        let RuntimeError::Hear(HearFailure::Refused { reason, .. }) =
            native_hear_refused(Some("busy"), "unsupported", Some(DawId::Renoise))
        else {
            panic!("typed refusal")
        };
        assert!(reason.contains("Edit Mode"));
    }

    #[test]
    fn restricted_restoration_compares_every_snapshot_field_but_reports_reaper_dirty_residue() {
        use super::{restored_state, DawId, HearFailure, RuntimeError, RENDER_NUMBERS, RENDER_TEXTS};
        let numbers: serde_json::Map<String, Value> = RENDER_NUMBERS.iter().map(|key| ((*key).into(), json!(0.))).collect();
        let texts: serde_json::Map<String, Value> = RENDER_TEXTS.iter().map(|key| ((*key).into(), json!("before"))).collect();
        let before = json!({
            "numbers": numbers, "texts": texts, "projrenderlimit": 0,
            "selection": [{"ref": "master", "value": 0.}, {"ref": "track", "value": 1.}],
            "mutes": [{"ref": "master", "value": 0.}, {"ref": "track", "value": 0.}],
            "master_fx": 1., "time_selection": [0., 4.], "dirty": false,
        });
        assert!(!restored_state(Some(DawId::Reaper), &before, &before).unwrap());
        let mut dirty = before.clone();
        dirty["dirty"] = true.into();
        assert!(restored_state(Some(DawId::Reaper), &before, &dirty).unwrap());
        for pointer in [
            "/numbers/RENDER_SETTINGS",
            "/numbers/RENDER_STARTPOS",
            "/texts/RENDER_FILE",
            "/projrenderlimit",
            "/selection/0/value",
            "/mutes/1/value",
            "/master_fx",
            "/time_selection/1",
        ] {
            let mut changed = before.clone();
            *changed.pointer_mut(pointer).unwrap() = json!(99);
            assert!(
                matches!(restored_state(Some(DawId::Reaper), &before, &changed), Err(RuntimeError::Hear(HearFailure::Unsure { .. }))),
                "{pointer}"
            );
        }
        let renoise = json!({"bpm": 120., "lpb": 4, "tracks": [{"ref": "track", "mute": 1, "solo": false}]});
        assert!(!restored_state(Some(DawId::Renoise), &renoise, &renoise).unwrap());
        for pointer in ["/bpm", "/lpb", "/tracks/0/mute", "/tracks/0/solo"] {
            let mut changed = renoise.clone();
            *changed.pointer_mut(pointer).unwrap() = json!(2);
            assert!(
                matches!(restored_state(Some(DawId::Renoise), &renoise, &changed), Err(RuntimeError::Hear(HearFailure::Unsure { .. }))),
                "{pointer}"
            );
        }
    }

    #[test]
    fn only_an_enabled_vst3_limiter_identity_satisfies_the_existing_chain() {
        let mut device = super::DeviceRow {
            reference: "fx1".into(),
            name: "Producer name".into(),
            own_name: "ignored".into(),
            identity: "VST3: Ozone 12 Maximizer (iZotope)".into(),
            on: true,
            plugin: true,
            instrument: false,
            index: 0,
            mixer: false,
        };
        assert!(super::verified_plugin_limiter(&device));
        device.on = false;
        assert!(!super::verified_plugin_limiter(&device));
        device.on = true;
        device.name = "Ozone 12 Maximizer".into();
        device.identity = "VST3: Serum 2 (Xfer Records)".into();
        assert!(!super::verified_plugin_limiter(&device));
        device.identity = "VST3: Ozone 12 (iZotope)".into();
        assert!(!super::verified_plugin_limiter(&device));
        let catalog = kumi_library::plugins::formats::structure::builtin("ozone-12").unwrap();
        let id = &catalog.info.ids.vst3.iter().find(|id| id.name == "Ozone 12 Maximizer").unwrap().id;
        device.identity = format!("Audio/Effects/VST3/{id}");
        assert!(super::verified_plugin_limiter(&device));
        device.identity = "VST: Ozone 12 Maximizer (iZotope)".into();
        assert!(!super::verified_plugin_limiter(&device));
    }
    use super::*;

    fn refused(busy: bool, there: bool, code: Option<&str>) -> Refused {
        Refused { why: "Renoise is still reading".into(), there, busy, code: code.map(str::to_owned), answered: true }
    }

    fn busy() -> Refused {
        refused(true, true, Some("busy"))
    }

    #[tokio::test(start_paused = true)]
    async fn a_render_answered_busy_is_asked_again_until_it_starts_and_says_why_once() {
        let (asked, heard) = (Cell::new(0), RefCell::new(vec![]));
        let began = tokio::time::Instant::now();
        let waiting = |why: &str| heard.borrow_mut().push(why.to_string());
        let answer = while_busy(&Signal::new(), waiting, || {
            asked.set(asked.get() + 1);
            let refuse = asked.get() <= 2;
            async move { Ok(if refuse { Err(busy()) } else { Ok(json!({"jobId": "1"})) }) }
        })
        .await
        .unwrap();
        assert_eq!(answer.ok(), Some(json!({"jobId": "1"})));
        assert_eq!(asked.get(), 3);
        assert_eq!(began.elapsed(), BUSY_PACE * 2);
        assert_eq!(*heard.borrow(), ["Renoise is still reading"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_daw_still_busy_is_said_once_the_wait_is_over() {
        let asked = Cell::new(0);
        let began = tokio::time::Instant::now();
        let answer = while_busy(
            &Signal::new(),
            |_| {},
            || {
                asked.set(asked.get() + 1);
                async { Ok(Err(busy())) }
            },
        )
        .await
        .unwrap();
        assert_eq!(answer.err().map(|refused| refused.why).as_deref(), Some("Renoise is still reading"));
        assert_eq!(began.elapsed(), BUSY_WAIT);
        assert_eq!(asked.get(), 21, "once at the start and once a second");
    }

    #[tokio::test(start_paused = true)]
    async fn any_other_refusal_is_said_at_once() {
        // Not busy; a DAW that's gone; and the host's "didn't answer in time", busy with no code, where the render may
        // have started and a second would queue behind it.
        for (busy, there, code) in [(false, true, None), (true, false, Some("busy")), (true, true, None)] {
            let (asked, heard) = (Cell::new(0), Cell::new(false));
            let answer = while_busy(
                &Signal::new(),
                |_| heard.set(true),
                || {
                    asked.set(asked.get() + 1);
                    async move { Ok(Err(refused(busy, there, code))) }
                },
            )
            .await
            .unwrap();
            assert!(answer.is_err());
            assert_eq!(asked.get(), 1, "busy {busy}, there {there}, code {code:?}");
            assert!(!heard.get());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_ends_the_wait_for_a_busy_daw_at_once() {
        let signal = Signal::new();
        let asked = Cell::new(0);
        let began = tokio::time::Instant::now();
        let stopping = signal.clone();
        let wait = while_busy(
            &signal,
            |_| {},
            || {
                asked.set(asked.get() + 1);
                async { Ok(Err(busy())) }
            },
        );
        let stop = async {
            tokio::time::sleep(BUSY_PACE + BUSY_PACE / 2).await;
            stopping.cancel();
        };
        let (answer, ()) = tokio::join!(wait, stop);
        assert!(matches!(answer, Err(RuntimeError::Aborted)));
        assert_eq!(began.elapsed(), BUSY_PACE + BUSY_PACE / 2, "at once, not at the next ask");
        assert_eq!(asked.get(), 2);
    }

    fn knob(name: &str, raw: f64) -> DeviceKnob {
        DeviceKnob { reference: format!("p:{name}"), name: name.into(), raw, min: 0., max: 1., items: vec![], scale: None }
    }

    /// (#361 review K41) A capture of what REAPER plays isn't called a render.
    #[test]
    fn a_jobs_progress_says_what_it_is() {
        assert_eq!(progress_words("REAPER", Some("capture"), 40), "Hearing what REAPER plays · 40%");
        assert_eq!(progress_words("REAPER", Some("render"), 40), "Rendering in REAPER · 40%");
    }

    #[test]
    fn a_bridges_notes_are_sentences() {
        let notes =
            notes_of(&json!({"notes": ["REAPER stopped playback to render", "Renoise stopped playback to render.", "gone (undo) "]}));
        assert_eq!(notes, ["REAPER stopped playback to render.", "Renoise stopped playback to render.", "gone (undo)."]);
    }

    #[test]
    fn a_knob_is_set_on_its_device_as_it_is_now_by_its_number() {
        // (a Renoise param ref from before a scratch copy moved the epoch; the device ref read just now)
        assert_eq!(
            knob_change("70c9ea:device:4.2", "70c9e6:param:4.2.3", "value", 8.),
            json!({"ref": "70c9ea:device:4.2", "param": 3, "value": 8.0})
        );
        assert_eq!(
            knob_change("r:fx:{G}", "r:param:{G}:12", "normalized", 0.7),
            json!({"ref": "r:fx:{G}", "param": 12, "normalized": 0.7})
        );
        assert_eq!(knob_change("d", "knob", "value", 1.), json!({"ref": "knob", "value": 1.0}));
    }

    #[test]
    fn an_equalisers_bands_come_from_its_knobs_names() {
        let reaeq = [
            knob("Freq-Low Shelf", 0.2),
            knob("Gain-Low Shelf", 0.5),
            knob("Q-Low Shelf", 0.3),
            knob("Freq-Band 2", 0.4),
            knob("Gain-Band 2", 0.5),
            knob("BW-Band 2", 0.3),
            knob("Freq-High Shelf", 0.8),
            knob("Gain-High Shelf", 0.5),
            knob("Gain", 0.5),
        ];
        let bands = eq_bands(&reaeq, false);
        let shapes: Vec<(String, Shape, bool)> = bands.iter().map(|band| (band.label.clone(), band.shape, band.octaves)).collect();
        assert_eq!(
            shapes,
            [
                ("Low Shelf".to_string(), Shape::LowShelf, false),
                ("Band 2".into(), Shape::Bell, true),
                ("High Shelf".into(), Shape::HighShelf, false)
            ]
        );
        assert!(bands[2].width.is_none(), "a band without a width knob");
        let eq10: Vec<DeviceKnob> = (1..=4)
            .flat_map(|n| [knob(&format!("Gain {n}"), 0.5), knob(&format!("Frequency {n}"), 0.5), knob(&format!("Bandwidth {n}"), 0.5)])
            .collect();
        let shapes: Vec<Shape> = eq_bands(&eq10, true).iter().map(|band| band.shape).collect();
        assert_eq!(shapes, [Shape::LowShelf, Shape::Bell, Shape::Bell, Shape::HighShelf], "shelves at the ends");
    }

    #[test]
    fn a_plug_ins_own_name_is_its_fx_name_without_reapers_wrapping() {
        assert_eq!(plugin_name("VST3: Pro-Q 3 (FabFilter)"), "Pro-Q 3");
        assert_eq!(plugin_name("JS: Volume Adjustment"), "Volume Adjustment");
        assert_eq!(plugin_name("VST: ReaEQ (Cockos)"), "ReaEQ");
        assert_eq!(plugin_name("EQ 10"), "EQ 10");
    }

    #[test]
    fn a_device_is_who_it_is_whatever_the_producer_named_it() {
        let row = |value: Value| identity_of(&value);
        // REAPER: the plug-in as REAPER calls it, format and maker in its name; renamed, its own name is `originalName`.
        assert_eq!(row(json!({"name": "VST3: Pro-Q 3 (FabFilter)"})).as_deref(), Some("VST3: Pro-Q 3 (FabFilter)"));
        assert_eq!(row(json!({"name": "Bass EQ", "originalName": "VST: ReaEQ (Cockos)"})).as_deref(), Some("VST: ReaEQ (Cockos)"));
        // Renoise: by its path, whatever its name says.
        assert_eq!(
            row(json!({"name": "Glue", "path": "Audio/Effects/Native/Bus Compressor"})).as_deref(),
            Some("Audio/Effects/Native/Bus Compressor")
        );
        assert_eq!(row(json!({"name": " "})), None, "nothing to know it by");
    }

    #[test]
    fn a_bandwidth_in_octaves_follows_q() {
        assert!((octaves_of(std::f64::consts::SQRT_2) - 1.).abs() < 0.01, "Q 1.41 is an octave");
        assert!(octaves_of(4.) < octaves_of(1.), "a narrower band");
    }

    #[test]
    fn a_renoise_parameter_in_its_own_units_scales_across_its_range_and_a_linear_gain_doesnt() {
        let grid = grid_from_units(0., "0.000 dB", -20., 20.).unwrap();
        let scale = Scale::read(&grid).unwrap();
        assert!((scale.raw(6.) - 6.).abs() < 1e-6 && (scale.shown(-12.) + 12.).abs() < 1e-6, "dB is the value");
        assert!(Scale::read(&grid_from_units(1000., "1000 Hz", 20., 20000.).unwrap()).is_some());
        assert!(grid_from_units(1., "0.000 dB", 0., 4.).is_none(), "the Gainer's linear gain shows dB");
        // A bare number that is the value (an EQ's bandwidth) scales as itself, so a fit can set it.
        let width = Scale::read(&grid_from_units(4., "4.00", 0.0001, 4.).unwrap()).unwrap();
        assert!((width.raw(2.) - 2.).abs() < 1e-3, "a bandwidth of 2 is 2: {}", width.raw(2.));
        assert!(grid_from_units(4., "11.00", 0.0001, 4.).is_none(), "a bare number that isn't the value");
        assert_eq!(shown_db("Adjustment (dB)", "6.0"), Some(6.));
        assert_eq!(shown_db("Boost", "4.500 dB"), Some(4.5));
        assert_eq!(shown_db("Ratio", "4.0"), None);
    }

    #[test]
    fn renoise_eq_gains_named_by_their_frequency_are_their_bands() {
        let eq = [
            knob("Gain 1 (50 Hz)", 0.),
            knob("Frequency 1", 50.),
            knob("Bandwidth 1", 1.),
            knob("Gain 2 (100 Hz)", 0.),
            knob("Frequency 2", 100.),
        ];
        let bands = eq_bands(&eq, true);
        assert_eq!(bands.iter().map(|band| band.label.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert!(bands[0].octaves && bands[1].width.is_none());
    }

    #[test]
    fn the_flags_come_from_the_status() {
        let flags = Flags::of(
            &json!({"listening": {"render": true, "capture": false, "audition": true, "grid": true, "scratch": true}, "demo": false}),
        );
        assert_eq!(flags, Flags { known: true, render: true, capture: false, audition: true, grid: true, scratch: true, demo: false });
        assert!(!Flags::of(&json!({"connected": true})).known, "an older bridge says nothing");
        assert!(Flags::of(&json!({"listening": {"render": false}, "demo": true})).demo);
    }
}

/// A bridge-cache file is never a library root. Copy through a verified descriptor, then claim
/// just that directory entry privately before deleting it; never sweep or repair the cache.
#[cfg(unix)]
mod restricted_staging {
    use super::wav_seconds;
    use super::{hear_refused, HearFailure, RuntimeError, Signal, SignalExt};
    use std::{
        ffi::{CStr, CString},
        fs::{File, Metadata, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
        path::{Path, PathBuf},
    };

    const MAX_BYTES: u64 = 50 * 1024 * 1024;

    pub(super) struct Staged {
        pub path: PathBuf,
        pub seconds: f64,
        pub warning: Option<String>,
    }

    fn unsafe_file(reason: impl Into<String>) -> RuntimeError {
        hear_refused("unsafe_render_file", reason)
    }

    fn name(path: &Path) -> Result<CString, RuntimeError> {
        CString::new(path.as_os_str().as_bytes()).map_err(|_| unsafe_file("A staging path contains a NUL."))
    }

    fn same_inode(a: &Metadata, b: &Metadata) -> bool {
        a.dev() == b.dev() && a.ino() == b.ino()
    }

    fn stable(a: &Metadata, b: &Metadata) -> bool {
        same_inode(a, b)
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }

    fn private(meta: &Metadata) -> bool {
        meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0
    }

    fn directory(path: &Path) -> Result<File, RuntimeError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| unsafe_file("The staging directory could not be opened without following a symlink."))?;
        let meta = file.metadata().map_err(|_| unsafe_file("The staging directory could not be inspected."))?;
        if !meta.is_dir() || !private(&meta) {
            return Err(unsafe_file("The staging directory must already be owner-only and owned by this account."));
        }
        Ok(file)
    }

    fn open_at(dir: &File, leaf: &CStr) -> std::io::Result<File> {
        let fd =
            unsafe { libc::openat(dir.as_raw_fd(), leaf.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    /// The installed bridges produce flat cache files, so no intermediate path may be traversed.
    pub(super) fn check_cache(cache: &Path) -> Result<(), RuntimeError> {
        match std::fs::symlink_metadata(cache) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => directory(cache).map(drop),
        }
    }

    fn rename_new(from: &File, old: &CStr, to: &File, new: &CStr) -> std::io::Result<()> {
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renameatx_np(from.as_raw_fd(), old.as_ptr(), to.as_raw_fd(), new.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe { libc::renameat2(from.as_raw_fd(), old.as_ptr(), to.as_raw_fd(), new.as_ptr(), libc::RENAME_NOREPLACE) };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "atomic no-replace rename is unavailable"));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// A race may replace the named entry. In that case put it back only if the old name is
    /// still vacant; otherwise preserve it in quarantine. Never unlink the substituted inode.
    fn remove_exact(
        cache: &File,
        leaf: &CStr,
        original: &Metadata,
        quarantine_root: &Path,
        call: &CStr,
        claim: impl FnOnce(&File, &CStr, &File, &CStr) -> std::io::Result<()>,
    ) -> Result<Option<String>, RuntimeError> {
        let left = |why: &str| {
            let raw = leaf.to_string_lossy();
            let mut chars = raw.chars();
            let mut name: String = chars.by_ref().take(48).map(|c| if c == '"' { '\'' } else { c }).collect();
            if chars.next().is_some() {
                name.push('…');
            }
            Some(format!("Source \"{name}\" remains in the owning Kumi listening cache: {why}."))
        };
        let current = open_at(cache, leaf)
            .and_then(|file| file.metadata())
            .map_err(|_| unsafe_file("The returned staging file disappeared or became unsafe before cleanup."))?;
        if !stable(original, &current) {
            return Err(unsafe_file("The returned staging file was replaced or modified; it was not deleted."));
        }
        let root = match directory(quarantine_root) {
            Ok(root) => root,
            Err(_) => return Ok(left("private quarantine unavailable")),
        };
        if unsafe { libc::mkdirat(root.as_raw_fd(), call.as_ptr(), 0o700) } != 0 {
            return Ok(left("private quarantine could not be created"));
        }
        let fd = unsafe {
            libc::openat(root.as_raw_fd(), call.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        };
        if fd < 0 {
            unsafe {
                libc::unlinkat(root.as_raw_fd(), call.as_ptr(), libc::AT_REMOVEDIR);
            }
            return Ok(left("private quarantine could not be opened"));
        }
        let claimed = unsafe { File::from_raw_fd(fd) };
        let slot = c"source";
        let kept_at = quarantine_root.join(std::ffi::OsStr::from_bytes(call.to_bytes())).join("source");
        if let Err(error) = claim(cache, leaf, &claimed, slot) {
            unsafe {
                libc::unlinkat(root.as_raw_fd(), call.as_ptr(), libc::AT_REMOVEDIR);
            }
            return Ok(left(if error.raw_os_error() == Some(libc::EXDEV) { "another filesystem" } else { "cleanup claim failed" }));
        }
        // The permanent private quarantine is never auto-deleted on cancellation or restart.
        // In particular, a crash before this comparison cannot make Scratch delete a replacement.
        // Inspect the exact directory descriptor used by rename/unlink, not a pathname whose
        // ancestors could now name another directory.
        let captured = open_at(&claimed, slot).and_then(|file| file.metadata());
        let ours = captured.as_ref().is_ok_and(|now| {
            same_inode(original, now)
                && original.len() == now.len()
                && original.mtime() == now.mtime()
                && original.mtime_nsec() == now.mtime_nsec()
        });
        if !ours {
            let restored = rename_new(&claimed, slot, cache, leaf).is_ok();
            if restored {
                unsafe {
                    libc::unlinkat(root.as_raw_fd(), call.as_ptr(), libc::AT_REMOVEDIR);
                }
            }
            return Err(unsafe_file(if restored {
                "The staging file changed during cleanup; its replacement was put back and nothing was deleted."
            } else {
                eprintln!("kumi listen: a substituted staging file was preserved at {}", kept_at.display());
                "The staging file changed during cleanup; its replacement remains in this profile's permanent quarantine."
            }));
        }
        if unsafe { libc::unlinkat(claimed.as_raw_fd(), slot.as_ptr(), 0) } != 0 {
            eprintln!("kumi listen: the verified staging inode remains at {}", kept_at.display());
            return Ok(Some("The verified source remains in this profile's permanent quarantine because deletion failed.".into()));
        }
        if unsafe { libc::unlinkat(root.as_raw_fd(), call.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Ok(Some("The render source was removed, but its empty private quarantine directory remains.".into()));
        }
        Ok(None)
    }

    fn verified_source(cache: &Path, returned: &Path) -> Result<(File, CString, File, Metadata), RuntimeError> {
        let cache_handle = directory(cache)?;
        let canonical_cache = std::fs::canonicalize(cache).map_err(|_| unsafe_file("The bridge cache cannot be resolved."))?;
        let canonical_file = std::fs::canonicalize(returned).map_err(|_| unsafe_file("The returned render file cannot be resolved."))?;
        if !returned.is_absolute() || canonical_file.parent() != Some(canonical_cache.as_path()) {
            return Err(unsafe_file("The returned file is not a direct file in this bridge's verified listening cache."));
        }
        let cache_named = std::fs::symlink_metadata(cache).map_err(|_| unsafe_file("The bridge cache disappeared."))?;
        let cache_opened = cache_handle.metadata().map_err(|_| unsafe_file("The bridge cache descriptor cannot be inspected."))?;
        if !same_inode(&cache_named, &cache_opened) {
            return Err(unsafe_file("The cache directory was replaced while it was being opened."));
        }
        let leaf = name(Path::new(returned.file_name().ok_or_else(|| unsafe_file("The returned render has no filename."))?))?;
        let source =
            open_at(&cache_handle, &leaf).map_err(|_| unsafe_file("The returned render is a symlink or cannot be opened safely."))?;
        let before = source.metadata().map_err(|_| unsafe_file("The opened render cannot be inspected."))?;
        if !before.is_file() || !private(&before) || before.nlink() != 1 || before.len() > MAX_BYTES {
            return Err(unsafe_file("The returned render must be an owner-only regular file, not hard-linked, and at most 50 MiB."));
        }
        let named = std::fs::symlink_metadata(returned).map_err(|_| unsafe_file("The named render disappeared."))?;
        if !same_inode(&before, &named) || named.file_type().is_symlink() {
            return Err(unsafe_file("The returned path does not name the verified open render file."));
        }
        Ok((cache_handle, leaf, source, before))
    }

    pub(super) fn discard(cache: &Path, scratch: &Path, quarantine_root: &Path, returned: &Path) -> Result<Option<String>, RuntimeError> {
        let (cache, leaf, source, before) = verified_source(cache, returned)?;
        if !stable(&before, &source.metadata().map_err(|_| unsafe_file("The opened render cannot be rechecked."))?) {
            return Err(unsafe_file("The render changed before it could be discarded."));
        }
        let call = name(Path::new(scratch.file_name().ok_or_else(|| unsafe_file("The call scratch has no name."))?))?;
        let warning = remove_exact(&cache, &leaf, &before, quarantine_root, &call, rename_new)?;
        if let Some(warning) = &warning {
            eprintln!("kumi listen: {warning} Source: {}", returned.display());
        }
        Ok(warning)
    }

    pub(super) fn copy(
        cache: &Path,
        scratch: &Path,
        quarantine_root: &Path,
        returned: &Path,
        signal: &Signal,
    ) -> Result<Staged, RuntimeError> {
        copy_after_open(cache, scratch, quarantine_root, returned, signal, || {})
    }

    fn copy_after_open(
        cache: &Path,
        scratch: &Path,
        quarantine_root: &Path,
        returned: &Path,
        signal: &Signal,
        after_open: impl FnOnce(),
    ) -> Result<Staged, RuntimeError> {
        signal.check()?;
        let (cache_handle, leaf, mut source, before) = verified_source(cache, returned)?;
        let scratch_handle = directory(scratch)?;
        let scratch_named = std::fs::symlink_metadata(scratch).map_err(|_| unsafe_file("The scratch directory disappeared."))?;
        let scratch_opened = scratch_handle.metadata().map_err(|_| unsafe_file("The scratch descriptor cannot be inspected."))?;
        if !same_inode(&scratch_named, &scratch_opened) {
            return Err(unsafe_file("The scratch directory was replaced while it was being opened."));
        }
        let call = name(Path::new(scratch.file_name().ok_or_else(|| unsafe_file("The call scratch has no name."))?))?;
        let target = CString::new(format!("render-{}.wav", uuid::Uuid::new_v4())).expect("UUID has no NUL");
        let fd = unsafe {
            libc::openat(
                scratch_handle.as_raw_fd(),
                target.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(unsafe_file("A private scratch copy could not be created."));
        }
        let mut output = unsafe { File::from_raw_fd(fd) };
        after_open();
        let result = (|| {
            let mut buffer = [0u8; 64 * 1024];
            let mut copied = 0u64;
            loop {
                signal.check()?;
                let count = source.read(&mut buffer).map_err(|_| unsafe_file("The render could not be read."))?;
                if count == 0 {
                    break;
                }
                copied += count as u64;
                if copied > before.len() || copied > MAX_BYTES {
                    return Err(unsafe_file("The render grew while it was being copied."));
                }
                output.write_all(&buffer[..count]).map_err(|_| unsafe_file("The render could not be copied into scratch."))?;
            }
            let after = source.metadata().map_err(|_| unsafe_file("The render could not be rechecked."))?;
            if copied != before.len() || !stable(&before, &after) {
                return Err(unsafe_file("The render changed while it was being copied."));
            }
            let named_scratch = std::fs::symlink_metadata(scratch).map_err(|_| unsafe_file("The scratch directory disappeared."))?;
            if !same_inode(&scratch_opened, &named_scratch) {
                return Err(unsafe_file("The scratch directory was replaced during the copy."));
            }
            let output_meta = output.metadata().map_err(|_| unsafe_file("The scratch copy could not be inspected."))?;
            let named_output = std::fs::symlink_metadata(scratch.join(target.to_str().expect("UUID filename is UTF-8")))
                .map_err(|_| unsafe_file("The scratch copy disappeared."))?;
            if !same_inode(&output_meta, &named_output)
                || named_output.file_type().is_symlink()
                || output_meta.nlink() != 1
                || !private(&output_meta)
                || output_meta.len() != before.len()
            {
                return Err(unsafe_file("The scratch copy was replaced before it could be returned."));
            }
            let seconds = wav_seconds(&mut output).map_err(|_| unsafe_file("The rendered file is not a complete PCM WAV."))?;
            let warning = remove_exact(&cache_handle, &leaf, &before, quarantine_root, &call, rename_new)?;
            if let Some(warning) = &warning {
                eprintln!("kumi listen: {warning} Source: {}", returned.display());
            }
            if !seconds.is_finite() || seconds > 65. {
                return Err(HearFailure::Unsure {
                    code: None,
                    reason: "The offline render exceeded the 65-second audio cap; it was discarded.".into(),
                    restored: false,
                    marked_modified: false,
                }
                .into());
            }
            Ok(Staged { path: scratch.join(target.to_str().expect("UUID filename is UTF-8")), seconds, warning })
        })();
        if matches!(result, Err(RuntimeError::Aborted)) {
            match remove_exact(&cache_handle, &leaf, &before, quarantine_root, &call, rename_new) {
                Ok(None) => {}
                Ok(Some(warning)) => eprintln!("kumi listen: {warning} Source: {}", returned.display()),
                Err(error) => {
                    eprintln!("kumi listen: cancelled staging left its source untouched: {error}; source: {}", returned.display())
                }
            }
        }
        if result.is_err() {
            unsafe {
                libc::unlinkat(scratch_handle.as_raw_fd(), target.as_ptr(), 0);
            }
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        fn copy(cache: &Path, scratch: &Path, returned: &Path, signal: &Signal) -> Result<Staged, RuntimeError> {
            super::copy(cache, scratch, &cache.parent().unwrap().join("quarantine"), returned, signal)
        }

        fn lab() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let home = tempfile::tempdir().unwrap();
            let cache = home.path().join("cache");
            let scratch = home.path().join("scratch");
            for dir in [&cache, &scratch, &home.path().join("quarantine")] {
                std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
            }
            (home, cache, scratch)
        }

        fn wav(path: &Path, seconds: usize) {
            let data = kumi_media::audio::decode::float_wav(&[vec![0.; seconds * 10]], 10);
            let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
            file.write_all(&data).unwrap();
        }

        #[test]
        fn terminal_discard_removes_only_the_returned_verified_source() {
            let (home, cache, scratch) = lab();
            let source = cache.join("this-call.wav");
            let neighbor = cache.join("unrelated.wav");
            wav(&source, 1);
            wav(&neighbor, 2);
            assert!(super::discard(&cache, &scratch, &home.path().join("quarantine"), &source).unwrap().is_none());
            assert!(!source.exists());
            assert_eq!(wav_seconds(&mut File::open(&neighbor).unwrap()).unwrap(), 2.);
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "discard never publishes audio");
        }

        #[test]
        fn copies_only_the_returned_file_and_preserves_neighbors() {
            let (_home, cache, scratch) = lab();
            let source = cache.join("this-call.wav");
            let neighbor = cache.join("old-unrelated.wav");
            wav(&source, 2);
            wav(&neighbor, 3);
            let expected = std::fs::read(&source).unwrap();
            let staged = copy(&cache, &scratch, &source, &Signal::new()).unwrap();
            assert_eq!(staged.seconds, 2.);
            assert!(staged.warning.is_none());
            assert_eq!(std::fs::read(&staged.path).unwrap(), expected);
            assert!(!source.exists());
            assert!(neighbor.is_file());
            assert_eq!(std::fs::metadata(&staged.path).unwrap().mode() & 0o077, 0);
        }

        #[test]
        fn cancellation_after_open_cleans_only_the_verified_source() {
            for replaced in [false, true] {
                let (home, cache, scratch) = lab();
                let source = cache.join("this-call.wav");
                wav(&source, 1);
                let signal = Signal::new();
                let result = super::copy_after_open(&cache, &scratch, &home.path().join("quarantine"), &source, &signal, || {
                    if replaced {
                        std::fs::rename(&source, cache.join("original.wav")).unwrap();
                        wav(&source, 2);
                    }
                    signal.cancel();
                });
                assert!(matches!(result, Err(RuntimeError::Aborted)));
                assert_eq!(source.exists(), replaced, "a replacement is never discarded");
                if replaced {
                    let mut survivor = File::open(&source).unwrap();
                    assert_eq!(wav_seconds(&mut survivor).unwrap(), 2.);
                }
                assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
                assert_eq!(std::fs::read_dir(home.path().join("quarantine")).unwrap().count(), 0);
            }
        }

        #[test]
        fn rejects_outside_symlink_directory_shared_and_oversized_files_without_deletion() {
            let (home, cache, scratch) = lab();
            let outside = home.path().join("outside.wav");
            wav(&outside, 1);
            let symlink = cache.join("linked.wav");
            std::os::unix::fs::symlink(&outside, &symlink).unwrap();
            let directory = cache.join("directory.wav");
            std::fs::create_dir(&directory).unwrap();
            let shared = cache.join("shared.wav");
            wav(&shared, 1);
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o644)).unwrap();
            let huge = cache.join("huge.wav");
            wav(&huge, 1);
            OpenOptions::new().write(true).open(&huge).unwrap().set_len(MAX_BYTES + 1).unwrap();
            for path in [&outside, &symlink, &directory, &shared, &huge] {
                assert!(copy(&cache, &scratch, path, &Signal::new()).is_err(), "{path:?}");
                assert!(std::fs::symlink_metadata(path).is_ok(), "{path:?} must not be deleted");
            }
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        }

        #[test]
        fn rejects_hardlinks_and_an_unsafe_cache_without_repairing_permissions() {
            let (_home, cache, scratch) = lab();
            let source = cache.join("this-call.wav");
            wav(&source, 1);
            let neighbor = cache.join("hardlinked-neighbor.wav");
            std::fs::hard_link(&source, &neighbor).unwrap();
            assert!(copy(&cache, &scratch, &source, &Signal::new()).is_err());
            std::fs::remove_file(&neighbor).unwrap();
            std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(copy(&cache, &scratch, &source, &Signal::new()).is_err());
            assert_eq!(std::fs::metadata(&cache).unwrap().mode() & 0o777, 0o755);
            assert!(source.exists());
        }

        #[test]
        fn replacement_is_not_deleted_and_cancellation_does_not_copy() {
            let (_home, cache, scratch) = lab();
            let source = cache.join("this-call.wav");
            wav(&source, 1);
            let before = std::fs::metadata(&source).unwrap();
            let kept = cache.join("original-kept.wav");
            std::fs::rename(&source, &kept).unwrap();
            wav(&source, 2);
            let replacement = std::fs::read(&source).unwrap();
            assert!(remove_exact(
                &directory(&cache).unwrap(),
                c"this-call.wav",
                &before,
                &_home.path().join("quarantine"),
                c"replacement",
                rename_new
            )
            .is_err());
            assert_eq!(std::fs::read(&source).unwrap(), replacement);
            assert!(kept.exists());
            let signal = Signal::new();
            signal.cancel();
            assert!(matches!(copy(&cache, &scratch, &source, &signal), Err(RuntimeError::Aborted)));
            assert!(source.exists());
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        }

        #[test]
        fn substitution_between_copy_and_claim_survives_and_is_not_left_in_scratch() {
            for occupied in [false, true] {
                let (home, cache, scratch) = lab();
                let source = cache.join("this-call.wav");
                let original_kept = cache.join("original-kept.wav");
                wav(&source, 1);
                let before = std::fs::metadata(&source).unwrap();
                let quarantine = home.path().join("quarantine");
                let outcome =
                    remove_exact(&directory(&cache).unwrap(), c"this-call.wav", &before, &quarantine, c"call-id", |from, old, to, new| {
                        std::fs::rename(&source, &original_kept).unwrap();
                        wav(&source, 2);
                        rename_new(from, old, to, new)?;
                        if occupied {
                            wav(&source, 3);
                        }
                        Ok(())
                    });
                assert!(outcome.is_err(), "a substituted file must never be accepted as this render");
                assert!(original_kept.exists());
                let preserved = if occupied { quarantine.join("call-id").join("source") } else { source.clone() };
                assert_eq!(wav_seconds(&mut File::open(&preserved).unwrap()).unwrap(), 2.);
                if occupied {
                    assert_eq!(wav_seconds(&mut File::open(&source).unwrap()).unwrap(), 3.);
                }
                assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
            }
        }

        #[test]
        fn cleanup_inspects_the_claimed_directory_handle_not_its_replaced_path() {
            let (home, cache, _scratch) = lab();
            let source = cache.join("this-call.wav");
            wav(&source, 1);
            let before = std::fs::metadata(&source).unwrap();
            let quarantine = home.path().join("quarantine");
            let call = quarantine.join("call-id");
            let moved = quarantine.join("moved");
            let warning =
                remove_exact(&directory(&cache).unwrap(), c"this-call.wav", &before, &quarantine, c"call-id", |from, old, to, new| {
                    rename_new(from, old, to, new)?;
                    std::fs::rename(&call, &moved)?;
                    std::fs::DirBuilder::new().mode(0o700).create(&call)?;
                    wav(&call.join("source"), 2);
                    Ok(())
                })
                .unwrap();
            assert!(warning.is_some(), "the substituted quarantine directory is left untouched");
            assert!(!source.exists());
            assert!(!moved.join("source").exists(), "the verified source is deleted through its descriptor");
            assert_eq!(wav_seconds(&mut File::open(call.join("source")).unwrap()).unwrap(), 2.);
        }

        #[test]
        fn cross_filesystem_cleanup_is_a_warning_and_leaves_the_exact_source() {
            let (home, cache, _scratch) = lab();
            let source = cache.join("this-call.wav");
            wav(&source, 1);
            let before = std::fs::metadata(&source).unwrap();
            let warning = remove_exact(
                &directory(&cache).unwrap(),
                c"this-call.wav",
                &before,
                &home.path().join("quarantine"),
                c"call-id",
                |_, _, _, _| Err(std::io::Error::from_raw_os_error(libc::EXDEV)),
            )
            .unwrap()
            .unwrap();
            assert!(warning.contains("this-call.wav"), "the notice identifies the remaining source");
            assert!(!warning.contains('/') && !warning.contains('\\'), "only a basename, never a local path: {warning}");
            assert!(same_inode(&before, &std::fs::metadata(&source).unwrap()));
            assert_eq!(std::fs::read_dir(home.path().join("quarantine")).unwrap().count(), 0);
        }

        #[test]
        fn audio_over_sixty_five_seconds_is_discarded_with_typed_uncertainty() {
            let (_home, cache, scratch) = lab();
            let source = cache.join("this-call.wav");
            wav(&source, 66);
            assert!(matches!(copy(&cache, &scratch, &source, &Signal::new()), Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
            assert!(!source.exists());
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        }
    }
}
/// Read only the PCM WAV's layout, without invoking a converter or measuring its audio.
fn wav_seconds(file: &mut File) -> std::io::Result<f64> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "the render is not a complete PCM WAV");
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0; 12];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err(invalid());
    }
    let end = u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64 + 8;
    if end > length || end < 12 {
        return Err(invalid());
    }
    let mut format = None;
    let mut data = None;
    let mut at = 12;
    while at + 8 <= end {
        file.seek(SeekFrom::Start(at))?;
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        if size > end - at - 8 {
            return Err(invalid());
        }
        match &chunk[..4] {
            b"fmt " if size >= 16 => {
                if format.is_some() {
                    return Err(invalid());
                }
                let mut fmt = [0; 16];
                file.read_exact(&mut fmt)?;
                let mut encoding = u16::from_le_bytes(fmt[..2].try_into().unwrap());
                let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                let align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
                let bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
                if encoding == 65534 {
                    if size < 40 {
                        return Err(invalid());
                    }
                    let mut extension = [0u8; 24];
                    file.read_exact(&mut extension)?;
                    let extra = u16::from_le_bytes(extension[..2].try_into().unwrap());
                    let valid_bits = u16::from_le_bytes(extension[2..4].try_into().unwrap());
                    if extra < 22
                        || u64::from(extra) + 18 > size
                        || valid_bits == 0
                        || valid_bits > bits
                        || extension[12..] != [0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]
                    {
                        return Err(invalid());
                    }
                    encoding = match u32::from_le_bytes(extension[8..12].try_into().unwrap()) {
                        1 => 1,
                        3 => 3,
                        _ => return Err(invalid()),
                    };
                }
                if !matches!(encoding, 1 | 3)
                    || channels == 0
                    || channels > 64
                    || rate == 0
                    || bits == 0
                    || bits % 8 != 0
                    || (encoding == 3 && !matches!(bits, 32 | 64))
                    || u32::from(align) != u32::from(channels) * u32::from(bits / 8)
                {
                    return Err(invalid());
                }
                format = Some((rate, align));
            }
            b"data" => {
                if data.replace(size).is_some() {
                    return Err(invalid());
                }
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    let (rate, align) = format.ok_or_else(invalid)?;
    let bytes = data.ok_or_else(invalid)?;
    if bytes == 0 || bytes % u64::from(align) != 0 {
        return Err(invalid());
    }
    Ok((bytes / u64::from(align)) as f64 / f64::from(rate))
}

#[cfg(test)]
#[test]
fn restricted_wave_extensible_rejects_compressed_subformats_before_duration_claims() {
    use std::io::Write;
    for (subformat, accepted) in [(1u32, true), (0x55, false)] {
        let mut format = Vec::new();
        format.extend_from_slice(&65534u16.to_le_bytes());
        format.extend_from_slice(&1u16.to_le_bytes());
        format.extend_from_slice(&1u32.to_le_bytes());
        format.extend_from_slice(&2u32.to_le_bytes());
        format.extend_from_slice(&2u16.to_le_bytes());
        format.extend_from_slice(&16u16.to_le_bytes());
        format.extend_from_slice(&22u16.to_le_bytes());
        format.extend_from_slice(&16u16.to_le_bytes());
        format.extend_from_slice(&0u32.to_le_bytes());
        format.extend_from_slice(&subformat.to_le_bytes());
        format.extend_from_slice(&[0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
        let mut wave = b"RIFF".to_vec();
        wave.extend_from_slice(&62u32.to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&40u32.to_le_bytes());
        wave.extend_from_slice(&format);
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&2u32.to_le_bytes());
        wave.extend_from_slice(&[0, 0]);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&wave).unwrap();
        let seconds = wav_seconds(&mut file);
        if accepted {
            assert_eq!(seconds.unwrap(), 1.);
        } else {
            assert!(seconds.is_err());
        }
    }
}

#[cfg(windows)]
mod restricted_staging {
    use super::{hear_refused, wav_seconds, HearFailure, RuntimeError, Signal, SignalExt};
    use std::{
        fs::{File, OpenOptions},
        io::{Read, Write},
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            fs::OpenOptionsExt,
            io::{AsRawHandle, FromRawHandle},
        },
        path::{Path, PathBuf},
    };
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FileDispositionInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW, SetFileInformationByHandle,
            BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING, VOLUME_NAME_DOS,
        },
    };

    const MAX_BYTES: u64 = 50 * 1024 * 1024;

    pub(super) struct Staged {
        pub path: PathBuf,
        pub seconds: f64,
        pub warning: Option<String>,
    }

    fn unsafe_file(reason: impl Into<String>) -> RuntimeError {
        hear_refused("unsafe_render_file", reason)
    }

    fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, RuntimeError> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(unsafe_file("The render file's handle identity could not be read."));
        }
        Ok(info)
    }

    fn same(a: &BY_HANDLE_FILE_INFORMATION, b: &BY_HANDLE_FILE_INFORMATION) -> bool {
        a.dwVolumeSerialNumber == b.dwVolumeSerialNumber
            && a.nFileIndexHigh == b.nFileIndexHigh
            && a.nFileIndexLow == b.nFileIndexLow
            && a.nFileSizeHigh == b.nFileSizeHigh
            && a.nFileSizeLow == b.nFileSizeLow
            && a.ftLastWriteTime.dwHighDateTime == b.ftLastWriteTime.dwHighDateTime
            && a.ftLastWriteTime.dwLowDateTime == b.ftLastWriteTime.dwLowDateTime
    }

    fn open(path: &Path, directory: bool) -> Result<File, RuntimeError> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(unsafe_file("A staging path contains a NUL."));
        }
        wide.push(0);
        // Holding the cache directory without delete sharing prevents replacement. Holding a
        // source without write/delete sharing prevents a writer changing it during the copy.
        let access = if directory { FILE_READ_ATTRIBUTES } else { GENERIC_READ | DELETE };
        let sharing = if directory { FILE_SHARE_READ | FILE_SHARE_WRITE } else { FILE_SHARE_READ };
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                sharing,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 },
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(unsafe_file("The staging path could not be opened safely."));
        }
        let file = unsafe { File::from_raw_handle(handle) };
        let info = information(&file)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || !daw_bridge::lifecycle::native_acl::is_kumis_shape(path)
        {
            return Err(unsafe_file("The staging path must already be owner-only and must not be a reparse point."));
        }
        Ok(file)
    }

    pub(super) fn check_cache(cache: &Path) -> Result<(), RuntimeError> {
        match std::fs::symlink_metadata(cache) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => open(cache, true).map(drop),
        }
    }

    fn remove_handle(file: &File, original: &BY_HANDLE_FILE_INFORMATION) -> Result<Option<String>, RuntimeError> {
        if !same(original, &information(file)?) {
            return Err(unsafe_file("The verified render handle changed; it was not deleted."));
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        let deleted = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        Ok((deleted == 0).then(|| "The verified bridge staging file could not be deleted by handle; it was left in place.".into()))
    }

    fn final_path(file: &File) -> Result<PathBuf, RuntimeError> {
        let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
        let size = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), std::ptr::null_mut(), 0, flags) };
        if size == 0 || size > 32768 {
            return Err(unsafe_file("The opened file's final path could not be established."));
        }
        let mut buffer = vec![0u16; size as usize];
        let written = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), size, flags) };
        if written == 0 || written >= size {
            return Err(unsafe_file("The opened file's final path changed while being read."));
        }
        Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written as usize])))
    }

    fn verified_source(cache: &Path, returned: &Path) -> Result<(File, File, BY_HANDLE_FILE_INFORMATION), RuntimeError> {
        if returned.file_name().is_none_or(|leaf| leaf.encode_wide().any(|character| character == u16::from(b':'))) {
            return Err(unsafe_file("A returned render must be a regular file, not an alternate data stream."));
        }
        let cache_handle = open(cache, true)?;
        let canonical_cache = std::fs::canonicalize(cache).map_err(|_| unsafe_file("The bridge cache cannot be resolved."))?;
        let canonical_file = std::fs::canonicalize(returned).map_err(|_| unsafe_file("The returned render cannot be resolved."))?;
        if !returned.is_absolute() || canonical_file.parent() != Some(canonical_cache.as_path()) {
            return Err(unsafe_file("The returned file is outside this bridge's verified listening cache."));
        }
        let source = open(returned, false)?;
        if final_path(&source)?.parent() != Some(final_path(&cache_handle)?.as_path()) {
            return Err(unsafe_file("The opened render is outside the verified cache directory."));
        }
        let before = information(&source)?;
        let length = (u64::from(before.nFileSizeHigh) << 32) | u64::from(before.nFileSizeLow);
        if before.nNumberOfLinks != 1 || length > MAX_BYTES {
            return Err(unsafe_file("The returned render must not be hard-linked and must be at most 50 MiB."));
        }
        Ok((cache_handle, source, before))
    }

    fn source_warning(warning: Option<String>, returned: &Path) -> Option<String> {
        warning.map(|mut warning| {
            let raw = returned.file_name().expect("verified source has a basename").to_string_lossy();
            let mut chars = raw.chars();
            warning.push_str(" Source \"");
            warning.extend(chars.by_ref().take(48).map(|c| if c == '"' { '\'' } else { c }));
            if chars.next().is_some() {
                warning.push('…');
            }
            warning.push_str("\".");
            eprintln!("kumi listen: {warning} Source: {}", returned.display());
            warning
        })
    }

    pub(super) fn discard(cache: &Path, _scratch: &Path, _quarantine_root: &Path, returned: &Path) -> Result<Option<String>, RuntimeError> {
        let (_cache, source, before) = verified_source(cache, returned)?;
        remove_handle(&source, &before).map(|warning| source_warning(warning, returned))
    }

    pub(super) fn copy(
        cache: &Path,
        scratch: &Path,
        _quarantine_root: &Path,
        returned: &Path,
        signal: &Signal,
    ) -> Result<Staged, RuntimeError> {
        copy_after_open(cache, scratch, returned, signal, || {})
    }

    fn copy_after_open(
        cache: &Path,
        scratch: &Path,
        returned: &Path,
        signal: &Signal,
        after_open: impl FnOnce(),
    ) -> Result<Staged, RuntimeError> {
        signal.check()?;
        let (_cache, mut source, before) = verified_source(cache, returned)?;
        let _scratch = open(scratch, true)?;
        let length = (u64::from(before.nFileSizeHigh) << 32) | u64::from(before.nFileSizeLow);
        let target = scratch.join(format!("render-{}.wav", uuid::Uuid::new_v4()));
        let mut output = OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&target)
            .map_err(|_| unsafe_file("A private scratch copy could not be created."))?;
        after_open();
        let result = (|| {
            if final_path(&output)?.parent() != Some(final_path(&_scratch)?.as_path()) {
                return Err(unsafe_file("The scratch path changed before its copy could be written."));
            }
            let mut buffer = [0u8; 64 * 1024];
            let mut copied = 0u64;
            loop {
                signal.check()?;
                let count = source.read(&mut buffer).map_err(|_| unsafe_file("The render could not be read."))?;
                if count == 0 {
                    break;
                }
                copied += count as u64;
                if copied > length || copied > MAX_BYTES {
                    return Err(unsafe_file("The render grew while being copied."));
                }
                output.write_all(&buffer[..count]).map_err(|_| unsafe_file("The render could not be copied into scratch."))?;
            }
            if copied != length || !same(&before, &information(&source)?) {
                return Err(unsafe_file("The render changed while being copied."));
            }
            let copied_path = final_path(&output)?;
            if copied_path.parent() != Some(final_path(&_scratch)?.as_path()) {
                return Err(unsafe_file("The scratch directory changed during the copy."));
            }
            let seconds = wav_seconds(&mut output).map_err(|_| unsafe_file("The rendered file is not a complete PCM WAV."))?;
            let warning = source_warning(remove_handle(&source, &before)?, returned);
            if !seconds.is_finite() || seconds > 65. {
                return Err(HearFailure::Unsure {
                    code: None,
                    reason: "The offline render exceeded the 65-second audio cap; it was discarded.".into(),
                    restored: false,
                    marked_modified: false,
                }
                .into());
            }
            Ok(Staged { path: copied_path, seconds, warning })
        })();
        if matches!(result, Err(RuntimeError::Aborted)) {
            match remove_handle(&source, &before) {
                Ok(warning) => {
                    source_warning(warning, returned);
                }
                Err(error) => {
                    eprintln!("kumi listen: cancelled staging left its source untouched: {error}; source: {}", returned.display())
                }
            }
        }
        if result.is_err() {
            if let Ok(info) = information(&output) {
                let _ = remove_handle(&output, &info);
            }
        }
        drop(output);
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn private_file(path: &Path, bytes: &[u8]) {
            let mut file = OpenOptions::new().write(true).create_new(true).share_mode(0).open(path).unwrap();
            // Known-new, pinned fixture; an elevated runner may otherwise make Administrators
            // the owner. Returned native files never get an ownership repair.
            daw_bridge::lifecycle::native_acl::make_kumis_shape(path).unwrap();
            file.write_all(bytes).unwrap();
        }

        #[test]
        fn deletion_uses_the_verified_handle_and_preserves_neighbors() {
            let home = tempfile::tempdir().unwrap();
            let cache = home.path().join("cache");
            let scratch = home.path().join("scratch");
            for path in [&cache, &scratch] {
                daw_bridge::lifecycle::native_acl::create_kumis_folder(path).unwrap();
            }
            let source = cache.join("this-call.wav");
            let neighbor = cache.join("neighbor.wav");
            let wav = kumi_media::audio::decode::float_wav(&[vec![0.; 10]], 10);
            private_file(&source, &wav);
            private_file(&neighbor, &wav);
            let staged = copy(&cache, &scratch, &home.path().join("quarantine"), &source, &Signal::new()).unwrap();
            assert_eq!(staged.seconds, 1.);
            assert!(staged.warning.is_none());
            assert!(!source.exists());
            assert_eq!(std::fs::read(&neighbor).unwrap(), wav);
            assert_eq!(std::fs::read(&staged.path).unwrap(), wav);
        }

        #[test]
        fn cancellation_before_open_preserves_the_source_and_after_open_discards_only_its_handle() {
            for after_open in [false, true] {
                let home = tempfile::tempdir().unwrap();
                let cache = home.path().join("cache");
                let scratch = home.path().join("scratch");
                for path in [&cache, &scratch] {
                    daw_bridge::lifecycle::native_acl::create_kumis_folder(path).unwrap();
                }
                let source = cache.join("this-call.wav");
                let neighbor = cache.join("neighbor.wav");
                let wav = kumi_media::audio::decode::float_wav(&[vec![0.; 10]], 10);
                private_file(&source, &wav);
                private_file(&neighbor, b"leave this alone");
                let signal = Signal::new();
                if !after_open {
                    signal.cancel();
                }
                let result = copy_after_open(&cache, &scratch, &source, &signal, || signal.cancel());
                assert!(matches!(result, Err(RuntimeError::Aborted)));
                assert_eq!(source.exists(), !after_open);
                assert_eq!(std::fs::read(&neighbor).unwrap(), b"leave this alone");
                assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
            }
        }

        #[test]
        fn an_open_verified_source_cannot_be_substituted_before_handle_deletion() {
            let home = tempfile::tempdir().unwrap();
            let source = home.path().join("source.wav");
            private_file(&source, b"owned");
            let opened = open(&source, false).unwrap();
            let before = information(&opened).unwrap();
            assert!(std::fs::rename(&source, home.path().join("moved.wav")).is_err());
            assert!(remove_handle(&opened, &before).unwrap().is_none());
            drop(opened);
            assert!(!source.exists());
        }
    }
}
