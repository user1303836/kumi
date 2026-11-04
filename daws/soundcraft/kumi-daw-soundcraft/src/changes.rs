//! What Kumi changes in SoundCraft: nothing yet. Its bridge reads sessions; its change kinds come with the control channel's
//! edits. With none, its integration offers no change tools, and its rules have the producer make the fix Kumi names.
use kumi_daw_own::changes::OwnChangeSet;

pub static CHANGES: OwnChangeSet = OwnChangeSet {
    kinds: &[],
    actions: &[],
    plan_description: "",
    undo_description: "",
    instructions: "Kumi reads SoundCraft's sessions but can't change them yet: say what you'd change and where, and the producer makes it.",
    plan_steps: false,
    most_track_adds: None,
};
