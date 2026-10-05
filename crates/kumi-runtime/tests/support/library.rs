//! Synthesized fixtures.
#![allow(dead_code)]
use kumi_common::js::number::round;
use std::f64::consts::PI;
pub const RATE: f64 = 44100.;
fn noise(seed: u32) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        state as f64 / 2147483648. - 1.
    }
}
pub fn kick(hz: f64, seconds: f64) -> Vec<f32> {
    let mut phase = 0.;
    (0..round(RATE * seconds) as usize)
        .map(|index| {
            let t = index as f64 / RATE;
            let pitch = hz * (1. + (-t / 0.02).exp());
            phase += 2. * PI * pitch / RATE;
            (0.9 * phase.sin() * (-t / 0.12).exp()) as f32
        })
        .collect()
}
pub fn hat(seconds: f64, seed: u32) -> Vec<f32> {
    let mut random = noise(seed);
    let mut previous = 0.;
    (0..round(RATE * seconds) as usize)
        .map(|index| {
            let value = random();
            let sample = (0.6 * (value - previous) * (-(index as f64) / RATE / 0.03).exp()) as f32;
            previous = value;
            sample
        })
        .collect()
}
pub fn snare(seconds: f64) -> Vec<f32> {
    let mut random = noise(11);
    (0..round(RATE * seconds) as usize)
        .map(|index| {
            let t = index as f64 / RATE;
            ((0.4 * (2. * PI * 190. * t).sin() + 0.5 * random()) * (-t / 0.06).exp()) as f32
        })
        .collect()
}
pub fn beat(bpm: f64, bars: usize) -> Vec<f32> {
    let beat_frames = round(RATE * 60. / bpm) as usize;
    let mut out = vec![0_f32; beat_frames * 4 * bars];
    let one = kick(55., 0.3);
    let tick = hat(0.08, 3);
    for at in 0..bars * 4 {
        let start = at * beat_frames;
        let length = one.len().min(out.len() - start);
        out[start..start + length].copy_from_slice(&one[..length]);
        let off = start + round(beat_frames as f64 / 2.) as usize;
        for (index, value) in tick.iter().enumerate() {
            if off + index >= out.len() {
                break;
            }
            out[off + index] = ((out[off + index] as f64) + (*value as f64)) as f32;
        }
    }
    out
}
pub fn pad(frequencies: &[f64], seconds: f64) -> Vec<f32> {
    (0..round(RATE * seconds) as usize)
        .map(|index| {
            let t = index as f64 / RATE;
            let sum: f64 = frequencies.iter().map(|hz| (2. * PI * hz * t).sin()).sum();
            (0.25 * sum / frequencies.len() as f64 * (t / 0.4).min(1.)) as f32
        })
        .collect()
}
pub fn wav(channels: &[Vec<f32>], rate: u32) -> Vec<u8> {
    let frames = channels[0].len();
    let count = channels.len();
    let length = frames * count * 2;
    let mut out = vec![];
    out.extend(b"RIFF");
    out.extend(((36 + length) as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16_u32.to_le_bytes());
    out.extend(1_u16.to_le_bytes());
    out.extend((count as u16).to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * count as u32 * 2).to_le_bytes());
    out.extend((count as u16 * 2).to_le_bytes());
    out.extend(16_u16.to_le_bytes());
    out.extend(b"data");
    out.extend((length as u32).to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            out.extend((round(channel[frame] as f64 * 32767.).clamp(-32768., 32767.) as i16).to_le_bytes());
        }
    }
    out
}

pub fn put(path: &std::path::Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
pub fn sound_cases() -> Vec<(&'static str, Vec<Vec<f32>>)> {
    vec![
        ("Kick Deep.wav", vec![kick(48., 0.6)]),
        ("Kick Short.wav", vec![kick(60., 0.25)]),
        ("Hat Closed.wav", vec![hat(0.12, 3)]),
        ("Untitled 7.wav", vec![kick(52., 0.5)]),
        ("Beat 120 bpm.wav", vec![beat(120., 2)]),
        ("Pad Am.wav", vec![pad(&[220., 261.63, 329.63], 3.), pad(&[220., 261.63, 329.63], 3.)]),
        ("Dusty Snare.wav", vec![snare(0.25)]),
    ]
}
pub struct Studio {
    pub home: tempfile::TempDir,
    pub user: std::path::PathBuf,
    pub extra: std::path::PathBuf,
    pub dir: std::path::PathBuf,
}
impl Studio {
    pub fn new() -> Self {
        use base64::Engine;
        let home = tempfile::tempdir().unwrap();
        let user = home.path().join("Music/Ableton/User Library");
        let extra = home.path().join("Crate");
        let dir = home.path().join(".kumi/library");
        let locations = ["Samples/Kicks", "Samples/Kicks", "Samples/Hats", "Samples", "Samples/Loops", "Samples/Pads"];
        for (index, (name, channels)) in sound_cases().into_iter().enumerate() {
            let path = if index == 6 { extra.join(name) } else { user.join(locations[index]).join(name) };
            put(&path, wav(&channels, 44100));
        }
        put(&user.join("Samples/Notes.txt"), "not audio");
        put(&user.join("Ableton Folder Info/Previews/Kick Preview.wav"), wav(&[kick(50., 0.5)], 44100));
        let files: serde_json::Value = serde_json::from_str(include_str!("library-files-oracle.json")).unwrap();
        for (name, relative) in [
            ("Rolling Bass.adv", "Presets/Instruments/Wavetable/Rolling Bass.adv"),
            ("Vox Chain.adg", "Presets/Audio Effects/Vox Chain.adg"),
            ("Tight Kit.adg", "Presets/Drums/Tight Kit.adg"),
            ("instrument.amxd", "Max/Bubbles.amxd"),
            ("Night Drive.als", "Projects/Night Drive Project/Night Drive.als"),
            ("Night Drive.als", "Projects/Night Drive Project/Night Drive.backup-2026-01-01T00-00-00-000Z.als"),
            ("Sunrise.als", "Projects/Sunrise Project/Sunrise.als"),
        ] {
            let case = files["cases"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap();
            put(&user.join(relative), base64::engine::general_purpose::STANDARD.decode(case["body"].as_str().unwrap()).unwrap());
        }
        Self { home, user, extra, dir }
    }
    pub fn plan(&self, workers: usize) -> kumi_runtime::library::learn::LearnPlan {
        use kumi_runtime::library::sources::{library_sources, SourceOptions};
        kumi_runtime::library::learn::LearnPlan {
            dir: self.dir.to_string_lossy().into(),
            sources: library_sources(&SourceOptions {
                home: Some(self.home.path().to_string_lossy().into()),
                platform: Some("darwin".into()),
                applications: Some(self.home.path().join("Applications").to_string_lossy().into()),
                folders: Some(vec![self.extra.to_string_lossy().into()]),
                ..Default::default()
            }),
            set_folders: vec![],
            set_files: vec![],
            plugin_presets: vec![],
            workers: Some(workers),
        }
    }
}
