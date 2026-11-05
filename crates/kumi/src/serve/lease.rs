//! The DAW endpoint's write lease (C2: one writer per DAW endpoint). A call that writes, plays or imports holds it from
//! before it runs until it has settled, cleanup included. Within the runtime, such calls take turns on it. Across
//! runtimes (another Hermes profile reaching the same DAW through the same Kumi home), an OS lock on
//! `<kumi-home>/serve/<daw>.lease` refuses the second at once (`leased`, naming who holds it) rather than queueing it:
//! the lock goes with its process, so a runtime that dies frees it.

use super::ctx::{Host, OpError};
use serde_json::{json, Value};
use std::cell::Cell;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

pub struct Lease {
    path: PathBuf,
    turns: tokio::sync::Mutex<()>,
    held: Cell<bool>,
}

/// The lease, held: freed when this drops.
pub struct Held<'a> {
    _turn: tokio::sync::MutexGuard<'a, ()>,
    _file: File,
    lease: &'a Lease,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        // Closing the file unlocks it.
        self.lease.held.set(false);
    }
}

impl Lease {
    /// The lease on `daw`'s endpoint reached through the Kumi home `kumi_home`.
    pub fn new(kumi_home: &Path, daw: &str) -> Lease {
        Lease { path: kumi_home.join("serve").join(format!("{daw}.lease")), turns: tokio::sync::Mutex::new(()), held: Cell::new(false) }
    }

    /// Whether this runtime holds it now.
    pub fn held(&self) -> bool {
        self.held.get()
    }

    /// Takes the lease for `host`'s call: waits for this runtime's other writes, then holds the endpoint's lock, or is
    /// refused `leased` when another runtime holds it.
    pub async fn take(&self, host: &Host) -> Result<Held<'_>, OpError> {
        let turn = self.turns.lock().await;
        let unopened = |error: std::io::Error| OpError::Failed { reason: format!("Kumi couldn't open the DAW's write lease: {error}") };
        if let Some(folder) = self.path.parent() {
            super::ctx::make_private_dir(folder).map_err(unopened)?;
        }
        let mut file =
            std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&self.path).map_err(unopened)?;
        if !lock(&file) {
            let mut said = String::new();
            let _ = file.read_to_string(&mut said);
            let owner: Value = serde_json::from_str(&said).unwrap_or(Value::Null);
            let reason = "Another Hermes session is writing to this DAW through Kumi now: try again when it's done.";
            return Err(OpError::Refused { code: "leased", reason: reason.into(), owner: Some(owner) });
        }
        let holder = json!({
            "pid": std::process::id(),
            "profile": host.profile_id,
            "session_id": host.session_id,
            "since": kumi_common::time::now_ms(),
        });
        // Who holds it, for a runtime it refuses; nothing relies on it being there.
        let _ = file.set_len(0).and_then(|()| file.rewind()).and_then(|()| file.write_all(holder.to_string().as_bytes()));
        self.held.set(true);
        Ok(Held { _turn: turn, _file: file, lease: self })
    }
}

/// An exclusive lock on `file` without waiting: false when another process holds it.
#[cfg(unix)]
fn lock(file: &File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: flock on the descriptor of a file this process holds open; it only takes or refuses the lock.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(windows)]
fn lock(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    use windows_sys::Win32::System::IO::OVERLAPPED;
    // The lock is on one byte far past the holder's record: Windows locks bytes, and a runtime refused the lease must
    // still read who holds it from the start of the file.
    // SAFETY: LockFileEx on the handle of a file this process holds open, with an OVERLAPPED that only names the offset.
    unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        overlapped.Anonymous.Anonymous.OffsetHigh = 0x7FFF_FFFF;
        LockFileEx(file.as_raw_handle() as _, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut overlapped) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(profile: &str) -> Host {
        Host { profile_id: Some(profile.into()), session_id: Some("s1".into()), ..Host::default() }
    }

    #[tokio::test]
    async fn one_runtime_writes_at_a_time_and_another_is_told_who() {
        let home = tempfile::tempdir().unwrap();
        let (ours, theirs) = (Lease::new(home.path(), "ableton"), Lease::new(home.path(), "ableton"));
        let held = ours.take(&host("default")).await.unwrap();
        assert!(ours.held());
        let Err(OpError::Refused { code, owner, .. }) = theirs.take(&host("work")).await else { panic!("two writers") };
        assert_eq!(code, "leased");
        assert_eq!(
            (owner.as_ref().unwrap()["profile"].clone(), owner.unwrap()["pid"].clone()),
            (json!("default"), json!(std::process::id()))
        );
        drop(held);
        assert!(!ours.held());
        assert!(theirs.take(&host("work")).await.is_ok(), "free once the first is done");
        assert!(Lease::new(home.path(), "reaper").take(&host("default")).await.is_ok(), "another DAW's endpoint is its own");
    }

    #[tokio::test]
    async fn the_runtimes_own_writes_take_turns() {
        let home = tempfile::tempdir().unwrap();
        let lease = std::rc::Rc::new(Lease::new(home.path(), "ableton"));
        tokio::task::LocalSet::new()
            .run_until(async {
                let first = lease.take(&host("default")).await.unwrap();
                let waiting = tokio::task::spawn_local({
                    let lease = lease.clone();
                    async move { lease.take(&host("default")).await.is_ok() }
                });
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                assert!(!waiting.is_finished(), "the second waits for the first");
                drop(first);
                assert!(waiting.await.unwrap());
            })
            .await;
    }
}
