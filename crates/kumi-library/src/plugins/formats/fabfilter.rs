//! FabFilter's VST3 component floats and native `.ffp` documents.
//!
//! Product-specific signatures, schema versions and parameter positions come from
//! the measured catalog. `FFBS` itself identifies no product: callers must verify
//! the outer VST3 class before decoding a component. Unknown bytes are retained,
//! and an unchanged document is byte-exact.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use serde_json::Value;

use super::structure::{self, PluginFormat};
use super::tree::Node;
use super::{write_value, FormatError, MAX_DECODED_BYTES, MAX_DECODED_ITEMS};

#[derive(Debug, Clone, PartialEq)]
pub struct FabFilterPreset {
    plugin: String,
    document: Document,
    indices: BTreeMap<String, usize>,
    native_scales: BTreeMap<usize, NativeScale>,
    name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Document {
    Floats(FloatDocument),
    Text(TextDocument),
}

#[derive(Debug, Clone, PartialEq)]
struct FloatDocument {
    bytes: Vec<u8>,
    signature: [u8; 4],
    schema: u32,
    offset: usize,
    count: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct TextDocument {
    bytes: Vec<u8>,
    signature: [u8; 4],
    schema: u32,
    parameters: BTreeMap<String, Range<usize>>,
    edits: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeScale {
    minimum: f64,
    maximum: f64,
    steps: u32,
}

impl NativeScale {
    fn plain(self, normalized: f32) -> Result<f32, FormatError> {
        if !(0.0..=1.0).contains(&normalized) {
            return Err(error("native normalized parameter is outside zero to one"));
        }
        let normalized = if self.steps == 0 {
            f64::from(normalized)
        } else {
            (f64::from(normalized) * f64::from(self.steps)).round() / f64::from(self.steps)
        };
        float(self.minimum + normalized * (self.maximum - self.minimum))
    }

    fn normalized(self, plain: f32) -> Result<f32, FormatError> {
        if f64::from(plain) < self.minimum || f64::from(plain) > self.maximum {
            return Err(error("parameter is outside its verified native conversion range"));
        }
        float((f64::from(plain) - self.minimum) / (self.maximum - self.minimum))
    }
}

impl FabFilterPreset {
    /// Reads a native preset or a component whose outer VST3 identity the caller
    /// has already checked. No guess based on a filename authorizes a layout.
    pub fn read(plugin: &str, bytes: &[u8]) -> Result<Self, FormatError> {
        let format = format(plugin)?;
        let component = is_component(bytes);
        let layout = constants(format, if component { "state" } else { "preset" })?;
        let magic = signature(layout)?;
        let mut schema = number(layout, "schema")?;
        let document = if !component && layout.get("encoding").and_then(Value::as_str) == Some("ini") {
            let text = TextDocument::read(bytes)?;
            if text.signature != magic || text.schema != schema {
                return Err(error("native signature or schema differs from the verified product"));
            }
            Document::Text(text)
        } else {
            let mut count = number(layout, "parameter_count")? as usize;
            if !component {
                if let Some(uncounted) = layout.get("uncounted_layouts").and_then(Value::as_object) {
                    schema = le(bytes, 4)?;
                    count = uncounted
                        .get(&schema.to_string())
                        .and_then(Value::as_u64)
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| error("unverified uncounted native schema"))?;
                    if count > MAX_DECODED_ITEMS || bytes.len() != 8 + count * 4 {
                        return Err(error("uncounted native preset has a different length than its verified schema"));
                    }
                }
            }
            if let Some(allowed) = layout.get("parameter_counts").and_then(Value::as_array) {
                if layout.get("encoding").and_then(Value::as_str) == Some("uncounted") {
                    return Err(error("uncounted native presets require one exact verified layout"));
                }
                let actual = if magic == *b"FabF" { le(bytes, controller_name(bytes)?.end + 4)? } else { le(bytes, 8)? };
                if !allowed.iter().any(|value| value.as_u64() == Some(u64::from(actual))) {
                    return Err(error("parameter count differs from every verified product layout"));
                }
                count = actual as usize;
            }
            let counted = component || layout.get("encoding").and_then(Value::as_str) != Some("uncounted");
            Document::Floats(FloatDocument::read(bytes, magic, schema, count, counted)?)
        };
        let state = constants(format, "state")?;
        let count = number(state, "parameter_count")? as usize;
        let map = state.get("parameter_indices").and_then(Value::as_object).ok_or_else(|| error("missing verified parameter map"))?;
        if map.len() > MAX_DECODED_ITEMS {
            return Err(error("parameter map exceeds the item limit"));
        }
        let mut indices = BTreeMap::new();
        let mut positions = BTreeSet::new();
        for (name, position) in map {
            let index = position
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n < count)
                .ok_or_else(|| error("parameter map points outside the verified component"))?;
            if name.is_empty() || name.chars().any(char::is_control) || !positions.insert(index) {
                return Err(error("invalid or ambiguous parameter map"));
            }
            indices.insert(name.clone(), index);
        }
        let mut native_scales = BTreeMap::new();
        if !component && state.get("native_conversion").and_then(Value::as_str) == Some("normalized") {
            let Document::Floats(source) = &document else { return Err(error("normalized native conversion requires a float array")) };
            let transforms =
                state.get("native_transforms").and_then(Value::as_object).ok_or_else(|| error("missing native conversion map"))?;
            for index in 0..source.count {
                let transform =
                    transforms.get(&index.to_string()).ok_or_else(|| error("native conversion map does not cover every slot"))?;
                let minimum = transform.get("minimum").and_then(Value::as_f64).ok_or_else(|| error("missing native minimum"))?;
                let maximum = transform.get("maximum").and_then(Value::as_f64).ok_or_else(|| error("missing native maximum"))?;
                let steps = transform
                    .get("steps")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| error("missing native steps"))?;
                if !minimum.is_finite() || !maximum.is_finite() || minimum >= maximum {
                    return Err(error("invalid native conversion interval"));
                }
                let scale = NativeScale { minimum, maximum, steps };
                scale.plain(source.value(index).ok_or_else(|| error("missing native slot"))?)?;
                native_scales.insert(index, scale);
            }
        }
        Ok(Self { plugin: plugin.into(), document, indices, native_scales, name: None })
    }

    /// Native files and components are checked against the same product's
    /// initialized state. Different component layouts are never transplanted.
    pub fn check_layout(&self, template_component: &[u8]) -> Result<(), FormatError> {
        let template = Self::read(&self.plugin, template_component)?;
        let Document::Floats(reference) = template.document else { return Err(error("template is not a component")) };
        if !is_component(&reference.bytes) {
            return Err(error("template is not a component"));
        }
        if let Document::Floats(source) = &self.document {
            let component = is_component(&source.bytes);
            let padded_native = !component
                && source.count < reference.count
                && matches!(
                    constants(format(&self.plugin)?, "state")?.get("native_conversion").and_then(Value::as_str),
                    Some("identity" | "normalized")
                );
            if source.count != reference.count && !padded_native {
                return Err(error("native and component parameter counts differ"));
            }
            if is_component(&source.bytes) && (source.signature != reference.signature || source.schema != reference.schema) {
                return Err(error("component schemas differ"));
            }
        } else if let Document::Text(source) = &self.document {
            if source.parameters.keys().any(|key| !self.indices.contains_key(key)) {
                return Err(error("native preset contains parameters absent from the verified component map"));
            }
        }
        Ok(())
    }

    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        self.check_native_conversion()?;
        let path = path.trim_start_matches('/');
        let name = path.strip_prefix("parameters/").ok_or_else(|| error("edits use parameters/<name>"))?;
        let index = *self.indices.get(name).ok_or_else(|| error("unknown parameter name"))?;
        write_value::validate_scalar(&self.plugin, path, value)?;
        let number = match value {
            Value::Bool(value) => f64::from(u8::from(*value)),
            value => value.as_f64().ok_or_else(|| error("parameter needs a number"))?,
        };
        let number = float(number)?;
        match &mut self.document {
            Document::Floats(document) => {
                let encoded = match self.native_scales.get(&index) {
                    Some(scale) => scale.normalized(number)?,
                    None => number,
                };
                document.set(index, encoded)
            }
            Document::Text(document) => {
                if !document.parameters.contains_key(name) {
                    return Err(error("parameter is absent from this native preset"));
                }
                document.edits.insert(name.into(), number);
                Ok(())
            }
        }
    }

    /// Writes the original envelope and retains all unedited bytes, including
    /// modulation source names and private extension blocks.
    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        match &self.document {
            Document::Floats(document) => Ok(document.bytes.clone()),
            Document::Text(document) => document.write(),
        }
    }

    /// Older FabFilter components store the display name before their float
    /// array. Newer components store it in the separate controller instead.
    pub fn rename(&mut self, name: &str) -> Result<(), FormatError> {
        write_value::validate_display_name(name)?;
        if let Document::Floats(document) = &mut self.document {
            if document.signature == *b"FabF" {
                let span = controller_name(&document.bytes)?;
                let mut bytes = document.bytes[..8].to_vec();
                bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
                bytes.extend_from_slice(name.as_bytes());
                bytes.extend_from_slice(&document.bytes[span.end..]);
                if bytes.len() > MAX_DECODED_BYTES {
                    return Err(error("renamed component exceeds the size limit"));
                }
                document.offset = 20 + name.len();
                document.bytes = bytes;
            }
        }
        self.name = Some(name.into());
        Ok(())
    }

    /// Transplants verified native parameter slots into the initialized VST3
    /// envelope. Existing component state retains its own private suffix.
    pub fn to_state(&self, template_component: &[u8]) -> Result<Vec<u8>, FormatError> {
        self.check_native_conversion()?;
        self.check_layout(template_component)?;
        if matches!(&self.document, Document::Floats(document) if is_component(&document.bytes)) {
            return self.write();
        }
        let mut template = Self::read(&self.plugin, template_component)?;
        let Document::Floats(target) = &mut template.document else { unreachable!() };
        match &self.document {
            Document::Floats(source) => {
                for index in 0..source.count {
                    let value = source.value(index).ok_or_else(|| error("missing native parameter"))?;
                    let value = match self.native_scales.get(&index) {
                        Some(scale) => scale.plain(value)?,
                        None => value,
                    };
                    target.set(index, value)?;
                }
            }
            Document::Text(source) => {
                for name in source.parameters.keys() {
                    target.set(self.indices[name], source.value(name).ok_or_else(|| error("missing native parameter"))?)?;
                }
            }
        }
        if let Some(name) = &self.name {
            template.rename(name)?;
        }
        template.write()
    }

    /// Native presets may use an older parameter scale even when their array has
    /// the same length. The catalog must separately verify that conversion.
    fn check_native_conversion(&self) -> Result<(), FormatError> {
        let required = match &self.document {
            Document::Floats(document) if is_component(&document.bytes) => return Ok(()),
            Document::Floats(_) if self.native_scales.is_empty() => "identity",
            Document::Floats(_) => "normalized",
            Document::Text(_) => "named",
        };
        if constants(format(&self.plugin)?, "state")?.get("native_conversion").and_then(Value::as_str) != Some(required) {
            return Err(error("native parameter conversion has not been verified for this product"));
        }
        Ok(())
    }

    /// Creative presets keep modulation-source names and tuning metadata after
    /// the float array. Those belong in the controller, not in FFBS's private
    /// suffix. Preserve the entire native extension block and the template UI.
    pub fn to_controller(&self, template: &[u8]) -> Result<Vec<u8>, FormatError> {
        let Document::Floats(source) = &self.document else { return Ok(template.into()) };
        if is_component(&source.bytes) {
            return Ok(template.into());
        }
        let tail = &source.bytes[source.offset + source.count * 4..];
        if tail.is_empty() {
            return Ok(template.into());
        }
        self.check_native_conversion()?;
        check_controller(&self.plugin, template, template)?;
        if template.starts_with(b"FFed") {
            return Err(error("native extension has no verified legacy controller location"));
        }
        let range = controller_extensions(template)?;
        custom_values_end(tail, 0, true)?;
        let mut output = template[..range.start].to_vec();
        output.extend_from_slice(tail);
        output.extend_from_slice(&template[range.end..]);
        if output.len() > MAX_DECODED_BYTES {
            return Err(error("converted controller exceeds the size limit"));
        }
        Ok(output)
    }

    pub fn to_node(&self) -> Node {
        let (signature, schema, count, opaque) = match &self.document {
            Document::Floats(document) => {
                (document.signature, document.schema, document.count, document.bytes.len() - document.offset - document.count * 4)
            }
            Document::Text(document) => (document.signature, document.schema, document.parameters.len(), 0),
        };
        let values = self.indices.iter().filter_map(|(name, index)| {
            let value = match &self.document {
                Document::Floats(document) => document.value(*index),
                Document::Text(document) => document.value(name),
            }?;
            let value = match self.native_scales.get(index) {
                Some(scale) => scale.plain(value).ok()?,
                None => value,
            };
            Some((name.clone(), Node::Float(f64::from(value))))
        });
        Node::Map(vec![
            ("parameters".into(), Node::Map(values.collect())),
            (
                "state".into(),
                Node::Map(vec![
                    ("signature".into(), Node::Text(String::from_utf8_lossy(&signature).into_owned())),
                    ("schema".into(), Node::Int(i128::from(schema))),
                    ("parameterCount".into(), Node::Int(count as i128)),
                    ("opaqueBytes".into(), Node::Int(opaque as i128)),
                ]),
            ),
        ])
    }
}

impl FloatDocument {
    fn read(bytes: &[u8], signature: [u8; 4], schema: u32, count: usize, counted: bool) -> Result<Self, FormatError> {
        if bytes.len() > MAX_DECODED_BYTES || count == 0 || count > MAX_DECODED_ITEMS {
            return Err(error("float document exceeds its size or item limit"));
        }
        if bytes.get(..4) != Some(signature.as_slice()) || le(bytes, 4)? != schema {
            return Err(error("signature or schema differs from the verified product"));
        }
        let offset = if signature == *b"FabF" {
            let name = controller_name(bytes)?;
            if le(bytes, name.end)? != 0 {
                return Err(error("unsupported legacy component envelope"));
            }
            if le(bytes, name.end + 4)? as usize != count {
                return Err(error("parameter count differs from the verified product"));
            }
            name.end + 8
        } else if counted {
            if le(bytes, 8)? as usize != count {
                return Err(error("parameter count differs from the verified product"));
            }
            12
        } else {
            8
        };
        let end = offset + count * 4;
        let values = bytes.get(offset..end).ok_or_else(|| error("truncated parameter array"))?;
        if values.chunks_exact(4).any(|value| !f32::from_le_bytes(value.try_into().unwrap()).is_finite()) {
            return Err(error("parameter array contains a non-finite value"));
        }
        Ok(Self { bytes: bytes.into(), signature, schema, offset, count })
    }

    fn value(&self, index: usize) -> Option<f32> {
        if index >= self.count {
            return None;
        }
        let offset = self.offset + index * 4;
        Some(f32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap()))
    }

    fn set(&mut self, index: usize, value: f32) -> Result<(), FormatError> {
        if index >= self.count || !value.is_finite() {
            return Err(error("invalid parameter position or value"));
        }
        let offset = self.offset + index * 4;
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }
}

impl TextDocument {
    fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(error("native text exceeds the size limit"));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| error("native text is not UTF-8"))?;
        if text.contains('\0') {
            return Err(error("native text contains a NUL byte"));
        }
        let mut section = "";
        let mut sections = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut signature = None;
        let mut schema = None;
        let mut parameters = BTreeMap::new();
        let mut offset = 0;
        for (number, line) in text.split_inclusive('\n').enumerate() {
            if number >= MAX_DECODED_ITEMS {
                return Err(error("native text exceeds the line limit"));
            }
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                section = &trimmed[1..trimmed.len() - 1];
                if !sections.insert(section) {
                    return Err(error("duplicate native section"));
                }
            } else if !trimmed.is_empty() && !trimmed.starts_with([';', '#']) {
                let (key, value) = trimmed.split_once('=').ok_or_else(|| error("invalid native key/value line"))?;
                let key = key.trim();
                let value = value.trim();
                if section.is_empty() || key.is_empty() || !keys.insert((section, key)) {
                    return Err(error("missing section or duplicate native key"));
                }
                match (section, key) {
                    ("Preset", "Signature") => {
                        signature = Some(value.as_bytes().try_into().map_err(|_| error("invalid native signature"))?);
                    }
                    ("Preset", "Version") => {
                        schema = Some(value.parse().map_err(|_| error("invalid native schema"))?);
                    }
                    ("Parameters", _) => {
                        float(value.parse::<f64>().map_err(|_| error("native parameter is not numeric"))?)?;
                        let equals = line.find('=').ok_or_else(|| error("missing native value separator"))?;
                        let start = offset + equals + 1 + line[equals + 1..].len() - line[equals + 1..].trim_start().len();
                        parameters.insert(key.into(), start..start + value.len());
                    }
                    _ => {}
                }
            }
            offset += line.len();
        }
        if parameters.is_empty() {
            return Err(error("native preset has no parameters"));
        }
        Ok(Self {
            bytes: bytes.into(),
            signature: signature.ok_or_else(|| error("missing native signature"))?,
            schema: schema.ok_or_else(|| error("missing native schema"))?,
            parameters,
            edits: BTreeMap::new(),
        })
    }

    fn value(&self, name: &str) -> Option<f32> {
        self.edits.get(name).copied().or_else(|| {
            let span = self.parameters.get(name)?;
            std::str::from_utf8(&self.bytes[span.clone()]).ok()?.parse::<f32>().ok()
        })
    }

    fn write(&self) -> Result<Vec<u8>, FormatError> {
        let mut patches: Vec<_> = self.edits.iter().map(|(name, value)| (&self.parameters[name], value.to_string())).collect();
        patches.sort_by_key(|(span, _)| span.start);
        let mut bytes = Vec::new();
        let mut position = 0;
        for (span, value) in patches {
            bytes.extend_from_slice(&self.bytes[position..span.start]);
            bytes.extend_from_slice(value.as_bytes());
            position = span.end;
        }
        bytes.extend_from_slice(&self.bytes[position..]);
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(error("edited native text exceeds the size limit"));
        }
        Ok(bytes)
    }
}

/// Controller state carries a product signature even though FFBS does not. Its
/// schema and prefix must match the initialized controller of the selected class.
pub fn check_controller(plugin: &str, bytes: &[u8], template: &[u8]) -> Result<(), FormatError> {
    let state_magic = signature(constants(format(plugin)?, "state")?)?;
    if state_magic == *b"FabF" {
        if bytes.len() < 12
            || bytes.len() > MAX_DECODED_BYTES
            || bytes.get(..8) != Some(b"FFed\0\0\0\0".as_slice())
            || template.get(..8) != bytes.get(..8)
        {
            return Err(error("invalid legacy controller envelope"));
        }
        return Ok(());
    }
    let expected = signature(constants(format(plugin)?, "preset")?)?;
    controller_name(bytes)?;
    controller_name(template)?;
    if bytes[..4] != expected || bytes[..8] != template[..8] {
        return Err(error("controller belongs to a different product or schema"));
    }
    // Validate the known length-prefixed metadata; tuning and editor extensions
    // remain opaque, rather than replaced by initialized data.
    custom_values_end(bytes, controller_extension_start(bytes)?, false)?;
    custom_values_end(template, controller_extension_start(template)?, false)?;
    Ok(())
}

pub fn rename_controller(plugin: &str, bytes: &[u8], template: &[u8], name: &str) -> Result<Vec<u8>, FormatError> {
    check_controller(plugin, bytes, template)?;
    write_value::validate_display_name(name)?;
    if bytes.starts_with(b"FFed") {
        return Ok(bytes.into());
    }
    let span = controller_name(bytes)?;
    let mut output = bytes[..8].to_vec();
    output.extend_from_slice(&(name.len() as u32).to_le_bytes());
    output.extend_from_slice(name.as_bytes());
    output.extend_from_slice(&bytes[span.end..]);
    if output.len() > MAX_DECODED_BYTES {
        return Err(error("renamed controller exceeds the size limit"));
    }
    Ok(output)
}

fn controller_name(bytes: &[u8]) -> Result<Range<usize>, FormatError> {
    if bytes.len() > MAX_DECODED_BYTES {
        return Err(error("controller exceeds the size limit"));
    }
    let size = le(bytes, 8)? as usize;
    let end = 12usize.checked_add(size).ok_or_else(|| error("controller name length overflow"))?;
    let name = bytes.get(12..end).ok_or_else(|| error("truncated controller name"))?;
    std::str::from_utf8(name).map_err(|_| error("controller name is not UTF-8"))?;
    Ok(12..end)
}

fn controller_extension_start(bytes: &[u8]) -> Result<usize, FormatError> {
    let name = controller_name(bytes)?;
    let mut offset = name.end + 8;
    for _ in 0..2 {
        let length = le(bytes, offset)? as usize;
        offset = offset.checked_add(4).and_then(|n| n.checked_add(length)).ok_or_else(|| error("controller string length overflow"))?;
        if offset > bytes.len() {
            return Err(error("truncated controller string"));
        }
    }
    Ok(offset)
}

fn controller_extensions(bytes: &[u8]) -> Result<Range<usize>, FormatError> {
    let offset = controller_extension_start(bytes)?;
    let end = bytes.len().checked_sub(12).ok_or_else(|| error("missing controller editor tail"))?;
    if bytes.get(end..) != Some(b"FFed\0\0\0\0\0\0\0\0".as_slice()) || custom_values_end(bytes, offset, false)? > end {
        return Err(error("unsupported controller editor tail"));
    }
    Ok(offset..end)
}

fn custom_values_end(bytes: &[u8], at: usize, check_resources: bool) -> Result<usize, FormatError> {
    if bytes.get(at..at + 4) != Some(b"CuSV".as_slice()) || le(bytes, at + 4)? != 1 {
        return Err(error("unsupported native extension block"));
    }
    let count = le(bytes, at + 8)? as usize;
    if count > MAX_DECODED_ITEMS {
        return Err(error("native extension exceeds its item limit"));
    }
    let mut position = at + 12;
    let mut keys = BTreeSet::new();
    for _ in 0..count {
        let mut key = "";
        for field in 0..2 {
            let length = le(bytes, position)? as usize;
            position += 4;
            let end = position.checked_add(length).ok_or_else(|| error("native extension length overflow"))?;
            let value = bytes.get(position..end).ok_or_else(|| error("truncated native extension string"))?;
            let value = std::str::from_utf8(value).map_err(|_| error("native extension string is not UTF-8"))?;
            if field == 0 {
                if !keys.insert(value) {
                    return Err(error("duplicate native extension key"));
                }
                key = value;
            } else if check_resources {
                check_resource_path(key, value)?;
            }
            position = end;
        }
    }
    Ok(position)
}

fn check_resource_path(key: &str, value: &str) -> Result<(), FormatError> {
    let key = key.to_ascii_lowercase();
    let resource = ["path", "filename", "directory", "folder", "url", "uri"].iter().any(|part| key.contains(part));
    let value = value.trim();
    if resource
        && (kumi_common::path::network_or_device(value) || kumi_common::path::reserved_device(value) || value.chars().any(char::is_control))
    {
        return Err(error("native preset resource paths cannot name a network location or device"));
    }
    Ok(())
}

fn format(plugin: &str) -> Result<&'static PluginFormat, FormatError> {
    structure::builtin(plugin).filter(|format| format.info.family == "fabfilter").ok_or_else(|| error("unknown FabFilter product"))
}

/// Both component envelopes lack product identity. Only accept these bytes
/// within a VST3 container whose class the workflow has checked.
pub fn is_component(bytes: &[u8]) -> bool {
    bytes.starts_with(b"FFBS") || bytes.starts_with(b"FabF")
}

fn constants<'a>(format: &'a PluginFormat, kind: &str) -> Result<&'a BTreeMap<String, Value>, FormatError> {
    format.structure.files.get(kind).map(|file| &file.constants).ok_or_else(|| error("missing verified file layout"))
}

fn signature(constants: &BTreeMap<String, Value>) -> Result<[u8; 4], FormatError> {
    constants
        .get("magic")
        .and_then(Value::as_str)
        .and_then(|value| value.as_bytes().try_into().ok())
        .ok_or_else(|| error("missing four-byte product signature"))
}

fn number(constants: &BTreeMap<String, Value>, key: &str) -> Result<u32, FormatError> {
    constants
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| error("missing numeric layout constant"))
}

fn le(bytes: &[u8], at: usize) -> Result<u32, FormatError> {
    let word: [u8; 4] = bytes.get(at..at + 4).ok_or_else(|| error("truncated header"))?.try_into().unwrap();
    Ok(u32::from_le_bytes(word))
}

fn float(value: f64) -> Result<f32, FormatError> {
    let value = value as f32;
    if !value.is_finite() {
        return Err(error("parameter cannot be represented as a finite float"));
    }
    Ok(value)
}

fn error(message: &str) -> FormatError {
    FormatError::new(format!("FabFilter preset: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floats(magic: &[u8; 4], schema: u32, values: &[f32], tail: &[u8]) -> Vec<u8> {
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&schema.to_le_bytes());
        bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(tail);
        bytes
    }

    #[test]
    fn float_edits_preserve_other_slots_and_every_extension_byte() {
        let original = floats(b"FFBS", 1, &[0.0, 9.965_784, -12.0], b"private\0\xfftail");
        let mut document = FloatDocument::read(&original, *b"FFBS", 1, 3, true).unwrap();
        assert_eq!(document.bytes, original);
        document.set(1, 10.0).unwrap();
        let mut expected = original.clone();
        expected[16..20].copy_from_slice(&10_f32.to_le_bytes());
        assert_eq!(document.bytes, expected);
        assert!(document.set(3, 1.0).is_err());
        assert!(document.set(1, f32::INFINITY).is_err());
        assert_eq!(document.bytes, expected, "rejected edits must leave the document unchanged");
    }

    #[test]
    fn arrays_reject_truncation_mismatched_schemas_and_nonfinite_values() {
        let bytes = floats(b"FFBS", 1, &[0.0, 1.0], b"");
        for size in 0..bytes.len() {
            assert!(FloatDocument::read(&bytes[..size], *b"FFBS", 1, 2, true).is_err(), "accepted {size} bytes");
        }
        assert!(FloatDocument::read(&bytes, *b"FabF", 1, 2, true).is_err());
        assert!(FloatDocument::read(&bytes, *b"FFBS", 2, 2, true).is_err());
        assert!(FloatDocument::read(&bytes, *b"FFBS", 1, 1, true).is_err());
        assert!(FloatDocument::read(&bytes, *b"FFBS", 1, MAX_DECODED_ITEMS + 1, true).is_err());
        for value in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let invalid = floats(b"FFBS", 1, &[value], b"");
            assert!(FloatDocument::read(&invalid, *b"FFBS", 1, 1, true).is_err());
        }
    }

    #[test]
    fn legacy_components_use_the_declared_utf8_name_before_the_array() {
        let name = "Kumi 音";
        let mut bytes = b"FabF\x02\0\0\0".to_vec();
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&(-18_f32).to_le_bytes());
        bytes.extend_from_slice(&0.25_f32.to_le_bytes());
        bytes.extend_from_slice(b"private");
        let document = FloatDocument::read(&bytes, *b"FabF", 2, 2, true).unwrap();
        assert_eq!(document.value(0), Some(-18.0));
        assert_eq!(document.value(1), Some(0.25));
        assert_eq!(document.bytes, bytes);
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(FloatDocument::read(&bytes, *b"FabF", 2, 2, true).is_err());
    }

    #[test]
    fn native_text_edits_only_the_requested_numeric_span() {
        let text = b"[Preset]\r\nSignature=FQ4p\r\nVersion=4\r\nAuthor=Private\r\n\r\n[Parameters]\r\nBand 1 Gain = -3.25 \r\nBand 1 Frequency=9.96578407287598\r\n[Unknown]\r\nopaque=leave me alone\r\n";
        let mut document = TextDocument::read(text).unwrap();
        assert_eq!(document.write().unwrap(), text);
        assert_eq!(document.value("Band 1 Gain"), Some(-3.25));
        document.edits.insert("Band 1 Gain".into(), 2.5);
        let expected = String::from_utf8(text.to_vec()).unwrap().replace("-3.25", "2.5");
        assert_eq!(document.write().unwrap(), expected.as_bytes());
        assert_eq!(TextDocument::read(expected.as_bytes()).unwrap().value("Band 1 Gain"), Some(2.5));
    }

    #[test]
    fn native_text_rejects_ambiguous_keys_and_unbounded_numbers() {
        let header = "[Preset]\nSignature=FQ4p\nVersion=4\n[Parameters]\n";
        for body in ["Gain=0\nGain=1\n", "Gain=NaN\n", "Gain=inf\n", "Gain=1e100\n", "Gain=0\n[Parameters]\nGain=1\n"] {
            assert!(TextDocument::read(format!("{header}{body}").as_bytes()).is_err(), "accepted {body}");
        }
        assert!(TextDocument::read(b"[Parameters]\nGain=0\n").is_err());
        assert!(TextDocument::read(b"[Preset]\nSignature=FQ4p\nVersion=4\n").is_err());
        let huge = vec![b' '; MAX_DECODED_BYTES + 1];
        assert!(TextDocument::read(&huge).is_err());
    }

    #[test]
    fn custom_values_are_bounded_and_do_not_swallow_the_following_tuning_block() {
        let mut bytes = b"CuSV\x01\0\0\0\x01\0\0\0".to_vec();
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(b"LFO");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(b"Slow");
        let end = bytes.len();
        bytes.extend_from_slice(b"FFMT tuning bytes");
        assert_eq!(custom_values_end(&bytes, 0, false).unwrap(), end);
        assert!(custom_values_end(&bytes[..end - 1], 0, false).is_err());
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(custom_values_end(&bytes, 0, false).is_err());
    }

    #[test]
    fn native_resource_paths_are_checked_without_reinterpreting_display_metadata() {
        for path in [r"\\host\share\room.wav", r"\??\UNC\host\share\room.wav", "//host/share/room.wav"] {
            assert!(check_resource_path("IRFileName", path).is_err());
            assert!(check_resource_path("ResourcePath", path).is_err());
            assert!(check_resource_path("DESCRIPTION", path).is_ok());
        }
        assert!(check_resource_path("IRFileName", r"C:\Sounds\Room.wav").is_ok());
        assert!(check_resource_path("IRFileName", "").is_ok());
        assert_eq!(check_resource_path("IRFileName", "NUL").is_err(), cfg!(windows));
    }

    #[test]
    fn normalized_native_scales_keep_enum_and_bipolar_meanings() {
        let wave = NativeScale { minimum: 0.0, maximum: 3.0, steps: 3 };
        assert_eq!(wave.plain(1.0 / 3.0).unwrap(), 1.0);
        assert_eq!(wave.plain(0.5).unwrap(), 2.0);
        assert_eq!(wave.normalized(2.0).unwrap(), 2.0 / 3.0);
        let pitch = NativeScale { minimum: -12.0, maximum: 12.0, steps: 0 };
        assert_eq!(pitch.plain(0.5).unwrap(), 0.0);
        assert_eq!(pitch.normalized(-6.0).unwrap(), 0.25);
        assert!(pitch.plain(1.1).is_err());
        assert!(pitch.normalized(13.0).is_err());
    }
}
