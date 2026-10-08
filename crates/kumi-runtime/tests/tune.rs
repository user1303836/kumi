//! Code picking numbers: homing that never runs out of room, a searched candidate that goes silent or hurts something
//! never winning, an EQ fit that holds the regions already in tolerance, a search as short as Kumi runs it, and knob
//! text read the way Live writes it, and a knob probed before starting where its saved response says.
use kumi_runtime::listening::{
    checklist::{Checklist, Item, Quantity, Role, Target, REGIONS},
    cmaes::Cmaes,
    fit::{fit, total, Limits},
    home::{Homed, Homing},
    judging::candidate_cost,
    knobs::{Scale, Unit},
    probes::Probes,
};

const RATE: f64 = 48_000.;

fn item(id: &str, quantity: Quantity, target: Target, role: Role, jnd: f64) -> Item {
    Item { id: id.into(), label: id.into(), role, unit: String::new(), quantity, target, jnd, fix: None }
}

#[test]
fn homing_hurt_where_it_started_rules_nothing_out_and_never_runs_out_of_room() {
    // Nothing known where the knob is: the first probe is there, and it already hurts (the device just put in did).
    let mut homing = Homing::new(-9., (-9.4, -8.6), (0., 24.), 0., None, 4);
    assert_eq!(homing.next(Some(1.)), Ok(0.));
    homing.hurt(0., -14.);
    assert_eq!((homing.low, homing.high), (0., 24.), "a hurt where it started bounds nothing");
    let at = homing.next(Some(1.)).unwrap();
    assert!(at > 0., "{at}");
    homing.hurt(at, -14. + at);
    assert!(homing.high < at);
    // Hurt on both sides, closer than the knob's resolution: no room left reads as stuck, never a panic.
    let mut cornered = Homing::new(-9., (-9.4, -8.6), (0., 24.), 12., Some(-14.), 6).resolution(0.5);
    cornered.hurt(12.6, -13.);
    cornered.hurt(11.4, -15.);
    for _ in 0..4 {
        match cornered.next(Some(1.)) {
            Ok(at) => cornered.hurt(at, -14.),
            Err(why) => {
                assert!(matches!(why, Homed::Stuck | Homed::Spent), "{why:?}");
                break;
            }
        }
    }
    assert!(cornered.low <= cornered.high);
}

/// A true-peak ceiling to work toward, with loudness held and clipping guarded.
fn peaks() -> Checklist {
    Checklist {
        items: vec![
            item("loudness", Quantity::Integrated, Target::Exactly { value: -14., within: 0.5 }, Role::Target, 1.),
            item("true peak", Quantity::TruePeak, Target::AtMost { value: -1. }, Role::Target, 0.2),
            item("clipping", Quantity::Clipped, Target::AtMost { value: 0. }, Role::Guard, 1.),
        ],
    }
}

#[test]
fn a_searched_candidate_that_goes_silent_or_hurts_something_never_wins() {
    let checklist = peaks();
    let whole = vec![Some(-14.), Some(0.), Some(0.)];
    let start = [0.5, 0.5];
    let standing = candidate_cost(&checklist, 1, &whole, &whole, &whole, &start, &start);
    assert!((standing - 5.).abs() < 1e-9, "1 dB over a ceiling with 0.2 dB steps: {standing}");
    // Peaks down to the ceiling, nothing else moved: better than standing still.
    let better = candidate_cost(&checklist, 1, &whole, &whole, &[Some(-14.), Some(-1.2), Some(0.)], &[0.6, 0.5], &start);
    assert!(better < standing, "{better} vs {standing}");
    // Silence reads no peaks: it isn't on target, it's the worst there is.
    assert_eq!(candidate_cost(&checklist, 1, &whole, &whole, &[None, None, Some(0.)], &[0.9, 0.9], &start), f64::INFINITY);
    // Peaks fixed by clipping instead: worse than standing still.
    let clipped = candidate_cost(&checklist, 1, &whole, &whole, &[Some(-14.), Some(-1.5), Some(40.)], &[0.6, 0.5], &start);
    assert!(clipped > standing, "{clipped} vs {standing}");
    // The copies aren't heard by the learned models: a style guard with no reading on one is unheard, not lost.
    let mut guarded = peaks();
    guarded.items.push(item("vibe", Quantity::Vibe { to: vec![1., 0.] }, Target::NoHigher, Role::Guard, 0.03));
    let whole = vec![Some(-14.), Some(0.), Some(0.), Some(0.1)];
    let copy = candidate_cost(&guarded, 1, &whole, &whole, &[Some(-14.), Some(-1.2), Some(0.), None], &[0.6, 0.5], &start);
    assert!(copy.is_finite() && copy < standing, "{copy}");
}

#[test]
fn an_eq_fit_holds_the_regions_already_in_tolerance() {
    // The bass and the upper mids 3 dB under the reference; everything else within ±1 dB of it, near the top.
    let checklist = Checklist {
        items: (0..REGIONS.len())
            .map(|region| item(REGIONS[region].0, Quantity::Region { region }, Target::Between { low: -1., high: 1. }, Role::Reference, 1.))
            .collect(),
    };
    let values = [0.8, -3., 0.8, 0.8, -3., 0.8, 0.8, 0.8];
    let whole: Vec<Option<f64>> = values.iter().map(|value| Some(*value)).collect();
    let points = checklist.region_points(&whole);
    assert_eq!(points.len(), REGIONS.len(), "every region is a point");
    assert!(points.iter().zip(&values).all(|((_, gap, _), value)| (*gap == 0.) == (*value > -1.)), "{points:?}");
    let limits = Limits { db: 6., q: (0.3, 4.), hz: (20., 20_000.), bands: 4, within: 0.5 };
    let after = |points: &[(f64, f64, f64)]| -> Vec<f64> {
        let bands = fit(points, &limits, RATE);
        checklist.region_points(&whole).iter().zip(&values).map(|((hz, _, _), value)| value + total(&bands, *hz, RATE)).collect()
    };
    // Fitted to every region, each ends within its tolerance.
    let held = after(&points);
    assert!(held.iter().all(|value| (-1.05..=1.05).contains(value)), "{held:?}");
    // Fitted to the open gaps alone, one wide band lifts the mids between them far out of it.
    let open: Vec<(f64, f64, f64)> = points.iter().copied().filter(|(_, gap, _)| *gap != 0.).collect();
    assert!(after(&open).iter().any(|value| *value > 2.), "{:?}", after(&open));
}

#[test]
fn a_search_as_short_as_kumi_runs_it_still_gets_somewhere() {
    // Kumi's own setting: σ 0.2 of each knob's range and four generations, on three knobs that interact.
    let cost = |x: &[f64]| {
        let (a, b, c) = (x[0] - 0.7, x[1] - 0.25, x[2] - 0.6);
        (a + b).powi(2) * 4. + (a - b).powi(2) + c * c * 2.
    };
    let start = cost(&[0.5, 0.5, 0.5]);
    let mut gains = vec![];
    for seed in [3u64, 17, 29, 41, 53] {
        let mut state = seed;
        let random = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut search = Cmaes::new(&[0.5, 0.5, 0.5], 0.2, None, random);
        for _ in 0..4 {
            let points = search.ask();
            let costs: Vec<f64> = points.iter().map(|point| cost(point)).collect();
            search.tell(&points, &costs);
        }
        gains.push(search.best.clone().unwrap().1 / start);
    }
    gains.sort_by(f64::total_cmp);
    // Four generations of seven don't converge, but most seeds at least halve the cost.
    assert!(gains[2] < 0.5, "{gains:?}");
}

#[test]
fn knob_text_reads_signs_decimal_commas_factors_and_quiet_ends() {
    let grid = |texts: &[&str]| -> Vec<(f64, String)> {
        texts.iter().enumerate().map(|(at, text)| (at as f64 / (texts.len() - 1) as f64, text.to_string())).collect()
    };
    let pitch = Scale::read(&grid(&["-48 st", "-24 st", "0 st", "+24 st", "+48 st"])).unwrap();
    assert_eq!((pitch.unit, pitch.range()), (Unit::Semitones, (-48., 48.)));
    let comma = Scale::read(&grid(&["20,0 Hz", "200 Hz", "1,5 kHz", "8,00 kHz", "20,0 kHz"])).unwrap();
    assert_eq!(comma.range(), (20., 20_000.));
    assert!((comma.shown(0.5) - 1500.).abs() < 1., "{}", comma.shown(0.5));
    // A Q is a factor: searched in octaves of itself, a quarter of one a step.
    let q = Scale::read(&grid(&["0.10", "0.50", "1.00", "4.00", "18.0"])).unwrap();
    assert!((q.perceptual(1.) - q.perceptual(0.5) - 1.).abs() < 1e-9 && q.step() == 0.25);
    // A gain that reads below the floor stands at it rather than spoiling the scale.
    let gain = Scale::read(&grid(&["-inf dB", "-90.0 dB", "-70.0 dB", "-40.0 dB", "0.00 dB", "6.00 dB"])).unwrap();
    assert_eq!(gain.range(), (-70., 6.));
}

#[test]
fn a_device_and_a_track_are_compared_by_their_long_refs() {
    use kumi_runtime::integrations::ableton::{mutations::same_track, references::References};
    let mut book = References::default();
    let track = book.short_ref("7:track:3");
    let other = book.short_ref("7:track:4");
    let device = book.short_ref("7:device:3:0");
    // A short ref is a counter ("track:1" is the first one handed out), so it's read long first.
    assert_eq!((track.as_str(), device.as_str()), ("track:1", "device:1"));
    assert!(same_track(&book, &device, &track));
    assert!(!same_track(&book, &device, &other));
    assert!(!same_track(&book, &device, "track:9"));
}

#[test]
fn a_true_peak_ceiling_is_homed_in_finer_than_a_decibel() {
    // A limiter's ceiling at −0.5 dB lets peaks reach −0.3 dBTP; the aim is −1.2. The next probe is 0.9 dB away: under
    // the knob's own 1 dB step, but over a true peak's 0.2 dB one, so it's heard.
    let homing = Homing::new(-1.2, (-1.6, -1.), (-24., 0.), -0.5, Some(-0.3), 4).resolution(0.2);
    let at = homing.next(Some(1.)).unwrap();
    assert!((at + 1.4).abs() < 1e-9, "{at}");
    let coarse = Homing::new(-1.2, (-1.6, -1.), (-24., 0.), -0.5, Some(-0.3), 4).resolution(1.);
    assert_eq!(coarse.next(Some(1.)), Err(Homed::Stuck));
}

#[test]
fn a_knob_probed_before_starts_where_its_saved_response_says() {
    let dir = std::env::temp_dir().join(format!("kumi-probes-{}", std::process::id()));
    let probes = Probes::at(&dir);
    // A reverb's decay knob (in octaves of time) against the decay time it gave, heard across its range.
    let heard = [(-1., 0.4), (0., 0.8), (1., 1.5), (2., 2.9), (3., 5.5)];
    probes.add("Reverb", "Decay Time", "decay time", "Pad", &heard, None).unwrap();
    assert_eq!(probes.load("Reverb", "Size", "decay time", "Pad"), None);
    assert_eq!(probes.load("Reverb", "Decay Time", "tail share", "Pad"), None);
    // Heard on the pad: not what the knob does to the drums.
    assert_eq!(probes.load("Reverb", "Decay Time", "decay time", "Drums"), None);
    let saved = probes.load("Reverb", "Decay Time", "decay time", "Pad").unwrap();
    // It reads 0.9 s where it is (0.1 s over the saved curve): 2.0 s is a curve's 1.9 s, two sevenths past its 1.5.
    let guess = saved.predict((0., 0.9), 2.).unwrap();
    assert!((guess - (1. + 0.4 / 1.4)).abs() < 1e-9, "{guess}");
    let homing = Homing::new(2., (1.9, 2.1), (-1., 4.), 0., Some(0.9), 4).guess(guess);
    assert_eq!(homing.next(None), Ok(guess));
    // Beyond what the knob ever reached: the end that came closest.
    assert_eq!(saved.predict((0., 0.8), 9.), Some(3.));
    // Heard again near a setting: the newer reading replaces it.
    probes.add("Reverb", "Decay Time", "decay time", "Pad", &[(1.02, 1.6)], Some(0.1)).unwrap();
    let points = probes.load("Reverb", "Decay Time", "decay time", "Pad").unwrap().points;
    assert_eq!(points.len(), 5);
    assert!(points.contains(&(1.02, 1.6)) && !points.contains(&(1., 1.5)));
    // Probed again across its range: what was heard is the response now.
    probes.add("Reverb", "Decay Time", "decay time", "Pad", &[(0., 1.), (2., 3.)], None).unwrap();
    assert_eq!(probes.load("Reverb", "Decay Time", "decay time", "Pad").unwrap().points, vec![(0., 1.), (2., 3.)]);
    // A response unheard for over a month isn't used.
    let file = dir.join("reverb.json");
    let aged = std::fs::read_to_string(&file).unwrap();
    let at = aged.split("\"at\": ").nth(1).unwrap().split(',').next().unwrap().trim().to_string();
    std::fs::write(&file, aged.replace(&format!("\"at\": {at}"), "\"at\": 1000")).unwrap();
    assert_eq!(probes.load("Reverb", "Decay Time", "decay time", "Pad"), None);
    let _ = std::fs::remove_dir_all(&dir);
}
