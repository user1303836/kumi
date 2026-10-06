//! What Kumi keeps of the clips a change cuts or deletes, so its own undo can make them again (history v0).
//!
//! - **Capture:** read in Live before the change, in one Python call (`assets/snapshots.py`): each clip's settings and
//!   every note field, or its file, warping and markers; its groove; a Session clip's automation and follow actions.
//! - **Keep:** in memory for this session's undo (as compact JSON, up to `KEPT_BYTES`), and in the Set's `history.db`
//!   (objects addressed by content, an op per change), where an undo reads back what memory let go. An unsaved Set's
//!   wait until it has a project id.
//! - **Restore:** checks first, so nothing changes when one fails; then the pieces Live left go, and each clip comes
//!   back whole. Every call fits what python.run takes: the clips go in as many calls as they need and a big clip's
//!   notes in more, all inside the one Live undo step the caller holds open. Live stopping partway is answered with
//!   how far it got, for the caller to take back.
use super::{
    fast::with_args,
    history::{FastResult, History},
    project::ProjectStore,
    remember::CurrentProject,
};
use indexmap::IndexMap;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_store::{
    history::{Object, Op},
    Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
};

/// The encoding of a kept clip (snapshots.py's `leaf`).
const CLIP_VERSION: u8 = 1;
/// The most code python.run takes (its characters; Kumi counts the UTF-8 bytes, never fewer).
pub const CODE_LIMIT: usize = 65_536;
/// What each call leaves free under that.
const MARGIN: usize = 1024;
/// The most kept clips' JSON this session holds in memory; the oldest past it are read back from history.db.
const KEPT_BYTES: usize = 64 * 1024 * 1024;
/// The most an unsaved Set's kept clips wait in memory to be written once it's saved.
const PENDING_BYTES: usize = 64 * 1024 * 1024;
/// Live's own refusals that Kumi passes on: they say what's in the way.
const CHECKS: &[&str] = &[
    "changed in Live since",
    "place now",
    "slot now",
    "isn't there any more",
    "isn't in the Set any more",
    "its track takes",
    "is frozen",
    "isn't the track it was",
    "again at its length",
];

pub fn script(args: &Value) -> String {
    with_args("snapshots", args, include_str!("assets/snapshots.py"))
}

/// What `value` adds to a call's code: its JSON, encoded again as the string ARGS is read from.
fn encoded(value: &Value) -> usize {
    stringify(&Value::String(stringify(value))).len() - 2
}

/// A clip as Live had it when Kumi read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Captured {
    pub identity: String,
    pub track: String,
    #[serde(default)]
    pub track_name: String,
    /// Kumi's own id for the track, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    #[serde(rename = "where")]
    pub place: Value,
    pub leaf: Value,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_hash: Option<String>,
    /// A looping audio clip drawn out past its file: Live can't make one that long again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub over_file: bool,
}
impl Captured {
    pub fn name(&self) -> String {
        self.leaf["name"].as_str().unwrap_or_default().to_owned()
    }
    /// Where it was in the Arrangement: (start, end).
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.place["start"].as_f64()?, self.place["end"].as_f64()?))
    }
    /// What Kumi's undo won't bring back of it, known before: what Live doesn't give Kumi, or won't take back.
    pub fn short_of(&self) -> Vec<&'static str> {
        self.leaf["partial"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|key| match key {
                "fades" => "its fades",
                _ => "its automation",
            })
            .collect()
    }
    /// Whether Kumi's undo can make it again: Live can make it at its length, and it fits one call, its notes aside
    /// (they can follow in more).
    pub fn makeable(&self) -> bool {
        !self.over_file && self.fits()
    }
    /// Whether it fits one call to make it again, its notes aside (they can follow in more).
    pub fn fits(&self) -> bool {
        let mut leaf = self.leaf.clone();
        if leaf.get("notes").is_some() {
            leaf["notes"] = json!([]);
        }
        let clip = json!({"where":self.place,"leaf":leaf,"notesToCome":true});
        script(&json!({"op":"restore","track":self.track,"trackName":self.track_name,"trackId":self.track_id,"remnants":[],"leaving":[],"clips":[]}))
            .len()
            + encoded(&clip)
            <= CODE_LIMIT - MARGIN
    }
}

/// A clip to make again: where it was, and what it was (an object in the Set's history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptClip {
    #[serde(rename = "where")]
    pub place: Value,
    pub object: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_hash: Option<String>,
}

/// A piece Live left of a clip a change cut: deleted before the clip is made again, once it's as the change left it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Remnant {
    pub identity: String,
    pub hash: String,
    pub name: String,
}

/// What Kumi's undo of a change puts back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    /// The track the clips were on, by its identity in Live, its name then and Kumi's id for it.
    pub track: String,
    #[serde(default)]
    pub track_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    pub clips: Vec<KeptClip>,
    pub remnants: Vec<Remnant>,
    /// Clips the change's own undo takes away first (a new clip, or one it moves back), left out of the place check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leaving: Vec<String>,
    /// Whether the change has its own undo (through the bridge), run before the clips are made again.
    pub host_undo: bool,
}
impl Material {
    /// The clips it brings back, by name, as a summary says them: “Verse” and “Fill”.
    pub fn names(&self) -> String {
        let names: Vec<String> = self.clips.iter().map(|clip| quoted(&clip.name)).collect();
        match names.len() {
            0 => String::new(),
            1 => names[0].clone(),
            n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
        }
    }
    /// The track, as every call names it.
    fn track_args(&self, op: &str) -> Value {
        let mut args = json!({"op":op,"track":self.track,"trackName":self.track_name});
        if let Some(id) = &self.track_id {
            args["trackId"] = json!(id);
        }
        args
    }
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// What a restore made, and what of it isn't exactly as it was.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restored {
    pub removed: Vec<String>,
    pub made: Vec<Value>,
    pub partial: Vec<Value>,
}
impl Restored {
    /// What didn't come back, in words: “Take” without its fades; “Pad” without its automation.
    pub fn short_of(&self) -> Option<String> {
        let parts: Vec<String> = self
            .partial
            .iter()
            .map(|row| {
                let missing: Vec<&str> = row["missing"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                format!("{} without {}", quoted(row["name"].as_str().unwrap_or_default()), join(&missing))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("; "))
    }
    fn took(&mut self, done: &Value) {
        self.removed.extend(done["removed"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
        self.made.extend(done["made"].as_array().into_iter().flatten().cloned());
        self.partial.extend(done["partial"].as_array().into_iter().flatten().cloned());
    }
    /// Whether Live changed anything yet.
    pub fn changed(&self) -> bool {
        !self.removed.is_empty() || !self.made.is_empty()
    }
}

/// "a, b and c".
pub fn join(words: &[&str]) -> String {
    match words.len() {
        0 => String::new(),
        1 => words[0].to_owned(),
        n => format!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

/// Why a restore didn't finish.
#[derive(Debug, Clone, PartialEq)]
pub enum Stopped {
    /// Before it changed anything: a check refused (Live's own words), or Live couldn't be asked.
    Nothing(String),
    /// Partway: why, and what it had done by then (Live may have done more, when it didn't answer).
    Partway(String, Restored),
}

/// What Live answered a call with: its result, a refusal in its own words, or a failure in Kumi's.
enum Said {
    Done(Value),
    Refused(String),
    Failed(String),
}

/// Read clips in Live: by ref (`{"clips": [...]}`), or a track's main-lane Arrangement clips over a span
/// (`{"track", "from", "to", "except"}`). None when Live couldn't say.
pub async fn capture(history: &History, mut args: Value, signal: Signal) -> Option<Vec<Captured>> {
    args["op"] = json!("capture");
    match history.run_fast(script(&args), signal).await.ok()? {
        FastResult::Result(value) => serde_json::from_value(value["clips"].clone()).ok(),
        FastResult::Error { .. } => None,
    }
}

/// Check that `material`'s clips can be made again (the remnants as the change left them, each place free, each file
/// there, the track as it was), changing nothing: Err is why not, in Live's words where it said.
pub async fn check(history: &History, material: &Material, leaves: &[Rc<Value>], bound: &dyn Fn() -> Signal) -> Result<(), String> {
    let mut base = material.track_args("restore");
    base["remnants"] = json!(material.remnants);
    base["leaving"] = json!(material.leaving);
    base["check"] = json!(true);
    // What the checks read of each clip: never its notes or automation.
    let clips: Vec<Value> = material
        .clips
        .iter()
        .zip(leaves)
        .map(|(kept, leaf)| {
            let mut slim = json!({"kind":leaf["kind"],"name":leaf["name"]});
            if let Some(file) = leaf.get("file") {
                slim["file"] = file.clone();
            }
            json!({"where":kept.place,"leaf":slim})
        })
        .collect();
    for chunk in packed(&base, "clips", clips) {
        let mut args = base.clone();
        args["clips"] = json!(chunk);
        match call(history, &args, bound()).await {
            Said::Done(_) => {}
            Said::Refused(why) | Said::Failed(why) => return Err(why),
        }
    }
    Ok(())
}

/// `items` as `base`'s list `field`, in as few calls as fit, in order (an item too big for one call goes alone).
fn packed(base: &Value, field: &str, items: Vec<Value>) -> Vec<Vec<Value>> {
    let mut empty = base.clone();
    empty[field] = json!([]);
    let empty = script(&empty).len();
    let mut calls: Vec<Vec<Value>> = vec![];
    let mut size = 0;
    for item in items {
        let length = encoded(&item) + 1;
        match calls.last_mut() {
            Some(call) if size + length <= CODE_LIMIT - MARGIN => {
                size += length;
                call.push(item);
            }
            _ => {
                size = empty + length;
                calls.push(vec![item]);
            }
        }
    }
    calls
}

/// Make `material`'s clips again from their kept leaves (in its order), once `check` passed. The remnants go first;
/// a clip whose notes don't fit its call takes the rest in more, the last of which checks every note is in. Each call
/// waits for Live as long as a fresh `bound` lets it.
pub async fn restore(
    history: &History,
    material: &Material,
    leaves: &[Rc<Value>],
    bound: &dyn Fn() -> Signal,
) -> Result<Restored, Stopped> {
    let mut done = Restored::default();
    let mut remnants = Some(&material.remnants);
    let mut clips: VecDeque<(&KeptClip, Value)> =
        material.clips.iter().zip(leaves).map(|(kept, leaf)| (kept, json!({"where":kept.place,"leaf":**leaf}))).collect();
    while !clips.is_empty() {
        let mut args = material.track_args("restore");
        args["remnants"] = json!(remnants.take().cloned().unwrap_or_default());
        args["leaving"] = json!(material.leaving);
        args["clips"] = json!([]);
        let mut size = script(&args).len();
        let mut call_clips: Vec<Value> = vec![];
        let mut later: Vec<(Option<String>, Vec<Value>)> = vec![];
        while let Some((kept, clip)) = clips.pop_front() {
            let length = encoded(&clip) + 1;
            if size + length <= CODE_LIMIT - MARGIN {
                size += length;
                call_clips.push(clip);
                later.push((None, vec![]));
                continue;
            }
            if !call_clips.is_empty() {
                clips.push_front((kept, clip));
                break;
            }
            // Alone and too big: its notes that fit go now, and the rest after.
            let mut clip = clip;
            let notes = clip["leaf"]["notes"].as_array().cloned().unwrap_or_default();
            clip["leaf"]["notes"] = json!([]);
            clip["notesToCome"] = json!(true);
            size += encoded(&clip) + 1;
            let mut first = vec![];
            let mut rest = vec![];
            for note in notes {
                let length = encoded(&note) + 1;
                if rest.is_empty() && size + length <= CODE_LIMIT - MARGIN {
                    size += length;
                    first.push(note);
                } else {
                    rest.push(note);
                }
            }
            clip["leaf"]["notes"] = json!(first);
            call_clips.push(clip);
            later.push((kept.notes_hash.clone(), rest));
            break;
        }
        args["clips"] = json!(call_clips);
        let answer = match call(history, &args, bound()).await {
            Said::Done(answer) => answer,
            // The script's checks run first in every call: nothing of this call changed.
            Said::Refused(why) if !done.changed() => return Err(Stopped::Nothing(why)),
            // Any other failure may have come partway through the call: what Live did is for the caller to find out.
            Said::Refused(why) | Said::Failed(why) => return Err(Stopped::Partway(why, done)),
        };
        done.took(&answer);
        if let Some(error) = answer["error"].as_str() {
            return Err(Stopped::Partway(in_words(error), done));
        }
        let made: Vec<Value> = answer["made"].as_array().cloned().unwrap_or_default();
        for ((hash, rest), clip) in later.into_iter().zip(&made) {
            if rest.is_empty() {
                continue;
            }
            if let Err(why) = more_notes(history, material, clip, rest, hash, bound, &mut done).await {
                return Err(Stopped::Partway(why, done));
            }
        }
    }
    Ok(done)
}

/// The rest of a made clip's notes, in as many calls as they take; the last checks the clip holds every note it had.
async fn more_notes(
    history: &History,
    material: &Material,
    made: &Value,
    notes: Vec<Value>,
    hash: Option<String>,
    bound: &dyn Fn() -> Signal,
    done: &mut Restored,
) -> Result<(), String> {
    let mut base = material.track_args("notes");
    base["clip"] = made["identity"].clone();
    base["notesHash"] = json!(hash);
    let calls = packed(&base, "notes", notes);
    let count = calls.len();
    for (index, chunk) in calls.into_iter().enumerate() {
        let mut args = base.clone();
        args["notes"] = json!(chunk);
        if index + 1 < count {
            args["notesHash"] = Value::Null;
        }
        match call(history, &args, bound()).await {
            Said::Done(answer) if answer["exact"] == json!(false) => {
                done.partial.push(json!({"name":made["name"],"missing":["its notes"]}));
            }
            Said::Done(_) => {}
            Said::Refused(why) | Said::Failed(why) => return Err(why),
        }
    }
    Ok(())
}

/// What `material`'s track holds where Kumi's undo works (from its first clip's start on, and its Session clips'
/// scenes), to tell whether Live's undo put it back as it was. None when Live couldn't say.
pub async fn state(history: &History, material: &Material, signal: Signal) -> Option<Value> {
    let from = material.clips.iter().filter_map(|clip| clip.place["start"].as_f64()).fold(1e12, f64::min);
    let scenes: Vec<&Value> = material.clips.iter().filter_map(|clip| clip.place.get("scene")).collect();
    let mut args = material.track_args("state");
    args["from"] = json!(from);
    args["scenes"] = json!(scenes);
    match history.run_fast(script(&args), signal).await.ok()? {
        FastResult::Result(value) => Some(value),
        FastResult::Error { .. } => None,
    }
}

async fn call(history: &History, args: &Value, signal: Signal) -> Said {
    match history.run_fast(script(args), signal).await {
        Ok(FastResult::Result(value)) => Said::Done(value),
        Ok(FastResult::Error { error, .. }) if CHECKS.iter().any(|said| error.contains(said)) => {
            Said::Refused(error.strip_prefix("ValueError: ").unwrap_or(&error).to_owned())
        }
        Ok(FastResult::Error { error, .. }) => Said::Failed(in_words(&error)),
        Err(_) => Said::Failed("Live didn't answer".into()),
    }
}

/// Why Live stopped, in words: the known causes named, anything else as Live put it.
fn in_words(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("arguments require code") || lower.contains("too large") || lower.contains("65536") {
        "it was more than Live takes at once".into()
    } else if lower.contains("timeouterror") || lower.contains("timed out") {
        "Live took too long".into()
    } else if error == "Live didn't answer" {
        error.into()
    } else {
        let said =
            error.split(": ").skip_while(|part| part.ends_with("Error") || part.ends_with("Exception")).collect::<Vec<_>>().join(": ");
        let said = if said.is_empty() { error.to_owned() } else { said };
        // The script's own sentences say what happened already.
        if said.starts_with("Live ") {
            said
        } else {
            format!("Live said: {said}")
        }
    }
}

/// The Set's history this session: kept clips in memory for undo, written to the project's history.db in the
/// background (a write queued here never waits on the disk).
#[derive(Default)]
pub struct SetHistory {
    /// Kept clips' canonical JSON, by their objects' hashes, oldest first; `bytes` in all.
    leaves: RefCell<IndexMap<String, Rc<str>>>,
    bytes: Cell<usize>,
    /// An unsaved Set's objects and ops, by its identity in Live (and their bytes): written once it has a project id.
    pending: RefCell<Option<(String, VecDeque<(Vec<Object>, Op)>, usize)>>,
    /// The latest op kept, by project (or an unsaved Set's identity): the next one's parent.
    last_op: RefCell<HashMap<String, String>>,
    writes: Rc<Writes>,
}
#[derive(Default)]
struct Writes {
    queue: RefCell<VecDeque<(String, PathBuf, Vec<Object>, Op)>>,
    draining: Cell<bool>,
    /// The open history.db, by project.
    open: RefCell<Option<(String, Store)>>,
}
impl SetHistory {
    /// A kept clip's leaf, by its object's hash, from memory.
    pub fn leaf(&self, hash: &str) -> Option<Rc<Value>> {
        let raw = self.leaves.borrow().get(hash).cloned()?;
        serde_json::from_str(&raw).ok().map(Rc::new)
    }
    /// A kept clip's leaf, from memory or, once memory let it go, the Set's history.db.
    pub async fn kept_leaf(&self, hash: &str, store: Option<&Rc<dyn ProjectStore>>, current: Option<&CurrentProject>) -> Option<Rc<Value>> {
        if let Some(leaf) = self.leaf(hash) {
            return Some(leaf);
        }
        // An unsaved Set's, waiting to be written.
        let waiting = self.pending.borrow().as_ref().and_then(|(_, rows, _)| {
            rows.iter().flat_map(|(objects, _)| objects).find(|object| object.hash == hash).map(|object| object.raw.clone())
        });
        if let Some(raw) = waiting {
            return serde_json::from_str(&raw).ok().map(Rc::new);
        }
        let project = current?.project.clone()?;
        let open = self.writes.open.borrow().as_ref().filter(|(open, _)| *open == project).map(|(_, store)| store.clone());
        let store = match open {
            Some(store) => store,
            None => {
                let path = store?.history_path(&project)?;
                tokio::task::spawn_blocking(move || Store::open_history(path)).await.ok()?.ok()?
            }
        };
        let hash = hash.to_owned();
        let (kind, value) =
            tokio::task::spawn_blocking(move || store.read(|c| kumi_store::history::object(c, &hash))).await.ok()?.ok()??;
        (kind == "clip").then(|| Rc::new(value))
    }
    /// Keep clips a change cuts or deletes, with an op saying so (`summary`, and `view`, what it holds): their
    /// objects' hashes, in their order.
    pub fn keep(
        &self,
        store: Option<&Rc<dyn ProjectStore>>,
        current: Option<&CurrentProject>,
        clips: &[Captured],
        summary: &str,
        mut view: Value,
        now: i64,
    ) -> Vec<String> {
        let objects: Vec<Object> = clips.iter().map(|clip| Object::new("clip", CLIP_VERSION, &clip.leaf)).collect();
        {
            let mut leaves = self.leaves.borrow_mut();
            for object in &objects {
                if leaves.insert(object.hash.clone(), object.raw.as_str().into()).is_none() {
                    self.bytes.set(self.bytes.get() + object.raw.len());
                }
            }
            while self.bytes.get() > KEPT_BYTES && leaves.len() > objects.len() {
                if let Some((_, raw)) = leaves.shift_remove_index(0) {
                    self.bytes.set(self.bytes.get() - raw.len());
                }
            }
        }
        let hashes: Vec<String> = objects.iter().map(|object| object.hash.clone()).collect();
        view["objects"] = json!(hashes);
        let chain = current.map(|current| current.project.clone().unwrap_or_else(|| current.identity.clone())).unwrap_or_default();
        let op = Op::new(self.last_op.borrow().get(&chain).cloned(), now, "cut", summary, view);
        self.last_op.borrow_mut().insert(chain, op.id.clone());
        self.write(store, current, objects, op);
        hashes
    }
    /// The Set may have a project id now: what an unsaved Set kept is written once it has one, and dropped when
    /// another Set is open instead.
    pub fn identified(&self, store: Option<&Rc<dyn ProjectStore>>, current: Option<&CurrentProject>) {
        let Some(current) = current else { return };
        let waiting = self.pending.borrow_mut().take();
        let Some((identity, rows, bytes)) = waiting else { return };
        if identity != current.identity {
            return;
        }
        let Some(project) = current.project.clone() else {
            *self.pending.borrow_mut() = Some((identity, rows, bytes));
            return;
        };
        // Its ops carry on the project's chain.
        if let Some(last) = self.last_op.borrow_mut().remove(&identity) {
            self.last_op.borrow_mut().entry(project).or_insert(last);
        }
        for (objects, op) in rows {
            self.write(store, Some(current), objects, op);
        }
    }
    fn write(&self, store: Option<&Rc<dyn ProjectStore>>, current: Option<&CurrentProject>, objects: Vec<Object>, op: Op) {
        let Some(current) = current else { return };
        let Some(project) = current.project.clone() else {
            let size: usize = objects.iter().map(|object| object.raw.len()).sum();
            let mut pending = self.pending.borrow_mut();
            match pending.as_mut().filter(|(identity, _, _)| *identity == current.identity) {
                Some((_, rows, bytes)) => {
                    rows.push_back((objects, op));
                    *bytes += size;
                    // Past the cap the oldest go unwritten: this session's undo still holds them in memory.
                    while *bytes > PENDING_BYTES && rows.len() > 1 {
                        if let Some((dropped, _)) = rows.pop_front() {
                            *bytes -= dropped.iter().map(|object| object.raw.len()).sum::<usize>();
                        }
                    }
                }
                None => *pending = Some((current.identity.clone(), VecDeque::from([(objects, op)]), size)),
            }
            return;
        };
        let Some(path) = store.and_then(|store| store.history_path(&project)) else { return };
        self.writes.queue.borrow_mut().push_back((project, path, objects, op));
        if !self.writes.draining.replace(true) {
            tokio::task::spawn_local(drain(self.writes.clone()));
        }
    }
}
/// Write what's queued, in order, opening each project's history.db off Kumi's thread. Best effort: this session's
/// undo works from memory either way.
async fn drain(writes: Rc<Writes>) {
    loop {
        let next = writes.queue.borrow_mut().pop_front();
        let Some((project, path, objects, op)) = next else { break };
        let open = writes.open.borrow().as_ref().filter(|(open, _)| *open == project).map(|(_, store)| store.clone());
        let store = match open {
            Some(store) => Some(store),
            None => match tokio::task::spawn_blocking(move || Store::open_history(path)).await {
                Ok(Ok(store)) => {
                    *writes.open.borrow_mut() = Some((project, store.clone()));
                    Some(store)
                }
                _ => None,
            },
        };
        if let Some(store) = store {
            store.write(
                move |connection| {
                    kumi_store::history::put_objects(connection, &objects)?;
                    kumi_store::history::put_op(connection, &op)
                },
                |_| {},
            );
        }
    }
    writes.draining.set(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn why_live_stopped_is_said_in_words() {
        assert_eq!(in_words("TimeoutError: python.run exceeded 10000 ms"), "Live took too long");
        assert_eq!(
            in_words("Python arguments require code, eval/exec mode and timeoutMs from 1 to 30000"),
            "it was more than Live takes at once"
        );
        assert_eq!(
            in_words("RuntimeError: Clips cannot be created on frozen tracks"),
            "Live said: Clips cannot be created on frozen tracks"
        );
    }

    #[test]
    fn clips_are_packed_into_calls_that_fit() {
        let base = json!({"op":"restore","track":"live:1","trackName":"Keys","remnants":[],"leaving":[],"check":true});
        let clip = |n: usize| json!({"where":{"start":n as f64,"end":n as f64 + 1.0},"leaf":{"kind":"midi","name":"x".repeat(1000)}});
        let calls = packed(&base, "clips", (0..200).map(clip).collect());
        assert!(calls.len() > 1);
        assert_eq!(calls.iter().map(Vec::len).sum::<usize>(), 200);
        for call in calls {
            let mut args = base.clone();
            args["clips"] = json!(call);
            assert!(script(&args).len() <= CODE_LIMIT, "{}", script(&args).len());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_unsaved_sets_leaf_is_read_from_its_queue_once_memory_let_it_go() {
        let history = SetHistory::default();
        let current = CurrentProject { identity: "set".into(), path: None, name: "Set".into(), project: None, unsaved: true };
        let clip = Captured {
            identity: "live:1".into(),
            track: "live:2".into(),
            track_name: "Keys".into(),
            track_id: None,
            place: json!({"start":0.0,"end":4.0}),
            leaf: json!({"kind":"midi","name":"Verse","notes":[[60, 0, 1, 100]]}),
            hash: "h".into(),
            notes_hash: None,
            over_file: false,
        };
        let hash = history.keep(None, Some(&current), &[clip], "Deleted", json!({}), 1).remove(0);
        history.leaves.borrow_mut().clear();
        assert_eq!(history.kept_leaf(&hash, None, Some(&current)).await.unwrap()["name"], "Verse");
    }

    #[test]
    fn memory_keeps_the_newest_leaves_by_their_bytes() {
        let history = SetHistory::default();
        let current = CurrentProject { identity: "set".into(), path: None, name: "Set".into(), project: None, unsaved: true };
        let clip = |name: &str| Captured {
            identity: "live:1".into(),
            track: "live:2".into(),
            track_name: "Keys".into(),
            track_id: None,
            place: json!({"start":0.0,"end":4.0}),
            leaf: json!({"kind":"midi","name":name,"notes":vec![[60, 0, 1, 100]; 20_000]}),
            hash: "h".into(),
            notes_hash: None,
            over_file: false,
        };
        let mut kept = vec![];
        for n in 0..300 {
            kept.extend(history.keep(None, Some(&current), &[clip(&n.to_string())], "Deleted", json!({}), n));
        }
        assert!(history.bytes.get() <= KEPT_BYTES, "{}", history.bytes.get());
        assert!(history.leaf(&kept[0]).is_none(), "the oldest went");
        assert!(history.leaf(kept.last().unwrap()).is_some());
        // An unsaved Set's rows wait, capped too, and carry one chain of ops.
        let pending = history.pending.borrow();
        let (_, rows, bytes) = pending.as_ref().unwrap();
        assert!(*bytes <= PENDING_BYTES);
        assert_eq!(rows.back().unwrap().1.parent.as_ref(), Some(&rows[rows.len() - 2].1.id));
    }
}
