//! Live's saved Sets: the key a Set keeps its project's id under, how Live's templates and Project folders are told,
//! and the semantic catch-up and watch descriptions from its bridge's snapshots. The store itself is core's
//! (core/project.rs).

use indexmap::IndexMap;
use kumi_common::js::json::stringify;
use kumi_runtime::core::contracts::{CatchUp, JsonObject};
pub use kumi_runtime::core::project::{
    create_project_store, decide_project_with, moved_to, project_id, project_id_of, same_file, Baseline, FileProjectStore, ProjectRules,
    ProjectStore,
};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

/// The key a Set keeps its project's id under (`Song.set_data`), with the Set: Save As and moving the
/// folder keep it.
pub const PROJECT_KEY: &str = "kumi.project";
/// Whether a Set at `path` is a template, or Live's own default Set: new songs start from these, so one
/// never gets a project id of its own, or every song made from it would share it.
pub fn template_location(path: &str) -> bool {
    let parts: Vec<String> = Path::new(path).components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
    let has = |name: &str| parts.iter().any(|part| part == name);
    let in_row = |names: &[&str]| parts.windows(names.len()).any(|row| row.iter().zip(names).all(|(part, name)| part == name));
    in_row(&["user library", "templates"])
        // Live's own settings, where its default Set is kept: ~/Library/Preferences/Ableton on a Mac,
        // %APPDATA%\Ableton on Windows.
        || in_row(&["library", "preferences", "ableton"])
        || in_row(&["appdata", "roaming", "ableton"])
        || (has("templates") && parts.iter().any(|part| part.ends_with(".app") || part == "resources"))
}
/// The Live Project folder a Set is in: the nearest folder above it with an "Ableton Project Info".
pub fn project_folder(path: &str) -> Option<PathBuf> {
    Path::new(path).ancestors().skip(1).find(|folder| folder.join("Ableton Project Info").is_dir()).map(Path::to_path_buf)
}
/// Live's templates and Project folders.
pub struct LiveRules;

impl ProjectRules for LiveRules {
    fn template_location(&self, path: &str) -> bool {
        template_location(path)
    }
    fn project_folder(&self, path: &str) -> Option<PathBuf> {
        project_folder(path)
    }
}

/// A Set's project, by Live's rules (`decide_project_with`).
pub fn decide_project(kept: Option<&str>, path: &str, last: Option<&str>, own: bool) -> (String, bool) {
    decide_project_with(&LiveRules, kept, path, last, own)
}
use kumi_runtime::core::describe::{describe_changes, describe_watch_changes, small, Detail, ProjectChange, ProjectItem, ProjectWords};
pub use kumi_runtime::core::describe::{DescribedDiff, DescribedWatch};

pub fn catch_up_from(name: &str, baseline: &Baseline, described: DescribedDiff) -> CatchUp {
    CatchUp { set: name.into(), last_seen_at: baseline.saved_at, lines: described.lines, more: described.more, after_reconnect: None }
}

/// Live's kinds, as its bridge's semantic snapshots name them.
pub const LIVE_WORDS: ProjectWords = ProjectWords {
    kinds: &[("track", "tracks"), ("scene", "scenes"), ("clip", "clips"), ("device", "devices"), ("locator", "locators")],
    project: "set",
    rank: &["set", "track", "scene", "device", "clip", "locator"],
    opaque: &["device"],
};
/// A snapshot's records by their ids, borrowed from its pages: a diff names only a few of them.
fn records(pages: &[JsonObject]) -> IndexMap<&str, &Value> {
    let mut rows = IndexMap::new();
    for page in pages {
        for item in page.get("records").and_then(Value::as_array).into_iter().flatten() {
            if let Some(id) = item["snapshotId"].as_str() {
                rows.insert(id, item);
            }
        }
    }
    rows
}
fn js_text(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            items.iter().map(|v| if v.is_null() { String::new() } else { js_text(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(v) => stringify(v),
    }
}
static DERIVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/(parentSnapshotId|structureHash|trackCount|sceneCount|clipCount|deviceCount)$|/hash$").unwrap());
fn pick<'a>(ids: &Value, from: &IndexMap<&str, &'a Value>) -> Vec<&'a Value> {
    ids.as_array().into_iter().flatten().filter_map(|id| from.get(js_text(Some(id)).as_str()).copied()).collect()
}
/// Live's semantic diff as ProjectChanges, one for each item the descriptions read, in order. `watching` adds what
/// watch_me says beside catch-up: the track each thing is on, what an added thing is made of, each detail's values.
fn changes_of(diff: &JsonObject, before: &[JsonObject], after: &[JsonObject], watching: bool) -> Vec<ProjectChange> {
    let was = records(before);
    let now = records(after);
    let mut tracks = IndexMap::new();
    if watching {
        tracks = track_coordinates(&was);
        tracks.extend(track_coordinates(&now));
    }
    let item = |row: &Value| ProjectItem {
        name: row.get("name").cloned(),
        order: row.get("order").cloned(),
        on: row["data"]["parentSnapshotId"].as_str().and_then(|parent| tracks.get(parent)).cloned(),
    };
    let mut changes = vec![];
    for entry in diff.get("items").and_then(Value::as_array).into_iter().flatten() {
        let kind = js_text(entry.get("kind"));
        if entry["type"] == "ambiguity" {
            let before = pick(&entry["beforeSnapshotIds"], &was).into_iter().map(item).collect();
            let after = pick(&entry["afterSnapshotIds"], &now).into_iter().map(item).collect();
            changes.push(ProjectChange::Unclear { kind, before, after });
            continue;
        }
        if entry["type"] != "change" {
            continue;
        }
        let Some(facets) = entry["facets"].as_array() else { continue };
        let old = entry["beforeSnapshotId"].as_str().and_then(|id| was.get(id)).copied();
        let current = entry["afterSnapshotId"].as_str().and_then(|id| now.get(id)).copied();
        if facets.iter().any(|v| v == "added") {
            if let Some(current) = current {
                let made_of = if watching { made_of(&kind, current) } else { JsonObject::new() };
                changes.push(ProjectChange::Added { item: item(current), made_of, kind });
                continue;
            }
        }
        if facets.iter().any(|v| v == "removed") {
            if let Some(old) = old {
                changes.push(ProjectChange::Removed { item: item(old), kind });
                continue;
            }
        }
        let details: Vec<_> = entry["details"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["path"].as_str().is_some_and(|p| !DERIVED.is_match(p)))
            .collect();
        // The Set itself: catch-up says its tempo, nothing else.
        let lines = (kind == "set").then(|| {
            details
                .iter()
                .filter(|d| d["path"] == "/data/tempo" && d["before"].is_number() && d["after"].is_number())
                .map(|d| format!("Tempo {} → {} BPM", js_text(d.get("before")), js_text(d.get("after"))))
                .collect()
        });
        let details = details
            .into_iter()
            .map(|d| {
                let path = d["path"].as_str().unwrap_or("");
                Detail {
                    facet: path.split('/').nth(2).unwrap_or("").to_owned(),
                    what: path.strip_prefix("/data/").unwrap_or(path).replace('/', "."),
                    from: if watching { small(d.get("before")).unwrap_or(Value::Null) } else { Value::Null },
                    to: if watching { small(d.get("after")).unwrap_or(Value::Null) } else { Value::Null },
                    fingerprint: path.ends_with("Hash") || path.ends_with("Fingerprint") || path.ends_with("/hash"),
                }
            })
            .collect();
        let renamed = facets.iter().any(|v| v == "renamed");
        changes.push(ProjectChange::Changed { kind, before: old.map(item), after: current.map(item), renamed, details, lines });
    }
    changes
}
/// Catch-up's lines from Live's semantic diff (core's describe_changes).
pub fn describe_diff(diff: &JsonObject, before: &[JsonObject], after: &[JsonObject], limit: Option<usize>) -> DescribedDiff {
    describe_changes(&changes_of(diff, before, after, false), &LIVE_WORDS, limit)
}

fn numeric(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Null) => 0.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(v)) => f64::from(*v),
        Some(Value::String(s)) => kumi_common::js::number::parse(s).unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}
fn track_coordinates(rows: &IndexMap<&str, &Value>) -> IndexMap<String, String> {
    let mut tracks: Vec<_> = rows.values().filter(|r| r["kind"] == "track").collect();
    tracks.sort_by(|a, b| (numeric(a.get("order")) - numeric(b.get("order"))).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut counts: IndexMap<String, usize> = IndexMap::new();
    let mut names = IndexMap::new();
    for row in tracks {
        let data = &row["data"];
        let canonical = json!({"kind":data["kind"],"name":row["name"],"structureHash":data["structureHash"]});
        let base = format!("track-snapshot:{}", &hex::encode(Sha256::digest(stringify(&canonical)))[..20]);
        let count = counts.entry(base.clone()).or_default();
        *count += 1;
        names.insert(format!("{base}-{count}"), row["name"].as_str().unwrap_or("(unnamed)").to_owned());
    }
    names
}
fn made_of(kind: &str, row: &Value) -> JsonObject {
    let data = &row["data"];
    let location = &data["location"];
    let name = row["name"].as_str().filter(|n| !n.is_empty() && *n != "unavailable");
    let mut made = JsonObject::new();
    if let Some(name) = name {
        made.insert("name".into(), json!(name));
    }
    match kind {
        "track" => {
            made.insert("trackKind".into(), data["kind"].clone());
            if let Some(routing) = small(data.get("routing")) {
                made.insert("routing".into(), routing);
            }
            made.insert("armed".into(), data["armed"].clone());
            made.insert("monitoring".into(), data["monitoring"].clone());
            let mut mixer = JsonObject::new();
            for key in ["volume", "pan", "sends", "mute", "solo"] {
                if let Some(value) = data["mixer"].get(key).filter(|v| !v.is_null()) {
                    mixer.insert(key.into(), value.clone());
                }
            }
            made.insert("mixer".into(), json!(mixer));
        }
        "device" => {
            made.insert("className".into(), data["className"].clone());
            made.insert("position".into(), data["siblingOrder"].clone());
            if data["depth"].as_f64().is_some_and(|n| n > 0.0) {
                made.insert("insideRack".into(), json!(true));
            }
        }
        "clip" => {
            made.insert("clipKind".into(), data["clipKind"].clone());
            made.insert("lane".into(), location["lane"].clone());
            if location["sceneOrder"].is_number() {
                made.insert("scene".into(), location["sceneOrder"].clone());
            }
            made.insert("start".into(), data["start"].clone());
            made.insert("length".into(), data["length"].clone());
        }
        _ => {}
    }
    made
}
/// watch_me's changes from Live's semantic diff (core's describe_watch_changes).
pub fn describe_watch(diff: &JsonObject, before: &[JsonObject], after: &[JsonObject], limit: Option<usize>) -> DescribedWatch {
    describe_watch_changes(&changes_of(diff, before, after, true), &LIVE_WORDS, limit)
}
