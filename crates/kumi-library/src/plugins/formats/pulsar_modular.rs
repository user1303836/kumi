//! Pulsar Modular's binary-XML presets and VST3 states. A `PARAM` element identifies
//! its control by `id`; its `value` is in the control's native units. A missing value
//! represents zero. Unknown XML nodes and JUCE's private state remain intact.

use serde_json::Value;

use super::juce_xml::{self, XmlState};
use super::tree::Node;
use super::FormatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulsarModularPreset {
    state: XmlState,
    plugin: &'static str,
}

impl PulsarModularPreset {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        let state = XmlState::read(bytes)?;
        let plugin = match state.document.root.name.as_str() {
            "Abyss" => "pulsar-p11-abyss",
            "P19" => "pulsar-p19-igloo",
            "P42_Climax" => "pulsar-p42-climax",
            "P422" => "pulsar-p422-fairuz",
            "P44" => "pulsar-p44-magnum",
            "P440_Sweet_Spot" => "pulsar-p440-sweet-spot",
            "P450" => "pulsar-p450-mdn-eq",
            "P455" => "pulsar-p455-mdn-sidecar",
            "P565_Siren" => "pulsar-p565-siren",
            "P821" => "pulsar-p821-mdn-tape",
            "P915" => "pulsar-p915-medusa",
            "P930" => "pulsar-p930-lunar-lander",
            "QSHR" => "pulsar-q-sahara",
            _ => return Err(FormatError::new("Pulsar Modular preset: unsupported XML root")),
        };
        if !state.document.root.children.iter().any(|child| child.name == "PARAM") {
            return Err(FormatError::new("Pulsar Modular preset: no supported parameter tree"));
        }
        Ok(Self { state, plugin })
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

    pub fn write_preset(&self) -> Result<Vec<u8>, FormatError> {
        self.state.write()
    }

    pub fn to_state(&self, template_component: &[u8]) -> Result<Vec<u8>, FormatError> {
        self.state.to_state(template_component)
    }

    pub fn check_version(&self, template_component: &[u8]) -> Result<(), FormatError> {
        self.state.check_version(template_component)
    }

    pub fn parameters(&self) -> impl Iterator<Item = (&str, f64)> {
        self.state.document.root.children.iter().filter(|element| element.name == "PARAM").filter_map(|element| {
            let name = element.attribute("id")?;
            if !self.is_current_parameter(name) {
                return None;
            }
            let value = element.attribute("value").unwrap_or("0").parse::<f64>().ok()?;
            value.is_finite().then_some((name, value))
        })
    }

    pub fn set_parameter(&mut self, name: &str, value: f64) -> Result<(), FormatError> {
        if !value.is_finite() {
            return Err(FormatError::new("Pulsar Modular parameter needs a finite physical value"));
        }
        if !self.is_current_parameter(name) {
            return Err(FormatError::new(format!(
                "Pulsar Modular parameter {name} is not a current host control; legacy state is preserved"
            )));
        }
        let matches = self.state.document.root.children.iter().filter(|e| e.name == "PARAM" && e.attribute("id") == Some(name)).count();
        if matches != 1 {
            return Err(FormatError::new(format!("Pulsar Modular parameter {name} is missing or ambiguous")));
        }
        let parameter =
            self.state.document.root.children.iter_mut().find(|e| e.name == "PARAM" && e.attribute("id") == Some(name)).unwrap();
        if let Some((_, current)) = parameter.attributes.iter_mut().find(|(key, _)| key == "value") {
            *current = value.to_string();
        } else {
            parameter.attributes.push(("value".into(), value.to_string()));
        }
        Ok(())
    }

    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        let path = path.trim_start_matches('/');
        if let Some(name) = path.strip_prefix("parameters/") {
            self.set_parameter(name, value.as_f64().ok_or_else(|| FormatError::new("Pulsar Modular parameter needs a number"))?)
        } else if let Some(path) = path.strip_prefix("state/") {
            if path.starts_with("PARAM") {
                return Err(FormatError::new("Edit parameters/<id> in the control's native units"));
            }
            juce_xml::set_attribute(&mut self.state.document.root, path, &juce_xml::scalar(value)?)
        } else {
            Err(FormatError::new("Pulsar Modular edits use parameters/<id> or an existing state XML attribute"))
        }
    }

    pub fn to_node(&self) -> Node {
        Node::Map(vec![
            ("root".into(), Node::Text(self.state.document.root.name.clone())),
            ("parameters".into(), Node::Map(self.parameters().map(|(key, value)| (key.into(), Node::Float(value))).collect())),
            ("state".into(), self.state.document.root.to_node()),
        ])
    }

    fn is_current_parameter(&self, name: &str) -> bool {
        let path = format!("parameters/{name}");
        super::structure::builtin(self.plugin).is_some_and(|format| format.structure.parameters.values().any(|link| link.field == path))
    }
}
