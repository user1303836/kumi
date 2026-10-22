//! A DAW's side of cleaning up a jam: only its own facts. Its parts read (each pattern the sequence plays, its tracks' hits),
//! and the fixes written: a track's notes removed or moved (one change), a track made an alias, a slot playing an earlier
//! part, a slot deleted.
use super::{Hit, Part, TrackPart};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// What a part, a slot and an alias are called.
    pub part: &'static str,
    pub slot: &'static str,
    pub alias: &'static str,
}

/// A note's fix: taken away, or moved to `to` (lines from its part's start, delay included).
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Remove(Hit),
    Move(Hit, f64),
}

#[async_trait(?Send)]
pub trait JamBackend {
    fn words(&self) -> &'static Words;

    /// Every part the sequence plays, in its order, with its tracks (only these, when named).
    async fn parts(&self, tracks: &[String], signal: &Signal) -> Result<Vec<Part>, String>;

    /// A note's name as the DAW writes it ("C-4").
    fn note_name(&self, note: i64) -> String;

    /// A track's notes edited in one change: what the DAW answered, and the edits it couldn't make (a cell taken), in words.
    async fn rewrite(&self, part: &Part, track: &TrackPart, edits: &[Edit], signal: &Signal) -> Result<(Value, Vec<String>), String>;

    /// A part's track made an alias of an earlier part's (one change).
    async fn alias(&self, part: &str, track: &str, of: &str, signal: &Signal) -> Result<Value, String>;

    /// A slot made to play an earlier part (one change).
    async fn replay(&self, slot: &str, of: &str, signal: &Signal) -> Result<Value, String>;

    /// A slot deleted (one change).
    async fn drop_slot(&self, slot: &str, signal: &Signal) -> Result<Value, String>;
}
