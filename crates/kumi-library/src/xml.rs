//! Reading Live's own files (Sets, presets, racks): gzipped XML, read as a stream of tags without
//! building a tree, so a 30 MB Set takes a fraction of a second and little memory. Only tags and
//! their attributes are seen: Live keeps its values in attributes, and text (sample data, plug-in
//! state) is skipped over.

use std::io::{self, Read};
use std::path::Path;
use std::sync::LazyLock;

use flate2::read::MultiGzDecoder;
use kumi_common::abort::{Aborted, Signal, SignalExt};
use regex::Regex;
use tokio::io::AsyncReadExt;

pub trait TagHandler {
    /// A tag opened; `attrs` is its raw attribute text, read with attribute(). `empty`: it closed itself (<Tag />).
    fn open(&mut self, name: &str, attrs: &str, empty: bool);
    fn close(&mut self, name: &str);
    /// `stop?()`: whether to end a file's read early.
    fn stop(&mut self) -> bool {
        false
    }
}

#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Aborted(#[from] Aborted),
    #[error("That file is larger than Kumi reads.")]
    TooLarge,
    #[error("That file has a tag that never ends.")]
    TagTooLong,
}

/// JavaScript's `\s`: its white space and line terminators.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]{1,6}|#[0-9]{1,7}|amp|lt|gt|quot|apos);").unwrap());

fn entity(name: &str) -> &'static str {
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        _ => "'",
    }
}

fn decode(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    ENTITY
        .replace_all(value, |found: &regex::Captures| {
            let name = &found[1];
            if !name.starts_with('#') {
                return entity(name).to_string();
            }
            let code = if name.as_bytes()[1] == b'x' { u32::from_str_radix(&name[2..], 16) } else { name[1..].parse::<u32>() };
            match code {
                // TS: String.fromCodePoint, which keeps a lone surrogate; a Rust string can't, so it's the replacement character.
                Ok(code) if code > 0 && code <= 0x10ffff => {
                    char::from_u32(code).map(String::from).unwrap_or_else(|| "\u{FFFD}".to_string())
                }
                _ => String::new(),
            }
        })
        .into_owned()
}

/// One attribute's value from a tag's raw attribute text, its entities decoded.
pub fn attribute(attrs: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let mut at = attrs.find(&key)?;
    // A longer name that ends in this one ("UserName" when asking for "Name") isn't it.
    while at > 0 && !attrs[..at].chars().next_back().is_some_and(is_js_space) {
        at = at + 1 + attrs[at + 1..].find(&key)?;
    }
    let quote_at = at + name.len() + 1;
    let quote = attrs.as_bytes().get(quote_at).copied()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value_at = quote_at + 1;
    let end = attrs[value_at..].find(quote as char)?;
    Some(decode(&attrs[value_at..value_at + end]))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack, needle)
}

/// Scan tags in `text` from the start; returns where an unfinished tag begins (or the end), so the
/// caller keeps the rest for the next piece of the stream.
pub fn scan_tags<H: TagHandler + ?Sized>(text: &str, handler: &mut H) -> usize {
    let bytes = text.as_bytes();
    let length = bytes.len();
    let mut at = 0;
    while at < length {
        let Some(found) = memchr::memchr(b'<', &bytes[at..]) else { return length };
        let start = at + found;
        if start + 1 >= length {
            return start;
        }
        let next = bytes[start + 1];
        // <?xml ?>, comments and CDATA aren't Live's values.
        if next == b'?' || next == b'!' {
            let closer: &[u8] = if bytes[start..].starts_with(b"<!--") {
                b"-->"
            } else if bytes[start..].starts_with(b"<![CDATA[") {
                b"]]>"
            } else {
                b">"
            };
            let Some(end) = find_bytes(&bytes[start + 2..], closer) else { return start };
            at = start + 2 + end + closer.len();
            continue;
        }
        // The tag ends at the first > outside quotes (a value may hold one).
        let mut end = start + 1;
        loop {
            if end >= length {
                return start;
            }
            let code = bytes[end];
            if code == b'>' {
                break;
            }
            if code == b'"' || code == b'\'' {
                let Some(close) = memchr::memchr(code, &bytes[end + 1..]) else { return start };
                end = end + 1 + close + 1;
                continue;
            }
            end += 1;
        }
        if next == b'/' {
            handler.close(kumi_common::js::string::trim(&text[start + 2..end]));
        } else {
            let empty = bytes[end - 1] == b'/';
            let inner = &text[start + 1..if empty { end - 1 } else { end }];
            let space = inner.char_indices().find(|(_, c)| *c == '/' || is_js_space(*c)).map(|(index, _)| index);
            let (name, attrs) = match space {
                None => (inner, ""),
                Some(space) => (&inner[..space], &inner[space..]),
            };
            handler.open(name, attrs, empty);
            if empty {
                handler.close(name);
            }
        }
        at = end + 1;
    }
    length
}

/// Live's files are gzipped; one saved by hand may be plain XML.
fn gzipped(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b
}

/// The first `bytes` of a file, or fewer when it's shorter.
async fn read_head(path: &Path, bytes: usize) -> io::Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; bytes];
    let mut filled = 0;
    while filled < bytes {
        let count = file.read(&mut buffer[filled..]).await?;
        if count == 0 {
            break;
        }
        filled += count;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// Node's `StringDecoder("utf8")`: text from a stream of bytes, a character cut by a chunk's end kept for the next.
#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn write(&mut self, chunk: &[u8]) -> String {
        let held;
        let bytes: &[u8] = if self.pending.is_empty() {
            chunk
        } else {
            self.pending.extend_from_slice(chunk);
            held = std::mem::take(&mut self.pending);
            &held
        };
        let mut out = String::with_capacity(bytes.len());
        let mut rest = bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                    match error.error_len() {
                        None => {
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid + bad..];
                        }
                    }
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    pub signal: Option<Signal>,
    pub max_bytes: Option<u64>,
}

const CHUNK: usize = 256 * 1024;
/// The most of one unfinished tag kept between pieces: Live's tags are short (long data is text between them), so
/// one this long is a damaged or hostile file's, and reading it again with each piece would cost the square of it.
const MAX_TAG: usize = 8 * 1024 * 1024;

/// Every tag of a gzipped (or plain) XML file, in order, as the file streams in. `maxBytes` bounds the
/// XML read, so a damaged or hostile file can't fill memory; `stop()` ends the read early.
pub async fn scan_xml_file<H: TagHandler + ?Sized>(path: &Path, handler: &mut H, options: ScanOptions) -> Result<(), XmlError> {
    let head = read_head(path, 2).await?;
    let max_bytes = options.max_bytes.unwrap_or(512 * 1024 * 1024);
    let unzip = gzipped(&head);
    // The file streams in on a thread of its own, a chunk at a time, unzipped as it comes; dropping the receiver ends it.
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<io::Result<Vec<u8>>>(2);
    let file = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let read = || -> io::Result<()> {
            let source = std::fs::File::open(&file)?;
            let mut stream: Box<dyn Read> =
                if unzip { Box::new(MultiGzDecoder::new(io::BufReader::with_capacity(CHUNK, source))) } else { Box::new(source) };
            loop {
                let mut chunk = vec![0u8; CHUNK];
                let count = match stream.read(&mut chunk) {
                    Ok(count) => count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                };
                if count == 0 {
                    return Ok(());
                }
                chunk.truncate(count);
                if sender.blocking_send(Ok(chunk)).is_err() {
                    return Ok(());
                }
            }
        };
        if let Err(error) = read() {
            let _ = sender.blocking_send(Err(error));
        }
    });
    let mut decoder = Utf8Decoder::default();
    let mut pending = String::new();
    let mut read: u64 = 0;
    while let Some(chunk) = receiver.recv().await {
        let chunk = chunk?;
        if let Some(signal) = &options.signal {
            signal.check()?;
        }
        read += chunk.len() as u64;
        if read > max_bytes {
            return Err(XmlError::TooLarge);
        }
        let mut text = std::mem::take(&mut pending);
        text.push_str(&decoder.write(&chunk));
        let used = scan_tags(&text, handler);
        text.drain(..used);
        pending = text;
        if pending.len() > MAX_TAG {
            return Err(XmlError::TagTooLong);
        }
        if handler.stop() {
            break;
        }
    }
    Ok(())
}

/// As much of a gzip stream as unzips: a cut-off one ends where it ends, with no error.
fn gunzip_prefix(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = MultiGzDecoder::new(data);
    let mut out = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match decoder.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(count) => out.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(out),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

/// The start of a gzipped (or plain) XML file as text: its first `bytes` on disk, unzipped as far as
/// they go. A preset's device and name are in its first few kilobytes.
pub async fn xml_head(path: &Path, bytes: usize) -> io::Result<String> {
    let data = read_head(path, bytes).await?;
    if !gzipped(&data) {
        return Ok(String::from_utf8_lossy(&data).into_owned());
    }
    // A cut-off gzip stream unzips as far as it goes, with no error.
    Ok(String::from_utf8_lossy(&gunzip_prefix(&data)?).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn a_cut_off_gzip_stream_unzips_as_far_as_it_goes() {
        let text: String = (0..2000).map(|index| format!("<Tag{index} Value=\"{index}\" />\n")).collect();
        let packed = gzip(&text);
        for cut in [2usize, 5, 9, 12, 40, 200, packed.len() / 2, packed.len() - 9, packed.len() - 1, packed.len()] {
            let out = gunzip_prefix(&packed[..cut]).unwrap_or_else(|error| panic!("cut at {cut}: {error}"));
            assert!(text.as_bytes().starts_with(&out), "cut at {cut}: a prefix");
            if cut == packed.len() {
                assert_eq!(out.len(), text.len());
            }
        }
    }

    #[test]
    fn a_character_cut_by_a_chunk_waits_for_the_rest() {
        let mut decoder = Utf8Decoder::default();
        let bytes = "ab☺cd".as_bytes();
        let first = decoder.write(&bytes[..3]);
        let second = decoder.write(&bytes[3..]);
        assert_eq!(format!("{first}{second}"), "ab☺cd");
        assert_eq!(first, "ab");
    }
}
