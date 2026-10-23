//! A small CMA-ES for a few knobs that interact (a compressor's attack, release and ratio; a reverb's size, decay and
//! damping): generations of a handful of candidates, each generation heard in one pass, the search's shape learned
//! from which candidates did best. Knobs are searched in [0, 1] of their perceptual range.
//!
//! Points from elsewhere (what worked on the same device before) can be injected: a generation hears them in place of as
//! many of its own samples, as they are, and they're repaired as Hansen's "Injecting External Solutions Into CMA-ES"
//! (2011, INRIA RR-7748, arXiv:1110.4181, Figure 1) repairs them: an injected point's step y = (x − m)/σ is scaled by
//! min(1, c_y / ‖C^−½ y‖), c_y = √n + 2n/(n + 2), before the mean and the shape learn from it (its equation 3), and a
//! generation with injected points changes σ by at most a factor e (Δσ^max = 1, its equation 11).

/// The search: its mean, step size and shape, and the best candidate heard.
pub struct Cmaes {
    n: usize,
    pub lambda: usize,
    mu: usize,
    weights: Vec<f64>,
    mueff: f64,
    cc: f64,
    cs: f64,
    c1: f64,
    cmu: f64,
    damps: f64,
    chi_n: f64,
    pub mean: Vec<f64>,
    pub sigma: f64,
    c: Vec<Vec<f64>>,
    b: Vec<Vec<f64>>,
    d: Vec<f64>,
    pc: Vec<f64>,
    ps: Vec<f64>,
    pub generation: u32,
    random: Box<dyn FnMut() -> f64>,
    spare: Option<f64>,
    pub best: Option<(Vec<f64>, f64)>,
    /// Points the next generation hears in place of its own samples, and the ones the last generation heard.
    waiting: Vec<Vec<f64>>,
    injected: Vec<Vec<f64>>,
}

impl Cmaes {
    /// From `start` (each in [0, 1]) with step `sigma`; `lambda` candidates a generation (4 + 3 ln n without one).
    pub fn new(start: &[f64], sigma: f64, lambda: Option<usize>, random: impl FnMut() -> f64 + 'static) -> Self {
        let n = start.len().max(1);
        let lambda = lambda.unwrap_or(4 + (3. * (n as f64).ln()).floor() as usize).max(2);
        let mu = lambda / 2;
        let raw: Vec<f64> = (0..mu).map(|i| ((mu as f64) + 0.5).ln() - ((i + 1) as f64).ln()).collect();
        let sum: f64 = raw.iter().sum();
        let weights: Vec<f64> = raw.iter().map(|w| w / sum).collect();
        let mueff = 1. / weights.iter().map(|w| w * w).sum::<f64>();
        let nf = n as f64;
        let cc = (4. + mueff / nf) / (nf + 4. + 2. * mueff / nf);
        let cs = (mueff + 2.) / (nf + mueff + 5.);
        let c1 = 2. / ((nf + 1.3).powi(2) + mueff);
        let cmu = (1. - c1).min(2. * (mueff - 2. + 1. / mueff) / ((nf + 2.).powi(2) + mueff));
        let damps = 1. + 2. * (((mueff - 1.) / (nf + 1.)).sqrt() - 1.).max(0.) + cs;
        let chi_n = nf.sqrt() * (1. - 1. / (4. * nf) + 1. / (21. * nf * nf));
        let identity: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1. } else { 0. }).collect()).collect();
        Self {
            n,
            lambda,
            mu,
            weights,
            mueff,
            cc,
            cs,
            c1,
            cmu,
            damps,
            chi_n,
            mean: start.iter().map(|x| x.clamp(0., 1.)).collect(),
            sigma,
            c: identity.clone(),
            b: identity,
            d: vec![1.; n],
            pc: vec![0.; n],
            ps: vec![0.; n],
            generation: 0,
            random: Box::new(random),
            spare: None,
            best: None,
            waiting: vec![],
            injected: vec![],
        }
    }

    /// Points for the next generation to hear in place of as many of its own samples (at most a generation's worth),
    /// each in [0, 1]: heard as they are, repaired when the search learns from them (`tell`).
    pub fn inject(&mut self, points: &[Vec<f64>]) {
        let room = self.lambda.saturating_sub(self.waiting.len());
        self.waiting.extend(points.iter().take(room).map(|point| {
            let mut point: Vec<f64> = point.iter().take(self.n).map(|x| x.clamp(0., 1.)).collect();
            point.resize(self.n, 0.5);
            point
        }));
    }

    /// A step's length in the search's own shape (its Mahalanobis length, ‖C^−½ y‖).
    fn shaped_length(&self, step: &[f64]) -> f64 {
        let n = self.n;
        (0..n).map(|j| ((0..n).map(|i| self.b[i][j] * step[i]).sum::<f64>() / self.d[j].max(1e-12)).powi(2)).sum::<f64>().sqrt()
    }

    /// An injected point's step from the mean (in units of σ), as the search learns from it: scaled down to a Mahalanobis
    /// length of at most c_y = √n + 2n/(n + 2), so a far one can't pull the search further than its own samples would.
    pub fn repaired(&self, point: &[f64]) -> Vec<f64> {
        let step: Vec<f64> = (0..self.n).map(|i| (point[i] - self.mean[i]) / self.sigma).collect();
        let nf = self.n as f64;
        let most = nf.sqrt() + 2. * nf / (nf + 2.);
        let length = self.shaped_length(&step);
        let scale = if length > most { most / length } else { 1. };
        step.iter().map(|y| y * scale).collect()
    }

    fn gaussian(&mut self) -> f64 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        // Box–Muller.
        let u = (self.random)().max(1e-12);
        let v = (self.random)();
        let radius = (-2. * u.ln()).sqrt();
        let angle = 2. * std::f64::consts::PI * v;
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }

    /// A generation's candidates, each in [0, 1] (drawn again when one falls outside, then held at the edge): the points
    /// injected for it first, then its own samples.
    pub fn ask(&mut self) -> Vec<Vec<f64>> {
        self.injected = std::mem::take(&mut self.waiting);
        let own = self.lambda - self.injected.len();
        let mut points = self.injected.clone();
        points.extend((0..own).map(|_| {
            let mut point = vec![0.; self.n];
            for attempt in 0..20 {
                let z: Vec<f64> = (0..self.n).map(|_| self.gaussian()).collect();
                for (i, value) in point.iter_mut().enumerate() {
                    let y: f64 = (0..self.n).map(|j| self.b[i][j] * self.d[j] * z[j]).sum();
                    *value = self.mean[i] + self.sigma * y;
                }
                if point.iter().all(|x| (0. ..=1.).contains(x)) || attempt == 19 {
                    break;
                }
            }
            point.iter().map(|x| x.clamp(0., 1.)).collect::<Vec<f64>>()
        }));
        points
    }

    /// What a generation's candidates cost (lower is better): the mean moves toward the best, and the shape learns.
    pub fn tell(&mut self, points: &[Vec<f64>], costs: &[f64]) {
        let mut order: Vec<usize> = (0..points.len().min(costs.len())).collect();
        order.sort_by(|a, b| costs[*a].total_cmp(&costs[*b]));
        if let Some(&first) = order.first() {
            if self.best.as_ref().is_none_or(|(_, cost)| costs[first] < *cost) {
                self.best = Some((points[first].clone(), costs[first]));
            }
        }
        let mu = self.mu.min(order.len());
        if mu == 0 {
            return;
        }
        let n = self.n;
        let old = self.mean.clone();
        let weights: Vec<f64> = {
            let sum: f64 = self.weights[..mu].iter().sum();
            self.weights[..mu].iter().map(|w| w / sum).collect()
        };
        // Each chosen candidate's step from the mean (in units of σ), an injected one's repaired.
        let injected = std::mem::take(&mut self.injected);
        let ys: Vec<Vec<f64>> = order[..mu]
            .iter()
            .map(|k| match injected.contains(&points[*k]) {
                true => self.repaired(&points[*k]),
                false => (0..n).map(|i| (points[*k][i] - old[i]) / self.sigma).collect(),
            })
            .collect();
        for i in 0..n {
            self.mean[i] = old[i] + self.sigma * ys.iter().zip(&weights).map(|(y, w)| w * y[i]).sum::<f64>();
        }
        let step: Vec<f64> = (0..n).map(|i| (self.mean[i] - old[i]) / self.sigma).collect();
        // C^(-1/2) · step = B · D^-1 · Bᵀ · step
        let projected: Vec<f64> = (0..n).map(|j| (0..n).map(|i| self.b[i][j] * step[i]).sum::<f64>() / self.d[j].max(1e-12)).collect();
        let whitened: Vec<f64> = (0..n).map(|i| (0..n).map(|j| self.b[i][j] * projected[j]).sum()).collect();
        let cs_factor = (self.cs * (2. - self.cs) * self.mueff).sqrt();
        for i in 0..n {
            self.ps[i] = (1. - self.cs) * self.ps[i] + cs_factor * whitened[i];
        }
        let ps_norm = self.ps.iter().map(|x| x * x).sum::<f64>().sqrt();
        self.generation += 1;
        let decay = (1. - (1. - self.cs).powi(2 * self.generation as i32)).sqrt();
        let hsig = if ps_norm / decay / self.chi_n < 1.4 + 2. / (n as f64 + 1.) { 1. } else { 0. };
        let cc_factor = (self.cc * (2. - self.cc) * self.mueff).sqrt();
        for i in 0..n {
            self.pc[i] = (1. - self.cc) * self.pc[i] + hsig * cc_factor * step[i];
        }
        for i in 0..n {
            for j in 0..n {
                let rank_one = self.pc[i] * self.pc[j] + (1. - hsig) * self.cc * (2. - self.cc) * self.c[i][j];
                let rank_mu: f64 = ys.iter().zip(&weights).map(|(y, w)| w * y[i] * y[j]).sum();
                self.c[i][j] = (1. - self.c1 - self.cmu) * self.c[i][j] + self.c1 * rank_one + self.cmu * rank_mu;
            }
        }
        // With injected points, σ changes by at most a factor e a generation (Δσ^max = 1).
        let change = (self.cs / self.damps) * (ps_norm / self.chi_n - 1.);
        let change = if injected.is_empty() { change } else { change.min(1.) };
        self.sigma = (self.sigma * change.exp()).clamp(1e-4, 0.5);
        let (values, vectors) = eigen(&self.c);
        self.d = values.iter().map(|v| v.max(1e-12).sqrt()).collect();
        self.b = vectors;
    }
}

/// A symmetric matrix's eigenvalues and eigenvectors (as columns), by Jacobi rotations.
fn eigen(matrix: &[Vec<f64>]) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = matrix.len();
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut v: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1. } else { 0. }).collect()).collect();
    for _ in 0..64 {
        let off: f64 = (0..n).flat_map(|i| (0..n).filter(move |j| *j != i).map(move |j| (i, j))).map(|(i, j)| a[i][j] * a[i][j]).sum();
        if off < 1e-18 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                if a[p][q].abs() < 1e-15 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2. * a[p][q]);
                let t = if theta >= 0. { 1. } else { -1. } / (theta.abs() + (theta * theta + 1.).sqrt());
                let c = 1. / (t * t + 1.).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k][p], v[k][q]);
                    v[k][p] = c * vkp - s * vkq;
                    v[k][q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| a[i][i]).collect(), v)
}

/// Mulberry32, with the same wrapping integer arithmetic as JavaScript's Math.imul.
pub fn seeded(seed: u32) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x6d2b79f5);
        let mut value = (state ^ (state >> 15)).wrapping_mul(1 | state);
        value = value.wrapping_add((value ^ (value >> 7)).wrapping_mul(61 | value)) ^ value;
        (value ^ (value >> 14)) as f64 / 4294967296.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_points_are_heard_first_as_they_are() {
        let mut search = Cmaes::new(&[0.5, 0.5, 0.5], 0.2, None, seeded(3));
        search.inject(&[vec![1.2, 1.0, 1.0], vec![0.55, 0.5]]);
        let points = search.ask();
        assert_eq!(points.len(), search.lambda);
        assert_eq!(points[..2], [vec![1.0, 1.0, 1.0], vec![0.55, 0.5, 0.5]], "first, held in range, one value a knob");
        assert!(points[2..].iter().all(|point| point.iter().all(|x| (0. ..=1.).contains(x))));
        // Only the next generation hears them.
        search.tell(&points, &vec![1.; points.len()]);
        let next = search.ask();
        assert!(!next.contains(&points[0]) && !next.contains(&points[1]));
        // At most a generation's worth.
        search.inject(&vec![vec![0.1, 0.1, 0.1]; 20]);
        assert_eq!(search.ask().iter().filter(|point| **point == [0.1, 0.1, 0.1]).count(), search.lambda);
    }

    #[test]
    fn a_far_injected_point_is_repaired_to_the_mahalanobis_length_hansen_allows() {
        let search = Cmaes::new(&[0.5, 0.5, 0.5], 0.2, None, seeded(3));
        let most = 3f64.sqrt() + 6. / 5.;
        // (1 − 0.5) / 0.2 = 2.5 a knob, √3 · 2.5 ≈ 4.33 long: scaled down to c_y, its direction kept.
        let far = search.repaired(&[1.0, 1.0, 1.0]);
        assert!((search.shaped_length(&far) - most).abs() < 1e-9, "{far:?}");
        assert!(far.iter().all(|y| (y - most / 3f64.sqrt()).abs() < 1e-9));
        // A near one is left as it is.
        let near = search.repaired(&[0.55, 0.5, 0.45]);
        assert!(near.iter().zip([0.25, 0., -0.25]).all(|(y, was)| (y - was).abs() < 1e-12), "{near:?}");
    }

    #[test]
    fn the_search_learns_from_an_injected_point_only_as_far_as_its_repaired_step() {
        let mut search = Cmaes::new(&[0.5, 0.5, 0.5], 0.2, None, seeded(3));
        search.inject(&[vec![1.0, 1.0, 1.0]]);
        let points = search.ask();
        // The injected point heard best, then the others in their order.
        let costs: Vec<f64> = (0..points.len()).map(|n| n as f64).collect();
        let (old, sigma, mu) = (search.mean.clone(), search.sigma, search.mu);
        let weights: Vec<f64> = search.weights[..mu].iter().map(|w| w / search.weights[..mu].iter().sum::<f64>()).collect();
        let far = search.repaired(&points[0]);
        search.tell(&points, &costs);
        for i in 0..3 {
            let step = weights[0] * far[i] + (1..mu).map(|k| weights[k] * (points[k][i] - old[i]) / sigma).sum::<f64>();
            assert!((search.mean[i] - (old[i] + sigma * step)).abs() < 1e-12, "knob {i}: {} vs {}", search.mean[i], old[i] + sigma * step);
        }
        assert_eq!(search.best, Some((vec![1.0, 1.0, 1.0], 0.)), "heard as it was, it's the best");
    }
}
