//! Listening (M5): what Kumi hears in REAPER, and what it plays out loud.
//!
//! - **Renders** (`render_start`): a stretch rendered offline by REAPER into Kumi's folder (`render`).
//! - **Captures** (`capture_start`): what REAPER plays, from its hardware hook (`capture`).
//! - **Previews** (`audition`): a file played out loud through REAPER's preview, the project untouched (`audition`).
//! - **The audio behind an item or take** (`audio_file`).
//! - **Scratch copies** of a track for candidates heard side by side (`scratch`), and the change kind that keeps one
//!   (`scratch-keep`, in changes/).
//! - **Reads**: `spans` and a parameter's `grid` (`reads`, paged by read.rs).
//!
//! The jobs live on a board the socket threads read (`board`): `listen_status`, `listen_cancel` and `audition_stop` answer
//! at once, even while the main thread is inside REAPER's render. Files go only into Kumi's own folder,
//! `<Kumi's folder>/bridges/reaper/listening/`, owner-only, named `<jobId>-<sourceId>.wav`.

mod audition;
pub mod board;
mod cancel;
mod capture;
pub mod deliver;
mod inside;
pub mod reads;
pub mod render;
pub mod scratch;
pub mod wav;

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub use audition::Slot;
pub use board::Board;
use board::{Job, Kind, Playing, State};

use crate::extension::{Outbox, Snapshot};
use crate::queue::Answer;
use crate::read::Reads;
use crate::reaper::{Busy, ProjectInfo, Reaper};
use crate::refs::{Ref, TrackId};

/// The listening protocol `status` names (M5 contract §2.8).
pub const VERSION: u32 = 1;
/// The event a sweep sends: `{count, names}`.
pub const SWEPT: &str = "scratch_swept";
/// How often a tick inside REAPER's render tries Cancel again, while Kumi asks it to stop.
const PRESS_AGAIN: Duration = Duration::from_millis(500);
const HEARTBEAT: Duration = Duration::from_millis(daw_bridge::wire::HEARTBEAT_MS);

pub type Fault = (String, String);

pub(crate) fn bad(message: impl Into<String>) -> Fault {
    ("bad-params".into(), message.into())
}

pub(crate) fn failed(message: impl Into<String>) -> Fault {
    ("failed".into(), message.into())
}

pub(crate) fn gone(message: impl Into<String>) -> Fault {
    ("gone".into(), message.into())
}

pub(crate) fn stale(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::STALE.into(), message.into())
}

fn busy(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::BUSY.into(), message.into())
}

/// The listening folder for a bridge whose discovery folder is this: `<Kumi's folder>/bridges/reaper/listening`.
pub fn folder_beside(discovery: &Path) -> Option<PathBuf> {
    discovery.parent().map(|bridge| bridge.join("listening"))
}

/// Makes the folder (owner-only on unix; Windows' folders take the bridge folder's rights).
fn make_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = folder.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(folder) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && folder.is_dir() => {
                std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(folder)
    }
}

/// What the main thread holds of listening.
pub struct Listening {
    board: Arc<Board>,
    folder: RefCell<Option<PathBuf>>,
    /// A render `render_start` planned, run on a following tick (`queued_now`: planned in this tick, so it waits for the
    /// next, and a cancel can still reach it while it is queued).
    pending: RefCell<Option<render::Plan>>,
    /// (#401) A delivery `deliver_start` planned: the producer's files, rendered on a following tick as `pending` is.
    delivery: RefCell<Option<deliver::Plan>>,
    queued_now: Cell<bool>,
    items: RefCell<Option<render::Items>>,
    capture: RefCell<Option<capture::Capture>>,
    /// Hooks taken out, freed a while later.
    retired: RefCell<Vec<(Instant, capture::Registered)>>,
    preview: RefCell<Option<audition::Preview>>,
    scratch: RefCell<scratch::Scratch>,
    /// Inside REAPER's render for Kumi: the ticks REAPER gives meanwhile only report and cancel.
    rendering: Cell<bool>,
    /// When Kumi's last render ended: REAPER closes its audio device to render and reopens it a moment after (0.2 s,
    /// measured), so an audition just after it may open the device itself; at any other time a closed device is the
    /// producer's choice.
    rendered_at: Cell<Option<Instant>>,
    /// When the render window's Cancel was last pressed (for this render).
    pressed: Cell<Option<Instant>>,
    told_titles: Cell<bool>,
    swept: Cell<bool>,
    beat: Cell<Option<Instant>>,
    /// Ticks inside the render now running (for the log: whether the platform's timer reached it).
    inside_ticks: Cell<u32>,
    /// Copies swept since the bridge loaded (a sweep at load comes before any host connects): `status` carries them.
    swept_names: RefCell<Vec<String>>,
}

/// What a render asks of its runner, from inside REAPER's render loop too.
struct RunHooks<'a> {
    listening: &'a Listening,
    snapshot: &'a Mutex<Snapshot>,
    out: &'a dyn Outbox,
    job: &'a str,
}

/// Inside REAPER's render, from Kumi's own timer: the same as a tick REAPER would give there.
fn inside_render(data: *const std::ffi::c_void) {
    // SAFETY: `data` is the RunHooks that started the timer, which stops before the RunHooks goes (see `render`).
    let hooks = unsafe { &*(data as *const RunHooks) };
    hooks.listening.nested(hooks.snapshot, hooks.out);
}

impl render::Hooks for RunHooks<'_> {
    fn render(&self, run: &mut dyn FnMut()) {
        self.listening.rendering.set(true);
        {
            let mut snapshot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
            snapshot.last_tick = Some(Instant::now());
            snapshot.busy = Some(Busy::KumiRender);
        }
        // Dropped when the render returns (or unwinds): it never outlives this call.
        self.listening.inside_ticks.set(0);
        let timer = inside::Timer::start(self as *const RunHooks as *const std::ffi::c_void, inside_render);
        let timed = timer.is_some();
        run();
        drop(timer);
        crate::log::line(&format!(
            "{} ticks inside REAPER's render (Kumi's own timer {})",
            self.listening.inside_ticks.get(),
            if timed { "running" } else { "missing on this platform" }
        ));
        self.listening.rendering.set(false);
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).last_tick = Some(Instant::now());
    }

    fn watch(&self, watch: board::Watch) {
        self.listening.board.update(self.job, |job| job.watch = Some(watch));
    }

    fn cancelled(&self) -> bool {
        self.listening.board.cancelled(self.job)
    }

    fn pressed(&self) -> bool {
        self.listening.pressed.get().is_some()
    }
}

impl Listening {
    pub fn new(board: Arc<Board>) -> Listening {
        Listening {
            board,
            folder: RefCell::new(None),
            pending: RefCell::new(None),
            delivery: RefCell::new(None),
            queued_now: Cell::new(false),
            items: RefCell::new(None),
            capture: RefCell::new(None),
            retired: RefCell::new(Vec::new()),
            preview: RefCell::new(None),
            scratch: RefCell::new(scratch::Scratch::default()),
            rendering: Cell::new(false),
            pressed: Cell::new(None),
            told_titles: Cell::new(false),
            rendered_at: Cell::new(None),
            swept: Cell::new(false),
            beat: Cell::new(None),
            inside_ticks: Cell::new(0),
            swept_names: RefCell::new(Vec::new()),
        }
    }

    /// Where listening's files go, once the bridge knows Kumi's folder (from its reference).
    pub fn set_folder(&self, folder: PathBuf) {
        *self.folder.borrow_mut() = Some(folder);
    }

    /// The folder, made if it isn't there yet.
    fn folder(&self) -> Result<PathBuf, Fault> {
        let folder = self.folder.borrow().clone().ok_or_else(|| failed("the bridge doesn't know Kumi's folder yet (no reference file)"))?;
        make_folder(&folder).map_err(|error| failed(format!("Kumi couldn't make its listening folder {} ({error})", folder.display())))?;
        Ok(folder)
    }

    /// One job at a time: why another can't start now.
    fn one_at_a_time(&self) -> Result<(), Fault> {
        match self.board.active() {
            Some((_, Kind::Render)) => Err(busy("REAPER is rendering for Kumi; nothing changed")),
            Some((_, Kind::Capture)) => Err(busy("Kumi is listening to REAPER already; nothing changed")),
            None => Ok(()),
        }
    }

    // ----- methods (the main thread's) -----

    /// `render_start`: checked and planned now, rendered on a following tick; answered at once.
    pub fn render_start(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        let job = render::job_id(params)?;
        if self.board.knows(&job) {
            return Err(bad(format!("there is a job {job} already")));
        }
        self.one_at_a_time()?;
        if let Some(busy_now) = reaper.busy() {
            return Err(busy(format!("{}; nothing changed", busy_now.reason())));
        }
        let folder = self.folder()?;
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        if reaper.play_state(info.ptr).recording {
            return Err(failed("REAPER is recording: a render would stop the recording, so Kumi renders nothing"));
        }
        let plan = render::plan(reaper, reads, params, &folder)?;
        let answer = plan.answer();
        self.board.add(Job::new(&plan.job, Kind::Render, true));
        self.pressed.set(None);
        self.told_titles.set(false);
        *self.pending.borrow_mut() = Some(plan);
        self.queued_now.set(true);
        Ok(answer)
    }

    /// `deliver_targets` (#401): the files a delivery would write, with every render setting put back before the answer.
    pub fn deliver_targets(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        self.one_at_a_time()?;
        if let Some(busy_now) = reaper.busy() {
            return Err(busy(format!("{}; nothing changed", busy_now.reason())));
        }
        deliver::targets(reaper, reads, params)
    }

    /// `deliver_start` (#401): a delivery checked and planned now, rendered on a following tick; answered at once.
    pub fn deliver_start(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        let job = render::job_id(params)?;
        if self.board.knows(&job) {
            return Err(bad(format!("there is a job {job} already")));
        }
        self.one_at_a_time()?;
        if let Some(busy_now) = reaper.busy() {
            return Err(busy(format!("{}; nothing changed", busy_now.reason())));
        }
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        if reaper.play_state(info.ptr).recording {
            return Err(failed("REAPER is recording: a render would stop the recording, so Kumi renders nothing"));
        }
        let plan = deliver::plan(reaper, reads, params)?;
        let answer = json!({"jobId": plan.job, "state": "queued", "folder": plan.spec.folder.to_string_lossy()});
        // (a REAPER render: its Cancel and its busy answers are a render's)
        self.board.add(Job::new(&plan.job, Kind::Render, true));
        self.pressed.set(None);
        self.told_titles.set(false);
        *self.delivery.borrow_mut() = Some(plan);
        self.queued_now.set(true);
        Ok(answer)
    }

    /// `capture_start`: the hook registered now; it runs while REAPER plays.
    pub fn capture_start(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        let job = render::job_id(params)?;
        if self.board.knows(&job) {
            return Err(bad(format!("there is a job {job} already")));
        }
        let seconds = params.get("seconds").and_then(Value::as_f64).filter(|s| (1.0..=capture::MAX_SECONDS).contains(s));
        let seconds = seconds.ok_or_else(|| bad(format!("seconds: 1 to {}", capture::MAX_SECONDS)))?;
        self.one_at_a_time()?;
        if let Some(free) = params.get("freeBytes").and_then(Value::as_u64) {
            let needed = (seconds * 96_000.0 * 8.0 * 1.5) as u64;
            if needed > free {
                return Err(failed(format!(
                    "Kumi's folder has {} MB free; this capture needs about {} MB",
                    free / 1_000_000,
                    needed.div_ceil(1_000_000)
                )));
            }
        }
        let folder = self.folder()?;
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        let file = folder.join(format!("{job}-master.wav"));
        if file.exists() {
            return Err(failed(format!("{} is in Kumi's folder already: Kumi never writes over a file", file.display())));
        }
        let capture = capture::Capture::begin(reaper, reads, &job, info.ptr, seconds, file).map_err(failed)?;
        let answer = capture.answer();
        let mut entry = Job::new(&job, Kind::Capture, false);
        entry.state = State::Running;
        self.board.add(entry);
        *self.capture.borrow_mut() = Some(capture);
        Ok(answer)
    }

    /// `audition`: plays a file out loud, stopping the one playing.
    pub fn audition(&self, reaper: &Reaper, params: &Value) -> Answer {
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        self.stop_preview(reaper);
        let after_render = self.rendered_at.get().is_some_and(|at| at.elapsed() < audition::AFTER_RENDER);
        let (preview, answer) = audition::Preview::start(reaper, info.ptr, params, after_render)?;
        crate::log::line(&format!("preview {} plays {}", preview.id, answer));
        self.board.set_playing(Some(Playing { id: preview.id.clone(), slot: preview.slot.clone(), stop: false }));
        *self.preview.borrow_mut() = Some(preview);
        Ok(answer)
    }

    fn stop_preview(&self, reaper: &Reaper) {
        if let Some(preview) = self.preview.borrow_mut().take() {
            self.board.set_playing(None);
            preview.stop(reaper);
        }
    }

    /// `scratch {action, …}`.
    pub fn scratch(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        if let Some(busy_now) = reaper.busy() {
            return Err(busy(format!("{}; nothing changed", busy_now.reason())));
        }
        let folder = self.folder()?;
        self.scratch.borrow_mut().answer(reaper, reads, &folder, params)
    }

    /// `audio_file {ref}`: the file an item's take (or a take) plays, and the part of it it plays.
    pub fn audio_file(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        let proj = info.ptr;
        if let Some(epoch) = params.get("epoch").and_then(Value::as_str) {
            if epoch != reads.epoch() && !reads.epoch_is_of(epoch, proj) {
                return Err(stale(format!(
                    "those refs are from an earlier epoch ({epoch}; now {}): the project in the tab changed, read again",
                    reads.epoch()
                )));
            }
        }
        let reference = params.get("ref").and_then(Value::as_str).ok_or_else(|| bad("ref: an item or a take"))?;
        let lost = |what: &str| gone(format!("that {what} isn't in the project any more (deleted, or another tab): read again"));
        let (item, take) = match Ref::parse(reference).map_err(bad)? {
            Ref::Item(guid) => {
                let item = reads.item_by_ref(reaper, proj, &guid).map_err(|_| lost("item"))?;
                (item, reaper.active_take(item).ok_or_else(|| failed("That item is empty: it has no sound of its own"))?)
            }
            Ref::Take(guid) => {
                let take = reads.take_by_ref(reaper, proj, &guid).map_err(|_| lost("take"))?;
                (reaper.take_item(take).ok_or_else(|| lost("take"))?, take)
            }
            Ref::Track(TrackId::Guid(_)) | Ref::Track(TrackId::Master) => {
                return Err(bad("that's a track: its sound is rendered (render), audio_file is an item's or a take's"))
            }
            other => return Err(bad(format!("ref must be an item or a take, not r:{}", other.kind()))),
        };
        let midi = "That's a MIDI item, which has no sound of its own: listen to its track instead (Kumi renders it)";
        let source = reaper.take_source(take).ok_or_else(|| failed("That item is empty: it has no sound of its own"))?;
        if reaper.take_is_midi(take) {
            return Err(failed(midi));
        }
        // A section, reversed or not, wraps the file's own source.
        let section = reaper.source_section(source).filter(|_| reaper.source_parent(source).is_some());
        let mut root = source;
        while let Some(parent) = reaper.source_parent(root) {
            root = parent;
        }
        let kind = reaper.source_type(root);
        if kind.eq_ignore_ascii_case("MIDI") || kind.eq_ignore_ascii_case("MIDIPOOL") {
            return Err(failed(midi));
        }
        if kind.to_ascii_uppercase().contains("RPP") {
            return Err(failed(
                "That's a subproject item, which has no sound file of its own: listen to its track instead (Kumi renders it)",
            ));
        }
        let file = reaper.source_file(root);
        if file.is_empty() {
            return Err(failed(
                "That item has no file of its own (it is made by REAPER as it plays): listen to its track instead (Kumi renders it)",
            ));
        }
        if !Path::new(&file).is_file() {
            return Err(failed(format!(
                "The item's file isn't there (offline media: {file}): bring it back, or listen to its track instead"
            )));
        }
        let seconds = reaper.source_seconds(root).unwrap_or(0.0);
        let rate = reaper.source_rate(root);
        let channels = reaper.source_channels(root);
        let play_rate = reaper.take_value(take, "D_PLAYRATE");
        let play_rate = if play_rate > 0.0 { play_rate } else { 1.0 };
        let start = reaper.take_value(take, "D_STARTOFFS") + section.map_or(0.0, |(offset, _, _)| offset);
        let length = reaper.item_value(item, "D_LENGTH") * play_rate;
        let offset = start.max(0.0);
        let played = length.min((seconds - offset).max(0.0));
        let mut answer = json!({
            "file": file,
            "seconds": crate::rows::seconds(seconds),
            "channels": channels,
            "sampleRate": rate,
            "kind": "source",
            "name": crate::rows::clip(&reaper.take_name(take)),
        });
        if offset > 5e-4 || played < seconds - 5e-4 {
            answer["offset"] = crate::rows::seconds(offset).into();
            answer["length"] = crate::rows::seconds(played).into();
        }
        if (play_rate - 1.0).abs() > 1e-9 {
            answer["playRate"] = play_rate.into();
        }
        if section.is_some_and(|(_, _, reversed)| reversed) {
            answer["reversed"] = true.into();
        }
        Ok(answer)
    }

    // ----- ticks -----

    /// Each tick: a preview's end, a capture's drain, a track's items read a slice further, a planned render run (it holds
    /// the main thread inside REAPER's render until it ends), the first sweep of scratch copies an earlier session left.
    pub fn tick(&self, reaper: &Reaper, reads: &Reads, free: bool, snapshot: &Mutex<Snapshot>, out: &dyn Outbox) {
        self.retired.borrow_mut().retain(|(at, _)| at.elapsed() < capture::RETIRE_AFTER);
        self.tick_preview(reaper);
        self.tick_capture(reaper, reads);
        // A track's items are read only while REAPER is free; a cancel of them is taken at once.
        self.tick_items(reaper, reads, free);
        if !free {
            self.give_up_waiting();
        }
        if free {
            self.run_pending(reaper, reads, snapshot, out);
            if !self.swept.get() && self.folder.borrow().is_some() {
                self.swept.set(true);
                self.sweep(reaper, &reaper.projects(), out, false);
            }
        }
    }

    fn tick_preview(&self, reaper: &Reaper) {
        let stop = match self.preview.borrow().as_ref() {
            Some(preview) => {
                let asked = self.board.playing().is_some_and(|(_, _, asked)| asked);
                let over = preview.over(reaper);
                if asked || over {
                    crate::log::line(&format!(
                        "preview {} stops at {:.2} s ({})",
                        preview.id,
                        preview.slot.position(),
                        if asked { "Kumi asked" } else { "its end, or REAPER plays" }
                    ));
                }
                asked || over
            }
            None => false,
        };
        if stop {
            self.stop_preview(reaper);
        }
    }

    fn tick_capture(&self, reaper: &Reaper, reads: &Reads) {
        let mut slot = self.capture.borrow_mut();
        let Some(capture) = slot.as_mut() else { return };
        let job = capture.job.clone();
        let cancelled = self.board.cancelled(&job);
        match capture.step(reaper, reads, cancelled) {
            None => {
                let progress = capture.progress();
                self.board.update(&job, |entry| entry.progress = progress);
            }
            Some(ended) => {
                let capture = slot.take().expect("the capture");
                self.retired.borrow_mut().push((Instant::now(), capture.registered));
                let state = if cancelled && ended.state == State::Done { State::Done } else { ended.state };
                self.board.update(&job, |entry| {
                    entry.state = state;
                    entry.result = ended.result;
                });
            }
        }
    }

    fn tick_items(&self, reaper: &Reaper, reads: &Reads, free: bool) {
        let mut slot = self.items.borrow_mut();
        let Some(items) = slot.as_mut() else { return };
        let job = items.job.clone();
        let cancelled = self.board.cancelled(&job);
        if !free && !cancelled {
            return;
        }
        match items.step(reaper, reads, cancelled) {
            render::Step::Going(progress) => {
                self.board.update(&job, |entry| entry.progress = progress);
            }
            render::Step::Ended(ended) => {
                slot.take();
                self.end(&job, ended);
            }
        }
    }

    /// Writes how a render ended on the board.
    fn end(&self, job: &str, ended: render::Ended) {
        self.board.update(job, |entry| match ended {
            render::Ended::Done { files, notes } => {
                entry.state = State::Done;
                entry.result.insert("files".into(), files);
                if !notes.is_empty() {
                    entry.result.insert("notes".into(), json!(notes));
                }
            }
            render::Ended::Failed { code, error, notes } => {
                entry.state = State::Failed;
                entry.result.insert("error".into(), error.into());
                entry.result.insert("code".into(), code.into());
                if !notes.is_empty() {
                    entry.result.insert("notes".into(), json!(notes));
                }
            }
            render::Ended::Cancelled { notes } => {
                entry.state = State::Cancelled;
                if !notes.is_empty() {
                    entry.result.insert("notes".into(), json!(notes));
                }
            }
        });
    }

    /// A planned render that has waited too long for REAPER to be free: given up, nothing done.
    fn give_up_waiting(&self) {
        let delivery_waited = self.delivery.borrow().as_ref().is_some_and(|plan| plan.queued.elapsed() > render::queued_for());
        if delivery_waited {
            if let Some(plan) = self.delivery.borrow_mut().take() {
                if self.board.start(&plan.job) {
                    let error = format!(
                        "REAPER stayed busy (a dialog, a load or its own render) for {} s: nothing was rendered",
                        render::queued_for().as_secs()
                    );
                    self.end(&plan.job, render::Ended::Failed { code: "busy".into(), error, notes: vec![] });
                }
            }
        }
        let waited = self.pending.borrow().as_ref().is_some_and(|plan| plan.queued.elapsed() > render::queued_for());
        if !waited {
            return;
        }
        if let Some(plan) = self.pending.borrow_mut().take() {
            if self.board.start(&plan.job) {
                let error = format!(
                    "REAPER stayed busy (a dialog, a load or its own render) for {} s: nothing was rendered",
                    render::queued_for().as_secs()
                );
                self.end(&plan.job, render::Ended::Failed { code: "busy".into(), error, notes: vec![] });
            }
        }
    }

    fn run_pending(&self, reaper: &Reaper, reads: &Reads, snapshot: &Mutex<Snapshot>, out: &dyn Outbox) {
        if self.queued_now.replace(false) {
            return;
        }
        if let Some(plan) = self.delivery.borrow_mut().take() {
            if self.board.start(&plan.job) {
                self.run_delivery(reaper, reads, &plan, snapshot, out);
            }
            return;
        }
        let Some(plan) = self.pending.borrow_mut().take() else { return };
        if !self.board.start(&plan.job) {
            // Cancelled while it waited: nothing was done.
            return;
        }
        if plan.items() {
            match render::Items::begin(reaper, reads, &plan) {
                Ok(items) => *self.items.borrow_mut() = Some(items),
                Err((code, error)) => self.end(&plan.job, render::Ended::Failed { code, error, notes: vec![] }),
            }
            return;
        }
        // REAPER closes its audio device to render (measured): an audition playing would stall and go on after.
        let auditioning = self.preview.borrow().is_some();
        if auditioning {
            crate::log::line("the preview stops: Kumi renders, and REAPER closes its audio device to render");
            self.stop_preview(reaper);
        }
        let hooks = RunHooks { listening: self, snapshot, out, job: &plan.job };
        let mut ended = render::run(reaper, reads, &plan, &hooks);
        self.rendering.set(false);
        self.rendered_at.set(Some(Instant::now()));
        if auditioning {
            ended.note_first("The audition playing was stopped: REAPER closes its audio device to render".into());
        }
        self.end(&plan.job, ended);
    }

    /// (#401) A delivery, run as a render is: an audition stops first (REAPER closes its audio device to render).
    fn run_delivery(&self, reaper: &Reaper, reads: &Reads, plan: &deliver::Plan, snapshot: &Mutex<Snapshot>, out: &dyn Outbox) {
        let auditioning = self.preview.borrow().is_some();
        if auditioning {
            self.stop_preview(reaper);
        }
        let hooks = RunHooks { listening: self, snapshot, out, job: &plan.job };
        let mut ended = deliver::run(reaper, reads, plan, &hooks);
        self.rendering.set(false);
        self.rendered_at.set(Some(Instant::now()));
        if auditioning {
            ended.note_first("The audition playing was stopped: REAPER closes its audio device to render".into());
        }
        self.end(&plan.job, ended);
    }

    /// A tick REAPER gives while Kumi's render holds the main thread inside its render loop: the socket threads are told it
    /// renders (not that it is stuck), and a cancel asked meanwhile presses the render window's Cancel. Nothing of the
    /// project is touched. False when Kumi isn't rendering (the tick was some other call's own).
    pub fn nested(&self, snapshot: &Mutex<Snapshot>, out: &dyn Outbox) -> bool {
        if !self.rendering.get() {
            return false;
        }
        self.inside_ticks.set(self.inside_ticks.get() + 1);
        {
            let mut snapshot = snapshot.lock().unwrap_or_else(|e| e.into_inner());
            snapshot.last_tick = Some(Instant::now());
            snapshot.busy = Some(Busy::KumiRender);
        }
        let asked = self.board.active().is_some_and(|(id, kind)| kind == Kind::Render && self.board.cancelled(&id));
        if asked {
            if !self.told_titles.replace(true) {
                crate::log::line(&format!("cancel asked inside the render; REAPER's windows: {:?}", cancel::titles()));
            }
            // REAPER asks whether to keep a cancelled render's files: no (Kumi deletes them).
            if self.pressed.get().is_some() && cancel::answer_incomplete() {
                crate::log::line("answered REAPER's question about the cancelled render's files: no");
            } else if self.pressed.get().is_none_or(|at| at.elapsed() >= PRESS_AGAIN) && cancel::press() {
                self.pressed.set(Some(Instant::now()));
            }
        }
        if self.beat.get().is_none_or(|at| at.elapsed() >= HEARTBEAT) {
            self.beat.set(Some(Instant::now()));
            out.heartbeat();
        }
        true
    }

    /// Scratch copies an earlier session left in these projects (or, `undo`, that REAPER's undo or redo brought back after
    /// Kumi dropped them) are removed, and the host told.
    fn sweep(&self, reaper: &Reaper, projects: &[ProjectInfo], out: &dyn Outbox, undo: bool) {
        let Some(folder) = self.folder.borrow().clone() else { return };
        let names = self.scratch.borrow_mut().sweep(reaper, &folder, projects);
        if !names.is_empty() {
            crate::log::line(&format!(
                "swept {} scratch copies ({})",
                names.len(),
                if undo { "REAPER's undo brought them back" } else { "an earlier session left them" }
            ));
            let mut event = json!({"count": names.len(), "names": names});
            if undo {
                event["why"] = "undo".into();
            }
            out.event(SWEPT, event);
            let mut swept = self.swept_names.borrow_mut();
            swept.extend(names);
            let over = swept.len().saturating_sub(64);
            swept.drain(..over);
        }
    }

    /// Another project came to the front (a tab, a load): copies an earlier session left in it are swept.
    pub fn project_changed(&self, reaper: &Reaper, out: &dyn Outbox) {
        if let Some(info) = reaper.current_project() {
            self.sweep(reaper, &[info], out, false);
        }
    }

    /// The producer's undo or redo moved the project in front: copies it brought back after Kumi dropped them are swept.
    pub fn undo_moved(&self, reaper: &Reaper, out: &dyn Outbox) {
        if !self.scratch.borrow().has_leftovers() {
            return;
        }
        if let Some(info) = reaper.current_project() {
            self.sweep(reaper, &[info], out, true);
        }
    }

    /// What works now (`status.listening`).
    pub fn flags(&self, reaper: &Reaper, audio: bool) -> Value {
        let api = reaper.api();
        let folder = self.folder.borrow().is_some();
        let render =
            folder && api.GetSetProjectInfo().is_some() && api.GetSetProjectInfo_String().is_some() && api.Main_OnCommandEx().is_some();
        let mut flags = json!({
            "render": render,
            "capture": folder && audio && api.Audio_RegHardwareHook().is_some(),
            "audition": audio && api.PlayPreviewEx().is_some() && api.StopPreview().is_some() && api.PCM_Source_CreateFromFile().is_some(),
            "grid": api.TrackFX_FormatParamValueNormalized().is_some(),
            "scratch": folder && api.GetTrackStateChunk().is_some() && api.SetTrackStateChunk().is_some() && api.InsertTrackAtIndex().is_some(),
            // (#401) the producer's own files: deliver_targets and deliver_start
            "deliver": render,
            "version": VERSION,
        });
        let swept = self.swept_names.borrow();
        if !swept.is_empty() {
            flags["swept"] = json!({"count": swept.len(), "names": *swept});
        }
        flags
    }

    /// The bridge is going: the preview stops, the hook comes out, what was running ends (its files deleted); the scratch
    /// copies stay for the journal to sweep next time. A render REAPER is inside of is left to put itself back.
    pub fn stop(&self, reaper: &Reaper) {
        if self.rendering.get() {
            crate::log::line("the bridge unloads inside a render; its settings are put back when REAPER's render returns");
            return;
        }
        self.stop_preview(reaper);
        if let Some(mut capture) = self.capture.borrow_mut().take() {
            capture.abandon(reaper);
            self.board.update(&capture.job, |job| {
                job.state = State::Failed;
                job.result.insert("error".into(), "the bridge unloaded".into());
                job.result.insert("code".into(), "failed".into());
            });
            // REAPER no longer calls it, and the module is going: kept (never freed) rather than risk a block in flight.
            std::mem::forget(capture.registered);
        }
        for (_, registered) in self.retired.borrow_mut().drain(..) {
            std::mem::forget(registered);
        }
        if let Some(mut items) = self.items.borrow_mut().take() {
            let job = items.job.clone();
            let ended = items.stop(reaper, Some(("failed", "the bridge unloaded".into())));
            self.end(&job, ended);
        }
        if let Some(plan) = self.pending.borrow_mut().take() {
            self.end(&plan.job, render::Ended::Failed { code: "failed".into(), error: "the bridge unloaded".into(), notes: vec![] });
        }
        if let Some(plan) = self.delivery.borrow_mut().take() {
            self.end(&plan.job, render::Ended::Failed { code: "failed".into(), error: "the bridge unloaded".into(), notes: vec![] });
        }
        self.scratch.borrow_mut().forget();
    }

    /// The board the socket threads read.
    pub fn board(&self) -> &Arc<Board> {
        &self.board
    }

    /// Whether a track is a scratch copy standing now (for scratch-keep's words).
    pub fn copy_named(&self, guid: &str) -> Option<String> {
        self.scratch.borrow().copy_named(guid)
    }
}
