use ableton_mcp_server::key_estimation::{estimate_key, KeyConfidence, KeyEstimateNote};

fn melody(pitches: &[i32], velocity: Option<f64>, beats_per_note: f64) -> Vec<KeyEstimateNote> {
    pitches
        .iter()
        .enumerate()
        .map(|(index, pitch)| KeyEstimateNote {
            pitch: *pitch as f64,
            start: index as f64 * beats_per_note,
            duration: beats_per_note,
            velocity,
        })
        .collect()
}

/// Deterministic pseudo-random helper for generated material.
fn random(seed: u32) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        state as f64 / 4_294_967_296.0
    }
}

const C_MAJOR_MELODY: [i32; 20] = [60, 62, 64, 65, 67, 67, 64, 62, 60, 60, 67, 64, 62, 60, 64, 67, 65, 64, 62, 60];
const A_MINOR_MELODY: [i32; 20] = [57, 60, 64, 62, 60, 59, 57, 57, 64, 60, 59, 57, 60, 64, 65, 64, 62, 60, 59, 57];

#[test]
fn known_key_synthetic_melodies_rank_the_expected_key_first() {
    let c_major = estimate_key(&melody(&C_MAJOR_MELODY, None, 0.5));
    assert_eq!(c_major.candidates[0].key, "C major");
    assert!(c_major.confidence == KeyConfidence::High || c_major.confidence == KeyConfidence::Medium);

    let transposed: Vec<i32> = C_MAJOR_MELODY.iter().map(|pitch| pitch + 7).collect();
    let transposed = estimate_key(&melody(&transposed, None, 0.5));
    assert_eq!(transposed.candidates[0].key, "G major");

    let a_minor = estimate_key(&melody(&A_MINOR_MELODY, None, 0.5));
    let top_keys: Vec<&str> = a_minor.candidates.iter().take(3).map(|candidate| candidate.key.as_str()).collect();
    assert!(top_keys.contains(&"A minor"), "expected A minor in top candidates, got {}", top_keys.join(", "));
    // The relative major/minor pair must be reported as alternatives, never suppressed.
    assert!(a_minor.candidates.iter().any(|candidate| candidate.key == "C major"));
}

#[test]
fn estimates_are_deterministic_and_order_independent() {
    let mut next = random(0x5eed);
    let pitches: Vec<i32> = (0..32).map(|_| 48 + (next() * 24.0).floor() as i32).collect();
    let notes = melody(&pitches, None, 0.5);
    let baseline = estimate_key(&notes);
    for _ in 0..25 {
        assert_eq!(estimate_key(&notes), baseline);
    }
    let shuffled: Vec<KeyEstimateNote> = notes.iter().rev().copied().collect();
    assert_eq!(estimate_key(&shuffled), baseline);
}

#[test]
fn duplicate_boundary_notes_with_different_weights_are_order_independent() {
    let notes = [
        KeyEstimateNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: Some(30.0) },
        KeyEstimateNote { pitch: 60.0, start: 0.0, duration: 2.0, velocity: Some(127.0) },
        KeyEstimateNote { pitch: 64.0, start: 1.0, duration: 0.5, velocity: Some(64.0) },
        KeyEstimateNote { pitch: 67.0, start: 2.0, duration: 1.0, velocity: Some(100.0) },
    ];
    let reversed: Vec<KeyEstimateNote> = notes.iter().rev().copied().collect();
    assert_eq!(estimate_key(&notes), estimate_key(&reversed));
}

#[test]
fn velocity_is_a_documented_secondary_weight_and_omission_equals_full_velocity() {
    let pitches = C_MAJOR_MELODY;
    let without_velocity = estimate_key(&melody(&pitches, None, 0.5));
    let full_velocity = estimate_key(&melody(&pitches, Some(127.0), 0.5));
    assert_eq!(full_velocity, without_velocity);
    // Uniform velocity is a uniform weight scaling and cannot change correlations;
    // differentiating velocity between notes changes the profile deterministically.
    let uniform_soft = estimate_key(&melody(&pitches, Some(40.0), 0.5));
    assert_eq!(uniform_soft, without_velocity);
    let accented: Vec<KeyEstimateNote> = pitches
        .iter()
        .enumerate()
        .map(|(index, pitch)| KeyEstimateNote {
            pitch: *pitch as f64,
            start: index as f64 * 0.5,
            duration: 0.5,
            velocity: Some(if index % 2 == 0 { 127.0 } else { 30.0 }),
        })
        .collect();
    assert_ne!(estimate_key(&accented), without_velocity);
}

#[test]
fn insufficient_evidence_cases_never_produce_a_forced_answer() {
    let cases: Vec<Vec<KeyEstimateNote>> = vec![
        Vec::new(),
        melody(&[60], None, 0.5),
        melody(&[60, 64], None, 0.5),
        melody(&[60, 60, 60, 60, 60], None, 0.5),
        melody(&[60, 61, 60, 61, 60], None, 0.5),
    ];
    for notes in cases {
        let estimate = estimate_key(&notes);
        assert_eq!(estimate.confidence, KeyConfidence::InsufficientEvidence);
        assert!(!estimate.ambiguous);
        assert_eq!(estimate.evidence.note_count, notes.len());
    }
    let chromatic = estimate_key(&melody(&[60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 60, 61, 62, 63], None, 0.5));
    assert_eq!(chromatic.confidence, KeyConfidence::InsufficientEvidence);
    assert!(chromatic.evidence.chromatic);
}

#[test]
fn ambiguous_material_reports_alternatives_instead_of_a_forced_key() {
    // Whole-tone-ish material: equidistant between two key centers.
    let whole_tone = melody(&[60, 62, 64, 66, 68, 70, 60, 62, 64, 66, 68, 70, 61, 63, 65, 67, 69, 71, 61, 63, 65, 67, 69, 71], None, 0.5);
    let estimate = estimate_key(&whole_tone);
    assert_ne!(estimate.confidence, KeyConfidence::High);
    if estimate.ambiguous {
        assert!(estimate.alternatives.len() >= 2);
    }
}

#[test]
fn out_of_shape_notes_are_skipped_honestly() {
    let estimate = estimate_key(&[
        KeyEstimateNote { pitch: 200.0, start: 0.0, duration: 1.0, velocity: None },
        KeyEstimateNote { pitch: 60.0, start: 0.0, duration: -4.0, velocity: None },
        KeyEstimateNote { pitch: 60.0, start: 0.0, duration: 1.0, velocity: None },
        KeyEstimateNote { pitch: 64.0, start: 1.0, duration: 1.0, velocity: None },
        KeyEstimateNote { pitch: 67.0, start: 2.0, duration: 1.0, velocity: None },
    ]);
    assert_eq!(estimate.evidence.note_count, 3);
    assert_eq!(estimate.evidence.pitch_class_count, 3);
}

#[test]
fn serializes_as_the_typescript_did() {
    let estimate = estimate_key(&melody(&C_MAJOR_MELODY, None, 0.5));
    let json = kumi_common::js::json::stringify(&serde_json::to_value(&estimate).unwrap());
    assert!(json.starts_with("{\"candidates\":[{\"key\":\"C major\",\"tonic\":0,\"mode\":\"major\",\"score\":"), "{json}");
    assert!(json.contains("\"confidence\":\"high\"") || json.contains("\"confidence\":\"medium\""));
    assert!(json.ends_with("\"algorithm\":\"krumhansl-schmuckler-1982+modes(duration*velocity/127,tonal-center-tiebreak)\"}}"), "{json}");
    assert_eq!(
        kumi_common::js::json::stringify(&serde_json::to_value(KeyConfidence::InsufficientEvidence).unwrap()),
        "\"insufficient-evidence\""
    );
    assert_eq!(estimate.evidence.total_duration_beats, 10.0);
}
