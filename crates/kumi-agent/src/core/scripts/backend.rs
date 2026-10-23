//! A DAW's side of kept scripts: only its own facts. Its words, what a script takes by its place there (lint rules), whether a
//! chunk ran in this session without an error, its copy installed as its action and taken away, and a run as its run-Lua change.
use super::store::Saved;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// The language a script is in ("Lua (ReaScript)").
    pub language: &'static str,
    /// What a kept script becomes in the DAW, in words ("an action in REAPER's Action List…").
    pub action: &'static str,
    /// Code that takes a track, an item or another thing by its place, which in another project is another one: a regex
    /// each, and what to say.
    pub places: &'static [(&'static str, &'static str)],
}

/// Where a kept script went in the DAW, in words, and its file there.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub action: String,
    pub file: String,
}

#[async_trait(?Send)]
pub trait ScriptsBackend {
    fn words(&self) -> &'static Words;

    /// Whether this chunk ran in this session as a change, without stopping at an error.
    fn ran_cleanly(&self, code: &str) -> bool;

    /// The DAW's copy written and installed as its action.
    async fn install(&self, saved: &Saved, replace: bool, signal: &Signal) -> Result<Installed, String>;

    /// The DAW's copy and its action taken away.
    async fn uninstall(&self, slug: &str, signal: &Signal) -> Result<(), String>;

    /// The chunk run as the DAW's run-Lua change (one step of its undo): what it answered.
    async fn run(&self, code: &str, signal: &Signal) -> Result<Value, String>;
}
