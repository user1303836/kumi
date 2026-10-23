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

use std::io::Read;

use base64::Engine;
use flate2::read::ZlibDecoder;
use serde_json::Value;

use super::tree::Node;
use super::xml::{self, Document, Element};
use super::{FormatError, MAX_DECODED_BYTES};

/// The first u32 of every Ozone 12 state seen.
pub const STATE_MAGIC: u32 = 0x0068_8ade;

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

    /// Every module's settings ("DSP State" → "DSP Elements"), by ElementID.
    pub fn elements(&self) -> Option<&serde_json::Map<String, Value>> {
        typed(&self.json["DSP State"])?.get("DSP Elements").and_then(typed)?.as_object()
    }

    /// One setting's value, untyped.
    pub fn value(&self, element: &str, id: &str) -> Option<&Value> {
        typed(self.elements()?.get(element)?)?.get(id).and_then(typed)
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
}
