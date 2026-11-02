//! A goal's rig in Live over a small Set of the test's own: a bridge that keeps tracks, devices and knobs, makes the changes
//! it's asked to, and answers every take with one short file. The oracle's replay (`live_rendering`, in Extended) pins which
//! calls a goal makes; this is what they come to in a Set that changes under them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::contracts::*;
use kumi_agent::core::evolve::Knob;
use kumi_agent::core::goal_rig::knob_key;
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_daw_ableton::{
    connection::LiveConnection,
    history::{History, LiveHistory},
    mutations::Mutations,
    observation::Observer,
    options::{AbletonOptions, EarsSetup},
    parameters::Parameters,
    remember::{CurrentProject, Remember},
    rendering::Rendering,
};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};

/// The tools a goal can reach, as the oracle's bridge lists them.
const TOOLS: [&str; 35] = [
    "server_status",
    "live_status",
    "live_discover",
    "live_snapshot",
    "live_undo",
    "live_transaction_release",
    "live_tempo_preview",
    "live_tempo_apply",
    "live_mixer_preview",
    "live_mixer_apply",
    "live_session_structure_preview",
    "live_session_structure_apply",
    "live_object_rename_preview",
    "live_object_rename_apply",
    "live_audio_capture_apply",
    "live_transport_apply",
    "live_track_properties_preview",
    "live_track_properties_apply",
    "live_device_preview",
    "live_device_apply",
    "live_browser_load_preview",
    "live_browser_load_apply",
    "live_transport_preview",
    "live_song_state",
    "live_track_structure_preview",
    "live_track_structure_apply",
    "live_device_parameter_preview",
    "live_device_parameter_apply",
    "live_transport_action_preview",
    "live_transport_action_apply",
    "live_recording_preview",
    "live_recording_apply",
    "live_session_emergency_stop",
    "live_routing_preview",
    "live_routing_apply",
];

#[derive(Clone)]
struct Device {
    /// Live's identity for it, which it keeps wherever it goes.
    identity: u32,
    name: String,
    class: String,
    knobs: Vec<(String, f64)>,
}
thread_local! {
    static IDENTITIES: Cell<u32> = const { Cell::new(0) };
}
fn next_identity() -> u32 {
    IDENTITIES.with(|next| next.replace(next.get() + 1))
}
impl Device {
    /// A device with the two knobs every test's device has.
    fn new(name: &str) -> Self {
        Device {
            identity: next_identity(),
            name: name.into(),
            class: name.into(),
            knobs: vec![("Tone".into(), 0.5), ("Drive".into(), 0.2)],
        }
    }
    fn limiter() -> Self {
        Device {
            identity: next_identity(),
            name: "Limiter".into(),
            class: "Limiter".into(),
            knobs: vec![("Gain".into(), 0.5), ("Ceiling".into(), 0.97)],
        }
    }
}
#[derive(Clone)]
struct Track {
    name: String,
    armed: bool,
    devices: Vec<Device>,
}
impl Track {
    fn new(name: &str, devices: &[&str]) -> Self {
        Track { name: name.into(), armed: false, devices: devices.iter().map(|name| Device::new(name)).collect() }
    }
}

/// Live, as far as a goal can tell: refs are places, as Live's are, so a device or a track put in above moves them.
struct Set {
    tracks: RefCell<Vec<Track>>,
    /// The one file every take plays.
    take: PathBuf,
    playing: Cell<bool>,
    next: Cell<u32>,
    previews: RefCell<HashMap<String, (String, JsonObject)>>,
    /// The tracks each transaction made, which its undo takes away.
    made: RefCell<HashMap<String, Vec<String>>>,
    /// Every call, as `{name, args}`.
    asked: RefCell<Vec<Value>>,
    /// A test's look at each call as it comes.
    on_call: RefCell<Option<Box<dyn Fn(&str)>>>,
}
fn reply(body: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap()
}
/// A ref's places: `7:parameter:2:1:0` is track 2, device 1, parameter 0.
fn places(reference: &str) -> Vec<usize> {
    reference.split(':').skip(2).filter_map(|place| place.trim_start_matches('d').parse().ok()).collect()
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
impl Set {
    fn new(take: &Path, tracks: &[(&str, &[&str])]) -> Rc<Self> {
        Rc::new(Set {
            tracks: RefCell::new(tracks.iter().map(|(name, devices)| Track::new(name, devices)).collect()),
            take: take.into(),
            playing: Cell::new(false),
            next: Cell::new(0),
            previews: RefCell::default(),
            made: RefCell::default(),
            asked: RefCell::default(),
            on_call: RefCell::new(None),
        })
    }
    /// A knob's value now, found by its names, however the Set moved.
    fn knob(&self, track: &str, device: &str, knob: &str) -> f64 {
        let tracks = self.tracks.borrow();
        let track = tracks.iter().find(|found| found.name == track).unwrap();
        let device = track.devices.iter().find(|found| found.name == device).unwrap();
        device.knobs.iter().find(|(name, _)| name == knob).unwrap().1
    }
    /// A knob's value now, on the first device of the track at this place.
    fn knob_on_track(&self, at: usize, knob: &str) -> f64 {
        self.tracks.borrow()[at].devices[0].knobs.iter().find(|(name, _)| name == knob).unwrap().1
    }
    /// A knob's value now, on the device at this place.
    fn knob_at(&self, track: &str, at: usize, knob: &str) -> f64 {
        let tracks = self.tracks.borrow();
        let track = tracks.iter().find(|found| found.name == track).unwrap();
        track.devices[at].knobs.iter().find(|(name, _)| name == knob).unwrap().1
    }
    /// The producer renames the device at this place.
    fn rename_device(&self, track: &str, at: usize, name: &str) {
        let mut tracks = self.tracks.borrow_mut();
        tracks.iter_mut().find(|found| found.name == track).unwrap().devices[at].name = name.into();
    }
    /// The producer puts a device in at this place on a track.
    fn put_device_in(&self, track: &str, at: usize, name: &str) {
        let mut tracks = self.tracks.borrow_mut();
        tracks.iter_mut().find(|found| found.name == track).unwrap().devices.insert(at, Device::new(name));
    }
    /// The producer deletes the first device of this name from a track.
    fn delete_device(&self, track: &str, name: &str) {
        let mut tracks = self.tracks.borrow_mut();
        let devices = &mut tracks.iter_mut().find(|found| found.name == track).unwrap().devices;
        let at = devices.iter().position(|device| device.name == name).unwrap();
        devices.remove(at);
    }
    /// The producer puts a track in at this place.
    fn put_track_in(&self, at: usize, name: &str, devices: &[&str]) {
        self.tracks.borrow_mut().insert(at, Track::new(name, devices));
    }
    /// The producer deletes a track.
    fn delete_track(&self, name: &str) {
        self.tracks.borrow_mut().retain(|track| track.name != name);
    }
    /// How many times this kind of thing was read.
    fn reads(&self, kind: &str) -> usize {
        self.asked.borrow().iter().filter(|call| call["name"] == "live_discover" && call["args"]["kind"] == kind).count()
    }
    fn asked_for(&self, name: &str) -> usize {
        self.asked.borrow().iter().filter(|call| call["name"] == name).count()
    }
    fn discover(&self, args: &JsonObject) -> Value {
        let tracks = self.tracks.borrow();
        let kind = text(&args["kind"]);
        let parent = args.get("parent").map(text).unwrap_or_default();
        let at = places(parent);
        let items: Vec<Value> = match kind {
            "set" => {
                vec![json!({"ref":"7:set:song","objectIdentity":"song","name":"Fixture Set","tempo":120,"position":if self.playing.get() {100} else {0},"playing":self.playing.get()})]
            }
            "track" => tracks
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    json!({"ref":format!("7:track:{index}"),"parentRef":"7:set:song","name":track.name,"color":0,"armed":track.armed,"isFrozen":false})
                })
                .collect(),
            "device" => tracks[at[0]]
                .devices
                .iter()
                .enumerate()
                .map(|(index, device)| {
                    json!({"ref":format!("7:device:{}:d{index}",at[0]),"parentRef":parent,"name":device.name,"className":device.class,"classDisplayName":device.class,"enabled":true,"objectIdentity":device.identity})
                })
                .collect(),
            "parameter" => tracks[at[0]].devices[at[1]]
                .knobs
                .iter()
                .enumerate()
                .map(|(index, (name, value))| {
                    json!({"ref":format!("7:parameter:{}:{}:{index}",at[0],at[1]),"parentRef":parent,"name":name,"value":value,"min":0,"max":1})
                })
                .collect(),
            "arrangement-clip" if tracks[at[0]].name.starts_with("Kumi · render") => {
                vec![json!({"ref":format!("7:arrangement_clip:{}:0",at[0]),"parentRef":parent,"name":"Take","isAudio":true,"start":0,"length":16,"filePath":self.take})]
            }
            "session-playback" => {
                vec![json!({"ref":"7:session_playback:0","transport":{"playing":false,"sessionRecord":false,"arrangementRecord":false},"firedTargets":[],"playingTargets":[]})]
            }
            _ => vec![],
        };
        json!({"epoch":7,"kind":kind,"items":items,"revision":"r1","truncated":false})
    }
    fn preview(&self, tool: &str, args: &JsonObject) -> Result<Value, RuntimeError> {
        if tool == "live_device_parameter" {
            // (as Live's bridge does: a ref is a place, and a place with no such parameter is refused)
            let tracks = self.tracks.borrow();
            for value in args["values"].as_array().unwrap() {
                let at = places(text(&value["parameterRef"]));
                if tracks.get(at[0]).and_then(|track| track.devices.get(at[1])).and_then(|device| device.knobs.get(at[2])).is_none() {
                    return Err(RuntimeError::plain("No such parameter."));
                }
            }
        }
        let number = self.next.get();
        self.next.set(number + 1);
        let transaction = format!("tx{number}");
        self.previews.borrow_mut().insert(transaction.clone(), (tool.into(), args.clone()));
        let prior: Vec<Value> = self
            .tracks
            .borrow()
            .iter()
            .enumerate()
            .map(|(index, track)| json!({"ref":format!("7:track:{index}"),"name":track.name,"index":index}))
            .collect();
        Ok(json!({"transactionId":transaction,"epoch":7,"confirmation":"apply","prior":{"tracks":prior,"scenes":[]},"proposed":[]}))
    }
    fn apply(&self, args: &JsonObject) -> Value {
        let transaction = text(&args["transactionId"]).to_owned();
        let (tool, asked) = self.previews.borrow_mut().remove(&transaction).unwrap();
        let mut answer = json!({"transactionId":transaction,"state":"applied"});
        let mut tracks = self.tracks.borrow_mut();
        match tool.as_str() {
            "live_session_structure" => {
                let mut created = vec![];
                for track in asked["tracks"].as_array().unwrap() {
                    let name = text(&track["name"]);
                    let at = track["index"].as_u64().map_or(tracks.len(), |at| at as usize);
                    tracks.insert(at, Track::new(name, &[]));
                    created.push(json!({"kind":"track","ref":format!("7:track:{at}"),"name":name}));
                }
                let names = created.iter().map(|track| text(&track["name"]).to_owned()).collect();
                self.made.borrow_mut().insert(transaction, names);
                answer["created"] = json!(created);
            }
            "live_routing" => {
                if let Some(arm) = asked.get("arm").and_then(Value::as_bool) {
                    tracks[places(text(&asked["trackRef"]))[0]].armed = arm;
                }
            }
            "live_browser_load" => {
                let at = places(text(&asked["trackRef"]))[0];
                tracks[at].devices.push(Device::limiter());
                answer["deviceRef"] = json!(format!("7:device:{at}:d{}", tracks[at].devices.len() - 1));
            }
            "live_device_parameter" => {
                for value in asked["values"].as_array().unwrap() {
                    let at = places(text(&value["parameterRef"]));
                    tracks[at[0]].devices[at[1]].knobs[at[2]].1 = value["value"].as_f64().unwrap();
                }
            }
            "live_recording" => self.playing.set(asked.get("action").map(text) == Some("start")),
            "live_transport_action" => {
                if asked.get("action").map(text) == Some("stop") {
                    self.playing.set(false);
                }
            }
            _ => {}
        }
        answer
    }
    fn undo(&self, args: &JsonObject) -> Value {
        let transaction = text(&args["transactionId"]);
        if let Some(made) = self.made.borrow_mut().remove(transaction) {
            self.tracks.borrow_mut().retain(|track| !made.contains(&track.name));
        }
        json!({"transactionId":transaction,"state":"undone","idempotent":false})
    }
}
#[async_trait(?Send)]
impl McpEndpoint for Set {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.92"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        let tools: Vec<Value> = TOOLS
            .iter()
            .map(|name| json!({"name":name,"description":name,"inputSchema":{"type":"object","properties":{},"additionalProperties":true}}))
            .collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        if let Some(on_call) = &*self.on_call.borrow() {
            on_call(name);
        }
        self.asked.borrow_mut().push(json!({"name":name,"args":args}));
        tokio::task::yield_now().await;
        Ok(reply(match name {
            "live_discover" => self.discover(&args),
            "live_undo" => self.undo(&args),
            "live_status" => json!({"connected":true,"adapter":"remote-script","provenance":"fake-live","epoch":7}),
            "live_transaction_release" => json!({}),
            _ => match (name.strip_suffix("_preview"), name.strip_suffix("_apply")) {
                (Some(tool), _) => self.preview(tool, &args)?,
                (_, Some(_)) => self.apply(&args),
                _ => return Err(RuntimeError::plain(format!("the test's Set has no {name}"))),
            },
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

/// A stereo 16-bit file of noise, loud enough to be heard.
fn noise(path: &Path, seconds: usize) {
    let frames = 48000 * seconds;
    let mut bytes = Vec::with_capacity(44 + frames * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&((frames * 4 + 36) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&48000u32.to_le_bytes());
    bytes.extend_from_slice(&192000u32.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&((frames * 4) as u32).to_le_bytes());
    let mut seed = 7u32;
    for _ in 0..frames {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let value = (seed % 16001) as i16 - 8000;
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A Live with this Set, and Kumi's rendering over it. A take runs three seconds, and the reference four.
struct Live {
    set: Rc<Set>,
    rendering: Rc<Rendering>,
    observer: Rc<Observer>,
    reference: PathBuf,
    _folder: tempfile::TempDir,
}
impl Live {
    async fn new(tracks: &[(&str, &[&str])]) -> Self {
        let folder = tempfile::tempdir().unwrap();
        let (take, reference) = (folder.path().join("take.wav"), folder.path().join("reference.wav"));
        noise(&take, 3);
        noise(&reference, 4);
        let set = Set::new(&take, tracks);
        let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
        let bridge = set.clone();
        options.connect = Some(Rc::new(move |_| {
            let bridge: Rc<dyn McpEndpoint> = bridge.clone();
            async move { Ok(bridge) }.boxed_local()
        }));
        options.now = Some(Rc::new(|| chrono::DateTime::from_timestamp_millis(0).unwrap()));
        // (each test's renders in a folder of its own: the generation's first 8 characters name it)
        options.generation = Some(uuid::Uuid::new_v4().to_string());
        options.change_timeout_ms = Some(2000);
        options.fast = Some(false);
        options.ears = Some(EarsSetup::Disabled);
        options.low_disk = Some(Rc::new(|_, _, _| async { None }.boxed_local()));
        options.restore_file = Some(folder.path().join("restore.json").to_string_lossy().into_owned());
        let options = Rc::new(options);
        let connection = LiveConnection::new(options.connection_options());
        connection.start(Signal::new()).await.unwrap();
        connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
        connection.available.set(true);
        connection.lost.set(false);
        connection.epoch.set(Some(7.));
        *connection.set.borrow_mut() = Some("fixture".into());
        let remember = Remember::new(connection.clone(), None, None);
        *remember.current.borrow_mut() = Some(Rc::new(CurrentProject {
            identity: "fixture".into(),
            path: None,
            name: "Fixture Set".into(),
            project: None,
            unsaved: false,
        }));
        let history = Rc::new(History::new(connection.clone(), remember.clone(), options.change_timeout_ms, options.on_change.clone()));
        let observer = Rc::new(Observer::new(connection.clone(), remember));
        observer.tempo.set(Some(120.));
        let mutations = Rc::new(Mutations::new(Rc::new(Parameters::new(history.clone(), options.fast)), observer.clone(), options.clone()));
        let step = mutations.clone();
        let files = mutations.clone();
        let rendering = Rendering::new(
            history.clone(),
            observer.clone(),
            &options,
            Rc::new(move |tool, args, signal| {
                let step = step.clone();
                async move { step.step(&tool, args, signal).await }.boxed_local()
            }),
            Rc::new(move |file, signal| {
                let files = files.clone();
                async move { files.clip_file(&file, signal).await }.boxed_local()
            }),
        );
        Self { set, rendering, observer, reference, _folder: folder }
    }
    /// A goal on these tracks, over two beats from the Set's start (one second at 120).
    async fn goal(&self, candidates: &[&str]) -> Rc<dyn GoalRig> {
        self.goal_over(candidates, 2.).await
    }
    async fn goal_over(&self, candidates: &[&str], beats: f64) -> Rc<dyn GoalRig> {
        let candidates: Vec<Value> = candidates.iter().map(|name| json!({"track":name,"label":name})).collect();
        let request = json!({"candidates":candidates,"fromBeat":0,"beats":beats,"reference":self.reference,"focus":"sound"});
        self.rendering.open_goal(&serde_json::from_value(request).unwrap(), Signal::new()).await.unwrap().unwrap()
    }
}
fn slot_named(goal: &Rc<dyn GoalRig>, name: &str) -> GoalSlotInfo {
    goal.slots().into_iter().find(|slot| slot.name == name).unwrap()
}
/// The slot's knob on this device, by the name it had as the goal opened, or by the knobs' word for it where two share a name.
fn knob_on(slot: &GoalSlotInfo, device: &str, name: &str) -> Knob {
    let this = |knob: &&Knob| knob.device == device || knob.device.ends_with(&format!(":{device}"));
    slot.knobs.iter().filter(this).find(|knob| knob.name == name).unwrap().clone()
}
/// A trial of these knobs on a slot.
fn trial(slot: &GoalSlotInfo, set: &[(&str, &str, f64)]) -> GenerationTrial {
    GenerationTrial {
        slot: slot.name.clone(),
        knobs: set.iter().map(|(device, name, _)| knob_on(slot, device, name)).collect(),
        values: set.iter().map(|(_, _, value)| *value).collect(),
        fresh: None,
    }
}
fn on_a_paused_clock(work: impl std::future::Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
    runtime.block_on(tokio::task::LocalSet::new().run_until(work));
}

/// (#399) A device put in above moves the refs of everything below it, and neither the lease nor the epoch changes: a running
/// search still finds each knob, rather than sending its values to whatever is at the old places.
#[test]
fn a_trial_sets_a_knob_where_its_device_is_now_once_one_was_put_in_above() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6), ("EQ", "Drive", 0.3)])], Signal::new(), None).await.unwrap();
        live.set.put_device_in("Candidate", 0, "New");

        let generation =
            goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7), ("EQ", "Drive", 0.4)])], Signal::new(), None).await.unwrap();

        assert!(generation.frozen.is_empty(), "{:?}", generation.frozen);
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.7);
        assert_eq!(live.set.knob("Candidate", "EQ", "Drive"), 0.4);
        assert_eq!(live.set.knob("Candidate", "New", "Tone"), 0.5, "the new device isn't touched");
        assert_eq!(live.set.knob("Candidate", "New", "Drive"), 0.2);
        goal.close().await.unwrap();
    });
}

/// (#399) A device deleted mid-search: its knobs aren't sent (at its old place there's now another device), the trial's other
/// knobs still are, and the search is told to leave those knobs be.
#[test]
fn a_trial_leaves_a_knob_whose_device_went_and_freezes_it() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6), ("EQ", "Drive", 0.3)])], Signal::new(), None).await.unwrap();
        live.set.delete_device("Candidate", "EQ");

        let generation =
            goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7), ("EQ", "Drive", 0.4)])], Signal::new(), None).await.unwrap();

        let frozen: Vec<&String> = generation.frozen["Candidate"].iter().collect();
        assert_eq!(frozen, [&knob_key(&knob_on(&slot, "EQ", "Drive"))]);
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.7);
        assert_eq!(live.set.knob("Candidate", "Limiter", "Ceiling"), 0.97, "the Limiter is at the place the EQ had");
        goal.close().await.unwrap();
    });
}

/// (#399) A track put in above moves every track's refs: a trial still goes to its own track.
#[test]
fn a_trial_goes_to_its_own_track_once_another_was_put_in_above() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Other", &["Synth"]), ("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        live.set.put_track_in(0, "New", &["Synth"]);

        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7)])], Signal::new(), None).await.unwrap();

        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.7);
        assert_eq!(live.set.knob("Other", "Synth", "Tone"), 0.5, "the track now at the candidate's old place");
        goal.close().await.unwrap();
    });
}

/// (#399) One candidate's track deleted mid-search: its knobs freeze and the search goes on with the others, as in REAPER and
/// Renoise, and nothing goes to the track now at its place.
#[test]
fn a_trial_for_a_track_thats_gone_freezes_its_knobs_and_the_others_go_on() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Gone", &["Synth"]), ("Stays", &["Synth"])]).await;
        let goal = live.goal(&["Gone", "Stays"]).await;
        let (gone, stays) = (slot_named(&goal, "Gone"), slot_named(&goal, "Stays"));
        let trials = |value: f64| [trial(&gone, &[("Synth", "Tone", value)]), trial(&stays, &[("Synth", "Tone", value)])];
        goal.generation(&trials(0.6), Signal::new(), None).await.unwrap();
        live.set.delete_track("Gone");

        let generation = goal.generation(&trials(0.7), Signal::new(), None).await.unwrap();

        let frozen: Vec<&String> = generation.frozen["Gone"].iter().collect();
        assert_eq!(frozen, [&knob_key(&knob_on(&gone, "Synth", "Tone"))]);
        assert!(!generation.frozen.contains_key("Stays"));
        assert_eq!(live.set.knob("Stays", "Synth", "Tone"), 0.7, "the other candidate moved, once");
        goal.close().await.unwrap();
    });
}

/// (#399) A device deleted from a chain with another of its name: the knob is lost with it, not moved onto the other.
#[test]
fn a_knob_whose_device_went_doesnt_land_on_another_of_its_name() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["EQ", "Synth", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        let first = knob_on(&slot, "0:EQ", "Tone");
        goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        live.set.delete_device("Candidate", "EQ");

        let generation = goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.7)])], Signal::new(), None).await.unwrap();

        assert!(generation.frozen["Candidate"].contains(&knob_key(&first)));
        assert_eq!(live.set.knob("Candidate", "EQ", "Tone"), 0.5, "the EQ that's left was never in this knob's trial");
        goal.close().await.unwrap();
    });
}

/// (#399) Two devices of one name with a device put in above: each knob stays on its own.
#[test]
fn each_knob_stays_on_its_own_device_of_two_of_a_name_once_one_was_put_in_above() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["EQ", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.6), ("1:EQ", "Tone", 0.4)])], Signal::new(), None).await.unwrap();
        live.set.put_device_in("Candidate", 0, "New");

        let generation =
            goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.7), ("1:EQ", "Tone", 0.3)])], Signal::new(), None).await.unwrap();

        assert!(generation.frozen.is_empty(), "{:?}", generation.frozen);
        assert_eq!(live.set.knob_at("Candidate", 1, "Tone"), 0.7, "the first EQ");
        assert_eq!(live.set.knob_at("Candidate", 2, "Tone"), 0.3, "the second EQ");
        assert_eq!(live.set.knob_at("Candidate", 0, "Tone"), 0.5, "the new device");
        goal.close().await.unwrap();
    });
}

/// A device the producer renamed mid-search is the same device: its knobs still move.
#[test]
fn a_device_renamed_mid_search_keeps_its_knobs() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        live.set.rename_device("Candidate", 0, "My Synth");

        let generation = goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7)])], Signal::new(), None).await.unwrap();

        assert!(generation.frozen.is_empty(), "{:?}", generation.frozen);
        assert_eq!(live.set.knob("Candidate", "My Synth", "Tone"), 0.7);
        goal.close().await.unwrap();
    });
}

/// (#399) A second track of a candidate's name put in above: it's told, rather than tuning whichever comes first. One put in
/// below leaves the candidate where it is.
#[test]
fn a_second_track_of_a_candidates_name_is_told_unless_its_own_place_still_holds() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Other", &["Synth"]), ("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();

        live.set.put_track_in(2, "Candidate", &["Synth"]);
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7)])], Signal::new(), None).await.unwrap();
        assert_eq!(live.set.knob_on_track(1, "Tone"), 0.7, "one below: the candidate is where it was");
        assert_eq!(live.set.knob_on_track(2, "Tone"), 0.5);

        live.set.put_track_in(0, "Candidate", &["Synth"]);
        let said = goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.8)])], Signal::new(), None).await.err().unwrap().to_string();
        assert!(said.contains("Two tracks are named “Candidate”"), "{said}");
        goal.close().await.unwrap();
    });
}

/// A chain that stayed costs a trial two reads, not its knobs again.
#[test]
fn a_chain_that_stayed_isnt_read_again_for_the_next_trial() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        let knobs_read = live.set.reads("parameter");

        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7)])], Signal::new(), None).await.unwrap();

        assert_eq!(live.set.reads("parameter"), knobs_read);
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.7);
        goal.close().await.unwrap();
    });
}

/// (#789) A look loses the tempo just as a listen begins, so its rig opens with none either: refused in words before Live plays.
#[test]
fn a_listen_is_refused_in_words_when_neither_a_look_nor_its_rig_knows_the_tempo() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let observer = live.observer.clone();
        *live.set.on_call.borrow_mut() = Some(Box::new(move |_| observer.tempo.set(None)));

        let request = json!({"tracks":["Candidate"],"fromBeat":0,"beats":2});
        let said = live.rendering.hear_in_set(&serde_json::from_value(request).unwrap(), Signal::new()).await;

        let said = format!("{said:?}");
        assert!(said.contains("Kumi doesn't know the Set's tempo yet; try again."), "{said}");
        assert_eq!(live.set.asked_for("live_recording_preview"), 0, "Live didn't play");
    });
}

/// (#399) The Set's tempo moves while a long reference is heard, and a look then loses it: the goal hears a render for as long
/// as its rig recorded it, one tempo for both.
#[test]
fn a_goal_hears_at_the_tempo_its_rig_opened_with() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        // (it runs when the opening goal first waits, which is on hearing the reference)
        let observer = live.observer.clone();
        tokio::task::spawn_local(async move { observer.tempo.set(Some(60.)) });
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        live.observer.tempo.set(None);

        let generation = goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();

        // Two beats at 60 are two seconds, where at the 120 the goal began with they'd be one.
        let gaps = generation.gaps["Candidate"].join(" · ");
        let heard: f64 = gaps.split("too short (").nth(1).and_then(|rest| rest.split(" ms").next()).unwrap().parse().unwrap();
        assert!(heard > 1500., "{gaps}");
        goal.close().await.unwrap();
    });
}

/// (#789) A pass the rig knows the tempo of is still checked for room, though a look lost it: refused before Live plays.
#[test]
fn a_pass_with_no_room_is_refused_at_the_tempo_its_rig_opened_with() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        // (more takes than any disk has room for)
        let goal = live.goal_over(&["Candidate"], 1e12).await;
        let slot = slot_named(&goal, "Candidate");
        live.observer.tempo.set(None);

        let said = goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.err().unwrap().to_string();

        assert!(said.contains("is free on the disk"), "{said}");
        assert_eq!(live.set.asked_for("live_recording_preview"), 0, "Live didn't play");
        goal.close().await.unwrap();
    });
}
