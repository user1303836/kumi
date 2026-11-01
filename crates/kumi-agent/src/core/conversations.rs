//! Each Set's kept conversations, on disk: `<place>/conversations/<id>.json`, with `<place>/current` naming the one
//! the place carries on with. Beside a conversation, `<id>.meta` keeps the producer's name for it, its pin and what it
//! was copied from, `<id>.lock` says which Kumi has it open, and `<id>.jsonl` is its log (`session_log`); an earlier
//! Kumi reading the folder takes none of them for a conversation. `conversation-index.json` lists every place's
//! conversations, to find one by its id or name: it's read again from the files wherever they changed, and made again
//! from them when it's gone. A later version of the index is kept beside it, under that version's own name.

use crate::core::text::cut;
use crate::{
    core::{
        contracts::{
            Claim, ConversationMatch, ConversationStore, ConversationSummary, CurrentConversation, FoundExchange, KernelCheckpoint,
            SavedConversation, TranscriptRole,
        },
        session_log::{log_name, read_log_file, ReadLog},
    },
    kernel::budget::{json_bytes, transcript_of},
};
use async_trait::async_trait;
use kumi_common::js::{
    json::stringify,
    string::{trim, utf16_len},
};
use kumi_types::daw::DawId;
use kumi_types::place::{Place, Places};
use kumi_types::{contracts::ChangeState, errors::RuntimeError};
use rand::TryRngCore;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    borrow::Cow,
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
/// The index every Kumi of its version (1) reads and writes. A later version keeps its own beside it, as
/// `conversation-index-v<version>.json`, and leaves this one to Kumis of version 1: each version keeps reading only what
/// changed, whichever ran last.
const INDEX: &str = "conversation-index.json";
const INDEX_VERSION: u32 = 1;
/// This version's own index, where it's kept when `conversation-index.json` holds a later version (one written there by
/// a Kumi that didn't keep its own beside it): never written over, and not made again from every file on each look.
const INDEX_BESIDE: &str = "conversation-index-v1.json";
/// The most let-go ids the index remembers, to say one isn't kept any more.
const MAX_GONE: usize = 2000;
/// The longest name the producer gives a conversation, and title, in UTF-16 units.
const MAX_TITLE: usize = 60;
/// How long a name or pin (or log, or lock) stays without its conversation before it's taken for one left behind: a
/// copy's is written just before the copy itself.
const ORPHAN_MS: f64 = 10.0 * 60_000.0;
/// How near a file's write a look at it must be for another write in the same tick to leave it the same time and size:
/// HFS+, exFAT and SMB keep times to the second, FAT to two.
const TICK_NS: u64 = 3_000_000_000;
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
use kumi_types::place::IMPORTED;
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
/// Whether a conversation can be kept under `id`.
pub(crate) fn is_conversation_id(id: &str) -> bool {
    CONVERSATION.is_match(id)
}
/// What a file beside a conversation keeps of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Beside {
    /// `<id>.meta`: its name, pin and the rest.
    Meta,
    /// `<id>.jsonl`: its log.
    Log,
    /// `<id>.lock`: which Kumi has it open.
    Lock,
}
/// The conversation a file beside one is of, and what it keeps, by the file's name.
fn beside(name: &str) -> Option<(&str, Beside)> {
    [(".meta", Beside::Meta), (".jsonl", Beside::Log), (".lock", Beside::Lock)]
        .into_iter()
        .find_map(|(suffix, kind)| name.strip_suffix(suffix).map(|id| (id, kind)))
        .filter(|(id, _)| CONVERSATION.is_match(id))
}
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
        format!("{}…", trim(cut(&line, MAX_TITLE - 1)))
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
    let line = cut(&line, MAX_TITLE);
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
    /// On Linux, the boot the process runs in, and its start in clock ticks after that boot: `started` is reckoned from the
    /// time the system says it booted, which moves when the clock is set, and these don't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ticks: Option<u64>,
}
impl Lock {
    fn mine() -> Lock {
        let pid = std::process::id();
        Lock { pid, started: kumi_common::process::started_at_ms(pid), boot: since_boot::boot(), ticks: since_boot::ticks(pid) }
    }
    /// The process that wrote it is still running.
    fn alive(&self) -> bool {
        if let (Some(boot), Some(ticks), Some(now)) = (&self.boot, self.ticks, since_boot::boot()) {
            return *boot == now && since_boot::ticks(self.pid) == Some(ticks);
        }
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
/// The system's lock on a conversation's lock `file` (made first, when `make`), held for as long as the handle stays
/// open; None when another Kumi holds it. An error when the file isn't there or can't be opened, or the file system
/// can't lock (some network drives).
fn lock_file(file: &Path, make: bool) -> std::io::Result<Option<std::fs::File>> {
    if let Some(folder) = file.parent().filter(|_| make) {
        kumi_types::private::create_dir_all(folder)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(make).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let handle = options.open(file)?;
    match handle.try_lock() {
        Ok(()) => Ok(Some(handle)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}
/// `lock_file`, asked again for a moment while another Kumi holds it only to look (a Kumi tidying takes it for an
/// instant to see whether anyone has it, and one letting go of the file removes it while holding it), and again when
/// the file it locked was removed meanwhile: that one is nobody's to hold, the one made in its place is. Still removed
/// at the last try, it's another Kumi's to have; one whose id can't be read (a file system that keeps none) is taken as
/// it is.
fn take_lock(file: &Path) -> std::io::Result<Option<std::fs::File>> {
    take_lock_with(file, |file| lock_file(file, true))
}
/// `take_lock`, locking with `lock`.
fn take_lock_with(
    file: &Path,
    mut lock: impl FnMut(&Path) -> std::io::Result<Option<std::fs::File>>,
) -> std::io::Result<Option<std::fs::File>> {
    const TRIES: u32 = 4;
    let mut tries = 0;
    loop {
        tries += 1;
        let last = tries == TRIES;
        match lock(file) {
            Ok(Some(handle)) => match is_at(&handle, file) {
                Some(true) | None => return Ok(Some(handle)),
                Some(false) => {
                    unlock(handle);
                    if last {
                        return Ok(None);
                    }
                }
            },
            Ok(None) if last => return Ok(None),
            Ok(None) => {}
            // Windows refuses to open a file removed while something still has it open, until that lets go.
            Err(error) if last || error.kind() != std::io::ErrorKind::PermissionDenied => return Err(error),
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(1 << tries));
    }
}
/// Lets go of a lock now: Windows frees a closed file's lock only in time, and a process started a moment ago still
/// shares the handle.
fn unlock(handle: std::fs::File) {
    let _ = handle.unlock();
}
/// Whether another Kumi holds the lock on `file` (taking it to see lets it go again at once).
fn locked_elsewhere(file: &Path) -> bool {
    match lock_file(file, false) {
        Ok(None) => true,
        Ok(Some(handle)) => {
            unlock(handle);
            false
        }
        Err(_) => false,
    }
}
/// Whether the open `handle` is still the file at `path`, not one removed since (and maybe made again); None when that
/// can't be told: the file system keeps no ids, or they can't be read.
fn is_at(handle: &std::fs::File, path: &Path) -> Option<bool> {
    let there = match std::fs::File::open(path) {
        Ok(there) => there,
        // Gone, or (on Windows) going: removed while something still has it open.
        Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied) => return Some(false),
        Err(_) => return None,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (held, there) = (handle.metadata().ok()?, there.metadata().ok()?);
        Some((held.dev(), held.ino()) == (there.dev(), there.ino()))
    }
    #[cfg(windows)]
    {
        Some(file_id::of(handle)? == file_id::of(&there)?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (handle, there);
        None
    }
}
/// A file's volume and index there, which no other file on that volume has while it's there. Windows reuses a removed
/// file's creation time for one made under its name soon after, so the time can't tell them apart.
#[cfg(windows)]
mod file_id {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(file: *mut std::ffi::c_void, information: *mut [u32; 13]) -> i32;
    }
    pub fn of(file: &std::fs::File) -> Option<(u32, u64)> {
        // BY_HANDLE_FILE_INFORMATION: thirteen DWORDs, the volume's serial number the eighth, the index the last two.
        let mut information = [0u32; 13];
        // SAFETY: the handle is the open file's, and the buffer is the size of all the call writes.
        let read = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
        (read != 0).then(|| (information[7], (u64::from(information[11]) << 32) | u64::from(information[12])))
    }
}
/// When a process started, on Linux, counted from the boot it runs in: setting the clock moves neither.
#[cfg(target_os = "linux")]
mod since_boot {
    /// This boot's id, new each time the system starts.
    pub fn boot() -> Option<String> {
        let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        Some(id.trim().to_owned()).filter(|id| !id.is_empty())
    }
    /// When process `pid` started, in clock ticks after boot: /proc/<pid>/stat's field 22, counted after the command's
    /// closing parenthesis (which the command itself may contain). None when it isn't running, or has ended and is
    /// waiting to be reaped.
    pub fn ticks(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
        if fields.next()? == "Z" {
            return None;
        }
        fields.nth(18)?.parse().ok()
    }
}
#[cfg(not(target_os = "linux"))]
mod since_boot {
    pub fn boot() -> Option<String> {
        None
    }
    pub fn ticks(_: u32) -> Option<u64> {
        None
    }
}
/// Where the file system can't lock, whether this Kumi named itself in a conversation's lock file.
enum Named {
    Mine,
    /// Another Kumi that's still running named itself there, or is naming itself.
    Taken,
    /// The file can be neither made nor written (a folder there, say).
    Neither,
}
/// Names this Kumi in a conversation's lock `file`, made only where there's none: of two Kumis naming themselves at
/// once, one makes it and the other finds it. One naming a Kumi that's gone, this one, or nobody, is made again; one
/// with nothing in it yet, made a moment ago, is another Kumi's being written. Each reads its file back once it's
/// written; two Kumis replacing, at the same instant, one that a Kumi that's gone left behind can still both have it, when
/// each reads its own back before the other removes it: only the system's lock tells them apart then.
fn name_myself(file: &Path) -> Named {
    for again in [false, true] {
        match make_named(file) {
            // Read back: another Kumi replacing the file at the same instant may have put its own name there.
            Ok(()) if read_lock(file).is_some_and(|lock| lock.other()) => return Named::Taken,
            Ok(()) => return Named::Mine,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Named::Neither,
        }
        match read_lock(file) {
            Some(lock) if lock.other() => return Named::Taken,
            None if being_written(file) => return Named::Taken,
            _ if again || std::fs::remove_file(file).is_err() => return Named::Neither,
            _ => {}
        }
    }
    Named::Neither
}
/// A lock file made new, naming this Kumi; an error when there's one already, or it can't be made or written.
fn make_named(file: &Path) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(folder) = file.parent() {
        kumi_types::private::create_dir_all(folder)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut handle = options.open(file)?;
    let written = handle.write_all(stringify(&json!(Lock::mine())).as_bytes());
    if written.is_err() {
        drop(handle);
        let _ = std::fs::remove_file(file);
    }
    written
}
/// A lock file with nothing in it yet, made a moment ago: another Kumi between making it and naming itself in it. A time
/// ahead of this computer's clock counts as a moment ago: on a network drive it's the server's clock.
fn being_written(file: &Path) -> bool {
    std::fs::metadata(file).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.len() == 0
            && metadata.modified().is_ok_and(|at| at.elapsed().map_or(true, |age| age < std::time::Duration::from_secs(2)))
    })
}

/// The files of a conversation as last read: when `<id>.json` was written (ns) and its size, and when `<id>.meta` was (0
/// without one) and its size, with what of them the entry holds ([`READS`]). Anything else, and it's read again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    modified: u64,
    size: u64,
    meta: u64,
    /// A name and a pin written in the same second have the same time where the file system keeps seconds (HFS+, exFAT,
    /// SMB), but not the same size.
    #[serde(default, rename = "metaSize", skip_serializing_if = "is_zero")]
    meta_size: u64,
    /// An entry an earlier Kumi wrote back has none (it doesn't know them), so it's read again.
    #[serde(default)]
    reads: u32,
    /// Read within a tick ([`TICK_NS`]) of a write to either file: another write in that tick could leave the same times
    /// and sizes (a name changed for one as long), so it's read again at the next look.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    recent: bool,
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}
impl Stamp {
    /// A conversation's files as they are now: its `<id>.json`, and its `<id>.meta` when it has one.
    fn of(json: &std::fs::Metadata, meta: Option<&std::fs::Metadata>) -> Stamp {
        let (modified, meta_modified) = (stamp_of(json), meta.map_or(0, stamp_of));
        let now = stamp_of_time(std::time::SystemTime::now());
        let near = |at: u64| at != 0 && now.abs_diff(at) < TICK_NS;
        Stamp {
            modified,
            size: json.len(),
            meta: meta_modified,
            meta_size: meta.map_or(0, |meta| meta.len()),
            reads: READS,
            recent: near(modified) || near(meta_modified),
        }
    }
    /// Whether an entry stamped so holds what the files `found` hold now: they're as they were when it was read, it was
    /// read by this Kumi or a later one that holds more of them, and not just after a write.
    fn holds(&self, found: &Stamp) -> bool {
        !self.recent
            && self.reads >= found.reads
            && (self.modified, self.size, self.meta, self.meta_size) == (found.modified, found.size, found.meta, found.meta_size)
    }
    /// Whether `<id>.json` is as it was when this was its stamp: one that didn't read as a conversation reads no better.
    fn same_file(&self, found: &Stamp) -> bool {
        !self.recent && (self.modified, self.size) == (found.modified, found.size)
    }
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
    /// What a later Kumi keeps of it, kept as it is while its files don't change. Read again from changed files, it
    /// goes (it was of the files as they were), and that Kumi reads them again: this one's `reads` is fewer than its own.
    #[serde(flatten)]
    later: serde_json::Map<String, Value>,
}
/// A place's Set name, as its baseline said when Kumi read it, with when the baseline was written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PlaceName {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    modified: u64,
    /// What a later Kumi keeps of it, kept as it is until the baseline changes.
    #[serde(flatten)]
    later: serde_json::Map<String, Value>,
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
    /// Files named like a conversation that don't read as one, by "<place>/<id>", as they were when read: not read again
    /// until they change, and their ids not given to new conversations.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    unreadable: BTreeMap<String, Stamp>,
    /// What a later Kumi of this version keeps in it, kept as it is when this one writes it again.
    #[serde(flatten)]
    later: serde_json::Map<String, Value>,
}
/// What an index file holds, as this Kumi reads it.
enum IndexFile {
    Ours(Index),
    /// Nothing this Kumi reads: none there, or not an index.
    None,
    /// A later version's, and the days it says settings said to keep conversations.
    Later(Option<u32>),
}
fn stamp_of(metadata: &std::fs::Metadata) -> u64 {
    metadata.modified().ok().map(stamp_of_time).unwrap_or(0)
}
fn stamp_of_time(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}
/// Of two numbers of days settings said to keep conversations, the one that keeps more (0: every one).
fn keeps_longer(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(0), _) | (_, Some(0)) => Some(0),
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
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
        Self::parse(&tokio::fs::read(file).await.ok()?)
    }
    /// A conversation from a file's bytes; None when they aren't one.
    fn parse(bytes: &[u8]) -> Option<SavedConversation> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        if !value["savedAt"].is_number()
            || value["checkpoint"]["version"].as_f64() != Some(1.0)
            || !value["checkpoint"]["messages"].is_array()
        {
            return None;
        }
        // What's kept is taken out of what was read, not copied from it: the messages are most of a file of up to 256 KB.
        let Value::Object(mut read) = value else { return None };
        let mut kept = serde_json::Map::new();
        for key in ["savedAt", "checkpoint", "changes", "first", "turns"] {
            let Some(field) = read.remove(key) else { continue };
            let allowed = match key {
                "changes" => field.is_array(),
                "first" => field.is_string(),
                "turns" => field.is_number(),
                _ => true,
            };
            if allowed {
                kept.insert(key.into(), field);
            }
        }
        serde_json::from_value(Value::Object(kept)).ok()
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
        let now = kumi_common::time::now_ms() as f64;
        // Within the count every place keeps there is nothing beyond it to let go, so no file is looked at: this runs
        // after every save.
        let names = self.names(place).await;
        if names.len() > MAX_KEPT {
            let mut all = vec![];
            for name in names {
                let at = modified_ms(&kept.join(&name)).await;
                all.push((name, at));
            }
            // Newest first, and the same order an earlier Kumi gave them, ties and all.
            all.sort_by(|a, b| b.1.total_cmp(&a.1));
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
                // A log that can't go now goes with a later look, as one left behind.
                let _ = remove(kept.join(log_name(id))).await;
                // Only a place to lock: one that can't go (a folder there, say) stays.
                self.remove_lock(&kept.join(format!("{id}.lock")));
            }
        }
        self.sweep(place).await?;
        Ok(())
    }
    /// What's been left a while beside no conversation in `place`: a name, pin or log whose conversation went follows it
    /// to the place that keeps it (an earlier Kumi moved it to its Set's place once the Set was saved), or goes (one let
    /// it go); a lock no Kumi holds goes. A log a Kumi still has open stays: it's written beside its conversation, which
    /// follows it there. True when anything moved or went.
    async fn sweep(&self, place: &str) -> Result<bool, RuntimeError> {
        let kept = self.kept(place)?;
        let now = kumi_common::time::now_ms() as f64;
        let mut swept = false;
        let Ok(mut dir) = tokio::fs::read_dir(&kept).await else { return Ok(false) };
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((id, kind)) = beside(&name) else { continue };
            if kept.join(format!("{id}.json")).exists() || now - modified_ms(&entry.path()).await < ORPHAN_MS {
                continue;
            }
            let held = self.held.borrow().contains_key(&(place.to_owned(), id.to_owned()));
            match kind {
                // Its conversation not kept yet, a lock may be one a Kumi holds for a new one: it goes only when none does.
                Beside::Lock => {
                    swept |= !held && self.remove_lock(&entry.path());
                    continue;
                }
                Beside::Log if held || self.open_elsewhere(place, id) => continue,
                _ => {}
            }
            match self.kept_elsewhere(place, id).await {
                Some(there) if !there.join(&name).exists() => {
                    swept |= tokio::fs::rename(entry.path(), there.join(&name)).await.is_ok();
                }
                _ if kind == Beside::Log => swept |= remove(entry.path()).await.is_ok(),
                _ => {
                    remove(entry.path()).await?;
                    swept = true;
                }
            }
        }
        Ok(swept)
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
    /// named this Kumi goes, and so does the file of a conversation that isn't kept (a copy taken up only to look, say):
    /// no other Kumi knows its id.
    fn let_go(&self, place: &str, id: &str, hold: Hold) {
        match hold {
            Hold::Locked(mut handle) => {
                let _ = say(&mut handle, None);
                // Removed while it's held: a Kumi that opened it a moment ago finds it gone once it locks it.
                if let Ok(kept) = self.kept(place) {
                    if !kept.join(format!("{id}.json")).exists() {
                        let _ = std::fs::remove_file(kept.join(format!("{id}.lock")));
                    }
                }
                unlock(handle);
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
    /// Removes a conversation's lock `file` when no Kumi holds it, holding it meanwhile: a Kumi asking for it at that
    /// instant finds the file it opened gone, and holds the one it makes. True when it's gone; one held stays, and so
    /// does one that can't go (a folder there, say).
    fn remove_lock(&self, file: &Path) -> bool {
        if self.file_locks.get() {
            match lock_file(file, false) {
                Ok(Some(mut handle)) => {
                    // Another Kumi that couldn't lock named itself in it, and still runs.
                    if said(&mut handle).is_some_and(|lock| lock.other()) {
                        unlock(handle);
                        return false;
                    }
                    let removed = std::fs::remove_file(file).is_ok();
                    unlock(handle);
                    return removed;
                }
                Ok(None) => return false,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
                // The file system can't lock: as where it can't.
                Err(_) => {}
            }
        }
        if read_lock(file).is_some_and(|lock| lock.other()) {
            return false;
        }
        std::fs::remove_file(file).is_ok() || !file.exists()
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
    /// A kept conversation's log, as far as it reads; None without one this Kumi reads.
    pub async fn log(&self, place: &str, id: &str) -> Option<ReadLog> {
        if !CONVERSATION.is_match(id) {
            return None;
        }
        read_log_file(&self.kept(place).ok()?.join(log_name(id))).await
    }
    fn place_of(&self, key: &str) -> Option<(String, String)> {
        key.split_once('/').map(|(place, id)| (place.to_owned(), id.to_owned()))
    }
    /// Every place's conversation files as they are now ("<place>/<id>" → stamp), and the places where something's been
    /// left a while beside no conversation: `sweep` sees to it. A lock a Kumi holds isn't left (one held a while for a
    /// conversation not kept yet would have every look sweep that place again).
    async fn scan(&self) -> (BTreeMap<String, Stamp>, Vec<String>) {
        let mut found = BTreeMap::new();
        let mut left = vec![];
        let Ok(mut places) = tokio::fs::read_dir(&self.directory).await else { return (found, left) };
        while let Ok(Some(place)) = places.next_entry().await {
            let Some(place) = place.file_name().to_str().filter(|p| self.places.parse(p).is_some()).map(str::to_owned) else { continue };
            let Ok(kept) = self.kept(&place) else { continue };
            let Ok(mut dir) = tokio::fs::read_dir(&kept).await else { continue };
            let mut metas = HashMap::new();
            let mut files = vec![];
            let mut beside_a_while = vec![];
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(metadata) = entry.metadata().await else { continue };
                if let Some(id) = name.strip_suffix(".json").filter(|id| CONVERSATION.is_match(id)) {
                    files.push((id.to_owned(), metadata));
                } else if let Some((id, kind)) = beside(&name) {
                    let age = metadata.modified().ok().and_then(|at| at.elapsed().ok());
                    if age.is_some_and(|age| age.as_secs_f64() * 1000.0 >= ORPHAN_MS) {
                        beside_a_while.push((id.to_owned(), kind));
                    }
                    if kind == Beside::Meta {
                        metas.insert(id.to_owned(), metadata);
                    }
                }
            }
            let ids: HashSet<&str> = files.iter().map(|(id, _)| id.as_str()).collect();
            let held = |id: &str| self.held.borrow().contains_key(&(place.clone(), id.to_owned())) || self.open_elsewhere(&place, id);
            if beside_a_while.iter().any(|(id, kind)| !ids.contains(id.as_str()) && (*kind != Beside::Lock || !held(id))) {
                left.push(place.clone());
            }
            for (id, json) in &files {
                found.insert(format!("{place}/{id}"), Stamp::of(json, metas.get(id)));
            }
        }
        (found, left)
    }
    /// The index as kept, and the file this Kumi writes it to: `conversation-index.json`, or, when a later Kumi wrote a
    /// version this one doesn't know there, this version's own beside it (that one is never written over). None when
    /// both hold a later version: this one then makes its own from the files each time, and writes it nowhere.
    async fn read_index(&self) -> (Index, Option<&'static str>) {
        let fresh = Index { version: INDEX_VERSION, ..Index::default() };
        match self.read_index_file(INDEX).await {
            IndexFile::Ours(index) => (index, Some(INDEX)),
            IndexFile::None => (fresh, Some(INDEX)),
            IndexFile::Later(said) => {
                let (index, file) = match self.read_index_file(INDEX_BESIDE).await {
                    IndexFile::Ours(index) => (index, Some(INDEX_BESIDE)),
                    IndexFile::None => (fresh, Some(INDEX_BESIDE)),
                    IndexFile::Later(_) => (fresh, None),
                };
                // What settings said, as either knew it last.
                (Index { keep_days: keeps_longer(index.keep_days, said), ..index }, file)
            }
        }
    }
    async fn read_index_file(&self, name: &str) -> IndexFile {
        let Ok(bytes) = tokio::fs::read(self.directory.join(name)).await else { return IndexFile::None };
        #[derive(Deserialize)]
        struct Version {
            #[serde(default)]
            version: u64,
        }
        match serde_json::from_slice::<Version>(&bytes).map(|written| written.version) {
            Ok(version) if version == u64::from(INDEX_VERSION) => {
                serde_json::from_slice::<Index>(&bytes).map(IndexFile::Ours).unwrap_or(IndexFile::None)
            }
            Ok(version) if version > u64::from(INDEX_VERSION) => {
                let said = serde_json::from_slice::<Value>(&bytes).ok().and_then(|index| index["keepDays"].as_u64());
                IndexFile::Later(said.and_then(|days| u32::try_from(days).ok()))
            }
            _ => IndexFile::None,
        }
    }
    async fn write_index(&self, file: &str, index: &Index) {
        let _ = write_privately(&self.directory, file, &stringify(&serde_json::to_value(index).unwrap_or_default())).await;
    }
    /// The index, read again from the files wherever they changed (made from them all when it's missing or unreadable),
    /// and written back when anything did.
    async fn index(&self) -> Index {
        let _one = self.looking.lock().await;
        let (before, file) = self.read_index().await;
        let mut index = before.clone();
        index.keep_days = self.keep_days.or(before.keep_days);
        let (mut files, left) = self.scan().await;
        // What's been left beside no conversation follows it or goes at a look, not only when its place saves again: an
        // earlier Kumi that moved a conversation to its Set's place left its name and pin in a place that may not.
        let mut swept = false;
        for place in &left {
            swept |= matches!(self.sweep(place).await, Ok(true));
        }
        if swept {
            files = self.scan().await.0;
        }
        let kept_ids: HashSet<&str> = files.keys().filter_map(|key| key.split_once('/').map(|(_, id)| id)).collect();
        let now = kumi_common::time::now_ms();
        for key in index.conversations.keys().filter(|key| !files.contains_key(*key)).cloned().collect::<Vec<_>>() {
            index.conversations.remove(&key);
            if let Some((_, id)) = self.place_of(&key).filter(|(_, id)| !kept_ids.contains(id.as_str())) {
                index.gone.insert(id, now);
            }
        }
        index.unreadable.retain(|key, _| files.contains_key(key));
        for (key, stamp) in &files {
            if index.conversations.get(key).is_some_and(|entry| entry.stamp.holds(stamp))
                || index.unreadable.get(key).is_some_and(|was| was.same_file(stamp))
            {
                continue;
            }
            let Some((place, id)) = self.place_of(key) else { continue };
            let Ok(kept) = self.kept(&place) else { continue };
            // Reading many files: the app keeps drawing between them.
            tokio::task::yield_now().await;
            // Not read this time (a backup or a scanner holding it, say): listed as it was last read, if it was, and read
            // again at the next look. Only one whose bytes aren't a conversation waits for the file to change.
            let Ok(bytes) = tokio::fs::read(kept.join(format!("{id}.json"))).await else { continue };
            let Some(value) = Self::parse(&bytes) else {
                index.conversations.remove(key);
                index.unreadable.insert(key.clone(), *stamp);
                continue;
            };
            index.unreadable.remove(key);
            let meta = read_meta(&kept.join(format!("{id}.meta"))).await;
            let said: Vec<_> = transcript_of(&value.checkpoint.messages).into_iter().filter(|l| l.role == TranscriptRole::User).collect();
            index.conversations.insert(
                key.clone(),
                Entry {
                    saved_at: value.saved_at,
                    first: value.first.unwrap_or_else(|| said.first().map(|l| cut(&l.text, 200).to_owned()).unwrap_or_default()),
                    turns: value.turns.unwrap_or(said.len() as u32),
                    created: created_of(&id, value.saved_at),
                    name: meta.name,
                    pinned: meta.pinned,
                    forked_from: meta.forked_from,
                    imported: meta.imported,
                    set: meta.set,
                    stamp: *stamp,
                    later: serde_json::Map::new(),
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
            index.places.insert(place, PlaceName { name, modified, later: serde_json::Map::new() });
        }
        if let Some(file) = file.filter(|_| index != before && !(files.is_empty() && index.gone.is_empty())) {
            self.write_index(file, &index).await;
        }
        index
    }
    /// The index with this conversation as it was just saved: its entry from what was written (one stat of its file
    /// and one of its meta), without reading every place's files again; `index` does that when the list is asked for.
    async fn note_saved(&self, place: &str, id: &str, saved: &SavedConversation) {
        let _one = self.looking.lock().await;
        let (before, file) = self.read_index().await;
        let Some(file) = file else { return };
        let Ok(kept) = self.kept(place) else { return };
        let Ok(metadata) = tokio::fs::metadata(kept.join(format!("{id}.json"))).await else { return };
        let meta_file = kept.join(format!("{id}.meta"));
        let meta_metadata = tokio::fs::metadata(&meta_file).await.ok();
        let meta = read_meta(&meta_file).await;
        let mut index = before.clone();
        index.keep_days = self.keep_days.or(before.keep_days);
        let key = format!("{place}/{id}");
        index.unreadable.remove(&key);
        index.conversations.insert(
            key,
            Entry {
                saved_at: saved.saved_at,
                first: saved.first.clone().unwrap_or_default(),
                turns: saved.turns.unwrap_or(0),
                created: created_of(id, saved.saved_at),
                name: meta.name,
                pinned: meta.pinned,
                forked_from: meta.forked_from,
                imported: meta.imported,
                set: meta.set,
                stamp: Stamp::of(&metadata, meta_metadata.as_ref()),
                later: serde_json::Map::new(),
            },
        );
        index.gone.remove(id);
        // The place's Set name, as `index` keeps it: read again when its baseline was written since.
        let modified = match self.folder(place) {
            Ok(folder) => tokio::fs::metadata(folder.join("last-seen.json")).await.map(|m| stamp_of(&m)).unwrap_or(0),
            Err(_) => 0,
        };
        if !index.places.get(place).is_some_and(|known| known.modified == modified) {
            let name = self.set_name(place).await;
            index.places.insert(place.to_owned(), PlaceName { name, modified, later: serde_json::Map::new() });
        }
        if index != before {
            self.write_index(file, &index).await;
        }
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
        rows.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
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
fn without_pictures(message: &Value) -> Cow<'_, Value> {
    let Some(parts) = message["content"].as_array().filter(|_| message["role"] == "user") else { return Cow::Borrowed(message) };
    if !parts.iter().any(|part| part["type"] == "file") {
        return Cow::Borrowed(message);
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
    Cow::Owned(kept)
}
// Disk checkpoints also contain older string-form messages; keep those forms intact.
fn bounded_messages(all: &[Value]) -> Vec<Value> {
    let sizes: Vec<_> = all.iter().map(|m| json_bytes(m) + 1).collect();
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
            // The last exchange as an array: its messages' bytes, the commas between them and the brackets.
            if sizes[last..].iter().sum::<usize>() + 1 <= 4 * MAX_CONVERSATION_BYTES {
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
        // Pictures are rare: the messages are copied only from the first that is kept without one.
        let originals = &conversation.checkpoint.messages;
        let mut rewritten: Option<Vec<Value>> = None;
        for (index, message) in originals.iter().enumerate() {
            match without_pictures(message) {
                Cow::Owned(kept) => rewritten.get_or_insert_with(|| originals[..index].to_vec()).push(kept),
                Cow::Borrowed(_) => {
                    if let Some(all) = rewritten.as_mut() {
                        all.push(message.clone());
                    }
                }
            }
        }
        let all: &[Value] = rewritten.as_deref().unwrap_or(originals);
        let messages = bounded_messages(all);
        if messages.is_empty() {
            return Ok(());
        }
        let said: Vec<_> = transcript_of(all).into_iter().filter(|l| l.role == TranscriptRole::User).collect();
        let checkpoint = KernelCheckpoint {
            version: conversation.checkpoint.version,
            messages,
            origin: conversation.checkpoint.origin.clone(),
            tools: conversation.checkpoint.tools.clone(),
        };
        let saved = SavedConversation {
            saved_at: conversation.saved_at,
            checkpoint,
            changes: conversation
                .changes
                .as_ref()
                .filter(|c| !c.is_empty())
                .map(|c| c[c.len().saturating_sub(MAX_KEPT_CHANGES)..].to_vec()),
            first: Some(cut(conversation.first.as_deref().or_else(|| said.first().map(|l| l.text.as_str())).unwrap_or(""), 200).to_owned()),
            turns: Some(conversation.turns.unwrap_or(0).max(said.len() as u32)),
        };
        self.file(place, id)?;
        write_privately(&self.kept(place)?, &format!("{id}.json"), &stringify(&serde_json::to_value(&saved).map_err(error)?)).await?;
        write_privately(&self.folder(place)?, "current", id).await?;
        self.trim(place).await?;
        self.note_saved(place, id, &saved).await;
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
        // Its lock there was only a place to lock it.
        self.remove_lock(&self.kept(from)?.join(format!("{id}.lock")));
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
        // A file named like a conversation that doesn't read as one keeps its id from new ones too.
        let taken: HashSet<String> = index
            .conversations
            .keys()
            .chain(index.unreadable.keys())
            .filter_map(|key| self.place_of(key).map(|(_, id)| id))
            .chain(index.gone.keys().cloned())
            .collect();
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
            return Err(kumi_types::errors::KumiError::new(
                kumi_types::errors::FailureKind::Request,
                "That conversation is open in another Kumi window: close it there, then delete it.",
            )
            .into());
        }
        let kept = self.kept(place)?;
        remove(&file).await?;
        remove(kept.join(format!("{id}.meta"))).await?;
        let _ = remove(kept.join(log_name(id))).await;
        // One this Kumi has open keeps its lock till it lets go: the lock goes then, its conversation not kept.
        if !self.held.borrow().contains_key(&(place.to_owned(), id.to_owned())) {
            self.remove_lock(&kept.join(format!("{id}.lock")));
        }
        if self.current_id(place).await.as_deref() == Some(id) {
            write_privately(&self.folder(place)?, "current", "").await?
        }
        self.index().await;
        // Deleted by the producer, not let go: the id is unknown from here on.
        let _one = self.looking.lock().await;
        let (mut index, file) = self.read_index().await;
        if let Some(file) = file.filter(|_| index.gone.remove(id).is_some()) {
            self.write_index(file, &index).await;
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
    /// it, as long as that Kumi runs: it's made only where there's none, so of two naming themselves at once one does.
    /// Kumi never refuses a conversation: when neither can be had, it goes on, held by nobody, as before Kumi 1.17.
    fn claim(&self, place: &str, id: &str) -> Claim {
        let (Ok(kept), true) = (self.kept(place), CONVERSATION.is_match(id)) else { return Claim::Ours };
        let key = (place.to_owned(), id.to_owned());
        if self.held.borrow().contains_key(&key) {
            return Claim::Ours;
        }
        let file = kept.join(format!("{id}.lock"));
        let locked = if self.file_locks.get() { take_lock(&file) } else { Err(std::io::ErrorKind::Unsupported.into()) };
        let hold = match locked {
            Ok(None) => return Claim::Taken,
            Ok(Some(mut handle)) => {
                // Another Kumi that couldn't lock (from another computer, say) named itself here, and still runs.
                if said(&mut handle).is_some_and(|lock| lock.other()) {
                    unlock(handle);
                    return Claim::Taken;
                }
                let _ = say(&mut handle, Some(&Lock::mine()));
                Hold::Locked(handle)
            }
            Err(_) => match name_myself(&file) {
                Named::Mine => Hold::Named,
                Named::Taken => return Claim::Taken,
                Named::Neither => return Claim::Ours,
            },
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

#[cfg(test)]
mod tests {
    use super::{is_at, keeps_longer, lock_file, take_lock_with};

    /// A lock file removed and made again under its name is another file to a handle on the first: Windows gives the new
    /// one the old one's creation time, so only their ids tell them apart.
    #[test]
    fn a_lock_file_made_again_is_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mv66me0m3fa91c.lock");
        let first = lock_file(&file, true).unwrap().expect("nobody else holds it");
        assert_eq!(is_at(&first, &file), Some(true));
        // Removed while held, as a Kumi letting go of it does.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(is_at(&first, &file), Some(false), "removed");
        let again = lock_file(&file, true).unwrap().expect("a new file, nobody's");
        assert_eq!((is_at(&again, &file), is_at(&first, &file)), (Some(true), Some(false)), "made again");
    }

    /// A claim that locks a file removed (and made again) since it opened it locks the one there now; one that finds
    /// the file it locked removed at every try is another Kumi's to have.
    #[test]
    fn a_claim_holds_the_lock_file_thats_there_not_one_removed_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mv66me0m3fa91c.lock");
        // The first try opens and locks a file that a Kumi letting go removes meanwhile, and another Kumi's claim makes
        // again.
        let mut tries = 0;
        let mut stale = vec![];
        let held = take_lock_with(&file, |file| {
            tries += 1;
            let handle = lock_file(file, true);
            if tries == 1 {
                std::fs::remove_file(file).unwrap();
                stale.push(std::fs::File::create(file).unwrap());
            }
            handle
        })
        .unwrap()
        .expect("the file there now");
        assert_eq!((tries, is_at(&held, &file)), (2, Some(true)));
        drop((held, stale));
        // Removed at every try: taken.
        let mut tries = 0;
        let taken = take_lock_with(&file, |file| {
            tries += 1;
            let handle = lock_file(file, true);
            std::fs::remove_file(file).unwrap();
            handle
        })
        .unwrap();
        assert!(taken.is_none() && tries == 4, "{tries}");
    }

    #[test]
    fn of_two_numbers_of_days_to_keep_the_longer_counts_and_none_says_nothing() {
        assert_eq!(keeps_longer(Some(30), Some(120)), Some(120));
        assert_eq!(keeps_longer(Some(0), Some(120)), Some(0), "0 keeps every one");
        assert_eq!(keeps_longer(None, Some(7)), Some(7));
        assert_eq!(keeps_longer(None, None), None);
    }
}
