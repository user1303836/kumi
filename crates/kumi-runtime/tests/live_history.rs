use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_runtime::{
    core::{
        contracts::{ChangeRecord, JsonObject},
        errors::RuntimeError,
    },
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        history::{Applied, FastResult, History, MAX_ENTRIES},
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
struct Fixture {
    config: Value,
    calls: RefCell<Vec<Value>>,
    original: RefCell<Signal>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_runtime::mcp::types::Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let names =
            ["live_status", "live_discover", "live_undo", "live_transaction_release", "live_run_python", "live_session_emergency_stop"];
        Ok(serde_json::from_value(json!({"tools":names.iter().filter(|name|!self.config["missing"].as_array().is_some_and(|a|a.contains(&json!(name)))).map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let index = self.calls.borrow().len();
        self.calls.borrow_mut().push(json!({"name":name,"args":args}));
        let response =
            self.config["responses"].as_array().and_then(|a| a.get(index)).cloned().unwrap_or(json!({"value":{"state":"undone"}}));
        if response["cancel"] == true {
            self.original.borrow().cancel();
        }
        if let Some(error) = response["throw"].as_str() {
            return Err(RuntimeError::plain(error));
        }
        let body = &response["value"];
        serde_json::from_value(response.get("reply").cloned().unwrap_or_else(|| {
            if body.is_object() {
                json!({"content":[{"type":"text","text":stringify(body)}],"structuredContent":body})
            } else {
                json!({"content":[{"type":"text","text":stringify(body)}]})
            }
        }))
        .map_err(|e| RuntimeError::plain(e.to_string()))
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
fn op<'a>(history: &'a History, operation: &'a Value, signal: Signal) -> LocalBoxFuture<'a, Result<Value, RuntimeError>> {
    async move {
        Ok(match operation["op"].as_str().unwrap() {
            "remember" => {
                history.remember(
                    serde_json::from_value(operation["record"].clone()).unwrap(),
                    operation["transactionId"].as_str().unwrap_or("").into(),
                    operation.get("restore").map(|v| serde_json::from_value(v.clone()).unwrap()),
                );
                Value::Null
            }
            "count" => {
                history.changes_this_turn.set(history.changes_this_turn.get() + operation["count"].as_u64().unwrap() as usize);
                Value::Null
            }
            "undo" => serde_json::to_value(
                history.undo(operation["target"].as_str().unwrap_or("last"), signal, operation["discard"] == true).await?,
            )
            .unwrap(),
            "retire" => {
                history.retire(operation["note"].as_str().unwrap());
                Value::Null
            }
            "release" => {
                history.release(&serde_json::from_value::<Vec<String>>(operation["ids"].clone()).unwrap());
                Value::Null
            }
            "group" => json!(history.grouped(
                operation["title"].as_str().unwrap(),
                &serde_json::from_value::<Vec<String>>(operation["ids"].clone()).unwrap(),
                &operation.get("apart").map(|v| serde_json::from_value::<Vec<String>>(v.clone()).unwrap()).unwrap_or_default()
            )),
            "fast" => match history.run_fast(operation["code"].as_str().unwrap_or("pass").into(), signal).await? {
                FastResult::Result(value) => json!({"result":value}),
                FastResult::Error { error, sent } => json!({"error":error,"sent":sent}),
            },
            "stop" => json!(history.stop_everything(signal).await),
            "fail" => return Err(RuntimeError::plain("quiet failed")),
            "quiet" => {
                let mut into = Vec::new();
                let result = history
                    .quietly((operation["keep"] == true).then_some(&mut into), async {
                        let mut values = Vec::new();
                        for operation in operation["steps"].as_array().unwrap() {
                            values.push(op(history, operation, signal.clone()).await?);
                        }
                        Ok::<_, RuntimeError>(values)
                    })
                    .await;
                let mut value = JsonObject::new();
                if operation["keep"] == true {
                    value.insert("into".into(), json!(into));
                }
                match result {
                    Ok(values) => value.insert("value".into(), json!(values)),
                    Err(e) => value.insert("error".into(), json!(e.to_string())),
                };
                Value::Object(value)
            }
            other => panic!("unknown operation {other}"),
        })
    }
    .boxed_local()
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|key| (key.clone(), canonical(&map[key]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        v => v.clone(),
    }
}
fn normalized(value: &Value) -> String {
    let uuid = regex::Regex::new("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    let group = regex::Regex::new(r"\bc\d+\b").unwrap();
    let text = stringify(&canonical(value));
    group.replace_all(&uuid.replace_all(&text, "<uuid>"), "<group>").into_owned()
}
fn eq(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(normalized(actual), normalized(expected), "{label}")
}
#[tokio::test(flavor = "current_thread")]
async fn undo_quiet_groups_retirement_and_emergency_stop_match_source() {
    tokio::task::LocalSet::new().run_until(async{
 let fixture:Value=serde_json::from_str(include_str!("support/history-oracle.json")).unwrap();
 for case in fixture["cases"].as_array().unwrap(){
  let config=&case["config"];let endpoint=Rc::new(Fixture{config:config.clone(),calls:RefCell::new(Vec::new()),original:RefCell::new(Signal::new())});
  let mut options=ConnectionOptions::new(Rc::new(|_,_|{}));let out=endpoint.clone();options.connect=Some(Rc::new(move |_|{let endpoint:Rc<dyn McpEndpoint>=out.clone();async move{Ok(endpoint)}.boxed_local()}));options.now=Some(Rc::new(||chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
  let connection=LiveConnection::new(options);connection.start(Signal::new()).await.unwrap();connection.tools().unwrap().refresh(Signal::new()).await.unwrap();connection.available.set(config["available"]!=false);connection.lost.set(config["lost"]==true);
  let events=Rc::new(RefCell::new(Vec::<Value>::new()));let out=events.clone();let remember=Remember::new(connection.clone(),None,None);let history=History::new(connection.clone(),remember,Some(50),Some(Rc::new(move|record|out.borrow_mut().push(serde_json::to_value(record).unwrap()))));
  for entry in config["entries"].as_array().into_iter().flatten(){let entry:Applied=serde_json::from_value(entry.clone()).unwrap();history.entries.borrow_mut().insert(entry.record.id.clone(),Rc::new(RefCell::new(entry)));}
  for row in config["known"].as_array().into_iter().flatten(){connection.references.borrow_mut().known.insert(row[0].as_str().unwrap().into(),serde_json::from_value(row[1].clone()).unwrap());}
  for (index,operation) in case["operations"].as_array().unwrap().iter().enumerate(){
   let signal=Signal::new();if operation["abort"]==true{signal.cancel();}*endpoint.original.borrow_mut()=signal.clone();
   let value=match op(&history,operation,signal).await{Ok(v)=>v,Err(RuntimeError::Aborted)=>json!({"error":"cancelled"}),Err(e)=>json!({"error":e.to_string()})};
   let changes:Vec<_>=history.entries.borrow().values().map(|entry|serde_json::to_value(&*entry.borrow()).unwrap()).collect();let state=json!({"changes":changes,"changesThisTurn":history.changes_this_turn.get(),"known":connection.references.borrow().known.iter().collect::<Vec<_>>()});
   let label=format!("{} {index}",case["label"]);eq(&value,&case["results"][index]["value"],&label);eq(&state,&case["results"][index]["state"],&format!("{label} state"));
  }
  eq(&json!(*endpoint.calls.borrow()),&case["calls"],&format!("{} calls",case["label"]));eq(&json!(*events.borrow()),&case["events"],&format!("{} events",case["label"]));
  if case["label"]=="retry-throw"{let calls=endpoint.calls.borrow();assert_eq!(calls[0]["args"]["idempotencyKey"],calls[1]["args"]["idempotencyKey"]);}
  if case["label"]=="stop-retry-fails"{let calls=endpoint.calls.borrow();assert_ne!(calls[1]["args"]["idempotencyKey"],calls[3]["args"]["idempotencyKey"]);}
  connection.close().await.unwrap();
 }
}).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_sessions_history_keeps_at_most_its_cap_however_changes_come_in() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let endpoint = Rc::new(Fixture { config: json!({}), calls: RefCell::new(Vec::new()), original: RefCell::new(Signal::new()) });
            let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
            let out = endpoint.clone();
            options.connect = Some(Rc::new(move |_| {
                let endpoint: Rc<dyn McpEndpoint> = out.clone();
                async move { Ok(endpoint) }.boxed_local()
            }));
            let connection = LiveConnection::new(options);
            connection.start(Signal::new()).await.unwrap();
            let remember = Remember::new(connection.clone(), None, None);
            let history = History::new(connection.clone(), remember, Some(50), None);
            let record = |n: usize| {
                serde_json::from_value::<ChangeRecord>(
                    json!({"id":format!("c{n}"),"family":"clip","title":"Made a clip","state":"applied","at":0}),
                )
                .unwrap()
            };
            // An Arrangement build: more changes than the cap, quietly, then one group of them.
            let mut ids = Vec::new();
            history
                .quietly(Some(&mut ids), async {
                    for n in 0..MAX_ENTRIES + 100 {
                        history.remember(record(n), String::new(), None);
                    }
                })
                .await;
            assert_eq!(history.entries.borrow().len(), MAX_ENTRIES);
            assert!(history.entries.borrow().get("c0").is_none(), "the oldest go first");
            history.grouped("Built the arrangement", &ids, &[]);
            assert_eq!(history.entries.borrow().len(), MAX_ENTRIES);
            // One by one past the cap: each brings it back down.
            for n in 0..3 {
                history.remember(record(MAX_ENTRIES + 100 + n), String::new(), None);
            }
            assert_eq!(history.entries.borrow().len(), MAX_ENTRIES);
            connection.close().await.unwrap();
        })
        .await;
}
