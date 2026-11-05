//! Pure parsing and resolution of Logic's key-binding data, isolated from M4a commit
//! `6f8f6e9bbc3666159ce68be1ce7466aa30e82670` (`crates/logic-kumi/src/keys.rs`).
//! Callers provide JSON preferences and decoded preset bytes. This module reads no files or preferences, starts no
//! process, posts no keys, and chooses no runtime fallback. `embedded` explicitly selects the recorded U.S. table.
//! The original data is retained byte-for-byte with hashes in `tests/fixtures/key-bindings/sources.json`.
//!
//! The preset ends in 5000 six-byte records indexed by command id: key, modifiers, reserved, character low/high,
//! flags. Modifier bits are shift=1, control=4, option=8, command=32; flags=1 selects a virtual key code.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// A malformed record or unresolved command, without a wire or UI dependency.
pub type ParseError = String;

fn bad(message: impl Into<String>) -> ParseError {
    message.into()
}
fn quoted(text: &str) -> String {
    format!("“{text}”")
}

/// The names, by id, as extracted from the Logic the bridge was built against.
const NAMES: &str = include_str!("key_bindings/names.json");
/// The recorded U.S. factory preset's bindings, by id, as the original `keys/extract_names.py` spells them.
const FACTORY: &str = include_str!("key_bindings/us-bindings.json");
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
        if modifier & !45 != 0 || flags & !1 != 0 {
            return None;
        }
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
    /// Codes below ten use `key:1` to distinguish them from a single digit character.
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
            Key::Code(code) if *code < 10 => format!("key:{code}"),
            Key::Code(code) => code.to_string(),
        });
        parts.join("-")
    }

    /// A chord written as the helper takes it, for the bridge's own fallbacks: `Chord::parse("cmd-t")`.
    pub fn parse(text: &str) -> Option<Chord> {
        // The hyphen is a key too: `cmd--` is Cmd-hyphen.
        let (mods, last) = if text == "-" {
            ("", "-")
        } else if let Some(mods) = text.strip_suffix("--") {
            if mods.is_empty() {
                return None;
            }
            (mods, "-")
        } else {
            text.rsplit_once('-').unwrap_or(("", text))
        };
        let parts: Vec<&str> = if mods.is_empty() { vec![] } else { mods.split('-').collect() };
        let key = match last {
            code if code.starts_with("key:") => {
                let code = code.strip_prefix("key:")?;
                if code.is_empty() || !code.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                Key::Code(code.parse().ok()?)
            }
            code if code.len() > 1 && code.bytes().all(|b| b.is_ascii_digit()) => Key::Code(code.parse().ok()?),
            k if k.len() == 1 && (b'!'..=b'~').contains(&k.as_bytes()[0]) => Key::Char(k.chars().next()?.to_ascii_lowercase()),
            "return" | "delete" | "enter" | "tab" | "escape" | "space" | "up" | "down" | "left" | "right" | "home" | "end" | "pageup"
            | "pagedown" | "forwarddelete" | "help" => Key::Named(NAMED.iter().find(|n| **n == last)?),
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
const NAMED: [&str; 16] = [
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
    "help",
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
            Some(name) => quoted(name),
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
    /// No source label was supplied (the unbound set has no bindings either).
    None,
}

/// A parsed key-command set with no connection to an app or a user's preferences.
#[derive(Debug, Clone)]
pub struct KeyCommands {
    /// The Logic version the names were extracted from.
    pub logic: String,
    names: HashMap<u32, String>,
    /// Lower-cased names without a trailing ellipsis, to their ids.
    by_name: HashMap<String, u32>,
    ambiguous: HashMap<String, Vec<u32>>,
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
    /// The recorded names alone, with every command unbound.
    pub fn unbound() -> KeyCommands {
        let value: Value = serde_json::from_str(NAMES).expect("key_bindings/names.json parses");
        let logic = value["logic"].as_str().unwrap_or_default().to_string();
        let mut names = HashMap::new();
        let mut by_name = HashMap::new();
        let mut ambiguous: HashMap<String, Vec<u32>> = HashMap::new();
        for (id, name) in value["commands"].as_object().into_iter().flatten() {
            let (Ok(id), Some(name)) = (id.parse::<u32>(), name.as_str()) else { continue };
            let label = folded(name);
            if let Some(before) = by_name.insert(label.clone(), id) {
                ambiguous.entry(label).or_insert_with(|| vec![before]).push(id);
            }
            names.insert(id, name.to_string());
        }
        KeyCommands { logic, names, by_name, ambiguous, bindings: HashMap::new(), source: Source::None }
    }

    /// Explicitly selects the recorded U.S. bindings. Parsing preferences never selects this fallback automatically.
    pub fn embedded() -> KeyCommands {
        let mut set = KeyCommands::unbound();
        let value: Value = serde_json::from_str(FACTORY).expect("key_bindings/us-bindings.json parses");
        for (id, spelled) in value["bindings"].as_object().into_iter().flatten() {
            if let (Ok(id), Some(chord)) = (id.parse::<u32>(), spelled.as_str().and_then(Chord::parse)) {
                set.bindings.insert(id, chord);
            }
        }
        set.source = Source::Factory(PathBuf::from("built in"));
        set
    }

    /// Parses a supplied `KeyCommands` dictionary over a supplied preset blob. Paths are provenance labels only;
    /// no path is read. Explicit preference records replace or unbind a preset entry; malformed records are errors.
    pub fn from_parts(
        prefs: Option<&Value>,
        prefs_path: Option<&Path>,
        preset: Option<&[u8]>,
        preset_path: Option<&Path>,
    ) -> Result<KeyCommands, ParseError> {
        let mut set = KeyCommands::unbound();
        if let Some(blob) = preset {
            set.bindings.extend(preset_bindings(blob)?);
            if let Some(path) = preset_path {
                set.source = Source::Factory(path.to_path_buf());
            }
        }
        if let Some(prefs) = prefs {
            let entries = prefs.as_object().ok_or_else(|| bad("KeyCommands must be an object"))?;
            for (spelled_id, record) in entries {
                let id = spelled_id
                    .parse::<u32>()
                    .ok()
                    .filter(|id| *id > 0 && *id < PRESET_IDS as u32)
                    .ok_or_else(|| bad("a key-command id must be a whole number from 1 to 4999"))?;
                if id.to_string() != *spelled_id {
                    return Err(bad("a key-command record id must use its canonical decimal spelling"));
                }
                let record = record.as_object().ok_or_else(|| bad(format!("key command {id} must be a record")))?;
                let number = |key: &str| {
                    record
                        .get(key)
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| bad(format!("key command {id}: {key} must be a whole number from 0 to {}", u32::MAX)))
                };
                let (key, modifier, flags, char_code) = (number("Key")?, number("Modifier")?, number("Flags")?, number("CharCode")?);
                if modifier & !45 != 0 || flags & !1 != 0 {
                    return Err(bad(format!("key command {id}: unsupported modifier or flag bits")));
                }
                match Chord::of_record(key, modifier, flags, char_code) {
                    Some(chord) => {
                        set.bindings.insert(id, chord);
                    }
                    None => {
                        set.bindings.remove(&id);
                    }
                }
            }
            if !entries.is_empty() {
                if let Some(path) = prefs_path {
                    set.source = Source::Preferences(path.to_path_buf());
                }
            }
        }
        Ok(set)
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
    pub fn resolve(&self, asked: &Value) -> Result<Command, ParseError> {
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

    fn by_name_or_close(&self, text: &str) -> Result<u32, ParseError> {
        let wanted = folded(text);
        if wanted.is_empty() {
            return Err(bad("command: a key command's name, as Logic Pro's Key Commands window lists it"));
        }
        if let Some(ids) = self.ambiguous.get(&wanted) {
            return Err(bad(format!(
                "Logic Pro has several key commands named {} (ids {}): use an id",
                quoted(text),
                ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
            )));
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
            .map(|(_, known)| quoted(self.names.get(&self.by_name[known]).map_or(known.as_str(), String::as_str)))
            .collect();
        if close.is_empty() {
            Err(bad(format!("Logic Pro has no key command named {}", quoted(text))))
        } else {
            Err(bad(format!("Logic Pro has no key command named {}; close: {}", quoted(text), close.join(", "))))
        }
    }
}

/// The bindings a preset's blob holds: its last 5000 records, one per id.
pub fn preset_bindings(blob: &[u8]) -> Result<Vec<(u32, Chord)>, ParseError> {
    let table = PRESET_IDS * RECORD;
    let base = blob.len().checked_sub(table).ok_or_else(|| bad("the preset has fewer than 5000 six-byte records"))?;
    let mut bindings = Vec::new();
    for id in 0..PRESET_IDS {
        let record = &blob[base + id * RECORD..base + (id + 1) * RECORD];
        if record.iter().all(|byte| *byte == 0) {
            continue;
        }
        if record[2] != 0 || record[1] & !45 != 0 || record[5] & !1 != 0 {
            return Err(bad(format!("key command {id}: unsupported preset record bits")));
        }
        let char_code = u32::from(record[3]) | (u32::from(record[4]) << 8);
        if let Some(chord) = Chord::of_record(u32::from(record[0]), u32::from(record[1]), u32::from(record[5]), char_code) {
            bindings.push((id as u32, chord));
        }
    }
    Ok(bindings)
}

/// Standard padded base64 with optional ASCII whitespace, for a supplied preset's data blob.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = text.bytes().filter(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n')).collect();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let digit = |byte| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, part) in bytes.chunks_exact(4).enumerate() {
        let (a, b) = (digit(part[0])?, digit(part[1])?);
        out.push(a << 2 | b >> 4);
        if part[2] == b'=' {
            if part[3] != b'=' || b & 15 != 0 || (index + 1) * 4 != bytes.len() {
                return None;
            }
            continue;
        }
        let c = digit(part[2])?;
        out.push(b << 4 | c >> 2);
        if part[3] == b'=' {
            if c & 3 != 0 || (index + 1) * 4 != bytes.len() {
                return None;
            }
            continue;
        }
        out.push(c << 6 | digit(part[3])?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "prefs.json" => include_str!("../tests/fixtures/key-bindings/prefs.json"),
            "us.logikcs.json" => include_str!("../tests/fixtures/key-bindings/us.logikcs.json"),
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(text).unwrap()
    }

    /// The recorded preferences over the U.S. factory preset, supplied as data rather than read from this Mac.
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
        .unwrap()
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
        for key in [Key::Code(0), Key::Code(1), Key::Code(9), Key::Code(82), Key::Char('1')] {
            let chord = Chord { key, shift: false, ctrl: false, opt: false, cmd: true };
            assert_eq!(Chord::parse(&chord.spelled()), Some(chord));
        }
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
        assert!(refused.contains("Split Regions/Events at Playhead Position"), "{refused}");
        assert!(set.resolve(&json!("Zzzz")).unwrap_err().ends_with("“Zzzz”"));
        assert!(set.resolve(&json!(0)).is_err() && set.resolve(&json!(5000)).is_err() && set.resolve(&json!(true)).is_err());
        assert!(set.resolve(&json!("")).is_err());
    }

    #[test]
    fn the_factory_preset_alone_binds_what_the_preferences_would_and_nothing_reads_as_nothing() {
        let preset = fixture("us.logikcs.json");
        let blob = base64_decode(preset["LogicBinaryPreferences"].as_str().unwrap()).unwrap();
        let bindings = preset_bindings(&blob).unwrap();
        assert!(bindings.len() > 800, "{}", bindings.len());
        let set = KeyCommands::from_parts(
            None,
            None,
            Some(&blob),
            Some(Path::new("/Applications/Logic Pro.app/Contents/Resources/Key Commands/U.S..logikcs")),
        )
        .unwrap();
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
        assert!(preset_bindings(&[0u8; 100]).is_err());
        // The built-in table is the preset's, spelled as `parse` reads it.
        let built_in = KeyCommands::embedded();
        assert_eq!(built_in.source, Source::Factory(PathBuf::from("built in")));
        for (id, chord) in &bindings {
            assert_eq!(built_in.chord(*id), Some(chord), "id {id}");
        }
        assert_eq!(built_in.counts().1, bindings.len());
        assert_eq!(Chord::parse("82").unwrap(), Chord { key: Key::Code(82), shift: false, ctrl: false, opt: false, cmd: false });
        assert_eq!(Chord::parse(&Chord::of_record(8, 32, 0, 127).unwrap().spelled()).unwrap().text(), "⌘delete");
        let none = KeyCommands::from_parts(None, None, None, None).unwrap();
        assert_eq!(none.source, Source::None);
        assert_eq!(none.resolve(&json!("Undo")).unwrap().chord, None);
        assert_eq!(none.name(761), Some("Undo"));
    }

    #[test]
    fn command_names_fold_case_whitespace_and_a_trailing_ellipsis() {
        assert_eq!(folded("Save As…"), "save as");
        assert_eq!(folded("  Select   All... "), "select all");
    }
    #[test]
    fn a_remap_and_an_explicit_unbinding_override_the_preset_without_guessing_a_fallback() {
        let preset = fixture("us.logikcs.json");
        let blob = base64_decode(preset["LogicBinaryPreferences"].as_str().unwrap()).unwrap();
        let prefs = json!({
            "574": {"Key":116,"Modifier":8,"Flags":0,"CharCode":116},
            "754": {"Key":0,"Modifier":0,"Flags":0,"CharCode":0}
        });
        let set = KeyCommands::from_parts(Some(&prefs), None, Some(&blob), None).unwrap();
        assert_eq!(set.chord(574).map(Chord::text).as_deref(), Some("⌥T"));
        assert_eq!(set.chord(754), None, "an explicit unbinding never inherits the factory Save chord");
        assert_eq!(set.chord(761).map(Chord::text).as_deref(), Some("⌘Z"), "an absent preference keeps its supplied preset binding");
        assert_eq!(KeyCommands::from_parts(None, None, None, None).unwrap().chord(574), None);
        assert!(
            set.resolve(&json!("Create Chord")).unwrap_err().contains("use an id"),
            "the recorded table has two commands with that label"
        );
        assert_eq!(set.resolve(&json!(330)).unwrap().id, 330);
        assert_eq!(set.resolve(&json!(3332)).unwrap().id, 3332);
    }

    #[test]
    fn malformed_records_and_base64_are_refused_instead_of_becoming_another_binding() {
        for prefs in [
            json!([]),
            json!({"574":{}}),
            json!({"574":null}),
            json!({"0574":{"Key":116,"Modifier":32,"Flags":0,"CharCode":116},"574":{"Key":116,"Modifier":8,"Flags":0,"CharCode":116}}),
            json!({"+574":{"Key":116,"Modifier":32,"Flags":0,"CharCode":116}}),
            json!({"5000":{"Key":116,"Modifier":32,"Flags":0,"CharCode":116}}),
            json!({"574":{"Key":4294967412u64,"Modifier":32,"Flags":0,"CharCode":116}}),
            json!({"574":{"Key":116,"Modifier":32.5,"Flags":0,"CharCode":116}}),
            json!({"574":{"Key":116,"Modifier":34,"Flags":0,"CharCode":116}}),
            json!({"574":{"Key":116,"Modifier":32,"Flags":2,"CharCode":116}}),
        ] {
            assert!(KeyCommands::from_parts(Some(&prefs), None, None, None).is_err(), "{prefs}");
        }
        let mut blob = vec![0u8; PRESET_IDS * RECORD];
        // One literal command record with an unknown reserved byte must not shift or reinterpret the table.
        blob[574 * RECORD..575 * RECORD].copy_from_slice(&[116, 32, 1, 116, 0, 0]);
        assert!(preset_bindings(&blob).is_err());
        assert!(preset_bindings(&blob[..blob.len() - 1]).is_err());
        for bad in ["A", "AA", "AAA", "A===", "=AAA", "AA=A", "AA==AAAA", "AB==", "AAB=", "AAAA*==="] {
            assert_eq!(base64_decode(bad), None, "{bad}");
        }
        assert_eq!(base64_decode(" Zg== \n"), Some(b"f".to_vec()));
        assert_eq!(base64_decode("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(base64_decode("Zm9v"), Some(b"foo".to_vec()));
        assert_eq!(Chord::parse("cmd-meta-x"), None);
        assert_eq!(Chord::parse("cmd--x"), None);
        assert_eq!(Chord::parse("cmd---"), None);
        assert_eq!(Chord::parse("--"), None);
        assert_eq!(Chord::parse("key:"), None);
        assert_eq!(Chord::parse("key:+1"), None);
        assert_eq!(Chord::parse("key:65536"), None);
        assert_eq!(Chord::parse("é"), None);
        assert_eq!(Chord::parse("help"), Chord::of_record(0, 0, 0, 0xF746));
    }

    #[test]
    fn the_original_tables_and_recordings_keep_their_source_hashes() {
        let sources: Value = serde_json::from_str(include_str!("../tests/fixtures/key-bindings/sources.json")).unwrap();
        assert_eq!(sources["commit"], "6f8f6e9bbc3666159ce68be1ce7466aa30e82670");
        assert_eq!(sources["files"].as_array().unwrap().len(), 4);
        for entry in sources["files"].as_array().unwrap() {
            let bytes: &[u8] = match entry["path"].as_str().unwrap() {
                "src/key_bindings/names.json" => NAMES.as_bytes(),
                "src/key_bindings/us-bindings.json" => FACTORY.as_bytes(),
                "tests/fixtures/key-bindings/prefs.json" => include_bytes!("../tests/fixtures/key-bindings/prefs.json"),
                "tests/fixtures/key-bindings/us.logikcs.json" => include_bytes!("../tests/fixtures/key-bindings/us.logikcs.json"),
                path => panic!("unknown source path {path}"),
            };
            assert_eq!(daw_bridge::auth::sha256_hex(bytes), entry["sha256"].as_str().unwrap(), "{}", entry["source"]);
        }
    }
}
