//! Homing in on one knob where more means more: a limiter's gain toward a loudness, a de-esser's threshold until the
//! harsh hits drop, a send toward a reverb level. Each probe is a listen, so it steps by what the last ones did (a
//! secant, then false position once the aim is bracketed) and stops as soon as the aim is met: two to four listens,
//! fewer with a good first guess.

/// One knob's search, in its perceptual units (dB, octaves, log-time), against one measured value.
#[derive(Debug, Clone, PartialEq)]
pub struct Homing {
    /// Where it steps toward, inside `met`.
    pub aim: f64,
    /// Any measure in this range meets the target (a few steps under a ceiling, say).
    pub met: (f64, f64),
    pub low: f64,
    pub high: f64,
    /// Where the knob is now: the first probe when what it measures there isn't known yet.
    pub first: f64,
    /// Every setting heard: (knob, measured), where it started first when that was known.
    pub probes: Vec<(f64, f64)>,
    known: bool,
    /// Probes that made something else audibly worse: never the answer, and the knob doesn't go that far again.
    pub hurt: Vec<f64>,
    pub most: usize,
    /// Probes closer than this sound the same (one just-noticeable step of the knob): one that close to another isn't
    /// heard again.
    pub resolution: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Homed {
    Met,
    /// Its probes are spent.
    Spent,
    /// The knob reached the end of its range, or the measure stopped answering it.
    Stuck,
}

impl Homing {
    /// From where the knob is (`first`) and, when it's known without a listen, what that measures.
    pub fn new(aim: f64, met: (f64, f64), (low, high): (f64, f64), first: f64, measured: Option<f64>, most: usize) -> Self {
        Self {
            aim,
            met: (met.0.min(met.1), met.0.max(met.1)),
            low: low.min(high),
            high: low.max(high),
            first,
            probes: measured.map(|measured| vec![(first, measured)]).unwrap_or_default(),
            known: measured.is_some(),
            hurt: vec![],
            most,
            resolution: (high - low).abs() * 1e-6,
        }
    }
    /// Probes closer than `step` (one just-noticeable step of the knob, in its perceptual units) count as the same.
    pub fn resolution(mut self, step: f64) -> Self {
        self.resolution = self.resolution.max(step.abs());
        self
    }
    /// A probe that made something else worse: the knob stays on this side of it from now on. One where the knob
    /// started (it already hurt before the knob moved: the change around it did) rules nothing out on either side.
    pub fn hurt(&mut self, knob: f64, measured: f64) {
        self.probes.push((knob, measured));
        self.hurt.push(knob);
        let margin = (self.high - self.low) * 1e-3;
        if (knob - self.first).abs() <= self.resolution {
            return;
        }
        if knob > self.first {
            self.high = self.high.min(knob - margin).max(self.low);
        } else {
            self.low = self.low.max(knob + margin).min(self.high);
        }
    }
    pub fn met(&self, measured: f64) -> bool {
        (self.met.0..=self.met.1).contains(&measured)
    }
    /// Probes heard (where it started counts only when it was heard for this).
    pub fn listens(&self) -> usize {
        self.probes.len() - usize::from(self.known)
    }
    pub fn heard(&mut self, knob: f64, measured: f64) {
        self.probes.push((knob, measured));
    }
    /// Why it stops now, if it does.
    pub fn done(&self) -> Option<Homed> {
        let last = self.probes.last()?;
        if self.met(last.1) && !self.hurt.contains(&last.0) {
            return Some(Homed::Met);
        }
        if self.listens() >= self.most {
            return Some(Homed::Spent);
        }
        None
    }
    /// The next knob setting to hear, or why there's none. `slope` is how much the measure moves per unit of the
    /// knob, when known before any probe (a gain moves loudness one for one).
    pub fn next(&self, slope: Option<f64>) -> Result<f64, Homed> {
        if self.probes.is_empty() {
            return Ok(self.first);
        }
        if let Some(done) = self.done() {
            return Err(done);
        }
        let (x, y) = *self.probes.last().unwrap();
        // The closest probes on either side of the aim, once there are some: false position between them.
        let below = self.probes.iter().filter(|p| p.1 < self.aim).min_by(|a, b| (self.aim - a.1).total_cmp(&(self.aim - b.1)));
        let above = self.probes.iter().filter(|p| p.1 > self.aim).min_by(|a, b| (a.1 - self.aim).total_cmp(&(b.1 - self.aim)));
        let wanted = match (below, above) {
            (Some(a), Some(b)) if (b.1 - a.1).abs() > 1e-9 => a.0 + (self.aim - a.1) * (b.0 - a.0) / (b.1 - a.1),
            _ => {
                // A secant through the last two probes when they moved the measure; else the slope given; else a
                // step of an eighth of the range, up.
                let learned = (self.probes.len() >= 2)
                    .then(|| {
                        let (px, py) = self.probes[self.probes.len() - 2];
                        let dx = x - px;
                        (dx.abs() > 1e-9 && (y - py).abs() > 1e-9).then(|| (y - py) / dx)
                    })
                    .flatten();
                match learned.or(slope).filter(|s| s.abs() > 1e-6) {
                    Some(s) => {
                        // Never more than half the range at once.
                        let step = ((self.aim - y) / s).clamp(-(self.high - self.low) / 2., (self.high - self.low) / 2.);
                        x + step
                    }
                    None => x + (self.high - self.low) / 8.,
                }
            }
        };
        // Hurt on both sides leaves no room (and no NaN gets this far).
        if !(self.low < self.high) || !wanted.is_finite() {
            return Err(Homed::Stuck);
        }
        let wanted = wanted.clamp(self.low, self.high);
        if self.probes.iter().any(|p| (p.0 - wanted).abs() < self.resolution.max(1e-9 * (self.high - self.low))) {
            return Err(Homed::Stuck);
        }
        Ok(wanted)
    }
    /// The setting that met it, closest to the aim (or, with none, the closest), and what it measured; never one that
    /// made something else worse.
    pub fn best(&self) -> Option<(f64, f64)> {
        let distance = |p: &(f64, f64)| (if self.met(p.1) { 0. } else { 1e6 }) + (p.1 - self.aim).abs();
        self.probes.iter().filter(|p| !self.hurt.contains(&p.0)).min_by(|a, b| distance(a).total_cmp(&distance(b))).copied()
    }
}
