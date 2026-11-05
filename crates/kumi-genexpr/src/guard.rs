//! The guarded output stage a device built for Hermes ends in (CC-22 22c as amended by W1-C), and its proof.
//!
//! The stage is a gen~ of its own after the model's: the model's outputs (in1, in2) reach `plugout~` only through it, and
//! the dry signal (in3, in4) comes straight from `plugin~`. The wet signal has anything that isn't a finite number under
//! 1e9 made 0, denormals flushed, DC blocked, and is held within ±0.966 (−0.3 dBFS). Then the Mix blends it with the dry
//! signal, which passes untouched, and Output (−36 to 0 dB) never boosts. So the device's output peaks at most at the
//! higher of the dry signal's peak and 0.966, and at 0% Mix it is the dry signal bit for bit.
//!
//! The mix is `dry·(1−m) + wet·m`, not gen's `mix(dry, wet, m)`: each product is exact at 0%, 50% and 100%, so the dry
//! signal is bit-exact at 0% Mix and a NaN wet signal at 50% gives exactly half the dry.

use crate::Gen;

/// The most the wet signal reaches: −0.3 dBFS.
pub const CEILING: f64 = 0.966;

/// The stage's code: one gen~ codebox, wet in on in1/in2, dry in on in3/in4, the device's output on out1/out2.
pub const STAGE: &str = concat!(
    "// Kumi's output stage (fixed): the effect's own output made safe (anything not a finite number under 1e9 is 0,\n",
    "// denormals and DC out, held within -0.3 dBFS), mixed with the dry signal, which passes untouched, then Output,\n",
    "// which never boosts.\n",
    "Param kumi_mix(100, min=0, max=100);\n",
    "Param kumi_output(0, min=-36, max=0);\n",
    "m = clamp(kumi_mix, 0, 100) / 100;\n",
    "gain = dbtoa(clamp(kumi_output, -36, 0));\n",
    "wet1 = clamp(dcblock(fixdenorm(abs(fixnan(in1)) <= 1e9 ? fixnan(in1) : 0)), -0.966, 0.966);\n",
    "wet2 = clamp(dcblock(fixdenorm(abs(fixnan(in2)) <= 1e9 ? fixnan(in2) : 0)), -0.966, 0.966);\n",
    "out1 = (in3 * (1 - m) + wet1 * m) * gain;\n",
    "out2 = (in4 * (1 - m) + wet2 * m) * gain;"
);

/// Samples in one second at 48 kHz: how long each hostile signal runs, and how soon DC must have settled.
const SECOND: usize = 48_000;

/// The hostile wet signals the stage is proven against, by name, as a sample at each index.
fn hostile() -> Vec<(&'static str, Box<dyn Fn(usize) -> f64>)> {
    vec![
        ("full-scale white noise", Box::new(noise)),
        ("+1 DC", Box::new(|_| 1.0)),
        ("NaN", Box::new(|_| f64::NAN)),
        ("+Inf", Box::new(|_| f64::INFINITY)),
        ("-Inf", Box::new(|_| f64::NEG_INFINITY)),
        ("a 2.0 square", Box::new(|at| if (at / 50) % 2 == 0 { 2.0 } else { -2.0 })),
    ]
}

/// Full-scale white noise, the same every run: a xorshift's bits as −1 to 1.
fn noise(at: usize) -> f64 {
    let mut x = (at as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// The producer's dry signal: a sine peaking at 1.5, hotter than full scale, as a track before Main may be.
fn dry(at: usize) -> f64 {
    1.5 * (0.37 + at as f64 * std::f64::consts::TAU * 220.0 / SECOND as f64).sin()
}

/// One second of `wet` through `stage` at `mix` percent and Output 0 dB, with the dry signal beside it: each sample's
/// dry input and the two outputs.
fn run(stage: &str, wet: &dyn Fn(usize) -> f64, mix: f64) -> Result<Vec<(f64, [f64; 2])>, String> {
    let mut gen = Gen::new(stage, false)?;
    gen.samplerate = SECOND as f64;
    gen.set("kumi_mix", mix)?;
    gen.set("kumi_output", 0.0)?;
    (0..SECOND)
        .map(|at| {
            let (w, d) = (wet(at), dry(at));
            let out = gen.tick(&[w, -w, d, -d], 2)?;
            Ok((d, [out[0], out[1]]))
        })
        .collect()
}

/// Runs `stage` with every hostile wet signal and checks what CC-22 says the device's output may be: within ±0.966 at
/// 100% Mix and never anything but a finite number; exactly 0 of its own for NaN or ±Inf; DC under 1e-3 within one second
/// at 48 kHz; the dry signal bit for bit at 0% Mix; at 50% within 0.5·|dry| + 0.5·0.966, and exactly 0.5·dry for NaN or
/// ±Inf. Err says the first thing that doesn't hold.
pub fn prove(stage: &str) -> Result<(), String> {
    for (name, wet) in hostile() {
        let non_finite = name.contains("NaN") || name.contains("Inf");
        let full = run(stage, &*wet, 100.0).map_err(|why| format!("{name} at 100% Mix: {why}"))?;
        for (at, (_, out)) in full.iter().enumerate() {
            for y in out {
                if !y.is_finite() || y.abs() > CEILING {
                    return Err(format!("{name} at 100% Mix: sample {at} is {y}, past ±{CEILING}"));
                }
                if non_finite && *y != 0.0 {
                    return Err(format!("{name} at 100% Mix: sample {at} is {y}, not 0"));
                }
            }
        }
        if name == "+1 DC" {
            let settled = full[SECOND - 1].1;
            if settled.iter().any(|y| y.abs() >= 1e-3) {
                return Err(format!("{name}: still {settled:?} after one second at 48 kHz, not under 1e-3"));
            }
        }
        let none = run(stage, &*wet, 0.0).map_err(|why| format!("{name} at 0% Mix: {why}"))?;
        for (at, (d, out)) in none.iter().enumerate() {
            if out[0].to_bits() != d.to_bits() || out[1].to_bits() != (-d).to_bits() {
                return Err(format!("{name} at 0% Mix: sample {at} is {out:?}, not the dry {d} bit for bit"));
            }
        }
        let half = run(stage, &*wet, 50.0).map_err(|why| format!("{name} at 50% Mix: {why}"))?;
        for (at, (d, out)) in half.iter().enumerate() {
            let bound = 0.5 * d.abs() + 0.5 * CEILING;
            if out.iter().any(|y| !y.is_finite() || y.abs() > bound) {
                return Err(format!("{name} at 50% Mix: sample {at} is {out:?}, past {bound}"));
            }
            if non_finite && (out[0] != 0.5 * d || out[1] != 0.5 * -d) {
                return Err(format!("{name} at 50% Mix: sample {at} is {out:?}, not half the dry {d}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a wet signal through the stage, at a Mix: the dry inputs and the outputs.
    fn through(wet: impl Fn(usize) -> f64, mix: f64) -> Vec<(f64, [f64; 2])> {
        run(STAGE, &wet, mix).unwrap()
    }

    #[test]
    fn every_hostile_wet_signal_at_full_mix_stays_within_the_ceiling_and_finite() {
        for (name, wet) in hostile() {
            let out = through(wet, 100.0);
            assert!(out.iter().flat_map(|(_, out)| out).all(|y| y.is_finite() && y.abs() <= CEILING), "{name}");
        }
    }

    #[test]
    fn nan_or_inf_wet_adds_exactly_nothing() {
        for wet in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(through(|_| wet, 100.0).iter().all(|(_, out)| out == &[0.0, 0.0]), "{wet} at 100%");
            for (d, out) in through(|_| wet, 50.0) {
                assert_eq!(out, [0.5 * d, 0.5 * -d], "{wet} at 50% is exactly half the dry");
            }
        }
    }

    #[test]
    fn dc_settles_under_a_thousandth_within_a_second_at_48_khz() {
        let out = through(|_| 1.0, 100.0);
        assert!(out[0].1[0] > 0.9, "a step passes at first");
        assert!(out[SECOND - 1].1.iter().all(|y| y.abs() < 1e-3), "{:?}", out[SECOND - 1]);
    }

    #[test]
    fn a_hot_dry_signal_at_no_mix_comes_out_bit_for_bit() {
        let out = through(|at| if (at / 50) % 2 == 0 { 2.0 } else { -2.0 }, 0.0);
        assert!(out.iter().any(|(d, _)| d.abs() > 1.49), "the dry signal peaks at 1.5");
        for (d, out) in out {
            assert_eq!((out[0].to_bits(), out[1].to_bits()), (d.to_bits(), (-d).to_bits()));
        }
    }

    #[test]
    fn at_half_mix_the_output_is_held_to_half_the_dry_and_half_the_ceiling() {
        for (name, wet) in hostile() {
            for (d, out) in through(wet, 50.0) {
                assert!(out.iter().all(|y| y.abs() <= 0.5 * d.abs() + 0.5 * CEILING), "{name}: {out:?} with dry {d}");
            }
        }
    }

    #[test]
    fn output_never_boosts() {
        let mut gen = Gen::new(STAGE, false).unwrap();
        gen.set("kumi_output", 12.0).unwrap();
        let out = gen.tick(&[0.5, 0.5, 0.0, 0.0], 2).unwrap();
        assert_eq!(out, [0.5, 0.5], "past 0 dB is held at 0 dB");
    }

    #[test]
    fn the_proof_holds_for_the_stage_and_catches_one_without_its_guard() {
        assert_eq!(prove(STAGE), Ok(()));
        let unclamped = STAGE.replace("-0.966, 0.966", "-1e9, 1e9");
        assert!(prove(&unclamped).unwrap_err().contains("past ±0.966"), "a square at 2.0 gets through");
        let unguarded = STAGE.replace("abs(fixnan(in1)) <= 1e9 ? fixnan(in1) : 0", "in1");
        assert!(prove(&unguarded).unwrap_err().starts_with("NaN at 100% Mix"), "a NaN reaches the mix");
        let mixed = STAGE.replace("in3 * (1 - m) + wet1 * m", "wet1");
        assert!(prove(&mixed).unwrap_err().contains("0% Mix"), "the dry signal must pass");
    }
}
