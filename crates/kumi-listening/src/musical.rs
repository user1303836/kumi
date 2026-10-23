//! What a listen says of the music in it, beyond the mix: its tempo, its key, how busy it is (onsets a bar) and how it
//! grooves (its swing, and how far it sits ahead of or behind the beat). Read from what a listen keeps anyway (the level
//! every 5 ms, the semitone spectrum, the bass's pitch), so a reference or a song is heard once for all of it.

use crate::{
    measure::{Frames, Heard, ENVELOPE_HOP},
    notes::{fit_grid, fit_swing, push_of},
};
use kumi_media::audio::analyze::{estimate_key, estimate_tempo};

/// The music in a listen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Musical {
    /// Beats a minute.
    pub tempo: Option<f64>,
    /// "A minor", "F# major".
    pub key: Option<String>,
    /// Onsets in a bar of four beats.
    pub busyness: Option<f64>,
    pub groove: Option<Feel>,
}

/// How it grooves: the swing as the ratio of a pair of steps (1 straight, 2 a triplet's lilt), and how far it sits
/// ahead (−) of or behind (+) the beat, ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feel {
    pub swing_ratio: f64,
    pub push_ms: f64,
}

/// Onsets a groove needs, at least.
const FEW: usize = 16;
/// A rise in level (dB over 15 ms) that's an onset, at least: and above most of the listen's own rises.
const RISE: f64 = 6.;
/// Two onsets closer than this are one.
const APART: f64 = 0.03;

/// The listen's level in dB, every `ENVELOPE_HOP`.
fn levels(frames: &Frames) -> Vec<f64> {
    frames.envelope.iter().map(|value| 20. * (*value as f64 + 1e-9).log10()).collect()
}

/// Where hits start, seconds, each with how much the level rose: the rises of the level over 15 ms, at least 30 ms
/// apart (of two closer, the bigger).
pub fn onsets(frames: &Frames) -> Vec<(f64, f64)> {
    let level = levels(frames);
    if level.len() < 8 {
        return vec![];
    }
    let rise: Vec<f64> = (0..level.len()).map(|at| if at < 3 { 0. } else { (level[at] - level[at - 3]).max(0.) }).collect();
    let mut sorted = rise.clone();
    sorted.sort_by(f64::total_cmp);
    let threshold = sorted[sorted.len() * 9 / 10].max(RISE);
    let gap = (APART / ENVELOPE_HOP).round() as usize;
    let mut found: Vec<(usize, f64)> = vec![];
    for at in 1..rise.len() {
        let value = rise[at];
        if value < threshold || rise[at - 1] >= value || rise.get(at + 1).is_some_and(|next| *next > value) {
            continue;
        }
        match found.last_mut() {
            Some((last, kept)) if at - *last < gap => {
                if value > *kept {
                    (*last, *kept) = (at, value);
                }
            }
            _ => found.push((at, value)),
        }
    }
    // A rise over three steps: its hit is about halfway up.
    found.into_iter().map(|(at, value)| (((at as f64) - 1.5).max(0.) * ENVELOPE_HOP, value)).collect()
}

/// Its tempo: the level's rises, autocorrelated (as a mix's analysis reads tempo).
pub fn tempo(frames: &Frames) -> Option<f64> {
    let level = levels(frames);
    let strength: Vec<f64> = (0..level.len()).map(|at| if at == 0 { 0. } else { (level[at] - level[at - 1]).max(0.) }).collect();
    estimate_tempo(&strength, 1. / ENVELOPE_HOP).filter(|tempo| tempo.confidence > 0.).map(|tempo| tempo.bpm)
}

/// Its key: the loud frames' pitch classes (the semitone spectrum from 250 Hz, its first bin B3, and the bass's pitch),
/// against the major and minor key profiles.
pub fn key(frames: &Frames) -> Option<String> {
    let mut levels: Vec<f32> = frames.level.clone();
    levels.sort_by(f32::total_cmp);
    let loud = levels.get(levels.len() * 95 / 100).copied().unwrap_or(f32::MIN) - 20.;
    let mut fine = [0f64; 12];
    for (frame, row) in frames.fine.iter().enumerate() {
        if frames.level.get(frame).is_none_or(|level| *level < loud) {
            continue;
        }
        for (bin, db) in row.iter().enumerate() {
            fine[(11 + bin) % 12] += 10f64.powf(*db as f64 / 10.);
        }
    }
    let mut bass = [0f64; 12];
    for (hz, db) in frames.bass.iter().flatten() {
        let class = (12. * (*hz as f64 / 261.6256).log2()).round().rem_euclid(12.) as usize % 12;
        bass[class] += 10f64.powf(*db as f64 / 10.);
    }
    let unit = |chroma: [f64; 12]| -> [f64; 12] {
        let total: f64 = chroma.iter().sum();
        chroma.map(|value| if total > 0. { value / total } else { 0. })
    };
    let (fine, bass) = (unit(fine), unit(bass));
    let chroma: Vec<f64> = (0..12).map(|class| fine[class] + 0.5 * bass[class]).collect();
    estimate_key(&chroma).map(|key| key.name)
}

/// How a run of onsets (seconds, each with how hard it hit) grooves at about `tempo`: a grid fitted to them, the
/// harder hits counting for more (a kick on the beat places it, not the hats between), then their swing and push on it
/// (as the groove judge reads a part's).
pub fn groove(onsets: &[(f64, f64)], tempo: f64) -> Option<Feel> {
    if onsets.len() < FEW || !(tempo > 0.) {
        return None;
    }
    let grid = fit_grid(onsets, tempo, 4., false, None);
    let beats: Vec<f64> = onsets.iter().map(|(at, _)| (at - grid.downbeat) * grid.tempo / 60.).collect();
    let (_, swing) = fit_swing(&beats);
    let push = push_of(&beats) * 60_000. / grid.tempo;
    Some(Feel { swing_ratio: swing / (100. - swing), push_ms: push })
}

/// The music in a listen.
pub fn of(heard: &Heard) -> Musical {
    let frames = &heard.frames;
    let tempo = tempo(frames);
    let found = onsets(frames);
    let seconds = heard.measures.seconds;
    let busyness = tempo.filter(|_| seconds > 0.).map(|tempo| found.len() as f64 / (seconds * tempo / 240.));
    Musical { tempo, key: key(frames), busyness, groove: tempo.and_then(|tempo| groove(&found, tempo)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixteenths at `tempo` for `bars` bars, each off-sixteenth late by `swing` (50 straight), all `push` ms late: a
    /// hard hit on each beat, softer ones between.
    fn sixteenths(tempo: f64, bars: usize, swing: f64, push: f64) -> Vec<(f64, f64)> {
        let beat = 60. / tempo;
        (0..bars * 16)
            .map(|step| {
                let pair = (step / 2) as f64 * beat / 2.;
                let at = if step % 2 == 1 { pair + beat / 2. * swing / 100. } else { pair };
                (0.5 + at + push / 1000., if step % 4 == 0 { 30. } else { 10. })
            })
            .collect()
    }

    #[test]
    fn a_straight_grid_has_no_swing_and_sits_on_the_beat() {
        let feel = groove(&sixteenths(120., 8, 50., 0.), 120.).unwrap();
        assert!((feel.swing_ratio - 1.).abs() < 0.05, "{feel:?}");
        assert!(feel.push_ms.abs() < 5., "{feel:?}");
    }

    #[test]
    fn swung_sixteenths_read_as_their_swing() {
        // 60 % swing: a pair's first step half again as long as its second.
        let feel = groove(&sixteenths(100., 8, 60., 0.), 100.).unwrap();
        assert!((feel.swing_ratio - 1.5).abs() < 0.1, "{feel:?}");
        let triplets = groove(&sixteenths(100., 8, 66.7, 0.), 100.).unwrap();
        assert!((triplets.swing_ratio - 2.).abs() < 0.15, "{triplets:?}");
    }

    #[test]
    fn too_few_onsets_have_no_groove() {
        assert_eq!(groove(&sixteenths(120., 1, 50., 0.)[..8], 120.), None);
        assert_eq!(groove(&sixteenths(120., 4, 50., 0.), 0.), None);
    }
}
