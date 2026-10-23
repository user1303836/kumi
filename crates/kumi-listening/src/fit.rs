//! Calculating an EQ instead of trying one: an EQ band changes the spectrum by its known curve, so code fits bands to
//! a gap directly (with limits on gain, width and frequency) and predicts what a change does to a measured spectrum
//! before anything is heard. Bands are the usual digital EQ shapes (bell and shelves, as EQ Eight draws them).

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    Bell,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub shape: Shape,
    pub hz: f64,
    pub db: f64,
    pub q: f64,
}

/// Normalized biquad coefficients: b0, b1, b2, a1, a2.
fn coefficients(band: &Band, rate: f64) -> [f64; 5] {
    let a = 10f64.powf(band.db / 40.);
    let w = 2. * PI * band.hz.clamp(10., rate * 0.49) / rate;
    let (sin, cos) = w.sin_cos();
    let alpha = sin / (2. * band.q.max(0.05));
    let (b0, b1, b2, a0, a1, a2) = match band.shape {
        Shape::Bell => (1. + alpha * a, -2. * cos, 1. - alpha * a, 1. + alpha / a, -2. * cos, 1. - alpha / a),
        Shape::LowShelf => {
            let root = 2. * a.sqrt() * alpha;
            (
                a * ((a + 1.) - (a - 1.) * cos + root),
                2. * a * ((a - 1.) - (a + 1.) * cos),
                a * ((a + 1.) - (a - 1.) * cos - root),
                (a + 1.) + (a - 1.) * cos + root,
                -2. * ((a - 1.) + (a + 1.) * cos),
                (a + 1.) + (a - 1.) * cos - root,
            )
        }
        Shape::HighShelf => {
            let root = 2. * a.sqrt() * alpha;
            (
                a * ((a + 1.) + (a - 1.) * cos + root),
                -2. * a * ((a - 1.) + (a + 1.) * cos),
                a * ((a + 1.) + (a - 1.) * cos - root),
                (a + 1.) - (a - 1.) * cos + root,
                2. * ((a - 1.) - (a + 1.) * cos),
                (a + 1.) - (a - 1.) * cos - root,
            )
        }
    };
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// What a band does at a frequency, dB.
pub fn response(band: &Band, hz: f64, rate: f64) -> f64 {
    let [b0, b1, b2, a1, a2] = coefficients(band, rate);
    let w = 2. * PI * hz / rate;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2. * w).cos(), (2. * w).sin());
    let (nr, ni) = (b0 + b1 * c1 + b2 * c2, -(b1 * s1 + b2 * s2));
    let (dr, di) = (1. + a1 * c1 + a2 * c2, -(a1 * s1 + a2 * s2));
    10. * ((nr * nr + ni * ni) / (dr * dr + di * di)).max(1e-30).log10()
}

/// What several bands do at a frequency, dB.
pub fn total(bands: &[Band], hz: f64, rate: f64) -> f64 {
    bands.iter().map(|band| response(band, hz, rate)).sum()
}

/// Runs a channel through a band, in place (for predicting what a change does, and for tests).
pub fn filter(samples: &mut [f32], band: &Band, rate: f64) {
    let [b0, b1, b2, a1, a2] = coefficients(band, rate);
    let (mut x1, mut x2, mut y1, mut y2) = (0f64, 0f64, 0f64, 0f64);
    for sample in samples.iter_mut() {
        let x = *sample as f64;
        let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        (x2, x1, y2, y1) = (x1, x, y1, y);
        *sample = y as f32;
    }
}

/// How far a fit may go.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// The most any band cuts or boosts, dB.
    pub db: f64,
    pub q: (f64, f64),
    pub hz: (f64, f64),
    pub bands: usize,
    /// Stop once no point is further off than this, dB.
    pub within: f64,
}

pub const MASTER_LIMITS: Limits = Limits { db: 6., q: (0.3, 8.), hz: (20., 20_000.), bands: 4, within: 0.75 };

/// Bands that bring a curve toward a gap: `gap` holds (frequency, dB wanted, weight). Greedy, the biggest error
/// first: a band starts there (a shelf at the ends, a bell between), then every band's frequency, gain and width
/// are refined together. Bands that do less than half a dB anywhere are dropped.
pub fn fit(gap: &[(f64, f64, f64)], limits: &Limits, rate: f64) -> Vec<Band> {
    let mut bands: Vec<Band> = vec![];
    let error = |bands: &[Band]| -> f64 { gap.iter().map(|(hz, want, weight)| weight * (want - total(bands, *hz, rate)).powi(2)).sum() };
    while bands.len() < limits.bands {
        let worst = gap
            .iter()
            .map(|(hz, want, weight)| (*hz, want - total(&bands, *hz, rate), *weight))
            .max_by(|a, b| (a.1.abs() * a.2).total_cmp(&(b.1.abs() * b.2)));
        let Some((hz, off, _)) = worst else { break };
        if off.abs() <= limits.within {
            break;
        }
        let lowest = gap.iter().map(|point| point.0).fold(f64::MAX, f64::min);
        let highest = gap.iter().map(|point| point.0).fold(f64::MIN, f64::max);
        let shape = if hz <= lowest * 1.3 && hz < 150. {
            Shape::LowShelf
        } else if hz >= highest / 1.3 && hz > 6000. {
            Shape::HighShelf
        } else {
            Shape::Bell
        };
        bands.push(Band {
            shape,
            hz: hz.clamp(limits.hz.0, limits.hz.1),
            db: off.clamp(-limits.db, limits.db),
            q: if shape == Shape::Bell { 1.4 } else { 0.7 },
        });
        refine(&mut bands, limits, &error);
    }
    bands.retain(|band| gap.iter().any(|(hz, _, _)| response(band, *hz, rate).abs() >= 0.5));
    bands
}

/// Coordinate descent on every band's log-frequency, gain and log-width, within the limits.
fn refine(bands: &mut [Band], limits: &Limits, error: &dyn Fn(&[Band]) -> f64) {
    let mut best = error(bands);
    let mut steps = [0.25f64, 1.0, 0.25];
    for _ in 0..40 {
        let mut moved = false;
        for index in 0..bands.len() {
            for (axis, step) in steps.iter().enumerate() {
                for direction in [-1., 1.] {
                    let mut trial = bands.to_vec();
                    let band = &mut trial[index];
                    match axis {
                        0 => band.hz = (band.hz * 2f64.powf(direction * step)).clamp(limits.hz.0, limits.hz.1),
                        1 => band.db = (band.db + direction * step).clamp(-limits.db, limits.db),
                        _ => band.q = (band.q * 2f64.powf(direction * step)).clamp(limits.q.0, limits.q.1),
                    }
                    let tried = error(&trial);
                    if tried < best - 1e-9 {
                        best = tried;
                        bands.copy_from_slice(&trial);
                        moved = true;
                    }
                }
            }
        }
        if !moved {
            steps.iter_mut().for_each(|step| *step /= 2.);
            if steps[1] < 0.05 {
                break;
            }
        }
    }
}
