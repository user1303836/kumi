//! The refs a turn's reads hand the model, as the DAW's bridge names its objects: which are current (only refs from
//! this turn's discovery may be changed), their short names (`track:5`, a name keeps naming one object), discovery
//! cursors, and, for a DAW whose refs are positions, where a restructure moved them. How a ref is spelled and what it
//! points at is the DAW's (`RefScheme`).
use crate::core::contracts::{JsonObject, TrackChip};
use indexmap::{IndexMap, IndexSet};
use kumi_common::js::{
    json::stringify,
    string::{head, locale_compare, utf16_len},
};
use serde_json::{json, Value};

/// A read or a change the turn can't trust: a ref not from this turn, a stale cursor, a parent of the wrong kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ObservationError(pub String);

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

/// A known track, as the turn's reads named it: its name and colour.
pub type KnownTrack = TrackChip;

/// A parameter's name and range, as discovery read them.
#[derive(Clone, Debug, PartialEq)]
pub struct KnownParameter {
    pub name: String,
    pub min: f64,
    pub max: f64,
}
const KNOWN_PARAMETERS: usize = 4096;

/// Refs and their short names kept to put back (`Book::put_back`).
#[derive(Default)]
pub struct KeptRefs {
    pub refs: Vec<(String, String)>,
    pub names: Vec<(String, String)>,
}

/// A discovery query's key, without its cursor: a cursor belongs to the query that gave it.
pub fn query_key(args: &JsonObject) -> String {
    fn ordered(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut fields: Vec<_> = map.iter().collect();
                fields.sort_by(|a, b| locale_compare(a.0, b.0));
                Value::Object(fields.into_iter().map(|(k, v)| (k.clone(), ordered(v))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(ordered).collect()),
            other => other.clone(),
        }
    }
    let mut query = args.clone();
    query.shift_remove("cursor");
    stringify(&ordered(&Value::Object(query)))
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
/// Kumi's tools carry refs under these keys: `ref`, `parent`, and any `…Ref` or `…Refs`.
fn ref_key(key: &str) -> bool {
    key == "ref" || key == "parent" || key.ends_with("Ref") || key.ends_with("Refs")
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

/// Where a restructure moved the project's tracks and scenes, so the refs past it follow what they named rather than
/// being dropped (#261, #253): for a DAW whose refs are positions.
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
}
/// What a restructure did to a ref.
#[derive(Debug, Clone, PartialEq)]
pub enum Moved {
    Same,
    To(String),
    Gone,
}
/// An index after `gone` (places before) are deleted and `made` (places after) are made.
pub fn moved_index(index: usize, gone: &[usize], made: &[usize]) -> Option<usize> {
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

/// How a DAW's bridge spells its refs and what they point at.
pub trait RefScheme: Default {
    /// Whether refs are positions, so a restructure moves them (Live's and Renoise's; REAPER's GUIDs aren't).
    fn positional(&self) -> bool;
    /// The kind a ref names, for its short name (`track` for Live's `7:track:3`); None for text that isn't a ref.
    fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str>;
    /// Where a ref points after a restructure (asked only of a positional scheme).
    fn moved(&self, shift: &Shift, reference: &str) -> Moved;
    /// The place the DAW reads a moved ref's object at again.
    fn place_of(&self, reference: &str) -> String;
    /// The refs a text names (code to run in the DAW, its `ref`).
    fn refs_in<'a>(&self, text: &'a str) -> Vec<&'a str>;
    /// The fields a change takes a ref in, each from this turn's discovery.
    fn reference_fields(&self) -> &[String];
    /// The kinds a discovery kind takes as its parent; None for one that names none.
    fn parents(&self, kind: &str) -> Option<Vec<String>>;
    /// What to add when a discovery kind is given a parent of the wrong kind ("discover the track's clip-slots…").
    fn parent_hint(&self, kind: &str, parent: &str) -> &'static str;
    /// Refs a discovery row names beside its own (a clip slot's clip, a rack's chains): each with its kind, and
    /// whether the DAW has registered it at its place by this read.
    fn row_refs(&self, kind: &str, row: &JsonObject) -> Vec<(String, String, bool)>;
    /// The long ref of the track a long ref is on, if it's on one.
    fn track_of(&self, reference: &str) -> Option<String>;
    /// Refs every project has whatever its epoch, which no read lists (Logic's Stereo Out, `<epoch>:output:1`): each
    /// long ref in `epoch`, its kind and what it is called. The book names them as it names any ref and the name stays
    /// with the object from one epoch to the next; the observation lists them.
    fn standing(&self, _epoch: &str) -> Vec<(String, &'static str, &'static str)> {
        Vec::new()
    }
}

/// The turn's refs, by the DAW's scheme.
#[derive(Default)]
pub struct Book<S: RefScheme> {
    pub refs: IndexMap<String, String>,
    pub cursors: IndexMap<String, String>,
    pub known: IndexMap<String, KnownTrack>,
    /// Parameters as discovery last read them, by ref: a picture of one's automation is drawn to its range.
    pub parameters: IndexMap<String, KnownParameter>,
    pub(crate) short: IndexMap<String, String>,
    long: IndexMap<String, String>,
    counts: IndexMap<String, u64>,
    /// Refs a restructure moved to a place the DAW hasn't been read at since. A DAW whose registry keeps objects by
    /// these strings still holds, under each, what was at that place when it was last read there; code run in the DAW
    /// has the places it names read again first (`moved_places`), or it would act on what used to be there.
    moved: IndexSet<String>,
    /// The scheme's standing refs as the book stands them now (`stand`), by what each is called: its long ref, its kind
    /// and its short name.
    standing: IndexMap<&'static str, (String, String, String)>,
    pub scheme: S,
}

impl<S: RefScheme> Book<S> {
    /// Retired names never acquire a different object; counters continue across retirement. A standing ref's name
    /// stays: its object is there whatever moved.
    pub fn clear_names(&mut self) {
        self.short.clear();
        self.long.clear();
        self.name_standing();
    }
    /// Keeps the names of the refs `keep` holds to; the others retire (counters go on, so none is given again).
    pub fn keep_names(&mut self, keep: impl Fn(&S, &str) -> bool) {
        let scheme = &self.scheme;
        self.short.retain(|reference, _| keep(scheme, reference));
        self.long.retain(|_, reference| keep(scheme, reference));
        self.name_standing();
    }
    /// The scheme's standing refs in `epoch`, registered and named, each name kept from the epoch before: each short
    /// name with what it is called.
    pub fn stand(&mut self, epoch: &str) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        for (reference, kind, called) in self.scheme.standing(epoch) {
            let short = match self.standing.get(called) {
                Some((before, _, short)) => {
                    let short = short.clone();
                    if *before != reference {
                        self.short.shift_remove(before);
                    }
                    short
                }
                None => self.short_ref(&reference),
            };
            self.refs.insert(reference.clone(), kind.into());
            self.standing.insert(called, (reference, kind.into(), short.clone()));
            out.push((short, called));
        }
        self.name_standing();
        out
    }
    /// The standing refs' names, put (back) in the maps.
    fn name_standing(&mut self) {
        for (reference, _, short) in self.standing.values() {
            self.short.insert(reference.clone(), short.clone());
            self.long.insert(short.clone(), reference.clone());
        }
    }
    /// The kind of a ref the book knows: one this turn's reads named, or a standing one.
    fn kind_known(&self, reference: &str) -> Option<&String> {
        self.refs.get(reference).or_else(|| self.standing.values().find(|(standing, _, _)| standing == reference).map(|(_, kind, _)| kind))
    }
    pub fn unname(&mut self, reference: &str) {
        if let Some(short) = self.short.shift_remove(reference) {
            self.long.shift_remove(&short);
        }
    }
    pub fn retire(&mut self, reference: &str) {
        self.refs.shift_remove(reference);
        self.known.shift_remove(reference);
        self.parameters.shift_remove(reference);
        self.unname(reference);
    }
    /// Puts back refs kept, with their short names: a name given to one of them since (Kumi's own reads and steps
    /// meanwhile, which the model never saw) gives way to the one the model knows.
    pub fn put_back(&mut self, kept: KeptRefs) {
        for (reference, kind) in kept.refs {
            self.refs.entry(reference).or_insert(kind);
        }
        for (reference, short) in kept.names {
            // A retired name never names anything else, so this only guards against a cleared book.
            if self.long.get(&short).is_some_and(|other| *other != reference) {
                continue;
            }
            if let Some(since) = self.short.insert(reference.clone(), short.clone()).filter(|since| *since != short) {
                self.long.shift_remove(&since);
            }
            self.long.insert(short, reference);
        }
    }
    pub fn named_references(&self) -> Vec<String> {
        self.short.keys().cloned().collect()
    }
    /// Where a ref is after a shift, or None for what was deleted.
    fn shifted(&self, shift: &Shift, reference: &str) -> Option<String> {
        match self.scheme.moved(shift, reference) {
            Moved::Same => Some(reference.to_owned()),
            Moved::To(now) => Some(now),
            Moved::Gone => None,
        }
    }
    /// After a restructure, for a DAW whose refs are positions: every ref past it follows what it named to its new
    /// place, short names and all, and those to what was deleted are retired. A name keeps naming one object, so a
    /// plan's later steps still find theirs. Discovery cursors were for the old places, so they go. A DAW whose refs
    /// aren't positions keeps them all as they are.
    pub fn shift(&mut self, shift: &Shift) {
        if !self.scheme.positional() {
            return;
        }
        // What was marked moved goes with the shift too: a place not read again since is still stale.
        let moved_before = std::mem::take(&mut self.moved);
        let mut moved: IndexSet<String> = moved_before.into_iter().filter_map(|reference| self.shifted(shift, &reference)).collect();
        let refs = std::mem::take(&mut self.refs);
        let known = std::mem::take(&mut self.known);
        let parameters = std::mem::take(&mut self.parameters);
        let names = std::mem::take(&mut self.short);
        let mut to = |reference: &str| {
            let now = self.shifted(shift, reference)?;
            if now != reference {
                moved.insert(now.clone());
            }
            Some(now)
        };
        let refs: IndexMap<_, _> = refs.into_iter().filter_map(|(reference, kind)| Some((to(&reference)?, kind))).collect();
        let known: IndexMap<_, _> = known.into_iter().filter_map(|(reference, track)| Some((to(&reference)?, track))).collect();
        let parameters: IndexMap<_, _> = parameters.into_iter().filter_map(|(reference, known)| Some((to(&reference)?, known))).collect();
        let mut short = IndexMap::new();
        let mut long = IndexMap::new();
        for (reference, name) in names {
            if let Some(reference) = to(&reference) {
                long.insert(name.clone(), reference.clone());
                short.insert(reference, name);
            }
        }
        self.refs = refs;
        self.known = known;
        self.parameters = parameters;
        self.short = short;
        self.long = long;
        self.moved = moved;
        self.cursors.clear();
    }
    /// Refs moved by a restructure outside the book (a change's undo in HISTORY), to read again before code uses them.
    pub fn mark_moved(&mut self, references: impl IntoIterator<Item = String>) {
        self.moved.extend(references);
    }
    /// A ref the DAW has just registered at its place (a read, or a change that made it): nothing to read again.
    pub fn registered(&mut self, reference: &str) {
        self.moved.shift_remove(reference);
    }
    /// The places to read again before the DAW runs code that names these texts (its code, its `ref`): one for each
    /// moved ref they name.
    pub fn moved_places(&self, texts: &[&str]) -> Vec<String> {
        let mut places: Vec<String> = Vec::new();
        if self.moved.is_empty() {
            return places;
        }
        for text in texts {
            for reference in self.scheme.refs_in(text) {
                if self.moved.contains(reference) {
                    let place = self.scheme.place_of(reference);
                    if !places.contains(&place) {
                        places.push(place);
                    }
                }
            }
        }
        places
    }
    /// The DAW read these places again (`moved_places`): what's on them is registered as it is now.
    pub fn read_again(&mut self, places: &[String]) {
        let scheme = &self.scheme;
        self.moved.retain(|reference| !places.iter().any(|place| *place == scheme.place_of(reference)));
    }
    pub fn invalidate(&mut self) {
        self.refs.clear();
        self.cursors.clear();
        self.known.clear();
        self.parameters.clear();
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
                for (other, other_kind, registered) in self.scheme.row_refs(kind, row) {
                    if registered {
                        self.moved.shift_remove(&other);
                    }
                    self.refs.insert(other, other_kind);
                }
                if kind == "parameter" {
                    self.know_parameter(reference, row);
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
    /// Keeps a parameter's name and range when its row has them; a row naming it otherwise forgets what was kept.
    fn know_parameter(&mut self, reference: &str, row: &JsonObject) {
        let Some(name) = row.get("name").and_then(Value::as_str).map(|name| head(name, 256)) else { return };
        let (min, max) = (row.get("min").and_then(Value::as_f64), row.get("max").and_then(Value::as_f64));
        match (min, max) {
            (Some(min), Some(max)) if min.is_finite() && max.is_finite() && max > min => {
                if !self.parameters.contains_key(reference) && self.parameters.len() >= KNOWN_PARAMETERS {
                    self.parameters.shift_remove_index(0);
                }
                self.parameters.insert(reference.into(), KnownParameter { name, min, max });
            }
            _ if self.parameters.get(reference).is_some_and(|known| known.name != name) => {
                self.parameters.shift_remove(reference);
            }
            _ => {}
        }
    }
    pub fn validate_parent_and_cursor(&self, args: &JsonObject) -> Result<(), ObservationError> {
        let kind = js_string(args.get("kind"));
        let parent_kinds = self.scheme.parents(&kind);
        if parent_kinds.is_some() || args.contains_key("parent") {
            let parent = args.get("parent").and_then(Value::as_str).and_then(|s| self.kind_known(s));
            let takes = parent_kinds.unwrap_or_else(|| vec!["set".into()]);
            let parent = parent.ok_or_else(|| {
                if !args.contains_key("parent") {
                    error(format!("{kind} needs a parent: a {} from this turn", takes.join(" or ")))
                } else {
                    error("A fresh authoritative parent is required; discover the parent in this turn, not from history")
                }
            })?;
            if !takes.contains(parent) {
                return Err(error(format!(
                    "{kind} takes a {} as its parent, not a {parent}{}",
                    takes.join(" or "),
                    self.scheme.parent_hint(&kind, parent)
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
        let Some(kind) = self.scheme.kind_of(reference) else { return reference.into() };
        if let Some(name) = self.short.get(reference) {
            return name.clone();
        }
        if self.short.len() >= 10_000_000 {
            self.short.clear();
            self.long.clear();
        }
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
        for field in self.scheme.reference_fields() {
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
}

/// A colour as a bridge sends it (an RGB integer), as `#rrggbb`.
pub fn hex_color(value: &Value) -> Option<String> {
    value.as_f64().filter(|v| v.fract() == 0.0 && (0.0..=16777215.0).contains(v)).map(|v| format!("#{:06x}", v as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refs as `<kind>:<index>`, positional or not by the scheme.
    #[derive(Default)]
    struct Simple<const POSITIONAL: bool>;
    impl<const POSITIONAL: bool> RefScheme for Simple<POSITIONAL> {
        fn positional(&self) -> bool {
            POSITIONAL
        }
        fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str> {
            reference.split_once(':').map(|(kind, _)| kind)
        }
        fn moved(&self, shift: &Shift, reference: &str) -> Moved {
            let Some(index) = reference.strip_prefix("track:").and_then(|index| index.parse::<usize>().ok()) else { return Moved::Same };
            match shift.track(index) {
                None => Moved::Gone,
                Some(now) if now != index => Moved::To(format!("track:{now}")),
                Some(_) => Moved::Same,
            }
        }
        fn place_of(&self, reference: &str) -> String {
            reference.into()
        }
        fn refs_in<'a>(&self, _text: &'a str) -> Vec<&'a str> {
            vec![]
        }
        fn reference_fields(&self) -> &[String] {
            &[]
        }
        fn parents(&self, _kind: &str) -> Option<Vec<String>> {
            None
        }
        fn parent_hint(&self, _kind: &str, _parent: &str) -> &'static str {
            ""
        }
        fn row_refs(&self, _kind: &str, _row: &JsonObject) -> Vec<(String, String, bool)> {
            vec![]
        }
        fn track_of(&self, _reference: &str) -> Option<String> {
            None
        }
    }

    fn book<const POSITIONAL: bool>() -> Book<Simple<POSITIONAL>> {
        let mut book = Book::<Simple<POSITIONAL>>::default();
        for reference in ["track:0", "track:1", "track:2"] {
            book.refs.insert(reference.into(), "track".into());
            book.short_ref(reference);
        }
        book
    }

    #[test]
    fn positional_refs_follow_a_restructure_and_stable_ones_stay_as_they_are() {
        // The track at 1 deleted: the one at 2 moves to 1 and keeps its short name; the deleted one's goes.
        let shift = Shift { tracks_gone: vec![1], ..Shift::default() };
        let mut moving = book::<true>();
        moving.shift(&shift);
        assert_eq!(moving.refs.keys().collect::<Vec<_>>(), ["track:0", "track:1"]);
        assert_eq!(moving.lengthen(&json!({"ref": "track:3"}))["ref"], "track:1", "the short name follows its track");
        let mut stable = book::<false>();
        stable.shift(&shift);
        assert_eq!(stable.refs.keys().collect::<Vec<_>>(), ["track:0", "track:1", "track:2"]);
        assert_eq!(stable.lengthen(&json!({"ref": "track:3"}))["ref"], "track:2");
    }
}
