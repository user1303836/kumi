//! Just enough of the zip format for a Renoise instrument file (`.xrni`): read its entries, take one out, and write the
//! archive again with one entry's contents replaced and every other entry copied byte for byte (the sample data is not
//! decoded or packed again). Zip64 archives, encrypted entries and archives split over disks are refused.
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::io::{Read, Write};

/// The most an entry may hold once unpacked (Renoise's Instrument.xml is kilobytes, or a few megabytes with plug-in state).
pub const MAX_UNPACKED: usize = 64 * 1024 * 1024;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const DESCRIPTOR: u32 = 0x0807_4b50;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
/// The flags bit that says the sizes and CRC follow the data, and the one that says the name is UTF-8.
const HAS_DESCRIPTOR: u16 = 0x0008;
const UTF8: u16 = 0x0800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipError(pub String);

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ZipError {}

fn bad(message: impl Into<String>) -> ZipError {
    ZipError(message.into())
}

/// One entry, as the central directory says, with where its bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc: u32,
    pub packed: u64,
    pub unpacked: u64,
    /// The central directory's header for it, as written.
    central: Vec<u8>,
    /// Where its local header starts, and where its record ends (data and any descriptor included).
    start: usize,
    end: usize,
    /// Where its data starts.
    data: usize,
    time: u16,
    date: u16,
}

/// An archive read from its bytes.
#[derive(Debug)]
pub struct Archive<'a> {
    bytes: &'a [u8],
    pub entries: Vec<Entry>,
    /// The comment after the end record, as written.
    comment: Vec<u8>,
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, ZipError> {
    bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| bad("the archive ends early"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ZipError> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| bad("the archive ends early"))
}

impl<'a> Archive<'a> {
    pub fn read(bytes: &'a [u8]) -> Result<Self, ZipError> {
        // The end record: the last one in the final 64 KiB (its comment may be up to 65,535 bytes).
        let floor = bytes.len().saturating_sub(22 + 65_535);
        let mut at = bytes.len().checked_sub(22).ok_or_else(|| bad("not a zip archive (too short)"))?;
        loop {
            if u32_at(bytes, at)? == END {
                break;
            }
            if at == floor {
                return Err(bad("not a zip archive (no end record)"));
            }
            at -= 1;
        }
        let end = at;
        if u16_at(bytes, end + 4)? != 0 || u16_at(bytes, end + 6)? != 0 {
            return Err(bad("an archive split over disks isn't read"));
        }
        let count = u16_at(bytes, end + 10)? as usize;
        let size = u32_at(bytes, end + 12)? as usize;
        let offset = u32_at(bytes, end + 16)? as usize;
        let comment_len = u16_at(bytes, end + 20)? as usize;
        if count == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
            return Err(bad("a zip64 archive isn't read"));
        }
        let comment = bytes.get(end + 22..end + 22 + comment_len).ok_or_else(|| bad("the archive's comment is cut"))?.to_vec();
        if offset.checked_add(size).is_none_or(|stop| stop > end) {
            return Err(bad("the central directory is outside the archive"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = offset;
        for _ in 0..count {
            if u32_at(bytes, at)? != CENTRAL {
                return Err(bad("a central directory entry is broken"));
            }
            let flags = u16_at(bytes, at + 8)?;
            let method = u16_at(bytes, at + 10)?;
            let time = u16_at(bytes, at + 12)?;
            let date = u16_at(bytes, at + 14)?;
            let crc = u32_at(bytes, at + 16)?;
            let packed = u32_at(bytes, at + 20)?;
            let unpacked = u32_at(bytes, at + 24)?;
            let name_len = u16_at(bytes, at + 28)? as usize;
            let extra_len = u16_at(bytes, at + 30)? as usize;
            let comment_len = u16_at(bytes, at + 32)? as usize;
            let local = u32_at(bytes, at + 42)? as usize;
            if packed == 0xFFFF_FFFF || unpacked == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
                return Err(bad("a zip64 entry isn't read"));
            }
            if flags & 0x0001 != 0 {
                return Err(bad("an encrypted entry isn't read"));
            }
            let header_end = at + 46 + name_len + extra_len + comment_len;
            let name_bytes = bytes.get(at + 46..at + 46 + name_len).ok_or_else(|| bad("an entry's name is cut"))?;
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            let central = bytes.get(at..header_end).ok_or_else(|| bad("a central directory entry is cut"))?.to_vec();
            // The local header says how long its own name and extra field are.
            if u32_at(bytes, local)? != LOCAL {
                return Err(bad(format!("{name}: its local header is broken")));
            }
            let data = local + 30 + u16_at(bytes, local + 26)? as usize + u16_at(bytes, local + 28)? as usize;
            let mut stop = data.checked_add(packed as usize).ok_or_else(|| bad("an entry's size is broken"))?;
            if flags & HAS_DESCRIPTOR != 0 {
                // (a descriptor may start with its signature or not; with it, 16 bytes, else 12)
                stop += if u32_at(bytes, stop)? == DESCRIPTOR { 16 } else { 12 };
            }
            if stop > offset {
                return Err(bad(format!("{name}: its data runs into the central directory")));
            }
            entries.push(Entry {
                name,
                method,
                flags,
                crc,
                packed: packed as u64,
                unpacked: unpacked as u64,
                central,
                start: local,
                end: stop,
                data,
                time,
                date,
            });
            at = header_end;
        }
        Ok(Archive { bytes, entries, comment })
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// An entry's contents, unpacked and checked against its CRC.
    pub fn contents(&self, entry: &Entry) -> Result<Vec<u8>, ZipError> {
        if entry.unpacked as usize > MAX_UNPACKED {
            return Err(bad(format!("{} holds more than {} MiB", entry.name, MAX_UNPACKED / 1024 / 1024)));
        }
        let packed = self.bytes.get(entry.data..entry.data + entry.packed as usize).ok_or_else(|| bad("an entry's data is cut"))?;
        let out = match entry.method {
            STORED => packed.to_vec(),
            DEFLATED => {
                let mut out = Vec::with_capacity(entry.unpacked as usize);
                DeflateDecoder::new(packed)
                    .take(MAX_UNPACKED as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| bad(format!("{}: {e}", entry.name)))?;
                out
            }
            other => return Err(bad(format!("{}: packed in a way this doesn't read (method {other})", entry.name))),
        };
        if out.len() as u64 != entry.unpacked || crc(&out) != entry.crc {
            return Err(bad(format!("{}: its contents don't match their CRC", entry.name)));
        }
        Ok(out)
    }

    /// The archive again with `name`'s contents made `contents` (deflated anew, in its place), every other entry copied
    /// as it was.
    pub fn with_replaced(&self, name: &str, contents: &[u8]) -> Result<Vec<u8>, ZipError> {
        if !self.entries.iter().any(|entry| entry.name == name) {
            return Err(bad(format!("the archive has no {name}")));
        }
        if contents.len() > MAX_UNPACKED {
            return Err(bad(format!("{name} would hold more than {} MiB", MAX_UNPACKED / 1024 / 1024)));
        }
        let mut out: Vec<u8> = Vec::with_capacity(self.bytes.len() + contents.len());
        let mut central: Vec<u8> = vec![];
        for entry in &self.entries {
            let offset = u32::try_from(out.len()).map_err(|_| bad("the archive would need zip64"))?;
            if entry.name == name {
                let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(contents).map_err(|e| bad(e.to_string()))?;
                let packed = encoder.finish().map_err(|e| bad(e.to_string()))?;
                let sum = crc(contents);
                let flags = entry.flags & UTF8;
                let name_bytes = entry.name.as_bytes();
                let packed_len = u32::try_from(packed.len()).map_err(|_| bad("an entry too big for a zip"))?;
                let unpacked_len = contents.len() as u32;
                let mut local = vec![];
                local.extend(LOCAL.to_le_bytes());
                local.extend(20u16.to_le_bytes());
                local.extend(flags.to_le_bytes());
                local.extend(DEFLATED.to_le_bytes());
                local.extend(entry.time.to_le_bytes());
                local.extend(entry.date.to_le_bytes());
                local.extend(sum.to_le_bytes());
                local.extend(packed_len.to_le_bytes());
                local.extend(unpacked_len.to_le_bytes());
                local.extend((name_bytes.len() as u16).to_le_bytes());
                local.extend(0u16.to_le_bytes());
                local.extend(name_bytes);
                out.extend(local);
                out.extend(&packed);
                // Its central header: the one read, with the new method, CRC, sizes and place (and no extra field,
                // which may have held the old ones).
                let mut header = entry.central[..46].to_vec();
                header[6..8].copy_from_slice(&20u16.to_le_bytes());
                header[8..10].copy_from_slice(&flags.to_le_bytes());
                header[10..12].copy_from_slice(&DEFLATED.to_le_bytes());
                header[16..20].copy_from_slice(&sum.to_le_bytes());
                header[20..24].copy_from_slice(&packed_len.to_le_bytes());
                header[24..28].copy_from_slice(&unpacked_len.to_le_bytes());
                header[30..32].copy_from_slice(&0u16.to_le_bytes());
                header[32..34].copy_from_slice(&0u16.to_le_bytes());
                header[42..46].copy_from_slice(&offset.to_le_bytes());
                header.extend(name_bytes);
                central.extend(header);
            } else {
                out.extend(&self.bytes[entry.start..entry.end]);
                let mut header = entry.central.clone();
                header[42..46].copy_from_slice(&offset.to_le_bytes());
                central.extend(header);
            }
        }
        let central_at = u32::try_from(out.len()).map_err(|_| bad("the archive would need zip64"))?;
        let central_len = central.len() as u32;
        out.extend(central);
        out.extend(END.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend((self.entries.len() as u16).to_le_bytes());
        out.extend((self.entries.len() as u16).to_le_bytes());
        out.extend(central_len.to_le_bytes());
        out.extend(central_at.to_le_bytes());
        out.extend((self.comment.len() as u16).to_le_bytes());
        out.extend(&self.comment);
        Ok(out)
    }
}

fn crc(bytes: &[u8]) -> u32 {
    let mut sum = flate2::Crc::new();
    sum.update(bytes);
    sum.sum()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small archive written the way common zip writers write one: a deflated XML and a stored blob, the second with
    /// a data descriptor after it (sizes and CRC in the descriptor, zeros in its local header).
    pub(crate) fn sample(xml: &[u8], blob: &[u8]) -> Vec<u8> {
        let mut out = vec![];
        let mut central = vec![];
        let mut add = |name: &str, contents: &[u8], deflate: bool, descriptor: bool, out: &mut Vec<u8>| {
            let offset = out.len() as u32;
            let packed = if deflate {
                let mut e = DeflateEncoder::new(Vec::new(), flate2::Compression::best());
                e.write_all(contents).unwrap();
                e.finish().unwrap()
            } else {
                contents.to_vec()
            };
            let method: u16 = if deflate { DEFLATED } else { STORED };
            let flags: u16 = if descriptor { HAS_DESCRIPTOR } else { 0 };
            let sum = crc(contents);
            let (c, p, u) = if descriptor { (0, 0, 0) } else { (sum, packed.len() as u32, contents.len() as u32) };
            out.extend(LOCAL.to_le_bytes());
            out.extend(20u16.to_le_bytes());
            out.extend(flags.to_le_bytes());
            out.extend(method.to_le_bytes());
            out.extend(0x6000u16.to_le_bytes());
            out.extend(0x5a43u16.to_le_bytes());
            out.extend(c.to_le_bytes());
            out.extend(p.to_le_bytes());
            out.extend(u.to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(4u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend([0xca, 0xfe, 0, 0]);
            out.extend(&packed);
            if descriptor {
                out.extend(DESCRIPTOR.to_le_bytes());
                out.extend(sum.to_le_bytes());
                out.extend((packed.len() as u32).to_le_bytes());
                out.extend((contents.len() as u32).to_le_bytes());
            }
            central.extend(CENTRAL.to_le_bytes());
            central.extend(0x031eu16.to_le_bytes());
            central.extend(20u16.to_le_bytes());
            central.extend(flags.to_le_bytes());
            central.extend(method.to_le_bytes());
            central.extend(0x6000u16.to_le_bytes());
            central.extend(0x5a43u16.to_le_bytes());
            central.extend(sum.to_le_bytes());
            central.extend((packed.len() as u32).to_le_bytes());
            central.extend((contents.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend(0u32.to_le_bytes());
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        };
        add("Instrument.xml", xml, true, false, &mut out);
        add("SampleData/Sample00 (Kick).flac", blob, false, true, &mut out);
        let at = out.len() as u32;
        let len = central.len() as u32;
        out.extend(central);
        out.extend(END.to_le_bytes());
        out.extend([0, 0, 0, 0]);
        out.extend(2u16.to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend(at.to_le_bytes());
        out.extend(3u16.to_le_bytes());
        out.extend(b"hi!");
        out
    }

    #[test]
    fn an_entry_is_read_and_replaced_and_the_others_are_copied_byte_for_byte() {
        let xml = b"<?xml version=\"1.0\"?>\n<RenoiseInstrument doc_version=\"34\"><Name>Kick</Name></RenoiseInstrument>\n";
        let blob: Vec<u8> = (0..5000u32).map(|n| (n * 7 % 251) as u8).collect();
        let bytes = sample(xml, &blob);
        let archive = Archive::read(&bytes).unwrap();
        assert_eq!(
            archive.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Instrument.xml", "SampleData/Sample00 (Kick).flac"]
        );
        assert_eq!(archive.contents(archive.entry("Instrument.xml").unwrap()).unwrap(), xml);
        assert_eq!(archive.contents(archive.entry("SampleData/Sample00 (Kick).flac").unwrap()).unwrap(), blob);
        let new_xml = b"<RenoiseInstrument doc_version=\"34\"><Name>Kick 2</Name></RenoiseInstrument>";
        let written = archive.with_replaced("Instrument.xml", new_xml).unwrap();
        let again = Archive::read(&written).unwrap();
        assert_eq!(again.contents(again.entry("Instrument.xml").unwrap()).unwrap(), new_xml);
        assert_eq!(again.contents(again.entry("SampleData/Sample00 (Kick).flac").unwrap()).unwrap(), blob);
        // The sample's record (header, data and descriptor) is the very bytes it was.
        let old = archive.entry("SampleData/Sample00 (Kick).flac").unwrap();
        let new = again.entry("SampleData/Sample00 (Kick).flac").unwrap();
        assert_eq!(&bytes[old.start..old.end], &written[new.start..new.end]);
        assert_eq!(again.comment, b"hi!");
        // Replaced twice is the same archive as replaced once from the second's contents.
        assert_eq!(again.with_replaced("Instrument.xml", new_xml).unwrap(), written);
    }

    /// Every instrument Renoise ships, where Renoise is installed here (not in CI): its Instrument.xml reads, and writing the
    /// archive with it unchanged keeps every other entry's bytes.
    #[test]
    fn renoises_own_instruments_read_and_write_back_where_renoise_is_installed() {
        fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "xrni") {
                    found.push(path);
                }
            }
        }
        let mut found = vec![];
        walk(std::path::Path::new("/Applications/Renoise.app/Contents/Resources/Library/Instruments"), &mut found);
        for path in found.iter().take(40) {
            let bytes = std::fs::read(path).unwrap();
            let archive = Archive::read(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let entry = archive.entry("Instrument.xml").unwrap_or_else(|| panic!("{}: no Instrument.xml", path.display()));
            let xml = archive.contents(entry).unwrap();
            let text = String::from_utf8(xml.clone()).unwrap();
            assert_eq!(crate::renoise::xml::parse(&text).unwrap().to_xml(), text, "{}", path.display());
            let written = archive.with_replaced("Instrument.xml", &xml).unwrap();
            let again = Archive::read(&written).unwrap();
            for (old, new) in archive.entries.iter().zip(&again.entries) {
                assert_eq!(old.name, new.name);
                if old.name != "Instrument.xml" {
                    assert_eq!(&bytes[old.start..old.end], &written[new.start..new.end], "{}: {}", path.display(), old.name);
                }
            }
            assert_eq!(again.contents(again.entry("Instrument.xml").unwrap()).unwrap(), xml);
        }
    }

    #[test]
    fn what_isnt_a_plain_zip_is_refused_with_why() {
        let xml = b"<a/>";
        let bytes = sample(xml, b"x");
        for (cut, why) in [(&bytes[..10], "too short"), (&b"PK\x05\x06 nothing like a zip at all......."[..], "")] {
            assert!(Archive::read(cut).is_err(), "{why}");
        }
        let mut corrupt = bytes.clone();
        let entry = Archive::read(&bytes).unwrap().entries[0].clone();
        corrupt[entry.data] ^= 0xff;
        let archive = Archive::read(&corrupt).unwrap();
        assert!(archive.contents(&archive.entries[0]).is_err(), "a corrupted entry is refused, not returned");
        let mut zip64 = bytes.clone();
        let end = zip64.len() - 22 - 3;
        zip64[end + 16..end + 20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(Archive::read(&zip64).unwrap_err().0.contains("zip64"));
        assert!(Archive::read(&bytes).unwrap().with_replaced("Missing.xml", b"").unwrap_err().0.contains("no Missing.xml"));
    }
}
