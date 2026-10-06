//! The schema, as forward-only migrations. Migration N brings the database from version N-1 to N, and
//! the version is SQLite's own `user_version`, so opening an up-to-date database reads one number.
//!
//! Conventions every table keeps (later ones too):
//! - `id TEXT PRIMARY KEY`: a ULID for a new row, or an id made from the content for an imported one
//!   (`ids`), so ids mean the same on every machine. A rowid never leaves its table.
//! - Times are INTEGER milliseconds, UTC.
//! - Who a row is about is `scope_kind` (`global`, `context`, `project`, `session`) and `scope_id`
//!   (empty for `global`, the project's id for `project`).
//! - The label the model sees (`p3`, `t12`) is a column of its own, unique among rows in use.
//! - A row set aside to make room (a full list) keeps its data with `archived_at`; a row the producer
//!   forgets is deleted, and its id kept in `forgotten` so no import brings it back.
//! - JSONB for fields still taking shape; STRICT tables.

use crate::StoreError;
use rusqlite::{Connection, TransactionBehavior};

const V1: &str = "
CREATE TABLE notes (
  id TEXT PRIMARY KEY,
  scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'project')),
  scope_id TEXT NOT NULL DEFAULT '',
  label TEXT NOT NULL,
  text TEXT NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER
) STRICT;
CREATE UNIQUE INDEX notes_in_use ON notes (scope_kind, scope_id, label) WHERE archived_at IS NULL;

CREATE TABLE techniques (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  name TEXT NOT NULL,
  fits TEXT NOT NULL,
  idea TEXT NOT NULL,
  settings TEXT,
  substitutes TEXT,
  recipe TEXT,
  source_title TEXT,
  source_url TEXT,
  request TEXT,
  used REAL NOT NULL DEFAULT 0,
  undone REAL NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER,
  last_used_at INTEGER,
  archived_at INTEGER
) STRICT;
CREATE UNIQUE INDEX techniques_in_use ON techniques (label) WHERE archived_at IS NULL;

CREATE TABLE lessons (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  matched TEXT NOT NULL,
  winner TEXT NOT NULL,
  from_score REAL NOT NULL,
  to_score REAL NOT NULL,
  moves BLOB NOT NULL,
  reaction TEXT CHECK (reaction IN ('liked', 'disliked')),
  created_at INTEGER NOT NULL,
  archived_at INTEGER
) STRICT;
CREATE UNIQUE INDEX lessons_in_use ON lessons (label) WHERE archived_at IS NULL;

CREATE TABLE gaps (
  id TEXT PRIMARY KEY,
  kumi_version TEXT NOT NULL,
  missing TEXT NOT NULL,
  asked TEXT,
  workaround TEXT,
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE imports (
  source TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  blake3 TEXT NOT NULL,
  rows INTEGER NOT NULL,
  imported_at INTEGER NOT NULL
) STRICT;

CREATE TABLE forgotten (
  id TEXT PRIMARY KEY,
  at INTEGER NOT NULL
) STRICT;
";

/// A file's base (`sync`): the file as Kumi last read or wrote it, as JSONB, for a three-way merge.
const V2: &str = "ALTER TABLE imports ADD COLUMN base BLOB;";

const MIGRATIONS: &[&str] = &[V1, V2];
/// The schema version this build writes.
pub const SCHEMA_VERSION: usize = MIGRATIONS.len();

/// Bring the database up to `SCHEMA_VERSION`, in one transaction that waits its turn, so two Kumis
/// opening a new database at once migrate it once. A newer database is left as it is.
pub(crate) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found = transaction.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?.max(0) as usize;
    if found > SCHEMA_VERSION {
        return Err(StoreError::Newer { found, known: SCHEMA_VERSION });
    }
    for migration in &MIGRATIONS[found..] {
        transaction.execute_batch(migration)?;
    }
    if found < SCHEMA_VERSION {
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION as i64)?;
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_from_the_first_schema_keeps_its_records_and_gets_bases() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(V1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute(
                "INSERT INTO imports (source, kind, size, mtime, blake3, rows, imported_at) VALUES ('/memory.json', 'notes', 2, 3, 'h', 4, 5)",
                [],
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION as i64);
        assert_eq!(
            crate::imports::record_of(&connection, "/memory.json").unwrap(),
            Some(crate::imports::Record { blake3: "h".into(), base: None }),
            "read in before bases: the next change is read in as the first time was"
        );
    }
}
