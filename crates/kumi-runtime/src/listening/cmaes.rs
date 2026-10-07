//! A small CMA-ES for a few knobs that interact (a compressor's attack, release and ratio; a reverb's size, decay and
//! damping): generations of a handful of candidates, each generation heard in one pass, the search's shape learned
//! from which candidates did best. Knobs are searched in [0, 1] of their perceptual range.

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
        }
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

    /// A generation's candidates, each in [0, 1] (drawn again when one falls outside, then held at the edge).
    pub fn ask(&mut self) -> Vec<Vec<f64>> {
        (0..self.lambda)
            .map(|_| {
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
                point.iter().map(|x| x.clamp(0., 1.)).collect()
            })
            .collect()
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
        for i in 0..n {
            self.mean[i] = order[..mu].iter().zip(&weights).map(|(k, w)| w * points[*k][i]).sum();
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
        let ys: Vec<Vec<f64>> = order[..mu].iter().map(|k| (0..n).map(|i| (points[*k][i] - old[i]) / self.sigma).collect()).collect();
        for i in 0..n {
            for j in 0..n {
                let rank_one = self.pc[i] * self.pc[j] + (1. - hsig) * self.cc * (2. - self.cc) * self.c[i][j];
                let rank_mu: f64 = ys.iter().zip(&weights).map(|(y, w)| w * y[i] * y[j]).sum();
                self.c[i][j] = (1. - self.c1 - self.cmu) * self.c[i][j] + self.c1 * rank_one + self.cmu * rank_mu;
            }
        }
        self.sigma = (self.sigma * ((self.cs / self.damps) * (ps_norm / self.chi_n - 1.)).exp()).clamp(1e-4, 0.5);
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
