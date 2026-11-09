//! Stack: two to eight sounds printed as one hit. They're aligned on their attacks (or their peaks), matched in
//! loudness, carved so they don't mask each other (one owns the low end; where two crowd a band the quieter gives way),
//! and summed; a reroll moves their offsets, gains and cuts.

use super::{
    analysis::{frame_at, hann, Stereo},
    fx,
    rng::Rng,
    short, Ending, Options, Prepared, RATE,
};
use kumi_media::audio::dsp::high_pass;

/// The bands a layer's energy is read in (Hz edges); masking is judged in the middle ones.
const EDGES: [f64; 10] = [20.0, 60.0, 120.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

/// A layer's energy in each band, over its first half second.
fn bands(source: &Prepared) -> Vec<f64> {
    let n = 4096usize;
    let window = hann(n);
    let mut energy = vec![0.0f64; EDGES.len() - 1];
    let reach = source.len().min((0.5 * RATE) as usize);
    let mut center = n / 2;
    while center < reach + n / 2 {
        let frame = frame_at(&source.mono, center as i64, &window);
        for k in 1..=n / 2 {
            let hz = k as f64 * RATE / n as f64;
            if let Some(b) = EDGES.windows(2).position(|e| hz >= e[0] && hz < e[1]) {
                energy[b] += frame.magnitude(k).powi(2);
            }
        }
        center += n / 2;
    }
    energy
}

pub(super) fn stack(sources: &[Prepared], options: &Options, mut rng: Rng) -> (Stereo, String, Ending) {
    let n = sources.len();
    let energies: Vec<Vec<f64>> = sources.iter().map(bands).collect();
    let primary = rng.below(n);
    let on_peaks = rng.chance(0.3);
    let latest_peak = sources.iter().map(|s| s.peak).max().unwrap_or(0);
    let low = |e: &Vec<f64>| e[0] + e[1];
    let owner = (0..n).max_by(|a, b| low(&energies[*a]).total_cmp(&low(&energies[*b]))).unwrap_or(primary);
    let mut layers = Vec::with_capacity(n);
    let mut words = Vec::with_capacity(n);
    let mut longest = 0usize;
    for (i, source) in sources.iter().enumerate() {
        let gain_db = if i == primary { 0.0 } else { rng.range(-6.0, 0.0) };
        let nudge = if i != primary && rng.chance(0.6) { (rng.range(0.0, 0.012) * RATE) as usize } else { 0 };
        let offset = nudge + if on_peaks { latest_peak - source.peak } else { 0 };
        let mut audio = source.audio.clone();
        let mut said = vec![format!("{:+.0} dB", gain_db)];
        if i == owner {
            said.push("owns the lows".into());
        } else {
            let hz = rng.range(90.0, 220.0);
            fx::filter(&mut audio, || high_pass(RATE, hz));
            said.push(format!("HP {:.0} Hz", hz));
        }
        // Where this layer crowds a louder one in a middle band, it gives way there.
        for b in 3..8 {
            let mine = energies[i][b];
            let louder = (0..n).filter(|j| *j != i).map(|j| energies[j][b]).fold(0.0, f64::max);
            if louder > mine && mine > louder * 0.5 {
                let cut = rng.range(2.0, 5.0);
                let hz = (EDGES[b] * EDGES[b + 1]).sqrt();
                fx::filter(&mut audio, || fx::peaking(hz, -cut, 1.0));
                said.push(format!(
                    "−{:.0} dB at {}",
                    cut,
                    if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{:.0} Hz", hz) }
                ));
            }
        }
        if offset > 0 && nudge > 0 {
            said.push(format!("+{:.0} ms", nudge as f64 / RATE * 1000.0));
        }
        longest = longest.max(offset + source.len());
        layers.push((audio, offset, 10f64.powf(gain_db / 20.0)));
        words.push(format!("{} ({})", short(&source.name), said.join(", ")));
    }
    let len = options.seconds.map_or(longest, |s| (s * RATE) as usize).min((super::MAX_SECONDS * RATE) as usize);
    let mut out = fx::silence(len);
    for (audio, offset, gain) in &layers {
        fx::mix_into(&mut out, audio, *offset, [*gain, *gain]);
    }
    let recipe = format!("Stack of {} on the {}: {}", n, if on_peaks { "peak" } else { "attack" }, words.join("; "));
    (out, recipe, Ending::OneShot { fade: (0.03 * RATE) as usize })
}
