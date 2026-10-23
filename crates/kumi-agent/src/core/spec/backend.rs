//! A DAW's side of delivery to a spec: only its own facts. The mix rendered offline (the whole of it, or a stretch), its master
//! fader as it is and set by a gain (one change), and its words for what a fader can't do (a limiter, a gainer).
use crate::core::contracts::Render;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// The highest the master fader goes (dB).
    pub fader_most: f64,
    /// What limits the master's peaks in the DAW, in words (its limiter, and how to set it).
    pub limiter: &'static str,
    /// What raises the master past its fader's most, in words.
    pub gainer: &'static str,
}

#[async_trait(?Send)]
pub trait SpecBackend {
    fn words(&self) -> &'static Words;

    /// The mix rendered offline: from `from` for `beats` (quarter notes), or the whole of it.
    async fn render_mix(&self, stretch: Option<(f64, f64)>, signal: &Signal) -> Result<Render, String>;

    /// The master fader now (dB).
    async fn master_db(&self, signal: &Signal) -> Result<f64, String>;

    /// The master fader set to `db` (one change): what the DAW answered.
    async fn set_master_db(&self, db: f64, signal: &Signal) -> Result<Value, String>;
}
