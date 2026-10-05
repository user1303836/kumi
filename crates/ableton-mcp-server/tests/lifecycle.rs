#[path = "support/lifecycle_fixture.rs"]
mod fixture;
use ableton_mcp_server::{delivery::*, lifecycle::*, lifecycle_cli};
use fixture::*;
use serde_json::{json, Value};
use std::{fs, path::Path};
fn message(error: ableton_mcp_server::live::LiveError, expected: &str) {
    assert!(error.message().contains(expected), "expected {expected}: {}", error.message());
}
#[tokio::test(flavor = "current_thread")]
async fn plan_is_read_only_and_native_artifact_identity_is_required() {
    let f = Fixture::new();
    let mut options = f.options.clone();
    options.apply = false;
    options.confirm_live_stopped = false;
    let result = run_lifecycle(&options).await.unwrap();
    assert_eq!(result["state"], "planned");
    assert_eq!(result["verification"]["portsRequireApplyTimeProbe"], true);
    assert!(!options.state_directory.exists());
    options.apply = true;
    message(run_lifecycle(&options).await.unwrap_err(), "confirm-live-stopped");
    assert!(!f.remote().exists());
    options.apply = false;
    options.artifact_sha256 = Some("f".repeat(64));
    message(run_lifecycle(&options).await.unwrap_err(), "exact tarball bytes");
    options.artifact_path = None;
    message(run_lifecycle(&options).await.unwrap_err(), "exact local npm tarball");
}
#[tokio::test(flavor = "current_thread")]
async fn native_install_status_activation_and_idempotent_repair_report_truthfully() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new();
            let result = run_lifecycle(&f.options).await.unwrap();
            assert_eq!(result["state"], "completed");
            assert_eq!(result["verification"]["remoteFiles"], 6);
            let receipt = f.receipt();
            assert_eq!(receipt["generation"], 1);
            assert_eq!(receipt["config"]["server"]["command"], json!(native_entrypoint(&f.options.package_root)));
            assert_eq!(receipt["config"]["server"]["args"], json!(["--config", f.options.state_directory.join("bridge-config.json")]));
            let worker = std::path::Path::new(receipt["config"]["server"]["command"].as_str().unwrap()).with_file_name(if cfg!(windows) {
                "ableton-mcp-analysis-worker.exe"
            } else {
                "ableton-mcp-analysis-worker"
            });
            assert_eq!(fs::read_to_string(worker).unwrap(), "fixture worker payload 1.0.0\n");
            assert!(f.remote().join("__pycache__").is_file());
            assert_eq!(secret_permissions(&f.receipt_path()), SecretPermissions::OwnerOnly);
            let status = run_lifecycle(&f.action("status")).await.unwrap();
            assert_eq!(status["verification"]["installationIntegrityValid"], true);
            let activation = run_lifecycle(&f.action("activate")).await.unwrap();
            assert_eq!(activation["state"], "activation-required");
            assert_eq!(activation["verification"]["liveConnected"], false);
            assert_eq!(f.receipt()["activation"]["realLiveVerified"], false);
            let repair = run_lifecycle(&f.action("repair")).await.unwrap();
            assert_eq!(repair["verification"]["changed"], false);
            assert_eq!(repair["steps"][1]["status"], "skipped");
            assert!(!f.options.state_directory.join("lifecycle.lock").exists());
        })
        .await;
}
#[tokio::test(flavor = "current_thread")]
async fn occupied_ports_are_rejected_before_state_creation() {
    let f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut options = f.options.clone();
    options.port = Some(listener.local_addr().unwrap().port() as f64);
    message(run_lifecycle(&options).await.unwrap_err(), "is occupied");
    assert!(!options.state_directory.exists());
    options.port = Some(1.);
    options.realtime_port = Some(1.);
    message(run_lifecycle(&options).await.unwrap_err(), "distinct values");
    options.host = Some("localhost".into());
    message(run_lifecycle(&options).await.unwrap_err(), "exact loopback");
}
#[tokio::test(flavor = "current_thread")]
async fn every_install_fault_removes_only_new_authority_and_records_compensation() {
    for point in ["after-secret", "after-config", "after-remote", "before-receipt"] {
        let f = Fixture::new();
        let mut options = f.options.clone();
        options.fault_at = Some(point.into());
        options.enable_bridge_diagnostics = true;
        message(run_lifecycle(&options).await.unwrap_err(), "injected lifecycle failure");
        assert!(!f.remote().exists(), "{point}");
        assert!(!f.receipt_path().exists());
        for name in ["bridge.secret", "bridge-config.json", "bridge-diagnostics.log", "lifecycle.lock"] {
            assert!(!options.state_directory.join(name).exists(), "{point}: {name}");
        }
        assert_eq!(read(options.state_directory.join("lifecycle-journal.json"))["state"], "failed-rolled-back");
    }
    let f = Fixture::new();
    fs::create_dir(&f.options.state_directory).unwrap();
    let secret = f.options.state_directory.join("bridge.secret");
    write_secret_file(&secret, None).unwrap();
    let bytes = fs::read(&secret).unwrap();
    let mut options = f.options.clone();
    options.fault_at = Some("after-secret".into());
    assert!(run_lifecycle(&options).await.is_err());
    assert_eq!(fs::read(secret).unwrap(), bytes);
}
#[tokio::test(flavor = "current_thread")]
async fn diagnostics_is_opt_in_and_repair_preserves_drift() {
    let f = Fixture::new();
    let mut options = f.options.clone();
    options.enable_bridge_diagnostics = true;
    run_lifecycle(&options).await.unwrap();
    let diagnostics = options.state_directory.join("bridge-diagnostics.log");
    assert!(diagnostics.is_file());
    assert_eq!(secret_permissions(&diagnostics), SecretPermissions::OwnerOnly);
    fs::write(f.remote().join("user.py"), "custom").unwrap();
    fs::write(&diagnostics, "do not lose").unwrap();
    chmod(&diagnostics, 0o644);
    let status = run_lifecycle(&f.action("status")).await.unwrap();
    assert_eq!(status["verification"]["installationIntegrityValid"], false);
    let mut repair = f.action("repair");
    repair.confirm_live_stopped = false;
    let result = run_lifecycle(&repair).await.unwrap();
    assert_eq!(result["verification"]["changed"], true);
    assert!(!f.remote().join("user.py").exists());
    assert_eq!(fs::read_to_string(Path::new(result["recovery"]["quarantine"].as_str().unwrap()).join("user.py")).unwrap(), "custom");
    #[cfg(unix)]
    {
        assert_eq!(result["verification"]["diagnosticsRepaired"], true);
        assert_eq!(fs::read_to_string(result["verification"]["diagnosticsQuarantine"].as_str().unwrap()).unwrap(), "do not lose");
    }
    let mut invalid = f.action("repair");
    invalid.enable_bridge_diagnostics = true;
    message(run_lifecycle(&invalid).await.unwrap_err(), "only during lifecycle install");
}
#[tokio::test(flavor = "current_thread")]
async fn repair_compensation_restores_original_drift_and_legacy_cache_blocker() {
    let f = Fixture::new();
    run_lifecycle(&f.options).await.unwrap();
    fs::write(f.remote().join("user.py"), "custom").unwrap();
    let config = f.options.state_directory.join("bridge-config.json");
    fs::write(&config, "custom config").unwrap();
    let mut repair = f.action("repair");
    repair.fault_at = Some("after-repair-install".into());
    message(run_lifecycle(&repair).await.unwrap_err(), "injected lifecycle failure");
    assert_eq!(fs::read_to_string(f.remote().join("user.py")).unwrap(), "custom");
    assert_eq!(fs::read_to_string(&config).unwrap(), "custom config");
    assert_eq!(f.receipt()["generation"], 1);
    repair.fault_at = None;
    run_lifecycle(&repair).await.unwrap();
    let blocker = f.remote().join("__pycache__");
    fs::remove_file(&blocker).unwrap();
    fs::create_dir(&blocker).unwrap();
    fs::write(blocker.join("cached.pyc"), b"cached").unwrap();
    let mut r = f.receipt();
    r["remoteFiles"].as_object_mut().unwrap().remove("__pycache__");
    r["remoteFiles"]["__pycache__/cached.pyc"] = json!(sha("cached"));
    write(f.receipt_path(), &r);
    let result = run_lifecycle(&repair).await.unwrap();
    assert_eq!(result["verification"]["changed"], true);
    assert!(blocker.is_file());
    assert_eq!(fs::metadata(blocker).unwrap().len(), 0);
}
#[tokio::test(flavor = "current_thread")]
async fn the_willington_switch_is_not_drift_and_upgrades_keep_it_owner_only() {
    let f = Fixture::new();
    run_lifecycle(&f.options).await.unwrap();
    // /willington writes the switch after an install, and removes it again: neither is drift.
    let switch = f.remote().join(WILLINGTON_CONFIG);
    write_owner_file(&switch, br#"{"version":1,"followActions":true,"deviceTools":true,"rackZones":true,"enableWrites":true}"#).unwrap();
    let bytes = fs::read(&switch).unwrap();
    assert_eq!(run_lifecycle(&f.action("status")).await.unwrap()["verification"]["installationIntegrityValid"], true);
    run_lifecycle(&f.upgrade("1.1.0")).await.unwrap();
    assert_eq!(fs::read(&switch).unwrap(), bytes);
    assert_eq!(secret_permissions(&switch), SecretPermissions::OwnerOnly);
    fs::remove_file(&switch).unwrap();
    assert_eq!(run_lifecycle(&f.action("status")).await.unwrap()["verification"]["installationIntegrityValid"], true);
}
#[tokio::test(flavor = "current_thread")]
async fn upgrades_retain_generation_and_rollback_compensates_configuration_exactly() {
    let f = Fixture::new();
    run_lifecycle(&f.options).await.unwrap();
    let first = f.receipt();
    let upgrade = f.upgrade("1.1.0");
    run_lifecycle(&upgrade).await.unwrap();
    let second = f.receipt();
    assert_eq!(second["generation"], 2);
    assert_eq!(second["previous"]["artifactSha256"], first["artifactSha256"]);
    assert!(Path::new(second["previous"]["remoteBackup"].as_str().unwrap()).exists());
    let config = fs::read(f.options.state_directory.join("bridge-config.json")).unwrap();
    let mut rollback = f.action("rollback");
    rollback.fault_at = Some("after-rollback-config".into());
    message(run_lifecycle(&rollback).await.unwrap_err(), "injected lifecycle failure");
    assert_eq!(f.receipt(), second);
    assert_eq!(fs::read(f.options.state_directory.join("bridge-config.json")).unwrap(), config);
    assert!(Path::new(second["previous"]["remoteBackup"].as_str().unwrap()).exists());
    rollback.fault_at = None;
    let result = run_lifecycle(&rollback).await.unwrap();
    assert_eq!(result["verification"]["artifactSha256"], first["artifactSha256"]);
    assert_eq!(f.receipt()["previous"]["artifactSha256"], second["artifactSha256"]);
    assert_eq!(f.receipt()["generation"], 3);
}
#[tokio::test(flavor = "current_thread")]
async fn upgrade_failures_restore_original_config_and_remote_generation() {
    for point in ["before-remote", "after-remote", "before-receipt"] {
        let f = Fixture::new();
        run_lifecycle(&f.options).await.unwrap();
        let receipt = f.receipt();
        let config = fs::read(f.options.state_directory.join("bridge-config.json")).unwrap();
        let remote = fs::read(f.remote().join(REMOTE_SCRIPT_ASSET)).unwrap();
        let mut upgrade = f.upgrade("1.1.0");
        upgrade.fault_at = Some(point.into());
        message(run_lifecycle(&upgrade).await.unwrap_err(), "injected lifecycle failure");
        assert_eq!(f.receipt(), receipt);
        assert_eq!(fs::read(f.options.state_directory.join("bridge-config.json")).unwrap(), config);
        assert_eq!(fs::read(f.remote().join(REMOTE_SCRIPT_ASSET)).unwrap(), remote);
        assert_eq!(read(f.options.state_directory.join("lifecycle-journal.json"))["state"], "failed-rolled-back");
    }
}
#[tokio::test(flavor = "current_thread")]
async fn legacy_private_and_retired_node_generations_are_receipt_bound_only() {
    for policy in ["legacy", "node25", "current"] {
        let f = Fixture::new();
        run_lifecycle(&f.options).await.unwrap();
        f.legacy_receipt(policy);
        let legacy = f.receipt();
        let mut fresh = f.options.clone();
        fresh.apply = false;
        fresh.package_root = legacy["packageRoot"].as_str().unwrap().into();
        let (artifact, hash) = bind(&fresh.package_root);
        fresh.artifact_path = Some(artifact);
        fresh.artifact_sha256 = Some(hash);
        message(run_lifecycle(&fresh).await.unwrap_err(), "receipt-bound compatibility only");
        assert_eq!(run_lifecycle(&f.action("status")).await.unwrap()["verification"]["packageValid"], true);
        let mut repair = f.action("repair");
        repair.package_root = fresh.package_root;
        assert_eq!(run_lifecycle(&repair).await.unwrap()["verification"]["changed"], false);
        let upgrade = f.upgrade("1.1.0");
        run_lifecycle(&upgrade).await.unwrap();
        let restored = run_lifecycle(&f.action("rollback")).await.unwrap();
        assert_eq!(restored["verification"]["artifactSha256"], legacy["artifactSha256"]);
        assert_eq!(f.receipt()["config"], legacy["config"]);
    }
}
#[tokio::test(flavor = "current_thread")]
async fn uninstall_cleans_retired_generations_preserves_drift_and_respects_secret_ownership() {
    let f = Fixture::new();
    run_lifecycle(&f.options).await.unwrap();
    run_lifecycle(&f.upgrade("1.1.0")).await.unwrap();
    let retired = f.receipt()["previous"]["remoteBackup"].clone();
    let mut upgrade = f.upgrade("1.2.0");
    upgrade.fault_at = Some("retired-cleanup-blocked".into());
    let result = run_lifecycle(&upgrade).await.unwrap();
    assert_eq!(result["verification"]["retiredPreviousBackupRemoved"], false);
    assert_eq!(f.receipt()["retained"]["pendingCleanup"], json!([retired]));
    fs::write(f.remote().join("keep.py"), "custom").unwrap();
    let result = run_lifecycle(&f.action("uninstall")).await.unwrap();
    assert!(!f.remote().exists());
    assert!(!Path::new(retired.as_str().unwrap()).exists());
    assert_eq!(result["verification"]["secretPreserved"], true);
    assert_eq!(fs::read_to_string(Path::new(result["recovery"]["quarantine"].as_str().unwrap()).join("keep.py")).unwrap(), "custom");
    let mut uninstall = f.action("uninstall");
    uninstall.purge_secret = true;
    let result = run_lifecycle(&uninstall).await.unwrap();
    assert_eq!(result["verification"]["alreadyUninstalled"], true);
    assert!(!f.options.state_directory.join("bridge.secret").exists());
    message(run_lifecycle(&uninstall).await.unwrap_err(), "not created by this lifecycle");
    let f = Fixture::new();
    fs::create_dir(&f.options.state_directory).unwrap();
    write_secret_file(&f.options.state_directory.join("bridge.secret"), None).unwrap();
    run_lifecycle(&f.options).await.unwrap();
    let mut uninstall = f.action("uninstall");
    uninstall.purge_secret = true;
    message(run_lifecycle(&uninstall).await.unwrap_err(), "not created by this lifecycle");
    assert!(f.remote().exists());
}
#[tokio::test(flavor = "current_thread")]
async fn stale_dead_locks_are_reclaimed_but_live_or_malformed_locks_remain() {
    let f = Fixture::new();
    run_lifecycle(&f.options).await.unwrap();
    let path = f.options.state_directory.join("lifecycle.lock");
    write(&path, &json!({"pid":std::process::id()}));
    message(run_lifecycle(&f.action("repair")).await.unwrap_err(), "another lifecycle operation");
    assert!(path.exists());
    for value in [json!({"pid":0}), json!({"pid":1.5}), json!({"pid":"999999"})] {
        write(&path, &value);
        message(run_lifecycle(&f.action("repair")).await.unwrap_err(), "another lifecycle operation");
        assert!(path.exists());
    }
    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "exit", "0"]).spawn().unwrap()
    } else {
        std::process::Command::new("/usr/bin/true").spawn().unwrap()
    };
    let pid = child.id();
    child.wait().unwrap();
    write(&path, &json!({"pid":pid}));
    assert_eq!(run_lifecycle(&f.action("repair")).await.unwrap()["state"], "completed");
    assert!(!path.exists());
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn linked_ancestors_receipts_and_diagnostics_hardlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let link = f.folder.path().join("linked");
    symlink(&f.options.remote_scripts_directory, &link).unwrap();
    let mut options = f.options.clone();
    options.remote_scripts_directory = link;
    message(run_lifecycle(&options).await.unwrap_err(), "symbolic-link or junction ancestor");
    let mut options = f.options.clone();
    options.enable_bridge_diagnostics = true;
    run_lifecycle(&options).await.unwrap();
    let receipt = f.receipt_path();
    let saved = receipt.with_extension("saved");
    fs::rename(&receipt, &saved).unwrap();
    symlink(&saved, &receipt).unwrap();
    message(run_lifecycle(&f.action("status")).await.unwrap_err(), "symbolic-link or junction ancestor");
    fs::remove_file(&receipt).unwrap();
    fs::rename(saved, &receipt).unwrap();
    let diagnostics = options.state_directory.join("bridge-diagnostics.log");
    fs::hard_link(&diagnostics, options.state_directory.join("outside-link")).unwrap();
    assert_eq!(run_lifecycle(&f.action("status")).await.unwrap()["verification"]["bridgeDiagnostics"]["valid"], false);
    run_lifecycle(&f.action("repair")).await.unwrap();
    assert_eq!(run_lifecycle(&f.action("status")).await.unwrap()["verification"]["bridgeDiagnostics"]["valid"], true);
}
#[tokio::test(flavor = "current_thread")]
async fn cli_parses_integer_bounds_duplicates_and_redacts_paths() {
    for (arguments, reason) in [
        (vec!["install", "--remote-scripts-dir", "/fixture", "--port", "-1"], "requires an integer"),
        (vec!["install", "--remote-scripts-dir", "/fixture", "--port", "9007199254740992"], "outside the safe integer range"),
        (vec!["install", "--remote-scripts-dir", "/fixture", "--apply", "--apply"], "duplicate option"),
        (vec!["install", "--remote-scripts-dir", "--apply"], "requires a value"),
        (vec!["install", "--remote-scripts-dir", "/fixture", "--bad"], "unknown option"),
    ] {
        let args = arguments.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        let out = lifecycle_cli::run(&args).await;
        assert_eq!(out.code, 2);
        let output: Value = serde_json::from_str(&out.stderr).unwrap();
        assert!(output["reason"].as_str().unwrap().contains(reason), "{out:?}");
        assert!(!out.stderr.contains("/fixture"));
    }
    let f = Fixture::new();
    let args = vec![
        "status".into(),
        "--remote-scripts-dir".into(),
        f.options.remote_scripts_directory.to_string_lossy().into(),
        "--package-root".into(),
        f.options.package_root.to_string_lossy().into(),
        "--state-dir".into(),
        f.options.state_directory.to_string_lossy().into(),
    ];
    let output = lifecycle_cli::run(&args).await;
    assert_eq!(output.code, 0);
    assert_eq!(serde_json::from_str::<Value>(&output.stdout).unwrap()["verification"]["receipt"], Value::Null);
}
#[tokio::test(flavor = "current_thread")]
async fn lifecycle_results_match_normalized_typescript_sequence_oracle() {
    let f = Fixture::new();
    let mut rows = Vec::new();
    let mut plan = f.options.clone();
    plan.apply = false;
    plan.confirm_live_stopped = false;
    rows.push(json!({"label":"plan","result":run_lifecycle(&plan).await.unwrap()}));
    rows.push(json!({"label":"install","result":run_lifecycle(&f.options).await.unwrap()}));
    rows.push(json!({"label":"status","result":run_lifecycle(&f.action("status")).await.unwrap()}));
    rows.push(json!({"label":"repair-noop","result":run_lifecycle(&f.action("repair")).await.unwrap()}));
    rows.push(json!({"label":"upgrade","result":run_lifecycle(&f.upgrade("1.1.0")).await.unwrap()}));
    rows.push(json!({"label":"rollback","result":run_lifecycle(&f.action("rollback")).await.unwrap()}));
    rows.push(json!({"label":"uninstall","result":run_lifecycle(&f.action("uninstall")).await.unwrap()}));
    rows.push(json!({"label":"uninstall-again","result":run_lifecycle(&f.action("uninstall")).await.unwrap()}));
    fn normalize(value: Value, root: &str) -> Value {
        match value {
            Value::String(value) => {
                let value = value.replace(root, "<root>").replace('\\', "/");
                let value = if value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()) {
                    String::from("<sha256>")
                } else if value.len() == 40 && value.bytes().all(|c| c.is_ascii_hexdigit()) {
                    String::from("<commit>")
                } else {
                    value
                };
                let regex = regex::Regex::new(r"-\d+-\d+-[a-f0-9]+$").unwrap();
                json!(regex.replace_all(&value, "-<id>"))
            }
            Value::Array(values) => json!(values.into_iter().map(|v| normalize(v, root)).collect::<Vec<_>>()),
            Value::Object(values) => Value::Object(values.into_iter().map(|(k, v)| (k, normalize(v, root))).collect()),
            value => value,
        }
    }
    let actual = normalize(json!(rows), f.folder.path().to_str().unwrap());
    let expected: Value = serde_json::from_str(include_str!("support/lifecycle_oracle.json")).unwrap();
    assert_eq!(actual, expected);
}
