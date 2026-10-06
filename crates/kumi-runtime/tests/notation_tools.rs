//! Kumi's notation in the tools: the write tools' notation becomes the notes Live takes (with the length, an
//! Arrangement clip's start and drum names filled in), and read_notes prints a clip's notes in its frame.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        more_changes::set_meter,
        notes::{expand, read_notes},
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

/// A Live with a Drum Rack on track 0 (pads Kick 808 at 36 and Snare at 38), a Session clip on track 0 slot 0, and an
/// Arrangement clip on track 1 at bar 9, with their notes.
struct Live {
    calls: RefCell<Vec<JsonObject>>,
}
fn reply(kind: &str, items: Value) -> CallToolResult {
    let body = json!({"epoch":7,"kind":kind,"items":items,"revision":"r1","truncated":false});
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap()
}
#[async_trait(?Send)]
impl McpEndpoint for Live {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fake","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":[{"name":"live_discover","inputSchema":{"type":"object"}}]})).unwrap())
    }
    async fn call(&self, _: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push(args.clone());
        let parent = args.get("parent").and_then(Value::as_str).unwrap_or("");
        let pad = |name: &str, note: u8| json!({"name":name,"note":note,"chains":[{"name":name}]});
        Ok(match args["kind"].as_str().unwrap() {
            "device" if args.get("filters").is_some() => reply(
                "device",
                json!([{"ref":"7:device:0:0","drumPads":[pad("Kick 808", 36), pad("Snare", 38), {"name":"Pad 3","note":39,"chains":[]}]}]),
            ),
            "device" if parent == "7:track:0" => reply("device", json!([{"ref":"7:device:0:0","className":"DrumGroupDevice","canHaveDrumPads":true}])),
            "device" => reply("device", json!([{"ref":"7:device:1:0","className":"Operator","canHaveDrumPads":false}])),
            "session-clip" => reply(
                "session-clip",
                json!([{"ref":"7:clip:0:0","name":"Beat","length":4.0,"isAudio":false,"signatureNumerator":4,"signatureDenominator":4}]),
            ),
            "arrangement-clip" if parent == "7:track:1" => {
                reply("arrangement-clip", json!([{"ref":"7:arrangement_clip:1:0","name":"Bass","start":32.0,"length":8.0,"isAudio":false}]))
            }
            "arrangement-clip" => reply("arrangement-clip", json!([{"ref":"7:arrangement_clip:2:0","name":"Vox","start":0.0,"length":4.0,"isAudio":true}])),
            "note" if parent == "7:clip:0:0" => reply(
                "note",
                json!((0..8).map(|step| json!({"pitch": if step % 2 == 0 { 36 } else { 38 },"start":step as f64 * 0.5,"duration":0.25,"velocity":100})).collect::<Vec<_>>()),
            ),
            "note" => reply("note", json!([{"pitch":36,"start":0.0,"duration":1.5,"velocity":110},{"pitch":43,"start":2.0,"duration":0.5,"velocity":90,"releaseVelocity":20}])),
            other => panic!("not a read here: {other}"),
        })
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
async fn connection() -> (Rc<Live>, Rc<LiveConnection>) {
    let live = Rc::new(Live { calls: RefCell::new(vec![]) });
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    let endpoint = live.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    set_meter(4., 4.);
    (live, connection)
}
fn object(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

#[tokio::test(flavor = "current_thread")]
async fn notation_becomes_the_notes_live_takes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (live, connection) = connection().await;
            let signal = Signal::new();
            // A Session clip: the clip's own bars, the length whole bars of them.
            let input =
                object(json!({"trackRef":"7:track:1","sceneIndex":0,"name":"Riff","notation":"l/8 1|1 C3 D3 v90 E3 . 2|1 [C3 E3 G3]/2"}));
            let written = expand("write_midi_clip", input, &connection, Some(120.), &signal).await.unwrap();
            assert_eq!(written["length"], json!(8.0));
            assert_eq!(written["notes"][2], json!({"pitch":64,"start":1.0,"duration":0.5,"velocity":90}));
            assert_eq!(written["notes"].as_array().unwrap().len(), 6);
            assert!(!written.contains_key("notation"));
            assert!(live.calls.borrow().is_empty(), "no lane names a drum: nothing read");
            // An Arrangement clip in song time: it starts at the bar of its first note.
            let input = object(json!({"trackRef":"7:track:1","notation":"9|3 C2/1"}));
            let written = expand("write_arrangement_clip", input, &connection, Some(120.), &signal).await.unwrap();
            assert_eq!((written["start"].clone(), written["length"].clone()), (json!(32.0), json!(8.0)));
            assert_eq!(written["notes"], json!([{"pitch":48,"start":2.0,"duration":4.0,"velocity":100}]));
            // Several clips, and drums named by the track's pads.
            let input =
                object(json!({"clips":[{"trackRef":"7:track:0","start":0,"length":4,"notation":"kick x... *4\nsnare ....x... *2"}]}));
            let written = expand("write_arrangement_clip", input, &connection, Some(120.), &signal).await.unwrap();
            let pitches: Vec<_> = written["clips"][0]["notes"].as_array().unwrap().iter().map(|n| n["pitch"].as_u64().unwrap()).collect();
            assert_eq!(pitches, [36, 36, 38, 36, 36, 38]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_mistake_in_the_notation_is_the_changes_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_, connection) = connection().await;
            let signal = Signal::new();
            let error =
                expand("write_midi_clip", object(json!({"trackRef":"7:track:1","notation":"1|1 C3 Q3"})), &connection, None, &signal).await;
            assert_eq!(
                error.unwrap_err(),
                "Notation line 1, column 8: “Q3” isn't a pitch: write Live's names (C3 is middle C, F#2, Bb1) or a MIDI number (0–127)"
            );
            let both = object(json!({"trackRef":"7:track:1","notes":[],"notation":"1|1 C3"}));
            assert!(expand("write_midi_clip", both, &connection, None, &signal).await.unwrap_err().contains("not both"));
            let short = object(json!({"trackRef":"7:track:1","length":4,"notation":"2|1 C3"}));
            assert!(expand("write_midi_clip", short, &connection, None, &signal).await.unwrap_err().contains("ends after the clip"));
            let clip = object(json!({"clips":[{"trackRef":"7:track:1","start":0,"notation":"1|1 snr"}]}));
            assert!(expand("write_arrangement_clip", clip, &connection, None, &signal)
                .await
                .unwrap_err()
                .starts_with("clips[0]: Notation line 1"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn read_notes_prints_each_clip_in_its_own_frame() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_, connection) = connection().await;
            let read = read_notes(
                &object(json!({"clipRefs":["7:clip:0:0","7:arrangement_clip:1:0","7:arrangement_clip:2:0"]})),
                &connection,
                Some(120.),
                Signal::new(),
            )
            .await;
            let clips: Value = serde_json::from_str(&read.text).unwrap();
            // A Session clip on a Drum Rack track: its own time, drums as lanes named by the pads.
            assert_eq!(clips["clips"][0]["notation"], json!("kick808 x... *4\nsnare ..x. *4"));
            assert_eq!((clips["clips"][0]["notes"].clone(), clips["clips"][0]["exact"].clone()), (json!(8), json!(true)));
            // An Arrangement clip at bar 9: song time; a release velocity isn't written, so it isn't exact.
            assert_eq!(clips["clips"][1]["notation"], json!("9|1 l/4. v110 C1 9|3 v90 G1/8"));
            assert_eq!(clips["clips"][1]["time"], json!("song time: the clip runs from 9|1 to 11|1"));
            assert_eq!(clips["clips"][1]["exact"], json!(false));
            assert!(clips["clips"][2]["error"].as_str().unwrap().contains("audio clip"));
            // Live's own rows, for exact edits.
            let read =
                read_notes(&object(json!({"clipRef":"7:arrangement_clip:1:0","format":"json"})), &connection, None, Signal::new()).await;
            let clips: Value = serde_json::from_str(&read.text).unwrap();
            assert_eq!(clips["clips"][0]["notes"][1]["releaseVelocity"], json!(20));
            assert!(read_notes(&object(json!({})), &connection, None, Signal::new()).await.is_error);
        })
        .await;
}
