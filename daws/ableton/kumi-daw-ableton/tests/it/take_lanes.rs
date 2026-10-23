//! Take lanes through Kumi's changes: a lane added to a track, a MIDI clip put in one, both left to Live's undo (Live's
//! API deletes neither), and a lane clip never read against the track's main lane.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{changes::CHANGES, integration::Ableton, options::AbletonOptions},
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

/// A bridge whose previews and applies answer as Live would for a lane and a clip in it, and whose track "Bass" has
/// "Verse" on its main lane at beats 16–32, under where the lane clip goes.
struct Bridge {
    calls: RefCell<Vec<(String, JsonObject)>>,
}
fn wrap(value: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&value)}],"structuredContent":value})).unwrap()
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.82"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let tools: Vec<Value> = ["live_status", "live_discover", "live_arrangement_clip_preview", "live_arrangement_clip_apply"]
            .iter()
            .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}))
            .collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), args.clone()));
        let lane = args.get("action") == Some(&json!("create-lane"));
        Ok(wrap(match name {
            "live_arrangement_clip_preview" if lane => json!({
                "transactionId":"arrclip_lane","epoch":7,"action":"create-lane","confirmation":"apply","expiresAt":9e15,
                "payload":{"trackRef":"7:track:1","name":"Comp"},"impact":"creates-take-lane-live-undo-only"
            }),
            "live_arrangement_clip_preview" => json!({
                "transactionId":"arrclip_clip","epoch":7,"action":"create","kind":"take-lane","confirmation":"apply","expiresAt":9e15,
                "payload":{"takeLaneRef":"7:take_lane:1:0","position":16,"length":8,"name":"Take A"},
                "takeLane":{"ref":"7:take_lane:1:0","name":"Comp"},"impact":"creates-take-lane-clip-no-undo"
            }),
            "live_arrangement_clip_apply" if args.get("transactionId") == Some(&json!("arrclip_lane")) => json!({
                "transactionId":"arrclip_lane","state":"applied","idempotent":false,
                "result":{"ref":"7:take_lane:1:0","objectIdentity":"live:lane","name":"Comp","index":0}
            }),
            "live_arrangement_clip_apply" => json!({
                "transactionId":"arrclip_clip","state":"applied","idempotent":false,
                "result":{"ref":"7:take_lane_clip:1:0:0","objectIdentity":"live:take","name":"Take A","start":16,"length":8}
            }),
            "live_status" => json!({"connected":true,"adapter":"remote","epoch":7}),
            "live_discover" if args.get("kind") == Some(&json!("arrangement-clip")) => json!({
                "epoch":7,"kind":"arrangement-clip","revision":"r1","truncated":false,
                "items":[{"ref":"7:arrangement_clip:1:0","name":"Verse","start":16,"endTime":32,"length":16,"objectIdentity":"live:verse"}]
            }),
            _ => json!({"epoch":7,"kind":args.get("kind"),"items":[],"revision":"r1","truncated":false}),
        }))
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

/// Each change, with what reached the bridge and the records Kumi kept.
async fn change(tool: &str, input: Value) -> (Value, Vec<String>, Vec<Value>) {
    let bridge = Rc::new(Bridge { calls: RefCell::new(vec![]) });
    let records = Rc::new(RefCell::new(vec![]));
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    let endpoint = bridge.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    options.now = Some(Rc::new(|| chrono::DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
    options.change_timeout_ms = Some(50);
    let out = records.clone();
    options.on_change = Some(Rc::new(move |record| out.borrow_mut().push(json!(record))));
    let integration = Ableton::new(options);
    let connection = integration.connection.clone();
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.epoch.set(Some(7.0));
    {
        let mut book = connection.references.borrow_mut();
        for (reference, kind) in [("7:track:1", "track"), ("7:take_lane:1:0", "take_lane")] {
            book.refs.insert(reference.into(), kind.into());
        }
        book.known.insert("7:track:1".into(), serde_json::from_value(json!({"name":"Bass"})).unwrap());
    }
    let kind = CHANGES.iter().find(|kind| kind.tool == tool).unwrap();
    let outcome = integration.mutations.change(kind, input.as_object().unwrap().clone(), Signal::new(), false).await;
    let called = bridge.calls.borrow().iter().map(|(name, args)| format!("{name} {}", stringify(&json!(args)))).collect();
    let records = records.borrow().clone();
    (json!(outcome), called, records)
}

#[tokio::test(flavor = "current_thread")]
async fn a_take_lane_is_added_and_a_midi_clip_put_in_one() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // The lane: Live's undo takes it back, so Kumi keeps it rather than offering an undo it can't do.
            let (outcome, called, records) = change("add_take_lane", json!({"trackRef":"7:track:1","name":"Comp"})).await;
            assert_eq!(outcome["isError"], json!(false), "{outcome}");
            let preview = |called: &[String]| {
                called.iter().find(|call| call.starts_with("live_arrangement_clip_preview")).cloned().unwrap_or_default()
            };
            assert_eq!(
                preview(&called),
                r#"live_arrangement_clip_preview {"action":"create-lane","trackRef":"7:track:1","name":"Comp"}"#,
                "{called:?}"
            );
            let record = records.last().unwrap();
            assert_eq!(
                (record["title"].clone(), record["state"].clone()),
                (json!("New take lane “Comp” on Bass"), json!("kept")),
                "{record}"
            );
            assert_eq!(record["note"], json!("Live's API can't delete a take lane; Live's own undo takes it back."));
            // A MIDI clip in it, over beats where the main lane has "Verse": the lane clip lands on nothing there, so
            // the main lane isn't read and nothing is said replaced; Live's undo takes it back, so it's kept too.
            let input = json!({"trackRef":"7:track:1","takeLaneRef":"7:take_lane:1:0","position":16,"length":8,"name":"Take A"});
            let (outcome, called, records) = change("add_arrangement_clip", input).await;
            assert_eq!(outcome["isError"], json!(false), "{outcome}");
            assert!(called.iter().all(|call| !call.starts_with("live_discover")), "the main lane isn't read for a lane clip: {called:?}");
            assert!(preview(&called).contains(r#""takeLaneRef":"7:take_lane:1:0""#), "{called:?}");
            let record = records.last().unwrap();
            assert_eq!(
                (record["title"].clone(), record["state"].clone()),
                (json!("New Arrangement clip “Take A” in take lane “Comp” on Bass at bar 5 (2 bars)"), json!("kept")),
                "{record}"
            );
            assert_eq!(record["note"], json!("Live's API can't delete a clip in a take lane; Live's own undo takes it back."));
            // An audio file asked for in a lane goes to import_audio, before the bridge is asked anything.
            let input = json!({"trackRef":"7:track:1","takeLaneRef":"7:take_lane:1:0","position":16,"sample":"/samples/Vox.wav"});
            let (outcome, called, _) = change("add_arrangement_clip", input).await;
            assert_eq!(outcome["isError"], json!(true), "{outcome}");
            assert!(outcome["text"].as_str().unwrap().contains("use import_audio with its takeLaneRef"), "{outcome}");
            assert!(called.iter().all(|call| !call.starts_with("live_arrangement_clip")), "{called:?}");
        })
        .await;
}
