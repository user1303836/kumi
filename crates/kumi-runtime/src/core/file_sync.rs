//! An older Kumi may still be open while this one runs, writing its notes, techniques and lessons to
//! the files. Once a turn, beside the turn, Kumi looks at those files (their sizes and times) and, when
//! one changed, brings in what changed (`store_import::import_json`), so the next turn knows it. The
//! turn never waits for the look.

use super::{
    store_client::StoreClient,
    store_import::{every_path, import_json, Imported, JsonFiles},
};
use kumi_store::StoreError;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::SystemTime,
};

/// The files as last looked at: each with its size and modified time, when it's there.
type Seen = Vec<(PathBuf, Option<(u64, SystemTime)>)>;

#[derive(Clone)]
pub struct FileSync {
    client: StoreClient,
    files: JsonFiles,
    seen: Arc<Mutex<Option<Seen>>>,
    looking: Arc<AtomicBool>,
    /// Files named already as not whole: each is named once a session.
    named: Arc<Mutex<HashSet<PathBuf>>>,
}

impl FileSync {
    /// Looks for a session whose start named `named` as not whole already.
    pub fn new(client: StoreClient, files: JsonFiles, named: &[PathBuf]) -> FileSync {
        let named = Arc::new(Mutex::new(named.iter().cloned().collect()));
        FileSync { client, files, seen: Arc::default(), looking: Arc::default(), named }
    }
    /// Look at the files once, on a blocking thread, and bring in what changed: what came in (a file
    /// that isn't whole among it only the first time), or None when no file had changed since the last
    /// look (or a look was already under way).
    pub async fn look(&self) -> Result<Option<Imported>, StoreError> {
        if self.looking.swap(true, Ordering::AcqRel) {
            return Ok(None);
        }
        let (store, files, seen, named) = (self.client.store().clone(), self.files.clone(), self.seen.clone(), self.named.clone());
        let looked = tokio::task::spawn_blocking(move || {
            let now = sizes(&files);
            if seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() == Some(&now) {
                return Ok(None);
            }
            let mut imported = import_json(&store, &files, kumi_common::time::now_ms())?;
            *seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(now);
            let mut named = named.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            imported.not_whole.retain(|path| named.insert(path.clone()));
            Ok(Some(imported))
        })
        .await
        .unwrap_or(Err(StoreError::Closed));
        self.looking.store(false, Ordering::Release);
        looked
    }
}

fn sizes(files: &JsonFiles) -> Seen {
    every_path(files)
        .into_iter()
        .map(|path| {
            let seen = std::fs::metadata(&path).ok().and_then(|meta| Some((meta.len(), meta.modified().ok()?)));
            (path, seen)
        })
        .collect()
}
