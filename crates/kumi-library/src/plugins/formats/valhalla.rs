//! Valhalla's `.vpreset` XML and the same XML inside its VST3 component state.
//! Sound parameters are normalized root attributes; Shimmer calls them `parameter0`,
//! `parameter1`, and so on. The per-product structure records their host names.

use serde_json::Value;

use super::juce_xml::{self, XmlState};
use super::tree::Node;
use super::FormatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValhallaPreset {
    state: XmlState,
    plugin: &'static str,
}

impl ValhallaPreset {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        let state = XmlState::read(bytes)?;
        if !matches!(
            state.document.root.name.as_str(),
            "ValhallaDelay"
                | "MYPLUGINSETTINGS"
                | "ValhallaFutureVerb"
                | "ValhallaPlate"
                | "ValhallaRoom"
                | "ValhallaSpaceModulator"
                | "ValhallaSupermassive"
                | "ValhallaUberMod"
                | "ValhallaVintageVerb"
        ) {
            return Err(FormatError::new("Valhalla preset: unknown XML root"));
        }
        let plugin = match state.document.root.name.as_str() {
            "ValhallaDelay" => "valhalla-delay",
            "MYPLUGINSETTINGS" if state.document.root.attribute("parameter0").is_some() => "valhalla-shimmer",
            "MYPLUGINSETTINGS" => "valhalla-freq-echo",
            "ValhallaFutureVerb" => "valhalla-future-verb",
            "ValhallaPlate" => "valhalla-plate",
            "ValhallaRoom" => "valhalla-room",
            "ValhallaSpaceModulator" => "valhalla-space-modulator",
            "ValhallaSupermassive" => "valhalla-supermassive",
            "ValhallaUberMod" => "valhalla-uber-mod",
            "ValhallaVintageVerb" => "valhalla-vintage-verb",
            _ => unreachable!(),
        };
        Ok(Self { state, plugin })
    }

    pub fn read_state(bytes: &[u8]) -> Result<Self, FormatError> {
        Self::read(bytes)
    }

    /// Preserves whether the input was native XML or VST3 component state.
    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        self.state.write()
    }

    pub fn write_state(&self) -> Result<Vec<u8>, FormatError> {
        self.state.write()
    }

    pub fn write_preset(&self) -> Vec<u8> {
        self.state.write_xml()
    }

    pub fn to_state(&self, template_component: &[u8]) -> Result<Vec<u8>, FormatError> {
        self.state.to_state(template_component)
    }

    pub fn check_version(&self, template_component: &[u8]) -> Result<(), FormatError> {
        self.state.check_version(template_component)
    }

    pub fn parameters(&self) -> impl Iterator<Item = (&str, f64)> {
        self.state.document.root.attributes.iter().filter_map(|(key, value)| {
            let value = value.parse::<f64>().ok()?;
            (is_parameter(key) && value.is_finite()).then_some((key.as_str(), value))
        })
    }

    pub fn set_parameter(&mut self, name: &str, value: f64) -> Result<(), FormatError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(FormatError::new("Valhalla parameters use normalized values from 0 to 1"));
        }
        if !self.parameters().any(|(key, _)| key == name) {
            return Err(FormatError::new(format!("Valhalla preset has no parameter {name}")));
        }
        super::structure::validate_number(self.plugin, &format!("parameters/{name}"), value)?;
        juce_xml::set_attribute(&mut self.state.document.root, &format!("@{name}"), &value.to_string())
    }

    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        let path = path.trim_start_matches('/');
        if let Some(name) = path.strip_prefix("parameters/") {
            self.set_parameter(name, value.as_f64().ok_or_else(|| FormatError::new("Valhalla parameter needs a number"))?)
        } else if path == "name" {
            let name = value.as_str().ok_or_else(|| FormatError::new("Valhalla preset name needs text"))?;
            if name.contains('\0') {
                return Err(FormatError::new("Valhalla preset name contains NUL"));
            }
            let root = &mut self.state.document.root;
            if let Some((_, value)) = root.attributes.iter_mut().find(|(key, _)| key == "presetName") {
                *value = name.into();
            } else {
                root.attributes.push(("presetName".into(), name.into()));
            }
            Ok(())
        } else {
            Err(FormatError::new("Valhalla edits use parameters/<field> or name"))
        }
    }

    pub fn to_node(&self) -> Node {
        Node::Map(vec![
            ("root".into(), Node::Text(self.state.document.root.name.clone())),
            ("name".into(), Node::Text(self.state.document.root.attribute("presetName").unwrap_or_default().into())),
            ("parameters".into(), Node::Map(self.parameters().map(|(key, value)| (key.into(), Node::Float(value))).collect())),
            ("state".into(), self.state.document.root.to_node()),
        ])
    }
}

fn is_parameter(key: &str) -> bool {
    !matches!(key, "pluginVersion" | "presetName" | "currentProgram" | "uiWidth" | "uiHeight" | "mixLock" | "earlyLateTabState")
}
