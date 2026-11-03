use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::mcp::{
    client::{McpEndpoint, StderrStatus},
    types::{CallToolResult, Implementation, ListToolsResult},
};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_daw_ableton::connection::{ConnectionOptions, LiveConnection};
use kumi_types::{contracts::JsonObject, errors::RuntimeError};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
#[derive(Clone)]
struct Listener {
    on: Rc<Cell<bool>>,
    callback: Rc<dyn Fn()>,
}
struct Fixture {
    case: Value,
    calls: RefCell<Vec<Value>>,
    disconnects: RefCell<Vec<Listener>>,
    closes: Cell<usize>,
    owner: RefCell<Weak<LiveConnection>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        let names = ["live_status", "live_discover", "server_status", "live_note_read", "live_device_read"];
        Ok(serde_json::from_value(json!({"tools":names.iter().filter(|n|self.case["settings"]["missing"].as_str()!=Some(n)).map(|name|json!({"name":name,"inputSchema":{"type":"object","properties":{},"additionalProperties":true}})).collect::<Vec<_>>()})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let index = self.calls.borrow().len();
        self.calls.borrow_mut().push(json!({"name":name,"args":args}));
        let entry = &self.case["responses"][index];
        match entry["effect"].as_str() {
            Some("invalidate") => self.owner.borrow().upgrade().unwrap().invalidate(),
            Some("disconnect") => {
                let listeners = self.disconnects.borrow().clone();
                for l in listeners {
                    if l.on.get() {
                        (l.callback)();
                    }
                }
            }
            Some("abort") => {
                signal.cancel();
                return Err(RuntimeError::Aborted);
            }
            _ => {}
        }
        if let Some(message) = entry["throw"].as_str() {
            return Err(RuntimeError::plain(message));
        }
        serde_json::from_value(entry["reply"].clone()).map_err(|e| RuntimeError::plain(format!("fixture call {index}: {e}")))
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, callback: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        let on = Rc::new(Cell::new(true));
        self.disconnects.borrow_mut().push(Listener { on: on.clone(), callback });
        Box::new(move || on.set(false))
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closes.set(self.closes.get() + 1);
        Ok(())
    }
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|key| (key.clone(), canonical(&map[key]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}
fn eq(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(stringify(&canonical(actual)), stringify(&canonical(expected)), "{label}");
}
#[tokio::test(flavor = "current_thread")]
async fn model_reads_match_source_authority_epoch_errors_and_dispatches() {
    tokio::task::LocalSet::new().run_until(async{
 let fixture:Value=serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/connection-oracle.json")).unwrap();
 for original in fixture["cases"].as_array().unwrap(){
  let mut case=original.clone();case["responses"]=json!(case["responses"].as_array().unwrap().iter().map(|id|fixture["values"][id.as_u64().unwrap() as usize].clone()).collect::<Vec<_>>());
  let endpoint=Rc::new(Fixture{case:case.clone(),calls:RefCell::new(Vec::new()),disconnects:RefCell::new(Vec::new()),closes:Cell::new(0),owner:RefCell::new(Weak::new())});
  let states=Rc::new(RefCell::new(Vec::<Value>::new()));let out=states.clone();let mut options=ConnectionOptions::new(Rc::new(move|state,cause|out.borrow_mut().push(json!([state,cause]))));
  let connected=endpoint.clone();options.connect=Some(Rc::new(move|_|{let endpoint:Rc<dyn McpEndpoint>=connected.clone();async move{Ok(endpoint)}.boxed_local()}));options.now=Some(Rc::new(||chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00.000Z").unwrap().with_timezone(&chrono::Utc)));options.generation=Some("connection".into());options.reconnect_interval_ms=Some(3600000);options.quick_looks=Some(false);
  let connection=LiveConnection::new(options);*endpoint.owner.borrow_mut()=Rc::downgrade(&connection);let signal=Signal::new();connection.start(signal.clone()).await.unwrap();connection.tools().unwrap().refresh(signal.clone()).await.unwrap();
  let seed=&case["seed"];connection.epoch.set(seed["epoch"].as_f64());connection.last_epoch.set(seed["epoch"].as_f64());*connection.set.borrow_mut()=seed["set"].as_str().map(str::to_owned);connection.available.set(seed["available"]!=false);connection.lost.set(seed["lost"]==true);
  for row in seed["rows"].as_array().unwrap(){connection.register_rows(row[0].as_str().unwrap(),&row[1].as_array().unwrap().iter().map(|v|v.as_object().unwrap().clone()).collect::<Vec<_>>(),row[2].as_object().unwrap(),None).unwrap();}
  for reference in seed["shorts"].as_array().unwrap(){connection.references.borrow_mut().short_ref(reference.as_str().unwrap());}
  if case["settings"]["abort"]==true{signal.cancel();}
  let result=connection.invoke(case["name"].as_str().unwrap(),case["input"].as_object().unwrap().clone(),signal).await;
  let state={let book=connection.references.borrow();json!({"available":connection.available.get(),"lost":connection.lost.get(),"closed":connection.closed.get(),"lease":connection.lease.get(),"epoch":connection.epoch.get(),"refs":book.refs.iter().collect::<Vec<_>>(),"cursors":book.cursors.iter().collect::<Vec<_>>(),"known":book.known.iter().collect::<Vec<_>>(),"reconnected":connection.reconnected.get()})};
  connection.close().await.unwrap();let label=case["label"].as_str().unwrap();
  eq(&json!({"text":result.text,"isError":result.is_error}),&case["value"],label);eq(&state,&case["state"],&format!("{label} state"));eq(&json!(*endpoint.calls.borrow()),&case["calls"],&format!("{label} dispatches"));eq(&json!(*states.borrow()),&case["states"],&format!("{label} connection states"));eq(&json!(endpoint.closes.get()),&case["closes"],label);
 }
}).await;
}
struct LifeEndpoint {
    transport: bool,
    config: Value,
    id: usize,
    at: Cell<usize>,
    calls: Rc<RefCell<Vec<Value>>>,
    closes: Cell<usize>,
    disconnects: RefCell<Vec<Listener>>,
    /// Each call waits for this first, when there is one: a read in flight.
    gate: Option<Rc<tokio::sync::Notify>>,
}
#[async_trait(?Send)]
impl McpEndpoint for LifeEndpoint {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let mut names = vec!["live_status", "live_discover"];
        if self.transport {
            names.extend(["live_subscribe", "live_song_state"]);
        }
        Ok(serde_json::from_value(
            json!({"tools":names.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        self.calls.borrow_mut().push(json!({"endpoint":self.id,"name":name,"args":args}));
        let at = self.at.get();
        self.at.set(at + 1);
        if let Some(body) = self.config["replies"].as_array().and_then(|a| a.get(at.min(a.len().saturating_sub(1)))) {
            if let Some(error) = body["throw"].as_str() {
                return Err(RuntimeError::plain(error));
            }
            if body["error"] == true {
                return Ok(serde_json::from_value(json!({"isError":true,"content":[{"type":"text","text":"refused"}]})).unwrap());
            }
            return Ok(
                serde_json::from_value(json!({"content":[{"type":"text","text":stringify(body)}],"structuredContent":body})).unwrap()
            );
        }
        let mut body = json!({"connected":true,"adapter":"remote-script","epoch":7,"provenance":"fake-live"}).as_object().unwrap().clone();
        if let Some(status) = self.config["statuses"].as_array().and_then(|a| a.get(at.min(a.len().saturating_sub(1)))) {
            if let Some(error) = status["throw"].as_str() {
                return Err(RuntimeError::plain(error));
            }
            body.extend(status.as_object().unwrap().clone());
        }
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&json!(body))}],"structuredContent":body})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, callback: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        let on = Rc::new(Cell::new(true));
        self.disconnects.borrow_mut().push(Listener { on: on.clone(), callback });
        Box::new(move || on.set(false))
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closes.set(self.closes.get() + 1);
        Ok(())
    }
}
fn state(connection: &LiveConnection) -> Value {
    let book = connection.references.borrow();
    json!({"available":connection.available.get(),"lost":connection.lost.get(),"closed":connection.closed.get(),"lease":connection.lease.get(),"epoch":connection.epoch.get(),"refs":book.refs.iter().collect::<Vec<_>>(),"cursors":book.cursors.iter().collect::<Vec<_>>(),"known":book.known.iter().collect::<Vec<_>>(),"reconnected":connection.reconnected.get()})
}
#[tokio::test(flavor = "current_thread")]
async fn connection_start_close_disconnection_and_reconnection_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/connection-oracle.json")).unwrap();
            for case in fixture["lifecycle"].as_array().unwrap() {
                let calls = Rc::new(RefCell::new(Vec::new()));
                let states = Rc::new(RefCell::new(Vec::new()));
                let notes = Rc::new(RefCell::new(Vec::new()));
                let connections = Rc::new(RefCell::new(Vec::new()));
                let transports = Rc::new(RefCell::new(Vec::<Value>::new()));
                let endpoints: Vec<_> = case["configs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(id, config)| {
                        Rc::new(LifeEndpoint {
                            transport: case["transport"] == true,
                            config: config.clone(),
                            id,
                            at: Cell::new(0),
                            calls: calls.clone(),
                            closes: Cell::new(0),
                            disconnects: RefCell::new(Vec::new()),
                            gate: None,
                        })
                    })
                    .collect();
                let out = states.clone();
                let mut options = ConnectionOptions::new(Rc::new(move |state, cause| out.borrow_mut().push(json!([state, cause]))));
                let out = notes.clone();
                let retireable = Rc::new(Cell::new(false));
                let active = retireable.clone();
                options.on_retire = Some(Rc::new(move |note| {
                    if active.replace(false) {
                        out.borrow_mut().push(note.to_owned());
                    }
                }));
                let attempts = Rc::new(Cell::new(0));
                let next = attempts.clone();
                let out = connections.clone();
                let choices = endpoints.clone();
                options.connect = Some(Rc::new(move |_| {
                    let id = next.get();
                    next.set(id + 1);
                    out.borrow_mut().push(id);
                    let endpoint = choices.get(id).cloned();
                    async move {
                        let endpoint = endpoint.ok_or_else(|| RuntimeError::plain("no endpoint"))?;
                        if endpoint.config["fail"] == true {
                            return Err(RuntimeError::plain("spawn failed"));
                        }
                        Ok(endpoint as Rc<dyn McpEndpoint>)
                    }
                    .boxed_local()
                }));
                if case["transport"] == true {
                    let out = transports.clone();
                    options.on_transport = Some(Rc::new(move |transport| {
                        let mut value = serde_json::to_value(transport).unwrap();
                        if value.is_object() {
                            value["at"] = json!(0);
                        }
                        out.borrow_mut().push(value);
                    }));
                }
                options.generation = Some("connection".into());
                options.reconnect_interval_ms = Some(3600000);
                // The TypeScript Kumi recorded here looked for Live only at the interval: no look at once after a
                // dropped link, nor quick looks for a Live that still runs (A3-1, A3-2), which their own tests cover.
                options.quick_looks = Some(false);
                let connection = LiveConnection::new(options);
                let mut results = Vec::new();
                for command in case["commands"].as_array().unwrap() {
                    let command = command.as_str().unwrap();
                    let signal = Signal::new();
                    if command == "abortStart" {
                        signal.cancel();
                    }
                    let result: Result<Value, RuntimeError> = async {
                        match command {
                            "start" | "abortStart" => {
                                connection.start(signal.clone()).await?;
                                connection.tools().unwrap().refresh(signal.clone()).await?;
                                connection.epoch.set(Some(7.));
                                connection.last_epoch.set(Some(7.));
                                *connection.set.borrow_mut() = Some("[\"7:set:0\",\"song\"]".into());
                                connection.available.set(true);
                                connection.lost.set(false);
                                connection
                                    .register_rows(
                                        "track",
                                        &[json!({"ref":"7:track:0","name":"Bass"}).as_object().unwrap().clone()],
                                        &JsonObject::new(),
                                        None,
                                    )
                                    .unwrap();
                                connection
                                    .register_rows(
                                        "device",
                                        &[json!({"ref":"7:device:0:0","parentRef":"7:track:0","name":"Filter"})
                                            .as_object()
                                            .unwrap()
                                            .clone()],
                                        &JsonObject::new(),
                                        None,
                                    )
                                    .unwrap();
                                connection.references.borrow_mut().short_ref("7:track:0");
                                connection.references.borrow_mut().short_ref("7:device:0:0");
                                retireable.set(true);
                            }
                            "subscribe" => connection.subscribe(signal).await,
                            "transport" => connection.read_transport().await,
                            "close" => connection.close().await?,
                            "status" => return connection.read_status(signal).await.map(Value::Object).map_err(RuntimeError::from),
                            "liveAway" => connection.lose_live(),
                            "bridgeAway" => connection.lose_access(),
                            "look" => connection.look_for_live().await,
                            "disconnect" => {
                                for listener in endpoints[0].disconnects.borrow().clone() {
                                    if listener.on.get() {
                                        (listener.callback)();
                                    }
                                }
                            }
                            _ => panic!("unknown command {command}"),
                        };
                        Ok(Value::Null)
                    }
                    .await;
                    let value = result.unwrap_or_else(|e| json!({"error":e.to_string().replace(*kumi_types::command::KUMI,"<kumi>")}));
                    for _ in 0..8 {
                        tokio::task::yield_now().await;
                    }
                    results.push(json!({"command":command,"value":value,"state":state(&connection)}));
                }
                connection.close().await.unwrap();
                tokio::task::yield_now().await;
                let label = case["label"].as_str().unwrap();
                eq(&json!(results), &case["results"], label);
                eq(&json!(*calls.borrow()), &case["calls"], &format!("{label} calls"));
                eq(&json!(*states.borrow()), &case["states"], &format!("{label} states"));
                eq(&json!(*notes.borrow()), &case["notes"], &format!("{label} retirement"));
                eq(&json!(*transports.borrow()), &case["transports"], &format!("{label} transport events"));
                eq(&json!(*connections.borrow()), &case["connections"], &format!("{label} connections"));
                eq(&json!(endpoints.iter().map(|e| e.closes.get()).collect::<Vec<_>>()), &case["closes"], &format!("{label} closes"));
            }
        })
        .await;
}

/// A connection whose bridges are `LifeEndpoint`s made on demand: the first with `statuses`, those after it connected.
fn reconnecting(statuses: Value, running: Option<bool>) -> (Rc<LiveConnection>, Rc<RefCell<Vec<Rc<LifeEndpoint>>>>) {
    reconnecting_with(statuses, json!([{"connected": true}]), running, None)
}
/// The same, those after the first with `later`, and the first one's calls each waiting for `gate`.
fn reconnecting_with(
    statuses: Value,
    later: Value,
    running: Option<bool>,
    gate: Option<Rc<tokio::sync::Notify>>,
) -> (Rc<LiveConnection>, Rc<RefCell<Vec<Rc<LifeEndpoint>>>>) {
    let made = Rc::new(RefCell::new(Vec::<Rc<LifeEndpoint>>::new()));
    let calls = Rc::new(RefCell::new(vec![]));
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new({
        let made = made.clone();
        move |_| {
            let endpoint = Rc::new(LifeEndpoint {
                transport: false,
                config: json!({"statuses": if made.borrow().is_empty() { statuses.clone() } else { later.clone() }}),
                id: made.borrow().len(),
                at: Cell::new(0),
                calls: calls.clone(),
                closes: Cell::new(0),
                disconnects: RefCell::new(vec![]),
                gate: if made.borrow().is_empty() { gate.clone() } else { None },
            });
            made.borrow_mut().push(endpoint.clone());
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        }
    }));
    // Only the quick looks can happen in a test's time.
    options.reconnect_interval_ms = Some(3_600_000);
    options.live_running = running.map(|running| Rc::new(move || async move { running }.boxed_local()) as kumi_daw::process::AsyncBool);
    (LiveConnection::new(options), made)
}
fn drop_link(endpoint: &LifeEndpoint) {
    for listener in endpoint.disconnects.borrow().clone() {
        if listener.on.get() {
            (listener.callback)();
        }
    }
}
async fn until(what: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        if what() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    false
}

#[tokio::test(flavor = "current_thread")]
async fn a_dropped_link_is_looked_for_at_once_and_one_that_keeps_dropping_at_the_usual_pace() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (connection, made) = reconnecting(json!([{"connected": true}]), None);
            connection.start(Signal::new()).await.unwrap();
            // The bridge's process ends: Live is looked for now, not at the interval (an hour here).
            drop_link(&made.borrow()[0].clone());
            assert!(until(|| made.borrow().len() == 2 && connection.is_back()).await, "back at once: {} bridges", made.borrow().len());
            // It drops again within the minute: the usual pace, so a link that keeps dropping isn't reconnected as fast
            // as it drops.
            drop_link(&made.borrow()[1].clone());
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            assert_eq!(made.borrow().len(), 2, "no bridge started at once the second time");
            assert!(!connection.is_back());
            connection.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_cut_off_by_a_drop_fails_as_before_though_the_link_is_back_at_once() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // A status read is in flight on the first bridge when its process ends, and the new bridge is attached
            // before the read's answer comes: the read still fails with the dropped transport's own error.
            let gate = Rc::new(tokio::sync::Notify::new());
            let (connection, made) =
                reconnecting_with(json!([{"throw": "MCP disconnected"}]), json!([{"connected": true}]), None, Some(gate.clone()));
            connection.start(Signal::new()).await.unwrap();
            let reading = connection.clone();
            let read =
                tokio::task::spawn_local(async move { reading.read_status(Signal::new()).await.map(|_| ()).map_err(RuntimeError::from) });
            tokio::task::yield_now().await;
            drop_link(&made.borrow()[0].clone());
            assert!(until(|| made.borrow().len() == 2 && connection.is_back()).await, "back at once, while the read waits");
            gate.notify_one();
            let error = read.await.unwrap().unwrap_err().to_string();
            assert!(error.contains("MCP disconnected"), "the read's own error, as before: {error}");
            connection.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_live_that_still_runs_is_looked_for_every_quarter_second_and_a_closed_one_at_the_usual_pace() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for running in [true, false] {
                // Live's status says it's away, so each look reads it again through the bridge that's still there (the
                // fresh bridge the first look starts isn't ready either, and the next is due only 30 s later).
                let away = json!([{"connected": false}]);
                let (connection, made) = reconnecting_with(away.clone(), away, Some(running), None);
                connection.start(Signal::new()).await.unwrap();
                let first = made.borrow()[0].clone();
                let before = first.at.get();
                connection.lose_live();
                tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                let looks = first.at.get() - before;
                if running {
                    assert!((2..=5).contains(&looks), "every 250 ms while Live runs: {looks} looks in 900 ms");
                } else {
                    assert_eq!(looks, 0, "Live closed: the usual interval");
                }
                connection.close().await.unwrap();
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn losing_live_while_it_still_runs_is_another_set_opening_and_kumis_own_when_it_asked() {
    // #188: opening another Set reloads Live's Remote Script, so Kumi loses Live for a moment; that was
    // said as "Live closed", and the request that asked for the Set was cancelled.
    tokio::task::LocalSet::new()
        .run_until(async {
            for (running, asked, cause) in [(false, false, "live"), (true, false, "set"), (true, true, "askedset"), (false, true, "live")] {
                let states = Rc::new(RefCell::new(Vec::<Value>::new()));
                let out = states.clone();
                let mut options = ConnectionOptions::new(Rc::new(move |state, cause| out.borrow_mut().push(json!([state, cause]))));
                options.live_running = Some(Rc::new(move || async move { running }.boxed_local()));
                options.reconnect_interval_ms = Some(3600000);
                let connection = LiveConnection::new(options);
                if asked {
                    connection.expect_set_change(60_000);
                }
                connection.lose_live();
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(*states.borrow(), [json!(["disconnected", cause])], "running {running}, asked {asked}");
                connection.close().await.unwrap();
            }
            // Without a way to tell, Live is lost as before, at once.
            let states = Rc::new(RefCell::new(Vec::<Value>::new()));
            let out = states.clone();
            let mut options = ConnectionOptions::new(Rc::new(move |state, cause| out.borrow_mut().push(json!([state, cause]))));
            options.reconnect_interval_ms = Some(3600000);
            let connection = LiveConnection::new(options);
            connection.lose_live();
            assert_eq!(*states.borrow(), [json!(["disconnected", "live"])]);
            connection.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn losing_live_while_it_runs_and_its_remote_script_still_listens_is_live_busy() {
    // #256: Live too busy to answer in time was taken for another Set opening, and the request was cancelled. A Set
    // opening reloads the Remote Script, which stops listening meanwhile; a busy Live's keeps listening.
    tokio::task::LocalSet::new()
        .run_until(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let folder = tempfile::tempdir().unwrap();
            let config = folder.path().join("bridge-config.json");
            std::fs::write(&config, json!({"version":2,"bridge":{"host":"127.0.0.1","port":port}}).to_string()).unwrap();
            for (listening, cause) in [(true, "busy"), (false, "set")] {
                let states = Rc::new(RefCell::new(Vec::<Value>::new()));
                let out = states.clone();
                let mut options = ConnectionOptions::new(Rc::new(move |state, cause| out.borrow_mut().push(json!([state, cause]))));
                options.live_running = Some(Rc::new(|| async { true }.boxed_local()));
                options.reconnect_interval_ms = Some(3600000);
                options.bridge_config = listening.then(|| config.to_string_lossy().into_owned());
                let connection = LiveConnection::new(options);
                connection.lose_live();
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while states.borrow().is_empty() {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(*states.borrow(), [json!(["disconnected", cause])], "listening {listening}");
                connection.close().await.unwrap();
            }
            drop(listener);
        })
        .await;
}
