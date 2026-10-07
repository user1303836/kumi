use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, mutations::*, McpHost, McpHostOptions},
    live::*,
    registry::live_registry_operations,
};
use futures::FutureExt;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-mutations-oracle.json")).unwrap()
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn clean(mut value: Value) -> Value {
    if let Some(text) = value["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            value["result"]["content"][0]["text"] = body;
        }
    }
    value
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: LiveStatus,
    options: Value,
    calls: RefCell<Vec<Value>>,
    retired: RefCell<Vec<Value>>,
}
impl Adapter {
    fn new(options: &Value) -> Self {
        let sim = DeterministicLiveSimulator::new();
        let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
        status["operations"] = json!(live_registry_operations());
        if let Some(patch) = options["statusPatch"].as_object() {
            for (k, v) in patch {
                status[k] = v.clone();
            }
        }
        Self {
            sim,
            status: serde_json::from_value(status).unwrap(),
            options: options.clone(),
            calls: Default::default(),
            retired: Default::default(),
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
        let mut ctx = json!({"deadline":c.is_some_and(|c|c.deadline_ms.is_some())});
        if let Some(c) = c {
            if let Some(id) = &c.transaction_id {
                ctx["transactionId"] = json!(id);
            }
            if let Some(id) = &c.idempotency_key {
                ctx["idempotencyKey"] = json!(id);
            }
            if c.signal.is_some() {
                ctx["signal"] = json!(true);
            }
        }
        self.calls.borrow_mut().push(json!({"invocation":i,"context":ctx}));
        if self.options["fail"] == true {
            return Err(LiveError::error("request failed: exact refusal"));
        }
        if let Some(result) = self.options.get("result") {
            return Ok(result.clone());
        }
        Ok(match i.operation.as_str() {
            "undo.step.begin" => json!({"open":true,"stepId":"undo-step-fixture","expiresAt":9999999999999i64}),
            "undo.step.end" => json!({"open":false,"ended":true}),
            "song.undo" => json!({"done":true,"canUndo":false,"canRedo":true}),
            "song.redo" => json!({"done":true,"canUndo":true,"canRedo":false}),
            _ => panic!("unexpected invocation {i:?}"),
        })
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
        self.calls.borrow_mut().push(json!({"refresh":true}));
        self.status()
    }
    fn has_retire_transaction_async(&self) -> bool {
        true
    }
    fn retires_on_its_own(&self) -> bool {
        self.options["retiresOnItsOwn"] == true
    }
    async fn retire_transaction_async(&self, id: &str, c: Option<&LiveOperationContext>, _: bool) -> Result<Value, LiveError> {
        self.retired.borrow_mut().push(json!({"id":id,"deadline":c.is_some_and(|c|c.deadline_ms.is_some())}));
        if self.options["retireFail"] == true {
            Err(LiveError::error("retirement failed"))
        } else {
            Ok(json!({}))
        }
    }
}
#[test]
fn preview_change_payloads_match_source() {
    for row in fixture()["previews"].as_array().unwrap() {
        let got = preview_change(row["name"].as_str().unwrap(), &row["record"]);
        if let Some(error) = row.get("error") {
            assert_eq!(got.unwrap_err().message(), error.as_str().unwrap(), "{row}");
        } else if row["missing"] == true {
            assert!(got.unwrap().is_none(), "{row}");
        } else {
            same(&serde_json::to_value(got.unwrap().unwrap()).unwrap(), &row["result"], &row.to_string());
        }
    }
}
#[tokio::test]
async fn a_mutation_that_panics_leaves_its_transaction_free_to_try_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let adapter = Rc::new(Adapter::new(&json!({})));
            let host = Rc::new(McpHost::new(adapter, McpHostOptions::default()).unwrap());
            let args = json!({"transactionId":"transaction-1","idempotencyKey":"key-1234"});
            // It yields first, so the panic comes inside the flight's task.
            let panics = |_: Option<Signal>| async {
                tokio::task::yield_now().await;
                if args_are_fine() {
                    panic!("the operation panics");
                }
                Ok::<_, LiveError>(None)
            };
            let first = host.single_flight_mutation("tool", &json!(1), &args, panics, None).await;
            assert!(first.is_err(), "{first:?}");
            assert_eq!(host.active_async_operations(), 0);
            // The same request runs again, rather than joining the dead flight or finding the transaction in flight.
            let answers = |_: Option<Signal>| async {
                Ok(Some(json!({"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{}"}],"isError":false}})))
            };
            let second = host.single_flight_mutation("tool", &json!(2), &args, answers, None).await;
            assert!(second.as_ref().is_ok_and(Option::is_some), "{second:?}");
        })
        .await;
}
fn args_are_fine() -> bool {
    true
}
#[tokio::test]
async fn shared_mutation_waiters_cancellation_conflicts_and_retirement_match_source() {
    tokio::task::LocalSet::new().run_until(async{
 for row in fixture()["flights"].as_array().unwrap(){
  let options=&row["options"];let adapter=Rc::new(Adapter::new(options));let host=Rc::new(McpHost::new(adapter.clone(),McpHostOptions::default()).unwrap());
  let signals=[Signal::new(),Signal::new()];if options["preAborted"]==true{signals[0].cancel();}
  let (send,recv)=tokio::sync::oneshot::channel::<Result<Option<Value>,LiveError>>();let gate=async move{recv.await.unwrap()}.boxed_local().shared();let seen=Rc::new(RefCell::new(Vec::new()));
  let mut args=if options["noKey"]==true{json!({})}else{json!({"idempotencyKey":"key-1234"})};
  if options["noKey"]!=true && options["keyOnly"]!=true{args[if options["capture"]==true{"captureId"}else{"transactionId"}]=json!(if options["capture"]==true{"capture-1"}else{"transaction-1"});}
  let mut second_args=args.clone();if options["differentKey"]==true{second_args["idempotencyKey"]=json!("other-key");}if options["differentArgs"]==true{second_args["confirmation"]=json!("different");}
  let execute=|gate:futures::future::Shared<futures::future::LocalBoxFuture<'static,Result<Option<Value>,LiveError>>>,seen:Rc<RefCell<Vec<Value>>>|move|signal:Option<Signal>|async move{seen.borrow_mut().push(json!({"signal":signal.is_some(),"aborted":signal.is_some_and(|s|s.is_cancelled())}));gate.await};
  let first_id=json!(1);let mut a=host.single_flight_mutation("tool",&first_id,&args,execute(gate.clone(),seen.clone()),Some(&signals[0])).boxed_local();
  // Keep request IDs alive while the borrowed futures run.
  let second_id=json!(2);let mut b=host.single_flight_mutation(if options["differentTool"]==true{"other"}else{"tool"},&second_id,&second_args,execute(gate,seen.clone()),Some(&signals[1])).boxed_local();
  let mut first_ready=a.as_mut().now_or_never();let mut second_ready=b.as_mut().now_or_never();let active=host.active_async_operations();
  if options["abortFirst"]==true{signals[0].cancel();}if options["abortSecond"]==true{signals[1].cancel();}
  for _ in 0..8{if first_ready.is_none(){first_ready=a.as_mut().now_or_never();}if second_ready.is_none(){second_ready=b.as_mut().now_or_never();}tokio::task::yield_now().await;}
  let outcome=if options["nullResult"]==true{None}else{Some(json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":if options["textResult"]==true{"unparsed text".into()}else{kumi_common::js::json::stringify(&json!({"state":"applied","idempotent":false}))}}],"isError":options["resultError"]==true}}))};
  let _=send.send(if options["fail"]==true{Err(LiveError::error("execution failed"))}else{Ok(outcome)});
  let (a,b)=futures::join!(async move{if let Some(v)=first_ready{v}else{a.await}},async move{if let Some(v)=second_ready{v}else{b.await}});
  let results=[a,b].into_iter().map(|r|match r{Ok(v)=>clean(v.unwrap_or(Value::Null)),Err(e)=>json!({"error":e.message()})}).collect::<Vec<_>>();
  for _ in 0..8{tokio::task::yield_now().await;}
  same(&json!({"seen":*seen.borrow(),"active":active,"results":results,"retired":*adapter.retired.borrow(),"after":host.active_async_operations()}),&json!({"seen":row["seen"],"active":row["active"],"results":row["results"],"retired":row["retired"],"after":row["after"]}),&options.to_string());
 }
 }).await;
}
#[tokio::test]
async fn live_undo_step_and_history_handlers_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for row in fixture()["history"].as_array().unwrap() {
                let adapter = Rc::new(Adapter::new(&row["options"]));
                let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
                let call = || async {
                    match row["tool"].as_str().unwrap() {
                        "begin" => host.live_undo_step_begin_async(&json!(1), &row["args"]).await,
                        "end" => host.live_undo_step_end_async(&json!(1), &row["args"]).await,
                        other => host.live_song_history_async(&json!(1), &row["args"], other == "redo", None).await.unwrap_or(Value::Null),
                    }
                };
                same(&clean(call().await), &row["result"], &row.to_string());
                if row["options"]["repeat"] == true {
                    same(&clean(call().await), &row["repeat"], &row.to_string());
                }
                if row["options"]["close"] == true {
                    host.close_open_undo_step().await;
                }
                same(&json!(*adapter.calls.borrow()), &row["calls"], &row.to_string());
            }
        })
        .await;
}
