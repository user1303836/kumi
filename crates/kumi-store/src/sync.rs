//! Keeping the files an older Kumi uses and the database in step, by a three-way merge: each file's
//! base (the file as Kumi last read or wrote it), the file now, and the database now. A change on one
//! side is taken; a row changed on both sides is kept twice; nothing is deleted, and a row the producer
//! forgot never comes back.

use crate::{Connection, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A row of a file as Kumi last read or wrote it: its label there, the database row it is, and a hash
/// of what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseRow {
    pub label: String,
    pub id: String,
    pub hash: String,
}

/// What a merge did to the database: rows added, rows changed (one set aside and back in use too), and
/// rows set aside.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kept {
    pub added: usize,
    pub changed: usize,
    pub archived: usize,
}
impl std::ops::AddAssign for Kept {
    fn add_assign(&mut self, other: Kept) {
        self.added += other.added;
        self.changed += other.changed;
        self.archived += other.archived;
    }
}

/// A kind of row a file keeps (notes of one scope, techniques, lessons), as the merge reaches it.
pub(crate) trait Table {
    type Row;
    fn label<'a>(&self, row: &'a Self::Row) -> &'a str;
    /// A hash of what the row says: what an edit changes, not its counts or times.
    fn hash(&self, row: &Self::Row) -> String;
    /// The row with this id, and whether it's set aside; None when it's gone (forgotten).
    fn get(&self, c: &Connection, id: &str) -> Result<Option<(Self::Row, bool)>, StoreError>;
    /// The id of the row in use with this label.
    fn in_use(&self, c: &Connection, label: &str) -> Result<Option<String>, StoreError>;
    /// The file's version written over the row with this id, which is in use after (under the next free
    /// label if it was set aside and another row took its label).
    fn overwrite(&self, c: &Connection, id: &str, row: &Self::Row) -> Result<(), StoreError>;
    /// The row added as a new one, under its label or the next free one: its id.
    fn add(&self, c: &Connection, row: &Self::Row) -> Result<String, StoreError>;
    /// A row new to the database read in, once (the import's rule): the id of the row that holds it, and
    /// whether it was added.
    fn import(&self, c: &Connection, row: &Self::Row) -> Result<(String, bool), StoreError>;
    /// Set the row in use with this id aside. Whether it was in use.
    fn archive(&self, c: &Connection, id: &str, now: i64) -> Result<bool, StoreError>;
    /// What the file's row counts beyond its words (a technique's uses), taken where it's more.
    fn count(&self, _c: &Connection, _id: &str, _row: &Self::Row) -> Result<(), StoreError> {
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
        base.push(BaseRow { label: table.label(row).into(), id, hash: table.hash(row) });
    }
    Ok((base, added))
}

/// What changed in a file since `base`, brought in:
/// - a row edited in the file takes the file's words where the database's row is as it was at the base
///   (and is in use again if it had been set aside); where both changed, the file's is kept as a new
///   row beside it; a row the producer forgot stays forgotten;
/// - a row new in the file is read in as the import does;
/// - a row gone from the file is set aside where the database's is as it was at the base;
/// - counts (a technique's uses) take the larger.
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
    let mut kept = Kept::default();
    let mut next = Vec::with_capacity(file.len());
    for row in file {
        let (label, hash) = (table.label(row), table.hash(row));
        let Some(before) = was.get(label) else {
            let (id, added) = table.import(c, row)?;
            kept.added += added as usize;
            next.push(BaseRow { label: label.into(), id, hash });
            continue;
        };
        let mut id = before.id.clone();
        if hash != before.hash {
            match table.get(c, &id)? {
                None => {}
                Some((here, _)) if table.hash(&here) == hash => {}
                Some((here, _)) if table.hash(&here) == before.hash => {
                    table.overwrite(c, &id, row)?;
                    kept.changed += 1;
                }
                Some(_) => {
                    id = table.add(c, row)?;
                    kept.added += 1;
                }
            }
        }
        table.count(c, &id, row)?;
        next.push(BaseRow { label: label.into(), id, hash });
    }
    let labels: HashSet<&str> = file.iter().map(|row| table.label(row)).collect();
    for before in base.iter().filter(|row| !labels.contains(row.label.as_str())) {
        if let Some((here, false)) = table.get(c, &before.id)? {
            if table.hash(&here) == before.hash && table.archive(c, &before.id, now)? {
                kept.archived += 1;
            }
        }
    }
    Ok((next, kept))
}

/// The base of a file just written from the database's rows in use: each of its rows is the row in use
/// with its label (one without is left out, and is read in as new if it's ever seen again).
pub(crate) fn base_of<T: Table>(c: &Connection, table: &T, file: &[T::Row]) -> Result<Vec<BaseRow>, StoreError> {
    let mut base = Vec::with_capacity(file.len());
    for row in file {
        if let Some(id) = table.in_use(c, table.label(row))? {
            base.push(BaseRow { label: table.label(row).into(), id, hash: table.hash(row) });
        }
    }
    Ok(base)
}
