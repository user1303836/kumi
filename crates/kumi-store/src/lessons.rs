//! Lessons from matching sounds: what was matched, what won, the scores, the moves that helped.

use crate::{
    ids::{content_id, new_id},
    imports, params,
    sync::{self, BaseRow, Kept, Table},
    Connection, OptionalExtension, StoreError,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Lesson {
    pub label: String,
    pub matched: String,
    pub winner: String,
    pub from: f64,
    pub to: f64,
    /// The moves that improved it, as JSON (kept as JSONB: their shape may still change).
    pub moves: Value,
    /// `liked` or `disliked`, when the producer said.
    pub reaction: Option<String>,
    pub at: i64,
}

const COLUMNS: &str = "label, matched, winner, from_score, to_score, json(moves), reaction, created_at";

/// A lesson as `COLUMNS` read it, its moves still text.
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Lesson, String)> {
    Ok((
        Lesson {
            label: row.get(0)?,
            matched: row.get(1)?,
            winner: row.get(2)?,
            from: row.get(3)?,
            to: row.get(4)?,
            moves: Value::Null,
            reaction: row.get(6)?,
            at: row.get(7)?,
        },
        row.get(5)?,
    ))
}
fn with_moves((mut lesson, moves): (Lesson, String)) -> Result<Lesson, StoreError> {
    lesson.moves = serde_json::from_str(&moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
    Ok(lesson)
}

/// The lessons in use, oldest first.
pub fn in_use(connection: &Connection) -> Result<Vec<Lesson>, StoreError> {
    let mut statement =
        connection.prepare_cached(&format!("SELECT {COLUMNS} FROM lessons WHERE archived_at IS NULL ORDER BY created_at, rowid"))?;
    let rows = statement.query_map([], row)?;
    rows.map(|found| with_moves(found?)).collect()
}

/// The lessons in use become `lessons`: new ones are added, known ones updated, and one in use that
/// isn't among them is set aside (archived), never deleted.
pub fn keep(connection: &Connection, lessons: &[Lesson], now: i64) -> Result<(), StoreError> {
    let mut known: HashMap<String, String> = HashMap::new();
    {
        let mut statement = connection.prepare_cached("SELECT label, id FROM lessons WHERE archived_at IS NULL")?;
        for found in statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (label, id) = found?;
            known.insert(label, id);
        }
    }
    for l in lessons {
        let moves = serde_json::to_string(&l.moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
        match known.remove(&l.label) {
            Some(id) => connection
                .prepare_cached(
                    "UPDATE lessons SET matched = ?2, winner = ?3, from_score = ?4, to_score = ?5, moves = jsonb(?6), reaction = ?7,
                     created_at = ?8 WHERE id = ?1",
                )?
                .execute(params![id, l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?,
            None => connection
                .prepare_cached(
                    "INSERT INTO lessons (id, label, matched, winner, from_score, to_score, moves, reaction, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, jsonb(?7), ?8, ?9)",
                )?
                .execute(params![new_id(), l.label, l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?,
        };
    }
    for id in known.into_values() {
        connection.prepare_cached("UPDATE lessons SET archived_at = ?2 WHERE id = ?1")?.execute(params![id, now])?;
    }
    Ok(())
}

/// Add a lesson, or keep a known one (by label, among those in use) anew, last; then, past `most` in use,
/// the oldest are set aside. Whether it was known.
pub fn put(connection: &Connection, l: &Lesson, most: usize, now: i64) -> Result<bool, StoreError> {
    let moves = serde_json::to_string(&l.moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
    let known = connection
        .prepare_cached(
            "UPDATE lessons SET matched = ?2, winner = ?3, from_score = ?4, to_score = ?5, moves = jsonb(?6), reaction = ?7, created_at = ?8,
             rowid = (SELECT max(rowid) + 1 FROM lessons) WHERE label = ?1 AND archived_at IS NULL",
        )?
        .execute(params![l.label, l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?
        > 0;
    if !known {
        connection
            .prepare_cached(
                "INSERT INTO lessons (id, label, matched, winner, from_score, to_score, moves, reaction, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, jsonb(?7), ?8, ?9)",
            )?
            .execute(params![new_id(), l.label, l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?;
    }
    connection
        .prepare_cached(
            "UPDATE lessons SET archived_at = ?2 WHERE id IN (SELECT id FROM lessons WHERE archived_at IS NULL
             ORDER BY created_at DESC, rowid DESC LIMIT -1 OFFSET ?1)",
        )?
        .execute(params![most as i64, now])?;
    Ok(known)
}

/// The producer's reaction to a lesson's result. Whether there was one in use.
pub fn react(connection: &Connection, label: &str, reaction: &str) -> Result<bool, StoreError> {
    Ok(connection
        .prepare_cached("UPDATE lessons SET reaction = ?2 WHERE label = ?1 AND archived_at IS NULL")?
        .execute(params![label, reaction])?
        > 0)
}

/// Forget a lesson: deleted, and remembered as forgotten so no import brings it back. Whether there was
/// one in use.
pub fn forget(connection: &Connection, label: &str, now: i64) -> Result<bool, StoreError> {
    let id: Option<String> = connection
        .prepare_cached("SELECT id FROM lessons WHERE label = ?1 AND archived_at IS NULL")?
        .query_row(params![label], |row| row.get(0))
        .optional()?;
    let Some(id) = id else { return Ok(false) };
    connection.prepare_cached("DELETE FROM lessons WHERE id = ?1")?.execute(params![id])?;
    connection.prepare_cached("INSERT OR REPLACE INTO forgotten (id, at) VALUES (?1, ?2)")?.execute(params![id, now])?;
    Ok(true)
}

/// Read in a lesson an earlier Kumi kept in a file, once: its id comes from its label and time, one
/// forgotten here stays forgotten and one set aside here comes back in use, and a lesson in use with its
/// label (written back for an older Kumi) is the same lesson. The id of the lesson that holds it, and
/// whether it was added or came back.
pub fn import(connection: &Connection, l: &Lesson) -> Result<(String, bool), StoreError> {
    let id = content_id(&["lesson", &l.label, &l.at.to_string()]);
    if imports::forgotten(connection, &id)? {
        return Ok((id, false));
    }
    if let Some((_, archived)) = Lessons.get(connection, &id)? {
        if archived {
            Lessons.overwrite(connection, &id, l)?;
        }
        return Ok((id, archived));
    }
    if let Some(same) = Lessons.in_use(connection, &l.label)? {
        return Ok((same, false));
    }
    insert_as(connection, &id, l)?;
    Ok((id, true))
}

/// Add `l` with this id, under its label or, when another lesson in use has it, the next free one.
fn insert_as(connection: &Connection, id: &str, l: &Lesson) -> Result<(), StoreError> {
    let moves = serde_json::to_string(&l.moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
    connection
        .prepare_cached(
            "INSERT INTO lessons (id, label, matched, winner, from_score, to_score, moves, reaction, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, jsonb(?7), ?8, ?9)",
        )?
        .execute(params![id, free_label(connection, &l.label)?, l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?;
    Ok(())
}
/// `label`, or when another lesson in use has it, a new label of the lessons' own form, `l` and 8 hex digits as Kumi
/// makes them, that none in use has: a numbered one (`l1`) is a label Kumi never shows, and the lessons' file would
/// be written back every time.
fn free_label(connection: &Connection, label: &str) -> Result<String, StoreError> {
    let in_use: HashSet<String> = connection
        .prepare_cached("SELECT label FROM lessons WHERE archived_at IS NULL")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    if !in_use.contains(label) {
        return Ok(label.into());
    }
    loop {
        let minted = format!("l{:08x}", rand::random::<u32>());
        if !in_use.contains(&minted) {
            return Ok(minted);
        }
    }
}

/// A hash of what a lesson says: everything but when (`sync`).
pub fn hash(l: &Lesson) -> String {
    content_id(&[
        "lesson",
        &l.matched,
        &l.winner,
        &l.from.to_string(),
        &l.to.to_string(),
        &l.moves.to_string(),
        l.reaction.as_deref().unwrap_or(""),
    ])
}
/// A lessons file read in with no base yet: the file's base, and how many were added.
pub fn read_in(connection: &Connection, file: &[Lesson]) -> Result<(Vec<BaseRow>, usize), StoreError> {
    sync::read_in(connection, &Lessons, file)
}
/// What changed in a lessons file since `base`, brought in (`sync::merge`).
pub fn merge(connection: &Connection, base: &[BaseRow], file: &[Lesson], now: i64) -> Result<(Vec<BaseRow>, Kept), StoreError> {
    sync::merge(connection, &Lessons, base, file, now)
}
/// The base of a file just written with the lessons in use.
pub fn base_of(connection: &Connection, file: &[Lesson]) -> Result<Vec<BaseRow>, StoreError> {
    sync::base_of(connection, &Lessons, file)
}

/// The lessons, as `sync` reaches them.
struct Lessons;
impl Table for Lessons {
    type Row = Lesson;
    fn label<'a>(&self, row: &'a Lesson) -> &'a str {
        &row.label
    }
    fn hash(&self, row: &Lesson) -> String {
        hash(row)
    }
    fn get(&self, c: &Connection, id: &str) -> Result<Option<(Lesson, bool)>, StoreError> {
        let found = c
            .prepare_cached(&format!("SELECT {COLUMNS}, archived_at IS NOT NULL FROM lessons WHERE id = ?1"))?
            .query_row(params![id], |found| Ok((row(found)?, found.get::<_, bool>(8)?)))
            .optional()?;
        found.map(|(lesson, archived)| Ok((with_moves(lesson)?, archived))).transpose()
    }
    fn in_use(&self, c: &Connection, label: &str) -> Result<Option<String>, StoreError> {
        Ok(c.prepare_cached("SELECT id FROM lessons WHERE label = ?1 AND archived_at IS NULL")?
            .query_row(params![label], |row| row.get(0))
            .optional()?)
    }
    fn overwrite(&self, c: &Connection, id: &str, l: &Lesson) -> Result<(), StoreError> {
        let Some((here, archived)) = self.get(c, id)? else { return Ok(()) };
        let label = if archived { free_label(c, &here.label)? } else { here.label };
        let moves = serde_json::to_string(&l.moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
        c.prepare_cached(
            "UPDATE lessons SET label = ?2, matched = ?3, winner = ?4, from_score = ?5, to_score = ?6, moves = jsonb(?7), reaction = ?8,
             created_at = ?9, archived_at = NULL WHERE id = ?1",
        )?
        .execute(params![id, label, l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?;
        Ok(())
    }
    fn add(&self, c: &Connection, l: &Lesson) -> Result<String, StoreError> {
        let id = new_id();
        insert_as(c, &id, l)?;
        Ok(id)
    }
    fn import(&self, c: &Connection, l: &Lesson) -> Result<(String, bool), StoreError> {
        import(c, l)
    }
    fn archive(&self, c: &Connection, id: &str, now: i64) -> Result<bool, StoreError> {
        Ok(c.prepare_cached("UPDATE lessons SET archived_at = ?2 WHERE id = ?1 AND archived_at IS NULL")?.execute(params![id, now])? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lessons_round_trip_with_their_moves_and_a_dropped_one_is_set_aside() {
        let mut db = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut db).unwrap();
        let lesson = |label: &str, at| Lesson {
            label: label.into(),
            matched: "the reference pad".into(),
            winner: "Wavetable".into(),
            from: 41.0,
            to: 77.0,
            moves: json!([{"label":"brighter","score":63},{"label":"slower attack","score":77}]),
            reaction: Some("liked".into()),
            at,
        };
        keep(&db, &[lesson("l0a1b2c3d", 1), lesson("l4e5f6a7b", 2)], 3).unwrap();
        assert_eq!(in_use(&db).unwrap(), [lesson("l0a1b2c3d", 1), lesson("l4e5f6a7b", 2)]);
        keep(&db, &[lesson("l4e5f6a7b", 2)], 4).unwrap();
        assert_eq!(in_use(&db).unwrap(), [lesson("l4e5f6a7b", 2)]);
        assert_eq!(db.query_row("SELECT count(*) FROM lessons", [], |row| row.get::<_, i64>(0)).unwrap(), 2, "set aside, not deleted");
    }

    #[test]
    fn a_lesson_kept_again_under_a_taken_label_gets_a_label_kumi_shows() {
        // A lesson changed in the file and here since the last merge is kept twice: the second can't have the
        // first's label, and a numbered one ("l1") isn't one Kumi reads.
        let mut db = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut db).unwrap();
        let lesson = Lesson {
            label: "l0a1b2c3d".into(),
            matched: "the pad".into(),
            winner: "Drift".into(),
            from: 41.0,
            to: 77.0,
            moves: json!([]),
            reaction: None,
            at: 1,
        };
        keep(&db, std::slice::from_ref(&lesson), 2).unwrap();
        assert_eq!(free_label(&db, "l4e5f6a7b").unwrap(), "l4e5f6a7b", "a free one stays");
        Lessons.add(&db, &Lesson { winner: "Wavetable".into(), ..lesson.clone() }).unwrap();
        let labels: Vec<String> = in_use(&db).unwrap().into_iter().map(|l| l.label).collect();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0], "l0a1b2c3d");
        let minted = &labels[1];
        assert!(
            minted.len() == 9 && minted.starts_with('l') && minted[1..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{minted}"
        );
        assert_ne!(minted, "l0a1b2c3d");
    }
}
