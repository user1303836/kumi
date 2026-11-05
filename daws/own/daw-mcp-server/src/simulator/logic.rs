//! A scripted Logic Pro project for the host's tests: what Kumi's own bridge beside Logic (`logic-kumi`) answers to
//! `read`, `snapshot`, `focus` and `focus_body` (the native M2 contract), replayed verbatim.
//!
//! The 26 original M2 requests over Scratch are replayed from the byte-preserved vector at
//! `d821e6690c06f9b983ca63ac98e37ba40eb50c7e:crates/daw-mcp-server/tests/vectors/logic-reads.json`.
//! The native reader tests independently compare their recorded AX scenes with the same vector. Nothing here
//! generates or blesses recordings. Epoch changes and timeline pages derive from those recorded answers.
use super::replay::{epoch_of, find, load_cases, retag, Case, RECORDED_EPOCH};
use super::{Respond, SimulatorConfig};
use daw_bridge::auth::Secret;
use daw_bridge::wire::Project;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const VECTORS: &str = include_str!("../../tests/vectors/logic-reads.json");
const VECTORS_NAME: &str = "tests/vectors/logic-reads.json";
/// The timeline body's page, in tracks.
const MAX_LANES: usize = 500;
const METHODS: [&str; 4] = ["focus", "read", "snapshot", "focus_body"];

struct State {
    /// The epoch the project is in now (the vectors were recorded in [`RECORDED_EPOCH`]).
    epoch: u32,
    /// Which project is open: 0 the recorded one, else another copy of it, with an id and a name of its own.
    project: u32,
    cases: Vec<Case>,
    /// What the simulator was asked, in order: method and params.
    asked: Vec<(String, Value)>,
}

/// A project that answers like the bridge. Clones share the project.
#[derive(Clone)]
pub struct LogicScript {
    state: Arc<Mutex<State>>,
}

type Answer = Result<Value, (String, String)>;

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

/// A page of the recorded Tracks area, as the bridge pages it: `limit` lanes from a cursor, or centred on the `around`
/// track (half a page above it), each lane's regions kept to the `window` asked for (the recorded one otherwise).
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
    let window = params.get("window").cloned().unwrap_or_else(|| whole["window"].clone());
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

impl LogicScript {
    /// The project of the vectors: what the bridge answers for Scratch.
    pub fn new() -> Self {
        LogicScript {
            state: Arc::new(Mutex::new(State {
                epoch: RECORDED_EPOCH,
                project: 0,
                cases: load_cases(VECTORS, VECTORS_NAME),
                asked: vec![],
            })),
        }
    }

    /// A track or region was added, removed or moved: the epoch moves, so refs and cursors from before are stale.
    pub fn change_tracks(&self) {
        self.state.lock().unwrap().epoch += 1;
    }

    /// Another project is open: the `n`th copy of the recorded one (0 is the recorded one itself), with an id, an
    /// identity, a name and a path of its own and the same rows, as a copy saved under another name has. The epoch
    /// moves with it.
    pub fn open_project(&self, n: u32) {
        let mut state = self.state.lock().unwrap();
        state.epoch += 1;
        state.project = n;
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

    pub fn respond(&self) -> Respond {
        let script = self.clone();
        Arc::new(move |method, params| script.answer(method, params))
    }

    /// The simulator's config for Logic with this project behind it.
    pub fn config(&self, secret: Secret, resource: &str) -> SimulatorConfig {
        let mut config = SimulatorConfig::logic(secret, resource).with_methods(&METHODS, self.respond());
        config.identity.project = Some(self.project());
        config.offer.events = ["project", "transport", "tracks", "selection"].map(str::to_string).to_vec();
        config
    }

    fn answer(&self, method: &str, params: &Value) -> Option<Answer> {
        if !METHODS.contains(&method) {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        state.asked.push((method.to_string(), params.clone()));
        let (epoch, project) = (state.epoch, state.project);
        // Past the recorded epoch, a ref or cursor from an earlier one is refused before anything is looked up, as the
        // bridge refuses it whatever it was for; in the recorded epoch the refusals recorded answer.
        if epoch != RECORDED_EPOCH {
            if let Some(stale) = stale_in(params, epoch) {
                return Some(Err(stale));
            }
        }
        let answered = |result: &Value| another_project(&retag(result, RECORDED_EPOCH, epoch), project);
        let asked = retag(params, epoch, RECORDED_EPOCH);
        if let Some(case) = find(&state.cases, method, &asked) {
            return Some(case.response.clone().map(|result| answered(&result)));
        }
        // A page of the Tracks area nobody recorded: cut from the recorded one.
        if method == "focus_body" && asked["kind"] == "timeline" {
            let whole = find(&state.cases, "focus_body", &json!({"kind": "timeline"})).and_then(|case| case.response.as_ref().ok());
            if let Some(whole) = whole {
                return Some(timeline_page(whole, &asked).map(|page| answered(&page)));
            }
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
}
