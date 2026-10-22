//! What a spec reads beside loudness: the RMS of the whole (dB, unweighted, as ACX's meters read it) and the noise floor (the
//! quietest half second's RMS that isn't digital silence), with how much digital silence there is.

/// The window the noise floor is read in (seconds).
const WINDOW: f64 = 0.5;
/// Quieter than this is digital silence, not a noise floor (dB).
const SILENCE: f64 = -100.;

#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub rms: f64,
    pub noise_floor: Option<f64>,
    /// Seconds of digital silence (ACX asks for room tone instead).
    pub silence: f64,
}

fn db(power: f64) -> f64 {
    if power > 0. {
        10. * power.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// The levels read as the audio streams by, a block at a time: a long programme never sits in memory whole.
pub struct Leveller {
    window: usize,
    total: f64,
    count: usize,
    held: f64,
    in_window: usize,
    floor: Option<f64>,
    silent: usize,
}

impl Leveller {
    pub fn new(rate: f64) -> Leveller {
        Leveller { window: ((WINDOW * rate) as usize).max(1), total: 0., count: 0, held: 0., in_window: 0, floor: None, silent: 0 }
    }

    /// A block of samples, its channels averaged.
    pub fn push(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            let sample = (*l as f64 + *r as f64) / 2.;
            let power = sample * sample;
            self.total += power;
            self.count += 1;
            self.held += power;
            self.in_window += 1;
            if self.in_window == self.window {
                let level = db(self.held / self.window as f64);
                if level < SILENCE {
                    self.silent += 1;
                } else {
                    self.floor = Some(self.floor.map_or(level, |floor: f64| floor.min(level)));
                }
                (self.held, self.in_window) = (0., 0);
            }
        }
    }

    pub fn finish(&self) -> Levels {
        Levels { rms: db(self.total / self.count.max(1) as f64), noise_floor: self.floor, silence: self.silent as f64 * WINDOW }
    }
}

pub fn levels(mono: &[f32], rate: f64) -> Levels {
    let mut leveller = Leveller::new(rate);
    leveller.push(mono, mono);
    leveller.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_noise_floor_is_the_quietest_half_second_and_digital_silence_is_counted_apart() {
        let rate = 1000.;
        // a second of a -20 dB tone-ish level, half a second of -70 dB room tone, half a second of digital silence
        let loud = 0.1f32 * std::f32::consts::SQRT_2;
        let mut mono: Vec<f32> = (0..1000).map(|k| if k % 2 == 0 { loud } else { -loud }).collect();
        mono.extend((0..500).map(|k| if k % 2 == 0 { 0.000_316 } else { -0.000_316 }));
        mono.extend(std::iter::repeat_n(0., 500));
        let found = levels(&mono, rate);
        assert!((found.noise_floor.unwrap() + 70.).abs() < 0.1, "{found:?}");
        assert_eq!(found.silence, 0.5);
        assert!((found.rms + 20.).abs() < 0.1, "the loud second over all two: {found:?}");
    }
}
