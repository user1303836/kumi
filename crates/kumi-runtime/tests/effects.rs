//! Effects, read off synthetic sounds: a reverb's decay time and how its tail darkens and widens, echoes found (and a
//! rhythm not taken for them), a tremolo's depth, a filter's sweep, pumping against the beat, the tail's share, a tail
//! cut off, and a dry sound judged against a wet reference.
use kumi_runtime::listening::{
    checklist::{Checklist, Goal, Profile},
    effects::{cut_tails, decay_time, echo, effects, note_value, pump, sweep, swing, tail_share},
    measure::{measure_samples, Heard},
};
use std::f64::consts::PI;

const RATE: f64 = 48_000.;
/// An amplitude falling 60 dB over `rt60` seconds, `t` seconds in.
fn falling(t: f64, rt60: f64) -> f64 {
    (-6.9078 * t / rt60).exp()
}

struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2. - 1.
    }
}

fn heard(left: &[f64], right: &[f64]) -> Heard {
    let left: Vec<f32> = left.iter().map(|s| *s as f32).collect();
    let right: Vec<f32> = right.iter().map(|s| *s as f32).collect();
    measure_samples(&left, &right, RATE)
}

/// A burst every 1.5 s for 6 s, gone in 0.2 s; wet, a tail after it falling 60 dB in 1.2 s, its top rolling off an
/// octave every 0.2 s, each side its own noise; `cut` stops the tail that far after each burst.
fn hits(wet: bool, cut: Option<f64>) -> Heard {
    let (mut burst, mut left_noise, mut right_noise) = (Noise(1), Noise(2), Noise(3));
    let (mut low_left, mut low_right) = ([0.; 2], [0.; 2]);
    let count = (6. * RATE) as usize;
    let (mut left, mut right) = (vec![0.; count], vec![0.; count]);
    for n in 0..count {
        let t = (n as f64 / RATE) % 1.5;
        let dry = 0.5 * burst.next() * falling(t, 0.2);
        let (mut l, mut r) = (dry, dry);
        if wet && cut.is_none_or(|cut| t < cut) {
            let cutoff = (8000. * 2f64.powf(-t / 0.2)).max(300.);
            let pole = 1. - (-2. * PI * cutoff / RATE).exp();
            // Rolled off without getting quieter for it: the noise's power follows the cutoff.
            let gain = 0.15 * falling(t, 1.2) * (8000. / cutoff).sqrt();
            for (state, noise, out) in [(&mut low_left, &mut left_noise, &mut l), (&mut low_right, &mut right_noise, &mut r)] {
                state[0] += pole * (noise.next() - state[0]);
                state[1] += pole * (state[0] - state[1]);
                *out += gain * state[1] * 1.4;
            }
        }
        left[n] = l;
        right[n] = r;
    }
    heard(&left, &right)
}

#[test]
fn a_reverbs_decay_darkening_width_and_share_are_read_off_its_tails() {
    let (wet, dry) = (hits(true, None), hits(false, None));
    let (wet_fx, dry_fx) = (effects(&wet, None, None), effects(&dry, None, None));
    let time = wet_fx.decay_time.unwrap();
    assert!((time - 1.2).abs() < 0.25, "{time} s");
    assert!(dry_fx.decay_time.unwrap() < 0.35, "{dry_fx:?}");
    assert!(wet_fx.darkening.unwrap() > 0.5 && dry_fx.darkening.unwrap_or(0.).abs() < 0.3, "{wet_fx:?} against {dry_fx:?}");
    assert!(wet_fx.widening.unwrap() > 3. && dry_fx.widening.unwrap_or(0.) < 1., "{wet_fx:?} against {dry_fx:?}");
    let (wet_share, dry_share) = (tail_share(&wet).unwrap(), tail_share(&dry).unwrap());
    assert!(wet_share > dry_share + 10. && dry_share < 10., "{wet_share} % against {dry_share} %");
    // A tail stopped half a second in reads as cut; one that dies away doesn't.
    assert!(cut_tails(&hits(true, Some(0.5))).len() >= 3);
    assert!(cut_tails(&wet).is_empty() && cut_tails(&dry).is_empty());
    // A dry sound against the wet one as its reference: the checklist asks for the longer decay.
    let goal = Goal { reference: Some(Profile::of("wet", &wet)), sound: true, ..Default::default() };
    let checklist = Checklist::new(&goal, &dry, &[]);
    let values = checklist.read(&dry, None);
    let at = checklist.items.iter().position(|item| item.id == "decay time").expect("a decay time item");
    assert!(checklist.items[at].gap(values[at]) > 3., "{:?} reads {:?}", checklist.items[at], values[at]);
}

#[test]
fn echoes_are_found_and_a_rhythm_isnt_taken_for_them() {
    // A blip every 2 s, echoed every 375 ms, each echo 6 dB under the one before, five of them.
    let blip = |t: f64| if (0. ..0.02).contains(&t) { (2. * PI * 1000. * t).sin() * (-t / 0.005).exp() } else { 0. };
    let echoed: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| (n as f64 / RATE) % 2.)
        .map(|t| (0..=5).map(|k| 0.5 * 10f64.powf(-6. * k as f64 / 20.) * blip(t - k as f64 * 0.375)).sum())
        .collect();
    let found = echo(&heard(&echoed, &echoed)).expect("echoes");
    assert!((found.ms - 375.).abs() <= 10. && (found.falls - 6.).abs() <= 1.5 && found.repeats >= 4, "{found:?}");
    assert_eq!(note_value(found.ms / 1000., 120.).0, "dotted 1/8");
    assert!(note_value(0.3, 120.).1 < -0.05, "300 ms sits between note values at 120 BPM");
    // Hats on the eighths at 120 BPM, the ones on the beat 4 dB louder: a rhythm, not echoes.
    let mut noise = Noise(9);
    let hats: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| {
            let t = n as f64 / RATE;
            let (into, accent) = (t % 0.25, (t % 0.5) < 0.25);
            let level = if accent { 0.5 } else { 0.5 * 10f64.powf(-4. / 20.) };
            if into < 0.03 {
                level * noise.next() * (-into / 0.008).exp()
            } else {
                0.
            }
        })
        .collect();
    assert_eq!(echo(&heard(&hats, &hats)), None);
}

#[test]
fn a_tremolos_depth_a_filters_sweep_and_pumping_against_the_beat() {
    // A tone swinging 6 dB (top to bottom) four times a second.
    let tremolo: Vec<f64> = (0..(6. * RATE) as usize)
        .map(|n| n as f64 / RATE)
        .map(|t| 0.3 * 10f64.powf(3. * (2. * PI * 4. * t).sin() / 20.) * (2. * PI * 440. * t).sin())
        .collect();
    let depth = swing(&heard(&tremolo, &tremolo)).unwrap();
    assert!((depth - 6.).abs() < 1.2, "{depth} dB");
    // Noise through a filter sweeping three octaves (350 Hz to 2.8 kHz) and back every 2 s.
    let mut noise = Noise(4);
    let mut state = [0.; 2];
    let swept: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| {
            let cutoff = 1000. * 2f64.powf(1.5 * (2. * PI * 0.5 * n as f64 / RATE).sin());
            let pole = 1. - (-2. * PI * cutoff / RATE).exp();
            state[0] += pole * (noise.next() - state[0]);
            state[1] += pole * (state[0] - state[1]);
            state[1]
        })
        .collect();
    let (octaves, cycle) = sweep(&heard(&swept, &swept)).unwrap();
    assert!(octaves > 1.5 && (cycle.unwrap() - 2.).abs() < 0.25, "{octaves} octaves every {cycle:?} s");
    // A pad ducking 8 dB on every beat at 120 BPM, back over the first 30 % of the beat.
    let mut noise = Noise(5);
    let pad: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| {
            let phase = (n as f64 / RATE % 0.5) / 0.5;
            let duck = if phase < 0.3 { -8. * (1. - phase / 0.3) } else { 0. };
            0.2 * noise.next() * 10f64.powf(duck / 20.)
        })
        .collect();
    let pumped = pump(&heard(&pad, &pad), 120., Some(0.)).unwrap();
    assert!(pumped.depth > 5. && pumped.depth < 10. && pumped.lowest < Some(0.15) && pumped.back > 0.1 && pumped.back < 0.45, "{pumped:?}");
    // A hit isn't pumping.
    assert_eq!(pump(&hits(false, None), 120., Some(0.)), None);
    assert!(decay_time(&heard(&pad, &pad)).is_none_or(|time| time < 0.5));
}
