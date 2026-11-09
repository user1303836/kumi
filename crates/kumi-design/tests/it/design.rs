//! What a producer hears of every design: each op and mode renders something finite, audible, click-free, under
//! -1 dBTP and of a sensible length; the same seed renders the same bytes and another seed a variation; loops land on
//! their grid and come round without a seam; textures move; and requests no design can make are refused.

use kumi_design::{analysis, render, Mode, Op, Options, Render, Request, Source, SourceRef, RATE};
use std::f64::consts::TAU;

/// A seeded noise source for the test sounds (not the engine's generator).
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

fn mono(name: &str, rate: f64, seconds: f64, f: impl Fn(f64, &mut Noise) -> f64) -> Source {
    let mut noise = Noise(name.len() as u64 * 7919);
    let samples: Vec<f32> = (0..(seconds * rate) as usize).map(|i| f(i as f64 / rate, &mut noise) as f32).collect();
    // A little silence first, as a sample file has.
    let mut padded = vec![0.0f32; (0.01 * rate) as usize];
    padded.extend(samples);
    Source::new(name, &[padded], rate)
}

fn kick() -> Source {
    mono("Kick 01.wav", 44_100.0, 0.6, |t, _| {
        let phase = TAU * (50.0 * t + 100.0 * (1.0 - (-t * 30.0).exp()) / 30.0);
        phase.sin() * (-t * 7.0).exp() * 0.9
    })
}
fn snare() -> Source {
    mono("Snare 02.wav", 48_000.0, 0.35, |t, n| {
        (0.6 * n.next() * (-t * 18.0).exp() + 0.4 * (TAU * 190.0 * t).sin() * (-t * 30.0).exp()) * 0.8
    })
}
fn hat() -> Source {
    mono("Hat 03.wav", 48_000.0, 0.12, |t, n| {
        let a = n.next();
        let b = n.next();
        (a - b) * 0.5 * (-t * 45.0).exp()
    })
}
fn pad() -> Source {
    let left: Vec<f32> = (0..(2.5 * RATE) as usize)
        .map(|i| {
            let t = i as f64 / RATE;
            let env = (t / 0.3).min(1.0) * (-(t - 1.5).max(0.0) * 2.0).exp();
            let saw = |hz: f64| 2.0 * ((t * hz).fract()) - 1.0;
            ((saw(220.0) + saw(277.2) + saw(329.6)) / 3.0 * env * 0.5) as f32
        })
        .collect();
    let right: Vec<f32> = left.iter().enumerate().map(|(i, v)| v * (0.8 + 0.2 * (i as f32 / 9000.0).sin())).collect();
    Source::new("Pad Chord.wav", &[left, right], RATE)
}
fn bell() -> Source {
    mono("Bell.wav", 48_000.0, 2.0, |t, _| {
        [(523.0, 1.0, 1.5), (1410.0, 0.5, 0.9), (2380.0, 0.3, 0.5), (3510.0, 0.2, 0.3)]
            .iter()
            .map(|(hz, a, d)| a * (TAU * hz * t).sin() * (-t / d).exp())
            .sum::<f64>()
            * 0.4
    })
}
/// A two-bar groove of its own hits: kick, snare and hat at 100 BPM, so a loop has onsets to slice.
fn groove() -> Source {
    let step = 60.0 / 100.0 / 4.0;
    mono("Groove.wav", 48_000.0, 32.0 * step, move |t, n| {
        let s = (t / step).floor() as usize;
        let since = t - s as f64 * step;
        let mut v = 0.0;
        if s % 4 == 0 {
            v += (TAU * (55.0 * since + 90.0 * (1.0 - (-since * 25.0).exp()) / 25.0)).sin() * (-since * 9.0).exp();
        }
        if s % 8 == 4 {
            v += n.next() * 0.6 * (-since * 20.0).exp();
        }
        if s % 2 == 1 {
            v += n.next() * 0.25 * (-since * 60.0).exp();
        }
        v * 0.7
    })
}

fn named(sources: &[Source]) -> Vec<SourceRef> {
    sources.iter().map(|s| SourceRef { file: Some(format!("/tmp/{}", s.name)), name: Some(s.name.clone()), ..Default::default() }).collect()
}

fn request(op: Op, mode: Option<Mode>, sources: &[Source], options: Options) -> Request {
    Request { op, mode, sources: named(sources), seed: None, options }
}

fn run(request: &Request, seed: u64, sources: &[Source]) -> Render {
    render(request, seed, sources, &mut |_| {}).unwrap_or_else(|e| panic!("{:?} {:?}: {e}", request.op, request.mode))
}

/// The render's true peak, by the engine's 8x meter.
fn true_peak(render: &Render) -> f64 {
    render.audio.iter().map(|c| kumi_design::true_peak(c, false)).fold(0.0, f64::max)
}

/// Everything a producer would hear wrong in any render.
fn sound(render: &Render, what: &str) {
    let len = render.audio[0].len();
    assert!(render.audio.iter().all(|c| c.len() == len && c.iter().all(|v| v.is_finite())), "{what}: finite");
    let tp = 20.0 * true_peak(render).log10();
    assert!(tp <= -0.99 && tp > -1.2, "{what}: true peak {tp:.2} dBTP");
    let rms = (render.audio[0].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / len as f64).sqrt();
    assert!(20.0 * rms.log10() > -45.0, "{what}: audible ({:.1} dBFS RMS)", 20.0 * rms.log10());
    assert!(!render.recipe.is_empty() && !render.recipe.contains('\n'), "{what}: a one-line recipe");
    let wav = render.wav();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2, "{what}: stereo");
    assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 48_000, "{what}: 48 kHz");
    assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 24, "{what}: 24-bit");
    assert_eq!(wav.len(), 44 + len * 6);
    let peaks = render.peaks(256);
    assert_eq!(peaks.len(), 256);
    assert!(peaks.iter().any(|p| *p == 1.0) && peaks.iter().all(|p| (0.0..=1.0).contains(p)));
}

/// A one-shot starts and ends at silence: no click either side.
fn no_clicks(render: &Render, what: &str) {
    for channel in &render.audio {
        assert!(channel[0].abs() < 0.02, "{what}: starts at silence ({})", channel[0]);
        let last = channel[channel.len() - 1].abs();
        assert!(last < 0.002, "{what}: ends at silence ({last})");
    }
}

/// A loop comes round without a seam: its last sample steps to its first no further than its own samples step.
fn seamless(render: &Render, what: &str) {
    for channel in &render.audio {
        let len = channel.len();
        let steps: Vec<f32> = channel.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        let mut sorted = steps.clone();
        sorted.sort_by(f32::total_cmp);
        let typical = sorted[(sorted.len() as f64 * 0.999) as usize];
        let seam = (channel[0] - channel[len - 1]).abs();
        assert!(seam <= typical.max(1e-4), "{what}: the seam steps {seam}, its samples up to {typical}");
    }
}

#[test]
fn every_op_and_mode_renders_a_finished_sound() {
    let one_shots = [kick(), snare(), hat(), bell()];
    for mode in [Mode::Blend, Mode::Attack, Mode::Resonate, Mode::Modulate] {
        let r = run(&request(Op::Cross, Some(mode), &one_shots, Options::default()), 11, &one_shots);
        let what = format!("cross {}", mode.name());
        sound(&r, &what);
        no_clicks(&r, &what);
        assert!((0.05..=10.0).contains(&r.duration()), "{what}: {} s", r.duration());
    }
    let stacked = [kick(), snare(), hat()];
    let r = run(&request(Op::Stack, None, &stacked, Options::default()), 5, &stacked);
    sound(&r, "stack");
    no_clicks(&r, "stack");
    // A stack is as long as its longest layer, give or take its nudges and trimmed tail.
    assert!((0.3..=0.75).contains(&r.duration()), "stack: {} s", r.duration());
    let r = run(&request(Op::Cross, Some(Mode::Blend), &one_shots, Options { seconds: Some(1.5), ..Default::default() }), 3, &one_shots);
    assert_eq!(r.audio[0].len(), (1.5 * RATE) as usize, "seconds sets a render's length");
}

#[test]
fn smears_stretch_and_textures_move_and_loop() {
    let cluster = [pad(), bell(), snare()];
    let smear = run(&request(Op::Fuse, Some(Mode::Smear), &cluster, Options { stretch: Some(6.0), ..Default::default() }), 2, &cluster);
    sound(&smear, "smear");
    no_clicks(&smear, "smear");
    let mean = (2.5 + 2.0 + 0.35) / 3.0;
    assert!(smear.duration() >= mean * 6.0 * 0.6, "a 6× smear lasts about six times its sources: {} s", smear.duration());

    for seed in [1, 2, 3] {
        let texture =
            run(&request(Op::Fuse, Some(Mode::Texture), &cluster, Options { seconds: Some(6.0), ..Default::default() }), seed, &cluster);
        let what = format!("texture {seed}");
        sound(&texture, &what);
        assert_eq!(texture.audio[0].len(), (6.0 * RATE) as usize, "{what}: its length");
        seamless(&texture, &what);
        // It moves between its sources: its brightness wanders over its length.
        let track = analysis::centroid_track(&analysis::mono(&texture.audio), (0.25 * RATE) as usize);
        let mean = track.iter().sum::<f64>() / track.len() as f64;
        let spread = (track.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / track.len() as f64).sqrt();
        assert!(spread / mean > 0.05, "{what}: its centroid moves ({spread:.0} Hz around {mean:.0} Hz)");
    }
}

#[test]
fn loops_land_on_their_grid_and_come_round() {
    let sources = [groove(), kick(), snare(), hat()];
    for (seed, bpm, bars) in [(1u64, 124.0, 2u32), (7, 90.0, 1), (9, 140.0, 4)] {
        let options = Options { bpm: Some(bpm), bars: Some(bars), ..Default::default() };
        let r = run(&request(Op::Fuse, Some(Mode::Loop), &sources, options), seed, &sources);
        let what = format!("loop {bpm} BPM {bars} bars");
        sound(&r, &what);
        let step = 60.0 / bpm / 4.0 * RATE;
        assert_eq!(r.audio[0].len(), (step * 16.0 * bars as f64).round() as usize, "{what}: exactly its bars");
        seamless(&r, &what);
        // Every hit falls on a sixteenth, an odd one perhaps swung late by up to a quarter step.
        let onsets = analysis::onsets(&analysis::mono(&r.audio));
        assert!(onsets.len() >= 4 * bars as usize, "{what}: {} hits", onsets.len());
        let tolerance = 0.012 * RATE;
        for onset in &onsets {
            let within = (*onset as f64).rem_euclid(2.0 * step);
            let on_even = within < tolerance || within > 2.0 * step - tolerance;
            let on_odd = within > step - tolerance && within < 1.25 * step + tolerance;
            assert!(on_even || on_odd, "{what}: a hit at {:.3} s is off the grid", *onset as f64 / RATE);
        }
    }
}

#[test]
fn the_same_seed_renders_the_same_bytes_and_another_a_variation() {
    let sources = [kick(), snare(), bell()];
    let cases: Vec<(Op, Option<Mode>)> = vec![
        (Op::Cross, Some(Mode::Blend)),
        (Op::Cross, Some(Mode::Attack)),
        (Op::Cross, Some(Mode::Resonate)),
        (Op::Cross, Some(Mode::Modulate)),
        (Op::Fuse, Some(Mode::Texture)),
        (Op::Fuse, Some(Mode::Loop)),
        (Op::Fuse, Some(Mode::Smear)),
        (Op::Stack, None),
    ];
    for (op, mode) in cases {
        let options = Options {
            seconds: (mode == Some(Mode::Texture)).then_some(3.0),
            stretch: (mode == Some(Mode::Smear)).then_some(4.0),
            ..Default::default()
        };
        let req = request(op, mode, &sources, options);
        let first = run(&req, 42, &sources).wav();
        let again = run(&req, 42, &sources).wav();
        let other = run(&req, 43, &sources).wav();
        let what = format!("{op:?} {mode:?}");
        assert!(first == again, "{what}: the same seed, the same bytes");
        assert!(first != other, "{what}: another seed, another sound");
    }
}

#[test]
fn requests_no_design_can_make_are_refused() {
    let two = [kick(), snare()];
    let refused = |req: Request| req.check().unwrap_err().0;
    assert!(refused(request(Op::Cross, Some(Mode::Blend), &two[..1], Options::default())).contains("2 to 16"));
    let many: Vec<Source> = (0..9).map(|_| kick()).collect();
    assert!(refused(request(Op::Stack, None, &many, Options::default())).contains("2 to 8"));
    let seventeen: Vec<Source> = (0..17).map(|_| kick()).collect();
    assert!(refused(request(Op::Fuse, Some(Mode::Smear), &seventeen, Options::default())).contains("2 to 16"));
    assert!(refused(request(Op::Cross, None, &two, Options::default())).contains("needs a mode"));
    assert!(refused(request(Op::Fuse, Some(Mode::Blend), &two, Options::default())).contains("mode of cross"));
    assert!(refused(request(Op::Stack, Some(Mode::Loop), &two, Options::default())).contains("mode of fuse"));
    let split = |s: Vec<usize>| refused(request(Op::Cross, Some(Mode::Resonate), &two, Options { split: Some(s), ..Default::default() }));
    assert!(split(vec![0, 1]).contains("at least one"));
    assert!(split(vec![2]).contains("by place"));
    assert!(refused(request(Op::Fuse, Some(Mode::Loop), &two, Options { bars: Some(5), ..Default::default() })).contains("bars"));
    assert!(refused(request(Op::Fuse, Some(Mode::Smear), &two, Options { stretch: Some(2.0), ..Default::default() })).contains("stretch"));
    let mut seeded = request(Op::Stack, None, &two, Options::default());
    seeded.seed = Some(1 << 60);
    assert!(refused(seeded).contains("seed"));
    let mut both = request(Op::Stack, None, &two, Options::default());
    both.sources[0].sound_id = Some("s-1".into());
    assert!(refused(both).contains("one of"));
    // Unknown fields don't slip through as defaults.
    let typo = serde_json::from_str::<Request>(r#"{"op":"stack","sources":[{"file":"a"},{"file":"b"}],"options":{"secs":2}}"#);
    assert!(typo.is_err());
    // A silent source says which.
    let silent = [kick(), Source::new("Room Tone.wav", &[vec![0.0; 48_000]], RATE)];
    let error = render(&request(Op::Stack, None, &silent, Options::default()), 1, &silent, &mut |_| {}).err().unwrap();
    assert!(error.0.contains("Room Tone.wav is silent"), "{}", error.0);
}
