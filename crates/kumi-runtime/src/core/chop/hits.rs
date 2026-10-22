//! Where a drum recording's hits are, to the frame, and what each is: an onset found as a sharp rise of a peak follower's
//! level (of the signal and of its first difference, so a kick's lows and a hat's edge both rise, and a low sine's own swing
//! never does), placed on the sample where its energy steps up and onto the zero crossing before it (a slice cut there never
//! clicks), with its peak and how its energy splits between lows and highs: a kick, a snare or a hat, and a ghost when it's far
//! quieter than its kind's loudest. Rolls need the tempo and are the plan's.
use crate::audio::dsp::{high_pass, low_pass};

/// A hit: where its attack starts (frame, from 0), how loud it peaks, its kind, and whether it's a ghost.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub frame: usize,
    pub seconds: f64,
    pub peak_db: f64,
    pub role: Role,
    pub ghost: bool,
    /// The shares of its energy below 150 Hz and above 5 kHz.
    pub lows: f64,
    pub highs: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Kick,
    Snare,
    Hat,
}

impl Role {
    pub fn word(self) -> &'static str {
        match self {
            Role::Kick => "kick",
            Role::Snare => "snare",
            Role::Hat => "hat",
        }
    }
}

/// A rise this steep (dB over 3 ms) starts a hit.
const RISE_DB: f64 = 6.0;
/// Hits closer than this are one (seconds).
const GAP: f64 = 0.02;
/// What's quieter than the loudest hit by more than this is the noise floor, not a hit (dB).
const FLOOR_DB: f64 = 45.0;
/// A hit this far under the loudest of its kind is a ghost (dB).
const GHOST_DB: f64 = 9.0;

/// A follower's release (seconds): long enough that a 40 Hz sine's swing between its peaks stays under the rise a hit makes.
const RELEASE: f64 = 0.03;

/// A peak follower's level each millisecond (dB): instant attack, `RELEASE` decay.
fn followed(values: impl Iterator<Item = f64>, rate: f64, hop: usize) -> Vec<f64> {
    let fall = (-1. / (RELEASE * rate)).exp();
    let mut env = 0f64;
    let mut out = Vec::new();
    let mut most = 0f64;
    for (n, value) in values.enumerate() {
        env = value.abs().max(env * fall);
        most = most.max(env);
        if (n + 1) % hop == 0 {
            out.push(20. * most.max(1e-9).log10());
            most = 0.;
        }
    }
    out
}

/// The hits of a mono recording at `rate`.
pub fn hits(mono: &[f32], rate: f64) -> Vec<Hit> {
    let hop = ((rate / 1000.).round() as usize).max(1);
    if mono.len() < hop * 32 {
        return Vec::new();
    }
    let plain = followed(mono.iter().map(|v| *v as f64), rate, hop);
    let edges = followed(std::iter::once(0.).chain(mono.windows(2).map(|pair| (pair[1] - pair[0]) as f64)), rate, hop);
    let frames = plain.len().min(edges.len());
    let loudest = plain.iter().copied().fold(f64::MIN, f64::max);
    // Each millisecond's rise over the next three, and which level rose (the signal's, or its first difference's: a hat's).
    let rise: Vec<(f64, bool)> = (0..frames.saturating_sub(3))
        .map(|f| {
            let (lows, edge) = (plain[f + 3] - plain[f], edges[f + 3] - edges[f]);
            if edge > lows {
                (edge.max(0.), true)
            } else {
                (lows.max(0.), false)
            }
        })
        .collect();
    let gap = ((GAP * rate) as usize / hop).max(1);
    let mut found: Vec<(usize, f64, bool)> = Vec::new();
    for (f, (value, edge)) in rise.iter().enumerate() {
        let peak = *value >= RISE_DB
            && (f == 0 || rise[f - 1].0 < *value)
            && rise.get(f + 1).is_none_or(|next| next.0 <= *value)
            && plain[f + 3] > loudest - FLOOR_DB;
        if !peak {
            continue;
        }
        match found.last_mut() {
            // (of two within the gap, the steeper)
            Some((at, kept, which)) if f - *at < gap => {
                if *value > *kept {
                    (*at, *kept, *which) = (f, *value, *edge);
                }
            }
            _ => found.push((f, *value, *edge)),
        }
    }
    let mut out: Vec<Hit> = Vec::new();
    for (k, (f, _, edge)) in found.iter().enumerate() {
        let floor = out.last().map_or(0, |hit: &Hit| hit.frame + hop);
        let levels = if *edge { &edges } else { &plain };
        // (the level halfway up the rise, in dB: the first sample past it is where the attack is)
        let halfway = 10f64.powf((levels[*f] + levels[f + 3]) / 40.);
        let frame = attack_start(mono, rate, *f * hop, hop, halfway, *edge, floor);
        let next = found.get(k + 1).map_or(mono.len(), |(f, _, _)| f * hop);
        let shape = shape(mono, rate, frame, next);
        out.push(Hit {
            frame,
            seconds: frame as f64 / rate,
            peak_db: shape.peak_db,
            role: role_of(&shape),
            ghost: false,
            lows: shape.share[0],
            highs: shape.share[2],
        });
    }
    // The ghosts, by how far under their kind's loudest they peak.
    for role in [Role::Kick, Role::Snare, Role::Hat] {
        let most = out.iter().filter(|hit| hit.role == role).map(|hit| hit.peak_db).fold(f64::MIN, f64::max);
        for hit in out.iter_mut().filter(|hit| hit.role == role) {
            hit.ghost = hit.peak_db < most - GHOST_DB;
        }
    }
    out
}

/// Where an attack that rose from frame `from` (a millisecond of `hop` samples) starts: the first sample, from a millisecond
/// before it to the end of the rise, past `halfway` (of the signal, or of its first difference for an `edge`), then onto the
/// zero crossing at or before it (within a millisecond), never before `floor`.
fn attack_start(mono: &[f32], rate: f64, from: usize, hop: usize, halfway: f64, edge: bool, floor: usize) -> usize {
    let ms = ((rate / 1000.) as usize).max(1);
    let first = from.saturating_sub(ms).max(floor).max(1);
    let last = (from + 4 * hop).min(mono.len());
    let value = |n: usize| if edge { (mono[n] - mono[n - 1]).abs() as f64 } else { mono[n].abs() as f64 };
    let start = (first..last).find(|n| value(*n) >= halfway).unwrap_or(from.max(floor).min(mono.len().saturating_sub(1)));
    // (onto the zero crossing at or before it within a millisecond; with none, a tail sounding under it, the quietest sample)
    let earliest = start.saturating_sub(ms).max(floor);
    let mut at = start;
    while at > earliest && (mono[at] >= 0.) == (mono[at - 1] >= 0.) {
        at -= 1;
    }
    if at > earliest {
        at
    } else {
        // (of equally quiet ones, the latest: silence before an attack is cut at its end)
        (earliest..=start).rev().min_by(|a, b| mono[*a].abs().total_cmp(&mono[*b].abs())).unwrap_or(start)
    }
}

/// A hit's peak (dB) and, over its first 15 ms (or to the next hit) against the 15 ms before (what was sounding already: a
/// kick's tail under a hat), how far each band rose (below 150 Hz, between, above 5 kHz) and each band's share after it.
fn shape(mono: &[f32], rate: f64, frame: usize, next: usize) -> Shape {
    let window = ((rate * 0.015) as usize).min(next.saturating_sub(frame)).max(1);
    let end = (frame + window).min(mono.len());
    let before = frame.saturating_sub(window);
    let settle = before.saturating_sub((rate * 0.01) as usize);
    let mut lows = [low_pass(rate, 150.), low_pass(rate, 150.)];
    let mut highs = [high_pass(rate, 5000f64.min(rate * 0.4)), high_pass(rate, 5000f64.min(rate * 0.4))];
    // (energy before and after the attack: all, lows, highs)
    let mut sums = [[0f64; 3]; 2];
    for (n, value) in mono.iter().enumerate().take(end).skip(settle) {
        let x = *value as f64;
        let l = lows.iter_mut().fold(x, |v, filter| filter.process(v));
        let h = highs.iter_mut().fold(x, |v, filter| filter.process(v));
        let side = if n >= frame {
            1
        } else if n >= before {
            0
        } else {
            continue;
        };
        sums[side][0] += x * x;
        sums[side][1] += l * l;
        sums[side][2] += h * h;
    }
    let bands = |side: usize| {
        let [all, low, high] = sums[side];
        [low, (all - low - high).max(0.), high]
    };
    let (was, now) = (bands(0), bands(1));
    let all = now.iter().sum::<f64>().max(1e-12);
    // (a band from nearly nothing rises as far as a ten-thousandth of what sounds after: two tiny energies say nothing)
    let rose = |k: usize| now[k] / (was[k] + 1e-4 * all);
    let peak =
        mono[frame..mono.len().min(frame + (rate * 0.03) as usize).min(next.max(frame + 1))].iter().fold(0f32, |most, v| most.max(v.abs()));
    Shape {
        peak_db: 20. * (peak as f64).max(1e-9).log10(),
        rose: [rose(0), rose(1), rose(2)],
        share: [now[0] / all, now[1] / all, now[2] / all],
    }
}

struct Shape {
    peak_db: f64,
    /// How far the lows, the middle and the highs rose.
    rose: [f64; 3],
    /// Their shares of the energy after the attack.
    share: [f64; 3],
}

/// A hit's kind by its bands: a kick's lows rise and lead; a hat's highs rise far more than its middle; the rest is a snare.
fn role_of(shape: &Shape) -> Role {
    let [low, mid, high] = shape.rose;
    if low >= 3. && shape.share[0] >= 0.35 {
        Role::Kick
    } else if high >= 3. && mid * 3. < high && shape.share[2] >= 0.4 {
        Role::Hat
    } else {
        Role::Snare
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Noise(u32);

    impl Noise {
        pub(crate) fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (self.0 >> 9) as f32 / 4_194_304. - 1.
        }
    }

    /// A drum sound starting at `at` (frames) added into `into`.
    pub(crate) fn drum(into: &mut [f32], rate: f64, at: usize, role: Role, gain: f32, noise: &mut Noise) {
        let mut hp = [high_pass(rate, 7000.), high_pass(rate, 7000.)];
        for n in 0..(rate * 0.25) as usize {
            let Some(slot) = into.get_mut(at + n) else { break };
            let t = n as f64 / rate;
            let value = match role {
                Role::Kick => {
                    let body = (2. * std::f64::consts::PI * 55. * t).sin() * (-t / 0.12).exp();
                    let click = if t < 0.002 { noise.next() as f64 * 0.5 } else { 0. };
                    body * 0.9 + click
                }
                Role::Snare => (noise.next() as f64 * 0.6 + (2. * std::f64::consts::PI * 190. * t).sin() * 0.4) * (-t / 0.07).exp(),
                Role::Hat => {
                    let x = noise.next() as f64;
                    hp.iter_mut().fold(x, |v, filter| filter.process(v)) * (-t / 0.025).exp() * 1.6
                }
            };
            *slot += value as f32 * gain;
        }
    }

    /// One bar at 170 BPM in 16ths: kicks on 1 and the "and" of 3, snares on 2 and 4, ghost snares, hats between.
    pub(crate) fn break_bar(rate: f64) -> (Vec<f32>, Vec<(usize, Role, bool)>) {
        let sixteenth = 60. / 170. / 4.;
        let mut mono = vec![0f32; (rate * sixteenth * 16.) as usize + 4410];
        let mut noise = Noise(7);
        let events = [
            (0, Role::Kick, false),
            (1, Role::Hat, false),
            (3, Role::Hat, false),
            (4, Role::Snare, false),
            (5, Role::Hat, false),
            (7, Role::Snare, true),
            (9, Role::Hat, false),
            (10, Role::Kick, false),
            (11, Role::Hat, false),
            (12, Role::Snare, false),
            (14, Role::Snare, true),
            (15, Role::Hat, false),
        ];
        let mut truth = Vec::new();
        for (step, role, ghost) in events {
            let at = (step as f64 * sixteenth * rate).round() as usize + 441;
            let gain = if ghost {
                0.15
            } else if role == Role::Hat {
                0.5
            } else {
                1.0
            };
            drum(&mut mono, rate, at, role, gain, &mut noise);
            truth.push((at, role, ghost));
        }
        (mono, truth)
    }

    #[test]
    fn a_breaks_hits_are_found_to_the_millisecond_with_their_kinds_and_ghosts() {
        let rate = 44_100.;
        let (mono, truth) = break_bar(rate);
        let found = hits(&mono, rate);
        assert_eq!(found.len(), truth.len(), "{:#?}", found.iter().map(|h| (h.frame, h.role, h.ghost, h.peak_db)).collect::<Vec<_>>());
        for (hit, (at, role, ghost)) in found.iter().zip(&truth) {
            let off = hit.frame as f64 - *at as f64;
            assert!(off.abs() <= rate / 1000., "a hit at {} found at {} ({off} frames)", at, hit.frame);
            assert_eq!(hit.role, *role, "the hit at {at}: {hit:?}");
            assert_eq!(hit.ghost, *ghost, "the hit at {at}: {hit:?}");
        }
    }

    #[test]
    fn a_slice_starts_on_a_zero_crossing_at_or_before_its_attack_when_one_is_near() {
        let rate = 44_100.;
        let (mono, truth) = break_bar(rate);
        let ms = 44;
        for (hit, (at, _, _)) in hits(&mono, rate).iter().zip(&truth) {
            let n = hit.frame;
            assert!(n <= at + ms / 4 && n + ms >= *at, "the cut at {n} for the hit at {at}");
            let crossing = (n.saturating_sub(ms)..*at).any(|k| k > 0 && (mono[k] >= 0.) != (mono[k - 1] >= 0.));
            if crossing && n > 0 {
                assert!((mono[n] >= 0.) != (mono[n - 1] >= 0.), "frame {n}: {} after {}", mono[n], mono[n - 1]);
            }
        }
    }

    #[test]
    fn silence_and_a_steady_tone_have_no_hits() {
        assert!(hits(&vec![0.; 44_100], 44_100.).is_empty());
        let tone: Vec<f32> = (0..44_100).map(|n| (n as f32 * 0.05).sin() * 0.5).collect();
        assert!(hits(&tone, 44_100.).len() <= 1, "only the tone's start");
    }
}
