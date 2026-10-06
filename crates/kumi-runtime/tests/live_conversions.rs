//! Live's conversions to MIDI through its API (Live.Conversions), not its menus: the clip converted where it is, the new
//! track waited for and named, Live brought forward when it hangs back, and the menus when a Live has no such API.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::*, errors::RuntimeError},
    integrations::ableton::{
        command_tools::{CommandTools, CommandToolsOptions},
        connection::{ConnectionOptions, LiveConnection},
        history::History,
        options::HandsSetup,
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// A Live with two tracks whose Python answers as told, and whose new track shows `lands_after` track reads after a
/// conversion starts (never, when None). `failing` is a read during the conversion that fails; `stranger` adds a track
/// of the producer's while Kumi waits.
struct Live {
    answer: Value,
    lands_after: Option<usize>,
    failing: Option<usize>,
    stranger: bool,
    python: RefCell<Vec<JsonObject>>,
    converting: Cell<bool>,
    reads: Cell<usize>,
}
fn wrap(value: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&value)}],"structuredContent":value})).unwrap()
}
fn track(index: usize, identity: &str, name: &str) -> Value {
    json!({"ref":format!("7:track:{index}"),"objectIdentity":identity,"name":name})
}
#[async_trait(?Send)]
impl McpEndpoint for Live {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_runtime::mcp::types::Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.80"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let tools: Vec<Value> = ["live_status", "live_discover", "live_run_python"]
            .iter()
            .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}))
            .collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        if name == "live_run_python" {
            self.python.borrow_mut().push(args);
            self.converting.set(self.answer["ok"] == true);
            return Ok(wrap(self.answer.clone()));
        }
        let mut tracks = vec![track(0, "live:vox", "1-Vox"), track(1, "live:bass", "2-Bass")];
        if args["kind"] == "track" && self.converting.get() {
            self.reads.set(self.reads.get() + 1);
            if self.failing == Some(self.reads.get()) {
                return Err(RuntimeError::plain("the bridge didn't answer"));
            }
            if self.stranger {
                tracks.push(track(2, "live:pad", "3-Pad"));
            }
            if self.lands_after.is_some_and(|after| self.reads.get() > after) {
                // Next to the clip's track, and the auto-named tracks after it renumbered, as Live does: the Bass keeps
                // its identity under its new name.
                tracks.insert(1, track(1, "live:new", "2-Melody to MIDI"));
                tracks[2]["name"] = json!("3-Bass");
            }
        }
        let items = if args["kind"] == "track" { json!(tracks) } else { json!([]) };
        Ok(wrap(json!({"epoch":7,"kind":args["kind"],"items":items,"revision":"r1","truncated":false})))
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

/// What a conversion did: its result, the Python Kumi ran, what Kumi said meanwhile, its change records, how often
/// Live was brought forward, and whether the turn's references retired (the lease bumped, a known ref gone).
struct Run {
    result: ToolResult,
    python: Vec<JsonObject>,
    said: Vec<String>,
    records: Vec<Value>,
    fronted: usize,
    retired: bool,
}
async fn convert(answer: Value, lands_after: Option<usize>, input: Value) -> Run {
    convert_with(answer, lands_after, None, false, input).await
}
async fn convert_with(answer: Value, lands_after: Option<usize>, failing: Option<usize>, stranger: bool, input: Value) -> Run {
    let live = Rc::new(Live {
        answer,
        lands_after,
        failing,
        stranger,
        python: RefCell::new(vec![]),
        converting: Cell::new(false),
        reads: Cell::new(0),
    });
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    let endpoint = live.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    options.now = Some(Rc::new(|| chrono::DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.epoch.set(Some(7.0));
    let records = Rc::new(RefCell::new(vec![]));
    let out = records.clone();
    let remember = Remember::new(connection.clone(), None, None);
    let history = Rc::new(History::new(
        connection.clone(),
        remember.clone(),
        None,
        Some(Rc::new(move |record| out.borrow_mut().push(serde_json::to_value(record).unwrap()))),
    ));
    let said = Rc::new(RefCell::new(vec![]));
    let out = said.clone();
    let fronted = Rc::new(Cell::new(0));
    let count = fronted.clone();
    let commands = CommandTools::new(
        connection.clone(),
        history,
        remember,
        CommandToolsOptions {
            // No menus at all: the API needs none.
            hands: Some(HandsSetup::Disabled),
            on_action: Some(Rc::new(move |event: ActionEvent| out.borrow_mut().push(event.title))),
            front_live: Some(Rc::new(move || {
                count.set(count.get() + 1);
                async { true }.boxed_local()
            })),
            ..Default::default()
        },
        Rc::new(|_, _, _| async { Ok(ToolResult::text("unused")) }.boxed_local()),
    );
    connection.references.borrow_mut().refs.insert("7:track:1".into(), "track".into());
    let lease = connection.lease.get();
    let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
    let retired = connection.lease.get() == lease + 1 && !connection.references.borrow().refs.contains_key("7:track:1");
    let python = live.python.borrow().clone();
    let said = said.borrow().clone();
    let records = records.borrow().clone();
    Run { result, python, said, records, fronted: fronted.get(), retired }
}
fn melody(clip: &str) -> Value {
    json!({"command":"convert_melody_to_midi","clip":clip})
}
fn landed(run: &Run) -> Value {
    serde_json::from_str::<Value>(&run.result.text).unwrap()["newTracks"].clone()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_clip_converts_through_lives_api_and_its_new_track_is_named() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let run = convert(json!({"ok":true,"result":{"name":"Vox take"}}), Some(2), melody("7:clip:1:0")).await;
            assert!(!run.result.is_error, "{}", run.result.text);
            let result: Value = serde_json::from_str(&run.result.text).unwrap();
            assert_eq!((result["converted"].clone(), result["from"].clone()), (json!("Melody to MIDI"), json!("Vox take")));
            // Only Live's new track: "3-Bass" is the Bass renumbered, the same track by its identity.
            assert_eq!(result["newTracks"], json!(["2-Melody to MIDI"]));
            // The clip by its ref, with Live's own type for a melody.
            assert_eq!(run.python.len(), 1);
            assert_eq!(run.python[0]["ref"], json!("7:clip:1:0"));
            let code = run.python[0]["code"].as_str().unwrap();
            assert!(code.contains("clip = obj if True") && code.contains("AudioToMidiType.melody_to_midi"), "{code}");
            assert_eq!(run.said, ["Melody to MIDI: converting “Vox take”", "Melody to MIDI: new track “2-Melody to MIDI”"]);
            assert_eq!(run.fronted, 0, "it landed before Live needed bringing forward");
            assert!(run.retired, "the tracks after the new one moved: the turn's references retire");
            assert_eq!(
                (run.records[0]["title"].clone(), run.records[0]["state"].clone()),
                (json!("Melody to MIDI: new track “2-Melody to MIDI”"), json!("kept"))
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn live_is_brought_forward_when_it_hangs_back_and_the_selected_clip_converts() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Twelve seconds without a track: Live comes forward, as a fallback.
            let run = convert(
                json!({"ok":true,"result":{"name":"Loop"}}),
                Some(50),
                json!({"command":"convert_drums_to_midi","clip":"selected"}),
            )
            .await;
            assert!(!run.result.is_error, "{}", run.result.text);
            assert_eq!(run.fronted, 1);
            let forward = "Drums to MIDI: Live hasn't finished yet, so Kumi brought its window forward in case it's waiting for it";
            assert!(run.said.contains(&forward.to_owned()), "{:?}", run.said);
            let code = run.python[0]["code"].as_str().unwrap();
            assert!(run.python[0].get("ref").is_none() && code.contains("clip = obj if False else song.view.detail_clip"), "{code}");
            assert!(code.contains("AudioToMidiType.drums_to_midi") && code.contains("its track is frozen"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn what_live_refuses_or_never_finishes_is_said() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let midi = json!({"ok":false,"error":{"message":"that's a MIDI clip: only an audio clip converts to MIDI"}});
            let run = convert(midi, Some(0), melody("7:clip:0:0")).await;
            assert!(run.result.is_error);
            assert_eq!(run.result.text, "Live didn't start Melody to MIDI: that's a MIDI clip: only an audio clip converts to MIDI");
            assert!(!run.retired, "nothing started, so nothing moved");
            // A stale clip ref, as the bridge words it.
            let stale = json!({"ok":false,"error":{"message":"ValueError: stale reference 7:clip:1:0"}});
            let run = convert(stale, Some(0), melody("7:clip:1:0")).await;
            assert_eq!(run.result.text, "That clip isn't in Live any more: discover it again, then ask again.");
            // A conversion with no new track after two minutes.
            let run = convert(json!({"ok":true,"result":{"name":"Vox take"}}), None, melody("7:clip:1:0")).await;
            assert!(
                run.result.is_error && run.result.text.starts_with("Live hasn't finished Melody to MIDI on “Vox take” after two minutes")
            );
            assert!(run.retired, "Live may still land it: references retire on giving up too");
            // No clip named.
            let run = convert(json!({"ok":true,"result":{}}), Some(0), json!({"command":"convert_harmony_to_midi"})).await;
            assert!(run.result.is_error && run.result.text.starts_with("Harmony to MIDI works on an audio clip"));
            assert!(run.python.is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_failed_read_or_a_producers_track_doesnt_mislead_the_wait() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // A read that fails while Live converts is read again, never reported as the conversion failing: asking
            // again would land a second track.
            let run = convert_with(json!({"ok":true,"result":{"name":"Vox take"}}), Some(3), Some(2), false, melody("7:clip:1:0")).await;
            assert!(!run.result.is_error, "{}", run.result.text);
            assert_eq!(landed(&run), json!(["2-Melody to MIDI"]));
            // A track the producer adds meanwhile isn't taken for Live's: the new track is told by identity, right after
            // the clip's track, or by its name.
            let from_vox = json!({"ok":true,"result":{"name":"Vox take","track":"live:vox"}});
            let run = convert_with(from_vox, Some(3), None, true, melody("7:clip:1:0")).await;
            assert_eq!(landed(&run), json!(["2-Melody to MIDI"]));
            assert!(run.retired);
            let run = convert_with(json!({"ok":true,"result":{"name":"Vox take"}}), Some(3), None, true, melody("7:clip:1:0")).await;
            assert_eq!(landed(&run), json!(["2-Melody to MIDI"]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_live_without_the_api_goes_to_its_menus() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for unavailable in ["LookupError: this Live has no conversions API", "ValueError: Live's Python module is unavailable"] {
                let run = convert(json!({"ok":false,"error":{"message":unavailable}}), Some(0), melody("7:clip:1:0")).await;
                // The menus need hands, which this computer hasn't: their own error, not the API's.
                assert!(run.result.is_error && run.result.text.starts_with("Kumi can't use Live's menus"), "{}", run.result.text);
            }
        })
        .await;
}
