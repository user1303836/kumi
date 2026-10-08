//! Sound design, measured on synthetic sounds: an 808's pitch drop, a tremolo's rate, clicks at note edges and dust,
//! harmonics (warmth), width and top, a kit that piles up, a pad ducking under a kick, hats interlocking with it, a
//! bass note off the key, and a sound judged against a reference sound.
use kumi_runtime::listening::{
    checklist::{Checklist, Goal, Profile},
    measure::{measure_samples, Heard},
    sound::{against_key, bandwidth, ducking, harmonics, interlock, kit, problems, tail, width},
};
use std::f64::consts::PI;

const RATE: f64 = 48_000.;

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
fn mono(samples: &[f64]) -> Heard {
    heard(samples, samples)
}

/// Notes every `every` seconds for `seconds`: each from `make(time into the note)`, silent between.
fn notes(seconds: f64, every: f64, length: f64, mut make: impl FnMut(f64) -> f64) -> Vec<f64> {
    (0..(seconds * RATE) as usize)
        .map(|n| {
            let into = (n as f64 / RATE) % every;
            if into < length {
                make(into)
            } else {
                0.
            }
        })
        .collect()
}

#[test]
fn an_808s_pitch_drop_and_a_tremolos_rate_are_measured() {
    // A sine falling from 60 to 45 Hz over its first 200 ms, decaying, every second.
    let eight = notes(6., 1., 0.9, |t| {
        // Its phase: the integral of a pitch falling from 60 to 45 Hz.
        let phase = 2. * PI * (45. * t + 15. * 0.07 * (1. - (-t / 0.07).exp()));
        0.8 * (-t / 0.5).exp() * phase.sin()
    });
    // Its first period (about 55 Hz) against 200 ms in (about 46 Hz): some 3 semitones.
    let drop = mono(&eight).measures.pitch_drop.unwrap();
    assert!((2.5..4.5).contains(&drop), "{drop} semitones");
    // A held tone swinging 4 times a second.
    let tremolo: Vec<f64> = (0..(6. * RATE) as usize)
        .map(|n| n as f64 / RATE)
        .map(|t| 0.3 * (1. + 0.5 * (2. * PI * 4. * t).sin()) * (2. * PI * 440. * t).sin())
        .collect();
    let rate = mono(&tremolo).measures.modulation.unwrap();
    assert!((rate - 4.).abs() < 0.3, "{rate} Hz");
}

#[test]
fn clicks_at_note_edges_and_dust_are_counted() {
    // Notes cut in at the top of their wave: each starts with a jump out of silence.
    let cut = notes(4., 0.5, 0.25, |t| 0.5 * (2. * PI * 440. * t + PI / 2.).sin());
    let measured = mono(&cut);
    assert!(measured.measures.clicks >= 6, "{} clicks", measured.measures.clicks);
    assert!(problems(&measured).iter().any(|problem| problem.contains("start with a click")));
    // The same notes faded in over 5 ms: none.
    let faded = notes(4., 0.5, 0.25, |t| 0.5 * (t / 0.005).min(1.) * (2. * PI * 440. * t + PI / 2.).sin());
    assert_eq!(mono(&faded).measures.clicks, 0);
    // A quiet bed with a lone spike every 50 ms: about 20 a second.
    let mut noise = Noise(3);
    let dusty: Vec<f64> = (0..(4. * RATE) as usize).map(|n| noise.next() * 0.003 + if n % 2400 == 0 { 0.2 } else { 0. }).collect();
    let crackle = mono(&dusty).measures.crackle;
    assert!((crackle - 20.).abs() < 5., "{crackle} a second");
    let mut noise = Noise(4);
    let clean: Vec<f64> = (0..(4. * RATE) as usize).map(|_| noise.next() * 0.003).collect();
    assert!(mono(&clean).measures.crackle < 2.);
}

#[test]
fn harmonics_width_top_and_tail_read_as_heard() {
    // A 330 Hz tone with its 2nd 6 dB and its 3rd 12 dB under it: warm; the bare tone isn't.
    let tone = |second: f64, third: f64| -> Vec<f64> {
        (0..(3. * RATE) as usize)
            .map(|n| n as f64 / RATE)
            .map(|t| 0.3 * ((2. * PI * 330. * t).sin() + second * (2. * PI * 660. * t).sin() + third * (2. * PI * 990. * t).sin()))
            .collect()
    };
    let warm = harmonics(&mono(&tone(0.5, 0.25))).unwrap();
    let bare = harmonics(&mono(&tone(0., 0.))).unwrap();
    assert!((warm.fundamental - 330.).abs() < 20., "{warm:?}");
    assert!(warm.warmth > -8. && warm.warmth < -2., "{warm:?}");
    assert!(bare.warmth < warm.warmth - 15., "{bare:?} against {warm:?}");
    // Mono is narrow; two unrelated noises are wide.
    let (mut a, mut b) = (Noise(5), Noise(6));
    let left: Vec<f64> = (0..(2. * RATE) as usize).map(|_| a.next() * 0.2).collect();
    let right: Vec<f64> = (0..(2. * RATE) as usize).map(|_| b.next() * 0.2).collect();
    assert!(width(&mono(&left)).unwrap() <= -30.);
    assert!(width(&heard(&left, &right)).unwrap() > -1.);
    // Noise rolled off steeply above about 2 kHz stops lower than noise that isn't.
    let mut stages = [0.; 6];
    let dull: Vec<f64> = left
        .iter()
        .map(|s| {
            let mut value = *s;
            for stage in stages.iter_mut() {
                *stage += 0.25 * (value - *stage);
                value = *stage;
            }
            value * 4.
        })
        .collect();
    let (top_dull, top_full) = (bandwidth(&mono(&dull)).unwrap(), bandwidth(&mono(&left)).unwrap());
    assert!(top_dull < top_full, "{top_dull} against {top_full}");
    // Dry hits fall far in 300 ms; ringing ones hang on.
    let dry = notes(4., 0.5, 0.45, |t| 0.6 * (-t / 0.02).exp() * (2. * PI * 200. * t).sin());
    let wet = notes(4., 0.5, 0.45, |t| 0.6 * (-t / 0.25).exp() * (2. * PI * 200. * t).sin());
    assert!(tail(&mono(&dry)).unwrap() < tail(&mono(&wet)).unwrap() - 10.);
}

#[test]
fn a_kit_that_piles_up_and_parts_that_duck_and_interlock() {
    // Three noises filtered alike pile up; a pure low tone stands apart.
    let filtered = |seed: u64, keep: f64| -> Vec<f64> {
        let mut noise = Noise(seed);
        let mut state = 0.;
        notes(2., 0.25, 0.1, |_| {
            state += keep * (noise.next() - state);
            state * 0.5
        })
    };
    let pieces = vec![
        ("Snare".to_string(), mono(&filtered(7, 0.3))),
        ("Clap".to_string(), mono(&filtered(8, 0.3))),
        ("Rim".to_string(), mono(&filtered(9, 0.3))),
        ("Kick".to_string(), mono(&notes(2., 0.5, 0.3, |t| 0.8 * (-t / 0.1).exp() * (2. * PI * 55. * t).sin()))),
    ];
    let (measured, found) = kit(&pieces, 120.);
    assert_eq!(measured.len(), 4);
    assert!(found.iter().any(|problem| problem.contains("pile up")), "{found:?}");
    // A pad ducking 6 dB under a kick on every beat, back within about 150 ms.
    let kick = notes(4., 0.5, 0.1, |t| 0.8 * (-t / 0.03).exp() * (2. * PI * 55. * t).sin());
    let mut noise = Noise(10);
    let pad: Vec<f64> = (0..(4. * RATE) as usize)
        .map(|n| {
            let into = (n as f64 / RATE) % 0.5;
            let duck = if into < 0.15 { 0.5 + 0.5 * (into / 0.15) } else { 1. };
            noise.next() * 0.2 * duck
        })
        .collect();
    let (depth, recovery) = ducking(&mono(&pad), &mono(&kick)).unwrap();
    assert!((depth - 6.).abs() < 2.5 && recovery > 60. && recovery < 250., "{depth} dB, {recovery} ms");
    // Hats between the kicks interlock; hats on them collide.
    let offbeat: Vec<f64> = (0..(4. * RATE) as usize)
        .map(|n| {
            let into = (n as f64 / RATE + 0.25) % 0.5;
            if into < 0.03 {
                (2. * PI * 8000. * into).sin() * 0.5 * (-into / 0.01).exp()
            } else {
                0.
            }
        })
        .collect();
    let (colliding, interlocking) = interlock(&mono(&offbeat), &mono(&kick)).unwrap();
    assert!(colliding <= 10. && interlocking >= 90., "{colliding}% collide, {interlocking}% interlock");
}

#[test]
fn a_bass_note_off_the_key_is_heard_and_a_sound_is_judged_against_a_reference_sound() {
    // A bass line in C major (C, E, G at 65, 82, 98 Hz) with one F# among them.
    let line = |hz: [f64; 4]| -> Vec<f64> {
        (0..(4. * RATE) as usize).map(|n| n as f64 / RATE).map(|t| 0.5 * (2. * PI * hz[((t / 1.) as usize).min(3)] * t).sin()).collect()
    };
    let major = [0u8, 2, 4, 5, 7, 9, 11];
    let (_, in_key) = against_key(&mono(&line([65.41, 82.41, 98., 65.41])), &major).unwrap();
    let (_, off_key) = against_key(&mono(&line([65.41, 82.41, 92.5, 65.41])), &major).unwrap();
    assert!(in_key < 10. && off_key > 15., "{in_key}% and {off_key}% off the key");
    // An 808 with its drop as the reference; one without as the sound: the checklist asks for the drop.
    let falling = notes(6., 1., 0.9, |t| 0.8 * (-t / 0.5).exp() * (2. * PI * (45. * t + 15. * 0.07 * (1. - (-t / 0.07).exp()))).sin());
    let flat = notes(6., 1., 0.9, |t| 0.8 * (-t / 0.5).exp() * (2. * PI * 45. * t).sin());
    let reference = Profile::of("808", &mono(&falling));
    let sound = mono(&flat);
    let goal = Goal { reference: Some(reference), sound: true, ..Default::default() };
    let checklist = Checklist::new(&goal, &sound, &[]);
    let values = checklist.read(&sound, None);
    let at = checklist.items.iter().position(|item| item.id == "pitch drop").expect("a pitch drop item");
    assert!(checklist.items[at].gap(values[at]) > 3., "{:?} reads {:?}", checklist.items[at], values[at]);
    assert_eq!(checklist.next(&values).map(|index| checklist.items[index].id.as_str()).is_some(), true);
}
