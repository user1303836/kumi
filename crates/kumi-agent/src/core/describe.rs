//! What changed in a saved project, in words: "Since you were last here" (catch-up, a few lines) and what watch_me saw
//! (the changes, for the model to make a recipe of). Each DAW maps its own export and diff into `ProjectChange`s; the
//! words and the order come from here, with the DAW's names for the kinds of things in a project.
use crate::core::contracts::JsonObject;
use indexmap::{IndexMap, IndexSet};
use kumi_common::js::{
    json::stringify,
    string::{head, trim, utf16_len},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A thing in the project as a change names it: its name and its position among its kind, as the DAW gave them (None
/// when it gave none, which isn't the same as a null), and the track it's on (for watch_me).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectItem {
    pub name: Option<Value>,
    pub order: Option<Value>,
    pub on: Option<String>,
}

impl ProjectItem {
    /// Its name, null when it has none.
    fn name(&self) -> &Value {
        self.name.as_ref().unwrap_or(&Value::Null)
    }
    /// Its position, null when it has none.
    fn order(&self) -> Value {
        self.order.clone().unwrap_or(Value::Null)
    }
}

/// One detail of a change: the facet it belongs to ("mixer", for catch-up), what changed by name ("mixer.volume", for
/// watch_me), its value before and after (already small enough to show), and whether it's only a fingerprint (a hash
/// watch_me leaves out).
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub facet: String,
    pub what: String,
    pub from: Value,
    pub to: Value,
    pub fingerprint: bool,
}

/// One change in the project, as the DAW's diff tells it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ProjectChange {
    /// Things of a kind the DAW couldn't match one for one: those before and those after.
    Unclear {
        kind: String,
        before: Vec<ProjectItem>,
        after: Vec<ProjectItem>,
    },
    /// A thing added, and what it's made of (the DAW's own fields, for watch_me).
    Added {
        kind: String,
        item: ProjectItem,
        made_of: JsonObject,
    },
    Removed {
        kind: String,
        item: ProjectItem,
    },
    /// A thing changed: as it was and as it is, whether the DAW says it was renamed, the details, and, for the project
    /// itself, the lines the DAW words for catch-up in place of the rest ("Tempo 120 → 124 BPM").
    Changed {
        kind: String,
        before: Option<ProjectItem>,
        after: Option<ProjectItem>,
        renamed: bool,
        details: Vec<Detail>,
        lines: Option<Vec<String>>,
    },
}

/// The DAW's names for the kinds of things in a project.
#[derive(Debug, Clone, Copy)]
pub struct ProjectWords {
    /// The kinds catch-up tells, in its order, each with its plural ("track", "tracks").
    pub kinds: &'static [(&'static str, &'static str)],
    /// The project itself as a kind ("set"), which watch_me tells beside the others.
    pub project: &'static str,
    /// watch_me's order of kinds.
    pub rank: &'static [&'static str],
    /// Kinds whose change counts even when the DAW lists no detail of it ("its settings").
    pub opaque: &'static [&'static str],
}

impl ProjectWords {
    fn plural(&self, kind: &str) -> String {
        self.kinds.iter().find(|(one, _)| *one == kind).map(|(_, many)| many.to_string()).unwrap_or_else(|| format!("{kind}s"))
    }
    fn known(&self, kind: &str) -> bool {
        self.kinds.iter().any(|(one, _)| *one == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescribedDiff {
    pub lines: Vec<String>,
    pub more: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescribedWatch {
    pub changes: Vec<JsonObject>,
    pub more: usize,
}

fn quoted(value: &Value, fallback: &str) -> String {
    value.as_str().map(trim).filter(|s| !s.is_empty()).map(|s| format!("“{}”", head(s, 60))).unwrap_or_else(|| fallback.into())
}

fn named(words: &ProjectWords, kind: &str, items: &[&ProjectItem], verb: &str) -> String {
    let names: Vec<_> = items.iter().map(|item| quoted(item.name(), "")).filter(|s| !s.is_empty()).collect();
    if items.len() == 1 {
        return trim(&format!("{verb} {kind} {}", names.first().map(String::as_str).unwrap_or(""))).into();
    }
    if names.len() == items.len() && items.len() <= 3 {
        format!("{verb} {} {}", words.plural(kind), names.join(", "))
    } else {
        format!("{verb} {} {}", items.len(), words.plural(kind))
    }
}

/// A value small enough to show: a string to 120 characters, a list or an object to 240 as JSON.
pub fn small(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    Some(match value {
        Value::String(s) => json!(head(s, 120)),
        Value::Array(_) | Value::Object(_) => {
            let text = stringify(value);
            if utf16_len(&text) <= 240 {
                value.clone()
            } else {
                json!(format!("{}…", head(&text, 237)))
            }
        }
        _ => value.clone(),
    })
}

/// Catch-up's lines (at most `limit`, 8 by default): renames and the DAW's own lines as they come, then each kind's
/// additions, removals and changes, a thing removed and added at the same place read as renamed and changed.
pub fn describe_changes(changes: &[ProjectChange], words: &ProjectWords, limit: Option<usize>) -> DescribedDiff {
    let mut lines = vec![];
    let mut added: IndexMap<String, Vec<&ProjectItem>> = IndexMap::new();
    let mut removed: IndexMap<String, Vec<&ProjectItem>> = IndexMap::new();
    let mut edited: IndexMap<String, Vec<(&ProjectItem, IndexSet<String>)>> = IndexMap::new();
    for change in changes {
        match change {
            ProjectChange::Unclear { kind, before, after } => {
                if !words.known(kind) {
                    continue;
                }
                let mut olds: Vec<&ProjectItem> = before.iter().collect();
                let mut news: Vec<&ProjectItem> = after.iter().collect();
                olds.retain(|old| {
                    if let Some(i) = news.iter().position(|row| row.name == old.name) {
                        news.remove(i);
                        false
                    } else {
                        true
                    }
                });
                olds.retain(|old| {
                    if let Some(i) = news.iter().position(|row| row.order == old.order) {
                        let same = news.remove(i);
                        lines.push(format!("Renamed {kind} {} → {}", quoted(old.name(), "(unnamed)"), quoted(same.name(), "(unnamed)")));
                        false
                    } else {
                        true
                    }
                });
                if !news.is_empty() {
                    added.entry(kind.clone()).or_default().extend(news);
                }
                if !olds.is_empty() {
                    removed.entry(kind.clone()).or_default().extend(olds);
                }
            }
            ProjectChange::Added { kind, item, .. } => added.entry(kind.clone()).or_default().push(item),
            ProjectChange::Removed { kind, item } => removed.entry(kind.clone()).or_default().push(item),
            ProjectChange::Changed { kind, before, after, renamed, details, lines: own } => {
                if let Some(own) = own {
                    lines.extend(own.iter().cloned());
                    continue;
                }
                if *renamed {
                    if let (Some(old), Some(current)) = (before, after) {
                        lines.push(format!("Renamed {kind} {} → {}", quoted(old.name(), "(unnamed)"), quoted(current.name(), "(unnamed)")));
                    }
                }
                let what: IndexSet<_> = details.iter().map(|d| d.facet.clone()).filter(|p| !p.is_empty()).collect();
                if !what.is_empty() {
                    if let Some(row) = after.as_ref().or(before.as_ref()) {
                        edited.entry(kind.clone()).or_default().push((row, what));
                    }
                }
            }
        }
    }
    for (kind, gone) in &mut removed {
        let Some(fresh) = added.get_mut(kind) else { continue };
        gone.retain(|old| {
            if let Some(i) = fresh.iter().position(|item| item.order == old.order) {
                let matched = fresh.remove(i);
                lines.push(format!(
                    "{} → {} (renamed and changed)",
                    quoted(old.name(), &format!("An unnamed {kind}")),
                    quoted(matched.name(), "(unnamed)")
                ));
                false
            } else {
                true
            }
        });
    }
    for (kind, _) in words.kinds {
        if let Some(items) = added.get(*kind).filter(|items| !items.is_empty()) {
            lines.push(named(words, kind, items, "Added"))
        }
        if let Some(items) = removed.get(*kind).filter(|items| !items.is_empty()) {
            lines.push(named(words, kind, items, "Removed"))
        }
        if let Some(changes) = edited.get(*kind) {
            if changes.len() == 1 {
                let (record, what) = &changes[0];
                let what = what.iter().map(String::as_str).collect::<Vec<_>>().join(", ");
                lines.push(format!(
                    "Changed {kind} {}{}",
                    quoted(record.name(), "(unnamed)"),
                    if what.is_empty() { String::new() } else { format!(" ({what})") }
                ));
            } else if changes.len() > 1 {
                lines.push(format!("Changed {} {}", changes.len(), words.plural(kind)));
            }
        }
    }
    let limit = limit.unwrap_or(8);
    let more = lines.len().saturating_sub(limit);
    lines.truncate(limit);
    DescribedDiff { lines, more }
}

/// The track a thing is on, as watch_me says it.
fn on(item: Option<&ProjectItem>) -> JsonObject {
    item.and_then(|item| item.on.as_ref()).map(|name| json!({"on":name}).as_object().unwrap().clone()).unwrap_or_default()
}

/// watch_me's changes (at most `limit`, 40 by default), by kind in the DAW's order: what was added (with what it's made
/// of), removed, changed (each detail from → to) or couldn't be told apart, a thing removed and added at the same place
/// on the same track read as renamed and changed.
pub fn describe_watch_changes(changes: &[ProjectChange], words: &ProjectWords, limit: Option<usize>) -> DescribedWatch {
    let mut out: Vec<(usize, JsonObject)> = vec![];
    for (index, change) in changes.iter().enumerate() {
        let kind = match change {
            ProjectChange::Unclear { kind, .. }
            | ProjectChange::Added { kind, .. }
            | ProjectChange::Removed { kind, .. }
            | ProjectChange::Changed { kind, .. } => kind,
        };
        if kind != words.project && !words.known(kind) {
            continue;
        }
        let entry = match change {
            ProjectChange::Unclear { before, after, .. } => {
                let names = |items: &[ProjectItem]| {
                    let mut counted: IndexMap<String, usize> = IndexMap::new();
                    for name in items.iter().filter_map(|item| item.name.as_ref().and_then(Value::as_str)) {
                        *counted.entry(name.into()).or_default() += 1;
                    }
                    let mut listed: Vec<_> = counted
                        .iter()
                        .take(20)
                        .map(|(name, count)| if *count > 1 { format!("{name} ×{count}") } else { name.clone() })
                        .collect();
                    if counted.len() > 20 {
                        listed.push(format!("+{} more names", counted.len() - 20))
                    }
                    listed
                };
                json!({"unclear":kind,"before":names(before),"after":names(after)}).as_object().unwrap().clone()
            }
            ProjectChange::Added { item, made_of, .. } => {
                let mut c = json!({"added":kind}).as_object().unwrap().clone();
                c.extend(made_of.clone());
                c.extend(on(Some(item)));
                c.insert("order".into(), item.order());
                c
            }
            ProjectChange::Removed { item, .. } => {
                let mut c = json!({"removed":kind,"name":item.name()}).as_object().unwrap().clone();
                c.extend(on(Some(item)));
                c.insert("order".into(), item.order());
                c
            }
            ProjectChange::Changed { before, after, renamed, details, .. } => {
                let what: Vec<_> =
                    details.iter().filter(|d| !d.fingerprint).map(|d| json!({"what":d.what,"from":d.from,"to":d.to})).collect();
                let renamed =
                    *renamed && before.is_some() && after.is_some() && before.as_ref().unwrap().name != after.as_ref().unwrap().name;
                let row = after.as_ref().or(before.as_ref());
                if what.is_empty() && !renamed {
                    if words.opaque.contains(&kind.as_str()) && row.is_some() {
                        let mut c = json!({"changed":kind,"name":row.unwrap().name()}).as_object().unwrap().clone();
                        c.extend(on(row));
                        c.insert("what".into(), json!("its settings"));
                        c
                    } else {
                        continue;
                    }
                } else {
                    let mut c =
                        json!({"changed":kind,"name":row.map(|r| r.name().clone()).unwrap_or(Value::Null)}).as_object().unwrap().clone();
                    c.extend(on(row));
                    if renamed {
                        c.insert("renamedFrom".into(), before.as_ref().unwrap().name().clone());
                    }
                    if !what.is_empty() {
                        c.insert("what".into(), json!(&what[..what.len().min(12)]));
                    }
                    c
                }
            }
        };
        out.push((index, entry));
    }
    let gone: Vec<_> = out.iter().filter(|(_, c)| c.contains_key("removed")).cloned().collect();
    for (id, gone) in gone {
        if let Some(fresh) = out.iter().position(|(_, c)| {
            c.get("added") == gone.get("removed") && c.get("order") == gone.get("order") && c.get("on") == gone.get("on")
        }) {
            let mut rest = out[fresh].1.clone();
            let added = rest.shift_remove("added").unwrap();
            rest.shift_remove("order");
            let mut replacement = json!({"changed":added}).as_object().unwrap().clone();
            replacement.extend(rest);
            replacement.insert("renamedFrom".into(), gone.get("name").cloned().unwrap_or(Value::Null));
            replacement.insert("note".into(), json!("renamed and changed"));
            out[fresh].1 = replacement;
            out.retain(|(old, _)| *old != id);
        }
    }
    let rank = |c: &JsonObject| {
        let kind = c.get("added").or(c.get("removed")).or(c.get("changed")).or(c.get("unclear")).and_then(Value::as_str).unwrap_or("");
        words.rank.iter().position(|v| *v == kind).unwrap_or(usize::MAX)
    };
    out.sort_by_key(|(_, c)| rank(c));
    let mut changes: Vec<_> = out
        .into_iter()
        .map(|(_, mut c)| {
            c.shift_remove("order");
            c
        })
        .collect();
    let limit = limit.unwrap_or(40);
    let more = changes.len().saturating_sub(limit);
    changes.truncate(limit);
    DescribedWatch { changes, more }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER: ProjectWords = ProjectWords {
        kinds: &[("track", "tracks"), ("item", "items")],
        project: "project",
        rank: &["project", "track", "item"],
        opaque: &[],
    };

    fn item(name: &str, order: u64) -> ProjectItem {
        ProjectItem { name: Some(json!(name)), order: Some(json!(order)), on: Some("Drums".into()) }
    }

    #[test]
    fn another_daws_kinds_are_told_in_its_words_and_its_own_lines_stand() {
        let changes = [
            ProjectChange::Changed {
                kind: "project".into(),
                before: None,
                after: None,
                renamed: false,
                details: vec![],
                lines: Some(vec!["Tempo 90 → 100 BPM".into()]),
            },
            ProjectChange::Added { kind: "item".into(), item: item("Kick", 0), made_of: JsonObject::new() },
            ProjectChange::Added { kind: "item".into(), item: item("Snare", 1), made_of: JsonObject::new() },
            ProjectChange::Removed { kind: "track".into(), item: item("Old", 3) },
            ProjectChange::Added { kind: "track".into(), item: item("New", 3), made_of: JsonObject::new() },
        ];
        let said = describe_changes(&changes, &OTHER, None);
        assert_eq!(said.lines, ["Tempo 90 → 100 BPM", "“Old” → “New” (renamed and changed)", "Added items “Kick”, “Snare”"]);
        let watched = describe_watch_changes(&changes, &OTHER, None);
        assert_eq!(watched.changes[0]["changed"], "track");
        assert_eq!(watched.changes[0]["note"], "renamed and changed");
        assert_eq!(watched.changes[1]["added"], "item");
        assert_eq!(watched.changes[1]["on"], "Drums");
    }
}
