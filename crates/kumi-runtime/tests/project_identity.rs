//! A saved Set's project id, kept inside the Set (`kumi.project`), so Save As and moving the folder keep
//! what Kumi knows about the song.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        project::{create_project_store, project_id_of, Baseline, ProjectStore, PROJECT_KEY},
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, path::Path, rc::Rc};

/// A bridge whose Set keeps text under keys, as Live's does.
struct Bridge {
    tools: Vec<&'static str>,
    data: RefCell<Option<String>>,
    proposed: RefCell<Option<String>>,
    calls: RefCell<Vec<String>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(
            json!({"tools":self.tools.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push(name.into());
        assert_eq!(args.get("key").and_then(Value::as_str).unwrap_or(PROJECT_KEY), PROJECT_KEY);
        let body = match name {
            "live_data_read" => json!({"key":PROJECT_KEY,"value":*self.data.borrow()}),
            "live_data_preview" => {
                *self.proposed.borrow_mut() = args.get("value").and_then(Value::as_str).map(str::to_owned);
                json!({"transactionId":"data_1","prior":*self.data.borrow(),"proposed":args.get("value")})
            }
            "live_data_apply" => {
                *self.data.borrow_mut() = self.proposed.borrow_mut().take();
                json!({"transactionId":"data_1","state":"applied"})
            }
            other => panic!("not a tool here: {other}"),
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap())
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

async fn remember(bridge: Rc<Bridge>, store: Rc<dyn ProjectStore>) -> Rc<Remember> {
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = bridge.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    Remember::new(connection, Some(store), None)
}
fn bridge(tools: Vec<&'static str>) -> Rc<Bridge> {
    Rc::new(Bridge { tools, data: RefCell::new(None), proposed: RefCell::new(None), calls: RefCell::new(vec![]) })
}
fn set_in(folder: &Path, name: &str) -> String {
    std::fs::create_dir_all(folder.join("Ableton Project Info")).unwrap();
    let set = folder.join(name);
    std::fs::write(&set, "").unwrap();
    set.to_string_lossy().into_owned()
}
fn baseline(path: &str) -> Baseline {
    Baseline {
        version: 1,
        path: path.into(),
        name: "Night Drive".into(),
        saved_at: 1,
        artifact_id: "a".into(),
        pages: vec![json!({"records":[]}).as_object().unwrap().clone()],
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_saved_set_keeps_its_project_inside_it_and_a_copy_elsewhere_is_a_new_song() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let store = create_project_store(dir.path().join("projects"));
            let set = bridge(vec!["live_data_read", "live_data_preview", "live_data_apply"]);
            let remember = remember(set.clone(), store.clone()).await;
            let first = set_in(&dir.path().join("Night Drive Project"), "Night Drive.als");
            // First seen: the id its path gave before, now kept in the Set.
            let id = remember.project_of(&first, Signal::new()).await;
            assert_eq!((id.clone(), set.data.borrow().clone()), (project_id_of(&first), Some(project_id_of(&first))));
            store.save(&id, &baseline(&first)).await.unwrap();
            // Saved as a version in the same Project folder: the same song, nothing written.
            let version = set_in(&dir.path().join("Night Drive Project"), "Night Drive v2.als");
            set.calls.borrow_mut().clear();
            assert_eq!(remember.project_of(&version, Signal::new()).await, id);
            assert_eq!(*set.calls.borrow(), ["live_data_read"]);
            // Copied into another Project folder while the first is still there: a new song, kept in its Set.
            let copy = set_in(&dir.path().join("Remix Project"), "Night Drive.als");
            assert_eq!(remember.project_of(&copy, Signal::new()).await, project_id_of(&copy));
            assert_eq!(*set.data.borrow(), Some(project_id_of(&copy)));
            // The first Set moved (its last place gone): the same project.
            set.data.replace(Some(id.clone()));
            std::fs::remove_file(&first).unwrap();
            let moved = set_in(&dir.path().join("Moved Project"), "Night Drive.als");
            assert_eq!(remember.project_of(&moved, Signal::new()).await, id);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn without_set_data_a_set_is_the_project_its_path_gives() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let set = bridge(vec![]);
            let remember = remember(set.clone(), create_project_store(dir.path().join("projects"))).await;
            let path = set_in(&dir.path().join("Night Drive Project"), "Night Drive.als");
            assert_eq!(remember.project_of(&path, Signal::new()).await, project_id_of(&path));
            assert!(set.calls.borrow().is_empty());
        })
        .await;
}
