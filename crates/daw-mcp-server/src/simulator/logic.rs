//! A scripted Logic Pro project for the host's tests: what Kumi's own bridge beside Logic (`logic-kumi`) answers to
//! `read`, `snapshot`, `focus` and `focus_body` (the M2 contract, docs/en/LOGIC_CONTRACT.md), replayed verbatim; and
//! what it answers to Kumi's changes (M3): `change_preview`, `change_apply` and `change_restore`, the transport and the
//! playhead, and a plan's steps, counted, since Logic has no undo block.
//!
//! What the bridge answers to the fixed request set of M2's brief over the Scratch project is kept in
//! tests/vectors/logic-reads.json, which the bridge's own tests write from the real Logic over its AX fixtures
//! (KUMI_BLESS=1 in crates/logic-kumi/tests/reads.rs). This replays those answers, so the simulator cannot drift from
//! the bridge. The project can be changed, as a track or region added, removed or moved changes it in Logic: the epoch
//! moves, refs and cursors from before are refused as `stale`, and the answers carry the new epoch. Another project can
//! be opened: the epoch moves and the project's id, identity, name and path are another's. A page of the Tracks area
//! nobody recorded (FOCUS asks for a screenful of lanes around the selected track) is cut from the recorded one, as the
//! bridge pages it.
//!
//! Kumi's changes are made on the project as changed over the recorded rows, the way the bridge makes them (the
//! contract's Changes section): a preview that writes nothing and fences what it saw, an apply that checks the fence
//! again, writes and answers the value read back, a restore by value fenced by what the apply wrote. Logic keeps no
//! undo step for a header's control (volume, pan, mute, solo, arm) or the tempo, so those answer `undo.entry: false`
//! with no title; a rename ("Renaming"), a track made ("Create Track", then "Renaming" when it is given a name, which
//! goes through the Inspector once the track is made) or deleted ("Delete Tracks") and a marker made ("Create Marker"),
//! moved ("Value Change") or deleted ("Delete") are steps of Logic's undo, named by the Edit menu's title and by the
//! `desc` the host sent. A track added goes below the selected one (or the one `index` names), named after its channel
//! strip ("Audio 2", "Inst 2": counted by the tracks of its kind), and moves the epoch, as the M2 replay moves it; a
//! restore's ref from before is carried to where its track is now. A marker Logic makes is 0 beats long and named by
//! its place in the Marker List ("Marker 1" is the first, so one made before it renumbers the rest), as the M3 and M4
//! scenes read. A deleted track comes back only through Logic's own undo (Cmd-Z), while that step is still on top; a
//! deleted marker the same way first, else made again by value. The reads replay the vectors with the changed
//! values in them. A dialog (`busy_once`) refuses the next preview before anything is written; a write Logic doesn't
//! answer after (`uncertain_once`) is made and answered `uncertain`. The kinds the bridge can't make are refused saying
//! why, as it refuses them; of the kinds it makes, the simulator makes those whose state is a value here, and says so
//! for the rest (notes).
//!
//! The M4 kinds answer as the bridge's recorded scenes over the real Logic do (`crates/logic-kumi/tests/fixtures`): a
//! key command resolved by its name or id from Logic's own table (a few of them, with their factory chords), a row only
//! when Logic's Edit menu names a step of it; regions moved in time, resized and split on whole beats, looped, renamed
//! and deleted, each answering the region's state as the bridge keeps it and moving the epoch as it does; the cycle
//! with its locators; the Tempo and Signature Lists; a MIDI file imported onto a new track Logic adds; Track Stacks
//! Logic names and gathers. Logic's undo steps carry the titles its Edit menu gave them there ("Split Regions", "Loop
//! Parameter Change", "Insert Tempo" then "Value Change", "Flatten Stack"), keyed by the change's kind and target as
//! the bridge's undo watch keys them, so a restore through Logic's own undo finds its step on top or says it can't.
//!
//! The second M4 slice (`logic/mixer.rs`, `logic/render.rs`) holds each channel strip's plug-ins (with the rows their
//! window's Controls View lists and the settings their next and previous buttons step through), sends and patch, the
//! Stereo Out's plug-ins (`<epoch>:output:1`, which no track has), the auxes Logic makes for a bus, and each track's
//! Smart Controls and automation; it makes the plug-in, patch, send and Smart Control kinds and the bypass as the
//! bridge's second slice makes them (`crates/logic-kumi/src/changes`, its `change-fx-*` scenes), and the automation
//! kinds in the shared brief's shapes; it refuses `fx-move` and `send-mode` as the bridge does, answers `param`,
//! `smart` and `automation` reads and a strip's tree and routing bodies, and writes a bounce's and an export's audio
//! files where it is asked to (`bounce`, `export_tracks`: no change, the bridge's answers).
//!
//! (M5, `logic/listen.rs`) Logic is heard as the bridge hears it, with audio the simulation makes: `hear` plays a part
//! and captures it (each track a sine of its own through its fader, pan and plug-ins, then the Stereo Out's), `audition`
//! takes such a pass for each candidate, `render` bounces a part offline; each writes a WAV file into the folder Kumi
//! names and answers what the file holds, and none changes the project.
use super::replay::{epoch_of, find, load_cases, normal, retag, Case, RECORDED_EPOCH};
use super::{Respond, SimulatorConfig};
use daw_bridge::auth::Secret;
use daw_bridge::wire::Project;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod listen;
mod mixer;
mod render;
mod sampler;
use mixer::{AutoEvent, Aux, Channel, Output, Plugin, Smart};

const VECTORS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/logic-reads.json"));
const VECTORS_NAME: &str = "tests/vectors/logic-reads.json";
/// The timeline body's page, in tracks.
const MAX_LANES: usize = 500;
/// A read's page: the contract's.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
/// The reads, in the order the bridge's welcome lists them after ping and status.
const METHODS: [&str; 4] = ["focus", "read", "snapshot", "focus_body"];
/// What the project answers besides: Kumi's changes, the transport and the playhead, a plan's steps, (M4) the project
/// saved, and (M4b) a bounce and the tracks exported, none of which is a change.
const CHANGE_METHODS: [&str; 11] = [
    "change_preview",
    "change_apply",
    "change_restore",
    "transport_stop",
    "transport_play",
    "cursor_set",
    "undo_step_begin",
    "undo_step_end",
    "project_save",
    "bounce",
    "export_tracks",
];
/// (M5) Listening: a part played and captured, a pass per candidate, a part bounced offline. None is a change.
const LISTEN_METHODS: [&str; 3] = ["hear", "audition", "render"];
/// The kinds the bridge can't make, and why, as it refuses them.
const NOT_YET: [(&str, &str); 5] = [
    ("marker-rename", "a marker's name cell in the Marker List isn't a text field"),
    ("send-mode", "a send's mode is a mark in its menu, which Logic Pro draws itself and Accessibility can't read, so it could never be read back"),
    ("fx-move", "Logic Pro moves a plug-in from one slot to another only by dragging it, and no key command or menu item moves one"),
    ("track-move", "the track headers can't be dragged from outside, no key command reorders tracks, and the headers offer no action but their menu"),
    ("track-color", "Logic Pro's Color palette shows its swatches to Accessibility as bare positions, and the track headers show no colour: nothing to read back"),
];
/// (M6) The kinds the bridge's bulk read answers (`read.rs`'s `BULK_KINDS`), and the most rows of a kind it answers.
const BULK_KINDS: [&str; 7] = ["project", "track", "region", "plugin", "send", "marker", "tempo"];
const MOST_BULK: usize = 10_000;
/// (M6) Where Spotlight found one of the recent projects (one outside `~/Music`, which macOS keeps from the bridge).
const RECENT_FOUND: &str = "/Users/me/Projects/Night Drive.logicx";
/// The kinds the bridge makes whose state the simulator doesn't hold.
const NOT_SIMULATED: [&str; 4] = ["notes-add", "notes-change", "notes-delete", "notes-quantize"];
/// The kinds the simulator makes, for the refusal that names them.
const SIMULATED: &str = "track-volume, track-pan, track-mute, track-solo, track-arm, track-rename, track-add, track-delete, track-stack, track-stack-flatten, marker-add, marker-move, marker-delete, tempo-set, tempo-marker-set, tempo-marker-delete, signature-set, loop-range, region-move, region-resize, region-split, region-loop, region-rename, region-delete, midi-import, key-command, fx-add, fx-delete, fx-bypass, fx-param, fx-params, fx-preset, patch-load, send-add, send-remove, send-volume, smart-control-set, automation-add, automation-change, automation-delete, metronome-set and sample-load";
/// (M4) What a key command's answers say of taking it back, and what one Logic kept no step of says, as the bridge
/// says them (`changes/command.rs`).
const KEY_NOTE: &str = "Only Logic Pro's own undo can take this back (Cmd-Z in Logic Pro).";
const KEY_NO_STEP: &str = "Logic Pro made no undo step of it: it changed nothing its history keeps (a view, selection or transport command), so it has no row in HISTORY.";
/// (M4) A key command the simulated Logic knows, as the bridge resolves it from `keys/names.json` and the factory U.S.
/// bindings (`keys/us-bindings.json`): its id, its English name (none for an id Logic names nowhere), its chord as the
/// Key Commands window shows it (none when unbound), and the step Logic's history takes for it (none for a selection).
struct KeyCommand {
    id: u32,
    name: Option<&'static str>,
    chord: Option<&'static str>,
    step: Option<&'static str>,
}

impl KeyCommand {
    /// The command as a title says it: its name in quotes, or its id.
    fn label(&self) -> String {
        match self.name {
            Some(name) => quoted(name),
            None => format!("key command {}", self.id),
        }
    }
}

static KEY_COMMANDS: [KeyCommand; 10] = [
    KeyCommand { id: 766, name: Some("Select All"), chord: Some("⌘A"), step: None },
    KeyCommand { id: 347, name: Some("Deselect All"), chord: Some("⇧D"), step: None },
    KeyCommand { id: 513, name: Some("Create Marker"), chord: Some("⌥'"), step: Some("Create Marker") },
    KeyCommand { id: 574, name: Some("Split Regions/Events at Playhead Position"), chord: Some("⌘T"), step: Some("Split Regions") },
    KeyCommand { id: 503, name: Some("Set Left Locator by Playhead"), chord: None, step: None },
    KeyCommand { id: 1749, name: Some("Bounce Tracks in Place"), chord: Some("⌃⌘B"), step: None },
    KeyCommand { id: 761, name: Some("Undo"), chord: Some("⌘Z"), step: None },
    KeyCommand { id: 796, name: Some("Redo"), chord: Some("⇧⌘Z"), step: None },
    KeyCommand { id: 754, name: None, chord: Some("⌘S"), step: None },
    KeyCommand { id: 32, name: None, chord: Some("⇧⌘S"), step: None },
];
/// The commands the bridge never runs as a change, by id: Save and Save As (`project_save`), Undo and Redo.
const KEY_REFUSED_IDS: [(u32, &str); 4] = [
    (754, "saves the project: use project_save"),
    (32, "saves the project as: use project_save"),
    (761, "moves Logic Pro's undo history under Kumi's HISTORY: Kumi's own changes are taken back by Kumi's undo, anything else is the producer's to undo in Logic Pro"),
    (796, "moves Logic Pro's undo history under Kumi's HISTORY: anything to redo is the producer's to redo in Logic Pro (Shift-Cmd-Z)"),
];
/// (M4) Where the bridge writes the MIDI file it imports, the most notes one import takes, and what Logic names what
/// the import makes: the file carries no track name, so the region is Logic's own "MIDI Region", on a new software
/// instrument track named after Logic's default patch (as the M4 gate's run read them).
const IMPORT_FOLDER: &str = "/tmp/kumi-logic";
const MOST_IMPORTED: usize = 10_000;
const IMPORT_REGION: &str = "MIDI Region";
const IMPORT_TRACK: &str = "Studio Grand";
/// (M4) The recorded project's cycle: off, its locators at bar 1 and bar 5 (as the M4 gate's scenes read them).
const CYCLE: (bool, f64, f64) = (false, 0.0, 16.0);
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
/// How long a marker Logic makes is: none (the M3 and M4 scenes read a new marker's length as 0).
const MARKER_LENGTH: f64 = 0.0;
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
    /// (M4) The Track Stack this track is in: its own track's id.
    in_stack: Option<u64>,
    /// (M4b) Its channel strip's plug-ins, sends and patch; its patch's Smart Controls; its automation, by parameter.
    channel: Channel,
    smart: Vec<Smart>,
    automation: Vec<AutoEvent>,
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
            in_stack: None,
            channel: Channel::default(),
            smart: vec![],
            automation: vec![],
        }
    }

    /// A track Logic's Track menu makes: an audio, software instrument or external MIDI track, named.
    fn made(id: u64, name: &str, kind: &str) -> Track {
        let row = json!({
            "armed": false, "color": null, "index": 0, "inputs": if kind == "audio" { json!(["Input 1"]) } else { json!([]) },
            "kind": kind, "monitoring": false, "mute": false, "name": name, "output": "Stereo Output", "pan": 0,
            "ref": "", "selected": false, "solo": false, "volumeDb": 0.0,
        });
        Track {
            id,
            row,
            name: name.into(),
            kind: kind.into(),
            volume_db: 0.0,
            pan_steps: 0,
            mute: false,
            solo: false,
            armed: false,
            in_stack: None,
            channel: Channel::default(),
            smart: vec![],
            automation: vec![],
        }
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

    /// (M4) A Track Stack's own track, as Logic makes it and the bridge's rows read it (`rows.rs`): a summing stack's
    /// is an aux fed by the bus it mixes through, a folder stack's has no input or output of its own (the strip reads
    /// as an external MIDI track's); its header row carries `stack: true`.
    fn stack(id: u64, name: &str, kind: &str, bus: usize) -> Track {
        let summing = kind == "summing";
        let row_kind = if summing { "aux" } else { "external" };
        let row = json!({
            "armed": false, "color": null, "index": 0, "inputs": if summing { json!([format!("Bus {bus}")]) } else { json!([]) },
            "kind": row_kind, "monitoring": false, "mute": false, "name": name, "output": if summing { "Stereo Output" } else { "" },
            "pan": 0, "ref": "", "selected": false, "solo": false, "stack": true, "volumeDb": 0.0,
        });
        Track {
            id,
            row,
            name: name.into(),
            kind: row_kind.into(),
            volume_db: 0.0,
            pan_steps: 0,
            mute: false,
            solo: false,
            armed: false,
            in_stack: None,
            channel: Channel::default(),
            smart: vec![],
            automation: vec![],
        }
    }

    /// Whether it is a Track Stack's own track.
    fn is_stack(&self) -> bool {
        self.row["stack"] == true
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
    /// The name the marker was given; none for one Logic names itself, which its Marker List shows as "Marker <its
    /// place>": a marker made before it renumbers it (the M4 key command scene: a marker made at bar 1 is "Marker 1",
    /// and the one at bar 9 reads "Marker 2" from then on).
    own_name: Option<String>,
    position_beats: f64,
    length: f64,
}

/// A marker's name as Logic gives it, or none when it is Logic's own numbering ("Marker 2").
fn own_marker_name(name: &str) -> Option<String> {
    let numbered = name.strip_prefix("Marker ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!numbered).then(|| name.to_string())
}

impl Marker {
    /// A marker Logic makes (and names itself), as long as Logic makes it: 0 beats (the M3 and M4 scenes).
    fn made(position_beats: f64) -> Marker {
        Marker { own_name: None, position_beats, length: MARKER_LENGTH }
    }
    /// Its name as the Marker List shows it at `place` (from 1).
    fn name_at(&self, place: usize) -> String {
        self.own_name.clone().unwrap_or_else(|| format!("Marker {place}"))
    }
    /// What a marker is, as the Marker List shows it at `place` (the bridge's state).
    fn state(&self, place: usize) -> Value {
        json!({"position": position_of(self.position_beats), "name": self.name_at(place), "length": self.length, "text": ""})
    }
    /// The row as the Marker List reads it, at `place` (from 1).
    fn row(&self, place: usize) -> Value {
        json!({"length": self.length, "name": self.name_at(place), "position": position_of(self.position_beats), "ref": format!("{RECORDED_EPOCH}:marker:{place}"), "text": ""})
    }
    /// The marker at `place` as a title says it: `marker “Marker 1”`.
    fn label(&self, place: usize) -> String {
        format!("marker {}", quoted(&self.name_at(place)))
    }
}

/// (M4) One of the project's regions, as Kumi's changes left it: the recorded row's fields, the track it sits on (by
/// the track's id, through the places it has over the epochs), and its notes as the Event List lists them.
#[derive(Clone)]
struct Region {
    /// Tells a region apart through the places it has over the epochs.
    id: u64,
    track: u64,
    name: String,
    kind: String,
    start: f64,
    end: f64,
    /// The Region inspector's Loop checkbox (the Tracks area's rows never show it: their `looped` reads false).
    looping: bool,
    selected: bool,
    /// Each note as its Event List row less its refs and position (`channel`, `lengthTicks`, `pitch`, `velocity`,
    /// `selected` when it is), with `beat`: its start from the region's start, in beats.
    notes: Vec<Value>,
}

impl Region {
    /// A recorded row, with the notes recorded for it (their positions are the project's: made the region's).
    fn recorded(id: u64, row: &Value, track: u64, notes: &[Value]) -> Region {
        let start = row["startBeats"].as_f64().unwrap_or(0.0);
        let notes = notes
            .iter()
            .map(|note| {
                let mut kept = note.clone();
                let beat = beats_of(&note["position"]) - start;
                if let Some(map) = kept.as_object_mut() {
                    map.remove("position");
                    map.remove("ref");
                    map.remove("regionRef");
                }
                kept["beat"] = beat.into();
                kept
            })
            .collect();
        Region {
            id,
            track,
            name: row["name"].as_str().unwrap_or_default().to_string(),
            kind: row["kind"].as_str().unwrap_or("midi").to_string(),
            start,
            end: row["endBeats"].as_f64().unwrap_or(start),
            looping: row["looped"] == true,
            selected: row["selected"] == true,
            notes,
        }
    }

    /// The row as a read shows it at `place` (from 1), its track at `track_place`, in the recorded epoch: `looped` as
    /// the Tracks area reads it (never true: Accessibility shows no loop there).
    fn row(&self, place: usize, track_place: usize) -> Value {
        alphabetical(json!({
            "endBeats": self.end, "kind": self.kind, "looped": false, "name": self.name,
            "ref": format!("{RECORDED_EPOCH}:region:{place}"), "selected": self.selected, "startBeats": self.start,
            "trackRef": format!("{RECORDED_EPOCH}:track:{track_place}"),
        }))
    }

    /// The region as the Tracks area's lane lists it.
    fn lane_row(&self, place: usize) -> Value {
        alphabetical(json!({
            "endBeats": self.end, "kind": self.kind, "name": self.name, "ref": format!("{RECORDED_EPOCH}:region:{place}"),
            "selected": self.selected, "startBeats": self.start,
        }))
    }

    /// Its notes as the Event List's rows, each at its place in the region (from 1), in the recorded epoch.
    fn note_rows(&self, place: usize) -> Vec<Value> {
        self.notes
            .iter()
            .enumerate()
            .map(|(n, note)| {
                let mut row = note.clone();
                let beat = row.as_object_mut().and_then(|map| map.remove("beat")).and_then(|beat| beat.as_f64()).unwrap_or(0.0);
                row["position"] = position_of(self.start + beat);
                row["ref"] = format!("{RECORDED_EPOCH}:note:{place}.{}", n + 1).into();
                row["regionRef"] = format!("{RECORDED_EPOCH}:region:{place}").into();
                alphabetical(row)
            })
            .collect()
    }

    /// The region as a title says it: `region “Keys”`.
    fn label(&self) -> String {
        format!("region {}", quoted(&self.name))
    }
}

/// A number in a state, NaN when it isn't there.
fn number(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(f64::NAN)
}

/// (M4) Two positions in beats within a tick of each other, as the bridge compares them.
fn close_beats(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0 / TICKS_PER_BEAT
}

/// (M4) Whether a position is on a beat (no division or tick past it): where Logic's playhead display can go.
fn on_a_beat(beats: f64) -> bool {
    let position = position_of(beats);
    position["division"] == 1 && position["tick"] == 1
}

/// (M4) A position in beats on Logic's ticks (960 to a beat), where the LCD's locators sit.
fn on_ticks(beats: f64) -> f64 {
    (beats * TICKS_PER_BEAT).round() / TICKS_PER_BEAT
}

/// (M4) Names as a title lists them: `“Vocal”`, `“Vocal” and “Keys”`, `“Vocal”, “Keys” and “Bass”`.
fn and_list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<String> = names.map(quoted).collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

/// A row with its keys in alphabetical order, as the bridge writes its rows (the goldens are the wire's bytes).
fn alphabetical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut fields: Vec<(String, Value)> = map.into_iter().map(|(key, field)| (key, alphabetical(field))).collect();
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(fields.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(alphabetical).collect()),
        other => other,
    }
}

/// (M4) A note of a MIDI import, as the bridge's file takes it: beats from the file's start, a length in beats, a
/// channel from 1.
#[derive(Clone, Copy)]
struct ImportNote {
    pitch: u8,
    start: f64,
    length: f64,
    velocity: u8,
    channel: u8,
}

impl ImportNote {
    /// The note as the region holds it: an Event List row less its position, `beat` from the region's start.
    fn row(&self) -> Value {
        json!({
            "beat": self.start, "channel": self.channel, "lengthTicks": (self.length * TICKS_PER_BEAT).round() as i64,
            "pitch": self.pitch, "velocity": self.velocity,
        })
    }
}

/// (M4) The Standard MIDI File the bridge writes for an import (`changes/smf.rs`): format 0, one track, 480 ticks a
/// quarter note, each note an on and an off (the offs first at a tick), no tempo, no running status. The bridge names
/// the file by its bytes, so the simulator's answer names the file the bridge would write.
fn midi_file(notes: &[ImportNote]) -> Vec<u8> {
    const TICKS_PER_QUARTER: u16 = 480;
    let ticks = |beats: f64| (beats.max(0.0) * f64::from(TICKS_PER_QUARTER)).round() as u64;
    // (tick, 0 for an off and 1 for an on, status, pitch, velocity): the order the file's events go in
    let mut events: Vec<(u64, u8, u8, u8, u8)> = Vec::with_capacity(notes.len() * 2);
    for note in notes {
        let channel = note.channel.clamp(1, 16) - 1;
        let start = ticks(note.start);
        events.push((start, 1, 0x90 | channel, note.pitch & 0x7F, note.velocity.clamp(1, 127)));
        events.push((start + ticks(note.length).max(1), 0, 0x80 | channel, note.pitch & 0x7F, 0));
    }
    events.sort();
    let mut track = Vec::with_capacity(events.len() * 5 + 4);
    let mut at = 0;
    for (tick, _, status, pitch, velocity) in events {
        // The delta as a variable-length quantity.
        let mut delta = tick - at;
        let mut bytes = vec![(delta & 0x7F) as u8];
        delta >>= 7;
        while delta > 0 {
            bytes.push((delta & 0x7F) as u8 | 0x80);
            delta >>= 7;
        }
        bytes.reverse();
        track.extend_from_slice(&bytes);
        track.extend_from_slice(&[status, pitch, velocity]);
        at = tick;
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let mut file = Vec::with_capacity(track.len() + 22);
    file.extend_from_slice(b"MThd");
    file.extend_from_slice(&6u32.to_be_bytes());
    file.extend_from_slice(&0u16.to_be_bytes());
    file.extend_from_slice(&1u16.to_be_bytes());
    file.extend_from_slice(&TICKS_PER_QUARTER.to_be_bytes());
    file.extend_from_slice(b"MTrk");
    file.extend_from_slice(&(track.len() as u32).to_be_bytes());
    file.extend_from_slice(&track);
    file
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

/// A step of Logic's undo history: the Edit menu's title for it; the `desc` the host sent with the change that made
/// it (the bridge keeps a change under its last step, so the steps before it carry none, nor does a restore's step or
/// a write Logic didn't answer after); and the change's kind and target, as the bridge's undo watch keys Kumi's steps.
struct UndoStep {
    title: String,
    desc: Option<String>,
    key: Option<String>,
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
    /// The tracks Kumi deleted, by the apply's `desc`: Logic's own undo brings one back whole.
    deleted: HashMap<String, Track>,
    /// The markers, in the Marker List's order (by position).
    markers: Vec<Marker>,
    tempo: f64,
    /// (M4) The project's regions, in the Tracks area's order (by track, then by start); which region stood at each
    /// place in every epoch before this one, for a restore's ref; and the regions Kumi deleted, by the apply's `desc`,
    /// which Logic's own undo brings back whole.
    regions: Vec<Region>,
    region_places: HashMap<u32, Vec<u64>>,
    next_region: u64,
    deleted_regions: HashMap<String, Region>,
    /// (M4) The Tempo List past the project's tempo at the start: each change's beat and BPM, by beat; and the
    /// Signature List: the project's signature at the start, and each change's beat and signature.
    tempo_changes: Vec<(f64, f64)>,
    signature: (i64, i64),
    signature_changes: Vec<(f64, (i64, i64))>,
    /// (M4) The cycle: on, and the locators in beats.
    cycle: (bool, f64, f64),
    /// (M4) How many summing and folder stacks Logic has made (it names them Sum 1, Folder 1); the tracks' order before
    /// each stack Kumi made, by the apply's `desc` (Logic's own undo of it puts them back exactly); and each stack Kumi
    /// flattened, its own track and its tracks, by the apply's `desc` (Logic's own undo brings it back).
    summing_stacks: usize,
    folder_stacks: usize,
    stacked: HashMap<String, Vec<u64>>,
    flattened: HashMap<String, (Track, Vec<u64>)>,
    /// (M4) The package the project was saved as, if Kumi saved it as another; every path saved to.
    saved_as: Option<String>,
    saved_paths: Vec<String>,
    /// (M4b) The Mixer's output strips, which no track has (the Stereo Out), and the auxes Logic made for a bus, which
    /// have no ref.
    outputs: Vec<Output>,
    auxes: Vec<Aux>,
    /// (M4b) What Logic's own undo brings back whole: a plug-in Kumi removed, and a strip as it was before a patch Kumi
    /// loaded, by the apply's `desc`.
    removed_plugins: HashMap<String, Plugin>,
    loaded_patches: HashMap<String, (Channel, Vec<Smart>)>,
    /// The playhead, in beats from the project's start.
    playhead: f64,
    playing: bool,
    /// (M6) The control bar's Metronome Click: on, as Scratch's scenes recorded it.
    metronome: bool,
    /// (M6) The octave Logic's editors name middle C in ("Display Middle C As": 3 for C3, Yamaha, Logic's default; 4
    /// for C4, Roland), which the project row says: the recorded one's until the producer chooses the other.
    middle_c: i64,
    /// Logic's undo history, newest last.
    undo: Vec<UndoStep>,
    /// A plan's step, counted (Logic has no undo block): its id and label, and the changes applied in it so far.
    open_step: Option<(String, String, usize)>,
    steps: u32,
    /// A dialog shows for the next change preview only (it closes right after).
    busy_once: bool,
    /// The next write (an apply or a restore) is made, and Logic doesn't answer after it.
    uncertain_once: bool,
    /// The next restore alone is made, and Logic doesn't answer after it (REAPER's simulator's switch).
    uncertain_restore_once: bool,
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

/// A time signature from its text (`4/4`), as the Signature List shows it.
fn signature_of(text: &str) -> Option<(i64, i64)> {
    let (num, den) = text.split_once('/')?;
    Some((num.trim().parse().ok()?, den.trim().parse().ok()?))
}

/// The rows a recorded read of `kind` under `parent` answered, if one was recorded.
fn rows_of(cases: &[Case], kind: &str, parent: &str) -> Vec<Value> {
    find(cases, "read", &json!({"kind": kind, "parent": parent}))
        .and_then(|case| case.response.as_ref().ok())
        .and_then(|page| page["items"].as_array().cloned())
        .unwrap_or_default()
}

/// (M4) A key command's name as the bridge matches it (`keys.rs`): lower-cased, without a trailing ellipsis, the
/// spaces folded.
fn folded(name: &str) -> String {
    let mut text = name.trim().to_lowercase();
    while text.ends_with('…') || text.ends_with("...") {
        text = text.trim_end_matches('…').trim_end_matches("...").trim_end().to_string();
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// (M4) Why the bridge never runs a command it resolved, by its name (`changes/command.rs`): a dialog, leaving the
/// project, a save, Logic's own undo history.
fn refused_by_name(name: &str) -> Option<&'static str> {
    if name.ends_with('…') || name.ends_with("...") {
        return Some("opens a dialog, which Kumi never runs unattended");
    }
    let lower = name.to_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    if starts("quit") {
        return Some("quits Logic Pro");
    }
    if starts("open ") && !starts("open track stack") && !lower.contains("list") && !lower.contains("editor") && !lower.contains("window")
        || starts("close project")
        || starts("new project")
        || starts("new from template")
    {
        return Some("opens, closes or switches projects");
    }
    if starts("save") || starts("revert to") {
        return Some("saves or reverts the project: use project_save");
    }
    if starts("import") || starts("export") || starts("bounce") || starts("share") {
        return Some("imports, exports or bounces through a dialog, which Kumi never runs unattended");
    }
    if lower == "undo" || lower == "redo" || starts("delete undo history") {
        return Some("moves Logic Pro's undo history under Kumi's HISTORY");
    }
    None
}

/// (M4) The key command a request names, as the bridge resolves it (`keys.rs`): by its English name as Logic Pro's Key
/// Commands window lists it (case, a trailing ellipsis and spaces aside) or by its id; then refused, as the bridge
/// refuses it, when it saves, moves Logic's undo history, opens a dialog or leaves the project, or has no key. The
/// simulated Logic knows the commands of `KEY_COMMANDS`: another name is one Logic hasn't (refused with the closest it
/// knows), another id one the simulator doesn't model.
fn key_command_of(input: &Value) -> Result<&'static KeyCommand, Fault> {
    let asked = input.get("command").filter(|command| !command.is_null()).ok_or_else(|| {
        bad("command: a key command's English name as Logic Pro's Key Commands window lists it (\"Select All\"), or its id")
    })?;
    let by_id = |id: u64| -> Result<&'static KeyCommand, Fault> {
        let id = u32::try_from(id)
            .ok()
            .filter(|id| (1..5000).contains(id))
            .ok_or_else(|| bad("command: an id is a whole number from 1 to 4999"))?;
        KEY_COMMANDS.iter().find(|command| command.id == id).ok_or_else(|| {
            let known: Vec<String> = KEY_COMMANDS.iter().map(|command| command.id.to_string()).collect();
            bad(format!(
                "the simulated Logic Pro knows only the key commands {} by id; the bridge resolves every id Logic Pro has",
                known.join(", ")
            ))
        })
    };
    let command = match asked {
        Value::Number(id) => by_id(id.as_u64().unwrap_or(0))?,
        Value::String(text) if text.trim().parse::<u32>().is_ok() => by_id(text.trim().parse().unwrap_or(0))?,
        Value::String(text) => key_command_named(text)?,
        _ => return Err(bad("command: a key command's English name as Logic Pro's Key Commands window lists it, or its id")),
    };
    let refused =
        KEY_REFUSED_IDS.iter().find(|(id, _)| *id == command.id).map(|(_, why)| *why).or_else(|| command.name.and_then(refused_by_name));
    if let Some(why) = refused {
        return Err(bad(format!("{} {why}: Kumi won't run it as a change", command.label())));
    }
    if command.chord.is_none() {
        return Err(bad(format!(
            "{} isn't bound to any key in Logic Pro's key command set, so Kumi can't post it: assign one in Logic Pro > Key Commands",
            command.label()
        )));
    }
    Ok(command)
}

/// (M4) A key command by its name, or the bridge's refusal naming the closest ones the simulated Logic knows.
fn key_command_named(text: &str) -> Result<&'static KeyCommand, Fault> {
    let wanted = folded(text);
    if wanted.is_empty() {
        return Err(bad("command: a key command's name, as Logic Pro's Key Commands window lists it"));
    }
    let named = || KEY_COMMANDS.iter().filter_map(|command| command.name.map(|name| (folded(name), name, command)));
    if let Some((_, _, command)) = named().find(|(known, _, _)| *known == wanted) {
        return Ok(command);
    }
    let words: Vec<&str> = wanted.split(' ').filter(|word| word.len() > 2).collect();
    let mut close: Vec<(usize, String, &str)> = named()
        .map(|(known, name, _)| (words.iter().filter(|word| known.contains(**word)).count(), known, name))
        .filter(|(hits, _, _)| *hits > 0)
        .collect();
    close.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(&b.1)));
    let close: Vec<String> = close.into_iter().take(5).map(|(_, _, name)| quoted(name)).collect();
    Err(bad(if close.is_empty() {
        format!("Logic Pro has no key command named {}", quoted(text))
    } else {
        format!("Logic Pro has no key command named {}; close: {}", quoted(text), close.join(", "))
    }))
}

/// Thirty-two hex digits from a path, as the project's id is a hash of its path (the bridge's).
fn id_of_path(path: &str) -> String {
    let fnv = |seed: u64| path.bytes().fold(seed, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{:016x}{:016x}", fnv(0xcbf2_9ce4_8422_2325), fnv(0x84222325_cbf29ce4))
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

/// (M4) The project row of `value`, if it holds one, as the package Kumi saved the project as: that path, its stem as
/// the name and identity, and an id hashed from the path, as the bridge's ids are.
fn saved_project(value: &Value, path: &str) -> Value {
    match value {
        Value::Object(map) => {
            let is_project = map.get("id").and_then(Value::as_str).is_some_and(|id| id.len() == 32)
                && (map.contains_key("path") || map.contains_key("identity"));
            let stem = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".logicx");
            Value::Object(
                map.iter()
                    .map(|(key, field)| {
                        let field = match (is_project, key.as_str()) {
                            (true, "id") => Value::String(id_of_path(path)),
                            (true, "identity" | "name") => Value::String(stem.to_string()),
                            (true, "path") => Value::String(path.to_string()),
                            _ => saved_project(field, path),
                        };
                        (key.clone(), field)
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| saved_project(item, path)).collect()),
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
        let mut tracks: Vec<Track> = rows("track").iter().enumerate().map(|(n, row)| Track::recorded(n as u64 + 1, row)).collect();
        // (M4b) Each strip as the recorded project holds it: Vocal's plug-in as its row reads, Keys playing Studio Grand
        // (the M2 scene's Mixer), and the Stereo Out with no plug-in.
        for (at, track) in tracks.iter_mut().enumerate() {
            track.channel = mixer::recorded_channel(&rows_of(&cases, "plugin", &format!("{RECORDED_EPOCH}:track:{}", at + 1)), &track.name);
            track.smart = mixer::recorded_smart(&track.channel);
        }
        let markers: Vec<Marker> = rows("marker")
            .iter()
            .map(|row| Marker {
                own_name: own_marker_name(row["name"].as_str().unwrap_or_default()),
                position_beats: beats_of(&row["position"]),
                length: row["length"].as_f64().unwrap_or(0.0),
            })
            .collect();
        let tempo_row = rows("tempo").into_iter().next().unwrap_or(Value::Null);
        let tempo = tempo_row["bpm"].as_f64().unwrap_or(120.0);
        let signature = tempo_row["signatures"][0]["time"].as_str().and_then(signature_of).unwrap_or((4, 4));
        // The regions as read through the project, each with the notes recorded for it, on the track its ref names.
        let regions: Vec<Region> = rows("region")
            .iter()
            .enumerate()
            .map(|(n, row)| {
                let track = row["trackRef"].as_str().and_then(|r| r.rsplit(':').next()).and_then(|p| p.parse::<u64>().ok()).unwrap_or(1);
                let notes = rows_of(&cases, "note", &format!("{RECORDED_EPOCH}:region:{}", n + 1));
                Region::recorded(n as u64 + 1, row, track, &notes)
            })
            .collect();
        let focus = find(&cases, "focus", &json!({})).and_then(|case| case.response.as_ref().ok()).cloned().unwrap_or(Value::Null);
        let middle_c = rows("project").first().and_then(|row| row["middleC"].as_i64()).unwrap_or(3);
        let (next_track, next_region) = (tracks.len() as u64 + 2, regions.len() as u64 + 1);
        let mut state = State {
            epoch: RECORDED_EPOCH,
            project: 0,
            cases,
            asked: vec![],
            tracks,
            places: HashMap::new(),
            next_track,
            deleted: HashMap::new(),
            markers,
            tempo,
            regions,
            region_places: HashMap::new(),
            next_region,
            deleted_regions: HashMap::new(),
            tempo_changes: vec![],
            signature,
            signature_changes: vec![],
            cycle: CYCLE,
            summing_stacks: 0,
            folder_stacks: 0,
            stacked: HashMap::new(),
            flattened: HashMap::new(),
            saved_as: None,
            saved_paths: vec![],
            outputs: vec![Output::stereo_out()],
            auxes: vec![],
            removed_plugins: HashMap::new(),
            loaded_patches: HashMap::new(),
            playhead: focus["transport"]["beat"].as_f64().unwrap_or(0.0),
            playing: focus["transport"]["playing"] == true,
            metronome: true,
            middle_c,
            undo: vec![],
            open_step: None,
            steps: 0,
            busy_once: false,
            uncertain_once: false,
            uncertain_restore_once: false,
        };
        state.keep_places();
        state
    }

    fn epoch_text(&self) -> String {
        self.epoch.to_string()
    }

    /// The epoch moves (a track added, deleted or renamed; a region added, removed or moved): the places the tracks
    /// and regions have in the new epoch are kept under it, for a restore's ref (a layout holds for a whole epoch,
    /// since every change to it moves the epoch; the changes bump it once they have made the new layout).
    fn bump_epoch(&mut self) {
        self.epoch += 1;
        self.keep_places();
    }

    /// The places the tracks and regions have in the epoch now.
    fn keep_places(&mut self) {
        self.places.insert(self.epoch, self.tracks.iter().map(|track| track.id).collect());
        self.region_places.insert(self.epoch, self.regions.iter().map(|region| region.id).collect());
    }

    /// The regions a track holds now.
    fn regions_of(&self, track: &Track) -> usize {
        self.regions.iter().filter(|region| region.track == track.id).count()
    }

    /// The place (from 0) of the track with `id`, if it is in the project.
    fn track_index(&self, id: u64) -> Option<usize> {
        self.tracks.iter().position(|track| track.id == id)
    }

    /// The regions in the Tracks area's order: by their track's place, then by their start.
    fn sort_regions(&mut self) {
        let order: HashMap<u64, usize> = self.tracks.iter().enumerate().map(|(at, track)| (track.id, at)).collect();
        self.regions.sort_by(|a, b| {
            let (a_track, b_track) =
                (order.get(&a.track).copied().unwrap_or(usize::MAX), order.get(&b.track).copied().unwrap_or(usize::MAX));
            a_track.cmp(&b_track).then(a.start.total_cmp(&b.start)).then(a.id.cmp(&b.id))
        });
    }

    /// The region a ref names, by its place now (from 0): a ref of the epoch now, or (a restore's) one of an earlier
    /// epoch, carried to where that region is now.
    fn region_at(&self, reference: Option<&str>, carry: bool) -> Result<usize, Fault> {
        let text = reference.ok_or_else(|| bad("ref must be a region ref"))?;
        let mut parts = text.splitn(3, ':');
        let (epoch, kind, place) = (parts.next(), parts.next(), parts.next().and_then(|n| n.parse::<usize>().ok()));
        let (Some(epoch), Some("region"), Some(place)) = (epoch.and_then(|e| e.parse::<u32>().ok()), kind, place) else {
            return Err(bad(format!("ref must be a region ref ({}:region:1), not {text}", self.epoch)));
        };
        if epoch == self.epoch {
            return (place >= 1 && place <= self.regions.len())
                .then_some(place - 1)
                .ok_or_else(|| failed(format!("{text} isn't in the project (deleted, or another project)")));
        }
        if !carry || epoch > self.epoch {
            return Err(("stale".into(), format!("that ref is from an earlier epoch ({epoch}; now {}): read again", self.epoch)));
        }
        let id = self
            .region_places
            .get(&epoch)
            .and_then(|places| places.get(place.wrapping_sub(1)))
            .ok_or_else(|| gone(format!("the region {text} named isn't in the project any more (or that epoch is too far back)")))?;
        self.regions
            .iter()
            .position(|region| region.id == *id)
            .ok_or_else(|| gone(format!("the region {text} named isn't in the project any more (or that epoch is too far back)")))
    }

    fn region_ref(&self, place: usize) -> String {
        format!("{}:region:{place}", self.epoch)
    }

    /// The region rows as a read shows them (in the recorded epoch, retagged later).
    fn region_rows(&self) -> Vec<Value> {
        self.regions
            .iter()
            .enumerate()
            .map(|(at, region)| region.row(at + 1, self.track_index(region.track).map_or(0, |t| t + 1)))
            .collect()
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
        self.undo.last().map_or(CANT_UNDO.to_string(), |step| step.title.clone())
    }

    /// A write made: the apply's `undo`. Logic's history takes a step for `step`, named by the Edit menu's title and
    /// by the host's `desc` (none for a header's control or the tempo); a write Logic doesn't answer after is
    /// `uncertain`, its step (if any) unnamed to Kumi.
    fn applied(&mut self, step: Option<&str>, desc: &str) -> Result<Value, Fault> {
        self.applied_steps(step.as_slice(), desc)
    }

    /// (M4) A write that made `steps` of Logic's undo, in order: the apply's `undo`, titled by the last (the Edit menu's
    /// Undo title after the write). The bridge keeps the change under that step, so it alone carries the `desc`.
    fn applied_steps(&mut self, steps: &[&str], desc: &str) -> Result<Value, Fault> {
        let unanswered = std::mem::take(&mut self.uncertain_once);
        for (n, step) in steps.iter().enumerate() {
            let named = !unanswered && n + 1 == steps.len();
            self.undo.push(UndoStep { title: format!("Undo {step}"), desc: named.then(|| desc.to_string()), key: None });
        }
        if unanswered {
            return Err(uncertain(UNANSWERED));
        }
        Ok(match steps.last() {
            Some(step) => json!({"desc": desc, "entry": true, "title": format!("Undo {step}")}),
            None => json!({"desc": desc, "entry": false}),
        })
    }

    /// Whether Logic doesn't answer after the restore being made now (either switch).
    fn restore_unanswered(&mut self) -> bool {
        let once = std::mem::take(&mut self.uncertain_once);
        std::mem::take(&mut self.uncertain_restore_once) || once
    }

    /// A restore made by value: the restore's `undo`. Logic's history takes a step for `step` (a rename, a track
    /// deleted, a marker made, moved or deleted again), which the bridge doesn't name.
    fn restored(&mut self, step: Option<&str>, desc: &str) -> Result<Value, Fault> {
        self.restored_steps(step.as_slice(), desc)
    }

    /// (M4) A restore by value that made `steps` of Logic's undo (a region's edges set back): none named.
    fn restored_steps(&mut self, steps: &[&str], desc: &str) -> Result<Value, Fault> {
        let unanswered = self.restore_unanswered();
        for step in steps {
            self.undo.push(UndoStep { title: format!("Undo {step}"), desc: None, key: None });
        }
        if unanswered {
            return Err(uncertain(UNANSWERED));
        }
        Ok(json!({"desc": desc, "entry": false}))
    }

    /// Logic's own undo of Kumi's step, through the Edit menu: only while its Undo title still names `step` and the top
    /// step is the change's (`ours` says so: by the change's kind and target, as the bridge's watch keys a step, or by
    /// the `desc` the apply sent). The step taken off: its title is what was pressed.
    fn undo_in_logic(&mut self, step: &str, ours: impl Fn(&UndoStep) -> bool) -> Result<UndoStep, Fault> {
        let title = self.undo_title();
        let top = self.undo.last().is_some_and(|top| top.desc.is_some() && ours(top));
        if title != format!("Undo {step}") || !top {
            return Err(failed(format!(
                "Logic Pro's undo has other steps on top of this one (its Edit menu says {title:?}), so Kumi can't put it back; Cmd-Z in Logic Pro can, step by step"
            )));
        }
        Ok(self.undo.pop().expect("the top step"))
    }

    /// (M4) The change a restore names, as the bridge's undo watch keys Logic's steps: its kind and its target's ref.
    fn key_of(params: &Value) -> String {
        format!("{} {}", params["kind"].as_str().unwrap_or_default(), params["target"]["ref"].as_str().unwrap_or_default())
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
        let steps = self.undo.len();
        let answer = self.change_of(&kind, method, params);
        // (M4) The bridge's watch keeps an apply's steps under the change's kind and its target's ref (the apply's own
        // `target` when it answers one, the input's ref else): a restore through Logic's own undo checks the top step
        // is the change's by them.
        if let (Ok(applied), "change_apply") = (&answer, method) {
            let target = applied["target"]["ref"].as_str().or_else(|| params["input"]["ref"].as_str()).unwrap_or_default();
            for step in self.undo.iter_mut().skip(steps) {
                step.key = Some(format!("{kind} {target}"));
            }
        }
        answer
    }

    /// A change of `kind` previewed, applied or restored by the function that makes it.
    fn change_of(&mut self, kind: &str, method: &str, params: &Value) -> Answer {
        match kind {
            "track-add" => self.track_add(method, params),
            "track-delete" => self.track_delete(method, params),
            "track-stack" => self.track_stack(method, params),
            "track-stack-flatten" => self.track_stack_flatten(method, params),
            "marker-add" => self.marker_add(method, params),
            "marker-move" => self.marker_move(method, params),
            "marker-delete" => self.marker_delete(method, params),
            "tempo-set" => self.tempo_set(method, params),
            "tempo-marker-set" => self.tempo_marker_set(method, params),
            "tempo-marker-delete" => self.tempo_marker_delete(method, params),
            "signature-set" => self.signature_set(method, params),
            "loop-range" => self.loop_range(method, params),
            "region-move" => self.region_move(method, params),
            "region-resize" => self.region_resize(method, params),
            "region-split" => self.region_split(method, params),
            "region-loop" => self.region_loop(method, params),
            "region-rename" => self.region_rename(method, params),
            "region-delete" => self.region_delete(method, params),
            "midi-import" => self.midi_import(method, params),
            "key-command" => self.key_command(method, params),
            // (M6) the metronome, and a sample into a Sampler on a new track
            "metronome-set" => self.metronome_set(method, params),
            "sample-load" => self.sample_load(method, params),
            // (M4b) plug-ins, a patch, sends, Smart Controls, automation
            "fx-add" => self.fx_add(method, params),
            "fx-delete" => self.fx_delete(method, params),
            "fx-bypass" => self.fx_bypass(method, params),
            "fx-param" => self.fx_param(method, params),
            "fx-params" => self.fx_params(method, params),
            "fx-preset" => self.fx_preset(method, params),
            "patch-load" => self.patch_load(method, params),
            "send-add" => self.send_add(method, params),
            "send-remove" => self.send_remove(method, params),
            "send-volume" => self.send_volume(method, params),
            "smart-control-set" => self.smart_control_set(method, params),
            "automation-add" | "automation-change" | "automation-delete" => self.automation(kind, method, params),
            _ => match TRACK_PROPS.iter().find(|prop| prop.kind == kind) {
                Some(prop) => self.track_prop(prop, method, params),
                None if NOT_SIMULATED.contains(&kind) => Err(bad(format!(
                    "the simulated Logic Pro makes only {SIMULATED} (not {kind:?}); the bridge makes {} too",
                    NOT_SIMULATED.join(", ")
                ))),
                None => Err(bad(format!("Logic Pro has no change kind {kind:?}"))),
            },
        }
    }

    // ----- (M4) saving, and the escape hatch -----

    /// `project_save {epoch, path?, version?}`: the package written where it is (Cmd-S), or as `path` (File > Save
    /// As…, a `.logicx` that isn't there yet; the project is that file from then on). No change: nothing in the
    /// project moves, and Logic's history takes no step. `version` is REAPER's: Logic keeps no versions.
    fn project_save(&mut self, params: &Value) -> Answer {
        let Some(sent) = params.get("epoch").and_then(Value::as_str) else {
            return Err(bad("epoch: the epoch the refs were read in"));
        };
        if sent != self.epoch_text() {
            return Err((
                "stale".into(),
                format!("those refs are from an earlier epoch ({sent}; now {}): the tracks or regions changed, read again", self.epoch),
            ));
        }
        if params.get("version").is_some_and(|version| !version.is_null() && *version != false) {
            return Err(bad(
                "Logic Pro keeps no versions: give path (a .logicx that isn't there yet) to save a copy, or nothing to save in place",
            ));
        }
        let now = self.project_path();
        match params.get("path").and_then(Value::as_str).filter(|path| !path.is_empty()) {
            None => Ok(json!({"epoch": self.epoch_text(), "saved": now, "dirty": false})),
            Some(path) => {
                Self::save_path(path)?;
                if path == now {
                    return Err(bad("that is the project's own file: save in place (no path), or give another path"));
                }
                if self.saved_paths.iter().any(|saved| saved == path) {
                    return Err(bad(format!("{path} is there already: Kumi never saves over a file")));
                }
                self.saved_as = Some(path.to_string());
                self.saved_paths.push(path.to_string());
                Ok(json!({"epoch": self.epoch_text(), "saved": path, "dirty": false}))
            }
        }
    }

    /// A save as's path as the bridge takes it (`save.rs`): a whole path to a `.logicx` package whose name Logic Pro can
    /// save. (By its text, as macOS reads a path, whatever the system the simulator runs on.)
    fn save_path(path: &str) -> Result<(), Fault> {
        if !path.starts_with('/') || path.split('/').any(|part| part == "..") {
            return Err(bad("path must be a whole path, with no \"..\" in it"));
        }
        let file = path.rsplit('/').next().unwrap_or_default();
        let (stem, extension) = match file.rfind('.') {
            Some(dot) if dot > 0 => (&file[..dot], &file[dot + 1..]),
            _ => (file, ""),
        };
        if !extension.eq_ignore_ascii_case("logicx") {
            return Err(bad("path must name a .logicx package"));
        }
        if stem.is_empty() || stem.contains(':') || stem.chars().any(char::is_control) || stem.chars().count() > 200 {
            return Err(bad("path must end in a file name Logic Pro can save: no colon or slash in it"));
        }
        Ok(())
    }

    /// The package the project is saved as now: the recorded one's, or the one Kumi saved it as.
    fn project_path(&self) -> String {
        self.saved_as.clone().unwrap_or_else(|| {
            another_project(&serde_json::to_value(project()).unwrap_or(Value::Null), self.project)["path"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
    }

    /// `key-command {command}`: one of Logic's key commands, by its English name or its id (`key_command_of`), its
    /// chord posted with Logic in front. Logic's history takes a step for an edit (Create Marker; a split, when the
    /// playhead is inside a selected region) and none for a selection; the apply compares the Edit menu's Undo title
    /// before and after, as the bridge does: moved, the change is a row named by Logic's step (`applied.step`), else no
    /// row (`applied.changed: false`, `display.unchanged`, `undo.entry: false`). A restore is Logic's own undo's
    /// business (`daw-only`).
    fn key_command(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return Err((
                "daw-only".into(),
                "Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro)".into(),
            ));
        }
        let command = key_command_of(&params["input"])?;
        let chord = command.chord.map(|chord| format!(" ({chord})")).unwrap_or_default();
        let title = format!("Ran key command {}{chord}", command.label());
        if method == "change_preview" {
            let mut target = json!({"kind": "action", "command": command.id});
            if let Some(name) = command.name {
                target["name"] = name.into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": target,
                "prior": {},
                "fence": {"command": command.id, "name": command.name},
                "display": {"title": title, "note": format!("{KEY_NOTE} It acts on what the producer has selected now.")},
            }));
        }
        if params["fence"]["command"] != command.id {
            return Err(changed("the command isn't the one previewed"));
        }
        let desc = Self::desc(params)?;
        // What the command does to the project: a marker at the playhead; every selected region the playhead is inside
        // split there; the regions selected, or none.
        let edited = match command.id {
            513 => {
                self.markers.push(Marker::made(on_the_beat(self.playhead)));
                self.sort_markers();
                true
            }
            574 => {
                let playhead = self.playhead;
                let split: Vec<u64> = self
                    .regions
                    .iter()
                    .filter(|region| region.selected && region.start < playhead && playhead < region.end)
                    .map(|region| region.id)
                    .collect();
                for id in &split {
                    if let Some(at) = self.region_index(*id) {
                        self.split_region(at, playhead);
                    }
                }
                if !split.is_empty() {
                    self.bump_epoch();
                }
                !split.is_empty()
            }
            766 | 347 => {
                for region in &mut self.regions {
                    region.selected = command.id == 766;
                }
                false
            }
            _ => false,
        };
        let mut applied = json!({"command": command.id, "name": command.name, "key": command.chord});
        match command.step.filter(|_| edited) {
            Some(step) => {
                let undo = self.applied(Some(step), &desc)?;
                applied["step"] = step.into();
                Ok(json!({"epoch": self.epoch_text(), "applied": applied, "display": {"title": title, "note": KEY_NOTE}, "undo": undo}))
            }
            None => {
                applied["changed"] = false.into();
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": applied,
                    "display": {"title": title, "note": KEY_NO_STEP, "unchanged": true},
                    "undo": {"desc": desc, "entry": false},
                }))
            }
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
                // Logic names a new track after its channel strip, counted by the strips of its kind (the M3 scenes: a
                // new audio track beside Vocal is "Audio 2", a new instrument track beside Keys "Inst 2"); a name given
                // goes through the Inspector once the track is made, so "Renaming" is on top of "Create Track".
                let renamed = name.is_some();
                let named = name.unwrap_or_else(|| {
                    let same = self.tracks.iter().filter(|track| !track.is_stack() && track.kind == kind).count();
                    format!(
                        "{} {}",
                        match kind.as_str() {
                            "audio" => "Audio",
                            "instrument" => "Inst",
                            _ => "MIDI",
                        },
                        same + 1
                    )
                });
                let made = Track::made(self.next_track, &named, &kind);
                self.next_track += 1;
                self.tracks.insert(after, made);
                self.bump_epoch();
                let place = after + 1;
                let steps: &[&str] = if renamed { &["Create Track", "Renaming"] } else { &["Create Track"] };
                let undo = self.applied_steps(steps, &desc)?;
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
    /// kept in `material`. Its restore is Logic's own undo (Cmd-Z), sent only while the Edit menu's Undo title is this
    /// very step: a track can't be made again by value with what it held.
    fn track_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let material = &params["material"];
            let desc = Self::desc(params)?;
            let name = material["name"].as_str().unwrap_or_default().to_string();
            let place = material["index"].as_u64().unwrap_or(0) as usize;
            if self.tracks.get(place.wrapping_sub(1)).is_some_and(|track| track.name == name) {
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"ref": format!("{}:track:{place}", self.epoch)},
                    "display": {"title": format!("{} is back already", quoted(&name))},
                    "undo": {"desc": desc, "entry": false},
                }));
            }
            let step = material["step"].as_str().unwrap_or_default().to_string();
            let pressed = self.undo_in_logic("Delete Tracks", |top| top.desc.as_deref() == Some(step.as_str()))?.title;
            if self.restore_unanswered() {
                return Err(uncertain(format!("Logic Pro's {pressed:?} was pressed, but {} isn't back at place {place}", quoted(&name))));
            }
            let track = self.deleted.remove(&step).unwrap_or_else(|| {
                let mut track = Track::made(0, &name, material["kind"].as_str().unwrap_or("audio"));
                track.volume_db = material["volumeDb"].as_f64().unwrap_or(0.0);
                track.pan_steps = (material["pan"].as_f64().unwrap_or(0.0) * PAN_CENTRE as f64).round() as i64;
                track.mute = material["mute"] == true;
                track.solo = material["solo"] == true;
                track.armed = material["arm"] == true;
                track
            });
            let place = place.clamp(1, self.tracks.len() + 1);
            self.tracks.insert(place - 1, track);
            self.bump_epoch();
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"ref": format!("{}:track:{place}", self.epoch), "was": params["target"]["ref"], "pressed": pressed},
                "display": {"title": format!("{} put back", quoted(&name)), "note": "Put back with Logic Pro's own undo."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
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
                let gone = self.tracks.remove(at);
                self.deleted.insert(desc.clone(), gone);
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
        Self::position_of_input(input, "markers")
    }

    /// `positionBeats` from an input, refusing REAPER's seconds in the bridge's words for `what` it places.
    fn position_of_input(input: &Value, what: &str) -> Result<f64, Fault> {
        if input.get("position").is_some_and(|v| !v.is_null()) && input.get("positionBeats").is_none_or(Value::is_null) {
            return Err(bad(format!("Logic Pro's bridge places {what} by positionBeats (beats from the project's start), not seconds")));
        }
        input
            .get("positionBeats")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))
    }

    /// `marker-add {positionBeats}`: a marker at the playhead moved there (on the beat), from the Marker List's Create
    /// new Marker; Logic names it by its place in the list. Its restore deletes the row at its place, if the marker
    /// there is still at that position.
    fn marker_add(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let created = params["created"]["ref"].as_str();
            let place = self.marker_place(created)?;
            let desc = Self::desc(params)?;
            let Some(marker) = self.markers.get(place - 1).cloned() else {
                return Err(gone("the marker Kumi added isn't in the Marker List any more"));
            };
            if params["discard"] != true && position_of(marker.position_beats) != params["applied"]["position"] {
                return Err(changed(format!("{} has been moved in Logic Pro since Kumi added it", marker.label(place))));
            }
            self.markers.remove(place - 1);
            let undo = self.restored(Some("Delete"), &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"deleted": created},
                "display": {"title": format!("{} taken away again", marker.label(place))},
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
                let marker = Marker::made(beats);
                self.markers.push(marker.clone());
                self.sort_markers();
                let place = self.marker_at(beats).map_or(0, |at| at + 1);
                let undo = self.applied(Some("Create Marker"), &desc)?;
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": marker.state(place),
                    "created": {"ref": self.marker_ref(place), "kind": "marker"},
                    "display": {"title": format!("Marker {} at {}", quoted(&marker.name_at(place)), bar_text(beats))},
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
            let label = self.markers[at].label(at + 1);
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
        let title = format!("Moved {} to {}", marker.label(place), bar_text(to));
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
                    return Err(changed(format!("{} moved since the preview", marker.label(place))));
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

    /// `marker-delete {ref}`: the row's Delete action. Its restore is Logic's own undo while that step is on top, else
    /// the marker made again by value at its place, numbered by Logic.
    fn marker_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let material = &params["material"];
            let (name, beats, length) = (
                material["name"].as_str().unwrap_or_default().to_string(),
                beats_of(&material["position"]),
                material["length"].as_f64().unwrap_or(MARKER_LENGTH),
            );
            let desc = Self::desc(params)?;
            // Logic's own undo brings it back whole, name and all, while the Edit menu still names Kumi's deletion first.
            let ours = |top: &UndoStep| top.desc.as_deref().is_some_and(|desc| desc.starts_with("Kumi: Deleted marker"));
            if let Ok(UndoStep { title: pressed, .. }) = self.undo_in_logic("Delete", ours) {
                if self.restore_unanswered() {
                    return Err(uncertain(format!("Logic Pro's {pressed:?} was pressed, but no marker is back at {}", bar_text(beats))));
                }
                self.markers.push(Marker { own_name: own_marker_name(&name), position_beats: beats, length });
                self.sort_markers();
                let place = self.marker_at(beats).map_or(0, |at| at + 1);
                let shown = self.markers.get(place.wrapping_sub(1)).map_or(name.clone(), |marker| marker.name_at(place));
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"ref": self.marker_ref(place), "was": params["target"]["ref"], "pressed": pressed},
                    "display": {"title": format!("Marker {} put back", quoted(&shown)), "note": "Put back with Logic Pro's own undo."},
                    "undo": {"desc": desc, "entry": false, "how": "undo"},
                }));
            }
            // Else by value: a marker made again at its place; Logic names it by its place in the list.
            if let Some(at) = self.marker_at(beats) {
                let there = self.markers[at].name_at(at + 1);
                if params["discard"] == true || there == name {
                    return Ok(json!({
                        "epoch": self.epoch_text(),
                        "restored": {"ref": self.marker_ref(at + 1)},
                        "display": {"title": format!("Marker {} is back already", quoted(&name))},
                        "undo": {"desc": desc, "entry": false},
                    }));
                }
                return Err(changed(format!("another marker ({}) is at {} now", quoted(&there), bar_text(beats))));
            }
            self.markers.push(Marker::made(beats));
            self.sort_markers();
            let place = self.marker_at(beats).map_or(0, |at| at + 1);
            let made = self.markers[place - 1].name_at(place);
            let undo = self.restored(Some("Create Marker"), &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"ref": self.marker_ref(place), "name": made},
                "display": {"title": format!("Marker {} put back as {}", quoted(&name), quoted(&made)), "note": "Logic Pro numbers a marker made again itself."},
                "undo": undo,
            }));
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
        let title = format!("Deleted {}", marker.label(place));
        match method {
            "change_preview" => Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": input["ref"], "kind": "marker"},
                "prior": marker.state(place),
                "fence": marker.state(place),
                "display": {"title": title},
            })),
            _ => {
                if params["fence"] != marker.state(place) {
                    return Err(changed(format!("{} changed since the preview", marker.label(place))));
                }
                let desc = Self::desc(params)?;
                self.markers.remove(place - 1);
                let undo = self.applied(Some("Delete"), &desc)?;
                Ok(json!({
                    "epoch": self.epoch_text(),
                    "applied": {"deleted": true},
                    "material": marker.state(place),
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

    // ----- (M4) regions -----

    /// A region's state as the bridge keeps it (`regions.rs`): its edges, its name, its track's place (from 1), and
    /// `looped` as the Tracks area reads it (never true: Accessibility shows no loop there).
    fn region_state(&self, region: &Region) -> Value {
        json!({
            "startBeats": region.start,
            "endBeats": region.end,
            "name": region.name,
            "track": self.track_index(region.track).map_or(0, |at| at + 1),
            "looped": false,
        })
    }

    /// The region whose track, name and edges are `state`'s now (where it is after a move or a resize), by its place
    /// (from 0), as the bridge finds a region again.
    fn region_by_state(&self, state: &Value) -> Option<usize> {
        self.regions.iter().position(|region| {
            let now = self.region_state(region);
            now["track"] == state["track"]
                && now["name"] == state["name"]
                && close_beats(region.start, number(state, "startBeats"))
                && close_beats(region.end, number(state, "endBeats"))
        })
    }

    /// A region as a change's target: its ref in the epoch now.
    fn region_target(&self, at: usize) -> Value {
        json!({"ref": self.region_ref(at + 1), "kind": "region"})
    }

    /// The region a preview or an apply names (a ref of the epoch now; one gone since the preview is `changed`).
    fn region_for(&self, method: &str, input: &Value) -> Result<usize, Fault> {
        self.region_at(input["ref"].as_str(), false).map_err(|fault| match (method, fault.0.as_str()) {
            ("change_apply", "failed") => changed(format!("{}: it went since the preview", fault.1)),
            _ => fault,
        })
    }

    /// The region a restore names: its ref from the change's epoch, carried to where the region is now.
    fn region_for_restore(&self, params: &Value) -> Result<usize, Fault> {
        self.region_at(params["target"]["ref"].as_str(), true).map_err(|fault| if fault.0 == "failed" { gone(fault.1) } else { fault })
    }

    /// The place (from 0) of the region with `id`.
    fn region_index(&self, id: u64) -> Option<usize> {
        self.regions.iter().position(|region| region.id == id)
    }

    /// The region with `id` alone selected, as the click the bridge makes to act on a region leaves it.
    fn select_region(&mut self, id: u64) {
        for region in &mut self.regions {
            region.selected = region.id == id;
        }
    }

    /// The region at `at` at new edges, the Tracks area's order kept; its place now (from 0).
    fn place_region(&mut self, at: usize, start: f64, end: f64) -> usize {
        let id = self.regions[at].id;
        let region = &mut self.regions[at];
        region.start = start;
        region.end = end;
        self.sort_regions();
        self.region_index(id).unwrap_or(at)
    }

    /// `region-move {ref, positionBeats}`: the region's row at the Event List's region level (Leave Folder), its
    /// Position stepped a segment at a time: in time only (the Event List moves a region along its own track, so a
    /// `toRef` is refused). It keeps its length; the epoch moves, and the apply answers the region's ref in it as its
    /// `target`. Its restore moves it back, by value.
    fn region_move(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            if let Some(back) = self.region_by_state(&params["prior"]) {
                let title = format!("{} is where it was", self.regions[back].label());
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": self.region_state(&self.regions[back]), "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
                );
            }
            let at = match self.region_by_state(&params["applied"]) {
                Some(at) => at,
                None if params["discard"] == true => self.region_for_restore(params)?,
                None => return Err(changed("the region has been moved in Logic Pro since Kumi moved it")),
            };
            let to = params["prior"]["startBeats"].as_f64().ok_or_else(|| bad("prior.startBeats"))?;
            let length = self.regions[at].end - self.regions[at].start;
            let at = self.place_region(at, to, to + length);
            self.bump_epoch();
            let undo = self.restored(Some("Value Change"), &desc)?;
            let region = &self.regions[at];
            let title = format!("{} moved back to {}", region.label(), bar_text(region.start));
            return Ok(
                json!({"epoch": self.epoch_text(), "restored": self.region_state(region), "display": {"title": title}, "undo": undo}),
            );
        }
        let input = &params["input"];
        if input.get("toRef").is_some_and(|to| !to.is_null()) {
            return Err(bad("Logic Pro's Event List moves a region in time, not to another track: leave toRef out"));
        }
        let at = self.region_for(method, input)?;
        let to = Self::position_of_input(input, "regions")?;
        let region = self.regions[at].clone();
        let now = self.region_state(&region);
        let length = region.end - region.start;
        let after = json!({"startBeats": to, "endBeats": to + length, "name": now["name"], "track": now["track"], "looped": now["looped"]});
        let title = format!("Moved {} to {}", region.label(), bar_text(to));
        let unchanged = close_beats(region.start, to);
        if method == "change_preview" {
            let mut display = json!({"title": title, "before": now, "after": after});
            if unchanged {
                display["unchanged"] = true.into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.region_target(at), "prior": now, "fence": now, "display": display}),
            );
        }
        if params["fence"] != now {
            return Err(changed(format!("{} changed since the preview", region.label())));
        }
        let desc = Self::desc(params)?;
        if unchanged {
            return Ok(
                json!({"epoch": self.epoch_text(), "applied": now, "target": self.region_target(at), "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
            );
        }
        let at = self.place_region(at, to, to + length);
        self.bump_epoch();
        let undo = self.applied(Some("Value Change"), &desc)?;
        let after = self.region_state(&self.regions[at]);
        Ok(
            json!({"epoch": self.epoch_text(), "applied": after, "target": self.region_target(at), "display": {"title": title, "before": now, "after": after}, "undo": undo}),
        )
    }

    /// The edges a resize asks for (the one not given stays), in the bridge's words for what is off.
    fn region_edges(input: &Value, start: f64, end: f64) -> Result<(Option<f64>, Option<f64>), Fault> {
        let given = |key: &str| input.get(key).is_some_and(|value| !value.is_null());
        if (given("start") || given("end")) && !given("startBeats") && !given("endBeats") {
            return Err(bad(
                "Logic Pro's bridge moves a region's edges by startBeats and endBeats (beats from the project's start), not seconds",
            ));
        }
        let edge = |key: &str| -> Result<Option<f64>, Fault> {
            match input.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => value
                    .as_f64()
                    .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                    .map(Some)
                    .ok_or_else(|| bad(format!("{key} must be a number from 0: beats from the project's start"))),
            }
        };
        let (new_start, new_end) = (edge("startBeats")?, edge("endBeats")?);
        if new_start.is_none() && new_end.is_none() {
            return Err(bad("pass the new start (startBeats), the new end (endBeats), or both"));
        }
        if new_end.unwrap_or(end) <= new_start.unwrap_or(start) + 1.0 / TICKS_PER_BEAT {
            return Err(bad("the region must end after it starts"));
        }
        Ok((new_start, new_end))
    }

    /// A region's edges set with Logic's own Set Region/Event/Marquee Start and End to Playhead Position (the start
    /// first), the playhead placed by the LCD's bar and beat: whole beats only, both checked before either is set. Each
    /// edge that moves is a step of Logic's undo, named as its Edit menu names it; the notes stay where they are in
    /// time; the region is left selected (the bridge clicks it), and the epoch moves. Its place now (from 0), and the
    /// steps.
    fn set_edges(&mut self, at: usize, start: Option<f64>, end: Option<f64>) -> Result<(usize, Vec<&'static str>), Fault> {
        if [start, end].into_iter().flatten().any(|edge| !on_a_beat(edge)) {
            return Err(bad("Logic Pro's playhead display places an edge on a beat: give whole beats"));
        }
        let id = self.regions[at].id;
        let mut steps = vec![];
        if let Some(start) = start {
            let region = &mut self.regions[at];
            let moved = region.start - start;
            for note in &mut region.notes {
                note["beat"] = (note["beat"].as_f64().unwrap_or(0.0) + moved).into();
            }
            region.start = start;
            steps.push("Set Start to Playhead Position");
        }
        if let Some(end) = end {
            self.regions[at].end = end;
            steps.push("Set End to Playhead Position");
        }
        self.select_region(id);
        self.sort_regions();
        self.bump_epoch();
        Ok((self.region_index(id).unwrap_or(at), steps))
    }

    /// `region-resize {ref, startBeats?, endBeats?}`: the region's edges at the playhead (`set_edges`); the epoch moves,
    /// and the apply answers the region's ref in it. Its restore sets the edges back, by value.
    fn region_resize(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            if let Some(back) = self.region_by_state(&params["prior"]) {
                let title = format!("{} is as it was", self.regions[back].label());
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": self.region_state(&self.regions[back]), "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
                );
            }
            let at = match self.region_by_state(&params["applied"]) {
                Some(at) => at,
                None if params["discard"] == true => self.region_for_restore(params)?,
                None => return Err(changed("the region has been changed in Logic Pro since Kumi trimmed it")),
            };
            let start = params["prior"]["startBeats"].as_f64().ok_or_else(|| bad("prior.startBeats"))?;
            let end = params["prior"]["endBeats"].as_f64().ok_or_else(|| bad("prior.endBeats"))?;
            let region = &self.regions[at];
            let (start, end) = ((!close_beats(start, region.start)).then_some(start), (!close_beats(end, region.end)).then_some(end));
            let (at, steps) = self.set_edges(at, start, end)?;
            let undo = self.restored_steps(&steps, &desc)?;
            let region = &self.regions[at];
            let title = format!("{} back to its length", region.label());
            return Ok(
                json!({"epoch": self.epoch_text(), "restored": self.region_state(region), "display": {"title": title}, "undo": undo}),
            );
        }
        let input = &params["input"];
        let at = self.region_for(method, input)?;
        let region = self.regions[at].clone();
        let now = self.region_state(&region);
        let (start, end) = Self::region_edges(input, region.start, region.end)?;
        let (new_start, new_end) = (start.unwrap_or(region.start), end.unwrap_or(region.end));
        let wanted =
            json!({"startBeats": new_start, "endBeats": new_end, "name": now["name"], "track": now["track"], "looped": now["looped"]});
        let title = format!("Trimmed {} to {}–{}", region.label(), bar_text(new_start), bar_text(new_end));
        if method == "change_preview" {
            let mut display = json!({"title": title, "before": now, "after": wanted});
            if close_beats(new_start, region.start) && close_beats(new_end, region.end) {
                display["unchanged"] = true.into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.region_target(at), "prior": now, "fence": now, "display": display}),
            );
        }
        if params["fence"] != now {
            return Err(changed(format!("{} changed since the preview", region.label())));
        }
        let desc = Self::desc(params)?;
        if wanted == now {
            return Ok(
                json!({"epoch": self.epoch_text(), "applied": now, "target": self.region_target(at), "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
            );
        }
        let start = start.filter(|start| !close_beats(*start, region.start));
        let end = end.filter(|end| !close_beats(*end, region.end));
        let (at, steps) = self.set_edges(at, start, end)?;
        let undo = self.applied_steps(&steps, &desc)?;
        let after = self.region_state(&self.regions[at]);
        Ok(
            json!({"epoch": self.epoch_text(), "applied": after, "target": self.region_target(at), "display": {"title": title, "before": now, "after": after}, "undo": undo}),
        )
    }

    /// The region at `at` split at `beats` (inside it): the left part keeps the region's id, the right part is a new
    /// region after it with the notes from the split on, and Logic leaves it selected. The right part's id.
    fn split_region(&mut self, at: usize, beats: f64) -> u64 {
        let id = self.next_region;
        self.next_region += 1;
        let left = &mut self.regions[at];
        let cut = beats - left.start;
        let (kept, moved): (Vec<Value>, Vec<Value>) = left.notes.drain(..).partition(|note| note["beat"].as_f64().unwrap_or(0.0) < cut);
        left.notes = kept;
        left.selected = false;
        let right = Region {
            id,
            track: left.track,
            name: left.name.clone(),
            kind: left.kind.clone(),
            start: beats,
            end: left.end,
            looping: left.looping,
            selected: true,
            notes: moved
                .into_iter()
                .map(|mut note| {
                    note["beat"] = (note["beat"].as_f64().unwrap_or(0.0) - cut).into();
                    note
                })
                .collect(),
        };
        left.end = beats;
        self.regions.push(right);
        self.sort_regions();
        id
    }

    /// The parts of a split joined again: the right part's notes back in the left, the right part gone, the left's end
    /// back at `end`, the region selected; the epoch moves. The left's place now (from 0).
    fn join_split(&mut self, left_at: usize, right_at: Option<usize>, end: f64) -> usize {
        let left_id = self.regions[left_at].id;
        if let Some(right_at) = right_at {
            let right = self.regions.remove(right_at);
            if let Some(left_at) = self.region_index(left_id) {
                let cut = right.start - self.regions[left_at].start;
                for mut note in right.notes {
                    note["beat"] = (note["beat"].as_f64().unwrap_or(0.0) + cut).into();
                    self.regions[left_at].notes.push(note);
                }
            }
        }
        if let Some(left) = self.regions.iter_mut().find(|region| region.id == left_id) {
            left.end = end;
        }
        self.select_region(left_id);
        self.sort_regions();
        self.bump_epoch();
        self.region_index(left_id).unwrap_or(0)
    }

    /// `region-split {ref, positionBeats}`: the region clicked, the playhead there (the LCD's bar and beat: whole beats
    /// only), Split Regions/Events at Playhead Position: the left part keeps its place (its ref in the new epoch is the
    /// apply's `target`), the right part is new (its ref in `created`), and the epoch moves. Its restore is Logic's own
    /// undo while that step is on top: a split region isn't made whole by value.
    fn region_split(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.region_split_restore(params);
        }
        let input = &params["input"];
        let at = self.region_for(method, input)?;
        let beats = Self::position_of_input(input, "regions")?;
        let region = self.regions[at].clone();
        let now = self.region_state(&region);
        if beats <= region.start || beats >= region.end {
            return Err(bad(format!("that's not inside the region: it runs from {} to {}", bar_text(region.start), bar_text(region.end))));
        }
        if !on_a_beat(beats) {
            return Err(bad("Logic Pro's playhead display splits on a beat: give whole beats"));
        }
        let fence = json!({"region": now, "regions": self.regions.len()});
        let title = format!("Split {} at {}", region.label(), bar_text(beats));
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": self.region_target(at),
                "prior": now,
                "fence": fence,
                "display": {"title": title, "note": "Logic Pro's own undo puts the region back whole while the split is its latest step."},
            }));
        }
        if params["fence"] != fence {
            return Err(changed(format!("{} or the regions changed since the preview", region.label())));
        }
        let desc = Self::desc(params)?;
        // (the region clicked alone first)
        self.select_region(region.id);
        let right = self.split_region(at, beats);
        self.bump_epoch();
        let undo = self.applied(Some("Split Regions"), &desc)?;
        let (left_at, right_at) = (self.region_index(region.id).unwrap_or(at), self.region_index(right).unwrap_or(at + 1));
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"left": self.region_state(&self.regions[left_at]), "right": self.region_state(&self.regions[right_at])},
            "target": self.region_target(left_at),
            "created": {"ref": self.region_ref(right_at + 1), "kind": "region"},
            "display": {"title": title},
            "undo": undo,
        }))
    }

    fn region_split_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        if self.region_by_state(&params["prior"]).is_some() {
            return Ok(
                json!({"epoch": self.epoch_text(), "restored": {"ref": params["target"]["ref"]}, "display": {"title": "The region is whole again"}, "undo": {"desc": desc, "entry": false}}),
            );
        }
        let (left, right) = (self.region_by_state(&params["applied"]["left"]), self.region_by_state(&params["applied"]["right"]));
        if params["discard"] != true && (left.is_none() || right.is_none()) {
            return Err(changed("one of the split's parts has been changed in Logic Pro since Kumi split it"));
        }
        let key = Self::key_of(params);
        let pressed = self.undo_in_logic("Split Regions", |top| top.key.as_deref() == Some(key.as_str()))?.title;
        let unanswered = || uncertain(format!("Logic Pro's {pressed:?} was pressed, but the region isn't whole again"));
        if self.restore_unanswered() {
            return Err(unanswered());
        }
        // (Logic's own undo takes the split back whatever the parts are now)
        let left = left.or_else(|| self.region_at(params["target"]["ref"].as_str(), true).ok());
        let right = right.or_else(|| self.region_at(params["created"]["ref"].as_str(), true).ok());
        let Some(left) = left else { return Err(unanswered()) };
        let place = self.join_split(left, right.filter(|right| *right != left), number(&params["prior"], "endBeats"));
        let label = self.regions[place].label();
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"ref": self.region_ref(place + 1), "was": params["target"]["ref"], "pressed": pressed},
            "display": {"title": format!("{label} whole again"), "note": "Put back with Logic Pro's own undo."},
            "undo": {"desc": desc, "entry": false, "how": "undo"},
        }))
    }

    /// `region-loop {ref, loop}`: the Region inspector's Loop checkbox, pressed with the region selected through its
    /// item (the Tracks area's rows never show the loop). A step of Logic's undo ("Loop Parameter Change"); its
    /// restore presses it back, by value.
    fn region_loop(&mut self, method: &str, params: &Value) -> Answer {
        let title_of = |region: &Region, on: bool| format!("{} {}", region.label(), if on { "looped" } else { "not looped" });
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let at = self.region_for_restore(params)?;
            let (prior, applied) = (params["prior"]["loop"] == true, params["applied"]["loop"] == true);
            let region = self.regions[at].clone();
            let mut step = None;
            if region.looping != prior {
                if params["discard"] != true && region.looping != applied {
                    return Err(changed(format!("{}'s loop has been changed in Logic Pro since Kumi set it", region.label())));
                }
                self.regions[at].looping = prior;
                self.select_region(region.id);
                step = Some("Loop Parameter Change");
            }
            let undo = self.restored(step, &desc)?;
            let title = format!("{} (put back)", title_of(&region, prior));
            return Ok(json!({"epoch": self.epoch_text(), "restored": {"loop": prior}, "display": {"title": title}, "undo": undo}));
        }
        let input = &params["input"];
        let at = self.region_for(method, input)?;
        let wanted = input.get("loop").and_then(Value::as_bool).ok_or_else(|| bad("loop must be true or false"))?;
        let region = self.regions[at].clone();
        let now = json!({"loop": region.looping});
        let title = title_of(&region, wanted);
        if method == "change_preview" {
            let mut display = json!({"title": title, "before": now, "after": {"loop": wanted}});
            if region.looping == wanted {
                display["unchanged"] = true.into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.region_target(at), "prior": now, "fence": now, "display": display}),
            );
        }
        if params["fence"] != now {
            return Err(changed(format!("{}'s loop changed since the preview", region.label())));
        }
        let desc = Self::desc(params)?;
        let step = (region.looping != wanted).then_some("Loop Parameter Change");
        self.regions[at].looping = wanted;
        self.select_region(region.id);
        let undo = self.applied(step, &desc)?;
        Ok(
            json!({"epoch": self.epoch_text(), "applied": {"loop": wanted}, "display": {"title": title, "before": now, "after": {"loop": wanted}}, "undo": undo}),
        )
    }

    /// A region's name as an input gives it, in the bridge's words for what is off.
    fn region_name_input(input: &Value) -> Result<String, Fault> {
        let name = input.get("name").and_then(Value::as_str).ok_or_else(|| bad("name must be text"))?;
        if name.trim().is_empty() {
            return Err(bad("name must not be empty"));
        }
        if name.chars().count() > 256 || name.chars().any(char::is_control) {
            return Err(bad("name must be one line of at most 256 characters"));
        }
        Ok(name.to_string())
    }

    /// `region-rename {ref, name}`: the Region inspector's name field, confirmed (the region selected through its item):
    /// a step of Logic's undo ("Renaming"); the epoch moves, and the apply answers the region's ref in it. Its restore
    /// renames it back, by value.
    fn region_rename(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let at = self.region_for_restore(params)?;
            let prior = params["prior"]["name"].as_str().unwrap_or_default().to_string();
            let region = self.regions[at].clone();
            let mut step = None;
            if region.name != prior {
                if params["discard"] != true && json!({"name": region.name}) != params["applied"] {
                    return Err(changed(format!("{}'s name has been changed in Logic Pro since Kumi set it", region.label())));
                }
                self.regions[at].name = prior.clone();
                self.select_region(region.id);
                self.bump_epoch();
                step = Some("Renaming");
            }
            let undo = self.restored(step, &desc)?;
            return Ok(
                json!({"epoch": self.epoch_text(), "restored": {"name": prior}, "display": {"title": format!("Region named {} again", quoted(&prior))}, "undo": undo}),
            );
        }
        let input = &params["input"];
        let at = self.region_for(method, input)?;
        let name = Self::region_name_input(input)?;
        let region = self.regions[at].clone();
        let now = json!({"name": region.name});
        let title = format!("Renamed region {} → {}", quoted(&region.name), quoted(&name));
        if method == "change_preview" {
            let mut display = json!({"title": title, "before": now, "after": {"name": name}});
            if region.name == name {
                display["unchanged"] = true.into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.region_target(at), "prior": now, "fence": now, "display": display}),
            );
        }
        if params["fence"] != now {
            return Err(changed(format!("{}'s name changed since the preview", region.label())));
        }
        let desc = Self::desc(params)?;
        let mut step = None;
        if region.name != name {
            self.regions[at].name = name.clone();
            self.select_region(region.id);
            self.bump_epoch();
            step = Some("Renaming");
        }
        let undo = self.applied(step, &desc)?;
        let at = self.region_index(region.id).unwrap_or(at);
        Ok(
            json!({"epoch": self.epoch_text(), "applied": {"name": name}, "target": self.region_target(at), "display": {"title": title, "before": now, "after": {"name": name}}, "undo": undo}),
        )
    }

    /// `region-delete {ref}`: the region clicked, the Delete key; the epoch moves, and what it was is kept in
    /// `material` (with the apply's `desc` as its `step`). Its restore is Logic's own undo while that step is on top
    /// (the region comes back with its notes): Logic can't make it again by value.
    fn region_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let material =
                params.get("material").filter(|material| !material.is_null()).ok_or_else(|| bad("material: what the delete removed"))?;
            if let Some(back) = self.region_by_state(material) {
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"ref": self.region_ref(back + 1)},
                    "display": {"title": format!("{} is back already", self.regions[back].label())},
                    "undo": {"desc": desc, "entry": false},
                }));
            }
            let (key, step) = (Self::key_of(params), material["step"].as_str().unwrap_or_default().to_string());
            let pressed = self
                .undo_in_logic("Delete", |top| top.key.as_deref() == Some(key.as_str()) && top.desc.as_deref() == Some(step.as_str()))?
                .title;
            let unanswered = || uncertain(format!("Logic Pro's {pressed:?} was pressed, but the region isn't back"));
            if self.restore_unanswered() {
                return Err(unanswered());
            }
            let Some(region) = self.deleted_regions.remove(&step) else { return Err(unanswered()) };
            let id = region.id;
            self.regions.push(region);
            self.select_region(id);
            self.sort_regions();
            self.bump_epoch();
            let place = self.region_index(id).unwrap_or(0);
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"ref": self.region_ref(place + 1), "was": params["target"]["ref"], "pressed": pressed},
                "display": {"title": format!("{} put back", self.regions[place].label()), "note": "Put back with Logic Pro's own undo."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
        }
        let input = &params["input"];
        let at = self.region_for(method, input)?;
        let region = self.regions[at].clone();
        let now = self.region_state(&region);
        let fence = json!({"region": now, "regions": self.regions.len()});
        let title = format!("Deleted {}", region.label());
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": self.region_target(at),
                "prior": now,
                "fence": fence,
                "display": {"title": title, "note": "Only Logic Pro's own undo brings a deleted region back, while the deletion is its latest step."},
            }));
        }
        if params["fence"] != fence {
            return Err(changed(format!("{} or the regions changed since the preview", region.label())));
        }
        let desc = Self::desc(params)?;
        let mut material = now;
        material["kind"] = region.kind.clone().into();
        material["step"] = desc.clone().into();
        // (the region clicked alone, then gone: nothing is left selected)
        self.select_region(region.id);
        let mut gone = self.regions.remove(at);
        gone.selected = false;
        self.deleted_regions.insert(desc.clone(), gone);
        self.bump_epoch();
        let undo = self.applied(Some("Delete"), &desc)?;
        Ok(
            json!({"epoch": self.epoch_text(), "applied": {"deleted": true}, "material": material, "display": {"title": title}, "undo": undo}),
        )
    }

    // ----- (M4) the cycle, the Tempo and Signature Lists -----

    /// The cycle as the bridge reads it: the control bar's Cycle button and the LCD's locators.
    fn cycle_state(&self) -> Value {
        let (on, start, end) = self.cycle;
        json!({"on": on, "startBeats": start, "endBeats": end})
    }

    /// `loop-range {startBeats, endBeats}`: the locators through the LCD on its Custom display (they sit on Logic's
    /// ticks) and the Cycle button; 0 and 0 turn the cycle off and leave the locators. No step of Logic's undo; its
    /// restore puts the cycle back with its locators, by value.
    fn loop_range(&mut self, method: &str, params: &Value) -> Answer {
        let same = |a: &Value, b: &Value| {
            a["on"] == b["on"]
                && close_beats(number(a, "startBeats"), number(b, "startBeats"))
                && close_beats(number(a, "endBeats"), number(b, "endBeats"))
        };
        let title_of = |state: &Value| {
            if state["on"] == true {
                format!("Cycle {} to {}", bar_text(number(state, "startBeats")), bar_text(number(state, "endBeats")))
            } else {
                "Cycle off".to_string()
            }
        };
        let now = self.cycle_state();
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let prior = &params["prior"];
            if !same(&now, prior) {
                if params["discard"] != true && !same(&now, &params["applied"]) {
                    return Err(changed("the cycle has been changed in Logic Pro since Kumi set it"));
                }
                // (an off cycle keeps its locators, and a restore puts them back too)
                let (start, end) = (number(prior, "startBeats"), number(prior, "endBeats"));
                let (start, end) = if end > start { (on_ticks(start), on_ticks(end)) } else { (self.cycle.1, self.cycle.2) };
                self.cycle = (prior["on"] == true, start, end);
            }
            let undo = self.restored(None, &desc)?;
            let back = self.cycle_state();
            let title = format!("{} (put back)", title_of(&back));
            return Ok(
                json!({"epoch": self.epoch_text(), "restored": back, "display": {"title": title, "before": now, "after": back}, "undo": undo}),
            );
        }
        let input = &params["input"];
        let given = |key: &str| input.get(key).is_some_and(|value| !value.is_null());
        if (given("start") || given("end")) && !given("startBeats") {
            return Err(bad(
                "Logic Pro's bridge places the cycle by startBeats and endBeats (beats from the project's start), not seconds",
            ));
        }
        let beat = |key: &str| -> Result<Option<f64>, Fault> {
            match input.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => value
                    .as_f64()
                    .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                    .map(Some)
                    .ok_or_else(|| bad(format!("{key} must be a number from 0: beats from the project's start"))),
            }
        };
        let start = beat("startBeats")?
            .ok_or_else(|| bad("pass startBeats (beats from the project's start) and endBeats; 0 and 0 turn the cycle off"))?;
        let end = beat("endBeats")?.ok_or_else(|| bad("pass endBeats"))?;
        let on = !(start == 0.0 && end == 0.0);
        if on && end <= start {
            return Err(bad("the cycle must end after it starts (0 and 0 turn it off)"));
        }
        let after = if on {
            json!({"on": true, "startBeats": start, "endBeats": end})
        } else {
            json!({"on": false, "startBeats": now["startBeats"], "endBeats": now["endBeats"]})
        };
        if method == "change_preview" {
            let mut display = json!({"title": title_of(&after), "before": now, "after": after});
            if same(&now, &after) {
                display["unchanged"] = true.into();
            }
            if on && (on_ticks(start) != start || on_ticks(end) != end) {
                display["note"] = "The locators sit on Logic Pro's ticks (960 to a beat): the range is rounded to them.".into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": {"ref": null, "kind": "project"}, "prior": now, "fence": now, "display": display}),
            );
        }
        if !same(&now, &params["fence"]) {
            return Err(changed("the cycle changed since the preview"));
        }
        let desc = Self::desc(params)?;
        if !same(&now, &after) {
            self.cycle = if on { (true, on_ticks(start), on_ticks(end)) } else { (false, self.cycle.1, self.cycle.2) };
        }
        let undo = self.applied(None, &desc)?;
        let applied = self.cycle_state();
        Ok(
            json!({"epoch": self.epoch_text(), "applied": applied, "display": {"title": title_of(&applied), "before": now, "after": applied}, "undo": undo}),
        )
    }

    /// (M6) `metronome-set {on}`: the control bar's Metronome Click, pressed only when it differs from what is asked and
    /// read back (`changes/metronome.rs`). A click already as asked previews as unchanged. No step of Logic's undo: its
    /// restore presses it back by value, unless the producer turned it since.
    fn metronome_set(&mut self, method: &str, params: &Value) -> Answer {
        let state = |on: bool| json!({"on": on});
        let title = |on: bool| if on { "Metronome on" } else { "Metronome off" };
        let now = self.metronome;
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let prior = params["prior"]["on"].as_bool().ok_or_else(|| bad("prior.on"))?;
            let applied = params["applied"]["on"].as_bool();
            if now != prior {
                if params["discard"] != true && applied.is_some_and(|applied| applied != now) {
                    return Err(changed("the metronome has been turned on or off in Logic Pro since Kumi set it"));
                }
                self.metronome = prior;
            }
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(), "restored": state(prior),
                "display": {"title": format!("{} (put back)", title(prior)), "before": state(now), "after": state(prior)},
                "undo": undo,
            }));
        }
        let on = params["input"]["on"].as_bool().ok_or_else(|| bad("on must be true (the metronome's click on) or false (off)"))?;
        if method == "change_preview" {
            let mut display = json!({"title": title(on), "before": state(now), "after": state(on)});
            if now == on {
                display["unchanged"] = true.into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(), "target": {"ref": null, "kind": "project"}, "prior": state(now), "fence": state(now),
                "display": display,
            }));
        }
        if params["fence"] != state(now) {
            return Err(changed("the metronome was turned on or off in Logic Pro since the preview"));
        }
        let desc = Self::desc(params)?;
        self.metronome = on;
        let undo = self.applied(None, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(), "applied": state(on),
            "display": {"title": title(on), "before": state(now), "after": state(on)}, "undo": undo,
        }))
    }

    /// The Tempo List's row at a beat, if one is (0 is the project's first tempo event, always there).
    fn tempo_at(&self, beats: f64) -> Value {
        if beats == 0.0 {
            return json!({"exists": true, "bpm": self.tempo});
        }
        match self.tempo_changes.iter().find(|(at, _)| *at == beats) {
            Some((_, bpm)) => json!({"exists": true, "bpm": bpm}),
            None => json!({"exists": false}),
        }
    }

    /// The tempo in force at a beat: what a tempo event made there comes with.
    fn tempo_in_force(&self, beats: f64) -> f64 {
        self.tempo_changes.iter().rev().find(|(at, _)| *at <= beats).map_or(self.tempo, |(_, bpm)| *bpm)
    }

    /// The Tempo List's rows: the project's first tempo event and each one after it.
    fn tempo_events(&self) -> usize {
        1 + self.tempo_changes.len()
    }

    fn put_tempo(&mut self, beats: f64, bpm: f64) {
        if beats == 0.0 {
            self.tempo = bpm;
            return;
        }
        match self.tempo_changes.iter_mut().find(|(at, _)| *at == beats) {
            Some(change) => change.1 = bpm,
            None => {
                self.tempo_changes.push((beats, bpm));
                self.tempo_changes.sort_by(|a, b| a.0.total_cmp(&b.0));
            }
        }
    }

    /// The steps of Logic's undo a tempo of `bpm` at `beats` takes: a row made first ("Insert Tempo", with the tempo in
    /// force) when none is there, and its Tempo cell stepped ("Value Change") when its BPM isn't `bpm` already.
    fn tempo_steps(&self, beats: f64, bpm: f64) -> Vec<&'static str> {
        let now = self.tempo_at(beats);
        let mut steps = vec![];
        let from = if now["exists"] == true {
            number(&now, "bpm")
        } else {
            steps.push("Insert Tempo");
            self.tempo_in_force(beats)
        };
        if (from - bpm).abs() >= 0.0001 {
            steps.push("Value Change");
        }
        steps
    }

    /// Two Tempo List rows the same: both there with the same BPM, or neither there.
    fn same_tempo(a: &Value, b: &Value) -> bool {
        a["exists"] == b["exists"] && (a["exists"] == false || (number(a, "bpm") - number(b, "bpm")).abs() < 0.0001)
    }

    /// A BPM as the Tempo List's Tempo cell takes it: 5 to 990, to four decimals.
    fn bpm_input(input: &Value) -> Result<f64, Fault> {
        let bpm = input.get("bpm").and_then(Value::as_f64).filter(|bpm| bpm.is_finite()).ok_or_else(|| bad("bpm must be a number"))?;
        if !(TEMPO_MIN..=TEMPO_MAX).contains(&bpm) {
            return Err(bad(format!("bpm must be from {TEMPO_MIN} to {TEMPO_MAX}")));
        }
        Ok((bpm * 10000.0).round() / 10000.0)
    }

    /// A BPM as the bridge's titles say it: `128`, `120.25`.
    fn bpm_text(bpm: f64) -> String {
        let text = format!("{bpm:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }

    /// `tempo-marker-set {positionBeats, bpm}`: the Tempo List's row at the beat, its Tempo cell stepped; or a row made
    /// first at the playhead, Create new Tempo Event (two steps of Logic's undo: "Insert Tempo", then the cell's "Value
    /// Change"). At 0 it is the project's first tempo. Its restore is by value: the row stepped back, or (one Kumi made)
    /// deleted again.
    fn tempo_marker_set(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let beats = on_the_beat(params["target"]["positionBeats"].as_f64().ok_or_else(|| bad("target.positionBeats"))?);
            let (now, prior) = (self.tempo_at(beats), &params["prior"]);
            let title = format!("Tempo at {} put back", bar_text(beats));
            if Self::same_tempo(&now, prior) {
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": now, "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
                );
            }
            if params["discard"] != true && !Self::same_tempo(&now, &params["applied"]) {
                return Err(changed(format!("the tempo at {} has been changed in Logic Pro since Kumi set it", bar_text(beats))));
            }
            if prior["exists"] == true {
                let bpm = prior["bpm"].as_f64().unwrap_or(120.0);
                let steps = self.tempo_steps(beats, bpm);
                self.put_tempo(beats, bpm);
                let undo = self.restored_steps(&steps, &desc)?;
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": self.tempo_at(beats), "display": {"title": title}, "undo": undo}),
                );
            }
            // Kumi made the row: taken away again.
            if now["exists"] != true {
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": {"exists": false}, "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
                );
            }
            self.tempo_changes.retain(|(at, _)| *at != beats);
            let undo = self.restored(Some("Delete"), &desc)?;
            let title = format!("Tempo event at {} taken away again", bar_text(beats));
            return Ok(json!({"epoch": self.epoch_text(), "restored": {"exists": false}, "display": {"title": title}, "undo": undo}));
        }
        let input = &params["input"];
        let bpm = Self::bpm_input(input)?;
        let beats = on_the_beat(Self::position_of_input(input, "tempo and signature changes")?);
        let now = self.tempo_at(beats);
        let wanted = json!({"exists": true, "bpm": bpm});
        let title = format!("Tempo {} BPM at {}", Self::bpm_text(bpm), bar_text(beats));
        let fence = json!({"tempo": now, "events": self.tempo_events()});
        if method == "change_preview" {
            let mut display = json!({"title": title, "before": now, "after": wanted});
            if Self::same_tempo(&now, &wanted) {
                display["unchanged"] = true.into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": null, "kind": "tempo-marker", "positionBeats": beats},
                "prior": now,
                "fence": fence,
                "display": display,
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the tempo events changed since the preview"));
        }
        let desc = Self::desc(params)?;
        if Self::same_tempo(&now, &wanted) {
            return Ok(
                json!({"epoch": self.epoch_text(), "applied": now, "display": {"title": title, "unchanged": true}, "undo": {"desc": desc, "entry": false}}),
            );
        }
        let made = now["exists"] != true;
        let steps = self.tempo_steps(beats, bpm);
        self.put_tempo(beats, bpm);
        let undo = self.applied_steps(&steps, &desc)?;
        let after = self.tempo_at(beats);
        let mut applied = after.clone();
        applied["made"] = made.into();
        Ok(
            json!({"epoch": self.epoch_text(), "applied": applied, "display": {"title": title, "before": now, "after": after}, "undo": undo}),
        )
    }

    /// `tempo-marker-delete {positionBeats}`: the Tempo List row's Delete action (never the project's first tempo
    /// event); what it was is kept in `material`. Its restore is Logic's own undo while that step is on top, else the
    /// row made again by value.
    fn tempo_marker_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let material =
                params.get("material").filter(|material| !material.is_null()).ok_or_else(|| bad("material: what the delete removed"))?;
            let bpm = material["bpm"].as_f64().ok_or_else(|| bad("material.bpm"))?;
            let beats = on_the_beat(material["positionBeats"].as_f64().unwrap_or(0.0));
            let title = format!("Tempo event at {} put back", bar_text(beats));
            let now = self.tempo_at(beats);
            if now["exists"] == true {
                if params["discard"] == true || (number(&now, "bpm") - bpm).abs() < 0.0001 {
                    return Ok(
                        json!({"epoch": self.epoch_text(), "restored": now, "display": {"title": format!("{title} already")}, "undo": {"desc": desc, "entry": false}}),
                    );
                }
                return Err(changed(format!("another tempo event is at {} now", bar_text(beats))));
            }
            let key = Self::key_of(params);
            if let Ok(step) = self.undo_in_logic("Delete", |top| top.key.as_deref() == Some(key.as_str())) {
                if self.restore_unanswered() {
                    return Err(uncertain(format!(
                        "Logic Pro's {:?} was pressed, but no tempo event is back at {}",
                        step.title,
                        bar_text(beats)
                    )));
                }
                self.put_tempo(beats, bpm);
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": self.tempo_at(beats),
                    "display": {"title": title, "note": "Put back with Logic Pro's own undo."},
                    "undo": {"desc": desc, "entry": false, "how": "undo"},
                }));
            }
            let steps = self.tempo_steps(beats, bpm);
            self.put_tempo(beats, bpm);
            let undo = self.restored_steps(&steps, &desc)?;
            return Ok(json!({"epoch": self.epoch_text(), "restored": self.tempo_at(beats), "display": {"title": title}, "undo": undo}));
        }
        let input = &params["input"];
        let beats = on_the_beat(Self::position_of_input(input, "tempo and signature changes")?);
        let now = self.tempo_at(beats);
        if now["exists"] != true {
            return Err(match method {
                "change_preview" => failed(format!("there is no tempo event at {}", bar_text(beats))),
                _ => changed("the tempo event went since the preview"),
            });
        }
        if beats == 0.0 {
            return Err(bad("the project's first tempo event stays: change it with tempo_marker_set"));
        }
        let fence = json!({"tempo": now, "events": self.tempo_events()});
        let title = format!("Deleted tempo event at {}", bar_text(beats));
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": null, "kind": "tempo-marker", "positionBeats": beats},
                "prior": now,
                "fence": fence,
                "display": {"title": title},
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the tempo events changed since the preview"));
        }
        let desc = Self::desc(params)?;
        self.tempo_changes.retain(|(at, _)| *at != beats);
        let undo = self.applied(Some("Delete"), &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"deleted": true},
            "material": {"positionBeats": beats, "bpm": now["bpm"]},
            "display": {"title": title},
            "undo": undo,
        }))
    }

    /// The Signature List's Time row at a beat, if one is (0 is the project's, always there).
    fn signature_at(&self, beats: f64) -> Value {
        if beats == 0.0 {
            return json!({"exists": true, "timeSignature": [self.signature.0, self.signature.1]});
        }
        match self.signature_changes.iter().find(|(at, _)| *at == beats) {
            Some((_, (num, den))) => json!({"exists": true, "timeSignature": [num, den]}),
            None => json!({"exists": false}),
        }
    }

    fn put_signature(&mut self, beats: f64, signature: (i64, i64)) {
        if beats == 0.0 {
            self.signature = signature;
            return;
        }
        match self.signature_changes.iter_mut().find(|(at, _)| *at == beats) {
            Some(change) => change.1 = signature,
            None => {
                self.signature_changes.push((beats, signature));
                self.signature_changes.sort_by(|a, b| a.0.total_cmp(&b.0));
            }
        }
    }

    /// A time signature as the Signature List takes it: [numerator, denominator] (or {num, den}), 1 to 99 over one of
    /// its denominators.
    fn signature_input(input: &Value) -> Result<(i64, i64), Fault> {
        let pair = input.get("timeSignature").ok_or_else(|| bad("timeSignature must be [numerator, denominator]"))?;
        let (num, den) = match pair {
            Value::Array(pair) if pair.len() == 2 => (pair[0].as_u64(), pair[1].as_u64()),
            Value::Object(_) => (pair["num"].as_u64(), pair["den"].as_u64()),
            _ => (None, None),
        };
        match (num, den) {
            (Some(num), Some(den)) if (1..=99).contains(&num) && [1, 2, 4, 8, 16, 32].contains(&den) => Ok((num as i64, den as i64)),
            _ => Err(bad("timeSignature must be [numerator, denominator]: 1 to 99 over 1, 2, 4, 8, 16 or 32")),
        }
    }

    /// `signature-set {positionBeats, timeSignature}`: the Signature List's Time row at the bar, its numerator stepped
    /// and its denominator chosen; or a row made first (Create new Time/Key Signature Change): a step of Logic's undo
    /// its Edit menu names "Value Change" either way. At 0 it is the project's. Its restore is by value: the row set
    /// back, or (one Kumi made) deleted through the list's Edit > Delete. (The simulator counts bars by four beats
    /// whatever the signature: its positions don't follow a change at the start.)
    fn signature_set(&mut self, method: &str, params: &Value) -> Answer {
        let text = |state: &Value| format!("{}/{}", state["timeSignature"][0], state["timeSignature"][1]);
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let beats = on_the_beat(params["target"]["positionBeats"].as_f64().ok_or_else(|| bad("target.positionBeats"))?);
            let (now, prior) = (self.signature_at(beats), &params["prior"]);
            let title = format!("Time signature at {} put back", bar_text(beats));
            if now == *prior {
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": now, "display": {"title": title}, "undo": {"desc": desc, "entry": false}}),
                );
            }
            let mut applied = params["applied"].clone();
            if let Some(fields) = applied.as_object_mut() {
                fields.remove("made");
            }
            if params["discard"] != true && now != applied {
                return Err(changed(format!("the time signature at {} has been changed in Logic Pro since Kumi set it", bar_text(beats))));
            }
            if prior["exists"] == true {
                let signature = Self::signature_input(&json!({"timeSignature": prior["timeSignature"]}))?;
                self.put_signature(beats, signature);
                let undo = self.restored(Some("Value Change"), &desc)?;
                return Ok(
                    json!({"epoch": self.epoch_text(), "restored": self.signature_at(beats), "display": {"title": title}, "undo": undo}),
                );
            }
            // Kumi made the row, and a signature row has no Delete action of its own: the list's Edit > Delete.
            self.signature_changes.retain(|(at, _)| *at != beats);
            let undo = self.restored(Some("Delete"), &desc)?;
            let title = format!("Time signature at {} taken away again", bar_text(beats));
            return Ok(json!({"epoch": self.epoch_text(), "restored": {"exists": false}, "display": {"title": title}, "undo": undo}));
        }
        let input = &params["input"];
        let signature = Self::signature_input(input)?;
        let beats = on_the_beat(Self::position_of_input(input, "tempo and signature changes")?);
        if position_of(beats)["beat"] != 1 {
            return Err(bad("a time signature changes at a bar: give the bar's first beat"));
        }
        let now = self.signature_at(beats);
        let wanted = json!({"exists": true, "timeSignature": [signature.0, signature.1]});
        let title = format!("Time signature {} at {}", text(&wanted), bar_text(beats));
        let fence = json!({"signature": now, "events": 1 + self.signature_changes.len()});
        if method == "change_preview" {
            let mut display = json!({"title": title, "before": now, "after": wanted});
            if now == wanted {
                display["unchanged"] = true.into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": null, "kind": "signature", "positionBeats": beats},
                "prior": now,
                "fence": fence,
                "display": display,
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the time signatures changed since the preview"));
        }
        let desc = Self::desc(params)?;
        if now == wanted {
            return Ok(
                json!({"epoch": self.epoch_text(), "applied": now, "display": {"title": title, "unchanged": true}, "undo": {"desc": desc, "entry": false}}),
            );
        }
        let made = now["exists"] != true;
        self.put_signature(beats, signature);
        let undo = self.applied(Some("Value Change"), &desc)?;
        let after = self.signature_at(beats);
        let mut applied = after.clone();
        applied["made"] = made.into();
        Ok(
            json!({"epoch": self.epoch_text(), "applied": applied, "display": {"title": title, "before": now, "after": after}, "undo": undo}),
        )
    }

    // ----- (M4) a MIDI file imported, Track Stacks -----

    /// The notes of an import as the bridge's file takes them, in its words for what is off.
    fn import_notes(input: &Value) -> Result<Vec<ImportNote>, Fault> {
        let list = input.get("notes").and_then(Value::as_array).ok_or_else(|| bad("notes must be a list"))?;
        if list.is_empty() || list.len() > MOST_IMPORTED {
            return Err(bad(format!("notes must hold from 1 to {MOST_IMPORTED} notes")));
        }
        let whole = |spec: &Value, key: &str, low: u64, high: u64, fallback: Option<u64>| -> Result<u8, Fault> {
            match spec.get(key) {
                None | Some(Value::Null) => {
                    fallback.map(|n| n as u8).ok_or_else(|| bad(format!("every note needs a {key} ({low} to {high})")))
                }
                Some(value) => value
                    .as_u64()
                    .filter(|n| (low..=high).contains(n))
                    .map(|n| n as u8)
                    .ok_or_else(|| bad(format!("{key} must be a whole number from {low} to {high}"))),
            }
        };
        let beats = |spec: &Value, key: &str| -> Result<f64, Fault> {
            spec.get(key)
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                .ok_or_else(|| bad(format!("every note needs a {key} (beats from the region's start)")))
        };
        list.iter()
            .map(|spec| {
                let length = beats(spec, "length")?;
                if length <= 0.0 {
                    return Err(bad("every note needs a length above 0 (beats)"));
                }
                Ok(ImportNote {
                    pitch: whole(spec, "pitch", 0, 127, None)?,
                    start: beats(spec, "start")?,
                    length,
                    velocity: whole(spec, "velocity", 1, 127, Some(100))?,
                    channel: whole(spec, "channel", 1, 16, Some(1))?,
                })
            })
            .collect()
    }

    /// `midi-import {ref, positionBeats, notes}`: the bridge writes the notes as a Standard MIDI File (named by its
    /// bytes, under /tmp/kumi-logic), selects the track, puts the playhead at the beat (the LCD's bar and beat) and
    /// imports the file (File > Import > MIDI File…, through the panel service): Logic lands it on a new software
    /// instrument track below the one named, which it names after its default patch, as a region it names itself (the
    /// file carries no track name), as long as the file's span rounded up to the bar. The epoch moves. Its restore is
    /// Logic's own undo ("Import") while that step is on top, which takes the track away with the region.
    fn midi_import(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let created =
                params.get("created").filter(|created| !created.is_null()).ok_or_else(|| bad("created: the region the import made"))?;
            let Ok(at) = self.region_at(created["ref"].as_str(), true) else {
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"deleted": created["ref"]},
                    "display": {"title": "The imported region is gone already"},
                    "undo": {"desc": desc, "entry": false},
                }));
            };
            let (region_id, track_id) = (self.regions[at].id, self.regions[at].track);
            let there = self.region_state(&self.regions[at]);
            let applied = &params["applied"];
            if params["discard"] != true
                && (there["startBeats"] != applied["startBeats"]
                    || there["endBeats"] != applied["endBeats"]
                    || there["track"] != applied["track"])
            {
                return Err(changed("the imported region has been moved in Logic Pro since Kumi imported it"));
            }
            let key = Self::key_of(params);
            let pressed = self.undo_in_logic("Import", |top| top.key.as_deref() == Some(key.as_str()))?.title;
            if self.restore_unanswered() {
                return Err(uncertain(format!("Logic Pro's {pressed:?} was pressed, but the imported region is still there")));
            }
            self.regions.retain(|region| region.id != region_id);
            // (the track Logic made for the file goes with it)
            if created["trackAdded"] == true {
                if let Some(track) = self.track_index(track_id) {
                    self.tracks.remove(track);
                }
            }
            self.bump_epoch();
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"deleted": created["ref"], "pressed": pressed},
                "display": {"title": "Import taken back", "note": "Put back with Logic Pro's own undo."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
        }
        let input = &params["input"];
        let at = self.track_at(input["ref"].as_str(), false).map_err(|fault| {
            if method == "change_apply" && fault.0 == "failed" {
                changed(fault.1)
            } else {
                fault
            }
        })?;
        let position = input
            .get("positionBeats")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start, where the region goes"))?;
        let notes = Self::import_notes(input)?;
        let count = notes.len();
        let title = |track: &str, beats: f64| {
            format!("Imported {count} note{} on {} at {}", if count == 1 { "" } else { "s" }, quoted(track), bar_text(beats))
        };
        let names: Vec<String> = self.tracks.iter().map(|track| track.name.clone()).collect();
        let fence = json!({"tracks": self.tracks.len(), "names": names, "regions": self.regions.len()});
        if method == "change_preview" {
            let named = if self.tracks[at].name.is_empty() { format!("Track {}", at + 1) } else { self.tracks[at].name.clone() };
            let mut display = json!({"title": title(&named, position), "notes": count, "note": "Logic Pro's own undo takes an import back (\"Import\"), while it is its latest step."});
            if !on_a_beat(position) {
                display["note"] = "Logic Pro's playhead display places the region on a beat: the division and tick are dropped.".into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": format!("{}:track:{}", self.epoch, at + 1), "kind": "track"},
                "prior": {"tracks": fence["tracks"], "regions": fence["regions"]},
                "fence": fence,
                "display": display,
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the tracks or regions changed since the preview"));
        }
        let desc = Self::desc(params)?;
        let file = format!("{IMPORT_FOLDER}/kumi-{}.mid", &daw_bridge::auth::sha256_hex(&midi_file(&notes))[..12]);
        // The region Logic makes: at the playhead's bar and beat, the file's span rounded up to the bar.
        let start = on_the_beat(position);
        let span = notes.iter().map(|note| note.start + note.length).fold(0.0, f64::max);
        let end = start + (span / BEATS_PER_BAR).ceil().max(1.0) * BEATS_PER_BAR;
        let mut track = Track::made(self.next_track, IMPORT_TRACK, "instrument");
        // (M4b) Logic's default patch on it, which names it
        (track.channel, track.smart) = mixer::imported_channel();
        let track_id = track.id;
        self.next_track += 1;
        self.tracks.insert(at + 1, track);
        let region = Region {
            id: self.next_region,
            track: track_id,
            name: IMPORT_REGION.into(),
            kind: "midi".into(),
            start,
            end,
            looping: false,
            selected: false,
            notes: notes.iter().map(ImportNote::row).collect(),
        };
        let id = region.id;
        self.next_region += 1;
        self.regions.push(region);
        self.select_region(id);
        self.sort_regions();
        self.bump_epoch();
        let undo = self.applied(Some("Import"), &desc)?;
        let (place, track_place) = (self.region_index(id).map_or(0, |at| at + 1), at + 2);
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"notes": count, "startBeats": start, "endBeats": end, "track": track_place, "trackName": IMPORT_TRACK, "file": file, "answered": []},
            "created": {"ref": self.region_ref(place), "kind": "region", "track": format!("{}:track:{track_place}", self.epoch), "trackAdded": true},
            "display": {"title": title(IMPORT_TRACK, start), "notes": count},
            "undo": undo,
        }))
    }

    /// The stack whose own track is at `at` flattened: its track gone, its tracks on their own where they are (a
    /// summing stack's keep their output on its bus: Logic's own doing); the epoch moves. The stack's own track.
    fn flatten(&mut self, at: usize) -> Track {
        let header = self.tracks.remove(at);
        for track in &mut self.tracks {
            if track.in_stack == Some(header.id) {
                track.in_stack = None;
            }
        }
        self.sort_regions();
        self.bump_epoch();
        header
    }

    /// `track-stack {refs, kind, name?}`: the tracks' headers clicked (Shift for the others), then Logic's own Create
    /// Summing Stack or Create Folder Stack: the stack's own track where the first of them was, the tracks after it in
    /// their order (Logic gathers them); Logic names it (Sum 1, Folder 1), and a `name` renames it after through the
    /// Inspector, which puts "Renaming" on top of "Create Track Stack". The epoch moves once. Its restore is Logic's own
    /// undo while the creation is on top (the tracks back where they were), else Flatten Stack, by value.
    fn track_stack(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let at =
                self.track_at(params["created"]["ref"].as_str(), true)
                    .map_err(|fault| if fault.0 == "failed" { gone(fault.1) } else { fault })?;
            let name = params["applied"]["name"].as_str().unwrap_or_default().to_string();
            let stack = self.tracks[at].clone();
            if !stack.is_stack() || stack.name != name {
                if params["discard"] == true {
                    return Ok(json!({
                        "epoch": self.epoch_text(),
                        "restored": {"tracks": self.tracks.len()},
                        "display": {"title": format!("Stack {} is gone already", quoted(&name))},
                        "undo": {"desc": desc, "entry": false},
                    }));
                }
                return Err(changed(format!("track {} isn't the stack {} Kumi made any more", at + 1, quoted(&name))));
            }
            let key = Self::key_of(params);
            if let Ok(step) = self.undo_in_logic("Create Track Stack", |top| top.key.as_deref() == Some(key.as_str())) {
                if self.restore_unanswered() {
                    return Err(uncertain(format!(
                        "Logic Pro's {:?} was pressed, but the stack {} is still there",
                        step.title,
                        quoted(&name)
                    )));
                }
                // The tracks where they were before, exactly: the stack's own track gone.
                self.tracks.remove(at);
                for track in &mut self.tracks {
                    if track.in_stack == Some(stack.id) {
                        track.in_stack = None;
                    }
                }
                if let Some(order) = step.desc.and_then(|desc| self.stacked.remove(&desc)) {
                    self.tracks.sort_by_key(|track| order.iter().position(|id| *id == track.id).unwrap_or(usize::MAX));
                }
                self.sort_regions();
                self.bump_epoch();
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"tracks": self.tracks.len(), "pressed": step.title},
                    "display": {"title": format!("Stack {} taken apart again", quoted(&name)), "note": "Put back with Logic Pro's own undo."},
                    "undo": {"desc": desc, "entry": false, "how": "undo"},
                }));
            }
            self.flatten(at);
            let undo = self.restored(Some("Flatten Stack"), &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"tracks": self.tracks.len()},
                "display": {"title": format!("Stack {} flattened again", quoted(&name))},
                "undo": undo,
            }));
        }
        let input = &params["input"];
        let refs = input.get("refs").and_then(Value::as_array).ok_or_else(|| bad("refs must be a list of track refs"))?;
        if refs.is_empty() || refs.len() > 64 {
            return Err(bad("refs must name from 1 to 64 tracks"));
        }
        let mut places: Vec<usize> = vec![];
        for reference in refs {
            let place = self.track_at(reference.as_str(), false).map_err(|fault| {
                if method == "change_apply" && fault.0 == "failed" {
                    changed(fault.1)
                } else {
                    fault
                }
            })?;
            if !places.contains(&place) {
                places.push(place);
            }
        }
        places.sort_unstable();
        let kind = match input.get("kind").and_then(Value::as_str) {
            Some("summing") => "summing",
            Some("folder") => "folder",
            _ => return Err(bad("kind must be \"summing\" or \"folder\"")),
        };
        let name = match input.get("name") {
            None | Some(Value::Null) => None,
            Some(name) => Some(name.as_str().filter(|name| !name.trim().is_empty()).ok_or_else(|| bad("name must be text"))?.to_string()),
        };
        let names: Vec<String> = self.tracks.iter().map(|track| track.name.clone()).collect();
        let fence = json!({"tracks": self.tracks.len(), "names": names});
        let members = and_list(places.iter().map(|&place| self.tracks[place].name.as_str()));
        let noun = if kind == "summing" { "Summing" } else { "Folder" };
        if method == "change_preview" {
            if let Some(stacked) = places.iter().find(|&&place| self.tracks[place].is_stack()) {
                return Err(bad(format!("track {} is a Track Stack already; Logic Pro doesn't stack stacks from outside", stacked + 1)));
            }
            let mut display = json!({"title": format!("{noun} stack of {members}"), "kind": kind});
            if let Some(name) = &name {
                display["name"] = name.clone().into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": null, "kind": "track"},
                "prior": {"tracks": self.tracks.len()},
                "fence": fence,
                "display": display,
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the tracks changed since the preview"));
        }
        let desc = Self::desc(params)?;
        let before: Vec<u64> = self.tracks.iter().map(|track| track.id).collect();
        // Logic's own name for it, and (a summing stack's) the bus it mixes through.
        let made = if kind == "summing" { &mut self.summing_stacks } else { &mut self.folder_stacks };
        *made += 1;
        let number = *made;
        let named = format!("{} {number}", if kind == "summing" { "Sum" } else { "Folder" });
        let stack = Track::stack(self.next_track, &named, kind, number);
        let stack_id = stack.id;
        self.next_track += 1;
        let first = places[0];
        let mut gathered: Vec<Track> = places.iter().rev().map(|&place| self.tracks.remove(place)).collect();
        gathered.reverse();
        self.tracks.insert(first, stack);
        for (n, mut track) in gathered.into_iter().enumerate() {
            track.in_stack = Some(stack_id);
            if kind == "summing" {
                track.row["output"] = format!("Bus {number}").into();
            }
            self.tracks.insert(first + 1 + n, track);
        }
        let mut steps = vec!["Create Track Stack"];
        let shown = match name {
            Some(name) => {
                self.tracks[first].name = name.clone();
                steps.push("Renaming");
                name
            }
            None => named,
        };
        self.sort_regions();
        self.bump_epoch();
        self.stacked.insert(desc.clone(), before);
        let undo = self.applied_steps(&steps, &desc)?;
        let subtracks: Vec<String> = (first + 2..=first + 1 + places.len()).map(|place| format!("{}:track:{place}", self.epoch)).collect();
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"index": first + 1, "name": shown, "kind": kind, "tracks": subtracks},
            "created": {"ref": format!("{}:track:{}", self.epoch, first + 1), "kind": "track", "stack": kind},
            "display": {"title": format!("{noun} stack {} of {members}", quoted(&shown))},
            "undo": undo,
        }))
    }

    /// `track-stack-flatten {ref}`: the stack's header clicked, then Logic's own Flatten Stack: its own track gone, its
    /// tracks on their own where they are; the epoch moves, and the stack as it was is kept in `material` (its name,
    /// its place, and how many tracks showed with it open). Its restore is Logic's own undo while that step is on top:
    /// which tracks were under a stack isn't readable once it is flat.
    fn track_stack_flatten(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let material =
                params.get("material").filter(|material| !material.is_null()).ok_or_else(|| bad("material: the stack as it was"))?;
            let name = material["name"].as_str().unwrap_or_default().to_string();
            let place = material["index"].as_u64().unwrap_or(0) as usize;
            if self.tracks.get(place.wrapping_sub(1)).is_some_and(|track| track.is_stack() && track.name == name) {
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"tracks": self.tracks.len()},
                    "display": {"title": format!("Stack {} is back already", quoted(&name))},
                    "undo": {"desc": desc, "entry": false},
                }));
            }
            let key = Self::key_of(params);
            let step = self.undo_in_logic("Flatten Stack", |top| top.key.as_deref() == Some(key.as_str()))?;
            let unanswered =
                || uncertain(format!("Logic Pro's {:?} was pressed, but no stack {} is back at track {place}", step.title, quoted(&name)));
            if self.restore_unanswered() {
                return Err(unanswered());
            }
            let Some((stack, members)) = step.desc.as_ref().and_then(|desc| self.flattened.remove(desc)) else { return Err(unanswered()) };
            let id = stack.id;
            let at = place.saturating_sub(1).min(self.tracks.len());
            self.tracks.insert(at, stack);
            for track in &mut self.tracks {
                if members.contains(&track.id) {
                    track.in_stack = Some(id);
                }
            }
            self.sort_regions();
            self.bump_epoch();
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"tracks": self.tracks.len(), "pressed": step.title, "ref": format!("{}:track:{}", self.epoch, at + 1)},
                "display": {"title": format!("Stack {} back", quoted(&name)), "note": "Put back with Logic Pro's own undo."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
        }
        let input = &params["input"];
        let at = self.track_at(input["ref"].as_str(), false).map_err(|fault| {
            if method == "change_apply" && fault.0 == "failed" {
                changed(fault.1)
            } else {
                fault
            }
        })?;
        let stack = self.tracks[at].clone();
        if !stack.is_stack() {
            return Err(bad(format!("track {} {} isn't a Track Stack", at + 1, quoted(&stack.name))));
        }
        let names: Vec<String> = self.tracks.iter().map(|track| track.name.clone()).collect();
        let fence = json!({"tracks": self.tracks.len(), "names": names});
        let title = format!("Stack {} flattened", quoted(&stack.name));
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": format!("{}:track:{}", self.epoch, at + 1), "kind": "track"},
                "prior": {"name": stack.name, "index": at + 1, "tracks": self.tracks.len()},
                "fence": fence,
                "display": {"title": title},
            }));
        }
        if params["fence"] != fence {
            return Err(changed("the tracks changed since the preview"));
        }
        let desc = Self::desc(params)?;
        // (the stack open, every track of it shows)
        let shown = self.tracks.len();
        let members: Vec<u64> = self.tracks.iter().filter(|track| track.in_stack == Some(stack.id)).map(|track| track.id).collect();
        let header = self.flatten(at);
        self.flattened.insert(desc.clone(), (header, members));
        let undo = self.applied(Some("Flatten Stack"), &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"name": stack.name, "tracks": self.tracks.len()},
            "material": {"name": stack.name, "index": at + 1, "tracks": shown},
            "display": {"title": title},
            "undo": undo,
        }))
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
        // (M4b) A strip's plug-ins and sends, a plug-in's parameters, Smart Controls, automation, the Mixer's strips, and
        // the strip's bodies.
        if let Some(made) = self.mixer_rows(method, asked, params) {
            return Some(made);
        }
        let kind = asked.get("kind").and_then(Value::as_str);
        let track_rows = || -> Vec<Value> { self.tracks.iter().enumerate().map(|(at, track)| track.row(at + 1)).collect() };
        let marker_rows = || -> Vec<Value> { self.markers.iter().enumerate().map(|(at, marker)| marker.row(at + 1)).collect() };
        let signature_text = format!("{}/{}", self.signature.0, self.signature.1);
        let patch_transport = |transport: &mut Value| {
            transport["playing"] = self.playing.into();
            transport["tempo"] = self.tempo.into();
            transport["cycle"] = self.cycle.0.into();
            if transport.get("timeSignature").is_some() {
                transport["timeSignature"] = signature_text.clone().into();
            }
            if transport.get("beatsPerBar").is_some() {
                transport["beatsPerBar"] = self.signature.0.into();
            }
        };
        let patch_project = |project: &mut Value| {
            project["tempo"] = self.tempo.into();
            project["trackCount"] = self.tracks.len().into();
            project["timeSignature"] = signature_text.clone().into();
            // (M6) the octave Logic names middle C in, as the producer set it
            if project.get("middleC").is_some() {
                project["middleC"] = self.middle_c.into();
            }
        };
        let playhead = || {
            let position = position_of(self.playhead);
            json!({"bar": position["bar"], "beat": position["beat"]})
        };
        // The place a region ref of the recorded epoch names (from 0), if the region is there.
        let region_of = |reference: &str| -> Option<usize> {
            let rest = reference.strip_prefix(&format!("{RECORDED_EPOCH}:region:"))?;
            rest.parse::<usize>().ok().filter(|place| *place >= 1 && *place <= self.regions.len()).map(|place| place - 1)
        };
        let gone = |reference: &str| ("failed".to_string(), format!("{reference} isn't in the project (deleted, or another project)"));
        // A region's notes as a FOCUS body (the recorded shape).
        let notes_body = |at: usize| {
            let region = &self.regions[at];
            let rows = region.note_rows(at + 1);
            alphabetical(json!({
                "endBeats": region.end, "epoch": RECORDED_EPOCH.to_string(), "kind": "notes", "name": region.name,
                "notes": rows, "regionRef": format!("{RECORDED_EPOCH}:region:{}", at + 1), "startBeats": region.start,
                "timeSignature": signature_text, "total": region.notes.len(),
            }))
        };
        Some(match method {
            "read" if asked.get("ref").is_some() => {
                let reference = asked["ref"].as_str().unwrap_or_default();
                let (_, rest) = reference.split_once(':')?;
                if epoch_of(reference) != Some(RECORDED_EPOCH) {
                    return None;
                }
                let asked_for = params["ref"].as_str().unwrap_or_default();
                if rest.starts_with("region:") {
                    return Some(match region_of(reference) {
                        Some(at) => Ok(json!({"epoch": RECORDED_EPOCH.to_string(), "items": [self.region_rows()[at].clone()]})),
                        None => Err(gone(asked_for)),
                    });
                }
                if !rest.starts_with("track:") {
                    return None;
                }
                match rest["track:".len()..].parse::<usize>().ok().filter(|place| *place >= 1 && *place <= self.tracks.len()) {
                    Some(place) => Ok(json!({"epoch": RECORDED_EPOCH.to_string(), "items": [self.tracks[place - 1].row(place)]})),
                    None => Err(gone(asked_for)),
                }
            }
            "read" if kind == Some("track") && asked.get("parent").is_none() => page_of(track_rows(), asked),
            "read" if kind == Some("marker") && asked.get("parent").is_none() => page_of(marker_rows(), asked),
            // (M4) The regions, through the project or under a track; a region's notes as the Event List lists them.
            "read" if kind == Some("region") => match asked.get("parent").and_then(Value::as_str) {
                None => page_of(self.region_rows(), asked),
                // (the request is in the recorded epoch here; a parent from another epoch is left to the recorded
                // refusals and the stale check)
                Some(parent) => {
                    let place = parent.strip_prefix(&format!("{RECORDED_EPOCH}:track:")).and_then(|n| n.parse::<usize>().ok())?;
                    match place >= 1 && place <= self.tracks.len() {
                        true => page_of(self.region_rows().into_iter().filter(|row| row["trackRef"] == parent).collect(), asked),
                        false => Err(gone(params["parent"].as_str().unwrap_or_default())),
                    }
                }
            },
            "read" if kind == Some("note") && asked.get("parent").is_some() => match region_of(asked["parent"].as_str().unwrap_or_default()) {
                Some(at) => page_of(self.regions[at].note_rows(at + 1), asked),
                None => Err(gone(params["parent"].as_str().unwrap_or_default())),
            },
            // (M6) The project's row and the tempo row are paged as any kind's (catch-up's snapshot asks for a page).
            "read" if kind == Some("project") => recorded("read", json!({"kind": "project"})).and_then(|page| {
                let mut row = page["items"][0].clone();
                patch_project(&mut row);
                page_of(vec![row], asked)
            }),
            "read" if kind == Some("tempo") => recorded("read", json!({"kind": "tempo"}))
                .and_then(|page| page_of(vec![self.tempo_row(&page["items"][0], &signature_text)], asked)),
            // (M6) The projects Logic opened lately, as File > Open Recent lists them.
            "read" if kind == Some("recent") => page_of(self.recent_rows(), asked),
            // (M6) The bulk read: every row of the kinds asked, as `read` pages them, from one look in one epoch.
            "snapshot" if asked.get("rows").is_some_and(|rows| !rows.is_null()) => self.bulk(asked, &signature_text),
            "snapshot" => recorded("snapshot", asked.clone()).map(|mut snapshot| {
                patch_project(&mut snapshot["project"]);
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
                let lanes: Vec<Value> = self
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(at, track)| {
                        // Each track's regions, as the Tracks area shows them now (a track Kumi added holds none).
                        let regions: Vec<Value> = self
                            .regions
                            .iter()
                            .enumerate()
                            .filter(|(_, region)| region.track == track.id)
                            .map(|(place, region)| region.lane_row(place + 1))
                            .collect();
                        json!({"kind": track.kind, "name": track.name, "regions": regions, "trackRef": format!("{RECORDED_EPOCH}:track:{}", at + 1)})
                    })
                    .collect();
                whole["lanes"] = Value::Array(lanes);
                whole["trackCount"] = self.tracks.len().into();
                whole["playing"] = self.playing.into();
                whole["playheadBeats"] = self.playhead.into();
                whole["beatsPerBar"] = self.signature.0.into();
                whole["markers"] = self
                    .markers
                    .iter()
                    .enumerate()
                    .map(|(at, m)| json!({"name": m.name_at(at + 1), "positionBeats": m.position_beats}))
                    .collect();
                timeline_page(&whole, asked)
            }),
            "focus_body" if kind == Some("notes") => match asked.get("ref").and_then(Value::as_str) {
                Some(reference) => match region_of(reference) {
                    Some(at) => Ok(notes_body(at)),
                    None => Err(gone(params["ref"].as_str().unwrap_or_default())),
                },
                // (the selected region, else the first)
                None => match self.regions.iter().position(|region| region.selected).or((!self.regions.is_empty()).then_some(0)) {
                    Some(at) => Ok(notes_body(at)),
                    None => Err(("failed".to_string(), "no region is selected to show the notes of".to_string())),
                },
            },
            _ => return None,
        })
    }
}

impl State {
    /// The tempo row as the Tempo and Signature Lists read now (`recorded`, the row the vectors hold): the project's tempo,
    /// (M6) every event of the Tempo List by position (the first the project's tempo at its start), and (M4) the
    /// Signature List as changed.
    fn tempo_row(&self, recorded: &Value, signature_text: &str) -> Value {
        let mut row = recorded.clone();
        row["bpm"] = self.tempo.into();
        if row.get("tempos").is_some() {
            let mut tempos = vec![json!({"bpm": self.tempo, "position": position_of(0.0)})];
            tempos.extend(self.tempo_changes.iter().map(|(at, bpm)| json!({"bpm": bpm, "position": position_of(*at)})));
            row["tempos"] = Value::Array(tempos);
        }
        if self.signature != (4, 4) || !self.signature_changes.is_empty() {
            let key = row["signatures"][0]["key"].clone();
            let mut signatures = vec![json!({"key": key, "position": position_of(0.0), "time": signature_text})];
            signatures.extend(
                self.signature_changes
                    .iter()
                    .map(|(at, (num, den))| json!({"key": key, "position": position_of(*at), "time": format!("{num}/{den}")})),
            );
            row["signatures"] = Value::Array(signatures);
        }
        row
    }

    /// (M6) File > Open Recent's items, most recent first, as the bridge reads them (`recent.rs`): the project open now
    /// (its path from its window), Scratch when another is open, a project Spotlight found once by its name, and one it
    /// found under two folders (so no path).
    fn recent_rows(&self) -> Vec<Value> {
        let open = self.project_path();
        let name_of = |path: &str| path.rsplit('/').next().unwrap_or(path).trim_end_matches(".logicx").to_string();
        let mut rows = vec![json!({"name": name_of(&open), "path": open, "open": true})];
        let recorded = project().path.unwrap_or_default();
        if recorded != open {
            rows.push(json!({"name": name_of(&recorded), "path": recorded, "open": false}));
        }
        rows.push(json!({"name": "Night Drive", "path": RECENT_FOUND, "open": false}));
        rows.push(json!({"name": "Old Demo", "path": null, "open": false}));
        rows.into_iter()
            .enumerate()
            .map(|(at, mut row)| {
                row["index"] = (at + 1).into();
                alphabetical(row)
            })
            .collect()
    }

    /// (M6) `snapshot {rows, limit?}`: every row of the kinds asked (of the bridge's `BULK_KINDS`, each once, in the
    /// order asked), each kind's exactly what `read` pages for it, with its total; a kind past `limit` (10000 at most) is
    /// cut there and named under `truncated`.
    fn bulk(&self, asked: &Value, signature_text: &str) -> Answer {
        let kinds = asked["rows"]
            .as_array()
            .filter(|kinds| !kinds.is_empty())
            .ok_or_else(|| bad(format!("rows: a list of the kinds to read whole, of {}", BULK_KINDS.join(", "))))?;
        let mut wanted: Vec<&str> = vec![];
        for kind in kinds {
            let name = kind.as_str().unwrap_or_default();
            let Some(known) = BULK_KINDS.iter().find(|known| **known == name) else {
                let parent = if ["note", "param", "smart", "automation"].contains(&name) { ", under its parent" } else { "" };
                return Err(bad(format!(
                    "rows: Logic Pro's bulk read takes {}; {} is read with read{parent}",
                    BULK_KINDS.join(", "),
                    if name.is_empty() { kind.to_string() } else { name.to_string() }
                )));
            };
            if !wanted.contains(known) {
                wanted.push(known);
            }
        }
        let most = asked.get("limit").and_then(Value::as_u64).map_or(MOST_BULK, |n| (n as usize).clamp(1, MOST_BULK));
        let recorded_row = |kind: &str| -> Result<Value, Fault> {
            find(&self.cases, "read", &json!({"kind": kind}))
                .and_then(|case| case.response.as_ref().ok())
                .map(|page| page["items"][0].clone())
                .ok_or_else(|| bad(format!("the simulator has no {kind} row recorded")))
        };
        let (mut rows, mut totals, mut truncated) = (serde_json::Map::new(), serde_json::Map::new(), vec![]);
        for kind in wanted {
            let all: Vec<Value> = match kind {
                "project" => {
                    let mut row = recorded_row("project")?;
                    row["tempo"] = self.tempo.into();
                    row["trackCount"] = self.tracks.len().into();
                    row["timeSignature"] = signature_text.into();
                    if row.get("middleC").is_some() {
                        row["middleC"] = self.middle_c.into();
                    }
                    vec![row]
                }
                "track" => self.tracks.iter().enumerate().map(|(at, track)| track.row(at + 1)).collect(),
                "region" => self.region_rows(),
                "plugin" => self.plugin_rows(),
                "send" => self.send_rows(),
                "marker" => self.markers.iter().enumerate().map(|(at, marker)| marker.row(at + 1)).collect(),
                _ => vec![self.tempo_row(&recorded_row("tempo")?, signature_text)],
            };
            totals.insert(kind.into(), all.len().into());
            if all.len() > most {
                truncated.push(Value::from(kind));
            }
            rows.insert(kind.into(), Value::Array(all.into_iter().take(most).collect()));
        }
        let mut answer = json!({"epoch": RECORDED_EPOCH.to_string(), "rows": rows, "totals": totals});
        if !truncated.is_empty() {
            answer["truncated"] = Value::Array(truncated);
        }
        Ok(answer)
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
        state.region_places.clear();
        state.keep_places();
    }

    /// The project open now, as the bridge's `status` would name it: the recorded one, the copy `open_project` opened,
    /// or (M4) the package Kumi saved it as.
    pub fn project(&self) -> Project {
        let state = self.state.lock().unwrap();
        let project = another_project(&serde_json::to_value(project()).unwrap(), state.project);
        let project = match &state.saved_as {
            Some(path) => saved_project(&project, path),
            None => project,
        };
        serde_json::from_value(project).unwrap()
    }

    /// (M4) The package the project was last saved to by Kumi (a save as), if it was.
    pub fn saved_as(&self) -> Option<String> {
        self.state.lock().unwrap().saved_as.clone()
    }

    /// (M4) The regions in the Tracks area's order: each name, its track's place (from 1), its start and its end.
    pub fn regions(&self) -> Vec<(String, usize, f64, f64)> {
        let state = self.state.lock().unwrap();
        state
            .regions
            .iter()
            .map(|region| (region.name.clone(), state.track_index(region.track).map_or(0, |at| at + 1), region.start, region.end))
            .collect()
    }

    /// (M4) How many notes the region at `place` (from 1) holds.
    pub fn notes_of(&self, place: usize) -> usize {
        self.state.lock().unwrap().regions.get(place - 1).map_or(0, |region| region.notes.len())
    }

    /// (M6) Whether the control bar's Metronome Click is on (it is, as recorded).
    pub fn metronome(&self) -> bool {
        self.state.lock().unwrap().metronome
    }

    /// (M6) The producer turns the metronome on or off in Logic, behind Kumi's back.
    pub fn set_metronome(&self, on: bool) {
        self.state.lock().unwrap().metronome = on;
    }

    /// (M6) The producer chooses the octave Logic names middle C in (Settings > View > General, "Display Middle C As":
    /// 3 for C3, 4 for C4): the project row says it from the next read.
    pub fn set_middle_c(&self, octave: i64) {
        self.state.lock().unwrap().middle_c = octave;
    }

    /// (M4) The cycle: whether it is on, and the locators in beats (off, bar 1 to bar 5, as recorded).
    pub fn cycle(&self) -> (bool, f64, f64) {
        self.state.lock().unwrap().cycle
    }

    /// (M4) The Tempo List: each change's beat and BPM, the project's at 0 first.
    pub fn tempos(&self) -> Vec<(f64, f64)> {
        let state = self.state.lock().unwrap();
        std::iter::once((0.0, state.tempo)).chain(state.tempo_changes.iter().copied()).collect()
    }

    /// (M4) The Signature List: each change's beat and signature, the project's at 0 first.
    pub fn signatures(&self) -> Vec<(f64, (i64, i64))> {
        let state = self.state.lock().unwrap();
        std::iter::once((0.0, state.signature)).chain(state.signature_changes.iter().copied()).collect()
    }

    /// (M4b) The plug-ins in the slots of a strip, `track:2` or `output:1` (the Stereo Out), the instrument first: each
    /// name and whether it is bypassed.
    pub fn plugins(&self, strip: &str) -> Vec<(String, bool)> {
        let state = self.state.lock().unwrap();
        state
            .strip_named(strip)
            .map(|owner| state.channel(owner).plugins.iter().map(|plugin| (plugin.name.clone(), plugin.bypassed)).collect())
            .unwrap_or_default()
    }

    /// (M4b) A row of the Controls View of the plug-in in `slot` (from 1) of a strip, by its name: its raw value (none
    /// for a menu) and its text.
    pub fn param(&self, strip: &str, slot: usize, name: &str) -> Option<(Option<f64>, String)> {
        let state = self.state.lock().unwrap();
        let owner = state.strip_named(strip)?;
        state.channel(owner).plugins.get(slot.checked_sub(1)?)?.row_named(name)
    }

    /// (M4b) The setting the window of the plug-in in `slot` of a strip names.
    pub fn preset(&self, strip: &str, slot: usize) -> Option<String> {
        let state = self.state.lock().unwrap();
        let owner = state.strip_named(strip)?;
        state.channel(owner).plugins.get(slot.checked_sub(1)?).map(|plugin| plugin.preset.clone())
    }

    /// (M4b) The sends of the track at `index` (from 1): each destination and level in dB (−∞ as -96).
    pub fn sends(&self, index: usize) -> Vec<(String, f64)> {
        let state = self.state.lock().unwrap();
        state
            .tracks
            .get(index.wrapping_sub(1))
            .map(|track| track.channel.sends.iter().map(|send| (format!("Bus {}", send.bus), send.level)).collect())
            .unwrap_or_default()
    }

    /// (M4b) The auxes Logic made for a bus nothing took, by name, in the order it made them.
    pub fn auxes(&self) -> Vec<String> {
        self.state.lock().unwrap().auxes.iter().map(|aux| aux.name.clone()).collect()
    }

    /// (M4b) The patch the track at `index` (from 1) plays, as its strip's setting button names it (none when it names
    /// none), and its Smart Controls: each name and value (0 to 1).
    pub fn patch(&self, index: usize) -> Option<String> {
        let state = self.state.lock().unwrap();
        Some(state.tracks.get(index.checked_sub(1)?)?.channel.patch.clone()).filter(|patch| !patch.is_empty())
    }

    pub fn smart(&self, index: usize) -> Vec<(String, f64)> {
        let state = self.state.lock().unwrap();
        state
            .tracks
            .get(index.wrapping_sub(1))
            .map(|track| track.smart.iter().map(|smart| (smart.name.clone(), smart.value)).collect())
            .unwrap_or_default()
    }

    /// (M4b) The automation of `parameter` on the track at `index` (from 1): each event's position in beats and value
    /// (0 to 127).
    pub fn automation(&self, index: usize, parameter: &str) -> Vec<(f64, u32)> {
        let state = self.state.lock().unwrap();
        state
            .tracks
            .get(index.wrapping_sub(1))
            .map(|track| {
                track.automation.iter().filter(|event| event.parameter == parameter).map(|event| (event.beats, event.value)).collect()
            })
            .unwrap_or_default()
    }

    /// (M4) The next restore alone is made, and Logic doesn't answer after it (REAPER's simulator's switch).
    pub fn uncertain_restore_once(&self) {
        self.state.lock().unwrap().uncertain_restore_once = true;
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
        self.state.lock().unwrap().markers.iter().enumerate().map(|(at, m)| (m.name_at(at + 1), m.position_beats)).collect()
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
        self.state.lock().unwrap().undo.iter().map(|step| step.desc.clone().unwrap_or_default()).collect()
    }

    /// The same, as Logic's Edit menu titles them.
    pub fn undo_titles(&self) -> Vec<String> {
        self.state.lock().unwrap().undo.iter().map(|step| step.title.clone()).collect()
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
    /// test's business), and the bridge names it to the host by the `desc` it kept (`undone_in_daw`): Logic's title
    /// for the step, and that desc, if the bridge had one.
    pub fn undo_last(&self) -> Option<(String, Option<String>)> {
        self.state.lock().unwrap().undo.pop().map(|step| (step.title, step.desc))
    }

    pub fn respond(&self) -> Respond {
        let script = self.clone();
        Arc::new(move |method, params| script.answer(method, params))
    }

    /// The simulator's config for Logic with this project behind it: the reads, and Kumi's changes, in the bridge's order.
    pub fn config(&self, secret: Secret, resource: &str) -> SimulatorConfig {
        SimulatorConfig::logic(secret, resource)
            .with_methods(&METHODS, self.respond())
            .with_methods(&CHANGE_METHODS, self.respond())
            .with_methods(&LISTEN_METHODS, self.respond())
    }

    fn answer(&self, method: &str, params: &Value) -> Option<Answer> {
        if !METHODS.contains(&method) && !CHANGE_METHODS.contains(&method) && !LISTEN_METHODS.contains(&method) {
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
            // (M4) Saving: the package written, nothing in the project changed; (M4b) a bounce and an export alike.
            "project_save" => return Some(state.project_save(params)),
            "bounce" => return Some(state.bounce(params)),
            "export_tracks" => return Some(state.export_tracks(params)),
            // (M5) A part heard, auditioned or bounced: files written, nothing in the project changed.
            "hear" => return Some(state.hear(params)),
            "audition" => return Some(state.audition(params)),
            "render" => return Some(state.render(params)),
            _ => {}
        }
        let (epoch, project, saved_as) = (state.epoch, state.project, state.saved_as.clone());
        // Past the recorded epoch, a ref or cursor from an earlier one is refused before anything is looked up, as the
        // bridge refuses it whatever it was for; in the recorded epoch the refusals recorded answer.
        if epoch != RECORDED_EPOCH {
            if let Some(stale) = stale_in(params, epoch) {
                return Some(Err(stale));
            }
        }
        let answered = |result: &Value| {
            let answer = another_project(&retag(result, RECORDED_EPOCH, epoch), project);
            match &saved_as {
                Some(path) => saved_project(&answer, path),
                None => answer,
            }
        };
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
        // (M4) The notes are the region's, paged as a read pages them, as recorded when the request was.
        let notes = answer("read", json!({"kind": "note", "parent": "1:region:1", "limit": 3})).unwrap();
        assert_eq!(
            (notes["items"].as_array().map(Vec::len), notes["nextCursor"].as_str(), notes["total"].as_u64()),
            (Some(3), Some("1.3"), Some(4))
        );
        assert!(answer("read", json!({"kind": "note", "parent": "1:region:9"})).unwrap_err().1.contains("isn't in the project"));
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
        // (named, it is renamed through the Inspector once made: "Renaming" is on top of "Create Track", as the M3 scene reads)
        assert_eq!(added["undo"], json!({"desc": "Kumi: Added audio track “Pad” (k0002)", "entry": true, "title": "Undo Renaming"}));
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
        assert_eq!(script.undo_titles(), ["Undo Create Track", "Undo Renaming", "Undo Delete Tracks"]);
        assert_eq!(
            script.undo_history(),
            ["", "Kumi: Added audio track “Pad” (k0002)", ""],
            "the change is kept under its last step; a restore's step the bridge doesn't name"
        );
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
