//! Other ways to lay the sounds' map out than similarity: seven arrangements (`ARRANGEMENTS`), each a place in [0, 1]
//! for every sound (y down, like the map's own) and the data words it writes where they belong. The same sounds give
//! the same places: axes go by rank, so sounds spread over them rather than piling up on a few values, and ties and
//! spreads come from a hash of the sound's path, never from chance.

use crate::{
    classify::{parse_key, parse_note, SoundClass, CLASSES},
    map::{Arrangement, Label, MapSound, ARRANGEMENTS},
};
use std::{cmp::Ordering, collections::HashMap, f64::consts::TAU};

/// The margins every place keeps from the map's edges.
const LOW: f64 = 0.04;
const HIGH: f64 = 0.96;
/// The golden angle, in radians: a sunflower's spread, which fills a disc evenly however many it holds.
const GOLDEN: f64 = 2.399_963_229_728_653;
/// The classes, then "other": a sound's group, as the sounds' layer numbers them.
const GROUPS: usize = CLASSES.len() + 1;
/// The most folder islands; the rest share one more.
const FOLDER_ISLANDS: usize = 24;

/// Each arrangement's places, in `ARRANGEMENTS`' order, and each sound's place in each (in the sounds' order), rounded
/// to 4 decimals.
pub fn arrange(sounds: &[MapSound]) -> (Vec<Arrangement>, Vec<Vec<[f32; 2]>>) {
    let seeds: Vec<u64> = sounds.iter().map(|sound| hash(&sound.path)).collect();
    let groups: Vec<usize> = sounds.iter().map(|sound| group_of(sound.class)).collect();
    let laid = [
        character(sounds, &seeds),
        texture(sounds, &seeds),
        keys(sounds, &seeds, &groups),
        tempo(sounds, &seeds, &groups),
        families(sounds, &groups),
        length(sounds, &seeds),
        folders(sounds),
    ];
    let mut places: Vec<Vec<[f32; 2]>> = (0..sounds.len()).map(|_| Vec::with_capacity(ARRANGEMENTS.len())).collect();
    let arrangements = ARRANGEMENTS
        .iter()
        .zip(laid)
        .map(|(id, (at, labels))| {
            for (point, [x, y]) in places.iter_mut().zip(at) {
                point.push([round(x), round(y)]);
            }
            let labels = labels.into_iter().map(|(text, x, y)| Label { text, x: round(x), y: round(y) }).collect();
            Arrangement { id, labels }
        })
        .collect();
    (arrangements, places)
}

/// One arrangement: each sound's place, and its labels (`text`, x, y).
type Laid = (Vec<[f64; 2]>, Vec<(String, f64, f64)>);

fn group_of(class: Option<SoundClass>) -> usize {
    class.and_then(|class| CLASSES.iter().position(|c| *c == class)).unwrap_or(CLASSES.len())
}

fn group_name(group: usize) -> &'static str {
    CLASSES.get(group).map_or("other", |class| class.as_str())
}

fn round(value: f64) -> f32 {
    ((value.clamp(0., 1.) * 1e4).round() / 1e4) as f32
}

fn hash(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    mix(hash)
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A number in [0, 1) from a sound's seed and what it's for (`salt`): the same each time.
fn unit(seed: u64, salt: u64) -> f64 {
    (mix(seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15)) >> 11) as f64 / (1u64 << 53) as f64
}

/// [0, 1] onto the map inside its margins.
fn span(t: f64) -> f64 {
    LOW + (HIGH - LOW) * t
}

/// Each known value's rank among the known ones, 0 (least) to 1 (most); ties in their seeds' order, so they spread.
fn ranks(values: &[Option<f64>], seeds: &[u64]) -> Vec<Option<f64>> {
    let mut order: Vec<usize> = (0..values.len()).filter(|at| values[*at].is_some_and(f64::is_finite)).collect();
    order.sort_unstable_by(|a, b| {
        values[*a].unwrap_or_default().total_cmp(&values[*b].unwrap_or_default()).then(seeds[*a].cmp(&seeds[*b])).then(a.cmp(b))
    });
    let mut ranked = vec![None; values.len()];
    let last = order.len().saturating_sub(1).max(1) as f64;
    for (rank, at) in order.iter().enumerate() {
        ranked[*at] = Some(if order.len() == 1 { 0.5 } else { rank as f64 / last });
    }
    ranked
}

/// A rank on the map; an unknown value near the middle, spread by the seed.
fn axis(rank: Option<f64>, seed: u64, salt: u64) -> f64 {
    span(rank.unwrap_or_else(|| 0.45 + 0.1 * unit(seed, salt)))
}

fn both(a: Option<f64>, b: Option<f64>, with: impl Fn(f64, f64) -> f64) -> Option<f64> {
    a.zip(b).map(|(a, b)| with(a, b))
}

/// Dark (left) to bright (right) by brightness; punchy (top) to soft (bottom) by a quick attack and many onsets.
fn character(sounds: &[MapSound], seeds: &[u64]) -> Laid {
    let column = |of: fn(&MapSound) -> Option<f64>| ranks(&sounds.iter().map(of).collect::<Vec<_>>(), seeds);
    let bright = column(|sound| sound.brightness);
    let quick = column(|sound| sound.attack.map(|attack| -attack));
    let busy = column(|sound| sound.onsets);
    let punch: Vec<_> = quick.iter().zip(&busy).map(|(quick, busy)| both(*quick, *busy, |a, b| (a + b) / 2.).or(*quick).or(*busy)).collect();
    let punch = ranks(&punch, seeds);
    let at = (0..sounds.len()).map(|at| [axis(bright[at], seeds[at], 1), 1. - axis(punch[at], seeds[at], 2)]).collect();
    (at, vec![])
}

/// Tonal (left) to noisy (right) by flatness; high (top) to low (bottom) by the balance of the highs' and lows' shares.
fn texture(sounds: &[MapSound], seeds: &[u64]) -> Laid {
    let noisy = ranks(&sounds.iter().map(|sound| sound.flatness).collect::<Vec<_>>(), seeds);
    let high = ranks(&sounds.iter().map(|sound| both(sound.high, sound.low, |high, low| high - low)).collect::<Vec<_>>(), seeds);
    let at = (0..sounds.len()).map(|at| [axis(noisy[at], seeds[at], 3), 1. - axis(high[at], seeds[at], 4)]).collect();
    (at, vec![])
}

const FIFTHS: [&str; 12] = ["C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#", "F"];
const SEMITONES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/// The wheel's rings (radius, and how far a ring's cell spreads around its spoke): majors, minors, root notes.
const MAJORS: (f64, f64) = (0.39, 0.06);
const MINORS: (f64, f64) = (0.25, 0.045);
const NOTES: (f64, f64) = (0.15, 0.03);
/// The middle, where untuned sounds sit by class.
const UNTUNED: f64 = 0.085;

fn on_wheel(radius: f64, angle: f64) -> [f64; 2] {
    [0.5 + radius * angle.sin(), 0.5 - radius * angle.cos()]
}

fn spoke_angle(spoke: usize) -> f64 {
    spoke as f64 * TAU / 12.
}

/// The spoke of a key's root (a minor on its relative major's).
fn spoke_of(root: &str, minor: bool) -> Option<usize> {
    let semitone = SEMITONES.iter().position(|name| *name == root)?;
    let major = if minor { (semitone + 3) % 12 } else { semitone };
    FIFTHS.iter().position(|name| *name == SEMITONES[major])
}

/// The k-th of `count` in a disc of `radius`: a sunflower's spread.
fn sunflower(k: usize, count: usize, radius: f64) -> [f64; 2] {
    let r = radius * ((k as f64 + 0.5) / count as f64).sqrt();
    let angle = k as f64 * GOLDEN;
    [r * angle.cos(), r * angle.sin()]
}

/// A circle of fifths, C at the top and clockwise G, D…: majors on the outer ring, minors on the inner one (on their
/// relative major's spoke), a one-shot's root note inside them; untuned sounds in the middle, a slice for each class.
fn keys(sounds: &[MapSound], seeds: &[u64], groups: &[usize]) -> Laid {
    let mut cells: HashMap<(u8, usize), Vec<usize>> = HashMap::new();
    let mut untuned: Vec<Vec<usize>> = vec![vec![]; GROUPS];
    for (at, sound) in sounds.iter().enumerate() {
        let key = sound.key.as_deref().and_then(parse_key);
        let tuned = match key.as_deref().and_then(|key| key.split_once(' ')) {
            Some((root, mode)) => spoke_of(root, mode == "minor").map(|spoke| (if mode == "minor" { 1 } else { 0 }, spoke)),
            None => sound
                .note
                .as_deref()
                .and_then(parse_note)
                .and_then(|note| spoke_of(note.name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-'), false))
                .map(|spoke| (2, spoke)),
        };
        match tuned {
            Some(cell) => cells.entry(cell).or_default().push(at),
            None => untuned[groups[at]].push(at),
        }
    }
    let mut at = vec![[0.5, 0.5]; sounds.len()];
    for ((ring, spoke), mut members) in cells {
        members.sort_unstable_by_key(|member| (seeds[*member], *member));
        let (radius, spread) = [MAJORS, MINORS, NOTES][ring as usize];
        let [cx, cy] = on_wheel(radius, spoke_angle(spoke));
        for (k, member) in members.iter().enumerate() {
            let [dx, dy] = sunflower(k, members.len(), spread);
            at[*member] = [cx + dx, cy + dy];
        }
    }
    let total: usize = untuned.iter().map(Vec::len).sum();
    let mut start = 0.;
    for mut members in untuned.into_iter().filter(|members| !members.is_empty()) {
        members.sort_unstable_by_key(|member| (seeds[*member], *member));
        let width = TAU * members.len() as f64 / total as f64;
        for (k, member) in members.iter().enumerate() {
            let r = UNTUNED * ((k as f64 + 0.5) / members.len() as f64).sqrt();
            let angle = start + width * (0.05 + 0.9 * (k as f64 * 0.618_033_988_75).fract());
            at[*member] = on_wheel(r, angle);
        }
        start += width;
    }
    let mut labels = Vec::with_capacity(24);
    for (spoke, root) in FIFTHS.iter().enumerate() {
        let [x, y] = on_wheel(0.468, spoke_angle(spoke));
        labels.push((root.to_string(), x, y));
    }
    for (spoke, root) in FIFTHS.iter().enumerate() {
        let semitone = SEMITONES.iter().position(|name| name == root).unwrap_or(0);
        let [x, y] = on_wheel(0.315, spoke_angle(spoke));
        labels.push((format!("{}m", SEMITONES[(semitone + 9) % 12]), x, y));
    }
    (at, labels)
}

const SLOWEST: f64 = 60.;
const FASTEST: f64 = 200.;
/// Where the lanes end and the band of sounds without a tempo begins.
const LANES_END: f64 = 0.76;
const UNTIMED: f64 = 0.85;

fn tempo_x(bpm: f64) -> f64 {
    span((bpm.clamp(SLOWEST, FASTEST) - SLOWEST) / (FASTEST - SLOWEST))
}

/// Left to right by tempo (60 to 200 BPM), a lane for each class; sounds without a tempo in a band at the bottom, the
/// shortest left.
fn tempo(sounds: &[MapSound], seeds: &[u64], groups: &[usize]) -> Laid {
    let bpm = |sound: &MapSound| sound.bpm.filter(|bpm| bpm.is_finite() && *bpm > 0.);
    let mut lane_of = vec![usize::MAX; GROUPS];
    for (sound, group) in sounds.iter().zip(groups) {
        if bpm(sound).is_some() {
            lane_of[*group] = 0;
        }
    }
    let mut lanes = 0;
    for lane in lane_of.iter_mut().filter(|lane| **lane == 0) {
        *lane = lanes;
        lanes += 1;
    }
    let height = (LANES_END - LOW) / lanes.max(1) as f64;
    let mut buckets: HashMap<(usize, i64), Vec<usize>> = HashMap::new();
    for (at, sound) in sounds.iter().enumerate() {
        if let Some(bpm) = bpm(sound) {
            buckets.entry((lane_of[groups[at]], bpm.clamp(SLOWEST, FASTEST).round() as i64)).or_default().push(at);
        }
    }
    let mut at = vec![[0.5, 0.5]; sounds.len()];
    for ((lane, _), mut members) in buckets {
        members.sort_unstable_by_key(|member| (seeds[*member], *member));
        let count = members.len() as f64;
        for (k, member) in members.iter().enumerate() {
            let seed = seeds[*member];
            let along = (k as f64 + 0.5 + 0.7 * (unit(seed, 5) - 0.5)) / count;
            let y = LOW + height * (lane as f64 + 0.12 + 0.76 * along);
            at[*member] = [tempo_x(bpm(&sounds[*member]).unwrap_or(SLOWEST)) + 0.006 * (unit(seed, 6) - 0.5), y];
        }
    }
    let lengths: Vec<_> = sounds.iter().map(|sound| if bpm(sound).is_some() { None } else { sound.seconds }).collect();
    let lengths = ranks(&lengths, seeds);
    for (place, ((sound, length), seed)) in at.iter_mut().zip(sounds.iter().zip(lengths).zip(seeds)) {
        if bpm(sound).is_none() {
            *place = [span(length.unwrap_or_else(|| unit(*seed, 7))), UNTIMED + (HIGH - UNTIMED) * unit(*seed, 8)];
        }
    }
    let labels = (SLOWEST as u32..=FASTEST as u32).step_by(20).map(|bpm| (bpm.to_string(), tempo_x(bpm as f64), 0.8)).collect();
    (at, labels)
}

/// An island for each class (in the classes' order, then "other"), its members where similarity puts them.
fn families(sounds: &[MapSound], groups: &[usize]) -> Laid {
    let mut present = [false; GROUPS];
    for group in groups {
        present[*group] = true;
    }
    let names: Vec<usize> = (0..GROUPS).filter(|group| present[*group]).collect();
    let island_of: HashMap<usize, usize> = names.iter().enumerate().map(|(island, group)| (*group, island)).collect();
    let islands_of: Vec<usize> = groups.iter().map(|group| island_of[group]).collect();
    islands(sounds, &islands_of, &names.iter().map(|group| group_name(*group).to_string()).collect::<Vec<_>>())
}

/// An island for each top folder (the first folder the sounds' places don't all share), the largest first, its
/// members where similarity puts them; past 24, the smallest share one island ("…").
fn folders(sounds: &[MapSound]) -> Laid {
    let parts: Vec<Vec<&str>> = sounds.iter().map(|sound| sound.place.split(" / ").collect()).collect();
    let shared = parts.first().map_or(0, |first| {
        parts.iter().fold(first.len(), |shared, part| shared.min(part.iter().zip(first).take_while(|(a, b)| a == b).count()))
    });
    let tops: Vec<&str> = parts
        .iter()
        .map(|part| part.get(shared).or_else(|| part.last()).copied().unwrap_or_default())
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for top in &tops {
        *counts.entry(*top).or_default() += 1;
    }
    let mut order: Vec<(&str, usize)> = counts.into_iter().collect();
    order.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let kept = if order.len() > FOLDER_ISLANDS { FOLDER_ISLANDS - 1 } else { order.len() };
    let island_of: HashMap<&str, usize> = order.iter().enumerate().map(|(island, (top, _))| (*top, island.min(kept))).collect();
    let mut names: Vec<String> = order[..kept].iter().map(|(top, _)| top.to_string()).collect();
    if kept < order.len() {
        names.push("…".into());
    }
    islands(sounds, &tops.iter().map(|top| island_of[top]).collect::<Vec<_>>(), &names)
}

/// Islands on a grid, each `names[i]` written above it and sized by how many it holds, its members' similarity places
/// stretched over it.
fn islands(sounds: &[MapSound], island_of: &[usize], names: &[String]) -> Laid {
    let count = names.len().max(1);
    let columns = (count as f64).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let (width, height) = ((HIGH - LOW) / columns as f64, (HIGH - LOW) / rows as f64);
    // Each island's members' extent on the similarity map, and how many it holds.
    let mut extent = vec![(f64::MAX, f64::MAX, f64::MIN, f64::MIN, 0usize); count];
    for (sound, island) in sounds.iter().zip(island_of) {
        let e = &mut extent[*island];
        *e = (e.0.min(sound.x), e.1.min(sound.y), e.2.max(sound.x), e.3.max(sound.y), e.4 + 1);
    }
    let most = extent.iter().map(|e| e.4).max().unwrap_or(1).max(1) as f64;
    let boxes: Vec<[f64; 4]> = (0..count)
        .map(|island| {
            let (left, top) = (LOW + (island % columns) as f64 * width, LOW + (island / columns) as f64 * height);
            let size = 0.45 + 0.55 * (extent[island].4 as f64 / most).sqrt();
            let (w, h) = (0.84 * width * size, 0.72 * height * size);
            let (cx, cy) = (left + width / 2., top + height * 0.58);
            [cx - w / 2., cy - h / 2., w, h]
        })
        .collect();
    let at = sounds
        .iter()
        .zip(island_of)
        .map(|(sound, island)| {
            let (min_x, min_y, max_x, max_y, _) = extent[*island];
            let stretch = |value: f64, min: f64, max: f64| if max > min { (value - min) / (max - min) } else { 0.5 };
            let [left, top, w, h] = boxes[*island];
            [left + w * stretch(sound.x, min_x, max_x), top + h * stretch(sound.y, min_y, max_y)]
        })
        .collect();
    let labels = names
        .iter()
        .enumerate()
        .map(|(island, name)| {
            let (left, top) = (LOW + (island % columns) as f64 * width, LOW + (island / columns) as f64 * height);
            (name.clone(), left + width / 2., top + height * 0.1)
        })
        .collect();
    (at, labels)
}

/// The spiral's turns and its radius at the middle and at the outside.
const TURNS: f64 = 3.5;
const INNER: f64 = 0.03;
const OUTER: f64 = 0.42;

/// Where the spiral is `t` (0 to 1) of the way out: its radius and angle. Equal steps of `t²` are equal steps along it.
fn spiral(t: f64) -> (f64, f64) {
    (INNER + (OUTER - INNER) * t, TAU * TURNS * t)
}

/// A spiral, the shortest at the middle winding out to the longest (those of unknown length last).
fn length(sounds: &[MapSound], seeds: &[u64]) -> Laid {
    let seconds: Vec<Option<f64>> = sounds.iter().map(|sound| sound.seconds.filter(|s| s.is_finite() && *s >= 0.)).collect();
    let mut order: Vec<usize> = (0..sounds.len()).collect();
    order.sort_unstable_by(|a, b| {
        match (seconds[*a], seconds[*b]) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
        .then(seeds[*a].cmp(&seeds[*b]))
        .then(a.cmp(b))
    });
    let count = sounds.len().max(1) as f64;
    let mut at = vec![[0.5, 0.5]; sounds.len()];
    for (k, member) in order.iter().enumerate() {
        let (radius, angle) = spiral(((k as f64 + 0.5) / count).sqrt());
        at[*member] = on_wheel(radius + 0.03 * (unit(seeds[*member], 9) - 0.5), angle);
    }
    let known: Vec<f64> = order.iter().map_while(|member| seconds[*member]).collect();
    let labels = [(0.1, "0.1 s"), (1., "1 s"), (10., "10 s")]
        .into_iter()
        .filter_map(|(value, text)| {
            let k = known.partition_point(|seconds| *seconds < value);
            (k > 0 && k < known.len()).then(|| {
                let (radius, angle) = spiral((k as f64 / count).sqrt());
                let [x, y] = on_wheel(radius + 0.045, angle);
                (text.to_string(), x, y)
            })
        })
        .collect();
    (at, labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(at: usize) -> MapSound {
        MapSound {
            path: format!("/Samples/{at}.wav"),
            x: (at as f64 * 0.618_034).fract(),
            y: (at as f64 * 0.414_214).fract(),
            name: format!("{at}"),
            place: "Samples / Drums".into(),
            ..Default::default()
        }
    }

    fn place(sounds: &[MapSound], id: &str, at: usize) -> [f32; 2] {
        let k = ARRANGEMENTS.iter().position(|each| *each == id).unwrap();
        arrange(sounds).1[at][k]
    }

    #[test]
    fn every_place_is_on_the_map_and_the_same_sounds_give_the_same_places() {
        let sounds: Vec<_> = (0..300)
            .map(|at| MapSound {
                class: CLASSES.get(at % 30).copied(),
                seconds: (at % 11 != 0).then(|| 0.01 * (at as f64 + 1.)),
                bpm: (at % 3 == 0).then(|| 40. + at as f64),
                key: (at % 4 == 0).then(|| format!("{} {}", SEMITONES[at % 12], if at % 8 == 0 { "minor" } else { "major" })),
                note: (at % 4 == 1).then(|| format!("{}2", SEMITONES[at % 12])),
                brightness: (at % 9 != 0).then(|| at as f64 * 10.),
                attack: Some((at % 13) as f64),
                onsets: Some((at % 17) as f64),
                flatness: Some((at % 19) as f64 / 19.),
                low: Some(0.3),
                high: Some((at % 23) as f64 / 23.),
                place: format!("Pack {} / Sub {}", at % 30, at % 2),
                ..sound(at)
            })
            .collect();
        let (arrangements, places) = arrange(&sounds);
        assert_eq!(arrangements.iter().map(|each| each.id).collect::<Vec<_>>(), ARRANGEMENTS);
        assert_eq!(places.len(), sounds.len());
        for point in &places {
            assert_eq!(point.len(), ARRANGEMENTS.len());
            assert!(point.iter().flatten().all(|v| (0. ..=1.).contains(v)));
        }
        assert!(arrangements.iter().flat_map(|each| &each.labels).all(|label| (0. ..=1.).contains(&label.x) && (0. ..=1.).contains(&label.y)));
        assert_eq!(arrange(&sounds), (arrangements, places.clone()));
        // Spread: within each arrangement no two sounds share a place.
        for k in 0..ARRANGEMENTS.len() {
            let mut seen: Vec<_> = places.iter().map(|point| (point[k][0].to_bits(), point[k][1].to_bits())).collect();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), sounds.len(), "{} piles sounds up", ARRANGEMENTS[k]);
        }
    }

    #[test]
    fn character_puts_brighter_right_and_punchier_higher() {
        let dark_soft = MapSound { brightness: Some(400.), attack: Some(80.), onsets: Some(0.5), ..sound(0) };
        let bright_punchy = MapSound { brightness: Some(6000.), attack: Some(2.), onsets: Some(6.), ..sound(1) };
        let middle = MapSound { brightness: Some(2000.), attack: Some(20.), onsets: Some(2.), ..sound(2) };
        let sounds = [dark_soft, bright_punchy, middle];
        let (soft, punchy) = (place(&sounds, "character", 0), place(&sounds, "character", 1));
        assert!(punchy[0] > soft[0], "brighter is further right");
        assert!(punchy[1] < soft[1], "punchier is higher (y down)");
    }

    #[test]
    fn keys_put_c_major_on_the_top_spokes_outer_ring() {
        let sounds = [
            MapSound { key: Some("C major".into()), ..sound(0) },
            MapSound { key: Some("A minor".into()), ..sound(1) },
            MapSound { key: Some("G major".into()), ..sound(2) },
            MapSound { class: Some(SoundClass::Kick), ..sound(3) },
        ];
        let [x, y] = place(&sounds, "keys", 0);
        assert!((x - 0.5).abs() < 0.07 && y < 0.18, "C major sits at the top, outside: {x} {y}");
        let [mx, my] = place(&sounds, "keys", 1);
        assert!((mx - 0.5).abs() < 0.05 && my > y && my < 0.35, "A minor on C's spoke, inside: {mx} {my}");
        let [gx, _] = place(&sounds, "keys", 2);
        assert!(gx > 0.6, "G is clockwise from C");
        let [ux, uy] = place(&sounds, "keys", 3);
        assert!((ux - 0.5).hypot(uy - 0.5) <= UNTUNED as f32 + 1e-3, "an untuned kick sits in the middle");
        let labels = &arrange(&sounds).0[2].labels;
        assert_eq!((labels[0].text.as_str(), labels[1].text.as_str(), labels[12].text.as_str()), ("C", "G", "Am"));
    }

    #[test]
    fn tempo_puts_faster_loops_right_and_untimed_sounds_below() {
        let sounds = [
            MapSound { bpm: Some(90.), class: Some(SoundClass::Drums), ..sound(0) },
            MapSound { bpm: Some(140.), class: Some(SoundClass::Drums), ..sound(1) },
            MapSound { seconds: Some(0.2), ..sound(2) },
        ];
        let (slow, fast, untimed) = (place(&sounds, "tempo", 0), place(&sounds, "tempo", 1), place(&sounds, "tempo", 2));
        assert!(fast[0] > slow[0]);
        assert!(untimed[1] > slow[1].max(fast[1]));
    }

    #[test]
    fn families_keep_each_class_together_and_apart() {
        let sounds: Vec<_> = (0..60)
            .map(|at| MapSound {
                class: [Some(SoundClass::Kick), Some(SoundClass::Pad), None][at % 3],
                x: (at as f64 * 0.37).fract(),
                y: (at as f64 * 0.61).fract(),
                ..sound(at)
            })
            .collect();
        let (arrangements, places) = arrange(&sounds);
        let k = ARRANGEMENTS.iter().position(|id| *id == "families").unwrap();
        let extent = |class: usize| {
            places.iter().enumerate().filter(|(at, _)| at % 3 == class).fold([1f32, 1., 0., 0.], |e, (_, point)| {
                let [x, y] = point[k];
                [e[0].min(x), e[1].min(y), e[2].max(x), e[3].max(y)]
            })
        };
        let boxes: Vec<_> = (0..3).map(extent).collect();
        for a in 0..3 {
            for b in a + 1..3 {
                let apart = boxes[a][2] < boxes[b][0] || boxes[b][2] < boxes[a][0] || boxes[a][3] < boxes[b][1] || boxes[b][3] < boxes[a][1];
                assert!(apart, "class islands {a} and {b} overlap: {:?} {:?}", boxes[a], boxes[b]);
            }
        }
        let labels: Vec<_> = arrangements[k].labels.iter().map(|label| label.text.as_str()).collect();
        assert_eq!(labels, ["kick", "pad", "other"]);
    }

    #[test]
    fn length_winds_out_from_the_shortest_and_folders_name_the_first_folder_apart() {
        let sounds: Vec<_> = (0..40)
            .map(|at| MapSound {
                seconds: Some(0.02 * 1.2f64.powi(at as i32)),
                place: format!("Samples / {} / Kicks", ["Drums", "Synths"][at % 2]),
                ..sound(at)
            })
            .collect();
        let (arrangements, places) = arrange(&sounds);
        let from_middle = |at: usize| (places[at][5][0] - 0.5).hypot(places[at][5][1] - 0.5);
        assert!(from_middle(0) < from_middle(20) && from_middle(20) < from_middle(39));
        let texts: Vec<_> = arrangements[5].labels.iter().map(|label| label.text.as_str()).collect();
        assert_eq!(texts, ["0.1 s", "1 s", "10 s"]);
        let folders: Vec<_> = arrangements[6].labels.iter().map(|label| label.text.as_str()).collect();
        assert_eq!(folders, ["Drums", "Synths"]);
    }
}
