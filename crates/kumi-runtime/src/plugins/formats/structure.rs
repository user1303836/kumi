//! The data in each plug-in's folder (`plugin-formats/<plug-in>/`): `plugin.json` (vendor, plug-in IDs,
//! preset extension and folders) and, per format, `structure.json` (the wrapping, every field's type, unit,
//! range and option meanings, and which host parameter moves it) and `verified.json` (the plug-in versions
//! checked against it). Every meaning and link says whether it was verified or guessed. Kumi carries the
//! folders it ships with (`builtin`), so the reader, a later writer and the tests all run off the same data.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::survey::SurveyReport;
use super::tree::{Node, PathRule};
use super::FormatError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Checked against the plug-in: by changing it and reading what moved, or from its published source.
    Verified,
    /// Inferred from names, values and how often they appear.
    Guessed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginId {
    pub id: String,
    /// The name hosts show for it.
    pub name: String,
    pub status: Status,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginIds {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vst3: Vec<PluginId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vst2: Vec<PluginId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub au: Vec<PluginId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clap: Vec<PluginId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    /// "~" for the home folder, `%APPDATA%`-style variables on Windows.
    pub path: String,
    /// What it holds ("factory presets", "user presets", "wavetables").
    pub holds: String,
    pub status: Status,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetPlaces {
    pub extensions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Folder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mac: Vec<Folder>,
}

/// `plugin.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Its folder's name ("serum-2").
    pub plugin: String,
    pub name: String,
    pub vendor: String,
    /// "instrument" or "effect".
    pub kind: String,
    pub ids: PluginIds,
    pub presets: PresetPlaces,
    /// Its format folders, oldest first ("format-1").
    pub formats: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One kind of file a format covers ("preset", "processor", "state").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileKind {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// How the bytes are wrapped, outermost first.
    pub layers: Vec<String>,
    /// Values the reader checks (a magic, an encoding number), so code and data can't drift apart.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub constants: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    #[serde(rename = "type")]
    pub kind: String,
    /// The kinds of file it appears in.
    #[serde(rename = "in", default)]
    pub seen_in: Vec<String>,
    /// How many surveyed files had it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
    /// The smallest and largest value the survey saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<[f64; 2]>,
    /// The range the plug-in allows, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// An option list's values, each with its meaning ("" while unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    pub status: Status,
}

/// A host parameter linked to the field it moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterLink {
    pub field: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `structure.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Structure {
    pub plugin: String,
    pub format: String,
    pub about: String,
    /// How tree keys become field paths.
    pub paths: PathRule,
    pub files: BTreeMap<String, FileKind>,
    /// Where the survey behind `files` and `seen` came from.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub survey: Value,
    pub fields: BTreeMap<String, Field>,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifiedVersion {
    /// The plug-in's version ("2.1.5").
    pub version: String,
    /// The kinds of file checked ("preset", "processor").
    pub files: Vec<String>,
    /// What was done to check it.
    pub how: String,
    /// When (YYYY-MM-DD).
    pub checked: String,
}

/// `verified.json`: the plug-in versions this format was checked against. Kumi writes only for these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verified {
    pub plugin: String,
    pub format: String,
    /// What reading and writing each cover so far.
    pub reads: Vec<VerifiedVersion>,
    #[serde(default)]
    pub writes: Vec<VerifiedVersion>,
}

impl Verified {
    /// Whether Kumi may write this kind of file for this plug-in version (only what a check covered).
    pub fn can_write(&self, version: &str, file: &str) -> bool {
        self.writes.iter().any(|v| v.version == version && v.files.iter().any(|f| f == file))
    }

    pub fn was_read(&self, version: &str, file: &str) -> bool {
        self.reads.iter().any(|v| v.version == version && v.files.iter().any(|f| f == file))
    }
}

/// A plug-in's folder: its info and one format's structure and checks.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginFormat {
    pub info: PluginInfo,
    pub structure: Structure,
    pub verified: Verified,
}

/// What a tree has that its structure doesn't know, or knows as another type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Check {
    pub unknown: BTreeSet<String>,
    /// Path → (the structure's type, the tree's).
    pub mismatched: BTreeMap<String, (String, String)>,
}

impl Check {
    pub fn is_clean(&self) -> bool {
        self.unknown.is_empty() && self.mismatched.is_empty()
    }
}

/// A leaf of a tree with the field it is, when known.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading<'a> {
    pub path: String,
    pub value: Node,
    pub field: Option<&'a Field>,
}

impl Structure {
    pub fn field(&self, path: &str) -> Option<&Field> {
        self.fields.get(path)
    }

    /// The field a host parameter moves, by the parameter's name.
    pub fn parameter(&self, name: &str) -> Option<(&ParameterLink, Option<&Field>)> {
        self.parameters.get(name).map(|link| (link, self.fields.get(&link.field)))
    }

    /// Every path in the tree the structure doesn't list, and every one listed as another type. An int where a
    /// float is listed is fine (encoders write whole floats as ints), and so is null for an optional value.
    pub fn check(&self, tree: &Node) -> Check {
        let mut check = Check::default();
        tree.walk(self.paths, &mut |path, node| {
            if path.is_empty() {
                return;
            }
            match self.fields.get(path) {
                None => {
                    check.unknown.insert(path.to_string());
                }
                Some(field) => {
                    let found = node.kind();
                    let fits = field.kind == found || (field.kind == "float" && found == "int") || found == "null" || field.kind == "any";
                    if !fits {
                        check.mismatched.insert(path.to_string(), (field.kind.clone(), found.to_string()));
                    }
                }
            }
        });
        check
    }

    /// The tree's leaves, each with its field.
    pub fn describe(&self, tree: &Node) -> Vec<Reading<'_>> {
        let mut readings = Vec::new();
        tree.walk(self.paths, &mut |path, node| {
            if !node.is_container() && !path.is_empty() {
                readings.push(Reading { path: path.to_string(), value: node.clone(), field: self.fields.get(path) });
            }
        });
        readings
    }
}

impl PluginFormat {
    /// A folder's files, from their text.
    pub fn parse(plugin: &str, structure: &str, verified: &str) -> Result<PluginFormat, FormatError> {
        let fail = |file: &str, error: serde_json::Error| FormatError::new(format!("{file}: {error}"));
        Ok(PluginFormat {
            info: serde_json::from_str(plugin).map_err(|e| fail("plugin.json", e))?,
            structure: serde_json::from_str(structure).map_err(|e| fail("structure.json", e))?,
            verified: serde_json::from_str(verified).map_err(|e| fail("verified.json", e))?,
        })
    }

    /// Whether a plug-in ID (VST3 class ID, VST2 unique ID, AU or CLAP ID) is this plug-in's.
    pub fn has_id(&self, id: &str) -> bool {
        let ids = &self.info.ids;
        [&ids.vst3, &ids.vst2, &ids.au, &ids.clap].into_iter().flatten().any(|known| known.id.eq_ignore_ascii_case(id))
    }
}

macro_rules! folder {
    ($plugin:literal, $format:literal) => {
        (
            $plugin,
            include_str!(concat!("../../../../../plugin-formats/", $plugin, "/plugin.json")),
            include_str!(concat!("../../../../../plugin-formats/", $plugin, "/", $format, "/structure.json")),
            include_str!(concat!("../../../../../plugin-formats/", $plugin, "/", $format, "/verified.json")),
        )
    };
}

/// The folders Kumi ships with, as text: plug-in, plugin.json, structure.json, verified.json.
pub const BUILTIN_FILES: [(&str, &str, &str, &str); 3] =
    [folder!("serum-2", "format-1"), folder!("vital", "format-1"), folder!("ozone-12", "format-1")];

static BUILTIN: LazyLock<Vec<PluginFormat>> = LazyLock::new(|| {
    BUILTIN_FILES
        .iter()
        .map(|(plugin, info, structure, verified)| {
            PluginFormat::parse(info, structure, verified).unwrap_or_else(|error| panic!("plugin-formats/{plugin}: {error}"))
        })
        .collect()
});

/// Every plug-in format Kumi ships with.
pub fn builtins() -> &'static [PluginFormat] {
    &BUILTIN
}

/// A shipped plug-in's format by its folder name ("serum-2").
pub fn builtin(plugin: &str) -> Option<&'static PluginFormat> {
    BUILTIN.iter().find(|format| format.info.plugin == plugin)
}

/// A shipped plug-in's format by a plug-in ID a host reports.
pub fn for_id(id: &str) -> Option<&'static PluginFormat> {
    BUILTIN.iter().find(|format| format.has_id(id))
}

/// A survey folded into a structure file's JSON, keeping everything written by hand: each field's type,
/// file kinds, count, seen range and option values come from the survey; its unit, range, meanings, notes
/// and status stay. New fields arrive as guessed. Fields are kept sorted by path.
pub fn merge_survey(structure: &mut Value, report: &SurveyReport, survey: Value) {
    let old = structure.get("fields").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut fields: BTreeMap<String, Value> = old.into_iter().collect();
    for (path, found) in &report.fields {
        let entry = fields.entry(path.clone()).or_insert_with(|| json!({"status": "guessed"}));
        let object = entry.as_object_mut().expect("a field is an object");
        object.insert("type".into(), json!(found.kind));
        let mut seen_in: BTreeSet<String> =
            object.get("in").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        seen_in.extend(found.seen_in.iter().cloned());
        object.insert("in".into(), json!(seen_in));
        object.insert("files".into(), json!(found.files));
        match found.seen {
            Some(seen) => object.insert("seen".into(), json!(seen)),
            None => object.remove("seen"),
        };
        if found.options {
            let mut options = object.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
            for value in found.values.keys() {
                options.entry(value.clone()).or_insert(json!(""));
            }
            object.insert("options".into(), Value::Object(options));
        }
    }
    let sorted: serde_json::Map<String, Value> = fields.into_iter().map(|(path, field)| (path, order_field(field))).collect();
    if let Some(object) = structure.as_object_mut() {
        object.insert("survey".into(), survey);
        object.insert("fields".into(), Value::Object(sorted));
    }
}

/// A field's keys in a fixed order, so diffs stay small.
fn order_field(field: Value) -> Value {
    const ORDER: [&str; 10] = ["type", "in", "files", "seen", "range", "unit", "options", "about", "status", "note"];
    let Value::Object(mut object) = field else { return field };
    let mut ordered = serde_json::Map::new();
    for key in ORDER {
        if let Some(value) = object.remove(key) {
            ordered.insert(key.into(), value);
        }
    }
    ordered.extend(object);
    Value::Object(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::formats::survey::Survey;

    #[test]
    fn checks_and_describes_a_tree() {
        let structure: Structure = serde_json::from_value(json!({
            "plugin": "x", "format": "format-1", "about": "", "paths": "numbered-sections",
            "files": {"preset": {"extension": ".x", "layers": ["JSON"]}},
            "fields": {
                "Osc{n}": {"type": "map", "status": "guessed"},
                "Osc{n}/level": {"type": "float", "unit": "%", "status": "verified"},
                "Osc{n}/mode": {"type": "text", "options": {"kSaw": "saw"}, "status": "guessed"}
            },
            "parameters": {"A Level": {"field": "Osc{n}/level", "status": "guessed"}}
        }))
        .unwrap();
        let tree = Node::from(&json!({"Osc0": {"level": 1, "mode": 3, "extra": true}}));
        let check = structure.check(&tree);
        assert_eq!(check.unknown.iter().collect::<Vec<_>>(), ["Osc{n}/extra"]);
        assert_eq!(check.mismatched["Osc{n}/mode"], ("text".to_string(), "int".to_string()));
        let readings = structure.describe(&tree);
        assert_eq!(readings[0].field.and_then(|f| f.unit.as_deref()), Some("%"));
        assert!(readings[2].field.is_none());
        assert_eq!(structure.parameter("A Level").unwrap().1.unwrap().status, Status::Verified);
    }

    #[test]
    fn a_survey_merges_without_losing_hand_written_meaning() {
        let mut survey = Survey::new(PathRule::Exact);
        for mode in ["kSaw", "kSine", "kSaw", "kSine"] {
            survey.add(&Node::from(&json!({"mode": mode, "level": 0.5})), "preset");
        }
        let mut structure =
            json!({"fields": {"mode": {"status": "verified", "about": "the wave", "options": {"kSaw": "saw"}, "type": "int"}}});
        merge_survey(&mut structure, &survey.report(), json!({"files": 4}));
        assert_eq!(
            structure["fields"]["mode"],
            json!({"type": "text", "in": ["preset"], "files": 4, "options": {"kSaw": "saw", "kSine": ""}, "about": "the wave", "status": "verified"})
        );
        assert_eq!(
            structure["fields"]["level"],
            json!({"type": "float", "in": ["preset"], "files": 4, "seen": [0.5, 0.5], "status": "guessed"})
        );
        assert_eq!(structure["survey"]["files"], 4);
        assert_eq!(structure["fields"].as_object().unwrap().keys().collect::<Vec<_>>(), ["level", "mode"]);
    }

    #[test]
    fn writing_needs_a_checked_version() {
        let verified: Verified = serde_json::from_value(json!({
            "plugin": "x", "format": "format-1",
            "reads": [{"version": "2.1.5", "files": ["preset"], "how": "", "checked": "2026-10-07"}],
            "writes": []
        }))
        .unwrap();
        assert!(verified.was_read("2.1.5", "preset"));
        assert!(!verified.was_read("2.1.6", "preset"));
        assert!(!verified.can_write("2.1.5", "preset"));
    }
}
