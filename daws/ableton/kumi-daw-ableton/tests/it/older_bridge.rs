//! A change an older bridge can't do (one from before Arrangement editing, its clip move taking no keepSource) is
//! refused in the older-bridge words, before anything is sent; a bridge that can takes it as it is.
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

/// A bridge at `version` whose clip move does (`copies`) or doesn't take keepSource, answering every preview with an
/// error (so a change that reaches it stops there).
struct Bridge {
    version: &'static str,
    copies: bool,
    calls: RefCell<Vec<String>>,
}
fn wrap(value: Value, error: bool) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&value)}],"structuredContent":value,"isError":error})).unwrap()
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":self.version})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let mut tools: Vec<Value> =
            ["live_status", "live_discover", "live_clip_move_apply", "live_note_update_preview", "live_note_update_apply"]
                .iter()
                .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}))
                .collect();
        let mut properties = json!({"clipRef":{"type":"string"},"position":{"type":"number"},"targetTrackRef":{"type":"string"},"targetSceneIndex":{"type":"integer"}});
        if self.copies {
            properties["keepSource"] = json!({"type":"boolean"});
        }
        tools.push(json!({"name":"live_clip_move_preview","inputSchema":{"type":"object","properties":properties}}));
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, _: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push(name.into());
        Ok(match name {
            "live_status" => wrap(json!({"connected":true,"adapter":"remote","epoch":7}), false),
            _ => wrap(json!({"reason":"reached the bridge","remediation":"none"}), true),
        })
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

/// A change against a bridge: what Kumi said, and whether its preview reached the bridge.
async fn change(copies: bool, tool: &str, input: Value) -> (String, bool) {
    let bridge = Rc::new(Bridge { version: if copies { "1.0.83" } else { "1.0.81" }, copies, calls: RefCell::new(vec![]) });
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    let endpoint = bridge.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    options.change_timeout_ms = Some(50);
    let integration = Ableton::new(options);
    let connection = integration.connection.clone();
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.epoch.set(Some(7.0));
    {
        let mut book = connection.references.borrow_mut();
        for (reference, kind) in [("7:arrangement_clip:1:0", "arrangement-clip"), ("7:clip:1:0", "clip"), ("7:track:2", "track")] {
            book.refs.insert(reference.into(), kind.into());
        }
    }
    let kind = CHANGES.iter().find(|kind| kind.tool == tool).unwrap();
    let outcome = integration.mutations.change(kind, input.as_object().unwrap().clone(), Signal::new(), false).await;
    let reached = bridge.calls.borrow().iter().any(|call| call.ends_with("_preview"));
    (outcome.text, reached)
}

#[tokio::test(flavor = "current_thread")]
async fn what_an_older_bridge_cant_do_is_said_as_such_before_anything_is_sent() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let arrangement = "7:arrangement_clip:1:0";
            let older = |what: &str| {
                format!("That needs a newer Ableton bridge than this one (1.0.81): {what}. Tell the producer to update it (kumi doctor says how).")
            };
            for (tool, input, what) in [
                ("move_clip", json!({"clipRef":arrangement,"position":8,"keepSource":true}), "copying an Arrangement clip"),
                ("move_clip", json!({"clipRef":arrangement,"position":8,"targetTrackRef":"7:track:2"}), "moving an Arrangement clip to another track"),
                ("change_notes", json!({"clipRef":arrangement,"notes":[{"id":1,"pitch":60}]}), "editing an Arrangement clip's notes"),
            ] {
                let (said, reached) = change(false, tool, input.clone()).await;
                assert!(said.contains(&older(what)), "{tool} {input}: {said}");
                assert!(!reached, "{tool}: nothing reaches an older bridge");
                // A bridge that can takes it as it is.
                let (said, reached) = change(true, tool, input.clone()).await;
                assert!(reached && !said.contains("newer Ableton bridge"), "{tool} {input}: {said}");
            }
            // What an older bridge does as asked still goes to it: a move on its own track, a Session clip's notes.
            for (tool, input) in [
                ("move_clip", json!({"clipRef":arrangement,"position":8})),
                ("change_notes", json!({"clipRef":"7:clip:1:0","notes":[{"id":1,"pitch":60}]})),
            ] {
                let (said, reached) = change(false, tool, input.clone()).await;
                assert!(reached && !said.contains("newer Ableton bridge"), "{tool} {input}: {said}");
            }
        })
        .await;
}
