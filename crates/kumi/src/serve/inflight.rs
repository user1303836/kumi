//! CC-2's R4: a call that writes, plays or imports is on disk before it's dispatched, so a runtime that dies in the
//! middle of one (killed, crashed, or stopped before it settled) leaves word of it. `inflight.json` (the operation, its
//! title, the host call and the digest of what it asked, the project, when) goes in a scratch folder of the call's own,
//! fsynced; it goes with that folder once the call settles. At the next start, each one left becomes W1-b's audit row
//! in HISTORY and R6's `status.unsure`, and its folder is removed. (W1-c's epoch moves on by itself: a new runtime's
//! epochs start past every earlier one's.) One runtime per state folder (CC-6) means a record found at start belongs to
//! no live call.

use super::ctx::{Ctx, OpError, Scratch};
use super::history::CallIdentity;
use super::say;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

const RECORD: &str = "inflight.json";

/// A call being dispatched, on disk until it settles.
pub struct InFlight {
    scratch: Option<Scratch>,
}

impl InFlight {
    /// The call settled (whatever it answered): its record goes, with its folder.
    pub fn settle(mut self) {
        self.scratch.take();
    }
}

impl Drop for InFlight {
    /// Dropped without settling (the runtime is stopping with the call still running): the record stays, for the next
    /// start to find.
    fn drop(&mut self) {
        if let Some(scratch) = self.scratch.take() {
            scratch.keep();
        }
    }
}

/// The record of `op`'s call, written and fsynced before it's dispatched: refused when it can't be, so nothing runs
/// that a crash would leave unrecorded.
pub fn begin(ctx: &Ctx, op: &str, title: Option<&str>, identity: CallIdentity) -> Result<InFlight, OpError> {
    let scratch = ctx.scratch(op)?;
    let record = json!({
        "op": op,
        "title": title,
        "tool_call_id": identity.tool_call_id,
        "digest": identity.digest,
        "project_key": ctx.link().project().map(|project| project.key),
        "generation": ctx.history().generation(),
        "at": kumi_common::time::now_ms(),
    });
    let written = write_synced(&scratch.path().join(RECORD), &serde_json::to_vec(&record).unwrap_or_default());
    if let Err(error) = written {
        return Err(OpError::Failed { reason: format!("Kumi couldn't note the call before running it, so it didn't run: {error}") });
    }
    Ok(InFlight { scratch: Some(scratch) })
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    // The folder's entry for it, too, where the system lets a folder be synced.
    #[cfg(unix)]
    if let Some(folder) = path.parent() {
        std::fs::File::open(folder)?.sync_all()?;
    }
    Ok(())
}

/// The records an earlier runtime left in `state_dir` (each call it didn't see settle), oldest first, each with its
/// folder: [`clear`] it once what the record says is kept elsewhere. A record that can't be read was being written when
/// the runtime stopped, before its call ran (`unread: true`).
pub fn left_over(state_dir: &Path) -> Vec<(Value, PathBuf)> {
    let mut found = Vec::new();
    let Ok(ops) = std::fs::read_dir(state_dir.join("scratch")) else { return found };
    for op in ops.filter_map(Result::ok) {
        let Ok(calls) = std::fs::read_dir(op.path()) else { continue };
        for call in calls.filter_map(Result::ok) {
            let folder = call.path();
            let Ok(bytes) = std::fs::read(folder.join(RECORD)) else { continue };
            let record =
                serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| json!({"op": op.file_name().to_string_lossy(), "unread": true}));
            found.push((record, folder));
        }
    }
    found.sort_by_key(|(record, _)| record["at"].as_i64().unwrap_or(0));
    found
}

/// The folders of records [`left_over`] gave, now kept elsewhere.
pub fn clear(folders: &[PathBuf]) {
    for folder in folders {
        if let Err(error) = std::fs::remove_dir_all(folder) {
            say(&format!("kumi serve: {} not removed: {error}", folder.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_left_by_a_call_that_never_settled_is_found_once() {
        let state = tempfile::tempdir().unwrap();
        let call = state.path().join("scratch/change/1-1");
        std::fs::create_dir_all(&call).unwrap();
        write_synced(&call.join(RECORD), br#"{"op":"change","title":"Raise the drums","tool_call_id":"tc-1","digest":"d1","at":5}"#)
            .unwrap();
        // A listen's scratch with no record isn't a call this knows about.
        std::fs::create_dir_all(state.path().join("scratch/listen_track/2-1")).unwrap();
        // One whose record was still being written when the runtime stopped: its call never ran.
        let torn = state.path().join("scratch/change/1-2");
        std::fs::create_dir_all(&torn).unwrap();
        std::fs::write(torn.join(RECORD), b"{\"op\":\"cha").unwrap();
        let found = left_over(state.path());
        assert_eq!(found.len(), 2);
        let read = found.iter().find(|(record, _)| record.get("unread").is_none()).unwrap();
        assert_eq!((read.0["op"].clone(), read.0["tool_call_id"].clone()), (json!("change"), json!("tc-1")));
        assert!(found.iter().any(|(record, _)| record["unread"] == true));
        assert!(call.exists(), "kept until what it says is kept elsewhere");
        clear(&found.into_iter().map(|(_, folder)| folder).collect::<Vec<_>>());
        assert!(!call.exists() && state.path().join("scratch/listen_track/2-1").exists());
        assert!(left_over(state.path()).is_empty(), "once");
    }
}
