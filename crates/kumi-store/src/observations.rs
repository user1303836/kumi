//! What the producer did in answer to Kumi, kept to learn their taste from later: words they reacted
//! with, a pick, an undo, a value they moved after Kumi set it, something Kumi made used again.
//!
//! Rows are only ever added. Nothing here judges them: each keeps the default weight of its kind and
//! every fact behind it, so a later learner can weigh them again. Nothing reads them into a prompt.

use crate::{ids::new_id, params, Connection, StoreError};
use serde_json::Value;

/// What kind of reaction an observation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The producer's own words, tagged by the model.
    Words,
    /// One of the options an answer ended on, picked with a key.
    Pick,
    /// Yes or no to keeping a technique from what Kumi built.
    TechniqueOffer,
    /// The producer undid one of Kumi's changes.
    Undo,
    /// The producer moved a value Kumi had set.
    EditAfter,
    /// Something Kumi made turned up in another project.
    Reuse,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Words => "words",
            Kind::Pick => "pick",
            Kind::TechniqueOffer => "technique_offer",
            Kind::Undo => "undo",
            Kind::EditAfter => "edit_after",
            Kind::Reuse => "reuse",
        }
    }
    fn parse(text: &str) -> Option<Kind> {
        [Kind::Words, Kind::Pick, Kind::TechniqueOffer, Kind::Undo, Kind::EditAfter, Kind::Reuse]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// An observation, as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub id: String,
    pub at: i64,
    /// The conversation it happened in.
    pub session: String,
    /// The Set's project; none for a Set not saved yet.
    pub project: Option<String>,
    pub kind: Kind,
    /// How much it counts by default; none for a rule ("never", "always").
    pub weight: Option<f64>,
    /// Whether the producer had heard what it's about; none where that doesn't apply.
    pub heard: Option<bool>,
    /// What it's about.
    pub subject: Value,
    /// What happened.
    pub facts: Value,
    /// Where it happened: the Set's fingerprint and the producer's latest requests.
    pub context: Value,
}

impl Observation {
    /// A new observation of `kind` at `at`, with a new id.
    pub fn new(at: i64, session: &str, project: Option<String>, kind: Kind) -> Observation {
        Observation {
            id: new_id(),
            at,
            session: session.into(),
            project,
            kind,
            weight: None,
            heard: None,
            subject: Value::Null,
            facts: Value::Null,
            context: Value::Null,
        }
    }
}

/// Keep an observation.
pub fn append(connection: &Connection, observation: &Observation) -> Result<(), StoreError> {
    let json = |value: &Value| serde_json::to_string(value).map_err(|error| StoreError::Io(error.to_string()));
    connection
        .prepare_cached(
            "INSERT INTO observations (id, at, session, project, kind, weight, heard, subject, facts, context)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, jsonb(?8), jsonb(?9), jsonb(?10))",
        )?
        .execute(params![
            observation.id,
            observation.at,
            observation.session,
            observation.project,
            observation.kind.as_str(),
            observation.weight,
            observation.heard,
            json(&observation.subject)?,
            json(&observation.facts)?,
            json(&observation.context)?,
        ])?;
    Ok(())
}

/// The latest observations, newest first.
pub fn recent(connection: &Connection, limit: usize) -> Result<Vec<Observation>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT id, at, session, project, kind, weight, heard, json(subject), json(facts), json(context)
         FROM observations ORDER BY at DESC, id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        let json = |at: usize| -> rusqlite::Result<Value> { Ok(serde_json::from_str(&row.get::<_, String>(at)?).unwrap_or(Value::Null)) };
        let kind: String = row.get(4)?;
        Ok(Observation {
            id: row.get(0)?,
            at: row.get(1)?,
            session: row.get(2)?,
            project: row.get(3)?,
            kind: Kind::parse(&kind).ok_or_else(|| rusqlite::Error::InvalidColumnType(4, kind, rusqlite::types::Type::Text))?,
            weight: row.get(5)?,
            heard: row.get(6)?,
            subject: json(7)?,
            facts: json(8)?,
            context: json(9)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
