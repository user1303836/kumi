#[path = "../../../tests/support/chunks.rs"]
mod chunks;
use ableton_mcp_server::{live::LiveStatus, tool_catalog::*};
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/tool-catalog-oracle.json")).unwrap()
}

#[test]
fn catalog_has_one_rule_and_class_for_each_unique_tool() {
    assert!(TOOL_CATALOG.len() > 140);
    assert_eq!(TOOL_CATALOG.iter().map(|e| &e.name).collect::<HashSet<_>>().len(), TOOL_CATALOG.len());
    assert!(tool_catalog_entry("live_project_save").is_none());
    assert!(tool_catalog_entry("live_project_open").is_none());
    for entry in TOOL_CATALOG.iter() {
        assert!(TOOL_POLICY_CLASSES.contains(&entry.policy_class));
    }
}

/// The oracle's cases in 12 tests that nextest runs side by side.
mod exact_visibility_and_descriptors_match_typescript_for_every_capability_operation_and_profile {
    crate::chunks::chunked!(super::check_visibility_cases; part_00 = 0, part_01 = 1, part_02 = 2, part_03 = 3, part_04 = 4,
        part_05 = 5, part_06 = 6, part_07 = 7, part_08 = 8, part_09 = 9, part_10 = 10, part_11 = 11);
}

/// The oracle's cases at index `chunk`, `chunk + chunks`, …
fn check_visibility_cases(chunk: usize, chunks: usize) {
    let fixture = oracle();
    let statuses: Vec<LiveStatus> = serde_json::from_value(fixture["statuses"].clone()).unwrap();
    let policies: Vec<ToolPolicySpec> = serde_json::from_value(fixture["policies"].clone()).unwrap();
    assert!(fixture["cases"].as_array().unwrap().len() > 2000);
    for case in fixture["cases"].as_array().unwrap().iter().skip(chunk).step_by(chunks) {
        let status = &statuses[case["status"].as_u64().unwrap() as usize];
        let policy = &policies[case["policy"].as_u64().unwrap() as usize];
        let rows: Vec<_> = resolve_tool_visibility(status, policy)
            .unwrap()
            .iter()
            .map(|r| json!({"name":r.entry.name,"executable":r.executable,"policyAllowed":r.policy_allowed,"visible":r.visible}))
            .collect();
        let result = json!({"rows":rows,"descriptors":visible_tool_descriptors(status, policy).unwrap()});
        let digest = hex::encode(Sha256::digest(stringify(&result)));
        assert_eq!(digest, case["sha256"].as_str().unwrap(), "status={}, policy={policy:?}", case["status"]);
    }
}

#[test]
fn policy_validation_and_environment_preserve_reference_errors_defaults_and_unknown_names() {
    let fixture = oracle();
    for case in fixture["invalid"].as_array().unwrap() {
        assert_eq!(parse_tool_policy_spec(Some(&case["input"])).unwrap_err().to_string(), case["error"]);
    }
    assert_eq!(parse_tool_policy_spec(None).unwrap(), *DEFAULT_TOOL_POLICY);
    assert_eq!(tool_policy_from_env(&HashMap::new()).unwrap(), *DEFAULT_TOOL_POLICY);
    let env = HashMap::from([
        ("ABLETON_MCP_TOOL_POLICY".into(), "performance".into()),
        ("ABLETON_MCP_TOOL_DENY".into(), "live_realtime_*, live_tempo_apply".into()),
    ]);
    let policy = tool_policy_from_env(&env).unwrap();
    assert_eq!(policy.profile, "performance");
    assert_eq!(policy.deny, ["live_realtime_*", "live_tempo_apply"]);
    let next_line = HashMap::from([("ABLETON_MCP_TOOL_ALLOW".into(), "\u{0085}live_status".into())]);
    assert_eq!(tool_policy_from_env(&next_line).unwrap_err().to_string(), "tool policy allow list is invalid");
    // A `*` anywhere but at the end would match nothing: `*python*` denied nothing while looking as if it did.
    for pattern in ["*python*", "*python", "live_*_apply"] {
        let env = HashMap::from([("ABLETON_MCP_TOOL_DENY".into(), pattern.into())]);
        assert_eq!(tool_policy_from_env(&env).unwrap_err().to_string(), "tool policy deny list is invalid", "{pattern}");
    }
    let status: LiveStatus = serde_json::from_value(fixture["statuses"][1].clone()).unwrap();
    let names = |value: Value| {
        resolve_tool_visibility(&status, &parse_tool_policy_spec(Some(&value)).unwrap())
            .unwrap()
            .iter()
            .filter(|r| r.policy_allowed)
            .map(|r| r.entry.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(json!({"allow":["live_nonexistent_*","live_status","live_retired_tool"]})), ["live_status"]);
    assert_eq!(names(json!({"deny":["live_retired_tool"]})), names(json!({})));
    assert!(names(json!({"allow":["live_retired_tool"]})).is_empty());
    // Preserve the reference parser's inherited-property edge without a process panic.
    let inherited = parse_tool_policy_spec(Some(&json!({"profile":"toString"}))).unwrap();
    assert_eq!(
        tool_allowed_by_policy(&TOOL_CATALOG[0], &inherited).unwrap_err().to_string(),
        "Cannot read properties of undefined (reading 'includes')"
    );
}

#[test]
fn disconnected_surface_has_only_reference_local_and_always_available_tools() {
    let status: LiveStatus = serde_json::from_value(oracle()["statuses"][0].clone()).unwrap();
    let descriptors = visible_tool_descriptors(&status, &DEFAULT_TOOL_POLICY).unwrap();
    assert_eq!(
        descriptors.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        [
            "server_status",
            "capabilities",
            "plan_user_journey",
            "audio_analyze",
            "audio_compare_reference",
            "live_status",
            "live_library_search",
            "live_project_snapshot_diff",
            "als_read",
            "als_lint",
            "als_diff"
        ]
    );
    for descriptor in descriptors {
        assert_eq!(descriptor.input_schema["type"], "object");
    }
}

#[test]
fn all_profiles_filter_negotiated_surface_and_performance_retains_recovery() {
    let status: LiveStatus = serde_json::from_value(oracle()["statuses"][1].clone()).unwrap();
    let names = |profile: &str| {
        visible_tool_descriptors(&status, &ToolPolicySpec { profile: profile.into(), ..Default::default() })
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
    };
    let full = names("full");
    let read = names("read-only");
    let edit = names("edit-no-audio");
    let performance = names("performance");
    assert!(full.len() > 140);
    for name in ["server_status", "capabilities", "live_status", "live_snapshot", "live_browser_search"] {
        assert!(read.iter().any(|n| n == name));
    }
    for name in ["live_tempo_preview", "live_session_structure_preview", "live_audio_clip_preview", "live_recording_preview", "live_undo"] {
        assert!(!read.iter().any(|n| n == name));
    }
    for name in ["live_tempo_preview", "live_session_structure_preview", "live_mixer_preview", "live_undo"] {
        assert!(edit.iter().any(|n| n == name));
    }
    for name in [
        "live_audio_clip_preview",
        "live_warp_marker_preview",
        "live_audio_import_preview",
        "live_recording_preview",
        "live_realtime_arm_preview",
        "live_audio_capture_preview",
        "live_transport_preview",
        "live_clip_launch_preview",
    ] {
        assert!(!edit.iter().any(|n| n == name));
    }
    for name in [
        "live_transport_preview",
        "live_tempo_preview",
        "live_clip_launch_preview",
        "live_session_emergency_stop",
        "live_mixer_preview",
        "live_view_preview",
        "live_undo",
        "live_recovery_finalize",
    ] {
        assert!(performance.iter().any(|n| n == name));
    }
    for name in ["live_session_structure_preview", "live_note_update_preview", "live_device_apply", "live_recording_preview"] {
        assert!(!performance.iter().any(|n| n == name));
    }
    assert!(read.len() < edit.len() && read.len() < performance.len() && performance.len() < full.len() && edit.len() < full.len());
}
