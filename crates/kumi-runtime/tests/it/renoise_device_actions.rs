//! Renoise's actions on a device end to end (M4 n-devices): Kumi's tool takes a short ref, and Kumi's host sends the tool's
//! method the long ref and the epoch it was read in; what Renoise did comes back in the answer.
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    renoise::simulator::RenoiseScript,
    simulator::{Respond, Simulator},
};
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::core::{context::IntegrationContext, contracts::*};
use kumi_runtime::integrations::{
    own_bridge::{Connect, OwnOptions},
    renoise::create_renoise_integration,
};
use kumi_runtime::mcp::client::McpEndpoint;
use serde_json::{json, Value};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
};

kumi_testkit::in_process!();
use in_process::InProcess;

fn tool(observed: &Observation, name: &str) -> Rc<dyn KernelTool> {
    observed.tools.iter().find(|tool| tool.name() == name).cloned().unwrap_or_else(|| panic!("no {name} tool"))
}

fn input(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

#[tokio::test(flavor = "current_thread")]
async fn a_device_action_names_its_device_by_a_short_ref_and_says_what_renoise_did() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = RenoiseScript::new();
            let asked: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
            let into = asked.clone();
            // (the simulator has one responder: the song's answers everything but the actions)
            let song = script.respond();
            let respond: Respond = Arc::new(move |method, params| {
                into.lock().unwrap().push((method.to_string(), params.clone()));
                let epoch = params.get("epoch").cloned().unwrap_or(Value::Null);
                match method {
                    "device_editor" => Some(Ok(json!({"epoch": epoch, "layout": 0, "visible": params["show"] != json!(false), "label": "EQ 10 on Track 1"}))),
                    "preset_save" => Some(Ok(json!({
                        "epoch": epoch, "layout": 0, "file": format!("/Users/p/Documents/Renoise/User Library/Effect Presets/EQ 10/{}.xrdp", params["name"].as_str().unwrap()),
                        "listed": true, "label": "EQ 10 on Track 1",
                    }))),
                    _ => song(method, params),
                }
            });
            let mut config = script.config(secret, "/lab/Renoise").with_methods(&["device_editor", "preset_save"], respond);
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let context = IntegrationContext::new(Rc::new(|_, _| {}));
            let renoise = create_renoise_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            let read = tool(&observed, "renoise_read");
            let devices = read.execute(input(json!({"kind": "device", "parent": "track:1"})), Signal::new()).await.unwrap();
            let devices: Value = serde_json::from_str(&devices.text).unwrap();
            let device = devices["items"][0]["ref"].as_str().unwrap().to_owned();
            assert!(!device.contains(":device:"), "a short ref: {device}");

            let shown = tool(&observed, "plugin_editor").execute(input(json!({"ref": device})), Signal::new()).await.unwrap();
            assert!(!shown.is_error, "{}", shown.text);
            assert!(shown.text.contains("Showed or hid the device's editor window") && shown.text.contains("\"visible\":true"), "{}", shown.text);
            let (method, params) = asked.lock().unwrap().last().cloned().unwrap();
            assert_eq!(method, "device_editor");
            assert!(params["ref"].as_str().unwrap().contains(":device:"), "the long ref went to the tool: {params}");
            assert!(params["epoch"].is_string(), "with the epoch it was read in: {params}");

            let saved = tool(&observed, "save_device_preset").execute(input(json!({"ref": device, "name": "Bright"})), Signal::new()).await.unwrap();
            assert!(!saved.is_error, "{}", saved.text);
            assert!(saved.text.contains("Effect Presets/EQ 10/Bright.xrdp") && saved.text.contains("\"listed\":true"), "{}", saved.text);

            // A ref that isn't one of this turn's: refused in Kumi's words, nothing sent.
            let before = asked.lock().unwrap().len();
            let refused = tool(&observed, "plugin_editor").execute(input(json!({"ref": "device:99"})), Signal::new()).await.unwrap();
            assert!(refused.is_error && refused.text.contains("isn't one of this turn's refs"), "{}", refused.text);
            assert_eq!(asked.lock().unwrap().len(), before);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}
