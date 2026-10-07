//! Replay full source-host file/transaction workflows against real native file staging.
use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::VecDeque, fs, path::Path, rc::Rc};
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn clean(mut value: Value, root: &str) -> Value {
    if let Some(text) = value["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            value["result"]["content"][0]["text"] = body;
        }
    }
    fn walk(value: &mut Value, key: &str, root: &str) {
        if key == "expiresAt" && *value != 0 {
            *value = json!("$time");
            return;
        }
        if key == "mtimeMs" {
            *value = json!("$mtime");
            return;
        }
        match value {
            Value::String(text) => {
                if text.starts_with("simpler_") {
                    *text = "$transaction".into();
                    return;
                }
                *text = text.replace(root, "$root");
                #[cfg(windows)]
                if text.contains("$root") || text.contains("$stage") {
                    *text = text.replace('\\', "/");
                }
                *text = text.replace("$root/managed", "$stage");
                *text = regex::Regex::new(r#"\$stage/[^/"\\\s]+/"#).unwrap().replace_all(text, "$$stage/$$copy/").into_owned();
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, "", root)
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    walk(item, key, root)
                }
            }
            _ => {}
        }
    }
    walk(&mut value, "", root);
    value
}
fn expand(value: &Value, root: &str) -> Value {
    match value {
        Value::String(s) => json!(s.replace("$root", root)),
        Value::Array(a) => Value::Array(a.iter().map(|v| expand(v, root)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), expand(v, root))).collect()),
        _ => value.clone(),
    }
}
fn context(context: Option<&LiveOperationContext>) -> Value {
    let Some(c) = context else { return Value::Null };
    let mut out = json!({"deadline":c.deadline_ms.is_some()});
    if let Some(key) = &c.idempotency_key {
        out["idempotencyKey"] = json!(key)
    }
    if let Some(key) = &c.transaction_id {
        out["transactionId"] = json!(key)
    }
    out
}
struct Replay {
    root: String,
    status: RefCell<LiveStatus>,
    calls: RefCell<VecDeque<Value>>,
    label: RefCell<String>,
}
impl Replay {
    fn call(&self, method: &str, args: Value, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let want = self.calls.borrow_mut().pop_front().unwrap_or_else(|| panic!("{}: unexpected {method}", self.label.borrow()));
        let actual = clean(json!({"method":method,"args":args,"context":context(c)}), &self.root);
        same(&actual, &json!({"method":want["method"],"args":want["args"],"context":want["context"]}), &self.label.borrow());
        if let Some(error) = want.get("error") {
            let text = error["message"].as_str().unwrap().to_string();
            Err(if error["kind"] == "not-dispatched" { LiveError::MutationNotDispatched(text) } else { LiveError::error(text) })
        } else {
            Ok(want["result"].clone())
        }
    }
}
impl LiveAdapter for Replay {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.borrow().clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        panic!("unexpected sync snapshot")
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        panic!("unexpected sync get")
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        panic!("unexpected sync invoke")
    }
    fn subscribe(&self, _: LiveListener) -> Result<Unsubscribe, LiveError> {
        Ok(Box::new(|| {}))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Replay {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        serde_json::from_value(self.call("snapshot", json!(r), c)?).map_err(|e| LiveError::error(e.to_string()))
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        serde_json::from_value(self.call("discover", json!(r), c)?).map_err(|e| LiveError::error(e.to_string()))
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.call("get", json!(r), c).map(|v| (!v.is_null()).then_some(v))
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.call("invoke", json!(i), c)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        serde_json::from_value(self.call("status", Value::Null, c)?).map_err(|e| LiveError::error(e.to_string()))
    }
}
fn mode(path: &Path) -> Value {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        json!(fs::metadata(path).unwrap().permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Value::Null
    }
}
fn writable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).unwrap();
}
fn staged_files(root: &Path) -> Value {
    fn visit(root: &Path, output: &mut Vec<Value>) {
        for entry in fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit(&path, output)
            } else {
                let bytes = fs::read(&path).unwrap();
                output.push(json!({"name":entry.file_name().to_string_lossy(),"size":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes)),"mode":mode(&path)}));
            }
        }
    }
    let mut values = vec![];
    visit(root, &mut values);
    values.sort_by_key(|v| serde_json::to_string(v).unwrap());
    json!(values)
}
/// The simulator with its Simpler shown as the Remote Script shows one: its file as its sample's `filePath`.
struct SampleRows(Rc<DeterministicLiveSimulator>);
fn as_live(snapshot: LiveSnapshot) -> Result<LiveSnapshot, LiveError> {
    let mut value = serde_json::to_value(snapshot).unwrap();
    for device in
        value["tracks"].as_array_mut().into_iter().flatten().flat_map(|track| track["devices"].as_array_mut().into_iter().flatten())
    {
        if let Some(path) = device.as_object_mut().and_then(|device| device.remove("samplePath")) {
            device["sample"] = json!({"filePath":path});
        }
    }
    serde_json::from_value(value).map_err(|e| LiveError::error(e.to_string()))
}
impl LiveAdapter for SampleRows {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.0.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        as_live(self.0.snapshot()?)
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.0.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.0.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for SampleRows {
    async fn snapshot_async(&self, _: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        as_live(self.0.snapshot()?)
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("unused discovery"))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.0.invoke(i)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
#[tokio::test(flavor = "current_thread")]
async fn a_simpler_that_holds_a_sample_takes_another_and_gives_it_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().canonicalize().unwrap();
            #[cfg(windows)]
            let root = std::path::PathBuf::from(root.to_string_lossy().strip_prefix(r"\\?\").unwrap_or(&root.to_string_lossy()));
            let stage = root.join("managed");
            fs::create_dir(&stage).unwrap();
            fs::write(root.join("snare.wav"), hex::decode("52494646100000005741564566616b652d617564696f2d6279746573").unwrap()).unwrap();
            let sim = Rc::new(DeterministicLiveSimulator::new());
            sim.state.borrow_mut()["tracks"][0]["devices"].as_array_mut().unwrap().push(json!({"ref":"device:simpler-1","parentRef":"track:track-1","name":"Simpler","kind":"instrument","className":"OriginalSimpler","objectIdentity":"simulator:device:simpler-1","enabled":true,"parameters":[],"samplePath":"/Samples/kick.wav"}));
            let options = McpHostOptions { import_staging_dir: Some(stage.to_string_lossy().into_owned()), ..Default::default() };
            let host = McpHost::new(Rc::new(SampleRows(sim.clone())), options).unwrap();
            let text = |reply: &Value| serde_json::from_str::<Value>(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let sample = || sim.state.borrow()["tracks"][0]["devices"][1]["samplePath"].clone();
            let args = json!({"deviceRef":"device:simpler-1","filePath":root.join("snare.wav"),"allowedRoot":root});
            let preview = text(&host.live_simpler_preview_async(&json!(1), &args).await);
            assert_eq!(preview["currentSample"], "/Samples/kick.wav", "{preview}");
            let id = preview["transactionId"].clone();
            let applied = host
                .live_simpler_apply_async(&json!(2), &json!({"transactionId":id,"confirmation":"apply","idempotencyKey":"simpler-apply-1"}), None)
                .await
                .unwrap();
            assert_eq!(text(&applied)["state"], "applied", "{applied}");
            assert!(sample().as_str().unwrap().starts_with(stage.to_str().unwrap()), "{}", sample());
            let undone = host.undo_simpler_async(&json!(3), &json!({"transactionId":id,"confirmation":"undo","idempotencyKey":"simpler-undo-1"}), None).await;
            assert_eq!(text(&undone)["state"], "undone", "{undone}");
            assert_eq!(sample(), "/Samples/kick.wav");
        })
        .await
}
#[tokio::test(flavor = "current_thread")]
async fn simpler_sample_replacement_preserves_source_file_and_recovery_authority() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("fixtures/host-simpler-oracle.json")).unwrap();
            for case in fixture["cases"].as_array().unwrap() {
                let label = case["label"].as_str().unwrap();
                if std::env::var("KUMI_SIMPLER_CASE").ok().is_some_and(|v| v != label) {
                    continue;
                }
                let folder = tempfile::tempdir().unwrap();
                let root = folder.path().canonicalize().unwrap();
                #[cfg(windows)]
                let root = std::path::PathBuf::from(root.to_string_lossy().strip_prefix(r"\\?\").unwrap_or(&root.to_string_lossy()));
                let root_text = root.to_string_lossy().into_owned();
                let stage = root.join("managed");
                fs::create_dir(&stage).unwrap();
                for file in case["files"].as_array().unwrap() {
                    let path = root.join(file["name"].as_str().unwrap());
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    if let Some(link) = file["link"].as_str() {
                        #[cfg(unix)]
                        std::os::unix::fs::symlink(root.join(link), path).unwrap();
                        #[cfg(windows)]
                        std::os::windows::fs::symlink_file(root.join(link), path).unwrap();
                    } else if file["directory"] == true {
                        fs::create_dir_all(path).unwrap()
                    } else {
                        fs::write(&path, hex::decode(file["hex"].as_str().unwrap_or("")).unwrap()).unwrap();
                        if let Some(size) = file["size"].as_u64() {
                            fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(size).unwrap();
                        }
                    }
                }
                let adapter = Rc::new(Replay {
                    root: root_text.clone(),
                    status: RefCell::new(serde_json::from_value(case["steps"][0]["status"].clone()).unwrap()),
                    calls: Default::default(),
                    label: RefCell::new(label.into()),
                });
                let host = Rc::new(
                    McpHost::new(
                        adapter.clone(),
                        McpHostOptions { import_staging_dir: Some(stage.to_string_lossy().into_owned()), ..Default::default() },
                    )
                    .unwrap(),
                );
                let mut tx = None::<String>;
                for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                    let at = format!("{label} step {index}");
                    *adapter.label.borrow_mut() = at.clone();
                    *adapter.status.borrow_mut() = serde_json::from_value(step["status"].clone()).unwrap();
                    *adapter.calls.borrow_mut() = step["calls"].as_array().unwrap().clone().into();
                    if let Some(change) = step["change"].as_str() {
                        let record = host.transaction_record(tx.as_ref().unwrap()).unwrap();
                        let path = record.borrow()["payload"]["filePath"].as_str().unwrap().to_string();
                        match change {
                            "source-edit" => fs::write(root.join("demo.wav"), b"unauthorized changed source").unwrap(),
                            "staged-edit" => {
                                writable(Path::new(&path));
                                fs::write(&path, b"tampered bytes").unwrap()
                            }
                            "staged-remove" => fs::remove_file(path).unwrap(),
                            "staged-asd" => fs::write(format!("{path}.asd"), b"analysis").unwrap(),
                            "expire" | "expire-sweep" => record.borrow_mut()["expiresAt"] = json!(0),
                            _ => panic!("unknown change"),
                        }
                    }
                    let action = step["action"].as_str().unwrap();
                    let mut params = expand(&step["args"], &root_text);
                    if matches!(action, "apply" | "undo" | "finalize") {
                        params["transactionId"] = json!(tx.as_ref().unwrap())
                    }
                    let signal = Signal::new();
                    if step["abort"] == true {
                        signal.cancel()
                    }
                    let result = match action {
                        "preview" => host.live_simpler_preview_async(&json!(1), &params).await,
                        "project" => host.live_project_import_async(&json!(1), &params).await,
                        "apply" => host.live_simpler_apply_async(&json!(1), &params, Some(&signal)).await.unwrap_or(Value::Null),
                        "undo" => host
                            .with_undo_watch(&json!(1), &params, async {
                                Ok(host.undo_simpler_async(&json!(1), &params, Some(&signal)).await)
                            })
                            .await
                            .unwrap(),
                        "finalize" => host.live_recovery_finalize_async(&json!(1), &params).await.unwrap(),
                        _ => panic!("unknown action"),
                    };
                    if action == "preview" {
                        tx = result["result"]["content"][0]["text"]
                            .as_str()
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .and_then(|v| v["transactionId"].as_str().map(str::to_string));
                    }
                    same(&clean(result, &root_text), &step["result"], &at);
                    let record = tx.as_ref().and_then(|tx| host.transaction_record(tx)).map(|t| t.borrow().clone()).unwrap_or(Value::Null);
                    same(&clean(record, &root_text), &step["record"], &format!("{at} record"));
                    assert!(adapter.calls.borrow().is_empty(), "{at}: {} remaining adapter calls", adapter.calls.borrow().len());
                    let expected_files = step["files"].clone();
                    #[cfg(windows)]
                    let expected_files = {
                        let mut files = expected_files;
                        for file in files.as_array_mut().unwrap() {
                            file["mode"] = Value::Null;
                        }
                        files
                    };
                    same(&staged_files(&stage), &expected_files, &format!("{at} files"));
                    #[cfg(unix)]
                    same(&mode(&stage), &step["stageMode"], &format!("{at} managed root mode"));
                }
                let retained = staged_files(&stage);
                drop(host);
                same(&staged_files(&stage), &retained, &format!("{label}: managed media survives host shutdown"));
                for entry in fs::read_dir(&stage).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        for file in fs::read_dir(path).unwrap() {
                            writable(&file.unwrap().path())
                        }
                    }
                }
            }
        })
        .await
}
