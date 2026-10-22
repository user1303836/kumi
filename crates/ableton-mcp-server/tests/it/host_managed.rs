use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
};
use serde_json::{json, Value};
use std::rc::Rc;
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-managed-oracle.json")).unwrap()
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn clean(mut value: Value) -> Value {
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
    value
}
fn simulator(state: &Value) -> Rc<DeterministicLiveSimulator> {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    *sim.state.borrow_mut() = serde_json::from_value(state.clone()).unwrap();
    sim
}
async fn call(host: &McpHost, tool: &str, id: usize, args: &Value) -> Result<Value, LiveError> {
    let id = json!(id);
    match tool {
        "liveMidiPreview" => Ok(host.live_midi_preview(&id, args)),
        "liveMidiPreviewAsync" => host.live_midi_preview_async(&id, args).await,
        "liveMidiApply" => Ok(host.live_midi_apply(&id, args)),
        "liveMidiApplyAsync" => host.live_midi_apply_async(&id, args, None).await,
        "liveBatchPreviewAsync" => Ok(host.live_batch_preview_async(&id, args).await),
        "liveBatchApplyAsync" => Ok(host.live_batch_apply_async(&id, args, None).await.unwrap_or(Value::Null)),
        _ => panic!("{tool}"),
    }
}
#[tokio::test]
async fn manager_host_validation_policy_and_error_frames_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = fixture();
            for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
                let sim = simulator(&fixture["state"]);
                let host =
                    McpHost::new(sim, McpHostOptions { tool_policy: row["options"].get("policy").cloned(), ..Default::default() }).unwrap();
                let got = call(&host, row["tool"].as_str().unwrap(), 1, &row["args"]).await;
                if let Some(error) = row.get("error") {
                    assert_eq!(got.unwrap_err().message(), error.as_str().unwrap(), "row {index}");
                } else {
                    same(&clean(got.unwrap_or_else(|e| panic!("row {index}: {e}"))), &row["result"], &format!("row {index} {row}"));
                }
            }
        })
        .await;
}
#[tokio::test]
async fn manager_host_preview_apply_retry_and_epoch_fences_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture = fixture();
            for row in fixture["workflows"].as_array().unwrap() {
                let sim = simulator(&fixture["state"]);
                let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
                let tool = row["tool"].as_str().unwrap();
                let args = if tool.contains("Batch") {
                    json!({"operations":[{"kind":"track.rename","trackRef":"track:track-1","name":"Renamed"}]})
                } else {
                    fixture["preview"].clone()
                };
                let made = call(&host, tool, 1, &args).await.unwrap();
                let body: Value = serde_json::from_str(made["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                let mut results = vec![clean(made)];
                if row["reconnect"] == true {
                    sim.reconnect().unwrap();
                }
                let apply_tool = tool.replace("Preview", "Apply");
                for key in ["apply-key", "apply-key", "another-key"] {
                    let result = call(
                        &host,
                        &apply_tool,
                        results.len() + 1,
                        &json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":key}),
                    )
                    .await;
                    results.push(match result {
                        Ok(v) => clean(v),
                        Err(e) => json!({"error":e.message()}),
                    });
                }
                same(&json!(results), &row["results"], tool);
                same(&serde_json::to_value(sim.state.borrow().clone()).unwrap(), &row["state"], tool);
            }
        })
        .await;
}
