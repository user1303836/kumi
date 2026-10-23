//! Listening (M5): Kumi hears Logic Pro. `hear` plays a part and captures what Logic outputs; `audition` hears several
//! candidates the same way in one capture, each a track soloed alone or the whole mix; `render` bounces a part offline
//! through Logic's Bounce dialog. None is a change: no row, no step of Logic's undo, and Logic is put back as it was
//! (its playhead, solos, metronome and cycle).
//!
//! A pass: Logic stopped, its cycle and metronome off, the solos the pass asks for, the playhead placed exactly on the
//! part's first beat; the helper's process tap on Logic records into a file under the folder the request names
//! (muted for the producer when asked); Logic plays, the playhead is followed ([`follow`]) past the part's end, and
//! Logic stops. After the passes, one more short play a bar past the song's end with the metronome on: its clicks
//! say how late the tap hears what the playhead shows, which changes from one tap to the next (9 to 21 ms on the lab's
//! Mac); every pass of the capture is cut by that ([`Aligned`]). The tap stops, Logic is put back, each pass's part is
//! cut out of the capture into a file of its own, and the capture goes.

pub mod audio;
pub mod follow;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::changes::{busy, cycle, nudge, timeline, tracks, uncertain, Ctx, Done, Epoch};
use crate::helper::{tap_start, tap_stop, HelperError, TapCapture};
use crate::mcu::Mcu;
use crate::queue::Answer;
use crate::refs::{bad, failed};
use crate::rows::Position;
use follow::{Follower, Grid, Line, Poll, Shown};

/// The methods of listening.
pub const METHODS: [&str; 3] = ["hear", "audition", "render"];
/// The longest part a pass plays, and the most candidates an audition hears.
pub const MOST_SECONDS: f64 = 600.0;
pub const MOST_CANDIDATES: usize = 16;
/// How long the follower waits for the surface's first position after Play before it polls the control bar instead
/// (the lab saw it come up to 0.4 s late in a tap's first play).
const NO_TIMECODE: Duration = Duration::from_millis(1500);
/// How far past the part's end the playhead goes before Logic is stopped: the audio comes a little after the display
/// (the latency the clicks measure), and the part's last moments must be in the capture.
const PAST_END_SECONDS: f64 = 0.15;
/// A play is given its part's length and this much more before it counts as stopped short.
const PLAY_GRACE: Duration = Duration::from_secs(4);
/// How many of the display's updates a play needs before it may stop (its line is fitted through them).
const MOST_NEEDED: usize = 8;
/// What a pass's latency is taken as when the clicks can't be heard, and how far off that may be (the lab's taps lay
/// between 9 and 21.5 ms).
const NOMINAL_LATENCY_MS: f64 = 15.0;
const NOMINAL_ERROR_MS: f64 = 10.0;
/// How far a click may land from its beat (the lab's: 9 to 21.5 ms after it), and how far the second click may be
/// from where the first one says.
const CLICK_EARLIEST_MS: f64 = -30.0;
const CLICK_LATEST_MS: f64 = 300.0;
const CLICK_AGREE_MS: f64 = 3.0;
/// What a play's latency varies by within one tap (the lab's spread between plays), which the click's measure carries.
const PLAY_SPREAD_MS: f64 = 1.5;
/// How long a solo gets to show on the headers after it was pressed.
const SOLO_WAIT: Duration = Duration::from_millis(1200);
const SOLO_POLL: Duration = Duration::from_millis(150);
/// How often a heartbeat is due while a pass plays.
const KEEPALIVE: Duration = Duration::from_secs(1);

/// A part to hear: from where, how many beats (the time signature's), where its files go, and whether Logic's own
/// output is kept from the speakers while it is heard.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub from: f64,
    pub beats: f64,
    pub folder: PathBuf,
    pub mute: bool,
}

/// What one pass plays: the tracks soloed alone (by place; none: the whole mix, no track soloed).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub solo: Vec<usize>,
}

/// How a capture was cut: by what the playhead's place was known (`timecode`: the Mackie Control's display;
/// `transport`: the control bar polled), with `+click` when the tap's latency was measured by the metronome's clicks
/// rather than taken as usual; the latency; and how far the cut may be from the part's true start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aligned {
    pub by: &'static str,
    pub latency_ms: f64,
    pub error_ms: f64,
}

impl Aligned {
    fn to_json(self) -> Value {
        json!({"by": self.by, "latencyMs": round(self.latency_ms, 2), "errorMs": round(self.error_ms, 1)})
    }
}

/// A pass heard: its file and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    pub file: PathBuf,
    pub cut: audio::Cut,
    pub note: Option<String>,
}

impl Take {
    fn to_json(&self) -> Value {
        let seconds = if self.cut.rate > 0 { self.cut.frames as f64 / f64::from(self.cut.rate) } else { 0.0 };
        let mut take = json!({
            "file": self.file.to_string_lossy(),
            "seconds": round(seconds, 4),
            "sampleRate": self.cut.rate,
            "channels": self.cut.channels,
            "lead": 0.0,
            "peak": round(self.cut.peak, 6),
            "rms": round(self.cut.rms, 6),
        });
        if let Some(note) = &self.note {
            take["note"] = note.clone().into();
        }
        take
    }
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

// ----- the requests -----

fn number(params: &Value, key: &str) -> Done<Option<f64>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .map(Some)
            .ok_or_else(|| bad(format!("{key} must be a number from 0"))),
    }
}

/// `{fromBeats, beats, folder, mute?}`, checked: a part of at least a sixteenth, whose folder is a whole path to a
/// folder that is there.
pub fn part(params: &Value) -> Done<Part> {
    let from = number(params, "fromBeats")?.ok_or_else(|| bad("fromBeats: where the part starts, in beats from the project's start"))?;
    let beats = number(params, "beats")?.ok_or_else(|| bad("beats: how long the part is, in beats"))?;
    if beats < 0.25 {
        return Err(bad("beats must be at least 0.25"));
    }
    let text = params
        .get("folder")
        .and_then(Value::as_str)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| bad("folder: a whole path to the folder the audio files go into (it must be there)"))?;
    let folder = PathBuf::from(text);
    if !folder.is_absolute() || folder.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(bad("folder must be a whole path, with no \"..\" in it"));
    }
    if !crate::disk::exists(&folder) || crate::disk::list(&folder).is_none() {
        return Err(bad(format!("the folder {} isn't there (or Kumi can't read it); Kumi makes no folders", folder.display())));
    }
    let mute = match params.get("mute") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(mute)) => *mute,
        Some(_) => return Err(bad("mute must be true or false")),
    };
    Ok(Part { from, beats, folder, mute })
}

/// A request's `epoch`, when it gives one, against the project's.
fn check_epoch(ctx: &Ctx, params: &Value) -> Done<()> {
    match params.get("epoch") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(epoch)) if *epoch == ctx.project.epoch.to_string() => Ok(()),
        Some(epoch) => Err((
            daw_bridge::wire::code::STALE.into(),
            format!("the epoch {epoch} is an earlier one (now {}): the tracks or regions changed, read again", ctx.project.epoch),
        )),
    }
}

/// A track ref of this epoch: its place.
fn track_of(ctx: &Ctx, reference: &Value) -> Done<usize> {
    crate::changes::track_place(ctx, &json!({"ref": reference}), "ref", Epoch::Current)
}

// ----- Logic as it was, and put back -----

/// What a pass changes of Logic, as it was before: every track's solo, the metronome, the cycle and the playhead.
#[derive(Debug, Clone)]
struct Before {
    solos: Vec<bool>,
    /// Which tracks the producer muted (a muted track stays muted, soloed or not).
    mutes: Vec<bool>,
    names: Vec<String>,
    metronome: bool,
    cycle: bool,
    playhead: Position,
}

fn before(ctx: &mut Ctx) -> Done<(Before, crate::focus::Transport)> {
    let (bar, transport) = timeline::control_bar(ctx)?;
    if transport.playing || transport.recording {
        return Err(busy("Logic Pro is playing: Kumi listens only while it is stopped; nothing changed"));
    }
    let metronome = timeline::metronome_on(&bar, &ctx.project.strings);
    let headers = nudge::read(tracks::headers(ctx))?;
    Ok((
        Before {
            solos: headers.iter().map(|found| found.header.solo).collect(),
            mutes: headers.iter().map(|found| found.header.mute).collect(),
            names: headers.iter().map(|found| found.header.name.clone()).collect(),
            metronome,
            cycle: transport.cycle,
            playhead: transport.position(),
        },
        transport,
    ))
}

/// Whether the surface's strips line up with the tracks (the bank starts at the first track and the LCD names them in
/// order, as Logic last wrote the names): then the strip of each of the first eight tracks is its place's.
fn strips_line_up(mcu: Option<&Mcu>, names: &[String]) -> bool {
    let Some(mcu) = mcu.filter(|mcu| mcu.open) else { return false };
    !names.is_empty()
        && names.iter().take(crate::mcu::STRIPS).enumerate().all(|(i, name)| {
            let short: String = name.chars().take(crate::mcu::LCD_COLUMNS).collect();
            mcu.names.get(i).is_some_and(|lcd| !lcd.is_empty() && *lcd == short.trim())
        })
}

/// Sets every track's solo to `wanted` (by place): through the surface's solo buttons when its strips line up with the
/// tracks (`lined_up`: no click, Logic left where it is), else each header's checkbox; read back from the headers.
fn put_solos(ctx: &mut Ctx, mcu: Option<&mut Mcu>, lined_up: bool, wanted: &[bool]) -> Done<()> {
    let now: Vec<bool> = nudge::read(tracks::headers(ctx))?.iter().map(|found| found.header.solo).collect();
    let differ: Vec<usize> = (0..wanted.len().min(now.len())).filter(|&i| now[i] != wanted[i]).collect();
    if differ.is_empty() {
        return Ok(());
    }
    let mut by_click = differ.clone();
    if let Some(mcu) = mcu.filter(|_| lined_up) {
        by_click = differ.iter().copied().filter(|&i| i >= crate::mcu::STRIPS).collect();
        for &i in differ.iter().filter(|&&i| i < crate::mcu::STRIPS) {
            nudge::wrote(mcu.press_solo(i, &mut *ctx.ax.helper).map_err(crate::ax::AxError::from))?;
        }
        // Logic shows the solos on the headers a moment later.
        let until = Instant::now() + SOLO_WAIT;
        loop {
            crate::changes::pause(SOLO_POLL);
            let shown: Vec<bool> = nudge::read(tracks::headers(ctx))?.iter().map(|found| found.header.solo).collect();
            let left: Vec<usize> = differ.iter().copied().filter(|&i| i < crate::mcu::STRIPS && shown.get(i) != Some(&wanted[i])).collect();
            if left.is_empty() {
                break;
            }
            if Instant::now() >= until {
                by_click.extend(left);
                break;
            }
        }
    }
    for i in by_click {
        let on = tracks::set_solo(ctx, i + 1, wanted[i])?;
        if on != wanted[i] {
            return Err(nudge::kept(&format!("track {}'s solo", i + 1), wanted[i], on));
        }
    }
    Ok(())
}

/// Puts back what the passes changed, each part tried whatever the others did: the solos, the metronome, the playhead
/// (before the cycle, whose left locator the Stop button would go to), the cycle. What couldn't be put back, in words.
fn put_back(ctx: &mut Ctx, mcu: Option<&mut Mcu>, lined_up: bool, was: &Before) -> Vec<String> {
    let mut missed = vec![];
    if let Err((_, why)) = timeline::stop(ctx) {
        missed.push(format!("Logic Pro may still be playing ({why})"));
    }
    if let Err((_, why)) = put_solos(ctx, mcu, lined_up, &was.solos) {
        missed.push(format!("the solos ({why})"));
    }
    if let Err((_, why)) = timeline::set_metronome(ctx, was.metronome) {
        missed.push(format!("the metronome ({why})"));
    }
    let at = Position { bar: was.playhead.bar, beat: was.playhead.beat, ..Position::START };
    if let Err((_, why)) = timeline::set_playhead_exactly(ctx, at) {
        missed.push(format!("the playhead ({why})"));
    }
    if was.cycle {
        if let Err((_, why)) = cycle::set_cycle(ctx, true) {
            missed.push(format!("the cycle ({why})"));
        }
    }
    missed
}

// ----- playing and following -----

/// What a play saw of the playhead: the display's updates, or the control bar's polls.
#[derive(Debug, Clone, Default)]
struct Played {
    shown: Vec<Shown>,
    polls: Vec<Poll>,
    /// Where the playhead got (beats), and whether that was past where the play was to go.
    reached: f64,
    through: bool,
}

/// How far the display's update says the playhead is, in beats: the division taken as a sixteenth unless the display
/// counted more of them in a beat (a finer division read as a sixteenth would run ahead).
fn shown_beats(shown: &Shown, grid: &Grid, divisions: i64) -> f64 {
    let per_division = grid.ticks_per_beat / divisions.max(4) as f64;
    grid.ticks(shown.bbt, per_division) / grid.ticks_per_beat
}

/// Plays from where the playhead is and follows it until it passes `until` (beats), then stops Logic.
fn play(ctx: &mut Ctx, mcu: Option<&mut Mcu>, grid: &Grid, until: f64, seconds: f64, keepalive: &mut dyn FnMut()) -> Done<Played> {
    let surface = mcu.filter(|mcu| mcu.open && mcu.surface.state.installed);
    let mut follower = match &surface {
        Some(mcu) => Follower::new(mcu.surface.state.beats_mode()),
        None => Follower::new(false),
    };
    timeline::play(ctx)?;
    let started = Instant::now();
    let bound = Duration::from_secs_f64(seconds.max(0.0)) + PLAY_GRACE;
    let mut beat_at = Instant::now();
    let mut played = Played::default();
    let mut divisions = 4;
    let mut by_timecode = surface.is_some();
    if let Some(mcu) = surface {
        loop {
            if beat_at.elapsed() >= KEEPALIVE {
                keepalive();
                beat_at = Instant::now();
            }
            // Logic streams the surface's meters too: the times are looked at whether an event came or not.
            if let Some(event) = ctx.ax.helper.event(Duration::from_millis(20)) {
                mcu.on_event(&event, &mut *ctx.ax.helper);
                let seen = follower.shown.len();
                follower.feed(&event);
                if follower.shown.len() > seen {
                    let last = *follower.shown.last().expect("one more");
                    divisions = divisions.max(last.bbt[2]);
                    played.reached = shown_beats(&last, grid, divisions);
                    if played.reached >= until && follower.shown.len() >= MOST_NEEDED {
                        played.through = true;
                        break;
                    }
                }
                if follower.stopped {
                    break;
                }
            }
            if follower.shown.is_empty() && started.elapsed() >= NO_TIMECODE {
                by_timecode = false;
                break;
            }
            if started.elapsed() >= bound {
                break;
            }
        }
        played.shown = follower.shown;
    }
    if !by_timecode {
        loop {
            if beat_at.elapsed() >= KEEPALIVE {
                keepalive();
                beat_at = Instant::now();
            }
            let before = crate::disk::host_now();
            let (_, transport) = timeline::control_bar(ctx)?;
            let after = crate::disk::host_now();
            let beats = transport.beats();
            played.polls.push(Poll { before, after, beats, playing: transport.playing });
            played.reached = beats;
            if !transport.playing || beats >= until.ceil() || started.elapsed() >= bound {
                played.through = beats >= until;
                break;
            }
        }
    }
    timeline::stop(ctx)?;
    Ok(played)
}

// ----- the passes -----

/// The files of a pass's run: the capture, and each pass's part, numbered past any file already in the folder.
fn files(folder: &Path, stem: &str, passes: usize) -> (PathBuf, Vec<PathBuf>) {
    let named = |n: usize| -> (PathBuf, Vec<PathBuf>) {
        let capture = folder.join(format!("kumi-tap-{n}.wav"));
        let parts = if passes == 1 {
            vec![folder.join(format!("kumi-{stem}-{n}.wav"))]
        } else {
            (1..=passes).map(|k| folder.join(format!("kumi-{stem}-{n}-{k}.wav"))).collect()
        };
        (capture, parts)
    };
    let mut n = 1;
    loop {
        let (capture, parts) = named(n);
        if !crate::disk::exists(&capture) && parts.iter().all(|part| !crate::disk::exists(part)) {
            return (capture, parts);
        }
        n += 1;
    }
}

/// The song's end: where the last region ends, in beats.
fn song_end(ctx: &mut Ctx, beats_per_bar: u32) -> Done<f64> {
    let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    Ok(regions.iter().map(|(_, region)| region.end.beats(beats_per_bar)).fold(0.0, f64::max))
}

/// What a run of passes heard: a take for each, in order, and how the capture was cut.
#[derive(Debug, Clone)]
pub struct Heard {
    pub takes: Vec<Take>,
    pub aligned: Aligned,
    /// Whether the metronome was on before (it is off for the passes, and put back).
    pub metronome: bool,
    /// What couldn't be put back of Logic as it was.
    pub missed: Vec<String>,
}

/// What the answer says of the metronome: the click is never in what Kumi hears.
fn metronome_word(was_on: bool) -> &'static str {
    if was_on {
        "off for the pass"
    } else {
        "off"
    }
}

/// Runs the passes in one capture, as the module's page says; Logic put back whatever happens.
pub fn passes(
    ctx: &mut Ctx,
    mut mcu: Option<&mut Mcu>,
    part: &Part,
    plans: &[Plan],
    stem: &str,
    keepalive: &mut dyn FnMut(),
) -> Done<Heard> {
    let (was, transport) = before(ctx)?;
    let lined_up = strips_line_up(mcu.as_deref(), &was.names);
    for plan in plans {
        if let Some(place) = plan.solo.iter().find(|&&place| place == 0 || place > was.solos.len()) {
            return Err(crate::refs::not_in_project(&format!("{}:track:{place}", ctx.project.epoch)));
        }
    }
    let grid = Grid::of(&transport.time_signature);
    if transport.tempo <= 0.0 {
        return Err(failed("Logic Pro's control bar shows no tempo"));
    }
    let ns_per_tick = 60.0e9 / (transport.tempo * follow::TICKS_PER_QUARTER);
    let seconds_per_beat = grid.ticks_per_beat * ns_per_tick / 1.0e9;
    let seconds = part.beats * seconds_per_beat;
    if seconds > MOST_SECONDS {
        return Err(bad(format!(
            "Kumi hears at most {MOST_SECONDS} s at once; {} beats at {} BPM is {seconds:.0} s",
            part.beats, transport.tempo
        )));
    }
    let end = song_end(ctx, grid.beats_per_bar)?;
    let bar = f64::from(grid.beats_per_bar);
    let click_at = ((end / bar).ceil() + 1.0) * bar;
    let (capture_path, part_paths) = files(&part.folder, stem, plans.len());
    let past = PAST_END_SECONDS / seconds_per_beat;
    let total = plans.len() as f64 * (seconds + 6.0) + 2.5 * seconds_per_beat + 20.0;

    // From here on Logic is put back whatever happens.
    let mut cycle_off = Ok(true);
    if was.cycle {
        cycle_off = cycle::set_cycle(ctx, false).map(|on| !on);
    }
    let started = cycle_off.and_then(|_| {
        timeline::set_metronome(ctx, false)?;
        tap_start(&mut *ctx.ax.helper, ctx.ax.pid, &capture_path, part.mute, Some(total.min(3600.0))).map_err(tap_fault)
    });
    let mut plays: Vec<Played> = vec![];
    let mut calibration: Option<Played> = None;
    let mut tapped: Option<TapCapture> = None;
    let run = match started {
        Err(fault) => Err(fault),
        Ok(_) => {
            let run = (|| -> Done<()> {
                let first = Position::from_beats(part.from.floor(), grid.beats_per_bar);
                for plan in plans {
                    let wanted: Vec<bool> = (1..=was.solos.len()).map(|place| plan.solo.contains(&place)).collect();
                    put_solos(ctx, mcu.as_deref_mut(), lined_up, &wanted)?;
                    timeline::set_playhead_exactly(ctx, first)?;
                    let until = part.from + part.beats + past;
                    plays.push(play(ctx, mcu.as_deref_mut(), &grid, until, (until - part.from.floor()) * seconds_per_beat, keepalive)?);
                }
                // The clicks: a bar past the song's end, the metronome on, two beats.
                timeline::set_metronome(ctx, true)?;
                timeline::set_playhead_exactly(ctx, Position::from_beats(click_at, grid.beats_per_bar))?;
                calibration =
                    Some(play(ctx, mcu.as_deref_mut(), &grid, click_at + 1.0 + past, (1.0 + past) * seconds_per_beat, keepalive)?);
                Ok(())
            })();
            match tap_stop(&mut *ctx.ax.helper) {
                Ok(capture) => tapped = Some(capture),
                Err(error) => crate::log::line(&format!("the tap didn't stop as asked: {error}")),
            }
            run
        }
    };
    let missed = put_back(ctx, mcu, lined_up, &was);
    if !missed.is_empty() {
        crate::log::line(&format!("after listening, Kumi couldn't put back {}", missed.join("; ")));
    }
    run?;
    let capture = tapped.ok_or_else(|| uncertain("the helper's tap didn't stop as asked; nothing was heard"))?;
    let heard = cut(&capture, &capture_path, &part_paths, part, &grid, ns_per_tick, &plays, calibration.as_ref(), click_at);
    crate::disk::remove(&capture_path);
    let (mut takes, aligned) = heard?;
    for (take, plan) in takes.iter_mut().zip(plans) {
        let muted: Vec<&str> = plan
            .solo
            .iter()
            .filter(|&&place| was.mutes.get(place - 1) == Some(&true))
            .map(|&place| was.names[place - 1].as_str())
            .collect();
        if !muted.is_empty() {
            let said = format!(
                "{} muted in Logic Pro, so soloing doesn't make it heard",
                muted.join(", ") + if muted.len() == 1 { " is" } else { " are" }
            );
            take.note = Some(take.note.take().map_or(said.clone(), |note| format!("{note}; {said}")));
        }
    }
    Ok(Heard { takes, aligned, metronome: was.metronome, missed })
}

fn tap_fault(error: HelperError) -> crate::refs::Fault {
    match &error {
        HelperError::Refused { code, reply } if code == "denied" => failed(format!(
            "macOS refused to let Kumi's helper hear Logic Pro: {}",
            reply.get("detail").and_then(Value::as_str).unwrap_or("allow System Audio Recording for the app Kumi runs in")
        )),
        HelperError::Refused { code, .. } if code == "busy" => busy("Kumi's helper is hearing something else already; nothing changed"),
        HelperError::Refused { code, .. } if code == "unsupported" => failed("hearing Logic Pro needs macOS 14.2 or later"),
        _ => failed(format!("Kumi's helper couldn't hear Logic Pro: {error}")),
    }
}

/// A play's line of host time (ns) against ticks, and the alignment it was found by.
fn line_of(played: &Played, grid: &Grid, ns_per_tick: f64, ns_per_host: f64) -> Option<(Line, &'static str, f64)> {
    if played.shown.len() >= 3 {
        let per_division = follow::ticks_per_division(&played.shown, grid, ns_per_tick, ns_per_host);
        let points: Vec<(f64, f64)> =
            played.shown.iter().map(|shown| (shown.host as f64 * ns_per_host, grid.ticks(shown.bbt, per_division))).collect();
        return follow::fit(&points, ns_per_tick).map(|line| (line, "timecode", line.spread_ms));
    }
    let (points, widest_ms) = follow::poll_points(&played.polls, grid, ns_per_host);
    if points.len() >= 3 {
        // Few points, each good to half the polls' spacing: the tempo's slope, through their mean.
        let origin = points.iter().map(|(host, ticks)| host - ticks * ns_per_tick).sum::<f64>() / points.len() as f64;
        return Some((Line { origin, ns_per_tick, spread_ms: widest_ms / 2.0, kept: points.len() }, "transport", widest_ms / 2.0));
    }
    None
}

/// Where a host time (ns) falls in the capture, as a frame.
fn frame_of(capture: &TapCapture, ns: f64, ns_per_host: f64) -> Option<f64> {
    capture.frame_at((ns / ns_per_host).max(0.0) as u64)
}

/// The latency the clicks measured (frames), or None: the first onset near the first click's beat and, when the
/// second is heard, the second's agreeing with it.
fn latency(capture: &TapCapture, path: &Path, line: &Line, grid: &Grid, click_at: f64, ns_per_host: f64) -> Option<(f64, f64)> {
    let rate = capture.sample_rate;
    let ms = |frames: f64| frames / rate * 1000.0;
    let expected: Vec<f64> = (0..2).filter_map(|k| frame_of(capture, line.at(grid.of_beats(click_at + k as f64)), ns_per_host)).collect();
    let [first, second] = expected[..] else { return None };
    let from = (first + CLICK_EARLIEST_MS / 1000.0 * rate).max(0.0) as u64;
    let to = (second + CLICK_LATEST_MS / 1000.0 * rate) as u64;
    let onsets = crate::disk::onsets(path, from, to.min(capture.frames))?;
    let late = onsets.iter().map(|onset| onset - first).find(|late| (CLICK_EARLIEST_MS..=CLICK_LATEST_MS).contains(&ms(*late)))?;
    let again = onsets.iter().map(|onset| onset - second).find(|other| ms((other - late).abs()) <= CLICK_AGREE_MS);
    Some(match again {
        Some(other) => ((late + other) / 2.0, ms((late - other).abs()) / 2.0),
        None => (late, PLAY_SPREAD_MS),
    })
}

/// Cuts each pass's part out of the capture.
#[allow(clippy::too_many_arguments)]
fn cut(
    capture: &TapCapture,
    capture_path: &Path,
    part_paths: &[PathBuf],
    part: &Part,
    grid: &Grid,
    ns_per_tick: f64,
    plays: &[Played],
    calibration: Option<&Played>,
    click_at: f64,
) -> Done<(Vec<Take>, Aligned)> {
    let ns_per_host = f64::from(capture.timebase.numer) / f64::from(capture.timebase.denom);
    let rate = capture.sample_rate;
    // The latency, measured by the clicks when they were heard (how well: the clicks' agreement and the line they were
    // placed by, and what a play varies by within a tap), else the usual one.
    let measured = calibration.and_then(|played| line_of(played, grid, ns_per_tick, ns_per_host)).and_then(|(line, _, spread)| {
        latency(capture, capture_path, &line, grid, click_at, ns_per_host).map(|(frames, error)| (frames, error + spread))
    });
    let (latency_frames, latency_error) = match measured {
        Some((frames, error)) => (frames, error + PLAY_SPREAD_MS),
        None => (NOMINAL_LATENCY_MS / 1000.0 * rate, NOMINAL_ERROR_MS),
    };
    let mut takes = vec![];
    let (mut polled, mut error_ms) = (false, latency_error);
    for (played, path) in plays.iter().zip(part_paths) {
        let (line, by, spread) = line_of(played, grid, ns_per_tick, ns_per_host).ok_or_else(|| {
            uncertain(
                "Logic Pro played, but Kumi couldn't follow its playhead (neither the Mackie Control nor the control bar showed it moving)",
            )
        })?;
        polled |= by == "transport";
        error_ms = error_ms.max(latency_error + spread);
        let start = frame_of(capture, line.at(grid.of_beats(part.from)), ns_per_host).map(|f| f + latency_frames);
        let end = frame_of(capture, line.at(grid.of_beats(part.from + part.beats)), ns_per_host).map(|f| f + latency_frames);
        let (Some(start), Some(end)) = (start, end) else {
            return Err(uncertain("the capture holds no times to place the part by"));
        };
        let (start, end) = (start.round().max(0.0) as u64, end.round().max(0.0) as u64);
        let cut = crate::disk::cut(capture_path, path, start, end.saturating_sub(start))
            .ok_or_else(|| failed(format!("Kumi couldn't write {} from the capture", path.display())))?;
        let mut note = None;
        if !played.through {
            note = Some(format!(
                "Logic Pro stopped at beat {:.2} before the part's end at beat {:.2}; the file holds what was played",
                played.reached,
                part.from + part.beats
            ));
        } else if end > capture.frames {
            note = Some("the capture ended before the part did; the file holds what was captured".to_string());
        }
        takes.push(Take { file: path.clone(), cut, note });
    }
    let by = match (polled, measured.is_some()) {
        (false, true) => "timecode+click",
        (false, false) => "timecode",
        (true, true) => "transport+click",
        (true, false) => "transport",
    };
    let aligned = Aligned { by, latency_ms: latency_frames / rate * 1000.0, error_ms };
    Ok((takes, aligned))
}

// ----- the methods -----

/// `hear {fromBeats, beats, folder, mute?, solo?: [track refs], epoch?}`: one pass, the tracks in `solo` soloed alone
/// (none: the whole mix).
pub fn hear(ctx: &mut Ctx, mcu: Option<&mut Mcu>, params: &Value, keepalive: &mut dyn FnMut()) -> Answer {
    check_epoch(ctx, params)?;
    let part = part(params)?;
    let refs: Vec<Value> = match params.get("solo") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(refs)) => refs.clone(),
        Some(_) => return Err(bad("solo must be a list of track refs")),
    };
    let solo = refs.iter().map(|reference| track_of(ctx, reference)).collect::<Done<Vec<usize>>>()?;
    let heard = passes(ctx, mcu, &part, &[Plan { solo }], "hear", keepalive)?;
    let mut answer = heard.takes[0].to_json();
    answer["aligned"] = heard.aligned.to_json();
    answer["fromBeats"] = part.from.into();
    answer["beats"] = part.beats.into();
    answer["mute"] = part.mute.into();
    answer["solo"] = Value::Array(refs);
    answer["metronome"] = metronome_word(heard.metronome).into();
    answer["epoch"] = ctx.project.epoch.to_string().into();
    with_missed(&mut answer, &heard.missed);
    Ok(answer)
}

fn with_missed(answer: &mut Value, missed: &[String]) {
    if !missed.is_empty() {
        answer["notPutBack"] = missed.to_vec().into();
    }
}

/// `audition {candidates: [{ref} | {mix: true}], fromBeats, beats, folder, mute?, epoch?}`: one pass a candidate, in
/// one capture, each a track soloed alone or the whole mix; the takes in the candidates' order.
pub fn audition(ctx: &mut Ctx, mcu: Option<&mut Mcu>, params: &Value, keepalive: &mut dyn FnMut()) -> Answer {
    check_epoch(ctx, params)?;
    let part = part(params)?;
    let candidates = params
        .get("candidates")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or_else(|| bad("candidates: a list of {\"ref\": <track ref>} or {\"mix\": true}"))?;
    if candidates.len() > MOST_CANDIDATES {
        return Err(bad(format!("Kumi auditions at most {MOST_CANDIDATES} candidates at once")));
    }
    let mut plans = vec![];
    for candidate in candidates {
        if candidate.get("mix") == Some(&Value::Bool(true)) {
            plans.push(Plan { solo: vec![] });
        } else if let Some(reference) = candidate.get("ref") {
            plans.push(Plan { solo: vec![track_of(ctx, reference)?] });
        } else {
            return Err(bad("each candidate is {\"ref\": <track ref>} or {\"mix\": true}"));
        }
    }
    let names: Vec<String> = nudge::read(tracks::headers(ctx))?.iter().map(|found| found.label()).collect();
    let heard = passes(ctx, mcu, &part, &plans, "audition", keepalive)?;
    let listed: Vec<Value> = candidates
        .iter()
        .zip(&plans)
        .zip(&heard.takes)
        .map(|((candidate, plan), take)| {
            let mut entry = take.to_json();
            match plan.solo.first() {
                Some(place) => {
                    entry["ref"] = candidate["ref"].clone();
                    entry["name"] = names.get(place - 1).cloned().unwrap_or_default().into();
                }
                None => {
                    entry["mix"] = true.into();
                    entry["name"] = "the whole mix".into();
                }
            }
            entry
        })
        .collect();
    let mut answer = json!({
        "epoch": ctx.project.epoch.to_string(),
        "fromBeats": part.from,
        "beats": part.beats,
        "mute": part.mute,
        "aligned": heard.aligned.to_json(),
        "metronome": metronome_word(heard.metronome),
        "candidates": listed,
    });
    with_missed(&mut answer, &heard.missed);
    Ok(answer)
}

/// `render {ref?, fromBeats, beats, folder, epoch?}`: the part bounced offline through Logic's Bounce dialog (M4b's
/// `bounce`, 32-bit float WAV), the metronome off; a track's own audio by soloing it alone for the bounce, which then
/// passes the Stereo Out's plug-ins; without `ref`, the whole mix (no track soloed).
pub fn render(ctx: &mut Ctx, mcu: Option<&mut Mcu>, params: &Value) -> Answer {
    check_epoch(ctx, params)?;
    let part = part(params)?;
    let track = match params.get("ref") {
        None | Some(Value::Null) => None,
        Some(reference) => Some(track_of(ctx, reference)?),
    };
    let (was, _) = before(ctx)?;
    if let Some(place) = track.filter(|&place| place > was.solos.len()) {
        return Err(crate::refs::not_in_project(&format!("{}:track:{place}", ctx.project.epoch)));
    }
    let mut n = 1;
    let path = loop {
        let path = part.folder.join(format!("kumi-render-{n}.wav"));
        if !crate::disk::exists(&path) {
            break path;
        }
        n += 1;
    };
    let mut mcu = mcu;
    let lined_up = strips_line_up(mcu.as_deref(), &was.names);
    let wanted: Vec<bool> = (1..=was.solos.len()).map(|place| Some(place) == track).collect();
    let bounced = put_solos(ctx, mcu.as_deref_mut(), lined_up, &wanted).and_then(|_| {
        timeline::set_metronome(ctx, false)?;
        crate::changes::bounce::bounce(
            ctx,
            &json!({"startBeats": part.from, "endBeats": part.from + part.beats, "path": path.to_string_lossy(), "format": "wav32f"}),
        )
    });
    let mut missed = vec![];
    if let Err((_, why)) = put_solos(ctx, mcu, lined_up, &was.solos) {
        missed.push(format!("the solos ({why})"));
    }
    if let Err((_, why)) = timeline::set_metronome(ctx, was.metronome) {
        missed.push(format!("the metronome ({why})"));
    }
    let bounced = bounced?;
    let mut answer = json!({
        "epoch": ctx.project.epoch.to_string(),
        "file": bounced["bounced"],
        "seconds": bounced["seconds"],
        "sampleRate": bounced["rate"],
        "channels": bounced["channels"],
        "bits": bounced["bits"],
        "lead": 0.0,
        "fromBeats": part.from,
        "beats": part.beats,
        "metronome": metronome_word(was.metronome),
    });
    if let Some(reference) = params.get("ref").filter(|r| !r.is_null()) {
        answer["ref"] = reference.clone();
        answer["note"] = "The track was soloed alone for the bounce, so its audio passed the Stereo Out's plug-ins.".into();
    }
    with_missed(&mut answer, &missed);
    Ok(answer)
}

/// Answers a listening method over Logic, with the Mackie Control when the bridge has one: the run loop's for a queued
/// request, the recorder's and the tests' over a helper of their own. `keepalive` is called about once a second while
/// Logic plays (the bridge sends its heartbeats).
pub fn answer(ctx: &mut Ctx, mcu: Option<&mut Mcu>, method: &str, params: &Value, keepalive: &mut dyn FnMut()) -> Answer {
    match method {
        "hear" => hear(ctx, mcu, params, keepalive),
        "audition" => audition(ctx, mcu, params, keepalive),
        "render" => render(ctx, mcu, params),
        other => Err((daw_bridge::wire::code::UNKNOWN_METHOD.into(), format!("the Logic Pro bridge has no method {other:?}"))),
    }
}

/// How long a listening request may take, in seconds, at a tempo: what the bridge waits for its answer, and what the
/// host's call must allow. A pass is the part plus 8 s (the playhead placed, the solos, the stop), the clicks 10 s, a
/// bounce its dialog's 30 s and the part played offline.
pub fn bound_seconds(method: &str, params: &Value, tempo: f64) -> f64 {
    let tempo = if tempo > 0.0 { tempo } else { 60.0 };
    let beats = params.get("beats").and_then(Value::as_f64).filter(|b| b.is_finite() && *b > 0.0).unwrap_or(16.0);
    let seconds = (beats * 60.0 / tempo).min(MOST_SECONDS);
    match method {
        "render" => seconds + 45.0,
        "audition" => {
            let passes = params.get("candidates").and_then(Value::as_array).map_or(1, Vec::len).clamp(1, MOST_CANDIDATES);
            passes as f64 * (seconds + 8.0) + 20.0
        }
        _ => seconds + 8.0 + 20.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::Timebase;

    /// What the display showed while Logic played from `origin` (the host time, in ns, of tick 0) for `seconds`, an
    /// update every 33 ms, in sixteenths.
    fn shown(origin: f64, from_ticks: f64, seconds: f64) -> Vec<Shown> {
        let ns_per_tick = 60.0e9 / (160.0 * 960.0);
        (0..((seconds / 0.033) as usize))
            .map(|i| {
                let host = origin + from_ticks * ns_per_tick + 0.004e9 + i as f64 * 0.033e9;
                let ticks = ((host - origin) / ns_per_tick) as i64;
                let (beat, rest) = (ticks / 960, ticks % 960);
                Shown { host: host as u64, bbt: [1 + beat / 4, 1 + beat % 4, 1 + rest / 240, 1 + rest % 240] }
            })
            .collect()
    }

    #[test]
    fn a_pass_is_cut_where_the_playheads_line_and_the_clicks_latency_put_its_beats() {
        let dir = tempfile::tempdir().unwrap();
        let capture_path = dir.path().join("kumi-tap-1.wav");
        // The capture: one frame each 1/44100 s from host 1 s (the clock in nanoseconds), silent but for two clicks
        // 20 ms after the calibration's beats 20 and 21 (host 3 s and 3.375 s), and a hit on the part's second beat,
        // as late after it (host 1.2 s + 375 ms + 20 ms).
        let rate = 44100.0;
        let frame = |host: f64| ((host - 1.0e9) / 1.0e9 * rate).round() as usize;
        let mut samples = vec![0f32; 2 * 150_000];
        for click in [frame(3.02e9), frame(3.395e9), frame(1.595e9)] {
            for k in 0..32 {
                samples[(click + k) * 2] = 0.4;
                samples[(click + k) * 2 + 1] = 0.4;
            }
        }
        let mut bytes = audio::header(44100, 2, 150_000);
        for sample in &samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&capture_path, bytes).unwrap();
        let capture = TapCapture {
            file: capture_path.clone(),
            frames: 150_000,
            seconds: 150_000.0 / rate,
            sample_rate: rate,
            channels: 2,
            peak: 0.4,
            rms: 0.0,
            anchors: (0..4).map(|s| (s * 44100, 1_000_000_000 + s * 1_000_000_000)).collect(),
            timebase: Timebase { numer: 1, denom: 1 },
            dropped: 0,
            truncated: false,
        };
        let grid = Grid::of("4/4");
        let ns_per_tick = 60.0e9 / (160.0 * 960.0);
        // The part played from bar 1 at host 1.2 s; the clicks from beat 20 at host 3 s.
        let part_play = Played { shown: shown(1.2e9, 0.0, 1.7), reached: 4.4, through: true, ..Played::default() };
        let click_origin = 3.0e9 - 20.0 * 960.0 * ns_per_tick;
        let clicks = Played { shown: shown(click_origin, 20.0 * 960.0, 0.6), reached: 21.2, through: true, ..Played::default() };
        let part = Part { from: 0.0, beats: 4.0, folder: dir.path().into(), mute: true };
        let parts = [dir.path().join("kumi-hear-1.wav")];
        let (takes, aligned) = cut(&capture, &capture_path, &parts, &part, &grid, ns_per_tick, &[part_play], Some(&clicks), 20.0).unwrap();
        // The display truncates its ticks, so a line runs up to a tick (0.4 ms at 160 BPM) late; the clicks' line and the
        // part's carry it alike, and it cancels where the part is cut.
        assert_eq!(aligned.by, "timecode+click");
        assert!((aligned.latency_ms - 20.0).abs() < 0.5, "{aligned:?}");
        assert!(aligned.error_ms < 3.0, "{aligned:?}");
        // The part starts at host 1.2 s and its audio 20 ms later; four beats are 1.5 s.
        let (mut file, wav) = audio::open(&parts[0]).unwrap();
        assert!((wav.frames as i64 - 66150).abs() <= 1, "{}", wav.frames);
        assert_eq!(takes[0].cut.frames, wav.frames);
        // The hit on the second beat lands 375 ms into the file, where the beats say, within a fraction of a ms.
        let hit = audio::onsets(&audio::read_frames(&mut file, &wav, 0, wav.frames).unwrap(), 2, rate);
        assert_eq!(hit.len(), 1, "{hit:?}");
        assert!((hit[0] - 0.375 * rate).abs() <= 20.0, "{} frames from where the beat says", hit[0] - 0.375 * rate);
        // Without the clicks: the usual latency, and an error to say so.
        let silent = dir.path().join("kumi-tap-2.wav");
        std::fs::write(&silent, audio::header(44100, 2, 0)).unwrap();
        let other = [dir.path().join("kumi-hear-2.wav")];
        let part_play = Played { shown: shown(1.2e9, 0.0, 1.7), reached: 4.4, through: true, ..Played::default() };
        let (_, nominal) = cut(&capture, &silent, &other, &part, &grid, ns_per_tick, &[part_play], Some(&clicks), 20.0).unwrap();
        assert_eq!((nominal.by, nominal.latency_ms, nominal.error_ms >= NOMINAL_ERROR_MS), ("timecode", NOMINAL_LATENCY_MS, true));
    }

    #[test]
    fn a_play_that_stopped_short_says_so_and_the_files_are_numbered_past_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("kumi-hear-1.wav"), b"").unwrap();
        let (capture, parts) = files(dir.path(), "hear", 1);
        assert_eq!((capture.file_name().unwrap(), parts[0].file_name().unwrap()), ("kumi-tap-2.wav".as_ref(), "kumi-hear-2.wav".as_ref()));
        let (_, parts) = files(dir.path(), "audition", 3);
        assert_eq!(
            parts.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(),
            ["kumi-audition-1-1.wav", "kumi-audition-1-2.wav", "kumi-audition-1-3.wav"]
        );
        assert_eq!(shown_beats(&Shown { host: 0, bbt: [2, 3, 2, 121] }, &Grid::of("4/4"), 4), 6.0 + 0.25 + 120.0 / 960.0);
        assert_eq!(shown_beats(&Shown { host: 0, bbt: [1, 1, 6, 1] }, &Grid::of("4/4"), 8), 5.0 / 8.0, "a finer division seen");
        assert_eq!(metronome_word(true), "off for the pass");
        assert_eq!(
            Aligned { by: "click", latency_ms: 20.456, error_ms: 1.83 }.to_json(),
            json!({"by": "click", "latencyMs": 20.46, "errorMs": 1.8})
        );
    }
}
