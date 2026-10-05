//! Follow actions, as far as they run without the host.
//!
//! The TypeScript tests drive `live_follow_actions_preview/apply` and `live_undo` through `McpHost`;
//! those belong with the host port. What the simulator and the validator do on their own is tested here:
//! the field list, the validation messages, and `clip.follow-actions.set`'s own refusals.

use ableton_mcp_server::follow_actions::{validate_follow_actions, FOLLOW_ACTION_FIELDS, FOLLOW_ACTION_SCHEMA};
use serde_json::{json, Map, Value};

#[path = "support/mod.rs"]
mod support;
use support::assert_throws;

fn initial() -> Map<String, Value> {
    json!({ "followActionEnabled": false, "followActionLinked": true, "followActionA": 4, "followActionB": 0, "followActionChanceA": 100, "followActionChanceB": 0, "followActionLoopCount": 1, "followActionTime": 4, "followActionJumpA": 1, "followActionJumpB": 1 })
        .as_object()
        .cloned()
        .unwrap()
}

fn with(base: &Map<String, Value>, patch: Value) -> Map<String, Value> {
    let mut merged = base.clone();
    for (key, value) in patch.as_object().cloned().unwrap_or_default() {
        merged.insert(key, value);
    }
    merged
}

#[test]
fn the_follow_action_fields_come_from_the_registry_in_its_order() {
    assert_eq!(
        FOLLOW_ACTION_FIELDS.as_slice(),
        [
            "followActionEnabled",
            "followActionLinked",
            "followActionA",
            "followActionB",
            "followActionChanceA",
            "followActionChanceB",
            "followActionLoopCount",
            "followActionTime",
            "followActionJumpA",
            "followActionJumpB"
        ]
    );
    assert_eq!(FOLLOW_ACTION_SCHEMA.len(), FOLLOW_ACTION_FIELDS.len());
    let time = &FOLLOW_ACTION_SCHEMA.iter().find(|(field, _)| field == "followActionTime").unwrap().1;
    assert_eq!((time.minimum, time.maximum), (Some(0.25), Some(1_000_000_000.0)));
}

#[test]
fn follow_actions_refuse_malformed_values() {
    validate_follow_actions(&initial()).unwrap();
    assert_throws(
        validate_follow_actions(&with(&initial(), json!({ "followActionA": 10 }))),
        "followActionA is unavailable or out of bounds",
    );
    assert_throws(
        validate_follow_actions(&with(&initial(), json!({ "followActionJumpA": 0 }))),
        "followActionJumpA is unavailable or out of bounds",
    );
    assert_throws(
        validate_follow_actions(&with(&initial(), json!({ "followActionA": 4.5 }))),
        "followActionA is unavailable or out of bounds",
    );
    assert_throws(
        validate_follow_actions(&with(&initial(), json!({ "followActionEnabled": "yes" }))),
        "followActionEnabled must be boolean",
    );
}

#[test]
fn follow_validation_errors_retain_their_useful_reasons() {
    assert_throws(
        validate_follow_actions(&with(&initial(), json!({ "followActionChanceA": 20, "followActionChanceB": 30 }))),
        "probabilities must sum to 100",
    );
    let mut missing = initial();
    missing.remove("followActionLoopCount");
    assert_throws(validate_follow_actions(&missing), "followActionLoopCount is unavailable or out of bounds");
}

#[test]
fn follow_timing_accepts_fractional_time_while_integral_fields_remain_exact() {
    validate_follow_actions(&with(&initial(), json!({ "followActionTime": 1.333 }))).unwrap();
    validate_follow_actions(&with(&initial(), json!({ "followActionTime": 0.25 }))).unwrap();
    assert_throws(
        validate_follow_actions(&with(&initial(), json!({ "followActionTime": 0.2 }))),
        "followActionTime is unavailable or out of bounds",
    );
}
