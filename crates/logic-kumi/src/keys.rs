//! Logic's key commands: which keys a command is bound to, by its English name or its id, so a change can post the
//! producer's own binding (a remapped Cmd-T still splits) and `key-command` can run any command a producer names.
//!
//! Logic binds commands by id. The active set is in the producer's preferences (`com.apple.logic10.plist`, the
//! `KeyCommands` dictionary: one `{Key, Modifier, Flags, CharCode}` record per bound id, written whole once the
//! producer has touched the set) and, before that, in the factory preset the preferences name
//! (`lastSelectedKeyCommandsPath`, the U.S. layout by default), whose `LogicBinaryPreferences` blob ends with 5000
//! six-byte records indexed by id (`key, modifier, 0, charcode as two bytes, flags`). The modifier bits are 1 shift,
//! 4 control, 8 option, 32 command; `Flags` 1 says `Key` is a virtual key code (the keypad, the number row), else
//! `CharCode` is the character or an AppKit function key (`0xF700` up, `0xF728` forward delete, …) and `Key` the
//! classic code of it (8 delete, 13 return, 28 to 31 the arrows). The names are Logic's own, read out of
//! `Logic.framework` by `keys/extract_names.py` into `keys/names.json`, which the bridge is built with: Logic keeps
//! no table of them anywhere else. Measured on Logic Pro 12.3.1 (`docs/evidence/logic-m4-bridge-2026-10-20.md`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use serde_json::{json, Value};

use crate::refs::{bad, Fault};

/// The names, by id, as extracted from the Logic the bridge was built against.
const NAMES: &str = include_str!("keys/names.json");
/// The U.S. factory preset's bindings, by id, as `keys/extract_names.py` spells them: what a run binds when it can
/// read neither the preferences nor the preset, and what a replay of a recorded scene posts.
const FACTORY: &str = include_str!("keys/us-bindings.json");
/// The factory preset the preferences fall back to, in Logic's bundle.
pub const FACTORY_PRESET: &str = "Contents/Resources/Key Commands/U.S..logikcs";
/// The records at the end of a preset's blob: ids 0 to 4999.
const PRESET_IDS: usize = 5000;
const RECORD: usize = 6;

/// The key of a chord, as the helper posts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A character of the U.S. layout: `t`, `'`, `1`.
    Char(char),
    /// A key the helper names: `return`, `delete`, `left`, `f1`.
    Named(&'static str),
    /// A virtual key code: the keypad, and the number row when Logic binds the key rather than the character.
    Code(u16),
}

/// A key with its modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub shift: bool,
    pub ctrl: bool,
    pub opt: bool,
    pub cmd: bool,
}

impl Chord {
    /// The helper's `key` field.
    pub fn helper_key(&self) -> Value {
        match &self.key {
            Key::Char(c) => json!(c.to_string()),
            Key::Named(name) => json!(name),
            Key::Code(code) => json!(code),
        }
    }

    /// The helper's `mods` field.
    pub fn mods(&self) -> Vec<&'static str> {
        let mut mods = Vec::new();
        if self.cmd {
            mods.push("cmd");
        }
        if self.shift {
            mods.push("shift");
        }
        if self.opt {
            mods.push("opt");
        }
        if self.ctrl {
            mods.push("ctrl");
        }
        mods
    }

    /// The chord as Logic's Key Commands window shows it: `⌘T`, `⌥'`, `⇧⌘Z`.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if self.ctrl {
            text.push('⌃');
        }
        if self.opt {
            text.push('⌥');
        }
        if self.shift {
            text.push('⇧');
        }
        if self.cmd {
            text.push('⌘');
        }
        match &self.key {
            Key::Char(c) => text.push(c.to_ascii_uppercase()),
            Key::Named(name) => text.push_str(name),
            Key::Code(code) => text.push_str(&format!("key {code}")),
        }
        text
    }

    /// The chord a preference record stands for, if the helper can post it.
    pub fn of_record(key: u32, modifier: u32, flags: u32, char_code: u32) -> Option<Chord> {
        let key = if flags & 1 != 0 {
            Key::Code(u16::try_from(key).ok()?)
        } else {
            match char_code {
                0xF700 => Key::Named("up"),
                0xF701 => Key::Named("down"),
                0xF702 => Key::Named("left"),
                0xF703 => Key::Named("right"),
                0xF704..=0xF717 => Key::Named(F_KEYS[(char_code - 0xF704) as usize]),
                0xF728 => Key::Named("forwarddelete"),
                0xF729 => Key::Named("home"),
                0xF72B => Key::Named("end"),
                0xF72C => Key::Named("pageup"),
                0xF72D => Key::Named("pagedown"),
                0xF746 => Key::Named("help"),
                127 | 8 => Key::Named("delete"),
                13 => Key::Named("return"),
                3 => Key::Named("enter"),
                9 => Key::Named("tab"),
                27 => Key::Named("escape"),
                32 => Key::Named("space"),
                33..=126 => Key::Char(char::from(char_code as u8).to_ascii_lowercase()),
                _ => return None,
            }
        };
        Some(Chord { key, shift: modifier & 1 != 0, ctrl: modifier & 4 != 0, opt: modifier & 8 != 0, cmd: modifier & 32 != 0 })
    }

    /// The chord as [`Chord::parse`] reads it: `cmd-t`, `opt-'`, `shift-return`, `82` (a key code).
    pub fn spelled(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (on, name) in [(self.cmd, "cmd"), (self.shift, "shift"), (self.opt, "opt"), (self.ctrl, "ctrl")] {
            if on {
                parts.push(name.to_string());
            }
        }
        parts.push(match &self.key {
            Key::Char(c) => c.to_string(),
            Key::Named(name) => name.to_string(),
            Key::Code(code) => code.to_string(),
        });
        parts.join("-")
    }

    /// A chord written as the helper takes it, for the bridge's own fallbacks: `Chord::parse("cmd-t")`.
    pub fn parse(text: &str) -> Option<Chord> {
        // The hyphen is a key too: `cmd--` is Cmd-hyphen.
        let (mods, last) = match text.rsplit_once('-') {
            Some((mods, "")) => (mods.trim_end_matches('-'), "-"),
            Some((mods, last)) => (mods, last),
            None => ("", text),
        };
        let parts: Vec<&str> = mods.split('-').filter(|part| !part.is_empty()).collect();
        let key = match last {
            code if code.len() > 1 && code.bytes().all(|b| b.is_ascii_digit()) => Key::Code(code.parse().ok()?),
            k if k.chars().count() == 1 => Key::Char(k.chars().next()?.to_ascii_lowercase()),
            "return" | "delete" | "enter" | "tab" | "escape" | "space" | "up" | "down" | "left" | "right" | "home" | "end" | "pageup"
            | "pagedown" | "forwarddelete" => Key::Named(NAMED.iter().find(|n| **n == last)?),
            other => Key::Named(F_KEYS.iter().find(|n| **n == other)?),
        };
        let mut chord = Chord { key, shift: false, ctrl: false, opt: false, cmd: false };
        for part in parts {
            match part {
                "cmd" => chord.cmd = true,
                "shift" => chord.shift = true,
                "opt" => chord.opt = true,
                "ctrl" => chord.ctrl = true,
                _ => return None,
            }
        }
        Some(chord)
    }
}

const F_KEYS: [&str; 20] =
    ["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20"];
const NAMED: [&str; 15] = [
    "return",
    "delete",
    "enter",
    "tab",
    "escape",
    "space",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "forwarddelete",
];

/// A command as a request resolves it: its id, its English name when the bridge knows it, and its binding when it has
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: u32,
    pub name: Option<String>,
    pub chord: Option<Chord>,
}

impl Command {
    /// The command as a title says it: its name in quotes, or its id.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => crate::changes::quoted(name),
            None => format!("key command {}", self.id),
        }
    }
}

/// Where the bindings came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The producer's preferences, over the factory preset for what they leave out.
    Preferences(PathBuf),
    /// The factory preset alone.
    Factory(PathBuf),
    /// Nothing could be read: every command is unbound.
    None,
}

/// The key command set as the bridge resolves it: cached for the run.
#[derive(Debug, Clone)]
pub struct KeyCommands {
    /// The Logic version the names were extracted from.
    pub logic: String,
    names: HashMap<u32, String>,
    /// Lower-cased names without a trailing ellipsis, to their ids.
    by_name: HashMap<String, u32>,
    bindings: HashMap<u32, Chord>,
    pub source: Source,
}

/// A name as it is matched: lower-cased, without a trailing ellipsis, the whitespace folded.
fn folded(name: &str) -> String {
    let mut text = name.trim().to_lowercase();
    while text.ends_with('…') || text.ends_with("...") {
        text = text.trim_end_matches('…').trim_end_matches("...").trim_end().to_string();
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl KeyCommands {
    /// The names alone, no bindings: for a run that could read neither the preferences nor the preset.
    pub fn unbound() -> KeyCommands {
        let value: Value = serde_json::from_str(NAMES).expect("keys/names.json parses");
        let logic = value["logic"].as_str().unwrap_or_default().to_string();
        let mut names = HashMap::new();
        let mut by_name = HashMap::new();
        for (id, name) in value["commands"].as_object().into_iter().flatten() {
            let (Ok(id), Some(name)) = (id.parse::<u32>(), name.as_str()) else { continue };
            by_name.entry(folded(name)).or_insert(id);
            names.insert(id, name.to_string());
        }
        KeyCommands { logic, names, by_name, bindings: HashMap::new(), source: Source::None }
    }

    /// The names with the built-in U.S. factory bindings: the set a scene replays with, and a run's last fallback.
    pub fn embedded() -> KeyCommands {
        let mut set = KeyCommands::unbound();
        let value: Value = serde_json::from_str(FACTORY).expect("keys/us-bindings.json parses");
        for (id, spelled) in value["bindings"].as_object().into_iter().flatten() {
            if let (Ok(id), Some(chord)) = (id.parse::<u32>(), spelled.as_str().and_then(Chord::parse)) {
                set.bindings.insert(id, chord);
            }
        }
        set.source = Source::Factory(PathBuf::from("built in"));
        set
    }

    /// The set from the preferences' `KeyCommands` dictionary (as `plutil` gives it as JSON) over a preset's blob.
    pub fn from_parts(prefs: Option<&Value>, prefs_path: Option<&Path>, preset: Option<&[u8]>, preset_path: Option<&Path>) -> KeyCommands {
        let mut set = KeyCommands::unbound();
        if let Some(blob) = preset {
            for (id, chord) in preset_bindings(blob) {
                set.bindings.insert(id, chord);
            }
            if let Some(path) = preset_path {
                set.source = Source::Factory(path.to_path_buf());
            }
        }
        if let Some(entries) = prefs.and_then(Value::as_object) {
            let mut any = false;
            for (id, record) in entries {
                let Ok(id) = id.parse::<u32>() else { continue };
                let number = |key: &str| record.get(key).and_then(Value::as_u64).unwrap_or(0) as u32;
                any = true;
                match Chord::of_record(number("Key"), number("Modifier"), number("Flags"), number("CharCode")) {
                    Some(chord) => {
                        set.bindings.insert(id, chord);
                    }
                    None => {
                        set.bindings.remove(&id);
                    }
                }
            }
            if any {
                if let Some(path) = prefs_path {
                    set.source = Source::Preferences(path.to_path_buf());
                }
            }
        }
        set
    }

    /// The producer's set: the preferences in their home, the preset they name (else the U.S. one) in Logic's bundle.
    pub fn load(bundle: &Path, home: Option<&Path>) -> KeyCommands {
        let prefs_path = home.map(|home| home.join("Library/Preferences/com.apple.logic10.plist")).filter(|path| path.is_file());
        let prefs = prefs_path.as_deref().and_then(|path| plutil_json(path, "KeyCommands"));
        let named = prefs_path.as_deref().and_then(|path| plutil_raw(path, "lastSelectedKeyCommandsPath"));
        let preset_path = preset_path(bundle, named.as_deref());
        let preset =
            preset_path.as_deref().and_then(|path| plutil_raw(path, "LogicBinaryPreferences")).and_then(|text| base64_decode(&text));
        let mut set = KeyCommands::from_parts(prefs.as_ref(), prefs_path.as_deref(), preset.as_deref(), preset_path.as_deref());
        if preset.is_none() {
            // No preset to read: the built-in table stands under the preferences.
            let built_in = KeyCommands::embedded();
            for (id, chord) in built_in.bindings {
                set.bindings.entry(id).or_insert(chord);
            }
            if set.source == Source::None {
                set.source = built_in.source;
            }
        }
        set
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    pub fn chord(&self, id: u32) -> Option<&Chord> {
        self.bindings.get(&id)
    }

    /// How many commands have a name, and how many a binding.
    pub fn counts(&self) -> (usize, usize) {
        (self.names.len(), self.bindings.len())
    }

    /// The command a request names: by its English name as the Key Commands window lists it (an ellipsis and case
    /// aside), or by its id. A name the bridge doesn't know is refused with the closest it does.
    pub fn resolve(&self, asked: &Value) -> Result<Command, Fault> {
        let id = match asked {
            Value::Number(n) => n
                .as_u64()
                .filter(|n| *n > 0 && *n < PRESET_IDS as u64)
                .ok_or_else(|| bad("command: an id is a whole number from 1 to 4999"))? as u32,
            Value::String(text) if text.trim().parse::<u32>().is_ok() => {
                let id: u32 = text.trim().parse().unwrap_or(0);
                if id == 0 || id as usize >= PRESET_IDS {
                    return Err(bad("command: an id is a whole number from 1 to 4999"));
                }
                id
            }
            Value::String(text) => self.by_name_or_close(text)?,
            _ => return Err(bad("command: a key command's English name as Logic Pro's Key Commands window lists it, or its id")),
        };
        Ok(Command { id, name: self.names.get(&id).cloned(), chord: self.bindings.get(&id).cloned() })
    }

    fn by_name_or_close(&self, text: &str) -> Result<u32, Fault> {
        let wanted = folded(text);
        if wanted.is_empty() {
            return Err(bad("command: a key command's name, as Logic Pro's Key Commands window lists it"));
        }
        if let Some(id) = self.by_name.get(&wanted) {
            return Ok(*id);
        }
        let words: Vec<&str> = wanted.split(' ').filter(|w| w.len() > 2).collect();
        let mut close: Vec<(usize, &String)> = self
            .by_name
            .keys()
            .map(|known| (words.iter().filter(|w| known.contains(*w)).count(), known))
            .filter(|(hits, _)| *hits > 0)
            .collect();
        close.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
        let close: Vec<String> = close
            .into_iter()
            .take(5)
            .map(|(_, known)| crate::changes::quoted(self.names.get(&self.by_name[known]).map_or(known.as_str(), String::as_str)))
            .collect();
        if close.is_empty() {
            Err(bad(format!("Logic Pro has no key command named {}", crate::changes::quoted(text))))
        } else {
            Err(bad(format!("Logic Pro has no key command named {}; close: {}", crate::changes::quoted(text), close.join(", "))))
        }
    }
}

/// The bindings a preset's blob holds: its last 5000 records, one per id.
pub fn preset_bindings(blob: &[u8]) -> Vec<(u32, Chord)> {
    let table = PRESET_IDS * RECORD;
    if blob.len() < table {
        return Vec::new();
    }
    let base = blob.len() - table;
    (0..PRESET_IDS)
        .filter_map(|id| {
            let record = &blob[base + id * RECORD..base + (id + 1) * RECORD];
            if record.iter().all(|byte| *byte == 0) {
                return None;
            }
            let char_code = u32::from(record[3]) | (u32::from(record[4]) << 8);
            Chord::of_record(u32::from(record[0]), u32::from(record[1]), u32::from(record[5]), char_code).map(|chord| (id as u32, chord))
        })
        .collect()
}

/// The preset file the preferences name, by its file name in the bundle given (the path they keep names the bundle
/// Logic had when the set was chosen), else the U.S. preset.
fn preset_path(bundle: &Path, named: Option<&str>) -> Option<PathBuf> {
    let folder = bundle.join("Contents/Resources/Key Commands");
    let named = named.and_then(|path| Path::new(path.trim()).file_name().map(|name| folder.join(name))).filter(|path| path.is_file());
    named.or_else(|| Some(bundle.join(FACTORY_PRESET)).filter(|path| path.is_file()))
}

/// One key of a property list as JSON, through `plutil` (the preferences are a binary plist).
fn plutil_json(path: &Path, key: &str) -> Option<Value> {
    let output = Process::new("plutil").args(["-extract", key, "json", "-o", "-"]).arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// One key of a property list as text: a string as it is, a data blob as base64.
fn plutil_raw(path: &Path, key: &str) -> Option<String> {
    let output = Process::new("plutil").args(["-extract", key, "raw", "-o", "-"]).arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Standard base64 (whitespace and padding allowed), as `plutil -extract … raw` prints a data blob.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/key-commands").join(name);
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{}", path.display()))).unwrap()
    }

    /// The recorded set: this Mac's preferences over the U.S. factory preset of Logic 12.2.
    fn recorded() -> KeyCommands {
        let prefs = fixture("prefs.json");
        let preset = fixture("us.logikcs.json");
        let blob = base64_decode(preset["LogicBinaryPreferences"].as_str().unwrap()).unwrap();
        KeyCommands::from_parts(
            Some(&prefs["KeyCommands"]),
            Some(Path::new("/Users/me/Library/Preferences/com.apple.logic10.plist")),
            Some(&blob),
            Some(Path::new("/Applications/Logic Pro.app/Contents/Resources/Key Commands/U.S..logikcs")),
        )
    }

    #[test]
    fn a_record_is_read_as_the_chord_the_helper_posts() {
        // Cmd-T, as the preferences keep it.
        let chord = Chord::of_record(116, 32, 0, 116).unwrap();
        assert_eq!(chord, Chord { key: Key::Char('t'), shift: false, ctrl: false, opt: false, cmd: true });
        assert_eq!((chord.helper_key(), chord.mods(), chord.text()), (json!("t"), vec!["cmd"], "⌘T".into()));
        // Shift-Cmd-Z, Opt-apostrophe, Cmd-Delete, Shift-Return, Ctrl-Opt-Cmd-S.
        assert_eq!(Chord::of_record(122, 33, 0, 122).unwrap().text(), "⇧⌘Z");
        assert_eq!(Chord::of_record(39, 8, 0, 39).unwrap().text(), "⌥'");
        let delete = Chord::of_record(8, 32, 0, 127).unwrap();
        assert_eq!((delete.helper_key(), delete.text()), (json!("delete"), "⌘delete".into()));
        assert_eq!(Chord::of_record(13, 1, 0, 13).unwrap().helper_key(), json!("return"));
        assert_eq!(Chord::of_record(115, 44, 0, 115).unwrap().mods(), vec!["cmd", "opt", "ctrl"]);
        // The arrows and the function keys come as AppKit's characters; the keypad as virtual key codes.
        assert_eq!(Chord::of_record(28, 32, 0, 0xF702).unwrap().helper_key(), json!("left"));
        assert_eq!(Chord::of_record(0, 0, 0, 0xF704).unwrap().helper_key(), json!("f1"));
        assert_eq!(Chord::of_record(127, 0, 0, 0xF728).unwrap().helper_key(), json!("forwarddelete"));
        assert_eq!(Chord::of_record(82, 0, 1, 0).unwrap().helper_key(), json!(82));
        assert_eq!(Chord::of_record(0, 0, 0, 0xE4), None, "a character the helper can't post");
        assert_eq!(Chord::parse("cmd-shift-s").unwrap().text(), "⇧⌘S");
        assert_eq!(Chord::parse("opt-'").unwrap(), Chord::of_record(39, 8, 0, 39).unwrap());
        assert_eq!(Chord::parse("ctrl-left").unwrap().helper_key(), json!("left"));
        assert_eq!(Chord::parse("meta-x"), None);
        assert_eq!(Chord::parse("cmd--").unwrap().text(), "⌘-");
        assert_eq!(Chord::parse("-").unwrap(), Chord { key: Key::Char('-'), shift: false, ctrl: false, opt: false, cmd: false });
        assert_eq!(Chord::parse("cmd--").unwrap().spelled(), "cmd--");
        assert_eq!(base64_decode("SGVsbG8gS3VtaQ==").unwrap(), b"Hello Kumi");
        assert_eq!(base64_decode("AAAA\nAQ==").unwrap(), vec![0, 0, 0, 1]);
        assert_eq!(base64_decode("x*"), None);
    }

    #[test]
    fn the_recorded_set_resolves_names_and_ids_to_the_producers_bindings() {
        let set = recorded();
        assert_eq!(set.logic, "12.3.1");
        let (names, bindings) = set.counts();
        assert!(names > 1500 && bindings > 850, "{names} names, {bindings} bindings");
        assert!(matches!(set.source, Source::Preferences(_)));
        let split = set.resolve(&json!("Split Regions/Events at Playhead Position")).unwrap();
        assert_eq!((split.id, split.name.as_deref()), (574, Some("Split Regions/Events at Playhead Position")));
        assert_eq!(split.chord.as_ref().map(Chord::text).as_deref(), Some("⌘T"));
        assert_eq!(split.label(), "“Split Regions/Events at Playhead Position”");
        // Case and a trailing ellipsis don't matter; an id resolves as a number or as text.
        assert_eq!(set.resolve(&json!("select all")).unwrap().id, 766);
        assert_eq!(set.resolve(&json!("Select All…")).unwrap().chord.unwrap().text(), "⌘A");
        assert_eq!(set.resolve(&json!(513)).unwrap().name.as_deref(), Some("Create Marker"));
        assert_eq!(set.resolve(&json!("513")).unwrap().chord.unwrap().text(), "⌥'");
        assert_eq!(set.resolve(&json!(761)).unwrap().name.as_deref(), Some("Undo"));
        assert_eq!(set.resolve(&json!("Cycle Mode")).unwrap().chord.unwrap().text(), "C");
        assert_eq!(set.resolve(&json!("Delete Track")).unwrap().chord.unwrap().text(), "⌘delete");
        assert_eq!(set.resolve(&json!("New Audio Track")).unwrap().chord.unwrap().text(), "⌥⌘A");
        // Bound only in the factory preset (the preferences leave it out): the preset's binding stands.
        assert_eq!(set.resolve(&json!("Go to Left Locator")).unwrap().chord.unwrap().text(), "⌃⇧,");
        // Named, unbound in both: the command resolves without a chord.
        let locator = set.resolve(&json!("Set Left Locator by Playhead")).unwrap();
        assert_eq!((locator.id, locator.chord), (503, None));
        // An id without a name, bound: Cmd-S is Save.
        let save = set.resolve(&json!(754)).unwrap();
        assert_eq!((save.name, save.chord.unwrap().text()), (None, "⌘S".into()));
        // Unknown names are refused with the closest known ones.
        let refused = set.resolve(&json!("Split at Playhead")).unwrap_err();
        assert_eq!(refused.0, "bad-params");
        assert!(refused.1.contains("Split Regions/Events at Playhead Position"), "{}", refused.1);
        assert!(set.resolve(&json!("Zzzz")).unwrap_err().1.ends_with("“Zzzz”"));
        assert!(set.resolve(&json!(0)).is_err() && set.resolve(&json!(5000)).is_err() && set.resolve(&json!(true)).is_err());
        assert!(set.resolve(&json!("")).is_err());
    }

    #[test]
    fn the_factory_preset_alone_binds_what_the_preferences_would_and_nothing_reads_as_nothing() {
        let preset = fixture("us.logikcs.json");
        let blob = base64_decode(preset["LogicBinaryPreferences"].as_str().unwrap()).unwrap();
        let bindings = preset_bindings(&blob);
        assert!(bindings.len() > 800, "{}", bindings.len());
        let set = KeyCommands::from_parts(
            None,
            None,
            Some(&blob),
            Some(Path::new("/Applications/Logic Pro.app/Contents/Resources/Key Commands/U.S..logikcs")),
        );
        assert!(matches!(set.source, Source::Factory(_)));
        assert_eq!(set.resolve(&json!("Undo")).unwrap().chord.unwrap().text(), "⌘Z");
        assert_eq!(set.resolve(&json!("Create Marker")).unwrap().chord.unwrap().text(), "⌥'");
        // The recorded preferences agree with the preset on every binding they share but this Logic's own changes.
        let prefs = fixture("prefs.json");
        let mut differ = Vec::new();
        for (id, record) in prefs["KeyCommands"].as_object().unwrap() {
            let id: u32 = id.parse().unwrap();
            let number = |key: &str| record[key].as_u64().unwrap_or(0) as u32;
            let theirs = Chord::of_record(number("Key"), number("Modifier"), number("Flags"), number("CharCode"));
            let ours = bindings.iter().find(|(bound, _)| *bound == id).map(|(_, chord)| chord.clone());
            if theirs.is_some() && ours.is_some() && theirs != ours {
                differ.push((id, theirs, ours));
            }
        }
        assert!(differ.len() < 20, "{} bindings differ: {:?}", differ.len(), &differ[..differ.len().min(4)]);
        assert_eq!(preset_bindings(&[0u8; 100]), Vec::new());
        // The built-in table is the preset's, spelled as `parse` reads it.
        let built_in = KeyCommands::embedded();
        assert_eq!(built_in.source, Source::Factory(PathBuf::from("built in")));
        for (id, chord) in &bindings {
            assert_eq!(built_in.chord(*id), Some(chord), "id {id}");
        }
        assert_eq!(built_in.counts().1, bindings.len());
        assert_eq!(Chord::parse("82").unwrap(), Chord { key: Key::Code(82), shift: false, ctrl: false, opt: false, cmd: false });
        assert_eq!(Chord::parse(&Chord::of_record(8, 32, 0, 127).unwrap().spelled()).unwrap().text(), "⌘delete");
        let none = KeyCommands::from_parts(None, None, None, None);
        assert_eq!(none.source, Source::None);
        assert_eq!(none.resolve(&json!("Undo")).unwrap().chord, None);
        assert_eq!(none.name(761), Some("Undo"));
    }

    #[test]
    fn the_preset_is_found_by_the_name_the_preferences_keep_else_the_us_one() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Logic Pro.app");
        let folder = bundle.join("Contents/Resources/Key Commands");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("U.S..logikcs"), "").unwrap();
        std::fs::write(folder.join("German.logikcs"), "").unwrap();
        assert_eq!(
            preset_path(&bundle, Some("/Applications/Logic Pro X.app/Contents/Resources/Key Commands/German.logikcs")),
            Some(folder.join("German.logikcs"))
        );
        assert_eq!(preset_path(&bundle, Some("/gone/French.logikcs")), Some(folder.join("U.S..logikcs")));
        assert_eq!(preset_path(&bundle, None), Some(folder.join("U.S..logikcs")));
        assert_eq!(preset_path(&dir.path().join("Nothing.app"), None), None);
        assert_eq!(folded("Save As…"), "save as");
        assert_eq!(folded("  Select   All... "), "select all");
    }
}
