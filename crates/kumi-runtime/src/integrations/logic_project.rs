//! Logic Pro's saved projects, for catching up and watch_me: how Logic's templates and project folders are told, its
//! names for what a project holds, the snapshot Kumi keeps of a project (from the bridge's rows), and what changed
//! between two snapshots. Logic keeps no id Kumi could put inside a project, and its refs are places in one epoch,
//! so a snapshot keeps no ref: tracks are matched by name and order, regions by their track and start, plug-ins by
//! their track and slot, markers by position, and the tempo and signatures are the project's own lines. What can't be
//! told apart is said to be unclear. The bridge's project id is a hash of the package's path: a project moved or saved
//! under another name is another project here, as it is for memory.
use super::own_project::{KindRows, OwnProject};
use crate::core::{
    contracts::JsonObject,
    describe::{small, Detail, ProjectChange, ProjectItem, ProjectWords},
    project::ProjectRules,
};
use indexmap::IndexMap;
use kumi_common::js::number::to_string;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Logic's names for what a project holds: no Set, scene or clip.
pub const LOGIC_WORDS: ProjectWords = ProjectWords {
    kinds: &[("track", "tracks"), ("region", "regions"), ("plug-in", "plug-ins"), ("marker", "markers"), ("tempo event", "tempo events")],
    project: "project",
    rank: &["project", "track", "plug-in", "region", "marker", "tempo event"],
    // A plug-in's settings aren't read yet: a change of one counts with no detail.
    opaque: &["plug-in"],
};

/// The kinds a snapshot reads through the bridge, the project's row first.
pub const READS: &[&str] = &["project", "track", "region", "plugin", "marker", "tempo"];

/// What tells a snapshot Kumi kept of a Logic project, and its shape's version.
const SNAPSHOT: &str = "logic";
const VERSION: u64 = 1;
/// Logic's resolution: a position within a tick of another is the same.
const TICKS_PER_BEAT: f64 = 960.0;
/// A fader moved by less is where it was (the bridge reads tenths of a dB).
const CLOSE_DB: f64 = 0.05;
/// The most places matched by name in order the long way; past it, by name alone.
const MOST_CELLS: usize = 1_000_000;

/// Logic's templates and project folders. A project in the user's Project Templates folder (`~/Music/Audio Music
/// Apps/Project Templates`) is a template; a project's folder is its `.logicx` package, or the folder holding it when
/// the project is organised as a folder (its `Audio Files`, `Bounces` or `Freeze Files` beside the package).
pub struct LogicRules;

impl ProjectRules for LogicRules {
    fn template_location(&self, path: &str) -> bool {
        let parts: Vec<String> = Path::new(path).components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
        parts.windows(2).any(|pair| pair[0] == "audio music apps" && pair[1] == "project templates")
    }
    fn project_folder(&self, path: &str) -> Option<PathBuf> {
        let package =
            Path::new(path).ancestors().find(|at| at.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("logicx")))?;
        let holder = package.parent()?;
        let in_folder = ["Audio Files", "Bounces", "Freeze Files"].iter().any(|beside| holder.join(beside).is_dir());
        Some(if in_folder { holder } else { package }.to_path_buf())
    }
}

/// watch_me in Logic's words.
const WATCH_DESCRIPTION: &str = "Learn a routine the producer does by hand in Logic Pro: action start just before they do it (Kumi notes the project as it is; nothing in Logic Pro changes), then action stop when they say they're done. Stop gives what changed, each with its track: tracks added, renamed, moved or deleted (with their kind and channel strip: volume, pan, mute, solo, output), regions added, moved, resized, renamed or deleted, plug-ins inserted, removed, moved or bypassed (their settings aren't compared yet), markers, tempo events, and the project's tempo, time signature and key. Save it at once with save_recipe as the make_changes steps that would redo it, with $blanks for what differs each time (the track, say), leaving out anything clearly unrelated. Then say in a few words what the recipe does. The producer can ask you to change it.";

/// What Logic gives the shared integration for catching up and watch_me.
pub const PROJECT: OwnProject = OwnProject {
    rules: &LogicRules,
    words: &LOGIC_WORDS,
    reads: READS,
    snapshot,
    changes,
    watch_description: WATCH_DESCRIPTION,
    // (M6) The bridge's bulk read: one look at the project, its Mixer and List Editors shown once.
    bulk: true,
};

// ----- the snapshot -----

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn flag(value: &Value, key: &str) -> bool {
    value.get(key) == Some(&Value::Bool(true))
}

/// The first bar, as Logic places the project's tempo and signature.
fn start_position() -> Value {
    json!({"bar": 1, "beat": 1, "division": 1, "tick": 1})
}

/// A position as Logic shows it, whole: bar, beat, division and tick, each from 1.
fn position(value: &Value) -> Value {
    let part = |key: &str| value.get(key).and_then(Value::as_i64).unwrap_or(1);
    json!({"bar": part("bar"), "beat": part("beat"), "division": part("division"), "tick": part("tick")})
}

/// A position's parts, in order: bar, beat, division, tick.
type Place = (i64, i64, i64, i64);

/// A position's parts, in order.
fn place_of(position: &Value) -> Place {
    let part = |key: &str| position.get(key).and_then(Value::as_i64).unwrap_or(1);
    (part("bar"), part("beat"), part("division"), part("tick"))
}

/// A position as a producer says it: `bar 9`, `bar 9 beat 3`, `bar 9 beat 3.2.121`.
fn at_text(position: &Value) -> String {
    match place_of(position) {
        (bar, 1, 1, 1) => format!("bar {bar}"),
        (bar, beat, 1, 1) => format!("bar {bar} beat {beat}"),
        (bar, beat, division, tick) => format!("bar {bar} beat {beat}.{division}.{tick}"),
    }
}

/// A position in beats, on Logic's ticks, as a key.
fn ticks(beats: f64) -> i64 {
    (beats * TICKS_PER_BEAT).round() as i64
}

/// The snapshot Kumi keeps of a project, from the bridge's rows of each kind: the tracks in the Tracks area's order,
/// each region and plug-in on its track by the track's place, the markers, the tempo list and the signature list. No
/// ref is kept (a ref is a place in one epoch), nor what Logic shows but the project doesn't keep (the selection, the
/// playhead) or reads only sometimes (the sample rate).
fn snapshot(rows: &[KindRows]) -> Result<JsonObject, String> {
    let of = |kind: &str| rows.iter().find(|(read, _)| *read == kind).map(|(_, rows)| rows.as_slice()).unwrap_or(&[]);
    let project = of("project").first().ok_or("Logic Pro's bridge named no project")?;
    let track_rows = of("track");
    // Each track's place (from 1) by its ref in this epoch.
    let places: HashMap<&str, usize> =
        track_rows.iter().enumerate().filter_map(|(at, row)| Some((row.get("ref")?.as_str()?, at + 1))).collect();
    let tracks: Vec<Value> = track_rows
        .iter()
        .enumerate()
        .map(|(at, row)| {
            let mut track = json!({
                "order": at + 1, "name": text(row, "name"), "kind": text(row, "kind"), "volumeDb": number(row, "volumeDb"),
                "pan": row.get("pan").cloned().unwrap_or(Value::Null), "mute": flag(row, "mute"), "solo": flag(row, "solo"),
                "output": text(row, "output"), "inputs": row.get("inputs").cloned().unwrap_or_else(|| json!([])),
            });
            if flag(row, "stack") {
                track["stack"] = json!(true);
            }
            // (M6) A strip the Mixer hadn't drawn when it was read: its routing and plug-ins weren't read, not empty.
            if flag(row, "stripUnread") {
                track["stripUnread"] = json!(true);
            }
            track
        })
        .collect();
    let on_track = |row: &Value| row.get("trackRef").and_then(Value::as_str).and_then(|reference| places.get(reference)).copied();
    let mut regions: Vec<Value> = of("region")
        .iter()
        .filter_map(|row| {
            Some(json!({
                "track": on_track(row)?, "name": text(row, "name"), "startBeats": number(row, "startBeats")?,
                "endBeats": number(row, "endBeats")?, "kind": text(row, "kind"),
            }))
        })
        .collect();
    regions.sort_by(|a, b| {
        (a["track"].as_u64(), ticks(a["startBeats"].as_f64().unwrap_or(0.0)), ticks(a["endBeats"].as_f64().unwrap_or(0.0))).cmp(&(
            b["track"].as_u64(),
            ticks(b["startBeats"].as_f64().unwrap_or(0.0)),
            ticks(b["endBeats"].as_f64().unwrap_or(0.0)),
        ))
    });
    let mut plugins: Vec<Value> = of("plugin")
        .iter()
        .filter_map(|row| {
            Some(json!({
                "track": on_track(row)?, "slot": row.get("slot").and_then(Value::as_u64)?, "name": text(row, "name"),
                "bypassed": flag(row, "bypassed"), "kind": text(row, "kind"),
            }))
        })
        .collect();
    plugins.sort_by_key(|plugin| (plugin["track"].as_u64(), plugin["slot"].as_u64()));
    let mut markers: Vec<Value> = of("marker")
        .iter()
        .map(|row| {
            json!({"position": position(row.get("position").unwrap_or(&Value::Null)), "name": text(row, "name"), "length": number(row, "length")})
        })
        .collect();
    markers.sort_by_key(|marker| place_of(&marker["position"]));
    let tempo = of("tempo").first().cloned().unwrap_or(Value::Null);
    // The Tempo List: its events when the bridge lists them, else the project's tempo at the first bar.
    let mut tempos: Vec<Value> = tempo
        .get("tempos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|event| Some(json!({"position": position(event.get("position")?), "bpm": number(event, "bpm")?})))
        .collect();
    if tempos.is_empty() {
        if let Some(bpm) = number(&tempo, "bpm").or_else(|| number(project, "tempo")) {
            tempos.push(json!({"position": start_position(), "bpm": bpm}));
        }
    }
    tempos.sort_by_key(|event| place_of(&event["position"]));
    let mut signatures: Vec<Value> = tempo
        .get("signatures")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|event| {
            json!({"position": position(event.get("position").unwrap_or(&Value::Null)), "time": text(event, "time"), "key": text(event, "key")})
        })
        .collect();
    if signatures.is_empty() {
        signatures
            .push(json!({"position": start_position(), "time": text(project, "timeSignature"), "key": text(project, "keySignature")}));
    }
    signatures.sort_by_key(|event| place_of(&event["position"]));
    // (M6) The Stereo Out's plug-ins: an output strip no track has (its rows' trackRef is `<epoch>:output:1`), the
    // project's own, in slot order.
    let mut stereo_out: Vec<Value> = of("plugin")
        .iter()
        .filter(|row| row.get("trackRef").and_then(Value::as_str).is_some_and(|reference| reference.split(':').nth(1) == Some("output")))
        .filter_map(|row| {
            Some(json!({
                "slot": row.get("slot").and_then(Value::as_u64)?, "name": text(row, "name"), "bypassed": flag(row, "bypassed"),
                "kind": text(row, "kind"),
            }))
        })
        .collect();
    stereo_out.sort_by_key(|plugin| plugin["slot"].as_u64());
    let snapshot = json!({
        "snapshot": SNAPSHOT, "version": VERSION, "tracks": tracks, "regions": regions, "plugins": plugins, "markers": markers,
        "tempos": tempos, "signatures": signatures, "stereoOut": stereo_out,
    });
    Ok(snapshot.as_object().cloned().unwrap_or_default())
}

// ----- matching -----

/// Two lists matched in order by their keys: the longest run of keys both keep in the same order, a key each list holds
/// once weighing more (so a unique name anchors), as pairs of places. Past `MOST_CELLS`, the k-th of each key with
/// the k-th, kept in order.
fn in_order(before: &[String], after: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (before.len(), after.len());
    if n == 0 || m == 0 {
        return vec![];
    }
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
    for key in before {
        counts.entry(key.as_str()).or_default().0 += 1;
    }
    for key in after {
        counts.entry(key.as_str()).or_default().1 += 1;
    }
    let weight = |key: &str| if counts.get(key) == Some(&(1, 1)) { 2 } else { 1 };
    if n.saturating_mul(m) > MOST_CELLS {
        let mut seen: HashMap<&str, Vec<usize>> = HashMap::new();
        for (j, key) in after.iter().enumerate() {
            seen.entry(key.as_str()).or_default().push(j);
        }
        let mut taken: HashMap<&str, usize> = HashMap::new();
        let (mut pairs, mut last) = (vec![], None::<usize>);
        for (i, key) in before.iter().enumerate() {
            let k = taken.entry(key.as_str()).or_default();
            if let Some(&j) = seen.get(key.as_str()).and_then(|places| places.get(*k)) {
                *k += 1;
                if last.is_none_or(|last| j > last) {
                    pairs.push((i, j));
                    last = Some(j);
                }
            }
        }
        return pairs;
    }
    let mut score = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let mut best = score[at(i + 1, j)].max(score[at(i, j + 1)]);
            if before[i] == after[j] {
                best = best.max(score[at(i + 1, j + 1)] + weight(&before[i]));
            }
            score[at(i, j)] = best;
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, vec![]);
    while i < n && j < m {
        if before[i] == after[j] && score[at(i, j)] == score[at(i + 1, j + 1)] + weight(&before[i]) {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if score[at(i + 1, j)] >= score[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Each place's gap: how many of the pairs come before it on its side.
fn gaps(len: usize, matched: &[usize]) -> Vec<usize> {
    (0..len).map(|place| matched.iter().filter(|other| **other < place).count()).collect()
}

/// How two lists of one kind matched.
#[derive(Default)]
struct Matched {
    /// Pairs of places, and whether each moved out of the others' order.
    pairs: Vec<(usize, usize, bool)>,
    removed: Vec<usize>,
    added: Vec<usize>,
    /// Places that share a key on both sides and moved, so which is which can't be told.
    unclear: Vec<(Vec<usize>, Vec<usize>)>,
}

/// Matches two lists by key (`name` for tracks and plug-ins), in order: the longest run in the same order; then a key
/// left on both sides that each list holds once is that thing moved, and one held more than once is unclear; then
/// what's left between the same two pairs is paired in order when both sides left as many there and `alike` takes
/// each pair (a rename), else removed and added.
fn match_lists(before: &[String], after: &[String], alike: impl Fn(usize, usize) -> bool) -> Matched {
    let kept = in_order(before, after);
    let (mut left_before, mut left_after): (Vec<usize>, Vec<usize>) = (
        (0..before.len()).filter(|i| !kept.iter().any(|(b, _)| b == i)).collect(),
        (0..after.len()).filter(|j| !kept.iter().any(|(_, a)| a == j)).collect(),
    );
    let mut matched = Matched { pairs: kept.iter().map(|(b, a)| (*b, *a, false)).collect(), ..Matched::default() };
    let count = |list: &[String], key: &str| list.iter().filter(|other| *other == key).count();
    let mut keys: Vec<&String> =
        left_before.iter().map(|i| &before[*i]).filter(|key| left_after.iter().any(|j| after[*j] == **key)).collect();
    keys.dedup();
    let mut done: Vec<&String> = vec![];
    for key in keys {
        if done.contains(&key) {
            continue;
        }
        done.push(key);
        let olds: Vec<usize> = left_before.iter().copied().filter(|i| before[*i] == *key).collect();
        let news: Vec<usize> = left_after.iter().copied().filter(|j| after[*j] == *key).collect();
        if count(before, key) == 1 && count(after, key) == 1 {
            matched.pairs.push((olds[0], news[0], true));
        } else {
            matched.unclear.push((olds.clone(), news.clone()));
        }
        left_before.retain(|i| !olds.contains(i));
        left_after.retain(|j| !news.contains(j));
    }
    let gap_before = gaps(before.len(), &kept.iter().map(|(b, _)| *b).collect::<Vec<_>>());
    let gap_after = gaps(after.len(), &kept.iter().map(|(_, a)| *a).collect::<Vec<_>>());
    let mut by_gap: IndexMap<usize, (Vec<usize>, Vec<usize>)> = IndexMap::new();
    for i in &left_before {
        by_gap.entry(gap_before[*i]).or_default().0.push(*i);
    }
    for j in &left_after {
        by_gap.entry(gap_after[*j]).or_default().1.push(*j);
    }
    for (_, (olds, news)) in by_gap {
        if olds.len() == news.len() && olds.iter().zip(&news).all(|(b, a)| alike(*b, *a)) {
            matched.pairs.extend(olds.iter().zip(&news).map(|(b, a)| (*b, *a, false)));
        } else {
            matched.removed.extend(olds);
            matched.added.extend(news);
        }
    }
    matched.pairs.sort();
    matched.removed.sort();
    matched.added.sort();
    matched
}

// ----- the changes -----

fn list<'a>(snapshot: &'a JsonObject, key: &str) -> &'a [Value] {
    snapshot.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}
fn names(rows: &[&Value]) -> Vec<String> {
    rows.iter().map(|row| text(row, "name").to_owned()).collect()
}
fn item(name: &str, order: Value, on: Option<&str>) -> ProjectItem {
    ProjectItem { name: Some(json!(name)), order: Some(order), on: on.map(str::to_owned) }
}
/// A detail of a change: its facet for catch-up, what changed by name and its values for watch_me.
fn detail(facet: &str, what: &str, from: &Value, to: &Value, watching: bool) -> Detail {
    Detail {
        facet: facet.into(),
        what: what.into(),
        from: if watching { small(Some(from)).unwrap_or(Value::Null) } else { Value::Null },
        to: if watching { small(Some(to)).unwrap_or(Value::Null) } else { Value::Null },
        fingerprint: false,
    }
}
fn bpm_text(bpm: f64) -> String {
    to_string((bpm * 100.0).round() / 100.0)
}

/// What changed from one snapshot of a Logic project to another, in the order catch-up tells it: the project's own
/// lines (the tempo, the time signature, the key), then the tracks, plug-ins, regions, markers and tempo events.
/// `watching` adds what watch_me tells: what an added thing is made of, and each detail's values.
pub fn changes(before: &JsonObject, after: &JsonObject, watching: bool) -> Vec<ProjectChange> {
    // A baseline in another shape than this one is no baseline to compare with.
    let shaped =
        |snapshot: &JsonObject| snapshot.get("snapshot") == Some(&json!(SNAPSHOT)) && snapshot.get("version") == Some(&json!(VERSION));
    if !shaped(before) || !shaped(after) {
        return vec![];
    }
    let mut out = vec![];
    project_lines(before, after, watching, &mut out);
    let was: Vec<&Value> = list(before, "tracks").iter().collect();
    let now: Vec<&Value> = list(after, "tracks").iter().collect();
    let tracks = match_lists(&names(&was), &names(&now), |b, a| text(was[b], "kind") == text(now[a], "kind"));
    let order = |row: &Value| row["order"].as_u64().unwrap_or(0);
    let track_item = |row: &Value| item(text(row, "name"), json!(order(row)), None);
    for (olds, news) in &tracks.unclear {
        out.push(ProjectChange::Unclear {
            kind: "track".into(),
            before: olds.iter().map(|b| track_item(was[*b])).collect(),
            after: news.iter().map(|a| track_item(now[*a])).collect(),
        });
    }
    for b in &tracks.removed {
        out.push(ProjectChange::Removed { kind: "track".into(), item: track_item(was[*b]) });
    }
    for a in &tracks.added {
        let made_of = if watching { track_made_of(now[*a]) } else { JsonObject::new() };
        out.push(ProjectChange::Added { kind: "track".into(), item: track_item(now[*a]), made_of });
    }
    for (b, a, moved) in &tracks.pairs {
        let (old, new) = (was[*b], now[*a]);
        let mut details = vec![];
        if *moved {
            details.push(detail("order", "order", &old["order"], &new["order"], watching));
        }
        if text(old, "kind") != text(new, "kind") {
            details.push(detail("kind", "kind", &old["kind"], &new["kind"], watching));
        }
        let fader = |row: &Value| number(row, "volumeDb");
        let faders_moved = match (fader(old), fader(new)) {
            (Some(x), Some(y)) => (x - y).abs() > CLOSE_DB,
            (x, y) => x.is_some() != y.is_some(),
        };
        if faders_moved {
            details.push(detail("mixer", "volumeDb", &old["volumeDb"], &new["volumeDb"], watching));
        }
        for what in ["pan", "mute", "solo"] {
            if old[what] != new[what] {
                details.push(detail("mixer", what, &old[what], &new[what], watching));
            }
        }
        // (a strip read on one side only says nothing of its routing)
        let unread = flag(old, "stripUnread") || flag(new, "stripUnread");
        for what in ["output", "inputs"].into_iter().filter(|_| !unread) {
            if old[what] != new[what] {
                details.push(detail("routing", what, &old[what], &new[what], watching));
            }
        }
        let renamed = text(old, "name") != text(new, "name");
        if renamed || !details.is_empty() {
            out.push(ProjectChange::Changed {
                kind: "track".into(),
                before: Some(track_item(old)),
                after: Some(track_item(new)),
                renamed,
                details,
                lines: None,
            });
        }
    }
    // What's on a track goes with it: the matched tracks' plug-ins and regions are compared, an added track's are added,
    // and a removed or unclear track's aren't told again.
    let paired: Vec<(u64, u64)> = tracks.pairs.iter().map(|(b, a, _)| (order(was[*b]), order(now[*a]))).collect();
    // (M6) A track whose strip the Mixer hadn't drawn on either side has its plug-ins left out: they weren't read.
    let strips_read: Vec<(u64, u64)> = tracks
        .pairs
        .iter()
        .filter(|(b, a, _)| !flag(was[*b], "stripUnread") && !flag(now[*a], "stripUnread"))
        .map(|(b, a, _)| (order(was[*b]), order(now[*a])))
        .collect();
    let added_tracks: Vec<u64> = tracks.added.iter().map(|a| order(now[*a])).collect();
    let name_now: HashMap<u64, &str> = now.iter().map(|row| (order(row), text(row, "name"))).collect();
    plugin_changes(before, after, &strips_read, &added_tracks, &name_now, watching, &mut out);
    region_changes(before, after, &paired, &added_tracks, &name_now, watching, &mut out);
    marker_changes(before, after, watching, &mut out);
    tempo_event_changes(before, after, watching, &mut out);
    out
}

fn track_made_of(row: &Value) -> JsonObject {
    let mut made = JsonObject::new();
    made.insert("name".into(), row["name"].clone());
    made.insert("trackKind".into(), row["kind"].clone());
    let mut mixer = JsonObject::new();
    for key in ["volumeDb", "pan", "mute", "solo"] {
        if let Some(value) = row.get(key).filter(|value| !value.is_null()) {
            mixer.insert(key.into(), value.clone());
        }
    }
    made.insert("mixer".into(), json!(mixer));
    for key in ["output", "inputs"] {
        if let Some(value) = small(row.get(key)).filter(|value| !value.is_null()) {
            made.insert(key.into(), value);
        }
    }
    if flag(row, "stack") {
        made.insert("trackStack".into(), json!(true));
    }
    made
}

/// The project's own lines: its tempo, time signature and key at the first bar, and each signature change after it.
fn project_lines(before: &JsonObject, after: &JsonObject, watching: bool, out: &mut Vec<ProjectChange>) {
    let (mut lines, mut details) = (vec![], vec![]);
    let first_bpm = |snapshot: &JsonObject| list(snapshot, "tempos").first().and_then(|event| number(event, "bpm"));
    if let (Some(was), Some(now)) = (first_bpm(before), first_bpm(after)) {
        if (was - now).abs() >= 0.005 {
            lines.push(format!("Tempo {} → {} BPM", bpm_text(was), bpm_text(now)));
            details.push(detail("tempo", "tempo", &json!(was), &json!(now), watching));
        }
    }
    let (was, now) = (list(before, "signatures"), list(after, "signatures"));
    let at = |events: &[Value], place: Place| events.iter().find(|event| place_of(&event["position"]) == place).cloned();
    let first = (1, 1, 1, 1);
    if let (Some(old), Some(new)) = (at(was, first), at(now, first)) {
        if text(&old, "time") != text(&new, "time") {
            lines.push(format!("Time signature {} → {}", text(&old, "time"), text(&new, "time")));
            details.push(detail("signature", "timeSignature", &old["time"], &new["time"], watching));
        }
        if text(&old, "key") != text(&new, "key") {
            lines.push(format!("Key {} → {}", text(&old, "key"), text(&new, "key")));
            details.push(detail("signature", "keySignature", &old["key"], &new["key"], watching));
        }
    }
    // The changes after the first bar, by position: each told as Logic's Signature List would show it now.
    let mut places: Vec<Place> = was.iter().chain(now).map(|event| place_of(&event["position"])).filter(|place| *place != first).collect();
    places.sort();
    places.dedup();
    for place in places {
        let (old, new) = (at(was, place), at(now, place));
        let where_ = at_text(&old.as_ref().or(new.as_ref()).map(|event| event["position"].clone()).unwrap_or(Value::Null));
        let what = format!("signature at {where_}");
        match (old, new) {
            (None, Some(new)) => {
                lines.push(format!("Signature {}, {} from {where_}", text(&new, "time"), text(&new, "key")));
                details.push(detail(
                    "signature",
                    &what,
                    &Value::Null,
                    &json!(format!("{} {}", text(&new, "time"), text(&new, "key"))),
                    watching,
                ));
            }
            (Some(old), None) => {
                lines.push(format!("Signature change at {where_} removed"));
                details.push(detail(
                    "signature",
                    &what,
                    &json!(format!("{} {}", text(&old, "time"), text(&old, "key"))),
                    &Value::Null,
                    watching,
                ));
            }
            (Some(old), Some(new)) if old != new => {
                let (from, to) =
                    (format!("{} {}", text(&old, "time"), text(&old, "key")), format!("{} {}", text(&new, "time"), text(&new, "key")));
                lines.push(format!("Signature at {where_} {from} → {to}"));
                details.push(detail("signature", &what, &json!(from), &json!(to), watching));
            }
            _ => {}
        }
    }
    stereo_out_lines(before, after, watching, &mut lines, &mut details);
    if !lines.is_empty() {
        out.push(ProjectChange::Changed {
            kind: LOGIC_WORDS.project.into(),
            before: None,
            after: None,
            renamed: false,
            details,
            lines: Some(lines),
        });
    }
}

/// (M6) The Stereo Out's plug-ins, the project's own lines: matched by name in slot order, each one added, removed, or
/// bypassed or back on. Compared only when both snapshots read them (a baseline Kumi kept before it did says nothing).
fn stereo_out_lines(before: &JsonObject, after: &JsonObject, watching: bool, lines: &mut Vec<String>, details: &mut Vec<Detail>) {
    let (Some(was), Some(now)) = (before.get("stereoOut").and_then(Value::as_array), after.get("stereoOut").and_then(Value::as_array))
    else {
        return;
    };
    let (old, new): (Vec<&Value>, Vec<&Value>) = (was.iter().collect(), now.iter().collect());
    let matched = match_lists(&names(&old), &names(&new), |_, _| false);
    let named = |row: &Value| quoted(text(row, "name"));
    for b in &matched.removed {
        lines.push(format!("Removed plug-in {} from the Stereo Out", named(old[*b])));
        details.push(detail("stereoOut", "plug-in", &old[*b]["name"], &Value::Null, watching));
    }
    for a in &matched.added {
        lines.push(format!("Added plug-in {} on the Stereo Out", named(new[*a])));
        details.push(detail("stereoOut", "plug-in", &Value::Null, &new[*a]["name"], watching));
    }
    for (olds, news) in &matched.unclear {
        let all = |rows: &Vec<&Value>, at: &[usize]| at.iter().map(|at| named(rows[*at])).collect::<Vec<_>>().join(", ");
        lines.push(format!("The Stereo Out's plug-ins changed ({} → {})", all(&old, olds), all(&new, news)));
    }
    for (b, a, _) in &matched.pairs {
        if old[*b]["bypassed"] != new[*a]["bypassed"] {
            let state = if flag(new[*a], "bypassed") { "bypassed" } else { "back on" };
            lines.push(format!("Plug-in {} on the Stereo Out {state}", named(new[*a])));
            details.push(detail(
                "stereoOut",
                &format!("{} bypassed", text(new[*a], "name")),
                &old[*b]["bypassed"],
                &new[*a]["bypassed"],
                watching,
            ));
        }
    }
}

/// A name inside a line, in curly quotes.
fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// Each matched track's plug-ins by slot order (one inserted or removed moves the ones after it), an added track's
/// as added.
fn plugin_changes(
    before: &JsonObject,
    after: &JsonObject,
    paired: &[(u64, u64)],
    added_tracks: &[u64],
    name_now: &HashMap<u64, &str>,
    watching: bool,
    out: &mut Vec<ProjectChange>,
) {
    let on = |snapshot: &JsonObject, track: u64| -> Vec<Value> {
        list(snapshot, "plugins").iter().filter(|plugin| plugin["track"].as_u64() == Some(track)).cloned().collect()
    };
    let slot = |row: &Value| row["slot"].as_u64().unwrap_or(0);
    // (a plug-in's place is its track's place now and its slot: one removed and one added there read as replaced)
    let plugin_item = |row: &Value, track: u64| item(text(row, "name"), json!([track, slot(row)]), name_now.get(&track).copied());
    let made_of = |row: &Value| {
        let mut made = JsonObject::new();
        if watching {
            made.insert("name".into(), row["name"].clone());
            made.insert("pluginKind".into(), row["kind"].clone());
            made.insert("slot".into(), row["slot"].clone());
            if flag(row, "bypassed") {
                made.insert("bypassed".into(), json!(true));
            }
        }
        made
    };
    for (track_was, track_now) in paired {
        let (was, now) = (on(before, *track_was), on(after, *track_now));
        let refs: (Vec<&Value>, Vec<&Value>) = (was.iter().collect(), now.iter().collect());
        let matched = match_lists(&names(&refs.0), &names(&refs.1), |_, _| false);
        for (olds, news) in &matched.unclear {
            out.push(ProjectChange::Unclear {
                kind: "plug-in".into(),
                before: olds.iter().map(|b| plugin_item(&was[*b], *track_now)).collect(),
                after: news.iter().map(|a| plugin_item(&now[*a], *track_now)).collect(),
            });
        }
        for b in &matched.removed {
            out.push(ProjectChange::Removed { kind: "plug-in".into(), item: plugin_item(&was[*b], *track_now) });
        }
        for a in &matched.added {
            out.push(ProjectChange::Added { kind: "plug-in".into(), item: plugin_item(&now[*a], *track_now), made_of: made_of(&now[*a]) });
        }
        for (b, a, moved) in &matched.pairs {
            let (old, new) = (&was[*b], &now[*a]);
            let mut details = vec![];
            if *moved {
                details.push(detail("slot", "slot", &old["slot"], &new["slot"], watching));
            }
            if old["bypassed"] != new["bypassed"] {
                details.push(detail("bypass", "bypassed", &old["bypassed"], &new["bypassed"], watching));
            }
            if !details.is_empty() {
                out.push(ProjectChange::Changed {
                    kind: "plug-in".into(),
                    before: Some(plugin_item(old, *track_now)),
                    after: Some(plugin_item(new, *track_now)),
                    renamed: false,
                    details,
                    lines: None,
                });
            }
        }
    }
    for track in added_tracks {
        for row in on(after, *track) {
            out.push(ProjectChange::Added { kind: "plug-in".into(), item: plugin_item(&row, *track), made_of: made_of(&row) });
        }
    }
}

/// Each matched track's regions by their start; one left on both sides of a track with the same name and length moved
/// along it, with the same name alone moved and resized; same-named ones left over can't be told apart. A region left
/// on one track and found on another (or on a track added) at the same start, with the same name and length, moved
/// there.
fn region_changes(
    before: &JsonObject,
    after: &JsonObject,
    paired: &[(u64, u64)],
    added_tracks: &[u64],
    name_now: &HashMap<u64, &str>,
    watching: bool,
    out: &mut Vec<ProjectChange>,
) {
    let on = |snapshot: &JsonObject, track: u64| -> Vec<Value> {
        list(snapshot, "regions").iter().filter(|region| region["track"].as_u64() == Some(track)).cloned().collect()
    };
    let start = |row: &Value| ticks(row["startBeats"].as_f64().unwrap_or(0.0));
    let length = |row: &Value| ticks(row["endBeats"].as_f64().unwrap_or(0.0)) - start(row);
    // (a region's place is its track's place now and its start: none is left at the same one on both sides)
    let region_item =
        |row: &Value, track: u64| item(text(row, "name"), json!([track, row["startBeats"].clone()]), name_now.get(&track).copied());
    let made_of = |row: &Value| {
        let mut made = JsonObject::new();
        if watching {
            made.insert("name".into(), row["name"].clone());
            made.insert("regionKind".into(), row["kind"].clone());
            made.insert("startBeats".into(), row["startBeats"].clone());
            made.insert("endBeats".into(), row["endBeats"].clone());
        }
        made
    };
    let changed = |old: &Value, new: &Value, track_was: u64, track_now: u64, out: &mut Vec<ProjectChange>| {
        let mut details = vec![];
        if track_was != track_now {
            let named = |track: u64| json!(name_now.get(&track).copied().unwrap_or_default());
            details.push(detail("track", "track", &named(track_was), &named(track_now), watching));
        }
        if start(old) != start(new) {
            details.push(detail("position", "startBeats", &old["startBeats"], &new["startBeats"], watching));
        }
        if length(old) != length(new) {
            let span = |row: &Value| json!(row["endBeats"].as_f64().unwrap_or(0.0) - row["startBeats"].as_f64().unwrap_or(0.0));
            details.push(detail("length", "lengthBeats", &span(old), &span(new), watching));
        }
        let renamed = text(old, "name") != text(new, "name");
        if renamed || !details.is_empty() {
            out.push(ProjectChange::Changed {
                kind: "region".into(),
                before: Some(region_item(old, track_now)),
                after: Some(region_item(new, track_now)),
                renamed,
                details,
                lines: None,
            });
        }
    };
    // What's left on each side, with its track (its place before, and the place it's on now).
    let mut gone: Vec<(Value, u64, u64)> = vec![];
    let mut fresh: Vec<(Value, u64)> = vec![];
    for (track_was, track_now) in paired {
        let (was, now) = (on(before, *track_was), on(after, *track_now));
        // By start: the k-th at one start with the k-th at it.
        let mut starts: IndexMap<i64, (Vec<usize>, Vec<usize>)> = IndexMap::new();
        for (b, row) in was.iter().enumerate() {
            starts.entry(start(row)).or_default().0.push(b);
        }
        for (a, row) in now.iter().enumerate() {
            starts.entry(start(row)).or_default().1.push(a);
        }
        let (mut left_was, mut left_now) = (vec![], vec![]);
        for (_, (olds, news)) in starts {
            for (b, a) in olds.iter().zip(&news) {
                changed(&was[*b], &now[*a], *track_now, *track_now, out);
            }
            left_was.extend(olds.iter().skip(news.len()).copied());
            left_now.extend(news.iter().skip(olds.len()).copied());
        }
        // Moved along the track: the same name and length, each side holding one such.
        let key = |row: &Value| (text(row, "name").to_owned(), length(row));
        let mut moved = vec![];
        for b in &left_was {
            let same: Vec<usize> = left_now.iter().copied().filter(|a| key(&now[*a]) == key(&was[*b])).collect();
            let twins = left_was.iter().filter(|other| key(&was[**other]) == key(&was[*b])).count();
            if same.len() == 1 && twins == 1 {
                moved.push((*b, same[0]));
            }
        }
        for (b, a) in &moved {
            changed(&was[*b], &now[*a], *track_now, *track_now, out);
        }
        left_was.retain(|b| !moved.iter().any(|(old, _)| old == b));
        left_now.retain(|a| !moved.iter().any(|(_, new)| new == a));
        // Moved and resized: one left on each side, of the same name.
        if let ([b], [a]) = (left_was.as_slice(), left_now.as_slice()) {
            if text(&was[*b], "name") == text(&now[*a], "name") {
                changed(&was[*b], &now[*a], *track_now, *track_now, out);
                left_was.clear();
                left_now.clear();
            }
        }
        // The same name left on both sides, more than once: which moved where can't be told.
        let mut unclear_names: Vec<String> = left_was
            .iter()
            .map(|b| text(&was[*b], "name").to_owned())
            .filter(|name| left_now.iter().any(|a| text(&now[*a], "name") == name))
            .collect();
        unclear_names.dedup();
        for name in unclear_names {
            let olds: Vec<usize> = left_was.iter().copied().filter(|b| text(&was[*b], "name") == name).collect();
            let news: Vec<usize> = left_now.iter().copied().filter(|a| text(&now[*a], "name") == name).collect();
            if olds.is_empty() || news.is_empty() {
                continue;
            }
            out.push(ProjectChange::Unclear {
                kind: "region".into(),
                before: olds.iter().map(|b| region_item(&was[*b], *track_now)).collect(),
                after: news.iter().map(|a| region_item(&now[*a], *track_now)).collect(),
            });
            left_was.retain(|b| !olds.contains(b));
            left_now.retain(|a| !news.contains(a));
        }
        gone.extend(left_was.into_iter().map(|b| (was[b].clone(), *track_was, *track_now)));
        fresh.extend(left_now.into_iter().map(|a| (now[a].clone(), *track_now)));
    }
    for track in added_tracks {
        fresh.extend(on(after, *track).into_iter().map(|row| (row, *track)));
    }
    // Moved to another track: the same name, start and length, each side holding one such.
    let key = |row: &Value| (text(row, "name").to_owned(), start(row), length(row));
    let mut across = vec![];
    for (b, (old, _, _)) in gone.iter().enumerate() {
        let same: Vec<usize> = (0..fresh.len()).filter(|a| key(&fresh[*a].0) == key(old)).collect();
        let twins = gone.iter().filter(|(other, _, _)| key(other) == key(old)).count();
        if same.len() == 1 && twins == 1 {
            across.push((b, same[0]));
        }
    }
    for (b, a) in &across {
        let ((old, _, track_was_now), (new, track_now)) = (&gone[*b], &fresh[*a]);
        if track_was_now != track_now {
            changed(old, new, *track_was_now, *track_now, out);
        }
    }
    for (b, (old, _, track_now)) in gone.iter().enumerate() {
        if !across.iter().any(|(moved, _)| *moved == b) {
            out.push(ProjectChange::Removed { kind: "region".into(), item: region_item(old, *track_now) });
        }
    }
    for (a, (new, track)) in fresh.iter().enumerate() {
        if !across.iter().any(|(_, moved)| *moved == a) {
            out.push(ProjectChange::Added { kind: "region".into(), item: region_item(new, *track), made_of: made_of(new) });
        }
    }
}

/// The markers by position; one left on both sides with a name each side holds once moved.
fn marker_changes(before: &JsonObject, after: &JsonObject, watching: bool, out: &mut Vec<ProjectChange>) {
    let (was, now) = (list(before, "markers"), list(after, "markers"));
    let marker_item = |row: &Value| {
        let (bar, beat, division, tick) = place_of(&row["position"]);
        item(text(row, "name"), json!(format!("{bar}.{beat}.{division}.{tick}")), None)
    };
    let made_of = |row: &Value| {
        let mut made = JsonObject::new();
        if watching {
            made.insert("name".into(), row["name"].clone());
            made.insert("position".into(), json!(at_text(&row["position"])));
            made.insert("length".into(), row["length"].clone());
        }
        made
    };
    let changed = |old: &Value, new: &Value, out: &mut Vec<ProjectChange>| {
        let mut details = vec![];
        if place_of(&old["position"]) != place_of(&new["position"]) {
            details.push(detail("position", "position", &json!(at_text(&old["position"])), &json!(at_text(&new["position"])), watching));
        }
        let length = |row: &Value| ticks(row["length"].as_f64().unwrap_or(0.0));
        if length(old) != length(new) {
            details.push(detail("length", "length", &old["length"], &new["length"], watching));
        }
        let renamed = text(old, "name") != text(new, "name");
        if renamed || !details.is_empty() {
            out.push(ProjectChange::Changed {
                kind: "marker".into(),
                before: Some(marker_item(old)),
                after: Some(marker_item(new)),
                renamed,
                details,
                lines: None,
            });
        }
    };
    let mut places: IndexMap<Place, (Vec<usize>, Vec<usize>)> = IndexMap::new();
    for (b, row) in was.iter().enumerate() {
        places.entry(place_of(&row["position"])).or_default().0.push(b);
    }
    for (a, row) in now.iter().enumerate() {
        places.entry(place_of(&row["position"])).or_default().1.push(a);
    }
    let (mut left_was, mut left_now) = (vec![], vec![]);
    for (_, (olds, news)) in places {
        for (b, a) in olds.iter().zip(&news) {
            changed(&was[*b], &now[*a], out);
        }
        left_was.extend(olds.iter().skip(news.len()).copied());
        left_now.extend(news.iter().skip(olds.len()).copied());
    }
    let named =
        |rows: &[usize], list: &[Value], name: &str| rows.iter().copied().filter(|at| text(&list[*at], "name") == name).collect::<Vec<_>>();
    let mut moved = vec![];
    for b in &left_was {
        let name = text(&was[*b], "name");
        let (olds, news) = (named(&left_was, was, name), named(&left_now, now, name));
        if olds.len() == 1 && news.len() == 1 {
            moved.push((*b, news[0]));
        }
    }
    for (b, a) in &moved {
        changed(&was[*b], &now[*a], out);
    }
    for b in left_was.iter().filter(|b| !moved.iter().any(|(old, _)| old == *b)) {
        out.push(ProjectChange::Removed { kind: "marker".into(), item: marker_item(&was[*b]) });
    }
    for a in left_now.iter().filter(|a| !moved.iter().any(|(_, new)| new == *a)) {
        out.push(ProjectChange::Added { kind: "marker".into(), item: marker_item(&now[*a]), made_of: made_of(&now[*a]) });
    }
}

/// The Tempo List's events after the project's tempo, by position.
fn tempo_event_changes(before: &JsonObject, after: &JsonObject, watching: bool, out: &mut Vec<ProjectChange>) {
    let events = |snapshot: &JsonObject| -> IndexMap<Place, Value> {
        list(snapshot, "tempos").iter().skip(1).map(|event| (place_of(&event["position"]), event.clone())).collect()
    };
    let (was, now) = (events(before), events(after));
    let event_item = |event: &Value| {
        let (bar, beat, division, tick) = place_of(&event["position"]);
        let name = format!("{} BPM at {}", bpm_text(event["bpm"].as_f64().unwrap_or(0.0)), at_text(&event["position"]));
        item(&name, json!(format!("{bar}.{beat}.{division}.{tick}")), None)
    };
    for (place, old) in &was {
        match now.get(place) {
            None => out.push(ProjectChange::Removed { kind: "tempo event".into(), item: event_item(old) }),
            Some(new) if (number(old, "bpm").unwrap_or(0.0) - number(new, "bpm").unwrap_or(0.0)).abs() >= 0.005 => {
                out.push(ProjectChange::Changed {
                    kind: "tempo event".into(),
                    before: Some(event_item(old)),
                    after: Some(event_item(new)),
                    renamed: false,
                    details: vec![detail("tempo", "bpm", &old["bpm"], &new["bpm"], watching)],
                    lines: None,
                });
            }
            Some(_) => {}
        }
    }
    for (place, new) in &now {
        if !was.contains_key(place) {
            let mut made = JsonObject::new();
            if watching {
                made.insert("bpm".into(), new["bpm"].clone());
                made.insert("position".into(), json!(at_text(&new["position"])));
            }
            out.push(ProjectChange::Added { kind: "tempo event".into(), item: event_item(new), made_of: made });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::describe::{describe_changes, describe_watch_changes};
    use regex::Regex;

    /// Live's words, whole: none may be said of a Logic project.
    fn live_words(text: &str) -> bool {
        Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Main|[Rr]acks?|[Cc]lips?|[Ss]cenes?)\b").unwrap().is_match(text)
    }

    fn position(bar: i64, beat: i64) -> Value {
        json!({"bar": bar, "beat": beat, "division": 1, "tick": 1})
    }

    /// The bridge's rows of a project, as `snapshot` takes them, in epoch `epoch`: each track `(name, kind)`, each region
    /// `(track place, name, start, end)`, each plug-in `(track place, slot, name, bypassed)`, each marker `(bar, name)`.
    fn rows(
        epoch: u32,
        tracks: &[(&str, &str)],
        regions: &[(usize, &str, f64, f64)],
        plugins: &[(usize, usize, &str, bool)],
        markers: &[(i64, &str)],
        bpm: f64,
    ) -> Vec<KindRows> {
        let r = |kind: &str, n: String| format!("{epoch}:{kind}:{n}");
        vec![
            ("project", vec![json!({"id": "e805ab87b3ca9d66f0f62d47b97caa29", "identity": "Scratch", "path": "/Users/me/Music/Logic/Scratch.logicx", "tempo": bpm, "timeSignature": "4/4", "keySignature": "C Major", "sampleRate": null, "trackCount": tracks.len()})]),
            (
                "track",
                tracks
                    .iter()
                    .enumerate()
                    .map(|(at, (name, kind))| json!({"ref": r("track", (at + 1).to_string()), "index": at + 1, "name": name, "kind": kind, "mute": false, "solo": false, "armed": false, "volumeDb": 0.0, "pan": 0, "output": "Stereo Output", "inputs": [], "selected": at == 0, "color": null}))
                    .collect(),
            ),
            (
                "region",
                regions
                    .iter()
                    .enumerate()
                    .map(|(at, (track, name, start, end))| json!({"ref": r("region", (at + 1).to_string()), "trackRef": r("track", track.to_string()), "name": name, "startBeats": start, "endBeats": end, "kind": "midi", "selected": false, "looped": false}))
                    .collect(),
            ),
            (
                "plugin",
                plugins
                    .iter()
                    .map(|(track, slot, name, bypassed)| json!({"ref": r("plugin", format!("{track}.{slot}")), "trackRef": r("track", track.to_string()), "slot": slot, "name": name, "bypassed": bypassed, "kind": "effect"}))
                    .collect(),
            ),
            (
                "marker",
                markers
                    .iter()
                    .enumerate()
                    .map(|(at, (bar, name))| json!({"ref": r("marker", (at + 1).to_string()), "position": position(*bar, 1), "name": name, "length": 16.0, "text": ""}))
                    .collect(),
            ),
            ("tempo", vec![json!({"bpm": bpm, "signatures": [{"key": "C Major", "position": position(1, 1), "time": "4/4"}]})]),
        ]
    }

    fn said(before: &JsonObject, after: &JsonObject) -> Vec<String> {
        let lines = describe_changes(&changes(before, after, false), &LOGIC_WORDS, None).lines;
        assert!(!lines.iter().any(|line| live_words(line)), "{lines:?}");
        lines
    }

    fn watched(before: &JsonObject, after: &JsonObject) -> Vec<Value> {
        let watched = describe_watch_changes(&changes(before, after, true), &LOGIC_WORDS, None).changes;
        let text = Value::Array(watched.iter().cloned().map(Value::Object).collect());
        assert!(!live_words(&text.to_string()), "{text}");
        text.as_array().unwrap().clone()
    }

    const SCRATCH: [(&str, &str); 3] = [("Vocal", "audio"), ("Keys", "instrument"), ("Bass", "instrument")];

    #[test]
    fn a_snapshot_keeps_no_ref_and_puts_each_region_and_plug_in_on_its_tracks_place() {
        let kept =
            snapshot(&rows(7, &SCRATCH, &[(2, "Keys", 0.0, 16.0)], &[(1, 1, "Channel EQ", false)], &[(9, "Chorus")], 160.0)).unwrap();
        let text = Value::Object(kept.clone()).to_string();
        assert!(!text.contains("7:track:") && !text.contains("\"ref\""), "{text}");
        assert!(!text.contains("selected") && !text.contains("sampleRate"), "what the project doesn't keep: {text}");
        assert_eq!(kept["tracks"][1]["name"], "Keys");
        assert_eq!((kept["regions"][0]["track"].as_u64(), kept["plugins"][0]["track"].as_u64()), (Some(2), Some(1)));
        assert_eq!(kept["tempos"], json!([{"position": position(1, 1), "bpm": 160.0}]), "the project's tempo at the first bar");
        assert_eq!(kept["signatures"][0]["time"], "4/4");
        // The same project read in another epoch is the same snapshot.
        let again =
            snapshot(&rows(8, &SCRATCH, &[(2, "Keys", 0.0, 16.0)], &[(1, 1, "Channel EQ", false)], &[(9, "Chorus")], 160.0)).unwrap();
        assert_eq!(kept, again);
        assert!(changes(&kept, &again, false).is_empty() && changes(&kept, &again, true).is_empty());
        assert!(snapshot(&[("track", vec![])]).is_err(), "no project row, no snapshot");
    }

    #[test]
    fn tracks_match_by_name_and_order_a_rename_holds_its_place_and_kind_and_same_named_ones_moved_are_unclear() {
        let before = snapshot(&rows(1, &SCRATCH, &[], &[], &[], 160.0)).unwrap();
        // Vocal renamed in its place, a Pad added after Keys, Bass deleted.
        let after =
            snapshot(&rows(2, &[("Lead Vocal", "audio"), ("Keys", "instrument"), ("Pad", "instrument")], &[], &[], &[], 160.0)).unwrap();
        // (Bass and Pad hold one place and kind: a rename, by the rule, though the producer may have made Pad anew)
        assert_eq!(said(&before, &after), ["Renamed track “Vocal” → “Lead Vocal”", "Renamed track “Bass” → “Pad”"]);
        let lead_and_pad = snapshot(&rows(
            2,
            &[("Lead Vocal", "audio"), ("Keys", "instrument"), ("Bass", "instrument"), ("Pad", "audio")],
            &[],
            &[],
            &[],
            160.0,
        ))
        .unwrap();
        assert_eq!(said(&before, &lead_and_pad), ["Renamed track “Vocal” → “Lead Vocal”", "Added track “Pad”"]);
        // Another kind in a track's place is another track.
        let other_kind =
            snapshot(&rows(2, &[("Drums", "instrument"), ("Keys", "instrument"), ("Bass", "instrument")], &[], &[], &[], 160.0)).unwrap();
        assert_eq!(said(&before, &other_kind), ["“Vocal” → “Drums” (renamed and changed)"]);
        // A track of its own name moved down: its order changed, nothing else.
        let moved =
            snapshot(&rows(2, &[("Keys", "instrument"), ("Bass", "instrument"), ("Vocal", "audio")], &[], &[], &[], 160.0)).unwrap();
        assert_eq!(said(&before, &moved), ["Changed track “Vocal” (order)"]);
        assert_eq!(
            watched(&before, &moved),
            [json!({"changed": "track", "name": "Vocal", "what": [{"what": "order", "from": 1, "to": 3}]})]
        );
        // Two tracks of one name moved: which is which can't be told, and catch-up says nothing of them.
        let twins = [("Audio", "audio"), ("Keys", "instrument"), ("Audio", "audio")];
        let swapped = [("Keys", "instrument"), ("Audio", "audio"), ("Audio", "audio")];
        let (twins, swapped) =
            (snapshot(&rows(1, &twins, &[], &[], &[], 160.0)).unwrap(), snapshot(&rows(2, &swapped, &[], &[], &[], 160.0)).unwrap());
        let found = changes(&twins, &swapped, false);
        assert!(found.iter().any(|change| matches!(change, ProjectChange::Unclear { kind, .. } if kind == "track")), "{found:?}");
        assert!(said(&twins, &swapped).is_empty());
        assert_eq!(watched(&twins, &swapped), [json!({"unclear": "track", "before": ["Audio"], "after": ["Audio"]})]);
    }

    #[test]
    fn a_tracks_strip_says_what_changed_on_it() {
        let before = snapshot(&rows(1, &SCRATCH, &[], &[], &[], 160.0)).unwrap();
        let mut after_rows = rows(1, &SCRATCH, &[], &[], &[], 160.0);
        after_rows[1].1[0]["volumeDb"] = json!(-6.0);
        after_rows[1].1[0]["mute"] = json!(true);
        after_rows[1].1[1]["output"] = json!("Bus 1");
        after_rows[1].1[2]["volumeDb"] = json!(0.04);
        let after = snapshot(&after_rows).unwrap();
        assert_eq!(said(&before, &after), ["Changed 2 tracks"]);
        let told = watched(&before, &after);
        assert_eq!(
            told[0],
            json!({"changed": "track", "name": "Vocal", "what": [{"what": "volumeDb", "from": 0.0, "to": -6.0}, {"what": "mute", "from": false, "to": true}]})
        );
        assert_eq!(
            told[1],
            json!({"changed": "track", "name": "Keys", "what": [{"what": "output", "from": "Stereo Output", "to": "Bus 1"}]})
        );
        assert_eq!(told.len(), 2, "a fader within a twentieth of a dB is where it was: {told:?}");
        let one = rows(1, &SCRATCH, &[], &[], &[], 160.0);
        let mut louder = one.clone();
        louder[1].1[0]["volumeDb"] = json!(-3.0);
        assert_eq!(said(&snapshot(&one).unwrap(), &snapshot(&louder).unwrap()), ["Changed track “Vocal” (mixer)"]);
        // A track added is told with what it's made of, for a recipe.
        let added = snapshot(&rows(
            1,
            &[("Vocal", "audio"), ("Keys", "instrument"), ("Bass", "instrument"), ("Pad", "instrument")],
            &[],
            &[],
            &[],
            160.0,
        ))
        .unwrap();
        assert_eq!(
            watched(&before, &added),
            [
                json!({"added": "track", "name": "Pad", "trackKind": "instrument", "mixer": {"volumeDb": 0.0, "pan": 0, "mute": false, "solo": false}, "output": "Stereo Output", "inputs": []})
            ]
        );
    }

    /// (M6) A track whose Mixer strip Logic hadn't drawn (`stripUnread`: a project too wide for the Mixer's view) gives no
    /// plug-ins and no routing: catch-up leaves them out rather than telling them removed, and still tells its regions.
    #[test]
    fn a_strip_the_mixer_hadnt_drawn_is_left_out_not_told_emptied() {
        let plugins = [(3, 1, "Channel EQ", false), (3, 2, "Compressor", false)];
        let before = snapshot(&rows(1, &SCRATCH, &[(3, "Bass", 0.0, 8.0)], &plugins, &[], 160.0)).unwrap();
        let mut read = rows(2, &SCRATCH, &[(3, "Bass", 4.0, 12.0)], &[], &[], 160.0);
        let bass = &mut read[1].1[2];
        (bass["stripUnread"], bass["output"], bass["inputs"]) = (json!(true), json!(""), json!([]));
        let after = snapshot(&read).unwrap();
        assert_eq!(after["tracks"][2]["stripUnread"], true);
        assert_eq!(said(&before, &after), ["Changed region “Bass” (position)"], "its plug-ins and routing unread, not gone");
        // Read again with its strip drawn: the plug-ins compare as ever.
        let drawn = snapshot(&rows(3, &SCRATCH, &[(3, "Bass", 4.0, 12.0)], &plugins[..1], &[], 160.0)).unwrap();
        assert_eq!(said(&before, &drawn), ["Changed region “Bass” (position)", "Removed plug-in “Compressor”"]);
    }

    /// (M6) The Stereo Out's plug-ins (an output strip no track has) are the project's own lines: a Gain added there, a
    /// Limiter taken off and another bypassed are told; a baseline kept before Kumi read them says nothing of them.
    #[test]
    fn the_stereo_outs_plug_ins_are_the_projects_own_lines() {
        let with = |plugins: &[(usize, &str, bool)]| {
            let mut read = rows(4, &SCRATCH, &[], &[(1, 1, "Channel EQ", false)], &[], 160.0);
            read[3].1.extend(plugins.iter().map(|(slot, name, bypassed)| {
                json!({"ref": format!("4:plugin:0.1.{slot}"), "trackRef": "4:output:1", "slot": slot, "name": name, "bypassed": bypassed, "kind": "effect"})
            }));
            snapshot(&read).unwrap()
        };
        let before = with(&[(1, "Limiter", false), (2, "Multipressor", false)]);
        assert_eq!(before["stereoOut"][0], json!({"slot": 1, "name": "Limiter", "bypassed": false, "kind": "effect"}));
        assert_eq!(before["plugins"].as_array().map(Vec::len), Some(1), "a track's plug-ins stay the tracks'");
        let after = with(&[(1, "Multipressor", true), (2, "Gain", false)]);
        assert_eq!(
            said(&before, &after),
            [
                "Removed plug-in “Limiter” from the Stereo Out",
                "Added plug-in “Gain” on the Stereo Out",
                "Plug-in “Multipressor” on the Stereo Out bypassed"
            ]
        );
        let told = watched(&before, &after);
        assert!(told.iter().any(|change| change["changed"] == "project" && change.to_string().contains("Gain")), "{told:?}");
        // A baseline kept before the Stereo Out was read: nothing to compare.
        let mut older = before.clone();
        older.remove("stereoOut");
        assert!(said(&older, &after).is_empty());
    }

    #[test]
    fn regions_match_by_track_and_start_and_a_moved_one_is_told_moved() {
        let regions = [(2, "Keys", 0.0, 16.0), (2, "Keys", 16.0, 32.0), (3, "Bass", 0.0, 8.0)];
        let before = snapshot(&rows(1, &SCRATCH, &regions, &[], &[], 160.0)).unwrap();
        let at = |regions: &[(usize, &str, f64, f64)]| snapshot(&rows(2, &SCRATCH, regions, &[], &[], 160.0)).unwrap();
        // Bass's region moved along its track; one of Keys' resized; another renamed in place.
        let moved = at(&[(2, "Keys", 0.0, 12.0), (2, "Keys B", 16.0, 32.0), (3, "Bass", 4.0, 12.0)]);
        assert_eq!(said(&before, &moved), ["Renamed region “Keys” → “Keys B”", "Changed 2 regions"]);
        let told = watched(&before, &moved);
        assert!(
            told.contains(
                &json!({"changed": "region", "name": "Keys", "on": "Keys", "what": [{"what": "lengthBeats", "from": 16.0, "to": 12.0}]})
            ),
            "{told:?}"
        );
        assert!(
            told.contains(
                &json!({"changed": "region", "name": "Bass", "on": "Bass", "what": [{"what": "startBeats", "from": 0.0, "to": 4.0}]})
            ),
            "{told:?}"
        );
        assert!(told.contains(&json!({"changed": "region", "name": "Keys B", "on": "Keys", "renamedFrom": "Keys"})), "{told:?}");
        // Moved to another track (Logic's region_move with toRef): told as moved there.
        let across = at(&[(2, "Keys", 0.0, 16.0), (2, "Keys", 16.0, 32.0), (1, "Bass", 0.0, 8.0)]);
        assert_eq!(said(&before, &across), ["Changed region “Bass” (track)"]);
        assert_eq!(
            watched(&before, &across),
            [json!({"changed": "region", "name": "Bass", "on": "Vocal", "what": [{"what": "track", "from": "Bass", "to": "Vocal"}]})]
        );
        // Two regions of one name both moved along the track: which went where can't be told.
        let shuffled = at(&[(2, "Keys", 4.0, 20.0), (2, "Keys", 24.0, 40.0), (3, "Bass", 0.0, 8.0)]);
        assert!(changes(&before, &shuffled, false)
            .iter()
            .any(|change| matches!(change, ProjectChange::Unclear { kind, .. } if kind == "region")));
        // Deleted and added.
        let fewer = at(&[(2, "Keys", 0.0, 16.0), (3, "Bass", 0.0, 8.0), (1, "Take", 32.0, 40.0)]);
        assert_eq!(said(&before, &fewer), ["Added region “Take”", "Removed region “Keys”"]);
    }

    #[test]
    fn whats_on_a_track_goes_with_it() {
        let before = snapshot(&rows(1, &SCRATCH, &[(3, "Bass", 0.0, 8.0)], &[(3, 1, "Compressor", false)], &[], 160.0)).unwrap();
        // Bass deleted with its region and plug-in: only the track is told.
        let gone = snapshot(&rows(2, &SCRATCH[..2], &[], &[], &[], 160.0)).unwrap();
        assert_eq!(said(&before, &gone), ["Removed track “Bass”"]);
        // A track added with a region and a plug-in on it: each is told, on it.
        let tracks = [("Vocal", "audio"), ("Keys", "instrument"), ("Bass", "instrument"), ("Pad", "instrument")];
        let added = snapshot(&rows(
            2,
            &tracks,
            &[(3, "Bass", 0.0, 8.0), (4, "Pad", 0.0, 16.0)],
            &[(3, 1, "Compressor", false), (4, 1, "Alchemy", false)],
            &[],
            160.0,
        ))
        .unwrap();
        assert_eq!(said(&before, &added), ["Added track “Pad”", "Added region “Pad”", "Added plug-in “Alchemy”"]);
        let told = watched(&before, &added);
        assert_eq!(
            told.iter().map(|change| change.get("added").cloned().unwrap_or_default()).collect::<Vec<_>>(),
            ["track", "plug-in", "region"]
        );
        assert_eq!(told[1], json!({"added": "plug-in", "name": "Alchemy", "pluginKind": "effect", "slot": 1, "on": "Pad"}));
    }

    #[test]
    fn plug_ins_match_by_track_and_slot_order() {
        let chain = [(1, 1, "Channel EQ", false), (1, 2, "Compressor", false), (1, 3, "ChromaVerb", false)];
        let before = snapshot(&rows(1, &SCRATCH, &[], &chain, &[], 160.0)).unwrap();
        let at = |plugins: &[(usize, usize, &str, bool)]| snapshot(&rows(1, &SCRATCH, &[], plugins, &[], 160.0)).unwrap();
        // One inserted first moves the others down a slot: only it is told.
        let inserted = at(&[(1, 1, "Gain", false), (1, 2, "Channel EQ", false), (1, 3, "Compressor", false), (1, 4, "ChromaVerb", false)]);
        assert_eq!(said(&before, &inserted), ["Added plug-in “Gain”"]);
        // One bypassed, one removed.
        let bypassed = at(&[(1, 1, "Channel EQ", true), (1, 2, "ChromaVerb", false)]);
        assert_eq!(said(&before, &bypassed), ["Removed plug-in “Compressor”", "Changed plug-in “Channel EQ” (bypass)"]);
        assert!(watched(&before, &bypassed).contains(
            &json!({"changed": "plug-in", "name": "Channel EQ", "on": "Vocal", "what": [{"what": "bypassed", "from": false, "to": true}]})
        ));
        // One replaced in its slot: as Live's devices read, the one gone and the one come at one place.
        let replaced = at(&[(1, 1, "Channel EQ", false), (1, 2, "Limiter", false), (1, 3, "ChromaVerb", false)]);
        assert_eq!(said(&before, &replaced), ["“Compressor” → “Limiter” (renamed and changed)"]);
        // One moved to the end of the chain.
        let moved = at(&[(1, 1, "Compressor", false), (1, 2, "ChromaVerb", false), (1, 3, "Channel EQ", false)]);
        assert_eq!(said(&before, &moved), ["Changed plug-in “Channel EQ” (slot)"]);
    }

    #[test]
    fn markers_match_by_position_and_the_tempo_and_signatures_are_the_projects_lines() {
        let before = snapshot(&rows(1, &SCRATCH, &[], &[], &[(9, "Chorus"), (17, "Verse")], 160.0)).unwrap();
        let mut after_rows = rows(1, &SCRATCH, &[], &[], &[(9, "Hook"), (25, "Verse"), (33, "Outro")], 124.0);
        after_rows[5].1[0]["signatures"] = json!([
            {"key": "A Minor", "position": position(1, 1), "time": "3/4"},
            {"key": "A Minor", "position": position(17, 1), "time": "7/8"},
        ]);
        let after = snapshot(&after_rows).unwrap();
        assert_eq!(
            said(&before, &after),
            [
                "Tempo 160 → 124 BPM",
                "Time signature 4/4 → 3/4",
                "Key C Major → A Minor",
                "Signature 7/8, A Minor from bar 17",
                "Renamed marker “Chorus” → “Hook”",
                "Added marker “Outro”",
                "Changed marker “Verse” (position)",
            ]
        );
        let told = watched(&before, &after);
        assert_eq!(told[0]["changed"], "project");
        assert_eq!(told[0]["what"][0], json!({"what": "tempo", "from": 160.0, "to": 124.0}));
        assert!(
            told.contains(&json!({"changed": "marker", "name": "Verse", "what": [{"what": "position", "from": "bar 17", "to": "bar 25"}]})),
            "{told:?}"
        );
        assert!(told.contains(&json!({"added": "marker", "name": "Outro", "position": "bar 33", "length": 16.0})), "{told:?}");
        // The Tempo List's later events, when the bridge lists them: tempo events of their own.
        let mut listed = rows(1, &SCRATCH, &[], &[], &[], 160.0);
        listed[5].1[0]["tempos"] = json!([{"position": position(1, 1), "bpm": 160.0}, {"position": position(17, 1), "bpm": 140.0}]);
        let mut changed = listed.clone();
        changed[5].1[0]["tempos"] = json!([{"position": position(1, 1), "bpm": 160.0}, {"position": position(17, 1), "bpm": 150.0}, {"position": position(33, 3), "bpm": 90.0}]);
        let (listed, changed) = (snapshot(&listed).unwrap(), snapshot(&changed).unwrap());
        assert_eq!(
            said(&listed, &changed),
            ["Added tempo event “90 BPM at bar 33 beat 3”", "Changed tempo event “150 BPM at bar 17” (tempo)"]
        );
    }

    #[test]
    fn a_baseline_of_another_shape_compares_with_nothing() {
        let now = snapshot(&rows(1, &SCRATCH, &[], &[], &[], 160.0)).unwrap();
        let older = json!({"snapshot": "logic", "version": 0, "tracks": []}).as_object().unwrap().clone();
        let lives = json!({"records": [{"kind": "track", "name": "Audio"}]}).as_object().unwrap().clone();
        assert!(changes(&older, &now, false).is_empty() && changes(&lives, &now, true).is_empty());
    }

    #[test]
    fn logics_templates_and_project_folders() {
        let rules = LogicRules;
        assert!(rules.template_location("/Users/me/Music/Audio Music Apps/Project Templates/Band.logicx"));
        assert!(rules.template_location("/Users/me/Music/Audio Music Apps/Project Templates/Mine/Band.logicx"));
        assert!(!rules.template_location("/Users/me/Music/Logic/Scratch.logicx"));
        assert!(!rules.template_location("/Users/me/Music/Project Templates/Song.logicx"));
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("Song.logicx");
        std::fs::create_dir_all(&package).unwrap();
        let path = package.to_string_lossy().into_owned();
        assert_eq!(rules.project_folder(&path), Some(package.clone()), "a package is its own project's folder");
        std::fs::create_dir_all(dir.path().join("Audio Files")).unwrap();
        assert_eq!(rules.project_folder(&path), Some(dir.path().to_path_buf()), "a project organised as a folder");
        assert_eq!(rules.project_folder("/Users/me/Music/Song.als"), None);
    }

    #[test]
    fn logics_words_name_none_of_lives_things() {
        let words = format!("{:?} {} {:?} {}", LOGIC_WORDS.kinds, LOGIC_WORDS.project, LOGIC_WORDS.rank, WATCH_DESCRIPTION);
        assert!(!live_words(&words), "{words}");
        for kind in LOGIC_WORDS.rank {
            assert!(*kind == LOGIC_WORDS.project || LOGIC_WORDS.kinds.iter().any(|(one, _)| one == kind), "{kind}");
        }
    }
}
