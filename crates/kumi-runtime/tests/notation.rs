//! The notation reads back what it prints: random notes of every kind, in several meters and frames, print and read
//! back as the same notes (times to float noise, everything else exactly), and on-grid parts print short.
use kumi_runtime::notation::{parse, print, Frame, Note};
use rand::{rngs::StdRng, Rng, SeedableRng};

const EPSILON: f64 = 1e-9;

fn same(read: &[Note], notes: &[Note]) -> bool {
    let by_pitch = |a: &Note, b: &Note| a.pitch.cmp(&b.pitch).then(a.start.total_cmp(&b.start)).then(a.duration.total_cmp(&b.duration));
    let (mut read, mut notes) = (read.to_vec(), notes.to_vec());
    read.sort_by(by_pitch);
    notes.sort_by(by_pitch);
    let close = |a: f64, b: f64| (a - b).abs() < EPSILON;
    read.len() == notes.len()
        && read.iter().zip(&notes).all(|(a, b)| {
            a.pitch == b.pitch
                && close(a.start, b.start)
                && close(a.duration, b.duration)
                && close(a.velocity, b.velocity)
                && a.mute == b.mute
                && close(a.probability, b.probability)
                && close(a.velocity_deviation, b.velocity_deviation)
        })
}
fn round_trip(notes: &[Note], frame: &Frame) -> String {
    let printed = print(notes, frame);
    if !printed.exact {
        let read = parse(&printed.text, &Frame { length: None, ..frame.clone() });
        let mut sorted = notes.to_vec();
        sorted.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)).then(a.duration.total_cmp(&b.duration)));
        let first_off = read.as_ref().ok().and_then(|read| {
            read.iter().zip(&sorted).find(|(a, b)| !same(&[(*a).clone()], &[(*b).clone()])).map(|(a, b)| format!("{a:?}\n vs {b:?}"))
        });
        panic!(
            "a print that isn't exact ({:?}; {} notes, read {:?}; first off: {first_off:?}):\n{}",
            frame,
            notes.len(),
            read.as_ref().map(Vec::len).map_err(|e| e.to_string()),
            printed.text
        );
    }
    let read = parse(&printed.text, &Frame { length: None, ..frame.clone() }).unwrap_or_else(|error| panic!("{error}\n{}", printed.text));
    assert!(same(&read, notes), "read back differently:\n{}", printed.text);
    printed.text
}
fn frames(rng: &mut StdRng) -> Frame {
    let (numerator, denominator) = [(4, 4), (3, 4), (6, 8), (7, 8), (5, 4), (12, 8)][rng.random_range(0..6)];
    let origin = if rng.random_bool(0.5) { 0. } else { rng.random_range(0..64) as f64 * 4. };
    Frame { origin, numerator, denominator, tempo: rng.random_range(60..180) as f64, ..Frame::default() }
}

#[test]
fn notes_on_a_grid_read_back_as_they_were() {
    let mut rng = StdRng::seed_from_u64(7);
    for _ in 0..300 {
        let frame = frames(&mut rng);
        let grid = [0.25, 0.5, 1. / 3., 1. / 6., 0.125, 1.][rng.random_range(0..6)];
        let notes: Vec<Note> = (0..rng.random_range(1..40))
            .map(|_| {
                let mut note = Note::new(
                    rng.random_range(24..100),
                    rng.random_range(0..64) as f64 * grid,
                    rng.random_range(1..9) as f64 * grid,
                    rng.random_range(1..=127) as f64,
                );
                if rng.random_bool(0.2) {
                    note.probability = rng.random_range(0..=20) as f64 / 20.;
                }
                if rng.random_bool(0.1) {
                    note.velocity_deviation = rng.random_range(-20..=20) as f64;
                    note.velocity = note.velocity.clamp(21., 106.);
                }
                note.mute = rng.random_bool(0.05);
                note
            })
            .collect();
        round_trip(&notes, &frame);
    }
}

#[test]
fn recorded_notes_off_any_grid_read_back_as_they_were() {
    let mut rng = StdRng::seed_from_u64(11);
    for _ in 0..300 {
        let frame = Frame { drums: rng.random_bool(0.5), ..frames(&mut rng) };
        let notes: Vec<Note> = (0..rng.random_range(1..30))
            .map(|_| {
                Note::new(rng.random_range(36..90), rng.random_range(0.0..64.), rng.random_range(0.01..4.), rng.random_range(1.0..127.))
            })
            .collect();
        round_trip(&notes, &frame);
    }
}

#[test]
fn drum_patterns_print_as_lanes_and_read_back() {
    let mut rng = StdRng::seed_from_u64(13);
    let (mut lanes, mut drums) = (0, 0);
    for _ in 0..300 {
        let frame = Frame { drums: true, ..frames(&mut rng) };
        let mut notes = vec![];
        for pitch in [36, 38, 42, 46] {
            let step = [0.25, 0.5, 1. / 3., 1. / 6., 0.125][rng.random_range(0..5)];
            let period = rng.random_range(1..=16);
            let shift = if rng.random_bool(0.2) { rng.random_range(-20..20) as f64 / 960. } else { 0. };
            let cells: Vec<u8> = (0..period).map(|_| rng.random_range(0..6)).collect();
            let start = frame.origin + rng.random_range(0..4) as f64 * frame.bar();
            for at in 0..rng.random_range(period..=period * 6) {
                let time = start + at as f64 * step + shift;
                match cells[at % period] {
                    0 | 1 => {}
                    2 => notes.push(Note::new(pitch, time - frame.origin, step, 100.)),
                    3 => notes.push(Note::new(pitch, time - frame.origin, step, 127.)),
                    4 => notes.push(Note::new(pitch, time - frame.origin, step, 60.)),
                    _ => {
                        let count = rng.random_range(2..=4);
                        for hit in 0..count {
                            let part = step / count as f64;
                            notes.push(Note::new(pitch, time + hit as f64 * part - frame.origin, part, 100.));
                        }
                    }
                }
            }
        }
        notes.retain(|note| note.start >= 0.);
        if !notes.is_empty() {
            let text = round_trip(&notes, &frame);
            // A lane's line begins with its drum; a sequence's with a position or a setting.
            let setting = |line: &str| {
                line.len() > 1
                    && line.starts_with(['v', 'p', 'l'])
                    && line[1..].starts_with(|c: char| c.is_ascii_digit() || "/.".contains(c))
            };
            lanes += text.lines().filter(|line| line.starts_with(|c: char| c.is_ascii_alphabetic()) && !setting(line)).count();
            let mut pitches: Vec<u8> = notes.iter().map(|note| note.pitch).collect();
            pitches.sort_unstable();
            pitches.dedup();
            drums += pitches.len();
        }
    }
    assert!(lanes * 10 >= drums * 9, "{lanes} of {drums} drums printed as lanes");
}

#[test]
fn a_loop_on_the_grid_prints_short() {
    let frame = Frame { drums: true, ..Frame::default() };
    let written = "kick x..x..x...x..x.. *16\nsnare ....X... *32\nhat x.x.x.x.x.x.3.x. *16 o=40\nC0 /8 x--.x... *16";
    let notes = parse(written, &frame).unwrap();
    let text = round_trip(&notes, &frame);
    assert!(text.lines().count() == 4 && text.len() < 160, "{text}");
    // Five steps over four bars of 4/4: one line, however long it runs.
    let five = parse("rim x.x.. *to 5|1", &frame).unwrap();
    assert_eq!(round_trip(&five, &frame), "rim x.x.. *13", "thirteen times fills the four bars, the last two steps rests");
    // A melody in song time, in 6/8, from bar 9.
    let six = Frame { origin: 24., numerator: 6, denominator: 8, ..Frame::default() };
    let melody = parse("l/8 9|1 C3 D3 E3 l/4. G3 10|1 [C3 E3 G3]", &six).unwrap();
    assert_eq!(round_trip(&melody, &six), "9|1 l/8 C3 D3 E3 G3/4.\n10|1 l/4. [C3 E3 G3]");
}
