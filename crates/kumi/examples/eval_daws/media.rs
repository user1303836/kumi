//! The sounds the scenarios' fixtures load, made here (deterministic, 44.1 kHz 16-bit mono): drum hits, breaks, gravel
//! footstep layers, a vocal-like line and a bass. Nothing is downloaded.
use std::path::Path;

const RATE: u32 = 44_100;

/// A small deterministic noise source (a linear congruential generator), from -1 to 1.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

fn kick(len: f32) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..(len * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            phase += 2.0 * std::f32::consts::PI * (50.0 + 90.0 * (-t * 30.0).exp()) / RATE as f32;
            phase.sin() * (-t * 9.0).exp()
        })
        .collect()
}

fn snare(len: f32, noise: &mut Noise) -> Vec<f32> {
    (0..(len * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            (0.7 * noise.next() + 0.3 * (2.0 * std::f32::consts::PI * 190.0 * t).sin()) * (-t * 22.0).exp()
        })
        .collect()
}

fn hat(len: f32, noise: &mut Noise) -> Vec<f32> {
    let mut last = 0.0f32;
    (0..(len * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let n = noise.next();
            let high = n - last;
            last = n;
            0.5 * high * (-t * 70.0).exp()
        })
        .collect()
}

/// Adds `sound` into `out` at `at` seconds.
fn mix(out: &mut [f32], sound: &[f32], at: f32, gain: f32) {
    let start = (at * RATE as f32) as usize;
    for (i, s) in sound.iter().enumerate() {
        if let Some(o) = out.get_mut(start + i) {
            *o += s * gain;
        }
    }
}

/// A drum break: `bars` bars at `bpm`, sixteen steps a bar, kicks and snares on the steps given, hats on every other step;
/// `swing` pushes the off-beat sixteenths later (in steps).
fn break_loop(bpm: f32, bars: usize, kicks: &[usize], snares: &[usize], swing: f32, seed: u64) -> Vec<f32> {
    drums(bpm, bars, kicks, snares, swing, seed, true)
}

fn drums(bpm: f32, bars: usize, kicks: &[usize], snares: &[usize], swing: f32, seed: u64, hats: bool) -> Vec<f32> {
    let step = 60.0 / bpm / 4.0;
    let mut noise = Noise(seed);
    let mut out = vec![0.0; (bars as f32 * 16.0 * step * RATE as f32) as usize + RATE as usize / 4];
    let (k, s) = (kick(0.35), snare(0.25, &mut noise));
    for bar in 0..bars {
        for i in 0..16 {
            let at = (bar * 16 + i) as f32 * step + if i % 2 == 1 { swing * step } else { 0.0 };
            if kicks.contains(&i) {
                mix(&mut out, &k, at, 0.9);
            }
            if snares.contains(&i) {
                mix(&mut out, &s, at, 0.7);
            }
            if hats && i % 2 == 0 {
                mix(&mut out, &hat(0.06, &mut noise), at, 0.35);
            }
        }
    }
    out
}

/// A footstep layer: a noise burst, smoothed over `smooth` samples (more is darker), decaying at `decay`.
fn gravel(smooth: usize, decay: f32, seed: u64) -> Vec<f32> {
    let mut noise = Noise(seed);
    let raw: Vec<f32> = (0..(0.4 * RATE as f32) as usize).map(|_| noise.next()).collect();
    raw.iter()
        .enumerate()
        .map(|(i, _)| {
            let from = i.saturating_sub(smooth);
            let avg = raw[from..=i].iter().sum::<f32>() / (i - from + 1) as f32;
            let t = i as f32 / RATE as f32;
            avg * (-t * decay).exp() * (smooth as f32).sqrt().min(3.0)
        })
        .collect()
}

/// A vocal-like line: syllables of a voiced tone at changing levels, so a compressor has something to do.
fn vocal(seconds: f32) -> Vec<f32> {
    let levels = [0.2, 0.8, 0.5, 0.9, 0.3, 0.7, 0.4, 0.95, 0.25, 0.6];
    (0..(seconds * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let syllable = (t / 0.3) as usize;
            let within = t - syllable as f32 * 0.3;
            let envelope = (within * 30.0).min(1.0) * (-(within * 6.0)).exp();
            let f = 196.0 * (1.0 + 0.05 * (syllable % 3) as f32);
            let voice: f32 = (1..6).map(|h| (2.0 * std::f32::consts::PI * f * h as f32 * t).sin() / h as f32).sum();
            levels[syllable % levels.len()] * envelope * voice * 0.4
        })
        .collect()
}

/// A bass: a sawtooth on one note.
/// A plucked key: a few harmonics decaying.
fn key(seconds: f32, hz: f32) -> Vec<f32> {
    (0..(seconds * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let tone: f32 = (1..=4).map(|h| (2.0 * std::f32::consts::PI * hz * h as f32 * t).sin() / h as f32).sum();
            0.4 * tone * (-t * 3.0).exp()
        })
        .collect()
}

fn bass(seconds: f32, hz: f32) -> Vec<f32> {
    (0..(seconds * RATE as f32) as usize).map(|i| (((i as f32 * hz / RATE as f32) % 1.0) * 2.0 - 1.0) * 0.4).collect()
}

fn write(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    let scale = if peak > 0.98 { 0.98 / peak } else { 1.0 };
    let data: Vec<u8> = samples.iter().flat_map(|s| (((s * scale).clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes()).collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav)
}

/// Writes every sound the scenarios use into `dir`.
pub fn write_all(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut noise = Noise(7);
    write(&dir.join("kick.wav"), &kick(0.4))?;
    write(&dir.join("snare.wav"), &snare(0.3, &mut noise))?;
    // A 4-bar break at 170 BPM (the one bounced and chopped), its Amen-like and Think-like stand-ins, and a 4-bar break at
    // 120 BPM under a synth.
    write(&dir.join("break.wav"), &break_loop(170.0, 4, &[0, 10], &[4, 12], 0.0, 11))?;
    write(&dir.join("amen.wav"), &break_loop(170.0, 4, &[0, 2, 10, 11], &[4, 7, 9, 12, 15], 0.0, 13))?;
    write(&dir.join("think.wav"), &break_loop(170.0, 4, &[0, 6, 10], &[4, 12], 0.15, 17))?;
    write(&dir.join("break120.wav"), &break_loop(120.0, 4, &[0, 8, 10], &[4, 12], 0.0, 19))?;
    // (the same break 16 ms late, a layer of it: what only hearing it finds)
    let late: Vec<f32> =
        std::iter::repeat_n(0.0, (0.016 * RATE as f32) as usize).chain(break_loop(120.0, 4, &[0, 8, 10], &[4, 12], 0.0, 19)).collect();
    write(&dir.join("break120-late.wav"), &late)?;
    for (n, (smooth, decay)) in [(1usize, 18.0f32), (6, 12.0), (24, 8.0)].iter().enumerate() {
        write(&dir.join(format!("gravel{}.wav", n + 1)), &gravel(*smooth, *decay, 23 + n as u64))?;
    }
    write(&dir.join("vocal.wav"), &vocal(6.0))?;
    write(&dir.join("bass.wav"), &bass(8.0, 55.0))?;
    write(&dir.join("kickloop.wav"), &drums(120.0, 4, &[0, 4, 8, 12], &[], 0.0, 29, false))?;
    // (a keyboard's notes named as their file names say, middle C as C3: 261.63 Hz)
    for (name, hz) in [("C3", 261.63f32), ("E3", 329.63), ("G3", 392.0)] {
        write(&dir.join(format!("keys_{name}.wav")), &key(1.0, hz))?;
    }
    Ok(())
}
