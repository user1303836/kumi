//! A Max patcher as Kumi reads, checks and lays it out: its boxes in order (Max draws a box above the boxes listed
//! after it in its layer), its cords, and every other field as Max wrote it, so a patcher read and written back is the
//! same JSON. A box's own patcher (a [p], an embedded bpatcher, a gen~) is read with it, and the file an abstraction or
//! a bpatcher names is read through [`Files`] (a frozen device's own files, or none).
//!
//! The standard Kumi holds patchers to is data in `max-standard/` at the repository's root: each rule's level, what
//! the rules know of Max's objects, and what measuring well-made devices found. The model never reads it: it hears a
//! broken rule in one line, with the fix.

pub mod catalog;
pub mod check;
pub mod frozen;
pub mod geometry;
pub mod measure;
pub mod standard;

use serde_json::{json, Map, Value};

use geometry::Rect;

/// The files a patcher's abstractions and bpatchers name.
pub trait Files {
    /// The JSON of the patcher file `name` (`dial.maxpat`), if there is one.
    fn patcher(&self, name: &str) -> Option<Value>;
}

/// No files: only what's embedded is read.
pub struct NoFiles;

impl Files for NoFiles {
    fn patcher(&self, _name: &str) -> Option<Value> {
        None
    }
}

/// How deep files are read inside each other: an abstraction that holds itself stops here.
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Patcher {
    pub boxes: Vec<MaxBox>,
    pub cords: Vec<Cord>,
    /// Every other field, in the order Max wrote them; "boxes" and "lines" keep their places as nulls.
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaxBox {
    /// The box's fields as Max wrote them; an embedded patcher keeps its place as a null.
    pub fields: Map<String, Value>,
    /// The patcher embedded in the box: a [p], a bpatcher with its patcher embedded, a gen~.
    pub patcher: Option<Patcher>,
    /// The file the box loads (an abstraction, a bpatcher, a poly~'s voice), read through [`Files`]: its name and
    /// patcher. It's never written back into the box.
    pub file: Option<(String, Patcher)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cord {
    pub from: String,
    pub outlet: usize,
    pub to: String,
    pub inlet: usize,
    /// The points a segmented cord bends at, between its outlet and its inlet.
    pub midpoints: Vec<[f64; 2]>,
    /// Its other fields (order, hidden, color) and the midpoints as read; source and destination keep their places as
    /// nulls.
    pub fields: Map<String, Value>,
}

impl Patcher {
    /// A patcher from Max's JSON, `{ "patcher": {...} }` or the patcher itself, with the files it names read through
    /// `files`.
    pub fn read(value: &Value, files: &dyn Files) -> Patcher {
        Patcher::read_within(value, files, &mut Vec::new())
    }

    fn read_within(value: &Value, files: &dyn Files, open: &mut Vec<String>) -> Patcher {
        let object = value.get("patcher").filter(|inner| inner.is_object()).unwrap_or(value);
        let mut fields = object.as_object().cloned().unwrap_or_default();
        let boxes = match fields.get_mut("boxes").map(Value::take) {
            Some(Value::Array(boxes)) => boxes,
            _ => Vec::new(),
        };
        let lines = match fields.get_mut("lines").map(Value::take) {
            Some(Value::Array(lines)) => lines,
            _ => Vec::new(),
        };
        let boxes = boxes.into_iter().filter_map(|entry| MaxBox::read(entry, files, open)).collect();
        let cords = lines.iter().filter_map(Cord::read).collect();
        Patcher { boxes, cords, fields }
    }

    /// The patcher as Max's JSON (without the `{ "patcher": … }` around it).
    pub fn to_value(&self) -> Value {
        let mut fields = self.fields.clone();
        let boxes = Value::Array(self.boxes.iter().map(|item| json!({ "box": item.to_value() })).collect());
        let lines = Value::Array(self.cords.iter().map(|cord| json!({ "patchline": cord.to_value() })).collect());
        fields.insert("boxes".into(), boxes);
        fields.insert("lines".into(), lines);
        Value::Object(fields)
    }

    /// The patcher as a document: `{ "patcher": … }`, as a device file and a .maxpat hold it.
    pub fn to_document(&self) -> Value {
        json!({ "patcher": self.to_value() })
    }

    pub fn find(&self, id: &str) -> Option<&MaxBox> {
        self.boxes.iter().find(|item| item.id() == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.boxes.iter().position(|item| item.id() == id)
    }

    /// Whether this is a gen patcher (gen~ or gen), whose boxes are gen's operators rather than Max's objects.
    pub fn is_gen(&self) -> bool {
        self.fields.get("classnamespace").and_then(Value::as_str).is_some_and(|space| space.starts_with("dsp."))
    }
}

impl MaxBox {
    fn read(entry: Value, files: &dyn Files, open: &mut Vec<String>) -> Option<MaxBox> {
        let mut fields = match entry {
            Value::Object(mut entry) => match entry.remove("box") {
                Some(Value::Object(fields)) => fields,
                _ => return None,
            },
            _ => return None,
        };
        let patcher = match fields.get_mut("patcher") {
            Some(inner) if inner.is_object() => Some(Patcher::read_within(&inner.take(), files, open)),
            _ => None,
        };
        let mut item = MaxBox { fields, patcher, file: None };
        if item.patcher.is_none() && open.len() < MAX_DEPTH {
            if let Some(name) = item.file_name() {
                if !open.contains(&name) {
                    if let Some(value) = files.patcher(&name) {
                        open.push(name.clone());
                        item.file = Some((name, Patcher::read_within(&value, files, open)));
                        open.pop();
                    }
                }
            }
        }
        Some(item)
    }

    /// The file the box would load: a bpatcher's, a poly~'s voice, a gen~'s @gen, or an abstraction named like it.
    fn file_name(&self) -> Option<String> {
        match self.maxclass() {
            "bpatcher" => self.fields.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_string),
            "newobj" => {
                let words = self.words();
                match words.first().copied()? {
                    "poly~" | "mc.poly~" => words.get(1).map(|name| with_extension(name, ".maxpat")),
                    "gen~" | "gen" | "mc.gen~" => words
                        .iter()
                        .position(|word| *word == "@gen")
                        .and_then(|at| words.get(at + 1))
                        .map(|name| with_extension(name, ".gendsp")),
                    class => Some(with_extension(class, ".maxpat")),
                }
            }
            _ => None,
        }
    }

    /// The box as Max's JSON (without the `{ "box": … }` around it).
    pub fn to_value(&self) -> Value {
        let mut fields = self.fields.clone();
        match &self.patcher {
            Some(patcher) => {
                fields.insert("patcher".into(), patcher.to_value());
            }
            None => {
                if fields.get("patcher") == Some(&Value::Null) {
                    fields.shift_remove("patcher");
                }
            }
        }
        Value::Object(fields)
    }

    pub fn id(&self) -> &str {
        self.str("id")
    }

    pub fn maxclass(&self) -> &str {
        self.str("maxclass")
    }

    /// A newobj's or a message's text ("" for a box without one).
    pub fn text(&self) -> &str {
        self.str("text")
    }

    /// A field's text, "" when it isn't text.
    pub fn str(&self, key: &str) -> &str {
        self.fields.get(key).and_then(Value::as_str).unwrap_or("")
    }

    /// A newobj's words: its class, then its arguments and attributes.
    pub fn words(&self) -> Vec<&str> {
        self.text().split_whitespace().collect()
    }

    /// What the box is: a newobj's first word ("prepend", "t"), else its maxclass ("live.dial", "message").
    pub fn class(&self) -> &str {
        if self.maxclass() == "newobj" {
            self.text().split_whitespace().next().unwrap_or("")
        } else {
            self.maxclass()
        }
    }

    /// A newobj's arguments: the words after its class, up to its first attribute.
    pub fn args(&self) -> Vec<&str> {
        if self.maxclass() != "newobj" {
            return Vec::new();
        }
        self.words().into_iter().skip(1).take_while(|word| !word.starts_with('@')).collect()
    }

    pub fn rect(&self) -> Option<Rect> {
        self.fields.get("patching_rect").and_then(Rect::of)
    }

    pub fn set_rect(&mut self, rect: Rect) {
        self.fields.insert("patching_rect".into(), rect.to_value());
    }

    pub fn presentation_rect(&self) -> Option<Rect> {
        self.fields.get("presentation_rect").and_then(Rect::of)
    }

    /// Whether the box is on the device's face (presentation).
    pub fn shown(&self) -> bool {
        self.fields.get("presentation").and_then(Value::as_f64) == Some(1.0)
    }

    pub fn inlets(&self) -> usize {
        self.fields.get("numinlets").and_then(Value::as_u64).unwrap_or(0) as usize
    }

    pub fn outlets(&self) -> usize {
        self.fields.get("numoutlets").and_then(Value::as_u64).unwrap_or(0) as usize
    }

    /// What an outlet sends, as Max lists it: "signal", "bang", "int", "" (anything) and so on.
    pub fn outlet_type(&self, index: usize) -> &str {
        self.fields.get("outlettype").and_then(|types| types.get(index)).and_then(Value::as_str).unwrap_or("")
    }

    /// Whether an outlet carries audio (signals have no order to get wrong).
    pub fn sends_signal(&self, index: usize) -> bool {
        matches!(self.outlet_type(index), "signal" | "multichannelsignal")
    }

    /// The patcher inside the box: embedded, or read from the file it names.
    pub fn inner(&self) -> Option<&Patcher> {
        self.patcher.as_ref().or(self.file.as_ref().map(|(_, patcher)| patcher))
    }

    /// The box's Live parameter attributes (its `valueof`), when it has them.
    pub fn valueof(&self) -> Option<&Map<String, Value>> {
        self.fields.get("saved_attribute_attributes")?.get("valueof")?.as_object()
    }

    /// How a note names the box: [prepend drive], live.dial "Drive", comment "Made by Kumi".
    pub fn label(&self) -> String {
        match self.maxclass() {
            "newobj" | "message" => format!("[{}]", clip(self.text(), 40)),
            "comment" => format!("comment \"{}\"", clip(self.text(), 30)),
            "bpatcher" if !self.str("name").is_empty() => format!("bpatcher {}", clip(self.str("name"), 40)),
            maxclass => {
                let name = self
                    .valueof()
                    .and_then(|valueof| valueof.get("parameter_longname"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .or(Some(self.str("varname")).filter(|name| !name.is_empty()));
                match name {
                    Some(name) => format!("{maxclass} \"{}\"", clip(name, 30)),
                    None => maxclass.to_string(),
                }
            }
        }
    }
}

impl Cord {
    fn read(entry: &Value) -> Option<Cord> {
        let line = entry.get("patchline")?.as_object()?;
        let end = |key: &str| -> Option<(String, usize)> {
            let end = line.get(key)?.as_array()?;
            Some((end.first()?.as_str()?.to_string(), end.get(1)?.as_u64()? as usize))
        };
        let (from, outlet) = end("source")?;
        let (to, inlet) = end("destination")?;
        let midpoints = line.get("midpoints").map(points).unwrap_or_default();
        let mut fields = line.clone();
        for key in ["source", "destination"] {
            if let Some(value) = fields.get_mut(key) {
                *value = Value::Null;
            }
        }
        Some(Cord { from, outlet, to, inlet, midpoints, fields })
    }

    pub fn new(from: &str, outlet: usize, to: &str, inlet: usize) -> Cord {
        Cord { from: from.into(), outlet, to: to.into(), inlet, ..Cord::default() }
    }

    /// The cord as Max's JSON (without the `{ "patchline": … }` around it).
    pub fn to_value(&self) -> Value {
        let mut fields = self.fields.clone();
        fields.insert("source".into(), json!([self.from, self.outlet]));
        fields.insert("destination".into(), json!([self.to, self.inlet]));
        // Midpoints as they were read keep Max's own numbers (20, not 20.0).
        if self.midpoints.is_empty() {
            fields.shift_remove("midpoints");
        } else if fields.get("midpoints").map(points).as_ref() != Some(&self.midpoints) {
            fields.insert(
                "midpoints".into(),
                Value::Array(self.midpoints.iter().flat_map(|point| [json!(point[0]), json!(point[1])]).collect()),
            );
        }
        Value::Object(fields)
    }
}

/// A segmented cord's midpoints as Max saves them, [x1, y1, x2, y2…], as points.
fn points(value: &Value) -> Vec<[f64; 2]> {
    let numbers: Vec<f64> = value.as_array().map(|numbers| numbers.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    numbers.chunks_exact(2).map(|pair| [pair[0], pair[1]]).collect()
}

fn with_extension(name: &str, extension: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{name}{extension}")
    }
}

/// Text cut to `most` characters, an ellipsis marking the cut.
pub(crate) fn clip(text: &str, most: usize) -> String {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > most {
        format!("{}…", text.chars().take(most - 1).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Library(HashMap<String, Value>);

    impl Files for Library {
        fn patcher(&self, name: &str) -> Option<Value> {
            self.0.get(name).cloned()
        }
    }

    #[test]
    fn a_patcher_read_and_written_back_is_the_same_json() {
        let document = json!({ "patcher": { "fileversion": 1, "rect": [0, 0, 100, 100], "boxes": [
            { "box": { "id": "obj-1", "maxclass": "newobj", "text": "p inner", "patching_rect": [10, 10, 50, 22], "numinlets": 0, "numoutlets": 0,
                "patcher": { "fileversion": 1, "boxes": [{ "box": { "id": "obj-1", "maxclass": "comment", "text": "hi" } }], "lines": [] } } },
            { "box": { "id": "obj-2", "maxclass": "newobj", "text": "t b f", "numinlets": 1, "numoutlets": 2, "outlettype": ["bang", "float"] } }
        ], "lines": [
            { "patchline": { "destination": ["obj-2", 0], "midpoints": [20, 40, 60, 40], "order": 1, "source": ["obj-1", 0] } }
        ], "dependency_cache": [] } });
        let patcher = Patcher::read(&document, &NoFiles);
        assert_eq!(patcher.boxes.len(), 2);
        assert_eq!(patcher.boxes[0].patcher.as_ref().map(|inner| inner.boxes.len()), Some(1));
        assert_eq!(patcher.cords[0].midpoints, vec![[20., 40.], [60., 40.]]);
        assert_eq!(patcher.boxes[1].class(), "t");
        assert_eq!(patcher.boxes[1].args(), ["b", "f"]);
        assert_eq!(patcher.boxes[1].label(), "[t b f]");
        assert_eq!(patcher.to_document(), document, "keys stay in Max's order");
    }

    #[test]
    fn a_bpatcher_or_an_abstraction_is_read_from_the_file_it_names_once_per_nesting() {
        let dial = json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "jsui" } }], "lines": [] } });
        let looped = json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "newobj", "text": "loop" } }], "lines": [] } });
        let files = Library(HashMap::from([("dial.maxpat".to_string(), dial), ("loop.maxpat".to_string(), looped)]));
        let device = json!({ "patcher": { "boxes": [
            { "box": { "id": "obj-1", "maxclass": "bpatcher", "name": "dial.maxpat" } },
            { "box": { "id": "obj-2", "maxclass": "newobj", "text": "loop" } },
            { "box": { "id": "obj-3", "maxclass": "newobj", "text": "prepend set" } }
        ], "lines": [] } });
        let patcher = Patcher::read(&device, &files);
        assert_eq!(patcher.boxes[0].inner().map(|inner| inner.boxes[0].maxclass()), Some("jsui"));
        let inner = patcher.boxes[1].inner().expect("the abstraction");
        assert!(inner.boxes[0].file.is_none(), "an abstraction inside itself isn't read again");
        assert!(patcher.boxes[2].inner().is_none());
        assert_eq!(patcher.to_document(), device, "files read aren't written into the boxes");
    }
}
