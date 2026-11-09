//! Sound design on a group of samples: **cross** two or more into one (blend, attack, resonate, modulate), **fuse**
//! them into a texture, a loop or a smear, **stack** them as one hit, or **grow** one part of the way toward another.
//! Every design draws its choices from a seed: the same request and seed render the same bytes, and another seed is
//! a variation (a reroll). A **steer** pushes any design one way before it renders. The result is 48 kHz stereo,
//! faded, finite, and at -1 dBTP; `Render::wav` writes it as 24-bit WAV.

pub mod analysis;
pub mod choices;
mod cross;
mod fuse;
mod fx;
mod grow;
pub mod rng;
pub mod schema;
mod stack;
pub mod steer;

use analysis::Stereo;
use choices::{Loop, Modulate, Resonate};
use kumi_common::abort::Signal;
use kumi_media::audio::{
    decode::{open_audio_to_with_options, DecodeOptions},
    dsp::{high_pass, resample},
    AudioError,
};
use rng::Rng;
use serde::{Deserialize, Serialize};
use std::path::Path;
use steer::Steer;

/// The rate every design works and renders at.
pub const RATE: f64 = 48_000.0;
pub const MIN_SOURCES: usize = 2;
/// The most of each source that's read, when a design has few.
pub const MAX_SOURCE_SECONDS: f64 = 30.0;
/// The least of each source that's read, however many a design has.
pub const MIN_SOURCE_SECONDS: f64 = 0.5;
/// The audio a design reads in all, across its sources: a design of many sounds reads less of each.
pub const SOURCES_SECONDS: f64 = 300.0;
/// A loop's most bars, and its longest length.
pub const MAX_BARS: u32 = 8;
pub const MAX_LOOP_SECONDS: f64 = 60.0;
/// The longest render asked for.
pub const MAX_SECONDS: f64 = 30.0;
/// The peaks an answer carries.
pub const PEAKS: usize = 256;
/// The loudness every source is brought to before it's designed with.
const SOURCE_LUFS: f64 = -20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Cross,
    Fuse,
    Stack,
    Grow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Blend,
    Attack,
    Resonate,
    Modulate,
    Texture,
    Loop,
    Smear,
}

impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Op::Cross => "cross",
            Op::Fuse => "fuse",
            Op::Stack => "stack",
            Op::Grow => "grow",
        }
    }
    /// The modes the op takes (stack and grow take none).
    pub fn modes(self) -> &'static [Mode] {
        match self {
            Op::Cross => &[Mode::Blend, Mode::Attack, Mode::Resonate, Mode::Modulate],
            Op::Fuse => &[Mode::Texture, Mode::Loop, Mode::Smear],
            Op::Stack | Op::Grow => &[],
        }
    }
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Blend => "blend",
            Mode::Attack => "attack",
            Mode::Resonate => "resonate",
            Mode::Modulate => "modulate",
            Mode::Texture => "texture",
            Mode::Loop => "loop",
            Mode::Smear => "smear",
        }
    }
    pub fn op(self) -> Op {
        match self {
            Mode::Blend | Mode::Attack | Mode::Resonate | Mode::Modulate => Op::Cross,
            Mode::Texture | Mode::Loop | Mode::Smear => Op::Fuse,
        }
    }
}

/// What a design may be told; everything is optional and drawn from the seed when it isn't given.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// The render's length in seconds (0.1 to 30); a loop's is its bars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    /// A loop's tempo (40 to 240; 120 when not given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    /// A loop's length in bars (1 to 4; 2 when not given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<u32>,
    /// Attack: which source (index into `sources`) gives the attack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_from: Option<usize>,
    /// Resonate: the sources (indices) that excite; the rest resonate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Vec<usize>>,
    /// How far the design goes, 0 to 1: modulation depth, resonance wet, a smear's diffusion, a texture's scatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<f64>,
    /// Smear: how many times longer than its sources (4 to 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch: Option<f64>,
    /// Grow: how far from the first sound toward the second, 0 to 1 (0.5 when not given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    /// Pushes any design one way: controls on the result and on each source (see `Steer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer: Option<Steer>,
    /// Resonate's choices: who rings through whom, how, and for how long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resonate: Option<Resonate>,
    /// Modulate's choices: which sound is shaped, by which ways, in how many stages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulate: Option<Modulate>,
    /// A loop's choices: a beat or a melody, its meter and groove, its key, scale and tuning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#loop: Option<Loop>,
}

/// How much of each source a design of `count` sounds reads: all 30 seconds when there are few, less as there are
/// more, so the whole design reads at most `SOURCES_SECONDS` of audio (never under half a second of each).
pub fn source_seconds(count: usize) -> f64 {
    (SOURCES_SECONDS / count.max(1) as f64).clamp(MIN_SOURCE_SECONDS, MAX_SOURCE_SECONDS)
}

/// A loop's length in seconds: its bars of its meter at its tempo (the tempo counts quarter notes).
pub fn loop_seconds(options: &Options) -> f64 {
    let bpm = options.bpm.unwrap_or(120.0);
    let bars = options.bars.unwrap_or(2) as f64;
    let [beats, unit] = options.r#loop.as_ref().and_then(|l| l.meter).unwrap_or([4, 4]);
    bars * beats as f64 * (4.0 / unit as f64) * 60.0 / bpm
}

/// A source as a request names it: a library sound or a made sound by id, or (the one-shot `kumi design`, whose caller
/// has resolved and approved it) a file. `name` is how the recipe calls it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound_id: Option<String>,
    /// The map snapshot a `sound_id` was read from (serve's sounds are named within one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl SourceRef {
    /// The source as an answer echoes it: its ids and name, never a file.
    pub fn echoed(&self) -> SourceRef {
        SourceRef { file: None, ..self.clone() }
    }
}

/// One request for every surface: serve's `design`, `kumi design --json`, the agent's tool and the plug-in's routes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    pub sources: Vec<SourceRef>,
    /// The variation: a reroll is the same request with another seed. Not given: the surface draws one and answers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default)]
    pub options: Options,
}

/// Why a design can't be made, in a line for the producer.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct DesignError(pub String);

impl From<AudioError> for DesignError {
    fn from(error: AudioError) -> Self {
        DesignError(error.0)
    }
}

fn refuse<T>(message: impl Into<String>) -> Result<T, DesignError> {
    Err(DesignError(message.into()))
}

/// The largest seed a request may carry: what a JSON number holds exactly.
pub const MAX_SEED: u64 = (1 << 53) - 1;

impl Request {
    /// Refuses a request no design can make, before anything is read.
    pub fn check(&self) -> Result<(), DesignError> {
        let modes = self.op.modes();
        match self.mode {
            None if !modes.is_empty() => {
                return refuse(format!(
                    "{} needs a mode: {}.",
                    self.op.name(),
                    modes.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
                ))
            }
            Some(mode) if mode.op() != self.op || modes.is_empty() => {
                return refuse(format!("{} is a mode of {}, not of {}.", mode.name(), mode.op().name(), self.op.name()))
            }
            _ => {}
        }
        let count = self.sources.len();
        if self.op == Op::Grow && count != 2 {
            return refuse(format!("grow takes 2 sounds, the one to grow from and the one to grow toward; this has {count}."));
        }
        if count < MIN_SOURCES {
            return refuse(format!("{} takes {MIN_SOURCES} or more sounds; this has {count}.", self.op.name()));
        }
        for source in &self.sources {
            let given = [&source.sound_id, &source.artifact_id, &source.file].iter().filter(|v| v.is_some()).count();
            if given != 1 || (source.map_revision.is_some() && source.sound_id.is_none()) {
                return refuse("Each sound is one of a sound_id (with its map_revision), an artifact_id or a file.");
            }
        }
        if self.seed.is_some_and(|seed| seed > MAX_SEED) {
            return refuse(format!("A seed is a whole number up to {MAX_SEED}."));
        }
        let o = &self.options;
        let within = |value: Option<f64>, low: f64, high: f64, what: &str| -> Result<(), DesignError> {
            match value {
                Some(v) if !(v.is_finite() && (low..=high).contains(&v)) => refuse(format!("{what} is {low} to {high}.")),
                _ => Ok(()),
            }
        };
        within(o.seconds, 0.1, MAX_SECONDS, "seconds")?;
        within(o.bpm, 40.0, 240.0, "bpm")?;
        within(o.depth, 0.0, 1.0, "depth")?;
        within(o.stretch, 4.0, 16.0, "stretch")?;
        within(o.at, 0.0, 1.0, "at")?;
        if o.bars.is_some_and(|bars| !(1..=MAX_BARS).contains(&bars)) {
            return refuse(format!("bars is 1 to {MAX_BARS}."));
        }
        if o.attack_from.is_some_and(|i| i >= count) {
            return refuse(format!("attack_from names a sound by its place, 0 to {}.", count - 1));
        }
        if let Some(split) = &o.split {
            let mut seen = vec![false; count];
            for &i in split {
                if i >= count || std::mem::replace(&mut seen[i], true) {
                    return refuse(format!("split lists the exciting sounds by place, each once, 0 to {}.", count - 1));
                }
            }
            if split.is_empty() || split.len() >= count {
                return refuse("split leaves at least one sound to excite and one to resonate.");
            }
        }
        if self.op == Op::Grow {
            let others = [
                o.bpm.is_some(),
                o.bars.is_some(),
                o.attack_from.is_some(),
                o.split.is_some(),
                o.depth.is_some(),
                o.stretch.is_some(),
                o.resonate.is_some(),
                o.modulate.is_some(),
                o.r#loop.is_some(),
            ];
            if others.contains(&true) {
                return refuse("grow takes at, seconds and steer.");
            }
        } else if o.at.is_some() {
            return refuse("at is grow's: how far from the first sound toward the second.");
        }
        if let Some(steer) = &o.steer {
            steer.check()?;
        }
        for (given, mode, what) in [
            (o.resonate.is_some(), Mode::Resonate, "resonate's"),
            (o.modulate.is_some(), Mode::Modulate, "modulate's"),
            (o.r#loop.is_some(), Mode::Loop, "loop's"),
        ] {
            if given && self.mode != Some(mode) {
                return refuse(format!("{what} choices are for {}:{}.", mode.op().name(), mode.name()));
            }
        }
        if let Some(resonate) = &o.resonate {
            resonate.check()?;
        }
        if let Some(modulate) = &o.modulate {
            modulate.check()?;
        }
        if let Some(looping) = &o.r#loop {
            looping.check()?;
        }
        if self.mode == Some(Mode::Loop) {
            let seconds = loop_seconds(o);
            if seconds > MAX_LOOP_SECONDS + 1e-9 {
                return refuse(format!(
                    "That loop would run {seconds:.0} s; a loop is at most {MAX_LOOP_SECONDS:.0} s: fewer bars, a shorter meter or a faster tempo."
                ));
            }
        }
        Ok(())
    }
}

/// A decoded source at the design rate.
#[derive(Clone)]
pub struct Source {
    pub name: String,
    pub audio: Stereo,
}

impl Source {
    /// A source from channels at any rate (one channel is used for both sides; past two, the first two are kept).
    pub fn new(name: impl Into<String>, channels: &[Vec<f32>], rate: f64) -> Self {
        let left = channels.first().cloned().unwrap_or_default();
        let right = channels.get(1).cloned().unwrap_or_else(|| left.clone());
        let most = (MAX_SOURCE_SECONDS * rate) as usize;
        let at = |c: Vec<f32>| {
            let mut c: Vec<f32> = c.into_iter().take(most).map(|v| if v.is_finite() { v } else { 0.0 }).collect();
            c = resample(&c, rate, RATE);
            c
        };
        Source { name: name.into(), audio: [at(left), at(right)] }
    }
}

/// Reads up to `seconds` of a file as a source (`source_seconds` says how much a design of so many sounds reads), WAV
/// and AIFF by Kumi's own reader; anything else only through the decoder `decode` allows.
pub async fn load_source(path: &Path, name: &str, seconds: f64, decode: &DecodeOptions, signal: Option<Signal>) -> Result<Source, AudioError> {
    let seconds = seconds.clamp(MIN_SOURCE_SECONDS, MAX_SOURCE_SECONDS);
    let mut audio = open_audio_to_with_options(path, signal, Some(seconds), decode).await?;
    let rate = audio.sample_rate;
    let wanted = (rate * seconds) as usize;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    while channels.first().map_or(0, Vec::len) < wanted {
        let Some(block) = audio.read(65_536).await? else { break };
        if channels.is_empty() {
            channels = vec![Vec::new(); block.len().clamp(1, 2)];
        }
        for (c, channel) in channels.iter_mut().enumerate() {
            channel.extend_from_slice(&block[c.min(block.len() - 1)]);
        }
    }
    let _ = audio.close().await;
    if channels.first().is_none_or(Vec::is_empty) {
        return Err(AudioError(format!("{name} has no audio.")));
    }
    for channel in channels.iter_mut() {
        channel.truncate(wanted);
    }
    Ok(Source::new(name, &channels, rate))
}

/// A source made ready: its first hit at its start (1 ms before), its tail cut where it dies, its loudness matched.
pub(crate) struct Prepared {
    pub name: String,
    pub audio: Stereo,
    pub mono: Vec<f32>,
    /// Its loudest moment near the start, samples.
    pub peak: usize,
}

impl Prepared {
    pub fn len(&self) -> usize {
        self.audio[0].len()
    }
}

fn prepare(source: &Source, most: usize) -> Result<Prepared, DesignError> {
    let read = source.audio[0].len().min(most);
    let whole: Stereo = [source.audio[0][..read].to_vec(), source.audio[1][..read].to_vec()];
    let mono = analysis::mono(&whole);
    let loudest = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if loudest < 1e-5 {
        return refuse(format!("{} is silent.", source.name));
    }
    let start = analysis::start_of(&mono).saturating_sub((0.001 * RATE) as usize);
    let end = analysis::end_of(&mono).max(start + (0.02 * RATE) as usize).min(mono.len());
    let mut audio: Stereo = [whole[0][start..end].to_vec(), whole[1][start..end].to_vec()];
    fx::fade_out(&mut audio, ((end - start) / 20).min((0.01 * RATE) as usize));
    let loudness = analysis::lufs(&audio);
    let gain = 10f64.powf(((SOURCE_LUFS - loudness) / 20.0).min(1.5));
    fx::scale(&mut audio, gain);
    let mono = analysis::mono(&audio);
    let peak = analysis::peak_after(&mono, 0, (0.05 * RATE) as usize);
    Ok(Prepared { name: source.name.clone(), audio, mono, peak })
}

/// How a render ends: a one-shot fades out over `fade` samples and loses its silent tail; a loop's end runs into its
/// start, so it isn't faded or trimmed.
pub(crate) enum Ending {
    OneShot { fade: usize },
    Loop,
}

/// A finished design.
pub struct Render {
    pub audio: Stereo,
    pub recipe: String,
}

impl Render {
    pub fn duration(&self) -> f64 {
        self.audio[0].len() as f64 / RATE
    }
    /// The render at a glance: `columns` peaks, each the loudest sample of its stretch, the loudest of all 1.
    pub fn peaks(&self, columns: usize) -> Vec<f32> {
        let len = self.audio[0].len();
        let mut peaks: Vec<f32> = (0..columns)
            .map(|c| {
                let (from, to) = (c * len / columns, ((c + 1) * len / columns).max(c * len / columns + 1).min(len));
                (from..to).map(|i| self.audio[0][i].abs().max(self.audio[1][i].abs())).fold(0.0, f32::max)
            })
            .collect();
        let loudest = peaks.iter().copied().fold(0.0, f32::max);
        if loudest > 0.0 {
            for p in &mut peaks {
                *p = (*p / loudest * 1e4).round() / 1e4;
            }
        }
        peaks
    }
    /// The render as a 48 kHz 24-bit stereo WAV.
    pub fn wav(&self) -> Vec<u8> {
        let frames = self.audio[0].len();
        let data = frames * 6;
        let mut wav = Vec::with_capacity(44 + data);
        wav.extend(b"RIFF");
        wav.extend((36 + data as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend((RATE as u32).to_le_bytes());
        wav.extend((RATE as u32 * 6).to_le_bytes());
        wav.extend(6u16.to_le_bytes());
        wav.extend(24u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((data as u32).to_le_bytes());
        for i in 0..frames {
            for channel in &self.audio {
                let v = (channel[i] as f64 * 8_388_607.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                wav.extend(&v.to_le_bytes()[..3]);
            }
        }
        wav
    }
}

/// What every surface answers about a render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The made sound's id (serve's artifact; the plug-in's own id for `kumi design`, which leaves it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    pub seed: u64,
    pub sources: Vec<SourceRef>,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u32,
    pub peaks: Vec<f32>,
    pub recipe: String,
}

impl Answer {
    pub fn new(request: &Request, seed: u64, render: &Render, artifact_id: Option<String>) -> Self {
        Answer {
            artifact_id,
            op: request.op,
            mode: request.mode,
            seed,
            sources: request.sources.iter().map(SourceRef::echoed).collect(),
            duration: (render.duration() * 1000.0).round() / 1000.0,
            sample_rate: RATE as u32,
            channels: 2,
            peaks: render.peaks(PEAKS),
            recipe: render.recipe.clone(),
        }
    }
}

/// A seed for a request that gave none: a fresh variation each time.
pub fn fresh_seed() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    Rng::new(nanos ^ u64::from(std::process::id())).next() & MAX_SEED
}

/// Steering draws from a stream of its own, so a design's own choices are the same steered or not.
const STEER_SALT: u64 = 0x7374_6565_72;

/// Renders a checked request from its decoded sources (in the request's order), with `seed` deciding every choice.
/// `progress` hears how far along it is, 0 to 1.
pub fn render(request: &Request, seed: u64, sources: &[Source], progress: &mut dyn FnMut(f64)) -> Result<Render, DesignError> {
    request.check()?;
    if sources.len() != request.sources.len() {
        return refuse("Every sound the request names must be read before it's designed.");
    }
    progress(0.0);
    let most = (source_seconds(sources.len()) * RATE) as usize;
    let prepared = sources.iter().map(|source| prepare(source, most)).collect::<Result<Vec<_>, _>>()?;
    progress(0.1);
    let rng = Rng::new(seed);
    let options = &request.options;
    let steer = options.steer.as_ref().filter(|steer| steer.moves());
    let steering = Rng::new(seed).fork(STEER_SALT);
    let (prepared, mut steered) = match steer {
        Some(steer) => steer.sources(prepared, &steering),
        None => (prepared, Vec::new()),
    };
    let (audio, recipe, ending) = match (request.op, request.mode) {
        (Op::Stack, _) => stack::stack(&prepared, options, rng),
        (Op::Grow, _) => grow::grow(&prepared, options, rng, progress),
        (_, Some(Mode::Blend)) => cross::blend(&prepared, options, rng, progress),
        (_, Some(Mode::Attack)) => cross::attack(&prepared, options, rng, progress),
        (_, Some(Mode::Resonate)) => cross::resonate(&prepared, options, rng),
        (_, Some(Mode::Modulate)) => cross::modulate(&prepared, options, rng),
        (_, Some(Mode::Texture)) => fuse::texture(&prepared, options, rng, progress),
        (_, Some(Mode::Loop)) => fuse::looped(&prepared, options, rng),
        (_, Some(Mode::Smear)) => fuse::smear(&prepared, options, rng, progress),
        (_, None) => unreachable!("check refuses a cross or fuse without a mode"),
    };
    progress(0.9);
    let (audio, more) = match steer {
        Some(steer) => steer.result(audio, &ending, &steering),
        None => (audio, Vec::new()),
    };
    steered.extend(more);
    let recipe = if steered.is_empty() { recipe } else { format!("{recipe} · {}", steered.join(", ")) };
    progress(0.95);
    let audio = finish(audio, ending, options.seconds)?;
    progress(1.0);
    Ok(Render { audio, recipe })
}

/// The last steps every design takes: no DC, no clicks, no silent tail, no sample past -1 dBTP, nothing but numbers.
fn finish(mut audio: Stereo, ending: Ending, seconds: Option<f64>) -> Result<Stereo, DesignError> {
    for channel in audio.iter_mut() {
        for v in channel.iter_mut() {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
    }
    let looped = matches!(ending, Ending::Loop);
    let len = audio[0].len();
    if len == 0 {
        return refuse("The design came out empty.");
    }
    // DC out; a loop's filter is run over its end first, so its start continues from where it ends.
    for channel in audio.iter_mut() {
        let mut filter = high_pass(RATE, 15.0);
        if looped {
            let lead = len.min(RATE as usize);
            for v in &channel[len - lead..] {
                filter.process(*v as f64);
            }
        }
        for v in channel.iter_mut() {
            *v = filter.process(*v as f64) as f32;
        }
    }
    if let Ending::OneShot { fade } = ending {
        let mono = analysis::mono(&audio);
        let wanted = seconds.map(|s| (s * RATE) as usize);
        let end = match wanted {
            Some(wanted) => wanted.min(len),
            None => (analysis::end_of(&mono) + fade).clamp((0.05 * RATE) as usize, len),
        };
        for channel in audio.iter_mut() {
            channel.truncate(end);
        }
        fx::fade_in(&mut audio, 32);
        fx::fade_out(&mut audio, fade.min(end / 3));
    }
    let peak = audio.iter().map(|channel| true_peak(channel, looped)).fold(0.0f64, f64::max);
    if !(peak > 1e-6) {
        return refuse("The design came out silent: try another variation or other sounds.");
    }
    let target = 10f64.powf(-1.0 / 20.0);
    fx::scale(&mut audio, target / peak);
    Ok(audio)
}

/// The oversampling the true peak is read at, and the half-length (in input samples) of its interpolating kernel.
const OVERSAMPLE: usize = 8;
const KERNEL: i64 = 24;

/// The highest level `channel` reaches between its samples as well as on them: 8× oversampled through a
/// Blackman-windowed sinc, which reads a bright transient's overs that BS.1770's 4×, 12-tap filter misses. A loop's
/// end is read on into its start.
pub fn true_peak(channel: &[f32], looped: bool) -> f64 {
    let len = channel.len() as i64;
    if len == 0 {
        return 0.0;
    }
    let kernels: Vec<Vec<f64>> = (1..OVERSAMPLE)
        .map(|phase| {
            let offset = phase as f64 / OVERSAMPLE as f64;
            (-KERNEL + 1..=KERNEL)
                .map(|k| {
                    let x = offset - k as f64;
                    let sinc = (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x);
                    let w = std::f64::consts::PI * (x + KERNEL as f64) / (2 * KERNEL) as f64;
                    sinc * (0.42 - 0.5 * (2.0 * w).cos() + 0.08 * (4.0 * w).cos())
                })
                .collect()
        })
        .collect();
    let at = |i: i64| -> f64 {
        if looped {
            channel[i.rem_euclid(len) as usize] as f64
        } else if (0..len).contains(&i) {
            channel[i as usize] as f64
        } else {
            0.0
        }
    };
    let mut peak = channel.iter().fold(0.0f64, |m, v| m.max(v.abs() as f64));
    for n in -1..len {
        let interior = n - KERNEL + 1 >= 0 && n + KERNEL < len;
        for kernel in &kernels {
            let mut sum = 0.0;
            if interior {
                let from = (n - KERNEL + 1) as usize;
                for (tap, w) in channel[from..from + kernel.len()].iter().zip(kernel) {
                    sum += *tap as f64 * w;
                }
            } else {
                for (j, w) in kernel.iter().enumerate() {
                    sum += at(n - KERNEL + 1 + j as i64) * w;
                }
            }
            peak = peak.max(sum.abs());
        }
    }
    peak
}

/// A short name for a source in a recipe: its name without folders or extension.
pub(crate) fn short(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = match base.rfind('.') {
        Some(dot) if dot > 0 && base.len() - dot <= 5 => &base[..dot],
        _ => base,
    };
    base.to_string()
}

/// Weights as a recipe says them: "45% Kick, 35% Tom, 20% Clap" (largest first).
pub(crate) fn shares(names: &[&str], weights: &[f64]) -> String {
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by(|a, b| weights[*b].total_cmp(&weights[*a]));
    order.iter().map(|&i| format!("{}% {}", (weights[i] * 100.0).round(), short(names[i]))).collect::<Vec<_>>().join(", ")
}
