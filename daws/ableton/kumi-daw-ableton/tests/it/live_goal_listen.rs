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
    looped: Cell<bool>,
    tempo: Cell<f64>,
    recording: Cell<bool>,
    back_to_arrangement: Cell<bool>,
    advancing: Cell<bool>,
    ignore_restore: Cell<bool>,
    ignore_stop: Cell<bool>,
    selected_device: Cell<&'static str>,
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
    raw_frames: Cell<usize>,
}
impl State {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            identity: RefCell::new("song".into()),
            position: Cell::new(12.),
            looped: Cell::new(true),
            epoch: Cell::new(7.),
            transactions: RefCell::new(std::collections::HashMap::new()),
            tempo: Cell::new(120.),
            recording: Cell::new(false),
            back_to_arrangement: Cell::new(false),
            advancing: Cell::new(true),
            ignore_restore: Cell::new(false),
            ignore_stop: Cell::new(false),
            selected_device: Cell::new("7:device:0:d0"),
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
            raw_frames: Cell::new(16_000),
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
    pub(super) fn discover(&self, set: &Set, args: &JsonObject) -> Option<Vec<Value>> {
        let tracks = set.tracks.borrow();
        let kind = text(&args["kind"]);
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
                "selectedTrackRef":"7:track:0","selectedDeviceRef":self.selected_device.get()})],
            "session-playback" => vec![json!({"ref":"7:session_playback:0","transport":{
                "playing":set.playing.get(),"arrangementRecord":self.recording.get(),"sessionRecord":false,
                "position":self.position.get(),"loop":{"enabled":self.looped.get(),"start":4,"length":16},
                "punchIn":false,"punchOut":false},"firedTargets":[],"playingTargets":[]})],
            "device" => {
                let parent = text(&args["parent"]);
                let at = places(parent)[0];
                tracks[at].devices.iter().enumerate().map(|(index, device)| {
                    json!({"ref":format!("7:device:{at}:d{index}"),"parentRef":parent,"name":device.name,
                        "className":device.class,"classDisplayName":device.class,"enabled":true,"objectIdentity":device.identity})
                }).collect()
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
        let mut answer = json!({"transactionId":transaction,"state":"applied"});
        match tool {
            "live_transport" => {
                if let Some(position) = args.get("position").and_then(Value::as_f64) {
                    if !(position == 12. && self.ignore_restore.get()) {
                        self.position.set(position);
                    }
                }
                if let Some(looped) = args.get("loopEnabled").and_then(Value::as_bool) {
                    self.looped.set(looped);
                }
            }
            "live_transport_action" => match args.get("action").map(text) {
                Some("continue") => {
                    set.playing.set(true);
                    self.starts.set(self.starts.get() + 1);
                }
                Some("stop") => {
                    if !self.ignore_stop.get() {
                        set.playing.set(false);
                    }
                    self.stops.set(self.stops.get() + 1);
                }
                other => panic!("a restricted capture cannot perform {other:?}"),
            },
            "live_browser_load" => {
                assert_eq!(args["itemId"], EARS_ITEM);
                let at = places(text(&args["trackRef"]))[0];
                let device = Device::new("Kumi Ears");
                set.undos.borrow_mut().insert(transaction.into(), Undo::RemoveDevice(device.identity));
                let mut tracks = set.tracks.borrow_mut();
                tracks[at].devices.push(device);
                answer["deviceRef"] = json!(format!("7:device:{at}:d{}", tracks[at].devices.len() - 1));
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
        assert!(!quiet, "a listen cannot mute the producer's track");
        self.state.arms.borrow_mut().push(seconds);
        self.state.armed_locations.borrow_mut().push(tap.path.clone());
        self.state.armed_from.set(self.state.position.get());
        Ok(Armed { beats: self.state.position.get(), running: self.set.playing.get(), sample_rate: 8000. })
    }
    async fn write(&self, _: &Tap, file: &str, _: Option<Signal>) -> Result<Written, EarsError> {
        self.state.written.set(self.state.written.get() + 1);
        let mut raw = std::fs::File::create(file).unwrap();
        // Four channels, 8 kHz: a known two-second tone, sample clock and actual beat positions.
        for frame in 0..self.state.raw_frames.get() {
            let beat = self.state.armed_from.get() + frame as f64 / 4000.;
            let sample = (frame as f64 * std::f64::consts::TAU * 200. / 8000.).sin() as f32 * 0.2;
            for value in [sample, sample, 1. + beat.fract() as f32, beat as f32] {
                raw.write_all(&value.to_le_bytes()).unwrap();
            }
        }
        Ok(Written { file: file.into(), sample_rate: 8000., channels: 4, beats: self.state.armed_from.get(), running: false })
    }
    fn stop(&self, _: &Tap) {}
    async fn ping(&self, tap: &Tap, _: Option<Signal>) -> Option<Tap> {
        Some(tap.clone())
    }
    async fn transport(&self, _: &Tap, _: Option<Signal>) -> Result<Option<Transport>, EarsError> {
        if let Some(cancel) = self.state.cancel.borrow_mut().take() {
            cancel.cancel();
        }
        if self.state.switch_set.get() {
            *self.state.identity.borrow_mut() = "another-song".into();
            self.state.position.set(77.);
        } else if self.state.advancing.get() {
            self.state.position.set(self.state.position.get() + 4.);
        }
        Ok(Some(Transport { beats: self.state.position.get(), running: self.set.playing.get() }))
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
        options.policy = HostPolicy::plugin(scratch, root.join("quarantine"));
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
    request.approval = Some(kumi_types::contracts::HearApproval { seconds: 2., deadline_seconds: 300 });
    (live, state, request)
}

fn restored(live: &Live, state: &State) {
    assert_eq!(live.set.chain("Bass"), ["Synth"]);
    assert!(!live.set.playing.get());
    assert_eq!(state.position.get(), 12.);
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
        assert_eq!(live.set.next.get(), 0, "no mutation preview or apply during a listen preview");
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn capture_trims_the_approved_window_and_verifies_restoration() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let takes = live.rendering.hear_in_set(&request, Signal::new()).await.unwrap().unwrap();
        assert_eq!(takes.len(), 1);
        assert_eq!(takes[0].start, 0.);
        assert!((takes[0].seconds.unwrap() - 2.).abs() < 0.001);
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
                assert!(matches!(name, "live_browser_load_apply" | "live_transport_apply" | "live_transport_action_apply"),
                    "forbidden mutation: {call}");
            }
            if name == "live_transport_preview" {
                assert!(call["args"].as_object().unwrap().keys().all(|key| matches!(key.as_str(), "position" | "loopEnabled")),
                    "no loop range, punch, tempo or recording-mode mutation: {call}");
            }
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
            assert_eq!(live.set.next.get(), 0, "{case}");
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
        assert_eq!(live.set.next.get(), 0);
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
        assert!(matches!(live.rendering.hear_in_set(&request, cancel).await, Err(RuntimeError::Aborted)));
        restored(&live, &state);
        assert_eq!(state.closed.get(), 1);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());

        let (live, state, mut request) = bounded(true).await;
        state.ignore_restore.set(true);
        // End the simulated playback away from the original cursor, so an ignored restore is observable.
        request.from_beat = Some(4.);
        assert!(matches!(live.rendering.hear_in_set(&request, Signal::new()).await, Err(RuntimeError::Hear(HearFailure::Unsure { .. }))));
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
            assert!(matches!(
                live.rendering.hear_in_set(&request, Signal::new()).await,
                Err(RuntimeError::Hear(HearFailure::Refused { code: "ears_missing", .. }))
            ), "{version:?}");
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
        assert_eq!(live.set.next.get(), 0, "no native mutation was even previewed");
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
        assert_eq!(live.set.next.get(), 0);
    });
}

#[test]
fn recording_enabled_during_the_start_preview_is_never_started_by_capture() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let s = state.clone();
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, args| {
            if name == "live_transport_action_preview" && args["action"] == "continue" {
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
            assert_eq!(live.set.next.get(), 0);
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
        assert_eq!(live.set.next.get(), 0);
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
        assert_eq!(live.set.next.get(), 0);
        for call in live.set.asked.borrow().iter() {
            let name = call["name"].as_str().unwrap();
            assert!(!name.starts_with("live_browser_load") && !name.ends_with("_apply") && name != "live_run_python",
                "no device load, setup or write: {call}");
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
            assert_eq!(live.set.next.get(), 0);
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
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Unsure { .. }))
        ));
        assert!(state.stops.get() > 0, "the bridge acknowledged a stop");
        assert!(live.set.playing.get(), "the independent transport read still sees playback");
        assert_eq!(state.written.get(), 1, "discard even audio already written before the restoration read");
        assert_eq!(live.set.chain("Bass"), ["Synth"]);
        assert_eq!(state.closed.get(), 1);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}

#[test]
fn a_changed_selected_device_after_cleanup_is_unsure_and_discards_audio() {
    on_a_paused_clock(async {
        let (live, state, request) = bounded(true).await;
        let s = state.clone();
        *live.set.on_call.borrow_mut() = Some(Box::new(move |name, _| {
            if name == "live_undo" {
                s.selected_device.set("7:device:1:d0");
            }
            Ok(())
        }));
        assert!(matches!(
            live.rendering.hear_in_set(&request, Signal::new()).await,
            Err(RuntimeError::Hear(HearFailure::Unsure { .. }))
        ));
        assert_eq!(state.selected_device.get(), "7:device:1:d0", "do not overwrite the producer's selection");
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
            ("set", "/objectIdentity"), ("set", "/tempo"),
            ("selection", "/selectedTrackRef"), ("selection", "/selectedDeviceRef"),
            ("track", "/objectIdentity"), ("track", "/armed"), ("track", "/monitoringState"),
            ("main-track", "/objectIdentity"), ("main-track", "/armed"), ("main-track", "/monitoringState"),
            ("device", "/objectIdentity"),
            ("session-playback", "/transport/playing"), ("session-playback", "/transport/arrangementRecord"),
            ("session-playback", "/transport/sessionRecord"), ("session-playback", "/transport/position"),
            ("session-playback", "/transport/loop/enabled"), ("session-playback", "/transport/loop/start"),
            ("session-playback", "/transport/loop/length"), ("session-playback", "/transport/punchIn"),
            ("session-playback", "/transport/punchOut"), ("session-playback", "/firedTargets"), ("session-playback", "/playingTargets"),
            ("song", "/backToArranger"), ("song", "/sessionRecord"), ("song", "/sessionAutomationRecord"),
            ("song", "/overdub"), ("song", "/arrangementOverdub"), ("song", "/reEnableAutomationEnabled"),
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
                    assert_eq!(live.set.next.get(), 0);
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
            Err(RuntimeError::Hear(HearFailure::Refused { code: "capture_incomplete", .. }))
        ));
        assert_eq!(state.written.get(), 1);
        assert_eq!(state.closed.get(), 1);
        restored(&live, &state);
        assert!(std::fs::read_dir(request.scratch_dir.unwrap()).unwrap().next().is_none());
    });
}
