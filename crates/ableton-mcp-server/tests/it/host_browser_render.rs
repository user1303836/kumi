use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions, ToolCall},
    live::*,
    registry::live_registry_operations,
};
use kumi_common::{abort::Signal, time::now_ms_f64};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
fn context(c: Option<&LiveOperationContext>) -> Value {
    let Some(c) = c else { return Value::Null };
    let mut v = json!({});
    if let Some(deadline) = c.deadline_ms {
        v["deadlineSeconds"] = json!(((deadline - now_ms_f64()) / 250.).round() / 4.);
    }
    if c.signal.is_some() {
        v["signal"] = json!(true);
    }
    v
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: RefCell<Value>,
    step: RefCell<Value>,
    defaults: Value,
    calls: RefCell<Vec<Value>>,
}
impl Adapter {
    fn fail(&self, kind: &str, reason: &str) -> Result<(), LiveError> {
        if self.step.borrow()["fail"] == kind {
            Err(LiveError::error(reason))
        } else {
            Ok(())
        }
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(serde_json::from_value(self.status.borrow().clone()).unwrap())
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
        self.fail("refresh", "request failed: offline")?;
        self.status()
    }
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"snapshot","context":context(c),"request":r}));
        self.fail("snapshot", "request failed: no snapshot")?;
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(json!({"kind":"invoke","invocation":i,"context":context(c)}));
        self.fail("invoke", "request failed: exact refusal")?;
        let step = self.step.borrow();
        if let Some(count) = step["returnCount"].as_u64() {
            return Ok(
                json!({"items":(0..count).map(|n|json!({"id":n.to_string(),"objectIdentity":format!("object-{n}"),"name":"Bass","category":"instruments","path":"Library/Instruments","isDevice":true,"extra":"discard"})).collect::<Vec<_>>()}),
            );
        }
        if let Some(result) = step.get("returns") {
            return Ok(result.clone());
        }
        if let Some(result) = self.defaults.get(&i.operation) {
            return Ok(result.clone());
        }
        self.sim.invoke_async(i, c).await
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
fn patch(target: &mut Value, patch: &Value) {
    if let Some(fields) = patch.as_object() {
        for (k, v) in fields {
            target[k] = v.clone();
        }
    }
}
fn clean(mut v: Value) -> Value {
    if let Some(s) = v["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(s) {
            v["result"]["content"][0]["text"] = body;
        }
    }
    v
}
#[tokio::test(flavor = "current_thread")]
async fn browser_ranking_cache_and_offline_render_match_source() {
    let data: Value = serde_json::from_str(include_str!("fixtures/host-browser-render-oracle.json")).unwrap();
    for (index, case) in data["cases"].as_array().unwrap().iter().enumerate() {
        let sim = DeterministicLiveSimulator::new();
        sim.state.borrow_mut()["tracks"][0]["mediaKind"] = json!("audio");
        patch(&mut sim.state.borrow_mut()["tracks"][0], &case["trackPatch"]);
        let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
        status["operations"] = json!(live_registry_operations());
        patch(&mut status, &case["statusPatch"]);
        let adapter = Rc::new(Adapter {
            sim,
            status: RefCell::new(status),
            step: RefCell::new(json!({})),
            defaults: data["defaults"].clone(),
            calls: Default::default(),
        });
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        for (number, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            *adapter.step.borrow_mut() = step.clone();
            adapter.calls.borrow_mut().clear();
            patch(&mut adapter.status.borrow_mut(), &step["statusPatch"]);
            patch(&mut adapter.sim.state.borrow_mut()["tracks"][0], &step["trackPatch"]);
            let signal = Signal::new();
            if step["abort"] == true {
                signal.cancel();
            }
            let call = ToolCall {
                id: json!(1),
                name: if step["tool"] == "render" { "live_render_offline" } else { "live_browser_search" }.into(),
                arguments: step.get("args").cloned(),
                asynchronous: true,
            };
            let result = host.dispatch_browser_render_tool(&call, Some(&signal)).await.unwrap();
            let result = match result {
                Ok(result) => json!({"result":clean(result.unwrap_or(Value::Null)),"calls":*adapter.calls.borrow()}),
                Err(error) => json!({"error":error.message(),"calls":*adapter.calls.borrow()}),
            };
            let expected = &data["pool"][case["results"][number].as_u64().unwrap() as usize];
            assert_eq!(
                canonical_mutation_identity(&result).unwrap(),
                canonical_mutation_identity(expected).unwrap(),
                "case {index} {} step {number}: {step}",
                case["label"]
            );
        }
    }
}

/// A walk of Live's Browser is kept ten minutes; a search it can't answer walks again, at most once a minute.
#[tokio::test(flavor = "current_thread")]
async fn a_kept_browser_walk_lasts_ten_minutes_and_a_search_it_cant_answer_walks_again() {
    let data: Value = serde_json::from_str(include_str!("fixtures/host-browser-render-oracle.json")).unwrap();
    let sim = DeterministicLiveSimulator::new();
    let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
    status["operations"] = json!(live_registry_operations());
    let adapter = Rc::new(Adapter {
        sim,
        status: RefCell::new(status),
        step: RefCell::new(json!({})),
        defaults: data["defaults"].clone(),
        calls: Default::default(),
    });
    let clock = Rc::new(std::cell::Cell::new(1_000_000.0));
    let now = clock.clone();
    let host = McpHost::new(adapter.clone(), McpHostOptions { now: Some(Rc::new(move || now.get())), ..Default::default() }).unwrap();
    let search = |query: &str| {
        let call =
            ToolCall { id: json!(1), name: "live_browser_search".into(), arguments: Some(json!({"query": query})), asynchronous: true };
        let host = &host;
        async move {
            clean(host.dispatch_browser_render_tool(&call, None).await.unwrap().unwrap().unwrap())["result"]["content"][0]["text"].clone()
        }
    };
    let walks = || adapter.calls.borrow().iter().filter(|call| call["kind"] == "invoke").count();
    let names =
        |body: &Value| body["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    let minutes = |n: f64| clock.set(clock.get() + n * 60_000.0);

    assert!(names(&search("bass").await).contains(&"Bass".to_owned()));
    assert_eq!(walks(), 1);
    minutes(9.0);
    let kept = search("amp").await;
    assert_eq!((walks(), &kept["fromCache"], &kept["cacheTtlSeconds"]), (1, &json!(true), &json!(600)), "nine minutes on, it's kept");
    // A device appears in the Browser (a pack installed, one Kumi made): a search for it walks again.
    let mut items = data["defaults"]["browser.search"]["items"].as_array().unwrap().clone();
    items.push(json!({"id":"fresh","objectIdentity":"object-fresh","name":"Fresh Reese","category":"instruments","path":"Library/Instruments","isDevice":true}));
    *adapter.step.borrow_mut() = json!({"returns": {"items": items}});
    let fresh = search("reese").await;
    assert_eq!((walks(), &fresh["fromCache"]), (2, &json!(false)));
    assert_eq!(names(&fresh), ["Fresh Reese"]);
    // A search for what isn't there, within a minute of that walk, doesn't walk again.
    minutes(0.5);
    assert!(names(&search("nothing like this").await).is_empty());
    assert_eq!(walks(), 2, "at most one walk a minute for searches that find nothing");
    // A minute on, one that shares words with what's kept but isn't in it walks again too.
    minutes(1.0);
    let mut items = adapter.step.borrow()["returns"]["items"].as_array().unwrap().clone();
    items.push(json!({"id":"gritty","objectIdentity":"object-gritty","name":"Gritty Reese Bass","category":"instruments","path":"Library/Instruments","isDevice":true}));
    *adapter.step.borrow_mut() = json!({"returns": {"items": items}});
    let gritty = search("gritty reese bass").await;
    assert_eq!(walks(), 3, "\"Bass\" and \"Fresh Reese\" share its words, but neither is it");
    assert_eq!(names(&gritty)[0], "Gritty Reese Bass");
    // A search that something kept answers by name doesn't walk.
    minutes(2.0);
    search("bass").await;
    assert_eq!(walks(), 3);
    minutes(11.0);
    search("bass").await;
    assert_eq!(walks(), 4, "past ten minutes, any search walks");
}
