//! Approved-reference measurement: one decode, the two existing meters, no models.
//! Original-source spans and replay-at-zero retained WAVs have separate identities.

use crate::{checklist::{Profile, REGIONS}, measure::{band_span, fine_hz, percentile, Heard, Meter, FINE_BINS}};
use kumi_common::abort::{Signal, SignalExt};
use kumi_media::audio::{
    analyze::{Analysis, StreamingAnalysis, USUAL},
    decode::{open_audio_range_with_options, AudioError, AudioRange, DecodeOptions},
};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::{Seek, SeekFrom, Write}};

/// An explicit source window; both fields are required and neither is inferred.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReferenceWindow {
    pub from: f64,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ReferenceOptions {
    pub window: Option<ReferenceWindow>,
    pub profile: bool,
    pub signal: Option<Signal>,
    pub decode: DecodeOptions,
}

/// The caller owns staging, verification, cleanup and atomic publication. On an
/// error this handle is closed, but any partial bytes remain in its unpublished stage.
pub struct RetainedWav {
    pub file: File,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleWindow {
    /// Exact for directly readable PCM; not invented from compressed-container tags.
    pub original_total_frames: Option<usize>,
    /// May be an estimate for a compressed container, as marked by the next field.
    pub original_seconds: Option<f64>,
    pub original_duration_exact: bool,
    pub requested: Option<ReferenceWindow>,
    /// Original-source sample index; start and count are floored independently,
    /// matching canonical analysis's sample selection. No padding is measured.
    pub start_frame: usize,
    pub frame_count: usize,
    pub sample_rate: f64,
    pub channels: usize,
    /// Retained WAV sample zero is the first measured sample, not `requested.from`.
    pub replay_from: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    NotRequested,
    ZeroEnergy,
    TooShortForProfileFrames,
    NoUsableProfileEnergy,
    TooShortForLoudness,
    BelowLoudnessGate,
    TooShortForRange,
    Mono,
    NoLowBandEnergy,
    NotMeasured,
    AboveNyquist,
    PartialFrequencyCoverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnavailableMetric {
    pub quantity: String,
    pub reason: UnavailableReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceAvailability {
    /// Computed from actual decoded sample squares, never from integrated LUFS.
    pub energy_zero: bool,
    pub profile_reason: Option<UnavailableReason>,
    /// Real frames inside Profile::of's 95th-percentile minus 15 dB gate,
    /// counted by that unchanged definition for its region-tolerance fallback.
    pub loud_frame_count: usize,
    /// Fewer than five frames use Profile::of's fixed ±1.5 dB region tolerance,
    /// not an observed variation or confidence interval.
    pub fixed_region_tolerance: bool,
    pub unavailable: Vec<UnavailableMetric>,
}

#[derive(Debug, Clone)]
pub struct ReferenceMeasurement {
    /// Canonical ten-band mix analysis, never replaced by a Heard quantity.
    /// `seconds` is the original known duration (or known decoded extent), while
    /// `analyzed` is the original measured span. Read `window` for exact provenance.
    pub analysis: Analysis,
    /// Formed from a real Heard while its frames exist; those frames are then dropped.
    /// Embeddings remain absent and this profile has no true-peak target.
    /// A field is usable only when `availability` carries it, including complete decoded frequency coverage.
    pub profile: Option<Profile>,
    pub window: SampleWindow,
    pub availability: ReferenceAvailability,
}

/// Measures an already-authorized immutable source with the caller's pinned local
/// decoder and cancellation signal. No timeout/deadline is created or extended.
/// Canonical analysis uses mix focus, avoiding a full-PCM sound/transcription buffer.
/// Without an explicit window the ordinary 720-second analysis bound is retained.
/// Ordinary listening APIs do not call this opt-in path and are unchanged.
pub async fn measure_reference(
    path: &str,
    name: &str,
    options: ReferenceOptions,
    retained: Option<RetainedWav>,
) -> Result<ReferenceMeasurement, AudioError> {
    validate_window(options.window)?;
    check(&options.signal)?;
    let path = path.to_owned();
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name).to_owned();
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| AudioError(e.to_string()))?;
        runtime.block_on(async move {
            let from = options.window.map_or(0., |w| w.from);
            let seconds = options.window.map_or(USUAL, |w| w.seconds);
            let mut decoded = open_audio_range_with_options(&path, options.signal.clone(), from, seconds, &options.decode).await?;
            let result = measure_decoded(&mut decoded, &name, &options, retained).await;
            decoded.source.close().await?;
            result
        })
    })
    .await
    .map_err(|error| AudioError(error.to_string()))?
}

fn validate_window(window: Option<ReferenceWindow>) -> Result<(), AudioError> {
    if let Some(w) = window {
        if !w.from.is_finite() || w.from < 0. || !w.seconds.is_finite() || w.seconds <= 0. || w.seconds > 60.
            || !(w.from + w.seconds).is_finite()
        {
            return Err(AudioError("Choose a finite start of at least zero and a duration greater than zero and at most 60 seconds.".into()));
        }
    }
    Ok(())
}

fn check(signal: &Option<Signal>) -> Result<(), AudioError> {
    if let Some(signal) = signal { signal.check()?; }
    Ok(())
}

fn past_end(name: &str, seconds: f64) -> AudioError {
    AudioError(format!("That window runs past the end of {name} ({seconds:.6} s)."))
}

async fn measure_decoded(
    decoded: &mut AudioRange,
    name: &str,
    options: &ReferenceOptions,
    retained: Option<RetainedWav>,
) -> Result<ReferenceMeasurement, AudioError> {
    check(&options.signal)?;
    let source = &mut decoded.source;
    let rate = source.sample_rate;
    let channels = source.channels;
    if !(8000. ..=384000.).contains(&rate) || rate.fract() != 0. {
        return Err(AudioError("Reference measurement needs a supported integral sample rate, without resampling.".into()));
    }
    if options.profile && !(1..=2).contains(&channels) {
        return Err(AudioError(format!("Reference profiles support mono or stereo, not {channels} channels; Kumi won't downmix them.")));
    }
    let from = options.window.map_or(0., |w| w.from);
    let start = from * rate;
    if start >= usize::MAX as f64 {
        return Err(AudioError("That reference start is outside the supported sample range.".into()));
    }
    let start_frame = start.floor() as usize;
    let copy_start_frame = (decoded.start * rate).round() as usize;
    let local_start = start_frame.saturating_sub(copy_start_frame);
    let available = source.frames.saturating_sub(local_start);
    let frame_count = match options.window {
        Some(w) => {
            // The extra decoded second proves the requested end against actual
            // PCM EOF, not an MP3 duration estimate. Only arithmetic roundoff is tolerated.
            let requested_end = (w.from + w.seconds - decoded.start) * rate;
            let roundoff = requested_end.abs().max(1.) * f64::EPSILON * 8.;
            if requested_end > source.frames as f64 + roundoff {
                if source.frames == 0 && decoded.original_frames.is_none() {
                    let duration = decoded.original_seconds.map_or_else(
                        || "original duration unknown".into(),
                        |seconds| format!("original duration approximately {seconds:.6} s"),
                    );
                    return Err(AudioError(format!("That window runs past the end of {name} ({duration}).")));
                }
                return Err(past_end(name, decoded.start + source.frames as f64 / rate));
            }
            (w.seconds * rate).floor() as usize
        }
        None => available.min((USUAL * rate).floor() as usize),
    };
    let selected_seconds = options.window.map_or(available as f64 / rate, |window| window.seconds).min(USUAL);
    if !(selected_seconds > 0.02) || (frame_count as f64) < rate * 0.02 {
        return Err(AudioError("There's no audio in that part of the file.".into()));
    }
    let mut canonical = StreamingAnalysis::new(rate, channels, frame_count)?;
    let mut detailed = options.profile.then(|| Meter::new(rate));
    let mut writer = retained.map(|r| WavWriter::new(r, rate as u32, channels, frame_count)).transpose()?;
    source.seek(local_start as f64);
    let mut done = 0;
    while done < frame_count {
        check(&options.signal)?;
        let Some(block) = source.read(65536.min(frame_count - done)).await? else {
            return Err(past_end(name, decoded.start + (local_start + done) as f64 / rate));
        };
        canonical.push(&block);
        if !canonical.energy().is_finite() {
            return Err(AudioError("That reference contains a non-finite audio sample.".into()));
        }
        if let Some(meter) = &mut detailed {
            let left = &block[0];
            meter.push(left, block.get(1).unwrap_or(left));
        }
        if let Some(writer) = &mut writer { writer.push(&block)?; }
        done += block[0].len();
        tokio::task::yield_now().await;
    }
    check(&options.signal)?;
    let actual_from = start_frame as f64 / rate;
    let extent = decoded.start + source.frames as f64 / rate;
    let energy_zero = canonical.energy() == 0.;
    let analysis = canonical.finish(name, &source.format, decoded.original_seconds.unwrap_or(extent), actual_from);
    let (profile, availability) = match detailed {
        Some(meter) => profile_of(name, meter.finish(), energy_zero, channels),
        None => (None, ReferenceAvailability {
            energy_zero, profile_reason: Some(UnavailableReason::NotRequested), loud_frame_count: 0,
            fixed_region_tolerance: false, unavailable: vec![],
        }),
    };
    check(&options.signal)?;
    if let Some(writer) = writer { writer.finish()?; }
    check(&options.signal)?;
    Ok(ReferenceMeasurement {
        analysis, profile,
        window: SampleWindow {
            original_total_frames: decoded.original_frames,
            original_seconds: decoded.original_seconds,
            original_duration_exact: decoded.original_frames.is_some(),
            requested: options.window, start_frame, frame_count: done, sample_rate: rate, channels, replay_from: 0.,
        },
        availability,
    })
}

fn profile_of(name: &str, heard: Heard, energy_zero: bool, channels: usize) -> (Option<Profile>, ReferenceAvailability) {
    let mut levels: Vec<f64> = heard.frames.level.iter().map(|level| *level as f64).collect();
    levels.sort_by(f64::total_cmp);
    let gate = if levels.is_empty() { 0. } else { percentile(&levels, 0.95) - 15. };
    let loud_frame_count = heard.frames.level.iter().filter(|level| **level as f64 >= gate).count();
    // Use the detailed meter's existing centroid energy threshold to avoid
    // treating epsilon/floor-only band shares as a usable tonal reference.
    let usable_frame_count = heard.frames.mid.iter().zip(&heard.frames.side).zip(&heard.frames.level)
        .filter(|((mid, side), level)| {
            **level as f64 >= gate && mid.iter().zip(side.iter()).map(|(m, s)| *m as f64 + *s as f64).sum::<f64>() > 1e-14
        })
        .count();
    let profile_reason = if energy_zero { Some(UnavailableReason::ZeroEnergy) }
        else if heard.frames.mid.is_empty() || heard.frames.low_fine.is_empty() { Some(UnavailableReason::TooShortForProfileFrames) }
        else if usable_frame_count == 0 { Some(UnavailableReason::NoUsableProfileEnergy) }
        else { None };
    let mut availability = ReferenceAvailability {
        energy_zero, profile_reason, loud_frame_count,
        fixed_region_tolerance: profile_reason.is_none() && loud_frame_count < 5,
        unavailable: vec![],
    };
    if let Some(reason) = profile_reason {
        availability.unavailable = ["regions", "tilt", "integrated", "plr", "range", "crest", "low_width",
            "attack", "decay", "sustain", "centroid", "noise", "tempo", "key", "busyness", "groove"]
            .into_iter().map(|quantity| UnavailableMetric { quantity: quantity.into(), reason }).collect();
        return (None, availability);
    }
    let mut profile = Profile::of(name, &heard);
    // The detailed meter's unchanged mono convention feeds the same channel into
    // both ears. That is not a measured stereo width and must not become a target.
    if channels == 1 {
        profile.low_width = None;
        profile.sound.width = None;
    }
    let m = &heard.measures;
    let mut absent = |quantity: &str, missing: bool, reason: UnavailableReason| {
        if missing { availability.unavailable.push(UnavailableMetric { quantity: quantity.into(), reason }); }
    };
    let loudness_reason = if m.seconds < 0.4 { UnavailableReason::TooShortForLoudness } else { UnavailableReason::BelowLoudnessGate };
    absent("integrated", profile.integrated.is_none(), loudness_reason);
    absent("plr", profile.plr.is_none(), loudness_reason);
    absent("range", profile.range.is_none(), if m.seconds < 3. { UnavailableReason::TooShortForRange } else { UnavailableReason::BelowLoudnessGate });
    absent("crest", profile.crest.is_none(), if m.seconds < 0.4 { UnavailableReason::TooShortForLoudness } else { UnavailableReason::NotMeasured });
    absent("low_width", profile.low_width.is_none(), if channels == 1 { UnavailableReason::Mono } else { UnavailableReason::NoLowBandEnergy });
    absent("sound.width", profile.sound.width.is_none(), if channels == 1 { UnavailableReason::Mono } else { UnavailableReason::NotMeasured });
    for (quantity, missing) in [
        ("attack", profile.attack.is_none()), ("decay", profile.decay.is_none()),
        ("sustain", profile.sustain.is_none()), ("centroid", profile.centroid.is_none()),
        ("noise", profile.noise.is_none()), ("tempo", profile.tempo.is_none()),
        ("key", profile.key.is_none()), ("busyness", profile.busyness.is_none()),
        ("groove", profile.groove.is_none()),
        ("sound.pitch_drop", profile.sound.pitch_drop.is_none()), ("sound.modulation", profile.sound.modulation.is_none()),
        ("sound.bandwidth", profile.sound.bandwidth.is_none()), ("sound.noise_floor", profile.sound.noise_floor.is_none()),
        ("sound.warmth", profile.sound.warmth.is_none()), ("sound.tail", profile.sound.tail.is_none()),
        ("sound.crackle", profile.sound.crackle.is_none()), ("sound.decay_time", profile.sound.decay_time.is_none()),
        ("sound.darkening", profile.sound.darkening.is_none()), ("sound.echo_time", profile.sound.echo_time.is_none()),
        ("sound.echo_falls", profile.sound.echo_falls.is_none()), ("sound.swing", profile.sound.swing.is_none()),
        ("sound.sweep", profile.sound.sweep.is_none()), ("sound.tail_share", profile.sound.tail_share.is_none()),
        ("sound.pumping", profile.sound.pumping.is_none()), ("sound.hit_crest", profile.sound.hit_crest.is_none()),
    ] { absent(quantity, missing, UnavailableReason::NotMeasured); }
    bandwidth_availability(&mut availability, m.sample_rate);
    (Some(profile), availability)
}

/// Availability describes the existing meter's support; it never changes its bands or substitutes zeros.
fn bandwidth_availability(availability: &mut ReferenceAvailability, rate: f64) {
    let nyquist = rate / 2.;
    let mut check = |quantity: String, low: f64, high: f64| {
        let reason = if nyquist <= low { Some(UnavailableReason::AboveNyquist) }
            else if nyquist < high { Some(UnavailableReason::PartialFrequencyCoverage) } else { None };
        if let Some(reason) = reason {
            if let Some(missing) = availability.unavailable.iter_mut().find(|missing| missing.quantity == quantity) {
                missing.reason = reason;
            } else {
                availability.unavailable.push(UnavailableMetric { quantity, reason });
            }
        }
    };
    for (name, from, to) in REGIONS {
        check(format!("region/{name}"), band_span(from).0, band_span(to).1);
    }
    check("tilt".into(), band_span(7).0, band_span(27).1);
    check("centroid".into(), band_span(0).0, band_span(30).1);
    let half_bin = 2f64.powf(1. / 24.);
    check("noise".into(), fine_hz(0) / half_bin, fine_hz(FINE_BINS - 1) * half_bin);
}

struct WavWriter {
    file: File,
    channels: usize,
    remaining: usize,
    buffer: Vec<u8>,
}

impl WavWriter {
    fn new(retained: RetainedWav, rate: u32, channels: usize, frames: usize) -> Result<Self, AudioError> {
        let data = frames.checked_mul(channels).and_then(|n| n.checked_mul(4))
            .and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= u32::MAX - 36)
            .ok_or_else(|| AudioError("That retained excerpt is too large for float32 WAV.".into()))?;
        if u64::from(data) + 44 > retained.max_bytes {
            return Err(AudioError("That retained float32 excerpt exceeds the artifact byte limit; choose a shorter window.".into()));
        }
        let channels_u16 = u16::try_from(channels).ok().filter(|n| *n > 0 && *n <= u16::MAX / 4)
            .ok_or_else(|| AudioError("That channel count can't be retained as float32 WAV.".into()))?;
        let byte_rate = rate.checked_mul(u32::from(channels_u16) * 4)
            .ok_or_else(|| AudioError("That sample rate/channel count can't be retained as float32 WAV.".into()))?;
        let mut file = retained.file;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + data).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&3u16.to_le_bytes())?;
        file.write_all(&channels_u16.to_le_bytes())?;
        file.write_all(&rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&(channels_u16 * 4).to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data.to_le_bytes())?;
        Ok(Self { file, channels, remaining: frames, buffer: Vec::new() })
    }

    fn push(&mut self, block: &[Vec<f32>]) -> Result<(), AudioError> {
        let frames = block[0].len();
        self.buffer.clear();
        self.buffer.reserve(frames * self.channels * 4);
        for frame in 0..frames {
            for channel in block { self.buffer.extend_from_slice(&channel[frame].to_le_bytes()); }
        }
        self.file.write_all(&self.buffer)?;
        self.remaining -= frames;
        Ok(())
    }

    fn finish(mut self) -> Result<(), AudioError> {
        if self.remaining != 0 { return Err(AudioError("The retained excerpt is incomplete.".into())); }
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_media::audio::{analyze::{analyze_file, AnalyzeOptions}, decode::float_wav};
    use std::io::Read;

    fn options(from: f64, seconds: f64, profile: bool) -> ReferenceOptions {
        ReferenceOptions {
            window: Some(ReferenceWindow { from, seconds }),
            profile,
            decode: DecodeOptions { installed_only: true, ffmpeg: None },
            ..Default::default()
        }
    }

    fn source(channels: &[Vec<f32>], rate: u32) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        file.write_all(&float_wav(channels, rate)).unwrap();
        file
    }

    fn tone(frames: usize, rate: u32, amplitude: f32) -> Vec<f32> {
        (0..frames).map(|i| ((i as f64 * 440. * std::f64::consts::TAU / rate as f64).sin() as f32) * amplitude).collect()
    }

    #[tokio::test]
    async fn exact_excerpt_feeds_both_existing_meters_and_float32_replay_at_zero() {
        let rate = 96_000;
        let channels = vec![tone(60_000, rate, 1.2), tone(60_000, rate, 0.4)];
        let input = source(&channels, rate);
        let output = tempfile::NamedTempFile::new().unwrap();
        let measured = measure_reference(
            input.path().to_str().unwrap(), "original.wav", options(0.125, 0.5, true),
            Some(RetainedWav { file: output.reopen().unwrap(), max_bytes: 50 * 1024 * 1024 }),
        ).await.unwrap();
        assert_eq!(measured.window.start_frame, 12_000);
        assert_eq!(measured.window.frame_count, 48_000);
        assert_eq!(measured.window.original_total_frames, Some(60_000));
        assert_eq!(measured.window.original_seconds, Some(0.625));
        assert!(measured.window.original_duration_exact);
        assert_eq!(measured.window.sample_rate, 96_000.);
        assert_eq!(measured.window.channels, 2);
        assert_eq!(measured.window.replay_from, 0.);
        assert_eq!(measured.analysis.seconds, 0.63);

        let mut canonical = analyze_file(input.path().to_str().unwrap(), AnalyzeOptions {
            focus: Some("mix".into()), start: Some(0.125), seconds: Some(0.5),
            decode: DecodeOptions { installed_only: true, ffmpeg: None }, ..Default::default()
        }).await.unwrap();
        canonical.file = "original.wav".into();
        assert_eq!(measured.analysis, canonical);
        let heard = crate::measure::measure_samples(&channels[0][12_000..], &channels[1][12_000..], rate as f64);
        assert_eq!(measured.profile, Some(Profile::of("original.wav", &heard)));
        assert!(heard.embedding.is_none());
        let selected: Vec<Vec<f32>> = channels.iter().map(|channel| channel[12_000..].to_vec()).collect();
        let mut bytes = vec![];
        output.reopen().unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, float_wav(&selected, rate));
        assert!(measured.analysis.loudness.sample_peak_dbfs > 0.);
    }

    #[tokio::test]
    async fn silence_shortness_and_missing_lufs_are_not_conflated() {
        let silent = source(&[vec![0.; 24_000]], 48_000);
        let measured = measure_reference(silent.path().to_str().unwrap(), "silence.wav", options(0., 0.5, true), None).await.unwrap();
        assert!(measured.availability.energy_zero);
        assert_eq!(measured.availability.profile_reason, Some(UnavailableReason::ZeroEnergy));
        assert!(measured.profile.is_none());

        let gated = source(&[tone(24_000, 48_000, 0.0001)], 48_000);
        let measured = measure_reference(gated.path().to_str().unwrap(), "quiet.wav", options(0., 0.5, true), None).await.unwrap();
        assert!(!measured.availability.energy_zero);
        assert!(measured.profile.as_ref().unwrap().integrated.is_none());
        assert!(measured.availability.unavailable.contains(&UnavailableMetric {
            quantity: "integrated".into(), reason: UnavailableReason::BelowLoudnessGate,
        }));

        let near_zero = source(&[tone(24_000, 48_000, 1e-20)], 48_000);
        let measured = measure_reference(near_zero.path().to_str().unwrap(), "near-zero.wav", options(0., 0.5, true), None).await.unwrap();
        assert!(!measured.availability.energy_zero);
        assert_eq!(measured.availability.profile_reason, Some(UnavailableReason::NoUsableProfileEnergy));
        assert!(measured.profile.is_none());

        let short = source(&[tone(17_280, 48_000, 0.25)], 48_000);
        let measured = measure_reference(short.path().to_str().unwrap(), "short.wav", options(0., 0.36, true), None).await.unwrap();
        assert!(!measured.availability.energy_zero);
        let profile = measured.profile.unwrap();
        assert!(profile.integrated.is_none());
        assert!(profile.low_width.is_none());
        assert!(profile.sound.width.is_none());
        assert!(measured.availability.unavailable.contains(&UnavailableMetric {
            quantity: "integrated".into(), reason: UnavailableReason::TooShortForLoudness,
        }));
        assert!(measured.availability.unavailable.contains(&UnavailableMetric {
            quantity: "low_width".into(), reason: UnavailableReason::Mono,
        }));
        assert!(measured.analysis.stereo.is_none());

        let measured = measure_reference(short.path().to_str().unwrap(), "short.wav", options(0., 0.1, true), None).await.unwrap();
        assert!(!measured.availability.energy_zero);
        assert_eq!(measured.availability.profile_reason, Some(UnavailableReason::TooShortForProfileFrames));
        assert!(measured.profile.is_none());
    }

    #[tokio::test]
    async fn eof_multichannel_and_retained_size_fail_without_truncating_or_downmixing() {
        let input = source(&[tone(24_000, 48_000, 0.25)], 48_000);
        let output = tempfile::NamedTempFile::new().unwrap();
        let error = measure_reference(
            input.path().to_str().unwrap(), "song.wav", options(0.25, 0.5, false),
            Some(RetainedWav { file: output.reopen().unwrap(), max_bytes: 50 * 1024 * 1024 }),
        ).await.unwrap_err();
        assert!(error.0.starts_with("That window runs past the end of song.wav"));
        assert_eq!(output.as_file().metadata().unwrap().len(), 0);

        let error = measure_reference(
            input.path().to_str().unwrap(), "song.wav", options(0., 0.5, false),
            Some(RetainedWav { file: output.reopen().unwrap(), max_bytes: 44 + 24_000 * 4 - 1 }),
        ).await.unwrap_err();
        assert!(error.0.contains("artifact byte limit"));
        assert_eq!(output.as_file().metadata().unwrap().len(), 0);

        let input = source(&vec![vec![0.1; 24_000]; 3], 48_000);
        let error = measure_reference(input.path().to_str().unwrap(), "three.wav", options(0., 0.5, true), None).await.unwrap_err();
        assert!(error.0.contains("not 3 channels"));
        let measured = measure_reference(input.path().to_str().unwrap(), "three.wav", options(0., 0.5, false), None).await.unwrap();
        assert_eq!(measured.analysis.channels, 3);
        assert!(measured.profile.is_none());
        assert_eq!(measured.availability.profile_reason, Some(UnavailableReason::NotRequested));
    }

    #[tokio::test]
    async fn decoder_policy_and_cancellation_are_checked_without_tool_acquisition() {
        let input = source(&[vec![0.1; 24_000]], 48_000);
        let mut unrestricted = options(0., 0.5, false);
        unrestricted.decode = DecodeOptions::default();
        let error = measure_reference(input.path().to_str().unwrap(), "song.wav", unrestricted, None).await.unwrap_err();
        assert!(error.0.contains("installed-only"));
        let compressed = tempfile::Builder::new().suffix(".mp3").tempfile().unwrap();
        let error = measure_reference(compressed.path().to_str().unwrap(), "song.mp3", options(0., 0.5, true), None).await.unwrap_err();
        assert!(error.0.contains("approved ffmpeg dependency is unavailable"));

        let signal = Signal::new();
        signal.cancel();
        let mut cancelled = options(0., 0.5, true);
        cancelled.signal = Some(signal);
        assert!(measure_reference(input.path().to_str().unwrap(), "song.wav", cancelled, None).await.is_err());
    }

    #[tokio::test]
    async fn windows_at_or_below_twenty_ms_never_produce_metrics_or_retained_samples() {
        let input = source(&[tone(24_000, 48_000, 0.25)], 48_000);
        for seconds in [0.005, 0.02] {
            let output = tempfile::NamedTempFile::new().unwrap();
            let measured = measure_reference(input.path().to_str().unwrap(), "short.wav", options(0., seconds, true),
                Some(RetainedWav { file: output.reopen().unwrap(), max_bytes: 50 * 1024 * 1024 })).await;
            assert!(measured.is_err());
            assert_eq!(output.as_file().metadata().unwrap().len(), 0, "refused canonical windows never reach the WAV writer");
        }
    }

    #[test]
    fn explicit_windows_require_finite_nonnegative_start_and_at_most_sixty_seconds() {
        for (from, seconds) in [(f64::NAN, 1.), (f64::INFINITY, 1.), (-1., 1.), (0., 0.), (0., -1.), (0., f64::NAN), (0., 60.001)] {
            assert!(validate_window(Some(ReferenceWindow { from, seconds })).is_err());
        }
        assert!(validate_window(Some(ReferenceWindow { from: 0., seconds: 60. })).is_ok());
    }
}
