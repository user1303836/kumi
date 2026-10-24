//! Every setting a search hears on a device, handed out as it's heard, for Kumi to remember, and what was heard before on
//! a device, for a search there to start from what worked. Listening only says what it hears and asks what was heard
//! (`SettingsMemory`); Kumi's database keeps it.

use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_types::daw::DawId;
use serde_json::Value;

/// What Kumi remembers of searches, as a search sees it (Kumi's database, through the agent).
#[async_trait(?Send)]
pub trait SettingsMemory {
    /// Settings a search heard, or its end, to keep.
    fn heard(&self, event: SettingsHeard);
    /// The best setting of each past search of these kinds on a device, in a DAW (the session's own when it's None),
    /// newest first.
    async fn bests(&self, daw: Option<DawId>, device: &str, kinds: &[SearchKind]) -> Vec<Remembered>;
}

/// A past search's best setting on a device, as remembered.
#[derive(Debug, Clone, PartialEq)]
pub struct Remembered {
    /// Its row, to say a new search heard it again.
    pub id: String,
    /// When it was heard (ms).
    pub at: i64,
    /// The track it was on.
    pub track: Option<String>,
    pub kind: SearchKind,
    /// Its knobs by name, each a fraction of its range (a perceptual one for tune's).
    pub knobs: IndexMap<String, f64>,
    /// What its search aimed at, and how the track sounded before that search (as `SettingsHeard::Heard` said them).
    pub target: Value,
    pub before: Option<Value>,
    pub cost: f64,
}

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
/// within its search); which of its search's generations heard it (1 the first), and the remembered setting it is, when
/// it was one heard again (`Remembered::id`).
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub knobs: IndexMap<String, f64>,
    pub measured: Value,
    pub cost: f64,
    pub generation: Option<u32>,
    pub remembered: Option<String>,
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
    /// The search ended (however it did): its best can be told from what it heard. `kept`: the device took it (it beat
    /// where the knobs were); a search whose best wasn't kept marks none, so it isn't remembered as what worked.
    Ended { run: String, kept: bool },
}
