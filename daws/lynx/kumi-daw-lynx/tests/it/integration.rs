use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::{
    core::{
        context::IntegrationContext,
        refs::{Moved, RefScheme, Shift},
    },
    kernel::schema::problem,
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use kumi_common::abort::Signal;
use kumi_daw_lynx::{changes::KINDS, create_lynx_integration, LynxRefs, DAW, PRESENTATION};
use kumi_daw_own::bridge::OwnOptions;
use kumi_types::{
    contracts::{ChangeFamily, ChangeState, JsonObject},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[test]
fn positional_refs_are_revision_scoped_and_never_carried_to_a_different_channel() {
    let refs = LynxRefs;
    assert!(refs.positional());
    assert_eq!(refs.kind_of("l:track:0:7"), Some("track"));
    assert_eq!(refs.kind_of("l:device:mixer:2:1:7"), Some("device"));
    for invalid in ["l:track:0", "l:track::7", "l:track:0:old", "r:track:0:7"] {
        assert_eq!(refs.kind_of(invalid), None, "{invalid}");
    }
    assert_eq!(refs.refs_in("move l:track:0:7, inspect l:device:mixer:2:1:7."), ["l:track:0:7", "l:device:mixer:2:1:7"]);
    assert_eq!(refs.moved(&Shift::default(), "l:track:0:7"), Moved::Gone);
    assert_eq!(refs.track_of("l:track:0:7"), Some("l:track:0:7".into()));
    assert_eq!(refs.track_of("l:pattern:0:7"), None);
}

#[test]
fn instrument_changes_require_the_planned_revision_and_bounded_catalog_destinations() {
    let create = KINDS.iter().find(|kind| kind.tool == "lynx_channel_create").unwrap();
    let replace = KINDS.iter().find(|kind| kind.tool == "lynx_instrument_replace").unwrap();
    for input in [
        json!({"instrumentId":"lynx:gen:Synth"}),
        json!({"instrumentId":"","expectedRevision":7}),
        json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"channelIndex":-1}),
        json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"channelIndex":1.5}),
        json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"path":"/tmp/plugin.vst3"}),
        json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"name":"x".repeat(257)}),
    ] {
        assert!(problem(&Value::Object(create.input_schema()), &input).is_some(), "{input}");
    }
    assert!(problem(
        &Value::Object(create.input_schema()),
        &json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"channelIndex":0,"mixerIndex":2,"name":"Lead"})
    )
    .is_none());
    assert!(problem(&Value::Object(replace.input_schema()), &json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7})).is_some());
    assert!(problem(
        &Value::Object(replace.input_schema()),
        &json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"channelIndex":1,"mixerIndex":2})
    )
    .is_some());
    assert!(problem(
        &Value::Object(replace.input_schema()),
        &json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7,"channelIndex":1})
    )
    .is_none());
}

#[test]
fn presentation_leaves_other_tools_alone_and_uses_the_shared_plan() {
    for other in ["remember", "find_tools", "jobs", "reaper_read"] {
        assert_eq!((PRESENTATION.doing)(other), None);
        assert_eq!((PRESENTATION.activity)(other), None);
    }
    assert!((PRESENTATION.plan)("make_changes"));
    assert!(!DAW.changes.changes_nothing());
    assert!(!DAW.changes.plan_steps);
    assert!(DAW.changes.kinds.iter().all(|kind| !kind.kind.starts_with("history.")));
}

/// The shared MCP boundary, with request recording and no listener/accept loop.
struct Host {
    requests: RefCell<Vec<(String, JsonObject)>>,
    revision: Cell<u64>,
}
impl Host {
    fn snapshot(&self) -> Value {
        json!({"epoch":format!("process:project:{}",self.revision.get()),"layout":self.revision.get(),
            "project":{"id":"1234567890abcdef1234567890abcdef","identity":"project-a","name":"Current song","path":"/tmp/kumi-fixture.lynx","revision":self.revision.get()},
            "transport":{"playing":false,"tempo":120,"positionBeats":0,"beatsPerBar":4},
            "tracks":[{"ref":format!("l:track:0:{}",self.revision.get()),"name":"Bass","type":"instrument","mixer":{"volume":0.8,"pan":0}}],"selection":{}})
    }
}
#[async_trait(?Send)]
impl McpEndpoint for Host {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":[]})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.requests.borrow_mut().push((name.into(), args));
        let result = match name {
            "status" => json!({"connected":true,"listening":{"render":false}}),
            "snapshot" => self.snapshot(),
            "read" => {
                json!({"epoch":format!("process:project:{}",self.revision.get()),"rows":[{"instrumentId":"lynx:gen:Synth","name":"Synth","available":true}]})
            }
            "change_preview" => {
                json!({"epoch":"process:project:7","layout":7,"transactionId":"insert-1","confirmation":"preview-confirmation","target":{},"prior":{},"display":{"title":"Inserted Synth"}})
            }
            "change_apply" => {
                self.revision.set(8);
                json!({"epoch":"process:project:8","layout":8,"state":"applied","transactionId":"insert-1","applied":{"channelIndex":1},"display":{"title":"Inserted Synth"}})
            }
            "change_undo" => {
                self.revision.set(9);
                json!({"epoch":"process:project:9","layout":9,"state":"undone","transactionId":"insert-1"})
            }
            "transport_stop" => json!({"stopped":true}),
            "baseline_save" | "change_release" => json!({}),
            _ => return Err(RuntimeError::plain(format!("Unexpected host method: {name}"))),
        };
        Ok(serde_json::from_value(
            json!({"content":[{"type":"text","text":result.to_string()}],"structuredContent":result,"isError":false}),
        )
        .unwrap())
    }
}

#[tokio::test]
async fn instrument_insertion_uses_shared_preview_apply_history_and_targeted_undo() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let host = Rc::new(Host { requests: RefCell::default(), revision: Cell::new(7) });
            let connected: Rc<dyn McpEndpoint> = host.clone();
            let changes = Rc::new(RefCell::new(Vec::new()));
            let recorded = changes.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |change| recorded.borrow_mut().push(change)));
            let home = tempfile::tempdir().unwrap();
            let integration = create_lynx_integration(OwnOptions {
                context,
                kumi_home: home.path().into(),
                connect: Some(Rc::new(move |_| {
                    let host = connected.clone();
                    async move { Ok(host) }.boxed_local()
                })),
            });
            let observation = integration.observe(Signal::new(), None).await.unwrap();
            assert_eq!(observation.native_epoch.as_deref(), Some("process:project:7"));
            for name in ["lynx_read", "lynx_channel_create", "make_changes", "undo_change"] {
                assert!(observation.tools.iter().any(|tool| tool.name() == name), "{name}");
            }
            let read = observation.tools.iter().find(|tool| tool.name() == "lynx_read").unwrap();
            assert!(
                !read
                    .execute(
                        json!({"kind":"call","fn":"instrument.catalog","args":{"query":"Synth"}}).as_object().unwrap().clone(),
                        Signal::new()
                    )
                    .await
                    .unwrap()
                    .is_error
            );
            let create = observation.tools.iter().find(|tool| tool.name() == "lynx_channel_create").unwrap();
            let created = create
                .execute(json!({"instrumentId":"lynx:gen:Synth","expectedRevision":7}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert!(!created.is_error, "{}", created.text);
            assert_eq!(changes.borrow().len(), 1);
            assert_eq!(changes.borrow()[0].family, ChangeFamily::Device);
            let undone = integration.undo(None, Signal::new()).await.unwrap();
            assert_eq!(undone.state, ChangeState::Undone);
            assert!(integration.stop_live(Signal::new()).await.unwrap());
            integration.close().await.unwrap();
            let requests = host.requests.borrow();
            let preview = &requests.iter().find(|(name, _)| name == "change_preview").unwrap().1;
            assert_eq!(preview["kind"], "channel.create");
            assert_eq!(preview["input"]["expectedRevision"], 7);
            assert_eq!(preview["input"]["instrumentId"], "lynx:gen:Synth");
            let undo = &requests.iter().find(|(name, _)| name == "change_undo").unwrap().1;
            assert_eq!(undo["transactionId"], "insert-1");
            assert!(!requests.iter().any(|(name, _)| name == "history.undo"));
        })
        .await;
}
