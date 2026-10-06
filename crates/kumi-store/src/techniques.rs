//! Sound-building techniques the producer chose to keep.

use crate::{
    ids::{content_id, new_id},
    imports, params,
    sync::{self, BaseRow, Kept, Table},
    Connection, OptionalExtension, StoreError,
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
/// kept, so it's read in once, one forgotten here stays forgotten, and one set aside here comes back in
/// use; one in use with that label, name and idea (written back for an older Kumi) is the same
/// technique. A label another technique in use has gets the next free one. The id of the technique that
/// holds it, and whether it was added or came back.
pub fn import(connection: &Connection, t: &Technique) -> Result<(String, bool), StoreError> {
    let id = content_id(&["technique", &t.label, &t.at.to_string()]);
    if imports::forgotten(connection, &id)? {
        return Ok((id, false));
    }
    if let Some((_, archived)) = Techniques.get(connection, &id)? {
        if archived {
            Techniques.overwrite(connection, &id, t)?;
            Techniques.count(connection, &id, t, None)?;
        }
        return Ok((id, archived));
    }
    let same: Option<String> = connection
        .prepare_cached("SELECT id FROM techniques WHERE label = ?1 AND name = ?2 AND idea = ?3 AND archived_at IS NULL")?
        .query_row(params![t.label, t.name, t.idea], |row| row.get(0))
        .optional()?;
    if let Some(same) = same {
        return Ok((same, false));
    }
    insert_as(connection, &id, t)?;
    Ok((id, true))
}

/// Add `t` with this id, under its label or, when another technique in use has it, the next free one.
fn insert_as(connection: &Connection, id: &str, t: &Technique) -> Result<(), StoreError> {
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
    Ok(())
}

/// A hash of what a technique says: everything but how it has fared and when (`sync`).
pub fn hash(t: &Technique) -> String {
    let given = |value: &Option<String>| value.as_deref().map_or_else(|| "-".to_string(), |value| format!("+{value}"));
    content_id(&[
        "technique",
        &t.name,
        &t.fits,
        &t.idea,
        &given(&t.settings),
        &given(&t.substitutes),
        &given(&t.recipe),
        &given(&t.source_title),
        &given(&t.source_url),
        &given(&t.request),
    ])
}
/// A techniques file read in with no base yet: the file's base, and how many were added.
pub fn read_in(connection: &Connection, file: &[Technique]) -> Result<(Vec<BaseRow>, usize), StoreError> {
    sync::read_in(connection, &Techniques, file)
}
/// What changed in a techniques file since `base`, brought in (`sync::merge`): uses, undos and the
/// last use take the larger of the file's and the database's.
pub fn merge(connection: &Connection, base: &[BaseRow], file: &[Technique], now: i64) -> Result<(Vec<BaseRow>, Kept), StoreError> {
    sync::merge(connection, &Techniques, base, file, now)
}
/// The base of a file just written with the techniques in use.
pub fn base_of(connection: &Connection, file: &[Technique]) -> Result<Vec<BaseRow>, StoreError> {
    sync::base_of(connection, &Techniques, file)
}

/// The techniques, as `sync` reaches them.
struct Techniques;
impl Table for Techniques {
    type Row = Technique;
    fn label<'a>(&self, row: &'a Technique) -> &'a str {
        &row.label
    }
    fn hash(&self, row: &Technique) -> String {
        hash(row)
    }
    fn base(&self, t: &Technique, id: String) -> BaseRow {
        BaseRow { label: t.label.clone(), id, hash: hash(t), at: Some(t.at), used: Some(t.used), undone: Some(t.undone) }
    }
    fn same(&self, before: &BaseRow, t: &Technique) -> bool {
        // An older Kumi labels a new technique with the number after the highest, so one kept after the
        // newest was forgotten takes its label: when it was kept tells them apart.
        before.at.is_none_or(|at| at == t.at)
    }
    fn get(&self, c: &Connection, id: &str) -> Result<Option<(Technique, bool)>, StoreError> {
        Ok(c.prepare_cached(&format!("SELECT {COLUMNS}, archived_at IS NOT NULL FROM techniques WHERE id = ?1"))?
            .query_row(params![id], |found| Ok((row(found)?, found.get(15)?)))
            .optional()?)
    }
    fn in_use(&self, c: &Connection, label: &str) -> Result<Option<String>, StoreError> {
        Ok(c.prepare_cached("SELECT id FROM techniques WHERE label = ?1 AND archived_at IS NULL")?
            .query_row(params![label], |row| row.get(0))
            .optional()?)
    }
    fn overwrite(&self, c: &Connection, id: &str, t: &Technique) -> Result<(), StoreError> {
        let Some((here, archived)) = self.get(c, id)? else { return Ok(()) };
        let label = if archived {
            let in_use: Vec<String> = c
                .prepare_cached("SELECT label FROM techniques WHERE archived_at IS NULL")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            imports::free_label(&in_use, &here.label)
        } else {
            here.label
        };
        c.prepare_cached(
            "UPDATE techniques SET label = ?2, name = ?3, fits = ?4, idea = ?5, settings = ?6, substitutes = ?7, recipe = ?8,
             source_title = ?9, source_url = ?10, request = ?11, updated_at = ?12, archived_at = NULL WHERE id = ?1",
        )?
        .execute(params![
            id,
            label,
            t.name,
            t.fits,
            t.idea,
            t.settings,
            t.substitutes,
            t.recipe,
            t.source_title,
            t.source_url,
            t.request,
            t.updated
        ])?;
        Ok(())
    }
    fn add(&self, c: &Connection, t: &Technique) -> Result<String, StoreError> {
        let id = new_id();
        insert_as(c, &id, t)?;
        Ok(id)
    }
    fn import(&self, c: &Connection, t: &Technique) -> Result<(String, bool), StoreError> {
        import(c, t)
    }
    fn archive(&self, c: &Connection, id: &str, now: i64) -> Result<bool, StoreError> {
        Ok(c.prepare_cached("UPDATE techniques SET archived_at = ?2 WHERE id = ?1 AND archived_at IS NULL")?.execute(params![id, now])? > 0)
    }
    fn count(&self, c: &Connection, id: &str, t: &Technique, before: Option<&BaseRow>) -> Result<(), StoreError> {
        // Uses and undos the file counted since the base are added to the database's; with no base, the
        // larger count stands. The last use is the later one.
        let since = before.and_then(|before| Some((t.used - before.used?, t.undone - before.undone?)));
        let sql = if since.is_some() {
            "UPDATE techniques SET used = max(used + ?2, 0), undone = max(undone + ?3, 0),
             last_used_at = CASE WHEN ?4 IS NULL THEN last_used_at WHEN last_used_at IS NULL THEN ?4 ELSE max(last_used_at, ?4) END
             WHERE id = ?1"
        } else {
            "UPDATE techniques SET used = max(used, ?2), undone = max(undone, ?3),
             last_used_at = CASE WHEN ?4 IS NULL THEN last_used_at WHEN last_used_at IS NULL THEN ?4 ELSE max(last_used_at, ?4) END
             WHERE id = ?1"
        };
        let (used, undone) = since.unwrap_or((t.used, t.undone));
        c.prepare_cached(sql)?.execute(params![id, used, undone, t.last_used])?;
        Ok(())
    }
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
