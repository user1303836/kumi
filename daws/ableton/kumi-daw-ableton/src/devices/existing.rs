//! A device that's already there, made into a new one (#353): Kumi opens a Max for Live device file (its own or anyone
//! else's, frozen or not), says what's in it in the patch notation, and makes a new device from it with only what's
//! asked changed. Every box, cord and face element a change doesn't touch stays as Max wrote it.
//!
//! The original's file is never written: a saved Set refers to a Max for Live device by its file, so changing the file
//! would change every Set that uses it. The new device is saved as a device of its own, which records the file it came
//! from and that file's hash.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use kumi_common::js::{json::stringify_with_indent, number::to_string};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::amxd::{encrypted, with_patcher_chunk, DeviceType};
use super::findings::Findings;
use super::look;
use super::patch::check::{check_with, UNKNOWN_OBJECT};
use super::patch::frozen::{read_device, Frozen};
use super::patch::geometry::Rect;
use super::patch::layout::{arrange, text_width};
use super::patch::notation::{expand, parse_beside, ports_had, Outside, OUTSIDE};
use super::patch::reference::Reference;
use super::patch::standard::{standard, Level};
use super::patch::{Cord, Files, MaxBox, NoFiles, Patcher};
use super::spec::Control;
use super::tool::kind_folder;

/// The most a device file Kumi opens can weigh: one with samples frozen in is big, and it's read whole.
const MOST_BYTES: u64 = 256 * 1024 * 1024;
/// How many files and folders of the User Library are looked through for a device by its name.
const MOST_LOOKED: usize = 50_000;
/// The most of a device's patch the answer that opens it says, in characters.
const MOST_SAID: usize = 60_000;
/// The scripting name of the comment that records what a device was made from.
pub const FROM_NAME: &str = "kumi-from";
/// What a patch says of the boxes Kumi rewrites a field of when one is given again (its object, its code, its ports);
/// the rest of its fields (where it sits, its look, its scripting name) stay as they were.
const WHAT_IT_IS: [&str; 10] =
    ["maxclass", "text", "numinlets", "numoutlets", "outlettype", "patcher", "textfile", "filename", "code", "saved_object_attributes"];
/// What a change's checks report: what's in the patch (its objects and cords, their order, the names it shares, its
/// timing, its parameters, its gen~ code and its face). Where its boxes sit is the original's own style.
const REPORTED: [&str; 8] = ["patch.", "order.", "names.", "timing.", "map.", "params.", "gen.", "face."];
/// Marks a box id that's already the device's own (an existing box, a control) while a change's patch is expanded.
const OWN: char = '\u{1}';

/// The device file `from` names: a path to one, its item in Live's Browser (`user_library/…`), or the name of one in
/// the User Library, as Live names a Max for Live device by its file (Kumi's own folders first).
pub fn find(from: &str, user_library: &Path) -> Result<PathBuf, String> {
    let from = from.trim().trim_matches('"').trim();
    if from.is_empty() {
        return Err("from: give the device's file, its item in Live's Browser (user_library/…) or its name.".into());
    }
    if let Some(relative) = from.strip_prefix("user_library/") {
        // A Browser id for a second item at one path ends "#2" (#183): the file is the path's.
        let relative = relative
            .rsplit_once('#')
            .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map_or(relative, |(path, _)| path);
        // Only a path inside the library: no part that climbs out, and none Windows reads as more than one part or as a
        // drive.
        if relative.split('/').any(|part| part.is_empty() || part == ".." || part.contains(['\\', ':'])) {
            return Err(format!("from {from}: that isn't an item in the User Library."));
        }
        let mut parts: Vec<String> = relative.split('/').map(str::to_string).collect();
        if let Some(last) = parts.last_mut().filter(|last| !last.to_lowercase().ends_with(".amxd")) {
            last.push_str(".amxd");
        }
        let file = parts.iter().fold(user_library.to_path_buf(), |path, part| path.join(part));
        return if file.is_file() { Ok(file) } else { Err(format!("from {from}: the User Library has no device there.")) };
    }
    let path = Path::new(from);
    if path.is_absolute() || from.contains(['/', '\\']) {
        return if path.is_file() { Ok(path.to_path_buf()) } else { Err(format!("from: there's no device file at {from}.")) };
    }
    let wanted = format!("{}.amxd", from.strip_suffix(".amxd").unwrap_or(from));
    let ours: Vec<PathBuf> = DeviceType::ALL
        .into_iter()
        .map(|kind| user_library.join("Kumi").join(kind_folder(kind)).join(&wanted))
        .filter(|file| file.is_file())
        .collect();
    let found = if ours.is_empty() { look_for(user_library, &wanted) } else { ours };
    match found.len() {
        0 => Err(format!(
            "from {from}: the User Library has no device of that name. Give its file's path (a device from a Pack or elsewhere), or its item in Live's Browser."
        )),
        1 => Ok(found.into_iter().next().unwrap_or_default()),
        _ => Err(format!(
            "from {from}: the User Library has {} devices of that name; give the one meant by its path: {}.",
            found.len(),
            found.iter().take(5).map(|file| file.to_string_lossy().into_owned()).collect::<Vec<_>>().join("; ")
        )),
    }
}

/// The files named `wanted` (in any case) in the folder and the folders in it, up to [`MOST_LOOKED`] looked at.
fn look_for(folder: &Path, wanted: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    let mut looked = 0;
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            looked += 1;
            if looked > MOST_LOOKED {
                return found;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && !name.starts_with('.') => folders.push(entry.path()),
                Ok(kind) if kind.is_file() && name.eq_ignore_ascii_case(wanted) => found.push(entry.path()),
                _ => {}
            }
        }
    }
    found.sort();
    found
}

/// A device file opened to be made into a new one: what it is, as it lies on disk.
#[derive(Debug, Clone)]
pub struct Original {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub kind: DeviceType,
    /// Its name: its file's, as Live names it.
    pub name: String,
    /// Its bytes' SHA-256, in hex.
    pub hash: String,
    /// Its patcher, `{ "patcher": … }`.
    pub document: Value,
    /// The files frozen into it, when it's frozen.
    pub frozen: Option<Frozen>,
}

/// The device file at `path`, opened, or why it can't be.
pub fn open(path: &Path) -> Result<Original, String> {
    let shown = path.to_string_lossy();
    let size = std::fs::metadata(path).map_err(|error| format!("Kumi couldn't read {shown}: {error}."))?.len();
    if size > MOST_BYTES {
        return Err(format!("{shown} is {} MB, past the {} MB Kumi opens.", size / 1_048_576, MOST_BYTES / 1_048_576));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("Kumi couldn't read {shown}: {error}."))?;
    if encrypted(&bytes) {
        return Err(format!(
            "{shown} is one of Live's own Max for Live devices, whose patch is encrypted: only Live can read it, so Kumi can't make a device from it. Make a new one that does what's wanted instead."
        ));
    }
    let device = read_device(&bytes).ok_or_else(|| format!("{shown} isn't a Max for Live device file Kumi can read."))?;
    let kind = DeviceType::ALL.into_iter().find(|kind| kind.code() == device.code).ok_or_else(|| {
        format!(
            "{shown} is a kind of Max for Live device Kumi doesn't make ({}): only MIDI effects, audio effects and instruments.",
            device.code
        )
    })?;
    let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let hash = hex::encode(Sha256::digest(&bytes));
    Ok(Original { path: path.to_path_buf(), bytes, kind, name, hash, document: device.patcher, frozen: device.frozen })
}

impl Original {
    fn files(&self) -> &dyn Files {
        match &self.frozen {
            Some(frozen) => frozen,
            None => &NoFiles,
        }
    }

    /// Its top-level patcher, with the files its boxes name read from what's frozen into it.
    pub fn patcher(&self) -> Patcher {
        Patcher::read(&self.document, self.files())
    }
}

/// What a change calls each of a device's boxes: the names its patch uses for them.
#[derive(Debug, Default, Clone)]
pub struct Names {
    /// Each box's name, by its id ("in" and "out" for the device's way in and out).
    pub by_id: HashMap<String, String>,
    /// Each box's id, by its name (not in or out, which are the notation's own).
    pub by_name: HashMap<String, String>,
    /// The device's way in and its way out, by id, when it has one of each.
    pub way_in: Option<String>,
    pub way_out: Option<String>,
    /// Its controls (Live parameters on its face), their ids by their names.
    pub controls: BTreeMap<String, String>,
}

/// A name the notation reads as one: a letter or _, then letters, digits and _.
fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Text as a name's start: lower-case letters, digits and single underscores ("live.dial" is "live_dial").
fn name_part(text: &str) -> String {
    let mut part = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            part.push(c.to_ascii_lowercase());
        } else if !part.ends_with('_') && !part.is_empty() {
            part.push('_');
        }
    }
    let part = part.trim_end_matches('_').to_string();
    if part.is_empty() || part.starts_with(|c: char| c.is_ascii_digit()) {
        format!("box_{part}").trim_end_matches('_').to_string()
    } else {
        part
    }
}

/// A box's Live parameter name, when it's a control on the face: a live.* object with a parameter.
fn parameter(item: &MaxBox) -> Option<&str> {
    if !item.maxclass().starts_with("live.") || item.fields.get("parameter_enable").and_then(Value::as_f64) == Some(0.0) {
        return None;
    }
    item.valueof()?.get("parameter_longname")?.as_str().filter(|name| !name.is_empty())
}

/// The names a patcher's boxes go by in the notation: each its scripting name where that's a name the notation reads,
/// else its object's and its number ("metro_12"). Names are given in the boxes' order, so the same file always gives
/// the same names.
fn local_names(patcher: &Patcher, taken: &mut HashSet<String>, skip: &HashSet<&str>) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for (at, item) in patcher.boxes.iter().enumerate() {
        if skip.contains(item.id()) {
            continue;
        }
        let varname = item.str("varname");
        let name = if is_name(varname) && !OUTSIDE.contains(&varname) && !taken.contains(varname) {
            varname.to_string()
        } else {
            let base = name_part(if item.class().is_empty() { "box" } else { item.class() });
            let number = item
                .id()
                .strip_prefix("obj-")
                .filter(|n| n.chars().all(|c| c.is_ascii_digit()))
                .map_or(at + 1, |n| n.parse().unwrap_or(at + 1));
            let mut name = format!("{base}_{number}");
            let mut again = 2;
            while taken.contains(&name) {
                name = format!("{base}_{number}_{again}");
                again += 1;
            }
            name
        };
        taken.insert(name.clone());
        names.insert(item.id().to_string(), name);
    }
    names
}

/// The one box at the patcher's top whose class is one of `classes`, if there's exactly one.
fn only(patcher: &Patcher, classes: &[&str]) -> Option<String> {
    let mut found = patcher.boxes.iter().filter(|item| classes.contains(&item.class()));
    let first = found.next()?;
    found.next().is_none().then(|| first.id().to_string())
}

/// What a change calls each of the device's boxes, its way in and out, and its controls.
pub fn names(patcher: &Patcher, kind: DeviceType) -> Names {
    // Kumi's own sound devices go out through its output stage, a gen~ it names obj-out; any other device, through the
    // one object that sends its kind out.
    let way_in = match kind {
        DeviceType::AudioEffect => only(patcher, &["plugin~", "mc.plugin~"]),
        _ => only(patcher, &["midiin"]),
    };
    let way_out = match kind {
        DeviceType::MidiEffect => only(patcher, &["midiout"]),
        _ => patcher
            .find("obj-out")
            .filter(|item| item.class() == "gen~")
            .map(|item| item.id().to_string())
            .or_else(|| only(patcher, &["plugout~", "mc.plugout~"])),
    };
    let mut controls = BTreeMap::new();
    for item in &patcher.boxes {
        if let Some(name) = parameter(item) {
            controls.entry(name.to_string()).or_insert_with(|| item.id().to_string());
        }
    }
    let ways: HashSet<&str> = way_in.iter().chain(&way_out).map(String::as_str).collect();
    let mut by_id = local_names(patcher, &mut HashSet::new(), &ways);
    let by_name = by_id.iter().map(|(id, name)| (name.clone(), id.clone())).collect();
    if let Some(id) = &way_in {
        by_id.insert(id.clone(), "in".into());
    }
    if let Some(id) = &way_out {
        by_id.insert(id.clone(), "out".into());
    }
    Names { by_id, by_name, way_in, way_out, controls }
}

/// A gen patcher's GenExpr, when the patcher is one codebox between its ins and outs, as Kumi makes them.
fn gen_code(inner: &Patcher) -> Option<String> {
    let mut code = None;
    for item in &inner.boxes {
        match item.maxclass() {
            "codebox" if code.is_none() => code = Some(item.str("code").replace("\r\n", "\n").replace('\r', "\n")),
            "newobj" if matches!(item.class(), "in" | "out") => {}
            _ => return None,
        }
    }
    code
}

/// The JavaScript a box holds in the device itself: a [v8] or [js] with its code embedded, a [v8ui]'s, a
/// [v8.codebox]'s.
fn embedded_code(item: &MaxBox) -> Option<String> {
    let code = match item.maxclass() {
        "v8.codebox" => item.fields.get("code")?.as_str()?,
        _ => {
            let file = item.fields.get("textfile")?;
            if file.get("embed").and_then(Value::as_f64) != Some(1.0) {
                return None;
            }
            file.get("text")?.as_str()?
        }
    };
    Some(code.replace("\r\n", "\n").replace('\r', "\n"))
}

/// A box as the notation writes it (`[metro 125]`, `(bang)`, `[gen~] { … }`), or why it can't be: a box that isn't one
/// of Max's objects or a message (a panel, a comment, a bpatcher), a gen~ patched with gen's operators, code in a file
/// of its own, or a subpatcher holding any of these.
fn written(item: &MaxBox) -> Result<String, String> {
    let block = |head: &str, inside: &str| {
        let inside = inside.trim_matches('\n');
        format!("[{head}] {{\n{}\n}}", if inside.contains('\n') { inside.trim_end() } else { inside.trim() })
    };
    match item.maxclass() {
        "message" if !item.text().contains('\n') => Ok(format!("({})", item.text())),
        "newobj" => {
            let text = item.text();
            if text.trim().is_empty() || text.contains([']', '\n']) {
                return Err("an object whose text the notation can't hold".into());
            }
            match (item.class(), &item.patcher) {
                ("gen~" | "gen" | "mc.gen~", Some(inner)) => {
                    gen_code(inner).map(|code| block(text, &code)).ok_or_else(|| "a gen patch of gen's operators, not code".into())
                }
                ("p" | "patcher", Some(_)) if item.args().first().is_some_and(|name| name.starts_with("kumi.")) => {
                    Err(format!("Kumi's building block [{}], made the tested way", item.args().join(" ")))
                }
                ("p" | "patcher", Some(inner)) => {
                    let (patch, kept) = notation_of(inner, &local_names(inner, &mut HashSet::new(), &HashSet::new()), &BTreeMap::new());
                    if kept.is_empty() {
                        Ok(block(text, &patch))
                    } else {
                        Err("a subpatcher holding what the notation can't write".into())
                    }
                }
                (_, Some(_)) => Err("an object with a patcher of its own the notation can't write".into()),
                ("v8" | "js", None) => embedded_code(item).map(|code| block(text, &code)).ok_or_else(|| {
                    let file = Some(item.str("filename").trim())
                        .filter(|file| !file.is_empty() && *file != "none")
                        .or(item.args().first().copied());
                    file.map_or_else(|| "its code is a file".to_string(), |file| format!("its code is the file {file}"))
                }),
                _ => Ok(format!("[{text}]")),
            }
        }
        "v8.codebox" | "v8ui" => embedded_code(item).map(|code| block(item.maxclass(), &code)).ok_or_else(|| "its code is a file".into()),
        "comment" | "live.comment" => Err(format!("a comment: \"{}\"", super::patch::clip(item.text(), 40))),
        other => Err(format!("a {other}{}", if item.shown() { ", on the face" } else { "" })),
    }
}

/// A patcher in the notation: each box it can write, by its name, then its cords (a control by its name, the device's
/// way in and out as in and out); and what's kept as it is, each by its name and what it is. A box the notation can't
/// write still has its name, for cords and to be removed.
fn notation_of(patcher: &Patcher, names: &HashMap<String, String>, controls: &BTreeMap<String, String>) -> (String, Vec<String>) {
    let control_of: HashMap<&str, &str> = controls.iter().map(|(name, id)| (id.as_str(), name.as_str())).collect();
    let mut lines = Vec::new();
    let mut kept = Vec::new();
    // Max numbers a subpatcher's inlets and outlets from the left: written in that order, they're made in it again.
    let mut order: Vec<&MaxBox> = patcher.boxes.iter().collect();
    let x = |item: &MaxBox| item.rect().map_or(0.0, |rect| rect.x);
    order.sort_by(|a, b| {
        let rank = |item: &MaxBox| match item.class() {
            "inlet" => 0,
            "outlet" => 1,
            _ => 2,
        };
        rank(a).cmp(&rank(b)).then(if rank(a) < 2 { x(a).total_cmp(&x(b)) } else { std::cmp::Ordering::Equal })
    });
    for item in order {
        let Some(name) = names.get(item.id()) else { continue };
        if name == "in" || name == "out" || control_of.contains_key(item.id()) {
            continue;
        }
        match written(item) {
            Ok(text) => lines.push(format!("{name} = {text}")),
            Err(why) => kept.push(format!("{name}: {why}")),
        }
    }
    let end = |id: &str| -> Option<String> { control_of.get(id).map(|name| format!("\"{name}\"")).or_else(|| names.get(id).cloned()) };
    let mut cords = Vec::new();
    for cord in &patcher.cords {
        let (Some(from), Some(to)) = (end(&cord.from), end(&cord.to)) else { continue };
        let port = |port: usize| if port == 0 { String::new() } else { format!(".{port}") };
        cords.push(format!("{from}{} -> {to}{}", port(cord.outlet), port(cord.inlet)));
    }
    if !cords.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(cords);
    }
    (lines.join("\n"), kept)
}

/// A control's parameter as Live shows it: its object, its range or options, and its default.
fn said_control(name: &str, item: &MaxBox) -> String {
    let valueof = item.valueof().cloned().unwrap_or_default();
    let number = |key: &str| valueof.get(key).and_then(Value::as_f64);
    let initial = valueof.get("parameter_initial").and_then(|value| value.get(0).or(Some(value))).and_then(Value::as_f64);
    let options: Vec<&str> =
        valueof.get("parameter_enum").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let class = item.maxclass();
    if valueof.get("parameter_type").and_then(Value::as_f64) == Some(2.0) && !options.is_empty() {
        let default = initial.and_then(|at| options.get(at as usize)).unwrap_or(&options[0]);
        return format!("\"{name}\" ({class}: {}; {default})", options.join(" / "));
    }
    let unit = match valueof.get("parameter_unitstyle").and_then(Value::as_f64).map(|style| style as i64) {
        Some(2) => " ms".to_string(),
        Some(3) => " Hz".into(),
        Some(4) => " dB".into(),
        Some(5) => " %".into(),
        Some(6) => " pan".into(),
        Some(7) => " st".into(),
        Some(8) => " note".into(),
        Some(9) => valueof.get("parameter_units").and_then(Value::as_str).map_or(String::new(), |units| format!(" {units}")),
        _ => String::new(),
    };
    let (min, max) = (number("parameter_mmin").unwrap_or(0.0), number("parameter_mmax").unwrap_or(127.0));
    let default = initial.map_or(String::new(), |initial| format!("; {}", to_string(initial)));
    format!("\"{name}\" ({class}: {}–{}{unit}{default})", to_string(min), to_string(max))
}

/// The device's controls as Live shows them, in the order they're on its face.
pub fn said_controls(patcher: &Patcher) -> Vec<String> {
    let mut seen = HashSet::new();
    patcher
        .boxes
        .iter()
        .filter_map(|item| parameter(item).filter(|name| seen.insert(name.to_string())).map(|name| said_control(name, item)))
        .collect()
}

/// What the device records it was made from, when Kumi made it from another.
fn provenance(patcher: &Patcher) -> Option<String> {
    patcher.boxes.iter().find(|item| item.str("varname") == FROM_NAME).map(|item| item.text().to_string()).filter(|text| !text.is_empty())
}

fn kind_name(kind: DeviceType) -> &'static str {
    match kind {
        DeviceType::MidiEffect => "MIDI effect",
        DeviceType::AudioEffect => "audio effect",
        DeviceType::Instrument => "instrument",
    }
}

/// Text cut to `most` characters at a line's end, and how many lines were left out.
fn cut(text: &str, most: usize) -> (String, usize) {
    if text.chars().count() <= most {
        return (text.to_string(), 0);
    }
    let head: String = text.chars().take(most).collect();
    let kept = head.rfind('\n').map_or(head.as_str(), |at| &head[..at]);
    (kept.to_string(), text.lines().count() - kept.lines().count())
}

/// The answer to opening a device: what it is, its controls, its patch in the notation (each box by the name a change
/// uses), what's kept as it is, and how to make a new device from it.
pub fn describe(original: &Original) -> Value {
    let patcher = original.patcher();
    let names = names(&patcher, original.kind);
    let (patch, kept) = notation_of(&patcher, &names.by_id, &names.controls);
    let (patch, left) = cut(&patch, MOST_SAID);
    let mut result = json!({
        "device": original.name,
        "type": kind_name(original.kind),
        "file": original.path.to_string_lossy(),
        "controls": said_controls(&patcher),
        "patch": if left > 0 { format!("{patch}\n# … and {left} more lines, left out here") } else { patch },
    });
    let fields = result.as_object_mut().expect("an object");
    let ways: Vec<&str> =
        [("in", &names.way_in), ("out", &names.way_out)].iter().filter(|(_, id)| id.is_none()).map(|(way, _)| *way).collect();
    if !ways.is_empty() {
        let what = if ways.len() == 2 { "in or out".to_string() } else { ways[0].to_string() };
        fields.insert(
            "ways".into(),
            json!(format!(
                "Kumi found no single way {what} for this {}: a change reaches it through its boxes' names.",
                kind_name(original.kind)
            )),
        );
    }
    if !kept.is_empty() {
        fields.insert("kept".into(), json!(kept));
    }
    if let Some(frozen) = &original.frozen {
        let mut files: Vec<&str> = frozen.files.iter().filter(|file| !file.main()).map(|file| file.name.as_str()).collect();
        files.dedup();
        let shown = files.iter().take(12).copied().collect::<Vec<_>>().join(", ");
        fields.insert(
            "frozen".into(),
            json!(format!(
                "Frozen, with {} files of its own ({shown}{}): a new device keeps them as they are.",
                files.len(),
                if files.len() > 12 { ", …" } else { "" }
            )),
        );
    }
    if let Some(from) = provenance(&patcher) {
        fields.insert("madeFrom".into(), json!(from));
    }
    fields.insert(
        "next".into(),
        json!(
            "To make a new device from it, call make_device with this from and only what changes; everything else stays as it is, and this file is never written. patch: new boxes and cords in the notation, naming its boxes by the names above (in and out are its way in and out, its controls go by their names); a box given again by its name (name = [new text], or its code in a { … } block) changes in place. controls: new controls for its face, or one of its own by name to change it. remove: boxes by name, or cords (\"a.1 -> b\"). name: the new device's name (the original's, numbered, when left out); about: its description. It's saved in Kumi's folder of the User Library. What's kept can be wired to and removed, not changed."
        ),
    );
    result
}

/// What a change asks for, checked as make_device checks a new device's.
#[derive(Debug, Clone, Default)]
pub struct Asked {
    /// New boxes and cords, and boxes given again, in the notation.
    pub patch: String,
    /// Boxes, controls and cords to take out.
    pub remove: Vec<String>,
    /// New controls, or the device's own by name, changed.
    pub controls: Vec<Control>,
    /// The new device's name (its title, where it has one).
    pub name: String,
    /// Its description, when a new one is given.
    pub about: Option<String>,
}

/// The new device's patcher, and what was found making it.
#[derive(Debug, Clone)]
pub struct Changed {
    pub document: Value,
    pub found: Findings,
    /// Its controls as Live shows them.
    pub controls: Vec<String>,
    /// The controls it keeps from the original, by name: what carries a setting over from one to the other.
    pub shared: Vec<String>,
}

/// Fresh box ids for a patcher: obj- and a number past those it has.
struct Ids {
    taken: HashSet<String>,
    next: usize,
}

impl Ids {
    fn of(patcher: &Patcher) -> Ids {
        let taken: HashSet<String> = patcher.boxes.iter().map(|item| item.id().to_string()).collect();
        let next = taken.iter().filter_map(|id| id.strip_prefix("obj-")?.parse::<usize>().ok()).max().unwrap_or(0) + 1;
        Ids { taken, next }
    }

    fn fresh(&mut self) -> String {
        loop {
            let id = format!("obj-{}", self.next);
            self.next += 1;
            if self.taken.insert(id.clone()) {
                return id;
            }
        }
    }

    /// `wanted`, or wanted numbered, unused as an id and a scripting name.
    fn named(&mut self, wanted: &str, varnames: &HashSet<String>) -> String {
        let mut name = wanted.to_string();
        let mut again = 2;
        while self.taken.contains(&name) || varnames.contains(&name) {
            name = format!("{wanted}-{again}");
            again += 1;
        }
        self.taken.insert(name.clone());
        name
    }
}

/// A box as Max's JSON, its own patcher read back in (its place in the fields kept as a null).
fn max_box(mut fields: Map<String, Value>) -> MaxBox {
    let patcher = match fields.get_mut("patcher") {
        Some(inner) if inner.is_object() => Some(Patcher::read(&inner.take(), &NoFiles)),
        _ => None,
    };
    MaxBox { fields, patcher, file: None, code: None }
}

/// A box's fields after it's given again: what it is from the new one, where it sits and how it looks from the old.
fn retyped(old: &MaxBox, new: Map<String, Value>) -> MaxBox {
    let Value::Object(mut fields) = old.to_value() else { return old.clone() };
    for key in WHAT_IT_IS {
        fields.shift_remove(key);
    }
    for (key, value) in new {
        if WHAT_IT_IS.contains(&key.as_str()) {
            fields.insert(key, value);
        }
    }
    let mut item = max_box(fields);
    // A subpatcher made anew from the notation is laid out as Kumi lays out its own.
    if let Some(inner) = item.patcher.as_mut().filter(|inner| !inner.is_gen()) {
        arrange(inner);
    }
    item
}

/// A box as the notation writes it, to tell one given again unchanged (the `@embed 1` Kumi adds to code it embeds
/// aside).
fn sameness(item: &MaxBox) -> Option<String> {
    written(item).ok().map(|text| {
        text.replace(" @embed 1", "")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join(
                "
",
            )
            .trim()
            .to_string()
    })
}

/// Where a cord's end is: the box it reaches by its id, as a cord names it in a change's patch or in remove.
fn end_id(text: &str, names: &Names) -> Option<String> {
    let text = text.trim();
    match text {
        "in" => names.way_in.clone(),
        "out" => names.way_out.clone(),
        _ => match text.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
            Some(control) => names.controls.get(control).cloned(),
            None => names.by_name.get(text).cloned(),
        },
    }
}

/// One end of a cord as remove gives it: a box and its port ("metro_3.1", "\"Rate\"").
fn cord_end(text: &str) -> (&str, usize) {
    let text = text.trim();
    match text.rsplit_once('.') {
        Some((head, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) && (is_name(head) || head.ends_with('"')) => {
            (head, port.parse().unwrap_or(0))
        }
        _ => (text, 0),
    }
}

/// The free spot on the face for a block of new controls `size` big (width, height): the first, from the left and the
/// top, that no control, comment or picture already shown covers; None when there's none.
fn free_spot(shown: &[Rect], width: f64, size: (f64, f64)) -> Option<(f64, f64)> {
    let (w, h) = size;
    if w > width || h > look::HEIGHT {
        return None;
    }
    let mut x = 0.0;
    while x + w <= width {
        let mut y = 0.0;
        while y + h <= look::HEIGHT {
            let spot = Rect::new(x - 4.0, y - 4.0, w + 8.0, h + 8.0);
            if !shown.iter().any(|rect| rect.overlaps(&spot)) {
                return Some((x, y));
            }
            y += 4.0;
        }
        x += 4.0;
    }
    None
}

/// The device's face as wide as it is: its width as Live shows it, else as far right as anything on it reaches.
fn face_width(patcher: &Patcher) -> f64 {
    let shown =
        patcher.boxes.iter().filter(|item| item.shown()).filter_map(MaxBox::presentation_rect).map(|rect| rect.right()).fold(0.0, f64::max);
    match patcher.fields.get("devicewidth").and_then(Value::as_f64) {
        Some(width) if width > 0.0 => width,
        _ => shown,
    }
}

/// The new device made from the original with only what's asked changed, and what was found making it. `reference`
/// is what Kumi learned of the installed Max, for each new object's inlets and outlets.
pub fn change(original: &Original, asked: &Asked, reference: Option<&Reference>) -> Changed {
    let base = original.patcher();
    let names = names(&base, original.kind);
    let mut patcher = base.clone();
    let mut found = Findings::default();
    let mut ids = Ids::of(&patcher);
    let varnames: HashSet<String> =
        patcher.boxes.iter().map(|item| item.str("varname").to_string()).filter(|name| !name.is_empty()).collect();

    // What's taken out: boxes with their cords, and cords.
    let mut removed: HashSet<String> = HashSet::new();
    for entry in &asked.remove {
        let entry = entry.trim();
        if let Some((from, to)) = entry.split_once("->").or_else(|| entry.split_once('→')) {
            let ((from, outlet), (to, inlet)) = (cord_end(from), cord_end(to));
            match (end_id(from, &names), end_id(to, &names)) {
                (Some(from_id), Some(to_id)) => {
                    let before = patcher.cords.len();
                    patcher
                        .cords
                        .retain(|cord| !(cord.from == from_id && cord.outlet == outlet && cord.to == to_id && cord.inlet == inlet));
                    if patcher.cords.len() == before {
                        found.wrong.push(format!("remove {entry}: there's no such cord, so nothing was taken out."));
                    }
                }
                _ => found.wrong.push(format!(
                    "remove {entry}: {} isn't one of its boxes, so nothing was taken out.",
                    if end_id(from, &names).is_none() { from } else { to }
                )),
            }
            continue;
        }
        let name = entry.trim_matches('"');
        if name == "in" || name == "out" {
            found.wrong.push(format!("remove {entry}: {name} is the device's way {name}, so it stays."));
            continue;
        }
        match end_id(entry, &names).or_else(|| names.controls.get(name).cloned()) {
            Some(id) => {
                removed.insert(id);
            }
            None => found.wrong.push(format!("remove {entry}: the device has no box or control of that name, so nothing was taken out.")),
        }
    }
    // A control Kumi labelled goes with its label.
    let labels: HashSet<String> = names.controls.iter().filter(|(_, id)| removed.contains(*id)).map(|(name, _)| name.clone()).collect();
    for item in &patcher.boxes {
        let kumis = item.id().starts_with("kumi-face-") || item.id().starts_with("kumi-added-");
        if kumis && item.maxclass() == "live.comment" && labels.contains(item.text()) {
            removed.insert(item.id().to_string());
        }
    }
    patcher.boxes.retain(|item| !removed.contains(item.id()));
    patcher.cords.retain(|cord| !removed.contains(&cord.from) && !removed.contains(&cord.to));

    // Its own controls changed in place, and the new ones laid out as a block of their own.
    let mut controls: BTreeMap<String, String> =
        names.controls.iter().filter(|(_, id)| !removed.contains(*id)).map(|(name, id)| (name.clone(), id.clone())).collect();
    let mut fresh_controls: Vec<Control> = Vec::new();
    for control in &asked.controls {
        let own = controls.iter().find(|(name, _)| name.eq_ignore_ascii_case(control.name())).map(|(_, id)| id.clone());
        let Some(id) = own else {
            fresh_controls.push(control.clone());
            continue;
        };
        let Some(at) = patcher.index_of(&id) else { continue };
        let (item, valueof) = look::control_box(control);
        let old = &patcher.boxes[at];
        let mut fields = old.fields.clone();
        let mut merged = old.valueof().cloned().unwrap_or_default();
        if let Value::Object(valueof) = valueof {
            merged.extend(valueof);
        }
        if let Value::Object(item) = item {
            fields.extend(item);
        }
        fields.insert("saved_attribute_attributes".into(), json!({ "valueof": merged }));
        if fields.get("maxclass") != old.fields.get("maxclass") {
            // Another kind of control takes the size Kumi gives that kind, where the old one was.
            let size = look::lay_out(std::slice::from_ref(control), &look::Extras::default()).spots[0].rect;
            if let Some(rect) = old.presentation_rect() {
                fields.insert("presentation_rect".into(), json!([rect.x, rect.y, size[2], size[3]]));
            }
        }
        patcher.boxes[at].fields = fields;
        found.changed.push(format!("controls: \"{}\" was changed in place, where it is on the face.", control.name()));
    }
    let mut face_boxes: Vec<MaxBox> = Vec::new();
    let mut face_cords: Vec<Cord> = Vec::new();
    let mut fresh_ids: Vec<String> = Vec::new();
    if !fresh_controls.is_empty() {
        let face = look::lay_out(&fresh_controls, &look::Extras::default());
        let (marks, mark_cords) = look::marks(&fresh_controls, &face, "kumi-face-playhead");
        let mut renamed: HashMap<String, String> = HashMap::new();
        let mut blocks: Vec<Map<String, Value>> = Vec::new();
        for (control, spot) in fresh_controls.iter().zip(&face.spots) {
            let id = ids.fresh();
            let (item, valueof) = look::control_box(control);
            let varname = ids.named(control.name(), &varnames);
            let mut fields =
                json!({ "id": id, "varname": varname, "parameter_enable": 1, "presentation": 1, "patching_rect": [0.0, 0.0, 44.0, 48.0] });
            super::gen::extend(&mut fields, item);
            super::gen::extend(&mut fields, json!({ "saved_attribute_attributes": { "valueof": valueof } }));
            look::put(&mut fields, spot);
            if let Value::Object(fields) = fields {
                blocks.push(fields);
            }
            controls.insert(control.name().to_string(), id.clone());
            fresh_ids.push(id);
        }
        for mark in marks {
            let Value::Object(mut fields) = mark else { continue };
            let old = fields.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let id = ids.named(&old.replace("kumi-face", "kumi-added"), &varnames);
            if fields.contains_key("varname") {
                fields.insert("varname".into(), json!(id));
            }
            fields.insert("id".into(), json!(id));
            renamed.insert(old, id);
            blocks.push(fields);
        }
        for (from, outlet, to, inlet) in mark_cords {
            let end = |id: &String| renamed.get(id).cloned().unwrap_or_else(|| id.clone());
            face_cords.push(Cord::new(&end(&from), outlet as usize, &end(&to), inlet as usize));
        }
        // The block goes where the face has room for it, or the face grows to the right to hold it.
        let rects: Vec<Rect> = blocks.iter().filter_map(|fields| fields.get("presentation_rect").and_then(Rect::of)).collect();
        let (left, top) =
            (rects.iter().map(|rect| rect.x).fold(f64::MAX, f64::min), rects.iter().map(|rect| rect.y).fold(f64::MAX, f64::min));
        let (right, bottom) = (rects.iter().map(Rect::right).fold(0.0, f64::max), rects.iter().map(Rect::bottom).fold(0.0, f64::max));
        let width = face_width(&patcher);
        let shown: Vec<Rect> = patcher
            .boxes
            .iter()
            .filter(|item| item.shown() && item.fields.get("background").and_then(Value::as_f64) != Some(1.0))
            .filter_map(MaxBox::presentation_rect)
            .collect();
        let (dx, dy) = match free_spot(&shown, width, (right - left, bottom - top)) {
            Some((x, y)) => (x - left, y - top),
            None => {
                let grown = width + 8.0 + (right - left) + 8.0;
                for key in ["devicewidth"] {
                    patcher.fields.insert(key.into(), json!(grown));
                }
                if let Some(Value::Array(rect)) = patcher.fields.get_mut("openrect") {
                    if rect.get(2).and_then(Value::as_f64).is_some_and(|w| (w - width).abs() < 0.5 || w == 0.0) && rect.len() == 4 {
                        rect[2] = json!(grown);
                    }
                }
                found.notes.push(format!(
                    "controls: the face had no room for the new controls, so it's {} pixels wider, with them at its right.",
                    to_string((grown - width).round())
                ));
                (width + 8.0 - left, 0.0)
            }
        };
        let names_shown: Vec<String> =
            blocks.iter().filter_map(|fields| fields.get("varname").and_then(Value::as_str).map(str::to_string)).collect();
        for mut fields in blocks {
            if let Some(rect) = fields.get("presentation_rect").and_then(Rect::of) {
                fields.insert("presentation_rect".into(), Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h).to_value());
            }
            face_boxes.push(max_box(fields));
        }
        // A face with Kumi's Map page shows the new controls on its main page, and hides them on the Map page.
        let quoted = |name: &String| if name.contains(' ') { format!("\"{name}\"") } else { name.clone() };
        for (page, verb) in [("kumi-page-show-main", "show"), ("kumi-page-show-map", "hide")] {
            if let Some(item) = patcher.boxes.iter_mut().find(|item| item.id() == page) {
                let text = item.text().to_string();
                let added: Vec<String> = names_shown.iter().map(|name| format!("script {verb} {}", quoted(name))).collect();
                item.fields.insert("text".into(), json!(format!("{}, {text}", added.join(", "))));
            }
        }
    }

    // The patch: boxes new and given again, and cords, beside the device's own boxes, which it names.
    let existing: HashSet<String> = names.by_name.keys().cloned().collect();
    let (notation, parse_found) = parse_beside(&asked.patch, &existing);
    found.append(parse_found);
    let own = |id: &str| format!("{OWN}{id}");
    let mut outside_boxes: HashMap<String, String> = names.by_name.iter().map(|(name, id)| (name.clone(), own(id))).collect();
    if let Some(id) = &names.way_in {
        outside_boxes.insert("in".into(), own(id));
    }
    if let Some(id) = &names.way_out {
        outside_boxes.insert("out".into(), own(id));
    }
    let outside = Outside {
        boxes: outside_boxes,
        controls: controls.iter().map(|(name, id)| (name.clone(), own(id))).collect(),
        reference,
        output_stage: original.kind != DeviceType::MidiEffect && names.way_out.is_some(),
    };
    let expanded = expand(&notation, &outside);
    found.append(expanded.found);
    // Each box the patch made: one of the device's given again (its own id), or a new one.
    let mut made_as: HashMap<String, String> = HashMap::new();
    let mut new_ids: Vec<String> = Vec::new();
    for entry in expanded.boxes {
        let Value::Object(mut entry) = entry else { continue };
        let Some(Value::Object(mut fields)) = entry.remove("box") else { continue };
        let made = fields.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let at: usize = made.strip_prefix("obj-").and_then(|n| n.parse::<usize>().ok()).unwrap_or(0).saturating_sub(1);
        let item = &notation.boxes[at.min(notation.boxes.len().saturating_sub(1))];
        let line = item.line;
        let given_again = item.name.as_ref().and_then(|name| names.by_name.get(name)).cloned();
        match given_again {
            Some(id) => {
                made_as.insert(made, id.clone());
                let name = item.name.clone().unwrap_or_default();
                if removed.contains(&id) {
                    found.wrong.push(format!("line {line}: {name} is removed, so it wasn't given again."));
                    continue;
                }
                let Some(at) = patcher.index_of(&id) else { continue };
                let old = &patcher.boxes[at];
                if controls.values().any(|control| *control == id) {
                    found
                        .wrong
                        .push(format!("line {line}: {name} is a control on the face, so it stays as it is: change it through controls."));
                    continue;
                }
                fields.shift_remove("varname");
                let new = max_box(fields.clone());
                if sameness(&new) == sameness(old) {
                    continue;
                }
                if let Err(why) = written(old) {
                    found.wrong.push(format!(
                        "line {line}: {name} is kept as it is ({why}), so it wasn't changed: remove it and make a new box in its place."
                    ));
                    continue;
                }
                let (ins, outs) = (old.inlets(), old.outlets());
                // An object whose ports Kumi doesn't know from Max (Max isn't here) keeps those it had, beside those its
                // cords use.
                let text = fields.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                if new.class() == old.class() && reference.and_then(|reference| reference.ports(&text)).is_none() {
                    fields.insert("numinlets".into(), json!(ins.max(new.inlets())));
                    fields.insert("numoutlets".into(), json!(outs.max(new.outlets())));
                    if let Some(types) = old.fields.get("outlettype").filter(|_| outs > new.outlets()) {
                        fields.insert("outlettype".into(), types.clone());
                    }
                }
                patcher.boxes[at] = retyped(old, fields);
                let item = &patcher.boxes[at];
                found.changed.push(format!("line {line}: {name} was changed in place, where it sits, to {}.", item.label()));
                // A cord from a port it no longer has goes, as Max takes it out.
                if item.inlets() < ins || item.outlets() < outs {
                    let (ins, outs) = (item.inlets(), item.outlets());
                    let before = patcher.cords.len();
                    patcher.cords.retain(|cord| !((cord.from == id && cord.outlet >= outs) || (cord.to == id && cord.inlet >= ins)));
                    if patcher.cords.len() < before {
                        found.changed.push(format!(
                            "line {line}: {name} has fewer inlets or outlets now, so {} cords to the ports it lost were taken out.",
                            before - patcher.cords.len()
                        ));
                    }
                }
            }
            None => {
                let id = ids.fresh();
                made_as.insert(made, id.clone());
                // A new box's name is its scripting name, unless another box has that one.
                if fields.get("varname").and_then(Value::as_str).is_some_and(|name| varnames.contains(name)) {
                    fields.shift_remove("varname");
                }
                fields.insert("id".into(), json!(id));
                new_ids.push(id);
                patcher.boxes.push(max_box(fields));
            }
        }
    }
    // The new controls and their labels go in the patcher too, after the patch's new boxes.
    new_ids.extend(face_boxes.iter().map(|item| item.id().to_string()));
    patcher.boxes.extend(face_boxes);
    let mut new_cords: Vec<(Cord, usize)> = Vec::new();
    for (entry, line) in &expanded.lines {
        let end = |key: &str| -> (String, usize) {
            let id = entry["patchline"][key][0].as_str().unwrap_or_default();
            let id = id.strip_prefix(OWN).map(str::to_string).or_else(|| made_as.get(id).cloned()).unwrap_or_else(|| id.to_string());
            (id, entry["patchline"][key][1].as_u64().unwrap_or(0) as usize)
        };
        let ((from, outlet), (to, inlet)) = (end("source"), end("destination"));
        if let Some(gone) = [&from, &to].into_iter().find(|id| removed.contains(*id)) {
            let name = names.by_id.get(gone).cloned().unwrap_or_else(|| gone.clone());
            found.wrong.push(format!("line {line}: {name} is removed, so the cord to it was left out."));
            continue;
        }
        let (Some(source), Some(destination)) = (patcher.find(&from), patcher.find(&to)) else { continue };
        if outlet >= source.outlets() {
            let has = ports_had("outlet", source.outlets());
            found.wrong.push(format!(
                "line {line}: a cord leaves outlet {outlet} of {}, which has {has}, so Kumi left it out, as Max would.",
                source.label()
            ));
            continue;
        }
        if inlet >= destination.inlets() {
            let has = ports_had("inlet", destination.inlets());
            found.wrong.push(format!(
                "line {line}: a cord enters inlet {inlet} of {}, which has {has}, so Kumi left it out, as Max would.",
                destination.label()
            ));
            continue;
        }
        let cord = Cord::new(&from, outlet, &to, inlet);
        let same =
            |other: &Cord| other.from == cord.from && other.outlet == cord.outlet && other.to == cord.to && other.inlet == cord.inlet;
        if !patcher.cords.iter().any(same) && !new_cords.iter().any(|(other, _)| same(other)) {
            new_cords.push((cord, *line));
        }
    }
    patcher.cords.extend(new_cords.into_iter().map(|(cord, _)| cord));
    patcher.cords.extend(face_cords);
    for (control, id) in fresh_controls.iter().zip(&fresh_ids) {
        if !patcher.cords.iter().any(|cord| cord.from == *id || cord.to == *id) {
            found.wrong.push(format!(
                "patch: \"{}\" isn't wired, so it changes nothing: take its value from \"{}\" -> … (or leave the control out).",
                control.name(),
                control.name()
            ));
        }
    }
    place(&mut patcher, &new_ids);
    record(&mut patcher, original, &mut ids);
    if patcher.fields.contains_key("title") {
        patcher.fields.insert("title".into(), json!(asked.name));
    }
    if let Some(about) = &asked.about {
        patcher.fields.insert("description".into(), json!(about));
    }

    // Kumi's checks, of what the change brought: what they find in the new device that they don't in the original.
    let reported = |finding: &&super::patch::check::Finding| REPORTED.iter().any(|family| finding.rule.starts_with(family));
    let mut before: Vec<String> =
        check_with(&base, standard(), reference).at_least(Level::Warn).filter(reported).map(ToString::to_string).collect();
    for finding in check_with(&patcher, standard(), reference).at_least(Level::Warn).filter(reported) {
        let said = finding.to_string();
        if let Some(at) = before.iter().position(|old| *old == said) {
            before.remove(at);
        } else if !(reference.is_some() && finding.rule == UNKNOWN_OBJECT) {
            found.rule(finding.level, format!("patch: {said}"));
        }
    }
    found.by_line();
    for text in found.wrong.iter_mut().chain(&mut found.changed).chain(&mut found.notes).filter(|text| text.starts_with("line ")) {
        text.insert_str(0, "patch ");
    }
    let kept: HashSet<&str> = names.controls.keys().map(String::as_str).collect();
    let shared = said_names(&patcher).into_iter().filter(|name| kept.contains(name.as_str())).collect();
    Changed { controls: said_controls(&patcher), shared, document: patcher.to_document(), found }
}

/// The names of a patcher's controls.
fn said_names(patcher: &Patcher) -> Vec<String> {
    let mut seen = HashSet::new();
    patcher.boxes.iter().filter_map(parameter).filter(|name| seen.insert(name.to_string())).map(str::to_string).collect()
}

/// New boxes placed in the patcher near what they're wired to: under a box they take from, else over one they send
/// to, else under everything, moved down until they cover no other box.
fn place(patcher: &mut Patcher, new_ids: &[String]) {
    let size = patcher.fields.get("default_fontsize").and_then(Value::as_f64).unwrap_or(12.0);
    let new: HashSet<&str> = new_ids.iter().map(String::as_str).collect();
    let mut placed: HashMap<String, Rect> = patcher
        .boxes
        .iter()
        .filter(|item| !new.contains(item.id()))
        .filter_map(|item| Some((item.id().to_string(), item.rect()?)))
        .collect();
    let lowest = placed.values().map(Rect::bottom).fold(0.0, f64::max);
    let leftmost = placed.values().map(|rect| rect.x).fold(f64::MAX, f64::min).min(1e6);
    let leftmost = if leftmost == 1e6 { 40.0 } else { leftmost };
    for id in new_ids {
        let Some(at) = patcher.index_of(id) else { continue };
        let item = &patcher.boxes[at];
        let given = item.rect().unwrap_or(Rect::new(0.0, 0.0, 60.0, 22.0));
        let (w, h) = match item.maxclass() {
            "newobj" | "message" => (text_width(item.text(), size).max(given.w.min(40.0)).max(30.0), given.h.max(22.0)),
            _ => (given.w, given.h),
        };
        let from = patcher.cords.iter().find(|cord| cord.to == *id && placed.contains_key(&cord.from)).map(|cord| placed[&cord.from]);
        let to = patcher.cords.iter().find(|cord| cord.from == *id && placed.contains_key(&cord.to)).map(|cord| placed[&cord.to]);
        // Where it would sit best first: under what it takes from (or beside that, a column to the right), else over what
        // it sends to.
        let spots = match (from, to) {
            (Some(above), _) => vec![
                Rect::new(above.x, above.bottom() + 30.0, w, h),
                Rect::new(above.right() + 30.0, above.bottom() + 30.0, w, h),
                Rect::new(above.right() + 30.0, above.y, w, h),
            ],
            (None, Some(below)) if below.y - h - 30.0 >= 0.0 => {
                vec![Rect::new(below.x, below.y - h - 30.0, w, h), Rect::new(below.right() + 30.0, below.y - h - 30.0, w, h)]
            }
            (None, Some(below)) => vec![Rect::new(below.right() + 30.0, below.y, w, h)],
            (None, None) => vec![Rect::new(leftmost, lowest + 40.0, w, h)],
        };
        let margin = |rect: &Rect| Rect::new(rect.x - 6.0, rect.y - 6.0, rect.w + 12.0, rect.h + 12.0);
        let free = |rect: &Rect| !placed.values().any(|other| margin(other).overlaps(rect));
        let mut rect = spots.iter().find(|spot| free(spot)).copied().unwrap_or(spots[0]);
        for _ in 0..400 {
            if free(&rect) {
                break;
            }
            rect.y += 30.0;
        }
        patcher.boxes[at].set_rect(rect);
        placed.insert(id.clone(), rect);
    }
}

/// The comment in the new device that records what it was made from (the file and its hash), with what the original's
/// own said, when Kumi made that one from another.
fn record(patcher: &mut Patcher, original: &Original, ids: &mut Ids) {
    let mut text = format!(
        "Kumi made this device from {} ({}, SHA-256 {}).",
        original.name,
        original.path.to_string_lossy(),
        &original.hash[..16.min(original.hash.len())]
    );
    if let Some(earlier) = provenance(&original.patcher()) {
        let earlier: String = earlier.chars().take(600).collect();
        text = format!("{text} Before that: {earlier}");
    }
    if let Some(item) = patcher.boxes.iter_mut().find(|item| item.str("varname") == FROM_NAME) {
        item.fields.insert("text".into(), json!(text));
        return;
    }
    let lowest = patcher.boxes.iter().filter_map(MaxBox::rect).map(|rect| rect.bottom()).fold(0.0, f64::max);
    let left = patcher.boxes.iter().filter_map(MaxBox::rect).map(|rect| rect.x).fold(f64::MAX, f64::min);
    let left = if left == f64::MAX { 40.0 } else { left };
    let lines = (text.chars().count() / 70 + 1) as f64;
    patcher.boxes.push(max_box(
        json!({ "id": ids.fresh(), "varname": FROM_NAME, "maxclass": "comment", "text": text, "linecount": lines, "numinlets": 1, "numoutlets": 0,
            "patching_rect": [left, lowest + 40.0, 420.0, 6.0 + 14.0 * lines] })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    ));
}

/// The new device's file: the original's, with the new patcher in place of its own and every other byte as it was. A
/// frozen device keeps every file frozen into it. None when a frozen device's files can't be rewritten safely.
pub fn device_bytes(original: &Original, document: &Value) -> Option<Vec<u8>> {
    let mut json = format!("{}\n", stringify_with_indent(document, "\t")).into_bytes();
    json.push(0);
    let chunk = match &original.frozen {
        None => json,
        Some(frozen) => frozen.with_file(frozen.main_file()?, &json)?,
    };
    with_patcher_chunk(&original.bytes, &chunk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_scripting_names_the_notation_reads_else_the_object_and_its_number() {
        let patcher = Patcher::read(
            &json!({ "patcher": { "boxes": [
                { "box": { "id": "obj-1", "maxclass": "newobj", "text": "midiin" } },
                { "box": { "id": "obj-2", "maxclass": "newobj", "text": "metro 125", "varname": "clock" } },
                { "box": { "id": "obj-3", "maxclass": "newobj", "text": "live.dial" } },
                { "box": { "id": "obj-4", "maxclass": "newobj", "text": "*~ 0.5", "varname": "two words" } },
                { "box": { "id": "obj-5", "maxclass": "newobj", "text": "metro 250", "varname": "clock" } },
                { "box": { "id": "obj-6", "maxclass": "newobj", "text": "midiout" } }
            ], "lines": [] } }),
            &NoFiles,
        );
        let names = names(&patcher, DeviceType::MidiEffect);
        let name = |id: &str| names.by_id[id].as_str();
        assert_eq!(
            [name("obj-1"), name("obj-2"), name("obj-3"), name("obj-4"), name("obj-5"), name("obj-6")],
            ["in", "clock", "live_dial_3", "box_4", "metro_5", "out"]
        );
        assert_eq!(names.by_name.get("clock").map(String::as_str), Some("obj-2"));
        assert!(!names.by_name.contains_key("in"), "in and out are the notation's own");
    }

    #[test]
    fn a_cords_end_in_remove_is_a_box_or_a_control_and_its_port() {
        assert_eq!(cord_end("metro_3.1"), ("metro_3", 1));
        assert_eq!(cord_end(" counter_4 "), ("counter_4", 0));
        assert_eq!(cord_end("\"Rate\".2"), ("\"Rate\"", 2));
        assert_eq!(cord_end("\"Rate\""), ("\"Rate\"", 0));
    }

    #[test]
    fn a_block_of_new_controls_goes_where_the_face_has_room() {
        let shown = [Rect::new(8.0, 8.0, 44.0, 48.0), Rect::new(60.0, 8.0, 44.0, 48.0)];
        assert_eq!(free_spot(&shown, 200.0, (44.0, 60.0)), Some((0.0, 60.0)), "under what's there, 4 pixels clear");
        assert_eq!(free_spot(&shown, 120.0, (44.0, 169.0)), None, "a full-height block in a full face");
        assert_eq!(free_spot(&[], 40.0, (44.0, 60.0)), None, "wider than the face");
    }
}
