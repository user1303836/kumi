//! The map's layout: the same map for the same points, groups that sound apart kept apart, landmarks past a size.

use kumi_layout::{layout, layout_until, Layout};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A number in [0, 1) for `at`, the same every time (SplitMix64).
fn noise(at: u64) -> f64 {
    let mut z = at.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// A Gaussian number for `at` (Box–Muller on two of the above).
fn gaussian(at: u64) -> f64 {
    let (u, v) = (noise(2 * at).max(1e-12), noise(2 * at + 1));
    (-2. * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

/// `groups` groups of points in `dims` numbers: point `at` is in group `at % groups`, around its centre (each centre's
/// numbers drawn about ±1 apart from the others) by `wide`.
fn grouped(at: usize, dims: usize, groups: usize, wide: f64, buffer: &mut [f32]) {
    let group = at % groups;
    for (d, value) in buffer.iter_mut().enumerate().take(dims) {
        let centre = gaussian((1_000_003 * group + d) as u64);
        *value = (centre + wide * gaussian((7 + at * dims + d) as u64 * 31)) as f32;
    }
}

fn how(threads: usize) -> Layout {
    Layout { threads, ..Layout::default() }
}

/// How well the map keeps each point's `k` nearest (van der Maaten's trustworthiness, 1 when it keeps them all): the
/// points that come near on the map, by how far down they were in the points' own order.
fn trustworthiness(points: &[Vec<f32>], places: &[[f32; 2]], k: usize) -> f64 {
    let n = points.len();
    let apart = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(a, b)| ((a - b) as f64).powi(2)).sum::<f64>();
    let mut penalty = 0f64;
    for i in 0..n {
        let mut by_points: Vec<usize> = (0..n).filter(|j| *j != i).collect();
        by_points.sort_by(|a, b| apart(&points[i], &points[*a]).total_cmp(&apart(&points[i], &points[*b])).then(a.cmp(b)));
        let mut rank = vec![0usize; n];
        for (r, j) in by_points.iter().enumerate() {
            rank[*j] = r + 1;
        }
        let mut by_map: Vec<usize> = (0..n).filter(|j| *j != i).collect();
        by_map.sort_by(|a, b| apart(&places[i], &places[*a]).total_cmp(&apart(&places[i], &places[*b])).then(a.cmp(b)));
        for j in &by_map[..k] {
            if rank[*j] > k {
                penalty += (rank[*j] - k) as f64;
            }
        }
    }
    1. - 2. / (n as f64 * k as f64 * (2. * n as f64 - 3. * k as f64 - 1.)) * penalty
}

/// How many points sit nearer their own group's middle on the map than any other group's.
fn kept_together(places: &[[f32; 2]], groups: usize) -> usize {
    let middles: Vec<[f64; 2]> = (0..groups)
        .map(|group| {
            let mine: Vec<&[f32; 2]> = places.iter().skip(group).step_by(groups).collect();
            let sum = mine.iter().fold([0f64; 2], |sum, place| [sum[0] + place[0] as f64, sum[1] + place[1] as f64]);
            [sum[0] / mine.len() as f64, sum[1] / mine.len() as f64]
        })
        .collect();
    places
        .iter()
        .enumerate()
        .filter(|(at, place)| {
            let apart = |middle: &[f64; 2]| (place[0] as f64 - middle[0]).powi(2) + (place[1] as f64 - middle[1]).powi(2);
            (0..groups).all(|group| group == at % groups || apart(&middles[at % groups]) < apart(&middles[group]))
        })
        .count()
}

#[test]
fn the_same_points_make_the_same_map_however_many_threads_make_it() {
    let read = |at: usize, buffer: &mut [f32]| grouped(at, 40, 5, 0.3, buffer);
    let one = layout(300, 40, &read, &how(1));
    assert_eq!(one.len(), 300);
    assert!(one.iter().all(|place| place.iter().all(|value| (0. ..=1.).contains(value))));
    assert_eq!(layout(300, 40, &read, &how(3)), one, "the same map with three threads");
    assert_eq!(layout(300, 40, &read, &how(1)), one, "and again");
}

#[test]
fn groups_that_sound_apart_stay_apart() {
    let (groups, count) = (6, 360);
    let points: Vec<Vec<f32>> = (0..count)
        .map(|at| {
            let mut buffer = vec![0f32; 40];
            grouped(at, 40, groups, 0.25, &mut buffer);
            buffer
        })
        .collect();
    let places = layout(count, 40, &|at, buffer| buffer.copy_from_slice(&points[at]), &how(2));
    let kept = kept_together(&places, groups);
    assert!(kept >= count * 98 / 100, "{kept} of {count} by their own group");
    let trust = trustworthiness(&points, &places, 10);
    assert!(trust > 0.9, "trustworthiness {trust}");
}

#[test]
fn points_of_many_numbers_are_brought_down_to_their_main_directions_first() {
    // 512 numbers, as a song's style vector has: four groups, and the same map each time.
    let read = |at: usize, buffer: &mut [f32]| grouped(at, 512, 4, 0.2, buffer);
    let places = layout(240, 512, &read, &how(2));
    assert_eq!(kept_together(&places, 4), 240);
    assert_eq!(layout(240, 512, &read, &how(1)), places);
}

#[test]
fn past_the_landmarks_every_other_point_sits_among_its_nearest() {
    // Each point has a twin with the same numbers: wherever the landmarks fall, twins share a place.
    let (groups, count) = (6, 1_200);
    let point = |at: usize, buffer: &mut [f32]| grouped(at / 2, 40, groups, 0.25, buffer);
    let how = Layout { landmarks: 300, ..how(2) };
    let places = layout(count, 40, &point, &how);
    assert!(places.iter().all(|place| place.iter().all(|value| (0. ..=1.).contains(value))));
    for twin in 0..count / 2 {
        assert_eq!(places[2 * twin], places[2 * twin + 1], "twins {} and {}", 2 * twin, 2 * twin + 1);
    }
    // The twins' groups (by `at / 2`) still sit apart.
    let firsts: Vec<[f32; 2]> = places.iter().step_by(2).copied().collect();
    assert!(kept_together(&firsts, groups) >= firsts.len() * 97 / 100);
    assert_eq!(layout(count, 40, &point, &Layout { threads: 1, ..how.clone() }), places, "the same with one thread");
}

#[test]
fn sixteen_thousand_points_past_their_landmarks_sit_among_them_within_their_share_of_the_maps_budget() {
    // The same at a library's size: 16,000 points past 1,000 landmarks, in a couple of seconds. The map's budget on the
    // producer's computer is a quarter of an hour for 300,000 songs (3 ms a point): this size keeps to its share of it.
    let (groups, count) = (8, 16_000);
    let point = |at: usize, buffer: &mut [f32]| grouped(at / 2, 40, groups, 0.25, buffer);
    let how = Layout { landmarks: 1_000, rounds: 300, ..how(2) };
    let started = std::time::Instant::now();
    let places = layout(count, 40, &point, &how);
    let took = started.elapsed();
    eprintln!("{count} points past {} landmarks: {took:?}", how.landmarks);
    assert!(took < std::time::Duration::from_millis(3 * count as u64), "{count} points in {took:?}");
    assert!(places.iter().all(|place| place.iter().all(|value| (0. ..=1.).contains(value))));
    for twin in 0..count / 2 {
        assert_eq!(places[2 * twin], places[2 * twin + 1], "twins {} and {}", 2 * twin, 2 * twin + 1);
    }
    let firsts: Vec<[f32; 2]> = places.iter().step_by(2).copied().collect();
    assert!(kept_together(&firsts, groups) >= firsts.len() * 97 / 100);
}

/// How far apart the groups' middles sit on the map, against how far each group's points sit from its middle.
fn parted(places: &[[f32; 2]], groups: usize) -> f64 {
    let middles: Vec<[f64; 2]> = (0..groups)
        .map(|group| {
            let mine: Vec<&[f32; 2]> = places.iter().skip(group).step_by(groups).collect();
            let sum = mine.iter().fold([0f64; 2], |sum, place| [sum[0] + place[0] as f64, sum[1] + place[1] as f64]);
            [sum[0] / mine.len() as f64, sum[1] / mine.len() as f64]
        })
        .collect();
    let apart = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    let between = (0..groups).flat_map(|a| (a + 1..groups).map(move |b| (a, b))).map(|(a, b)| apart(middles[a], middles[b])).sum::<f64>()
        / (groups * (groups - 1) / 2) as f64;
    let within = places.iter().enumerate().map(|(at, place)| apart([place[0] as f64, place[1] as f64], middles[at % groups])).sum::<f64>()
        / places.len() as f64;
    between / within
}

#[test]
fn the_maps_shape_parts_groups_further_than_plain_t_sne_without_drawing_them_to_points() {
    // The same six groups (sharing no neighbours) laid out as Kumi's map is and as plain t-SNE would (no pull after the
    // first rounds, t-SNE's tail, no floor): Kumi's keeps each group tighter against the room between them, and with no
    // floor the pull draws each to a point.
    let (groups, count) = (6, 360);
    let read = |at: usize, buffer: &mut [f32]| grouped(at, 40, groups, 0.25, buffer);
    let shaped = parted(&layout(count, 40, &read, &how(2)), groups);
    let plain = parted(&layout(count, 40, &read, &Layout { pull: 1., tail: 1., floor: 0., ..how(2) }), groups);
    assert!(shaped > plain * 1.5, "Kumi's {shaped:.2}, plain t-SNE's {plain:.2}");
    assert!(shaped < plain * 20., "Kumi's {shaped:.2} against plain t-SNE's {plain:.2}: groups drawn to points");
    let floorless = parted(&layout(count, 40, &read, &Layout { floor: 0., ..how(2) }), groups);
    assert!(floorless > plain * 100., "with no floor {floorless:.2}, plain t-SNE's {plain:.2}");
}

#[test]
fn a_few_points_still_have_places() {
    let read = |at: usize, buffer: &mut [f32]| grouped(at, 8, 2, 0.1, buffer);
    assert!(layout(0, 8, &read, &how(1)).is_empty());
    assert_eq!(layout(1, 8, &read, &how(1)), [[0.5, 0.5]]);
    for count in [2, 3, 5, 9] {
        let places = layout(count, 8, &read, &how(1));
        assert_eq!(places.len(), count);
        assert!(places.iter().all(|place| place.iter().all(|value| (0. ..=1.).contains(value))), "{count}: {places:?}");
    }
    // The same point many times: one place, the middle.
    let same = layout(20, 8, &|_, buffer| buffer.fill(0.25), &how(1));
    assert!(same.iter().all(|place| *place == [0.5, 0.5]), "{same:?}");
}

#[test]
fn a_layout_asked_to_stop_gives_no_places_and_one_let_go_on_is_the_same_map() {
    let read = |at: usize, buffer: &mut [f32]| grouped(at, 16, 3, 0.3, buffer);
    let how = Layout { rounds: 60, threads: 2, ..Layout::default() };
    let asked = AtomicUsize::new(0);
    let going = layout_until(300, 16, &read, &how, &|| asked.fetch_add(1, Ordering::Relaxed) < usize::MAX);
    assert_eq!(going.as_deref(), Some(layout(300, 16, &read, &how).as_slice()), "asking changes nothing");
    assert!(asked.load(Ordering::Relaxed) >= 60, "asked at least every round: {}", asked.load(Ordering::Relaxed));
    let stopping = AtomicUsize::new(0);
    assert_eq!(layout_until(300, 16, &read, &how, &|| stopping.fetch_add(1, Ordering::Relaxed) < 20), None);
}
