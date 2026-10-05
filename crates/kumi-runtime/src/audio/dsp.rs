//! The signal processing Kumi's listening is built from: FFT, windows, filters, conversions.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

struct Table {
    cos: Vec<f64>,
    sin: Vec<f64>,
    reverse: Vec<usize>,
}
static TABLES: LazyLock<Mutex<HashMap<usize, Arc<Table>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn table(size: usize) -> Arc<Table> {
    let mut tables = TABLES.lock().unwrap();
    tables
        .entry(size)
        .or_insert_with(|| {
            assert!(size >= 2 && size.is_power_of_two(), "FFT size must be a power of two");
            let bits = size.ilog2();
            Arc::new(Table {
                cos: (0..size / 2).map(|i| (-2.0 * PI * i as f64 / size as f64).cos()).collect(),
                sin: (0..size / 2).map(|i| (-2.0 * PI * i as f64 / size as f64).sin()).collect(),
                reverse: (0..size).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect(),
            })
        })
        .clone()
}
/// In-place complex FFT (radix 2); `re` and `im` have a power-of-two length.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let size = re.len();
    let table = table(size);
    for index in 0..size {
        let other = table.reverse[index];
        if other > index {
            re.swap(index, other);
            im.swap(index, other);
        }
    }
    let mut length = 2;
    while length <= size {
        let half = length / 2;
        let stride = size / length;
        for start in (0..size).step_by(length) {
            for offset in 0..half {
                let wr = table.cos[offset * stride];
                let wi = table.sin[offset * stride];
                let a = start + offset;
                let b = a + half;
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        length *= 2;
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Default)]
pub enum Window {
    #[default]
    Hann,
    BlackmanHarris,
}
type WindowCache = HashMap<(usize, Window), Arc<Vec<f64>>>;
static WINDOWS: LazyLock<Mutex<WindowCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// A Hann window (or Blackman–Harris, for finding partials), normalized so its sum is 1.
pub fn window(size: usize, kind: Window) -> Arc<Vec<f64>> {
    WINDOWS
        .lock()
        .unwrap()
        .entry((size, kind))
        .or_insert_with(|| {
            let mut values: Vec<_> = (0..size)
                .map(|index| {
                    let phase = 2.0 * PI * index as f64 / (size as f64 - 1.0);
                    match kind {
                        Window::Hann => 0.5 - 0.5 * phase.cos(),
                        Window::BlackmanHarris => {
                            0.35875 - 0.48829 * phase.cos() + 0.14128 * (2.0 * phase).cos() - 0.01168 * (3.0 * phase).cos()
                        }
                    }
                })
                .collect();
            let sum: f64 = values.iter().sum();
            for value in &mut values {
                *value /= sum;
            }
            Arc::new(values)
        })
        .clone()
}
/// A biquad filter (transposed direct form II) with its own state.
#[derive(Clone, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}
impl Biquad {
    pub fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self { b0, b1, b2, a1, a2, z1: 0.0, z2: 0.0 }
    }
    pub fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }
}
/// ITU-R BS.1770's K-weighting at any sample rate, from the analog prototypes.
pub fn k_weighting(sample_rate: f64) -> [Biquad; 2] {
    let k = (PI * 1681.974450955533 / sample_rate).tan();
    let q = 0.7071752369554196;
    let vh = 10f64.powf(3.999843853973347 / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    let k = (PI * 38.13547087602444 / sample_rate).tan();
    let q = 0.5003270373238773;
    let a0 = 1.0 + k / q + k * k;
    [shelf, Biquad::new(1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0)]
}
static PHASES: LazyLock<[[f64; 12]; 4]> = LazyLock::new(|| {
    std::array::from_fn(|phase| {
        std::array::from_fn(|tap| {
            let x = tap as f64 - 6.0 + 1.0 - phase as f64 / 4.0;
            let sinc = if x == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
            sinc * (0.5 + 0.5 * (PI * x / 6.0).cos())
        })
    })
});
/// 4× oversampling for true peak (BS.1770 annex 2), as a windowed-sinc polyphase filter.
#[derive(Default, Clone)]
pub struct TruePeak {
    // Mirror the ring so the latest twelve samples are always one contiguous slice.
    history: [f64; 24],
    at: usize,
}
impl TruePeak {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, sample: f64) {
        self.history[self.at] = sample;
        self.history[self.at + 12] = sample;
        self.at = (self.at + 1) % 12;
    }
    pub fn peak(&self) -> f64 {
        let history: &[f64; 12] = self.history[self.at..self.at + 12].try_into().unwrap();
        // Fixed tap accesses remove per-sample ring arithmetic and bounds checks.
        // Keep the source's tap order and separate additions for identical rounding.
        let dot = |coefficients: &[f64; 12]| {
            let mut sum = 0.0_f64;
            sum += coefficients[0] * history[11];
            sum += coefficients[1] * history[10];
            sum += coefficients[2] * history[9];
            sum += coefficients[3] * history[8];
            sum += coefficients[4] * history[7];
            sum += coefficients[5] * history[6];
            sum += coefficients[6] * history[5];
            sum += coefficients[7] * history[4];
            sum += coefficients[8] * history[3];
            sum += coefficients[9] * history[2];
            sum += coefficients[10] * history[1];
            sum += coefficients[11] * history[0];
            sum.abs()
        };
        let phases = &*PHASES;
        dot(&phases[0]).max(dot(&phases[1])).max(dot(&phases[2])).max(dot(&phases[3]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_true_peak_taps_match_the_reference_ring_at_every_position() {
        let mut peak = TruePeak::new();
        let mut history = [0.0; 12];
        let mut at = 0;
        let mut seed = 123456789_u32;
        assert_eq!(peak.peak(), 0.0);
        for index in 0..10_000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let sample = match index % 5 {
                0 => 0.0,
                1 => 1.0,
                2 => -1.0,
                3 => f64::MIN_POSITIVE,
                _ => seed as f64 / u32::MAX as f64 * 2.0 - 1.0,
            };
            history[at] = sample;
            at = (at + 1) % 12;
            peak.push(sample);
            // Direct convolution, independent of the mirrored-ring layout.
            let mut expected = 0.0_f64;
            for coefficients in PHASES.iter() {
                let mut sum = 0.0;
                for tap in 0..12 {
                    sum += coefficients[tap] * history[(at + 12 - 1 - tap) % 12];
                }
                expected = expected.max(sum.abs());
            }
            assert_eq!(peak.peak().to_bits(), expected.to_bits(), "sample {index}, ring position {at}");
            if index % 12 == 0 {
                peak = peak.clone();
            }
        }
    }
}

pub fn db(power: f64) -> f64 {
    if power > 1e-20 {
        10.0 * power.log10()
    } else {
        -200.0
    }
}
pub fn db_amplitude(amplitude: f64) -> f64 {
    if amplitude > 1e-10 {
        20.0 * amplitude.log10()
    } else {
        -200.0
    }
}
pub fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    kumi_common::js::number::round(value * scale) / scale
}
pub const PITCH_CLASSES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub name: String,
    pub midi: f64,
    pub cents: f64,
}
pub fn note_of(hz: f64) -> Note {
    let exact = 69.0 + 12.0 * (hz / 440.0).log2();
    let midi = kumi_common::js::number::round(exact);
    Note {
        name: format!("{}{}", PITCH_CLASSES[(midi as i64).rem_euclid(12) as usize], (midi / 12.0).floor() as i64 - 1),
        midi,
        cents: kumi_common::js::number::round((exact - midi) * 100.0),
    }
}
/// The value at fraction `p` (0…1) of the sorted values.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = ((sorted.len() - 1) as f64 * p).clamp(0.0, (sorted.len() - 1) as f64);
    let low = at.floor() as usize;
    let high = at.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (at - low as f64)
}
pub fn clock(seconds: f64) -> String {
    format!("{}:{:02}", (seconds / 60.0).floor() as i64, (seconds % 60.0).floor() as i64)
}
