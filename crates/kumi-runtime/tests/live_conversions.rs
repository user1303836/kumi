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
/// conversion starts (never, when None).
struct Live {
    answer: Value,
    lands_after: Option<usize>,
    python: RefCell<Vec<JsonObject>>,
    converting: Cell<bool>,
    reads: Cell<usize>,
}
fn wrap(value: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&value)}],"structuredContent":value})).unwrap()
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
        let mut tracks = vec![json!({"ref":"7:track:0","name":"1-Vox"}), json!({"ref":"7:track:1","name":"2-Bass"})];
        if args["kind"] == "track" && self.converting.get() {
            self.reads.set(self.reads.get() + 1);
            if self.lands_after.is_some_and(|after| self.reads.get() > after) {
                // Next to the clip's track, and the auto-named tracks after it renumbered, as Live does.
                tracks = vec![
                    json!({"ref":"7:track:0","name":"1-Vox"}),
                    json!({"ref":"7:track:1","name":"2-Melody to MIDI"}),
                    json!({"ref":"7:track:2","name":"3-Bass"}),
                ];
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

/// What a conversion did: its result, the Python Kumi ran, what Kumi said meanwhile, its change records, and how
/// often Live was brought forward.
struct Run {
    result: ToolResult,
    python: Vec<JsonObject>,
    said: Vec<String>,
    records: Vec<Value>,
    fronted: usize,
}
async fn convert(answer: Value, lands_after: Option<usize>, input: Value) -> Run {
    let live = Rc::new(Live { answer, lands_after, python: RefCell::new(vec![]), converting: Cell::new(false), reads: Cell::new(0) });
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
    let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
    let python = live.python.borrow().clone();
    let said = said.borrow().clone();
    let records = records.borrow().clone();
    Run { result, python, said, records, fronted: fronted.get() }
}
fn melody(clip: &str) -> Value {
    json!({"command":"convert_melody_to_midi","clip":clip})
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_clip_converts_through_lives_api_and_its_new_track_is_named() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let run = convert(json!({"ok":true,"result":{"name":"Vox take"}}), Some(2), melody("7:clip:1:0")).await;
            assert!(!run.result.is_error, "{}", run.result.text);
            let result: Value = serde_json::from_str(&run.result.text).unwrap();
            assert_eq!((result["converted"].clone(), result["from"].clone()), (json!("Melody to MIDI"), json!("Vox take")));
            // Only Live's new track: "3-Bass" is the Bass renumbered, not new.
            assert_eq!(result["newTracks"], json!(["2-Melody to MIDI"]));
            // The clip by its ref, with Live's own type for a melody.
            assert_eq!(run.python.len(), 1);
            assert_eq!(run.python[0]["ref"], json!("7:clip:1:0"));
            let code = run.python[0]["code"].as_str().unwrap();
            assert!(code.contains("clip = obj if True") && code.contains("AudioToMidiType.melody_to_midi"), "{code}");
            assert_eq!(run.said, ["Melody to MIDI: converting “Vox take”", "Melody to MIDI: new track “2-Melody to MIDI”"]);
            assert_eq!(run.fronted, 0, "it landed before Live needed bringing forward");
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
            let run = convert(
                json!({"ok":true,"result":{"name":"Loop"}}),
                Some(20),
                json!({"command":"convert_drums_to_midi","clip":"selected"}),
            )
            .await;
            assert!(!run.result.is_error, "{}", run.result.text);
            assert_eq!(run.fronted, 1);
            assert!(
                run.said.contains(&"Drums to MIDI: Live converts in front, so Kumi brought its window forward".to_owned()),
                "{:?}",
                run.said
            );
            let code = run.python[0]["code"].as_str().unwrap();
            assert!(run.python[0].get("ref").is_none() && code.contains("clip = obj if False else song.view.detail_clip"), "{code}");
            assert!(code.contains("AudioToMidiType.drums_to_midi"));
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
            assert_eq!(run.result.text, "Kumi couldn't start Melody to MIDI: that's a MIDI clip: only an audio clip converts to MIDI");
            // A conversion with no new track after two minutes.
            let run = convert(json!({"ok":true,"result":{"name":"Vox take"}}), None, melody("7:clip:1:0")).await;
            assert!(
                run.result.is_error && run.result.text.starts_with("Live hasn't finished Melody to MIDI on “Vox take” after two minutes")
            );
            // No clip named.
            let run = convert(json!({"ok":true,"result":{}}), Some(0), json!({"command":"convert_harmony_to_midi"})).await;
            assert!(run.result.is_error && run.result.text.starts_with("Harmony to MIDI works on an audio clip"));
            assert!(run.python.is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_live_without_the_api_goes_to_its_menus() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let old = json!({"ok":false,"error":{"message":"LookupError: this Live has no conversions API"}});
            let run = convert(old, Some(0), melody("7:clip:1:0")).await;
            // The menus need hands, which this computer hasn't: their own error, not the API's.
            assert!(run.result.is_error && run.result.text.starts_with("Kumi can't use Live's menus"), "{}", run.result.text);
        })
        .await;
}
