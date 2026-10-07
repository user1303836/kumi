//! Mutating lifecycle actions and their exact compensation boundaries.
use super::*;
use release::assert_package_still_bound;
fn source(o: &LifecycleOptions) -> PathBuf {
    o.package_root.join("remote-script").join(REMOTE_SCRIPT_PACKAGE).join(REMOTE_SCRIPT_ASSET)
}
fn finish(result: &mut Value, next: &Value) {
    result["applied"] = true.into();
    result["state"] = "completed".into();
    result["instructions"] = json!([next["activation"]["remediation"]]);
    complete_steps(result);
}
/// Runs every compensation step, in order, even after one fails, and names the ones that failed.
fn compensate<const N: usize>(steps: [(&str, Result<(), LiveError>); N]) -> Vec<String> {
    steps.into_iter().filter_map(|(step, result)| result.err().map(|error| format!("{step}: {}", error.message()))).collect()
}
/// The action's own error once compensation is complete; otherwise both, saying "compensation was incomplete"
/// (what Kumi's installer looks for to send the producer to doctor).
fn compensated(action: &str, error: LiveError, failed: &[String]) -> LiveError {
    if failed.is_empty() {
        return error;
    }
    fail(format!("{action} failed ({}) and its compensation was incomplete: {}", error.message(), failed.join("; ")))
}
fn journal_failed(paths: &Paths, action: &str, error: &LiveError, failed: &[String]) {
    let state = if failed.is_empty() { "failed-rolled-back" } else { "failed-compensation-incomplete" };
    try_write_owner_json(
        &paths.journal,
        &json!({"version":1,"action":action,"state":state,"reason":error.message(),"compensationFailures":failed}),
    );
}
fn bridge_config(root: &Path, bridge: &Value, path: &Path) -> Result<BridgeConfig, LiveError> {
    config_for_bridge(&native_entrypoint(root), bridge, None, Some(path), true)
}
fn unique(values: impl IntoIterator<Item = Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}
fn version_compare(candidate: &str, current: &str) -> Result<std::cmp::Ordering, LiveError> {
    let regex = regex::Regex::new(r"^([0-9]+)\.([0-9]+)\.([0-9]+)(?:[-+].*)?$").unwrap();
    let parse = |s: &str| -> Result<Vec<f64>, LiveError> {
        let captures = regex.captures(s).ok_or_else(|| fail(format!("package version is not semantic: {s}")))?;
        Ok((1..=3).map(|i| kumi_common::js::number::parse(&captures[i]).unwrap_or(f64::INFINITY)).collect())
    };
    let a = parse(candidate)?;
    let b = parse(current)?;
    for (a, b) in a.into_iter().zip(b) {
        if a != b {
            return Ok(if a > b { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less });
        }
    }
    Ok(std::cmp::Ordering::Equal)
}
pub(super) fn apply(
    o: &LifecycleOptions,
    paths: &Paths,
    receipt: &Value,
    evidence: Option<&ReleaseEvidence>,
    artifact_sha256: &str,
    result: &mut Value,
) -> Result<(), LiveError> {
    if o.action == "install" {
        return install(o, paths, receipt, evidence.unwrap(), artifact_sha256, result);
    }
    if receipt.is_null() || (receipt["status"] == "uninstalled" && o.action != "uninstall") {
        return Err(fail("an active installation receipt is required"));
    }
    match o.action.as_str() {
        "upgrade" => upgrade(o, paths, receipt, evidence.unwrap(), artifact_sha256, result),
        "repair" => repair(o, paths, receipt, evidence.unwrap(), result),
        "rollback" => rollback(o, paths, receipt, result),
        "uninstall" => uninstall(o, paths, receipt, result),
        _ => Err(fail("lifecycle action was not handled")),
    }
}
fn install(
    o: &LifecycleOptions,
    paths: &Paths,
    receipt: &Value,
    evidence: &ReleaseEvidence,
    artifact_sha256: &str,
    result: &mut Value,
) -> Result<(), LiveError> {
    if active(receipt) {
        return Err(fail("installation already exists; use upgrade or repair"));
    }
    if paths.remote.exists() || paths.config.exists() {
        return Err(fail("unowned destination content exists; adopt it manually or choose an empty lifecycle root"));
    }
    let (mut backup, mut secret_created, mut diagnostics_created, mut config_created) = (None, false, false, false);
    let installed = (|| -> Result<(), LiveError> {
        if !paths.secret.exists() {
            write_secret_file(&paths.secret, Some(&generate_secret(None)?))?;
            secret_created = true;
        } else {
            read_secret_file(&paths.secret)?;
        }
        if o.enable_bridge_diagnostics {
            diagnostics_created = ensure_diagnostics_file(&paths.diagnostics)?;
        }
        fault(o, "after-secret")?;
        let mut bridge = json!({"host":o.host.as_deref().unwrap_or("127.0.0.1"),"port":o.port.unwrap_or(9765.),"secretFile":paths.secret,"timeoutMs":o.timeout_ms.unwrap_or(5000.),"realtimePort":o.realtime_port.unwrap_or(9766.)});
        if o.enable_bridge_diagnostics {
            bridge["diagnostics"] = json!({"path":paths.diagnostics,"maxBytes":BRIDGE_DIAGNOSTICS_MAX_BYTES});
        }
        let config = bridge_config(&o.package_root, &bridge, &paths.config)?;
        write_config(&paths.config, &config, false)?;
        config_created = true;
        fault(o, "after-config")?;
        let installed = install_remote_script(
            &source(o),
            &paths.remote,
            &InstallOptions { config_path: Some(paths.config.clone()), ..Default::default() },
        )?;
        backup = installed.backup;
        fault(o, "after-remote")?;
        let remote_files = hash_regular_tree(&paths.remote)?;
        assert_package_still_bound(&o.package_root, evidence, o.allow_dirty_private_build)?;
        let next = json!({"version":LIFECYCLE_RECEIPT_VERSION,"status":"installed-restart-required","generation":next_generation(receipt),"platform":crate::platform::current_platform(),"packageRoot":o.package_root,"packageVersion":evidence.manifest["package"]["version"],"artifactSha256":artifact_sha256,"releaseManifestSha256":evidence.manifest_sha256,"registryHash":evidence.manifest["protocol"]["registryHash"],"stateDirectory":o.state_directory,"remoteScriptsDirectory":o.remote_scripts_directory,"remoteScriptDirectory":paths.remote,"remoteFiles":remote_files,"configPath":paths.config,"config":config,"configSha256":file_digest(&paths.config)?,"secretPath":paths.secret,"secretCreatedByLifecycle":secret_created||receipt["secretCreatedByLifecycle"]==true,"previous":null,"activation":activation("Restart Live, select AbletonMcpBridge as a Control Surface, then run activate."),"lastAction":"install"});
        fault(o, "before-receipt")?;
        write_owner_json(&paths.receipt, &next)?;
        let finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"install","state":"completed","generation":next["generation"]}),
        );
        result["restartRequired"] = true.into();
        result["verification"] = json!({"packageVersion":next["packageVersion"],"artifactSha256":next["artifactSha256"],"releaseManifestSha256":next["releaseManifestSha256"],"registryHash":next["registryHash"],"remoteFiles":remote_files.len(),"configSha256":next["configSha256"],"configPermissions":secret_permissions(&paths.config),"secretPermissions":secret_permissions(&paths.secret),"bridgeDiagnostics":config.bridge.diagnostics.as_ref().map(|d|json!({"path":d.path,"maxBytes":d.max_bytes,"permissions":secret_permissions(&d.path)})),"portsAvailableAtPreflight":true,"journalFinalized":finalized});
        finish(result, &next);
        Ok(())
    })();
    if let Err(error) = installed {
        let failed = compensate([
            ("Remote Script", fault(o, "compensate-remote").and_then(|()| restore_backup(&paths.remote, backup.as_deref()))),
            ("bridge config", if config_created { remove(&paths.config, false) } else { Ok(()) }),
            ("diagnostics", if diagnostics_created { remove(&paths.diagnostics, false) } else { Ok(()) }),
            ("secret", if secret_created { remove(&paths.secret, false) } else { Ok(()) }),
        ]);
        journal_failed(paths, "install", &error, &failed);
        return Err(compensated("install", error, &failed));
    }
    Ok(())
}
fn upgrade(
    o: &LifecycleOptions,
    paths: &Paths,
    receipt: &Value,
    evidence: &ReleaseEvidence,
    artifact_sha256: &str,
    result: &mut Value,
) -> Result<(), LiveError> {
    let order =
        version_compare(evidence.manifest["package"]["version"].as_str().unwrap_or(""), receipt["packageVersion"].as_str().unwrap_or(""))?;
    // The first native distribution keeps the bridge protocol/package version. A runtime
    // migration at that version is allowed only from the exact verified Node generation;
    // ordinary native upgrades and all downgrades retain the semantic-version fence.
    // No real install takes this branch any more: every shipped Node generation carries the
    // registry before this bridge's (ec05dd40…), which the last check refuses. It goes with
    // the Node removal; until then it stays fenced, never loosened.
    let runtime_migration = if order == std::cmp::Ordering::Equal
        && evidence.manifest["package"]["version"] == receipt["packageVersion"]
        && evidence.manifest["schema"] == "ableton-mcp-native-release/v1"
    {
        let current = verify_retained_package(p(&receipt["packageRoot"]), o.allow_dirty_private_build)?;
        ["ableton-mcp-release/v2", "ableton-mcp-private-release/v1"].iter().any(|schema| current.manifest["schema"] == *schema)
            && json!(current.manifest_sha256) == receipt["releaseManifestSha256"]
            && current.manifest["package"]["version"] == receipt["packageVersion"]
            && current.manifest["protocol"]["registryHash"] == receipt["registryHash"]
            // The same version keeps the same protocol: a registry change comes with a newer bridge.
            && current.manifest["protocol"]["registryHash"] == registry_digest()
    } else {
        false
    };
    if order != std::cmp::Ordering::Greater && !runtime_migration {
        return Err(fail("upgrade requires a strictly newer semantic package version; use rollback for a prior retained generation"));
    }
    if json!(artifact_sha256) == receipt["artifactSha256"] && json!(evidence.manifest_sha256) == receipt["releaseManifestSha256"] {
        return Err(fail("candidate is already installed; use repair"));
    }
    let drift = verify_files(&paths.remote, &receipt["remoteFiles"])?;
    if drift["valid"] != true
        || json!(file_digest(&paths.config)?) != receipt["configSha256"]
        || !owner_only(&paths.config)
        || !owner_only(&paths.secret)
    {
        return Err(fail("current generation is drifted; repair or resolve it before upgrade"));
    }
    if receipt["previous"].is_object() {
        let backup = p(&receipt["previous"]["remoteBackup"]);
        if !contained(&o.state_directory, backup) || verify_files(backup, &receipt["previous"]["remoteFiles"])?["valid"] != true {
            return Err(fail("retained rollback generation is outside owner state or drifted"));
        }
    }
    let current = read_any_config(&paths.config)?;
    let Some(bridge) = current.bridge() else {
        return Err(fail("managed bridge config is invalid"));
    };
    let mut backup = None;
    let operation = (|| -> Result<(), LiveError> {
        let next_config = bridge_config(&o.package_root, &serde_json::to_value(bridge)?, &paths.config)?;
        write_config(&paths.config, &next_config, true)?;
        fault(o, "before-remote")?;
        let installed = install_remote_script(
            &source(o),
            &paths.remote,
            &InstallOptions { force: true, config_path: Some(paths.config.clone()), ..Default::default() },
        )?;
        backup = installed.backup;
        if let Some(current) = &backup {
            let owner_backup = quarantine_path(&o.state_directory, "rollback-generation")?;
            move_remote_folder(current, &owner_backup)?;
            backup = Some(owner_backup);
        }
        fault(o, "after-remote")?;
        let Some(backup) = &backup else {
            return Err(fail("upgrade did not retain a previous Remote Script generation"));
        };
        assert_package_still_bound(&o.package_root, evidence, o.allow_dirty_private_build)?;
        let previous = json!({"packageRoot":receipt["packageRoot"],"packageVersion":receipt["packageVersion"],"artifactSha256":receipt["artifactSha256"],"releaseManifestSha256":receipt["releaseManifestSha256"],"registryHash":receipt["registryHash"],"remoteBackup":backup,"remoteFiles":receipt["remoteFiles"],"config":receipt["config"],"configSha256":receipt["configSha256"]});
        let retired = receipt["previous"].get("remoteBackup").filter(|v| p(v) != backup);
        let preserved = retention(receipt, "preserved");
        let pending_before = unique(retention(receipt, "pendingCleanup").into_iter().chain(retired.cloned()));
        let mut next = receipt.clone();
        for (key, value) in [
            ("status", json!("installed-restart-required")),
            ("generation", next_generation(receipt)),
            ("packageRoot", json!(o.package_root)),
            ("packageVersion", evidence.manifest["package"]["version"].clone()),
            ("artifactSha256", json!(artifact_sha256)),
            ("releaseManifestSha256", json!(evidence.manifest_sha256)),
            ("registryHash", evidence.manifest["protocol"]["registryHash"].clone()),
            ("remoteFiles", json!(hash_regular_tree(&paths.remote)?)),
            ("config", json!(next_config)),
            ("configSha256", json!(file_digest(&paths.config)?)),
            ("previous", previous.clone()),
            ("retained", json!({"pendingCleanup":pending_before,"preserved":preserved})),
            ("activation", activation("Restart Live and run activate; rollback remains available.")),
            ("lastAction", json!("upgrade")),
        ] {
            next[key] = value;
        }
        fault(o, "before-receipt")?;
        write_owner_json(&paths.receipt, &next)?;
        let removed = retired.is_none_or(|path| o.fault_at.as_deref() != Some("retired-cleanup-blocked") && try_remove(p(path), true));
        let pending = if removed && retired.is_some() {
            pending_before.into_iter().filter(|v| Some(v) != retired).collect::<Vec<_>>()
        } else {
            pending_before
        };
        next["retained"] = json!({"pendingCleanup":pending,"preserved":preserved});
        let retention_finalized = try_write_owner_json(&paths.receipt, &next);
        let journal_finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"upgrade","state":"completed","generation":next["generation"],"pendingCleanup":pending,"preserved":preserved}),
        );
        result["recovery"]["rollbackAvailable"] = true.into();
        recovery_quarantine(result, &preserved, &pending);
        result["verification"] = json!({"artifactSha256":next["artifactSha256"],"previousArtifactSha256":previous["artifactSha256"],"remoteFiles":next["remoteFiles"].as_object().unwrap().len(),"retiredPreviousBackupRemoved":removed,"pendingCleanup":pending,"retentionFinalized":retention_finalized,"journalFinalized":journal_finalized});
        finish(result, &next);
        Ok(())
    })();
    if let Err(error) = operation {
        let failed = compensate([
            (
                "Remote Script",
                fault(o, "compensate-remote")
                    .and_then(|()| backup.as_deref().map_or(Ok(()), |backup| restore_backup(&paths.remote, Some(backup)))),
            ),
            ("bridge config", write_config(&paths.config, &receipt["config"], true)),
        ]);
        journal_failed(paths, "upgrade", &error, &failed);
        return Err(compensated("upgrade", error, &failed));
    }
    Ok(())
}
fn repair(o: &LifecycleOptions, paths: &Paths, receipt: &Value, evidence: &ReleaseEvidence, result: &mut Value) -> Result<(), LiveError> {
    let drift = verify_files(&paths.remote, &receipt["remoteFiles"])?;
    let config_valid = config_valid(receipt)?;
    let permissions_valid = paths.secret.exists() && owner_only(&paths.secret);
    let diagnostics = diagnostics_path(receipt);
    let diagnostics_valid = diagnostics.is_none_or(diagnostics_file_valid);
    if drift["valid"] == true && config_valid && permissions_valid && diagnostics_valid {
        let finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"repair","state":"completed","generation":receipt["generation"],"changed":false}),
        );
        result["applied"] = true.into();
        result["state"] = "completed".into();
        result["verification"] = json!({"changed":false,"remoteScript":drift,"configValid":config_valid,"permissionsValid":permissions_valid,"diagnosticsValid":diagnostics_valid,"journalFinalized":finalized});
        for step in result["steps"].as_array_mut().unwrap() {
            step["status"] = json!(if step["id"] == "inspect" || step["id"] == "verify" { "completed" } else { "skipped" });
        }
        return Ok(());
    }
    if !paths.secret.exists() {
        return Err(fail("managed secret is missing; repair refuses to manufacture new bridge authority"));
    }
    // What repair moved aside (each set once its move succeeded) or made: its compensation takes back only those.
    // A step that failed, or never ran, left the original where it was.
    let (remote_existed, config_existed) = (paths.remote.exists(), paths.config.exists());
    let (mut remote_quarantine, mut config_quarantine, mut diagnostics_quarantine, mut diagnostics_created) = (None, None, None, false);
    let operation = (|| -> Result<(), LiveError> {
        if remote_existed {
            let quarantine = quarantine_path(&o.state_directory, "repair-remote")?;
            move_remote_folder(&paths.remote, &quarantine)?;
            remote_quarantine = Some(quarantine);
        }
        if config_existed && !config_valid {
            let quarantine = quarantine_path(&o.state_directory, "repair-config")?;
            rename(&paths.config, &quarantine)?;
            config_quarantine = Some(quarantine);
        }
        if let Some(path) = diagnostics.filter(|_| !diagnostics_valid) {
            if path_entry_exists(path)? {
                let quarantine = quarantine_path(&o.state_directory, "repair-diagnostics")?;
                rename(path, &quarantine)?;
                diagnostics_quarantine = Some(quarantine);
            }
            diagnostics_created = ensure_diagnostics_file(path)?;
        }
        if !permissions_valid {
            chmod(&paths.secret, 0o600)?;
            secure_windows_file(&paths.secret)?;
            if !owner_only(&paths.secret) {
                return Err(fail("repair could not restore owner-only secret permissions"));
            }
        }
        write_config(&paths.config, &receipt["config"], paths.config.exists())?;
        // The producer's files (Willington's switch and self-test receipt) come along from the copy moved aside.
        install_remote_script(
            &source(o),
            &paths.remote,
            &InstallOptions {
                config_path: Some(paths.config.clone()),
                producer_files_from: remote_quarantine.clone(),
                ..Default::default()
            },
        )?;
        fault(o, "after-repair-install")?;
        assert_package_still_bound(&o.package_root, evidence, o.allow_dirty_private_build)?;
        let mut next = receipt.clone();
        next["generation"] = next_generation(receipt);
        next["remoteFiles"] = json!(hash_regular_tree(&paths.remote)?);
        next["configSha256"] = json!(file_digest(&paths.config)?);
        next["status"] = "installed-restart-required".into();
        next["activation"] = activation("Restart Live and rerun activate after repair.");
        next["lastAction"] = "repair".into();
        write_owner_json(&paths.receipt, &next)?;
        let finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"repair","state":"completed","generation":next["generation"],"quarantine":{"remote":remote_quarantine,"config":config_quarantine,"diagnostics":diagnostics_quarantine}}),
        );
        result["restartRequired"] = true.into();
        if let Some(path) = remote_quarantine.as_ref().or(config_quarantine.as_ref()).or(diagnostics_quarantine.as_ref()) {
            result["recovery"]["quarantine"] = json!(path);
        }
        result["verification"] = json!({"changed":true,"priorDrift":drift,"configRepaired":!config_valid,"diagnosticsRepaired":!diagnostics_valid,"secretPermissions":secret_permissions(&paths.secret),"repairedRemoteFiles":next["remoteFiles"].as_object().unwrap().len(),"configQuarantine":config_quarantine,"diagnosticsQuarantine":diagnostics_quarantine,"journalFinalized":finalized});
        finish(result, &next);
        Ok(())
    })();
    if let Err(error) = operation {
        // What's in the Remote Script's place now is repair's only once the original was moved aside, or when there
        // was none.
        let remote = || -> Result<(), LiveError> {
            fault(o, "compensate-remote")?;
            if remote_quarantine.is_some() || !remote_existed {
                remove(&paths.remote, true)?;
            }
            match &remote_quarantine {
                Some(path) => move_remote_folder(path, &paths.remote),
                None => Ok(()),
            }
        };
        let config = || -> Result<(), LiveError> {
            if config_quarantine.is_some() || !config_existed {
                remove(&paths.config, false)?;
            }
            match &config_quarantine {
                Some(path) => rename(path, &paths.config),
                None => Ok(()),
            }
        };
        let diagnostics_back = || -> Result<(), LiveError> {
            let Some(path) = diagnostics else { return Ok(()) };
            if diagnostics_created {
                remove(path, false)?;
            }
            match &diagnostics_quarantine {
                Some(quarantine) => rename(quarantine, path),
                None => Ok(()),
            }
        };
        let failed = compensate([("Remote Script", remote()), ("bridge config", config()), ("diagnostics", diagnostics_back())]);
        let state = if failed.is_empty() { "failed-rolled-back" } else { "failed-compensation-incomplete" };
        try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"repair","state":state,"reason":error.message(),"compensationFailures":failed,"quarantine":{"remote":remote_quarantine,"config":config_quarantine,"diagnostics":diagnostics_quarantine}}),
        );
        return Err(compensated("repair", error, &failed));
    }
    Ok(())
}
fn rollback(o: &LifecycleOptions, paths: &Paths, receipt: &Value, result: &mut Value) -> Result<(), LiveError> {
    let previous = &receipt["previous"];
    if !previous.is_object() || !p(&previous["remoteBackup"]).exists() {
        return Err(fail("no verified previous generation is available"));
    }
    let current = verify_files(&paths.remote, &receipt["remoteFiles"])?;
    if current["valid"] != true
        || diagnostics_path(receipt).is_some_and(|path| !diagnostics_file_valid(path))
        || !config_valid(receipt)?
        || !owner_only(&paths.secret)
    {
        return Err(fail("current generation is drifted; repair or preserve it before rollback"));
    }
    assert_no_linked_ancestors(p(&previous["remoteBackup"]))?;
    // The previous generation runs its own bridge again, with its own registry, which may be older than this one's.
    let previous_package = verify_retained_package(p(&previous["packageRoot"]), o.allow_dirty_private_build)?;
    if json!(previous_package.manifest_sha256) != previous["releaseManifestSha256"]
        || previous_package.manifest["protocol"]["registryHash"] != previous["registryHash"]
    {
        return Err(fail("previous package root is unavailable or differs from the retained generation"));
    }
    let failed = quarantine_path(&o.state_directory, "rolled-back-generation")?;
    move_remote_folder(&paths.remote, &failed)?;
    let operation = (|| -> Result<(), LiveError> {
        move_remote_folder(p(&previous["remoteBackup"]), &paths.remote)?;
        let verified = verify_files(&paths.remote, &previous["remoteFiles"])?;
        if verified["valid"] != true {
            return Err(fail("previous Remote Script generation failed hash verification"));
        }
        write_config(&paths.config, &previous["config"], true)?;
        fault(o, "after-rollback-config")?;
        let mut next = receipt.clone();
        for key in ["packageRoot", "packageVersion", "artifactSha256", "releaseManifestSha256", "registryHash", "remoteFiles", "config"] {
            next[key] = previous[key].clone();
        }
        next["status"] = "installed-restart-required".into();
        next["generation"] = next_generation(receipt);
        next["configSha256"] = json!(file_digest(&paths.config)?);
        next["previous"] = json!({"packageRoot":receipt["packageRoot"],"packageVersion":receipt["packageVersion"],"artifactSha256":receipt["artifactSha256"],"releaseManifestSha256":receipt["releaseManifestSha256"],"registryHash":receipt["registryHash"],"remoteBackup":failed,"remoteFiles":receipt["remoteFiles"],"config":receipt["config"],"configSha256":receipt["configSha256"]});
        next["activation"] = activation("Restart Live and run activate after rollback.");
        next["lastAction"] = "rollback".into();
        write_owner_json(&paths.receipt, &next)?;
        let finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"rollback","state":"completed","generation":next["generation"]}),
        );
        result["recovery"]["rollbackAvailable"] = true.into();
        result["recovery"]["quarantine"] = json!(failed);
        result["verification"] = json!({"artifactSha256":next["artifactSha256"],"remoteScript":verified,"journalFinalized":finalized});
        finish(result, &next);
        Ok(())
    })();
    if let Err(error) = operation {
        let restored_remote = (|| -> Result<(), LiveError> {
            if paths.remote.exists() {
                move_remote_folder(&paths.remote, p(&previous["remoteBackup"]))?;
            }
            if failed.exists() {
                move_remote_folder(&failed, &paths.remote)?;
            }
            Ok(())
        })();
        let restored_config = (|| -> Result<(), LiveError> {
            write_config(&paths.config, &receipt["config"], paths.config.exists())?;
            if json!(file_digest(&paths.config)?) != receipt["configSha256"] {
                return Err(fail("active bridge configuration was not restored exactly"));
            }
            Ok(())
        })();
        if restored_remote.is_err() || restored_config.is_err() {
            return Err(fail("rollback failed and active-generation compensation was incomplete"));
        }
        return Err(error);
    }
    Ok(())
}
fn remove_retained(state: &Path, items: Vec<Value>) -> Result<Vec<Value>, LiveError> {
    let mut retained = Vec::new();
    for item in items {
        let path = p(&item);
        if !contained(state, path) {
            retained.push(item);
            continue;
        }
        assert_no_linked_ancestors(path)?;
        if path.exists() && !try_remove(path, lstat(path)?.is_dir()) {
            retained.push(item);
        }
    }
    Ok(retained)
}
fn uninstall(o: &LifecycleOptions, paths: &Paths, receipt: &Value, result: &mut Value) -> Result<(), LiveError> {
    if o.purge_secret && receipt["secretCreatedByLifecycle"] != true {
        return Err(fail("refusing to purge a secret not created by this lifecycle"));
    }
    if receipt["status"] == "uninstalled" {
        let preserved = retention(receipt, "preserved");
        let mut pending = retention(receipt, "pendingCleanup");
        if o.purge_secret && paths.secret.exists() {
            read_secret_file(&paths.secret)?;
            let staged = quarantine_path(&o.state_directory, "uninstall-delete-secret")?;
            rename(&paths.secret, &staged)?;
            pending.push(json!(staged));
        }
        let pending = remove_retained(&o.state_directory, pending)?;
        let mut next = receipt.clone();
        next["generation"] = next_generation(receipt);
        next["secretCreatedByLifecycle"] = if o.purge_secret { false.into() } else { receipt["secretCreatedByLifecycle"].clone() };
        next["retained"] = json!({"pendingCleanup":pending,"preserved":preserved});
        next["lastAction"] = "uninstall".into();
        write_owner_json(&paths.receipt, &next)?;
        let finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"uninstall","state":"completed","generation":next["generation"],"pendingCleanup":pending,"preserved":preserved}),
        );
        result["applied"] = true.into();
        result["state"] = "completed".into();
        recovery_quarantine(result, &preserved, &pending);
        result["verification"] = json!({"alreadyUninstalled":true,"pendingCleanup":pending,"preserved":preserved,"secretPurged":o.purge_secret,"journalFinalized":finalized});
        result["instructions"] = json!(["Inspect preserved quarantine content; remove the npm package only after clients are updated."]);
        complete_steps(result);
        return Ok(());
    }
    let drift = verify_files(&paths.remote, &receipt["remoteFiles"])?;
    let config_matches = config_valid(receipt)?;
    let remote_staged = if paths.remote.exists() {
        Some(quarantine_path(&o.state_directory, if drift["valid"] == true { "uninstall-delete-remote" } else { "uninstall-preserved" })?)
    } else {
        None
    };
    let config_staged = if config_matches { Some(quarantine_path(&o.state_directory, "uninstall-delete-config")?) } else { None };
    let secret_staged =
        if o.purge_secret && paths.secret.exists() { Some(quarantine_path(&o.state_directory, "uninstall-delete-secret")?) } else { None };
    let previous = &receipt["previous"];
    let previous_drift = if previous.is_object() && p(&previous["remoteBackup"]).exists() {
        verify_files(p(&previous["remoteBackup"]), &previous["remoteFiles"])?
    } else {
        Value::Null
    };
    let mut committed = false;
    let operation = (|| -> Result<(), LiveError> {
        if secret_staged.is_some() {
            read_secret_file(&paths.secret)?;
        }
        if let Some(path) = &remote_staged {
            move_remote_folder(&paths.remote, path)?;
        }
        if let Some(path) = &config_staged {
            rename(&paths.config, path)?;
        }
        if let Some(path) = &secret_staged {
            rename(&paths.secret, path)?;
        }
        let preserved = unique(
            retention(receipt, "preserved")
                .into_iter()
                .chain(remote_staged.as_ref().filter(|_| drift["valid"] != true).map(|v| json!(v)))
                .chain(if previous_drift.is_object() && previous_drift["valid"] != true {
                    Some(previous["remoteBackup"].clone())
                } else {
                    None
                }),
        );
        let pending = unique(
            retention(receipt, "pendingCleanup")
                .into_iter()
                .chain(remote_staged.as_ref().filter(|_| drift["valid"] == true).map(|v| json!(v)))
                .chain(config_staged.as_ref().map(|v| json!(v)))
                .chain(secret_staged.as_ref().map(|v| json!(v)))
                .chain(if previous_drift["valid"] == true { Some(previous["remoteBackup"].clone()) } else { None }),
        );
        let mut next = receipt.clone();
        next["status"] = "uninstalled".into();
        next["generation"] = next_generation(receipt);
        next["secretCreatedByLifecycle"] = if o.purge_secret { false.into() } else { receipt["secretCreatedByLifecycle"].clone() };
        next["previous"] = Value::Null;
        next["retained"] = json!({"pendingCleanup":pending,"preserved":preserved});
        next["activation"] = json!({"required":false,"realLiveVerified":false,"provenance":"unavailable","remediation":"Remove the npm package separately after clients stop using it."});
        next["lastAction"] = "uninstall".into();
        write_owner_json(&paths.receipt, &next)?;
        committed = true;
        let pending = remove_retained(&o.state_directory, pending)?;
        next["retained"] = json!({"pendingCleanup":pending,"preserved":preserved});
        write_owner_json(&paths.receipt, &next)?;
        let quarantine = preserved.first().or_else(|| pending.first());
        let finalized = try_write_owner_json(
            &paths.journal,
            &json!({"version":1,"action":"uninstall","state":"completed","generation":next["generation"],"quarantine":quarantine,"pendingCleanup":pending,"preserved":preserved}),
        );
        result["applied"] = true.into();
        result["state"] = "completed".into();
        recovery_quarantine(result, &preserved, &pending);
        result["verification"] = json!({"priorDrift":drift,"priorGenerationDrift":previous_drift,"remoteRemoved":!paths.remote.exists(),"configRemoved":config_matches,"configPreservedBecauseModified":paths.config.exists(),"secretPurged":o.purge_secret,"secretPreserved":!o.purge_secret,"pendingCleanup":pending,"preserved":preserved,"journalFinalized":finalized});
        result["instructions"] = json!([
            "Restart Live to unload the removed Control Surface.",
            "Inspect preserved quarantine content, then remove the npm package only after every client configuration is updated."
        ]);
        complete_steps(result);
        Ok(())
    })();
    if let Err(error) = operation {
        if !committed {
            if let Some(path) = secret_staged.as_ref().filter(|p| p.exists() && !paths.secret.exists()) {
                rename(path, &paths.secret)?;
            }
            if let Some(path) = config_staged.as_ref().filter(|p| p.exists() && !paths.config.exists()) {
                rename(path, &paths.config)?;
            }
            if let Some(path) = remote_staged.as_ref().filter(|p| p.exists() && !paths.remote.exists()) {
                move_remote_folder(path, &paths.remote)?;
            }
        }
        return Err(error);
    }
    Ok(())
}
