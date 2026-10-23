//! Sound design, measured on synthetic sounds: an 808's pitch drop (at 44.1 kHz over a long listen too), a tremolo's
//! rate, clicks at note edges and dust, harmonics (warmth) on either side of 250 Hz, width and top, a noise floor and
//! digital silence, a kit that piles up and one that doesn't, a pad ducking under a kick, hats interlocking with it, a
//! bass note off the key and a lead that isn't a bass line, and a sound judged against a reference sound.
use kumi_runtime::listening::{
    checklist::{Checklist, Goal, Profile},
    measure::{measure_samples, Heard},
    sound::{against_key, bandwidth, ducking, harmonics, interlock, key_classes, kit, noise_floor, problems, tail, width},
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
fn a_low_notes_own_waveform_isnt_a_swing_and_each_hits_crest_reads_its_grit() {
    // A held 55 Hz saw: its waveform ripples the millisecond envelope, which isn't a swing; with a 4 Hz tremolo, it is.
    let saw = |depth: f64| -> Vec<f64> {
        (0..(6. * RATE) as usize)
            .map(|n| n as f64 / RATE)
            .map(|t| {
                let tremolo = 10f64.powf(depth * (2. * PI * 4. * t).sin() / 20.);
                0.15 * tremolo * (1..=12).map(|k| (2. * PI * 55. * k as f64 * t).sin() / k as f64).sum::<f64>()
            })
            .collect()
    };
    assert_eq!(mono(&saw(0.)).measures.modulation, None);
    let rate = mono(&saw(3.)).measures.modulation.unwrap();
    assert!((rate - 4.).abs() < 0.3, "{rate} Hz");
    // Each hit's crest: driven into saturation it flattens; a reverb tail after it leaves it be.
    let mut noise = Noise(21);
    let snare = notes(4., 0.5, 0.45, |t| 0.4 * noise.next() * (-t / 0.05).exp());
    let clean = mono(&snare).measures.hit_crest.unwrap();
    let driven: Vec<f64> = snare.iter().map(|sample| 0.5 * (4. * sample).tanh() / 4f64.tanh()).collect();
    let gritty = mono(&driven).measures.hit_crest.unwrap();
    assert!(gritty < clean - 3., "{clean} dB, driven {gritty} dB");
    let mut noise = Noise(22);
    let tail: Vec<f64> = notes(4., 0.5, 0.45, |t| if t > 0.01 { 0.1 * noise.next() * (-t / 0.3).exp() } else { 0. });
    let wet: Vec<f64> = snare.iter().zip(&tail).map(|(hit, tail)| hit + tail).collect();
    let roomy = mono(&wet).measures.hit_crest.unwrap();
    assert!((roomy - clean).abs() < 1.5, "{clean} dB, with a tail {roomy} dB");
}

#[test]
fn at_44_1_khz_an_808s_drop_holds_over_a_long_listen() {
    // The 808 every second for 40 s at 44.1 kHz: its millisecond steps (44 and 45 samples) keep to the clock, so its
    // hits don't drift off the pitch they're read at.
    let rate = 44_100.;
    let samples: Vec<f32> = (0..(40. * rate) as usize)
        .map(|n| {
            let t = (n as f64 / rate) % 1.;
            let phase = 2. * PI * (45. * t + 15. * 0.07 * (1. - (-t / 0.07).exp()));
            if t < 0.9 {
                (0.8 * (-t / 0.5).exp() * phase.sin()) as f32
            } else {
                0.
            }
        })
        .collect();
    let drop = measure_samples(&samples, &samples, rate).measures.pitch_drop.unwrap();
    assert!((2.5..4.5).contains(&drop), "{drop} semitones");
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
    // The same saturation on a 150 Hz bass (partials over 250 Hz) and an 82 Hz one (all under it) reads the same.
    let saturated = |hz: f64| -> Vec<f64> {
        (0..(3. * RATE) as usize)
            .map(|n| n as f64 / RATE)
            .map(|t| 0.3 * ((2. * PI * hz * t).sin() + 0.5 * (4. * PI * hz * t).sin() + 0.25 * (6. * PI * hz * t).sin()))
            .collect()
    };
    let (high, low) = (harmonics(&mono(&saturated(150.))).unwrap(), harmonics(&mono(&saturated(82.))).unwrap());
    assert!((high.warmth - low.warmth).abs() < 1. && (high.warmth + 5.).abs() < 1.5, "{high:?} against {low:?}");
    // A 55 Hz note's 3rd (165 Hz) is read apart from its loud 4th (220 Hz), a third-octave away.
    let fourth: Vec<f64> = (0..(3. * RATE) as usize)
        .map(|n| n as f64 / RATE)
        .map(|t| {
            let phase = 2. * PI * 55. * t;
            0.25 * (phase.sin() + 0.5 * (2. * phase).sin() + 0.25 * (3. * phase).sin() + 0.7 * (4. * phase).sin())
        })
        .collect();
    let deep = harmonics(&mono(&fourth)).unwrap();
    assert!((deep.warmth + 5.).abs() < 1.5, "{deep:?}");
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
    // Digital silence after a dry hit reads as about 70 dB down, not 200.
    assert!(tail(&mono(&dry)).unwrap() > -75., "{:?}", tail(&mono(&dry)));
    // Dry 16th hats at 120 BPM: the next hat comes within 300 ms, so there's no tail to read.
    let mut noise = Noise(12);
    let hats = notes(4., 0.125, 0.03, |t| noise.next() * 0.5 * (-t / 0.008).exp());
    assert_eq!(tail(&mono(&hats)), None);
}

#[test]
fn a_noise_floor_is_where_quiet_stretches_hold_still() {
    // Notes with digital silence between them: 70 dB down, as far as it reads.
    let clean = notes(4., 0.5, 0.2, |t| 0.5 * (2. * PI * 440. * t).sin());
    assert_eq!(noise_floor(&mono(&clean)), Some(-70.));
    // A hiss about 52 dB under them reads as itself.
    let mut noise = Noise(11);
    let hissy: Vec<f64> = clean.iter().map(|sample| sample + 0.5 * 10f64.powf(-50. / 20.) * noise.next()).collect();
    let floor = noise_floor(&mono(&hissy)).unwrap();
    assert!((-56. ..-48.).contains(&floor), "{floor} dB");
    // Gaps a long tail fills never hold still: no floor to read, rather than the tail read as one.
    let ringing = notes(4., 0.5, 0.5, |t| 0.5 * (-6.9 * t / 1.2).exp() * (2. * PI * 440. * t).sin());
    assert_eq!(noise_floor(&mono(&ringing)), None);
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
    // The kick is a kick: octaves under the rest, so its top and grit aren't held against theirs.
    assert!(!found.iter().any(|problem| problem.contains("sits apart") || problem.contains("nothing between")), "{found:?}");
    // An ordinary kit (kick, snare, clap and hats) hangs together, and a crash ringing between its rare hits is fine.
    let band = |seed: u64, keep: f64, every: f64, length: f64| -> Vec<f64> {
        let mut noise = Noise(seed);
        let (mut state, mut last) = (0., 0.);
        notes(4., every, length, |t| {
            // Noise high-passed (keep near 1) or low-passed (keep small), decaying.
            let raw = noise.next();
            state += keep * (raw - state);
            let out = if keep > 0.5 { raw - last } else { state };
            last = raw;
            out * 0.5 * (-t / (length / 3.)).exp()
        })
    };
    let ordinary = vec![
        ("Kick".to_string(), mono(&notes(4., 0.5, 0.3, |t| 0.8 * (-t / 0.1).exp() * (2. * PI * 55. * t).sin()))),
        ("Snare".to_string(), mono(&band(13, 0.3, 1., 0.2))),
        ("Clap".to_string(), mono(&band(14, 0.2, 1., 0.15))),
        ("Hats".to_string(), mono(&band(15, 0.9, 0.25, 0.05))),
        ("Crash".to_string(), mono(&band(16, 0.9, 2., 1.9))),
    ];
    let (_, found) = kit(&ordinary, 120.);
    assert!(!found.iter().any(|problem| problem.contains("sits apart") || problem.contains("nothing between")), "{found:?}");
    assert!(!found.iter().any(|problem| problem.contains("Crash rings")), "{found:?}");
    // Two tracks far apart (a bass and hats) aren't a kit with a gap.
    let bass = ("Bass".to_string(), mono(&notes(4., 0.5, 0.4, |t| 0.5 * (2. * PI * 100. * t).sin())));
    let (_, found) = kit(&[bass, ordinary[3].clone()], 120.);
    assert!(!found.iter().any(|problem| problem.contains("nothing between")), "{found:?}");
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
    // A take shorter than the kick's (measured apart): its hits past the end have nothing to duck.
    assert!(ducking(&mono(&pad[..pad.len() / 2]), &mono(&kick)).is_some());
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
    let major = key_classes("C major").unwrap();
    let (_, in_key) = against_key(&mono(&line([65.41, 82.41, 98., 65.41])), &major).unwrap();
    let (_, off_key) = against_key(&mono(&line([65.41, 82.41, 92.5, 65.41])), &major).unwrap();
    assert!(in_key < 10. && off_key > 15., "{in_key}% and {off_key}% off the key");
    // A lead in C major (C5, E5, G5) isn't a bass line: no low notes are invented for it, and its fundamental is its own.
    let lead = mono(&line([523.25, 659.25, 783.99, 523.25]));
    assert_eq!(against_key(&lead, &major), None);
    let own = harmonics(&lead).unwrap();
    assert!((own.fundamental - 523.).abs() < 30., "{own:?}");
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
    assert!(checklist.next(&values).is_some());
}

#[test]
fn keys_read_as_said() {
    let sorted = |key: &str| {
        key_classes(key).map(|mut classes| {
            classes.sort_unstable();
            classes
        })
    };
    assert_eq!(sorted("Bb major"), Some(vec![0, 2, 3, 5, 7, 9, 10]));
    for minor in ["F# minor", "F#m", "f#min", "F♯ Min", "Gb minor"] {
        assert_eq!(sorted(minor), Some(vec![1, 2, 4, 6, 8, 9, 11]), "{minor}");
    }
    assert_eq!(sorted("E♭m"), sorted("Eb minor"));
    // D dorian has C major's notes; A aeolian is A minor.
    assert_eq!(sorted("D dorian"), sorted("C"));
    assert_eq!(sorted("A aeolian"), sorted("Am"));
    // The minor's 7th raised, and its 6th too.
    assert_eq!(sorted("C harmonic minor"), Some(vec![0, 2, 3, 5, 7, 8, 11]));
    assert_eq!(sorted("C melodic minor"), Some(vec![0, 2, 3, 5, 7, 9, 11]));
    for not_a_key in ["", "H minor", "apple", "C hungarian"] {
        assert_eq!(key_classes(not_a_key), None, "{not_a_key}");
    }
}

#[test]
fn a_saturated_or_held_sound_keeps_its_punch_reading_and_a_slow_swing_reads() {
    // Plucked 16ths at 120 BPM (a bright tone falling fast): driven 12 dB into saturation, a note's tail stays near
    // the next note's level, so the hits' 12 dB rises are gone. Its punch still reads, and lower.
    let pluck = |t: f64| 0.5 * (-t / 0.05).exp() * (1..=8).map(|k| (2. * PI * 330. * k as f64 * t).sin() / k as f64).sum::<f64>();
    let plucks = notes(4., 0.125, 0.125, pluck);
    let clean = mono(&plucks).measures.hit_crest.unwrap();
    let driven: Vec<f64> = plucks.iter().map(|sample| 0.5 * (4. * sample).tanh()).collect();
    let flattened = mono(&driven).measures.hit_crest.expect("a saturated part's punch still reads");
    assert!(flattened < clean - 2., "{clean} dB, driven {flattened} dB");
    // A held pad has no hits: its punch is its louder stretches' crest, and a sound goal guards it.
    let pad: Vec<f64> = (0..(4. * RATE) as usize)
        .map(|n| n as f64 / RATE)
        .map(|t| {
            0.1 * [220., 277.2, 329.6]
                .iter()
                .map(|hz| (1..=10).map(|k| (2. * PI * hz * k as f64 * t).sin() / k as f64).sum::<f64>())
                .sum::<f64>()
        })
        .collect();
    let held = mono(&pad);
    assert!(held.measures.hit_crest.is_some(), "{:?}", held.measures.hit_crest);
    let goal = Goal { reference: Some(Profile::of("pad", &held)), sound: true, ..Default::default() };
    let checklist = Checklist::new(&goal, &held, &[]);
    assert!(checklist.items.iter().any(|item| item.id == "hit crest"), "{:?}", checklist.items);
    // A held tone swinging once every two seconds, over 16 s.
    let slow: Vec<f64> = (0..(16. * RATE) as usize)
        .map(|n| n as f64 / RATE)
        .map(|t| 0.3 * 10f64.powf(3. * (2. * PI * 0.5 * t).sin() / 20.) * (2. * PI * 440. * t).sin())
        .collect();
    let rate = mono(&slow).measures.modulation.expect("a 0.5 Hz swing reads");
    assert!((rate - 0.5).abs() < 0.02, "{rate} Hz");
}
