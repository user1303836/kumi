//! A part's feel measured on its notes: swing fitted on the right grid, each lane's timing, velocity and where it
//! plays, and the notes moved toward a reference's feel closing the gaps.
use kumi_runtime::listening::notes::{feel, fit_swing, gaps, toward, Note};

/// A two-bar drum pattern: kick on 1 and 3, snare on 2 and 4, hats on every sixteenth (or eighth) with `swing` (50 is
/// straight) on the grid's off-beats, and the hats' accents.
fn pattern(bars: usize, hats_grid: u32, swing: f64, hat_late_ms: f64, tempo: f64) -> Vec<Note> {
    let ms = 60_000. / tempo;
    let mut notes = vec![];
    for bar in 0..bars {
        let at = bar as f64 * 4.;
        for beat in [0., 2.] {
            notes.push(Note { start: at + beat, length: 0.25, pitch: 36, velocity: 110. });
        }
        for beat in [1., 3.] {
            notes.push(Note { start: at + beat, length: 0.25, pitch: 38, velocity: 100. });
        }
        let unit = 4. / hats_grid as f64;
        for step in 0..hats_grid as usize {
            let straight = at + step as f64 * unit;
            let swung = if step % 2 == 1 { straight - unit + 2. * unit * swing / 100. } else { straight };
            let velocity = if step % 2 == 0 { 96. } else { 64. };
            notes.push(Note { start: swung + hat_late_ms / ms, length: 0.1, pitch: 42, velocity });
        }
    }
    notes
}

#[test]
fn swing_is_fitted_on_the_grid_the_part_plays() {
    let straight: Vec<f64> = pattern(2, 16, 50., 0., 90.).iter().map(|note| note.start).collect();
    assert_eq!(fit_swing(&straight), (16, 50.));
    let sixteenths: Vec<f64> = pattern(2, 16, 62., 0., 90.).iter().map(|note| note.start).collect();
    assert_eq!(fit_swing(&sixteenths), (16, 62.));
    let eighths: Vec<f64> = pattern(2, 8, 66., 0., 90.).iter().map(|note| note.start).collect();
    assert_eq!(fit_swing(&eighths), (8, 66.));
}

#[test]
fn lanes_keep_their_own_timing_velocity_and_where_they_play() {
    // Hats laid back 12 ms behind a straight kick and snare.
    let notes = pattern(4, 16, 50., 12., 90.);
    let measured = feel(&notes, 90., 4.);
    assert_eq!((measured.steps, measured.bars, measured.grid), (16, 4, 16));
    let lane = |name: &str| measured.lanes.iter().find(|lane| lane.name == name).unwrap();
    assert_eq!(lane("kick").density[0], 1.);
    assert_eq!(lane("kick").density[4], 0.);
    assert_eq!(lane("snare").offset[4], Some(0.));
    assert!((lane("hats").offset[1].unwrap() - 12.).abs() < 0.2, "{:?}", lane("hats").offset);
    assert_eq!((lane("hats").velocity[0], lane("hats").velocity[1]), (Some(96.), Some(64.)));
    assert!(measured.push > 5., "{}", measured.push);
}

#[test]
fn moving_a_part_toward_a_reference_closes_its_gaps() {
    let tempo = 92.;
    // The reference swings its sixteenths and lays its hats back; the part is straight on the grid.
    let reference = feel(&pattern(4, 16, 60., 10., tempo), tempo, 4.);
    let notes = pattern(4, 16, 50., 0., tempo);
    let part = feel(&notes, tempo, 4.);
    let before = gaps(&part, &reference);
    assert!(before.swing >= 9., "{before:?}");
    let hats = |gaps: &kumi_runtime::listening::notes::Gaps| gaps.timing.iter().find(|(name, _)| name == "hats").unwrap().1;
    assert!(hats(&before) > 5., "{before:?}");
    // Halfway, then all the way: each closes more.
    let half = feel(&toward(&notes, &part, &reference, 4., 0.5), tempo, 4.);
    let full = feel(&toward(&notes, &part, &reference, 4., 1.), tempo, 4.);
    let (half, full) = (gaps(&half, &reference), gaps(&full, &reference));
    assert!(hats(&half) < hats(&before) && hats(&full) < 1., "{} → {} → {}", hats(&before), hats(&half), hats(&full));
    assert!(full.swing <= 1., "{full:?}");
}

#[test]
fn syncopation_and_fills_read_off_the_notes() {
    // Off-beat sixteenths with nothing after them: syncopated; a busier fourth bar: a fill.
    let mut notes = vec![];
    for bar in 0..8 {
        let at = bar as f64 * 4.;
        notes.push(Note { start: at, length: 0.25, pitch: 36, velocity: 100. });
        notes.push(Note { start: at + 1.75, length: 0.25, pitch: 38, velocity: 90. });
        if bar % 4 == 3 {
            for step in 8..16 {
                notes.push(Note { start: at + step as f64 * 0.25, length: 0.2, pitch: 38, velocity: 80. });
            }
        }
    }
    let measured = feel(&notes, 100., 4.);
    assert!(measured.syncopation > 0.1, "{}", measured.syncopation);
    assert!(measured.fills.unwrap() > 2., "{:?}", measured.fills);
}

#[test]
fn the_groove_checklist_counts_steps_off_each_tolerance() {
    use kumi_runtime::listening::notes::lines;
    let tempo = 92.;
    let reference = feel(&pattern(4, 16, 60., 10., tempo), tempo, 4.);
    let part = feel(&pattern(4, 16, 50., 0., tempo), tempo, 4.);
    let checklist = lines(&gaps(&part, &reference));
    let swing = checklist.iter().find(|line| line.id == "swing").unwrap();
    // 10 points of swing apart, 2 allowed, 1 a step: 8 steps off.
    assert!((swing.off() - 8.).abs() < 1.1, "{swing:?}");
    assert!(checklist.iter().any(|line| line.id == "timing hats" && line.off() > 0.));
    // The same feel is within every tolerance.
    assert!(lines(&gaps(&reference, &reference)).iter().all(|line| line.off() == 0.));
}
