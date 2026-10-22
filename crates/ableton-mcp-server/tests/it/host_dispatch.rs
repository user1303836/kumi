use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, mutations::result_body, McpHost, McpHostOptions},
    live::*,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{rc::Rc, sync::LazyLock};
/// The bridge version the golden files were recorded with; pinned so a version bump changes none of them.
const ORACLE_VERSION: &str = "1.0.74";
fn clean(value: &Value) -> Value {
    static ID: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"^(trackstruct|transportaction|devdel|clipdel|scenedel|trackdel|locatordel|arrmidi|data|browserload|devdup|device|mixer|tempo|parameter|parameters|structure|arrangement|rename|transport|change|noteupdate|notedelete|routing|capturemidi|scenecapture|clipdup|arrclip|clipmove)_[A-Za-z0-9_-]+$",
        )
        .unwrap()
    });
    match value {
        Value::String(s) if ID.is_match(s) => json!(format!("{}_<id>", s.split('_').next().unwrap())),
        Value::Array(a) => json!(a.iter().map(clean).collect::<Vec<_>>()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if k == "expiresAt" || k == "sampledAt" {
                            json!("<time>")
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
#[tokio::test(flavor = "current_thread")]
async fn public_requests_fused_changes_and_undo_match_source() {
    tokio::task::LocalSet::new().run_until(async{
        let data:Value=serde_json::from_str(include_str!("fixtures/host-dispatch-oracle.json")).unwrap();
        for (index,case) in data["cases"].as_array().unwrap().iter().enumerate(){
            let sim=Rc::new(DeterministicLiveSimulator::new());
            let host=Rc::new(McpHost::new(sim.clone(),McpHostOptions{tool_policy:case.get("policy").cloned(),server_version:Some(ORACLE_VERSION.into()),..Default::default()}).unwrap());
            if case["modern"]!=true{
                host.handle(&json!({"jsonrpc":"2.0","id":"setup","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"oracle","version":"1"}}})).unwrap();
                host.handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
            }
            let mut tx="missing".to_owned();let mut seq=0;
            for (step,op) in case["operations"].as_array().unwrap().iter().enumerate(){
                let result=if let Some(policy)=op.get("policy"){host.set_tool_policy(policy).unwrap();json!({"policy":true})}else{
                    let args=serde_json::from_str::<Value>(&serde_json::to_string(op.get("args").unwrap_or(&json!({}))).unwrap().replace("\"$tx\"",&serde_json::to_string(&tx).unwrap())).unwrap();
                    seq+=1;let mut request=json!({"jsonrpc":"2.0","id":seq,"method":"tools/call","params":{"name":op["tool"],"arguments":args}});
                    if case["modern"]==true{request["params"]["_meta"]=json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});}
                    let signal=Signal::new();if op["abort"]==true{signal.cancel();}
                    let result=if case["sync"]==true{host.handle(&request)}else{host.handle_async(&request,Some(&signal)).await};
                    let result=match result{Ok(v)=>v.unwrap_or(Value::Null),Err(e)=>json!({"error":e.to_string()})};
                    if let Some(id)=result_body(&result).and_then(|b|b["transactionId"].as_str().map(str::to_owned)){tx=id;}
                    clean(&result)
                };
                let expected=&data["pool"][case["results"][step].as_u64().unwrap() as usize];
                assert_eq!(canonical_mutation_identity(&result).unwrap(),canonical_mutation_identity(expected).unwrap(),"{} case {index} step {step}: {op}",case["label"]);
            }
            let state=clean(&sim.state.borrow());let expected=&data["pool"][case["state"].as_u64().unwrap() as usize];
            assert_eq!(canonical_mutation_identity(&state).unwrap(),canonical_mutation_identity(expected).unwrap(),"{} case {index} final state",case["label"]);
        }
    }).await;
}
