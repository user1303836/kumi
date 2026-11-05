//! A scripted Logic Pro project for the host's tests: what Kumi's own bridge beside Logic (`logic-kumi`) answers to
//! `read`, `snapshot`, `focus` and `focus_body` (the native M2 contract), replayed verbatim; and
//! what it answers to Kumi's changes (M3): `change_preview`, `change_apply` and `change_restore`, the transport and the
//! playhead, and a plan's steps, counted, since Logic has no undo block.
//!
//! The M2 answers come from the unchanged original 26-case vector (d821e669), independently replayed by the native
//! readers over their AX recordings. The M3 state and eight host regressions are ported from
//! 8d609d6a051e4a6f040431bf64c4b9712214094e. No recording is generated or blessed here.
//!
//! Kumi's changes are made on the project as changed over the recorded rows, the way the bridge makes them (the
//! contract's Changes section): a preview that writes nothing and fences what it saw, an apply that checks the fence
//! again, writes and answers the value read back, a restore by value fenced by what the apply wrote. Logic keeps no
//! undo step for a header's control (volume, pan, mute, solo, arm) or the tempo, so those answer `undo.entry: false`
//! with no title; a rename ("Renaming"), a track made ("Create Track") or deleted ("Delete Tracks") and a marker made
//! ("Create Marker"), moved ("Value Change") or deleted ("Delete") are steps of Logic's undo, named by the Edit menu's
//! title and by the `desc` the host sent. A track added goes below the selected one (or the one `index` names) and
//! moves the epoch, as the M2 replay moves it; a restore's ref from before is carried to where its track is now. A
//! deleted track or marker requires the producer's manual Logic undo: identical Undo titles cannot prove which
//! edit is Kumi's, and a deleted marker's original name cannot be recreated faithfully. The reads replay the vectors with the changed
//! values in them. A dialog (`busy_once`) refuses the next preview before anything is written; a write Logic doesn't
//! answer after (`uncertain_once`) is made and answered `uncertain`. The kinds the bridge can't make are refused saying
//! why, as it refuses them; of the kinds it makes, the simulator makes those whose state is a value here (the strip's
//! controls and its name, tracks added and deleted, markers and the tempo), and says so for the rest (notes, bypass).
use super::replay::{epoch_of, find, load_cases, normal, retag, Case, RECORDED_EPOCH};
use super::{Respond, SimulatorConfig};
use daw_bridge::auth::Secret;
use daw_bridge::wire::Project;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const VECTORS: &str = include_str!("../../tests/vectors/logic-reads.json");
const VECTORS_NAME: &str = "tests/vectors/logic-reads.json";
/// The timeline body's page, in tracks.
const MAX_LANES: usize = 500;
/// A read's page: the contract's.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
/// The reads, in the order the bridge's welcome lists them after ping and status.
const METHODS: [&str; 4] = ["focus", "read", "snapshot", "focus_body"];
/// What the project answers besides: Kumi's changes, the transport and the playhead, and a plan's steps.
const CHANGE_METHODS: [&str; 8] = [
    "change_preview",
    "change_apply",
    "change_restore",
    "transport_stop",
    "transport_play",
    "cursor_set",
    "undo_step_begin",
    "undo_step_end",
];
const MANUAL_UNDO: &str = "Logic Pro's Undo title cannot identify Kumi's specific step. Kumi won't undo a possibly unrelated producer edit; use Logic Pro's own undo manually.";
/// The kinds the bridge can't make yet, and why, as it refuses them.
const NOT_YET: [(&str, &str); 9] = [
    ("loop-range", "the cycle locators have no field Accessibility can set"),
    ("marker-rename", "a marker's name cell in the Marker List isn't a text field"),
    ("fx-add", "the insert slot's pop-up menu can't be driven from outside"),
    ("fx-delete", "the insert slot's pop-up menu can't be driven from outside"),
    ("fx-param", "a plug-in's controls aren't reached yet"),
    ("fx-move", "the insert slots can't be rearranged from outside"),
    ("send-add", "the send slot's pop-up menu can't be driven from outside"),
    ("send-volume", "the send slot's pop-up menu can't be driven from outside"),
    ("track-move", "the track headers can't be dragged from outside"),
];
/// The kinds the bridge makes whose state the simulator doesn't hold.
const NOT_SIMULATED: [&str; 4] = ["notes-add", "notes-change", "notes-delete", "fx-bypass"];
/// The recorded project's signature: four beats to the bar.
const BEATS_PER_BAR: f64 = 4.0;
/// Logic's resolution: 960 ticks to a beat, a division being a sixteenth of 240 ticks.
const TICKS_PER_BEAT: f64 = 960.0;
const DIVISIONS_PER_BEAT: f64 = 4.0;
/// The fader's floor and ceiling in dB, and how far a reading may be from what was asked and still count.
const MIN_DB: f64 = -96.0;
const MAX_DB: f64 = 6.0;
const CLOSE_DB: f64 = 0.051;
/// Logic's pan runs 64 steps each way from centre.
const PAN_CENTRE: i64 = 64;
/// Logic's tempo slider runs 5 to 990 BPM in whole steps.
const TEMPO_MIN: f64 = 5.0;
const TEMPO_MAX: f64 = 990.0;
/// How long a marker Logic makes is (its length to the next one isn't modelled).
const MARKER_LENGTH: f64 = 16.0;
/// What the Edit menu says with nothing to undo.
const CANT_UNDO: &str = "Can't Undo";
/// What the bridge answers when Logic doesn't answer after a write was sent.
const UNANSWERED: &str = "Logic Pro didn't answer after the change was sent: the helper didn't answer in time";

/// One of the project's tracks, as Kumi's changes left it: the recorded row (or one made for a track Kumi added) and
/// the values the changes reach.
#[derive(Clone)]
struct Track {
    /// Tells a track apart through the places it has over the epochs.
    id: u64,
    row: Value,
    name: String,
    kind: String,
    volume_db: f64,
    /// Logic's pan, -64 to 63 (the tool's -1 to 1, as steps).
    pan_steps: i64,
    mute: bool,
    solo: bool,
    armed: bool,
}

impl Track {
    /// A recorded row.
    fn recorded(id: u64, row: &Value) -> Track {
        Track {
            id,
            row: row.clone(),
            name: row["name"].as_str().unwrap_or_default().to_string(),
            kind: row["kind"].as_str().unwrap_or("audio").to_string(),
            volume_db: row["volumeDb"].as_f64().unwrap_or(0.0),
            pan_steps: row["pan"].as_i64().unwrap_or(0),
            mute: row["mute"] == true,
            solo: row["solo"] == true,
            armed: row["armed"] == true,
        }
    }

    /// A track Logic's Track menu makes: an audio, software instrument or external MIDI track, named.
    fn made(id: u64, name: &str, kind: &str) -> Track {
        let row = json!({
            "armed": false, "color": null, "index": 0, "inputs": if kind == "audio" { json!(["Input 1"]) } else { json!([]) },
            "kind": kind, "monitoring": false, "mute": false, "name": name, "output": "Stereo Output", "pan": 0,
            "ref": "", "selected": false, "solo": false, "volumeDb": 0.0,
        });
        Track { id, row, name: name.into(), kind: kind.into(), volume_db: 0.0, pan_steps: 0, mute: false, solo: false, armed: false }
    }

    /// The row as a read shows it at `place` (from 1) in the recorded epoch (the answer is retagged to the one now).
    fn row(&self, place: usize) -> Value {
        let mut row = self.row.clone();
        row["ref"] = format!("{RECORDED_EPOCH}:track:{place}").into();
        row["index"] = place.into();
        row["name"] = self.name.clone().into();
        row["kind"] = self.kind.clone().into();
        row["volumeDb"] = self.volume_db.into();
        row["pan"] = self.pan_steps.into();
        row["mute"] = self.mute.into();
        row["solo"] = self.solo.into();
        row["armed"] = self.armed.into();
        row
    }

    /// A prop's state as the bridge reads it from the header.
    fn state(&self, field: &str) -> Value {
        match field {
            "volumeDb" => volume_state(self.volume_db),
            "pan" => pan_state(self.pan_steps),
            "mute" => json!({"mute": self.mute}),
            "solo" => json!({"solo": self.solo}),
            "arm" => json!({"armed": self.armed}),
            _ => json!({"name": self.name}),
        }
    }

    /// A prop's state written.
    fn write(&mut self, field: &str, state: &Value) {
        match field {
            "volumeDb" => self.volume_db = state["volumeDb"].as_f64().unwrap_or(self.volume_db),
            "pan" => self.pan_steps = state["steps"].as_i64().unwrap_or(self.pan_steps),
            "mute" => self.mute = state["mute"] == true,
            "solo" => self.solo = state["solo"] == true,
            "arm" => self.armed = state["armed"] == true,
            _ => self.name = state["name"].as_str().unwrap_or(&self.name).to_string(),
        }
    }
}

fn volume_state(db: f64) -> Value {
    json!({"volumeDb": (db * 10.0).round() / 10.0})
}

fn pan_state(steps: i64) -> Value {
    json!({"pan": (steps as f64 / PAN_CENTRE as f64 * 1000.0).round() / 1000.0, "steps": steps})
}

/// dB with the true minus sign, as the bridge's titles show it: `−6`, `+3`, `0`, `−6.5`, `−∞`.
fn db_text(db: f64) -> String {
    if db <= MIN_DB + 0.5 {
        return "−∞".into();
    }
    let rounded = (db * 10.0).round() / 10.0;
    let text = format!("{:.1}", rounded.abs());
    let text = text.strip_suffix(".0").map(str::to_string).unwrap_or(text);
    if rounded == 0.0 {
        "0".into()
    } else if rounded < 0.0 {
        format!("−{text}")
    } else {
        format!("+{text}")
    }
}

/// A pan as a producer says it: `centre`, `50% left`, `98% right`.
fn pan_text(steps: i64) -> String {
    let percent = ((steps.abs() as f64 / PAN_CENTRE as f64) * 100.0).round() as i64;
    match steps {
        0 => "centre".into(),
        n if n < 0 => format!("{percent}% left"),
        _ => format!("{percent}% right"),
    }
}

#[derive(Clone, PartialEq)]
struct Marker {
    name: String,
    position_beats: f64,
    length: f64,
}

impl Marker {
    /// What a marker is, as the Marker List shows it (the bridge's state).
    fn state(&self) -> Value {
        json!({"position": position_of(self.position_beats), "name": self.name, "length": self.length, "text": ""})
    }
    /// The row as the Marker List reads it, at `place` (from 1).
    fn row(&self, place: usize) -> Value {
        json!({"length": self.length, "name": self.name, "position": position_of(self.position_beats), "ref": format!("{RECORDED_EPOCH}:marker:{place}"), "text": ""})
    }
    /// The marker as a title says it: `marker “Marker 1”`.
    fn label(&self) -> String {
        if self.name.is_empty() {
            "marker".into()
        } else {
            format!("marker {}", quoted(&self.name))
        }
    }
}

/// A track attribute Kumi changes through its header or the Inspector: the input's field, its noun for the words, and
/// the name Logic's history gives its step, when it takes one (a header's control makes none).
struct Prop {
    kind: &'static str,
    field: &'static str,
    noun: &'static str,
    step: Option<&'static str>,
}

static TRACK_PROPS: [Prop; 6] = [
    Prop { kind: "track-volume", field: "volumeDb", noun: "volume", step: None },
    Prop { kind: "track-pan", field: "pan", noun: "pan", step: None },
    Prop { kind: "track-mute", field: "mute", noun: "mute", step: None },
    Prop { kind: "track-solo", field: "solo", noun: "solo", step: None },
    Prop { kind: "track-arm", field: "arm", noun: "record arm", step: None },
    Prop { kind: "track-rename", field: "name", noun: "name", step: Some("Renaming") },
];

impl Prop {
    /// The state the input asks for.
    fn wanted(&self, input: &Value) -> Result<Value, Fault> {
        Ok(match self.field {
            "volumeDb" => {
                let db = input
                    .get("volumeDb")
                    .and_then(Value::as_f64)
                    .filter(|db| db.is_finite())
                    .ok_or_else(|| bad("volumeDb must be a number"))?;
                if db > MAX_DB {
                    return Err(bad(format!("volumeDb must be at most {MAX_DB} (Logic's fader goes no higher)")));
                }
                volume_state(db.max(MIN_DB))
            }
            "pan" => {
                let pan = input
                    .get("pan")
                    .and_then(Value::as_f64)
                    .filter(|pan| (-1.0..=1.0).contains(pan))
                    .ok_or_else(|| bad("pan must be a number from -1 (left) to 1 (right)"))?;
                pan_state(((pan * PAN_CENTRE as f64).round() as i64).clamp(-PAN_CENTRE, PAN_CENTRE - 1))
            }
            "mute" | "solo" => {
                let on =
                    input.get(self.field).and_then(Value::as_bool).ok_or_else(|| bad(format!("{} must be true or false", self.field)))?;
                json!({self.field: on})
            }
            "arm" => {
                let armed = input
                    .get("armed")
                    .or_else(|| input.get("arm"))
                    .and_then(Value::as_bool)
                    .ok_or_else(|| bad("armed must be true or false"))?;
                json!({"armed": armed})
            }
            _ => {
                let name = input.get("name").and_then(Value::as_str).ok_or_else(|| bad("name must be text"))?;
                if name.trim().is_empty() {
                    return Err(bad("name must not be empty: Logic Pro names a track itself when its name is cleared"));
                }
                if name.chars().count() > 256 || name.chars().any(char::is_control) {
                    return Err(bad("name must be one line of at most 256 characters"));
                }
                json!({"name": name})
            }
        })
    }

    /// Two states the same: a fader within a step, a pan by its steps, the rest exactly.
    fn same(&self, a: &Value, b: &Value) -> bool {
        match self.field {
            "volumeDb" => (a["volumeDb"].as_f64().unwrap_or(f64::NAN) - b["volumeDb"].as_f64().unwrap_or(f64::NAN)).abs() <= CLOSE_DB,
            "pan" => a["steps"] == b["steps"],
            "arm" => a["armed"] == b["armed"],
            field => a[field] == b[field],
        }
    }

    /// The title of the change, with the track's name and the state it goes to.
    fn title(&self, name: &str, from: &Value, to: &Value) -> String {
        match self.field {
            "volumeDb" => format!("{name} volume {} dB", db_text(to["volumeDb"].as_f64().unwrap_or(0.0))),
            "pan" => format!("{name} pan {}", pan_text(to["steps"].as_i64().unwrap_or(0))),
            "mute" => format!("{name} {}", if to["mute"] == true { "muted" } else { "unmuted" }),
            "solo" => format!("{name} {}", if to["solo"] == true { "soloed" } else { "unsoloed" }),
            "arm" => format!("{name} {}", if to["armed"] == true { "armed" } else { "disarmed" }),
            _ => format!(
                "Renamed track {} → {}",
                quoted(from["name"].as_str().unwrap_or_default()),
                quoted(to["name"].as_str().unwrap_or_default())
            ),
        }
    }
}

struct State {
    /// The epoch the project is in now (the vectors were recorded in [`RECORDED_EPOCH`]).
    epoch: u32,
    /// Which project is open: 0 the recorded one, else another copy of it, with an id and a name of its own.
    project: u32,
    cases: Vec<Case>,
    /// What the simulator was asked, in order: method and params.
    asked: Vec<(String, Value)>,
    /// The project's tracks as Kumi's changes left them, in the Tracks area's order.
    tracks: Vec<Track>,
    /// Which track stood at each place in every epoch before this one: a restore names a track by the place it had
    /// when the change was previewed, and the bridge carries it to where the track is now.
    places: HashMap<u32, Vec<u64>>,
    next_track: u64,
    /// The markers, in the Marker List's order (by position).
    markers: Vec<Marker>,
    /// The number Logic gives the next marker it makes ("Marker 2").
    next_marker: usize,
    tempo: f64,
    /// The playhead, in beats from the project's start.
    playhead: f64,
    playing: bool,
    /// Logic's undo history, newest last: the Edit menu's title for each step, and the `desc` the host sent with the
    /// change that made it (none for a restore's step, or a write Logic didn't answer after: the bridge names neither).
    undo: Vec<(String, Option<String>)>,
    /// A plan's step, counted (Logic has no undo block): its id and label, and the changes applied in it so far.
    open_step: Option<(String, String, usize)>,
    steps: u32,
    /// A dialog shows for the next change preview only (it closes right after).
    busy_once: bool,
    /// The next write (an apply or a restore) is made, and Logic doesn't answer after it.
    uncertain_once: bool,
}

/// A project that answers like the bridge. Clones share the project.
#[derive(Clone)]
pub struct LogicScript {
    state: Arc<Mutex<State>>,
}

type Fault = (String, String);
type Answer = Result<Value, Fault>;

fn bad(message: impl Into<String>) -> Fault {
    ("bad-params".into(), message.into())
}

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

fn failed(message: impl Into<String>) -> Fault {
    ("failed".into(), message.into())
}

fn gone(message: impl Into<String>) -> Fault {
    ("gone".into(), message.into())
}

fn uncertain(message: impl Into<String>) -> Fault {
    ("uncertain".into(), message.into())
}

/// A name inside a title's sentence, in curly quotes.
fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// A position as Logic shows it (bar, beat, division and tick, each from 1), from beats.
fn position_of(beats: f64) -> Value {
    let bar = (beats / BEATS_PER_BAR).floor();
    let in_bar = beats - bar * BEATS_PER_BAR;
    let beat = in_bar.floor();
    let in_beat = in_bar - beat;
    let division = (in_beat * DIVISIONS_PER_BEAT).floor();
    let tick = ((in_beat * DIVISIONS_PER_BEAT - division) * TICKS_PER_BEAT / DIVISIONS_PER_BEAT).round();
    json!({"bar": bar as i64 + 1, "beat": beat as i64 + 1, "division": division as i64 + 1, "tick": tick as i64 + 1})
}

/// Beats from the project's start, from a position as Logic shows it.
fn beats_of(position: &Value) -> f64 {
    let number = |key: &str| position.get(key).and_then(Value::as_f64).unwrap_or(1.0);
    (number("bar") - 1.0) * BEATS_PER_BAR
        + (number("beat") - 1.0)
        + (number("division") - 1.0) / DIVISIONS_PER_BEAT
        + (number("tick") - 1.0) / TICKS_PER_BEAT
}

/// Where a position is, as the bridge's titles say it: `bar 17`, `bar 2 beat 3`, or `bar 2 beat 3.3.1` off the beat.
fn bar_text(beats: f64) -> String {
    let position = position_of(beats);
    let at = |key: &str| position[key].as_i64().unwrap_or(1);
    let mut text = format!("bar {}", at("bar"));
    if at("beat") > 1 || at("division") > 1 || at("tick") > 1 {
        text.push_str(&format!(" beat {}", at("beat")));
    }
    if at("division") > 1 || at("tick") > 1 {
        text.push_str(&format!(".{}.{}", at("division"), at("tick")));
    }
    text
}

/// A marker's or the playhead's beat: Logic's displays place them on the bar and beat.
fn on_the_beat(beats: f64) -> f64 {
    beats.floor()
}

/// The project the vectors were recorded over, as the bridge's `status` named it: what the welcome and the discovery
/// file carry.
pub fn project() -> Project {
    let cases = load_cases(VECTORS, VECTORS_NAME);
    find(&cases, "status", &json!({}))
        .and_then(|case| case.response.as_ref().ok())
        .and_then(|status| serde_json::from_value(status["project"].clone()).ok())
        .unwrap_or_else(|| Project { name: Some("Scratch".into()), ..Project::default() })
}

/// The project row of `value`, if it holds one (an `id` of 32 hex digits beside a path or an identity), made the
/// `n`th copy of the recorded project: another id, identity, name and path.
fn another_project(value: &Value, n: u32) -> Value {
    if n == 0 {
        return value.clone();
    }
    match value {
        Value::Object(map) => {
            let is_project = map.get("id").and_then(Value::as_str).is_some_and(|id| id.len() == 32)
                && (map.contains_key("path") || map.contains_key("identity"));
            Value::Object(
                map.iter()
                    .map(|(key, field)| {
                        let field = match (is_project, key.as_str(), field.as_str()) {
                            (true, "id", Some(id)) => Value::String(format!("{}{n:02x}", &id[..30])),
                            (true, "identity" | "name", Some(text)) if !text.is_empty() => Value::String(format!("{text} {n}")),
                            (true, "path", Some(path)) if !path.is_empty() => {
                                let (stem, extension) = path.rsplit_once('.').unwrap_or((path, ""));
                                Value::String(if extension.is_empty() { format!("{stem} {n}") } else { format!("{stem} {n}.{extension}") })
                            }
                            _ => another_project(field, n),
                        };
                        (key.clone(), field)
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| another_project(item, n)).collect()),
        other => other.clone(),
    }
}

/// A ref or cursor in `params` from another epoch than `epoch`: the bridge refuses it, whatever it was for.
fn stale_in(params: &Value, epoch: u32) -> Option<(String, String)> {
    for key in ["ref", "parent", "around", "cursor"] {
        let Some(from) = params.get(key).and_then(Value::as_str).and_then(epoch_of).filter(|from| *from != epoch) else { continue };
        let (what, then) = if key == "cursor" { ("cursor", "read again from the start") } else { ("ref", "read again") };
        return Some(("stale".into(), format!("that {what} is from an earlier epoch ({from}; now {epoch}): {then}")));
    }
    None
}

/// Where a page starts: the position a cursor (`<epoch>.<n>`) names, or 0.
fn start_of(params: &Value) -> Result<usize, (String, String)> {
    match params.get("cursor").and_then(Value::as_str) {
        None => Ok(0),
        Some(cursor) => cursor
            .split_once('.')
            .and_then(|(_, n)| n.parse::<usize>().ok())
            .ok_or_else(|| ("bad-params".to_string(), "cursor: pass the nextCursor an earlier read gave, unchanged".to_string())),
    }
}

/// A page of the Tracks area, as the bridge pages it: `limit` lanes from a cursor, or centred on the `around` track
/// (half a page above it), each lane's regions kept to the `window` asked for (the recorded one otherwise).
fn timeline_page(whole: &Value, params: &Value) -> Answer {
    let lanes = whole["lanes"].as_array().cloned().unwrap_or_default();
    let limit = params.get("limit").and_then(Value::as_u64).map_or(MAX_LANES, |n| (n as usize).clamp(1, MAX_LANES));
    let start = match params.get("around").and_then(Value::as_str) {
        Some(around) => {
            let place = lanes
                .iter()
                .position(|lane| lane["trackRef"] == around)
                .ok_or_else(|| ("failed".to_string(), format!("{around} isn't in the project (deleted, or another project)")))?;
            place.saturating_sub(limit / 2).min(lanes.len().saturating_sub(limit))
        }
        None => start_of(params)?,
    };
    let window = match params.get("window") {
        Some(window) => json!({"from": window["from"].as_f64().unwrap_or(0.0), "to": window["to"].as_f64().unwrap_or(f64::MAX)}),
        None => whole["window"].clone(),
    };
    let (from, to) = (window["from"].as_f64().unwrap_or(0.0), window["to"].as_f64().unwrap_or(f64::MAX));
    let page: Vec<Value> = lanes
        .iter()
        .skip(start)
        .take(limit)
        .map(|lane| {
            let mut lane = lane.clone();
            if let Some(regions) = lane["regions"].as_array_mut() {
                regions.retain(|region| {
                    region["startBeats"].as_f64().unwrap_or(0.0) < to && region["endBeats"].as_f64().unwrap_or(0.0) > from
                });
            }
            lane
        })
        .collect();
    let mut body = whole.clone();
    body["window"] = window;
    body["lanes"] = Value::Array(page);
    body["from"] = start.into();
    if let Some(map) = body.as_object_mut() {
        map.remove("nextCursor");
    }
    if start + limit < lanes.len() {
        body["nextCursor"] = format!("{RECORDED_EPOCH}.{}", start + limit).into();
    }
    Ok(body)
}

/// The page of rows `rows` (every row, in order) a read asks for, by limit and cursor; `total` with it.
fn page_of(rows: Vec<Value>, params: &Value) -> Answer {
    let limit = params.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT));
    let start = start_of(params)?;
    let items: Vec<Value> = rows.iter().skip(start).take(limit).cloned().collect();
    // (the keys in the bridge's order, as the recorded pages have them)
    let mut page = json!({"epoch": RECORDED_EPOCH.to_string(), "items": items});
    if start + limit < rows.len() {
        page["nextCursor"] = format!("{RECORDED_EPOCH}.{}", start + limit).into();
    }
    page["total"] = rows.len().into();
    Ok(page)
}

impl State {
    /// The project of the vectors: its tracks, markers, tempo and transport as recorded.
    fn recorded() -> State {
        let cases = load_cases(VECTORS, VECTORS_NAME);
        let rows = |kind: &str| -> Vec<Value> {
            find(&cases, "read", &json!({"kind": kind}))
                .and_then(|case| case.response.as_ref().ok())
                .and_then(|page| page["items"].as_array().cloned())
                .unwrap_or_default()
        };
        let tracks: Vec<Track> = rows("track").iter().enumerate().map(|(n, row)| Track::recorded(n as u64 + 1, row)).collect();
        let markers: Vec<Marker> = rows("marker")
            .iter()
            .map(|row| Marker {
                name: row["name"].as_str().unwrap_or_default().to_string(),
                position_beats: beats_of(&row["position"]),
                length: row["length"].as_f64().unwrap_or(0.0),
            })
            .collect();
        let tempo = rows("tempo").first().and_then(|row| row["bpm"].as_f64()).unwrap_or(120.0);
        let focus = find(&cases, "focus", &json!({})).and_then(|case| case.response.as_ref().ok()).cloned().unwrap_or(Value::Null);
        let (next_track, next_marker) = (tracks.len() as u64 + 1, markers.len() + 1);
        State {
            epoch: RECORDED_EPOCH,
            project: 0,
            cases,
            asked: vec![],
            tracks,
            places: HashMap::new(),
            next_track,
            markers,
            next_marker,
            tempo,
            playhead: focus["transport"]["beat"].as_f64().unwrap_or(0.0),
            playing: focus["transport"]["playing"] == true,
            undo: vec![],
            open_step: None,
            steps: 0,
            busy_once: false,
            uncertain_once: false,
        }
    }

    fn epoch_text(&self) -> String {
        self.epoch.to_string()
    }

    /// The epoch moves (a track added, deleted or renamed): the places of this one are kept, for a restore's ref.
    fn bump_epoch(&mut self) {
        self.places.insert(self.epoch, self.tracks.iter().map(|track| track.id).collect());
        self.epoch += 1;
    }

    /// The regions a track holds, as the recorded Tracks area has them (a track Kumi added holds none).
    fn regions_of(&self, track: &Track) -> usize {
        find(&self.cases, "focus_body", &json!({"kind": "timeline"}))
            .and_then(|case| case.response.as_ref().ok())
            .and_then(|whole| whole["lanes"].get(track.id as usize - 1))
            .and_then(|lane| lane["regions"].as_array())
            .map_or(0, Vec::len)
    }

    /// The track a ref names, by its place now: a ref of the epoch now, or (a restore's) one of an earlier epoch,
    /// carried to where that track is now.
    fn track_at(&self, reference: Option<&str>, carry: bool) -> Result<usize, Fault> {
        let text = reference.ok_or_else(|| bad("ref must be a track ref"))?;
        let mut parts = text.splitn(3, ':');
        let (epoch, kind, place) = (parts.next(), parts.next(), parts.next().and_then(|n| n.parse::<usize>().ok()));
        let (Some(epoch), Some("track"), Some(place)) = (epoch.and_then(|e| e.parse::<u32>().ok()), kind, place) else {
            return Err(bad(format!("ref must be a track ref ({}:track:1), not {text}", self.epoch)));
        };
        if epoch == self.epoch {
            return (place >= 1 && place <= self.tracks.len())
                .then_some(place - 1)
                .ok_or_else(|| failed(format!("{text} isn't in the project (deleted, or another project)")));
        }
        if !carry || epoch > self.epoch {
            return Err(("stale".into(), format!("that ref is from an earlier epoch ({epoch}; now {}): read again", self.epoch)));
        }
        let id = self
            .places
            .get(&epoch)
            .and_then(|places| places.get(place.wrapping_sub(1)))
            .ok_or_else(|| gone(format!("the track {text} named isn't in the project any more (or that epoch is too far back)")))?;
        self.tracks
            .iter()
            .position(|track| track.id == *id)
            .ok_or_else(|| gone(format!("the track {text} named isn't in the project any more (or that epoch is too far back)")))
    }

    /// The place a marker ref names (from 1): markers are by their place in the list, whatever the epoch.
    fn marker_place(&self, reference: Option<&str>) -> Result<usize, Fault> {
        let text = reference.ok_or_else(|| bad("ref must be a marker ref"))?;
        let mut parts = text.splitn(3, ':');
        let (_, kind, place) = (parts.next(), parts.next(), parts.next().and_then(|n| n.parse::<usize>().ok()));
        match (kind, place) {
            (Some("marker"), Some(place)) if place >= 1 => Ok(place),
            _ => Err(bad(format!("ref must be a marker ref ({}:marker:1), not {text}", self.epoch))),
        }
    }

    /// The marker at a position, if one is.
    fn marker_at(&self, beats: f64) -> Option<usize> {
        self.markers.iter().position(|m| m.position_beats == beats)
    }

    fn marker_ref(&self, place: usize) -> String {
        format!("{}:marker:{place}", self.epoch)
    }

    fn sort_markers(&mut self) {
        self.markers.sort_by(|a, b| a.position_beats.total_cmp(&b.position_beats));
    }

    /// What the Edit menu's Undo title says now.
    fn undo_title(&self) -> String {
        self.undo.last().map_or(CANT_UNDO.to_string(), |(title, _)| title.clone())
    }

    /// A write made: the apply's `undo`. Logic's history takes a step for `step`, named by the Edit menu's title and
    /// by the host's `desc` (none for a header's control or the tempo); a write Logic doesn't answer after is
    /// `uncertain`, its step (if any) unnamed to Kumi.
    fn applied(&mut self, step: Option<&str>, desc: &str) -> Result<Value, Fault> {
        let title = step.map(|step| format!("Undo {step}"));
        if std::mem::take(&mut self.uncertain_once) {
            if let Some(title) = title {
                self.undo.push((title, None));
            }
            return Err(uncertain(UNANSWERED));
        }
        match title {
            Some(title) => {
                self.undo.push((title.clone(), Some(desc.to_string())));
                Ok(json!({"desc": desc, "entry": true, "title": title}))
            }
            None => Ok(json!({"desc": desc, "entry": false})),
        }
    }

    /// A restore made by value: the restore's `undo`. Logic's history takes a step for `step` (a rename, a track
    /// deleted, a marker made, moved or deleted again), which the bridge doesn't name.
    fn restored(&mut self, step: Option<&str>, desc: &str) -> Result<Value, Fault> {
        if std::mem::take(&mut self.uncertain_once) {
            if let Some(step) = step {
                self.undo.push((format!("Undo {step}"), None));
            }
            return Err(uncertain(UNANSWERED));
        }
        if let Some(step) = step {
            self.undo.push((format!("Undo {step}"), None));
        }
        Ok(json!({"desc": desc, "entry": false}))
    }

    // ----- the changes -----

    fn change(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_preview" && std::mem::take(&mut self.busy_once) {
            return Err(("busy".into(), "a dialog is up in Logic Pro; nothing changed".into()));
        }
        // Refs from before the project changed are refused for a preview or an apply, as the bridge refuses them
        // whatever they were for; a restore names the change's own place, carried to where its track is now.
        if method != "change_restore" {
            if let Some(sent) = params.get("epoch").and_then(Value::as_str).filter(|sent| *sent != self.epoch_text()) {
                return Err((
                    "stale".into(),
                    format!("those refs are from an earlier epoch ({sent}; now {}): the tracks or regions changed, read again", self.epoch),
                ));
            }
        }
        let kind = params["kind"].as_str().unwrap_or_default().to_string();
        if let Some((_, why)) = NOT_YET.iter().find(|(not_yet, _)| *not_yet == kind) {
            return Err(bad(format!("Logic Pro's bridge can't make a {kind} change yet: {why}")));
        }
        match kind.as_str() {
            "track-add" => self.track_add(method, params),
            "track-delete" => self.track_delete(method, params),
            "marker-add" => self.marker_add(method, params),
            "marker-move" => self.marker_move(method, params),
            "marker-delete" => self.marker_delete(method, params),
            "tempo-set" => self.tempo_set(method, params),
            _ => match TRACK_PROPS.iter().find(|prop| prop.kind == kind) {
                Some(prop) => self.track_prop(prop, method, params),
                None if NOT_SIMULATED.contains(&kind.as_str()) => Err(bad(format!(
                    "the simulated Logic Pro makes only {} (not {kind:?}); the bridge makes {} too",
                    "track-volume, track-pan, track-mute, track-solo, track-arm, track-rename, track-add, track-delete, marker-add, marker-move, marker-delete and tempo-set",
                    NOT_SIMULATED.join(", ")
                ))),
                None => Err(bad(format!("Logic Pro has no change kind {kind:?}"))),
            },
        }
    }

    fn desc(params: &Value) -> Result<String, Fault> {
        params["desc"].as_str().map(str::to_owned).ok_or_else(|| bad("desc must be a string"))
    }

    fn track_prop(&mut self, prop: &Prop, method: &str, params: &Value) -> Answer {
        let (reference, input) = match method {
            "change_restore" => (params["target"]["ref"].as_str(), &params["prior"]),
            _ => (params["input"]["ref"].as_str(), &params["input"]),
        };
        let at = self.track_at(reference, method == "change_restore")?;
        let (now, name) = (self.tracks[at].state(prop.field), self.tracks[at].name.clone());
        let epoch = self.epoch_text();
        match method {
            "change_preview" => {
                let wanted = prop.wanted(input)?;
                let mut display = json!({"title": prop.title(&name, &now, &wanted), "before": now, "after": wanted});
                if prop.same(&now, &wanted) {
                    display["unchanged"] = true.into();
                }
                Ok(json!({"epoch": epoch, "target": {"ref": reference, "kind": "track"}, "prior": now, "fence": now, "display": display}))
            }
            "change_apply" => {
                if !prop.same(&now, &params["fence"]) {
                    return Err(changed(format!("{name}'s {} changed since the preview", prop.noun)));
                }
                let wanted = prop.wanted(input)?;
                let desc = Self::desc(params)?;
                self.tracks[at].write(prop.field, &wanted);
                let after = self.tracks[at].state(prop.field);
                let undo = self.applied(prop.step, &desc)?;
                // (a renamed header moves the epoch, as the observers count it)
                if prop.step.is_some() && !prop.same(&now, &after) {
                    self.bump_epoch();
                }
                Ok(
                    json!({"epoch": self.epoch_text(), "applied": after, "display": {"title": prop.title(&name, &now, &after), "before": now, "after": after}, "undo": undo}),
                )
            }
            _ => {
                let prior = &params["prior"];
                let desc = Self::desc(params)?;
                if !prop.same(&now, prior) {
                    if !prop.same(&now, &params["applied"]) && params["discard"] != true {
                        return Err(changed(format!("{name}'s {} changed in Logic Pro since Kumi set it", prop.noun)));
                    }
                    self.tracks[at].write(prop.field, prior);
                }
                let after = self.tracks[at].state(prop.field);
                let undo = self.restored(prop.step.filter(|_| !prop.same(&now, prior)), &desc)?;
                if prop.step.is_some() && !prop.same(&now, &after) {
                    self.bump_epoch();
                }
                let title = format!("{} (put back)", prop.title(&name, &now, &after));
                Ok(
                    json!({"epoch": self.epoch_text(), "restored": after, "display": {"title": title, "before": now, "after": after}, "undo": undo}),
                )
            }
        }
    }

    /// `track-add {kind?, index?, name?}`: a track below the one `index` names (the last by default), as Logic's Track
    /// menu makes one, which moves the epoch; its ref in `created`. Its restore deletes it by value, unless it's gone
    /// or holds regions since.
    fn track_add(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let created = params["created"]["ref"].as_str();
            let at = self.track_at(created, true)?;
            let name = params["applied"]["name"].as_str().unwrap_or_default().to_string();
            if self.tracks[at].name != name {
                return Err(gone(format!("the track Kumi added ({}) isn't at place {} any more", quoted(&name), at + 1)));
            }
            if params["discard"] != true {
                let held = self.regions_of(&self.tracks[at]);
                if held > 0 {
                    return Err(changed(format!("the track Kumi added ({}) holds {held} region(s) now", quoted(&name))));
                }
            }
            let desc = Self::desc(params)?;
            self.tracks.remove(at);
            self.bump_epoch();
            let undo = self.restored(Some("Delete Tracks"), &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"deleted": created},
                "display": {"title": format!("{} taken away again", quoted(&name))},
                "undo": undo,
            }));
        }
        let input = &params["input"];
        let count = self.tracks.len();
        let kind = match input.get("kind").filter(|kind| !kind.is_null()) {
            None => "audio".to_string(),
            Some(Value::String(kind)) if ["audio", "instrument", "external"].contains(&kind.as_str()) => kind.clone(),
            Some(_) => return Err(bad("kind must be audio, instrument or external")),
        };
        let after = match input.get("index").filter(|index| !index.is_null()) {
            None => count,
            Some(index) => {
                let index = index
                    .as_u64()
                    .map(|i| i as usize)
                    .filter(|i| *i <= count)
                    .ok_or_else(|| bad(format!("index must be a whole number from 1 to {count}")))?;
                if index == 0 {
                    return Err(bad(
                        "Logic Pro adds a track below the selected one, so index 0 (above every track) isn't reachable: use 1 or more",
                    ));
                }
                index
            }
        };
        if after == 0 {
            return Err(failed("Logic Pro has no track to add one after: the project has none"));
        }
        let name = match input.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) if !name.trim().is_empty() && name.chars().count() <= 256 && !name.chars().any(char::is_control) => {
                Some(name.clone())
            }
            Some(_) => return Err(bad("name must be one line of at most 256 characters")),
        };
        let names: Vec<String> = self.tracks.iter().map(|track| track.name.clone()).collect();
        let fence = json!({"tracks": count, "names": names});
        match method {
            "change_preview" => {
                let title = match &name {
                    Some(name) => format!("Added {kind} track {}", quoted(name)),
                    None => format!(
                        "Added {} track",
                        match kind.as_str() {
                            "external" => "an external MIDI",
                            "audio" => "an audio",
                            _ => "an instrument",
                        }
                    ),
                };
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "target": {"ref": null, "kind": "track", "index": after},
                    "prior": {"tracks": count},
                    "fence": fence,
                    "display": {"title": title, "index": after},
                }))
            }
            _ => {
                if params["fence"] != fence {
                    return Err(changed("the tracks changed since the preview"));
                }
                let desc = Self::desc(params)?;
                let named = name.unwrap_or_else(|| {
                    format!(
                        "{} {}",
                        match kind.as_str() {
                            "audio" => "Audio",
                            "instrument" => "Inst",
                            _ => "MIDI",
                        },
                        count + 1
                    )
                });
                let made = Track::made(self.next_track, &named, &kind);
                self.next_track += 1;
                self.tracks.insert(after, made);
                self.bump_epoch();
                let place = after + 1;
                let undo = self.applied(Some("Create Track"), &desc)?;
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": {"index": place, "name": named, "kind": kind},
                    "created": {"ref": format!("{}:track:{place}", self.epoch), "kind": "track"},
                    "display": {"title": format!("Added {kind} track {}", quoted(&named))},
                    "undo": undo,
                }))
            }
        }
    }

    /// `track-delete {ref}`: the track goes (Delete Track, its sheet answered), which moves the epoch; what it was is
    /// kept in `material`. Automatic restore refuses: the title does not prove ownership of the undo step.
    fn track_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return Err(failed(MANUAL_UNDO));
        }
        let at =
            self.track_at(params["input"]["ref"].as_str(), false)
                .map_err(|fault| if fault.0 == "failed" { changed(fault.1) } else { fault })?;
        let track = self.tracks[at].clone();
        let regions = self.regions_of(&track);
        let fence = json!({"name": track.name, "index": at + 1, "regions": regions});
        let title = format!("Deleted track {}", quoted(&track.name));
        match method {
            "change_preview" => {
                let mut display = json!({"title": title});
                if regions > 0 {
                    display["note"] =
                        format!("It holds {regions} region(s); only Logic Pro's own undo brings a deleted track back.").into();
                }
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "target": {"ref": params["input"]["ref"], "kind": "track"},
                    "prior": fence,
                    "fence": fence,
                    "display": display,
                }))
            }
            _ => {
                if params["fence"] != fence {
                    return Err(changed(format!("{} changed since the preview", track.name)));
                }
                let desc = Self::desc(params)?;
                let material = json!({
                    "name": track.name, "kind": track.kind, "index": at + 1, "regions": regions,
                    "volumeDb": track.volume_db, "pan": pan_state(track.pan_steps)["pan"],
                    "mute": track.mute, "solo": track.solo, "arm": track.armed, "step": desc,
                });
                self.tracks.remove(at);
                self.bump_epoch();
                let undo = self.applied(Some("Delete Tracks"), &desc)?;
                Ok(
                    json!({"epoch": self.epoch_text(), "applied": {"deleted": true}, "material": material, "display": {"title": title}, "undo": undo}),
                )
            }
        }
    }

    /// A marker's position from an input: `positionBeats`, beats from the project's start (seconds are refused).
    fn position_input(input: &Value) -> Result<f64, Fault> {
        if input.get("position").is_some_and(|v| !v.is_null()) && input.get("positionBeats").is_none_or(Value::is_null) {
            return Err(bad("Logic Pro's bridge places markers by positionBeats (beats from the project's start), not seconds"));
        }
        input
            .get("positionBeats")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))
    }

    /// `marker-add {positionBeats}`: a marker at the playhead moved there (on the beat), from the Marker List's Create
    /// new Marker; Logic names it. Its restore deletes the row at its place, if the marker there is still at that
    /// position.
    fn marker_add(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let created = params["created"]["ref"].as_str();
            let place = self.marker_place(created)?;
            let desc = Self::desc(params)?;
            let Some(marker) = self.markers.get(place - 1).cloned() else {
                return Err(gone("the marker Kumi added isn't in the Marker List any more"));
            };
            if params["discard"] != true && marker.state()["position"] != params["applied"]["position"] {
                return Err(changed(format!("{} has been moved in Logic Pro since Kumi added it", marker.label())));
            }
            self.markers.remove(place - 1);
            let undo = self.restored(Some("Delete"), &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"deleted": created},
                "display": {"title": format!("{} taken away again", marker.label())},
                "undo": undo,
            }));
        }
        let input = &params["input"];
        if input.get("name").is_some_and(|v| !v.is_null()) {
            return Err(bad(
                "Logic Pro names markers itself (Marker 1, Marker 2, in order): a marker can't be named from outside, so leave name out",
            ));
        }
        let asked = Self::position_input(input)?;
        let beats = on_the_beat(asked);
        let count = self.markers.len();
        match method {
            "change_preview" => {
                let mut display = json!({"title": format!("Marker at {}", bar_text(beats))});
                if asked != beats {
                    display["note"] = "Logic Pro's playhead display places a marker on a beat: the division and tick are dropped.".into();
                }
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "target": {"ref": null, "kind": "marker"},
                    "prior": {"markers": count},
                    "fence": {"markers": count},
                    "display": display,
                }))
            }
            _ => {
                if params["fence"] != json!({"markers": count}) {
                    return Err(changed("the markers changed since the preview"));
                }
                let desc = Self::desc(params)?;
                let marker = Marker { name: format!("Marker {}", self.next_marker), position_beats: beats, length: MARKER_LENGTH };
                self.next_marker += 1;
                self.markers.push(marker.clone());
                self.sort_markers();
                let place = self.marker_at(beats).map_or(0, |at| at + 1);
                let undo = self.applied(Some("Create Marker"), &desc)?;
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": marker.state(),
                    "created": {"ref": self.marker_ref(place), "kind": "marker"},
                    "display": {"title": format!("Marker {} at {}", quoted(&marker.name), bar_text(beats))},
                    "undo": undo,
                }))
            }
        }
    }

    /// `marker-move {ref, positionBeats}`: the row's position scrubber, a segment at a time; the list keeps its rows in
    /// order of position. Its restore moves the marker back from where Kumi left it.
    fn marker_move(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let (prior, applied) = (beats_of(&params["prior"]["position"]), beats_of(&params["applied"]["position"]));
            let desc = Self::desc(params)?;
            if self.marker_at(prior).is_some() && self.marker_at(applied).is_none() {
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"position": position_of(prior)},
                    "display": {"title": "The marker is where it was"},
                    "undo": {"desc": desc, "entry": false},
                }));
            }
            let Some(at) = self.marker_at(applied) else {
                return Err(if params["discard"] == true {
                    gone("the marker Kumi moved isn't where it left it, nor where it was")
                } else {
                    changed("the marker has been moved in Logic Pro since Kumi moved it")
                });
            };
            let label = self.markers[at].label();
            self.markers[at].position_beats = prior;
            self.sort_markers();
            let undo = self.restored(Some("Value Change"), &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"position": position_of(prior)},
                "display": {"title": format!("{label} back at {}", bar_text(prior))},
                "undo": undo,
            }));
        }
        let input = &params["input"];
        let place = self.marker_place(input["ref"].as_str())?;
        let to = on_the_beat(Self::position_input(input)?);
        let Some(marker) = self.markers.get(place - 1).cloned() else {
            return Err(match method {
                "change_preview" => {
                    failed(format!("{} isn't in the project (deleted, or another project)", input["ref"].as_str().unwrap_or_default()))
                }
                _ => changed("the marker went since the preview"),
            });
        };
        let now = json!({"position": position_of(marker.position_beats)});
        let title = format!("Moved {} to {}", marker.label(), bar_text(to));
        match method {
            "change_preview" => {
                let mut display = json!({"title": title, "before": now, "after": {"position": position_of(to)}});
                if marker.position_beats == to {
                    display["unchanged"] = true.into();
                }
                Ok(
                    json!({"epoch": self.epoch_text(), "target": {"ref": input["ref"], "kind": "marker"}, "prior": now, "fence": now, "display": display}),
                )
            }
            _ => {
                if params["fence"] != now {
                    return Err(changed(format!("{} moved since the preview", marker.label())));
                }
                let desc = Self::desc(params)?;
                self.markers[place - 1].position_beats = to;
                self.sort_markers();
                let undo = self.applied(Some("Value Change"), &desc)?;
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": {"position": position_of(to)},
                    "display": {"title": title, "before": now, "after": {"position": position_of(to)}},
                    "undo": undo,
                }))
            }
        }
    }

    /// `marker-delete {ref}`: the row's Delete action. Automatic restore refuses: Logic's title proves no identity,
    /// and a marker made again cannot faithfully recover the original name.
    fn marker_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return Err(failed(MANUAL_UNDO));
        }
        let input = &params["input"];
        let place = self.marker_place(input["ref"].as_str())?;
        let Some(marker) = self.markers.get(place - 1).cloned() else {
            return Err(match method {
                "change_preview" => {
                    failed(format!("{} isn't in the project (deleted, or another project)", input["ref"].as_str().unwrap_or_default()))
                }
                _ => changed("the marker went since the preview"),
            });
        };
        let title = format!("Deleted {}", marker.label());
        match method {
            "change_preview" => Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": input["ref"], "kind": "marker"},
                "prior": marker.state(),
                "fence": marker.state(),
                "display": {"title": title},
            })),
            _ => {
                if params["fence"] != marker.state() {
                    return Err(changed(format!("{} changed since the preview", marker.label())));
                }
                let desc = Self::desc(params)?;
                self.markers.remove(place - 1);
                let undo = self.applied(Some("Delete"), &desc)?;
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": {"deleted": true},
                    "material": marker.state(),
                    "display": {"title": title},
                    "undo": undo,
                }))
            }
        }
    }

    /// `tempo-set {bpm}`: the LCD's tempo slider, whole BPM; no step of Logic's undo. Its restore is by value.
    fn tempo_set(&mut self, method: &str, params: &Value) -> Answer {
        let now = json!({"bpm": self.tempo});
        let same = |a: &Value, b: &Value| (a["bpm"].as_f64().unwrap_or(f64::NAN) - b["bpm"].as_f64().unwrap_or(f64::NAN)).abs() < 0.01;
        let epoch = self.epoch_text();
        if method == "change_restore" {
            let prior = params["prior"]["bpm"].as_f64().ok_or_else(|| bad("prior.bpm"))?;
            let desc = Self::desc(params)?;
            if !same(&now, &params["prior"]) {
                if !same(&now, &params["applied"]) && params["discard"] != true {
                    return Err(changed("the tempo has been changed in Logic Pro since Kumi set it"));
                }
                self.tempo = prior;
            }
            let undo = self.restored(None, &desc)?;
            return Ok(
                json!({"epoch": epoch, "restored": {"bpm": prior}, "display": {"title": format!("Tempo back to {prior} BPM"), "before": now, "after": {"bpm": prior}}, "undo": undo}),
            );
        }
        let asked = params["input"]["bpm"].as_f64().filter(|bpm| bpm.is_finite()).ok_or_else(|| bad("bpm must be a number"))?;
        if !(TEMPO_MIN..=TEMPO_MAX).contains(&asked) {
            return Err(bad(format!("bpm must be from {TEMPO_MIN} to {TEMPO_MAX}")));
        }
        let wanted = asked.round();
        match method {
            "change_preview" => {
                let mut display = json!({"title": format!("Tempo {wanted} BPM"), "before": now, "after": {"bpm": wanted}});
                if asked != wanted {
                    display["note"] = format!(
                        "Logic Pro's tempo display takes whole BPM from outside: {} was rounded to {wanted}.",
                        params["input"]["bpm"]
                    )
                    .into();
                }
                if same(&now, &json!({"bpm": wanted})) {
                    display["unchanged"] = true.into();
                }
                Ok(json!({"epoch": epoch, "target": {"ref": null, "kind": "project"}, "prior": now, "fence": now, "display": display}))
            }
            _ => {
                if !same(&now, &params["fence"]) {
                    return Err(changed("the tempo changed since the preview"));
                }
                let desc = Self::desc(params)?;
                self.tempo = wanted;
                let undo = self.applied(None, &desc)?;
                Ok(
                    json!({"epoch": epoch, "applied": {"bpm": wanted}, "display": {"title": format!("Tempo {wanted} BPM"), "before": now, "after": {"bpm": wanted}}, "undo": undo}),
                )
            }
        }
    }

    // ----- the transport and the plan's steps -----

    fn transport(&mut self, method: &str, params: &Value) -> Answer {
        match method {
            "transport_stop" => {
                let was = std::mem::replace(&mut self.playing, false);
                Ok(json!({"stopped": was, "wasRecording": false}))
            }
            "transport_play" => {
                let was = std::mem::replace(&mut self.playing, true);
                Ok(json!({"started": !was, "playing": true}))
            }
            _ => {
                let number = |key: &str| -> Result<Option<f64>, Fault> {
                    match params.get(key) {
                        None | Some(Value::Null) => Ok(None),
                        Some(value) => value
                            .as_f64()
                            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                            .map(Some)
                            .ok_or_else(|| bad(format!("{key} must be a number from 0"))),
                    }
                };
                let beats = match (number("positionBeats")?, number("position")?) {
                    (Some(_), Some(_)) => return Err(bad("pass positionBeats or position, not both")),
                    (Some(beats), None) => beats,
                    (None, Some(seconds)) => seconds * self.tempo.max(1.0) / 60.0,
                    (None, None) => return Err(bad("pass positionBeats (beats from the project's start) or position (seconds)")),
                };
                // The LCD's bar and beat: no finer.
                self.playhead = on_the_beat(beats);
                let position = position_of(self.playhead);
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "positionBeats": self.playhead,
                    "position": self.playhead * 60.0 / self.tempo,
                    "playhead": {"bar": position["bar"], "beat": position["beat"]},
                    "playing": self.playing,
                }))
            }
        }
    }

    /// `undo_step_begin {label}` / `undo_step_end {stepId, label?}`: a plan's step, counted. Logic has no undo block:
    /// the changes made meanwhile are each what they are to Logic's undo, and the end says how many there were.
    fn plan_step(&mut self, method: &str, params: &Value) -> Answer {
        let label = params.get("label").and_then(Value::as_str).map(str::to_owned);
        if method == "undo_step_begin" {
            let label = label.ok_or_else(|| bad("label must be a string"))?;
            if self.open_step.is_some() {
                return Err(failed("a step is open already: end it before beginning another"));
            }
            self.steps += 1;
            let id = format!("s{}", self.steps);
            self.open_step = Some((id.clone(), label, 0));
            return Ok(json!({"stepId": id}));
        }
        let id = params.get("stepId").and_then(Value::as_str).ok_or_else(|| bad("stepId must be a string"))?;
        let (_, begun, changes) = match self.open_step.as_ref() {
            Some((open, _, _)) if open == id => self.open_step.take().expect("the step"),
            _ => return Err(failed("that step isn't open (it ended already)")),
        };
        Ok(json!({"desc": label.unwrap_or(begun), "entry": false, "changes": changes}))
    }

    // ----- the reads, over the project as changed -----

    /// A read's rows that Kumi's changes reach, made from the project as changed over the recorded rows: a page of the
    /// tracks or of the markers, one track by its ref, and the project's, the transport's and the bodies' fields; None
    /// for a read the changes don't touch, which the recorded case answers. `asked` is in the recorded epoch.
    fn changed_rows(&self, method: &str, asked: &Value, params: &Value) -> Option<Answer> {
        let recorded = |method: &str, params: Value| {
            find(&self.cases, method, &params)
                .and_then(|case| case.response.as_ref().ok())
                .cloned()
                .ok_or_else(|| ("bad-params".to_string(), format!("the simulator has no answer scripted for {method} {params}")))
        };
        let kind = asked.get("kind").and_then(Value::as_str);
        let track_rows = || -> Vec<Value> { self.tracks.iter().enumerate().map(|(at, track)| track.row(at + 1)).collect() };
        let marker_rows = || -> Vec<Value> { self.markers.iter().enumerate().map(|(at, marker)| marker.row(at + 1)).collect() };
        let patch_transport = |transport: &mut Value| {
            transport["playing"] = self.playing.into();
            transport["tempo"] = self.tempo.into();
        };
        let playhead = || {
            let position = position_of(self.playhead);
            json!({"bar": position["bar"], "beat": position["beat"]})
        };
        Some(match method {
            "read" if asked.get("ref").is_some() => {
                let reference = asked["ref"].as_str().unwrap_or_default();
                let (_, rest) = reference.split_once(':')?;
                if !rest.starts_with("track:") || epoch_of(reference) != Some(RECORDED_EPOCH) {
                    return None;
                }
                match rest["track:".len()..].parse::<usize>().ok().filter(|place| *place >= 1 && *place <= self.tracks.len()) {
                    Some(place) => Ok(json!({"epoch": RECORDED_EPOCH.to_string(), "items": [self.tracks[place - 1].row(place)]})),
                    None => Err((
                        "failed".to_string(),
                        format!("{} isn't in the project (deleted, or another project)", params["ref"].as_str().unwrap_or_default()),
                    )),
                }
            }
            "read" if kind == Some("track") && asked.get("parent").is_none() => page_of(track_rows(), asked),
            "read" if kind == Some("marker") && asked.get("parent").is_none() => page_of(marker_rows(), asked),
            "read" if kind == Some("project") => recorded("read", asked.clone()).map(|mut page| {
                page["items"][0]["tempo"] = self.tempo.into();
                page["items"][0]["trackCount"] = self.tracks.len().into();
                page
            }),
            "read" if kind == Some("tempo") => recorded("read", asked.clone()).map(|mut page| {
                page["items"][0]["bpm"] = self.tempo.into();
                page
            }),
            "snapshot" => recorded("snapshot", asked.clone()).map(|mut snapshot| {
                snapshot["project"]["tempo"] = self.tempo.into();
                snapshot["project"]["trackCount"] = self.tracks.len().into();
                snapshot["tracks"] = Value::Array(track_rows());
                snapshot["markers"] = Value::Array(marker_rows());
                patch_transport(&mut snapshot["transport"]);
                snapshot["transport"]["playhead"] = playhead();
                snapshot
            }),
            "focus" => recorded("focus", asked.clone()).map(|mut focus| {
                patch_transport(&mut focus["transport"]);
                focus["transport"]["beat"] = self.playhead.into();
                focus["playhead"] = playhead();
                focus
            }),
            "focus_body" if kind == Some("timeline") => recorded("focus_body", json!({"kind": "timeline"})).and_then(|mut whole| {
                let recorded_lanes = whole["lanes"].as_array().cloned().unwrap_or_default();
                let lanes: Vec<Value> = self
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(at, track)| {
                        // A recorded track keeps its lane (its regions); a track Kumi added has none.
                        let mut lane = recorded_lanes
                            .get(track.id as usize - 1)
                            .filter(|_| track.id >= 1)
                            .cloned()
                            .unwrap_or_else(|| json!({"kind": track.kind, "regions": []}));
                        lane["name"] = track.name.clone().into();
                        lane["trackRef"] = format!("{RECORDED_EPOCH}:track:{}", at + 1).into();
                        lane
                    })
                    .collect();
                whole["lanes"] = Value::Array(lanes);
                whole["trackCount"] = self.tracks.len().into();
                whole["playing"] = self.playing.into();
                whole["playheadBeats"] = self.playhead.into();
                whole["markers"] = self.markers.iter().map(|m| json!({"name": m.name, "positionBeats": m.position_beats})).collect();
                timeline_page(&whole, asked)
            }),
            "focus_body" if kind == Some("tree") => {
                let mut body = recorded("focus_body", asked.clone()).ok()?;
                let reference = body["trackRef"].as_str().unwrap_or_default();
                let place =
                    reference.rsplit(':').next().and_then(|n| n.parse::<usize>().ok()).filter(|p| *p >= 1 && *p <= self.tracks.len())?;
                let track = &self.tracks[place - 1];
                let strip = &mut body["strip"];
                strip["name"] = track.name.clone().into();
                strip["volumeDb"] = track.volume_db.into();
                strip["pan"] = track.pan_steps.into();
                strip["mute"] = track.mute.into();
                strip["solo"] = track.solo.into();
                strip["armed"] = track.armed.into();
                Ok(body)
            }
            _ => return None,
        })
    }
}

impl LogicScript {
    /// The project of the vectors: what the bridge answers for Scratch.
    pub fn new() -> Self {
        LogicScript { state: Arc::new(Mutex::new(State::recorded())) }
    }

    /// A track or region was added, removed or moved: the epoch moves, so refs and cursors from before are stale.
    pub fn change_tracks(&self) {
        self.state.lock().unwrap().bump_epoch();
    }

    /// Another project is open: the `n`th copy of the recorded one (0 is the recorded one itself), with an id, an
    /// identity, a name and a path of its own and the same rows, as a copy saved under another name has. The epoch
    /// moves with it, and no ref of the project before names anything in it.
    pub fn open_project(&self, n: u32) {
        let mut state = self.state.lock().unwrap();
        state.epoch += 1;
        state.project = n;
        state.places.clear();
    }

    /// The project open now, as the bridge's `status` would name it: the recorded one, or the copy `open_project` opened.
    pub fn project(&self) -> Project {
        let n = self.state.lock().unwrap().project;
        serde_json::from_value(another_project(&serde_json::to_value(project()).unwrap(), n)).unwrap()
    }

    /// Every request the simulator answered, in order.
    pub fn asked(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().asked.clone()
    }

    /// The epoch the project is in now.
    pub fn epoch(&self) -> u32 {
        self.state.lock().unwrap().epoch
    }

    /// A track's fader, in dB (`index` is its place in the Tracks area, from 1).
    pub fn volume(&self, index: usize) -> f64 {
        self.state.lock().unwrap().tracks[index - 1].volume_db
    }

    /// A track's row as a read shows it now (`index` from 1).
    pub fn track(&self, index: usize) -> Value {
        let state = self.state.lock().unwrap();
        retag(&state.tracks[index - 1].row(index), RECORDED_EPOCH, state.epoch)
    }

    pub fn track_count(&self) -> usize {
        self.state.lock().unwrap().tracks.len()
    }

    /// The producer moves a track's fader in Logic (`index` from 1).
    pub fn set_volume(&self, index: usize, db: f64) {
        self.state.lock().unwrap().tracks[index - 1].volume_db = db;
    }

    /// The producer changes a track's `field` in Logic, as the tool names it: `pan` (-1 to 1), `mute`, `solo`, `arm`,
    /// `name`.
    pub fn set_track(&self, index: usize, field: &str, value: Value) {
        let mut state = self.state.lock().unwrap();
        let track = &mut state.tracks[index - 1];
        match field {
            "pan" => {
                track.pan_steps = ((value.as_f64().unwrap_or(0.0) * PAN_CENTRE as f64).round() as i64).clamp(-PAN_CENTRE, PAN_CENTRE - 1)
            }
            "mute" => track.mute = value == true,
            "solo" => track.solo = value == true,
            "arm" => track.armed = value == true,
            "name" => track.name = value.as_str().unwrap_or_default().to_string(),
            _ => track.volume_db = value.as_f64().unwrap_or(track.volume_db),
        }
    }

    pub fn tempo(&self) -> f64 {
        self.state.lock().unwrap().tempo
    }

    /// The markers, in the Marker List's order: each name and its position in beats.
    pub fn markers(&self) -> Vec<(String, f64)> {
        self.state.lock().unwrap().markers.iter().map(|m| (m.name.clone(), m.position_beats)).collect()
    }

    pub fn playing(&self) -> bool {
        self.state.lock().unwrap().playing
    }

    /// The playhead, in beats from the project's start.
    pub fn playhead(&self) -> f64 {
        self.state.lock().unwrap().playhead
    }

    /// Logic's undo history as the host named its steps (`desc`), oldest first; a step the bridge couldn't name (a
    /// restore's, or a write Logic didn't answer after) is empty.
    pub fn undo_history(&self) -> Vec<String> {
        self.state.lock().unwrap().undo.iter().map(|(_, desc)| desc.clone().unwrap_or_default()).collect()
    }

    /// The same, as Logic's Edit menu titles them.
    pub fn undo_titles(&self) -> Vec<String> {
        self.state.lock().unwrap().undo.iter().map(|(title, _)| title.clone()).collect()
    }

    /// A dialog shows for the next change's preview, and closes right after.
    pub fn busy_once(&self) {
        self.state.lock().unwrap().busy_once = true;
    }

    /// The next write (an apply or a restore) is made, and Logic doesn't answer after it: the bridge answers
    /// `uncertain`.
    pub fn uncertain_once(&self) {
        self.state.lock().unwrap().uncertain_once = true;
    }

    /// The producer presses Cmd-Z in Logic: its last step comes off (its values back as the preview saw them is the
    /// test's business). Tests emit an event only for positive value-backed proof, such as a renamed value restored;
    /// this fixture access is not title-only attribution in the native watcher. The returned Logic title
    /// for the step, and that desc, if the bridge had one.
    pub fn undo_last(&self) -> Option<(String, Option<String>)> {
        self.state.lock().unwrap().undo.pop()
    }

    pub fn respond(&self) -> Respond {
        let script = self.clone();
        Arc::new(move |method, params| script.answer(method, params))
    }

    /// The simulator's config for Logic with this project behind it: the reads, and Kumi's changes, in the bridge's order.
    pub fn config(&self, secret: Secret, resource: &str) -> SimulatorConfig {
        let mut config =
            SimulatorConfig::logic(secret, resource).with_methods(&METHODS, self.respond()).with_methods(&CHANGE_METHODS, self.respond());
        config.identity.project = Some(self.project());
        config.offer.events = ["project", "transport", "tracks", "selection", "undone_in_daw"].map(str::to_string).to_vec();
        config
    }

    fn answer(&self, method: &str, params: &Value) -> Option<Answer> {
        if !METHODS.contains(&method) && !CHANGE_METHODS.contains(&method) {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        state.asked.push((method.to_string(), params.clone()));
        match method {
            "change_preview" | "change_apply" | "change_restore" => {
                let answer = state.change(method, params);
                if method == "change_apply" && answer.is_ok() {
                    if let Some((_, _, changes)) = state.open_step.as_mut() {
                        *changes += 1;
                    }
                }
                return Some(answer);
            }
            "transport_stop" | "transport_play" | "cursor_set" => return Some(state.transport(method, params)),
            "undo_step_begin" | "undo_step_end" => return Some(state.plan_step(method, params)),
            _ => {}
        }
        let (epoch, project) = (state.epoch, state.project);
        // Past the recorded epoch, a ref or cursor from an earlier one is refused before anything is looked up, as the
        // bridge refuses it whatever it was for; in the recorded epoch the refusals recorded answer.
        if epoch != RECORDED_EPOCH {
            if let Some(stale) = stale_in(params, epoch) {
                return Some(Err(stale));
            }
        }
        let answered = |result: &Value| another_project(&retag(result, RECORDED_EPOCH, epoch), project);
        // (whole numbers as integers, so a page asked with limit 1.0 is the page of limit 1)
        let asked = normal(&retag(params, epoch, RECORDED_EPOCH));
        // The rows Kumi's changes reach are made from the project as changed, over the recorded ones.
        if let Some(made) = state.changed_rows(method, &asked, params) {
            return Some(made.map(|result| answered(&result)));
        }
        if let Some(case) = find(&state.cases, method, &asked) {
            return Some(case.response.clone().map(|result| answered(&result)));
        }
        if let Some(stale) = stale_in(params, epoch) {
            return Some(Err(stale));
        }
        Some(Err(("bad-params".into(), format!("the simulator has no answer scripted for {method} {params}"))))
    }
}

impl Default for LogicScript {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recorded_set_answers_verbatim_and_follows_the_epoch() {
        let script = LogicScript::new();
        let answer = |method: &str, params: Value| script.answer(method, &params).expect("a method the simulator has");
        let tracks = answer("read", json!({"kind": "track"})).unwrap();
        assert_eq!((tracks["epoch"].as_str(), tracks["items"][0]["ref"].as_str()), (Some("1"), Some("1:track:1")));
        assert_eq!(answer("read", json!({"kind": "note"})).unwrap_err().0, "bad-params", "the recorded refusal");
        assert_eq!(answer("read", json!({"kind": "region", "parent": "0:track:2"})).unwrap_err().0, "stale");
        assert_eq!(answer("read", json!({"kind": "track", "limit": 1.0})).unwrap()["nextCursor"], "1.1", "whole numbers alike");
        assert!(answer("read", json!({"kind": "note", "parent": "1:region:1", "limit": 3})).unwrap_err().1.contains("no answer scripted"));
        // The epoch moves: refs from before are stale, and the same questions answer in the new epoch.
        script.change_tracks();
        assert_eq!(answer("read", json!({"ref": "1:track:2"})).unwrap_err().0, "stale");
        assert_eq!(
            answer("read", json!({"kind": "track", "cursor": "1.1"})).unwrap_err().1,
            "that cursor is from an earlier epoch (1; now 2): read again from the start"
        );
        let again = answer("read", json!({"kind": "track", "limit": 1})).unwrap();
        assert_eq!(
            (again["epoch"].as_str(), again["items"][0]["ref"].as_str(), again["nextCursor"].as_str()),
            (Some("2"), Some("2:track:1"), Some("2.1"))
        );
        let project = answer("read", json!({"kind": "project"})).unwrap();
        assert_eq!(project["items"][0]["id"], json!(super::project().id), "the same project: its id stays");
        assert_eq!(answer("focus", json!({})).unwrap()["selection"]["trackRef"], "2:track:1");
        // Another project: another id, identity and path, the same rows.
        script.open_project(1);
        let other = answer("snapshot", json!({})).unwrap();
        assert_eq!((other["epoch"].as_str(), other["tracks"][0]["ref"].as_str()), (Some("3"), Some("3:track:1")));
        assert_ne!(other["project"]["id"], json!(super::project().id));
        let copy = super::project().path.map(|path| path.replace(".logicx", " 1.logicx")).expect("the recorded project has a path");
        assert_eq!(other["project"]["path"], json!(copy));
        assert_eq!(script.project().path.as_deref(), Some(copy.as_str()));
        script.open_project(0);
        assert_eq!(answer("snapshot", json!({})).unwrap()["project"]["id"], json!(super::project().id));
    }

    #[test]
    fn a_page_of_the_tracks_area_is_cut_from_the_recorded_one() {
        let script = LogicScript::new();
        let answer = |params: Value| script.answer("focus_body", &params).unwrap();
        let page = answer(json!({"kind": "timeline", "limit": 24, "around": "1:track:1"})).unwrap();
        assert_eq!((page["from"].as_u64(), page["lanes"].as_array().map(Vec::len), page.get("nextCursor")), (Some(0), Some(2), None));
        let one = answer(json!({"kind": "timeline", "limit": 1})).unwrap();
        assert_eq!((one["lanes"][0]["name"].as_str(), one["nextCursor"].as_str()), (Some("Vocal"), Some("1.1")));
        let rest = answer(json!({"kind": "timeline", "limit": 1, "cursor": "1.1"})).unwrap();
        assert_eq!((rest["lanes"][0]["name"].as_str(), rest["from"].as_u64()), (Some("Keys"), Some(1)));
        let later = answer(json!({"kind": "timeline", "window": {"from": 32.0, "to": 48.0}})).unwrap();
        assert!(later["lanes"][1]["regions"].as_array().unwrap().is_empty(), "the region ends before the window: {later}");
        assert_eq!(answer(json!({"kind": "timeline", "around": "1:track:7"})).unwrap_err().0, "failed");
        // The recorded page itself, when the request is the recorded one.
        let recorded = answer(json!({"kind": "timeline", "around": "1:track:2", "window": {"from": 0.0, "to": 16.0}, "limit": 1})).unwrap();
        assert_eq!((recorded["from"].as_u64(), recorded["lanes"][0]["name"].as_str()), (Some(1), Some("Keys")));
    }

    /// A change's value shows in every read after it, over the recorded rows; a header's control is no step of Logic's
    /// undo; a track added goes below the selected one, moves the epoch and takes its place in the pages, the snapshot
    /// and the Tracks area; a restore's ref from before is carried.
    #[test]
    fn a_changed_value_shows_in_the_reads_and_a_track_added_takes_its_place_in_them() {
        let script = LogicScript::new();
        let answer = |method: &str, params: Value| script.answer(method, &params).expect("a method the simulator has");
        let preview =
            answer("change_preview", json!({"kind": "track-volume", "input": {"ref": "1:track:1", "volumeDb": -6}, "epoch": "1"})).unwrap();
        assert_eq!(
            (preview["prior"].clone(), preview["display"]["title"].as_str()),
            (json!({"volumeDb": 0.0}), Some("Vocal volume −6 dB"))
        );
        assert_eq!(script.volume(1), 0.0, "a preview changes nothing");
        let applied = answer(
            "change_apply",
            json!({"kind": "track-volume", "input": {"ref": "1:track:1", "volumeDb": -6}, "fence": {"volumeDb": 0.0}, "desc": "Kumi: Vocal volume −6 dB (k0001)", "epoch": "1"}),
        )
        .unwrap();
        assert_eq!(
            (applied["applied"].clone(), applied["undo"].clone()),
            (json!({"volumeDb": -6.0}), json!({"desc": "Kumi: Vocal volume −6 dB (k0001)", "entry": false})),
            "the value read back after the write; no step of Logic's undo"
        );
        assert_eq!(answer("read", json!({"ref": "1:track:1"})).unwrap()["items"][0]["volumeDb"], -6.0);
        assert_eq!(answer("read", json!({"kind": "track", "limit": 1})).unwrap()["items"][0]["volumeDb"], -6.0);
        assert_eq!(answer("snapshot", json!({})).unwrap()["tracks"][0]["volumeDb"], -6.0);
        assert_eq!(answer("focus_body", json!({"kind": "tree"})).unwrap()["strip"]["volumeDb"], -6.0);
        assert!(script.undo_titles().is_empty());

        // A track added below track 1: the epoch moves, the new track is second everywhere, and the fader's restore
        // still finds track 1.
        let added = answer(
            "change_apply",
            json!({"kind": "track-add", "input": {"name": "Pad", "index": 1}, "fence": {"tracks": 2, "names": ["Vocal", "Keys"]}, "desc": "Kumi: Added audio track “Pad” (k0002)", "epoch": "1"}),
        )
        .unwrap();
        assert_eq!(
            (added["epoch"].as_str(), added["created"]["ref"].as_str(), added["applied"]["kind"].as_str()),
            (Some("2"), Some("2:track:2"), Some("audio"))
        );
        assert_eq!(added["undo"], json!({"desc": "Kumi: Added audio track “Pad” (k0002)", "entry": true, "title": "Undo Create Track"}));
        let page = answer("read", json!({"kind": "track"})).unwrap();
        assert_eq!(
            (page["total"].as_u64(), page["items"][1]["name"].as_str(), page["items"][2]["ref"].as_str()),
            (Some(3), Some("Pad"), Some("2:track:3"))
        );
        assert_eq!(answer("read", json!({"kind": "project"})).unwrap()["items"][0]["trackCount"], 3);
        let timeline = answer("focus_body", json!({"kind": "timeline"})).unwrap();
        assert_eq!(
            (
                timeline["trackCount"].as_u64(),
                timeline["lanes"][1]["name"].as_str(),
                timeline["lanes"][2]["regions"].as_array().map(Vec::len)
            ),
            (Some(3), Some("Pad"), Some(1))
        );
        assert_eq!(answer("read", json!({"ref": "1:track:1"})).unwrap_err().0, "stale", "a read from before is stale");
        let restored = answer(
            "change_restore",
            json!({"kind": "track-volume", "target": {"ref": "1:track:1", "kind": "track"}, "prior": {"volumeDb": 0.0}, "applied": {"volumeDb": -6.0}, "desc": "Kumi: undo Vocal volume −6 dB (k0003)", "epoch": "1"}),
        )
        .unwrap();
        assert_eq!(
            (restored["epoch"].as_str(), restored["restored"].clone(), restored["undo"]["entry"].clone()),
            (Some("2"), json!({"volumeDb": 0.0}), json!(false))
        );
        assert_eq!(script.volume(1), 0.0);
        // The added track taken away again: the epoch moves once more, and the pages are as recorded.
        let gone = answer(
            "change_restore",
            json!({"kind": "track-add", "target": {"ref": null, "kind": "track", "index": 1}, "prior": {"tracks": 2}, "applied": added["applied"], "created": added["created"], "desc": "Kumi: undo Added audio track “Pad” (k0004)", "epoch": "2"}),
        )
        .unwrap();
        assert_eq!((gone["epoch"].as_str(), script.track_count()), (Some("3"), 2));
        assert_eq!(answer("read", json!({"kind": "track"})).unwrap()["items"][1]["name"], "Keys");
        assert_eq!(script.undo_titles(), ["Undo Create Track", "Undo Delete Tracks"]);
        assert_eq!(script.undo_history(), ["Kumi: Added audio track “Pad” (k0002)", ""], "a restore's step the bridge doesn't name");
    }

    #[test]
    fn positions_and_levels_read_as_logic_shows_them() {
        assert_eq!(position_of(32.0), json!({"bar": 9, "beat": 1, "division": 1, "tick": 1}));
        assert_eq!(position_of(5.5), json!({"bar": 2, "beat": 2, "division": 3, "tick": 1}));
        assert_eq!(beats_of(&json!({"bar": 9, "beat": 1, "division": 1, "tick": 1})), 32.0);
        assert_eq!(beats_of(&position_of(5.5)), 5.5);
        assert_eq!(
            (bar_text(16.0), bar_text(18.0), bar_text(18.5)),
            ("bar 5".to_string(), "bar 5 beat 3".to_string(), "bar 5 beat 3.3.1".to_string())
        );
        assert_eq!(
            (db_text(-6.0), db_text(3.5), db_text(0.0), db_text(-96.0)),
            ("−6".to_string(), "+3.5".to_string(), "0".to_string(), "−∞".to_string())
        );
        assert_eq!((pan_text(0), pan_text(-32), pan_text(63)), ("centre".to_string(), "50% left".to_string(), "98% right".to_string()));
        let pan = &TRACK_PROPS[1];
        assert_eq!(pan.wanted(&json!({"pan": -0.5})).unwrap(), json!({"pan": -0.5, "steps": -32}));
        assert_eq!(pan.wanted(&json!({"pan": 1.0})).unwrap()["steps"], 63);
        let arm = &TRACK_PROPS[4];
        assert_eq!(arm.wanted(&json!({"arm": true})).unwrap(), json!({"armed": true}));
        assert_eq!(arm.title("Vocal", &json!({}), &json!({"armed": false})), "Vocal disarmed");
    }
}
