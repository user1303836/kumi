//! What Kumi tells a producer working in Live: a line in Live's status bar, only through a bridge that shows it as
//! the Control Surface's own line (an older one showed a modal alert that held Live), and whether Live records.
use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::contracts::Integration;
use kumi_agent::mcp::{
    client::{McpEndpoint, StderrStatus},
    types::{CallToolResult, Implementation, ListToolsResult},
};
use kumi_common::abort::{self, Signal};
use kumi_common::js::json::stringify;
use kumi_daw_ableton::{integration::Ableton, options::AbletonOptions};
use kumi_types::contracts::JsonObject;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};

/// A bridge of a given version, answering the status-bar line and Live's playback with what it's given.
struct Bridge {
    version: &'static str,
    transport: Value,
    calls: RefCell<Vec<(String, Value)>>,
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
        let tools = ["live_status", "live_message", "live_discover"].map(|name| json!({"name":name,"inputSchema":{"type":"object"}}));
        Ok(serde_json::from_value(json!({"tools":tools})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), Value::Object(args.clone())));
        let result = match name {
            "live_status" => json!({"connected":true,"adapter":"remote-script","epoch":7}),
            "live_message" => json!({"shown":true,"modal":args["modal"]}),
            "live_discover" => json!({"items":[{"transport":self.transport,"firedTargets":[],"playingTargets":[]}]}),
            _ => panic!("unexpected bridge call {name}"),
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&result)}]})).unwrap())
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

async fn connected(bridge: Rc<Bridge>) -> Rc<Ableton> {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let bridge = bridge.clone();
        async move { Ok(bridge as Rc<dyn McpEndpoint>) }.boxed_local()
    }));
    let integration = Ableton::new(options);
    integration.start(signal()).await.unwrap();
    integration.connection.tools().unwrap().refresh(signal()).await.unwrap();
    integration
}

fn signal() -> Signal {
    abort::timeout(10_000)
}

fn local<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, future)
}

#[test]
fn a_line_in_lives_status_bar_only_through_a_bridge_that_never_shows_it_as_a_dialog() {
    local(async {
        // An older bridge: its message in passing was a modal alert, so Kumi sends none.
        let older = Rc::new(Bridge { version: "1.0.91", transport: json!({}), calls: RefCell::default() });
        let integration = connected(older.clone()).await;
        assert!(!integration.show_message("Kumi · done", signal()).await.unwrap());
        assert!(!older.calls.borrow().iter().any(|(name, _)| name == "live_message"));
        // A bridge with the Control Surface's own line: never modal, cut to Live's 1,024 characters by Kumi.
        let fixed = Rc::new(Bridge { version: "1.0.92", transport: json!({}), calls: RefCell::default() });
        let integration = connected(fixed.clone()).await;
        assert!(integration.show_message("Kumi · done · the bass is darker", signal()).await.unwrap());
        assert!(integration.show_message(&"x".repeat(2000), signal()).await.unwrap());
        let sent: Vec<Value> =
            fixed.calls.borrow().iter().filter(|(name, _)| name == "live_message").map(|(_, args)| args.clone()).collect();
        assert_eq!(sent[0], json!({"text":"Kumi · done · the bass is darker","modal":false}));
        assert_eq!(sent[1]["text"].as_str().unwrap().len(), 1024);
        assert_eq!(sent[1]["modal"], false);
        assert!(!integration.show_message("   ", signal()).await.unwrap(), "nothing to say, nothing sent");
    })
}

#[test]
fn live_records_only_while_it_plays_with_session_or_arrangement_recording_on() {
    local(async {
        for (transport, recording) in [
            (json!({"playing":true,"arrangementRecord":true,"sessionRecord":false}), true),
            (json!({"playing":true,"arrangementRecord":false,"sessionRecord":true}), true),
            (json!({"playing":true,"arrangementRecord":false,"sessionRecord":false}), false),
            (json!({"playing":false,"arrangementRecord":true,"sessionRecord":false}), false),
        ] {
            let bridge = Rc::new(Bridge { version: "1.0.92", transport: transport.clone(), calls: RefCell::default() });
            let integration = connected(bridge).await;
            assert_eq!(integration.recording(signal()).await.unwrap(), Some(recording), "{transport}");
        }
    })
}
