//! Every catalog entry crosses the public protocol boundary, including capability gates and sync refusals.
#[path = "../../../tests/support/chunks.rs"]
mod chunks;
use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
    tool_catalog::TOOL_CATALOG,
};
use serde_json::{json, Value};
use std::rc::Rc;
struct Advertised {
    sim: Rc<DeterministicLiveSimulator>,
    status: LiveStatus,
}
impl LiveAdapter for Advertised {
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
impl AsyncLiveAdapter for Advertised {
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
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
/// The bridge version the golden files were recorded with; pinned so a version bump changes none of them.
const ORACLE_VERSION: &str = "1.0.74";
fn clean(value: &Value) -> Value {
    match value {
        Value::Array(a) => json!(a.iter().map(clean).collect::<Vec<_>>()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if k == "expiresAt" || k == "sampledAt" {
                            json!("<time>")
                        } else if k == "transactionId" {
                            json!("<transaction>")
                        } else if k == "text" && v.is_string() {
                            serde_json::from_str::<Value>(v.as_str().unwrap()).map(|v| clean(&v)).unwrap_or(v.clone())
                        } else {
                            clean(v)
                        },
                    )
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}
fn difference(a: &Value, b: &Value, path: &str) -> Option<String> {
    if canonical_mutation_identity(a).unwrap() == canonical_mutation_identity(b).unwrap() {
        return None;
    }
    if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
        for (k, v) in a {
            if let Some(expected) = b.get(k) {
                if let Some(d) = difference(v, expected, &format!("{path}.{k}")) {
                    return Some(d);
                }
            } else {
                return Some(format!("{path}.{k}: unexpected actual field"));
            }
        }
        for k in b.keys() {
            if !a.contains_key(k) {
                return Some(format!("{path}.{k}: missing actual field"));
            }
        }
    }
    if let (Some(a), Some(b)) = (a.as_array(), b.as_array()) {
        if a.len() != b.len() {
            return Some(format!("{path}: array lengths {} != {}", a.len(), b.len()));
        }
        for (i, (a, b)) in a.iter().zip(b).enumerate() {
            if let Some(d) = difference(a, b, &format!("{path}[{i}]")) {
                return Some(d);
            }
        }
    }
    Some(format!(
        "{path}: actual {} != expected {}",
        a.to_string().chars().take(400).collect::<String>(),
        b.to_string().chars().take(400).collect::<String>()
    ))
}
fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/host-catalog-routes-oracle.json")).unwrap()
}
#[test]
fn source_oracle_covers_the_complete_current_catalog() {
    assert_eq!(
        json!(TOOL_CATALOG.iter().map(|t| &t.name).collect::<Vec<_>>()),
        oracle()["tools"],
        "source oracle must cover the complete current catalog"
    );
}
/// The oracle's cases, every tool's, in 16 tests that nextest runs side by side.
mod every_catalog_tool_matches_source_at_the_public_boundary {
    crate::chunks::chunked!(super::check_cases; part_00 = 0, part_01 = 1, part_02 = 2, part_03 = 3, part_04 = 4, part_05 = 5,
        part_06 = 6, part_07 = 7, part_08 = 8, part_09 = 9, part_10 = 10, part_11 = 11, part_12 = 12, part_13 = 13, part_14 = 14,
        part_15 = 15);
}
/// The oracle's cases at index `chunk`, `chunk + chunks`, …
fn check_cases(chunk: usize, chunks: usize) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(tokio::task::LocalSet::new()
        .run_until(async {
            let oracle = oracle();
            let status: LiveStatus = serde_json::from_value(oracle["fullStatus"].clone()).unwrap();
            let mut failures = vec![];
            let mut checked = 0;
            for (index, case) in oracle["cases"].as_array().unwrap().iter().enumerate().skip(chunk).step_by(chunks) {
                checked += 1;
                let sim = Rc::new(DeterministicLiveSimulator::new());
                let adapter: Rc<dyn AsyncLiveAdapter> = if case["mode"] == "available" {
                    Rc::new(Advertised { sim: sim.clone(), status: status.clone() })
                } else {
                    sim.clone()
                };
                let host = Rc::new(McpHost::new(adapter, McpHostOptions { server_version: Some(ORACLE_VERSION.into()), ..Default::default() }).unwrap());
                if case["modern"] != true {
                    host.handle(&json!({"jsonrpc":"2.0","id":"setup","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"catalog-oracle","version":"1"}}})).unwrap();
                    host.handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
                }
                let mut request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":case["tool"],"arguments":case["args"]}});
                if case["modern"] == true {
                    request["params"]["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
                }
                let result = if case["sync"] == true { host.handle(&request) } else { host.handle_async(&request, None).await };
                let actual = clean(&match result {
                    Ok(v) => v.unwrap_or(Value::Null),
                    Err(e) => json!({"thrown":e.to_string()}),
                });
                let expected = &oracle["pool"][case["result"].as_u64().unwrap() as usize];
                let label = format!(
                    "case {index}, {} / {} / modern {} / sync {} / {}",
                    case["tool"], case["mode"], case["modern"], case["sync"], case["args"]
                );
                if let Some(d) = difference(&actual, expected, "response") {
                    failures.push(format!("{label}: {d}"));
                }
                let state=clean(&sim.state.borrow());
                if let Some(d) = difference(&state, &oracle["pool"][case["state"].as_u64().unwrap() as usize], "state")
                {
                    failures.push(format!("{label}: {d}"));
                }
            }
            assert!(
                failures.is_empty(),
                "{} discrepancies across {checked} of {} source cases (part {chunk} of {chunks}):\n{}",
                failures.len(),
                oracle["cases"].as_array().unwrap().len(),
                failures.join("\n")
            );
        }));
}
