//! The seeded generator every choice in a design is drawn from: the same seed draws the same numbers on every build, so
//! the same request and seed render the same bytes (SplitMix64, owned here so no dependency's update changes a sound).

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x6b75_6d69_6465_7369)
    }
    /// A generator of its own for one part of a design (`salt` names the part), so adding a draw in one part doesn't move
    /// every other part's numbers.
    pub fn fork(&self, salt: u64) -> Self {
        let mut forked = Self(self.0 ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        forked.next();
        forked
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }
    /// Uniform in [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        (self.unit() * n as f64) as usize % n
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u = self.unit().max(1e-12);
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
    /// Gamma(shape, 1) by Marsaglia–Tsang.
    fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            return self.gamma(shape + 1.0) * self.unit().max(1e-12).powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = (1.0 + c * x).powi(3);
            if v <= 0.0 {
                continue;
            }
            let u = self.unit().max(1e-12);
            if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
                return d * v;
            }
        }
    }
    /// Weights that sum to 1, drawn around even (Dirichlet with concentration `alpha` per part: higher is closer to even).
    pub fn dirichlet(&mut self, n: usize, alpha: f64) -> Vec<f64> {
        let draws: Vec<f64> = (0..n).map(|_| self.gamma(alpha).max(1e-6)).collect();
        let sum: f64 = draws.iter().sum();
        draws.into_iter().map(|d| d / sum).collect()
    }
    /// The indices 0..n in a seeded order.
    pub fn shuffled(&mut self, n: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            order.swap(i, self.below(i + 1));
        }
        order
    }
    /// An index drawn in proportion to `weights` (all zero: uniform).
    pub fn weighted(&mut self, weights: &[f64]) -> usize {
        let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
        if total <= 0.0 {
            return self.below(weights.len());
        }
        let mut at = self.unit() * total;
        for (i, w) in weights.iter().enumerate() {
            at -= w.max(0.0);
            if at < 0.0 {
                return i;
            }
        }
        weights.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn the_same_seed_draws_the_same_numbers_and_weights_sum_to_one() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert!((0..100).all(|_| a.next() == b.next()));
        assert_ne!(Rng::new(7).next(), Rng::new(8).next());
        let weights = Rng::new(3).dirichlet(5, 1.5);
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        let mut order = Rng::new(3).shuffled(9);
        order.sort();
        assert_eq!(order, (0..9).collect::<Vec<_>>());
    }
}
