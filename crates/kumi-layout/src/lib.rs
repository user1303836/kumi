//! Where each of the producer's sounds and songs sits on the map: t-SNE with Barnes–Hut's approximation (van der
//! Maaten, "Accelerating t-SNE using Tree-Based Algorithms", JMLR 2014), so what sounds alike sits together.
//!
//! The same points make the same map each time: the layout starts from their two main directions, draws nothing at
//! random, and adds up in a fixed order however many threads share the work. Points are first brought down to their
//! 50 main directions. Past 50,000 points, t-SNE lays out 50,000 landmarks spread over them, and every other point sits
//! at the distance-weighted mean of its 5 nearest landmarks. Places are in [0, 1] on both axes.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How a map is made: the map's own, or smaller for tests.
#[derive(Debug, Clone)]
pub struct Layout {
    /// About how many neighbours each point's place answers to.
    pub perplexity: f64,
    /// Rounds of the layout, the first 250 (a third at most) with the pull between neighbours exaggerated, so groups
    /// form before they settle.
    pub rounds: usize,
    /// Points t-SNE lays out at most: past that, landmarks.
    pub landmarks: usize,
    /// Threads sharing the work (the map is the same whatever the number).
    pub threads: usize,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout { perplexity: 30., rounds: 750, landmarks: 50_000, threads: threads() }
    }
}

/// The threads a map takes: two fewer than the computer has, at most four, as it's made in the background.
pub fn threads() -> usize {
    std::thread::available_parallelism().map(usize::from).unwrap_or(1).saturating_sub(2).clamp(1, 4)
}

const EXAGGERATED: usize = 250;
const EXAGGERATION: f64 = 12.;
/// Barnes–Hut's θ: a group of points farther than twice its width away counts as one.
const THETA: f64 = 0.5;
/// The main directions points are brought down to.
const REDUCED: usize = 50;
/// Rounds that find the main directions of points with more numbers than that.
const DIRECTION_ROUNDS: usize = 40;
/// Nearest landmarks a point that isn't one sits among.
const PLACED_BY: usize = 5;
/// Points taken at a time: what's summed over points is summed a chunk at a time, and the chunks in order.
const CHUNK: usize = 256;
/// The same for a sum as big as the points' spread (a number for each pair of their numbers).
const SPREAD_CHUNK: usize = 4_096;

/// Each of `count` points' place on the map (x, y in [0, 1]), its `dims` numbers read into a buffer by `read`. Points
/// that are alike in the same proportion sit as close: give each number the weight it has in how alike they sound.
pub fn layout(count: usize, dims: usize, read: &(dyn Fn(usize, &mut [f32]) + Sync), how: &Layout) -> Vec<[f32; 2]> {
    layout_until(count, dims, read, how, &|| true).unwrap_or_default()
}

/// `layout`, asking `go_on` as it works (between t-SNE's rounds, and as each point is weighed or placed): it may wait
/// there (the learner waits while the DAW plays), and false ends the layout, with no places (None).
pub fn layout_until(
    count: usize,
    dims: usize,
    read: &(dyn Fn(usize, &mut [f32]) + Sync),
    how: &Layout,
    go_on: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<[f32; 2]>> {
    if count == 0 {
        return Some(vec![]);
    }
    if dims == 0 {
        return Some(vec![[0.5, 0.5]; count]);
    }
    let going = Going { go_on, stopped: AtomicBool::new(false) };
    let threads = how.threads.max(1);
    let most = how.landmarks.max(1);
    // Their main directions, from points spread through them by their order (every so many), then every point in them.
    let sample: Vec<usize> = (0..count).step_by(count.div_ceil(most)).collect();
    let axes = Axes::of(&sample, dims, read, threads);
    let k = axes.k;
    if !going.on() {
        return None;
    }
    let reduced = axes.project(count, read, threads);
    let landmarks: Vec<usize> = if count <= most { (0..count).collect() } else { spread(&reduced, k, most) };
    let points: Vec<f32> = landmarks.iter().flat_map(|at| reduced[at * k..(at + 1) * k].iter().copied()).collect();
    if !going.on() {
        return None;
    }
    let laid = tsne(&points, k, how, threads, &going)?;
    let mut places = vec![[0f64; 2]; count];
    if landmarks.len() == count {
        places = laid;
    } else {
        let mut landmark = vec![false; count];
        for (at, place) in landmarks.iter().zip(&laid) {
            landmark[*at] = true;
            places[*at] = *place;
        }
        let others: Vec<usize> = (0..count).filter(|at| !landmark[*at]).collect();
        let placed = fill_rows(
            threads,
            others.len(),
            2,
            || (),
            |_, row, place: &mut [f64]| {
                if going.on() {
                    let at = others[row];
                    place.copy_from_slice(&among(&reduced[at * k..(at + 1) * k], &points, k, &laid));
                }
            },
        );
        if going.stopped() {
            return None;
        }
        for (at, place) in others.iter().zip(placed.chunks(2)) {
            places[*at] = [place[0], place[1]];
        }
    }
    Some(unit_square(&places))
}

/// Whether a layout goes on: its `go_on`, until that says no once.
struct Going<'a> {
    go_on: &'a (dyn Fn() -> bool + Sync),
    stopped: AtomicBool,
}

impl Going<'_> {
    fn on(&self) -> bool {
        if self.stopped() || (self.go_on)() {
            return !self.stopped();
        }
        self.stopped.store(true, Ordering::Relaxed);
        false
    }
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
}

/// A point that isn't a landmark: at its nearest landmarks' places, each counting for one over its distance.
fn among(point: &[f32], landmarks: &[f32], k: usize, places: &[[f64; 2]]) -> [f64; 2] {
    let nearest = nearest_to(point, landmarks, k, PLACED_BY, None);
    if let Some((_, at)) = nearest.first().filter(|(distance, _)| *distance <= 0.) {
        return places[*at as usize];
    }
    let (mut sum, mut weights) = ([0f64; 2], 0f64);
    for (distance, at) in &nearest {
        let weight = 1. / (*distance as f64).sqrt();
        sum[0] += weight * places[*at as usize][0];
        sum[1] += weight * places[*at as usize][1];
        weights += weight;
    }
    [sum[0] / weights, sum[1] / weights]
}

/// The points' main directions: their mean and up to `REDUCED` directions (unit length, the widest first).
struct Axes {
    dims: usize,
    k: usize,
    mean: Vec<f64>,
    /// `k` directions of `dims` numbers each.
    axes: Vec<f64>,
}

impl Axes {
    fn of(sample: &[usize], dims: usize, read: &(dyn Fn(usize, &mut [f32]) + Sync), threads: usize) -> Axes {
        let chunks = sample.len().div_ceil(SPREAD_CHUNK);
        let part = |chunk: usize| &sample[chunk * SPREAD_CHUNK..((chunk + 1) * SPREAD_CHUNK).min(sample.len())];
        let sums = in_chunks(threads, chunks, |chunk| {
            let (mut sum, mut buffer) = (vec![0f64; dims], vec![0f32; dims]);
            for at in part(chunk) {
                read(*at, &mut buffer);
                sum.iter_mut().zip(&buffer).for_each(|(sum, value)| *sum += *value as f64);
            }
            sum
        });
        let mut mean = vec![0f64; dims];
        for sum in &sums {
            mean.iter_mut().zip(sum).for_each(|(mean, sum)| *mean += sum);
        }
        mean.iter_mut().for_each(|mean| *mean /= sample.len() as f64);
        // How the numbers vary together (the upper triangle, a chunk at a time).
        let spreads = in_chunks(threads, chunks, |chunk| {
            let (mut spread, mut buffer, mut centred) = (vec![0f64; dims * dims], vec![0f32; dims], vec![0f64; dims]);
            for at in part(chunk) {
                read(*at, &mut buffer);
                centred.iter_mut().zip(&buffer).zip(&mean).for_each(|((centred, value), mean)| *centred = *value as f64 - mean);
                for i in 0..dims {
                    let row = &mut spread[i * dims..(i + 1) * dims];
                    let value = centred[i];
                    for j in i..dims {
                        row[j] += value * centred[j];
                    }
                }
            }
            spread
        });
        let mut spread = vec![0f64; dims * dims];
        for part in &spreads {
            spread.iter_mut().zip(part).for_each(|(spread, part)| *spread += part);
        }
        for i in 0..dims {
            for j in 0..i {
                spread[i * dims + j] = spread[j * dims + i];
            }
        }
        let k = dims.min(REDUCED);
        let mut axes = if dims <= REDUCED { eigen(&spread, dims).1 } else { widest(&spread, dims, k) };
        // Each direction points the way its largest number is positive: the same every time.
        for axis in axes.chunks_mut(dims) {
            let largest = axis.iter().copied().fold(0f64, |largest, value| if value.abs() > largest.abs() { value } else { largest });
            if largest < 0. {
                axis.iter_mut().for_each(|value| *value = -*value);
            }
        }
        Axes { dims, k, mean, axes }
    }

    /// Every point in the main directions: `count` rows of `k`.
    fn project(&self, count: usize, read: &(dyn Fn(usize, &mut [f32]) + Sync), threads: usize) -> Vec<f32> {
        let dims = self.dims;
        let scratch = || (vec![0f32; dims], vec![0f64; dims]);
        fill_rows(threads, count, self.k, scratch, |(buffer, centred), at, row: &mut [f32]| {
            read(at, buffer);
            centred.iter_mut().zip(buffer.iter()).zip(&self.mean).for_each(|((centred, value), mean)| *centred = *value as f64 - mean);
            for (value, axis) in row.iter_mut().zip(self.axes.chunks(dims)) {
                *value = axis.iter().zip(centred.iter()).map(|(a, b)| a * b).sum::<f64>() as f32;
            }
        })
    }
}

/// A symmetric matrix's eigenvalues and unit eigenvectors (`n` of `n` numbers each), the largest first: Jacobi's
/// rotations, swept until what's off the diagonal is gone.
fn eigen(matrix: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut a = matrix.to_vec();
    let mut v = vec![0f64; n * n];
    (0..n).for_each(|i| v[i * n + i] = 1.);
    let total: f64 = a.iter().map(|value| value * value).sum();
    for _ in 0..100 {
        let off: f64 = (0..n).flat_map(|i| (0..n).filter(move |j| *j != i).map(move |j| (i, j))).map(|(i, j)| a[i * n + j].powi(2)).sum();
        if off <= total * 1e-24 || off == 0. {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq == 0. {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2. * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.).sqrt());
                let t = if theta == 0. { 1. } else { t };
                let c = 1. / (t * t + 1.).sqrt();
                let s = t * c;
                for r in 0..n {
                    let (arp, arq) = (a[r * n + p], a[r * n + q]);
                    a[r * n + p] = c * arp - s * arq;
                    a[r * n + q] = s * arp + c * arq;
                }
                for r in 0..n {
                    let (apr, aqr) = (a[p * n + r], a[q * n + r]);
                    a[p * n + r] = c * apr - s * aqr;
                    a[q * n + r] = s * apr + c * aqr;
                }
                for r in 0..n {
                    let (vrp, vrq) = (v[r * n + p], v[r * n + q]);
                    v[r * n + p] = c * vrp - s * vrq;
                    v[r * n + q] = s * vrp + c * vrq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|x, y| a[y * n + y].total_cmp(&a[x * n + x]).then(x.cmp(y)));
    let values = order.iter().map(|i| a[i * n + i]).collect();
    // Row-major directions: the `i`th eigenvector is column `order[i]` of v.
    let vectors = order.iter().flat_map(|column| (0..n).map(|row| v[row * n + *column]).collect::<Vec<_>>()).collect();
    (values, vectors)
}

/// The `k` widest directions of a symmetric `n` by `n` matrix: its subspace, found by rounds of multiplying and
/// straightening a start that's the same every time, then turned to its own main directions.
fn widest(matrix: &[f64], n: usize, k: usize) -> Vec<f64> {
    // k directions of n numbers, row by row.
    let mut q: Vec<f64> = (0..k * n).map(|at| unit_noise(at as u64)).collect();
    straighten(&mut q, n);
    let multiply = |q: &[f64]| -> Vec<f64> {
        let mut out = vec![0f64; k * n];
        for (direction, row) in q.chunks(n).zip(out.chunks_mut(n)) {
            for (i, out) in row.iter_mut().enumerate() {
                *out = matrix[i * n..(i + 1) * n].iter().zip(direction).map(|(a, b)| a * b).sum();
            }
        }
        out
    };
    for _ in 0..DIRECTION_ROUNDS {
        q = multiply(&q);
        straighten(&mut q, n);
    }
    // The matrix within the subspace, and its own directions there.
    let aq = multiply(&q);
    let small: Vec<f64> = (0..k)
        .flat_map(|i| {
            (0..k).map(|j| q[i * n..(i + 1) * n].iter().zip(&aq[j * n..(j + 1) * n]).map(|(a, b)| a * b).sum::<f64>()).collect::<Vec<_>>()
        })
        .collect();
    let (_, turns) = eigen(&small, k);
    (0..k).flat_map(|axis| (0..n).map(|i| (0..k).map(|j| turns[axis * k + j] * q[j * n + i]).sum::<f64>()).collect::<Vec<_>>()).collect()
}

/// Gram–Schmidt, twice over, on directions of `n` numbers: each at unit length, square to those before it.
fn straighten(directions: &mut [f64], n: usize) {
    let k = directions.len() / n;
    for _ in 0..2 {
        for i in 0..k {
            for j in 0..i {
                let dot: f64 = (0..n).map(|at| directions[i * n + at] * directions[j * n + at]).sum();
                for at in 0..n {
                    directions[i * n + at] -= dot * directions[j * n + at];
                }
            }
            let length = (0..n).map(|at| directions[i * n + at].powi(2)).sum::<f64>().sqrt();
            for at in 0..n {
                directions[i * n + at] = if length > 0. {
                    directions[i * n + at] / length
                } else if at == i % n {
                    1.
                } else {
                    0.
                };
            }
        }
    }
}

/// A number in [−1, 1] for `at`, the same every time (SplitMix64).
fn unit_noise(at: u64) -> f64 {
    let mut z = at.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64 * 2. - 1.
}

/// `most` of the points spread through them: in order along a curve through their three main directions (Morton's
/// Z-order, so neighbours on it are neighbours in them), every so many. Busier places get more, as they should.
fn spread(reduced: &[f32], k: usize, most: usize) -> Vec<usize> {
    let count = reduced.len() / k;
    let used = k.min(3);
    let (mut low, mut high) = (vec![f32::MAX; used], vec![f32::MIN; used]);
    for row in reduced.chunks(k) {
        for axis in 0..used {
            low[axis] = low[axis].min(row[axis]);
            high[axis] = high[axis].max(row[axis]);
        }
    }
    let mut keyed: Vec<(u64, usize)> = reduced
        .chunks(k)
        .enumerate()
        .map(|(at, row)| {
            let cell = |axis: usize| {
                let span = (high[axis] - low[axis]).max(f32::MIN_POSITIVE);
                (((row[axis] - low[axis]) / span * 1023.) as u64).min(1023)
            };
            let cells: Vec<u64> = (0..used).map(cell).collect();
            let mut key = 0u64;
            for bit in (0..10).rev() {
                for cell in &cells {
                    key = (key << 1) | ((cell >> bit) & 1);
                }
            }
            (key, at)
        })
        .collect();
    keyed.sort_unstable();
    let mut chosen: Vec<usize> = (0..most).map(|n| keyed[n * count / most].1).collect();
    chosen.sort_unstable();
    chosen
}

/// A squared distance, added up in eight lanes (so it's quick) in a fixed order (so it's the same everywhere): what
/// `distance_within` gives when it isn't cut short.
#[cfg(test)]
fn distance(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    let (a8, b8) = (a.chunks_exact(8), b.chunks_exact(8));
    let (a_rest, b_rest) = (a8.remainder(), b8.remainder());
    for (a, b) in a8.zip(b8) {
        for lane in 0..8 {
            let d = a[lane] - b[lane];
            lanes[lane] += d * d;
        }
    }
    let mut sum = lanes_sum(&lanes);
    for (a, b) in a_rest.iter().zip(b_rest) {
        sum += (a - b) * (a - b);
    }
    sum
}

fn lanes_sum(lanes: &[f32; 8]) -> f32 {
    ((lanes[0] + lanes[1]) + (lanes[2] + lanes[3])) + ((lanes[4] + lanes[5]) + (lanes[6] + lanes[7]))
}

/// `distance`, the same number to the bit, or None once it's sure to be more than `limit`: each lane only grows, and so
/// does their sum, so what they add up to so far is never more than the whole. The points' numbers come widest first,
/// so a point far off is known for one in a few eights.
fn distance_within(a: &[f32], b: &[f32], limit: f32) -> Option<f32> {
    let mut lanes = [0f32; 8];
    let (a8, b8) = (a.chunks_exact(8), b.chunks_exact(8));
    let (a_rest, b_rest) = (a8.remainder(), b8.remainder());
    for (a, b) in a8.zip(b8) {
        for lane in 0..8 {
            let d = a[lane] - b[lane];
            lanes[lane] += d * d;
        }
        if lanes_sum(&lanes) > limit {
            return None;
        }
    }
    let mut sum = lanes_sum(&lanes);
    for (a, b) in a_rest.iter().zip(b_rest) {
        sum += (a - b) * (a - b);
    }
    Some(sum)
}

/// The `most` points of `points` (rows of `k`) nearest `point`: squared distances and indexes, nearest first, ties by
/// index; `skip` leaves one out (the point itself).
fn nearest_to(point: &[f32], points: &[f32], k: usize, most: usize, skip: Option<usize>) -> Vec<(f32, u32)> {
    let mut best: Vec<(f32, u32)> = Vec::with_capacity(most + 1);
    for (at, row) in points.chunks(k).enumerate() {
        if skip == Some(at) {
            continue;
        }
        let farthest = if best.len() == most { best[most - 1].0 } else { f32::INFINITY };
        let Some(apart) = distance_within(point, row, farthest) else { continue };
        let candidate = (apart, at as u32);
        if best.len() == most && !closer(candidate, best[most - 1]) {
            continue;
        }
        let place = best.partition_point(|kept| closer(*kept, candidate));
        best.insert(place, candidate);
        best.truncate(most);
    }
    best
}

fn closer(a: (f32, u32), b: (f32, u32)) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1 < b.1)
}

/// The pull between neighbours: for each point (`entries[starts[at]..starts[at + 1]]`), its neighbours and how much each
/// pulls, the pairs' pulls summing to 1.
struct Pulls {
    starts: Vec<usize>,
    entries: Vec<(u32, f32)>,
}

/// Each point's neighbours, weighted by a Gaussian whose width gives it `perplexity` neighbours in effect, then made the
/// same both ways (i on j as j on i) and summing to 1.
fn pulls(points: &[f32], k: usize, perplexity: f64, threads: usize, going: &Going) -> Option<Pulls> {
    let n = points.len() / k;
    let near = ((3. * perplexity) as usize).clamp(1, n - 1);
    // Each point's nearest, and its weights on them.
    let forward: Vec<(u32, f32)> = fill_rows(
        threads,
        n,
        near,
        || (),
        |_, at, row: &mut [(u32, f32)]| {
            if !going.on() {
                return;
            }
            let nearest = nearest_to(&points[at * k..(at + 1) * k], points, k, near, Some(at));
            let distances: Vec<f64> = nearest.iter().map(|(distance, _)| *distance as f64).collect();
            for (slot, ((_, other), weight)) in row.iter_mut().zip(nearest.iter().zip(calibrated(&distances, perplexity))) {
                *slot = (*other, weight as f32);
            }
        },
    );
    if going.stopped() {
        return None;
    }
    // Each pair both ways, a row a point: its own weights, then those its neighbours give it.
    let mut sizes = vec![near; n];
    for (other, _) in &forward {
        sizes[*other as usize] += 1;
    }
    let mut starts = Vec::with_capacity(n + 1);
    let mut total = 0;
    for size in &sizes {
        starts.push(total);
        total += size;
    }
    starts.push(total);
    let mut entries = vec![(0u32, 0f32); total];
    let mut next: Vec<usize> = starts[..n].to_vec();
    for (at, row) in forward.chunks(near).enumerate() {
        entries[next[at]..next[at] + near].copy_from_slice(row);
        next[at] += near;
    }
    for (at, row) in forward.chunks(near).enumerate() {
        for (other, weight) in row {
            entries[next[*other as usize]] = (at as u32, *weight);
            next[*other as usize] += 1;
        }
    }
    drop(forward);
    // Each row by neighbour, a pair's two weights added, written back in place from the start.
    let scale = 1. / (2. * n as f64);
    let mut written = 0;
    for at in 0..n {
        let (first, last) = (starts[at], starts[at + 1]);
        entries[first..last].sort_by_key(|(other, _)| *other);
        starts[at] = written;
        let mut read = first;
        while read < last {
            let (other, mut weight) = entries[read];
            read += 1;
            while read < last && entries[read].0 == other {
                weight += entries[read].1;
                read += 1;
            }
            entries[written] = (other, (weight as f64 * scale) as f32);
            written += 1;
        }
    }
    starts[n] = written;
    entries.truncate(written);
    Some(Pulls { starts, entries })
}

/// A point's weights on its neighbours (by squared distance, nearest first): a Gaussian's, its width found by halving
/// until it gives `perplexity` neighbours in effect.
fn calibrated(distances: &[f64], perplexity: f64) -> Vec<f64> {
    let base = distances.first().copied().unwrap_or(0.);
    let wanted = perplexity.ln();
    let (mut beta, mut low, mut high) = (1f64, 0f64, f64::INFINITY);
    let mut weights = vec![0f64; distances.len()];
    for _ in 0..200 {
        for (weight, distance) in weights.iter_mut().zip(distances) {
            *weight = (-beta * (distance - base)).exp();
        }
        let sum: f64 = weights.iter().sum::<f64>().max(f64::MIN_POSITIVE);
        let entropy =
            sum.ln() + beta * weights.iter().zip(distances).map(|(weight, distance)| weight * (distance - base)).sum::<f64>() / sum;
        if (entropy - wanted).abs() < 1e-5 {
            break;
        }
        if entropy > wanted {
            low = beta;
            beta = if high.is_infinite() { beta * 2. } else { (beta + high) / 2. };
        } else {
            high = beta;
            beta = if low == 0. { beta / 2. } else { (beta + low) / 2. };
        }
    }
    let sum: f64 = weights.iter().sum::<f64>().max(f64::MIN_POSITIVE);
    weights.iter().map(|weight| weight / sum).collect()
}

/// t-SNE on points (rows of `k` numbers): their places in two dimensions.
fn tsne(points: &[f32], k: usize, how: &Layout, threads: usize, going: &Going) -> Option<Vec<[f64; 2]>> {
    let n = points.len() / k;
    // It starts from the first two main directions, small.
    let mut places: Vec<[f64; 2]> = points.chunks(k).map(|row| [row[0] as f64, row.get(1).copied().unwrap_or(0.) as f64]).collect();
    if n < 4 {
        return Some(places);
    }
    let mean = places.iter().map(|place| place[0]).sum::<f64>() / n as f64;
    let wide = (places.iter().map(|place| (place[0] - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let scale = if wide > 0. { 1e-4 / wide } else { 1. };
    places.iter_mut().for_each(|place| *place = [place[0] * scale, place[1] * scale]);
    let perplexity = how.perplexity.min((n - 1) as f64 / 3.).max(1.);
    let pulls = pulls(points, k, perplexity, threads, going)?;
    // Steps as big as the points are many (Belkina et al., 2019), with each place's own gains.
    let rate = (n as f64 / EXAGGERATION / 4.).max(50.);
    let exaggerated = EXAGGERATED.min(how.rounds / 3);
    let (mut moves, mut gains) = (vec![[0f64; 2]; n], vec![[1f64; 2]; n]);
    for round in 0..how.rounds {
        if !going.on() {
            return None;
        }
        let (exaggeration, momentum) = if round < exaggerated { (EXAGGERATION, 0.5) } else { (1., 0.8) };
        let tree = Tree::of(&places);
        // Each point's pull from its neighbours and push from the rest, and its part of the push's sum.
        let forces = fill_rows(
            threads,
            n,
            5,
            || (),
            |_, at, force: &mut [f64]| {
                let here = places[at];
                let mut pull = [0f64; 2];
                for (other, weight) in &pulls.entries[pulls.starts[at]..pulls.starts[at + 1]] {
                    let there = places[*other as usize];
                    let (dx, dy) = (here[0] - there[0], here[1] - there[1]);
                    let weight = *weight as f64 / (1. + dx * dx + dy * dy);
                    pull[0] += weight * dx;
                    pull[1] += weight * dy;
                }
                let (push, sum) = tree.push(&places, at);
                force.copy_from_slice(&[pull[0], pull[1], push[0], push[1], sum]);
            },
        );
        let sum: f64 = forces.chunks(5).map(|force| force[4]).sum::<f64>().max(f64::MIN_POSITIVE);
        for (at, force) in forces.chunks(5).enumerate() {
            for axis in 0..2 {
                let gradient = 4. * (exaggeration * force[axis] - force[2 + axis] / sum);
                let gain = &mut gains[at][axis];
                *gain = if (gradient > 0.) != (moves[at][axis] > 0.) { *gain + 0.2 } else { *gain * 0.8 };
                *gain = gain.max(0.01);
                moves[at][axis] = momentum * moves[at][axis] - rate * *gain * gradient;
                places[at][axis] += moves[at][axis];
            }
        }
        let centre = places.iter().fold([0f64; 2], |sum, place| [sum[0] + place[0], sum[1] + place[1]]);
        let centre = [centre[0] / n as f64, centre[1] / n as f64];
        places.iter_mut().for_each(|place| *place = [place[0] - centre[0], place[1] - centre[1]]);
    }
    Some(places)
}

/// A quadtree over the places, for Barnes–Hut: each cell's centre of mass, its count, its width, and its points or
/// its four quarters.
struct Tree {
    cells: Vec<Cell>,
    order: Vec<u32>,
}

struct Cell {
    mass: [f64; 2],
    count: usize,
    width: f64,
    /// The cell's quarters (0: none; the root is never one), or for a leaf its points, `order[first..last]`.
    quarters: [u32; 4],
    first: usize,
    last: usize,
}

impl Tree {
    fn of(places: &[[f64; 2]]) -> Tree {
        let (mut low, mut high) = ([f64::MAX; 2], [f64::MIN; 2]);
        for place in places {
            for axis in 0..2 {
                low[axis] = low[axis].min(place[axis]);
                high[axis] = high[axis].max(place[axis]);
            }
        }
        let half = ((high[0] - low[0]).max(high[1] - low[1]) / 2.).max(1e-9) * (1. + 1e-9);
        let centre = [(low[0] + high[0]) / 2., (low[1] + high[1]) / 2.];
        let mut tree = Tree { cells: vec![], order: (0..places.len() as u32).collect() };
        let mut spare = vec![0u32; places.len()];
        tree.build(places, &mut spare, 0, places.len(), centre, half, 0);
        tree
    }

    fn build(
        &mut self,
        places: &[[f64; 2]],
        spare: &mut [u32],
        first: usize,
        last: usize,
        centre: [f64; 2],
        half: f64,
        depth: usize,
    ) -> u32 {
        let at = self.cells.len();
        let count = last - first;
        let mut mass = [0f64; 2];
        for point in &self.order[first..last] {
            mass[0] += places[*point as usize][0];
            mass[1] += places[*point as usize][1];
        }
        mass = [mass[0] / count as f64, mass[1] / count as f64];
        self.cells.push(Cell { mass, count, width: 2. * half, quarters: [0; 4], first, last });
        if count <= 1 || depth >= 48 || half < 1e-12 {
            return at as u32;
        }
        // The points by quarter, each quarter's in the order they were.
        let quarter = |point: u32| {
            let place = places[point as usize];
            (place[0] >= centre[0]) as usize + 2 * (place[1] >= centre[1]) as usize
        };
        let mut counts = [0usize; 4];
        for point in &self.order[first..last] {
            counts[quarter(*point)] += 1;
        }
        let mut starts = [first, 0, 0, 0];
        for q in 1..4 {
            starts[q] = starts[q - 1] + counts[q - 1];
        }
        let mut next = starts;
        for point in &self.order[first..last] {
            let q = quarter(*point);
            spare[next[q]] = *point;
            next[q] += 1;
        }
        self.order[first..last].copy_from_slice(&spare[first..last]);
        for q in 0..4 {
            if counts[q] == 0 {
                continue;
            }
            let offset = half / 2.;
            let centre = [centre[0] + if q & 1 == 1 { offset } else { -offset }, centre[1] + if q & 2 == 2 { offset } else { -offset }];
            let cell = self.build(places, spare, starts[q], starts[q] + counts[q], centre, offset, depth + 1);
            self.cells[at].quarters[q] = cell;
        }
        at as u32
    }

    /// How the other points push the one `at`: the push (unnormalized) and its part of the sum that normalizes it.
    fn push(&self, places: &[[f64; 2]], at: usize) -> ([f64; 2], f64) {
        let here = places[at];
        let (mut push, mut sum) = ([0f64; 2], 0f64);
        let mut stack: Vec<u32> = vec![0];
        while let Some(cell) = stack.pop() {
            let cell = &self.cells[cell as usize];
            let leaf = cell.quarters == [0; 4];
            if leaf {
                for point in &self.order[cell.first..cell.last] {
                    if *point as usize == at {
                        continue;
                    }
                    let there = places[*point as usize];
                    let (dx, dy) = (here[0] - there[0], here[1] - there[1]);
                    let weight = 1. / (1. + dx * dx + dy * dy);
                    sum += weight;
                    push[0] += weight * weight * dx;
                    push[1] += weight * weight * dy;
                }
                continue;
            }
            let (dx, dy) = (here[0] - cell.mass[0], here[1] - cell.mass[1]);
            let apart = dx * dx + dy * dy;
            if cell.width * cell.width < THETA * THETA * apart {
                let weight = 1. / (1. + apart);
                let count = cell.count as f64;
                sum += count * weight;
                push[0] += count * weight * weight * dx;
                push[1] += count * weight * weight * dy;
                continue;
            }
            for quarter in cell.quarters.iter().rev() {
                if *quarter != 0 {
                    stack.push(*quarter);
                }
            }
        }
        (push, sum)
    }
}

/// Places in [0, 1] on both axes, the map's shape kept and centred.
fn unit_square(places: &[[f64; 2]]) -> Vec<[f32; 2]> {
    let (mut low, mut high) = ([f64::MAX; 2], [f64::MIN; 2]);
    for place in places {
        for axis in 0..2 {
            low[axis] = low[axis].min(place[axis]);
            high[axis] = high[axis].max(place[axis]);
        }
    }
    let span = (high[0] - low[0]).max(high[1] - low[1]);
    if !(span > 0.) {
        return vec![[0.5, 0.5]; places.len()];
    }
    let margin = [(span - (high[0] - low[0])) / 2., (span - (high[1] - low[1])) / 2.];
    places
        .iter()
        .map(|place| {
            let unit = |axis: usize| (((place[axis] - low[axis] + margin[axis]) / span) as f32).clamp(0., 1.);
            [unit(0), unit(1)]
        })
        .collect()
}

/// `chunks` pieces of work shared by `threads`, each piece's answer kept in the pieces' order, so what's added up from
/// them comes out the same however the work was shared.
fn in_chunks<T: Send>(threads: usize, chunks: usize, work: impl Fn(usize) -> T + Sync) -> Vec<T> {
    if threads <= 1 || chunks <= 1 {
        return (0..chunks).map(&work).collect();
    }
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, T)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.min(chunks))
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = vec![];
                    loop {
                        let chunk = next.fetch_add(1, Ordering::Relaxed);
                        if chunk >= chunks {
                            break mine;
                        }
                        mine.push((chunk, work(chunk)));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().expect("a layout thread stopped")).collect()
    });
    done.sort_by_key(|(chunk, _)| *chunk);
    done.into_iter().map(|(_, answer)| answer).collect()
}

/// `rows` rows of `width` values, each written by `fill` (given a scratch of its thread's own, the row's index and the
/// row), the rows split among `threads` in fixed parts: each row comes out the same however they're shared.
fn fill_rows<T: Clone + Default + Send, S>(
    threads: usize,
    rows: usize,
    width: usize,
    scratch: impl Fn() -> S + Sync,
    fill: impl Fn(&mut S, usize, &mut [T]) + Sync,
) -> Vec<T> {
    assert!(width > 0);
    let mut out = vec![T::default(); rows * width];
    let parts = threads.min(rows.div_ceil(CHUNK)).max(1);
    if parts == 1 {
        let mut own = scratch();
        out.chunks_mut(width).enumerate().for_each(|(at, row)| fill(&mut own, at, row));
        return out;
    }
    std::thread::scope(|scope| {
        let mut rest: &mut [T] = &mut out;
        for part in 0..parts {
            let (first, last) = (rows * part / parts, rows * (part + 1) / parts);
            let (mine, after) = std::mem::take(&mut rest).split_at_mut((last - first) * width);
            rest = after;
            let (scratch, fill) = (&scratch, &fill);
            scope.spawn(move || {
                let mut own = scratch();
                mine.chunks_mut(width).enumerate().for_each(|(offset, row)| fill(&mut own, first + offset, row));
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_finds_a_symmetric_matrixs_directions_largest_first() {
        let (values, vectors) = eigen(&[2., 1., 1., 2.], 2);
        assert!((values[0] - 3.).abs() < 1e-12 && (values[1] - 1.).abs() < 1e-12, "{values:?}");
        let half = 0.5f64.sqrt();
        assert!((vectors[0].abs() - half).abs() < 1e-12 && (vectors[0] - vectors[1]).abs() < 1e-12, "{vectors:?}");
    }

    #[test]
    fn the_widest_directions_of_many_numbers_are_found() {
        // Wide along numbers 3 and 7, narrow elsewhere: the two widest directions are those.
        let n = 60;
        let mut matrix = vec![0f64; n * n];
        (0..n).for_each(|i| matrix[i * n + i] = 0.01 + i as f64 * 1e-4);
        matrix[3 * n + 3] = 9.;
        matrix[7 * n + 7] = 4.;
        let axes = widest(&matrix, n, 5);
        assert!((axes[3].abs() - 1.).abs() < 1e-6, "{:?}", &axes[..n]);
        assert!((axes[n + 7].abs() - 1.).abs() < 1e-6, "{:?}", &axes[n..2 * n]);
    }

    #[test]
    fn a_squared_distance_adds_every_number() {
        let a: Vec<f32> = (0..13).map(|at| at as f32).collect();
        let b = vec![0f32; 13];
        assert_eq!(distance(&a, &b), (0..13).map(|at| (at * at) as f32).sum::<f32>());
    }

    #[test]
    fn a_distance_cut_short_is_the_same_to_the_bit_or_more_than_its_limit() {
        // Points of 50 numbers, a pseudo-random walk, each pair's distance against limits around it.
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let points: Vec<Vec<f32>> = (0..60).map(|_| (0..50).map(|_| next()).collect()).collect();
        for a in &points {
            for b in &points {
                let whole = distance(a, b);
                for limit in [f32::INFINITY, whole, whole * 1.0001, whole * 0.5, 0.] {
                    match distance_within(a, b, limit) {
                        Some(within) => assert_eq!(within.to_bits(), whole.to_bits()),
                        None => assert!(whole > limit, "{whole} cut short at {limit}"),
                    }
                }
                assert_eq!(distance_within(a, b, whole), Some(whole), "a tie isn't cut short");
            }
        }
    }

    #[test]
    fn a_landmark_spread_takes_from_everywhere_in_order() {
        // Two groups along the first direction, a hundred points and ten: a tenth of them comes from each.
        let mut reduced = vec![];
        for at in 0..110 {
            reduced.extend([if at < 100 { at as f32 * 0.01 } else { 10. + at as f32 * 0.01 }, 0., 0.]);
        }
        let chosen = spread(&reduced, 3, 11);
        assert_eq!(chosen.len(), 11);
        assert_eq!(chosen.iter().filter(|at| **at >= 100).count(), 1, "{chosen:?}");
        assert!(chosen.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
