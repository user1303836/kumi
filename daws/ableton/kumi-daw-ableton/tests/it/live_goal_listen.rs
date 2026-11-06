//! The guarded listen path against the same stateful Set and real mutations/history as goals.
use super::*;
use kumi_agent::core::context::HostPolicy;
use kumi_daw_ableton::ears::{
    device::{EARS_ITEM, EARS_VERSION},
    link::{Armed, EarsError, EarsLink, Tap, Transport, Written},
};
use kumi_types::errors::HearFailure;
use std::io::Write;

pub(super) struct State {
    identity: RefCell<String>,
    epoch: Cell<f64>,
    transactions: RefCell<std::collections::HashMap<String, f64>>,
    position: Cell<f64>,
    start_time: Cell<f64>,
    resume_position: Cell<f64>,
    playback_from: Cell<f64>,
    transport_writes: RefCell<Vec<&'static str>>,
    undo_open: Cell<bool>,
    undo_begins: Cell<usize>,
    undo_ends: Cell<usize>,
    begin_fails: Cell<bool>,
    delete_fails: Cell<bool>,
    end_fails: Cell<bool>,
    delete_cancel: RefCell<Option<Signal>>,
    native_selection: RefCell<HashMap<String, Value>>,
    disarmed: Cell<usize>,
    seek_delay_reads: Cell<u8>,
    pending_position: Cell<Option<(f64, u8)>>,
    ears_position: Cell<f64>,
    ears_seek_delay_reads: Cell<u8>,
    pending_ears_position: Cell<Option<(f64, u8)>>,
    false_end: Cell<bool>,
    captured_frames: Cell<usize>,
    looped: Cell<bool>,
    tempo: Cell<f64>,
    recording: Cell<bool>,
    back_to_arrangement: Cell<bool>,
    advancing: Cell<bool>,
    ignore_restore: Cell<bool>,
    ignore_stop: Cell<bool>,
    selected_track: Cell<usize>,
    selected_device: RefCell<Value>,
    selection_extra: RefCell<JsonObject>,
    after_delete: RefCell<Option<Box<dyn Fn(&State)>>>,
    applies: Cell<usize>,
    selection_writes: Cell<usize>,
    missing: Cell<Option<(&'static str, &'static str)>>,
    switch_set: Cell<bool>,
    cancel: RefCell<Option<Signal>>,
    starts: Cell<usize>,
    stops: Cell<usize>,
    opened: Cell<usize>,
    closed: Cell<usize>,
    arms: RefCell<Vec<f64>>,
    armed_locations: RefCell<Vec<String>>,
    written: Cell<usize>,
    armed_from: Cell<f64>,
    tap_version: Cell<f64>,
    tap_available: Cell<bool>,
    prepend_ears: Cell<bool>,
    raw_frames: Cell<usize>,
    reported_file: RefCell<Option<Box<dyn Fn(&str) -> String>>>,
}
impl State {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            identity: RefCell::new("song".into()),
            position: Cell::new(12.),
            start_time: Cell::new(3.),
            resume_position: Cell::new(12.),
            playback_from: Cell::new(12.),
            transport_writes: RefCell::new(vec![]),
            undo_open: Cell::new(false),
            undo_begins: Cell::new(0),
            undo_ends: Cell::new(0),
            begin_fails: Cell::new(false),
            delete_fails: Cell::new(false),
            end_fails: Cell::new(false),
            delete_cancel: RefCell::new(None),
            native_selection: RefCell::new(HashMap::new()),
            disarmed: Cell::new(0),
            seek_delay_reads: Cell::new(0),
            pending_position: Cell::new(None),
            ears_position: Cell::new(12.),
            ears_seek_delay_reads: Cell::new(0),
            pending_ears_position: Cell::new(None),
            false_end: Cell::new(false),
            captured_frames: Cell::new(0),
            looped: Cell::new(true),
            epoch: Cell::new(7.),
            transactions: RefCell::new(std::collections::HashMap::new()),
            tempo: Cell::new(120.),
            recording: Cell::new(false),
            back_to_arrangement: Cell::new(false),
            advancing: Cell::new(true),
            ignore_restore: Cell::new(false),
            ignore_stop: Cell::new(false),
            selected_track: Cell::new(0),
            selected_device: RefCell::new(json!("7:device:0:d0")),
            selection_extra: RefCell::new(
                json!({"sceneRef":null,"slotRef":null,"detailClipRef":null,
                "parameterRef":null,"chainRef":null})
                .as_object()
                .unwrap()
                .clone(),
            ),
            after_delete: RefCell::new(None),
            applies: Cell::new(0),
            selection_writes: Cell::new(0),
            missing: Cell::new(None),
            switch_set: Cell::new(false),
            cancel: RefCell::new(None),
            starts: Cell::new(0),
            stops: Cell::new(0),
            opened: Cell::new(0),
            closed: Cell::new(0),
            arms: RefCell::new(vec![]),
            written: Cell::new(0),
            armed_from: Cell::new(0.),
            armed_locations: RefCell::new(Vec::new()),
            tap_version: Cell::new(EARS_VERSION as f64),
            tap_available: Cell::new(true),
            prepend_ears: Cell::new(false),
            raw_frames: Cell::new(usize::MAX),
            reported_file: RefCell::new(None),
        })
    }
    pub(super) fn epoch(&self) -> f64 {
        self.epoch.get()
    }
    pub(super) fn prepared(&self, transaction: &str) -> f64 {
        let epoch = self.epoch.get();
        self.transactions.borrow_mut().insert(transaction.to_owned(), epoch);
        epoch
    }
    pub(super) fn transaction_matches(&self, transaction: &str) -> bool {
        self.transactions.borrow().get(transaction) == Some(&self.epoch.get())
    }
    pub(super) fn song_state(&self) -> Value {
        let mut song = json!({"backToArranger":self.back_to_arrangement.get(),"sessionRecord":false,
            "sessionAutomationRecord":false,"overdub":false,"arrangementOverdub":false,"reEnableAutomationEnabled":false});
        self.omit("song", &mut song);
        song
    }
    fn omit(&self, kind: &str, row: &mut Value) {
        if let Some((missing_kind, pointer)) = self.missing.get() {
            if missing_kind == kind {
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                row.pointer_mut(parent).unwrap().as_object_mut().unwrap().remove(key);
            }
        }
    }
    pub(super) fn selection(&self) -> Value {
        let mut selection = Value::Object(self.selection_extra.borrow().clone());
        selection["trackRef"] = json!(format!("7:track:{}", self.selected_track.get()));
        selection["deviceRef"] = self.selected_device.borrow().clone();
        self.omit("selection_snapshot", &mut selection);
        selection
    }
    fn native_selection_receipt(&self, set: &Set) -> Value {
        let tracks = set.tracks.borrow();
        let track_identity = tracks.get(self.selected_track.get()).map(|track| track.id);
        let device = self.selected_device.borrow();
        let device_identity = device.as_str().and_then(|reference| {
            let at = places(reference);
            tracks.get(*at.first()?).and_then(|track| track.devices.get(*at.get(1)?)).map(|device| device.identity)
        });
        json!({"row":self.selection(),"identities":{"trackRef":track_identity,"deviceRef":device_identity}})
    }
    fn set_selection(&self, selection: &Value) {
        if let Some(track) = selection["trackRef"].as_str() {
            self.selected_track.set(places(track)[0]);
        }
        if let Some(device) = selection.get("deviceRef") {
            *self.selected_device.borrow_mut() = device.clone();
        }
        for key in ["sceneRef", "slotRef", "detailClipRef", "parameterRef", "chainRef"] {
            if let Some(value) = selection.get(key) {
                self.selection_extra.borrow_mut()[key] = value.clone();
            }
        }
    }
    pub(super) fn undo_step(&self, _set: &Set, name: &str, args: &JsonObject) -> Result<Value, RuntimeError> {
        if name == "live_undo_step_begin" {
            self.undo_begins.set(self.undo_begins.get() + 1);
            assert!(!self.undo_open.get(), "never nest cleanup scopes");
            assert_eq!(args["label"], "Kumi: listen");
            assert_eq!(args["timeoutMs"], 30000);
            if self.begin_fails.get() {
                return Err(RuntimeError::plain("begin failed"));
            }
            self.undo_open.set(true);
            Ok(json!({"open":true,"stepId":"listen-step"}))
        } else {
            self.undo_ends.set(self.undo_ends.get() + 1);
            assert!(self.undo_open.get());
            assert_eq!(args["stepId"], "listen-step");
            if self.end_fails.get() {
                return Err(RuntimeError::plain("end failed"));
            }
            self.undo_open.set(false);
            Ok(json!({"closed":true,"stepId":"listen-step"}))
        }
    }
    pub(super) fn undo(&self, set: &Set, args: &JsonObject) -> Value {
        assert!(self.undo_open.get(), "native deletion must be inside its own undo step");
        assert_eq!(args["preserveSelection"], true);
        let transaction = text(&args["transactionId"]);
        if let Some(cancel) = self.delete_cancel.borrow_mut().take() {
            cancel.cancel();
        }
        if self.delete_fails.get() || !self.transaction_matches(transaction) {
            return json!({"transactionId":transaction,"state":"kept","deleted":false});
        }
        let before = self.selection();
        let previous = self.native_selection.borrow().get(transaction) == Some(&self.native_selection_receipt(set));
        let mut deleted = false;
        if let Some(Undo::RemoveDevice(identity)) = set.undos.borrow_mut().remove(transaction) {
            for (track_index, track) in set.tracks.borrow_mut().iter_mut().enumerate() {
                if let Some(index) = track.devices.iter().position(|device| device.identity == identity) {
                    track.devices.remove(index);
                    self.selected_track.set(track_index);
                    *self.selected_device.borrow_mut() = if track.devices.is_empty() {
                        Value::Null
                    } else {
                        json!(format!("7:device:{track_index}:d{}", index.saturating_sub(1).min(track.devices.len() - 1)))
                    };
                    deleted = true;
                }
            }
        }
        // Native keepSelection restores the pre-delete producer state, not a client-guessed sibling.
        self.set_selection(&before);
        if let Some(after) = self.after_delete.borrow_mut().take() {
            after(self);
        }
        json!({"transactionId":transaction,"state":"undone","deleted":deleted,
            "selection":{"before":before,"after":self.selection(),"previous":previous,"restored":self.selection() == before}})
    }
    pub(super) fn discover(&self, set: &Set, args: &JsonObject) -> Option<Vec<Value>> {
        let tracks = set.tracks.borrow();
        let kind = text(&args["kind"]);
        if kind == "session-playback" {
            if let Some((position, left)) = self.pending_position.get() {
                if left == 0 {
                    self.position.set(position);
                    self.pending_position.set(None);
                } else {
                    self.pending_position.set(Some((position, left - 1)));
                }
            }
        }
        let mut rows: Vec<Value> = match kind {
            "set" => vec![json!({"ref":"7:set:song","objectIdentity":*self.identity.borrow(),"name":"Fixture Set",
                "tempo":self.tempo.get(),"position":self.position.get(),"playing":set.playing.get(),
                "loop":{"enabled":self.looped.get(),"length":16}})],
            "track" => tracks
                .iter()
                .take(tracks.len() - 1)
                .enumerate()
                .map(|(index, track)| {
                    json!({"ref":format!("7:track:{index}"),
                "parentRef":"7:set:song","objectIdentity":track.id,"name":track.name,"armed":track.armed,"monitoringState":1,
                "isFrozen":false})
                })
                .collect(),
            "return-track" => vec![],
            "main-track" => vec![json!({"ref":format!("7:track:{}",tracks.len() - 1),"parentRef":"7:set:song",
                "objectIdentity":tracks.last().unwrap().id,"name":"Main","armed":false,"monitoringState":null})],
            "selection" => vec![json!({"ref":"7:selection:current","parentRef":"7:set:song",
                "selectedTrackRef":format!("7:track:{}",self.selected_track.get()),"selectedDeviceRef":*self.selected_device.borrow()})],
            "session-playback" => vec![json!({"ref":"7:session_playback:0","transport":{
                "playing":set.playing.get(),"arrangementRecord":self.recording.get(),"sessionRecord":false,
                "position":self.position.get(),"startTime":self.start_time.get(),"loop":{"enabled":self.looped.get(),"start":4,"length":16},
                "punchIn":false,"punchOut":false},"firedTargets":[],"playingTargets":[]})],
            "device" => {
                let parent = text(&args["parent"]);
                let at = places(parent)[0];
                tracks[at]
                    .devices
                    .iter()
                    .enumerate()
                    .map(|(index, device)| {
                        json!({"ref":format!("7:device:{at}:d{index}"),"parentRef":parent,"name":device.name,
                        "className":device.class,"classDisplayName":device.class,"enabled":true,"objectIdentity":device.identity})
                    })
                    .collect()
            }
            _ => return None,
        };
        for row in &mut rows {
            self.omit(kind, row);
            // Remote Script trims every explicitly requested field list, including an
            // empty one, while retaining the row's ref and parentRef.
            if let Some(fields) = args.get("fields").and_then(Value::as_array) {
                row.as_object_mut().unwrap().retain(|key, _| {
                    matches!(key.as_str(), "ref" | "parentRef") || fields.iter().any(|field| field.as_str() == Some(key.as_str()))
                });
            }
        }
        Some(rows)
    }
    pub(super) fn apply(&self, set: &Set, tool: &str, args: &JsonObject, transaction: &str) -> Option<Value> {
        self.applies.set(self.applies.get() + 1);
        let mut answer = json!({"transactionId":transaction,"state":"applied"});
        match tool {
            "live_transport" => {
                if let Some(position) = args.get("position").and_then(Value::as_f64) {
                    assert!(!set.playing.get());
                    self.transport_writes.borrow_mut().push("position");
                    self.start_time.set(position);
                    if !(position == 12. && self.ignore_restore.get()) {
                        if self.seek_delay_reads.get() == 0 {
                            self.position.set(position);
                        } else {
                            self.pending_position.set(Some((position, self.seek_delay_reads.get())));
                        }
                        self.pending_ears_position.set(Some((position, self.ears_seek_delay_reads.get())));
                    }
                }
                if let Some(looped) = args.get("loopEnabled").and_then(Value::as_bool) {
                    self.looped.set(looped);
                }
                if let Some(start) = args.get("startTime") {
                    assert!(!set.playing.get(), "startTime is only writable while stopped");
                    self.transport_writes.borrow_mut().push("startTime");
                    self.start_time.set(start.as_f64().expect("numeric startTime"));
                }
            }
            "live_transport_action" => match args.get("action").map(text) {
                Some(action @ ("start" | "continue")) => {
                    assert!(!self.undo_open.get(), "no native undo scope across capture");
                    // A stopped seek does not move Live's saved resume position.
                    let from = if action == "start" { self.start_time.get() } else { self.resume_position.get() };
                    self.position.set(from);
                    self.ears_position.set(from);
                    self.playback_from.set(from);
                    set.playing.set(true);
                    self.starts.set(self.starts.get() + 1);
                }
                Some("stop") => {
                    if !self.ignore_stop.get() {
                        self.resume_position.set(self.position.get());
                        set.playing.set(false);
                    }
                    self.stops.set(self.stops.get() + 1);
                }
                other => panic!("a restricted capture cannot perform {other:?}"),
            },
            "live_selection" => panic!("restricted listening must not guess native selection writes"),
            "live_browser_load" => {
                assert_eq!(args["itemId"], EARS_ITEM);
                assert_eq!(args["preserveSelection"], true);
                self.native_selection.borrow_mut().insert(transaction.into(), self.native_selection_receipt(set));
                let at = places(text(&args["trackRef"]))[0];
                let device = Device::new("Kumi Ears");
                answer["deviceObjectIdentity"] = json!(device.identity.to_string());
                set.undos.borrow_mut().insert(transaction.into(), Undo::RemoveDevice(device.identity));
                let mut tracks = set.tracks.borrow_mut();
                let index = if self.prepend_ears.get() { 0 } else { tracks[at].devices.len() };
                tracks[at].devices.insert(index, device);
                answer["deviceRef"] = json!(format!("7:device:{at}:d{index}"));
                answer["placement"] = json!({"owner":"track","index":index});
                // The bridge privately retains previous/left selection receipts.
            }
            _ => return None,
        }
        Some(answer)
    }
}

struct Ears {
    set: Rc<Set>,
    state: Rc<State>,
}
#[async_trait(?Send)]
impl EarsLink for Ears {
    fn port(&self) -> u16 {
        18000
    }
    fn taps(&self) -> Vec<Tap> {
        if !self.state.tap_available.get() {
            return vec![];
        }
        let tracks = self.set.tracks.borrow();
        let main = tracks.len() - 1;
        tracks
            .iter()
            .enumerate()
            .flat_map(|(track, value)| {
                value.devices.iter().enumerate().filter(|(_, device)| device.name == "Kumi Ears").map(move |(device, value)| Tap {
                    id: value.identity as f64,
                    port: 18001,
                    path: if track == main {
                        format!("live_set master_track devices {device}")
                    } else {
                        format!("live_set tracks {track} devices {device}")
                    },
                    version: self.state.tap_version.get(),
                    sample_rate: 8000.,
                    seen_at: 0,
                    loaded_at: None,
                })
            })
            .collect()
    }
    async fn wait_for(&self, accept: Rc<dyn for<'a> Fn(&'a Tap) -> bool>, _: u64, _: Option<Signal>) -> Option<Tap> {
        self.taps().into_iter().find(|tap| tap.version == EARS_VERSION as f64 && accept(tap))
    }
    async fn arm(&self, tap: &Tap, seconds: f64, quiet: bool, _: Option<Signal>) -> Result<Armed, EarsError> {
        assert!(!self.state.undo_open.get(), "arming is outside the deletion undo step");
        assert!(!quiet, "a listen cannot mute the producer's track");
        self.state.arms.borrow_mut().push(seconds);
        self.state.armed_locations.borrow_mut().push(tap.path.clone());
        self.state.armed_from.set(self.state.ears_position.get());
        self.state.captured_frames.set(0);
        Ok(Armed { beats: self.state.ears_position.get(), running: self.set.playing.get(), sample_rate: 8000. })
    }
    async fn write(&self, _: &Tap, file: &str, _: Option<Signal>) -> Result<Written, EarsError> {
        self.state.written.set(self.state.written.get() + 1);
        let mut raw = std::fs::File::create(file).unwrap();
        // Both the clock's origin and duration come from playback, not the arm
        // request or a successful tool reply. raw_frames can model a short capture.
        for frame in 0..self.state.raw_frames.get().min(self.state.captured_frames.get()) {
            let beat = self.state.playback_from.get() + frame as f64 / 4000.;
            let sample = (frame as f64 * std::f64::consts::TAU * 200. / 8000.).sin() as f32 * 0.2;
            for value in [sample, sample, 1. + beat.fract() as f32, beat as f32] {
                raw.write_all(&value.to_le_bytes()).unwrap();
            }
        }
        let file = self.state.reported_file.borrow().as_ref().map_or_else(|| file.to_owned(), |report| report(file));
        Ok(Written { file, sample_rate: 8000., channels: 4, beats: self.state.armed_from.get(), running: false })
    }
    fn stop(&self, _: &Tap) {
        self.state.disarmed.set(self.state.disarmed.get() + 1);
    }
    async fn ping(&self, tap: &Tap, _: Option<Signal>) -> Option<Tap> {
        Some(tap.clone())
    }
    async fn transport(&self, _: &Tap, _: Option<Signal>) -> Result<Option<Transport>, EarsError> {
        assert!(!self.state.undo_open.get(), "audio capture is outside the deletion undo step");
        if !self.set.playing.get() {
            if let Some((position, left)) = self.state.pending_ears_position.get() {
                if left == 0 {
                    self.state.ears_position.set(position);
                    self.state.pending_ears_position.set(None);
                } else {
                    self.state.pending_ears_position.set(Some((position, left - 1)));
                }
            }
        } else {
            let seconds = 60. / self.state.tempo.get();
            tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
            self.state.captured_frames.set(self.state.captured_frames.get() + (seconds * 8000.).round() as usize);
            if let Some(cancel) = self.state.cancel.borrow_mut().take() {
                cancel.cancel();
            }
            if self.state.switch_set.get() {
                *self.state.identity.borrow_mut() = "another-song".into();
                self.state.position.set(77.);
            } else if self.state.advancing.get() {
                self.state.position.set(self.state.position.get() + 1.);
            }
            self.state.ears_position.set(self.state.position.get());
        }
        let beats = self.state.ears_position.get() + if self.set.playing.get() && self.state.false_end.get() { 100. } else { 0. };
        Ok(Some(Transport { beats, running: self.set.playing.get() }))
    }
    async fn close(&self) {
        self.state.closed.set(self.state.closed.get() + 1);
    }
}

async fn bounded(ears: bool) -> (Live, Rc<State>, HearRequest) {
    let state = State::new();
    let configured = state.clone();
    let live = Live::configured(&[("Bass", &["Synth"]), ("Main", &["Limiter"])], move |set, options, root| {
        let scratch = root.join("listen");
        let call = scratch.join("native-call");
        std::fs::create_dir(&scratch).unwrap();
        std::fs::create_dir(&call).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&scratch, &call] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        options.policy = HostPolicy::plugin(scratch, root.join("quarantine"), root.to_path_buf());
        options.ears = None; // Without an injected link, exercise the real read-only browser lookup.
        *set.bounded.borrow_mut() = Some(configured.clone());
        if ears {
            let link = Rc::new(Ears { set: set.clone(), state: configured.clone() });
            options.ears = Some(EarsSetup::Open(Rc::new(move || {
                configured.opened.set(configured.opened.get() + 1);
                let link: Rc<dyn EarsLink> = link.clone();
                async move { Ok(link) }.boxed_local()
            })));
        }
    })
    .await;
    *live.history.connection.set.borrow_mut() = Some(json!(["7:set:song", "song"]).to_string());
    let rows =
        live.history.connection.rows("track", serde_json::from_value(json!({"fields":["name"]})).unwrap(), Signal::new()).await.unwrap();
    let reference = rows[0]["ref"].as_str().unwrap().to_owned();
    let mut request: HearRequest = serde_json::from_value(json!({"tracks":[reference],"fromBeat":8,"beats":4})).unwrap();
    request.scratch_dir = Some(live._folder.path().join("listen/native-call"));
    request.expected_native_epoch = Some("7".into());
    request.native_deadline_unix_ms = Some(kumi_common::time::now_ms() as u64 + 300_000);
    request.approval = Some(kumi_types::contracts::HearApproval { seconds: 2., deadline_seconds: 300 });
    (live, state, request)
}

fn restored(live: &Live, state: &State) {
    assert_eq!(live.set.chain("Bass"), ["Synth"]);
    assert!(!live.set.playing.get());
    assert_eq!(state.position.get(), 12.);
    assert_eq!(state.start_time.get(), 3.);
    assert!(!state.undo_open.get());
    assert_eq!(state.selection_writes.get(), 0, "selection restoration belongs to the native bridge");
    assert!(state.looped.get());
    assert!(live.history.entries.borrow().is_empty(), "temporary changes cannot become an undoable user change");
}

#[test]
fn preview_reads_only_and_names_a_bounded_ears_capture() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let preview = live.rendering.preview_hear(&request, Signal::new()).await.unwrap();
        assert_eq!(preview.seconds, 2.);
        assert_eq!(preview.method, HearMethod::Ears);
        assert!((1..=360).contains(&preview.deadline_seconds));
        assert_eq!(state.opened.get(), 0);
        assert_eq!(state.applies.get(), 0, "a listen preview changes nothing in Live");
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn capture_trims_the_approved_window_and_verifies_restoration() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        // Real Live resumed here after a verified seek to the approved start.
        state.resume_position.set(28.1456);
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert_eq!(takes.len(), 1);
        assert_eq!(takes[0].start, 0.);
        assert!((takes[0].seconds.unwrap() - 2.).abs() < 0.001);
        assert!(!takes[0].marked_modified, "Live's read state has no dirty flag; mutation is not a reported-modified claim");
        assert!(Path::new(&takes[0].file).starts_with(request.scratch_dir.as_ref().unwrap().canonicalize().unwrap()));
        let wav = std::fs::read(&takes[0].file).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(state.starts.get(), 1, "no replay");
        assert_eq!(state.arms.borrow().as_slice(), [7.]);
        assert_eq!(state.closed.get(), 1);
        restored(&live, &state);
        for call in live.set.asked.borrow().iter() {
            let name = call["name"].as_str().unwrap();
            assert_ne!(name, "live_run_python", "no script fallback");
            assert_ne!(name, "live_session_emergency_stop", "no unfenced stop");
            if name.ends_with("_apply") {
                assert!(
                    matches!(
                        name,
                        "live_browser_load_apply" | "live_transport_apply" | "live_transport_action_apply" | "live_selection_apply"
                    ),
                    "forbidden mutation: {call}"
                );
            }
            if name == "live_transport_preview" {
                assert!(
                    call["args"].as_object().unwrap().keys().all(|key| matches!(key.as_str(), "position" | "loopEnabled" | "startTime")),
                    "no loop range, punch, tempo or recording-mode mutation: {call}"
                );
            }
        }
    });
}

#[cfg(windows)]
#[test]
fn windows_ears_receipts_resolve_to_the_requested_capture_file() {
    on_a_paused_clock(async {
        for spelling in 0..4 {
            let (live, state, request) = bounded(true).await;
            let original_directory = request.scratch_dir.clone().unwrap();
            *state.reported_file.borrow_mut() = Some(Box::new(move |wire| {
                let canonical = Path::new(wire).canonicalize().unwrap();
                let native = canonical.to_string_lossy();
                match spelling {
                    0 => wire.to_owned(), // Ears' actual echoed //?/C:/... wire spelling.
                    1 => native.into_owned(),
                    2 => native.strip_prefix(r"\\?\").unwrap().replace('\\', "/").to_uppercase(),
                    _ => original_directory.join(canonical.file_name().unwrap()).to_string_lossy().into_owned(),
                }
            }));
            let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
            assert_eq!(takes.len(), 1);
            assert_eq!(std::fs::metadata(&takes[0].file).unwrap().len(), 44 + 16000 * 2 * 4);
            assert_eq!(takes[0].seconds, Some(2.));
            assert_eq!(state.starts.get(), 1);
            restored(&live, &state);
        }
    });
}

#[test]
fn a_receipt_for_another_file_never_publishes_audio_or_removes_that_file() {
    on_a_paused_clock(async {
        for inside_call in [true, false] {
            let (live, state, request) = bounded(true).await;
            let call = request.scratch_dir.as_ref().unwrap();
            let other = if inside_call { call.join("another.raw") } else { live._folder.path().join("another.raw") };
            std::fs::write(&other, b"not this call's capture").unwrap();
            let reported = other.to_string_lossy().into_owned();
            *state.reported_file.borrow_mut() = Some(Box::new(move |_| reported.clone()));
            assert!(matches!(
                live.rendering.hear_in_set(&request, Signal::new()).await,
                Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_failed", .. }))
            ));
            assert_eq!(std::fs::read(&other).unwrap(), b"not this call's capture");
            assert!(std::fs::read_dir(call).unwrap().all(|entry| entry.unwrap().path() == other));
            assert_eq!(state.written.get(), 1);
            restored(&live, &state);
        }
    });
}

#[test]
fn back_to_arrangement_playback_and_recording_refuse_before_mutation() {
    on_a_paused_clock(async {
        for case in ["back", "playing", "recording"] {
            let (live, state, request) = bounded(true).await;
            match case {
                "back" => state.back_to_arrangement.set(true),
                "playing" => live.set.playing.set(true),
                "recording" => state.recording.set(true),
                _ => {}
            }
            assert!(
                matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Refused { .. }))),
                "{case}"
            );
            assert_eq!(state.applies.get(), 0, "{case}");
            assert_eq!(state.opened.get(), 0, "{case}");
        }
    });
}

#[test]
fn windows_are_explicit_finite_single_source_and_at_most_sixty_seconds() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        for (from, beats) in [
            (None, Some(4.)),
            (Some(0.), None),
            (Some(-1.), Some(4.)),
            (Some(f64::NAN), Some(4.)),
            (Some(0.), Some(0.)),
            (Some(0.), Some(257.)),
            (Some(0.), Some(121.)),
        ] {
            let mut invalid = request.clone();
            invalid.from_beat = from;
            invalid.beats = beats;
            assert!(matches!(
                live.rendering.preview_hear(&invalid, Signal::new()).await,
                Err(RuntimeError::Hear(HearFailure::Refused { .. }))
            ));
        }
        let mut invalid = request.clone();
        invalid.mix = Some(true);
        assert!(live.rendering.preview_hear(&invalid, Signal::new()).await.is_err());
        invalid.mix = None;
        invalid.tracks[0] = "Bass".into();
        assert!(live.rendering.preview_hear(&invalid, Signal::new()).await.is_err());
        assert_eq!(state.applies.get(), 0);
        assert_eq!(state.opened.get(), 0);
    });
}

#[test]
fn an_unmoving_transport_reaches_the_independent_capture_cap_without_replay() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        state.tempo.set(60.);
        state.advancing.set(false);
        request.beats = Some(60.);
        request.approval = None;
        let preview = live.rendering.preview_hear(&request, Signal::new()).await.unwrap();
        request.approval =
            Some(kumi_types::contracts::HearApproval { seconds: preview.seconds, deadline_seconds: preview.deadline_seconds as u16 });
        let started = tokio::time::Instant::now();
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_deadline", .. }))
        ));
        assert!(started.elapsed() <= std::time::Duration::from_secs(66));
        assert_eq!(state.arms.borrow().as_slice(), [65.]);
        assert_eq!(state.starts.get(), 1);
        assert_eq!(state.written.get(), 0);
        assert_eq!(state.closed.get(), 1);
        restored(&live, &state);
    });
}

#[test]
fn cancellation_waits_for_cleanup_and_an_acknowledged_but_failed_restore_is_unsure() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let cancel = Signal::new();
        *state.cancel.borrow_mut() = Some(cancel.clone());
        assert!(matches!(
            live.rendering.hear_in_set(&request, cancel).await,
            Err(RuntimeError::Hear(HearFailure::Cancelled { marked_modified: false }))
        ));
        restored(&live, &state);
        assert_eq!(state.closed.get(), 1);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());

        let (live, state, mut request) = bounded(true).await;
        state.ignore_restore.set(true);
        // End the simulated playback away from the original cursor, so an ignored restore is observable.
        request.from_beat = Some(4.);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Unsure { marked_modified: false, .. }))
        ));
        assert_eq!(state.closed.get(), 1);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn a_new_set_is_not_stopped_or_cleaned_up_as_if_it_were_the_capture_set() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.switch_set.set(true);
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert!(live.set.playing.get(), "do not stop the newly opened Set");
        assert_eq!(state.position.get(), 77.);
        assert_eq!(state.stops.get(), 0);
        assert_eq!(state.written.get(), 0);
        assert_eq!(state.closed.get(), 1);
    });
}

#[test]
fn incompatible_or_silent_ears_never_starts_playback_or_installs_a_replacement() {
    on_a_paused_clock(async {
        for version in [None, Some(EARS_VERSION as f64 - 1.), Some(EARS_VERSION as f64 + 1.)] {
            let (live, state, request) = bounded(true).await;
            state.tap_available.set(version.is_some());
            state.tap_version.set(version.unwrap_or(EARS_VERSION as f64));
            assert!(
                matches!(
                    live.rendering.hear_in_set(&request, Signal::new()).await,
                    Err(RuntimeError::Hear(HearFailure::Refused { code: "ears_missing", .. }))
                ),
                "{version:?}"
            );
            assert_eq!(state.starts.get(), 0);
            assert!(state.arms.borrow().is_empty());
            assert_eq!(state.closed.get(), 1);
            assert_eq!(live.set.asked_for("live_browser_load_apply"), 1, "load the preinstalled device only once");
            assert_eq!(live.set.asked_for("live_undo"), 1, "remove even a device that never greeted the link");
            restored(&live, &state);
            assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
        }
    });
}

#[test]
fn a_changed_set_identity_at_the_observed_epoch_is_refused_before_preview_or_capture() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.tracks.clear();
        request.mix = Some(true);
        *state.identity.borrow_mut() = "another-set".into();
        assert!(matches!(
            live.rendering.preview_hear(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
        ));
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
        ));
        assert_eq!(state.opened.get(), 0);
        assert_eq!(state.starts.get(), 0);
        assert_eq!(state.applies.get(), 0);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn a_set_switched_between_the_identity_read_and_stop_preview_is_not_stopped() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let switched = Rc::new(Cell::new(false));
        let (s, set, changed) = (state.clone(), live.set.clone(), switched.clone());
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_transport_action_preview" && args["action"] == "stop" && !changed.replace(true) {
                s.epoch.set(8.);
                *s.identity.borrow_mut() = "another-set".into();
                s.position.set(77.);
                set.playing.set(true);
            }
            Ok(())
        }));
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert!(switched.get());
        assert!(live.set.playing.get(), "neither an ordinary nor an emergency stop may reach the new Set");
        assert_eq!(state.position.get(), 77.);
        assert_eq!(state.stops.get(), 0);
        assert_eq!(state.written.get(), 0);
        assert_eq!(state.closed.get(), 1);
    });
}

#[test]
fn refreshing_to_another_set_during_ears_undo_does_not_restore_transport_into_it() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let switched = Rc::new(Cell::new(false));
        let (s, set, connection, changed) = (state.clone(), live.set.clone(), live.history.connection.clone(), switched.clone());
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
            if name == "live_undo" && !changed.replace(true) {
                s.epoch.set(8.);
                *s.identity.borrow_mut() = "another-set".into();
                s.position.set(77.);
                s.looped.set(false);
                set.playing.set(true);
                // A concurrent observation can refresh the shared connection. Cleanup must
                // still use the capture's identity, not whichever Set is now observed.
                connection.epoch.set(Some(8.));
                *connection.set.borrow_mut() = Some(json!(["7:set:song", "another-set"]).to_string());
            }
            Ok(())
        }));
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert!(switched.get());
        assert!(live.set.playing.get());
        assert_eq!(state.position.get(), 77.);
        assert!(!state.looped.get());
        assert_eq!(state.closed.get(), 1);
    });
}

#[test]
fn a_partial_observation_cannot_replace_the_mix_requests_retained_epoch() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.tracks.clear();
        request.mix = Some(true);
        let missing = HearRequest { expected_native_epoch: None, ..request.clone() };
        assert!(matches!(
            live.rendering.preview_hear(&missing, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
        ));
        state.epoch.set(8.);
        *state.identity.borrow_mut() = "another-set".into();
        live.history.connection.epoch.set(Some(8.));
        *live.history.connection.set.borrow_mut() = Some(json!(["7:set:song", "another-set"]).to_string());
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
        ));
        assert_eq!(state.opened.get(), 0);
        assert_eq!(state.applies.get(), 0);
    });
}

#[test]
fn recording_enabled_during_the_start_preview_is_never_started_by_capture() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let s = state.clone();
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_transport_action_preview" && args["action"] == "start" {
                s.recording.set(true);
            }
            Ok(())
        }));
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert!(state.recording.get(), "do not overwrite the producer's recording-mode change");
        assert_eq!(state.starts.get(), 0, "no playback start may turn that mode into a recording");
        assert!(!live.set.playing.get());
        assert_eq!(state.written.get(), 0);
        assert_eq!(state.closed.get(), 1);
    });
}

#[test]
fn whole_mix_capture_uses_main_and_restores_its_device_chain() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.tracks.clear();
        request.mix = Some(true);
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert_eq!(takes.len(), 1);
        assert_eq!(takes[0].label, "The whole mix");
        let locations = state.armed_locations.borrow();
        assert_eq!(locations.len(), 1);
        assert!(locations[0].starts_with("live_set master_track devices "), "{locations:?}");
        assert_eq!(live.set.chain("Main"), ["Limiter"]);
        restored(&live, &state);

        let (live, state, mut request) = bounded(true).await;
        request.tracks.clear();
        request.mix = Some(true);
        state.prepend_ears.set(true);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_incomplete", .. }))
        ));
        assert!(state.arms.borrow().is_empty(), "a tap before Main's limiter cannot represent the processed mix");
        assert_eq!(state.starts.get(), 0);
        assert_eq!(state.written.get(), 0);
        assert_eq!(state.closed.get(), 1);
        assert_eq!(live.set.chain("Main"), ["Limiter"]);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn a_slower_tempo_after_approval_is_stale_before_any_device_or_transport_change() {
    on_a_paused_clock(async {
        for mix in [false, true] {
            let (live, state, mut request) = bounded(true).await;
            if mix {
                request.tracks.clear();
                request.mix = Some(true);
            }
            state.tempo.set(60.);
            assert!(matches!(
                live.rendering.hear_in_set(&request, Signal::new()).await,
                Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
            ));
            assert_eq!(state.starts.get(), 0);
            assert_eq!(state.opened.get(), 0);
            assert_eq!(state.applies.get(), 0);
            assert_eq!(state.tempo.get(), 60., "the producer's tempo change is preserved");
            restored(&live, &state);
        }
    });
}

#[test]
fn a_capture_requires_approval_with_room_for_its_native_cleanup() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.approval = None;
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "not_approved", .. }))
        ));
        request.approval = Some(kumi_types::contracts::HearApproval { seconds: 2., deadline_seconds: 299 });
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
        ));
        assert_eq!(state.opened.get(), 0);
        assert_eq!(state.starts.get(), 0);
        assert_eq!(state.applies.get(), 0);
        restored(&live, &state);
    });
}

#[test]
fn missing_preinstalled_ears_is_typed_and_never_loads_or_writes_a_device() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(false).await;
        for preview in [true, false] {
            let error = if preview {
                live.rendering.preview_hear(&request, Signal::new()).await.unwrap_err()
            } else {
                live.rendering.hear_in_set(&request, Signal::new()).await.unwrap_err()
            };
            assert!(matches!(error, RuntimeError::Hear(HearFailure::Refused { code: "ears_missing", .. })));
        }
        assert_eq!(live.set.asked_for("live_browser_inspect"), 2, "exercise the actual preinstalled-item lookup");
        assert_eq!(state.opened.get(), 0);
        assert_eq!(state.applies.get(), 0);
        for call in live.set.asked.borrow().iter() {
            let name = call["name"].as_str().unwrap();
            assert!(
                !name.starts_with("live_browser_load") && !name.ends_with("_apply") && name != "live_run_python",
                "no device load, setup or write: {call}"
            );
        }
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn an_unreadable_preflight_refuses_without_claiming_uncertain_restoration() {
    on_a_paused_clock(async {
        for failed_read in ["live_discover", "live_song_state"] {
            let (live, state, request) = bounded(true).await;
            *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
                if name == failed_read {
                    return Err(RuntimeError::plain("bridge read failed at /private/producer/set"));
                }
                Ok(())
            }));
            for preview in [true, false] {
                let error = if preview {
                    live.rendering.preview_hear(&request, Signal::new()).await.unwrap_err()
                } else {
                    live.rendering.hear_in_set(&request, Signal::new()).await.unwrap_err()
                };
                let RuntimeError::Hear(HearFailure::Refused { code: "unsupported", reason, .. }) = error else {
                    panic!("preflight must refuse without a restoration claim: {error:?}");
                };
                assert!(!reason.contains("/private/"));
            }
            assert_eq!(state.opened.get(), 0);
            assert_eq!(state.applies.get(), 0);
            restored(&live, &state);
            assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
        }
    });
}

#[test]
fn an_acknowledged_stop_that_leaves_live_playing_is_unsure_and_discards_audio() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.ignore_stop.set(true);
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert!(state.stops.get() > 0, "the bridge acknowledged a stop");
        assert!(live.set.playing.get(), "the independent transport read still sees playback");
        assert_eq!(live.set.chain("Bass"), ["Synth"]);
        assert_eq!(state.closed.get(), 1);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn a_changed_selected_device_after_cleanup_is_unsure_and_discards_audio() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        *state.after_delete.borrow_mut() = Some(Box::new(|state| {
            *state.selected_device.borrow_mut() = json!("7:device:1:d0");
        }));
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert_eq!(*state.selected_device.borrow(), "7:device:1:d0", "do not overwrite the producer's selection");
        assert_eq!(state.written.get(), 1);
        assert_eq!(state.closed.get(), 1);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn every_required_state_field_must_be_read_before_and_after_capture() {
    on_a_paused_clock(async {
        for missing in [
            ("set", "/objectIdentity"),
            ("set", "/tempo"),
            ("selection_snapshot", "/trackRef"),
            ("selection_snapshot", "/deviceRef"),
            ("selection_snapshot", "/sceneRef"),
            ("selection_snapshot", "/slotRef"),
            ("selection_snapshot", "/detailClipRef"),
            ("selection_snapshot", "/parameterRef"),
            ("selection_snapshot", "/chainRef"),
            ("track", "/objectIdentity"),
            ("track", "/armed"),
            ("track", "/monitoringState"),
            ("main-track", "/objectIdentity"),
            ("main-track", "/armed"),
            ("main-track", "/monitoringState"),
            ("device", "/objectIdentity"),
            ("session-playback", "/transport/playing"),
            ("session-playback", "/transport/arrangementRecord"),
            ("session-playback", "/transport/sessionRecord"),
            ("session-playback", "/transport/position"),
            ("session-playback", "/transport/startTime"),
            ("session-playback", "/transport/loop/enabled"),
            ("session-playback", "/transport/loop/start"),
            ("session-playback", "/transport/loop/length"),
            ("session-playback", "/transport/punchIn"),
            ("session-playback", "/transport/punchOut"),
            ("session-playback", "/firedTargets"),
            ("session-playback", "/playingTargets"),
            ("song", "/backToArranger"),
            ("song", "/sessionRecord"),
            ("song", "/sessionAutomationRecord"),
            ("song", "/overdub"),
            ("song", "/arrangementOverdub"),
            ("song", "/reEnableAutomationEnabled"),
        ] {
            for before in [true, false] {
                let (live, state, request) = bounded(true).await;
                if before {
                    state.missing.set(Some(missing));
                } else {
                    let s = state.clone();
                    *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
                        if name == "live_undo" {
                            s.missing.set(Some(missing));
                        }
                        Ok(())
                    }));
                }
                let result = live.rendering.hear_in_set(&request, Signal::new()).await;
                if before {
                    assert!(matches!(result, Err(RuntimeError::Hear(HearFailure::Refused { .. }))), "{missing:?}: {result:?}");
                    assert_eq!(state.opened.get(), 0);
                    assert_eq!(state.applies.get(), 0);
                } else {
                    assert!(matches!(result, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))), "{missing:?}: {result:?}");
                    assert_eq!(state.written.get(), 1);
                    assert_eq!(state.closed.get(), 1);
                }
                assert_eq!(live.set.chain("Bass"), ["Synth"], "{missing:?}");
                assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none(), "{missing:?}, before={before}");
            }
        }
    });
}

#[test]
fn a_short_live_capture_is_refused_not_published_as_a_short_success() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.raw_frames.set(8000);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_incomplete", marked_modified: false, .. }))
        ));
        assert_eq!(state.written.get(), 1);
        assert_eq!(state.closed.get(), 1);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn deferred_live_seek_is_confirmed_before_ears_arms_and_playback_starts() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        state.seek_delay_reads.set(3);
        request.from_beat = Some(0.);
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert!((takes[0].seconds.unwrap() - 2.).abs() < 0.001);
        assert_eq!(state.starts.get(), 1, "the approved window plays once, never from the old cursor");
        restored(&live, &state);
    });
}

#[test]
fn deleting_ears_on_another_track_restores_the_original_selection() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.selected_track.set(1);
        *state.selected_device.borrow_mut() = json!("7:device:1:d0");
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert!((takes[0].seconds.unwrap() - 2.).abs() < 0.001);
        assert_eq!(state.selected_track.get(), 1);
        assert_eq!(*state.selected_device.borrow(), "7:device:1:d0");
        restored(&live, &state);
    });
}

#[test]
fn a_producer_selection_during_capture_is_not_overwritten_by_cleanup() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let changed = Rc::new(Cell::new(false));
        let (s, once) = (state.clone(), changed.clone());
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_transport_action_preview" && args["action"] == "stop" && !once.replace(true) {
                s.selected_track.set(1);
                *s.selected_device.borrow_mut() = json!("7:device:1:d0");
            }
            Ok(())
        }));
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert_eq!(takes.len(), 1);
        assert_eq!(state.selected_track.get(), 1);
        assert_eq!(*state.selected_device.borrow(), "7:device:1:d0");
        restored(&live, &state);
        assert!(Path::new(&takes[0].file).is_file());
    });
}

#[test]
fn native_and_observed_track_refs_name_the_same_track_but_old_epochs_refuse() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        let observed = live.rendering.preview_hear(&request, Signal::new()).await.unwrap();
        request.tracks[0] = "7:track:0".into();
        let native = live.rendering.preview_hear(&request, Signal::new()).await.unwrap();
        assert_eq!((native.label, native.seconds), (observed.label, observed.seconds));
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert_eq!(takes[0].label, "Bass");
        restored(&live, &state);
        request.tracks[0] = "8:track:0".into();
        let mutations = state.applies.get();
        assert!(matches!(
            live.rendering.preview_hear(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "stale", .. }))
        ));
        assert_eq!(state.applies.get(), mutations);
    });
}

#[test]
fn ears_must_read_back_the_stopped_seek_before_arming() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.from_beat = Some(0.);
        state.ears_seek_delay_reads.set(3);
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert!((takes[0].seconds.unwrap() - 2.).abs() < 0.001);
        assert_eq!(state.armed_from.get(), 0.);
        assert_eq!(state.starts.get(), 1);
        restored(&live, &state);
    });
}

#[test]
fn an_ears_clock_that_never_confirms_the_seek_is_never_armed_or_played() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.from_beat = Some(0.);
        state.ears_seek_delay_reads.set(u8::MAX);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_incomplete", .. }))
        ));
        assert!(state.arms.borrow().is_empty());
        assert_eq!(state.starts.get(), 0);
        assert_eq!(state.written.get(), 0);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn an_apparent_ears_end_before_the_native_end_discards_the_capture() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.false_end.set(true);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_incomplete", .. }))
        ));
        assert_eq!(state.starts.get(), 1);
        assert_eq!(state.written.get(), 0);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn a_selection_field_unrelated_to_tap_deletion_is_not_overwritten() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.selected_track.set(1);
        *state.selected_device.borrow_mut() = json!("7:device:1:d0");
        *state.after_delete.borrow_mut() = Some(Box::new(|state| {
            state.selection_extra.borrow_mut()["sceneRef"] = json!("7:scene:1");
        }));
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert_eq!(state.selection_extra.borrow()["sceneRef"], "7:scene:1");
        assert_eq!(state.selection_writes.get(), 0);
        assert_eq!(state.selected_track.get(), 1);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn cancellation_cannot_cancel_the_cleanup_selection_restore() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.selected_track.set(1);
        *state.selected_device.borrow_mut() = json!("7:device:1:d0");
        let cancel = Signal::new();
        *state.cancel.borrow_mut() = Some(cancel.clone());
        assert!(matches!(live.rendering.hear_in_set(&request, cancel).await, Err(RuntimeError::Hear(HearFailure::Cancelled { .. }))));
        assert_eq!(state.selected_track.get(), 1);
        assert_eq!(*state.selected_device.borrow(), "7:device:1:d0");
        assert_eq!(state.selection_writes.get(), 0);
        assert_eq!(state.written.get(), 0);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn deletion_owns_one_closed_undo_step_and_restores_start_time_last() {
    on_a_paused_clock(async {
        let (live, state, mut request) = bounded(true).await;
        request.from_beat = Some(4.);
        let (s, set) = (state.clone(), live.set.clone());
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
            if name == "live_undo_step_begin" {
                assert!(!set.playing.get());
                assert_eq!(s.position.get(), 12.);
                assert_eq!(s.start_time.get(), 3.);
            }
            Ok(())
        }));
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert_eq!(takes.len(), 1);
        assert_eq!(state.undo_begins.get(), 1);
        assert_eq!(state.undo_ends.get(), 1);
        let writes = state.transport_writes.borrow();
        assert_eq!(&writes[writes.len() - 2..], ["position", "startTime"]);
        restored(&live, &state);
    });
}

#[test]
fn failure_to_open_an_undo_step_leaves_ears_disarmed_without_deleting() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.begin_fails.set(true);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Unsure { code: Some("undo_step_begin_failed"), marked_modified: false, .. }))
        ));
        assert_eq!(live.set.chain("Bass"), ["Synth", "Kumi Ears"]);
        assert_eq!(live.set.asked_for("live_undo"), 0);
        assert_eq!(state.undo_begins.get(), 1);
        assert_eq!(state.undo_ends.get(), 0);
        assert!(!state.undo_open.get());
        assert!(state.disarmed.get() > 0);
        assert!(!live.set.playing.get());
        assert_eq!(state.position.get(), 12.);
        assert_eq!(state.start_time.get(), 3.);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn refused_or_failed_deletion_still_closes_the_native_undo_step() {
    on_a_paused_clock(async {
        for transport_failure in [false, true] {
            let (live, state, request) = bounded(true).await;
            if transport_failure {
                *live.set.on_call.borrow_mut() =
                    Some(Box::new(
                        |name, _| {
                            if name == "live_undo" {
                                Err(RuntimeError::plain("delete transport failed"))
                            } else {
                                Ok(())
                            }
                        },
                    ));
            } else {
                state.delete_fails.set(true);
            }
            assert!(matches!(
                live.rendering.hear_in_set(&request, Signal::new()).await,
                Err(RuntimeError::Hear(HearFailure::Unsure { .. }))
            ));
            assert_eq!(live.set.chain("Bass"), ["Synth", "Kumi Ears"]);
            assert_eq!(state.undo_begins.get(), 1);
            assert_eq!(state.undo_ends.get(), 1);
            assert!(!state.undo_open.get());
            assert!(!live.set.playing.get());
            assert_eq!(state.position.get(), 12.);
            assert_eq!(state.start_time.get(), 3.);
            assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
        }
    });
}

#[test]
fn cancellation_during_deletion_cannot_drop_the_open_undo_step() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let cancel = Signal::new();
        *state.delete_cancel.borrow_mut() = Some(cancel.clone());
        assert!(matches!(live.rendering.hear_in_set(&request, cancel).await, Err(RuntimeError::Hear(HearFailure::Cancelled { .. }))));
        assert_eq!(state.undo_begins.get(), 1);
        assert_eq!(state.undo_ends.get(), 1);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn unconfirmed_undo_step_end_discards_the_capture_even_after_device_deletion() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        state.end_fails.set(true);
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Unsure { code: Some("undo_step_end_failed"), .. }))
        ));
        assert_eq!(state.undo_begins.get(), 1);
        assert_eq!(state.undo_ends.get(), 1);
        assert!(state.undo_open.get(), "an unacknowledged end cannot be assumed closed");
        assert_eq!(live.set.chain("Bass"), ["Synth"]);
        assert!(!live.set.playing.get());
        assert_eq!(state.position.get(), 12.);
        assert_eq!(state.start_time.get(), 3.);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn absent_or_expired_native_deadline_never_receives_a_fresh_capture_allowance() {
    on_a_paused_clock(async {
        for deadline in [None, Some(1)] {
            let (live, state, mut request) = bounded(true).await;
            request.native_deadline_unix_ms = deadline;
            assert!(matches!(
                live.rendering.hear_in_set(&request, Signal::new()).await,
                Err(RuntimeError::Hear(HearFailure::Refused { .. }))
            ));
            assert_eq!(state.applies.get(), 0);
            assert_eq!(state.starts.get(), 0);
            assert_eq!(state.undo_begins.get(), 0);
            restored(&live, &state);
            assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
        }
    });
}

#[test]
fn a_producers_start_time_change_is_not_replaced_by_the_original_start_time() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let s = state.clone();
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_transport_action_preview" && args["action"] == "stop" {
                s.start_time.set(91.);
            }
            Ok(())
        }));
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
        assert_eq!(state.start_time.get(), 91.);
        assert_eq!(live.set.chain("Bass"), ["Synth"]);
        assert!(!state.undo_open.get());
        assert_eq!(state.undo_begins.get(), state.undo_ends.get());
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}
