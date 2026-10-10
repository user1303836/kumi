//! A frozen Max for Live device. Max keeps the device's patcher and every file it uses (abstractions, JavaScript,
//! images, samples) in one container in the device file's "ptch" chunk:
//!
//! - "mx@c", then three big-endian numbers: the header's size (16), 0, and where the directory starts;
//! - the files, one after another;
//! - the directory: "dlst" and its size, then a "dire" entry for each file, made of fields (a tag, the field's size
//!   with its header, the value): type ("JSON", "TEXT", "WAVE"…), fnam (the name, NUL-padded), sz32 (its size), of32
//!   (where it starts, from the container's start) and flag (1 marks the device's own patcher).
//!
//! Max stores a file once for each place that uses it; the first entry of a name is the one read.

use serde_json::Value;

use super::Files;
use crate::devices::amxd::patcher_chunk;

/// A file in a frozen device.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrozenFile {
    pub name: String,
    /// Max's type for it: "JSON" for a patcher, "TEXT" for code, "WAVE" for audio…
    pub kind: String,
    pub size: usize,
    start: usize,
    flags: u32,
}

impl FrozenFile {
    /// Whether this is the device's own patcher.
    pub fn main(&self) -> bool {
        self.flags & 1 == 1
    }
}

#[derive(Debug, Clone, Default)]
pub struct Frozen {
    container: Vec<u8>,
    pub files: Vec<FrozenFile>,
}

/// A device file read whole: its four-letter type ("aaaa", "iiii", "mmmm"…), its patcher, and its frozen files, if it
/// was frozen.
#[derive(Debug, Clone)]
pub struct DeviceFile {
    pub code: String,
    pub patcher: Value,
    pub frozen: Option<Frozen>,
}

/// A device file's patcher, from its JSON or from the files frozen into it.
pub fn read_device(bytes: &[u8]) -> Option<DeviceFile> {
    let (code, chunk) = patcher_chunk(bytes)?;
    let code = String::from_utf8_lossy(code).into_owned();
    match Frozen::read(chunk) {
        Some(frozen) => {
            let main = frozen.files.iter().find(|file| file.main()).or(frozen.files.iter().find(|file| file.kind == "JSON"))?;
            let patcher = parse(frozen.bytes(main)?)?;
            Some(DeviceFile { code, patcher, frozen: Some(frozen) })
        }
        None => Some(DeviceFile { code, patcher: parse(chunk)?, frozen: None }),
    }
}

/// JSON as Max saves it, a NUL at its end.
fn parse(bytes: &[u8]) -> Option<Value> {
    let end = bytes.iter().rposition(|byte| *byte != 0).map_or(0, |at| at + 1);
    serde_json::from_slice(&bytes[..end]).ok()
}

impl Frozen {
    /// The files of a frozen device's container; None when the chunk isn't one.
    pub fn read(chunk: &[u8]) -> Option<Frozen> {
        let number = |at: usize| -> Option<usize> { Some(u32::from_be_bytes(chunk.get(at..at + 4)?.try_into().ok()?) as usize) };
        if chunk.get(0..4)? != b"mx@c" {
            return None;
        }
        let directory = number(12)?;
        if chunk.get(directory..directory + 4)? != b"dlst" {
            return None;
        }
        let end = directory.saturating_add(number(directory + 4)?).min(chunk.len());
        let mut files = Vec::new();
        let mut at = directory + 8;
        while at + 8 <= end && chunk.get(at..at + 4) == Some(b"dire") {
            let size = number(at + 4)?;
            if size < 8 {
                break;
            }
            let mut file = FrozenFile::default();
            let mut field = at + 8;
            while field + 8 <= (at + size).min(end) {
                let length = number(field + 4)?;
                if length < 8 {
                    break;
                }
                let value = chunk.get(field + 8..field + length)?;
                let text = || String::from_utf8_lossy(value.split(|byte| *byte == 0).next().unwrap_or_default()).into_owned();
                let whole = || value.get(0..4).and_then(|bytes| bytes.try_into().ok()).map(u32::from_be_bytes).unwrap_or(0);
                match &chunk[field..field + 4] {
                    b"type" => file.kind = text(),
                    b"fnam" => file.name = text(),
                    b"sz32" => file.size = whole() as usize,
                    b"of32" => file.start = whole() as usize,
                    b"flag" => file.flags = whole(),
                    _ => {}
                }
                field += length;
            }
            files.push(file);
            at += size;
        }
        Some(Frozen { container: chunk.to_vec(), files })
    }

    /// A file's bytes, as they lie in the container.
    pub fn bytes(&self, file: &FrozenFile) -> Option<&[u8]> {
        self.container.get(file.start..file.start.checked_add(file.size)?)
    }

    /// The first file of a name.
    pub fn file(&self, name: &str) -> Option<&FrozenFile> {
        self.files.iter().find(|file| file.name == name)
    }
}

impl Files for Frozen {
    fn patcher(&self, name: &str) -> Option<Value> {
        let file = self.file(name).filter(|file| file.kind == "JSON")?;
        parse(self.bytes(file)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::amxd::{encode_amxd, DeviceType};
    use serde_json::json;

    /// A frozen device as Max lays one out: the container's header, the files, then the directory.
    fn frozen_device(files: &[(&str, &str, u32, &[u8])]) -> Vec<u8> {
        let mut container = b"mx@c".to_vec();
        container.extend_from_slice(&16u32.to_be_bytes());
        container.extend_from_slice(&0u32.to_be_bytes());
        container.extend_from_slice(&0u32.to_be_bytes());
        let mut entries = Vec::new();
        for (kind, name, flags, bytes) in files {
            let start = container.len() as u32;
            container.extend_from_slice(bytes);
            let field = |tag: &[u8], value: &[u8]| {
                let mut out = tag.to_vec();
                out.extend_from_slice(&(8 + value.len() as u32).to_be_bytes());
                out.extend_from_slice(value);
                out
            };
            let mut padded = name.as_bytes().to_vec();
            padded.resize(name.len().div_ceil(4) * 4 + 4, 0);
            let mut body = field(b"type", kind.as_bytes());
            body.extend(field(b"fnam", &padded));
            body.extend(field(b"sz32", &(bytes.len() as u32).to_be_bytes()));
            body.extend(field(b"of32", &start.to_be_bytes()));
            body.extend(field(b"flag", &flags.to_be_bytes()));
            let mut entry = b"dire".to_vec();
            entry.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
            entry.extend(body);
            entries.extend(entry);
        }
        let directory = container.len() as u32;
        container[12..16].copy_from_slice(&directory.to_be_bytes());
        container.extend_from_slice(b"dlst");
        container.extend_from_slice(&(8 + entries.len() as u32).to_be_bytes());
        container.extend(entries);
        let mut bytes = b"ampf\x04\x00\x00\x00aaaameta\x04\x00\x00\x00\x07\x00\x00\x00ptch".to_vec();
        bytes.extend_from_slice(&(container.len() as u32).to_le_bytes());
        bytes.extend(container);
        bytes
    }

    #[test]
    fn a_frozen_device_gives_its_patcher_and_the_files_it_names() {
        let main =
            br#"{ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "bpatcher", "name": "dial.maxpat" } }], "lines": [] } }"#;
        let dial = br#"{ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "jsui" } }], "lines": [] } }"#;
        let mut main = main.to_vec();
        main.push(0);
        let bytes =
            frozen_device(&[("JSON", "Echo.amxd", 0x11, &main), ("JSON", "dial.maxpat", 0, dial), ("TEXT", "dial.js", 0, b"mgraphics")]);
        let device = read_device(&bytes).expect("a frozen device");
        assert_eq!(device.code, "aaaa");
        let frozen = device.frozen.as_ref().expect("its files");
        assert_eq!(frozen.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(), ["Echo.amxd", "dial.maxpat", "dial.js"]);
        assert_eq!(frozen.bytes(frozen.file("dial.js").unwrap()), Some(&b"mgraphics"[..]));
        let patcher = super::super::Patcher::read(&device.patcher, frozen);
        assert_eq!(patcher.boxes[0].inner().map(|inner| inner.boxes[0].maxclass()), Some("jsui"));
        // A device that isn't frozen is its JSON.
        let plain = read_device(&encode_amxd(DeviceType::MidiEffect, &json!({ "patcher": { "title": "x" } }))).unwrap();
        assert_eq!((plain.code.as_str(), plain.patcher["patcher"]["title"].as_str(), plain.frozen.is_none()), ("mmmm", Some("x"), true));
    }
}
