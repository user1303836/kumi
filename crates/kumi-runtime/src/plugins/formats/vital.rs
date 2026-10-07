//! Vital (Matt Tytel). A preset (`.vital`) is JSON text: the preset's name, author, comments, style and macro
//! names, `synth_version`, and `settings`, which holds every control by name ("osc_1_level", "filter_1_cutoff",
//! in the synth's own units), `modulations` (source and destination by name, and a curve when not linear),
//! the three oscillators' `wavetables` (keyframes with their 2048-sample frames as base64 32-bit floats),
//! the eight `lfos` (points, powers, smoothing) and the `sample` (base64 16-bit PCM).
//!
//! The state a host saves is that same JSON with the tuning added under `tuning`, written as a string with a
//! closing NUL; the plug-in framework (JUCE) may add its own data after the NUL. The VST3 build wraps that
//! chunk as its VST2 build's: see `vstpreset::vst2_chunk`. The format is read from Vital's published source
//! (GPL-3.0); none of its code is here.

use base64::Engine;
use serde_json::{Map, Value};

use super::tree::Node;
use super::FormatError;

/// A preset, or the state a host saved.
#[derive(Debug, Clone, PartialEq)]
pub struct VitalPreset {
    pub json: Value,
}

impl VitalPreset {
    /// A `.vital` file.
    pub fn read(bytes: &[u8]) -> Result<VitalPreset, FormatError> {
        let json: Value = serde_json::from_slice(bytes).map_err(|error| FormatError::new(format!("Vital preset isn't JSON: {error}")))?;
        check(json)
    }

    /// The state a host saved (the VST2 chunk): the same JSON with a tuning inside, up to its NUL. What
    /// follows the NUL is the framework's, not Vital's.
    pub fn read_state(bytes: &[u8]) -> Result<VitalPreset, FormatError> {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let json: Value =
            serde_json::from_slice(&bytes[..end]).map_err(|error| FormatError::new(format!("Vital state isn't JSON: {error}")))?;
        check(json)
    }

    /// A `.vital` file's bytes.
    pub fn write(&self) -> Vec<u8> {
        serde_json::to_vec(&self.json).expect("JSON values serialize")
    }

    pub fn settings(&self) -> &Map<String, Value> {
        self.json["settings"].as_object().expect("checked on read")
    }

    /// The Vital version that wrote it ("1.0.7").
    pub fn synth_version(&self) -> Option<&str> {
        self.json.get("synth_version").and_then(Value::as_str)
    }

    pub fn name(&self) -> Option<&str> {
        self.json.get("preset_name").and_then(Value::as_str).filter(|name| !name.is_empty())
    }

    /// Every control with its value, in the synth's own units.
    pub fn controls(&self) -> impl Iterator<Item = (&str, f64)> {
        self.settings().iter().filter_map(|(name, value)| value.as_f64().map(|value| (name.as_str(), value)))
    }

    /// The modulations in use: source and destination by name.
    pub fn modulations(&self) -> Vec<(&str, &str)> {
        self.settings()
            .get("modulations")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|m| Some((m.get("source")?.as_str()?, m.get("destination")?.as_str()?)))
                    .filter(|(source, destination)| !source.is_empty() && !destination.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The tuning a host's state carries (none in a preset file).
    pub fn tuning(&self) -> Option<&Value> {
        self.json.get("tuning")
    }

    /// One oscillator's wavetable keyframes (0 to 2), each frame's samples decoded: groups, then components,
    /// then keyframes in order. A component drawn from a line or a file has no frames of its own here.
    pub fn wavetable_frames(&self, oscillator: usize) -> Result<Vec<Vec<f32>>, FormatError> {
        let Some(table) = self.settings().get("wavetables").and_then(Value::as_array).and_then(|t| t.get(oscillator)) else {
            return Ok(Vec::new());
        };
        let mut frames = Vec::new();
        for group in table.get("groups").and_then(Value::as_array).into_iter().flatten() {
            for component in group.get("components").and_then(Value::as_array).into_iter().flatten() {
                for keyframe in component.get("keyframes").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(data) = keyframe.get("wave_data").and_then(Value::as_str) {
                        frames.push(floats(data)?);
                    }
                }
            }
        }
        Ok(frames)
    }

    pub fn to_node(&self) -> Node {
        Node::from(&self.json)
    }
}

fn check(json: Value) -> Result<VitalPreset, FormatError> {
    if !json.get("settings").is_some_and(Value::is_object) {
        return Err(FormatError::new("Vital preset has no settings"));
    }
    Ok(VitalPreset { json })
}

/// Base64 little-endian 32-bit floats: a wavetable frame as Vital keeps it.
pub fn floats(base64: &str) -> Result<Vec<f32>, FormatError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .map_err(|error| FormatError::new(format!("Vital frame isn't base64: {error}")))?;
    Ok(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// Base64 little-endian 16-bit PCM: a sample as Vital keeps it.
pub fn pcm16(base64: &str) -> Result<Vec<i16>, FormatError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .map_err(|error| FormatError::new(format!("Vital sample isn't base64: {error}")))?;
    Ok(bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn preset() -> Value {
        let frame: Vec<u8> = (0..4).flat_map(|i| (i as f32 * 0.5).to_le_bytes()).collect();
        json!({
            "author": "Kumi", "comments": "", "macro1": "MACRO 1", "preset_name": "Init", "preset_style": "", "synth_version": "1.0.7",
            "settings": {
                "osc_1_level": 0.70710677, "filter_1_cutoff": 60.0,
                "modulations": [{"source": "lfo_1", "destination": "filter_1_cutoff"}, {"source": "", "destination": ""}],
                "wavetables": [{"groups": [{"components": [{"keyframes": [{"position": 0, "wave_data": base64::engine::general_purpose::STANDARD.encode(&frame)}], "type": "Wave Source"}]}], "name": "Init"}],
                "lfos": [{"name": "Triangle", "num_points": 3, "points": [0.0, 1.0, 0.5, 0.0, 1.0, 1.0], "powers": [0.0, 0.0, 0.0], "smooth": false}]
            }
        })
    }

    #[test]
    fn reads_a_preset_and_writes_the_same_tree() {
        let bytes = serde_json::to_vec(&preset()).unwrap();
        let read = VitalPreset::read(&bytes).unwrap();
        assert_eq!(read.synth_version(), Some("1.0.7"));
        assert_eq!(read.name(), Some("Init"));
        assert_eq!(read.controls().collect::<Vec<_>>(), [("osc_1_level", 0.70710677), ("filter_1_cutoff", 60.0)]);
        assert_eq!(read.modulations(), [("lfo_1", "filter_1_cutoff")]);
        assert_eq!(read.wavetable_frames(0).unwrap(), [vec![0.0, 0.5, 1.0, 1.5]]);
        assert!(read.wavetable_frames(2).unwrap().is_empty());
        assert_eq!(VitalPreset::read(&read.write()).unwrap(), read);
    }

    #[test]
    fn a_state_is_the_preset_with_a_tuning_and_a_nul() {
        let mut state = preset();
        state["tuning"] = json!({"mapping_name": "", "scale": [0.0, 1.0]});
        let mut bytes = serde_json::to_vec(&state).unwrap();
        bytes.push(0);
        bytes.extend_from_slice(b"\0\0\0\0\0\0\0\0JUCEPrivateData");
        let read = VitalPreset::read_state(&bytes).unwrap();
        assert!(read.tuning().is_some());
        assert!(VitalPreset::read(&bytes).is_err(), "a preset file has no NUL");
        assert!(VitalPreset::read(b"{\"synth_version\":\"1.0.7\"}").is_err());
        assert_eq!(pcm16("AAD/fw==").unwrap(), [0, 32767]);
    }
}
