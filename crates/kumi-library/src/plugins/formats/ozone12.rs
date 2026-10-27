//! Ozone 12 (iZotope): the main plug-in and its module plug-ins (Maximizer, Equalizer, Dynamics …).
//!
//! A preset is XML. Its root names the era and the plug-in ("Ozone9Maximizer", "Ozone10", "OzoneMS"), with
//! `PresetVer` and `PluginVer`. Each module is an element with `Enabled`, holding `<Param ElementID ParamID
//! Value>` for its settings and `<ExtraBytes ElementID Data>` (base64) for what isn't a number. The global
//! section's "ElementChain" bytes list the modules in signal order.
//!
//! The state a host saves is something else: a 16-byte header (a constant, 4, the length of what follows the
//! first 12 bytes, the decoded length; each a u32 LE) and a zlib stream of JSON in which every value is typed
//! (`{"Type": "Float", "Value": 0.5}`): "Context State" (the window), "DSP State" → "DSP Elements" (every
//! module's settings by the same ElementIDs and ParamIDs a preset uses, plus more), and its version.

use std::collections::BTreeSet;
use std::io::{Read, Write};

use base64::Engine;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde_json::{json, Value};

use super::tree::Node;
use super::vstpreset::VstPreset;
use super::xml::{self, Document, Element};
use super::{FormatError, MAX_DECODED_BYTES};

/// The first u32 of every Ozone 12 state seen.
pub const STATE_MAGIC: u32 = 0x0068_8ade;
pub const MAIN_CLASS_ID: &str = "5653545A424F5A4F7A6F6E6500000000";

/// A VST3 build whose initialized component state was captured and loaded again.
/// The main suite and each standalone module have distinct class IDs and templates.
pub struct OzoneVariant {
    pub class_id: &'static str,
    pub name: &'static str,
    pub state_name: &'static str,
    init: &'static [u8],
}

macro_rules! ozone_variant {
    ($id:literal, $name:literal, $state:literal, $file:literal) => {
        OzoneVariant {
            class_id: $id,
            name: $name,
            state_name: $state,
            init: include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/ozone-12/format-1/fixtures/defaults/", $file)),
        }
    };
}

/// The VST3 suite and its twenty standalone modules, verified with Ozone 12.1.0.
pub const VARIANTS: &[OzoneVariant] = &[
    ozone_variant!("5653545A424F5A4F7A6F6E6500000000", "Ozone 12", "Ozone", "ozone-12.vstpreset"),
    ozone_variant!("5653545A4242414F7A6F6E652050726F", "Ozone 12 Bass Control", "Bass Control", "ozone-12-bass-control.vstpreset"),
    ozone_variant!("5653545A42434C4F7A6F6E652050726F", "Ozone 12 Clarity", "Clarity", "ozone-12-clarity.vstpreset"),
    ozone_variant!("5653545A42595A4F7A6F6E652050726F", "Ozone 12 Dynamic EQ", "Dynamic EQ", "ozone-12-dynamic-eq.vstpreset"),
    ozone_variant!("5653545A42445A4F7A6F6E652050726F", "Ozone 12 Dynamics", "Dynamics", "ozone-12-dynamics.vstpreset"),
    ozone_variant!("5653545A425A554F7A6F6E652050726F", "Ozone 12 Equalizer", "EQ", "ozone-12-equalizer.vstpreset"),
    ozone_variant!("5653545A425A584F7A6F6E652050726F", "Ozone 12 Exciter", "Exciter", "ozone-12-exciter.vstpreset"),
    ozone_variant!("5653545A42495A4F7A6F6E652050726F", "Ozone 12 Imager", "Imager  ", "ozone-12-imager.vstpreset"),
    ozone_variant!("5653545A424F494F7A6F6E652050726F", "Ozone 12 Impact", "Impact  ", "ozone-12-impact.vstpreset"),
    ozone_variant!("5653545A425A4C4F7A6F6E652050726F", "Ozone 12 Low End Focus", "Low End Focus", "ozone-12-low-end-focus.vstpreset"),
    ozone_variant!(
        "5653545A425A524F7A6F6E652050726F",
        "Ozone 12 Master Rebalance",
        "Master Rebalance",
        "ozone-12-master-rebalance.vstpreset"
    ),
    ozone_variant!("5653545A425A484F7A6F6E652050726F", "Ozone 12 Match EQ", "Match EQ", "ozone-12-match-eq.vstpreset"),
    ozone_variant!("5653545A425A4D4F7A6F6E652050726F", "Ozone 12 Maximizer", "Maximizer", "ozone-12-maximizer.vstpreset"),
    ozone_variant!("5653545A425A534F7A6F6E652050726F", "Ozone 12 Spectral Shaper", "Spectral Shaper", "ozone-12-spectral-shaper.vstpreset"),
    ozone_variant!("5653545A424F534F7A6F6E652050726F", "Ozone 12 Stabilizer", "Stabilizer  ", "ozone-12-stabilizer.vstpreset"),
    ozone_variant!("5653545A4253514F7A6F6E652050726F", "Ozone 12 Stem EQ", "Stem EQ", "ozone-12-stem-eq.vstpreset"),
    ozone_variant!("5653545A42554C4F7A6F6E652050726F", "Ozone 12 Unlimiter", "Unlimiter", "ozone-12-unlimiter.vstpreset"),
    ozone_variant!(
        "5653545A425A434F7A6F6E652050726F",
        "Ozone 12 Vintage Compressor",
        "Vintage Compressor",
        "ozone-12-vintage-compressor.vstpreset"
    ),
    ozone_variant!("5653545A425A514F7A6F6E652050726F", "Ozone 12 Vintage EQ", "Vintage EQ", "ozone-12-vintage-eq.vstpreset"),
    ozone_variant!("5653545A425A564F7A6F6E652050726F", "Ozone 12 Vintage Limiter", "Vintage Limiter", "ozone-12-vintage-limiter.vstpreset"),
    ozone_variant!("5653545A425A544F7A6F6E652050726F", "Ozone 12 Vintage Tape", "Vintage Tape", "ozone-12-vintage-tape.vstpreset"),
];

/// Match a VST3 class ID; a module name alone cannot identify its state format.
pub fn variant(class_id: &str) -> Option<&'static OzoneVariant> {
    VARIANTS.iter().find(|item| item.class_id.eq_ignore_ascii_case(class_id))
}

/// Processing modules, using the state file's ElementIDs rather than the UI labels.
pub const MODULES: &[&str] = &[
    "Bass Control",
    "Clarity",
    "Dynamic EQ",
    "Dynamics",
    "Equalizer",
    "Post Equalizer",
    "Exciter",
    "Stereo Imager",
    "Impact",
    "Low End Focus",
    "Master Rebalance",
    "Match Equalizer",
    "Maximizer",
    "Spectral Shaper",
    "Stabilizer",
    "Stem EQ",
    "Unlimiter",
    "Vintage Compressor",
    "Vintage EQ",
    "Vintage Limiter",
    "Vintage Tape",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OzoneParam {
    pub element: String,
    pub id: String,
    /// As written; most are numbers.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OzoneModule {
    /// The preset's tag for it ("Maximizer", "EQ2", "DynamicEQ", "Global").
    pub tag: String,
    pub enabled: Option<bool>,
    pub params: Vec<OzoneParam>,
    /// Data that isn't a number, by ElementID.
    pub extra: Vec<(String, Vec<u8>)>,
}

/// One module in the signal chain, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainEntry {
    /// The byte before each name; always 0 in the presets seen.
    pub flag: u8,
    /// The module's ElementID ("Maximizer", "Low End Focus").
    pub name: String,
}

/// An Ozone preset file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OzonePreset {
    pub document: Document,
}

impl OzonePreset {
    pub fn read(bytes: &[u8]) -> Result<OzonePreset, FormatError> {
        let text = std::str::from_utf8(bytes).map_err(|_| FormatError::new("Ozone preset isn't UTF-8"))?;
        let document = xml::parse(text)?;
        if !document.root.name.starts_with("Ozone") {
            return Err(FormatError::new(format!("<{}> isn't an Ozone preset", document.root.name)));
        }
        Ok(OzonePreset { document })
    }

    pub fn write(&self) -> Vec<u8> {
        xml::write(&self.document).into_bytes()
    }

    /// The root's tag: the era and plug-in that wrote it.
    pub fn root(&self) -> &str {
        &self.document.root.name
    }

    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.document.root.attribute(name)
    }

    /// XML identifies the producing build, unlike a host component state. Reads retain older presets, but edits and
    /// import into a VST3 state must not silently reinterpret settings from a different generation.
    pub fn validate_write_version(&self) -> Result<(), FormatError> {
        if self.attribute("PluginVer") != Some("120100")
            || self.attribute("PluginBuild") != Some("1440")
            || self.attribute("PresetVer") != Some("6")
        {
            return Err(FormatError::new("Ozone XML writes require verified PluginVer 120100, PluginBuild 1440 and PresetVer 6"));
        }
        Ok(())
    }

    pub fn modules(&self) -> Vec<OzoneModule> {
        self.document.root.children.iter().map(module).collect()
    }

    /// A setting's value as a number.
    pub fn value(&self, element: &str, id: &str) -> Option<f64> {
        self.modules()
            .into_iter()
            .flat_map(|m| m.params)
            .find(|p| p.element == element && p.id == id)
            .and_then(|p| p.value.trim().parse().ok())
    }

    /// Change a numeric XML setting without losing other parameters or extra data.
    /// A sparse preset may omit a default: insert only fields present in Ozone 12's init state.
    pub fn set_value(&mut self, element: &str, id: &str, value: f64) -> Result<(), FormatError> {
        self.validate_write_version()?;
        if !value.is_finite() {
            return Err(FormatError::new("Ozone preset values must be finite"));
        }
        let init = OzoneState::init(MAIN_CLASS_ID)?;
        numeric_setting(&init, element, id, value)?;
        let module = self
            .document
            .root
            .children
            .iter_mut()
            .find(|module| module.children.iter().any(|child| child.attribute("ElementID") == Some(element)))
            .ok_or_else(|| FormatError::new(format!("Ozone preset has no element {element}")))?;
        if let Some(param) = module
            .children
            .iter_mut()
            .find(|child| child.name == "Param" && child.attribute("ElementID") == Some(element) && child.attribute("ParamID") == Some(id))
        {
            set_attribute(param, "Value", value.to_string());
            return Ok(());
        }
        module.children.push(Element {
            name: "Param".into(),
            attributes: vec![("ElementID".into(), element.into()), ("ParamID".into(), id.into()), ("Value".into(), value.to_string())],
            children: Vec::new(),
            text: String::new(),
        });
        Ok(())
    }

    /// The modules in signal order, from the global "ElementChain" bytes.
    pub fn chain(&self) -> Result<Option<Vec<ChainEntry>>, FormatError> {
        let bytes = self.modules().into_iter().flat_map(|m| m.extra).find(|(element, _)| element == "ElementChain").map(|(_, bytes)| bytes);
        bytes.map(|bytes| chain(&bytes)).transpose()
    }

    /// The tree surveys read: each module's settings as "<ElementID>:<ParamID>" numbers, its extra data by length.
    pub fn to_node(&self) -> Node {
        let mut entries: Vec<(String, Node)> =
            self.document.root.attributes.iter().map(|(k, v)| (format!("@{k}"), Node::Text(v.clone()))).collect();
        for module in self.modules() {
            let mut fields = Vec::new();
            if let Some(enabled) = module.enabled {
                fields.push(("@Enabled".to_string(), Node::Bool(enabled)));
            }
            for param in module.params {
                let value = param.value.trim().parse::<f64>().map(Node::Float).unwrap_or(Node::Text(param.value));
                fields.push((format!("{}:{}", param.element, param.id), value));
            }
            for (element, bytes) in module.extra {
                fields.push((format!("{element}:bytes"), Node::Bytes(bytes.len())));
            }
            entries.push((module.tag, Node::Map(fields)));
        }
        Node::Map(entries)
    }
}

fn module(element: &Element) -> OzoneModule {
    let mut params = Vec::new();
    let mut extra = Vec::new();
    for child in &element.children {
        let id = child.attribute("ElementID").unwrap_or_default().to_string();
        match child.name.as_str() {
            "Param" => params.push(OzoneParam {
                element: id,
                id: child.attribute("ParamID").unwrap_or_default().to_string(),
                value: child.attribute("Value").unwrap_or_default().to_string(),
            }),
            "ExtraBytes" => {
                let data = child.attribute("Data").unwrap_or_default();
                extra.push((id, base64::engine::general_purpose::STANDARD.decode(data).unwrap_or_default()));
            }
            _ => {}
        }
    }
    OzoneModule { tag: element.name.clone(), enabled: element.attribute("Enabled").map(|v| v != "0"), params, extra }
}

/// The chain's bytes: for each module a flag byte, a u32 LE length and its name.
pub fn chain(bytes: &[u8]) -> Result<Vec<ChainEntry>, FormatError> {
    let mut entries = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let header = bytes.get(at..at + 5).ok_or_else(|| FormatError::new("Ozone chain ends inside an entry"))?;
        let len = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
        let name = bytes.get(at + 5..at + 5 + len).ok_or_else(|| FormatError::new("Ozone chain name runs past its end"))?;
        entries.push(ChainEntry { flag: header[0], name: String::from_utf8_lossy(name).into_owned() });
        at += 5 + len;
    }
    Ok(entries)
}

/// The state a host saved for Ozone 12 or one of its modules.
#[derive(Debug, Clone, PartialEq)]
pub struct OzoneState {
    /// The second u32 of the header (4 so far).
    pub version: u32,
    /// The typed JSON as written.
    pub json: Value,
}

impl OzoneState {
    /// A new preset starts with the selected plug-in's own initialized state.
    /// No installed plug-in or factory library is required to create the file.
    pub fn init(class_id: &str) -> Result<Self, FormatError> {
        let variant = variant(class_id).ok_or_else(|| FormatError::new("Unknown Ozone 12 VST3 class ID"))?;
        let preset = VstPreset::read(variant.init)?;
        Self::read(preset.component().ok_or_else(|| FormatError::new("Ozone init preset has no component state"))?)
    }

    pub fn read(bytes: &[u8]) -> Result<OzoneState, FormatError> {
        let word = |i: usize| bytes.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let (Some(magic), Some(version), Some(rest), Some(decoded_len)) = (word(0), word(1), word(2), word(3)) else {
            return Err(FormatError::new("Ozone state is shorter than its header"));
        };
        if magic != STATE_MAGIC {
            return Err(FormatError::new(format!("Ozone state starts {magic:#x}, not {STATE_MAGIC:#x}")));
        }
        if rest as usize != bytes.len() - 12 {
            return Err(FormatError::new(format!("Ozone state says {rest} bytes follow, {} do", bytes.len() - 12)));
        }
        let decoded_len = decoded_len as usize;
        if decoded_len > MAX_DECODED_BYTES {
            return Err(FormatError::new(format!("Ozone state would decode to {decoded_len} bytes")));
        }
        let mut decoded = Vec::with_capacity(decoded_len);
        ZlibDecoder::new(&bytes[16..])
            .take(decoded_len as u64 + 1)
            .read_to_end(&mut decoded)
            .map_err(|error| FormatError::new(format!("Ozone state isn't zlib: {error}")))?;
        if decoded.len() != decoded_len {
            return Err(FormatError::new(format!("Ozone state decodes to {} bytes, its header says {decoded_len}", decoded.len())));
        }
        let json: Value = serde_json::from_slice(&decoded).map_err(|error| FormatError::new(format!("Ozone state isn't JSON: {error}")))?;
        Ok(OzoneState { version, json })
    }

    /// Write the component state consumed by IComponent::setState, preserving every untouched field.
    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        self.validate_write_schema()?;
        let raw = serde_json::to_vec(&self.json).map_err(|error| FormatError::new(format!("Ozone JSON: {error}")))?;
        if raw.len() > MAX_DECODED_BYTES {
            return Err(FormatError::new("Ozone state exceeds the decoded size limit"));
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw).map_err(|error| FormatError::new(format!("Ozone compression: {error}")))?;
        let compressed = encoder.finish().map_err(|error| FormatError::new(format!("Ozone compression: {error}")))?;
        let mut bytes = Vec::with_capacity(16 + compressed.len());
        for word in [STATE_MAGIC, self.version, (compressed.len() + 4) as u32, raw.len() as u32] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&compressed);
        Ok(bytes)
    }

    /// Host state contains serialization markers, not the plug-in's product version. Check those markers against the
    /// observed initialized variant instead of treating a caller-supplied product version as evidence about the source.
    /// Unknown non-structural fields remain untouched; a different known module schema is refused.
    pub fn validate_write_schema(&self) -> Result<(), FormatError> {
        if self.version != 4 {
            return Err(FormatError::new(format!("Ozone state version {} has not been verified for writing", self.version)));
        }
        for field in ["Major Version", "Minor Version"] {
            if self.json[field]["Type"] != "UInt" || self.json[field]["Value"].as_u64() != Some(1) {
                return Err(FormatError::new("Ozone component writes require the verified 1.1 serialization schema"));
            }
        }
        if self.json["Context State"]["Type"] != "Dictionary"
            || self.json["DSP State"]["Type"] != "Dictionary"
            || self.json["DSP State"]["Value"]["DSP Elements"]["Type"] != "Dictionary"
        {
            return Err(FormatError::new("Ozone state does not have the verified dictionary layout"));
        }
        let name = typed(&self.json["Context State"]).and_then(|c| c.get("Plugin Name")).and_then(typed).and_then(Value::as_str);
        let variant = VARIANTS
            .iter()
            .find(|v| Some(v.state_name) == name)
            .ok_or_else(|| FormatError::new("Ozone component has no verified plug-in identity"))?;
        let reference = Self::init(variant.class_id)?;
        let given = self.elements().ok_or_else(|| FormatError::new("Ozone state has no DSP Elements dictionary"))?;
        for (element, expected) in reference.elements().into_iter().flatten() {
            let schema = typed(expected).and_then(|e| e.get("Param Version"));
            let actual = given.get(element).and_then(typed).and_then(|e| e.get("Param Version"));
            if schema != actual || given.get(element).and_then(|node| node.get("Type")) != expected.get("Type") {
                return Err(FormatError::new(format!("Ozone {element} uses an unverified parameter schema")));
            }
            for (setting, expected) in typed(expected).and_then(Value::as_object).into_iter().flatten() {
                let actual = given.get(element).and_then(typed).and_then(|e| e.get(setting));
                if actual.and_then(|value| value.get("Type")) != expected.get("Type") {
                    return Err(FormatError::new(format!("Ozone {element}/{setting} uses an unverified setting type")));
                }
            }
        }
        Ok(())
    }

    /// Check the exact VST3 variant without applying write-only schema restrictions to readable older state.
    pub fn validate_class(&self, class_id: &str) -> Result<(), FormatError> {
        let variant = variant(class_id).ok_or_else(|| FormatError::new("Unknown Ozone 12 VST3 class ID"))?;
        let name =
            typed(&self.json["Context State"]).and_then(|context| context.get("Plugin Name")).and_then(typed).and_then(Value::as_str);
        if name != Some(variant.state_name) {
            return Err(FormatError::new(format!("Ozone state belongs to {name:?}, not {}", variant.name)));
        }
        Ok(())
    }

    /// Package a state for the exact VST3 variant. Module state cannot be relabelled as a different plug-in.
    pub fn to_vstpreset(&self, class_id: &str) -> Result<VstPreset, FormatError> {
        self.validate_class(class_id)?;
        Ok(VstPreset {
            version: 1,
            class_id: class_id.to_ascii_uppercase(),
            chunks: vec![("Comp".into(), self.write()?), ("Cont".into(), Vec::new())],
        })
    }

    /// Every module's settings ("DSP State" → "DSP Elements"), by ElementID.
    pub fn elements(&self) -> Option<&serde_json::Map<String, Value>> {
        typed(&self.json["DSP State"])?.get("DSP Elements").and_then(typed)?.as_object()
    }

    /// One setting's value, untyped.
    pub fn value(&self, element: &str, id: &str) -> Option<&Value> {
        typed(self.elements()?.get(element)?)?.get(id).and_then(typed)
    }

    /// Change a real DSP setting, retaining its declared type and all unrelated state.
    /// Float values are stored with the plug-in's single-precision semantics.
    pub fn set_value(&mut self, element: &str, id: &str, value: Value) -> Result<(), FormatError> {
        if matches!(id, "Element Name" | "Param Version" | "Extra Bytes") {
            return Err(FormatError::new(format!("Ozone {id} is structural state, not a numeric setting")));
        }
        let node = self
            .elements_mut()?
            .get_mut(element)
            .and_then(typed_mut)
            .and_then(|value| value.get_mut(id))
            .ok_or_else(|| FormatError::new(format!("Ozone has no setting {element}/{id}")))?;
        let kind = node.get("Type").and_then(Value::as_str).unwrap_or_default();
        let converted = match kind {
            "Bool" if value.is_boolean() => value,
            "UInt" if value.as_u64().is_some_and(|value| value <= u32::MAX as u64) => value,
            "Int" if value.as_i64().is_some_and(|value| i32::try_from(value).is_ok()) => value,
            "Float" => {
                let number = value
                    .as_f64()
                    .filter(|value| value.is_finite() && (*value as f32).is_finite())
                    .ok_or_else(|| FormatError::new(format!("Ozone {element}/{id} requires a finite float")))?;
                json!((number as f32) as f64)
            }
            _ => return Err(FormatError::new(format!("Ozone {element}/{id} requires {kind}, not {value}"))),
        };
        *node.get_mut("Value").ok_or_else(|| FormatError::new("Ozone typed setting has no Value"))? = converted;
        Ok(())
    }

    fn elements_mut(&mut self) -> Result<&mut serde_json::Map<String, Value>, FormatError> {
        self.json
            .get_mut("DSP State")
            .and_then(typed_mut)
            .and_then(|state| state.get_mut("DSP Elements"))
            .and_then(typed_mut)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| FormatError::new("Ozone state has no DSP Elements dictionary"))
    }

    /// Add, remove or reorder processing modules in the suite. A standalone module keeps its own identity.
    pub fn set_chain(&mut self, modules: &[&str]) -> Result<(), FormatError> {
        let name =
            typed(&self.json["Context State"]).and_then(|context| context.get("Plugin Name")).and_then(typed).and_then(Value::as_str);
        if name != Some("Ozone") {
            let current = self.chain()?.unwrap_or_default();
            if current.len() != modules.len() || current.iter().zip(modules).any(|(entry, name)| entry.name != *name) {
                return Err(FormatError::new("A standalone Ozone module's signal chain cannot be changed"));
            }
        }
        let available = self.elements().ok_or_else(|| FormatError::new("Ozone state has no DSP Elements dictionary"))?;
        let mut unique = BTreeSet::new();
        let mut bytes = Vec::new();
        for name in modules {
            if !MODULES.contains(name) || !available.contains_key(*name) {
                return Err(FormatError::new(format!("Ozone state has no processing module {name}")));
            }
            if !unique.insert(*name) {
                return Err(FormatError::new(format!("Ozone chain contains {name} more than once")));
            }
            bytes.push(0);
            bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
        }
        self.set_extra("ElementChain", &bytes)
    }

    fn set_extra(&mut self, element: &str, bytes: &[u8]) -> Result<(), FormatError> {
        let entry = self
            .elements_mut()?
            .get_mut(element)
            .and_then(typed_mut)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| FormatError::new(format!("Ozone state has no element {element}")))?;
        entry.insert("Extra Bytes".into(), json!({"Type": "Base64", "Value": base64::engine::general_purpose::STANDARD.encode(bytes)}));
        Ok(())
    }

    /// Import a sparse XML preset into this variant's state. Unknown legacy settings are errors;
    /// the operation is atomic, so an incompatible preset cannot leave half its changes behind.
    pub fn apply_preset(&mut self, preset: &OzonePreset) -> Result<(), FormatError> {
        preset.validate_write_version()?;
        self.validate_write_schema()?;
        for extra in preset.document.root.children.iter().flat_map(|module| &module.children).filter(|child| child.name == "ExtraBytes") {
            base64::engine::general_purpose::STANDARD
                .decode(extra.attribute("Data").unwrap_or_default())
                .map_err(|_| FormatError::new("Ozone preset contains invalid base64 extra data"))?;
        }
        let mut updated = self.clone();
        for module in preset.modules() {
            if module.enabled == Some(false) {
                continue;
            }
            for param in module.params {
                let number: f64 =
                    param.value.parse().map_err(|_| FormatError::new(format!("Ozone {}/{} isn't numeric", param.element, param.id)))?;
                let value = numeric_setting(&updated, &param.element, &param.id, number)?;
                updated.set_value(&param.element, &param.id, value)?;
            }
            for (element, bytes) in module.extra {
                if element == "ElementChain" {
                    let entries = chain(&bytes)?;
                    if entries.iter().any(|entry| entry.flag != 0) {
                        return Err(FormatError::new("Ozone chain uses an unverified flag"));
                    }
                    updated.set_chain(&entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>())?;
                } else if !bytes.is_empty() {
                    updated.set_extra(&element, &bytes)?;
                }
            }
        }
        *self = updated;
        Ok(())
    }

    /// The modules in signal order.
    pub fn chain(&self) -> Result<Option<Vec<ChainEntry>>, FormatError> {
        let Some(data) = self.value("ElementChain", "Extra Bytes").and_then(Value::as_str) else {
            return Ok(None);
        };
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| FormatError::new("Ozone chain isn't base64"))?;
        chain(&bytes).map(Some)
    }

    /// The typed JSON as a tree: each `{Type, Value}` becomes its value.
    pub fn to_node(&self) -> Node {
        typed_node(&self.json)
    }
}

/// A typed entry's Value.
fn typed(entry: &Value) -> Option<&Value> {
    entry.get("Type")?;
    entry.get("Value")
}

fn typed_mut(entry: &mut Value) -> Option<&mut Value> {
    entry.get("Type")?;
    entry.get_mut("Value")
}

fn numeric_setting(state: &OzoneState, element: &str, id: &str, value: f64) -> Result<Value, FormatError> {
    let node = state
        .elements()
        .and_then(|elements| elements.get(element))
        .and_then(typed)
        .and_then(|element| element.get(id))
        .ok_or_else(|| FormatError::new(format!("Ozone has no setting {element}/{id}")))?;
    if !value.is_finite() {
        return Err(FormatError::new("Ozone preset values must be finite"));
    }
    match node.get("Type").and_then(Value::as_str) {
        Some("Bool") if value == 0.0 || value == 1.0 => Ok(json!(value == 1.0)),
        Some("UInt") if value.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&value) => Ok(json!(value as u32)),
        Some("Int") if value.fract() == 0.0 && (i32::MIN as f64..=i32::MAX as f64).contains(&value) => Ok(json!(value as i32)),
        Some("Float") if (value as f32).is_finite() => Ok(json!((value as f32) as f64)),
        _ => Err(FormatError::new(format!("Ozone {element}/{id} cannot take numeric value {value}"))),
    }
}

fn set_attribute(element: &mut Element, key: &str, value: String) {
    if let Some((_, current)) = element.attributes.iter_mut().find(|(name, _)| name == key) {
        *current = value;
    } else {
        element.attributes.push((key.into(), value));
    }
}

fn typed_node(value: &Value) -> Node {
    match (value.get("Type").and_then(Value::as_str), value.get("Value")) {
        (Some("Dictionary"), Some(Value::Object(map))) => Node::Map(map.iter().map(|(k, v)| (k.clone(), typed_node(v))).collect()),
        (Some("Array"), Some(Value::Array(items))) => Node::List(items.iter().map(typed_node).collect()),
        (Some("Base64"), Some(Value::String(data))) => {
            Node::Bytes(base64::engine::general_purpose::STANDARD.decode(data).map_or(0, |b| b.len()))
        }
        // Its own type decides: a Float written as 0 is still a float.
        (Some("Float"), Some(inner)) if inner.is_number() => Node::Float(inner.as_f64().unwrap_or(f64::NAN)),
        (Some(_), Some(inner)) => Node::from(inner),
        _ => match value {
            Value::Object(map) => Node::Map(map.iter().map(|(k, v)| (k.clone(), typed_node(v))).collect()),
            other => Node::from(other),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    #[test]
    fn reads_a_module_preset() {
        let xml = "<?xml version=\"1.0\" standalone=\"yes\" ?>\n<Ozone9Maximizer PresetVer=\"1\" PluginVer=\"9030\">\n    <Global Enabled=\"0\">\n        <ExtraBytes ElementID=\"ElementChain\" Data=\"AAkAAABNYXhpbWl6ZXI=\" />\n    </Global>\n    <Maximizer Enabled=\"1\">\n        <Param ElementID=\"Maximizer\" ParamID=\"Threshold\" Value=\"-2.22614670\" />\n    </Maximizer>\n</Ozone9Maximizer>\n";
        let preset = OzonePreset::read(xml.as_bytes()).unwrap();
        assert_eq!(preset.root(), "Ozone9Maximizer");
        assert_eq!(preset.attribute("PluginVer"), Some("9030"));
        assert_eq!(preset.value("Maximizer", "Threshold"), Some(-2.2261467));
        assert_eq!(preset.chain().unwrap(), Some(vec![ChainEntry { flag: 0, name: "Maximizer".to_string() }]));
        assert_eq!(preset.modules()[1].enabled, Some(true));
        assert_eq!(OzonePreset::read(&preset.write()).unwrap(), preset);
        assert!(OzonePreset::read(b"<Preset/>").is_err());
        assert!(chain(&[0, 9, 0, 0, 0, b'M']).is_err());
    }

    fn state(json: &str) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(json.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut out = Vec::new();
        for word in [STATE_MAGIC, 4, (compressed.len() + 4) as u32, json.len() as u32] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&compressed);
        out
    }

    #[test]
    fn reads_a_state() {
        let json = r#"{"DSP State":{"Type":"Dictionary","Value":{"DSP Elements":{"Type":"Dictionary","Value":{
            "ElementChain":{"Type":"Dictionary","Value":{"Extra Bytes":{"Type":"Base64","Value":"AAkAAABNYXhpbWl6ZXI="}}},
            "Maximizer":{"Type":"Dictionary","Value":{"Threshold":{"Type":"Float","Value":-2.5},"Bypass":{"Type":"Bool","Value":false}}}}}}},
            "Major Version":{"Type":"UInt","Value":1}}"#;
        let read = OzoneState::read(&state(json)).unwrap();
        assert_eq!(read.version, 4);
        assert_eq!(read.value("Maximizer", "Threshold").and_then(Value::as_f64), Some(-2.5));
        assert_eq!(read.chain().unwrap().unwrap()[0].name, "Maximizer");
        let node = read.to_node();
        assert_eq!(node.get("Major Version"), Some(&Node::Int(1)));
        assert_eq!(
            node.get("DSP State")
                .and_then(|n| n.get("DSP Elements"))
                .and_then(|n| n.get("ElementChain"))
                .and_then(|n| n.get("Extra Bytes")),
            Some(&Node::Bytes(14))
        );
        let bytes = state(json);
        assert!(OzoneState::read(&bytes[..bytes.len() - 1]).is_err());
        let mut wrong = bytes.clone();
        wrong[0] = 0;
        assert!(OzoneState::read(&wrong).is_err());
        assert!(OzoneState::read(&bytes[..10]).is_err());
    }

    #[test]
    fn creates_presets_for_the_suite_and_every_standalone_module() {
        assert_eq!(VARIANTS.len(), 21);
        let mut ids = BTreeSet::new();
        for variant in VARIANTS {
            assert!(ids.insert(variant.class_id));
            let state = OzoneState::init(variant.class_id).unwrap();
            assert!(state.elements().is_some_and(|elements| !elements.is_empty()));
            let preset = state.to_vstpreset(variant.class_id).unwrap();
            assert_eq!(preset.class_id, variant.class_id);
            let parsed = VstPreset::read(&preset.write()).unwrap();
            assert_eq!(OzoneState::read(parsed.component().unwrap()).unwrap(), state, "{}", variant.name);
            assert_eq!(parsed.controller(), Some(&[][..]));
        }
        assert!(OzoneState::init("unknown").is_err());
        let maximizer = OzoneState::init("5653545A425A4D4F7A6F6E652050726F").unwrap();
        assert!(maximizer.to_vstpreset(MAIN_CLASS_ID).is_err());
    }

    #[test]
    fn changes_gain_width_true_peak_and_chain_without_losing_other_state() {
        let mut state = OzoneState::init(MAIN_CLASS_ID).unwrap();
        let original = state.clone();
        state.json["future field"] = json!({"Type":"Dictionary","Value":{"opaque":[1,2,"three"]}});
        state.set_chain(&["Equalizer", "Stereo Imager", "Maximizer"]).unwrap();
        state.set_value("Maximizer", "Gain", json!(3)).unwrap();
        state.set_value("Maximizer", "Margin", json!(-1)).unwrap();
        state.set_value("Maximizer", "Prevent Intersample Clipping", json!(true)).unwrap();
        state.set_value("Stereo Imager", "Band 1 Width Percent", json!(10)).unwrap();
        let reread = OzoneState::read(&state.write().unwrap()).unwrap();
        assert_eq!(reread.value("Maximizer", "Gain"), Some(&json!(3.0)));
        assert_eq!(reread.value("Maximizer", "Prevent Intersample Clipping"), Some(&json!(true)));
        assert_eq!(reread.value("Stereo Imager", "Band 1 Width Percent"), Some(&json!(10.0)));
        assert_eq!(reread.json["future field"], state.json["future field"]);
        assert_eq!(reread.json["Context State"], original.json["Context State"]);
        assert_eq!(reread.elements().unwrap()["Dither"], original.elements().unwrap()["Dither"]);
        assert_eq!(
            reread.chain().unwrap().unwrap().iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            ["Equalizer", "Stereo Imager", "Maximizer"]
        );
    }

    #[test]
    fn refuses_invalid_types_structural_changes_and_cross_module_chains() {
        let mut state = OzoneState::init(MAIN_CLASS_ID).unwrap();
        let original = state.clone();
        for (id, value) in [
            ("Gain", json!("3")),
            ("Mode", json!(-1)),
            ("Mode", json!(1.5)),
            ("Bypass", json!(1)),
            ("Param Version", json!(2)),
            ("missing", json!(0)),
        ] {
            assert!(state.set_value("Maximizer", id, value).is_err());
            assert_eq!(state, original);
        }
        assert!(state.set_value("missing", "Gain", json!(3)).is_err());
        assert!(state.set_value("Maximizer", "Gain", json!(f64::MAX)).is_err());
        assert!(state.set_chain(&["Maximizer", "Maximizer"]).is_err());
        assert!(state.set_chain(&["Global"]).is_err());
        assert_eq!(state, original);
        let mut standalone = OzoneState::init("5653545A425A4D4F7A6F6E652050726F").unwrap();
        assert!(standalone.set_chain(&["Maximizer", "Equalizer"]).is_err());
        assert!(standalone.set_chain(&[]).is_err());
        standalone.set_chain(&["Maximizer"]).unwrap();
        state.version = 5;
        assert!(state.write().is_err());
        state = original.clone();
        state.json["Minor Version"]["Value"] = json!(2);
        assert!(state.write().is_err());
        state = original;
        state.json["DSP State"]["Value"]["DSP Elements"]["Value"]["Maximizer"]["Value"]["Param Version"]["Value"] = json!(99);
        assert!(state.write().is_err());
        state = OzoneState::init(MAIN_CLASS_ID).unwrap();
        state.json["DSP State"]["Value"]["DSP Elements"]["Value"]["Maximizer"]["Value"]["Gain"]["Type"] = json!("UInt");
        assert!(state.write().is_err());
        state = OzoneState::init(MAIN_CLASS_ID).unwrap();
        state.json["DSP State"]["Type"] = json!("UInt");
        assert!(state.write().is_err());
    }

    #[test]
    fn imports_sparse_xml_and_rejects_incompatible_presets_atomically() {
        let xml = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/ozone-12/format-1/fixtures/init-maximizer.xml"));
        let mut preset = OzonePreset::read(xml).unwrap();
        preset.set_value("Maximizer", "Gain", 3.0).unwrap();
        preset.set_value("Maximizer", "Prevent Intersample Clipping", 1.0).unwrap();
        assert_eq!(preset.value("Maximizer", "Gain"), Some(3.0));
        assert_eq!(OzonePreset::read(&preset.write()).unwrap(), preset);
        let mut state = OzoneState::init("5653545A425A4D4F7A6F6E652050726F").unwrap();
        state.apply_preset(&preset).unwrap();
        assert_eq!(state.value("Maximizer", "Gain"), Some(&json!(3.0)));
        assert_eq!(state.value("Maximizer", "Prevent Intersample Clipping"), Some(&json!(true)));
        let original = state.clone();
        let unsupported = OzonePreset::read(br#"<OzoneMS PluginVer="120100" PluginBuild="1440" PresetVer="6"><Maximizer Enabled="1"><Param ElementID="Maximizer" ParamID="Gain" Value="6"/><Param ElementID="Maximizer" ParamID="UnrecognizedLegacyField" Value="1"/></Maximizer></OzoneMS>"#).unwrap();
        assert!(state.apply_preset(&unsupported).is_err());
        assert_eq!(state, original);
        let invalid_extra =
            OzonePreset::read(br#"<OzoneMS PluginVer="120100" PluginBuild="1440" PresetVer="6"><Global><ExtraBytes ElementID="ElementChain" Data="!bad!"/></Global></OzoneMS>"#).unwrap();
        assert!(state.apply_preset(&invalid_extra).is_err());
        assert_eq!(state, original);
        assert!(preset.set_value("Maximizer", "missing", 1.0).is_err());
        assert!(preset.set_value("Maximizer", "Gain", f64::INFINITY).is_err());
        assert!(preset.set_value("Maximizer", "Prevent Intersample Clipping", 0.5).is_err());
        set_attribute(&mut preset.document.root, "PluginVer", "130000".into());
        assert!(preset.set_value("Maximizer", "Gain", 1.0).is_err());
        assert!(state.apply_preset(&preset).is_err());
        assert_eq!(state, original);
    }
}
