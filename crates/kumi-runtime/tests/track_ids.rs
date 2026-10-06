//! Kumi's own ids for a Set's tracks (`kumi.track`): who gets a new one, and one write for all of them.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        project::{create_project_store, ProjectStore},
        track_ids::{gaps, ids_to_write, track_id, Keepers, Seen, TrackIds, Write, TRACK_KEY},
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

fn seen(reference: &str, identity: &str, id: Option<&str>) -> Seen {
    Seen { reference: reference.into(), identity: identity.into(), id: id.map(str::to_owned) }
}
const A: &str = "01J9ZQ3V6M8K2D7X4N5P0R1S2A";
const B: &str = "01J9ZQ3V6M8K2D7X4N5P0R1S2B";
fn counter() -> impl FnMut() -> String {
    let mut n = 0;
    move || {
        n += 1;
        format!("01J9ZQ3V6M8K2D7X4N5P0R1N{n:02}")
    }
}

#[test]
fn a_track_without_an_id_and_each_copy_get_one_and_the_original_keeps_its_own() {
    assert!(track_id(A) && !track_id("not an id") && !track_id(&A.to_lowercase()));
    // Every track with an id of its own: nothing to write.
    let tracks = [seen("t0", "live:1", Some(A)), seen("t1", "live:2", Some(B))];
    assert!(ids_to_write(&tracks, &HashMap::new(), counter()).is_empty());
    // None kept, or text that isn't an id of Kumi's: a new one, written only while it still holds that.
    let tracks = [seen("t0", "live:1", None), seen("t1", "live:2", Some("x")), seen("t2", "live:3", Some(A))];
    let writes = ids_to_write(&tracks, &HashMap::new(), counter());
    assert_eq!(
        writes,
        [
            Write { reference: "t0".into(), identity: "live:1".into(), prior: None, id: "01J9ZQ3V6M8K2D7X4N5P0R1N01".into() },
            Write { reference: "t1".into(), identity: "live:2".into(), prior: Some("x".into()), id: "01J9ZQ3V6M8K2D7X4N5P0R1N02".into() },
        ]
    );
    // A duplicate carries its original's id: the first keeps it (Live places the copy right after it)...
    let copies = [seen("t0", "live:1", Some(A)), seen("t1", "live:2", Some(A))];
    assert_eq!(ids_to_write(&copies, &HashMap::new(), counter()).iter().map(|w| w.reference.as_str()).collect::<Vec<_>>(), ["t1"]);
    // ...unless the other was seen with it before, as when the copy was moved above the original.
    let known = HashMap::from([(A.to_owned(), "live:2".to_owned())]);
    assert_eq!(ids_to_write(&copies, &known, counter()).iter().map(|w| w.reference.as_str()).collect::<Vec<_>>(), ["t0"]);
}

#[test]
fn rows_read_with_their_ids_show_a_gap_when_one_is_missing_or_shared() {
    let rows = |ids: &[Option<&str>]| -> Vec<JsonObject> {
        ids.iter().enumerate().map(|(n, id)| json!({"ref":format!("t{n}"),"kumiTrack":id}).as_object().unwrap().clone()).collect()
    };
    assert!(gaps(&rows(&[Some(A), Some(B)])).is_empty());
    assert_eq!(gaps(&rows(&[Some(A), None])), ["t1"]);
    assert_eq!(gaps(&rows(&[Some(A), Some(A), Some(B)])), ["t0", "t1"]);
    assert!(gaps(&[json!({"ref":"t"}).as_object().unwrap().clone()]).is_empty(), "a Live that doesn't report ids shows no gap");
}

/// A bridge whose tracks keep text, as Live's do; `reports` is whether its rows say each track's id.
struct Bridge {
    reports: bool,
    /// Tracks (by ref) whose data won't take an id.
    refuse: RefCell<Vec<String>>,
    tracks: RefCell<Vec<(String, String, Option<String>)>>,
    pending: RefCell<Option<Value>>,
    calls: RefCell<Vec<(String, Value)>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let tools = ["live_discover", "live_data_preview", "live_data_apply"];
        Ok(serde_json::from_value(
            json!({"tools":tools.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push((name.into(), Value::Object(args.clone())));
        let body = match name {
            "live_discover" => {
                let kind = args["kind"].as_str().unwrap();
                let items: Vec<Value> = self
                    .tracks
                    .borrow()
                    .iter()
                    .filter(|(reference, ..)| match kind {
                        "return-track" => reference.starts_with("1:return"),
                        "main-track" => reference.starts_with("1:main"),
                        _ => reference.starts_with("1:track"),
                    })
                    .take(args.get("limit").and_then(Value::as_u64).unwrap_or(1000) as usize)
                    .map(|(reference, identity, id)| {
                        let mut row = json!({"ref":reference,"objectIdentity":identity});
                        if self.reports {
                            row["kumiTrack"] = json!(id);
                        }
                        row
                    })
                    .collect();
                json!({"kind":kind,"epoch":1,"revision":"rev-1","items":items})
            }
            "live_data_preview" => {
                assert_eq!(args["key"], TRACK_KEY);
                *self.pending.borrow_mut() = Some(Value::Object(args));
                json!({"transactionId":"data_1","places":1})
            }
            "live_data_apply" => {
                let asked = self.pending.borrow_mut().take().unwrap();
                let places: Vec<Value> =
                    std::iter::once(asked.clone()).chain(asked["entries"].as_array().cloned().unwrap_or_default()).collect();
                let mut tracks = self.tracks.borrow_mut();
                // Every track checked first, then each one written, as the host does; one that won't take its id
                // is left.
                for place in &places {
                    let track = tracks.iter().find(|(r, ..)| *r == place["trackRef"]).unwrap();
                    assert_eq!((json!(track.1), json!(track.2)), (place["expectedIdentity"].clone(), place["expectedValue"].clone()));
                }
                let refused = self.refuse.borrow();
                for place in places.iter().filter(|p| !refused.iter().any(|r| *r == p["trackRef"])) {
                    tracks.iter_mut().find(|(r, ..)| *r == place["trackRef"]).unwrap().2 = place["value"].as_str().map(str::to_owned);
                }
                if places.iter().any(|p| refused.iter().any(|r| *r == p["trackRef"])) {
                    json!({"reason":"The text was saved on some of the tracks; read them again"})
                } else {
                    json!({"transactionId":"data_1","state":"applied"})
                }
            }
            other => panic!("not a tool here: {other}"),
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap())
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
fn bridge(reports: bool, tracks: &[(&str, &str, Option<&str>)]) -> Rc<Bridge> {
    Rc::new(Bridge {
        reports,
        refuse: RefCell::new(vec![]),
        tracks: RefCell::new(tracks.iter().map(|(r, i, id)| (r.to_string(), i.to_string(), id.map(str::to_owned))).collect()),
        pending: RefCell::new(None),
        calls: RefCell::new(vec![]),
    })
}
async fn connect(bridge: Rc<Bridge>) -> Rc<LiveConnection> {
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = bridge.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.epoch.set(Some(1.0));
    connection
}
fn writes(bridge: &Bridge) -> usize {
    bridge.calls.borrow().iter().filter(|(name, _)| name == "live_data_apply").count()
}

#[tokio::test(flavor = "current_thread")]
async fn a_sets_tracks_get_their_ids_in_one_write_and_a_copy_gets_its_own() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let live = bridge(
                true,
                &[("1:track:0", "live:1", None), ("1:track:1", "live:2", None), ("1:return:0", "live:3", None), ("1:main", "live:4", None)],
            );
            let connection = connect(live.clone()).await;
            let ids = TrackIds::default();
            assert!(ids.due("p", Some("rev-1"), &[], 0), "never made whole");
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            // Every track, its returns and its main track: an id each, all different, in one write.
            let given: Vec<String> = live.tracks.borrow().iter().map(|t| t.2.clone().unwrap()).collect();
            assert!(given.iter().all(|id| track_id(id)) && given.iter().collect::<std::collections::HashSet<_>>().len() == 4, "{given:?}");
            assert_eq!(writes(&live), 1);
            assert!(
                ids.reported(Some(1.0))
                    && !ids.due("p", Some("rev-1"), &[], 0)
                    && ids.due("p", Some("rev-2"), &[], 0)
                    && ids.due("p", Some("rev-1"), &["1:track:0".into()], 0)
            );
            // The first track duplicated: the copy (after it) gets its own, the original keeps its id.
            let first = given[0].clone();
            live.tracks.borrow_mut().insert(1, ("1:track:1".into(), "live:5".into(), Some(first.clone())));
            live.tracks.borrow_mut()[2].0 = "1:track:2".into();
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            let tracks = live.tracks.borrow().clone();
            assert_eq!(tracks[0].2.as_deref(), Some(first.as_str()));
            assert!(tracks[1].2.as_deref().is_some_and(|id| track_id(id) && id != first));
            assert_eq!(writes(&live), 2, "one more write, for the copy alone");
            // Nothing missing or shared: no write.
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            assert_eq!(writes(&live), 2);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_live_that_doesnt_report_ids_is_asked_once_and_a_template_is_never_written() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // A bridge from before track ids: one track read, once, and nothing more.
            let old = bridge(false, &[("1:track:0", "live:1", None), ("1:track:1", "live:2", None), ("1:main", "live:9", None)]);
            let connection = connect(old.clone()).await;
            let ids = TrackIds::default();
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            let calls = old.calls.borrow().clone();
            assert_eq!(calls.len(), 1, "{calls:?}");
            assert_eq!(calls[0].1, json!({"kind":"main-track","fields":["kumiTrack"],"limit":1}), "the main track: every Set has one");
            assert!(!ids.reported(Some(1.0)));
            // A template: its tracks are read, never written.
            let template = bridge(true, &[("1:track:0", "live:1", None), ("1:main", "live:9", None)]);
            let connection = connect(template.clone()).await;
            let ids = TrackIds::default();
            ids.pass(&connection, "p", None, false, 0, Signal::new()).await.unwrap();
            assert_eq!((writes(&template), template.tracks.borrow()[0].2.clone()), (0, None));
            assert!(!ids.due("p", Some("rev-1"), &[], 0), "read through, not asked again for the same list");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_track_that_wont_take_its_id_waits_for_the_list_to_change_and_live_s_structure_news_counts() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let live = bridge(true, &[("1:track:0", "live:1", None), ("1:track:1", "live:2", None), ("1:main", "live:9", None)]);
            live.refuse.borrow_mut().push("1:track:1".into());
            let connection = connect(live.clone()).await;
            let ids = TrackIds::default();
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            let tracks = live.tracks.borrow().clone();
            assert!(tracks[0].2.as_deref().is_some_and(track_id) && tracks[1].2.is_none(), "{tracks:?}");
            // The track still without one isn't a reason for another pass, until the track list changes.
            assert!(!ids.due("p", Some("rev-1"), &["1:track:1".into()], 0));
            assert!(ids.due("p", Some("rev-2"), &["1:track:1".into()], 0));
            assert!(ids.due("p", Some("rev-1"), &["1:track:0".into()], 0), "another track's gap is");
            // A return duplicated changes no list the observation reads, but Live tells of the structure change.
            assert!(ids.due("p", Some("rev-1"), &[], 1));
            // The first track duplicated, and the copy won't take an id: it doesn't take its original's over either,
            // so once it can, the copy gets a new one and the original keeps its own.
            let first = tracks[0].2.clone().unwrap();
            live.tracks.borrow_mut().insert(1, ("1:track:1".into(), "live:5".into(), Some(first.clone())));
            live.tracks.borrow_mut()[2].0 = "1:track:2".into();
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            assert_eq!(live.tracks.borrow()[1].2.as_deref(), Some(first.as_str()), "the copy refused");
            live.refuse.borrow_mut().clear();
            ids.pass(&connection, "p", None, true, 0, Signal::new()).await.unwrap();
            let tracks = live.tracks.borrow().clone();
            assert_eq!(tracks[0].2.as_deref(), Some(first.as_str()), "the original keeps its id: {tracks:?}");
            assert!(tracks[1].2.as_deref().is_some_and(|id| track_id(id) && id != first), "{tracks:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn which_track_keeps_a_shared_id_is_kept_with_the_project_so_a_restart_keeps_the_original() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let store: Rc<dyn ProjectStore> = create_project_store(dir.path());
            let project = "0123456789abcdef0123456789abcdef";
            let keepers = || Some(Keepers { project: project.into(), store: store.clone() });
            let live = bridge(true, &[("1:track:0", "live:1", Some(A)), ("1:main", "live:9", Some(B))]);
            let connection = connect(live.clone()).await;
            TrackIds::default().pass(&connection, project, keepers(), true, 0, Signal::new()).await.unwrap();
            assert_eq!(store.load_track_keepers(project).await.unwrap().get(A).map(String::as_str), Some("live:1"));
            // Kumi restarts (nothing in memory), and a copy of the track now sits above it: the original keeps its id.
            live.tracks.borrow_mut().insert(0, ("1:track:0".into(), "live:5".into(), Some(A.into())));
            live.tracks.borrow_mut()[1].0 = "1:track:1".into();
            TrackIds::default().pass(&connection, project, keepers(), true, 0, Signal::new()).await.unwrap();
            let tracks = live.tracks.borrow().clone();
            assert_eq!(tracks[1].2.as_deref(), Some(A), "the original, seen with it before");
            assert!(tracks[0].2.as_deref().is_some_and(|id| track_id(id) && id != A), "the copy, a new one");
        })
        .await;
}
