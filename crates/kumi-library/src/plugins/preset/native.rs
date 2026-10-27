//! Native user files are separate from verified host state. These operations use
//! an explicitly supplied template and never claim that the result can be loaded
//! through a VST3 component-state API.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::{check_name, check_range, error, family, known, Edit, FormatError, PluginFormat, MAX_DECODED_BYTES};
use crate::plugins::formats::{softube, zynaptiq};

#[derive(Debug)]
pub struct NativePreset {
    pub bytes: Vec<u8>,
    pub plugin: String,
    pub extension: String,
    /// The source plug-in version when the native document identifies it.
    pub source_version: Option<String>,
    /// Native file writing is not evidence of acceptance by an installed plug-in.
    pub runtime_verified: bool,
    pub identity_note: String,
}

/// Edit an explicit native document without enabling an unverified VST3 writer.
pub fn edit_native(plugin: &str, input: &[u8], name: Option<&str>, edits: &[Edit]) -> Result<NativePreset, FormatError> {
    native(plugin, input, name, None, edits)
}

/// Create a new native user preset from one explicitly supplied source preset.
/// Softube bundles select `preset_index`; Zynaptiq files contain only index zero.
/// Output Softube edit paths address the new single preset at `presets/0/...`.
pub fn create_native_from_template(
    plugin: &str,
    input: &[u8],
    preset_index: usize,
    name: &str,
    edits: &[Edit],
) -> Result<NativePreset, FormatError> {
    native(plugin, input, Some(name), Some(preset_index), edits)
}

fn native(plugin: &str, input: &[u8], name: Option<&str>, template: Option<usize>, edits: &[Edit]) -> Result<NativePreset, FormatError> {
    let format = known(plugin)?;
    if input.len() > MAX_DECODED_BYTES {
        return Err(error("native preset exceeds the size limit"));
    }
    if input.starts_with(b"VST3") {
        return Err(error("native preset actions require a native document, not a VST3 container"));
    }
    if let Some(name) = name {
        check_name(name)?;
    }
    let (bytes, extension, source_version, identity_note) = match family(format) {
        "softube" => {
            let original = softube::Preset::read(input)?;
            let mut preset = match template {
                Some(index) => {
                    softube::Preset::from_template(&original, index, name.ok_or_else(|| error("a new native preset needs a name"))?)?
                }
                None => original,
            };
            validate_softube(format, &preset)?;
            if template.is_none() {
                if let Some(name) = name {
                    if preset.document["PresetList"].as_array().is_none_or(|presets| presets.len() != 1) {
                        return Err(error("a bundle has multiple names; edit presets/<index>/name explicitly"));
                    }
                    preset.set("presets/0/name", &json!(name))?;
                }
            }
            for edit in edits {
                preset.set(&edit.path, &edit.value)?;
            }
            validate_softube(format, &preset)?;
            let presets = preset.document["PresetList"].as_array().ok_or_else(|| error("missing native preset list"))?;
            let versions: BTreeSet<_> = presets.iter().filter_map(|preset| preset["PluginVersion"].as_str()).collect();
            let version = (versions.len() == 1).then(|| versions.iter().next().unwrap().to_string());
            (preset.write()?, if presets.len() == 1 { ".softubepreset" } else { ".softubebundle" }, version,
                "Native product ID and complete parameter ID set match the selected product. VST3 state creation and plug-in acceptance remain unverified.")
        }
        "zynaptiq" => {
            let original = zynaptiq::Preset::read(input)?;
            validate_zynaptiq(format, &original)?;
            check_zynaptiq_version(format, &original)?;
            let mut preset = match template {
                Some(0) => zynaptiq::Preset::from_template(&original, name.ok_or_else(|| error("a new native preset needs a name"))?)?,
                Some(_) => return Err(error("a Zynaptiq native preset contains only preset index 0")),
                None => original,
            };
            if let Some(name) = name {
                preset.set("@progName", &json!(name))?;
            }
            for edit in edits {
                check_range(format, edit)?;
                preset.set(&edit.path, &edit.value)?;
            }
            (preset.write()?, ".preset", None,
                "The parameter count matches the supplied product context. This native XML does not identify its product; the caller's selected product is retained, and plug-in acceptance remains unverified.")
        }
        _ => {
            return Err(error(
                "native template actions are available for Softube and Zynaptiq; other supported products use the verified VST3 workflow",
            ))
        }
    };
    Ok(NativePreset {
        bytes: if template.is_none() && name.is_none() && edits.is_empty() { input.to_vec() } else { bytes },
        plugin: plugin.into(),
        extension: extension.into(),
        source_version,
        runtime_verified: false,
        identity_note: identity_note.into(),
    })
}

pub(super) fn validate_softube(format: &PluginFormat, preset: &softube::Preset) -> Result<(), FormatError> {
    let expected = format
        .structure
        .files
        .get("preset")
        .and_then(|file| file.constants.get("nativePluginId"))
        .and_then(Value::as_str)
        .ok_or_else(|| error("no verified native product identity is recorded for this Softube product"))?;
    let expected_ids: BTreeSet<_> = format
        .structure
        .fields
        .keys()
        .filter_map(|path| path.strip_prefix("presets[]/parameters/").and_then(|id| id.parse::<u64>().ok()))
        .collect();
    if expected_ids.is_empty() {
        return Err(error("no native parameter layout is recorded for this Softube product"));
    }
    let presets = preset.document["PresetList"].as_array().ok_or_else(|| error("missing native preset list"))?;
    for item in presets {
        if item["PluginID"].as_str() != Some(expected) {
            return Err(error("native Softube preset belongs to a different product"));
        }
        let ids: BTreeSet<_> = item["ParameterData"]["Parameters"]
            .as_array()
            .ok_or_else(|| error("missing native parameters"))?
            .iter()
            .filter_map(|parameter| parameter[0].as_u64())
            .collect();
        if ids != expected_ids {
            return Err(error("native Softube parameter IDs differ from the selected product's recorded layout"));
        }
    }
    Ok(())
}

pub(super) fn validate_zynaptiq(format: &PluginFormat, preset: &zynaptiq::Preset) -> Result<(), FormatError> {
    let count = preset
        .document
        .root
        .child("inputParams")
        .and_then(|parameters| parameters.attribute("count"))
        .and_then(|count| count.parse::<u64>().ok())
        .ok_or_else(|| error("native Zynaptiq preset has no parameter count"))?;
    if !format.structure.survey["parameterCounts"].as_array().is_some_and(|counts| counts.iter().any(|known| known.as_u64() == Some(count)))
    {
        return Err(error("native Zynaptiq parameter count differs from the selected product's recorded layout"));
    }
    Ok(())
}

pub(super) fn check_zynaptiq_version(format: &PluginFormat, preset: &zynaptiq::Preset) -> Result<(), FormatError> {
    let version = preset.document.root.attribute("version").ok_or_else(|| error("native Zynaptiq preset has no format version"))?;
    if !format.structure.survey["presetVersions"]
        .as_array()
        .is_some_and(|versions| versions.iter().any(|known| known.as_str() == Some(version)))
    {
        return Err(error("native Zynaptiq format version differs from the selected product's recorded layout"));
    }
    Ok(())
}
