//! The XML envelope found in the supported JUCE-based VST3 plug-ins. This handles the
//! component state, including an optional compatibility bank and JUCE's opaque tail.
//! Neither a compatibility bank nor a preset extension enables a VST2 plug-in loader.

use super::xml::{self, Document, Element};
use super::{FormatError, MAX_DECODED_BYTES};

const MAGIC: &[u8; 4] = b"VC2!";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlState {
    pub document: Document,
    original_document: Document,
    original: Vec<u8>,
    envelope: Envelope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Envelope {
    Text,
    Binary { xml_start: usize, xml_len: usize, bank: Option<usize> },
}

impl XmlState {
    /// A native XML preset or a supported VST3 component state. Unknown suffix bytes
    /// remain opaque and are retained when the XML changes.
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(error("state exceeds the size limit"));
        }
        let (text, envelope) = if bytes.starts_with(MAGIC) || bytes.starts_with(b"VstW") {
            let bank = if bytes.starts_with(b"VstW") {
                let chunk = super::vstpreset::vst2_chunk(bytes)?.ok_or_else(|| error("missing compatibility bank"))?;
                if chunk.kind != *b"FBCh" {
                    return Err(error("unsupported compatibility program envelope"));
                }
                Some(8usize.checked_add(be(bytes, 4)? as usize).ok_or_else(|| error("bank offset overflow"))?)
            } else {
                None
            };
            let start = bank.map_or(0, |at| at + 160);
            if bytes.get(start..start + 4) != Some(MAGIC.as_slice()) {
                return Err(error("no binary XML header"));
            }
            let size_bytes: [u8; 4] = bytes.get(start + 4..start + 8).ok_or_else(|| error("truncated XML length"))?.try_into().unwrap();
            let xml_len = u32::from_le_bytes(size_bytes) as usize;
            let xml_start = start + 8;
            let end = xml_start.checked_add(xml_len).ok_or_else(|| error("XML length overflow"))?;
            let body = bytes.get(xml_start..end).ok_or_else(|| error("XML extends beyond the state"))?;
            if xml_len == 0 || bytes.get(end) != Some(&0) {
                return Err(error("missing XML or its terminator"));
            }
            if let Some(bank) = bank {
                let bank_end = bank.checked_add(8).and_then(|n| n.checked_add(be(bytes, bank + 4).ok()? as usize));
                let chunk_end = start.checked_add(be(bytes, bank + 156)? as usize);
                if bank_end != Some(bytes.len()) || chunk_end != Some(bytes.len()) {
                    return Err(error("inconsistent compatibility bank lengths"));
                }
            }
            (body, Envelope::Binary { xml_start, xml_len, bank })
        } else {
            (bytes, Envelope::Text)
        };
        let text = std::str::from_utf8(text).map_err(|_| error("XML is not UTF-8"))?;
        let document = xml::parse(text)?;
        Ok(Self { original_document: document.clone(), document, original: bytes.to_vec(), envelope })
    }

    /// An unchanged state is byte-exact. An edited state keeps its envelope, all
    /// unknown XML fields, and the private suffix; only affected lengths change.
    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        if self.document == self.original_document {
            return Ok(self.original.clone());
        }
        let body = xml::write(&self.document).into_bytes();
        if body.len() > MAX_DECODED_BYTES {
            return Err(error("XML exceeds the size limit"));
        }
        match self.envelope {
            Envelope::Text => Ok(body),
            Envelope::Binary { xml_start, xml_len, bank } => {
                let mut out = self.original[..xml_start].to_vec();
                out[xml_start - 4..xml_start].copy_from_slice(&(body.len() as u32).to_le_bytes());
                out.extend_from_slice(&body);
                out.extend_from_slice(&self.original[xml_start + xml_len..]);
                if out.len() > MAX_DECODED_BYTES {
                    return Err(error("state exceeds the size limit"));
                }
                if let Some(bank) = bank {
                    let bank_size = u32::try_from(out.len() - bank - 8).map_err(|_| error("bank too large"))?;
                    let chunk_size = u32::try_from(out.len() - bank - 160).map_err(|_| error("chunk too large"))?;
                    out[bank + 4..bank + 8].copy_from_slice(&bank_size.to_be_bytes());
                    out[bank + 156..bank + 160].copy_from_slice(&chunk_size.to_be_bytes());
                }
                Ok(out)
            }
        }
    }

    pub fn write_xml(&self) -> Vec<u8> {
        xml::write(&self.document).into_bytes()
    }

    /// Compare the product-version markers that are actually present in a
    /// verified component template. Unmarked formats cannot identify their
    /// source version; this does not manufacture a marker for them.
    pub fn check_version(&self, template: &[u8]) -> Result<(), FormatError> {
        let reference = Self::read(template)?;
        let markers = |state: &Self| {
            [
                state.document.root.attribute("pluginVersion").map(str::to_owned),
                state.document.root.child("Meta").and_then(|meta| meta.attribute("VERSION")).map(str::to_owned),
            ]
        };
        let actual = markers(self);
        for (index, expected) in markers(&reference).iter().enumerate() {
            if expected.is_some() && actual[index] != *expected {
                return Err(error("source preset version differs from the verified product template"));
            }
        }
        if let (Envelope::Binary { bank: Some(bank), .. }, Envelope::Binary { bank: Some(other), .. }) =
            (&self.envelope, &reference.envelope)
        {
            if self.original.get(bank + 20..bank + 24) != reference.original.get(other + 20..other + 24) {
                return Err(error("source compatibility-bank version differs from the verified product template"));
            }
        }
        Ok(())
    }

    /// Places a native preset's XML in the verified component envelope of the same
    /// product. The caller must also match the VST3 class ID of that template.
    pub fn to_state(&self, template: &[u8]) -> Result<Vec<u8>, FormatError> {
        let mut state = Self::read(template)?;
        if matches!(state.envelope, Envelope::Text) {
            return Err(error("the state template must have a binary component envelope"));
        }
        if state.document.root.name != self.document.root.name {
            return Err(error("the preset and component template have different XML roots"));
        }
        // Two older Valhalla products share a generic root name. Shimmer uses indexed
        // attributes; FreqEcho uses named controls. Do not transplant between them.
        if self.document.root.name == "MYPLUGINSETTINGS"
            && state.document.root.attribute("parameter0").is_some() != self.document.root.attribute("parameter0").is_some()
        {
            return Err(error("the legacy Valhalla preset belongs to another product"));
        }
        if let Envelope::Binary { bank, .. } = self.envelope {
            let Envelope::Binary { bank: reference, .. } = state.envelope else { unreachable!() };
            if bank.is_some() != reference.is_some() {
                return Err(error("binary preset and component use different envelopes; a native XML document is required"));
            }
            if let (Some(bank), Some(reference)) = (bank, reference) {
                if self.original.get(bank + 16..bank + 20) != state.original.get(reference + 16..reference + 20) {
                    return Err(error("compatibility bank belongs to a different product"));
                }
            }
            // A native binary preset is already a component document. Its private
            // suffix belongs to that preset and must not be replaced by init data.
            return self.write();
        }
        state.document = self.document.clone();
        state.write()
    }
}

fn be(bytes: &[u8], at: usize) -> Result<u32, FormatError> {
    let word: [u8; 4] = bytes.get(at..at + 4).ok_or_else(|| error("truncated bank"))?.try_into().unwrap();
    Ok(u32::from_be_bytes(word))
}

fn error(message: &str) -> FormatError {
    FormatError::new(format!("VST3 XML state: {message}"))
}

/// Updates an existing XML attribute using the same paths as `Element::to_node`.
/// A repeated child must have an explicit zero-based index, such as `PARAM[2]/@value`.
/// No new XML structure or attribute is invented by this operation.
pub fn set_attribute(root: &mut Element, path: &str, value: &str) -> Result<(), FormatError> {
    let path = path.trim_start_matches('/');
    let mut parts = path.split('/').peekable();
    let mut current = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let name = part.strip_prefix('@').ok_or_else(|| error("an edit must address an existing @attribute"))?;
            let attribute = current.attributes.iter_mut().find(|(key, _)| key == name).ok_or_else(|| error("unknown attribute path"))?;
            attribute.1 = value.to_string();
            return Ok(());
        }
        let (name, index) = if let Some((name, index)) = part.strip_suffix(']').and_then(|s| s.rsplit_once('[')) {
            (name, Some(index.parse::<usize>().map_err(|_| error("invalid child index"))?))
        } else {
            (part, None)
        };
        if name.is_empty() {
            return Err(error("empty child path"));
        }
        let count = current.children.iter().filter(|child| child.name == name).count();
        if count > 1 && index.is_none() {
            return Err(error("a repeated child needs an index"));
        }
        current = current
            .children
            .iter_mut()
            .filter(|child| child.name == name)
            .nth(index.unwrap_or(0))
            .ok_or_else(|| error("unknown child path"))?;
    }
    Err(error("empty attribute path"))
}

pub fn scalar(value: &serde_json::Value) -> Result<String, FormatError> {
    match value {
        serde_json::Value::String(value) if !value.contains('\0') => Ok(value.clone()),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        serde_json::Value::Bool(value) => Ok(if *value { "1" } else { "0" }.into()),
        _ => Err(error("an XML attribute requires a finite number, string or boolean")),
    }
}
