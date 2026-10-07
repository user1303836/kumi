//! The library on disk: one log per kind (sounds, presets, Sets), a line of JSON per file, appended
//! as each is learned, so learning that stops (Kumi quits, the computer sleeps) carries on where it
//! was. A file learned again adds a line that replaces the one before; one that's gone adds a line
//! saying so; now and then the log is written afresh with only what stands. A reader keeps its place
//! and reads only what was added since.

use std::collections::HashMap;
use std::io::{self, SeekFrom};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use indexmap::IndexMap;
use kumi_common::js::json::{stringify, stringify_pretty};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Node's EPERM, EBUSY and EACCES.
fn is_busy(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(libc::EPERM | libc::EBUSY | libc::EACCES))
    }
    #[cfg(windows)]
    {
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        matches!(error.raw_os_error(), Some(5 | 32 | 33)) || error.kind() == io::ErrorKind::PermissionDenied
    }
    #[cfg(not(any(unix, windows)))]
    {
        error.kind() == io::ErrorKind::PermissionDenied
    }
}

/// Put a written file in place; on Windows a reader holding the old one briefly refuses it, so it's tried again.
async fn replace(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt: u64 = 0;
    loop {
        match tokio::fs::rename(from, to).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt >= 5 || !is_busy(&error) {
                    return Err(error);
                }
                tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
                attempt += 1;
            }
        }
    }
}

/// Every entry is about one file, as it was when learned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone: Option<bool>,
}

impl Entry {
    /// The line that says a file is gone.
    pub fn gone(path: impl Into<String>) -> Entry {
        Entry { path: path.into(), size: 0, mtime: 0, gone: Some(true) }
    }
}

/// What a log's entries share with `Entry`: the path that names the file.
pub trait LogEntry: Serialize + DeserializeOwned {
    fn path(&self) -> &str;
}

impl LogEntry for Entry {
    fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    kumi_library: String,
    version: u32,
    generation: String,
    created: i64,
}

/// Files Kumi writes that only this user can read: the library says what's on their computer.
const PRIVATE: u32 = 0o600;

async fn make_private_dir(folder: &Path) -> io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(folder).await
}

async fn open_private(path: &Path, append: bool) -> io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    #[cfg(unix)]
    options.mode(PRIVATE);
    #[cfg(not(unix))]
    let _ = PRIVATE;
    options.open(path).await
}

async fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let mut file = open_private(path, false).await?;
    file.write_all(text.as_bytes()).await?;
    file.flush().await
}

/// A log's bytes as text, in place when they're UTF-8 (a copy, with U+FFFD, only when they aren't).
fn text_of(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn as_io(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

async fn read_header(file: &Path) -> Option<Header> {
    let mut handle = tokio::fs::File::open(file).await.ok()?;
    let mut buffer = vec![0u8; 512];
    let mut filled = 0;
    while filled < buffer.len() {
        let count = handle.read(&mut buffer[filled..]).await.ok()?;
        if count == 0 {
            break;
        }
        filled += count;
    }
    let text = String::from_utf8_lossy(&buffer[..filled]).into_owned();
    let line = text.split('\n').next().unwrap_or_default();
    let value: Value = serde_json::from_str(line).ok()?;
    let kumi_library = value.get("kumiLibrary")?.as_str()?.to_string();
    let version = value.get("version")?.as_f64()?;
    let generation = value.get("generation")?.as_str()?.to_string();
    let created = value.get("created").and_then(Value::as_i64).unwrap_or(0);
    if version.fract() != 0.0 || version < 0.0 || version > u32::MAX as f64 {
        return Some(Header { kumi_library, version: u32::MAX, generation, created });
    }
    Some(Header { kumi_library, version: version as u32, generation, created })
}

/// A log of entries of one kind, at one version: an older version's log is started afresh.
#[derive(Debug, Clone)]
pub struct Log<T> {
    pub file: PathBuf,
    pub kind: String,
    pub version: u32,
    entry: PhantomData<T>,
}

impl<T: LogEntry> Log<T> {
    pub fn new(file: impl Into<PathBuf>, kind: &str, version: u32) -> Self {
        Log { file: file.into(), kind: kind.to_string(), version, entry: PhantomData }
    }

    fn is_mine(&self, header: &Option<Header>) -> bool {
        header.as_ref().is_some_and(|header| header.kumi_library == self.kind && header.version == self.version)
    }

    /// What stands in the log now, by path.
    pub async fn load(&self) -> IndexMap<String, T> {
        let header = read_header(&self.file).await;
        if !self.is_mine(&header) {
            return IndexMap::new();
        }
        let text = tokio::fs::read(&self.file).await.map(text_of).unwrap_or_default();
        let mut entries = IndexMap::new();
        let body = match text.find('\n') {
            Some(at) => &text[at + 1..],
            None => &text,
        };
        apply_lines(body, &mut entries);
        entries
    }

    /// Add entries (learned, or gone) at the end; a log that isn't this version's is started first.
    pub async fn append<E: Serialize>(&self, entries: &[E]) -> io::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let header = read_header(&self.file).await;
        if !self.is_mine(&header) {
            self.write(std::iter::empty()).await?;
        }
        let mut text = String::new();
        for entry in entries {
            text.push_str(&stringify(&serde_json::to_value(entry).map_err(as_io)?));
            text.push('\n');
        }
        let mut file = open_private(&self.file, true).await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await
    }

    /// The log written afresh with only `entries`: a new generation, which readers load whole.
    pub async fn write<'a, I>(&self, entries: I) -> io::Result<()>
    where
        I: IntoIterator<Item = &'a T>,
        T: 'a,
    {
        let folder = self.file.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        make_private_dir(&folder).await?;
        let header = Header {
            kumi_library: self.kind.clone(),
            version: self.version,
            generation: uuid::Uuid::new_v4().to_string(),
            created: kumi_common::time::now_ms(),
        };
        let temporary = folder.join(format!(".{}-{}", self.kind, uuid::Uuid::new_v4()));
        let written = async {
            // A line at a time through a buffer: the log is never held whole in memory (nor its lines, nor their join).
            // A megabyte of it: a 100 MB log is a hundred writes, not tokio's default's 12,800.
            let mut file = tokio::io::BufWriter::with_capacity(1 << 20, open_private(&temporary, false).await?);
            file.write_all(stringify(&serde_json::to_value(&header).map_err(as_io)?).as_bytes()).await?;
            file.write_all(b"\n").await?;
            for entry in entries {
                file.write_all(stringify(&serde_json::to_value(entry).map_err(as_io)?).as_bytes()).await?;
                file.write_all(b"\n").await?;
            }
            file.flush().await?;
            replace(&temporary, &self.file).await
        }
        .await;
        if let Err(error) = written {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error);
        }
        Ok(())
    }
}

/// JavaScript's truthiness, for a line's `gone`.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Apply a log's lines to `entries`; a line cut off by a crash is left out. A path's last line decides: gone, it
/// leaves; learned, it goes to the end, so the entries come out as applying the lines one by one leaves them. The map
/// is gone through once for all the lines (removing one entry at a time moves every entry after it).
fn apply_lines<T: LogEntry>(text: &str, entries: &mut IndexMap<String, T>) {
    // Each path's last line, numbered; None is gone.
    let mut last: HashMap<String, (usize, Option<T>)> = HashMap::new();
    for (number, line) in text.split('\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        let Some(path) = value.get("path").and_then(Value::as_str).map(str::to_string) else { continue };
        let entry = if value.get("gone").is_some_and(truthy) {
            None
        } else {
            // TS: kept a line as parsed; one that isn't an entry of this kind is left out here.
            let Ok(entry) = serde_json::from_value::<T>(value) else { continue };
            Some(entry)
        };
        last.insert(path, (number, entry));
    }
    if last.keys().any(|path| entries.contains_key(path)) {
        entries.retain(|path, _| !last.contains_key(path));
    }
    let mut learned: Vec<(usize, String, T)> =
        last.into_iter().filter_map(|(path, (number, entry))| entry.map(|entry| (number, path, entry))).collect();
    learned.sort_unstable_by_key(|(number, _, _)| *number);
    entries.reserve(learned.len());
    for (_, path, entry) in learned {
        entries.insert(path, entry);
    }
}

/// What a reader's refresh found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refreshed {
    pub changed: bool,
    pub reloaded: bool,
}

/// Reading a log as it grows: the first read loads it whole, later ones only what was appended since
/// (or the whole again when it was written afresh). `changed` says whether anything did.
#[derive(Debug)]
pub struct LogReader<T> {
    pub file: PathBuf,
    pub kind: String,
    pub version: u32,
    generation: Option<String>,
    offset: u64,
    pub entries: IndexMap<String, T>,
}

impl<T: LogEntry> LogReader<T> {
    pub fn new(file: impl Into<PathBuf>, kind: &str, version: u32) -> Self {
        LogReader { file: file.into(), kind: kind.to_string(), version, generation: None, offset: 0, entries: IndexMap::new() }
    }

    pub async fn refresh(&mut self) -> io::Result<Refreshed> {
        let header = read_header(&self.file).await;
        let Some(header) = header.filter(|header| header.kumi_library == self.kind && header.version == self.version) else {
            let had = !self.entries.is_empty();
            self.entries.clear();
            self.generation = None;
            self.offset = 0;
            return Ok(Refreshed { changed: had, reloaded: had });
        };
        let size = tokio::fs::metadata(&self.file).await.map(|info| info.len()).unwrap_or(0);
        let reloaded = self.generation.as_deref() != Some(header.generation.as_str()) || size < self.offset;
        if reloaded {
            self.entries.clear();
            self.offset = 0;
            self.generation = Some(header.generation.clone());
        }
        if size == self.offset {
            return Ok(Refreshed { changed: reloaded, reloaded });
        }
        let mut handle = tokio::fs::File::open(&self.file).await?;
        handle.seek(SeekFrom::Start(self.offset)).await?;
        let mut buffer = vec![0u8; (size - self.offset) as usize];
        let mut filled = 0;
        while filled < buffer.len() {
            let count = handle.read(&mut buffer[filled..]).await?;
            if count == 0 {
                break;
            }
            filled += count;
        }
        // Only whole lines: one being written now is read next time.
        let Some(end) = buffer[..filled].iter().rposition(|byte| *byte == b'\n') else {
            return Ok(Refreshed { changed: reloaded, reloaded });
        };
        buffer.truncate(end + 1);
        let text = text_of(buffer);
        // The header, skipped where it is.
        let from = if self.offset == 0 { text.find('\n').map(|at| at + 1).unwrap_or(0) } else { 0 };
        self.offset += end as u64 + 1;
        apply_chunked(&text[from..], &mut self.entries).await;
        Ok(Refreshed { changed: true, reloaded })
    }
}

/// Big logs are read a few thousand lines at a time, so the screen keeps drawing meanwhile.
async fn apply_chunked<T: LogEntry>(text: &str, entries: &mut IndexMap<String, T>) {
    const CHUNK: usize = 1 << 20;
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let from = (at + CHUNK).min(bytes.len() - 1);
        let end = bytes[from..].iter().position(|byte| *byte == b'\n').map(|found| from + found).unwrap_or(bytes.len() - 1);
        apply_lines(&text[at..=end], entries);
        at = end + 1;
        if at < bytes.len() {
            tokio::task::yield_now().await;
        }
    }
}

/// A small JSON file written whole (state, settings of the library's own): unreadable is None.
pub async fn read_json<T: DeserializeOwned>(file: &Path) -> Option<T> {
    let bytes = tokio::fs::read(file).await.ok()?;
    serde_json::from_str(&String::from_utf8_lossy(&bytes)).ok()
}

pub async fn write_json<V: Serialize>(file: &Path, value: &V) -> io::Result<()> {
    let folder = file.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    make_private_dir(&folder).await?;
    let temporary = folder.join(format!(".{}.json", uuid::Uuid::new_v4()));
    let text = format!("{}\n", stringify_pretty(&serde_json::to_value(value).map_err(as_io)?, 1));
    let written = async {
        write_private(&temporary, &text).await?;
        replace(&temporary, file).await
    }
    .await;
    if let Err(error) = written {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(())
}

/// Numbers as compact text in a log: a Float32 vector in base64.
pub fn pack_vector<V: Copy + Into<f64>>(values: &[V]) -> String {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&((*value).into() as f32).to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Node's base64 reading: the alphabet's characters (URL-safe ones too), whatever else is around them.
static LENIENT: LazyLockEngine = LazyLockEngine::new();

struct LazyLockEngine(std::sync::LazyLock<GeneralPurpose>);

impl LazyLockEngine {
    const fn new() -> Self {
        LazyLockEngine(std::sync::LazyLock::new(|| {
            GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent).with_decode_allow_trailing_bits(true),
            )
        }))
    }
}

pub fn unpack_vector(text: &str) -> Vec<f32> {
    let clean: String = text
        .chars()
        .take_while(|c| *c != '=')
        .filter_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => Some(c),
            '-' => Some('+'),
            '_' => Some('/'),
            _ => None,
        })
        .collect();
    let clean = &clean[..clean.len() - clean.len() % 4 + if clean.len() % 4 >= 2 { clean.len() % 4 } else { 0 }];
    let bytes = LENIENT.0.decode(clean).unwrap_or_default();
    bytes.chunks_exact(4).map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Sound {
        path: String,
        size: u64,
        mtime: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        class: Option<String>,
    }

    impl LogEntry for Sound {
        fn path(&self) -> &str {
            &self.path
        }
    }

    fn sound(path: &str, class: &str) -> Sound {
        Sound { path: path.into(), size: 10, mtime: 20, class: Some(class.into()) }
    }

    #[tokio::test]
    async fn a_log_is_appended_read_back_and_written_afresh_and_a_reader_reads_only_what_was_added() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sounds.jsonl");
        let log: Log<Sound> = Log::new(&file, "sounds", 1);
        assert!(log.load().await.is_empty());
        log.append(&[sound("/a.wav", "kick"), sound("/b.wav", "hat")]).await.unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("{\"kumiLibrary\":\"sounds\",\"version\":1,\"generation\":\""), "{text}");
        assert!(text.ends_with("{\"path\":\"/a.wav\",\"size\":10,\"mtime\":20,\"class\":\"kick\"}\n{\"path\":\"/b.wav\",\"size\":10,\"mtime\":20,\"class\":\"hat\"}\n"), "{text}");
        let mut reader: LogReader<Sound> = LogReader::new(&file, "sounds", 1);
        assert_eq!(reader.refresh().await.unwrap(), Refreshed { changed: true, reloaded: true });
        assert_eq!(reader.entries.keys().collect::<Vec<_>>(), vec!["/a.wav", "/b.wav"]);
        assert_eq!(reader.refresh().await.unwrap(), Refreshed { changed: false, reloaded: false });
        // Learned again: the newer line replaces the older, at the end. Gone: it leaves.
        log.append(&[serde_json::to_value(sound("/a.wav", "snare")).unwrap(), serde_json::to_value(Entry::gone("/b.wav")).unwrap()])
            .await
            .unwrap();
        assert_eq!(reader.refresh().await.unwrap(), Refreshed { changed: true, reloaded: false });
        assert_eq!(reader.entries.get("/a.wav").and_then(|entry| entry.class.clone()).as_deref(), Some("snare"));
        assert!(!reader.entries.contains_key("/b.wav"));
        let loaded = log.load().await;
        assert_eq!(loaded.len(), 1);
        // Written afresh: a new generation, read whole.
        log.write(loaded.values()).await.unwrap();
        assert_eq!(reader.refresh().await.unwrap(), Refreshed { changed: true, reloaded: true });
        assert_eq!(reader.entries.len(), 1);
        // Another version's log starts afresh.
        let newer: Log<Sound> = Log::new(&file, "sounds", 2);
        assert!(newer.load().await.is_empty());
        newer.append(&[sound("/c.wav", "clap")]).await.unwrap();
        assert_eq!(newer.load().await.len(), 1);
        assert_eq!(reader.refresh().await.unwrap(), Refreshed { changed: true, reloaded: true });
        assert!(reader.entries.is_empty());
    }

    /// What applying a log's lines one at a time leaves.
    fn apply_one_by_one(text: &str, entries: &mut IndexMap<String, Sound>) {
        for line in text.split('\n') {
            let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
            let Some(path) = value.get("path").and_then(Value::as_str).map(str::to_string) else { continue };
            if value.get("gone").is_some_and(truthy) {
                entries.shift_remove(&path);
            } else if let Ok(entry) = serde_json::from_value::<Sound>(value) {
                entries.shift_remove(&path);
                entries.insert(path, entry);
            }
        }
    }

    #[tokio::test]
    async fn lines_applied_together_leave_what_applying_them_one_by_one_does() {
        use rand::{Rng, SeedableRng};
        let mut random = rand::rngs::StdRng::seed_from_u64(245);
        for round in 0..200 {
            let paths = random.random_range(1..40);
            let mut text = String::new();
            for number in 0..random.random_range(0..120) {
                let path = format!("/s/{}.wav", random.random_range(0..paths));
                let line = match random.random_range(0..10) {
                    0..=4 => stringify(&serde_json::to_value(sound(&path, &format!("c{number}"))).unwrap()),
                    5..=7 => stringify(&serde_json::to_value(Entry::gone(&path)).unwrap()),
                    // Not a sound: left out, so it neither replaces nor removes.
                    8 => format!("{{\"path\":\"{path}\",\"size\":\"big\"}}"),
                    _ => format!("{{\"path\":\"{path}\",\"size\":1"),
                };
                text.push_str(&line);
                text.push('\n');
            }
            let mut before = IndexMap::new();
            for index in 0..random.random_range(0..20) {
                before.insert(format!("/s/{index}.wav"), sound(&format!("/s/{index}.wav"), "old"));
            }
            let mut expected = before.clone();
            apply_one_by_one(&text, &mut expected);
            let mut whole = before.clone();
            apply_lines(&text, &mut whole);
            assert_eq!(whole.iter().collect::<Vec<_>>(), expected.iter().collect::<Vec<_>>(), "round {round}");
            // A reader's chunks, split anywhere between lines, leave the same.
            let mut chunked = before.clone();
            let mut at = 0;
            while at < text.len() {
                let from = (at + random.random_range(1..400)).min(text.len() - 1);
                let end = text[from..].find('\n').map(|found| from + found).unwrap_or(text.len() - 1);
                apply_lines(&text[at..=end], &mut chunked);
                at = end + 1;
            }
            assert_eq!(chunked.iter().collect::<Vec<_>>(), expected.iter().collect::<Vec<_>>(), "round {round}, chunked");
        }
    }

    #[tokio::test]
    async fn a_log_written_afresh_is_its_header_then_a_line_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sounds.jsonl");
        let log: Log<Sound> = Log::new(&file, "sounds", 1);
        log.write([sound("/a.wav", "kick"), sound("/b.wav", "hat")].iter()).await.unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let (header, rest) = text.split_once('\n').unwrap();
        let header: Value = serde_json::from_str(header).unwrap();
        assert_eq!((header["kumiLibrary"].as_str(), header["version"].as_u64()), (Some("sounds"), Some(1)));
        assert_eq!(
            rest,
            "{\"path\":\"/a.wav\",\"size\":10,\"mtime\":20,\"class\":\"kick\"}\n{\"path\":\"/b.wav\",\"size\":10,\"mtime\":20,\"class\":\"hat\"}\n"
        );
        log.write(std::iter::empty()).await.unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.ends_with("}\n") && text.matches('\n').count() == 1, "{text}");
    }

    #[tokio::test]
    async fn a_line_that_isnt_utf8_reads_lossily_and_its_neighbours_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sounds.jsonl");
        let log: Log<Sound> = Log::new(&file, "sounds", 1);
        log.append(&[sound("/a.wav", "kick")]).await.unwrap();
        let mut bytes = std::fs::read(&file).unwrap();
        bytes.extend_from_slice(b"{\"path\":\"/b\xff.wav\",\"size\":10,\"mtime\":20}\n");
        bytes.extend_from_slice(b"{\"path\":\"/c.wav\",\"size\":10,\"mtime\":20,\"class\":\"hat\"}\n");
        std::fs::write(&file, bytes).unwrap();
        let mut reader: LogReader<Sound> = LogReader::new(&file, "sounds", 1);
        reader.refresh().await.unwrap();
        assert_eq!(reader.entries.keys().collect::<Vec<_>>(), ["/a.wav", "/b\u{fffd}.wav", "/c.wav"]);
        assert_eq!(log.load().await.keys().cloned().collect::<Vec<_>>(), ["/a.wav", "/b\u{fffd}.wav", "/c.wav"]);
    }

    #[tokio::test]
    async fn json_files_are_written_with_one_space_of_indent_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deeper").join("state.json");
        write_json(&file, &serde_json::json!({ "last": { "sounds": 7 }, "list": [1, 2] })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\n \"last\": {\n  \"sounds\": 7\n },\n \"list\": [\n  1,\n  2\n ]\n}\n");
        let read: Option<Value> = read_json(&file).await;
        assert_eq!(read.unwrap()["last"]["sounds"], 7);
        let none: Option<Value> = read_json(&dir.path().join("missing.json")).await;
        assert!(none.is_none());
    }

    #[test]
    fn vectors_pack_to_base64_and_back() {
        let packed = pack_vector(&[1.0f32, -2.5, 0.0]);
        assert_eq!(packed, "AACAPwAAIMAAAAAA");
        assert_eq!(unpack_vector(&packed), vec![1.0, -2.5, 0.0]);
        assert_eq!(unpack_vector("AACAPwAAIM"), vec![1.0]);
        assert_eq!(unpack_vector(""), Vec::<f32>::new());
    }
}
