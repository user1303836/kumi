//! What Kumi heard of each setting it tried on a device: every candidate a search heard (tune's knob search, a match
//! run's, a goal's), with its knobs, what it was aimed at, how the track sounded before, how this setting sounded, and
//! its cost. A later search on the same device can start from what worked there.
//!
//! A device is who it is in every session (a built-in device's own name, a plug-in's format, maker and name as far as
//! the DAW says them), never its place on a track. Knobs are by name, each a fraction of its range. A device keeps its
//! newest `PER_DEVICE` rows: older ones go first, but never a search's best.

use crate::{params, Connection, StoreError};
use serde_json::Value;

/// The rows a device keeps (a search's best are kept beyond them).
pub const PER_DEVICE: usize = 5_000;

/// Which search heard a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// tune's knob search (its CMA-ES generations).
    Tune,
    /// A match run's knob search, once the model's own work stalled.
    Match,
    /// A goal's evolution (/goal, /loop).
    Goal,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Tune => "tune",
            Source::Match => "match",
            Source::Goal => "goal",
        }
    }
    fn parse(text: &str) -> Option<Source> {
        [Source::Tune, Source::Match, Source::Goal].into_iter().find(|source| source.as_str() == text)
    }
}

/// A setting heard, as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardSetting {
    pub id: String,
    pub at: i64,
    /// The DAW it was heard in (`ableton`, `reaper`, `renoise`).
    pub daw: String,
    /// Who the device is, the same in every session.
    pub device: String,
    pub device_version: Option<String>,
    /// The track it was on, by name: for saying where a setting worked.
    pub track: Option<String>,
    pub source: Source,
    /// The search it was heard in: one search's settings share it.
    pub run_id: String,
    /// The knobs by name, each a fraction of its range.
    pub knobs: Value,
    /// What the search aimed at: its kind, and what it wanted.
    pub target: Value,
    /// How the track sounded before the search changed it.
    pub before: Option<Value>,
    /// How this setting sounded.
    pub measured: Value,
    /// How far it was from what the search wanted: lower is better, within one search.
    pub cost: f64,
    /// The search's best.
    pub best: bool,
    /// Which generation of its search heard it (1 the first), when the search has generations.
    pub generation: Option<u32>,
    /// The remembered setting it was (that row's id), when a search started from one.
    pub remembered: Option<String>,
}

/// Keep these settings, then trim each device they're on to its newest `PER_DEVICE` rows (a search's best stay).
pub fn add(connection: &Connection, settings: &[HeardSetting]) -> Result<(), StoreError> {
    let json = |value: &Value| serde_json::to_string(value).map_err(|error| StoreError::Io(error.to_string()));
    let mut devices: Vec<(&str, &str)> = vec![];
    for setting in settings {
        connection
            .prepare_cached(
                "INSERT INTO heard_settings (id, at, daw, device, device_version, track, source, run_id, knobs, target, before, measured, cost, best,
                   generation, remembered)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, jsonb(?9), jsonb(?10), jsonb(?11), jsonb(?12), ?13, ?14, ?15, ?16)",
            )?
            .execute(params![
                setting.id,
                setting.at,
                setting.daw,
                setting.device,
                setting.device_version,
                setting.track,
                setting.source.as_str(),
                setting.run_id,
                json(&setting.knobs)?,
                json(&setting.target)?,
                setting.before.as_ref().map(json).transpose()?,
                json(&setting.measured)?,
                setting.cost,
                setting.best,
                setting.generation,
                setting.remembered,
            ])?;
        if !devices.contains(&(setting.daw.as_str(), setting.device.as_str())) {
            devices.push((&setting.daw, &setting.device));
        }
    }
    for (daw, device) in devices {
        trim(connection, daw, device, PER_DEVICE)?;
    }
    Ok(())
}

/// A device's oldest rows past its newest `most`, gone (never a search's best): how many went.
fn trim(connection: &Connection, daw: &str, device: &str, most: usize) -> Result<usize, StoreError> {
    let kept: i64 = connection
        .prepare_cached("SELECT count(*) FROM heard_settings WHERE daw = ?1 AND device = ?2")?
        .query_row(params![daw, device], |row| row.get(0))?;
    let over = kept - most as i64;
    if over <= 0 {
        return Ok(0);
    }
    Ok(connection
        .prepare_cached(
            "DELETE FROM heard_settings WHERE id IN (
               SELECT id FROM heard_settings WHERE daw = ?1 AND device = ?2 AND best = 0 ORDER BY at, id LIMIT ?3
             )",
        )?
        .execute(params![daw, device, over])?)
}

/// A search's best: `best` (rows of `run`) are, the rest of its rows aren't. How many rows of the run there are.
pub fn mark_best(connection: &Connection, run: &str, best: &[String]) -> Result<usize, StoreError> {
    let ids = serde_json::to_string(best).map_err(|error| StoreError::Io(error.to_string()))?;
    Ok(connection
        .prepare_cached("UPDATE heard_settings SET best = (id IN (SELECT value FROM json_each(?2))) WHERE run_id = ?1")?
        .execute(params![run, ids])?)
}

/// How many settings are kept, and on how many devices.
pub fn counts(connection: &Connection) -> Result<(usize, usize), StoreError> {
    Ok(connection
        .prepare_cached("SELECT count(*), count(DISTINCT daw || char(0) || device) FROM heard_settings")?
        .query_row([], |row| Ok((row.get::<_, i64>(0)? as usize, row.get::<_, i64>(1)? as usize)))?)
}

/// The columns a setting is read back by.
const COLUMNS: &str =
    "id, at, daw, device, device_version, track, source, run_id, json(knobs), json(target), json(before), json(measured), \
     cost, best, generation, remembered";

/// A device's settings in a DAW, newest first, at most `limit`.
pub fn on_device(connection: &Connection, daw: &str, device: &str, limit: usize) -> Result<Vec<HeardSetting>, StoreError> {
    read(
        connection,
        &format!("SELECT {COLUMNS} FROM heard_settings WHERE daw = ?1 AND device = ?2 ORDER BY at DESC, id DESC LIMIT ?3"),
        params![daw, device, limit as i64],
    )
}

/// Each past search's best on a device in a DAW, from these sources, newest first, at most `limit`: what a new search
/// there can start from.
pub fn bests_on_device(
    connection: &Connection,
    daw: &str,
    device: &str,
    sources: &[Source],
    limit: usize,
) -> Result<Vec<HeardSetting>, StoreError> {
    let sources = serde_json::to_string(&sources.iter().map(|source| source.as_str()).collect::<Vec<_>>())
        .map_err(|error| StoreError::Io(error.to_string()))?;
    read(
        connection,
        &format!(
            "SELECT {COLUMNS} FROM heard_settings WHERE daw = ?1 AND device = ?2 AND best = 1
               AND source IN (SELECT value FROM json_each(?3)) ORDER BY at DESC, id DESC LIMIT ?4"
        ),
        params![daw, device, sources, limit as i64],
    )
}

/// How often a remembered setting won a search's first round: the searches whose first generation heard one, and those
/// where one of them was that generation's best.
pub fn first_rounds(connection: &Connection) -> Result<(usize, usize), StoreError> {
    Ok(connection
        .prepare_cached(
            "WITH first AS (SELECT run_id, remembered, cost FROM heard_settings WHERE generation = 1),
                  started AS (SELECT DISTINCT run_id FROM first WHERE remembered IS NOT NULL),
                  won AS (SELECT DISTINCT f.run_id FROM first f WHERE f.remembered IS NOT NULL
                            AND f.cost <= (SELECT min(g.cost) FROM first g WHERE g.run_id = f.run_id))
             SELECT (SELECT count(*) FROM started), (SELECT count(*) FROM won)",
        )?
        .query_row([], |row| Ok((row.get::<_, i64>(0)? as usize, row.get::<_, i64>(1)? as usize)))?)
}

fn read(connection: &Connection, sql: &str, parameters: impl rusqlite::Params) -> Result<Vec<HeardSetting>, StoreError> {
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map(parameters, |row| {
        let json = |at: usize| -> rusqlite::Result<Value> {
            Ok(row.get::<_, Option<String>>(at)?.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null))
        };
        let source: String = row.get(6)?;
        Ok(HeardSetting {
            id: row.get(0)?,
            at: row.get(1)?,
            daw: row.get(2)?,
            device: row.get(3)?,
            device_version: row.get(4)?,
            track: row.get(5)?,
            source: Source::parse(&source).ok_or_else(|| rusqlite::Error::InvalidColumnType(6, source, rusqlite::types::Type::Text))?,
            run_id: row.get(7)?,
            knobs: json(8)?,
            target: json(9)?,
            before: Some(json(10)?).filter(|value| !value.is_null()),
            measured: json(11)?,
            cost: row.get(12)?,
            best: row.get(13)?,
            generation: row.get(14)?,
            remembered: row.get(15)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
