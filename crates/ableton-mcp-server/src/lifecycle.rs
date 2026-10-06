//! Receipt-bound native installation, with strict legacy release compatibility.
use crate::{delivery::*, live::LiveError};
use serde_json::{json, Value};
#[path = "lifecycle_fs.rs"]
mod fs;
#[path = "lifecycle_release.rs"]
mod release;
pub use fs::assert_no_linked_ancestors;
use fs::*;
pub use release::{verify_artifact_binding, verify_release_package, verify_retained_package, ReleaseEvidence};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[path = "lifecycle_actions.rs"]
mod actions;
pub const LIFECYCLE_RECEIPT_VERSION: u8 = 1;
pub const LIFECYCLE_ACTIONS: &[&str] = &["install", "activate", "upgrade", "repair", "rollback", "uninstall", "status"];
pub type LifecycleReceipt = Value;
pub type LifecycleResult = Value;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleOptions {
    pub action: String,
    pub package_root: PathBuf,
    pub state_directory: PathBuf,
    pub remote_scripts_directory: PathBuf,
    pub artifact_path: Option<PathBuf>,
    pub artifact_sha256: Option<String>,
    pub config_path: Option<PathBuf>,
    pub secret_path: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<f64>,
    pub realtime_port: Option<f64>,
    pub timeout_ms: Option<f64>,
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub confirm_live_stopped: bool,
    #[serde(default)]
    pub purge_secret: bool,
    #[serde(default)]
    pub enable_bridge_diagnostics: bool,
    #[serde(default)]
    pub allow_dirty_private_build: bool,
    /// Deterministic failure injection for compensation tests; never exposed by the CLI.
    pub fault_at: Option<String>,
}
struct Paths {
    receipt: PathBuf,
    journal: PathBuf,
    remote: PathBuf,
    config: PathBuf,
    secret: PathBuf,
    diagnostics: PathBuf,
}
fn expected_paths(o: &LifecycleOptions) -> Result<Paths, LiveError> {
    for (path, label) in [
        (&o.package_root, "package root"),
        (&o.state_directory, "state directory"),
        (&o.remote_scripts_directory, "Remote Scripts directory"),
    ] {
        validate_absolute_path(path, label)?;
    }
    let paths = Paths {
        receipt: o.state_directory.join("install-receipt.json"),
        journal: o.state_directory.join("lifecycle-journal.json"),
        remote: o.remote_scripts_directory.join(REMOTE_SCRIPT_PACKAGE),
        config: o.config_path.clone().unwrap_or_else(|| o.state_directory.join("bridge-config.json")),
        secret: o.secret_path.clone().unwrap_or_else(|| o.state_directory.join("bridge.secret")),
        diagnostics: o.state_directory.join("bridge-diagnostics.log"),
    };
    for (path, label) in [(&paths.config, "configuration path"), (&paths.secret, "secret path"), (&paths.diagnostics, "diagnostics path")] {
        validate_absolute_path(path, label)?;
    }
    Ok(paths)
}
fn planned_steps(action: &str) -> Value {
    let steps: &[(&str, &str)] = match action {
        "install" => &[
            ("preflight", "read-only validation"),
            ("secret", "creates owner-only secret"),
            ("config", "creates owner-only bridge config"),
            ("remote-script", "installs managed Live Remote Script"),
            ("receipt", "writes owner-only lifecycle receipt"),
        ],
        "activate" => &[
            ("diagnostics", "authenticated read-only Live discovery"),
            ("receipt", "records activation evidence only for real-live provenance"),
        ],
        "upgrade" => &[
            ("preflight", "verifies current and candidate manifests"),
            ("backup", "retains current generation"),
            ("config", "switches exact host entrypoint"),
            ("remote-script", "atomically replaces managed bridge"),
            ("receipt", "records rollback generation"),
        ],
        "repair" => &[
            ("inspect", "compares receipt hashes and permissions"),
            ("quarantine", "preserves drift before replacement"),
            ("restore", "restores managed payload only"),
            ("verify", "rehashes repaired state"),
        ],
        "rollback" => &[
            ("preflight", "verifies retained previous generation"),
            ("swap", "restores prior Remote Script and configuration"),
            ("verify", "rehashes restored generation"),
        ],
        "uninstall" => &[
            ("preflight", "verifies receipt ownership"),
            ("remote-script", "removes exact managed files or quarantines drift"),
            ("config", "removes only digest-matching managed config"),
            ("secret", "preserves secret unless purge is explicit"),
            ("receipt", "records uninstalled state"),
        ],
        _ => &[("inspect", "read-only lifecycle and drift report")],
    };
    json!(steps.iter().map(|(id, impact)| json!({"id":id,"impact":impact,"status":"planned"})).collect::<Vec<_>>())
}
fn complete_steps(result: &mut Value) {
    for step in result["steps"].as_array_mut().unwrap() {
        step["status"] = "completed".into();
    }
}
fn fault(o: &LifecycleOptions, point: &str) -> Result<(), LiveError> {
    if o.fault_at.as_deref() == Some(point) {
        Err(fail(format!("injected lifecycle failure at {point}")))
    } else {
        Ok(())
    }
}
fn p(value: &Value) -> &Path {
    Path::new(value.as_str().unwrap_or(""))
}
fn active(receipt: &Value) -> bool {
    receipt.is_object() && receipt["status"] != "uninstalled"
}
fn permissions(path: &Path) -> Value {
    if path.exists() {
        json!(secret_permissions(path))
    } else {
        json!("unavailable")
    }
}
fn next_generation(receipt: &Value) -> Value {
    json!((receipt["generation"].as_f64().unwrap_or(0.) + 1.) as u64)
}
fn package_valid(receipt: &Value, allow_dirty: bool) -> bool {
    // The installed generation runs its own bridge: after a rollback, an older one with its own registry.
    verify_retained_package(p(&receipt["packageRoot"]), allow_dirty).is_ok_and(|e| {
        json!(e.manifest_sha256) == receipt["releaseManifestSha256"] && e.manifest["protocol"]["registryHash"] == receipt["registryHash"]
    })
}
fn config_valid(receipt: &Value) -> Result<bool, LiveError> {
    let path = p(&receipt["configPath"]);
    Ok(path.exists() && json!(file_digest(path)?) == receipt["configSha256"] && owner_only(path))
}
fn diagnostics_path(receipt: &Value) -> Option<&Path> {
    receipt["config"]["bridge"]["diagnostics"].get("path").map(p)
}
fn retention(receipt: &Value, key: &str) -> Vec<Value> {
    receipt["retained"][key].as_array().cloned().unwrap_or_default()
}
fn recovery_quarantine(result: &mut Value, preserved: &[Value], pending: &[Value]) {
    if let Some(path) = preserved.first().or_else(|| pending.first()) {
        result["recovery"]["quarantine"] = path.clone();
    }
}
fn activation(remediation: &str) -> Value {
    json!({"required":true,"realLiveVerified":false,"provenance":"unavailable","remediation":remediation})
}
async fn verify_ports(host: &str, control: f64, realtime: f64) -> Result<(), LiveError> {
    if !["127.0.0.1", "::1"].contains(&host) {
        return Err(fail("lifecycle bridge host must be an exact loopback address"));
    }
    if [control, realtime].iter().any(|v| !v.is_finite() || v.fract() != 0. || *v < 1. || *v > 65535.) || control == realtime {
        return Err(fail("control and realtime ports must be distinct values from 1 to 65535"));
    }
    for (label, port) in [("control", control), ("realtime", realtime)] {
        if tokio::net::TcpListener::bind((host, port as u16)).await.is_err() {
            return Err(fail(format!("{label} port {port} is occupied")));
        }
    }
    Ok(())
}
pub async fn run_lifecycle(o: &LifecycleOptions) -> Result<LifecycleResult, LiveError> {
    if !LIFECYCLE_ACTIONS.contains(&o.action.as_str()) {
        return Err(fail("unsupported lifecycle action"));
    }
    if o.enable_bridge_diagnostics && o.action != "install" {
        return Err(fail("bridge diagnostics can be selected only during lifecycle install"));
    }
    let paths = expected_paths(o)?;
    let mut result = json!({"version":"ableton-mcp-lifecycle/v1","action":o.action,"applied":false,"state":"planned","receiptPath":paths.receipt,"restartRequired":(["install","upgrade","rollback","uninstall"].contains(&o.action.as_str())),"steps":planned_steps(&o.action),"verification":{},"recovery":{"journalPath":paths.journal,"rollbackAvailable":false},"instructions":[]});
    for path in [
        &o.package_root,
        &o.state_directory,
        &o.remote_scripts_directory,
        &paths.config,
        &paths.secret,
        &paths.remote,
        &paths.receipt,
        &paths.journal,
    ]
    .into_iter()
    .chain(o.artifact_path.iter())
    {
        assert_no_linked_ancestors(path)?;
    }
    if !o.remote_scripts_directory.is_dir() {
        return Err(fail("Remote Scripts directory must already exist and be explicitly selected"));
    }
    let receipt = if paths.receipt.exists() { parse_receipt(&paths.receipt)? } else { Value::Null };
    let evidence = if ["install", "upgrade", "repair"].contains(&o.action.as_str()) {
        Some(verify_release_package(&o.package_root, o.allow_dirty_private_build)?)
    } else {
        None
    };
    if ["install", "upgrade"].contains(&o.action.as_str())
        && evidence.as_ref().unwrap().manifest["schema"] != "ableton-mcp-native-release/v1"
    {
        return Err(fail("install and upgrade require a current ableton-mcp-native-release/v1 MIT candidate; older releases are receipt-bound compatibility only"));
    }
    let artifact_sha256 = if ["install", "upgrade"].contains(&o.action.as_str()) {
        verify_artifact_binding(
            o.artifact_path.as_deref(),
            o.artifact_sha256.as_deref(),
            &o.package_root,
            &evidence.as_ref().unwrap().manifest_sha256,
        )?
    } else {
        o.artifact_sha256.clone().unwrap_or_default()
    };
    if o.apply && ["install", "upgrade", "rollback", "uninstall"].contains(&o.action.as_str()) && !o.confirm_live_stopped {
        return Err(fail("explicit --confirm-live-stopped is required; the lifecycle never kills Ableton Live"));
    }
    if receipt["previous"].is_object() && !contained(p(&receipt["stateDirectory"]), p(&receipt["previous"]["remoteBackup"])) {
        return Err(fail("receipt rollback path escapes owner state"));
    }
    if !receipt.is_null()
        && [
            (&o.remote_scripts_directory, "remoteScriptsDirectory"),
            (&o.state_directory, "stateDirectory"),
            (&paths.remote, "remoteScriptDirectory"),
            (&paths.config, "configPath"),
            (&paths.secret, "secretPath"),
        ]
        .iter()
        .any(|(path, key)| json!(path) != receipt[*key])
    {
        return Err(fail("lifecycle paths do not match the owner receipt"));
    }
    if !receipt.is_null() && o.action == "repair" {
        let e = evidence.as_ref().unwrap();
        if crate::command::resolve(&o.package_root)? != crate::command::resolve(p(&receipt["packageRoot"]))?
            || json!(e.manifest_sha256) != receipt["releaseManifestSha256"]
            || e.manifest["package"]["version"] != receipt["packageVersion"]
            || e.manifest["protocol"]["registryHash"] != receipt["registryHash"]
        {
            return Err(fail("repair package root is not the exact receipt-bound generation; use upgrade for a different artifact"));
        }
    }
    if !o.apply && !["activate", "status"].contains(&o.action.as_str()) {
        let manifest = evidence.as_ref().map(|e| &e.manifest);
        result["verification"] = json!({"packageVersion":manifest.map(|m|&m["package"]["version"]).unwrap_or(&receipt["packageVersion"]),"sourceCommit":manifest.map(|m|&m["source"]["commit"]),"sourceDirty":manifest.map(|m|&m["source"]["dirty"]),"packageRootVerified":evidence.is_some(),"receiptPresent":!receipt.is_null(),"bridgeDiagnosticsRequested":o.enable_bridge_diagnostics,"portsRequireApplyTimeProbe":o.action=="install"});
        result["instructions"] = json!(["Review the plan, stop Ableton Live, then repeat with --apply and --confirm-live-stopped."]);
        return Ok(result);
    }
    if o.action == "status" {
        let drift = if active(&receipt) { verify_files(&paths.remote, &receipt["remoteFiles"])? } else { Value::Null };
        let config_valid = active(&receipt) && config_valid(&receipt)?;
        let diagnostics = diagnostics_path(&receipt);
        let diagnostics_valid = diagnostics.is_none_or(diagnostics_file_valid);
        let package_valid = active(&receipt) && package_valid(&receipt, o.allow_dirty_private_build);
        let secret_valid = active(&receipt) && p(&receipt["secretPath"]).exists() && owner_only(p(&receipt["secretPath"]));
        let integrity = drift["valid"] == true && config_valid && diagnostics_valid && package_valid && secret_valid;
        let summary = if receipt.is_null() {
            Value::Null
        } else {
            json!({"status":receipt["status"],"effectiveStatus":if receipt["status"]=="activated"&&!integrity{json!("installed-restart-required")}else{receipt["status"].clone()},"generation":receipt["generation"],"packageVersion":receipt["packageVersion"],"artifactSha256":receipt["artifactSha256"],"recordedActivation":receipt["activation"],"activationEvidenceScope":"historical-receipt-not-current-connectivity","retained":receipt.get("retained").cloned().unwrap_or_else(||json!({"pendingCleanup":[],"preserved":[]}))})
        };
        result["state"] = "completed".into();
        result["verification"] = json!({"receipt":summary,"installationIntegrityValid":integrity,"packageValid":package_valid,"configValid":config_valid,"bridgeDiagnostics":diagnostics.map(|path|json!({"path":path,"valid":diagnostics_valid,"permissions":permissions(path)})),"remoteScript":drift,"configPermissions":permissions(p(&receipt["configPath"])),"secretPermissions":permissions(p(&receipt["secretPath"])),"lifecycleLockPresent":o.state_directory.join("lifecycle.lock").exists()});
        result["recovery"]["rollbackAvailable"] =
            json!(receipt["previous"].is_object() && p(&receipt["previous"]["remoteBackup"]).exists());
        recovery_quarantine(&mut result, &retention(&receipt, "preserved"), &retention(&receipt, "pendingCleanup"));
        complete_steps(&mut result);
        return Ok(result);
    }
    if o.action == "activate" {
        if !active(&receipt) {
            return Err(fail("an installed lifecycle receipt is required for activation"));
        }
        ensure_owner_directory(&o.state_directory)?;
        let _lock = LifecycleLock::take(&o.state_directory)?;
        let current = parse_receipt(&paths.receipt)?;
        if current["generation"] != receipt["generation"] {
            return Err(fail("receipt generation changed before activation; retry from fresh status"));
        }
        let remote = verify_files(&paths.remote, &current["remoteFiles"])?;
        let valid = remote["valid"] == true
            && diagnostics_path(&current).is_none_or(diagnostics_file_valid)
            && config_valid(&current)?
            && owner_only(p(&current["secretPath"]))
            && package_valid(&current, o.allow_dirty_private_build);
        let report =
            if valid { diagnostics_async(Some(p(&current["packageRoot"])), Some(p(&current["configPath"]))).await } else { Value::Null };
        let activated = valid
            && report["liveConnected"] == true
            && report["provenance"] == "real-live"
            && report["registryHash"] == current["registryHash"];
        let mut next = current.clone();
        next["status"] = json!(if activated { "activated" } else { "installed-restart-required" });
        next["activation"] = json!({"required":!activated,"realLiveVerified":activated,"provenance":if activated{"real-live"}else{"unavailable"},"remediation":if activated{"none"}else if valid{"Restart Live, select AbletonMcpBridge as a Control Surface, then rerun activate."}else{"Run lifecycle repair and verify receipt-bound hashes before activation."}});
        next["lastAction"] = "activate".into();
        write_owner_json(&paths.receipt, &next)?;
        result["applied"] = true.into();
        result["state"] = json!(if activated { "completed" } else { "activation-required" });
        result["restartRequired"] = json!(!activated);
        result["verification"] = json!({"installationValid":valid,"remoteScript":remote,"authenticatedReachable":report.get("authenticatedReachable").unwrap_or(&Value::Bool(false)),"liveConnected":report.get("liveConnected").unwrap_or(&Value::Bool(false)),"provenance":report.get("provenance").cloned().unwrap_or_else(||json!("unavailable")),"registryHash":report["registryHash"],"expectedRegistryHash":current["registryHash"]});
        result["instructions"] = json!([if activated {
            json!("Real-Live activation, receipt-bound installation, and registry identity verified.")
        } else {
            next["activation"]["remediation"].clone()
        }]);
        complete_steps(&mut result);
        return Ok(result);
    }
    if o.apply && !["install", "activate", "status", "uninstall"].contains(&o.action.as_str()) && !active(&receipt) {
        return Err(fail("an active installation receipt is required"));
    }
    if o.apply && o.action == "rollback" && (!receipt["previous"].is_object() || !p(&receipt["previous"]["remoteBackup"]).exists()) {
        return Err(fail("no verified previous generation is available"));
    }
    if o.action == "install" {
        verify_ports(o.host.as_deref().unwrap_or("127.0.0.1"), o.port.unwrap_or(9765.), o.realtime_port.unwrap_or(9766.)).await?;
    }
    ensure_owner_directory(&o.state_directory)?;
    let _lock = LifecycleLock::take(&o.state_directory)?;
    let locked = if paths.receipt.exists() { parse_receipt(&paths.receipt)? } else { Value::Null };
    if locked["generation"] != receipt["generation"] || locked["status"] != receipt["status"] {
        return Err(fail("receipt changed before lifecycle lock acquisition; retry from fresh status"));
    }
    write_owner_json(
        &paths.journal,
        &json!({"version":1,"action":o.action,"state":"applying","receiptGeneration":receipt["generation"],"recovery":"receipt is authoritative; inspect quarantine before retrying"}),
    )?;
    let operation = actions::apply(o, &paths, &receipt, evidence.as_ref(), &artifact_sha256, &mut result);
    if let Err(error) = operation {
        finalize_failed_journal(&paths.journal, &o.action, receipt["generation"].clone(), &error);
        return Err(error);
    }
    Ok(result)
}
