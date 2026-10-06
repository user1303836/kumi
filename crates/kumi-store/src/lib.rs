//! Kumi's database, `~/.kumi/kumi.db`: what Kumi keeps between sessions (notes, techniques, lessons,
//! gaps), in SQLite.
//!
//! One thread writes, committing what's queued together; reads run beside it on their own
//! connections. Kumi's runtime is a single thread, so nothing here is called from it directly: writes
//! are queued with a callback for their result, and reads run on a blocking thread. Synchronous
//! throughout; the async side lives in the runtime.

pub mod gaps;
pub mod ids;
pub mod imports;
pub mod lessons;
pub mod notes;
mod reader;
mod schema;
pub mod sync;
pub mod techniques;
mod writer;

pub use rusqlite::{params, Connection, OptionalExtension};
pub use schema::SCHEMA_VERSION;
pub use sync::{BaseRow, Kept};

use reader::Readers;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use writer::Writer;

/// How long a write or read waits for another process (another Kumi) to finish its own write.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections that only read, beside the one that writes.
const READERS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Sqlite(String),
    #[error("Kumi's database: {0}")]
    Io(String),
    #[error("Kumi's database is from a newer Kumi (schema {found}; this one knows {known})")]
    Newer { found: usize, known: usize },
    #[error("Kumi's database has stopped writing")]
    Closed,
}
impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Sqlite(error.to_string())
    }
}

/// Who a row is about: everything (the producer), a kind of project, one Set, or one session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Context(String),
    Project(String),
    Session(String),
}
impl Scope {
    pub fn kind(&self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Context(_) => "context",
            Scope::Project(_) => "project",
            Scope::Session(_) => "session",
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Scope::Global => "",
            Scope::Context(id) | Scope::Project(id) | Scope::Session(id) => id,
        }
    }
}

/// The open database: cheap to clone and to share between threads.
#[derive(Clone)]
pub struct Store(Arc<Inner>);
struct Inner {
    path: PathBuf,
    writer: Writer,
    readers: Readers,
}

impl Store {
    /// Open the database at `path`, making it (and its folder) if needed, and bring its schema up to
    /// date. Another Kumi may have it open too. A database from a newer Kumi isn't opened.
    pub fn open(path: impl AsRef<Path>) -> Result<Store, StoreError> {
        let path = path.as_ref().to_path_buf();
        prepare_file(&path)?;
        let mut connection = Connection::open(&path)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        // Write-ahead logging: readers and the writer don't wait for each other, and a commit is an append.
        let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Io(format!("SQLite kept journal mode {mode} instead of WAL (is the folder on a network drive?)")));
        }
        // An app crash never loses a commit with NORMAL; only a power cut can lose the last few.
        connection.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;")?;
        schema::migrate(&mut connection)?;
        let readers = Readers::open(&path, READERS)?;
        Ok(Store(Arc::new(Inner { writer: Writer::start(connection)?, readers, path })))
    }
    pub fn path(&self) -> &Path {
        &self.0.path
    }
    /// Queue a write. It runs on the writer thread, in a transaction with whatever else is queued, and
    /// `done` gets its result once that transaction commits: an error in one write rolls back only it.
    pub fn write<T, J, D>(&self, job: J, done: D)
    where
        T: Send + 'static,
        J: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        D: FnOnce(Result<T, StoreError>) + Send + 'static,
    {
        self.0.writer.write(job, done)
    }
    /// A write, waiting for its commit: for startup, tests and blocking threads, never Kumi's own.
    pub fn write_wait<T, J>(&self, job: J) -> Result<T, StoreError>
    where
        T: Send + 'static,
        J: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.write(job, move |result| {
            let _ = sender.send(result);
        });
        receiver.recv().unwrap_or(Err(StoreError::Closed))
    }
    /// Run a read on one of the read-only connections, waiting for one to be free. Blocking: from Kumi's
    /// runtime it goes through a blocking thread.
    pub fn read<T>(&self, job: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
        self.0.readers.read(job)
    }
}

/// Read the database at `path` as it is, writing nothing (no migration, no writer, a `query_only`
/// connection) and never waiting on another Kumi's write: for `kumi report`. A database from a newer
/// Kumi isn't read.
pub fn read_only<T>(path: impl AsRef<Path>, job: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    connection.pragma_update(None, "query_only", true)?;
    let found = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?.max(0) as usize;
    if found > SCHEMA_VERSION {
        return Err(StoreError::Newer { found, known: SCHEMA_VERSION });
    }
    job(&connection)
}

/// The database's folder (only the producer's) and file (only theirs): SQLite gives the WAL files the
/// file's own permissions.
fn prepare_file(path: &Path) -> Result<(), StoreError> {
    let io = |error: std::io::Error| StoreError::Io(error.to_string());
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(folder).map_err(io)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path).map(drop).map_err(io)
}

/// Run `job` inside a savepoint of the current transaction: its error rolls back only its own changes.
pub(crate) fn in_savepoint<T>(connection: &Connection, job: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
    connection.execute_batch("SAVEPOINT job")?;
    match job(connection) {
        Ok(value) => {
            connection.execute_batch("RELEASE job")?;
            Ok(value)
        }
        Err(error) => {
            connection.execute_batch("ROLLBACK TO job; RELEASE job")?;
            Err(error)
        }
    }
}
