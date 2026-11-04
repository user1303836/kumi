//! C3's snapshot files under the state folder (`status.json`, `focus.json`, `history.json`, `map/<revision>.json`…):
//! each written whole beside its place and swapped in, so a reader never sees half of one. Every snapshot carries the
//! envelope its readers check: `schema`, `seq`, `at` (ms) and `owner` (the profile and session of the latest call, and
//! this process).
//!
//! `seq` is one counter for all of this runtime's snapshots, starting at the time it started (ms): a reader can tell a
//! newer snapshot from an older one, a restarted runtime's included.

use super::ctx::Host;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

/// The snapshots' format.
pub const SCHEMA: u32 = 1;

pub struct Snapshots {
    dir: PathBuf,
    seq: Cell<u64>,
    owner: RefCell<Host>,
}

impl Snapshots {
    pub fn new(dir: PathBuf) -> Snapshots {
        Snapshots { dir, seq: Cell::new(kumi_common::time::now_ms().max(0) as u64), owner: RefCell::new(Host::default()) }
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

    /// Writes snapshot `name` (`status`, `map/r7`): `body`'s fields with the envelope; its `seq`.
    pub fn write(&self, name: &str, body: &Value) -> std::io::Result<u64> {
        let path = self.path(name)?;
        let seq = self.seq.get() + 1;
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
        let mut text = serde_json::to_vec_pretty(&snapshot).map_err(std::io::Error::other)?;
        text.push(b'\n');
        replace(&path, &text)?;
        Ok(seq)
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

/// `path` replaced by `bytes` at once: written beside it, then renamed over it. A reader that has it open (Windows
/// refuses the rename then) gets a few more tries.
pub fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let folder = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    super::ctx::make_private_dir(folder)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = folder.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_is_its_fields_in_the_envelope_and_each_is_newer_than_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(dir.path().join("state"));
        snapshots.note_host(&Host { profile_id: Some("default".into()), session_id: Some("s1".into()), ..Host::default() });
        let first = snapshots.write("status", &json!({"connected": false, "seq": 1})).unwrap();
        let second = snapshots.write("map/r7", &json!({"revision": "r7"})).unwrap();
        assert!(second > first && first > 0);
        let status: Value = serde_json::from_slice(&std::fs::read(dir.path().join("state/status.json")).unwrap()).unwrap();
        assert_eq!((status["schema"].clone(), status["seq"].clone(), status["connected"].clone()), (json!(1), json!(first), json!(false)));
        assert_eq!(status["owner"], json!({"profile": "default", "session_id": "s1", "pid": std::process::id()}));
        assert!(dir.path().join("state/map/r7.json").is_file());
        let left: Vec<_> = std::fs::read_dir(dir.path().join("state"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "nothing half-written is left beside them");
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
