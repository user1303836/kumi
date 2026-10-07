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
    integrations::ableton::{integration::Ableton, observation::ObservationHost, options::AbletonOptions},
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
    /// Refusals a preview gives once, by a text its arguments hold: the first such preview is refused with the
    /// message, as when Live is still setting up what the change fences.
    once: RefCell<Vec<(&'static str, &'static str)>>,
    calls: RefCell<Vec<(String, JsonObject)>>,
    /// What Live's undo answers, last first; "undone" once there's nothing left.
    undo: RefCell<Vec<&'static str>>,
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
            "live_clip_properties_preview",
            "live_clip_properties_apply",
            "live_audio_clip_preview",
            "live_audio_clip_apply",
            "live_device_io_preview",
            "live_device_io_apply",
            "live_device_parameter_preview",
            "live_device_parameter_apply",
            "live_willington_device_preview",
            "live_willington_device_apply",
            "live_arrangement_midi_clip_preview",
            "live_arrangement_midi_clip_apply",
            "live_undo",
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
        if name == "live_undo" {
            return Ok(reply(json!({"state":self.undo.borrow_mut().pop().unwrap_or("undone")})));
        }
        if name.ends_with("_preview") {
            let text = stringify(&Value::Object(args.clone()));
            let refused_once = {
                let mut once = self.once.borrow_mut();
                once.iter().position(|(held, _)| text.contains(held)).map(|at| once.remove(at).1)
            };
            if let Some(message) = refused_once {
                return Ok(serde_json::from_value(json!({"content":[{"type":"text","text":message}],"isError":true})).unwrap());
            }
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
            // Live doesn't confirm a clip write here: it may or may not have made it.
            "live_arrangement_midi_clip_apply" => json!({"state":"pending"}),
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
        once: RefCell::new(vec![]),
        calls: RefCell::new(vec![]),
        undo: RefCell::new(vec![]),
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
async fn a_held_back_step_holds_back_the_launches_and_waits_and_track_steps_a_refused_one_would() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // The Drop scene isn't made, so neither is the copy of the bass clip into it, nor its launch. The minute's
            // wait after the launch would wait in silence, so it's held back too, and so is the bass's volume, on the
            // track the held-back copy would have changed. The tempo, on neither, is set.
            let (integration, bridge, names) = kumi(vec!["\"Drop\""]).await;
            integration.connection.references.borrow_mut().refs.insert("7:clip:1:0".into(), "session-clip".into());
            let (result, reply) = plan(
                &integration,
                json!([
                    {"tool":"add_tracks_and_scenes","as":"drop","input":{"scenes":[{"name":"Drop","index":2}]}},
                    {"tool":"duplicate_clip","input":{"clipRef":"7:clip:1:0","targetSceneRef":"@drop"}},
                    {"tool":"fire_scene","input":{"sceneRef":"@drop"}},
                    {"tool":"wait","input":{"seconds":60}},
                    {"tool":"set_mixer","input":{"trackRef":names[1],"volume":0.8}},
                    {"tool":"set_tempo","input":{"tempo":124}}
                ]),
            )
            .await;
            assert!(result.is_error, "{}", result.text);
            assert_eq!(reply["refused"][0]["step"], 1);
            assert_eq!(
                reply["heldBack"],
                json!([
                    {"step":2,"tool":"duplicate_clip","after":1},
                    {"step":3,"tool":"fire_scene","after":1},
                    {"step":4,"tool":"wait","after":1},
                    {"step":5,"tool":"set_mixer","after":1}
                ])
            );
            let done: Vec<_> = reply["done"].as_array().unwrap().iter().map(|row| row["step"].as_u64().unwrap()).collect();
            assert_eq!(done, [6]);
            assert!(bridge.previewed("live_mixer_preview").is_empty());
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

#[tokio::test(flavor = "current_thread")]
async fn a_refusal_from_a_fence_that_moved_is_asked_again_and_an_audio_clips_loop_goes_to_set_audio_clip() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (integration, bridge, names) = kumi(vec![]).await;
            // Live still setting up the track a step loads onto: its fence moved, and asked again a moment later the
            // load goes through, with no refusal for the model to deal with (#259).
            bridge
                .once
                .borrow_mut()
                .push(("instruments/Drift", "device insertion did not confirm the exact requested name, index, and siblings"));
            // An audio clip's loop points are set_audio_clip's: set_clip is refused for them, and Kumi asks there instead.
            bridge.once.borrow_mut().push(("7:clip:2:0", "audio clip loop editing uses live_audio_clip_preview"));
            integration.connection.references.borrow_mut().refs.insert("7:clip:2:0".into(), "session-clip".into());
            let (result, reply) = plan(
                &integration,
                json!([
                    {"tool":"load_device","input":{"trackRef":names[1],"itemId":"instruments/Drift"}},
                    {"tool":"set_clip","input":{"clipRef":"7:clip:2:0","looping":true,"loopStart":0,"loopEnd":8}}
                ]),
            )
            .await;
            assert!(!result.is_error, "{}", result.text);
            assert_eq!(reply["done"].as_array().unwrap().len(), 2);
            assert_eq!(bridge.previewed("live_browser_load_preview").len(), 2, "asked again once");
            assert_eq!(
                bridge.previewed("live_audio_clip_preview"),
                [json!({"clipRef":"7:clip:2:0","loopStart":0,"loopEnd":8}).as_object().unwrap().clone()]
            );
            // set_audio_clip can't switch the clip's looping on, which the step asked for: the answer says so.
            assert!(reply["done"][1]["looping"].as_str().unwrap().contains("turn Loop on"), "{reply}");
            // Refused for good, a refusal names Kumi's tool, not the bridge's.
            bridge.once.borrow_mut().push(("7:clip:2:0", "audio clip loop editing uses live_audio_clip_preview"));
            let (_, reply) = plan(&integration, json!([{"tool":"set_clip","input":{"clipRef":"7:clip:2:0","looping":false}}])).await;
            let error = reply["refused"][0]["error"].as_str().unwrap();
            assert!(error.contains("uses set_audio_clip") && !error.contains("live_audio_clip_preview"), "{error}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_sidechains_channel_is_left_to_live_rather_than_refused() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // The bridge's sidechain takes the source track only: a channel given with it used to refuse the change
            // (#259); now the track is set and the answer says the channel is Live's.
            let (integration, bridge, _) = kumi(vec![]).await;
            integration.connection.references.borrow_mut().refs.insert("7:device:1:0".into(), "device".into());
            let (result, reply) = plan(
                &integration,
                json!([{"tool":"set_sidechain","input":{"action":"sidechain","deviceRef":"7:device:1:0","routingType":"1-Kick","routingChannel":"Post FX"}}]),
            )
            .await;
            assert!(!result.is_error, "{}", result.text);
            let previewed = &bridge.previewed("live_device_io_preview")[0];
            assert!(!previewed.contains_key("routingChannel") && previewed["routingType"] == "1-Kick");
            assert!(reply["done"][0]["channel"].as_str().unwrap().contains("Kumi doesn't set its channel"), "{reply}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kumis_python_has_live_read_a_moved_refs_place_again_before_it_uses_the_ref() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Live's registry keeps objects by ref string. Once track 3 is deleted, old track 5's EQ is at
            // "7:device:4:0", but Live still holds old track 4's under that string until track 4 is read again. The
            // bridge reads a place again before it changes anything there; Kumi's Python gets objects by string, so it
            // has Live read the place again first, or it would set what used to be there.
            let (integration, bridge, names) = kumi(vec![]).await;
            for (reference, kind) in [("7:device:5:0", "device"), ("7:parameter:7:device:5:0:2", "parameter")] {
                integration.connection.references.borrow_mut().refs.insert(reference.into(), kind.into());
            }
            let set = |value: f64| {
                json!({"ok":true,"stdout":"","error":null,"result":{"device":"EQ Eight","track":null,"items":[{"name":"1 Gain A","prior":0.5,"priorDisplay":"0.0 dB","min":0,"max":1,"applied":value,"value":value,"display":"1.0 dB"}]}})
            };
            let ran = |result: Value| json!({"ok":true,"stdout":"","error":null,"result":result});
            *bridge.python.borrow_mut() =
                vec![set(0.6), set(0.7), set(0.8), ran(json!({"back":1,"moved":[],"gone":[]})), ran(Value::Null), ran(json!("Main"))];
            let gain = |track: usize, value: f64| {
                json!([{"tool":"set_device_parameter","input":{"deviceRef":format!("7:device:{track}:0"),"parameterRef":format!("7:parameter:7:device:{track}:0:2"),"value":value}}])
            };
            let (result, reply) = plan(&integration, gain(5, 0.6)).await;
            assert!(!result.is_error, "{}", result.text);
            let first = reply["done"][0]["change"].as_str().unwrap().to_owned();
            let (result, _) = plan(&integration, json!([{"tool":"delete_track","input":{"trackRef":names[3]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            // The EQ followed its track to 4: the first set there reads track 4 again first thing, in the same run;
            // the next doesn't need to.
            for value in [0.7, 0.8] {
                let (result, _) = plan(&integration, gain(4, value)).await;
                assert!(!result.is_error, "{}", result.text);
            }
            let python = || -> Vec<String> {
                bridge.previewed("live_run_python").iter().map(|args| args["code"].as_str().unwrap().to_owned()).collect()
            };
            assert!(!python()[0].contains("PLACES"), "nothing had moved yet");
            assert!(python()[1].starts_with("# kumi:fast-set\nPLACES = [\"7:track:4\"]\n"), "{}", python()[1]);
            assert!(python()[1].contains("bridge._refresh(*PLACES)\nimport json\nARGS = json.loads("));
            assert!(!python()[2].contains("PLACES"), "track 4 was read again");
            // Another delete moves it to 3. Kumi's undo of the first set names the EQ where both deletes moved it, and
            // has that place read again first.
            let (result, _) = plan(&integration, json!([{"tool":"delete_track","input":{"trackRef":names[0]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            let undone = integration.history.undo(&first, Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            let revert = python().pop().unwrap();
            assert!(revert.starts_with("# kumi:fast-revert
PLACES = [\"7:track:3\"]
"), "{revert}");
            assert!(revert.contains(r#"\"ref\":\"7:parameter:7:device:3:0:2\""#), "{revert}");
            // run_python's own code (or a ref Live gets before any code runs) has a run of its own first, which
            // changes nothing. Main was at 8; two deletes moved it to 6.
            let python_tool = integration.definitions().into_iter().find(|tool| tool.name() == "run_python").unwrap();
            let input = json!({"code":"result = obj.name","ref":"7:track:6"}).as_object().unwrap().clone();
            let run = python_tool.execute(input, Signal::new()).await.unwrap();
            assert!(!run.is_error, "{}", run.text);
            let calls = bridge.previewed("live_run_python");
            assert!(calls[4]["code"].as_str().unwrap().starts_with("# kumi:moved-places
PLACES = [\"7:track:6\"]
"), "{}", calls[4]["code"]);
            assert_eq!((&calls[5]["code"], &calls[5]["ref"]), (&json!("result = obj.name"), &json!("7:track:6")));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kumis_undo_of_a_track_add_moves_the_refs_that_followed_it_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // A track made at 3 moves what was at 3 and after it up one, and Kumi's refs go with them. Kumi's undo
            // deletes it again, and they go back with it: the names, the refs, and those HISTORY's undo names.
            let (integration, bridge, names) = kumi(vec![]).await;
            for (reference, kind) in [("7:device:5:0", "device"), ("7:parameter:7:device:5:0:2", "parameter")] {
                integration.connection.references.borrow_mut().refs.insert(reference.into(), kind.into());
            }
            *bridge.python.borrow_mut() = vec![
                json!({"ok":true,"stdout":"","error":null,"result":{"device":"EQ Eight","track":null,"items":[{"name":"1 Gain A","prior":0.5,"priorDisplay":"0.0 dB","min":0,"max":1,"applied":0.6,"value":0.6,"display":"1.0 dB"}]}}),
                json!({"ok":true,"stdout":"","error":null,"result":{"back":1,"moved":[],"gone":[]}}),
            ];
            let set = json!([{"tool":"set_device_parameter","input":{"deviceRef":"7:device:5:0","parameterRef":"7:parameter:7:device:5:0:2","value":0.6}}]);
            let (_, reply) = plan(&integration, set).await;
            let first = reply["done"][0]["change"].as_str().unwrap().to_owned();
            let (result, reply) = plan(&integration, json!([{"tool":"add_tracks_and_scenes","input":{"tracks":[{"name":"Pad","index":3}]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            let add = reply["done"][0]["change"].as_str().unwrap().to_owned();
            let lengthen = |name: &String| integration.connection.references.borrow().lengthen(&json!({"ref":name}))["ref"].clone();
            assert_eq!((lengthen(&names[3]), lengthen(&names[5])), (json!("7:track:4"), json!("7:track:6")));
            // Another track made first moves everything again, the Pad to 4 among them.
            let (result, _) = plan(&integration, json!([{"tool":"add_tracks_and_scenes","input":{"tracks":[{"name":"Lead","index":0}]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            let undone = integration.history.undo(&add, Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            // The Pad's undo deleted it at 4, where it was by then: what was after it is back one place, what was before
            // it isn't.
            let now: Vec<Value> = [2, 3, 5].iter().map(|at| lengthen(&names[*at])).collect();
            assert_eq!(now, [json!("7:track:3"), json!("7:track:4"), json!("7:track:6")]);
            assert!(integration.connection.references.borrow().refs.contains_key("7:device:6:0"));
            // The first set's undo names the EQ where it is now, and has that place read again first.
            let undone = integration.history.undo(&first, Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            let revert = bridge.previewed("live_run_python").pop().unwrap()["code"].as_str().unwrap().to_owned();
            assert!(revert.starts_with("# kumi:fast-revert
PLACES = [\"7:track:6\"]
"), "{revert}");
            assert!(revert.contains(r#"\"ref\":\"7:parameter:7:device:6:0:2\""#), "{revert}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_change_live_may_have_made_stops_the_plan_though_some_of_it_was_only_missed() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Two clips, one with a mistake in its notation: that one is missed, and the other is sent. Live doesn't
            // confirm the write, so it may have made it: the plan stops there instead of going on as though only the
            // notation had missed.
            let (integration, bridge, names) = kumi(vec![]).await;
            let (result, reply) = plan(
                &integration,
                json!([
                    {"tool":"write_arrangement_clip","input":{"clips":[
                        {"trackRef":names[1],"start":0,"length":4,"notation":"1|1 C3 D3"},
                        {"trackRef":names[1],"start":4,"length":4,"notation":"2|1 C3 Q3"}
                    ]}},
                    {"tool":"set_tempo","input":{"tempo":124}}
                ]),
            )
            .await;
            assert!(result.is_error, "{}", result.text);
            assert_eq!(bridge.previewed("live_arrangement_midi_clip_apply").len(), 1, "{reply}");
            assert!(bridge.previewed("live_tempo_preview").is_empty(), "the plan stopped: {reply}");
            assert!(reply["done"].as_array().is_none_or(|done| done.is_empty()), "{reply}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_undo_of_a_track_add_live_doesnt_confirm_retires_the_refs_that_followed_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Live may or may not have deleted the track again, so the refs that moved with the add can't be trusted
            // either way: they're retired, and the model is told to discover again.
            let (integration, bridge, names) = kumi(vec![]).await;
            let (result, reply) =
                plan(&integration, json!([{"tool":"add_tracks_and_scenes","input":{"tracks":[{"name":"Pad","index":3}]}}])).await;
            assert!(!result.is_error, "{}", result.text);
            let add = reply["done"][0]["change"].as_str().unwrap().to_owned();
            bridge.undo.borrow_mut().push("pending");
            let undone = integration.history.undo(&add, Signal::new(), false).await.unwrap();
            assert!(undone.is_error && undone.text.contains("discover again"), "{}", undone.text);
            let book = integration.connection.references.borrow();
            assert!(book.refs.is_empty());
            assert_eq!(book.lengthen(&json!({"ref":names[5]}))["ref"], json!(names[5]), "a retired name names nothing");
        })
        .await;
}
