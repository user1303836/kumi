//! Catching up on a Logic Pro project, and watch_me, end to end: Kumi's host (in process) in front of the simulated
//! Logic. A project changed between two sessions (a track added, one renamed, a region moved, a marker deleted, the
//! tempo changed) is told in Logic's words when the next session opens it, through `on_catch_up` and in the model's
//! context, and its baseline is kept in the project store under Logic's place, where no Live code reads it as a Set;
//! watch_me tells what the producer did between its start and its stop; the snapshot reads each kind whole, a page at a
//! time, and again when an edit tore it. Nothing of it in Live's words.
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    simulator::{logic::LogicScript, Simulator},
};
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::core::{
    context::IntegrationContext,
    contracts::*,
    daw::DawId,
    errors::RuntimeError,
    project::{project_key_of, Baseline, ProjectStore},
};
use kumi_runtime::integrations::{
    create_project_store,
    logic::create_logic_integration,
    own_bridge::{Connect, OwnOptions},
};
use kumi_runtime::mcp::client::{McpEndpoint, StderrStatus};
use kumi_runtime::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

kumi_testkit::in_process!();
use in_process::InProcess;

/// Live's words, whole: none may reach the producer or the model over Logic.
static LIVE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|[Rr]acks?|[Cc]lips?|[Cc]lip slots?|[Ss]cenes?)\b")
        .unwrap()
});

fn assert_logics_words(said: &str) {
    let found: Vec<&str> = LIVE_WORDS.find_iter(said).map(|word| word.as_str()).collect();
    assert!(found.is_empty(), "Live's words {found:?} in: {said}");
}

/// The simulated Logic on the recorded Scratch, the project store's folder, and a host for each session.
struct Lab {
    home: tempfile::TempDir,
    script: LogicScript,
    simulator: Rc<Simulator>,
    projects: PathBuf,
}

/// A session's integration, and what it told the app.
struct Session {
    logic: Rc<dyn Integration>,
    caught_up: Rc<RefCell<Vec<CatchUp>>>,
    watching: Rc<RefCell<Vec<bool>>>,
}

impl Lab {
    async fn new() -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Logic);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = LogicScript::new();
        let mut config = script.config(secret, "com.apple.logic10");
        config.discovery_dir = Some(paths.discovery());
        let simulator = Rc::new(Simulator::start(config).await.unwrap());
        let projects = home.path().join("projects");
        Lab { home, script, simulator, projects }
    }

    fn store(&self) -> Rc<dyn ProjectStore> {
        create_project_store(&self.projects)
    }

    /// A session of Kumi's with Logic, through a host of its own: `store` keeps the projects' baselines; `endpoint` stands
    /// in front of the host when a test needs it to.
    fn session(
        &self,
        store: Option<Rc<dyn ProjectStore>>,
        endpoint: Option<Rc<dyn Fn(Rc<dyn McpEndpoint>) -> Rc<dyn McpEndpoint>>>,
    ) -> Session {
        let host: Rc<dyn McpEndpoint> = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Logic, self.home.path())), id: Cell::new(0) });
        let host = match endpoint {
            Some(wrap) => wrap(host),
            None => host,
        };
        let connect: Connect = Rc::new(move |_| {
            let host = host.clone();
            async move { Ok(host) }.boxed_local()
        });
        let (caught_up, watching): (Rc<RefCell<Vec<CatchUp>>>, Rc<RefCell<Vec<bool>>>) = Default::default();
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        let told = caught_up.clone();
        context.on_catch_up = Some(Rc::new(move |catch_up| told.borrow_mut().push(catch_up)));
        let watched = watching.clone();
        context.on_watch = Some(Rc::new(move |on| watched.borrow_mut().push(on)));
        context.project_store = store;
        let options = OwnOptions { context, kumi_home: self.home.path().into(), connect: Some(connect), open_file: None };
        Session { logic: create_logic_integration(options), caught_up, watching }
    }

    /// The store's key for the project open now.
    fn key(&self) -> String {
        project_key_of(DawId::Logic, &self.script.project().id.expect("a saved project"))
    }

    /// The baseline kept for the project open now, as its file holds it.
    fn baseline(&self) -> Option<Baseline> {
        let file = self.projects.join(self.key()).join("last-seen.json");
        std::fs::read(file).ok().map(|bytes| serde_json::from_slice(&bytes).unwrap())
    }

    /// The producer changes the project in Logic, by hand: no host, no journal of Kumi's.
    fn by_hand(&self, kind: &str, input: Value) {
        let respond = self.script.respond();
        let epoch = self.script.epoch().to_string();
        let preview = respond("change_preview", &json!({"kind": kind, "input": input, "epoch": epoch}))
            .expect("a change")
            .unwrap_or_else(|why| panic!("{kind}: {why:?}"));
        respond("change_apply", &json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": "by hand", "epoch": epoch}))
            .expect("a change")
            .unwrap_or_else(|why| panic!("{kind}: {why:?}"));
    }

    /// A ref of the epoch the project is in now.
    fn r(&self, what: &str) -> String {
        format!("{}:{what}", self.script.epoch())
    }
}

/// Waits (a while) for `done`.
async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

fn tool(observed: &Observation, name: &str) -> Rc<dyn KernelTool> {
    observed.tools.iter().find(|tool| tool.name() == name).cloned().unwrap_or_else(|| panic!("no {name} tool"))
}

fn action(name: &str) -> JsonObject {
    json!({"action": name}).as_object().unwrap().clone()
}

/// What the producer did between two sessions is told when the next one opens the project, in Logic's words: to the
/// app (`on_catch_up`) and to the model (`sinceLastTime`); the project as it is then is the next baseline, and a project
/// nobody changed says so.
#[tokio::test(flavor = "current_thread")]
async fn a_project_changed_between_sessions_is_caught_up_on_in_logics_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let lab = Lab::new().await;
            // The first session keeps the project as it is, and has nothing to tell.
            let first = lab.session(Some(lab.store()), None);
            first.logic.start(Signal::new()).await.unwrap();
            let seen = first.logic.observe(Signal::new(), None).await.unwrap();
            assert_eq!(seen.label, "Logic Pro — Scratch");
            until("the first baseline", || lab.baseline().is_some()).await;
            assert!(first.caught_up.borrow().is_empty(), "nothing to tell of a project seen first");
            first.logic.close().await.unwrap();
            // (kept again as the session closed)
            let kept = lab.baseline().unwrap();
            assert_eq!((kept.name.as_str(), kept.path), ("Scratch", lab.script.project().path.unwrap()));
            assert!(lab.projects.join(format!("logic-{}", lab.script.project().id.unwrap())).join("last-seen.json").is_file());

            // Between the sessions, in Logic: a track added after Keys, Vocal renamed, the region moved to bar 3, the
            // marker deleted, the tempo changed.
            let (marker, tempo) = (lab.script.markers()[0].0.clone(), lab.script.tempo());
            lab.by_hand("track-add", json!({"name": "Pad", "index": 2}));
            lab.by_hand("track-rename", json!({"ref": lab.r("track:1"), "name": "Lead Vocal"}));
            lab.by_hand("region-move", json!({"ref": lab.r("region:1"), "positionBeats": 8}));
            lab.by_hand("marker-delete", json!({"ref": lab.r("marker:1")}));
            lab.by_hand("tempo-set", json!({"bpm": 124}));

            // The next session catches up as it opens the project.
            let second = lab.session(Some(lab.store()), None);
            second.logic.start(Signal::new()).await.unwrap();
            second.logic.observe(Signal::new(), None).await.unwrap();
            until("the catch-up", || !second.caught_up.borrow().is_empty()).await;
            let told = second.caught_up.borrow()[0].clone();
            let tempo = kumi_common::js::number::to_string(tempo);
            assert_eq!(
                told.lines,
                [
                    format!("Tempo {tempo} → 124 BPM"),
                    "Renamed track “Vocal” → “Lead Vocal”".into(),
                    "Added track “Pad”".into(),
                    "Changed region “Keys” (position)".into(),
                    format!("Removed marker “{marker}”"),
                ]
            );
            assert_eq!((told.set.as_str(), told.last_seen_at, told.more, told.after_reconnect), ("Scratch", kept.saved_at, 0, None));
            // The model is told too, in each look while the project is open.
            let look = second.logic.observe(Signal::new(), None).await.unwrap();
            let context: Value = serde_json::from_str(&look.context).unwrap();
            assert_eq!(context["sinceLastTime"], json!({"lastSeen": "just now", "changes": told.lines}), "{context}");
            assert_logics_words(&look.context);
            assert_logics_words(&told.lines.join("\n"));
            // The project as it is now is the baseline the next session compares with.
            until("the new baseline", || lab.baseline().is_some_and(|now| now.artifact_id != kept.artifact_id)).await;
            second.logic.close().await.unwrap();

            // Nobody changed it since: the next session says so.
            let third = lab.session(Some(lab.store()), None);
            third.logic.start(Signal::new()).await.unwrap();
            third.logic.observe(Signal::new(), None).await.unwrap();
            until("the catch-up", || !third.caught_up.borrow().is_empty()).await;
            assert!(third.caught_up.borrow()[0].lines.is_empty(), "{:?}", third.caught_up.borrow());

            // Another project opened in Logic is its own: kept under its own place, with nothing to tell.
            lab.script.open_project(1);
            lab.simulator.open_project(Some(lab.script.project())).unwrap();
            let other = third.logic.observe(Signal::new(), None).await.unwrap();
            assert_eq!(other.label, "Logic Pro — Scratch 1");
            until("the other project's baseline", || lab.baseline().is_some()).await;
            assert_eq!(third.caught_up.borrow().len(), 1, "nothing told of a project seen first");
            let context: Value = serde_json::from_str(&other.context).unwrap();
            assert!(context.get("sinceLastTime").is_none(), "what was told of Scratch isn't Scratch 1's: {context}");
            third.logic.close().await.unwrap();
            lab.simulator.stop();
        })
        .await;
}

/// watch_me in Logic: start notes the project, stop tells what the producer did in between, each with its track, and
/// the app hears when Kumi watches; a stop without a start, or after another project opened, says why not.
#[tokio::test(flavor = "current_thread")]
async fn watch_me_tells_what_the_producer_did_in_logics_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let lab = Lab::new().await;
            let session = lab.session(None, None);
            session.logic.start(Signal::new()).await.unwrap();
            let seen = session.logic.observe(Signal::new(), None).await.unwrap();
            let watch = tool(&seen, "watch_me");
            assert!(watch.deferred(), "rarely needed: behind find_tools, as Live's");
            assert_eq!(watch.reach(), Reach::DawRead);
            for words in ["Logic Pro", "regions", "plug-ins", "save_recipe"] {
                assert!(watch.description().contains(words), "{words}: {}", watch.description());
            }
            assert_logics_words(watch.description());
            assert_logics_words(&serde_json::to_string(&watch.input_schema()).unwrap());

            let early = watch.execute(action("stop"), Signal::new()).await.unwrap();
            assert!(early.is_error && early.text.contains("start first"), "{}", early.text);
            let started = watch.execute(action("start"), Signal::new()).await.unwrap();
            assert!(!started.is_error, "{}", started.text);
            let started: Value = serde_json::from_str(&started.text).unwrap();
            assert_eq!(
                started,
                json!({"watching": true, "note": "Tell the producer to go ahead in Logic Pro and to say when they're done."})
            );
            assert_eq!(*session.watching.borrow(), [true]);

            // The producer works in Logic: an instrument track added, Vocal muted, the region shortened, a marker made,
            // the tempo changed.
            lab.by_hand("track-add", json!({"name": "Bass", "kind": "instrument", "index": 2}));
            lab.by_hand("track-mute", json!({"ref": lab.r("track:1"), "mute": true}));
            lab.by_hand("region-resize", json!({"ref": lab.r("region:1"), "endBeats": 12}));
            lab.by_hand("marker-add", json!({"positionBeats": 64}));
            lab.by_hand("tempo-set", json!({"bpm": 128}));

            let stopped = watch.execute(action("stop"), Signal::new()).await.unwrap();
            assert!(!stopped.is_error, "{}", stopped.text);
            assert_logics_words(&stopped.text);
            let stopped: Value = serde_json::from_str(&stopped.text).unwrap();
            assert!(stopped["watched"].as_str().is_some_and(|watched| watched.ends_with(" s")), "{stopped}");
            let changes = stopped["changes"].as_array().unwrap();
            // (the answer is JSON as Kumi writes it: a whole number has no decimals)
            assert_eq!(lab.script.tempo(), 128.0);
            assert_eq!(changes[0], json!({"changed": "project", "name": null, "what": [{"what": "tempo", "from": 160, "to": 128}]}));
            assert_eq!(changes[1]["added"], "track");
            assert_eq!((changes[1]["name"].as_str(), changes[1]["trackKind"].as_str()), (Some("Bass"), Some("instrument")));
            assert!(
                changes.contains(&json!({"changed": "track", "name": "Vocal", "what": [{"what": "mute", "from": false, "to": true}]})),
                "{stopped}"
            );
            assert!(
                changes.contains(
                    &json!({"changed": "region", "name": "Keys", "on": "Keys", "what": [{"what": "lengthBeats", "from": 16, "to": 12}]})
                ),
                "{stopped}"
            );
            let marker = changes.iter().find(|change| change["added"] == "marker").unwrap_or_else(|| panic!("the marker: {stopped}"));
            assert_eq!(marker["position"], "bar 17");
            assert_eq!(changes.len(), 5, "{stopped}");
            assert_eq!(*session.watching.borrow(), [true, false]);
            let again = watch.execute(action("stop"), Signal::new()).await.unwrap();
            assert!(again.is_error && again.text.contains("start first"), "{}", again.text);

            // Nothing done between start and stop: said so.
            watch.execute(action("start"), Signal::new()).await.unwrap();
            let idle: Value = serde_json::from_str(&watch.execute(action("stop"), Signal::new()).await.unwrap().text).unwrap();
            assert_eq!(
                (idle["changes"].clone(), idle["note"].as_str()),
                (json!([]), Some("Nothing in the project changed while Kumi watched."))
            );

            // Another project opened while Kumi watched: nothing to compare.
            watch.execute(action("start"), Signal::new()).await.unwrap();
            lab.script.open_project(1);
            lab.simulator.open_project(Some(lab.script.project())).unwrap();
            let other = watch.execute(action("stop"), Signal::new()).await.unwrap();
            assert!(
                other.is_error && other.text == "A different project is open now, so there's nothing to compare; start again.",
                "{}",
                other.text
            );
            assert_eq!(*session.watching.borrow(), [true, false, true, false, true, false]);
            session.logic.close().await.unwrap();
            lab.simulator.stop();
        })
        .await;
}

/// The host in front of the simulated Logic, moving the project's epoch once, as an edit in Logic would, when the
/// snapshot's first page of `kind` is asked for: that page answers in the new epoch, tearing the snapshot.
struct Tearing {
    inner: Rc<dyn McpEndpoint>,
    script: LogicScript,
    kind: &'static str,
    torn: Cell<bool>,
}

/// (M6) A bridge from before the bulk read: its snapshot is the one screen whatever it's asked.
fn without_rows(name: &str, mut args: JsonObject) -> JsonObject {
    if name == "snapshot" {
        args.remove("rows");
        args.remove("limit");
    }
    args
}
#[async_trait::async_trait(?Send)]
impl McpEndpoint for Tearing {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, server: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.inner.list(server, signal).await
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        if name == "read" && args.get("kind") == Some(&json!(self.kind)) && args.contains_key("limit") && !self.torn.replace(true) {
            self.script.change_tracks();
        }
        self.inner.call(name, without_rows(name, args), signal).await
    }
    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.inner.on_catalog_changed(listener)
    }
    fn on_disconnect(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.inner.on_disconnect(listener)
    }
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        self.inner.on_live_event(listener)
    }
    fn stderr_status(&self) -> StderrStatus {
        self.inner.stderr_status()
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.inner.close().await
    }
}

/// The snapshot reads each kind of the project whole, a page of 200 at a time, and again from the start when an edit
/// tore it (an epoch moved between its pages). What it takes over the simulated Scratch is printed: its size as the
/// baseline keeps it, its reads, and how long it takes.
#[tokio::test(flavor = "current_thread")]
async fn the_snapshot_reads_each_kind_whole_and_again_when_an_edit_tore_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let lab = Lab::new().await;
            let script = lab.script.clone();
            let tear: Rc<dyn Fn(Rc<dyn McpEndpoint>) -> Rc<dyn McpEndpoint>> = Rc::new(move |inner| {
                Rc::new(Tearing { inner, script: script.clone(), kind: "region", torn: Cell::new(false) }) as Rc<dyn McpEndpoint>
            });
            let session = lab.session(Some(lab.store()), Some(tear));
            session.logic.start(Signal::new()).await.unwrap();
            session.logic.observe(Signal::new(), None).await.unwrap();
            until("the baseline", || lab.baseline().is_some()).await;
            let paged = |asked: &[(String, Value)]| -> Vec<String> {
                asked
                    .iter()
                    .filter(|(method, params)| method == "read" && params.get("limit") == Some(&json!(200)))
                    .map(|(_, params)| params["kind"].as_str().unwrap_or_default().to_owned())
                    .collect()
            };
            // Torn at its regions: read again from the project's row, every page in the epoch of now.
            let kinds = ["project", "track", "region", "plugin", "marker", "tempo"];
            let mut expected: Vec<String> = kinds[..3].iter().map(|kind| kind.to_string()).collect();
            expected.extend(kinds.iter().map(|kind| kind.to_string()));
            assert_eq!(paged(&lab.script.asked()), expected);
            let kept = lab.baseline().unwrap();
            let snapshot = &kept.pages[0];
            assert_eq!(snapshot["tracks"].as_array().map(Vec::len), Some(2));
            assert_eq!(snapshot["plugins"][0], json!({"track": 1, "slot": 1, "name": "Channel EQ", "bypassed": false, "kind": "effect"}));
            assert_eq!(snapshot["regions"][0]["track"], 2);

            // One snapshot, as watch_me's start takes it: its reads and its time over the simulator.
            let seen = session.logic.observe(Signal::new(), None).await.unwrap();
            let before = paged(&lab.script.asked()).len();
            let started = Instant::now();
            assert!(!tool(&seen, "watch_me").execute(action("start"), Signal::new()).await.unwrap().is_error);
            let took = started.elapsed();
            let reads = paged(&lab.script.asked()).len() - before;
            let bytes = std::fs::metadata(lab.projects.join(lab.key()).join("last-seen.json")).unwrap().len();
            eprintln!(
                "snapshot of Scratch over the simulated Logic: {reads} reads, {} ms; baseline {bytes} bytes ({} bytes of snapshot)",
                took.as_millis(),
                serde_json::to_string(snapshot).unwrap().len()
            );
            assert_eq!(reads, kinds.len(), "one page of each kind for Scratch");
            assert!(took < Duration::from_secs(2) && bytes < 8 * 1024, "{took:?}, {bytes} bytes");
            session.logic.close().await.unwrap();
            lab.simulator.stop();
        })
        .await;
}

/// (M6) With the bridge's bulk read, the snapshot is one request (`snapshot {rows}`), no page read, and what it keeps is
/// what the pages kept.
#[tokio::test(flavor = "current_thread")]
async fn the_snapshot_is_one_bulk_read_where_the_bridge_has_it_and_keeps_what_the_pages_kept() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let read_by = |bulk: bool| async move {
                let lab = Lab::new().await;
                let script = lab.script.clone();
                let older: Rc<dyn Fn(Rc<dyn McpEndpoint>) -> Rc<dyn McpEndpoint>> = Rc::new(move |inner| {
                    Rc::new(Tearing { inner, script: script.clone(), kind: "none", torn: Cell::new(true) }) as Rc<dyn McpEndpoint>
                });
                let session = lab.session(Some(lab.store()), (!bulk).then_some(older));
                session.logic.start(Signal::new()).await.unwrap();
                session.logic.observe(Signal::new(), None).await.unwrap();
                until("the baseline", || lab.baseline().is_some()).await;
                let asked = lab.script.asked();
                let bulk_reads = asked.iter().filter(|(method, params)| method == "snapshot" && params.get("rows").is_some()).count();
                let pages = asked.iter().filter(|(method, params)| method == "read" && params.get("limit") == Some(&json!(200))).count();
                let kept = lab.baseline().unwrap().pages[0].clone();
                session.logic.close().await.unwrap();
                lab.simulator.stop();
                (bulk_reads, pages, kept)
            };
            let (bulk_reads, pages, bulk) = read_by(true).await;
            assert_eq!((bulk_reads, pages), (1, 0), "one look at the project");
            let (bulk_reads, pages, paged) = read_by(false).await;
            assert_eq!((bulk_reads, pages), (0, 6), "an older bridge's pages, a kind at a time");
            assert_eq!(bulk, paged, "the same snapshot");
        })
        .await;
}

/// A Logic project's baseline sits in its place's folder (`logic-<id>`), beside its conversations: no Live code takes it
/// for a Set (the library's scan of Kumi's projects reads only Live's bare ids), and a Live Set's stays where it was.
#[tokio::test(flavor = "current_thread")]
async fn a_logic_projects_baseline_is_never_read_as_a_live_set() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_project_store(dir.path());
    let (song, set) = (dir.path().join("Song.logicx"), dir.path().join("Beat.als"));
    std::fs::create_dir_all(&song).unwrap();
    std::fs::write(&set, b"").unwrap();
    let id = "e805ab87b3ca9d66f0f62d47b97caa29";
    let baseline = |path: &std::path::Path| Baseline {
        version: 1,
        path: path.to_string_lossy().into_owned(),
        name: "Song".into(),
        saved_at: 1,
        artifact_id: "a".into(),
        pages: vec![json!({"snapshot": "logic"}).as_object().unwrap().clone()],
    };
    let logic = project_key_of(DawId::Logic, id);
    store.save(&logic, &baseline(&song)).await.unwrap();
    // (even a Logic baseline naming a Set's file: its folder isn't one of Live's)
    let odd = project_key_of(DawId::Logic, "0123456789abcdef0123456789abcdef");
    store.save(&odd, &baseline(&set)).await.unwrap();
    let projects = dir.path().to_string_lossy().into_owned();
    assert!(kumi_runtime::library::plan::kumi_sets(Some(&projects)).await.is_empty());
    assert_eq!(kumi_runtime::core::place::Place::parse(&logic).and_then(|place| place.daw()), Some(DawId::Logic));
    store.save(id, &baseline(&set)).await.unwrap();
    assert_eq!(kumi_runtime::library::plan::kumi_sets(Some(&projects)).await, [set.to_string_lossy().into_owned()]);
}
