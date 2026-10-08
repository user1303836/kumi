//! Embeddings' inputs, made in Kumi as the models take them: CLAP's log-mel against its own feature extractor's on a
//! known signal, and audio resampled without moving its pitch. With KUMI_TEST_MODELS naming a folder that holds the
//! runtime and the models (Kumi's models folder layout), whole embeddings are checked against what the shipped models
//! gave for the same signal; tests never fetch. A style vector names the model that made it.
use kumi_runtime::listening::embed::{clap_features, distance, own_style_id, resample, style_id, CLAP_MELS};
use serde_json::Value;
use std::f64::consts::PI;

fn known_signal() -> Vec<f32> {
    (0..144_000)
        .map(|n| n as f64 / 48_000.)
        .map(|t| {
            0.3 * (2. * PI * 220. * t).sin()
                + 0.2 * (2. * PI * 330. * t).sin()
                + 0.1 * (2. * PI * 440. * t).sin()
                + 0.1 * (2. * PI * (2000. * t - 150. * t * t)).sin()
        })
        .map(|sample| sample as f32)
        .collect()
}

#[test]
fn clap_features_match_its_feature_extractor() {
    let fixture: Value = serde_json::from_str(include_str!("support/clap-fixture.json")).unwrap();
    let features = clap_features(&known_signal());
    let mean = features.iter().map(|value| *value as f64).sum::<f64>() / features.len() as f64;
    assert!((mean - fixture["mean"].as_f64().unwrap()).abs() < 1e-3, "{mean}");
    for (frame, values) in fixture["frames"].as_array().unwrap().iter().zip(fixture["values"].as_array().unwrap()) {
        let frame = frame.as_u64().unwrap() as usize;
        for (mel, expected) in values.as_array().unwrap().iter().enumerate() {
            let got = features[frame * CLAP_MELS + mel] as f64;
            assert!((got - expected.as_f64().unwrap()).abs() < 0.01, "frame {frame}, band {mel}: {got} against {expected}");
        }
    }
}

#[test]
fn resampling_keeps_the_pitch_and_the_level() {
    // A 1 kHz tone at 44.1 kHz, to 48 kHz: the same tone, a sample longer for every 11 kHz of rate.
    let tone: Vec<f32> = (0..44_100).map(|n| (0.5 * (2. * PI * 1000. * n as f64 / 44_100.).sin()) as f32).collect();
    let up = resample(&tone, 44_100., 48_000.);
    assert_eq!(up.len(), 48_000);
    let middle = &up[4_800..43_200];
    let crossings = middle.windows(2).filter(|pair| pair[0] < 0. && pair[1] >= 0.).count();
    assert!((crossings as f64 - 800.).abs() <= 1., "{crossings} cycles in 0.8 s");
    let peak = middle.iter().fold(0f32, |peak, sample| peak.max(sample.abs()));
    assert!((peak - 0.5).abs() < 0.01, "{peak}");
    // Down to 22.05 kHz, a tone over the new half rate is gone.
    let high: Vec<f32> = (0..48_000).map(|n| (0.5 * (2. * PI * 15_000. * n as f64 / 48_000.).sin()) as f32).collect();
    let down = resample(&high, 48_000., 22_050.);
    let left = down[2_000..20_000].iter().fold(0f32, |peak, sample| peak.max(sample.abs()));
    assert!(left < 0.01, "{left}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_known_signal_embeds_as_the_converted_model_did() {
    let Some(folder) = std::env::var("KUMI_TEST_MODELS").ok().filter(|folder| !folder.is_empty()) else {
        eprintln!("skipped: set KUMI_TEST_MODELS to a folder named models holding onnxruntime-1.23.2/ and clap-music-audio.onnx to run it");
        return;
    };
    let fixture: Value = serde_json::from_str(include_str!("support/clap-fixture.json")).unwrap();
    let expected: Vec<f32> = fixture["embedding"].as_array().unwrap().iter().map(|value| value.as_f64().unwrap() as f32).collect();
    // A WAV of the known signal, embedded the way Kumi embeds what it hears.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("known.wav");
    std::fs::write(&file, wav(&[known_signal()], 48_000)).unwrap();
    // Kumi finds its models in `models` in its folder.
    assert!(std::path::Path::new(&folder).ends_with("models"), "KUMI_TEST_MODELS has to be a folder named models: {folder}");
    std::env::set_var("KUMI_HOME", std::path::Path::new(&folder).parent().unwrap());
    let signal = kumi_common::abort::Signal::new();
    let (got, model) = kumi_runtime::listening::embed::vibe(&file, 0., 3., None, None, &|said| eprintln!("{said}"), &signal).await.unwrap();
    assert_eq!(model, own_style_id());
    let apart = distance(&got, &expected).unwrap();
    assert!(apart < 1e-3, "{apart}");
    // 4 dB louder: heard at a common loudness, it's the same style; heard as it is, it isn't.
    let louder = dir.path().join("louder.wav");
    std::fs::write(&louder, wav(&[known_signal().iter().map(|sample| sample * 10f32.powf(0.2)).collect()], 48_000)).unwrap();
    let vibe = |file: std::path::PathBuf, loudness: Option<f64>| {
        let signal = signal.clone();
        async move { kumi_runtime::listening::embed::vibe(&file, 0., 3., loudness, None, &|_| {}, &signal).await.unwrap().0 }
    };
    let matched = distance(&vibe(file.clone(), Some(-20.)).await, &vibe(louder.clone(), Some(-16.)).await).unwrap();
    let unmatched = distance(&got, &vibe(louder, None).await).unwrap();
    eprintln!("4 dB louder: {unmatched:.4} apart as heard, {matched:.5} at a common loudness");
    assert!(matched < 1e-4 && unmatched > 10. * matched, "{matched} {unmatched}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_known_signals_effects_embed_as_the_converted_model_did() {
    let Some(folder) = std::env::var("KUMI_TEST_MODELS").ok().filter(|folder| !folder.is_empty()) else {
        eprintln!("skipped: set KUMI_TEST_MODELS to a folder named models holding onnxruntime-1.23.2/ and afx-rep.onnx to run it");
        return;
    };
    let fixture: Value = serde_json::from_str(include_str!("support/afx-fixture.json")).unwrap();
    let unit = |key: &str| {
        let values: Vec<f32> = fixture[key].as_array().unwrap().iter().map(|value| value.as_f64().unwrap() as f32).collect();
        kumi_runtime::listening::embed::normalized(values)
    };
    let expected: Vec<f32> = unit("mid").into_iter().chain(unit("side")).collect();
    // The right side 10 ms behind the left and 2 dB down, as the fixture was made.
    let left = known_signal();
    let right: Vec<f32> = (0..left.len()).map(|n| left[(n + left.len() - 480) % left.len()] * 0.8).collect();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("known-stereo.wav");
    std::fs::write(&file, wav(&[left, right], 48_000)).unwrap();
    // Kumi finds its models in `models` in its folder.
    assert!(std::path::Path::new(&folder).ends_with("models"), "KUMI_TEST_MODELS has to be a folder named models: {folder}");
    std::env::set_var("KUMI_HOME", std::path::Path::new(&folder).parent().unwrap());
    let signal = kumi_common::abort::Signal::new();
    let got = kumi_runtime::listening::embed::effects(&file, 0., 3., &|said| eprintln!("{said}"), &signal).await.unwrap();
    let apart = distance(&got, &expected).unwrap();
    assert!(apart < 1e-3, "{apart}");
}

/// A 32-bit float WAV of the channels given (as they are: no rounding to hear past).
fn wav(channels: &[Vec<f32>], rate: u32) -> Vec<u8> {
    let count = channels.len() as u16;
    let data: Vec<u8> =
        (0..channels[0].len()).flat_map(|n| channels.iter().map(move |channel| channel[n])).flat_map(f32::to_le_bytes).collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&3u16.to_le_bytes());
    wav.extend_from_slice(&count.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4 * count as u32).to_le_bytes());
    wav.extend_from_slice(&(4 * count).to_le_bytes());
    wav.extend_from_slice(&32u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

#[tokio::test(flavor = "current_thread")]
async fn a_style_vector_names_the_model_that_made_it() {
    // No slot file, or one that's gone: Kumi's own, by its pinned SHA-256.
    assert_eq!(style_id(None).await, own_style_id());
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(style_id(Some(&dir.path().join("gone.onnx"))).await, own_style_id());
    // A slot's file: its own SHA-256, the same each time, and another once the file changes.
    let file = dir.path().join("clap.onnx");
    std::fs::write(&file, b"one model").unwrap();
    let first = style_id(Some(&file)).await;
    assert_eq!(first, "sha256:e7940ca0c091e09b23054a7756d26bebdbe8dd70c1250850a35a9561325c83b9");
    assert_eq!(style_id(Some(&file)).await, first);
    std::fs::write(&file, b"another model").unwrap();
    assert_ne!(style_id(Some(&file)).await, first);
}
