use ableton_mcp_server::{
    host::{McpHost, McpHostOptions},
    live::*,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

struct Adapter {
    sim: DeterministicLiveSimulator,
    state: RefCell<String>,
    writes: Cell<usize>,
    enabled: Cell<bool>,
    lose_reply: Cell<bool>,
    malformed_reply: Cell<bool>,
    cache: RefCell<HashMap<String, Value>>,
}
impl Adapter {
    fn new() -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            state: RefCell::new(json!({"value":false,"signature":"private-snapshot"}).to_string()),
            writes: Cell::new(0),
            enabled: Cell::new(true),
            lose_reply: Cell::new(false),
            malformed_reply: Cell::new(false),
            cache: RefCell::new(HashMap::new()),
        }
    }
    fn read(&self) -> Value {
        let state = self.state.borrow().clone();
        json!({"state":state,"stateRevision":hex::encode(Sha256::digest(state.as_bytes())),"summary":json!({"kind":"global-follow","value":serde_json::from_str::<Value>(&state).unwrap()["value"]}).to_string()})
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        let mut status = self.sim.status()?;
        status.operations = Some(if self.enabled.get() {
            vec!["snapshot".into(), "willington.editing.read".into(), "willington.editing.set".into()]
        } else {
            vec![]
        });
        status.extra.insert("nativeEditingKinds".into(), json!(["global-follow", "group-tracks"]));
        Ok(status)
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("unexpected sync invoke"))
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
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(r)
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
    async fn refresh_status_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        if i.operation == "willington.editing.read" {
            let mut read = self.read();
            if i.args.get("edit").is_some() {
                read["next"] = json!(json!({"value":i.args["edit"]["enabled"]}).to_string());
            }
            return Ok(read);
        }
        assert_eq!(i.operation, "willington.editing.set");
        let c = c.unwrap();
        let key = format!("{:?}:{:?}:{:?}", c.transaction_id, c.idempotency_key, i.args);
        if let Some(cached) = self.cache.borrow().get(&key) {
            return Ok(cached.clone());
        }
        if i.args["expectedStateRevision"] != self.read()["stateRevision"] {
            return Err(LiveError::error("state changed"));
        }
        let next: Value = serde_json::from_str(i.args["next"].as_str().unwrap()).unwrap();
        *self.state.borrow_mut() = if let Some(prior) = next["restore"].as_str() {
            prior.into()
        } else {
            json!({"value":next["value"],"signature":"private-snapshot"}).to_string()
        };
        self.writes.set(self.writes.get() + 1);
        let mut result = self.read();
        result["changed"] = json!(true);
        if self.malformed_reply.replace(false) {
            result["stateRevision"] = json!("invalid");
        }
        self.cache.borrow_mut().insert(key, result.clone());
        if self.lose_reply.replace(false) {
            return Err(LiveError::error("injected reply lost after mutation"));
        }
        Ok(result)
    }
}
fn body(result: Value) -> Value {
    if result["result"]["isError"] == true || result.get("error").is_some() {
        return result;
    }
    serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
async fn preview(host: &McpHost, kind: &str) -> Value {
    body(host.live_native_editing_preview_async(&json!(1), &json!({"kind":kind,"edit":{"enabled":true}})).await.unwrap())
}
fn args(p: &Value, confirmation: &str, key: &str) -> Value {
    json!({"transactionId":p["transactionId"],"confirmation":confirmation,"idempotencyKey":key})
}

#[tokio::test]
async fn native_apply_retry_and_explicit_history_restore_keep_private_state_out_of_receipts() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let adapter = Rc::new(Adapter::new());
            let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
            let prior = adapter.state.borrow().clone();
            let p = preview(&host, "global-follow").await;
            assert!(p["transactionId"].as_str().is_some_and(|id| id.starts_with("nativeedit_")), "{p}");
            assert!(!p.to_string().contains("private-snapshot"));
            let a = args(&p, "apply", "apply-key-1");
            adapter.lose_reply.set(true);
            let failed = host.live_native_editing_apply_async(&json!(2), &a, None).await.unwrap();
            assert_eq!(failed["result"]["isError"], true);
            let applied = body(host.live_native_editing_apply_async(&json!(3), &a, None).await.unwrap());
            assert_eq!(applied["state"], "applied");
            assert_eq!(adapter.writes.get(), 1);
            assert!(!applied.to_string().contains("private-snapshot"));
            let repeated = body(host.live_native_editing_apply_async(&json!(4), &a, None).await.unwrap());
            assert_eq!(repeated["idempotent"], true);
            let u = args(&p, "undo", "undo-key-1");
            adapter.lose_reply.set(true);
            let failed = host.undo_native_editing_async(&json!(5), &u, None).await;
            assert_eq!(failed["result"]["isError"], true);
            let restored = body(host.undo_native_editing_async(&json!(6), &u, None).await);
            assert_eq!(restored["state"], "undone");
            assert_eq!(*adapter.state.borrow(), prior);
            assert_eq!(adapter.writes.get(), 2);
            assert_eq!(body(host.undo_native_editing_async(&json!(7), &u, None).await)["idempotent"], true);
        })
        .await;
}
#[tokio::test]
async fn native_stale_apply_and_stale_history_never_overwrite_external_changes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let adapter = Rc::new(Adapter::new());
            let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
            let p = preview(&host, "global-follow").await;
            *adapter.state.borrow_mut() = json!({"value":"external"}).to_string();
            let failed = host.live_native_editing_apply_async(&json!(2), &args(&p, "apply", "apply-stale"), None).await.unwrap();
            assert_eq!(failed["result"]["isError"], true);
            assert_eq!(adapter.writes.get(), 0);
            let p = preview(&host, "global-follow").await;
            let applied = body(host.live_native_editing_apply_async(&json!(3), &args(&p, "apply", "apply-good"), None).await.unwrap());
            assert_eq!(applied["state"], "applied");
            *adapter.state.borrow_mut() = json!({"value":"external-2"}).to_string();
            let failed = host.undo_native_editing_async(&json!(4), &args(&p, "undo", "undo-stale"), None).await;
            assert_eq!(failed["result"]["isError"], true);
            assert_eq!(adapter.writes.get(), 1);
        })
        .await;
}
#[tokio::test]
async fn native_capability_loss_group_history_and_malformed_readback_refuse() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let adapter = Rc::new(Adapter::new());
            let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
            let p = preview(&host, "global-follow").await;
            adapter.enabled.set(false);
            assert_eq!(
                host.live_native_editing_apply_async(&json!(2), &args(&p, "apply", "apply-off"), None).await.unwrap()["result"]["isError"],
                true
            );
            assert_eq!(adapter.writes.get(), 0);
            adapter.enabled.set(true);
            let p = preview(&host, "group-tracks").await;
            assert_eq!(p["undoable"], false);
            host.live_native_editing_apply_async(&json!(3), &args(&p, "apply", "apply-group"), None).await.unwrap();
            let refused = host.undo_native_editing_async(&json!(4), &args(&p, "undo", "undo-group"), None).await;
            assert_eq!(refused["result"]["isError"], true);
            assert!(refused.to_string().contains("not undoable"));
            let p = preview(&host, "global-follow").await;
            adapter.malformed_reply.set(true);
            let refused = host.live_native_editing_apply_async(&json!(5), &args(&p, "apply", "apply-bad"), None).await.unwrap();
            assert_eq!(refused["result"]["isError"], true);
            assert!(refused.to_string().contains("uncertain"));
        })
        .await;
}

#[tokio::test]
async fn native_tools_and_history_cross_public_protocol_boundary() {
    tokio::task::LocalSet::new().run_until(async {
        let adapter = Rc::new(Adapter::new());
        let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
        let prior = adapter.state.borrow().clone();
        host.handle_async(&json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),None).await.unwrap();
        host.handle_async(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}),None).await.unwrap();
        let p = body(host.handle_async(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"live_native_editing_preview","arguments":{"kind":"global-follow","edit":{"enabled":true}}}}),None).await.unwrap().unwrap());
        assert!(p["transactionId"].as_str().is_some_and(|id| id.starts_with("nativeedit_")), "{p}");
        let a = body(host.handle_async(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"live_native_editing_apply","arguments":args(&p,"apply","boundary-apply")}}),None).await.unwrap().unwrap());
        assert_eq!(a["state"],"applied");
        let u = body(host.handle_async(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"live_undo","arguments":args(&p,"undo","boundary-undo")}}),None).await.unwrap().unwrap());
        assert_eq!(u["state"],"undone", "{u}");
        assert_eq!(*adapter.state.borrow(),prior);
    }).await;
}
