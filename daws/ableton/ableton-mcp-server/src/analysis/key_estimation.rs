//! Deterministic musical key/scale estimation (read-only).
//!
//! Pitch-class profile: each note contributes `duration * (velocity / 127)`,
//! falling back to `duration` alone when velocity is absent. Durations use the
//! canonical beat units of the note schema; no time signature or tempo is
//! assumed.
//!
//! Correlation: Pearson product-moment correlation between the observed
//! pitch-class weight vector and the published Krumhansl-Schmuckler (1982)
//! major and minor key profiles. Modal candidates (dorian, phrygian, lydian,
//! mixolydian) use the documented approximation of rotating the major profile
//! to each mode's parent scale; locrian is excluded because the rotated
//! profile cannot distinguish it reliably.
//!
//! Relative modes share a diatonic collection, so pitch-class content alone
//! cannot separate them (C major vs A minor vs D dorian). Ranking therefore
//! adds a documented tonal-center tiebreak: the share of first/last note
//! weight (by start time, order-independent) that falls on the candidate's
//! tonic pitch class. Candidates whose scores are within the ambiguity margin
//! AND whose tonal-center evidence is indistinguishable are reported as
//! ambiguous alternatives, never a forced answer.
//!
//! Collections covering ten or more pitch classes are chromatic: no diatonic
//! candidate can explain them, so they report insufficient evidence.
//!
//! The estimate never forces a single answer: it reports ranked candidates
//! with scores, an explicit confidence classification, and an ambiguity flag
//! when the top candidates fall within heuristic margins (not calibrated probabilities).

use std::cmp::Ordering;

use kumi_common::js::number;
use serde::{Deserialize, Serialize};

/// A note as the key estimate reads it; `pitch` is a number the estimate checks to be an integer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEstimateNote {
    pub pitch: f64,
    pub start: f64,
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCandidate {
    pub key: String,
    pub tonic: usize,
    pub mode: String,
    pub score: f64,
    pub tonic_evidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyConfidence {
    High,
    Medium,
    Low,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEstimateEvidence {
    pub note_count: usize,
    pub pitch_class_count: usize,
    pub chromatic: bool,
    pub total_duration_beats: f64,
    pub algorithm: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEstimate {
    pub candidates: Vec<KeyCandidate>,
    pub confidence: KeyConfidence,
    pub ambiguous: bool,
    pub alternatives: Vec<KeyCandidate>,
    pub evidence: KeyEstimateEvidence,
}

const MAJOR_PROFILE: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.6, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Semitones from the mode's final up to its parent major scale's tonic.
const MODE_PARENT_OFFSETS: [(&str, usize); 4] = [("dorian", 10), ("phrygian", 8), ("lydian", 7), ("mixolydian", 5)];

const ALGORITHM_ID: &str = "krumhansl-schmuckler-1982+modes(duration*velocity/127,tonal-center-tiebreak)";
/// Top candidates closer than this heuristic margin remain ambiguous.
const AMBIGUITY_MARGIN: f64 = 0.05;
/// Tonal-center evidence closer than this is indistinguishable.
const TONIC_EVIDENCE_MARGIN: f64 = 0.01;
/// Below this best score no candidate explains the material.
const EVIDENCE_FLOOR: f64 = 0.4;
const HIGH_CONFIDENCE: f64 = 0.75;
const MEDIUM_CONFIDENCE: f64 = 0.55;
const MAX_CANDIDATES: usize = 5;

fn pearson(observed: &[f64; 12], expected: &[f64; 12]) -> f64 {
    let n = observed.len() as f64;
    let mean = |values: &[f64; 12]| values.iter().fold(0.0, |sum, value| sum + value) / n;
    let mean_o = mean(observed);
    let mean_e = mean(expected);
    let mut numerator = 0.0;
    let mut sum_sq_o = 0.0;
    let mut sum_sq_e = 0.0;
    for index in 0..observed.len() {
        let delta_o = observed[index] - mean_o;
        let delta_e = expected[index] - mean_e;
        numerator += delta_o * delta_e;
        sum_sq_o += delta_o * delta_o;
        sum_sq_e += delta_e * delta_e;
    }
    let denominator = (sum_sq_o * sum_sq_e).sqrt();
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

fn profile_for(tonic: usize, profile: &[f64; 12]) -> [f64; 12] {
    let mut vector = [0.0; 12];
    for (pitch_class, slot) in vector.iter_mut().enumerate() {
        *slot = profile[(pitch_class + 120 - tonic) % 12];
    }
    vector
}

/// A JavaScript comparator `a - b`: equal when the difference is zero or not a number.
fn compare(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

/// `Number(value.toFixed(6))`.
fn six_places(value: f64) -> f64 {
    number::to_fixed(value, 6).parse().unwrap_or(value)
}

struct BoundaryNote {
    pitch_class: usize,
    weight: f64,
    /// The first note's start, or the last note's end.
    edge: f64,
}

pub fn estimate_key(notes: &[KeyEstimateNote]) -> KeyEstimate {
    let mut weights = [0.0f64; 12];
    let mut note_count = 0usize;
    let mut total_duration_beats = 0.0;
    let mut first_note: Option<BoundaryNote> = None;
    let mut last_note: Option<BoundaryNote> = None;
    // Canonical accumulation and boundary selection prevent caller order from
    // changing floating-point sums or tied first/last-note weights.
    let mut ordered: Vec<&KeyEstimateNote> = notes.iter().collect();
    ordered.sort_by(|a, b| {
        compare(a.start, b.start)
            .then_with(|| compare(a.pitch, b.pitch))
            .then_with(|| compare(a.duration, b.duration))
            .then_with(|| compare(a.velocity.unwrap_or(127.0), b.velocity.unwrap_or(127.0)))
    });
    for note in ordered {
        if !note.start.is_finite() || note.start < 0.0 || note.start > 1_000_000.0 {
            continue;
        }
        if !(note.pitch.is_finite() && note.pitch.fract() == 0.0) || note.pitch < 0.0 || note.pitch > 127.0 {
            continue;
        }
        if !note.duration.is_finite() || note.duration <= 0.0 || note.duration > 1_000_000.0 {
            continue;
        }
        let velocity = match note.velocity {
            Some(velocity) if velocity.is_finite() => velocity.max(0.0).min(127.0) / 127.0,
            _ => 1.0,
        };
        let weight = note.duration * velocity;
        let pitch_class = note.pitch as usize % 12;
        weights[pitch_class] += weight;
        note_count += 1;
        total_duration_beats += note.duration;
        if first_note.as_ref().is_none_or(|first| note.start < first.edge || (note.start == first.edge && pitch_class < first.pitch_class))
        {
            first_note = Some(BoundaryNote { pitch_class, weight, edge: note.start });
        }
        let end = note.start + note.duration;
        if last_note.as_ref().is_none_or(|last| end > last.edge || (end == last.edge && pitch_class > last.pitch_class)) {
            last_note = Some(BoundaryNote { pitch_class, weight, edge: end });
        }
    }
    let pitch_class_count = weights.iter().filter(|weight| **weight > 0.0).count();

    let boundary_weight = first_note.as_ref().map_or(0.0, |first| first.weight)
        + if note_count == 1 { 0.0 } else { last_note.as_ref().map_or(0.0, |last| last.weight) };
    let tonal_evidence = |tonic: usize| -> f64 {
        let Some(first) = &first_note else { return 0.0 };
        if boundary_weight <= 0.0 {
            return 0.0;
        }
        let mut evidence = if first.pitch_class == tonic { first.weight } else { 0.0 };
        if note_count > 1 {
            if let Some(last) = &last_note {
                if last.pitch_class == tonic {
                    evidence += last.weight;
                }
            }
        }
        evidence / boundary_weight
    };

    let mut scored: Vec<KeyCandidate> = Vec::new();
    if pitch_class_count > 0 {
        for tonic in 0..12 {
            scored.push(KeyCandidate {
                key: format!("{} major", NOTE_NAMES[tonic]),
                tonic,
                mode: "major".to_string(),
                score: pearson(&weights, &profile_for(tonic, &MAJOR_PROFILE)),
                tonic_evidence: tonal_evidence(tonic),
            });
            scored.push(KeyCandidate {
                key: format!("{} minor", NOTE_NAMES[tonic]),
                tonic,
                mode: "minor".to_string(),
                score: pearson(&weights, &profile_for(tonic, &MINOR_PROFILE)),
                tonic_evidence: tonal_evidence(tonic),
            });
            for (mode, offset) in MODE_PARENT_OFFSETS {
                let parent = (tonic + offset) % 12;
                scored.push(KeyCandidate {
                    key: format!("{} {mode}", NOTE_NAMES[tonic]),
                    tonic,
                    mode: mode.to_string(),
                    score: pearson(&weights, &profile_for(parent, &MAJOR_PROFILE)),
                    tonic_evidence: tonal_evidence(tonic),
                });
            }
        }
        scored.sort_by(|a, b| {
            compare(b.score, a.score).then_with(|| compare(b.tonic_evidence, a.tonic_evidence)).then_with(|| a.key.cmp(&b.key))
        });
    }

    let candidates: Vec<KeyCandidate> = scored
        .iter()
        .take(MAX_CANDIDATES)
        .map(|candidate| KeyCandidate {
            score: six_places(candidate.score),
            tonic_evidence: six_places(candidate.tonic_evidence),
            ..candidate.clone()
        })
        .collect();
    let best = scored.first().map_or(0.0, |candidate| candidate.score);
    let second = scored.get(1).map_or(0.0, |candidate| candidate.score);
    let insufficient = note_count < 3 || pitch_class_count < 3 || pitch_class_count >= 10 || best < EVIDENCE_FLOOR;
    let ambiguous = !insufficient
        && best - second < AMBIGUITY_MARGIN
        && (scored.first().map_or(0.0, |candidate| candidate.tonic_evidence)
            - scored.get(1).map_or(0.0, |candidate| candidate.tonic_evidence))
        .abs()
            < TONIC_EVIDENCE_MARGIN;
    let confidence = if insufficient {
        KeyConfidence::InsufficientEvidence
    } else if ambiguous {
        if best >= HIGH_CONFIDENCE {
            KeyConfidence::Medium
        } else {
            KeyConfidence::Low
        }
    } else if best >= HIGH_CONFIDENCE {
        KeyConfidence::High
    } else if best >= MEDIUM_CONFIDENCE {
        KeyConfidence::Medium
    } else {
        KeyConfidence::Low
    };
    let alternatives = if ambiguous {
        candidates.iter().filter(|candidate| best - candidate.score < AMBIGUITY_MARGIN).cloned().collect()
    } else {
        Vec::new()
    };
    KeyEstimate {
        candidates,
        confidence,
        ambiguous,
        alternatives,
        evidence: KeyEstimateEvidence {
            note_count,
            pitch_class_count,
            chromatic: pitch_class_count >= 10,
            total_duration_beats: six_places(total_duration_beats),
            algorithm: ALGORITHM_ID.to_string(),
        },
    }
}
