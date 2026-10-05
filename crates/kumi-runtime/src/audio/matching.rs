//! How close a render is to a reference, as one number to watch rise while matching a sound, and the
//! differences behind it, biggest first, in words the model can act on ("brighter above 4 kHz by
//! ~3 dB", "attack too slow", "too dense"). Each feature is a similarity from 0 to 1; the score is
//! their weighted mean, weighted for a single sound (timbre, envelope, pitch) or a section (balance,
//! density and rhythm too). The target is a similar character: a patch rarely matches a finished mix.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureName {
    Balance,
    Tilt,
    Brightness,
    Movement,
    Envelope,
    Pitch,
    Density,
    Width,
    Rhythm,
    Contour,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub name: FeatureName,
    /// 0 to 100.
    pub similarity: f64,
    /// What to change, when it's far enough off to matter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
    pub weight: f64,
}

/// What a comparison weighs for: a single sound, or a section of a mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Focus {
    Sound,
    Section,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructuralKind {
    MissingLow,
    ExcessLow,
    MissingTop,
    ExcessTop,
    MissingMids,
    ExcessMids,
    Envelope,
    Register,
    Pitched,
    Width,
    Density,
}

/// A gap no knob closes (a band 9 dB or more off, an attack three times off, the wrong register, far
/// too wide or narrow), when it's the largest part of what's lost: what it is, and the structural
/// change that closes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Structural {
    pub kind: StructuralKind,
    pub gap: String,
    pub r#move: String,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Closeness {
    /// 0 to 100: the weighted mean of the features both sides have.
    pub score: f64,
    pub focus: Focus,
    pub features: Vec<Feature>,
    /// The biggest gaps, in words, biggest first.
    pub gaps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural: Option<Structural>,
}

use super::analyze::{Analysis, Timeline};
use kumi_common::js::number::{round, to_fixed, to_string};
pub const STRUCTURE_MOVES:&[(StructuralKind,&str)]=&[
(StructuralKind::MissingLow,"add a sub layer (an Operator sine or a Drift one octave down, in a rack chain), an EQ Eight low shelf, or change the base instrument"),
(StructuralKind::ExcessLow,"high-pass it (EQ Eight), take the low layer out, or change the base"),
(StructuralKind::MissingTop,"add saturation (Saturator, Roar), an exciter or a brighter base (a saw or noise layer)"),
(StructuralKind::ExcessTop,"low-pass it (Auto Filter), use a darker base (sine, triangle), or take a layer out"),
(StructuralKind::MissingMids,"add a layer for the body (a second oscillator or a rack chain), or change the base"),
(StructuralKind::ExcessMids,"cut the mids (EQ Eight), change the base, or thin the layers"),
(StructuralKind::Envelope,"change the instrument family (a plucked or struck model against a sustained one), or add a transient shaper (Drum Buss, Compressor)"),
(StructuralKind::Register,"transpose it (the MIDI, or the instrument's octave), or change the base"),
(StructuralKind::Pitched,"change the kind of source: a tonal oscillator against noise or a sample"),
(StructuralKind::Width,"widen or narrow it: Utility width, Chorus-Ensemble, or parallel chains panned apart"),
(StructuralKind::Density,"change the MIDI: more or fewer notes, another rhythm")];
pub fn structure_move(kind: StructuralKind) -> &'static str {
    STRUCTURE_MOVES.iter().find(|(k, _)| *k == kind).unwrap().1
}
fn near(distance: f64, scale: f64) -> f64 {
    (-distance.abs() / scale).exp()
}
fn ratio(a: f64, b: f64) -> f64 {
    (a.max(1e-6) / b.max(1e-6)).log2()
}
fn std(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}
fn signed(value: f64) -> String {
    format!("{}{}", if value > 0.0 { "+" } else { "−" }, to_fixed(value.abs(), 1))
}
fn weight(focus: Focus, name: FeatureName) -> f64 {
    let index = match name {
        FeatureName::Balance => 0,
        FeatureName::Tilt => 1,
        FeatureName::Brightness => 2,
        FeatureName::Movement => 3,
        FeatureName::Envelope => 4,
        FeatureName::Pitch => 5,
        FeatureName::Density => 6,
        FeatureName::Width => 7,
        FeatureName::Rhythm => 8,
        FeatureName::Contour => 9,
    };
    match focus {
        Focus::Sound => [0.22, 0.1, 0.14, 0.06, 0.24, 0.16, 0.03, 0.05, 0.0, 0.04][index],
        Focus::Section => [0.2, 0.08, 0.08, 0.04, 0.08, 0.06, 0.1, 0.08, 0.18, 0.1][index],
    }
}
fn grid(timeline: &Timeline, values: &[f64]) -> Vec<f64> {
    let mut out = Vec::new();
    let per = 0.02 / timeline.step;
    let mut at = 0;
    while at as f64 * per < values.len() as f64 {
        let from = (at as f64 * per).floor() as usize;
        let to = (from + 1).max(((at + 1) as f64 * per).floor() as usize).min(values.len());
        out.push(values[from..to].iter().copied().fold(f64::NEG_INFINITY, f64::max));
        at += 1;
    }
    out
}
fn correlate(a: &[f64], b: &[f64], lag: i64, length: usize) -> f64 {
    let ma = a.iter().sum::<f64>() / a.len() as f64;
    let mb = b.iter().sum::<f64>() / b.len() as f64;
    let (mut sum, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for i in (-lag).max(0) as usize..length {
        let j = i as i64 + lag;
        if j >= length as i64 {
            break;
        }
        let x = a[i] - ma;
        let y = b[j as usize] - mb;
        sum += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa > 0.0 && bb > 0.0 {
        sum / (aa * bb).sqrt()
    } else {
        0.0
    }
}
fn trend(values: &[f64]) -> f64 {
    let half = values.len() / 2;
    let mean = |p: &[f64]| p.iter().sum::<f64>() / p.len().max(1) as f64;
    mean(&values[half..]) - mean(&values[..half])
}
/// Mine against the reference, weighted for either a single sound or a section.
pub fn closeness(mine: &Analysis, reference: &Analysis, focus: Option<Focus>) -> Closeness {
    use FeatureName::*;
    let kind =
        focus.unwrap_or(if mine.analyzed.focus == "sound" && reference.analyzed.focus == "sound" { Focus::Sound } else { Focus::Section });
    let mut features = Vec::new();
    let mut push = |name, similarity, gap| features.push(Feature { name, similarity, weight: weight(kind, name), gap });
    let bands: Vec<_> = mine.balance.bands.iter().zip(&reference.balance.bands).filter(|(b, r)| b.db.max(r.db) > -45.0).collect();
    if !bands.is_empty() {
        let mean = bands.iter().map(|(b, r)| (b.db - r.db).abs().min(15.0)).sum::<f64>() / bands.len() as f64;
        let mut worst = &bands[0];
        for band in &bands[1..] {
            if (band.0.db - band.1.db).abs() > (worst.0.db - worst.1.db).abs() {
                worst = band;
            }
        }
        let difference = worst.0.db - worst.1.db;
        push(
            Balance,
            near(mean, 5.0),
            if difference.abs() >= 2.0 {
                Some(format!("{} ({} Hz) {} dB against the reference", worst.0.name, worst.0.hz, signed(difference)))
            } else {
                None
            },
        );
    }
    let tilt = mine.balance.tilt_db_per_octave - reference.balance.tilt_db_per_octave;
    push(
        Tilt,
        near(tilt, 1.5),
        if tilt.abs() >= 0.7 {
            Some(format!("{} overall ({} dB/octave)", if tilt > 0.0 { "brighter" } else { "darker" }, signed(tilt)))
        } else {
            None
        },
    );
    let bright = ratio(mine.balance.centroid_hz, reference.balance.centroid_hz);
    push(
        Brightness,
        near(bright, 0.6),
        if bright.abs() >= 0.25 {
            Some(format!(
                "centre of brightness {} Hz against {} Hz: {}",
                to_string(round(mine.balance.centroid_hz)),
                to_string(round(reference.balance.centroid_hz)),
                if bright > 0.0 { "darken it" } else { "brighten it" }
            ))
        } else {
            None
        },
    );
    let moving = |a: &Analysis| std(&a.over_time.lufs.iter().flatten().copied().collect::<Vec<_>>());
    let movement = moving(mine) - moving(reference);
    push(
        Movement,
        near(movement, 3.0),
        if movement.abs() >= 2.0 {
            Some(format!("{} over time than the reference", if movement > 0.0 { "moves more" } else { "steadier" }))
        } else {
            None
        },
    );
    match (&mine.sound, &reference.sound) {
        (Some(m), Some(r)) if kind == Focus::Sound => {
            let attack = ratio(m.envelope.attack_ms + 1.0, r.envelope.attack_ms + 1.0);
            let length = ratio(m.envelope.length_ms + 1.0, r.envelope.length_ms + 1.0);
            push(
                Envelope,
                near(attack, 1.5) * 0.6 + near(length, 1.0) * 0.4,
                if attack.abs() >= 1.0 {
                    Some(format!(
                        "attack {} ({} ms against {} ms)",
                        if attack > 0.0 { "too slow" } else { "too fast" },
                        to_string(m.envelope.attack_ms),
                        to_string(r.envelope.attack_ms)
                    ))
                } else if length.abs() >= 0.8 {
                    Some(format!(
                        "{} ({} ms against {} ms)",
                        if length > 0.0 { "too long" } else { "too short" },
                        to_string(m.envelope.length_ms),
                        to_string(r.envelope.length_ms)
                    ))
                } else {
                    None
                },
            );
        }
        _ => {
            let crest = mine.dynamics.crest_db - reference.dynamics.crest_db;
            push(
                Envelope,
                near(crest, 4.0),
                if crest.abs() >= 2.5 {
                    Some(
                        if crest > 0.0 {
                            "punchier, with sharper peaks, than the reference"
                        } else {
                            "flatter, more compressed, than the reference"
                        }
                        .into(),
                    )
                } else {
                    None
                },
            );
        }
    }
    let mine_pitch = mine.sound.as_ref().and_then(|s| s.pitch.as_ref());
    let ref_pitch = reference.sound.as_ref().and_then(|s| s.pitch.as_ref());
    if let (Some(m), Some(r)) = (mine_pitch, ref_pitch) {
        let semitones = 12.0 * (m.hz / r.hz).log2();
        push(
            Pitch,
            near(semitones, 2.0),
            if semitones.abs() >= 0.5 {
                Some(format!("pitch {} against {} ({} semitones)", m.note, r.note, signed(semitones)))
            } else {
                None
            },
        );
    } else if kind == Focus::Sound && mine_pitch.is_some() != ref_pitch.is_some() {
        push(
            Pitch,
            0.15,
            Some(if let Some(r) = ref_pitch {
                format!("the reference is pitched ({}); this has no clear note", r.note)
            } else {
                "the reference has no clear note; this is pitched".into()
            }),
        );
    } else if let (Some(m), Some(r)) = (&mine.key, &reference.key) {
        push(
            Pitch,
            if m.name == r.name { 1.0 } else { 0.5 },
            if m.name != r.name { Some(format!("key {} against {}", m.name, r.name)) } else { None },
        );
    }
    let onsets = [mine.dynamics.onsets_per_second, reference.dynamics.onsets_per_second];
    if onsets[0] > 0.05 || onsets[1] > 0.05 {
        let dense = ratio(onsets[0] + 0.1, onsets[1] + 0.1);
        push(
            Density,
            near(dense, 0.8),
            if dense.abs() >= 0.5 {
                Some(format!(
                    "{} ({} against {} onsets a second)",
                    if dense > 0.0 { "too dense" } else { "too sparse" },
                    to_string(onsets[0]),
                    to_string(onsets[1])
                ))
            } else {
                None
            },
        );
    }
    if let (Some(m), Some(r)) = (&mine.stereo, &reference.stereo) {
        let width = m.width - r.width;
        push(
            Width,
            near(width, 0.25),
            if width.abs() >= 0.15 { Some(format!("{} than the reference", if width > 0.0 { "wider" } else { "narrower" })) } else { None },
        );
    }
    if let (Some(m), Some(r)) = (&mine.timeline, &reference.timeline) {
        let om = grid(m, &m.onset);
        let or = grid(r, &r.onset);
        let length = om.len().min(or.len());
        if length >= 50 {
            let smear = |values: &[f64]| {
                (0..values.len())
                    .map(|i| values[i.saturating_sub(2)..(i + 3).min(values.len())].iter().copied().fold(f64::NEG_INFINITY, f64::max))
                    .collect::<Vec<_>>()
            };
            let a = smear(&om[..length]);
            let b = smear(&or[..length]);
            let hits = |values: &[f64]| {
                values
                    .iter()
                    .enumerate()
                    .filter(|(i, v)| {
                        **v >= 0.5 && **v >= i.checked_sub(1).map_or(0.0, |p| values[p]) && **v >= values.get(i + 1).copied().unwrap_or(0.0)
                    })
                    .count()
            };
            let hm = hits(&om[..length]);
            let hr = hits(&or[..length]);
            if hm >= 2 || hr >= 2 {
                let mut rhythm = -1f64;
                if hm >= 2 && hr >= 2 {
                    for lag in -5..=5 {
                        rhythm = rhythm.max(correlate(&a, &b, lag, length));
                    }
                }
                push(
                    Rhythm,
                    rhythm.max(0.0),
                    if rhythm < 0.4 {
                        Some(
                            "the rhythm doesn't line up with the reference's: transcribe its notes (listen with transcribe) and play those"
                                .into(),
                        )
                    } else {
                        None
                    },
                );
            }
            let lm = grid(m, &m.level);
            let lr = grid(r, &r.level);
            let smooth = |values: &[f64]| {
                (0..values.len())
                    .map(|i| {
                        let p = &values[i.saturating_sub(6)..(i + 7).min(values.len())];
                        10.0 * (p.iter().map(|v| 10f64.powf(v / 10.0)).sum::<f64>() / p.len() as f64).max(1e-9).log10()
                    })
                    .collect::<Vec<_>>()
            };
            let sm = smooth(&lm[..length]);
            let sr = smooth(&lr[..length]);
            let contour = correlate(&sm, &sr, 0, length);
            let builds = trend(&sr) - trend(&sm);
            push(
                Contour,
                (contour + 1.0) / 2.0,
                if builds.abs() >= 4.0 {
                    Some(if builds > 0.0 {
                        format!(
                            "the reference builds up over time ({} dB from its first half to its second); this doesn't as much",
                            signed(trend(&sr))
                        )
                    } else {
                        "this builds up more over time than the reference".into()
                    })
                } else {
                    None
                },
            );
        }
    }
    let mut found: Vec<(StructuralKind, Option<FeatureName>, String)> = Vec::new();
    for (index, (band, other)) in mine.balance.bands.iter().zip(&reference.balance.bands).enumerate() {
        if band.db.max(other.db) <= -45.0 {
            continue;
        }
        let difference = band.db - other.db;
        if difference.abs() < 9.0 {
            continue;
        }
        let missing = difference < 0.0;
        let sk = if index <= 2 {
            if missing {
                StructuralKind::MissingLow
            } else {
                StructuralKind::ExcessLow
            }
        } else if index >= 6 {
            if missing {
                StructuralKind::MissingTop
            } else {
                StructuralKind::ExcessTop
            }
        } else if missing {
            StructuralKind::MissingMids
        } else {
            StructuralKind::ExcessMids
        };
        found.push((sk, None, format!("{} {} dB against the reference", band.name, signed(difference))));
    }
    if let (Some(m), Some(r)) = (&mine.sound, &reference.sound) {
        if kind == Focus::Sound {
            let attack = ratio(m.envelope.attack_ms + 1.0, r.envelope.attack_ms + 1.0);
            let length = ratio(m.envelope.length_ms + 1.0, r.envelope.length_ms + 1.0);
            if attack.abs() >= 1.6 || length.abs() >= 1.6 {
                found.push((
                    StructuralKind::Envelope,
                    Some(Envelope),
                    if attack.abs() >= 1.6 {
                        format!("attack {} ms against {} ms", to_string(m.envelope.attack_ms), to_string(r.envelope.attack_ms))
                    } else {
                        format!("length {} ms against {} ms", to_string(m.envelope.length_ms), to_string(r.envelope.length_ms))
                    },
                ));
            }
        }
    }
    if let (Some(m), Some(r)) = (mine_pitch, ref_pitch) {
        if (12.0 * (m.hz / r.hz).log2()).abs() >= 5.0 {
            found.push((StructuralKind::Register, Some(Pitch), format!("{} against {}", m.note, r.note)));
        }
    } else if kind == Focus::Sound && mine_pitch.is_some() != ref_pitch.is_some() {
        found.push((
            StructuralKind::Pitched,
            Some(Pitch),
            if ref_pitch.is_some() { "the reference is pitched; this isn't" } else { "this is pitched; the reference isn't" }.into(),
        ));
    }
    if let (Some(m), Some(r)) = (&mine.stereo, &reference.stereo) {
        if (m.width - r.width).abs() >= 0.35 {
            found.push((
                StructuralKind::Width,
                Some(Width),
                format!("{} than the reference", if m.width > r.width { "far wider" } else { "far narrower" }),
            ));
        }
    }
    if kind == Focus::Section && ratio(onsets[0] + 0.1, onsets[1] + 0.1).abs() >= 1.6 {
        found.push((
            StructuralKind::Density,
            Some(Density),
            format!("{} against {} onsets a second", to_string(onsets[0]), to_string(onsets[1])),
        ));
    }
    let lost = |name| features.iter().find(|f| f.name == name).map_or(0.0, |f| (1.0 - f.similarity) * f.weight);
    let mut found: Vec<_> = found
        .into_iter()
        .map(|(kind, name, gap)| (kind, gap, name.map_or_else(|| lost(Balance) + lost(Tilt) + lost(Brightness), lost)))
        .collect();
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    let total_lost = features.iter().map(|f| (1.0 - f.similarity) * f.weight).sum::<f64>();
    let structural = found.first().and_then(|(kind, gap, cost)| {
        let share = if total_lost > 0.0 { cost / total_lost } else { 0.0 };
        if share >= 0.3 {
            Some(Structural { kind: *kind, gap: gap.clone(), r#move: structure_move(*kind).into(), share: round(share * 100.0) / 100.0 })
        } else {
            None
        }
    });
    let total = features.iter().map(|f| f.weight).sum::<f64>();
    let score = if total > 0.0 { features.iter().map(|f| f.similarity * f.weight).sum::<f64>() / total } else { 0.0 };
    let mut gaps: Vec<_> = features.iter().filter(|f| f.gap.is_some()).collect();
    gaps.sort_by(|a, b| ((1.0 - b.similarity) * b.weight).total_cmp(&((1.0 - a.similarity) * a.weight)));
    let gaps = gaps.into_iter().take(5).map(|f| f.gap.clone().unwrap()).collect();
    for f in &mut features {
        f.similarity = round(f.similarity * 100.0);
    }
    Closeness { score: round(score * 100.0), focus: kind, features, gaps, structural }
}
