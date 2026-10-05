//! Capabilities a producer needed that Kumi or Live doesn't offer, kept for Kumi's developers.

use crate::{
    ids::{content_id, new_id},
    imports, params, Connection, StoreError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub kumi_version: String,
    pub missing: String,
    pub asked: Option<String>,
    pub workaround: Option<String>,
    pub at: i64,
}

/// Note a gap. Every one is kept: they're small, and nothing reads them into a prompt.
pub fn add(connection: &Connection, gap: &Gap) -> Result<(), StoreError> {
    connection
        .prepare_cached("INSERT INTO gaps (id, kumi_version, missing, asked, workaround, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
        .execute(params![new_id(), gap.kumi_version, gap.missing, gap.asked, gap.workaround, gap.at])?;
    Ok(())
}

/// The gaps noted, oldest first.
pub fn all(connection: &Connection) -> Result<Vec<Gap>, StoreError> {
    let mut statement =
        connection.prepare_cached("SELECT kumi_version, missing, asked, workaround, created_at FROM gaps ORDER BY created_at, id")?;
    let rows = statement.query_map([], |row| {
        Ok(Gap { kumi_version: row.get(0)?, missing: row.get(1)?, asked: row.get(2)?, workaround: row.get(3)?, at: row.get(4)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Read in a gap an earlier Kumi logged in a file, once (its id comes from its time and words). Whether
/// it was added.
pub fn import(connection: &Connection, gap: &Gap) -> Result<bool, StoreError> {
    let id = content_id(&["gap", &gap.at.to_string(), &gap.missing]);
    if imports::known_or_forgotten(connection, "gaps", &id)? {
        return Ok(false);
    }
    connection
        .prepare_cached("INSERT INTO gaps (id, kumi_version, missing, asked, workaround, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
        .execute(params![id, gap.kumi_version, gap.missing, gap.asked, gap.workaround, gap.at])?;
    Ok(true)
}
