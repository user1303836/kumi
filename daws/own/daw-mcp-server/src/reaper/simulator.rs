//! A scripted REAPER project for the host's tests: the bridge's `read`, `snapshot`, `focus` and `focus_body` answered
//! from a table of rows, the way the extension answers them (the M2 contract): pages by limit and cursor, a page that
//! stops at the frame bound, an epoch that refuses a cursor from before it changed. Small on purpose: what the host
//! tests are about is the plumbing, not what REAPER holds.
use crate::simulator::listening::SimListening;
use crate::simulator::{Respond, SimulatorConfig};
use daw_bridge::auth::Secret;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The most an answer carries (the contract's frame bound, less room for the envelope).
const PAGE_BYTES: usize = 224 * 1024;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
/// The arrange body's page, in tracks.
const MAX_ARRANGE: usize = 500;
const KINDS: [&str; 12] = ["project", "track", "item", "take", "fx", "param", "send", "marker", "region", "note", "envelope", "spans"];
/// What the bridge answers for listening (M5).
pub const LISTENING_METHODS: [&str; 8] =
    ["render_start", "capture_start", "listen_status", "listen_cancel", "audition", "audition_stop", "audio_file", "scratch"];
const BODIES: [&str; 5] = ["arrange", "midi", "fx", "routing", "mixer"];

struct State {
    epoch: u32,
    tracks: Vec<Value>,
    changed: u64,
    /// The sim keeps its pages inside the frame bound, as the extension does; a test can make it ignore the bound.
    keeps_bound: bool,
    /// What the sim was asked, in order: method and params.
    asked: Vec<(String, Value)>,
    /// The undo history's descriptions, oldest first (the changes applied and restored).
    undo: Vec<String>,
    /// A plan's undo step, open: its label (the changes made meanwhile make no step of their own).
    open_step: Option<String>,
    /// A dialog shows for the next change preview only (it closes right after).
    busy_once: bool,
    /// The next apply is made and then answered `uncertain` (a job the extension took and didn't finish in time).
    uncertain_once: bool,
    /// The next restore is made and then answered `uncertain`.
    uncertain_restore_once: bool,
    /// The next restore is made in part: its answer says what it left out.
    left_out_once: Option<String>,
    /// M6: the project's own id (none until it has one), and which run of REAPER this is.
    project_id: Option<String>,
    run: u32,
    /// Tracks a digest page holds (all of them unless a test says).
    digest_tracks: usize,
    /// (m6-places) The project's own settings the fixed phrases change: `bpm`, `loop` {start, end} in beats, `repeat`,
    /// `metronome`.
    settings: Value,
    /// (m6-places) The project row's id, file and identity, when a test set them (else the default unsaved-id project).
    project: Option<Value>,
    /// Whether REAPER is playing (its transport, and what a capture needs).
    playing: bool,
    /// Each track's FX chain, by the track's ref ("r:track:master" is the first row's): M5's listening reads and sets them.
    fx: std::collections::HashMap<String, Vec<SimFx>>,
    next_fx: u32,
    /// The most rows a page holds, whatever it's asked for: a busy REAPER's pages stop at their time (`short_pages`).
    page_rows: Option<usize>,
    /// MIDI items a test adds (`add_midi_item`): groove reads their notes and moves them with notes-change.
    midi: Vec<SimMidi>,
}

/// A MIDI item of the sim's: its GUID and its active take's, its track, name and place in beats, and its notes (pitch,
/// start from the item's start, length, velocity), kept in order of their start as REAPER sorts them.
#[derive(Debug, Clone)]
struct SimMidi {
    guid: String,
    take: String,
    track: String,
    name: String,
    position: f64,
    length: f64,
    notes: Vec<(i64, f64, f64, i64)>,
}

impl SimMidi {
    fn row(&self) -> Value {
        json!({"ref": format!("r:item:{}", self.guid), "trackRef": self.track, "position": self.position / 2., "length": self.length / 2.,
            "positionBeats": self.position, "lengthBeats": self.length, "name": self.name, "takeRef": format!("r:take:{}", self.take),
            "takes": 1, "source": daw_bridge::contract::MIDI_SOURCE, "muted": false})
    }
    /// Its take's row, as the extension reads one take.
    fn take_row(&self) -> Value {
        json!({"ref": format!("r:take:{}", self.take), "name": self.name, "source": daw_bridge::contract::MIDI_SOURCE, "fxCount": 0, "noteCount": self.notes.len(), "ccCount": 0, "ticksPerQuarter": 960.0, "tickAtStart": 0.0,
            "volumeDb": 0.0, "pan": 0.0, "pitch": 0.0, "playrate": 1.0, "preservePitch": false, "startOffset": 0.0})
    }
    fn note_rows(&self) -> Vec<Value> {
        self.notes
            .iter()
            .enumerate()
            .map(|(index, (pitch, start, length, velocity))| {
                json!({"index": index, "pitch": pitch, "start": start, "length": length, "velocity": velocity, "channel": 1, "selected": false, "muted": false})
            })
            .collect()
    }
    fn json(&self) -> Value {
        json!(self.notes.iter().map(|(pitch, start, length, velocity)| json!([pitch, start, length, velocity])).collect::<Vec<_>>())
    }
    fn set(&mut self, notes: &Value) {
        self.notes = notes
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|note| Some((note.get(0)?.as_i64()?, note.get(1)?.as_f64()?, note.get(2)?.as_f64()?, note.get(3)?.as_i64()?)))
            .collect();
        self.notes.sort_by(|a, b| a.1.total_cmp(&b.1));
    }
}

/// An FX of the sim's: its GUID, name and parameters (normalized values; each shown by its kind).
#[derive(Debug, Clone)]
pub struct SimFx {
    pub guid: String,
    pub name: String,
    /// The plug-in's own name once the producer renamed the FX (REAPER's `fx_name`).
    pub original: Option<String>,
    pub params: Vec<(String, f64, ParamShows)>,
    /// An input FX (the master's: its monitoring FX), which no render holds.
    pub input: bool,
    /// A plug-in from outside REAPER, as the extension's rows say (`plugin`), and whether it is an instrument (its type
    /// ends in "i": "VSTi: …").
    pub plugin: bool,
}

/// How an FX parameter of the sim's shows its normalized value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamShows {
    /// dB, linear from `min` to `max` dB across 0..1, shown bare ("6.0", as a JSFX shows it) or with its unit.
    Db { min: f64, max: f64, unit: bool },
    /// Hz, logarithmic from 20 Hz to 24 kHz.
    Hz,
    /// A plain number from `min` to `max`.
    Plain { min: f64, max: f64 },
}

impl ParamShows {
    pub fn shown(self, normalized: f64) -> f64 {
        match self {
            ParamShows::Db { min, max, .. } | ParamShows::Plain { min, max } => min + (max - min) * normalized,
            ParamShows::Hz => 20. * (24_000f64 / 20.).powf(normalized),
        }
    }
    fn text(self, normalized: f64) -> String {
        let value = self.shown(normalized);
        match self {
            ParamShows::Db { unit: true, .. } => format!("{value:.1} dB"),
            ParamShows::Db { unit: false, .. } | ParamShows::Plain { .. } => format!("{value:.1}"),
            ParamShows::Hz => format!("{value:.0} Hz"),
        }
    }
    /// The normalized value a text means (its leading number), clamped to the range.
    fn normalized(self, text: &str) -> Option<f64> {
        let number: String = text.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
        let value: f64 = number.parse().ok()?;
        Some(match self {
            ParamShows::Db { min, max, .. } | ParamShows::Plain { min, max } => ((value - min) / (max - min)).clamp(0., 1.),
            ParamShows::Hz => ((value.max(20.) / 20.).ln() / (24_000f64 / 20.).ln()).clamp(0., 1.),
        })
    }
}

/// The FX the sim knows by name, as REAPER's would be (names as the M5 lab measured them).
fn sim_fx(name: &str, guid: String) -> Option<SimFx> {
    let db = |min: f64, max: f64, unit: bool| ParamShows::Db { min, max, unit };
    let params: Vec<(String, f64, ParamShows)> = match name {
        "VST: ReaLimit (Cockos)" | "ReaLimit" => {
            vec![("Threshold".into(), 1., db(-60., 0., true)), ("Ceiling".into(), 0.99, db(-20., 0., true))]
        }
        // (added at +6 dB, as REAPER 7.82 adds it: measured)
        "JS: Volume Adjustment" | "Volume Adjustment" => {
            vec![("Adjustment (dB)".into(), 0.52, db(-150., 150., false)), ("Max Volume (dB)".into(), 0.5, db(-150., 150., false))]
        }
        "VST: ReaEQ (Cockos)" | "ReaEQ" => {
            let mut params = vec![];
            for (band, freq) in [("Low Shelf", 0.25), ("Band 2", 0.45), ("Band 3", 0.65), ("High Shelf 4", 0.85)] {
                params.push((format!("Freq-{band}"), freq, ParamShows::Hz));
                params.push((format!("Gain-{band}"), 0.5, db(-24., 24., true)));
                params.push((format!("BW-{band}"), 0.25, ParamShows::Plain { min: 0.01, max: 4. }));
            }
            params.push(("Gain".into(), 0.5, db(-24., 24., true)));
            params
        }
        _ => return None,
    };
    // (REAPER lists these three last on every FX)
    let mut params = params;
    params.extend([
        ("Bypass".to_string(), 0., ParamShows::Plain { min: 0., max: 1. }),
        ("Wet".to_string(), 1., ParamShows::Plain { min: 0., max: 1. }),
        ("Delta".to_string(), 0., ParamShows::Plain { min: 0., max: 1. }),
    ]);
    let name = match name {
        "ReaLimit" => "VST: ReaLimit (Cockos)",
        "Volume Adjustment" => "JS: Volume Adjustment",
        "ReaEQ" => "VST: ReaEQ (Cockos)",
        other => other,
    };
    Some(SimFx { guid, name: name.into(), original: None, params, input: false, plugin: false })
}

/// A project that answers like the extension. Clones share the project.
#[derive(Clone)]
pub struct ReaperScript {
    state: Arc<Mutex<State>>,
    listening: SimListening,
}

fn track_row(index: usize, name: &str) -> Value {
    let mut row = json!({
        "ref": format!("r:track:{{T{index:04}}}"),
        "index": index,
        "name": name,
        "depth": 0,
        "folder": 0,
        "color": null,
        "mute": false,
        "solo": false,
        "arm": false,
        "volumeDb": 0.0,
        "pan": 0.0,
        "fxCount": 0,
        "itemCount": 1,
        "sends": 0,
        "receives": 0,
        "selected": index == 1,
    });
    // (as the extension marks it)
    if index == 0 {
        row["master"] = true.into();
    }
    row
}

/// Two of a field's values are the same: numbers by their value (-3 and -3.0), anything else exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

fn item_row(track: &str) -> Value {
    json!({"ref": format!("r:item:{{I-{}}}", &track[9..track.len() - 1]), "trackRef": track, "position": 0.0, "length": 2.0, "name": "loop.wav", "source": "wav"})
}

impl ReaperScript {
    /// A project of `tracks` tracks (the master first) named `Track 1`…, each `name_len` characters longer than its number.
    pub fn new(tracks: usize, name_len: usize) -> Self {
        let rows = (0..=tracks)
            .map(|index| {
                let name = if index == 0 { "MASTER".to_string() } else { format!("Track {index}{}", "x".repeat(name_len)) };
                track_row(index, &name)
            })
            .collect();
        ReaperScript {
            state: Arc::new(Mutex::new(State {
                epoch: 1,
                tracks: rows,
                changed: 7,
                keeps_bound: true,
                asked: vec![],
                undo: vec![],
                open_step: None,
                busy_once: false,
                uncertain_once: false,
                uncertain_restore_once: false,
                left_out_once: None,
                project_id: None,
                run: 1,
                digest_tracks: usize::MAX,
                settings: json!({"bpm": 120.0, "loop": {"start": 0.0, "end": 0.0}, "repeat": false, "metronome": false}),
                project: None,
                playing: false,
                fx: std::collections::HashMap::new(),
                next_fx: 0,
                page_rows: None,
                midi: vec![],
            })),
            listening: SimListening::new(daw_bridge::Daw::Reaper),
        }
    }

    /// M6: the project keeps a Kumi id (it's a saved project with a place of its own).
    pub fn set_project_id(&self, id: Option<&str>) {
        self.state.lock().unwrap().project_id = id.map(str::to_owned);
    }

    /// REAPER quit and started again with the same project: another run, so another identity and epoch.
    pub fn restart(&self) {
        let mut state = self.state.lock().unwrap();
        state.run += 1;
        state.epoch += 1;
    }

    pub fn set_tempo(&self, bpm: f64) {
        self.set_setting("bpm", bpm.into());
    }

    /// The producer renames a track.
    pub fn rename(&self, index: usize, name: &str) {
        self.set_track(index, "name", name.into());
    }

    /// M6: the producer adds an FX of any name to a track (its place; the master is 0), with its parameters (name,
    /// normalized value, shown as a plain number): its GUID.
    pub fn add_fx_with(&self, track: usize, name: &str, params: &[(&str, f64)]) -> String {
        let mut state = self.state.lock().unwrap();
        state.next_fx += 1;
        let guid = format!("{{F{:04}}}", state.next_fx);
        let reference = state.tracks[track]["ref"].as_str().unwrap_or("").to_owned();
        let params = params.iter().map(|(n, v)| (n.to_string(), *v, ParamShows::Plain { min: 0., max: 1. })).collect();
        state.fx.entry(reference).or_default().push(SimFx {
            guid: guid.clone(),
            name: name.into(),
            original: None,
            params,
            input: false,
            plugin: false,
        });
        state.changed += 1;
        guid
    }

    /// A plug-in the sim doesn't otherwise know (a limiter from outside REAPER, an instrument: "VSTi: …") on a track's chain,
    /// by the track's ref: its ref.
    pub fn add_plugin_fx(&self, track: &str, name: &str) -> String {
        let mut state = self.state.lock().unwrap();
        state.next_fx += 1;
        let guid = format!("{{F{:04}}}", state.next_fx);
        state.fx.entry(track.to_owned()).or_default().push(SimFx {
            guid: guid.clone(),
            name: name.into(),
            original: None,
            params: vec![],
            input: false,
            plugin: true,
        });
        state.changed += 1;
        format!("r:fx:{guid}")
    }

    /// The producer renames an FX (its ref): its row says its own name as `originalName` from then on.
    pub fn rename_fx(&self, reference: &str, name: &str) {
        let mut state = self.state.lock().unwrap();
        let guid = reference.strip_prefix("r:fx:").unwrap_or(reference);
        if let Some(fx) = state.fx.values_mut().flatten().find(|fx| fx.guid == guid) {
            let own = std::mem::replace(&mut fx.name, name.into());
            fx.original.get_or_insert(own);
        }
        state.changed += 1;
    }

    /// M6: the producer moves an FX's parameter (its GUID, the parameter's index).
    pub fn set_param(&self, guid: &str, index: usize, value: f64) {
        let mut state = self.state.lock().unwrap();
        if let Some(fx) = state.fx.values_mut().flatten().find(|fx| fx.guid == guid) {
            fx.params[index].1 = value;
        }
        state.changed += 1;
    }

    /// The producer adds a track at the end: its place.
    pub fn add_named_track(&self, name: &str) -> usize {
        let mut state = self.state.lock().unwrap();
        let index = state.tracks.len();
        state.tracks.push(track_row(index, name));
        state.changed += 1;
        index
    }

    /// A digest page holds this many tracks (with their FX).
    pub fn digest_pages_of(&self, tracks: usize) {
        self.state.lock().unwrap().digest_tracks = tracks.max(1);
    }

    /// The bridge's listening side: renders, captures, previews, scratch copies.
    pub fn listening(&self) -> &SimListening {
        &self.listening
    }

    /// An FX the producer puts on a track (by a name the sim knows: ReaEQ, ReaLimit, JS Volume Adjustment): its ref.
    pub fn add_fx(&self, track: &str, name: &str) -> String {
        let mut state = self.state.lock().unwrap();
        state.next_fx += 1;
        let fx = sim_fx(name, format!("{{F{:04}}}", state.next_fx)).expect("an FX the sim knows");
        let reference = format!("r:fx:{}", fx.guid);
        state.fx.entry(track.to_owned()).or_default().push(fx);
        state.changed += 1;
        reference
    }

    /// An FX the producer puts first on a track's chain: its ref.
    pub fn insert_fx_first(&self, track: &str, name: &str) -> String {
        let reference = self.add_fx(track, name);
        let mut state = self.state.lock().unwrap();
        if let Some(chain) = state.fx.get_mut(track) {
            let fx = chain.pop().expect("the FX just added");
            chain.insert(0, fx);
        }
        reference
    }

    /// The producer deletes an FX (its ref).
    pub fn remove_fx(&self, reference: &str) {
        let mut state = self.state.lock().unwrap();
        let guid = reference.strip_prefix("r:fx:").unwrap_or(reference).to_owned();
        for chain in state.fx.values_mut() {
            chain.retain(|fx| fx.guid != guid);
        }
        state.changed += 1;
    }

    /// An input FX the producer puts on a track (on the master, a monitoring FX): its ref.
    pub fn add_input_fx(&self, track: &str, name: &str) -> String {
        let reference = self.add_fx(track, name);
        let mut state = self.state.lock().unwrap();
        if let Some(fx) = state.fx.get_mut(track).and_then(|chain| chain.last_mut()) {
            fx.input = true;
        }
        reference
    }

    /// A track's FX chain, by name, in order.
    pub fn fx_names(&self, track: &str) -> Vec<String> {
        self.state.lock().unwrap().fx.get(track).map(|chain| chain.iter().map(|fx| fx.name.clone()).collect()).unwrap_or_default()
    }

    /// An FX parameter's normalized value, by the FX's name on a track and the parameter's.
    pub fn fx_param(&self, track: &str, fx: &str, param: &str) -> Option<f64> {
        let state = self.state.lock().unwrap();
        let chain = state.fx.get(track)?;
        let found = chain.iter().find(|found| found.name.contains(fx))?;
        found.params.iter().find(|(name, ..)| name == param).map(|(_, value, _)| *value)
    }

    /// A MIDI item the producer has on a track (notes as pitch, start from its start, length, velocity, in beats): its ref
    /// and its take's.
    pub fn add_midi_item(&self, track: &str, name: &str, position: f64, length: f64, notes: &[(i64, f64, f64, i64)]) -> (String, String) {
        let mut state = self.state.lock().unwrap();
        let n = state.midi.len() + 1;
        let mut item = SimMidi {
            guid: format!("{{M{n:04}}}"),
            take: format!("{{K{n:04}}}"),
            track: track.into(),
            name: name.into(),
            position,
            length,
            notes: notes.to_vec(),
        };
        item.notes.sort_by(|a, b| a.1.total_cmp(&b.1));
        let refs = (format!("r:item:{}", item.guid), format!("r:take:{}", item.take));
        state.midi.push(item);
        refs
    }

    /// A MIDI item's notes now (pitch, start, length, velocity), by its ref or its take's.
    pub fn midi_notes(&self, reference: &str) -> Vec<(i64, f64, f64, i64)> {
        let state = self.state.lock().unwrap();
        Self::find_midi(&state, reference).map(|at| state.midi[at].notes.clone()).unwrap_or_default()
    }

    /// Pages of at most `rows` rows, as a busy REAPER's are.
    pub fn short_pages(&self, rows: usize) {
        self.state.lock().unwrap().page_rows = Some(rows);
    }

    /// The producer starts or stops REAPER: its transport says so, and a capture can hear it.
    pub fn set_playing(&self, playing: bool) {
        self.state.lock().unwrap().playing = playing;
        self.listening.set_playing(playing);
    }

    /// The project switched tabs: refs and cursors from before are stale.
    pub fn switch_tab(&self) {
        let mut state = self.state.lock().unwrap();
        state.epoch += 1;
        state.changed += 1;
    }

    /// The sim stops keeping its pages inside the frame bound, as a bridge that doesn't know it would.
    pub fn ignore_the_frame_bound(&self) {
        self.state.lock().unwrap().keeps_bound = false;
    }

    /// The producer moves a track's fader (`index` counts the master as 0).
    pub fn set_volume(&self, index: usize, db: f64) {
        let mut state = self.state.lock().unwrap();
        state.tracks[index]["volumeDb"] = db.into();
        state.changed += 1;
    }

    pub fn volume(&self, index: usize) -> f64 {
        self.state.lock().unwrap().tracks[index]["volumeDb"].as_f64().unwrap()
    }

    /// REAPER's undo history, as the changes named their steps.
    pub fn undo_history(&self) -> Vec<String> {
        self.state.lock().unwrap().undo.clone()
    }

    /// Every request the sim answered, in order.
    pub fn asked(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().asked.clone()
    }

    pub fn respond(&self) -> Respond {
        let script = self.clone();
        Arc::new(move |method, params| script.answer(method, params))
    }

    /// The simulator's config for REAPER with this project behind it.
    pub fn config(&self, secret: Secret, resource: &str) -> SimulatorConfig {
        let mut config = SimulatorConfig::reaper(secret, resource).with_methods(
            &[
                "read",
                "snapshot",
                "focus",
                "focus_body",
                "change_preview",
                "change_apply",
                "change_restore",
                "undo_step_begin",
                "undo_step_end",
                "digest",
            ],
            self.respond(),
        );
        config.offer.methods.extend(LISTENING_METHODS.iter().map(|m| m.to_string()));
        let listening = self.listening.clone();
        config.status_more = Some(Arc::new(move || {
            let mut more = serde_json::Map::new();
            more.insert("listening".into(), listening.flags());
            more
        }));
        config
    }

    /// The kinds the sim makes, the way the contract has every kind made: a preview that writes nothing, an apply
    /// fenced by what the preview saw, a restore fenced by what the apply wrote. A track's volume, mute or name (one
    /// field each), and a track added at the end (its restore deletes it).
    fn change(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: &str| ("bad-params".to_string(), message.to_string());
        if method == "change_preview" && std::mem::take(&mut state.busy_once) {
            return Err(("busy".into(), "a dialog is open; nothing changed".into()));
        }
        let kind = params["kind"].as_str().unwrap_or_default();
        if kind == "track-add" {
            return Self::add_track(state, epoch, method, params);
        }
        if kind == "notes-change" {
            return Self::notes_change(state, epoch, method, params);
        }
        if matches!(kind, "item-variation" | "item-variation-set" | "items-delete") {
            return Self::variation_change(state, epoch, method, kind, params);
        }
        if matches!(kind, "fx-add" | "fx-param" | "fx-params" | "fx-delete") {
            return Self::fx_change(state, epoch, method, params);
        }
        if kind == "run-action" {
            return Self::run_action(state, epoch, method, params);
        }
        if matches!(kind, "tempo-set" | "loop-range" | "loop-repeat" | "metronome-switch") {
            return Self::setting(state, epoch, method, params);
        }
        let field = match kind {
            "track-volume" => "volumeDb",
            "track-mute" => "mute",
            "track-solo" => "solo",
            "track-arm" => "arm",
            "track-rename" => "name",
            _ => {
                return Err(bad("the simulator makes only track-volume, -mute, -solo, -arm, -rename and -add, and the project's settings"))
            }
        };
        let (reference, input) = match method {
            "change_restore" => (params["target"]["ref"].as_str(), &params["prior"]),
            _ => (params["input"]["ref"].as_str(), &params["input"]),
        };
        let index = state
            .tracks
            .iter()
            .position(|track| Some(track["ref"].as_str().unwrap_or_default()) == reference)
            .ok_or(("failed".to_string(), "that track isn't in the project".to_string()))?;
        let now = state.tracks[index][field].clone();
        let name = state.tracks[index]["name"].as_str().unwrap_or_default().to_string();
        let wanted = input.get(field).filter(|value| !value.is_null()).cloned().ok_or(bad(&format!("{field} is missing")))?;
        let title = match field {
            "volumeDb" => format!("{name} volume {} dB", wanted.as_f64().unwrap_or_default()),
            "mute" if wanted == true => format!("{name} muted"),
            "mute" => format!("{name} unmuted"),
            "solo" if wanted == true => format!("{name} soloed"),
            "solo" => format!("{name} unsoloed"),
            "arm" if wanted == true => format!("{name} armed"),
            "arm" => format!("{name} disarmed"),
            _ => format!("Renamed track “{name}” → “{}”", wanted.as_str().unwrap_or_default()),
        };
        match method {
            "change_preview" => Ok(json!({
                "epoch": epoch,
                "target": {"ref": reference, "kind": "track"},
                "prior": {field: now},
                "fence": {field: now},
                "display": {"title": title, "unchanged": same(&now, &wanted)},
            })),
            "change_apply" => {
                if !same(&params["fence"][field], &now) {
                    return Err(("changed".into(), format!("{name} changed since the preview")));
                }
                let desc = params["desc"].as_str().ok_or(bad("desc must be a string"))?.to_string();
                state.tracks[index][field] = wanted.clone();
                state.changed += 1;
                Self::step(state, &desc);
                if std::mem::take(&mut state.uncertain_once) {
                    return Err(("uncertain".into(), "REAPER didn't finish the change in time; it may have been made".into()));
                }
                Ok(
                    json!({"epoch": epoch, "applied": {field: wanted}, "display": {"title": title}, "undo": {"desc": desc, "entry": true, "stateCount": state.changed}}),
                )
            }
            _ => {
                if same(&now, &wanted) {
                    return Ok(json!({"epoch": epoch, "restored": {field: wanted}, "display": {"title": title}}));
                }
                if !same(&params["applied"][field], &now) && params["discard"] != true {
                    return Err(("changed".into(), format!("{name} changed since Kumi set it")));
                }
                let desc = params["desc"].as_str().ok_or(bad("desc must be a string"))?.to_string();
                state.tracks[index][field] = wanted.clone();
                state.changed += 1;
                state.undo.push(desc);
                if std::mem::take(&mut state.uncertain_restore_once) {
                    return Err(("uncertain".into(), "REAPER didn't finish putting it back in time; it may have been".into()));
                }
                let mut display = json!({"title": format!("{title} (put back)")});
                if let Some(note) = state.left_out_once.take() {
                    display["note"] = note.into();
                }
                Ok(json!({"epoch": epoch, "restored": {field: wanted}, "display": display}))
            }
        }
    }

    /// (m6-places) The project's settings, each fenced by its value as the preview saw it: the tempo (`tempo-set {bpm}`)
    /// and the loop (`loop-range {startBeats, endBeats}`), steps of REAPER's undo; repeat and the metronome (`loop-repeat`,
    /// `metronome-switch {on}`), which REAPER keeps no history of.
    fn setting(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: &str| ("bad-params".to_string(), message.to_string());
        let kind = params["kind"].as_str().unwrap_or_default();
        let field = match kind {
            "tempo-set" => "bpm",
            "loop-range" => "loop",
            "loop-repeat" => "repeat",
            _ => "metronome",
        };
        let wanted = match method {
            "change_restore" => params["prior"][field].clone(),
            _ => match kind {
                "tempo-set" => params["input"]["bpm"].clone(),
                "loop-range" => json!({"start": params["input"]["startBeats"], "end": params["input"]["endBeats"]}),
                _ => params["input"]["on"].clone(),
            },
        };
        if wanted.is_null() || (kind == "loop-range" && (wanted["start"].is_null() || wanted["end"].is_null())) {
            return Err(bad(&format!("{kind}: its input is missing")));
        }
        let on_off = |value: &Value| if value == &Value::Bool(true) { "on" } else { "off" };
        let title = match kind {
            "tempo-set" => format!("Tempo {} BPM", wanted.as_f64().unwrap_or_default()),
            "loop-range" => format!("Loop {} to {} beats", wanted["start"], wanted["end"]),
            "loop-repeat" => format!("Repeat {}", on_off(&wanted)),
            _ => format!("Metronome {}", on_off(&wanted)),
        };
        let now = state.settings[field].clone();
        let same = |a: &Value, b: &Value| a == b || (a.as_f64().is_some() && a.as_f64() == b.as_f64());
        // REAPER keeps no history of repeat or the metronome: their changes make no step.
        let entry = matches!(kind, "tempo-set" | "loop-range");
        match method {
            "change_preview" => Ok(json!({
                "epoch": epoch,
                "target": {"ref": null, "kind": "project"},
                "prior": {field: now},
                "fence": {field: now},
                "display": {"title": title, "unchanged": same(&now, &wanted)},
            })),
            "change_apply" => {
                if !same(&params["fence"][field], &now) {
                    return Err(("changed".into(), format!("the project's {field} changed since the preview")));
                }
                let desc = params["desc"].as_str().unwrap_or_default().to_string();
                state.settings[field] = wanted.clone();
                state.changed += 1;
                if entry {
                    Self::step(state, &desc);
                }
                Ok(json!({"epoch": epoch, "applied": {field: wanted}, "display": {"title": title}, "undo": {"desc": desc, "entry": entry}}))
            }
            _ => {
                if same(&now, &wanted) {
                    return Ok(
                        json!({"epoch": epoch, "restored": {field: wanted}, "display": {"title": format!("{title} (put back)"), "note": "It was back already: nothing was written."}}),
                    );
                }
                if !same(&params["applied"][field], &now) && params["discard"] != true {
                    return Err(("changed".into(), format!("the project's {field} changed in REAPER since Kumi set it")));
                }
                state.settings[field] = wanted.clone();
                state.changed += 1;
                if entry {
                    state.undo.push(params["desc"].as_str().unwrap_or_default().to_string());
                }
                Ok(json!({"epoch": epoch, "restored": {field: wanted}, "display": {"title": format!("{title} (put back)")}}))
            }
        }
    }

    /// (m6-places) The project's settings as Kumi's changes left them: `bpm`, `loop`, `repeat`, `metronome`.
    pub fn settings(&self) -> Value {
        self.state.lock().unwrap().settings.clone()
    }

    /// The producer changes one of the project's settings in REAPER.
    pub fn set_setting(&self, field: &str, value: Value) {
        let mut state = self.state.lock().unwrap();
        state.settings[field] = value;
        state.changed += 1;
    }

    /// `track-add {name?}`: a track at the end, its ref in `created`; its restore deletes it, unless it's gone.
    fn add_track(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let name = params["input"]["name"].as_str().unwrap_or("").to_string();
        match method {
            "change_preview" => Ok(json!({
                "epoch": epoch,
                "target": {"ref": null, "kind": "track"},
                "prior": {"tracks": state.tracks.len() - 1},
                "fence": {"tracks": state.tracks.len() - 1},
                "display": {"title": format!("Added track “{name}”")},
            })),
            "change_apply" => {
                let desc = params["desc"].as_str().unwrap_or_default().to_string();
                let index = state.tracks.len();
                let row = track_row(index + 900, &name);
                let made = row["ref"].clone();
                state.tracks.push(row);
                state.changed += 1;
                Self::step(state, &desc);
                Ok(
                    json!({"epoch": epoch, "applied": {"added": true}, "created": {"ref": made, "kind": "track"}, "display": {"title": format!("Added track “{name}”")}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            _ => {
                let made = params["created"]["ref"].as_str().unwrap_or_default();
                let Some(index) = state.tracks.iter().position(|track| track["ref"] == made) else {
                    return Err(("gone".into(), "the track Kumi added is gone".into()));
                };
                state.tracks.remove(index);
                state.changed += 1;
                state.undo.push(params["desc"].as_str().unwrap_or_default().to_string());
                Ok(json!({"epoch": epoch, "restored": {"added": false}, "display": {"title": format!("“{name}” taken away again")}}))
            }
        }
    }

    /// The MIDI item a ref names: the item's, or its take's.
    fn find_midi(state: &State, reference: &str) -> Option<usize> {
        let guid = reference.strip_prefix("r:item:").or(reference.strip_prefix("r:take:"))?;
        state.midi.iter().position(|item| item.guid == guid || item.take == guid)
    }

    /// `notes-change {ref, changes: [{index, start?, length?, velocity?, pitch?}]}`: the take's notes changed by index (a
    /// start moves a note, keeping its length), fenced on the whole note list and put back by it, as the extension does.
    fn notes_change(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: String| ("bad-params".to_string(), message);
        let reference = if method == "change_restore" { params["target"]["ref"].as_str() } else { params["input"]["ref"].as_str() };
        let at = Self::find_midi(state, reference.unwrap_or_default()).ok_or(("gone".to_string(), "that take isn't there".to_string()))?;
        let desc = params["desc"].as_str().unwrap_or_default().to_string();
        let item = state.midi[at].clone();
        let title = format!("Changed {} notes in take “{}”", params["input"]["changes"].as_array().map_or(0, Vec::len), item.name);
        // The notes after the change: by index into the take's notes as they are, sorted by start again.
        let after = || -> Result<SimMidi, (String, String)> {
            let mut after = item.clone();
            for change in params["input"]["changes"].as_array().into_iter().flatten() {
                let index = change["index"].as_u64().ok_or_else(|| bad("index".into()))? as usize;
                let note = after.notes.get_mut(index).ok_or_else(|| bad(format!("there's no note {index}")))?;
                if let Some(start) = change["start"].as_f64() {
                    note.1 = start;
                }
                if let Some(length) = change["length"].as_f64() {
                    note.2 = length;
                }
                if let Some(velocity) = change["velocity"].as_i64() {
                    note.3 = velocity;
                }
                if let Some(pitch) = change["pitch"].as_i64() {
                    note.0 = pitch;
                }
            }
            after.notes.sort_by(|a, b| a.1.total_cmp(&b.1));
            Ok(after)
        };
        match method {
            "change_preview" => {
                let after = after()?;
                Ok(
                    json!({"epoch": epoch, "target": {"ref": format!("r:take:{}", item.take), "kind": "take"}, "prior": {"notes": item.json()},
                    "fence": {"notes": item.json()}, "display": {"title": title, "unchanged": item.notes == after.notes}}),
                )
            }
            "change_apply" => {
                if params["fence"]["notes"] != item.json() {
                    return Err(("changed".into(), "the take's notes changed since the preview".into()));
                }
                let after = after()?.json();
                state.midi[at].set(&after);
                state.changed += 1;
                Self::step(state, &desc);
                Ok(
                    json!({"epoch": epoch, "applied": {"notes": state.midi[at].json()}, "display": {"title": title}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            _ => {
                if params["applied"]["notes"] != item.json() && params["discard"] != true {
                    return Err(("changed".into(), "the take's notes changed in REAPER since Kumi changed them".into()));
                }
                state.midi[at].set(&params["prior"]["notes"]);
                state.changed += 1;
                state.undo.push(desc);
                Ok(json!({"epoch": epoch, "restored": {"notes": state.midi[at].json()}, "display": {"title": "Notes put back"}}))
            }
        }
    }

    /// `item-variation {ref, position, length, name, notes}` (a copy of a MIDI item with its own notes), `item-variation-set
    /// {ref, length, notes}` and `items-delete {refs}`: the variations tool's takes, as the extension makes them. Not undone here.
    fn variation_change(state: &mut State, epoch: &str, method: &str, kind: &str, params: &Value) -> Result<Value, (String, String)> {
        let input = &params["input"];
        let gone = || ("gone".to_string(), "that item isn't there".to_string());
        let beats = |key: &str| input[key].as_f64().map(|seconds| seconds * 2.);
        let notes = || -> Vec<(i64, f64, f64, i64)> {
            input["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|note| {
                    Some((
                        note["pitch"].as_i64()?,
                        note["start"].as_f64()?,
                        note["length"].as_f64()?,
                        note["velocity"].as_i64().unwrap_or(100),
                    ))
                })
                .collect()
        };
        let title = format!("Kumi's {kind}");
        if method == "change_preview" {
            return Ok(
                json!({"epoch": epoch, "target": {"ref": input["ref"], "kind": "item"}, "prior": {}, "fence": {}, "display": {"title": title}}),
            );
        }
        if method != "change_apply" {
            return Err(("failed".into(), "the simulator doesn't undo a variation".into()));
        }
        let desc = params["desc"].as_str().unwrap_or_default().to_string();
        let mut answer = json!({"epoch": epoch, "applied": {}, "display": {"title": title}, "undo": {"desc": desc, "entry": true}});
        match kind {
            "item-variation" => {
                let from = Self::find_midi(state, input["ref"].as_str().unwrap_or_default()).ok_or_else(gone)?;
                let n = state.midi.len() + 1;
                let mut made = SimMidi { guid: format!("{{M{n:04}}}"), take: format!("{{K{n:04}}}"), ..state.midi[from].clone() };
                made.position = beats("position").unwrap_or(made.position);
                made.length = beats("length").unwrap_or(made.length);
                made.name = input["name"].as_str().unwrap_or_default().to_string();
                made.notes = notes();
                made.notes.sort_by(|a, b| a.1.total_cmp(&b.1));
                answer["created"] = json!({"ref": format!("r:item:{}", made.guid), "kind": "item"});
                state.midi.push(made);
            }
            "item-variation-set" => {
                let at = Self::find_midi(state, input["ref"].as_str().unwrap_or_default()).ok_or_else(gone)?;
                state.midi[at].notes = notes();
                state.midi[at].notes.sort_by(|a, b| a.1.total_cmp(&b.1));
                state.midi[at].length = beats("length").unwrap_or(state.midi[at].length);
            }
            _ => {
                for reference in input["refs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    let at = Self::find_midi(state, reference).ok_or_else(gone)?;
                    state.midi.remove(at);
                }
            }
        }
        state.changed += 1;
        Self::step(state, &desc);
        Ok(answer)
    }

    /// The track an FX ref's chain is on, and the FX's place in it.
    fn find_fx(state: &State, reference: &str) -> Option<(String, usize)> {
        let guid = reference.strip_prefix("r:fx:")?;
        state.fx.iter().find_map(|(track, chain)| chain.iter().position(|fx| fx.guid == guid).map(|at| (track.clone(), at)))
    }

    /// `fx-add {ref: track, name}`, `fx-param {ref: param, value (text) | normalized}`, `fx-delete {ref: fx}`, fenced as the
    /// extension fences them (the parameter's value, the chain's length), each a step of REAPER's undo.
    fn fx_change(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: String| ("bad-params".to_string(), message);
        let kind = params["kind"].as_str().unwrap_or_default();
        let source = if method == "change_restore" { &params["target"] } else { &params["input"] };
        let desc = params["desc"].as_str().unwrap_or_default().to_string();
        match kind {
            "fx-add" => {
                let track = if method == "change_restore" { params["target"]["ref"].as_str() } else { params["input"]["ref"].as_str() };
                let track = track.unwrap_or_default().to_string();
                let name = params["input"]["name"].as_str().or(params["prior"]["name"].as_str()).unwrap_or_default().to_string();
                match method {
                    "change_preview" => {
                        let count = state.fx.get(&track).map_or(0, Vec::len);
                        sim_fx(&name, String::new()).ok_or_else(|| bad(format!("no FX named {name:?}")))?;
                        Ok(
                            json!({"epoch": epoch, "target": {"ref": track, "kind": "track"}, "prior": {"count": count, "name": name}, "fence": {"count": count}, "display": {"title": format!("Added “{name}”")}}),
                        )
                    }
                    "change_apply" => {
                        let count = state.fx.get(&track).map_or(0, Vec::len);
                        if params["fence"]["count"].as_u64() != Some(count as u64) {
                            return Err(("changed".into(), "the chain changed since the preview".into()));
                        }
                        state.next_fx += 1;
                        let fx = sim_fx(&name, format!("{{F{:04}}}", state.next_fx)).ok_or_else(|| bad(format!("no FX named {name:?}")))?;
                        let made = format!("r:fx:{}", fx.guid);
                        let chain = state.fx.entry(track.clone()).or_default();
                        let at = params["input"]["index"].as_u64().map_or(chain.len(), |at| (at as usize).min(chain.len()));
                        chain.insert(at, fx);
                        state.changed += 1;
                        Self::step(state, &desc);
                        Ok(
                            json!({"epoch": epoch, "applied": {"count": count + 1}, "created": {"ref": made, "kind": "fx"}, "display": {"title": format!("Added “{name}”")}, "undo": {"desc": desc, "entry": true}}),
                        )
                    }
                    _ => {
                        let made = params["created"]["ref"].as_str().unwrap_or_default();
                        let Some((on, at)) = Self::find_fx(state, made) else {
                            return Err(("gone".into(), "the FX Kumi added is gone".into()));
                        };
                        state.fx.get_mut(&on).unwrap().remove(at);
                        state.changed += 1;
                        state.undo.push(desc);
                        Ok(json!({"epoch": epoch, "restored": {"count": at}, "display": {"title": "FX taken away again"}}))
                    }
                }
            }
            "fx-param" => {
                let reference = source["ref"].as_str().unwrap_or_default().to_string();
                let (guid, index) = reference
                    .strip_prefix("r:param:")
                    .and_then(|rest| rest.rsplit_once(':'))
                    .ok_or_else(|| bad(format!("{reference} isn't a parameter")))?;
                let index: usize = index.parse().map_err(|_| bad(format!("{reference} isn't a parameter")))?;
                let (track, at) =
                    Self::find_fx(state, &format!("r:fx:{guid}")).ok_or(("gone".to_string(), "that FX isn't there".to_string()))?;
                let (name, now, shows) = state.fx[&track][at].params.get(index).cloned().ok_or_else(|| bad("no such parameter".into()))?;
                let wanted = match method {
                    "change_restore" => params["prior"]["normalized"].as_f64(),
                    _ => params["input"]["normalized"]
                        .as_f64()
                        .or_else(|| params["input"]["value"].as_str().and_then(|text| shows.normalized(text))),
                }
                .ok_or_else(|| bad("normalized or value".into()))?;
                let title = format!("{name} → {}", shows.text(wanted));
                match method {
                    "change_preview" => Ok(
                        json!({"epoch": epoch, "target": {"ref": reference, "kind": "param"}, "prior": {"normalized": now}, "fence": {"normalized": now}, "display": {"title": title, "unchanged": (now - wanted).abs() < 1e-9}}),
                    ),
                    "change_apply" => {
                        if params["fence"]["normalized"].as_f64().is_none_or(|fence| (fence - now).abs() > 1e-9) {
                            return Err(("changed".into(), format!("{name} changed since the preview")));
                        }
                        state.fx.get_mut(&track).unwrap()[at].params[index].1 = wanted;
                        state.changed += 1;
                        Self::step(state, &desc);
                        Ok(
                            json!({"epoch": epoch, "applied": {"normalized": wanted}, "display": {"title": title}, "undo": {"desc": desc, "entry": true}}),
                        )
                    }
                    _ => {
                        if params["applied"]["normalized"].as_f64().is_none_or(|applied| (applied - now).abs() > 1e-9)
                            && params["discard"] != true
                        {
                            return Err(("changed".into(), format!("{name} changed since Kumi set it")));
                        }
                        state.fx.get_mut(&track).unwrap()[at].params[index].1 = wanted;
                        state.changed += 1;
                        state.undo.push(desc);
                        Ok(json!({"epoch": epoch, "restored": {"normalized": wanted}, "display": {"title": format!("{title} (put back)")}}))
                    }
                }
            }
            "fx-params" => Self::fx_params(state, epoch, method, params),
            _ => Err(bad("the sim deletes no FX".into())),
        }
    }

    /// `fx-params {ref: fx, values: [{param: index | name, normalized? | value?}]}`: several parameters of one FX in one step,
    /// fenced and put back as the extension does (m4-r-fxenv): `{guid, chain, params: {"<index>": normalized}}`.
    fn fx_params(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: String| ("bad-params".to_string(), message);
        let near = |a: Option<f64>, b: Option<f64>| matches!((a, b), (Some(a), Some(b)) if (a - b).abs() <= 1e-9);
        let desc = params["desc"].as_str().unwrap_or_default().to_string();
        let guid = match method {
            "change_restore" => params["prior"]["guid"].as_str(),
            _ => params["input"]["ref"].as_str().and_then(|reference| reference.strip_prefix("r:fx:")),
        }
        .ok_or_else(|| bad("ref is an FX".into()))?
        .to_string();
        let (track, at) = Self::find_fx(state, &format!("r:fx:{guid}")).ok_or(("gone".to_string(), "that FX isn't there".to_string()))?;
        let fx = state.fx[&track][at].clone();
        let short = fx.name.split_once(": ").map_or(fx.name.as_str(), |(_, rest)| rest).split(" (").next().unwrap_or_default().to_string();
        let record = |values: serde_json::Map<String, Value>| json!({"guid": guid, "chain": {"trackRef": track, "input": fx.input}, "params": values});
        let now = |state: &State, indexes: &[usize]| -> serde_json::Map<String, Value> {
            indexes.iter().map(|&index| (index.to_string(), json!(state.fx[&track][at].params[index].1))).collect()
        };
        let line = |index: usize, normalized: f64| format!("{} → {}", fx.params[index].0, fx.params[index].2.text(normalized));
        if method == "change_restore" {
            let prior = params["prior"]["params"].as_object().ok_or_else(|| bad("prior.params".into()))?;
            let indexes: Vec<usize> = prior.keys().filter_map(|key| key.parse().ok()).filter(|&index| index < fx.params.len()).collect();
            if indexes.len() != prior.len() {
                return Err(("changed".into(), format!("{short} has fewer parameters than when Kumi set them")));
            }
            let title = format!("{short} {} parameters", indexes.len());
            let here = now(state, &indexes);
            if here.iter().all(|(key, value)| near(value.as_f64(), prior[key].as_f64())) {
                return Ok(json!({"epoch": epoch, "restored": record(here), "display": {"title": format!("{title} (as they were)")}}));
            }
            let moved: Vec<&str> = here
                .iter()
                .filter(|(key, value)| !near(value.as_f64(), params["applied"]["params"][key.as_str()].as_f64()))
                .filter_map(|(key, _)| key.parse::<usize>().ok().map(|index| fx.params[index].0.as_str()))
                .collect();
            if !moved.is_empty() && params["discard"] != true {
                return Err(("changed".into(), format!("{} changed in REAPER since Kumi set it", moved.join(", "))));
            }
            for (key, value) in prior {
                state.fx.get_mut(&track).unwrap()[at].params[key.parse::<usize>().unwrap()].1 = value.as_f64().unwrap_or_default();
            }
            state.changed += 1;
            state.undo.push(desc);
            let after = now(state, &indexes);
            let lines: Vec<String> =
                indexes.iter().map(|&index| line(index, after[&index.to_string()].as_f64().unwrap_or_default())).collect();
            return Ok(
                json!({"epoch": epoch, "restored": record(after), "display": {"title": format!("{title} put back"), "lines": lines}}),
            );
        }
        // What was asked: each parameter by index or exact name, at a normalized value or the text it shows.
        let mut asks: Vec<(usize, f64)> = vec![];
        for value in params["input"]["values"].as_array().into_iter().flatten() {
            let index = match &value["param"] {
                Value::Number(index) => index.as_u64().map(|index| index as usize),
                Value::String(name) => fx.params.iter().position(|(param, ..)| param == name),
                _ => None,
            }
            .filter(|&index| index < fx.params.len())
            .ok_or_else(|| bad(format!("{short} has no parameter {}", value["param"])))?;
            if asks.iter().any(|(seen, _)| *seen == index) {
                return Err(bad(format!("{} is given twice", fx.params[index].0)));
            }
            let wanted = value["normalized"]
                .as_f64()
                .or_else(|| value["value"].as_str().and_then(|text| fx.params[index].2.normalized(text)))
                .ok_or_else(|| bad("normalized or value".into()))?;
            asks.push((index, wanted));
        }
        if asks.is_empty() || asks.len() > 256 {
            return Err(bad("values: 1 to 256".into()));
        }
        let indexes: Vec<usize> = asks.iter().map(|(index, _)| *index).collect();
        let title = format!("{short} {} parameters", asks.len());
        let here = now(state, &indexes);
        match method {
            "change_preview" => {
                let unchanged = asks.iter().all(|(index, wanted)| near(here[&index.to_string()].as_f64(), Some(*wanted)));
                let lines: Vec<String> = asks.iter().map(|&(index, wanted)| line(index, wanted)).collect();
                Ok(json!({"epoch": epoch, "target": {"ref": format!("r:fx:{guid}"), "kind": "fx", "trackRef": track},
                    "prior": record(here.clone()), "fence": record(here), "display": {"title": title, "lines": lines, "unchanged": unchanged}}))
            }
            _ => {
                let fenced = &params["fence"];
                if fenced["chain"] != json!({"trackRef": track, "input": fx.input})
                    || !here.iter().all(|(key, value)| near(value.as_f64(), fenced["params"][key.as_str()].as_f64()))
                {
                    return Err(("changed".into(), "the FX's parameters, or the chain it is in, changed since the preview".into()));
                }
                for &(index, wanted) in &asks {
                    state.fx.get_mut(&track).unwrap()[at].params[index].1 = wanted;
                }
                state.changed += 1;
                Self::step(state, &desc);
                let after = now(state, &indexes);
                let lines: Vec<String> = asks.iter().map(|&(index, wanted)| line(index, wanted)).collect();
                Ok(
                    json!({"epoch": epoch, "applied": record(after), "display": {"title": title, "lines": lines}, "undo": {"desc": desc, "entry": true}}),
                )
            }
        }
    }

    /// (r-tracks) `run-action {action}`: 40001 inserts a track, a change REAPER makes a point of; 40297 unselects every track,
    /// which it makes none of (the answer says so, and Kumi makes no row). Only REAPER's undo takes either back.
    fn run_action(state: &mut State, epoch: &str, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let action = params["input"]["action"].as_i64().unwrap_or_default();
        let name = match action {
            40001 => "Track: Insert new track",
            40297 => "Track: Unselect (clear selection of) all tracks",
            _ => return Err(("bad-params".into(), format!("REAPER has no action {action}"))),
        };
        let title = format!("Ran action “{name}”");
        let note = "Only REAPER's own undo can take this back (Edit > Undo).";
        match method {
            "change_preview" => Ok(
                json!({"epoch": epoch, "target": {"kind": "action", "command": action}, "prior": {}, "fence": {"command": action}, "display": {"title": title, "note": note}}),
            ),
            "change_apply" => {
                let desc = params["desc"].as_str().unwrap_or_default().to_string();
                if action == 40297 {
                    let note = "REAPER made no undo point of it: it changed nothing the project keeps (a view, selection or transport action), so it has no row in HISTORY.";
                    return Ok(
                        json!({"epoch": epoch, "applied": {"command": action, "changed": false}, "display": {"title": title, "note": note, "unchanged": true}, "undo": {"desc": desc, "entry": false}}),
                    );
                }
                let index = state.tracks.len();
                state.tracks.push(track_row(index + 950, ""));
                state.changed += 1;
                Self::step(state, &desc);
                Ok(
                    json!({"epoch": epoch, "applied": {"command": action}, "display": {"title": title, "note": note}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            _ => Err(("daw-only".into(), "Kumi can't take back what an action did: REAPER's own undo can (Edit > Undo)".into())),
        }
    }

    /// A change's step in REAPER's undo: its own, or none inside a plan's (whose label names them all when it closes).
    fn step(state: &mut State, desc: &str) {
        if state.open_step.is_none() {
            state.undo.push(desc.to_string());
        }
    }

    /// `undo_step_begin {label}` / `undo_step_end {stepId, label?}`: one step around a plan's changes, named by the label
    /// it ends with (or began with).
    fn plan_step(state: &mut State, method: &str, params: &Value) -> Result<Value, (String, String)> {
        let label = params.get("label").and_then(Value::as_str).map(str::to_owned);
        if method == "undo_step_begin" {
            state.open_step = Some(label.unwrap_or_default());
            return Ok(json!({"stepId": "s1"}));
        }
        let begun = state.open_step.take().ok_or(("failed".to_string(), "no step is open".to_string()))?;
        let desc = label.unwrap_or(begun);
        state.undo.push(desc.clone());
        Ok(json!({"desc": desc, "entry": true}))
    }

    /// The next apply is made, then answered `uncertain`, as the extension answers a job it took and didn't finish in time.
    pub fn uncertain_once(&self) {
        self.state.lock().unwrap().uncertain_once = true;
    }

    /// The next restore is made, then answered `uncertain`.
    pub fn uncertain_restore_once(&self) {
        self.state.lock().unwrap().uncertain_restore_once = true;
    }

    /// The next restore is made in part, and its answer says what it left out (`display.note`).
    pub fn left_out_once(&self, note: &str) {
        self.state.lock().unwrap().left_out_once = Some(note.into());
    }

    /// A dialog shows for the next change's preview, and closes right after.
    pub fn busy_once(&self) {
        self.state.lock().unwrap().busy_once = true;
    }

    /// The producer presses REAPER's undo: Kumi's last step comes off (its values back as the preview saw them is the
    /// test's business); the description of what came off, for the bridge's `undone_in_daw`.
    pub fn undo_last(&self) -> Option<String> {
        self.state.lock().unwrap().undo.pop()
    }

    pub fn track(&self, index: usize) -> Value {
        self.state.lock().unwrap().tracks[index].clone()
    }

    pub fn set_track(&self, index: usize, field: &str, value: Value) {
        let mut state = self.state.lock().unwrap();
        state.tracks[index][field] = value;
        state.changed += 1;
    }

    fn epoch_text(state: &State) -> String {
        format!("sim-{}", state.epoch)
    }

    fn answer(&self, method: &str, params: &Value) -> Option<Result<Value, (String, String)>> {
        let mut state = self.state.lock().unwrap();
        state.asked.push((method.to_string(), params.clone()));
        let epoch = Self::epoch_text(&state);
        if LISTENING_METHODS.contains(&method) {
            if let Some(sent) = params.get("epoch").and_then(Value::as_str).filter(|sent| *sent != epoch) {
                let message =
                    format!("those refs are from an earlier epoch ({sent}; now {epoch}): the project in the tab changed, read again");
                return Some(Err(("stale".into(), message)));
            }
            let tracks: Vec<String> = state.tracks.iter().filter_map(|row| row["ref"].as_str().map(str::to_owned)).collect();
            drop(state);
            let known = |reference: &str| tracks.iter().any(|track| track == reference);
            return self.listening.answer(method, params, &epoch, &known);
        }
        // Refs that say which epoch they came from are refused once the project in the tab is another.
        if matches!(method, "read" | "focus_body" | "change_preview" | "change_apply" | "change_restore") {
            if let Some(sent) = params.get("epoch").and_then(Value::as_str).filter(|sent| *sent != epoch) {
                let message =
                    format!("those refs are from an earlier epoch ({sent}; now {epoch}): the project in the tab changed, read again");
                return Some(Err(("stale".into(), message)));
            }
        }
        // (M5) A copy kept as a track, muted: the listening side knows the kept copies.
        let muted = if method == "change_restore" { &params["target"]["ref"] } else { &params["input"]["ref"] };
        if method.starts_with("change_")
            && params["kind"] == "track-mute"
            && muted.as_str().is_some_and(|kept| self.listening.is_kept(kept))
        {
            return Some(self.listening.mute_kept(method, params, &epoch));
        }
        // (M5) A scratch copy kept as a track: the listening side knows the copies.
        if method.starts_with("change_") && params["kind"] == "scratch-keep" {
            let answer = self.listening.keep(method, params, &epoch);
            if answer.is_ok() && method != "change_preview" {
                let desc = params["desc"].as_str().unwrap_or_default();
                state.changed += 1;
                if method == "change_apply" {
                    Self::step(&mut state, desc);
                } else {
                    state.undo.push(desc.to_string());
                }
            }
            return Some(answer);
        }
        Some(match method {
            "change_preview" | "change_apply" | "change_restore" => {
                let answer = Self::change(&mut state, &epoch, method, params);
                // The master's JS Volume Adjustment turns what its renders hold, as REAPER's would.
                let gain: f64 = state
                    .fx
                    .get("r:track:master")
                    .into_iter()
                    .flatten()
                    .filter(|fx| fx.name == "JS: Volume Adjustment")
                    .filter_map(|fx| fx.params.first().map(|(_, value, shows)| shows.shown(*value)))
                    .sum();
                self.listening.set_master_gain_db(gain);
                answer
            }
            "undo_step_begin" | "undo_step_end" => Self::plan_step(&mut state, method, params),
            // (r-tracks) Saving: a file written, nothing in the project changed.
            // (as the extension: a save names the epoch its refs and project were read in)
            "project_save" | "track_template_save" if params.get("epoch").and_then(Value::as_str).is_none() => {
                Err(("bad-params".to_string(), "epoch: the epoch the refs were read in".to_string()))
            }
            "project_save" => Ok(json!({"epoch": epoch, "saved": "/lab/Projects/Sim.rpp", "dirty": false})),
            "track_template_save" => Ok(
                json!({"epoch": epoch, "saved": format!("/lab/REAPER/TrackTemplates/{}.RTrackTemplate", params["name"].as_str().unwrap_or("Template"))}),
            ),
            // (#401) a kept script: a file and an action, nothing in the project changed
            "script_install" => Ok(json!({
                "file": format!("/lab/REAPER/Scripts/Kumi/{}.lua", params["slug"].as_str().unwrap_or("script")),
                "command": "_RSlab",
                "where": "\u{201c}Script: lab.lua\u{201d} in REAPER's Action List",
            })),
            "script_remove" => {
                Ok(json!({"removed": format!("/lab/REAPER/Scripts/Kumi/{}.lua", params["slug"].as_str().unwrap_or("script"))}))
            }
            "read" => Self::read(&state, &epoch, params),
            "digest" => Self::digest(&state, &epoch, params),
            "snapshot" => {
                let tracks: Vec<Value> = state.tracks.iter().take(MAX_LIMIT).cloned().collect();
                let mut snapshot = json!({
                    "epoch": epoch,
                    "project": Self::project(&state),
                    // (the transport plays at the project's tempo, as REAPER's reads it)
                    "transport": {"playing": false, "recording": false, "beat": 0.0, "tempo": state.settings["bpm"], "beatsPerBar": 4.0, "seconds": 0.0, "loop": {"on": false, "start": 0.0, "end": 0.0}},
                    "tracks": tracks,
                    "selection": {"trackRef": "r:track:{T0001}", "itemRef": null, "takeRef": null, "fxRef": null, "cursor": 0.0},
                    "markers": [],
                });
                if state.tracks.len() > MAX_LIMIT {
                    snapshot["truncated"] =
                        json!({"tracks": {"total": state.tracks.len(), "nextCursor": format!("c1.{epoch}.{MAX_LIMIT}")}});
                }
                Ok(snapshot)
            }
            "focus" => Ok(json!({
                "epoch": epoch,
                "view": "arrange",
                "selection": {"trackRef": "r:track:{T0001}", "cursor": 0.0},
                "transport": {"playing": state.playing, "recording": false, "beat": 0.0, "tempo": state.settings["bpm"], "beatsPerBar": 4.0, "seconds": 0.0},
                "changed": state.changed,
                "ageMs": 12,
            })),
            "focus_body" => match params.get("kind").and_then(Value::as_str) {
                Some("arrange") => Self::arrange(&state, &epoch, params),
                Some(kind) if BODIES.contains(&kind) => {
                    Ok(json!({"kind": kind, "epoch": epoch, "ref": params.get("ref"), "window": params.get("window")}))
                }
                _ => Err(("bad-params".into(), format!("REAPER has no body of that kind: use one of {}", BODIES.join(", ")))),
            },
            _ => return None,
        })
    }

    /// The arrange body's tracks, paged like the extension's: from the top, from a cursor, or centred on the `around` track.
    fn arrange(state: &State, epoch: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: &str| ("bad-params".to_string(), message.to_string());
        // The tracks without the master, which is the first row.
        let tracks = &state.tracks[1..];
        let limit = params.get("limit").and_then(Value::as_u64).map_or(MAX_ARRANGE, |n| (n as usize).clamp(1, MAX_ARRANGE));
        let start = match (params.get("cursor").and_then(Value::as_str), params.get("around")) {
            (Some(cursor), _) => match cursor.splitn(3, '.').collect::<Vec<_>>().as_slice() {
                ["c1", from, position] if *from == epoch => position.parse::<usize>().map_err(|_| bad("cursor: not one this read gave"))?,
                ["c1", _, _] => return Err(bad("that cursor is from before the project tab changed: read again from the start")),
                _ => return Err(bad("cursor: pass the nextCursor an earlier read gave, unchanged")),
            },
            (None, Some(around)) => {
                let around = around.as_str().filter(|a| !a.is_empty()).ok_or_else(|| bad("around: a track ref"))?;
                if !around.starts_with("r:track:") {
                    return Err(bad("around: a track ref"));
                }
                let place = if around == "r:track:master" {
                    0
                } else {
                    tracks
                        .iter()
                        .position(|row| row["ref"] == around)
                        .ok_or_else(|| ("failed".to_string(), format!("{around} isn't in the project (deleted, or another tab)")))?
                };
                place.saturating_sub(limit / 2).min(tracks.len().saturating_sub(limit))
            }
            (None, None) => 0,
        };
        let shown: Vec<Value> = tracks
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .map(|mut row| {
                row["items"] = json!([]);
                row
            })
            .collect();
        let mut body = json!({
            "kind": "arrange",
            "epoch": epoch,
            "window": params.get("window").cloned().unwrap_or(json!({"from": 0.0, "to": 16.0})),
            "tracks": shown,
            "trackCount": tracks.len(),
            "from": start,
        });
        if start + limit < tracks.len() {
            body["nextCursor"] = format!("c1.{epoch}.{}", start + limit).into();
        }
        Ok(body)
    }

    /// An FX row as the extension reads it.
    fn fx_row(fx: &SimFx, at: usize) -> Value {
        let mut row = json!({"ref": format!("r:fx:{}", fx.guid), "index": at, "name": fx.name, "enabled": true, "offline": false, "plugin": fx.plugin, "paramCount": fx.params.len()});
        // (the extension says so from the FX's type, VSTi, VST3i, AUi, CLAPi, LV2i, DXi: its name starts with the type)
        let own = fx.original.as_deref().unwrap_or(&fx.name);
        if own.split_once(": ").is_some_and(|(kind, _)| kind.ends_with('i')) {
            row["instrument"] = true.into();
        }
        if fx.input {
            row["input"] = true.into();
        }
        if let Some(original) = &fx.original {
            row["originalName"] = original.clone().into();
        }
        row
    }

    /// The project row: another project in the tab (`switch_tab`) is another identity, and so is another run of REAPER.
    fn project(state: &State) -> Value {
        if let Some(project) = &state.project {
            return project.clone();
        }
        let identity = if state.run == 1 { format!("sim:{}", state.epoch) } else { format!("sim{}:{}", state.run, state.epoch) };
        json!({"id": state.project_id, "identity": identity, "name": "Song.rpp", "path": "/lab/Song.rpp", "tempo": state.settings["bpm"]})
    }

    /// M6: a page of the digest, as the extension makes it: each track (its GUID from its ref), then its FX.
    fn digest(state: &State, epoch: &str, params: &Value) -> Result<Value, (String, String)> {
        let values = params.get("values") == Some(&Value::Bool(true));
        let start = match params.get("cursor").and_then(Value::as_str) {
            None => 0,
            Some(cursor) => match cursor.splitn(3, '.').collect::<Vec<_>>().as_slice() {
                ["g1", from, n] if *from == epoch => n.parse::<usize>().map_err(|_| ("bad-params".to_string(), "cursor".to_string()))?,
                ["g1", _, _] => return Err(("stale".into(), "the project in the tab changed since the first page: read again".into())),
                _ => return Err(("bad-params".into(), "cursor: the nextCursor an earlier page gave, unchanged".into())),
            },
        };
        let end = start.saturating_add(state.digest_tracks).min(state.tracks.len());
        let mut records = vec![];
        for (index, row) in state.tracks.iter().enumerate().take(end).skip(start) {
            let reference = row["ref"].as_str().unwrap_or("");
            let id = if index == 0 { "master".to_string() } else { reference.trim_start_matches("r:track:").to_string() };
            let fx: Vec<&SimFx> = state.fx.get(reference).map(|chain| chain.iter().collect()).unwrap_or_default();
            records.push(json!({
                "k": "track",
                "id": id,
                "n": row["name"],
                "o": index,
                "f": {"mixer": {"volume": row["volumeDb"], "mute": row["mute"]}, "FX": {"count": fx.len()}},
            }));
            for (position, fx) in fx.into_iter().enumerate() {
                let print: Vec<String> = fx.params.iter().map(|(_, v, _)| format!("{v:.4}")).collect();
                let mut record = json!({
                    "k": "FX",
                    "id": fx.guid,
                    "n": fx.name,
                    "o": position,
                    "p": id,
                    "f": {"plugin": fx.name, "enabled": true, "parameters": {"count": fx.params.len(), "parametersHash": print.join(",")}},
                });
                if values {
                    record["v"] = fx.params.iter().map(|(_, v, _)| json!(v)).collect::<Vec<_>>().into();
                    record["r"] = format!("r:fx:{}", fx.guid).into();
                }
                records.push(record);
            }
        }
        let mut page = json!({"epoch": epoch, "records": records, "changed": state.changed});
        if end < state.tracks.len() {
            page["nextCursor"] = format!("g1.{epoch}.{end}").into();
        }
        if start == 0 {
            page["project"] = Self::project(state);
            page["facts"] = json!({"tempo": state.settings["bpm"], "timeSignature": [4, 4]});
            page["summaries"] = json!({"FX": "FX"});
            page["bounded"] = json!({});
        }
        Ok(page)
    }

    /// (m6-places) The project in the tab is `name` with this id and file (an unsaved one: no id and an empty path) and this
    /// identity; another identity is another project, whose refs are another epoch's.
    pub fn set_project(&self, id: Option<&str>, path: &str, identity: &str, name: &str) {
        let mut state = self.state.lock().unwrap();
        if state.project.as_ref().is_some_and(|project| project["identity"] != identity) {
            state.epoch += 1;
        }
        state.project =
            Some(json!({"id": id, "identity": identity, "name": name, "path": path, "saved": !path.is_empty(), "tempo": 120.0}));
        state.changed += 1;
    }

    fn read(state: &State, epoch: &str, params: &Value) -> Result<Value, (String, String)> {
        let bad = |message: &str| ("bad-params".to_string(), message.to_string());
        if let Some(reference) = params.get("ref").and_then(Value::as_str) {
            if let Some((owner, at)) = Self::find_fx(state, reference) {
                let mut row = Self::fx_row(&state.fx[&owner][at], at);
                // (a take's FX says its take, and the track its item is on)
                match state.midi.iter().find(|item| format!("r:take:{}", item.take) == owner) {
                    Some(item) => {
                        row["takeRef"] = owner.into();
                        row["trackRef"] = item.track.clone().into();
                    }
                    None => row["trackRef"] = owner.into(),
                }
                return Ok(json!({"items": [row], "epoch": epoch}));
            }
            if let Some(at) = Self::find_midi(state, reference) {
                let row = if reference.starts_with("r:item:") { state.midi[at].row() } else { state.midi[at].take_row() };
                return Ok(json!({"items": [row], "epoch": epoch}));
            }
            return match state.tracks.iter().find(|row| row["ref"] == reference) {
                Some(row) => Ok(json!({"items": [row], "epoch": epoch})),
                None => Err(("failed".into(), format!("{reference} isn't in the project (deleted, or another tab)"))),
            };
        }
        let kind = params.get("kind").and_then(Value::as_str).ok_or_else(|| bad("kind: one of the kinds"))?;
        if !KINDS.contains(&kind) {
            return Err(bad(&format!("REAPER has no kind {kind:?} to read")));
        }
        let limit = params.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT));
        let limit = state.page_rows.map_or(limit, |rows| limit.min(rows));
        let start = match params.get("cursor").and_then(Value::as_str) {
            None => 0,
            Some(cursor) => {
                let mut parts = cursor.splitn(3, '.');
                match (parts.next(), parts.next(), parts.next().and_then(|n| n.parse::<usize>().ok())) {
                    (Some("c1"), Some(from), Some(position)) if from == epoch => position,
                    (Some("c1"), Some(_), Some(_)) => {
                        return Err(bad("that cursor is from before the project tab changed: read again from the start"))
                    }
                    _ => return Err(bad("cursor: pass the nextCursor an earlier read gave, unchanged")),
                }
            }
        };
        let all: Vec<Value> = match (kind, params.get("parent").and_then(Value::as_str)) {
            ("project", _) => vec![Self::project(state)],
            ("track", _) => state.tracks.clone(),
            // Each track plays its MIDI items where they are, else its one item, from the start for two bars.
            ("spans", _) => state
                .tracks
                .iter()
                .skip(1)
                .map(|row| {
                    let mut items: Vec<(f64, f64)> = state
                        .midi
                        .iter()
                        .filter(|item| row["ref"] == format!("r:track:{}", item.track.trim_start_matches("r:track:")))
                        .map(|item| (item.position, item.position + item.length))
                        .collect();
                    items.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let played: Vec<Value> =
                        if items.is_empty() { vec![json!([0.0, 8.0])] } else { items.iter().map(|(from, to)| json!([from, to])).collect() };
                    let mut spans = json!({"ref": row["ref"], "name": row["name"], "spans": played});
                    if row["mute"] == true {
                        spans["mute"] = true.into();
                    }
                    spans
                })
                .collect(),
            ("item", Some(parent)) => state
                .tracks
                .iter()
                .filter(|row| row["ref"] == parent)
                .map(|row| item_row(row["ref"].as_str().unwrap()))
                .chain(state.midi.iter().filter(|item| item.track == parent).map(SimMidi::row))
                .collect(),
            ("note", Some(parent)) => match Self::find_midi(state, parent) {
                Some(at) => state.midi[at].note_rows(),
                None => return Err(("failed".into(), format!("{parent} isn't a MIDI take in the project"))),
            },
            ("fx", Some(parent)) => state
                .fx
                .get(parent)
                .map(|chain| chain.iter().enumerate().map(|(at, fx)| Self::fx_row(fx, at)).collect())
                .unwrap_or_default(),
            ("param", Some(parent)) => match Self::find_fx(state, parent) {
                Some((track, at)) => {
                    let fx = &state.fx[&track][at];
                    let points = params.get("grid").and_then(Value::as_u64);
                    fx.params
                        .iter()
                        .enumerate()
                        .map(|(index, (name, value, shows))| {
                            let mut row = json!({"ref": format!("r:param:{}:{index}", fx.guid), "index": index, "name": name, "value": value, "formatted": shows.text(*value), "min": 0.0, "max": 1.0});
                            if let Some(points) = points {
                                let grid: Vec<Value> =
                                    (0..points).map(|n| n as f64 / (points - 1) as f64).map(|at| json!([at, shows.text(at)])).collect();
                                row["grid"] = grid.into();
                            }
                            row
                        })
                        .collect()
                }
                None => return Err(("failed".into(), format!("{parent} isn't an FX in the project"))),
            },
            ("item", None) | ("fx", None) | ("param", None) | ("send", None) | ("note", None) | ("envelope", None) | ("take", None) => {
                return Err(bad(&format!("{kind}: pass the parent")))
            }
            _ => vec![],
        };
        let mut rows = Vec::new();
        let mut bytes = 0;
        let mut next = None;
        for (position, row) in all.iter().enumerate().skip(start) {
            let size = row.to_string().len() + 1;
            let full = rows.len() >= limit || (state.keeps_bound && !rows.is_empty() && bytes + size > PAGE_BYTES);
            if full {
                next = Some(position);
                break;
            }
            bytes += size;
            rows.push(row.clone());
        }
        let mut page = json!({"items": rows, "total": all.len(), "epoch": epoch});
        if let Some(position) = next {
            page["nextCursor"] = format!("c1.{epoch}.{position}").into();
        }
        Ok(page)
    }
}
