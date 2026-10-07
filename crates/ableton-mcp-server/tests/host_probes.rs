#[path = "../../../tests/support/fixture_paths.rs"]
mod fixture_paths;
use ableton_mcp_server::{
    host::{McpHost, McpHostOptions, ToolCall},
    live::*,
    registry::live_registry_operations,
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
#[path = "support/library_fixtures.rs"]
mod library_fixtures;
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: LiveStatus,
    snapshot: LiveSnapshot,
    read: Value,
    case: Value,
    calls: RefCell<Vec<Value>>,
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        Ok(self.snapshot.clone())
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
fn context(c: Option<&LiveOperationContext>) -> Value {
    let Some(c) = c else { return Value::Null };
    assert!(c.signal.is_none());
    assert!(c.deadline_ms.is_some_and(|v| v > 0.0));
    json!({"deadlineMs":0})
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"snapshot","context":context(c),"request":r}));
        if let Some(f) = self.case["snapshotFail"].as_str() {
            return Err(LiveError::error(f));
        }
        Ok(self.snapshot.clone())
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"invoke","invocation":i,"context":context(c)}));
        if let Some(f) = self.case["invokeFail"].as_str() {
            return Err(LiveError::error(f));
        }
        Ok(self.read.clone())
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
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
fn fixture() -> Value {
    serde_json::from_str(include_str!("support/host_probes_oracle.json")).unwrap()
}
fn parsed(mut v: Value) -> Value {
    if let Some(t) = v["result"]["content"][0]["text"].as_str() {
        v["result"]["content"][0]["text"] = serde_json::from_str(t).unwrap();
    }
    v
}
fn canonical(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Object(o) => {
            let mut k: Vec<_> = o.keys().collect();
            k.sort();
            format!(
                "{{{}}}",
                k.into_iter().map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical(&o[k]))).collect::<Vec<_>>().join(",")
            )
        }
        _ => kumi_common::js::json::stringify(v),
    }
}
fn equal(a: &Value, b: &Value, label: &str) {
    let (a, b) = (canonical(a), canonical(b));
    if a != b {
        let i = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
        panic!(
            "{label} difference {i}\nactual {}\nexpected {}",
            a.chars().skip(i.saturating_sub(70)).take(400).collect::<String>(),
            b.chars().skip(i.saturating_sub(70)).take(400).collect::<String>()
        );
    }
}
fn patch(v: &mut Value, path: &[Value], value: Value) {
    if path.is_empty() {
        *v = value;
        return;
    }
    match &path[0] {
        Value::String(k) => patch(&mut v[k], &path[1..], value),
        Value::Number(n) => patch(&mut v[n.as_u64().unwrap() as usize], &path[1..], value),
        _ => panic!("bad patch"),
    }
}
#[tokio::test(flavor = "current_thread")]
async fn probe_host_matches_source_replies_dispatch_and_pagination() {
    let f = fixture();
    for (i, case) in f["cases"].as_array().unwrap().iter().enumerate() {
        let sim = DeterministicLiveSimulator::new();
        let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
        status["operations"] = json!(live_registry_operations());
        if let Some(p) = case["status"].as_object() {
            status.as_object_mut().unwrap().extend(p.clone());
        }
        let mut snapshot = f["base"].clone();
        for p in case["patch"].as_array().into_iter().flatten() {
            patch(&mut snapshot, p[0].as_array().unwrap(), p[1].clone());
        }
        let adapter = Rc::new(Adapter {
            sim,
            status: serde_json::from_value(status).unwrap(),
            snapshot: serde_json::from_value(snapshot).unwrap(),
            read: case.get("read").cloned().unwrap_or_else(|| f["reads"][case["tool"].as_str().unwrap()].clone()),
            case: case.clone(),
            calls: RefCell::new(vec![]),
        });
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let label = format!("case {i} {} {}", case["tool"], case["args"]);
        let out = host
            .dispatch_probe_tool(
                &ToolCall {
                    id: json!(1),
                    name: case["tool"].as_str().unwrap().into(),
                    arguments: Some(case["args"].clone()),
                    asynchronous: true,
                },
                None,
            )
            .await
            .unwrap();
        match out {
            Ok(v) => equal(&parsed(v), &case["result"], &label),
            Err(e) => assert_eq!(e.message(), case["error"].as_str().unwrap(), "{label}"),
        };
        equal(&json!(*adapter.calls.borrow()), &case["calls"], &format!("{label} dispatch"));
    }
}
fn replace(v: Value, root: &str, to: &str) -> Value {
    fixture_paths::map_strings(&v, &|text| {
        if to == "ROOT" {
            fixture_paths::normalize_root(text, root, to)
        } else {
            text.replace(root, to)
        }
    })
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_library_database_is_read_once_until_it_changes() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::getuid() } == 0 {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let db = root.join("files.db");
    std::fs::write(&db, library_fixtures::files_db()).unwrap();
    let host = McpHost::default();
    let args = json!({"database":db,"allowlistRoot":root,"limit":2});
    let first = host.live_library_search_async(&json!(1), &args).await.unwrap();
    assert_ne!(first["result"]["isError"], true, "{first}");
    // Unreadable now, but unchanged: the next page (or search) is answered from what was read.
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o000)).unwrap();
    let again = host.live_library_search_async(&json!(1), &args).await;
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(again.unwrap(), first);
    // Changed: it's read again.
    std::fs::write(&db, "not a database").unwrap();
    let changed = host.live_library_search_async(&json!(1), &args).await.unwrap();
    assert!(changed.to_string().contains("unreadable"), "{changed}");
}
#[tokio::test(flavor = "current_thread")]
async fn library_host_matches_source_allowlists_wal_queries_and_coercion() {
    let root = tempfile::tempdir().unwrap();
    let root = fixture_paths::native_path(&root.path().canonicalize().unwrap());
    let db = root.join("files.db");
    std::fs::write(&db, library_fixtures::files_db()).unwrap();
    std::fs::write(root.join("plugins.db"), library_fixtures::plugins_db()).unwrap();
    std::fs::write(root.join("bad.db"), "bad").unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::File::create(root.join("large.db")).unwrap().set_len(128 * 1024 * 1024 + 1).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&db, root.join("link.db")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("link-dir")).unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(&db, root.join("link.db")).unwrap();
        std::os::windows::fs::symlink_dir(&root, root.join("link-dir")).unwrap();
    }
    let host = McpHost::default();
    let f = fixture();
    for (i, case) in f["library"].as_array().unwrap().iter().enumerate() {
        let args = replace(case["args"].clone(), "ROOT", &root.to_string_lossy());
        let wal = case["wal"] == true;
        if wal {
            let mut bytes = library_fixtures::files_db();
            bytes[18] = 2;
            bytes[19] = 2;
            std::fs::write(&db, bytes).unwrap();
            std::fs::write(root.join("files.db-wal"), "WAL").unwrap();
        }
        let label = format!("library case {i} {}", case["args"]);
        match host.live_library_search_async(&json!(1), &args).await {
            Ok(v) => equal(&replace(parsed(v), &root.to_string_lossy(), "ROOT"), &case["result"], &label),
            Err(e) => assert_eq!(e.message(), case["error"].as_str().unwrap(), "{label}"),
        };
        if wal {
            std::fs::write(&db, library_fixtures::files_db()).unwrap();
            std::fs::remove_file(root.join("files.db-wal")).unwrap();
        }
    }
}
