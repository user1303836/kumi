//! The plugin host's guarded undo (`HostPolicy::guards_undo`): what Live read back of each value Kumi set with
//! `set_mixer`, `set_tempo` or a device's parameters, kept by change, and the check an undo makes of each right before
//! Live is asked, refusing (`changed_since`) when one changed since. Nothing is kept for standalone Kumi.
//!
//! Live's refs are places: Kumi's own restructures move what they name. The fast path's targets are its change's revert
//! items, which HISTORY moves with every restructure; the bridge path's carry the object's identity as Live read it
//! once set, and a place that now holds another object is left to Live's own undo, which says so.

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
use std::collections::HashSet;

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
    /// A parameter the fast path set: its change's revert item at this index, as HISTORY keeps it (moved with Kumi's own
    /// restructures).
    Fast(usize),
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
    /// Live's identity for the object at the place once it was set (a track's, a parameter's): another there now means
    /// the place moved, not the value.
    pub identity: Option<String>,
}

/// What a change Kumi set values with kept, by change id (the newest [`KEPT`]); a guarded undo's refusal, for the
/// integration to answer with; and the guarded undo's own signal, for the reads the check makes.
#[derive(Default)]
pub struct Guard {
    /// Whether undo is guarded here (the plugin host's policy): nothing is kept otherwise.
    pub on: Cell<bool>,
    /// None for a change whose values Live couldn't read back once set: its undo is refused.
    kept: RefCell<IndexMap<String, Option<Vec<Target>>>>,
    refused: RefCell<Option<UndoRefusal>>,
    asked: RefCell<Option<Signal>>,
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
    /// The guarded undo under way: its signal, which the check's reads follow (None once it's done).
    pub fn asked(&self, signal: Option<Signal>) {
        *self.asked.borrow_mut() = signal;
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

/// A value as Live has it now: the value, Live's words for it, and the identity of what holds it.
#[derive(Debug, Clone)]
struct Now {
    value: Value,
    shown: Option<String>,
    identity: Option<String>,
}

/// What a guarded change set, as its preview and apply answered and Live reads back once it's set: None when Live
/// can't be read, or the read isn't what the apply verified (a hand on it already), so the undo is refused rather
/// than blind.
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
                None => tempo(connection, signal).await?.value,
            };
            Some(vec![Target { place: Place::Tempo, name: "tempo".into(), prior, applied, prior_shown: None, identity: None }])
        }
        "set_device_parameter" => {
            let parameter = preview.get("parameter")?;
            let (device, reference) = (args.get("deviceRef")?.as_str()?.to_owned(), args.get("parameterRef")?.as_str()?.to_owned());
            // The apply's own readback is the value; the read after it gives what holds it.
            let applied = result.get("value").filter(|v| v.is_number())?.clone();
            let now = parameter_now(connection, &device, &reference, signal).await?;
            if !same(&now.value, &applied) {
                return None;
            }
            Some(vec![Target {
                place: Place::Parameter { device, parameter: reference },
                name: parameter.get("name").and_then(Value::as_str).map(shown).unwrap_or_else(|| "a parameter".into()),
                prior: parameter.get("currentValue").filter(|v| v.is_number())?.clone(),
                applied,
                prior_shown: text(parameter.get("displayValue")),
                identity: now.identity,
            }])
        }
        "set_device_parameters" => {
            // The apply answers what it asked for (normalized as Live keeps it): what Live keeps is read back, and
            // must be that.
            let device = args.get("deviceRef")?.as_str()?.to_owned();
            let asked: Vec<Value> = result.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut targets = Vec::new();
            for row in preview.get("parameters")?.as_array()? {
                let reference = row.get("ref")?.as_str()?.to_owned();
                let now = parameter_now(connection, &device, &reference, signal).await?;
                let proposed = asked.iter().find(|given| given.get("ref").and_then(Value::as_str) == Some(&reference));
                if proposed.and_then(|given| given.get("value")).is_some_and(|value| !same(&now.value, value)) {
                    return None;
                }
                targets.push(Target {
                    place: Place::Parameter { device: device.clone(), parameter: reference },
                    name: row.get("name").and_then(Value::as_str).map(shown).unwrap_or_else(|| "a parameter".into()),
                    prior: row.get("currentValue").filter(|v| v.is_number())?.clone(),
                    applied: now.value,
                    prior_shown: text(row.get("displayValue")),
                    identity: now.identity,
                });
            }
            Some(targets)
        }
        "set_mixer" => {
            // The apply answers in words: what Live keeps is read back, and must be what was asked (the bridge
            // verified it so).
            let track = args.get("trackRef")?.as_str()?.to_owned();
            let (mixer, identity) = mixer(connection, &track, signal).await?;
            let (prior, prior_shown) = (preview.get("prior")?, preview.get("priorDisplay"));
            let mut targets = Vec::new();
            for field in MIXER.iter().filter(|field| args.contains_key(**field)) {
                let applied = mixer.get(*field).filter(|v| !v.is_null())?.clone();
                if !same(&applied, args.get(*field)?) {
                    return None;
                }
                targets.push(Target {
                    place: Place::Mixer { track: track.clone(), field: (*field).into(), send: None },
                    name: if *field == "cueVolume" { "cue volume".into() } else { (*field).into() },
                    prior: prior.get(*field).filter(|v| !v.is_null())?.clone(),
                    applied,
                    prior_shown: text(prior_shown.and_then(|shown| shown.get(*field))),
                    identity: identity.clone(),
                });
            }
            // Only the sends the change named: the rest it left.
            let named = args.get("sends").and_then(Value::as_array).cloned().unwrap_or_default();
            for (send, asked) in named.iter().enumerate() {
                let applied = mixer.get("sends")?.get(send).filter(|v| v.is_number())?.clone();
                if !same(&applied, asked) {
                    return None;
                }
                targets.push(Target {
                    place: Place::Mixer { track: track.clone(), field: "sends".into(), send: Some(send) },
                    name: send_name(send),
                    prior: prior.get("sends")?.get(send).filter(|v| v.is_number())?.clone(),
                    applied,
                    prior_shown: text(prior_shown.and_then(|shown| shown.get("sends")?.get(send))),
                    identity: identity.clone(),
                });
            }
            Some(targets)
        }
        _ => None,
    }
}

/// What the fast path set: each target with the rows Live read as it set them (`prior`, `priorDisplay`, and
/// `value`, read once every set was made), by its place among the change's revert items.
pub fn fast(targets: &[JsonObject], rows: &[JsonObject]) -> Option<Vec<Target>> {
    if targets.len() != rows.len() || rows.is_empty() {
        return None;
    }
    rows.iter()
        .enumerate()
        .map(|(at, row)| {
            Some(Target {
                place: Place::Fast(at),
                name: row.get("name").and_then(Value::as_str).map(shown).unwrap_or_else(|| "a parameter".into()),
                prior: row.get("prior").filter(|v| v.is_number())?.clone(),
                applied: row.get("value").filter(|v| v.is_number())?.clone(),
                prior_shown: row.get("priorDisplay").and_then(Value::as_str).map(shown),
                identity: None,
            })
        })
        .collect()
}

async fn tempo(connection: &LiveConnection, signal: &Signal) -> Option<Now> {
    let rows = connection.rows("set", object(json!({"fields": ["tempo"], "limit": 1})), signal.clone()).await.ok()?;
    let tempo = rows.first()?.get("tempo").filter(|v| v.is_number())?.clone();
    let shown = words(&Place::Tempo, &tempo);
    Some(Now { value: tempo, shown: Some(shown), identity: None })
}

/// A row's ref as Kumi keeps it (long), from the turn's short one discovery gives.
fn long(connection: &LiveConnection, reference: Option<&Value>) -> Option<String> {
    let row = connection.references.borrow().lengthen(&json!({"ref": reference?}));
    row["ref"].as_str().map(str::to_owned)
}

fn identity(row: &JsonObject) -> Option<String> {
    row.get("objectIdentity").and_then(Value::as_str).filter(|identity| !identity.is_empty()).map(str::to_owned)
}

/// A track's mixer as Live has it now, whichever kind of track it is, and the track's identity.
async fn mixer(connection: &LiveConnection, track: &str, signal: &Signal) -> Option<(JsonObject, Option<String>)> {
    for kind in TRACK_KINDS {
        let asked = object(json!({"filter": {"ref": track}, "fields": ["mixer", "objectIdentity"], "limit": 1}));
        let rows = connection.rows(kind, asked, signal.clone()).await.ok()?;
        // (rows come back with the turn's short refs)
        if let Some(row) = rows.into_iter().find(|row| long(connection, row.get("ref")).as_deref() == Some(track)) {
            return Some((row.get("mixer").and_then(Value::as_object).cloned()?, identity(&row)));
        }
    }
    None
}

/// One of a device's parameters as Live has it now, read by its ref (a device's whole list stops at a page).
async fn parameter_now(connection: &LiveConnection, device: &str, parameter: &str, signal: &Signal) -> Option<Now> {
    let asked =
        object(json!({"parent": device, "filter": {"ref": parameter}, "fields": ["value", "displayValue", "objectIdentity"], "limit": 1}));
    let rows = connection.rows("parameter", asked, signal.clone()).await.ok()?;
    let row = rows.into_iter().find(|row| long(connection, row.get("ref")).as_deref() == Some(parameter))?;
    Some(Now {
        value: row.get("value").filter(|v| v.is_number())?.clone(),
        shown: row.get("displayValue").and_then(Value::as_str).map(shown),
        identity: identity(&row),
    })
}

/// Each target as Live has it now, read fresh (None for one that can't be read); the fast path's where HISTORY's
/// `revert` items say they are now.
async fn now(history: &History, targets: &[Target], revert: Option<&[Value]>, signal: &Signal) -> Vec<Option<Now>> {
    let connection = &history.host.connection;
    let mut out = vec![None; targets.len()];
    let fast: Vec<(usize, Value)> = targets
        .iter()
        .enumerate()
        .filter_map(|(at, target)| match target.place {
            Place::Fast(item) => Some((at, revert?.get(item)?.clone())),
            _ => None,
        })
        .collect();
    if !fast.is_empty() {
        let items: Vec<Value> = fast.iter().map(|(_, item)| item.clone()).collect();
        if let Ok(FastResult::Result(read)) = history.run_fast(read_script(&Value::Array(items)), signal.clone()).await {
            for ((at, _), row) in fast.iter().zip(read.as_array().into_iter().flatten()) {
                out[*at] = row.get("value").filter(|v| v.is_number()).map(|value| Now {
                    value: value.clone(),
                    shown: row.get("display").and_then(Value::as_str).map(shown),
                    identity: None,
                });
            }
        }
    }
    let mut mixers: IndexMap<String, Option<(JsonObject, Option<String>)>> = IndexMap::new();
    let mut set = None;
    for (at, target) in targets.iter().enumerate() {
        out[at] = match &target.place {
            Place::Fast(_) => continue,
            Place::Tempo => {
                if set.is_none() {
                    set = Some(tempo(connection, signal).await);
                }
                set.clone().flatten()
            }
            Place::Mixer { track, field, send } => {
                if !mixers.contains_key(track) {
                    let read = mixer(connection, track, signal).await;
                    mixers.insert(track.clone(), read);
                }
                mixers[track].as_ref().and_then(|(mixer, identity)| {
                    let (value, words) = match send {
                        Some(send) => (mixer.get("sends")?.get(*send)?, mixer.get("sendDisplays").and_then(|d| d.get(*send))),
                        None => (mixer.get(field.as_str())?, mixer.get(&format!("{field}Display"))),
                    };
                    (!value.is_null()).then(|| Now {
                        value: value.clone(),
                        shown: words.and_then(Value::as_str).map(shown),
                        identity: identity.clone(),
                    })
                })
            }
            Place::Parameter { device, parameter } => parameter_now(connection, device, parameter, signal).await,
        };
    }
    out
}

/// What the check found: what undoing the targets would do now ("volume -4.0 dB → -6.0 dB; pan 20L → C"), why it
/// won't (a value changed since Kumi set it, or can't be read), and whether a place now holds another object, which is
/// left to Live's own undo to say.
struct Checked {
    said: String,
    why: Option<String>,
    moved: Vec<String>,
}

async fn check(history: &History, targets: &[Target], revert: Option<&[Value]>, signal: &Signal) -> Checked {
    let read = now(history, targets, revert, signal).await;
    let mut said = Vec::new();
    let (mut changed, mut unread, mut moved) = (Vec::new(), Vec::new(), Vec::new());
    for (target, now) in targets.iter().zip(&read) {
        let back = target.prior_shown.clone().unwrap_or_else(|| words(&target.place, &target.prior));
        match now {
            Some(now) if target.identity.is_some() && now.identity.is_some() && now.identity != target.identity => {
                moved.push(target.name.clone());
            }
            Some(now) => {
                said.push(format!("{} {} → {back}", target.name, now.shown.clone().unwrap_or_else(|| words(&target.place, &now.value))));
                if !same(&now.value, &target.applied) {
                    changed.push(target.name.clone());
                }
            }
            None => {
                said.push(format!("{} → {back}", target.name));
                unread.push(target.name.clone());
            }
        }
    }
    let why = if !moved.is_empty() {
        None
    } else if !changed.is_empty() {
        Some(format!("{} changed in Live since Kumi set {}, so Kumi won't undo the change.", capital(&changed.join(", ")), them(&changed)))
    } else if !unread.is_empty() {
        Some(format!("Kumi can't read {} in Live now, so it won't undo the change blind.", unread.join(", ")))
    } else {
        None
    };
    Checked { said: said.join("; "), why, moved }
}

fn them(names: &[String]) -> &'static str {
    if names.len() == 1 {
        "it"
    } else {
        "them"
    }
}

/// A reason's first word starts a sentence ("volume" is how Live's mixer names it).
fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Whether the change in `entry` is guarded: one of [`GUARDED`]'s, on the plugin host.
pub fn guarded(history: &History, entry: &Applied) -> bool {
    history.host.guard.on.get() && entry.tool.as_deref().is_some_and(|tool| GUARDED.contains(&tool))
}

/// Why the guarded change in `entry` mustn't be undone now, read fresh: a value changed since Kumi set it or can't be
/// read, nothing was kept of it, or Live's Set moved on while Kumi read. None: it may (a place that holds another
/// object now included: Live's own undo says that one).
pub async fn refusal(history: &History, entry: &Applied, signal: &Signal) -> Option<String> {
    let connection = &history.host.connection;
    let (lease, epoch) = (connection.lease.get(), connection.epoch.get());
    let Some(targets) = history.host.guard.of(&entry.record.id).flatten() else {
        return Some(nothing_kept());
    };
    let checked = check(history, &targets, entry.revert.as_deref(), signal).await;
    checked.why.or_else(|| {
        (connection.lease.get() != lease || connection.epoch.get() != epoch)
            .then(|| "Live's Set changed while Kumi read it, so Kumi didn't undo the change; look again.".to_owned())
    })
}

/// A group's guarded members, newest first: each refused as [`refusal`] would, except for the values a newer member
/// set at the same place (undoing that one first puts them back).
pub async fn group_refusal(history: &History, members: &[Applied], signal: &Signal) -> Option<String> {
    let mut claimed: HashSet<String> = HashSet::new();
    for member in members.iter().rev() {
        let Some(targets) = history.host.guard.of(&member.record.id).flatten() else {
            return Some(nothing_kept());
        };
        let place = |target: &Target| match &target.place {
            Place::Fast(item) => member.revert.as_ref().and_then(|items| items.get(*item)).map(|item| format!("fast {item}")),
            other => Some(format!("{other:?}")),
        };
        let open: Vec<Target> = targets.iter().filter(|target| place(target).is_none_or(|key| !claimed.contains(&key))).cloned().collect();
        if let Some(why) = check(history, &open, member.revert.as_deref(), signal).await.why {
            return Some(why);
        }
        claimed.extend(targets.iter().filter_map(place));
    }
    None
}

/// The guard's last word right before Live is asked to undo `entry`: when it refuses, an undo result that changes
/// nothing (and the refusal, kept for the integration to answer with). Nothing awaits between it and Live's undo.
pub async fn before_undo(history: &History, entry: &Applied) -> Option<UndoResult> {
    if !guarded(history, entry) {
        return None;
    }
    let connection = &history.host.connection;
    let asked = history.host.guard.asked.borrow().clone();
    let signal = abort::any(asked.into_iter().chain([connection.lifetime.clone(), abort::timeout(10_000)]));
    let why = refusal(history, entry, &signal).await?;
    history.host.guard.refuse(why.clone());
    Some(UndoResult::error(why))
}

/// Why a guarded change whose values weren't kept isn't undone.
pub fn nothing_kept() -> String {
    "Kumi kept no record of what this change set as Live read it back (it was made before Kumi checked undos, Live \
     couldn't confirm it once it was set, or it's past the most Kumi keeps), so it won't undo it blind."
        .into()
}

/// The preview of a guarded undo, as what it would do now (the host adds the track and the refusal's reason): each
/// value now and as the undo would put it, with the refusal it would meet.
pub async fn preview(history: &History, entry: &Applied, signal: &Signal) -> UndoPreview {
    let refusal = |reason: String| Some(UndoRefusal { code: "changed_since", reason });
    let Some(targets) = history.host.guard.of(&entry.record.id).flatten() else {
        return UndoPreview { text: "Undo the change".into(), refusal: refusal(nothing_kept()) };
    };
    let checked = check(history, &targets, entry.revert.as_deref(), signal).await;
    if !checked.moved.is_empty() {
        let text = format!("Undo through Live's own undo: {} moved since Kumi set {}", checked.moved.join(", "), them(&checked.moved));
        return UndoPreview { text, refusal: None };
    }
    UndoPreview { text: format!("Undo {}", checked.said), refusal: checked.why.and_then(refusal) }
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(n: usize) -> Option<Vec<Target>> {
        Some(vec![Target {
            place: Place::Tempo,
            name: format!("tempo {n}"),
            prior: json!(120),
            applied: json!(124),
            prior_shown: None,
            identity: None,
        }])
    }

    #[test]
    fn the_newest_changes_are_kept_and_the_oldest_go_first() {
        let guard = Guard::default();
        guard.keep("c0", target(0));
        assert_eq!(guard.of("c0"), None, "nothing is kept off the plugin host");
        guard.on.set(true);
        for n in 0..=KEPT {
            guard.keep(&format!("c{n}"), target(n));
        }
        assert_eq!(guard.of("c0"), None, "past the most kept, the oldest went");
        assert_eq!(guard.of("c1"), Some(target(1)));
        assert_eq!(guard.of(&format!("c{KEPT}")), Some(target(KEPT)));
        guard.forget("c1");
        assert_eq!(guard.of("c1"), None, "an undone change keeps nothing");
    }
}
