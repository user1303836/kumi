//! A DAW's side of layering breaks: only its own facts. Tracks rendered over a stretch (each as the producer hears it alone),
//! a layer nudged earlier or later and its polarity flipped (a change each), the group's compressor found (or added) and its
//! threshold set, the master's dynamics (what every render passes through) found, and a device bypassed or on again.
use crate::core::contracts::Render;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// What nudges a layer, flips its polarity, and glues a group, in words.
    pub nudge: &'static str,
    pub invert: &'static str,
    pub compressor: &'static str,
}

/// A compressor the glue works with: its ref and name, its threshold now and the range it takes (dB), and whether it's on.
#[derive(Debug, Clone, PartialEq)]
pub struct Compressor {
    pub reference: String,
    pub name: String,
    pub threshold: f64,
    pub lowest: f64,
    pub highest: f64,
    pub active: bool,
}

/// A limiter, maximizer or compressor on the master: its ref and name.
#[derive(Debug, Clone, PartialEq)]
pub struct Dynamics {
    pub reference: String,
    pub name: String,
}

#[async_trait(?Send)]
pub trait LayersBackend {
    fn words(&self) -> &'static Words;

    /// Each track rendered alone over the stretch (`from`, `beats` in quarter notes), in order.
    async fn render(&self, tracks: &[String], from: f64, beats: f64, signal: &Signal) -> Result<Vec<Render>, String>;

    /// A layer moved by `ms` (earlier when negative), in one change; the stretch is the one the offset was heard in.
    async fn nudge(&self, track: &str, ms: f64, from: f64, beats: f64, signal: &Signal) -> Result<Value, String>;

    /// A layer's polarity flipped.
    async fn invert(&self, track: &str, signal: &Signal) -> Result<Value, String>;

    /// The group's compressor, None when it has none; with `add`, one added when it has none (its words say which).
    async fn compressor(&self, group: &str, add: bool, signal: &Signal) -> Result<Option<Compressor>, String>;

    /// Its threshold set (dB, one change).
    async fn set_threshold(&self, compressor: &Compressor, db: f64, signal: &Signal) -> Result<Value, String>;

    /// The master's dynamics that are on: a limiter there takes peak-to-loudness ratio from every render, so the glue
    /// bypasses them while it measures.
    async fn master_dynamics(&self, signal: &Signal) -> Result<Vec<Dynamics>, String>;

    /// A device (a compressor's or a dynamics' ref) bypassed, or on again (one change).
    async fn bypass(&self, reference: &str, bypass: bool, signal: &Signal) -> Result<Value, String>;
}
