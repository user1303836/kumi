//! Serum's preset and VST3 state are different documents. Translate their synthesis and editor
//! sections explicitly; handing a .SerumPreset straight to IComponent silently initializes Serum.

use serde_json::{json, Value as Json};

use super::cbor::{Float, Head, Length, Value};
use super::serum2::{SerumPreset, SerumState, XferFile, ZSTD};
use super::structure;
use super::tree::PathRule;
use super::vstpreset::VstPreset;
use super::FormatError;

pub const CLASS_ID: &str = "56534558667350736572756D20320000";

fn map() -> Value {
    Value::Map(Vec::new(), Length::Definite(Head::Inline))
}
fn put(tree: &mut Value, key: &str, value: Value) {
    if let Value::Map(entries, _) = tree {
        if let Some((_, old)) = entries.iter_mut().find(|(k, _)| k.as_str() == Some(key)) {
            *old = value;
        } else {
            entries.push((Value::text(key), value));
        }
    }
}
fn take(tree: &mut Value, keys: &[&str]) {
    if let Value::Map(entries, _) = tree {
        entries.retain(|(key, _)| !keys.contains(&key.as_str().unwrap_or_default()));
    }
}
fn float(n: f64) -> Value {
    Value::Float(Float::Double(n.to_bits()))
}
fn numbered(key: &str, stem: &str) -> bool {
    key.strip_prefix(stem).is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}
fn expand(tree: &mut Value) {
    match tree {
        Value::Map(entries, _) => {
            for (key, value) in entries {
                if key.as_str() == Some("plainParams") && value.as_str() == Some("default") {
                    *value = map();
                } else {
                    expand(value);
                }
            }
        }
        Value::Array(items, _) => {
            for item in items {
                expand(item);
            }
        }
        _ => {}
    }
}
fn container(mut header: Json, body: Value, component: &str) -> Result<XferFile, FormatError> {
    let object = header.as_object_mut().ok_or_else(|| FormatError::new("Serum header must be an object"))?;
    object.remove("fileType");
    object.insert("component".into(), json!(component));
    object.insert("hash".into(), json!(""));
    Ok(XferFile { header: serde_json::to_vec(&header).map_err(|e| FormatError::new(e.to_string()))?, encoding: ZSTD, body, hash_ok: None })
}

impl SerumPreset {
    /// Start from Serum's own initialized patch, without copying any factory sound.
    pub fn init(name: &str) -> Result<Self, FormatError> {
        let mut preset =
            Self::read(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.SerumPreset")))?;
        preset.rename(name)?;
        Ok(preset)
    }

    pub fn rename(&mut self, name: &str) -> Result<(), FormatError> {
        if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err(FormatError::new("Serum preset name must be 1–256 bytes without control characters"));
        }
        let mut header = self.header();
        header["presetName"] = json!(name);
        self.file.header = serde_json::to_vec(&header).map_err(|e| FormatError::new(e.to_string()))?;
        put(&mut self.file.body, "presetName", Value::text(name));
        Ok(())
    }

    /// Change a documented scalar path in its stored units. Missing defaults are expanded only at plainParams.
    pub fn set(&mut self, path: &str, value: &Json) -> Result<(), FormatError> {
        if matches!(path, "productVersion" | "version" | "component" | "fileType") {
            return Err(FormatError::new("Serum format and product version markers are not editable controls"));
        }
        if path == "presetName" {
            return self.rename(value.as_str().ok_or_else(|| FormatError::new("presetName must be text"))?);
        }
        let parts: Vec<&str> = path.split('/').collect();
        if parts.is_empty() || parts.len() > 32 || parts.iter().any(|p| p.is_empty()) {
            return Err(FormatError::new("invalid Serum field path"));
        }
        let mut canonical = String::new();
        for (i, part) in parts.iter().enumerate() {
            if part.parse::<usize>().is_ok() {
                canonical.push_str("[]");
            } else {
                let segment = PathRule::NumberedSections.segment(&canonical, part, i + 1 < parts.len());
                if !canonical.is_empty() {
                    canonical.push('/');
                }
                canonical.push_str(&segment);
            }
        }
        let format = structure::builtin("serum-2").ok_or_else(|| FormatError::new("Serum format missing"))?;
        let field = format.structure.field(&canonical).ok_or_else(|| FormatError::new(format!("unknown Serum field {path}")))?;
        let replacement = match value {
            Json::Number(n) if field.kind.split('|').any(|k| matches!(k, "int" | "float")) => {
                let number = n.as_f64().filter(|n| n.is_finite()).ok_or_else(|| FormatError::new("Serum value must be finite"))?;
                if field.range.is_some_and(|[lo, hi]| number < lo || number > hi) {
                    return Err(FormatError::new(format!("{path} is outside its range")));
                }
                if field.kind == "int" {
                    let integer = n.as_i64().ok_or_else(|| FormatError::new(format!("{path} requires an integer")))?;
                    if integer >= 0 {
                        Value::Unsigned(integer as u64, Head::U64)
                    } else {
                        Value::Negative((-1 - integer) as u64, Head::U64)
                    }
                } else {
                    float(number)
                }
            }
            Json::String(text) if field.kind.split('|').any(|k| k == "text") && text.len() <= 4096 => Value::text(text),
            Json::Bool(on) if field.kind == "bool" => Value::Simple(if *on { 21 } else { 20 }, Head::Inline),
            _ => return Err(FormatError::new(format!("{path} requires a {} value", field.kind))),
        };
        let mut edited = self.file.body.clone();
        set_path(&mut edited, &parts, replacement)?;
        self.file.body = edited;
        Ok(())
    }

    /// Add the initialized Distortion effect, whose slot type and state are captured in the fixture.
    pub fn add_distortion(&mut self, rack: usize) -> Result<(), FormatError> {
        if rack > 2 {
            return Err(FormatError::new("Serum has FX racks 0, 1 and 2"));
        }
        let fixture = VstPreset::read(include_bytes!(concat!(
            env!("KUMI_WORKSPACE"),
            "/plugin-formats/serum-2/format-1/fixtures/init-distortion.vstpreset"
        )))?;
        let state =
            SerumState::read(fixture.component().ok_or_else(|| FormatError::new("missing Serum processor"))?, fixture.controller())?;
        let fx = state.processor.body.at("FXRack0/FX").cloned().ok_or_else(|| FormatError::new("missing Distortion template"))?;
        let Value::Array(mut effects, _) = fx else {
            return Err(FormatError::new("invalid Distortion template"));
        };
        let Value::Map(sections, _) = &mut self.file.body else {
            return Err(FormatError::new("Serum preset body must be a map"));
        };
        let name = format!("FXRack{rack}");
        let section = sections
            .iter_mut()
            .find(|(k, _)| k.as_str() == Some(&name))
            .map(|(_, v)| v)
            .ok_or_else(|| FormatError::new("FX rack is missing"))?;
        let Value::Map(fields, _) = section else {
            return Err(FormatError::new("FX rack must be a map"));
        };
        let list = fields
            .iter_mut()
            .find(|(k, _)| k.as_str() == Some("FX"))
            .map(|(_, v)| v)
            .ok_or_else(|| FormatError::new("FX rack has no slots"))?;
        let Value::Array(slots, _) = list else {
            return Err(FormatError::new("FX slots must be a list"));
        };
        if slots.len() >= 16 {
            return Err(FormatError::new("Serum FX rack is full"));
        }
        slots.append(&mut effects);
        Ok(())
    }

    /// Preserve original version markers: Serum uses them to migrate old patches when it restores the state.
    pub fn to_state(&self) -> Result<SerumState, FormatError> {
        if self.file.hash_ok == Some(false) {
            return Err(FormatError::new("Serum preset checksum does not match"));
        }
        let Value::Map(sections, _) = &self.file.body else {
            return Err(FormatError::new("Serum preset body must be a map"));
        };
        let header = self.header();
        let product = self
            .file
            .body
            .get("productVersion")
            .and_then(Value::as_str)
            .or_else(|| header["productVersion"].as_str())
            .ok_or_else(|| FormatError::new("Serum preset is missing productVersion"))?;
        let version = self
            .file
            .body
            .get("version")
            .and_then(Value::as_f64)
            .or_else(|| header["version"].as_f64())
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| FormatError::new("Serum preset is missing its format version"))?;
        const EDITOR: &[&str] = &[
            "ClipPlayer",
            "Filter",
            "SerumGUI",
            "GranularOsc",
            "MultiSampleOsc",
            "Osc",
            "SpectralOsc",
            "WTOsc",
            "arpBankDisplayName",
            "clipBankDisplayName",
            "presetName",
            "presetAuthor",
            "presetDescription",
        ];
        let mut processor = self.file.body.clone();
        expand(&mut processor);
        take(&mut processor, EDITOR);
        take(&mut processor, &["fileType"]);
        let mut controller = map();
        for (key, value) in sections {
            let Some(name) = key.as_str() else { continue };
            if EDITOR.contains(&name) {
                put(&mut controller, name, value.clone());
            } else if numbered(name, "Macro") {
                let mut entry = map();
                if let Some(v) = value.get("name") {
                    put(&mut entry, "name", v.clone());
                }
                if let Some(v) = value.at("plainParams/kParamValue").and_then(Value::as_f64) {
                    if !(0.0..=100.0).contains(&v) {
                        return Err(FormatError::new("Serum macro value is outside 0–100"));
                    }
                    put(&mut entry, "default", float(v / 100.0));
                }
                put(&mut controller, name, entry);
            } else if numbered(name, "MidiClip") {
                let mut entry = value.clone();
                if let Value::Map(fields, _) = &mut entry {
                    fields.retain(|(k, _)| MIDI_EDITOR.contains(&k.as_str().unwrap_or_default()));
                }
                put(&mut controller, name, entry);
            } else if numbered(name, "FXRack") {
                let mut entry = map();
                put(&mut entry, "displayName", value.get("displayName").cloned().unwrap_or_else(|| Value::text("")));
                if let Some(Value::Array(slots, length)) = value.get("FX") {
                    let mut defaults = Vec::new();
                    for slot in slots {
                        let mut reset = map();
                        if let Value::Map(fields, _) = slot {
                            for (key, module) in fields {
                                if key.as_str().is_some_and(|k| k.starts_with("FX")) {
                                    if let Some(Value::Map(params, _)) = module.get("plainParams") {
                                        for (key, v) in params {
                                            if let (Some(key), Some(n)) = (key.as_str(), v.as_f64()) {
                                                if n.is_finite() {
                                                    put(&mut reset, key, float(n));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let mut settings = map();
                        put(&mut settings, "defaultPlainValues", reset);
                        put(&mut settings, "kUIParamMixOrGain", slot.get("kUIParamMixOrGain").cloned().unwrap_or_else(|| float(0.0)));
                        defaults.push(settings);
                    }
                    put(&mut entry, "FX", Value::Array(defaults, *length));
                }
                put(&mut controller, name, entry);
            } else if numbered(name, "LFO") {
                let mut entry = map();
                if let Some(v) = value.get("curveDisplayName") {
                    put(&mut entry, "curveDisplayName", v.clone());
                }
                put(&mut controller, name, entry);
            } else if numbered(name, "ArpClip") {
                put(&mut controller, name, map());
            } else if numbered(name, "VoicePanel") {
                let mut entry = map();
                put(&mut entry, "displayName", value.get("displayName").cloned().unwrap_or_else(|| Value::text("")));
                put(&mut controller, name, entry);
            }
        }
        if let Value::Map(sections, _) = &mut processor {
            for (key, section) in sections {
                let name = key.as_str().unwrap_or_default();
                if numbered(name, "Macro") {
                    take(section, &["name"]);
                }
                if numbered(name, "FXRack") {
                    take(section, &["displayName"]);
                }
                if numbered(name, "MidiClip") {
                    take(section, MIDI_EDITOR);
                }
                if numbered(name, "Arp") && section.get("activeClip").is_none() {
                    put(section, "activeClip", Value::Unsigned(0, Head::Inline));
                }
            }
        }
        for (tree, component) in [(&mut processor, "processor"), (&mut controller, "controller")] {
            put(tree, "component", Value::text(component));
            put(tree, "productVersion", Value::text(product));
            put(tree, "version", float(version));
        }
        put(&mut controller, "presetHasBeenEdited", Value::Simple(20, Head::Inline));
        let mut header = header.clone();
        header["productVersion"] = json!(product);
        header["version"] = json!(version);
        Ok(SerumState {
            processor: container(header.clone(), processor, "processor")?,
            controller: Some(container(header, controller, "controller")?),
        })
    }

    pub fn to_vstpreset(&self) -> Result<VstPreset, FormatError> {
        let state = self.to_state()?;
        Ok(VstPreset {
            version: 1,
            class_id: CLASS_ID.into(),
            chunks: vec![
                ("Comp".into(), state.processor.write()?),
                ("Cont".into(), state.controller.as_ref().ok_or_else(|| FormatError::new("missing controller"))?.write()?),
            ],
        })
    }
}

const MIDI_EDITOR: &[&str] = &["displayLength_Beats", "gridWidth_Beats", "gridYOffset_Rows", "laneTabs", "name"];

impl SerumState {
    /// Reconstitute the editable preset document. A caller editing a saved host state should retain
    /// the original halves and overlay the translated edits, so unknown controller fields survive.
    pub fn to_preset(&self) -> Result<SerumPreset, FormatError> {
        if self.processor.hash_ok == Some(false) || self.controller.as_ref().is_some_and(|c| c.hash_ok == Some(false)) {
            return Err(FormatError::new("Serum state checksum does not match"));
        }
        let mut header = self.processor.header_json()?;
        if let Some(controller) = &self.controller {
            let ui = controller.header_json()?;
            for key in ["presetName", "presetAuthor", "presetDescription", "tags"] {
                if let Some(value) = ui.get(key) {
                    header[key] = value.clone();
                }
            }
        }
        let object = header.as_object_mut().ok_or_else(|| FormatError::new("Serum header must be an object"))?;
        object.remove("component");
        object.insert("fileType".into(), json!("SerumPreset"));
        object.insert("hash".into(), json!(""));
        let mut body = self.merged();
        take(&mut body, &["component"]);
        put(&mut body, "fileType", Value::text("SerumPreset"));
        Ok(SerumPreset {
            file: XferFile {
                header: serde_json::to_vec(&header).map_err(|e| FormatError::new(e.to_string()))?,
                encoding: self.processor.encoding,
                body,
                hash_ok: None,
            },
        })
    }
}

fn set_path(tree: &mut Value, path: &[&str], replacement: Value) -> Result<(), FormatError> {
    let fail = || FormatError::new(format!("Serum section {} is missing or is not editable", path[0]));
    match tree {
        Value::Map(entries, _) => {
            if path.len() == 1 {
                put(tree, path[0], replacement);
                return Ok(());
            }
            let value = entries.iter_mut().find(|(key, _)| key.as_str() == Some(path[0])).map(|(_, value)| value).ok_or_else(fail)?;
            if path[0] == "plainParams" && value.as_str() == Some("default") {
                *value = map();
            }
            set_path(value, &path[1..], replacement)
        }
        Value::Array(items, _) => {
            let index = path[0].parse::<usize>().map_err(|_| fail())?;
            let item = items.get_mut(index).ok_or_else(fail)?;
            if path.len() == 1 {
                *item = replacement;
                Ok(())
            } else {
                set_path(item, &path[1..], replacement)
            }
        }
        _ => Err(fail()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_two_oscillators_and_an_effect_in_both_state_halves() {
        let mut patch = SerumPreset::init("Kumi test patch").unwrap();
        patch.set("Oscillator1/plainParams/kParamEnable", &json!(1)).unwrap();
        patch.set("Oscillator1/plainParams/kParamVolume", &json!(0.45)).unwrap();
        patch.set("Oscillator1/plainParams/kParamCoarsePit", &json!(12)).unwrap();
        patch.add_distortion(0).unwrap();
        let preset = VstPreset::read(&patch.to_vstpreset().unwrap().write()).unwrap();
        assert_eq!(preset.class_id, CLASS_ID);
        let state = SerumState::read(preset.component().unwrap(), preset.controller()).unwrap();
        assert_eq!(state.processor.hash_ok, Some(true));
        assert_eq!(state.controller.as_ref().unwrap().hash_ok, Some(true));
        assert_eq!(state.processor.body.at("Oscillator1/plainParams/kParamVolume").and_then(Value::as_f64), Some(0.45));
        let Value::Array(slots, _) = state.processor.body.at("FXRack0/FX").unwrap() else { panic!("slots") };
        assert_eq!(slots.len(), 1);
        assert!(slots[0].get("FXDistortion").is_some());
        assert_eq!(state.preset_name().as_deref(), Some("Kumi test patch"));
        assert_eq!(state.to_preset().unwrap().section("Oscillator1"), patch.section("Oscillator1"));
    }

    #[test]
    fn refuses_unknown_fields_types_and_corrupt_state_without_partial_edits() {
        let mut patch = SerumPreset::init("Test").unwrap();
        let original = patch.clone();
        assert!(patch.set("Oscillator1/plainParams/not-a-control", &json!(0.5)).is_err());
        assert!(patch.set("Oscillator1/plainParams/kParamVolume", &json!("loud")).is_err());
        assert!(patch.add_distortion(3).is_err());
        assert!(patch.set("productVersion", &json!("3.0.0")).is_err());
        assert_eq!(patch, original);
        let mut state = patch.to_state().unwrap();
        state.processor.hash_ok = Some(false);
        assert!(state.to_preset().is_err());
        patch.file.hash_ok = Some(false);
        assert!(patch.to_vstpreset().is_err());
    }
}
