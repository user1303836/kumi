//! Listening against the fake REAPER (M5 contract §14, m5-reaper): renders put every setting back on success, failure and
//! cancel; the socket threads answer while REAPER renders; captures, previews, audio files, spans, grids, scratch copies
//! and the scratch-keep kind.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::board::State;
use super::*;
use crate::extension::{Driver, Extension, Outbox, Snapshot};
use crate::queue::{self, Job, Outcome};
use crate::reaper::fake::{self, FakeFx, FakeMarker, FakeMedia, FakeParam, FakeSend, FakeTrack};

#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<Vec<(String, Value)>>>);

impl Outbox for Recorder {
    fn event(&self, name: &str, data: Value) {
        self.0.borrow_mut().push((name.into(), data));
    }
    fn heartbeat(&self) {
        self.0.borrow_mut().push(("heartbeat".into(), Value::Null));
    }
}

impl Recorder {
    fn events(&self) -> Vec<(String, Value)> {
        self.0.borrow().iter().filter(|(name, _)| name != "heartbeat").cloned().collect()
    }
}

static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Rig {
    ext: Extension,
    tx: Sender<Job>,
    snapshot: Arc<Mutex<Snapshot>>,
    dir: tempfile::TempDir,
    /// What the extension sent the host (events).
    out: Recorder,
}

thread_local! {
    /// The rig whose extension REAPER's timer reaches from inside the fake render.
    static RIG: Cell<*const Extension> = const { Cell::new(std::ptr::null()) };
    /// What a test's tick inside the render saw: (busy in the snapshot, the board holding).
    static INSIDE: RefCell<Vec<(Option<crate::reaper::Busy>, bool)>> = const { RefCell::new(Vec::new()) };
    static CANCEL_INSIDE: Cell<bool> = const { Cell::new(false) };
}

/// REAPER's timer firing inside its render loop: the extension ticks (nested), after Kumi's cancel when the test asks.
fn tick_inside_the_render() {
    let ext = RIG.with(Cell::get);
    assert!(!ext.is_null());
    let ext = unsafe { &*ext };
    let board = ext.snapshot().listening.clone();
    if CANCEL_INSIDE.with(Cell::get) {
        if let Some((id, _)) = board.active() {
            board.update(&id, |job| job.cancel = true);
        }
    }
    ext.tick(Driver::Timer);
    let busy = ext.snapshot().busy;
    INSIDE.with(|i| i.borrow_mut().push((busy, board.holding())));
}

impl Rig {
    fn new() -> Rig {
        Rig::with_project(|| {})
    }

    /// A rig over the lab project as `setup` leaves it. The extension's first look is at that project: a setup that gives
    /// every track another id (`real_ids`) after it would be, to the bridge, another project in the tab.
    fn with_project(setup: impl FnOnce()) -> Rig {
        fake::lab_project();
        fake::with(|s| {
            s.on_action = Some(fake::render_action);
            s.listening.sources_from_files = true;
        });
        setup();
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = queue::channel();
        let out = Recorder::default();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let ext = Extension::new(fake::api(), rx, Box::new(out.clone()), snapshot.clone());
        ext.set_listening_folder(dir.path().join("bridges/reaper/listening"));
        ext.tick(Driver::Timer);
        INSIDE.with(|i| i.borrow_mut().clear());
        CANCEL_INSIDE.with(|c| c.set(false));
        Rig { ext, tx, snapshot, dir, out }
    }

    /// A rig over `copyable`'s project, and Kick's ref.
    fn copyable() -> (Rig, String) {
        let mut kick = String::new();
        let rig = Rig::with_project(|| kick = copyable());
        (rig, kick)
    }

    /// Lets the fake's render tick this rig's extension, as REAPER's timer does inside a render.
    fn reachable_from_the_render(&self) {
        RIG.with(|r| r.set(&self.ext as *const Extension));
        fake::with(|s| s.listening.during_render = Some(tick_inside_the_render));
    }

    fn folder(&self) -> PathBuf {
        self.dir.path().join("bridges/reaper/listening")
    }

    fn ask(&self, method: &str, params: Value) -> Answer {
        let (job, waiter) = queue::job(method, params);
        self.tx.send(job).unwrap();
        self.ext.tick(Driver::Timer);
        match waiter.wait(Duration::from_secs(1), &OPEN) {
            Outcome::Answered(answer) => answer,
            other => panic!("{method}: {other:?}"),
        }
    }

    fn tick(&self) {
        self.ext.tick(Driver::Timer);
    }

    fn epoch(&self) -> String {
        self.ask("read", json!({"kind": "project"})).unwrap()["epoch"].as_str().unwrap().to_string()
    }

    fn board(&self) -> Arc<Board> {
        self.snapshot.lock().unwrap().listening.clone()
    }

    fn status(&self, id: &str) -> Value {
        self.board().status(&json!({"jobId": id})).unwrap()
    }

    /// Ticks until the job has ended (a track's items take a slice a tick), and its status then.
    fn until_ended(&self, id: &str) -> Value {
        for _ in 0..500 {
            self.tick();
            let status = self.status(id);
            if !matches!(status["state"].as_str(), Some("queued" | "running")) {
                return status;
            }
        }
        panic!("{id} never ended: {}", self.status(id));
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        RIG.with(|r| r.set(std::ptr::null()));
    }
}

fn files_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(folder).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    names
}

/// Every render setting, as the project has it now: numbers, then texts.
type Settings = (Vec<(String, f64)>, Vec<(String, String)>);

fn render_settings() -> Settings {
    fake::with(|s| {
        let numbers = render::NUMBERS.iter().map(|k| (k.to_string(), s.listening.numbers.get(*k).copied().unwrap_or(f64::NAN))).collect();
        let texts = render::TEXTS.iter().map(|k| (k.to_string(), s.listening.texts.get(*k).cloned().unwrap_or_default())).collect();
        (numbers, texts)
    })
}

/// A producer's own render settings, nothing like Kumi's.
fn producer_settings() {
    fake::with(|s| {
        let n = &mut s.listening.numbers;
        n.insert("RENDER_SETTINGS".into(), (128 | (16 << 16)) as f64);
        n.insert("RENDER_BOUNDSFLAG".into(), 2.0);
        n.insert("RENDER_TAILFLAG".into(), 2.0);
        n.insert("RENDER_TAILMS".into(), 1500.0);
        n.insert("RENDER_NORMALIZE".into(), 1.0);
        n.insert("RENDER_ADDTOPROJ".into(), 1.0);
        n.insert("RENDER_SRATE".into(), 96000.0);
        n.insert("RENDER_DELAY".into(), 0.25);
        let t = &mut s.listening.texts;
        t.insert("RENDER_FILE".into(), "/producer/renders".into());
        t.insert("RENDER_PATTERN".into(), "$project-$track".into());
        t.insert("RENDER_FORMAT2".into(), "l3pm".into());
        *s.listening.speed = 1;
        s.tracks[1].selected = true;
        s.dirty = false;
        s.listening.dirty_on_set = true;
    });
}

fn source(id: &str, what: &str, reference: Option<&str>, stage: Option<&str>) -> Value {
    let mut source = json!({"id": id, "what": what});
    if let Some(reference) = reference {
        source["ref"] = reference.into();
    }
    if let Some(stage) = stage {
        source["stage"] = stage.into();
    }
    source
}

// ----- renders -----

#[test]
fn a_render_of_the_mix_and_two_tracks_runs_in_two_passes_names_its_files_and_puts_every_setting_back() {
    let rig = Rig::new();
    producer_settings();
    let before = render_settings();
    let epoch = rig.epoch();
    let started = rig
        .ask(
            "render_start",
            json!({
                "jobId": "j1",
                "sources": [source("mix", "master", None, None), source("kick", "track", Some("r:track:{BBBB}"), Some("pre-fader")), source("snare", "track", Some("r:track:{CCCC}"), Some("post-fader"))],
                "range": {"fromBeat": 8, "beats": 8},
                "preRollBeats": 4,
                "sampleRate": 48000,
                "epoch": epoch,
            }),
        )
        .unwrap();
    // Answered at once, queued: the files are planned, 12 beats at 120 BPM.
    assert_eq!((started["state"].as_str(), started["seconds"].as_f64()), (Some("queued"), Some(6.0)));
    let folder = rig.folder();
    let planned: Vec<String> = started["files"].as_array().unwrap().iter().map(|f| f["file"].as_str().unwrap().to_string()).collect();
    assert_eq!(planned, ["mix", "kick", "snare"].map(|id| folder.join(format!("j1-{id}.wav")).to_string_lossy().into_owned()));
    assert_eq!(rig.status("j1")["state"], "queued");
    assert!(fake::with(|s| s.listening.renders.is_empty()), "nothing rendered in the tick that answered");
    rig.tick();
    let status = rig.status("j1");
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!(status["progress"], 1.0);
    let files = status["files"].as_array().unwrap();
    for (file, (id, stage)) in files.iter().zip([("mix", "master"), ("kick", "pre-fader"), ("snare", "post-fader")]) {
        assert_eq!((file["id"].as_str(), file["stage"].as_str()), (Some(id), Some(stage)));
        assert_eq!((file["start"].as_f64(), file["seconds"].as_f64()), (Some(2.0), Some(6.0)));
        assert_eq!(
            (file["channels"].as_u64(), file["sampleRate"].as_u64(), file["throughMaster"].as_bool()),
            (Some(2), Some(48000), Some(false))
        );
        let info = wav::read_info(Path::new(file["file"].as_str().unwrap())).unwrap();
        assert!(info.float && info.frames == 288_000, "{info:?}");
        #[cfg(unix)]
        assert!(daw_bridge::owner::is_owner_only(Path::new(file["file"].as_str().unwrap())));
    }
    // Two passes: the post-fader stems with the mix (stems and master), then the pre-fader stems; each with Kumi's settings.
    let renders = fake::with(|s| s.listening.renders.clone());
    assert_eq!(renders.len(), 2);
    assert_eq!((renders[0].settings, renders[0].selected.clone()), (1 | (16 << 16), vec!["{CCCC}".to_string()]));
    assert_eq!((renders[1].settings, renders[1].selected.clone()), (3 | 8192 | (16 << 16), vec!["{BBBB}".to_string()]));
    for seen in &renders {
        assert_eq!(
            (seen.speed, seen.format.as_str(), seen.normalize, seen.start, seen.end, seen.rate),
            (0, render::FLOAT_WAV, (4 << 16) as f64, 2.0, 8.0, 48000.0)
        );
        // The producer's "delay the start" stays.
        assert_eq!(seen.settings & (16 << 16), 16 << 16);
    }
    // Every setting back, the selection, the speed, the project's changed flag (the settings marked it changed).
    assert_eq!(render_settings(), before);
    assert_eq!(
        fake::with(|s| (s.tracks.iter().map(|t| t.selected).collect::<Vec<_>>(), *s.listening.speed, s.dirty)),
        (vec![false, true, false, false], 1, false)
    );
    assert_eq!(files_in(&folder), ["j1-kick.wav", "j1-mix.wav", "j1-snare.wav"], "only the sources' files, no pass leftovers");
    for answer in [started, status] {
        assert!(daw_bridge::contract::misplaced_refs(&answer).is_empty(), "{answer}");
    }
}

#[test]
fn the_mix_before_its_chain_renders_with_the_masters_fx_off_and_back_on_after() {
    let rig = Rig::new();
    fake::with(|s| s.master.fx = vec![FakeFx::new("{FX-LIM}", "VST: ReaLimit (Cockos)")]);
    let started = rig.ask(
        "render_start",
        json!({"jobId": "j2", "sources": [source("mix", "master", None, None), {"id": "pre", "what": "master", "beforeChain": true}], "range": {"fromBeat": 0, "beats": 4}}),
    );
    assert!(started.is_ok(), "{started:?}");
    rig.tick();
    assert_eq!(rig.status("j2")["state"], "done");
    let renders = fake::with(|s| s.listening.renders.clone());
    assert_eq!(renders.iter().map(|r| (r.settings, r.master_fx)).collect::<Vec<_>>(), [(0, true), (0, false)]);
    assert_eq!(fake::with(|s| s.master.extra.get("I_FXEN").copied()), Some(1.0));
    assert_eq!(rig.status("j2")["files"][1]["stage"], "before-chain");
}

#[test]
fn a_muted_folder_is_unmuted_for_its_pre_fader_stem_and_muted_again() {
    let rig = Rig::new();
    fake::with(|s| s.tracks[0].mute = true);
    let epoch = rig.epoch();
    rig.ask(
        "render_start",
        json!({"jobId": "jf", "sources": [source("drums", "track", Some("r:track:{AAAA}"), None), source("kick", "track", Some("r:track:{BBBB}"), Some("post-fader"))], "range": {"fromBeat": 0, "beats": 4}, "epoch": epoch}),
    )
    .unwrap();
    rig.tick();
    let status = rig.status("jf");
    assert_eq!(status["state"], "done");
    assert_eq!(
        status["notes"],
        json!(["“Drums” is muted: Kumi unmuted it for the render (REAPER renders a muted folder silent even before its fader) and muted it again"])
    );
    // The post-fader pass hears Drums muted (as its fader does), the pre-fader one unmuted.
    let renders = fake::with(|s| s.listening.renders.clone());
    assert_eq!(renders.iter().map(|r| r.muted.clone()).collect::<Vec<_>>(), [vec!["{AAAA}".to_string()], vec![]]);
    assert!(fake::with(|s| s.tracks[0].mute), "muted again");
}

#[test]
fn a_render_that_writes_nothing_fails_deletes_what_it_made_and_puts_everything_back() {
    let rig = Rig::new();
    producer_settings();
    let before = render_settings();
    fake::with(|s| s.listening.fail_render = true);
    let epoch = rig.epoch();
    rig.ask("render_start", json!({"jobId": "j3", "sources": [source("kick", "track", Some("r:track:{BBBB}"), None)], "range": {"fromBeat": 0, "beats": 4}, "epoch": epoch}))
        .unwrap();
    rig.tick();
    let status = rig.status("j3");
    assert_eq!((status["state"].as_str(), status["code"].as_str()), (Some("failed"), Some("failed")));
    assert!(status["error"].as_str().unwrap().contains("didn't write"), "{status}");
    assert_eq!(render_settings(), before);
    assert_eq!(fake::with(|s| (s.dirty, *s.listening.speed, s.tracks[1].selected)), (false, 1, true));
    assert!(files_in(&rig.folder()).is_empty());
}

#[test]
fn files_reaper_wouldnt_write_as_planned_stop_the_render_before_it_begins() {
    let rig = Rig::new();
    let before = render_settings();
    fake::with(|s| s.listening.odd_targets = true);
    rig.ask("render_start", json!({"jobId": "j4", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}))
        .unwrap();
    rig.tick();
    let status = rig.status("j4");
    assert_eq!(status["state"], "failed");
    assert!(status["error"].as_str().unwrap().contains("other files than Kumi planned"), "{status}");
    assert!(fake::with(|s| s.listening.renders.is_empty()), "REAPER's render never ran");
    assert_eq!(render_settings(), before);
}

#[test]
fn a_cancel_inside_the_render_presses_its_cancel_and_puts_everything_back() {
    let rig = Rig::new();
    producer_settings();
    let before = render_settings();
    rig.reachable_from_the_render();
    CANCEL_INSIDE.with(|c| c.set(true));
    fake::with(|s| s.listening.cancel_button = true);
    let epoch = rig.epoch();
    rig.ask(
        "render_start",
        json!({"jobId": "j5", "sources": [source("mix", "master", None, None), source("kick", "track", Some("r:track:{BBBB}"), None)], "range": {"fromBeat": 0, "beats": 8}, "epoch": epoch}),
    )
    .unwrap();
    rig.tick();
    let status = rig.status("j5");
    assert_eq!(status["state"], "cancelled", "{status}");
    assert_eq!(status.get("notes"), None, "REAPER stopped when asked");
    assert_eq!(fake::with(|s| s.listening.cancel_presses), 1);
    assert!(files_in(&rig.folder()).is_empty());
    assert_eq!(render_settings(), before);
    // Inside the render the socket threads were told REAPER renders for Kumi (not that it is stuck), and the board held.
    let inside = INSIDE.with(|i| i.borrow().clone());
    assert!(
        !inside.is_empty() && inside.iter().all(|(busy, holding)| *busy == Some(crate::reaper::Busy::KumiRender) && *holding),
        "{inside:?}"
    );
}

#[test]
fn a_cancel_reaper_cant_take_lets_it_finish_then_deletes_the_files_and_says_so() {
    let rig = Rig::new();
    rig.reachable_from_the_render();
    CANCEL_INSIDE.with(|c| c.set(true));
    rig.ask("render_start", json!({"jobId": "j6", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}))
        .unwrap();
    rig.tick();
    let status = rig.status("j6");
    assert_eq!(status["state"], "cancelled");
    assert_eq!(status["notes"], json!(["REAPER finished the render before it could stop"]));
    assert!(fake::with(|s| s.listening.cancel_presses) >= 1);
    assert!(files_in(&rig.folder()).is_empty());
}

#[test]
fn a_render_reaper_stopped_short_is_refused_and_one_with_no_known_speed_says_so() {
    let rig = Rig::new();
    fake::with(|s| s.listening.short_render = true);
    rig.ask(
        "render_start",
        json!({"jobId": "js1", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 8}}),
    )
    .unwrap();
    rig.tick();
    let status = rig.status("js1");
    assert_eq!(status["state"], "failed");
    assert!(status["error"].as_str().unwrap().contains("stopped before the end"), "{status}");
    assert!(files_in(&rig.folder()).is_empty());
    fake::with(|s| s.listening.no_speed = true);
    rig.ask(
        "render_start",
        json!({"jobId": "js2", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 8}}),
    )
    .unwrap();
    rig.tick();
    let status = rig.status("js2");
    assert_eq!(status["state"], "done");
    assert!(status["notes"][0].as_str().unwrap().contains("doesn't say its render speed"), "{status}");
}

#[test]
fn a_render_reaper_keeps_too_busy_to_begin_gives_up_with_nothing_done() {
    let rig = Rig::new();
    // (a window a slow test machine can't cross between the start and the first tick, as 30 ms once was)
    render::TEST_QUEUED_FOR.with(|w| w.set(Some(Duration::from_millis(400))));
    rig.ask(
        "render_start",
        json!({"jobId": "jb1", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}),
    )
    .unwrap();
    fake::with(|s| s.loading = true);
    rig.tick();
    assert_eq!(rig.status("jb1")["state"], "queued", "it waits for REAPER");
    std::thread::sleep(Duration::from_millis(450));
    rig.tick();
    let status = rig.status("jb1");
    render::TEST_QUEUED_FOR.with(|w| w.set(None));
    assert_eq!((status["state"].as_str(), status["code"].as_str()), (Some("failed"), Some("busy")));
    assert!(fake::with(|s| s.listening.renders.is_empty()));
}

#[test]
fn a_tracks_items_read_cancelled_while_reaper_is_busy_stops_at_once() {
    let rig = Rig::new();
    let epoch = rig.epoch();
    rig.ask("render_start", json!({"jobId": "jc1", "sources": [source("dry", "track", Some("r:track:{BBBB}"), Some("items"))], "range": {"fromBeat": 0, "beats": 400}, "epoch": epoch}))
        .unwrap();
    rig.tick();
    assert_eq!(rig.status("jc1")["state"], "running");
    fake::with(|s| s.loading = true);
    rig.board().update("jc1", |job| job.cancel = true);
    rig.tick();
    assert_eq!(rig.status("jc1")["state"], "cancelled");
    assert!(!rig.folder().join("jc1-dry.wav").exists());
    assert!(fake::with(|s| s.listening.accessors.iter().all(|(_, destroyed)| *destroyed)));
}

#[test]
fn a_render_cancelled_while_it_is_queued_never_runs() {
    let rig = Rig::new();
    rig.ask("render_start", json!({"jobId": "j7", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}))
        .unwrap();
    assert_eq!(rig.board().cancel(&json!({"jobId": "j7"}), board::CANCEL_WAIT).unwrap(), json!({"jobId": "j7", "state": "cancelled"}));
    rig.tick();
    assert!(fake::with(|s| s.listening.renders.is_empty()));
    assert_eq!(rig.status("j7")["state"], "cancelled");
}

#[test]
fn every_other_request_waits_out_a_render_and_runs_after_it() {
    let rig = Rig::new();
    rig.reachable_from_the_render();
    rig.ask("render_start", json!({"jobId": "j8", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}))
        .unwrap();
    // A read queued now runs in the tick, before the render (which waits for the next tick)...
    let (job, waiter) = queue::job("read", json!({"kind": "project"}));
    rig.tx.send(job).unwrap();
    rig.tick();
    assert!(matches!(waiter.wait(Duration::from_secs(1), &OPEN), Outcome::Answered(Ok(_))));
    // ...and the render ran in the tick after, the ticks inside it running nothing but the snapshot.
    assert_eq!(rig.status("j8")["state"], "done");
    assert!(!INSIDE.with(|i| i.borrow().is_empty()));
    assert!(!rig.board().holding());
    assert_eq!(rig.ext.snapshot().busy, None, "the tick after the render says REAPER is free again");
}

#[test]
fn a_render_while_reaper_plays_says_it_stopped_playback_and_one_while_recording_is_refused() {
    let rig = Rig::new();
    fake::with(|s| s.play_bits = 1);
    rig.ask("render_start", json!({"jobId": "j9", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}))
        .unwrap();
    rig.tick();
    assert_eq!(rig.status("j9")["notes"], json!(["REAPER stopped playback to render"]));
    assert_eq!(fake::with(|s| s.play_bits & 1), 0);
    fake::with(|s| s.play_bits = 5);
    let refused = rig.ask(
        "render_start",
        json!({"jobId": "j10", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}),
    );
    assert!(refused.unwrap_err().1.contains("recording"));
}

#[test]
fn render_start_checks_everything_before_it_queues_anything() {
    let rig = Rig::new();
    let epoch = rig.epoch();
    let mix = source("mix", "master", None, None);
    let kick = source("kick", "track", Some("r:track:{BBBB}"), None);
    let range = json!({"fromBeat": 0, "beats": 4});
    let refused = |params: Value| -> (String, String) { rig.ask("render_start", params).unwrap_err() };
    let base = |sources: Value| json!({"jobId": "jx", "sources": sources, "range": range, "epoch": epoch});
    for (params, code, words) in [
        (json!({"sources": [mix], "range": range}), "bad-params", "jobId"),
        (json!({"jobId": "../x", "sources": [mix], "range": range}), "bad-params", "jobId"),
        (base(json!([])), "bad-params", "1 to 18"),
        (base(json!((0..19).map(|n| source(&format!("m{n}"), "master", None, None)).collect::<Vec<_>>())), "bad-params", "1 to 18"),
        (base(json!([{"id": "a b", "what": "master"}])), "bad-params", "letters"),
        (base(json!([mix, mix])), "bad-params", "two sources"),
        (base(json!([{"id": "k", "what": "track"}])), "bad-params", "needs its ref"),
        (base(json!([{"id": "m", "what": "master", "ref": "r:track:{BBBB}"}])), "bad-params", "no ref"),
        (base(json!([source("k", "track", Some("r:track:{BBBB}"), Some("solo"))])), "bad-params", "stage"),
        (base(json!([source("k", "track", Some("r:track:{BBBB}"), Some("items")), mix])), "bad-params", "on their own"),
        (base(json!([source("k", "track", Some("r:track:master"), None)])), "bad-params", "master"),
        (base(json!([kick, source("k2", "track", Some("r:track:{BBBB}"), Some("pre-fader"))])), "bad-params", "once at each stage"),
        (base(json!([source("k", "track", Some("r:track:{ZZZZ}"), None)])), "gone", "isn't in the project"),
        (json!({"jobId": "jx", "sources": [kick], "range": range}), "bad-params", "epoch"),
        (json!({"jobId": "jx", "sources": [kick], "range": range, "epoch": "e-old"}), "stale", "earlier epoch"),
        (json!({"jobId": "jx", "sources": [mix]}), "bad-params", "range"),
        (json!({"jobId": "jx", "sources": [mix], "range": {"fromBeat": -1, "beats": 4}}), "bad-params", "fromBeat"),
        (json!({"jobId": "jx", "sources": [mix], "range": {"fromBeat": 0, "beats": 0}}), "bad-params", "beats"),
        (json!({"jobId": "jx", "sources": [mix], "range": range, "preRollBeats": -1}), "bad-params", "preRollBeats"),
        (json!({"jobId": "jx", "sources": [mix], "range": range, "sampleRate": 12345}), "bad-params", "sampleRate"),
        (json!({"jobId": "jx", "sources": [mix], "range": {"fromBeat": 0, "beats": 7300}}), "bad-params", "at most 3600"),
        (json!({"jobId": "jx", "sources": [mix, kick], "range": range, "epoch": epoch, "freeBytes": 1000}), "failed", "MB free"),
    ] {
        let (got, message) = refused(params.clone());
        assert_eq!(got, code, "{params}: {message}");
        assert!(message.contains(words), "{params}: {message}");
    }
    assert!(rig.board().active().is_none(), "nothing was queued");
    // A file that is there already is never written over.
    std::fs::create_dir_all(rig.folder()).unwrap();
    std::fs::write(rig.folder().join("jy-mix.wav"), b"x").unwrap();
    assert!(refused(json!({"jobId": "jy", "sources": [mix], "range": range})).1.contains("never writes over"));
    // One job at a time.
    rig.ask("render_start", json!({"jobId": "jz", "sources": [mix], "range": range})).unwrap();
    let (code, message) = refused(json!({"jobId": "jw", "sources": [mix], "range": range}));
    assert_eq!((code.as_str(), message.contains("rendering for Kumi")), ("busy", true));
    assert_eq!(refused(json!({"jobId": "jz", "sources": [mix], "range": range})).0, "bad-params", "a job id is used once");
}

#[test]
fn passes_put_each_stage_together_and_the_master_where_it_fits() {
    let s = |id: &str, stage: render::Stage, guid: Option<&str>| render::Source { id: id.into(), stage, guid: guid.map(str::to_string) };
    use render::Stage::*;
    let plan = |sources: Vec<render::Source>, quiet: &[usize]| render::passes_for_test(&sources, quiet);
    assert_eq!(plan(vec![s("m", Master, None)], &[]), [(0, 0, true, false)]);
    assert_eq!(plan(vec![s("a", PreFader, Some("A")), s("b", PreFader, Some("B"))], &[]), [(3 | 8192, 2, false, false)]);
    assert_eq!(plan(vec![s("m", Master, None), s("a", PostFader, Some("A"))], &[]), [(1, 1, true, false)]);
    // The master rides pre-fader stems when none of their tracks is muted...
    assert_eq!(plan(vec![s("m", Master, None), s("a", PreFader, Some("A"))], &[]), [(1 | 8192, 1, true, false)]);
    // ...never when one is: REAPER skips a muted track when it renders the master too, pre-fader stem or not.
    assert_eq!(plan(vec![s("m", Master, None), s("a", PreFader, Some("A"))], &[1]), [(3 | 8192, 1, false, false), (0, 0, true, false)]);
    let all = || vec![s("m", Master, None), s("p", BeforeChain, None), s("a", PostFader, Some("A")), s("b", PreFader, Some("B"))];
    assert_eq!(plan(all(), &[]), [(1, 1, true, false), (1 | 8192, 1, true, true)]);
    assert_eq!(plan(all(), &[3]), [(1, 1, true, false), (3 | 8192, 1, false, false), (0, 0, true, true)]);
    assert_eq!(plan(vec![s("m", Master, None), s("p", BeforeChain, None)], &[]), [(0, 0, true, false), (0, 0, true, true)]);
    assert_eq!(plan(vec![s("p", BeforeChain, None), s("a", PostFader, Some("A"))], &[]), [(1, 1, true, true)]);
}

#[test]
fn the_mix_rides_with_pre_fader_stems_unless_one_of_them_is_muted() {
    let rig = Rig::new();
    let epoch = rig.epoch();
    let job = |id: &str, track: &str| {
        rig.ask(
            "render_start",
            json!({"jobId": id, "sources": [source("mix", "master", None, None), source("t", "track", Some(track), None)], "range": {"fromBeat": 0, "beats": 4}, "epoch": epoch}),
        )
        .unwrap();
        rig.tick();
        assert_eq!(rig.status(id)["state"], "done");
        fake::with(|s| std::mem::take(&mut s.listening.renders)).iter().map(|r| r.settings).collect::<Vec<_>>()
    };
    assert_eq!(job("jm1", "r:track:{BBBB}"), [1 | 8192]);
    fake::with(|s| s.tracks[1].mute = true);
    assert_eq!(job("jm2", "r:track:{BBBB}"), [3 | 8192, 0]);
    // Kick sits in the Drums folder: REAPER still renders a track in a muted folder with the master (measured).
    fake::with(|s| {
        s.tracks[1].mute = false;
        s.tracks[0].mute = true;
    });
    assert_eq!(job("jm3", "r:track:{BBBB}"), [1 | 8192]);
}

#[test]
fn a_tracks_items_are_read_a_slice_a_tick_into_a_wav_with_no_render() {
    let rig = Rig::new();
    let epoch = rig.epoch();
    rig.ask("render_start", json!({"jobId": "j11", "sources": [source("dry", "track", Some("r:track:{BBBB}"), Some("items"))], "range": {"fromBeat": 0, "beats": 8}, "sampleRate": 44100, "epoch": epoch}))
        .unwrap();
    let status = rig.until_ended("j11");
    assert_eq!(status["state"], "done", "{status}");
    let file = &status["files"][0];
    assert_eq!((file["id"].as_str(), file["stage"].as_str(), file["seconds"].as_f64()), (Some("dry"), Some("items"), Some(4.0)));
    let path = PathBuf::from(file["file"].as_str().unwrap());
    let info = wav::read_info(&path).unwrap();
    assert_eq!((info.frames, info.rate), (176_400, 44100));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(f32::from_le_bytes(bytes[44..48].try_into().unwrap()), 0.25, "Kick's items play from the start");
    assert!(fake::with(|s| s.listening.renders.is_empty()), "no render");
    assert!(fake::with(|s| s.listening.accessors.iter().all(|(_, destroyed)| *destroyed)));
    // A MIDI track's items have no sound of their own: said.
    rig.ask("render_start", json!({"jobId": "j12", "sources": [source("dry", "track", Some("r:track:{CCCC}"), Some("items"))], "range": {"fromBeat": 0, "beats": 8}, "epoch": epoch}))
        .unwrap();
    assert!(rig.until_ended("j12")["notes"][0].as_str().unwrap().contains("no audio of its own"));
    // Items that change while they are read: refused, the file deleted.
    fake::with(|s| s.listening.accessor_changed = true);
    rig.ask("render_start", json!({"jobId": "j13", "sources": [source("dry", "track", Some("r:track:{BBBB}"), Some("items"))], "range": {"fromBeat": 0, "beats": 8}, "epoch": epoch}))
        .unwrap();
    assert_eq!(rig.until_ended("j13")["state"], "failed");
    assert!(!rig.folder().join("j13-dry.wav").exists());
}

// ----- the socket threads -----

#[test]
fn status_cancel_and_a_previews_stop_answer_while_the_main_thread_is_held() {
    use daw_bridge::handshake::{ClientHandshake, ClientStep, ServerConnection};
    use daw_bridge::wire::{Frame, Request};
    use std::io::{BufRead, BufReader, Write};
    const SECRET: &str = "kumi-test-secret-0123456789abcdefghijklmnopqrstuvwxyz";
    let dir = tempfile::tempdir().unwrap();
    let bridge = dir.path().join("bridges/reaper");
    std::fs::create_dir_all(&bridge).unwrap();
    daw_bridge::owner::write_atomic(&bridge.join("bridge.secret"), format!("{SECRET}\n").as_bytes()).unwrap();
    let config = crate::server::Config {
        secret_file: bridge.join("bridge.secret"),
        discovery_dir: bridge.join("discovery"),
        port: None,
        log_dir: None,
        connection: ServerConnection::new,
    };
    let board = Arc::new(Board::default());
    let mut running = board::Job::new("j1", board::Kind::Render, true);
    running.state = State::Running;
    board.add(running);
    // The main thread last ticked long ago (it is inside REAPER's render) and said so before it went in.
    let snapshot = Arc::new(Mutex::new(Snapshot {
        daw_version: "7.82".into(),
        last_tick: Some(std::time::Instant::now() - Duration::from_secs(30)),
        busy: Some(crate::reaper::Busy::KumiRender),
        listening: board.clone(),
        ..Snapshot::default()
    }));
    let (tx, _jobs) = queue::channel();
    let listener = std::net::TcpListener::bind((daw_bridge::loopback::BIND_ADDRESS, 0)).unwrap();
    let server = crate::server::Server::start(listener, config, crate::server::Hub::default(), tx, snapshot).unwrap();
    let mut stream = std::net::TcpStream::connect((daw_bridge::loopback::BIND_ADDRESS, server.port())).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = || {
        let mut text = String::new();
        reader.read_line(&mut text).unwrap();
        text
    };
    let (mut handshake, hello) = ClientHandshake::start(daw_bridge::auth::Secret::new(SECRET).unwrap(), "test");
    stream.write_all(&hello).unwrap();
    let welcome = loop {
        match handshake.on_line(line().trim_end()).unwrap() {
            ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
            ClientStep::Welcome(welcome) => break serde_json::to_value(*welcome).unwrap(),
        }
    };
    let methods: Vec<&str> = welcome["methods"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for method in ["render_start", "capture_start", "listen_status", "listen_cancel", "audition", "audition_stop", "audio_file", "scratch"]
    {
        assert!(methods.contains(&method), "{method} isn't offered: {methods:?}");
    }
    assert!(welcome["events"].as_array().unwrap().iter().any(|e| e == "scratch_swept"));
    let mut ask = |id: u64, method: &str, params: Value| -> Value {
        stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
        loop {
            let frame: Value = serde_json::from_str(line().trim_end()).unwrap();
            if frame["type"] == "response" {
                return frame;
            }
        }
    };
    let started = std::time::Instant::now();
    assert_eq!(ask(1, "listen_status", json!({"jobId": "j1"}))["result"]["state"], "running");
    assert_eq!(ask(2, "status", json!({}))["result"]["busy"], "rendering");
    let read = ask(3, "read", json!({"kind": "track"}));
    assert_eq!(read["error"]["code"], "busy");
    assert!(read["error"]["message"].as_str().unwrap().contains("rendering for Kumi"), "{read}");
    assert_eq!(ask(4, "audition_stop", json!({}))["result"], json!({"stopped": false}));
    assert!(started.elapsed() < Duration::from_secs(1), "each answered at once");
    // A cancel nobody can act on in time says it is still stopping.
    let cancel = ask(5, "listen_cancel", json!({"jobId": "j1"}));
    assert_eq!(cancel["result"], json!({"jobId": "j1", "state": "running", "stopping": true}));
    assert!(board.cancelled("j1"));
    server.stop("test over");
}

// ----- captures -----

fn playing_at(seconds: f64) {
    fake::with(|s| {
        s.play_bits = 1;
        s.play_position = seconds;
    });
}

#[test]
fn a_capture_takes_what_reaper_plays_for_its_seconds_and_marks_where_the_project_was() {
    let rig = Rig::new();
    playing_at(10.0);
    let started = rig.ask("capture_start", json!({"jobId": "c1", "seconds": 1})).unwrap();
    assert_eq!(started["files"][0]["file"], rig.folder().join("c1-master.wav").to_string_lossy().as_ref());
    assert_eq!(fake::with(|s| s.listening.hooks.len()), 1);
    for _ in 0..12 {
        fake::audio_block(4096, 2, |channel, _| if channel == 0 { 0.5 } else { -0.5 });
        rig.tick();
    }
    let status = rig.status("c1");
    assert_eq!(status["state"], "done", "{status}");
    let file = &status["files"][0];
    assert_eq!((file["seconds"].as_f64(), file["sampleRate"].as_u64(), file["partial"].as_bool()), (Some(1.0), Some(44100), Some(false)));
    assert_eq!(wav::read_info(Path::new(file["file"].as_str().unwrap())).unwrap().frames, 44100);
    // Frame 0 was the project at 10 s: beat 20 at 120 BPM.
    assert_eq!(status["marks"], json!([[0, 10.0, 20.0, true]]));
    assert_eq!(fake::with(|s| s.listening.hooks.len()), 0, "the hook is taken out at the end");
    let bytes = std::fs::read(file["file"].as_str().unwrap()).unwrap();
    assert_eq!((f32::from_le_bytes(bytes[44..48].try_into().unwrap()), f32::from_le_bytes(bytes[48..52].try_into().unwrap())), (0.5, -0.5));
}

#[test]
fn a_capture_ends_when_reaper_stops_and_marks_the_stop() {
    let rig = Rig::new();
    playing_at(4.0);
    rig.ask("capture_start", json!({"jobId": "c2", "seconds": 10})).unwrap();
    for _ in 0..3 {
        fake::audio_block(4410, 2, |_, _| 0.1);
        rig.tick();
    }
    fake::with(|s| s.play_bits = 0);
    fake::audio_block(4410, 2, |_, _| 0.0);
    rig.tick();
    let status = rig.status("c2");
    assert_eq!(status["state"], "done");
    assert_eq!(status["files"][0]["partial"], true);
    assert!(status["notes"][0].as_str().unwrap().starts_with("REAPER stopped after"), "{status}");
    let marks = status["marks"].as_array().unwrap();
    assert_eq!(marks.first().unwrap()[3], true);
    assert_eq!(marks.last().unwrap()[3], false, "the stop is marked");
}

#[test]
fn a_loop_wrap_is_marked_at_the_frame_the_loop_ended() {
    let rig = Rig::new();
    playing_at(10.0);
    fake::with(|s| {
        s.repeat = true;
        s.loop_range = (10.0, 11.0);
    });
    rig.ask("capture_start", json!({"jobId": "c3", "seconds": 3})).unwrap();
    for _ in 0..30 {
        fake::audio_block(4410, 2, |_, _| 0.1);
        fake::with(|s| {
            if s.play_position >= 11.0 {
                s.play_position -= 1.0;
            }
        });
        rig.tick();
        if rig.status("c3")["state"] == "done" {
            break;
        }
    }
    let status = rig.status("c3");
    let marks = status["marks"].as_array().unwrap();
    assert_eq!(marks[0], json!([0, 10.0, 20.0, true]));
    assert_eq!(marks[1], json!([44100, 10.0, 20.0, true]), "{marks:?}");
}

#[test]
fn a_capture_that_falls_behind_ends_at_the_gap_and_says_why() {
    let rig = Rig::new();
    playing_at(0.0);
    rig.ask("capture_start", json!({"jobId": "c4", "seconds": 40})).unwrap();
    // 28 s of audio while the main thread is held (no ticks): more than the ring holds (2^20 frames, 23.8 s at 44.1 kHz).
    for _ in 0..300 {
        fake::audio_block(4096, 2, |_, _| 0.2);
    }
    rig.tick();
    let status = rig.status("c4");
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!(status["files"][0]["partial"], true);
    assert!(status["notes"][0].as_str().unwrap().contains("fell behind"), "{status}");
    let seconds = status["files"][0]["seconds"].as_f64().unwrap();
    assert_eq!(seconds, crate::rows::seconds(1_048_576.0 / 44100.0), "the take ends where the gap began");
}

#[test]
fn a_capture_whose_device_gives_no_output_says_so_and_keeps_nothing() {
    let rig = Rig::new();
    playing_at(0.0);
    rig.ask("capture_start", json!({"jobId": "c7", "seconds": 4})).unwrap();
    // REAPER's dummy device: blocks come, with no output channel in them (measured in 7.82).
    for _ in 0..20 {
        fake::audio_block(512, 0, |_, _| 0.0);
    }
    rig.tick();
    let status = rig.status("c7");
    assert_eq!((status["state"].as_str(), status["code"].as_str()), (Some("failed"), Some("failed")));
    assert!(status["error"].as_str().unwrap().contains("no output Kumi can hear"), "{status}");
    assert!(!rig.folder().join("c7-master.wav").exists());
    assert_eq!(fake::with(|s| s.listening.hooks.len()), 0);
}

#[test]
fn a_cancelled_capture_keeps_what_it_has() {
    let rig = Rig::new();
    playing_at(0.0);
    fake::with(|s| s.listening.metronome = true);
    rig.ask("capture_start", json!({"jobId": "c5", "seconds": 20})).unwrap();
    fake::audio_block(22050, 2, |_, _| 0.3);
    rig.tick();
    rig.board().update("c5", |job| job.cancel = true);
    rig.tick();
    let status = rig.status("c5");
    assert_eq!((status["state"].as_str(), status["files"][0]["partial"].as_bool()), (Some("done"), Some(true)));
    let notes: Vec<&str> = status["notes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(
        notes.contains(&"REAPER's metronome is on: it is in the take") && notes.iter().any(|n| n.starts_with("Kumi stopped listening")),
        "{notes:?}"
    );
}

#[test]
fn a_capture_is_refused_when_reaper_isnt_playing_or_its_audio_isnt_running() {
    let rig = Rig::new();
    let (code, message) = rig.ask("capture_start", json!({"jobId": "c6", "seconds": 4})).unwrap_err();
    assert_eq!((code.as_str(), message.as_str()), ("failed", "REAPER isn't playing: give from_beat and beats, and Kumi renders the part"));
    playing_at(0.0);
    fake::with(|s| s.listening.audio = false);
    assert!(rig.ask("capture_start", json!({"jobId": "c6", "seconds": 4})).unwrap_err().1.contains("audio isn't running"));
    fake::with(|s| {
        s.listening.audio = true;
        s.listening.device = "Dummy Audio".into();
    });
    assert!(rig.ask("capture_start", json!({"jobId": "c6", "seconds": 4})).unwrap_err().1.contains("dummy device"));
    fake::with(|s| {
        s.listening.device = "CoreAudio Built-in Output".into();
        s.listening.hook_refuses = true;
    });
    assert!(rig.ask("capture_start", json!({"jobId": "c6", "seconds": 4})).unwrap_err().1.contains("hardware hook refused"));
    assert_eq!(rig.ask("capture_start", json!({"jobId": "c6", "seconds": 61})).unwrap_err().0, "bad-params");
    assert!(rig.board().active().is_none());
}

// ----- previews -----

fn wav_file(folder: &Path, name: &str, seconds: f64) -> String {
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(name);
    let mut writer = wav::Writer::create(&path, 2, 44100).unwrap();
    writer.write(&vec![0.1f32; (seconds * 44100.0) as usize * 2]).unwrap();
    writer.finish().unwrap();
    path.to_string_lossy().into_owned()
}

fn preview_ptr() -> *mut std::ffi::c_void {
    fake::with(|s| s.listening.previews.first().copied()).expect("a preview playing") as *mut std::ffi::c_void
}

#[test]
fn a_preview_plays_out_loud_and_stops_at_its_end_on_play_and_on_audition_stop() {
    let rig = Rig::new();
    let file = wav_file(&rig.folder(), "take.wav", 2.0);
    let answer = rig.ask("audition", json!({"previewId": "p1", "file": file, "fromSeconds": 0.5, "seconds": 1.0, "gainDb": -6.0})).unwrap();
    assert_eq!(answer, json!({"previewId": "p1", "playing": true, "seconds": 1.0, "via": "preview"}));
    let preview = preview_ptr();
    unsafe {
        assert_eq!(super::audition::kumi_preview_out_chan(preview), 0);
        assert!((super::audition::kumi_preview_volume(preview) - 10f64.powf(-6.0 / 20.0)).abs() < 1e-9);
    }
    rig.tick();
    assert_eq!(fake::with(|s| s.listening.previews.len()), 1, "still playing");
    unsafe { super::audition::kumi_preview_set_position(preview, 1.5) };
    rig.tick();
    assert!(fake::with(|s| s.listening.previews.is_empty()), "stopped at its end");
    assert!(fake::with(|s| s.listening.sources.iter().all(|source| source.destroyed)));
    // The producer presses play: it stops.
    rig.ask("audition", json!({"previewId": "p2", "file": file})).unwrap();
    fake::with(|s| s.play_bits = 1);
    rig.tick();
    assert!(fake::with(|s| s.listening.previews.is_empty()));
    fake::with(|s| s.play_bits = 0);
    // Kumi stops it from a socket thread: silent at once, stopped on the next tick.
    rig.ask("audition", json!({"previewId": "p3", "file": file})).unwrap();
    let preview = preview_ptr();
    assert_eq!(rig.board().stop_preview(&json!({"previewId": "other"})).unwrap(), json!({"stopped": false}));
    assert_eq!(rig.board().stop_preview(&json!({})).unwrap(), json!({"stopped": true}));
    assert_eq!(unsafe { super::audition::kumi_preview_volume(preview) }, 0.0);
    assert_eq!(rig.board().stop_preview(&json!({})).unwrap(), json!({"stopped": false}), "asked once already");
    rig.tick();
    assert!(fake::with(|s| s.listening.previews.is_empty()));
    // A new one stops the one playing.
    rig.ask("audition", json!({"previewId": "p4", "file": file})).unwrap();
    rig.ask("audition", json!({"previewId": "p5", "file": file})).unwrap();
    assert_eq!(fake::with(|s| s.listening.previews.len()), 1);
}

#[test]
fn a_render_stops_the_audition_playing_and_says_so() {
    let rig = Rig::new();
    let file = wav_file(&rig.folder(), "take.wav", 4.0);
    rig.ask("audition", json!({"previewId": "p1", "file": file})).unwrap();
    rig.ask(
        "render_start",
        json!({"jobId": "ja1", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}),
    )
    .unwrap();
    rig.tick();
    let status = rig.status("ja1");
    assert_eq!(status["state"], "done");
    assert!(status["notes"][0].as_str().unwrap().contains("audition playing was stopped"), "{status}");
    assert!(fake::with(|s| s.listening.previews.is_empty()));
    assert!(rig.board().playing().is_none());
}

#[test]
fn a_preview_and_a_scratch_set_never_ask_a_closed_tabs_project_anything() {
    let (rig, kick) = Rig::copyable();
    let file = wav_file(&rig.folder(), "take.wav", 4.0);
    make(&rig, "s4", &kick, 1);
    let copy = fake::with(|s| s.tracks[4].guid.clone());
    rig.ask("audition", json!({"previewId": "p1", "file": file})).unwrap();
    // The producer closes the tab (another project takes its place): its pointer is freed.
    fake::with(|s| s.project_id += 1);
    rig.tick();
    assert!(rig.board().playing().is_none(), "the preview stopped with its project");
    let set = rig.ask("scratch", json!({"action": "set", "scratchId": "s4", "values": [{"ref": format!("r:track:{copy}"), "device": 0, "param": 0, "normalized": 0.3}]}));
    assert_eq!(set.unwrap_err().0, "gone");
    assert_eq!(fake::with(|s| s.freed_project_uses), 0, "nothing asked of the closed project");
}

#[test]
fn a_preview_goes_to_the_masters_hardware_output() {
    let rig = Rig::new();
    let file = wav_file(&rig.folder(), "take.wav", 1.0);
    fake::with(|s| s.master.hw_outs = vec![FakeSend { dst_chan: 2 | 1024, ..FakeSend::to(0) }]);
    rig.ask("audition", json!({"previewId": "p1", "file": file})).unwrap();
    assert_eq!(unsafe { super::audition::kumi_preview_out_chan(preview_ptr()) }, 2 | 1024);
}

#[test]
fn audition_says_why_when_it_cant_play() {
    let rig = Rig::new();
    let file = wav_file(&rig.folder(), "take.wav", 1.0);
    let refused = |params: Value| rig.ask("audition", params).unwrap_err();
    // A device that isn't running is the producer's choice (freed for another app, say): never opened, only said.
    fake::with(|s| {
        s.listening.audio = false;
        s.listening.device_opens = true;
    });
    let (_, why) = refused(json!({"previewId": "p1", "file": file}));
    assert!(why.contains("audio isn't running") && why.contains("start REAPER's audio"), "{why}");
    assert_eq!(fake::with(|s| s.listening.inits), 0, "Kumi never opens the producer's device");
    // Right after Kumi's own render (REAPER closes its device to render, and reopens it a moment after), it's opened.
    fake::with(|s| s.listening.audio = true);
    rig.ask(
        "render_start",
        json!({"jobId": "jr1", "sources": [source("mix", "master", None, None)], "range": {"fromBeat": 0, "beats": 4}}),
    )
    .unwrap();
    rig.tick();
    assert_eq!(rig.status("jr1")["state"], "done");
    fake::with(|s| s.listening.audio = false);
    assert!(rig.ask("audition", json!({"previewId": "p0", "file": file})).is_ok());
    assert_eq!(fake::with(|s| s.listening.inits), 1);
    rig.board().stop_preview(&json!({})).unwrap();
    rig.tick();
    fake::with(|s| {
        s.listening.audio = true;
        s.play_bits = 1;
    });
    assert!(refused(json!({"previewId": "p1", "file": file})).1.contains("REAPER is playing"));
    fake::with(|s| s.play_bits = 0);
    assert!(refused(json!({"previewId": "p1", "file": "/nowhere/x.wav"})).1.contains("isn't there"));
    let text = rig.folder().join("notes.txt");
    std::fs::write(&text, "not audio").unwrap();
    assert_eq!(refused(json!({"previewId": "p1", "file": text})).1, "the file isn't audio REAPER reads");
    assert_eq!(refused(json!({"previewId": "p1", "file": file, "fromSeconds": 5})).0, "bad-params");
    assert_eq!(refused(json!({"previewId": "p1", "file": file, "gainDb": 30})).0, "bad-params");
    assert_eq!(refused(json!({"file": file})).0, "bad-params");
    fake::with(|s| s.listening.preview_refuses = true);
    assert!(refused(json!({"previewId": "p1", "file": file})).1.contains("wouldn't play"));
    assert!(fake::with(|s| s.listening.sources.iter().all(|source| source.destroyed)), "every source made is freed");
}

// ----- FX rows -----

#[test]
fn fx_rows_say_their_track_take_and_whether_they_are_plug_ins_from_outside_reaper() {
    let rig = Rig::new();
    fake::with(|s| {
        let mut pro_q = FakeFx::new("{FX-PQ}", "VST3: Pro-Q 3 (FabFilter)");
        pro_q.renamed = "Kick EQ".into();
        s.tracks[1].fx = vec![pro_q];
        s.tracks[1].input_fx = vec![FakeFx::new("{FX-IN}", "JS: Volume Adjustment")];
        s.tracks[1].items[0].takes[0].fx = vec![FakeFx::new("{FX-TK}", "AUi: DLSMusicDevice (Apple)")];
        s.state_count += 1;
    });
    rig.tick();
    let one = |reference: &str| {
        let answer = rig.ask("read", json!({"ref": reference})).unwrap_or_else(|e| panic!("{reference}: {e:?}"));
        assert!(daw_bridge::contract::misplaced_refs(&answer).is_empty(), "{answer}");
        answer["items"][0].clone()
    };
    let pro_q = one("r:fx:{FX-PQ}");
    assert_eq!(
        (&pro_q["trackRef"], &pro_q["plugin"], &pro_q["name"], &pro_q["originalName"]),
        (&json!("r:track:{BBBB}"), &json!(true), &json!("Kick EQ"), &json!("VST3: Pro-Q 3 (FabFilter)"))
    );
    let input = one("r:fx:{FX-IN}");
    assert_eq!((&input["trackRef"], &input["input"], &input["plugin"]), (&json!("r:track:{BBBB}"), &json!(true), &json!(false)));
    assert_eq!(input.get("originalName"), None, "not renamed");
    let take = one("r:fx:{FX-TK}");
    assert_eq!(
        (&take["trackRef"], &take["takeRef"], &take["plugin"]),
        (&json!("r:track:{BBBB}"), &json!("r:take:{TAKE-K1}"), &json!(true))
    );
    // An instrument says so (its type ends in "i"), so a gain Kumi adds goes after it; an effect says nothing.
    assert_eq!(take["instrument"], json!(true), "{take}");
    assert_eq!(pro_q.get("instrument"), None, "{pro_q}");
    // A page: REAPER's own ReaEQ (VST3 by Cockos) and a container are no plug-ins from outside.
    let page = rig.ask("read", json!({"kind": "fx", "parent": "r:track:{AAAA}"})).unwrap();
    assert!(daw_bridge::contract::misplaced_refs(&page).is_empty(), "{page}");
    let rows = page["items"].as_array().unwrap();
    assert_eq!(
        rows.iter().map(|r| (r["ref"].clone(), r["plugin"].clone(), r["trackRef"].clone())).collect::<Vec<_>>(),
        [(json!("r:fx:{FX-EQ}"), json!(false), json!("r:track:{AAAA}")), (json!("r:fx:{FX-C}"), json!(false), json!("r:track:{AAAA}")),]
    );
}

// ----- the audio behind an item -----

#[test]
fn audio_file_names_the_file_a_take_plays_and_the_part_it_plays() {
    let rig = Rig::new();
    let file = wav_file(&rig.folder(), "kick.wav", 4.0);
    let media = FakeMedia { path: file.clone(), seconds: 4.0, channels: 2, rate: 44100, section: None };
    fake::with(|s| {
        s.listening.take_files.insert("{TAKE-K1}".into(), media.clone());
        s.tracks[1].items[0].takes[0].extra.insert("D_STARTOFFS".into(), 0.5);
    });
    let epoch = rig.epoch();
    let answer = rig.ask("audio_file", json!({"ref": "r:item:{ITEM-K1}", "epoch": epoch, "jobId": "j1"})).unwrap();
    assert_eq!(
        answer,
        json!({"file": file, "seconds": 4.0, "channels": 2, "sampleRate": 44100, "kind": "source", "name": "kick.wav", "offset": 0.5, "length": 2.0})
    );
    assert!(daw_bridge::contract::misplaced_refs(&answer).is_empty());
    // A take by its own ref; a whole file played has no part.
    fake::with(|s| {
        s.tracks[1].items[0].takes[0].extra.clear();
        s.tracks[1].items[0].length = 4.0;
    });
    let whole = rig.ask("audio_file", json!({"ref": "r:take:{TAKE-K1}", "epoch": epoch})).unwrap();
    assert_eq!((whole.get("offset"), whole.get("length")), (None, None));
    // A section of a file, reversed: its offset is the section's, and it says reversed.
    fake::with(|s| s.listening.take_files.insert("{TAKE-K1}".into(), FakeMedia { section: Some((1.0, 2.0, true)), ..media.clone() }));
    let section = rig.ask("audio_file", json!({"ref": "r:take:{TAKE-K1}", "epoch": epoch})).unwrap();
    assert_eq!((section["offset"].as_f64(), section["reversed"].as_bool()), (Some(1.0), Some(true)));
    // What has no file of its own says so in REAPER's words.
    let refused = |reference: &str| rig.ask("audio_file", json!({"ref": reference, "epoch": epoch})).unwrap_err();
    assert_eq!(
        refused("r:item:{ITEM-S1}").1,
        "That's a MIDI item, which has no sound of its own: listen to its track instead (Kumi renders it)"
    );
    fake::with(|s| s.listening.take_files.insert("{TAKE-K2}".into(), FakeMedia { path: "/gone/kick.wav".into(), ..media.clone() }));
    assert!(refused("r:item:{ITEM-K2}").1.contains("offline media"));
    fake::with(|s| s.tracks[1].items[1].takes.clear());
    assert!(refused("r:item:{ITEM-K2}").1.contains("empty"));
    assert_eq!(refused("r:item:{NOPE}").0, "gone");
    assert_eq!(refused("r:track:{BBBB}").0, "bad-params");
    assert_eq!(rig.ask("audio_file", json!({"ref": "r:item:{ITEM-K1}", "epoch": "e-old"})).unwrap_err().0, "stale");
}

// ----- spans and grids -----

#[test]
fn spans_list_each_tracks_stretches_in_beats_merged_and_paged() {
    let rig = Rig::new();
    fake::with(|s| {
        // A muted item and one on a lane that doesn't play are not heard.
        s.tracks[3].items = vec![fake::FakeItem::new("{ITEM-R1}", 20.0, 2.0, vec![fake::FakeTake::wave("{TAKE-R1}", "r.wav")])];
        s.tracks[3].items[0].mute = true;
        s.tracks[0].extra.insert("I_FREEMODE".into(), 2.0);
        s.tracks[0].items = vec![fake::FakeItem::new("{ITEM-D1}", 30.0, 1.0, vec![fake::FakeTake::wave("{TAKE-D1}", "d.wav")])];
        s.tracks[0].items[0].extra.insert("C_LANEPLAYS".into(), 0.0);
        s.tracks[2].mute = true;
    });
    let epoch = rig.epoch();
    let page = rig.ask("read", json!({"kind": "spans", "epoch": epoch})).unwrap();
    let rows = page["items"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1], json!({"ref": "r:track:{BBBB}", "name": "Kick", "spans": [[0.0, 8.0]]}), "two items that touch are one stretch");
    assert_eq!(rows[2], json!({"ref": "r:track:{CCCC}", "name": "Snare", "spans": [[16.0, 24.0]], "mute": true}));
    assert_eq!(rows[0]["spans"], json!([]));
    assert_eq!(rows[3]["spans"], json!([]));
    assert_eq!(page["total"], 4);
    assert!(daw_bridge::contract::misplaced_refs(&page).is_empty());
    let first = rig.ask("read", json!({"kind": "spans", "limit": 2})).unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let second = rig.ask("read", json!({"kind": "spans", "cursor": first["nextCursor"]})).unwrap();
    assert_eq!(second["items"][0]["name"], "Snare");
    assert_eq!(rig.ask("read", json!({"kind": "spans", "limit": 201})).unwrap_err().0, "bad-params");
    assert_eq!(rig.ask("read", json!({"kind": "spans", "epoch": "e-old"})).unwrap_err().0, "stale");
}

#[test]
fn a_grid_prints_a_parameter_across_its_range_without_setting_it() {
    let rig = Rig::new();
    fake::with(|s| {
        let mut fx = FakeFx::new("{FX-G}", "VST: ReaComp (Cockos)");
        fx.params = vec![
            FakeParam::linear("Gain", 0.5, -12.0, 12.0, 1, "dB"),
            FakeParam::steps("Mode", 0.0, &["Low", "Mid", "High"]),
            FakeParam::steps("Bypass", 0.0, &["Off", "On"]),
            FakeParam::new("Meter", 0.3, "-4 dB"),
        ];
        s.tracks[3].fx = vec![fx];
    });
    let page = rig.ask("read", json!({"kind": "param", "parent": "r:fx:{FX-G}", "grid": 3})).unwrap();
    let rows = page["items"].as_array().unwrap();
    assert_eq!(rows[0]["grid"], json!([[0.0, "-12.0 dB"], [0.5, "0.0 dB"], [1.0, "12.0 dB"]]));
    assert_eq!(rows[1]["items"], json!(["Low", "Mid", "High"]));
    assert_eq!(rows[1].get("grid"), None);
    assert_eq!(rows[2]["items"], json!(["Off", "On"]));
    assert_eq!(rows[3]["grid"], json!([[0.3, "-4 dB"]]), "a plug-in that prints only its current value");
    assert_eq!(rows[3]["gridShort"], true);
    assert_eq!(fake::with(|s| s.tracks[3].fx[0].params[0].value), 0.5, "nothing was set");
    let one = rig.ask("read", json!({"ref": "r:param:{FX-G}:0", "grid": 2})).unwrap();
    assert_eq!(one["items"][0]["grid"], json!([[0.0, "-12.0 dB"], [1.0, "12.0 dB"]]));
    for bad in [json!(1), json!(65), json!("49")] {
        assert_eq!(rig.ask("read", json!({"kind": "param", "parent": "r:fx:{FX-G}", "grid": bad})).unwrap_err().0, "bad-params");
    }
}

// ----- status -----

#[test]
fn status_says_what_listening_can_do_now() {
    let rig = Rig::new();
    rig.tick();
    let flags = rig.ext.snapshot().status["listening"].clone();
    assert_eq!(
        flags,
        json!({"render": true, "capture": true, "audition": true, "grid": true, "scratch": true, "deliver": true, "version": 1})
    );
    fake::with(|s| s.listening.audio = false);
    rig.tick();
    let flags = rig.ext.snapshot().status["listening"].clone();
    assert_eq!(
        (flags["capture"].as_bool(), flags["audition"].as_bool(), flags["render"].as_bool()),
        (Some(false), Some(false), Some(true))
    );
    // REAPER's dummy device runs the audio engine with nothing behind it: nothing to hear, nothing heard.
    fake::with(|s| {
        s.listening.audio = true;
        s.listening.device = "Dummy Audio".into();
    });
    rig.tick();
    let flags = rig.ext.snapshot().status["listening"].clone();
    assert_eq!((flags["capture"].as_bool(), flags["audition"].as_bool()), (Some(false), Some(false)));
    // A bridge that doesn't know Kumi's folder yet renders nothing.
    let (_tx, rx) = queue::channel();
    let bare = Extension::new(fake::api(), rx, Box::new(Recorder::default()), Arc::new(Mutex::new(Snapshot::default())));
    bare.tick(Driver::Timer);
    let flags = bare.snapshot().status["listening"].clone();
    assert_eq!((flags["render"].as_bool(), flags["scratch"].as_bool(), flags["grid"].as_bool()), (Some(false), Some(false), Some(true)));
    assert_eq!(flags["deliver"], false, "a delivery is a render");
}

// ----- scratch copies -----

/// REAPER's ids are GUIDs (`{8-4-4-4-12}`): the lab project's short ones made so, for the copies' rewrite to see them.
/// Returns a track's ref by its old id. Called before the extension first looks (`Rig::with_project`).
fn real_ids() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut n = 0u32;
    let mut fresh = |old: &mut String, map: &mut std::collections::HashMap<String, String>| {
        n += 1;
        let new = format!("{{{n:08X}-0000-4000-8000-00000000{n:04X}}}");
        map.insert(old.clone(), new.clone());
        *old = new;
    };
    fake::with(|s| {
        for track in s.tracks.iter_mut() {
            fresh(&mut track.guid, &mut map);
            for fx in track.fx.iter_mut() {
                fresh(&mut fx.guid, &mut map);
            }
            for item in track.items.iter_mut() {
                fresh(&mut item.guid, &mut map);
                for take in item.takes.iter_mut() {
                    fresh(&mut take.guid, &mut map);
                }
            }
        }
    });
    map
}

/// A track with an FX, two audio items and a receive, in a project where chunks keep the GUIDs written in them; ids made
/// GUIDs. Returns Kick's ref.
fn copyable() -> String {
    fake::with(|s| {
        s.listening.chunk_guids = true;
        s.tracks[1].fx = vec![{
            let mut fx = FakeFx::new("{FX-KICK}", "VST: ReaEQ (Cockos)");
            fx.params = vec![FakeParam::linear("Gain", 0.5, -12.0, 12.0, 1, "dB")];
            fx
        }];
        s.tracks[1].selected = true;
        s.tracks[1].solo = 2;
        s.tracks[1].arm = true;
        s.tracks[1].hw_outs = vec![FakeSend::to(0)];
    });
    let ids = real_ids();
    format!("r:track:{}", ids["{BBBB}"])
}

#[test]
fn the_copyable_project_is_the_one_the_bridge_first_saw_when_it_reads_the_tracks_again() {
    let (rig, _) = Rig::copyable();
    let epoch = rig.epoch();
    // A change REAPER counts, and the bridge's next look at the tracks (a slow runner's test gets there by itself).
    fake::producer_edit("Track volume");
    std::thread::sleep(crate::extension::WATCH_REFRESH);
    assert_eq!(rig.epoch(), epoch, "the same project: refs read before still hold");
}

fn make(rig: &Rig, id: &str, reference: &str, count: u64) -> Value {
    let epoch = rig.epoch();
    rig.ask("scratch", json!({"action": "make", "scratchId": id, "ref": reference, "count": count, "label": "tune", "epoch": epoch}))
        .unwrap()
}

#[test]
fn a_scratch_copy_has_every_id_new_and_reaches_nothing_of_the_producers() {
    let (rig, kick) = Rig::copyable();
    let epoch = rig.epoch();
    let made = make(&rig, "s1k9z", &kick, 3);
    assert_eq!(made["undo"], json!({"steps": 0}));
    assert_eq!(made["epoch"], epoch.as_str(), "the copies don't move the epoch");
    let copies = made["copies"].as_array().unwrap();
    assert_eq!(
        copies.iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["Kumi · try 1k9z 1", "Kumi · try 1k9z 2", "Kumi · try 1k9z 3"]
    );
    assert!(daw_bridge::contract::misplaced_refs(&made).is_empty());
    fake::with(|s| {
        assert_eq!(s.tracks.len(), 7, "after the last track");
        let source = s.tracks[1].clone();
        let mut seen: Vec<String> = vec![source.guid.clone(), source.fx[0].guid.clone()];
        seen.extend(source.items.iter().map(|i| i.guid.clone()));
        seen.extend(source.items.iter().flat_map(|i| i.takes.iter().map(|t| t.guid.clone())));
        for (n, copy) in s.tracks[4..].iter().enumerate() {
            assert_eq!(format!("r:track:{}", copy.guid), copies[n]["ref"].as_str().unwrap());
            let mut ids = vec![copy.guid.clone(), copy.fx[0].guid.clone()];
            ids.extend(copy.items.iter().map(|i| i.guid.clone()));
            ids.extend(copy.items.iter().flat_map(|i| i.takes.iter().map(|t| t.guid.clone())));
            assert_eq!(ids.len(), 6);
            for id in ids {
                assert!(!seen.contains(&id), "{id} is shared");
                seen.push(id);
            }
            // Its sound is the source's: the same FX and items.
            assert_eq!((copy.fx[0].name.as_str(), copy.items.len()), ("VST: ReaEQ (Cockos)", 2));
            assert_eq!(
                (copy.main_send, copy.solo, copy.arm, copy.mute, copy.selected, copy.folder, copy.hw_outs.len()),
                (false, 0, false, false, false, 0, 0)
            );
            assert_eq!(
                s.listening.track_ext.get(&(copy.guid.clone(), "P_EXT:kumi_scratch".to_string())).map(String::as_str),
                Some(format!("s1k9z {}", n + 1).as_str())
            );
        }
        assert!(s.tracks[1].selected && s.tracks[1].solo == 2, "the source is as it was");
    });
    // Noted in Kumi's folder.
    let journal = scratch::read_journal(&rig.folder());
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].copies.len(), 3);
    assert_eq!(format!("r:track:{}", journal[0].source), kick);
}

#[test]
fn a_folders_copy_hears_its_tracks_through_sends_that_go_with_it() {
    let mut ids = Default::default();
    let rig = Rig::with_project(|| {
        fake::with(|s| s.listening.chunk_guids = true);
        ids = real_ids();
    });
    make(&rig, "s2", &format!("r:track:{}", ids["{AAAA}"]), 1);
    fake::with(|s| {
        assert_eq!(s.tracks.len(), 5);
        let copy = 4;
        assert!(s.tracks[1].sends.iter().any(|send| send.dest == copy) && s.tracks[2].sends.iter().any(|send| send.dest == copy));
    });
    let dropped = rig.ask("scratch", json!({"action": "drop", "scratchId": "s2"})).unwrap();
    assert_eq!((dropped["dropped"].as_u64(), dropped["left"].as_u64()), (Some(1), Some(0)));
    fake::with(|s| assert!(s.tracks[1].sends.is_empty() && s.tracks[2].sends.is_empty(), "the sends went with the copy"));
}

#[test]
fn copies_go_above_the_first_track_when_the_last_leaves_a_folder_open() {
    let mut ids = Default::default();
    let rig = Rig::with_project(|| {
        fake::with(|s| {
            s.listening.chunk_guids = true;
            s.tracks[3].folder = 1;
        });
        ids = real_ids();
    });
    let made = make(&rig, "s3", &format!("r:track:{}", ids["{BBBB}"]), 2);
    assert!(made["notes"][0].as_str().unwrap().contains("above the first track"));
    fake::with(|s| {
        assert!(s.tracks[0].name.starts_with("Kumi · try") && s.tracks[1].name.starts_with("Kumi · try") && s.tracks[2].name == "Drums")
    });
}

#[test]
fn a_make_reaper_would_give_the_sources_ids_is_refused_whole() {
    let rig = Rig::new();
    // This REAPER's chunks don't keep the GUIDs written in them: a copy would be the source's twin.
    let epoch = rig.epoch();
    let refused = rig
        .ask("scratch", json!({"action": "make", "scratchId": "s4", "ref": "r:track:{BBBB}", "count": 2, "label": "tune", "epoch": epoch}))
        .unwrap_err();
    assert!(refused.1.contains("kept an id") && refused.1.contains("no copies were made"), "{refused:?}");
    assert_eq!(fake::with(|s| s.tracks.len()), 4);
    assert!(scratch::read_journal(&rig.folder()).is_empty());
}

#[test]
fn scratch_set_turns_a_copys_knob_and_nothing_else() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "s5", &kick, 2);
    let copy = made["copies"][1]["ref"].as_str().unwrap().to_string();
    let set = |values: Value| rig.ask("scratch", json!({"action": "set", "scratchId": "s5", "values": values}));
    assert_eq!(set(json!([{"ref": copy, "device": 0, "param": 0, "normalized": 0.3}])).unwrap(), json!({"set": 1, "undo": {"steps": 0}}));
    fake::with(|s| {
        assert_eq!(s.tracks[5].fx[0].params[0].value, 0.3);
        assert_eq!((s.tracks[1].fx[0].params[0].value, s.tracks[4].fx[0].params[0].value), (0.5, 0.5));
    });
    let refused = |values: Value| set(values).unwrap_err();
    assert!(refused(json!([{"ref": copy, "device": 0, "param": 0, "value": 3}])).1.contains("normalized"));
    assert_eq!(refused(json!([{"ref": copy, "device": 0, "param": 0, "normalized": 1.5}])).0, "bad-params");
    assert_eq!(refused(json!([{"ref": copy, "device": 4, "param": 0, "normalized": 0.1}])).0, "bad-params");
    assert_eq!(refused(json!([{"ref": copy, "device": 0, "param": 9, "normalized": 0.1}])).0, "bad-params");
    assert!(refused(json!([{"ref": kick, "device": 0, "param": 0, "normalized": 0.1}])).1.contains("isn't one of"));
    // Whole or nothing: one bad value, nothing set.
    assert_eq!(
        refused(
            json!([{"ref": copy, "device": 0, "param": 0, "normalized": 0.9}, {"ref": copy, "device": 0, "param": 9, "normalized": 0.1}])
        )
        .0,
        "bad-params"
    );
    assert_eq!(fake::with(|s| s.tracks[5].fx[0].params[0].value), 0.3);
    // A copy whose FX is another now, or that is gone.
    fake::with(|s| s.tracks[5].fx[0].guid = "{OTHER}".into());
    assert_eq!(refused(json!([{"ref": copy, "device": 0, "param": 0, "normalized": 0.1}])).0, "changed");
    fake::with(|s| {
        s.tracks.remove(5);
    });
    assert_eq!(refused(json!([{"ref": copy, "device": 0, "param": 0, "normalized": 0.1}])).0, "gone");
    assert_eq!(rig.ask("scratch", json!({"action": "set", "scratchId": "nope", "values": [{}]})).unwrap_err().0, "bad-params");
}

#[test]
fn a_drop_takes_the_copies_away_leaves_one_the_producer_renamed_and_puts_the_changed_flag_back() {
    let (rig, kick) = Rig::copyable();
    fake::with(|s| s.dirty = false);
    let made = make(&rig, "s6", &kick, 3);
    assert!(fake::with(|s| s.tracks.len()) == 7);
    fake::with(|s| {
        s.dirty = true;
        s.tracks[5].name = "Kick alt".into();
    });
    let dropped = rig.ask("scratch", json!({"action": "drop", "scratchId": "s6"})).unwrap();
    assert_eq!((dropped["dropped"].as_u64(), dropped["left"].as_u64()), (Some(2), Some(1)));
    assert_eq!(dropped["notes"], json!(["“Kumi · try s6 2” was renamed “Kick alt”: Kumi left it as yours"]));
    assert_eq!(
        fake::with(|s| s.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
        ["Drums", "Kick", "Snare", "Reverb", "Kick alt"]
    );
    // A copy the producer kept: the project stays marked changed.
    assert!(fake::with(|s| s.dirty));
    // The journal keeps the copies Kumi removed (an undo may bring them back), not the one the producer kept.
    let journal = scratch::read_journal(&rig.folder());
    assert_eq!(
        journal.iter().flat_map(|e| e.copies.iter().map(|(_, name)| name.clone())).collect::<Vec<_>>(),
        ["Kumi · try s6 1", "Kumi · try s6 3"]
    );
    let _ = made;
    // Asked again: nothing stands.
    assert_eq!(
        rig.ask("scratch", json!({"action": "drop", "scratchId": "s6"})).unwrap(),
        json!({"dropped": 0, "left": 0, "undo": {"steps": 0}})
    );
    // All dropped and nothing else changed: the project is as unchanged as it was.
    fake::with(|s| s.dirty = false);
    make(&rig, "s7", &kick, 2);
    fake::with(|s| s.dirty = true);
    rig.ask("scratch", json!({"action": "drop", "scratchId": "s7"})).unwrap();
    assert!(!fake::with(|s| s.dirty));
}

#[test]
fn copies_reapers_undo_brings_back_after_a_drop_are_swept_again_and_the_host_told() {
    let (rig, kick) = Rig::copyable();
    make(&rig, "s5", &kick, 2);
    // The producer moves a fader: REAPER's point holds the whole state, copies included.
    fake::with(|s| {
        s.undo_entries += 1;
        s.undo_cur = None;
    });
    rig.tick();
    let with_copies: Vec<fake::FakeTrack> = fake::with(|s| s.tracks.clone());
    rig.ask("scratch", json!({"action": "drop", "scratchId": "s5"})).unwrap();
    assert_eq!(fake::with(|s| s.tracks.len()), 4);
    // Cmd-Z for the fader brings the copies back with it.
    fake::with(|s| {
        s.tracks = with_copies.clone();
        s.undo_cur = Some(s.undo_entries - 2);
    });
    rig.tick();
    assert_eq!(fake::with(|s| s.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()), ["Drums", "Kick", "Snare", "Reverb"]);
    let swept: Vec<Value> = rig.out.events().into_iter().filter(|(name, _)| name == "scratch_swept").map(|(_, data)| data).collect();
    assert_eq!(swept, [json!({"count": 2, "names": ["Kumi · try s5 1", "Kumi · try s5 2"], "why": "undo"})]);
    // A redo with them gone: nothing more to sweep, nothing told.
    fake::with(|s| s.undo_cur = None);
    rig.tick();
    assert_eq!(rig.out.events().iter().filter(|(name, _)| name == "scratch_swept").count(), 1);
}

#[test]
fn a_drop_after_an_edit_of_the_producers_leaves_the_project_marked_changed() {
    let (rig, kick) = Rig::copyable();
    fake::with(|s| s.dirty = false);
    make(&rig, "s8", &kick, 1);
    fake::with(|s| {
        s.dirty = true;
        s.state_count += 1;
    });
    rig.ask("scratch", json!({"action": "drop", "scratchId": "s8"})).unwrap();
    assert!(fake::with(|s| s.dirty));
}

#[test]
fn copies_a_crash_left_are_swept_when_the_bridge_loads_and_the_host_is_told() {
    let (rig, kick) = Rig::copyable();
    make(&rig, "s9", &kick, 2);
    let names: Vec<String> = fake::with(|s| s.tracks[4..].iter().map(|t| t.name.clone()).collect());
    // The producer renamed the second; then REAPER crashed (the bridge never dropped them).
    fake::with(|s| s.tracks[5].name = "Mine now".into());
    let folder = rig.folder();
    let (_tx, rx) = queue::channel();
    let out = Recorder::default();
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let fresh = Extension::new(fake::api(), rx, Box::new(out.clone()), snapshot);
    fresh.set_listening_folder(folder.clone());
    fresh.tick(Driver::Timer);
    assert_eq!(out.events(), [("scratch_swept".to_string(), json!({"count": 1, "names": [names[0]]}))]);
    // A host that connects after the sweep reads it in the status.
    fresh.tick(Driver::Timer);
    assert_eq!(fresh.snapshot().status["listening"]["swept"], json!({"count": 1, "names": [names[0]]}));
    assert_eq!(
        fake::with(|s| s.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
        ["Drums", "Kick", "Snare", "Reverb", "Mine now"]
    );
    // The project's file still holds the copy Kumi removed: the project is marked changed, and the make is kept until the
    // saved file is seen without it. The copy the producer renamed is theirs, no longer looked for.
    assert!(fake::with(|s| s.dirty));
    let journal = scratch::read_journal(&folder);
    assert_eq!((journal.len(), journal[0].copies.len()), (1, 1));
    // Not saved, reopened: still looked for. Saved (clean) and looked at again with nothing left in it: forgotten.
    let look = || {
        let (_tx, rx) = queue::channel();
        let again = Extension::new(fake::api(), rx, Box::new(Recorder::default()), Arc::new(Mutex::new(Snapshot::default())));
        again.set_listening_folder(folder.clone());
        again.tick(Driver::Timer);
    };
    look();
    assert_eq!(scratch::read_journal(&folder).len(), 1, "the project is changed: its file may still hold the copy");
    fake::with(|s| s.dirty = false);
    look();
    assert!(scratch::read_journal(&folder).is_empty(), "the clean file holds none: forgotten");
}

// ----- scratch-keep -----

fn change(rig: &Rig, method: &str, params: Value) -> Answer {
    let mut params = params;
    params["epoch"] = rig.epoch().into();
    rig.ask(method, params)
}

fn keep_input(copy: &str) -> Value {
    json!({"ref": copy, "name": "Kumi · Goal best"})
}

#[test]
fn scratch_keep_makes_a_copy_the_producers_and_kumis_undo_takes_it_away() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sa", &kick, 2);
    let copy = made["copies"][0]["ref"].as_str().unwrap().to_string();
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    assert_eq!(preview["display"]["title"], "Kept “Kumi · Goal best”");
    assert_eq!(preview["target"], json!({"ref": copy, "kind": "track"}));
    let applied = change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k1)"}),
    )
    .unwrap();
    assert_eq!(applied["created"], json!({"ref": copy, "kind": "track"}));
    assert_eq!(applied["undo"]["entry"], true, "one undo point");
    fake::with(|s| {
        let kept = &s.tracks[4];
        assert_eq!((kept.name.as_str(), kept.main_send), ("Kumi · Goal best", true));
        assert!(!s.listening.track_ext.contains_key(&(kept.guid.clone(), "P_EXT:kumi_scratch".to_string())), "out of the scratch copies");
    });
    for answer in [&preview, &applied] {
        assert!(daw_bridge::contract::misplaced_refs(answer).is_empty(), "{answer}");
    }
    // The drop leaves the kept track, and takes the other copy.
    let dropped = rig.ask("scratch", json!({"action": "drop", "scratchId": "sa"})).unwrap();
    assert_eq!((dropped["dropped"].as_u64(), dropped["left"].as_u64()), (Some(1), Some(0)));
    assert_eq!(fake::with(|s| s.tracks.len()), 5);
    // Kumi's undo: a new step that deletes it.
    let restore = json!({"kind": "scratch-keep", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo Kept (k2)"});
    let undone = change(&rig, "change_restore", restore.clone()).unwrap();
    assert_eq!(undone["restored"], json!({"deletedRef": copy}));
    assert_eq!(undone["display"]["title"], "“Kumi · Goal best” taken away again");
    assert!(daw_bridge::contract::misplaced_refs(&undone).is_empty());
    assert_eq!(fake::with(|s| s.tracks.len()), 4);
    // Asked again, it is gone: nothing is written.
    assert_eq!(change(&rig, "change_restore", restore).unwrap_err().0, "gone");
}

#[test]
fn a_goal_keeps_its_best_copy_and_the_drop_takes_every_other_one() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sg", &kick, 3);
    // The best is the last copy: kept, it sits after the others with no scratch mark.
    let best = made["copies"][2]["ref"].as_str().unwrap().to_string();
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&best)})).unwrap();
    change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&best), "fence": preview["fence"], "desc": "Kumi: Kept (kg1)"}),
    )
    .unwrap();
    let dropped = rig.ask("scratch", json!({"action": "drop", "scratchId": "sg"})).unwrap();
    assert_eq!((dropped["dropped"].as_u64(), dropped["left"].as_u64()), (Some(2), Some(0)), "{dropped}");
    assert_eq!(
        fake::with(|s| s.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
        ["Drums", "Kick", "Snare", "Reverb", "Kumi · Goal best"]
    );
}

#[test]
fn scratch_keep_refuses_a_copy_changed_since_its_preview_and_writes_nothing() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sb", &kick, 1);
    let copy = made["copies"][0]["ref"].as_str().unwrap().to_string();
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    // Between the preview and the apply an FX of the copy's was turned off (the fence's signature).
    fake::with(|s| s.tracks[4].fx[0].enabled = false);
    let refused = change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k3)"}),
    )
    .unwrap_err();
    assert_eq!(refused.0, "changed");
    fake::with(|s| assert_eq!((s.tracks[4].name.as_str(), s.tracks[4].main_send), ("Kumi · try sb 1", false)));
    // And renamed between them: refused too.
    fake::with(|s| s.tracks[4].fx[0].enabled = true);
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    fake::with(|s| s.tracks[4].name = "Hm".into());
    assert_eq!(
        change(
            &rig,
            "change_apply",
            json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k4)"})
        )
        .unwrap_err()
        .0,
        "changed"
    );
    // Only a scratch copy can be kept.
    assert_eq!(change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&kick)})).unwrap_err().0, "bad-params");
    assert_eq!(change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": {"ref": copy}})).unwrap_err().0, "bad-params");
}

#[test]
fn kumis_undo_of_a_kept_copy_refuses_when_the_producer_changed_it_unless_told_to_discard() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sc", &kick, 1);
    let copy = made["copies"][0]["ref"].as_str().unwrap().to_string();
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    let applied = change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k5)"}),
    )
    .unwrap();
    // The producer moves its fader.
    fake::with(|s| s.tracks[4].volume = 0.5);
    let restore = json!({"kind": "scratch-keep", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo Kept (k6)"});
    let refused = change(&rig, "change_restore", restore.clone()).unwrap_err();
    assert_eq!(refused.0, "changed");
    assert!(refused.1.contains("changed in REAPER since Kumi kept it"));
    assert_eq!(fake::with(|s| s.tracks.len()), 5, "nothing was deleted");
    let mut discard = restore;
    discard["discard"] = true.into();
    change(&rig, "change_restore", discard).unwrap();
    assert_eq!(fake::with(|s| s.tracks.len()), 4);
}

#[test]
fn kumis_undo_of_a_kept_copy_leaves_it_once_the_producer_faded_muted_or_put_a_take_fx_on_its_items() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sf", &kick, 1);
    let copy = made["copies"][0]["ref"].as_str().unwrap().to_string();
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    let applied = change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k9)"}),
    )
    .unwrap();
    let restore = json!({"kind": "scratch-keep", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo Kept (k10)"});
    // What the producer does to the kept track's items, none of it its FX, fader or envelopes.
    type Edit = fn(&mut fake::FakeTrack);
    let edits: [(&str, Edit); 4] = [
        ("a fade", |t| {
            t.items[0].extra.insert("D_FADEINLEN".into(), 0.25);
        }),
        ("a take FX", |t| t.items[0].takes[0].fx.push(FakeFx::new("{FX-TAKE}", "VST: ReaEQ (Cockos)"))),
        ("an item muted", |t| t.items[0].mute = true),
        ("an item's volume", |t| {
            t.items[0].extra.insert("D_VOL".into(), 0.5);
        }),
    ];
    for (what, edit) in edits {
        let was = fake::with(|s| s.tracks[4].clone());
        fake::with(|s| edit(&mut s.tracks[4]));
        let refused = change(&rig, "change_restore", restore.clone()).unwrap_err();
        assert_eq!(refused.0, "changed", "{what}");
        assert_eq!(fake::with(|s| s.tracks.len()), 5, "{what}: nothing was deleted");
        fake::with(|s| s.tracks[4] = was);
    }
    // As Kumi left it, Kumi's undo takes it away.
    change(&rig, "change_restore", restore).unwrap();
    assert_eq!(fake::with(|s| s.tracks.len()), 4);
}

#[test]
fn kumis_undo_of_a_kept_copy_takes_it_away_after_the_project_played_its_automation() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sp", &kick, 1);
    let copy = made["copies"][0]["ref"].as_str().unwrap().to_string();
    // The copy's ReaEQ gain is automated: an envelope on it.
    fake::with(|s| {
        let guid = s.tracks[4].fx[0].guid.clone();
        s.tracks[4].envelopes.push(fake::FakeEnvelope::for_param("Gain / ReaEQ", &guid, 0, 2));
    });
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    let applied = change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k11)"}),
    )
    .unwrap();
    // The project plays: the envelope moves the gain, and with it the plug-in's saved state.
    fake::with(|s| s.tracks[4].fx[0].params[0].value = 0.9);
    let restore = json!({"kind": "scratch-keep", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo Kept (k12)"});
    change(&rig, "change_restore", restore).unwrap();
    assert_eq!(fake::with(|s| s.tracks.len()), 4, "taken away");
}

#[test]
fn kumis_undo_of_a_kept_copy_leaves_it_once_the_producer_turned_a_knob_on_it() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "se", &kick, 1);
    let copy = made["copies"][0]["ref"].as_str().unwrap().to_string();
    let preview = change(&rig, "change_preview", json!({"kind": "scratch-keep", "input": keep_input(&copy)})).unwrap();
    let applied = change(
        &rig,
        "change_apply",
        json!({"kind": "scratch-keep", "input": keep_input(&copy), "fence": preview["fence"], "desc": "Kumi: Kept (k7)"}),
    )
    .unwrap();
    let restore = json!({"kind": "scratch-keep", "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "created": applied["created"], "desc": "Kumi: undo Kept (k8)"});
    // The producer turns the kept copy's ReaEQ: the same FX, items and fader, only what it holds.
    let was = fake::with(|s| std::mem::replace(&mut s.tracks[4].fx[0].params[0].value, 0.8));
    let refused = change(&rig, "change_restore", restore.clone()).unwrap_err();
    assert_eq!(refused.0, "changed", "{refused:?}");
    assert_eq!(fake::with(|s| s.tracks.len()), 5, "nothing was deleted");
    // Turned back, it is as Kumi left it, and Kumi's undo takes it away.
    fake::with(|s| s.tracks[4].fx[0].params[0].value = was);
    change(&rig, "change_restore", restore).unwrap();
    assert_eq!(fake::with(|s| s.tracks.len()), 4);
}

#[test]
fn listening_answers_and_rows_name_refs_only_where_refs_go() {
    let (rig, kick) = Rig::copyable();
    let made = make(&rig, "sd", &kick, 1);
    let spans = rig.ask("read", json!({"kind": "spans"})).unwrap();
    let fx = fake::with(|s| s.tracks[1].fx[0].guid.clone());
    let grid = rig.ask("read", json!({"kind": "param", "parent": format!("r:fx:{fx}"), "grid": 4})).unwrap();
    for answer in [made, spans, grid] {
        assert_eq!(daw_bridge::contract::misplaced_refs(&answer), Vec::<String>::new(), "{answer}");
    }
    let _ = FakeTrack::new("{X}", "x");
}

// ----- deliveries (#401, the render pipeline) -----

#[test]
fn a_delivery_dry_run_lists_reapers_files_and_puts_every_setting_back() {
    let rig = Rig::new();
    producer_settings();
    let before = render_settings();
    let folder = rig.dir.path().join("game");
    let answer = rig
        .ask(
            "deliver_targets",
            json!({"bounds": "regions", "pattern": "SFX_$region", "folder": folder, "format": "wav", "bits": 16, "sampleRate": 48000}),
        )
        .unwrap();
    assert_eq!(answer["targets"], json!([format!("{}/SFX_Verse.wav", folder.display())]), "{answer}");
    assert_eq!(answer["count"], 1);
    assert!(answer.get("there").is_none());
    assert_eq!(render_settings(), before, "every render setting put back before the answer");
}

fn file_names(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|t| Path::new(t.as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned()).collect()
}

#[test]
fn a_delivery_renders_a_file_per_region_through_the_master_and_never_writes_over_a_file_unless_asked() {
    let rig = Rig::new();
    fake::with(|s| {
        s.render_matrix.insert(1, vec!["{BBBB}".into(), "{CCCC}".into()]);
    });
    let before = render_settings();
    let folder = rig.dir.path().join("game");
    let spec = json!({"bounds": "regions", "source": "matrix", "pattern": "$region", "folder": folder});
    let dry = rig.ask("deliver_targets", spec.clone()).unwrap();
    // The region's two tracks summed: one file, not one a track.
    assert_eq!(file_names(&dry["targets"]), ["Verse.wav"]);
    let mut asked = spec.clone();
    asked["jobId"] = "d1".into();
    asked["expect"] = dry["targets"].clone();
    let started = rig.ask("deliver_start", asked).unwrap();
    assert_eq!(started["state"], "queued");
    let ended = rig.until_ended("d1");
    assert_eq!(ended["state"], "done", "{ended}");
    assert_eq!(ended["files"].as_array().unwrap().len(), 1);
    assert_eq!(fake::with(|s| s.listening.renders.last().unwrap().settings), 8 | 128, "the region render matrix via the master");
    assert!(folder.join("Verse.wav").exists());
    assert_eq!(render_settings(), before, "the producer's settings are back after the render");
    // The files are there now: refused, and the file stays as it was.
    let kept = std::fs::metadata(folder.join("Verse.wav")).unwrap().len();
    let mut again = spec.clone();
    again["jobId"] = "d2".into();
    rig.ask("deliver_start", again).unwrap();
    let ended = rig.until_ended("d2");
    assert_eq!(ended["state"], "failed");
    assert!(ended["error"].as_str().unwrap().contains("there already"), "{ended}");
    assert_eq!(std::fs::metadata(folder.join("Verse.wav")).unwrap().len(), kept);
    let mut over = spec.clone();
    over["jobId"] = "d3".into();
    over["overwrite"] = true.into();
    rig.ask("deliver_start", over).unwrap();
    assert_eq!(rig.until_ended("d3")["state"], "done");
}

#[test]
fn a_delivery_of_some_regions_renders_only_theirs_and_puts_every_row_back() {
    let rig = Rig::new();
    fake::with(|s| {
        for (guid, name, number, start) in [("{REG-2}", "Chorus", 2, 20.0), ("{REG-3}", "Outro", 3, 30.0)] {
            s.markers.push(FakeMarker { guid: guid.into(), is_region: true, start, end: start + 4.0, name: name.into(), number, color: 0 });
        }
        s.render_matrix.insert(1, vec!["{BBBB}".into()]);
        s.render_matrix.insert(2, vec!["{CCCC}".into(), "{BBBB}".into()]);
    });
    let rows = || {
        fake::with(|s| {
            let mut rows: Vec<(i32, Vec<String>)> = s
                .render_matrix
                .iter()
                .map(|(number, row)| (*number, row.iter().cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect()))
                .filter(|(_, row): &(i32, Vec<String>)| !row.is_empty())
                .collect();
            rows.sort();
            rows
        })
    };
    let before = rows();
    let folder = rig.dir.path().join("game");
    let ask = |more: Value| {
        let mut spec = json!({"source": "matrix", "pattern": "$region", "folder": folder});
        spec.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        rig.ask("deliver_targets", spec).unwrap()
    };
    // Every region with a row renders, a file each; the Outro's row is empty.
    assert_eq!(file_names(&ask(json!({}))["targets"]), ["Verse.wav", "Chorus.wav"]);
    // Picked, only the Chorus; every row back after.
    let picked = ask(json!({"regionRefs": ["r:region:{REG-2}"]}));
    assert_eq!(file_names(&picked["targets"]), ["Chorus.wav"]);
    assert_eq!(rows(), before, "every row put back");
    // A region picked with an empty row renders nothing, and is said.
    let empty = ask(json!({"regionRefs": ["r:region:{REG-3}"]}));
    assert_eq!(empty["count"], 0);
    assert!(empty["notes"].to_string().contains("render nothing") && empty["notes"].to_string().contains("Outro"), "{empty}");
    // The mix of a region picked: through the master's row for the render, the row empty again after.
    let mix = ask(json!({"source": "master", "regionRefs": ["r:region:{REG-3}"]}));
    assert_eq!(file_names(&mix["targets"]), ["Outro.wav"]);
    assert_eq!(rows(), before);
    // A soloed track decides every file, and a muted track a rendering row has is in none: both said.
    fake::with(|s| {
        s.tracks[3].solo = 1;
        s.tracks[1].mute = true;
    });
    let notes = ask(json!({"regionRefs": ["r:region:{REG-2}"]}))["notes"].to_string();
    assert!(notes.contains("\u{201c}Reverb\u{201d} is soloed: every file holds only what solo lets through"), "{notes}");
    assert!(notes.contains("\u{201c}Kick\u{201d} is muted"), "{notes}");
    // The render says them too (a render called without a check).
    rig.ask(
        "deliver_start",
        json!({"jobId": "d0", "source": "matrix", "pattern": "$region", "folder": folder, "regionRefs": ["r:region:{REG-2}"]}),
    )
    .unwrap();
    let ended = rig.until_ended("d0");
    assert!(ended["notes"].to_string().contains("is soloed") && ended["notes"].to_string().contains("is muted"), "{ended}");
    std::fs::remove_file(folder.join("Chorus.wav")).unwrap();
    fake::with(|s| {
        s.tracks[3].solo = 0;
        s.tracks[1].mute = false;
    });
    // Rendered: only the Chorus, every row back.
    rig.ask(
        "deliver_start",
        json!({"jobId": "d1", "source": "matrix", "pattern": "$region", "folder": folder, "regionRefs": ["r:region:{REG-2}"], "expect": picked["targets"]}),
    )
    .unwrap();
    assert_eq!(rig.until_ended("d1")["state"], "done");
    assert!(folder.join("Chorus.wav").exists() && !folder.join("Verse.wav").exists());
    assert_eq!(rows(), before);
}

#[test]
fn a_delivery_turns_the_render_dialogs_extras_off_unless_asked_and_says_which() {
    let rig = Rig::new();
    producer_settings();
    // (and the producer embeds metadata in their files)
    fake::with(|s| *s.listening.numbers.get_mut("RENDER_SETTINGS").unwrap() += 512.0);
    let before = render_settings();
    let folder = rig.dir.path().join("game");
    let dry = rig.ask("deliver_targets", json!({"pattern": "$region", "folder": folder})).unwrap();
    assert_eq!(dry["notes"], json!(["the render dialog's normalizing, a second format off for this delivery (dialog: true keeps them)"]));
    rig.ask("deliver_start", json!({"jobId": "d1", "pattern": "$region", "folder": folder, "expect": dry["targets"]})).unwrap();
    let ended = rig.until_ended("d1");
    assert_eq!(ended["state"], "done", "{ended}");
    assert!(ended["notes"].to_string().contains("off for this delivery"), "{ended}");
    let seen = fake::with(|s| s.listening.renders.last().unwrap().clone());
    assert_eq!(seen.normalize, render::NO_POSTPROCESSING, "rendered without them");
    assert_ne!(seen.settings & render::DELAY_START, 0, "the producer's start delay stays");
    assert_ne!(seen.settings & 512, 0, "and their metadata");
    assert_eq!(render_settings(), before, "and they're back after");
    // Kept, when asked.
    let kept = rig.ask("deliver_targets", json!({"pattern": "$region", "folder": folder, "dialog": true})).unwrap();
    assert!(kept["notes"][0].as_str().unwrap().contains("kept, as asked"), "{kept}");
    rig.ask("deliver_start", json!({"jobId": "d2", "pattern": "$region", "folder": folder, "dialog": true, "overwrite": true})).unwrap();
    assert_eq!(rig.until_ended("d2")["state"], "done");
    assert_eq!(fake::with(|s| s.listening.renders.last().unwrap().normalize), 1.0);
}

#[test]
fn a_delivery_whose_files_arent_the_dry_runs_renders_nothing() {
    let rig = Rig::new();
    let folder = rig.dir.path().join("game");
    let renders = || fake::with(|s| s.listening.renders.len());
    let before = renders();
    rig.ask("deliver_start", json!({"jobId": "d1", "pattern": "$region", "folder": folder, "expect": ["/elsewhere/Verse.wav"]})).unwrap();
    let ended = rig.until_ended("d1");
    assert_eq!((ended["state"].as_str(), ended["code"].as_str()), (Some("failed"), Some("changed")), "{ended}");
    assert_eq!(renders(), before, "REAPER never rendered");
    assert!(!folder.join("Verse.wav").exists());
}

#[test]
fn a_deliverys_settings_are_checked_before_anything_is_set() {
    let rig = Rig::new();
    let folder = rig.dir.path().join("game");
    for wrong in [
        json!({"source": "matrix", "bounds": "project", "pattern": "$region", "folder": folder}),
        json!({"source": "stems", "pattern": "$track", "folder": folder}),
        json!({"pattern": "../$region", "folder": folder}),
        json!({"pattern": "", "folder": folder}),
        json!({"pattern": "$region", "folder": "relative/folder"}),
        json!({"pattern": "$region", "folder": folder, "format": "wav", "bits": 20}),
        json!({"pattern": "$region", "folder": folder, "sampleRate": 12345}),
        json!({"pattern": "$region", "folder": folder, "bounds": {"start": 4.0, "end": 2.0}}),
        // (a render's tail over regions runs into the next region's sound)
        json!({"pattern": "$region", "folder": folder, "tailMs": 500}),
        json!({"pattern": "$region", "folder": folder, "bounds": "selected-regions", "tailMs": 500}),
        json!({"pattern": "$region", "folder": folder, "bounds": "project", "regionRefs": ["r:region:{REG-1}"]}),
        json!({"source": "stems", "trackRefs": ["r:track:{BBBB}"], "pattern": "$track", "folder": folder, "regionRefs": ["r:region:{REG-1}"]}),
        json!({"pattern": "$region", "folder": folder, "regionRefs": ["r:track:{BBBB}"]}),
        json!({"pattern": "$region", "folder": folder, "regionRefs": []}),
    ] {
        assert_eq!(rig.ask("deliver_targets", wrong.clone()).unwrap_err().0, "bad-params", "{wrong}");
    }
    let gone = rig.ask("deliver_targets", json!({"pattern": "$region", "folder": folder, "regionRefs": ["r:region:{NOPE}"]}));
    assert_eq!(gone.unwrap_err().0, "gone");
    // A tail past a stretch's end is fine.
    rig.ask("deliver_targets", json!({"pattern": "$project", "folder": folder, "bounds": "project", "tailMs": 500})).unwrap();
    // With no folder given or set, the project's own folder; a render folder set relative to it, under it.
    fake::with(|s| s.listening.texts.remove("RENDER_FILE"));
    let answer = rig.ask("deliver_targets", json!({"pattern": "$region"})).unwrap();
    assert_eq!(answer["targets"], json!(["/lab/Projects/Verse.wav"]));
    fake::with(|s| s.listening.texts.insert("RENDER_FILE".into(), "Renders".into()));
    let answer = rig.ask("deliver_targets", json!({"pattern": "$region"})).unwrap();
    assert_eq!(answer["folder"], "/lab/Projects/Renders");
}
