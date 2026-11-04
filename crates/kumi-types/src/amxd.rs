//! Max for Live's device file (.amxd): "ampf", the device's type, a "meta" chunk and a "ptch" chunk
//! holding the Max patcher as JSON (NUL-terminated), as Live's own device templates are laid out. The web tool reads one
//! wherever it finds it, and Live's device builder writes and reads them.

use kumi_common::js::json::stringify_with_indent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    MidiEffect,
    AudioEffect,
    Instrument,
}

impl DeviceType {
    pub const ALL: [DeviceType; 3] = [DeviceType::MidiEffect, DeviceType::AudioEffect, DeviceType::Instrument];

    pub fn as_str(self) -> &'static str {
        match self {
            DeviceType::MidiEffect => "midi_effect",
            DeviceType::AudioEffect => "audio_effect",
            DeviceType::Instrument => "instrument",
        }
    }

    pub fn parse(text: &str) -> Option<DeviceType> {
        DeviceType::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// The four-letter type code Live reads; the same letters, as a number, name it in the patcher's project.
    pub fn code(self) -> &'static str {
        match self {
            DeviceType::MidiEffect => "mmmm",
            DeviceType::AudioEffect => "aaaa",
            DeviceType::Instrument => "iiii",
        }
    }

    fn from_code(code: &[u8]) -> Option<DeviceType> {
        DeviceType::ALL.into_iter().find(|kind| kind.code().as_bytes() == code)
    }
}

impl std::fmt::Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn u32le(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

/// A device file from its patcher.
pub fn encode_amxd(kind: DeviceType, patcher: &Value) -> Vec<u8> {
    let code = kind.code().as_bytes();
    let mut body = format!("{}\n", stringify_with_indent(patcher, "\t")).into_bytes();
    body.push(0);
    let mut bytes = Vec::with_capacity(32 + body.len());
    bytes.extend_from_slice(b"ampf");
    bytes.extend_from_slice(&u32le(code.len() as u32));
    bytes.extend_from_slice(code);
    bytes.extend_from_slice(b"meta");
    bytes.extend_from_slice(&u32le(4));
    bytes.extend_from_slice(&u32le(0));
    bytes.extend_from_slice(b"ptch");
    bytes.extend_from_slice(&u32le(body.len() as u32));
    bytes.extend_from_slice(&body);
    bytes
}

/// A device file's type and patcher (`{ patcher: {...} }`), as [`decode_amxd`] reads them.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAmxd {
    pub kind: DeviceType,
    pub patcher: Value,
}

/// A device file's type and patcher; None when it isn't one.
pub fn decode_amxd(bytes: &[u8]) -> Option<DecodedAmxd> {
    let (code, chunk) = patcher_chunk(bytes)?;
    let kind = DeviceType::from_code(code)?;
    let text = String::from_utf8_lossy(chunk);
    let text = text.trim_end_matches('\0');
    serde_json::from_str::<Value>(text).ok().map(|patcher| DecodedAmxd { kind, patcher })
}

/// A device file's four-letter type code and its "ptch" chunk as it lies in the file: the patcher's JSON, or a frozen
/// device's container of files (which Live's device builder reads).
pub fn patcher_chunk(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    if bytes.len() < 12 || &bytes[0..4] != b"ampf" {
        return None;
    }
    let read_u32 = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    let length = read_u32(4);
    let code = &bytes[8..(8 + length).min(bytes.len())];
    let mut at = 8 + length;
    while at + 8 <= bytes.len() {
        let tag = &bytes[at..at + 4];
        let size = read_u32(at + 4);
        if tag == b"ptch" {
            return Some((code, &bytes[(at + 8).min(bytes.len())..(at + 8 + size).min(bytes.len())]));
        }
        at += 8 + size;
    }
    None
}

/// Whether a device file keeps its patcher encrypted, in a "ciph" chunk in place of "ptch", as Live's own Max for Live
/// devices do (LFO, Shaper, the Drum Synths…): nothing can read it but Live.
pub fn encrypted(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[0..4] != b"ampf" {
        return false;
    }
    let read_u32 = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    let mut at = 8usize.saturating_add(read_u32(4));
    while at.saturating_add(8) <= bytes.len() {
        match &bytes[at..at + 4] {
            b"ciph" => return true,
            b"ptch" => return false,
            _ => at = at.saturating_add(8).saturating_add(read_u32(at + 4)),
        }
    }
    false
}

/// The device file with `chunk` in place of its "ptch" chunk, and every other byte as it was (its type, its "meta" and
/// any other chunk); None when it isn't one.
pub fn with_patcher_chunk(bytes: &[u8], chunk: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 12 || &bytes[0..4] != b"ampf" {
        return None;
    }
    let read_u32 = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    let mut at = 8usize.saturating_add(read_u32(4));
    while at.saturating_add(8) <= bytes.len() {
        let size = read_u32(at + 4);
        if &bytes[at..at + 4] == b"ptch" {
            let rest = bytes.get(at.saturating_add(8).saturating_add(size)..)?;
            let mut out = Vec::with_capacity(bytes.len() + chunk.len());
            out.extend_from_slice(&bytes[..at + 4]);
            out.extend_from_slice(&u32le(u32::try_from(chunk.len()).ok()?));
            out.extend_from_slice(chunk);
            out.extend_from_slice(rest);
            return Some(out);
        }
        at = at.saturating_add(8).saturating_add(size);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_new_patcher_chunk_keeps_every_other_byte_of_the_file() {
        let mut file = encode_amxd(DeviceType::AudioEffect, &json!({ "patcher": { "title": "Old" } }));
        // A meta of Max's own, and a chunk after the patcher that Kumi doesn't know.
        file[20..24].copy_from_slice(&7u32.to_le_bytes());
        file.extend_from_slice(b"xtra\x02\x00\x00\x00ok");
        let chunk = b"{\"patcher\":{\"title\":\"New\"}}\n\0";
        let changed = with_patcher_chunk(&file, chunk).unwrap();
        assert_eq!(patcher_chunk(&changed), Some((&b"aaaa"[..], &chunk[..])));
        assert_eq!(decode_amxd(&changed).unwrap().patcher["patcher"]["title"], "New");
        assert_eq!(&changed[..28], &file[..28], "its type and meta as they were");
        assert!(changed.ends_with(b"xtra\x02\x00\x00\x00ok"));
        assert_eq!(with_patcher_chunk(b"not a device", chunk), None);
        assert!(!encrypted(&file));
    }

    #[test]
    fn a_device_whose_patcher_is_encrypted_is_known_as_one() {
        let mut lfo = b"ampf\x04\x00\x00\x00aaaameta\x04\x00\x00\x00\x00\x00\x00\x00ciph".to_vec();
        lfo.extend_from_slice(&4u32.to_le_bytes());
        lfo.extend_from_slice(b"\x01\x02\x03\x04");
        assert!(encrypted(&lfo));
        assert_eq!(patcher_chunk(&lfo), None);
        assert!(!encrypted(b"ampf"));
    }
}
