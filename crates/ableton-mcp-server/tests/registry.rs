use std::path::PathBuf;
use std::process::Command;

use ableton_mcp_server::registry::{
    live_registry_hash, load_live_registry, validate_live_operation_request, validate_live_operation_result, LiveRegistryMethod,
};
use serde_json::{json, Map, Value};

#[path = "support/mod.rs"]
mod support;
use support::assert_throws;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

// The Remote Script hashes the registry with Python's json.dumps and the host with JSON.stringify:
// a number the two spell differently (1e-06 against 0.000001) would keep Live from ever connecting.
#[test]
fn the_remote_script_and_the_host_compute_the_same_registry_hash() {
    if !Command::new("python3").arg("--version").output().map(|output| output.status.success()).unwrap_or(false) {
        eprintln!("skipped: python3 is unavailable");
        return;
    }
    let remote_script = repository_root().join("remote-script");
    let python = Command::new("python3")
        .arg("-c")
        .arg("import sys; sys.path.insert(0, sys.argv[1]); from ableton_mcp_remote_script import operation_registry; print(operation_registry()[1])")
        .arg(&remote_script)
        .output()
        .expect("python3 runs");
    assert!(python.status.success(), "{}", String::from_utf8_lossy(&python.stderr));
    assert_eq!(String::from_utf8_lossy(&python.stdout).trim(), live_registry_hash());
}

// A bridge started from a folder that holds another registry (a checkout of another version) keeps its own.
#[test]
fn the_bridges_registry_is_its_own_whatever_folder_its_started_from() {
    // The probe: this very test, started again from the other folder, prints the hash it loads.
    if std::env::var_os("KUMI_REGISTRY_HASH_PROBE").is_some() {
        println!("registry-hash={}", live_registry_hash());
        return;
    }
    let folder = tempfile::Builder::new().prefix("registry-cwd-").tempdir().expect("a temp folder");
    let other = json!({ "version": 1, "protocol": "ableton-live/v1", "operations": [] }).to_string();
    std::fs::create_dir_all(folder.path().join("protocol")).unwrap();
    std::fs::write(folder.path().join("protocol").join("ableton-live-v1.operations.json"), &other).unwrap();
    // Started two folders down, the other registry is both in the folder above that and two above.
    std::fs::create_dir_all(folder.path().join("a").join("b").join("protocol")).unwrap();
    std::fs::write(folder.path().join("a").join("b").join("protocol").join("ableton-live-v1.operations.json"), &other).unwrap();
    let started = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["the_bridges_registry_is_its_own_whatever_folder_its_started_from", "--exact", "--nocapture"])
        .env("KUMI_REGISTRY_HASH_PROBE", "1")
        .current_dir(folder.path().join("a").join("b"))
        .output()
        .expect("the probe runs");
    assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));
    let stdout = String::from_utf8_lossy(&started.stdout);
    let printed = stdout.lines().find_map(|line| line.strip_prefix("registry-hash=")).expect("the probe printed its hash");
    assert_eq!(printed, live_registry_hash());
}

fn output_safety() -> Value {
    json!({ "safe": true, "provenance": "test-operator" })
}

fn playback() -> Value {
    json!({
        "ref": "1:session_playback:playback",
        "epoch": 1,
        "revision": "1:playback:abc",
        "transport": {
            "playing": false,
            "arrangementRecord": null,
            "sessionRecord": false,
            "position": 0,
            "launchQuantization": { "raw": "1_bar", "normalized": "1-bar" },
            "loop": { "enabled": false, "start": 0, "length": 4 },
            "punchIn": false,
            "punchOut": false,
            "metronome": null,
            "countIn": 1,
        },
        "firedTargets": [],
        "playingTargets": [{ "trackRef": "1:track:0", "clipSlotRef": "1:clip_slot:0:0", "sceneRef": "1:scene:0", "sceneIndex": 0, "clipRef": "1:clip:0:0" }],
    })
}

/// `{ ...base, ...patch }`, with a patch value of `undefined` (here: `Value::Null` under a key listed in `remove`) dropping the key.
fn with(base: &Value, patch: Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    for (key, value) in patch.as_object().cloned().unwrap_or_default() {
        merged.insert(key, value);
    }
    Value::Object(merged)
}

fn without(base: &Value, key: &str) -> Value {
    let mut map: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    map.remove(key);
    Value::Object(map)
}

#[test]
fn canonical_registry_includes_strict_snapshot_and_playback_contracts() {
    let registry = load_live_registry();
    assert_eq!(registry.operation("snapshot").map(|item| item.method), Some(LiveRegistryMethod::Snapshot));
    assert_eq!(registry.operation("session.playback").map(|item| item.method), Some(LiveRegistryMethod::Discover));
    validate_live_operation_request("session.playback", &json!({})).unwrap();
    validate_live_operation_result("session.playback", &playback()).unwrap();
    let note = json!({ "pitch": 36, "start": 0, "duration": 0.25, "velocity": 100, "channel": 1 });
    let note_authority = json!({ "expectedObjectIdentity": "live:clip:0", "expectedTrackRef": "1:track:0", "expectedTrackIdentity": "live:track:0", "expectedSlotRef": "1:clip_slot:0:0", "expectedSlotIdentity": "live:slot:0", "expectedSceneRef": "1:scene:0", "expectedSceneIdentity": "live:scene:0" });
    validate_live_operation_request(
        "note.add-batch",
        &json!({ "ref": "1:clip:0:0", "notes": [note, with(&note, json!({ "pitch": 38, "start": 1 }))], "expectedClipAuthority": note_authority, "expectedNotesRevision": "a".repeat(64) }),
    )
    .unwrap();
    validate_live_operation_result("note.add-batch", &json!({ "added": 2, "noteIds": [1, null], "notesRevision": "b".repeat(64) }))
        .unwrap();
    assert_throws(
        validate_live_operation_request(
            "note.add-batch",
            &json!({ "ref": "1:clip:0:0", "notes": [], "expectedClipAuthority": note_authority, "expectedNotesRevision": "a".repeat(64) }),
        ),
        "below registry item bound",
    );
    assert_throws(
        validate_live_operation_request("browser.load", &json!({ "itemId": "instruments/Synth", "expectedName": "Synth" })),
        "required",
    );
    validate_live_operation_request(
        "device.delete",
        &json!({ "ref": "1:device:0:0", "expectedObjectIdentity": "live:device-1", "expectedOwnerRef": "1:track:0", "expectedOwnerIdentity": "live:track-1", "expectedSiblings": [{ "ref": "1:device:0:0", "objectIdentity": "live:device-1" }], "expectedTrackRef": "1:track:0", "expectedTrackIdentity": "live:track-1" }),
    )
    .unwrap();
    assert_throws(validate_live_operation_request("device.delete", &json!({ "ref": "1:device:0:0" })), "required");
    validate_live_operation_request("authority.retire", &json!({ "transactionId": "transaction-123", "terminal": true })).unwrap();
    validate_live_operation_result("authority.retire", &json!({ "retired": 3 })).unwrap();
    assert_throws(validate_live_operation_request("authority.retire", &json!({ "transactionId": "short" })), "shorter");
    assert_throws(validate_live_operation_result("authority.retire", &json!({ "retired": 10_000_001 })), "numeric bounds");
    assert_throws(
        validate_live_operation_result(
            "clip.move",
            &json!({ "ref": "1:clip:0:1", "objectIdentity": "live:clip:1", "name": "Moved", "createdFingerprint": "a".repeat(64), "ownershipToken": "x".repeat(32) }),
        ),
        "not allowed",
    );
}

#[test]
fn transport_actions_fence_on_the_playback_revision_and_explicit_deletions_carry_their_own_authority() {
    // Real Live's playback revision is "<epoch>:playback:<n>:<digest>", as transport.set carries it.
    let set_authority = json!({ "setRef": "4695124654589702:set:song", "expectedObjectIdentity": "live:4695124654589702" });
    validate_live_operation_request(
        "transport.action",
        &with(&set_authority, json!({ "action": "stop", "expectedRevision": "4695124654589702:playback:1:d9860b45721dd3cf" })),
    )
    .unwrap();
    validate_live_operation_request(
        "transport.set",
        &with(&set_authority, json!({ "metronome": true, "expectedRevision": "4695124654589702:playback:1:d9860b45721dd3cf" })),
    )
    .unwrap();
    assert_throws(
        validate_live_operation_request("transport.action", &with(&set_authority, json!({ "action": "stop", "expectedRevision": "" }))),
        "shorter",
    );
    let device = json!({ "ref": "1:device:0:1", "expectedObjectIdentity": "live:device-2", "expectedOwnerRef": "1:track:0", "expectedOwnerIdentity": "live:track-1", "expectedSiblings": [{ "ref": "1:device:0:1", "objectIdentity": "live:device-2" }], "expectedTrackRef": "1:track:0", "expectedTrackIdentity": "live:track-1" });
    validate_live_operation_request("device.delete", &with(&device, json!({ "explicitDeletion": true }))).unwrap();
    assert_throws(validate_live_operation_request("device.delete", &with(&device, json!({ "explicitDeletion": false }))), "constant");
    validate_live_operation_request("track.delete-return", &json!({ "ref": "1:track:3", "expectedObjectIdentity": "live:return-1", "expectedStructureRevision": "a".repeat(64), "explicitDeletion": true })).unwrap();
    // Clips, scenes, tracks and locators the producer asks to delete are explicit deletions too.
    validate_live_operation_request("track.delete", &json!({ "ref": "1:track:3", "expectedObjectIdentity": "live:track-3", "expectedStructureRevision": "a".repeat(64), "explicitDeletion": true })).unwrap();
    validate_live_operation_request("scene.delete", &json!({ "ref": "1:scene:2", "expectedObjectIdentity": "live:scene-2", "expectedStructureRevision": "a".repeat(64), "explicitDeletion": true })).unwrap();
    assert_throws(
        validate_live_operation_request(
            "track.delete",
            &json!({ "ref": "1:track:3", "expectedObjectIdentity": "live:track-3", "expectedStructureRevision": "a".repeat(64), "explicitDeletion": false }),
        ),
        "constant",
    );
}

#[test]
fn runtime_registry_validation_rejects_missing_unknown_and_weak_playback_fields() {
    let playback = playback();
    // `{ ...playback, revision: undefined }` keeps the key with no string in it; JSON spells that `null`.
    assert_throws(validate_live_operation_result("session.playback", &with(&playback, json!({ "revision": null }))), "type");
    assert_throws(validate_live_operation_result("session.playback", &with(&playback, json!({ "extra": true }))), "not allowed");
    let transport = with(&playback["transport"], json!({ "playing": "false" }));
    assert_throws(validate_live_operation_result("session.playback", &with(&playback, json!({ "transport": transport }))), "type");
    let target = with(&playback["playingTargets"][0], json!({ "clipSlotRef": "" }));
    assert_throws(validate_live_operation_result("session.playback", &with(&playback, json!({ "playingTargets": [target] }))), "shorter");
}

#[test]
fn runtime_registry_validation_rejects_noncanonical_discovery_requests_and_results() {
    validate_live_operation_request("discover", &json!({ "kind": "return_track", "parent": "1:set:song", "filters": { "name": "Return" }, "requestedFields": ["name"], "traversalBudget": 10, "limit": 4 })).unwrap();
    validate_live_operation_result(
        "discover",
        &json!({ "epoch": 1, "items": [], "truncated": false, "revision": "1:return_track:0", "kind": "return_track" }),
    )
    .unwrap();
    assert_throws(validate_live_operation_request("discover", &json!({ "kind": "track", "unknown": true })), "not allowed");
    assert_throws(validate_live_operation_request("discover", &json!({ "kind": "track", "filters": { "nested": {} } })), "registry type");
    assert_throws(
        validate_live_operation_request("discover", &json!({ "kind": "track", "filters": { "name": "x".repeat(257) } })),
        "registry maximum",
    );
    assert_throws(
        validate_live_operation_result(
            "discover",
            &json!({ "epoch": 1, "items": [], "truncated": false, "revision": "", "kind": "track" }),
        ),
        "shorter",
    );
}

#[test]
fn realtime_registry_enforces_explicit_unique_channels_and_measured_bounded_results() {
    let target_authorities = json!([{ "ref": "1:parameter:device:0", "parameterIdentity": "live:parameter:0", "ownerRef": "1:device:0", "ownerIdentity": "live:device:0", "trackRef": "1:track:0", "trackIdentity": "live:track:0", "siblings": [{ "ref": "1:parameter:device:0", "objectIdentity": "live:parameter:0" }] }]);
    validate_live_operation_request(
        "realtime.arm",
        &json!({ "ttlMs": 5000, "channels": ["udp-json", "osc", "xy", "max"], "parameterRefs": ["1:parameter:device:0"], "targetAuthorities": target_authorities, "sourcePorts": [41000], "outputSafety": output_safety() }),
    )
    .unwrap();
    assert_throws(
        validate_live_operation_request(
            "realtime.arm",
            &json!({ "channels": [], "parameterRefs": [], "targetAuthorities": [], "outputSafety": output_safety() }),
        ),
        "below registry item bound",
    );
    assert_throws(
        validate_live_operation_request(
            "realtime.arm",
            &json!({ "channels": ["xy", "xy"], "parameterRefs": [], "targetAuthorities": [], "outputSafety": output_safety() }),
        ),
        "duplicate registry items",
    );
    let now = kumi_common::time::now_ms();
    validate_live_operation_result(
        "realtime.arm",
        &json!({ "host": "127.0.0.1", "port": 9766, "token": "t".repeat(32), "expiresAt": now + 5000, "channels": ["xy"], "parameterRefs": ["1:parameter:device:0"], "packetLimitBytes": 512, "ratePerSecond": 64, "burst": 16 }),
    )
    .unwrap();
    validate_live_operation_result(
        "realtime.stats",
        &json!({ "armed": true, "accepted": 2, "applied": 2, "applyFailures": 0, "pending": 0, "droppedUnarmed": 0, "droppedEndpoint": 0, "droppedTarget": 0, "droppedInvalid": 0, "droppedReplay": 0, "droppedRateLimited": 0, "droppedQueueFull": 0, "droppedBeforeDispatch": 0, "revokedBeforeApply": 0, "sequenceGaps": 0, "lastSequence": 2, "jitterMs": 0.2, "maxJitterMs": 0.4 }),
    )
    .unwrap();
}

#[test]
fn capture_registry_requires_exact_bounded_authority_and_cleanup_identity() {
    let base = json!({ "captureId": "capture_1234567890", "setName": "Disposable", "sourceSlotRef": "1:clip_slot:0:0", "destinationSlotRef": "1:clip_slot:1:0", "fence": "a".repeat(64), "maxDurationMs": 5000, "outputSafety": output_safety() });
    validate_live_operation_request("audio.capture.start", &base).unwrap();
    assert_throws(
        validate_live_operation_request("audio.capture.start", &with(&base, json!({ "maxDurationMs": 10001 }))),
        "outside registry numeric bounds",
    );
    assert_throws(validate_live_operation_request("audio.capture.start", &with(&base, json!({ "extra": true }))), "not allowed");
    let now = kumi_common::time::now_ms();
    validate_live_operation_result("audio.capture.start", &json!({ "captureId": base["captureId"], "token": "t".repeat(32), "expiresAt": now + 5000, "state": "active", "sourceSlotRef": base["sourceSlotRef"] })).unwrap();
    validate_live_operation_request(
        "audio.capture.cleanup",
        &json!({ "captureId": base["captureId"], "token": "t".repeat(32), "expectedClipRef": "1:clip:1:0" }),
    )
    .unwrap();
    assert_throws(
        validate_live_operation_request("audio.capture.cleanup", &json!({ "captureId": base["captureId"], "token": "t".repeat(32) })),
        "required",
    );
    validate_live_operation_result(
        "audio.capture.cleanup",
        &json!({ "cleaned": true, "filePath": "/project/Samples/Recorded/capture.wav", "residual": [] }),
    )
    .unwrap();
    validate_live_operation_result("audio.capture.status", &json!({ "active": false, "state": "captured", "captureId": base["captureId"], "clip": { "ref": "1:clip:1:0", "filePath": "/project/capture.wav" } })).unwrap();
}

#[test]
fn guarded_audition_and_emergency_operations_replace_generic_audible_invocation() {
    let registry = load_live_registry();
    let ids: Vec<&str> = registry.operations.iter().map(|item| item.id.as_str()).collect();
    for operation in ["clip.duplicate", "clip.move", "arrangement.clip.move"] {
        let required = registry
            .operation(operation)
            .and_then(|item| item.request.get("required"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(required.iter().any(|item| item == "expectedContentFingerprint"), "{operation}");
    }
    for extension in [
        "project.new",
        "project.open",
        "project.save",
        "project.save-as",
        "project.collect",
        "project.export",
        "project.bounce",
        "arrangement.automation.read",
        "arrangement.automation.create",
        "audio.warp-marker.read",
        "audio.warp-marker.add",
        "audio.take-lane.read",
        "audio.comp.read",
        "browser.preview.start",
        "browser.preview.stop",
    ] {
        assert!(ids.contains(&extension), "{extension}");
    }
    for forbidden in ["set", "clip.launch", "track.stop", "playback.stop-all-clips", "scene.launch", "stop-all-clips", "transport.stop"] {
        assert!(!ids.contains(&forbidden), "{forbidden}");
    }
    let launch = json!({ "ref": "1:scene:0", "setName": "Disposable Set", "sceneName": "Scene 1", "sceneIndex": 0, "playbackRevision": "1:playback:abc", "eligibleTargets": ["1:track:0|1:clip_slot:0:0|1:scene:0"], "expectedSetIdentity": "live:set:1", "expectedAuthorityRevision": "a".repeat(64), "outputSafety": output_safety() });
    validate_live_operation_request("session.audition-launch", &launch).unwrap();
    validate_live_operation_result("session.audition-launch", &json!({ "launched": "1:scene:0", "targets": [{ "trackRef": "1:track:0", "clipSlotRef": "1:clip_slot:0:0", "sceneRef": "1:scene:0", "sceneIndex": 0, "clipRef": "1:clip:0:0" }] })).unwrap();
    assert_throws(validate_live_operation_request("session.audition-launch", &with(&launch, json!({ "eligibleTargets": [42] }))), "type");
    let clip_authority = json!({ "slotRef": "1:clip_slot:0:0", "trackRef": "1:track:0", "sceneRef": "1:scene:0", "sceneIndex": 0, "clipRef": "1:clip:0:0", "trackIdentity": "live:track:0", "sceneIdentity": "live:scene:0", "slotIdentity": "live:slot:0:0", "clipIdentity": "live:clip:0:0", "playbackRevision": "1:playback:abc", "outputSafety": output_safety() });
    validate_live_operation_request("session.clip-launch", &clip_authority).unwrap();
    validate_live_operation_request(
        "session.clip-stop",
        &json!({ "slotRef": clip_authority["slotRef"], "trackRef": clip_authority["trackRef"], "sceneRef": clip_authority["sceneRef"], "sceneIndex": 0, "clipRef": clip_authority["clipRef"], "trackIdentity": clip_authority["trackIdentity"], "sceneIdentity": clip_authority["sceneIdentity"], "slotIdentity": clip_authority["slotIdentity"], "clipIdentity": clip_authority["clipIdentity"] }),
    )
    .unwrap();
    assert_throws(validate_live_operation_request("session.clip-launch", &without(&clip_authority, "trackRef")), "type|required");
    validate_live_operation_result("track.create", &json!({ "ref": "1:track:0", "objectIdentity": "live:track:100", "name": "Created", "kind": "midi", "index": 0, "createdFingerprint": "f".repeat(64) })).unwrap();
    validate_live_operation_request(
        "track.delete",
        &json!({ "ref": "1:track:0", "expectedStructureRevision": "a".repeat(64), "expectedObjectIdentity": "live:track:100" }),
    )
    .unwrap();
    assert_throws(
        validate_live_operation_request("track.delete", &json!({ "ref": "1:track:0", "expectedStructureRevision": "a".repeat(64) })),
        "required",
    );
    validate_live_operation_result("scene.create", &json!({ "ref": "1:scene:0", "objectIdentity": "live:scene:100", "name": "Created", "index": 0, "createdFingerprint": "f".repeat(64) })).unwrap();
    validate_live_operation_request(
        "scene.delete",
        &json!({ "ref": "1:scene:0", "expectedStructureRevision": "a".repeat(64), "expectedObjectIdentity": "live:scene:100" }),
    )
    .unwrap();
    validate_live_operation_request("session.audition-stop", &json!({ "ref": "1:scene:0", "setName": "Disposable Set", "eligibleTargets": [], "expectedSetIdentity": "live:set:1", "expectedAuthorityRevision": "a".repeat(64) })).unwrap();
    validate_live_operation_result("session.audition-stop", &json!({ "stopped": true })).unwrap();
    assert_throws(validate_live_operation_result("session.audition-stop", &json!({ "stopped": false })), "constant");
    validate_live_operation_request("session.emergency-stop", &json!({ "expectedTargets": [], "expectedRecording": "stopped" })).unwrap();
    validate_live_operation_result(
        "session.emergency-stop",
        &json!({ "stopped": true, "stoppedTargets": ["1:track:0|1:clip_slot:0:0|1:scene:0"], "recordingStopped": true }),
    )
    .unwrap();
    assert_throws(validate_live_operation_request("session.emergency-stop", &json!({})), "required");
    let recording_authority = json!({ "action": "start", "expectedSessionRecord": false, "expectedArrangementRecord": false, "destinationTrackRef": "1:track:0", "destinationTrackIdentity": "live:track:0", "outputSafety": { "safe": true, "provenance": "operator-observed" } });
    validate_live_operation_request("recording.session", &recording_authority).unwrap();
    assert_throws(validate_live_operation_request("recording.session", &json!({ "action": "start" })), "required");
}
