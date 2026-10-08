//! Pitched parts as notes, with Basic Pitch (Spotify's, run in Kumi's own model runtime): for a reference that isn't
//! in Live, where Live's own conversions can't reach. Audio at 22 050 Hz in overlapping 2 s windows; the model's note
//! and onset activations per frame (86 a second, 88 keys) read into notes as Basic Pitch's own decoding does: onsets
//! (predicted, or where a note's activation rises) followed while the note sounds, then what's left traced from its
//! strongest point (its "melodia trick").
//!
//! The decoding and windowing are translated from Basic Pitch's `basic_pitch/note_creation.py`, `inference.py` and
//! `constants.py` (github.com/spotify/basic-pitch at fa5997af0a8210982619003269994a1be25eddf3), Copyright 2022 Spotify
//! AB, under the Apache License 2.0. Changed: written in Rust, reading the note and onset outputs only (no pitch
//! bends, contours or MIDI files), resampled with Kumi's own resampler. THIRD_PARTY_NOTICES.md carries its notices.

use crate::{
    audio::decode::open_audio_to,
    listening::embed::resample,
    models::{self, pinned, Say, Tensor},
};
use kumi_common::abort::Signal;
use std::path::Path;

const RATE: f64 = 22_050.;
const HOP: usize = 256;
/// A window's samples (2 s less a hop) and frames.
const WINDOW: usize = 43_844;
const WINDOW_FRAMES: usize = 172;
/// Frames dropped at each end of a window's activations (their overlap with the next), and the overlap in samples.
const OVERLAP_FRAMES: usize = 30;
const KEYS: usize = 88;
const LOWEST_MIDI: u8 = 21;
/// Basic Pitch's own thresholds: an onset at 0.5, a note sounding at 0.3, at least 11 frames (128 ms), and how many
/// frames under the threshold end a note.
const ONSET: f32 = 0.5;
const FRAME: f32 = 0.3;
const SHORTEST: usize = 11;
const ENERGY_TOLERANCE: usize = 11;

/// A note heard: when (seconds), its MIDI pitch, and how strongly (0–1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub strength: f32,
}

/// A frame's time, seconds, as Basic Pitch places it (each window's frames a little early, which it corrects).
pub fn frame_time(frame: usize) -> f64 {
    let offset = (HOP as f64 / RATE) * (WINDOW_FRAMES as f64 - WINDOW as f64 / HOP as f64) + 0.0018;
    frame as f64 * HOP as f64 / RATE - offset * (frame / WINDOW_FRAMES) as f64
}

/// Onsets where the model predicted them, or where a note's activation rises over two frames (scaled to the
/// predictions' strongest), whichever is stronger.
fn inferred_onsets(onsets: &[Vec<f32>], frames: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let count = frames.len();
    let mut rises = vec![vec![0f32; KEYS]; count];
    for t in 2..count {
        for key in 0..KEYS {
            let one = frames[t][key] - frames[t - 1][key];
            let two = frames[t][key] - frames[t - 2][key];
            rises[t][key] = one.min(two).max(0.);
        }
    }
    // The first two frames rise from nothing before them: Basic Pitch zeroes those rises, as they are here.
    let strongest_onset = onsets.iter().flatten().copied().fold(0f32, f32::max);
    let strongest_rise = rises.iter().flatten().copied().fold(0f32, f32::max);
    (0..count)
        .map(|t| {
            (0..KEYS)
                .map(|key| {
                    let rise = if strongest_rise > 0. { strongest_onset * rises[t][key] / strongest_rise } else { 0. };
                    onsets[t][key].max(rise)
                })
                .collect()
        })
        .collect()
}

/// Notes from the model's per-frame activations (frames × 88 keys): each as its first frame, its end frame, its MIDI
/// pitch and its mean activation.
pub fn notes_from(frames: &[Vec<f32>], onsets: &[Vec<f32>]) -> Vec<(usize, usize, u8, f32)> {
    let count = frames.len();
    if count < 3 {
        return vec![];
    }
    let onsets = inferred_onsets(onsets, frames);
    // Onsets: peaks in time over the threshold, latest first.
    let mut starts: Vec<(usize, usize)> = vec![];
    for t in 1..count - 1 {
        for key in 0..KEYS {
            let value = onsets[t][key];
            if value >= ONSET && value > onsets[t - 1][key] && value > onsets[t + 1][key] {
                starts.push((t, key));
            }
        }
    }
    starts.sort_by(|a, b| b.cmp(a));
    let mut energy: Vec<Vec<f32>> = frames.to_vec();
    let clear = |energy: &mut Vec<Vec<f32>>, t: usize, key: usize| {
        energy[t][key] = 0.;
        if key + 1 < KEYS {
            energy[t][key + 1] = 0.;
        }
        if key > 0 {
            energy[t][key - 1] = 0.;
        }
    };
    let mean = |from: usize, to: usize, key: usize| -> f32 {
        if to <= from {
            return 0.;
        }
        frames[from..to].iter().map(|row| row[key]).sum::<f32>() / (to - from) as f32
    };
    let mut notes = vec![];
    for (start, key) in starts {
        if start >= count - 1 {
            continue;
        }
        // Followed until the note's activation stays under the threshold for a while.
        let (mut t, mut under) = (start + 1, 0);
        while t < count - 1 && under < ENERGY_TOLERANCE {
            if energy[t][key] < FRAME {
                under += 1;
            } else {
                under = 0;
            }
            t += 1;
        }
        let end = t - under;
        if end <= start + SHORTEST {
            continue;
        }
        for t in start..end {
            clear(&mut energy, t, key);
        }
        notes.push((start, end, key as u8 + LOWEST_MIDI, mean(start, end, key)));
    }
    // What's left, traced out from its strongest point both ways.
    loop {
        let (mut best, mut at) = (FRAME, None);
        for (t, row) in energy.iter().enumerate() {
            for (key, value) in row.iter().enumerate() {
                if *value > best {
                    best = *value;
                    at = Some((t, key));
                }
            }
        }
        let Some((middle, key)) = at else { break };
        energy[middle][key] = 0.;
        let (mut t, mut under) = (middle + 1, 0);
        while t < count - 1 && under < ENERGY_TOLERANCE {
            if energy[t][key] < FRAME {
                under += 1;
            } else {
                under = 0;
            }
            clear(&mut energy, t, key);
            t += 1;
        }
        let end = t - 1 - under;
        let (mut t, mut under) = (middle as i64 - 1, 0);
        while t > 0 && under < ENERGY_TOLERANCE {
            if energy[t as usize][key] < FRAME {
                under += 1;
            } else {
                under = 0;
            }
            clear(&mut energy, t as usize, key);
            t -= 1;
        }
        let start = (t + 1 + under as i64).max(0) as usize;
        if end <= start + SHORTEST {
            continue;
        }
        notes.push((start, end, key as u8 + LOWEST_MIDI, mean(start, end, key)));
    }
    notes
}

/// The pitched notes in a stretch of a file (seconds from `start`), with Basic Pitch: fetched with Kumi's model
/// runtime the first time.
pub async fn pitched_notes(file: &Path, start: f64, seconds: f64, say: Say<'_>, signal: &Signal) -> Result<Vec<Heard>, String> {
    models::runtime(say, signal).await?;
    let model = models::dir().join(pinned::BASIC_PITCH.name);
    models::fetch(&pinned::BASIC_PITCH, &model, "its note model, Basic Pitch", say, signal).await?;
    let mut source = open_audio_to(file, Some(signal.clone()), Some(start + seconds + 1.)).await.map_err(|error| error.to_string())?;
    let native = source.sample_rate;
    source.seek(start * native);
    let wanted = (seconds * native).round() as usize;
    let mut mono: Vec<f32> = vec![];
    while mono.len() < wanted {
        let Some(chunk) = source.read((wanted - mono.len()).min(65_536)).await.map_err(|error| error.to_string())? else { break };
        let right = chunk.get(1).unwrap_or(&chunk[0]);
        mono.extend(chunk[0].iter().zip(right).map(|(left, right)| (left + right) / 2.));
    }
    let _ = source.close().await;
    if mono.iter().all(|sample| sample.abs() < 1e-4) {
        return Err("Nothing was heard to transcribe.".into());
    }
    // Resampled and, below, decoded off the app's thread.
    let audio = tokio::task::spawn_blocking(move || resample(&mono, native, RATE)).await.map_err(|error| error.to_string())?;
    // Overlapping windows, the first half an overlap in, as Basic Pitch slices them.
    let overlap = OVERLAP_FRAMES * HOP;
    let step = WINDOW - overlap;
    let mut padded = vec![0f32; overlap / 2];
    padded.extend_from_slice(&audio);
    let (mut frames, mut onsets): (Vec<Vec<f32>>, Vec<Vec<f32>>) = (vec![], vec![]);
    let keep = OVERLAP_FRAMES / 2;
    let mut at = 0;
    while at < padded.len() {
        if signal.is_cancelled() {
            return Err("stopped".into());
        }
        let mut window: Vec<f32> = padded[at..(at + WINDOW).min(padded.len())].to_vec();
        window.resize(WINDOW, 0.);
        let input = Tensor { shape: vec![1, WINDOW, 1], data: window };
        let out = models::run(
            &model,
            vec![("serving_default_input_2:0".into(), input)],
            vec!["StatefulPartitionedCall:1".into(), "StatefulPartitionedCall:2".into()],
        )
        .await?;
        let rows = |tensor: &Tensor| -> Vec<Vec<f32>> { tensor.data.chunks(KEYS).map(<[f32]>::to_vec).collect() };
        let (note, onset) = (rows(&out[0]), rows(&out[1]));
        let end = note.len().saturating_sub(keep);
        frames.extend(note[keep.min(end)..end].iter().cloned());
        onsets.extend(onset[keep.min(end)..end].iter().cloned());
        at += step;
    }
    // As many frames as the audio holds.
    let count = (audio.len() as f64 * (RATE / HOP as f64).floor() / RATE).floor() as usize;
    frames.truncate(count);
    onsets.truncate(count);
    tokio::task::spawn_blocking(move || {
        let mut heard: Vec<Heard> = notes_from(&frames, &onsets)
            .into_iter()
            .map(|(first, end, pitch, strength)| Heard { start: frame_time(first), end: frame_time(end), pitch, strength })
            .collect();
        heard.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
        heard
    })
    .await
    .map_err(|error| error.to_string())
}
