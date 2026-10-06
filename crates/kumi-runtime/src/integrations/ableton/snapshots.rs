//! What Kumi keeps of the clips a change cuts or deletes, so its own undo can make them again (history v0).
//!
//! - **Capture:** read in Live before the change, in one Python call (`assets/snapshots.py`): each clip's settings and
//!   every note field, or its file, warping and markers.
//! - **Keep:** in memory for this session's undo, and in the Set's `history.db` (objects addressed by content, an op
//!   per change). An unsaved Set's wait until it has a project id.
//! - **Restore:** checks first, so nothing changes when one fails; then the pieces Live left go, and each clip comes
//!   back whole. A big clip's notes go in more calls, all inside the one Live undo step the caller holds open.
use super::{
    fast::with_args,
    history::{FastResult, History},
    project::ProjectStore,
    remember::CurrentProject,
};
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use kumi_store::{
    history::{Object, Op},
    Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::PathBuf,
    rc::Rc,
};

/// The encoding of a kept clip (snapshots.py's `leaf`).
const CLIP_VERSION: u8 = 1;
/// The most code one Live Python call carries: python.run takes 64 KiB, and the script itself is some of it.
const CODE_BUDGET: usize = 60 * 1024;
/// The most kept clips this session holds in memory for its undo.
const KEPT_IN_MEMORY: usize = 4096;
/// Live's own refusals that Kumi passes on: they say what changed.
const CHECKS: &[&str] =
    &["changed in Live since", "place now", "slot now", "isn't there any more", "isn't in the Set any more", "its track takes"];

pub fn script(args: &Value) -> String {
    with_args("snapshots", args, include_str!("assets/snapshots.py"))
}

/// A clip as Live had it when Kumi read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Captured {
    pub identity: String,
    pub track: String,
    #[serde(rename = "where")]
    pub place: Value,
    pub leaf: Value,
    pub hash: String,
}
impl Captured {
    pub fn name(&self) -> String {
        self.leaf["name"].as_str().unwrap_or_default().to_owned()
    }
    /// Where it was in the Arrangement: (start, end).
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.place["start"].as_f64()?, self.place["end"].as_f64()?))
    }
}

/// A clip to make again: where it was, and what it was (an object in the Set's history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptClip {
    #[serde(rename = "where")]
    pub place: Value,
    pub object: String,
    pub name: String,
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
    /// The track the clips were on, by its identity in Live.
    pub track: String,
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
        let names: Vec<String> = self.clips.iter().map(|clip| format!("\u{201c}{}\u{201d}", clip.name)).collect();
        match names.len() {
            0 => String::new(),
            1 => names[0].clone(),
            n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
        }
    }
}

/// What a restore made, and what of it isn't exactly as it was.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restored {
    pub made: Vec<Value>,
    pub partial: Vec<Value>,
}
impl Restored {
    /// What didn't come back exactly, in words: “Take” (its length, its fades).
    pub fn short_of(&self) -> Option<String> {
        let parts: Vec<String> = self
            .partial
            .iter()
            .map(|row| {
                let missing: Vec<&str> = row["missing"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                format!("\u{201c}{}\u{201d} ({})", row["name"].as_str().unwrap_or_default(), missing.join(", "))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("; "))
    }
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

/// Make `material`'s clips again from their kept leaves (in its order), or, with `check_only`, only check that it
/// could. Err is why not, in Live's words where it said; nothing changed then unless a later call failed.
pub async fn restore(
    history: &History,
    material: &Material,
    leaves: &[Rc<Value>],
    check_only: bool,
    signal: Signal,
) -> Result<Restored, String> {
    let base = json!({"op":"restore","track":material.track,"remnants":material.remnants,"leaving":material.leaving,"check":check_only});
    // The notes that fit the first call go with it; the rest follow, clip by clip, once it's made.
    let mut budget = CODE_BUDGET.saturating_sub(script(&base).len() + 1024);
    let mut clips = vec![];
    let mut later: Vec<Vec<Value>> = vec![];
    for (kept, leaf) in material.clips.iter().zip(leaves) {
        let mut leaf = (**leaf).clone();
        let notes = leaf["notes"].as_array().cloned().unwrap_or_default();
        let mut first = vec![];
        let mut rest = vec![];
        for note in notes {
            let size = note.to_string().len() + 1;
            if rest.is_empty() && size <= budget {
                budget -= size;
                first.push(note);
            } else {
                rest.push(note);
            }
        }
        let mut clip = json!({"where":kept.place,"leaf":Value::Null});
        if leaf.get("notes").is_some() {
            leaf["notes"] = json!(first);
        }
        if !rest.is_empty() && !check_only {
            clip["notesToCome"] = json!(true);
        }
        clip["leaf"] = leaf;
        clips.push(clip);
        later.push(rest);
    }
    let mut args = base;
    args["clips"] = json!(clips);
    let done = call(history, &args, signal.clone()).await?;
    if check_only {
        return Ok(Restored::default());
    }
    let made = done["made"].as_array().cloned().unwrap_or_default();
    for (rest, clip) in later.iter().zip(&made) {
        let mut chunk: Vec<Value> = vec![];
        let mut size = 0;
        for note in rest {
            let length = note.to_string().len() + 1;
            if !chunk.is_empty() && size + length > CODE_BUDGET - 16 * 1024 {
                more_notes(history, material, clip, std::mem::take(&mut chunk), signal.clone()).await?;
                size = 0;
            }
            size += length;
            chunk.push(note.clone());
        }
        if !chunk.is_empty() {
            more_notes(history, material, clip, chunk, signal.clone()).await?;
        }
    }
    Ok(Restored { made, partial: done["partial"].as_array().cloned().unwrap_or_default() })
}

async fn more_notes(history: &History, material: &Material, made: &Value, notes: Vec<Value>, signal: Signal) -> Result<(), String> {
    call(history, &json!({"op":"notes","track":material.track,"clip":made["identity"],"notes":notes}), signal).await.map(|_| ())
}

async fn call(history: &History, args: &Value, signal: Signal) -> Result<Value, String> {
    match history.run_fast(script(args), signal).await {
        Ok(FastResult::Result(value)) => Ok(value),
        Ok(FastResult::Error { error, .. }) => Err(if CHECKS.iter().any(|said| error.contains(said)) {
            error.strip_prefix("ValueError: ").unwrap_or(&error).to_owned()
        } else {
            format!("Live didn't do it ({error})")
        }),
        Err(error) => Err(error.to_string()),
    }
}

/// The Set's history this session: kept clips in memory for undo, written to the project's history.db in the
/// background (a write queued here never waits on the disk).
#[derive(Default)]
pub struct SetHistory {
    leaves: RefCell<IndexMap<String, Rc<Value>>>,
    /// An unsaved Set's objects and ops, by its identity in Live: written once it has a project id.
    pending: RefCell<Option<(String, Vec<(Vec<Object>, Op)>)>>,
    /// The latest op kept, the next one's parent.
    last_op: RefCell<Option<String>>,
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
    /// A kept clip's leaf, by its object's hash.
    pub fn leaf(&self, hash: &str) -> Option<Rc<Value>> {
        self.leaves.borrow().get(hash).cloned()
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
            for (object, clip) in objects.iter().zip(clips) {
                leaves.insert(object.hash.clone(), Rc::new(clip.leaf.clone()));
            }
            while leaves.len() > KEPT_IN_MEMORY {
                leaves.shift_remove_index(0);
            }
        }
        let hashes: Vec<String> = objects.iter().map(|object| object.hash.clone()).collect();
        view["objects"] = json!(hashes);
        let op = Op::new(self.last_op.borrow().clone(), now, "cut", summary, view);
        *self.last_op.borrow_mut() = Some(op.id.clone());
        self.write(store, current, objects, op);
        hashes
    }
    /// The Set may have a project id now: what an unsaved Set kept is written once it has one, and dropped when
    /// another Set is open instead.
    pub fn identified(&self, store: Option<&Rc<dyn ProjectStore>>, current: Option<&CurrentProject>) {
        let Some(current) = current else { return };
        let waiting = self.pending.borrow_mut().take();
        let Some((identity, rows)) = waiting else { return };
        if identity != current.identity {
            return;
        }
        if current.project.is_none() {
            *self.pending.borrow_mut() = Some((identity, rows));
            return;
        }
        for (objects, op) in rows {
            self.write(store, Some(current), objects, op);
        }
    }
    fn write(&self, store: Option<&Rc<dyn ProjectStore>>, current: Option<&CurrentProject>, objects: Vec<Object>, op: Op) {
        let Some(current) = current else { return };
        let Some(project) = current.project.clone() else {
            let mut pending = self.pending.borrow_mut();
            match pending.as_mut().filter(|(identity, _)| *identity == current.identity) {
                Some((_, rows)) => rows.push((objects, op)),
                None => *pending = Some((current.identity.clone(), vec![(objects, op)])),
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
