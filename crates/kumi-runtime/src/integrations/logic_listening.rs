//! (M5) Logic Pro as the listening tools meet it (`Renderer`), over Kumi's host and the bridge beside Logic: a part is
//! heard by playing it while the bridge's helper taps what Logic outputs (`hear`: after the Stereo Out, cut to the
//! part, Logic put back as it was), a track alone by soloing it for its pass, several candidates one pass after the
//! other (`audition`), and a track's audio is bounced offline (`render`). Every capture is a file the bridge writes into
//! a folder of Kumi's (under the system's temporary folder, taken away when the integration closes); nothing plays
//! "before the Stereo Out", so a run never hears the mix as it comes into it (`before_main` is never captured).
//!
//! The gain stage a judged round turns is a plug-in, found by name since Logic's plug-in rows carry no class: on the
//! run's track, or the Stereo Out without one, the last Adaptive Limiter or Limiter that's on (its Gain; it holds the
//! peaks), else a Gain that is the strip's last plug-in on (its Gain), else a Gain Kumi puts at the end; each set
//! through the plug-in window's Controls View (`fx_add`, `fx_param`), so it is a change in HISTORY that Kumi's undo
//! takes back. Kumi's changes, their undo and what changed outside them are the integration's HISTORY.
//!
//! A track heard alone is soloed through its own fader and the Stereo Out's plug-ins, so it is heard as it plays in the
//! mix: the run adds no fader to it (`fader_of` stays unread), and a neutral Gain on the Stereo Out is in both the mix
//! and the track already (`master_chain` adds no level). What reacts to the level it's given (a compressor, a limiter,
//! anything Kumi doesn't know as a neutral Gain, a meter or an analyzer) treats a track alone otherwise than within the
//! mix, so masking isn't read through it.
use super::own_listen::{ListenLink, OwnEars, OwnHearing, Refusal};
use crate::{
    core::{
        contracts::{AuditionCandidate, ChangeRecord, HearRequest, HeardTake, JsonObject, Render, MIX_CANDIDATE},
        daw::DawProfile,
        errors::RuntimeError,
    },
    listening::{
        checklist::MainState,
        engine::too_long,
        judging::Placed,
        renderer::{
            bars_of, Audible, AuditionOptions, AuditionPass, AuditionRender, AuditionSource, Captured, ClipNotes, ListenFlag, Pass,
            Renderer, Window,
        },
        tools::{ToolDef, AUDITION_TOOL, RENDER_TOOL},
    },
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{number::to_string, string::head},
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, SystemTime},
};

/// What a pass takes besides its part, as Kumi's host bounds it (`daw-mcp-server`'s `listen.rs`): a stretch of the
/// part may play up to `SLOWER` times longer (a slower tempo further on), each pass takes `PASS_MARGIN` more and a call
/// `CALL_MARGIN`, a bounce its dialogs (`RENDER_MARGIN`). Kumi waits `OURS` past the host's bound, so the host answers
/// first.
const SLOWER: f64 = 1.25;
const PASS_MARGIN: f64 = 20.0;
const CALL_MARGIN: f64 = 15.0;
const RENDER_MARGIN: f64 = 45.0;
const OURS: f64 = 15.0;
/// The tempo taken when none is known: a slow one, so a bound is long enough.
const SLOWEST_TEMPO: f64 = 30.0;
/// How long a read may take (the host gives one 15 s).
const READ_MS: u64 = 20_000;
/// The rows a page of a read asks for, and the most pages read.
const PAGE: u64 = 200;
const MOST_PAGES: usize = 50;
/// The listening folder's captures older than this go before a pass (the judge keeps its own in `judge/`).
const KEPT_FOR: Duration = Duration::from_secs(3600);
/// Plug-ins that change nothing heard: Logic's meters and analyzers, by the names its slots show.
const ANALYZERS: [&str; 7] = ["Level Meter", "MultiMeter", "Tuner", "Correlation Meter", "Loudness Meter", "BPM Counter", "Multimeter"];
/// The limiters a gain stage turns (their Gain), and the plug-in it adds.
const LIMITERS: [&str; 2] = ["Adaptive Limiter", "Limiter"];
const GAIN: &str = "Gain";
/// What the producer is told with every listen: the clicks that time a run of passes turn Logic's metronome on and
/// off, and its state is a setting Logic saves with the project (the bridge's contract), so Logic shows the project as
/// edited though nothing in it changed.
const EDITED: &str =
    "the project now shows as edited in Logic Pro: Kumi turned its metronome, a project setting, on and off to time the listen";
/// Kumi's changes that can't change what's heard (a name, an arm, a marker, the cycle).
const SILENT: [&str; 7] = ["track_rename", "track_arm", "region_rename", "marker_add", "marker_move", "marker_delete", "loop_range"];

/// Logic's listening, as its integration's hook builds it.
pub fn hook(link: Rc<dyn ListenLink>) -> OwnEars {
    let ears = Rc::new(LogicEars::new(link));
    OwnEars { renderer: ears.clone(), hearing: ears }
}

/// Logic as the listening tools meet it: Kumi's link to its host, whether Kumi is listening now, the folder captures go
/// into, and the meter (and tempo) as the project was last read.
pub struct LogicEars {
    link: Rc<dyn ListenLink>,
    flag: ListenFlag,
    folder: PathBuf,
    tempo: Cell<Option<f64>>,
    meter: Cell<f64>,
}

/// One of a pass's sources: the name it goes by now (the mix's, or its track's), and its track's long ref.
#[derive(Debug, Clone, PartialEq)]
struct Source {
    name: String,
    reference: Option<String>,
}

fn text(row: &Value, key: &str) -> String {
    row.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// dB as a Controls View's row shows it ("-3.0 dB", "+3.0 dB", "0.0 dB"); −∞ (or −96 and below) as minus infinity.
fn db_of(shown: &str) -> Option<f64> {
    let shown = shown.trim().replace('\u{2212}', "-");
    if shown.contains('∞') || shown.to_lowercase().contains("inf") {
        return Some(f64::NEG_INFINITY);
    }
    let number: String = shown
        .chars()
        .skip_while(|c| !(c.is_ascii_digit() || *c == '-' || *c == '+'))
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))
        .collect();
    let db = number.parse::<f64>().ok()?;
    Some(if db <= -96.0 { f64::NEG_INFINITY } else { db })
}

/// The plug-in a gain stage turns on a strip, from its plug-in rows (one bypassed turns nothing): the last Adaptive
/// Limiter or Limiter (its peaks then stay put), else a Gain that's the strip's last plug-in on. None: a Gain goes at
/// the end.
fn gain_plugin(rows: &[Value]) -> Option<(Value, bool)> {
    let on = |row: &&Value| row.get("bypassed") != Some(&Value::Bool(true));
    if let Some(row) = rows.iter().rev().filter(on).find(|row| LIMITERS.contains(&text(row, "name").as_str())) {
        return Some((row.clone(), true));
    }
    rows.iter().rev().find(on).filter(|row| text(row, "name") == GAIN).map(|row| (row.clone(), false))
}

/// Whether a Gain's Controls View only turns the level: its balance in the middle, both phases as they are, the sides
/// not swapped, stereo. A row Kumi doesn't know makes it more than a level.
fn neutral_gain(params: &[Value]) -> bool {
    params.iter().all(|row| {
        let shown = text(row, "text");
        match text(row, "name").as_str() {
            "Gain" => true,
            "Balance" => db_of(&shown).is_some_and(|balance| balance.abs() < 0.05),
            "Phase Left" | "Phase Right" | "Swap L/R" => shown.eq_ignore_ascii_case("Norm"),
            "Make Mono" => shown.eq_ignore_ascii_case("Stereo"),
            _ => false,
        }
    })
}

/// The takes an audition answered, in its candidates' order (under `candidates`).
fn takes_of(answer: &Value) -> Option<Vec<Value>> {
    answer.get("candidates").and_then(Value::as_array).cloned()
}

/// What a run of passes captured: each source's capture with its note (a soloed track the producer muted, a play that
/// stopped short), and what the run says besides (what Logic couldn't be put back to, a cut the clicks didn't time).
struct Captures {
    renders: Vec<Render>,
    notes: Vec<Option<String>>,
    said: Vec<String>,
}

impl Captures {
    /// What a call of the run says, each line once (an audition's first candidate and the rest are two calls).
    fn keep(&mut self, said: Vec<String>) {
        for line in said {
            if !self.said.contains(&line) {
                self.said.push(line);
            }
        }
    }

    /// Every note, the run's first.
    fn all_notes(&self) -> Vec<String> {
        self.said.iter().cloned().chain(self.notes.iter().flatten().cloned()).collect()
    }
}

/// What a run's answer says besides its takes, in `app`'s words: what Logic couldn't be put back to (`notPutBack`),
/// and a cut the metronome's clicks didn't time (`aligned.by` without them: the beats may be off by its `errorMs`).
fn run_said(app: &str, answer: &Value) -> Vec<String> {
    let mut said = vec![];
    let missed: Vec<&str> = answer.get("notPutBack").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    if !missed.is_empty() {
        said.push(format!("After listening, Kumi couldn't put {app} back as it was: {}. Check it in {app}.", missed.join("; ")));
    }
    let aligned = answer.get("aligned");
    let by = aligned.and_then(|aligned| aligned.get("by")).and_then(Value::as_str).unwrap_or_default();
    let error = aligned.and_then(|aligned| aligned.get("errorMs")).and_then(Value::as_f64).unwrap_or(0.);
    if !by.is_empty() && !by.contains("click") && error >= 5. {
        said.push(format!(
            "Kumi couldn't time {app}'s capture by its metronome's clicks, so the part's beats may be up to {} ms off in the file.",
            to_string(error.round())
        ));
    }
    said
}

/// A capture's file, where the part starts in it (the lead before it) and how long the part is in it.
fn render_of(answer: &Value) -> Option<Render> {
    let file = answer.get("file").and_then(Value::as_str).filter(|file| !file.is_empty())?.to_owned();
    let lead = answer.get("lead").and_then(Value::as_f64).filter(|lead| lead.is_finite()).unwrap_or(0.).max(0.);
    let seconds = answer.get("seconds").and_then(Value::as_f64).map(|seconds| (seconds - lead).max(0.));
    Some(Render { file, start: lead, seconds })
}

impl LogicEars {
    fn new(link: Rc<dyn ListenLink>) -> Self {
        let session = uuid::Uuid::new_v4().to_string().chars().take(8).collect::<String>();
        let folder = std::env::temp_dir().join("kumi-logic-listen").join(session);
        Self { link, flag: ListenFlag::default(), folder, tempo: Cell::new(None), meter: Cell::new(4.) }
    }

    fn words(&self) -> &'static DawProfile {
        self.link.profile()
    }

    /// The host's refusal in words: busy (a dialog, Logic playing), as Kumi says a busy DAW's, says to wait; gone says how
    /// to reach Logic.
    fn said(&self, refusal: &Refusal) -> String {
        let app = self.words().words.app;
        if !refusal.there {
            return self.words().vocab.cant_reach.into();
        }
        if refusal.busy {
            return format!("{app} is busy ({}): try again in a moment", self.link.in_short_refs(&refusal.why));
        }
        self.link.in_short_refs(&refusal.why)
    }

    /// A host tool's answer, or why not in words (Esc and Logic going away stay what they are).
    async fn ask(&self, tool: &str, args: Value, ms: u64, signal: &Signal) -> Result<Value, RuntimeError> {
        match self.link.call(tool, args, ms, signal.clone()).await? {
            Ok(answer) => Ok(answer),
            Err(refusal) => Err(RuntimeError::plain(self.said(&refusal))),
        }
    }

    /// The rows of a read, every page of them.
    async fn rows(&self, mut args: Value, signal: &Signal) -> Result<Vec<Value>, String> {
        args["limit"] = PAGE.into();
        let mut rows = Vec::new();
        for _ in 0..MOST_PAGES {
            let page = self.ask("read", args.clone(), READ_MS, signal).await.map_err(|error| head(&error.to_string(), 300))?;
            rows.extend(page.get("items").and_then(Value::as_array).cloned().unwrap_or_default());
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(cursor) => args["cursor"] = cursor.into(),
                None => break,
            }
        }
        Ok(rows)
    }

    async fn tracks(&self, signal: &Signal) -> Result<Vec<Value>, String> {
        self.rows(json!({"kind": "track"}), signal).await
    }

    /// The project as it is now: its tempo and meter kept, and the epoch its refs are in (the answer's: the link follows
    /// it).
    async fn refresh(&self, signal: &Signal) -> Result<String, RuntimeError> {
        let page = self.ask("read", json!({"kind": "project"}), READ_MS, signal).await?;
        if let Some(project) = page.pointer("/items/0") {
            self.keep(project.get("tempo").and_then(Value::as_f64), project.get("timeSignature").and_then(Value::as_str));
        }
        page.get("epoch")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.link.epoch())
            .ok_or_else(|| RuntimeError::plain(format!("{} didn't say which epoch its project is in", self.words().words.app)))
    }

    /// The tempo and the meter as a read said them.
    fn keep(&self, tempo: Option<f64>, signature: Option<&str>) {
        if let Some(tempo) = tempo.filter(|tempo| tempo.is_finite() && *tempo > 0.) {
            self.tempo.set(Some(tempo));
        }
        let beats = signature.and_then(|signature| signature.split('/').next()?.trim().parse::<f64>().ok());
        if let Some(beats) = beats.filter(|beats| *beats > 0.) {
            self.meter.set(beats);
        }
    }

    /// A track by what the model or a run calls it: this turn's ref (or a long one), else its name.
    fn find<'a>(&self, tracks: &'a [Value], named: &str) -> Option<&'a Value> {
        let long = self.link.lengthen(named);
        tracks
            .iter()
            .find(|row| text(row, "ref") == long)
            .or_else(|| tracks.iter().find(|row| text(row, "name") == named))
            .or_else(|| tracks.iter().find(|row| text(row, "name").eq_ignore_ascii_case(named)))
    }

    /// The Stereo Out, in the epoch now: the Mixer's first output strip, which no read lists.
    async fn stereo_out(&self, signal: &Signal) -> Result<String, String> {
        let epoch = self.refresh(signal).await.map_err(|error| head(&error.to_string(), 200))?;
        Ok(format!("{epoch}:output:1"))
    }

    /// The strip a gain stage is on: the run's track (its ref and its name in quotes), or the Stereo Out without one.
    async fn strip_of(&self, track: Option<&str>, signal: &Signal) -> Result<(String, String), String> {
        let Some(named) = track else { return Ok((self.stereo_out(signal).await?, "the Stereo Out".into())) };
        let tracks = self.tracks(signal).await?;
        let row = self.find(&tracks, named).ok_or_else(|| format!("{named} isn't a track in the project now"))?;
        Ok((text(row, "ref"), quoted(&text(row, "name"))))
    }

    async fn plugins_of(&self, strip: &str, signal: &Signal) -> Result<Vec<Value>, String> {
        self.rows(json!({"kind": "plugin", "parent": strip}), signal).await
    }

    async fn params_of(&self, plugin: &str, signal: &Signal) -> Result<Vec<Value>, String> {
        self.rows(json!({"kind": "param", "parent": plugin}), signal).await
    }

    /// The pass's sources, in order: the mix, or each candidate's track by its ref or name now. A clip isn't Logic's to
    /// play: a part is played from the Tracks area. Two sources called alike keep apart by their place.
    async fn sources(&self, candidates: &[AuditionCandidate], signal: &Signal) -> Result<Vec<Source>, String> {
        if let Some(clip) = candidates.iter().find_map(|candidate| candidate.clip.as_deref()) {
            return Err(format!(
                "{} plays the Tracks area to hear a part: give from_beat (and beats), not a clip ({clip}).",
                self.words().words.app
            ));
        }
        let mix = |candidate: &AuditionCandidate| candidate.mix == Some(true) || candidate.track == MIX_CANDIDATE;
        let tracks = if candidates.iter().all(mix) { vec![] } else { self.tracks(signal).await? };
        let mut sources: Vec<Source> = Vec::new();
        for candidate in candidates {
            let source = if mix(candidate) {
                Source { name: MIX_CANDIDATE.into(), reference: None }
            } else {
                let row = self
                    .find(&tracks, &candidate.track)
                    .ok_or_else(|| format!("{} isn't a track in the project now: read the tracks again.", candidate.track))?;
                Source { name: text(row, "name"), reference: Some(text(row, "ref")) }
            };
            let (mut name, mut n) = (source.name.clone(), 2);
            while sources.iter().any(|other| other.name == name) {
                name = format!("{} ({n})", source.name);
                n += 1;
            }
            sources.push(Source { name, ..source });
        }
        Ok(sources)
    }

    /// How long a call of `passes` passes over `beats` may take: the host's bound, and a little more.
    fn pass_ms(&self, beats: f64, passes: usize) -> u64 {
        let part = beats * 60. / self.tempo().unwrap_or(SLOWEST_TEMPO).max(1.);
        (((part.min(3600.) * SLOWER + PASS_MARGIN) * passes.max(1) as f64 + CALL_MARGIN + OURS) * 1000.) as u64
    }

    fn render_ms(&self, beats: f64) -> u64 {
        let part = beats * 60. / self.tempo().unwrap_or(SLOWEST_TEMPO).max(1.);
        ((part.min(3600.) * SLOWER + RENDER_MARGIN + OURS) * 1000.) as u64
    }

    /// The folder captures go into, made when it isn't there; what an hour ago left in it goes.
    async fn ready_folder(&self) -> Result<String, RuntimeError> {
        tokio::fs::create_dir_all(&self.folder)
            .await
            .map_err(|error| RuntimeError::plain(format!("Kumi couldn't make its listening folder: {error}")))?;
        if let Ok(mut entries) = tokio::fs::read_dir(&self.folder).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let old = entry.metadata().await.ok().filter(|meta| meta.is_file()).and_then(|meta| meta.modified().ok());
                if old.is_some_and(|at| SystemTime::now().duration_since(at).is_ok_and(|age| age > KEPT_FOR)) {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
        Ok(self.folder.to_string_lossy().into_owned())
    }

    /// Each source captured over `window`, in order: Logic plays for the producer where `audible` says, silent
    /// otherwise. Consecutive sources heard alike go in one call (`hear` for one, an `audition` for more).
    async fn capture(&self, sources: &[Source], window: Window, audible: &[bool], signal: &Signal) -> Result<Captures, RuntimeError> {
        let epoch = self.refresh(signal).await?;
        let folder = self.ready_folder().await?;
        let app = self.words().words.app;
        let mut captures = Captures { renders: vec![], notes: vec![], said: vec![] };
        let mut at = 0;
        while at < sources.len() {
            let heard = audible.get(at).copied().unwrap_or(true);
            let end = (at + 1..sources.len()).find(|next| audible.get(*next).copied().unwrap_or(true) != heard).unwrap_or(sources.len());
            let group = &sources[at..end];
            let answers = if let [one] = group {
                let mut args = json!({"fromBeats": window.from, "beats": window.beats, "folder": folder, "mute": !heard, "epoch": epoch});
                if let Some(reference) = &one.reference {
                    args["solo"] = json!([reference]);
                }
                let answer = self.ask("hear", args, self.pass_ms(window.beats, 1), signal).await?;
                captures.keep(run_said(app, &answer));
                vec![answer]
            } else {
                let candidates: Vec<Value> = group
                    .iter()
                    .map(|source| match &source.reference {
                        Some(reference) => json!({"ref": reference}),
                        None => json!({"mix": true}),
                    })
                    .collect();
                let args = json!({"candidates": candidates, "fromBeats": window.from, "beats": window.beats, "folder": folder, "mute": !heard, "epoch": epoch});
                let answer = self.ask("audition", args, self.pass_ms(window.beats, group.len()), signal).await?;
                captures.keep(run_said(app, &answer));
                takes_of(&answer)
                    .filter(|takes| takes.len() == group.len())
                    .ok_or_else(|| RuntimeError::plain(format!("{app} answered the audition without a take for each candidate")))?
            };
            for answer in answers {
                let render = render_of(&answer).ok_or_else(|| RuntimeError::plain(format!("{app} answered without the capture's file")))?;
                captures.renders.push(render);
                captures.notes.push(answer.get("note").and_then(Value::as_str).filter(|note| !note.is_empty()).map(str::to_owned));
            }
            at = end;
        }
        Ok(captures)
    }

    /// Where the playhead is now, in beats (the focus's transport).
    async fn playhead(&self, signal: &Signal) -> Option<f64> {
        let focus = self.link.call("focus", json!({}), 5_000, signal.clone()).await.ok()?.ok()?;
        focus.pointer("/transport/beat").and_then(Value::as_f64).filter(|beat| beat.is_finite() && *beat >= 0.)
    }

    /// The regions of the project, every page.
    async fn regions(&self, signal: &Signal) -> Result<Vec<Value>, String> {
        self.rows(json!({"kind": "region"}), signal).await
    }

    /// What an audition round's first candidate is, and the others heard silently, as the producer is told.
    fn auditioning(&self, sources: &[Source], options: &AuditionOptions, beats: f64) -> String {
        let label = |source: &Source| if source.reference.is_none() { "the whole mix".to_string() } else { source.name.clone() };
        let first = sources.first().map(label).unwrap_or_else(|| "the candidates".into());
        let app = self.words().words.app;
        let more = match (sources.len().saturating_sub(1), options.audible) {
            (0, _) => String::new(),
            (others, Audible::Every) => format!(", then {others} more, each on its own"),
            (1, Audible::First) => format!("; Kumi hears the other one on its own, {app} silent"),
            (others, Audible::First) => format!("; Kumi hears the other {others} on their own, {app} silent"),
        };
        let round = beats * 60. / options.tempo.max(1.) + PASS_MARGIN / 4.;
        let length =
            if options.first { format!(" (about {} s a candidate)", to_string((round * 10.).round() / 10.)) } else { String::new() };
        format!("Round {}: playing {first}{more}{length}", options.round)
    }
}

#[async_trait(?Send)]
impl Renderer for LogicEars {
    fn profile(&self) -> &DawProfile {
        self.link.profile()
    }
    /// Groove reads a part's notes with the ids it moves them by, which Logic's reads don't give: said in Logic's words.
    async fn clip_notes(&self, _clip: &str, _signal: Signal) -> Result<ClipNotes, String> {
        Err(format!("Kumi can't read a region's notes for groove in {}.", self.words().words.app))
    }
    fn refusal(&self) -> Option<String> {
        (!self.link.connected()).then(|| self.words().vocab.cant_reach.to_string())
    }
    fn tempo(&self) -> Option<f64> {
        self.tempo.get().or_else(|| self.link.tempo())
    }
    fn beats_per_bar(&self) -> f64 {
        self.meter.get()
    }
    fn lifetime(&self) -> Signal {
        self.link.lifetime()
    }
    fn cleanup(&self) -> Signal {
        abort::any([self.link.lifetime(), abort::timeout(self.link.change_ms().saturating_mul(3))])
    }
    /// Says what Kumi does; a listen's end ("Listened", "Auditioned") says too that Logic now shows the project as edited.
    fn tell(&self, title: &str, playing: Option<bool>) {
        if title == "Listened" || title.starts_with("Auditioned") {
            return self.link.tell(&format!("{title} ({EDITED})"), playing);
        }
        self.link.tell(title, playing)
    }
    fn flag(&self) -> &ListenFlag {
        &self.flag
    }
    fn kept_folder(&self) -> PathBuf {
        self.folder.join("judge")
    }

    /// Where the last region ends.
    async fn song_end(&self, signal: Signal) -> Option<f64> {
        let regions = self.regions(&signal).await.ok()?;
        regions.iter().filter_map(|row| row.get("endBeats").and_then(Value::as_f64)).reduce(f64::max).filter(|end| *end > 0.)
    }
    async fn track_name(&self, named: &str, signal: Signal) -> Option<String> {
        let tracks = self.tracks(&signal).await.ok()?;
        self.find(&tracks, named).map(|row| text(row, "name"))
    }
    async fn track_ref(&self, named: &str, signal: Signal) -> Option<String> {
        let tracks = self.tracks(&signal).await.ok()?;
        self.find(&tracks, named).map(|row| text(row, "ref"))
    }
    /// A region's audio file, as its row says it; a name that is no region is a file's own.
    async fn clip_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let long = self.link.lengthen(named);
        if !long.contains(":region:") {
            return Ok(None);
        }
        let page = self.ask("read", json!({"ref": long}), READ_MS, &signal).await?;
        let row = page.pointer("/items/0").cloned().unwrap_or(Value::Null);
        match row.get("file").and_then(Value::as_str).filter(|file| !file.is_empty()) {
            Some(file) => Ok(Some(file.to_owned())),
            None if row.get("kind").and_then(Value::as_str) == Some("midi") => Err(RuntimeError::plain(format!(
                "{named} is a MIDI region, with no audio file: hear its track instead (listen with track, or render)."
            ))),
            None => Err(RuntimeError::plain(format!(
                "{} shows a region's audio file by name only, so Kumi can't hear {named} as a file: hear its track over the region's bars instead (listen with track and from_beat, or render), or give the file's path.",
                self.words().words.app
            ))),
        }
    }

    /// Each candidate heard in a pass of its own over `window` (a track soloed alone, or the whole mix), the producer
    /// hearing every pass. Nothing comes before the Stereo Out, so the mix is never heard as it comes into it.
    async fn hear_pass(&self, candidates: &[AuditionCandidate], window: Window, _before_main: bool, signal: Signal) -> Pass {
        let rendered = async {
            if let Some(why) = self.refusal() {
                return Err(RuntimeError::plain(why));
            }
            let sources = self.sources(candidates, &signal).await.map_err(RuntimeError::plain)?;
            let captures = self.capture(&sources, window, &vec![true; sources.len()], &signal).await?;
            let notes = captures.all_notes();
            let files: IndexMap<String, Render> = sources.iter().map(|source| source.name.clone()).zip(captures.renders).collect();
            Ok((sources, files, notes))
        }
        .await;
        let lead = rendered.as_ref().map_or(0., |(_, files, _)| files.values().map(|render| render.start).fold(0., f64::max));
        let mut notes = vec![];
        let rendered = rendered.map(|(sources, files, said)| {
            notes = said;
            Captured {
                files,
                sources: sources.iter().map(|source| (source.name.clone(), source.reference.is_none())).collect(),
                before_master: None,
            }
        });
        Pass { rendered, notes, tried: None, lead, audible: true }
    }

    /// The Stereo Out as masking needs it. A track heard alone passes it too: a neutral Gain there is in both (no level
    /// to add); what reacts to the level it's given treats a track alone otherwise than the mix, so it's listed, as is
    /// anything Kumi doesn't know; meters and analyzers change nothing.
    async fn master_chain(&self, signal: Signal) -> MainState {
        let read = async {
            let strip = self.stereo_out(&signal).await?;
            let mut unfair = vec![];
            for row in self.plugins_of(&strip, &signal).await? {
                let name = text(&row, "name");
                if row.get("bypassed") == Some(&Value::Bool(true)) || ANALYZERS.contains(&name.as_str()) {
                    continue;
                }
                if name == GAIN && neutral_gain(&self.params_of(&text(&row, "ref"), &signal).await?) {
                    continue;
                }
                unfair.push(name);
            }
            Ok::<_, String>(MainState { unfair, gain: 0. })
        };
        read.await.unwrap_or_else(|_| MainState { unfair: vec!["the Stereo Out's plug-ins, which Kumi couldn't read".into()], gain: 0. })
    }

    fn applied(&self) -> Vec<String> {
        self.link.applied()
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        self.link.applied_since(mark)
    }
    fn audible(&self, id: &str) -> Option<bool> {
        self.link.made_by(id).map(|tool| tool.is_none_or(|tool| !SILENT.contains(&tool.as_str())))
    }
    fn outside(&self) -> usize {
        self.link.outside()
    }
    fn outside_since(&self, mark: usize) -> Vec<&'static str> {
        self.link.outside_since(mark)
    }
    /// Kumi's undo of one change; taking back goes on after Esc, within the cleanup's own time.
    async fn undo(&self, id: &str) -> Result<(), String> {
        self.link.undo(id, self.cleanup()).await
    }

    /// Turns the level by `gain` dB at the gain stage (a Gain added at the end of the strip when it has none), each a
    /// change in HISTORY: what it turned, and whether a limiter holds the peaks after it.
    async fn turn_gain(&self, gain: f64, track: Option<&str>, signal: Signal) -> Result<(String, bool), String> {
        let (strip, called) = self.strip_of(track, &signal).await?;
        let (plugin, limited) = match gain_plugin(&self.plugins_of(&strip, &signal).await?) {
            Some(found) => found,
            None => {
                self.link
                    .change("fx_add", object(json!({"ref": strip, "name": GAIN})), signal.clone())
                    .await
                    .map_err(|why| head(&why, 200))?;
                let rows = self.plugins_of(&strip, &signal).await?;
                let added = rows.iter().rev().find(|row| text(row, "name") == GAIN).cloned();
                (added.ok_or_else(|| format!("the Gain Kumi put on {called} didn't appear"))?, false)
            }
        };
        let reference = text(&plugin, "ref");
        let params = self.params_of(&reference, &signal).await?;
        let now = params
            .iter()
            .find(|row| text(row, "name") == "Gain")
            .and_then(|row| db_of(&text(row, "text")))
            .ok_or_else(|| format!("Kumi couldn't read the Gain of {} on {called}", text(&plugin, "name")))?;
        let wanted = (if now.is_finite() { now + gain } else { gain } * 10.).round() / 10.;
        self.link
            .change("fx_param", object(json!({"ref": reference, "param": "Gain", "value": wanted})), signal)
            .await
            .map_err(|why| head(&why, 200))?;
        let label = if limited { format!("the {}'s Gain on {called}", text(&plugin, "name")) } else { format!("the Gain on {called}") };
        Ok((label, limited))
    }
    async fn limited(&self, track: Option<&str>, signal: Signal) -> bool {
        let Ok((strip, _)) = self.strip_of(track, &signal).await else { return false };
        self.plugins_of(&strip, &signal).await.ok().and_then(|rows| gain_plugin(&rows)).is_some_and(|(_, limited)| limited)
    }
    async fn chain(&self, track: Option<&str>) -> Result<Vec<Placed>, String> {
        let signal = self.cleanup();
        let (strip, _) = self.strip_of(track, &signal).await?;
        Ok(self
            .plugins_of(&strip, &signal)
            .await?
            .iter()
            .map(|row| Placed { identity: text(row, "ref"), name: text(row, "name"), reference: text(row, "ref") })
            .collect())
    }
    async fn delete_device(&self, reference: &str) -> bool {
        self.link.change("fx_delete", object(json!({"ref": reference})), self.cleanup()).await.is_ok()
    }

    /// Each candidate heard in a pass of its own: the producer hears the first (or every one, as `options` says) and
    /// Kumi hears the rest with Logic silent.
    async fn audition_pass(
        &self,
        candidates: &[AuditionCandidate],
        from_beat: Option<f64>,
        beats: Option<f64>,
        options: AuditionOptions,
        signal: Signal,
    ) -> AuditionPass {
        let mut opened = false;
        let mut notes = vec![];
        let rendered = async {
            if let Some(why) = self.render_refusal() {
                return Err(RuntimeError::plain(why));
            }
            let from = from_beat.ok_or_else(|| {
                RuntimeError::plain(format!("Say where the part is: from_beat (and beats) in the {}.", self.words().words.timeline))
            })?;
            let beats = beats.filter(|beats| *beats > 0.).unwrap_or(8.);
            let sources = self.sources(candidates, &signal).await.map_err(RuntimeError::plain)?;
            let audible: Vec<bool> = (0..sources.len()).map(|at| options.audible == Audible::Every || at == 0).collect();
            opened = true;
            self.tell(&self.auditioning(&sources, &options, beats), Some(true));
            let captures = self.capture(&sources, Window { from: from.max(0.), beats }, &audible, &signal).await?;
            notes = captures.all_notes();
            let files: IndexMap<String, Render> = sources.iter().map(|source| source.name.clone()).zip(captures.renders).collect();
            let sources = candidates
                .iter()
                .zip(&sources)
                .zip(&audible)
                .map(|((candidate, source), heard)| AuditionSource {
                    track: candidate.track.clone(),
                    name: source.name.clone(),
                    scene: None,
                    audible: *heard,
                })
                .collect();
            Ok(AuditionRender { sources, files, beats })
        }
        .await;
        AuditionPass { rendered, notes, opened }
    }
    /// A track's audio between two points, bounced offline with the track soloed: through its plug-ins and the Stereo
    /// Out's, as it plays in the mix.
    async fn render(&self, track: &str, from: f64, beats: f64, signal: Signal) -> Result<Result<JsonObject, String>, RuntimeError> {
        if let Some(why) = self.render_refusal() {
            return Ok(Err(why));
        }
        let signal = abort::any([signal, self.link.lifetime()]);
        let tracks = match self.tracks(&signal).await {
            Ok(tracks) => tracks,
            Err(why) => return Ok(Err(why)),
        };
        let Some(row) = self.find(&tracks, track) else {
            return Ok(Err(format!("{track} isn't a track in the project now: read the tracks again.")));
        };
        let (reference, name) = (text(row, "ref"), text(row, "name"));
        let ready = async { Ok::<_, RuntimeError>((self.refresh(&signal).await?, self.ready_folder().await?)) };
        let (epoch, folder) = match ready.await {
            Ok(ready) => ready,
            Err(error) => {
                signal.check()?;
                return Ok(Err(head(&error.to_string(), 300)));
            }
        };
        self.tell(&format!("Bouncing {} of {name}", bars_of(self.beats_per_bar(), Window { from, beats })), None);
        let args = json!({"ref": reference, "fromBeats": from, "beats": beats, "folder": folder, "epoch": epoch});
        let answer = match self.link.call("render", args, self.render_ms(beats), signal.clone()).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(refusal)) => return Ok(Err(self.said(&refusal))),
            Err(error) => {
                signal.check()?;
                return Ok(Err(head(&error.to_string(), 300)));
            }
        };
        // A bounce turns the metronome off for itself when it was on (a project setting: Logic shows the project edited).
        let bounced = format!("Bounced {} of {name}", bars_of(self.beats_per_bar(), Window { from, beats }));
        match answer.get("metronome").and_then(Value::as_str) {
            Some("off for the pass") => self.link.tell(&format!("{bounced} (the project now shows as edited in Logic Pro: Kumi turned its metronome, a project setting, off and on again for the bounce)"), None),
            _ => self.link.tell(&bounced, None),
        }
        let mut reply = JsonObject::new();
        for key in ["file", "seconds", "channels", "sampleRate"] {
            if let Some(value) = answer.get(key) {
                reply.insert(key.into(), value.clone());
            }
        }
        if !reply.contains_key("file") {
            return Ok(Err(format!("{} answered the bounce without its file", self.words().words.app)));
        }
        reply.insert(
            "note".into(),
            json!(format!(
                "{name} bounced offline, soloed: its regions through its plug-ins and the Stereo Out's, as it plays in the mix. Hear it with listen (file), against a reference with compare_to."
            )),
        );
        Ok(Ok(reply))
    }
    fn short_ref(&self, long: &str) -> String {
        self.link.short_ref(long)
    }
    fn emit(&self, record: ChangeRecord) {
        self.link.emit(record)
    }

    /// Each track's regions, by its name (the first 64 tracks): where each starts and ends, in beats.
    async fn clips_by_track(&self, signal: Signal) -> Vec<(String, Vec<(f64, f64)>)> {
        let (Ok(tracks), Ok(regions)) = (self.tracks(&signal).await, self.regions(&signal).await) else { return vec![] };
        tracks
            .iter()
            .take(64)
            .filter_map(|track| {
                let reference = text(track, "ref");
                let spans: Vec<(f64, f64)> = regions
                    .iter()
                    .filter(|region| text(region, "trackRef") == reference)
                    .filter_map(|region| Some((region.get("startBeats")?.as_f64()?, region.get("endBeats")?.as_f64()?)))
                    .filter(|(start, end)| end > start)
                    .collect();
                (!spans.is_empty()).then(|| (text(track, "name"), spans))
            })
            .collect()
    }
}

#[async_trait(?Send)]
impl OwnHearing for LogicEars {
    /// Tracks (each alone, soloed for its pass) or the mix, over a stretch of the Tracks area: from_beat and beats, or
    /// `seconds` (8 by default) from the playhead, or the whole song.
    async fn hear(&self, request: &HearRequest, original: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        if let Some(why) = self.refusal() {
            return Ok(Err(why));
        }
        let place = self.words().words.place;
        let Some(tempo) = self.tempo().filter(|tempo| *tempo > 0. && tempo.is_finite()) else {
            return Ok(Err(format!("Kumi doesn't know the {place}'s tempo yet; try again.")));
        };
        if self.flag.is_on() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let signal = abort::any([original, self.link.lifetime()]);
        let window = if request.whole == Some(true) {
            let Some(end) = self.song_end(signal.clone()).await else {
                return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into()));
            };
            Window { from: 0., beats: end }
        } else {
            let given = request.from_beat.filter(|from| from.is_finite());
            let from = match given {
                Some(from) => from.max(0.),
                None => self.playhead(&signal).await.unwrap_or(0.),
            };
            let seconds = request.seconds.filter(|seconds| *seconds > 0. && seconds.is_finite());
            let beats = request
                .beats
                .filter(|beats| *beats > 0. && beats.is_finite())
                .or_else(|| seconds.map(|seconds| seconds * tempo / 60.))
                .unwrap_or_else(|| if given.is_some() { 4. * self.beats_per_bar() } else { 8. * tempo / 60. });
            Window { from, beats }
        };
        if let Some(why) = too_long(window.beats, tempo, "listen in parts") {
            return Ok(Err(why));
        }
        let candidates: Vec<AuditionCandidate> = if request.mix == Some(true) || request.tracks.is_empty() {
            vec![AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label: None, clip: None }]
        } else {
            request.tracks.iter().map(|track| AuditionCandidate { track: track.clone(), mix: None, label: None, clip: None }).collect()
        };
        let listening = self.flag.begin();
        let heard = async {
            let sources = self.sources(&candidates, &signal).await.map_err(RuntimeError::plain)?;
            let names: Vec<String> =
                sources.iter().map(|source| if source.reference.is_none() { "the mix".into() } else { source.name.clone() }).collect();
            self.tell(&format!("Playing {} · listening to {}", bars_of(self.beats_per_bar(), window), names.join(", ")), Some(true));
            let captures = self.capture(&sources, window, &vec![true; sources.len()], &signal).await?;
            let said = captures.said.join(" ");
            Ok::<_, RuntimeError>(
                sources
                    .iter()
                    .zip(captures.renders)
                    .zip(captures.notes)
                    .map(|((source, render), note)| HeardTake {
                        label: if source.reference.is_none() { "The whole mix".into() } else { source.name.clone() },
                        file: render.file,
                        start: render.start,
                        seconds: Some(render.seconds.unwrap_or(window.beats * 60. / tempo)),
                        live: false,
                        // (its own note first: a track soloed that the producer muted, a play that stopped short)
                        note: match (note, said.is_empty()) {
                            (Some(note), true) => Some(note),
                            (Some(note), false) => Some(format!("{note}. {said}")),
                            (None, false) => Some(said.clone()),
                            (None, true) => None,
                        },
                    })
                    .collect::<Vec<_>>(),
            )
        }
        .await;
        drop(listening);
        self.tell("Listened", Some(false));
        match heard {
            Ok(takes) if takes.is_empty() => Ok(Err("Nothing came through to hear.".into())),
            Ok(takes) => Ok(Ok(takes)),
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
        }
    }
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.clip_file(named, signal).await
    }
    /// The meter and tempo a look read (the project's time signature, its tempo).
    fn seen(&self, snapshot: &Value) {
        let tempo = ["/transport/tempo", "/project/tempo"].iter().find_map(|at| snapshot.pointer(at).and_then(Value::as_f64));
        let signature =
            ["/project/timeSignature", "/transport/timeSignature"].iter().find_map(|at| snapshot.pointer(at).and_then(Value::as_str));
        self.keep(tempo, signature);
    }
    /// The audition and the render as Logic does them: a pass per candidate, each soloed, from the Tracks area (no
    /// clips); a render bounced offline with the track soloed, through its plug-ins and the Stereo Out's.
    fn describe(&self, tool: &str, shared: ToolDef) -> ToolDef {
        let app = self.words().words.app;
        match tool {
            AUDITION_TOOL => {
                let mut schema = shared.schema;
                if let Some(items) =
                    schema.get_mut("properties").and_then(|p| p.pointer_mut("/candidates/items/properties")).and_then(Value::as_object_mut)
                {
                    items.remove("clip");
                }
                if let Some(beats) = schema.get_mut("properties").and_then(|p| p.get_mut("beats")) {
                    beats["description"] = json!("How long the part is, in beats; 8 when left out");
                }
                ToolDef {
                    description: format!("Hear what you built and how close it is to a reference, in one call: Kumi plays the part in {app} and captures each candidate track soloed, after its plug-ins and the Stereo Out's, compares it with the reference, then puts {app} back as it was. The producer hears the first candidate, and Kumi hears the others with {app} silent, so put the one you lean toward first. Each candidate is a real-time pass of its own, one after the other, so a round lasts the part's length for each: try a few ideas at once (different instruments, plug-ins or settings on separate tracks). Say where the part is: from_beat and beats in the Tracks area. A candidate can be the whole mix ({{\"mix\": true}}, on its own): what the Stereo Out plays at from_beat, heard like a track; audition it against a reference to match a mix, changing EQ, compression and levels between rounds. Returns each candidate's closeness to the reference (0–100) with the biggest gaps in words, and what it heard. A silent render is reported, not compared. Without a reference it only listens."),
                    schema,
                }
            }
            RENDER_TOOL => {
                let mut schema = shared.schema;
                if let Some(track) = schema.get_mut("properties").and_then(|p| p.get_mut("track")) {
                    track["description"] = json!("A track (its reference from this turn)");
                }
                ToolDef {
                    description: format!("Bounce a track's audio between two points to a file, offline: Kumi solos the track and bounces the range with {app}'s Bounce (nothing plays, the metronome off), so its regions come through its plug-ins and the Stereo Out's, as they play in the mix; then the solos and the metronome are put back. Returns the file for listen, to hear it or compare it with a reference."),
                    schema,
                }
            }
            // (judge and groove say regions where Live's say clips, from Logic's profile)
            _ => shared,
        }
    }
    async fn close(&self) {
        let _ = tokio::fs::remove_dir_all(&self.folder).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listening::tools::{groove_tool, judge_tool};

    #[test]
    fn judge_and_groove_say_regions_where_live_says_clips() {
        // (the shared tools say each DAW's words from its profile: a region with its ref, its notes changed through
        // Logic's own changes)
        let shared = [judge_tool(&DawProfile::LOGIC), groove_tool(&DawProfile::LOGIC)];
        // What a tool says: its description and every text in its parameters (not their names).
        fn texts(value: &Value, into: &mut Vec<String>) {
            match value {
                Value::String(text) => into.push(text.clone()),
                Value::Array(items) => items.iter().for_each(|item| texts(item, into)),
                Value::Object(fields) => fields.values().for_each(|field| texts(field, into)),
                _ => {}
            }
        }
        let all = |def: &ToolDef| {
            let mut said = vec![def.description.clone()];
            def.schema.values().for_each(|value| texts(value, &mut said));
            said.join(" ")
        };
        for def in &shared {
            let said = all(def);
            // (groove's parameter keeps its name)
            let words = said.replace("Start a run with clip (", "");
            let clips: Vec<&str> =
                words.split(|c: char| !c.is_alphanumeric()).filter(|word| word.to_lowercase().starts_with("clip")).collect();
            assert!(clips.is_empty(), "{clips:?} in {said}");
            assert!(!said.contains("transform_midi") && !said.contains("change_notes"), "Live's tools: {said}");
        }
        let groove = groove_tool(&DawProfile::LOGIC);
        assert!(groove.description.contains("Start a run with clip (the part, a MIDI region) and reference (a MIDI region with"));
        assert_eq!(groove.schema["properties"]["clip"]["description"], "Starts a run: the part, a MIDI region's ref");
        assert_eq!(
            groove.schema["properties"]["audio"]["description"],
            "The reference's drum stem (an audio region's ref in the Tracks area): its hits' onsets refine the reference's timing"
        );
        let judge = judge_tool(&DawProfile::LOGIC);
        assert!(judge.schema["properties"]["goal"]["properties"]["reference"]["description"]
            .as_str()
            .is_some_and(|said| said.ends_with("an audio file's path (~/…) or an audio region's ref")));
    }

    #[test]
    fn a_gain_stage_is_the_last_limiter_on_else_a_last_gain_else_none() {
        let row = |name: &str, bypassed: bool| json!({"ref": format!("1:plugin:0.1.{name}"), "name": name, "bypassed": bypassed});
        let name = |found: Option<(Value, bool)>| found.map(|(row, limited)| (text(&row, "name"), limited));
        assert_eq!(name(gain_plugin(&[row("Limiter", false), row("Channel EQ", false)])), Some(("Limiter".into(), true)));
        assert_eq!(
            name(gain_plugin(&[row("Adaptive Limiter", false), row("Limiter", true), row("Gain", false)])),
            Some(("Adaptive Limiter".into(), true)),
            "a limiter that's bypassed holds nothing"
        );
        assert_eq!(name(gain_plugin(&[row("Channel EQ", false), row("Gain", false)])), Some(("Gain".into(), false)));
        assert_eq!(
            name(gain_plugin(&[row("Gain", false), row("Compressor", false)])),
            None,
            "a Gain before the end turns the level before it"
        );
        assert_eq!(name(gain_plugin(&[row("Gain", false), row("Compressor", true)])), Some(("Gain".into(), false)));
        assert_eq!(name(gain_plugin(&[])), None);
    }

    #[test]
    fn a_gain_is_neutral_only_when_it_turns_the_level_alone() {
        let rows = |make_mono: &str, balance: &str| {
            vec![
                json!({"name": "Balance", "text": balance}),
                json!({"name": "Gain", "text": "-6.0 dB"}),
                json!({"name": "Phase Left", "text": "Norm"}),
                json!({"name": "Phase Right", "text": "Norm"}),
                json!({"name": "Swap L/R", "text": "Norm"}),
                json!({"name": "Make Mono", "text": make_mono}),
            ]
        };
        assert!(neutral_gain(&rows("Stereo", "0.0 %")));
        assert!(!neutral_gain(&rows("Mono", "0.0 %")));
        assert!(!neutral_gain(&rows("Stereo", "-20.0 %")));
        let mut unknown = rows("Stereo", "0.0 %");
        unknown.push(json!({"name": "Mid/Side", "text": "off"}));
        assert!(!neutral_gain(&unknown), "a row Kumi doesn't know");
    }

    #[test]
    fn levels_takes_and_captures_read_as_the_bridge_answers_them() {
        assert_eq!(
            (db_of("-3.0 dB"), db_of("+3.5 dB"), db_of("0.0 dB"), db_of("\u{2212}6 dB")),
            (Some(-3.0), Some(3.5), Some(0.0), Some(-6.0))
        );
        assert_eq!((db_of("-96.0 dB"), db_of("−∞"), db_of("on")), (Some(f64::NEG_INFINITY), Some(f64::NEG_INFINITY), None));
        let take = json!({"file": "/tmp/a.wav", "seconds": 6.5, "lead": 0.5, "sampleRate": 48000});
        assert_eq!(render_of(&take), Some(Render { file: "/tmp/a.wav".into(), start: 0.5, seconds: Some(6.0) }));
        assert_eq!(render_of(&json!({"seconds": 6.0})), None, "no file, no capture");
        assert_eq!(takes_of(&json!({"candidates": [take.clone()], "aligned": {}})).map(|takes| takes.len()), Some(1));
        assert_eq!(takes_of(&json!({"file": "x"})), None);
        // What a run says besides: what wasn't put back, and a cut the clicks didn't time.
        let run = json!({"notPutBack": ["the solos (a dialog came up)", "the metronome (it didn't read back)"], "aligned": {"by": "timecode", "latencyMs": 15.0, "errorMs": 10.0}});
        assert_eq!(
            run_said("Logic Pro", &run),
            [
                "After listening, Kumi couldn't put Logic Pro back as it was: the solos (a dialog came up); the metronome (it didn't read back). Check it in Logic Pro.",
                "Kumi couldn't time Logic Pro's capture by its metronome's clicks, so the part's beats may be up to 10 ms off in the file."
            ]
        );
        assert!(run_said("Logic Pro", &json!({"aligned": {"by": "timecode+click", "latencyMs": 20.4, "errorMs": 1.8}})).is_empty());
    }
}
