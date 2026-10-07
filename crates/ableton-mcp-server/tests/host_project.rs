#[path = "../../../tests/support/fixture_paths.rs"]
mod fixture_paths;
use ableton_mcp_server::{
    host::{McpHost, McpHostOptions, ToolCall},
    live::*,
    registry::live_registry_operations,
};
use base64::Engine;
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: RefCell<LiveStatus>,
    case: Value,
    calls: RefCell<Vec<Value>>,
    path: RefCell<Option<String>>,
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.borrow().clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.sim.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.sim.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        assert!(c.is_none());
        self.calls.borrow_mut().push(json!({"context":null,"request":r}));
        if let Some(fail) = self.case["fail"].as_str() {
            return Err(LiveError::error(fail));
        }
        let snapshot = self.sim.snapshot_async(c, r).await?;
        if let Some(path) = self.path.borrow().as_ref() {
            let mut value = serde_json::to_value(snapshot).unwrap();
            value["set"]["filePath"] = json!(path);
            return Ok(serde_json::from_value(value).unwrap());
        }
        Ok(snapshot)
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.sim.invoke_async(i, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get_async(r, c).await
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
/// The bridge version the golden files were recorded with; pinned so a version bump changes none of them.
const ORACLE_VERSION: &str = "1.0.74";
/// A host that reports `ORACLE_VERSION`.
fn pinned_host(adapter: Rc<dyn AsyncLiveAdapter>) -> McpHost {
    McpHost::new(adapter, McpHostOptions { server_version: Some(ORACLE_VERSION.into()), ..Default::default() }).unwrap()
}
fn adapter(case: Value) -> Rc<Adapter> {
    let sim = DeterministicLiveSimulator::new();
    let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
    status["operations"] = json!(live_registry_operations());
    if let Some(patch) = case["status"].as_object() {
        status.as_object_mut().unwrap().extend(patch.clone());
    }
    Rc::new(Adapter {
        sim,
        status: RefCell::new(serde_json::from_value(status).unwrap()),
        case,
        calls: RefCell::new(vec![]),
        path: RefCell::new(None),
    })
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("support/host_project_oracle.json")).unwrap()
}
fn parsed(mut v: Value) -> Value {
    if let Some(text) = v["result"]["content"][0]["text"].as_str() {
        if let Ok(value) = serde_json::from_str(text) {
            v["result"]["content"][0]["text"] = value;
        }
    }
    v
}
fn canonical(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Object(o) => {
            let mut keys: Vec<_> = o.keys().collect();
            keys.sort();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical(&o[k])))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => kumi_common::js::json::stringify(v),
    }
}
fn equal(actual: &Value, expected: &Value, label: &str) {
    let a = canonical(actual);
    let b = canonical(expected);
    if a != b {
        let i = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
        panic!(
            "{label}, difference byte {i}\nactual {}\nexpected {}",
            a.chars().skip(i.saturating_sub(80)).take(250).collect::<String>(),
            b.chars().skip(i.saturating_sub(80)).take(250).collect::<String>()
        );
    }
}
async fn call(host: &McpHost, tool: &str, args: Value) -> Value {
    parsed(
        host.dispatch_project_tool(&ToolCall { id: json!(1), name: tool.into(), arguments: Some(args), asynchronous: true }, None)
            .await
            .unwrap()
            .unwrap(),
    )
}
#[tokio::test]
async fn project_host_matches_source_validation_and_adapter_dispatch() {
    for (i, row) in fixture()["cases"].as_array().unwrap().iter().enumerate() {
        let adapter = adapter(row.clone());
        let host = pinned_host(adapter.clone());
        let actual = call(&host, row["tool"].as_str().unwrap(), row["args"].clone()).await;
        equal(&actual, &row["result"], &format!("{i} {} {}", row["tool"], row["args"]));
        equal(&json!(*adapter.calls.borrow()), &row["calls"], &format!("dispatch {i}"));
    }
}
#[tokio::test(flavor = "current_thread")]
async fn files_on_a_network_share_are_refused_before_anything_opens_them() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().canonicalize().unwrap().to_string_lossy().into_owned();
            // Windows canonicalizes to a \\?\ device path, which is refused itself and takes no "/" joins.
            let root = root.strip_prefix(r"\\?\").unwrap_or(&root).to_owned();
            std::fs::write(format!("{root}/kick.wav"), b"RIFF\x04\x00\x00\x00WAVE").unwrap();
            std::fs::write(format!("{root}/Song.als"), b"not gzip").unwrap();
            let host = pinned_host(adapter(json!({})));
            // "//" before a local path is that same path off Windows, as "\\?\" is on it: only the refusal keeps it out there.
            let local = if cfg!(windows) { format!(r"\\?\{root}") } else { format!("/{root}") };
            let shares = [local, r"\\host\share".into(), r"/\host\share".into(), r"\/host\share".into(), r"\??\UNC\host\share".into()];
            let said = |reply: &Value| reply.to_string();
            for share in &shares {
                for (file, folder) in [
                    (format!("{share}/kick.wav"), share.clone()),
                    (format!("{share}/kick.wav"), root.clone()),
                    (format!("{root}/kick.wav"), share.clone()),
                ] {
                    for (tool, args) in [
                        (
                            "live_audio_import_preview",
                            json!({"filePath":file,"allowedRoot":folder,"trackRef":"track:track-1","sceneIndex":0}),
                        ),
                        ("live_project_import", json!({"filePath":file,"allowedRoot":folder})),
                    ] {
                        let call = ToolCall { id: json!(1), name: tool.into(), arguments: Some(args), asynchronous: true };
                        let reply = host.dispatch_audio_import_tool(&call, None).await.unwrap().unwrap().unwrap();
                        assert!(said(&reply).contains("network share"), "{tool} {file} in {folder}: {reply}");
                    }
                }
                // An Arrangement audio clip is made from the file by Live itself.
                let args = json!({"action":"create","kind":"audio","trackRef":"track:track-1","position":0,"filePath":format!("{share}/kick.wav")});
                let reply = host.live_arrangement_clip_preview_async(&json!(1), &args).await;
                assert!(said(&reply).contains("network share"), "live_arrangement_clip {share}: {reply}");
                let set = format!("{share}/Song.als");
                for (path, folder) in [(set.clone(), share.clone()), (set, root.clone()), (format!("{root}/Song.als"), share.clone())] {
                    let reply = call(&host, "als_read", json!({"path":path,"allowedRoot":folder})).await;
                    assert!(said(&reply).contains("network share"), "als_read {path} in {folder}: {reply}");
                }
            }
        })
        .await;
}
fn substitute(value: &Value, root: &str) -> Value {
    match value {
        Value::String(s) => json!(s.replace("ROOT", root)),
        Value::Array(a) => json!(a.iter().map(|v| substitute(v, root)).collect::<Vec<_>>()),
        Value::Object(o) => json!(o.iter().map(|(k, v)| (k.clone(), substitute(v, root))).collect::<serde_json::Map<_, _>>()),
        _ => value.clone(),
    }
}
#[tokio::test]
async fn offline_project_host_source_artifacts_and_shared_profile_diff() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_paths::native_path(&std::fs::canonicalize(dir.path()).unwrap());
    let path = root.join("Test.als");
    for (name, key) in [("Test.als", "raw"), ("Changed.als", "changedRaw")] {
        std::fs::write(root.join(name), base64::engine::general_purpose::STANDARD.decode(f[key].as_str().unwrap()).unwrap()).unwrap();
    }
    let host = pinned_host(Rc::new(UnavailableLiveAdapter));
    for row in f["offline"].as_array().unwrap() {
        let args = if row.get("args").is_some() {
            substitute(&row["args"], root.to_str().unwrap())
        } else {
            let mut args = row["extra"].clone();
            args["path"] = json!(path);
            args["allowedRoot"] = json!(root);
            args
        };
        equal(
            &call(&host, row["tool"].as_str().unwrap(), args).await,
            &row["result"],
            &format!("offline {} {}", row["tool"], row["extra"]),
        );
    }
}
#[tokio::test]
async fn live_export_cache_and_guarded_backup_lifecycle() {
    let f = fixture();
    let adapter = adapter(json!({}));
    let host = pinned_host(adapter.clone());
    let first = call(&host, "live_project_snapshot_export", json!({"limit":1})).await;
    let cursor = first["result"]["content"][0]["text"]["page"]["nextCursor"].as_str().unwrap();
    let calls = adapter.calls.borrow().len();
    let second = call(&host, "live_project_snapshot_export", json!({"limit":1,"cursor":cursor})).await;
    assert_eq!(second["result"]["content"][0]["text"]["page"]["offset"], 1);
    assert_eq!(adapter.calls.borrow().len(), calls);
    adapter.status.borrow_mut().epoch = Some(2);
    call(&host, "live_project_snapshot_export", json!({"limit":1,"cursor":cursor})).await;
    assert_eq!(adapter.calls.borrow().len(), calls + 1);
    let dir = tempfile::tempdir().unwrap();
    let root = fixture_paths::native_path(&std::fs::canonicalize(dir.path()).unwrap());
    let path = root.join("Test.als");
    std::fs::write(&path, base64::engine::general_purpose::STANDARD.decode(f["raw"].as_str().unwrap()).unwrap()).unwrap();
    *adapter.path.borrow_mut() = Some(path.to_string_lossy().into_owned());
    let preview = call(&host, "live_project_backup_preview", json!({"confirmation":"backup","allowedRoot":root})).await;
    let preview = &preview["result"]["content"][0]["text"];
    assert_eq!(preview["impact"], "creates-verified-backup");
    let args = json!({"transactionId":preview["transactionId"],"confirmation":"apply","idempotencyKey":"backup-key"});
    let applied = call(&host, "live_project_backup_apply", args.clone()).await;
    let applied = &applied["result"]["content"][0]["text"];
    assert_eq!(applied["verified"], true, "{applied}");
    assert_eq!(std::fs::read(applied["backup"].as_str().unwrap()).unwrap(), std::fs::read(&path).unwrap());
    let replay = call(&host, "live_project_backup_apply", args.clone()).await;
    let replay = &replay["result"]["content"][0]["text"];
    assert_eq!(replay["idempotent"], true);
    assert_eq!(replay["backup"]["backup"], applied["backup"]); // Source replay returns the retained whole backup result.
    let mut other = args.clone();
    other["idempotencyKey"] = json!("other-key");
    let changed_key = call(&host, "live_project_backup_apply", other).await;
    assert_eq!(changed_key["result"]["isError"], true);
    let preview = call(&host, "live_project_backup_preview", json!({"confirmation":"backup","allowedRoot":root})).await;
    let args = json!({"transactionId":preview["result"]["content"][0]["text"]["transactionId"],"confirmation":"apply","idempotencyKey":"changed-key"});
    std::fs::write(&path, base64::engine::general_purpose::STANDARD.decode(f["changedRaw"].as_str().unwrap()).unwrap()).unwrap();
    let changed = call(&host, "live_project_backup_apply", args).await;
    assert!(changed["result"]["content"][0]["text"]["reason"].as_str().unwrap().contains("content changed"));
}
fn normalize_backup(value: &Value, root: &str, id: &str, key: &str) -> Value {
    if key == "mtimeMs" || key == "expiresAt" {
        return json!(0);
    }
    match value {
        Value::String(s) => {
            let s = fixture_paths::normalize_root(s, root, "ROOT").replace(id, "backup_ID");
            let s = if s.starts_with("ROOT/Test.backup-") && s.ends_with(".als") { "ROOT/BACKUP.als".into() } else { s };
            json!(s)
        }
        Value::Array(a) => json!(a.iter().map(|v| normalize_backup(v, root, id, "")).collect::<Vec<_>>()),
        Value::Object(o) => json!(o.iter().map(|(k, v)| (k.clone(), normalize_backup(v, root, id, k))).collect::<serde_json::Map<_, _>>()),
        _ => value.clone(),
    }
}
#[tokio::test]
async fn source_backup_complete_traces_preserve_cancellation_replay_uncertainty_and_path_fences() {
    let fixture = fixture();
    for row in fixture["backupTraces"].as_array().unwrap() {
        let mode = row["mode"].as_str().unwrap();
        if !["success", "uncertain", "path-changed"].contains(&mode) {
            continue;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = fixture_paths::native_path(&std::fs::canonicalize(directory.path()).unwrap());
        let path = root.join("Test.als");
        std::fs::write(&path, base64::engine::general_purpose::STANDARD.decode(fixture["raw"].as_str().unwrap()).unwrap()).unwrap();
        let adapter = adapter(json!({}));
        *adapter.path.borrow_mut() = Some(path.to_string_lossy().into_owned());
        let host = pinned_host(adapter.clone());
        let mut results = vec![];
        let preview = call(
            &host,
            "live_project_backup_preview",
            json!({"confirmation":"backup","allowedRoot":if mode=="uncertain"{"relative"}else{root.to_str().unwrap()}}),
        )
        .await;
        let id = preview["result"]["content"][0]["text"]["transactionId"].as_str().unwrap().to_owned();
        results.push(preview);
        let args = json!({"transactionId":id,"confirmation":"apply","idempotencyKey":"backup-key"});
        if mode == "path-changed" {
            *adapter.path.borrow_mut() = Some(root.join("other.als").to_string_lossy().into_owned());
        }
        if mode == "success" {
            let signal = kumi_common::abort::Signal::new();
            signal.cancel();
            results.push(parsed(host.live_project_backup_apply_async(&json!(1), &args, Some(&signal)).await));
        }
        results.push(call(&host, "live_project_backup_apply", args.clone()).await);
        results.push(call(&host, "live_project_backup_apply", args.clone()).await);
        let mut other = args;
        other["idempotencyKey"] = json!("other-key");
        results.push(call(&host, "live_project_backup_apply", other).await);
        equal(&normalize_backup(&json!(results), root.to_str().unwrap(), &id, ""), &row["results"], mode);
        equal(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{mode} dispatch"));
    }
}
