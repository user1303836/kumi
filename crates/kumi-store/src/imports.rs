//! The files earlier Kumis kept their notes, techniques, lessons and gaps in, read into the database
//! once per version of each: a file is known by its size, modified time and contents, so reading it
//! again changes nothing until it changes. The files themselves are never written.

use crate::{params, Connection, OptionalExtension, StoreError};
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
    let source = Source {
        path: path.to_string_lossy().into_owned(),
        kind: kind.into(),
        size: bytes.len() as i64,
        mtime,
        blake3: blake3::hash(&bytes).to_hex().to_string(),
    };
    Ok(Some((source, bytes)))
}

/// Whether this version of the file has been read in already.
pub fn imported(connection: &Connection, source: &Source) -> Result<bool, StoreError> {
    let known: Option<(i64, i64, String)> = connection
        .prepare_cached("SELECT size, mtime, blake3 FROM imports WHERE source = ?1")?
        .query_row(params![source.path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    Ok(known.is_some_and(|(size, mtime, hash)| size == source.size && mtime == source.mtime && hash == source.blake3))
}

/// Note that this version of the file has been read in, with how many rows it added.
pub fn record(connection: &Connection, source: &Source, rows: usize, now: i64) -> Result<(), StoreError> {
    connection
        .prepare_cached(
            "INSERT OR REPLACE INTO imports (source, kind, size, mtime, blake3, rows, imported_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?
        .execute(params![source.path, source.kind, source.size, source.mtime, source.blake3, rows as i64, now])?;
    Ok(())
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
    fn a_file_is_known_by_its_size_time_and_contents() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("memory.json");
        assert_eq!(read(&path, "notes").unwrap(), None);
        std::fs::write(&path, br#"{"version":1,"notes":[]}"#).unwrap();
        let (source, bytes) = read(&path, "notes").unwrap().unwrap();
        assert_eq!((source.size, bytes.len()), (24, 24));
        let mut db = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut db).unwrap();
        assert!(!imported(&db, &source).unwrap());
        record(&db, &source, 0, 1).unwrap();
        assert!(imported(&db, &source).unwrap());
        let changed = Source { blake3: "other".into(), ..source };
        assert!(!imported(&db, &changed).unwrap(), "new contents are read again");
    }
}
