//! A real JavaScript bridge installation (Kumi 1.7.5's lifecycle) → native migration, with the existing receipt and files untouched by fixture rebinding.
#[path = "support/lifecycle_fixture.rs"]
mod fixture;
use ableton_mcp_server::{delivery::*, lifecycle::*};
use fixture::*;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
/// The published Kumi 1.7.5 bundle, the last JavaScript release, unpacked once: KUMI_LEGACY_APP names
/// an unpacked copy; otherwise it's downloaded, checked against its SHA-256 and kept in the build's
/// temporary folder.
fn legacy_app() -> PathBuf {
    static APP: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    APP.get_or_init(|| {
        if let Some(app) = std::env::var_os("KUMI_LEGACY_APP") {
            return PathBuf::from(app);
        }
        const SHA256: &str = "1e932a401e88a1e6d3985d1f21c2c6af11883887675d5ae1de291dbb75385795";
        let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("kumi-1.7.5");
        let lifecycle = |app: &Path| app.join("apps/mcp-server/dist/src/lifecycle.js").is_file();
        if lifecycle(&cache) {
            return cache;
        }
        let staging = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let archive = staging.path().join("kumi.tar.gz");
        let fetched = Command::new("curl")
            .args(["--fail", "--silent", "--show-error", "--location", "--retry", "3", "--output"])
            .arg(&archive)
            .arg("https://github.com/user1303836/kumi/releases/download/v1.7.5/kumi.tar.gz")
            .status()
            .expect("curl fetches the Kumi 1.7.5 bundle (or set KUMI_LEGACY_APP to an unpacked copy)");
        assert!(fetched.success(), "couldn't download the Kumi 1.7.5 bundle; set KUMI_LEGACY_APP to an unpacked copy");
        assert_eq!(sha(fs::read(&archive).unwrap()), SHA256, "the Kumi 1.7.5 bundle isn't the published one");
        let unpacked = staging.path().join("app");
        fs::create_dir_all(&unpacked).unwrap();
        // Windows' own tar: Git's GNU tar reads "D:\…" as a remote host.
        let tar = if cfg!(windows) {
            PathBuf::from(std::env::var_os("SystemRoot").unwrap_or("C:\\Windows".into())).join("System32\\tar.exe")
        } else {
            "tar".into()
        };
        assert!(Command::new(tar).arg("-xzf").arg(&archive).arg("-C").arg(&unpacked).status().unwrap().success());
        assert!(lifecycle(&unpacked), "the Kumi 1.7.5 bundle has no bridge lifecycle");
        // Another test process may have put its copy in place first. When the move fails otherwise (an
        // interrupted run's copy is in the way, or Windows is still scanning the new files), this copy
        // serves where it is.
        if fs::rename(&unpacked, &cache).is_ok() || lifecycle(&cache) {
            return cache;
        }
        staging.keep().join("app")
    })
    .clone()
}
async fn bind_retrying(port: u16) -> tokio::net::TcpListener {
    for _ in 0..50 {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            Err(error) => panic!("{error}"),
        }
    }
    panic!("port {port} stayed in use")
}
fn old_install(root: &Path, custom: bool) -> (LifecycleOptions, Value) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new("node")
        .arg("crates/ableton-mcp-server/tests/support/legacy_install.mjs")
        .arg(json!({"root":root,"version":"1.0.73","custom":custom,"bundle":legacy_app()}).to_string())
        .current_dir(workspace)
        .output()
        .expect("Node is required to validate source-to-native upgrade");
    assert!(out.status.success(), "source installation failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["installed"]["state"], "completed");
    let mut options: LifecycleOptions = serde_json::from_value(v["options"].clone()).unwrap();
    options.enable_bridge_diagnostics = false;
    (options, v["receipt"].clone())
}
fn native_upgrade(root: &Path, old: &LifecycleOptions, version: &str) -> LifecycleOptions {
    let candidate = root.join(format!("native-{version}"));
    fs::create_dir_all(&candidate).unwrap();
    let (package, artifact, hash) = package(&candidate, version, "native");
    LifecycleOptions {
        action: "upgrade".into(),
        package_root: package,
        artifact_path: Some(artifact),
        artifact_sha256: Some(hash),
        ..old.clone()
    }
}
fn receipt(o: &LifecycleOptions) -> Value {
    read(o.state_directory.join("install-receipt.json"))
}
fn files(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(at).unwrap() {
            let p = item.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(p.strip_prefix(root).unwrap().into(), fs::read(p).unwrap());
            }
        }
    }
    let mut out = Default::default();
    walk(root, root, &mut out);
    out
}
#[tokio::test(flavor = "current_thread")]
async fn actual_node_install_migrates_at_same_version_and_rolls_back_exactly() {
    for custom in [false, true] {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let (old, prior) = old_install(&root, custom);
        let config = Path::new(prior["configPath"].as_str().unwrap());
        let secret = Path::new(prior["secretPath"].as_str().unwrap());
        let remote = Path::new(prior["remoteScriptDirectory"].as_str().unwrap());
        let before_config = fs::read(config).unwrap();
        let before_secret = fs::read(secret).unwrap();
        let before_remote = files(remote);
        let unrelated = old.state_directory.join("user-history.json");
        fs::write(&unrelated, "owned user history").unwrap();
        let upgrade = native_upgrade(&root, &old, "1.0.73");
        let result = run_lifecycle(&upgrade).await.unwrap();
        assert_eq!(result["state"], "completed");
        let migrated = receipt(&old);
        assert_eq!(migrated["packageVersion"], prior["packageVersion"]);
        assert_eq!(migrated["generation"], 2);
        assert_eq!(migrated["previous"]["artifactSha256"], prior["artifactSha256"]);
        assert_eq!(migrated["config"]["bridge"], prior["config"]["bridge"]);
        assert_eq!(migrated["config"]["server"]["command"], json!(native_entrypoint(&upgrade.package_root)));
        assert_eq!(migrated["config"]["server"]["args"], json!(["--config", config]));
        for key in
            ["stateDirectory", "remoteScriptsDirectory", "remoteScriptDirectory", "configPath", "secretPath", "secretCreatedByLifecycle"]
        {
            assert_eq!(migrated[key], prior[key], "preserve {key}");
        }
        assert_eq!(fs::read(secret).unwrap(), before_secret);
        assert_eq!(files(Path::new(migrated["previous"]["remoteBackup"].as_str().unwrap())), before_remote);
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "owned user history");
        assert!(config.is_file() && secret.is_file());
        assert_eq!(secret_permissions(config), SecretPermissions::OwnerOnly);
        assert_eq!(secret_permissions(secret), SecretPermissions::OwnerOnly);
        let status = run_lifecycle(&LifecycleOptions { action: "status".into(), ..upgrade.clone() }).await.unwrap();
        assert_eq!(status["verification"]["installationIntegrityValid"], true);
        let duplicate = run_lifecycle(&upgrade).await.unwrap_err();
        assert!(duplicate.message().contains("strictly newer"));
        let rollback = run_lifecycle(&LifecycleOptions { action: "rollback".into(), ..upgrade.clone() }).await.unwrap();
        assert_eq!(rollback["state"], "completed");
        assert_eq!(fs::read(config).unwrap(), before_config);
        assert_eq!(fs::read(secret).unwrap(), before_secret);
        assert_eq!(files(remote), before_remote);
        let restored = receipt(&old);
        assert_eq!(restored["config"], prior["config"]);
        assert_eq!(restored["packageRoot"], prior["packageRoot"]);
        assert_eq!(restored["artifactSha256"], prior["artifactSha256"]);
        assert_eq!(restored["generation"], 3);
        let status = run_lifecycle(&LifecycleOptions { action: "status".into(), ..old.clone() }).await.unwrap();
        assert_eq!(status["verification"]["installationIntegrityValid"], true);
        // The retained native generation can be restored after an owner-requested rollback.
        run_lifecycle(&LifecycleOptions { action: "rollback".into(), ..upgrade.clone() }).await.unwrap();
        assert_eq!(receipt(&old)["config"], migrated["config"]);
        assert_eq!(fs::read(secret).unwrap(), before_secret);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_install_migration_failures_restore_every_owned_file_and_refuse_drift() {
    for point in ["before-remote", "after-remote", "before-receipt"] {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let (old, prior) = old_install(&root, true);
        let config = Path::new(prior["configPath"].as_str().unwrap());
        let secret = Path::new(prior["secretPath"].as_str().unwrap());
        let remote = Path::new(prior["remoteScriptDirectory"].as_str().unwrap());
        let before_config = fs::read(config).unwrap();
        let before_secret = fs::read(secret).unwrap();
        let before_remote = files(remote);
        let mut upgrade = native_upgrade(&root, &old, "1.0.73");
        upgrade.fault_at = Some(point.into());
        assert!(run_lifecycle(&upgrade).await.unwrap_err().message().contains("injected lifecycle failure"));
        assert_eq!(receipt(&old), prior);
        assert_eq!(fs::read(config).unwrap(), before_config);
        assert_eq!(fs::read(secret).unwrap(), before_secret);
        assert_eq!(files(remote), before_remote);
        assert!(!old.state_directory.join("lifecycle.lock").exists());
        assert_eq!(read(old.state_directory.join("lifecycle-journal.json"))["state"], "failed-rolled-back");
        upgrade.fault_at = None;
        run_lifecycle(&upgrade).await.unwrap();
        assert_eq!(receipt(&old)["generation"], 2);
    }
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let (old, prior) = old_install(&root, false);
    let mut upgrade = native_upgrade(&root, &old, "1.0.73");
    upgrade.confirm_live_stopped = false;
    assert!(run_lifecycle(&upgrade).await.unwrap_err().message().contains("confirm-live-stopped"));
    assert_eq!(receipt(&old), prior);
    upgrade.confirm_live_stopped = true;
    let downgrade = native_upgrade(&root, &old, "1.0.72");
    assert!(run_lifecycle(&downgrade).await.unwrap_err().message().contains("strictly newer"));
    assert_eq!(receipt(&old), prior);
    let legacy_entry = old.package_root.join("dist/src/cli.js");
    let original = fs::read(&legacy_entry).unwrap();
    fs::write(&legacy_entry, "changed installed package").unwrap();
    assert!(run_lifecycle(&upgrade).await.is_err());
    assert_eq!(receipt(&old), prior);
    fs::write(&legacy_entry, original).unwrap();
    let mut forged = prior.clone();
    forged["releaseManifestSha256"] = json!("0".repeat(64));
    write(old.state_directory.join("install-receipt.json"), &forged);
    let failure = run_lifecycle(&upgrade).await.unwrap_err();
    assert!(failure.message().contains("strictly newer"), "{failure}");
    assert_eq!(receipt(&old), forged);
}

#[tokio::test(flavor = "current_thread")]
async fn migrated_activation_uses_preserved_port_secret_and_authenticated_discovery() {
    use ableton_mcp_server::registry::{canonical_json, live_registry_hash, WIRE_CANONICAL_LIMITS};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    fn sign(mut value: Value, secret: &str) -> Value {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(canonical_json(&value, &WIRE_CANONICAL_LIMITS).unwrap().as_bytes());
        value["mac"] = json!(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()));
        value
    }
    fn frame(id: &str, result: Value, secret: &str) -> Value {
        sign(
            json!({"version":"ableton-loopback/v1","id":id,"ok":true,"bridgeEpoch":"legacy-native-migration-epoch","connectionChallenge":"legacy-native-migration-challenge","result":result}),
            secret,
        )
    }
    tokio::task::LocalSet::new().run_until(async{
  let folder=tempfile::tempdir().unwrap();let root=folder.path().canonicalize().unwrap();let(old,prior)=old_install(&root,true);let upgrade=native_upgrade(&root,&old,"1.0.73");run_lifecycle(&upgrade).await.unwrap();
  let secret=read_secret_file(Path::new(prior["secretPath"].as_str().unwrap())).unwrap();let port=prior["config"]["bridge"]["port"].as_u64().unwrap()as u16;
  for provenance in ["fake-live","real-live"] {
   // The last round's listener may still be letting go of the port.
   let listener=bind_retrying(port).await;let secret=secret.clone();let peer=tokio::task::spawn_local(async move{
    let(stream,_)=listener.accept().await.unwrap();let(read,mut write)=stream.into_split();write.write_all(format!("{}\n",frame("hello",json!({"protocol":"ableton-live/v1","registryHash":live_registry_hash(),"maxDeadlineMs":60000}),&secret)).as_bytes()).await.unwrap();let mut lines=BufReader::new(read).lines();let mut methods=vec![];
    while let Some(line)=lines.next_line().await.unwrap(){let request:Value=serde_json::from_str(&line).unwrap();let mut unsigned=request.clone();let supplied=unsigned.as_object_mut().unwrap().shift_remove("mac").unwrap();assert_eq!(sign(unsigned,&secret)["mac"],supplied);let method=request["method"].as_str().unwrap();methods.push(method.to_string());let result=if method=="status"{json!({"connected":true,"adapter":"remote-script","epoch":1,"protocol":"ableton-live/v1","capabilities":[],"registryHash":live_registry_hash(),"operations":["status","snapshot","discover","get","reconnect","session.playback"],"provenance":provenance})}else{assert_eq!(method,"discover");let kind=&request["args"]["kind"];if kind=="session_playback"{json!({"ref":"live:1:set:1","epoch":1,"revision":"1","transport":{"playing":false,"arrangementRecord":false,"sessionRecord":false,"position":0,"launchQuantization":{"raw":0,"normalized":"none"},"loop":{"enabled":false,"start":0,"length":4},"punchIn":false,"punchOut":false,"metronome":false,"countIn":0},"firedTargets":[],"playingTargets":[]})}else{let items=if kind=="scene"{json!([{"ref":"live:1:scene:1"}])}else if kind=="track"{json!([{"ref":"live:1:track:1"}])}else{json!([])};json!({"epoch":1,"kind":kind,"items":items,"truncated":false,"revision":"1"})}};write.write_all(format!("{}\n",frame(request["id"].as_str().unwrap(),result,&secret)).as_bytes()).await.unwrap();}methods
   });
   let activation=run_lifecycle(&LifecycleOptions{action:"activate".into(),..upgrade.clone()}).await.unwrap();assert_eq!(activation["verification"]["authenticatedReachable"],true);assert_eq!(activation["verification"]["liveConnected"],provenance=="real-live");assert_eq!(activation["state"],if provenance=="real-live"{"completed"}else{"activation-required"});assert_eq!(receipt(&old)["activation"]["realLiveVerified"],provenance=="real-live");let methods=peer.await.unwrap();assert_eq!(methods.iter().filter(|m|m.as_str()=="discover").count(),5);assert!(methods.iter().all(|m|m=="status"||m=="discover"));
  }
 }).await;
}
