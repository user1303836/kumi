use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::mcp::{
    client::{McpEndpoint, StderrStatus},
    types::{CallToolResult, ListToolsResult},
};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_daw_ableton::history::LiveHistory;
use kumi_daw_ableton::{
    connection::{ConnectionOptions, LiveConnection},
    history::{Applied, FastResult, History, MAX_ENTRIES},
    remember::Remember,
};
use kumi_types::{
    contracts::{ChangeRecord, JsonObject},
    errors::RuntimeError,
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
    fn server_info(&self) -> Option<kumi_agent::mcp::types::Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let mut names =
            vec!["live_status", "live_discover", "live_undo", "live_transaction_release", "live_run_python", "live_session_emergency_stop"];
        if self.config["undoStep"] == true {
            names.extend(["live_undo_step_begin", "live_undo_step_end"]);
        }
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
    // The recorded TypeScript source predates bridge causes in unsure undo notes. Compare its other behavior
    // unchanged; the focused regression below asserts the new note's exact cause, bounds and sanitization.
    fn legacy_notes(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                if fields
                    .get("note")
                    .and_then(Value::as_str)
                    .is_some_and(|note| note.starts_with("Live didn't confirm the undo; inspect the Set before another change."))
                {
                    fields.insert("note".into(), json!("Live didn't confirm the undo; try again."));
                }
                fields.values_mut().for_each(legacy_notes);
            }
            Value::Array(items) => items.iter_mut().for_each(legacy_notes),
            _ => {}
        }
    }
    let mut value = value.clone();
    legacy_notes(&mut value);
    let text = stringify(&canonical(&value));
    group.replace_all(&uuid.replace_all(&text, "<uuid>"), "<group>").into_owned()
}
fn eq(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(normalized(actual), normalized(expected), "{label}")
}

#[tokio::test(flavor = "current_thread")]
async fn an_unsure_undo_keeps_the_bridge_cause_in_its_record_for_the_serve_audit() {
    tokio::task::LocalSet::new().run_until(async {
        // More than the text response's 2,048 UTF-16 units: the cause must be extracted before wire text is bounded.
        let cause = format!("device move was not confirmed\n\t\u{202e}\u{200b}\x1b[31mactual index 1\x1b[0m {}", "é".repeat(2_500));
        let plain_prefix = "device move was not confirmed actual index 1 ";
        let expected_cause = format!("{plain_prefix}{}…", "é".repeat(399 - plain_prefix.chars().count()));
        let replies = [
            json!({"isError":true,"content":[{"type":"text","text":json!({"reason":cause,"remediation":"Device undo is uncertain"}).to_string()}],"structuredContent":{"state":"uncertain"}}),
            json!({"isError":true,"content":[],"structuredContent":{"state":"uncertain","reason":cause}}),
            json!({"content":[{"type":"text","text":json!({"state":"pending","reason":"device move confirmation is pending"}).to_string()}]}),
        ];
        for (index, reply) in replies.into_iter().enumerate() {
            let endpoint = Rc::new(Fixture { config: json!({"responses":[{"reply":reply}]}), calls: RefCell::new(vec![]), original: RefCell::new(Signal::new()) });
            let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
            let out = endpoint.clone();
            options.connect = Some(Rc::new(move |_| { let endpoint: Rc<dyn McpEndpoint> = out.clone(); async move { Ok(endpoint) }.boxed_local() }));
            let connection = LiveConnection::new(options);
            connection.start(Signal::new()).await.unwrap();
            connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            connection.available.set(true);
            let history = History::new(connection.clone(), Remember::new(connection.clone(), None, None), Some(50), None);
            history.remember(serde_json::from_value(json!({"id":"move-1","family":"device","title":"Move SoftClip","state":"applied","at":0})).unwrap(), "device-move-1".into(), None);
            history.entries.borrow()["move-1"].borrow_mut().tool = Some("move_device".into());
            let result = history.undo("move-1", Signal::new(), false).await.unwrap();
            let record = result.record.unwrap();
            assert_eq!(record.state, kumi_types::contracts::ChangeState::Unsure);
            let note = record.note.unwrap();
            assert!(note.starts_with("Live didn't confirm the undo; inspect the Set before another change. Bridge: "), "{note}");
            if index < 2 {
                assert_eq!(note, format!("Live didn't confirm the undo; inspect the Set before another change. Bridge: {expected_cause}"));
                assert_eq!(note.split_once(" Bridge: ").unwrap().1.chars().count(), 400);
            } else {
                assert!(note.contains("device move confirmation is pending"), "{note}");
            }
            assert!(note.chars().count() <= 500, "{}", note.chars().count());
            assert!(!note.chars().any(|c| c.is_control() || matches!(c, '\u{202e}' | '\u{200b}')));
            assert!(!note.contains("[31m") && !note.contains("remediation") && !note.contains("{\"reason\"") && !note.contains("\\u"));
            assert_eq!(endpoint.calls.borrow().len(), 1, "an uncertain write is never retried automatically");
            connection.close().await.unwrap();
        }
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_typed_pad_content_refusal_keeps_its_history_row_applied() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (tool, state) in ["load_sample_to_pad", "load_samples_to_pads"].into_iter().flat_map(|tool| {
                [kumi_types::contracts::ChangeState::Applied, kumi_types::contracts::ChangeState::Unsure].map(|state| (tool, state))
            }) {
                let reason = "This request sent nothing to Live; a prior uncertain undo remains uncertain.";
                let reply = json!({"isError":true,"content":[{"type":"text","text":json!({"code":"changed_since","reason":reason,"undoUnavailable":true}).to_string()}]});
                let endpoint = Rc::new(Fixture {
                    config: json!({"undoStep":true,"responses":[{"value":{"stepId":"pad-undo-step"}},{"reply":reply},{"value":{"closed":true}}]}),
                    calls: RefCell::new(vec![]),
                    original: RefCell::new(Signal::new()),
                });
                let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
                let out = endpoint.clone();
                options.connect = Some(Rc::new(move |_| {
                    let endpoint: Rc<dyn McpEndpoint> = out.clone();
                    async move { Ok(endpoint) }.boxed_local()
                }));
                let connection = LiveConnection::new(options);
                connection.start(Signal::new()).await.unwrap();
                connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
                connection.available.set(true);
                let history = History::new(connection.clone(), Remember::new(connection.clone(), None, None), Some(50), None);
                history.remember(
                    serde_json::from_value(json!({"id":"pad-1","family":"device","title":"Load kick","state":state,"note":"Existing history note","at":0})).unwrap(),
                    "pad-load-1".into(),
                    None,
                );
                history.entries.borrow()["pad-1"].borrow_mut().tool = Some(tool.into());
                history.entries.borrow()["pad-1"].borrow_mut().undo_key = Some("original-undo-key".into());
                let before = serde_json::to_value(&*history.entries.borrow()["pad-1"].borrow()).unwrap();
                let result = history.undo("pad-1", Signal::new(), false).await.unwrap();
                assert!(result.record.is_none(), "a typed refusal does not move the history state on");
                assert!(result.refused && result.stopped);
                assert_eq!(serde_json::to_value(&*history.entries.borrow()["pad-1"].borrow()).unwrap(), before, "a no-write refusal preserves prior uncertainty and recovery keys");
                let refusal = history.host.guard.take_refusal().unwrap();
                assert_eq!((refusal.code, refusal.reason), ("changed_since", reason.into()));
                assert_eq!(endpoint.calls.borrow().iter().map(|call| call["name"].as_str().unwrap()).collect::<Vec<_>>(),
                    ["live_undo_step_begin", "live_undo", "live_undo_step_end"]);
                connection.close().await.unwrap();
            }
        })
        .await;
}
#[tokio::test(flavor = "current_thread")]
async fn undo_quiet_groups_retirement_and_emergency_stop_match_source() {
    tokio::task::LocalSet::new().run_until(async{
 let fixture:Value=serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/history-oracle.json")).unwrap();
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
                        history.remember(record(n), format!("t{n}"), None);
                    }
                })
                .await;
            assert_eq!(history.entries.borrow().len(), MAX_ENTRIES);
            assert!(history.entries.borrow().get("c0").is_none(), "the oldest go first");
            let group = history.grouped("Built the arrangement", &ids, &[]).unwrap();
            assert_eq!(history.entries.borrow().len(), MAX_ENTRIES);
            // The group made room for its own record before choosing its members, so none of them went for it.
            let members = history.entries.borrow()[&group].borrow().members.clone().unwrap();
            assert_eq!(members.len(), MAX_ENTRIES - 1);
            assert!(members.iter().all(|id| history.entries.borrow().contains_key(id)), "each member is kept");
            // One by one past the cap: each brings it back down, the group's first members going.
            for n in MAX_ENTRIES + 100..MAX_ENTRIES + 103 {
                history.remember(record(n), format!("t{n}"), None);
            }
            assert_eq!(history.entries.borrow().len(), MAX_ENTRIES);
            // The group's undo takes back the members Kumi keeps, and says only Live's own undo has the first steps.
            let undone = history.undo(&group, Signal::new(), false).await.unwrap();
            assert_eq!(
                (undone.text.as_str(), undone.is_error),
                ("Kumi took back 19,996 of its 20,100 changes; the first 104 are past the 20,000 changes Kumi keeps, so only Live's own undo (Cmd-Z in Live) can take them back.", true)
            );
            connection.close().await.unwrap();
        })
        .await;
}
