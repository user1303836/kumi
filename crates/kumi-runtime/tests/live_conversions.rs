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

/// A Live with two audio tracks whose Python answers as told, and whose new MIDI track (`new_name`) shows `lands_after`
/// track reads after a conversion starts (never, when None). The answer `{"lost": why}` loses the Python call's answer
/// after Live took it; `{"host": reason}` is the bridge's own error. `failing` is a read during the conversion that
/// fails; `stranger` is a track of the producer's (where, name, media kind) that shows while Kumi waits; `no_python`
/// leaves Python out of the bridge's tools.
#[derive(Default)]
struct Setup {
    answer: Value,
    lands_after: Option<usize>,
    new_name: Option<&'static str>,
    failing: Option<usize>,
    stranger: Option<(usize, &'static str, &'static str)>,
    no_python: bool,
}
struct Live {
    setup: Setup,
    python: RefCell<Vec<JsonObject>>,
    converting: Cell<bool>,
    reads: Cell<usize>,
}
fn wrap(value: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&value)}],"structuredContent":value})).unwrap()
}
fn track(identity: &str, name: &str, media: &str) -> Value {
    json!({"objectIdentity":identity,"name":name,"mediaKind":media})
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
            .filter(|name| !(self.setup.no_python && **name == "live_run_python"))
            .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}))
            .collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        let answer = &self.setup.answer;
        if name == "live_run_python" {
            self.python.borrow_mut().push(args);
            if let Some(lost) = answer.get("lost").and_then(Value::as_str) {
                self.converting.set(true);
                return Err(RuntimeError::plain(lost));
            }
            if let Some(reason) = answer.get("host") {
                // The bridge's error, as it words one (host/helpers.rs reason_error).
                let text = stringify(
                    &json!({"reason":reason,"remediation":"Read the Set again before continuing; a dispatched script may have changed it."}),
                );
                return Ok(serde_json::from_value(json!({"content":[{"type":"text","text":text}],"isError":true})).unwrap());
            }
            let asked = answer["stdout"].as_str().is_some_and(|out| out.contains("asked Live to convert"));
            self.converting.set(answer["ok"] == true || asked);
            return Ok(wrap(answer.clone()));
        }
        let mut tracks = vec![track("live:vox", "1-Vox", "audio"), track("live:bass", "2-Bass", "audio")];
        if args["kind"] == "track" && self.converting.get() {
            self.reads.set(self.reads.get() + 1);
            if self.setup.failing == Some(self.reads.get()) {
                return Err(RuntimeError::plain("the bridge didn't answer"));
            }
            if let Some((at, name, media)) = self.setup.stranger {
                tracks.insert(at, track("live:stranger", name, media));
            }
            if self.setup.lands_after.is_some_and(|after| self.reads.get() > after) {
                // Right after the clip's track, and the auto-named tracks after it renumbered, as Live does: the Bass
                // keeps its identity under its new name.
                tracks.insert(1, track("live:new", self.setup.new_name.unwrap_or("2-Melody to MIDI"), "midi"));
                let bass = tracks.iter_mut().find(|track| track["objectIdentity"] == "live:bass").unwrap();
                bass["name"] = json!("3-Bass");
            }
        }
        for (index, track) in tracks.iter_mut().enumerate() {
            track["ref"] = json!(format!("7:track:{index}"));
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
/// Live was brought forward, whether the turn's references retired (the lease bumped, a known ref gone), and how many
/// times the tracks were read while Live converted.
struct Run {
    result: ToolResult,
    python: Vec<JsonObject>,
    said: Vec<String>,
    records: Vec<Value>,
    fronted: usize,
    retired: bool,
    reads: usize,
}
async fn convert(answer: Value, lands_after: Option<usize>, input: Value) -> Run {
    convert_with(Setup { answer, lands_after, ..Default::default() }, input).await
}
async fn convert_with(setup: Setup, input: Value) -> Run {
    let live = Rc::new(Live { setup, python: RefCell::new(vec![]), converting: Cell::new(false), reads: Cell::new(0) });
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
    Run { result, python, said, records, fronted: fronted.get(), retired, reads: live.reads.get() }
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
async fn what_live_refuses_is_said_and_what_may_still_land_isnt_a_failure() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let midi = json!({"ok":false,"error":{"type":"ValueError","message":"that's a MIDI clip: only an audio clip converts to MIDI"}});
            let run = convert(midi, Some(0), melody("7:clip:0:0")).await;
            assert!(run.result.is_error);
            assert_eq!(run.result.text, "Live didn't start Melody to MIDI: that's a MIDI clip: only an audio clip converts to MIDI");
            assert!(!run.retired, "nothing started, so nothing moved");
            // A clip ref the bridge no longer holds, in both of its words for it.
            for gone in [
                json!({"ok":false,"error":{"type":"KeyError","message":"'stale or invalid reference'"}}),
                json!({"ok":false,"error":{"type":"KeyError","message":"'7:clip:1:0'"}}),
            ] {
                let run = convert(gone, Some(0), melody("7:clip:1:0")).await;
                assert_eq!(run.result.text, "That clip isn't in Live any more: discover it again, then ask again.");
            }
            // No new track after two minutes: Live may still land it, so it isn't a failure (asking again would make a
            // second track), and the reads slow after the first ten seconds.
            let run = convert(json!({"ok":true,"result":{"name":"Vox take"}}), None, melody("7:clip:1:0")).await;
            assert!(!run.result.is_error, "{}", run.result.text);
            let result: Value = serde_json::from_str(&run.result.text).unwrap();
            assert_eq!(result["converting"], json!("Melody to MIDI"));
            assert!(
                result["note"].as_str().unwrap().starts_with(
                    "Kumi hasn't seen Melody to MIDI on “Vox take” land after two minutes, and Live may still be converting. Don't ask again"
                ),
                "{result}"
            );
            assert!(run.retired, "Live may still land it: references retire on giving up too");
            assert!((140..=150).contains(&run.reads), "every 300 ms for ten seconds, then every second: {} reads", run.reads);
            // No clip named.
            let run = convert(json!({"ok":true,"result":{}}), Some(0), json!({"command":"convert_harmony_to_midi"})).await;
            assert!(run.result.is_error && run.result.text.starts_with("Harmony to MIDI works on an audio clip"));
            assert!(run.python.is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_answer_lost_once_live_may_be_converting_says_so() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let lost = "Kumi lost Live's answer to Melody to MIDI";
            // The call's answer lost on the way back.
            let run = convert(json!({"lost":"the bridge didn't answer"}), Some(0), melody("7:clip:1:0")).await;
            // Python past its time once it had asked Live (a slow conversion call), or the bridge failing on the way.
            let late = json!({"ok":false,"stdout":"kumi: asked Live to convert\n","error":{"type":"TimeoutError","message":"Python exceeded timeoutMs (10000 ms)"}});
            let slow = convert(late, Some(0), melody("7:clip:1:0")).await;
            let bridge = convert(json!({"host":"Live did not answer in time"}), Some(0), melody("7:clip:1:0")).await;
            for (run, why) in [(run, "the bridge didn't answer"), (slow, "Python exceeded timeoutMs (10000 ms)"), (bridge, "Live did not answer in time")] {
                assert!(!run.result.is_error, "{}", run.result.text);
                let result: Value = serde_json::from_str(&run.result.text).unwrap();
                let note = result["note"].as_str().unwrap();
                assert!(note.starts_with(&format!("{lost} ({why}), and Live may still be converting. Don't ask again")), "{note}");
                assert!(run.retired, "a track may land next to the clip's: references retire");
                assert_eq!(run.python.len(), 1, "never asked again");
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_failed_read_or_a_producers_track_doesnt_mislead_the_wait() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let vox = || json!({"ok":true,"result":{"name":"Vox take","track":"live:vox"}});
            // A read that fails while Live converts is read again, never reported as the conversion failing: asking
            // again would land a second track.
            let run = convert_with(
                Setup {
                    answer: json!({"ok":true,"result":{"name":"Vox take"}}),
                    lands_after: Some(3),
                    failing: Some(2),
                    ..Default::default()
                },
                melody("7:clip:1:0"),
            )
            .await;
            assert!(!run.result.is_error, "{}", run.result.text);
            assert_eq!(landed(&run), json!(["2-Melody to MIDI"]));
            // A track the producer adds meanwhile isn't taken for Live's, wherever it is: a MIDI track at the end, an
            // audio track right after the clip's (where Live inserts one for a selected track).
            for stranger in [(2, "3-Keys", "midi"), (1, "2-Audio", "audio")] {
                let run = convert_with(
                    Setup { answer: vox(), lands_after: Some(20), stranger: Some(stranger), ..Default::default() },
                    melody("7:clip:1:0"),
                )
                .await;
                assert_eq!(landed(&run), json!(["2-Melody to MIDI"]), "{stranger:?}");
                assert!(run.retired);
            }
            // Not knowing the clip's track, a new MIDI track is taken only after a few seconds without one named for the
            // conversion: Live's lands meanwhile.
            let run = convert_with(
                Setup {
                    answer: json!({"ok":true,"result":{"name":"Vox take"}}),
                    lands_after: Some(10),
                    stranger: Some((2, "3-Keys", "midi")),
                    ..Default::default()
                },
                melody("7:clip:1:0"),
            )
            .await;
            assert_eq!(landed(&run), json!(["2-Melody to MIDI"]));
            // A Live in another language names it otherwise: the new MIDI track right after the clip's is Live's.
            let run = convert_with(
                Setup { answer: vox(), lands_after: Some(3), new_name: Some("2-Melodie zu MIDI"), ..Default::default() },
                melody("7:clip:1:0"),
            )
            .await;
            assert_eq!(landed(&run), json!(["2-Melodie zu MIDI"]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_live_without_the_api_goes_to_its_menus() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let menus = |run: &Run| {
                // The menus need hands, which this computer hasn't: their own error, not the API's.
                assert!(run.result.is_error && run.result.text.starts_with("Kumi can't use Live's menus"), "{}", run.result.text);
                assert!(!run.retired);
            };
            for unavailable in [
                json!({"ok":false,"error":{"type":"LookupError","message":"this Live has no conversions API"}}),
                json!({"ok":false,"error":{"type":"ValueError","message":"Live's Python module is unavailable"}}),
                // The bridge's own words (host/reads.rs) when its Live has no Python.
                json!({"host":"Python execution is unavailable on this Live shape"}),
            ] {
                menus(&convert(unavailable, Some(0), melody("7:clip:1:0")).await);
            }
            // A bridge without Python among its tools.
            let run =
                convert_with(Setup { answer: json!({"ok":true,"result":{}}), no_python: true, ..Default::default() }, melody("7:clip:1:0"))
                    .await;
            menus(&run);
            assert!(run.python.is_empty());
        })
        .await;
}
