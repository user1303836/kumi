//! The loop's measuring ears: one streaming pass over a mix, a stem or a master gives loudness and peaks (true
//! peak at 8×, PLR, PSR, loudness range), tonal balance and width in third-octaves, punch, pumping, distortion
//! and the low end, plus what the detectors place in time: each frame's bands and fine spectrum, and the bass
//! notes. Everything a goal checklist compares is level-independent or compared level-matched.

use crate::audio::{
    decode::{open_audio_to, AudioError, AudioSource},
    dsp::{fft, k_weighting, Biquad},
};
use kumi_common::abort::{Signal, SignalExt};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// ISO third-octave centers, 20 Hz to 20 kHz.
pub const THIRDS: [f64; 31] = [
    20., 25., 31.5, 40., 50., 63., 80., 100., 125., 160., 200., 250., 315., 400., 500., 630., 800., 1000., 1250., 1600., 2000., 2500.,
    3150., 4000., 5000., 6300., 8000., 10000., 12500., 16000., 20000.,
];
/// The bands the low stream measures (20–100 Hz); the main stream's FFT is too coarse there.
pub const LOW_BANDS: usize = 8;
/// The fine spectrum's bins: twelfths of an octave from 250 Hz, six octaves.
pub const FINE_FROM: f64 = 250.;
pub const FINE_BINS: usize = 72;
/// The level envelope's step, seconds.
pub const ENVELOPE_HOP: f64 = 0.005;
/// How high the low stream's own bins are kept, Hz: past the fine spectrum's first bins, for a partial's window around
/// 250 Hz.
pub const LOW_FINE_TOP: f64 = 280.;
/// A fine bin's center, in Hz.
pub fn fine_hz(bin: usize) -> f64 {
    FINE_FROM * 2f64.powf(bin as f64 / 12.)
}
/// A third-octave band's edges.
pub fn band_edges(band: usize) -> (f64, f64) {
    (THIRDS[band] * 2f64.powf(-1. / 6.), THIRDS[band] * 2f64.powf(1. / 6.))
}
/// The frequencies a third-octave band gathers: from halfway (in octaves) to the centre under it to halfway to the
/// one over it, so every frequency falls in exactly one band. The ISO centres are rounded: their own sixth of an
/// octave either side leaves gaps, where a partial at 141 Hz would go uncounted.
pub fn band_span(band: usize) -> (f64, f64) {
    let (lowest, highest) = band_edges(band);
    let low = if band == 0 { lowest } else { (THIRDS[band - 1] * THIRDS[band]).sqrt() };
    let high = THIRDS.get(band + 1).map_or(highest, |next| (THIRDS[band] * next).sqrt());
    (low, high)
}

/// What one listen measured: what the checklist reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measures {
    pub seconds: f64,
    pub sample_rate: f64,
    /// Integrated loudness (BS.1770-4), LUFS.
    pub integrated: Option<f64>,
    pub short_term_max: Option<f64>,
    pub momentary_max: Option<f64>,
    /// Loudness range (EBU 3342), LU.
    pub range: Option<f64>,
    /// True peak at 8× oversampling, dBTP.
    pub true_peak: f64,
    pub sample_peak: f64,
    /// Peak to loudness ratio: the highest true peak over the integrated loudness, dB.
    pub plr: Option<f64>,
    /// Peak to short-term loudness ratio at the densest loud moment, dB: how squashed the loud parts are.
    pub psr: Option<f64>,
    /// Samples at or past full scale.
    pub clipped: usize,
    /// When they came (seconds from the start of what was measured), moments under half a second apart as one: the
    /// first five.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clipped_at: Vec<[f64; 2]>,
    /// When the highest true peak came, in seconds from the start of what was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_at: Option<f64>,
    /// Each third-octave's share of the whole, dB (level-independent).
    pub balance: Vec<f64>,
    /// Each third-octave's side share: 0 is mono, 0.5 as much side as mid.
    pub width: Vec<f64>,
    /// The balance's slope from 100 Hz to 10 kHz, dB per octave (0 is pink).
    pub tilt: f64,
    /// Punch: the median crest (peak over RMS in 400 ms) of the louder half, dB.
    pub crest: Option<f64>,
    /// Pumping: how far the mid band's level swings at 0.5–6 Hz around its trend, dB.
    pub pumping: Option<f64>,
    /// Distortion: how much brighter the loudest moments are than middling ones, dB.
    pub distortion: Option<f64>,
    /// Side below 120 Hz against mid, dB (−30 and lower is mono).
    pub low_width: Option<f64>,
    /// Below 30 Hz against 30–120 Hz, dB.
    pub rumble: Option<f64>,
    pub dc: [f64; 2],
    /// Short-term loudness every second, LUFS.
    pub short_term: Vec<Option<f64>>,
    /// A sound's envelope over its hits (medians): attack from a tenth to nine tenths of the peak, ms; decay from the
    /// peak to 20 dB under it, ms; and where it settles a quarter second after the peak, dB under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain: Option<f64>,
    /// Brightness: the spectral centroid of the loud frames, Hz (median).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centroid: Option<f64>,
    /// Noisiness: the spectral flatness of the loud frames, dB (0 is noise, far under it a pure tone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<f64>,
    /// How far a low sound's pitch falls over its first quarter second, semitones (an 808's or a kick's drop; median
    /// over its hits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_drop: Option<f64>,
    /// How often its level swings (a tremolo, an LFO, beating between detuned voices), Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulation: Option<f64>,
    /// Each hit's waveform crest: its sample peak over its RMS in the 20 ms around the hit's peak, dB (the median over
    /// its hits). It reads the hit alone, so tails, reverb and how often it plays don't move it; saturation and
    /// clipping flatten it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_crest: Option<f64>,
    /// Notes that start with a jump from silence (a click at the note's edge).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub clicks: usize,
    /// Crackle: lone spikes a second in the quiet stretches (dust).
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub crackle: f64,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}
fn is_nothing(value: &f64) -> bool {
    *value == 0.
}

/// What the detectors read, in time: per frame (every `hop` seconds) each third-octave's mid and side power, the
/// fine spectrum (dB of power density), and the frame's level; per low frame, the strongest bass note.
#[derive(Debug, Clone, Default)]
pub struct Frames {
    pub hop: f64,
    pub mid: Vec<[f32; 31]>,
    pub side: Vec<[f32; 31]>,
    pub fine: Vec<[f32; FINE_BINS]>,
    /// The frame's total power, dB.
    pub level: Vec<f32>,
    pub bass_hop: f64,
    /// The strongest pitch from 30 to 250 Hz in each low frame: its frequency and level (dB), when there's one.
    pub bass: Vec<Option<(f32, f32)>>,
    /// Each low frame's mid power per bin up to `LOW_FINE_TOP` (the low stream's own resolution, `low_bin_hz` apart): a
    /// low sound's partials, read one by one.
    pub low_fine: Vec<Vec<f32>>,
    pub low_bin_hz: f64,
    /// The level every 5 ms (`ENVELOPE_HOP`): the RMS of mid and side over the 20 ms around it, linear. Echoes,
    /// decays and swings are read off it.
    pub envelope: Vec<f32>,
}

impl Frames {
    /// A frame's time, in seconds from the start of what was measured.
    pub fn time(&self, frame: usize) -> f64 {
        frame as f64 * self.hop
    }
}

/// A listen's measures and the frames behind them, and what the learned models made of it when they heard it.
#[derive(Debug, Clone)]
pub struct Heard {
    pub measures: Measures,
    pub frames: Frames,
    pub embedding: Option<Embedding>,
}

/// What the learned models made of a listen: the style model's embedding (CLAP) and the effects model's (AFx-Rep).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embedding {
    pub vibe: Option<Vec<f32>>,
    /// Which style model made `vibe` (`embed::style_id`): it compares only with a vector the same model made.
    pub vibe_model: Option<String>,
    pub effects: Option<Vec<f32>>,
}

impl Heard {
    /// The same sound `db` louder (or quieter), as a fader after it would make it: its frames scaled (the measures
    /// stay as heard). A track heard before its fader, as the mix hears it.
    pub fn gained(mut self, db: f64) -> Heard {
        let (power, db) = (10f32.powf(db as f32 / 10.), db as f32);
        for bands in self.frames.mid.iter_mut().chain(self.frames.side.iter_mut()) {
            bands.iter_mut().for_each(|value| *value *= power);
        }
        for row in self.frames.fine.iter_mut() {
            row.iter_mut().for_each(|value| *value += db);
        }
        self.frames.level.iter_mut().for_each(|value| *value += db);
        for (_, level) in self.frames.bass.iter_mut().flatten() {
            *level += db;
        }
        for row in self.frames.low_fine.iter_mut() {
            row.iter_mut().for_each(|value| *value *= power);
        }
        let amplitude = 10f32.powf(db / 20.);
        self.frames.envelope.iter_mut().for_each(|value| *value *= amplitude);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct MeasureOptions {
    pub start: Option<f64>,
    pub seconds: Option<f64>,
    pub signal: Option<Signal>,
}

/// Measures a file (any format Kumi reads).
pub async fn measure_file(path: &str, options: MeasureOptions) -> Result<Heard, AudioError> {
    let until = options.start.unwrap_or(0.).max(0.) + options.seconds.unwrap_or(crate::audio::analyze::LONGEST) + 1.;
    let mut source = open_audio_to(path, options.signal.clone(), Some(until)).await?;
    let result = measure_source(&mut source, &options).await;
    source.close().await?;
    result
}

async fn measure_source(source: &mut AudioSource, options: &MeasureOptions) -> Result<Heard, AudioError> {
    let rate = source.sample_rate;
    if !(8000.0..=384000.0).contains(&rate) {
        return Err(AudioError(format!("A sample rate of {rate} Hz isn't one Kumi can measure.")));
    }
    let start = options.start.unwrap_or(0.).max(0.);
    let total = source.frames as f64 / rate;
    let length = options.seconds.unwrap_or(crate::audio::analyze::LONGEST).min(total - start);
    if !(length > 0.05) {
        return Err(AudioError("There's no audio in that part of the file.".into()));
    }
    source.seek(start * rate);
    let frames = (length * rate).floor() as usize;
    let mut meter = Meter::new(rate);
    let mut done = 0;
    while done < frames {
        if let Some(signal) = &options.signal {
            signal.check()?;
        }
        let Some(block) = source.read(65536.min(frames - done)).await? else { break };
        let left = &block[0];
        let right = block.get(1).unwrap_or(left);
        meter.push(left, right);
        done += left.len();
        tokio::task::yield_now().await;
    }
    Ok(meter.finish())
}

/// Measures samples in memory (tests, and audio Kumi made itself).
pub fn measure_samples(left: &[f32], right: &[f32], rate: f64) -> Heard {
    let mut meter = Meter::new(rate);
    meter.push(left, right);
    meter.finish()
}

/// A Butterworth low pass or high pass section (RBJ cookbook, Q 0.7071).
fn section(rate: f64, hz: f64, high: bool, q: f64) -> Biquad {
    let w = 2. * PI * hz / rate;
    let alpha = w.sin() / (2. * q);
    let cos = w.cos();
    let a0 = 1. + alpha;
    let (b0, b1, b2) = if high { ((1. + cos) / 2., -(1. + cos), (1. + cos) / 2.) } else { ((1. - cos) / 2., 1. - cos, (1. - cos) / 2.) };
    Biquad::new(b0 / a0, b1 / a0, b2 / a0, -2. * cos / a0, (1. - alpha) / a0)
}
/// A fourth-order Butterworth: two sections.
fn fourth(rate: f64, hz: f64, high: bool) -> [Biquad; 2] {
    [section(rate, hz, high, 0.541_196_100_146_197), section(rate, hz, high, 1.306_562_964_876_376_5)]
}

const TRUE_PEAK_PHASES: usize = 8;
const TRUE_PEAK_TAPS: usize = 12;
/// 8× oversampling for true peak: a windowed-sinc polyphase filter, each phase normalized to unity at DC.
static TRUE_PEAK: std::sync::LazyLock<[[f64; TRUE_PEAK_TAPS]; TRUE_PEAK_PHASES]> = std::sync::LazyLock::new(|| {
    let length = TRUE_PEAK_PHASES * TRUE_PEAK_TAPS;
    let center = (length - 1) as f64 / 2.;
    let prototype: Vec<f64> = (0..length)
        .map(|n| {
            let x = (n as f64 - center) / TRUE_PEAK_PHASES as f64;
            let sinc = if x == 0. { 1. } else { (PI * x).sin() / (PI * x) };
            let phase = 2. * PI * n as f64 / (length - 1) as f64;
            let window = 0.35875 - 0.48829 * phase.cos() + 0.14128 * (2. * phase).cos() - 0.01168 * (3. * phase).cos();
            sinc * window
        })
        .collect();
    std::array::from_fn(|phase| {
        let mut taps: [f64; TRUE_PEAK_TAPS] = std::array::from_fn(|tap| prototype[tap * TRUE_PEAK_PHASES + phase]);
        let sum: f64 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }
        taps
    })
});

#[derive(Clone, Default)]
struct TruePeak8 {
    history: [f64; TRUE_PEAK_TAPS * 2],
    at: usize,
}
impl TruePeak8 {
    fn push(&mut self, sample: f64) {
        self.history[self.at] = sample;
        self.history[self.at + TRUE_PEAK_TAPS] = sample;
        self.at = (self.at + 1) % TRUE_PEAK_TAPS;
    }
    fn peak(&self) -> f64 {
        let history = &self.history[self.at..self.at + TRUE_PEAK_TAPS];
        TRUE_PEAK
            .iter()
            .map(|taps| taps.iter().zip(history.iter().rev()).map(|(tap, sample)| tap * sample).sum::<f64>().abs())
            .fold(0., f64::max)
    }
}

/// The main stream's FFT: about 85 ms, every quarter of that.
fn main_size(rate: f64) -> usize {
    ((rate * 0.085) as usize).next_power_of_two().max(1024)
}
const LOW_SIZE: usize = 2048;

struct Stream {
    size: usize,
    hop: usize,
    window: Vec<f64>,
    /// Σw² × size: a bin's power divided by it is that bin's share of the signal's power (one-sided, ×2).
    scale: f64,
    mid: Vec<f64>,
    side: Vec<f64>,
    filled: usize,
    re: Vec<f64>,
    im: Vec<f64>,
}
impl Stream {
    fn new(size: usize) -> Self {
        let window: Vec<f64> = (0..size).map(|n| 0.5 - 0.5 * (2. * PI * n as f64 / size as f64).cos()).collect();
        let scale = window.iter().map(|w| w * w).sum::<f64>() * size as f64 / 2.;
        Self {
            size,
            hop: size / 4,
            window,
            scale,
            mid: vec![0.; size],
            side: vec![0.; size],
            filled: 0,
            re: vec![0.; size],
            im: vec![0.; size],
        }
    }
    /// Takes a sample; true when a frame is ready (then `spectra` gives it).
    fn push(&mut self, mid: f64, side: f64) -> bool {
        self.mid[self.filled] = mid;
        self.side[self.filled] = side;
        self.filled += 1;
        self.filled == self.size
    }
    /// The ready frame's mid and side power per bin (normalized), then slides by a hop.
    fn spectra(&mut self, mid: &mut [f64], side: &mut [f64]) {
        for n in 0..self.size {
            self.re[n] = self.mid[n] * self.window[n];
            self.im[n] = self.side[n] * self.window[n];
        }
        fft(&mut self.re, &mut self.im);
        for k in 0..=self.size / 2 {
            let mirror = (self.size - k) % self.size;
            let (zr, zi, wr, wi) = (self.re[k], self.im[k], self.re[mirror], self.im[mirror]);
            // Two real signals in one complex FFT: X = (Z + conj Z*) / 2, Y = (Z − conj Z*) / 2i.
            let (xr, xi, yr, yi) = ((zr + wr) / 2., (zi - wi) / 2., (zi + wi) / 2., (wr - zr) / 2.);
            mid[k] = (xr * xr + xi * xi) / self.scale;
            side[k] = (yr * yr + yi * yi) / self.scale;
        }
        self.mid.copy_within(self.hop.., 0);
        self.side.copy_within(self.hop.., 0);
        self.filled = self.size - self.hop;
    }
}

struct Meter {
    rate: f64,
    weighting: [[Biquad; 2]; 2],
    block: usize,
    in_block: usize,
    block_sum: [f64; 2],
    blocks: Vec<f64>,
    block_peak: f64,
    block_peaks: Vec<f64>,
    peakers: [TruePeak8; 2],
    watch: [usize; 2],
    true_peak: f64,
    sample_peak: f64,
    clipped: usize,
    clipped_at: Vec<[f64; 2]>,
    dc: [f64; 2],
    count: usize,
    // Punch and pumping, every 10 ms.
    tick: usize,
    in_tick: usize,
    tick_peak: f64,
    tick_sum: f64,
    tick_band: f64,
    ticks: Vec<(f32, f32, f32)>,
    band: [Biquad; 2],
    // The main stream, the low stream and their decimation.
    main: Stream,
    main_band: Vec<Option<usize>>,
    fine_of: Vec<Option<usize>>,
    fine_count: [usize; FINE_BINS],
    spectrum_mid: Vec<f64>,
    spectrum_side: Vec<f64>,
    low: Stream,
    low_rate: f64,
    decimate: usize,
    decimated: usize,
    alias: [[Biquad; 2]; 2],
    low_band: Vec<Option<usize>>,
    low_mid: Vec<f64>,
    low_side: Vec<f64>,
    low_sums: [f64; 4],
    frames: Frames,
    low_frames: Vec<([f32; LOW_BANDS], [f32; LOW_BANDS])>,
    // The envelope every millisecond (RMS of the mid, and of the side), for attacks and decays: step k ends at the
    // sample nearest k ms, so a step is a millisecond exactly on average at any rate (44.1 kHz's are 44 and 45 samples).
    millis: usize,
    milli_end: usize,
    in_milli: usize,
    milli_sum: f64,
    milli_side: f64,
    envelope: Vec<f32>,
    side_envelope: Vec<f32>,
    // Each millisecond's sample peak (of the mid), for a hit's waveform crest.
    peaks: Vec<f32>,
    // Each millisecond's biggest and typical sample-to-sample jump, and its peak: clicks at note edges and crackle.
    previous: f64,
    jumps: Vec<f32>,
    milli_peak: f64,
    clicks: usize,
    spikes: usize,
    // The low end's rising zero crossings (seconds), for a low sound's pitch as it moves: two one-poles at 300 Hz.
    low_pass: [f64; 2],
    low_last: f64,
    crossings: Vec<f64>,
}

impl Meter {
    fn new(rate: f64) -> Self {
        let size = main_size(rate);
        let main = Stream::new(size);
        let main_band: Vec<Option<usize>> = (0..=size / 2)
            .map(|k| {
                let hz = k as f64 * rate / size as f64;
                (LOW_BANDS..THIRDS.len()).find(|band| {
                    let (from, to) = band_span(*band);
                    hz >= from && hz < to
                })
            })
            .collect();
        let mut fine_count = [0; FINE_BINS];
        let fine_of: Vec<Option<usize>> = (0..=size / 2)
            .map(|k| {
                let hz = k as f64 * rate / size as f64;
                let bin = (12. * (hz / FINE_FROM).log2() + 0.5).floor();
                (bin >= 0. && (bin as usize) < FINE_BINS).then(|| {
                    fine_count[bin as usize] += 1;
                    bin as usize
                })
            })
            .collect();
        let decimate = ((rate / 6000.).round() as usize).max(1);
        let low_rate = rate / decimate as f64;
        let low_band = (0..=LOW_SIZE / 2)
            .map(|k| {
                let hz = k as f64 * low_rate / LOW_SIZE as f64;
                (0..LOW_BANDS).find(|band| {
                    let (from, to) = band_span(*band);
                    hz >= from && hz < to
                })
            })
            .collect();
        let block = (rate / 10.).round() as usize;
        let tick = (rate / 100.).round() as usize;
        Self {
            rate,
            weighting: [k_weighting(rate), k_weighting(rate)],
            block,
            in_block: 0,
            block_sum: [0.; 2],
            blocks: vec![],
            block_peak: 0.,
            block_peaks: vec![],
            peakers: Default::default(),
            watch: [0; 2],
            true_peak: 0.,
            sample_peak: 0.,
            clipped: 0,
            clipped_at: vec![],
            dc: [0.; 2],
            count: 0,
            tick,
            in_tick: 0,
            tick_peak: 0.,
            tick_sum: 0.,
            tick_band: 0.,
            ticks: vec![],
            band: [
                section(rate, 300., true, std::f64::consts::FRAC_1_SQRT_2),
                section(rate, 3000., false, std::f64::consts::FRAC_1_SQRT_2),
            ],
            main,
            main_band,
            fine_of,
            fine_count,
            spectrum_mid: vec![0.; size / 2 + 1],
            spectrum_side: vec![0.; size / 2 + 1],
            low: Stream::new(LOW_SIZE),
            low_rate,
            decimate,
            decimated: 0,
            alias: [fourth(rate, 400., false), fourth(rate, 400., false)],
            low_band,
            low_mid: vec![0.; LOW_SIZE / 2 + 1],
            low_side: vec![0.; LOW_SIZE / 2 + 1],
            low_sums: [0.; 4],
            frames: Frames {
                hop: (size / 4) as f64 / rate,
                bass_hop: (LOW_SIZE / 4 * decimate) as f64 / rate,
                low_bin_hz: low_rate / LOW_SIZE as f64,
                ..Default::default()
            },
            low_frames: vec![],
            millis: 0,
            milli_end: ((rate / 1000.).round() as usize).max(1),
            in_milli: 0,
            milli_sum: 0.,
            milli_side: 0.,
            envelope: vec![],
            side_envelope: vec![],
            peaks: vec![],
            previous: 0.,
            jumps: vec![],
            milli_peak: 0.,
            clicks: 0,
            spikes: 0,
            low_pass: [0.; 2],
            low_last: 0.,
            crossings: vec![],
        }
    }

    fn push(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            let (l, r) = (*l as f64, *r as f64);
            self.count += 1;
            for (channel, sample) in [l, r].into_iter().enumerate() {
                let [shelf, high] = &mut self.weighting[channel];
                let weighted = high.process(shelf.process(sample));
                self.block_sum[channel] += weighted * weighted;
                self.dc[channel] += sample;
                let magnitude = sample.abs();
                self.sample_peak = self.sample_peak.max(magnitude);
                if magnitude >= 0.999 {
                    self.clipped += 1;
                    let at = self.count as f64 / self.rate;
                    let count = self.clipped_at.len();
                    match self.clipped_at.last_mut() {
                        Some(last) if at - last[1] < 0.5 => last[1] = at,
                        _ if count < 5 => self.clipped_at.push([at, at]),
                        _ => {}
                    }
                }
                let peaker = &mut self.peakers[channel];
                peaker.push(sample);
                // The oversampled peak is worked out only near the loudest so far (it lies within 1 dB of a sample peak).
                if magnitude > self.true_peak * 0.5 {
                    self.watch[channel] = TRUE_PEAK_TAPS;
                }
                let found = if self.watch[channel] > 0 {
                    self.watch[channel] -= 1;
                    peaker.peak().max(magnitude)
                } else {
                    magnitude
                };
                self.true_peak = self.true_peak.max(found);
                self.block_peak = self.block_peak.max(found);
            }
            self.in_block += 1;
            if self.in_block == self.block {
                self.blocks.push((self.block_sum[0] + self.block_sum[1]) / self.block as f64);
                self.block_peaks.push(self.block_peak);
                self.block_sum = [0.; 2];
                self.block_peak = 0.;
                self.in_block = 0;
            }
            let (mid, side) = ((l + r) / 2., (l - r) / 2.);
            self.milli_sum += mid * mid;
            self.milli_side += side * side;
            self.in_milli += 1;
            self.jumps.push((mid - self.previous).abs() as f32);
            self.previous = mid;
            let pole = 1. - (-2. * std::f64::consts::PI * 300. / self.rate).exp();
            self.low_pass[0] += pole * (mid - self.low_pass[0]);
            self.low_pass[1] += pole * (self.low_pass[0] - self.low_pass[1]);
            let low = self.low_pass[1];
            if self.low_last < 0. && low >= 0. && self.crossings.len() < 2_000_000 {
                // Where between this sample and the last it crossed.
                let into = self.low_last / (self.low_last - low);
                self.crossings.push((self.count as f64 - 1. + into) / self.rate);
            }
            self.low_last = low;
            self.milli_peak = self.milli_peak.max(mid.abs());
            if self.count >= self.milli_end {
                let rms = (self.milli_sum / self.in_milli as f64).sqrt();
                // A click: out of near silence (the 3 ms before under −60 dB), one jump far bigger than the rest of
                // the millisecond's (a waveform cut, not a transient's burst of them).
                let silent_before = self.envelope.len() >= 3 && self.envelope[self.envelope.len() - 3..].iter().all(|level| *level < 1e-3);
                let mut jumps = std::mem::take(&mut self.jumps);
                let biggest = jumps.iter().copied().fold(0f32, f32::max) as f64;
                jumps.sort_by(f32::total_cmp);
                let typical = jumps.get(jumps.len() * 3 / 4).copied().unwrap_or(0.) as f64;
                if silent_before && biggest >= 0.05 && biggest >= typical * 6. {
                    self.clicks += 1;
                }
                // Crackle: a lone spike standing far over a quiet millisecond (under −30 dB).
                if rms < 0.03 && rms > 1e-6 && self.milli_peak >= rms * 5. && !silent_before {
                    self.spikes += 1;
                }
                self.envelope.push(rms as f32);
                self.side_envelope.push((self.milli_side / self.in_milli as f64).sqrt() as f32);
                self.peaks.push(self.milli_peak as f32);
                self.milli_sum = 0.;
                self.milli_side = 0.;
                self.in_milli = 0;
                self.milli_peak = 0.;
                self.millis += 1;
                self.milli_end = (((self.millis + 1) as f64 * self.rate / 1000.).round() as usize).max(self.count + 1);
            }
            let first = self.band[0].process(mid);
            let banded = self.band[1].process(first);
            self.tick_peak = self.tick_peak.max(mid.abs());
            self.tick_sum += mid * mid;
            self.tick_band += banded * banded;
            self.in_tick += 1;
            if self.in_tick == self.tick {
                let n = self.tick as f64;
                self.ticks.push((self.tick_peak as f32, (self.tick_sum / n) as f32, (self.tick_band / n) as f32));
                self.tick_peak = 0.;
                self.tick_sum = 0.;
                self.tick_band = 0.;
                self.in_tick = 0;
            }
            if self.main.push(mid, side) {
                self.main_frame();
            }
            let first = self.alias[0][0].process(mid);
            let low_mid = self.alias[0][1].process(first);
            let first = self.alias[1][0].process(side);
            let low_side = self.alias[1][1].process(first);
            self.decimated += 1;
            if self.decimated == self.decimate {
                self.decimated = 0;
                if self.low.push(low_mid, low_side) {
                    self.low_frame();
                }
            }
        }
    }

    fn main_frame(&mut self) {
        self.main.spectra(&mut self.spectrum_mid, &mut self.spectrum_side);
        let mut mid = [0f32; 31];
        let mut side = [0f32; 31];
        let mut fine = [0f64; FINE_BINS];
        let mut total = 0.;
        for k in 1..self.spectrum_mid.len() {
            let (m, s) = (self.spectrum_mid[k], self.spectrum_side[k]);
            if let Some(band) = self.main_band[k] {
                mid[band] += m as f32;
                side[band] += s as f32;
            }
            if let Some(bin) = self.fine_of[k] {
                fine[bin] += m;
            }
            total += m + s;
        }
        let bin_hz = self.rate / self.main.size as f64;
        let fine_db: [f32; FINE_BINS] = std::array::from_fn(|bin| {
            // Power density: a bin with no FFT bin in it (none at these sizes) takes its nearest.
            let density = if self.fine_count[bin] > 0 {
                fine[bin] / self.fine_count[bin] as f64
            } else {
                self.spectrum_mid[((fine_hz(bin) / bin_hz).round() as usize).min(self.spectrum_mid.len() - 1)]
            };
            (10. * (density + 1e-20).log10()) as f32
        });
        self.frames.mid.push(mid);
        self.frames.side.push(side);
        self.frames.fine.push(fine_db);
        self.frames.level.push((10. * (total + 1e-20).log10()) as f32);
    }

    fn low_frame(&mut self) {
        self.low.spectra(&mut self.low_mid, &mut self.low_side);
        let mut mid = [0f32; LOW_BANDS];
        let mut side = [0f32; LOW_BANDS];
        let bin_hz = self.low_rate / LOW_SIZE as f64;
        let mut strongest: Option<(usize, f64)> = None;
        for k in 1..self.low_mid.len() {
            let (m, s) = (self.low_mid[k], self.low_side[k]);
            if let Some(band) = self.low_band[k] {
                mid[band] += m as f32;
                side[band] += s as f32;
            }
            let hz = k as f64 * bin_hz;
            if hz < 30. {
                self.low_sums[0] += m + s;
            } else if hz < 120. {
                self.low_sums[1] += m + s;
                self.low_sums[2] += m;
                self.low_sums[3] += s;
            }
            if (30. ..250.).contains(&hz)
                && k + 1 < self.low_mid.len()
                && m > self.low_mid[k - 1]
                && m >= self.low_mid[k + 1]
                && strongest.is_none_or(|(_, power)| m > power)
            {
                strongest = Some((k, m));
            }
        }
        let note = strongest.map(|(k, power)| {
            // Where the peak really is, between bins (a parabola through three).
            let (a, b, c) = ((self.low_mid[k - 1] + 1e-20).ln(), (power + 1e-20).ln(), (self.low_mid[k + 1] + 1e-20).ln());
            let shift = if (a - 2. * b + c).abs() > 1e-12 { 0.5 * (a - c) / (a - 2. * b + c) } else { 0. };
            (((k as f64 + shift.clamp(-0.5, 0.5)) * bin_hz) as f32, (10. * (power + 1e-20).log10()) as f32)
        });
        self.frames.bass.push(note);
        let kept = ((LOW_FINE_TOP / bin_hz) as usize + 1).min(self.low_mid.len());
        self.frames.low_fine.push(self.low_mid[..kept].iter().map(|power| *power as f32).collect());
        self.low_frames.push((mid, side));
    }

    fn finish(mut self) -> Heard {
        let rate = self.rate;
        let seconds = self.count as f64 / rate;
        // Loudness, from 100 ms blocks of K-weighted power.
        let lufs = |power: f64| -0.691 + 10. * power.log10();
        let windowed = |size: usize, hop: usize| -> Vec<f64> {
            if self.blocks.len() < size {
                return vec![];
            }
            (0..=self.blocks.len() - size).step_by(hop).map(|at| self.blocks[at..at + size].iter().sum::<f64>() / size as f64).collect()
        };
        let momentary = windowed(4, 1);
        let short = windowed(30, 1);
        let gated = |values: &[f64], relative: f64| -> Option<(f64, Vec<f64>)> {
            let loud: Vec<f64> = values.iter().copied().filter(|power| lufs(*power) > -70.).collect();
            if loud.is_empty() {
                return None;
            }
            let threshold = lufs(loud.iter().sum::<f64>() / loud.len() as f64) + relative;
            let kept: Vec<f64> = loud.into_iter().filter(|power| lufs(*power) > threshold).collect();
            (!kept.is_empty()).then(|| (lufs(kept.iter().sum::<f64>() / kept.len() as f64), kept))
        };
        let integrated = gated(&momentary, -10.).map(|(value, _)| value);
        let range = gated(&short, -20.).map(|(_, kept)| {
            let mut levels: Vec<f64> = kept.into_iter().map(lufs).collect();
            levels.sort_by(f64::total_cmp);
            percentile(&levels, 0.95) - percentile(&levels, 0.10)
        });
        let max_lufs = |values: &[f64]| {
            values.iter().copied().filter(|power| *power > 0.).map(lufs).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))
        };
        let true_peak = db_amplitude(self.true_peak);
        // PSR at the densest loud moment: each 3 s window's highest true peak over its loudness.
        let psr = integrated.and_then(|integrated| {
            (0..short.len())
                .filter(|at| lufs(short[*at]) > integrated - 10.)
                .map(|at| db_amplitude(self.block_peaks[at..at + 30].iter().copied().fold(0., f64::max)) - lufs(short[at]))
                .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.min(b))))
        });
        let short_term = (0..short.len())
            .step_by(10)
            .map(|at| Some(lufs(short[at])).filter(|value| value.is_finite() && *value > -70.).map(|value| (value * 10.).round() / 10.))
            .collect();
        // The low stream's bands fill the main frames' low bands, from the nearest low frame.
        let low_step = self.frames.bass_hop / self.frames.hop;
        let low_offset = ((LOW_SIZE * self.decimate) as f64 - self.main.size as f64) / 2. / rate / self.frames.hop;
        for frame in 0..self.frames.mid.len() {
            let low = ((frame as f64 - low_offset) / low_step).round().max(0.) as usize;
            if let Some((mid, side)) = self.low_frames.get(low.min(self.low_frames.len().saturating_sub(1))) {
                // Each low frame stands for `low_step` main frames' worth of power.
                self.frames.mid[frame][..LOW_BANDS].copy_from_slice(mid);
                self.frames.side[frame][..LOW_BANDS].copy_from_slice(side);
            }
        }
        // Tonal balance and width, from the whole: the low bands from the low stream, the rest from the main stream.
        let mut mid_sum = [0f64; 31];
        let mut side_sum = [0f64; 31];
        for (mid, side) in &self.low_frames {
            for band in 0..LOW_BANDS {
                mid_sum[band] += mid[band] as f64 / self.low_frames.len() as f64;
                side_sum[band] += side[band] as f64 / self.low_frames.len() as f64;
            }
        }
        for (mid, side) in self.frames.mid.iter().zip(&self.frames.side) {
            for band in LOW_BANDS..31 {
                mid_sum[band] += mid[band] as f64 / self.frames.mid.len() as f64;
                side_sum[band] += side[band] as f64 / self.frames.mid.len() as f64;
            }
        }
        let total: f64 = mid_sum.iter().zip(&side_sum).map(|(mid, side)| mid + side).sum();
        let nyquist = rate / 2.;
        let balance: Vec<f64> = (0..31)
            .map(|band| {
                if band_edges(band).0 >= nyquist {
                    return -120.;
                }
                round1(10. * ((mid_sum[band] + side_sum[band]) / total.max(1e-30) + 1e-12).log10())
            })
            .collect();
        let width: Vec<f64> = (0..31).map(|band| round2(side_sum[band] / (mid_sum[band] + side_sum[band]).max(1e-30))).collect();
        let tilt =
            slope(&(7..=27).map(|band| (THIRDS[band].log2(), balance[band])).filter(|(_, level)| *level > -100.).collect::<Vec<_>>());
        // Punch: crest in 400 ms windows of the louder half.
        let crest = {
            let windows: Vec<(f64, f64)> = if self.ticks.len() >= 40 {
                (0..=self.ticks.len() - 40)
                    .step_by(10)
                    .map(|at| {
                        let slice = &self.ticks[at..at + 40];
                        let peak = slice.iter().map(|tick| tick.0 as f64).fold(0., f64::max);
                        let power = slice.iter().map(|tick| tick.1 as f64).sum::<f64>() / 40.;
                        (power, peak)
                    })
                    .collect()
            } else {
                vec![]
            };
            let mut powers: Vec<f64> = windows.iter().map(|w| w.0).filter(|power| *power > 1e-10).collect();
            powers.sort_by(f64::total_cmp);
            (!powers.is_empty()).then(|| {
                let middle = percentile(&powers, 0.5);
                let mut crests: Vec<f64> = windows
                    .iter()
                    .filter(|(power, _)| *power >= middle && *power > 1e-10)
                    .map(|(power, peak)| db_amplitude(*peak) - 10. * power.log10())
                    .collect();
                crests.sort_by(f64::total_cmp);
                round1(percentile(&crests, 0.5))
            })
        };
        // Pumping: the mid band's level against its own one-second trend, at the rhythm's rates.
        let pumping = {
            let level: Vec<f64> = self.ticks.iter().map(|tick| 10. * (tick.2 as f64 + 1e-12).log10()).collect();
            if level.len() >= 200 {
                let smooth = moving(&level, 5);
                let trend = moving(&level, 100);
                let loudest = trend.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let swings: Vec<f64> =
                    smooth.iter().zip(&trend).filter(|(_, trend)| **trend > loudest - 20.).map(|(smooth, trend)| smooth - trend).collect();
                (!swings.is_empty()).then(|| round1((swings.iter().map(|swing| swing * swing).sum::<f64>() / swings.len() as f64).sqrt()))
            } else {
                None
            }
        };
        // Distortion: the loudest tenth's air against its middle, over the same for middling frames.
        let distortion = {
            let ratios: Vec<(f32, f64)> = self
                .frames
                .mid
                .iter()
                .zip(&self.frames.level)
                .filter_map(|(bands, level)| {
                    let high: f64 = bands[25..=29].iter().map(|v| *v as f64).sum();
                    let middle: f64 = bands[17..=23].iter().map(|v| *v as f64).sum();
                    (middle > 1e-12).then(|| (*level, 10. * ((high + 1e-20) / middle).log10()))
                })
                .collect();
            let mut levels: Vec<f64> = ratios.iter().map(|(level, _)| *level as f64).collect();
            levels.sort_by(f64::total_cmp);
            (ratios.len() >= 20).then(|| {
                let (loud, low, high) = (percentile(&levels, 0.9), percentile(&levels, 0.4), percentile(&levels, 0.7));
                let mean = |keep: &dyn Fn(f64) -> bool| {
                    let picked: Vec<f64> = ratios.iter().filter(|(level, _)| keep(*level as f64)).map(|(_, ratio)| *ratio).collect();
                    picked.iter().sum::<f64>() / picked.len().max(1) as f64
                };
                round1(mean(&|level| level >= loud) - mean(&|level| level >= low && level <= high))
            })
        };
        let (attack, decay, sustain) = envelope_of(&self.envelope);
        let modulation = modulation_of(&self.envelope);
        let hit_crest = hit_crest_of(&self.envelope, &self.peaks);
        let pitch_drop = pitch_drop_of(&self.envelope, &self.crossings);
        // Brightness and noisiness over the loud frames.
        let (centroid, noise) = {
            let mut levels: Vec<f64> = self.frames.level.iter().map(|level| *level as f64).collect();
            levels.sort_by(f64::total_cmp);
            let loud = if levels.is_empty() { 0. } else { percentile(&levels, 0.95) - 20. };
            let mut centroids = vec![];
            let mut flatness = vec![];
            for (frame, level) in self.frames.level.iter().enumerate() {
                if (*level as f64) < loud {
                    continue;
                }
                let powers: Vec<f64> = (0..31).map(|band| (self.frames.mid[frame][band] + self.frames.side[frame][band]) as f64).collect();
                let total: f64 = powers.iter().sum();
                if total > 1e-14 {
                    centroids.push(powers.iter().zip(THIRDS.iter()).map(|(power, hz)| power * hz).sum::<f64>() / total);
                }
                let fine: Vec<f64> = self.frames.fine[frame].iter().map(|db| 10f64.powf(*db as f64 / 10.)).collect();
                let arithmetic = fine.iter().sum::<f64>() / fine.len() as f64;
                let geometric = (fine.iter().map(|power| (power + 1e-30).ln()).sum::<f64>() / fine.len() as f64).exp();
                if arithmetic > 1e-20 {
                    flatness.push(10. * (geometric / arithmetic).log10());
                }
            }
            centroids.sort_by(f64::total_cmp);
            flatness.sort_by(f64::total_cmp);
            (
                (!centroids.is_empty()).then(|| percentile(&centroids, 0.5).round()),
                (!flatness.is_empty()).then(|| round1(percentile(&flatness, 0.5))),
            )
        };
        let [below, low_all, low_mid, low_side] = self.low_sums;
        let measures = Measures {
            seconds,
            sample_rate: rate,
            integrated: integrated.map(round1),
            short_term_max: max_lufs(&short).map(round1),
            momentary_max: max_lufs(&momentary).map(round1),
            range: range.map(round1),
            true_peak: round2(true_peak),
            sample_peak: round2(db_amplitude(self.sample_peak)),
            plr: integrated.map(|integrated| round1(true_peak - integrated)),
            psr: psr.map(round1),
            clipped: self.clipped,
            clipped_at: self.clipped_at.iter().map(|span| [round1(span[0]), round1(span[1])]).collect(),
            peak_at: self
                .block_peaks
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .filter(|(_, peak)| **peak > 0.)
                .map(|(at, _)| round1(at as f64 * self.block as f64 / rate)),
            balance,
            width,
            tilt: round2(tilt),
            crest,
            pumping,
            distortion,
            low_width: (low_mid > 1e-14).then(|| round1((10. * ((low_side + 1e-20) / low_mid).log10()).max(-60.))),
            rumble: (low_all > 1e-14).then(|| round1(10. * ((below + 1e-20) / low_all).log10())),
            dc: [self.dc[0] / self.count.max(1) as f64, self.dc[1] / self.count.max(1) as f64],
            short_term,
            attack,
            decay,
            sustain,
            centroid,
            noise,
            pitch_drop,
            modulation,
            hit_crest,
            clicks: self.clicks,
            crackle: if seconds > 0. { round1(self.spikes as f64 / seconds) } else { 0. },
        };
        // Every 5 ms, the level over the 20 ms around it.
        let power: Vec<f64> =
            self.envelope.iter().zip(&self.side_envelope).map(|(mid, side)| (*mid as f64).powi(2) + (*side as f64).powi(2)).collect();
        self.frames.envelope = (0..power.len() / 5)
            .map(|step| {
                let centre = step * 5 + 2;
                let (from, to) = (centre.saturating_sub(10), (centre + 10).min(power.len()));
                (power[from..to].iter().sum::<f64>() / (to - from).max(1) as f64).sqrt() as f32
            })
            .collect();
        Heard { measures, frames: self.frames, embedding: None }
    }
}

/// A sound's envelope from its hits, at a millisecond a step: each hit is a rise of 12 dB or more within 20 ms, and
/// its attack, decay (to 20 dB under the peak, before the next hit) and the level a quarter second on are the medians
/// over the hits.
fn envelope_of(envelope: &[f32]) -> (Option<f64>, Option<f64>, Option<f64>) {
    let raw: Vec<f64> = envelope.iter().map(|value| 20. * (*value as f64 + 1e-9).log10()).collect();
    let db = held(&raw);
    let hits = hits_of(&db);
    let (mut attacks, mut decays, mut sustains) = (vec![], vec![], vec![]);
    for (index, start) in hits.iter().enumerate() {
        let end = hits.get(index + 1).copied().unwrap_or(db.len()).min(start + 4000);
        let Some((peak_at, peak)) = db[*start..end].iter().copied().enumerate().take(300).max_by(|a, b| a.1.total_cmp(&b.1)) else {
            continue;
        };
        let peak_at = start + peak_at;
        let amplitude = |level: f64| 10f64.powf(level / 20.);
        let (low, high) = (amplitude(peak) * 0.1, amplitude(peak) * 0.9);
        // The attack on the raw envelope (the held one keeps the rise's timing too).
        let from = (*start..=peak_at).find(|at| amplitude(raw[*at].max(db[*at])) >= low).unwrap_or(*start);
        let to = (from..=peak_at).find(|at| amplitude(db[*at]) >= high).unwrap_or(peak_at);
        attacks.push((to - from) as f64);
        if let Some(fallen) = (peak_at..end).find(|at| db[*at] <= peak - 20.) {
            decays.push((fallen - peak_at) as f64);
        }
        if peak_at + 250 < end {
            sustains.push(db[peak_at + 250] - peak);
        }
    }
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        (!values.is_empty()).then(|| round1(percentile(&values, 0.5)))
    };
    (median(attacks), median(decays), median(sustains))
}

/// Its hits in the held millisecond envelope (dB): rises of 12 dB or more within 20 ms, within 40 dB of the loudest
/// and 60 ms apart at least, each where it starts rising.
fn hits_of(db: &[f64]) -> Vec<usize> {
    hits_rising(db, 12.)
}

/// Its hits, rises of `rise` dB or more within 20 ms (otherwise as `hits_of`).
fn hits_rising(db: &[f64], rise: f64) -> Vec<usize> {
    let loudest = db.iter().copied().fold(f64::MIN, f64::max);
    let mut hits: Vec<usize> = vec![];
    let mut at = 20;
    while at < db.len() {
        let before = db[at - 20..at].iter().copied().fold(f64::MAX, f64::min);
        if db[at] - before >= rise && db[at] > loudest - 40. && hits.last().is_none_or(|last| at - last > 60) {
            hits.push(at - 20 + db[at - 20..=at].iter().position(|level| *level >= before + 1.).unwrap_or(0));
            at += 60;
        } else {
            at += 1;
        }
    }
    hits
}

/// How often the level swings, Hz: a clear repeat of the millisecond envelope (held over 25 ms, so a low note's own
/// waveform isn't read as a swing; its slow trend over 4 s taken out) between a twentieth of a second and two seconds
/// (20 to 0.5 Hz): the shortest within a tenth of the strongest, so a swing isn't read at twice its period. A repeat
/// that's the notes' own rhythm (most hits followed by another that far on) isn't a swing: plucked 8ths aren't a
/// tremolo.
fn modulation_of(envelope: &[f32]) -> Option<f64> {
    if envelope.len() < 2000 {
        return None;
    }
    let db = held(&envelope.iter().map(|value| 20. * (*value as f64 + 1e-9).log10()).collect::<Vec<_>>());
    // Ten-millisecond steps, the slow trend taken out.
    let coarse: Vec<f64> = db.chunks(10).map(|chunk| chunk.iter().sum::<f64>() / chunk.len() as f64).collect();
    let trend = moving(&coarse, 400);
    let wave: Vec<f64> = coarse.iter().zip(&trend).map(|(value, trend)| value - trend).collect();
    let energy: f64 = wave.iter().map(|value| value * value).sum();
    if energy / wave.len() as f64 <= 0.25 {
        return None;
    }
    let hits = hits_of(&db);
    // Whether most hits are followed by another `lag` steps on (within 15 ms).
    let rhythm = |lag: usize| {
        let followed = hits
            .iter()
            .filter(|at| {
                let wanted = **at + lag * 10;
                let next = hits.partition_point(|other| *other + 15 < wanted);
                hits.get(next).is_some_and(|other| *other <= wanted + 15)
            })
            .count();
        hits.len() >= 4 && followed * 2 >= hits.len()
    };
    // Two seconds (0.5 Hz) is the slowest swing it reports; the correlation of one that slow peaks a lag or three
    // past it, so the search reaches 2.05 s.
    let top = 205.min(wave.len() / 3);
    let correlation: Vec<f64> = (0..=top + 1).map(|lag| wave.iter().zip(&wave[lag..]).map(|(a, b)| a * b).sum::<f64>() / energy).collect();
    // Clear repeats, each a peak of its own (not the slope down from lag 0).
    let repeats: Vec<usize> = (5..=top)
        .filter(|lag| correlation[*lag] >= 0.4 && correlation[*lag] >= correlation[lag - 1] && correlation[*lag] >= correlation[lag + 1])
        .filter(|lag| !rhythm(*lag))
        .collect();
    let strongest = repeats.iter().map(|lag| correlation[*lag]).fold(f64::MIN, f64::max);
    repeats.into_iter().find(|lag| correlation[*lag] >= strongest * 0.9).map(|lag| round2(100. / lag as f64))
}

/// Each hit's waveform crest, dB: the sample peak over the RMS in the 20 ms around the hit's loudest millisecond, the
/// median over its hits. Saturation or a tail that fills the gaps can flatten its hits' 12 dB rises: then rises of
/// 6 dB count. A sound with no hits at all (a pad) reads the median crest of its louder 20 ms stretches. So the
/// reading stays while a change flattens the sound, which is what it guards.
fn hit_crest_of(envelope: &[f32], peaks: &[f32]) -> Option<f64> {
    let raw: Vec<f64> = envelope.iter().map(|value| 20. * (*value as f64 + 1e-9).log10()).collect();
    let db = held(&raw);
    let length = envelope.len().min(peaks.len());
    // A stretch's crest: its sample peak over its RMS.
    let crest = |from: usize, to: usize| {
        let power = envelope[from..to].iter().map(|value| (*value as f64).powi(2)).sum::<f64>() / (to - from).max(1) as f64;
        let peak = peaks[from..to].iter().copied().fold(0f32, f32::max) as f64;
        (power > 1e-12 && peak > 0.).then(|| (power, 20. * peak.log10() - 10. * power.log10()))
    };
    let mut hits = hits_rising(&db, 12.);
    if hits.is_empty() {
        hits = hits_rising(&db, 6.);
    }
    let mut crests: Vec<f64> = if hits.is_empty() {
        // The louder half of its 20 ms stretches.
        let stretches: Vec<(f64, f64)> = (0..length / 20).filter_map(|k| crest(k * 20, k * 20 + 20)).collect();
        let mut powers: Vec<f64> = stretches.iter().map(|(power, _)| *power).collect();
        powers.sort_by(f64::total_cmp);
        let middle = if powers.is_empty() { 0. } else { percentile(&powers, 0.5) };
        stretches.into_iter().filter(|(power, _)| *power >= middle).map(|(_, crest)| crest).collect()
    } else {
        hits.iter()
            .enumerate()
            .filter_map(|(index, start)| {
                let end = hits.get(index + 1).copied().unwrap_or(raw.len()).min(start + 300);
                let at = (*start..end).max_by(|a, b| raw[*a].total_cmp(&raw[*b]))?;
                crest(at.saturating_sub(10), (at + 10).min(length).max(at.saturating_sub(10)))
            })
            .map(|(_, crest)| crest)
            .collect()
    };
    crests.sort_by(f64::total_cmp);
    (!crests.is_empty()).then(|| round1(percentile(&crests, 0.5)))
}

/// How far a low sound's pitch falls over its first quarter second, semitones: at each hit, the low end's first full
/// period (between its first two rising zero crossings) against its period 200 ms in, the median over hits that have
/// both.
fn pitch_drop_of(envelope: &[f32], crossings: &[f64]) -> Option<f64> {
    if crossings.len() < 4 {
        return None;
    }
    // The pitch around a moment: one over the period of the crossing pair that holds it, when it's 30–250 Hz.
    let pitch = |at: f64| {
        let next = crossings.partition_point(|crossing| *crossing <= at);
        let (a, b) = (crossings.get(next.checked_sub(1)?)?, crossings.get(next)?);
        let hz = 1. / (b - a);
        (30. ..=250.).contains(&hz).then_some(hz)
    };
    let db = held(&envelope.iter().map(|value| 20. * (*value as f64 + 1e-9).log10()).collect::<Vec<_>>());
    let mut drops = vec![];
    let mut at = 20;
    while at < db.len() {
        let before = db[at - 20..at].iter().copied().fold(f64::MAX, f64::min);
        if db[at] - before >= 12. {
            let start = (at - 20 + db[at - 20..=at].iter().position(|level| *level >= before + 1.).unwrap_or(0)) as f64 / 1000.;
            // Its first period: the first two crossings after it starts, the first within 60 ms.
            let first = crossings.partition_point(|crossing| *crossing <= start);
            let early = match (crossings.get(first), crossings.get(first + 1)) {
                (Some(a), Some(b)) if a - start < 0.06 => Some(1. / (b - a)).filter(|hz| (30. ..=250.).contains(hz)),
                _ => None,
            };
            if let (Some(early), Some(later)) = (early, pitch(start + 0.2)) {
                drops.push(12. * (early / later).log2());
            }
            at += 300;
        } else {
            at += 1;
        }
    }
    drops.sort_by(f64::total_cmp);
    (!drops.is_empty()).then(|| round1(percentile(&drops, 0.5)))
}

/// The millisecond envelope held at its peak over the 25 ms before each step: a low note's waveform dips between its
/// peaks (a 1 ms window is far shorter than its period), and those dips aren't the sound falling away.
fn held(db: &[f64]) -> Vec<f64> {
    (0..db.len()).map(|at| db[at.saturating_sub(24)..=at].iter().copied().fold(f64::MIN, f64::max)).collect()
}

fn moving(values: &[f64], size: usize) -> Vec<f64> {
    let half = size / 2;
    let mut sums = vec![0.; values.len() + 1];
    for (at, value) in values.iter().enumerate() {
        sums[at + 1] = sums[at] + value;
    }
    (0..values.len())
        .map(|at| {
            let (from, to) = (at.saturating_sub(half), (at + half + 1).min(values.len()));
            (sums[to] - sums[from]) / (to - from) as f64
        })
        .collect()
}
/// A sorted list's value at `p` (0–1), between neighbors.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let at = p.clamp(0., 1.) * (sorted.len() - 1) as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    sorted[low] + (sorted[high] - sorted[low]) * (at - low as f64)
}
fn slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    if n < 2. {
        return 0.;
    }
    let (sx, sy) = points.iter().fold((0., 0.), |(sx, sy), (x, y)| (sx + x, sy + y));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = points.iter().fold((0., 0.), |(num, den), (x, y)| (num + (x - mx) * (y - my), den + (x - mx) * (x - mx)));
    if den > 0. {
        num / den
    } else {
        0.
    }
}
pub fn db_amplitude(amplitude: f64) -> f64 {
    if amplitude > 0. {
        20. * amplitude.log10()
    } else {
        f64::NEG_INFINITY
    }
}
fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
fn round2(value: f64) -> f64 {
    (value * 100.).round() / 100.
}
