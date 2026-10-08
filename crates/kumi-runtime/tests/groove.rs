//! The groove judge against a Live that answers as Kumi's bridge does (each row only the fields asked for, with its ref
//! and parent): it moves a part's notes by their ids, keeps them inside the clip, leaves alone the notes Live never
//! plays, and finds a clip's track from the short ref the model has.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        connection::LiveConnection,
        history::History,
        notes::{clip_and_track, track_name},
        observation::Observer,
        options::{AbletonOptions, EarsSetup},
        remember::Remember,
        rendering::{GrooveRequest, Rendering},
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// A Live with two Drum Rack tracks ("Beat" and "Reference", pads Kick 36, Snare 38 and Hihat 42), each with a
/// one-bar looped Session clip in its first slot, and a third track holding a drum stem.
struct Live {
    notes: RefCell<HashMap<String, Vec<Value>>>,
    /// Whether note rows carry their ids when asked (an old bridge's didn't).
    ids: bool,
}

/// A one-bar beat: kick on 1 and 3, snare on 2 and 4, a hat on every sixteenth (each a sixteenth long, so the last
/// ends at the loop's end) `late` ms behind, accented on the beats when `accents`.
fn beat(late: f64, accents: bool) -> Vec<Value> {
    let ms = 500.;
    let mut notes = vec![];
    for at in [0., 2.] {
        notes.push((36, at, 0.25, 110.));
    }
    for at in [1., 3.] {
        notes.push((38, at, 0.25, 100.));
    }
    for step in 0..16 {
        let velocity = if !accents {
            90.
        } else if step % 4 == 0 {
            100.
        } else {
            70.
        };
        notes.push((42, step as f64 * 0.25 + late / ms, 0.25, velocity));
    }
    notes
        .into_iter()
        .enumerate()
        .map(|(id, (pitch, start, duration, velocity))| json!({"id":id+1,"pitch":pitch,"start":start,"duration":duration,"velocity":velocity,"mute":false}))
        .collect()
}

fn page(kind: &str, items: Value) -> CallToolResult {
    let body = json!({"epoch":7,"kind":kind,"items":items,"revision":"r1","truncated":false});
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap()
}

/// A row as the bridge gives it: the fields asked for, and its ref and parent.
fn asked(row: &Value, args: &JsonObject) -> Value {
    let fields: Vec<&str> = args.get("fields").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let kept = row.as_object().unwrap().iter().filter(|(key, _)| fields.contains(&key.as_str()) || *key == "ref" || *key == "parentRef");
    Value::Object(kept.map(|(key, value)| (key.clone(), value.clone())).collect())
}

#[async_trait(?Send)]
impl McpEndpoint for Live {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fake","version":"1.0.89"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":[{"name":"live_discover","inputSchema":{"type":"object"}}]})).unwrap())
    }
    async fn call(&self, _: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        let parent = args.get("parent").and_then(Value::as_str).unwrap_or("").to_owned();
        let rows = |items: Vec<Value>| Value::Array(items.iter().map(|row| asked(row, &args)).collect());
        let pad = |name: &str, note: u8| json!({"name":name,"note":note,"chains":[{"name":name}]});
        Ok(match args["kind"].as_str().unwrap() {
            "track" => page(
                "track",
                rows(vec![
                    json!({"ref":"7:track:0","name":"Beat"}),
                    json!({"ref":"7:track:1","name":"Reference"}),
                    json!({"ref":"7:track:2","name":"Drum stem"}),
                ]),
            ),
            "device" if args.get("filters").is_some() => {
                page("device", json!([{"ref":"7:device:0:0","drumPads":[pad("Kick", 36), pad("Snare", 38), pad("Hihat", 42)]}]))
            }
            "device" if parent == "7:track:0" || parent == "7:track:1" => {
                page("device", json!([{"ref":"7:device:0:0","className":"DrumGroupDevice","canHaveDrumPads":true}]))
            }
            "device" => page("device", json!([])),
            "session-clip" => {
                let clip = parent.replacen(":clip_slot:", ":clip:", 1);
                let row = json!({"ref":clip,"parentRef":parent,"name":if clip == "7:clip:0:0" { "Beat" } else { "Reference" },
                    "length":4.0,"isAudio":false,"signatureNumerator":4,"signatureDenominator":4,
                    "looping":true,"loopStart":0.0,"loopEnd":4.0,"startMarker":0.0,"endMarker":4.0});
                page("session-clip", rows(vec![row]))
            }
            "note" => {
                let notes = self.notes.borrow().get(&parent).cloned().unwrap_or_default();
                let notes = notes.into_iter().map(|mut row| {
                    row["parentRef"] = json!(parent);
                    if !self.ids {
                        row.as_object_mut().unwrap().remove("id");
                    }
                    row
                });
                page("note", rows(notes.collect()))
            }
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

/// The part (straight hats, flat velocities, and a note past the loop's end, which Live never plays) and the
/// reference (hats 12 ms behind, accented).
fn live(ids: bool) -> Rc<Live> {
    let mut part = beat(0., false);
    part.push(json!({"id":99,"pitch":42,"start":4.5,"duration":0.25,"velocity":90,"mute":false}));
    Rc::new(Live { notes: RefCell::new(HashMap::from([("7:clip:0:0".into(), part), ("7:clip:1:0".into(), beat(12., true))])), ids })
}

/// The groove judge over `live`, and the changes it made (each tool and input), applied to `live`'s notes.
async fn groove(live: Rc<Live>) -> (Rc<Rendering>, Rc<LiveConnection>, Rc<RefCell<Vec<(String, JsonObject)>>>) {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    let endpoint = live.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    options.ears = Some(EarsSetup::Disabled);
    let options = Rc::new(options);
    let connection = LiveConnection::new(options.connection_options());
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.available.set(true);
    connection.epoch.set(Some(7.));
    let remember = Remember::new(connection.clone(), None, None);
    let history = Rc::new(History::new(connection.clone(), remember.clone(), Some(2000), None));
    let observer = Rc::new(Observer::new(connection.clone(), remember));
    observer.tempo.set(Some(120.));
    let made = Rc::new(RefCell::new(vec![]));
    let changes = made.clone();
    let rendering = Rendering::new(
        history,
        observer,
        &options,
        Rc::new(move |tool: String, input: JsonObject, _| {
            if tool == "change_notes" {
                let mut notes = live.notes.borrow_mut();
                let clip = notes.get_mut(input["clipRef"].as_str().unwrap()).unwrap();
                for patch in input["notes"].as_array().unwrap() {
                    let note = clip.iter_mut().find(|note| note["id"] == patch["id"]).unwrap();
                    for (key, value) in patch.as_object().unwrap() {
                        note[key] = value.clone();
                    }
                }
            }
            changes.borrow_mut().push((tool, input));
            async { Ok(JsonObject::new()) }.boxed_local()
        }),
        Rc::new(|_, _| async { Ok(None) }.boxed_local()),
    );
    (rendering, connection, made)
}

fn request(value: Value) -> GrooveRequest {
    let input = value.as_object().unwrap();
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    GrooveRequest {
        clip: text("clip"),
        reference: text("reference"),
        apply: input.get("apply") == Some(&json!(true)),
        ..Default::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn groove_moves_the_notes_live_plays_by_their_ids_and_keeps_them_in_the_clip() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let live = live(true);
            let (rendering, _, made) = groove(live.clone()).await;
            let start =
                rendering.groove(&request(json!({"clip":"7:clip:0:0","reference":"7:clip:1:0"})), Signal::new()).await.unwrap().unwrap();
            let gap =
                |rows: &[kumi_runtime::listening::checklist::Row], id: &str| rows.iter().find(|row| row.id == id).map(|row| row.gap_after);
            assert!(gap(&start.rows, "timing hats").is_some_and(|gap| gap > 0.), "{:?}", start.rows);
            assert!(gap(&start.rows, "velocity hats").is_some_and(|gap| gap > 0.), "{:?}", start.rows);
            let round = rendering.groove(&request(json!({"apply":true})), Signal::new()).await.unwrap().unwrap();
            // One change, every patch by a note's id, none for the note past the loop's end.
            let made = made.borrow();
            assert_eq!(made.len(), 1);
            assert_eq!(made[0].0, "change_notes");
            let patches = made[0].1["notes"].as_array().unwrap();
            assert!(!patches.is_empty() && patches.iter().all(|patch| patch["id"].as_i64().is_some_and(|id| id >= 1)), "{patches:?}");
            assert!(patches.iter().all(|patch| patch["id"] != 99), "{patches:?}");
            // Laid back, the last hat would run past the loop's end: it's cut to end there.
            for note in &live.notes.borrow()["7:clip:0:0"] {
                if note["id"] != 99 {
                    let end = note["start"].as_f64().unwrap() + note["duration"].as_f64().unwrap();
                    assert!(end <= 4. + 1e-9, "{note}");
                }
            }
            assert_eq!(round.kept, Some(true), "{:?}", round.why);
            assert!(gap(&round.rows, "timing hats") == Some(0.), "{:?}", round.rows);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn without_note_ids_groove_says_why_it_cant_move_them() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (rendering, _, made) = groove(live(false)).await;
            rendering.groove(&request(json!({"clip":"7:clip:0:0","reference":"7:clip:1:0"})), Signal::new()).await.unwrap().unwrap();
            let refused = rendering.groove(&request(json!({"apply":true})), Signal::new()).await.unwrap().unwrap_err();
            assert!(refused.contains("without their ids"), "{refused}");
            assert!(made.borrow().is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_short_ref_finds_its_clips_track() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_, connection, _) = groove(live(true)).await;
            // The model has the drum stem's short ref; Live's rows come back with short refs too.
            let short = connection.references.borrow_mut().short_ref("7:arrangement_clip:2:0");
            let (long, track) = clip_and_track(&connection, &short).unwrap();
            assert_eq!((long.as_str(), track.as_str()), ("7:arrangement_clip:2:0", "7:track:2"));
            assert_eq!(track_name(&connection, &track, Signal::new()).await.as_deref(), Some("Drum stem"));
        })
        .await;
}
