//! What Kumi tells a producer working in Live: a line in Live's status bar, only through a bridge that shows it as
//! the Control Surface's own line (an older one showed a modal alert that held Live), whether Live records or a play
//! would, and Live's own play and stop.
use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::contracts::{Integration, PlayFrom};
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

/// A bridge of a given version, answering the status-bar line, Live's playback and its one track's arm with what it's
/// given, and previewing and applying Live's play and a clip's launch.
struct Bridge {
    version: &'static str,
    transport: Value,
    armed: bool,
    calls: RefCell<Vec<(String, Value)>>,
}

impl Bridge {
    fn new(version: &'static str, transport: Value) -> Bridge {
        Bridge { version, transport, armed: false, calls: RefCell::default() }
    }
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
        let tools = [
            "live_status",
            "live_message",
            "live_discover",
            "live_transport_action_preview",
            "live_transport_action_apply",
            "live_clip_launch_preview",
            "live_clip_launch_apply",
        ]
        .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}));
        Ok(serde_json::from_value(json!({"tools":tools})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), Value::Object(args.clone())));
        let result = match name {
            "live_status" => json!({"connected":true,"adapter":"remote-script","epoch":7}),
            "live_message" => json!({"shown":true,"modal":args["modal"]}),
            "live_discover" if args.get("kind") == Some(&json!("track")) => {
                json!({"items":[{"ref":"track:1","name":"Drums","armed":self.armed}]})
            }
            "live_discover" => json!({"items":[{"transport":self.transport,"firedTargets":[],"playingTargets":[]}]}),
            "live_transport_action_preview" | "live_clip_launch_preview" => {
                json!({"transactionId":"t1","epoch":7,"confirmation":"go","proposed":args})
            }
            "live_transport_action_apply" | "live_clip_launch_apply" => json!({"applied":true,"epoch":8}),
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
        let older = Rc::new(Bridge::new("1.0.91", json!({})));
        let integration = connected(older.clone()).await;
        assert!(!integration.show_message("Kumi · done", signal()).await.unwrap());
        assert!(!older.calls.borrow().iter().any(|(name, _)| name == "live_message"));
        // A bridge with the Control Surface's own line: never modal, cut to Live's 1,024 characters by Kumi.
        let fixed = Rc::new(Bridge::new("1.0.92", json!({})));
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
fn live_records_only_while_it_plays_with_session_or_arrangement_recording_on_and_a_play_would_record_with_either_on() {
    local(async {
        for (transport, recording, armed) in [
            (json!({"playing":true,"arrangementRecord":true,"sessionRecord":false}), Some(true), Some(true)),
            (json!({"playing":true,"arrangementRecord":false,"sessionRecord":true}), Some(true), Some(true)),
            (json!({"playing":true,"arrangementRecord":false,"sessionRecord":false}), Some(false), Some(false)),
            // Stopped with Arrangement Record on: it records nothing now, and a play would.
            (json!({"playing":false,"arrangementRecord":true,"sessionRecord":false}), Some(false), Some(true)),
            // One of the two unknown and the other off: Live can't say whether a play would record.
            (json!({"playing":false,"arrangementRecord":false}), Some(false), None),
        ] {
            let bridge = Rc::new(Bridge::new("1.0.92", transport.clone()));
            let integration = connected(bridge).await;
            assert_eq!(integration.recording(signal()).await.unwrap(), recording, "{transport}");
            assert_eq!(integration.record_armed(signal()).await.unwrap(), armed, "{transport}");
        }
    })
}

#[test]
fn kumi_stops_live_only_while_it_plays_and_launches_a_session_clip_by_its_slot() {
    local(async {
        let asked = |bridge: &Bridge, tool: &str| -> Vec<Value> {
            bridge.calls.borrow().iter().filter(|(name, _)| name == tool).map(|(_, args)| args.clone()).collect()
        };
        // Stopped: a stop would move Live's start marker back to the Set's start, so Kumi sends none.
        let stopped = Rc::new(Bridge::new("1.0.92", json!({"playing":false})));
        let integration = connected(stopped.clone()).await;
        assert!(!integration.stop_playback(signal()).await.unwrap().playing);
        assert!(asked(&stopped, "live_transport_action_preview").is_empty() && asked(&stopped, "live_transport_action_apply").is_empty());
        // Playing: Live's own stop, previewed and applied.
        let playing = Rc::new(Bridge::new("1.0.92", json!({"playing":true})));
        let integration = connected(playing.clone()).await;
        // As a look leaves it (serve's connect looks first): Live's epoch, the fake's 7.
        integration.connection.epoch.set(Some(7.0));
        assert!(!integration.stop_playback(signal()).await.unwrap().playing);
        assert_eq!(asked(&playing, "live_transport_action_preview")[0]["action"], "stop");
        assert_eq!(asked(&playing, "live_transport_action_apply").len(), 1);
        // A Session clip is launched by its slot; a play from a bar, a beat, a sequence slot or an Arrangement clip is
        // refused before Live is asked anything.
        // As serve's read of a track's clip slots leaves them: each slot and its clip discovered this turn, and the clip
        // named to the model by its short name, which is what a play gives back.
        let slot: JsonObject =
            serde_json::from_value(json!({"ref":"1:clip_slot:2:0","clipRef":"1:clip:2:0","parentRef":"1:track:2"})).unwrap();
        integration.connection.register_rows("clip-slot", &[slot], &JsonObject::new(), None).unwrap();
        let named = integration.connection.references.borrow_mut().short_ref("1:clip:2:0");
        assert!(integration.plays_from(&PlayFrom::Clip(named.clone())).is_ok(), "{named}");
        assert!(integration.play(&PlayFrom::Clip(named), signal()).await.unwrap().playing);
        assert_eq!(asked(&playing, "live_clip_launch_preview")[0]["slotRef"], "1:clip_slot:2:0");
        for refused in [
            PlayFrom::Bar(5),
            PlayFrom::Beat(8.0),
            PlayFrom::Slot { slot: Some(1), line: None },
            PlayFrom::Clip("1:arrangement_clip:2:0".into()),
        ] {
            assert!(integration.plays_from(&refused).is_err(), "{refused:?}");
        }
        for allowed in [PlayFrom::Here, PlayFrom::Scene(0), PlayFrom::Clip("1:clip:2:0".into())] {
            assert!(integration.plays_from(&allowed).is_ok(), "{allowed:?}");
        }
        // A scene fires only while no track is armed: with Live's Start Recording on Scene Launch, it would record into an
        // armed track's empty slots.
        let armed = Rc::new(Bridge { armed: true, ..Bridge::new("1.0.92", json!({"playing":true})) });
        let integration = connected(armed.clone()).await;
        integration.connection.epoch.set(Some(7.0));
        let refused = integration.play(&PlayFrom::Scene(0), signal()).await.unwrap_err();
        assert!(refused.message().contains("A track is armed"), "{}", refused.message());
        assert!(asked(&armed, "live_discover").iter().all(|args| args["kind"] != "scene"), "no scene was looked up to fire");
    })
}
