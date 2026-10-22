//! The loaded extension on REAPER's main thread: each tick (REAPER's timer and the control surface's Run()) runs
//! queued requests within a budget, then turns what the control surface reported into events.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::api::{self, Api};
use crate::csurf::Call;
use crate::focus::Focus;
use crate::queue::{self, Answer, Budget, Job};
use crate::read::Reads;
use crate::reaper::{Busy, PlayState, ProjectInfo, Reaper, ACTION_QUIT};

pub const BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const HEARTBEAT: Duration = Duration::from_millis(daw_bridge::wire::HEARTBEAT_MS);
/// How long the main window has to stay disabled before REAPER counts as in a dialog: a disable that comes and goes
/// within an ordinary action is not one.
const MODAL_AFTER: Duration = Duration::from_millis(300);
/// Set to 0 to stop asking whether a dialog has the main window (for a REAPER where the question gets the wrong answer).
const MODAL_CHECK_ENV: &str = "KUMI_REAPER_MODAL_CHECK";
/// How often `focus` is polled when nothing has said it changed (Kumi asks about every 150 ms).
const FOCUS_EVERY: Duration = Duration::from_millis(50);

// Actions that close or replace the project, and so ask "Save changes?" in a dialog inside the request when it has any.
const CLOSES_CURRENT_PROJECT: [i32; 2] = [40860, 40023];
const CLOSES_ALL_PROJECTS: [i32; 1] = [40886];
// Actions that render with a progress window of their own, which holds the main thread for the whole render.
const RENDERS: [i32; 2] = [41824, 42230];
const SAVE_PROJECT: i32 = 40026;

/// Where the main thread's events and heartbeats go: the socket server, or a test's list.
pub trait Outbox {
    fn event(&self, name: &str, data: Value);
    fn heartbeat(&self);
}

/// Clears the flag when dropped.
struct ClearOnDrop<'a>(&'a Cell<bool>);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// REAPER's UI refresh held (PreventUIRefresh +1) from the first `hold`, and let go when dropped.
struct RefreshHold<'a> {
    reaper: &'a Reaper<'a>,
    held: bool,
}

impl RefreshHold<'_> {
    fn hold(&mut self) {
        if !self.held {
            self.reaper.prevent_ui_refresh(1);
            self.held = true;
        }
    }
}

impl Drop for RefreshHold<'_> {
    fn drop(&mut self) {
        if self.held {
            self.reaper.prevent_ui_refresh(-1);
        }
    }
}

/// What the socket threads may read without the main thread: who this bridge is, and when it last ticked.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub daw_version: String,
    pub resource: String,
    pub project: Option<ProjectSummary>,
    pub last_tick: Option<Instant>,
    /// Loading, saving or rendering, as of the last tick.
    pub busy: Option<Busy>,
    /// `status`'s fields read on the main thread: transport, track count, tick rates.
    pub status: Value,
    /// What `focus` answers, polled each tick (Null until the first poll finds a project).
    pub focus: Value,
    /// When `focus` was polled.
    pub focus_at: Option<Instant>,
    /// The main thread is running a request it claimed, since then: it isn't ticking, and it isn't stuck (a plug-in
    /// loading, a big take). That request's socket waits for its answer as long as the request allows; others are told
    /// REAPER is busy once it has run longer than a stall.
    pub working: Option<Instant>,
    /// M5 listening (m5-reaper): the jobs and the preview, which the socket threads answer for themselves.
    pub listening: Arc<crate::listening::Board>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectSummary {
    pub name: String,
    pub path: String,
}

impl ProjectSummary {
    pub fn to_json(&self) -> Value {
        let mut project = serde_json::Map::new();
        // As long as the discovery file and the welcome keep them.
        if !self.name.is_empty() {
            project.insert("name".into(), crate::server::cut(&self.name, crate::server::MAX_NAME).into());
        }
        if !self.path.is_empty() {
            project.insert("path".into(), crate::server::cut(&self.path, crate::server::MAX_PATH).into());
        }
        Value::Object(project)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Timer,
    Surface,
}

// What the control surface reported since the last tick.
#[derive(Default)]
struct Pending {
    tracks: bool,
    selection: bool,
    play: Option<PlayState>,
    tempo: bool,
    // Something `focus` shows moved (selection, transport, FX focus): poll it this tick.
    focus: bool,
}

/// `{playing, paused, recording, bpm}`: the keys the Renoise bridge shares (`playing`, `bpm`) plus REAPER's own.
fn transport_json(play: PlayState, bpm: Option<f64>) -> Value {
    let mut transport = play.to_json();
    if let Some(bpm) = bpm {
        transport["bpm"] = bpm.into();
    }
    transport
}

fn bad_params(message: &str) -> (String, String) {
    ("bad-params".into(), message.into())
}

// What was last sent, so a tick sends only changes.
#[derive(Default)]
struct Sent {
    project: Option<(usize, ProjectSummary)>,
    transport: Option<Value>,
}

/// How long the set of track GUIDs a project had may be before it is read again when the project changes.
pub(crate) const WATCH_REFRESH: Duration = Duration::from_millis(250);
/// The most track GUIDs kept to tell one project from another.
const WATCH_TRACKS: i32 = 2000;

/// What the bridge remembers of the project in the active tab, to tell a new project from the same one changed. A
/// project opened in the tab where another was (File > Open, New project) keeps the tab's pointer, so the pointer alone
/// doesn't say; the file, the state change count and the tracks' GUIDs do.
#[derive(Default)]
struct Watch {
    project: Option<usize>,
    path: String,
    name: String,
    count: Option<i32>,
    /// The undo history's entries: one right after a project is loaded.
    undo: Option<i32>,
    tracks: HashSet<String>,
    tracks_at: Option<Instant>,
    /// REAPER's count of loads into this project (`projectconfig`), as of the last look.
    loads: u64,
}

fn track_guids(reaper: &Reaper, project: crate::reaper::Project) -> HashSet<String> {
    (0..reaper.track_count(project).min(WATCH_TRACKS)).filter_map(|i| reaper.track_at(project, i)).map(|t| reaper.track_guid(t)).collect()
}

/// What the bridge saw change in the active tab's project since it last looked.
#[derive(Debug, Clone, Copy, Default)]
struct Moves {
    file_changed: bool,
    counter_went_back: bool,
    /// The undo history fell back to a fresh project's: a project was loaded, whatever file.
    history_reset: bool,
    /// A saved project became unsaved: a project made from a template, never a Save As.
    went_unsaved: bool,
}

/// Whether the project now is another project than the one before. A load (the undo history gone), or a saved project
/// that became unsaved, is one whatever the tracks (a template carries the GUIDs of the project it was made from);
/// otherwise the tracks decide: they share none; or one side has none and the file changed or the counter went back;
/// or neither has any and the file changed.
fn is_another_project(before: &HashSet<String>, now: &HashSet<String>, moves: Moves) -> bool {
    let Moves { file_changed, counter_went_back, history_reset, went_unsaved } = moves;
    if history_reset || went_unsaved {
        return true;
    }
    match (before.is_empty(), now.is_empty()) {
        (false, false) => before.is_disjoint(now),
        (true, true) => file_changed,
        _ => file_changed || counter_went_back,
    }
}

// Ticks per second for each driver, measured over the last second.
#[derive(Default)]
struct Rates {
    window_start: Option<Instant>,
    timer: u32,
    surface: u32,
    timer_hz: u32,
    surface_hz: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quit {
    No,
    // Answered this tick; REAPER quits next tick, after the answer is on its way.
    Answered,
}

pub struct Extension {
    api: Api,
    reads: Reads,
    changes: crate::changes::Changes,
    focus: Focus,
    jobs: Receiver<Job>,
    out: Box<dyn Outbox>,
    snapshot: Arc<Mutex<Snapshot>>,
    budget: Budget,
    pending: RefCell<Pending>,
    sent: RefCell<Sent>,
    watch: RefCell<Watch>,
    /// Each tab's load count as of when it was last the active one (a project loaded at its pointer since is another).
    seen_loads: RefCell<HashMap<usize, u64>>,
    modal_since: Cell<Option<Instant>>,
    modal_check: bool,
    rates: RefCell<Rates>,
    in_tick: Cell<bool>,
    quit: Cell<Quit>,
    last_heartbeat: Cell<Option<Instant>>,
    focus_value: RefCell<Value>,
    focus_at: Cell<Option<Instant>>,
    /// M5 listening (m5-reaper): renders, captures, previews, scratch copies.
    listening: crate::listening::Listening,
}

impl Extension {
    pub fn new(api: Api, jobs: Receiver<Job>, out: Box<dyn Outbox>, snapshot: Arc<Mutex<Snapshot>>) -> Extension {
        let board = snapshot.lock().unwrap_or_else(|e| e.into_inner()).listening.clone();
        let extension = Extension {
            api,
            reads: Reads::default(),
            changes: Default::default(),
            focus: Focus::default(),
            jobs,
            out,
            snapshot,
            budget: Budget::default(),
            pending: RefCell::default(),
            sent: RefCell::default(),
            watch: RefCell::default(),
            seen_loads: RefCell::default(),
            modal_since: Cell::new(None),
            modal_check: std::env::var(MODAL_CHECK_ENV).map_or(true, |value| value != "0"),
            rates: RefCell::default(),
            in_tick: Cell::new(false),
            quit: Cell::new(Quit::No),
            last_heartbeat: Cell::new(None),
            focus_value: RefCell::new(Value::Null),
            focus_at: Cell::new(None),
            listening: crate::listening::Listening::new(board),
        };
        extension.refresh_snapshot(None);
        extension
    }

    pub fn reaper(&self) -> Reaper<'_> {
        Reaper::new(&self.api)
    }

    /// A control surface callback. Returns what `Extended` answers.
    pub fn on_surface(&self, call: Call) -> i32 {
        let mut pending = self.pending.borrow_mut();
        match call {
            Call::Run => {
                drop(pending);
                self.tick(Driver::Surface);
            }
            Call::TrackListChange => {
                pending.tracks = true;
                pending.focus = true;
            }
            Call::Selected(..) | Call::TrackSelection(_) => {
                pending.selection = true;
                pending.focus = true;
            }
            Call::PlayState { play, pause, rec } => {
                pending.play = Some(PlayState { playing: play, paused: pause, recording: rec });
                pending.focus = true;
            }
            Call::Extended { call: api::CSURF_EXT_SETBPMANDPLAYRATE, .. } => {
                pending.tempo = true;
                pending.focus = true;
            }
            Call::Extended {
                call:
                    api::CSURF_EXT_SETFOCUSEDFX
                    | api::CSURF_EXT_SETLASTTOUCHEDFX
                    | api::CSURF_EXT_SETFXOPEN
                    | api::CSURF_EXT_SETFXCHANGE
                    | api::CSURF_EXT_SETLASTTOUCHEDTRACK,
                ..
            } => pending.focus = true,
            _ => {}
        }
        0
    }

    /// REAPER ran an action (the post-command hooks): the undo history is looked at on the next tick.
    pub fn nudge(&self) {
        self.changes.nudge();
    }

    /// The extension is going: a plan's open undo block is closed.
    pub fn stop(&self) {
        self.changes.stop(&self.reaper(), &self.reads);
        // M5 listening (m5-reaper): the preview stops, the hook comes out, what runs ends.
        self.listening.stop(&self.reaper());
    }

    /// M5 listening (m5-reaper): where renders and captures go, once the reference names Kumi's folder.
    pub fn set_listening_folder(&self, folder: std::path::PathBuf) {
        self.listening.set_folder(folder);
    }

    /// Kumi's own folder: a file in it is copied into the project's media folder before a change makes REAPER play it.
    pub fn set_kumi_folder(&self, folder: std::path::PathBuf) {
        self.reads.set_kumi_folder(folder);
    }

    pub fn tick(&self, driver: Driver) {
        // A request's REAPER call can call back into the surface (Run included): that tick is skipped.
        if self.in_tick.replace(true) {
            // M5 listening (m5-reaper): REAPER's render loop ticks while Kumi's render holds the main thread: the socket
            // threads are told it renders, and a cancel presses the render window's Cancel; nothing else runs.
            self.listening.nested(&self.snapshot, &*self.out);
            return;
        }
        // Cleared on the way out whatever happens, so one panic caught above doesn't skip every later tick.
        let _in_tick = ClearOnDrop(&self.in_tick);
        self.count_tick(driver);
        // Only the timer quits: REAPER may unload the extension inside the action, and the control surface's Run()
        // would then be running inside the surface being deleted.
        if self.quit.get() == Quit::Answered && driver == Driver::Timer {
            // REAPER may unload the extension inside this call; nothing more this tick.
            self.quit.set(Quit::No);
            self.reaper().run_action(ACTION_QUIT);
            return;
        }
        // Nothing runs while a dialog has the main window, even in the moments before it counts as one.
        let blocked = self.window_blocked();
        let busy = self.reaper().busy().or_else(|| self.in_dialog(blocked).then_some(Busy::Modal));
        // The project first: a request waiting at a tab switch or a load is answered about the project REAPER has now,
        // under the epoch that project has, and a cursor from the one before is refused.
        self.poll_project(busy.is_none());
        if busy.is_none() && !blocked {
            self.run_jobs();
        }
        // M5 listening (m5-reaper): a preview's end and a capture's drain each tick; a track's items, a planned render (held
        // inside REAPER's render until it ends) and the first scratch sweep only while REAPER is free.
        self.listening.tick(&self.reaper(), &self.reads, busy.is_none() && !blocked, &self.snapshot, &*self.out);
        if busy.is_none() {
            self.poll_focus();
            // The producer's undo of Kumi's changes; a plan left open. An undo or redo may bring back scratch copies Kumi
            // dropped (they were bare edits inside the producer's points): they're swept again.
            if self.changes.tick(&self.reaper(), &self.reads, &*self.out) {
                self.listening.undo_moved(&self.reaper(), &*self.out);
            }
        }
        self.flush_events();
        self.refresh_snapshot(busy);
        let now = Instant::now();
        if self.last_heartbeat.get().is_none_or(|last| now.duration_since(last) >= HEARTBEAT) {
            self.last_heartbeat.set(Some(now));
            self.out.heartbeat();
        }
    }

    // Whether the main window is disabled right now, as a dialog does.
    fn window_blocked(&self) -> bool {
        self.modal_check && self.reaper().main_window_enabled() == Some(false)
    }

    // Whether a dialog has had REAPER's main window for a while: requests are then answered busy rather than queued behind
    // it. (A window disabled for an instant is not one; but nothing runs while it is, either way.)
    fn in_dialog(&self, blocked: bool) -> bool {
        if !blocked {
            self.modal_since.set(None);
            return false;
        }
        let since = self.modal_since.get().unwrap_or_else(|| {
            let now = Instant::now();
            self.modal_since.set(Some(now));
            now
        });
        since.elapsed() >= MODAL_AFTER
    }

    fn run_jobs(&self) {
        let reaper = self.reaper();
        // Held by the first request, released on the way out even if something unwinds: an unbalanced hold leaves
        // REAPER's arrange view frozen.
        let mut refresh = RefreshHold { reaper: &reaper, held: false };
        let mark = || (reaper.current_project().map(|p| p.ptr.addr()), crate::projectconfig::loads());
        let mut seen = mark();
        queue::drain(&self.jobs, self.budget, |job| {
            refresh.hold();
            // The socket threads are told the main thread is working, not stalled, for as long as this takes; whatever
            // unwinds, they are told it stopped.
            self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).working = Some(Instant::now());
            let _working = StopWorking(&self.snapshot);
            // A job before this one switched tabs or loaded a project (an action, a script): the project is looked at
            // again first, so this one is answered about the project REAPER has now, under its epoch.
            let now = mark();
            if now != seen {
                self.poll_project(true);
                seen = now;
            }
            let answer = crate::guard("request", Err(("internal".into(), "the request failed inside the bridge".into())), || {
                self.handle(&job.method, &job.params)
            });
            job.answer(answer);
        });
    }

    fn handle(&self, method: &str, params: &Value) -> Answer {
        match method {
            "action" => self.action(params),
            "quit" => self.quit(params),
            "read" => self.reads.read(&self.reaper(), params),
            "snapshot" => self.reads.snapshot(&self.reaper()),
            "digest" => self.reads.digest(&self.reaper(), params),
            "focus_body" => self.reads.focus_body(&self.reaper(), params),
            "change_preview" => self.changes.preview(&self.reaper(), &self.reads, params),
            "change_apply" => self.changes.apply(&self.reaper(), &self.reads, params),
            "change_restore" => self.changes.restore(&self.reaper(), &self.reads, params),
            "transport_stop" => self.changes.transport_stop(&self.reaper()),
            "transport_play" => self.changes.transport_play(&self.reaper()),
            "cursor_set" => self.changes.cursor_set(&self.reaper(), &self.reads, params),
            "undo_step_begin" => self.changes.step_begin(&self.reaper(), &self.reads, params),
            "undo_step_end" => self.changes.step_end(&self.reaper(), &self.reads, params),
            // m4-r-fxenv
            "fx_chain_save" => self.changes.fx_chain_save(&self.reaper(), &self.reads, params),
            // (#401) a chore kept as an action: a file and the Action List, not the project
            "script_install" => crate::scripts::install(&self.reaper(), params),
            "script_remove" => crate::scripts::remove(&self.reaper(), params),
            // (r-tracks) Saving: actions, no change.
            "project_save" => self.changes.project_save(&self.reaper(), &self.reads, params),
            "track_template_save" => self.changes.track_template_save(&self.reaper(), &self.reads, params),
            // M5 listening (m5-reaper).
            "render_start" => self.listening.render_start(&self.reaper(), &self.reads, params),
            // #401: the producer's own files, dry run and render.
            "deliver_targets" => self.listening.deliver_targets(&self.reaper(), &self.reads, params),
            "deliver_start" => self.listening.deliver_start(&self.reaper(), &self.reads, params),
            "capture_start" => self.listening.capture_start(&self.reaper(), &self.reads, params),
            "audition" => self.listening.audition(&self.reaper(), params),
            "audio_file" => self.listening.audio_file(&self.reaper(), &self.reads, params),
            "scratch" => self.listening.scratch(&self.reaper(), &self.reads, params),
            other => Err(("unknown-method".into(), format!("the REAPER bridge has no method {other:?}"))),
        }
    }

    // Runs a main-section action by id or command name. Actions that open a dialog (named "…") never run unattended.
    fn action(&self, params: &Value) -> Answer {
        let reaper = self.reaper();
        let command = match &params["command"] {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => return Err(bad_params("command: an action id or a command name")),
        };
        let Some(id) = reaper.command_id(&command) else { return Err(bad_params(&format!("no action {command:?}"))) };
        let Some(name) = reaper.action_name(id) else { return Err(bad_params(&format!("no action {command:?}"))) };
        if name.ends_with("...") || name.ends_with('…') {
            return Err(bad_params(&format!("{name:?} opens a dialog, which Kumi never runs unattended")));
        }
        if id == ACTION_QUIT {
            return Err(bad_params("use quit, which checks for unsaved changes first"));
        }
        if RENDERS.contains(&id) {
            return Err(bad_params(&format!("{name:?} renders with a progress window that holds REAPER's main thread")));
        }
        // These would ask "Save changes?" in a dialog, inside this request, if a project they close has changes.
        let closing = if CLOSES_ALL_PROJECTS.contains(&id) {
            reaper.projects()
        } else if CLOSES_CURRENT_PROJECT.contains(&id) {
            reaper.current_project().into_iter().collect()
        } else {
            vec![]
        };
        let dirty: Vec<&str> = closing.iter().filter(|p| reaper.is_dirty(p.ptr)).map(|p| p.name.as_str()).collect();
        if !dirty.is_empty() {
            let names = dirty.iter().map(|name| if name.is_empty() { "(unsaved project)" } else { name }).collect::<Vec<_>>();
            return Err(("failed".into(), format!("{name:?} would ask about unsaved changes in {}: save them first", names.join(", "))));
        }
        // Saving a project that was never saved opens Save As.
        if id == SAVE_PROJECT && reaper.current_project().is_some_and(|p| p.path.is_empty()) {
            return Err(("failed".into(), "this project was never saved, so Save would open Save As: save it in REAPER first".into()));
        }
        reaper.run_action(id);
        Ok(json!({"command": id, "name": name}))
    }

    // Quits on the next tick. REAPER would ask about unsaved changes in a dialog, so those are saved first when asked
    // (`save: true`), and refused otherwise; a project never saved can't be saved without Save As.
    fn quit(&self, params: &Value) -> Answer {
        let reaper = self.reaper();
        let save = params["save"].as_bool().unwrap_or(false);
        if reaper.projects().iter().any(|p| reaper.play_state(p.ptr).recording) {
            return Err((
                "failed".into(),
                "REAPER is recording: quitting would ask what to do with the recorded files; stop it first".into(),
            ));
        }
        let dirty: Vec<ProjectInfo> = reaper.projects().into_iter().filter(|p| reaper.is_dirty(p.ptr)).collect();
        if dirty.iter().any(|p| p.path.is_empty()) {
            return Err(("failed".into(), "a project that was never saved has changes: save it in REAPER first".into()));
        }
        if !dirty.is_empty() && !save {
            let names: Vec<&str> = dirty.iter().map(|p| p.name.as_str()).collect();
            return Err(("failed".into(), format!("unsaved changes in {}: pass save: true to save them first", names.join(", "))));
        }
        for project in &dirty {
            reaper.save(project.ptr);
        }
        // Saving can fail (a read-only folder, a plug-in refusing) and leave the project changed: REAPER would then
        // ask about it in a dialog when it quits, so look again.
        let still: Vec<&str> = dirty.iter().filter(|p| reaper.is_dirty(p.ptr)).map(|p| p.name.as_str()).collect();
        if !still.is_empty() {
            return Err((
                "failed".into(),
                format!("{} still has unsaved changes after saving: REAPER would ask about them", still.join(", ")),
            ));
        }
        self.quit.set(Quit::Answered);
        Ok(json!({"quitting": true, "saved": dirty.iter().map(|p| p.path.clone()).collect::<Vec<_>>()}))
    }

    // What `status` adds to the bridge's identity; the socket threads serve it from the snapshot.
    fn status(&self, project: Option<&ProjectInfo>) -> Value {
        let reaper = self.reaper();
        let rates = self.rates.borrow();
        let mut status = json!({
            "sdk": api::SDK_VERSION,
            "api": {"resolved": self.api.resolved(), "total": api::FUNCTION_COUNT},
            "ticksPerSecond": {"timer": rates.timer_hz, "surface": rates.surface_hz},
        });
        // M5 listening (m5-reaper): what listening can do now.
        status["listening"] = self.listening.flags(&reaper);
        let missing = reaper.missing_features();
        if !missing.is_empty() {
            status["missing"] =
                missing.iter().map(|(feature, needs)| json!({"feature": feature, "needs": needs})).collect::<Vec<_>>().into();
        }
        if let Some(project) = project {
            status["transport"] = transport_json(reaper.play_state(project.ptr), reaper.tempo());
            status["tracks"] = reaper.track_count(project.ptr).into();
            if let Some(count) = reaper.state_change_count(project.ptr) {
                status["stateChangeCount"] = count.into();
            }
        }
        status
    }

    // A tab switch, a project opened or closed, or saved under another name; and what a project's pointer can't show: a new
    // project opened in the tab where another was. `settled` is false while REAPER loads or renders.
    fn poll_project(&self, settled: bool) {
        let reaper = self.reaper();
        let info = reaper.current_project();
        if settled {
            let open: Vec<usize> = reaper.projects().iter().map(|p| p.ptr.addr()).collect();
            self.reads.keep_tabs(&open);
        }
        let (switched, replaced) = if settled { self.watch_project(&reaper, info.as_ref()) } else { (false, false) };
        let current = info.as_ref().map(|p| (p.ptr.addr(), ProjectSummary { name: p.name.clone(), path: p.path.clone() }));
        let mut sent = self.sent.borrow_mut();
        let changed = sent.project != current;
        let first = sent.project.is_none();
        if changed {
            sent.project = current.clone();
        }
        drop(sent);
        if switched || replaced {
            // Refs name objects of the project in this tab: they mean something else now.
            self.reads.bump_epoch();
            self.reads.forget_touched(&reaper);
            self.focus.reset();
            if let Some(info) = info.as_ref().filter(|_| replaced) {
                self.reads.project_replaced(info.ptr);
            }
            // M5 listening (m5-reaper): scratch copies an earlier session left in the project now in front are swept.
            self.listening.project_changed(&reaper, &*self.out);
        }
        // Noticed on every settled tick, so an id that came with a template is seen while the project is unsaved.
        if let Some(info) = info.as_ref().filter(|_| settled) {
            self.reads.note_project(info.ptr);
            self.reads.project_id(&reaper, info.ptr, info);
            self.reads.keep_project_id(&reaper, info.ptr, info);
        }
        if (changed && !first) || replaced {
            self.out.event("project", current.map(|(_, p)| p.to_json()).unwrap_or(Value::Null));
            let mut pending = self.pending.borrow_mut();
            pending.tracks = true;
            pending.selection = true;
            pending.focus = true;
        }
    }

    // Compares the active tab's project with what it was: (the tab changed, another project took its place in the tab).
    fn watch_project(&self, reaper: &Reaper, info: Option<&ProjectInfo>) -> (bool, bool) {
        let mut watch = self.watch.borrow_mut();
        let Some(info) = info else {
            *watch = Watch::default();
            return (false, false);
        };
        let addr = info.ptr.addr();
        let count = reaper.state_change_count(info.ptr);
        let undo = reaper.undo_entries(info.ptr);
        let fresh = |watch: &mut Watch| {
            *watch = Watch {
                project: Some(addr),
                path: info.path.clone(),
                name: info.name.clone(),
                count,
                undo,
                tracks: track_guids(reaper, info.ptr),
                tracks_at: Some(Instant::now()),
                loads: crate::projectconfig::loads_of(addr),
            };
        };
        match watch.project {
            None => {
                fresh(&mut watch);
                (false, false)
            }
            Some(before) if before != addr => {
                // A tab whose project was loaded since it was last seen (one closed and another opened at its pointer
                // within a tick) holds another project.
                let reloaded = self.seen_loads.borrow().get(&addr).is_some_and(|seen| *seen != crate::projectconfig::loads_of(addr));
                self.seen_loads.borrow_mut().insert(before, watch.loads);
                fresh(&mut watch);
                (true, reloaded)
            }
            Some(_) => {
                // REAPER loaded a project or made one since the last look, and the tab is the same: what it holds is
                // another project, whatever its file, its tracks or its counter say.
                let loaded = crate::projectconfig::loads_of(addr) != watch.loads;
                let file_changed = watch.path != info.path || watch.name != info.name;
                let counter_went_back = matches!((watch.count, count), (Some(before), Some(now)) if now < before);
                let history_reset = matches!((watch.undo, undo), (Some(before), Some(now)) if before >= 2 && now <= 1);
                let went_unsaved = !watch.path.is_empty() && info.path.is_empty();
                let due = count != watch.count && watch.tracks_at.is_none_or(|at| at.elapsed() >= WATCH_REFRESH);
                let mut replaced = false;
                let look = loaded || file_changed || counter_went_back || history_reset || due;
                if look {
                    let now = track_guids(reaper, info.ptr);
                    let moves = Moves { file_changed, counter_went_back, history_reset: history_reset || loaded, went_unsaved };
                    replaced = is_another_project(&watch.tracks, &now, moves);
                    watch.tracks = now;
                    watch.tracks_at = Some(Instant::now());
                }
                watch.loads = crate::projectconfig::loads_of(addr);
                watch.path = info.path.clone();
                watch.name = info.name.clone();
                // A change the throttle held back is still a change when the window is over: the counter stays what it was.
                if look || count == watch.count {
                    watch.count = count;
                    watch.undo = undo;
                }
                (false, replaced)
            }
        }
    }

    // `focus`, polled every FOCUS_EVERY, or at once when the surface said something it shows moved. The selection part
    // of it is read now when REAPER said the selection moved, and by its own throttle otherwise.
    fn poll_focus(&self) {
        let (urgent, selection) = {
            let pending = self.pending.borrow();
            (pending.focus, pending.selection)
        };
        let due = urgent || self.focus_at.get().is_none_or(|at| at.elapsed() >= FOCUS_EVERY);
        if !due {
            return;
        }
        let reaper = self.reaper();
        let value = match reaper.current_project() {
            Some(project) => self.focus.poll(&self.reads, &reaper, project.ptr, selection),
            None => Value::Null,
        };
        *self.focus_value.borrow_mut() = value;
        self.focus_at.set(Some(Instant::now()));
        self.pending.borrow_mut().focus = false;
    }

    fn flush_events(&self) {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        if !pending.tracks && !pending.selection && pending.play.is_none() && !pending.tempo {
            return;
        }
        let reaper = self.reaper();
        let Some(project) = reaper.current_project() else { return };
        if pending.play.is_some() || pending.tempo {
            let play = pending.play.unwrap_or_else(|| reaper.play_state(project.ptr));
            let transport = transport_json(play, reaper.tempo());
            let mut sent = self.sent.borrow_mut();
            if sent.transport.as_ref() != Some(&transport) {
                sent.transport = Some(transport.clone());
                drop(sent);
                self.out.event("transport", transport);
            }
        }
        if pending.tracks {
            self.out.event("tracks", json!({"count": reaper.track_count(project.ptr)}));
        }
        if pending.selection {
            let tracks: Vec<Value> = reaper.selected_tracks(project.ptr).into_iter().map(|t| reaper.track_json(t)).collect();
            self.out.event("selection", json!({"tracks": tracks}));
        }
    }

    fn refresh_snapshot(&self, busy: Option<Busy>) {
        let reaper = self.reaper();
        let project = reaper.current_project();
        let status = self.status(project.as_ref());
        let mut snapshot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        if snapshot.daw_version.is_empty() {
            snapshot.daw_version = reaper.app_version();
            snapshot.resource = reaper.resource_path();
        }
        snapshot.project = project.map(|p| ProjectSummary { name: p.name, path: p.path });
        snapshot.busy = busy;
        snapshot.status = status;
        snapshot.focus = self.focus_value.borrow().clone();
        snapshot.focus_at = self.focus_at.get();
        snapshot.last_tick = Some(Instant::now());
    }

    /// Answers every queued job "closing" without running it: the extension is unloading.
    pub fn cancel_jobs(&self) {
        while let Ok(job) = self.jobs.try_recv() {
            if job.claim() {
                job.answer(Err(("failed".into(), "REAPER is closing; nothing changed".into())));
            }
        }
    }

    /// A copy of what the socket threads see.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn count_tick(&self, driver: Driver) {
        let mut rates = self.rates.borrow_mut();
        let now = Instant::now();
        let start = *rates.window_start.get_or_insert(now);
        if now.duration_since(start) >= Duration::from_secs(1) {
            rates.timer_hz = rates.timer;
            rates.surface_hz = rates.surface;
            rates.timer = 0;
            rates.surface = 0;
            rates.window_start = Some(now);
        }
        match driver {
            Driver::Timer => rates.timer += 1,
            Driver::Surface => rates.surface += 1,
        }
    }
}

/// Tells the socket threads the main thread is done with the request it was running.
struct StopWorking<'a>(&'a Mutex<Snapshot>);

impl Drop for StopWorking<'_> {
    fn drop(&mut self) {
        let mut snapshot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        snapshot.working = None;
        // (the main thread is back from the request: the time it spent there is no stall of the next request's)
        snapshot.last_tick = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::Outcome;
    use crate::reaper::fake;
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;

    static OPEN: AtomicBool = AtomicBool::new(false);

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
        fn take(&self) -> Vec<(String, Value)> {
            self.0.take().into_iter().filter(|(name, _)| name != "heartbeat").collect()
        }
    }

    fn setup() -> (Extension, std::sync::mpsc::Sender<Job>, Recorder, Arc<Mutex<Snapshot>>) {
        fake::reset();
        let (tx, rx) = queue::channel();
        let recorder = Recorder::default();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let extension = Extension::new(fake::api(), rx, Box::new(recorder.clone()), snapshot.clone());
        (extension, tx, recorder, snapshot)
    }

    fn ask(extension: &Extension, tx: &std::sync::mpsc::Sender<Job>, method: &str) -> Outcome {
        let (job, waiter) = queue::job(method, json!({}));
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        waiter.wait(Duration::from_secs(1), &OPEN)
    }

    #[test]
    fn each_tick_refreshes_what_the_socket_threads_serve() {
        let (extension, _tx, _, _) = setup();
        fake::with(|s| s.play_bits = 1);
        extension.tick(Driver::Timer);
        let snapshot = extension.snapshot();
        assert_eq!(snapshot.daw_version, "7.82/macOS-arm64");
        assert_eq!(snapshot.resource, "/lab/REAPER");
        assert_eq!(snapshot.project, Some(ProjectSummary::default()));
        assert_eq!(snapshot.busy, None);
        assert!(snapshot.last_tick.is_some());
        assert_eq!(snapshot.status["tracks"], 2);
        assert_eq!(snapshot.status["transport"]["playing"], true);
        assert_eq!(snapshot.status["api"], json!({"resolved": fake::api().resolved(), "total": 877}));
        fake::with(|s| s.loading = true);
        extension.tick(Driver::Timer);
        assert_eq!(extension.snapshot().busy, Some(Busy::Loading));
    }

    #[test]
    fn a_reaper_too_old_for_a_feature_says_so_in_its_status() {
        let (extension, _tx, _, snapshot) = setup();
        extension.tick(Driver::Timer);
        assert_eq!(snapshot.lock().unwrap().status.get("missing"), None, "nothing is missing from a complete API");
        fake::reset();
        let (_tx2, rx) = queue::channel();
        let old = Extension::new(
            fake::api_without(&["GetRegionOrMarker", "Undo_GetNumEntries"]),
            rx,
            Box::new(Recorder::default()),
            Arc::new(Mutex::new(Snapshot::default())),
        );
        old.tick(Driver::Timer);
        let status = old.snapshot().status;
        let missing: Vec<(&str, &str)> =
            status["missing"].as_array().unwrap().iter().map(|m| (m["feature"].as_str().unwrap(), m["needs"].as_str().unwrap())).collect();
        assert_eq!(missing.iter().map(|m| m.1).collect::<Vec<_>>(), ["7.62", "7.79"]);
        assert!(missing[0].0.contains("markers"), "{missing:?}");
    }

    #[test]
    fn unknown_methods_are_refused_by_name() {
        let (extension, tx, _, _) = setup();
        let Outcome::Answered(Err((code, message))) = ask(&extension, &tx, "explode") else { panic!() };
        assert_eq!(code, "unknown-method");
        assert!(message.contains("explode"));
    }

    fn ask_with(extension: &Extension, tx: &std::sync::mpsc::Sender<Job>, method: &str, params: Value) -> Answer {
        let (job, waiter) = queue::job(method, params);
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        match waiter.wait(Duration::from_secs(1), &OPEN) {
            Outcome::Answered(answer) => answer,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn actions_run_by_id_or_name_but_never_a_dialog() {
        let (extension, tx, _, _) = setup();
        assert_eq!(
            ask_with(&extension, &tx, "action", json!({"command": 40001})),
            Ok(json!({"command": 40001, "name": "Track: Insert new track"}))
        );
        assert_eq!(
            ask_with(&extension, &tx, "action", json!({"command": "_RS_kumi"})),
            Ok(json!({"command": 54000, "name": "Script: kumi.lua"}))
        );
        let refused = |params: Value| ask_with(&extension, &tx, "action", params).unwrap_err();
        assert!(refused(json!({"command": 40022})).1.contains("dialog"));
        assert!(refused(json!({"command": 40004})).1.contains("quit"));
        assert!(refused(json!({"command": 99999})).1.contains("no action"));
        assert!(refused(json!({"command": "_nope"})).1.contains("no action"));
        assert_eq!(refused(json!({})).0, "bad-params");
        assert_eq!(fake::with(|s| s.actions.clone()), [40001, 54000]);
    }

    #[test]
    fn reads_and_snapshots_are_answered_on_the_main_thread() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        let read = ask_with(&extension, &tx, "read", json!({"kind": "track", "limit": 2})).unwrap();
        assert_eq!(read["items"][1]["name"], "Drums");
        assert!(read["nextCursor"].is_string());
        let snapshot = ask_with(&extension, &tx, "snapshot", json!({})).unwrap();
        assert_eq!(snapshot["tracks"].as_array().unwrap().len(), 5);
        assert_eq!(snapshot["epoch"], read["epoch"]);
        assert_eq!(ask_with(&extension, &tx, "read", json!({})).unwrap_err().0, "bad-params");
        let body = ask_with(&extension, &tx, "focus_body", json!({"kind": "routing", "ref": "r:track:{AAAA}"})).unwrap();
        assert_eq!(
            (&body["kind"], &body["epoch"], &body["routes"][0]["destRef"]),
            (&json!("routing"), &read["epoch"], &json!("r:track:{DDDD}"))
        );
        assert_eq!(ask_with(&extension, &tx, "focus_body", json!({"kind": "console"})).unwrap_err().0, "bad-params");
    }

    fn focus_of(extension: &Extension) -> Value {
        extension.snapshot().focus
    }

    #[test]
    fn focus_is_polled_each_tick_and_kept_for_the_socket_threads() {
        let (extension, _tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let focus = focus_of(&extension);
        assert_eq!(focus["view"], "arrange");
        assert_eq!(focus["changed"], 11);
        assert_eq!(focus["selection"]["trackCount"], 0);
        assert!(extension.snapshot().focus_at.is_some());
        // Within the poll interval a tick leaves it be; a state change shows once it is due.
        fake::with(|s| s.state_count = 12);
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["changed"], 11);
        std::thread::sleep(FOCUS_EVERY + Duration::from_millis(5));
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["changed"], 12);
    }

    /// `GetTouchedOrFocusedFX` names a track by its number, in whichever project is active: after a tab switch it can be
    /// about the project before.
    #[test]
    fn the_touched_parameter_from_before_a_tab_switch_is_not_this_projects_until_reaper_says_another() {
        let (extension, _tx, _, _) = setup();
        fake::lab_project();
        fake::with(|s| {
            s.tracks[0].fx[0].params = vec![fake::FakeParam::new("Gain", 0.75, "-3.0 dB")];
            s.touched_fx = Some((0, -1, -1, 0, 0));
        });
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["selection"]["param"]["name"], "Gain");
        // The producer switches to another project: REAPER still says the same.
        fake::with(|s| s.project_id += 1);
        extension.on_surface(Call::TrackListChange);
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["selection"].get("param"), None);
        // It says something else once a parameter is touched there.
        fake::with(|s| {
            s.tracks[0].fx[0].params.push(fake::FakeParam::new("Pan", 0.5, "C"));
            s.touched_fx = Some((0, -1, -1, 0, 1));
        });
        extension.on_surface(Call::TrackListChange);
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["selection"]["param"]["name"], "Pan");
    }

    #[test]
    fn the_surface_saying_the_selection_moved_refreshes_focus_at_once() {
        let (extension, _tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["selection"]["trackRef"], Value::Null);
        fake::select_tracks(&[1]);
        extension.on_surface(Call::TrackSelection(fake::track_ptr(1)));
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["selection"]["trackRef"], "r:track:{BBBB}");
        // A focused FX window comes with the surface's word for it.
        fake::with(|s| s.focused_fx = Some((0, -1, -1, 0, 0)));
        extension.on_surface(Call::Extended {
            call: api::CSURF_EXT_SETFOCUSEDFX,
            parm1: std::ptr::null_mut(),
            parm2: std::ptr::null_mut(),
            parm3: std::ptr::null_mut(),
        });
        extension.tick(Driver::Timer);
        assert_eq!((&focus_of(&extension)["view"], &focus_of(&extension)["selection"]["fxRef"]), (&json!("fx"), &json!("r:fx:{FX-EQ}")));
    }

    #[test]
    fn a_tab_switch_changes_the_epoch_and_the_focus_that_carries_it() {
        let (extension, _tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = focus_of(&extension)["epoch"].clone();
        fake::with(|s| s.project_id += 1);
        extension.tick(Driver::Timer);
        let after = focus_of(&extension)["epoch"].clone();
        assert_ne!(before, after);
        assert_eq!(after, extension.reads.epoch());
    }

    #[test]
    fn a_request_waiting_at_a_tab_switch_is_answered_under_the_new_epoch_and_a_cursor_from_before_is_refused() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let first = ask_with(&extension, &tx, "read", json!({"kind": "track", "limit": 1})).unwrap();
        let cursor = first["nextCursor"].clone();
        let before = first["epoch"].as_str().unwrap().to_string();
        // The producer switches tab; the next page's request was queued before the tick that notices.
        fake::with(|s| s.project_id += 1);
        let (code, message) = ask_with(&extension, &tx, "read", json!({"kind": "track", "cursor": cursor})).unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(message.contains("from before the project tab changed"), "{message}");
        // A read with no cursor, queued at the same moment, carries the new project's epoch, never the old one.
        fake::with(|s| s.project_id += 1);
        let after = ask_with(&extension, &tx, "read", json!({"kind": "track", "limit": 1})).unwrap();
        assert_ne!(after["epoch"].as_str().unwrap(), before);
        assert_eq!(after["epoch"], json!(extension.reads.epoch()));
        // A snapshot taken in that tick carries the new epoch too.
        fake::with(|s| s.project_id += 1);
        let snapshot = ask_with(&extension, &tx, "snapshot", json!({})).unwrap();
        assert_eq!(snapshot["epoch"], json!(extension.reads.epoch()));
    }

    /// The epoch and the project's identity as a read gives them.
    fn epoch_and_identity(extension: &Extension, tx: &std::sync::mpsc::Sender<Job>) -> (String, String) {
        let read = ask_with(extension, tx, "read", json!({"kind": "project"})).unwrap();
        (read["epoch"].as_str().unwrap().to_string(), read["items"][0]["identity"].as_str().unwrap().to_string())
    }

    const TEMPLATE_ID: &str = "11111111111111111111111111111111";
    const OWN_ID: &str = "22222222222222222222222222222222";

    fn set_id(id: Option<&str>) {
        fake::with(|s| match id {
            Some(id) => {
                s.project_ext.insert(("kumi".into(), "project".into()), id.into());
            }
            None => {
                s.project_ext.clear();
            }
        });
    }

    /// The `id` of the project row, as a read gives it.
    fn project_id(extension: &Extension, tx: &std::sync::mpsc::Sender<Job>) -> Value {
        ask_with(extension, tx, "read", json!({"kind": "project"})).unwrap()["items"][0]["id"].clone()
    }

    #[test]
    fn a_projects_id_travels_with_its_file_through_save_as_and_a_copy_opened_from_disk() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        set_id(Some(OWN_ID));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), OWN_ID);
        // Save As: the id is project data, so the copy is the same place (and nothing about the tab changed).
        let before = epoch_and_identity(&extension, &tx);
        fake::with(|s| s.project = Some(("/lab/Projects/Copy.rpp".into(), "Copy.rpp".into())));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), OWN_ID);
        assert_eq!(epoch_and_identity(&extension, &tx), before);
        // The file copied on disk and opened in the tab: another project object's worth of tracks, the same id.
        fake::with(|s| {
            s.project = Some(("/lab/Projects/Elsewhere/Song.rpp".into(), "Song.rpp".into()));
            s.tracks = vec![fake::FakeTrack::new("{OTHER-1}", "Pad")];
            s.state_count = 1;
        });
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx), before, "another project took the tab");
        assert_eq!(project_id(&extension, &tx), OWN_ID);
    }

    #[test]
    fn a_project_made_from_a_template_does_not_take_the_templates_id() {
        let (extension, tx, _, _) = setup();
        // New from a template saved with an id: unsaved, and the id is in it.
        fake::reset();
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), Value::Null, "an unsaved project has no id of its own");
        // The first save writes the file with the template's id in it. It still isn't this project's: until one is
        // written, the project is named by its path.
        fake::with(|s| s.project = Some(("/lab/Projects/Mix.rpp".into(), "Mix.rpp".into())));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), crate::read::path_id("/lab/Projects/Mix.rpp"));
        // Kumi writes the project's own id over it (M6): that is the id from then on, through Save As too.
        set_id(Some(OWN_ID));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), OWN_ID);
        fake::with(|s| s.project = Some(("/lab/Projects/Mix v2.rpp".into(), "Mix v2.rpp".into())));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), OWN_ID);
    }

    #[test]
    fn a_saved_project_without_an_id_is_named_by_its_path_until_one_is_written_and_the_snapshot_agrees() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        // The same file reopened after a restart is the same project.
        assert_eq!(project_id(&extension, &tx), crate::read::path_id("/lab/Projects/Song.rpp"));
        set_id(Some(OWN_ID));
        assert_eq!(project_id(&extension, &tx), OWN_ID);
        assert_eq!(ask_with(&extension, &tx, "snapshot", json!({})).unwrap()["project"]["id"], OWN_ID);
    }

    /// m6-places: the id a saved project is named by goes into it, so its place holds through a Save As (as Live's Sets
    /// do); a project made from a template gets its own over the template's at its first save, and an unsaved one none.
    #[test]
    fn a_saved_projects_path_id_is_written_into_it_and_a_save_as_keeps_it() {
        let (extension, tx, _, _) = setup();
        let stored = || fake::with(|s| s.project_ext.get(&("kumi".to_string(), "project".to_string())).cloned());
        fake::lab_project();
        extension.tick(Driver::Timer);
        let song = crate::read::path_id("/lab/Projects/Song.rpp");
        assert_eq!(stored().as_deref(), Some(song.as_str()), "written into the project");
        assert_eq!(project_id(&extension, &tx), song.as_str());
        fake::with(|s| s.project = Some(("/lab/Projects/Song v2.rpp".into(), "Song v2.rpp".into())));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), song.as_str(), "a Save As is the same place");
        // An id of its own is never written over.
        set_id(Some(OWN_ID));
        extension.tick(Driver::Timer);
        assert_eq!(stored().as_deref(), Some(OWN_ID));
        // From a template: nothing while unsaved; at the first save, the project's own over the template's.
        fake::reset();
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        assert_eq!((project_id(&extension, &tx), stored().as_deref()), (Value::Null, Some(TEMPLATE_ID)));
        fake::with(|s| s.project = Some(("/lab/Projects/Mix.rpp".into(), "Mix.rpp".into())));
        extension.tick(Driver::Timer);
        let mix = crate::read::path_id("/lab/Projects/Mix.rpp");
        assert_eq!((project_id(&extension, &tx), stored()), (Value::String(mix.clone()), Some(mix.clone())));
        fake::with(|s| s.project = Some(("/lab/Projects/Mix v2.rpp".into(), "Mix v2.rpp".into())));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), mix.as_str());
        // An unsaved project with none: none written.
        fake::reset();
        extension.tick(Driver::Timer);
        assert_eq!((project_id(&extension, &tx), stored()), (Value::Null, None));
    }

    #[test]
    fn opening_the_template_file_itself_is_the_templates_own_place() {
        let (extension, tx, _, _) = setup();
        fake::reset();
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), Value::Null);
        // The user opens the template's RPP (saved, with its id) in the same tab: that is the template's project.
        fake::with(|s| {
            s.project = Some(("/lab/Templates/Kit.rpp".into(), "Kit.rpp".into()));
            s.tracks = vec![fake::FakeTrack::new("{KIT-1}", "Kit")];
            s.state_count = 1;
        });
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), TEMPLATE_ID);
    }

    #[test]
    fn saving_a_project_under_another_name_keeps_the_epoch_and_the_identity() {
        let (extension, tx, recorder, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        recorder.take();
        fake::with(|s| s.project = Some(("/lab/Projects/Copy.rpp".into(), "Copy.rpp".into())));
        extension.tick(Driver::Timer);
        // The project event goes out (its name changed), but every ref still means what it did.
        assert_eq!(recorder.take().first().map(|(name, _)| name.as_str()), Some("project"));
        assert_eq!(epoch_and_identity(&extension, &tx), before);
    }

    #[test]
    fn a_new_project_opened_in_the_same_tab_is_a_new_epoch_and_a_new_identity() {
        let (extension, tx, recorder, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        recorder.take();
        // The tab keeps its project object; the file, the tracks and the state counter are another project's.
        fake::with(|s| {
            s.project = Some(("/lab/Projects/Other.rpp".into(), "Other.rpp".into()));
            s.tracks = vec![fake::FakeTrack::new("{OTHER-1}", "Pad")];
            s.state_count = 1;
        });
        extension.tick(Driver::Timer);
        let after = epoch_and_identity(&extension, &tx);
        assert_ne!(after.0, before.0);
        // The serial goes up; the tab's pointer is not in it.
        assert_eq!(
            (before.1.as_str(), after.1.as_str()),
            (format!("{}:1", std::process::id()).as_str(), format!("{}:2", std::process::id()).as_str())
        );
        assert_eq!(recorder.take().iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["project", "tracks", "selection"]);
        // A ref from the old project is not read against the new one.
        assert_eq!(ask_with(&extension, &tx, "read", json!({"ref": "r:track:{AAAA}"})).unwrap_err().0, "failed");
        assert_eq!(focus_of(&extension)["epoch"], after.0);
    }

    #[test]
    fn a_new_unsaved_project_in_an_unsaved_tab_is_noticed_by_its_tracks() {
        let (extension, tx, _, _) = setup();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        // Nothing about the file changed (there is none); the counter moved on, and none of the old tracks is left.
        fake::with(|s| {
            s.tracks = vec![fake::FakeTrack::new("{NEW-1}", "Keys")];
            s.state_count = 5;
        });
        extension.watch.borrow_mut().tracks_at = None;
        extension.tick(Driver::Timer);
        let after = epoch_and_identity(&extension, &tx);
        assert_ne!(after.0, before.0);
        assert_ne!(after.1, before.1);
    }

    #[test]
    fn a_change_inside_the_throttle_window_is_looked_at_when_it_is_over() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        // Every track replaced at once (a script, a paste of a whole project), a moment after the tracks were last read.
        fake::with(|s| {
            s.tracks = vec![fake::FakeTrack::new("{NEW-1}", "Keys")];
            s.state_count += 1;
        });
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), before, "held back by the throttle");
        // The counter does not move again; the change is not forgotten.
        extension.watch.borrow_mut().tracks_at = None;
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx).0, before.0);
    }

    #[test]
    fn editing_the_project_is_not_a_new_project() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        // A track added, one removed, an undo: the counter moves, the tracks overlap.
        for step in 0..3 {
            fake::with(|s| {
                match step {
                    0 => s.tracks.push(fake::FakeTrack::new("{EXTRA}", "Extra")),
                    1 => {
                        s.tracks.remove(0);
                    }
                    _ => {
                        s.tracks.pop();
                    }
                }
                s.state_count += 1;
            });
            extension.watch.borrow_mut().tracks_at = None;
            extension.tick(Driver::Timer);
            assert_eq!(epoch_and_identity(&extension, &tx), before, "step {step}");
        }
        // The first track of an empty project is no new project either.
        fake::with(|s| {
            s.tracks.clear();
            s.state_count += 1;
        });
        extension.watch.borrow_mut().tracks_at = None;
        extension.tick(Driver::Timer);
        fake::with(|s| {
            s.tracks.push(fake::FakeTrack::new("{FIRST}", "First"));
            s.state_count += 1;
        });
        extension.watch.borrow_mut().tracks_at = None;
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), before);
    }

    #[test]
    fn a_fresh_empty_project_in_the_tab_is_noticed_when_the_counter_goes_back() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        fake::with(|s| {
            s.tracks.clear();
            s.state_count = 0;
        });
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx).0, before.0);
    }

    #[test]
    fn the_classifier_weighs_a_load_then_tracks_then_file_then_counter() {
        let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<HashSet<_>>();
        let (a, ab, c, none) = (set(&["a"]), set(&["a", "b"]), set(&["c"]), set(&[]));
        let moved = |file_changed, counter_went_back| Moves { file_changed, counter_went_back, ..Moves::default() };
        assert!(!is_another_project(&a, &ab, moved(false, false)));
        assert!(!is_another_project(&ab, &a, moved(true, true)), "a file saved under another name keeps its tracks");
        assert!(is_another_project(&a, &c, moved(false, false)), "no track in common is another project, whatever the file");
        assert!(!is_another_project(&none, &a, moved(false, false)), "the first track of an empty project");
        assert!(is_another_project(&a, &none, moved(false, true)), "tracks gone and the counter back");
        assert!(!is_another_project(&a, &none, moved(false, false)), "tracks gone by editing");
        assert!(is_another_project(&none, &none, moved(true, false)), "another file, nothing to compare");
        assert!(!is_another_project(&none, &none, moved(false, true)));
        // A load, or a saved project that became unsaved, is another project with the very same tracks: a template carries
        // the GUIDs of the project it was made from, and so does a copy of a file.
        assert!(is_another_project(&ab, &ab, Moves { history_reset: true, ..Moves::default() }));
        assert!(is_another_project(&ab, &ab, Moves { went_unsaved: true, file_changed: true, ..Moves::default() }));
    }

    /// A project the bridge sees, with the file it came from and how far its undo history has got.
    fn project_with_history(path: &str, undo: i32) {
        fake::with(|s| {
            s.project = Some((path.into(), path.rsplit('/').next().unwrap_or("").into()));
            s.undo_entries = undo;
        });
    }

    #[test]
    fn a_template_made_from_the_open_project_is_a_new_project_though_it_has_the_same_tracks() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        // New project from a template that was saved from this very project: unsaved, the same GUIDs.
        project_with_history("", 1);
        extension.tick(Driver::Timer);
        let after = epoch_and_identity(&extension, &tx);
        assert_ne!(after, before);
    }

    #[test]
    fn a_first_save_is_the_same_project_and_opening_a_file_is_not_even_with_the_same_tracks() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        // The template-born project: unsaved, with the template's id in it and some edits made.
        project_with_history("", 5);
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        let unsaved = epoch_and_identity(&extension, &tx);
        // The first save: the file gets a name, the undo history stays, the project is the one it was.
        project_with_history("/lab/Projects/Mix.rpp", 5);
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), unsaved);
        assert_eq!(project_id(&extension, &tx), crate::read::path_id("/lab/Projects/Mix.rpp"), "still not the template's id: its path's");
        // A copy of the template's file opened in the tab: a load, the undo history starts over, whatever the tracks. (The
        // file carries the template's id: the fake keeps one project's values, so the opened file's are set here.)
        project_with_history("/lab/Projects/Template copy.rpp", 1);
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx), unsaved);
        assert_eq!(project_id(&extension, &tx), TEMPLATE_ID, "a copy of a file shares its place");
    }

    #[test]
    fn opening_a_file_over_an_unsaved_project_with_edits_is_a_load() {
        let (extension, tx, _, _) = setup();
        fake::reset();
        project_with_history("", 4);
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), Value::Null);
        // The template's own file, opened over it, with the same tracks (they came from it).
        project_with_history("/lab/Templates/Kit.rpp", 1);
        extension.tick(Driver::Timer);
        assert_eq!(project_id(&extension, &tx), TEMPLATE_ID);
    }

    /// The gap in the heuristics: an unedited project made from a template, then the template's own file opened over it.
    /// Its file changes (from none to a name), which a first save does too; its undo history stays at one entry; its
    /// tracks are the same. Only REAPER's word that a project was loaded says it is another one.
    #[test]
    fn a_file_opened_over_an_unedited_template_project_is_a_load_because_reaper_says_so() {
        let (extension, tx, _, _) = setup();
        fake::reset();
        project_with_history("", 1);
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        let template_born = epoch_and_identity(&extension, &tx);
        assert_eq!(project_id(&extension, &tx), Value::Null, "an unsaved project has no id of its own");
        // File > Open of the template's RPP in the same tab: the same tracks, one undo entry before and after.
        project_with_history("/lab/Templates/Kit.rpp", 1);
        crate::projectconfig::note_load();
        extension.tick(Driver::Timer);
        let opened = epoch_and_identity(&extension, &tx);
        assert_ne!(opened.0, template_born.0, "a new epoch");
        assert_ne!(opened.1, template_born.1, "a new identity");
        assert_eq!(project_id(&extension, &tx), TEMPLATE_ID, "the template's own id is its own, not the one the project inherited");
        // The same without REAPER's word is what the heuristics take for a first save.
        let (extension, tx, _, _) = setup();
        fake::reset();
        project_with_history("", 1);
        set_id(Some(TEMPLATE_ID));
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        project_with_history("/lab/Templates/Kit.rpp", 1);
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), before, "a first save");
    }

    /// A tab closed and a project opened that REAPER gave the same address, within one tick: nothing about the pointer, the
    /// file or the tracks need differ, and it is another project all the same.
    #[test]
    fn a_tab_closed_and_a_project_opened_at_its_address_within_one_tick_is_another_project() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        crate::projectconfig::note_load();
        extension.tick(Driver::Timer);
        let after = epoch_and_identity(&extension, &tx);
        assert_ne!(after.0, before.0);
        assert_ne!(after.1, before.1);
    }

    #[test]
    fn a_job_that_switches_the_project_leaves_the_jobs_behind_it_under_the_new_projects_epoch() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = extension.reads.epoch();
        // Another host's action switches to the next tab; Kumi's read is queued behind it, in the same tick.
        fn next_tab(_: i32) {
            fake::with(|s| s.project_id += 1);
        }
        fake::with(|s| s.on_action = Some(next_tab));
        // (40001 stands in for 40861: the fake runs any named action, and its hook does the switching.)
        let (switch, switched) = queue::job("action", json!({"command": 40001}));
        let (read, answered) = queue::job("read", json!({"kind": "track", "limit": 1}));
        tx.send(switch).unwrap();
        tx.send(read).unwrap();
        extension.tick(Driver::Timer);
        assert!(matches!(switched.wait(Duration::from_secs(1), &OPEN), Outcome::Answered(Ok(_))));
        let Outcome::Answered(Ok(page)) = answered.wait(Duration::from_secs(1), &OPEN) else { panic!("the read") };
        assert_ne!(page["epoch"], json!(before), "answered about the project REAPER has now");
        assert_eq!(page["epoch"], json!(extension.reads.epoch()));
    }

    #[test]
    fn a_load_into_another_tabs_project_is_not_another_project_in_this_tab() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        let active = extension.reaper().current_project().unwrap().ptr.addr();
        // REAPER loads something into another project (the render queue, a subproject): this tab is the same.
        crate::projectconfig::ask_with(move || Some(active + 16 * 7), move || Some(active));
        crate::projectconfig::note_load();
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), before);
        // A load into this tab's project is another project.
        crate::projectconfig::ask_with(move || Some(active), move || Some(active));
        crate::projectconfig::note_load();
        extension.tick(Driver::Timer);
        let after = epoch_and_identity(&extension, &tx);
        assert_ne!((after.0.as_str(), after.1.as_str()), (before.0.as_str(), before.1.as_str()));
    }

    #[test]
    fn a_tab_loaded_while_another_was_in_front_is_another_project_when_it_comes_to_the_front() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        let a = fake::with(|s| s.project_id);
        fake::with(|s| s.other_tabs = vec![a + 1]);
        extension.tick(Driver::Timer);
        // B in front, then A again.
        fake::with(|s| {
            s.other_tabs = vec![a];
            s.project_id = a + 1;
        });
        extension.tick(Driver::Timer);
        let b = epoch_and_identity(&extension, &tx);
        let b_ptr = extension.reaper().current_project().unwrap().ptr.addr();
        fake::with(|s| {
            s.other_tabs = vec![a + 1];
            s.project_id = a;
        });
        extension.tick(Driver::Timer);
        // B closed and C opened at B's pointer within one tick, while A was in front: a load into that pointer.
        crate::projectconfig::ask_with(move || Some(b_ptr), || None);
        crate::projectconfig::note_load();
        fake::with(|s| {
            s.other_tabs = vec![a];
            s.project_id = a + 1;
        });
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx).1, b.1, "C isn't B, though it has B's pointer");
    }

    #[test]
    fn a_load_reaper_doesnt_name_is_the_front_tabs_and_leaves_background_tabs_alone() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        let a = fake::with(|s| s.project_id);
        fake::with(|s| s.other_tabs = vec![a + 1]);
        extension.tick(Driver::Timer);
        // B in front once, then A again.
        fake::with(|s| {
            s.other_tabs = vec![a];
            s.project_id = a + 1;
        });
        extension.tick(Driver::Timer);
        let b = epoch_and_identity(&extension, &tx);
        fake::with(|s| {
            s.other_tabs = vec![a + 1];
            s.project_id = a;
        });
        extension.tick(Driver::Timer);
        // A load REAPER doesn't name, with A in front: A's, not B's.
        let front = extension.reaper().current_project().unwrap().ptr.addr();
        crate::projectconfig::ask_with(|| None, move || Some(front));
        crate::projectconfig::note_load();
        extension.tick(Driver::Timer);
        fake::with(|s| {
            s.other_tabs = vec![a];
            s.project_id = a + 1;
        });
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx).1, b.1, "B nobody touched is still B");
    }

    #[test]
    fn a_load_reaper_reports_while_it_loads_is_noticed_once_it_has_finished() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        crate::projectconfig::note_load();
        fake::with(|s| s.loading = true);
        extension.tick(Driver::Timer);
        fake::with(|s| s.loading = false);
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx).0, before.0);
        // One load is one new project, not a new one every tick after.
        let after = epoch_and_identity(&extension, &tx);
        extension.tick(Driver::Timer);
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), after);
    }

    #[test]
    fn reverting_to_the_saved_file_reloads_the_project_and_a_few_undos_dont() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        project_with_history("/lab/Projects/Song.rpp", 6);
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        // Entries come and go with edits; pruning an old one is not a load.
        project_with_history("/lab/Projects/Song.rpp", 5);
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), before);
        // Revert to saved: the file is loaded again.
        project_with_history("/lab/Projects/Song.rpp", 1);
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx).0, before.0);
    }

    #[test]
    fn nothing_is_concluded_while_reaper_loads_a_project() {
        let (extension, tx, _, _) = setup();
        fake::lab_project();
        extension.tick(Driver::Timer);
        let before = epoch_and_identity(&extension, &tx);
        // A load into the tab: the file and the tracks change while REAPER is busy, and nothing is judged then.
        fake::with(|s| {
            s.loading = true;
            s.project = Some(("/lab/Projects/Other.rpp".into(), "Other.rpp".into()));
            s.tracks = vec![fake::FakeTrack::new("{OTHER-1}", "Pad")];
            s.state_count = 1;
        });
        extension.tick(Driver::Timer);
        extension.tick(Driver::Timer);
        assert_eq!(extension.reads.epoch(), before.0);
        // When REAPER is done, it is the new project that is looked at, once.
        fake::with(|s| s.loading = false);
        extension.tick(Driver::Timer);
        assert_ne!(epoch_and_identity(&extension, &tx).0, before.0);
        let settled = epoch_and_identity(&extension, &tx);
        extension.tick(Driver::Timer);
        assert_eq!(epoch_and_identity(&extension, &tx), settled);
    }

    #[test]
    fn a_dialog_over_the_main_window_holds_requests_and_is_said_busy() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| s.main_enabled = false);
        // Disabled for long enough: a dialog. Requests wait, nothing is polled, and the snapshot says so.
        extension.modal_since.set(Some(Instant::now() - MODAL_AFTER));
        let (job, waiter) = queue::job("action", json!({"command": 40001}));
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        assert_eq!(extension.snapshot().busy, Some(Busy::Modal));
        assert_eq!(waiter.wait(Duration::from_millis(1), &OPEN), Outcome::Cancelled);
        assert!(fake::with(|s| s.actions.is_empty()));
        // The dialog closes: the next tick is free again, and the next request runs.
        fake::with(|s| s.main_enabled = true);
        extension.tick(Driver::Timer);
        assert_eq!(extension.snapshot().busy, None);
        assert!(ask_with(&extension, &tx, "action", json!({"command": 40001})).is_ok());
    }

    #[test]
    fn nothing_runs_in_the_first_moments_of_a_dialog_either() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| s.main_enabled = false);
        let (job, waiter) = queue::job("action", json!({"command": 40001}));
        tx.send(job).unwrap();
        // The window was just disabled: not yet a dialog to report, but no place to start work.
        extension.tick(Driver::Timer);
        assert_eq!(extension.snapshot().busy, None);
        assert!(fake::with(|s| s.actions.is_empty()), "a request ran inside the dialog");
        // It was only an instant: the request goes ahead once the window is back.
        fake::with(|s| s.main_enabled = true);
        extension.tick(Driver::Timer);
        assert!(matches!(waiter.wait(Duration::from_secs(1), &OPEN), Outcome::Answered(Ok(_))));
        assert_eq!(fake::with(|s| s.actions.clone()), [40001]);
    }

    #[test]
    fn a_window_disabled_for_an_instant_is_not_a_dialog() {
        let (extension, _tx, _, _) = setup();
        fake::with(|s| s.main_enabled = false);
        extension.tick(Driver::Timer);
        fake::with(|s| s.main_enabled = true);
        extension.tick(Driver::Timer);
        assert_eq!(extension.modal_since.get(), None);
        fake::with(|s| s.main_enabled = false);
        extension.tick(Driver::Timer);
        // The clock started again, not where it was.
        assert!(extension.modal_since.get().is_some_and(|since| since.elapsed() < MODAL_AFTER));
        assert_eq!(extension.snapshot().busy, None);
    }

    #[test]
    fn the_dialog_check_can_be_turned_off() {
        fake::reset();
        let (tx, rx) = queue::channel();
        let mut extension = Extension::new(fake::api(), rx, Box::new(Recorder::default()), Arc::new(Mutex::new(Snapshot::default())));
        extension.modal_check = false;
        fake::with(|s| s.main_enabled = false);
        extension.modal_since.set(Some(Instant::now() - MODAL_AFTER));
        let (job, waiter) = queue::job("action", json!({"command": 40001}));
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        assert!(matches!(waiter.wait(Duration::from_secs(1), &OPEN), Outcome::Answered(Ok(_))));
        assert_eq!(extension.snapshot().busy, None);
    }

    #[test]
    fn nothing_is_polled_while_reaper_loads_a_project() {
        let (extension, _tx, _, _) = setup();
        fake::lab_project();
        fake::with(|s| s.loading = true);
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension), Value::Null);
        fake::with(|s| s.loading = false);
        extension.tick(Driver::Timer);
        assert_eq!(focus_of(&extension)["view"], "arrange");
    }

    #[test]
    fn quit_refuses_unsaved_changes_unless_asked_to_save() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| s.dirty = true);
        assert!(ask_with(&extension, &tx, "quit", json!({})).unwrap_err().1.contains("never saved"));
        fake::with(|s| s.project = Some(("/lab/Projects/a.rpp".into(), "a.rpp".into())));
        assert!(ask_with(&extension, &tx, "quit", json!({})).unwrap_err().1.contains("a.rpp"));
        assert!(fake::with(|s| s.actions.is_empty()));
        assert_eq!(
            ask_with(&extension, &tx, "quit", json!({"save": true})),
            Ok(json!({"quitting": true, "saved": ["/lab/Projects/a.rpp"]}))
        );
        assert_eq!(fake::with(|s| s.saved), 1);
    }

    #[test]
    fn actions_that_would_ask_to_save_or_hold_the_main_thread_are_refused() {
        let (extension, tx, _, _) = setup();
        let refused = |command: i32| ask_with(&extension, &tx, "action", json!({"command": command})).unwrap_err();
        // A render holds the main thread for as long as it takes.
        assert!(refused(41824).1.contains("renders"));
        // Closing or replacing a project with changes asks "Save changes?" inside the request.
        fake::with(|s| {
            s.project = Some(("/lab/Projects/a.rpp".into(), "a.rpp".into()));
            s.dirty = true;
        });
        for command in [40860, 40023, 40886] {
            let (code, message) = refused(command);
            assert_eq!(code, "failed");
            assert!(message.contains("a.rpp"), "{message}");
        }
        assert!(fake::with(|s| s.actions.is_empty()));
        // Without changes they run; and Save on a project never saved would open Save As.
        fake::with(|s| s.dirty = false);
        assert!(ask_with(&extension, &tx, "action", json!({"command": 40860})).is_ok());
        fake::with(|s| s.project = Some((String::new(), String::new())));
        assert!(refused(SAVE_PROJECT).1.contains("Save As"));
        fake::with(|s| s.project = Some(("/lab/Projects/a.rpp".into(), "a.rpp".into())));
        assert!(ask_with(&extension, &tx, "action", json!({"command": SAVE_PROJECT})).is_ok());
        assert_eq!(fake::with(|s| s.actions.clone()), [40860, SAVE_PROJECT]);
    }

    #[test]
    fn quit_waits_for_a_recording_to_stop() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| s.play_bits = 4);
        assert!(ask_with(&extension, &tx, "quit", json!({})).unwrap_err().1.contains("recording"));
        fake::with(|s| s.play_bits = 0);
        assert!(ask_with(&extension, &tx, "quit", json!({})).is_ok());
    }

    #[test]
    fn only_the_timer_quits_never_the_control_surfaces_run() {
        let (extension, tx, _, _) = setup();
        ask(&extension, &tx, "quit");
        extension.on_surface(Call::Run);
        extension.on_surface(Call::Run);
        assert!(fake::with(|s| s.actions.is_empty()), "REAPER quit inside the surface's own Run()");
        extension.tick(Driver::Timer);
        assert_eq!(fake::with(|s| s.actions.clone()), [ACTION_QUIT]);
    }

    #[test]
    fn rendering_shows_in_the_snapshot_and_holds_requests() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| s.rendering = true);
        extension.tick(Driver::Timer);
        assert_eq!(extension.snapshot().busy, Some(Busy::Rendering));
        let (job, waiter) = queue::job("action", json!({"command": 40001}));
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        assert_eq!(waiter.wait(Duration::from_millis(1), &OPEN), Outcome::Cancelled);
        assert!(fake::with(|s| s.actions.is_empty()));
        fake::with(|s| s.rendering = false);
        extension.tick(Driver::Timer);
        assert_eq!(extension.snapshot().busy, None);
    }

    // An outbox whose heartbeat panics once: the kind of fault that gets past `guard` around one request.
    struct Panicky(Cell<bool>);

    impl Outbox for Panicky {
        fn event(&self, _: &str, _: Value) {}
        fn heartbeat(&self) {
            if self.0.replace(false) {
                panic!("a heartbeat failed");
            }
        }
    }

    #[test]
    fn a_panic_in_a_tick_doesnt_skip_every_tick_after_it() {
        fake::reset();
        let (tx, rx) = queue::channel();
        let extension = Extension::new(fake::api(), rx, Box::new(Panicky(Cell::new(true))), Arc::new(Mutex::new(Snapshot::default())));
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extension.tick(Driver::Timer)));
        assert!(first.is_err());
        let (job, waiter) = queue::job("action", json!({"command": 40001}));
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        assert!(matches!(waiter.wait(Duration::from_secs(1), &OPEN), Outcome::Answered(Ok(_))));
        // And the UI refresh hold is balanced when something unwinds while it is held.
        assert_eq!(fake::with(|s| s.ui_refresh), 0);
    }

    #[test]
    fn quit_looks_again_after_saving_and_stays_if_the_project_is_still_changed() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| {
            s.project = Some(("/lab/Projects/a.rpp".into(), "a.rpp".into()));
            s.dirty = true;
            s.save_fails = true;
        });
        let refused = ask_with(&extension, &tx, "quit", json!({"save": true})).unwrap_err();
        assert!(refused.1.contains("still has unsaved changes") && refused.1.contains("a.rpp"), "{refused:?}");
        extension.tick(Driver::Timer);
        assert!(fake::with(|s| s.actions.is_empty()), "REAPER was told to quit with a project changed");
        fake::with(|s| s.save_fails = false);
        assert!(ask_with(&extension, &tx, "quit", json!({"save": true})).is_ok());
    }

    #[test]
    fn quit_answers_first_and_runs_reapers_quit_on_the_next_tick() {
        let (extension, tx, _, _) = setup();
        assert_eq!(ask(&extension, &tx, "quit"), Outcome::Answered(Ok(json!({"quitting": true, "saved": []}))));
        assert!(fake::with(|s| s.actions.is_empty()));
        extension.tick(Driver::Timer);
        assert_eq!(fake::with(|s| s.actions.clone()), [ACTION_QUIT]);
        extension.tick(Driver::Timer);
        assert_eq!(fake::with(|s| s.actions.len()), 1);
    }

    #[test]
    fn requests_wait_while_reaper_loads_a_project() {
        let (extension, tx, _, _) = setup();
        fake::with(|s| s.loading = true);
        let (job, waiter) = queue::job("quit", json!({}));
        tx.send(job).unwrap();
        extension.tick(Driver::Timer);
        assert_eq!(waiter.wait(Duration::from_millis(1), &OPEN), Outcome::Cancelled);
    }

    #[test]
    fn unloading_answers_queued_jobs_without_running_them() {
        let (extension, tx, _, _) = setup();
        let (job, waiter) = queue::job("action", json!({"command": 40001}));
        tx.send(job).unwrap();
        extension.cancel_jobs();
        let Outcome::Answered(Err((code, message))) = waiter.wait(Duration::from_secs(1), &OPEN) else { panic!() };
        assert_eq!(code, "failed");
        assert!(message.contains("closing"));
        assert!(fake::with(|s| s.actions.is_empty()));
    }

    #[test]
    fn requests_run_between_one_ui_refresh_hold() {
        let (extension, tx, _, _) = setup();
        ask(&extension, &tx, "explode");
        assert_eq!(fake::with(|s| s.ui_refresh), 0);
    }

    #[test]
    fn surface_calls_become_coalesced_events_on_the_next_tick() {
        let (extension, _tx, recorder, _) = setup();
        extension.tick(Driver::Timer);
        recorder.take();
        let track = 0x1008 as *mut api::MediaTrack;
        extension.on_surface(Call::TrackListChange);
        extension.on_surface(Call::TrackListChange);
        fake::select_tracks(&[1]);
        extension.on_surface(Call::Selected(track, true));
        extension.on_surface(Call::TrackSelection(track));
        extension.on_surface(Call::PlayState { play: true, pause: false, rec: false });
        assert!(recorder.take().is_empty());
        extension.on_surface(Call::Run);
        assert_eq!(
            recorder.take(),
            [
                ("transport".to_string(), json!({"playing": true, "paused": false, "recording": false, "bpm": 120.0})),
                ("tracks".to_string(), json!({"count": 2})),
                ("selection".to_string(), json!({"tracks": [{"index": 2, "guid": "{BBBB}", "name": "Bass"}]})),
            ]
        );
        // The same play state again is not news; a tempo change is.
        extension.on_surface(Call::PlayState { play: true, pause: false, rec: false });
        extension.tick(Driver::Timer);
        assert!(recorder.take().is_empty());
        fake::with(|s| {
            s.bpm = 96.0;
            s.play_bits = 1;
        });
        let tempo = Call::Extended {
            call: api::CSURF_EXT_SETBPMANDPLAYRATE,
            parm1: std::ptr::null_mut(),
            parm2: std::ptr::null_mut(),
            parm3: std::ptr::null_mut(),
        };
        extension.on_surface(tempo);
        extension.tick(Driver::Timer);
        assert_eq!(
            recorder.take(),
            [("transport".to_string(), json!({"playing": true, "paused": false, "recording": false, "bpm": 96.0}))]
        );
    }

    #[test]
    fn a_project_switch_sends_the_project_then_its_tracks_and_selection() {
        let (extension, _tx, recorder, _) = setup();
        extension.tick(Driver::Timer);
        recorder.take();
        fake::with(|s| s.project = Some(("/lab/Projects/b.rpp".into(), "b.rpp".into())));
        extension.tick(Driver::Timer);
        let events = recorder.take();
        assert_eq!(events[0], ("project".to_string(), json!({"name": "b.rpp", "path": "/lab/Projects/b.rpp"})));
        assert_eq!(events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["project", "tracks", "selection"]);
    }

    #[test]
    fn heartbeats_come_from_the_main_thread_once_a_second() {
        let (extension, _tx, recorder, _) = setup();
        extension.tick(Driver::Timer);
        extension.tick(Driver::Surface);
        let beats = recorder.0.borrow().iter().filter(|(n, _)| n == "heartbeat").count();
        assert_eq!(beats, 1);
    }
}
