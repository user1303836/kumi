//! The one thread that writes: it takes whatever writes are queued, up to `MOST`, runs each in its own
//! savepoint of one transaction, commits once, then answers each. Nothing waits on purpose, so a lone
//! write commits as soon as it arrives.

use crate::{in_savepoint, StoreError};
use rusqlite::{Connection, TransactionBehavior};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

/// The most writes one commit takes.
const MOST: usize = 64;
/// How long without writes before the WAL is folded back into the database (a passive checkpoint,
/// which never waits on readers).
const IDLE: Duration = Duration::from_secs(1);

/// What answers a write once its transaction is over: given the commit's error, if any.
type Answer = Box<dyn FnOnce(Option<&StoreError>) + Send>;
/// A queued write: run on the transaction's connection (or told why it can't run), giving its answer.
type Job = Box<dyn FnOnce(Result<&Connection, &StoreError>) -> Answer + Send>;

pub(crate) struct Writer {
    queue: Mutex<Option<mpsc::Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Writer {
    pub(crate) fn start(connection: Connection) -> Result<Writer, StoreError> {
        let (queue, jobs) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("kumi-store".into())
            .spawn(move || run(connection, jobs))
            .map_err(|error| StoreError::Io(error.to_string()))?;
        Ok(Writer { queue: Mutex::new(Some(queue)), thread: Mutex::new(Some(thread)) })
    }
    pub(crate) fn write<T, J, D>(&self, job: J, done: D)
    where
        T: Send + 'static,
        J: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        D: FnOnce(Result<T, StoreError>) + Send + 'static,
    {
        let job: Job = Box::new(move |connection| {
            let result = match connection {
                Err(why) => Err(why.clone()),
                // A write that panics is that write's error, rolled back like any other; the writer carries on.
                Ok(connection) => in_savepoint(connection, |connection| {
                    catch_unwind(AssertUnwindSafe(|| job(connection)))
                        .unwrap_or_else(|_| Err(StoreError::Sqlite("a write to Kumi's database failed unexpectedly".into())))
                }),
            };
            Box::new(move |commit: Option<&StoreError>| {
                done(match (result, commit) {
                    (Ok(_), Some(why)) => Err(why.clone()),
                    (result, _) => result,
                })
            })
        });
        let queue = self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(Err(mpsc::SendError(job))) = queue.map(|queue| queue.send(job)) {
            job(Err(&StoreError::Closed))(None);
        }
    }
}

impl Drop for Writer {
    /// Whatever is queued is written, then the thread ends. Dropped on the writer's own thread (the
    /// last `Store` held by a write), it ends by itself once that write is answered.
    fn drop(&mut self) {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(thread) = self.thread.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

fn run(mut connection: Connection, jobs: mpsc::Receiver<Job>) {
    let mut dirty = false;
    loop {
        let first = match jobs.recv_timeout(IDLE) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if std::mem::take(&mut dirty) {
                    let _ = connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut batch = vec![first];
        while batch.len() < MOST {
            match jobs.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }
        match connection.transaction_with_behavior(TransactionBehavior::Immediate) {
            Err(why) => {
                let why = StoreError::from(why);
                for job in batch {
                    job(Err(&why))(None);
                }
            }
            Ok(transaction) => {
                // SQLite can roll the whole transaction back mid-batch (a full disk, an I/O error): then
                // nothing of the batch is kept, and the writes still queued in it don't run, since they would
                // land outside any transaction while their callers heard of a failure.
                let mut lost: Option<StoreError> = None;
                let mut answers: Vec<Answer> = Vec::with_capacity(batch.len());
                for job in batch {
                    match &lost {
                        Some(why) => answers.push(job(Err(why))),
                        None => {
                            answers.push(job(Ok(&*transaction)));
                            if transaction.is_autocommit() {
                                lost = Some(StoreError::Sqlite("SQLite gave up the write (a full disk or a read or write error)".into()));
                            }
                        }
                    }
                }
                let committed = match lost {
                    Some(why) => Some(why),
                    None => transaction.commit().map_err(StoreError::from).err(),
                };
                for answer in answers {
                    answer(committed.as_ref());
                }
                dirty = true;
            }
        }
    }
    let _ = connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
}
