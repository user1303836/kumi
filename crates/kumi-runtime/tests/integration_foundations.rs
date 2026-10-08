use async_trait::async_trait;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{
        contracts::*,
        errors::{FailureKind, KumiError, RuntimeError},
    },
    integrations::{
        ableton::{
            bridge_version::at_least,
            fold::{fold_tracks, track_line},
        },
        fallback::with_fallback,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("support/integration-foundations-oracle.json")).unwrap()
}
#[test]
fn folded_tracks_and_version_gates_match_complete_source_outputs() {
    let data = fixture();
    for case in data["fold"].as_array().unwrap() {
        let count = case["count"].as_u64().unwrap();
        let tracks=(1..=count).map(|i|{
        let mut track=json!({"ref":format!("track:{i}"),"name":format!("Track {i}"),"type":"midi","devices":(["Operator","EQ Eight","Reverb"].iter().enumerate().map(|(j,name)|json!({"ref":format!("device:{j}"),"name":name})).collect::<Vec<_>>())}).as_object().unwrap().clone();if i%4==2{track.insert("group".into(),json!("track:1"));}track
    }).collect::<Vec<_>>();
        let focused = format!("track:{}", case["focused"]);
        let folded = fold_tracks(
            &tracks,
            |t| t.get("ref").and_then(Value::as_str) == Some(&focused),
            Some(case["budget"].as_u64().unwrap() as usize),
        );
        assert_eq!(hex::encode(Sha256::digest(stringify(&serde_json::to_value(folded).unwrap()).as_bytes())), case["hash"], "{case}");
    }
    for case in data["lines"].as_array().unwrap() {
        assert_eq!(track_line(case["track"].as_object().unwrap(), case["names"].as_bool().unwrap()), case["text"]);
    }
    for case in data["versions"].as_array().unwrap() {
        assert_eq!(at_least(case["version"].as_str(), case["minimum"].as_str().unwrap()), case["expected"].as_bool().unwrap(), "{case}");
    }
}
struct TestIntegration {
    name: &'static str,
    error: Option<RuntimeError>,
    log: Rc<RefCell<Vec<String>>>,
    hints: RefCell<Option<ObserveHints>>,
    closed: Cell<bool>,
}
#[async_trait(?Send)]
impl Integration for TestIntegration {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        self.log.borrow_mut().push(format!("{}:start", self.name));
        self.error.clone().map_or(Ok(()), Err)
    }
    async fn observe(&self, _: Signal, hints: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        *self.hints.borrow_mut() = hints;
        Ok(Observation {
            key: self.name.into(),
            revision: None,
            label: self.name.into(),
            context: "{}".into(),
            instructions: String::new(),
            tools: vec![],
            project: None,
            set: None,
            tracks: None,
            saved_at: None,
        })
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.closed.set(true);
        self.log.borrow_mut().push(format!("{}:close", self.name));
        Ok(())
    }
    fn has_device_tree(&self) -> bool {
        self.name == "live"
    }
    async fn device_tree(&self, track_ref: &str, _: Signal) -> Result<Option<DeviceTree>, RuntimeError> {
        Ok(Some(DeviceTree { track_ref: track_ref.into(), devices: vec![] }))
    }
}
fn integration(name: &'static str, error: Option<RuntimeError>, log: Rc<RefCell<Vec<String>>>) -> Rc<TestIntegration> {
    Rc::new(TestIntegration { name, error, log, hints: RefCell::new(None), closed: Cell::new(false) })
}
#[tokio::test(flavor = "current_thread")]
async fn live_setup_failure_closes_primary_then_chat_and_other_errors_propagate() {
    let log = Rc::new(RefCell::new(vec![]));
    let said = Rc::new(RefCell::new(vec![]));
    let notes = said.clone();
    let events = log.clone();
    let primary =
        integration("live", Some(KumiError::new(FailureKind::Live, "Quit Live, then run npm run kumi -- bridge").into()), log.clone());
    let fallen = with_fallback(
        primary.clone(),
        Rc::new(move || integration("chat", None, events.clone())),
        Rc::new(move |s| notes.borrow_mut().push(s)),
    );
    fallen.start(Signal::new()).await.unwrap();
    assert_eq!(*log.borrow(), ["live:start", "live:close", "chat:start"]);
    assert_eq!(*said.borrow(), ["Quit Live, then run npm run kumi -- bridge"]);
    assert_eq!(fallen.observe(Signal::new(), None).await.unwrap().key, "chat");
    assert!(!fallen.stop_live(Signal::new()).await.unwrap());
    assert!(fallen.undo(None, Signal::new()).await.unwrap_err().message().contains("isn't connected to Live"));
    assert!(fallen.device_tree("x", Signal::new()).await.unwrap().is_none());
    assert!(fallen.has_goal());
    let request: AuditionRequest = serde_json::from_value(json!({"candidates":[]})).unwrap();
    assert!(fallen.goal(&request, Signal::new()).await.unwrap().is_err());
    assert!(fallen.audition(&request, Signal::new()).await.unwrap().is_err());
    assert!(fallen.hear(&HearRequest::default(), Signal::new()).await.unwrap().is_err());
    fallen.close().await.unwrap();
    assert_eq!(log.borrow().last().unwrap(), "chat:close");
    for (error, cancel) in [
        (RuntimeError::plain("socket closed"), false),
        (KumiError::new(FailureKind::Auth, "login").into(), false),
        (KumiError::new(FailureKind::Live, "x").into(), true),
    ] {
        let primary = integration("live", Some(error.clone()), Rc::new(RefCell::new(vec![])));
        let wrapped = with_fallback(primary, Rc::new(|| panic!("no fallback")), Rc::new(|_| panic!("no notice")));
        let signal = Signal::new();
        if cancel {
            signal.cancel();
        }
        assert_eq!(wrapped.start(signal).await.unwrap_err(), error);
    }
}
#[tokio::test(flavor = "current_thread")]
async fn connected_primary_forwards_focus_hints_and_optional_views() {
    let primary = integration("live", None, Rc::new(RefCell::new(vec![])));
    let wrapped = with_fallback(primary.clone(), Rc::new(|| panic!("no fallback")), Rc::new(|_| panic!("no notice")));
    wrapped.start(Signal::new()).await.unwrap();
    let hints = ObserveHints {
        pinned: Some(
            serde_json::from_value(
                json!({"trackRef":"1:track:0","ref":"1:device:0:0","node":"device","name":"Saturator","trail":[],"siblings":[]}),
            )
            .unwrap(),
        ),
        continuing: None,
    };
    wrapped.observe(Signal::new(), Some(hints.clone())).await.unwrap();
    assert_eq!(*primary.hints.borrow(), Some(hints));
    assert_eq!(wrapped.device_tree("1:track:0", Signal::new()).await.unwrap().unwrap().track_ref, "1:track:0");
    wrapped.close().await.unwrap();
    assert!(primary.closed.get());
}
