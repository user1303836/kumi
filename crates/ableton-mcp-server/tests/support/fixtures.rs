//! PCM generators. Each builds the `Float32Array` the
//! TypeScript did, so values are `f32`; `as_f64` widens them for the analysis, as reading a
//! `Float32Array` in JavaScript does.

#![allow(dead_code)]

use std::f64::consts::PI;

pub fn sine_fixture(length: usize, frequency: f64, sample_rate: f64, amplitude: f64) -> Vec<f32> {
    (0..length).map(|index| (amplitude * ((2.0 * PI * frequency * index as f64) / sample_rate).sin()) as f32).collect()
}

pub fn impulse_fixture(length: usize, amplitude: f64) -> Vec<f32> {
    let mut result = vec![0f32; length];
    if length > 0 {
        result[0] = amplitude as f32;
    }
    result
}

pub fn dc_fixture(length: usize, value: f64) -> Vec<f32> {
    vec![value as f32; length]
}

pub fn silence_fixture(length: usize) -> Vec<f32> {
    vec![0f32; length]
}

pub fn sweep_fixture(length: usize, start_hz: f64, end_hz: f64, sample_rate: f64, amplitude: f64) -> Vec<f32> {
    (0..length)
        .map(|index| {
            let progress = index as f64 / (length as f64 - 1.0).max(1.0);
            let frequency = start_hz + (end_hz - start_hz) * progress;
            (amplitude * ((2.0 * PI * frequency * index as f64) / sample_rate).sin()) as f32
        })
        .collect()
}

pub fn stereo_fixture(length: usize, left: impl Fn(usize) -> f64, right: impl Fn(usize) -> f64) -> Vec<f32> {
    let mut result = vec![0f32; length * 2];
    for frame in 0..length {
        result[frame * 2] = left(frame) as f32;
        result[frame * 2 + 1] = right(frame) as f32;
    }
    result
}

/// `Float32Array` values as the analysis reads them: widened to `f64`.
pub fn as_f64(samples: &[f32]) -> Vec<f64> {
    samples.iter().map(|value| *value as f64).collect()
}
