//! Kumi's part in a DAW as an agent host's setup puts it there (Kumi for Hermes, C8's `setup_daw`): a plan read from
//! files alone that names every path putting it in writes, and that plan carried out with no question asked, no wait,
//! and the DAW neither started nor quit. The DAW's crate makes and carries out its own plans
//! ([`crate::provider::DawProvider::component_plan`]); `kumi serve` decides from them what a call may do.
use serde::Serialize;
use std::path::Path;

/// Where one part of Kumi's in the DAW stands, against what this Kumi brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartState {
    /// It isn't there.
    Missing,
    /// It's this Kumi's own, as this Kumi brings it.
    Same,
    /// It's an earlier one: putting it in brings it up to date.
    Older,
    /// It's a newer one than this Kumi brings: it stays, never downgraded.
    Newer,
    /// Another Kumi on this computer put it there (its Kumi home isn't this one), or nothing says whose it is: it stays.
    OtherOwner,
}

/// What the producer does once the part is in, before Kumi reaches the DAW through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum After {
    /// Live: open it, and choose Kumi's Remote Script as a Control Surface.
    ChooseControlSurface,
    /// Restart the DAW (or open it) so it loads the part.
    RestartDaw,
    /// Turn the tool on in the DAW.
    EnableTool,
    /// Nothing: the DAW loads it the next time it opens.
    None,
}

/// One part of Kumi's in the DAW (Live's Remote Script, Kumi Ears), where it is and how it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Part {
    pub name: &'static str,
    pub path: String,
    pub state: PartState,
}

/// Kumi's part in the DAW for one Kumi home: how it stands, and what putting it in would write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentPlan {
    /// What it is, in the producer's words ("Live Remote Script").
    pub component: &'static str,
    /// The version of the part that is in the DAW once this plan has run: the one this Kumi brings, or the one there
    /// when it stays (newer, or another Kumi's).
    pub version: Option<String>,
    /// How the whole part stands: another Kumi's or newer when its main part is; else missing or older when any part
    /// is; else the same.
    pub state: PartState,
    /// The Kumi home the part belongs to when it's another's (`OtherOwner`), when the DAW's files say.
    pub owner: Option<String>,
    /// Every path putting it in leaves changed (written or removed), each whole: nothing when there's nothing to put in,
    /// or it must stay. While it works it may also make temporary files beside these, each named for what it replaces
    /// with a random suffix, and removes them before it's done (one an interruption leaves, the next setup removes):
    /// the effects say where.
    pub paths: Vec<String>,
    /// What putting it in does, in plain sentences naming those paths.
    pub effects: Vec<String>,
    pub after: After,
    /// Each part, where the DAW's part has more than one (Live's Remote Script and Kumi Ears).
    pub parts: Vec<Part>,
    /// Whether the part, once in, keeps the DAW's own id for a project only while a standalone Kumi asks it to (CC-8):
    /// false where its files don't say so.
    pub keep_identity: bool,
}

/// Why carrying out a plan stopped. Nothing is left half done: a part that was being replaced is put back as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The DAW has the part's files in use (Live is open while its Remote Script would change): quit it and try again.
    InUse(String),
    /// What's on disk isn't what the plan read any more: plan again.
    Changed(String),
    /// It failed, in words.
    Failed(String),
}

/// Whether `a` and `b` are the same folder on disk (links resolved); false when either isn't there.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}
