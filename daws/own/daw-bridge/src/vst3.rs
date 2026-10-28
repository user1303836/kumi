//! The standard VST3 preset container used by the own-bridge DAWs. Vendor component data remains opaque.

use std::collections::HashSet;

pub const MAX_PRESET_BYTES: usize = 32 << 20;

#[derive(Debug)]
pub struct Preset<'a> {
    pub class_id: String,
    pub component: &'a [u8],
    pub controller: Option<&'a [u8]>,
}

/// Validate all offsets before handing vendor state to a DAW. A preset is a 48-byte header followed by chunks and a
/// `List` directory; no chunk may overlap the header, directory, or another chunk.
pub fn read(bytes: &[u8]) -> Result<Preset<'_>, String> {
    let bad = || "not a valid VST3 preset container".to_string();
    if bytes.len() < 48 || bytes.len() > MAX_PRESET_BYTES || &bytes[..4] != b"VST3" || bytes[4..8] != 1u32.to_le_bytes() {
        return Err(bad());
    }
    let id = &bytes[8..40];
    if !id.iter().all(u8::is_ascii_hexdigit) {
        return Err("the VST3 preset has an invalid class ID".into());
    }
    let number = |at: usize| -> Result<usize, String> {
        let value = u64::from_le_bytes(bytes.get(at..at + 8).ok_or_else(bad)?.try_into().map_err(|_| bad())?);
        usize::try_from(value).map_err(|_| bad())
    };
    let directory = number(40)?;
    let header_end = directory.checked_add(8).filter(|end| *end <= bytes.len()).ok_or_else(bad)?;
    if directory < 48 || &bytes[directory..directory + 4] != b"List" {
        return Err(bad());
    }
    let count = u32::from_le_bytes(bytes[directory + 4..header_end].try_into().map_err(|_| bad())?) as usize;
    if count == 0 || count > 128 {
        return Err("the VST3 preset has an invalid chunk count".into());
    }
    let directory_end = header_end.checked_add(count * 20).filter(|end| *end <= bytes.len()).ok_or_else(bad)?;
    let mut ids = HashSet::new();
    let mut ranges = vec![(0, 48), (directory, directory_end)];
    let mut component = None;
    let mut controller = None;
    for index in 0..count {
        let entry = header_end + index * 20;
        let id = &bytes[entry..entry + 4];
        if !id.iter().all(u8::is_ascii_graphic) || !ids.insert(id) {
            return Err("the VST3 preset has duplicate or invalid chunk IDs".into());
        }
        let start = number(entry + 4)?;
        let end = start.checked_add(number(entry + 12)?).filter(|end| *end <= bytes.len()).ok_or_else(bad)?;
        if start < 48 {
            return Err(bad());
        }
        if ranges.iter().any(|&(a, b)| start < b && a < end) {
            return Err("the VST3 preset has overlapping chunks".into());
        }
        ranges.push((start, end));
        match id {
            b"Comp" => component = Some(&bytes[start..end]),
            b"Cont" => controller = Some(&bytes[start..end]),
            _ => (),
        }
    }
    Ok(Preset {
        class_id: std::str::from_utf8(&bytes[8..40]).map_err(|_| bad())?.to_ascii_uppercase(),
        component: component.filter(|c| !c.is_empty()).ok_or("the VST3 preset has no component state")?,
        controller,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset() -> Vec<u8> {
        let mut bytes = b"VST3".to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(b"00112233445566778899AABBCCDDEEFF");
        bytes.extend(51u64.to_le_bytes());
        bytes.extend(b"abcList");
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(b"Comp");
        bytes.extend(48u64.to_le_bytes());
        bytes.extend(3u64.to_le_bytes());
        bytes
    }

    #[test]
    fn checks_container_boundaries_and_identity() {
        let bytes = preset();
        let parsed = read(&bytes).unwrap();
        assert_eq!(parsed.component, b"abc");
        assert_eq!(parsed.class_id, "00112233445566778899AABBCCDDEEFF");
        for len in 0..bytes.len() {
            assert!(read(&bytes[..len]).is_err());
        }
        let mut invalid = bytes.clone();
        invalid[63..71].copy_from_slice(&0u64.to_le_bytes());
        assert!(read(&invalid).is_err());
        let mut invalid = bytes.clone();
        invalid[71..79].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(read(&invalid).is_err());
    }
}
