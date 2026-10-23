//! Kumi's player: a song or a sound of the producer's, heard through the computer's output. One sound at a time, from a
//! position for a length; a new one takes over from the one playing, and a stop ends it, each with a fade a few
//! milliseconds long so nothing clicks. The file is read as Kumi reads any (WAV and AIFF in place, other formats
//! converted), resampled to the output's rate and handed to it whole.
//!
//! The output is the computer's default one, through cpal: Core Audio on a Mac, WASAPI on Windows. On Linux cpal needs
//! ALSA's library to build and to run, which Kumi's Linux bundle doesn't carry, so there the player is off unless Kumi
//! was built with kumi-media's `linux-player`. Nothing opens the output until something plays.

use super::{decode::open_audio_to, dsp::resample};
use kumi_common::abort::Signal;
use std::{
    cell::{Cell, RefCell},
    path::Path,
};

/// How long a fade in or out takes, seconds: short enough not to be heard as one, long enough not to click.
const FADE: f64 = 0.005;

/// How an output plays: its rate (Hz) and its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
}

/// Why the player didn't play.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlayerError {
    #[error("There's no sound output on this computer to play through.")]
    NoOutput,
    /// This Kumi was built without its player (Linux's, without ALSA).
    #[error("This Kumi can't play sounds itself: on Linux its player needs ALSA, which it was built without.")]
    Off,
    #[error("The sound output wouldn't play: {0}")]
    Output(String),
    /// The file couldn't be read, or there's nothing to play where it was asked.
    #[error("{0}")]
    File(String),
    #[error("Stopped before it played.")]
    Stopped,
}

/// Where the player's sound goes: the computer's output, or a test's.
pub trait Output {
    fn format(&self) -> Format;
    /// Plays these samples (interleaved, at its rate and in its channels), in place of whatever plays.
    fn start(&self, samples: Vec<f32>) -> Result<(), PlayerError>;
    /// Ends the sound playing.
    fn stop(&self);
    /// Whether a sound is still playing.
    fn playing(&self) -> bool;
    /// Whether it's still where sound should go (the computer's default output, and working): when it isn't, the next
    /// sound opens the output again.
    fn current(&self) -> bool {
        true
    }
}

/// How the player opens its output: the computer's default one (`open_default`), or a test's.
pub type Opener = Box<dyn Fn() -> Result<Box<dyn Output>, PlayerError>>;

pub struct Player {
    opener: Opener,
    output: RefCell<Option<Box<dyn Output>>>,
    /// Counts plays and stops: a play whose file is still being read when another play or a stop comes plays nothing.
    turn: Cell<u64>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    /// A player through the computer's default output, opened the first time something plays.
    pub fn new() -> Self {
        Self::with_opener(Box::new(open_default))
    }

    pub fn with_opener(opener: Opener) -> Self {
        Self { opener, output: RefCell::new(None), turn: Cell::new(0) }
    }

    /// Plays `seconds` of a file from `from` (seconds into it), in place of whatever plays; done once it has begun. When
    /// another play or a stop comes while the file is still being read, this one plays nothing (and isn't an error).
    pub async fn play(&self, path: impl AsRef<Path>, from: f64, seconds: f64, signal: Signal) -> Result<(), PlayerError> {
        let turn = self.turn.get() + 1;
        self.turn.set(turn);
        // The output first: with none, there's nothing to read the file for.
        let format = self.opened()?;
        let clip = read(path.as_ref(), from, seconds, &signal).await?;
        let samples =
            tokio::task::spawn_blocking(move || shaped(clip, format)).await.map_err(|error| PlayerError::Output(error.to_string()))?;
        if self.turn.get() != turn {
            return Ok(());
        }
        if signal.is_cancelled() {
            return Err(PlayerError::Stopped);
        }
        // The output may have changed while the file was read (another default chosen): the sound is for this one.
        let started = match self.output.borrow().as_ref() {
            Some(output) if output.format() == format => output.start(samples),
            _ => Err(PlayerError::Output("it changed while Kumi read the file".into())),
        };
        if started.is_err() {
            // Opened afresh for the next sound.
            self.output.borrow_mut().take();
        }
        started
    }

    /// Ends the sound playing, and a play still reading its file.
    pub fn stop(&self) {
        self.turn.set(self.turn.get() + 1);
        if let Some(output) = self.output.borrow().as_ref() {
            output.stop();
        }
    }

    /// Whether a sound is playing.
    pub fn playing(&self) -> bool {
        self.output.borrow().as_ref().is_some_and(|output| output.playing())
    }

    /// The output, opened when there's none yet or it isn't where sound should go any more: how it plays.
    fn opened(&self) -> Result<Format, PlayerError> {
        if let Some(output) = self.output.borrow().as_ref().filter(|output| output.current()) {
            return Ok(output.format());
        }
        if let Some(gone) = self.output.borrow_mut().take() {
            gone.stop();
        }
        let output = (self.opener)()?;
        let format = output.format();
        if format.rate == 0 || format.channels == 0 {
            return Err(PlayerError::Output(format!("it plays {} channels at {} Hz", format.channels, format.rate)));
        }
        *self.output.borrow_mut() = Some(output);
        Ok(format)
    }
}

/// A stretch of a file as its channels (one, or the first two) at its own rate.
async fn read(path: &Path, from: f64, seconds: f64, signal: &Signal) -> Result<(Vec<Vec<f32>>, f64), PlayerError> {
    let (from, seconds) = (from.max(0.), seconds.max(0.));
    let stopped = |error: super::AudioError| if signal.is_cancelled() { PlayerError::Stopped } else { PlayerError::File(error.0) };
    let mut source = open_audio_to(path, Some(signal.clone()), Some(from + seconds)).await.map_err(stopped)?;
    let rate = source.sample_rate;
    source.seek(from * rate);
    let wanted = (seconds * rate).round() as usize;
    let mut channels: Vec<Vec<f32>> = vec![Vec::with_capacity(wanted); source.channels.clamp(1, 2)];
    let read = async {
        while channels[0].len() < wanted {
            if signal.is_cancelled() {
                return Err(PlayerError::Stopped);
            }
            let Some(block) = source.read((wanted - channels[0].len()).min(65_536)).await.map_err(stopped)? else { break };
            for (channel, samples) in channels.iter_mut().enumerate() {
                samples.extend_from_slice(&block[channel.min(block.len() - 1)]);
            }
        }
        Ok(())
    }
    .await;
    let _ = source.close().await;
    read?;
    if channels[0].is_empty() {
        return Err(PlayerError::File("There's nothing to play there: it's at or past the end of the file.".into()));
    }
    Ok((channels, rate))
}

/// A clip as an output plays it: at its rate, in its channels (one channel heard in each of them; two in the first two,
/// or mixed into one), faded in and out.
fn shaped((channels, rate): (Vec<Vec<f32>>, f64), format: Format) -> Vec<f32> {
    let resampled: Vec<Vec<f32>> = channels.iter().map(|channel| resample(channel, rate, format.rate as f64)).collect();
    let frames = resampled.iter().map(Vec::len).min().unwrap_or(0);
    let wide = format.channels.max(1) as usize;
    let fade = ((format.rate as f64 * FADE).round() as usize).max(1);
    let mut samples = Vec::with_capacity(frames * wide);
    for frame in 0..frames {
        let gain = ((frame + 1).min(frames - frame) as f32 / fade as f32).min(1.);
        for channel in 0..wide {
            let value = match resampled.as_slice() {
                [mono] => mono[frame],
                [left, right] if wide == 1 => (left[frame] + right[frame]) / 2.,
                [left, right] => match channel {
                    0 => left[frame],
                    1 => right[frame],
                    _ => 0.,
                },
                _ => 0.,
            };
            samples.push((value * gain).clamp(-1., 1.));
        }
    }
    samples
}

/// What an output plays, shared with its audio thread: the sound, how far it has got, and a fade out under way (a stop,
/// or a new sound taking over once it's done). (Without the player, as on Linux, only its tests use it.)
#[derive(Debug, Default)]
#[cfg_attr(all(target_os = "linux", not(feature = "linux-player")), allow(dead_code))]
pub(crate) struct Playback {
    samples: Vec<f32>,
    at: usize,
    /// Frames of the fade left, and how many it has in all.
    fading: Option<(usize, usize)>,
    next: Option<Vec<f32>>,
}

#[cfg_attr(all(target_os = "linux", not(feature = "linux-player")), allow(dead_code))]
impl Playback {
    /// A new sound: at once when nothing plays, else once the one playing has faded out over `fade` frames.
    pub(crate) fn start(&mut self, samples: Vec<f32>, fade: usize) {
        if self.playing() {
            self.fading.get_or_insert((fade.max(1), fade.max(1)));
            self.next = Some(samples);
        } else {
            (self.samples, self.at, self.fading, self.next) = (samples, 0, None, None);
        }
    }

    /// The sound playing fades out over `fade` frames, and nothing follows it.
    pub(crate) fn stop(&mut self, fade: usize) {
        self.next = None;
        if self.at < self.samples.len() {
            self.fading.get_or_insert((fade.max(1), fade.max(1)));
        }
    }

    pub(crate) fn playing(&self) -> bool {
        self.at < self.samples.len() || self.next.is_some()
    }

    /// The output's next block, `channels` interleaved: the sound, faded where it's fading, then silence.
    pub(crate) fn fill(&mut self, out: &mut [f32], channels: usize) {
        for frame in out.chunks_mut(channels.max(1)) {
            if self.fading.is_some_and(|(left, _)| left == 0) {
                (self.samples, self.at, self.fading) = (self.next.take().unwrap_or_default(), 0, None);
            }
            let gain = self.fading.map_or(1., |(left, of)| left as f32 / of as f32);
            for sample in frame.iter_mut() {
                *sample = match self.samples.get(self.at) {
                    Some(value) => {
                        self.at += 1;
                        value * gain
                    }
                    None => 0.,
                };
            }
            if let Some((left, _)) = self.fading.as_mut() {
                *left -= 1;
            }
        }
    }
}

/// The computer's default output: Core Audio on a Mac, WASAPI on Windows, ALSA on Linux with `linux-player`.
#[cfg(any(not(target_os = "linux"), feature = "linux-player"))]
pub fn open_default() -> Result<Box<dyn Output>, PlayerError> {
    device::open().map(|output| Box::new(output) as Box<dyn Output>)
}

/// Off: this Kumi was built without its player (Linux, without `linux-player`).
#[cfg(all(target_os = "linux", not(feature = "linux-player")))]
pub fn open_default() -> Result<Box<dyn Output>, PlayerError> {
    Err(PlayerError::Off)
}

#[cfg(any(not(target_os = "linux"), feature = "linux-player"))]
mod device {
    use super::{Format, Output, Playback, PlayerError, FADE};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    };

    /// The computer's default output, through cpal: a stream that plays the sound it's given, and silence after it.
    pub(super) struct DeviceOutput {
        stream: cpal::Stream,
        format: Format,
        /// Which device it is, to notice another default chosen since.
        id: Option<cpal::DeviceId>,
        playback: Arc<Mutex<Playback>>,
        /// The device went (unplugged, its driver restarted): the next sound opens the output again.
        broken: Arc<AtomicBool>,
    }

    pub(super) fn open() -> Result<DeviceOutput, PlayerError> {
        let device = cpal::default_host().default_output_device().ok_or(PlayerError::NoOutput)?;
        let wouldnt = |error: cpal::Error| PlayerError::Output(error.to_string());
        let supported = device.default_output_config().map_err(wouldnt)?;
        let format = Format { rate: supported.sample_rate(), channels: supported.channels() };
        let config = supported.config();
        let (playback, broken) = (Arc::new(Mutex::new(Playback::default())), Arc::new(AtomicBool::new(false)));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream::<f32>(&device, config, &playback, &broken),
            cpal::SampleFormat::F64 => stream::<f64>(&device, config, &playback, &broken),
            cpal::SampleFormat::I16 => stream::<i16>(&device, config, &playback, &broken),
            cpal::SampleFormat::I32 => stream::<i32>(&device, config, &playback, &broken),
            cpal::SampleFormat::U16 => stream::<u16>(&device, config, &playback, &broken),
            cpal::SampleFormat::I8 => stream::<i8>(&device, config, &playback, &broken),
            cpal::SampleFormat::U8 => stream::<u8>(&device, config, &playback, &broken),
            other => return Err(PlayerError::Output(format!("Kumi can't play to an output that takes {other} samples"))),
        }
        .map_err(wouldnt)?;
        Ok(DeviceOutput { stream, format, id: device.id().ok(), playback, broken })
    }

    fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        playback: &Arc<Mutex<Playback>>,
        broken: &Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::Error> {
        let channels = config.channels as usize;
        let (playback, broken) = (playback.clone(), broken.clone());
        let mut block: Vec<f32> = Vec::new();
        device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                block.resize(data.len(), 0.);
                // Never waiting on Kumi's thread: a moment's silence rather than a late block.
                match playback.try_lock() {
                    Ok(mut playback) => playback.fill(&mut block, channels),
                    Err(_) => block.fill(0.),
                }
                for (out, value) in data.iter_mut().zip(&block) {
                    *out = <T as cpal::Sample>::from_sample(*value);
                }
            },
            move |_| broken.store(true, Ordering::Relaxed),
            None,
        )
    }

    impl DeviceOutput {
        fn playback(&self) -> MutexGuard<'_, Playback> {
            self.playback.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
        fn fade(&self) -> usize {
            (self.format.rate as f64 * FADE).round() as usize
        }
    }

    impl Output for DeviceOutput {
        fn format(&self) -> Format {
            self.format
        }
        fn start(&self, samples: Vec<f32>) -> Result<(), PlayerError> {
            if self.broken.load(Ordering::Relaxed) {
                return Err(PlayerError::Output("the output went away".into()));
            }
            self.playback().start(samples, self.fade());
            self.stream.play().map_err(|error| PlayerError::Output(error.to_string()))
        }
        fn stop(&self) {
            self.playback().stop(self.fade());
        }
        fn playing(&self) -> bool {
            self.playback().playing()
        }
        fn current(&self) -> bool {
            !self.broken.load(Ordering::Relaxed)
                && cpal::default_host().default_output_device().and_then(|device| device.id().ok()) == self.id
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(playback: &mut Playback, frames: usize) -> Vec<f32> {
        let mut out = vec![9.; frames * 2];
        playback.fill(&mut out, 2);
        out
    }

    #[test]
    fn a_sound_plays_to_its_end_then_silence() {
        let mut playback = Playback::default();
        assert!(!playback.playing());
        playback.start(vec![0.5, -0.5, 0.25, -0.25], 4);
        assert!(playback.playing());
        assert_eq!(played(&mut playback, 3), [0.5, -0.5, 0.25, -0.25, 0., 0.]);
        assert!(!playback.playing());
    }

    #[test]
    fn a_new_sound_takes_over_once_the_one_playing_has_faded_out() {
        let mut playback = Playback::default();
        playback.start(vec![1.; 20], 2);
        assert_eq!(played(&mut playback, 1), [1., 1.]);
        playback.start(vec![0.5; 4], 2);
        // Two frames of fade (all, then half), then the new sound from its start.
        assert_eq!(played(&mut playback, 4), [1., 1., 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
        assert_eq!(played(&mut playback, 1), [0., 0.]);
        assert!(!playback.playing());
    }

    #[test]
    fn a_stop_fades_out_and_nothing_follows() {
        let mut playback = Playback::default();
        playback.start(vec![1.; 20], 4);
        playback.start(vec![0.5; 4], 4);
        playback.stop(2);
        assert!(playback.playing(), "it's still fading");
        // The fade under way goes on as it was (four frames), and nothing comes after it.
        assert_eq!(played(&mut playback, 5), [1., 1., 0.75, 0.75, 0.5, 0.5, 0.25, 0.25, 0., 0.]);
        assert!(!playback.playing());
        playback.stop(2);
        assert_eq!(played(&mut playback, 1), [0., 0.], "a stop with nothing playing is nothing");
    }

    #[test]
    fn a_clip_is_shaped_to_the_outputs_channels_and_faded_at_its_ends() {
        let mono = (vec![vec![1.; 1_000]], 48_000.);
        let stereo = shaped(mono.clone(), Format { rate: 48_000, channels: 2 });
        assert_eq!(stereo.len(), 2_000, "one channel heard in both");
        assert!(stereo[0] < 0.01 && stereo[1_999] < 0.01, "faded in and out");
        assert_eq!((stereo[1_000], stereo[1_001]), (1., 1.));
        let two = (vec![vec![0.5; 1_000], vec![-0.5; 1_000]], 48_000.);
        let one = shaped(two.clone(), Format { rate: 48_000, channels: 1 });
        assert!(one.iter().all(|value| value.abs() < 1e-6), "two mixed into one");
        let four = shaped(two, Format { rate: 48_000, channels: 4 });
        assert_eq!(&four[2_000..2_004], [0.5, -0.5, 0., 0.], "left and right in the first two");
    }
}
