//! What catch-up and watch_me compare: a project's digest (the bridge's `digest` pages, put together) and the diff of two
//! digests, as the changes Kumi words ("Since you were last here", what watch_me saw).
//!
//! A digest is records: `{k kind, id? key, n name, o order, p parent key, dn default-named, f facets, v values, r ref}`.
//! REAPER keys its records by GUID; Renoise's have no key, so they are matched by name, by what they hold and by place.
//! A record without a key is named `<k>#<p>/<o>` (its parent's key, its place there), which its children take as `p`.
//! A facet leaf named `…Hash` is a fingerprint (watch_me leaves it out); a null leaf wasn't read and is never compared.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// A snapshot file's format.
pub const SNAPSHOT_VERSION: u32 = 1;
/// The most changes a diff lists; the rest are counted.
pub const MAX_CHANGES: usize = 400;
/// Two values closer than this are the same (a parameter read back after REAPER's undo state keeps 6 places, say).
const SAME_VALUE: f64 = 1e-6;

/// One thing in the project as the digest names it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Record {
    pub k: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    #[serde(default)]
    pub o: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<String>,
    /// Named by the DAW after its place ("Track 03" in Renoise): the name says nothing of which one it is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dn: bool,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub f: Map<String, Value>,
    /// Its parameters' values by index (watch_me), null where not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<Vec<Value>>,
    /// Its ref in the DAW, to read it again (watch_me).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
}

impl Record {
    /// Its key within the snapshot: its own, else its kind, parent and place.
    pub fn key(&self) -> String {
        match &self.id {
            Some(id) => id.clone(),
            None => format!("{}#{}/{}", self.k, self.p.as_deref().unwrap_or(""), self.o),
        }
    }

    /// Its name, when it says which one it is.
    fn telling_name(&self) -> Option<&str> {
        self.n.as_deref().filter(|name| !self.dn && !name.is_empty())
    }
}

/// A project as a digest saw it, as kept between sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: u32,
    pub daw: String,
    #[serde(default)]
    pub place: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub name: String,
    /// The open project as this run of the DAW names it (an unsaved project is kept by it).
    #[serde(default)]
    pub identity: String,
    /// When it was read, in ms since the epoch.
    #[serde(default)]
    pub saved_at: i64,
    /// The project's own values (tempo, time signature…).
    #[serde(default)]
    pub facts: Map<String, Value>,
    #[serde(default)]
    pub records: Vec<Record>,
    /// Kinds the bridge summed up on their parents instead of listing (too many): `{kind: why}`.
    #[serde(default)]
    pub bounded: Map<String, Value>,
    /// The facet of a parent that sums up a kind (`{"item": "items"}`): when both snapshots list that kind, the facet is
    /// left out, so a change isn't told twice.
    #[serde(default)]
    pub summaries: Map<String, Value>,
    /// The DAW's change counter when it was read (REAPER's state change count, Renoise's changes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<Value>,
}

/// A thing as a change names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub name: Option<String>,
    pub order: u64,
    /// The track it's on (for a thing inside one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// One detail of a change: its facet ("mixer"), what changed ("mixer.volume"), before and after, whether it's only a
/// fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Detail {
    pub facet: String,
    pub what: String,
    pub from: Value,
    pub to: Value,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fingerprint: bool,
}

/// A parameter that moved (watch_me): its index, its value before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct Moved {
    pub index: usize,
    pub from: Value,
    pub to: Value,
}

/// One change, as Kumi's describers take it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "lowercase")]
pub enum Change {
    Added {
        kind: String,
        item: Item,
        #[serde(rename = "madeOf", skip_serializing_if = "Map::is_empty")]
        made_of: Map<String, Value>,
        #[serde(skip)]
        moved: Vec<Moved>,
        #[serde(skip)]
        key: String,
    },
    Removed {
        kind: String,
        item: Item,
    },
    Changed {
        kind: String,
        before: Item,
        after: Item,
        renamed: bool,
        details: Vec<Detail>,
        #[serde(skip)]
        moved: Vec<Moved>,
        #[serde(skip)]
        key: String,
    },
}

impl Change {
    pub fn kind(&self) -> &str {
        match self {
            Change::Added { kind, .. } | Change::Removed { kind, .. } | Change::Changed { kind, .. } => kind,
        }
    }
}

/// What changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Diff {
    /// The project's own values that changed.
    pub project: Vec<Detail>,
    pub changes: Vec<Change>,
    /// Changes not listed.
    pub more: usize,
}

/// Whether a snapshot was cut short: the project had more pages or things than a digest reads, so it holds only some.
pub fn cut(snapshot: &Snapshot) -> bool {
    cut_bounds(&snapshot.bounded)
}

/// Whether a snapshot's bounds say its read was cut short (more pages or records than a digest reads).
pub fn cut_bounds(bounded: &Map<String, Value>) -> bool {
    bounded.contains_key("pages") || bounded.contains_key("records")
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.project.is_empty() && self.changes.is_empty() && self.more == 0
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn same(a: &Value, b: &Value) -> bool {
    match (number(a), number(b)) {
        (Some(a), Some(b)) => (a - b).abs() <= SAME_VALUE.max(a.abs().max(b.abs()) * 1e-9),
        _ => a == b,
    }
}

/// Every leaf of two facet maps that differs, by path (`mixer.volume`), skipping facets left out and null leaves.
fn compare(before: &Map<String, Value>, after: &Map<String, Value>, skip: &HashSet<String>, out: &mut Vec<Detail>) {
    fn walk(path: &str, facet: &str, before: Option<&Value>, after: Option<&Value>, out: &mut Vec<Detail>) {
        match (before, after) {
            (Some(Value::Null), _) | (_, Some(Value::Null)) => {}
            (Some(Value::Object(a)), Some(Value::Object(b))) => {
                let mut keys: Vec<&String> = a.keys().collect();
                keys.extend(b.keys().filter(|key| !a.contains_key(*key)));
                for key in keys {
                    walk(&format!("{path}.{key}"), facet, a.get(key), b.get(key), out);
                }
            }
            (Some(a), Some(b)) if same(a, b) => {}
            (None, None) => {}
            (a, b) => {
                let leaf = path.rsplit('.').next().unwrap_or(path);
                out.push(Detail {
                    facet: facet.into(),
                    what: path.into(),
                    from: a.cloned().unwrap_or(Value::Null),
                    to: b.cloned().unwrap_or(Value::Null),
                    fingerprint: leaf.ends_with("Hash"),
                })
            }
        }
    }
    let mut keys: Vec<&String> = before.keys().collect();
    keys.extend(after.keys().filter(|key| !before.contains_key(*key)));
    for key in keys.into_iter().filter(|key| !skip.contains(*key)) {
        walk(key, key, before.get(key), after.get(key), out);
    }
}

/// The parameters that moved between two value lists.
fn moved(before: Option<&Vec<Value>>, after: Option<&Vec<Value>>) -> Vec<Moved> {
    let (Some(before), Some(after)) = (before, after) else { return vec![] };
    after
        .iter()
        .enumerate()
        .filter_map(|(index, to)| {
            let from = before.get(index)?;
            (!from.is_null() && !to.is_null() && !same(from, to)).then(|| Moved { index, from: from.clone(), to: to.clone() })
        })
        .collect()
}

/// Pairs the records of one kind under one parent that have no key: by name and content, by name, by content, then by
/// place, each in order. Returns (before, after) indexes.
fn pair_unkeyed(before: &[&Record], after: &[&Record]) -> Vec<(usize, usize)> {
    let content = |record: &Record| serde_json::to_string(&record.f).unwrap_or_default();
    let mut used_before = vec![false; before.len()];
    let mut used_after = vec![false; after.len()];
    let mut pairs = vec![];
    type KeyOf<'a> = Box<dyn Fn(&Record) -> Option<String> + 'a>;
    let tiers: [KeyOf; 4] = [
        Box::new(|r| r.telling_name().map(|name| format!("{name}\u{0}{}", content(r)))),
        Box::new(|r| r.telling_name().map(str::to_owned)),
        Box::new(|r| (!r.f.is_empty()).then(|| content(r))),
        Box::new(|r| Some(r.o.to_string())),
    ];
    for key_of in &tiers {
        let mut waiting: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, record) in before.iter().enumerate().rev() {
            if !used_before[index] {
                if let Some(key) = key_of(record) {
                    waiting.entry(key).or_default().push(index);
                }
            }
        }
        for (index, record) in after.iter().enumerate() {
            if used_after[index] {
                continue;
            }
            let Some(key) = key_of(record) else { continue };
            if let Some(found) = waiting.get_mut(&key).and_then(Vec::pop) {
                used_before[found] = true;
                used_after[index] = true;
                pairs.push((found, index));
            }
        }
    }
    pairs
}

/// The name of the track a record is on: its topmost parent's.
fn track_of(record: &Record, by_key: &HashMap<String, &Record>) -> Option<String> {
    let mut parent = record.p.as_deref()?;
    let mut top = None;
    for _ in 0..16 {
        let Some(found) = by_key.get(parent) else { break };
        top = Some(*found);
        match found.p.as_deref() {
            Some(up) => parent = up,
            None => break,
        }
    }
    top.filter(|found| found.k == "track").and_then(|found| found.n.clone())
}

fn item(record: &Record, by_key: &HashMap<String, &Record>) -> Item {
    Item { name: record.n.clone(), order: record.o, on: track_of(record, by_key), reference: record.r.clone() }
}

/// What an added thing is made of (watch_me): its facets that aren't fingerprints, and the names of what's inside it.
fn made_of(record: &Record, children: &[&Record]) -> Map<String, Value> {
    fn plain(value: &Value) -> Option<Value> {
        match value {
            Value::Null => None,
            Value::Object(fields) => {
                let kept: Map<String, Value> = fields
                    .iter()
                    .filter(|(key, _)| !key.ends_with("Hash"))
                    .filter_map(|(key, value)| Some((key.clone(), plain(value)?)))
                    .collect();
                (!kept.is_empty()).then_some(Value::Object(kept))
            }
            other => Some(other.clone()),
        }
    }
    let mut made = Map::new();
    if let Some(name) = record.n.as_deref().filter(|name| !name.is_empty()) {
        made.insert("name".into(), name.into());
    }
    for (key, value) in &record.f {
        if key.ends_with("Hash") {
            continue;
        }
        if let Some(value) = plain(value) {
            made.insert(key.clone(), value);
        }
    }
    let mut inside: Map<String, Value> = Map::new();
    for child in children {
        let names = inside.entry(child.k.clone()).or_insert_with(|| Value::Array(vec![]));
        if let Value::Array(names) = names {
            if names.len() < 24 {
                names.push(child.n.clone().map(Value::String).unwrap_or(Value::Null));
            }
        }
    }
    made.extend(inside);
    made
}

/// The diff of two snapshots of one project. `watching` adds what watch_me says beside catch-up: each detail's values,
/// what an added thing is made of, the parameters that moved (against `firsts`, the first look at a thing added while
/// watching, when there was one).
pub fn diff(before: &Snapshot, after: &Snapshot, watching: bool, firsts: &HashMap<String, Record>) -> Diff {
    let mut out = Diff::default();
    let before_keys: HashMap<String, &Record> = before.records.iter().map(|r| (r.key(), r)).collect();
    let after_keys: HashMap<String, &Record> = after.records.iter().map(|r| (r.key(), r)).collect();
    // Kinds either side summed up are compared through their parents' facets only.
    let bounded = |kind: &str| before.bounded.contains_key(kind) || after.bounded.contains_key(kind);
    // A side cut short holds only some of the project: what it didn't reach is neither gone nor new, so only what both
    // sides read is compared.
    let partial = cut(before) || cut(after);
    let mut kinds: Vec<&str> = vec![];
    for record in after.records.iter().chain(&before.records) {
        if !kinds.contains(&record.k.as_str()) {
            kinds.push(&record.k);
        }
    }
    // The facets left out: those that sum up a kind both sides list.
    let mut skip = HashSet::new();
    for (kind, facet) in after.summaries.iter().chain(&before.summaries) {
        if let Some(facet) = facet.as_str() {
            let listed = |snapshot: &Snapshot| snapshot.records.iter().any(|r| r.k == *kind);
            if !bounded(kind) && (listed(before) || listed(after)) {
                skip.insert(facet.to_owned());
            }
        }
    }
    // The project's own values: a sum of a kind both sides list (the markers) is told by that kind's records.
    compare(&before.facts, &after.facts, &skip, &mut out.project);
    // Parents first: a kind any of whose records has no parent.
    let top = |kind: &str| after.records.iter().chain(&before.records).any(|r| r.k == kind && r.p.is_none());
    kinds.sort_by_key(|kind| !top(kind));
    // before key → after key, for every record matched.
    let mut matched: HashMap<String, String> = HashMap::new();
    let mut matched_after: HashSet<String> = HashSet::new();
    for kind in kinds.into_iter().filter(|kind| !bounded(kind)) {
        let olds: Vec<&Record> = before.records.iter().filter(|r| r.k == kind).collect();
        let news: Vec<&Record> = after.records.iter().filter(|r| r.k == kind).collect();
        let mut pairs: Vec<(&Record, &Record)> = vec![];
        let mut taken_old = HashSet::new();
        let mut taken_new = HashSet::new();
        let new_by_id: HashMap<&str, &Record> = news.iter().filter_map(|r| Some((r.id.as_deref()?, *r))).collect();
        for old in &olds {
            if let Some(new) = old.id.as_deref().and_then(|id| new_by_id.get(id)) {
                pairs.push((old, new));
                taken_old.insert(old.key());
                taken_new.insert(new.key());
            }
        }
        // Without keys: within the same parent (a matched one), by name, content and place.
        let mut groups: Vec<(Option<String>, Vec<&Record>, Vec<&Record>)> = vec![];
        let group_of = |groups: &mut Vec<(Option<String>, Vec<&Record>, Vec<&Record>)>, parent: Option<String>| -> usize {
            match groups.iter().position(|(p, _, _)| *p == parent) {
                Some(at) => at,
                None => {
                    groups.push((parent, vec![], vec![]));
                    groups.len() - 1
                }
            }
        };
        for old in olds.iter().filter(|r| r.id.is_none()) {
            let parent = old.p.as_ref().map(|p| matched.get(p).cloned().unwrap_or_else(|| format!("\u{0}gone:{p}")));
            let at = group_of(&mut groups, parent);
            groups[at].1.push(old);
        }
        for new in news.iter().filter(|r| r.id.is_none()) {
            let at = group_of(&mut groups, new.p.clone());
            groups[at].2.push(new);
        }
        for (_, olds, news) in &groups {
            for (a, b) in pair_unkeyed(olds, news) {
                pairs.push((olds[a], news[b]));
                taken_old.insert(olds[a].key());
                taken_new.insert(news[b].key());
            }
        }
        for (old, new) in &pairs {
            matched.insert(old.key(), new.key());
            matched_after.insert(new.key());
        }
        pairs.sort_by_key(|(_, new)| new.o);
        for (old, new) in pairs {
            let mut details = vec![];
            compare(&old.f, &new.f, &skip, &mut details);
            // (two names the DAW gave by place differ only because the place did)
            let renamed = new.n != old.n && !(old.dn && new.dn);
            let moves = if watching { moved(old.v.as_ref(), new.v.as_ref()) } else { vec![] };
            if details.is_empty() && !renamed && moves.is_empty() {
                continue;
            }
            if !watching {
                for detail in &mut details {
                    detail.from = Value::Null;
                    detail.to = Value::Null;
                }
            }
            out.changes.push(Change::Changed {
                kind: kind.into(),
                before: item(old, &before_keys),
                after: item(new, &after_keys),
                renamed,
                details,
                moved: moves,
                key: new.key(),
            });
        }
        if partial {
            continue;
        }
        // What's gone with its parent, or came with it, is told by the parent.
        for old in olds.iter().filter(|r| !taken_old.contains(&r.key())) {
            if old.p.as_ref().is_some_and(|p| before_keys.contains_key(p) && !matched.contains_key(p)) {
                continue;
            }
            out.changes.push(Change::Removed { kind: kind.into(), item: item(old, &before_keys) });
        }
        for new in news.iter().filter(|r| !taken_new.contains(&r.key())) {
            // (while watching, what has values of its own, an FX or a device, is told on its own too, with the parameters
            // moved on it, so that it can be made again)
            if new.p.as_ref().is_some_and(|p| after_keys.contains_key(p) && !matched_after.contains(p)) && !(watching && new.v.is_some()) {
                continue;
            }
            let key = new.key();
            let (made, moves) = if watching {
                let children: Vec<&Record> =
                    after.records.iter().filter(|r| r.p.as_deref() == Some(key.as_str()) && r.v.is_none()).collect();
                let first = firsts.get(&key);
                (made_of(new, &children), moved(first.and_then(|f| f.v.as_ref()), new.v.as_ref()))
            } else {
                (Map::new(), vec![])
            };
            out.changes.push(Change::Added { kind: kind.into(), item: item(new, &after_keys), made_of: made, moved: moves, key });
        }
    }
    if !watching {
        for detail in &mut out.project {
            // The project's own values are said with their numbers ("Tempo 120 → 124 BPM").
            detail.fingerprint = detail.what.rsplit('.').next().is_some_and(|leaf| leaf.ends_with("Hash"));
        }
    }
    if out.changes.len() > MAX_CHANGES {
        out.more = out.changes.len() - MAX_CHANGES;
        out.changes.truncate(MAX_CHANGES);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(value: Value) -> Record {
        serde_json::from_value(value).unwrap()
    }

    fn snapshot(records: Vec<Value>) -> Snapshot {
        Snapshot {
            version: SNAPSHOT_VERSION,
            daw: "reaper".into(),
            records: records.into_iter().map(record).collect(),
            ..Default::default()
        }
    }

    fn said(diff: &Diff) -> Vec<String> {
        diff.changes
            .iter()
            .map(|change| match change {
                Change::Added { kind, item, .. } => format!("+{kind} {}", item.name.clone().unwrap_or_default()),
                Change::Removed { kind, item } => format!("-{kind} {}", item.name.clone().unwrap_or_default()),
                Change::Changed { kind, after, renamed, details, .. } => format!(
                    "~{kind} {}{} ({})",
                    after.name.clone().unwrap_or_default(),
                    if *renamed { " renamed" } else { "" },
                    details.iter().map(|d| d.what.as_str()).collect::<Vec<_>>().join(", ")
                ),
            })
            .collect()
    }

    #[test]
    fn keyed_records_match_by_their_key_whatever_their_place_or_name() {
        let before = snapshot(vec![
            json!({"k": "track", "id": "{A}", "n": "Drums", "o": 1, "f": {"mixer": {"volume": 0.0}}}),
            json!({"k": "track", "id": "{B}", "n": "Bass", "o": 2, "f": {"mixer": {"volume": -6.0}}}),
            json!({"k": "FX", "id": "{F1}", "n": "ReaComp", "o": 0, "p": "{B}", "f": {"parameters": {"parametersHash": "1"}}}),
        ]);
        let after = snapshot(vec![
            json!({"k": "track", "id": "{B}", "n": "Bass DI", "o": 1, "f": {"mixer": {"volume": -3.0}}}),
            json!({"k": "track", "id": "{C}", "n": "Keys", "o": 2}),
            json!({"k": "FX", "id": "{F1}", "n": "ReaComp", "o": 0, "p": "{B}", "f": {"parameters": {"parametersHash": "2"}}}),
            json!({"k": "FX", "id": "{F2}", "n": "ReaEQ", "o": 0, "p": "{C}"}),
        ]);
        let diff = diff(&before, &after, false, &HashMap::new());
        assert_eq!(
            said(&diff),
            ["~track Bass DI renamed (mixer.volume)", "-track Drums", "+track Keys", "~FX ReaComp (parameters.parametersHash)"]
        );
        // Catch-up says what changed, not the numbers.
        let Change::Changed { details, .. } = &diff.changes[0] else { panic!() };
        assert_eq!((details[0].from.clone(), details[0].to.clone()), (Value::Null, Value::Null));
        // The new track's FX came with it: told by the track.
        assert!(!said(&diff).iter().any(|line| line.contains("ReaEQ")));
    }

    #[test]
    fn records_without_keys_are_matched_by_name_then_content_then_place() {
        // Renoise: default names follow the place, so a deleted first track renames the next one.
        let before = snapshot(vec![
            json!({"k": "track", "n": "Track 01", "dn": true, "o": 1, "f": {"devices": {"list": "A"}}}),
            json!({"k": "track", "n": "Track 02", "dn": true, "o": 2, "f": {"devices": {"list": "B"}}}),
            json!({"k": "track", "n": "Bass", "o": 3, "f": {"devices": {"list": "C"}}}),
            json!({"k": "device", "n": "EQ 10", "o": 2, "p": "track#/3", "f": {"active": true}}),
        ]);
        let after = snapshot(vec![
            json!({"k": "track", "n": "Track 01", "dn": true, "o": 1, "f": {"devices": {"list": "B"}}}),
            json!({"k": "track", "n": "Bass", "o": 2, "f": {"devices": {"list": "C"}}}),
            json!({"k": "device", "n": "EQ 10", "o": 2, "p": "track#/2", "f": {"active": false}}),
        ]);
        let diff = diff(&before, &after, true, &HashMap::new());
        assert_eq!(said(&diff), ["-track Track 01", "~device EQ 10 (active)"]);
        let Change::Changed { after: on, details, .. } = &diff.changes[1] else { panic!() };
        assert_eq!(on.on.as_deref(), Some("Bass"), "the device is on its track, found through the track's match");
        assert_eq!((details[0].from.clone(), details[0].to.clone()), (json!(true), json!(false)), "watch_me keeps the values");
    }

    #[test]
    fn a_kind_summed_up_on_either_side_is_compared_on_its_parents_only() {
        let mut before = snapshot(vec![
            json!({"k": "track", "id": "{A}", "n": "Drums", "o": 1, "f": {"items": {"count": 2, "itemsHash": "x"}}}),
            json!({"k": "item", "id": "{I1}", "n": "Kick", "o": 0, "p": "{A}", "f": {"position": 0.0}}),
            json!({"k": "item", "id": "{I2}", "n": "Snare", "o": 1, "p": "{A}", "f": {"position": 1.0}}),
        ]);
        before.summaries.insert("item".into(), json!("items"));
        let mut after = before.clone();
        after.records[0].f = json!({"items": {"count": 3, "itemsHash": "y"}}).as_object().unwrap().clone();
        after.records.push(record(json!({"k": "item", "id": "{I3}", "n": "Hat", "o": 2, "p": "{A}", "f": {"position": 2.0}})));
        // Both list items: the item is told, the track's sum isn't (no change told twice).
        assert_eq!(said(&diff(&before, &after, false, &HashMap::new())), ["+item Hat"]);
        // The project grew past what's listed: items aren't compared one by one (else every one would read removed).
        after.records.retain(|r| r.k != "item");
        after.bounded.insert("item".into(), json!("more than 5,000 items"));
        assert_eq!(said(&diff(&before, &after, false, &HashMap::new())), ["~track Drums (items.count, items.itemsHash)"]);
    }

    #[test]
    fn a_snapshot_cut_short_compares_only_what_both_sides_read() {
        let tracks = |names: &[&str]| -> Vec<Value> {
            names
                .iter()
                .enumerate()
                .map(|(o, name)| json!({"k": "track", "id": format!("{{{name}}}"), "n": name, "o": o, "f": {"mute": false}}))
                .collect()
        };
        let whole = snapshot(tracks(&["Kick", "Snare", "Bass"]));
        let mut cut_short = snapshot(tracks(&["Kick", "Snare"]));
        cut_short.records[1].f = json!({"mute": true}).as_object().unwrap().clone();
        cut_short.bounded.insert("pages".into(), json!("more than 400 pages"));
        // Bass wasn't reached: not removed, and not added the time after; Snare, read on both sides, is compared.
        assert_eq!(said(&diff(&whole, &cut_short, false, &HashMap::new())), ["~track Snare (mute)"]);
        assert_eq!(said(&diff(&cut_short, &whole, false, &HashMap::new())), ["~track Snare (mute)"]);
        let mut records = cut_short.clone();
        records.bounded.clear();
        records.bounded.insert("records".into(), json!("more than 20000 things"));
        assert_eq!(said(&diff(&whole, &records, true, &HashMap::new())), ["~track Snare (mute)"], "watch_me drafts no track_add");
    }

    #[test]
    fn a_projects_sum_of_a_kind_both_sides_list_is_told_by_its_records_only() {
        let mut before = snapshot(vec![json!({"k": "marker", "id": "{M1}", "n": "Intro", "o": 0})]);
        before.facts = json!({"markers": {"count": 1, "markersHash": "a"}}).as_object().unwrap().clone();
        before.summaries.insert("marker".into(), json!("markers"));
        let mut after = before.clone();
        after.records.push(record(json!({"k": "marker", "id": "{M2}", "n": "Drop", "o": 1})));
        after.facts = json!({"markers": {"count": 2, "markersHash": "b"}}).as_object().unwrap().clone();
        let found = diff(&before, &after, false, &HashMap::new());
        assert!(found.project.is_empty(), "{:?}", found.project);
        assert_eq!(said(&found), ["+marker Drop"]);
    }

    #[test]
    fn a_leaf_not_read_on_either_side_is_not_compared_and_the_projects_own_values_keep_their_numbers() {
        let mut before = snapshot(vec![json!({"k": "pattern", "n": "Intro", "o": 1, "f": {"content": {"contentHash": null}}})]);
        before.facts = json!({"tempo": 120.0, "timeSignature": [4, 4]}).as_object().unwrap().clone();
        let mut after = snapshot(vec![json!({"k": "pattern", "n": "Intro", "o": 1, "f": {"content": {"contentHash": "abc"}}})]);
        after.facts = json!({"tempo": 124.0, "timeSignature": [4, 4]}).as_object().unwrap().clone();
        let diff = diff(&before, &after, false, &HashMap::new());
        assert!(diff.changes.is_empty(), "{:?}", diff.changes);
        assert_eq!(diff.project.len(), 1);
        assert_eq!(
            (diff.project[0].what.as_str(), diff.project[0].from.clone(), diff.project[0].to.clone()),
            ("tempo", json!(120.0), json!(124.0))
        );
    }

    #[test]
    fn watching_tells_the_parameters_that_moved_and_what_an_added_thing_is_made_of() {
        let before = snapshot(vec![
            json!({"k": "track", "id": "{A}", "n": "Vox", "o": 1}),
            json!({"k": "FX", "id": "{F1}", "n": "ReaEQ", "o": 0, "p": "{A}", "v": [0.5, 0.25, null]}),
        ]);
        let after = snapshot(vec![
            json!({"k": "track", "id": "{A}", "n": "Vox", "o": 1}),
            json!({"k": "FX", "id": "{F1}", "n": "ReaEQ", "o": 0, "p": "{A}", "v": [0.5, 0.3, 0.9]}),
            json!({"k": "FX", "id": "{F2}", "n": "ReaComp", "o": 1, "p": "{A}", "v": [0.1, 0.7], "f": {"enabled": true, "parameters": {"parametersHash": "z"}}}),
            json!({"k": "track", "id": "{B}", "n": "Vox Bus", "o": 2, "f": {"mixer": {"volume": -3.0}}}),
            json!({"k": "FX", "id": "{F3}", "n": "ReaVerbate", "o": 0, "p": "{B}"}),
        ]);
        let firsts: HashMap<String, Record> = [("{F2}".to_string(), record(json!({"k": "FX", "id": "{F2}", "v": [0.1, 0.5]})))].into();
        let diff = diff(&before, &after, true, &firsts);
        assert_eq!(said(&diff), ["+track Vox Bus", "~FX ReaEQ ()", "+FX ReaComp"]);
        let Change::Changed { moved, .. } = &diff.changes[1] else { panic!() };
        assert_eq!(moved, &[Moved { index: 1, from: json!(0.25), to: json!(0.3) }], "a value not read before isn't a move");
        let Change::Added { moved, made_of, item, .. } = &diff.changes[2] else { panic!() };
        assert_eq!(moved, &[Moved { index: 1, from: json!(0.5), to: json!(0.7) }], "against its first look, not its defaults");
        assert_eq!(item.on.as_deref(), Some("Vox"));
        assert_eq!(made_of.get("enabled"), Some(&json!(true)));
        assert!(!made_of.contains_key("parameters"), "fingerprints aren't what it's made of: {made_of:?}");
        let Change::Added { made_of, .. } = &diff.changes[0] else { panic!() };
        assert_eq!(made_of.get("FX"), Some(&json!(["ReaVerbate"])), "{made_of:?}");
        assert_eq!(made_of.get("mixer"), Some(&json!({"volume": -3.0})));
    }

    // Review K8: an FX put on a track added while watching is told on its own, with what it's made of and the parameters
    // moved since its first look, after the track; what has no values (an item) is still told by the track.
    #[test]
    fn what_comes_on_a_track_added_while_watching_is_told_with_its_own_parameters() {
        let before = snapshot(vec![json!({"k": "track", "id": "{A}", "n": "Vox", "o": 1})]);
        let after = snapshot(vec![
            json!({"k": "track", "id": "{A}", "n": "Vox", "o": 1}),
            json!({"k": "track", "id": "{B}", "n": "Vox Bus", "o": 2}),
            json!({"k": "FX", "id": "{F1}", "n": "ReaEQ", "o": 0, "p": "{B}", "v": [0.5, 0.25], "f": {"plugin": "VST: ReaEQ (Cockos)"}}),
            json!({"k": "item", "id": "{I1}", "n": "take 1", "o": 0, "p": "{B}"}),
        ]);
        let firsts: HashMap<String, Record> = [("{F1}".to_string(), record(json!({"k": "FX", "id": "{F1}", "v": [0.5, 0.5]})))].into();
        let diff = diff(&before, &after, true, &firsts);
        assert_eq!(said(&diff), ["+track Vox Bus", "+FX ReaEQ"]);
        let Change::Added { made_of, .. } = &diff.changes[0] else { panic!() };
        assert_eq!((made_of.get("FX"), made_of.get("item")), (None, Some(&json!(["take 1"]))), "{made_of:?}");
        let Change::Added { moved, made_of, item, .. } = &diff.changes[1] else { panic!() };
        assert_eq!(item.on.as_deref(), Some("Vox Bus"));
        assert_eq!(made_of.get("plugin"), Some(&json!("VST: ReaEQ (Cockos)")));
        assert_eq!(moved, &[Moved { index: 1, from: json!(0.5), to: json!(0.25) }]);
        // Catch-up (not watching) still tells it by the track.
        let diff = super::diff(&before, &after, false, &HashMap::new());
        assert_eq!(said(&diff), ["+track Vox Bus"]);
    }

    #[test]
    fn a_big_projects_diff_stays_quick() {
        // REAPER-like (keyed) and Renoise-like (matched by name, content and place): 5,200 and 1,300 records.
        let keyed: Vec<Value> = (0..5200).map(|i| json!({"k": if i % 26 == 0 { "track" } else { "item" }, "id": format!("{{G{i}}}"), "n": format!("N{i}"), "o": i, "p": if i % 26 == 0 { Value::Null } else { json!(format!("{{G{}}}", i - i % 26)) }, "f": {"position": i as f64, "content": {"contentHash": format!("{i:x}")}}})).collect();
        let unkeyed: Vec<Value> = (0..1300).map(|i| json!({"k": "pattern", "n": format!("Pattern {i:02X}"), "dn": true, "o": i, "f": {"lines": {"Kick": format!("{} notes, 0 effects #{i:x}", i % 7)}}})).collect();
        for records in [keyed, unkeyed] {
            let before = snapshot(records.clone());
            let mut changed = records;
            changed[7]["f"]["position"] = json!(-1.0);
            changed.swap(10, 11);
            let after = snapshot(changed);
            let began = std::time::Instant::now();
            let found = diff(&before, &after, false, &HashMap::new());
            let took = began.elapsed();
            eprintln!("diff of {} records: {} changes in {took:?}", before.records.len(), found.changes.len());
            assert!(took < std::time::Duration::from_secs(2), "{took:?}");
            assert!(!found.changes.is_empty());
        }
    }

    #[test]
    fn equal_snapshots_have_no_diff_and_a_long_one_is_counted_past_its_limit() {
        let before = snapshot((0..500).map(|i| json!({"k": "marker", "id": format!("{{M{i}}}"), "n": format!("M{i}"), "o": i})).collect());
        assert!(diff(&before, &before, false, &HashMap::new()).is_empty());
        let after = snapshot(vec![]);
        let gone = diff(&before, &after, false, &HashMap::new());
        assert_eq!((gone.changes.len(), gone.more), (MAX_CHANGES, 100));
    }
}
