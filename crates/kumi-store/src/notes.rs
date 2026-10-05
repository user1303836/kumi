//! Notes in the producer's words: about them (`Scope::Global`) or about one Set (`Scope::Project`).

use crate::{
    ids::{content_id, new_id},
    imports, params, Connection, OptionalExtension, Scope, StoreError,
};
use std::collections::HashMap;

/// A note in use: its label (`p3`, `s1`), words, whether it's pinned, and when it was last written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub label: String,
    pub text: String,
    pub pinned: bool,
    pub at: i64,
}

/// The scope's notes in use, oldest first.
pub fn in_use(connection: &Connection, scope: &Scope) -> Result<Vec<Note>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT label, text, pinned, updated_at FROM notes
         WHERE scope_kind = ?1 AND scope_id = ?2 AND archived_at IS NULL ORDER BY updated_at, label",
    )?;
    let rows = statement.query_map(params![scope.kind(), scope.id()], |row| {
        Ok(Note { label: row.get(0)?, text: row.get(1)?, pinned: row.get(2)?, at: row.get(3)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The scope's notes in use become `notes`: a new label is added, a known one takes the new words and
/// pin, and a note in use that isn't among them is set aside (archived, as a full list makes room),
/// never deleted.
pub fn keep(connection: &Connection, scope: &Scope, notes: &[Note], now: i64) -> Result<(), StoreError> {
    let mut known: HashMap<String, (String, String, bool, i64)> = HashMap::new();
    {
        let mut statement = connection.prepare_cached(
            "SELECT label, id, text, pinned, updated_at FROM notes WHERE scope_kind = ?1 AND scope_id = ?2 AND archived_at IS NULL",
        )?;
        let rows = statement.query_map(params![scope.kind(), scope.id()], |row| {
            Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        })?;
        for row in rows {
            let (label, rest) = row?;
            known.insert(label, rest);
        }
    }
    for note in notes {
        match known.remove(&note.label) {
            Some((_, text, pinned, at)) if text == note.text && pinned == note.pinned && at == note.at => {}
            Some((id, ..)) => {
                connection.prepare_cached("UPDATE notes SET text = ?2, pinned = ?3, updated_at = ?4 WHERE id = ?1")?.execute(params![
                    id,
                    note.text,
                    note.pinned,
                    note.at
                ])?;
            }
            None => {
                connection
                    .prepare_cached(
                        "INSERT INTO notes (id, scope_kind, scope_id, label, text, pinned, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    )?
                    .execute(params![new_id(), scope.kind(), scope.id(), note.label, note.text, note.pinned, note.at])?;
            }
        }
    }
    for (id, ..) in known.into_values() {
        connection.prepare_cached("UPDATE notes SET archived_at = ?2 WHERE id = ?1")?.execute(params![id, now])?;
    }
    Ok(())
}

/// Forget a note the producer no longer wants: deleted, and remembered as forgotten so no import brings
/// it back. Whether there was one.
pub fn forget(connection: &Connection, scope: &Scope, label: &str, now: i64) -> Result<bool, StoreError> {
    let id: Option<String> = connection
        .prepare_cached("SELECT id FROM notes WHERE scope_kind = ?1 AND scope_id = ?2 AND label = ?3 AND archived_at IS NULL")?
        .query_row(params![scope.kind(), scope.id(), label], |row| row.get(0))
        .optional()?;
    let Some(id) = id else { return Ok(false) };
    connection.prepare_cached("DELETE FROM notes WHERE id = ?1")?.execute(params![id])?;
    connection.prepare_cached("INSERT OR REPLACE INTO forgotten (id, at) VALUES (?1, ?2)")?.execute(params![id, now])?;
    Ok(true)
}

/// Read in a note an earlier Kumi kept in a file. Its id comes from its scope, label and words, so the
/// same note is read in once and one forgotten here stays forgotten; a note in use with that label and
/// those words (one written back for an older Kumi) is the same note. A label another note in use has
/// gets the next free one. Whether it was added.
pub fn import(connection: &Connection, scope: &Scope, note: &Note) -> Result<bool, StoreError> {
    let id = content_id(&["note", scope.kind(), scope.id(), &note.label, &note.text]);
    if imports::known_or_forgotten(connection, "notes", &id)?
        || connection
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM notes WHERE scope_kind = ?1 AND scope_id = ?2 AND label = ?3 AND text = ?4 AND archived_at IS NULL)",
            )?
            .query_row(params![scope.kind(), scope.id(), note.label, note.text], |row| row.get::<_, bool>(0))?
    {
        return Ok(false);
    }
    let in_use: Vec<String> = connection
        .prepare_cached("SELECT label FROM notes WHERE scope_kind = ?1 AND scope_id = ?2 AND archived_at IS NULL")?
        .query_map(params![scope.kind(), scope.id()], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    connection
        .prepare_cached(
            "INSERT INTO notes (id, scope_kind, scope_id, label, text, pinned, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        )?
        .execute(params![id, scope.kind(), scope.id(), imports::free_label(&in_use, &note.label), note.text, note.pinned, note.at])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut connection).unwrap();
        connection
    }
    fn note(label: &str, text: &str, at: i64) -> Note {
        Note { label: label.into(), text: text.into(), pinned: false, at }
    }

    #[test]
    fn a_scope_keeps_exactly_its_notes_and_sets_aside_what_it_drops() {
        let db = database();
        let producer = Scope::Global;
        let set = Scope::Project("0123456789abcdef0123456789abcdef".into());
        keep(&db, &producer, &[note("p1", "Likes short reverbs", 10), note("p2", "Names drums in caps", 20)], 30).unwrap();
        keep(&db, &set, &[note("s1", "The Reese is the main bass", 15)], 30).unwrap();
        assert_eq!(in_use(&db, &producer).unwrap(), [note("p1", "Likes short reverbs", 10), note("p2", "Names drums in caps", 20)]);
        assert_eq!(in_use(&db, &set).unwrap(), [note("s1", "The Reese is the main bass", 15)]);

        // A full list makes room: p1 is set aside, p2 changes words and pin, p3 is new.
        let mut p2 = note("p2", "Names drums in capitals", 40);
        p2.pinned = true;
        keep(&db, &producer, &[p2.clone(), note("p3", "Works at 140", 41)], 50).unwrap();
        assert_eq!(in_use(&db, &producer).unwrap(), [p2, note("p3", "Works at 140", 41)]);
        let archived: (String, i64) =
            db.query_row("SELECT text, archived_at FROM notes WHERE label = 'p1'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(archived, ("Likes short reverbs".into(), 50), "set aside, not deleted");
        assert_eq!(in_use(&db, &set).unwrap().len(), 1, "another scope is untouched");

        // A label set aside can be used again.
        keep(&db, &producer, &[note("p1", "Mixes on headphones", 60)], 70).unwrap();
        assert_eq!(in_use(&db, &producer).unwrap(), [note("p1", "Mixes on headphones", 60)]);
    }

    #[test]
    fn a_forgotten_note_is_deleted_and_remembered_as_forgotten() {
        let db = database();
        keep(&db, &Scope::Global, &[note("p1", "Hates sidechain pumping", 1)], 2).unwrap();
        let id: String = db.query_row("SELECT id FROM notes", [], |row| row.get(0)).unwrap();
        assert!(forget(&db, &Scope::Global, "p1", 3).unwrap());
        assert!(!forget(&db, &Scope::Global, "p1", 4).unwrap(), "nothing left to forget");
        assert_eq!(db.query_row("SELECT count(*) FROM notes", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(db.query_row("SELECT id FROM forgotten", [], |row| row.get::<_, String>(0)).unwrap(), id);
    }

    #[test]
    fn notes_belong_to_the_producer_or_a_set_only() {
        let db = database();
        assert!(keep(&db, &Scope::Context("footwork".into()), &[note("c1", "x", 1)], 2).is_err());
    }
}
