//! Serum 2 (Xfer Records). A preset (`.SerumPreset`) and each half of the state a host saves (the processor's
//! and the controller's) are the same container, which Xfer's other files (`.SerumFX`, `.XferShape`,
//! `.XferClip` …) use too:
//!
//! ```text
//! "XferJson\0" | u64 LE header length | header: JSON, in older files with a NUL inside the length
//!              | u32 LE length of the decoded body | u32 LE body encoding (2: Zstandard) | the encoded body, to the end
//! ```
//!
//! The header names the file (`fileType`, or `component` for a state half), Serum's version and the format's
//! (`version`), and carries `hash`, the MD5 of the encoded body. The body is CBOR: a map of sections
//! ("Oscillator0", "Env1", "FXRack0", "ModSlot12" …), each with `plainParams` (the knobs that differ from their
//! defaults, in Serum's own units) or the text "default". Wavetables, samples and impulse responses are named
//! by path (`relativePathToWT`, `relativePathToIR`), not embedded.

use std::io::Read;

use md5::{Digest, Md5};
use serde_json::Value as Json;

use super::cbor::{self, Head, Length, Value};
use super::{FormatError, MAX_DECODED_BYTES};

pub const MAGIC: &[u8] = b"XferJson\0";
/// The one body encoding seen: Zstandard.
pub const ZSTD: u32 = 2;

/// Whether bytes start as an Xfer container.
pub fn is_xfer(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

#[derive(Debug, Clone, PartialEq)]
pub struct XferFile {
    /// The header's bytes as written, kept so a round trip changes nothing it doesn't mean to.
    pub header: Vec<u8>,
    pub encoding: u32,
    pub body: Value,
    /// Whether the header's hash matched the encoded body (None when it has no hash).
    pub hash_ok: Option<bool>,
}

impl XferFile {
    pub fn read(bytes: &[u8]) -> Result<XferFile, FormatError> {
        let (file, _) = XferFile::read_with_body(bytes)?;
        Ok(file)
    }

    /// The file and its body's decoded bytes (the CBOR as Serum wrote it).
    pub fn read_with_body(bytes: &[u8]) -> Result<(XferFile, Vec<u8>), FormatError> {
        if !is_xfer(bytes) {
            return Err(FormatError::new("not an Xfer file: no XferJson magic"));
        }
        let mut at = MAGIC.len();
        let header_len = read_u64(bytes, &mut at)?;
        let header_len = usize::try_from(header_len).ok().filter(|n| *n <= bytes.len() - at).ok_or_else(|| short("header"))?;
        let header = bytes[at..at + header_len].to_vec();
        at += header_len;
        let decoded_len = read_u32(bytes, &mut at)? as usize;
        let encoding = read_u32(bytes, &mut at)?;
        let encoded = &bytes[at..];
        if encoding != ZSTD {
            return Err(FormatError::new(format!("Xfer body encoding {encoding} is unknown (2 is Zstandard)")));
        }
        if decoded_len > MAX_DECODED_BYTES {
            return Err(FormatError::new(format!("Xfer body would decode to {decoded_len} bytes")));
        }
        let decoded = zstd_decode(encoded, decoded_len)?;
        let body = cbor::decode(&decoded)?;
        let mut file = XferFile { header, encoding, body, hash_ok: None };
        if let Some(hash) = file.header_json()?.get("hash").and_then(Json::as_str) {
            file.hash_ok = Some(hash.eq_ignore_ascii_case(&md5_hex(encoded)));
        }
        Ok((file, decoded))
    }

    /// The header as JSON, any NUL padding left out.
    pub fn header_json(&self) -> Result<Json, FormatError> {
        let end = self.header.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        serde_json::from_slice(&self.header[..end]).map_err(|error| FormatError::new(format!("Xfer header isn't JSON: {error}")))
    }

    /// What the file is: its `fileType` ("SerumPreset", "SerumFX" …) or, for half a state, its `component`
    /// ("processor", "controller").
    pub fn kind(&self) -> Option<String> {
        let header = self.header_json().ok()?;
        header.get("fileType").or_else(|| header.get("component")).and_then(Json::as_str).map(str::to_string)
    }

    /// The container's bytes: the body encoded again, its length and the header's hash set to match.
    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        if self.encoding != ZSTD {
            return Err(FormatError::new(format!("Xfer body encoding {} can't be written", self.encoding)));
        }
        let decoded = cbor::encode(&self.body);
        let encoded = ruzstd::encoding::compress_to_vec(decoded.as_slice(), ruzstd::encoding::CompressionLevel::Fastest);
        let header = with_hash(&self.header, &md5_hex(&encoded));
        let mut out = Vec::with_capacity(MAGIC.len() + 16 + header.len() + encoded.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(header.len() as u64).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&u32::try_from(decoded.len()).map_err(|_| FormatError::new("Xfer body too long"))?.to_le_bytes());
        out.extend_from_slice(&self.encoding.to_le_bytes());
        out.extend_from_slice(&encoded);
        Ok(out)
    }
}

/// A `.SerumPreset` file.
#[derive(Debug, Clone, PartialEq)]
pub struct SerumPreset {
    pub file: XferFile,
}

impl SerumPreset {
    pub fn read(bytes: &[u8]) -> Result<SerumPreset, FormatError> {
        let file = XferFile::read(bytes)?;
        match file.kind().as_deref() {
            Some("SerumPreset") => Ok(SerumPreset { file }),
            other => Err(FormatError::new(format!("an Xfer file of type {other:?}, not a Serum 2 preset"))),
        }
    }

    pub fn header(&self) -> Json {
        self.file.header_json().unwrap_or(Json::Null)
    }

    pub fn name(&self) -> Option<String> {
        self.header().get("presetName").and_then(Json::as_str).map(str::to_string)
    }

    /// The Serum version that wrote it ("2.0.24").
    pub fn product_version(&self) -> Option<String> {
        self.header().get("productVersion").and_then(Json::as_str).map(str::to_string)
    }

    /// The format's own version number (`version` in the header: 4 to 11 so far).
    pub fn format_version(&self) -> Option<f64> {
        self.header().get("version").and_then(Json::as_f64)
    }

    pub fn tags(&self) -> Vec<String> {
        self.header()
            .get("tags")
            .and_then(Json::as_array)
            .map(|tags| tags.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    /// A section of the body ("Oscillator0", "Env1" …).
    pub fn section(&self, name: &str) -> Option<&Value> {
        self.file.body.get(name)
    }
}

/// The state a host saves for Serum 2: the processor's half (the sound) and the controller's (Serum's
/// window and the preset's name), each its own Xfer container. Live keeps them as a VST3 plug-in's
/// ProcessorState and ControllerState.
#[derive(Debug, Clone, PartialEq)]
pub struct SerumState {
    pub processor: XferFile,
    pub controller: Option<XferFile>,
}

impl SerumState {
    pub fn read(processor: &[u8], controller: Option<&[u8]>) -> Result<SerumState, FormatError> {
        let processor = XferFile::read(processor)?;
        if processor.kind().as_deref() != Some("processor") {
            return Err(FormatError::new(format!("Serum 2 processor state names itself {:?}", processor.kind())));
        }
        let controller = controller.filter(|bytes| !bytes.is_empty()).map(XferFile::read).transpose()?;
        if let Some(kind) = controller.as_ref().map(XferFile::kind) {
            if kind.as_deref() != Some("controller") {
                return Err(FormatError::new(format!("Serum 2 controller state names itself {kind:?}")));
            }
        }
        Ok(SerumState { processor, controller })
    }

    /// The preset name the controller half remembers.
    pub fn preset_name(&self) -> Option<String> {
        self.controller.as_ref()?.header_json().ok()?.get("presetName").and_then(Json::as_str).map(str::to_string)
    }

    /// The two halves as one tree, the way a preset holds them: every section of either half, and for a
    /// section in both, the controller's entries with the processor's laid over them. For a patch saved
    /// unedited this gives the preset's tree section for section (checked on Serum 2.1.5's init patch); the
    /// preset adds `fileType`, and the state keeps its own `component`, `presetHasBeenEdited`,
    /// `selectedPresetPath`, `modMatrixLocked` and `scalarCurvesLocked`.
    pub fn merged(&self) -> Value {
        let mut sections: Vec<(Value, Value)> = Vec::new();
        let halves = self.controller.iter().map(|c| &c.body).chain([&self.processor.body]);
        for half in halves {
            let Value::Map(entries, _) = half else { continue };
            for (key, value) in entries {
                match sections.iter_mut().find(|(k, _)| k == key) {
                    Some((_, Value::Map(existing, _))) if matches!(value, Value::Map(..)) => {
                        let Value::Map(over, _) = value else { unreachable!() };
                        for (inner_key, inner) in over {
                            match existing.iter_mut().find(|(k, _)| k == inner_key) {
                                Some((_, slot)) => *slot = inner.clone(),
                                None => existing.push((inner_key.clone(), inner.clone())),
                            }
                        }
                    }
                    Some((_, slot)) => *slot = value.clone(),
                    None => sections.push((key.clone(), value.clone())),
                }
            }
        }
        sections.sort_by(|(a, _), (b, _)| a.as_str().cmp(&b.as_str()));
        let length = Length::Definite(Head::shortest(sections.len() as u64));
        Value::Map(sections, length)
    }
}

fn short(what: &str) -> FormatError {
    FormatError::new(format!("Xfer file ends inside its {what}"))
}

fn read_u64(bytes: &[u8], at: &mut usize) -> Result<u64, FormatError> {
    let slice = bytes.get(*at..*at + 8).ok_or_else(|| short("header length"))?;
    *at += 8;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], at: &mut usize) -> Result<u32, FormatError> {
    let slice = bytes.get(*at..*at + 4).ok_or_else(|| short("body lengths"))?;
    *at += 4;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn md5_hex(bytes: &[u8]) -> String {
    hex::encode(Md5::digest(bytes))
}

/// One Zstandard frame that fills `encoded` and decodes to exactly `expected` bytes.
fn zstd_decode(encoded: &[u8], expected: usize) -> Result<Vec<u8>, FormatError> {
    let fail = |what: String| FormatError::new(format!("Xfer body: {what}"));
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(encoded).map_err(|error| fail(error.to_string()))?;
    let mut decoded = Vec::with_capacity(expected);
    (&mut decoder).take(expected as u64 + 1).read_to_end(&mut decoded).map_err(|error| fail(error.to_string()))?;
    if decoded.len() != expected {
        return Err(fail(format!("decodes to {} bytes, its header says {expected}", decoded.len())));
    }
    if !decoder.get_ref().is_empty() {
        return Err(fail(format!("{} bytes after its Zstandard frame", decoder.get_ref().len())));
    }
    Ok(decoded)
}

/// The header with its `"hash":"…"` value replaced, every other byte kept.
fn with_hash(header: &[u8], hash: &str) -> Vec<u8> {
    const KEY: &[u8] = b"\"hash\":\"";
    let Some(start) = header.windows(KEY.len()).position(|w| w == KEY).map(|i| i + KEY.len()) else {
        return header.to_vec();
    };
    let Some(len) = header[start..].iter().position(|b| *b == b'"') else {
        return header.to_vec();
    };
    [&header[..start], hash.as_bytes(), &header[start + len..]].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::formats::cbor::{Head, Length};

    fn container(header: &str, body: &Value) -> Vec<u8> {
        XferFile { header: header.as_bytes().to_vec(), encoding: ZSTD, body: body.clone(), hash_ok: None }.write().unwrap()
    }

    fn body() -> Value {
        let params = Value::Map(
            vec![(Value::text("kParamTablePos"), Value::Float(cbor::Float::Double(12.5f64.to_bits())))],
            Length::Definite(Head::Inline),
        );
        Value::Map(
            vec![
                (Value::text("Oscillator0"), Value::Map(vec![(Value::text("plainParams"), params)], Length::Definite(Head::Inline))),
                (
                    Value::text("Env1"),
                    Value::Map(vec![(Value::text("plainParams"), Value::text("default"))], Length::Definite(Head::Inline)),
                ),
            ],
            Length::Definite(Head::Inline),
        )
    }

    #[test]
    fn a_written_container_reads_back_with_its_hash() {
        let header = r#"{"fileType":"SerumPreset","hash":"00000000000000000000000000000000","presetName":"Kumi","productVersion":"2.1.5","tags":["Bass"],"version":11.0}"#;
        let bytes = container(header, &body());
        let preset = SerumPreset::read(&bytes).unwrap();
        assert_eq!(preset.file.hash_ok, Some(true));
        assert_eq!(preset.file.body, body());
        assert_eq!(preset.name().as_deref(), Some("Kumi"));
        assert_eq!(preset.product_version().as_deref(), Some("2.1.5"));
        assert_eq!(preset.format_version(), Some(11.0));
        assert_eq!(preset.tags(), ["Bass"]);
        assert_eq!(preset.section("Oscillator0").and_then(|s| s.at("plainParams/kParamTablePos")).and_then(Value::as_f64), Some(12.5));
        assert_eq!(preset.file.write().unwrap(), bytes);
    }

    #[test]
    fn a_nul_padded_header_still_reads() {
        let bytes = container("{\"fileType\":\"SerumFX\",\"version\":2.001}\0", &body());
        let file = XferFile::read(&bytes).unwrap();
        assert_eq!(file.kind().as_deref(), Some("SerumFX"));
        assert_eq!(file.hash_ok, None);
        assert!(SerumPreset::read(&bytes).is_err());
    }

    #[test]
    fn a_state_has_a_processor_and_maybe_a_controller() {
        let processor = container(r#"{"component":"processor","hash":"","version":11.0}"#, &body());
        let controller = container(r#"{"component":"controller","hash":"","presetName":" - Init - ","version":11.0}"#, &body());
        let state = SerumState::read(&processor, Some(&controller)).unwrap();
        assert_eq!(state.preset_name().as_deref(), Some(" - Init - "));
        assert!(SerumState::read(&processor, None).unwrap().controller.is_none());
        assert!(SerumState::read(&controller, None).is_err());
    }

    #[test]
    fn damage_is_refused() {
        let bytes = container(r#"{"fileType":"SerumPreset","hash":"x"}"#, &body());
        assert!(XferFile::read(&bytes[..bytes.len() - 3]).is_err());
        assert!(XferFile::read(&bytes[..20]).is_err());
        assert!(XferFile::read(b"XferJson\0\xff\xff\xff\xff\xff\xff\xff\xff").is_err());
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(b"junk");
        assert!(XferFile::read(&trailing).is_err());
        let mut other_encoding = bytes.clone();
        let header_len = u64::from_le_bytes(bytes[MAGIC.len()..MAGIC.len() + 8].try_into().unwrap()) as usize;
        let at = MAGIC.len() + 8 + header_len + 4;
        other_encoding[at] = 1;
        assert!(XferFile::read(&other_encoding).is_err());
        // write() set the hash; a header whose hash doesn't match still reads, flagged.
        let at = bytes.windows(8).position(|w| w == b"\"hash\":\"").unwrap() + 8;
        let mut tampered = bytes;
        tampered[at] = if tampered[at] == b'0' { b'1' } else { b'0' };
        assert_eq!(XferFile::read(&tampered).unwrap().hash_ok, Some(false));
    }
}
