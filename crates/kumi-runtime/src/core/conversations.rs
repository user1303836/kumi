//! Each Set's kept conversations, on disk: `<place>/conversations/<id>.json`, with `<place>/current` naming the one
//! the place carries on with. Beside a conversation, `<id>.meta` keeps the producer's name for it, its pin and what it
//! was copied from, and `<id>.lock` says which Kumi has it open; an earlier Kumi reading the folder takes neither for a
//! conversation. `conversation-index.json` lists every place's conversations, to find one by its id or name: it's
//! read again from the files wherever they changed, and made again from them when it's gone.

use super::daw::DawId;
use super::place::{Place, Places};
use crate::{
    core::{
        contracts::{
            ChangeState, Claim, ConversationMatch, ConversationStore, ConversationSummary, CurrentConversation, FoundExchange,
            SavedConversation, TranscriptRole,
        },
        errors::RuntimeError,
    },
    kernel::budget::transcript_of,
};
use async_trait::async_trait;
use kumi_common::js::{
    json::stringify,
    string::{head, trim, utf16_len},
};
use rand::TryRngCore;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::LazyLock,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_CONVERSATION_BYTES: usize = 256 * 1024;
const MAX_KEPT_CHANGES: usize = 100;
/// The newest conversations a place keeps, however old.
const MAX_KEPT: usize = 20;
/// Days a conversation is kept after it was last used, beyond its place's newest (`conversations.keepDays`).
pub const KEEP_DAYS: u32 = 90;
/// What Kumi says of a conversation it let go, when the producer names it.
pub fn gone_words(keep_days: u32) -> String {
    match keep_days {
        0 => "That conversation isn't kept any more.".into(),
        days => format!("That conversation isn't kept any more (Kumi keeps {days} days, and pinned ones always)."),
    }
}
const DAY_MS: f64 = 86_400_000.0;
const INDEX: &str = "conversation-index.json";
/// The most let-go ids the index remembers, to say one isn't kept any more.
const MAX_GONE: usize = 2000;
/// The longest name the producer gives a conversation, and title, in UTF-16 units.
const MAX_TITLE: usize = 60;
/// How long a name or pin stays without its conversation before it's taken for one left behind: a copy's is written just
/// before the copy itself.
const ORPHAN_MS: f64 = 10.0 * 60_000.0;
fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
pub(crate) async fn write_privately(folder: &Path, name: &str, text: &str) -> Result<(), RuntimeError> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(folder).await.map_err(error)?;
    let temporary = folder.join(format!(".{name}-{}", uuid::Uuid::new_v4()));
    let result = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&temporary, folder.join(name)).await
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error(e));
    }
    Ok(())
}
pub(crate) async fn remove(file: impl AsRef<Path>) -> Result<(), RuntimeError> {
    match tokio::fs::remove_file(file).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(e)),
    }
}
pub fn new_conversation_id(at: i64) -> String {
    let mut number = at.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(number % 36) as usize] as char);
        number /= 36;
        if number == 0 {
            break;
        }
    }
    if at < 0 {
        digits.push('-')
    }
    let mut bytes = [0; 3];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).expect("OS randomness");
    format!("{}{}", digits.into_iter().rev().collect::<String>(), hex::encode(bytes))
}
/// Where `kumi import` keeps conversations from files: made on another computer, they carry on in a copy.
pub use super::place::IMPORTED;
/// What a place is called, kept beside its conversations (its DAW's baseline is its DAW's own).
const PLACE_FILE: &str = "place.json";
/// `place.json`: the place's name, and its DAW when it isn't Live.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PlaceFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daw: Option<DawId>,
    name: String,
}
static CONVERSATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-z]{6,24}$").unwrap());
/// The Set's name in a baseline file: its first key `name`, which comes before the pages.
static BASELINE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[{,]"name":("(?:[^"\\]|\\.)*")"#).unwrap());
/// The most kept conversations a search reads, newest first.
const MAX_SEARCHED: usize = 400;
pub(crate) async fn modified_ms(path: &Path) -> f64 {
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|s| s.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
pub struct FileConversationStore {
    directory: PathBuf,
    /// The DAWs whose places it keeps conversations in.
    places: Places,
    /// The days settings say to keep conversations past each place's 20 newest; None when they don't say.
    keep_days: Option<u32>,
    /// The conversations this Kumi has open (place, id), and how it holds each: another Kumi asking for one carries on in
    /// a copy, and a Kumi that closes, or crashes, lets go.
    held: RefCell<HashMap<(String, String), Hold>>,
    /// Whether to take the system's file locks: without them, as on a file system that can't, the file names its Kumi.
    file_locks: Cell<bool>,
    /// One look at the files at a time.
    looking: tokio::sync::Mutex<()>,
}
/// How this Kumi holds a conversation it has open.
enum Hold {
    /// The system's lock on `<id>.lock`, for as long as the handle is open.
    Locked(std::fs::File),
    /// Where the file system can't lock (some network drives): `<id>.lock` names this Kumi until it lets go.
    Named,
}
pub fn create_conversation_store(directory: impl Into<PathBuf>) -> Rc<FileConversationStore> {
    create_conversation_store_keeping(directory, None)
}
/// A store that keeps conversations `keep_days` after they were last used (0: every one, however old), as settings say.
/// None when they don't: as they said last, and 90 days at least.
pub fn create_conversation_store_keeping(directory: impl Into<PathBuf>, keep_days: Option<u32>) -> Rc<FileConversationStore> {
    create_conversation_store_in(directory, keep_days, Places::LIVE)
}
/// `create_conversation_store_keeping`, taking the places of these DAWs (Live's and the others this Kumi integrates).
pub fn create_conversation_store_in(directory: impl Into<PathBuf>, keep_days: Option<u32>, places: Places) -> Rc<FileConversationStore> {
    Rc::new(FileConversationStore {
        directory: directory.into(),
        keep_days,
        places,
        held: RefCell::default(),
        file_locks: Cell::new(true),
        looking: tokio::sync::Mutex::new(()),
    })
}
impl Drop for FileConversationStore {
    fn drop(&mut self) {
        for ((place, id), hold) in self.held.take() {
            self.let_go(&place, &id, hold);
        }
    }
}

/// What a conversation is called: the producer's name for it, else its first request, as one line with anything that
/// looks like a key hidden, at most 60 characters.
pub fn title(name: Option<&str>, first: &str) -> String {
    let words = name.map(trim).filter(|name| !name.is_empty()).unwrap_or(first);
    // Characters that draw nothing could hide words in it, or turn it around.
    let words: String = words.chars().filter(|c| !crate::core::export::invisible(*c)).collect();
    let line = crate::core::recall::plain(&words, MAX_TITLE + 1);
    if utf16_len(&line) > MAX_TITLE {
        format!("{}…", trim(&head(&line, MAX_TITLE - 1)))
    } else {
        line
    }
}

/// The first id `make` gives that isn't `taken`: a kept conversation's, or one let go.
pub fn fresh_id(taken: impl Fn(&str) -> bool, mut make: impl FnMut() -> String) -> String {
    loop {
        let id = make();
        if !taken(&id) {
            return id;
        }
    }
}

/// When a conversation began, read from its id (the time it was made, in base 36, then six random hex digits).
fn created_of(id: &str, saved_at: i64) -> i64 {
    id.get(..id.len().saturating_sub(6))
        .filter(|time| !time.is_empty())
        .and_then(|time| i64::from_str_radix(time, 36).ok())
        .filter(|at| (1..=saved_at).contains(at))
        .unwrap_or(saved_at)
}

/// A conversation's own settings, in `<id>.meta`: an earlier Kumi rewriting `<id>.json` keeps only what it knows, so they
/// live beside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    #[serde(default)]
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forked_from: Option<String>,
    /// It came from a file (kumi import), or was copied from one that did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    imported: bool,
    /// The Set a conversation from a file was about, as the file said: no place of its own names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set: Option<String>,
    /// What a later Kumi keeps here, kept as it is when this one writes the file again.
    #[serde(flatten)]
    later: serde_json::Map<String, Value>,
}
impl Meta {
    fn is_empty(&self) -> bool {
        self.name.is_none() && !self.pinned && self.forked_from.is_none() && !self.imported && self.set.is_none() && self.later.is_empty()
    }
}
/// A name as the producer gave it: one line, at most 60 characters; None when there's nothing left.
fn clean_name(name: &str) -> Option<String> {
    let line =
        name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    let line = head(&line, MAX_TITLE);
    (!trim(&line).is_empty()).then(|| trim(&line).to_owned())
}
async fn read_meta(file: &Path) -> Meta {
    let Ok(bytes) = tokio::fs::read(file).await else { return Meta::default() };
    if bytes.len() > 16 * 1024 {
        return Meta::default();
    }
    let meta: Meta = serde_json::from_slice(&bytes).unwrap_or_default();
    Meta {
        version: meta.version.max(1),
        name: meta.name.as_deref().and_then(clean_name),
        pinned: meta.pinned,
        forked_from: meta.forked_from.filter(|id| CONVERSATION.is_match(id)),
        imported: meta.imported,
        set: meta.set.as_deref().and_then(clean_name),
        later: meta.later,
    }
}

/// Which Kumi has a conversation open, written in its lock file: its process, and when that process started (a pid is
/// used again once its process is gone). The system's lock decides where the file system can lock; this, where it can't.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Lock {
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started: Option<i64>,
}
impl Lock {
    fn mine() -> Lock {
        let pid = std::process::id();
        Lock { pid, started: kumi_common::process::started_at_ms(pid) }
    }
    /// The process that wrote it is still running.
    fn alive(&self) -> bool {
        matches!((self.started, kumi_common::process::started_at_ms(self.pid)), (Some(was), Some(now)) if (was - now).abs() <= 2000)
    }
    fn is_mine(&self) -> bool {
        self.pid == std::process::id() && self.alive()
    }
    /// Another Kumi's, still running.
    fn other(&self) -> bool {
        self.pid != std::process::id() && self.alive()
    }
}
fn read_lock(file: &Path) -> Option<Lock> {
    let bytes = std::fs::read(file).ok().filter(|bytes| bytes.len() <= 4096)?;
    serde_json::from_slice(&bytes).ok()
}
/// What a lock file says, read through the handle that holds it.
fn said(handle: &mut std::fs::File) -> Option<Lock> {
    use std::io::{Read, Seek};
    handle.seek(std::io::SeekFrom::Start(0)).ok()?;
    let mut bytes = vec![];
    handle.by_ref().take(4096).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}
/// A held lock file made to name `lock`, or nobody.
fn say(handle: &mut std::fs::File, lock: Option<&Lock>) -> std::io::Result<()> {
    use std::io::{Seek, Write};
    handle.set_len(0)?;
    handle.seek(std::io::SeekFrom::Start(0))?;
    match lock {
        Some(lock) => handle.write_all(stringify(&json!(lock)).as_bytes()),
        None => Ok(()),
    }
}
/// The system's lock on a conversation's `file`, held for as long as the handle stays open; None when another Kumi holds
/// it. An error when the file can't be opened, or the file system can't lock (some network drives).
fn lock_file(file: &Path) -> std::io::Result<Option<std::fs::File>> {
    if let Some(folder) = file.parent() {
        crate::core::private::create_dir_all(folder)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let handle = options.open(file)?;
    match handle.try_lock() {
        Ok(()) => Ok(Some(handle)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}
/// Whether another process holds the lock on `file` (taking it to see lets it go again at once).
fn locked_elsewhere(file: &Path) -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(file)
        .is_ok_and(|handle| matches!(handle.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// The files of a conversation as last read: when `<id>.json` was written (ns) and its size, and when `<id>.meta` was (0
/// without one), with what of them the entry holds ([`READS`]). Anything else, and it's read again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    modified: u64,
    size: u64,
    meta: u64,
    /// An entry an earlier Kumi wrote back has none (it doesn't know them), so it's read again.
    #[serde(default)]
    reads: u32,
}
/// What of a conversation's files an entry holds: 1 once it holds whether it came from a file, and that file's Set.
const READS: u32 = 1;
/// A kept conversation, as the index lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    saved_at: i64,
    first: String,
    turns: u32,
    created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forked_from: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    imported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set: Option<String>,
    stamp: Stamp,
}
/// A place's Set name, as its baseline said when Kumi read it, with when the baseline was written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PlaceName {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    modified: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Index {
    version: u32,
    /// By "<place>/<id>".
    #[serde(default)]
    conversations: BTreeMap<String, Entry>,
    #[serde(default)]
    places: BTreeMap<String, PlaceName>,
    /// Conversations that went from their folders, and when (ms): Kumi let them go, or an earlier Kumi did.
    #[serde(default)]
    gone: BTreeMap<String, i64>,
    /// The days settings last said to keep conversations: an earlier Kumi rewriting settings.json drops what it doesn't
    /// know, and a Kumi after it would otherwise keep 90 days of what the producer kept longer.
    #[serde(default, rename = "keepDays", skip_serializing_if = "Option::is_none")]
    keep_days: Option<u32>,
}
fn stamp_of(metadata: &std::fs::Metadata) -> u64 {
    metadata.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0)
}
impl FileConversationStore {
    fn folder(&self, place: &str) -> Result<PathBuf, RuntimeError> {
        if self.places.parse(place).is_none() {
            return Err(error("invalid place"));
        }
        Ok(self.directory.join(place))
    }
    fn kept(&self, place: &str) -> Result<PathBuf, RuntimeError> {
        Ok(self.folder(place)?.join("conversations"))
    }
    fn file(&self, place: &str, id: &str) -> Result<PathBuf, RuntimeError> {
        if !CONVERSATION.is_match(id) {
            return Err(error("invalid conversation id"));
        }
        Ok(self.kept(place)?.join(format!("{id}.json")))
    }
    async fn read(file: &Path) -> Option<SavedConversation> {
        let bytes = tokio::fs::read(file).await.ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        if !value["savedAt"].is_number()
            || value["checkpoint"]["version"].as_f64() != Some(1.0)
            || !value["checkpoint"]["messages"].is_array()
        {
            return None;
        }
        let mut kept = json!({"savedAt":value["savedAt"],"checkpoint":value["checkpoint"]});
        for key in ["changes", "first", "turns"] {
            let allowed = match key {
                "changes" => value[key].is_array(),
                "first" => value[key].is_string(),
                _ => value[key].is_number(),
            };
            if allowed {
                kept[key] = value[key].clone()
            }
        }
        serde_json::from_value(kept).ok()
    }
    async fn current_id(&self, place: &str) -> Option<String> {
        let folder = self.folder(place).ok()?;
        let text = tokio::fs::read_to_string(folder.join("current")).await.ok()?;
        let id = trim(&text);
        CONVERSATION.is_match(id).then(|| id.to_owned())
    }
    async fn migrate(&self, place: &str) -> Result<(), RuntimeError> {
        let legacy = self.folder(place)?.join("conversation.json");
        let Some(value) = Self::read(&legacy).await else { return Ok(()) };
        let id = new_conversation_id(value.saved_at);
        write_privately(&self.kept(place)?, &format!("{id}.json"), &stringify(&serde_json::to_value(&value).map_err(error)?)).await?;
        if self.current_id(place).await.is_none() {
            write_privately(&self.folder(place)?, "current", &id).await?;
        }
        remove(&legacy).await
    }
    async fn names(&self, place: &str) -> Vec<String> {
        let Ok(folder) = self.kept(place) else { return vec![] };
        let Ok(mut dir) = tokio::fs::read_dir(folder).await else { return vec![] };
        let mut names = vec![];
        while let Ok(Some(entry)) = dir.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if name.strip_suffix(".json").is_some_and(|id| CONVERSATION.is_match(id)) {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        names
    }
    /// A saved place's name: as `note_place` kept it, else (a Set from before) read from the start of Live's baseline
    /// (the pages after it can be megabytes).
    async fn set_name(&self, place: &str) -> Option<String> {
        if matches!(self.places.parse(place), Some(Place::Unsaved(_)) | None) {
            return None;
        }
        if let Ok(text) = tokio::fs::read_to_string(self.folder(place).ok()?.join(PLACE_FILE)).await {
            if let Some(name) = serde_json::from_str::<PlaceFile>(&text).ok().map(|file| file.name).filter(|name| !trim(name).is_empty()) {
                return Some(name);
            }
        }
        let file = tokio::fs::File::open(self.folder(place).ok()?.join("last-seen.json")).await.ok()?;
        let mut start = Vec::with_capacity(16 * 1024);
        file.take(16 * 1024).read_to_end(&mut start).await.ok()?;
        let text = String::from_utf8_lossy(&start);
        let quoted = BASELINE_NAME.captures(&text)?.get(1)?.as_str();
        serde_json::from_str::<String>(quoted).ok().filter(|name| !trim(name).is_empty())
    }
    /// A place keeps its current conversation, its 20 newest, the pinned ones, any another Kumi has open, and every one
    /// used in the last `keep_days` days (all of them, at 0): every conversation an earlier Kumi kept, and more.
    async fn trim(&self, place: &str) -> Result<(), RuntimeError> {
        let current = self.current_id(place).await;
        let kept = self.kept(place)?;
        let mut all = vec![];
        for name in self.names(place).await {
            let at = modified_ms(&kept.join(&name)).await;
            all.push((name, at));
        }
        // Newest first, and the same order an earlier Kumi gave them, ties and all.
        all.sort_by(|a, b| b.1.total_cmp(&a.1));
        let now = kumi_common::time::now_ms() as f64;
        let beyond: Vec<_> = all.into_iter().skip(MAX_KEPT).collect();
        let days = if beyond.is_empty() { KEEP_DAYS } else { self.days_kept().await };
        for (name, at) in beyond {
            let id = &name[..name.len() - 5];
            if current.as_deref() == Some(id) || days == 0 || now - at < f64::from(days) * DAY_MS {
                continue;
            }
            if read_meta(&kept.join(format!("{id}.meta"))).await.pinned
                || self.held.borrow().contains_key(&(place.to_owned(), id.to_owned()))
                || self.open_elsewhere(place, id)
            {
                continue;
            }
            remove(kept.join(&name)).await?;
            remove(kept.join(format!("{id}.meta"))).await?;
            // Only a place to lock: one that can't go (a folder there, say) stays.
            let _ = tokio::fs::remove_file(kept.join(format!("{id}.lock"))).await;
        }
        // A name or pin whose conversation went goes too: an earlier Kumi let it go, or moved it to its Set's place once
        // the Set was saved (then the name and pin go after it).
        if let Ok(mut dir) = tokio::fs::read_dir(&kept).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_suffix(".meta").filter(|id| CONVERSATION.is_match(id)) else { continue };
                if kept.join(format!("{id}.json")).exists() || now - modified_ms(&entry.path()).await < ORPHAN_MS {
                    continue;
                }
                match self.kept_elsewhere(place, id).await {
                    Some(there) if !there.join(&name).exists() => {
                        let _ = tokio::fs::rename(entry.path(), there.join(&name)).await;
                    }
                    _ => remove(entry.path()).await?,
                }
            }
        }
        Ok(())
    }
    /// The folder of another place keeping conversation `id`.
    async fn kept_elsewhere(&self, place: &str, id: &str) -> Option<PathBuf> {
        let mut places = tokio::fs::read_dir(&self.directory).await.ok()?;
        while let Ok(Some(entry)) = places.next_entry().await {
            let other = entry.file_name().to_string_lossy().into_owned();
            if other == place || self.places.parse(&other).is_none() {
                continue;
            }
            let kept = entry.path().join("conversations");
            if tokio::fs::try_exists(kept.join(format!("{id}.json"))).await.unwrap_or(false) {
                return Some(kept);
            }
        }
        None
    }
    /// How many days past its 20 newest a place keeps conversations (0: all of them): what settings say, else what they
    /// said last, and 90 at least.
    pub async fn days_kept(&self) -> u32 {
        if let Some(days) = self.keep_days {
            return days;
        }
        match self.read_index().await.0.keep_days {
            Some(0) => 0,
            Some(days) => days.max(KEEP_DAYS),
            None => KEEP_DAYS,
        }
    }
    /// What `place` keeps and lets go, as after each save there.
    pub async fn tidy(&self, place: &str) -> Result<(), RuntimeError> {
        self.trim(place).await?;
        self.index().await;
        Ok(())
    }
    /// Whether another Kumi has the conversation open (this one's own are in `held`): the system's lock on it is taken,
    /// or, where the file system can't lock, the file names another Kumi that's still running.
    fn open_elsewhere(&self, place: &str, id: &str) -> bool {
        if self.held.borrow().contains_key(&(place.to_owned(), id.to_owned())) {
            return false;
        }
        let Ok(kept) = self.kept(place) else { return false };
        let file = kept.join(format!("{id}.lock"));
        (self.file_locks.get() && locked_elsewhere(&file)) || read_lock(&file).is_some_and(|lock| lock.other())
    }
    /// Lets go of a conversation: the file names nobody, and the system's lock goes with the handle. A file that only
    /// named this Kumi goes.
    fn let_go(&self, place: &str, id: &str, hold: Hold) {
        match hold {
            Hold::Locked(mut handle) => {
                let _ = say(&mut handle, None);
                // Let go now: Windows frees a closed file's lock only in time, and a process started a moment ago
                // still shares the handle.
                let _ = handle.unlock();
            }
            Hold::Named => {
                let Ok(kept) = self.kept(place) else { return };
                let file = kept.join(format!("{id}.lock"));
                if read_lock(&file).is_some_and(|lock| lock.is_mine()) {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
    }
    /// Remembers the Set a conversation from a file was about, as the file said: listed and exported again with it. False
    /// when it isn't kept.
    pub async fn note_set(&self, place: &str, id: &str, set: &str) -> Result<bool, RuntimeError> {
        let set = clean_name(set);
        self.change_meta(place, id, |meta| meta.set = set).await
    }
    /// Holds conversations as on a file system that can't lock (some network drives): for tests.
    #[doc(hidden)]
    pub fn without_file_locks(&self) {
        self.file_locks.set(false);
    }
    fn place_of(&self, key: &str) -> Option<(String, String)> {
        key.split_once('/').map(|(place, id)| (place.to_owned(), id.to_owned()))
    }
    /// Every place's conversation files as they are now: (place, id) → stamp.
    async fn scan(&self) -> BTreeMap<String, Stamp> {
        let mut found = BTreeMap::new();
        let Ok(mut places) = tokio::fs::read_dir(&self.directory).await else { return found };
        while let Ok(Some(place)) = places.next_entry().await {
            let Some(place) = place.file_name().to_str().filter(|p| self.places.parse(p).is_some()).map(str::to_owned) else { continue };
            let Ok(kept) = self.kept(&place) else { continue };
            let Ok(mut dir) = tokio::fs::read_dir(&kept).await else { continue };
            let mut metas = HashMap::new();
            let mut files = vec![];
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(metadata) = entry.metadata().await else { continue };
                if let Some(id) = name.strip_suffix(".json").filter(|id| CONVERSATION.is_match(id)) {
                    files.push((id.to_owned(), Stamp { modified: stamp_of(&metadata), size: metadata.len(), meta: 0, reads: READS }));
                } else if let Some(id) = name.strip_suffix(".meta").filter(|id| CONVERSATION.is_match(id)) {
                    metas.insert(id.to_owned(), stamp_of(&metadata));
                }
            }
            for (id, mut stamp) in files {
                stamp.meta = metas.get(&id).copied().unwrap_or(0);
                found.insert(format!("{place}/{id}"), stamp);
            }
        }
        found
    }
    /// The index as kept, and whether a later Kumi wrote it in a version this one doesn't know: this one then makes its
    /// own from the files each time, and never writes over that one.
    async fn read_index(&self) -> (Index, bool) {
        let fresh = Index { version: 1, ..Index::default() };
        let Ok(bytes) = tokio::fs::read(self.directory.join(INDEX)).await else { return (fresh, false) };
        #[derive(Deserialize)]
        struct Version {
            #[serde(default)]
            version: u64,
        }
        match serde_json::from_slice::<Version>(&bytes).map(|written| written.version) {
            Ok(1) => (serde_json::from_slice::<Index>(&bytes).unwrap_or(fresh), false),
            Ok(version) if version > 1 => {
                let said = serde_json::from_slice::<Value>(&bytes).ok().and_then(|index| index["keepDays"].as_u64());
                (Index { keep_days: said.and_then(|days| u32::try_from(days).ok()), ..fresh }, true)
            }
            _ => (fresh, false),
        }
    }
    /// The index, read again from the files wherever they changed (made from them all when it's missing or unreadable),
    /// and written back when anything did.
    async fn index(&self) -> Index {
        let _one = self.looking.lock().await;
        let (before, later) = self.read_index().await;
        let mut index = before.clone();
        index.keep_days = self.keep_days.or(before.keep_days);
        let files = self.scan().await;
        let kept_ids: HashSet<&str> = files.keys().filter_map(|key| key.split_once('/').map(|(_, id)| id)).collect();
        let now = kumi_common::time::now_ms();
        for key in index.conversations.keys().filter(|key| !files.contains_key(*key)).cloned().collect::<Vec<_>>() {
            index.conversations.remove(&key);
            if let Some((_, id)) = self.place_of(&key).filter(|(_, id)| !kept_ids.contains(id.as_str())) {
                index.gone.insert(id, now);
            }
        }
        for (key, stamp) in &files {
            if index.conversations.get(key).is_some_and(|entry| entry.stamp == *stamp) {
                continue;
            }
            let Some((place, id)) = self.place_of(key) else { continue };
            let Ok(kept) = self.kept(&place) else { continue };
            // Reading many files: the app keeps drawing between them.
            tokio::task::yield_now().await;
            let Some(value) = Self::read(&kept.join(format!("{id}.json"))).await else {
                index.conversations.remove(key);
                continue;
            };
            let meta = read_meta(&kept.join(format!("{id}.meta"))).await;
            let said: Vec<_> = transcript_of(&value.checkpoint.messages).into_iter().filter(|l| l.role == TranscriptRole::User).collect();
            index.conversations.insert(
                key.clone(),
                Entry {
                    saved_at: value.saved_at,
                    first: value.first.unwrap_or_else(|| said.first().map(|l| head(&l.text, 200)).unwrap_or_default()),
                    turns: value.turns.unwrap_or(said.len() as u32),
                    created: created_of(&id, value.saved_at),
                    name: meta.name,
                    pinned: meta.pinned,
                    forked_from: meta.forked_from,
                    imported: meta.imported,
                    set: meta.set,
                    stamp: *stamp,
                },
            );
        }
        for id in &kept_ids {
            index.gone.remove(*id);
        }
        while index.gone.len() > MAX_GONE {
            let oldest = index.gone.iter().min_by_key(|(_, at)| **at).map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                index.gone.remove(&oldest);
            }
        }
        // Each place's Set name, read again when its baseline was written since.
        let places: HashSet<String> = index.conversations.keys().filter_map(|key| self.place_of(key).map(|(place, _)| place)).collect();
        index.places.retain(|place, _| places.contains(place));
        for place in places {
            let modified = match self.folder(&place) {
                Ok(folder) => tokio::fs::metadata(folder.join("last-seen.json")).await.map(|m| stamp_of(&m)).unwrap_or(0),
                Err(_) => 0,
            };
            if index.places.get(&place).is_some_and(|known| known.modified == modified) {
                continue;
            }
            let name = self.set_name(&place).await;
            index.places.insert(place, PlaceName { name, modified });
        }
        if !later && index != before && !(files.is_empty() && index.gone.is_empty()) {
            let _ = write_privately(&self.directory, INDEX, &stringify(&serde_json::to_value(&index).unwrap_or_default())).await;
        }
        index
    }
    /// Every kept conversation, from the index, in the order an earlier Kumi listed a place's: newest first.
    async fn summaries(&self) -> Vec<ConversationSummary> {
        let index = self.index().await;
        let mut currents: HashMap<String, Option<String>> = HashMap::new();
        let mut rows = vec![];
        for (key, entry) in &index.conversations {
            let Some((place, id)) = self.place_of(key) else { continue };
            if !currents.contains_key(&place) {
                let current = self.current_id(&place).await;
                currents.insert(place.clone(), current);
            }
            let imported = entry.imported || place == IMPORTED;
            rows.push(ConversationSummary {
                current: currents[&place].as_deref() == Some(id.as_str()),
                id,
                saved_at: entry.saved_at,
                first: entry.first.clone(),
                turns: entry.turns,
                set: index.places.get(&place).and_then(|known| known.name.clone()).or_else(|| entry.set.clone()),
                place,
                name: entry.name.clone(),
                pinned: entry.pinned,
                forked_from: entry.forked_from.clone(),
                created: entry.created,
                imported,
            });
        }
        rows.sort_by_key(|a| std::cmp::Reverse(a.saved_at));
        rows
    }
    async fn write_meta(&self, place: &str, id: &str, meta: &Meta) -> Result<(), RuntimeError> {
        let kept = self.kept(place)?;
        if meta.is_empty() {
            return remove(kept.join(format!("{id}.meta"))).await;
        }
        let meta = Meta { version: meta.version.max(1), ..meta.clone() };
        write_privately(&kept, &format!("{id}.meta"), &stringify(&serde_json::to_value(meta).map_err(error)?)).await
    }
    /// Change a kept conversation's own settings; false when it isn't kept.
    async fn change_meta(&self, place: &str, id: &str, change: impl FnOnce(&mut Meta)) -> Result<bool, RuntimeError> {
        if !self.file(place, id)?.exists() {
            return Ok(false);
        }
        let mut meta = read_meta(&self.kept(place)?.join(format!("{id}.meta"))).await;
        change(&mut meta);
        self.write_meta(place, id, &meta).await?;
        self.index().await;
        Ok(true)
    }
}
/// A message as kept on disk: a picture the producer added is named, not kept, so one screenshot
/// can't crowd out the conversation.
fn without_pictures(message: &Value) -> Value {
    let Some(parts) = message["content"].as_array().filter(|_| message["role"] == "user") else { return message.clone() };
    if !parts.iter().any(|part| part["type"] == "file") {
        return message.clone();
    }
    let mut kept = message.clone();
    kept["content"] = parts
        .iter()
        .map(|part| {
            if part["type"] != "file" {
                return part.clone();
            }
            let name = part["filename"].as_str().unwrap_or("a picture");
            json!({"type":"text","text":format!("[The producer showed {name} here; pictures aren't kept with saved conversations.]")})
        })
        .collect();
    kept
}
// Disk checkpoints also contain older string-form messages; keep those forms intact.
fn bounded_messages(all: &[Value]) -> Vec<Value> {
    let sizes: Vec<_> = all.iter().map(|m| stringify(m).len() + 1).collect();
    let mut total = sizes.iter().sum::<usize>() + 1;
    let mut start = 0;
    while start < all.len() && total > MAX_CONVERSATION_BYTES {
        loop {
            total -= sizes[start];
            start += 1;
            if start >= all.len() || all[start]["role"] == "user" {
                break;
            }
        }
    }
    let mut messages = all[start..].to_vec();
    if messages.is_empty() {
        if let Some(last) = all.iter().rposition(|m| m["role"] == "user") {
            if stringify(&json!(&all[last..])).len() <= 4 * MAX_CONVERSATION_BYTES {
                messages = all[last..].to_vec();
            }
        }
    }
    if !messages.is_empty() && messages.len() < all.len() {
        const SHORT: &str = "[Kumi removed the earlier part of this conversation to save room.]\n\n";
        if messages[0]["role"] == "user" {
            if let Some(content) = messages[0]["content"].as_array_mut() {
                if let Some(part) = content.iter_mut().find(|p| p["type"] == "text") {
                    if let Some(text) = part["text"].as_str() {
                        if !text.starts_with(SHORT) {
                            part["text"] = json!(format!("{SHORT}{text}"))
                        }
                    }
                } else {
                    content.insert(0, json!({"type":"text","text":trim(SHORT)}));
                }
            }
        }
    }
    messages
}
#[async_trait(?Send)]
impl ConversationStore for FileConversationStore {
    async fn current(&self, place: &str) -> Result<Option<CurrentConversation>, RuntimeError> {
        if self.migrate(place).await.is_err() {
            return Ok(None);
        }
        let Some(id) = self.current_id(place).await else { return Ok(None) };
        let Some(conversation) = Self::read(&self.file(place, &id)?).await else { return Ok(None) };
        Ok(Some(CurrentConversation { id, conversation }))
    }
    async fn load(&self, place: &str, id: &str) -> Result<Option<SavedConversation>, RuntimeError> {
        if self.migrate(place).await.is_err() {
            return Ok(None);
        }
        let Ok(file) = self.file(place, id) else { return Ok(None) };
        Ok(Self::read(&file).await)
    }
    async fn save(&self, place: &str, id: &str, conversation: &SavedConversation) -> Result<(), RuntimeError> {
        let all: &Vec<Value> = &conversation.checkpoint.messages.iter().map(without_pictures).collect();
        let messages = bounded_messages(all);
        if messages.is_empty() {
            return Ok(());
        }
        let said: Vec<_> = transcript_of(all).into_iter().filter(|l| l.role == TranscriptRole::User).collect();
        let mut checkpoint = conversation.checkpoint.clone();
        checkpoint.messages = messages;
        let saved = SavedConversation {
            saved_at: conversation.saved_at,
            checkpoint,
            changes: conversation
                .changes
                .as_ref()
                .filter(|c| !c.is_empty())
                .map(|c| c[c.len().saturating_sub(MAX_KEPT_CHANGES)..].to_vec()),
            first: Some(head(conversation.first.as_deref().or_else(|| said.first().map(|l| l.text.as_str())).unwrap_or(""), 200)),
            turns: Some(conversation.turns.unwrap_or(0).max(said.len() as u32)),
        };
        self.file(place, id)?;
        write_privately(&self.kept(place)?, &format!("{id}.json"), &stringify(&serde_json::to_value(saved).map_err(error)?)).await?;
        write_privately(&self.folder(place)?, "current", id).await?;
        self.trim(place).await?;
        self.index().await;
        Ok(())
    }
    async fn fresh(&self, place: &str) -> Result<(), RuntimeError> {
        let folder = self.folder(place)?;
        if self.current_id(place).await.is_some() {
            write_privately(&folder, "current", "").await?
        }
        Ok(())
    }
    async fn list(&self, place: &str) -> Result<Vec<ConversationSummary>, RuntimeError> {
        if self.migrate(place).await.is_err() {
            return Ok(vec![]);
        }
        self.folder(place)?;
        Ok(self.summaries().await.into_iter().filter(|row| row.place == place).collect())
    }
    async fn set_current(&self, place: &str, id: &str) -> Result<(), RuntimeError> {
        if self.file(place, id)?.exists() {
            write_privately(&self.folder(place)?, "current", id).await?;
        }
        Ok(())
    }
    async fn note_place(&self, place: &str, daw: DawId, name: &str) -> Result<(), RuntimeError> {
        if !matches!(self.places.parse(place), Some(Place::Saved { .. })) || trim(name).is_empty() {
            return Ok(());
        }
        let folder = self.folder(place)?;
        let kept = PlaceFile { version: 1, daw: (daw != DawId::Ableton).then_some(daw), name: name.into() };
        let had =
            tokio::fs::read_to_string(folder.join(PLACE_FILE)).await.ok().and_then(|text| serde_json::from_str::<PlaceFile>(&text).ok());
        if had.as_ref() == Some(&kept) {
            return Ok(());
        }
        write_privately(&folder, PLACE_FILE, &stringify(&serde_json::to_value(&kept).map_err(error)?)).await
    }
    async fn move_conversation(&self, id: &str, from: &str, to: &str) -> Result<(), RuntimeError> {
        let file = self.file(from, id)?;
        self.file(to, id)?;
        // This Kumi's hold goes with it, kept yet or not (a Set saved before its first request).
        if self.held.borrow().contains_key(&(from.to_owned(), id.to_owned())) {
            self.release(from, id);
            self.claim(to, id);
        }
        let Some(value) = Self::read(&file).await else { return Ok(()) };
        write_privately(&self.kept(to)?, &format!("{id}.json"), &stringify(&serde_json::to_value(value).map_err(error)?)).await?;
        // Its name and pin go with it.
        let meta = read_meta(&self.kept(from)?.join(format!("{id}.meta"))).await;
        self.write_meta(to, id, &meta).await?;
        write_privately(&self.folder(to)?, "current", id).await?;
        remove(&file).await?;
        remove(self.kept(from)?.join(format!("{id}.meta"))).await?;
        if self.current_id(from).await.as_deref() == Some(id) {
            write_privately(&self.folder(from)?, "current", "").await?
        }
        self.trim(to).await?;
        self.index().await;
        Ok(())
    }
    async fn search(
        &self,
        words: &[String],
        needed: usize,
        limit: usize,
        skip: Option<(&str, &str)>,
    ) -> Result<Vec<FoundExchange>, RuntimeError> {
        if words.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let mut files = vec![];
        if let Ok(mut places) = tokio::fs::read_dir(&self.directory).await {
            while let Ok(Some(entry)) = places.next_entry().await {
                let Some(place) = entry.file_name().to_str().filter(|p| self.places.parse(p).is_some()).map(str::to_owned) else {
                    continue;
                };
                for name in self.names(&place).await {
                    let path = self.kept(&place)?.join(&name);
                    let at = modified_ms(&path).await;
                    files.push((place.clone(), name, path, at));
                }
            }
        }
        files.sort_by(|a, b| b.3.total_cmp(&a.3));
        files.truncate(MAX_SEARCHED);
        let mut found = vec![];
        for (place, name, path, _) in files {
            let id = name[..name.len() - 5].to_owned();
            if skip == Some((place.as_str(), id.as_str())) {
                continue;
            }
            // Reading and scoring hundreds of files: the app keeps drawing between them.
            tokio::task::yield_now().await;
            let Some(conversation) = Self::read(&path).await else { continue };
            let mut exchange = |said: String, answer: String, tools: Vec<String>| {
                let text = format!("{said}\n{answer}\n{}", tools.join(" ")).to_lowercase();
                let matched = words.iter().filter(|word| text.contains(word.as_str())).count();
                if matched >= needed {
                    found.push(FoundExchange {
                        place: place.clone(),
                        set: None,
                        conversation: id.clone(),
                        saved_at: conversation.saved_at,
                        said,
                        answer,
                        tools,
                        matched,
                    });
                }
            };
            // An exchange is a request and the answers to it, with the tools they used.
            let mut current: Option<(String, Vec<String>, Vec<String>)> = None;
            for line in transcript_of(&conversation.checkpoint.messages) {
                match line.role {
                    TranscriptRole::User => {
                        if let Some((said, answers, tools)) = current.take() {
                            exchange(said, answers.join("\n"), tools);
                        }
                        current = Some((line.text, vec![], vec![]));
                    }
                    TranscriptRole::Assistant => {
                        if let Some((_, answers, tools)) = current.as_mut() {
                            if !line.text.is_empty() {
                                answers.push(line.text);
                            }
                            tools.extend(line.tools.unwrap_or_default());
                        }
                    }
                }
            }
            if let Some((said, answers, tools)) = current.take() {
                exchange(said, answers.join("\n"), tools);
            }
            // What the conversation changed in Live, in plain words.
            let titles: Vec<_> =
                conversation.changes.iter().flatten().filter(|c| c.state != ChangeState::Undone).map(|c| c.title.as_str()).collect();
            if !titles.is_empty() {
                exchange(conversation.first.clone().unwrap_or_default(), format!("Changed: {}", titles.join("; ")), vec![]);
            }
        }
        found.sort_by(|a, b| b.matched.cmp(&a.matched).then(b.saved_at.cmp(&a.saved_at)));
        found.truncate(limit);
        let mut names: Vec<(String, Option<String>)> = vec![];
        for hit in &mut found {
            if let Some((_, name)) = names.iter().find(|(place, _)| *place == hit.place) {
                hit.set = name.clone();
            } else {
                let name = self.set_name(&hit.place).await;
                names.push((hit.place.clone(), name.clone()));
                hit.set = name;
            }
        }
        Ok(found)
    }
    async fn all(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(self.summaries().await)
    }
    async fn find(&self, word: &str) -> Result<ConversationMatch, RuntimeError> {
        let word = trim(word).to_lowercase();
        if word.is_empty() {
            return Ok(ConversationMatch::None);
        }
        let all = self.summaries().await;
        let picked = |rows: Vec<ConversationSummary>| match rows.len() {
            0 => ConversationMatch::None,
            1 => ConversationMatch::One(rows.into_iter().next().unwrap()),
            _ => ConversationMatch::Several(rows),
        };
        let exact: Vec<_> = all.iter().filter(|row| row.id == word).cloned().collect();
        if !exact.is_empty() {
            return Ok(picked(exact));
        }
        if CONVERSATION.is_match(&word) && self.read_index().await.0.gone.contains_key(&word) {
            return Ok(ConversationMatch::Gone);
        }
        // The start of an id (4 characters or more), or the producer's name for one.
        let start = utf16_len(&word) >= 4 && word.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
        Ok(picked(
            all.into_iter()
                .filter(|row| (start && row.id.starts_with(&word)) || row.name.as_ref().is_some_and(|name| name.to_lowercase() == word))
                .collect(),
        ))
    }
    async fn new_id(&self) -> String {
        let index = self.index().await;
        let taken: HashSet<String> =
            index.conversations.keys().filter_map(|key| self.place_of(key).map(|(_, id)| id)).chain(index.gone.keys().cloned()).collect();
        fresh_id(|id| taken.contains(id), || new_conversation_id(kumi_common::time::now_ms()))
    }
    async fn rename(&self, place: &str, id: &str, name: Option<&str>) -> Result<bool, RuntimeError> {
        let name = name.and_then(clean_name);
        self.change_meta(place, id, |meta| meta.name = name).await
    }
    async fn pin(&self, place: &str, id: &str, pinned: bool) -> Result<bool, RuntimeError> {
        self.change_meta(place, id, |meta| meta.pinned = pinned).await
    }
    async fn delete(&self, place: &str, id: &str) -> Result<bool, RuntimeError> {
        let file = self.file(place, id)?;
        if !file.exists() {
            return Ok(false);
        }
        // Gone from under another window, it would come back with its next save, without its name or pin.
        if self.open_elsewhere(place, id) {
            return Err(crate::core::errors::KumiError::new(
                crate::core::errors::FailureKind::Request,
                "That conversation is open in another Kumi window: close it there, then delete it.",
            )
            .into());
        }
        let kept = self.kept(place)?;
        remove(&file).await?;
        remove(kept.join(format!("{id}.meta"))).await?;
        if !self.held.borrow().contains_key(&(place.to_owned(), id.to_owned())) {
            let _ = tokio::fs::remove_file(kept.join(format!("{id}.lock"))).await;
        }
        if self.current_id(place).await.as_deref() == Some(id) {
            write_privately(&self.folder(place)?, "current", "").await?
        }
        self.index().await;
        // Deleted by the producer, not let go: the id is unknown from here on.
        let _one = self.looking.lock().await;
        let (mut index, later) = self.read_index().await;
        if !later && index.gone.remove(id).is_some() {
            let _ = write_privately(&self.directory, INDEX, &stringify(&serde_json::to_value(&index).unwrap_or_default())).await;
        }
        Ok(true)
    }
    async fn create(
        &self,
        place: &str,
        id: &str,
        conversation: &SavedConversation,
        name: Option<&str>,
        from: Option<&str>,
    ) -> Result<(), RuntimeError> {
        self.file(place, id)?;
        let from = from.filter(|from| CONVERSATION.is_match(from));
        // A copy of a conversation from a file is one too: its words are the file's, however often it's copied.
        let imported = place == IMPORTED
            || match from {
                Some(from) => matches!(self.find(from).await, Ok(ConversationMatch::One(row)) if row.id == from && row.imported),
                None => false,
            };
        // Beside it before it's there: a look at the folder never finds it without them.
        let meta = Meta { name: name.and_then(clean_name), forked_from: from.map(str::to_owned), imported, ..Meta::default() };
        self.write_meta(place, id, &meta).await?;
        self.save(place, id, conversation).await?;
        // Nothing kept of it (its last exchange alone runs longer than Kumi keeps): nothing beside it either.
        if !self.file(place, id)?.exists() {
            remove(self.kept(place)?.join(format!("{id}.meta"))).await?;
        }
        Ok(())
    }
    /// Where the file system can lock, the system's lock on `<id>.lock` decides: one Kumi gets it however many ask at
    /// once, and a Kumi that's gone holds nothing. Where it can't (some network drives), the file names the Kumi that has
    /// it, as long as that Kumi runs. Kumi never refuses a conversation: when neither can be had, it goes on, held by
    /// nobody, as before Kumi 1.17.
    fn claim(&self, place: &str, id: &str) -> Claim {
        let (Ok(kept), true) = (self.kept(place), CONVERSATION.is_match(id)) else { return Claim::Ours };
        let key = (place.to_owned(), id.to_owned());
        if self.held.borrow().contains_key(&key) {
            return Claim::Ours;
        }
        let file = kept.join(format!("{id}.lock"));
        let locked = if self.file_locks.get() { lock_file(&file) } else { Err(std::io::ErrorKind::Unsupported.into()) };
        let hold = match locked {
            Ok(None) => return Claim::Taken,
            Ok(Some(mut handle)) => {
                // Another Kumi that couldn't lock (from another computer, say) named itself here, and still runs.
                if said(&mut handle).is_some_and(|lock| lock.other()) {
                    return Claim::Taken;
                }
                let _ = say(&mut handle, Some(&Lock::mine()));
                Hold::Locked(handle)
            }
            Err(_) => {
                if read_lock(&file).is_some_and(|lock| lock.other()) {
                    return Claim::Taken;
                }
                let mine = Lock::mine();
                if crate::core::private::write(&file, stringify(&json!(mine)).as_bytes()).is_err() {
                    return Claim::Ours;
                }
                // Two Kumis naming themselves at once: the one whose words stayed has it.
                if read_lock(&file).is_some_and(|lock| lock != mine && lock.alive()) {
                    return Claim::Taken;
                }
                Hold::Named
            }
        };
        self.held.borrow_mut().insert(key, hold);
        Claim::Ours
    }
    fn release(&self, place: &str, id: &str) {
        let hold = self.held.borrow_mut().remove(&(place.to_owned(), id.to_owned()));
        if let Some(hold) = hold {
            self.let_go(place, id, hold);
        }
    }
}
