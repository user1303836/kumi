use ableton_mcp_server::{journeys::*, live::LiveStatus};
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static ORACLE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("fixtures/journeys-oracle.json")).unwrap());
fn status(index: usize) -> LiveStatus {
    serde_json::from_value(ORACLE["statuses"][index].clone()).unwrap()
}
fn plan(journey: &str, traits: &str, connected: bool) -> Value {
    plan_user_journey(&json!({"journey":journey,"traits":traits}), &status(usize::from(connected))).unwrap()
}
fn stage<'a>(plan: &'a Value, id: &str) -> &'a Value {
    plan["stages"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap()
}
fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text))
}

#[test]
fn complete_plans_and_rendered_prompts_match_typescript_across_traits_and_negotiation() {
    for case in ORACLE["cases"].as_array().unwrap() {
        let input = &ORACLE["inputs"][case["input"].as_u64().unwrap() as usize];
        let status = status(case["status"].as_u64().unwrap() as usize);
        let actual = plan_user_journey(input, &status).unwrap();
        if let Some(expected) = case.get("plan") {
            assert_eq!(stringify(&actual), stringify(expected), "{input}");
        }
        assert_eq!(hash(&stringify(&actual)), case["sha256"], "input={input}, status={}", case["status"]);
        assert_eq!(
            hash(&render_journey_prompt(input, &status).unwrap()),
            case["promptSha256"],
            "prompt input={input}, status={}",
            case["status"]
        );
    }
    for (i, expected) in ORACLE["resources"].as_array().unwrap().iter().enumerate() {
        assert_eq!(hash(&stringify(&journey_resource(&status(i)))), *expected, "resource status {i}");
    }
}

#[test]
fn catalog_exposes_five_ordered_semantic_journeys_with_authority_and_recovery() {
    assert_eq!(JOURNEY_CATALOG.iter().map(|j| j["id"].as_str().unwrap()).collect::<Vec<_>>(), JOURNEY_IDS);
    for j in JOURNEY_CATALOG.iter() {
        assert!(
            j["title"].as_str().unwrap().len() > 8
                && j["summary"].as_str().unwrap().len() > 40
                && j["fallback"].as_str().unwrap().len() > 80
        );
        assert!(j["stages"].as_array().unwrap().len() >= 5);
        for s in j["stages"].as_array().unwrap() {
            assert!(s["announcement"].as_str().unwrap().ends_with('.'));
            assert!(!s["authorities"].as_array().unwrap().is_empty());
            assert!(s["authorities"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| ["none", "fixed-phrase", "unpredictable-preview-token"].contains(&a["mechanism"].as_str().unwrap())));
            for (key, bound) in [("verification", 20), ("recovery", 10), ("unavailableFallback", 10)] {
                assert!(s[key].as_str().unwrap().len() > bound);
            }
        }
    }
}

#[test]
fn complete_plans_remain_non_authoritative_and_disclose_progress_rights_and_recovery() {
    for journey in JOURNEY_IDS {
        let p = plan_user_journey(
            &json!({"journey":journey,"traits":"syncopated, spacious, warm, controlled","experienceLevel":"advanced","bars":8}),
            &status(1),
        )
        .unwrap();
        assert_eq!(p["version"], JOURNEY_PLAN_VERSION);
        assert_eq!(p["executable"], true);
        assert_eq!(p["mode"], "capability-complete");
        for (i, s) in p["stages"].as_array().unwrap().iter().enumerate() {
            assert_eq!(s["order"], i + 1);
            assert_eq!(s["status"], "planned");
            assert_eq!(s["available"], true);
        }
        assert!(p["stages"].as_array().unwrap().iter().any(|s| s["authorities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["mechanism"] != "none")));
        assert_eq!(p["rights"]["exactReplicationDelivered"], false);
        assert_eq!(p["rights"]["legalClearanceClaimed"], false);
        assert_eq!(p["accessibility"]["nonColorStatusLabels"], true);
        assert_eq!(p["accessibility"]["mouseOnlyInstructions"], false);
        assert_eq!(p["progress"]["terminalResultRequiresResidualState"], true);
        assert_eq!(p["residualStateTemplate"], json!({"status":"not-started","requiredAtTerminal":true,"items":[]}));
        let text = stringify(&p);
        for absent in ["transactionId", "confirmationToken", "idempotencyKey"] {
            assert!(!text.contains(absent));
        }
    }
}

#[test]
fn unavailable_live_preserves_local_analysis_and_truthful_fallbacks() {
    for journey in JOURNEY_IDS {
        let p = plan(journey, "clear and controlled", false);
        if *journey == "compare-reference-mix" {
            assert_eq!(p["executable"], true);
            assert_eq!(p["mode"], "local-analysis");
            assert_eq!(p["advanced"]["missingCapabilities"], json!([]));
            assert_eq!(p["advanced"]["missingOperations"], json!([]));
        } else {
            assert_eq!(p["executable"], false);
            assert_eq!(p["mode"], "capability-limited");
            assert!(!p["advanced"]["missingCapabilities"].as_array().unwrap().is_empty());
            assert!(p["beginner"]["summary"].as_str().unwrap().contains("cannot complete"));
        }
    }
    let resource = journey_resource(&status(0));
    assert_eq!(resource["journeys"].as_array().unwrap().len(), 5);
    assert!(resource["journeys"].as_array().unwrap().iter().all(|j| j["fallback"].as_str().unwrap().len() > 40));
    assert!(resource["authority"].as_str().unwrap().contains("requires its purpose-specific preview"));
}

#[test]
fn optional_stages_negotiate_independently() {
    let count = ORACLE["statuses"].as_array().unwrap().len();
    let sound =
        plan_user_journey(&json!({"journey":"design-owned-sound","traits":"warm spacious controlled"}), &status(count - 2)).unwrap();
    assert_eq!(sound["executable"], true);
    assert_eq!(sound["mode"], "core-capability-complete");
    assert_eq!(stage(&sound, "apply-load")["status"], "planned");
    for id in ["shape-published-controls", "audition"] {
        assert_eq!(stage(&sound, id)["status"], "unavailable");
    }
    assert_eq!(
        sound["advanced"]["unavailableOptionalStages"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(),
        ["shape-published-controls", "audition"]
    );
    let reference = plan("compare-reference-mix", "balanced clear", false);
    for s in reference["stages"].as_array().unwrap() {
        assert_eq!(s["status"], if s["requiredForCore"] == true { "planned" } else { "unavailable" });
    }
    let performance =
        plan_user_journey(&json!({"journey":"diagnose-performance-setup","traits":"controlled clear"}), &status(count - 1)).unwrap();
    assert_eq!(performance["executable"], true);
    assert_eq!(performance["mode"], "core-capability-complete");
    for id in ["bounded-recording", "bounded-realtime"] {
        assert_eq!(stage(&performance, id)["status"], "unavailable");
    }
    let mut no_read = status(1);
    no_read.capabilities.retain(|c| c.as_str() != "session.read");
    let arrangement = plan_user_journey(&json!({"journey":"create-beat-or-song","traits":"syncopated warm"}), &no_read).unwrap();
    assert_eq!(stage(&arrangement, "arrange")["status"], "unavailable");
}

#[test]
fn authority_metadata_matches_each_purpose_specific_tool() {
    for (journey, id, tool, mechanism, phrase) in [
        ("create-beat-or-song", "revise", "live_note_update_apply", "fixed-phrase", Some("apply")),
        ("compare-reference-mix", "reversible-hypothesis", "live_mixer_apply", "fixed-phrase", Some("apply")),
        ("diagnose-performance-setup", "apply-fixes", "live_routing_apply", "fixed-phrase", Some("apply")),
        ("diagnose-performance-setup", "bounded-recording", "live_recording_apply", "fixed-phrase", Some("apply")),
        ("compare-reference-mix", "guarded-capture", "live_audio_capture_apply", "unpredictable-preview-token", None),
        ("compare-reference-mix", "guarded-capture", "live_audio_capture_emergency_stop", "fixed-phrase", Some("emergency-stop-and-clean")),
        ("diagnose-performance-setup", "bounded-realtime", "live_realtime_arm_apply", "fixed-phrase", Some("apply")),
        ("diagnose-performance-setup", "bounded-realtime", "live_realtime_disarm", "fixed-phrase", Some("disarm")),
        ("create-beat-or-song", "apply-create", "live_undo", "fixed-phrase", Some("undo")),
    ] {
        let p = plan(journey, "syncopated warm controlled clear", true);
        let authority = stage(&p, id)["authorities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["tools"].as_array().unwrap().iter().any(|t| t == tool))
            .unwrap();
        assert_eq!(authority["mechanism"], mechanism);
        assert_eq!(authority["phrase"].as_str(), phrase);
    }
    assert_eq!(stage(&plan("diagnose-performance-setup", "clear", true), "final-readback")["tools"], json!(["live_discover"]));
}

#[test]
fn deterministic_bounded_planning_rejects_malformed_input_and_tracks_identity() {
    for case in ORACLE["invalid"].as_array().unwrap() {
        assert_eq!(plan_user_journey(&case["input"], &status(1)).unwrap_err().to_string(), case["error"]);
    }
    let input = json!({"journey":"create-beat-or-song","traits":"broken beat, sparse bass","experienceLevel":"beginner","bars":4});
    let p = plan_user_journey(&input, &status(1)).unwrap();
    assert_eq!(p["planId"], plan_user_journey(&input, &status(1)).unwrap()["planId"]);
    let mut longer = input.clone();
    longer["bars"] = json!(8);
    assert_ne!(p["planId"], plan_user_journey(&longer, &status(1)).unwrap()["planId"]);
    assert_ne!(p["planId"], plan_user_journey(&input, &status(2)).unwrap()["planId"]);
    assert_ne!(p["planId"], plan_user_journey(&input, &status(0)).unwrap()["planId"]);
    assert_eq!(stage(&p, "audition")["requiredProvenance"], Value::Null);
    let events = p["guidance"]["drumRoleEvents"].as_array().unwrap();
    assert!(events.iter().any(|e| e["startBeat"].as_f64().unwrap().fract() == 0.75));
    assert!(events.iter().all(|e| e["pitch"].is_null()));
}

#[test]
fn prompt_copy_detection_blocks_coincident_traits_and_avoids_substring_matches() {
    for text in [
        "copy Artist X's exact signature patch",
        "in the style of Bright Eyes",
        "sound like Major Lazer",
        "copy Dark Star exactly",
        "majority business",
        "brightness and softness",
    ] {
        let p = plan("design-owned-sound", text, true);
        assert_eq!(p["mode"], "intent-clarification-required");
        assert_eq!(p["executable"], false);
        assert_eq!(p["intent"]["highLevelTraits"], json!([]));
        assert!(p["stages"].as_array().unwrap().iter().all(|s| s["status"] == "blocked-by-intent" && s["available"] == false));
    }
    let p = plan("design-owned-sound", "Warm Spacious", true);
    assert_eq!(p["intent"]["identityReferenceMayBePresent"], false);
    assert_eq!(
        p["intent"]["highLevelTraits"].as_array().unwrap().iter().map(|t| t["value"].as_str().unwrap()).collect::<Vec<_>>(),
        ["warm", "spacious"]
    );
    let prompt =
        render_journey_prompt(&json!({"journey":"design-owned-sound","traits":"copy Artist X's exact signature patch"}), &status(0))
            .unwrap();
    for text in [
        "intent-clarification-required",
        "Never forward untrustedOriginalRequest names or exact-copy wording",
        "Do not promise exact replication or legal clearance",
        "status by color alone",
        "report uncertain state",
    ] {
        assert!(prompt.contains(text));
    }
}
