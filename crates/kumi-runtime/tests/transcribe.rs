//! Pitched parts as notes with Basic Pitch: its decoding of the model's activations into notes (a predicted onset
//! followed while it sounds, and a note no onset was predicted for traced from its strongest point), and its frame
//! times. With KUMI_TEST_MODELS naming a local models folder, a known melody is transcribed whole; tests never fetch.
use kumi_runtime::listening::transcribe::{frame_time, notes_from, pitched_notes};

/// Activations for `count` frames: a note on key `key` (MIDI − 21) over `span`, and an onset at its first frame when
/// `onset`.
fn activations(count: usize, notes: &[(usize, std::ops::Range<usize>, bool)]) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let (mut frames, mut onsets) = (vec![vec![0f32; 88]; count], vec![vec![0f32; 88]; count]);
    for (key, span, onset) in notes {
        for t in span.clone() {
            frames[t][*key] = 0.8;
        }
        if *onset {
            onsets[span.start][*key] = 0.9;
        }
    }
    (frames, onsets)
}

#[test]
fn activations_read_into_notes_as_basic_pitch_reads_them() {
    // Middle C with its onset predicted, then a G held without one.
    let (frames, onsets) = activations(120, &[(39, 10..40, true), (46, 60..100, false)]);
    let mut notes = notes_from(&frames, &onsets);
    notes.sort_by_key(|note| (note.0, note.2));
    assert_eq!(notes.len(), 2, "{notes:?}");
    let (start, end, pitch, strength) = notes[0];
    assert_eq!((pitch, start, end), (60, 10, 40));
    assert!((strength - 0.8).abs() < 1e-6);
    // The held G: found from its strongest point, without an onset (its rise counts as one too).
    let (start, end, pitch, _) = notes[1];
    assert_eq!(pitch, 67);
    assert!(start.abs_diff(60) <= 1 && end.abs_diff(100) <= 1, "{start}..{end}");
    // Too short to be a note: 11 frames or fewer.
    let (frames, onsets) = activations(60, &[(39, 10..20, true)]);
    assert!(notes_from(&frames, &onsets).is_empty());
    // A window's frames are placed a little earlier than a hop each, as Basic Pitch corrects them.
    assert_eq!(frame_time(0), 0.);
    assert!((frame_time(86) - 86. * 256. / 22_050.).abs() < 1e-12);
    assert!((frame_time(172) - (172. * 256. / 22_050. - 0.010326)).abs() < 1e-5);
}

#[tokio::test(flavor = "current_thread")]
async fn a_known_melody_is_heard_as_its_notes() {
    let Some(folder) = std::env::var("KUMI_TEST_MODELS").ok().filter(|folder| !folder.is_empty()) else {
        eprintln!("skipped: set KUMI_TEST_MODELS to a folder named models holding onnxruntime-1.23.2/ and basic-pitch.onnx to run it");
        return;
    };
    // C3, E3, G3, C4, half a second each with a gap, a few harmonics each, at 44.1 kHz.
    let rate = 44_100.;
    let melody = [48, 52, 55, 60];
    let samples: Vec<f32> = (0..(rate * 2.4) as usize)
        .map(|n| {
            let t = n as f64 / rate;
            let (index, into) = ((t / 0.6) as usize, t % 0.6);
            match melody.get(index) {
                Some(midi) if into < 0.5 => {
                    let hz = 440. * 2f64.powf((*midi as f64 - 69.) / 12.);
                    let fade = (into / 0.01).min(1.) * ((0.5 - into) / 0.01).min(1.);
                    (1..=4).map(|k| (2. * std::f64::consts::PI * hz * k as f64 * t).sin() / k as f64).sum::<f64>() * 0.25 * fade
                }
                _ => 0.,
            }
        })
        .map(|sample| sample as f32)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("melody.wav");
    let data: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    let mut wav = vec![];
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&3u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&44_100u32.to_le_bytes());
    wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&32u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(&file, wav).unwrap();
    // Kumi finds its models in `models` in its folder.
    assert!(std::path::Path::new(&folder).ends_with("models"), "KUMI_TEST_MODELS has to be a folder named models: {folder}");
    std::env::set_var("KUMI_HOME", std::path::Path::new(&folder).parent().unwrap());
    let signal = kumi_common::abort::Signal::new();
    let heard = pitched_notes(&file, 0., 2.4, &|said| eprintln!("{said}"), &signal).await.unwrap();
    // Each note of the melody, the strongest heard where it starts (a harmonic can read as a quieter octave above).
    for (index, midi) in melody.iter().enumerate() {
        let at = index as f64 * 0.6;
        let strongest = heard
            .iter()
            .filter(|note| (note.start - at).abs() < 0.06)
            .max_by(|a, b| a.strength.total_cmp(&b.strength))
            .unwrap_or_else(|| panic!("nothing heard at {at} s: {heard:?}"));
        assert_eq!(strongest.pitch, *midi, "{heard:?}");
        assert!((strongest.end - strongest.start - 0.5).abs() < 0.12, "{strongest:?}");
    }
}
