//! C3's snapshot files under the state folder (`status.json`, `focus.json`, `history.json`, `map/<revision>.json`…).
//! Every snapshot carries the envelope its readers check: `schema`, `seq`, `at` (ms) and `owner` (the profile and
//! session of the latest call, and this process).
//!
//! The runtime's thread never waits on the disk: writes go, in order, to a writer thread of their own. There each file
//! is written whole to an owner-only temporary file beside its place, flushed to disk, renamed over it, and its folder
//! flushed (Unix): a reader never sees half of one, and a crash never leaves an empty one. When a reader holds the file
//! open (Windows refuses the rename then), the writer tries again a few times; a newer write of the same file waiting
//! behind it supersedes it.
//!
//! `seq` grows with every write, starting past every snapshot found at start and past the clock (microseconds), so a
//! reader can order the snapshots of one runtime, and of a restarted one, even across a clock step.

use super::ctx::Host;
use super::say;
use serde_json::{json, Value};
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

enum Write {
    File {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    /// Told once every write sent before it is done.
    Flush(Sender<()>),
}

pub struct Snapshots {
    dir: PathBuf,
    seq: Cell<u64>,
    owner: RefCell<Host>,
    /// None when the writer thread couldn't start: writes are done in place then.
    writer: Option<Sender<Write>>,
}

impl Snapshots {
    pub fn new(dir: PathBuf) -> Snapshots {
        let (writer, writes) = channel();
        let writer = match std::thread::Builder::new().name("kumi-serve-snapshots".into()).spawn(move || write_files(writes)) {
            Ok(_) => Some(writer),
            Err(error) => {
                say(&format!("kumi serve: no snapshot writer ({error}); writing in place"));
                None
            }
        };
        let seq = Cell::new(newest_seq(&dir));
        Snapshots { dir, seq, owner: RefCell::new(Host::default()), writer }
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
    /// on the writer thread (see [`Snapshots::flush`]); only a name that isn't one is an error here.
    pub fn write(&self, name: &str, body: &Value) -> std::io::Result<u64> {
        let path = self.path(name)?;
        let seq = (self.seq.get() + 1).max(now_us());
        self.seq.set(seq);
        let owner = self.owner.borrow();
        let mut snapshot = json!({
            "schema": SCHEMA,
            "seq": seq,
            "at": kumi_common::time::now_ms(),
            "owner": {"profile": owner.profile_id, "session_id": owner.session_id, "pid": std::process::id()},
        });
        if let (Value::Object(snapshot), Value::Object(fields)) = (&mut snapshot, body) {
            for (key, value) in fields {
                if !matches!(key.as_str(), "schema" | "seq" | "at" | "owner") {
                    snapshot.insert(key.clone(), value.clone());
                }
            }
        }
        let mut bytes = serde_json::to_vec_pretty(&snapshot).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        // Written in place only when there's no writer thread to take it.
        let unsent = match &self.writer {
            Some(writer) => writer.send(Write::File { path, bytes }).err().map(|failed| failed.0),
            None => Some(Write::File { path, bytes }),
        };
        if let Some(Write::File { path, bytes }) = unsent {
            replace(&path, &bytes)?;
        }
        Ok(seq)
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

/// The writer thread: the newest write of each file in what's waiting, in order; a flush is told once the writes
/// before it are done.
fn write_files(writes: Receiver<Write>) {
    while let Ok(first) = writes.recv() {
        let mut waiting = vec![first];
        while let Ok(more) = writes.try_recv() {
            waiting.push(more);
        }
        let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut flushed = Vec::new();
        for write in waiting {
            match write {
                Write::File { path, bytes } => match files.iter_mut().find(|(waiting, _)| *waiting == path) {
                    Some(newer) => newer.1 = bytes,
                    None => files.push((path, bytes)),
                },
                Write::Flush(done) => flushed.push(done),
            }
        }
        for (path, bytes) in files {
            if let Err(error) = replace(&path, &bytes) {
                say(&format!("kumi serve: {} not written: {error}", path.display()));
            }
        }
        for done in flushed {
            let _ = done.send(());
        }
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

    #[test]
    fn the_last_write_of_a_file_is_the_one_kept() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(dir.path().to_path_buf());
        let mut last = 0;
        for n in 0..200 {
            last = snapshots.write("focus", &json!({"n": n})).unwrap();
        }
        snapshots.flush();
        let focus = read(&dir.path().join("focus.json"));
        assert_eq!((focus["n"].clone(), focus["seq"].clone()), (json!(199), json!(last)));
    }

    #[test]
    fn a_restarted_runtime_writes_past_the_newest_snapshot_even_one_from_ahead_of_the_clock() {
        let dir = tempfile::tempdir().unwrap();
        let ahead = now_us() + 3_600_000_000;
        std::fs::write(dir.path().join("status.json"), json!({"schema": 1, "seq": ahead}).to_string()).unwrap();
        let snapshots = Snapshots::new(dir.path().to_path_buf());
        assert!(snapshots.write("focus", &json!({})).unwrap() > ahead);
    }

    #[test]
    fn a_snapshot_another_runtime_wrote_last_isnt_this_ones() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(dir.path().to_path_buf());
        snapshots.write("status", &json!({})).unwrap();
        snapshots.flush();
        let mut theirs = read(&dir.path().join("status.json"));
        theirs["owner"]["pid"] = json!(std::process::id() + 1);
        std::fs::write(dir.path().join("status.json"), theirs.to_string()).unwrap();
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
