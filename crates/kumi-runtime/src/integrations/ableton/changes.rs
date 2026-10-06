//! Native change preparation and metadata for Live's guarded preview/apply operations.
mod more_summaries;
mod summaries;
use crate::core::{contracts::*, errors::RuntimeError};
use async_trait::async_trait;
use kumi_common::js::{
    json::stringify,
    number,
    string::{head, trim},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    },
};
pub type KnownTrack = TrackChip;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleFile {
    pub path: String,
    pub folder: String,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SampleSelector {
    pub words: Vec<String>,
    pub folders: Vec<String>,
    pub random: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterRange {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}
#[async_trait(?Send)]
pub trait ChangeContext {
    fn sample(&self, path: &str) -> Option<SampleFile>;
    async fn parameters(&self, device_ref: &str) -> Result<Vec<ParameterRange>, RuntimeError>;
    async fn ranges(&self, device_ref: &str) -> Result<Vec<ParameterRange>, RuntimeError>;
    async fn pick(&self, selector: SampleSelector) -> Result<Option<SampleFile>, RuntimeError>;
    fn has_value_for(&self) -> bool {
        false
    }
    async fn value_for(&self, _parameter_ref: &str, _text: &str) -> Result<Result<f64, String>, RuntimeError> {
        unreachable!("optional valueFor")
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<KnownTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipPicture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<ColorChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<DevicePlacement>,
}
impl ChangeSummary {
    pub fn title(title: impl Into<String>) -> Self {
        Self { title: title.into(), ..Default::default() }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeKind {
    pub tool: String,
    pub preview: String,
    pub apply: String,
    pub family: ChangeFamily,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restructures: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_schema: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing)]
    pub methods: Vec<String>,
}
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/changes.json")).unwrap());
pub static CHANGES: LazyLock<Vec<ChangeKind>> = LazyLock::new(|| serde_json::from_value(DATA["kinds"].clone()).unwrap());
pub static SAMPLE_INPUT: LazyLock<JsonObject> = LazyLock::new(|| DATA["sampleInput"].as_object().unwrap().clone());
pub static HOST_TOOLS: LazyLock<HashSet<String>> = LazyLock::new(|| serde_json::from_value(DATA["hostTools"].clone()).unwrap());
pub static REFERENCE_FIELDS: LazyLock<Vec<String>> = LazyLock::new(|| serde_json::from_value(DATA["referenceFields"].clone()).unwrap());
pub static UNDO_DESCRIPTION: LazyLock<String> = LazyLock::new(|| DATA["undoDescription"].as_str().unwrap().into());
pub const UNDO_TOOL: &str = "undo_change";
pub const EMERGENCY_STOP: &str = "live_session_emergency_stop";
pub fn format_number(value: f64, digits: Option<usize>) -> String {
    number::to_string(number::parse(&number::to_fixed(value, digits.unwrap_or(2))).unwrap_or(f64::NAN))
}
pub fn note_name(note: f64) -> String {
    let index = note % 12.0;
    let notes = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let name = if index >= 0.0 && index < 12.0 && index.fract() == 0.0 { notes[index as usize] } else { "undefined" };
    format!("{name}{}", number::to_string((note / 12.0).floor() - 2.0))
}
pub fn hex_color(value: &Value) -> Option<String> {
    value.as_f64().filter(|v| v.fract() == 0.0 && (0.0..=16777215.0).contains(v)).map(|v| format!("#{:06x}", v as u32))
}
pub(crate) fn record(value: Option<&Value>) -> &JsonObject {
    static EMPTY: LazyLock<JsonObject> = LazyLock::new(JsonObject::new);
    value.and_then(Value::as_object).unwrap_or(&EMPTY)
}
pub(crate) fn array(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}
pub(crate) fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite())
}
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(v)) => *v,
        Some(Value::Number(v)) => v.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(v)) => !v.is_empty(),
        _ => true,
    }
}
fn fallback(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}
fn numeric(value: &Value) -> Value {
    value
        .as_str()
        .filter(|s| !trim(s).is_empty())
        .and_then(number::parse)
        .filter(|n| n.is_finite())
        .map(|n| json!(n))
        .unwrap_or_else(|| value.clone())
}
async fn sample_for(value: Option<&Value>, context: &dyn ChangeContext) -> Result<Result<SampleFile, String>, RuntimeError> {
    Ok(match value{
        Some(Value::String(path))=>context.sample(path).ok_or_else(||"Give the path of an audio file on this computer (one find_sounds returned, a recording, the producer's own), or {\"random\": true, \"words\": [...]} for Kumi to pick one.".into()),
        Some(Value::Object(selector))=>{
            let strings=|key:&str|array(selector.get(key)).iter().filter_map(Value::as_str).map(str::to_owned).collect();
            context.pick(SampleSelector{words:strings("words"),folders:strings("folders"),random:selector.get("random")==Some(&Value::Bool(true))}).await?.ok_or_else(||"No sample matches that; try other words or folders.".into())
        },_=>Err("Give the sample as the path of an audio file (one find_sounds returned, or any on this computer), or {\"random\": true, \"words\": [...]} .".replace("} .","}."))
    })
}
fn is_display_text(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| !trim(s).is_empty() && !number::parse(s).is_some_and(|n| n.is_finite()))
}
async fn convert(
    parameter_ref: Option<&Value>,
    value: Option<&Value>,
    context: &dyn ChangeContext,
) -> Result<Result<Option<Value>, String>, RuntimeError> {
    let Some(reference) = parameter_ref.and_then(Value::as_str).filter(|_| is_display_text(value)) else {
        return Ok(Ok(value.cloned()));
    };
    let value = value.unwrap();
    if !context.has_value_for() {
        return Ok(Err(format!(
            "Give {} as a number in the parameter's range: Kumi can't read this parameter's units here.",
            stringify(value)
        )));
    }
    Ok(context.value_for(reference, value.as_str().unwrap()).await?.map(|n| Some(json!(n))))
}
async fn displayed(mut input: JsonObject, context: &dyn ChangeContext) -> Result<Result<JsonObject, String>, RuntimeError> {
    if input.contains_key("value") && input.get("parameterRef").is_some_and(Value::is_string) {
        match convert(input.get("parameterRef"), input.get("value"), context).await? {
            Ok(Some(value)) => {
                input.insert("value".into(), value);
            }
            Err(why) => return Ok(Err(why)),
            _ => {}
        }
    } else if let Some(values) = input.get("values").and_then(Value::as_array) {
        let mut out = Vec::new();
        for item in values {
            let Some(row) = item.as_object() else {
                out.push(item.clone());
                continue;
            };
            let mut row = row.clone();
            match convert(row.get("parameterRef"), row.get("value"), context).await? {
                Ok(Some(value)) => {
                    row.insert("value".into(), value);
                }
                Err(why) => return Ok(Err(why)),
                _ => {}
            }
            out.push(Value::Object(row));
        }
        input.insert("values".into(), Value::Array(out));
    }
    Ok(Ok(input))
}
async fn resolve_parameters(given: &JsonObject, context: &dyn ChangeContext) -> Result<Result<JsonObject, String>, RuntimeError> {
    let mut input = given.clone();
    if let Some(value) = given.get("value") {
        input.insert("value".into(), numeric(value));
    }
    if let Some(values) = given.get("values").and_then(Value::as_array) {
        input.insert(
            "values".into(),
            Value::Array(
                values
                    .iter()
                    .map(|item| {
                        if let Some(row) = item.as_object().filter(|row| row.contains_key("value")) {
                            let mut row = row.clone();
                            row.insert("value".into(), numeric(&row["value"]));
                            Value::Object(row)
                        } else {
                            item.clone()
                        }
                    })
                    .collect(),
            ),
        );
    }
    let named = input.get("parameter").is_some_and(Value::is_string)
        || array(input.get("values")).iter().any(|v| v.get("parameter").is_some_and(Value::is_string));
    if !named {
        return displayed(input, context).await;
    }
    let Some(device) = input.get("deviceRef").and_then(Value::as_str) else {
        return Ok(Err("Name the device (deviceRef) whose parameter this is.".into()));
    };
    let list = context.parameters(device).await?;
    let find = |name: &str| {
        let wanted = trim(name).to_lowercase();
        list.iter().find(|r| r.name.to_lowercase() == wanted).or_else(|| list.iter().find(|r| r.name.to_lowercase().starts_with(&wanted)))
    };
    let missing = |name: &str| {
        format!(
            "The device has no parameter called {}; its parameters include {}.",
            stringify(&json!(head(name, 64))),
            list.iter().take(12).map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")
        )
    };
    let parameter = input.shift_remove("parameter");
    if let Some(name) = parameter.as_ref().and_then(Value::as_str) {
        let Some(found) = find(name) else {
            return Ok(Err(missing(name)));
        };
        input.insert("parameterRef".into(), json!(found.reference));
        return Ok(Ok(input));
    }
    let mut values = Vec::new();
    for item in array(input.get("values")) {
        let Some(name) = item.get("parameter").and_then(Value::as_str) else {
            values.push(item.clone());
            continue;
        };
        let Some(found) = find(name) else {
            return Ok(Err(missing(name)));
        };
        let mut item = record(Some(item)).clone();
        item.shift_remove("parameter");
        item.insert("parameterRef".into(), json!(found.reference));
        values.push(Value::Object(item));
    }
    input.insert("values".into(), Value::Array(values));
    displayed(input, context).await
}
impl ChangeKind {
    pub fn summarize(
        &self,
        preview: &JsonObject,
        input: &JsonObject,
        track: &dyn Fn(&Value) -> Option<KnownTrack>,
        applied: Option<&JsonObject>,
    ) -> ChangeSummary {
        summaries::base(self, preview, input, track, applied)
            .or_else(|| more_summaries::more(self, preview, input, track, applied))
            .expect("change catalog has a summary for every kind")
    }
    pub fn has(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }
    pub async fn prepare(&self, input: &JsonObject, context: &dyn ChangeContext) -> Result<Result<JsonObject, String>, RuntimeError> {
        let out = match self.tool.as_str() {
            "set_device_parameter" | "set_device_parameters" => return resolve_parameters(input, context).await,
            "load_sample" | "load_sample_to_pad" => {
                let found = match sample_for(input.get("sample"), context).await? {
                    Ok(found) => found,
                    Err(why) => return Ok(Err(why)),
                };
                let mut out = if self.tool == "load_sample" {
                    json!({"action":"insert","trackRef":fallback(input.get("trackRef")),"deviceName":"Simpler","filePath":found.path,"allowedRoot":found.folder})
                } else {
                    json!({"action":"load-sample","deviceRef":fallback(input.get("deviceRef")),"note":fallback(input.get("note")),"filePath":found.path,"allowedRoot":found.folder})
                };
                if self.tool == "load_sample_to_pad" && input.get("instrument").and_then(Value::as_str) == Some("Drum Sampler") {
                    out["instrument"] = json!("Drum Sampler");
                }
                out
            }
            "load_samples_to_pads" => {
                let mut pads = Vec::new();
                for pad in array(input.get("pads")) {
                    let pad = record(Some(pad));
                    let found = match sample_for(pad.get("sample"), context).await? {
                        Ok(found) => found,
                        Err(why) => {
                            return Ok(Err(format!("Pad {}: {why}", finite(pad.get("note")).map(note_name).unwrap_or_else(|| "?".into()))))
                        }
                    };
                    let mut out = json!({"note":fallback(pad.get("note")),"filePath":found.path,"allowedRoot":found.folder});
                    if pad.get("instrument").and_then(Value::as_str) == Some("Drum Sampler") {
                        out["instrument"] = json!("Drum Sampler");
                    }
                    pads.push(out);
                }
                json!({"action":"load-samples","deviceRef":fallback(input.get("deviceRef")),"pads":pads})
            }
            "edit_rack" => {
                let action = input.get("action").and_then(Value::as_str).map(|s| match s {
                    "add-chain" => "insert-chain",
                    "select-variation" => "set",
                    s => s,
                });
                let Some(action) = action.filter(|s| {
                    [
                        "insert-chain",
                        "add-macro",
                        "remove-macro",
                        "randomize-macros",
                        "store-variation",
                        "recall-variation",
                        "delete-variation",
                        "set",
                        "copy-pad",
                    ]
                    .contains(s)
                }) else {
                    return Ok(Err("action is add-chain, add-macro, remove-macro, randomize-macros, store-variation, recall-variation, delete-variation, select-variation or copy-pad.".into()));
                };
                let index = input.get("index").filter(|v| v.is_number());
                if ["recall-variation", "delete-variation", "set"].contains(&action) && index.is_none() {
                    return Ok(Err("Say which variation: index, 0 is the first.".into()));
                }
                if action == "copy-pad"
                    && !(input.get("sourceIndex").is_some_and(Value::is_number) && input.get("targetIndex").is_some_and(Value::is_number))
                {
                    return Ok(Err("copy-pad takes sourceIndex and targetIndex (pad notes).".into()));
                }
                let mut out = json!({"action":action,"rackRef":fallback(input.get("rackRef"))});
                if ["insert-chain", "recall-variation", "delete-variation"].contains(&action) {
                    if let Some(index) = index {
                        out["index"] = index.clone();
                    }
                }
                if action == "set" {
                    out["selectedVariationIndex"] = index.unwrap().clone();
                }
                if action == "copy-pad" {
                    out["sourceIndex"] = input["sourceIndex"].clone();
                    out["targetIndex"] = input["targetIndex"].clone();
                }
                out
            }
            "add_arrangement_clip" => {
                // An audio file as a clip when a sample is given; an empty MIDI clip otherwise.
                let mut out = if input.contains_key("sample") {
                    let found = match sample_for(input.get("sample"), context).await? {
                        Ok(found) => found,
                        Err(why) => return Ok(Err(why)),
                    };
                    json!({"action":"create","kind":"audio","trackRef":fallback(input.get("trackRef")),"position":fallback(input.get("position")),"filePath":found.path})
                } else if finite(input.get("length")).is_some() {
                    json!({"action":"create","kind":"midi","trackRef":fallback(input.get("trackRef")),"position":fallback(input.get("position")),"length":fallback(input.get("length"))})
                } else {
                    return Ok(Err("Say how long the MIDI clip is (length, in beats), or give a sample for an audio clip.".into()));
                };
                if let Some(name) = input.get("name").filter(|v| v.is_string()) {
                    out["name"] = name.clone();
                }
                out
            }
            "set_audio_clip" => {
                // Live's API has no clip fades (12.4.15b5): the bridge would take them and change nothing.
                if input.contains_key("fadeInLength") || input.contains_key("fadeOutLength") {
                    return Ok(Err(
                        "Live's API has no clip fades, so Kumi can't set them: leave fadeInLength and fadeOutLength out.".into()
                    ));
                }
                Value::Object(input.clone())
            }
            "switch_device" => {
                json!({"action":"enable","deviceRef":fallback(input.get("deviceRef")),"enabled":input.get("enabled")==Some(&Value::Bool(true))})
            }
            "move_device" => json!({"action":"move","deviceRef":fallback(input.get("deviceRef")),"index":fallback(input.get("index"))}),
            "move_device_to" => {
                if !truthy(input.get("targetTrackRef")) && !truthy(input.get("targetChainRef")) {
                    return Ok(Err("Say where: targetTrackRef or targetChainRef.".into()));
                }
                let mut out = json!({"action":"move-cross","ref":fallback(input.get("deviceRef"))});
                for key in ["targetTrackRef", "targetChainRef"] {
                    if truthy(input.get(key)) {
                        out[key] = input[key].clone();
                    }
                }
                if let Some(index) = input.get("index").filter(|v| v.is_number()) {
                    out["index"] = index.clone();
                }
                out
            }
            "replace_sample" | "import_audio" => {
                let Some(found) = input.get("sample").and_then(Value::as_str).and_then(|s| context.sample(s)) else {
                    return Ok(Err("Give the path of an audio file on this computer (absolute, or from ~).".into()));
                };
                let mut out = if self.tool == "replace_sample" {
                    json!({"deviceRef":fallback(input.get("deviceRef"))})
                } else {
                    let mut out = input.clone();
                    out.shift_remove("sample");
                    Value::Object(out)
                };
                out["filePath"] = json!(found.path);
                out["allowedRoot"] = json!(found.folder);
                out
            }
            _ => Value::Object(input.clone()),
        };
        Ok(Ok(out.as_object().unwrap().clone()))
    }
    pub async fn explain(&self, error: &str, input: &JsonObject, context: &dyn ChangeContext) -> Result<Option<String>, RuntimeError> {
        static RANGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i:bounds|finite value|value .*required|range)").unwrap());
        if !self.has("explain") || !RANGE.is_match(error) {
            return Ok(None);
        }
        let Some(device) = input.get("deviceRef").and_then(Value::as_str) else {
            return Ok(None);
        };
        let list = context.ranges(device).await?;
        let refs: HashSet<&str> = input
            .get("parameterRef")
            .and_then(Value::as_str)
            .into_iter()
            .chain(array(input.get("values")).iter().filter_map(|v| v.get("parameterRef").and_then(Value::as_str)))
            .collect();
        let asked: Vec<_> = list.iter().filter(|r| refs.contains(r.reference.as_str())).collect();
        let rows = if asked.is_empty() { list.iter().collect::<Vec<_>>() } else { asked };
        let shown: Vec<_> = rows
            .into_iter()
            .take(16)
            .filter_map(|row| {
                Some(format!(
                    "{} takes {} to {}{}",
                    row.name,
                    format_number(row.min?, None),
                    format_number(row.max?, None),
                    row.value
                        .map(|v| format!(
                            " (now {}{})",
                            format_number(v, None),
                            row.display.as_ref().filter(|s| !s.is_empty()).map(|s| format!(", shown as {s}")).unwrap_or_default()
                        ))
                        .unwrap_or_default()
                ))
            })
            .collect();
        Ok((!shown.is_empty())
            .then(|| format!("Values are the parameter's own, between its min and max, not what Live shows: {}.", shown.join("; "))))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Produced {
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: String,
}
impl ChangeKind {
    pub fn schema(&self, schema: &JsonObject) -> JsonObject {
        let mut schema = Value::Object(schema.clone());
        if self.tool == "add_tracks_and_scenes" {
            for list in ["tracks", "scenes"] {
                if let Some(index) = schema
                    .get_mut("properties")
                    .and_then(|p| p.get_mut(list))
                    .and_then(|v| v.get_mut("items"))
                    .and_then(|v| v.get_mut("properties"))
                    .and_then(|v| v.get_mut("index"))
                    .and_then(Value::as_object_mut)
                {
                    index.insert("description".into(), json!("Position, 0 is first. Leave it out to add after the last one."));
                }
            }
        } else if self.tool == "write_midi_clip" || self.tool == "write_arrangement_clip" {
            // Notes in Kumi's notation instead, with the length (and an Arrangement clip's start) taken from them.
            let notation = json!({"type":"string","minLength":1,"maxLength":1_000_000,"description":"The notes in Kumi's notation (see the instructions), instead of notes"});
            let relax = |object: &mut JsonObject| {
                if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                    properties.insert("notation".into(), notation.clone());
                }
                if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                    required.retain(|field| !matches!(field.as_str(), Some("notes" | "length" | "start")));
                }
            };
            if let Some(object) = schema.as_object_mut() {
                relax(object);
            }
            if let Some(items) = schema.pointer_mut("/properties/clips/items").and_then(Value::as_object_mut) {
                relax(items);
            }
        } else if self.tool == "set_device_parameter" {
            let name = json!({"type":"string","minLength":1,"maxLength":128,"description":"The parameter's name on the device, as Live shows it; instead of parameterRef"});
            let value = json!({"anyOf":[{"type":"number"},{"type":"string","minLength":1,"maxLength":48}],"description":"A number in the parameter's range, or the value as the device shows it (\"800 Hz\", \"-6 dB\", \"35 %\", \"1.2 s\", \"Saw\")"});
            let mut properties = record(schema.get("properties")).clone();
            properties.insert("parameter".into(), name.clone());
            if properties.contains_key("value") {
                properties.insert("value".into(), value.clone());
            }
            if let Some(values) = properties.get_mut("values").and_then(Value::as_object_mut) {
                if let Some(items) = values.get_mut("items").and_then(Value::as_object_mut) {
                    let mut props = record(items.get("properties")).clone();
                    props.insert("parameter".into(), name);
                    if props.contains_key("value") {
                        props.insert("value".into(), value);
                    }
                    let required = array(items.get("required")).iter().filter(|v| v.as_str() != Some("parameterRef")).cloned().collect();
                    items.insert("properties".into(), Value::Object(props));
                    items.insert("required".into(), Value::Array(required));
                }
            }
            schema["properties"] = Value::Object(properties);
        } else if self.tool == "set_audio_clip" {
            // Live's API has no clip fades (12.4.15b5): the bridge's fields for them change nothing.
            if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                properties.shift_remove("fadeInLength");
                properties.shift_remove("fadeOutLength");
            }
        }
        schema.as_object().unwrap().clone()
    }
    pub fn produces(&self, applied: &JsonObject) -> Option<Produced> {
        let (reference, kind) = match self.tool.as_str() {
            "add_tracks_and_scenes" => (
                array(applied.get("created"))
                    .iter()
                    .find(|v| v.get("kind").and_then(Value::as_str) == Some("track") && v.get("ref").is_some_and(Value::is_string))
                    .and_then(|v| v.get("ref"))
                    .and_then(Value::as_str),
                "track",
            ),
            "write_midi_clip" => (applied.get("clipRef").and_then(Value::as_str), "session-clip"),
            "load_sample" => (record(applied.get("result")).get("ref").and_then(Value::as_str), "device"),
            "load_device" => (
                applied
                    .get("deviceRef")
                    .filter(|v| !v.is_null())
                    .or_else(|| record(applied.get("created")).get("deviceRef"))
                    .and_then(Value::as_str),
                "device",
            ),
            "edit_rack" => (applied.get("chainRef").and_then(Value::as_str), "chain"),
            "duplicate_device" => (record(applied.get("created")).get("ref").and_then(Value::as_str), "device"),
            "duplicate_clip" => {
                let reference = record(applied.get("created")).get("ref").and_then(Value::as_str);
                (reference, if reference.is_some_and(|s| s.contains(":arrangement_clip:")) { "arrangement-clip" } else { "session-clip" })
            }
            _ => return None,
        };
        reference.map(|reference| Produced { reference: reference.into(), kind: kind.into() })
    }
    pub fn permanent(&self, input: &JsonObject) -> Option<String> {
        let action = input.get("action").and_then(Value::as_str);
        let why = match self.tool.as_str() {
            "edit_rack" => match action {
                Some("insert-chain") => "Live gives Kumi no way to take a chain away again; delete it in Live if you don't want it.",
                Some("delete-variation") => "Live gives Kumi no way to bring a deleted variation back.",
                _ => return None,
            },
            "edit_clip" => "Live gives Kumi no way to take this back; use Live's own undo if you need to.",
            "edit_notes" if action == Some("select") => "Selecting notes changes no notes: there's nothing to undo.",
            "change_structure" if action == Some("delete-return") => {
                "Live gives Kumi no way to bring a deleted return track back; use Live's own undo if you need to."
            }
            "delete_device" => "Live gives Kumi no way to bring a deleted device back; use Live's own undo if you need to.",
            "delete_clip" => "Kumi can't bring a deleted clip back; Live's own undo can.",
            "delete_scene" => "Kumi can't bring a deleted scene back; Live's own undo can.",
            "delete_track" => "Kumi can't bring a deleted track back; Live's own undo can.",
            "delete_locator" => "Kumi can't bring a deleted locator back; Live's own undo can.",
            "clear_range" => "Kumi can't put back what it cleared; Live's own undo can.",
            "edit_device"
                if action
                    .is_some_and(|s| ["slice-clear", "slice-reset", "warp-as", "warp-double", "warp-half", "modulate"].contains(&s)) =>
            {
                "Live gives Kumi no way to put this back; Live's own undo can."
            }
            _ => return None,
        };
        Some(why.into())
    }
}
/// The clips a new Arrangement clip on [start, end) lands on, as Live lays it over them (cutting them, as a drop does):
/// each one's name and span, the part it replaces (`from`–`to`), and whether all of it goes. The summary names them.
pub fn laid_over(clips: &[JsonObject], start: f64, end: f64) -> Vec<Value> {
    clips
        .iter()
        .filter_map(|clip| {
            let other_start = clip.get("start").and_then(Value::as_f64)?;
            let other_end = clip.get("endTime").and_then(Value::as_f64).or_else(|| Some(other_start + clip.get("length")?.as_f64()?))?;
            (other_start < end - 1e-6 && other_end > start + 1e-6).then(|| {
                json!({
                    "name": clip.get("name").and_then(Value::as_str).unwrap_or(""),
                    "start": other_start,
                    "end": other_end,
                    "from": other_start.max(start),
                    "to": other_end.min(end),
                    "whole": other_start >= start - 1e-6 && other_end <= end + 1e-6
                })
            })
        })
        .collect()
}
impl ChangeKind {
    /// Why Kumi can't take back part of an applied change, from its preview: an Arrangement move or new clip that
    /// replaced what was in its place.
    pub fn replaced(&self, preview: &JsonObject) -> Option<String> {
        if !matches!(self.tool.as_str(), "move_clip" | "add_arrangement_clip")
            || !preview.get("replaces").and_then(Value::as_array).is_some_and(|r| !r.is_empty())
        {
            return None;
        }
        if self.tool == "add_arrangement_clip" {
            return Some("Kumi can't bring back what the new clip replaced; Live's own undo can.".into());
        }
        // Kumi's Live extension cuts an audio clip first, and each cut is a step of its own in Live's undo.
        let cuts = preview.get("payload").and_then(|p| p.get("clearFirst")).and_then(Value::as_array).map_or(0, Vec::len);
        Some(match cuts {
            0 => "Kumi can't bring back what the move replaced; Live's own undo can.".into(),
            1 => "Kumi can't bring back what the move replaced; Live's own undo can, in two steps (the move, then the cut).".into(),
            n => {
                format!("Kumi can't bring back what the move replaced; Live's own undo can, in {} steps (the move, then each cut).", n + 1)
            }
        })
    }
}
pub fn undo_note(message: &str) -> String {
    let message = message.to_ascii_lowercase();
    let groups:[(&[&str],&str);8]=[
        (&["created session structure was modified","session structure changed before deletion"],"It changed after Kumi made it (something recorded, loaded or routed on it), so Kumi left it: deleting it would lose that. Delete it in Live if you don't need it."),
        (&["epoch","connection"],"Live restarted or reconnected since, so Kumi can't undo this."),
        (&["doesn't offer"],"Live doesn't offer what it was routed from before any more (an input with no audio device, or a track that no longer makes sound), so Kumi left it. Change it in Live if you need to."),
        (&["highest positional authority"],"A track Kumi made after it is still there, so Kumi left this one. Delete it in Live if you don't need it."),
        (&["stop playback first"],"Stop playback, then undo it: putting the playhead back while playing would be heard."),
        (&["unknown","expired","not found"],"Kumi can't undo this anymore."),
        (&["changed","modified","refused","postcondition","no longer","fingerprint","revision","identity","mismatch"],"It changed in Live since, so Kumi left it as it is."),
        (&["momentary","structural","not undoable"],"Live gives Kumi no way to take this back; change it in Live if you need to.")
    ];
    groups
        .into_iter()
        .find(|(patterns, _)| patterns.iter().any(|p| message.contains(p)))
        .map(|(_, why)| why)
        .unwrap_or("Live didn't accept the undo, so Kumi left it as it is.")
        .into()
}
pub fn next_change_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("c{}", COUNTER.fetch_add(1, Ordering::Relaxed) + 1)
}
pub fn new_record(kind: &ChangeKind, summary: ChangeSummary, state: ChangeState, at: i64) -> ChangeRecord {
    ChangeRecord {
        id: next_change_id(),
        family: kind.family,
        title: summary.title,
        track: summary.track,
        from: summary.from,
        to: summary.to,
        range: summary.range,
        colors: summary.colors,
        clip: summary.clip,
        devices: summary.devices,
        state,
        score: None,
        note: None,
        at,
    }
}
