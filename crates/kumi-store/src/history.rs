//! A Set's history, `~/.kumi/projects/<project>/history.db`, a file of its own beside the project's other files.
//!
//! - **Objects** are what Kumi keeps of the Set: a clip's notes and settings now, later a device's parameters and
//!   the snapshots that tie them together. Each is its canonical JSON (keys sorted), addressed by blake3 over its
//!   kind, encoding version and that JSON, stored zstd-compressed, and never rewritten: a new encoding takes a new
//!   version, so the same content always has the same address.
//! - **Ops** say what happened (a change that cut clips, later a snapshot or a restore), with a JSONB view of what
//!   it holds.
//! - **Snapshots** and **refs** (the Set's state at a point, and named pointers to them) come with checkpoints.

use crate::{ids, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

const V1: &str = "
CREATE TABLE objects (
  hash TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  raw_len INTEGER NOT NULL,
  z BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE ops (
  id TEXT PRIMARY KEY,
  parent TEXT,
  at INTEGER NOT NULL,
  kind TEXT NOT NULL,
  summary TEXT NOT NULL,
  view BLOB NOT NULL
) STRICT;
CREATE INDEX ops_at ON ops (at);

CREATE TABLE snapshots (
  id TEXT PRIMARY KEY,
  root TEXT NOT NULL,
  parent TEXT,
  parent2 TEXT,
  op TEXT NOT NULL,
  at INTEGER NOT NULL,
  label TEXT
) STRICT;

CREATE TABLE refs (
  name TEXT PRIMARY KEY,
  snapshot TEXT NOT NULL
) STRICT;
";

/// history.db's schema, as forward-only migrations (see `schema`).
pub(crate) const MIGRATIONS: &[&str] = &[V1];
/// The schema version this build writes.
pub const HISTORY_SCHEMA_VERSION: usize = MIGRATIONS.len();
/// zstd's level: fast, and small enough for notes (storage.md measured level 3).
const LEVEL: i32 = 3;
/// The most an object decompresses to: a clip of 100k notes is well under it.
const MAX_RAW: usize = 64 * 1024 * 1024;

/// An object ready to keep: its address, kind and canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub hash: String,
    pub kind: String,
    pub raw: String,
}

impl Object {
    /// `value` as an object of `kind` in encoding `version`.
    pub fn new(kind: &str, version: u8, value: &Value) -> Object {
        let raw = canonical(value);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(kind.len() as u64).to_le_bytes());
        hasher.update(kind.as_bytes());
        hasher.update(&[version]);
        hasher.update(raw.as_bytes());
        Object { hash: hasher.finalize().to_hex().to_string(), kind: kind.into(), raw }
    }
}

/// JSON with every object's keys sorted, so the same content always reads the same.
pub fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|key| (key.clone(), sorted(&map[key]))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).unwrap_or_default()
}

/// Keep objects; one already kept is left as it is. How many were new.
pub fn put_objects(connection: &Connection, objects: &[Object]) -> Result<usize, StoreError> {
    let mut statement = connection.prepare_cached("INSERT OR IGNORE INTO objects (hash, kind, raw_len, z) VALUES (?1, ?2, ?3, ?4)")?;
    let mut added = 0;
    for object in objects {
        let z = zstd::bulk::compress(object.raw.as_bytes(), LEVEL).map_err(|error| StoreError::Io(error.to_string()))?;
        added += statement.execute(params![object.hash, object.kind, object.raw.len() as i64, z])?;
    }
    Ok(added)
}

/// The object at `hash`: its kind and JSON.
pub fn object(connection: &Connection, hash: &str) -> Result<Option<(String, Value)>, StoreError> {
    let row: Option<(String, i64, Vec<u8>)> = connection
        .query_row("SELECT kind, raw_len, z FROM objects WHERE hash = ?1", [hash], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((kind, raw_len, z)) = row else { return Ok(None) };
    let capacity =
        usize::try_from(raw_len).ok().filter(|len| *len <= MAX_RAW).ok_or_else(|| StoreError::Io("an object is too big".into()))?;
    let raw = zstd::bulk::decompress(&z, capacity).map_err(|error| StoreError::Io(error.to_string()))?;
    let value = serde_json::from_slice(&raw).map_err(|error| StoreError::Io(error.to_string()))?;
    Ok(Some((kind, value)))
}

/// What happened, and what it holds (`view`).
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub id: String,
    pub parent: Option<String>,
    pub at: i64,
    pub kind: String,
    pub summary: String,
    pub view: Value,
}

impl Op {
    /// A new op, after `parent`.
    pub fn new(parent: Option<String>, at: i64, kind: &str, summary: &str, view: Value) -> Op {
        Op { id: ids::new_id(), parent, at, kind: kind.into(), summary: summary.into(), view }
    }
}

/// Keep an op (an op already kept is left as it is).
pub fn put_op(connection: &Connection, op: &Op) -> Result<bool, StoreError> {
    let view = serde_json::to_string(&op.view).map_err(|error| StoreError::Io(error.to_string()))?;
    let added = connection.execute(
        "INSERT OR IGNORE INTO ops (id, parent, at, kind, summary, view) VALUES (?1, ?2, ?3, ?4, ?5, jsonb(?6))",
        params![op.id, op.parent, op.at, op.kind, op.summary, view],
    )?;
    Ok(added == 1)
}

/// The latest ops, newest first.
pub fn recent_ops(connection: &Connection, limit: usize) -> Result<Vec<Op>, StoreError> {
    let mut statement =
        connection.prepare_cached("SELECT id, parent, at, kind, summary, json(view) FROM ops ORDER BY at DESC, id DESC LIMIT ?1")?;
    let rows = statement.query_map([limit as i64], |row| {
        let view: String = row.get(5)?;
        Ok(Op {
            id: row.get(0)?,
            parent: row.get(1)?,
            at: row.get(2)?,
            kind: row.get(3)?,
            summary: row.get(4)?,
            view: serde_json::from_str(&view).unwrap_or(Value::Null),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_same_content_has_one_address_whatever_its_key_order() {
        let a = Object::new("clip", 1, &json!({"name":"Bass","notes":[[60,0.0,1.0]],"looping":true}));
        let b = Object::new("clip", 1, &json!({"looping":true,"notes":[[60,0.0,1.0]],"name":"Bass"}));
        assert_eq!(a, b);
        assert_eq!(a.raw, r#"{"looping":true,"name":"Bass","notes":[[60,0.0,1.0]]}"#);
        // The kind and encoding version are part of the address.
        assert_ne!(a.hash, Object::new("clip", 2, &json!({"name":"Bass","notes":[[60,0.0,1.0]],"looping":true})).hash);
        assert_ne!(a.hash, Object::new("device", 1, &json!({"name":"Bass","notes":[[60,0.0,1.0]],"looping":true})).hash);
    }
}
