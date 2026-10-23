//! A DAW's side of stems: only its own facts. A track rendered over a stretch, where the stem files go, and the stems put
//! back beside the track over the stretch (one plan).
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::contracts::Render;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Words {
    pub app: &'static str,
    /// Where the stems go, in words ("a new track each under it, its audio over the stretch").
    pub placed: &'static str,
}

#[async_trait(?Send)]
pub trait StemsBackend {
    fn words(&self) -> &'static Words;

    /// The track rendered over the stretch (`from`, `beats` in quarter notes), and its name.
    async fn render(&self, track: &str, from: f64, beats: f64, signal: &Signal) -> Result<(Render, String), String>;

    /// The folder the stem files are written in (one the DAW can read).
    fn folder(&self) -> PathBuf;

    /// The stems (each its name and file) put back beside `track` over the stretch (one plan): what was made.
    async fn place(&self, track: &str, stems: &[(String, &Path)], from: f64, beats: f64, signal: &Signal) -> Result<Value, String>;
}
