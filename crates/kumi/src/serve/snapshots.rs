//! C3's snapshot files under the state folder (`status.json`, `focus.json`, `history.json`, `map/<revision>.json`…).
//! Every snapshot carries the envelope its readers check: `schema`, `seq`, `at` (ms) and `owner` (the profile and
//! session of the latest call, and this process).
//!
//! The runtime's thread never waits on the disk, or serializes: writes go, in order, to a writer thread of their own.
//! There each file is serialized and written whole to an owner-only temporary file beside its place, flushed to disk,
//! renamed over it, and its folder flushed (Unix): a reader never sees half of one, and a crash never leaves an empty
//! one. When a reader holds the file open (Windows refuses the rename then), the writer tries again a few times; a
//! newer write of the same file waiting behind it supersedes it.
//!
//! `seq` grows with every write, starting past every snapshot found at start and past the clock (microseconds), so a
//! reader can order the snapshots of one runtime, and of a restarted one, even across a clock step.

use super::ctx::Host;
use super::say;
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// The snapshots' format.
pub const SCHEMA: u32 = 1;
/// How long a reader holding a file may keep its replacement waiting (Windows): the writer's tries, 20 ms apart.
const TRIES: u32 = 25;
/// How long [`Snapshots::flush`] waits for the writer.
const FLUSH: Duration = Duration::from_secs(5);

/// The writer's result for one write: whether its file is on disk.
type Written = tokio::sync::oneshot::Sender<std::io::Result<()>>;
/// Told each snapshot's name once its file is on disk ([`Snapshots::landed`]), from the writer thread.
type Landed = std::sync::Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>>;

/// An already queued, ordered write. Dropping this handle never cancels that write.
#[must_use = "await the queued write's actual outcome; dropping it does not cancel it"]
pub(super) struct PendingWrite {
    seq: u64,
    written: tokio::sync::oneshot::Receiver<std::io::Result<()>>,
}

impl PendingWrite {
    pub(super) async fn wait(self) -> std::io::Result<u64> {
        self.written.await.map_err(|_| std::io::Error::other("the snapshot writer stopped"))??;
        Ok(self.seq)
    }
}

#[cfg(test)]
struct CommitGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: Receiver<()>,
}

struct FileWrite {
    name: String,
    path: PathBuf,
    snapshot: Value,
    /// Compact JSON whose complete envelope must fit this reader limit; None retains the ordinary pretty format.
    max_bytes: Option<usize>,
    /// Told once the file is on disk, or why it isn't.
    done: Option<Written>,
    #[cfg(test)]
    gate: Option<CommitGate>,
}

enum Write {
    File(FileWrite),
    /// Told once every write sent before it is done.
    Flush(Sender<()>),
}

pub struct Snapshots {
    dir: PathBuf,
    seq: Cell<u64>,
    owner: RefCell<Host>,
    /// None when the writer thread couldn't start: writes are done in place then.
    writer: Option<Sender<Write>>,
    landed: Landed,
    #[cfg(test)]
    next_gate: RefCell<Option<(String, CommitGate)>>,
}

impl Snapshots {
    pub fn new(dir: PathBuf) -> Snapshots {
        let (writer, writes) = channel();
        let landed = Landed::default();
        let told = landed.clone();
        let writer = match std::thread::Builder::new().name("kumi-serve-snapshots".into()).spawn(move || write_files(writes, told)) {
            Ok(_) => Some(writer),
            Err(error) => {
                say(&format!("kumi serve: no snapshot writer ({error}); writing in place"));
                None
            }
        };
        let seq = Cell::new(newest_seq(&dir));
        Snapshots {
            dir, seq, owner: RefCell::new(Host::default()), writer, landed,
            #[cfg(test)]
            next_gate: RefCell::new(None),
        }
    }

    /// Each snapshot's name (`status`, `map/r7`) once its file is on disk, in the order they land, and the names
    /// [`Snapshots::touched`] gives: for the runtime's resources (C2) to say what changed. One listener; a new one
    /// replaces the last.
    pub fn landed(&self) -> tokio::sync::mpsc::UnboundedReceiver<String> {
        let (told, landed) = tokio::sync::mpsc::unbounded_channel();
        *self.landed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(told);
        landed
    }

    /// A file of the state folder that isn't a snapshot (the map's `map/current` pointer) is on disk, replaced: its
    /// listener ([`Snapshots::landed`]) is told, as of a snapshot.
    pub fn touched(&self, name: &str) {
        tell(&self.landed, name);
    }

    /// The profile and session the snapshots are for now (from the latest caller's `_host`).
    pub fn owner(&self) -> Host {
        self.owner.borrow().clone()
    }

    /// The host whose calls the snapshots are for from here (the latest caller's profile and session).
    pub fn note_host(&self, host: &Host) {
        let mut owner = self.owner.borrow_mut();
        if host.profile_id.is_some() {
            owner.profile_id = host.profile_id.clone();
        }
        if host.session_id.is_some() {
            owner.session_id = host.session_id.clone();
        }
    }

    /// Writes snapshot `name` (`status`, `map/r7`): `body`'s fields in the envelope; its `seq`. The file is written
    /// on the writer thread (see [`Snapshots::flush`]); only a name that isn't one is an error here. For a large one
    /// (a map), [`Snapshots::write_owned`] spares the copy.
    pub fn write(&self, name: &str, body: &Value) -> std::io::Result<u64> {
        self.write_owned(name, body.clone())
    }

    /// [`Snapshots::write`] taking `body` itself: the envelope goes into it here, and it's serialized and written on the
    /// writer thread, in order with every other write, so the runtime's thread neither copies nor serializes it.
    pub fn write_owned(&self, name: &str, body: Value) -> std::io::Result<u64> {
        let (seq, write) = self.stamped(name, body, None, None)?;
        self.send(write)?;
        Ok(seq)
    }

    /// [`Snapshots::write_owned`], then waits until the file is on disk (written, flushed and renamed into place), or
    /// says why it isn't: for a file something else then relies on (an artifact's record, before its id is given out).
    /// Writes land in the order they're made, so one made after this returns lands after it.
    pub async fn write_durably(&self, name: &str, body: Value) -> std::io::Result<u64> {
        self.write_wait(name, body, None).await
    }

    /// A large reader-bounded snapshot: compact serialization, including the envelope, refuses before replacement
    /// if it exceeds `max_bytes`. Serialization stays on the ordered writer thread.
    pub async fn write_durably_bounded(&self, name: &str, body: Value, max_bytes: usize) -> std::io::Result<u64> {
        self.write_wait(name, body, Some(max_bytes)).await
    }

    async fn write_wait(&self, name: &str, body: Value, max_bytes: Option<usize>) -> std::io::Result<u64> {
        self.enqueue(name, body, max_bytes)?.wait().await
    }

    /// Enqueues synchronously, with no cancellation point before `send`. Once this
    /// returns, only the writer's actual result can establish whether it committed.
    pub(super) fn enqueue(&self, name: &str, body: Value, max_bytes: Option<usize>) -> std::io::Result<PendingWrite> {
        let (done, written) = tokio::sync::oneshot::channel();
        let (seq, write) = self.stamped(name, body, Some(done), max_bytes)?;
        self.send(write)?;
        Ok(PendingWrite { seq, written })
    }

    #[cfg(test)]
    pub(super) fn pause_next_commit(&self, name: &str) -> (tokio::sync::oneshot::Receiver<()>, Sender<()>) {
        assert!(self.writer.is_some(), "this fixture needs the normal writer thread");
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = channel();
        *self.next_gate.borrow_mut() = Some((name.into(), CommitGate { entered, release: released }));
        (waiting, release)
    }

    /// Snapshot `name`'s write: `body`'s fields in the envelope, with its `seq`.
    fn stamped(&self, name: &str, body: Value, done: Option<Written>, max_bytes: Option<usize>) -> std::io::Result<(u64, Write)> {
        let path = self.path(name)?;
        let seq = (self.seq.get() + 1).max(now_us());
        self.seq.set(seq);
        let mut snapshot = match body {
            Value::Object(fields) => fields,
            _ => Map::new(),
        };
        let owner = self.owner.borrow();
        snapshot.insert("schema".into(), json!(SCHEMA));
        snapshot.insert("seq".into(), json!(seq));
        snapshot.insert("at".into(), json!(kumi_common::time::now_ms()));
        snapshot.insert("owner".into(), json!({"profile": owner.profile_id, "session_id": owner.session_id, "pid": std::process::id()}));
        #[cfg(test)]
        let gate = {
            let mut next = self.next_gate.borrow_mut();
            if next.as_ref().is_some_and(|(target, _)| target == name) {
                next.take().map(|(_, gate)| gate)
            } else {
                None
            }
        };
        Ok((seq, Write::File(FileWrite {
            name: name.to_owned(), path, snapshot: Value::Object(snapshot), max_bytes, done,
            #[cfg(test)]
            gate,
        })))
    }

    /// To the writer thread; written in place only when there's none to take it.
    fn send(&self, write: Write) -> std::io::Result<()> {
        let unsent = match &self.writer {
            Some(writer) => writer.send(write).err().map(|failed| failed.0),
            None => Some(write),
        };
        if let Some(Write::File(mut file)) = unsent {
            let written = write_file(&mut file);
            if written.is_ok() {
                tell(&self.landed, &file.name);
            }
            let failed = written.as_ref().err().map(|error| std::io::Error::new(error.kind(), error.to_string()));
            if let Some(done) = file.done {
                let _ = done.send(written);
            }
            if let Some(error) = failed {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Waits (a few seconds at most) until every snapshot written so far is on disk.
    pub fn flush(&self) {
        let Some(writer) = &self.writer else { return };
        let (done, flushed) = channel();
        if writer.send(Write::Flush(done)).is_ok() {
            let _ = flushed.recv_timeout(FLUSH);
        }
    }

    /// Whether snapshot `name` on disk is this process's (its `owner.pid`): another runtime of the profile may have
    /// written it since. Flushes first.
    pub fn is_mine(&self, name: &str) -> bool {
        self.flush();
        let Ok(path) = self.path(name) else { return false };
        let read = std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        read.is_some_and(|snapshot| snapshot["owner"]["pid"] == std::process::id())
    }

    /// Where snapshot `name` goes: `<state>/<name>.json`, its name lowercase words and `/` between folders.
    fn path(&self, name: &str) -> std::io::Result<PathBuf> {
        let named = !name.is_empty()
            && name.split('/').all(|part| {
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            });
        if !named {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("no snapshot is named \"{name}\"")));
        }
        Ok(self.dir.join(format!("{name}.json")))
    }
}

/// The clock in microseconds.
fn now_us() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|since| since.as_micros() as u64).unwrap_or(0)
}

/// The highest `seq` of the snapshots in `dir` (and its `jobs/`), or the clock's when it's later.
fn newest_seq(dir: &Path) -> u64 {
    let mut newest = now_us();
    for folder in [dir.to_path_buf(), dir.join("jobs")] {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.filter_map(Result::ok).filter(|entry| entry.file_name().to_string_lossy().ends_with(".json")) {
            let seq = std::fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|snapshot| snapshot["seq"].as_u64());
            newest = newest.max(seq.unwrap_or(0));
        }
    }
    newest
}

/// The writer thread: the writes waiting, in the order they were made. A write followed at once by another of the same
/// file is skipped (the next replaces it straight away) unless someone waits for it; a flush is told once the writes
/// before it are done.
fn write_files(writes: Receiver<Write>, landed: Landed) {
    while let Ok(first) = writes.recv() {
        let mut waiting = vec![first];
        while let Ok(more) = writes.try_recv() {
            waiting.push(more);
        }
        let mut waiting = waiting.into_iter().peekable();
        while let Some(write) = waiting.next() {
            match write {
                Write::File(mut file) => {
                    let superseded = file.done.is_none() && matches!(waiting.peek(), Some(Write::File(next)) if next.path == file.path);
                    if superseded {
                        continue;
                    }
                    let written = write_file(&mut file);
                    match &written {
                        Ok(()) => tell(&landed, &file.name),
                        Err(error) => say(&format!("kumi serve: {} not written: {error}", file.path.display())),
                    }
                    if let Some(done) = file.done {
                        let _ = done.send(written);
                    }
                }
                Write::Flush(done) => {
                    let _ = done.send(());
                }
            }
        }
    }
}

/// The listener of landed snapshots, told `name` (none listening: nothing).
fn tell(landed: &Landed, name: &str) {
    if let Some(told) = landed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
        let _ = told.send(name.to_owned());
    }
}

fn write_file(file: &mut FileWrite) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(gate) = file.gate.take() {
        let _ = gate.entered.send(());
        gate.release.recv_timeout(FLUSH).map_err(std::io::Error::other)?;
    }
    serialized(&file.snapshot, file.max_bytes).and_then(|bytes| replace(&file.path, &bytes))
}

/// A reader-bounded snapshot is compact; ordinary snapshots keep their readable format.
fn serialized(snapshot: &Value, max_bytes: Option<usize>) -> std::io::Result<Vec<u8>> {
    if let Some(max) = max_bytes {
        let mut output = BoundedBytes { bytes: Vec::new(), max };
        serde_json::to_writer(&mut output, snapshot)
            .map_err(|error| std::io::Error::new(error.io_error_kind().unwrap_or(std::io::ErrorKind::InvalidData), error))?;
        output.write_all(b"\n")?;
        return Ok(output.bytes);
    }
    let mut bytes = serde_json::to_vec_pretty(snapshot).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

struct BoundedBytes {
    bytes: Vec<u8>,
    max: usize,
}

impl std::io::Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "snapshot exceeds its reader's byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `path` replaced by `bytes` at once: an owner-only file written beside it and flushed, renamed over it, its folder
/// flushed. A reader that has it open (Windows refuses the rename then) gets [`TRIES`] tries, 20 ms apart.
pub fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let folder = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    super::ctx::make_private_dir(folder)?;
    let mut file = tempfile::Builder::new().prefix(".snapshot-").suffix(".tmp").tempfile_in(folder)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    let mut temporary = file.into_temp_path();
    let mut tries = 1;
    while let Err(failed) = temporary.persist(path) {
        if tries >= TRIES {
            return Err(failed.error);
        }
        tries += 1;
        temporary = failed.path;
        std::thread::sleep(Duration::from_millis(20));
    }
    #[cfg(unix)]
    if let Ok(folder) = std::fs::File::open(folder) {
        let _ = folder.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn a_snapshot_is_its_fields_in_the_envelope_and_each_is_newer_than_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(dir.path().join("state"));
        snapshots.note_host(&Host { profile_id: Some("default".into()), session_id: Some("s1".into()), ..Host::default() });
        let first = snapshots.write("status", &json!({"connected": false, "seq": 1})).unwrap();
        let second = snapshots.write("map/r7", &json!({"revision": "r7"})).unwrap();
        assert!(second > first && first > 0);
        snapshots.flush();
        let status = read(&dir.path().join("state/status.json"));
        assert_eq!((status["schema"].clone(), status["seq"].clone(), status["connected"].clone()), (json!(1), json!(first), json!(false)));
        assert_eq!(status["owner"], json!({"profile": "default", "session_id": "s1", "pid": std::process::id()}));
        assert!(dir.path().join("state/map/r7.json").is_file());
        let left: Vec<_> = std::fs::read_dir(dir.path().join("state"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "nothing half-written is left beside them");
        assert!(snapshots.is_mine("status"));
    }

    #[tokio::test]
    async fn a_write_waited_for_is_on_disk_when_it_returns_after_every_write_made_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        snapshots.write_owned("map/r7", json!({"revision": "r7"})).unwrap();
        let seq = snapshots.write_durably("map/current", json!({"revision": "r7"})).await.unwrap();
        assert_eq!(read(&state.join("map/current.json"))["seq"], json!(seq), "on disk, without a flush");
        assert_eq!(read(&state.join("map/r7.json"))["revision"], "r7", "the map it points at landed first");
    }

    #[tokio::test]
    async fn a_reader_bound_includes_escaping_and_the_envelope_and_preserves_the_prior_file_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        snapshots.write_durably_bounded("map/r1", json!({"revision":"r1","sounds":{}}), 1024).await.unwrap();
        let path = state.join("map/r1.json");
        let before = std::fs::read(&path).unwrap();
        // The raw string fits; its JSON escaping plus the envelope does not.
        let error = snapshots.write_durably_bounded("map/r1", json!({"name":"\n".repeat(512)}), 1024).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(read(&path)["revision"], "r1");
        assert_eq!(snapshots.write_durably_bounded("map/r2", json!({}), 2).await.unwrap_err().kind(), std::io::ErrorKind::InvalidData,);
        assert!(!state.join("map/r2.json").exists());
    }

    #[tokio::test]
    async fn a_write_waited_for_that_fails_says_why() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state"), b"a file, not a folder").unwrap();
        let snapshots = Snapshots::new(dir.path().join("state"));
        assert!(snapshots.write_durably("artifacts", json!({})).await.is_err());
    }

    #[test]
    fn the_last_write_of_a_file_is_the_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        let mut last = 0;
        for n in 0..200 {
            last = snapshots.write("focus", &json!({"n": n})).unwrap();
        }
        snapshots.flush();
        let focus = read(&state.join("focus.json"));
        assert_eq!((focus["n"].clone(), focus["seq"].clone()), (json!(199), json!(last)));
    }

    #[test]
    fn a_restarted_runtime_writes_past_the_newest_snapshot_even_one_from_ahead_of_the_clock() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        super::super::ctx::make_private_dir(&state).unwrap();
        let ahead = now_us() + 3_600_000_000;
        std::fs::write(state.join("status.json"), json!({"schema": 1, "seq": ahead}).to_string()).unwrap();
        let snapshots = Snapshots::new(state);
        assert!(snapshots.write("focus", &json!({})).unwrap() > ahead);
    }

    #[test]
    fn a_snapshot_another_runtime_wrote_last_isnt_this_ones() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        snapshots.write("status", &json!({})).unwrap();
        snapshots.flush();
        let mut theirs = read(&state.join("status.json"));
        theirs["owner"]["pid"] = json!(std::process::id() + 1);
        std::fs::write(state.join("status.json"), theirs.to_string()).unwrap();
        assert!(!snapshots.is_mine("status"));
    }

    #[cfg(unix)]
    #[test]
    fn snapshots_and_their_folder_are_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
        let snapshots = Snapshots::new(state.clone());
        snapshots.write("jobs/j1", &json!({})).unwrap();
        snapshots.write("status", &json!({})).unwrap();
        snapshots.flush();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&state.join("jobs/j1.json")), 0o600);
        assert_eq!(mode(&state.join("jobs")), 0o700);
        assert_eq!(mode(&state), 0o700, "a folder that was there is made the owner's alone");
    }

    #[test]
    fn a_snapshot_is_named_plainly() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(dir.path().to_path_buf());
        for name in ["", "../status", "Status", "map//r7", "status.json"] {
            assert!(snapshots.write(name, &json!({})).is_err(), "{name}");
        }
    }
}
