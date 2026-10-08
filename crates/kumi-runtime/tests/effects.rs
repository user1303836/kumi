//! Effects, read off synthetic sounds: a reverb's decay time and how its tail darkens and widens, echoes found (and a
//! rhythm not taken for them), a tremolo's depth, a filter's sweep, pumping against the beat, the tail's share, a tail
//! cut off, and a dry sound judged against a wet reference, a change toward it kept and one past it taken back.
use kumi_runtime::listening::{
    checklist::{Change, Checklist, Explicit, Goal, Profile, Quantity, Role, Spread, Target},
    effects::{self, cut_tails, decay_time, echo, echo_falls, effects, note_value, pump, sweep, swing, tail_share, NO_ECHO},
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

/// A burst every 1.5 s for 6 s, gone in 0.2 s; with a tail (its RT60), one after it falling 60 dB in that time, its
/// top rolling off an octave every 0.2 s, each side its own noise; `cut` stops the tail that far after each burst.
fn hits(tail: Option<f64>, cut: Option<f64>) -> Heard {
    let (mut burst, mut left_noise, mut right_noise) = (Noise(1), Noise(2), Noise(3));
    let (mut low_left, mut low_right) = ([0.; 2], [0.; 2]);
    let count = (6. * RATE) as usize;
    let (mut left, mut right) = (vec![0.; count], vec![0.; count]);
    for n in 0..count {
        let t = (n as f64 / RATE) % 1.5;
        let dry = 0.5 * burst.next() * falling(t, 0.2);
        let (mut l, mut r) = (dry, dry);
        if let Some(rt60) = tail.filter(|_| cut.is_none_or(|cut| t < cut)) {
            let cutoff = (8000. * 2f64.powf(-t / 0.2)).max(300.);
            let pole = 1. - (-2. * PI * cutoff / RATE).exp();
            // Rolled off without getting quieter for it: the noise's power follows the cutoff.
            let gain = 0.15 * falling(t, rt60) * (8000. / cutoff).sqrt();
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
    let (wet, dry) = (hits(Some(1.2), None), hits(None, None));
    let (wet_fx, dry_fx) = (effects(&wet, None, None), effects(&dry, None, None));
    let time = wet_fx.decay_time.unwrap();
    assert!((time - 1.2).abs() < 0.25, "{time} s");
    assert!(dry_fx.decay_time.unwrap() < 0.35, "{dry_fx:?}");
    assert!(wet_fx.darkening.unwrap() > 0.5 && dry_fx.darkening.unwrap_or(0.).abs() < 0.3, "{wet_fx:?} against {dry_fx:?}");
    assert!(wet_fx.widening.unwrap() > 3. && dry_fx.widening.unwrap_or(0.) < 1., "{wet_fx:?} against {dry_fx:?}");
    let (wet_share, dry_share) = (tail_share(&wet).unwrap(), tail_share(&dry).unwrap());
    assert!(wet_share > dry_share + 10. && dry_share < 10., "{wet_share} % against {dry_share} %");
    // A tail stopped half a second in reads as cut; one that dies away doesn't.
    assert!(cut_tails(&hits(Some(1.2), Some(0.5))).len() >= 3);
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
    assert_eq!(pump(&hits(None, None), 120., Some(0.)), None);
    assert!(decay_time(&heard(&pad, &pad)).is_none_or(|time| time < 0.5));
}

#[test]
fn a_change_toward_a_wetter_reference_is_kept_and_one_past_its_punch_is_taken_back() {
    // The reference: the bursts with a 1.2 s tail. The sound: dry, then given a 0.6 s tail.
    let (reference, dry, wetter) = (hits(Some(1.2), None), hits(None, None), hits(Some(0.6), None));
    // A wetter reference with less punch: its reverb fills between the hits.
    let mut profile = Profile::of("wet", &reference);
    let punchy = dry.measures.crest.unwrap();
    profile.crest = Some(Spread { mid: punchy - 4., low: punchy - 5.5, high: punchy - 2.5 });
    let goal = Goal { reference: Some(profile), sound: true, ..Default::default() };
    let mut checklist = Checklist::new(&goal, &dry, &[]);
    let mut before = checklist.read(&dry, None);
    let dropped = checklist.drop_unreadable(&mut before);
    // A sample's level isn't the sound's: loudness is held where it was, not brought to the reference's.
    let loudness = checklist.items.iter().position(|item| item.quantity == Quantity::Integrated).expect("loudness held");
    assert_eq!((checklist.items[loudness].role, checklist.items[loudness].target), (Role::Guard, Target::Kept));
    let at = |id: &str| checklist.items.iter().position(|item| item.id == id).unwrap_or_else(|| panic!("no {id}: {dropped:?}"));
    let (decay_time, punch) = (at("decay time"), at("punch"));
    // How much brighter the hits are than their tails is the tail's darkness, not distortion: no such guard here.
    assert!(checklist.items.iter().all(|item| item.quantity != Quantity::Distortion), "{:?}", checklist.items);
    let mut after = checklist.read(&wetter, None);
    // A measure the tail hides this round (the hit's own decay to 20 dB under) is unreadable now, not lost.
    let hidden = at("decay");
    after[hidden] = None;
    let verdict = checklist.verdict(Some(decay_time), &before, &after);
    assert!(verdict.kept, "{verdict:#?}");
    assert_eq!(verdict.rows[hidden].change, Change::Same);
    // Punch falling toward the reference's isn't worse; past the reference's range, it is.
    after[punch] = before[punch].map(|crest| crest - 3.);
    let verdict = checklist.verdict(Some(decay_time), &before, &after);
    assert!(verdict.kept && verdict.rows[punch].change != Change::Worse, "{verdict:#?}");
    let Target::NoLowerThan { value: floor } = checklist.items[punch].target else { panic!("{:?}", checklist.items[punch]) };
    after[punch] = Some(floor - 2.);
    let verdict = checklist.verdict(Some(decay_time), &before, &after);
    assert!(!verdict.kept && verdict.hurt.contains(&"punch".to_string()), "{verdict:#?}");
    // Rounds are brought back to where the sound played.
    let mut louder = before.clone();
    louder[loudness] = before[loudness].map(|lufs| lufs + 3.);
    assert_eq!(checklist.rebalance(&before, &louder), Some(-3.));
}

#[test]
fn an_effect_only_one_side_has_reaches_the_checklist_and_a_number_asked_for_wins() {
    let blip = |t: f64| if (0. ..0.02).contains(&t) { (2. * PI * 1000. * t).sin() * (-t / 0.005).exp() } else { 0. };
    let echoed: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| (n as f64 / RATE) % 2.)
        .map(|t| (0..=5).map(|k| 0.5 * 10f64.powf(-6. * k as f64 / 20.) * blip(t - k as f64 * 0.375)).sum())
        .collect();
    let plain: Vec<f64> = (0..(8. * RATE) as usize).map(|n| 0.5 * blip((n as f64 / RATE) % 0.5)).collect();
    let (echoed, plain) = (heard(&echoed, &echoed), heard(&plain, &plain));
    assert_eq!(echo_falls(&plain), Some(NO_ECHO));
    // A dry reference: the echoes are asked to go.
    let goal = Goal { reference: Some(Profile::of("dry", &plain)), sound: true, ..Default::default() };
    let checklist = Checklist::new(&goal, &echoed, &[]);
    let values = checklist.read(&echoed, None);
    let fall = checklist.items.iter().position(|item| item.id == "echo fall").expect("an echo fall item");
    assert!(checklist.items[fall].gap(values[fall]) > 3., "{:?} reads {:?}", checklist.items[fall], values[fall]);
    // A delayed reference against the dry sound: no echoes is a fall of 60 dB, and the echo time stays on to aim at.
    let goal = Goal { reference: Some(Profile::of("echoed", &echoed)), sound: true, ..Default::default() };
    let mut checklist = Checklist::new(&goal, &plain, &[]);
    let mut values = checklist.read(&plain, None);
    checklist.drop_unreadable(&mut values);
    let fall = checklist.items.iter().position(|item| item.id == "echo fall").expect("an echo fall item");
    assert_eq!(values[fall], Some(NO_ECHO));
    let time = checklist.items.iter().position(|item| item.id == "echo time").expect("the echo time stays");
    assert_eq!(values[time], None);
    // What can be read is worked on first.
    assert!(checklist.next(&values).is_some_and(|next| next != time));
    // A number the producer asked for takes the reference's place, and says so.
    let asked = Target::Exactly { value: 250., within: 10. };
    let goal = Goal {
        reference: Some(Profile::of("echoed", &echoed)),
        sound: true,
        targets: vec![Explicit { measure: Quantity::EchoTime, target: asked }],
        ..Default::default()
    };
    let checklist = Checklist::new(&goal, &plain, &[]);
    let times: Vec<_> = checklist.items.iter().filter(|item| item.id == "echo time").collect();
    assert!(
        times.len() == 1
            && times[0].role == Role::Target
            && times[0].target == asked
            && times[0].label.contains("in place of the reference's"),
        "{times:?}"
    );
}

#[test]
fn guesses_arent_said_as_effects() {
    // One slope is the sound's own decay; two, the second slower, a reverb.
    let (wet, dry) = (hits(Some(1.2), None), hits(None, None));
    assert!(effects(&wet, None, None).reverb && !effects(&dry, None, None).reverb);
    // A long 808 is mostly tail by itself: no effect is burying it.
    let eight: Vec<f64> = (0..(6. * RATE) as usize)
        .map(|n| (n as f64 / RATE) % 1.)
        .map(|t| if t < 0.9 { 0.8 * (-t / 0.5).exp() * (2. * PI * (45. * t + 15. * 0.07 * (1. - (-t / 0.07).exp()))).sin() } else { 0. })
        .collect();
    let eight = heard(&eight, &eight);
    let found = effects::problems(&eight, &effects(&eight, Some(120.), None), Some(120.), &|seconds| format!("{seconds} s"));
    assert!(!found.iter().any(|problem| problem.contains("buries")), "{found:?}");
    // A held bass note let go with a 1 ms release isn't a tail cut off.
    let held: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| {
            let t = (n as f64 / RATE) % 1.;
            let gain = if t < 0.75 { 1. } else { (1. - (t - 0.75) / 0.001).max(0.) };
            0.5 * gain * (2. * PI * 55. * n as f64 / RATE).sin()
        })
        .collect();
    assert_eq!(cut_tails(&heard(&held, &held)), Vec::<f64>::new());
    // Plucked 8ths at 120 BPM with no LFO: their rhythm isn't a modulation, and they don't swing.
    let plucks: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| 0.5 * (-((n as f64 / RATE) % 0.25) / 0.05).exp() * (2. * PI * 330. * n as f64 / RATE).sin())
        .collect();
    let plucked = heard(&plucks, &plucks);
    assert_eq!((plucked.measures.modulation, swing(&plucked)), (None, Some(0.)));
    // A bass line an octave wide isn't a filter sweeping: its brightness is read against its notes.
    let mut phase = 0.;
    let line: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| {
            phase += 2. * PI * if (n as f64 / RATE / 0.5) as usize % 2 == 0 { 55. } else { 110. } / RATE;
            0.15 * (1..=10).map(|k| (k as f64 * phase).sin() / k as f64).sum::<f64>()
        })
        .collect();
    let (octaves, _) = sweep(&heard(&line, &line)).unwrap();
    assert!(octaves < 0.25, "{octaves} octaves");
}
