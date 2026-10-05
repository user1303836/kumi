//! Kumi's database from the runtime, whose one thread never waits on the disk: a write is queued to the
//! store's writer and answered here once committed; a read runs on a blocking thread, at most as many
//! at once as the store has read connections.

use super::store_import::{import_json, JsonFiles};
use kumi_store::{Connection, Store, StoreError};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct StoreClient {
    store: Store,
    reads: Arc<tokio::sync::Semaphore>,
}

impl StoreClient {
    /// Open Kumi's database at `path` and read in what earlier Kumis kept in `files`, on a blocking
    /// thread. An error means Kumi keeps its notes and the rest in the files this time, as before.
    pub async fn open(path: PathBuf, files: JsonFiles, now: i64) -> Result<StoreClient, StoreError> {
        tokio::task::spawn_blocking(move || {
            let store = Store::open(&path)?;
            import_json(&store, &files, now)?;
            Ok(StoreClient::new(store))
        })
        .await
        .unwrap_or(Err(StoreError::Closed))
    }
    pub fn new(store: Store) -> StoreClient {
        StoreClient { store, reads: Arc::new(tokio::sync::Semaphore::new(2)) }
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    /// A write, answered once its transaction has committed.
    pub async fn write<T, J>(&self, job: J) -> Result<T, StoreError>
    where
        T: Send + 'static,
        J: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (sender, answer) = tokio::sync::oneshot::channel();
        self.store.write(job, move |result| {
            let _ = sender.send(result);
        });
        answer.await.unwrap_or(Err(StoreError::Closed))
    }
    /// A read, on a blocking thread.
    pub async fn read<T, J>(&self, job: J) -> Result<T, StoreError>
    where
        T: Send + 'static,
        J: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let _turn = self.reads.acquire().await.map_err(|_| StoreError::Closed)?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.read(job)).await.unwrap_or_else(|_| Err(StoreError::Closed))
    }
}
