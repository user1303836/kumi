//! The listening loop's ears and judge on synthetic sound: what they measure, what they find and where, and how a
//! checklist decides whether a change stays.
use kumi_runtime::listening::{
    checklist::{Change, Checklist, Goal, Profile, Quantity, Target},
    detect::{self, ProblemKind},
    measure::{measure_samples, Heard},
};
use std::f64::consts::PI;

const RATE: f64 = 48_000.;

/// A seeded noise source: the same "random" every run.
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2. - 1.
    }
}

/// Pink-ish noise (Paul Kellet's filter), at roughly `db` dBFS RMS.
fn pink(seconds: f64, db: f64, seed: u64) -> Vec<f64> {
    let mut noise = Noise(seed);
    let mut b = [0f64; 7];
    let samples: Vec<f64> = (0..(seconds * RATE) as usize)
        .map(|_| {
            let white = noise.next();
            b[0] = 0.99886 * b[0] + white * 0.0555179;
            b[1] = 0.99332 * b[1] + white * 0.0750759;
            b[2] = 0.96900 * b[2] + white * 0.1538520;
            b[3] = 0.86650 * b[3] + white * 0.3104856;
            b[4] = 0.55000 * b[4] + white * 0.5329522;
            b[5] = -0.7616 * b[5] - white * 0.0168980;
            let out = b.iter().sum::<f64>() + white * 0.5362;
            b[6] = white * 0.115926;
            out
        })
        .collect();
    let rms = (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt();
    let gain = 10f64.powf(db / 20.) / rms;
    samples.into_iter().map(|s| s * gain).collect()
}

fn sine(seconds: f64, hz: f64, amplitude: f64) -> Vec<f64> {
    (0..(seconds * RATE) as usize).map(|n| amplitude * (2. * PI * hz * n as f64 / RATE).sin()).collect()
}

fn mixed(parts: &[&[f64]]) -> Vec<f64> {
    let length = parts.iter().map(|part| part.len()).max().unwrap_or(0);
    (0..length).map(|n| parts.iter().map(|part| part.get(n).copied().unwrap_or(0.)).sum()).collect()
}

fn heard(left: &[f64], right: &[f64]) -> Heard {
    let left: Vec<f32> = left.iter().map(|s| *s as f32).collect();
    let right: Vec<f32> = right.iter().map(|s| *s as f32).collect();
    measure_samples(&left, &right, RATE)
}

#[test]
fn loudness_and_peaks_read_as_meters_do() {
    // A 1 kHz sine at −20 dBFS in both channels reads −20 LUFS (BS.1770's calibration), and peaks at −20.
    let tone = sine(6., 1000., 0.1);
    let measured = heard(&tone, &tone).measures;
    assert!((measured.integrated.unwrap() + 20.).abs() < 0.2, "{:?}", measured.integrated);
    assert!((measured.true_peak + 20.).abs() < 0.1 && (measured.sample_peak + 20.).abs() < 0.1);
    assert!(measured.plr.unwrap().abs() < 0.3);
    assert_eq!(measured.clipped, 0);
    // All of it in the 1 kHz third-octave, none of it side.
    let at = measured.balance.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
    assert_eq!(kumi_runtime::listening::measure::THIRDS[at], 1000.);
    assert!(measured.width[at] < 0.01);
    // A true peak between samples: a sine at a quarter of the rate, phase-shifted, peaks over its samples.
    let between: Vec<f64> = (0..48_000).map(|n| 0.9 * (2. * PI * 12_000. * n as f64 / RATE + PI / 4.).sin()).collect();
    let measured = heard(&between, &between).measures;
    assert!(measured.true_peak > measured.sample_peak + 2., "{} over {}", measured.true_peak, measured.sample_peak);
}

#[test]
fn pink_noise_is_flat_and_a_bright_mix_tilts_up() {
    let noise = pink(6., -20., 1);
    let flat = heard(&noise, &noise).measures;
    assert!(flat.tilt.abs() < 0.8, "pink tilts {}", flat.tilt);
    let crest = flat.crest.unwrap();
    assert!((8. ..16.).contains(&crest), "crest {crest}");
    // A bright one: the same noise with a strong hiss on top.
    let mut hiss = Noise(2);
    let bright: Vec<f64> = noise.iter().map(|s| s + 0.3 * hiss.next()).collect();
    assert!(heard(&bright, &bright).measures.tilt > flat.tilt + 1.);
}

#[test]
fn harsh_hits_are_placed_in_time_and_a_steady_resonance_is_steady() {
    // Noise, with a 6.3 kHz whistle flaring four times for a fifth of a second.
    let base = pink(8., -24., 3);
    let flares: Vec<f64> = (0..base.len())
        .map(|n| {
            let t = n as f64 / RATE;
            let on = [1.0, 3.0, 5.0, 7.0].iter().any(|start| (*start..start + 0.2).contains(&t));
            if on {
                0.12 * (2. * PI * 6300. * t).sin()
            } else {
                0.
            }
        })
        .collect();
    let harsh = mixed(&[&base, &flares]);
    let problems = detect::harshness(&heard(&harsh, &harsh));
    let found = problems.iter().find(|p| p.kind == ProblemKind::Harshness).expect("harsh hits");
    assert_eq!(found.steady, Some(false), "{found:?}");
    let [low, high] = found.hz.unwrap();
    assert!(low < 6300. && high > 6300., "{found:?}");
    let starts: Vec<f64> = found.at.iter().map(|span| span[0]).collect();
    for flare in [1.0, 3.0, 5.0, 7.0] {
        assert!(starts.iter().any(|start| (start - flare).abs() < 0.2), "{flare} in {starts:?}");
    }
    // A steady 3 kHz ring through all of it.
    let ring = mixed(&[&base, &sine(8., 3150., 0.05)]);
    let problems = detect::harshness(&heard(&ring, &ring));
    let steady = problems.iter().find(|p| p.hz.is_some_and(|[low, high]| low < 3150. && high > 3150.)).expect("a ring at 3.15 kHz");
    assert_eq!(steady.steady, Some(true));
    assert!(steady.at.is_empty() && steady.fix.contains("static EQ"));
    // Plain noise has neither.
    assert!(detect::harshness(&heard(&base, &base)).is_empty());
}

#[test]
fn the_low_end_finds_stereo_lows_rumble_and_a_loud_note() {
    // A 60 Hz bass out of phase between the channels is all side.
    let bass = sine(6., 60., 0.3);
    let inverted: Vec<f64> = bass.iter().map(|s| -s).collect();
    let noise = pink(6., -30., 4);
    let wide = detect::low_end(&heard(&mixed(&[&bass, &noise]), &mixed(&[&inverted, &noise])));
    assert!(wide.iter().any(|p| p.kind == ProblemKind::StereoLows), "{wide:?}");
    let mono = detect::low_end(&heard(&mixed(&[&bass, &noise]), &mixed(&[&bass, &noise])));
    assert!(!mono.iter().any(|p| p.kind == ProblemKind::StereoLows));
    // A bassline of four notes, one of them 8 dB louder.
    let notes = [(55., 0.2), (65.4, 0.2), (73.4, 0.5), (82.4, 0.2)];
    let line: Vec<f64> = (0..(8. * RATE) as usize)
        .map(|n| {
            let t = n as f64 / RATE;
            let (hz, amplitude) = notes[(t / 0.5) as usize % 4];
            amplitude * (2. * PI * hz * t).sin()
        })
        .collect();
    let line = mixed(&[&line, &pink(8., -36., 5)]);
    let found = detect::low_end(&heard(&line, &line));
    let loud = found.iter().find(|p| p.kind == ProblemKind::LoudNote).expect("the loud note");
    assert!(loud.what.contains("D1") || loud.what.contains("73 Hz"), "{loud:?}");
    assert!(loud.excess >= 4.);
}

#[test]
fn masking_is_a_target_to_mask_ratio_against_the_rest() {
    // A 2 kHz "vocal" and, against it, the rest: loud noise in the same region, or the same noise far below it.
    let vocal = sine(6., 2000., 0.05);
    let mut noise = Noise(6);
    let mut low = [0f64; 2];
    let mut high = [0f64; 2];
    let rumble: Vec<f64> = (0..vocal.len())
        .map(|_| {
            let white = noise.next();
            low[0] += 0.02 * (white - low[0]);
            low[1] += 0.02 * (low[0] - low[1]);
            low[1] * 6.
        })
        .collect();
    let mut noise = Noise(7);
    let hiss_like: Vec<f64> = (0..vocal.len())
        .map(|_| {
            // Band-limited around 2 kHz: a crude resonator.
            let white = noise.next();
            let out = white + 1.94 * high[0] - 0.985 * high[1];
            high[1] = high[0];
            high[0] = out;
            out * 0.015
        })
        .collect();
    let buried_mix = mixed(&[&vocal, &hiss_like]);
    let clear_mix = mixed(&[&vocal, &rumble]);
    let target = heard(&vocal, &vocal);
    let buried = detect::masking(&target, &heard(&buried_mix, &buried_mix), "the vocal");
    assert!(buried.as_ref().is_some_and(|p| p.kind == ProblemKind::Masking && p.excess > 0.), "{buried:?}");
    assert!(detect::masking(&target, &heard(&clear_mix, &clear_mix), "the vocal").is_none());
}

#[test]
fn a_change_stays_only_when_its_target_improves_and_nothing_else_gets_audibly_worse() {
    let noise = pink(6., -18., 8);
    let first = heard(&noise, &noise);
    let reference = Profile::of("reference", &first);
    // Pink noise at −18 dBFS RMS reads about −15.4 LUFS: −10 is far off.
    let goal = Goal { loudness: Some(-10.), true_peak: Some(-1.), reference: Some(reference), problems: true, ..Default::default() };
    let checklist = Checklist::new(&goal, &first, &[]);
    let at = |id: &str| checklist.items.iter().position(|item| item.id == id).unwrap();
    let loudness = at("loudness");
    assert_eq!(checklist.items[loudness].target, Target::Exactly { value: -10., within: 0.5 });
    assert!(checklist.items.iter().any(|item| item.quantity == Quantity::Region { region: 2 }));
    let before = checklist.read(&first, None);
    // Loudness is the biggest gap.
    assert_eq!(checklist.next(&before), Some(loudness));
    // Louder by 3 dB, nothing else moved: kept.
    let mut louder = before.clone();
    louder[loudness] = louder[loudness].map(|v| v + 3.);
    let verdict = checklist.verdict(Some(loudness), &before, &louder);
    assert!(verdict.kept, "{verdict:?}");
    assert_eq!(verdict.rows[loudness].change, Change::Better);
    // Louder, but the punch went with it, further than the loudness moved: taken back, and the verdict says why.
    let punch = at("punch");
    let mut squashed = louder.clone();
    squashed[punch] = squashed[punch].map(|v| v - 4.5);
    let verdict = checklist.verdict(Some(loudness), &before, &squashed);
    assert!(!verdict.kept && verdict.hurt == ["punch"] && verdict.why.contains("punch"), "{verdict:?}");
    // Bringing loudness up 3 dB may cost punch about as much: a 2 dB drop is allowed then, a 5 dB one isn't.
    let mut pressed = louder.clone();
    pressed[punch] = pressed[punch].map(|v| v - 2.);
    assert!(checklist.verdict(Some(loudness), &before, &pressed).kept);
    pressed[punch] = pressed[punch].map(|v| v - 3.);
    assert!(!checklist.verdict(Some(loudness), &before, &pressed).kept);
    // A change that didn't move its target isn't kept either.
    assert!(!checklist.verdict(Some(loudness), &before, &before).kept);
    // After a kept change that left loudness off target, rebalancing brings it back.
    assert_eq!(checklist.rebalance(&before, &louder), Some(round1(-10. - louder[loudness].unwrap())));
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

/// A listening model that answers by the order it hears things in: it always says the first take is closer (position
/// bias), and hears "distorted" in whichever take is the change.
struct Biased {
    asked: std::cell::RefCell<Vec<usize>>,
}
#[async_trait::async_trait(?Send)]
impl kumi_runtime::listening::listener::Listener for Biased {
    fn name(&self) -> String {
        "biased".into()
    }
    async fn ask(&self, wav: &[u8], _: &str, _: kumi_common::abort::Signal) -> Result<kumi_runtime::listening::listener::Answer, String> {
        self.asked.borrow_mut().push(wav.len());
        let first_is_change = self.asked.borrow().len() == 2;
        let (first, second) = if first_is_change { (vec!["distorted".into()], vec![]) } else { (vec![], vec!["distorted".into()]) };
        Ok(kumi_runtime::listening::listener::Answer { closer: "first".into(), first, second })
    }
}

#[tokio::test]
async fn the_listening_model_counts_only_what_holds_both_ways() {
    use kumi_runtime::listening::listener::{compare, parse_answer, Choice, Take};
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("take.wav");
    let tone: Vec<f32> = sine(3., 440., 0.2).iter().map(|s| *s as f32).collect();
    let capture = kumi_runtime::ears::capture::Capture {
        left: tone.clone(),
        right: tone,
        sync: vec![1.5; 144_000],
        position: None,
        sample_rate: RATE,
        raw: None,
    };
    kumi_runtime::ears::capture::write_capture_wav(&file, &capture, 0., 144_000.).await.unwrap();
    let take = Take { file: file.clone(), start: 0.5, seconds: 2., gain: 0. };
    let biased = Biased { asked: Default::default() };
    let opinion =
        compare(&biased, &take, &Take { gain: -3., ..take.clone() }, "less harsh", kumi_common::abort::Signal::new()).await.unwrap();
    // "The first is closer" both times means it changed its mind with the order: no preference counts.
    assert_eq!(opinion.closer, None);
    // It heard the change distorted both ways round, and the take before never.
    assert_eq!(opinion.new_problems, ["distorted"]);
    assert_eq!(biased.asked.borrow().len(), 2);
    // Two 2 s takes and a 0.8 s pause, as 16-bit mono at 48 kHz.
    assert_eq!(biased.asked.borrow()[0], 44 + 2 * (2 * 2 * 48_000 + 38_400));
    assert!(opinion.line("biased").contains("isn't sure") && opinion.line("biased").contains("distorted"));
    // Its answer is read out of whatever it wraps the JSON in.
    let answer = parse_answer("Sure! ```json\n{\"closer\": \"second\", \"first\": [\"muddy\"], \"second\": []}\n```").unwrap();
    assert_eq!((answer.closer.as_str(), answer.first.len()), ("second", 1));
    let _ = Choice::After;
}
