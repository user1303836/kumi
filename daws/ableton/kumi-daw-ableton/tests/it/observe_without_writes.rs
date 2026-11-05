//! A host that keeps nothing in the Set (`HostPolicy::plugin`) looks at it without writing: the project and its
//! tracks are still known, from the Set's path and the ids its tracks already hold.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::{
    core::{context::HostPolicy, contracts::Integration},
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_daw_ableton::{integration::Ableton, options::AbletonOptions, project::project_id_of};
use kumi_types::{contracts::JsonObject, errors::RuntimeError};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

/// Every tool that changes the Set, offered so a look that meant to write could.
const WRITING: [&str; 5] = ["live_data_preview", "live_data_apply", "live_mixer_preview", "live_mixer_apply", "live_run_python"];

/// A saved Set whose project and tracks hold no Kumi ids, recording every call.
struct Live {
    file: String,
    calls: RefCell<Vec<(String, JsonObject)>>,
}
fn reply(body: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap()
}
#[async_trait(?Send)]
impl McpEndpoint for Live {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fake","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let names = ["live_status", "live_discover", "live_song_state", "live_data_read"].into_iter().chain(WRITING);
        Ok(serde_json::from_value(
            json!({"tools":names.map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), args.clone()));
        let page = |kind: &str, items: Vec<Value>| reply(json!({"epoch":7,"kind":kind,"items":items,"revision":"r1","truncated":false}));
        Ok(match (name, args.get("kind").and_then(Value::as_str)) {
            ("live_status", _) => reply(
                json!({"connected":true,"adapter":"remote-script","provenance":"fake-live","epoch":7,"environment":{"liveVersion":"12.0"}}),
            ),
            ("live_song_state", _) => reply(json!({"signatureNumerator":4,"signatureDenominator":4,"sessionRecord":false,"swingAmount":0})),
            ("live_discover", Some("set")) => page(
                "set",
                vec![
                    json!({"ref":"7:set:0","objectIdentity":"song","name":"Set","tempo":120,"playing":false,"position":0,"loop":{"start":0,"length":16},"filePath":self.file}),
                ],
            ),
            ("live_discover", Some("track")) => page(
                "track",
                ["Bass", "Drums"]
                    .iter()
                    .enumerate()
                    .map(|(index, name)| json!({"ref":format!("7:track:{index}"),"objectIdentity":format!("track-{index}"),"name":name,"kind":"regular","mediaKind":"midi"}))
                    .collect(),
            ),
            ("live_discover", Some("return-track")) => page("return-track", vec![]),
            ("live_discover", Some("main-track")) => page("main-track", vec![json!({"ref":"7:main_track:0","objectIdentity":"main"})]),
            ("live_discover", Some(kind)) => page(kind, vec![]),
            ("live_data_read", _) => reply(json!({"key":args["key"],"value":null})),
            ("live_data_preview", _) => reply(json!({"transactionId":"data_1"})),
            ("live_data_apply", _) => reply(json!({"transactionId":"data_1","state":"applied"})),
            (other, _) => serde_json::from_value(json!({"content":[{"type":"text","text":format!("not offered here: {other}")}],"isError":true}))
                .unwrap(),
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

/// One look at the Set through Live's integration under `policy`, once what it started in the background (the
/// tracks' id pass, which reads Main last) has finished: the observation, and every call Live received.
async fn look(policy: HostPolicy) -> (kumi_agent::core::contracts::Observation, String, Vec<(String, JsonObject)>) {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("Song.als").to_string_lossy().into_owned();
    std::fs::write(&file, b"").unwrap();
    let live = Rc::new(Live { file: file.clone(), calls: RefCell::new(vec![]) });
    let endpoint = live.clone();
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.policy = policy;
    options.reconnect_interval_ms = Some(3600000);
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let integration = Ableton::new(options);
    integration.start(Signal::new()).await.unwrap();
    let observation = integration.observe(Signal::new(), None).await.unwrap();
    for _ in 0..500 {
        if live.calls.borrow().iter().any(|(_, args)| args.get("kind") == Some(&json!("main-track"))) {
            break;
        }
        tokio::task::yield_now().await;
    }
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(live.calls.borrow().iter().any(|(_, args)| args.get("kind") == Some(&json!("main-track"))), "the tracks' id pass ran");
    integration.close().await.unwrap();
    let calls = live.calls.borrow().clone();
    (observation, file, calls)
}

#[tokio::test(flavor = "current_thread")]
async fn a_host_that_keeps_nothing_in_the_set_looks_at_it_without_writing_and_still_knows_the_project() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (observation, file, calls) = look(HostPolicy::plugin(std::env::temp_dir())).await;
            let identity = observation.identity.expect("the saved Set has a project");
            assert_eq!(identity.id.as_deref(), Some(project_id_of(&file).as_str()), "the project its path gives");
            assert_eq!(observation.project.map(|p| p.id), Some(project_id_of(&file)));
            assert_eq!(observation.tracks, Some(vec!["Bass".to_owned(), "Drums".to_owned()]));
            let written: Vec<&String> =
                calls.iter().map(|(name, _)| name).filter(|name| WRITING.contains(&name.as_str()) || name.ends_with("_apply")).collect();
            assert!(written.is_empty(), "nothing is written into the Set: {written:?}");
            assert!(calls.iter().any(|(name, _)| name == "live_data_read"), "the project's id is still read");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_standalone_look_at_the_same_set_keeps_its_project_inside_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (observation, file, calls) = look(HostPolicy::standalone()).await;
            assert_eq!(observation.identity.and_then(|i| i.id), Some(project_id_of(&file)));
            let kept = calls.iter().find(|(name, args)| name == "live_data_preview" && args.get("key") == Some(&json!("kumi.project")));
            assert_eq!(kept.expect("the project's id is written into the Set").1["value"], json!(project_id_of(&file)));
            assert!(calls.iter().any(|(name, _)| name == "live_data_apply"));
        })
        .await;
}
