use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions, ToolCall},
    live::*,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-clip-move-oracle.json")).unwrap()
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn clean(mut v: Value) -> Value {
    if let Some(s) = v["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(s) {
            v["result"]["content"][0]["text"] = body;
        }
    }
    fn walk(v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    if v.as_str().is_some_and(|v| v.starts_with("clipmove_")) {
                        *v = json!("$transaction");
                    } else if k == "expiresAt" {
                        *v = json!("$time");
                    } else {
                        walk(v)
                    }
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v)
                }
            }
            Value::String(s) if s.starts_with("clipmove_") => *v = json!("$transaction"),
            _ => {}
        }
    }
    walk(&mut v);
    v
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    calls: RefCell<Vec<Value>>,
    cache: RefCell<HashMap<String, Value>>,
    fault: RefCell<String>,
    fired: Cell<bool>,
    after_invoke: Cell<bool>,
    no_extension: Cell<bool>,
}
impl Adapter {
    fn new() -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            calls: Default::default(),
            cache: Default::default(),
            fault: Default::default(),
            fired: Cell::new(false),
            after_invoke: Cell::new(false),
            no_extension: Cell::new(false),
        }
    }
    fn status_now(&self) -> Result<LiveStatus, LiveError> {
        let mut status = self.sim.status()?;
        if self.no_extension.get() {
            status.operations.iter_mut().for_each(|operations| operations.retain(|operation| operation != "clip.clear-range"));
        }
        Ok(status)
    }
    fn reset(&self, fault: &str) {
        *self.fault.borrow_mut() = fault.into();
        self.fired.set(false);
        self.after_invoke.set(false);
    }
    fn invoke_impl(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let mut context = Value::Null;
        if let Some(c) = c {
            context = json!({"deadline":c.deadline_ms.is_some()});
            if let Some(k) = &c.idempotency_key {
                context["idempotencyKey"] = json!(k);
            }
            if let Some(k) = &c.transaction_id {
                context["transactionId"] = json!(k);
            }
        }
        self.calls.borrow_mut().push(clean(json!({"method":"invoke","invocation":i,"context":context})));
        let key =
            canonical_mutation_identity(&json!([c.and_then(|c| c.transaction_id.as_ref()), c.and_then(|c| c.idempotency_key.as_ref()), i]))
                .unwrap();
        if c.is_some() {
            if let Some(v) = self.cache.borrow().get(&key) {
                return Ok(v.clone());
            }
        }
        let fault = self.fault.borrow().clone();
        if !self.fired.get() && ["before", "cancel", "refusal"].iter().any(|s| fault.ends_with(s)) {
            self.fired.set(true);
            return Err(if fault.ends_with("refusal") {
                LiveError::MutationNotDispatched("mutation was not dispatched: ownership changed".into())
            } else {
                LiveError::error(if fault.ends_with("cancel") {
                    "operation cancelled before dispatch"
                } else {
                    "injected operation failure"
                })
            });
        }
        let no_effect = !self.fired.get() && fault.ends_with("no-effect");
        let mut result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(i)?
        };
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.after_invoke.set(true);
        if !self.fired.get() && fault.ends_with("after") {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && ["apply-invalid-identity", "apply-invalid-fingerprint"].contains(&fault.as_str()) {
            self.fired.set(true);
            let row = &mut result;
            row[if fault.ends_with("identity") { "objectIdentity" } else { "createdFingerprint" }] = json!("");
        }

        Ok(result)
    }
    fn read_fault(&self) -> Result<(), LiveError> {
        let fault = self.fault.borrow();
        if !self.fired.get() && ((fault.ends_with("-read") && self.after_invoke.get()) || fault.ends_with("-current-read")) {
            self.fired.set(true);
            return Err(LiveError::error("injected authoritative read failure"));
        }
        Ok(())
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.status_now()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.invoke_impl(i, None)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.sim.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"snapshot","request":r,"context":context(c)})));
        self.read_fault()?;
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"discover","request":r,"context":context(c)})));
        self.read_fault()?;
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"get","reference":r,"context":context(c)})));
        self.read_fault()?;
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke_impl(i, c)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"status","context":context(c)})));
        self.status_now()
    }
}

fn context(c: Option<&LiveOperationContext>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => {
            let mut value = json!({"deadline":c.deadline_ms.is_some()});
            if let Some(key) = &c.idempotency_key {
                value["idempotencyKey"] = json!(key);
            }
            if let Some(key) = &c.transaction_id {
                value["transactionId"] = json!(key);
            }
            value
        }
    }
}
fn setup(sim: &DeterministicLiveSimulator, kind: &str) {
    let mut s = sim.state.borrow_mut();
    let mut scene = s["scenes"][0].clone();
    scene["ref"] = json!("scene:scene-2");
    scene["objectIdentity"] = json!("simulator:scene:scene-2");
    scene["index"] = json!(1);
    scene["name"] = json!("Scene 2");
    s["scenes"].as_array_mut().unwrap().push(scene);
    let mut slot = s["tracks"][0]["clipSlots"][0].clone();
    slot["ref"] = json!("clip-slot:track-1:1");
    slot["objectIdentity"] = json!("simulator:clip-slot:track-1:1");
    slot["sceneIndex"] = json!(1);
    slot["clipRef"] = Value::Null;
    slot["empty"] = json!(true);
    s["tracks"][0]["clipSlots"].as_array_mut().unwrap().push(slot);
    if kind.ends_with("audio") {
        let clip = &mut s["tracks"][0]["clips"][0];
        clip["kind"] = json!("audio");
        clip["notes"] = json!([]);
        clip["filePath"] = json!("/mock/sample.wav");
    }
    if kind.starts_with("arrangement") {
        let mut clip = s["tracks"][0]["clips"][0].clone();
        clip["ref"] = json!("arrangement-clip:track-1:4");
        clip["objectIdentity"] = json!("simulator:arrangement-clip:0");
        clip["start"] = json!(4);
        s["arrangementClips"].as_array_mut().unwrap().push(json!({"trackRef":"track:track-1","clip":clip}));
    }
}
#[tokio::test]
async fn clip_move_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, if row["args"]["clipRef"] == "arrangement-clip:track-1:4" { "arrangement-midi" } else { "session-midi" });
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_clip_move_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_clip_move_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
            .await
            .unwrap()
            .unwrap()
            .unwrap_or(Value::Null);
        same(&clean(got), &row["result"], &format!("{index} {row}"));
    }
}
async fn perform(
    host: &McpHost,
    action: &str,
    key: &str,
    txid: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
    preabort: bool,
) {
    let id = json!(results.len() + 1);
    let args = json!({"transactionId":txid,"confirmation":action,"idempotencyKey":key});
    let signal = kumi_common::abort::Signal::new();
    if preabort {
        signal.cancel();
    }
    let result = if action == "apply" {
        host.live_clip_move_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_clip_move_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn clip_move_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        setup(&adapter.sim, kind);
        if scenario == "missing-identity" {
            {
                let mut s = adapter.sim.state.borrow_mut();
                let source =
                    if kind.starts_with("arrangement") { &mut s["arrangementClips"][0]["clip"] } else { &mut s["tracks"][0]["clips"][0] };
                source.as_object_mut().unwrap().remove("objectIdentity");
            }
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = if kind.starts_with("arrangement") {
            json!({"clipRef":"arrangement-clip:track-1:4","position":8})
        } else {
            json!({"clipRef":"clip:clip-1","targetTrackRef":"track:track-1","targetSceneIndex":1})
        };
        let preview = host.live_clip_move_preview_async(&json!(1), &args).await;
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if let Some(txid) = body.get("transactionId") {
            let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
            if scenario == "expire" {
                record.borrow_mut()["expiresAt"] = json!(0);
            }
            if scenario == "epoch" {
                adapter.sim.reconnect().unwrap();
            }
            {
                let mut s = adapter.sim.state.borrow_mut();
                let source =
                    if kind.starts_with("arrangement") { &mut s["arrangementClips"][0]["clip"] } else { &mut s["tracks"][0]["clips"][0] };
                match scenario {
                    "identity-edit" => source["objectIdentity"] = json!("other"),
                    "source-content" => source["name"] = json!("Manual"),
                    "position-edit" => source["start"] = json!(9),
                    "target-scene-identity" => s["scenes"][1]["objectIdentity"] = json!("other"),
                    "target-identity" => s["tracks"][0]["clipSlots"][1]["objectIdentity"] = json!("other"),
                    "target-occupied" => s["tracks"][0]["clipSlots"][1]["clipRef"] = json!("clip:external"),
                    _ => {}
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");
            {
                let t = record.borrow();
                let mut s = adapter.sim.state.borrow_mut();
                let reference = &t["created"]["ref"];
                let created = if kind.starts_with("arrangement") {
                    s["arrangementClips"].as_array_mut().unwrap().iter_mut().map(|r| &mut r["clip"]).find(|c| c["ref"] == *reference)
                } else {
                    s["tracks"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .flat_map(|t| t["clips"].as_array_mut().unwrap())
                        .find(|c| c["ref"] == *reference)
                };
                if let Some(created) = created {
                    if scenario == "undo-other-identity" {
                        created["objectIdentity"] = json!("other");
                    }
                    if scenario == "undo-position-edit" {
                        created["start"] = json!(12);
                    }
                    if scenario == "undo-other-content" {
                        created["name"] = json!("Manual");
                    }
                }
            }
            if scenario == "undo-target-occupied" {
                adapter.sim.state.borrow_mut()["tracks"][0]["clipSlots"][0]["clipRef"] = json!("clip:other");
            }
            if scenario == "undo-epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario.starts_with("undo-") && !["undo-other-identity", "undo-other-content", "undo-epoch"].contains(&scenario) {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, "undo", key, txid, &mut results, &mut states, &record, false).await;
            }
            adapter.reset("");
            perform(&host, "undo", "new-undo-key", txid, &mut results, &mut states, &record, false).await;
        }
        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}
/// Another clip on track 1's Arrangement: `name` from `start`, `length` beats long.
fn arrangement_clip(sim: &DeterministicLiveSimulator, name: &str, start: f64, length: f64, audio: bool) {
    let mut s = sim.state.borrow_mut();
    let mut clip = s["arrangementClips"][0]["clip"].clone();
    clip["ref"] = json!(format!("arrangement-clip:track-1:{name}"));
    clip["objectIdentity"] = json!(format!("simulator:arrangement-clip:{name}"));
    clip["name"] = json!(name);
    clip["start"] = json!(start);
    clip["length"] = json!(length);
    clip["looping"] = json!(false);
    if audio {
        clip["kind"] = json!("audio");
        clip["notes"] = json!([]);
    }
    s["arrangementClips"].as_array_mut().unwrap().push(json!({"trackRef":"track:track-1","clip":clip}));
}
/// Track 1's Arrangement clips as (name, start, end), in time order.
fn layout(sim: &DeterministicLiveSimulator) -> Vec<(String, f64, f64)> {
    let s = sim.state.borrow();
    let mut rows: Vec<_> = s["arrangementClips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let start = r["clip"]["start"].as_f64().unwrap();
            (r["clip"]["name"].as_str().unwrap().to_string(), start, start + r["clip"]["length"].as_f64().unwrap())
        })
        .collect();
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    rows
}
fn body(response: &Value) -> Value {
    response["result"]["content"][0]["text"].as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null)
}
async fn move_clip(host: &McpHost, position: f64) -> (Value, Value) {
    let preview = host.live_clip_move_preview_async(&json!(1), &json!({"clipRef":"arrangement-clip:track-1:4","position":position})).await;
    let shown = body(&preview);
    if shown["transactionId"].is_null() {
        return (preview, Value::Null);
    }
    let args = json!({"transactionId":shown["transactionId"],"confirmation":"apply","idempotencyKey":format!("apply-{position}")});
    let applied = host.live_clip_move_apply_async(&json!(2), &args, None).await.unwrap_or(Value::Null);
    (preview, applied)
}
#[tokio::test]
async fn an_arrangement_move_replaces_what_is_in_its_new_place() {
    // As dropping a clip in Live does. Live crashes when an Arrangement clip is copied onto a span a clip
    // holds, and a move is a copy, so what's there is cleared first.
    let adapter = Rc::new(Adapter::new());
    setup(&adapter.sim, "arrangement-midi");
    arrangement_clip(&adapter.sim, "Chorus", 12.0, 4.0, false);
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let (preview, applied) = move_clip(&host, 10.0).await;
    let shown = body(&preview);
    assert_eq!(shown["impact"], "moves-clip-replacing", "{preview}");
    assert_eq!(shown["replaces"], json!([{"name":"Chorus","start":12,"end":16,"from":12,"to":14,"whole":false}]));
    assert_eq!(body(&applied)["state"], "applied", "{applied}");
    let kick = "Kick Pattern".to_string();
    assert_eq!(layout(&adapter.sim), [(kick.clone(), 10.0, 14.0), ("Chorus".into(), 14.0, 16.0)]);
    // An undo would replace whatever is in the clip's old place now, so it leaves the clip where it is.
    arrangement_clip(&adapter.sim, "Fill", 4.0, 4.0, false);
    let txid = body(&preview)["transactionId"].clone();
    let undo =
        host.undo_clip_move_async(&json!(3), &json!({"transactionId":txid,"confirmation":"undo","idempotencyKey":"undo-key"}), None).await;
    assert!(undo.to_string().contains("(beats 4 to 8) is in the clip's old place now, so Kumi left the clip where it is"), "{undo}");
    assert_eq!(layout(&adapter.sim)[1], (kick, 10.0, 14.0));
}
#[tokio::test]
async fn an_audio_clip_crossing_the_new_place_is_cut_by_the_live_extension_first() {
    let adapter = Rc::new(Adapter::new());
    setup(&adapter.sim, "arrangement-audio");
    arrangement_clip(&adapter.sim, "Vox", 10.0, 8.0, true);
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let (middle, _) = move_clip(&host, 12.0).await;
    let refused = middle.to_string();
    assert!(refused.contains("Kumi can't move a clip into the middle of an audio clip yet"), "{refused}");
    assert!(refused.contains("Vox") && refused.contains("(beats 10 to 18) holds beats 12 to 16; pick a free spot"), "{refused}");
    adapter.no_extension.set(true);
    let (without, _) = move_clip(&host, 16.0).await;
    assert!(without.to_string().contains("Kumi can't cut into an audio clip without its Live extension yet"), "{without}");
    adapter.no_extension.set(false);
    let (preview, applied) = move_clip(&host, 16.0).await;
    assert_eq!(
        body(&preview)["payload"]["clearFirst"],
        json!([{"trackRef":"track:track-1","fromBeat":16,"toBeat":18,"expectedName":"Drums"}]),
        "{preview}"
    );
    assert_eq!(body(&applied)["state"], "applied", "{applied}");
    assert_eq!(layout(&adapter.sim), [("Vox".to_string(), 10.0, 16.0), ("Kick Pattern".into(), 16.0, 20.0)]);
}
#[tokio::test]
async fn cutting_into_a_looped_clip_waits_until_live_has_been_probed() {
    let adapter = Rc::new(Adapter::new());
    setup(&adapter.sim, "arrangement-midi");
    arrangement_clip(&adapter.sim, "Groove", 10.0, 8.0, false);
    arrangement_clip(&adapter.sim, "Fill", 21.0, 2.0, false);
    for row in [1, 2] {
        adapter.sim.state.borrow_mut()["arrangementClips"][row]["clip"]["looping"] = json!(true);
    }
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let (refused, _) = move_clip(&host, 12.0).await;
    let refused = refused.to_string();
    assert!(refused.contains("Kumi can't cut into a looped clip here yet") && refused.contains("crosses beats 12 to 16"), "{refused}");
    let (whole, applied) = move_clip(&host, 20.0).await;
    assert_eq!(body(&applied)["state"], "applied", "a looped clip inside the new place just goes: {whole}");
}
