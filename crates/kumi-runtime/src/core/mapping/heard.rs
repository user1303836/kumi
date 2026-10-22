//! A sample heard for mapping: its pitch (as a MIDI note, from the pitch tracker over its first seconds) and how sure, its
//! loudness (K-weighted, over its loudest 400 ms: what decides velocity order) and its peak. Off Kumi's thread.
use super::HeardSample;
use crate::audio::{analyze::track_pitch, decode::open_audio_to, dsp::k_weighting};
use kumi_common::abort::Signal;

/// How much of a sample is heard: enough for a note's pitch and its attack's loudness.
const HEARD_SECONDS: f64 = 6.0;
/// The window loudness is measured over (seconds).
const LOUDNESS_WINDOW: f64 = 0.4;

/// A file heard, or why it couldn't be.
pub async fn hear_file(path: &str, signal: &Signal) -> Result<HeardSample, String> {
    let mut source = open_audio_to(path, Some(signal.clone()), Some(HEARD_SECONDS)).await.map_err(|e| e.0)?;
    let rate = source.sample_rate;
    let most = (rate * HEARD_SECONDS) as usize;
    let mut mono: Vec<f32> = Vec::new();
    while mono.len() < most {
        let Some(block) = source.read(65_536).await.map_err(|e| e.0)? else { break };
        let channels = block.len().max(1);
        let frames = block.first().map_or(0, Vec::len);
        for frame in 0..frames {
            mono.push(block.iter().map(|channel| channel[frame]).sum::<f32>() / channels as f32);
        }
    }
    let _ = source.close().await;
    mono.truncate(most);
    let seconds = source.frames as f64 / rate;
    tokio::task::spawn_blocking(move || heard_of(&mono, rate, seconds)).await.map_err(|_| "Kumi couldn't hear that file".to_string())
}

/// What mono audio sounds like, for mapping.
pub fn heard_of(mono: &[f32], rate: f64, seconds: f64) -> HeardSample {
    let peak = mono.iter().fold(0f32, |most, v| most.max(v.abs())) as f64;
    let [mut shelf, mut high] = k_weighting(rate);
    let weighted: Vec<f64> = mono.iter().map(|v| high.process(shelf.process(*v as f64))).collect();
    let window = ((rate * LOUDNESS_WINDOW) as usize).clamp(1, weighted.len().max(1));
    let mut loudest = 0f64;
    let mut sum: f64 = weighted.iter().take(window).map(|v| v * v).sum();
    loudest = loudest.max(sum);
    for k in window..weighted.len() {
        sum += weighted[k] * weighted[k] - weighted[k - window] * weighted[k - window];
        loudest = loudest.max(sum);
    }
    let level_db = -0.691 + 10. * (loudest / window as f64).max(1e-12).log10();
    let frames = track_pitch(mono, rate);
    let voiced: Vec<f64> = frames.iter().filter(|frame| frame.confidence > 0.6 && frame.hz > 0.).map(|frame| frame.hz).collect();
    let sure = if frames.is_empty() { 0. } else { voiced.len() as f64 / frames.len() as f64 };
    let pitch = (voiced.len() >= 3).then(|| {
        let mut sorted = voiced.clone();
        sorted.sort_by(f64::total_cmp);
        let hz = sorted[sorted.len() / 2];
        69. + 12. * (hz / 440.).log2()
    });
    HeardSample { pitch, sure, level_db, peak_db: 20. * peak.max(1e-9).log10(), seconds }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, amplitude: f32, seconds: f64) -> Vec<f32> {
        let rate = 44_100.;
        (0..(rate * seconds) as usize).map(|k| amplitude * (2. * std::f64::consts::PI * hz * k as f64 / rate).sin() as f32).collect()
    }

    #[test]
    fn a_tone_is_heard_at_its_note_and_its_level() {
        let heard = heard_of(&tone(261.63, 0.5, 1.5), 44_100., 1.5);
        assert!((heard.pitch.unwrap() - 60.).abs() < 0.2, "{heard:?}");
        assert!(heard.sure > 0.8);
        let quieter = heard_of(&tone(261.63, 0.25, 1.5), 44_100., 1.5);
        assert!((heard.level_db - quieter.level_db - 6.02).abs() < 0.2, "half the amplitude is 6 dB down");
    }

    #[test]
    fn noise_has_no_pitch() {
        let mut state = 12345u32;
        let noise: Vec<f32> = (0..44_100)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 16) as f32 / 32_768. - 1.
            })
            .collect();
        let heard = heard_of(&noise, 44_100., 1.);
        assert!(heard.pitch.is_none() || heard.sure < 0.5, "{heard:?}");
    }
}
