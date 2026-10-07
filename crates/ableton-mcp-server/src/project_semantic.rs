//! Privacy-preserving, bounded semantic Set snapshots.
use crate::project::ProjectError;
use kumi_common::js::{json, string};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cmp::Ordering, sync::LazyLock};
use unicode_normalization::UnicodeNormalization;

pub const SEMANTIC_PROJECT_SNAPSHOT_SCHEMA: &str = "ableton-mcp-semantic-set-snapshot/v1";
pub const SEMANTIC_PROJECT_MAX_RECORDS: usize = 12_000;
pub const SEMANTIC_PROJECT_MAX_PAGE_RECORDS: usize = 200;
pub const SEMANTIC_PROJECT_MAX_PAGE_BYTES: usize = 512 * 1024;
pub const SEMANTIC_PROJECT_MAX_BUNDLE_BYTES: usize = 1024 * 1024 * 1024;
pub const SEMANTIC_PROJECT_MAX_DIFF_INPUT_BYTES: usize = 1024 * 1024 * 1024;
pub const SEMANTIC_PROJECT_MAX_PAGES: usize = 2048;
pub const SECTION_ORDER: [&str; 8] = ["set", "tracks", "scenes", "locators", "clips", "devices", "dependencies", "unavailable"];
pub type SemanticProjectArtifact = Value;
pub type SemanticProjectPage = Value;
pub type SemanticProjectRecord = Value;
pub type SemanticPrivacyProfile = str;
pub(crate) fn fail(message: impl Into<String>) -> ProjectError {
    ProjectError(message.into())
}
pub fn compare_semantic_strings(left: &str, right: &str) -> Ordering {
    if left.is_ascii() && right.is_ascii() {
        left.cmp(right)
    } else {
        left.encode_utf16().cmp(right.encode_utf16())
    }
}
fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        string::utf16_len(s)
    }
}
fn canonical_quote(s: &str, out: &mut String) {
    if s.bytes().any(|b| b < 32 || b == b'"' || b == b'\\') {
        json::escape(s, out);
    } else {
        out.push('"');
        out.push_str(s);
        out.push('"');
    }
}
fn canonical_visit(value: &Value, depth: usize, nodes: &mut usize, out: &mut String) -> Result<(), ProjectError> {
    *nodes += 1;
    if *nodes > 100_000_000 {
        return Err(fail("semantic artifact exceeds the canonical node bound"));
    }
    if depth > 24 {
        return Err(fail("semantic artifact exceeds the canonical depth bound"));
    }
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&json::number(n)),
        Value::String(s) => {
            if utf16_len(s) > 4096 {
                return Err(fail("semantic artifact string exceeds the bound"));
            }
            canonical_quote(s, out);
        }
        Value::Array(rows) => {
            if rows.len() > 10_000_000 {
                return Err(fail("semantic artifact array exceeds the bound"));
            }
            out.push('[');
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_visit(row, depth + 1, nodes, out)?;
            }
            out.push(']');
        }
        Value::Object(object) => {
            if object.len() > 64 || object.keys().any(|key| utf16_len(key) > 128) {
                return Err(fail("semantic artifact object exceeds field or key bounds"));
            }
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_by(|a, b| compare_semantic_strings(a.0, b.0));
            out.push('{');
            for (i, (key, value)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_quote(key, out);
                out.push(':');
                canonical_visit(value, depth + 1, nodes, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}
pub fn canonical_semantic_json(value: &Value) -> Result<String, ProjectError> {
    let mut out = String::new();
    canonical_visit(value, 0, &mut 0, &mut out)?;
    Ok(out)
}
fn canonical_digest(json: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(json)))
}
pub(crate) fn digest(value: &Value) -> Result<String, ProjectError> {
    Ok(canonical_digest(&canonical_semantic_json(value)?))
}
// Hash borrowed projections: constructing a temporary serde_json::Value would deep-copy
// every record in a section or artifact before immediately serializing it again.
fn digest_fields(fields: &[(&str, &Value)]) -> Result<String, ProjectError> {
    if fields.len() > 64 || fields.iter().any(|(key, _)| utf16_len(key) > 128) {
        return Err(fail("semantic artifact object exceeds field or key bounds"));
    }
    let mut entries = fields.to_vec();
    entries.sort_by(|a, b| compare_semantic_strings(a.0, b.0));
    let mut out = String::from("{");
    let mut nodes = 1;
    for (i, (key, value)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        canonical_quote(key, &mut out);
        out.push(':');
        canonical_visit(value, 1, &mut nodes, &mut out)?;
    }
    out.push('}');
    Ok(canonical_digest(&out))
}
fn digest_array(values: &[&Value]) -> Result<String, ProjectError> {
    if values.len() > 10_000_000 {
        return Err(fail("semantic artifact array exceeds the bound"));
    }
    let mut out = String::from("[");
    let mut nodes = 1;
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        canonical_visit(value, 1, &mut nodes, &mut out)?;
    }
    out.push(']');
    Ok(canonical_digest(&out))
}
// Offsets into one canonical serialization are local to this immutable artifact's
// validation. Every schema, record fingerprint, manifest and identity check still runs.
struct CanonicalArtifact<'a> {
    json: String,
    fields: Vec<(&'a str, std::ops::Range<usize>)>,
    records: Vec<std::ops::Range<usize>>,
}
impl<'a> CanonicalArtifact<'a> {
    fn new(artifact: &'a Value) -> Result<Self, ProjectError> {
        let object = artifact.as_object().expect("artifact object shape checked before canonical validation");
        if object.len() > 64 || object.keys().any(|key| utf16_len(key) > 128) {
            return Err(fail("semantic artifact object exceeds field or key bounds"));
        }
        let mut entries: Vec<_> = object.iter().collect();
        entries.sort_by(|a, b| compare_semantic_strings(a.0, b.0));
        let mut result = Self { json: String::from("{"), fields: Vec::with_capacity(entries.len()), records: vec![] };
        let mut nodes = 1;
        for (i, (key, value)) in entries.into_iter().enumerate() {
            if i > 0 {
                result.json.push(',');
            }
            canonical_quote(key, &mut result.json);
            result.json.push(':');
            let start = result.json.len();
            if key == "records" {
                let rows = value.as_array().expect("record array shape checked before canonical validation");
                nodes += 1;
                if nodes > 100_000_000 {
                    return Err(fail("semantic artifact exceeds the canonical node bound"));
                }
                if rows.len() > 10_000_000 {
                    return Err(fail("semantic artifact array exceeds the bound"));
                }
                result.records.reserve(rows.len());
                result.json.push('[');
                for (index, record) in rows.iter().enumerate() {
                    if index > 0 {
                        result.json.push(',');
                    }
                    let start = result.json.len();
                    canonical_visit(record, 2, &mut nodes, &mut result.json)?;
                    result.records.push(start..result.json.len());
                }
                result.json.push(']');
            } else {
                canonical_visit(value, 1, &mut nodes, &mut result.json)?;
            }
            result.fields.push((key, start..result.json.len()));
        }
        result.json.push('}');
        Ok(result)
    }
    fn field(&self, key: &str) -> &str {
        let range = &self.fields.iter().find(|(name, _)| *name == key).expect("validated artifact field").1;
        &self.json[range.clone()]
    }
    fn record(&self, index: usize) -> &str {
        &self.json[self.records[index].clone()]
    }
}
fn digest_canonical_fields(fields: &[(&str, &str)]) -> String {
    let mut entries = fields.to_vec();
    entries.sort_by(|a, b| compare_semantic_strings(a.0, b.0));
    let mut digest = Sha256::new();
    digest.update(b"{");
    for (i, (key, value)) in entries.into_iter().enumerate() {
        if i > 0 {
            digest.update(b",");
        }
        let mut quoted = String::new();
        canonical_quote(key, &mut quoted);
        digest.update(quoted);
        digest.update(b":");
        digest.update(value);
    }
    digest.update(b"}");
    format!("sha256:{}", hex::encode(digest.finalize()))
}
fn digest_canonical_array<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"[");
    for (i, value) in values.enumerate() {
        if i > 0 {
            digest.update(b",");
        }
        digest.update(value);
    }
    digest.update(b"]");
    format!("sha256:{}", hex::encode(digest.finalize()))
}
fn short_digest(value: &Value) -> Result<String, ProjectError> {
    Ok(digest(value)?[7..27].to_owned())
}
fn bounded_string(value: &Value) -> String {
    value
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| string::head(&s.nfc().collect::<String>(), 512))
        .unwrap_or_else(|| "unavailable".into())
}
// JavaScript's \s (Unicode WhiteSpace + LineTerminator), deliberately excluding U+0085.
pub(crate) const JS_SPACE: &str = r"[\t\n\x0b\x0c\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
fn regex(pattern: &str) -> Regex {
    Regex::new(&pattern.replace(r"\s", JS_SPACE).replace(r"\S", &format!("[^{0}]", &JS_SPACE[1..JS_SPACE.len() - 1]))).unwrap()
}
/// A name that reads as an absolute or network path: the exporter keeps anything else as it is, and a diff of its
/// artifacts audits with the same test.
pub(crate) fn absolute_path(value: &str) -> bool {
    static PATH: LazyLock<Regex> = LazyLock::new(|| {
        regex(
            r#"(?i)^\s*[\\/]|(?:^|[\t\n\x0b\x0c\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}"'(=])(?:[A-Za-z]:[\\/]|\\\\)|["'(=]\s*/|\s/\S|[A-Za-z][A-Za-z0-9+.-]*:[\\/]{1,2}"#,
        )
    });
    PATH.is_match(value)
}
fn authority(value: &str) -> bool {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        regex(
            r"(?i)(?:reusable[-_ ]?(?:(?:mutation|access|authority|confirmation|recovery)[-_ ]?)?(?:token|secret|confirmation)|bearer\s+[A-Za-z0-9._-]{8,}|(?:access|authority|idempotency|recovery|preflight)[-_ ]?(?:token|secret|key)\s*[:=]?)",
        )
    });
    PATTERN.is_match(value)
}
fn live_reference(value: &str) -> bool {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        regex(
            r"(?i)^(?:[0-9]+:)?(?:set|track|return[_-]track|main[_-]track|scene|clip[_-]slot|clip|session[_-]playback|arrangement[_-]clip|take[_-]lane(?:[_-]clip)?|groove|note|automation|locator|device|parameter|chain|drum[_-]pad|routing[_-]choice|browser[_-]item|selection):\S+$",
        )
    });
    PATTERN.is_match(value)
}
fn dynamic_string(profile: &str, kind: &str, value: &Value, strict_alias: bool) -> String {
    let normalized = bounded_string(value);
    if absolute_path(&normalized) || authority(&normalized) || live_reference(&normalized) || (profile == "strict" && strict_alias) {
        format!("{kind}-{}", short_digest(&json!([kind, normalized])).expect("bounded normalized name"))
    } else {
        normalized
    }
}
pub fn semantic_project_name(profile: &str, kind: &str, value: &Value) -> String {
    dynamic_string(profile, kind, value, true)
}

use crate::project::{project_source_evidence, ProjectSourceEvidence};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSemanticProjectOptions {
    pub profile: Option<String>,
    pub exporter_version: String,
    pub live: Value,
    pub project_path: Option<String>,
    pub source_evidence: Option<ProjectSourceEvidence>,
    pub max_records: Option<f64>,
    pub source_kind: Option<String>,
    #[serde(default)]
    pub extra_unavailable: Vec<SemanticUnavailable>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticUnavailable {
    pub field: String,
    pub reason: String,
    pub source_name: String,
}
pub(crate) fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn safe_scalar(value: &Value) -> Value {
    if value.is_null() || value.is_boolean() || value.is_number() {
        value.clone()
    } else {
        Value::Null
    }
}
fn dynamic_scalar(profile: &str, kind: &str, value: &Value, alias: bool) -> Value {
    if value.is_string() {
        json!(dynamic_string(profile, kind, value, alias))
    } else {
        safe_scalar(value)
    }
}
fn numeric(value: &Value) -> Result<Value, ProjectError> {
    value.as_f64().filter(|n| n.is_finite()).map(|_| value.clone()).ok_or_else(|| fail("semantic artifact contains a non-finite number"))
}
fn pick(value: &Value, keys: &[&str]) -> Value {
    json!(keys.iter().map(|k| ((*k).to_owned(), value[*k].clone())).collect::<Map<_, _>>())
}
fn nullish<'a>(left: &'a Value, right: &'a Value) -> &'a Value {
    if left.is_null() {
        right
    } else {
        left
    }
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
fn sort_json(rows: &mut [Value]) -> Result<(), ProjectError> {
    let mut keyed = rows.iter().map(|row| Ok((canonical_semantic_json(row)?, row.clone()))).collect::<Result<Vec<_>, ProjectError>>()?;
    keyed.sort_by(|a, b| compare_semantic_strings(&a.0, &b.0));
    for (target, (_, row)) in rows.iter_mut().zip(keyed) {
        *target = row;
    }
    Ok(())
}
fn note_content(notes: &Value) -> Result<Value, ProjectError> {
    let mut rows = vec![];
    for note in array(notes) {
        let mut row = pick(note, &["mute", "probability", "velocityDeviation", "releaseVelocity"]);
        for key in ["pitch", "start", "duration", "velocity", "channel"] {
            row[key] = numeric(&note[key])?;
        }
        rows.push(row);
    }
    sort_json(&mut rows)?;
    let min = rows.iter().map(|n| n["pitch"].as_f64().unwrap()).reduce(f64::min);
    let max = rows.iter().map(|n| n["pitch"].as_f64().unwrap()).reduce(f64::max);
    let end = rows.iter().map(|n| n["start"].as_f64().unwrap() + n["duration"].as_f64().unwrap()).reduce(f64::max);
    if end.is_some_and(|n| !n.is_finite()) {
        return Err(fail("semantic artifact contains a non-finite number"));
    }
    Ok(json!({"count":rows.len(),"pitchMin":min,"pitchMax":max,"end":end,"hash":digest(&json!(rows))?}))
}
fn automation_summary(clip: &Value) -> Result<Value, ProjectError> {
    fn points(points: &Value) -> Result<Vec<Value>, ProjectError> {
        let mut rows = vec![];
        for p in array(points) {
            rows.push(json!({"time":numeric(&p["time"] )?,"value":numeric(&p["value"] )?,"curve":if p.get("curve").is_none(){Value::Null}else{numeric(&p["curve"])?}}));
        }
        sort_json(&mut rows)?;
        Ok(rows)
    }
    let direct = points(&clip["automation"])?;
    let mut envelopes = vec![];
    for rows in clip["envelopes"].as_object().into_iter().flat_map(|o| o.values()) {
        let rows = points(rows)?;
        envelopes.push(json!({"points":rows.len(),"hash":digest(&json!(rows))?}));
    }
    envelopes.sort_by(|a, b| compare_semantic_strings(a["hash"].as_str().unwrap(), b["hash"].as_str().unwrap()));
    Ok(
        json!({"envelopeCount":envelopes.len(),"pointCount":direct.len()+envelopes.iter().map(|v|v["points"].as_u64().unwrap() as usize).sum::<usize>(),"contentHash":digest(&json!({"direct":direct,"envelopePointLists":envelopes}))?}),
    )
}
fn device_state(device: &Value) -> Result<Value, ProjectError> {
    let mut parameters = vec![];
    for p in array(&device["parameters"]) {
        let mut row = pick(p, &["enabled", "defaultValue", "state"]);
        row["name"] = json!(bounded_string(nullish(&p["originalName"], &p["name"])));
        for key in ["value", "min", "max"] {
            row[key] = numeric(&p[key])?;
        }
        if let Some(v) = p.get("automatable") {
            row["automatable"] = v.clone();
        }
        row["valueItems"] = json!(array(&p["valueItems"]).iter().take(256).map(bounded_string).collect::<Vec<_>>());
        parameters.push(row);
    }
    let mut keyed = parameters
        .iter()
        .map(|p| Ok((p["name"].as_str().unwrap().to_owned(), canonical_semantic_json(p)?, p.clone())))
        .collect::<Result<Vec<_>, ProjectError>>()?;
    keyed.sort_by(|a, b| compare_semantic_strings(&a.0, &b.0).then_with(|| compare_semantic_strings(&a.1, &b.1)));
    parameters = keyed.into_iter().map(|(_, _, p)| p).collect();
    let schema: Vec<_> = parameters
        .iter()
        .map(|p| {
            let mut row = pick(p, &["name", "min", "max", "valueItems"]);
            if let Some(v) = p.get("automatable") {
                row["automatable"] = v.clone();
            }
            row
        })
        .collect();
    let mut specialized = json!({});
    for (kind, keys) in [
        ("drift", &["pitchBendRange", "voiceCount", "voiceMode"][..]),
        ("eq8", &["editMode", "globalMode", "oversample", "selectedBand"]),
        ("hybridReverb", &["irCategory", "irFile", "attack", "decay", "size"]),
        ("meld", &["engine", "unison", "monoPoly", "polyphony"]),
        ("drumCell", &["gain"]),
        ("looper", &["overdubAfterRecord", "recordLengthIndex", "loopLength", "tempo", "state"]),
        ("maxDevice", &[]),
    ] {
        let mut row = if truthy(&device[kind]) { pick(&device[kind], keys) } else { Value::Null };
        if !row.is_null() {
            if kind == "drift" {
                for (key, source) in [("modSourceCount", "modSources"), ("modTargetCount", "modTargets")] {
                    row[key] = device[kind][source].as_array().map_or(Value::Null, |v| json!(v.len()));
                }
            }
            if kind == "maxDevice" {
                for key in ["audioIns", "audioOuts", "midiIns", "midiOuts"] {
                    row[key] = device[kind][key].as_array().map_or(Value::Null, |v| json!(v.len()));
                }
            }
        }
        specialized[kind] = row;
    }
    let selected = |v: &Value| if nonnegative_integer(v) { v.clone() } else { Value::Null };
    let visible = json!({"enabled":device["enabled"],"latencySamples":device["latencySamples"],"parameterCount":parameters.len(),"pluginPresetIndex":selected(&device["plugin"]["selectedPresetIndex"]),"pluginPresetCount":device["plugin"]["presets"].as_array().map(|v|v.len()),"rackVariationCount":device["variationCount"],"selectedVariationIndex":selected(&device["selectedVariationIndex"]),"specializedHash":digest(&specialized)?});
    Ok(
        json!({"schemaHash":digest(&json!(schema))?,"stateHash":digest(&json!({"parameters":parameters,"visible":visible}))?,"visible":visible}),
    )
}
fn track_structure(track: &Value) -> Value {
    let mut clips: Vec<_> = array(&track["clips"]).iter().map(|c| c["kind"].as_str().unwrap_or("undefined").to_owned()).collect();
    clips.sort_by(|a, b| compare_semantic_strings(a, b));
    let mut devices: Vec<_> = array(&track["devices"]).iter().map(|d| bounded_string(nullish(&d["className"], &d["kind"]))).collect();
    devices.sort_by(|a, b| compare_semantic_strings(a, b));
    json!({"kind":track["kind"],"clipKinds":clips,"deviceClasses":devices})
}
fn section_for_record(kind: &str) -> &'static str {
    match kind {
        "set" => "set",
        "track" => "tracks",
        "scene" => "scenes",
        "locator" => "locators",
        "clip" => "clips",
        "device" => "devices",
        "dependency" => "dependencies",
        _ => "unavailable",
    }
}
fn create_record(kind: &str, order: usize, name: Option<String>, data: Value, matching: Value) -> Result<Value, ProjectError> {
    let mut row = json!({"section":section_for_record(kind),"kind":kind,"order":order,"matching":matching,"data":data,"contentFingerprint":digest(&json!({"kind":kind,"name":name,"data":data}))?,"semanticFingerprint":digest(&matching)?,"nameFingerprint":digest(&json!([kind,name]))?});
    if let Some(name) = name {
        row["name"] = json!(name);
    }
    Ok(row)
}
struct Records {
    raw: Vec<Value>,
    observed: HashMap<&'static str, usize>,
    included: HashMap<&'static str, usize>,
    max: f64,
}
impl Records {
    fn new(max: f64) -> Self {
        Self {
            raw: vec![],
            observed: SECTION_ORDER.into_iter().map(|k| (k, 0)).collect(),
            included: SECTION_ORDER.into_iter().map(|k| (k, 0)).collect(),
            max,
        }
    }
    fn push(&mut self, row: Value) {
        let section = section_for_record(row["kind"].as_str().unwrap());
        *self.observed.get_mut(section).unwrap() += 1;
        if self.raw.len() as f64 >= self.max {
            return;
        }
        self.raw.push(row);
        *self.included.get_mut(section).unwrap() += 1;
    }
    fn unavailable(&mut self, field: &str, reason: &str, source: &str) -> Result<(), ProjectError> {
        let order = self.observed["unavailable"];
        self.push(create_record(
            "unavailable",
            order,
            None,
            json!({"field":field,"reason":reason,"source":source,"state":"unavailable"}),
            json!({"field":field,"source":source}),
        )?);
        Ok(())
    }
}
fn network_path(value: &str) -> bool {
    static URI: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^[A-Za-z][A-Za-z0-9+.-]*:[\\/]{1,2}"));
    static DRIVE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z]:[\\/]"));
    value.replace('/', "\\").starts_with("\\\\") || (!DRIVE.is_match(value) && URI.is_match(value))
}
fn path_locator(profile: &str, raw: &str, resolved: Option<&str>, project: Option<&str>) -> Result<String, ProjectError> {
    if profile == "strict" {
        return Ok(format!("path-{}", short_digest(&json!([raw, resolved]))?));
    }
    let normalized = raw.replace('\\', "/");
    // Node's Windows basename omits a drive prefix, including drive-relative
    // names such as C:Beat.wav. Its POSIX implementation keeps that prefix.
    let bytes = normalized.as_bytes();
    let normalized = if cfg!(windows) && bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        &normalized[2..]
    } else {
        &normalized
    };
    let base = normalized.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("unnamed");
    if profile == "local" {
        if let (Some(resolved), Some(project)) = (resolved, project) {
            let project = crate::command::resolve(project).map_err(|e| fail(e.message()))?;
            let dir = project.parent().unwrap_or(Path::new("/"));
            let resolved = crate::command::resolve(resolved).map_err(|e| fail(e.message()))?;
            if let Ok(candidate) = resolved.strip_prefix(dir) {
                if !candidate.as_os_str().is_empty() {
                    return Ok(dynamic_string(profile, "path", &json!(candidate.to_string_lossy().replace('\\', "/")), false));
                }
            }
        }
    }
    Ok(dynamic_string(profile, "path", &json!(base), false))
}
fn nonnegative_integer(v: &Value) -> bool {
    v.as_f64().is_some_and(|n| n >= 0. && n.fract() == 0.)
}
fn add_clip(
    rows: &mut Records,
    dependencies: &mut Vec<Value>,
    profile: &str,
    clip: &Value,
    order: usize,
    location: Value,
    parent: Value,
) -> Result<(), ProjectError> {
    let name = semantic_project_name(profile, "clip", &clip["name"]);
    let notes = note_content(&clip["notes"])?;
    let automation = automation_summary(clip)?;
    let mut audio = pick(clip, &["gain", "pitchCoarse", "pitchFine", "warpMode", "sampleLength"]);
    audio["warping"] = nullish(&clip["warping"], &clip["warp"]).clone();
    let length = numeric(&clip["length"])?;
    let hash = digest(&audio)?;
    let data = json!({"clipKind":clip["kind"],"parentSnapshotId":parent,"location":location,"start":numeric(&clip["start"] )?,"length":length,"loopStart":clip["loopStart"],"loopEnd":clip["loopEnd"],"looping":clip["looping"],"muted":clip["muted"],"notes":notes,"automation":automation,"audioMetadataHash":hash,"rawAudioContent":"unavailable-not-read"});
    rows.push(create_record(
        "clip",
        order,
        Some(name),
        data,
        json!({"clipKind":clip["kind"],"noteHash":notes["hash"],"length":length,"audioMetadataHash":hash}),
    )?);
    if let Some(raw) = clip["filePath"].as_str().filter(|s| !s.is_empty() && string::utf16_len(s) <= 4096) {
        let network = network_path(raw);
        // Node path.isAbsolute accepts a Windows rooted path without a drive
        // (\Samples\Beat.wav or /Samples/Beat.wav). Rust is_absolute requires
        // both; has_root matches the source for this lexical classification.
        let absolute = !network && Path::new(raw).has_root();
        let mut row =
            json!({"raw":raw,"resolution":if network{"network"}else if absolute{"absolute"}else{"unresolved"},"evidence":"live-clip"});
        if absolute {
            row["resolvedPath"] = json!(crate::command::resolve(raw).map_err(|e| fail(e.message()))?.to_string_lossy());
        }
        dependencies.push(row);
    }
    Ok(())
}
pub fn create_semantic_project_snapshot(snapshot: &Value, options: &CreateSemanticProjectOptions) -> Result<Value, ProjectError> {
    let profile = options.profile.as_deref().unwrap_or("collaboration");
    if !["strict", "collaboration", "local"].contains(&profile) {
        return Err(fail("unknown semantic snapshot privacy profile"));
    }
    let max = options.max_records.unwrap_or(12000.);
    let max = if max.is_nan() { max } else { max.clamp(1., 12000.) };
    let source = if let Some(source) = &options.source_evidence {
        Some(source.clone())
    } else if let Some(path) = options.project_path.as_deref().filter(|s| !s.is_empty()) {
        Some(project_source_evidence(path)?)
    } else {
        None
    };
    let mut rows = Records::new(max);
    let tracks = array(&snapshot["tracks"]);
    let scenes = array(&snapshot["scenes"]);
    let set_name = semantic_project_name(profile, "set", &snapshot["set"]["name"]);
    let set_data = json!({"tempo":safe_scalar(&snapshot["set"]["tempo"]),"arrangementLength":safe_scalar(&snapshot["arrangement"]["length"]),"trackCount":tracks.len(),"sceneCount":scenes.len()});
    let mut set = set_data.clone();
    set["name"] = json!(set_name);
    rows.push(create_record("set", 0, Some(set_name), set_data, json!({"kind":"set"}))?);
    let mut coords = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for track in tracks {
        let name = semantic_project_name(profile, "track", &track["name"]);
        let hash = digest(&track_structure(track))?;
        let base = format!("track-snapshot:{}", short_digest(&json!({"kind":track["kind"],"name":name,"structureHash":hash}))?);
        let count = counts.entry(base.clone()).or_default();
        *count += 1;
        coords.insert(track["ref"].as_str().unwrap_or(""), json!(format!("{base}-{count}")));
    }
    let coordinate = |reference: &Value| coords.get(reference.as_str().unwrap_or("")).cloned().unwrap_or(Value::Null);
    for (index, track) in tracks.iter().enumerate() {
        let name = semantic_project_name(profile, "track", &track["name"]);
        let hash = digest(&track_structure(track))?;
        let full = truthy(&track["mixer"]);
        let mixer_source = if full { &track["mixer"] } else { track };
        let mixer_keys = if full {
            &[
                "volume",
                "pan",
                "cueVolume",
                "mute",
                "solo",
                "trackActivator",
                "crossfader",
                "crossfadeAssign",
                "panningMode",
                "panningLeft",
                "panningRight",
            ][..]
        } else {
            &["volume", "pan", "mute", "solo"][..]
        };
        let mut mixer = json!({});
        for key in mixer_keys {
            mixer[key] = safe_scalar(&mixer_source[key]);
        }
        mixer["sends"] = json!(array(&mixer_source["sends"]).iter().take(128).map(safe_scalar).collect::<Vec<_>>());
        let mut routing = json!({});
        if truthy(&track["routing"]) {
            for key in ["inputType", "inputSubRouting", "outputType", "outputSubRouting"] {
                routing[key] = dynamic_scalar(profile, "routing", &track["routing"][key], true);
            }
        } else {
            routing["inputType"] = dynamic_scalar(profile, "routing", &track["input"], true);
            routing["outputType"] = dynamic_scalar(profile, "routing", &track["output"], true);
        }
        let data = json!({"kind":track["kind"],"mixer":mixer,"routing":routing,"armed":safe_scalar(&track["armed"]),"monitoring":safe_scalar(&track["monitoringState"]),"clipCount":array(&track["clips"]).len(),"deviceCount":array(&track["devices"]).len(),"structureHash":hash,"groupSnapshotId":if truthy(&track["groupTrackRef"]){coordinate(&track["groupTrackRef"])}else{Value::Null}});
        rows.push(create_record("track", index, Some(name), data, json!({"trackKind":track["kind"],"structureHash":hash}))?);
    }
    // Each track's first slot for each scene index, and its first clip for each ref, found once for every scene below
    // (the same ones the searches they replace would find: numbers compare as numbers, as js_equal does).
    fn number_key(number: f64) -> u64 {
        (if number == 0. { 0f64 } else { number }).to_bits()
    }
    let lookups: Vec<(HashMap<u64, &Value>, HashMap<&str, &Value>)> = tracks
        .iter()
        .map(|track| {
            let mut slots = HashMap::new();
            for slot in array(&track["clipSlots"]) {
                if let Some(index) = slot["sceneIndex"].as_f64() {
                    slots.entry(number_key(index)).or_insert(slot);
                }
            }
            let mut clips = HashMap::new();
            for clip in array(&track["clips"]) {
                if let Some(reference) = clip["ref"].as_str() {
                    clips.entry(reference).or_insert(clip);
                }
            }
            (slots, clips)
        })
        .collect();
    for (index, scene) in scenes.iter().enumerate() {
        let mut contents = vec![];
        for (track, (slots, clips)) in tracks.iter().zip(&lookups) {
            let slot = match scene["index"].as_f64() {
                Some(index) => slots.get(&number_key(index)).copied(),
                None => array(&track["clipSlots"]).iter().find(|slot| js_equal(&slot["sceneIndex"], &scene["index"])),
            };
            if let Some(slot) = slot.filter(|s| truthy(&s["clipRef"])) {
                let clip = match slot["clipRef"].as_str() {
                    Some(reference) => clips.get(reference).copied(),
                    None => array(&track["clips"]).iter().find(|c| c["ref"] == slot["clipRef"]),
                };
                if let Some(clip) = clip {
                    contents.push(json!({"kind":clip["kind"],"content":note_content(&clip["notes"])?["hash"],"length":clip["length"]}));
                }
            }
        }
        sort_json(&mut contents)?;
        let hash = digest(&json!(contents))?;
        let mut data = pick(scene, &["colorIndex", "tempoEnabled", "isEmpty"]);
        for key in ["tempo", "signatureNumerator", "signatureDenominator"] {
            data[key] = if scene[key].as_f64().is_some_and(|n| n >= 0.) { scene[key].clone() } else { Value::Null };
        }
        data["structureHash"] = json!(hash);
        rows.push(create_record(
            "scene",
            index,
            Some(semantic_project_name(profile, "scene", &scene["name"])),
            data,
            json!({"structureHash":hash}),
        )?);
    }
    for (index, locator) in array(&snapshot["arrangement"]["locators"]).iter().enumerate() {
        let data = json!({"position":numeric(&locator["position"])?});
        rows.push(create_record("locator", index, Some(semantic_project_name(profile, "locator", &locator["name"])), data.clone(), data)?);
    }
    let mut dependencies = vec![];
    let mut clip_order = 0;
    for track in tracks {
        let parent = coordinate(&track["ref"]);
        let mut slots = HashMap::new();
        for slot in array(&track["clipSlots"]) {
            if truthy(&slot["clipRef"]) {
                slots.insert(slot["clipRef"].as_str().unwrap_or(""), slot["sceneIndex"].clone());
            }
        }
        for clip in array(&track["clips"]) {
            let scene = slots.get(clip["ref"].as_str().unwrap_or("")).cloned().unwrap_or(Value::Null);
            add_clip(
                &mut rows,
                &mut dependencies,
                profile,
                clip,
                clip_order,
                json!({"lane":"session","sceneOrder":scene}),
                parent.clone(),
            )?;
            clip_order += 1;
        }
        for lane in array(&track["takeLanes"]) {
            for clip in array(&lane["clips"]) {
                add_clip(
                    &mut rows,
                    &mut dependencies,
                    profile,
                    clip,
                    clip_order,
                    json!({"lane":"take-lane","laneOrder":lane["index"]}),
                    parent.clone(),
                )?;
                clip_order += 1;
            }
        }
    }
    for entry in array(&snapshot["arrangementClips"]) {
        add_clip(
            &mut rows,
            &mut dependencies,
            profile,
            &entry["clip"],
            clip_order,
            json!({"lane":"arrangement"}),
            coordinate(&entry["trackRef"]),
        )?;
        clip_order += 1;
    }
    fn children(device: &Value) -> Vec<&Value> {
        array(&device["chains"])
            .iter()
            .chain(array(&device["drumPads"]).iter().flat_map(|p| array(&p["chains"])))
            .flat_map(|c| array(&c["devices"]))
            .collect()
    }
    let mut device_order = 0;
    static PLUGIN: LazyLock<Regex> = LazyLock::new(|| regex("(?i)plugin|vst|audio unit|auplugin"));
    static MAX: LazyLock<Regex> = LazyLock::new(|| regex("(?i)max"));
    for track in tracks {
        let mut stack: Vec<_> =
            array(&track["devices"]).iter().enumerate().map(|(i, d)| (d, 0usize, coordinate(&track["ref"]), i)).rev().collect();
        while let Some((device, depth, parent, sibling)) = stack.pop() {
            if depth > 8 {
                let mut pending = vec![device];
                let mut count = 0;
                while let Some(d) = pending.pop() {
                    count += 1;
                    pending.extend(children(d));
                }
                *rows.observed.get_mut("devices").unwrap() += count;
                rows.unavailable("device-hierarchy", "device hierarchy exceeded the exporter depth bound", "live-snapshot")?;
                continue;
            }
            let name = semantic_project_name(profile, "device", &device["name"]);
            let class = semantic_project_name(profile, "device-class", nullish(&device["className"], &device["kind"]));
            let state = device_state(device)?;
            let plugin =
                device["kind"] == "plugin" || device.get("plugin").is_some() || PLUGIN.is_match(device["className"].as_str().unwrap_or(""));
            let max = device.get("maxDevice").is_some() || MAX.is_match(device["className"].as_str().unwrap_or(""));
            let opaque = plugin || max;
            let matching =
                json!({"deviceKind":device["kind"],"className":class,"parameterSchemaHash":state["schemaHash"],"opaqueState":opaque});
            let coord = format!(
                "device-snapshot:{}",
                short_digest(&json!({"parentSnapshotId":parent,"name":name,"className":class,"siblingOrder":sibling}))?
            );
            let data = json!({"deviceKind":device["kind"],"className":class,"parentSnapshotId":parent,"depth":depth,"siblingOrder":sibling,"parameterSchemaHash":state["schemaHash"],"parameterStateHash":state["stateHash"],"opaqueState":opaque,"state":state["visible"]});
            rows.push(create_record("device", device_order, Some(name), data, matching)?);
            device_order += 1;
            if opaque {
                let category = if plugin { "plug-in" } else { "max-device" };
                let name = semantic_project_name(profile, category, &device["name"]);
                rows.push(create_record("dependency",rows.observed["dependencies"],Some(name.clone()),json!({"category":category,"origin":if plugin{"plug-in"}else{"max"},"availability":"discovered","stateVisibility":"opaque","locator":name,"evidence":"live-device","classificationEvidence":"live-generic-class","portability":"unknown"}),json!({"category":category,"className":class,"opaqueState":true}))?);
                rows.unavailable(
                    &format!("{category}-portability"),
                    &format!("{category} binary/blob portability is opaque and is not exported"),
                    "live-device",
                )?;
            }
            let mut child_rows = vec![];
            for chain in array(&device["chains"]) {
                let container = format!(
                    "chain-snapshot:{}",
                    short_digest(
                        &json!({"parentSnapshotId":coord,"chainIndex":chain["index"],"chainName":semantic_project_name(profile,"chain",&chain["name"])})
                    )?
                );
                for (i, d) in array(&chain["devices"]).iter().enumerate() {
                    child_rows.push((d, depth + 1, json!(container), i));
                }
            }
            for pad in array(&device["drumPads"]) {
                for chain in array(&pad["chains"]) {
                    let container = format!(
                        "drum-pad-snapshot:{}",
                        short_digest(
                            &json!({"parentSnapshotId":coord,"padIndex":pad["index"],"padNote":pad["note"],"padName":semantic_project_name(profile,"drum-pad",&pad["name"]),"chainIndex":chain["index"],"chainName":semantic_project_name(profile,"chain",&chain["name"])})
                        )?
                    );
                    for (i, d) in array(&chain["devices"]).iter().enumerate() {
                        child_rows.push((d, depth + 1, json!(container), i));
                    }
                }
            }
            stack.extend(child_rows.into_iter().rev());
        }
    }
    let mut references = source
        .as_ref()
        .map(|s| {
            s.references
                .iter()
                .map(|r| {
                    let mut row = serde_json::to_value(r).unwrap();
                    row["raw"] = json!(r.value);
                    row["evidence"] = json!("als-file-ref");
                    row
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    references.extend(dependencies);
    references.sort_by(|a, b| compare_semantic_strings(a["raw"].as_str().unwrap(), b["raw"].as_str().unwrap()));
    let mut seen = HashSet::new();
    for reference in references {
        let raw = reference["raw"].as_str().unwrap();
        let key = digest(&json!([raw.replace('\\', "/"), reference["resolvedPath"]]))?;
        if !seen.insert(key) {
            continue;
        }
        let resolution = reference["resolution"].as_str().unwrap();
        let normalized = raw.replace('\\', "/").to_lowercase();
        let origin = if ["network", "oversized", "unresolved"].contains(&resolution) {
            "unknown"
        } else if reference["projectLocal"] == true {
            "project-local"
        } else if normalized.contains("/packs/") || normalized.contains("/factory packs/") {
            "pack"
        } else if normalized.contains("/user library/") {
            "user-library"
        } else if resolution == "absolute" {
            "external"
        } else {
            "unknown"
        };
        let availability = if reference["exists"] == false {
            "missing"
        } else if reference["exists"] == true {
            "discovered"
        } else {
            "unknown"
        };
        let locator = if resolution == "network" {
            format!("network-{}", short_digest(&json!(["network-reference", raw]))?)
        } else {
            path_locator(profile, raw, reference["resolvedPath"].as_str(), options.project_path.as_deref())?
        };
        let evidence = if origin == "project-local" {
            if reference["exists"] == true {
                "verified-realpath"
            } else {
                "missing-lexical-project-path"
            }
        } else if origin == "pack" || origin == "user-library" {
            "path-segment-heuristic"
        } else if resolution == "network" {
            "network-reference-blocked"
        } else if resolution == "oversized" {
            "oversized-reference-blocked"
        } else {
            "path-evidence"
        };
        let hash = digest(&json!(raw))?;
        rows.push(create_record("dependency",rows.observed["dependencies"],Some(locator.clone()),json!({"category":"media","origin":origin,"availability":availability,"stateVisibility":if ["unresolved","network","oversized"].contains(&resolution){"opaque"}else{"semantic"},"locator":locator,"locatorDigest":hash,"evidence":reference["evidence"],"classificationEvidence":evidence,"portability":"unknown"}),json!({"category":"media","origin":origin,"locatorDigest":hash}))?);
    }
    if let Some(source) = &source {
        if !source.reference_bounds.complete {
            *rows.observed.get_mut("dependencies").unwrap() += source.reference_bounds.omitted;
            rows.unavailable(
                "dependency-manifest",
                &format!(
                    "at least {} FileRef entry exceeded the bounded evidence collection; observed counts are lower bounds",
                    source.reference_bounds.omitted
                ),
                "als-file-ref-bound",
            )?;
        }
    }
    if source.is_none() {
        rows.unavailable("set-file-provenance", "the Live Set is unsaved or host file evidence is unavailable", "live-snapshot")?;
    }
    if !truthy(&options.live["version"]) {
        rows.unavailable(
            "live-version",
            "active Live version was not exposed by the adapter; saved Set creator/version remains source provenance only",
            "live-adapter",
        )?;
    }
    if source.is_none() {
        rows.unavailable(
            "dependency-manifest",
            "saved Set FileRef evidence is unavailable; only observed Live clip/device dependencies are included",
            "live-snapshot",
        )?;
    }
    for extra in &options.extra_unavailable {
        rows.unavailable(&extra.field, &extra.reason, &extra.source_name)?;
    }
    rows.unavailable("audio-content-hash", "referenced media bytes are never read by semantic export", "policy")?;
    if let Some(source) = &source {
        for (section, count, actual) in [("tracks", source.manifest.tracks, tracks.len()), ("scenes", source.manifest.scenes, scenes.len())]
        {
            if count > actual {
                rows.unavailable(
                    section,
                    &format!("saved Set reports {count} {section} while the bounded adapter snapshot supplied {actual}"),
                    "adapter-bound",
                )?;
            }
            let observed = rows.observed.get_mut(section).unwrap();
            *observed = (*observed).max(count);
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for record in &mut rows.raw {
        let base = format!("semantic-{}-{}", record["kind"].as_str().unwrap(), &record["semanticFingerprint"].as_str().unwrap()[7..27]);
        let count = counts.entry(base.clone()).or_default();
        *count += 1;
        record["snapshotId"] = json!(format!("{base}-{count}"));
    }
    let records = rows.raw;
    let mut manifest = json!({});
    for section in SECTION_ORDER {
        let section_rows: Vec<_> = records.iter().filter(|r| r["section"] == section).collect();
        let omitted = rows.observed[section] - rows.included[section];
        manifest[section] = json!({"observed":rows.observed[section],"included":rows.included[section],"omitted":omitted,"complete":omitted==0,"digest":digest(&json!(section_rows))?});
    }
    let policy = json!({"profile":profile,"names":if profile=="strict"{"typed-aliases"}else{"retained"},"paths":match profile{"strict"=>"typed-digests","collaboration"=>"basenames",_=>"project-relative-or-basename"}});
    let provenance_value = |kind: &str, value: &Value| -> Result<Option<String>, ProjectError> {
        let Some(value) = value.as_str().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let normalized = string::head(&value.nfc().collect::<String>(), 128);
        static VALID: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z0-9 ._+()-]+$"));
        Ok(Some(if absolute_path(&normalized) || authority(&normalized) || !VALID.is_match(&normalized) {
            format!("{kind}-{}", short_digest(&json!([kind, normalized]))?)
        } else {
            normalized
        }))
    };
    let mut live = json!({"protocol":dynamic_string(profile,"protocol",&options.live["protocol"],false),"adapter":dynamic_string(profile,"adapter",&options.live["adapter"],false),"provenance":dynamic_string(profile,"provenance",nullish(&options.live["provenance"],&json!("unknown")),false)});
    if truthy(&options.live["registryHash"]) {
        live["registryHash"] = json!(dynamic_string(profile, "registry-hash", &options.live["registryHash"], false));
    }
    if let Some(version) = provenance_value("live-version", &options.live["version"])? {
        live["version"] = json!(version);
    }
    let mut provenance = json!({"source":if source.is_none(){"live-only"}else if options.source_kind.as_deref()==Some("offline-file"){"offline-file"}else{"live+als"},"live":live,"limitations":["host paging does not remove the existing bridge snapshot traversal/frame bounds","Pack and User Library origins are path-segment heuristics, not installed ownership or portability claims","opaque plug-in and Max state is not decoded"]});
    if let Some(source) = &source {
        let attrs = serde_json::to_value(&source.ableton).unwrap();
        let mut ableton = json!({});
        for (field, kind) in [
            ("creator", "creator"),
            ("majorVersion", "major-version"),
            ("minorVersion", "minor-version"),
            ("schemaChangeCount", "schema-change"),
        ] {
            if let Some(value) = provenance_value(kind, &attrs[field])? {
                ableton[field] = json!(value);
            }
        }
        provenance["setFileSha256"] = json!(source.manifest.sha256);
        provenance["ableton"] = ableton;
    }
    let safety = json!({"readOnly":true,"containsSessionReferences":false,"containsMutationAuthority":false,"crossRunIdentityClaimed":false,"mergeProposed":false});
    let hash = digest(
        &json!({"schema":SEMANTIC_PROJECT_SNAPSHOT_SCHEMA,"policy":policy,"set":set,"manifest":manifest,"safety":safety,"records":records}),
    )?;
    let version = bounded_string(&json!(options.exporter_version));
    let id = digest(
        &json!({"schema":SEMANTIC_PROJECT_SNAPSHOT_SCHEMA,"exporterVersion":version,"semanticHash":hash,"policy":policy,"provenance":provenance,"set":set,"manifest":manifest,"safety":safety,"records":records}),
    )?;
    let artifact = json!({"schema":SEMANTIC_PROJECT_SNAPSHOT_SCHEMA,"artifact":{"id":id,"semanticHash":hash,"exporterVersion":version},"policy":policy,"provenance":provenance,"set":set,"manifest":manifest,"safety":safety,"records":records});
    authority_audit(&artifact, 0)?;
    validate_semantic_project_artifact(&artifact)?;
    Ok(artifact)
}
pub(crate) fn js_equal(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}
#[path = "project_semantic_validate.rs"]
mod validation;
pub use validation::validate_semantic_project_artifact;
fn authority_audit(value: &Value, depth: usize) -> Result<(), ProjectError> {
    fn visit<'a>(value: &'a Value, depth: usize, keys: &mut HashSet<&'a str>, strings: &mut HashSet<&'a str>) -> Result<(), ProjectError> {
        if depth > 24 {
            return Err(fail("semantic output exceeds the audit depth bound"));
        }
        if let Some(s) = value.as_str() {
            if strings.contains(s) {
                return Ok(());
            }
            if absolute_path(s) {
                return Err(fail("semantic output contains an absolute, network, device, or file-URI path"));
            }
            if authority(s) {
                return Err(fail("semantic output contains reusable authority-like content"));
            }
            if live_reference(s) {
                return Err(fail("semantic output contains a Live session reference-like value"));
            }
            strings.insert(s);
            return Ok(());
        }
        if let Some(rows) = value.as_array() {
            for row in rows {
                visit(row, depth + 1, keys, strings)?;
            }
        }
        if let Some(object) = value.as_object() {
            static CAMEL: LazyLock<Regex> = LazyLock::new(|| regex("([a-z])([A-Z])"));
            static FORBIDDEN: LazyLock<Regex> = LazyLock::new(|| {
                regex("(?i)(?:^|_)(?:ref|objectIdentity|epoch|revision|transactionId|confirmation|token|secret|idempotencyKey|mac|authority|recoveryToken|preflightToken|accessToken|sessionRef)$")
            });
            for (key, child) in object {
                if !keys.contains(key.as_str()) {
                    if !["containsSessionReferences", "containsMutationAuthority", "crossRunIdentityClaimed"].contains(&key.as_str())
                        && FORBIDDEN.is_match(&CAMEL.replace_all(key, "${1}_${2}"))
                    {
                        return Err(fail(format!("semantic output contains forbidden session or authority field: {key}")));
                    }
                    keys.insert(key);
                }
                visit(child, depth + 1, keys, strings)?;
            }
        }
        Ok(())
    }
    visit(value, depth, &mut HashSet::new(), &mut HashSet::new())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticPageOptions {
    pub limit: Option<f64>,
    pub cursor: Option<String>,
}
pub(crate) fn cursor_json(cursor: &str) -> Option<Value> {
    let mut bytes = vec![];
    let mut accumulator = 0u32;
    let mut bits = 0;
    for ch in cursor.chars() {
        let n = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | n;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    serde_json::from_str(&String::from_utf8_lossy(&bytes)).ok()
}
pub fn semantic_cursor_artifact_id(cursor: &str) -> Option<String> {
    let row = cursor_json(cursor)?;
    row.as_object()?.get("artifactId")?.as_str().map(str::to_owned)
}
fn encode_cursor(id: &Value, profile: &Value, offset: usize) -> Result<String, ProjectError> {
    use base64::Engine;
    let mut payload =
        json!({"artifactId":id,"profile":profile,"offset":offset,"plan":"assemblable-v1","schema":SEMANTIC_PROJECT_SNAPSHOT_SCHEMA});
    payload["checksum"] = json!(short_digest(&payload)?);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(canonical_semantic_json(&payload)?))
}
fn decode_cursor(cursor: &str, artifact: &Value) -> Result<f64, ProjectError> {
    let row = cursor_json(cursor).filter(Value::is_object).ok_or_else(|| fail("semantic snapshot cursor is malformed"))?;
    let keys = ["artifactId", "profile", "offset", "plan", "schema"];
    if keys.iter().any(|k| row.get(*k).is_none()) {
        return Err(fail("semantic artifact contains an unsupported value"));
    }
    let payload = pick(&row, &keys);
    if row["checksum"] != short_digest(&payload)?
        || row["artifactId"] != artifact["artifact"]["id"]
        || row["profile"] != artifact["policy"]["profile"]
        || row["plan"] != "assemblable-v1"
        || row["schema"] != SEMANTIC_PROJECT_SNAPSHOT_SCHEMA
        || !nonnegative_integer(&row["offset"])
    {
        return Err(fail("semantic snapshot cursor does not match the artifact"));
    }
    Ok(row["offset"].as_f64().unwrap())
}
fn header(artifact: &Value) -> Value {
    pick(artifact, &["schema", "artifact", "policy", "provenance", "set", "manifest", "safety"])
}
fn make_page(artifact: &Value, offset: usize, count: usize) -> Result<Value, ProjectError> {
    let records = array(&artifact["records"]);
    let complete = offset + count == records.len();
    let mut page = header(artifact);
    page["page"] = json!({"offset":offset,"returned":count,"total":records.len(),"complete":complete});
    if !complete {
        page["page"]["nextCursor"] = json!(encode_cursor(&artifact["artifact"]["id"], &artifact["policy"]["profile"], offset + count)?);
    }
    page["records"] = json!(&records[offset..offset + count]);
    Ok(page)
}
pub fn page_semantic_project_snapshot(artifact: &Value, options: &SemanticPageOptions) -> Result<Value, ProjectError> {
    validate_semantic_project_artifact(artifact)?;
    let limit = options.limit.unwrap_or(100.);
    if limit.fract() != 0. || !(1. ..=SEMANTIC_PROJECT_MAX_PAGE_RECORDS as f64).contains(&limit) {
        return Err(fail("semantic snapshot page limit is invalid"));
    }
    let cursor = options.cursor.as_deref().filter(|s| !s.is_empty());
    let offset = cursor.map(|s| decode_cursor(s, artifact)).transpose()?.unwrap_or(0.);
    let total = array(&artifact["records"]).len();
    if offset > total as f64 {
        return Err(fail("semantic snapshot cursor offset is outside the artifact"));
    }
    let offset = offset as usize;
    let bounded_count = |offset: usize| -> Result<usize, ProjectError> {
        let mut candidate = (limit as usize).min(total - offset);
        while candidate > 0 {
            if canonical_semantic_json(&make_page(artifact, offset, candidate)?)?.len() <= SEMANTIC_PROJECT_MAX_PAGE_BYTES {
                return Ok(candidate);
            }
            candidate -= 1;
        }
        Ok(0)
    };
    if cursor.is_none() {
        let mut planned_offset = 0;
        let mut pages = 0;
        let mut bytes = 2;
        while planned_offset < total && pages <= SEMANTIC_PROJECT_MAX_PAGES && bytes <= SEMANTIC_PROJECT_MAX_BUNDLE_BYTES {
            let count = bounded_count(planned_offset)?;
            if count == 0 {
                break;
            }
            bytes += canonical_semantic_json(&make_page(artifact, planned_offset, count)?)?.len() + usize::from(pages > 0);
            planned_offset += count;
            pages += 1;
        }
        if planned_offset != total || pages > SEMANTIC_PROJECT_MAX_PAGES || bytes > SEMANTIC_PROJECT_MAX_BUNDLE_BYTES {
            return Err(fail("semantic snapshot page limit is too small for an assemblable bounded plan"));
        }
    }
    let count = bounded_count(offset)?;
    if total > offset && count == 0 {
        return Err(fail("one semantic record exceeds the page byte bound"));
    }
    let page = make_page(artifact, offset, count)?;
    authority_audit(&page, 0)?;
    Ok(page)
}
pub fn assemble_semantic_project_pages(pages: &[Value]) -> Result<Value, ProjectError> {
    if pages.is_empty()
        || pages.len() > SEMANTIC_PROJECT_MAX_PAGES
        || canonical_semantic_json(&json!(pages))?.len() > SEMANTIC_PROJECT_MAX_BUNDLE_BYTES
    {
        return Err(fail("semantic snapshot page bundle is empty or exceeds bounds"));
    }
    let first = &pages[0];
    let expected_header = canonical_semantic_json(&header(first))?;
    let mut offset = 0usize;
    let mut records = vec![];
    for page in pages {
        validation::only(page, &["schema", "artifact", "policy", "provenance", "set", "manifest", "safety", "page", "records"], "page")?;
        let coordinates = &page["page"];
        validation::only(coordinates, &["offset", "returned", "total", "complete", "nextCursor"], "page coordinates")?;
        if !page["records"].is_array()
            || ["offset", "returned", "total"].iter().any(|k| coordinates[k].as_f64().is_none_or(|n| n.fract() != 0.))
        {
            return Err(fail("semantic snapshot page coordinates are malformed"));
        }
        let rows = array(&page["records"]);
        let page_offset = coordinates["offset"].as_f64().unwrap();
        let complete = page_offset + rows.len() as f64 == coordinates["total"].as_f64().unwrap();
        let expected_cursor = if complete {
            None
        } else {
            Some(json!(encode_cursor_number(&first["artifact"]["id"], &first["policy"]["profile"], page_offset + rows.len() as f64)?))
        };
        if canonical_semantic_json(page)?.len() > SEMANTIC_PROJECT_MAX_PAGE_BYTES
            || canonical_semantic_json(&header(page))? != expected_header
            || page_offset != offset as f64
            || coordinates["returned"].as_f64() != Some(rows.len() as f64)
            || !js_equal(&coordinates["total"], &first["page"]["total"])
            || coordinates["complete"].as_bool() != Some(complete)
            || coordinates.get("nextCursor") != expected_cursor.as_ref()
        {
            return Err(fail("semantic snapshot pages are inconsistent, overlapping, reordered, tampered, or non-contiguous"));
        }
        records.extend_from_slice(rows);
        offset += rows.len();
    }
    if !truthy(&pages.last().unwrap()["page"]["complete"])
        || first["page"]["total"].as_f64() != Some(offset as f64)
        || records.len() > SEMANTIC_PROJECT_MAX_RECORDS
    {
        return Err(fail("semantic snapshot page bundle is incomplete"));
    }
    let mut artifact = header(first);
    artifact["records"] = json!(records);
    validate_semantic_project_artifact(&artifact)?;
    Ok(artifact)
}
fn encode_cursor_number(id: &Value, profile: &Value, offset: f64) -> Result<String, ProjectError> {
    use base64::Engine;
    let mut payload =
        json!({"artifactId":id,"profile":profile,"offset":offset,"plan":"assemblable-v1","schema":SEMANTIC_PROJECT_SNAPSHOT_SCHEMA});
    payload["checksum"] = json!(short_digest(&payload)?);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(canonical_semantic_json(&payload)?))
}
