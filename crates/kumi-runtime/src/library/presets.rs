use super::{
    sets::device_name,
    xml::{attribute, scan_tags, xml_head, TagHandler},
};
use indexmap::IndexSet;
use kumi_common::js::string::{head, trim};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::AsyncReadExt;
pub const PRESET_EXTENSIONS: &[&str] = &[".adv", ".adg", ".amxd", ".vstpreset", ".aupreset", ".fxp", ".fxb"];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetCategory {
    #[serde(rename = "instrument")]
    Instrument,
    #[serde(rename = "audio effect")]
    AudioEffect,
    #[serde(rename = "midi effect")]
    MidiEffect,
    #[serde(rename = "drum rack")]
    DrumRack,
    #[serde(rename = "plug-in")]
    Plugin,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PresetFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<PresetCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}
fn category_of(tag: &str) -> PresetCategory {
    use PresetCategory::*;
    match tag {
        "InstrumentGroupDevice" => Instrument,
        "DrumGroupDevice" => DrumRack,
        "AudioEffectGroupDevice" => AudioEffect,
        "MidiEffectGroupDevice" => MidiEffect,
        "OriginalSimpler"
        | "MultiSampler"
        | "Operator"
        | "UltraAnalog"
        | "InstrumentVector"
        | "Drift"
        | "InstrumentMeld"
        | "Collision"
        | "LoungeLizard"
        | "StringStudio"
        | "InstrumentImpulse"
        | "DrumCell"
        | "ProxyInstrumentDevice" => Instrument,
        tag if tag.starts_with("Midi") => MidiEffect,
        _ => AudioEffect,
    }
}
#[derive(Default)]
struct Reader {
    stack: Vec<String>,
    root: Option<(String, usize)>,
    inside: IndexSet<String>,
    about: Option<String>,
    plugin: Option<String>,
}
impl TagHandler for Reader {
    fn open(&mut self, name: &str, attrs: &str, _: bool) {
        let depth = self.stack.len();
        let parent = self.stack.last().map(String::as_str).unwrap_or("");
        if self.root.is_none()
            && depth >= 1
            && self.stack[0] == "Ableton"
            && !matches!(name, "GroupDevicePreset" | "Device" | "OverwriteProtectionNumber" | "PresetRef")
        {
            self.root = Some((name.into(), depth));
        } else if self.root.is_some() && parent == "Devices" && name != "MacroControls" {
            self.inside.insert(device_name(name));
        }
        if let Some((_, root_depth)) = &self.root {
            if depth == root_depth + 1 && name == "Annotation" && self.about.is_none() {
                self.about = attribute(attrs, "Value").map(|s| head(trim(&s), 200)).filter(|s| !s.is_empty());
            }
            if (name == "PlugName" || (name == "Name" && matches!(parent, "Vst3PluginInfo" | "AuPluginInfo"))) && self.plugin.is_none() {
                self.plugin = attribute(attrs, "Value").filter(|s| !s.is_empty());
            }
        }
        self.stack.push(name.into());
    }
    fn close(&mut self, _: &str) {
        self.stack.pop();
    }
}
/// A Live preset's first device, a rack's devices and its annotation, read from its first bytes.
pub async fn read_live_preset(path: &str) -> std::io::Result<PresetFacts> {
    let text = xml_head(Path::new(path), 24 * 1024).await?;
    let mut reader = Reader::default();
    scan_tags(&text, &mut reader);
    let Some((tag, _)) = reader.root else {
        return Ok(PresetFacts::default());
    };
    if matches!(tag.as_str(), "PluginDevice" | "AuPluginDevice") {
        return Ok(PresetFacts {
            category: Some(PresetCategory::Plugin),
            device: reader.plugin,
            about: reader.about,
            ..Default::default()
        });
    }
    let category = if tag.starts_with("MxDevice") {
        match tag.as_str() {
            "MxDeviceInstrument" => PresetCategory::Instrument,
            "MxDeviceMidiEffect" => PresetCategory::MidiEffect,
            _ => PresetCategory::AudioEffect,
        }
    } else {
        category_of(&tag)
    };
    Ok(PresetFacts {
        device: Some(device_name(&tag)),
        category: Some(category),
        inside: (!reader.inside.is_empty()).then(|| reader.inside.into_iter().take(12).collect()),
        about: reader.about,
    })
}
/// A Max for Live device's kind comes from its `ampf` header.
pub async fn read_max_device(path: &str) -> std::io::Result<PresetFacts> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut head = [0_u8; 12];
    let _ = file.read(&mut head).await?;
    let category = (&head[..4] == b"ampf").then(|| match &head[8..12] {
        b"iiii" => PresetCategory::Instrument,
        b"mmmm" => PresetCategory::MidiEffect,
        _ => PresetCategory::AudioEffect,
    });
    Ok(PresetFacts { device: Some("Max for Live".into()), category, ..Default::default() })
}
pub fn plugin_preset_facts(relative_path: &str) -> PresetFacts {
    let parts: Vec<_> = relative_path.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    let plugin = if parts.len() >= 3 {
        Some(parts[1])
    } else if parts.len() == 2 {
        Some(parts[0])
    } else {
        None
    };
    PresetFacts { category: Some(PresetCategory::Plugin), device: plugin.map(String::from), ..Default::default() }
}
