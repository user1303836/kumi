//! A plan goes on past a step Live refuses, holding back only the steps that need it (#259), and its references follow
//! the tracks a delete or an add moved, so deleting many tracks in one plan works (#261).
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{
        contracts::{JsonObject, ToolResult},
        errors::RuntimeError,
    },
    integrations::ableton::{integration::Ableton, options::AbletonOptions},
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// A bridge that makes every change it's asked to, except those whose preview's arguments hold a text in `refuse`, and
/// answers deletes and adds as Live would.
struct Bridge {
    refuse: Vec<&'static str>,
    /// What Live's Python answers, in turn.
    python: RefCell<Vec<Value>>,
    calls: RefCell<Vec<(String, JsonObject)>>,
    /// Each preview's arguments, by its transaction.
    previews: RefCell<Vec<JsonObject>>,
    next: Cell<usize>,
}
fn reply(body: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap()
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.88"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let names = [
            "live_status",
            "live_undo_step_begin",
            "live_undo_step_end",
            "live_tempo_preview",
            "live_tempo_apply",
            "live_mixer_preview",
            "live_mixer_apply",
            "live_object_rename_preview",
            "live_object_rename_apply",
            "live_track_delete_preview",
            "live_track_delete_apply",
            "live_session_structure_preview",
            "live_session_structure_apply",
            "live_browser_load_preview",
            "live_browser_load_apply",
            "live_run_python",
        ];
        Ok(serde_json::from_value(
            json!({"tools":names.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), args.clone()));
        if name == "live_status" {
            return Ok(reply(json!({"connected":true,"adapter":"remote-script","epoch":7})));
        }
        if name == "live_undo_step_begin" {
            return Ok(reply(json!({"stepId":"step"})));
        }
        if name == "live_undo_step_end" {
            return Ok(reply(json!({})));
        }
        if name == "live_run_python" {
            return Ok(reply(self.python.borrow_mut().remove(0)));
        }
        if name.ends_with("_preview") {
            let text = stringify(&Value::Object(args.clone()));
            if self.refuse.iter().any(|refused| text.contains(refused)) {
                return Ok(serde_json::from_value(json!({"content":[{"type":"text","text":"Live refused this"}],"isError":true})).unwrap());
            }
            let transaction = self.next.get();
            self.next.set(transaction + 1);
            self.previews.borrow_mut().push(args.clone());
            let mut preview = json!({"epoch":7,"transactionId":format!("tx{transaction}"),"confirmation":"yes"});
            if name == "live_track_delete_preview" {
                preview["track"] = json!({"ref":args["trackRef"],"name":"T","kind":"regular"});
            }
            return Ok(reply(preview));
        }
        let transaction: usize = args["transactionId"].as_str().unwrap()[2..].parse().unwrap();
        let previewed = self.previews.borrow()[transaction].clone();
        Ok(reply(match name {
            "live_browser_load_apply" => {
                let loaded = self.calls.borrow().iter().filter(|(name, _)| name == "live_browser_load_apply").count() - 1;
                json!({"state":"applied","deviceRef":format!("7:device:1:{loaded}"),"placement":{"owner":"track","index":loaded}})
            }
            "live_track_delete_apply" => {
                json!({"state":"applied","deleted":previewed["trackRef"],"kept":"Kumi can't bring this back; Live's undo can."})
            }
            "live_session_structure_apply" => {
                let made: Vec<Value> = previewed["tracks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|track| json!({"kind":"track","ref":format!("7:track:{}", track["index"]),"name":track["name"]}))
                    .collect();
                json!({"state":"applied","created":made})
            }
            _ => json!({"state":"applied"}),
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
impl Bridge {
    /// The arguments of each preview of this tool, in order.
    fn previewed(&self, tool: &str) -> Vec<JsonObject> {
        self.calls.borrow().iter().filter(|(name, _)| name == tool).map(|(_, args)| args.clone()).collect()
    }
}
/// Kumi with eight tracks (and Main after them) as this turn's look showed them, by their short names.
async fn kumi(refuse: Vec<&'static str>) -> (Rc<Ableton>, Rc<Bridge>, Vec<String>) {
    let bridge = Rc::new(Bridge {
        refuse,
        python: RefCell::new(vec![]),
        calls: RefCell::new(vec![]),
        previews: RefCell::new(vec![]),
        next: Cell::new(0),
    });
    let endpoint = bridge.clone();
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let integration = Ableton::new(options);
    let connection = integration.connection.clone();
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.available.set(true);
    connection.epoch.set(Some(7.));
    let names = {
        let mut book = connection.references.borrow_mut();
        (0..9)
            .map(|index| {
                let reference = format!("7:track:{index}");
                book.refs.insert(reference.clone(), "track".into());
                book.short_ref(&reference)
            })
            .collect()
    };
    (integration, bridge, names)
}
async fn plan(integration: &Ableton, steps: Value) -> (ToolResult, Value) {
    let result = integration.mutations.make_changes(json!({"steps":steps}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    let reply = serde_json::from_str(&result.text).unwrap_or(Value::Null);
    (result, reply)
}

#[tokio::test(flavor = "current_thread")]
async fn deleting_many_tracks_in_one_plan_deletes_each_wherever_the_deletes_before_it_moved_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Last first, as the model writes it: no remaining track moves, and every delete goes through (#261).
            let (integration, bridge, names) = kumi(vec![]).await;
            let (result, reply) =
                plan(&integration, json!([{"tool":"delete_track","each":{"trackRef":[names[7], names[6], names[5]]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            assert_eq!(reply["done"].as_array().unwrap().len(), 3);
            let deleted: Vec<_> = bridge.previewed("live_track_delete_preview").iter().map(|args| args["trackRef"].clone()).collect();
            assert_eq!(deleted, [json!("7:track:7"), json!("7:track:6"), json!("7:track:5")]);
            // First first: each later track moved up one when the one before it went, and its name went with it.
            let (integration, bridge, names) = kumi(vec![]).await;
            let (result, _) = plan(&integration, json!([{"tool":"delete_track","each":{"trackRef":[names[2], names[3], names[4]]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            let deleted: Vec<_> = bridge.previewed("live_track_delete_preview").iter().map(|args| args["trackRef"].clone()).collect();
            assert_eq!(deleted, [json!("7:track:2"), json!("7:track:2"), json!("7:track:2")]);
            // Main followed too: it's at 5 now, named as before; a deleted track's name names nothing.
            let book = integration.connection.references.borrow();
            assert_eq!(book.lengthen(&json!({"ref":names[8]}))["ref"], "7:track:5");
            assert_eq!(book.lengthen(&json!({"ref":names[3]}))["ref"], json!(names[3]), "a deleted track's name isn't anyone's");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_plan_goes_on_past_a_refused_step_and_holds_back_only_the_steps_that_need_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (integration, bridge, names) = kumi(vec!["\"7:track:1\""]).await;
            let (result, reply) = plan(
                &integration,
                json!([
                    {"tool":"set_mixer","input":{"trackRef":names[1],"volume":0.5}},
                    {"tool":"rename","input":{"ref":names[1],"name":"Lead"}},
                    {"tool":"set_mixer","input":{"trackRef":names[2],"volume":0.6}},
                    {"tool":"add_tracks_and_scenes","as":"pad","input":{"tracks":[{"name":"Pad","index":3}]}},
                    {"tool":"set_mixer","input":{"trackRef":"@pad","volume":0.4}},
                    {"tool":"set_tempo","input":{"tempo":124}}
                ]),
            )
            .await;
            // An error, so the model reads it: one step refused, one held back with it, and every other step made.
            assert!(result.is_error, "{}", result.text);
            assert_eq!(reply["refused"].as_array().unwrap().len(), 1);
            assert_eq!((&reply["refused"][0]["step"], &reply["refused"][0]["tool"]), (&json!(1), &json!("set_mixer")));
            assert!(reply["refused"][0]["error"].as_str().unwrap().contains("Live refused this"));
            assert_eq!(reply["heldBack"], json!([{"step":2,"tool":"rename","after":1}]));
            let done: Vec<_> = reply["done"].as_array().unwrap().iter().map(|row| row["step"].as_u64().unwrap()).collect();
            assert_eq!(done, [3, 4, 5, 6]);
            assert!(bridge.previewed("live_object_rename_preview").is_empty(), "the held-back rename never reached Live");
            // The new track's step used the name the plan gave it.
            let mixed: Vec<_> = bridge.previewed("live_mixer_preview").iter().map(|args| args["trackRef"].clone()).collect();
            assert_eq!(mixed, [json!("7:track:1"), json!("7:track:2"), json!("7:track:3")]);
            // A plan resending what was refused uses the @names this answer made.
            let (result, _) = plan(&integration, json!([{"tool":"rename","input":{"ref":"@pad","name":"Warm Pad"}}])).await;
            assert!(!result.is_error, "{}", result.text);
            assert_eq!(bridge.previewed("live_object_rename_preview")[0]["ref"], "7:track:3");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_track_add_holds_back_what_uses_it_and_the_restructures_after_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Adding the Sub track is refused: its own steps can't run, and nor can the delete after it, which may be
            // the other half of a swap (add the Sub, then delete the 808 it replaces). The tempo has nothing to do
            // with either, and is set.
            let (integration, bridge, names) = kumi(vec!["\"Sub\""]).await;
            let (result, reply) = plan(
                &integration,
                json!([
                    {"tool":"add_tracks_and_scenes","as":"sub","input":{"tracks":[{"name":"Sub","index":3}]}},
                    {"tool":"set_mixer","input":{"trackRef":"@sub","volume":0.4}},
                    {"tool":"delete_track","input":{"trackRef":names[0]}},
                    {"tool":"set_tempo","input":{"tempo":124}}
                ]),
            )
            .await;
            assert!(result.is_error, "{}", result.text);
            assert_eq!(reply["refused"][0]["step"], 1);
            assert_eq!(reply["heldBack"], json!([{"step":2,"tool":"set_mixer","after":1},{"step":3,"tool":"delete_track","after":1}]));
            let done: Vec<_> = reply["done"].as_array().unwrap().iter().map(|row| row["step"].as_u64().unwrap()).collect();
            assert_eq!(done, [4]);
            assert!(bridge.previewed("live_track_delete_preview").is_empty() && bridge.previewed("live_mixer_preview").is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_plan_loads_a_modulator_and_maps_it_in_one_go_and_kumis_undo_empties_the_slot() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (integration, bridge, names) = kumi(vec![]).await;
            // Live's LFO is a Max device: just loaded, it can't map until Max is ready, then it maps (#264).
            *bridge.python.borrow_mut() = vec![
                json!({"ok":false,"result":null,"stdout":"","error":{"type":"RuntimeError","message":"Max bridge is not initialized"}}),
                json!({"ok":true,"stdout":"","error":null,"result":{"modulator":"LFO","slot":0,"prior":null,"now":{"name":"Filter Freq","device":"Operator","identity":4242},"track":null}}),
                json!({"ok":true,"stdout":"","error":null,"result":{"back":1,"moved":[],"gone":[]}}),
            ];
            let (result, reply) = plan(
                &integration,
                json!([
                    {"tool":"load_device","as":"synth","input":{"trackRef":names[1],"itemId":"instruments/Operator"}},
                    {"tool":"load_device","as":"lfo","input":{"trackRef":names[1],"itemId":"modulators/LFO"}},
                    {"tool":"map_modulator","input":{"deviceRef":"@lfo","targetRef":"@synth","parameter":"Filter Freq"}}
                ]),
            )
            .await;
            assert!(!result.is_error, "{}", result.text);
            assert_eq!(reply["done"][2]["changed"], "Mapped LFO to Operator's Filter Freq");
            let python: Vec<String> = bridge.previewed("live_run_python").iter().map(|args| args["code"].as_str().unwrap().to_owned()).collect();
            assert_eq!(python.len(), 2, "tried again once Max was ready");
            assert!(python[1].starts_with("# kumi:map-modulator") && python[1].contains(r#"\"device\":\"7:device:1:1\""#) && python[1].contains(r#"\"target\":\"7:device:1:0\""#));
            // Kumi's undo empties the slot it filled, only while it still holds that parameter.
            let undone = integration.history.undo("last", Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            let code = bridge.previewed("live_run_python").last().unwrap()["code"].as_str().unwrap().to_owned();
            assert!(code.starts_with("# kumi:fast-revert") && code.contains(r#"\"kind\":\"modulation\""#) && code.contains(r#"\"applied\":4242"#), "{code}");
        })
        .await;
}
