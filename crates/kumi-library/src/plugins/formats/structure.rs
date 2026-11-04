//! The data in each plug-in's folder (`plugin-formats/<plug-in>/`): `plugin.json` (vendor, plug-in IDs,
//! preset extension and folders) and, per format, `structure.json` (the wrapping, every field's type, unit,
//! range and option meanings, and which host parameter moves it) and `verified.json` (the plug-in versions
//! checked against it). Every meaning and link says whether it was verified or guessed. Kumi carries the
//! folders it ships with (`builtin`), so the reader, a later writer and the tests all run off the same data.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{Arc, LazyLock};

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
    /// How many parameters it lists to a host (Live's get_parameter_names), when counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<u32>,
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
    /// Shared format directory beneath plugin-formats, including a variant when
    /// this product has different defaults or host identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_source: Option<String>,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub binaries: Vec<String>,
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
    /// Its kind ("float", "text", "map" ...), or kinds joined by "|" when it takes more than one ("map|text").
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
    /// The supported, verified numeric write interval; it may be deliberately
    /// narrower than the plug-in's complete UI range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Bounds have their own evidence; a guessed control label does not make a
    /// host-reported numeric domain uncertain, or an observed value a bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_status: Option<Status>,
    /// Products whose own binary supplied this field's numeric range evidence.
    /// Shared schemas require this list so a parent-only addition cannot grant
    /// numeric writes to every product inheriting that schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub range_products: Vec<String>,
    /// Verified spacing of discrete values, starting at the lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Its value when the file leaves it out (Serum and Ozone write only what differs from it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// An option list's values, each with its meaning ("" while unknown).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "shared_options")]
    pub options: Option<Arc<BTreeMap<String, String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    pub status: Status,
}

/// A host parameter linked to the field it moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterLink {
    pub field: String,
    /// The numbers that fill the field path's "{n}"s, in order ([1] for "Env{n}/..." is Env1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<u32>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `structure.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StructureSource")]
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
    /// Every numeric write needs explicit per-field product coverage when a
    /// schema is shared by products with separately captured host evidence.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub range_scope_required: bool,
    /// Preserved raw subtrees whose attributes are derived from the file rather
    /// than duplicated in this control schema. Their descendants are read-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opaque: Vec<String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterLink>,
}

// The source may share repetitive metadata; the public Structure and Field
// serializers keep the expanded catalog shape used by existing callers.
#[derive(Deserialize)]
struct StructureSource {
    plugin: String,
    format: String,
    about: String,
    paths: PathRule,
    files: BTreeMap<String, FileKind>,
    #[serde(default)]
    survey: Value,
    fields: BTreeMap<String, Value>,
    #[serde(default)]
    range_scope_required: bool,
    #[serde(default)]
    opaque: Vec<String>,
    #[serde(default)]
    parameters: BTreeMap<String, ParameterLink>,
    #[serde(default)]
    field_defaults: BTreeMap<String, FieldDefaults>,
    #[serde(default)]
    option_tables: BTreeMap<String, BTreeMap<String, String>>,
}

/// Defaults may describe units and file membership, never grant write evidence.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldDefaults {
    #[serde(default)]
    unit: Option<String>,
    #[serde(rename = "in", default)]
    seen_in: Option<Vec<String>>,
}

impl TryFrom<StructureSource> for Structure {
    type Error = String;

    fn try_from(source: StructureSource) -> Result<Self, Self::Error> {
        let tables: BTreeMap<_, _> = source.option_tables.into_iter().map(|(name, options)| (name, Arc::new(options))).collect();
        let mut fields = BTreeMap::new();
        for (path, mut value) in source.fields {
            let object = value.as_object_mut().ok_or_else(|| format!("field {path} must be an object"))?;
            if let Some(defaults) = object.get("type").and_then(Value::as_str).and_then(|kind| source.field_defaults.get(kind)) {
                if let Some(unit) = &defaults.unit {
                    object.entry("unit").or_insert_with(|| json!(unit));
                }
                if let Some(seen_in) = &defaults.seen_in {
                    object.entry("in").or_insert_with(|| json!(seen_in));
                }
            }
            let shared = match object.remove("options_ref") {
                Some(reference) => {
                    if object.contains_key("options") {
                        return Err(format!("field {path} has both options and options_ref"));
                    }
                    let reference = reference.as_str().ok_or_else(|| format!("field {path} options_ref must name a table"))?;
                    Some(Arc::clone(tables.get(reference).ok_or_else(|| format!("field {path} has unknown option table {reference}"))?))
                }
                None => None,
            };
            let mut field: Field = serde_json::from_value(value).map_err(|error| format!("field {path}: {error}"))?;
            if shared.is_some() {
                field.options = shared;
            }
            fields.insert(path, field);
        }
        Ok(Self {
            plugin: source.plugin,
            format: source.format,
            about: source.about,
            paths: source.paths,
            files: source.files,
            survey: source.survey,
            fields,
            range_scope_required: source.range_scope_required,
            opaque: source.opaque,
            parameters: source.parameters,
        })
    }
}

mod shared_options {
    use super::*;

    pub fn serialize<S: serde::Serializer>(options: &Option<Arc<BTreeMap<String, String>>>, serializer: S) -> Result<S::Ok, S::Error> {
        options.as_deref().serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Arc<BTreeMap<String, String>>>, D::Error> {
        Option::<BTreeMap<String, String>>::deserialize(deserializer).map(|options| options.map(Arc::new))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifiedVersion {
    /// The plug-in's version ("2.1.5").
    pub version: String,
    /// Other version strings observed in this same release's OS metadata.
    /// These are product-specific aliases, not a rule for reinterpreting decimals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_aliases: Vec<String>,
    /// The kinds of file checked ("preset", "processor").
    pub files: Vec<String>,
    /// What was done to check it.
    pub how: String,
    /// When (YYYY-MM-DD).
    pub checked: String,
    /// Exact initialized binary whose public factory reported this version, for
    /// products which ship neither moduleinfo nor OS version metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha256: Option<String>,
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
    /// Path -> (the structure's type, the tree's).
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

    /// Validate a numeric edit against this product's verified write interval.
    pub fn validate_number(&self, path: &str, value: f64) -> Result<(), FormatError> {
        let plugin = &self.plugin;
        let field = self.field(path).ok_or_else(|| FormatError::new(format!("Unknown numeric field {plugin}/{path}")))?;
        if (self.range_scope_required || !field.range_products.is_empty()) && !field.range_products.contains(plugin) {
            return Err(FormatError::new(format!("{plugin}/{path} has no range evidence for this product")));
        }
        let [low, high] = field
            .range
            .filter(|_| field.range_status == Some(Status::Verified))
            .ok_or_else(|| FormatError::new(format!("{plugin}/{path} has no verified writable range")))?;
        if !value.is_finite() || !low.is_finite() || !high.is_finite() || low > high || value < low || value > high {
            return Err(FormatError::new(format!("{plugin}/{path} must be between {low} and {high}")));
        }
        if let Some(step) = field.step {
            let position = (value - low) / step;
            if !step.is_finite() || step <= 0.0 || !position.is_finite() || (position - position.round()).abs() > 1e-6 {
                return Err(FormatError::new(format!("{plugin}/{path} requires steps of {step} from {low}")));
            }
        }
        Ok(())
    }

    /// The field a host parameter moves, by the parameter's name.
    pub fn parameter(&self, name: &str) -> Option<(&ParameterLink, Option<&Field>)> {
        self.parameters.get(name).map(|link| (link, self.fields.get(&link.field)))
    }

    /// Where a linked parameter's value sits in a tree: its field's path with each "{n}" filled from `at`
    /// ("Env{n}/plainParams/kParamAttack" at [0] is "Env0/plainParams/kParamAttack"; Vital's
    /// "settings/osc_{n}_level" at [1] is "settings/osc_1_level").
    pub fn location(link: &ParameterLink) -> String {
        let mut numbers = link.at.iter();
        let mut out = String::new();
        let mut rest = link.field.as_str();
        while let Some(i) = rest.find("{n}") {
            out.push_str(&rest[..i]);
            match numbers.next() {
                Some(n) => out.push_str(&n.to_string()),
                None => out.push_str("{n}"),
            }
            rest = &rest[i + 3..];
        }
        out.push_str(rest);
        out
    }

    /// Every path in the tree the structure doesn't list, and every one listed as another type. An int where a
    /// float is listed is fine (encoders write whole floats as ints), and so is null for an optional value.
    pub fn check(&self, tree: &Node) -> Check {
        let mut check = Check::default();
        tree.walk(self.paths, &mut |path, node| {
            if path.is_empty()
                || self
                    .opaque
                    .iter()
                    .any(|prefix| path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/') || rest.starts_with('[')))
            {
                return;
            }
            match self.fields.get(path) {
                None => {
                    check.unknown.insert(path.to_string());
                }
                Some(field) => {
                    let found = node.kind();
                    let fits = found == "null"
                        || field.kind.split('|').any(|kind| kind == found || kind == "any" || (kind == "float" && found == "int"));
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
            structure: serde_json::from_value(resolve_structure(structure, 0)?).map_err(|e| fail("structure.json", e))?,
            verified: serde_json::from_str(verified).map_err(|e| fail("verified.json", e))?,
        })
    }

    /// Whether a plug-in ID (VST3 class ID, VST2 unique ID, AU or CLAP ID) is this plug-in's.
    pub fn has_id(&self, id: &str) -> bool {
        let ids = &self.info.ids;
        [&ids.vst3, &ids.vst2, &ids.au, &ids.clap].into_iter().flatten().any(|known| known.id.eq_ignore_ascii_case(id))
    }
}

impl PluginInfo {
    pub fn format_directory(&self) -> String {
        self.format_source.clone().unwrap_or_else(|| format!("{}/{}", self.plugin, self.formats[0]))
    }
}

fn resolve_structure(text: &str, depth: usize) -> Result<Value, FormatError> {
    if depth > 8 {
        return Err(FormatError::new("format inheritance is cyclic or too deep"));
    }
    let mut patch: Value = serde_json::from_str(text).map_err(|error| FormatError::new(format!("structure.json: {error}")))?;
    let parent = patch.as_object_mut().and_then(|object| object.remove("extends"));
    let Some(parent) = parent else { return Ok(patch) };
    let parent = parent.as_str().ok_or_else(|| FormatError::new("format parent must name a built-in product"))?;
    let source = BUILTIN_FILES
        .iter()
        .find(|(plugin, _, _, _)| *plugin == parent)
        .ok_or_else(|| FormatError::new(format!("unknown shared format {parent}")))?
        .2;
    let mut base = resolve_structure(&bundled_json(source), depth + 1)?;
    merge_format_patch(&mut base, &patch);
    Ok(base)
}

/// JSON merge-patch semantics used for a shared schema's product differences.
/// Null removes an inherited key; all identity and version metadata remains in
/// the product's own plugin.json and verified.json.
pub fn merge_format_patch(base: &mut Value, patch: &Value) {
    if let Some(patch) = patch.as_object() {
        if !base.is_object() {
            *base = json!({});
        }
        let base = base.as_object_mut().unwrap();
        for (key, value) in patch {
            if value.is_null() {
                base.remove(key);
            } else {
                merge_format_patch(base.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
    } else {
        *base = patch.clone();
    }
}

// Generated from readable product folders, compacted and compressed once per
// source file. The bounded decoder is for our bundled resources, not user files.
include!(concat!(env!("OUT_DIR"), "/plugin_formats.rs"));

fn bundled_json(bytes: &[u8]) -> String {
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .take(super::MAX_DECODED_BYTES as u64 + 1)
        .read_to_end(&mut decoded)
        .expect("bundled format decompresses");
    assert!(decoded.len() <= super::MAX_DECODED_BYTES, "bundled format exceeds its size limit");
    String::from_utf8(decoded).expect("bundled format is UTF-8")
}

static BUILTIN: LazyLock<Vec<PluginFormat>> = LazyLock::new(|| {
    BUILTIN_FILES
        .iter()
        .map(|(plugin, info, structure, verified)| {
            PluginFormat::parse(&bundled_json(info), &bundled_json(structure), &bundled_json(verified))
                .unwrap_or_else(|error| panic!("plugin-formats/{plugin}: {error}"))
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

/// Numeric writes require a proven domain. `seen` is deliberately never used.
pub fn validate_number(plugin: &str, path: &str, value: f64) -> Result<(), FormatError> {
    builtin(plugin).ok_or_else(|| FormatError::new(format!("Unknown plug-in {plugin}")))?.structure.validate_number(path, value)
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
        // A field seen as more than one kind ("default" or a map of knobs) lists them all: "map|text".
        let kind = if found.kinds.len() > 1 {
            found.kinds.keys().filter(|kind| *kind != "null").cloned().collect::<Vec<_>>().join("|")
        } else {
            found.kind.clone()
        };
        object.insert("type".into(), json!(kind));
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

/// A structure file's text: two-space JSON, but each field and parameter link on one line, so the file stays
/// short and a change to one field is a one-line diff.
pub fn to_text(structure: &Value) -> String {
    let Value::Object(top) = structure else { return serde_json::to_string_pretty(structure).unwrap_or_default() };
    let mut out = String::from("{\n");
    for (i, (key, value)) in top.iter().enumerate() {
        let comma = if i + 1 < top.len() { "," } else { "" };
        let one_line_entries = matches!(key.as_str(), "fields" | "parameters");
        match value {
            Value::Object(entries) if one_line_entries && !entries.is_empty() => {
                out.push_str(&format!("  {}: {{\n", Value::String(key.clone())));
                for (j, (name, entry)) in entries.iter().enumerate() {
                    let entry_comma = if j + 1 < entries.len() { "," } else { "" };
                    out.push_str(&format!("    {}: {}{entry_comma}\n", Value::String(name.clone()), entry));
                }
                out.push_str(&format!("  }}{comma}\n"));
            }
            _ => {
                let text = serde_json::to_string_pretty(value).unwrap_or_default().replace('\n', "\n  ");
                out.push_str(&format!("  {}: {text}{comma}\n", Value::String(key.clone())));
            }
        }
    }
    out.push_str("}\n");
    out
}

/// A field's keys in a fixed order, so diffs stay small.
fn order_field(field: Value) -> Value {
    const ORDER: [&str; 11] = ["type", "in", "files", "seen", "range", "default", "unit", "options", "about", "status", "note"];
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

    fn shared_source() -> Value {
        json!({
            "plugin": "x", "format": "format-1", "about": "", "paths": "exact", "files": {},
            "field_defaults": {"float": {"unit": "stored value", "in": ["state", "preset"]}},
            "option_tables": {"toggle": {"0": "Off", "1": "On"}},
            "fields": {
                "a": {"type": "float", "status": "verified", "range": [0, 1], "range_status": "verified", "options_ref": "toggle"},
                "b": {"type": "float", "status": "verified", "range": [0, 1], "range_status": "verified", "options_ref": "toggle"},
                "c": {"type": "float", "status": "guessed", "seen": [0, 1], "unit": null, "in": []},
                "map": {"type": "map", "status": "verified"}
            }
        })
    }

    #[test]
    fn shared_metadata_keeps_the_expanded_catalog_and_write_evidence() {
        let structure: Structure = serde_json::from_value(shared_source()).unwrap();
        let a = &structure.fields["a"];
        let b = &structure.fields["b"];
        assert!(Arc::ptr_eq(a.options.as_ref().unwrap(), b.options.as_ref().unwrap()));
        assert_eq!(a.unit.as_deref(), Some("stored value"));
        assert_eq!(a.seen_in, ["state", "preset"]);
        assert_eq!(structure.fields["c"].unit, None);
        assert!(structure.fields["c"].seen_in.is_empty());
        assert_eq!(structure.fields["map"].unit, None);
        assert!(structure.fields["map"].seen_in.is_empty());
        assert!(structure.validate_number("a", 1.0).is_ok());
        assert!(structure.validate_number("a", 2.0).is_err());
        assert!(structure.validate_number("c", 0.5).is_err());

        let public = serde_json::to_value(&structure).unwrap();
        assert_eq!(public["fields"]["a"]["options"], json!({"0": "Off", "1": "On"}));
        assert!(public.get("option_tables").is_none());
        assert!(public["fields"]["a"].get("options_ref").is_none());
        assert_eq!(serde_json::from_value::<Structure>(public).unwrap(), structure);
    }

    #[test]
    fn inherited_tables_and_defaults_resolve_after_product_overrides() {
        let mut source = shared_source();
        merge_format_patch(
            &mut source,
            &json!({"plugin": "child", "field_defaults": {"float": {"unit": "%"}}, "option_tables": {"toggle": {"1": "Enabled"}}}),
        );
        let structure: Structure = serde_json::from_value(source).unwrap();
        let a = &structure.fields["a"];
        let b = &structure.fields["b"];
        assert_eq!(a.unit.as_deref(), Some("%"));
        assert_eq!(a.options.as_ref().unwrap()["1"], "Enabled");
        assert!(Arc::ptr_eq(a.options.as_ref().unwrap(), b.options.as_ref().unwrap()));
        assert_eq!(a.range, Some([0.0, 1.0]));
    }

    #[test]
    fn shared_metadata_rejects_missing_or_ambiguous_options_and_default_write_evidence() {
        let mut missing = shared_source();
        missing["fields"]["a"]["options_ref"] = json!("missing");
        assert!(serde_json::from_value::<Structure>(missing).unwrap_err().to_string().contains("unknown option table missing"));
        let mut ambiguous = shared_source();
        ambiguous["fields"]["a"]["options"] = json!({"0": "Disabled"});
        assert!(serde_json::from_value::<Structure>(ambiguous).unwrap_err().to_string().contains("both options and options_ref"));
        let mut grants_writes = shared_source();
        grants_writes["field_defaults"]["float"]["range_status"] = json!("verified");
        assert!(serde_json::from_value::<Structure>(grants_writes).is_err());
    }

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
        let link = ParameterLink { field: "Osc{n}/Unit{n}/level".into(), at: vec![2, 0], status: Status::Guessed, note: None };
        assert_eq!(Structure::location(&link), "Osc2/Unit0/level");
    }

    #[test]
    fn writes_fields_one_per_line() {
        let structure = json!({"plugin": "x", "files": {"preset": {"layers": ["a"]}}, "fields": {"a": {"type": "int", "status": "guessed"}, "b": {"type": "text", "status": "guessed"}}, "parameters": {}});
        let text = to_text(&structure);
        assert!(text.contains("\n    \"a\": {\"type\":\"int\",\"status\":\"guessed\"},\n"), "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), structure);
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
