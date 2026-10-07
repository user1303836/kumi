//! Kumi's notation in the tools: the write tools' notation becomes the notes Live takes (with the length, an
//! Arrangement clip's start and drum names filled in), and read_notes prints a clip's notes in its frame.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        more_changes::{set_meter, set_scale},
        notes::{expand, read_notes},
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
    notation::{parse, Frame},
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

/// A Live with a Drum Rack on track 0 (pads Kick 808 at 36 and Snare at 38), Session clips on track 0, and Arrangement
/// clips on track 1 (at bar 9; split; looped; looped on a bridge without start markers), with their notes.
struct Live {
    calls: RefCell<Vec<JsonObject>>,
}
fn reply(kind: &str, items: Value) -> CallToolResult {
    page(kind, items, false)
}
fn page(kind: &str, items: Value, truncated: bool) -> CallToolResult {
    let body = json!({"epoch":7,"kind":kind,"items":items,"revision":"r1","truncated":truncated});
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
            "arrangement-clip" if parent == "7:track:1" => reply(
                "arrangement-clip",
                json!([
                    {"ref":"7:arrangement_clip:1:0","name":"Bass","start":32.0,"length":8.0,"isAudio":false},
                    // The right half of a clip split at bar 13: its window starts 16 beats into its notes.
                    {"ref":"7:arrangement_clip:1:1","name":"Split","start":48.0,"endTime":56.0,"length":8.0,"isAudio":false,
                        "looping":false,"loopStart":16.0,"loopEnd":24.0,"startMarker":16.0},
                    // A one-bar loop from beat 4, started halfway through it (beat 6), playing three bars from bar 17.
                    {"ref":"7:arrangement_clip:1:2","name":"Loop","start":64.0,"endTime":76.0,"length":4.0,"isAudio":false,
                        "looping":true,"loopStart":4.0,"loopEnd":8.0,"startMarker":6.0},
                    // A loop on a bridge that doesn't give MIDI clips' start markers.
                    {"ref":"7:arrangement_clip:1:3","name":"Old","start":80.0,"endTime":88.0,"length":4.0,"isAudio":false,
                        "looping":true,"loopStart":0.0,"loopEnd":4.0,"startMarker":null},
                    // As Live left a new clip (looped over its 16 beats) split 8 beats in: the loop kept, the marker moved.
                    {"ref":"7:arrangement_clip:1:4","name":"Right","start":40.0,"endTime":48.0,"length":16.0,"isAudio":false,
                        "looping":true,"loopStart":0.0,"loopEnd":16.0,"startMarker":8.0}
                ]),
            ),
            "arrangement-clip" => reply("arrangement-clip", json!([{"ref":"7:arrangement_clip:2:0","name":"Vox","start":0.0,"length":4.0,"isAudio":true}])),
            "note" if parent.starts_with("7:clip:0:") => page(
                "note",
                json!((0..8).map(|step| json!({"pitch": if step % 2 == 0 { 36 } else { 38 },"start":step as f64 * 0.5,"duration":0.25,"velocity":100})).collect::<Vec<_>>()),
                // Slot 1's clip has more notes than a page holds.
                parent == "7:clip:0:1",
            ),
            "note" if parent == "7:arrangement_clip:1:1" => reply(
                "note",
                json!([
                    {"pitch":48,"start":2.0,"duration":1.0,"velocity":100},
                    {"pitch":60,"start":16.0,"duration":1.0,"velocity":100},
                    {"pitch":64,"start":20.0,"duration":8.0,"velocity":100},
                    {"pitch":72,"start":24.0,"duration":1.0,"velocity":100}
                ]),
            ),
            "note" if parent == "7:arrangement_clip:1:2" => reply(
                "note",
                json!([
                    {"pitch":65,"start":1.0,"duration":1.0,"velocity":100},
                    {"pitch":60,"start":4.0,"duration":1.0,"velocity":100},
                    {"pitch":62,"start":6.0,"duration":1.0,"velocity":100},
                    {"pitch":64,"start":7.5,"duration":2.0,"velocity":100}
                ]),
            ),
            "note" if parent == "7:arrangement_clip:1:3" => reply("note", json!([{"pitch":60,"start":0.0,"duration":1.0,"velocity":100}])),
            "note" if parent == "7:arrangement_clip:1:4" => {
                reply("note", json!([0, 4, 8, 12].map(|at| json!({"pitch":60,"start":at,"duration":0.25,"velocity":100}))))
            }
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
            let written = expand("write_midi_clip", input, &connection, Some(120.), &signal).await.unwrap().input;
            assert_eq!(written["length"], json!(8.0));
            assert_eq!(written["notes"][2], json!({"pitch":64,"start":1.0,"duration":0.5,"velocity":90}));
            assert_eq!(written["notes"].as_array().unwrap().len(), 6);
            assert!(!written.contains_key("notation"));
            assert!(live.calls.borrow().is_empty(), "no lane names a drum: nothing read");
            // An Arrangement clip in song time: it starts at the bar of its first note.
            let input = object(json!({"trackRef":"7:track:1","notation":"9|3 C2/1"}));
            let written = expand("write_arrangement_clip", input, &connection, Some(120.), &signal).await.unwrap().input;
            assert_eq!((written["start"].clone(), written["length"].clone()), (json!(32.0), json!(8.0)));
            assert_eq!(written["notes"], json!([{"pitch":48,"start":2.0,"duration":4.0,"velocity":100}]));
            // Several clips, and drums named by the track's pads.
            let input = object(json!({"clips":[{"trackRef":"7:track:0","start":0,"length":4,"notation":"kick x... *4
snare ....x... *2"}]}));
            let written = expand("write_arrangement_clip", input, &connection, Some(120.), &signal).await.unwrap().input;
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
                error.err().unwrap(),
                "Notation line 1, column 8: “Q3” isn't a pitch: write Live's names (C3 is middle C, F#2, Bb1) or a MIDI number (0–127)"
            );
            let both = object(json!({"trackRef":"7:track:1","notes":[],"notation":"1|1 C3"}));
            assert!(expand("write_midi_clip", both, &connection, None, &signal).await.err().unwrap().contains("not both"));
            // Every mistake comes back at once, each with its line, so one fix mends them all (#257).
            let several = object(json!({"trackRef":"7:track:1","notation":"1|1 C3 Q3
2|1 H2
kick /16 1|1 x... *4"}));
            let error = expand("write_midi_clip", several, &connection, None, &signal).await.err().unwrap();
            assert!(error.starts_with("Notation line 1, column 8: “Q3” isn't a pitch"), "{error}");
            assert!(
                error.contains(
                    "
line 2, column 5: “H2” isn't a pitch"
                ),
                "{error}"
            );
            // A note past the clip's end is cut there, one at its end left out, and the change says so: Kumi's bridge
            // writes notes inside a clip.
            let over = object(json!({"trackRef":"7:track:1","length":4,"notation":"1|4 C3/2 D3"}));
            let written = expand("write_midi_clip", over, &connection, None, &signal).await.unwrap();
            assert_eq!(written.input["notes"], json!([{"pitch":60,"start":3.0,"duration":1.0,"velocity":100}]));
            assert_eq!(
                written.fixed,
                ["1 note at or past the clip's end (2|1) was left out: at 2|2", "1 note ran past the clip's end (2|1) and was cut there"]
            );
            // Of several clips, those whose notation reads are written; the others come back with their mistakes.
            let clips = object(json!({"clips":[
                {"trackRef":"7:track:1","start":0,"length":4,"notation":"1|1 C3 D E3"},
                {"trackRef":"7:track:1","start":4,"length":4,"notation":"2|1 C3 Q3"},
                {"trackRef":"7:track:1","start":8,"length":4,"notation":"l1 3|1 G2"}
            ]}));
            let written = expand("write_arrangement_clip", clips, &connection, None, &signal).await.unwrap();
            let written_pitches = |clip: usize| -> Vec<u64> {
                written.input["clips"][clip]["notes"].as_array().unwrap().iter().map(|n| n["pitch"].as_u64().unwrap()).collect()
            };
            assert_eq!(written.input["clips"].as_array().unwrap().len(), 2);
            assert_eq!(written_pitches(0), [60, 62, 64], "D took C3's octave");
            assert_eq!(
                written.input["clips"][1]["notes"],
                json!([{"pitch":55,"start":0.0,"duration":4.0,"velocity":100}]),
                "l1 is a whole note"
            );
            assert_eq!(written.unwritten.len(), 1);
            assert_eq!(written.unwritten[0]["clip"], 1);
            assert!(written.unwritten[0]["error"].as_str().unwrap().contains("“Q3” isn't a pitch"));
            assert_eq!(
                written.fixed,
                ["clips[0]: “D” has no octave, so it was read as D3, the octave of the pitch before it", "clips[2]: “l1” was read as l/1"]
            );
            // When none reads, the write is refused with every clip's mistakes.
            let clip = object(json!({"clips":[{"trackRef":"7:track:1","start":0,"notation":"1|1 snr"}]}));
            assert!(expand("write_arrangement_clip", clip, &connection, None, &signal)
                .await
                .err()
                .unwrap()
                .starts_with("clips[0]: Notation line 1"));
        })
        .await;
}
