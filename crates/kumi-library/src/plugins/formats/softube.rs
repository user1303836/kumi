//! Softube preset files and bundles. The JSON document carries one or more
//! presets, each with `[parameter ID, normalized value, name]` tuples. This
//! reader preserves shared data and unknown metadata when changing a parameter.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::{bounded_json, tree::Node, FormatError, MAX_DECODED_BYTES};

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub document: Value,
}

impl Preset {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(error("document is too large"));
        }
        let preset = Self { document: bounded_json::parse(bytes)? };
        preset.validate()?;
        Ok(preset)
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.document["AssetType"] != "SoftubePresetFile" || self.document["FileFormatVersion"] != 1 {
            return Err(error("expected SoftubePresetFile format version 1"));
        }
        let presets = self.document["PresetList"].as_array().ok_or_else(|| error("missing PresetList"))?;
        if presets.is_empty() {
            return Err(error("empty PresetList"));
        }
        for preset in presets {
            if !preset["PluginID"].is_string() || !preset["PluginName"].is_string() || !preset["PluginVersion"].is_string() {
                return Err(error("missing plug-in identity"));
            }
            if preset["ParameterData"]["Type"] != "Full" {
                return Err(error("only full parameter sets can be edited"));
            }
            let parameters = preset["ParameterData"]["Parameters"].as_array().ok_or_else(|| error("missing Parameters"))?;
            if parameters.is_empty() || parameters.len() > 65536 {
                return Err(error("invalid parameter count"));
            }
            let mut ids = BTreeSet::new();
            for parameter in parameters {
                let tuple = parameter.as_array().filter(|t| t.len() >= 2).ok_or_else(|| error("invalid parameter tuple"))?;
                let id = tuple[0].as_u64().filter(|id| *id <= u32::MAX as u64).ok_or_else(|| error("invalid parameter ID"))?;
                tuple[1]
                    .as_f64()
                    .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                    .ok_or_else(|| error("parameter must be normalized to 0..1"))?;
                if !ids.insert(id) {
                    return Err(error("duplicate parameter ID"));
                }
            }
        }
        Ok(())
    }

    /// Creates a user preset from an explicitly supplied full parameter set.
    /// Defaults must come from the installed plug-in, not an inferred schema.
    pub fn create(
        plugin_id: &str,
        plugin_name: &str,
        version: &str,
        name: &str,
        parameters: &[(u32, f64, String)],
    ) -> Result<Self, FormatError> {
        let result = Self {
            document: json!({
                "AssetType": "SoftubePresetFile", "FileFormatVersion": 1,
                "PresetList": [{"PluginID":plugin_id,"PluginName":plugin_name,"PluginVersion":version,
                    "PresetName":name,"Description":"","Tags":[],"Rank":0,"UUID":uuid::Uuid::new_v4().to_string(),
                    "ParameterData":{"Type":"Full","Parameters":parameters}}],
                "SharedData":{}
            }),
        };
        result.validate()?;
        Ok(result)
    }

    /// Copies one explicit native preset as a new user preset. Shared resources
    /// and unknown fields are retained, and a fresh UUID distinguishes the copy.
    /// This does not infer initialized values or produce VST3 component state.
    pub fn from_template(template: &Self, preset_index: usize, name: &str) -> Result<Self, FormatError> {
        template.validate()?;
        let mut preset = template.document["PresetList"]
            .as_array()
            .and_then(|presets| presets.get(preset_index))
            .cloned()
            .ok_or_else(|| error("unknown template preset"))?;
        preset["UUID"] = json!(uuid::Uuid::new_v4().to_string());
        let mut result = template.clone();
        result.document["PresetList"] = Value::Array(vec![preset]);
        result.set("presets/0/name", &json!(name))?;
        Ok(result)
    }

    pub fn set_parameter(&mut self, preset_index: usize, id: u32, value: f64) -> Result<(), FormatError> {
        self.validate()?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(error("parameter must be normalized to 0..1"));
        }
        let parameters = self
            .document
            .get_mut("PresetList")
            .and_then(Value::as_array_mut)
            .and_then(|p| p.get_mut(preset_index))
            .and_then(|p| p.get_mut("ParameterData"))
            .and_then(|p| p.get_mut("Parameters"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| error("unknown preset"))?;
        let parameter = parameters.iter_mut().find(|p| p[0].as_u64() == Some(id as u64)).ok_or_else(|| error("unknown parameter ID"))?;
        parameter[1] = json!(value);
        Ok(())
    }

    /// Paths returned by `to_node`, for example `presets/0/parameters/2`.
    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        let parts: Vec<_> = path.split('/').collect();
        let index = parts.get(1).and_then(|s| s.parse::<usize>().ok()).ok_or_else(|| error("expected presets/<index>/parameters/<id>"))?;
        match parts.as_slice() {
            ["presets", _, "parameters", id] => self.set_parameter(
                index,
                id.parse().map_err(|_| error("invalid parameter ID"))?,
                value.as_f64().ok_or_else(|| error("parameter must be a number"))?,
            ),
            ["presets", _, "name"] => {
                let name = value.as_str().ok_or_else(|| error("preset name must be text"))?;
                let preset = self
                    .document
                    .get_mut("PresetList")
                    .and_then(Value::as_array_mut)
                    .and_then(|p| p.get_mut(index))
                    .ok_or_else(|| error("unknown preset"))?;
                preset["PresetName"] = json!(name);
                Ok(())
            }
            _ => Err(error("expected presets/<index>/parameters/<id> or presets/<index>/name")),
        }
    }

    pub fn to_node(&self) -> Node {
        let presets = self.document["PresetList"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|preset| {
                let parameters = preset["ParameterData"]["Parameters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Some((p[0].as_u64()?.to_string(), Node::Float(p[1].as_f64()?))))
                    .collect();
                Node::Map(vec![
                    ("name".into(), Node::from(&preset["PresetName"])),
                    ("plugin".into(), Node::from(&preset["PluginName"])),
                    ("version".into(), Node::from(&preset["PluginVersion"])),
                    ("parameters".into(), Node::Map(parameters)),
                ])
            })
            .collect();
        Node::Map(vec![("presets".into(), Node::List(presets))])
    }

    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(&self.document).map_err(|e| error(&e.to_string()))?;
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(error("document is too large"));
        }
        Ok(bytes)
    }
}

fn error(message: &str) -> FormatError {
    FormatError::new(format!("Softube preset: {message}"))
}
