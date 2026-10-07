//! Current-turn reference authority, short names, and discovery cursor bindings.
use super::{
    changes::{hex_color, KnownTrack, REFERENCE_FIELDS},
    context::{payload, query_key, ObservationError, PARENTS},
};
use crate::{
    core::contracts::{JsonObject, ToolResult},
    mcp::types::CallToolResult,
};
use indexmap::{IndexMap, IndexSet};
use kumi_common::js::{
    json::stringify,
    string::{head, utf16_len},
};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;
#[derive(Default)]
pub struct References {
    pub refs: IndexMap<String, String>,
    pub cursors: IndexMap<String, String>,
    pub known: IndexMap<String, KnownTrack>,
    short: IndexMap<String, String>,
    long: IndexMap<String, String>,
    counts: IndexMap<String, u64>,
    /// Refs a restructure moved to a place Live hasn't been read at since. Live's registry (the Remote Script's) keeps
    /// objects by these strings, so under each it still holds what was at that place when it was last read there. The
    /// bridge reads a place again before it acts there; Kumi's own Python has the places it names read again first
    /// (`moved_places`), or it would act on what used to be there.
    moved: IndexSet<String>,
}
/// Overflow also requires the owner to invalidate its observation lease.
#[derive(Debug, Clone)]
pub struct ReferenceError {
    pub error: ObservationError,
    pub invalidate: bool,
}
impl From<ObservationError> for ReferenceError {
    fn from(error: ObservationError) -> Self {
        Self { error, invalidate: false }
    }
}
impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for ReferenceError {}
fn error(message: impl Into<String>) -> ObservationError {
    ObservationError(message.into())
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(v)) => v.clone(),
        Some(Value::Array(v)) => {
            v.iter().map(|v| if v.is_null() { String::new() } else { js_string(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(value) => stringify(value),
    }
}
fn ref_key(key: &str) -> bool {
    key == "ref" || key == "parent" || key.ends_with("Ref") || key.ends_with("Refs")
}
/// Where a restructure moved the Set's tracks and scenes, so the refs past it follow what they named rather than
/// being dropped (#261, #253). Refs are positions: a track's index counts the regular and group tracks, then the
/// returns, then Main, so a track added or deleted moves everything after it, returns and Main among them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shift {
    /// Tracks made, at their places once made.
    pub tracks_made: Vec<usize>,
    /// Tracks deleted, at their places before.
    pub tracks_gone: Vec<usize>,
    pub scenes_made: Vec<usize>,
    pub scenes_gone: Vec<usize>,
    /// Returns were added or deleted: every track's sends are renumbered with them, so their refs are retired.
    pub sends: bool,
}
impl Shift {
    /// The shift that undoes this one: what it made is gone again, and what it deleted is back at its place.
    pub fn inverse(&self) -> Shift {
        Shift {
            tracks_made: self.tracks_gone.clone(),
            tracks_gone: self.tracks_made.clone(),
            scenes_made: self.scenes_gone.clone(),
            scenes_gone: self.scenes_made.clone(),
            sends: self.sends,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.tracks_made.is_empty() && self.tracks_gone.is_empty() && self.scenes_made.is_empty() && self.scenes_gone.is_empty()
    }
    /// Where the track at `index` before the change is after it, or None for one deleted.
    pub fn track(&self, index: usize) -> Option<usize> {
        moved_index(index, &self.tracks_gone, &self.tracks_made)
    }
    pub fn scene(&self, index: usize) -> Option<usize> {
        moved_index(index, &self.scenes_gone, &self.scenes_made)
    }
    /// Where a ref points after the change: the same, another place, or nowhere (what it named was deleted).
    pub fn reference(&self, reference: &str) -> Moved {
        self.moved(reference, 0)
    }
    fn moved(&self, reference: &str, depth: usize) -> Moved {
        let mut parts: Vec<String> = reference.split(':').map(str::to_owned).collect();
        if parts.len() < 3 || depth > 8 {
            return Moved::Same;
        }
        // A ref whose path is another ref (a parameter's device, a chain's rack): that one decides.
        if parts.len() >= 4 && parts[2] == parts[0] && !parts[3].is_empty() && !parts[3].bytes().all(|b| b.is_ascii_digit()) {
            return match self.moved(&parts[2..].join(":"), depth + 1) {
                Moved::To(inner) => Moved::To(format!("{}:{}:{inner}", parts[0], parts[1])),
                other => other,
            };
        }
        // A track's selected-device ref (device:view:N) isn't read again by place on the Remote Script's side, so it
        // can't follow its track: once the track moves, it's retired.
        if parts[1] == "device" && parts[2] == "view" {
            return match parts.get(3).and_then(|part| part.parse::<usize>().ok()).map(|index| (index, self.track(index))) {
                Some((index, Some(now))) if now == index => Moved::Same,
                Some(_) => Moved::Gone,
                None => Moved::Same,
            };
        }
        // Which parts hold a track's and a scene's index.
        let (track, scene) = match parts[1].as_str() {
            "track" | "device" | "chain" | "drum_pad" | "take_lane" | "take_lane_clip" | "routing_choice" | "mixer" => (Some(2), None),
            "clip" | "clip_slot" => (Some(2), Some(3)),
            // One part is a Song-level Arrangement clip, on no track.
            "arrangement_clip" if parts.len() == 4 => (Some(2), None),
            "scene" => (None, Some(2)),
            "parameter" if parts[2] == "mixer" => {
                if self.sends && parts.get(4).is_some_and(|part| part == "sends") {
                    return Moved::Gone;
                }
                (Some(3), None)
            }
            _ => (None, None),
        };
        let mut changed = false;
        for (at, place) in [(track, Shift::track as fn(&Self, usize) -> Option<usize>), (scene, Shift::scene)] {
            let Some(at) = at else { continue };
            let Some(index) = parts.get(at).and_then(|part| part.parse::<usize>().ok()) else { continue };
            match place(self, index) {
                None => return Moved::Gone,
                Some(now) if now != index => {
                    changed = true;
                    parts[at] = now.to_string();
                }
                Some(_) => {}
            }
        }
        if changed {
            Moved::To(parts.join(":"))
        } else {
            Moved::Same
        }
    }
}
/// What a restructure did to a ref.
#[derive(Debug, Clone, PartialEq)]
pub enum Moved {
    Same,
    To(String),
    Gone,
}
/// Where a ref is after a shift, or None for what was deleted.
fn shifted(shift: &Shift, reference: &str) -> Option<String> {
    match shift.reference(reference) {
        Moved::Same => Some(reference.to_owned()),
        Moved::To(now) => Some(now),
        Moved::Gone => None,
    }
}
/// The track a ref is on, as the Remote Script's `_ref_track_index` reads it: refs are positions (clip:3:5,
/// device:3:0:2, parameter:mixer:3:volume), and a ref whose path is another ref (a parameter's device, a return
/// chain's rack) is on that one's track. None for one on no track (a scene, the Set, a track's view, a Song-level
/// Arrangement clip).
fn track_of(reference: &str) -> Option<usize> {
    let parts: Vec<&str> = reference.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let (kind, path) = (parts[1], &parts[2..]);
    let index = |part: &str| part.bytes().all(|b| b.is_ascii_digit()).then(|| part.parse::<usize>().ok()).flatten();
    if path.len() >= 3 && path[0] == parts[0] && index(path[1]).is_none() {
        return track_of(&path.join(":"));
    }
    match kind {
        "parameter" if path.len() >= 2 && path[0] == "mixer" => index(path[1]),
        "arrangement_clip" if path.len() == 2 => index(path[0]),
        "track" | "clip" | "clip_slot" | "device" | "chain" | "drum_pad" | "take_lane" | "take_lane_clip" | "routing_choice" => {
            index(path[0])
        }
        _ => None,
    }
}
/// The place Live's registry keeps a ref's object at, as the Remote Script's `_refresh` reads it again: the track
/// (`E:track:N`, its whole row) for what's on one; a track's Arrangement clips, which its row doesn't list, together
/// (`E:arrangement_clip:N:0` reads them all); anything else (a scene) by itself.
fn place_of(reference: &str) -> String {
    let epoch = reference.split(':').next().unwrap_or_default();
    match track_of(reference) {
        Some(track) if reference.split(':').nth(1) == Some("arrangement_clip") => format!("{epoch}:arrangement_clip:{track}:0"),
        Some(track) => format!("{epoch}:track:{track}"),
        None => reference.to_owned(),
    }
}
/// An index after `gone` (places before) are deleted and `made` (places after) are made.
fn moved_index(index: usize, gone: &[usize], made: &[usize]) -> Option<usize> {
    if gone.contains(&index) {
        return None;
    }
    let mut at = index - gone.iter().filter(|place| **place < index).count();
    let mut made = made.to_vec();
    made.sort_unstable();
    made.dedup();
    for place in made {
        if place <= at {
            at += 1;
        }
    }
    Some(at)
}
impl References {
    /// Retired names never acquire a different object; counters continue across retirement.
    pub fn clear_names(&mut self) {
        self.short.clear();
        self.long.clear();
    }
    pub fn unname(&mut self, reference: &str) {
        if let Some(short) = self.short.shift_remove(reference) {
            self.long.shift_remove(&short);
        }
    }
    pub fn retire(&mut self, reference: &str) {
        self.refs.shift_remove(reference);
        self.known.shift_remove(reference);
        self.unname(reference);
    }
    pub fn named_references(&self) -> Vec<String> {
        self.short.keys().cloned().collect()
    }
    /// After a restructure: every ref past it follows what it named to its new place, short names and all, and those
    /// to what was deleted are retired. A name keeps naming one object, so a plan's later steps still find theirs.
    /// Discovery cursors were for the old places, so they go.
    pub fn shift(&mut self, shift: &Shift) {
        // What was marked moved goes with the shift too: a place not read again since is still stale.
        let mut moved: IndexSet<String> =
            std::mem::take(&mut self.moved).into_iter().filter_map(|reference| shifted(shift, &reference)).collect();
        let mut to = |reference: &str| {
            let now = shifted(shift, reference)?;
            if now != reference {
                moved.insert(now.clone());
            }
            Some(now)
        };
        self.refs = std::mem::take(&mut self.refs).into_iter().filter_map(|(reference, kind)| Some((to(&reference)?, kind))).collect();
        self.known = std::mem::take(&mut self.known).into_iter().filter_map(|(reference, track)| Some((to(&reference)?, track))).collect();
        let names = std::mem::take(&mut self.short);
        self.long.clear();
        for (reference, name) in names {
            if let Some(reference) = to(&reference) {
                self.long.insert(name.clone(), reference.clone());
                self.short.insert(reference, name);
            }
        }
        self.moved = moved;
        self.cursors.clear();
    }
    /// Refs moved by a restructure outside the book (a change's undo in HISTORY), to read again before Python uses them.
    pub fn mark_moved(&mut self, references: impl IntoIterator<Item = String>) {
        self.moved.extend(references);
    }
    /// A ref Live has just registered at its place (a read, or a change that made it): nothing to read again.
    pub fn registered(&mut self, reference: &str) {
        self.moved.shift_remove(reference);
    }
    /// The places to read again before Live runs Python that names these texts (its code, its `ref`): one for each
    /// moved ref they name. A place is what the Remote Script's `_refresh` reads again: a track, with everything on it
    /// but its Arrangement clips (`E:track:N`); a track's Arrangement clips (any one of them); a scene.
    pub fn moved_places(&self, texts: &[&str]) -> Vec<String> {
        static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+:[a-z_]+:[0-9a-z_:-]+").unwrap());
        let mut places: Vec<String> = Vec::new();
        if self.moved.is_empty() {
            return places;
        }
        for text in texts {
            for found in REF.find_iter(text) {
                let reference = found.as_str().trim_end_matches(':');
                if self.moved.contains(reference) {
                    let place = place_of(reference);
                    if !places.contains(&place) {
                        places.push(place);
                    }
                }
            }
        }
        places
    }
    /// Live read these places again (`moved_places`): what's on them is registered as it is now.
    pub fn read_again(&mut self, places: &[String]) {
        self.moved.retain(|reference| !places.iter().any(|place| *place == place_of(reference)));
    }
    pub fn invalidate(&mut self) {
        self.refs.clear();
        self.cursors.clear();
        self.known.clear();
    }
    pub fn register_rows(
        &mut self,
        kind: &str,
        rows: &[JsonObject],
        args: &JsonObject,
        next_cursor: Option<&str>,
    ) -> Result<(), ReferenceError> {
        for row in rows {
            if args.contains_key("parent") && !same_primitive(row.get("parentRef"), args.get("parent")) {
                return Err(error("Discovery returned a different parent; result discarded").into());
            }
            if let Some(reference) = row.get("ref").and_then(Value::as_str).filter(|s| !s.is_empty() && utf16_len(s) <= 256) {
                self.refs.insert(reference.into(), kind.into());
                self.moved.shift_remove(reference);
                if kind == "clip-slot" {
                    if let Some(clip) = row.get("clipRef").and_then(Value::as_str).filter(|s| utf16_len(s) <= 256) {
                        self.refs.insert(clip.into(), "session-clip".into());
                        self.moved.shift_remove(clip);
                    }
                }
                if kind == "device" {
                    if let Some(chains) = row.get("chainList").and_then(Value::as_array) {
                        for chain in chains {
                            if let Some(reference) = chain.get("ref").and_then(Value::as_str).filter(|s| utf16_len(s) <= 256) {
                                self.refs.insert(reference.into(), "chain".into());
                            }
                        }
                    }
                }
                if kind.ends_with("track") {
                    if let Some(name) = row.get("name").and_then(Value::as_str) {
                        self.known
                            .insert(reference.into(), KnownTrack { name: head(name, 256), color: row.get("color").and_then(hex_color) });
                    }
                }
            }
        }
        if self.refs.len() > 1_000_000 {
            self.invalidate();
            return Err(ReferenceError { error: error("Too many current references; refresh and narrow the request"), invalidate: true });
        }
        if let Some(cursor) = next_cursor.filter(|s| !s.is_empty()) {
            if args.get("cursor").and_then(Value::as_str) == Some(cursor) {
                return Err(error("Discovery cursor repeated; narrow the request").into());
            }
            self.cursors.insert(cursor.into(), query_key(args));
            if self.cursors.len() > 100_000 {
                self.invalidate();
                return Err(ReferenceError { error: error("Too many page cursors; refresh and narrow the request"), invalidate: true });
            }
        }
        Ok(())
    }
    pub fn validate_parent_and_cursor(&self, args: &JsonObject) -> Result<(), ObservationError> {
        let kind = js_string(args.get("kind"));
        let parent_kinds = PARENTS.get(&kind).and_then(Value::as_array);
        if parent_kinds.is_some() || args.contains_key("parent") {
            let parent = args.get("parent").and_then(Value::as_str).and_then(|s| self.refs.get(s));
            let takes = parent_kinds.map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>()).unwrap_or_else(|| vec!["set"]);
            let parent = parent.ok_or_else(|| {
                if !args.contains_key("parent") {
                    error(format!("{kind} needs a parent: a {} from this turn", takes.join(" or ")))
                } else {
                    error("A fresh authoritative parent is required; discover the parent in this turn, not from history")
                }
            })?;
            if !takes.contains(&parent.as_str()) {
                return Err(error(format!(
                    "{kind} takes a {} as its parent, not a {parent}{}",
                    takes.join(" or "),
                    if kind == "session-clip" && parent == "track" {
                        ": discover the track's clip-slots, each gives its clipRef"
                    } else {
                        ""
                    }
                )));
            }
        }
        if let Some(cursor) = args.get("cursor") {
            if cursor.as_str().and_then(|s| self.cursors.get(s)) != Some(&query_key(args)) {
                return Err(error("Cursor is stale or belongs to another query; rediscover without it"));
            }
        }
        Ok(())
    }
    pub fn short_ref(&mut self, reference: &str) -> String {
        static LIVE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:([a-z][a-z_]{0,31}):").unwrap());
        let Some(captures) = LIVE_REF.captures(reference) else { return reference.into() };
        if let Some(name) = self.short.get(reference) {
            return name.clone();
        }
        if self.short.len() >= 10_000_000 {
            self.short.clear();
            self.long.clear();
        }
        let kind = &captures[1];
        let count = self.counts.entry(kind.into()).or_insert(0);
        *count += 1;
        let name = format!("{kind}:{count}");
        self.short.insert(reference.into(), name.clone());
        self.long.insert(name.clone(), reference.into());
        name
    }
    pub fn shorten(&mut self, value: &Value) -> Value {
        self.shorten_at(value, "", 0)
    }
    fn shorten_at(&mut self, value: &Value, key: &str, depth: usize) -> Value {
        if depth > 32 {
            return value.clone();
        }
        match value {
            Value::String(s) if ref_key(key) => json!(self.short_ref(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.shorten_at(v, key, depth + 1)).collect()),
            Value::Object(row) => {
                Value::Object(row.iter().map(|(key, value)| (key.clone(), self.shorten_at(value, key, depth + 1))).collect())
            }
            _ => value.clone(),
        }
    }
    pub fn lengthen(&self, value: &Value) -> Value {
        self.lengthen_at(value, "", 0)
    }
    fn lengthen_at(&self, value: &Value, key: &str, depth: usize) -> Value {
        if depth > 32 {
            return value.clone();
        }
        match value {
            Value::String(s) if ref_key(key) => json!(self.long.get(s).unwrap_or(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.lengthen_at(v, key, depth + 1)).collect()),
            Value::Object(row) => {
                Value::Object(row.iter().map(|(key, value)| (key.clone(), self.lengthen_at(value, key, depth + 1))).collect())
            }
            _ => value.clone(),
        }
    }
    pub fn require_fresh_references(&self, args: &JsonObject) -> Result<(), ObservationError> {
        self.require_at(args, 0)
    }
    fn require_at(&self, args: &JsonObject, depth: usize) -> Result<(), ObservationError> {
        for field in REFERENCE_FIELDS.iter() {
            if let Some(value) = args.get(field) {
                if !value.as_str().is_some_and(|s| self.refs.contains_key(s)) {
                    return Err(error(format!(
                        "{field} must come from discovery in this turn; discover it again{}",
                        if field == "parameterRef" {
                            " (or name the parameter instead, with parameter \"Filter Freq\", and the device's deviceRef from this turn's observation)"
                        } else {
                            ""
                        }
                    )));
                }
            }
        }
        if depth < 2 {
            for value in args.values() {
                if let Some(items) = value.as_array() {
                    for item in items {
                        if let Some(row) = item.as_object() {
                            self.require_at(row, depth + 1)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn encode(&mut self, result: &CallToolResult, epoch: f64, slim: bool, observed_at: &str, generation: &str) -> ToolResult {
        if result.is_error == Some(true) {
            return ToolResult::error(stringify(&serde_json::to_value(result).unwrap()));
        }
        let mut live = payload(result).map(Value::Object).unwrap_or_else(|_| serde_json::to_value(result).unwrap());
        if slim {
            if let Some(row) = live.as_object() {
                live = Value::Object(slim_mixers(row));
            }
        }
        let text = stringify(
            &json!({"live":self.shorten(&live),"observation":{"observedAt":observed_at,"connectionGeneration":generation,"epoch":epoch,"coverage":"Bounded read; preserve truncated/nextCursor markers. Traversal completeness is not established."}}),
        );
        if text.len() > 64 * 1024 {
            ToolResult::error("Result too large; narrow fields/parent/page.")
        } else {
            ToolResult::text(text)
        }
    }
}
pub fn slim_mixers(content: &JsonObject) -> JsonObject {
    let Some(items) = content.get("items").and_then(Value::as_array) else { return content.clone() };
    if !items.iter().any(|v| v.as_object().is_some_and(|r| r.contains_key("mixer"))) {
        return content.clone();
    }
    let keep = ["volume", "pan", "mute", "solo", "cueVolume", "sends", "volumeDisplay", "panDisplay", "cueVolumeDisplay", "sendDisplays"];
    let items: Vec<_> = items
        .iter()
        .map(|item| {
            let Some(row) = item.as_object() else { return item.clone() };
            let Some(mixer) = row.get("mixer").filter(|v| v.is_object() || v.is_array()) else { return item.clone() };
            let mut row = row.clone();
            row.insert(
                "mixer".into(),
                Value::Object(
                    mixer
                        .as_object()
                        .map(|m| m.iter().filter(|(key, _)| keep.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                ),
            );
            Value::Object(row)
        })
        .collect();
    let mut result = content.clone();
    result.insert("items".into(), json!(items));
    result
}

fn same_primitive(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) | (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(Value::String(a)), Some(Value::String(b))) => a == b,
        (Some(Value::Bool(a)), Some(Value::Bool(b))) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shift_moves_every_ref_past_the_change_and_retires_what_was_deleted() {
        let to = |shift: &Shift, reference: &str| shift.reference(reference);
        // Track 5 of 8 deleted (Main at 8): what was after it moves up one, what was before stays.
        let gone = Shift { tracks_gone: vec![5], ..Default::default() };
        assert_eq!(to(&gone, "7:track:4"), Moved::Same);
        assert_eq!(to(&gone, "7:track:5"), Moved::Gone);
        assert_eq!(to(&gone, "7:track:8"), Moved::To("7:track:7".into()));
        assert_eq!(to(&gone, "7:device:6:0:1:2"), Moved::To("7:device:5:0:1:2".into()));
        assert_eq!(to(&gone, "7:clip:6:3"), Moved::To("7:clip:5:3".into()));
        assert_eq!(to(&gone, "7:clip:5:3:note:0"), Moved::Gone);
        // A ref built on another follows it: a device's parameter, a chain's volume, a rack's return chain.
        assert_eq!(to(&gone, "7:parameter:7:device:6:0:12"), Moved::To("7:parameter:7:device:5:0:12".into()));
        assert_eq!(to(&gone, "7:parameter:7:chain:6:0:1:volume"), Moved::To("7:parameter:7:chain:5:0:1:volume".into()));
        assert_eq!(to(&gone, "7:parameter:mixer:6:sends:1"), Moved::To("7:parameter:mixer:5:sends:1".into()));
        // A track's selected-device ref isn't a place Live reads again, so it's retired once its track moves.
        assert_eq!(to(&gone, "7:device:view:6"), Moved::Gone);
        assert_eq!(to(&gone, "7:device:view:4"), Moved::Same);
        // Scenes, locators and the Set aren't on a track.
        assert_eq!(to(&gone, "7:scene:6"), Moved::Same);
        assert_eq!(to(&gone, "7:locator:6"), Moved::Same);
        // Two tracks made at 2 and 4 (their places once made): the old 2 is now 3, the old 3 is now 5.
        let made = Shift { tracks_made: vec![4, 2], ..Default::default() };
        assert_eq!(to(&made, "7:track:1"), Moved::Same);
        assert_eq!(to(&made, "7:track:2"), Moved::To("7:track:3".into()));
        assert_eq!(to(&made, "7:track:3"), Moved::To("7:track:5".into()));
        // A scene made at 1 moves clips in later scenes, on every track.
        let scene = Shift { scenes_made: vec![1], ..Default::default() };
        assert_eq!(to(&scene, "7:clip_slot:4:0"), Moved::Same);
        assert_eq!(to(&scene, "7:clip_slot:4:1"), Moved::To("7:clip_slot:4:2".into()));
        assert_eq!(to(&scene, "7:scene:3"), Moved::To("7:scene:4".into()));
        // A return deleted renumbers every track's sends, so they're retired.
        let returns = Shift { tracks_gone: vec![9], sends: true, ..Default::default() };
        assert_eq!(to(&returns, "7:parameter:mixer:2:sends:0"), Moved::Gone);
        assert_eq!(to(&returns, "7:parameter:mixer:2:volume"), Moved::Same);
    }

    #[test]
    fn short_names_follow_their_refs_and_a_deleted_ones_name_names_nothing() {
        let mut book = References::default();
        for index in 0..4 {
            book.refs.insert(format!("7:track:{index}"), "track".into());
        }
        let names: Vec<String> = (0..4).map(|index| book.short_ref(&format!("7:track:{index}"))).collect();
        book.shift(&Shift { tracks_gone: vec![1], ..Default::default() });
        assert_eq!(book.refs.keys().cloned().collect::<Vec<_>>(), ["7:track:0", "7:track:1", "7:track:2"]);
        assert_eq!(book.lengthen(&json!({"ref":names[2]}))["ref"], "7:track:1");
        assert_eq!(book.lengthen(&json!({"ref":names[1]}))["ref"], json!(names[1]));
        // A name keeps its object: the track now at 1 is still called by the old 2's name.
        assert_eq!(book.short_ref("7:track:1"), names[2]);
    }

    #[test]
    fn a_moved_ref_names_the_place_live_reads_again_before_python_uses_it() {
        let mut book = References::default();
        for reference in ["7:track:0", "7:device:2:0", "7:parameter:7:device:2:0:3", "7:chain:7:device:2:0:return:0"] {
            book.refs.insert(reference.into(), "x".into());
        }
        for reference in ["7:parameter:mixer:2:volume", "7:clip:2:0", "7:arrangement_clip:2:0", "7:scene:0", "7:device:view:2"] {
            book.refs.insert(reference.into(), "x".into());
        }
        book.shift(&Shift { tracks_gone: vec![1], ..Default::default() });
        // Track 0 didn't move: nothing to read again.
        assert!(book.moved_places(&["7:track:0"]).is_empty());
        // What was on track 2 is on 1 now, and Live still holds old track 1's under those strings: Python naming them
        // has track 1 read again first, once for all of them.
        let code = r#"ARGS = json.loads("[{\"ref\":\"7:parameter:7:device:1:0:3\"},{\"device\":\"7:device:1:0\"}]")"#;
        assert_eq!(book.moved_places(&[code]), ["7:track:1"]);
        // A rack's return chain and a mixer's parameter are on their track too; Arrangement clips are read apart.
        let more = "7:chain:7:device:1:0:return:0 7:parameter:mixer:1:volume 7:arrangement_clip:1:0";
        assert_eq!(book.moved_places(&[more]), ["7:track:1", "7:arrangement_clip:1:0"]);
        assert!(!book.refs.contains_key("7:device:view:1"), "a view ref is retired, not moved");
        // Track 1 read again: what's on it is current, but not its Arrangement clips, which its row doesn't list.
        book.read_again(&["7:track:1".to_owned()]);
        assert!(book.moved_places(&[code]).is_empty());
        assert_eq!(book.moved_places(&[more]), ["7:arrangement_clip:1:0"]);
        // Live registering a ref (a read, a change that made it) leaves nothing to read again there.
        book.registered("7:arrangement_clip:1:0");
        assert!(book.moved_places(&[more]).is_empty());
        // A scene made first moves every scene, and every clip, after it; what was marked moves along.
        book.shift(&Shift { scenes_made: vec![0], ..Default::default() });
        assert_eq!(book.moved_places(&["7:scene:1", "7:clip:1:1"]), ["7:scene:1", "7:track:1"]);
        assert!(book.moved_places(&["7:clip:1:0"]).is_empty(), "nothing Kumi knows is at scene 0 now");
    }
}
