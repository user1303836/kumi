//! Detectors: problems placed in time and frequency, each with the kind of fix it calls for. Harshness and
//! resonances (the short-term spectrum against a smoothed copy, split into steady and intermittent), the low end
//! (stereo lows, rumble, DC, one bass note louder than the rest), peaks over a ceiling, and masking between a target
//! element and the rest of the mix as a target-to-mask ratio.

use super::measure::{band_edges, fine_hz, percentile, Frames, Heard, FINE_BINS, THIRDS};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProblemKind {
    Harshness,
    Resonance,
    StereoLows,
    Rumble,
    DcOffset,
    LoudNote,
    Overs,
    Clipping,
    Masking,
}

/// A problem the ears found: what, where in frequency and time, how far over, and the kind of fix it calls for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub kind: ProblemKind,
    /// A short name that stays the same from round to round ("harshness 6.3 kHz"), for the checklist.
    pub id: String,
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hz: Option<[f64; 2]>,
    /// Where it happens, in seconds from the start of what was heard (the worst first); empty when it's throughout.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<[f64; 2]>,
    /// How far over: dB above the local level, the target or the others.
    pub excess: f64,
    /// Present most of the time (a static fix) or coming and going (a dynamic one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady: Option<bool>,
    pub fix: String,
    /// The track it comes from, once traced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// The ear's sensitivity, as A-weighting (dB at `hz`, 0 at 1 kHz).
pub fn a_weighting(hz: f64) -> f64 {
    let f2 = hz * hz;
    let numerator = 12194f64.powi(2) * f2 * f2;
    let denominator = (f2 + 20.6f64.powi(2)) * ((f2 + 107.7f64.powi(2)) * (f2 + 737.9f64.powi(2))).sqrt() * (f2 + 12194f64.powi(2));
    20. * (numerator / denominator).log10() + 2.0
}

/// Hz, the way a producer reads it.
pub fn hertz(hz: f64) -> String {
    if hz >= 1000. {
        format!("{} kHz", trim_number((hz / 100.).round() / 10.))
    } else {
        format!("{} Hz", (hz.round() as i64))
    }
}
fn trim_number(value: f64) -> String {
    let text = format!("{value:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}
/// Seconds as m:ss.
pub fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.).round() as i64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// Frames loud enough to judge: within 20 dB of the loud end.
fn active(frames: &Frames) -> Vec<usize> {
    let mut levels: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
    levels.sort_by(f64::total_cmp);
    if levels.is_empty() {
        return vec![];
    }
    let loud = percentile(&levels, 0.95);
    (0..frames.level.len()).filter(|frame| frames.level[*frame] as f64 >= loud - 20.).collect()
}

/// Consecutive frames (allowing a gap) as spans in seconds, worst first by their peak value.
fn spans(frames: &Frames, hits: &[(usize, f64)], gap: usize) -> Vec<([f64; 2], f64)> {
    let mut found: Vec<(usize, usize, f64)> = vec![];
    for &(frame, value) in hits {
        match found.last_mut() {
            Some(last) if frame <= last.1 + gap => {
                last.1 = frame;
                last.2 = last.2.max(value);
            }
            _ => found.push((frame, frame, value)),
        }
    }
    let mut spans: Vec<([f64; 2], f64)> =
        found.into_iter().map(|(from, to, value)| ([round1(frames.time(from)), round1(frames.time(to) + frames.hop)], value)).collect();
    spans.sort_by(|a, b| b.1.total_cmp(&a.1));
    spans
}

/// The level a fine bin stands out from: the power mean of its neighbours a quarter of an octave to an octave away on
/// both sides. The bin's own peak and skirt are left out, so a cut at the peak shows as the peak going down.
pub fn local_level(power: &[f64], bin: usize) -> f64 {
    let (mut sum, mut count) = (0., 0);
    for distance in 3..=12usize {
        if bin >= distance {
            sum += power[bin - distance];
            count += 1;
        }
        if bin + distance < power.len() {
            sum += power[bin + distance];
            count += 1;
        }
    }
    if count == 0 {
        return f64::NEG_INFINITY;
    }
    10. * (sum / count as f64).max(1e-30).log10()
}

/// Harshness (2–10 kHz) and resonances (250 Hz up): peaks of the short-term spectrum over their neighbours, weighted
/// by the ear's sensitivity; steady ones call for a static cut, ones that come and go for a dynamic EQ band or a
/// de-esser.
pub fn harshness(heard: &Heard) -> Vec<Problem> {
    let frames = &heard.frames;
    let active = active(frames);
    if active.len() < 8 {
        return vec![];
    }
    // Each active frame's peaks over the local level, by fine bin.
    let mut hits: Vec<Vec<(usize, f64)>> = vec![vec![]; FINE_BINS];
    for &frame in &active {
        let fine = &frames.fine[frame];
        let power: Vec<f64> = fine.iter().map(|db| 10f64.powf(*db as f64 / 10.)).collect();
        let loudest = fine.iter().copied().fold(f32::MIN, f32::max) as f64;
        for bin in 1..FINE_BINS - 1 {
            let excess = fine[bin] as f64 - local_level(&power, bin);
            let peak = fine[bin] >= fine[bin - 1] && fine[bin] >= fine[bin + 1];
            // A peak worth hearing: well over its neighborhood, and not far under the frame's loudest part.
            let weighted = excess + (a_weighting(fine_hz(bin)) - a_weighting(1000.)).min(0.) * 0.5;
            if peak && weighted >= 5. && fine[bin] as f64 >= loudest - 30. {
                hits[bin].push((frame, excess));
            }
        }
    }
    // A problem is a narrow band: the bins around one that stands out often, each standing out at least half as
    // often, a sixth of an octave each way at most. Music's passing peaks land on every bin now and then; they don't
    // join up into one wide "problem" no cut could fix.
    let share = |bin: usize| hits[bin].len() as f64 / active.len() as f64;
    let mut tops: Vec<usize> = (0..FINE_BINS).filter(|bin| share(*bin) >= 0.03 && hits[*bin].len() >= 3).collect();
    tops.sort_by(|a, b| share(*b).total_cmp(&share(*a)));
    let mut taken = [false; FINE_BINS];
    let mut problems = vec![];
    for top in tops {
        if taken[top] {
            continue;
        }
        let floor = share(top) * 0.5;
        let (mut first, mut last) = (top, top);
        while first > 0 && top - (first - 1) <= 2 && !taken[first - 1] && share(first - 1) >= floor {
            first -= 1;
        }
        while last + 1 < FINE_BINS && last + 1 - top <= 2 && !taken[last + 1] && share(last + 1) >= floor {
            last += 1;
        }
        taken[first..=last].iter_mut().for_each(|taken| *taken = true);
        let bin = last + 1;
        let region = first..bin;
        let mut frames_hit: Vec<(usize, f64)> = region.clone().flat_map(|b| hits[b].iter().copied()).collect();
        frames_hit.sort_by_key(|hit| hit.0);
        frames_hit.dedup_by(|a, b| {
            if a.0 == b.0 {
                b.1 = b.1.max(a.1);
                true
            } else {
                false
            }
        });
        let share = frames_hit.len() as f64 / active.len() as f64;
        // A band heard in under 3% of the loud frames is noise in the measure.
        if share < 0.03 || frames_hit.len() < 3 {
            continue;
        }
        let weight: f64 = region.clone().map(|b| hits[b].len() as f64).sum();
        let center = region.clone().map(|b| b as f64 * hits[b].len() as f64).sum::<f64>() / weight;
        let hz = fine_hz(0) * 2f64.powf(center / 12.);
        let (low, high) = (fine_hz(first) * 2f64.powf(-1. / 24.), fine_hz(bin - 1) * 2f64.powf(1. / 24.));
        let mut excesses: Vec<f64> = frames_hit.iter().map(|hit| hit.1).collect();
        excesses.sort_by(f64::total_cmp);
        let steady = share >= 0.5;
        let harsh = (2000. ..10_000.).contains(&hz);
        if !harsh && !steady {
            continue;
        }
        let excess = round1(if steady { percentile(&excesses, 0.5) } else { percentile(&excesses, 0.9) });
        let at: Vec<[f64; 2]> =
            if steady { vec![] } else { spans(frames, &frames_hit, 3).into_iter().take(5).map(|span| span.0).collect() };
        let where_ = if at.is_empty() {
            "throughout".into()
        } else {
            format!("at {}", at.iter().map(|span| clock(span[0])).collect::<Vec<_>>().join(", "))
        };
        let (kind, what, fix) = if harsh && !steady {
            let sibilant = (5000. ..9500.).contains(&hz);
            (
                ProblemKind::Harshness,
                format!("harsh hits {where_}, {}–{}, +{} dB over the local level", hertz(low), hertz(high), excess),
                format!(
                    "{} at {} (its gain reduction only when it flares)",
                    if sibilant { "a de-esser or a dynamic EQ band" } else { "a dynamic EQ band" },
                    hertz(hz)
                ),
            )
        } else {
            let q = peak_q(frames, &frames_hit, first, bin - 1);
            (
                if harsh { ProblemKind::Harshness } else { ProblemKind::Resonance },
                format!(
                    "a steady {} {where_} at {}, +{} dB over the local level",
                    if harsh { "harshness" } else { "resonance" },
                    hertz(hz),
                    excess
                ),
                format!("a static EQ cut at {}, Q about {}", hertz(hz), trim_number((q * 10.).round() / 10.)),
            )
        };
        problems.push(Problem {
            id: format!("{} {}", if harsh { "harshness" } else { "resonance" }, hertz(hz)),
            kind,
            what,
            hz: Some([round0(low), round0(high)]),
            at,
            excess,
            steady: Some(steady),
            fix,
            source: None,
        });
    }
    problems.sort_by(|a, b| b.excess.total_cmp(&a.excess));
    // The few that stand out most: a checklist of every small peak is a checklist nobody finishes.
    problems.truncate(4);
    problems
}

/// A steady peak's Q, from its width where it stands 3 dB less over its neighborhood than at its tip: the cut that
/// takes it down has to be about that narrow, or it lowers the neighborhood with it and the peak still stands out.
fn peak_q(frames: &Frames, hit: &[(usize, f64)], first: usize, last: usize) -> f64 {
    let from = first.saturating_sub(3);
    let to = (last + 4).min(FINE_BINS);
    let mut sums = vec![0f64; to - from];
    for &(frame, _) in hit {
        let fine = &frames.fine[frame];
        let power: Vec<f64> = fine.iter().map(|db| 10f64.powf(*db as f64 / 10.)).collect();
        for bin in from..to {
            sums[bin - from] += fine[bin] as f64 - local_level(&power, bin);
        }
    }
    let curve: Vec<f64> = sums.iter().map(|sum| sum / hit.len().max(1) as f64).collect();
    let Some((tip, &top)) = curve.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else { return 4. };
    let mut left = tip;
    while left > 0 && curve[left - 1] > top - 3. {
        left -= 1;
    }
    let mut right = tip;
    while right + 1 < curve.len() && curve[right + 1] > top - 3. {
        right += 1;
    }
    // Twelfths of an octave: the width between the bins' outer edges.
    let octaves = (right - left + 1) as f64 / 12.;
    let ratio = 2f64.powf(octaves);
    (ratio.sqrt() / (ratio - 1.)).clamp(2., 12.)
}

/// The low end: stereo below 120 Hz, rumble under 30 Hz, a DC offset, one bass note louder than the rest.
pub fn low_end(heard: &Heard) -> Vec<Problem> {
    let measures = &heard.measures;
    let mut problems = vec![];
    if let Some(width) = measures.low_width.filter(|width| *width > -15.) {
        problems.push(Problem {
            kind: ProblemKind::StereoLows,
            id: "stereo lows".into(),
            what: format!("stereo below 120 Hz: the side is {} dB under the mid there", -width),
            hz: Some([20., 120.]),
            at: vec![],
            excess: round1(width + 20.),
            steady: Some(true),
            fix: "mono below about 120 Hz (Utility's Bass Mono)".into(),
            source: None,
        });
    }
    if let Some(rumble) = measures.rumble.filter(|rumble| *rumble > -10.) {
        problems.push(Problem {
            kind: ProblemKind::Rumble,
            id: "rumble".into(),
            what: format!("sub rumble: under 30 Hz is only {} dB below 30–120 Hz", -rumble),
            hz: Some([5., 30.]),
            at: vec![],
            excess: round1(rumble + 15.),
            steady: Some(true),
            fix: "a high pass at about 25–30 Hz".into(),
            source: None,
        });
    }
    let offset = measures.dc[0].abs().max(measures.dc[1].abs());
    if offset > 0.001 {
        problems.push(Problem {
            kind: ProblemKind::DcOffset,
            id: "dc offset".into(),
            what: format!("a DC offset of {} dBFS", round1(20. * offset.log10())),
            hz: Some([0., 5.]),
            at: vec![],
            excess: round1(20. * offset.log10() + 60.),
            steady: Some(true),
            fix: "a high pass (any, even at 5 Hz) on the track that carries it".into(),
            source: None,
        });
    }
    if let Some(problem) = loud_note(&heard.frames) {
        problems.push(problem);
    }
    problems
}

/// One bass note louder than the rest: each note's median level where it's the strongest in 30–250 Hz.
fn loud_note(frames: &Frames) -> Option<Problem> {
    let notes: Vec<(i64, f64)> =
        frames.bass.iter().flatten().map(|(hz, level)| ((69. + 12. * (*hz as f64 / 440.).log2()).round() as i64, *level as f64)).collect();
    if notes.len() < 20 {
        return None;
    }
    let loudest = notes.iter().map(|note| note.1).fold(f64::MIN, f64::max);
    let mut by_note: std::collections::BTreeMap<i64, Vec<f64>> = std::collections::BTreeMap::new();
    for (note, level) in notes.iter().filter(|note| note.1 >= loudest - 18.) {
        by_note.entry(*note).or_default().push(*level);
    }
    let counted: usize = by_note.values().map(Vec::len).sum();
    let medians: Vec<(i64, f64, usize)> = by_note
        .into_iter()
        .filter(|(_, levels)| levels.len() * 25 >= counted)
        .map(|(note, mut levels)| {
            levels.sort_by(f64::total_cmp);
            (note, percentile(&levels, 0.5), levels.len())
        })
        .collect();
    if medians.len() < 3 {
        return None;
    }
    let mut levels: Vec<f64> = medians.iter().map(|note| note.1).collect();
    levels.sort_by(f64::total_cmp);
    let typical = percentile(&levels, 0.5);
    let (note, level, _) = medians.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let excess = level - typical;
    (excess >= 4.).then(|| {
        let hz = 440. * 2f64.powf((note - 69) as f64 / 12.);
        Problem {
            kind: ProblemKind::LoudNote,
            id: format!("loud bass note {}", crate::notation::pitch_name(note.clamp(0, 127) as u8)),
            what: format!(
                "the bass's {} ({}) is {} dB louder than its other notes",
                crate::notation::pitch_name(note.clamp(0, 127) as u8),
                hertz(hz),
                round1(excess)
            ),
            hz: Some([round0(hz * 2f64.powf(-1. / 24.)), round0(hz * 2f64.powf(1. / 24.))]),
            at: vec![],
            excess: round1(excess),
            steady: Some(false),
            fix: format!("a narrow dynamic EQ cut at {} on the bass (or a static one, about {} dB)", hertz(hz), round1(excess - 1.)),
            source: None,
        }
    })
}

/// Peaks over a ceiling, and clipping.
pub fn peaks(heard: &Heard, ceiling: Option<f64>) -> Vec<Problem> {
    let measures = &heard.measures;
    let mut problems = vec![];
    if let Some(ceiling) = ceiling.filter(|ceiling| measures.true_peak > *ceiling) {
        problems.push(Problem {
            kind: ProblemKind::Overs,
            id: "true peak".into(),
            what: format!(
                "true peaks reach {} dBTP{}, over the {} dBTP ceiling",
                measures.true_peak,
                measures.peak_at.map(|at| format!(" (the highest at {})", clock(at))).unwrap_or_default(),
                ceiling
            ),
            hz: None,
            at: measures.peak_at.map(|at| vec![[at, at + 0.1]]).unwrap_or_default(),
            excess: round1(measures.true_peak - ceiling),
            steady: None,
            fix: format!("a true-peak limiter at the end with its ceiling at {ceiling} dBTP"),
            source: None,
        });
    }
    if measures.clipped > 0 {
        problems.push(Problem {
            kind: ProblemKind::Clipping,
            id: "clipping".into(),
            what: format!(
                "{} samples at full scale{}",
                measures.clipped,
                if measures.clipped_at.is_empty() {
                    String::new()
                } else {
                    format!(", at {}", measures.clipped_at.iter().map(|span| clock(span[0])).collect::<Vec<_>>().join(", "))
                }
            ),
            hz: None,
            at: measures.clipped_at.clone(),
            excess: round1((measures.clipped as f64).log10() * 10.),
            steady: None,
            fix: "lower the level into whatever clips (often the master, or a track driven into it)".into(),
            source: None,
        });
    }
    problems
}

/// Bark (Traunmüller): where a frequency sits on the ear's critical-band scale.
fn bark(hz: f64) -> f64 {
    13. * (0.00076 * hz).atan() + 3.5 * (hz / 7500.).powi(2).atan()
}
/// How far a masker in one band reaches into another (Schroeder's spreading function), dB.
fn spread(maskee: f64, masker: f64) -> f64 {
    let dz = bark(maskee) - bark(masker);
    15.81 + 7.5 * (dz + 0.474) - 17.5 * (1. + (dz + 0.474).powi(2)).sqrt()
}
/// The masking threshold sits this far under the spread masker energy (music masks like noise more than like tones).
const MASK_OFFSET: f64 = 6.;

/// Masking as a target-to-mask ratio: the target element's band energy against the masking threshold the rest of the
/// mix builds (the mix's bands less the target's, spread over the critical bands), frame by frame. The bands that
/// carry the target are weighed as the ear hears them (a vocal's presence region over its low fundamentals, which the
/// rest of a mix always covers); where most of that falls under the threshold, the target is buried. The problem's
/// amount is how much of the time the target plays it's buried there, in percent.
pub fn masking(target: &Heard, mix: &Heard, name: &str) -> Option<Problem> {
    let Buried { share, frames: buried, bands: masked_bands, deficits } = buried(target, mix)?;
    if share < 0.1 {
        return None;
    }
    let t = &target.frames;
    let mut deficits = deficits;
    deficits.sort_by(f64::total_cmp);
    let deficit = round1(percentile(&deficits, 0.5));
    // The bands most often masked, as one range.
    let most = masked_bands.iter().copied().max().unwrap_or(0);
    let bands: Vec<usize> = (0..31).filter(|band| masked_bands[*band] * 2 >= most && most > 0).collect();
    let (low, high) = (band_edges(*bands.first()?).0, band_edges(*bands.last()?).1);
    // Moments under half a second apart are one.
    let at: Vec<[f64; 2]> = spans(t, &buried, (0.5 / t.hop) as usize).into_iter().take(5).map(|span| span.0).collect();
    Some(Problem {
        kind: ProblemKind::Masking,
        id: format!("masking {name}"),
        what: format!(
            "{name} is buried under the rest {} of the time it plays, {}–{} (target-to-mask −{deficit} dB){}",
            percent(share),
            hertz(low),
            hertz(high),
            if at.is_empty() {
                String::new()
            } else {
                format!(", worst at {}", at.iter().map(|span| clock(span[0])).collect::<Vec<_>>().join(", "))
            }
        ),
        hz: Some([round0(low), round0(high)]),
        at,
        excess: (share * 100.).round(),
        steady: Some(share >= 0.5),
        fix: format!(
            "a dip of the others at {}–{} only while {name} plays (a dynamic EQ or multiband keyed from {name}), or {name} up there",
            hertz(low),
            hertz(high)
        ),
        source: None,
    })
}

/// How much of the time a target element plays it's buried under the rest of the mix, in percent: None when it hardly
/// plays in what was heard (nothing to tell, which isn't the same as not buried).
pub fn masking_share(target: &Heard, mix: &Heard) -> Option<f64> {
    buried(target, mix).map(|buried| (buried.share * 100.).round())
}

/// Where a target plays, how it fares against the rest: the share of those frames it's buried in, those frames (and how
/// far under, dB), and how often each band was masked.
struct Buried {
    share: f64,
    frames: Vec<(usize, f64)>,
    bands: [usize; 31],
    deficits: Vec<f64>,
}

/// A band level this low (dB, of full scale's power) is silence.
const SILENT_DB: f64 = -80.;

fn buried(target: &Heard, mix: &Heard) -> Option<Buried> {
    let (t, m) = (&target.frames, &mix.frames);
    let count = t.mid.len().min(m.mid.len());
    if count < 8 {
        return None;
    }
    let spreads: Vec<Vec<f64>> = (0..31)
        .map(|maskee| (0..31).map(|masker| 10f64.powf((spread(THIRDS[maskee], THIRDS[masker]) - MASK_OFFSET) / 10.)).collect())
        .collect();
    let hearing: Vec<f64> = THIRDS.iter().map(|hz| 10f64.powf(a_weighting(*hz) / 10.)).collect();
    let target_levels: Vec<f64> = (0..count)
        .map(|frame| 10. * (t.mid[frame].iter().zip(&t.side[frame]).map(|(a, b)| (*a + *b) as f64).sum::<f64>() + 1e-20).log10())
        .collect();
    let mut sorted = target_levels.clone();
    sorted.sort_by(f64::total_cmp);
    let loud = percentile(&sorted, 0.95);
    let mut buried: Vec<(usize, f64)> = vec![];
    let mut masked_bands = [0usize; 31];
    let mut deficits = vec![];
    let mut heard_frames = 0;
    for frame in 0..count {
        // Only where the target plays (and not silence: then there's nothing to bury).
        if target_levels[frame] < (loud - 20.).max(SILENT_DB) {
            continue;
        }
        heard_frames += 1;
        let power = |frames: &Frames, band: usize| (frames.mid[frame][band] + frames.side[frame][band]) as f64;
        let target_bands: Vec<f64> = (0..31).map(|band| power(t, band)).collect();
        let others: Vec<f64> = (0..31).map(|band| (power(m, band) - target_bands[band]).max(0.)).collect();
        let weighed: Vec<f64> = (0..31).map(|band| target_bands[band] * hearing[band]).collect();
        let strongest = weighed.iter().copied().fold(0., f64::max);
        let (mut weight, mut under, mut worst) = (0., 0., 0f64);
        for band in 0..31 {
            // The target's bands that carry it, as the ear weighs them: within 15 dB of its strongest.
            if weighed[band] < strongest * 10f64.powf(-1.5) {
                continue;
            }
            let threshold: f64 = (0..31).map(|masker| others[masker] * spreads[band][masker]).sum();
            let ratio = 10. * ((target_bands[band] + 1e-20) / (threshold + 1e-20)).log10();
            weight += weighed[band];
            if ratio < 0. {
                under += weighed[band];
                masked_bands[band] += 1;
                worst = worst.min(ratio);
            }
        }
        if weight > 0. && under / weight >= 0.5 {
            buried.push((frame, -worst));
            deficits.push(-worst);
        }
    }
    if heard_frames < 8 {
        return None;
    }
    Some(Buried { share: buried.len() as f64 / heard_frames as f64, frames: buried, bands: masked_bands, deficits })
}

fn percent(share: f64) -> String {
    format!("{}%", (share * 100.).round() as i64)
}
fn round0(value: f64) -> f64 {
    value.round()
}
fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
