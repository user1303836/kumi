use async_trait::async_trait;
use futures::FutureExt;
use indexmap::IndexMap;
use kumi_agent::{
    core::contracts::*,
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_daw_ableton::{
    connection::{ConnectionOptions, LiveConnection},
    observation::{ObservationHost, ObservedChange, Observer},
    remember::Remember,
};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
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
        let fixture:Value=serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
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
                let hints=config.get("hints").map(|h|ObserveHints{pinned:h.get("pinned").map(|v|serde_json::from_value(v.clone()).unwrap()),continuing:h["continuing"].as_bool(),..ObserveHints::default()});
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
    /// Each call, as (tool, args).
    calls: RefCell<Vec<(String, JsonObject)>>,
    /// How many times the tools were listed, and who hears that they changed.
    lists: Cell<usize>,
    catalog_changed: RefCell<Vec<Rc<dyn Fn()>>>,
}
impl Replies {
    fn new(replies: std::collections::HashMap<String, Value>) -> Self {
        Self { replies, sets: Cell::new(1), calls: RefCell::new(vec![]), lists: Cell::new(0), catalog_changed: RefCell::new(vec![]) }
    }
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
        self.lists.set(self.lists.get() + 1);
        let names = ["live_status", "live_discover", "live_song_state"];
        Ok(serde_json::from_value(
            json!({"tools":names.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        self.calls.borrow_mut().push((name.into(), args.clone()));
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
    fn on_catalog_changed(&self, callback: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.catalog_changed.borrow_mut().push(callback);
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
            let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
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
            let endpoint = Rc::new(Replies::new(replies));
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

#[tokio::test(flavor = "current_thread")]
async fn once_live_reports_track_ids_the_next_look_reads_them_with_the_tracks() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
            let case = fixture["cases"].as_array().unwrap().iter().find(|case| case["label"] == "time-signature").unwrap();
            let mut replies: std::collections::HashMap<String, Value> = case["calls"]
                .as_array()
                .unwrap()
                .iter()
                .zip(case["responses"].as_array().unwrap())
                .map(|(call, id)| {
                    let key = format!("{}:{}", call["name"].as_str().unwrap(), call["args"]["kind"].as_str().unwrap_or(""));
                    (key, fixture["values"][id.as_u64().unwrap() as usize]["reply"].clone())
                })
                .collect();
            // Live says Main's id when asked: it reports track ids.
            let main = json!({"epoch":7,"kind":"main-track","revision":"r","truncated":false,"items":[{"ref":"7:main_track:0","kumiTrack":"01JMAIN0000000000000000000"}]});
            replies.insert("live_discover:main-track".into(), json!({"content":[{"type":"text","text":stringify(&main)}],"structuredContent":main}));
            let endpoint = Rc::new(Replies::new(replies));
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
            // The background pass asks once whether Live reports ids.
            for _ in 0..200 {
                if endpoint.calls.borrow().iter().any(|(_, args)| args.get("kind") == Some(&json!("main-track"))) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            for _ in 0..20 {
                tokio::task::yield_now().await;
            }
            endpoint.calls.borrow_mut().clear();
            observer.observe(endpoint.as_ref(), Signal::new(), None).await.unwrap();
            let tracks = endpoint.calls.borrow().iter().find(|(name, args)| name == "live_discover" && args.get("kind") == Some(&json!("track"))).cloned();
            let fields = tracks.expect("the look read the tracks").1["fields"].clone();
            assert!(fields.as_array().unwrap().contains(&json!("kumiTrack")), "{fields}");
            remember.cancel_timer();
            connection.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_turn_lists_the_bridges_tools_again_only_once_they_changed() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
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
            let endpoint = Rc::new(Replies::new(replies));
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
            observer.observe(endpoint.as_ref(), Signal::new(), None).await.unwrap();
            assert_eq!(endpoint.lists.get(), 1, "two turns, one listing");
            // The bridge says its tools changed (notifications/tools/list_changed): the next turn lists them again.
            let heard = endpoint.catalog_changed.borrow().clone();
            for listener in heard {
                listener();
            }
            observer.observe(endpoint.as_ref(), Signal::new(), None).await.unwrap();
            assert_eq!(endpoint.lists.get(), 2, "listed again once they changed");
            remember.cancel_timer();
            connection.close().await.unwrap();
        })
        .await;
}

// W5-5 (review points 3 and 4): what `@` offers of the Set comes from its model, with no read of its own. A reference
// read at this epoch reaches the look where it is; one named before Live's references changed is found again, a track
// by its name among the Set's and a device in its track's tree read again, or else told by its name.
#[tokio::test(flavor = "current_thread")]
async fn a_reference_reaches_the_look_and_one_live_renumbered_is_found_again_or_told_by_name() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use kumi_agent::core::references::{Candidate, CandidateKind};
            let fixture: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
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
            let endpoint = Rc::new(Replies::new(replies));
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
            let model = observer.model();
            let calls = endpoint.calls.borrow().len();
            let offered = kumi_daw_ableton::pins::candidates(&model, None);
            assert_eq!(endpoint.calls.borrow().len(), calls, "offered from the model alone");
            assert_eq!(offered.iter().filter(|candidate| candidate.kind == CandidateKind::Track).count(), model.tracks.len());
            let node = |candidate: &Candidate| match &candidate.reference {
                Reference::Daw { node } => node.clone(),
                other => panic!("not the Set's: {other:?}"),
            };
            let first = &model.tracks[0];
            let track = node(&offered[0]);
            assert_eq!((track.r#ref.as_str(), track.name.as_str()), (first.reference.as_str(), first.name.as_str()));
            let references = |references: Vec<PinnedNode>| ObserveHints { references, ..ObserveHints::default() };
            let told = |seen: Observation| serde_json::from_str::<Value>(&seen.context).unwrap()["references"].clone();

            // Read at this epoch: where it is.
            let seen = observer.observe(endpoint.as_ref(), Signal::new(), Some(references(vec![track.clone()]))).await.unwrap();
            let now = told(seen);
            assert_eq!(now[0]["said"], format!("@{}", first.name));
            assert!(now[0].get("gone").is_none() && now[0]["ref"].as_str().is_some(), "{now}");

            // Named before Live's references changed: the track found again by its name, one the Set doesn't have told by
            // its name, and a device found again in its track's tree, read under the track's ref now.
            let renumbered = PinnedNode { r#ref: "999:track:0".into(), track_ref: "999:track:0".into(), ..track.clone() };
            let unknown = PinnedNode {
                r#ref: "999:track:7".into(),
                track_ref: "999:track:7".into(),
                name: "Not In The Set".into(),
                track: Some("Not In The Set".into()),
                ..track.clone()
            };
            let missing = PinnedNode {
                r#ref: "999:device:0:9".into(),
                track_ref: "999:track:0".into(),
                node: PinKind::Device,
                name: "No Such Device".into(),
                ..track.clone()
            };
            let seen =
                observer.observe(endpoint.as_ref(), Signal::new(), Some(references(vec![renumbered, unknown, missing]))).await.unwrap();
            let then = told(seen);
            assert_eq!(then[0]["ref"], now[0]["ref"], "the same track, found by its name: {then}");
            assert!(then[0].get("gone").is_none(), "{then}");
            let gone = then[1]["gone"].as_str().unwrap();
            assert!(gone.starts_with("The producer named “Not In The Set” with @, and Live's tracks changed since"), "{gone}");
            assert!(then[2]["gone"].as_str().is_some_and(|gone| gone.contains("“No Such Device” on “")), "{then}");
            assert!(then.as_array().unwrap().iter().all(|told| told["said"].as_str().is_some_and(|said| said.starts_with('@'))));
            remember.cancel_timer();
            connection.close().await.unwrap();
        })
        .await;
}

// #700 review: Live's refs are places, within an epoch too, so a reference is checked against this look's model. A track
// inserted above @Bass leaves it naming Bass where Bass is now, its device too; Bass deleted, both are told by name.
#[test]
fn a_reference_follows_its_track_when_one_is_inserted_above_and_is_told_by_name_once_deleted() {
    use kumi_daw_ableton::{
        pins::{candidates, reference_context},
        references::References,
        set_model::SetModel,
    };
    let row = |value: Value| value.as_object().unwrap().clone();
    let model = |tracks: &[(&str, &str)], devices: &[(&str, &str, &str)]| {
        let tracks: Vec<JsonObject> = tracks.iter().map(|(at, name)| row(json!({"ref": at, "name": name}))).collect();
        let devices: Vec<JsonObject> =
            devices.iter().map(|(at, parent, name)| row(json!({"ref": at, "parentRef": parent, "name": name}))).collect();
        SetModel::next(&SetModel::default(), &tracks, &devices, true)
    };
    let before = model(&[("1:track:0", "Drums"), ("1:track:1", "Bass")], &[("1:device:1:0", "1:track:1", "Saturator")]);
    let offered = candidates(&before, None);
    let node = |label: &str| match &offered.iter().find(|candidate| candidate.label == label).unwrap().reference {
        Reference::Daw { node } => node.clone(),
        other => panic!("not the Set's: {other:?}"),
    };
    let (bass, saturator) = (node("Bass"), node("Saturator"));
    let mut book = References::default();
    // Unchanged: where it is.
    let told = reference_context(&bass, None, &before, &mut book);
    assert_eq!((told["said"].clone(), told["ref"].clone()), (json!("@Bass"), json!(book.short_ref("1:track:1"))));
    // A track inserted above Bass at the same epoch: Bass is track 2, its Saturator device 2:0, and 1:track:1 is Keys.
    let after =
        model(&[("1:track:0", "Drums"), ("1:track:1", "Keys"), ("1:track:2", "Bass")], &[("1:device:2:0", "1:track:2", "Saturator")]);
    let told = reference_context(&bass, None, &after, &mut book);
    assert_eq!(told["ref"], json!(book.short_ref("1:track:2")), "{told:?}");
    let told = reference_context(&saturator, None, &after, &mut book);
    assert_eq!(told["ref"], json!(book.short_ref("1:device:2:0")), "{told:?}");
    // Bass deleted: both told by their names, with no ref to act on.
    let deleted = model(&[("1:track:0", "Drums"), ("1:track:1", "Keys")], &[]);
    let told = reference_context(&bass, None, &deleted, &mut book);
    assert!(told["gone"].as_str().unwrap().starts_with("The producer named “Bass” with @, and Live's tracks changed since"), "{told:?}");
    assert!(told.get("ref").is_none());
    let told = reference_context(&saturator, None, &deleted, &mut book);
    let gone = told["gone"].as_str().unwrap();
    assert!(gone.starts_with("The producer named “Saturator” on “Bass” with @, and Live's devices changed since"), "{gone}");

    // Two of one name: the second Saturator named, then the Compressor between them removed. Its ref now names the
    // place after the first, and two Saturators fit its name: which is meant can't be told, so it's told by its name.
    let two = model(
        &[("1:track:0", "Bass")],
        &[
            ("1:device:0:0", "1:track:0", "Saturator"),
            ("1:device:0:1", "1:track:0", "Compressor"),
            ("1:device:0:2", "1:track:0", "Saturator"),
        ],
    );
    let offered = candidates(&two, None);
    let second = match &offered.iter().filter(|candidate| candidate.label == "Saturator").nth(1).unwrap().reference {
        Reference::Daw { node } => node.clone(),
        other => panic!("not the Set's: {other:?}"),
    };
    assert_eq!(second.r#ref, "1:device:0:2");
    let told = reference_context(&second, None, &two, &mut book);
    assert_eq!(told["ref"], json!(book.short_ref("1:device:0:2")), "at its ref while it's there: {told:?}");
    let removed =
        model(&[("1:track:0", "Bass")], &[("1:device:0:0", "1:track:0", "Saturator"), ("1:device:0:1", "1:track:0", "Saturator")]);
    let told = reference_context(&second, None, &removed, &mut book);
    assert!(told["gone"].as_str().is_some() && told.get("ref").is_none(), "{told:?}");
}
