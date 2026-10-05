//! Sound-building techniques the producer chose to keep.

use crate::{
    ids::{content_id, new_id},
    imports, params, Connection, OptionalExtension, StoreError,
};
use std::collections::HashMap;

/// A technique in use, as Kumi keeps it: its label (`t12`), what it is, and how it has fared.
#[derive(Debug, Clone, PartialEq)]
pub struct Technique {
    pub label: String,
    pub name: String,
    pub fits: String,
    pub idea: String,
    pub settings: Option<String>,
    pub substitutes: Option<String>,
    pub recipe: Option<String>,
    pub source_title: Option<String>,
    pub source_url: Option<String>,
    /// What the producer asked for when Kumi built it, in their words.
    pub request: Option<String>,
    pub used: f64,
    pub undone: f64,
    pub at: i64,
    pub updated: Option<i64>,
    pub last_used: Option<i64>,
}

const COLUMNS: &str =
    "label, name, fits, idea, settings, substitutes, recipe, source_title, source_url, request, used, undone, created_at, updated_at, last_used_at";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Technique> {
    Ok(Technique {
        label: row.get(0)?,
        name: row.get(1)?,
        fits: row.get(2)?,
        idea: row.get(3)?,
        settings: row.get(4)?,
        substitutes: row.get(5)?,
        recipe: row.get(6)?,
        source_title: row.get(7)?,
        source_url: row.get(8)?,
        request: row.get(9)?,
        used: row.get(10)?,
        undone: row.get(11)?,
        at: row.get(12)?,
        updated: row.get(13)?,
        last_used: row.get(14)?,
    })
}

/// The techniques in use, in the order they were kept.
pub fn in_use(connection: &Connection) -> Result<Vec<Technique>, StoreError> {
    let mut statement =
        connection.prepare_cached(&format!("SELECT {COLUMNS} FROM techniques WHERE archived_at IS NULL ORDER BY created_at, rowid"))?;
    let rows = statement.query_map([], row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The techniques in use become `techniques`: a new label is added, a known one is updated, and one in
/// use that isn't among them is set aside (archived), never deleted.
pub fn keep(connection: &Connection, techniques: &[Technique], now: i64) -> Result<(), StoreError> {
    let mut known: HashMap<String, (String, Technique)> = HashMap::new();
    {
        let mut statement = connection.prepare_cached(&format!("SELECT {COLUMNS}, id FROM techniques WHERE archived_at IS NULL"))?;
        let rows = statement.query_map([], |r| Ok((r.get::<_, String>(15)?, row(r)?)))?;
        for found in rows {
            let (id, technique) = found?;
            known.insert(technique.label.clone(), (id, technique));
        }
    }
    for technique in techniques {
        let t = technique;
        match known.remove(&t.label) {
            Some((_, same)) if same == *t => {}
            Some((id, _)) => {
                connection
                    .prepare_cached(
                        "UPDATE techniques SET name = ?2, fits = ?3, idea = ?4, settings = ?5, substitutes = ?6, recipe = ?7,
                         source_title = ?8, source_url = ?9, request = ?10, used = ?11, undone = ?12, created_at = ?13,
                         updated_at = ?14, last_used_at = ?15 WHERE id = ?1",
                    )?
                    .execute(params![
                        id,
                        t.name,
                        t.fits,
                        t.idea,
                        t.settings,
                        t.substitutes,
                        t.recipe,
                        t.source_title,
                        t.source_url,
                        t.request,
                        t.used,
                        t.undone,
                        t.at,
                        t.updated,
                        t.last_used
                    ])?;
            }
            None => {
                connection
                    .prepare_cached(&format!(
                        "INSERT INTO techniques (id, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
                    ))?
                    .execute(params![
                        new_id(),
                        t.label,
                        t.name,
                        t.fits,
                        t.idea,
                        t.settings,
                        t.substitutes,
                        t.recipe,
                        t.source_title,
                        t.source_url,
                        t.request,
                        t.used,
                        t.undone,
                        t.at,
                        t.updated,
                        t.last_used
                    ])?;
            }
        }
    }
    for (id, _) in known.into_values() {
        connection.prepare_cached("UPDATE techniques SET archived_at = ?2 WHERE id = ?1")?.execute(params![id, now])?;
    }
    Ok(())
}

/// Add a technique, or write a known one's content anew (by label, among those in use).
pub fn put(connection: &Connection, t: &Technique) -> Result<(), StoreError> {
    let updated = connection
        .prepare_cached(
            "UPDATE techniques SET name = ?2, fits = ?3, idea = ?4, settings = ?5, substitutes = ?6, recipe = ?7, source_title = ?8,
             source_url = ?9, request = ?10, used = ?11, undone = ?12, created_at = ?13, updated_at = ?14, last_used_at = ?15
             WHERE label = ?1 AND archived_at IS NULL",
        )?
        .execute(params![
            t.label,
            t.name,
            t.fits,
            t.idea,
            t.settings,
            t.substitutes,
            t.recipe,
            t.source_title,
            t.source_url,
            t.request,
            t.used,
            t.undone,
            t.at,
            t.updated,
            t.last_used
        ])?;
    if updated == 0 {
        connection
            .prepare_cached(&format!(
                "INSERT INTO techniques (id, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
            ))?
            .execute(params![
                new_id(),
                t.label,
                t.name,
                t.fits,
                t.idea,
                t.settings,
                t.substitutes,
                t.recipe,
                t.source_title,
                t.source_url,
                t.request,
                t.used,
                t.undone,
                t.at,
                t.updated,
                t.last_used
            ])?;
    }
    Ok(())
}

/// A build that used the technique went in (`undone` false: one more use, the latest), or the producer
/// undid one (one use fewer, one undo more). Whether there was one in use.
pub fn record(connection: &Connection, label: &str, undone: bool, now: i64) -> Result<bool, StoreError> {
    let changed = if undone {
        connection
            .prepare_cached("UPDATE techniques SET used = max(used - 1, 0), undone = undone + 1 WHERE label = ?1 AND archived_at IS NULL")?
            .execute(params![label])?
    } else {
        connection
            .prepare_cached("UPDATE techniques SET used = used + 1, last_used_at = ?2 WHERE label = ?1 AND archived_at IS NULL")?
            .execute(params![label, now])?
    };
    Ok(changed > 0)
}

/// Set the technique in use with this label aside, as a full list makes room. Whether there was one.
pub fn archive(connection: &Connection, label: &str, now: i64) -> Result<bool, StoreError> {
    Ok(connection
        .prepare_cached("UPDATE techniques SET archived_at = ?2 WHERE label = ?1 AND archived_at IS NULL")?
        .execute(params![label, now])?
        > 0)
}

/// Forget a technique: deleted, and remembered as forgotten so no import brings it back. Whether there
/// was one.
pub fn forget(connection: &Connection, label: &str, now: i64) -> Result<bool, StoreError> {
    let id: Option<String> = connection
        .prepare_cached("SELECT id FROM techniques WHERE label = ?1 AND archived_at IS NULL")?
        .query_row(params![label], |row| row.get(0))
        .optional()?;
    let Some(id) = id else { return Ok(false) };
    connection.prepare_cached("DELETE FROM techniques WHERE id = ?1")?.execute(params![id])?;
    connection.prepare_cached("INSERT OR REPLACE INTO forgotten (id, at) VALUES (?1, ?2)")?.execute(params![id, now])?;
    Ok(true)
}

/// Read in a technique an earlier Kumi kept in a file. Its id comes from its label and when it was
/// kept, so it's read in once and one forgotten here stays forgotten; one in use with that label, name
/// and idea (written back for an older Kumi) is the same technique. A label another technique in use
/// has gets the next free one. Whether it was added.
pub fn import(connection: &Connection, t: &Technique) -> Result<bool, StoreError> {
    let id = content_id(&["technique", &t.label, &t.at.to_string()]);
    if imports::known_or_forgotten(connection, "techniques", &id)?
        || connection
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM techniques WHERE label = ?1 AND name = ?2 AND idea = ?3 AND archived_at IS NULL)",
            )?
            .query_row(params![t.label, t.name, t.idea], |row| row.get::<_, bool>(0))?
    {
        return Ok(false);
    }
    let in_use: Vec<String> = connection
        .prepare_cached("SELECT label FROM techniques WHERE archived_at IS NULL")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    connection
        .prepare_cached(&format!(
            "INSERT INTO techniques (id, {COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
        ))?
        .execute(params![
            id,
            imports::free_label(&in_use, &t.label),
            t.name,
            t.fits,
            t.idea,
            t.settings,
            t.substitutes,
            t.recipe,
            t.source_title,
            t.source_url,
            t.request,
            t.used,
            t.undone,
            t.at,
            t.updated,
            t.last_used
        ])?;
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
    pub(crate) fn technique(label: &str, name: &str, at: i64) -> Technique {
        Technique {
            label: label.into(),
            name: name.into(),
            fits: "dark basses".into(),
            idea: "Two detuned saws through a low-pass".into(),
            settings: Some("cutoff 400 Hz".into()),
            substitutes: None,
            recipe: None,
            source_title: Some("A tutorial".into()),
            source_url: Some("https://example.com/t".into()),
            request: Some("a darker reese".into()),
            used: 0.0,
            undone: 0.0,
            at,
            updated: None,
            last_used: None,
        }
    }

    #[test]
    fn techniques_round_trip_and_a_dropped_one_is_set_aside() {
        let db = database();
        keep(&db, &[technique("t1", "Reese", 1), technique("t2", "Air pad", 2)], 3).unwrap();
        assert_eq!(in_use(&db).unwrap(), [technique("t1", "Reese", 1), technique("t2", "Air pad", 2)]);
        let mut used = technique("t2", "Air pad", 2);
        used.used = 2.0;
        used.undone = 1.0;
        used.last_used = Some(9);
        keep(&db, &[used.clone()], 10).unwrap();
        assert_eq!(in_use(&db).unwrap(), [used]);
        let archived: i64 = db.query_row("SELECT archived_at FROM techniques WHERE label = 't1'", [], |row| row.get(0)).unwrap();
        assert_eq!(archived, 10);
        assert!(forget(&db, "t2", 11).unwrap());
        assert!(in_use(&db).unwrap().is_empty());
        assert_eq!(db.query_row("SELECT count(*) FROM forgotten", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }
}
