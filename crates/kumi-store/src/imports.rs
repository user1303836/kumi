//! The files earlier Kumis kept their notes, techniques, lessons and gaps in, read into the database
//! once per version of each: a file is known by its contents, so reading it again changes nothing until
//! it changes. With a notes, techniques or lessons file, Kumi keeps its base (`sync`): the file as Kumi
//! last read or wrote it.

use crate::{params, sync::BaseRow, Connection, OptionalExtension, StoreError};
use std::{path::Path, time::UNIX_EPOCH};

/// A source file as it was read: where, what it holds, and what it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    pub kind: String,
    pub size: i64,
    pub mtime: i64,
    pub blake3: String,
}

/// Read a source file whole: its fingerprint and its bytes, or None when there's no such file.
pub fn read(path: &Path, kind: &str) -> Result<Option<(Source, Vec<u8>)>, StoreError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StoreError::Io(format!("{}: {error}", path.display()))),
    };
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mtime = modified.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    let source =
        Source { path: path.to_string_lossy().into_owned(), kind: kind.into(), size: bytes.len() as i64, mtime, blake3: hash(&bytes) };
    Ok(Some((source, bytes)))
}

/// A file's contents' hash, as its record keeps it.
pub fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// What the database last read of a file (or wrote to it): the hash of its contents, and its base when
/// Kumi keeps one.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub blake3: String,
    pub base: Option<Vec<BaseRow>>,
}

/// The record of the file at `path`, if it has been read in.
pub fn record_of(connection: &Connection, path: &str) -> Result<Option<Record>, StoreError> {
    let found: Option<(String, Option<String>)> = connection
        .prepare_cached("SELECT blake3, json(base) FROM imports WHERE source = ?1")?
        .query_row(params![path], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    found
        .map(|(blake3, base)| {
            let base = base.map(|base| serde_json::from_str(&base)).transpose().map_err(|error| StoreError::Sqlite(error.to_string()))?;
            Ok(Record { blake3, base })
        })
        .transpose()
}

/// Note that this version of the file has been read in (or written), with how many rows it added and
/// its base.
pub fn record(connection: &Connection, source: &Source, rows: usize, base: Option<&[BaseRow]>, now: i64) -> Result<(), StoreError> {
    let base = base.map(serde_json::to_string).transpose().map_err(|error| StoreError::Sqlite(error.to_string()))?;
    connection
        .prepare_cached(
            "INSERT OR REPLACE INTO imports (source, kind, size, mtime, blake3, rows, imported_at, base)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, jsonb(?8))",
        )?
        .execute(params![source.path, source.kind, source.size, source.mtime, source.blake3, rows as i64, now, base])?;
    Ok(())
}

/// Whether the row with this id was forgotten (so an import leaves it out).
pub(crate) fn forgotten(connection: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(connection.prepare_cached("SELECT EXISTS (SELECT 1 FROM forgotten WHERE id = ?1)")?.query_row(params![id], |row| row.get(0))?)
}

/// Whether a row with this id is in the database, or was forgotten (so an import leaves it out).
pub(crate) fn known_or_forgotten(connection: &Connection, table: &str, id: &str) -> Result<bool, StoreError> {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE id = ?1) OR EXISTS (SELECT 1 FROM forgotten WHERE id = ?1)");
    Ok(connection.prepare_cached(&sql)?.query_row(params![id], |row| row.get(0))?)
}

/// `label`, or when another row in use has it, the next free label with its letter (`p3` taken: `p7`
/// after `p6`), so an imported row never replaces one.
pub(crate) fn free_label(in_use: &[String], label: &str) -> String {
    if !in_use.iter().any(|l| l == label) {
        return label.into();
    }
    let letter = &label[..label.char_indices().nth(1).map(|(i, _)| i).unwrap_or(label.len())];
    let highest = in_use.iter().filter_map(|l| l.strip_prefix(letter)?.parse::<u64>().ok()).max().unwrap_or(0);
    format!("{letter}{}", highest + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_label_gives_the_next_free_one_with_its_letter() {
        let in_use: Vec<String> = ["p1", "p2", "p6", "s9"].map(String::from).to_vec();
        assert_eq!(free_label(&in_use, "p3"), "p3");
        assert_eq!(free_label(&in_use, "p2"), "p7");
        assert_eq!(free_label(&in_use, "s9"), "s10");
    }

    #[test]
    fn a_file_is_known_by_its_contents_and_keeps_its_base() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("memory.json");
        assert_eq!(read(&path, "notes").unwrap(), None);
        std::fs::write(&path, br#"{"version":1,"notes":[]}"#).unwrap();
        let (source, bytes) = read(&path, "notes").unwrap().unwrap();
        assert_eq!((source.size, bytes.len()), (24, 24));
        let mut db = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut db).unwrap();
        assert_eq!(record_of(&db, &source.path).unwrap(), None);
        record(&db, &source, 0, None, 1).unwrap();
        assert_eq!(record_of(&db, &source.path).unwrap(), Some(Record { blake3: source.blake3.clone(), base: None }));
        let base = [BaseRow { label: "p1".into(), id: "01ABC".into(), hash: "h".into(), at: None, used: None, undone: None }];
        record(&db, &source, 0, Some(&base), 2).unwrap();
        assert_eq!(record_of(&db, &source.path).unwrap().unwrap().base.as_deref(), Some(&base[..]));
    }
}
