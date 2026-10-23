use ableton_mcp_server::live::*;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Recording {
    state: RefCell<LiveSnapshot>,
    requests: RefCell<Vec<LiveSnapshotRequest>>,
    ignore: Cell<bool>,
    whole_pages: Cell<bool>,
    move_after_focus: Cell<bool>,
    discoveries: RefCell<Vec<LiveDiscoveryResult>>,
}
impl Recording {
    fn new(count: usize) -> Rc<Self> {
        let tracks = (0..count)
            .map(|i| {
                let mut track = Track::new(format!("track:extra-{i}").into(), &format!("Extra {i}"), TrackKind::Midi);
                track.object_identity = Some(format!("simulator:track:extra-{i}"));
                if i == 0 {
                    track.extra.insert("owned".into(), json!({"ref":"parameter:gain-1"}));
                }
                track
            })
            .collect();
        Rc::new(Self {
            state: RefCell::new(LiveSnapshot {
                tracks: Some(tracks),
                epoch: Some(1),
                track_count: Some(count),
                scene_count: Some(1),
                ..Default::default()
            }),
            requests: RefCell::new(vec![]),
            ignore: Cell::new(false),
            whole_pages: Cell::new(false),
            move_after_focus: Cell::new(false),
            discoveries: RefCell::new(vec![]),
        })
    }
    fn view(&self, request: &LiveSnapshotRequest) -> LiveSnapshot {
        let mut result = self.state.borrow().clone();
        if self.ignore.get() || (self.whole_pages.get() && request.tracks.is_some()) {
            return result;
        }
        if let Some(focus) = &request.focus {
            result.tracks = Some(
                result
                    .tracks()
                    .iter()
                    .enumerate()
                    .map(|(i, row)| if focus.contains(&i) { row.clone() } else { light_track_row(row, None) })
                    .collect(),
            );
        }
        if let Some(window) = request.tracks {
            result.tracks = Some(result.tracks().iter().skip(window.from).take(window.count).cloned().collect());
        }
        result.window = Some(request.clone());
        if self.move_after_focus.replace(false) && request.focus.is_some() {
            let mut state = self.state.borrow_mut();
            let last = state.tracks_mut().pop().unwrap();
            state.tracks_mut().insert(WHOLE_SET_PAGE_TRACKS, last);
        }
        result
    }
}
impl LiveAdapter for Recording {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        UnavailableLiveAdapter.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        Ok(self.state.borrow().clone())
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        Ok(None)
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("unused test operation"))
    }
    fn subscribe(&self, _: LiveListener) -> Result<Unsubscribe, LiveError> {
        Ok(Box::new(|| {}))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait(?Send)]
impl AsyncLiveAdapter for Recording {
    async fn snapshot_async(
        &self,
        _: Option<&LiveOperationContext>,
        request: Option<&LiveSnapshotRequest>,
    ) -> Result<LiveSnapshot, LiveError> {
        let request = request.cloned().unwrap_or_default();
        self.requests.borrow_mut().push(request.clone());
        Ok(self.view(&request))
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Ok(self.discoveries.borrow_mut().remove(0))
    }
    async fn get_async(&self, reference: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(reference)
    }
    async fn invoke_async(&self, invocation: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke(invocation)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
fn views(adapter: &Rc<Recording>) -> LiveViews {
    let adapter = adapter.clone();
    LiveViews::new(move || adapter.clone())
}

#[test]
fn positional_references_place_themselves_on_their_track_by_the_combined_index() {
    for (reference, index) in [
        ("7:track:3", Some(3)),
        ("7:clip:3:5", Some(3)),
        ("7:clip_slot:3:5", Some(3)),
        ("7:device:3:0:2", Some(3)),
        ("7:chain:3:0", Some(3)),
        ("7:drum_pad:3:0:36", Some(3)),
        ("7:take_lane:3:1", Some(3)),
        ("7:take_lane_clip:3:1:0", Some(3)),
        ("7:arrangement_clip:3:7", Some(3)),
        ("7:routing_choice:12:input-type:0", Some(12)),
        ("7:parameter:7:device:3:0:2:5", Some(3)),
        ("7:parameter:7:device:4:1:macro:0", Some(4)),
        ("7:parameter:mixer:3:volume", Some(3)),
        ("7:parameter:mixer:9:sends:1", Some(9)),
        ("7:parameter:7:chain:3:0:1:volume", Some(3)),
        ("7:chain:7:device:5:0:selected", Some(5)),
        ("7:set:song", None),
        ("7:scene:2", None),
        ("7:locator:0", None),
        ("7:track:group:3", None),
        ("7:device:view:3", None),
        ("7:device:appointed", None),
        ("7:arrangement_clip:4", None),
        ("track:track-1", None),
        ("clip:clip-1", None),
        ("7:track:100001", None),
    ] {
        assert_eq!(track_index_of_ref(reference), index, "{reference}");
    }
}
#[tokio::test]
async fn views_read_the_tracks_an_operation_names_whole_find_references_and_page_the_whole_set() {
    let adapter = Recording::new(WHOLE_SET_PAGE_TRACKS + 4);
    let views = views(&adapter);
    let whole = views.whole_set(None, None).await.unwrap();
    assert_eq!(whole.tracks, adapter.state.borrow().tracks);
    assert!(whole.window.is_none());
    assert_eq!(views.track_count.get(), 20);
    assert_eq!(
        *adapter.requests.borrow(),
        vec![
            LiveSnapshotRequest::focused((0..16).collect()),
            LiveSnapshotRequest {
                tracks: Some(LiveSnapshotWindow { from: 16, count: 4 }),
                parts: Some(vec![LiveSnapshotPart::Tracks, LiveSnapshotPart::Arrangement]),
                ..Default::default()
            }
        ]
    );
    adapter.requests.borrow_mut().clear();
    let focused = views.view_for(None, &[json!("parameter:gain-1"), json!("scene:scene-1"), Value::Null], None, &[]).await.unwrap();
    assert_eq!(*adapter.requests.borrow(), vec![LiveSnapshotRequest::focused(vec![0])]);
    assert_eq!(focused.tracks().iter().filter(|track| !track.is_light()).count(), 1);
    {
        let mut state = adapter.state.borrow_mut();
        let first = state.tracks_mut().remove(0);
        state.tracks_mut().push(first);
    }
    adapter.requests.borrow_mut().clear();
    let moved = views.view_for(None, &[json!("parameter:gain-1")], None, &[]).await.unwrap();
    assert_eq!(adapter.requests.borrow()[0], LiveSnapshotRequest::focused(vec![0]));
    assert_eq!(adapter.requests.borrow().len(), 3);
    assert!(moved.tracks().iter().all(|track| !track.is_light()));
    adapter.requests.borrow_mut().clear();
    views.view_for(None, &[json!("parameter:gain-1")], None, &[]).await.unwrap();
    assert_eq!(*adapter.requests.borrow(), vec![LiveSnapshotRequest::focused(vec![19])]);
}
#[tokio::test]
async fn a_paged_whole_set_read_keeps_each_arrangement_clip_once() {
    let adapter = Recording::new(WHOLE_SET_PAGE_TRACKS + 4);
    // Every page carries the Set's Arrangement clips: the assembled read keeps one of each (one without a ref too).
    let clips: Vec<_> = (0..3)
        .map(|i| json!({"ref":format!("7:arrangement_clip:{i}:0"),"trackRef":format!("track:extra-{i}"),"name":format!("Clip {i}")}))
        .chain([json!({"trackRef":"track:extra-2","name":"No ref"})])
        .map(|clip| clip.as_object().unwrap().clone())
        .collect();
    adapter.state.borrow_mut().arrangement =
        Some(Arrangement { length: None, locator_revision: None, locators: vec![], clips: Some(clips.clone()), extra: Default::default() });
    let whole = views(&adapter).whole_set(None, None).await.unwrap();
    assert_eq!(adapter.requests.borrow().len(), 2, "two pages");
    assert_eq!(whole.arrangement.unwrap().clips.unwrap(), clips);
}
#[tokio::test]
async fn whole_set_read_accepts_old_adapters_and_retries_tracks_that_move_between_pages() {
    let adapter = Recording::new(18);
    adapter.ignore.set(true);
    assert_eq!(views(&adapter).whole_set(None, None).await.unwrap().tracks().len(), 18);
    assert_eq!(adapter.requests.borrow().len(), 1);
    adapter.ignore.set(false);
    adapter.requests.borrow_mut().clear();
    adapter.move_after_focus.set(true);
    let result = views(&adapter).whole_set(None, None).await.unwrap();
    assert_eq!(result.tracks, adapter.state.borrow().tracks);
    assert_eq!(adapter.requests.borrow().iter().filter(|r| r.focus.is_some()).count(), 2);
    adapter.whole_pages.set(true);
    assert!(views(&adapter).whole_set(None, None).await.unwrap().tracks().iter().all(|track| !track.is_light()));
}
#[tokio::test]
async fn discovery_refuses_revision_changes_and_nonadvancing_cursors() {
    let adapter = Recording::new(1);
    let page = |revision: &str, cursor: Option<&str>| LiveDiscoveryResult {
        epoch: 1,
        items: vec![json!({"ref":"track:one"}).as_object().unwrap().clone()],
        truncated: cursor.is_some(),
        revision: revision.into(),
        kind: LiveDiscoveryKind::Track,
        next_cursor: cursor.map(String::from),
    };
    *adapter.discoveries.borrow_mut() = vec![page("one", Some("a")), page("two", None)];
    assert_eq!(
        views(&adapter).discover_all(&LiveDiscoveryRequest::of(LiveDiscoveryKind::Track), None, None).await.unwrap_err().to_string(),
        "the track list changed while it was read; read it again"
    );
    *adapter.discoveries.borrow_mut() = vec![page("one", Some("a")), page("one", Some("a"))];
    assert_eq!(
        views(&adapter).discover_all(&LiveDiscoveryRequest::of(LiveDiscoveryKind::Track), None, None).await.unwrap_err().to_string(),
        "the track list's cursor didn't move on"
    );
    *adapter.discoveries.borrow_mut() = vec![page("one", Some("a")), page("one", None)];
    assert_eq!(views(&adapter).discover_all(&LiveDiscoveryRequest::of(LiveDiscoveryKind::Track), None, Some(1)).await.unwrap().len(), 1);
}
