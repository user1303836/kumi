//! Every setting a search hears on a device, handed out as it's heard, for Kumi to remember: a later search on the same
//! device can start from what worked there. Listening only says what it heard (`SettingsHeard`); Kumi's database keeps
//! it.

use indexmap::IndexMap;
use kumi_types::daw::DawId;
use serde_json::Value;

/// Which search heard them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    /// tune's knob search.
    Tune,
    /// A match run's knob search.
    Match,
    /// A goal's evolution.
    Goal,
}

/// Who a device is, the same in every session and wherever it sits (never its place on a track): a built-in device's own
/// name in its DAW, a plug-in's format, maker and name as far as the DAW says them, and its version when the DAW says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub name: String,
    pub version: Option<String>,
}

impl DeviceIdentity {
    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into(), version: None }
    }
}

/// One setting heard: its knobs by name (each a fraction of its range), how it sounded, and its cost (lower is better,
/// within its search).
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub knobs: IndexMap<String, f64>,
    pub measured: Value,
    pub cost: f64,
}

/// What a search hands out as it goes.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsHeard {
    /// Settings heard on one device in one go (a generation): the search they're in (`run`), where they were heard, what
    /// the search aimed at, how the track sounded before the search changed anything, and each setting.
    Heard {
        run: String,
        kind: SearchKind,
        daw: Option<DawId>,
        device: DeviceIdentity,
        track: Option<String>,
        target: Value,
        before: Option<Value>,
        settings: Vec<Setting>,
    },
    /// The search ended (however it did): its best can be told from what it heard.
    Ended { run: String },
}
