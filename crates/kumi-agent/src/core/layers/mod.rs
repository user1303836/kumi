//! Layering breaks (#401), built once for every DAW: each layer set against the main break hit by hit (`compare`): how early or
//! late its hits land (milliseconds, the median and the hits that stray from it), whether its polarity fights the main's (the
//! first milliseconds of matched hits correlating negatively), and how loud it sits under it; and the glue measured: how much
//! peak-to-loudness ratio (`plr`) the group's compressor takes, the group heard with it and without. The tool (`tool`)
//! renders, says it, and nudges, inverts and glues as asked; the DAW's backend (`backend`) renders and writes.
pub mod backend;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;

use crate::core::chop::hits::{hits, Hit};

/// Hits further apart than this aren't the same hit (seconds).
const PAIR: f64 = 0.04;
/// A hit this far from the layer's median offset strays (ms).
const STRAY_MS: f64 = 3.;
/// How much of a hit's start its polarity is read from (seconds).
const ATTACK: f64 = 0.005;

/// One layer against the main: the hits paired, its median offset and the hits that stray from it, how many pairs read as
/// inverted, and its loudness against the main's (dB).
#[derive(Debug, Clone, PartialEq)]
pub struct Compared {
    pub paired: usize,
    pub offset_ms: Option<f64>,
    pub strays: Vec<(f64, f64)>,
    pub inverted: usize,
    pub level_db: Option<f64>,
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0., 0., 0.);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x as f64, *y as f64);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa <= 0. || bb <= 0. {
        0.
    } else {
        ab / (aa * bb).sqrt()
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

/// The integrated loudness of a mono render (LUFS), None when it's too quiet or short to tell.
pub fn loudness(mono: &[f32], rate: f64) -> Option<f64> {
    kumi_listening::measure::measure_samples(mono, mono, rate).measures.integrated
}

/// A layer (`layer`, mono at `rate`) against the main (`main`, the same rate and stretch).
pub fn compare(main: &[f32], layer: &[f32], rate: f64) -> Compared {
    let (main_hits, layer_hits) = (hits(main, rate), hits(layer, rate));
    let span = (ATTACK * rate) as usize;
    let mut offsets = Vec::new();
    let mut pairs: Vec<(&Hit, &Hit)> = Vec::new();
    for hit in &main_hits {
        let nearest = layer_hits.iter().min_by(|a, b| (a.seconds - hit.seconds).abs().total_cmp(&(b.seconds - hit.seconds).abs()));
        if let Some(other) = nearest.filter(|other| (other.seconds - hit.seconds).abs() <= PAIR) {
            offsets.push((other.seconds - hit.seconds) * 1000.);
            pairs.push((hit, other));
        }
    }
    let offset_ms = median(&mut offsets.clone());
    let strays = pairs
        .iter()
        .filter_map(|(hit, other)| {
            let off = (other.seconds - hit.seconds) * 1000.;
            offset_ms.filter(|median| (off - median).abs() > STRAY_MS).map(|_| (hit.seconds, off))
        })
        .collect();
    let inverted = pairs
        .iter()
        .filter(|(hit, other)| {
            let (a, b) = (hit.frame, other.frame);
            a + span <= main.len() && b + span <= layer.len() && correlation(&main[a..a + span], &layer[b..b + span]) < -0.2
        })
        .count();
    let level_db = loudness(layer, rate).zip(loudness(main, rate)).map(|(layer, main)| layer - main);
    Compared { paired: pairs.len(), offset_ms, strays, inverted, level_db }
}

/// A render's peak-to-loudness ratio and its true peak (dB), None when it's too quiet or short to tell; the glue a compressor
/// adds is how much ratio it takes.
pub fn plr(mono: &[f32], rate: f64) -> Option<(f64, f64)> {
    let measures = kumi_listening::measure::measure_samples(mono, mono, rate).measures;
    measures.integrated.map(|integrated| (measures.true_peak - integrated, measures.true_peak))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clicks with a decaying tone each `every` seconds from `from`, `sign` their polarity.
    fn clicks(rate: f64, seconds: f64, every: f64, from: f64, sign: f32, level: f32) -> Vec<f32> {
        let mut out = vec![0f32; (rate * seconds) as usize];
        let mut at = from;
        while at < seconds - 0.2 {
            let start = (at * rate) as usize;
            for k in 0..(0.15 * rate) as usize {
                let t = k as f32 / rate as f32;
                out[start + k] += sign * level * (-t * 30.).exp() * (2. * std::f32::consts::PI * 80. * t).sin().max(-1.)
                    + sign * level * (-t * 400.).exp();
            }
            at += every;
        }
        out
    }

    #[test]
    fn a_late_quiet_layer_with_its_polarity_flipped_is_told_so() {
        let rate = 48_000.;
        let main = clicks(rate, 4., 0.5, 0.1, 1., 0.8);
        let layer = clicks(rate, 4., 0.5, 0.104, -1., 0.2);
        let found = compare(&main, &layer, rate);
        assert!(found.paired >= 6, "{found:?}");
        assert!((found.offset_ms.unwrap() - 4.).abs() < 1.5, "{found:?}");
        assert!(found.inverted * 2 > found.paired, "{found:?}");
        assert!(found.level_db.unwrap() < -9., "{found:?}");
        // In time and in polarity, as loud: nothing to say.
        let same = compare(&main, &main, rate);
        assert_eq!((same.offset_ms, same.inverted), (Some(0.), 0));
        assert!(same.level_db.unwrap().abs() < 0.01);
    }

    #[test]
    fn a_squashed_copy_has_less_peak_to_loudness_ratio() {
        let rate = 48_000.;
        let sum = clicks(rate, 4., 0.5, 0.1, 1., 0.8);
        // (a squashed copy: its peaks cut by half where the tone stays)
        let squashed: Vec<f32> = sum.iter().map(|s| s.clamp(-0.4, 0.4)).collect();
        let (open, peak) = plr(&sum, rate).unwrap();
        assert!(open - plr(&squashed, rate).unwrap().0 > 1., "{open}");
        assert!(peak > -3. && peak < 1., "{peak}");
        assert_eq!(plr(&vec![0.; 48_000], rate), None, "silence");
    }
}
