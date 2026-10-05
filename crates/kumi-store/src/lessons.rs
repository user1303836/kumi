//! Lessons from matching sounds: what was matched, what won, the scores, the moves that helped.

use crate::{
    ids::{content_id, new_id},
    imports, params, Connection, OptionalExtension, StoreError,
};
use serde_json::Value;
use std::collections::HashMap;

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

/// The lessons in use, oldest first.
pub fn in_use(connection: &Connection) -> Result<Vec<Lesson>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT label, matched, winner, from_score, to_score, json(moves), reaction, created_at FROM lessons
         WHERE archived_at IS NULL ORDER BY created_at, rowid",
    )?;
    let rows = statement.query_map([], |row| {
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
            row.get::<_, String>(5)?,
        ))
    })?;
    rows.map(|found| {
        let (mut lesson, moves) = found?;
        lesson.moves = serde_json::from_str(&moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
        Ok(lesson)
    })
    .collect()
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

/// Read in a lesson an earlier Kumi kept in a file, once: its id comes from its label and time, and a
/// lesson in use with its label (written back for an older Kumi) is the same lesson. Whether it was
/// added.
pub fn import(connection: &Connection, l: &Lesson) -> Result<bool, StoreError> {
    let id = content_id(&["lesson", &l.label, &l.at.to_string()]);
    if imports::known_or_forgotten(connection, "lessons", &id)?
        || connection
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM lessons WHERE label = ?1 AND archived_at IS NULL)")?
            .query_row(params![l.label], |row| row.get::<_, bool>(0))?
    {
        return Ok(false);
    }
    let in_use: Vec<String> = connection
        .prepare_cached("SELECT label FROM lessons WHERE archived_at IS NULL")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let moves = serde_json::to_string(&l.moves).map_err(|error| StoreError::Sqlite(error.to_string()))?;
    connection
        .prepare_cached(
            "INSERT INTO lessons (id, label, matched, winner, from_score, to_score, moves, reaction, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, jsonb(?7), ?8, ?9)",
        )?
        .execute(params![id, imports::free_label(&in_use, &l.label), l.matched, l.winner, l.from, l.to, moves, l.reaction, l.at])?;
    Ok(true)
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
}
