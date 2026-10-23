//! The producer's music in a few styles: mini-batch k-means over the songs' style vectors, by direction (each vector
//! and centre at unit length), with a fixed seed, so the same songs make the same clusters. Eight clusters, fewer
//! under 400 songs; the biggest is the first.

use crate::cmaes::seeded;

/// The clusters' seed.
pub const SEED: u32 = 0x6b75_6d69;
/// Clusters at most.
pub const MOST: usize = 8;
const BATCH: usize = 1_024;
const ROUNDS: usize = 100;
/// Points the first centres are chosen among.
const FIRST_FROM: usize = 2_000;

/// How many clusters for `songs` heard songs: eight, one for each 50 songs under 400.
pub fn clusters_for(songs: usize) -> usize {
    if songs >= 400 {
        MOST
    } else {
        (songs / 50).clamp(1, MOST)
    }
}

fn unit(vector: &mut [f32]) {
    let length = vector.iter().map(|value| (*value as f64).powi(2)).sum::<f64>().sqrt();
    if length > 0. {
        vector.iter_mut().for_each(|value| *value = (*value as f64 / length) as f32);
    }
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(a, b)| *a as f64 * *b as f64).sum()
}

fn nearest(centres: &[Vec<f32>], point: &[f32]) -> usize {
    let mut best = (0, f64::NEG_INFINITY);
    for (at, centre) in centres.iter().enumerate() {
        let alike = dot(centre, point);
        if alike > best.1 {
            best = (at, alike);
        }
    }
    best.0
}

/// `count` points (each read into a buffer of `dims` by `read`) in `k` clusters: the centres (unit length, the biggest
/// cluster first) and each point's cluster.
pub fn kmeans(count: usize, dims: usize, k: usize, read: &dyn Fn(usize, &mut [f32]), seed: u32) -> (Vec<Vec<f32>>, Vec<usize>) {
    let k = k.min(count);
    if k == 0 {
        return (vec![], vec![]);
    }
    let mut random = seeded(seed);
    let pick = |random: &mut dyn FnMut() -> f64, below: usize| ((random() * below as f64) as usize).min(below - 1);
    let point = |at: usize| {
        let mut buffer = vec![0f32; dims];
        read(at, &mut buffer);
        unit(&mut buffer);
        buffer
    };
    // The first centres, spread (k-means++): each next one likelier the farther it is from those chosen.
    let candidates: Vec<Vec<f32>> = if count <= FIRST_FROM {
        (0..count).map(point).collect()
    } else {
        (0..FIRST_FROM).map(|_| point(pick(&mut random, count))).collect()
    };
    let mut centres = vec![candidates[pick(&mut random, candidates.len())].clone()];
    while centres.len() < k {
        let far: Vec<f64> =
            candidates.iter().map(|candidate| (2. - 2. * dot(candidate, &centres[nearest(&centres, candidate)])).max(0.)).collect();
        let total: f64 = far.iter().sum();
        let next = if total <= 0. {
            pick(&mut random, candidates.len())
        } else {
            let mut wanted = random() * total;
            far.iter()
                .position(|far| {
                    wanted -= far;
                    wanted <= 0.
                })
                .unwrap_or(candidates.len() - 1)
        };
        centres.push(candidates[next].clone());
    }
    // Mini-batches: each point pulls its centre toward it, by less as the centre has taken more.
    let mut taken = vec![0usize; k];
    for _ in 0..ROUNDS {
        for _ in 0..BATCH.min(count) {
            let at = point(pick(&mut random, count));
            let centre = nearest(&centres, &at);
            taken[centre] += 1;
            let step = 1. / taken[centre] as f32;
            for (value, pulled) in centres[centre].iter_mut().zip(&at) {
                *value += step * (pulled - *value);
            }
        }
        centres.iter_mut().for_each(|centre| unit(centre));
    }
    let assigned: Vec<usize> = (0..count).map(|at| nearest(&centres, &point(at))).collect();
    // The biggest first: c1 is the producer's main style.
    let mut sizes: Vec<(usize, usize)> = (0..k).map(|cluster| (cluster, assigned.iter().filter(|got| **got == cluster).count())).collect();
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut renamed = vec![0; k];
    for (new, (old, _)) in sizes.iter().enumerate() {
        renamed[*old] = new;
    }
    let ordered: Vec<Vec<f32>> = sizes.iter().map(|(old, _)| centres[*old].clone()).collect();
    (ordered, assigned.into_iter().map(|cluster| renamed[cluster]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points around `groups` directions, a little apart.
    fn points(groups: usize, each: usize) -> Vec<Vec<f32>> {
        let mut random = seeded(7);
        let centres: Vec<Vec<f32>> = (0..groups).map(|_| (0..16).map(|_| random() as f32 - 0.5).collect()).collect();
        (0..groups * each).map(|at| centres[at % groups].iter().map(|value| value + (random() as f32 - 0.5) * 0.05).collect()).collect()
    }

    #[test]
    fn separate_groups_are_separate_clusters_the_same_each_time() {
        let all = points(4, 60);
        let read = |at: usize, buffer: &mut [f32]| buffer.copy_from_slice(&all[at]);
        let (centres, assigned) = kmeans(all.len(), 16, 4, &read, SEED);
        assert_eq!(centres.len(), 4);
        for group in 0..4 {
            let clusters: std::collections::BTreeSet<usize> = (0..all.len()).filter(|at| at % 4 == group).map(|at| assigned[at]).collect();
            assert_eq!(clusters.len(), 1, "group {group} is one cluster: {clusters:?}");
        }
        assert_eq!(kmeans(all.len(), 16, 4, &read, SEED), (centres, assigned), "a fixed seed: the same clusters");
    }

    #[test]
    fn fewer_songs_fewer_clusters() {
        assert_eq!((clusters_for(10), clusters_for(120), clusters_for(399), clusters_for(400), clusters_for(300_000)), (1, 2, 7, 8, 8));
        let read = |_: usize, buffer: &mut [f32]| buffer.fill(1.);
        assert_eq!(kmeans(3, 4, 8, &read, SEED).0.len(), 3);
        assert_eq!(kmeans(0, 4, 8, &read, SEED), (vec![], vec![]));
    }
}
