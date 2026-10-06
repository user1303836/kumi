use async_trait::async_trait;
use futures::FutureExt;
use indexmap::IndexMap;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_runtime::{
    core::{contracts::*, errors::RuntimeError},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        observation::{ObservationHost, ObservedChange, Observer},
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
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
    config: RefCell<Value>,
    calls: RefCell<Vec<Value>>,
    disconnects: RefCell<Vec<Listener>>,
    owner: RefCell<Weak<LiveConnection>>,
    changes: RefCell<IndexMap<String, ObservedChange>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(
            serde_json::from_value(
                json!({"name":"fixture","version":self.case["settings"].get("version").cloned().unwrap_or(json!("1.0.73"))}),
            )
            .unwrap(),
        )
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        let names = ["live_status", "live_discover", "live_song_state", "live_project_info"];
        Ok(serde_json::from_value(json!({"tools":names.iter().filter(|name|!self.config.borrow()["missing"].as_array().is_some_and(|a|a.contains(&json!(name)))).map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let index = self.calls.borrow().len();
        let call = json!({"name":name,"args":args});
        self.calls.borrow_mut().push(call.clone());
        eq(&call, &self.case["calls"][index], &format!("{} dispatch {index}", self.case["label"]));
        let entry = &self.case["responses"][index];
        match entry["effect"].as_str() {
            Some("invalidate") => self.owner.borrow().upgrade().unwrap().invalidate(),
            Some("disconnect") => {
                for listener in self.disconnects.borrow().clone() {
                    if listener.on.get() {
                        (listener.callback)();
                    }
                }
            }
            Some("abort") => {
                signal.cancel();
                return Err(RuntimeError::Aborted);
            }
            _ => {}
        }
        tokio::task::yield_now().await;
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
        Ok(())
    }
}
#[async_trait(?Send)]
impl ObservationHost for Fixture {
    fn reset_turn(&self, _: bool) {}
    fn changes(&self) -> Vec<ObservedChange> {
        self.changes.borrow().values().cloned().collect()
    }
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>> {
        Vec::new()
    }
    async fn restore_after_crash(&self, _: &str, _: Option<&str>, _: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
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
fn observation(value: Observation) -> Value {
    let mut row = json!({"key":value.key,"label":value.label,"context":value.context,"instructions":value.instructions,"tools":[]})
        .as_object()
        .unwrap()
        .clone();
    if let Some(v) = value.revision {
        row.insert("revision".into(), json!(v));
    }
    if let Some(v) = value.project {
        row.insert("project".into(), json!(v));
    }
    if let Some(v) = value.tracks {
        row.insert("tracks".into(), json!(v));
    }
    if let Some(v) = value.saved_at {
        row.insert("savedAt".into(), json!(v));
    }
    Value::Object(row)
}
#[tokio::test(flavor = "current_thread")]
async fn observation_context_dispatches_and_authority_match_source() {
    tokio::task::LocalSet::new().run_until(async{
        let fixture:Value=serde_json::from_str(include_str!("support/observation-oracle.json")).unwrap();
        let(named_turns,whole_turns)=(std::cell::Cell::new(0),std::cell::Cell::new(0));
        for original in fixture["cases"].as_array().unwrap(){
            let mut case=original.clone();case["responses"]=json!(case["responses"].as_array().unwrap().iter().map(|id|fixture["values"][id.as_u64().unwrap() as usize].clone()).collect::<Vec<_>>());
            let endpoint=Rc::new(Fixture{case:case.clone(),config:RefCell::new(json!({})),calls:RefCell::new(Vec::new()),disconnects:RefCell::new(Vec::new()),owner:RefCell::new(Weak::new()),changes:RefCell::new(IndexMap::new())});
            let states=Rc::new(RefCell::new(Vec::<Value>::new()));let out=states.clone();let mut options=ConnectionOptions::new(Rc::new(move|state,cause|out.borrow_mut().push(json!([state,cause]))));
            let out=endpoint.clone();options.connect=Some(Rc::new(move|_|{let endpoint:Rc<dyn McpEndpoint>=out.clone();async move{Ok(endpoint)}.boxed_local()}));
            options.now=Some(Rc::new(||chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));options.generation=Some("connection".into());options.reconnect_interval_ms=Some(3600000);
            let connection=LiveConnection::new(options);*endpoint.owner.borrow_mut()=Rc::downgrade(&connection);connection.start(Signal::new()).await.unwrap();
            let remember=Remember::new(connection.clone(),None,None);let observer=Observer::new(connection.clone(),remember.clone());
            for (index,config) in case["turns"].as_array().unwrap().iter().enumerate(){
                *endpoint.config.borrow_mut()=config.clone();
                for(key,cell) in [("reconnected",&connection.reconnected),("lost",&connection.lost),("available",&connection.available)]{if let Some(v)=config[key].as_bool(){cell.set(v)}}
                for change in config["changes"].as_array().into_iter().flatten(){let record:ChangeRecord=serde_json::from_value(change["record"].clone()).unwrap();endpoint.changes.borrow_mut().insert(record.id.clone(),ObservedChange{record,within:change["within"].as_str().is_some_and(|s|!s.is_empty())});}
                let signal=Signal::new();if config["abort"]==true{signal.cancel();}
                let hints=config.get("hints").map(|h|ObserveHints{pinned:h.get("pinned").map(|v|serde_json::from_value(v.clone()).unwrap()),continuing:h["continuing"].as_bool()});
                let value=match observer.observe(endpoint.as_ref(),signal,hints).await{Ok(v)=>{
                    // The Set model is built from the same rows: the tracks the observation names, in order (a row
                    // without a ref can't be found again, so the model leaves it out).
                    if let Some(names)=&v.tracks{let model=observer.model();let mut named=names.iter();assert!(model.tracks.iter().all(|t|named.any(|n|*n==t.name)),"{} turn {index}: the Set model {:?} against {names:?}",case["label"],model.tracks.iter().map(|t|&t.name).collect::<Vec<_>>());named_turns.set(named_turns.get()+1);if model.tracks.len()==names.len(){whole_turns.set(whole_turns.get()+1);}}
                    // A turn that read no whole track list leaves the model marked as out of date.
                    if v.tracks.is_none(){assert!(!observer.model().complete,"{} turn {index}: the Set model is marked incomplete",case["label"]);}
                    observation(v)
                },Err(RuntimeError::Aborted)=>json!({"error":"cancelled"}),Err(e)=>{
                    // A turn that failed part way (and wasn't overtaken by a newer one) leaves the model marked as out of date.
                    let message=e.to_string();if !message.contains("late refresh discarded"){assert!(!observer.model().complete,"{} turn {index}: {message}",case["label"]);}
                    json!({"error":message})
                }};
                let state={let book=connection.references.borrow();json!({"epoch":connection.epoch.get(),"lease":connection.lease.get(),"lastTrackCount":observer.last_track_count.get(),"currentTempo":observer.tempo.get(),"beatsPerBar":observer.beats_per_bar.get(),"refs":book.refs.iter().collect::<Vec<_>>(),"cursors":book.cursors.iter().collect::<Vec<_>>(),"known":book.known.iter().collect::<Vec<_>>()})};
                let label=format!("{} turn {index}",case["label"]);eq(&value,&case["results"][index]["value"],&label);eq(&state,&case["results"][index]["state"],&format!("{label} state"));
            }
            remember.cancel_timer();connection.close().await.unwrap();
            eq(&json!(*endpoint.calls.borrow()),&case["calls"],&format!("{} all dispatches",case["label"]));eq(&json!(*states.borrow()),&case["states"],&format!("{} connection states",case["label"]));
        }
        assert!(named_turns.get()>10&&whole_turns.get()+3>=named_turns.get(),"the Set model has every track in all but the malformed turns: {} of {}",whole_turns.get(),named_turns.get());
    }).await;
}

/// Answers each call with the oracle's reply for its tool and kind, in any order. With `sets` above 1, the Set read
/// returns that many Sets, as it can while Live switches Sets.
struct Replies {
    replies: std::collections::HashMap<String, Value>,
    sets: Cell<usize>,
}
#[async_trait(?Send)]
impl McpEndpoint for Replies {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        let names = ["live_status", "live_discover", "live_song_state"];
        Ok(serde_json::from_value(
            json!({"tools":names.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let key = format!("{name}:{}", args.get("kind").and_then(Value::as_str).unwrap_or(""));
        let mut reply = self.replies.get(&key).cloned().ok_or_else(|| RuntimeError::plain(format!("no reply for {key}")))?;
        if key == "live_discover:set" && self.sets.get() > 1 {
            let mut body = reply["structuredContent"].clone();
            let mut other = body["items"][0].clone();
            other["ref"] = json!("7:set:1");
            other["objectIdentity"] = json!("another song");
            body["items"].as_array_mut().unwrap().push(other);
            reply = json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body});
        }
        Ok(serde_json::from_value(reply).unwrap())
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
#[async_trait(?Send)]
impl ObservationHost for Replies {
    fn reset_turn(&self, _: bool) {}
    fn changes(&self) -> Vec<ObservedChange> {
        Vec::new()
    }
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>> {
        Vec::new()
    }
    async fn restore_after_crash(&self, _: &str, _: Option<&str>, _: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_turn_that_fails_part_way_leaves_the_set_model_marked_out_of_date() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("support/observation-oracle.json")).unwrap();
            let case = fixture["cases"].as_array().unwrap().iter().find(|case| case["label"] == "time-signature").unwrap();
            let replies = case["calls"]
                .as_array()
                .unwrap()
                .iter()
                .zip(case["responses"].as_array().unwrap())
                .map(|(call, id)| {
                    let key = format!("{}:{}", call["name"].as_str().unwrap(), call["args"]["kind"].as_str().unwrap_or(""));
                    (key, fixture["values"][id.as_u64().unwrap() as usize]["reply"].clone())
                })
                .collect();
            let endpoint = Rc::new(Replies { replies, sets: Cell::new(1) });
            let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
            let out = endpoint.clone();
            options.connect = Some(Rc::new(move |_| {
                let endpoint: Rc<dyn McpEndpoint> = out.clone();
                async move { Ok(endpoint) }.boxed_local()
            }));
            options.reconnect_interval_ms = Some(3600000);
            let connection = LiveConnection::new(options);
            connection.start(Signal::new()).await.unwrap();
            let remember = Remember::new(connection.clone(), None, None);
            let observer = Observer::new(connection.clone(), remember.clone());
            observer.observe(endpoint.as_ref(), Signal::new(), None).await.unwrap();
            assert!(observer.model().complete && observer.model().tracks.len() == 3);
            // Live switching Sets: the Set read returns two, so the turn fails, and the model no longer claims to be
            // the whole of the Set it last read.
            endpoint.sets.set(2);
            let Err(error) = observer.observe(endpoint.as_ref(), Signal::new(), None).await else { panic!("two Sets read as one") };
            let error = error.to_string();
            assert!(error.contains("one authoritative Set") && !observer.model().complete, "{error}");
            remember.cancel_timer();
            connection.close().await.unwrap();
        })
        .await;
}
