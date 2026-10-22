//! Offline renders (`render_start`): a stretch of the project, rendered by REAPER itself into Kumi's folder.
//!
//! `render_start` checks the request and plans the render; the render runs on the main thread's next tick, inside
//! REAPER's "render with the most recent settings" (42230), which holds the main thread until it is done. Before it, every
//! render setting of the project (`RENDER_*`), the track selection, the master's FX switch, the render speed and `DIRTY`
//! are noted; Kumi's own are set; after it (done, failed or cancelled) every one is put back exactly and read back.
//!
//! Measured in REAPER 7.82: source 0 is the master mix, 1 stems and the master, **3 stems only** (2 alone renders the master
//! and lists no targets); 8192 makes the stems pre-fader (a muted track's pre-fader stem still sounds); `$tracknumber`
//! names a stem `00` (the master) or `01`… ; a render doesn't make an undo point or mark the project changed; it stops
//! playback; a render speed other than full-speed offline renders in real time.
//!
//! A track's own audio before its FX (stage `items`) is read with an audio accessor instead, a slice a tick, with no render.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{bad, failed, gone, stale, wav, Fault};
use crate::read::Reads;
use crate::reaper::{Accessor, Project, Reaper, Track};
use crate::refs::{Ref, TrackId};

/// The longest stretch one render takes, in seconds of music.
pub const MAX_SECONDS: f64 = 3600.0;
/// The most sources one render has (eight candidates, a focus, the mix, the mix before its chain, a search's copies).
pub const MAX_SOURCES: usize = 18;
/// The sample rates Kumi renders at.
pub const RATES: [u32; 4] = [44100, 48000, 88200, 96000];
/// REAPER's WAV sink set to 32-bit float (`evaw`, 32, 0, 1): measured to write WAV format 3, 32 bits.
pub const FLOAT_WAV: &str = "ZXZhdyAAAQ==";
/// File: Render project, using the most recent render settings, auto-close render dialog.
pub const RENDER_WITH_LAST: i32 = 42230;
/// The project's render speed (`RENDER_1X` in its file): 0 is full-speed offline, 1 1x offline, 2 online (which plays
/// out loud), 3 and 4 the idle ones.
pub const SPEED: &str = "projrenderlimit";
const FULL_SPEED: i32 = 0;

/// The render settings a render may change, all noted and put back (`GetSetProjectInfo`).
pub const NUMBERS: [&str; 23] = [
    "RENDER_SETTINGS",
    "RENDER_BOUNDSFLAG",
    "RENDER_CHANNELS",
    "RENDER_SRATE",
    "RENDER_STARTPOS",
    "RENDER_ENDPOS",
    "RENDER_TAILFLAG",
    "RENDER_TAILMS",
    "RENDER_ADDTOPROJ",
    "RENDER_DITHER",
    "RENDER_NORMALIZE",
    "RENDER_NORMALIZE_TARGET",
    "RENDER_BRICKWALL",
    "RENDER_FADEIN",
    "RENDER_FADEOUT",
    "RENDER_FADEINSHAPE",
    "RENDER_FADEOUTSHAPE",
    "RENDER_FADELPF",
    "RENDER_PADSTART",
    "RENDER_PADEND",
    "RENDER_TRIMSTART",
    "RENDER_TRIMEND",
    "RENDER_DELAY",
];
/// And the text ones (`GetSetProjectInfo_String`).
pub const TEXTS: [&str; 5] = ["RENDER_FILE", "RENDER_PATTERN", "RENDER_FORMAT", "RENDER_FORMAT2", "RENDER_EXTRAFILEDIR"];

const MASTER_MIX: i32 = 0;
const STEMS_AND_MASTER: i32 = 1;
const STEMS_ONLY: i32 = 3;
const PRE_FADER: i32 = 8192;
/// "Delay render start to allow FX to initialize": the producer's choice stays (with its `RENDER_DELAY`).
pub(super) const DELAY_START: i32 = 16 << 16;
/// `RENDER_NORMALIZE`: disable all render postprocessing (normalize, limit, fades, trim, pad).
pub(super) const NO_POSTPROCESSING: f64 = (4 << 16) as f64;

/// What a source's file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    PreFader,
    PostFader,
    Master,
    BeforeChain,
    Items,
}

impl Stage {
    pub fn word(self) -> &'static str {
        match self {
            Stage::PreFader => "pre-fader",
            Stage::PostFader => "post-fader",
            Stage::Master => "master",
            Stage::BeforeChain => "before-chain",
            Stage::Items => "items",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Source {
    pub id: String,
    pub stage: Stage,
    /// A track's GUID (none for the master).
    pub guid: Option<String>,
}

/// One run of REAPER's render: the tracks it renders as stems, the master if it renders it, and how.
#[derive(Debug, Clone)]
pub struct Pass {
    settings: i32,
    /// Sources (by index) rendered as stems.
    stems: Vec<usize>,
    /// The source rendered as the master, if any.
    master: Option<usize>,
    /// The master's FX are switched off for this pass.
    before_chain: bool,
}

/// A render as `render_start` planned it, run on the next tick.
#[derive(Debug, Clone)]
pub struct Plan {
    pub job: String,
    /// When it was planned (it waits for REAPER to be free for at most `queued_for()`).
    pub queued: Instant,
    pub proj: Project,
    pub epoch: String,
    /// Where the files start and end, in the project's seconds (pre-roll included).
    pub start: f64,
    pub end: f64,
    /// Where the part asked for begins in each file (seconds).
    pub part: f64,
    pub rate: Option<u32>,
    pub sources: Vec<Source>,
    pub passes: Vec<Pass>,
    pub folder: PathBuf,
}

/// A job id or a source id as the host gives them: it names files, so only letters, digits, `-` and `_`.
pub fn id_ok(id: &str, max: usize) -> bool {
    !id.is_empty() && id.len() <= max && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn job_id(params: &Value) -> Result<String, Fault> {
    let id = params.get("jobId").and_then(Value::as_str).unwrap_or_default();
    if !id_ok(id, 40) {
        return Err(bad("jobId: the host's job id (letters and digits)"));
    }
    Ok(id.to_string())
}

fn number(value: &Value, name: &str) -> Result<Option<f64>, Fault> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|n| n.is_finite()).map(Some).ok_or_else(|| bad(format!("{name} must be a number"))),
    }
}

/// The track a source names, by its GUID: refused as gone when it isn't in the project.
pub fn track_of(reaper: &Reaper, reads: &Reads, proj: Project, reference: &str) -> Result<(Track, String), Fault> {
    let guid = match Ref::parse(reference).map_err(bad)? {
        Ref::Track(TrackId::Guid(guid)) => guid,
        Ref::Track(TrackId::Master) => return Err(bad("the master is the source what: \"master\", not a track")),
        other => return Err(bad(format!("ref must be a track ref, not r:{}", other.kind()))),
    };
    let track = reads
        .track_by_id(reaper, proj, &TrackId::Guid(guid.clone()))
        .map_err(|_| gone("that track isn't in the project any more (deleted, or another tab): read again"))?;
    Ok((track, guid))
}

/// The project's own sample rate: the one set in its settings, else the device's, else 44.1 kHz (REAPER's).
pub fn project_rate(reaper: &Reaper, proj: Project) -> u32 {
    if reaper.project_number(proj, "PROJECT_SRATE_USE").unwrap_or(0.0) != 0.0 {
        let rate = reaper.project_number(proj, "PROJECT_SRATE").unwrap_or(0.0);
        if rate >= 8000.0 {
            return rate as u32;
        }
    }
    if reaper.audio_running() {
        if let Some(rate) = reaper.audio_device("SRATE").and_then(|rate| rate.trim().parse::<f64>().ok()).filter(|r| *r >= 8000.0) {
            return rate as u32;
        }
    }
    44100
}

/// Checks a `render_start` and plans it: what each source is, how many renders it takes, which files they make.
pub fn plan(reaper: &Reaper, reads: &Reads, params: &Value, folder: &Path) -> Result<Plan, Fault> {
    let job = job_id(params)?;
    let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
    let proj = info.ptr;
    let given = params.get("sources").and_then(Value::as_array).ok_or_else(|| bad("sources: a list of what to render"))?;
    if given.is_empty() || given.len() > MAX_SOURCES {
        return Err(bad(format!("sources: 1 to {MAX_SOURCES} of them")));
    }
    let any_track = given.iter().any(|source| source.get("what").and_then(Value::as_str) == Some("track"));
    let epoch = match params.get("epoch").and_then(Value::as_str) {
        Some(epoch) => epoch.to_string(),
        None if any_track => return Err(bad("epoch: the epoch the refs were read in")),
        None => reads.epoch(),
    };
    if epoch != reads.epoch() && !reads.epoch_is_of(&epoch, proj) {
        return Err(stale(format!(
            "those refs are from an earlier epoch ({epoch}; now {}): the project in the tab changed, read again",
            reads.epoch()
        )));
    }
    let mut sources: Vec<Source> = Vec::new();
    for given in given {
        let id = given.get("id").and_then(Value::as_str).unwrap_or_default();
        if !id_ok(id, 32) {
            return Err(bad("each source's id: 1 to 32 letters, digits, - or _"));
        }
        if sources.iter().any(|source| source.id == id) {
            return Err(bad(format!("two sources are called {id:?}")));
        }
        let source = match given.get("what").and_then(Value::as_str) {
            Some("master") => {
                if given.get("ref").is_some_and(|r| !r.is_null()) || given.get("stage").is_some_and(|s| !s.is_null()) {
                    return Err(bad("the master takes no ref and no stage (beforeChain: true renders it before its FX)"));
                }
                let before = match given.get("beforeChain") {
                    None | Some(Value::Null) | Some(Value::Bool(false)) => false,
                    Some(Value::Bool(true)) => true,
                    Some(_) => return Err(bad("beforeChain must be true or false")),
                };
                Source { id: id.into(), stage: if before { Stage::BeforeChain } else { Stage::Master }, guid: None }
            }
            Some("track") => {
                let reference = given.get("ref").and_then(Value::as_str).ok_or_else(|| bad("a track source needs its ref"))?;
                let (_, guid) = track_of(reaper, reads, proj, reference)?;
                let stage = match given.get("stage").and_then(Value::as_str) {
                    None | Some("pre-fader") => Stage::PreFader,
                    Some("post-fader") => Stage::PostFader,
                    Some("items") => Stage::Items,
                    Some(other) => return Err(bad(format!("stage {other:?}: pre-fader, post-fader or items"))),
                };
                if given.get("beforeChain").is_some_and(|b| !b.is_null()) {
                    return Err(bad("beforeChain is the master's"));
                }
                Source { id: id.into(), stage, guid: Some(guid) }
            }
            _ => return Err(bad("each source's what: \"master\" or \"track\"")),
        };
        if sources.iter().any(|other| other.guid.is_some() && other.guid == source.guid && other.stage == source.stage) {
            return Err(bad("a track is rendered once at each stage: two sources ask the same"));
        }
        if sources.iter().any(|other| other.guid.is_none() && other.stage == source.stage && source.guid.is_none()) {
            return Err(bad("the master is rendered once at each stage: two sources ask the same"));
        }
        sources.push(source);
    }
    if sources.iter().any(|source| source.stage == Stage::Items) && sources.len() > 1 {
        return Err(bad("a track's items (stage items) are read on their own: one source"));
    }
    let range = params.get("range").filter(|r| r.is_object()).ok_or_else(|| bad("range: {fromBeat, beats}"))?;
    let from = number(range, "fromBeat")?
        .filter(|from| *from >= 0.0)
        .ok_or_else(|| bad("range.fromBeat: 0 or more (quarter notes from the start)"))?;
    let beats = number(range, "beats")?.filter(|beats| *beats > 0.0).ok_or_else(|| bad("range.beats: more than 0"))?;
    let pre_roll = number(params, "preRollBeats")?.unwrap_or(0.0);
    if pre_roll < 0.0 {
        return Err(bad("preRollBeats: 0 or more"));
    }
    let rate = match params.get("sampleRate") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .map(|rate| rate as u32)
                .filter(|rate| RATES.contains(rate))
                .ok_or_else(|| bad("sampleRate: 44100, 48000, 88200 or 96000"))?,
        ),
    };
    let time = |qn: f64| reaper.time_at_quarter_notes(proj, qn).ok_or_else(|| failed("this REAPER can't place beats in time"));
    let start = time((from - pre_roll).max(0.0))?;
    let at = time(from)?;
    let end = time(from + beats)?;
    if end - start > MAX_SECONDS {
        return Err(bad(format!("that stretch is {:.0} s of music: Kumi renders at most {MAX_SECONDS:.0} s at a time", end - start)));
    }
    if end <= start {
        return Err(bad("that stretch has no length in time"));
    }
    let seconds = end - start;
    if let Some(free) = params.get("freeBytes").and_then(Value::as_u64) {
        let needed = (seconds * rate.unwrap_or(48_000) as f64 * 8.0 * sources.len() as f64 * 1.5) as u64;
        if needed > free {
            return Err(failed(format!(
                "Kumi's folder has {} MB free; this render needs about {} MB",
                free / 1_000_000,
                needed.div_ceil(1_000_000)
            )));
        }
    }
    let plan = Plan {
        job,
        queued: Instant::now(),
        proj,
        epoch,
        start,
        end,
        part: at - start,
        rate,
        passes: passes(&sources),
        sources,
        folder: folder.to_path_buf(),
    };
    for source in &plan.sources {
        let file = plan.file(source);
        if file.exists() {
            return Err(failed(format!("{} is in Kumi's folder already: Kumi never writes over a file", file.display())));
        }
    }
    Ok(plan)
}

/// The renders a set of sources takes: post-fader stems in one, pre-fader stems in another, and the master with the
/// post-fader stems when there are some (stems and the master render together), else on its own. Never with pre-fader
/// stems: REAPER doesn't process a muted track when it renders the master too, so its pre-fader stem would be silent
/// (measured). The master before its chain needs its FX off for the whole pass, which stems don't mind.
fn passes(sources: &[Source]) -> Vec<Pass> {
    let mut passes: Vec<Pass> = Vec::new();
    for stage in [Stage::PostFader, Stage::PreFader] {
        let stems: Vec<usize> = sources.iter().enumerate().filter(|(_, s)| s.stage == stage).map(|(n, _)| n).collect();
        if !stems.is_empty() {
            let settings = if stage == Stage::PreFader { STEMS_ONLY | PRE_FADER } else { STEMS_ONLY };
            passes.push(Pass { settings, stems, master: None, before_chain: false });
        }
    }
    for stage in [Stage::Master, Stage::BeforeChain] {
        let Some(at) = sources.iter().position(|s| s.stage == stage) else { continue };
        let before_chain = stage == Stage::BeforeChain;
        match passes.iter_mut().find(|pass| pass.master.is_none() && !pass.stems.is_empty() && pass.settings & PRE_FADER == 0) {
            Some(pass) => {
                pass.master = Some(at);
                pass.before_chain = before_chain;
                pass.settings = (pass.settings & PRE_FADER) | STEMS_AND_MASTER;
            }
            None => passes.push(Pass { settings: MASTER_MIX, stems: vec![], master: Some(at), before_chain }),
        }
    }
    passes
}

/// The master moved into a pre-fader pass where none of that pass's tracks is muted (`quiet`, by source): one pass less. Decided as the render begins, with the main thread held from there to its end, so no mute can
/// change in between.
fn merged(passes: &[Pass], quiet: impl Fn(usize) -> bool) -> Vec<Pass> {
    let mut out: Vec<Pass> = passes.to_vec();
    let mut at = 0;
    while at < out.len() {
        let alone = out[at].stems.is_empty() && out[at].master.is_some();
        let into = out
            .iter()
            .position(|p| p.master.is_none() && p.settings & PRE_FADER != 0 && !p.stems.is_empty() && !p.stems.iter().any(|&s| quiet(s)));
        match (alone, into) {
            (true, Some(into)) => {
                let master = out.remove(at);
                let into = if into > at { into - 1 } else { into };
                out[into].master = master.master;
                out[into].before_chain = master.before_chain;
                out[into].settings = PRE_FADER | STEMS_AND_MASTER;
            }
            _ => at += 1,
        }
    }
    out
}

/// The passes a set of sources takes, as (settings, stems, has the master, before the chain), with `quiet` sources.
#[cfg(test)]
pub fn passes_for_test(sources: &[Source], quiet: &[usize]) -> Vec<(i32, usize, bool, bool)> {
    merged(&passes(sources), |s| quiet.contains(&s))
        .into_iter()
        .map(|p| (p.settings, p.stems.len(), p.master.is_some(), p.before_chain))
        .collect()
}

impl Plan {
    /// A source's file: `<jobId>-<sourceId>.wav`.
    pub fn file(&self, source: &Source) -> PathBuf {
        self.folder.join(format!("{}-{}.wav", self.job, source.id))
    }

    /// Where pass `n` renders: its own names, which no source's file can have (they have a dot), renamed once all is well.
    fn pattern(&self, pass: &Pass, n: usize) -> String {
        if pass.stems.is_empty() {
            format!("{}.p{}", self.job, n + 1)
        } else {
            format!("{}.p{}-$tracknumber", self.job, n + 1)
        }
    }

    /// The file pass `n` writes for a stem (by its track's number) or the master (0).
    fn pass_file(&self, pass: &Pass, n: usize, number: i64) -> PathBuf {
        if pass.stems.is_empty() {
            self.folder.join(format!("{}.p{}.wav", self.job, n + 1))
        } else {
            self.folder.join(format!("{}.p{}-{number:02}.wav", self.job, n + 1))
        }
    }

    pub fn seconds(&self) -> f64 {
        self.end - self.start
    }

    pub fn items(&self) -> bool {
        self.sources.first().is_some_and(|source| source.stage == Stage::Items)
    }

    /// What `render_start` answers.
    pub fn answer(&self) -> Value {
        json!({
            "jobId": self.job,
            "state": "queued",
            "seconds": crate::rows::seconds(self.seconds()),
            "files": self.sources.iter().map(|s| json!({"id": s.id, "file": self.file(s).to_string_lossy()})).collect::<Vec<_>>(),
            "epoch": self.epoch,
        })
    }
}

// ----- putting things back -----

/// What a render changes, as it was before.
struct Saved {
    numbers: Vec<(&'static str, f64)>,
    texts: Vec<(&'static str, String)>,
    /// Muted folders unmuted for a pre-fader pass (REAPER renders a muted folder silent even before its fader).
    unmuted: Vec<Track>,
    /// Every track's `I_SELECTED`, the master's included.
    selection: Vec<(Track, f64)>,
    master_fx: Option<(Track, f64)>,
    speed: Option<i32>,
    dirty: Option<f64>,
}

/// Puts back what a render changed, however the render ends (a panic included).
pub(super) struct PutBack<'a, 'r> {
    reaper: &'a Reaper<'r>,
    proj: Project,
    saved: Saved,
    done: bool,
}

impl<'a, 'r> PutBack<'a, 'r> {
    pub(super) fn note(reaper: &'a Reaper<'r>, proj: Project) -> PutBack<'a, 'r> {
        let numbers = NUMBERS.iter().filter_map(|key| reaper.project_number(proj, key).map(|value| (*key, value))).collect();
        let texts = TEXTS.iter().filter_map(|key| reaper.project_text_long(proj, key).map(|value| (*key, value))).collect();
        let master = reaper.master_track(proj);
        let mut selection: Vec<(Track, f64)> = master.iter().map(|m| (*m, reaper.track_value(*m, "I_SELECTED"))).collect();
        for index in 0..reaper.track_count(proj) {
            if let Some(track) = reaper.track_at(proj, index) {
                selection.push((track, reaper.track_value(track, "I_SELECTED")));
            }
        }
        let saved = Saved {
            numbers,
            texts,
            unmuted: Vec::new(),
            selection,
            master_fx: master.map(|m| (m, reaper.track_value(m, "I_FXEN"))),
            speed: reaper.project_config_int(proj, SPEED),
            dirty: reaper.project_number(proj, "DIRTY"),
        };
        PutBack { reaper, proj, saved, done: false }
    }

    /// Puts everything back and reads it back: what didn't come back, in words.
    pub(super) fn now(&mut self) -> Vec<String> {
        self.done = true;
        let (reaper, proj) = (self.reaper, self.proj);
        let mut missed = Vec::new();
        for (key, value) in &self.saved.numbers {
            reaper.set_project_number(proj, key, *value);
            if reaper.project_number(proj, key).is_some_and(|now| now != *value) {
                missed.push(key.to_string());
            }
        }
        for (key, value) in &self.saved.texts {
            reaper.set_project_text(proj, key, value);
            if reaper.project_text_long(proj, key).is_some_and(|now| now != *value) {
                missed.push(key.to_string());
            }
        }
        for (track, selected) in &self.saved.selection {
            if reaper.track_value(*track, "I_SELECTED") != *selected {
                reaper.set_track_value(*track, "I_SELECTED", *selected);
            }
        }
        if self.saved.selection.iter().any(|(track, selected)| reaper.track_value(*track, "I_SELECTED") != *selected) {
            missed.push("the track selection".into());
        }
        for track in &self.saved.unmuted {
            if reaper.track_value(*track, "B_MUTE") == 0.0 {
                reaper.set_track_value(*track, "B_MUTE", 1.0);
            }
            if reaper.track_value(*track, "B_MUTE") == 0.0 {
                missed.push("a folder's mute".into());
            }
        }
        if let Some((master, on)) = self.saved.master_fx {
            if reaper.track_value(master, "I_FXEN") != on {
                reaper.set_track_value(master, "I_FXEN", on);
                if reaper.track_value(master, "I_FXEN") != on {
                    missed.push("the master's FX switch".into());
                }
            }
        }
        if let Some(speed) = self.saved.speed {
            if reaper.project_config_int(proj, SPEED) != Some(speed) {
                reaper.set_project_config_int(proj, SPEED, speed);
                if reaper.project_config_int(proj, SPEED) != Some(speed) {
                    missed.push("the render speed".into());
                }
            }
        }
        // Last: everything above may have marked the project changed.
        if let Some(dirty) = self.saved.dirty {
            if reaper.project_number(proj, "DIRTY") != Some(dirty) {
                reaper.set_project_number(proj, "DIRTY", dirty);
            }
        }
        missed
    }
}

impl Drop for PutBack<'_, '_> {
    fn drop(&mut self) {
        if !self.done {
            let missed = self.now();
            crate::log::line(&format!(
                "a render was cut short; its settings were put back{}",
                if missed.is_empty() { String::new() } else { format!(" except {}", missed.join(", ")) }
            ));
        }
    }
}

// ----- running it -----

/// How a render ended.
#[derive(Debug)]
pub enum Ended {
    Done { files: Value, notes: Vec<String> },
    Failed { code: String, error: String, notes: Vec<String> },
    Cancelled { notes: Vec<String> },
}

impl Ended {
    /// A note first among the job's.
    pub fn note_first(&mut self, note: String) {
        let (Ended::Done { notes, .. } | Ended::Failed { notes, .. } | Ended::Cancelled { notes }) = self;
        notes.insert(0, note);
    }
}

/// What the running render asks of the one who runs it: to run REAPER's render (saying meanwhile that it renders, and
/// keeping up what must go on inside it), and whether Kumi asked it to stop.
pub trait Hooks {
    fn render(&self, run: &mut dyn FnMut());
    fn watch(&self, watch: super::board::Watch);
    fn cancelled(&self) -> bool;
    /// Whether the render window's Cancel was pressed while it ran.
    fn pressed(&self) -> bool;
}

/// Frames a file may lack and still be whole (REAPER's last block).
const SHORT_FRAMES: u64 = 4096;
/// How long a planned render waits for REAPER to be free (a dialog, a load, its own render) before it gives up.
const QUEUED_FOR: Duration = Duration::from_secs(30);

#[cfg(test)]
thread_local! {
    /// A test's own wait, to see a render give up without waiting half a minute.
    pub static TEST_QUEUED_FOR: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

pub fn queued_for() -> Duration {
    #[cfg(test)]
    if let Some(wait) = TEST_QUEUED_FOR.with(|w| w.get()) {
        return wait;
    }
    QUEUED_FOR
}

/// Frames a file of this many seconds has at this rate.
fn frames(seconds: f64, rate: u32) -> u64 {
    (seconds * rate as f64).round().max(0.0) as u64
}

/// Runs a planned render (passes of REAPER's own render), puts everything back, and names its files.
pub fn run(reaper: &Reaper, reads: &Reads, plan: &Plan, hooks: &dyn Hooks) -> Ended {
    let mut notes = Vec::new();
    let fail = |code: &str, error: String, notes: Vec<String>| Ended::Failed { code: code.into(), error, notes };
    let Some(info) = reaper.current_project() else { return fail("stale", "REAPER has no project open".into(), notes) };
    if info.ptr != plan.proj || !(reads.epoch() == plan.epoch || reads.epoch_is_of(&plan.epoch, plan.proj)) {
        return fail("stale", "the project in the tab changed before the render began: read again".into(), notes);
    }
    let proj = plan.proj;
    // The tracks again, by their GUIDs: one may have gone since the plan.
    let mut tracks: Vec<Option<Track>> = Vec::new();
    for source in &plan.sources {
        tracks.push(match &source.guid {
            Some(guid) => match reads.track_by_id(reaper, proj, &TrackId::Guid(guid.clone())) {
                Ok(track) => Some(track),
                Err(_) => return fail("gone", format!("source {:?}'s track isn't in the project any more: read again", source.id), notes),
            },
            None => None,
        });
    }
    let state = reaper.play_state(proj);
    if state.recording {
        return fail("failed", "REAPER is recording: a render would stop the recording, so Kumi renders nothing".into(), notes);
    }
    if state.playing {
        notes.push("REAPER stopped playback to render".to_string());
    }
    // Working files a crashed render of this job left (their names are only ever Kumi's): gone before it renders again.
    sweep_leftovers(&plan.folder, &plan.job);
    let rate = plan.rate.unwrap_or_else(|| project_rate(reaper, proj));
    let file_bytes = frames(plan.seconds(), rate) * 8;
    let total = file_bytes * plan.sources.len() as u64;
    let mut put_back = PutBack::note(reaper, proj);
    let producer_settings = put_back.saved.numbers.iter().find(|(k, _)| *k == "RENDER_SETTINGS").map_or(0, |(_, v)| *v as i32);
    if put_back.saved.speed.is_none() {
        notes.push("This REAPER doesn't say its render speed: if the project renders in real time (1x or online), so did this".into());
    }
    let master = reaper.master_track(proj);
    // Every file any pass writes, the renamed ones too: deleted unless the render ends well.
    let mut made: Vec<PathBuf> = Vec::new();
    let mut before = 0u64;
    let mut outcome: Result<(), (String, String)> = Ok(());
    let mut cancelled = false;
    // A muted source track: REAPER skips it when it renders the master in the same pass (a track in a muted folder it
    // still renders: measured).
    let quiet = |at: usize| tracks[at].is_some_and(|t| reaper.track_value(t, "B_MUTE") != 0.0);
    let passes = merged(&plan.passes, quiet);
    for (n, pass) in passes.iter().enumerate() {
        if hooks.cancelled() {
            cancelled = true;
            break;
        }
        // The pass's tracks selected, and nothing else (the master neither: stems + master renders it anyway).
        let wanted: Vec<Track> = pass.stems.iter().filter_map(|&s| tracks[s]).collect();
        for (track, _) in &put_back.saved.selection {
            let on = if wanted.contains(track) { 1.0 } else { 0.0 };
            if reaper.track_value(*track, "I_SELECTED") != on {
                reaper.set_track_value(*track, "I_SELECTED", on);
            }
        }
        let set = |key: &str, value: f64| reaper.set_project_number(proj, key, value);
        set("RENDER_SETTINGS", (pass.settings | (producer_settings & DELAY_START)) as f64);
        set("RENDER_BOUNDSFLAG", 0.0);
        set("RENDER_STARTPOS", plan.start);
        set("RENDER_ENDPOS", plan.end);
        set("RENDER_TAILFLAG", 0.0);
        set("RENDER_CHANNELS", 2.0);
        set("RENDER_SRATE", rate as f64);
        set("RENDER_ADDTOPROJ", 0.0);
        set("RENDER_DITHER", 0.0);
        set("RENDER_NORMALIZE", NO_POSTPROCESSING);
        let folder = plan.folder.to_string_lossy().into_owned();
        reaper.set_project_text(proj, "RENDER_FILE", &folder);
        reaper.set_project_text(proj, "RENDER_PATTERN", &plan.pattern(pass, n));
        reaper.set_project_text(proj, "RENDER_FORMAT", FLOAT_WAV);
        reaper.set_project_text(proj, "RENDER_FORMAT2", "");
        reaper.set_project_text(proj, "RENDER_EXTRAFILEDIR", &folder);
        if let Some(master) = master {
            let on = if pass.before_chain { 0.0 } else { put_back.saved.master_fx.map_or(1.0, |(_, on)| on) };
            if reaper.track_value(master, "I_FXEN") != on {
                reaper.set_track_value(master, "I_FXEN", on);
            }
        }
        if put_back.saved.speed.is_some_and(|speed| speed != FULL_SPEED) {
            reaper.set_project_config_int(proj, SPEED, FULL_SPEED);
        }
        // A muted folder renders silent even before its fader: unmuted for the pass, muted again after it.
        for track in put_back.saved.unmuted.drain(..).collect::<Vec<_>>() {
            reaper.set_track_value(track, "B_MUTE", 1.0);
        }
        if pass.settings & PRE_FADER != 0 {
            for track in
                wanted.iter().filter(|t| reaper.track_value(**t, "I_FOLDERDEPTH") > 0.0 && reaper.track_value(**t, "B_MUTE") != 0.0)
            {
                reaper.set_track_value(*track, "B_MUTE", 0.0);
                put_back.saved.unmuted.push(*track);
                let name = crate::changes::quoted(&crate::changes::track_name(reaper, *track));
                notes.push(format!("{name} is muted: Kumi unmuted it for the render (REAPER renders a muted folder silent even before its fader) and muted it again"));
            }
        }
        // What REAPER would write: exactly this pass's files, in Kumi's folder, none of them there yet.
        let mut expected: Vec<(usize, PathBuf)> = Vec::new();
        if let Some(at) = pass.master {
            expected.push((at, plan.pass_file(pass, n, 0)));
        }
        for &at in &pass.stems {
            let number = tracks[at].map_or(0, |track| reaper.track_value(track, "IP_TRACKNUMBER") as i64);
            expected.push((at, plan.pass_file(pass, n, number)));
        }
        let targets: Vec<PathBuf> = reaper
            .project_text_long(proj, "RENDER_TARGETS")
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(PathBuf::from)
            .collect();
        let mut want: Vec<&PathBuf> = expected.iter().map(|(_, file)| file).collect();
        let mut have: Vec<&PathBuf> = targets.iter().collect();
        want.sort();
        have.sort();
        if want != have {
            crate::log::line(&format!("render targets differ: REAPER {targets:?}, Kumi {want:?}"));
            outcome = Err((
                "failed".into(),
                format!(
                    "REAPER would write other files than Kumi planned ({} instead of {}): nothing was rendered",
                    have.len(),
                    want.len()
                ),
            ));
            break;
        }
        if let Some((_, there)) = expected.iter().find(|(_, file)| file.exists()) {
            outcome = Err(("failed".into(), format!("{} is in Kumi's folder already: Kumi never writes over a file", there.display())));
            break;
        }
        made.extend(expected.iter().map(|(_, file)| file.clone()));
        hooks.watch(super::board::Watch { files: expected.iter().map(|(_, file)| (file.clone(), file_bytes)).collect(), before, total });
        before += file_bytes * expected.len() as u64;
        let started = Instant::now();
        hooks.render(&mut || {
            reaper.run_action(RENDER_WITH_LAST);
        });
        crate::log::line(&format!("render pass {}/{} of {} took {:.3} s", n + 1, passes.len(), plan.job, started.elapsed().as_secs_f64()));
        if hooks.cancelled() {
            cancelled = true;
            break;
        }
        if let Some((_, missing)) = expected.iter().find(|(_, file)| std::fs::metadata(file).map_or(true, |meta| meta.len() <= 44)) {
            outcome = Err((
                "failed".into(),
                format!(
                    "REAPER's render didn't write {}: nothing was kept",
                    missing.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
                ),
            ));
            break;
        }
        // A render cut short (the producer pressed Cancel in REAPER's window and kept the incomplete files): not the part.
        let wanted_frames = frames(plan.seconds(), rate);
        if expected.iter().any(|(_, file)| wav::read_info(file).is_ok_and(|info| info.frames + SHORT_FRAMES < wanted_frames)) {
            outcome =
                Err(("failed".into(), "REAPER's render stopped before the end (it was cancelled in REAPER): nothing was kept".into()));
            break;
        }
        // Renamed to the sources' names once the pass is whole.
        for (at, file) in &expected {
            let target = plan.file(&plan.sources[*at]);
            if let Err(error) = std::fs::rename(file, &target) {
                outcome = Err(("failed".into(), format!("Kumi couldn't name its render file ({error})")));
                break;
            }
            made.push(target);
        }
        if outcome.is_err() {
            break;
        }
    }
    let missed = put_back.now();
    if !missed.is_empty() {
        notes.push(format!("Kumi couldn't put back {} as it was", missed.join(", ")));
    }
    let clean = |made: &[PathBuf]| {
        for file in made {
            let _ = std::fs::remove_file(file);
        }
        sweep_leftovers(&plan.folder, &plan.job);
    };
    if cancelled {
        clean(&made);
        if !hooks.pressed() && before > 0 {
            notes.push("REAPER finished the render before it could stop".into());
        }
        return Ended::Cancelled { notes };
    }
    if let Err((code, error)) = outcome {
        clean(&made);
        return fail(&code, error, notes);
    }
    sweep_leftovers(&plan.folder, &plan.job);
    let mut files = Vec::new();
    for source in &plan.sources {
        let file = plan.file(source);
        owner_only(&file);
        let info = wav::read_info(&file).ok();
        files.push(json!({
            "id": source.id,
            "file": file.to_string_lossy(),
            "start": crate::rows::seconds(plan.part),
            "seconds": crate::rows::seconds(info.map_or(plan.seconds(), |i| i.seconds())),
            "channels": info.map_or(2, |i| i.channels),
            "sampleRate": info.map_or(rate, |i| i.rate),
            "stage": source.stage.word(),
            "throughMaster": false,
        }));
    }
    Ended::Done { files: Value::Array(files), notes }
}

/// Files a render left in Kumi's folder under the job's working names (a pass cut short, a copy of the project some
/// preference has REAPER write beside a render): never a file of anyone else's.
fn sweep_leftovers(folder: &Path, job: &str) {
    let prefix = format!("{job}.");
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A file Kumi made, readable by its owner only.
pub fn owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

// ----- a track's own items (stage items) -----

/// How long one tick reads a track's items for.
const SLICE: Duration = Duration::from_millis(50);
/// Frames read at a time.
const BLOCK: usize = 4096;

/// A track's audio before its FX, read with an accessor a slice a tick into a WAV in Kumi's folder.
pub struct Items {
    pub job: String,
    id: String,
    proj: Project,
    epoch: String,
    guid: String,
    track: Track,
    accessor: Accessor,
    writer: Option<wav::Writer>,
    file: PathBuf,
    start: f64,
    part: f64,
    rate: u32,
    total: u64,
    done: u64,
    heard: bool,
}

pub enum Step {
    Going(f64),
    Ended(Ended),
}

impl Items {
    /// Begins reading: the accessor made, the file opened.
    pub fn begin(reaper: &Reaper, reads: &Reads, plan: &Plan) -> Result<Items, (String, String)> {
        let source = &plan.sources[0];
        let guid = source.guid.clone().unwrap_or_default();
        let track = reads
            .track_by_id(reaper, plan.proj, &TrackId::Guid(guid.clone()))
            .map_err(|_| ("gone".to_string(), "that track isn't in the project any more: read again".to_string()))?;
        let rate = plan.rate.unwrap_or_else(|| project_rate(reaper, plan.proj));
        let accessor = reaper
            .track_accessor(track)
            .ok_or_else(|| ("failed".to_string(), "REAPER gave no way to read that track's audio".to_string()))?;
        let file = plan.file(source);
        let writer = match wav::Writer::create(&file, 2, rate) {
            Ok(writer) => writer,
            Err(error) => {
                reaper.destroy_accessor(accessor);
                return Err(("failed".into(), format!("Kumi couldn't write {} ({error})", file.display())));
            }
        };
        Ok(Items {
            job: plan.job.clone(),
            id: source.id.clone(),
            proj: plan.proj,
            epoch: plan.epoch.clone(),
            guid,
            track,
            accessor,
            writer: Some(writer),
            file,
            start: plan.start,
            part: plan.part,
            rate,
            total: frames(plan.seconds(), rate),
            done: 0,
            heard: false,
        })
    }

    /// Reads for a slice; the end once all is read, or why it stopped.
    pub fn step(&mut self, reaper: &Reaper, reads: &Reads, cancelled: bool) -> Step {
        if cancelled {
            return Step::Ended(self.stop(reaper, None));
        }
        let still = reaper.current_project().is_some_and(|info| info.ptr == self.proj)
            && (reads.epoch() == self.epoch || reads.epoch_is_of(&self.epoch, self.proj));
        if !still {
            return Step::Ended(
                self.stop(reaper, Some(("stale", "the project in the tab changed while Kumi read the track: read again".into()))),
            );
        }
        if !reaper.track_valid(self.proj, self.track) || reaper.track_guid(self.track) != self.guid {
            return Step::Ended(self.stop(reaper, Some(("gone", "the track went while Kumi read it".into()))));
        }
        if reaper.accessor_changed(self.accessor) {
            return Step::Ended(self.stop(reaper, Some(("failed", "the track's items changed while Kumi read them: ask again".into()))));
        }
        let started = Instant::now();
        let mut buf = vec![0f64; BLOCK * 2];
        while self.done < self.total && started.elapsed() < SLICE {
            let frames = (self.total - self.done).min(BLOCK as u64) as usize;
            let at = self.start + self.done as f64 / self.rate as f64;
            let block = &mut buf[..frames * 2];
            match reaper.accessor_samples(self.accessor, self.rate as i32, 2, at, block) {
                -1 => return Step::Ended(self.stop(reaper, Some(("failed", "REAPER couldn't read the track's audio".into())))),
                0 => block.iter_mut().for_each(|s| *s = 0.0),
                _ => self.heard = true,
            }
            let samples: Vec<f32> = block.iter().map(|s| *s as f32).collect();
            if let Some(writer) = self.writer.as_mut() {
                if let Err(error) = writer.write(&samples) {
                    return Step::Ended(self.stop(reaper, Some(("failed", format!("Kumi couldn't write the track's audio ({error})")))));
                }
            }
            self.done += frames as u64;
        }
        if self.done < self.total {
            return Step::Going(self.done as f64 / self.total.max(1) as f64);
        }
        reaper.destroy_accessor(self.accessor);
        let writer = self.writer.take();
        let finished = writer.map(|writer| writer.finish());
        if let Some(Err(error)) = finished {
            let _ = std::fs::remove_file(&self.file);
            return Step::Ended(Ended::Failed {
                code: "failed".into(),
                error: format!("Kumi couldn't finish the file ({error})"),
                notes: vec![],
            });
        }
        let mut notes = vec![];
        if !self.heard {
            notes.push("The track has no audio of its own in that stretch (its items are MIDI, or there are none): its sound comes from its FX, so listen to the track instead".into());
        }
        Step::Ended(Ended::Done {
            files: json!([{
                "id": self.id,
                "file": self.file.to_string_lossy(),
                "start": crate::rows::seconds(self.part),
                "seconds": crate::rows::seconds(self.total as f64 / self.rate as f64),
                "channels": 2,
                "sampleRate": self.rate,
                "stage": "items",
                "throughMaster": false,
            }]),
            notes,
        })
    }

    /// Stops: the accessor freed, the file deleted; cancelled, or failed with why.
    pub fn stop(&mut self, reaper: &Reaper, why: Option<(&str, String)>) -> Ended {
        reaper.destroy_accessor(self.accessor);
        self.writer = None;
        let _ = std::fs::remove_file(&self.file);
        match why {
            None => Ended::Cancelled { notes: vec![] },
            Some((code, error)) => Ended::Failed { code: code.into(), error, notes: vec![] },
        }
    }
}
