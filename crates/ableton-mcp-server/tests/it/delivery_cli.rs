use ableton_mcp_server::{delivery::*, diagnostics, install_remote_script, migrate, setup};
use serde_json::{json, Value};
use std::path::Path;
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}
#[tokio::test(flavor = "current_thread")]
async fn command_parsers_match_source_oracle() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("../support/delivery_cli_oracle.json")).unwrap();
    assert_eq!(rows.len(), 79);
    for row in rows {
        let arguments: Vec<String> = serde_json::from_value(row["args"].clone()).unwrap();
        let output = match row["command"].as_str().unwrap() {
            "setup" => setup::run(&arguments),
            "migrate" => migrate::run(&arguments),
            "diagnostics" => diagnostics::run(&arguments).await,
            _ => install_remote_script::run(&arguments),
        };
        assert_eq!(
            json!({"stdout":output.stdout,"stderr":output.stderr,"code":output.code}),
            json!({"stdout":row["stdout"],"stderr":row["stderr"],"code":row["code"]}),
            "{row}"
        );
    }
}
#[test]
fn setup_creates_native_launch_config_and_migration_preserves_authority() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("package ü spaces");
    std::fs::create_dir(&root).unwrap();
    let output = folder.path().join("server.json");
    let path = output.to_str().unwrap();
    let result = setup::run_with_package_root(&args(&["--output", path]), &root);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let config = read_config(&output).unwrap();
    assert_eq!(config.server.command, native_entrypoint(&root).to_str().unwrap());
    assert!(config.server.args.is_empty());
    assert_eq!(setup::run_with_package_root(&args(&["--output", path]), &root).code, 1);
    let secret = folder.path().join("secret");
    write_secret_file(&secret, None).unwrap();
    let migrated = folder.path().join("migrated.json");
    let result = migrate::run(&args(&[
        "--input",
        path,
        "--output",
        migrated.to_str().unwrap(),
        "--bridge-host",
        "127.0.0.1",
        "--bridge-port",
        "0x2625",
        "--secret-file",
        secret.to_str().unwrap(),
        "--realtime-port",
        "0b10011000100110",
    ]));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let config = read_any_config(&migrated).unwrap();
    assert_eq!(config.bridge().unwrap().port, 9765.);
    assert_eq!(config.bridge().unwrap().realtime_port, Some(9766.));
    assert_eq!(config.server().args, vec!["--config", migrated.to_str().unwrap()]);
    let result = setup::run_with_package_root(
        &args(&[
            "--output",
            path,
            "--force",
            "--bridge-port",
            "9765",
            "--secret-file",
            secret.to_str().unwrap(),
            "--realtime-port",
            "9766",
        ]),
        &root,
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(read_any_config(&output).unwrap().bridge().unwrap().secret_file, secret);
}
#[test]
fn installer_creates_assets_only_after_explicit_apply_and_preserves_force_behavior() {
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("Remote ü Script");
    let root = folder.path().join("package");
    let asset = root.join("remote-script/AbletonMcpBridge");
    std::fs::create_dir_all(&asset).unwrap();
    let source = kumi_testkit::workspace_path("remote-script");
    std::fs::copy(source.join(REMOTE_SCRIPT_ASSET), asset.join(REMOTE_SCRIPT_ASSET)).unwrap();
    std::fs::copy(source.join("AbletonMcpBridge/__init__.py"), asset.join("__init__.py")).unwrap();
    let result = install_remote_script::run_with_package_root(&args(&["--destination", destination.to_str().unwrap(), "--dry-run"]), &root);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert!(!destination.exists());
    let result = install_remote_script::run_with_package_root(&args(&["--destination", destination.to_str().unwrap()]), &root);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert!(destination.join(REMOTE_SCRIPT_ASSET).is_file());
    assert_eq!(install_remote_script::run_with_package_root(&args(&["--destination", destination.to_str().unwrap()]), &root).code, 1);
    let result = install_remote_script::run_with_package_root(&args(&["--destination", destination.to_str().unwrap(), "--force"]), &root);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let result: Value = serde_json::from_str(&result.stdout).unwrap();
    assert!(Path::new(result["backup"].as_str().unwrap()).exists());
}
#[tokio::test(flavor = "current_thread")]
async fn diagnostics_command_emits_native_runtime_evidence_without_claiming_live() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let output = diagnostics::run_with_package_root(&[], Some(folder.path())).await;
            assert_eq!(output.code, 0);
            let report: Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(report["runtime"], "rust-native");
            assert_eq!(report["runtimeSupported"], true);
            assert_eq!(report["liveConnected"], false);
            assert_eq!(report["entrypoint"]["present"], false);
        })
        .await;
}
