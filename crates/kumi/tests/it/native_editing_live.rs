//! Explicit operator fixture: run only with the disposable Live bridge prepared.
//! KUMI_EDITING_LIVE_CONFIG names an owner-only local endpoint file. Never run in CI.
use ableton_mcp_server::{
    bridge::remote_adapter::{RemoteScriptEndpoint, RemoteScriptLiveAdapter},
    host::{McpHost, McpHostOptions},
    live::{AsyncLiveAdapter, LiveAdapter, LiveInvocation},
};
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::contracts::JsonObject,
    integrations::ableton::{integration::Ableton, options::AbletonOptions},
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
    Integration, RuntimeError,
};
use serde_json::{json, Value};
use std::{path::Path, rc::Rc};
struct HostEndpoint(Rc<McpHost>);
impl HostEndpoint {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let reply = self
            .0
            .handle_async(
                &json!({"jsonrpc":"2.0","id":NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),"method":method,"params":params}),
                None,
            )
            .await
            .map_err(|e| RuntimeError::plain(e.to_string()))?
            .unwrap();
        if reply.get("error").is_some() {
            return Err(RuntimeError::plain(reply["error"].to_string()));
        }
        Ok(reply["result"].clone())
    }
}
#[async_trait(?Send)]
impl McpEndpoint for HostEndpoint {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"real-live-fixture","version":"1.0.87"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        serde_json::from_value(self.request("tools/list", json!({})).await?).map_err(|e| RuntimeError::plain(e.to_string()))
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        serde_json::from_value(self.request("tools/call", json!({"name":name,"arguments":args})).await?)
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
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the explicitly prepared disposable real-Live fixture"]
async fn native_editing_through_make_changes_host_and_history() {
    tokio::task::LocalSet::new().run_until(async {
        let path=std::env::var("KUMI_EDITING_LIVE_CONFIG").expect("explicit fixture endpoint required");
        assert_eq!(ableton_mcp_server::delivery::secret_permissions(Path::new(&path)),ableton_mcp_server::delivery::SecretPermissions::OwnerOnly);
        let config:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut endpoint=RemoteScriptEndpoint::new("127.0.0.1",config["port"].as_u64().unwrap() as u16,config["secret"].as_str().unwrap());
        endpoint.timeout_ms=Some(15000.);
        let adapter=Rc::new(RemoteScriptLiveAdapter::connect(endpoint).await.unwrap());
        let host=Rc::new(McpHost::new(adapter.clone(),McpHostOptions::default()).unwrap());
        host.handle_async(&json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"native-editing-live-fixture","version":"1"}}}),None).await.unwrap();
        host.handle_async(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}),None).await.unwrap();
        let endpoint=Rc::new(HostEndpoint(host));let out=endpoint.clone();
        let mut options=AbletonOptions::new(Rc::new(|_,_|{}));
        options.connect=Some(Rc::new(move |_| { let endpoint:Rc<dyn McpEndpoint>=out.clone();async move{Ok(endpoint)}.boxed_local() }));
        let integration=Ableton::new(options);let connection=integration.connection.clone();
        connection.start(Signal::new()).await.unwrap();connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
        connection.available.set(true);connection.epoch.set(Some(adapter.status().unwrap().epoch.unwrap() as f64));
        let refs=&config["refs"];
        for (key,kind) in [("track","track"),("clip","clip"),("scene","scene"),("parameter","parameter")] { connection.references.borrow_mut().refs.insert(refs[key].as_str().unwrap().into(),kind.into()); }
        for r in refs["groupTracks"].as_array().unwrap() { connection.references.borrow_mut().refs.insert(r.as_str().unwrap().into(),"track".into()); }
        let cases=vec![
            ("set_global_follow_actions",json!({"kind":"global-follow"}),json!({"enabled":!config["enabled"].as_bool().unwrap()})),
            ("set_scene_follow_actions",json!({"kind":"scene-follow","ref":refs["scene"]}),json!({"chance_a":35,"time":1./3.})),
            ("set_note_expression",json!({"kind":"note-expression","ref":refs["clip"],"noteId":1,"dimension":"pressure"}),json!({"exists":true,"events":[[0.1,47.25,0.2,0.3,0.7,0.8],[1.1,92.125,0.5,0.5,0.5,0.5]]})),
            ("edit_arrangement_automation",json!({"kind":"arrangement-automation","ref":refs["track"],"targetRef":refs["parameter"]}),json!({"action":"insert","event":[60,0.55,0.5,0.5,0.5,0.5]})),
            ("set_scene_follow_actions",json!({"kind":"scene-follow","ref":refs["scene"]}),json!({"linked":!config["linked"].as_bool().unwrap()})),
            ("set_scene_follow_actions",json!({"kind":"scene-follow","ref":refs["scene"]}),json!({"linked":true,"loop_count":3})),
        ];
        let mut checks=vec![];
        for (tool,selector,edit) in cases {
            let before=adapter.invoke_async(&LiveInvocation::new("willington.editing.read",selector.clone()),None).await.unwrap();
            let mut input=selector.clone();input.as_object_mut().unwrap().remove("kind");input["edit"]=edit.clone();
            let made=integration.mutations.make_changes(json!({"steps":[{"tool":tool,"input":input}]}).as_object().unwrap().clone(),Signal::new()).await.unwrap();
            assert!(!made.is_error,"{tool}: {}",made.text);
            let after=adapter.invoke_async(&LiveInvocation::new("willington.editing.read",selector.clone()),None).await.unwrap();
            assert_ne!(before["stateRevision"],after["stateRevision"],"{tool} must write");
            let undone=integration.history.undo("last",Signal::new(),false).await.unwrap();
            assert!(!undone.is_error,"{tool}: {}",undone.text);
            let restored=adapter.invoke_async(&LiveInvocation::new("willington.editing.read",selector.clone()),None).await.unwrap();
            assert_eq!(before["stateRevision"],restored["stateRevision"],"{tool} must restore exactly");
            let state:Value=serde_json::from_str(after["state"].as_str().unwrap()).unwrap();
            checks.push(json!({"tool":tool,"edit":edit,"observed":if tool=="edit_arrangement_automation" {json!({"snapshotCaptured":true})} else {state["value"].clone()},"historyUndoRestoredExact":true}));
        }
        let made=integration.mutations.make_changes(json!({"steps":[{"tool":"group_tracks","input":{"trackRefs":refs["groupTracks"]}}]}).as_object().unwrap().clone(),Signal::new()).await.unwrap();
        assert!(!made.is_error,"group_tracks: {}",made.text);
        let id=integration.history.entries.borrow().values().last().unwrap().borrow().record.id.clone();
        let refused=integration.history.undo(&id,Signal::new(),false).await.unwrap();assert!(refused.is_error,"grouping must not claim a HISTORY inverse");
        checks.push(json!({"tool":"group_tracks","applied":true,"historyUndoRefused":true,"cleanup":"operator fixture native ungroup"}));
        let output=std::env::var("KUMI_EDITING_LIVE_REPORT").expect("explicit evidence output required");
        std::fs::write(output,serde_json::to_vec_pretty(&json!({"scope":"Kumi make_changes -> public MCP host -> authenticated Remote Script -> official native library; HISTORY undo for reversible kinds","checks":checks})).unwrap()).unwrap();
        integration.close().await.unwrap();adapter.close().await.unwrap();
    }).await;
}
