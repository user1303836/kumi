//! The plugin host's guarded undo (`HostPolicy::guards_undo`): what Live read back of each value Kumi set with
//! `set_mixer`, `set_tempo` or a device's parameters, kept by change, and the check an undo makes of each right before
//! Live is asked, refusing (`changed_since`) when one changed since. Nothing is kept for standalone Kumi.

use super::connection::LiveConnection;
use super::fast::read_script;
use super::history::{Applied, FastResult, History, LiveHistory};
use indexmap::IndexMap;
use kumi_agent::core::contracts::{UndoPreview, UndoRefusal};
use kumi_agent::core::history::UndoResult;
use kumi_common::abort::{self, Signal};
use kumi_common::js::number::to_string;
use kumi_types::contracts::JsonObject;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};

/// The changes whose undo is guarded: each sets values Live reads back.
pub const GUARDED: [&str; 4] = ["set_mixer", "set_tempo", "set_device_parameter", "set_device_parameters"];
/// The most changes whose values are kept. Past it the oldest go first, and their undo is refused, as one with nothing
/// kept is.
pub const KEPT: usize = 256;
/// The mixer's fields a change names, besides its sends.
const MIXER: [&str; 5] = ["volume", "pan", "cueVolume", "mute", "solo"];
/// The kinds a track's mixer is discovered by: a ref doesn't say which (refs are positional).
const TRACK_KINDS: [&str; 3] = ["track", "return-track", "main-track"];

/// Where a value Kumi set is.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    Tempo,
    /// A track's mixer: one of [`MIXER`], or its send at `send`.
    Mixer {
        track: String,
        field: String,
        send: Option<usize>,
    },
    Parameter {
        device: String,
        parameter: String,
    },
    /// A parameter the fast path set, as its revert script finds it: `{"ref"}`, or `{"device","index","name"}`.
    Fast(Value),
}

/// One value Kumi set: where, what was there before, and what Live read back once it was set.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub place: Place,
    /// What it's called, for what an undo says ("volume", "send B", "Filter Cutoff", "tempo").
    pub name: String,
    pub prior: Value,
    pub applied: Value,
    /// What Live showed for `prior`.
    pub prior_shown: Option<String>,
}

/// What a change Kumi set values with kept, by change id (the newest [`KEPT`]); and a guarded undo's refusal, for the
/// integration to answer with.
#[derive(Default)]
pub struct Guard {
    /// Whether undo is guarded here (the plugin host's policy): nothing is kept otherwise.
    pub on: Cell<bool>,
    /// None for a change whose values Live couldn't read back once set: its undo is refused.
    kept: RefCell<IndexMap<String, Option<Vec<Target>>>>,
    refused: RefCell<Option<UndoRefusal>>,
}

impl Guard {
    /// Keeps what the change `id` set, read back (None: it couldn't be).
    pub fn keep(&self, id: &str, targets: Option<Vec<Target>>) {
        if !self.on.get() {
            return;
        }
        let mut kept = self.kept.borrow_mut();
        kept.insert(id.to_owned(), targets);
        while kept.len() > KEPT {
            kept.shift_remove_index(0);
        }
    }
    /// What the change `id` set, read back: None when nothing was kept (before the guard, or past [`KEPT`]).
    pub fn of(&self, id: &str) -> Option<Option<Vec<Target>>> {
        self.kept.borrow().get(id).cloned()
    }
    /// The change `id` is undone: nothing is left to check.
    pub fn forget(&self, id: &str) {
        self.kept.borrow_mut().shift_remove(id);
    }
    pub fn refuse(&self, reason: String) {
        *self.refused.borrow_mut() = Some(UndoRefusal { code: "changed_since", reason });
    }
    pub fn take_refusal(&self) -> Option<UndoRefusal> {
        self.refused.borrow_mut().take()
    }
}

/// Numbers within Live's own tolerance (the bridge's `same_live_value`: float32 readback and Live's whole-number
/// normalization, never a hand's move); anything else exactly.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * 1f64.max(a.abs()).max(b.abs()),
        _ => a == b,
    }
}

/// Live's words for a value, on one line and at most 32 characters.
fn shown(text: &str) -> String {
    let words = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    match words.char_indices().nth(32) {
        Some((at, _)) => format!("{}…", &words[..at]),
        None => words,
    }
}

/// A value in words when Live gave none: a tempo's BPM, a switch's on or off, a number.
fn words(place: &Place, value: &Value) -> String {
    match (place, value) {
        (Place::Tempo, Value::Number(n)) => format!("{} BPM", to_string(n.as_f64().unwrap_or_default())),
        (_, Value::Bool(on)) => (if *on { "on" } else { "off" }).into(),
        (_, Value::Number(n)) => to_string(n.as_f64().unwrap_or_default()),
        _ => "?".into(),
    }
}

/// The send's letter, as Live names return tracks: A, B, …
fn send_name(index: usize) -> String {
    let letter = (b'A' + (index % 26) as u8) as char;
    format!("send {letter}")
}

/// What a guarded change set, as its preview and apply answered and Live reads back once it's set: None when Live
/// can't be read, so the undo is refused rather than blind.
pub async fn applied(
    connection: &LiveConnection,
    tool: &str,
    args: &JsonObject,
    preview: &JsonObject,
    result: &JsonObject,
    signal: &Signal,
) -> Option<Vec<Target>> {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(shown);
    match tool {
        "set_tempo" => {
            let prior = preview.get("priorTempo").filter(|v| v.is_number())?.clone();
            let applied = match result.get("tempo").filter(|v| v.is_number()) {
                Some(tempo) => tempo.clone(),
                None => tempo(connection, signal).await?,
            };
            Some(vec![Target { place: Place::Tempo, name: "tempo".into(), prior, applied, prior_shown: None }])
        }
        "set_device_parameter" => {
            let parameter = preview.get("parameter")?;
            let place = Place::Parameter {
                device: args.get("deviceRef")?.as_str()?.to_owned(),
                parameter: args.get("parameterRef")?.as_str()?.to_owned(),
            };
            Some(vec![Target {
                place,
                name: parameter.get("name").and_then(Value::as_str).map(shown).unwrap_or_else(|| "a parameter".into()),
                prior: parameter.get("currentValue").filter(|v| v.is_number())?.clone(),
                applied: result.get("value").filter(|v| v.is_number())?.clone(),
                prior_shown: text(parameter.get("displayValue")),
            }])
        }
        "set_device_parameters" => {
            // The apply answers what it asked for: what Live keeps is read back.
            let device = args.get("deviceRef")?.as_str()?.to_owned();
            let now = parameters(connection, &device, signal).await?;
            let mut targets = Vec::new();
            for row in preview.get("parameters")?.as_array()? {
                let parameter = row.get("ref")?.as_str()?.to_owned();
                let (applied, _) = now.get(&parameter)?.clone();
                targets.push(Target {
                    place: Place::Parameter { device: device.clone(), parameter },
                    name: row.get("name").and_then(Value::as_str).map(shown).unwrap_or_else(|| "a parameter".into()),
                    prior: row.get("currentValue").filter(|v| v.is_number())?.clone(),
                    applied,
                    prior_shown: text(row.get("displayValue")),
                });
            }
            Some(targets)
        }
        "set_mixer" => {
            // The apply answers in words: what Live keeps is read back.
            let track = args.get("trackRef")?.as_str()?.to_owned();
            let mixer = mixer(connection, &track, signal).await?;
            let (prior, prior_shown) = (preview.get("prior")?, preview.get("priorDisplay"));
            let mut targets = Vec::new();
            for field in MIXER.iter().filter(|field| args.contains_key(**field)) {
                targets.push(Target {
                    place: Place::Mixer { track: track.clone(), field: (*field).into(), send: None },
                    name: if *field == "cueVolume" { "cue volume".into() } else { (*field).into() },
                    prior: prior.get(*field).filter(|v| !v.is_null())?.clone(),
                    applied: mixer.get(*field).filter(|v| !v.is_null())?.clone(),
                    prior_shown: text(prior_shown.and_then(|shown| shown.get(*field))),
                });
            }
            // Only the sends the change named: the rest it left.
            let named = args.get("sends").and_then(Value::as_array).map_or(0, Vec::len);
            for send in 0..named {
                targets.push(Target {
                    place: Place::Mixer { track: track.clone(), field: "sends".into(), send: Some(send) },
                    name: send_name(send),
                    prior: prior.get("sends")?.get(send).filter(|v| v.is_number())?.clone(),
                    applied: mixer.get("sends")?.get(send).filter(|v| v.is_number())?.clone(),
                    prior_shown: text(prior_shown.and_then(|shown| shown.get("sends")?.get(send))),
                });
            }
            Some(targets)
        }
        _ => None,
    }
}

/// What the fast path set: each target as its revert script finds it, with the rows Live read as it set them (`prior`,
/// `priorDisplay`, and `value`, read once every set was made).
pub fn fast(targets: &[JsonObject], rows: &[JsonObject]) -> Option<Vec<Target>> {
    targets
        .iter()
        .zip(rows)
        .map(|(target, row)| {
            let place = match target.get("ref") {
                Some(reference) => json!({"ref": reference}),
                None => json!({"device": target.get("device")?, "index": target.get("index")?, "name": row.get("name")?}),
            };
            Some(Target {
                place: Place::Fast(place),
                name: row.get("name").and_then(Value::as_str).map(shown).unwrap_or_else(|| "a parameter".into()),
                prior: row.get("prior").filter(|v| v.is_number())?.clone(),
                applied: row.get("value").filter(|v| v.is_number())?.clone(),
                prior_shown: row.get("priorDisplay").and_then(Value::as_str).map(shown),
            })
        })
        .collect::<Option<Vec<_>>>()
        .filter(|targets| targets.len() == rows.len() && !targets.is_empty())
}

async fn tempo(connection: &LiveConnection, signal: &Signal) -> Option<Value> {
    let rows = connection.rows("set", object(json!({"fields": ["tempo"], "limit": 1})), signal.clone()).await.ok()?;
    rows.first()?.get("tempo").filter(|v| v.is_number()).cloned()
}

/// A track's mixer as Live has it now, whichever kind of track it is.
async fn mixer(connection: &LiveConnection, track: &str, signal: &Signal) -> Option<JsonObject> {
    for kind in TRACK_KINDS {
        let asked = object(json!({"filter": {"ref": track}, "fields": ["mixer"], "limit": 1}));
        let rows = connection.rows(kind, asked, signal.clone()).await.ok()?;
        // (rows come back with the turn's short refs)
        if let Some(mixer) = rows.into_iter().find(|row| long(connection, row.get("ref")).as_deref() == Some(track)) {
            return mixer.get("mixer").and_then(Value::as_object).cloned();
        }
    }
    None
}

/// A device's parameters as Live has them now, by ref: each value and what Live shows for it.
async fn parameters(connection: &LiveConnection, device: &str, signal: &Signal) -> Option<IndexMap<String, (Value, Option<String>)>> {
    let asked = object(json!({"parent": device, "fields": ["value", "displayValue"]}));
    let rows = connection.rows("parameter", asked, signal.clone()).await.ok()?;
    Some(
        rows.into_iter()
            .filter_map(|row| {
                let reference = long(connection, row.get("ref"))?;
                let value = row.get("value").filter(|v| v.is_number())?.clone();
                Some((reference, (value, row.get("displayValue").and_then(Value::as_str).map(shown))))
            })
            .collect(),
    )
}

/// A row's ref as Kumi keeps it (long), from the turn's short one discovery gives.
fn long(connection: &LiveConnection, reference: Option<&Value>) -> Option<String> {
    let row = connection.references.borrow().lengthen(&json!({"ref": reference?}));
    row["ref"].as_str().map(str::to_owned)
}

/// Each target as Live has it now, read fresh: its value and Live's words for it, or None for one that can't be read.
async fn now(history: &History, targets: &[Target], signal: &Signal) -> Vec<Option<(Value, Option<String>)>> {
    let connection = &history.host.connection;
    let mut out = vec![None; targets.len()];
    let fast: Vec<usize> = (0..targets.len()).filter(|at| matches!(targets[*at].place, Place::Fast(_))).collect();
    if !fast.is_empty() {
        let items: Vec<Value> =
            fast.iter().map(|at| if let Place::Fast(place) = &targets[*at].place { place.clone() } else { Value::Null }).collect();
        if let Ok(FastResult::Result(read)) = history.run_fast(read_script(&Value::Array(items)), signal.clone()).await {
            for (at, row) in fast.iter().zip(read.as_array().into_iter().flatten()) {
                out[*at] = row
                    .get("value")
                    .filter(|v| v.is_number())
                    .map(|value| (value.clone(), row.get("display").and_then(Value::as_str).map(shown)));
            }
        }
    }
    let mut mixers: IndexMap<String, Option<JsonObject>> = IndexMap::new();
    let mut devices: IndexMap<String, Option<IndexMap<String, (Value, Option<String>)>>> = IndexMap::new();
    let mut set = None;
    for (at, target) in targets.iter().enumerate() {
        out[at] = match &target.place {
            Place::Fast(_) => continue,
            Place::Tempo => {
                if set.is_none() {
                    set = Some(tempo(connection, signal).await);
                }
                set.clone().flatten().map(|tempo| {
                    let words = words(&Place::Tempo, &tempo);
                    (tempo, Some(words))
                })
            }
            Place::Mixer { track, field, send } => {
                if !mixers.contains_key(track) {
                    let read = mixer(connection, track, signal).await;
                    mixers.insert(track.clone(), read);
                }
                mixers[track].as_ref().and_then(|mixer| {
                    let (value, words) = match send {
                        Some(send) => (mixer.get("sends")?.get(*send)?, mixer.get("sendDisplays").and_then(|d| d.get(*send))),
                        None => (mixer.get(field.as_str())?, mixer.get(&format!("{field}Display"))),
                    };
                    (!value.is_null()).then(|| (value.clone(), words.and_then(Value::as_str).map(shown)))
                })
            }
            Place::Parameter { device, parameter } => {
                if !devices.contains_key(device) {
                    let read = parameters(connection, device, signal).await;
                    devices.insert(device.clone(), read);
                }
                devices[device].as_ref().and_then(|parameters| parameters.get(parameter).cloned())
            }
        };
    }
    out
}

/// What undoing `targets` would do now, read fresh: "volume -4.0 dB → -6.0 dB; pan 20L → C", and why it won't when
/// one changed since Kumi set it or can't be read. Live's words go in what's said; the values themselves are compared.
pub async fn check(history: &History, targets: &[Target], signal: &Signal) -> (String, Option<String>) {
    let read = now(history, targets, signal).await;
    let mut said = Vec::new();
    let (mut changed, mut unread) = (Vec::new(), Vec::new());
    for (target, now) in targets.iter().zip(&read) {
        let back = target.prior_shown.clone().unwrap_or_else(|| words(&target.place, &target.prior));
        match now {
            Some((value, shown_now)) => {
                said.push(format!("{} {} → {back}", target.name, shown_now.clone().unwrap_or_else(|| words(&target.place, value))));
                if !same(value, &target.applied) {
                    changed.push(target.name.clone());
                }
            }
            None => {
                said.push(format!("{} → {back}", target.name));
                unread.push(target.name.clone());
            }
        }
    }
    let why = if !changed.is_empty() {
        let them = if changed.len() == 1 { "it" } else { "them" };
        Some(format!("{} changed in Live since Kumi set {them}, so Kumi left the change as it is.", changed.join(", ")))
    } else if !unread.is_empty() {
        Some(format!("Kumi couldn't read {} in Live now, so it left the change as it is rather than undo it blind.", unread.join(", ")))
    } else {
        None
    };
    (said.join("; "), why)
}

/// Whether the change in `entry` is guarded: one of [`GUARDED`]'s, on the plugin host.
pub fn guarded(history: &History, entry: &Applied) -> bool {
    history.host.guard.on.get() && entry.tool.as_deref().is_some_and(|tool| GUARDED.contains(&tool))
}

/// Why the guarded change in `entry` mustn't be undone now, read fresh: a value changed since Kumi set it or can't be
/// read, nothing was kept of it, or Live's Set moved on while Kumi read. None: it may.
pub async fn refusal(history: &History, entry: &Applied, signal: &Signal) -> Option<String> {
    let connection = &history.host.connection;
    let (lease, epoch) = (connection.lease.get(), connection.epoch.get());
    let Some(targets) = history.host.guard.of(&entry.record.id).flatten() else {
        return Some(nothing_kept());
    };
    let (_, why) = check(history, &targets, signal).await;
    why.or_else(|| {
        (connection.lease.get() != lease || connection.epoch.get() != epoch)
            .then(|| "Live's Set changed while Kumi read it, so Kumi left the change as it is; look again.".to_owned())
    })
}

/// The guard's last word right before Live is asked to undo `entry`: when it refuses, an undo result that changes
/// nothing (and the refusal, kept for the integration to answer with). Nothing awaits between it and Live's undo.
pub async fn before_undo(history: &History, entry: &Applied) -> Option<UndoResult> {
    if !guarded(history, entry) {
        return None;
    }
    let connection = &history.host.connection;
    let signal = abort::any([connection.lifetime.clone(), abort::timeout(10_000)]);
    let why = refusal(history, entry, &signal).await?;
    history.host.guard.refuse(why.clone());
    Some(UndoResult::error(why))
}

/// Why a guarded change whose values weren't kept isn't undone.
pub fn nothing_kept() -> String {
    "Kumi didn't keep what this change set as Live read it back (it was made before Kumi checked undos, Live couldn't be \
     read once it was set, or it's past the most Kumi keeps), so it left the change as it is rather than undo it blind."
        .into()
}

/// The preview of a guarded undo: each value now and as the undo would put it, with the refusal it would meet.
pub async fn preview(history: &History, id: &str, title: &str, signal: &Signal) -> UndoPreview {
    let refusal = |reason: String| Some(UndoRefusal { code: "changed_since", reason });
    match history.host.guard.of(id).flatten() {
        Some(targets) => {
            let (said, why) = check(history, &targets, signal).await;
            UndoPreview { text: format!("Undo {said}"), refusal: why.and_then(refusal) }
        }
        None => UndoPreview { text: format!("Undo {}", shown(title)), refusal: refusal(nothing_kept()) },
    }
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}
