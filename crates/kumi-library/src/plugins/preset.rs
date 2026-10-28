//! The DAW-neutral preset workflow. It produces complete VST3 files from Kumi's
//! verified templates, native presets, or existing VST3 states. Loading and undo
//! remain the responsibility of the DAW integration.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::formats::{
    cbor,
    minimal_audio::MinimalAudioPreset,
    ozone12,
    pulsar_modular::PulsarModularPreset,
    serum2, softube,
    structure::{self, PluginFormat},
    superior_drummer,
    tree::Node,
    valhalla::ValhallaPreset,
    vital::VitalPreset,
    vstpreset::{self, VstPreset},
    xml, zynaptiq, FormatError, MAX_DECODED_BYTES,
};
use super::templates;

mod native;
pub use native::{create_native_from_template, edit_native, NativePreset};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub path: String,
    pub value: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOptions {
    /// The installed plug-in version, checked against the format's write evidence.
    pub version: String,
    pub class_id: Option<String>,
    pub name: Option<String>,
}

/// A new file from the selected product's own initialized state.
pub fn create(plugin: &str, options: &WriteOptions, edits: &[Edit]) -> Result<VstPreset, FormatError> {
    let format = known(plugin)?;
    writable(format, &options.version)?;
    let class = class_id(format, options.class_id.as_deref(), None)?;
    if family(format) == "serum-2" {
        let preset = serum2::SerumPreset::init(options.name.as_deref().unwrap_or("Kumi Init"))?;
        return edit(plugin, &preset.file.write()?, options, edits);
    }
    let initial = initial(format, &class)?;
    edit(plugin, &initial.write(), options, edits)
}

/// Apply a batch atomically, retaining unrelated VST3 chunks and native fields.
/// Native XML is converted using the selected product's verified component envelope.
pub fn edit(plugin: &str, input: &[u8], options: &WriteOptions, edits: &[Edit]) -> Result<VstPreset, FormatError> {
    let format = known(plugin)?;
    writable(format, &options.version)?;
    if input.len() > MAX_DECODED_BYTES {
        return Err(error("preset exceeds the size limit"));
    }
    if let Some(name) = &options.name {
        check_name(name)?;
    }
    let source = read_container(input)?;
    let class = class_id(format, options.class_id.as_deref(), source.as_ref())?;
    let template = initial(format, &class)?;
    let template_component = component(&template)?;
    let mut output = source.clone().unwrap_or_else(|| template.clone());
    let state = match &source {
        Some(source) => component(source)?,
        None => input,
    };
    let native = source.is_none();

    let component = match family(format) {
        "serum-2" => {
            let old = if native { None } else { Some(serum2::SerumState::read(state, output.controller())?) };
            if let Some(old) = &old {
                check_serum_state(old, &options.version)?;
            }
            let mut preset = match &old {
                Some(state) => state.to_preset()?,
                None => serum2::SerumPreset::read(input)?,
            };
            check_serum_file(&preset.file, &options.version)?;
            if let Some(name) = &options.name {
                preset.rename(name)?;
            }
            for change in edits {
                if change.path == "$add_distortion" {
                    let rack =
                        change.value.as_u64().filter(|rack| *rack <= 2).ok_or_else(|| error("$add_distortion requires rack 0, 1 or 2"))?;
                    preset.add_distortion(rack as usize)?;
                } else {
                    preset.set(&change.path, &change.value)?;
                }
            }
            check_serum_file(&preset.file, &options.version)?;
            let mut edited = preset.to_state()?;
            if let Some(old) = old {
                retain_unknown_xfer(&old.processor, &mut edited.processor)?;
                if let (Some(old), Some(new)) = (&old.controller, &mut edited.controller) {
                    retain_unknown_xfer(old, new)?;
                } else if old.controller.is_none() {
                    edited.controller = None;
                }
            }
            check_serum_state(&edited, &options.version)?;
            if let Some(controller) = edited.controller {
                replace_chunk(&mut output, "Cont", controller.write()?);
            }
            edited.processor.write()?
        }
        "ozone-12" => {
            let mut preset = if native {
                let mut preset = ozone12::OzoneState::init(&class)?;
                preset.apply_preset(&ozone12::OzonePreset::read(input)?)?;
                preset
            } else {
                ozone12::OzoneState::read(state)?
            };
            preset.to_vstpreset(&class)?;
            for change in edits {
                if change.path == "$chain" {
                    let values = change.value.as_array().ok_or_else(|| error("$chain requires an array of module names"))?;
                    let modules = values
                        .iter()
                        .map(|value| value.as_str().ok_or_else(|| error("chain entries must be strings")))
                        .collect::<Result<Vec<_>, _>>()?;
                    preset.set_chain(&modules)?;
                } else {
                    let path = change
                        .path
                        .strip_prefix("DSP State/DSP Elements/")
                        .or_else(|| change.path.strip_prefix("parameters/"))
                        .unwrap_or(&change.path);
                    let (element, parameter) = path.split_once('/').ok_or_else(|| error("Ozone edits use ElementID/ParamID"))?;
                    preset.set_value(element, parameter, change.value.clone())?;
                }
            }
            preset.to_vstpreset(&class)?;
            preset.write()?
        }
        "valhalla" => {
            let mut preset = ValhallaPreset::read(state)?;
            preset.check_version(template_component)?;
            preset.to_state(template_component)?;
            if let Some(name) = &options.name {
                preset.set("name", &json!(name))?;
            }
            for change in edits {
                check_range(format, change)?;
                preset.set(&change.path, &change.value)?;
            }
            preset.check_version(template_component)?;
            if native {
                preset.to_state(template_component)?
            } else {
                preset.write_state()?
            }
        }
        "minimal-audio" => {
            let mut preset = MinimalAudioPreset::read(state)?;
            preset.check_version(template_component)?;
            preset.to_state(template_component)?;
            for change in edits {
                check_range(format, change)?;
                preset.set(&change.path, &change.value)?;
            }
            preset.check_version(template_component)?;
            if native {
                preset.to_state(template_component)?
            } else {
                preset.write_state()?
            }
        }
        "pulsar-modular" => {
            let mut preset = PulsarModularPreset::read(state)?;
            preset.check_version(template_component)?;
            preset.to_state(template_component)?;
            for change in edits {
                check_range(format, change)?;
                preset.set(&change.path, &change.value)?;
            }
            preset.check_version(template_component)?;
            if native {
                preset.to_state(template_component)?
            } else {
                preset.write_state()?
            }
        }
        "zynaptiq" => {
            let mut preset = if native { zynaptiq::Preset::read(state)? } else { zynaptiq::State::read(state)?.preset };
            native::validate_zynaptiq(format, &preset)?;
            native::check_zynaptiq_version(format, &preset)?;
            let reference = zynaptiq::State::read(template_component)?;
            let count = |preset: &zynaptiq::Preset| {
                preset.document.root.child("inputParams").and_then(|node| node.attribute("count")).map(str::to_owned)
            };
            if count(&preset) != count(&reference.preset) {
                return Err(error("Zynaptiq preset has a different parameter layout than the selected product"));
            }
            if let Some(name) = &options.name {
                preset.set("@progName", &json!(name))?;
            }
            for change in edits {
                check_range(format, change)?;
                preset.set(&change.path, &change.value)?;
            }
            preset.component_state()?
        }
        "superior-drummer" => {
            let mut preset = superior_drummer::State::read(if native { template_component } else { state })?;
            if preset.version() != options.version {
                return Err(error("Superior Drummer component version differs from the verified installed version"));
            }
            if native {
                preset.preset = Some(superior_drummer::Preset::read(input)?);
            }
            if preset.preset.is_none() {
                preset.preset = Some(superior_drummer::Preset::create(&options.version)?);
            }
            if preset.preset.as_ref().and_then(superior_drummer::Preset::version).as_deref() != Some(options.version.as_str()) {
                return Err(error("Superior Drummer preset version differs from the verified installed version"));
            }
            for change in edits {
                check_range(format, change)?;
                preset.set(&change.path, &change.value)?;
            }
            preset.write()?
        }
        _ => return Err(error("this product has no verified VST3 state writer; use its exposed parameters")),
    };
    // Decoding still runs above, so an invalid checksum or mismatched component
    // cannot bypass validation. A no-op does not normalize someone else's state.
    if edits.is_empty() && options.name.is_none() {
        if let Some(original) = source {
            return Ok(original);
        }
    }
    replace_chunk(&mut output, "Comp", component);
    if let Some(name) = &options.name {
        set_name(&mut output, name)?;
    }
    Ok(output)
}

/// Decode a preset locally. The returned data uses the codec's documented edit paths;
/// Ozone additionally exposes a compact parameters map and its module chain.
pub fn inspect(plugin: &str, input: &[u8], requested_class: Option<&str>) -> Result<Value, FormatError> {
    let format = known(plugin)?;
    if input.len() > MAX_DECODED_BYTES {
        return Err(error("preset exceeds the size limit"));
    }
    let source = read_container(input)?;
    let class = if source.is_none() && requested_class.is_none() && format.info.ids.vst3.is_empty() {
        None
    } else {
        Some(class_id(format, requested_class, source.as_ref())?)
    };
    let state = match &source {
        Some(source) => component(source)?,
        None => input,
    };
    let tree = match family(format) {
        "serum-2" => match &source {
            Some(source) => serum2::SerumState::read(state, source.controller())?.to_preset()?.file.body.to_node(),
            None => {
                let preset = serum2::SerumPreset::read(state)?;
                if preset.file.hash_ok == Some(false) {
                    return Err(error("Serum preset checksum does not match"));
                }
                preset.file.body.to_node()
            }
        },
        "ozone-12" => {
            if source.is_some() {
                let preset = ozone12::OzoneState::read(state)?;
                preset.validate_class(class.as_deref().ok_or_else(|| error("missing Ozone VST3 processor identity"))?)?;
                let mut parameters = serde_json::Map::new();
                if let Some(elements) = preset.elements() {
                    for (element, data) in elements {
                        let mut settings = serde_json::Map::new();
                        if let Some(fields) = data.get("Value").and_then(Value::as_object) {
                            for id in fields.keys() {
                                if let Some(value) = preset.value(element, id) {
                                    settings.insert(id.clone(), value.clone());
                                }
                            }
                        }
                        parameters.insert(element.clone(), Value::Object(settings));
                    }
                }
                return Ok(
                    json!({"plugin":plugin,"classId":class,"data":{"parameters":parameters,"chain":preset.chain()?.map(|entries| entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>()),"state":node_json(&preset.to_node())}}),
                );
            } else {
                let preset = ozone12::OzonePreset::read(state)?;
                let mut parameters = serde_json::Map::new();
                for module in preset.modules() {
                    for parameter in module.params {
                        let fields = parameters.entry(parameter.element).or_insert_with(|| json!({}));
                        fields[parameter.id] = parameter
                            .value
                            .parse::<f64>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map_or_else(|| json!(parameter.value), |v| json!(v));
                    }
                }
                return Ok(
                    json!({"plugin":plugin,"classId":class,"data":{"parameters":parameters,"chain":preset.chain()?.map(|entries| entries.into_iter().map(|entry| entry.name).collect::<Vec<_>>()),"state":node_json(&preset.to_node())}}),
                );
            }
        }
        "valhalla" => {
            let preset = ValhallaPreset::read(state)?;
            preset
                .to_state(component(&initial(format, class.as_deref().ok_or_else(|| error("missing Valhalla processor identity"))?)?)?)?;
            preset.to_node()
        }
        "minimal-audio" => {
            let preset = MinimalAudioPreset::read(state)?;
            preset.to_state(component(&initial(
                format,
                class.as_deref().ok_or_else(|| error("missing Minimal Audio processor identity"))?,
            )?)?)?;
            preset.to_node()
        }
        "pulsar-modular" => {
            let preset = PulsarModularPreset::read(state)?;
            preset.to_state(component(&initial(
                format,
                class.as_deref().ok_or_else(|| error("missing Pulsar Modular processor identity"))?,
            )?)?)?;
            preset.to_node()
        }
        "zynaptiq" => {
            let preset = if source.is_some() { zynaptiq::State::read(state)?.preset } else { zynaptiq::Preset::read(state)? };
            native::validate_zynaptiq(format, &preset)?;
            preset.to_node()
        }
        "superior-drummer" => {
            if source.is_some() {
                superior_drummer::State::read(state)?.to_node()
            } else {
                superior_drummer::Preset::read(state)?.to_node()
            }
        }
        "softube" if source.is_none() => {
            let preset = softube::Preset::read(state)?;
            native::validate_softube(format, &preset)?;
            preset.to_node()
        }
        "vital" => {
            if source.is_some() {
                let chunk = vstpreset::vst2_chunk(state)?.ok_or_else(|| error("Vital state has no compatibility chunk"))?;
                VitalPreset::read_state(chunk.data)?.to_node()
            } else {
                VitalPreset::read(state)?.to_node()
            }
        }
        _ => return Err(error("no native reader for this state; inspect the plug-in's parameters")),
    };
    Ok(json!({"plugin":plugin,"classId":class,"data":node_json(&tree)}))
}

/// A bounded view for an agent tool. The query matches concrete paths or values;
/// the full decoded state remains available to internal callers through `inspect`.
pub fn inspect_fields(
    plugin: &str,
    input: &[u8],
    requested_class: Option<&str>,
    query: Option<&str>,
    limit: usize,
) -> Result<Value, FormatError> {
    let decoded = inspect(plugin, input, requested_class)?;
    let family = family(known(plugin)?);
    let needle = query.unwrap_or_default().to_lowercase();
    let mut fields = Vec::new();
    let mut total: usize = 0;
    let limit = limit.clamp(1, 200);
    let mut collect = |path: &str, value: &Value| {
        if needle.is_empty() || path.to_lowercase().contains(&needle) || value.to_string().to_lowercase().contains(&needle) {
            total += 1;
            if fields.len() < limit {
                let (value, truncated) = preview(value);
                fields.push(json!({"path":path,"value":value,"truncated":truncated}));
            }
        }
    };
    if family == "superior-drummer" {
        if let Some(records) = decoded["data"].as_object() {
            for (path, value) in records {
                collect(path, value);
            }
        }
    } else {
        leaves(&decoded["data"], "", family, &mut collect);
    }
    Ok(json!({"plugin":decoded["plugin"],"classId":decoded["classId"],"fields":fields,"more":total.saturating_sub(limit)}))
}

fn preview(value: &Value) -> (Value, bool) {
    match value {
        Value::String(text) if text.len() > 4096 => {
            let mut end = 4096;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            (json!(&text[..end]), true)
        }
        Value::Array(items) => {
            let mut truncated = items.len() > 32;
            let items = items
                .iter()
                .take(32)
                .map(|item| {
                    let (value, cut) = preview(item);
                    truncated |= cut;
                    value
                })
                .collect();
            (Value::Array(items), truncated)
        }
        _ => (value.clone(), false),
    }
}

fn leaves(value: &Value, path: &str, family: &str, collect: &mut impl FnMut(&str, &Value)) {
    match value {
        Value::Object(fields) if !fields.is_empty() && !fields.contains_key("bytes") => {
            for (key, value) in fields {
                let path = if path.is_empty() { key.clone() } else { format!("{path}/{key}") };
                leaves(value, &path, family, collect);
            }
        }
        Value::Array(values) if !values.is_empty() => {
            for (index, value) in values.iter().enumerate() {
                let indexed = if path.starts_with("state/") && matches!(family, "valhalla" | "minimal-audio" | "pulsar-modular") {
                    format!("{path}[{index}]")
                } else {
                    format!("{path}/{index}")
                };
                leaves(value, &indexed, family, collect);
            }
        }
        _ => collect(path, value),
    }
}

fn known(plugin: &str) -> Result<&'static PluginFormat, FormatError> {
    structure::builtin(plugin).ok_or_else(|| error(&format!("unknown plug-in format {plugin}")))
}

fn family(format: &PluginFormat) -> &str {
    if format.info.family.is_empty() {
        &format.info.plugin
    } else {
        &format.info.family
    }
}

fn writable(format: &PluginFormat, version: &str) -> Result<(), FormatError> {
    let verified = format.verified.can_write(version, "state")
        || (family(format) == "serum-2"
            && format.verified.can_write(version, "processor")
            && format.verified.can_write(version, "controller"));
    if !verified {
        return Err(error(&format!(
            "{} version {version:?} has no verified VST3 writer; inspect locally or use exposed parameters",
            format.info.name
        )));
    }
    Ok(())
}

fn class_id(format: &PluginFormat, requested: Option<&str>, source: Option<&VstPreset>) -> Result<String, FormatError> {
    if let (Some(requested), Some(source)) = (requested, source) {
        if !requested.eq_ignore_ascii_case(&source.class_id) {
            return Err(error("requested class ID differs from the input VST3 preset"));
        }
    }
    let id = source
        .map(|source| source.class_id.as_str())
        .or(requested)
        .or_else(|| format.info.ids.vst3.first().map(|id| id.id.as_str()))
        .ok_or_else(|| error("this product has no known VST3 class ID"))?;
    if id.len() != 32
        || !id.bytes().all(|b| b.is_ascii_hexdigit())
        || !format.info.ids.vst3.iter().any(|known| known.id.eq_ignore_ascii_case(id))
    {
        return Err(error("class ID is not a supported VST3 processor of the selected product"));
    }
    Ok(id.to_ascii_uppercase())
}

fn initial(format: &PluginFormat, class: &str) -> Result<VstPreset, FormatError> {
    let preset = if family(format) == "ozone-12" {
        ozone12::OzoneState::init(class)?.to_vstpreset(class)?
    } else {
        VstPreset::read(
            templates::initial(&format.info.plugin).ok_or_else(|| error("no verified VST3 init template is available for this product"))?,
        )?
    };
    if !preset.class_id.eq_ignore_ascii_case(class) {
        return Err(error("init template belongs to a different VST3 processor"));
    }
    component(&preset)?;
    Ok(preset)
}

fn read_container(input: &[u8]) -> Result<Option<VstPreset>, FormatError> {
    if !input.starts_with(b"VST3") {
        return Ok(None);
    }
    let preset = VstPreset::read(input)?;
    if preset.version != 1 {
        return Err(error("unsupported VST3 preset container version"));
    }
    component(&preset)?;
    if ["Cont", "Info"].iter().any(|id| preset.chunks.iter().filter(|(key, _)| key == id).count() > 1) {
        return Err(error("duplicate VST3 controller or metadata chunks"));
    }
    Ok(Some(preset))
}

fn component(preset: &VstPreset) -> Result<&[u8], FormatError> {
    if preset.chunks.iter().filter(|(id, _)| id == "Comp").count() != 1 {
        return Err(error("VST3 preset must have exactly one component chunk"));
    }
    preset.component().ok_or_else(|| error("missing VST3 component state"))
}

fn replace_chunk(preset: &mut VstPreset, id: &str, data: Vec<u8>) {
    if let Some((_, old)) = preset.chunks.iter_mut().find(|(key, _)| key == id) {
        *old = data;
    } else {
        preset.chunks.push((id.into(), data));
    }
}

fn check_range(format: &PluginFormat, edit: &Edit) -> Result<(), FormatError> {
    let path = edit.path.trim_start_matches('/');
    let canonical = if let Some(rest) = path.strip_prefix("presets/") {
        rest.split_once('/').map(|(_, field)| format!("presets[]/{field}")).unwrap_or_else(|| path.into())
    } else {
        path.into()
    };
    fn check(format: &PluginFormat, path: &str, value: &Value) -> Result<(), FormatError> {
        match value {
            Value::Number(value) => {
                structure::validate_number(&format.info.plugin, path, value.as_f64().ok_or_else(|| error("numeric value is not finite"))?)
            }
            Value::Bool(value) if format.structure.field(path).is_none_or(|field| field.kind != "bool") => {
                structure::validate_number(&format.info.plugin, path, u8::from(*value) as f64)
            }
            Value::Array(values) => {
                for value in values {
                    check(format, path, value)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    check(format, &canonical, &edit.value)
}

fn check_name(name: &str) -> Result<(), FormatError> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(error("preset name must be 1–256 bytes without control characters"));
    }
    Ok(())
}

fn set_name(preset: &mut VstPreset, name: &str) -> Result<(), FormatError> {
    let mut info = match preset.chunk("Info") {
        Some(bytes) => xml::parse(std::str::from_utf8(bytes).map_err(|_| error("VST3 Info is not UTF-8 XML"))?)?,
        None => xml::parse("<MetaInfo/>")?,
    };
    if info.root.name != "MetaInfo" {
        return Err(error("VST3 Info has an unsupported root; its data was not replaced"));
    }
    if let Some(attribute) = info.root.children.iter_mut().find(|child| child.name == "Attribute" && child.attribute("id") == Some("Name"))
    {
        if let Some((_, value)) = attribute.attributes.iter_mut().find(|(key, _)| key == "value") {
            *value = name.into();
        } else {
            attribute.attributes.push(("value".into(), name.into()));
        }
    } else {
        info.root.children.push(xml::Element {
            name: "Attribute".into(),
            attributes: vec![("id".into(), "Name".into()), ("value".into(), name.into())],
            children: Vec::new(),
            text: String::new(),
        });
    }
    replace_chunk(preset, "Info", xml::write(&info).into_bytes());
    Ok(())
}

fn check_serum_state(state: &serum2::SerumState, version: &str) -> Result<(), FormatError> {
    check_serum_file(&state.processor, version)?;
    if let Some(controller) = &state.controller {
        check_serum_file(controller, version)?;
    }
    Ok(())
}

fn check_serum_file(file: &serum2::XferFile, version: &str) -> Result<(), FormatError> {
    if file.hash_ok == Some(false) {
        return Err(error("Serum source checksum does not match"));
    }
    let header = file.header_json()?;
    if header["productVersion"].as_str() != Some(version) || file.body.get("productVersion").and_then(cbor::Value::as_str) != Some(version)
    {
        return Err(error("Serum source product version differs from the verified installed version"));
    }
    if header["version"].as_f64() != Some(11.0) || file.body.get("version").and_then(cbor::Value::as_f64) != Some(11.0) {
        return Err(error("Serum source format version is not the verified format 11"));
    }
    Ok(())
}

fn retain_unknown_xfer(original: &serum2::XferFile, edited: &mut serum2::XferFile) -> Result<(), FormatError> {
    // Re-splitting a merged preset must not move controller-only (including
    // unknown future) sections into the processor, or fabricate another half.
    if let cbor::Value::Map(fields, _) = &mut edited.body {
        fields.retain(|(key, _)| key.as_str().is_some_and(|key| original.body.get(key).is_some()));
    }
    let mut body = original.body.clone();
    overlay(&mut body, &edited.body);
    edited.body = body;
    let mut header = original.header_json()?;
    if let (Some(old), Some(new)) = (header.as_object_mut(), edited.header_json()?.as_object()) {
        for (key, value) in new {
            if old.contains_key(key) {
                old.insert(key.clone(), value.clone());
            }
        }
    }
    edited.header = serde_json::to_vec(&header).map_err(|e| error(&e.to_string()))?;
    Ok(())
}

fn overlay(original: &mut cbor::Value, updated: &cbor::Value) {
    match (original, updated) {
        (cbor::Value::Map(old, _), cbor::Value::Map(new, _)) => {
            for (key, value) in new {
                if let Some((_, previous)) = old.iter_mut().find(|(old, _)| old == key) {
                    overlay(previous, value);
                } else {
                    old.push((key.clone(), value.clone()));
                }
            }
        }
        (cbor::Value::Array(old, _), cbor::Value::Array(new, _)) => {
            for (index, value) in new.iter().enumerate() {
                if let Some(previous) = old.get_mut(index) {
                    overlay(previous, value);
                } else {
                    old.push(value.clone());
                }
            }
            old.truncate(new.len());
        }
        (old, new) => *old = new.clone(),
    }
}

fn node_json(node: &Node) -> Value {
    match node {
        Node::Null => Value::Null,
        Node::Bool(value) => json!(value),
        Node::Int(value) => i64::try_from(*value)
            .map(|value| json!(value))
            .or_else(|_| u64::try_from(*value).map(|value| json!(value)))
            .unwrap_or_else(|_| json!(value.to_string())),
        Node::Float(value) => json!(value),
        Node::Text(value) => json!(value),
        Node::Bytes(size) => json!({"bytes":size}),
        Node::List(items) => Value::Array(items.iter().map(node_json).collect()),
        Node::Map(items) => Value::Object(items.iter().map(|(key, value)| (key.clone(), node_json(value))).collect()),
    }
}

fn error(message: &str) -> FormatError {
    FormatError::new(format!("Plug-in preset: {message}"))
}
