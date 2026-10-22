kumi_testkit::fixture_paths!();
use ableton_mcp_server::{
    host::{device_state::*, helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
};
use serde_json::{json, Value};
use std::{fs, path::Path, rc::Rc};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-device-state-oracle.json")).unwrap()
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn expand(v: &Value, root: &Path) -> Value {
    fixture_paths::map_strings(v, &|text| text.replace("$root", root.to_str().unwrap()))
}
fn clean(mut value: Value, root: &Path) -> Value {
    if let Some(text) = value["result"]["content"][0]["text"].as_str() {
        if let Ok(mut body) = serde_json::from_str::<Value>(text) {
            if body.get("transactionId").is_some() {
                body["transactionId"] = json!("$transaction");
            }
            if body.get("expiresAt").is_some() {
                body["expiresAt"] = json!("$time");
            }
            value["result"]["content"][0]["text"] = body;
        }
    }
    fixture_paths::map_strings(&value, &|text| fixture_paths::normalize_root(text, root.to_str().unwrap(), "$root"))
}
fn setup(root: &Path, file: &Value) {
    fs::write(root.join("saved.ableton-device-state.json"), serde_json::to_vec(file).unwrap()).unwrap();
    fs::write(root.join("invalid.json"), "{").unwrap();
    fs::write(root.join("wrong.json"), "{}").unwrap();
    fs::write(root.join("oversized.json"), vec![b' '; 256 * 1024 + 1]).unwrap();
    fs::create_dir(root.join("folder")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("folder"), root.join("linked-directory")).unwrap();
        std::os::unix::fs::symlink(root.join("saved.ableton-device-state.json"), root.join("linked.json")).unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(root.join("folder"), root.join("linked-directory")).unwrap();
        std::os::windows::fs::symlink_file(root.join("saved.ableton-device-state.json"), root.join("linked.json")).unwrap();
    }
}
#[tokio::test]
async fn device_state_file_host_validation_and_recall_workflows_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = fixture();
            for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
                let temp = tempfile::tempdir().unwrap();
                let root = fixture_paths::native_path(&fs::canonicalize(temp.path()).unwrap());
                setup(&root, &fixture["file"]);
                let sim = Rc::new(DeterministicLiveSimulator::new());
                let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
                if let Some(patch) = row["options"]["statusPatch"].as_object() {
                    for (key, value) in patch {
                        status[key] = value.clone();
                    }
                }
                let adapter = Rc::new(StatusAdapter { sim, status: serde_json::from_value(status).unwrap() });
                let host = McpHost::new(adapter, McpHostOptions::default()).unwrap();
                let args = expand(&row["args"], &root);
                let got = match row["tool"].as_str().unwrap() {
                    "save" => host.live_device_state_save_async(&json!(1), &args).await,
                    "preview" => host.live_device_state_recall_preview_async(&json!(1), &args).await,
                    "apply" => host.live_device_state_recall_apply_async(&json!(1), &args, None).await.unwrap(),
                    _ => unreachable!(),
                };
                same(&clean(got, &root), &row["result"], &format!("row {index} {row}"));
            }
            for row in fixture["workflows"].as_array().unwrap() {
                let temp = tempfile::tempdir().unwrap();
                let root = fixture_paths::native_path(&fs::canonicalize(temp.path()).unwrap());
                setup(&root, &fixture["file"]);
                let sim = Rc::new(DeterministicLiveSimulator::new());
                sim.simulate_external_edit(
                    &LiveRef(fixture["parameter"]["ref"].as_str().unwrap().into()),
                    "value",
                    fixture["parameter"]["min"].clone(),
                )
                .unwrap();
                let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
                let made = host.live_device_state_recall_preview_async(&json!(1), &expand(&row["args"], &root)).await;
                let body: Value = serde_json::from_str(made["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                let mut results = vec![clean(made, &root)];
                for key in ["apply-key", "apply-key", "another-key"] {
                    let result = host
                        .live_device_state_recall_apply_async(
                            &json!(results.len() + 1),
                            &json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":key}),
                            None,
                        )
                        .await
                        .unwrap();
                    results.push(clean(result, &root));
                }
                same(&json!(results), &row["results"], row["mode"].as_str().unwrap());
                same(&serde_json::to_value(sim.state.borrow().clone()).unwrap(), &row["state"], "state");
            }
        })
        .await;
}
#[test]
fn atomic_device_state_files_keep_owner_permissions_and_preserve_linked_prior_versions() {
    let fixture = fixture();
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("saved.json");
    assert!(!write_device_state_file_atomically(&target, &fixture["file"], false).unwrap());
    assert_eq!(read_device_state_file(&json!(target)).unwrap(), fixture["file"]);
    assert!(write_device_state_file_atomically(&target, &fixture["file"], false).unwrap_err().message().contains("already exists"));
    let alias = temp.path().join("prior.json");
    fs::hard_link(&target, &alias).unwrap();
    let prior = fs::read(&alias).unwrap();
    let mut next = fixture["file"].clone();
    next["savedAt"] = json!("2026-01-02T00:00:00.000Z");
    assert!(write_device_state_file_atomically(&target, &next, true).unwrap());
    assert_eq!(fs::read(&alias).unwrap(), prior);
    assert_ne!(fs::read(&target).unwrap(), prior);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let mut invalid = next.clone();
    invalid["digest"] = json!("0".repeat(64));
    assert!(write_device_state_file_atomically(&target, &invalid, true).is_err());
    assert_eq!(read_device_state_file(&json!(target)).unwrap(), next);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
}

struct StatusAdapter {
    sim: Rc<DeterministicLiveSimulator>,
    status: LiveStatus,
}
impl LiveAdapter for StatusAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.clone())
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
        self.sim.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for StatusAdapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.sim.invoke_async(i, c).await
    }
    async fn reconnect_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect_async(c).await
    }
    async fn close(&self) -> Result<(), LiveError> {
        self.sim.close().await
    }
}
