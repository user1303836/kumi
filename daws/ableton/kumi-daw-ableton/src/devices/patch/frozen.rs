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
            let patcher = parse(frozen.bytes(frozen.main_file()?)?)?;
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

/// A big-endian number in the container.
fn number(chunk: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(chunk.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
}

/// A field of a directory entry: its tag and where its value lies in the container.
struct Field {
    tag: [u8; 4],
    value: std::ops::Range<usize>,
}

/// Where the container's directory starts, and each entry's fields; None when the chunk isn't a container.
fn directory(chunk: &[u8]) -> Option<(usize, Vec<Vec<Field>>)> {
    if chunk.get(0..4)? != b"mx@c" {
        return None;
    }
    let directory = number(chunk, 12)?;
    if chunk.get(directory..directory.checked_add(4)?)? != b"dlst" {
        return None;
    }
    let end = directory.saturating_add(number(chunk, directory + 4)?).min(chunk.len());
    let mut entries = Vec::new();
    let mut at = directory + 8;
    while at + 8 <= end && chunk.get(at..at + 4) == Some(b"dire") {
        let size = number(chunk, at + 4)?;
        if size < 8 {
            break;
        }
        let mut fields = Vec::new();
        let mut field = at + 8;
        while field + 8 <= at.saturating_add(size).min(end) {
            let length = number(chunk, field + 4)?;
            if length < 8 {
                break;
            }
            chunk.get(field + 8..field + length)?;
            fields.push(Field { tag: chunk[field..field + 4].try_into().ok()?, value: field + 8..field + length });
            field += length;
        }
        entries.push(fields);
        at += size;
    }
    Some((directory, entries))
}

impl Frozen {
    /// The files of a frozen device's container; None when the chunk isn't one.
    pub fn read(chunk: &[u8]) -> Option<Frozen> {
        let (_, entries) = directory(chunk)?;
        let files = entries
            .iter()
            .map(|fields| {
                let mut file = FrozenFile::default();
                for field in fields {
                    let value = &chunk[field.value.clone()];
                    let text = || String::from_utf8_lossy(value.split(|byte| *byte == 0).next().unwrap_or_default()).into_owned();
                    let whole = || value.get(0..4).and_then(|bytes| bytes.try_into().ok()).map(u32::from_be_bytes).unwrap_or(0);
                    match &field.tag {
                        b"type" => file.kind = text(),
                        b"fnam" => file.name = text(),
                        b"sz32" => file.size = whole() as usize,
                        b"of32" => file.start = whole() as usize,
                        b"flag" => file.flags = whole(),
                        _ => {}
                    }
                }
                file
            })
            .collect();
        Some(Frozen { container: chunk.to_vec(), files })
    }

    /// The container with `bytes` in place of `replaced`'s, and every other file, and every other field of the
    /// directory, as they were: each file's place moves by what the replaced one grew or shrank. None when two files
    /// overlap or one lies outside the files' part, which Kumi can't rewrite safely.
    pub fn with_file(&self, replaced: &FrozenFile, bytes: &[u8]) -> Option<Vec<u8>> {
        let chunk = &self.container;
        let (listed, entries) = directory(chunk)?;
        // Each file's place once, however many entries name it, in the order they lie.
        let mut places: Vec<(usize, usize)> = self.files.iter().map(|file| (file.start, file.size)).collect();
        places.sort_unstable();
        places.dedup();
        let mut out = chunk.get(0..16)?.to_vec();
        let mut moved: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        let mut cursor = 16;
        for (start, size) in places {
            let end = start.checked_add(size)?;
            if start < cursor || end > listed {
                return None;
            }
            out.extend_from_slice(&chunk[cursor..start]);
            moved.insert(start, out.len());
            if (start, size) == (replaced.start, replaced.size) {
                out.extend_from_slice(bytes);
            } else {
                out.extend_from_slice(&chunk[start..end]);
            }
            cursor = end;
        }
        out.extend_from_slice(&chunk[cursor..listed]);
        let shift = |at: usize| -> Option<[u8; 4]> { Some(u32::try_from(at).ok()?.to_be_bytes()) };
        let directory_at = out.len();
        out[12..16].copy_from_slice(&shift(directory_at)?);
        out.extend_from_slice(&chunk[listed..]);
        for fields in &entries {
            let whole = |tag: &[u8; 4]| fields.iter().find(|field| &field.tag == tag).and_then(|field| number(chunk, field.value.start));
            let (start, size) = (whole(b"of32").unwrap_or(0), whole(b"sz32").unwrap_or(0));
            for field in fields.iter().filter(|field| field.value.len() >= 4) {
                let at = field.value.start - listed + directory_at;
                match &field.tag {
                    b"of32" => {
                        if let Some(new) = moved.get(&start) {
                            out[at..at + 4].copy_from_slice(&shift(*new)?);
                        }
                    }
                    b"sz32" if (start, size) == (replaced.start, replaced.size) => out[at..at + 4].copy_from_slice(&shift(bytes.len())?),
                    _ => {}
                }
            }
        }
        Some(out)
    }

    /// A file's bytes, as they lie in the container.
    pub fn bytes(&self, file: &FrozenFile) -> Option<&[u8]> {
        self.container.get(file.start..file.start.checked_add(file.size)?)
    }

    /// The first file of a name.
    pub fn file(&self, name: &str) -> Option<&FrozenFile> {
        self.files.iter().find(|file| file.name == name)
    }

    /// The device's own patcher: the file flagged as it, else the first patcher.
    pub fn main_file(&self) -> Option<&FrozenFile> {
        self.files.iter().find(|file| file.main()).or(self.files.iter().find(|file| file.kind == "JSON"))
    }
}

impl Files for Frozen {
    fn patcher(&self, name: &str) -> Option<Value> {
        let file = self.file(name).filter(|file| file.kind == "JSON")?;
        parse(self.bytes(file)?)
    }

    fn text(&self, name: &str) -> Option<String> {
        let bytes = self.bytes(self.file(name)?)?;
        let end = bytes.iter().rposition(|byte| *byte != 0).map_or(0, |at| at + 1);
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::devices::amxd::{encode_amxd, DeviceType};
    use serde_json::json;

    /// A frozen device as Max lays one out: the container's header, the files, then the directory.
    pub(crate) fn frozen_device(files: &[(&str, &str, u32, &[u8])]) -> Vec<u8> {
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

    #[test]
    fn a_frozen_devices_patcher_is_replaced_and_its_other_files_kept_as_they_were() {
        let main = br#"{ "patcher": { "title": "Echo", "boxes": [], "lines": [] } }"#;
        let dial = br#"{ "patcher": { "boxes": [{ "box": { "id": "obj-1", "maxclass": "jsui" } }], "lines": [] } }"#;
        let bytes = frozen_device(&[
            ("JSON", "Echo.amxd", 0x11, main),
            ("JSON", "dial.maxpat", 0, dial),
            ("TEXT", "dial.js", 0, b"mgraphics"),
            ("WAVE", "click.wav", 0, b"RIFF....WAVE"),
        ]);
        let device = read_device(&bytes).unwrap();
        let frozen = device.frozen.unwrap();
        let longer = br#"{ "patcher": { "title": "Echo Sync", "boxes": [{ "box": { "id": "obj-1", "maxclass": "newobj", "text": "metro 16n" } }], "lines": [] } }"#;
        let container = frozen.with_file(frozen.main_file().unwrap(), longer).unwrap();
        let again = Frozen::read(&container).unwrap();
        assert_eq!(again.files.len(), 4);
        assert_eq!(again.bytes(again.main_file().unwrap()), Some(&longer[..]));
        for (name, was) in [("dial.maxpat", &dial[..]), ("dial.js", &b"mgraphics"[..]), ("click.wav", &b"RIFF....WAVE"[..])] {
            assert_eq!(again.bytes(again.file(name).unwrap()), Some(was), "{name} as it was");
        }
        let flags: Vec<u32> = again.files.iter().map(|file| file.flags).collect();
        assert_eq!(flags, [0x11, 0, 0, 0], "every other field of the directory as it was");
        // Shorter works as well; files that overlap aren't rewritten.
        let shorter = frozen.with_file(frozen.main_file().unwrap(), b"{}").unwrap();
        assert_eq!(
            Frozen::read(&shorter).unwrap().bytes(Frozen::read(&shorter).unwrap().file("dial.js").unwrap()),
            Some(&b"mgraphics"[..])
        );
        let mut overlapping = frozen.clone();
        overlapping.files[2].start = overlapping.files[1].start + 1;
        assert_eq!(overlapping.with_file(overlapping.main_file().unwrap(), longer), None);
    }
}
