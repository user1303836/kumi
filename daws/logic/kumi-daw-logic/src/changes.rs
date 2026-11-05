//! What Kumi can change in Logic Pro: nothing yet. M1 and M2 read Logic; M3 brings its changes and undo, as REAPER's
//! and Renoise's (`reaper_changes.rs`, `renoise_changes.rs`) came with theirs. An empty set gives the model no change
//! tools and adds nothing to Logic's instructions.
use kumi_daw_own::changes::OwnChangeSet;

pub static CHANGES: OwnChangeSet = OwnChangeSet {
    kinds: &[],
    actions: &[],
    plan_description: "",
    undo_description: "",
    instructions: "",
    plan_steps: false,
    most_track_adds: None,
};
