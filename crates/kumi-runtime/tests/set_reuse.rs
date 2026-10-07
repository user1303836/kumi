//! The observation keeps the Set's devices between turns while Live tells of no change, reads the selected track's
//! (and those Kumi changed) again each turn, and checks the rest with a whole read beside the turns.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{
        contracts::{JsonObject, KernelTool},
        errors::RuntimeError,
        timing,
    },
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        observation::{ObservationHost, ObservedChange, Observer},
        references::Shift,
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// A device, and its chains with their devices (one rack level down).
#[derive(Clone)]
struct Device {
    name: String,
    chains: Vec<(String, Vec<Device>)>,
}
fn device(name: &str) -> Device {
    Device { name: name.into(), chains: vec![] }
}
/// A Live whose tracks hold devices, as discovery lists them; `events` is whether it offers Live's events.
struct Live {
    events: bool,
    /// Whether it answers a subscription with an error.
    refuses: Cell<bool>,
    tracks: RefCell<Vec<(String, Vec<Device>)>>,
    selected: Cell<usize>,
    calls: RefCell<Vec<(String, JsonObject)>>,
    listener: RefCell<Option<Rc<dyn Fn(JsonObject)>>>,
}
impl Live {
    fn new(events: bool) -> Rc<Self> {
        let rack = Device { name: "Rack".into(), chains: vec![("Low".into(), vec![device("Operator")])] };
        Rc::new(Self {
            events,
            refuses: Cell::new(false),
            tracks: RefCell::new(vec![
                ("Bass".into(), vec![rack, device("EQ Eight")]),
                ("Drums".into(), vec![device("Compressor")]),
                ("Pad".into(), vec![device("Utility")]),
            ]),
            selected: Cell::new(1),
            calls: RefCell::new(vec![]),
            listener: RefCell::new(None),
        })
    }
    fn device_rows(&self) -> Vec<Value> {
        let mut rows = vec![];
        for (index, (_, devices)) in self.tracks.borrow().iter().enumerate() {
            for (at, device) in devices.iter().enumerate() {
                let reference = format!("7:device:{index}:{at}");
                let chains: Vec<Value> = device
                    .chains
                    .iter()
                    .enumerate()
                    .map(|(chain, (name, _))| json!({"ref":format!("7:chain:{index}:{at}:{chain}"),"name":name}))
                    .collect();
                rows.push(json!({"ref":reference,"parentRef":format!("7:track:{index}"),"name":device.name,"className":device.name,"chainList":chains}));
                for (chain, (_, inner)) in device.chains.iter().enumerate() {
                    for (place, inner) in inner.iter().enumerate() {
                        rows.push(json!({"ref":format!("7:device:{index}:{at}:{chain}:{place}"),"parentRef":format!("7:chain:{index}:{at}:{chain}"),"name":inner.name,"className":inner.name,"chainList":[]}));
                    }
                }
            }
        }
        rows
    }
    /// Reads of the Set's devices whole, and of one parent's.
    fn whole_reads(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(name, args)| name == "live_discover" && args["kind"] == "device" && !args.contains_key("parent"))
            .count()
    }
    fn parent_reads(&self, parent: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(_, args)| args.get("kind") == Some(&json!("device")) && args.get("parent") == Some(&json!(parent)))
            .count()
    }
    fn subscriptions(&self) -> usize {
        self.calls.borrow().iter().filter(|(name, _)| name == "live_subscribe").count()
    }
    fn structure_changed(&self) {
        let listener = self.listener.borrow().clone().unwrap();
        listener(json!({"type":"structure","what":"tracks"}).as_object().unwrap().clone());
    }
    fn rename(&self, track: usize, at: usize, name: &str) {
        self.tracks.borrow_mut()[track].1[at].name = name.into();
    }
}
fn reply(body: Value) -> CallToolResult {
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
        let mut names = vec!["live_status", "live_discover", "live_song_state"];
        if self.events {
            names.push("live_subscribe");
        }
        Ok(serde_json::from_value(
            json!({"tools":names.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), args.clone()));
        let page = |kind: &str, items: Vec<Value>| {
            let revision = stringify(&json!(items.iter().map(|row| [row["ref"].clone(), row["name"].clone()]).collect::<Vec<_>>()));
            reply(json!({"epoch":7,"kind":kind,"items":items,"revision":revision,"truncated":false}))
        };
        Ok(match (name, args.get("kind").and_then(Value::as_str)) {
            ("live_status", _) => reply(
                json!({"connected":true,"adapter":"remote-script","provenance":"fake-live","epoch":7,"environment":{"liveVersion":"12.0"}}),
            ),
            ("live_song_state", _) => reply(json!({"signatureNumerator":4,"signatureDenominator":4,"sessionRecord":false,"swingAmount":0})),
            ("live_subscribe", _) if self.refuses.get() => {
                serde_json::from_value(json!({"content":[{"type":"text","text":"Live isn't ready to tell of changes"}],"isError":true}))
                    .unwrap()
            }
            ("live_subscribe", _) => reply(json!({"subscribed":args["types"]})),
            ("live_discover", Some("set")) => page(
                "set",
                vec![
                    json!({"ref":"7:set:0","objectIdentity":"song","name":"Set","tempo":120,"playing":false,"position":0,"loop":{"start":0,"length":16},"filePath":""}),
                ],
            ),
            ("live_discover", Some("track")) => page(
                "track",
                self.tracks
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(index, (name, _))| json!({"ref":format!("7:track:{index}"),"name":name,"kind":"regular","mediaKind":"midi"}))
                    .collect(),
            ),
            ("live_discover", Some("device")) => {
                let rows = self.device_rows();
                match args.get("parent").and_then(Value::as_str) {
                    Some(parent) => page("device", rows.into_iter().filter(|row| row["parentRef"] == parent).collect()),
                    None => page("device", rows),
                }
            }
            ("live_discover", Some("selection")) => {
                page("selection", vec![json!({"selectedTrackRef":format!("7:track:{}", self.selected.get())})])
            }
            other => panic!("not a call here: {other:?}"),
        })
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn has_on_live_event(&self) -> bool {
        self.events
    }
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        *self.listener.borrow_mut() = Some(listener);
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
#[async_trait(?Send)]
impl ObservationHost for Live {
    fn reset_turn(&self, _: bool) {}
    fn changes(&self) -> Vec<ObservedChange> {
        vec![]
    }
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>> {
        vec![]
    }
    async fn restore_after_crash(&self, _: &str, _: Option<&str>, _: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
    }
}

struct Session {
    live: Rc<Live>,
    /// The session's clock, in seconds after noon.
    clock: Rc<Cell<i64>>,
    connection: Rc<LiveConnection>,
    remember: Rc<Remember>,
    observer: Observer,
}
async fn session(events: bool) -> Session {
    let live = Live::new(events);
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    let endpoint = live.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let clock = Rc::new(Cell::new(0));
    let now = clock.clone();
    options.now = Some(Rc::new(move || {
        chrono::DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z").unwrap().with_timezone(&chrono::Utc)
            + chrono::Duration::seconds(now.get())
    }));
    options.reconnect_interval_ms = Some(3600000);
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.unwrap();
    let remember = Remember::new(connection.clone(), None, None);
    let observer = Observer::new(connection.clone(), remember.clone());
    Session { live, clock, connection, remember, observer }
}
impl Session {
    /// One turn's look at the Set: the tracks as its context shows them (with their devices), and its timing.
    async fn turn(&self) -> (Value, timing::TurnTiming) {
        let recorder = timing::begin();
        let observed = self.observer.observe(self.live.as_ref(), Signal::new(), None).await;
        let timing = recorder.finish();
        let Ok(observed) = observed else { panic!("the turn failed") };
        let context: Value = serde_json::from_str(&observed.context).unwrap();
        (context["tracks"].clone(), timing)
    }
    async fn close(self) {
        self.remember.cancel_timer();
        self.connection.close().await.unwrap();
    }
}
fn names(tracks: &Value) -> Vec<String> {
    let mut names = vec![];
    fn walk(devices: &Value, names: &mut Vec<String>) {
        for device in devices.as_array().into_iter().flatten() {
            names.push(device["name"].as_str().unwrap().to_owned());
            for chain in device["chains"].as_array().into_iter().flatten() {
                walk(&chain["devices"], names);
            }
        }
    }
    for track in tracks.as_array().unwrap() {
        walk(&track["devices"], &mut names);
    }
    names
}

#[tokio::test(flavor = "current_thread")]
async fn a_turn_reuses_the_devices_while_live_tells_of_no_change_and_reads_the_selected_tracks_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            let (first, timing) = session.turn().await;
            assert_eq!(names(&first), ["Rack", "Operator", "EQ Eight", "Compressor", "Utility"]);
            assert_eq!((session.live.whole_reads(), timing.set_reused), (1, Some(false)));
            // Nothing changed: the kept devices, with the selected track's read again, show the same.
            let (second, timing) = session.turn().await;
            assert_eq!((second.clone(), session.live.whole_reads(), timing.set_reused), (first, 1, Some(true)));
            assert_eq!(session.live.parent_reads("7:track:1"), 1, "the selected track's devices");
            assert!(timing.live_bytes > 0 && timing.live_bytes < 2_000, "{}", timing.live_bytes);
            // The look is the whole of this turn: its own count is the turn's (#191).
            assert_eq!((timing.look_requests, timing.look_bytes), (timing.live_requests, timing.live_bytes));
            assert!(timing.look_ms.is_some());
            // The selected track's device renamed (Live tells of no rename): the next turn shows it.
            session.live.rename(1, 0, "Glue");
            assert_eq!(names(&session.turn().await.0)[3], "Glue");
            // Selecting the bass reads its rack a level down as well.
            session.live.selected.set(0);
            session.live.rename(0, 0, "Bass Rack");
            assert_eq!(names(&session.turn().await.0)[..2], ["Bass Rack", "Operator"]);
            assert_eq!((session.live.parent_reads("7:track:0"), session.live.parent_reads("7:chain:0:0:0")), (1, 1));
            // Another track's device renamed is what events miss: the kept name stays until the backstop.
            session.live.rename(2, 0, "Gain");
            assert_eq!(names(&session.turn().await.0)[4], "Utility");
            assert_eq!(session.live.whole_reads(), 1);
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_structure_change_or_another_track_list_reads_the_devices_whole_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.turn().await;
            session.live.structure_changed();
            let (_, timing) = session.turn().await;
            assert_eq!((session.live.whole_reads(), timing.set_reused), (2, Some(false)));
            // A track added with no word from Live: its refs renumber the devices, read whole in the same turn.
            session.live.tracks.borrow_mut().insert(0, ("Lead".into(), vec![device("Wavetable")]));
            let (tracks, timing) = session.turn().await;
            assert_eq!((session.live.whole_reads(), timing.set_reused), (3, Some(false)));
            assert_eq!(names(&tracks)[0], "Wavetable");
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_backstop_reads_them_whole_beside_the_turns_and_counts_what_events_missed() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.turn().await;
            session.live.rename(2, 0, "Gain");
            // Ten turns reuse the kept devices; after the tenth, the backstop reads them whole beside the turns.
            for _ in 0..10 {
                assert_eq!(names(&session.turn().await.0)[4], "Utility");
            }
            assert_eq!(session.live.whole_reads(), 1);
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert_eq!(session.live.whole_reads(), 2, "the backstop's read");
            // The next turn shows what it found, and its timing says how many rows events missed.
            let (tracks, timing) = session.turn().await;
            assert_eq!((names(&tracks)[4].as_str(), timing.set_reused, timing.set_drift), ("Gain", Some(true), Some(1)));
            assert_eq!(session.turn().await.1.set_drift, None, "said once");
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kumis_own_device_changes_are_read_again_before_and_after_and_anything_else_reads_all() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.turn().await;
            let observer = &session.observer;
            // A change naming a device on the pad, and one loading a device onto the drums.
            let named = |input: Value, tracks_too| observer.devices_named(input.as_object().unwrap(), tracks_too);
            assert_eq!(
                named(json!({"deviceRef":"7:device:2:0","value":0.5}), false),
                (vec!["7:track:2".into()], vec!["7:device:2:0".into()])
            );
            assert_eq!(named(json!({"trackRef":"7:track:1"}), false), (vec![], vec![]));
            assert_eq!(named(json!({"trackRef":"7:track:1"}), true), (vec!["7:track:1".into()], vec![]));
            // Before a change acts on the pad's devices they're read again, once a turn.
            let pad = vec!["7:track:2".to_owned()];
            observer.refresh_devices(&pad, &[], Signal::new()).await.unwrap();
            observer.refresh_devices(&pad, &[], Signal::new()).await.unwrap();
            assert_eq!(session.live.parent_reads("7:track:2"), 1);
            // Kumi renames the pad's device: the next turn reads the pad again and shows it.
            session.live.rename(2, 0, "Gain");
            observer.devices_changed(&pad);
            let (tracks, timing) = session.turn().await;
            assert_eq!((names(&tracks)[4].as_str(), timing.set_reused, session.live.parent_reads("7:track:2")), ("Gain", Some(true), 2));
            // Python run in Live could have changed anything: the next turn reads every device.
            observer.forget_devices();
            let (_, timing) = session.turn().await;
            assert_eq!((session.live.whole_reads(), timing.set_reused), (2, Some(false)));
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn without_lives_events_every_turn_reads_the_devices_whole() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(false).await;
            for _ in 0..3 {
                session.turn().await;
            }
            assert_eq!(session.live.whole_reads(), 3);
            // Nothing kept, so a change reads nothing more first.
            session.observer.refresh_devices(&["7:track:2".to_owned()], &["7:device:2:0".to_owned()], Signal::new()).await.unwrap();
            assert_eq!(session.live.parent_reads("7:track:2"), 0);
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_kept_devices_serve_for_a_minute_after_a_whole_read_then_are_read_whole_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.turn().await;
            session.clock.set(59);
            assert_eq!((session.turn().await.1.set_reused, session.live.whole_reads()), (Some(true), 1));
            // A quiet stretch: Live tells of no rack edited, so older devices are read whole again.
            session.clock.set(61);
            assert_eq!((session.turn().await.1.set_reused, session.live.whole_reads()), (Some(false), 2));
            session.clock.set(100);
            assert_eq!(session.turn().await.1.set_reused, Some(true), "a minute from the new read");
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_change_naming_a_device_that_isnt_the_one_the_turn_showed_is_refused() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.turn().await;
            let (observer, live) = (&session.observer, &session.live);
            let check = |track: &str, named: &str| {
                let (track, named) = (vec![track.to_owned()], vec![named.to_owned()]);
                async move { observer.refresh_devices(&track, &named, Signal::new()).await }
            };
            // The pad's Utility swapped for Echo in place, with no word from Live: its ref now names Echo.
            live.rename(2, 0, "Echo");
            let refused = check("7:track:2", "7:device:2:0").await.unwrap_err();
            assert!(refused.starts_with("Utility is now Echo") && refused.contains("nothing was changed"), "{refused}");
            // A device just as the turn showed it is fine.
            assert_eq!(check("7:track:0", "7:device:0:1").await, Ok(()));
            // On the next turn, a chain renamed or a device gone isn't.
            session.turn().await;
            live.tracks.borrow_mut()[0].1[0].chains[0].0 = "Mid".into();
            assert!(check("7:track:0", "7:chain:0:0:0").await.unwrap_err().starts_with("Low is now Mid"));
            live.tracks.borrow_mut()[1].1.clear();
            assert!(check("7:track:1", "7:device:1:0").await.unwrap_err().starts_with("Compressor isn't on the track any more"));
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_track_added_where_main_was_isnt_held_to_mains_devices_and_kumis_own_device_changes_arent_held_to_the_look() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            // Main, after the three tracks, with its Limiter: the look shows it at 7:device:3:0.
            session.live.tracks.borrow_mut().push(("Main".into(), vec![device("Limiter")]));
            session.turn().await;
            let (observer, live) = (&session.observer, &session.live);
            let check = |track: &str, named: &str| {
                let (track, named) = (vec![track.to_owned()], vec![named.to_owned()]);
                async move { observer.refresh_devices(&track, &named, Signal::new()).await }
            };
            // One plan adds a track where Main was (Main moves to 4) and loads a Drum Rack on it: the step naming the
            // Drum Rack isn't checked against Main's Limiter, which the look showed at that place (#253).
            live.tracks.borrow_mut().insert(3, ("Beat".into(), vec![device("Drum Rack")]));
            observer.shifted(&Shift { tracks_made: vec![3], ..Default::default() });
            assert_eq!(check("7:track:3", "7:device:3:0").await, Ok(()));
            // Main's Limiter moved with its track, and is still checked there.
            live.tracks.borrow_mut()[4].1[0].name = "Glue".into();
            assert!(check("7:track:4", "7:device:4:0").await.unwrap_err().starts_with("Limiter is now Glue"));
            // A refusal lets the next change read the track again: the Limiter is back, so it goes through.
            live.tracks.borrow_mut()[4].1[0].name = "Limiter".into();
            assert_eq!(check("7:track:4", "7:device:4:0").await, Ok(()));
            // Kumi loads a device before the pad's Utility: its own answer told the model what's at 7:device:2:0 now,
            // so that place isn't held to the Utility the look showed there.
            live.tracks.borrow_mut()[2].1.insert(0, device("Auto Filter"));
            observer.devices_changed(&["7:track:2".to_owned()]);
            assert_eq!(check("7:track:2", "7:device:2:0").await, Ok(()));
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_subscription_live_refuses_is_asked_again_and_nothing_is_reused_meanwhile() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.live.refuses.set(true);
            for _ in 0..3 {
                assert_eq!(session.turn().await.1.set_reused, Some(false));
            }
            assert_eq!((session.live.whole_reads(), session.live.subscriptions()), (3, 3), "asked again each turn");
            // Once Live takes it, the devices are kept.
            session.live.refuses.set(false);
            session.turn().await;
            assert_eq!(session.turn().await.1.set_reused, Some(true));
            session.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_device_ref_from_an_earlier_turn_is_checked_as_that_turn_showed_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let session = session(true).await;
            session.live.selected.set(0);
            session.turn().await;
            // The drums' Compressor is deleted (Live tells of it): the next turn reads every device, and shows none there.
            session.live.tracks.borrow_mut()[1].1.clear();
            session.live.structure_changed();
            session.turn().await;
            // A change still naming the Compressor's ref from the turn before is refused, not sent to Live.
            let refused = session.observer.refresh_devices(&["7:track:1".to_owned()], &["7:device:1:0".to_owned()], Signal::new()).await;
            assert!(refused.unwrap_err().starts_with("Compressor isn't on the track any more"));
            session.close().await;
        })
        .await;
}
