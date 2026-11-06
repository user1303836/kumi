//! A Max patcher as Kumi reads, checks and lays it out: its boxes in order (Max draws a box above the boxes listed
//! after it in its layer), its cords, and every other field as Max wrote it, so a patcher read and written back is the
//! same JSON. A box's own patcher (a [p], an embedded bpatcher, a gen~) is read with it, and the file an abstraction, a
//! bpatcher or a code box names is read through [`Files`] (a frozen device's own files, or none).
//!
//! The standard Kumi holds patchers to is data in `daws/ableton/max-standard/`: each rule's level, what
//! the rules know of Max's objects, and what measuring well-made devices found. The model never reads it: it hears a
//! broken rule in one line, with the fix.

pub mod catalog;
pub mod check;
pub mod code;
pub mod frozen;
pub mod geometry;
pub mod layout;
pub mod measure;
pub mod message;
pub mod notation;
pub mod page;
pub mod reference;
pub mod standard;

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use code::{Code, Own, Script, MOST_ASKED};
use geometry::Rect;

/// The files a patcher's abstractions, bpatchers and code boxes name.
pub trait Files {
    /// Where the file `name` lies and how big it is, without reading it; None when there's no file of that name.
    fn place(&self, name: &str) -> Option<Place>;

    /// The JSON of the patcher file `name` (`dial.maxpat`), if there is one.
    fn patcher(&self, name: &str) -> Option<Value>;

    /// The text of the file `name` (`dial.js`), if there is one.
    fn text(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Where a file lies: what tells it from every other file, and how many bytes reading it takes. Names that reach one
/// place reach one file, as a frozen device can list a file under any number of names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    pub at: String,
    pub bytes: usize,
}

/// No files: only what's embedded is read.
pub struct NoFiles;

impl Files for NoFiles {
    fn place(&self, _name: &str) -> Option<Place> {
        None
    }

    fn patcher(&self, _name: &str) -> Option<Value> {
        None
    }
}

/// How deep files are read inside each other: an abstraction that holds itself stops here.
const MAX_DEPTH: usize = 16;
/// The most boxes read from files for one patcher, however its abstractions nest: each box that uses an abstraction
/// counts its boxes again, as what walks the patcher meets them again, so a few abstractions each holding a few boxes of
/// the next can't make that millions.
const MOST_FILE_BOXES: usize = 20_000;
/// The most bytes read from files for one patcher, each file once: a patcher's JSON takes many times its size to read.
const MOST_FILE_BYTES: usize = 64 * 1024 * 1024;
/// The most names of files looked up for one patcher, each once, found or not: every object's name is one an
/// abstraction could have.
const MOST_NAMES: usize = 20_000;

/// What reading one patcher's files has used, and what it read. Each file is read once, by its place, however many names
/// reach it and however many boxes use it, and what a device can make Kumi look up, read or walk is bounded, whatever it
/// holds.
struct Reading {
    /// The places of the files being read, innermost last.
    open: Vec<String>,
    /// How many more boxes may be read from files: each use of a file counts its boxes, at any depth.
    left: usize,
    /// How many more bytes may be read from files.
    bytes_left: usize,
    /// How many more names may be looked up.
    names_left: usize,
    /// What questions of the code read may still read, shared by all of it.
    asked: Arc<AtomicUsize>,
    /// Each name looked up, and the place it reaches (None for none).
    places: HashMap<String, Option<Place>>,
    /// Each patcher file read, by its place: its patcher and how many boxes it holds with the files it uses (None for one
    /// that isn't a patcher, or that there were no bytes left to read).
    patchers: HashMap<String, Option<(Arc<Patcher>, usize)>>,
    /// Each script read, by its place (None when there were no bytes left to read it).
    scripts: HashMap<String, Option<Arc<Script>>>,
    /// The code of each script a box names as its own, by its place: every box that runs it shares it.
    codes: HashMap<String, Arc<Code>>,
}

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
    /// The patcher embedded in the box: a [p], a bpatcher with its patcher embedded, a gen~, an mc.gen~'s (in its data).
    pub patcher: Option<Patcher>,
    /// The file the box loads (an abstraction, a bpatcher, a poly~'s voice), read through [`Files`]: its name and
    /// patcher, shared by every box that loads it. It's never written back into the box.
    pub file: Option<(String, Arc<Patcher>)>,
    /// The JavaScript the box runs (a [v8ui]'s file, a [v8.codebox]'s own), read to know what a message makes it do;
    /// a script file's is shared by every box that runs it.
    pub code: Option<Arc<Code>>,
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
        Patcher::read_within(value.clone(), files, &mut Reading::bounded())
    }

    /// A patcher with every file it names read, however many: for surveying patchers Kumi trusts (Max's own), never for a
    /// device it's given.
    pub fn read_all(value: &Value, files: &dyn Files) -> Patcher {
        Patcher::read_within(value.clone(), files, &mut Reading::new(usize::MAX, usize::MAX, usize::MAX, usize::MAX))
    }

    /// A patcher read from its JSON, taken apart as it's read: what it holds, at any depth, is moved, never copied.
    fn read_within(mut value: Value, files: &dyn Files, reading: &mut Reading) -> Patcher {
        let inner = value.get_mut("patcher").filter(|inner| inner.is_object()).map(Value::take);
        let mut fields = match inner.unwrap_or(value) {
            Value::Object(fields) => fields,
            _ => Map::new(),
        };
        let boxes = match fields.get_mut("boxes").map(Value::take) {
            Some(Value::Array(boxes)) => boxes,
            _ => Vec::new(),
        };
        let lines = match fields.get_mut("lines").map(Value::take) {
            Some(Value::Array(lines)) => lines,
            _ => Vec::new(),
        };
        let boxes = boxes.into_iter().filter_map(|entry| MaxBox::read(entry, files, reading)).collect();
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

impl Reading {
    /// What reading a device's patcher, or one box's code, may use.
    fn bounded() -> Reading {
        Reading::new(MOST_FILE_BOXES, MOST_FILE_BYTES, MOST_NAMES, MOST_ASKED)
    }

    fn new(boxes: usize, bytes: usize, names: usize, asked: usize) -> Reading {
        Reading {
            open: Vec::new(),
            left: boxes,
            bytes_left: bytes,
            names_left: names,
            asked: Arc::new(AtomicUsize::new(asked)),
            places: HashMap::new(),
            patchers: HashMap::new(),
            scripts: HashMap::new(),
            codes: HashMap::new(),
        }
    }

    /// Where the file `name` lies, looked up once; past the names that may be looked up, nowhere.
    fn place(&mut self, files: &dyn Files, name: &str) -> Option<Place> {
        if let Some(known) = self.places.get(name) {
            return known.clone();
        }
        if self.names_left == 0 {
            return None;
        }
        self.names_left -= 1;
        let place = files.place(name);
        self.places.insert(name.to_string(), place.clone());
        place
    }

    /// The patcher file `name` names, at `place`: parsed once for its place, its bytes counted before it's parsed; each
    /// use counts its boxes, at any depth, with the files it uses.
    fn patcher(&mut self, files: &dyn Files, name: String, place: Place) -> Option<(String, Arc<Patcher>)> {
        match self.patchers.get(&place.at).cloned() {
            Some(Some((read, size))) if size <= self.left => {
                self.left -= size;
                Some((name, read))
            }
            Some(Some(_)) => {
                self.left = 0;
                None
            }
            Some(None) => None,
            // A file that holds itself stops there.
            None if self.open.contains(&place.at) => None,
            None if place.bytes > self.bytes_left => {
                self.patchers.insert(place.at, None);
                None
            }
            None => {
                self.bytes_left -= place.bytes;
                let Some(value) = files.patcher(&name) else {
                    self.patchers.insert(place.at, None);
                    return None;
                };
                // Its boxes, at any depth, count against what's left to read from files, and the file one more; then the
                // files it uses, as they're read.
                let count = boxes_in(&value) + 1;
                if count > self.left {
                    self.left = 0;
                    return None;
                }
                let before = self.left;
                self.left -= count;
                self.open.push(place.at.clone());
                let read = Arc::new(Patcher::read_within(value, files, self));
                self.open.pop();
                self.patchers.insert(place.at, Some((Arc::clone(&read), before - self.left)));
                Some((name, read))
            }
        }
    }

    /// The script `name` names, with or without `.js`, and its place: read once for its place, its bytes counted before
    /// it's read.
    fn script(&mut self, files: &dyn Files, name: &str) -> Option<(String, Arc<Script>)> {
        if name.is_empty() || name == "none" {
            return None;
        }
        let (named, place) = match self.place(files, name) {
            Some(place) => (name.to_string(), place),
            None if !name.contains('.') => {
                let named = format!("{name}.js");
                let place = self.place(files, &named)?;
                (named, place)
            }
            None => return None,
        };
        let script = match self.scripts.get(&place.at).cloned() {
            Some(known) => known,
            None => {
                let script = if place.bytes <= self.bytes_left {
                    self.bytes_left -= place.bytes;
                    files.text(&named).map(|text| Arc::new(Script::read(&text)))
                } else {
                    None
                };
                self.scripts.insert(place.at.clone(), script.clone());
                script
            }
        };
        script.map(|script| (place.at, script))
    }

    /// The code a box runs: each script read once, and the code of one a box names as its own shared by every box that
    /// names it.
    fn code(&mut self, files: &dyn Files, item: &MaxBox) -> Option<Arc<Code>> {
        let left = Arc::clone(&self.asked);
        match Code::own(item)? {
            Own::Held(text) => {
                Some(Arc::new(Code::of(Arc::new(Script::read(text)), &mut |name| self.script(files, name).map(|(_, used)| used), left)))
            }
            Own::File(name) => {
                let (at, own) = self.script(files, name)?;
                if let Some(code) = self.codes.get(&at) {
                    return Some(Arc::clone(code));
                }
                let code = Arc::new(Code::of(own, &mut |name| self.script(files, name).map(|(_, used)| used), left));
                self.codes.insert(at, Arc::clone(&code));
                Some(code)
            }
        }
    }
}

impl MaxBox {
    fn read(entry: Value, files: &dyn Files, reading: &mut Reading) -> Option<MaxBox> {
        let mut fields = match entry {
            Value::Object(mut entry) => match entry.remove("box") {
                Some(Value::Object(fields)) => fields,
                _ => return None,
            },
            _ => return None,
        };
        // An mc.gen~'s wrapper keeps its patcher in its data, which keeps its place as a null too.
        let patcher =
            embedded_patcher(&mut fields).filter(|inner| inner.is_object()).map(|inner| Patcher::read_within(inner.take(), files, reading));
        let mut item = MaxBox { fields, patcher, file: None, code: None };
        item.code = reading.code(files, &item);
        if item.patcher.is_none() && reading.open.len() < MAX_DEPTH && reading.left > 0 {
            if let Some(name) = item.file_name() {
                if let Some(place) = reading.place(files, &name) {
                    item.file = reading.patcher(files, name, place);
                }
            }
        }
        Some(item)
    }

    /// The file the box would load: a bpatcher's, a poly~'s voice, a gen's named code, or an abstraction named like it.
    fn file_name(&self) -> Option<String> {
        match self.maxclass() {
            "bpatcher" => self.fields.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_string),
            "newobj" => {
                let words = self.words();
                match words.first().copied()? {
                    "poly~" | "mc.poly~" => words.get(1).map(|name| with_extension(name, ".maxpat")),
                    "gen~" | "gen" | "mc.gen~" | "mcs.gen~" => words
                        .iter()
                        .position(|word| matches!(*word, "@gen" | "@file"))
                        .and_then(|at| words.get(at + 1))
                        .or_else(|| words.get(1).filter(|word| !word.starts_with('@')))
                        .filter(|name| !name.starts_with('@'))
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
                match fields.get_mut("data").and_then(Value::as_object_mut).filter(|data| data.get("patcher") == Some(&Value::Null)) {
                    Some(data) => data.insert("patcher".into(), patcher.to_value()),
                    None => fields.insert("patcher".into(), patcher.to_value()),
                };
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
        self.patcher.as_ref().or(self.file.as_ref().map(|(_, patcher)| patcher.as_ref()))
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

/// How many boxes a patcher's JSON holds, its subpatchers' too (an mc.gen~'s in its data).
fn boxes_in(value: &Value) -> usize {
    let patcher = value.get("patcher").filter(|inner| inner.is_object()).unwrap_or(value);
    let Some(boxes) = patcher.get("boxes").and_then(Value::as_array) else { return 0 };
    boxes
        .iter()
        .map(|entry| {
            let item = &entry["box"];
            1 + boxes_in(if item.as_object().is_some_and(keeps_patcher_in_data) { &item["data"]["patcher"] } else { &item["patcher"] })
        })
        .sum()
}

/// Whether a box, as Max's JSON, keeps the patcher embedded in it in its data, as an mc.gen~'s wrapper does, not in a
/// field of its own as every other box does. Another object's data (a dict's, say) is its own.
pub(crate) fn keeps_patcher_in_data(fields: &Map<String, Value>) -> bool {
    fields.get("text").and_then(Value::as_str).and_then(|text| text.split_whitespace().next()) == Some("mc.gen~")
}

/// Where a box, as Max's JSON, keeps the patcher embedded in it: an mc.gen~'s wrapper in its data, every other box in a
/// field of its own.
pub(crate) fn embedded_patcher(fields: &mut Map<String, Value>) -> Option<&mut Value> {
    if keeps_patcher_in_data(fields) {
        fields.get_mut("data").and_then(|data| data.get_mut("patcher"))
    } else {
        fields.get_mut("patcher")
    }
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
        fn place(&self, name: &str) -> Option<Place> {
            self.0.get(name).map(|value| Place { at: name.to_string(), bytes: value.to_string().len() })
        }

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

    /// Files that count each time they're read, and each time one is looked up.
    struct Counted {
        files: HashMap<String, Value>,
        scripts: HashMap<String, String>,
        reads: std::cell::Cell<usize>,
        looked: std::cell::Cell<usize>,
    }

    impl Counted {
        fn new(files: HashMap<String, Value>, scripts: HashMap<String, String>) -> Counted {
            Counted { files, scripts, reads: std::cell::Cell::new(0), looked: std::cell::Cell::new(0) }
        }
    }

    impl Files for Counted {
        fn place(&self, name: &str) -> Option<Place> {
            self.looked.set(self.looked.get() + 1);
            let bytes = self.files.get(name).map(|value| value.to_string().len()).or_else(|| self.scripts.get(name).map(String::len))?;
            Some(Place { at: name.to_string(), bytes })
        }

        fn patcher(&self, name: &str) -> Option<Value> {
            self.reads.set(self.reads.get() + 1);
            self.files.get(name).cloned()
        }

        fn text(&self, name: &str) -> Option<String> {
            self.reads.set(self.reads.get() + 1);
            self.scripts.get(name).cloned()
        }
    }

    #[test]
    fn a_big_file_ten_thousand_boxes_use_is_read_once_and_shared() {
        // 10,000 [big] boxes and 10,000 [js big.js] boxes, beside a frozen-in big.maxpat of one 20 MB box and a 20 MB
        // big.js: read once each, not 10,000 times (200 GB).
        let huge = "x".repeat(20 * 1024 * 1024);
        let big = json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "comment", "text": huge } }], "lines": [] } });
        let files = Counted::new(
            HashMap::from([("big.maxpat".to_string(), big)]),
            HashMap::from([("big.js".to_string(), format!("function bang() {{ outlet(0, 1); }}\n// {huge}"))]),
        );
        let mut boxes: Vec<Value> =
            (0..10_000).map(|n| json!({ "box": { "id": format!("obj-{n}"), "maxclass": "newobj", "text": "big" } })).collect();
        boxes.extend((0..10_000).map(|n| json!({ "box": { "id": format!("obj-js-{n}"), "maxclass": "newobj", "text": "js big.js" } })));
        let device = json!({ "patcher": { "boxes": boxes, "lines": [] } });
        let patcher = Patcher::read(&device, &files);
        assert!(files.reads.get() <= 3, "{} reads", files.reads.get());
        assert!(patcher.boxes.iter().take(10_000).all(|item| item.inner().is_some_and(|inner| inner.boxes.len() == 1)));
        let first = patcher.boxes[0].file.as_ref().map(|(_, read)| Arc::as_ptr(read));
        assert!(patcher.boxes.iter().take(10_000).all(|item| item.file.as_ref().map(|(_, read)| Arc::as_ptr(read)) == first), "shared");
        // A script that big isn't read for its functions: what it does is unknown, and every box shares that.
        let code = patcher.boxes[10_000].code.clone().expect("its code");
        assert!(code.partial);
        assert!(patcher.boxes[10_000..].iter().all(|item| item.code.as_ref().is_some_and(|other| Arc::ptr_eq(other, &code))));
    }

    #[test]
    fn abstractions_nested_in_threes_are_read_up_to_a_bound_not_millions_of_times() {
        // Sixteen abstractions, each holding three boxes of the next: 3^16 reads, were each read every time it's used.
        let mut library = HashMap::new();
        for level in 0..16 {
            let next = format!("level{}", level + 1);
            let boxes: Vec<Value> =
                (1..=3).map(|n| json!({ "box": { "id": format!("obj-{n}"), "maxclass": "newobj", "text": next } })).collect();
            library.insert(format!("level{level}.maxpat"), json!({ "patcher": { "boxes": boxes, "lines": [] } }));
        }
        let device = json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "newobj", "text": "level0" } }], "lines": [] } });
        let patcher = Patcher::read(&device, &Library(library));
        fn boxes(patcher: &Patcher) -> usize {
            patcher.boxes.iter().map(|item| 1 + item.inner().map_or(0, boxes)).sum()
        }
        let read = boxes(&patcher);
        assert!(read <= MOST_FILE_BOXES + 1, "{read} boxes read");
        assert!(patcher.boxes[0].inner().is_some(), "the first files are read");
    }

    #[test]
    fn an_mc_gens_patcher_in_a_file_counts_toward_the_bound_as_any_embedded_patcher_does() {
        // A file of one mc.gen~ whose wrapper's data holds 100 boxes, used by 1,000 boxes: 101,000 boxes to walk, were
        // the file counted as one box.
        let gen: Vec<Value> =
            (1..=100).map(|n| json!({ "box": { "id": format!("obj-{n}"), "maxclass": "comment", "text": "x" } })).collect();
        let wrapped = json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "newobj", "text": "mc.gen~ @chans 2",
            "data": { "patcher": { "classnamespace": "dsp.gen", "boxes": gen, "lines": [] } } } }], "lines": [] } });
        let users: Vec<Value> =
            (1..=1_000).map(|n| json!({ "box": { "id": format!("obj-{n}"), "maxclass": "newobj", "text": "wrapped" } })).collect();
        let device = json!({ "patcher": { "boxes": users, "lines": [] } });
        let patcher = Patcher::read(&device, &Library(HashMap::from([("wrapped.maxpat".to_string(), wrapped)])));
        fn boxes(patcher: &Patcher) -> usize {
            patcher.boxes.iter().map(|item| 1 + item.inner().map_or(0, boxes)).sum()
        }
        let gen = patcher.boxes[0].inner().and_then(|file| file.boxes[0].inner()).map(|gen| gen.boxes.len());
        assert_eq!(gen, Some(100), "the wrapper's patcher is read");
        let read = boxes(&patcher);
        assert!(read <= MOST_FILE_BOXES + 1_000, "{read} boxes read");
    }

    /// One patcher file that every name reaches, as a frozen device can list one file under any number of names, saying
    /// it holds `bytes`; counting each time it's read.
    struct OnePlace {
        patcher: Value,
        bytes: usize,
        reads: std::cell::Cell<usize>,
    }

    impl Files for OnePlace {
        fn place(&self, _name: &str) -> Option<Place> {
            Some(Place { at: "16+64".into(), bytes: self.bytes })
        }

        fn patcher(&self, _name: &str) -> Option<Value> {
            self.reads.set(self.reads.get() + 1);
            Some(self.patcher.clone())
        }
    }

    #[test]
    fn a_file_any_number_of_names_reach_is_read_once_and_what_files_are_read_is_bounded_by_their_bytes() {
        // 2,000 boxes, each naming the one file by a name of its own: it's read once, for its place.
        let file = json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "comment", "text": "x" } }], "lines": [] } });
        let boxes: Vec<Value> =
            (0..2_000).map(|n| json!({ "box": { "id": format!("obj-{n}"), "maxclass": "newobj", "text": format!("alias{n}") } })).collect();
        let device = json!({ "patcher": { "boxes": boxes, "lines": [] } });
        let one = OnePlace { patcher: file.clone(), bytes: 64, reads: std::cell::Cell::new(0) };
        let patcher = Patcher::read(&device, &one);
        assert_eq!(one.reads.get(), 1);
        let first = patcher.boxes[0].file.as_ref().map(|(_, read)| Arc::as_ptr(read));
        assert!(first.is_some() && patcher.boxes.iter().all(|item| item.file.as_ref().map(|(_, read)| Arc::as_ptr(read)) == first));
        // A file bigger than what's left to read isn't read at all, whatever its JSON holds.
        let big = OnePlace { patcher: file, bytes: MOST_FILE_BYTES + 1, reads: std::cell::Cell::new(0) };
        let patcher = Patcher::read(&device, &big);
        assert_eq!(big.reads.get(), 0);
        assert!(patcher.boxes.iter().all(|item| item.file.is_none()));
    }

    #[test]
    fn every_name_looked_up_counts_found_or_not_and_names_are_looked_up_to_a_bound() {
        // Every object's name is one an abstraction could have: 30,000 boxes, each of an object named its own way.
        let boxes: Vec<Value> = (0..30_000)
            .map(|n| json!({ "box": { "id": format!("obj-{n}"), "maxclass": "newobj", "text": format!("thing{n}") } }))
            .collect();
        let files = Counted::new(HashMap::new(), HashMap::new());
        Patcher::read(&json!({ "patcher": { "boxes": boxes, "lines": [] } }), &files);
        assert_eq!(files.looked.get(), MOST_NAMES);
    }

    #[test]
    fn a_script_and_what_it_includes_are_read_once_however_many_boxes_run_them() {
        // 1,000 [v8ui] boxes whose embed flag has no text with it, so they run the file they name; and 1,000 [js] boxes,
        // each running a script of its own that includes one big script.
        let mut scripts =
            HashMap::from([("big.js".to_string(), format!("function shared() {{ outlet(0, 1); }}\n// {}", "x".repeat(100_000)))]);
        let mut boxes = Vec::new();
        for n in 0..1_000 {
            boxes.push(
                json!({ "box": { "id": format!("obj-ui-{n}"), "maxclass": "v8ui", "filename": "big.js", "textfile": { "embed": 1 } } }),
            );
            scripts.insert(format!("own{n}.js"), "include('big.js');\nfunction bang() { shared(); }".to_string());
            boxes.push(json!({ "box": { "id": format!("obj-js-{n}"), "maxclass": "newobj", "text": format!("js own{n}.js") } }));
        }
        let files = Counted::new(HashMap::new(), scripts);
        let patcher = Patcher::read(&json!({ "patcher": { "boxes": boxes, "lines": [] } }), &files);
        assert_eq!(files.reads.get(), 1_001, "big.js once, and each box's own script once");
        let runs = |maxclass: &str, selector: &str| {
            patcher
                .boxes
                .iter()
                .filter(|item| item.maxclass() == maxclass)
                .all(|item| item.code.as_ref().is_some_and(|code| code.sends_at_once(selector) == Some(true)))
        };
        assert!(runs("v8ui", "shared") && runs("newobj", "bang"), "each box's code reaches what the big script defines");
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

    /// #310, #821: gen's forms load named code, and none is an abstraction named like the form.
    #[test]
    fn gens_multichannel_forms_load_the_file_their_at_gen_names() {
        let file = |text: &str| json!({ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "comment", "text": text } }], "lines": [] } });
        let files = Library(HashMap::from([
            ("shared.gendsp".to_string(), file("the code")),
            ("mc.gen~.maxpat".to_string(), file("not this")),
            ("mcs.gen~.maxpat".to_string(), file("not this")),
        ]));
        let device = json!({ "patcher": { "boxes": [
            { "box": { "id": "obj-1", "maxclass": "newobj", "text": "mc.gen~ @gen shared @chans 4" } },
            { "box": { "id": "obj-2", "maxclass": "newobj", "text": "mcs.gen~ @gen shared" } },
            { "box": { "id": "obj-3", "maxclass": "newobj", "text": "mc.gen~ @chans 4" } },
            { "box": { "id": "obj-4", "maxclass": "newobj", "text": "mcs.gen~" } },
            { "box": { "id": "obj-5", "maxclass": "newobj", "text": "mc.gen~ shared @chans 16" } },
            { "box": { "id": "obj-6", "maxclass": "newobj", "text": "mcs.gen~ shared" } },
            { "box": { "id": "obj-7", "maxclass": "newobj", "text": "gen @file shared" } },
            { "box": { "id": "obj-8", "maxclass": "newobj", "text": "gen~ @gen @chans 4" } }
        ], "lines": [] } });
        let patcher = Patcher::read(&device, &files);
        let loads = |at: usize| patcher.boxes[at].file.as_ref().map(|(name, _)| name.as_str());
        assert_eq!([loads(0), loads(1), loads(2), loads(3)], [Some("shared.gendsp"), Some("shared.gendsp"), None, None]);
        assert_eq!([loads(4), loads(5), loads(6), loads(7)], [Some("shared.gendsp"), Some("shared.gendsp"), Some("shared.gendsp"), None]);
    }
}
