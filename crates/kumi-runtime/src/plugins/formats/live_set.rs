//! Plug-in state as Live saves it, in a Set (`.als`) or a device preset (`.adv`): gzip'd XML. Live has no
//! API for a plug-in's state; the saved file holds it.
//!
//! A VST3 plug-in's `Vst3PluginInfo` carries its class ID (`Uid`, four signed 32-bit fields, big-endian) and,
//! under `Preset/Vst3Preset`, the plug-in's `ProcessorState` and `ControllerState` as hex. A VST2 plug-in's
//! `VstPluginInfo` carries its `UniqueId` and, under `Preset/VstPreset`, the chunk it handed Live in `Buffer`
//! (`Type` 'FBCh' for a bank chunk, 'FPCh' for one program's).

use std::io::Read;

use flate2::read::MultiGzDecoder;

use super::xml::{self, Element};
use super::{FormatError, MAX_DECODED_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginFormat {
    Vst2,
    Vst3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginState {
    pub format: PluginFormat,
    /// The plug-in's name as Live shows it ("Serum 2", "Vital").
    pub name: String,
    /// A VST3 class ID as 32 hex digits ("56534558667350736572756D20320000"), or a VST2 unique ID in decimal.
    pub id: String,
    /// VST3: the processor's state. VST2: the chunk.
    pub processor: Vec<u8>,
    /// VST3 only: the controller's state.
    pub controller: Vec<u8>,
    /// VST2 only: Live's `Type` for the chunk, as four characters ("FBCh").
    pub chunk_type: Option<String>,
}

/// Every plug-in state in a Set or device preset, gzip'd or not, in the order Live wrote them.
pub fn read(bytes: &[u8]) -> Result<Vec<PluginState>, FormatError> {
    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut text = String::new();
        MultiGzDecoder::new(bytes)
            .take(MAX_DECODED_BYTES as u64 * 4)
            .read_to_string(&mut text)
            .map_err(|error| FormatError::new(format!("Live file isn't gzip'd UTF-8: {error}")))?;
        text
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| FormatError::new("Live file isn't UTF-8"))?
    };
    let document = xml::parse(&text)?;
    let mut states = Vec::new();
    collect(&document.root, &mut states)?;
    Ok(states)
}

fn collect(element: &Element, states: &mut Vec<PluginState>) -> Result<(), FormatError> {
    match element.name.as_str() {
        "Vst3PluginInfo" => states.push(vst3(element)?),
        "VstPluginInfo" => states.push(vst2(element)?),
        _ => {
            for child in &element.children {
                collect(child, states)?;
            }
        }
    }
    Ok(())
}

fn value<'a>(element: &'a Element, child: &str) -> Option<&'a str> {
    element.child(child)?.attribute("Value")
}

fn hex(element: Option<&Element>) -> Result<Vec<u8>, FormatError> {
    let Some(element) = element else { return Ok(Vec::new()) };
    let digits: String = element.text.chars().filter(|c| !c.is_whitespace()).collect();
    hex::decode(digits).map_err(|error| FormatError::new(format!("Live's <{}> isn't hex: {error}", element.name)))
}

fn vst3(info: &Element) -> Result<PluginState, FormatError> {
    let uid = info.child("Uid").ok_or_else(|| FormatError::new("a VST3 plug-in without a Uid"))?;
    let mut id = String::new();
    for field in 0..4 {
        let raw: i64 = value(uid, &format!("Fields.{field}"))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| FormatError::new("a VST3 Uid field isn't a number"))?;
        id.push_str(&format!("{:08X}", raw as i32 as u32));
    }
    let preset = info.child("Preset").and_then(|p| p.child("Vst3Preset"));
    Ok(PluginState {
        format: PluginFormat::Vst3,
        name: value(info, "Name").unwrap_or_default().to_string(),
        id,
        processor: hex(preset.and_then(|p| p.child("ProcessorState")))?,
        controller: hex(preset.and_then(|p| p.child("ControllerState")))?,
        chunk_type: None,
    })
}

fn vst2(info: &Element) -> Result<PluginState, FormatError> {
    let preset = info.child("Preset").and_then(|p| p.child("VstPreset"));
    let chunk_type = preset
        .and_then(|p| value(p, "Type"))
        .and_then(|t| t.parse::<i64>().ok())
        .map(|t| String::from_utf8_lossy(&(t as u32).to_be_bytes()).into_owned());
    Ok(PluginState {
        format: PluginFormat::Vst2,
        name: value(info, "PlugName").unwrap_or_default().to_string(),
        id: value(info, "UniqueId").unwrap_or_default().to_string(),
        processor: hex(preset.and_then(|p| p.child("Buffer")))?,
        controller: Vec::new(),
        chunk_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use std::io::Write;

    const SET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5"><LiveSet><Tracks><MidiTrack Id="1"><DeviceChain><Devices>
  <PluginDevice Id="0"><PluginDesc><Vst3PluginInfo Id="0">
    <Preset><Vst3Preset Id="0">
      <ProcessorState>
        58666572
        4A736F6E
      </ProcessorState>
      <ControllerState />
    </Vst3Preset></Preset>
    <Name Value="Serum 2" />
    <Uid><Fields.0 Value="1448297816" /><Fields.1 Value="1718833267" /><Fields.2 Value="1701999981" /><Fields.3 Value="540147712" /></Uid>
  </Vst3PluginInfo></PluginDesc></PluginDevice>
  <PluginDevice Id="1"><PluginDesc><VstPluginInfo Id="0">
    <PlugName Value="Vital" /><UniqueId Value="1449751649" />
    <Preset><VstPreset Id="2"><Type Value="1178747752" /><Buffer>7B7D00</Buffer></VstPreset></Preset>
  </VstPluginInfo></PluginDesc></PluginDevice>
</Devices></DeviceChain></MidiTrack></Tracks></LiveSet></Ableton>"#;

    #[test]
    fn finds_each_plug_ins_state() {
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(SET.as_bytes()).unwrap();
        let states = read(&gz.finish().unwrap()).unwrap();
        assert_eq!(states, read(SET.as_bytes()).unwrap());
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].format, PluginFormat::Vst3);
        assert_eq!(states[0].name, "Serum 2");
        assert_eq!(states[0].id, "56534558667350736572756D20320000");
        assert_eq!(states[0].processor, b"XferJson");
        assert!(states[0].controller.is_empty());
        assert_eq!(states[1].format, PluginFormat::Vst2);
        assert_eq!(states[1].id, "1449751649");
        assert_eq!(states[1].processor, b"{}\0");
        assert_eq!(states[1].chunk_type.as_deref(), Some("FBCh"));
        assert!(read(b"<Ableton><Vst3PluginInfo /></Ableton>").is_err());
    }
}
