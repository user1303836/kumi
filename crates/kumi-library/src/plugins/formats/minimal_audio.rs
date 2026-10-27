//! Minimal Audio's full preset XML and VST3 state. `mapped_value` is authoritative
//! on restore; `unmapped_value` is a cached normalized representation. Kumi edits
//! physical values and lets the plug-in refresh that cache on loading.

use serde_json::Value;

use super::juce_xml::{self, XmlState};
use super::tree::Node;
use super::xml::Element;
use super::FormatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimalAudioPreset {
    state: XmlState,
}

impl MinimalAudioPreset {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        let state = XmlState::read(bytes)?;
        let root = &state.document.root;
        if !matches!(
            root.name.as_str(),
            "ClusterDelay"
                | "Current"
                | "EPROM-MemoryRites"
                | "Evoke"
                | "FlexChorus"
                | "Formant"
                | "FuseCompressor"
                | "HybridFilter"
                | "Lucid"
                | "MorphEQ"
                | "PolyFlanger"
                | "Rift"
                | "RipplePhaser"
                | "Squash"
                | "SwarmReverb"
                | "WaveShifter"
        ) || root.child("App_State").and_then(|app| app.child("Parameters")).is_none()
        {
            return Err(FormatError::new("Minimal Audio preset: no supported full preset state"));
        }
        Ok(Self { state })
    }

    pub fn read_state(bytes: &[u8]) -> Result<Self, FormatError> {
        Self::read(bytes)
    }

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

    fn parameter_elements(&self) -> &[Element] {
        &self.state.document.root.child("App_State").unwrap().child("Parameters").unwrap().children
    }

    /// Values in the plug-in's own units, not host-normalized values.
    pub fn parameters(&self) -> impl Iterator<Item = (&str, f64)> {
        self.parameter_elements().iter().filter_map(|element| {
            let value = element.attribute("mapped_value")?.parse::<f64>().ok()?;
            value.is_finite().then_some((element.name.as_str(), value))
        })
    }

    pub fn set_parameter(&mut self, name: &str, value: f64) -> Result<(), FormatError> {
        if !value.is_finite() {
            return Err(FormatError::new("Minimal Audio parameter needs a finite physical value"));
        }
        if !self.parameters().any(|(key, _)| key == name) {
            return Err(FormatError::new(format!("Minimal Audio preset has no parameter {name}")));
        }
        juce_xml::set_attribute(&mut self.state.document.root, &format!("App_State/Parameters/{name}/@mapped_value"), &value.to_string())
    }

    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        let path = path.trim_start_matches('/');
        if let Some(name) = path.strip_prefix("parameters/") {
            self.set_parameter(name, value.as_f64().ok_or_else(|| FormatError::new("Minimal Audio parameter needs a number"))?)
        } else if let Some(path) = path.strip_prefix("state/App_State/") {
            if path.starts_with("Parameters/") {
                return Err(FormatError::new("Edit parameters/<field> in physical units; normalized caches are not authoritative"));
            }
            juce_xml::set_attribute(&mut self.state.document.root, &format!("App_State/{path}"), &juce_xml::scalar(value)?)
        } else {
            Err(FormatError::new("Minimal Audio edits use parameters/<field> or an existing state/App_State XML attribute"))
        }
    }

    pub fn to_node(&self) -> Node {
        Node::Map(vec![
            ("root".into(), Node::Text(self.state.document.root.name.clone())),
            ("parameters".into(), Node::Map(self.parameters().map(|(key, value)| (key.into(), Node::Float(value))).collect())),
            ("state".into(), self.state.document.root.to_node()),
        ])
    }
}
