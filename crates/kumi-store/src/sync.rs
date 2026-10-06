//! Keeping the files an older Kumi uses and the database in step, by a three-way merge: each file's
//! base (the file as Kumi last read or wrote it), the file now, and the database now. A change on one
//! side is taken; a row changed on both sides is kept twice; nothing is deleted.

use crate::{Connection, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A row of a file as Kumi last read or wrote it: its label there, the database row it is, a hash of
/// what it said, and for a technique when it was kept (an older Kumi gives a new technique the label of
/// one just forgotten) and its counts then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseRow {
    pub label: String,
    pub id: String,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone: Option<f64>,
}

/// What a merge did to the database: rows added, rows changed (one set aside and back in use too), rows
/// set aside, and rows edited on both sides, whose file version was kept beside the database's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kept {
    pub added: usize,
    pub changed: usize,
    pub archived: usize,
    pub both: usize,
}
impl std::ops::AddAssign for Kept {
    fn add_assign(&mut self, other: Kept) {
        self.added += other.added;
        self.changed += other.changed;
        self.archived += other.archived;
        self.both += other.both;
    }
}

/// A kind of row a file keeps (notes of one scope, techniques, lessons), as the merge reaches it.
pub(crate) trait Table {
    type Row;
    fn label<'a>(&self, row: &'a Self::Row) -> &'a str;
    /// A hash of what the row says: what an edit changes, not its counts or times.
    fn hash(&self, row: &Self::Row) -> String;
    /// The file row's base entry, as the database row `id`.
    fn base(&self, row: &Self::Row, id: String) -> BaseRow {
        BaseRow { label: self.label(row).into(), id, hash: self.hash(row), at: None, used: None, undone: None }
    }
    /// Whether the file row is the row `before` was, beyond sharing its label.
    fn same(&self, _before: &BaseRow, _row: &Self::Row) -> bool {
        true
    }
    /// Whether a row forgotten here but changed in the file since comes back as a new one: when the
    /// file's label can't tell a change from a new row given a forgotten one's label (notes), a new row
    /// is never dropped. Otherwise the forget stands.
    fn changed_after_forget_is_new(&self) -> bool {
        false
    }
    /// The row with this id, and whether it's set aside; None when it's gone (forgotten).
    fn get(&self, c: &Connection, id: &str) -> Result<Option<(Self::Row, bool)>, StoreError>;
    /// The id of the row in use with this label.
    fn in_use(&self, c: &Connection, label: &str) -> Result<Option<String>, StoreError>;
    /// The file's version written over the row with this id, which is in use after (under the next free
    /// label if it was set aside and another row took its label).
    fn overwrite(&self, c: &Connection, id: &str, row: &Self::Row) -> Result<(), StoreError>;
    /// The row added as a new one, under its label or the next free one: its id.
    fn add(&self, c: &Connection, row: &Self::Row) -> Result<String, StoreError>;
    /// A row new to the database read in, once (the import's rule; one set aside comes back in use): the
    /// id of the row that holds it, and whether it was added or came back.
    fn import(&self, c: &Connection, row: &Self::Row) -> Result<(String, bool), StoreError>;
    /// Set the row in use with this id aside. Whether it was in use.
    fn archive(&self, c: &Connection, id: &str, now: i64) -> Result<bool, StoreError>;
    /// What the file's row counts beyond its words (a technique's uses), added to the row's: what the
    /// file counted since `before`, or the larger count when there's no base.
    fn count(&self, _c: &Connection, _id: &str, _row: &Self::Row, _before: Option<&BaseRow>) -> Result<(), StoreError> {
        Ok(())
    }
}

/// A file read in with no base yet (the first time): rows new to the database are added, nothing else
/// changes. The file's base, and how many rows were added.
pub(crate) fn read_in<T: Table>(c: &Connection, table: &T, file: &[T::Row]) -> Result<(Vec<BaseRow>, usize), StoreError> {
    let mut base = Vec::with_capacity(file.len());
    let mut added = 0;
    for row in file {
        let (id, new) = table.import(c, row)?;
        added += new as usize;
        base.push(table.base(row, id));
    }
    Ok((base, added))
}

/// What changed in a file since `base`, brought in:
/// - a row edited in the file takes the file's words where the database's row is as it was at the base
///   (and is in use again if it had been set aside); where both changed, the file's is kept as a new
///   row beside it; a row the producer forgot here stays forgotten, unless the table says a changed one
///   comes back as new (`changed_after_forget_is_new`);
/// - a row new in the file (or under a reused label, `same`) is read in as the import does;
/// - a row gone from the file is set aside where the database's is as it was at the base;
/// - counts (a technique's uses) take what the file counted since the base.
///
/// The file's new base, and what changed.
pub(crate) fn merge<T: Table>(
    c: &Connection,
    table: &T,
    base: &[BaseRow],
    file: &[T::Row],
    now: i64,
) -> Result<(Vec<BaseRow>, Kept), StoreError> {
    let was: HashMap<&str, &BaseRow> = base.iter().map(|row| (row.label.as_str(), row)).collect();
    let matches: Vec<Option<&BaseRow>> =
        file.iter().map(|row| was.get(table.label(row)).copied().filter(|before| table.same(before, row))).collect();
    let matched: HashSet<&str> = matches.iter().flatten().map(|before| before.label.as_str()).collect();
    let mut kept = Kept::default();
    // Rows gone from the file first, so a label one of them freed is free for a row new in it.
    for before in base.iter().filter(|row| !matched.contains(row.label.as_str())) {
        if let Some((here, false)) = table.get(c, &before.id)? {
            if table.hash(&here) == before.hash && table.archive(c, &before.id, now)? {
                kept.archived += 1;
            }
        }
    }
    let mut next = Vec::with_capacity(file.len());
    for (row, before) in file.iter().zip(matches) {
        let hash = table.hash(row);
        let Some(before) = before else {
            let (id, added) = table.import(c, row)?;
            kept.added += added as usize;
            next.push(table.base(row, id));
            continue;
        };
        let mut id = before.id.clone();
        if hash != before.hash {
            match table.get(c, &id)? {
                None if table.changed_after_forget_is_new() => {
                    let (new, added) = table.import(c, row)?;
                    kept.added += added as usize;
                    id = new;
                }
                None => {}
                Some((here, _)) if table.hash(&here) == hash => {}
                Some((here, _)) if table.hash(&here) == before.hash => {
                    table.overwrite(c, &id, row)?;
                    kept.changed += 1;
                }
                Some(_) => {
                    id = table.add(c, row)?;
                    kept.both += 1;
                    next.push(table.base(row, id));
                    continue;
                }
            }
        }
        table.count(c, &id, row, Some(before))?;
        next.push(table.base(row, id));
    }
    Ok((next, kept))
}

/// The base of a file just written from the database's rows in use, as re-read: each of its rows is the
/// row in use with its label, which takes what the file says where reading it back changed a word, so
/// base and row agree (one without a row is left out, and is read in as new if it's ever seen again).
pub(crate) fn base_of<T: Table>(c: &Connection, table: &T, file: &[T::Row]) -> Result<Vec<BaseRow>, StoreError> {
    let mut base = Vec::with_capacity(file.len());
    for row in file {
        let Some(id) = table.in_use(c, table.label(row))? else { continue };
        if table.get(c, &id)?.is_some_and(|(here, _)| table.hash(&here) != table.hash(row)) {
            table.overwrite(c, &id, row)?;
        }
        base.push(table.base(row, id));
    }
    Ok(base)
}
