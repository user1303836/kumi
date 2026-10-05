//! Frames of one cycle each, written as a mono 32-bit WAV with Serum's "clm " chunk.

use super::decode::{open_audio, AudioError};
use kumi_common::js::number::round;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

pub const FRAME: usize = 2048;
pub const MAX_FRAMES: usize = 256;
const MAX_HARMONIC: usize = FRAME / 2 - 1;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    #[default]
    Sine,
    Saw,
    Square,
    Triangle,
    Pulse,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Keyframe {
    pub harmonics: Option<Vec<f64>>,
    pub shape: Option<Shape>,
    pub width: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FromAudio {
    pub file: String,
    pub start: Option<f64>,
    pub seconds: Option<f64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WavetableSpec {
    pub keyframes: Option<Vec<Keyframe>>,
    pub count: Option<f64>,
    pub from_audio: Option<FromAudio>,
}
/// A shape's harmonic amplitudes (signed, so a triangle's alternate).
pub fn shape_harmonics(shape: Shape, width: Option<f64>, count: Option<usize>) -> Vec<f64> {
    (1..=count.unwrap_or(256).min(MAX_HARMONIC))
        .map(|n| {
            let nf = n as f64;
            match shape {
                Shape::Sine => {
                    if n == 1 {
                        1.0
                    } else {
                        0.0
                    }
                }
                Shape::Saw => 1.0 / nf,
                Shape::Square => {
                    if n % 2 == 1 {
                        1.0 / nf
                    } else {
                        0.0
                    }
                }
                Shape::Triangle => {
                    if n % 2 == 1 {
                        (if (n - 1) / 2 % 2 == 1 { -1.0 } else { 1.0 }) / (nf * nf)
                    } else {
                        0.0
                    }
                }
                Shape::Pulse => 2.0 / (nf * PI) * (nf * PI * width.unwrap_or(0.5).clamp(0.01, 0.99)).sin(),
            }
        })
        .collect()
}
/// One cycle from harmonic amplitudes (sines, from phase 0).
pub fn synthesize(harmonics: &[f64]) -> Vec<f32> {
    let mut frame = vec![0f32; FRAME];
    for (index, amplitude) in harmonics.iter().take(MAX_HARMONIC).enumerate() {
        if *amplitude == 0.0 || amplitude.is_nan() {
            continue;
        }
        for (sample, value) in frame.iter_mut().enumerate() {
            *value = (*value as f64 + amplitude * (2.0 * PI * (index + 1) as f64 * sample as f64 / FRAME as f64).sin()) as f32;
        }
    }
    frame
}
fn harmonics_of(keyframe: &Keyframe) -> Vec<f64> {
    keyframe
        .harmonics
        .as_ref()
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| shape_harmonics(keyframe.shape.unwrap_or_default(), keyframe.width, None))
}
/// Frames morph by harmonics; normalized together so the sweep keeps its levels.
pub fn frames_from_keyframes(keyframes: &[Keyframe], count: Option<f64>) -> Result<Vec<Vec<f32>>, AudioError> {
    if keyframes.is_empty() {
        return Err(AudioError("A wavetable needs at least one keyframe.".into()));
    }
    let total = round(count.unwrap_or(keyframes.len() as f64)).clamp(1.0, MAX_FRAMES as f64) as usize;
    let spectra: Vec<_> = keyframes.iter().map(harmonics_of).collect();
    let mut frames = Vec::new();
    for index in 0..total {
        let at = if total == 1 || spectra.len() == 1 { 0.0 } else { index as f64 / (total - 1) as f64 * (spectra.len() - 1) as f64 };
        let left = at.floor() as usize;
        let right = (left + 1).min(spectra.len() - 1);
        let mix = at - left as f64;
        let length = spectra[left].len().max(spectra[right].len());
        let harmonics: Vec<_> =
            (0..length).map(|n| spectra[left].get(n).unwrap_or(&0.0) * (1.0 - mix) + spectra[right].get(n).unwrap_or(&0.0) * mix).collect();
        frames.push(synthesize(&harmonics));
    }
    Ok(normalize(frames))
}
/// Single cycles cut evenly from a sound, each stretched to a frame and started at a rising zero crossing.
pub async fn frames_from_audio(file: &str, count: f64, start: Option<f64>, seconds: Option<f64>) -> Result<Vec<Vec<f32>>, AudioError> {
    let mut source = open_audio(file, None).await?;
    let rate = source.sample_rate;
    let result = async {
        let from = (start.unwrap_or(0.0) * rate).floor().max(0.0) as usize;
        let length = (source.frames as f64 - from as f64).min((seconds.unwrap_or(8.0) * rate).floor());
        if length < 256.0 {
            return Err(AudioError("That part of the sound is too short to cut cycles from.".into()));
        }
        let length = length as usize;
        source.seek(from as f64);
        let mut mono = Vec::with_capacity(length);
        while mono.len() < length {
            let Some(chunk) = source.read(65536.min(length - mono.len())).await? else {
                break;
            };
            if chunk.is_empty() || chunk[0].is_empty() {
                break;
            }
            for index in 0..chunk[0].len() {
                mono.push((chunk.iter().map(|c| c[index] as f64).sum::<f64>() / chunk.len() as f64) as f32);
            }
        }
        Ok(mono)
    }
    .await;
    source.close().await?;
    let mono = result?;
    let period = period_of(&mono, rate)
        .ok_or_else(|| AudioError("Kumi couldn't hear a steady pitch in that sound to cut single cycles from; use a held note.".into()))?;
    let total = round(count).clamp(1.0, MAX_FRAMES as f64) as usize;
    let usable = mono.len() as f64 - period.ceil() * 2.0;
    let mut frames = Vec::new();
    for index in 0..total {
        let mut at = (usable * index as f64 / total.saturating_sub(1).max(1) as f64).floor() as i64;
        let mut look = 0;
        while (look as f64) < period && at + 1 < mono.len() as i64 {
            if sample(&mono, at) <= 0.0 && sample(&mono, at + 1) > 0.0 {
                break;
            }
            look += 1;
            at += 1;
        }
        let frame = (0..FRAME)
            .map(|sample_index| {
                let position = at as f64 + sample_index as f64 * period / FRAME as f64;
                let whole = position.floor() as i64;
                let fraction = position - whole as f64;
                (sample(&mono, whole) * (1.0 - fraction) + sample(&mono, whole + 1) * fraction) as f32
            })
            .collect();
        frames.push(frame);
    }
    Ok(normalize(frames))
}
fn sample(mono: &[f32], index: i64) -> f64 {
    if index < 0 {
        0.0
    } else {
        mono.get(index as usize).copied().unwrap_or(0.0) as f64
    }
}
/// A held note's period in samples, by autocorrelation over 30 Hz–2 kHz.
pub fn period_of(signal: &[f32], sample_rate: f64) -> Option<f64> {
    let from = signal.len() / 4;
    let window = &signal[from..signal.len().min(from + 8192)];
    if window.is_empty() {
        return None;
    }
    let min_lag = (sample_rate / 2000.0).floor() as usize;
    let max_lag = (window.len() - 1).min((sample_rate / 30.0).floor() as usize);
    let energy = window.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    if energy < 1e-9 {
        return None;
    }
    let (mut best, mut best_lag) = (0.0, 0);
    let mut correlations = vec![0.0; max_lag + 2];
    for lag in min_lag..=max_lag {
        let sum = (0..window.len() - lag).map(|i| window[i] as f64 * window[i + lag] as f64).sum::<f64>();
        correlations[lag] = sum / energy;
        if correlations[lag] > best {
            best = correlations[lag];
            best_lag = lag;
        }
    }
    if best < 0.3 || best_lag == 0 {
        return None;
    }
    for lag in min_lag..best_lag {
        if lag > 0
            && correlations[lag] > best * 0.9
            && correlations[lag] >= correlations[lag - 1]
            && correlations[lag] >= correlations[lag + 1]
        {
            best_lag = lag;
            break;
        }
    }
    let (a, b, c) = (correlations[best_lag - 1], correlations[best_lag], correlations[best_lag + 1]);
    let den = 2.0 * (a - 2.0 * b + c);
    let shift = (a - c) / if den == 0.0 { 1.0 } else { den };
    Some(best_lag as f64 + if shift.is_finite() && shift.abs() < 1.0 { shift } else { 0.0 })
}
fn normalize(mut frames: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    let peak = frames.iter().flatten().map(|v| (*v as f64).abs()).fold(0.0_f64, |peak, sample| {
        if peak.is_nan() || sample.is_nan() {
            f64::NAN
        } else {
            peak.max(sample)
        }
    });
    if peak > 0.0 {
        for value in frames.iter_mut().flatten() {
            *value = (*value as f64 / (peak / 0.99)) as f32;
        }
    }
    frames
}
/// Mono 32-bit float WAV at 44.1 kHz, with Serum's marker saying the frame size.
pub async fn write_wavetable(file: impl AsRef<std::path::Path>, frames: &[Vec<f32>]) -> Result<(), AudioError> {
    let samples: usize = frames.iter().map(Vec::len).sum();
    let marker = format!("<!>{FRAME} 00000000 wavetable (Kumi)");
    let mut body = Vec::new();
    body.extend(b"WAVEfmt ");
    body.extend(16u32.to_le_bytes());
    body.extend(3u16.to_le_bytes());
    body.extend(1u16.to_le_bytes());
    body.extend(44100u32.to_le_bytes());
    body.extend((44100u32 * 4).to_le_bytes());
    body.extend(4u16.to_le_bytes());
    body.extend(32u16.to_le_bytes());
    body.extend(b"clm ");
    body.extend((marker.len() as u32).to_le_bytes());
    body.extend(marker.as_bytes());
    if marker.len() % 2 == 1 {
        body.push(0);
    }
    body.extend(b"data");
    body.extend((samples as u32 * 4).to_le_bytes());
    for value in frames.iter().flatten() {
        body.extend(value.to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    tokio::fs::write(file, out).await?;
    Ok(())
}
pub async fn build_wavetable(spec: &WavetableSpec) -> Result<Vec<Vec<f32>>, AudioError> {
    if let Some(audio) = &spec.from_audio {
        frames_from_audio(&audio.file, spec.count.unwrap_or(64.0), audio.start, audio.seconds).await
    } else {
        frames_from_keyframes(
            spec.keyframes.as_deref().unwrap_or(&[Keyframe { shape: Some(Shape::Saw), ..Default::default() }]),
            spec.count,
        )
    }
}
