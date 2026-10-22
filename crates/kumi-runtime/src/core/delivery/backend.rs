//! A DAW's side of delivery: only its own facts. Its items and regions as it reads them, the change that adds an asset's region
//! each (with the tracks it renders), the files a render with these settings would write (its own dry run), and the render.
use super::assets::Item;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// What a region is called, and the pattern that names a file for each one ("$region").
    pub region: &'static str,
    pub region_pattern: &'static str,
    /// The wildcards a pattern takes, in words.
    pub wildcards: &'static str,
    /// What a render's settings are, for the schema's descriptions: the sources and bounds it takes.
    pub sources: &'static str,
    pub bounds: &'static str,
}

/// A region the DAW has: its ref, name and stretch (seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub reference: String,
    pub name: String,
    pub start: f64,
    pub end: f64,
}

/// A region to add: its stretch, name, and the tracks it renders (refs; none: the master mix).
#[derive(Debug, Clone, PartialEq)]
pub struct NewRegion {
    pub start: f64,
    pub end: f64,
    pub name: String,
    pub renders: Vec<String>,
}

/// The files a render would write: their paths, the folder, and those that are there already.
#[derive(Debug, Clone, PartialEq)]
pub struct Targets {
    pub files: Vec<String>,
    pub folder: String,
    pub there: Vec<String>,
}

#[async_trait(?Send)]
pub trait DeliveryBackend {
    fn words(&self) -> &'static Words;

    /// The items on these tracks (all tracks when none), and only the selected ones when `selected`.
    async fn items(&self, tracks: &[String], selected: bool, signal: &Signal) -> Result<Vec<Item>, String>;

    async fn regions(&self, signal: &Signal) -> Result<Vec<Region>, String>;

    /// Adds the regions in one change: what the DAW answered.
    async fn add_regions(&self, regions: &[NewRegion], signal: &Signal) -> Result<Value, String>;

    /// The files a render with `settings` (the tool's own keys, passed as given) would write, rendering nothing.
    async fn targets(&self, settings: &Value, signal: &Signal) -> Result<Targets, String>;

    /// Renders with `settings`, refusing unless the DAW would write exactly `expect`: each file written, as the DAW says.
    async fn render(&self, settings: &Value, expect: &[String], signal: &Signal) -> Result<Value, String>;
}
