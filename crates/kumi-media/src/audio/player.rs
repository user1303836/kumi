//! Kumi's player: a song or a sound of the producer's, heard through the computer's output. One sound at a time, from a
//! position for a length; a new one takes over from the one playing, and a stop ends it, each with a fade a few
//! milliseconds long so nothing clicks. The file is read as Kumi reads any (WAV and AIFF in place, other formats
//! converted, only the stretch played), resampled to the output's rate and handed to it whole.
//!
//! The output is the computer's default one, through cpal: Core Audio on a Mac, WASAPI on Windows. It opens, plays and
//! closes on a thread of its own, never on Kumi's, and rests (its stream paused, holding neither the device nor the
//! computer awake) once nothing has played for a few seconds. On Linux cpal needs ALSA's library to build and to run,
//! which Kumi's Linux bundle doesn't carry, so there the player is off unless Kumi was built with kumi-media's
//! `linux-player`. Nothing opens the output until something plays.

use super::{
    decode::{open_audio_to, prepare_audio_range},
    dsp::resample,
};
use kumi_common::{abort::Signal, js::number::to_string};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    time::{Duration, Instant},
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

/// What became of a play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Played {
    /// It began: it plays for its length, unless another play or a stop comes.
    Began,
    /// Another play, or a stop, came while its file was read: none of it played.
    Overtaken,
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

/// How a test's player opens its output.
pub type Opener = Box<dyn Fn() -> Result<Box<dyn Output>, PlayerError>>;

/// Where the player's output comes from.
enum Opens {
    /// The computer's default one, opened on a thread of its own.
    Default,
    /// A test's.
    With(Opener),
}

pub struct Player {
    opens: Opens,
    output: RefCell<Option<Box<dyn Output>>>,
    /// One output opens at a time: a play asked while one opens waits for it, and plays through it.
    opening: tokio::sync::Mutex<()>,
    /// Counts plays and stops: a play whose file is still being read when another play or a stop comes plays nothing.
    turn: Cell<u64>,
    /// The latest play's: the next play or a stop cancels it, so a file still being read stops being read.
    reading: RefCell<Signal>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    /// A player through the computer's default output, opened the first time something plays.
    pub fn new() -> Self {
        Self::opening(Opens::Default)
    }

    pub fn with_opener(opener: Opener) -> Self {
        Self::opening(Opens::With(opener))
    }

    fn opening(opens: Opens) -> Self {
        Self {
            opens,
            output: RefCell::new(None),
            opening: tokio::sync::Mutex::new(()),
            turn: Cell::new(0),
            reading: RefCell::new(Signal::new()),
        }
    }

    /// Plays `seconds` of a file from `from` (seconds into it), in place of whatever plays: Began once it has. When
    /// another play or a stop comes while the file is still being read, this one stops reading it and plays nothing:
    /// Overtaken, which isn't an error.
    pub async fn play(&self, path: impl AsRef<Path>, from: f64, seconds: f64) -> Result<Played, PlayerError> {
        let turn = self.turn.get() + 1;
        self.turn.set(turn);
        let signal = Signal::new();
        self.reading.replace(signal.clone()).cancel();
        let overtaken = || self.turn.get() != turn;
        // The output first: with none, there's nothing to read the file for.
        let format = self.opened().await?;
        if overtaken() {
            return Ok(Played::Overtaken);
        }
        let clip = match stretch(path.as_ref(), from, seconds, &signal).await {
            Err(_) if overtaken() => return Ok(Played::Overtaken),
            clip => clip?,
        };
        let samples =
            tokio::task::spawn_blocking(move || shaped(clip, format)).await.map_err(|error| PlayerError::Output(error.to_string()))?;
        if overtaken() {
            return Ok(Played::Overtaken);
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
        started.map(|()| Played::Began)
    }

    /// Ends the sound playing, and a play still reading its file.
    pub fn stop(&self) {
        self.turn.set(self.turn.get() + 1);
        self.reading.borrow().cancel();
        if let Some(output) = self.output.borrow().as_ref() {
            output.stop();
        }
    }

    /// Whether a sound is playing.
    pub fn playing(&self) -> bool {
        self.output.borrow().as_ref().is_some_and(|output| output.playing())
    }

    /// The output, opened when there's none yet or it isn't where sound should go any more: how it plays.
    async fn opened(&self) -> Result<Format, PlayerError> {
        let _one = self.opening.lock().await;
        if let Some(output) = self.output.borrow().as_ref().filter(|output| output.current()) {
            return Ok(output.format());
        }
        // What the one before plays fades out, on its own thread, before it closes.
        if let Some(gone) = self.output.borrow_mut().take() {
            gone.stop();
        }
        let output = match &self.opens {
            Opens::Default => open_default().await?,
            Opens::With(opener) => opener()?,
        };
        let format = output.format();
        if format.rate == 0 || format.channels == 0 {
            return Err(PlayerError::Output(format!("it plays {} channels at {} Hz", format.channels, format.rate)));
        }
        *self.output.borrow_mut() = Some(output);
        Ok(format)
    }
}

/// A stretch of a file as its channels (one, or the first two) at its own rate: what the player plays, and what the map's
/// page plays. Only the stretch is converted (a song's middle without what comes before it), where the file needs
/// converting.
pub async fn stretch(path: &Path, from: f64, seconds: f64, signal: &Signal) -> Result<(Vec<Vec<f32>>, f64), PlayerError> {
    let (from, seconds) = (from.max(0.), seconds.max(0.));
    let stopped = |error: super::AudioError| if signal.is_cancelled() { PlayerError::Stopped } else { PlayerError::File(error.0) };
    let prepared = prepare_audio_range(path, Some(signal.clone()), from, seconds).await.map_err(stopped)?;
    let mut source = open_audio_to(&prepared.path, Some(signal.clone()), None).await.map_err(stopped)?;
    let rate = source.sample_rate;
    // A rate no file has (a broken header's): nothing is sized from it.
    if !(8000.0..=384000.0).contains(&rate) {
        let _ = source.close().await;
        prepared.cleanup().await;
        return Err(PlayerError::File(format!("Kumi can't play it: it says its sample rate is {} Hz.", to_string(rate))));
    }
    // Seconds of the file, as seconds of what was converted.
    source.seek((from - prepared.start).max(0.) * rate);
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
    prepared.cleanup().await;
    read?;
    if channels[0].is_empty() {
        return Err(PlayerError::File("There's nothing to play there: it's at or past the end of the file.".into()));
    }
    Ok((channels, rate))
}

/// A clip as an output plays it: at its rate, in its first two channels (one channel heard in both; two each in its
/// own, or mixed into one), faded in and out. An interface's other outputs stay silent.
fn shaped((channels, rate): (Vec<Vec<f32>>, f64), format: Format) -> Vec<f32> {
    let resampled: Vec<Vec<f32>> = channels.iter().map(|channel| resample(channel, rate, format.rate as f64)).collect();
    let frames = resampled.iter().map(Vec::len).min().unwrap_or(0);
    let wide = format.channels.max(1) as usize;
    let fade = ((format.rate as f64 * FADE).round() as usize).max(1);
    let mut samples = Vec::with_capacity(frames * wide);
    for frame in 0..frames {
        let gain = ((frame + 1).min(frames - frame) as f32 / fade as f32).min(1.);
        for channel in 0..wide {
            let value = match (resampled.as_slice(), channel) {
                ([mono], 0 | 1) => mono[frame],
                ([left, right], _) if wide == 1 => (left[frame] + right[frame]) / 2.,
                ([left, _], 0) => left[frame],
                ([_, right], 1) => right[frame],
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

/// What Kumi's thread asks an output's own thread. (Without the player, as on Linux, only its tests use it.)
#[cfg_attr(all(target_os = "linux", not(feature = "linux-player")), allow(dead_code))]
pub(crate) enum Asked {
    /// A sound was handed over: the stream plays (again, if it rested).
    Play,
    /// The output is done with: what plays fades out, and it closes.
    Close,
}

/// What an output's thread runs: cpal's stream, or a test's.
#[cfg_attr(all(target_os = "linux", not(feature = "linux-player")), allow(dead_code))]
pub(crate) trait Runs {
    fn play(&self) -> Result<(), String>;
    fn pause(&self);
}

/// An output's own thread, until it's closed (or Kumi's side goes): the stream plays when a sound is handed over and
/// rests (paused) once nothing has played for `idle`; on closing, what plays fades out first (over `fade` frames,
/// waited for a fifth of a second at most), so a sound that's replaced doesn't click.
#[cfg_attr(all(target_os = "linux", not(feature = "linux-player")), allow(dead_code))]
pub(crate) fn serve(
    stream: &dyn Runs,
    playback: &Mutex<Playback>,
    broken: &AtomicBool,
    asked: &mpsc::Receiver<Asked>,
    fade: usize,
    idle: Duration,
) {
    let lock = || playback.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let tick = (idle / 4).clamp(Duration::from_millis(5), Duration::from_millis(100));
    let (mut running, mut quiet) = (false, None::<Instant>);
    loop {
        match asked.recv_timeout(tick) {
            Ok(Asked::Play) => {
                if !running && stream.play().is_err() {
                    broken.store(true, Ordering::Relaxed);
                }
                (running, quiet) = (true, None);
            }
            Ok(Asked::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !running || lock().playing() {
            quiet = None;
        } else if quiet.get_or_insert_with(Instant::now).elapsed() >= idle {
            stream.pause();
            (running, quiet) = (false, None);
        }
    }
    if running {
        lock().stop(fade);
        let until = Instant::now() + Duration::from_millis(200);
        while lock().playing() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The computer's default output, opened on its own thread: Core Audio on a Mac, WASAPI on Windows, ALSA on Linux with
/// `linux-player`.
#[cfg(any(not(target_os = "linux"), feature = "linux-player"))]
async fn open_default() -> Result<Box<dyn Output>, PlayerError> {
    device::open().await.map(|output| Box::new(output) as Box<dyn Output>)
}

/// Off: this Kumi was built without its player (Linux, without `linux-player`).
#[cfg(all(target_os = "linux", not(feature = "linux-player")))]
async fn open_default() -> Result<Box<dyn Output>, PlayerError> {
    Err(PlayerError::Off)
}

#[cfg(any(not(target_os = "linux"), feature = "linux-player"))]
mod device {
    use super::{serve, Asked, Format, Output, Playback, PlayerError, Runs, FADE};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex, MutexGuard,
        },
        time::Duration,
    };

    /// How long the stream runs on with nothing to play before it rests.
    const IDLE: Duration = Duration::from_secs(3);

    /// The computer's default output, through cpal, on a thread of its own: the device opens, plays, rests and closes
    /// there, never on Kumi's thread. Its stream plays the sound it's given, then silence, until it rests.
    pub(super) struct DeviceOutput {
        format: Format,
        /// Which device it is, to notice another default chosen since.
        id: Option<cpal::DeviceId>,
        playback: Arc<Mutex<Playback>>,
        /// The device went (unplugged, its driver restarted): the next sound opens the output again.
        broken: Arc<AtomicBool>,
        asks: mpsc::Sender<Asked>,
    }

    struct Cpal(cpal::Stream);

    impl Runs for Cpal {
        fn play(&self) -> Result<(), String> {
            self.0.play().map_err(|error| error.to_string())
        }
        fn pause(&self) {
            let _ = self.0.pause();
        }
    }

    /// Whether an error from the stream ends it: an underrun, a route the system followed or a refused real-time
    /// promotion leave it playing.
    pub(super) fn breaks(kind: cpal::ErrorKind) -> bool {
        !matches!(kind, cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied)
    }

    fn fade(format: Format) -> usize {
        (format.rate as f64 * FADE).round() as usize
    }

    pub(super) async fn open() -> Result<DeviceOutput, PlayerError> {
        let (playback, broken) = (Arc::new(Mutex::new(Playback::default())), Arc::new(AtomicBool::new(false)));
        let (asks, asked) = mpsc::channel();
        let (opened, open) = tokio::sync::oneshot::channel();
        let (played, gone) = (playback.clone(), broken.clone());
        std::thread::Builder::new()
            .name("kumi-player".into())
            .spawn(move || match built(&played, &gone) {
                Ok((stream, format, id)) => {
                    let _ = opened.send(Ok((format, id)));
                    serve(&Cpal(stream), &played, &gone, &asked, fade(format), IDLE);
                }
                Err(error) => {
                    let _ = opened.send(Err(error));
                }
            })
            .map_err(|error| PlayerError::Output(error.to_string()))?;
        let (format, id) = open.await.map_err(|_| PlayerError::Output("its thread ended".into()))??;
        Ok(DeviceOutput { format, id, playback, broken, asks })
    }

    /// The default device's stream (not playing yet), on the thread that runs it.
    fn built(
        playback: &Arc<Mutex<Playback>>,
        broken: &Arc<AtomicBool>,
    ) -> Result<(cpal::Stream, Format, Option<cpal::DeviceId>), PlayerError> {
        let device = cpal::default_host().default_output_device().ok_or(PlayerError::NoOutput)?;
        let wouldnt = |error: cpal::Error| PlayerError::Output(error.to_string());
        let supported = device.default_output_config().map_err(wouldnt)?;
        let format = Format { rate: supported.sample_rate(), channels: supported.channels() };
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream::<f32>(&device, config, playback, broken),
            cpal::SampleFormat::F64 => stream::<f64>(&device, config, playback, broken),
            cpal::SampleFormat::I16 => stream::<i16>(&device, config, playback, broken),
            cpal::SampleFormat::I32 => stream::<i32>(&device, config, playback, broken),
            cpal::SampleFormat::U16 => stream::<u16>(&device, config, playback, broken),
            cpal::SampleFormat::I8 => stream::<i8>(&device, config, playback, broken),
            cpal::SampleFormat::U8 => stream::<u8>(&device, config, playback, broken),
            other => return Err(PlayerError::Output(format!("Kumi can't play to an output that takes {other} samples"))),
        }
        .map_err(wouldnt)?;
        Ok((stream, format, device.id().ok()))
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
            move |error: cpal::Error| {
                if breaks(error.kind()) {
                    broken.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
    }

    impl DeviceOutput {
        fn playback(&self) -> MutexGuard<'_, Playback> {
            self.playback.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    impl Drop for DeviceOutput {
        fn drop(&mut self) {
            // Its thread lets what plays fade out, then closes it.
            let _ = self.asks.send(Asked::Close);
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
            self.playback().start(samples, fade(self.format));
            self.asks.send(Asked::Play).map_err(|_| PlayerError::Output("the output went away".into()))
        }
        fn stop(&self) {
            self.playback().stop(fade(self.format));
        }
        fn playing(&self) -> bool {
            !self.broken.load(Ordering::Relaxed) && self.playback().playing()
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
    use std::sync::Arc;

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
        let four = shaped(mono, Format { rate: 48_000, channels: 4 });
        assert_eq!(&four[2_000..2_004], [1., 1., 0., 0.], "one channel in the first two, as two are");
    }

    /// An output that plays nothing.
    struct Silent;

    impl Output for Silent {
        fn format(&self) -> Format {
            Format { rate: 48_000, channels: 2 }
        }
        fn start(&self, _: Vec<f32>) -> Result<(), PlayerError> {
            Ok(())
        }
        fn stop(&self) {}
        fn playing(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_play_reading_its_file_stops_reading_when_another_play_or_a_stop_comes() {
        // One thread for blocking work, kept busy while both plays are asked: their file reads wait behind it, so
        // each is still reading, however fast the computer is.
        let runtime = tokio::runtime::Builder::new_current_thread().max_blocking_threads(1).enable_all().build().unwrap();
        runtime.block_on(async {
            let player = Player::with_opener(Box::new(|| Ok(Box::new(Silent) as Box<dyn Output>)));
            let file = Path::new("there's no such file.wav");
            let (release, held) = mpsc::channel::<()>();
            tokio::task::spawn_blocking(move || {
                let _ = held.recv();
            });
            let mut first = std::pin::pin!(player.play(file, 0., 1.));
            assert!(futures::poll!(first.as_mut()).is_pending(), "reading its file");
            let reading = player.reading.borrow().clone();
            let mut second = std::pin::pin!(player.play(file, 0., 1.));
            assert!(futures::poll!(second.as_mut()).is_pending());
            assert!(reading.is_cancelled(), "the first stops reading");
            drop(release);
            assert_eq!(first.await, Ok(Played::Overtaken), "its file was missing, but it was overtaken first");
            let reading = player.reading.borrow().clone();
            player.stop();
            assert!(reading.is_cancelled(), "a stop ends the second's reading too");
            assert_eq!(second.await, Ok(Played::Overtaken));
            assert!(matches!(player.play(file, 0., 1.).await, Err(PlayerError::File(_))), "alone, it says the file's missing");
        });
    }

    /// A stream that says what it was asked.
    #[derive(Default)]
    struct Told(Mutex<Vec<&'static str>>);

    impl Runs for Told {
        fn play(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("play");
            Ok(())
        }
        fn pause(&self) {
            self.0.lock().unwrap().push("pause");
        }
    }

    fn waits_for(what: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while !what() {
            if Instant::now() > until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }

    #[test]
    fn an_output_plays_when_a_sound_comes_rests_once_nothing_plays_and_lets_it_fade_out_as_it_closes() {
        let (playback, broken, told) =
            (Arc::new(Mutex::new(Playback::default())), Arc::new(AtomicBool::new(false)), Arc::new(Told::default()));
        let (asks, asked) = mpsc::channel();
        let thread = {
            let (playback, broken, told) = (playback.clone(), broken.clone(), told.clone());
            std::thread::spawn(move || serve(&*told, &playback, &broken, &asked, 2, Duration::from_millis(30)))
        };
        let said = || told.0.lock().unwrap().clone();
        playback.lock().unwrap().start(vec![0.5; 8], 2);
        asks.send(Asked::Play).unwrap();
        assert!(waits_for(|| said() == ["play"]), "{:?}", said());
        // While the sound lasts, the stream runs on.
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(said(), ["play"]);
        // Played out (the audio callback took it all): it rests.
        played(&mut playback.lock().unwrap(), 4);
        assert!(waits_for(|| said() == ["play", "pause"]), "{:?}", said());
        // The next sound plays it again.
        playback.lock().unwrap().start(vec![0.5; 8], 2);
        asks.send(Asked::Play).unwrap();
        assert!(waits_for(|| said() == ["play", "pause", "play"]), "{:?}", said());
        // Closed while it plays (replaced by another output): what plays fades out first, then the thread ends.
        asks.send(Asked::Close).unwrap();
        assert!(waits_for(|| playback.lock().unwrap().fading.is_some()), "a fade, not a cut");
        played(&mut playback.lock().unwrap(), 4);
        assert!(waits_for(|| thread.is_finished()));
        thread.join().unwrap();
        assert!(!playback.lock().unwrap().playing());
        assert!(!broken.load(Ordering::Relaxed));
    }

    #[cfg(any(not(target_os = "linux"), feature = "linux-player"))]
    #[test]
    fn only_an_error_that_ends_the_stream_breaks_the_output() {
        use cpal::ErrorKind::*;
        for kind in [Xrun, DeviceChanged, RealtimeDenied] {
            assert!(!device::breaks(kind), "{kind:?} leaves it playing");
        }
        for kind in [DeviceNotAvailable, StreamInvalidated, HostUnavailable, BackendError, Other] {
            assert!(device::breaks(kind), "{kind:?}");
        }
    }
}
