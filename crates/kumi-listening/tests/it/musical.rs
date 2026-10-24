//! The music a listen hears, beyond the mix, on synthetic material with known answers: a click track's tempo, a minor
//! chord's key, swung hats' swing.

use kumi_listening::{checklist::Profile, measure::measure_samples, musical};

const RATE: f64 = 22_050.;

/// A short burst of noise (a click, a hat) at each of `at` (seconds), `seconds` long in all; each `accent`th one twice
/// as loud.
fn hits(at: &[f64], seconds: f64, level: f32, accent: usize) -> Vec<f32> {
    let mut samples = vec![0f32; (seconds * RATE) as usize];
    let mut noise = 0x2545_f491u32;
    for (index, start) in at.iter().enumerate() {
        let level = if index % accent == 0 { level } else { level / 2. };
        let first = (start * RATE) as usize;
        for n in 0..(0.03 * RATE) as usize {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            if let Some(sample) = samples.get_mut(first + n) {
                let decay = (-(n as f32) / (0.006 * RATE as f32)).exp();
                *sample += level * decay * ((noise as f32 / u32::MAX as f32) * 2. - 1.);
            }
        }
    }
    samples
}

fn tones(hz: &[f64], seconds: f64) -> Vec<f32> {
    (0..(seconds * RATE) as usize)
        .map(|n| hz.iter().map(|hz| (2. * std::f64::consts::PI * hz * n as f64 / RATE).sin()).sum::<f64>() as f32 * 0.1)
        .collect()
}

fn heard(mono: &[f32]) -> kumi_listening::measure::Heard {
    measure_samples(mono, mono, RATE)
}

#[test]
fn a_click_tracks_tempo_is_its_beat() {
    let beat = 60. / 128.;
    let clicks: Vec<f64> = (0..40).map(|n| 0.2 + n as f64 * beat).collect();
    let heard = heard(&hits(&clicks, 20., 0.8, 1));
    let tempo = musical::tempo(&heard.frames).unwrap();
    assert!((tempo - 128.).abs() < 1.5, "{tempo}");
    let profile = Profile::of("clicks", &heard);
    assert!(profile.tempo.is_some_and(|tempo| (tempo.mid - 128.).abs() < 1.5), "{:?}", profile.tempo);
    // A click a beat: four onsets a bar.
    assert!(profile.busyness.is_some_and(|busy| (busy.mid - 4.).abs() < 0.6), "{:?}", profile.busyness);
}

#[test]
fn a_minor_chord_over_its_root_is_in_its_key() {
    // A minor: A3, C4, E4 over A2.
    let heard = heard(&tones(&[220., 261.63, 329.63, 110.], 8.));
    assert_eq!(musical::key(&heard.frames).as_deref(), Some("A minor"));
    // And C major's own triad over C.
    let heard = self::heard(&tones(&[261.63, 329.63, 392., 130.81], 8.));
    assert_eq!(musical::key(&heard.frames).as_deref(), Some("C major"));
}

#[test]
fn swung_hats_heard_as_audio_read_as_swung() {
    // Sixteenth hats at 100 BPM, each pair's second step at 62 % of the eighth: a swing of about 1.6.
    let beat = 60. / 100.;
    let at: Vec<f64> = (0..96)
        .map(|step| {
            let pair = (step / 2) as f64 * beat / 2.;
            0.25 + if step % 2 == 1 { pair + beat / 2. * 0.62 } else { pair }
        })
        .collect();
    let heard = heard(&hits(&at, 16., 0.6, 4));
    let onsets = musical::onsets(&heard.frames);
    assert!((onsets.len() as i64 - 96).abs() <= 4, "{} onsets", onsets.len());
    let feel = musical::groove(&onsets, 100.).unwrap();
    assert!((feel.swing_ratio - 1.63).abs() < 0.2, "{feel:?}");
}
