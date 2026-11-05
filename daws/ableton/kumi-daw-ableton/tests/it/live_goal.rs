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

#[path = "live_goal_listen.rs"]
mod bounded_listening;

/// The tools a goal can reach, as the oracle's bridge lists them.
const TOOLS: [&str; 41] = [
    "live_run_python",
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
    "live_device_delete_preview",
    "live_device_delete_apply",
    "live_browser_inspect",
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
    "live_selection_preview",
    "live_selection_apply",
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
    /// A device with the two knobs every test's device has; a Limiter has its own.
    fn new(name: &str) -> Self {
        if name == "Limiter" {
            return Self::limiter();
        }
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
    /// Who it is, wherever it goes: what an undo finds it by.
    id: u32,
    name: String,
    armed: bool,
    muted: bool,
    devices: Vec<Device>,
    /// Its clips, by what's in them.
    clips: Vec<String>,
    volume: f64,
}
impl Track {
    fn new(name: &str, devices: &[&str]) -> Self {
        Track {
            id: next_identity(),
            name: name.into(),
            armed: false,
            muted: false,
            devices: devices.iter().map(|name| Device::new(name)).collect(),
            clips: vec!["notes".into()],
            volume: 0.85,
        }
    }
}
/// What taking back a transaction does.
enum Undo {
    /// The tracks it made go, by who they are, whatever happened to them since (as Live's undo of a duplicated track does).
    Remove(Vec<u32>),
    /// The track goes back as it was.
    Restore(Track),
    /// The exact device a bounded capture placed, preserving other edits on its track.
    RemoveDevice(u32),
}

/// Live, as far as a goal can tell: refs are places, as Live's are, so a device or a track put in above moves them.
struct Set {
    tracks: RefCell<Vec<Track>>,
    /// The one file every take plays.
    take: PathBuf,
    playing: Cell<bool>,
    bounded: RefCell<Option<Rc<bounded_listening::State>>>,
    next: Cell<u32>,
    previews: RefCell<HashMap<String, (String, JsonObject)>>,
    undos: RefCell<HashMap<String, Undo>>,
    /// How many looks Live takes to have stopped playing, after it's asked: till then it shows no take.
    lag: Cell<u32>,
    stopping: Cell<u32>,
    /// Knobs Live won't set, by their device's name and theirs.
    refused: RefCell<Vec<(String, String)>>,
    /// Whether Live says who each device is (`objectIdentity`), as a bridge before it did wouldn't.
    identities: Cell<bool>,
    /// Live copies a track a moment behind what was just set on it, as a real Live was seen to: what the last set changed, by
    /// track, device, knob and its value before.
    last_set: RefCell<Vec<(usize, usize, usize, f64)>>,
    /// Every call, as `{name, args}`.
    asked: RefCell<Vec<Value>>,
    /// A test's look at each call as it comes, which can make it fail.
    on_call: RefCell<Option<Box<dyn Fn(&str, &JsonObject) -> Result<(), RuntimeError>>>>,
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
            bounded: RefCell::new(None),
            next: Cell::new(0),
            previews: RefCell::default(),
            undos: RefCell::default(),
            lag: Cell::new(0),
            stopping: Cell::new(0),
            refused: RefCell::default(),
            identities: Cell::new(true),
            last_set: RefCell::default(),
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
    /// The producer edits a track's clip.
    fn edit_clip(&self, track: &str) {
        self.tracks.borrow_mut().iter_mut().find(|found| found.name == track).unwrap().clips[0] = "edited notes".into();
    }
    /// The producer rides a track's fader.
    fn ride_fader(&self, track: &str) {
        self.tracks.borrow_mut().iter_mut().find(|found| found.name == track).unwrap().volume = 0.5;
    }
    /// What Kumi's read of a track whole comes to, here: its devices, clips and mixer as they are (`assets/best-print.py` in Live).
    fn python(&self, args: &JsonObject) -> Value {
        let code = text(&args["code"]);
        let line = code.lines().find_map(|line| line.strip_prefix("ARGS = json.loads(")).unwrap();
        let given: Value = serde_json::from_str(&serde_json::from_str::<String>(line.strip_suffix(')').unwrap()).unwrap()).unwrap();
        let tracks = self.tracks.borrow();
        let named: Vec<&Track> = tracks.iter().filter(|track| track.name == text(&given["track"])).collect();
        let result = match named.as_slice() {
            [] => Value::Null,
            [track] => json!({
                "devices": track.devices.iter().map(|device| json!([device.name, device.knobs])).collect::<Vec<_>>(),
                "clips": track.clips,
                "mixer": track.volume,
            }),
            _ => return json!({"ok":false,"result":null,"stdout":"","error":{"message":"Two tracks are named that"}}),
        };
        json!({"ok":true,"result":result,"stdout":"","error":null})
    }
    /// The names of a track's devices, in order.
    fn chain(&self, track: &str) -> Vec<String> {
        let tracks = self.tracks.borrow();
        tracks.iter().find(|found| found.name == track).unwrap().devices.iter().map(|device| device.name.clone()).collect()
    }
    fn has_track(&self, name: &str) -> bool {
        self.tracks.borrow().iter().any(|track| track.name == name)
    }
    fn muted(&self, track: &str) -> bool {
        self.tracks.borrow().iter().find(|found| found.name == track).unwrap().muted
    }
    /// The producer turns a knob.
    fn turn(&self, track: &str, device: &str, knob: &str, value: f64) {
        let mut tracks = self.tracks.borrow_mut();
        let track = tracks.iter_mut().find(|found| found.name == track).unwrap();
        let device = track.devices.iter_mut().find(|found| found.name == device).unwrap();
        device.knobs.iter_mut().find(|(name, _)| name == knob).unwrap().1 = value;
    }
    /// The producer renames a track.
    fn rename_track(&self, track: &str, name: &str) {
        self.tracks.borrow_mut().iter_mut().find(|found| found.name == track).unwrap().name = name.into();
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
        if let Some(state) = self.bounded.borrow().as_ref() {
            if let Some(items) = state.discover(self, args) {
                return json!({"epoch":state.epoch(),"kind":args["kind"],"items":items,"revision":"r1","truncated":false});
            }
        }
        let tracks = self.tracks.borrow();
        let kind = text(&args["kind"]);
        let parent = args.get("parent").map(text).unwrap_or_default();
        let at = places(parent);
        let items: Vec<Value> = match kind {
            "set" => {
                if self.stopping.get() > 0 {
                    self.stopping.set(self.stopping.get() - 1);
                    if self.stopping.get() == 0 {
                        self.playing.set(false);
                    }
                }
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
                    let mut row = json!({"ref":format!("7:device:{}:d{index}",at[0]),"parentRef":parent,"name":device.name,"className":device.class,"classDisplayName":device.class,"enabled":true,"objectIdentity":device.identity});
                    if !self.identities.get() {
                        row.as_object_mut().unwrap().remove("objectIdentity");
                    }
                    row
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
            "arrangement-clip" if tracks[at[0]].name.starts_with("Kumi · render") && !self.playing.get() => {
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
                let Some(device) = tracks.get(at[0]).and_then(|track| track.devices.get(at[1])) else {
                    return Err(RuntimeError::plain("No such parameter."));
                };
                let Some((knob, _)) = device.knobs.get(at[2]) else {
                    return Err(RuntimeError::plain("No such parameter."));
                };
                if self.refused.borrow().contains(&(device.name.clone(), knob.clone())) {
                    return Err(RuntimeError::plain("That parameter can't be changed now."));
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
        let proposed: Vec<Value> = match args.get("tracks").and_then(Value::as_array) {
            Some(tracks) => tracks.iter().map(|track| json!({"kind":"track","name":track["name"],"trackKind":track["kind"]})).collect(),
            None => vec![],
        };
        let epoch = self.bounded.borrow().as_ref().map_or(7., |state| state.prepared(&transaction));
        Ok(
            json!({"transactionId":transaction,"epoch":epoch,"confirmation":"apply","prior":{"tracks":prior,"scenes":[]},"proposed":proposed}),
        )
    }
    fn apply(&self, args: &JsonObject) -> Value {
        let transaction = text(&args["transactionId"]).to_owned();
        if self.bounded.borrow().as_ref().is_some_and(|state| !state.transaction_matches(&transaction)) {
            return json!({"transactionId":transaction,"state":"refused","reason":"Set epoch changed"});
        }
        let (tool, asked) = self.previews.borrow_mut().remove(&transaction).unwrap();
        let mut answer = json!({"transactionId":transaction,"state":"applied"});
        if let Some(state) = self.bounded.borrow().as_ref() {
            if let Some(answer) = state.apply(self, &tool, &asked, &transaction) {
                return answer;
            }
        }
        let mut tracks = self.tracks.borrow_mut();
        match tool.as_str() {
            "live_session_structure" => {
                let mut created = vec![];
                let mut made = vec![];
                for track in asked["tracks"].as_array().unwrap() {
                    let name = text(&track["name"]);
                    let at = track["index"].as_u64().map_or(tracks.len(), |at| at as usize);
                    let new = Track::new(name, &[]);
                    made.push(new.id);
                    tracks.insert(at, new);
                    created.push(json!({"kind":"track","ref":format!("7:track:{at}"),"name":name}));
                }
                self.undos.borrow_mut().insert(transaction, Undo::Remove(made));
                answer["created"] = json!(created);
            }
            "live_track_structure" => {
                let at = places(text(&asked["ref"]))[0];
                let mut copy = tracks[at].clone();
                for (track, device, knob, before) in self.last_set.borrow().iter() {
                    if *track == at {
                        copy.devices[*device].knobs[*knob].1 = *before;
                    }
                }
                copy.id = next_identity();
                copy.name = format!("{} copy", copy.name);
                for device in &mut copy.devices {
                    device.identity = next_identity();
                }
                self.undos.borrow_mut().insert(transaction, Undo::Remove(vec![copy.id]));
                tracks.insert(at + 1, copy);
            }
            "live_object_rename" => {
                let at = places(text(&asked["ref"]))[0];
                self.undos.borrow_mut().insert(transaction, Undo::Restore(tracks[at].clone()));
                tracks[at].name = text(&asked["name"]).to_owned();
            }
            "live_mixer" => {
                let at = places(text(&asked["trackRef"]))[0];
                self.undos.borrow_mut().insert(transaction, Undo::Restore(tracks[at].clone()));
                if let Some(mute) = asked.get("mute").and_then(Value::as_bool) {
                    tracks[at].muted = mute;
                }
            }
            "live_device_delete" => {
                let at = places(text(&asked["ref"]));
                tracks[at[0]].devices.remove(at[1]);
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
                let mut last_set = self.last_set.borrow_mut();
                last_set.clear();
                for value in asked["values"].as_array().unwrap() {
                    let at = places(text(&value["parameterRef"]));
                    let knob = &mut tracks[at[0]].devices[at[1]].knobs[at[2]];
                    last_set.push((at[0], at[1], at[2], knob.1));
                    knob.1 = value["value"].as_f64().unwrap();
                }
            }
            "live_recording" => self.playing.set(asked.get("action").map(text) == Some("start")),
            "live_transport_action" => {
                if asked.get("action").map(text) == Some("stop") {
                    if self.playing.get() && self.lag.get() > 0 {
                        self.stopping.set(self.lag.get());
                    } else {
                        self.playing.set(false);
                    }
                }
            }
            _ => {}
        }
        answer
    }
    fn undo(&self, args: &JsonObject) -> Value {
        let transaction = text(&args["transactionId"]);
        if self.bounded.borrow().as_ref().is_some_and(|state| !state.transaction_matches(transaction)) {
            return json!({"transactionId":transaction,"state":"kept","reason":"Set epoch changed"});
        }
        let mut tracks = self.tracks.borrow_mut();
        match self.undos.borrow_mut().remove(transaction) {
            Some(Undo::Remove(made)) => tracks.retain(|track| !made.contains(&track.id)),
            Some(Undo::Restore(was)) => {
                if let Some(track) = tracks.iter_mut().find(|track| track.id == was.id) {
                    *track = was;
                }
            }
            Some(Undo::RemoveDevice(identity)) => {
                for track in tracks.iter_mut() {
                    track.devices.retain(|device| device.identity != identity);
                }
            }
            None => {}
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
            on_call(name, &args)?;
        }
        self.asked.borrow_mut().push(json!({"name":name,"args":args}));
        tokio::task::yield_now().await;
        if name == "live_browser_inspect" {
            let mut missing = reply(json!({"error":"item_not_found"}));
            missing.is_error = Some(true);
            return Ok(missing);
        }
        Ok(reply(match name {
            "live_discover" => self.discover(&args),
            "live_song_state" if self.bounded.borrow().is_some() => self.bounded.borrow().as_ref().unwrap().song_state(),
            "live_undo" => self.undo(&args),
            "live_run_python" => self.python(&args),
            "live_status" => {
                json!({"connected":true,"adapter":"remote-script","provenance":"fake-live","epoch":self.bounded.borrow().as_ref().map_or(7., |state| state.epoch())})
            }
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
    history: Rc<History>,
    mutations: Rc<Mutations>,
    /// What Kumi told the app, in passing.
    told: Rc<RefCell<Vec<String>>>,
    reference: PathBuf,
    _folder: tempfile::TempDir,
}
impl Live {
    async fn new(tracks: &[(&str, &[&str])]) -> Self {
        Self::configured(tracks, |_, _, _| {}).await
    }
    async fn configured(tracks: &[(&str, &[&str])], configure: impl FnOnce(&Rc<Set>, &mut AbletonOptions, &Path)) -> Self {
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
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        let out = told.clone();
        options.on_action = Some(Rc::new(move |event| out.borrow_mut().push(event.title)));
        configure(&set, &mut options, folder.path());
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
        Self { set, rendering, observer, history, mutations, told, reference, _folder: folder }
    }
    /// A goal on these tracks, over two beats from the Set's start (one second at 120).
    async fn goal(&self, candidates: &[&str]) -> Rc<dyn GoalRig> {
        self.goal_over(candidates, 2.).await
    }
    async fn goal_over(&self, candidates: &[&str], beats: f64) -> Rc<dyn GoalRig> {
        self.open(candidates, beats).await.unwrap().unwrap()
    }
    async fn open(&self, candidates: &[&str], beats: f64) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        let candidates: Vec<Value> = candidates.iter().map(|name| json!({"track":name,"label":name})).collect();
        let request = json!({"candidates":candidates,"fromBeat":0,"beats":beats,"reference":self.reference,"focus":"sound"});
        self.rendering.open_goal(&serde_json::from_value(request).unwrap(), Signal::new()).await
    }
    /// A change the model asked for.
    async fn change(&self, tool: &str, args: Value) {
        self.mutations.step(tool, args.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    }
    /// Reads of this kind fail, the next this many times.
    fn fail_reads(&self, kind: &'static str, times: u32) {
        let left = Cell::new(times);
        *self.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_discover" && args.get("kind").map(text) == Some(kind) && left.get() > 0 {
                left.set(left.get() - 1);
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));
    }
    fn was_told(&self, text: &str) -> bool {
        self.told.borrow().iter().any(|line| line.contains(text))
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
/// The best kept at these values: its name.
async fn keep(goal: &Rc<dyn GoalRig>, slot: &GoalSlotInfo, set: &[(&str, &str, f64)]) -> String {
    let at = trial(slot, set);
    goal.keep_best(&slot.name, &at.knobs, &at.values, Signal::new()).await.unwrap()
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

/// (#399) A leap opens a new rig, and a search's knobs keep the words they began with: with a device put in above during it, each
/// still lands on the device it was read from.
#[test]
fn a_leap_keeps_each_knob_on_the_device_it_was_read_from() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["EQ", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.6), ("1:EQ", "Tone", 0.4)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();
        live.set.put_device_in("Candidate", 0, "Saturator");

        let goal = live.goal(&["Candidate"]).await;
        let generation =
            goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.7), ("1:EQ", "Tone", 0.3)])], Signal::new(), None).await.unwrap();

        assert!(generation.frozen.is_empty(), "{:?}", generation.frozen);
        assert_eq!(live.set.knob_at("Candidate", 1, "Tone"), 0.7, "the first EQ");
        assert_eq!(live.set.knob_at("Candidate", 2, "Tone"), 0.3, "the second EQ");
        assert_eq!(live.set.knob_at("Candidate", 0, "Tone"), 0.5, "the Saturator");
        goal.close().await.unwrap();
    });
}

/// (#399) A later goal on a chain that moved reads its own knobs: nothing of the last goal's words is left to send them elsewhere.
#[test]
fn a_later_goal_on_a_moved_chain_goes_by_the_knobs_it_read() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["EQ", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("0:EQ", "Tone", 0.6), ("1:EQ", "Tone", 0.4)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();
        drop(goal);
        live.set.put_device_in("Candidate", 0, "Saturator");

        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        let generation =
            goal.generation(&[trial(&slot, &[("1:EQ", "Tone", 0.7), ("2:EQ", "Tone", 0.3)])], Signal::new(), None).await.unwrap();

        assert!(generation.frozen.is_empty(), "{:?}", generation.frozen);
        assert_eq!(live.set.knob_at("Candidate", 1, "Tone"), 0.7, "the first EQ");
        assert_eq!(live.set.knob_at("Candidate", 2, "Tone"), 0.3, "the second EQ");
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
        *live.set.on_call.borrow_mut() = Some(Box::new(move |_, _| {
            observer.tempo.set(None);
            Ok(())
        }));

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

// ---- What a goal leaves behind

const BEST: &str = "Kumi · Goal best";

/// (#399) A Limiter the producer put last is theirs: neither a settled winner nor the best changes its gain.
#[test]
fn a_producers_own_limiter_keeps_its_gain() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth", "Limiter"])]).await;
        live.set.turn("Candidate", "Limiter", "Gain", 0.6);
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        assert_eq!(live.set.asked_for("live_browser_load_preview"), 0, "it has its limiter");
        goal.close().await.unwrap();

        let at = trial(&slot, &[("Synth", "Tone", 0.7)]);
        assert_eq!(goal.settle("Candidate", &at.knobs, &at.values, Signal::new()).await.unwrap(), None);
        assert_eq!(live.set.knob("Candidate", "Limiter", "Gain"), 0.6);
        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;
        assert_eq!(live.set.knob(&best, "Limiter", "Gain"), 0.6);
    });
}

/// (#399) The limiter Kumi added goes back to unity on the winner and the best, whichever rig added it: a leap's new rig finds
/// it there, and it's still Kumi's.
#[test]
fn kumis_own_limiter_goes_back_to_unity_even_after_a_leap_opened_a_new_rig() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        assert_eq!(live.set.knob("Candidate", "Limiter", "Gain"), 0.25, "turned down for the search");
        goal.close().await.unwrap();
        let goal = live.goal(&["Candidate"]).await;
        assert_eq!(live.set.asked_for("live_browser_load_preview"), 1, "added once");
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;
        assert_eq!(live.set.knob(&best, "Limiter", "Gain"), 0.5);
        let at = trial(&slot, &[("Synth", "Tone", 0.7)]);
        assert_eq!(goal.settle("Candidate", &at.knobs, &at.values, Signal::new()).await.unwrap(), None);
        assert_eq!(live.set.knob("Candidate", "Limiter", "Gain"), 0.5);
    });
}

/// (#399) Tidying takes back the track the model added. A duplicate that merely names a candidate isn't what made it: the
/// candidate stays, muted, and the duplicate stays too.
#[test]
fn tidying_takes_back_only_the_track_the_model_added() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Keep", &["Synth"]), ("Copied", &["Synth"])]).await;
        live.change("add_tracks_and_scenes", json!({"tracks":[{"name":"Made","kind":"midi"}],"scenes":[]})).await;
        let goal = live.goal(&["Keep", "Made", "Copied"]).await;
        live.change("change_structure", json!({"action":"duplicate-track","ref":"7:track:1"})).await;
        let titles: Vec<String> = live.history.entries.borrow().values().map(|entry| entry.borrow().record.title.clone()).collect();
        assert!(titles.iter().any(|title| title == "Added MIDI track “Made”"), "{titles:?}");
        assert!(titles.iter().any(|title| title == "Duplicated track “Copied”"), "{titles:?}");
        goal.close().await.unwrap();

        let said = goal.tidy(&["Keep".to_owned()], Signal::new()).await.unwrap();

        assert!(!live.set.has_track("Made"), "the model's own track went");
        assert!(live.set.has_track("Copied copy"), "the duplicate wasn't taken back");
        assert!(live.set.muted("Copied"));
        assert_eq!(said, ["“Copied” stays, muted."]);
        assert!(live.set.muted("Keep"), "the top one, for an A/B");
    });
}

/// (#399) Live copies a track a moment behind what was just set on it: a real Live's copy held the last trial's value, though the
/// track read as the winner's. The best holds what it was kept at.
#[test]
fn a_best_holds_what_it_was_kept_at_though_live_copies_a_moment_behind() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.3)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.7)]).await;

        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.7);
        assert_eq!(live.set.knob(&best, "Synth", "Tone"), 0.7, "and not the trial's 0.3");
    });
}

/// (#399) A paused goal's candidates weren't tidied and still play: its best waits muted beside them, and that's said.
#[test]
fn a_paused_goals_best_waits_muted() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

        assert_eq!(best, BEST);
        assert!(live.set.muted(BEST));
        assert!(!live.set.muted("Candidate"));
        assert!(live.was_told("“Kumi · Goal best” waits muted beside “Candidate”, which plays as it was: unmute it to hear the best"));
    });
}

/// (#399) A paused goal's best that can't be muted plays beside its candidate: that's said, with what to do.
#[test]
fn a_paused_goals_best_that_cant_be_muted_is_said() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();
        *live.set.on_call.borrow_mut() = Some(Box::new(|name, _| {
            if name == "live_mixer_preview" {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

        assert_eq!(best, BEST, "it's kept all the same");
        assert!(live.was_told("“Kumi · Goal best” plays beside “Candidate”: mute one of them to hear the other alone"));
        assert!(!live.was_told("waits muted"));
    });
}

/// (#399) A finished goal muted its top candidates for an A/B: its best plays, though it's a copy of a muted track.
#[test]
fn a_finished_goals_best_plays() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();
        goal.tidy(&["Candidate".to_owned()], Signal::new()).await.unwrap();

        keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

        assert!(live.set.muted("Candidate"));
        assert!(!live.set.muted(BEST));
        assert!(!live.was_told("waits muted"));
    });
}

/// (#399) A best as Kumi kept it is replaced by the next, though Kumi set its limiter's input itself after reading it.
#[test]
fn a_best_as_kumi_kept_it_is_replaced() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();
        keep(&goal, &slot, &[("Synth", "Tone", 0.7)]).await;

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

        assert_eq!(best, BEST);
        assert_eq!(live.set.knob(BEST, "Synth", "Tone"), 0.8);
        assert_eq!(live.set.tracks.borrow().iter().filter(|track| track.name.starts_with(BEST)).count(), 1);
        assert_eq!(goal.kept_only(), None);
    });
}

/// (#399) A best the producer worked on is theirs: it stays, the new one goes beside it under a name of its own, and the notice
/// names both and says why.
#[test]
fn a_best_the_producer_worked_on_stays() {
    type Work = fn(&Set);
    let works: [(&str, Work, &str); 4] = [
        ("turned a knob", |set| set.turn(BEST, "Synth", "Tone", 0.95), "its devices changed"),
        ("put a device in", |set| set.put_device_in(BEST, 0, "New"), "its devices changed"),
        ("edited its notes", |set| set.edit_clip(BEST), "its clips changed"),
        ("rode its fader", |set| set.ride_fader(BEST), "its mixer changed"),
    ];
    for (what, work, why) in works {
        on_a_paused_clock(async {
            let live = Live::new(&[("Candidate", &["Synth"])]).await;
            let goal = live.goal(&["Candidate"]).await;
            let slot = slot_named(&goal, "Candidate");
            goal.close().await.unwrap();
            keep(&goal, &slot, &[("Synth", "Tone", 0.7)]).await;
            assert_eq!(goal.kept_only(), None, "{what}: one best, nothing to explain");
            work(&live.set);
            let theirs = live.set.chain(BEST);

            let new = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

            assert_ne!(new, BEST, "{what}");
            assert!(new.starts_with(BEST), "{what}: {new}");
            assert_eq!(live.set.knob(&new, "Synth", "Tone"), 0.8, "{what}");
            assert_eq!(live.set.chain(BEST), theirs, "{what}");
            let said = goal.kept_only().unwrap();
            assert_eq!(
                said,
                format!("The last “{BEST}” stays where it is ({why} since Kumi kept it), so the new best is beside it on “{new}”."),
                "{what}"
            );

            // What's Kumi's now is the new one: it's the one the next keep replaces, and the one they worked on still stays.
            let next = keep(&goal, &slot, &[("Synth", "Tone", 0.85)]).await;
            assert!(!live.set.has_track(&new) || next == new, "{what}");
            assert!(live.set.has_track(BEST), "{what}");
            assert_eq!(goal.kept_only(), None, "{what}: the last one was replaced this time");
        });
    }
}

/// (#399) A best Kumi can't show is unchanged stays: a failed proof can only add a track, never lose one.
#[test]
fn a_best_kumi_cant_read_stays() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();
        keep(&goal, &slot, &[("Synth", "Tone", 0.7)]).await;
        *live.set.on_call.borrow_mut() = Some(Box::new(|name, _| {
            if name == "live_run_python" {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));

        let new = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

        assert_ne!(new, BEST);
        assert_eq!(live.set.knob(BEST, "Synth", "Tone"), 0.7);
        assert!(goal.kept_only().unwrap().contains("Kumi couldn't read it"), "{:?}", goal.kept_only());

        // As it was kept it couldn't be read either: the next can't show it's unchanged.
        *live.set.on_call.borrow_mut() = None;
        let next = keep(&goal, &slot, &[("Synth", "Tone", 0.9)]).await;
        assert!(live.set.has_track(&new), "{next}");
        assert!(goal.kept_only().unwrap().contains("Kumi couldn't read it when it kept it"), "{:?}", goal.kept_only());
    });
}

/// (#399) A best no longer there under its name was renamed, and so is theirs, or deleted: it's left, without a word.
#[test]
fn a_best_renamed_or_deleted_is_left_without_a_word() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();
        keep(&goal, &slot, &[("Synth", "Tone", 0.7)]).await;
        live.set.rename_track(BEST, "Mine");

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.8)]).await;

        assert_eq!(best, BEST, "the plain name is free");
        assert!(live.set.has_track("Mine"));
        assert_eq!(live.set.knob("Mine", "Synth", "Tone"), 0.7);
        assert_eq!(goal.kept_only(), None);
    });
}

/// (#399) A goal that can't open takes off the safety limiters it put on its candidates: the producer's chain is as it was.
#[test]
fn a_limiter_comes_off_the_candidates_of_a_goal_that_never_opened() {
    on_a_paused_clock(async {
        let live = Live::new(&[("First", &["Synth"]), ("Second", &["Synth"])]).await;
        let failed = Cell::new(false);
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            // (Second's chain can't be read, once)
            if name == "live_discover"
                && args.get("kind").map(text) == Some("device")
                && args.get("parent").map(text) == Some("7:track:1")
                && !failed.replace(true)
            {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));

        assert!(live.open(&["First", "Second"], 2.).await.is_err());

        assert_eq!(live.set.chain("First"), ["Synth"]);
        assert_eq!(live.set.chain("Second"), ["Synth"]);
    });
}

/// (#399) The reference goes away once the candidates have joined, before the stretch it screens on is heard: the goal never
/// opens, and its limiter comes off. (A part over six seconds is screened.)
#[test]
fn a_limiter_comes_off_when_the_reference_goes_before_its_screen_is_heard() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let reference = live.reference.clone();
        let turned_down = Cell::new(false);
        let read = Cell::new(0);
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            turned_down.set(turned_down.get() || name == "live_device_parameter_apply");
            // (the last read of the candidate joining: its Synth's knobs, then its Limiter's)
            if turned_down.get() && name == "live_discover" && args.get("kind").map(text) == Some("parameter") {
                read.set(read.get() + 1);
                if read.get() == 2 {
                    std::fs::remove_file(&reference).unwrap();
                }
            }
            Ok(())
        }));

        let said = live.open(&["Candidate"], 14.).await.err().unwrap().to_string();

        assert!(said.contains("There's no file there"), "{said}");
        assert_eq!(live.set.chain("Candidate"), ["Synth"]);
    });
}

/// (#399) A read failing just after the limiter went on a candidate joining mid-search: it comes off that one.
#[test]
fn a_limiter_comes_off_a_candidate_that_never_joined() {
    on_a_paused_clock(async {
        let live = Live::new(&[("First", &["Synth"]), ("Second", &["Synth"])]).await;
        let goal = live.goal(&["First"]).await;
        let loaded = Cell::new(false);
        let failed = Cell::new(false);
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
            if loaded.get() && name == "live_discover" && !failed.replace(true) {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            loaded.set(loaded.get() || name == "live_browser_load_apply");
            Ok(())
        }));

        // (as a turn's discovery gives it: tracks are numbered from one as they're first seen)
        let candidate = serde_json::from_value(json!({"track":"track:2","label":"Second"})).unwrap();
        let said = goal.add(&candidate, Signal::new()).await.unwrap().err().unwrap();

        assert!(said.contains("Live didn't answer"), "{said}");
        assert_eq!(live.set.chain("Second"), ["Synth"]);
        assert_eq!(live.set.chain("First"), ["Synth", "Limiter"], "the search's own candidate keeps its");
        goal.close().await.unwrap();
    });
}

/// (#399) A leap's new rig that can't open takes off only what it added: the search goes on with the rig before it.
#[test]
fn a_leap_that_cant_open_leaves_an_earlier_rigs_limiter() {
    on_a_paused_clock(async {
        let live = Live::new(&[("First", &["Synth"]), ("Second", &["Synth"])]).await;
        let goal = live.goal(&["First"]).await;
        goal.close().await.unwrap();
        let loads = Cell::new(0);
        let failed = Cell::new(false);
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
            if name == "live_browser_load_apply" {
                loads.set(loads.get() + 1);
            } else if loads.get() == 1 && name == "live_discover" && !failed.replace(true) {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));

        assert!(live.open(&["First", "Second"], 2.).await.is_err());

        assert_eq!(live.set.chain("First"), ["Synth", "Limiter"]);
        assert_eq!(live.set.chain("Second"), ["Synth"]);
    });
}

/// (#399) A limiter that can't come off is said, with what to do.
#[test]
fn a_limiter_that_cant_come_off_is_said() {
    on_a_paused_clock(async {
        let live = Live::new(&[("First", &["Synth"]), ("Second", &["Synth"])]).await;
        let failed = Cell::new(false);
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_device_delete_preview" {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            if name == "live_discover"
                && args.get("kind").map(text) == Some("device")
                && args.get("parent").map(text) == Some("7:track:1")
                && !failed.replace(true)
            {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));

        assert!(live.open(&["First", "Second"], 2.).await.is_err());

        assert!(live.was_told("Kumi's safety Limiter stays last on “First”: delete it if you don't want it there."));
    });
}

/// (#399) Live's search moves the candidates' own tracks. When the goal ends each goes back as the goal found it, without Kumi's
/// limiter, and the best keeps the values it was kept at.
#[test]
fn a_goals_candidates_go_back_as_it_found_them() {
    on_a_paused_clock(async {
        let live = Live::new(&[("A", &["Synth"]), ("B", &["Synth"])]).await;
        let goal = live.goal(&["A", "B"]).await;
        let (a, b) = (slot_named(&goal, "A"), slot_named(&goal, "B"));
        goal.generation(
            &[trial(&a, &[("Synth", "Tone", 0.6), ("Synth", "Drive", 0.3)]), trial(&b, &[("Synth", "Tone", 0.7)])],
            Signal::new(),
            None,
        )
        .await
        .unwrap();
        goal.close().await.unwrap();
        let best = keep(&goal, &a, &[("Synth", "Tone", 0.65), ("Synth", "Drive", 0.35)]).await;

        let said = goal.put_back(Signal::new()).await;

        assert!(said.is_empty(), "{said:?}");
        for track in ["A", "B"] {
            assert_eq!(live.set.knob(track, "Synth", "Tone"), 0.5, "{track}");
            assert_eq!(live.set.knob(track, "Synth", "Drive"), 0.2, "{track}");
            assert_eq!(live.set.chain(track), ["Synth"], "{track}: its limiter came off");
        }
        assert_eq!(live.set.knob(&best, "Limiter", "Gain"), 0.5, "the best's stays, at unity");
        assert_eq!(live.set.knob(&best, "Synth", "Tone"), 0.65);
        assert_eq!(live.set.knob(&best, "Synth", "Drive"), 0.35);
    });
}

/// (#399) A knob the producer turned since the search last set it stays where they put it, and is named. The rest go back.
#[test]
fn a_knob_the_producer_turned_during_the_search_stays_and_is_said() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6), ("Synth", "Drive", 0.3)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();
        live.set.turn("Candidate", "Synth", "Tone", 0.9);

        let said = goal.put_back(Signal::new()).await;

        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.9);
        assert_eq!(live.set.knob("Candidate", "Synth", "Drive"), 0.2);
        assert_eq!(
            said,
            ["“Candidate” is back as it was, except that you turned Synth Tone on “Candidate” since the search began, so Kumi left it as you set it."]
        );
    });
}

/// (#399) A match's winner stays at its values; the other candidates go back.
#[test]
fn a_matchs_winner_stays_at_its_values_and_the_others_go_back() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Winner", &["Synth"]), ("Other", &["Synth"])]).await;
        let goal = live.goal(&["Winner", "Other"]).await;
        let (winner, other) = (slot_named(&goal, "Winner"), slot_named(&goal, "Other"));
        goal.generation(&[trial(&winner, &[("Synth", "Tone", 0.6)]), trial(&other, &[("Synth", "Tone", 0.7)])], Signal::new(), None)
            .await
            .unwrap();
        goal.close().await.unwrap();
        let at = trial(&winner, &[("Synth", "Tone", 0.8)]);
        assert_eq!(goal.settle("Winner", &at.knobs, &at.values, Signal::new()).await.unwrap(), None);

        let said = goal.put_back(Signal::new()).await;

        assert!(said.is_empty(), "{said:?}");
        assert_eq!(live.set.knob("Winner", "Synth", "Tone"), 0.8);
        assert_eq!(live.set.knob("Other", "Synth", "Tone"), 0.5);
    });
}

/// (#399) A leap opens a new rig, which reads the search's values as if they'd always been there: the candidate still goes back
/// to where the goal first found it, by the words the search's knobs began with though a device went in above. What the model
/// changed in the leap is its idea, and stays.
#[test]
fn candidates_go_back_to_where_the_goal_first_found_them_across_a_leap() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6), ("Synth", "Drive", 0.3)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();
        live.set.turn("Candidate", "Synth", "Drive", 0.35);
        live.set.put_device_in("Candidate", 0, "New");

        let goal = live.goal(&["Candidate"]).await;
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7)])], Signal::new(), None).await.unwrap();
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.7);
        goal.close().await.unwrap();

        let said = goal.put_back(Signal::new()).await;

        assert!(said.is_empty(), "{said:?}");
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.5);
        assert_eq!(live.set.knob("Candidate", "Synth", "Drive"), 0.35);
        assert_eq!(live.set.knob("Candidate", "New", "Tone"), 0.5, "the new device isn't touched");
        assert_eq!(live.set.chain("Candidate"), ["New", "Synth"]);
    });
}

/// (#399) A candidate tidied away or deleted has nothing to go back to, and that needs no word. Nothing goes to the track now at
/// its old place either, though that track's devices have the same names, and though Live says nothing of who they are.
#[test]
fn a_track_thats_gone_is_skipped_without_a_word() {
    for (gone_first, identities) in [(false, true), (true, true), (true, false)] {
        on_a_paused_clock(async {
            let case = format!("gone first {gone_first}, identities {identities}");
            let (gone, other): ((&str, &[&str]), (&str, &[&str])) = (("Gone", &["Synth"]), ("Other", &["Synth", "Limiter"]));
            let live = Live::new(&if gone_first { [gone, other] } else { [other, gone] }).await;
            live.set.identities.set(identities);
            live.set.turn("Other", "Synth", "Tone", 0.3);
            let goal = live.goal(&["Gone"]).await;
            let slot = slot_named(&goal, "Gone");
            goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
            goal.close().await.unwrap();
            live.set.delete_track("Gone");

            let said = goal.put_back(Signal::new()).await;

            assert!(said.is_empty(), "{case}: {said:?}");
            assert_eq!(live.set.knob("Other", "Synth", "Tone"), 0.3, "{case}: the track now at its place");
            assert_eq!(live.set.chain("Other"), ["Synth", "Limiter"], "{case}: and its own Limiter");
        });
    }
}

/// (#399) One knob Live won't take back doesn't stop the rest, or the limiter: it's named, and a later goal can still finish it.
#[test]
fn a_knob_live_wont_take_back_is_said_and_the_rest_go_back() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth", "EQ"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6), ("Synth", "Drive", 0.3), ("EQ", "Tone", 0.7)])], Signal::new(), None)
            .await
            .unwrap();
        goal.close().await.unwrap();
        live.set.refused.borrow_mut().push(("Synth".into(), "Tone".into()));

        let said = goal.put_back(Signal::new()).await;

        assert_eq!(said, ["Live wouldn't take Synth Tone on “Candidate” back to where the goal found it: check it."]);
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.6);
        assert_eq!(live.set.knob("Candidate", "Synth", "Drive"), 0.2, "the rest of its device's");
        assert_eq!(live.set.knob("Candidate", "EQ", "Tone"), 0.5, "the next device's");
        assert_eq!(live.set.chain("Candidate"), ["Synth", "EQ"], "and its limiter came off");

        live.set.refused.borrow_mut().clear();
        assert!(goal.put_back(Signal::new()).await.is_empty());
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.5, "what was left, the next time");
    });
}

/// (#399) A goal paused because Live went, say, can't read anything back. What it knows stays, so the goal that picks it up still
/// takes the candidate back to where it first was, not to where this search left it.
#[test]
fn a_put_back_that_cant_read_keeps_what_it_knows_for_the_next_goal() {
    for kind in ["track", "device"] {
        on_a_paused_clock(async {
            let live = Live::new(&[("Candidate", &["Synth"])]).await;
            let goal = live.goal(&["Candidate"]).await;
            let slot = slot_named(&goal, "Candidate");
            goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
            goal.close().await.unwrap();
            live.fail_reads(kind, 1);

            let said = goal.put_back(Signal::new()).await;

            assert_eq!(said.len(), 1, "{kind}: {said:?}");
            assert!(said[0].starts_with("Kumi couldn't put “Candidate” back as the goal found it"), "{kind}: {said:?}");
            assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.6, "{kind}");

            *live.set.on_call.borrow_mut() = None;
            let goal = live.goal(&["Candidate"]).await;
            let slot = slot_named(&goal, "Candidate");
            goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.7)])], Signal::new(), None).await.unwrap();
            goal.close().await.unwrap();

            assert!(goal.put_back(Signal::new()).await.is_empty(), "{kind}");
            assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.5, "{kind}: where the first goal found it");
            assert_eq!(live.set.chain("Candidate"), ["Synth"], "{kind}");
        });
    }
}

/// (#399) A limiter that won't come off a candidate going back doesn't stay turned down, and it's said.
#[test]
fn a_limiter_that_wont_come_off_a_candidate_goes_to_unity_and_is_said() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        goal.close().await.unwrap();
        *live.set.on_call.borrow_mut() = Some(Box::new(|name, _| {
            if name == "live_device_delete_preview" {
                return Err(RuntimeError::plain("Live didn't answer."));
            }
            Ok(())
        }));

        let said = goal.put_back(Signal::new()).await;

        assert_eq!(said, ["Kumi's safety Limiter stays last on “Candidate”: delete it if you don't want it there."]);
        assert_eq!(live.set.knob("Candidate", "Limiter", "Gain"), 0.5);
    });
}

/// (#399) A resumed goal puts a fresh limiter on and turns it down, so it hears as it did before the pause, whether or not Kumi
/// was restarted meanwhile. Nothing of Kumi's is left when it ends again.
#[test]
fn a_resumed_goal_hears_through_a_limiter_turned_down() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        goal.close().await.unwrap();
        goal.put_back(Signal::new()).await;
        assert_eq!(live.set.chain("Candidate"), ["Synth"]);

        let goal = live.goal(&["Candidate"]).await;

        assert_eq!(live.set.knob("Candidate", "Limiter", "Gain"), 0.25);
        goal.close().await.unwrap();
        goal.put_back(Signal::new()).await;
        assert_eq!(live.set.chain("Candidate"), ["Synth"]);
    });
}

/// (#399) A device put in after Kumi's limiter, as `load_device` does in a leap: a second limiter goes on for the new end of the
/// chain, and every one of Kumi's is seen to, on the best and on the candidate.
#[test]
fn every_limiter_kumi_added_is_seen_to_when_a_device_went_in_after_one() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        goal.close().await.unwrap();
        live.set.put_device_in("Candidate", 2, "Saturator");
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        assert_eq!(live.set.chain("Candidate"), ["Synth", "Limiter", "Saturator", "Limiter"]);
        goal.close().await.unwrap();

        let best = keep(&goal, &slot, &[("Synth", "Tone", 0.7)]).await;
        assert_eq!(live.set.knob_at(&best, 1, "Gain"), 0.5, "the best's first");
        assert_eq!(live.set.knob_at(&best, 3, "Gain"), 0.5, "the best's second");

        let said = goal.put_back(Signal::new()).await;
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(live.set.chain("Candidate"), ["Synth", "Saturator"]);
    });
}

/// (#399) A Limiter the producer put last stays on when the candidate goes back, at its own gain.
#[test]
fn a_producers_own_limiter_stays_when_a_candidate_goes_back() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth", "Limiter"])]).await;
        live.set.turn("Candidate", "Limiter", "Gain", 0.6);
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();

        goal.put_back(Signal::new()).await;

        assert_eq!(live.set.chain("Candidate"), ["Synth", "Limiter"]);
        assert_eq!(live.set.knob("Candidate", "Limiter", "Gain"), 0.6);
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.5);
    });
}

/// (#399) Across a leap the search's words for a knob can be another device's in the new rig. A knob first moved then still goes
/// back to its own value, as it read.
#[test]
fn a_knob_first_moved_after_a_leap_goes_back_to_its_own_value() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["EQ", "EQ"])]).await;
        live.set.tracks.borrow_mut()[0].devices[0].knobs[1].1 = 0.9;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.close().await.unwrap();
        live.set.put_device_in("Candidate", 0, "Saturator");

        let goal = live.goal(&["Candidate"]).await;
        goal.generation(&[trial(&slot, &[("1:EQ", "Drive", 0.5)])], Signal::new(), None).await.unwrap();
        assert_eq!(live.set.knob_at("Candidate", 2, "Drive"), 0.5, "the second EQ");
        goal.close().await.unwrap();

        assert!(goal.put_back(Signal::new()).await.is_empty());
        assert_eq!(live.set.knob_at("Candidate", 2, "Drive"), 0.2, "its own, not the first EQ's");
        assert_eq!(live.set.knob_at("Candidate", 1, "Drive"), 0.9);
    });
}

/// (#399) A candidate renamed during the search is still there at the search's values: its knobs are found at its old place by
/// who their devices are, and it goes back. Nothing else at that place would answer to them.
#[test]
fn a_candidate_renamed_during_the_search_still_goes_back() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();
        live.set.rename_track("Candidate", "Renamed");

        let said = goal.put_back(Signal::new()).await;

        assert!(said.is_empty(), "{said:?}");
        assert_eq!(live.set.knob("Renamed", "Synth", "Tone"), 0.5);
        assert_eq!(live.set.chain("Renamed"), ["Synth"]);
    });
}

/// A goal without Kumi Ears holds its recording between passes, so nothing waits on stopping it: Live shows a take only once it
/// has stopped playing, a moment after it's asked. A pass waits for that, and hears its take.
#[test]
fn a_held_pass_waits_for_live_to_show_its_take() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        live.set.lag.set(3);
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");

        let generation = goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();

        assert!(generation.silent.is_empty(), "{:?}", generation.silent);
        assert!(generation.scores.contains_key("Candidate"), "{:?}", generation.scores);
        goal.close().await.unwrap();
    });
}

/// (#399) Once it's put back, a later goal on the track finds it as the producer left it: nothing of this goal's is carried on.
#[test]
fn a_goal_put_back_leaves_nothing_for_the_next() {
    on_a_paused_clock(async {
        let live = Live::new(&[("Candidate", &["Synth"])]).await;
        let goal = live.goal(&["Candidate"]).await;
        let slot = slot_named(&goal, "Candidate");
        goal.generation(&[trial(&slot, &[("Synth", "Tone", 0.6)])], Signal::new(), None).await.unwrap();
        goal.close().await.unwrap();
        goal.put_back(Signal::new()).await;
        live.set.turn("Candidate", "Synth", "Tone", 0.9);

        let said = goal.put_back(Signal::new()).await;

        assert!(said.is_empty(), "{said:?}");
        assert_eq!(live.set.knob("Candidate", "Synth", "Tone"), 0.9);
    });
}
