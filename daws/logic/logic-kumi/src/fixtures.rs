//! Scenes: every helper reply the bridge's readers asked for, recorded from the real Logic and replayed to the bridge's
//! own code unchanged. A scene is a gzipped JSON file (`tests/fixtures/<scene>/scene.json.gz`, `SCENE_FILE`): the Logic
//! it was recorded from, then each request (its op and parameters, without the id) with the reply the helper gave, in
//! order, and the events the helper sent meanwhile. The replaying helper answers a request by its op and parameters:
//! the same request asked again gets the next reply recorded for it, and the last one over and over, so a scene stands
//! for a Logic that holds still. Fixtures are the only Logic-version-specific data: a new Logic is a re-recording.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Map, Value};

use crate::helper::{Helper, HelperError, Launch};

/// What a scene file is: its format's name and version.
pub const SCHEMA: &str = "kumi-logic-scene/1";

/// The file a scene is stored in, under its folder: compact JSON, gzipped (read one with `gunzip -c <file> | jq .`).
pub const SCENE_FILE: &str = "scene.json.gz";

/// The JSON text of the scene stored in `dir`.
pub fn scene_text(dir: &Path) -> Result<String, String> {
    let path = dir.join(SCENE_FILE);
    let mut text = String::new();
    std::fs::File::open(&path)
        .and_then(|file| GzDecoder::new(file).read_to_string(&mut text))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(text)
}

/// Stores `text`, a scene's JSON, in `dir` (made if missing), gzipped.
pub fn write_scene_text(dir: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(text.as_bytes())?;
    std::fs::write(dir.join(SCENE_FILE), gz.finish()?)
}

/// One recorded request and its reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub op: String,
    pub params: Map<String, Value>,
    /// The reply without its id (`ok` and `ms` kept, as the helper wrote them), or the refusal's reply.
    pub reply: Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub name: String,
    /// What the recording says of Logic: its version, bundle, pid, and whatever else the recorder notes.
    pub logic: Map<String, Value>,
    pub calls: Vec<Call>,
    pub events: Vec<Value>,
}

/// The key a request is replayed by: its op and its parameters, less the id.
fn key(op: &str, params: &Map<String, Value>) -> String {
    let mut params = params.clone();
    params.remove("id");
    format!("{op} {}", Value::Object(params))
}

impl Scene {
    pub fn new(name: &str) -> Scene {
        Scene { name: name.into(), ..Scene::default() }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "scene": self.name,
            "logic": self.logic,
            "calls": self.calls.iter().map(|call| json!({"op": call.op, "params": call.params, "reply": call.reply})).collect::<Vec<_>>(),
            "events": self.events,
        })
    }

    pub fn from_json(value: &Value) -> Result<Scene, String> {
        if value.get("schema").and_then(Value::as_str) != Some(SCHEMA) {
            return Err(format!("not a {SCHEMA} file"));
        }
        let calls = value
            .get("calls")
            .and_then(Value::as_array)
            .ok_or("no calls")?
            .iter()
            .map(|call| {
                Ok(Call {
                    op: call.get("op").and_then(Value::as_str).ok_or("a call without an op")?.to_string(),
                    params: call.get("params").and_then(Value::as_object).cloned().unwrap_or_default(),
                    reply: call.get("reply").cloned().unwrap_or(Value::Null),
                })
            })
            .collect::<Result<Vec<Call>, String>>()?;
        Ok(Scene {
            name: value.get("scene").and_then(Value::as_str).unwrap_or_default().to_string(),
            logic: value.get("logic").and_then(Value::as_object).cloned().unwrap_or_default(),
            calls,
            events: value.get("events").and_then(Value::as_array).cloned().unwrap_or_default(),
        })
    }

    /// The scene in `dir/scene.json.gz`.
    pub fn load(dir: &Path) -> Result<Scene, String> {
        let path = dir.join(SCENE_FILE);
        let text = scene_text(dir)?;
        let value: Value = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        Scene::from_json(&value)
    }

    /// Writes the scene to `dir/scene.json.gz`: compact JSON with a final newline, naming no home of the Mac it was
    /// recorded on (`redacted`), gzipped.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let scene = self.to_json();
        let mut text = redacted(&serde_json::to_string(&scene)?, &scene);
        text.push('\n');
        write_scene_text(dir, &text)
    }

    /// The ops recorded, with how many calls each.
    pub fn counts(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = vec![];
        for call in &self.calls {
            match counts.iter_mut().find(|(op, _)| *op == call.op) {
                Some(entry) => entry.1 += 1,
                None => counts.push((call.op.clone(), 1)),
            }
        }
        counts
    }
}

/// A scene's text with no home of the Mac it was recorded on: `/Users/<name>` becomes `/Users/me` in every string, paths
/// and file URLs alike, and a project's id, the hash of its path (`logic::project_id`), becomes the hash of the redacted
/// path wherever it appears, so the bridge computes the same id over the replayed scene as the recording holds.
pub fn redacted(text: &str, scene: &Value) -> String {
    let mut ids = vec![];
    project_ids(scene, &mut ids);
    let mut text = redacted_homes(text);
    for (recorded, redacted) in ids {
        text = text.replace(&recorded, &redacted);
    }
    text
}

/// `text` with every `/Users/<name>` made `/Users/me`.
pub fn redacted_homes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(HOMES) {
        let (before, from) = rest.split_at(at + HOMES.len());
        out.push_str(before);
        out.push_str(STAND_IN);
        rest = &from[home_name_end(from)..];
    }
    out.push_str(rest);
    out
}

/// The names after `/Users/` in `text` other than the stand-in: the homes a recording names. A guard for files a
/// recorder didn't write.
pub fn homes_named(text: &str) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    let mut rest = text;
    while let Some(at) = rest.find(HOMES) {
        rest = &rest[at + HOMES.len()..];
        let name = &rest[..home_name_end(rest)];
        if !name.is_empty() && name != STAND_IN && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
        rest = &rest[name.len()..];
    }
    names
}

const HOMES: &str = "/Users/";
const STAND_IN: &str = "me";

/// Where the account name after `/Users/` ends in `from`: at the next slash, quote, backslash or space.
fn home_name_end(from: &str) -> usize {
    from.find(|c: char| c == '/' || c == '"' || c == '\\' || c.is_whitespace()).unwrap_or(from.len())
}

/// Every project in `value` (an object whose `id` is the hash of its `path`) as (recorded id, id of the redacted path).
fn project_ids(value: &Value, ids: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let (Some(Value::String(id)), Some(Value::String(path))) = (map.get("id"), map.get("path")) {
                if *id == crate::logic::project_id(path) {
                    let redacted = crate::logic::project_id(&redacted_homes(path));
                    if redacted != *id && !ids.iter().any(|(recorded, _)| recorded == id) {
                        ids.push((id.clone(), redacted));
                    }
                }
            }
            map.values().for_each(|inner| project_ids(inner, ids));
        }
        Value::Array(items) => items.iter().for_each(|inner| project_ids(inner, ids)),
        _ => {}
    }
}

/// A helper that records every request it passes to the real one, and every event, into a scene.
pub struct Recorder {
    inner: Box<dyn Helper>,
    scene: Arc<Mutex<Scene>>,
}

impl Recorder {
    pub fn new(inner: Box<dyn Helper>, scene: Arc<Mutex<Scene>>) -> Recorder {
        Recorder { inner, scene }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A reply as the scene keeps it: without the request's id.
fn stripped(reply: &Value) -> Value {
    let mut reply = reply.clone();
    if let Some(object) = reply.as_object_mut() {
        object.remove("id");
    }
    reply
}

impl Helper for Recorder {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        let result = self.inner.call(op, params.clone());
        let params = params.as_object().cloned().unwrap_or_default();
        let reply = match &result {
            Ok(reply) => stripped(reply),
            Err(HelperError::Refused { reply, .. }) => stripped(reply),
            Err(HelperError::Timeout) => json!({"ok": false, "error": "timeout"}),
            Err(HelperError::Gone(why)) => json!({"ok": false, "error": "gone", "detail": why}),
        };
        lock(&self.scene).calls.push(Call { op: op.into(), params, reply });
        result
    }

    fn event(&mut self, wait: Duration) -> Option<Value> {
        let event = self.inner.event(wait)?;
        lock(&self.scene).events.push(event.clone());
        Some(event)
    }

    fn alive(&self) -> bool {
        self.inner.alive()
    }

    fn quit(&mut self) {
        self.inner.quit();
    }
}

/// A helper made of a scene: each request gets the reply recorded for it, in order, then the last one again; a
/// request the scene never saw is refused with `unrecorded`, which a test reads as "the reader asked for something
/// the recording didn't", so the scene is recorded again.
#[derive(Clone)]
pub struct SceneHands {
    state: Arc<Mutex<SceneState>>,
}

struct SceneState {
    replies: HashMap<String, VecDeque<Value>>,
    events: VecDeque<Value>,
    /// Requests the scene had no reply for, in order: what a re-recording must cover.
    unrecorded: Vec<(String, Value)>,
    calls: Vec<(String, Value)>,
    gone: bool,
}

impl SceneHands {
    pub fn new(scene: &Scene) -> SceneHands {
        let mut replies: HashMap<String, VecDeque<Value>> = HashMap::new();
        for call in &scene.calls {
            replies.entry(key(&call.op, &call.params)).or_default().push_back(call.reply.clone());
        }
        SceneHands {
            state: Arc::new(Mutex::new(SceneState {
                replies,
                events: scene.events.iter().cloned().collect(),
                unrecorded: vec![],
                calls: vec![],
                gone: false,
            })),
        }
    }

    /// The requests the scene couldn't answer.
    pub fn unrecorded(&self) -> Vec<(String, Value)> {
        lock(&self.state).unrecorded.clone()
    }

    /// Every request made, in order.
    pub fn calls(&self) -> Vec<(String, Value)> {
        lock(&self.state).calls.clone()
    }
}

impl Launch for SceneHands {
    fn launch(&mut self) -> Result<Box<dyn Helper>, String> {
        lock(&self.state).gone = false;
        Ok(Box::new(self.clone()))
    }
}

impl Helper for SceneHands {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        let mut state = lock(&self.state);
        if state.gone {
            return Err(HelperError::Gone("the scene's helper was quit".into()));
        }
        state.calls.push((op.to_string(), params.clone()));
        let id = state.calls.len();
        let params_map = params.as_object().cloned().unwrap_or_default();
        let key = key(op, &params_map);
        let reply = match state.replies.get_mut(&key) {
            Some(queue) if queue.len() > 1 => queue.pop_front(),
            Some(queue) => queue.front().cloned(),
            None => None,
        };
        let Some(mut reply) = reply else {
            state.unrecorded.push((op.to_string(), params));
            return Err(HelperError::Refused {
                code: "unrecorded".into(),
                reply: json!({"id": id, "ok": false, "error": "unrecorded", "op": op}),
            });
        };
        reply["id"] = id.into();
        if reply["ok"] == Value::Bool(true) {
            Ok(reply)
        } else {
            let code = reply["error"].as_str().unwrap_or("failed").to_string();
            Err(HelperError::Refused { code, reply })
        }
    }

    fn event(&mut self, wait: Duration) -> Option<Value> {
        let event = lock(&self.state).events.pop_front();
        if event.is_none() && !wait.is_zero() {
            std::thread::sleep(wait.min(Duration::from_millis(20)));
        }
        event
    }

    fn alive(&self) -> bool {
        !lock(&self.state).gone
    }

    fn quit(&mut self) {
        lock(&self.state).gone = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::MockHands;

    #[test]
    fn a_scene_records_the_helpers_replies_and_replays_them_in_order() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7})]);
        hands.set_windows(json!([{"title": "Scratch.logicx - Tracks"}]), &["Scratch.logicx - Tracks"]);
        hands.reply("windows", Ok(json!({"windows": [], "listed": []})));
        hands.emit(json!({"event": "app", "state": "activated", "pid": 7}));
        let scene = Arc::new(Mutex::new(Scene::new("scratch")));
        let mut launcher = hands.clone();
        let mut recorder = Recorder::new(Launch::launch(&mut launcher).unwrap(), scene.clone());
        assert!(recorder.call("apps", json!({"bundles": ["com.apple.logic10"]})).is_ok());
        assert_eq!(recorder.call("windows", json!({"pid": 7})).unwrap()["windows"], json!([]));
        assert_eq!(recorder.call("windows", json!({"pid": 7})).unwrap()["windows"][0]["title"], "Scratch.logicx - Tracks");
        assert!(matches!(recorder.call("menu", json!({})), Err(HelperError::Refused { .. })));
        assert_eq!(recorder.event(Duration::from_millis(10)).unwrap()["state"], "activated");
        recorder.quit();
        let recorded = lock(&scene).clone();
        assert_eq!(recorded.counts(), [("apps".to_string(), 1), ("windows".to_string(), 2), ("menu".to_string(), 1)]);
        assert_eq!(recorded.calls[1].reply["ok"], true);
        assert_eq!(recorded.calls[1].reply.get("id"), None, "replies are kept without the request's id");
        assert_eq!(recorded.calls[3].reply["error"], "unknown-op");
        assert_eq!(recorded.events.len(), 1);

        // Through a file and back.
        let dir = tempfile::tempdir().unwrap();
        recorded.save(dir.path()).unwrap();
        assert_eq!(std::fs::read(dir.path().join(SCENE_FILE)).unwrap()[..2], [0x1f, 0x8b], "stored gzipped");
        let loaded = Scene::load(dir.path()).unwrap();
        assert_eq!(loaded, recorded);
        assert!(Scene::from_json(&json!({"schema": "other"})).is_err());

        // Replayed: the same request gets the replies in order, then the last again; an unknown one is `unrecorded`.
        let replay = SceneHands::new(&loaded);
        let mut helper = Launch::launch(&mut replay.clone()).unwrap();
        assert_eq!(helper.call("windows", json!({"pid": 7})).unwrap()["windows"], json!([]));
        assert_eq!(helper.call("windows", json!({"pid": 7})).unwrap()["windows"][0]["title"], "Scratch.logicx - Tracks");
        assert_eq!(helper.call("windows", json!({"pid": 7})).unwrap()["windows"][0]["title"], "Scratch.logicx - Tracks");
        assert_eq!(helper.call("apps", json!({"bundles": ["com.apple.logic10"]})).unwrap()["apps"][0]["pid"], 7);
        assert!(matches!(helper.call("menu", json!({})), Err(HelperError::Refused { code, .. }) if code == "unknown-op"));
        match helper.call("tree", json!({"pid": 7, "root": "main"})) {
            Err(HelperError::Refused { code, reply }) => assert_eq!((code.as_str(), reply["ok"].as_bool()), ("unrecorded", Some(false))),
            other => panic!("{other:?}"),
        }
        assert_eq!(replay.unrecorded(), [("tree".to_string(), json!({"pid": 7, "root": "main"}))]);
        assert_eq!(helper.event(Duration::from_millis(1)).unwrap()["state"], "activated");
        assert_eq!(helper.event(Duration::from_millis(1)), None);
        assert_eq!(replay.calls().len(), 6);
        helper.quit();
        assert!(!helper.alive());
        assert!(matches!(helper.call("apps", json!({})), Err(HelperError::Gone(_))));
    }

    #[test]
    fn a_saved_scene_names_no_home_and_its_project_ids_are_of_the_redacted_paths() {
        let path = "/Users/someone/Music/Logic/Song.logicx";
        let id = crate::logic::project_id(path);
        let mut scene = Scene::new("home");
        scene.logic.insert("project".into(), json!({"id": id, "name": "Song", "path": path}));
        scene.calls.push(Call {
            op: "windows".into(),
            params: Map::new(),
            reply: json!({"ok": true, "windows": [{"title": "Song - Tracks", "document": format!("file://{path}/")}]}),
        });
        scene.events.push(json!({"event": "project", "project": {"id": id, "name": "Song", "path": path}}));
        let dir = tempfile::tempdir().unwrap();
        scene.save(dir.path()).unwrap();
        let text = scene_text(dir.path()).unwrap();
        assert_eq!(homes_named(&text), Vec::<String>::new(), "{text}");
        assert!(!text.contains(&id), "{text}");

        let redacted = "/Users/me/Music/Logic/Song.logicx";
        let loaded = Scene::load(dir.path()).unwrap();
        assert_eq!(loaded.logic["project"], json!({"id": crate::logic::project_id(redacted), "name": "Song", "path": redacted}));
        assert_eq!(loaded.calls[0].reply["windows"][0]["document"], format!("file://{redacted}/"));
        assert_eq!(loaded.events[0]["project"]["id"], crate::logic::project_id(redacted));

        // The scan behind the guards: names end at a slash, a quote, a backslash or a space; the stand-in isn't one.
        assert_eq!(homes_named("/Users/me/x, \"/Users/a\", file:///Users/b/y /Users/a/z"), ["a", "b"]);
        assert_eq!(redacted_homes("file:///Users/b/y and /Users/c"), "file:///Users/me/y and /Users/me");
    }
}
