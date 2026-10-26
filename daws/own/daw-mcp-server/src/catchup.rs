//! Catch-up and watch_me in the host: the project's digest read from the bridge (`digest`, a page at a time), kept on
//! disk per place between sessions, and compared (`semantic::diff`).
//!
//! - `catch_up`: what changed since the place's last snapshot (edits made while Kumi was closed, a DAW restart), and the
//!   snapshot taken again.
//! - `baseline_save`: the snapshot taken again (after Kumi's changes, as the producer works), never before this host
//!   compared the open project with the snapshot it has, so what changed while Kumi was away is never lost.
//! - `watch_start` / `watch_stop`: what the producer did meanwhile, with the parameters they moved; what they add is read
//!   again while watching, so its parameters are told against how it came, not against nothing.
//!
//! Snapshots live in `<Kumi's folder>/bridges/<daw>/snapshots/`: a saved project's under its place (`<place>/last-seen.json`,
//! with the other files of the same song in `sets/`), an unsaved one's under the identity this run of the DAW gives it.
use crate::client::{BridgeClient, RequestError};
use crate::host::{CallError, Following, Host, Pick};
use crate::semantic::{self, Change, Detail, Diff, Record, Snapshot, SNAPSHOT_VERSION};
use daw_bridge::Daw;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A digest page waits this long: the bridge answers a busy DAW at once.
const DIGEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The most pages and records one digest reads, and the biggest snapshot kept.
const MAX_PAGES: usize = 400;
const MAX_RECORDS: usize = 250_000;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
/// A digest whose pages came from different epochs (another project in the tab meanwhile) is read again, this often.
const DIGEST_TRIES: usize = 3;
/// The other files of one song a place keeps a snapshot of, and the unsaved projects kept.
const MAX_SETS: usize = 20;
const MAX_UNSAVED: usize = 8;
/// While watching, what's in the project is looked at again this often (at least, and at most): ten times as long as a
/// look takes.
const POLL_MIN: Duration = Duration::from_secs(1);
const POLL_MAX: Duration = Duration::from_secs(10);
/// The most things watch_me reads the parameters of, and the parameters it tells of one.
const MOST_READ: usize = 12;
const MOST_PARAMETERS: usize = 24;
/// The selections watch_me keeps.
const MOST_SELECTIONS: usize = 32;

/// Where a project's snapshot is kept: a saved project by its place and file, an unsaved one by the identity this run of
/// the DAW gives it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Saved { place: String, path: String },
    Unsaved { identity: String },
}

impl Key {
    /// The place as the answers say it.
    pub fn place(&self, daw: Daw) -> String {
        match self {
            Key::Saved { place, .. } => place.clone(),
            Key::Unsaved { .. } => format!("{}-unsaved", daw.id()),
        }
    }
}

/// The project a digest's first page names, as a key.
pub fn key_of(daw: Daw, project: &Value) -> Key {
    let id = project.get("id").and_then(Value::as_str).filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()));
    let path = project.get("path").and_then(Value::as_str).filter(|path| !path.is_empty());
    match (id, path) {
        (Some(id), Some(path)) => Key::Saved { place: format!("{}-{}", daw.id(), id.to_ascii_lowercase()), path: path.into() },
        _ => Key::Unsaved { identity: project.get("identity").and_then(Value::as_str).unwrap_or("").into() },
    }
}

/// A short, stable name for a path or an identity in a file name.
fn file_key(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Whether two paths name the same file: a case-only rename, or a link, is the same one.
fn same_file(a: &str, b: &str) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// Whether the project seen at `last` is the one at `path` now: the same file, or moved from there.
fn moved_to(last: &str, path: &str) -> bool {
    last == path || !Path::new(last).exists() || same_file(last, path)
}

/// The snapshots on disk.
pub struct Store {
    root: PathBuf,
}

/// What of a kept snapshot `save` and `baseline_save` read: its form, the file it's of and its bounds, not its records (a
/// snapshot is parsed whole only to be compared).
#[derive(Deserialize)]
struct Head {
    version: u32,
    #[serde(default)]
    path: String,
    #[serde(default)]
    bounded: Map<String, Value>,
}

/// A kept snapshot read from its file: whole, or its head.
trait Kept: DeserializeOwned {
    fn version(&self) -> u32;
    fn path(&self) -> &str;
}

impl Kept for Snapshot {
    fn version(&self) -> u32 {
        self.version
    }
    fn path(&self) -> &str {
        &self.path
    }
}

impl Kept for Head {
    fn version(&self) -> u32 {
        self.version
    }
    fn path(&self) -> &str {
        &self.path
    }
}

fn read_snapshot<T: Kept>(file: &Path) -> Option<T> {
    let meta = std::fs::metadata(file).ok()?;
    if meta.len() as usize > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let snapshot: T = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    (snapshot.version() == SNAPSHOT_VERSION).then_some(snapshot)
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Written aside and renamed into place, readable by the owner only: a reader (another host, this one after a crash) finds
/// the old file or the new one whole. Not flushed to the drive, though: after a power cut the file can be empty or cut
/// short, which reads as no snapshot (catch-up starts afresh, the next save writes it anew). A snapshot is a cache, and a
/// flush on each save, every few seconds while the producer edits (macOS's F_FULLFSYNC waits for the drive's whole cache),
/// would cost more than the save.
fn write_private(dir: &Path, name: &str, text: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    private_dir(dir)?;
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(text)?;
        std::fs::rename(&temporary, dir.join(name))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Keeps the `keep` newest `.json` files of a folder.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .map(|entry| (entry.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), entry.path()))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, file) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(file);
    }
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn place_dir(&self, place: &str) -> PathBuf {
        self.root.join(place)
    }

    fn unsaved_dir(&self) -> PathBuf {
        self.root.join("unsaved")
    }

    /// The snapshot to compare the project with: a saved project's own file's, else its place's latest when the project
    /// moved there (a version first seen here has none); an unsaved one's by its identity.
    pub fn load(&self, key: &Key) -> Option<Snapshot> {
        self.load_kept(key)
    }

    /// The head of the snapshot `load` gives.
    fn load_head(&self, key: &Key) -> Option<Head> {
        self.load_kept(key)
    }

    fn load_kept<T: Kept>(&self, key: &Key) -> Option<T> {
        match key {
            Key::Unsaved { identity } => read_snapshot(&self.unsaved_dir().join(format!("{}.json", file_key(identity)))),
            Key::Saved { place, path } => {
                let dir = self.place_dir(place);
                let last: Option<T> = read_snapshot(&dir.join("last-seen.json"));
                if last.as_ref().is_some_and(|last| last.path() == path) {
                    return last;
                }
                if let Some(own) =
                    read_snapshot::<T>(&dir.join("sets").join(format!("{}.json", file_key(path)))).filter(|own| own.path() == path)
                {
                    return Some(own);
                }
                last.filter(|last| moved_to(last.path(), path))
            }
        }
    }

    /// The unsaved project's snapshot by its identity (it may have been saved since).
    pub fn load_unsaved(&self, identity: &str) -> Option<Snapshot> {
        self.load(&Key::Unsaved { identity: identity.into() })
    }

    pub fn forget_unsaved(&self, identity: &str) {
        let _ = std::fs::remove_file(self.unsaved_dir().join(format!("{}.json", file_key(identity))));
    }

    /// Keeps the snapshot as the project's latest. Another file of the song seen before, and still there, keeps its own.
    /// Not when it's too big to keep (it says so, with its size).
    pub fn save(&self, key: &Key, snapshot: &Snapshot) -> Result<Saved, String> {
        let text = serde_json::to_vec(snapshot).map_err(|e| e.to_string())?;
        if text.len() > MAX_SNAPSHOT_BYTES {
            return Ok(Saved::TooBig(text.len()));
        }
        let failed = |e: std::io::Error| format!("Kumi couldn't keep the project's snapshot: {e}");
        match key {
            Key::Unsaved { identity } => {
                let dir = self.unsaved_dir();
                write_private(&dir, &format!("{}.json", file_key(identity)), &text).map_err(failed)?;
                prune(&dir, MAX_UNSAVED);
            }
            Key::Saved { place, path } => {
                let dir = self.place_dir(place);
                let sets = dir.join("sets");
                if let Some(before) = read_snapshot::<Head>(&dir.join("last-seen.json")) {
                    if !moved_to(&before.path, path) {
                        private_dir(&sets).map_err(failed)?;
                        std::fs::rename(dir.join("last-seen.json"), sets.join(format!("{}.json", file_key(&before.path))))
                            .map_err(failed)?;
                        prune(&sets, MAX_SETS);
                    }
                }
                write_private(&dir, "last-seen.json", &text).map_err(failed)?;
                let _ = std::fs::remove_file(sets.join(format!("{}.json", file_key(path))));
            }
        }
        Ok(Saved::Kept)
    }
}

/// What became of a snapshot to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Kept,
    /// Over what Kumi keeps (its size in bytes): catch-up has nothing to compare with next time.
    TooBig(usize),
    /// Cut short (more than a digest reads): the whole snapshot kept before stays.
    Partial,
}

impl Saved {
    /// `saved` in a tool's answer, and why not.
    fn tell(self, answer: &mut Value) {
        answer["saved"] = (self == Saved::Kept).into();
        if self == Saved::Partial {
            answer["why"] = "the project is bigger than Kumi reads at once: the snapshot kept before stays".into();
        }
        if let Saved::TooBig(bytes) = self {
            const MIB: f64 = 1024.0 * 1024.0;
            answer["why"] = format!(
                "the project's snapshot is {:.1} MiB, over the {} MiB Kumi keeps: catch-up can't remember this project, and \
                 starts afresh each time",
                bytes as f64 / MIB,
                MAX_SNAPSHOT_BYTES as f64 / MIB
            )
            .into();
        }
    }
}

/// A watch under way.
struct Watching {
    start: Snapshot,
    at: Instant,
    /// What the producer added, as first seen while watching (by its key), and the tracks they selected.
    seen: Arc<Mutex<Seen>>,
    stop: tokio::sync::watch::Sender<bool>,
}

#[derive(Default)]
struct Seen {
    firsts: HashMap<String, Record>,
    selected: Vec<Value>,
    looks: usize,
    /// When a look last found no connection to the DAW (ms since the epoch), until one is made again.
    unlooked_since: Option<i64>,
}

/// The host's catch-up and watch state.
pub struct Catchup {
    store: Store,
    /// The projects (key, identity) this host compared with their snapshot: only theirs is taken again.
    caught_up: Mutex<HashSet<(Key, String)>>,
    /// The key each open project's snapshot was last kept under (by identity): a project saved for the first time, or
    /// under another place, carries its snapshot over.
    last_key: Mutex<HashMap<String, Key>>,
    /// One digest at a time.
    busy: tokio::sync::Mutex<()>,
    watching: Mutex<Option<Watching>>,
}

impl Catchup {
    pub fn new(root: PathBuf) -> Self {
        Self {
            store: Store::new(root),
            caught_up: Mutex::default(),
            last_key: Mutex::default(),
            busy: tokio::sync::Mutex::new(()),
            watching: Mutex::default(),
        }
    }
}

/// A digest put together from its pages, each fetched by `fetch` (the host's call, or a client's request).
async fn collect<F, Fut>(daw: Daw, values: bool, content: bool, mut fetch: F) -> Result<Snapshot, CallError>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, CallError>>,
{
    'tries: for try_number in 0..DIGEST_TRIES {
        let mut snapshot = Snapshot { version: SNAPSHOT_VERSION, daw: daw.id().into(), ..Default::default() };
        let mut cursor: Option<String> = None;
        let mut epoch: Option<String> = None;
        for page_number in 0.. {
            let mut params = json!({"values": values, "content": content});
            if let Some(cursor) = &cursor {
                params["cursor"] = cursor.clone().into();
            }
            let page = fetch(params).await?;
            let page_epoch = page.get("epoch").and_then(Value::as_str).map(str::to_owned);
            if page_number == 0 {
                epoch = page_epoch;
                let project = page.get("project").cloned().unwrap_or(Value::Null);
                let text = |name: &str| project.get(name).and_then(Value::as_str).unwrap_or("").to_owned();
                snapshot.name = text("name");
                snapshot.path = text("path");
                snapshot.identity = text("identity");
                snapshot.place = key_of(daw, &project).place(daw);
                // (the project row itself, for the key: kept out of what's compared)
                snapshot.bounded.insert("\u{0}project".into(), project);
                for (field, into) in
                    [("facts", &mut snapshot.facts), ("bounded", &mut snapshot.bounded), ("summaries", &mut snapshot.summaries)]
                {
                    if let Some(found) = page.get(field).and_then(Value::as_object) {
                        into.extend(found.clone());
                    }
                }
                snapshot.changed = page.get("changed").cloned().filter(|v| !v.is_null());
            } else if page_epoch != epoch {
                // Another project in the tab (or another song) meanwhile: the pages don't belong together.
                continue 'tries;
            } else if try_number + 1 < DIGEST_TRIES
                && page.get("changed").is_some_and(|c| !c.is_null() && Some(c) != snapshot.changed.as_ref())
            {
                // The producer changed the project between two pages: read it whole again (the last try keeps what it read).
                continue 'tries;
            }
            for record in page.get("records").and_then(Value::as_array).into_iter().flatten() {
                if snapshot.records.len() >= MAX_RECORDS {
                    snapshot.bounded.insert("records".into(), json!(format!("more than {MAX_RECORDS} things")));
                    break;
                }
                if let Ok(record) = serde_json::from_value::<Record>(record.clone()) {
                    snapshot.records.push(record);
                }
            }
            if let Some(more) = page.get("bounded").and_then(Value::as_object).filter(|_| page_number > 0) {
                snapshot.bounded.extend(more.clone());
            }
            cursor = page.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned);
            if cursor.is_none() || snapshot.bounded.contains_key("records") {
                break;
            }
            if page_number + 1 >= MAX_PAGES {
                snapshot.bounded.insert("pages".into(), json!(format!("more than {MAX_PAGES} pages")));
                break;
            }
        }
        // A GUID two things share (REAPER keeps a track's GUID in a copy made from its text): each is a thing of its own.
        let mut seen: HashMap<String, usize> = HashMap::new();
        for record in &mut snapshot.records {
            if let Some(id) = record.id.clone() {
                let count = seen.entry(id.clone()).or_default();
                *count += 1;
                if *count > 1 {
                    record.id = Some(format!("{id}~{count}"));
                }
            }
        }
        snapshot.saved_at = now_ms();
        return Ok(snapshot);
    }
    Err(CallError::Bridge { code: "changed".into(), message: "the project changed while Kumi read it: try again".into() })
}

/// Whether a snapshot read now is kept in place of the one kept before (`kept_cut`: whether that one was cut short; none
/// when nothing is kept): a read cut short never replaces a whole one (what it didn't reach would read as added next time);
/// with nothing whole kept, it's kept.
fn replaces(kept_cut: Option<bool>, now: &Snapshot) -> bool {
    !semantic::cut(now) || kept_cut.unwrap_or(true)
}

/// The project row a snapshot was read with.
fn project_row(snapshot: &Snapshot) -> Value {
    snapshot.bounded.get("\u{0}project").cloned().unwrap_or(Value::Null)
}

/// A snapshot as kept: without the project row it was read with.
fn kept(mut snapshot: Snapshot) -> Snapshot {
    snapshot.bounded.shift_remove("\u{0}project");
    snapshot
}

fn request_error(error: RequestError) -> CallError {
    match error {
        RequestError::Bridge { code, message } => CallError::Bridge { code, message },
        RequestError::Timeout => CallError::Timeout,
        RequestError::Closed => CallError::Closed("the connection closed".into()),
    }
}

/// The changes as the answer gives them: catch-up's without refs (they mean nothing to the producer later).
fn changes_json(diff: &Diff, refs: bool) -> Value {
    let mut changes = serde_json::to_value(&diff.changes).unwrap_or(Value::Null);
    if !refs {
        for change in changes.as_array_mut().into_iter().flatten() {
            for side in ["item", "before", "after"] {
                if let Some(item) = change.get_mut(side).and_then(Value::as_object_mut) {
                    item.remove("ref");
                }
            }
        }
    }
    changes
}

fn details_json(details: &[Detail]) -> Value {
    serde_json::to_value(details).unwrap_or(Value::Null)
}

/// The tracks a selection event names: REAPER's `{tracks: [{guid, name}]}`, Renoise's `{track: {name}, trackRef}`.
fn selected_tracks(data: &Value) -> Vec<Value> {
    let mut tracks = vec![];
    for track in data.get("tracks").and_then(Value::as_array).into_iter().flatten() {
        tracks.push(
            json!({"name": track.get("name").cloned().unwrap_or(Value::Null), "key": track.get("guid").cloned().unwrap_or(Value::Null)}),
        );
    }
    if let Some(track) = data.get("track").filter(|t| t.is_object()) {
        let place = data.get("trackRef").and_then(Value::as_str).and_then(|r| r.rsplit(':').next()).and_then(|n| n.parse::<u64>().ok());
        tracks.push(json!({"name": track.get("name").cloned().unwrap_or(Value::Null), "key": place.map(|n| format!("track#/{n}"))}));
    }
    tracks
}

/// Looks at the project again and again while watching, keeping the first look at each thing that wasn't there when the
/// watch started, and the tracks the producer selects.
/// The next of a connection's events; none while there's no connection.
async fn next_event(
    events: &mut Option<tokio::sync::broadcast::Receiver<daw_bridge::wire::Event>>,
) -> Result<daw_bridge::wire::Event, tokio::sync::broadcast::error::RecvError> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

async fn poll(
    daw: Daw,
    following: Following,
    start: HashSet<String>,
    seen: Arc<Mutex<Seen>>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    // The host's connection now: when it's replaced (or made again), the looks and the selections go on with the new one.
    let mut client = following.client().await;
    let mut events = client.as_ref().map(BridgeClient::events);
    let mut gap = POLL_MIN;
    let mut next = tokio::time::Instant::now() + gap;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            event = next_event(&mut events) => match event {
                Ok(event) if event.name == "selection" => {
                    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                    for track in selected_tracks(&event.data) {
                        if seen.selected.last() != Some(&track) && seen.selected.len() < MOST_SELECTIONS {
                            seen.selected.push(track);
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => events = None,
                _ => {}
            },
            _ = tokio::time::sleep_until(next) => {
                let now = following.client().await;
                let Some(now) = now else {
                    seen.lock().unwrap_or_else(|e| e.into_inner()).unlooked_since.get_or_insert_with(now_ms);
                    next = tokio::time::Instant::now() + POLL_MIN;
                    continue;
                };
                if client.as_ref().is_none_or(|before| !before.same_connection(&now)) {
                    events = Some(now.events());
                    client = Some(now.clone());
                }
                let client = now;
                let began = Instant::now();
                let look = collect(daw, true, false, |params| {
                    let client = client.clone();
                    async move { client.request("digest", params, DIGEST_TIMEOUT).await.map_err(request_error) }
                })
                .await;
                if let Ok(now) = look {
                    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                    seen.looks += 1;
                    seen.unlooked_since = None;
                    for record in now.records {
                        let key = record.key();
                        if !start.contains(&key) {
                            seen.firsts.entry(key).or_insert(record);
                        }
                    }
                }
                gap = (began.elapsed() * 10).clamp(POLL_MIN, POLL_MAX);
                next = tokio::time::Instant::now() + gap;
            }
        }
    }
}

impl Host {
    /// The project's digest now.
    async fn digest(&self, pick: &Pick, values: bool, content: bool) -> Result<Snapshot, CallError> {
        collect(self.daw(), values, content, |params| self.call("digest", params, pick, DIGEST_TIMEOUT)).await
    }

    /// Why a digest couldn't be read, as a tool answers it: a bridge without `digest` can't compare at all.
    fn digest_error(&self, error: &CallError) -> Value {
        match error {
            CallError::Bridge { code, .. } if code == daw_bridge::wire::code::UNKNOWN_METHOD => json!({
                "error": format!("this {} bridge can't compare the project's states: update Kumi's bridge in {}", self.daw(), self.daw()),
                "code": "unsupported",
                "connected": true,
                "daw": self.daw().id(),
            }),
            other => self.call_error(other),
        }
    }

    /// The snapshot to compare an open project with, and whether it's the one kept under its key: its own, else (saved
    /// for the first time, or under another place) the one it was last kept under, by its identity.
    fn baseline_of(&self, key: &Key, identity: &str) -> (Option<Snapshot>, bool) {
        let state = self.catchup();
        if let Some(found) = state.store.load(key) {
            return (Some(found), true);
        }
        let last = state.last_key.lock().unwrap_or_else(|e| e.into_inner()).get(identity).cloned();
        let found = last.filter(|last| last != key).and_then(|last| state.store.load(&last)).or_else(|| match key {
            Key::Saved { .. } if !identity.is_empty() => state.store.load_unsaved(identity),
            _ => None,
        });
        (found, false)
    }

    /// Keeps a snapshot read now as the project's, unless it's cut short and the one kept under its key isn't
    /// (`kept_cut`: whether that one was cut short, none when nothing is kept there).
    fn keep(&self, key: &Key, snapshot: Snapshot, kept_cut: Option<bool>) -> Result<Saved, String> {
        let state = self.catchup();
        if !replaces(kept_cut, &snapshot) {
            return Ok(Saved::Partial);
        }
        let snapshot = kept(snapshot);
        let saved = state.store.save(key, &snapshot)?;
        if saved == Saved::Kept {
            if let Key::Saved { .. } = key {
                // An unsaved project saved now: its snapshot by identity is the place's from here.
                state.store.forget_unsaved(&snapshot.identity);
            }
            state.last_key.lock().unwrap_or_else(|e| e.into_inner()).insert(snapshot.identity.clone(), key.clone());
        }
        Ok(saved)
    }

    /// `catch_up`: what changed in the open project since its last snapshot, and the snapshot taken again.
    pub async fn catch_up(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why}), true),
        };
        let _one = self.catchup().busy.lock().await;
        let began = Instant::now();
        let now = match self.digest(&pick, false, true).await {
            Ok(now) => now,
            Err(error) => return (self.digest_error(&error), true),
        };
        let key = key_of(self.daw(), &project_row(&now));
        let mut answer = json!({"place": key.place(self.daw()), "name": now.name, "records": now.records.len()});
        if !now.bounded.is_empty() {
            let bounded: Map<String, Value> =
                now.bounded.iter().filter(|(k, _)| !k.starts_with('\u{0}')).map(|(k, v)| (k.clone(), v.clone())).collect();
            if !bounded.is_empty() {
                answer["bounded"] = Value::Object(bounded);
            }
        }
        // (what's kept under the project's key is read once: compared with, and told to `keep`)
        let (before, own) = self.baseline_of(&key, &now.identity);
        let kept_cut = before.as_ref().filter(|_| own).map(semantic::cut);
        match before {
            None => answer["first"] = true.into(),
            Some(before) => {
                let diff = semantic::diff(&before, &now, false, &HashMap::new());
                answer["lastSeenAt"] = before.saved_at.into();
                answer["project"] = details_json(&diff.project);
                answer["changes"] = changes_json(&diff, false);
                if diff.more > 0 {
                    answer["more"] = diff.more.into();
                }
                if diff.is_empty() {
                    answer["same"] = true.into();
                }
            }
        }
        let identity = now.identity.clone();
        match self.keep(&key, now, kept_cut) {
            Ok(saved) => saved.tell(&mut answer),
            Err(why) => answer["saveError"] = why.into(),
        }
        self.catchup().caught_up.lock().unwrap_or_else(|e| e.into_inner()).insert((key, identity));
        answer["ms"] = (began.elapsed().as_millis() as u64).into();
        (answer, false)
    }

    /// `baseline_save`: the open project's snapshot taken again, once this host has compared it (or a project open in the
    /// same run of the DAW that it became, saved since) with the one it had.
    pub async fn baseline_save(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why}), true),
        };
        let _one = self.catchup().busy.lock().await;
        let began = Instant::now();
        let now = match self.digest(&pick, false, true).await {
            Ok(now) => now,
            Err(error) => return (self.digest_error(&error), true),
        };
        let key = key_of(self.daw(), &project_row(&now));
        let place = key.place(self.daw());
        {
            let mut caught_up = self.catchup().caught_up.lock().unwrap_or_else(|e| e.into_inner());
            let seen = caught_up.contains(&(key.clone(), now.identity.clone()))
                || (!now.identity.is_empty() && caught_up.iter().any(|(_, identity)| *identity == now.identity));
            if !seen {
                return (json!({"saved": false, "compared": false, "place": place, "why": "not compared yet: catch_up first"}), false);
            }
            caught_up.insert((key.clone(), now.identity.clone()));
        }
        let kept_cut = self.catchup().store.load_head(&key).map(|head| semantic::cut_bounds(&head.bounded));
        let mut answer = json!({"place": place, "records": now.records.len(), "changed": now.changed});
        match self.keep(&key, now, kept_cut) {
            Ok(saved) => {
                answer["ms"] = (began.elapsed().as_millis() as u64).into();
                saved.tell(&mut answer);
                (answer, false)
            }
            Err(why) => (json!({"error": why, "place": place}), true),
        }
    }

    /// `watch_start`: the project as it is now, kept to compare when the producer is done.
    pub async fn watch_start(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why}), true),
        };
        let start = match self.digest(&pick, true, true).await {
            Ok(start) => start,
            Err(error) => return (self.digest_error(&error), true),
        };
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (stop, stopped) = tokio::sync::watch::channel(false);
        if let Ok(instance) = self.pin(&pick).await {
            let keys = start.records.iter().map(Record::key).collect();
            tokio::spawn(poll(self.daw(), self.following(instance), keys, seen.clone(), stopped));
        }
        let answer = json!({"watching": true, "place": start.place, "name": start.name});
        let before =
            self.catchup().watching.lock().unwrap_or_else(|e| e.into_inner()).replace(Watching { start, at: Instant::now(), seen, stop });
        if let Some(before) = before {
            let _ = before.stop.send(true);
        }
        (answer, false)
    }

    /// `watch_stop`: what changed since `watch_start`, with the parameters the producer moved.
    pub async fn watch_stop(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why}), true),
        };
        let Some(watching) = self.catchup().watching.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return (json!({"error": "not watching: start first, before the producer works", "code": "not-watching"}), true);
        };
        let now = match self.digest(&pick, true, true).await {
            Ok(now) => now,
            Err(error) => {
                // (busy, say: the watch goes on, to stop once the DAW answers, unless another started meanwhile)
                let mut slot = self.catchup().watching.lock().unwrap_or_else(|e| e.into_inner());
                if slot.is_none() {
                    *slot = Some(watching);
                } else {
                    let _ = watching.stop.send(true);
                }
                return (self.digest_error(&error), true);
            }
        };
        let _ = watching.stop.send(true);
        if now.identity != watching.start.identity {
            return (
                json!({"error": "another project is open now, so there's nothing to compare: start again", "code": "other-project"}),
                true,
            );
        }
        let (firsts, selected, looks, unlooked_since) = {
            let seen = watching.seen.lock().unwrap_or_else(|e| e.into_inner());
            (seen.firsts.clone(), seen.selected.clone(), seen.looks, seen.unlooked_since)
        };
        let mut diff = semantic::diff(&watching.start, &now, true, &firsts);
        self.name_moves(&mut diff, &pick).await;
        let mut answer = json!({
            "watched": format!("{} s", watching.at.elapsed().as_secs_f64().round() as u64),
            "place": now.place,
            "name": now.name,
            "project": details_json(&diff.project),
            "changes": changes_json(&diff, true),
            "looks": looks,
        });
        if diff.more > 0 {
            answer["more"] = diff.more.into();
        }
        if !selected.is_empty() {
            answer["selected"] = selected.into();
        }
        if let Some(at) = unlooked_since {
            answer["stoppedLooking"] = json!({
                "at": at,
                "why": format!(
                    "Kumi's looks found no connection to {} from then on: what was added after is told without the parameters moved on it",
                    self.daw()
                ),
            });
        }
        (answer, false)
    }

    /// The parameters that moved, by their names and as the DAW shows them, as details of their changes (or, for a thing
    /// added, as what it's made of). A few things at most are read; the rest say how many moved.
    async fn name_moves(&self, diff: &mut Diff, pick: &Pick) {
        let first_index = if self.daw() == Daw::Renoise { 1 } else { 0 };
        let mut read = 0;
        for at in 0..diff.changes.len() {
            let (moved, reference) = match &diff.changes[at] {
                Change::Changed { moved, after, .. } => (moved.clone(), after.reference.clone()),
                Change::Added { moved, item, .. } => (moved.clone(), item.reference.clone()),
                Change::Removed { .. } => continue,
            };
            if moved.is_empty() {
                continue;
            }
            let named = if read < MOST_READ {
                read += 1;
                self.named(&moved, reference.as_deref(), first_index, pick).await
            } else {
                vec![]
            };
            match &mut diff.changes[at] {
                Change::Changed { details, .. } if named.is_empty() => details.push(Detail {
                    facet: "parameters".into(),
                    what: "parameters".into(),
                    from: Value::Null,
                    to: json!(format!("{} moved", moved.len())),
                    fingerprint: false,
                }),
                Change::Changed { details, .. } => details.extend(named),
                Change::Added { made_of, .. } if named.is_empty() => {
                    made_of.insert("parametersMoved".into(), moved.len().into());
                }
                Change::Added { made_of, .. } => {
                    let named = named.iter().map(|d| json!({"name": d.what.trim_start_matches("parameters."), "value": d.to})).collect();
                    made_of.insert("parameters".into(), Value::Array(named));
                }
                Change::Removed { .. } => {}
            }
        }
    }

    /// The moved parameters of the thing at `reference`, read by name.
    async fn named(&self, moved: &[semantic::Moved], reference: Option<&str>, first_index: usize, pick: &Pick) -> Vec<Detail> {
        let Some(reference) = reference else { return vec![] };
        let wanted: HashSet<usize> = moved.iter().take(MOST_PARAMETERS).map(|m| m.index + first_index).collect();
        let mut rows: HashMap<usize, Value> = HashMap::new();
        let mut cursor: Option<String> = None;
        for _ in 0..16 {
            let mut params = json!({"kind": "param", "parent": reference, "limit": crate::tools::MAX_LIMIT});
            if let Some(cursor) = &cursor {
                params["cursor"] = cursor.clone().into();
            }
            let Ok(page) = self.call("read", params, pick, DIGEST_TIMEOUT).await else { break };
            for row in page.get("items").and_then(Value::as_array).into_iter().flatten() {
                if let Some(index) = row.get("index").and_then(Value::as_u64).map(|i| i as usize).filter(|i| wanted.contains(i)) {
                    rows.insert(index, row.clone());
                }
            }
            cursor = page.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned);
            if cursor.is_none() || rows.len() == wanted.len() {
                break;
            }
        }
        moved
            .iter()
            .take(MOST_PARAMETERS)
            .filter_map(|m| {
                let row = rows.get(&(m.index + first_index))?;
                let name = row.get("name").and_then(Value::as_str)?;
                let shows = row.get("formatted").or_else(|| row.get("valueString")).cloned().unwrap_or(Value::Null);
                Some(Detail {
                    facet: "parameters".into(),
                    what: format!("parameters.{name}"),
                    from: m.from.clone(),
                    to: json!({"value": m.to, "shows": shows}),
                    fingerprint: false,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(path: &str, identity: &str, tempo: f64) -> Snapshot {
        Snapshot {
            version: SNAPSHOT_VERSION,
            daw: "reaper".into(),
            path: path.into(),
            identity: identity.into(),
            facts: json!({"tempo": tempo}).as_object().unwrap().clone(),
            ..Default::default()
        }
    }

    #[test]
    fn a_saved_projects_key_is_its_place_and_file_and_an_unsaved_ones_its_identity() {
        let id = "0123456789ABCDEF0123456789abcdef";
        assert_eq!(
            key_of(Daw::Reaper, &json!({"id": id, "path": "/p/a.rpp", "identity": "1:2"})),
            Key::Saved { place: "reaper-0123456789abcdef0123456789abcdef".into(), path: "/p/a.rpp".into() }
        );
        // Renoise gives an unsaved song an id of its own until it's saved: no file, no place.
        assert_eq!(key_of(Daw::Renoise, &json!({"id": id, "path": null, "identity": "7:1"})), Key::Unsaved { identity: "7:1".into() });
        assert_eq!(
            key_of(Daw::Reaper, &json!({"id": null, "path": "/p/a.rpp", "identity": "1:2"})),
            Key::Unsaved { identity: "1:2".into() }
        );
        assert_eq!(Key::Unsaved { identity: "x".into() }.place(Daw::Renoise), "renoise-unsaved");
    }

    #[test]
    fn each_file_of_a_song_keeps_its_own_snapshot_and_a_moved_one_finds_its_latest() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("snapshots"));
        let songs = tempfile::tempdir().unwrap();
        let (v1, v2) = (songs.path().join("song.rpp"), songs.path().join("song v2.rpp"));
        std::fs::write(&v1, "").unwrap();
        std::fs::write(&v2, "").unwrap();
        let (v1, v2) = (v1.to_string_lossy().to_string(), v2.to_string_lossy().to_string());
        let key = |path: &str| Key::Saved { place: "reaper-0123456789abcdef0123456789abcdef".into(), path: path.into() };
        assert!(store.load(&key(&v1)).is_none());
        assert_eq!(store.save(&key(&v1), &snapshot(&v1, "1:1", 120.0)).unwrap(), Saved::Kept);
        assert_eq!(store.load(&key(&v1)).unwrap().facts["tempo"], 120.0);
        // Another file of the same song, both still there: a version first seen here has none, and keeps its own.
        assert!(store.load(&key(&v2)).is_none());
        store.save(&key(&v2), &snapshot(&v2, "1:1", 130.0)).unwrap();
        assert_eq!(store.load(&key(&v1)).unwrap().facts["tempo"], 120.0, "v1 kept its own");
        assert_eq!(store.load(&key(&v2)).unwrap().facts["tempo"], 130.0);
        // The song moved: v2's file is gone, and the project is found where it is now.
        std::fs::remove_file(&v2).unwrap();
        let moved = songs.path().join("moved.rpp").to_string_lossy().to_string();
        assert_eq!(store.load(&key(&moved)).unwrap().facts["tempo"], 130.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = home.path().join("snapshots/reaper-0123456789abcdef0123456789abcdef/last-seen.json");
            assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600, "owner only");
        }
    }

    // Review H9: a snapshot too big to keep says so, with its size, and nothing is written.
    #[test]
    fn a_snapshot_too_big_to_keep_says_why_with_its_size() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().to_path_buf());
        let mut big = snapshot("", "5:1", 120.0);
        big.name = "x".repeat(MAX_SNAPSHOT_BYTES + 512 * 1024);
        let key = Key::Unsaved { identity: "5:1".into() };
        let saved = store.save(&key, &big).unwrap();
        assert!(matches!(saved, Saved::TooBig(bytes) if bytes > MAX_SNAPSHOT_BYTES), "{saved:?}");
        assert!(store.load(&key).is_none());
        let mut answer = json!({});
        saved.tell(&mut answer);
        assert_eq!(answer["saved"], false);
        let why = answer["why"].as_str().unwrap();
        assert!(why.starts_with("the project's snapshot is 8.5 MiB, over the 8 MiB Kumi keeps"), "{why}");
    }

    // Review of #576 (second sweep OW2): not flushed, a kept snapshot can be left empty or cut short by a power cut. Either
    // reads as none, whole or by its head (catch_up answers `first`, baseline_save keeps the snapshot read now), and the
    // next save writes it anew.
    #[test]
    fn a_kept_snapshot_left_empty_or_cut_short_reads_as_none_and_is_written_anew() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().to_path_buf());
        let place = "reaper-0123456789abcdef0123456789abcdef";
        let saved = Key::Saved { place: place.into(), path: "/p/song.rpp".into() };
        let unsaved = Key::Unsaved { identity: "7:1".into() };
        for (key, path, file) in [
            (&saved, "/p/song.rpp", home.path().join(place).join("last-seen.json")),
            (&unsaved, "", home.path().join("unsaved").join(format!("{}.json", file_key("7:1")))),
        ] {
            let whole = snapshot(path, "7:1", 120.0);
            assert_eq!(store.save(key, &whole).unwrap(), Saved::Kept);
            let text = std::fs::read(&file).unwrap();
            for broken in [&text[..0], &text[..text.len() / 2]] {
                std::fs::write(&file, broken).unwrap();
                assert!(store.load(key).is_none(), "{key:?}: {} of {} bytes", broken.len(), text.len());
                assert!(store.load_head(key).is_none(), "{key:?}: {} of {} bytes", broken.len(), text.len());
                assert!(replaces(store.load_head(key).map(|head| semantic::cut_bounds(&head.bounded)), &whole));
                assert_eq!(store.save(key, &whole).unwrap(), Saved::Kept);
                assert_eq!(store.load(key).unwrap().facts["tempo"], 120.0, "{key:?}: written anew");
            }
        }
    }

    #[test]
    fn a_snapshot_cut_short_never_replaces_a_whole_one() {
        let whole = snapshot("/a.rpp", "9:1", 120.);
        let mut cut_short = whole.clone();
        cut_short.bounded.insert("pages".into(), json!("more than 400 pages"));
        assert!(!replaces(Some(semantic::cut(&whole)), &cut_short), "the whole one stays");
        assert!(replaces(None, &cut_short), "nothing better kept");
        assert!(replaces(Some(semantic::cut(&cut_short)), &cut_short));
        assert!(replaces(Some(semantic::cut(&cut_short)), &whole));
        let mut answer = json!({});
        Saved::Partial.tell(&mut answer);
        assert_eq!(answer["saved"], false);
    }

    #[test]
    fn unsaved_projects_are_kept_by_identity_a_few_at_most() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().to_path_buf());
        for n in 0..(MAX_UNSAVED + 3) {
            let key = Key::Unsaved { identity: format!("99:{n}") };
            store.save(&key, &snapshot("", &format!("99:{n}"), n as f64)).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(std::fs::read_dir(home.path().join("unsaved")).unwrap().count(), MAX_UNSAVED);
        assert!(store.load_unsaved(&format!("99:{}", MAX_UNSAVED + 2)).is_some());
        assert!(store.load_unsaved("99:0").is_none(), "the oldest went");
        store.forget_unsaved(&format!("99:{}", MAX_UNSAVED + 2));
        assert!(store.load_unsaved(&format!("99:{}", MAX_UNSAVED + 2)).is_none());
    }

    // Second sweep OW2: what one catch_up and one baseline_save do with the snapshot files, as main did it (the kept file
    // parsed up to three times, the new snapshot cloned, the write flushed) and as now. A timing, release-like:
    // `--run-ignored only --no-capture -E 'test(/sweep2_ow2/)'`.
    #[test]
    #[ignore]
    fn sweep2_ow2_snapshot_files_of_a_catch_up_and_a_baseline_save() {
        use std::io::Write;
        fn median(mut times: Vec<Duration>) -> Duration {
            times.sort();
            times[times.len() / 2]
        }
        for tracks in [1_000, 10_000] {
            let home = tempfile::tempdir().unwrap();
            let store = Store::new(home.path().to_path_buf());
            let key = Key::Saved { place: "reaper-0123456789abcdef0123456789abcdef".into(), path: "/p/song.rpp".into() };
            let mut now = snapshot("/p/song.rpp", "1:1", 120.0);
            now.records = (0..tracks)
                .map(|n| {
                    serde_json::from_value(json!({"k": "track", "id": format!("{{{n:08X}-0000-0000-0000-000000000000}}"), "n": format!("Track {n}"),
                        "o": n, "f": {"mixer": {"volume": -6.02, "pan": 0.0, "width": 1.0, "mute": false, "solo": false, "phase": false},
                        "color": "#5a8fd0", "input": {"arm": false, "monitor": 1, "recInput": 4096},
                        "sends": {"count": 2, "sendsHash": "9c1185a5c5e9fc54"}, "FX": {"count": 3},
                        "items": {"count": 12, "itemsHash": "cbf29ce484222325"}, "automation": {"count": 1, "pointsHash": "af63bd4c8601b7df"}}}))
                    .unwrap()
                })
                .collect();
            now.bounded.insert("\u{0}project".into(), json!({"id": "0123456789abcdef0123456789abcdef", "path": "/p/song.rpp"}));
            store.save(&key, &kept(now.clone())).unwrap();
            let file = home.path().join("reaper-0123456789abcdef0123456789abcdef/last-seen.json");
            let bytes = std::fs::metadata(&file).unwrap().len();
            // (main's write: the same file, flushed before it's renamed into place)
            let flushed = |text: &[u8]| {
                let temporary = file.with_extension("tmp");
                let mut out = std::fs::File::create(&temporary).unwrap();
                out.write_all(text).unwrap();
                out.sync_all().unwrap();
                std::fs::rename(&temporary, &file).unwrap();
            };
            let (mut main_catch_up, mut main_save, mut catch_up, mut save) = (vec![], vec![], vec![], vec![]);
            for _ in 0..7 {
                let began = Instant::now();
                let before = store.load(&key).unwrap();
                let kept_before = store.load(&key);
                let copy = kept(now.clone());
                let last = read_snapshot::<Snapshot>(&file).unwrap();
                flushed(&serde_json::to_vec(&copy).unwrap());
                main_catch_up.push(began.elapsed());
                assert!(replaces(kept_before.as_ref().map(semantic::cut), &now) && last.path == before.path);
                let began = Instant::now();
                let kept_before = store.load(&key);
                let copy = kept(now.clone());
                let last = read_snapshot::<Snapshot>(&file).unwrap();
                flushed(&serde_json::to_vec(&copy).unwrap());
                main_save.push(began.elapsed());
                assert!(kept_before.is_some() && last.path == now.path);
                // Now: the kept one parsed once to be compared, the file's head read to save; the snapshot moved, not cloned.
                let fresh = now.clone();
                let began = Instant::now();
                let before = store.load(&key).unwrap();
                let kept_cut = Some(semantic::cut(&before));
                assert!(replaces(kept_cut, &fresh));
                assert_eq!(store.save(&key, &kept(fresh)).unwrap(), Saved::Kept);
                catch_up.push(began.elapsed());
                let fresh = now.clone();
                let began = Instant::now();
                let kept_cut = store.load_head(&key).map(|head| semantic::cut_bounds(&head.bounded));
                assert!(replaces(kept_cut, &fresh));
                assert_eq!(store.save(&key, &kept(fresh)).unwrap(), Saved::Kept);
                save.push(began.elapsed());
            }
            eprintln!(
                "{tracks} tracks, a {:.0} KB snapshot: catch_up's files {:?} -> {:?}, baseline_save's {:?} -> {:?} (medians of 7)",
                bytes as f64 / 1024.0,
                median(main_catch_up),
                median(catch_up),
                median(main_save),
                median(save)
            );
        }
    }

    #[tokio::test]
    async fn pages_from_another_epoch_are_read_again_and_bounds_are_kept() {
        let pages = [
            json!({"epoch": "1", "project": {"name": "A", "path": "/a.rpp", "id": null, "identity": "9:1"}, "facts": {"tempo": 120}, "records": [{"k": "track", "id": "{A}", "o": 1}], "nextCursor": "c"}),
            json!({"epoch": "2", "records": [{"k": "track", "id": "{B}", "o": 2}]}),
            json!({"epoch": "2", "project": {"name": "B", "path": "/b.rpp", "id": null, "identity": "9:2"}, "facts": {"tempo": 90}, "summaries": {"item": "items"}, "records": [{"k": "track", "id": "{C}", "o": 1}, {"k": 3}], "nextCursor": "c"}),
            json!({"epoch": "2", "records": [{"k": "track", "id": "{D}", "o": 2}], "bounded": {"item": "too many"}}),
        ];
        let at = std::sync::atomic::AtomicUsize::new(0);
        let snapshot = collect(Daw::Reaper, false, true, |_| {
            let page = pages[at.fetch_add(1, std::sync::atomic::Ordering::SeqCst)].clone();
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(snapshot.name, "B");
        assert_eq!(snapshot.records.iter().map(|r| r.id.clone().unwrap()).collect::<Vec<_>>(), ["{C}", "{D}"], "a bad record is skipped");
        assert_eq!(snapshot.facts["tempo"], 90);
        assert_eq!(snapshot.bounded["item"], "too many");
        assert_eq!(snapshot.summaries["item"], "items");
        assert_eq!(kept(snapshot).bounded.len(), 1, "the project row isn't kept");
    }

    #[tokio::test]
    async fn a_page_after_the_producer_changed_something_reads_the_digest_again_and_shared_keys_stay_apart() {
        let pages = [
            json!({"epoch": "1", "changed": 5, "project": {"identity": "9:1"}, "records": [{"k": "track", "id": "{A}", "o": 1}], "nextCursor": "c"}),
            json!({"epoch": "1", "changed": 6, "records": [{"k": "track", "id": "{B}", "o": 2}]}),
            json!({"epoch": "1", "changed": 6, "project": {"identity": "9:1"}, "records": [{"k": "track", "id": "{A}", "o": 1}, {"k": "track", "id": "{A}", "o": 2}], "nextCursor": "c"}),
            json!({"epoch": "1", "changed": 6, "records": [{"k": "track", "id": "{B}", "o": 3}]}),
        ];
        let at = std::sync::atomic::AtomicUsize::new(0);
        let snapshot = collect(Daw::Reaper, false, true, |_| {
            let page = pages[at.fetch_add(1, std::sync::atomic::Ordering::SeqCst)].clone();
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(snapshot.records.iter().map(|r| r.id.clone().unwrap()).collect::<Vec<_>>(), ["{A}", "{A}~2", "{B}"]);
        assert_eq!(snapshot.changed, Some(json!(6)));
    }
}
