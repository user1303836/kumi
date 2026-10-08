//! A part's feel measured on its notes: swing fitted on the right grid, each lane's timing, velocity and where it
//! plays (lanes by a Drum Rack's pads, one lane for anything else), the notes moved toward a reference's feel closing
//! the gaps, and a recording's grid fitted to its notes.
use kumi_runtime::listening::notes::{feel, fit_grid, fit_swing, gaps, lines, pad_lane, toward, Feel, Kit, Note};

/// A Drum Rack with a kick, a snare and closed hats where Live's Drums to MIDI writes them.
fn kit() -> Kit {
    Kit::rack(&[("Kick".into(), 36), ("Snare".into(), 38), ("Hihat Closed".into(), 42)])
}

fn drums(notes: &[Note], tempo: f64) -> Feel {
    feel(notes, tempo, 4., &kit())
}

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
    let measured = drums(&notes, 90.);
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
    let reference = drums(&pattern(4, 16, 60., 10., tempo), tempo);
    let notes = pattern(4, 16, 50., 0., tempo);
    let part = drums(&notes, tempo);
    let before = gaps(&part, &reference);
    assert!(before.swing >= 9., "{before:?}");
    let hats = |gaps: &kumi_runtime::listening::notes::Gaps| gaps.timing.iter().find(|(name, _)| name == "hats").unwrap().1;
    assert!(hats(&before) > 5., "{before:?}");
    // Halfway, then all the way: each closes more.
    let half = drums(&toward(&notes, &part, &reference, 4., 0.5), tempo);
    let full = drums(&toward(&notes, &part, &reference, 4., 1.), tempo);
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
    let measured = drums(&notes, 100.);
    assert!(measured.syncopation > 0.1, "{}", measured.syncopation);
    assert!(measured.fills.unwrap() > 2., "{:?}", measured.fills);
}

#[test]
fn the_groove_checklist_counts_steps_off_each_tolerance() {
    let tempo = 92.;
    let reference = drums(&pattern(4, 16, 60., 10., tempo), tempo);
    let part = drums(&pattern(4, 16, 50., 0., tempo), tempo);
    let checklist = lines(&gaps(&part, &reference));
    let swing = checklist.iter().find(|line| line.id == "swing").unwrap();
    // 10 points of swing apart, 2 allowed, 1 a step: 8 steps off.
    assert!((swing.off() - 8.).abs() < 1.1, "{swing:?}");
    assert!(checklist.iter().any(|line| line.id == "timing hats" && line.off() > 0.));
    // The same feel is within every tolerance.
    assert!(lines(&gaps(&reference, &reference)).iter().all(|line| line.off() == 0.));
}

#[test]
fn swung_eighths_alone_read_as_swing_not_push() {
    // Eighth-note hats alone, swung 58 %: as many swung off-beats as beats, and none of it is push.
    let tempo = 120.;
    let hats: Vec<Note> = pattern(4, 8, 58., 0., tempo).into_iter().filter(|note| note.pitch == 42).collect();
    let alone = drums(&hats, tempo);
    assert_eq!((alone.grid, alone.swing), (8, 58.), "{alone:?}");
    assert!(alone.push.abs() < 1., "{}", alone.push);
    // Against a full kit with the same hats, their swing and timing aren't apart.
    let kit = drums(&pattern(4, 8, 58., 0., tempo), tempo);
    let apart = gaps(&alone, &kit);
    assert!(apart.swing < 1., "{apart:?}");
    assert!(apart.timing.iter().all(|(_, gap)| *gap < 1.), "{apart:?}");
    // Sixteenths swung 58 % aren't eighths swung 58 %: each one's swing counts as apart.
    let sixteenths = drums(&pattern(4, 16, 58., 0., tempo), tempo);
    assert_eq!((sixteenths.grid, sixteenths.swing), (16, 58.));
    assert!((gaps(&sixteenths, &kit).swing - 16.).abs() < 1.1, "{:?}", gaps(&sixteenths, &kit));
}

#[test]
fn moving_notes_places_them_as_measuring_does_and_keeps_each_ones_lean() {
    let tempo = 120.;
    let ms = 60_000. / tempo;
    // Straight hats, one of them 10 ms early for bar 2's downbeat, and a roll of two sixteenth triplets around a hat in
    // bar 1; the reference lays its hats back 10 ms.
    let mut notes: Vec<Note> =
        pattern(2, 16, 50., 0., tempo).into_iter().filter(|note| (note.start - 4.).abs() > 1e-9 || note.pitch != 42).collect();
    notes.push(Note { start: 4. - 10. / ms, length: 0.1, pitch: 42, velocity: 96. });
    notes.extend([2. + 1. / 6., 2. + 1. / 3.].map(|start| Note { start, length: 0.05, pitch: 42, velocity: 64. }));
    let reference = drums(&pattern(2, 16, 50., 10., tempo), tempo);
    let part = drums(&notes, tempo);
    let moved = toward(&notes, &part, &reference, 4., 1.);
    // The early hat is bar 2's first, as measuring has it, not bar 1's last: it moves toward 4, not a sixteenth early.
    let early = moved[notes.len() - 3].start;
    assert!((early - 4.).abs() < 0.04, "{early}");
    // The roll moves as one: its notes keep their spacing.
    let (first, second) = (moved[notes.len() - 2].start, moved[notes.len() - 1].start);
    assert!((second - first - 1. / 6.).abs() < 1e-9, "{first} {second}");
    // Done, the hats sit 10 ms back as the reference's do.
    let hats = |feel: &Feel| gaps(feel, &reference).timing.iter().find(|(name, _)| name == "hats").unwrap().1;
    assert!(hats(&drums(&moved, tempo)) < 1., "{}", hats(&drums(&moved, tempo)));
}

#[test]
fn lanes_come_from_a_drum_racks_pads_and_a_pitched_part_is_one_lane() {
    // A bass on 36, 39, 43 and 46 isn't a kit, wherever its notes fall on the drum map: one lane.
    let bass: Vec<Note> = [36, 39, 43, 46]
        .iter()
        .enumerate()
        .map(|(at, pitch)| Note { start: at as f64, length: 0.5, pitch: *pitch, velocity: 100. })
        .collect();
    let pitched = feel(&bass, 120., 4., &Kit::pitched());
    assert_eq!(pitched.lanes.iter().map(|lane| lane.name.as_str()).collect::<Vec<_>>(), ["notes"]);
    // A Drum Rack's hats on D#1 (39) are hats, by their pad's name; a pad its name says nothing of goes by the drum map.
    let rack = Kit::rack(&[("BD Punchy".into(), 36), ("Hihat".into(), 39), ("Pad 7".into(), 38)]);
    assert_eq!((rack.lane(36), rack.lane(39), rack.lane(38), rack.lane(50)), ("kick", "hats", "snare", "perc"));
    assert_eq!(
        (pad_lane("Open Hat 808", 46), pad_lane("Clap", 40), pad_lane("Ride", 51), pad_lane("Conga Hi", 63)),
        ("hats", "snare", "cymbals", "perc")
    );
}

#[test]
fn where_a_lane_plays_counts_the_bars_the_part_plays() {
    // A song's drum stem: the beat in bars 1–4 and 9–12, nothing between, against a four-bar loop of the same beat.
    let tempo = 120.;
    let mut notes = pattern(4, 16, 50., 0., tempo);
    notes.extend(pattern(4, 16, 50., 0., tempo).into_iter().map(|note| Note { start: note.start + 32., ..note }));
    let song = drums(&notes, tempo);
    let kick = song.lanes.iter().find(|lane| lane.name == "kick").unwrap();
    assert_eq!((kick.density[0], kick.density[8]), (1., 1.), "{kick:?}");
    let apart = gaps(&drums(&pattern(4, 16, 50., 0., tempo), tempo), &song);
    assert!(apart.density.iter().all(|(_, gap)| *gap == 0.), "{apart:?}");
}

#[test]
fn a_transcriptions_velocities_are_neither_compared_nor_copied() {
    let tempo = 120.;
    let reference = drums(&pattern(4, 16, 50., 10., tempo), tempo).without_velocities();
    let notes: Vec<Note> = pattern(4, 16, 50., 0., tempo).into_iter().map(|note| Note { velocity: 100., ..note }).collect();
    let part = drums(&notes, tempo);
    assert!(lines(&gaps(&part, &reference)).iter().all(|line| !line.id.starts_with("velocity")));
    assert!(toward(&notes, &part, &reference, 4., 1.).iter().all(|note| note.velocity == 100.));
}

#[test]
fn a_recordings_grid_is_fitted_to_its_notes() {
    // A bassline at 123.4 BPM whose first downbeat is 150 ms in, on and off the beat for a minute, its onsets a few ms
    // off the grid as a transcription's are, its downbeats the strongest; the estimate is 0.3 % off.
    let (tempo, lead) = (123.4, 0.15);
    let sixteenth = 15. / tempo;
    let mut onsets = vec![];
    for bar in 0..60 {
        for (index, step) in [0, 3, 6, 8, 10, 14].iter().enumerate() {
            let jitter = ((bar * 7 + index * 3) % 5) as f64 * 0.002 - 0.004;
            let strength = match step {
                0 => 1.,
                8 => 0.8,
                _ => 0.6,
            };
            onsets.push((lead + (bar * 16 + step) as f64 * sixteenth + jitter, strength));
        }
    }
    let grid = fit_grid(&onsets, 123., 4., false, None);
    assert!((grid.tempo - tempo).abs() < 0.01, "{grid:?}");
    assert!((grid.downbeat - lead).abs() < 0.003, "{grid:?}");
    // An estimate of half the tempo: double it explains the notes, and it's nearest the Set's 120.
    let doubled = fit_grid(&onsets, 61.6, 4., true, Some(120.));
    assert!((doubled.tempo - tempo).abs() < 0.01, "{doubled:?}");
    assert!((doubled.downbeat - lead).abs() < 0.003, "{doubled:?}");
}
