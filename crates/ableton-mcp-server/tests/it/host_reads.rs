use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions, ToolCall},
    live::*,
    registry::live_registry_operations,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: LiveStatus,
    case: Value,
    defaults: Value,
    calls: RefCell<Vec<Value>>,
}
fn context(context: Option<&LiveOperationContext>) -> Value {
    let Some(context) = context else { return Value::Null };
    let mut value = json!({});
    if context.deadline_ms.is_some() {
        value["deadline"] = json!(true);
    }
    if let Some(key) = &context.idempotency_key {
        value["idempotencyKey"] = json!(key);
    }
    if let Some(id) = &context.transaction_id {
        value["transactionId"] = json!(id);
    }
    if context.signal.is_some() {
        value["signal"] = json!(true);
    }
    value
}
impl Adapter {
    fn failure(&self, kind: &str, default: &str) -> Result<(), LiveError> {
        if self.case["fail"] == kind {
            Err(LiveError::error(self.case["message"].as_str().unwrap_or(default)))
        } else {
            Ok(())
        }
    }
}
impl LiveAdapter for Adapter {
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
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"refresh","context":context(c)}));
        self.failure("refresh", "request failed: offline")?;
        self.status()
    }
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"snapshot","context":context(c),"request":r}));
        self.failure("snapshot", "request failed: no snapshot")?;
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"discover","request":r,"context":context(c)}));
        self.failure("discover", "request failed: no discovery")?;
        self.sim.discover_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"invoke","invocation":i,"context":context(c)}));
        self.failure("invoke", "request failed: exact refusal")?;
        self.failure(&i.operation, "request failed: exact refusal")?;
        if let Some(value) = self.case["returns"].get(&i.operation) {
            return Ok(value.clone());
        }
        if let Some(value) = self.defaults.get(&i.operation) {
            return Ok(value.clone());
        }
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
#[tokio::test]
async fn read_handlers_match_source_results_and_exact_adapter_dispatch() {
    let data: Value = serde_json::from_str(include_str!("fixtures/host-reads-oracle.json")).unwrap();
    for (index, case) in data["cases"].as_array().unwrap().iter().enumerate() {
        let sim = DeterministicLiveSimulator::new();
        let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
        status["operations"] = json!(live_registry_operations());
        if let Some(patch) = case["statusPatch"].as_object() {
            status.as_object_mut().unwrap().extend(patch.clone());
        }
        let adapter = Rc::new(Adapter {
            sim,
            status: serde_json::from_value(status).unwrap(),
            case: case.clone(),
            defaults: data["defaults"].clone(),
            calls: RefCell::new(Vec::new()),
        });
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let call = ToolCall {
            id: json!(1),
            name: case["tool"].as_str().unwrap().into(),
            arguments: case.get("args").cloned(),
            asynchronous: true,
        };
        let result = host.dispatch_read_tool(&call, None).await.expect("selected read family");
        if let Some(error) = case.get("error") {
            assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "case {index}: {case}");
        } else {
            let mut result = result.unwrap_or_else(|e| panic!("case {index}: {case}: {e}"));
            if let Some(text) = result["result"]["content"][0]["text"].as_str() {
                if let Ok(mut content) = serde_json::from_str::<Value>(text) {
                    if call.name == "live_performance_read" && content.get("sampledAt").is_some() {
                        content["sampledAt"] = json!(0);
                    }
                    result["result"]["content"][0]["text"] = json!(canonical_mutation_identity(&content).unwrap());
                }
            }
            let hash = hex::encode(Sha256::digest(canonical_mutation_identity(&result).unwrap().as_bytes()));
            assert_eq!(
                hash,
                case["resultHash"].as_str().unwrap(),
                "case {index}: {} {}\nexpected {}\ngot {result}",
                case["tool"],
                case["args"],
                case.get("result").unwrap_or(&Value::Null)
            );
        }
        assert_eq!(
            canonical_mutation_identity(&json!(*adapter.calls.borrow())).unwrap(),
            canonical_mutation_identity(&case["calls"]).unwrap(),
            "case {index}: {} {} dispatch differs",
            case["tool"],
            case["args"]
        );
    }
}
