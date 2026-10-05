//! What a listening device wrote, made sense of: its raw 32-bit floats (left, right, the beat's phase and
//! Live's position), trimmed to what it recorded, cut where Live's transport ran or jumped, and placed on
//! the Set's beats, so Kumi can take exactly the part it played and hand it to the ear as an ordinary WAV.

use std::path::Path;

use kumi_common::js::number::round;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Live's beat as the device heard it: 0 not recorded, 1 recorded while stopped, 1–2 recorded while playing (1 + the beat's phase).
    pub sync: Vec<f32>,
    /// Live's position in beats, as the device polled it (a few milliseconds behind); from a device that records it.
    pub position: Option<Vec<f32>>,
    pub sample_rate: f64,
}

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
#[derive(Debug, Clone, Copy)]
struct Layout {
    interleaved: bool,
    little_endian: bool,
}

fn is_sync(value: f64) -> bool {
    value == 0.0 || (1.0..=2.000001).contains(&value)
}
/// The shortest stretch worth keeping, in frames.
const MIN_RUN: usize = 64;

/// A device's raw file, read.
pub async fn read_capture(file: &Path, channels: usize, sample_rate: f64) -> Result<Capture, CaptureError> {
    parse_capture(&tokio::fs::read(file).await?, channels, sample_rate)
}

/// The floats as a capture: the layout whose beat channel reads as one, then trimmed to the frames recorded.
pub fn parse_capture(bytes: &[u8], channels: usize, sample_rate: f64) -> Result<Capture, CaptureError> {
    if channels < 3 {
        return Err(CaptureError::Capture("A capture has left, right and the beat.".to_string()));
    }
    // The beat's phase is the third channel; Live's position, when there's a fourth.
    const BEAT: usize = 2;
    let where_ = if channels >= 4 { Some(3usize) } else { None };
    let frames = bytes.len() / 4 / channels;
    if frames == 0 {
        return Ok(Capture { left: Vec::new(), right: Vec::new(), sync: Vec::new(), position: where_.map(|_| Vec::new()), sample_rate });
    }
    let read = |layout: Layout, channel: usize, frame: usize| -> f64 {
        let index = if layout.interleaved { frame * channels + channel } else { channel * frames + frame };
        let word: [u8; 4] = bytes[index * 4..index * 4 + 4].try_into().expect("four bytes");
        (if layout.little_endian { f32::from_le_bytes(word) } else { f32::from_be_bytes(word) }) as f64
    };
    let layouts = [
        Layout { interleaved: true, little_endian: true },
        Layout { interleaved: false, little_endian: true },
        Layout { interleaved: true, little_endian: false },
        Layout { interleaved: false, little_endian: false },
    ];
    // A few thousand frames spread over the file decide it.
    let step = (frames / 4096).max(1);
    let mut best = layouts[0];
    let mut best_score = -1.0;
    for layout in layouts {
        let mut fits = 0usize;
        let mut seen = 0usize;
        for frame in (0..frames).step_by(step) {
            seen += 1;
            let beat = read(layout, BEAT, frame);
            let left = read(layout, 0, frame);
            let here = where_.map(|channel| read(layout, channel, frame)).unwrap_or(0.0);
            if is_sync(beat) && left.is_finite() && left.abs() < 1_000.0 && here.is_finite() && here.abs() < 1e7 {
                fits += 1;
            }
        }
        let score = fits as f64 / seen.max(1) as f64;
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
    let mut position = where_.map(|_| Vec::with_capacity(length));
    for frame in 0..length {
        left.push(read(best, 0, frame) as f32);
        right.push(read(best, 1, frame) as f32);
        sync.push(read(best, BEAT, frame) as f32);
        if let (Some(position), Some(channel)) = (position.as_mut(), where_) {
            position.push(read(best, channel, frame) as f32);
        }
    }
    Ok(Capture { left, right, sync, position, sample_rate })
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
    let mut found: Vec<Run> = Vec::new();
    let mut start: Option<usize> = None;
    for frame in 0..=n {
        let on =
            frame < n && (playing(frame) || (at(frame) == 1.0 && frame > 0 && frame + 1 < n && playing(frame - 1) && playing(frame + 1)));
        let joined = on && start.is_some_and(|start| frame > start) && continues(frame);
        if on && start.is_none() {
            start = Some(frame);
            continue;
        }
        if let Some(from) = start.filter(|_| !on || !joined) {
            // A few frames aren't a stretch: once stopped, the beat holds where it stopped and no frame follows from the last.
            if frame - from >= MIN_RUN {
                found.push(Run { from, to: frame, beat: 0.0, samples_per_beat: 1.0 / step });
            }
            start = if on { Some(frame) } else { None };
        }
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
        // Where the phase reached 1, between the two samples.
        let rise = after + 1.0 - before;
        let at = (frame - 1) as f64 + if rise > 0.0 { (2.0 - before) / rise } else { 0.0 };
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
    let position = capture.position.as_ref().expect("a capture with Live's position");
    let hop = (round(sample_rate / 100.0) as usize).max(1);
    let per_beat = run.samples_per_beat;
    let mut samples: Vec<(usize, f64)> = Vec::new();
    for frame in (run.from..run.to).step_by(hop) {
        let phase = (sync[frame] as f64 - 1.0).max(0.0);
        samples.push((frame, round(position[frame] as f64 - phase) + phase - (frame - run.from) as f64 / per_beat));
    }
    // Samples in a row that agree; only those long enough count.
    let mut groups: Vec<(usize, usize, f64)> = Vec::new();
    for (index, &(_, first)) in samples.iter().enumerate() {
        match groups.last_mut() {
            Some(last) if (first - last.2).abs() < 0.02 => last.1 = index + 1,
            _ => groups.push((index, index + 1, first)),
        }
    }
    let steady: Vec<(usize, usize, f64)> =
        groups.into_iter().filter(|(from, to, _)| ((to - from) * hop) as f64 >= AGREED * sample_rate).collect();
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

/// Part of a capture as a 32-bit float stereo WAV (what the ear reads).
pub async fn write_capture_wav(file: &Path, capture: &Capture, from: f64, to: f64) -> std::io::Result<()> {
    let length = capture.left.len();
    let start = (from.floor().max(0.0) as usize).min(length);
    let end = (to.floor().max(0.0) as usize).min(length).max(start);
    let frames = end - start;
    let mut data: Vec<u8> = Vec::with_capacity(frames * 8);
    for frame in 0..frames {
        data.extend(capture.left[start + frame].to_le_bytes());
        data.extend(capture.right[start + frame].to_le_bytes());
    }
    let sample_rate = capture.sample_rate as u32;
    let mut header: Vec<u8> = Vec::with_capacity(44);
    header.extend(b"RIFF");
    header.extend((36 + data.len() as u32).to_le_bytes());
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
    header.extend((data.len() as u32).to_le_bytes());
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut out = options.open(file).await?;
    out.write_all(&header).await?;
    out.write_all(&data).await?;
    out.flush().await
}

/// The loudest sample of a stretch, in dBFS (-Infinity for silence): a quick "was anything there".
pub fn peak_db(capture: &Capture, from: usize, to: usize) -> f64 {
    let mut peak: f64 = 0.0;
    for frame in from..to.min(capture.left.len()) {
        peak = peak.max((capture.left[frame] as f64).abs()).max((capture.right[frame] as f64).abs());
    }
    if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        f64::NEG_INFINITY
    }
}
