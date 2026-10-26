//! Palettes: the sounds and songs the producer gathered to work with, kept by name and shared by every session. An
//! item is a file by its path, size and modification time (the library keeps no content hash), with what it's called,
//! whether it's a sound or a song, and its class or style as the map named it. A palette also keeps the sounds it must
//! never bring in (`excluded`), and how far it reaches past its own sounds: 0 keeps to them, k brings each one's k
//! nearest.

use crate::{ids::new_id, params, Connection, OptionalExtension, StoreError};

/// A palette in use, with how many items it has (its excluded sounds aside).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub id: String,
    pub name: String,
    pub reach: u32,
    pub items: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A sound, or a song.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Sound,
    Song,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Sound => "sound",
            ItemKind::Song => "song",
        }
    }
    fn from_str(text: &str) -> ItemKind {
        if text == "song" {
            ItemKind::Song
        } else {
            ItemKind::Sound
        }
    }
}

/// A file in a palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub name: String,
    pub kind: ItemKind,
    /// Its class (a sound's: kick, pad…) or its style (a song's), as the map named it.
    pub class: Option<String>,
    /// A sound the palette never brings in: not one of its items.
    pub excluded: bool,
    pub added_at: i64,
}

impl Item {
    /// What names it: its path, size and modification time.
    pub fn key(&self) -> (&str, i64, i64) {
        (&self.path, self.size, self.mtime)
    }
}

const PALETTE: &str = "SELECT p.id, p.name, p.reach, p.created_at, p.updated_at,
     (SELECT count(*) FROM palette_items i WHERE i.palette = p.id AND i.excluded = 0) FROM palettes p";

fn palette(row: &rusqlite::Row<'_>) -> rusqlite::Result<Palette> {
    Ok(Palette {
        id: row.get(0)?,
        name: row.get(1)?,
        reach: row.get::<_, i64>(2)?.max(0) as u32,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        items: row.get::<_, i64>(5)?.max(0) as usize,
    })
}

/// Every palette, the one changed last first.
pub fn all(connection: &Connection) -> Result<Vec<Palette>, StoreError> {
    let mut statement = connection.prepare_cached(&format!("{PALETTE} ORDER BY p.updated_at DESC, p.rowid DESC"))?;
    let rows = statement.query_map([], palette)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The palette with this id.
pub fn by_id(connection: &Connection, id: &str) -> Result<Option<Palette>, StoreError> {
    Ok(connection.prepare_cached(&format!("{PALETTE} WHERE p.id = ?1"))?.query_row([id], palette).optional()?)
}

/// The palette called `name`, whatever its case.
pub fn named(connection: &Connection, name: &str) -> Result<Option<Palette>, StoreError> {
    Ok(connection.prepare_cached(&format!("{PALETTE} WHERE p.name = ?1 COLLATE NOCASE"))?.query_row([name.trim()], palette).optional()?)
}

/// A new, empty palette called `name` (no other has it, whatever its case): None when one has.
pub fn create(connection: &Connection, name: &str, now: i64) -> Result<Option<Palette>, StoreError> {
    let name = name.trim();
    if named(connection, name)?.is_some() {
        return Ok(None);
    }
    let id = new_id();
    connection
        .prepare_cached("INSERT INTO palettes (id, name, reach, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?3)")?
        .execute(params![id, name, now])?;
    by_id(connection, &id)
}

/// A name for a new palette no other has: "Palette 1", "Palette 2"…
pub fn next_name(connection: &Connection) -> Result<String, StoreError> {
    for number in 1.. {
        let name = format!("Palette {number}");
        if named(connection, &name)?.is_none() {
            return Ok(name);
        }
    }
    unreachable!("a free number")
}

/// Renamed: false when another palette has the name, or there's no such palette.
pub fn rename(connection: &Connection, id: &str, name: &str, now: i64) -> Result<bool, StoreError> {
    let name = name.trim();
    if named(connection, name)?.is_some_and(|other| other.id != id) {
        return Ok(false);
    }
    let changed =
        connection.prepare_cached("UPDATE palettes SET name = ?2, updated_at = ?3 WHERE id = ?1")?.execute(params![id, name, now])?;
    Ok(changed > 0)
}

/// The palette and its items gone: whether there was one.
pub fn forget(connection: &Connection, id: &str) -> Result<bool, StoreError> {
    connection.prepare_cached("DELETE FROM palette_items WHERE palette = ?1")?.execute([id])?;
    Ok(connection.prepare_cached("DELETE FROM palettes WHERE id = ?1")?.execute([id])? > 0)
}

/// How far the palette reaches past its own sounds: 0 keeps to them, k brings each one's k nearest.
pub fn set_reach(connection: &Connection, id: &str, reach: u32, now: i64) -> Result<bool, StoreError> {
    let changed = connection.prepare_cached("UPDATE palettes SET reach = ?2, updated_at = ?3 WHERE id = ?1")?.execute(params![
        id,
        reach as i64,
        now
    ])?;
    Ok(changed > 0)
}

const ITEM: &str = "SELECT path, size, mtime, name, kind, class, excluded, added_at FROM palette_items";

fn item(row: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        path: row.get(0)?,
        size: row.get(1)?,
        mtime: row.get(2)?,
        name: row.get(3)?,
        kind: ItemKind::from_str(&row.get::<_, String>(4)?),
        class: row.get(5)?,
        excluded: row.get::<_, i64>(6)? != 0,
        added_at: row.get(7)?,
    })
}

/// A palette's items (and the sounds it excludes), in the order they came.
pub fn items(connection: &Connection, id: &str) -> Result<Vec<Item>, StoreError> {
    let mut statement = connection.prepare_cached(&format!("{ITEM} WHERE palette = ?1 ORDER BY added_at, rowid"))?;
    let rows = statement.query_map([id], item)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Items into a palette, in their order: one it has already stays as it is, except that an excluded sound added as an
/// item is one again. How many it didn't have.
pub fn add(connection: &Connection, id: &str, items: &[Item], now: i64) -> Result<usize, StoreError> {
    let mut added = 0;
    for (at, item) in items.iter().enumerate() {
        let excluded: Option<i64> = connection
            .prepare_cached("SELECT excluded FROM palette_items WHERE palette = ?1 AND path = ?2 AND size = ?3 AND mtime = ?4")?
            .query_row(params![id, item.path, item.size, item.mtime], |row| row.get(0))
            .optional()?;
        match excluded {
            Some(0) => continue,
            Some(_) => {
                connection
                    .prepare_cached("UPDATE palette_items SET excluded = 0 WHERE palette = ?1 AND path = ?2 AND size = ?3 AND mtime = ?4")?
                    .execute(params![id, item.path, item.size, item.mtime])?;
            }
            None => {
                connection
                    .prepare_cached(
                        "INSERT INTO palette_items (id, palette, path, size, mtime, name, kind, class, excluded, added_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
                    )?
                    .execute(params![
                        new_id(),
                        id,
                        item.path,
                        item.size,
                        item.mtime,
                        item.name,
                        item.kind.as_str(),
                        item.class,
                        now + at as i64
                    ])?;
            }
        }
        added += 1;
    }
    touch(connection, id, now)?;
    Ok(added)
}

/// An item out of a palette: whether it had it.
pub fn remove(connection: &Connection, id: &str, (path, size, mtime): (&str, i64, i64), now: i64) -> Result<bool, StoreError> {
    let removed = connection
        .prepare_cached("DELETE FROM palette_items WHERE palette = ?1 AND path = ?2 AND size = ?3 AND mtime = ?4")?
        .execute(params![id, path, size, mtime])?;
    touch(connection, id, now)?;
    Ok(removed > 0)
}

/// A sound the palette never brings in (an item of it no more), or brings in again (`excluded` false: forgotten).
pub fn exclude(connection: &Connection, id: &str, item: &Item, excluded: bool, now: i64) -> Result<(), StoreError> {
    if excluded {
        connection
            .prepare_cached(
                "INSERT INTO palette_items (id, palette, path, size, mtime, name, kind, class, excluded, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)
                 ON CONFLICT (palette, path, size, mtime) DO UPDATE SET excluded = 1",
            )?
            .execute(params![new_id(), id, item.path, item.size, item.mtime, item.name, item.kind.as_str(), item.class, now])?;
    } else {
        connection
            .prepare_cached("DELETE FROM palette_items WHERE palette = ?1 AND path = ?2 AND size = ?3 AND mtime = ?4 AND excluded = 1")?
            .execute(params![id, item.path, item.size, item.mtime])?;
    }
    touch(connection, id, now)
}

/// An item whose file moved (found again by its name and size): its new path and modification time.
pub fn moved(connection: &Connection, id: &str, (path, size, mtime): (&str, i64, i64), to: (&str, i64)) -> Result<bool, StoreError> {
    let changed = connection
        .prepare_cached(
            "UPDATE OR IGNORE palette_items SET path = ?5, mtime = ?6 WHERE palette = ?1 AND path = ?2 AND size = ?3 AND mtime = ?4",
        )?
        .execute(params![id, path, size, mtime, to.0, to.1])?;
    Ok(changed > 0)
}

fn touch(connection: &Connection, id: &str, now: i64) -> Result<(), StoreError> {
    connection.prepare_cached("UPDATE palettes SET updated_at = max(updated_at, ?2) WHERE id = ?1")?.execute(params![id, now])?;
    Ok(())
}
