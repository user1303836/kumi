use ableton_mcp_server::lifecycle::delivery::*;
use serde_json::json;
use std::path::Path;
#[test]
fn native_and_legacy_versioned_configs_round_trip_and_require_force() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("config.json");
    let native = config_for_entrypoint(Path::new("/opt/ableton-mcp-server"), None).unwrap();
    assert_eq!(native.server.command, "/opt/ableton-mcp-server");
    assert!(native.server.args.is_empty());
    write_config(&path, &native, false).unwrap();
    assert_eq!(read_config(&path).unwrap(), native);
    assert!(write_config(&path, &native, false).unwrap_err().message().contains("refusing to overwrite"));
    let legacy = config_for_entrypoint(Path::new("/opt/server.js"), Some("/usr/bin/node")).unwrap();
    write_config(&path, &legacy, true).unwrap();
    assert_eq!(read_config(&path).unwrap(), legacy);
    assert!(folder.path().read_dir().unwrap().all(|entry| entry.unwrap().file_name() == "config.json"));
    for value in
        [json!({"version":1,"server":{"command":"","args":[]}}), json!({"version":1,"server":{"command":"node","args":[],"extra":true}})]
    {
        assert_eq!(write_config(&folder.path().join("invalid.json"), &value, false).unwrap_err().message(), "invalid server configuration");
    }
    let directory = folder.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    assert!(write_config(&directory, &native, true).unwrap_err().message().contains("configuration directory"));
    assert!(directory.is_dir());
}
#[cfg(unix)]
#[test]
fn config_and_secret_symlinks_are_rejected_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let folder = tempfile::tempdir().unwrap();
    let target = folder.path().join("target");
    std::fs::write(&target, "sentinel").unwrap();
    let config = config_for_entrypoint(Path::new("/opt/ableton-mcp-server"), None).unwrap();
    for (destination, source) in
        [(folder.path().join("link"), target.clone()), (folder.path().join("dangling"), folder.path().join("missing"))]
    {
        symlink(&source, &destination).unwrap();
        assert!(write_config(&destination, &config, true).unwrap_err().message().contains("symbolic link"));
        assert!(read_secret_file(&destination).unwrap_err().message().contains("symbolic link"));
    }
    assert_eq!(std::fs::read_to_string(target).unwrap(), "sentinel");
    assert!(!folder.path().join("missing").exists());
}
#[test]
fn bridge_config_preserves_legacy_and_emits_native_launch_shape() {
    let folder = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    secure_windows_directory(folder.path()).unwrap();
    let secret = folder.path().join("secret");
    write_secret_file(&secret, None).unwrap();
    assert!(read_secret_file(&secret).unwrap().len() >= 32);
    let path = folder.path().join("bridge-config.json");
    let diagnostics = folder.path().join("diagnostics.log");
    write_secret_file(&diagnostics, Some(&"d".repeat(32))).unwrap();
    let bridge = json!({"host":"127.0.0.1","port":43210,"secretFile":secret,"timeoutMs":5000,"realtimePort":43211,"diagnostics":{"path":diagnostics,"maxBytes":BRIDGE_DIAGNOSTICS_MAX_BYTES}});
    let native = config_for_bridge(Path::new("/opt/ableton-mcp-server"), &bridge, None, Some(&path), true).unwrap();
    assert_eq!(native.server.args, vec!["--config", path.to_str().unwrap()]);
    assert_eq!(native.server.command, "/opt/ableton-mcp-server");
    write_config(&path, &native, false).unwrap();
    assert_eq!(read_any_config(&path).unwrap(), AnyConfig::Bridge(native.clone()));
    let legacy = config_for_bridge(Path::new("/opt/server.js"), &bridge, Some("/usr/bin/node"), Some(&path), true).unwrap();
    assert_eq!(legacy.server.args, vec!["/opt/server.js", "--config", path.to_str().unwrap()]);
    write_config(&path, &legacy, true).unwrap();
    assert_eq!(read_any_config(&path).unwrap(), AnyConfig::Bridge(legacy));
    std::fs::rename(&diagnostics, diagnostics.with_extension("missing")).unwrap();
    assert!(read_any_config(&path).is_ok());
    let mut invalid = bridge.clone();
    invalid["inlineSecret"] = "forbidden".into();
    assert_eq!(
        config_for_bridge(Path::new("/opt/server"), &invalid, None, None, false).unwrap_err().message(),
        "unsupported bridge configuration fields"
    );
    for host in ["localhost", "0.0.0.0", "127.0.0.2", "127.999.0.1"] {
        let mut invalid = bridge.clone();
        invalid["host"] = host.into();
        assert!(config_for_bridge(Path::new("/opt/server"), &invalid, None, None, false).unwrap_err().message().contains("exact loopback"));
    }
    let mut invalid = bridge.clone();
    invalid["realtimePort"] = 43210.into();
    assert!(config_for_bridge(Path::new("/opt/server"), &invalid, None, None, false).unwrap_err().message().contains("realtime port"));
}
#[test]
fn secrets_accept_one_line_ending_reject_whitespace_and_invalid_sizes() {
    let folder = tempfile::tempdir().unwrap();
    for size in [0., 8., 31., 129., 32.5, f64::NAN] {
        assert!(generate_secret(Some(size)).is_err());
    }
    for size in [32., 128.] {
        assert!(generate_secret(Some(size)).unwrap().len() >= size as usize);
    }
    let path = folder.path().join("secret");
    write_secret_file(&path, Some(&"a".repeat(32))).unwrap();
    assert_eq!(secret_permissions(&path), SecretPermissions::OwnerOnly);
    assert!(write_secret_file(&path, None).is_err());
    for raw in [format!("{}\r\n", "a".repeat(32)), "a".repeat(32)] {
        std::fs::write(&path, raw).unwrap();
        assert_eq!(read_secret_file(&path).unwrap(), "a".repeat(32));
    }
    for raw in [format!(" {}\n", "a".repeat(32)), format!("{}\n\n", "a".repeat(32)), format!("{}\u{feff}", "a".repeat(32))] {
        std::fs::write(&path, raw).unwrap();
        assert_eq!(read_secret_file(&path).unwrap_err().message(), "secret file is invalid");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&path, "a".repeat(32)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_secret_file(&path).unwrap_err().message().contains("conclusively owner-only"));
    }
}
#[test]
fn migrations_preserve_existing_configs_and_explicitly_add_bridge_authority() {
    let folder = tempfile::tempdir().unwrap();
    let input = folder.path().join("legacy.json");
    let output = folder.path().join("v1.json");
    std::fs::write(&input, r#"{"command":"/usr/bin/node","args":["server.js"]}"#).unwrap();
    assert_eq!(
        serde_json::to_value(migrate_config(&input, &output, false, None).unwrap()).unwrap(),
        json!({"version":1,"server":{"command":"/usr/bin/node","args":["server.js"]}})
    );
    let secret = folder.path().join("secret");
    write_secret_file(&secret, None).unwrap();
    let bridge = json!({"host":"127.0.0.1","port":9765,"secretFile":secret,"timeoutMs":5000});
    let native = config_for_entrypoint(Path::new("/opt/ableton-mcp-server"), None).unwrap();
    write_config(&input, &native, true).unwrap();
    let converted = migrate_config(&input, &output, true, Some(&bridge)).unwrap();
    assert_eq!(converted.server().args, vec!["--config", output.to_str().unwrap()]);
    assert_eq!(read_any_config(&output).unwrap(), converted);
}
#[test]
fn bridge_reference_names_only_its_absolute_config() {
    let folder = tempfile::tempdir().unwrap();
    let config = folder.path().join("config.json");
    std::fs::write(&config, "{}").unwrap();
    let reference = folder.path().join("bridge-reference.json");
    write_bridge_reference(&reference, &config, false).unwrap();
    assert_eq!(std::fs::read_to_string(&reference).unwrap(), format!("{}\n", json!({"config":config})));
    assert!(write_bridge_reference(&reference, &config, false).is_err());
    write_bridge_reference(&reference, &config, true).unwrap();
    assert!(write_bridge_reference(Path::new("relative"), &config, true).is_err());
}
#[test]
fn retained_node_policy_matches_the_javascript_bridge_and_platforms() {
    // What the last JavaScript bridge (1.0.74) declared: configurations it wrote still name Node.
    assert_eq!(NODE_ENGINE_RANGE, ">=22 <23 || >=24 <25");
    assert_eq!(json!(SUPPORTED_NODE_MAJORS), json!([22, 24]));
    for major in 21..=27 {
        assert_eq!(supported_node_major(&format!("{major}.0.0")), [22, 24].contains(&major));
    }
    assert!(!supported_node_major("22.0.0-rc.1"));
    for platform in ["darwin", "linux", "win32"] {
        assert!(is_supported_platform(Some(platform)));
    }
    assert!(!is_supported_platform(Some("freebsd")));
}
#[test]
fn legacy_bridge_validation_matches_typescript_oracle() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(&kumi_testkit::oracle("ableton-mcp-server/delivery-oracle.json")).unwrap();
    for case in &cases {
        let result = config_for_bridge(
            Path::new(case["entrypoint"].as_str().unwrap()),
            &case["bridge"],
            Some(case["command"].as_str().unwrap()),
            Some(Path::new(case["configPath"].as_str().unwrap())),
            false,
        );
        if let Some(error) = case.get("error") {
            assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{}", case["name"]);
        } else {
            let actual = kumi_common::js::json::stringify(&serde_json::to_value(result.unwrap()).unwrap());
            let expected = kumi_common::js::json::stringify(&case["result"]);
            assert_eq!(actual, expected, "{}", case["name"]);
        }
    }
    assert!(cases.len() > 100);
}

fn install_source(root: &Path) -> std::path::PathBuf {
    let folder = root.join("source");
    std::fs::create_dir_all(folder.join(REMOTE_SCRIPT_PACKAGE)).unwrap();
    std::fs::write(folder.join(REMOTE_SCRIPT_PACKAGE).join("__init__.py"), "# production package\n").unwrap();
    let source = folder.join("source.py");
    std::fs::write(&source, "production-remote-script").unwrap();
    source
}
#[test]
fn script_install_is_atomic_and_keeps_backup_configuration_and_cache_blocker() {
    use sha2::{Digest, Sha256};
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    let preview = install_remote_script(&source, &destination, &InstallOptions { dry_run: true, ..Default::default() }).unwrap();
    assert_eq!(preview, InstallResult { installed: destination.clone(), backup: None, reference: None, dry_run: true });
    assert!(!destination.exists());
    let config = folder.path().join("config.json");
    std::fs::write(&config, "{}").unwrap();
    let first =
        install_remote_script(&source, &destination, &InstallOptions { config_path: Some(config.clone()), ..Default::default() }).unwrap();
    assert!(first.backup.is_none());
    assert_eq!(std::fs::read_to_string(first.reference.unwrap()).unwrap(), format!("{}\n", json!({"config":config})));
    assert_eq!(std::fs::read_to_string(destination.join(REMOTE_SCRIPT_ASSET)).unwrap(), "production-remote-script");
    assert!(install_remote_script(&source, &destination, &InstallOptions::default())
        .unwrap_err()
        .message()
        .contains("refusing to overwrite"));
    let willington = destination.join("willington.json");
    std::fs::write(&willington, r#"{"version":1,"followActions":true,"enableWrites":false}"#).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&willington, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&source, "replacement").unwrap();
    let second = install_remote_script(&source, &destination, &InstallOptions { force: true, ..Default::default() }).unwrap();
    let backup = second.backup.unwrap();
    assert_eq!(std::fs::read_to_string(backup.join(REMOTE_SCRIPT_ASSET)).unwrap(), "production-remote-script");
    assert_eq!(std::fs::read_to_string(destination.join(REMOTE_SCRIPT_ASSET)).unwrap(), "replacement");
    assert_eq!(std::fs::read(&willington).unwrap(), std::fs::read(backup.join("willington.json")).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&willington).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(destination.join(REMOTE_SCRIPT_ASSET)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let blocker = destination.join("__pycache__");
    assert!(blocker.is_file());
    assert_eq!(std::fs::metadata(&blocker).unwrap().len(), 0);
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["registryHash"], ableton_mcp_server::protocol::registry::live_registry_hash());
    for name in ["__init__.py", REMOTE_SCRIPT_ASSET, OPERATION_REGISTRY_ASSET] {
        assert_eq!(manifest["files"][name], hex::encode(Sha256::digest(std::fs::read(destination.join(name)).unwrap())));
    }
    let python = if cfg!(windows) { "python.exe" } else { "python3" };
    let imported =
        std::process::Command::new(python).args(["-c", "import AbletonMcpBridge"]).env("PYTHONPATH", folder.path()).output().unwrap();
    assert!(imported.status.success(), "{}", String::from_utf8_lossy(&imported.stderr));
    assert!(blocker.is_file());
    assert!(folder.path().read_dir().unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".ableton-mcp-install")));
}
#[test]
fn script_install_carries_willington_runtime_files_with_cache_blockers() {
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let payload = source.parent().unwrap().join(REMOTE_SCRIPT_PACKAGE).join(WILLINGTON_FOLDER);
    for (name, text) in [
        ("release.json", "{}\n"),
        ("WillingtonRuntime/__init__.py", "LOADED = True\n"),
        ("WillingtonDeviceTools/__init__.py", ""),
        ("WillingtonDeviceTools/api.py", "def install(): pass\n"),
        ("WillingtonDeviceTools/build/live-12.4.15b5-windows-x86_64/build.json", "{}\n"),
    ] {
        std::fs::create_dir_all(payload.join(name).parent().unwrap()).unwrap();
        std::fs::write(payload.join(name), text).unwrap();
    }
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    install_remote_script(&source, &destination, &InstallOptions::default()).unwrap();
    let installed = destination.join(WILLINGTON_FOLDER);
    assert_eq!(std::fs::read_to_string(installed.join("WillingtonDeviceTools/api.py")).unwrap(), "def install(): pass\n");
    assert_eq!(std::fs::read_to_string(installed.join("release.json")).unwrap(), "{}\n");
    // A blocker beside each folder's Python files, and none where there are none.
    for blocked in ["WillingtonRuntime", "WillingtonDeviceTools"] {
        let blocker = installed.join(blocked).join("__pycache__");
        assert!(blocker.is_file() && std::fs::metadata(&blocker).unwrap().len() == 0, "{blocked}");
    }
    for unblocked in ["", "WillingtonDeviceTools/build", "WillingtonDeviceTools/build/live-12.4.15b5-windows-x86_64"] {
        assert!(!installed.join(unblocked).join("__pycache__").exists(), "{unblocked}");
    }
    let python = if cfg!(windows) { "python.exe" } else { "python3" };
    let imported = std::process::Command::new(python)
        .args(["-c", "import WillingtonRuntime; assert WillingtonRuntime.LOADED"])
        .env("PYTHONPATH", &installed)
        .output()
        .unwrap();
    assert!(imported.status.success(), "{}", String::from_utf8_lossy(&imported.stderr));
    assert!(installed.join("WillingtonRuntime/__pycache__").is_file());
    // A bytecode cache in the payload refuses the install and keeps the installed generation.
    std::fs::create_dir(payload.join("WillingtonRuntime/__pycache__")).unwrap();
    let force = InstallOptions { force: true, ..Default::default() };
    assert!(install_remote_script(&source, &destination, &force).unwrap_err().message().contains("can't contain"));
    assert_eq!(std::fs::read_to_string(installed.join("WillingtonRuntime/__init__.py")).unwrap(), "LOADED = True\n");
    assert!(folder.path().read_dir().unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".ableton-mcp-install")));
}
#[test]
fn script_install_keeps_willingtons_self_test_receipt_unless_the_release_ships_one() {
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let willington = source.parent().unwrap().join(REMOTE_SCRIPT_PACKAGE).join(WILLINGTON_FOLDER);
    std::fs::create_dir_all(willington.join("WillingtonBindings")).unwrap();
    std::fs::write(willington.join("WillingtonBindings/__init__.py"), "def install(): pass\n").unwrap();
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    install_remote_script(&source, &destination, &InstallOptions::default()).unwrap();
    let force = InstallOptions { force: true, ..Default::default() };
    let receipt = destination.join(WILLINGTON_RECEIPT);
    std::fs::write(&receipt, "the producer's receipt").unwrap();
    install_remote_script(&source, &destination, &force).unwrap();
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "the producer's receipt");
    assert_eq!(secret_permissions(&receipt), SecretPermissions::OwnerOnly);
    // A release that ships a receipt brings its own.
    std::fs::write(willington.join("WillingtonBindings/self-test.json"), "the release's receipt").unwrap();
    install_remote_script(&source, &destination, &force).unwrap();
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "the release's receipt");
    // Without Willington in the release, there's no copy for a receipt to stay with.
    std::fs::remove_dir_all(&willington).unwrap();
    install_remote_script(&source, &destination, &force).unwrap();
    assert!(!destination.join(WILLINGTON_FOLDER).exists());
}
#[test]
fn script_install_keeps_native_editings_self_test_receipt_unless_the_release_ships_one() {
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let willington = source.parent().unwrap().join(REMOTE_SCRIPT_PACKAGE).join(WILLINGTON_FOLDER);
    std::fs::create_dir_all(willington.join("WillingtonEditing")).unwrap();
    std::fs::write(willington.join("WillingtonEditing/__init__.py"), "def install(): pass\n").unwrap();
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    install_remote_script(&source, &destination, &InstallOptions::default()).unwrap();
    let force = InstallOptions { force: true, ..Default::default() };
    let receipt = destination.join(WILLINGTON_EDITING_RECEIPT);
    write_owner_file(&receipt, folder.path(), b"the producer's receipt").unwrap();
    install_remote_script(&source, &destination, &force).unwrap();
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "the producer's receipt");
    assert_eq!(secret_permissions(&receipt), SecretPermissions::OwnerOnly);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o666)).unwrap();
        install_remote_script(&source, &destination, &force).unwrap();
        assert!(!receipt.exists(), "an update must not trust an unsafe receipt");
    }
    // A release that ships a receipt brings its own.
    std::fs::write(willington.join("WillingtonEditing/self-test.json"), "the release's receipt").unwrap();
    install_remote_script(&source, &destination, &force).unwrap();
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "the release's receipt");
    // Without Willington in the release, there's no copy for a receipt to stay with.
    std::fs::remove_dir_all(&willington).unwrap();
    install_remote_script(&source, &destination, &force).unwrap();
    assert!(!destination.join(WILLINGTON_FOLDER).exists());
}
#[test]
fn script_install_keeps_the_willington_switch_owner_only() {
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    install_remote_script(&source, &destination, &InstallOptions::default()).unwrap();
    let switch = destination.join(WILLINGTON_CONFIG);
    let on = br#"{"version":1,"followActions":true,"deviceTools":true,"rackZones":true,"enableWrites":true}"#;
    // Staged where it's told, outside the installed package: with nowhere to stage, nothing is written.
    let elsewhere = folder.path().join("missing staging folder");
    assert!(write_owner_file(&switch, &elsewhere, on).unwrap_err().message().contains("missing staging folder"));
    assert!(!switch.exists());
    write_owner_file(&switch, folder.path(), on).unwrap();
    assert_eq!(secret_permissions(&switch), SecretPermissions::OwnerOnly);
    // Replaced whole, still owner-only.
    write_owner_file(&switch, folder.path(), on).unwrap();
    assert_eq!(std::fs::read(&switch).unwrap(), on);
    install_remote_script(&source, &destination, &InstallOptions { force: true, ..Default::default() }).unwrap();
    assert_eq!(std::fs::read(&switch).unwrap(), on);
    assert_eq!(secret_permissions(&switch), SecretPermissions::OwnerOnly);
    assert!(destination.read_dir().unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".ableton-mcp-")));
}
#[test]
fn failed_install_keeps_existing_generation_and_cleans_staging() {
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    install_remote_script(&source, &destination, &InstallOptions::default()).unwrap();
    std::fs::write(destination.join("willington.json"), vec![b'x'; 4097]).unwrap();
    let force = InstallOptions { force: true, ..Default::default() };
    assert_eq!(
        install_remote_script(&source, &destination, &force).unwrap_err().message(),
        "Willington configuration must be a bounded regular file"
    );
    assert_eq!(std::fs::read_to_string(destination.join(REMOTE_SCRIPT_ASSET)).unwrap(), "production-remote-script");
    std::fs::remove_file(source.parent().unwrap().join(REMOTE_SCRIPT_PACKAGE).join("__init__.py")).unwrap();
    assert_eq!(install_remote_script(&source, &destination, &force).unwrap_err().message(), "Remote Script package is missing __init__.py");
    assert_eq!(folder.path().read_dir().unwrap().count(), 2);
}
#[cfg(unix)]
#[test]
fn linked_install_destination_and_source_are_rejected() {
    let folder = tempfile::tempdir().unwrap();
    let source = install_source(folder.path());
    let destination = folder.path().join(REMOTE_SCRIPT_PACKAGE);
    std::fs::create_dir(&destination).unwrap();
    let link = destination.join("linked-file");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let force = InstallOptions { force: true, ..Default::default() };
    assert!(install_remote_script(&source, &destination, &force).unwrap_err().message().contains("symbolic-link destination"));
    assert_eq!(install_remote_script(&link, &destination, &force).unwrap_err().message(), "Remote Script source must be a regular file");
    assert_eq!(std::fs::read_to_string(source).unwrap(), "production-remote-script");
}
#[test]
fn diagnostic_package_and_config_evidence_stays_distinct_from_live_readiness() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let empty = diagnostics(Some(root), None);
    assert_eq!(empty["hostReady"], false);
    assert_eq!(empty["ready"], false);
    assert_eq!(empty["runtime"], "rust-native");
    let source = install_source(root);
    let remote = root.join("remote-script");
    std::fs::create_dir(&remote).unwrap();
    install_remote_script(&source, &remote.join(REMOTE_SCRIPT_PACKAGE), &InstallOptions::default()).unwrap();
    std::fs::write(native_entrypoint(root), "native binary fixture").unwrap();
    let secret = root.join("secret");
    write_secret_file(&secret, None).unwrap();
    let path = root.join("config.json");
    let config = config_for_bridge(
        &native_entrypoint(root),
        &json!({"host":"127.0.0.1","port":43567,"secretFile":secret,"timeoutMs":100}),
        None,
        Some(&path),
        true,
    )
    .unwrap();
    write_config(&path, &config, false).unwrap();
    let local = diagnostics(Some(root), Some(&path));
    assert_eq!(
        local["readiness"],
        json!({"package":true,"configured":true,"authenticatedBridge":false,"realLiveOperational":false,"releaseCertified":false})
    );
    assert_eq!(local["evidence"], "local-contract");
    assert_eq!(local["ready"], false);
    std::fs::write(remote.join(REMOTE_SCRIPT_PACKAGE).join(REMOTE_SCRIPT_ASSET), "changed").unwrap();
    assert_eq!(diagnostics(Some(root), Some(&path))["packageAssetsValid"], false);
    std::fs::remove_file(&secret).unwrap();
    let missing = diagnostics(Some(root), Some(&path));
    assert_eq!(missing["config"]["valid"], false);
    assert_eq!(missing["secretPermissions"], "unavailable");
    assert_eq!(missing["bridgeConfigured"], false);
}

/// Windows' own reading of Kumi's shape (`lifecycle::delivery::native_acl`) held to the bridge's PowerShell check, on the shapes a DACL can
/// take: whatever the fast path calls owner-only the check passes, and the files Kumi makes take the fast path.
#[cfg(windows)]
mod windows_acl_shapes {
    use ableton_mcp_server::lifecycle::delivery::{
        native_acl::{
            is_kumis_shape, make_kumis_shape,
            shapes::{own, own_mine, write, write_raw, Ace, Whose, FULL, INHERITABLE, INHERITED, INHERIT_ONLY},
        },
        powershell,
    };
    use std::path::{Path, PathBuf};

    const EVERYONE: &str = "S-1-1-0";
    const ADMINISTRATORS: &str = "S-1-5-32-544";
    const READ: u32 = 0x0012_0089;
    const WRITE: u32 = 0x0012_0116;
    const GENERIC_ALL: u32 = 0x1000_0000;

    fn allow(flags: u32, mask: u32, whose: Whose) -> Ace {
        Ace { allow: true, flags, mask, whose }
    }

    #[test]
    fn the_fast_path_says_owner_only_only_where_the_powershell_check_does() {
        let folder = tempfile::tempdir().unwrap();
        // A folder that hands nothing down, so an unprotected DACL holds only what's written on it.
        write(folder.path(), Some(&[allow(0, FULL, Whose::Me)]), true);
        let made = |name: &str| {
            let path = folder.path().join(name);
            std::fs::write(&path, "{}").unwrap();
            own_mine(&path);
            path
        };
        let in_temp = tempfile::NamedTempFile::new().unwrap();
        // (name, file, the check's code where the shape fixes it)
        let mut shapes: Vec<(&str, PathBuf, Option<i32>)> = vec![("a new file in TEMP", in_temp.path().to_path_buf(), None)];
        let natively = made("natively.json");
        make_kumis_shape(&natively).unwrap();
        shapes.push(("made natively", natively, Some(0)));
        let scripted = made("scripted.json");
        powershell::secure(&scripted).unwrap();
        shapes.push(("made by SECURE_FILE", scripted, Some(0)));
        let cases: Vec<(&str, Option<Vec<Ace>>, bool, Option<i32>)> = vec![
            ("Kumi's shape, written raw", Some(vec![allow(0, FULL, Whose::Me)]), true, Some(0)),
            ("a second rule, for everyone", Some(vec![allow(0, FULL, Whose::Me), allow(0, READ, Whose::Sid(EVERYONE))]), true, Some(4)),
            (
                "two rules for you that add up to full control",
                Some(vec![allow(0, READ, Whose::Me), allow(0, FULL & !READ, Whose::Me)]),
                true,
                None,
            ),
            // Windows tidies a file's ACEs as it writes them (flags dropped, generic rights mapped): the check decides.
            ("its one rule inherited", Some(vec![allow(INHERITED, FULL, Whose::Me)]), true, None),
            ("its one rule inheritable", Some(vec![allow(INHERITABLE, FULL, Whose::Me)]), true, None),
            ("its one rule inherit-only", Some(vec![allow(INHERITABLE | INHERIT_ONLY, FULL, Whose::Me)]), true, None),
            ("generic rights in place of full control", Some(vec![allow(0, GENERIC_ALL, Whose::Me)]), true, None),
            ("full control and generic rights besides", Some(vec![allow(0, FULL | GENERIC_ALL, Whose::Me)]), true, None),
            ("read and write, not full control", Some(vec![allow(0, READ | WRITE, Whose::Me)]), true, Some(8)),
            ("a deny rule", Some(vec![Ace { allow: false, flags: 0, mask: FULL, whose: Whose::Me }]), true, Some(7)),
            ("a rule for everyone alone", Some(vec![allow(0, FULL, Whose::Sid(EVERYONE))]), true, Some(5)),
            ("not protected", Some(vec![allow(0, FULL, Whose::Me)]), false, Some(3)),
            ("an empty DACL", Some(vec![]), true, Some(4)),
            ("a null DACL", None, true, None),
        ];
        for (n, (name, aces, protected, expected)) in cases.into_iter().enumerate() {
            let path = made(&format!("case-{n}.json"));
            write(&path, aces.as_deref(), protected);
            shapes.push((name, path, expected));
        }
        // Written whole, flags kept: an inherited ACE in a protected DACL, and an inherit-only one.
        for (name, flags, expected) in [
            ("its one rule marked inherited, written whole", INHERITED, Some(6)),
            ("its one rule inherit-only, written whole", INHERITABLE | INHERIT_ONLY, None),
        ] {
            let path = made(&format!("{}.json", name.replace([' ', ','], "-")));
            write_raw(&path, &[allow(flags, FULL, Whose::Me)]);
            shapes.push((name, path, expected));
        }
        // Owned by Administrators, where this account may hand it over: CI's elevated runners can.
        let handed = made("handed-over.json");
        write(&handed, Some(&[allow(0, FULL, Whose::Me)]), true);
        if own(&handed, ADMINISTRATORS) {
            shapes.push(("owned by Administrators", handed, Some(2)));
        } else {
            assert!(std::env::var_os("CI").is_none(), "this account couldn't hand the file to Administrators");
            println!("skipped: owned by Administrators (this account may not hand a file over)");
        }
        // Every shape in one PowerShell, the same check as the one `windows_owner_only` runs file by file (held to it on
        // three of them), then the whole matrix before any assertion.
        let paths: Vec<&Path> = shapes.iter().map(|(_, path, _)| path.as_path()).collect();
        let codes = powershell::checks(&paths);
        assert_eq!(codes.len(), shapes.len(), "one code a file: {codes:?}");
        for at in [0, 1, shapes.len() - 1] {
            assert_eq!(powershell::check(paths[at]), codes[at], "{}: one run a file says what the batch does", shapes[at].0);
        }
        let rows: Vec<_> =
            shapes.iter().zip(&codes).map(|((name, path, expected), code)| (*name, is_kumis_shape(path), *code, *expected)).collect();
        let said = rows
            .iter()
            .map(|(name, fast, check, _)| format!("{name}: fast path {fast}, PowerShell check {check}"))
            .collect::<Vec<_>>()
            .join("\n");
        println!("{said}");
        // Each file gets back a DACL that lets this account remove it (a deny, an empty or an inherit-only DACL leaves it
        // none), so the folder goes when the test ends.
        for (_, path, _) in &shapes {
            write(path, Some(&[allow(0, FULL, Whose::Me)]), true);
        }
        for (name, fast, check, expected) in &rows {
            assert!(!fast || *check == 0, "{name}: the fast path passed what the check refuses\n{said}");
            if let Some(expected) = expected {
                assert_eq!(check, expected, "{name}: the shape isn't what it says\n{said}");
                assert_eq!(*fast, *expected == 0, "{name}: Kumi's own shape takes the fast path, and only it\n{said}");
            }
        }
    }

    /// A file the fast path takes by its own path goes to the PowerShell check by any other way to it: a share, a drive
    /// mapped to one, a junction or a link on the way. Nothing is written there either.
    #[test]
    fn the_fast_path_leaves_shares_and_links_to_the_powershell_check() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("auth.json");
        std::fs::write(&file, "{}").unwrap();
        make_kumis_shape(&file).unwrap();
        assert!(is_kumis_shape(&file), "the file itself takes the fast path");
        let local = file.display().to_string();
        let (letter, rest) = local.split_once(":\\").unwrap();
        for shared in [format!(r"\\localhost\{letter}$\{rest}"), format!(r"\\?\UNC\localhost\{letter}$\{rest}")] {
            assert!(!is_kumis_shape(Path::new(&shared)), "{shared}");
            assert!(make_kumis_shape(Path::new(&shared)).is_err(), "nothing written by a share's path: {shared}");
        }
        // The same file by a drive-relative path, from its folder: the drive's working folder is the test's own, and goes
        // back to what it was (Windows can't remove a process's working folder, and other tests start from it).
        struct Back(PathBuf);
        impl Drop for Back {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }
        let back = Back(std::env::current_dir().unwrap());
        std::env::set_current_dir(folder.path()).unwrap();
        let relative = PathBuf::from(format!("{letter}:auth.json"));
        let (reached, fast, written) = (relative.is_file(), is_kumis_shape(&relative), make_kumis_shape(&relative).is_ok());
        drop(back);
        assert!(reached, "the drive-relative path reaches the file");
        assert!(!fast && !written, "a drive-relative path goes to PowerShell");
        // A drive mapped to that share, which Windows calls remote: CI's runners can map one.
        let free = ('G'..='Y').rev().find(|drive| !Path::new(&format!("{drive}:\\")).exists());
        let mapped = free.filter(|drive| {
            std::process::Command::new("net")
                .args(["use", &format!("{drive}:"), &format!(r"\\localhost\{letter}$")])
                .output()
                .is_ok_and(|made| made.status.success())
        });
        match mapped {
            Some(drive) => {
                let through = PathBuf::from(format!(r"{drive}:\{rest}"));
                let reached = through.is_file();
                let (fast, written) = (is_kumis_shape(&through), make_kumis_shape(&through).is_ok());
                let _ = std::process::Command::new("net").args(["use", &format!("{drive}:"), "/delete", "/y"]).output();
                assert!(reached, "the mapped drive reaches the file");
                assert!(!fast && !written, "a mapped drive goes to PowerShell");
            }
            None => {
                assert!(std::env::var_os("CI").is_none(), "this runner couldn't map a drive to its own share");
                println!("skipped: a mapped drive (no drive could be mapped)");
            }
        }
        // Through a junction to its folder, which needs no privilege to make.
        let parent = tempfile::tempdir().unwrap();
        let junction = parent.path().join("through");
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&junction).arg(folder.path()).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
        let linked = junction.join("auth.json");
        assert!(linked.is_file());
        assert!(!is_kumis_shape(&linked), "{}", linked.display());
        assert!(make_kumis_shape(&linked).is_err(), "nothing written through a junction");
        // A link to the file itself, where this account may make one.
        let link = parent.path().join("link.json");
        if std::os::windows::fs::symlink_file(&file, &link).is_ok() {
            assert!(!is_kumis_shape(&link), "{}", link.display());
        }
        let _ = std::fs::remove_dir(&junction);
    }
}
