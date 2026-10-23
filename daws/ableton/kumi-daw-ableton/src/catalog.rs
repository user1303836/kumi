//! Which of Live's tools every request carries. The rest wait behind find_tools (kernel/deferred.rs), so a
//! request stays small: changes and actions are core where changes.json or actions.json say so, the other
//! tools where `CORE` lists them, picked by what producers' conversations call most. A change or action
//! that isn't core is still a make_changes step, and make_changes' description gives each one's input.

use super::actions::ACTIONS;
use super::changes::CHANGES;

/// Live's tools besides changes and actions that every request carries.
pub const CORE: &[&str] = &[
    "live_discover",
    "live_browser_search",
    "live_song_state",
    "make_changes",
    "undo_change",
    "run_python",
    "find_sounds",
    "read_notes",
    "audition",
];

/// The tool that runs changes and actions as steps.
pub const PLAN_TOOL: &str = "make_changes";

/// Whether a Live tool waits behind find_tools instead of going with every request.
pub fn deferred(name: &str) -> bool {
    if let Some(kind) = CHANGES.iter().find(|kind| kind.tool == name) {
        return !kind.core;
    }
    if let Some(kind) = ACTIONS.iter().find(|kind| kind.tool == name) {
        return !kind.core;
    }
    !CORE.contains(&name)
}

/// Whether a Live tool is a change or an action: a step make_changes can run.
pub fn step(name: &str) -> bool {
    CHANGES.iter().any(|kind| kind.tool == name) || ACTIONS.iter().any(|kind| kind.tool == name)
}

pub use crate::integrations::shared::steps::step_line;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_changes_and_actions_come_from_their_files_and_the_rest_wait() {
        for kind in CHANGES.iter() {
            assert_eq!(deferred(&kind.tool), !kind.core, "{}", kind.tool);
            assert!(step(&kind.tool));
        }
        assert!(!deferred("set_device_parameter") && !deferred("play"));
        assert!(deferred("set_scale") && deferred("select"));
        assert!(!deferred("live_discover") && !deferred("make_changes"));
        assert!(deferred("judge") && deferred("live_key_estimate") && deferred("arrange"));
        assert!(!step("judge") && !step("make_changes"));
    }
}
