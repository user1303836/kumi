//! What a listening device wrote, made sense of: its raw 32-bit floats (left, right, the beat's phase and
//! Live's position), trimmed to what it recorded, cut where Live's transport ran or jumped, and placed on
//! the Set's beats, so Kumi can take exactly the part it played and hand it to the ear as an ordinary WAV.

use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use kumi_common::js::number::{round, to_string};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    /// The sound, when it was read into memory; a capture read from the device's file leaves it there (see `raw`).
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Live's beat as the device heard it: 0 not recorded, 1 recorded while stopped, 1–2 recorded while playing (1 + the beat's phase).
    pub sync: Vec<f32>,
    /// Live's position in beats, as the device polled it (a few milliseconds behind), at every
    /// [`POSITION_STEP`]th frame; from a device that records it.
    pub position: Option<Vec<f32>>,
    pub sample_rate: f64,
    /// Where the sound still is: the device's raw file, read only for the part Kumi keeps.
    pub raw: Option<RawAudio>,
}

impl Capture {
    /// How many frames were recorded.
    pub fn frames(&self) -> usize {
        self.sync.len()
    }
    /// Live's position at a frame, as polled.
    fn position_at(&self, frame: usize) -> Option<f64> {
        self.position.as_ref().and_then(|position| position.get(frame / POSITION_STEP)).map(|value| *value as f64)
    }
}

/// A capture's sound left in the device's raw file: a long one is hundreds of megabytes, and Kumi keeps one part of it.
#[derive(Debug, Clone, PartialEq)]
pub struct RawAudio {
    pub file: PathBuf,
    pub channels: usize,
    /// Frames in the file, recorded or not: where each channel starts when they're one after another.
    pub frames: usize,
    layout: Layout,
}

/// Live's position is polled every few milliseconds, so every 16th frame of it is kept.
pub const POSITION_STEP: usize = 16;

/// A stretch the transport ran through without a jump: its frames, the beat its first frame is on, and samples per beat.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub from: usize,
    pub to: usize,
    pub beat: f64,
    pub samples_per_beat: f64,
}

/// What beat a stretch started near, when the capture has no position of Live's own (see [`runs`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Anchors {
    pub first: Option<f64>,
    pub after_jump: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("{0}")]
    Capture(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How the device laid its floats out (Max doesn't say): channels interleaved or one after another, and the byte order.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    interleaved: bool,
    little_endian: bool,
}

const LAYOUTS: [Layout; 4] = [
    Layout { interleaved: true, little_endian: true },
    Layout { interleaved: false, little_endian: true },
    Layout { interleaved: true, little_endian: false },
    Layout { interleaved: false, little_endian: false },
];
/// The beat's phase is the third channel; Live's position, when there's a fourth.
const BEAT: usize = 2;
const WHERE: usize = 3;

impl Layout {
    /// Where a channel's frame is, in floats from the file's start.
    fn index(self, channels: usize, frames: usize, channel: usize, frame: usize) -> usize {
        if self.interleaved {
            frame * channels + channel
        } else {
            channel * frames + frame
        }
    }
    fn value(self, word: [u8; 4]) -> f32 {
        if self.little_endian {
            f32::from_le_bytes(word)
        } else {
            f32::from_be_bytes(word)
        }
    }
}

fn is_sync(value: f64) -> bool {
    value == 0.0 || (1.0..=2.0 + TOP_SLACK).contains(&value)
}
/// Whether a frame reads as one the device wrote: a beat channel that's one, sound and a position in range.
fn fits(beat: f64, left: f64, here: f64) -> bool {
    is_sync(beat) && left.is_finite() && left.abs() < 1_000.0 && here.is_finite() && here.abs() < 1e7
}
/// The shortest stretch worth keeping, in frames.
const MIN_RUN: usize = 64;
/// How far past the top of its ramp the beat's phase may read (#252): when a beat lands on the edge of a signal
/// vector, Max's `plugphasor~` holds the phase at the top (2.0, or a hair over) for the rest of that vector.
const TOP_SLACK: f64 = 1e-4;
/// The longest such hold that still joins the stretch on either side: a signal vector, with room for big ones.
const TOP_HOLD: usize = 1024;
/// Frames read from a raw file at a time.
const CHUNK: usize = 1 << 16;

/// A device's raw file, read: its beat and Live's position into memory, its sound left in the file until a part of it
/// is written out.
pub async fn read_capture(file: &Path, channels: usize, sample_rate: f64) -> Result<Capture, CaptureError> {
    let file = file.to_path_buf();
    tokio::task::spawn_blocking(move || read_raw(&file, channels, sample_rate))
        .await
        .map_err(|error| CaptureError::Capture(error.to_string()))?
}

fn read_raw(path: &Path, channels: usize, sample_rate: f64) -> Result<Capture, CaptureError> {
    if channels < 3 {
        return Err(CaptureError::Capture("A capture has left, right and the beat.".to_string()));
    }
    let mut file = std::fs::File::open(path)?;
    let frames = (file.metadata()?.len() / 4 / channels as u64) as usize;
    let where_ = (channels > WHERE).then_some(WHERE);
    if frames == 0 {
        return Ok(Capture { left: vec![], right: vec![], sync: vec![], position: where_.map(|_| vec![]), sample_rate, raw: None });
    }
    // A thousand frames spread over the file decide the layout.
    let step = (frames / 1024).max(1);
    let mut best = LAYOUTS[0];
    let mut best_score = -1.0;
    for layout in LAYOUTS {
        let (mut fitting, mut seen) = (0usize, 0usize);
        for frame in (0..frames).step_by(step) {
            seen += 1;
            let mut at = |channel: usize| -> std::io::Result<f64> {
                let mut word = [0u8; 4];
                file.seek(SeekFrom::Start(4 * layout.index(channels, frames, channel, frame) as u64))?;
                file.read_exact(&mut word)?;
                Ok(layout.value(word) as f64)
            };
            let (beat, left) = (at(BEAT)?, at(0)?);
            let here = match where_ {
                Some(channel) => at(channel)?,
                None => 0.0,
            };
            if fits(beat, left, here) {
                fitting += 1;
            }
        }
        let score = fitting as f64 / seen.max(1) as f64;
        if score > best_score {
            best_score = score;
            best = layout;
        }
    }
    let raw = RawAudio { file: path.to_path_buf(), channels, frames, layout: best };
    let mut sync = Vec::with_capacity(frames);
    let mut position = where_.map(|_| Vec::with_capacity(frames.div_ceil(POSITION_STEP)));
    let wanted: Vec<usize> = std::iter::once(BEAT).chain(where_).collect();
    raw_chunks(&mut file, &raw, &wanted, 0, frames, |first, values| {
        sync.extend_from_slice(&values[0]);
        if let Some(position) = position.as_mut() {
            let skip = (POSITION_STEP - first % POSITION_STEP) % POSITION_STEP;
            position.extend(values[1].iter().skip(skip).step_by(POSITION_STEP));
        }
        Ok(())
    })?;
    // Recording starts at the buffer's start, so what follows the last recorded frame was never written.
    let length = sync.iter().rposition(|value| *value >= 1.0).map_or(0, |last| last + 1);
    sync.truncate(length);
    if let Some(position) = position.as_mut() {
        position.truncate(length.div_ceil(POSITION_STEP));
    }
    Ok(Capture { left: vec![], right: vec![], sync, position, sample_rate, raw: Some(raw) })
}

/// Channels of a raw file, `CHUNK` frames at a time from `from` to `to`: each call has the first frame and one run of
/// values per wanted channel.
fn raw_chunks(
    file: &mut std::fs::File,
    raw: &RawAudio,
    wanted: &[usize],
    from: usize,
    to: usize,
    mut each: impl FnMut(usize, &[Vec<f32>]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut values: Vec<Vec<f32>> = wanted.iter().map(|_| Vec::with_capacity(CHUNK)).collect();
    let mut bytes = vec![0u8; CHUNK * 4 * if raw.layout.interleaved { raw.channels } else { 1 }];
    let mut first = from;
    while first < to {
        let count = CHUNK.min(to - first);
        for run in &mut values {
            run.clear();
        }
        if raw.layout.interleaved {
            let span = &mut bytes[..count * raw.channels * 4];
            file.seek(SeekFrom::Start(4 * raw.layout.index(raw.channels, raw.frames, 0, first) as u64))?;
            file.read_exact(span)?;
            for frame in 0..count {
                for (run, channel) in values.iter_mut().zip(wanted) {
                    let at = 4 * (frame * raw.channels + channel);
                    run.push(raw.layout.value(span[at..at + 4].try_into().expect("four bytes")));
                }
            }
        } else {
            for (run, channel) in values.iter_mut().zip(wanted) {
                let span = &mut bytes[..count * 4];
                file.seek(SeekFrom::Start(4 * raw.layout.index(raw.channels, raw.frames, *channel, first) as u64))?;
                file.read_exact(span)?;
                run.extend(span.chunks_exact(4).map(|word| raw.layout.value(word.try_into().expect("four bytes"))));
            }
        }
        each(first, &values)?;
        first += count;
    }
    Ok(())
}

/// The floats as a capture, in memory: the layout whose beat channel reads as one, then trimmed to the frames recorded.
pub fn parse_capture(bytes: &[u8], channels: usize, sample_rate: f64) -> Result<Capture, CaptureError> {
    if channels < 3 {
        return Err(CaptureError::Capture("A capture has left, right and the beat.".to_string()));
    }
    let where_ = (channels > WHERE).then_some(WHERE);
    let frames = bytes.len() / 4 / channels;
    if frames == 0 {
        return Ok(Capture {
            left: Vec::new(),
            right: Vec::new(),
            sync: Vec::new(),
            position: where_.map(|_| Vec::new()),
            sample_rate,
            raw: None,
        });
    }
    let read = |layout: Layout, channel: usize, frame: usize| -> f64 {
        let index = layout.index(channels, frames, channel, frame);
        layout.value(bytes[index * 4..index * 4 + 4].try_into().expect("four bytes")) as f64
    };
    // A few thousand frames spread over the file decide it.
    let step = (frames / 4096).max(1);
    let mut best = LAYOUTS[0];
    let mut best_score = -1.0;
    for layout in LAYOUTS {
        let mut fitting = 0usize;
        let mut seen = 0usize;
        for frame in (0..frames).step_by(step) {
            seen += 1;
            let here = where_.map(|channel| read(layout, channel, frame)).unwrap_or(0.0);
            if fits(read(layout, BEAT, frame), read(layout, 0, frame), here) {
                fitting += 1;
            }
        }
        let score = fitting as f64 / seen.max(1) as f64;
        if score > best_score {
            best_score = score;
            best = layout;
        }
    }
    // Recording starts at the buffer's start, so what follows the last recorded frame was never written.
    let mut length = frames;
    while length > 0 && !(read(best, BEAT, length - 1) >= 1.0) {
        length -= 1;
    }
    let mut left = Vec::with_capacity(length);
    let mut right = Vec::with_capacity(length);
    let mut sync = Vec::with_capacity(length);
    let mut position = where_.map(|_| Vec::with_capacity(length.div_ceil(POSITION_STEP)));
    for frame in 0..length {
        left.push(read(best, 0, frame) as f32);
        right.push(read(best, 1, frame) as f32);
        sync.push(read(best, BEAT, frame) as f32);
        if let (Some(position), Some(channel)) = (position.as_mut(), where_) {
            if frame % POSITION_STEP == 0 {
                position.push(read(best, channel, frame) as f32);
            }
        }
    }
    Ok(Capture { left, right, sync, position, sample_rate, raw: None })
}

/// The stretches where Live's transport ran, split where it jumped (a move of the playhead while playing).
/// A capture with Live's position places each stretch by it, and splits where it says Live jumped on a beat
/// (Live waits for its launch quantization, so the phase alone often can't show the jump). Without it,
/// `anchors` say what beat a stretch started near: the playhead Kumi jumped to, or where the Set was when
/// the device was armed; the beat's phase makes that exact. A stretch with no anchor near it is placed by
/// the one before it.
pub fn runs(capture: &Capture, anchors: Anchors) -> Vec<Run> {
    let sync = &capture.sync;
    let n = sync.len();
    let at = |frame: usize| sync[frame] as f64;
    let playing = |frame: usize| at(frame) > 1.0 + 1e-7;
    // How far the phase moves each sample while playing: the most common step.
    let mut steps: Vec<f64> = Vec::new();
    for frame in 1..n {
        if steps.len() >= 20_000 {
            break;
        }
        let delta = at(frame) - at(frame - 1);
        if playing(frame) && playing(frame - 1) && delta > 0.0 && delta < 0.01 {
            steps.push(delta);
        }
    }
    if steps.is_empty() {
        return Vec::new();
    }
    steps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let step = steps[steps.len() / 2];
    let tolerance = (step * 0.35).max(1e-6);
    let continues = |frame: usize| {
        let delta = at(frame) - at(frame - 1);
        (delta - step).abs() <= tolerance || (delta - (step - 1.0)).abs() <= tolerance
            // A phase of exactly 0 (a beat's first sample) reads as stopped: it continues when the phase before was
            // just under 1 (the beat came round). A jump that lands on a beat has some other phase before it.
            || (at(frame) == 1.0 && (at(frame - 1) - 2.0 + step).abs() <= tolerance)
    };
    let on = |frame: usize| {
        frame < n && (playing(frame) || (at(frame) == 1.0 && frame > 0 && frame + 1 < n && playing(frame - 1) && playing(frame + 1)))
    };
    // Where a stretch goes on past the phase held at the top of its ramp (#252): the frame after the hold, when the
    // phase there is where it would be had it kept moving from the last frame before the top.
    let past_hold = |from: usize, frame: usize| -> Option<usize> {
        let top = at(frame);
        if !(top >= 2.0 - 2.0 * step && top <= 2.0 + TOP_SLACK) {
            return None;
        }
        let mut first = frame;
        while first > from && at(first - 1) == top {
            first -= 1;
        }
        let before = first.checked_sub(1).filter(|before| *before >= from && at(*before) < top)?;
        let mut after = frame + 1;
        while after < n && at(after) == top && after - first <= TOP_HOLD {
            after += 1;
        }
        if after >= n || after - first > TOP_HOLD || !on(after) {
            return None;
        }
        let elapsed = (after - before) as f64;
        let expected = (at(before) - 1.0 + elapsed * step).rem_euclid(1.0);
        let off = (at(after) - 1.0 - expected).rem_euclid(1.0);
        (off.min(1.0 - off) <= tolerance + elapsed * 2e-7).then_some(after)
    };
    let mut found: Vec<Run> = Vec::new();
    let mut start: Option<usize> = None;
    let mut frame = 0;
    while frame <= n {
        let on = on(frame);
        let joined = on && start.is_some_and(|start| frame > start) && continues(frame);
        if on && start.is_none() {
            start = Some(frame);
            frame += 1;
            continue;
        }
        if let Some(from) = start.filter(|_| !on || !joined) {
            if let Some(after) = (on && frame > from).then(|| past_hold(from, frame)).flatten() {
                frame = after + 1;
                continue;
            }
            // A few frames aren't a stretch: once stopped, the beat holds where it stopped and no frame follows from the last.
            if frame - from >= MIN_RUN {
                found.push(Run { from, to: frame, beat: 0.0, samples_per_beat: 1.0 / step });
            }
            start = if on { Some(frame) } else { None };
        }
        frame += 1;
    }
    // How long a beat is, measured where the phase comes round (the step between two samples is too coarse in
    // 32-bit floats: a fraction of a percent, milliseconds over a bar).
    let mut measured: Vec<f64> = found.iter().filter_map(|run| beat_length(sync, run.from, run.to)).collect();
    measured.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let typical = if measured.is_empty() { 1.0 / step } else { measured[measured.len() / 2] };
    for run in &mut found {
        run.samples_per_beat = beat_length(sync, run.from, run.to).unwrap_or(typical);
    }
    if capture.position.is_some() {
        return found.iter().flat_map(|run| placed(capture, run)).collect();
    }
    // Each stretch's first beat: its phase is exact, the whole beats come from the anchor nearest it.
    let mut previous: Option<Run> = None;
    for (index, run) in found.iter_mut().enumerate() {
        let phase = at(run.from) - 1.0;
        let guess = match (index, anchors.first, anchors.after_jump, &previous) {
            (0, Some(first), _, _) => first,
            (index, _, Some(after_jump), _) if index > 0 => after_jump,
            (_, _, _, Some(previous)) => previous.beat + (run.from - previous.from) as f64 / previous.samples_per_beat,
            (_, first, after_jump, None) => first.or(after_jump).unwrap_or(0.0),
        };
        run.beat = round(guess - phase) + phase;
        previous = Some(run.clone());
    }
    found
}

/// Samples per beat in a stretch, from where its phase comes round (to a fraction of a sample); None with fewer than two.
fn beat_length(sync: &[f32], from: usize, to: usize) -> Option<f64> {
    let mut first: Option<f64> = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for frame in (from + 1)..to {
        let before = sync[frame - 1] as f64;
        let after = sync[frame] as f64;
        if after >= before {
            continue;
        }
        // A phase held at the top (#252) came round where the hold began, between its first frame and the one before.
        let mut top = frame - 1;
        while top > from + 1 && sync[top - 1] == sync[top] && frame - top <= TOP_HOLD {
            top -= 1;
        }
        let at = if top < frame - 1 && sync[top - 1] < sync[top] {
            let (below, held) = (sync[top - 1] as f64, sync[top] as f64);
            (top - 1) as f64 + if held >= 2.0 { ((2.0 - below) / (held - below)).clamp(0.0, 1.0) } else { 1.0 }
        } else {
            // Where the phase reached 1, between the two samples.
            let rise = after + 1.0 - before;
            (frame - 1) as f64 + if rise > 0.0 { (2.0 - before) / rise } else { 0.0 }
        };
        first.get_or_insert(at);
        last = at;
        count += 1;
    }
    match first {
        Some(first) if count >= 2 => Some((last - first) / (count - 1) as f64),
        _ => None,
    }
}

/// How far behind Live the polled position may be, at most, in seconds; and how long it must agree to count.
const POSITION_LAG: f64 = 0.06;
const AGREED: f64 = 0.06;

/// A stretch placed by Live's position: every 10 ms the position (rounded to the beat the phase is in) says
/// what beat the stretch started on. Where that changes and stays changed, Live jumped: on the last beat
/// before the position caught up (where the phase came round), or where it caught up when no beat was there.
/// A while that doesn't agree long enough (the position catching up after a jump the phase showed) goes with
/// the part after it.
fn placed(capture: &Capture, run: &Run) -> Vec<Run> {
    let sync = &capture.sync;
    let sample_rate = capture.sample_rate;
    let hop = (round(sample_rate / 100.0) as usize).max(1);
    let per_beat = run.samples_per_beat;
    let mut samples: Vec<(usize, f64)> = Vec::new();
    for frame in (run.from..run.to).step_by(hop) {
        let phase = (sync[frame] as f64 - 1.0).max(0.0);
        let position = capture.position_at(frame).expect("a capture with Live's position");
        samples.push((frame, round(position - phase) + phase - (frame - run.from) as f64 / per_beat));
    }
    // Samples in a row that agree (each with the one before, so a long stretch never drifts apart); only those long
    // enough count, and two that agree with a blip between them (a late poll, a held phase) are one.
    let mut groups: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (index, &(_, first)) in samples.iter().enumerate() {
        match groups.last_mut() {
            Some(last) if (first - last.3).abs() < 0.02 => {
                last.1 = index + 1;
                last.3 = first;
            }
            _ => groups.push((index, index + 1, first, first)),
        }
    }
    let mut steady: Vec<(usize, usize, f64)> = Vec::new();
    let mut latest = f64::NAN;
    for (from, to, first, last) in groups {
        if (((to - from) * hop) as f64) < AGREED * sample_rate {
            continue;
        }
        match steady.last_mut() {
            Some(group) if (first - latest).abs() < 0.02 => group.1 = to,
            _ => steady.push((from, to, first)),
        }
        latest = last;
    }
    let mut found: Vec<Run> = Vec::new();
    let mut from = run.from;
    for (index, group) in steady.iter().enumerate() {
        let next = steady.get(index + 1);
        let mut to = run.to;
        if let Some(next) = next {
            let caught_up = samples[next.0].0;
            to = caught_up;
            let floor = ((from + 1) as f64).max(caught_up as f64 - POSITION_LAG * sample_rate);
            let mut frame = caught_up;
            while (frame as f64) > floor {
                if sync[frame] < sync[frame - 1] {
                    to = frame;
                    break;
                }
                frame -= 1;
            }
        }
        if to as i64 - from as i64 >= MIN_RUN as i64 {
            found.push(Run { from, to, beat: group.2 + (from - run.from) as f64 / per_beat, samples_per_beat: per_beat });
        }
        from = to;
    }
    found
}

/// Max's beat ramp reads 0 for its first signal vector after Live starts or jumps (64 samples; at most this).
const FIRST_VECTOR: usize = 256;

/// The frame a beat falls on within a run, or None when the run doesn't cover it. A run's first signal
/// vector (where the ramp read 0) is Live playing too, so a beat just before the run's start is found there.
pub fn frame_at(run: &Run, beat: f64) -> Option<usize> {
    let frame = round(run.from as f64 + (beat - run.beat) * run.samples_per_beat);
    let earliest = run.from.saturating_sub(FIRST_VECTOR) as f64;
    if frame >= earliest && frame < run.to as f64 {
        Some(frame as usize)
    } else {
        None
    }
}

/// Why no stretch covers a window: what the capture held instead, to say what really happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Missed {
    /// Live never played while the device recorded.
    Nothing,
    /// Live reached the window's start only after it had begun (a jump that came late): a longer lead-in helps.
    Late { from_beat: f64 },
    /// Live played over the window's start but stopped or jumped before its end: `run` is what it heard of it.
    Cut { run: Run, to_beat: f64, pieces: usize, seconds: f64 },
    /// Live played, but somewhere else.
    Elsewhere { from_beat: f64, to_beat: f64 },
    /// The device's recording couldn't be read.
    Unread(String),
}

impl Missed {
    /// Whether playing the window again, from further ahead, can help.
    pub fn retry(&self) -> bool {
        matches!(self, Missed::Late { .. } | Missed::Elsewhere { .. })
    }
    pub fn describe(&self, from: f64, beats: f64) -> String {
        let span = format!("beats {}–{}", to_string(from), to_string(from + beats));
        match self {
            Missed::Nothing => "Live didn't play while Kumi listened.".into(),
            Missed::Late { from_beat } => {
                format!("Live reached {span} late: Kumi heard it only from beat {}.", to_string(round(*from_beat * 100.0) / 100.0))
            }
            Missed::Cut { to_beat, pieces, seconds, .. } => format!(
                "Kumi heard {} s while Live played{}, but only up to beat {} of {span}.",
                to_string(round(*seconds * 10.0) / 10.0),
                if *pieces > 1 { format!(", cut into {pieces} pieces") } else { String::new() },
                to_string(round(*to_beat * 100.0) / 100.0)
            ),
            Missed::Elsewhere { from_beat, to_beat } => format!(
                "Live played beats {}–{} instead of {span}.",
                to_string(round(*from_beat * 100.0) / 100.0),
                to_string(round(*to_beat * 100.0) / 100.0)
            ),
            Missed::Unread(why) => format!("Kumi couldn't read what it heard of {span}: {why}"),
        }
    }
}

/// The stretch that covers `beats` from `from`, joining pieces that follow one another on Live's beats (no jump
/// between them, only a few frames apart: a blip in the phase, not in the sound); or why there's none.
pub fn cover(stretches: &[Run], from: f64, beats: f64, sample_rate: f64) -> Result<Run, Missed> {
    // A blip in the phase is far shorter than this, in frames; and placed this close, in beats.
    const GAP: usize = 4096;
    let mut joined: Vec<Run> = Vec::new();
    for run in stretches {
        if let Some(last) = joined.last_mut() {
            let expected = last.beat + (run.from - last.from) as f64 / last.samples_per_beat;
            if run.from >= last.to && run.from - last.to <= GAP && (run.beat - expected).abs() < 0.01 {
                last.to = run.to;
                continue;
            }
        }
        joined.push(run.clone());
    }
    let end = from + beats - 1e-3;
    if let Some(found) = joined.iter().rev().find(|run| frame_at(run, from).is_some() && frame_at(run, end).is_some()) {
        return Ok(found.clone());
    }
    let reach = |run: &Run| run.beat + (run.to - run.from) as f64 / run.samples_per_beat;
    if joined.is_empty() {
        return Err(Missed::Nothing);
    }
    if let Some(late) = joined.iter().find(|run| run.beat > from && run.beat < end && frame_at(run, end).is_some()) {
        return Err(Missed::Late { from_beat: late.beat });
    }
    if let Some(cut) = joined.iter().filter(|run| frame_at(run, from).is_some()).max_by(|a, b| reach(a).total_cmp(&reach(b))) {
        let pieces = stretches.iter().filter(|run| run.beat < end && reach(run) > from).count().max(1);
        let frames: usize = stretches.iter().map(|run| run.to - run.from).sum();
        return Err(Missed::Cut { run: cut.clone(), to_beat: reach(cut), pieces, seconds: frames as f64 / sample_rate });
    }
    let first = joined.iter().map(|run| run.beat).fold(f64::INFINITY, f64::min);
    let last = joined.iter().map(reach).fold(f64::NEG_INFINITY, f64::max);
    Err(Missed::Elsewhere { from_beat: first, to_beat: last })
}

/// Part of a capture as a 32-bit float stereo WAV (what the ear reads), from frame `from` to `to`: from memory, or
/// copied from the device's raw file a chunk at a time.
pub async fn write_capture_wav(file: &Path, capture: &Capture, from: f64, to: f64) -> std::io::Result<()> {
    let length = capture.frames();
    let start = (from.floor().max(0.0) as usize).min(length);
    let end = (to.floor().max(0.0) as usize).min(length).max(start);
    let header = wav_header((end - start) * 8, capture.sample_rate);
    if let Some(raw) = capture.raw.clone() {
        let file = file.to_path_buf();
        return tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let mut out = std::io::BufWriter::new(private_std_file(&file)?);
            out.write_all(&header)?;
            let mut input = std::fs::File::open(&raw.file)?;
            raw_chunks(&mut input, &raw, &[0, 1], start, end, |_, values| out.write_all(&interleaved(&values[0], &values[1])))?;
            out.flush()
        })
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    }
    let mut out = private_file(file).await?;
    out.write_all(&header).await?;
    for first in (start..end).step_by(CHUNK) {
        let last = (first + CHUNK).min(end);
        out.write_all(&interleaved(&capture.left[first..last], &capture.right[first..last])).await?;
    }
    out.flush().await
}

fn interleaved(left: &[f32], right: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(left.len() * 8);
    for (left, right) in left.iter().zip(right) {
        bytes.extend(left.to_le_bytes());
        bytes.extend(right.to_le_bytes());
    }
    bytes
}

/// Kumi's own stereo 32-bit float WAVs (from [`write_capture_wav`]), one after another as one file. Each part but the
/// last carries `tail` frames past its end that the next part starts with: heard in another playback (an LFO or a delay
/// somewhere else in its cycle), so the two are crossfaded there rather than butted together.
pub async fn join_wavs(file: &Path, parts: &[(PathBuf, usize)]) -> std::io::Result<()> {
    let (file, parts) = (file.to_path_buf(), parts.to_vec());
    tokio::task::spawn_blocking(move || join_parts(&file, &parts)).await.map_err(|error| std::io::Error::other(error.to_string()))?
}

fn join_parts(file: &Path, parts: &[(PathBuf, usize)]) -> std::io::Result<()> {
    let mut sample_rate = 0.0;
    let mut sizes = Vec::with_capacity(parts.len());
    for (part, _) in parts {
        let mut header = [0u8; 44];
        std::fs::File::open(part)?.read_exact(&mut header)?;
        sample_rate = u32::from_le_bytes(header[24..28].try_into().expect("four bytes")) as f64;
        sizes.push(u32::from_le_bytes(header[40..44].try_into().expect("four bytes")) as usize / 8);
    }
    // Each seam's overlap, no longer than either side of it.
    let overlaps: Vec<usize> = (0..parts.len())
        .map(|index| if index + 1 < parts.len() { parts[index].1.min(sizes[index]).min(sizes[index + 1]) } else { 0 })
        .collect();
    let total = sizes.iter().sum::<usize>() - overlaps.iter().sum::<usize>();
    let mut out = std::io::BufWriter::new(private_std_file(file)?);
    out.write_all(&wav_header(total * 8, sample_rate))?;
    let mut held: Vec<f32> = vec![];
    let mut bytes = vec![0u8; CHUNK * 8];
    for (index, (part, _)) in parts.iter().enumerate() {
        let frames = sizes[index];
        let mut input = std::io::BufReader::new(std::fs::File::open(part)?);
        input.seek(SeekFrom::Start(44))?;
        // The previous part's tail fades out as this part's start fades in; this part's own tail waits for the next.
        let fade = held.len() / 2;
        let written = frames - overlaps[index];
        let mut tail = Vec::with_capacity(overlaps[index] * 2);
        let mut first = 0;
        while first < frames {
            let count = CHUNK.min(frames - first);
            input.read_exact(&mut bytes[..count * 8])?;
            let mut samples: Vec<f32> =
                bytes[..count * 8].chunks_exact(4).map(|word| f32::from_le_bytes(word.try_into().expect("four bytes"))).collect();
            for frame in first..(first + count).min(fade) {
                let weight = (frame as f32 + 0.5) / fade as f32;
                for channel in 0..2 {
                    let (at, was) = ((frame - first) * 2 + channel, frame * 2 + channel);
                    samples[at] = held[was] * (1.0 - weight) + samples[at] * weight;
                }
            }
            let kept = written.clamp(first, first + count) - first;
            out.write_all(&interleaved_samples(&samples[..kept * 2]))?;
            tail.extend_from_slice(&samples[kept * 2..]);
            first += count;
        }
        held = tail;
    }
    out.flush()
}

fn interleaved_samples(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

fn wav_header(data: usize, sample_rate: f64) -> Vec<u8> {
    let sample_rate = sample_rate as u32;
    let mut header: Vec<u8> = Vec::with_capacity(44);
    header.extend(b"RIFF");
    header.extend((36 + data as u32).to_le_bytes());
    header.extend(b"WAVE");
    header.extend(b"fmt ");
    header.extend(16u32.to_le_bytes());
    header.extend(3u16.to_le_bytes());
    header.extend(2u16.to_le_bytes());
    header.extend(sample_rate.to_le_bytes());
    header.extend((sample_rate * 8).to_le_bytes());
    header.extend(8u16.to_le_bytes());
    header.extend(32u16.to_le_bytes());
    header.extend(b"data");
    header.extend((data as u32).to_le_bytes());
    header
}

fn private_std_file(file: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)
}

async fn private_file(file: &Path) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    options.open(file).await
}
