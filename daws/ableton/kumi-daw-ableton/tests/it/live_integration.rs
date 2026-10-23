use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::{
    core::contracts::*,
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use kumi_common::abort::{Signal, SignalExt};
use kumi_daw_ableton::{
    integration::Ableton,
    observation::ObservationHost,
    options::{AbletonOptions, HandsSetup},
};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::{cell::Cell, rc::Rc};

struct Fixture {
    case: Value,
    catalog: Value,
    closed: Cell<usize>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(
            serde_json::from_value(
                json!({"name":"fixture","version":self.case["config"].get("version").cloned().unwrap_or(json!("1.0.73"))}),
            )
            .unwrap(),
        )
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        Ok(serde_json::from_value(json!({"tools":self.catalog})).unwrap())
    }
    async fn call(&self, name: &str, _: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        panic!("unexpected bridge call {name}")
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
        self.closed.set(self.closed.get() + 1);
        Ok(())
    }
}

// What every request carries stays small: of the fullest catalog's tools, only Live's core ones go with each request
// (the rest wait behind find_tools), and they stay under a ceiling. The session's own have theirs (session.rs).
#[tokio::test(flavor = "current_thread")]
async fn of_the_fullest_catalog_only_the_core_tools_go_with_every_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let source: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/public-tools-oracle.json")).unwrap();
            let cases = source["cases"].as_array().unwrap();
            let fullest = cases.iter().max_by_key(|case| case["definitions"].as_array().unwrap().len()).unwrap();
            let catalog = Value::Array(
                fullest["catalog"].as_array().unwrap().iter().map(|id| source["pool"][id.as_u64().unwrap() as usize].clone()).collect(),
            );
            let endpoint = Rc::new(Fixture { case: fullest.clone(), catalog, closed: Cell::new(0) });
            let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
            options.connect = Some(Rc::new(move |_| {
                let connect = endpoint.clone();
                async move { Ok(connect as Rc<dyn McpEndpoint>) }.boxed_local()
            }));
            options.hands = Some(HandsSetup::Open(Rc::new(|| async { Ok(None) }.boxed_local())));
            let integration = Ableton::new(options);
            integration.start(Signal::new()).await.unwrap();
            integration.connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            let definitions = integration.definitions();
            let sent: Vec<&str> = definitions.iter().filter(|tool| !tool.deferred()).map(|tool| tool.name()).collect();
            let bytes: usize = definitions
                .iter()
                .filter(|tool| !tool.deferred())
                .map(|tool| tool.name().len() + tool.description().len() + serde_json::to_string(&tool.input_schema()).unwrap().len())
                .sum();
            assert!(definitions.len() > 90, "the fullest catalog: {}", definitions.len());
            assert!(sent.len() <= 22, "{} tools go with every request: {sent:?}", sent.len());
            assert!(bytes <= 40 * 1024, "{bytes} bytes go with every request");
            // The checker in front of every deferred tool takes what the tool's own schema allows.
            for tool in definitions.iter().filter(|tool| tool.deferred()) {
                let schema = Value::Object(tool.input_schema());
                let example = kumi_agent::kernel::schema::example(&schema).unwrap_or_else(|| panic!("no example for {}", tool.name()));
                assert_eq!(kumi_agent::kernel::schema::problem(&schema, &example), None, "{} {example}", tool.name());
            }
            // Every change and action that isn't core is a step make_changes lists with its input.
            let plan = definitions.iter().find(|tool| tool.name() == "make_changes").unwrap();
            for tool in definitions.iter().filter(|tool| tool.deferred() && tool.listed_by() == Some("make_changes")) {
                assert!(
                    plan.description().contains(&format!("; {}: ", tool.name()))
                        || plan.description().contains(&format!(": {}: ", tool.name())),
                    "{}",
                    tool.name()
                );
            }
            integration.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn complete_public_tool_catalog_matches_source_versions_capabilities_and_schemas() {
    tokio::task::LocalSet::new().run_until(async{
        let source:Value=serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/public-tools-oracle.json")).unwrap();
        for (index,case) in source["cases"].as_array().unwrap().iter().enumerate(){
            let expand=|v:&Value|Value::Array(v.as_array().unwrap().iter().map(|id|source["pool"][id.as_u64().unwrap() as usize].clone()).collect());
            let endpoint=Rc::new(Fixture{case:case.clone(),catalog:expand(&case["catalog"]),closed:Cell::new(0)});
            let connect=endpoint.clone();
            let mut options=AbletonOptions::new(Rc::new(|_,_|{}));
            options.connect=Some(Rc::new(move |_|{let connect=connect.clone();async move{Ok(connect as Rc<dyn McpEndpoint>)}.boxed_local()}));
            options.hands=Some(if case["config"]["hands"]==false{HandsSetup::Disabled}else{HandsSetup::Open(Rc::new(||async{Ok(None)}.boxed_local()))});
            let integration=Ableton::new(options);
            integration.start(Signal::new()).await.unwrap();
            integration.connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            integration.connection.epoch.set(Some(7.));
            let definitions:Vec<_>=integration.definitions().iter().map(|tool|json!({"name":tool.name(),"description":tool.description(),"inputSchema":tool.input_schema(),"stream":tool.stream(Signal::new(),Rc::new(||{})).is_some()})).collect();
            assert_eq!(json!(definitions),expand(&case["definitions"]),"catalog {index} config {}",case["config"]);
            assert!(integration.has_undo()&&integration.has_audio_file()&&integration.has_stop_live()&&integration.has_device_tree()&&integration.has_clip_view()&&integration.has_session_strip()&&integration.has_arrangement_strip()&&integration.has_audition()&&integration.has_goal()&&integration.has_hear());
            let (first,second)=futures::join!(integration.close(),integration.close());first.unwrap();second.unwrap();
            assert_eq!(endpoint.closed.get(),1);
        }
    }).await;
}
