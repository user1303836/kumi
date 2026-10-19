//! Library measurement and classification, with differential cases.
#[path = "support/library.rs"]
mod fixtures;
use fixtures::*;
use kumi_runtime::library::{
    classify::{classify, name_hints, ClassFrom, SoundClass, SoundKind},
    features::{measure_samples, measure_sound, MeasureOptions, VECTOR_LENGTH},
};
use serde_json::Value;
use std::f64::consts::PI;
fn compare(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-10, "{path}: {a} != {b}"),
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                compare(a, b, &format!("{path}/{i}"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>(), "{path}");
            for (k, a) in a {
                compare(a, &b[k], &format!("{path}/{k}"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}
#[test]
fn complete_measurements_match_typescript_across_sounds_and_sample_rates() {
    let cases: Value = serde_json::from_str(include_str!("support/library-features-oracle.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let kind = case["kind"].as_str().unwrap();
        let rate = case["rate"].as_f64().unwrap();
        let seconds = case["seconds"].as_f64().unwrap();
        let length = kumi_common::js::number::round(rate * seconds) as usize;
        let channels = match kind {
            "kick" => vec![kick(50., 0.5)],
            "hat" => vec![hat(0.12, 3)],
            "snare" => vec![snare(0.25)],
            "beat" => vec![beat(120., 2)],
            "pad" => vec![pad(&[220., 261.63, 329.63], 4.)],
            "silence" | "empty" => vec![vec![0.; length]],
            "stereo" => (0..2)
                .map(|channel| {
                    (0..length).map(|i| (0.3 * (2. * PI * if channel == 1 { 446. } else { 440. } * i as f64 / rate).sin()) as f32).collect()
                })
                .collect(),
            "anti" => {
                let left: Vec<_> = (0..length).map(|i| (0.3 * (2. * PI * 440. * i as f64 / rate).sin()) as f32).collect();
                vec![left.clone(), left.iter().map(|v| -v).collect()]
            }
            _ => vec![(0..length).map(|i| (0.25 * (2. * PI * 440. * i as f64 / rate).sin()) as f32).collect()],
        };
        let actual = serde_json::to_value(measure_samples(&channels, rate, seconds)).unwrap();
        compare(&actual, &case["expected"], &format!("{kind}/{rate}"));
    }
}
#[test]
fn measurements_decide_class_kind_tempo_and_note() {
    let measured = |samples: Vec<f32>| {
        let seconds = samples.len() as f64 / RATE;
        measure_samples(&[samples], RATE, seconds)
    };
    let low = measured(kick(50., 0.5));
    let bright = measured(hat(0.12, 3));
    let looping = measured(beat(120., 2));
    let chord = measured(pad(&[220., 261.63, 329.63], 4.));
    assert_eq!(low.vector.len(), VECTOR_LENGTH);
    assert!(low.low_share > 0.8 && low.centroid_hz < 300.);
    assert!(bright.centroid_hz > 5000. && bright.flatness > bright.low_share);
    assert!((low.pitch.unwrap().hz - 50.).abs() < 3.);
    assert!(chord.attack_ms > 100.);
    let kick = classify(&name_hints("Untitled 1.wav"), &low.heard());
    assert_eq!(kick.class, Some(SoundClass::Kick));
    assert_eq!(kick.class_from, Some(ClassFrom::Sound));
    assert_eq!(kick.kind, SoundKind::OneShot);
    assert_eq!(kick.note.as_deref(), Some("G1"));
    assert_eq!(classify(&name_hints("Untitled 2.wav"), &bright.heard()).class, Some(SoundClass::Hat));
    let looping = classify(&name_hints("Audio 3.wav"), &looping.heard());
    assert_eq!(looping.kind, SoundKind::Loop);
    assert_eq!(looping.bpm, Some(120.));
    assert_eq!(looping.class, Some(SoundClass::Drums));
    let mut heard = low.heard();
    heard.pitch = Some(kumi_runtime::library::classify::Pitch { hz: 261.6, confidence: 0.9 });
    assert_eq!(classify(&name_hints("Stab Gabon C.wav"), &heard).note.as_deref(), Some("C4"));
    assert_eq!(classify(&name_hints("Hihat Closed Break 2.wav"), &bright.heard()).kind, SoundKind::OneShot);
}
#[tokio::test]
async fn files_keep_full_duration_measure_requested_part_and_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kick.wav");
    let samples = kick(50., 1.);
    std::fs::write(&path, wav(&[samples.clone()], 44100)).unwrap();
    let path = path.to_str().unwrap();
    let full = measure_sound(path, MeasureOptions::default()).await.unwrap();
    assert_eq!(full.seconds, 1.);
    let part = measure_sound(path, MeasureOptions { start: Some(0.25), seconds: Some(0.2), signal: None }).await.unwrap();
    assert_eq!(part.seconds, 1.);
    assert!(part.peak_db < full.peak_db);
    let quantized: Vec<_> =
        samples.iter().map(|v| (kumi_common::js::number::round(*v as f64 * 32767.).clamp(-32768., 32767.) / 32768.) as f32).collect();
    let expected = measure_samples(&[quantized[11025..19845].to_vec()], 44100., 1.);
    assert_eq!(part, expected);
    let error = measure_sound(path, MeasureOptions { start: Some(2.), ..Default::default() }).await.unwrap_err();
    assert_eq!(error.to_string(), "There's no audio in that part of the file.");
    let signal = kumi_common::abort::Signal::new();
    signal.cancel();
    assert!(measure_sound(path, MeasureOptions { signal: Some(signal), ..Default::default() }).await.is_err());
}
