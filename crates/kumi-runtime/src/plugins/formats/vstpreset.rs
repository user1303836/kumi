//! VST3 preset files (`.vstpreset`), the container every VST3 host shares. Live writes one when a VST3 device is
//! dragged into its browser, and loads one onto a new device from there.
//!
//! ```text
//! "VST3" | i32 LE version (1) | the class ID as 32 ASCII hex digits | i64 LE offset of the chunk list
//! | the chunks' data … | "List" | i32 LE count | per chunk: 4-character ID, i64 LE offset, i64 LE size
//! ```
//!
//! "Comp" is the processor's state, "Cont" the controller's, "Info" an XML block of attributes (name, vendor,
//! category). A plug-in built to stay compatible with its VST2 version wraps its processor state as that
//! VST2's chunk: "VstW" (a big-endian size, version and bypass flag), then an `.fxb` bank holding the chunk.

use super::FormatError;

pub const MAGIC: &[u8; 4] = b"VST3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VstPreset {
    pub version: i32,
    /// The class ID's 32 hex digits, as written.
    pub class_id: String,
    /// Each chunk's ID and bytes, in the list's order.
    pub chunks: Vec<(String, Vec<u8>)>,
}

impl VstPreset {
    pub fn read(bytes: &[u8]) -> Result<VstPreset, FormatError> {
        let fail = |what: &str| FormatError::new(format!("VST3 preset: {what}"));
        if bytes.len() < 48 || &bytes[..4] != MAGIC {
            return Err(fail("no VST3 header"));
        }
        let version = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let class_id = std::str::from_utf8(&bytes[8..40]).map_err(|_| fail("a class ID that isn't text"))?.to_string();
        let list = usize::try_from(i64::from_le_bytes(bytes[40..48].try_into().unwrap())).map_err(|_| fail("a negative list offset"))?;
        if bytes.get(list..list + 4) != Some(b"List".as_slice()) {
            return Err(fail("no chunk list where the header points"));
        }
        let count =
            bytes.get(list + 4..list + 8).map(|b| i32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| fail("a cut-off chunk list"))?;
        let count =
            usize::try_from(count).ok().filter(|n| list + 8 + n * 20 <= bytes.len()).ok_or_else(|| fail("more chunks than bytes"))?;
        let mut chunks = Vec::with_capacity(count);
        for i in 0..count {
            let entry = &bytes[list + 8 + i * 20..list + 8 + (i + 1) * 20];
            let id = String::from_utf8_lossy(&entry[..4]).into_owned();
            let offset = i64::from_le_bytes(entry[4..12].try_into().unwrap());
            let size = i64::from_le_bytes(entry[12..20].try_into().unwrap());
            let range = usize::try_from(offset)
                .ok()
                .zip(usize::try_from(size).ok())
                .and_then(|(offset, size)| Some(offset..offset.checked_add(size)?))
                .filter(|range| range.end <= list)
                .ok_or_else(|| fail(&format!("chunk {id} runs outside the data")))?;
            chunks.push((id, bytes[range].to_vec()));
        }
        Ok(VstPreset { version, class_id, chunks })
    }

    /// The file's bytes: the chunks' data in order, then the list.
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        let mut id = self.class_id.as_bytes().to_vec();
        id.resize(32, b'0');
        out.extend_from_slice(&id);
        out.extend_from_slice(&[0; 8]);
        let mut entries = Vec::new();
        for (chunk, data) in &self.chunks {
            entries.push((chunk, out.len() as i64, data.len() as i64));
            out.extend_from_slice(data);
        }
        let list = out.len() as i64;
        out[40..48].copy_from_slice(&list.to_le_bytes());
        out.extend_from_slice(b"List");
        out.extend_from_slice(&(entries.len() as i32).to_le_bytes());
        for (chunk, offset, size) in entries {
            let mut id = chunk.as_bytes().to_vec();
            id.resize(4, b' ');
            out.extend_from_slice(&id[..4]);
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        out
    }

    pub fn chunk(&self, id: &str) -> Option<&[u8]> {
        self.chunks.iter().find(|(chunk, _)| chunk == id).map(|(_, data)| data.as_slice())
    }

    /// The processor's state.
    pub fn component(&self) -> Option<&[u8]> {
        self.chunk("Comp")
    }

    /// The controller's state.
    pub fn controller(&self) -> Option<&[u8]> {
        self.chunk("Cont")
    }
}

/// A VST2 chunk inside a VST3 state: the plug-in's VST2 ID and version, and the chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst2Chunk<'a> {
    /// The four-character VST2 ID ("Vita").
    pub plugin: [u8; 4],
    pub plugin_version: u32,
    /// "FBCh" (a bank) or "FPCh" (one program).
    pub kind: [u8; 4],
    pub data: &'a [u8],
}

/// The VST2 chunk in a processor state wrapped with "VstW", or None when the state isn't wrapped.
pub fn vst2_chunk(state: &[u8]) -> Result<Option<Vst2Chunk<'_>>, FormatError> {
    if !state.starts_with(b"VstW") {
        return Ok(None);
    }
    let fail = |what: &str| FormatError::new(format!("VST2 chunk in a VST3 state: {what}"));
    let be = |at: usize| state.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap())).ok_or_else(|| fail("cut off"));
    // "VstW", then a size counting the version and bypass flag that follow it.
    let bank = 8 + be(4)? as usize;
    if state.get(bank..bank + 4) != Some(b"CcnK".as_slice()) {
        return Err(fail("no fxb bank after the VstW header"));
    }
    let kind: [u8; 4] = state[bank + 8..bank + 12].try_into().unwrap();
    let plugin: [u8; 4] = state.get(bank + 16..bank + 20).ok_or_else(|| fail("cut off"))?.try_into().unwrap();
    let plugin_version = be(bank + 20)?;
    // A bank's header: magic, size, kind, version, ID, version, program count, current program, 124 reserved
    // bytes, then the chunk's size; a program's has a 28-byte name where a bank keeps its current program.
    let size_at = match &kind {
        b"FBCh" => bank + 156,
        b"FPCh" => bank + 56,
        _ => return Err(fail("neither a bank nor a program chunk")),
    };
    let size = be(size_at)? as usize;
    let data = state.get(size_at + 4..size_at + 4 + size).ok_or_else(|| fail("the chunk runs past the state"))?;
    Ok(Some(Vst2Chunk { plugin, plugin_version, kind, data }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_the_same_chunks() {
        let preset = VstPreset {
            version: 1,
            class_id: "56534558667350736572756D20320000".into(),
            chunks: vec![("Comp".into(), b"processor".to_vec()), ("Cont".into(), Vec::new()), ("Info".into(), b"<MetaInfo/>".to_vec())],
        };
        let bytes = preset.write();
        assert_eq!(VstPreset::read(&bytes).unwrap(), preset);
        assert_eq!(VstPreset::read(&bytes).unwrap().component(), Some(b"processor".as_slice()));
        assert!(VstPreset::read(&bytes[..bytes.len() - 1]).is_err());
        assert!(VstPreset::read(b"VST2").is_err());
    }

    #[test]
    fn unwraps_a_vst2_bank_chunk() {
        let chunk = b"{\"settings\":{}}\0";
        let mut state = b"VstW".to_vec();
        for word in [8u32, 1, 0] {
            state.extend_from_slice(&word.to_be_bytes());
        }
        state.extend_from_slice(b"CcnK");
        state.extend_from_slice(&((152 + chunk.len()) as u32).to_be_bytes());
        state.extend_from_slice(b"FBCh");
        state.extend_from_slice(&2u32.to_be_bytes());
        state.extend_from_slice(b"Vita");
        state.extend_from_slice(&0x0001_0007u32.to_be_bytes());
        state.extend_from_slice(&[0; 132]);
        state.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
        state.extend_from_slice(chunk);
        let found = vst2_chunk(&state).unwrap().unwrap();
        assert_eq!((&found.plugin, found.plugin_version, &found.kind, found.data), (b"Vita", 0x0001_0007, b"FBCh", chunk.as_slice()));
        assert_eq!(vst2_chunk(b"XferJson").unwrap(), None);
        assert!(vst2_chunk(&state[..state.len() - 1]).is_err());
    }
}
