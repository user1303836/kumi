//! One pass measures loudness, balance, stereo, dynamics, tempo, key, pitch, harmonics and movement.

use super::{
    decode::{open_audio, AudioError, AudioSource},
    dsp::*,
};
use kumi_common::abort::{Signal, SignalExt};
use kumi_common::js::number::{round as js_round, to_string};
use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_1_SQRT_2, PI};
pub const ANALYSIS_VERSION: u32 = 1;
#[derive(Debug, Clone, Copy)]
pub struct Band {
    pub name: &'static str,
    pub from: f64,
    pub to: f64,
}
pub const BANDS: [Band; 10] = [
    Band { name: "sub", from: 20.0, to: 60.0 },
    Band { name: "bass", from: 60.0, to: 120.0 },
    Band { name: "upper bass", from: 120.0, to: 250.0 },
    Band { name: "low mids", from: 250.0, to: 500.0 },
    Band { name: "mids", from: 500.0, to: 1000.0 },
    Band { name: "upper mids", from: 1000.0, to: 2000.0 },
    Band { name: "presence", from: 2000.0, to: 4000.0 },
    Band { name: "bite", from: 4000.0, to: 6000.0 },
    Band { name: "brilliance", from: 6000.0, to: 10000.0 },
    Band { name: "air", from: 10000.0, to: 20000.0 },
];
macro_rules! object {($name:ident {$($field:ident : $ty:ty),*$(,)?})=>{#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]#[serde(rename_all="camelCase")]pub struct $name{$(pub $field:$ty),*}}}
object!(BandLevel { name: String, hz: String, db: f64, width: f64, correlation: f64 });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pitch {
    pub note: String,
    pub hz: f64,
    pub cents: f64,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement: Option<String>,
}
object!(Harmonics{partials:Vec<f64>,shape:String,slope_db_per_octave:f64,odd_to_even:f64,noise_db:f64,inharmonicity:f64});
object!(Envelope { attack_ms: f64, decay_ms: f64, sustain_db: f64, release_ms: f64, length_ms: f64 });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lfo {
    pub hz: f64,
    pub on: String,
    pub depth: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_tempo: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Movement {
    pub brightness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfo: Option<Lfo>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundAnalysis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<Pitch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harmonics: Option<Harmonics>,
    pub envelope: Envelope,
    pub movement: Movement,
}
object!(Analyzed { from: String, to: String, focus: String });
object!(Loudness{integrated_lufs:Option<f64>,short_term_max_lufs:Option<f64>,range_lu:Option<f64>,true_peak_dbtp:f64,sample_peak_dbfs:f64,clipped_samples:usize});
object!(Balance{bands:Vec<BandLevel>,tilt_db_per_octave:f64,centroid_hz:f64});
object!(Stereo { correlation: f64, width: f64, low_end_mono: bool });
object!(Dynamics{crest_db:f64,peak_to_loudness_db:Option<f64>,onsets_per_second:f64});
object!(Tempo { bpm: f64, confidence: f64 });
object!(Key { name: String, confidence: f64 });
object!(OverTime{every:String,lufs:Vec<Option<f64>>});
object!(SpectrogramRow { band: String, cells: String });
object!(Spectrogram{rows:Vec<SpectrogramRow>,columns:String,scale:String});
object!(Timeline{step:f64,onset:Vec<f64>,level:Vec<f64>});
object!(HeardNote{time:f64,duration:f64,midi:Option<f64>,confidence:f64,velocity:f64});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub kumi_audio: u32,
    pub file: String,
    pub format: String,
    pub sample_rate: f64,
    pub channels: usize,
    pub seconds: f64,
    pub analyzed: Analyzed,
    pub loudness: Loudness,
    pub balance: Balance,
    pub stereo: Option<Stereo>,
    pub dynamics: Dynamics,
    pub tempo: Option<Tempo>,
    pub key: Option<Key>,
    pub over_time: OverTime,
    pub spectrogram: Spectrogram,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Timeline>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<SoundAnalysis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<HeardNote>>,
}
#[derive(Debug, Clone, Default)]
pub struct AnalyzeOptions {
    pub focus: Option<String>,
    pub start: Option<f64>,
    pub seconds: Option<f64>,
    pub signal: Option<Signal>,
    pub transcribe: bool,
}
pub async fn analyze_file(path: &str, options: AnalyzeOptions) -> Result<Analysis, AudioError> {
    let mut source = open_audio(path, options.signal.clone()).await?;
    let result = analyze_source(&mut source, path, options).await;
    source.close().await?;
    result
}
pub async fn analyze_source(source: &mut AudioSource, name: &str, options: AnalyzeOptions) -> Result<Analysis, AudioError> {
    let rate = source.sample_rate;
    let channels = source.channels;
    if !(8000.0..=384000.0).contains(&rate) {
        return Err(AudioError(format!("A sample rate of {} Hz isn't one Kumi can analyze.", to_string(rate))));
    }
    let total = source.frames as f64 / rate;
    let start = options.start.unwrap_or(0.0).max(0.0);
    let length = options.seconds.unwrap_or(720.0).min(720.0).min(total - start);
    if !(length > 0.02) {
        return Err(AudioError(if start > 0.0 {
            format!("There's no audio in that part of the file: it's {} long.", clock(total))
        } else {
            "There's no audio in that part of the file.".into()
        }));
    }
    let focus = match options.focus.as_deref() {
        Some("mix") => "mix",
        Some("sound") => "sound",
        _ => {
            if length <= 20.0 {
                "sound"
            } else {
                "mix"
            }
        }
    };
    source.seek(start * rate);
    let frames = (length * rate).floor() as usize;
    let mut meter = Meter::new(rate, channels, frames);
    let mut mono = if focus == "sound" || options.transcribe {
        Some(vec![0f32; frames.min(if focus == "sound" { frames } else { (rate * 60.0) as usize })])
    } else {
        None
    };
    let mut done = 0;
    while done < frames {
        if let Some(signal) = &options.signal {
            signal.check()?;
        }
        let Some(block) = source.read(65536.min(frames - done)).await? else {
            break;
        };
        meter.push(&block, mono.as_deref_mut(), done);
        done += block[0].len();
        tokio::task::yield_now().await;
    }
    if (done as f64) < rate * 0.02 {
        return Err(AudioError("There's no audio in that part of the file.".into()));
    }
    let mut analysis = meter.finish(done);
    analysis.file = name.rsplit(['\\', '/']).next().unwrap_or(name).into();
    analysis.format = source.format.clone();
    analysis.seconds = round(total, 2);
    analysis.analyzed = Analyzed { from: clock(start), to: clock(start + done as f64 / rate), focus: focus.into() };
    if let Some(mono) = mono {
        let mono = &mono[..done.min(mono.len())];
        if focus == "sound" {
            analysis.sound = Some(analyze_sound(mono, rate, analysis.tempo.as_ref().map(|t| t.bpm)));
        }
        if options.transcribe {
            analysis.notes = Some(transcribe(mono, rate));
        }
    }
    Ok(analysis)
}
const FRAME: usize = 4096;
const COLUMNS: usize = 24;
struct Meter {
    rate: f64,
    channels: usize,
    filters: Vec<[Biquad; 2]>,
    block100: usize,
    squares: Vec<Vec<f64>>,
    partial: Vec<f64>,
    filled: usize,
    sample_peak: f64,
    true_peak: f64,
    clipped: usize,
    peakers: Vec<TruePeak>,
    peak_watch: Vec<usize>,
    sum_squares: f64,
    sum_lr: f64,
    sum_ll: f64,
    sum_rr: f64,
    left: Vec<f64>,
    right: Vec<f64>,
    ring: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    band_of: Vec<Option<usize>>,
    mid: [f64; 10],
    side: [f64; 10],
    cross: [f64; 10],
    left_power: [f64; 10],
    right_power: [f64; 10],
    slices: Vec<[f64; 10]>,
    chroma: [f64; 12],
    chroma_of: Vec<Option<usize>>,
    centroid_sum: f64,
    centroid_weight: f64,
    spectra: usize,
    slice_frames: usize,
    onset_low: Biquad,
    onset_high: Biquad,
    onset_energy: [f64; 3],
    onset_count: usize,
    onset_frames: Vec<[f64; 3]>,
    position: usize,
}
impl Meter {
    fn new(rate: f64, channels: usize, frames: usize) -> Self {
        let mut band_of = vec![None; FRAME / 2 + 1];
        let mut chroma_of = vec![None; FRAME / 2 + 1];
        for bin in 1..=FRAME / 2 {
            let hz = bin as f64 * rate / FRAME as f64;
            band_of[bin] = BANDS.iter().position(|b| hz >= b.from && hz < b.to);
            if (55.0..=4500.0).contains(&hz) {
                chroma_of[bin] = Some((js_round(69.0 + 12.0 * (hz / 440.0).log2()) as i64).rem_euclid(12) as usize);
            }
        }
        Self {
            rate,
            channels,
            filters: (0..channels).map(|_| k_weighting(rate)).collect(),
            block100: js_round(rate / 10.0) as usize,
            squares: Vec::new(),
            partial: vec![0.0; channels],
            filled: 0,
            sample_peak: 0.0,
            true_peak: 0.0,
            clipped: 0,
            peakers: vec![TruePeak::new(); channels],
            peak_watch: vec![0; channels],
            sum_squares: 0.0,
            sum_lr: 0.0,
            sum_ll: 0.0,
            sum_rr: 0.0,
            left: vec![0.0; FRAME],
            right: vec![0.0; FRAME],
            ring: 0,
            re: vec![0.0; FRAME],
            im: vec![0.0; FRAME],
            band_of,
            mid: [0.0; 10],
            side: [0.0; 10],
            cross: [0.0; 10],
            left_power: [0.0; 10],
            right_power: [0.0; 10],
            slices: Vec::new(),
            chroma: [0.0; 12],
            chroma_of,
            centroid_sum: 0.0,
            centroid_weight: 0.0,
            spectra: 0,
            slice_frames: FRAME.max(frames.div_ceil(COLUMNS)),
            onset_low: low_pass(rate, 150.0),
            onset_high: high_pass(rate, 2500.0),
            onset_energy: [0.0; 3],
            onset_count: 0,
            onset_frames: Vec::new(),
            position: 0,
        }
    }
    fn push(&mut self, block: &[Vec<f32>], mut keep_mono: Option<&mut [f32]>, offset: usize) {
        let left = &block[0];
        let right = block.get(1).unwrap_or(left);
        for index in 0..left.len() {
            let l = left[index] as f64;
            let r = right[index] as f64;
            for (channel, samples) in block.iter().enumerate() {
                let sample = samples[index] as f64;
                let [shelf, high] = &mut self.filters[channel];
                let weighted = high.process(shelf.process(sample));
                self.partial[channel] += weighted * weighted;
                let magnitude = sample.abs();
                if magnitude > self.sample_peak {
                    self.sample_peak = magnitude;
                }
                if magnitude >= 0.999 {
                    self.clipped += 1;
                }
                let peaker = &mut self.peakers[channel];
                peaker.push(sample);
                if magnitude > self.true_peak * 0.5 {
                    self.peak_watch[channel] = 12;
                }
                if self.peak_watch[channel] > 0 {
                    self.peak_watch[channel] -= 1;
                    self.true_peak = self.true_peak.max(peaker.peak()).max(magnitude);
                }
            }
            self.filled += 1;
            if self.filled == self.block100 {
                self.squares.push(self.partial.iter().map(|v| v / self.block100 as f64).collect());
                self.partial.fill(0.0);
                self.filled = 0;
            }
            let mono = (l + r) / 2.0;
            self.sum_squares += mono * mono;
            self.sum_lr += l * r;
            self.sum_ll += l * l;
            self.sum_rr += r * r;
            if let Some(keep) = keep_mono.as_deref_mut() {
                if let Some(sample) = keep.get_mut(offset + index) {
                    *sample = mono as f32;
                }
            }
            self.left[self.ring] = l;
            self.right[self.ring] = r;
            self.ring += 1;
            if self.ring == FRAME {
                self.spectrum();
                self.ring = 0;
            }
            let low = self.onset_low.process(mono);
            let high = self.onset_high.process(mono);
            self.onset_energy[0] += low * low;
            self.onset_energy[1] += mono * mono;
            self.onset_energy[2] += high * high;
            self.onset_count += 1;
            if self.onset_count == 512 {
                self.onset_frames.push(self.onset_energy);
                self.onset_energy = [0.0; 3];
                self.onset_count = 0;
            }
            self.position += 1;
        }
    }
    fn spectrum(&mut self) {
        let hann = window(FRAME, Window::Hann);
        for index in 0..FRAME {
            self.re[index] = self.left[index] * hann[index];
            self.im[index] = self.right[index] * hann[index];
        }
        fft(&mut self.re, &mut self.im);
        let slice =
            ((self.position as f64 - FRAME as f64 / 2.0) / self.slice_frames as f64).floor().clamp(0.0, (COLUMNS - 1) as f64) as usize;
        self.slices.resize(self.slices.len().max(slice + 1), [0.0; 10]);
        let (mut weighted, mut power) = (0.0, 0.0);
        for bin in 1..=FRAME / 2 {
            let mirror = (FRAME - bin) % FRAME;
            let (zr, zi, wr, wi) = (self.re[bin], self.im[bin], self.re[mirror], self.im[mirror]);
            let (lr, li, rr, ri) = ((zr + wr) / 2.0, (zi - wi) / 2.0, (zi + wi) / 2.0, (wr - zr) / 2.0);
            let lp = lr * lr + li * li;
            let rp = rr * rr + ri * ri;
            let (mr, mi, sr, si) = ((lr + rr) / 2.0, (li + ri) / 2.0, (lr - rr) / 2.0, (li - ri) / 2.0);
            let mid = mr * mr + mi * mi;
            let side = sr * sr + si * si;
            let total = if self.channels > 1 { mid } else { lp };
            if let Some(band) = self.band_of[bin] {
                self.slices[slice][band] += total;
                self.mid[band] += mid;
                self.side[band] += side;
                self.cross[band] += lr * rr + li * ri;
                self.left_power[band] += lp;
                self.right_power[band] += rp;
            }
            if let Some(class) = self.chroma_of[bin] {
                self.chroma[class] += total.sqrt();
            }
            weighted += bin as f64 * self.rate / FRAME as f64 * total;
            power += total;
        }
        if power > 1e-12 {
            self.centroid_sum += weighted;
            self.centroid_weight += power;
        }
        self.spectra += 1;
    }
    fn slice_sum(&self, band: usize) -> f64 {
        self.slices.iter().map(|s| s[band]).sum()
    }
    fn finish(&mut self, frames: usize) -> Analysis {
        if self.spectra == 0 && self.ring > 0 {
            self.left[self.ring..].fill(0.0);
            self.right[self.ring..].fill(0.0);
            self.position = self.position.max(FRAME / 2);
            self.spectrum();
        }
        let seconds = frames as f64 / self.rate;
        let (integrated, short, range) = gated_loudness(&self.squares);
        let rms = (self.sum_squares / frames.max(1) as f64).sqrt();
        let total_power = (0..10).map(|b| self.slice_sum(b)).sum::<f64>();
        let bands: Vec<_> = BANDS
            .iter()
            .enumerate()
            .map(|(index, band)| {
                let power = self.slice_sum(index);
                let (mid, side) = (self.mid[index], self.side[index]);
                let cross = self.cross[index] / (self.left_power[index] * self.right_power[index]).max(1e-30).sqrt();
                BandLevel {
                    name: band.name.into(),
                    hz: format!(
                        "{}–{}",
                        to_string(band.from),
                        if band.to >= 1000.0 { format!("{}k", to_string(band.to / 1000.0)) } else { to_string(band.to) }
                    ),
                    db: round(db(power / total_power.max(1e-30)), 1),
                    width: if self.channels > 1 { round(side / (mid + side).max(1e-30), 2) } else { 0.0 },
                    correlation: if self.channels > 1 { round(cross.clamp(-1.0, 1.0), 2) } else { 1.0 },
                }
            })
            .collect();
        let points: Vec<_> = BANDS
            .iter()
            .enumerate()
            .map(|(i, b)| ((b.from * b.to).sqrt().log2(), db(self.slice_sum(i) / (b.to / b.from).log2())))
            .filter(|(x, y)| *y > -150.0 && 2f64.powf(*x) < self.rate / 2.0)
            .collect();
        let tilt = slope(&points);
        let correlation = self.sum_lr / (self.sum_ll * self.sum_rr).max(1e-30).sqrt();
        let low_width = (self.side[0] + self.side[1]) / (self.mid[0] + self.mid[1] + self.side[0] + self.side[1]).max(1e-30);
        let onsets = onset_strength(&self.onset_frames);
        let stereo = if self.channels > 1 {
            Some(Stereo {
                correlation: round(correlation, 2),
                width: round(bands.iter().map(|b| b.width).sum::<f64>() / bands.len() as f64, 2),
                low_end_mono: low_width < 0.05,
            })
        } else {
            None
        };
        Analysis {
            kumi_audio: ANALYSIS_VERSION,
            file: String::new(),
            format: String::new(),
            sample_rate: self.rate,
            channels: self.channels,
            seconds,
            analyzed: Analyzed { from: String::new(), to: String::new(), focus: String::new() },
            loudness: Loudness {
                integrated_lufs: integrated.map(|v| round(v, 1)),
                short_term_max_lufs: short.map(|v| round(v, 1)),
                range_lu: range.map(|v| round(v, 1)),
                true_peak_dbtp: round(db_amplitude(self.true_peak), 1),
                sample_peak_dbfs: round(db_amplitude(self.sample_peak), 1),
                clipped_samples: self.clipped,
            },
            balance: Balance {
                bands,
                tilt_db_per_octave: round(tilt, 1),
                centroid_hz: js_round(self.centroid_sum / self.centroid_weight.max(1e-30)),
            },
            stereo,
            dynamics: Dynamics {
                crest_db: round(db_amplitude(self.sample_peak) - db_amplitude(rms), 1),
                peak_to_loudness_db: integrated.map(|v| round(db_amplitude(self.true_peak) - v, 1)),
                onsets_per_second: round(onset_peaks(&onsets).len() as f64 / seconds.max(0.1), 1),
            },
            tempo: if seconds >= 6.0 { estimate_tempo(&onsets, self.rate / 512.0) } else { None },
            key: estimate_key(&self.chroma),
            over_time: over_time(&self.squares, self.channels, seconds),
            spectrogram: self.picture(seconds),
            timeline: Some(self.timeline(&onsets)),
            sound: None,
            notes: None,
        }
    }
    fn timeline(&self, onsets: &[f64]) -> Timeline {
        let pool = onsets.len().div_ceil(2000).max(1);
        let peak = onsets.iter().copied().fold(1e-9, f64::max);
        let mut onset = Vec::new();
        let mut level = Vec::new();
        for at in (0..onsets.len()).step_by(pool) {
            let (mut strongest, mut energy) = (0f64, 0.0);
            for (index, value) in onsets.iter().enumerate().skip(at).take(pool) {
                strongest = strongest.max(*value);
                energy += self.onset_frames.get(index).map_or(0.0, |f| f[1]);
            }
            onset.push(round(strongest / peak, 2));
            level.push(round(db(energy / (512 * pool) as f64).max(-90.0), 1));
        }
        Timeline { step: round(512.0 * pool as f64 / self.rate, 4), onset, level }
    }
    fn picture(&self, seconds: f64) -> Spectrogram {
        let cells: Vec<Vec<f64>> = self.slices.iter().map(|s| s.iter().map(|v| db(*v)).collect()).collect();
        let loudest = cells.iter().flatten().copied().filter(|v| *v > -150.0).fold(f64::NEG_INFINITY, f64::max);
        let columns = self.slices.len().max(1);
        let rows = BANDS
            .iter()
            .enumerate()
            .map(|(index, band)| {
                let cells = (0..columns)
                    .map(|column| match cells.get(column).and_then(|c| c.get(index)) {
                        Some(value) if *value >= -150.0 && loudest.is_finite() => {
                            let level = js_round(9.0 - (loudest - value) / 3.0);
                            if level < 0.0 {
                                "·".into()
                            } else {
                                to_string(level)
                            }
                        }
                        _ => "·".into(),
                    })
                    .collect::<Vec<String>>()
                    .join("");
                SpectrogramRow { band: band.name.into(), cells }
            })
            .rev()
            .collect();
        Spectrogram {
            rows,
            columns: format!("{} to {}, {columns} steps", clock(0.0), clock(seconds)),
            scale: "9 = loudest, each step 3 dB quieter, · = silent".into(),
        }
    }
}
fn low_pass(rate: f64, hz: f64) -> Biquad {
    let k = (PI * hz / rate).tan();
    let norm = 1.0 / (1.0 + k / FRAC_1_SQRT_2 + k * k);
    Biquad::new(k * k * norm, 2.0 * k * k * norm, k * k * norm, 2.0 * (k * k - 1.0) * norm, (1.0 - k / FRAC_1_SQRT_2 + k * k) * norm)
}
fn high_pass(rate: f64, hz: f64) -> Biquad {
    let k = (PI * hz / rate).tan();
    let norm = 1.0 / (1.0 + k / FRAC_1_SQRT_2 + k * k);
    Biquad::new(norm, -2.0 * norm, norm, 2.0 * (k * k - 1.0) * norm, (1.0 - k / FRAC_1_SQRT_2 + k * k) * norm)
}
fn slope(points: &[(f64, f64)]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mx = points.iter().map(|v| v.0).sum::<f64>() / points.len() as f64;
    let my = points.iter().map(|v| v.1).sum::<f64>() / points.len() as f64;
    let num = points.iter().map(|(x, y)| (x - mx) * (y - my)).sum::<f64>();
    let den = points.iter().map(|(x, _)| (x - mx).powi(2)).sum::<f64>();
    if den != 0.0 {
        num / den
    } else {
        0.0
    }
}
fn gated_loudness(squares: &[Vec<f64>]) -> (Option<f64>, Option<f64>, Option<f64>) {
    let block = |from: usize, count: usize| {
        let sum = (0..squares.first().map_or(0, Vec::len))
            .map(|c| squares[from..from + count].iter().map(|v| v[c]).sum::<f64>() / count as f64)
            .sum::<f64>();
        (sum, -0.691 + db(sum))
    };
    let absolute: Vec<_> = (0..squares.len().saturating_sub(3)).map(|i| block(i, 4)).filter(|b| b.1 > -70.0).collect();
    let mut integrated = None;
    if !absolute.is_empty() {
        let relative = -0.691 + db(absolute.iter().map(|b| b.0).sum::<f64>() / absolute.len() as f64) - 10.0;
        let gated: Vec<_> = absolute.iter().filter(|b| b.1 > relative).collect();
        if !gated.is_empty() {
            integrated = Some(-0.691 + db(gated.iter().map(|b| b.0).sum::<f64>() / gated.len() as f64));
        }
    }
    let audible: Vec<_> = (0..squares.len().saturating_sub(29)).step_by(10).map(|i| block(i, 30).1).filter(|v| *v > -70.0).collect();
    let mut range = None;
    if audible.len() >= 2 {
        let gate = -0.691 + db(audible.iter().map(|v| 10f64.powf((v + 0.691) / 10.0)).sum::<f64>() / audible.len() as f64) - 20.0;
        let kept: Vec<_> = audible.iter().copied().filter(|v| *v > gate).collect();
        if kept.len() >= 2 {
            range = Some(percentile(&kept, 0.95) - percentile(&kept, 0.1));
        }
    }
    (integrated, audible.iter().copied().reduce(f64::max), range)
}
fn over_time(squares: &[Vec<f64>], channels: usize, seconds: f64) -> OverTime {
    let parts = (squares.len() / 10).clamp(1, 16);
    let per = squares.len() as f64 / parts as f64;
    let lufs = (0..parts)
        .map(|part| {
            let from = (part as f64 * per).floor() as usize;
            let to = from.saturating_add(1).max(((part + 1) as f64 * per).floor() as usize);
            let sum = (0..channels)
                .map(|c| (from..to).map(|i| squares.get(i).map_or(0.0, |s| s[c])).sum::<f64>() / (to - from) as f64)
                .sum::<f64>();
            let value = -0.691 + db(sum);
            if value > -70.0 {
                Some(round(value, 1))
            } else {
                None
            }
        })
        .collect();
    OverTime { every: format!("{} s", to_string(round(seconds / parts as f64, 1))), lufs }
}
pub(crate) fn onset_strength(frames: &[[f64; 3]]) -> Vec<f64> {
    let mut out = vec![0.0; frames.len()];
    for i in 1..frames.len() {
        for b in 0..3 {
            out[i] += ((1e-9 + frames[i][b]).log10() - (1e-9 + frames[i - 1][b]).log10()).max(0.0) * if b == 0 { 1.5 } else { 1.0 };
        }
    }
    let mut smooth = vec![0.0; out.len()];
    let mut running = 0.0;
    for i in 0..out.len() {
        running += out[i];
        if i >= 16 {
            running -= out[i - 16];
        }
        smooth[i] = (out[i] - running / (i + 1).min(16) as f64).max(0.0);
    }
    smooth
}
pub(crate) fn onset_peaks(strength: &[f64]) -> Vec<usize> {
    if strength.len() < 3 {
        return vec![];
    }
    let threshold = percentile(strength, 0.5) + 2.0 * (strength.iter().map(|v| v * v).sum::<f64>() / strength.len() as f64).sqrt();
    let mut peaks = Vec::new();
    let mut last = -10i64;
    for i in 1..strength.len() - 1 {
        let v = strength[i];
        if v > threshold && v >= strength[i - 1] && v >= strength[i + 1] && i as i64 - last >= 4 {
            peaks.push(i);
            last = i as i64;
        }
    }
    peaks
}
pub fn estimate_tempo(strength: &[f64], rate: f64) -> Option<Tempo> {
    let min = (rate * 60.0 / 200.0).floor() as usize;
    let max = rate.ceil() as usize;
    if strength.len() < max * 3 {
        return None;
    }
    let mean = strength.iter().sum::<f64>() / strength.len() as f64;
    let centered: Vec<_> = strength.iter().map(|v| v - mean).collect();
    let zero = centered.iter().map(|v| v * v).sum::<f64>();
    if zero <= 0.0 {
        return None;
    }
    let mut scores = vec![0.0; max + 2];
    for lag in min..=max + 1 {
        scores[lag] = (lag..centered.len()).map(|i| centered[i] * centered[i - lag]).sum::<f64>() / zero;
    }
    let mut best = None;
    let mut best_score = f64::NEG_INFINITY;
    for (lag, value) in scores.iter().enumerate().take(max + 1).skip(min) {
        let bpm = 60.0 * rate / lag as f64;
        let score = value * (-0.5 * ((bpm / 125.0).log2() / 0.9).powi(2)).exp();
        if score > best_score {
            best_score = score;
            best = Some(lag);
        }
    }
    let best = best?;
    if scores[best] <= 0.02 {
        return None;
    }
    let a = scores[best.saturating_sub(1)];
    let b = scores[best];
    let c = scores[best + 1];
    let shift = if a - 2.0 * b + c != 0.0 { 0.5 * (a - c) / (a - 2.0 * b + c) } else { 0.0 };
    Some(Tempo { bpm: round(60.0 * rate / (best as f64 + shift.clamp(-0.5, 0.5)), 1), confidence: round((scores[best] * 2.0).min(1.0), 2) })
}
pub fn estimate_key(chroma: &[f64]) -> Option<Key> {
    if chroma.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let major = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    let minor = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut scores = Vec::new();
    for (mode, profile) in [("major", major), ("minor", minor)] {
        for tonic in 0..12 {
            let x: Vec<_> = (0..12).map(|i| chroma[(i + tonic) % 12]).collect();
            let mx = x.iter().sum::<f64>() / 12.0;
            let my = profile.iter().sum::<f64>() / 12.0;
            let (mut num, mut dx, mut dy) = (0.0, 0.0, 0.0);
            for i in 0..12 {
                num += (x[i] - mx) * (profile[i] - my);
                dx += (x[i] - mx).powi(2);
                dy += (profile[i] - my).powi(2);
            }
            scores.push((format!("{} {mode}", PITCH_CLASSES[tonic]), num / (dx * dy).max(1e-30).sqrt()));
        }
    }
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    if scores[0].1 < 0.3 {
        return None;
    }
    Some(Key { name: scores[0].0.clone(), confidence: round(((scores[0].1 - scores[1].1) * 5.0 + scores[0].1 / 2.0).clamp(0.0, 1.0), 2) })
}
object!(PitchFrame { hz: f64, confidence: f64, at: usize });
pub fn track_pitch(mono: &[f32], rate: f64) -> Vec<PitchFrame> {
    let size = 2048;
    let max_lag = size;
    let fft_size = 4096;
    let mut out = Vec::new();
    let mut re = vec![0.0; fft_size];
    let mut im = vec![0.0; fft_size];
    let limit = mono.len().min((rate * 10.0) as usize);
    if limit < size + max_lag {
        return out;
    }
    for from in (0..=limit - size - max_lag).step_by(1024) {
        re.fill(0.0);
        im.fill(0.0);
        for i in 0..size + max_lag {
            re[i] = mono[from + i] as f64;
        }
        let energy = (0..size).map(|i| (mono[from + i] as f64).powi(2)).sum::<f64>();
        if energy / (size as f64) < 1e-7 {
            out.push(PitchFrame { hz: 0.0, confidence: 0.0, at: from });
            continue;
        }
        let mut kr = vec![0.0; fft_size];
        let mut ki = vec![0.0; fft_size];
        for i in 0..size {
            kr[i] = mono[from + i] as f64;
        }
        fft(&mut re, &mut im);
        fft(&mut kr, &mut ki);
        for bin in 0..fft_size {
            let (ar, ai, br, bi) = (re[bin], im[bin], kr[bin], -ki[bin]);
            re[bin] = ar * br - ai * bi;
            im[bin] = ar * bi + ai * br;
        }
        for v in &mut im {
            *v = -*v;
        }
        fft(&mut re, &mut im);
        let mut squares = vec![0.0; size + max_lag + 1];
        for i in 0..size + max_lag {
            squares[i + 1] = squares[i] + (mono[from + i] as f64).powi(2);
        }
        let window0 = squares[size];
        let mut running = 0.0;
        let mut best = None;
        let mut best_value = 1.0;
        let min_lag = 2.max((rate / 2000.0).floor() as usize);
        for lag in 1..max_lag {
            let shifted = squares[lag + size] - squares[lag];
            let difference = window0 + shifted - 2.0 * re[lag] / fft_size as f64;
            running += difference;
            let normalized = difference * lag as f64 / running.max(1e-12);
            if lag >= min_lag && normalized < 0.15 {
                let mut at = lag;
                let mut value = normalized;
                let mut cumulative = running;
                while at + 1 < max_lag {
                    let next_shift = squares[at + 1 + size] - squares[at + 1];
                    let next_difference = window0 + next_shift - 2.0 * re[at + 1] / fft_size as f64;
                    let next_running = cumulative + next_difference;
                    let next_value = next_difference * (at + 1) as f64 / next_running.max(1e-12);
                    if next_value >= value {
                        break;
                    }
                    at += 1;
                    value = next_value;
                    cumulative = next_running;
                }
                best = Some(at);
                best_value = value;
                break;
            }
        }
        out.push(match best {
            Some(best) => PitchFrame { hz: rate / best as f64, confidence: (1.0 - best_value).max(0.0), at: from },
            None => PitchFrame { hz: 0.0, confidence: 0.0, at: from },
        });
    }
    out
}
/// Pitch, harmonics, envelope and movement of one note or hit.
pub fn analyze_sound(mono: &[f32], rate: f64, bpm: Option<f64>) -> SoundAnalysis {
    let (envelope, steady) = amplitude_envelope(mono, rate);
    let tracked = track_pitch(mono, rate);
    let voiced: Vec<_> = tracked.iter().filter(|f| f.confidence > 0.6).collect();
    let (mut pitch, mut harmonics) = (None, None);
    if voiced.len() as f64 >= 3f64.max(tracked.len() as f64 * 0.25) {
        let hz = percentile(&voiced.iter().map(|f| f.hz).collect::<Vec<_>>(), 0.5);
        let note = note_of(hz);
        pitch = Some(Pitch {
            note: note.name,
            hz: round(hz, 1),
            cents: note.cents,
            confidence: round(voiced.len() as f64 / tracked.len().max(1) as f64, 2),
            movement: pitch_movement(&voiced, hz),
        });
        harmonics = partials(mono, rate, hz, steady);
    }
    SoundAnalysis { pitch, harmonics, envelope, movement: movement(mono, rate, bpm) }
}
fn amplitude_envelope(mono: &[f32], rate: f64) -> (Envelope, usize) {
    let step = (js_round(rate * 0.005) as usize).max(1);
    let levels: Vec<_> =
        mono.chunks(step).map(|chunk| (chunk.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt()).collect();
    let peak = levels.iter().copied().fold(1e-9, f64::max);
    let peak_at = levels.iter().position(|v| *v == peak).map_or(-1, |i| i as i64);
    let ms = |steps: i64| js_round(steps as f64 * step as f64 / rate * 1000.0);
    let start = levels.iter().position(|v| *v > peak * 0.01).unwrap_or(0) as i64;
    let attack_end = levels.iter().enumerate().position(|(i, v)| i as i64 >= start && *v >= peak * 0.9).map_or(-1, |i| i as i64);
    let end = levels.len() as i64 - 1 - levels.iter().rev().position(|v| *v > peak * 0.01).map_or(-1, |i| i as i64);
    let from = if peak_at < 0 { (levels.len() as i64 + peak_at).max(0) as usize } else { peak_at as usize };
    let after = &levels[from.min(levels.len())..((end + 1).max(0) as usize).min(levels.len())];
    let sustain = if after.len() > 8 {
        percentile(&after[(after.len() as f64 * 0.4).floor() as usize..(after.len() as f64 * 0.8).floor() as usize], 0.5)
    } else {
        peak
    };
    let decay_end = peak_at + after.iter().position(|v| *v <= sustain * 1.12).unwrap_or(0) as i64;
    let release_from = peak_at + after.len() as i64 - 1 - after.iter().rev().position(|v| *v >= sustain * 0.9).map_or(-1, |i| i as i64);
    (
        Envelope {
            attack_ms: ms((attack_end - start).max(0)),
            decay_ms: ms((decay_end - peak_at).max(0)),
            sustain_db: round(db_amplitude(sustain / peak), 1),
            release_ms: ms((end - release_from.max(decay_end)).max(0)),
            length_ms: ms((end - start).max(0)),
        },
        mono.len().saturating_sub(1).min(((decay_end.max(attack_end) + 2).max(0) as usize) * step),
    )
}
fn pitch_movement(voiced: &[&PitchFrame], hz: f64) -> Option<String> {
    let cents = |value: f64| 1200.0 * (value / hz).log2();
    let first: Vec<_> = voiced.iter().take(3).map(|f| cents(f.hz)).collect();
    let start = if first.is_empty() { 0.0 } else { percentile(&first, 0.5) };
    if start.abs() >= 150.0 {
        return Some(format!(
            "starts {} semitones {} and settles",
            to_string(round(start.abs() / 100.0, 1)),
            if start > 0.0 { "higher" } else { "lower" }
        ));
    }
    let deviations: Vec<_> = voiced.iter().map(|f| cents(f.hz)).collect();
    let spread = percentile(&deviations, 0.9) - percentile(&deviations, 0.1);
    if spread > 40.0 {
        Some(format!("wavers about {} cents (vibrato or drift)", to_string(js_round(spread / 2.0))))
    } else {
        None
    }
}
fn partials(mono: &[f32], rate: f64, f0: f64, steady_at: usize) -> Option<Harmonics> {
    let size = 16384;
    if mono.len() < 2048 {
        return None;
    }
    let from = steady_at.min(mono.len().saturating_sub(size));
    let mut re = vec![0.0; size];
    let mut im = vec![0.0; size];
    let taper = window(size.min(mono.len() - from), Window::BlackmanHarris);
    for i in 0..taper.len() {
        re[i] = mono[from + i] as f64 * taper[i];
    }
    fft(&mut re, &mut im);
    let magnitude = |bin: usize| re.get(bin).copied().unwrap_or(0.0).hypot(im.get(bin).copied().unwrap_or(0.0));
    let bin_hz = rate / size as f64;
    let mut found = Vec::new();
    for k in 1..=24 {
        let target = k as f64 * f0;
        if target > rate / 2.0 - 500.0 {
            break;
        }
        let reach = 2.max(js_round(target * 0.03 / bin_hz) as usize);
        let center = js_round(target / bin_hz) as usize;
        let (mut peak, mut peak_bin) = (0.0, center);
        for bin in center.saturating_sub(reach).max(1)..=(size / 2).min(center + reach) {
            let value = magnitude(bin);
            if value > peak {
                peak = value;
                peak_bin = bin;
            }
        }
        found.push((k, db_amplitude(peak), peak_bin as f64 * bin_hz));
    }
    if found.is_empty() {
        return None;
    }
    let strongest = found.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let relative: Vec<_> = found.iter().map(|p| round(p.1 - strongest, 1)).collect();
    let mut floor = Vec::new();
    for k in 1..found.len() {
        let between = js_round((k as f64 + 0.5) * f0 / bin_hz) as usize;
        if between < size / 2 {
            floor.push(db_amplitude(magnitude(between)));
        }
    }
    let noise_db = if floor.is_empty() { -90.0 } else { round(percentile(&floor, 0.5) - strongest, 1) };
    let audible: Vec<_> = found.iter().filter(|p| p.1 - strongest > -50.0).collect();
    let odd = audible.iter().filter(|p| p.0 % 2 == 1 && p.0 > 1).map(|p| 10f64.powf(p.1 / 10.0)).sum::<f64>();
    let even = audible.iter().filter(|p| p.0 % 2 == 0).map(|p| 10f64.powf(p.1 / 10.0)).sum::<f64>();
    let odd_to_even = round(if even > 0.0 { db(odd / even) } else { 60.0 }, 1);
    let slope_value = slope(&audible.iter().map(|p| ((p.0 as f64).log2(), p.1 - strongest)).collect::<Vec<_>>());
    let inharmonicity = round(
        audible.iter().skip(1).map(|p| (p.2 / (p.0 as f64 * f0) - 1.0).abs()).sum::<f64>() / audible.len().saturating_sub(1).max(1) as f64,
        3,
    );
    let significant = relative.iter().filter(|v| **v > -30.0).count();
    let shape = if inharmonicity > 0.02 {
        "inharmonic (FM, bell or metallic)"
    } else if noise_db > -20.0 {
        "noisy (noise, breath or heavy distortion)"
    } else if significant <= 2 && relative[0] > -3.0 {
        "sine-like (few harmonics)"
    } else if odd_to_even > 12.0 {
        if slope_value < -9.0 {
            "triangle-like (odd harmonics, falling fast)"
        } else {
            "square- or pulse-like (odd harmonics)"
        }
    } else if slope_value > -4.0 {
        "bright, rich (saw-like or distorted)"
    } else if slope_value > -8.0 {
        "saw-like (all harmonics)"
    } else {
        "soft or filtered (harmonics fall fast)"
    };
    Some(Harmonics {
        partials: relative.into_iter().take(16).collect(),
        shape: shape.into(),
        slope_db_per_octave: round(slope_value, 1),
        odd_to_even,
        noise_db,
        inharmonicity,
    })
}
fn movement(mono: &[f32], rate: f64, bpm: Option<f64>) -> Movement {
    let size = 1024;
    let hop = 256;
    let mut re = vec![0.0; size];
    let mut im = vec![0.0; size];
    let hann = window(size, Window::Hann);
    let (mut brightness, mut levels) = (Vec::new(), Vec::new());
    if mono.len() >= size {
        for from in (0..=mono.len() - size).step_by(hop).take(4000) {
            let mut energy = 0.0;
            for i in 0..size {
                let value = mono[from + i] as f64;
                re[i] = value * hann[i];
                im[i] = 0.0;
                energy += value * value;
            }
            if energy / (size as f64) < 1e-7 {
                brightness.push(f64::NAN);
                levels.push(0.0);
                continue;
            }
            fft(&mut re, &mut im);
            let (mut weighted, mut power) = (0.0, 0.0);
            for bin in 1..size / 2 {
                let p = re[bin] * re[bin] + im[bin] * im[bin];
                weighted += bin as f64 * p;
                power += p;
            }
            brightness.push(weighted / power.max(1e-30) * rate / size as f64);
            levels.push((energy / size as f64).sqrt());
        }
    }
    let valid: Vec<_> = brightness.iter().copied().filter(|v| v.is_finite()).collect();
    if valid.len() < 8 {
        return Movement { brightness: "too short to tell".into(), lfo: None };
    }
    let third = valid.len() / 3;
    let early = percentile(&valid[..third.max(1)], 0.5);
    let middle = percentile(&valid[third..(third + 1).max(2 * third)], 0.5);
    let late = percentile(&valid[2 * third..], 0.5);
    let ratio = |a: f64, b: f64| 12.0 * (a.max(1.0) / b.max(1.0)).log2();
    let change = ratio(late, early);
    let n = |v| to_string(js_round(v));
    let text = if change.abs() < 2.0 && ratio(middle, early).abs() < 2.0 {
        format!("steady, around {} Hz", n(middle))
    } else if middle > early * 1.25 && middle > late * 1.25 {
        format!("opens then closes ({} → {} → {} Hz)", n(early), n(middle), n(late))
    } else if change > 0.0 {
        format!("opens over the note ({} → {} Hz)", n(early), n(late))
    } else {
        format!("closes over the note ({} → {} Hz)", n(early), n(late))
    };
    let rate = rate / hop as f64;
    let lfo = periodicity(&valid.iter().map(|v| v.max(1.0).log2()).collect::<Vec<_>>(), rate, 0.5, 20.0);
    let silent = levels.iter().filter(|v| **v == 0.0).count() as f64 / levels.len().max(1) as f64;
    let level_lfo =
        if silent > 0.2 { None } else { periodicity(&levels.iter().map(|v| (1e-6 + v).log10()).collect::<Vec<_>>(), rate, 0.5, 20.0) };
    let chosen = match (lfo, level_lfo) {
        (Some(lfo), other) if other.is_none_or(|level| lfo.1 >= level.1) => Some((lfo, "brightness (filter)")),
        (_, Some(level)) => Some((level, "level (tremolo or sidechain)")),
        _ => None,
    };
    Movement {
        brightness: text,
        lfo: chosen.filter(|(v, _)| v.1 > 0.35).map(|((hz, strength), on)| Lfo {
            hz: round(hz, 2),
            on: on.into(),
            depth: if strength > 0.7 { "strong" } else { "moderate" }.into(),
            at_tempo: bpm.filter(|v| *v != 0.0).and_then(|bpm| note_value(hz, bpm)),
        }),
    }
}
fn periodicity(series: &[f64], rate: f64, low_hz: f64, high_hz: f64) -> Option<(f64, f64)> {
    let low_hz = low_hz.max(2.0 * rate / series.len() as f64);
    if low_hz >= high_hz || series.len() < 8 {
        return None;
    }
    let mean = series.iter().sum::<f64>() / series.len() as f64;
    let centered: Vec<_> = series.iter().map(|v| v - mean).collect();
    let zero = centered.iter().map(|v| v * v).sum::<f64>();
    if zero <= 1e-12 {
        return None;
    }
    let min = 1.max((rate / high_hz).floor() as usize);
    let max = (series.len() - 1).min((rate / low_hz).ceil() as usize);
    let mut scores = vec![0.0; max + 2];
    let (mut best, mut best_score) = (0, 0.0);
    for lag in min..=max {
        let sum = (lag..centered.len()).map(|i| centered[i] * centered[i - lag]).sum::<f64>();
        scores[lag] = sum / zero * centered.len() as f64 / (centered.len() - lag) as f64;
        if scores[lag] > best_score {
            best_score = scores[lag];
            best = lag;
        }
    }
    if best == 0 {
        return None;
    }
    for divisor in [4, 3, 2] {
        let guess = js_round(best as f64 / divisor as f64) as usize;
        let (mut local, mut score) = (0, 0.0);
        for (lag, value) in scores.iter().enumerate().take(max.min(guess + 2) + 1).skip(min.max(guess.saturating_sub(2))) {
            if *value > score {
                score = *value;
                local = lag;
            }
        }
        if local > 0 && score >= best_score * 0.8 {
            best = local;
            best_score = score;
            break;
        }
    }
    Some((rate / best as f64, best_score.min(1.0)))
}
/// A modulation's note value at a tempo, when close to one.
pub fn note_value(hz: f64, bpm: f64) -> Option<String> {
    let beat = bpm / 60.0;
    let (mut best, mut error) = (None, 0.06);
    for (name, per) in [
        ("1 bar", 0.25),
        ("1/2", 0.5),
        ("1/4", 1.0),
        ("1/4 triplet", 1.5),
        ("1/8", 2.0),
        ("1/8 triplet", 3.0),
        ("1/16", 4.0),
        ("1/16 triplet", 6.0),
        ("1/32", 8.0),
    ] {
        let found = (hz / (beat * per)).log2().abs();
        if found < error {
            error = found;
            best = Some(name.into());
        }
    }
    best
}
fn note_starts(mono: &[f32], rate: f64, hop: usize) -> Vec<usize> {
    let level: Vec<_> = mono
        .chunks(hop)
        .map(|c| 10.0 * (c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64 + 1e-12).log10())
        .collect();
    let loudest = level.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let back = 1.max(js_round(0.025 * rate / hop as f64) as usize);
    let gap = js_round(0.04 * rate / hop as f64) as i64;
    let rise: Vec<_> = level
        .iter()
        .enumerate()
        .map(|(i, v)| v - if i > 0 { level[i.saturating_sub(back)..i].iter().copied().fold(f64::INFINITY, f64::min) } else { -120.0 })
        .collect();
    let mut starts = Vec::new();
    let mut last = -gap;
    for i in 0..level.len() {
        if rise[i] < 6.0 || level[i] < loudest - 45.0 || i as i64 - last < gap {
            continue;
        }
        if rise[i] < rise.get(i + 1).copied().unwrap_or(f64::NEG_INFINITY) {
            continue;
        }
        starts.push(i);
        last = i as i64;
    }
    starts
}
/// Each onset's settled pitch, strength against the loudest, and duration to its decay or next note.
pub fn transcribe(mono: &[f32], rate: f64) -> Vec<HeardNote> {
    let hop = 256;
    let peaks = note_starts(mono, rate, hop);
    let rms = |from: usize, length: usize| {
        let to = mono.len().min(from + length);
        let sum = mono.get(from..to).unwrap_or(&[]).iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
        (sum / to.saturating_sub(from).max(1) as f64).sqrt()
    };
    let mut found = Vec::new();
    for (index, peak) in peaks.iter().enumerate() {
        let start = (peak * hop).saturating_sub(hop);
        let next = peaks.get(index + 1).map_or(mono.len(), |p| p * hop);
        let slot = js_round(rate * 0.005) as usize;
        let levels: Vec<_> = (0..12).map(|step| rms(start + step * slot, slot)).collect();
        let loudest = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let loudest_at = levels.iter().position(|v| *v == loudest).unwrap_or(0);
        let attack = db_amplitude(levels[loudest_at].max(1e-9));
        let window = js_round(rate * 0.01) as usize;
        let mut end = start + loudest_at * slot;
        while end + window < next.min(start + (rate * 2.0) as usize) && db_amplitude(rms(end, window)) > attack - 18.0 {
            end += window;
        }
        let from = start + js_round(rate * 0.02) as usize;
        let tracked: Vec<_> = if from < mono.len() {
            track_pitch(&mono[from..mono.len().min(from + js_round(rate * 0.15) as usize + 4096)], rate)
                .into_iter()
                .filter(|f| f.confidence > 0.6 && f.hz > 30.0)
                .collect()
        } else {
            vec![]
        };
        let hz = if tracked.is_empty() { 0.0 } else { percentile(&tracked.iter().map(|f| f.hz).collect::<Vec<_>>(), 0.5) };
        let confidence = if tracked.is_empty() { 0.0 } else { percentile(&tracked.iter().map(|f| f.confidence).collect::<Vec<_>>(), 0.5) };
        found.push((
            HeardNote {
                time: start as f64 / rate,
                duration: 0.02f64.max((end - start) as f64 / rate),
                midi: if hz > 0.0 { Some(note_of(hz).midi) } else { None },
                velocity: 0.0,
                confidence,
            },
            attack,
        ));
    }
    let loudest = found.iter().map(|(_, attack)| *attack).fold(-200.0, f64::max);
    found
        .into_iter()
        .map(|(mut note, attack)| {
            note.time = round(note.time, 3);
            note.duration = round(note.duration, 3);
            note.velocity = js_round(127.0 + 3.0 * (attack - loudest)).clamp(1.0, 127.0);
            note.confidence = round(note.confidence, 2);
            note
        })
        .collect()
}
