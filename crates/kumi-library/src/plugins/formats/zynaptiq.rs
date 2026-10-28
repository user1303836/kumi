//! Zynaptiq's XML presets and VST3 component state. Component state is the same
//! document prefixed by its little-endian byte count, including a trailing NUL.
//! Parameter values are in the plug-in's own units, not normalized VST3 values.

use serde_json::Value;

use super::{tree::Node, xml, FormatError, MAX_DECODED_BYTES};

// The largest observed product has 207 parameters. Keep headroom without
// allowing an untrusted count to size an arbitrary allocation.
const MAX_PARAMETERS: usize = 1024;
const MAX_PRESET_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub document: xml::Document,
}

impl Preset {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) {
            return Err(error("document is too large"));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| error("document is not UTF-8"))?;
        if text.contains('\0') {
            return Err(error("NUL inside the XML document"));
        }
        preflight_count(text)?;
        let preset = Self { document: xml::parse(text)? };
        preset.validate()?;
        Ok(preset)
    }

    fn validate(&self) -> Result<(), FormatError> {
        let root = &self.document.root;
        if root.name != "Preset" || !matches!(root.attribute("version"), Some("3" | "4")) {
            return Err(error("expected a Preset document, version 3 or 4"));
        }
        if root.children.iter().filter(|child| child.name == "inputParams").count() != 1 {
            return Err(error("expected exactly one inputParams element"));
        }
        let parameters = root.child("inputParams").ok_or_else(|| error("missing inputParams"))?;
        if parameters.attributes.iter().filter(|(key, _)| key == "count").count() != 1 {
            return Err(error("expected one parameter count"));
        }
        let count = parameters.attribute("count").and_then(|s| s.parse::<usize>().ok()).ok_or_else(|| error("invalid parameter count"))?;
        if count > MAX_PARAMETERS {
            return Err(error("too many parameters"));
        }
        let mut seen = vec![false; count];
        for (name, value) in &parameters.attributes {
            let Some(id) = name.strip_prefix('p').and_then(|value| value.parse::<usize>().ok()) else { continue };
            if name != &format!("p{id}") {
                return Err(error("noncanonical parameter ID"));
            }
            if id >= count {
                return Err(error("parameter ID exceeds the declared count"));
            }
            if !value.parse::<f64>().is_ok_and(f64::is_finite) {
                return Err(error(&format!("parameter {name} is not finite")));
            }
            if std::mem::replace(&mut seen[id], true) {
                return Err(error(&format!("duplicate parameter {name}")));
            }
        }
        if seen.iter().any(|found| !found) {
            return Err(error("a parameter is missing"));
        }
        Ok(())
    }

    pub fn parameter(&self, id: u32) -> Option<f64> {
        self.document.root.child("inputParams")?.attribute(&format!("p{id}"))?.parse().ok()
    }

    /// Creates a named copy from explicitly supplied native state. The caller
    /// must retain the source product identity: native XML has no VST3 class ID.
    pub fn from_template(template: &Self, name: &str) -> Result<Self, FormatError> {
        template.validate()?;
        let mut result = template.clone();
        result.set("@progName", &Value::String(name.to_string()))?;
        Ok(result)
    }

    pub fn set_parameter(&mut self, id: u32, value: f64) -> Result<(), FormatError> {
        if !value.is_finite() {
            return Err(error("parameter value must be finite"));
        }
        let count = self.document.root.child("inputParams").and_then(|p| p.attribute("count")).and_then(|s| s.parse::<u32>().ok());
        if count.is_none_or(|count| id >= count) {
            return Err(error("unknown parameter ID"));
        }
        let key = format!("p{id}");
        let parameter = self
            .document
            .root
            .children
            .iter_mut()
            .find(|c| c.name == "inputParams")
            .and_then(|p| p.attributes.iter_mut().find(|(name, _)| name == &key))
            .ok_or_else(|| error(&format!("unknown parameter {key}")))?;
        parameter.1 = value.to_string();
        Ok(())
    }

    /// Paths returned by `to_node`: `inputParams/@p4` and `@progName`.
    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        if path == "@progName" {
            let name = value.as_str().filter(|s| !s.contains('\0')).ok_or_else(|| error("preset name must be text without NUL"))?;
            let attribute = self.document.root.attributes.iter_mut().find(|(key, _)| key == "progName");
            if let Some((_, old)) = attribute {
                *old = name.to_string();
            } else {
                self.document.root.attributes.push(("progName".into(), name.into()));
            }
            return Ok(());
        }
        let id = path
            .strip_prefix("inputParams/@p")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| error("expected inputParams/@p<number> or @progName"))?;
        self.set_parameter(id, value.as_f64().ok_or_else(|| error("parameter value must be a number"))?)
    }

    pub fn to_node(&self) -> Node {
        let mut root = self.document.root.to_node();
        if let Node::Map(children) = &mut root {
            if let Some((_, Node::Map(parameters))) = children.iter_mut().find(|(name, _)| name == "inputParams") {
                for (name, value) in parameters {
                    if name.starts_with("@p") {
                        if let Node::Text(text) = value {
                            if let Ok(number) = text.parse::<f64>() {
                                *value = Node::Float(number);
                            }
                        }
                    }
                }
            }
        }
        root
    }

    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let bytes = xml::write(&self.document).into_bytes();
        if bytes.len() > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) {
            return Err(error("document is too large"));
        }
        Ok(bytes)
    }

    pub fn component_state(&self) -> Result<Vec<u8>, FormatError> {
        let mut document = self.write()?;
        document.push(0);
        let mut state = (document.len() as u32).to_le_bytes().to_vec();
        state.extend(document);
        Ok(state)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub preset: Preset,
}

impl State {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        let length =
            bytes.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| error("cut-off component length"))?;
        if length == 0 || length > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) || length != bytes.len() - 4 || bytes.last() != Some(&0) {
            return Err(error("component length or terminator is invalid"));
        }
        Ok(Self { preset: Preset::read(&bytes[4..bytes.len() - 1])? })
    }

    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        self.preset.component_state()
    }

    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        self.preset.set(path, value)
    }

    pub fn to_node(&self) -> Node {
        self.preset.to_node()
    }
}

/// Check the direct inputParams start tag without allocating an XML tree. The
/// full XML parser subsequently checks names, nesting, entities and duplicates.
fn preflight_count(text: &str) -> Result<(), FormatError> {
    let mut rest = text;
    let mut depth = 0usize;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").ok_or_else(|| error("unclosed XML comment"))?;
            rest = &rest[end + 3..];
            continue;
        }
        if rest.starts_with("<?") {
            let end = rest.find("?>").ok_or_else(|| error("unclosed XML declaration"))?;
            rest = &rest[end + 2..];
            continue;
        }
        let mut quote = None;
        let end = rest
            .bytes()
            .enumerate()
            .find_map(|(at, byte)| {
                if let Some(current) = quote {
                    if byte == current {
                        quote = None;
                    }
                } else if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                } else if byte == b'>' {
                    return Some(at);
                }
                None
            })
            .ok_or_else(|| error("unclosed XML tag"))?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if tag.starts_with('/') {
            depth = depth.saturating_sub(1);
            continue;
        }
        let name_end = tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len());
        if depth == 1 && &tag[..name_end] == "inputParams" {
            let mut attributes = &tag[name_end..];
            let mut count = None;
            let mut fields = 0;
            loop {
                attributes = attributes.trim_start();
                if attributes.is_empty() || attributes == "/" {
                    break;
                }
                fields += 1;
                if fields > MAX_PARAMETERS + 1 {
                    return Err(error("too many inputParams attributes"));
                }
                let key_end =
                    attributes.find(|c: char| c.is_whitespace() || c == '=').ok_or_else(|| error("invalid parameter attribute"))?;
                let key = &attributes[..key_end];
                attributes =
                    attributes[key_end..].trim_start().strip_prefix('=').ok_or_else(|| error("invalid parameter attribute"))?.trim_start();
                let delimiter =
                    attributes.chars().next().filter(|c| matches!(c, '\'' | '"')).ok_or_else(|| error("unquoted parameter attribute"))?;
                attributes = &attributes[1..];
                let end = attributes.find(delimiter).ok_or_else(|| error("unclosed parameter attribute"))?;
                if key == "count" {
                    if count.is_some() {
                        return Err(error("duplicate parameter count"));
                    }
                    let value = attributes[..end].parse::<usize>().map_err(|_| error("invalid parameter count"))?;
                    if value > MAX_PARAMETERS {
                        return Err(error("too many parameters"));
                    }
                    count = Some(value);
                }
                attributes = &attributes[end + 1..];
            }
            if count.is_none() {
                return Err(error("missing parameter count"));
            }
        }
        if !tag.trim_end().ends_with('/') {
            depth += 1;
        }
    }
    Ok(())
}

fn error(message: &str) -> FormatError {
    FormatError::new(format!("Zynaptiq preset: {message}"))
}
