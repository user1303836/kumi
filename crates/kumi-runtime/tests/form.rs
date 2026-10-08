//! A song's form, heard: sections found where the energy curve turns, their roles and repeats, the turns between
//! them (a prepared drop and an unprepared one), the intro, outro and first hook; and a flat song's plateau.
use kumi_runtime::listening::{
    form::{compare, form, Form},
    measure::measure_samples,
};

const RATE: f64 = 48_000.;
/// Bars of two seconds (120 BPM in 4/4).
const BAR: f64 = 2.;

/// A seeded noise source: the same "random" every run.
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2. - 1.
    }
}

/// One stretch of a song: bars, its level at the start and end (dB of full scale), hits a beat, and how bright.
struct Part {
    bars: usize,
    from: f64,
    to: f64,
    hits: f64,
    bright: f64,
}

/// A song from its parts: a bed of filtered noise at the part's level (rising or falling across it) with short hits
/// on the beat grid, brighter for a brighter part.
fn song(parts: &[Part]) -> Vec<f64> {
    let mut noise = Noise(11);
    let mut low = 0.;
    let mut out = vec![];
    for part in parts {
        let samples = (part.bars as f64 * BAR * RATE) as usize;
        let beat = BAR / 4.;
        let every = (beat / part.hits.max(1e-6) * RATE) as usize;
        for n in 0..samples {
            let along = n as f64 / samples as f64;
            let gain = 10f64.powf((part.from + (part.to - part.from) * along) / 20.);
            let white = noise.next();
            low += 0.05 * (white - low);
            let bed = low * 4. * (1. - part.bright) + white * part.bright;
            let hit = if part.hits > 0. && every > 0 && n % every < 600 { (1. - (n % every) as f64 / 600.) * white * 3. } else { 0. };
            out.push((bed + hit) * gain);
        }
    }
    out
}

fn heard(samples: &[f64]) -> kumi_runtime::listening::measure::Heard {
    let samples: Vec<f32> = samples.iter().map(|s| *s as f32).collect();
    measure_samples(&samples, &samples, RATE)
}

fn edges(form: &Form) -> Vec<usize> {
    form.sections.iter().skip(1).map(|section| section.from).collect()
}

#[test]
fn a_song_falls_into_sections_with_their_roles_turns_and_problems() {
    let parts = [
        Part { bars: 8, from: -32., to: -32., hits: 1., bright: 0.2 },
        Part { bars: 8, from: -26., to: -16., hits: 2., bright: 0.4 },
        Part { bars: 16, from: -14., to: -14., hits: 4., bright: 0.6 },
        Part { bars: 8, from: -30., to: -30., hits: 0.5, bright: 0.2 },
        Part { bars: 16, from: -14., to: -14., hits: 4., bright: 0.6 },
        Part { bars: 8, from: -32., to: -32., hits: 1., bright: 0.2 },
    ];
    let form = form(&heard(&song(&parts)), BAR);
    assert_eq!(form.bars.len(), 64);
    let found = edges(&form);
    for expected in [8, 16, 32, 40, 56] {
        assert!(found.iter().any(|at| at.abs_diff(expected) <= 1), "a section starts near bar {}: {found:?}", expected + 1);
    }
    let roles: Vec<&str> = form.sections.iter().map(|section| section.role.as_str()).collect();
    assert_eq!((roles.first(), roles.last()), (Some(&"intro"), Some(&"outro")), "{roles:?}");
    let peaks: Vec<_> = form.sections.iter().filter(|section| section.role == "peak").collect();
    assert!(peaks.len() >= 2 && peaks.iter().all(|section| section.letter == peaks[0].letter), "the two drops repeat: {:?}", form.sections);
    assert_eq!((form.intro, form.outro), (8, 8));
    assert!(form.hook.is_some_and(|bar| bar.abs_diff(16) <= 1), "{:?}", form.hook);
    // The first drop comes out of a build; the second straight out of a quiet break: unprepared.
    let first = form.transitions.iter().find(|turn| turn.at.abs_diff(16) <= 1).unwrap();
    assert!(first.kind == "drop" && !first.prepared.is_empty(), "{first:?}");
    let second = form.transitions.iter().find(|turn| turn.at.abs_diff(40) <= 1).unwrap();
    assert_eq!((second.kind.as_str(), second.prepared.is_empty()), ("drop", true), "{second:?} in {:?}", form.sections);
    assert!(form.problems.iter().any(|problem| problem.contains("arrives unprepared")), "{:?}", form.problems);
    assert!(!form.problems.iter().any(|problem| problem.contains("plateaus")), "{:?}", form.problems);
}

#[test]
fn a_song_that_never_moves_plateaus_and_is_compared_with_one_that_does() {
    // One bar looped 48 times, unchanged.
    let bar = song(&[Part { bars: 1, from: -18., to: -18., hits: 2., bright: 0.4 }]);
    let looped: Vec<f64> = (0..48).flat_map(|_| bar.iter().copied()).collect();
    let flat = form(&heard(&looped), BAR);
    assert!(flat.problems.iter().any(|problem| problem.contains("plateaus for")), "{:?}", flat.problems);
    assert!(flat.problems.iter().any(|problem| problem.contains("barely change")), "{:?}", flat.problems);
    let shaped = form(
        &heard(&song(&[
            Part { bars: 16, from: -32., to: -32., hits: 1., bright: 0.2 },
            Part { bars: 16, from: -14., to: -14., hits: 4., bright: 0.6 },
            Part { bars: 16, from: -30., to: -30., hits: 1., bright: 0.2 },
        ])),
        BAR,
    );
    let said = compare(&flat, &shaped);
    assert!(said.iter().any(|line| line.starts_with("contrast between sections")), "{said:?}");
    assert!(said.iter().any(|line| line.starts_with("intro")), "{said:?}");
}

#[test]
fn silent_bars_dont_wipe_out_the_sections_around_them() {
    // Intro, verse, break, drop and outro at -20, -15, -21, -10 and -20 dB, told apart by their level alone.
    let part = |bars: usize, level: f64| Part { bars, from: level, to: level, hits: 2., bright: 0.4 };
    let clean = song(&[part(16, -20.), part(16, -15.), part(8, -21.), part(16, -10.), part(8, -20.)]);
    let bar = (BAR * RATE) as usize;
    let cuts = |samples: &[f64]| edges(&form(&heard(samples), BAR));
    let near = |found: &[usize], expected: &[usize]| {
        found.len() == expected.len() && expected.iter().all(|at| found.iter().any(|cut| cut.abs_diff(*at) <= 1))
    };
    // As it is; with one silent bar before the drop (the gap ends the break); starting a bar in, as an Arrangement can;
    // and with two silent bars at its end.
    let mut gap = clean.clone();
    gap[39 * bar..40 * bar].fill(0.);
    let found =
        [cuts(&clean), cuts(&gap), cuts(&[vec![0.; bar], clean.clone()].concat()), cuts(&[clean.clone(), vec![0.; 2 * bar]].concat())];
    let expected = [[16, 32, 40, 56], [16, 32, 40, 56], [17, 33, 41, 57], [16, 32, 40, 56]];
    assert!(found.iter().zip(&expected).all(|(found, expected)| near(found, expected)), "{found:?}");
}
