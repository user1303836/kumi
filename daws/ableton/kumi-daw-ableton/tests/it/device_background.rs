//! A foreground make can move to a job without touching Live; either maker's file is loaded through the Browser wait.

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::{
    core::contracts::Integration,
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use kumi_common::abort::Signal;
use kumi_daw_ableton::{
    devices::amxd::{decode_amxd, DeviceType},
    integration::Ableton,
    observation::ObservationHost,
    options::AbletonOptions,
};
use kumi_types::{
    contracts::{JsonObject, Reach},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default)]
struct Browser {
    loading: Cell<bool>,
    item: RefCell<String>,
    inspected: Cell<usize>,
    loads: Cell<usize>,
    calls: RefCell<Vec<String>>,
}

#[async_trait(?Send)]
impl McpEndpoint for Browser {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.89"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let tools = ["live_status", "live_browser_inspect", "live_browser_load_preview", "live_browser_load_apply"]
            .map(|name| json!({"name":name, "inputSchema":{"type":"object"}}));
        Ok(serde_json::from_value(json!({"tools":tools})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        if name != "live_status" {
            assert!(self.loading.get(), "making a device must not call the DAW: {name}");
            self.calls.borrow_mut().push(name.into());
        }
        if matches!(name, "live_browser_inspect" | "live_browser_load_preview") {
            assert_eq!(args["itemId"].as_str(), Some(self.item.borrow().as_str()));
        }
        let (body, error) = match name {
            "live_status" => (json!({"connected":true,"adapter":"real-live","epoch":7}), false),
            "live_browser_inspect" => {
                self.inspected.set(self.inspected.get() + 1);
                if self.inspected.get() <= 2 {
                    (json!({"error":"Item not found"}), true)
                } else {
                    (json!({"itemId":self.item.borrow().clone()}), false)
                }
            }
            "live_browser_load_preview" => {
                assert!(self.inspected.get() > 2, "the preview must wait for the new file to appear in the Browser");
                self.loads.set(self.loads.get() + 1);
                (
                    json!({"transactionId":format!("load{}", self.loads.get()),"epoch":7,"confirmation":"apply","undoable":true,
                    "proposed":{"itemId":self.item.borrow().clone()}}),
                    false,
                )
            }
            "live_browser_load_apply" => (
                json!({"state":"applied","undoable":true,
                "deviceObjectIdentity":format!("device{}", self.loads.get()),"loaded":{"name":"New device"}}),
                false,
            ),
            _ => panic!("unexpected DAW read: {name}"),
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":body.to_string()}],"structuredContent":body,"isError":error}))
            .unwrap())
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
async fn foreground_and_job_devices_are_made_without_live_then_their_load_waits_for_the_browser() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let library = tempfile::tempdir().unwrap();
            // Max's documentation is optional; this test needs neither an installed Max nor a harness process.
            std::env::set_var("KUMI_MAX", library.path().join("no-max"));
            let endpoint = Rc::new(Browser::default());
            let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
            options.user_library = Some(library.path().to_string_lossy().into_owned());
            options.connect = Some(Rc::new({
                let endpoint = endpoint.clone();
                move |_| {
                    let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
                    async move { Ok(endpoint) }.boxed_local()
                }
            }));
            let integration = Ableton::new(options);
            let connection = &integration.connection;
            connection.start(Signal::new()).await.unwrap();
            connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            connection.available.set(true);
            connection.epoch.set(Some(7.));
            connection.references.borrow_mut().refs.insert("7:track:0".into(), "track".into());
            for job in [false, true] {
                let tools = if job { integration.worker_tools() } else { integration.definitions() };
                let make = tools.into_iter().find(|tool| tool.name() == "make_device").unwrap();
                assert_eq!(make.reach(), Reach::AwayFromDaw, "foreground make is eligible for Ctrl-B too");
                for kind in DeviceType::ALL {
                    endpoint.calls.borrow_mut().clear();
                    endpoint.inspected.set(0);
                    let name = format!("{} {kind}", if job { "Job" } else { "Foreground" });
                    let patch = match kind {
                        DeviceType::MidiEffect => "in -> out",
                        DeviceType::AudioEffect => "in.0 -> out.0\nin.1 -> out.1",
                        DeviceType::Instrument => "tone = [cycle~ 440]\ntone -> out.0\ntone -> out.1",
                    };
                    let made = make
                        .execute(
                            json!({"type":kind.as_str(),"name":name,"about":"A new device","controls":[],"patch":patch})
                                .as_object()
                                .unwrap()
                                .clone(),
                            Signal::new(),
                        )
                        .await
                        .unwrap();
                    assert!(!made.is_error, "{}", made.text);
                    assert!(endpoint.calls.borrow().is_empty(), "make wrote the file without calling MCP");
                    let made: Value = serde_json::from_str(&made.text).unwrap();
                    let file = std::path::Path::new(made["file"].as_str().unwrap());
                    assert_eq!(decode_amxd(&std::fs::read(file).unwrap()).unwrap().kind, kind);
                    let item = made["itemId"].as_str().unwrap();
                    assert!(integration.mutations.made.borrow().unlisted(item), "the foreground and worker share the load's pending file");
                    *endpoint.item.borrow_mut() = item.into();
                    endpoint.loading.set(true);
                    let loaded = integration
                        .mutations
                        .make_changes(
                            json!({"steps":[{"tool":"load_device",
                    "input":{"trackRef":"7:track:0","itemId":item}}]})
                            .as_object()
                            .unwrap()
                            .clone(),
                            Signal::new(),
                        )
                        .await
                        .unwrap();
                    endpoint.loading.set(false);
                    assert!(!loaded.is_error, "{}", loaded.text);
                    assert_eq!(
                        *endpoint.calls.borrow(),
                        [
                            "live_browser_inspect",
                            "live_browser_inspect",
                            "live_browser_inspect",
                            "live_browser_load_preview",
                            "live_browser_load_apply"
                        ]
                    );
                    assert!(!integration.mutations.made.borrow().unlisted(item));
                }
            }
        })
        .await;
}
