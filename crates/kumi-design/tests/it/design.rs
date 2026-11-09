//! What a producer hears of every design: each op and mode renders something finite, audible, click-free, under
//! -1 dBTP and of a sensible length; the same seed renders the same bytes and another seed a variation; loops land on
//! their grid and come round without a seam; textures move; and requests no design can make are refused.

use kumi_design::{analysis, render, steer::Steer, Mode, Op, Options, Render, Request, Source, SourceRef, RATE};
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
    assert!(refused(request(Op::Cross, Some(Mode::Blend), &two[..1], Options::default())).contains("2 or more"));
    // No most: any number of sounds is a request a design can make.
    let many: Vec<Source> = (0..40).map(|_| kick()).collect();
    assert!(request(Op::Stack, None, &many, Options::default()).check().is_ok());
    assert!(request(Op::Fuse, Some(Mode::Smear), &many, Options::default()).check().is_ok());
    assert!(refused(request(Op::Cross, None, &two, Options::default())).contains("needs a mode"));
    assert!(refused(request(Op::Fuse, Some(Mode::Blend), &two, Options::default())).contains("mode of cross"));
    assert!(refused(request(Op::Stack, Some(Mode::Loop), &two, Options::default())).contains("mode of fuse"));
    let split = |s: Vec<usize>| refused(request(Op::Cross, Some(Mode::Resonate), &two, Options { split: Some(s), ..Default::default() }));
    assert!(split(vec![0, 1]).contains("at least one"));
    assert!(split(vec![2]).contains("by place"));
    assert!(refused(request(Op::Fuse, Some(Mode::Loop), &two, Options { bars: Some(9), ..Default::default() })).contains("bars"));
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

/// The render's mono mix.
fn mono_of(render: &Render) -> Vec<f32> {
    analysis::mono(&render.audio)
}

/// Its brightness and noisiness over its first 300 ms, where the steered tone shows.
fn colour(render: &Render) -> analysis::Colour {
    let mono = mono_of(render);
    analysis::colour(&mono[..mono.len().min((0.3 * RATE) as usize)], 0)
}

/// How much of its energy is in its first 15 ms against the 15 to 150 ms after: how hard it hits.
fn hit_share(render: &Render) -> f64 {
    let mono = mono_of(render);
    let energy = |from: f64, to: f64| -> f64 {
        let (a, b) = ((from * RATE) as usize, ((to * RATE) as usize).min(mono.len()));
        mono[a.min(b)..b].iter().map(|v| (*v as f64).powi(2)).sum()
    };
    energy(0.0, 0.015) / energy(0.015, 0.15).max(1e-12)
}

fn steered(steer: Steer) -> Options {
    Options { steer: Some(steer), ..Default::default() }
}

#[test]
fn a_steer_that_moves_nothing_renders_the_same_bytes() {
    let sources = [kick(), snare(), bell()];
    for (op, mode) in [(Op::Cross, Some(Mode::Blend)), (Op::Stack, None), (Op::Fuse, Some(Mode::Loop))] {
        let plain = run(&request(op, mode, &sources, Options::default()), 9, &sources);
        let neutral = Steer { bright: Some(0.0), pitch: Some([0.0, 0.0]), reverse: Some(0.0), resonance: Some(0.7), ..Default::default() };
        for steer in [Steer::default(), neutral] {
            let same = run(&request(op, mode, &sources, steered(steer)), 9, &sources);
            assert!(same.wav() == plain.wav() && same.recipe == plain.recipe, "{op:?} {mode:?}: a neutral steer changes nothing");
        }
    }
}

#[test]
fn each_steer_control_pushes_the_sound_its_way() {
    let sources = [kick(), snare(), hat()];
    let make = |steer: Steer| {
        let r = run(&request(Op::Stack, None, &sources, steered(steer)), 4, &sources);
        sound(&r, &r.recipe.clone());
        no_clicks(&r, &r.recipe.clone());
        r
    };
    let base = make(Steer::default());
    let (dark, bright) =
        (make(Steer { bright: Some(-1.0), ..Default::default() }), make(Steer { bright: Some(1.0), ..Default::default() }));
    assert!(
        colour(&bright).centroid > colour(&base).centroid && colour(&base).centroid > colour(&dark).centroid,
        "bright moves the centroid up"
    );
    assert!(dark.recipe.contains("darker") && bright.recipe.contains("brighter"), "{} / {}", dark.recipe, bright.recipe);
    let closed = make(Steer { filter: Some(-0.8), resonance: Some(0.5), ..Default::default() });
    assert!(colour(&closed).centroid < colour(&base).centroid * 0.6, "a low-pass takes the top off");
    assert!(closed.recipe.contains("low-passed") && closed.recipe.contains("resonant"), "{}", closed.recipe);
    let (soft, punchy) = (make(Steer { punch: Some(-1.0), ..Default::default() }), make(Steer { punch: Some(1.0), ..Default::default() }));
    assert!(hit_share(&punchy) > hit_share(&soft), "punch moves energy onto the hit");
    let (tonal, noisy) = (make(Steer { noise: Some(-1.0), ..Default::default() }), make(Steer { noise: Some(1.0), ..Default::default() }));
    assert!(colour(&noisy).flatness > colour(&tonal).flatness, "noise flattens the spectrum");
    let (short, long) = (make(Steer { length: Some(-1.0), ..Default::default() }), make(Steer { length: Some(1.0), ..Default::default() }));
    assert!(
        short.duration() < base.duration() && base.duration() < long.duration(),
        "length: {} < {} < {}",
        short.duration(),
        base.duration(),
        long.duration()
    );
    let roomy = make(Steer { space: Some(1.0), ..Default::default() });
    assert!(roomy.duration() > base.duration() + 0.5 && roomy.recipe.contains("in a hall"), "space rings on: {} s", roomy.duration());
    let mono = make(Steer { width: Some(-1.0), ..Default::default() });
    assert!(mono.audio[0].iter().zip(&mono.audio[1]).all(|(l, r)| (l - r).abs() < 1e-6), "width -1 is mono");
    let wide = make(Steer { width: Some(1.0), ..Default::default() });
    assert!(wide.audio[0].iter().zip(&wide.audio[1]).any(|(l, r)| (l - r).abs() > 1e-3), "width 1 widens a mono stack");
    for (steer, word) in [
        (Steer { drive: Some(0.8), ..Default::default() }, "driven"),
        (Steer { crush: Some(0.9), ..Default::default() }, "crushed"),
        (Steer { level: Some(0.5), ..Default::default() }, "levels within"),
        (Steer { pan: Some(0.8), ..Default::default() }, "panned ±80%"),
        (Steer { start: Some([0.1, 0.3]), ..Default::default() }, "started 10…30% in"),
    ] {
        let r = make(steer);
        assert!(r.recipe.contains(word), "{word}: {}", r.recipe);
    }
    // Every sound reversed swells into its hit: the loudest moment comes late.
    let backwards = make(Steer { reverse: Some(1.0), ..Default::default() });
    let loudest = |r: &Render| mono_of(r).iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map_or(0, |(i, _)| i);
    assert!(
        loudest(&backwards) > loudest(&base) + (0.1 * RATE) as usize && backwards.recipe.contains("3 of 3 reversed"),
        "{}",
        backwards.recipe
    );
    // Each delayed by 100 ms: the stack starts 100 ms late.
    let late = make(Steer { delay: Some([100.0, 100.0]), ..Default::default() });
    let first = mono_of(&late).iter().position(|v| v.abs() > 1e-3).unwrap_or(0);
    assert!(first as f64 >= 0.095 * RATE, "delay: the first sound at {:.3} s", first as f64 / RATE);
}

#[test]
fn steered_pitch_moves_each_sound_and_a_reroll_draws_again() {
    // Grown no way at all, the render is its first sound alone: a plain harmonic tone, whose pitch is clear to hear.
    let tone = mono("Tone.wav", 48_000.0, 1.2, |t, _| (2.0 * (t * 330.0).fract() - 1.0) * (-t * 2.0).exp() * 0.5);
    let pair = [tone, kick()];
    let pitched = |steer: Steer| {
        let r = run(&request(Op::Grow, None, &pair, Options { at: Some(0.0), steer: Some(steer), ..Default::default() }), 1, &pair);
        (analysis::pitch(&mono_of(&r), 0).expect("a pitched bell"), r.recipe)
    };
    let (plain, _) = pitched(Steer::default());
    let (octave, recipe) = pitched(Steer { pitch: Some([12.0, 12.0]), ..Default::default() });
    assert!((octave / plain - 2.0).abs() < 0.04, "an octave up: {plain:.0} Hz to {octave:.0} Hz");
    assert!(recipe.contains("pitch +12 st"), "{recipe}");
    let bells = [bell(), bell()];
    let ranged = request(Op::Stack, None, &bells, steered(Steer { pitch: Some([-12.0, 12.0]), ..Default::default() }));
    assert!(run(&ranged, 5, &bells).wav() == run(&ranged, 5, &bells).wav(), "the same seed draws the same pitches");
    assert!(run(&ranged, 5, &bells).wav() != run(&ranged, 6, &bells).wav(), "another seed draws others");
}

#[test]
fn a_steered_loop_still_comes_round() {
    let sources = [groove(), kick(), snare(), hat()];
    let steer = Steer { space: Some(0.8), punch: Some(0.5), width: Some(0.6), length: Some(-0.5), noise: Some(0.3), ..Default::default() };
    let r = run(
        &request(
            Op::Fuse,
            Some(Mode::Loop),
            &sources,
            Options { bpm: Some(120.0), bars: Some(1), steer: Some(steer), ..Default::default() },
        ),
        3,
        &sources,
    );
    sound(&r, "steered loop");
    assert_eq!(r.audio[0].len(), (0.125 * RATE * 16.0).round() as usize, "a steered loop keeps its bars");
    seamless(&r, "steered loop");
}

#[test]
fn grow_starts_as_one_sound_and_ends_as_the_other() {
    let pair = [kick(), hat()];
    let grow = |at: f64, seed: u64| {
        let r = run(&request(Op::Grow, None, &pair, Options { at: Some(at), ..Default::default() }), seed, &pair);
        sound(&r, &format!("grow at {at}"));
        no_clicks(&r, &format!("grow at {at}"));
        r
    };
    let centroids: Vec<f64> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|at| colour(&grow(*at, 7)).centroid).collect();
    assert!(centroids.windows(2).all(|w| w[0] < w[1]), "from the low kick toward the bright hat, step by step: {centroids:?}");
    // The ends are the sounds themselves, whatever the path; the middle is the path's own.
    for at in [0.0, 1.0] {
        assert!(grow(at, 1).wav() == grow(at, 2).wav(), "at {at}, every path is at the same sound");
    }
    assert!(grow(0.5, 1).wav() != grow(0.5, 2).wav(), "halfway, another seed is another path");
    assert!(grow(0.5, 1).wav() == grow(0.5, 1).wav(), "one seed, one path");
    assert!(grow(0.35, 3).recipe.contains("Grown 35% of the way from Kick 01 toward Hat 03"), "{}", grow(0.35, 3).recipe);
}

#[test]
fn new_choices_are_refused_where_they_dont_belong() {
    let two = [kick(), snare()];
    let refused = |req: Request| req.check().unwrap_err().0;
    let three = [kick(), snare(), hat()];
    assert!(refused(request(Op::Grow, None, &three, Options::default())).contains("grow takes 2 sounds"));
    assert!(refused(request(Op::Grow, None, &two, Options { bpm: Some(120.0), ..Default::default() })).contains("grow takes at"));
    assert!(refused(request(Op::Grow, Some(Mode::Blend), &two, Options::default())).contains("not of grow"));
    assert!(refused(request(Op::Cross, Some(Mode::Blend), &two, Options { at: Some(0.5), ..Default::default() })).contains("at is grow's"));
    let steer = |s: Steer| refused(request(Op::Stack, None, &two, steered(s)));
    assert!(steer(Steer { bright: Some(1.5), ..Default::default() }).contains("bright is -1 to 1"));
    assert!(steer(Steer { pitch: Some([5.0, -5.0]), ..Default::default() }).contains("low first"));
    assert!(steer(Steer { delay: Some([0.0, 900.0]), ..Default::default() }).contains("delay"));
    let resonate = serde_json::from_str::<kumi_design::choices::Resonate>(r#"{"pairing":"bright-dark"}"#).unwrap();
    assert!(refused(request(Op::Cross, Some(Mode::Blend), &two, Options { resonate: Some(resonate), ..Default::default() }))
        .contains("cross:resonate"));
    let looping = |json: &str, options: Options| {
        refused(request(Op::Fuse, Some(Mode::Loop), &two, Options { r#loop: Some(serde_json::from_str(json).unwrap()), ..options }))
    };
    assert!(looping(r#"{"meter":[7,5]}"#, Options::default()).contains("meter"));
    assert!(looping(r#"{"key":"H"}"#, Options::default()).contains("key"));
    assert!(looping(r#"{"meter":[16,4]}"#, Options { bars: Some(8), bpm: Some(40.0), ..Default::default() }).contains("at most 60 s"));
    assert!(serde_json::from_str::<kumi_design::choices::Loop>(r#"{"scale":"bebop"}"#).is_err(), "an unknown scale isn't read");
}

#[test]
fn a_design_takes_any_number_of_sounds() {
    let many: Vec<Source> = (0..24).map(|i| [kick(), snare(), hat(), bell()][i % 4].clone()).collect();
    let blend = run(&request(Op::Cross, Some(Mode::Blend), &many, Options::default()), 2, &many);
    sound(&blend, "a blend of 24");
    let stack = run(&request(Op::Stack, None, &many[..12], Options::default()), 2, &many[..12]);
    sound(&stack, "a stack of 12");
}

fn looped(json: &str, bpm: f64, bars: u32) -> Options {
    Options { bpm: Some(bpm), bars: Some(bars), r#loop: Some(serde_json::from_str(json).unwrap()), ..Default::default() }
}

/// The loop's energy in its first and last quarters.
fn quarters(render: &Render) -> (f64, f64) {
    let mono = mono_of(render);
    let q = mono.len() / 4;
    let energy = |part: &[f32]| part.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    (energy(&mono[..q]), energy(&mono[mono.len() - q..]))
}

#[test]
fn a_beat_takes_its_meter_and_its_choices() {
    let sources = [groove(), kick(), snare(), hat()];
    let beat = |json: &str, seed: u64| {
        let r = run(&request(Op::Fuse, Some(Mode::Loop), &sources, looped(json, 120.0, 2)), seed, &sources);
        sound(&r, json);
        seamless(&r, json);
        r
    };
    // 7/8 at 120: an eighth is 0.25 s, so two bars are 3.5 s.
    let seven = beat(r#"{"meter":[7,8]}"#, 1);
    assert_eq!(seven.audio[0].len(), (3.5 * RATE) as usize, "two bars of 7/8 at 120");
    assert!(seven.recipe.contains("of 7/8"), "{}", seven.recipe);
    let triplets = beat(r#"{"grid":3}"#, 1);
    assert!(triplets.recipe.contains("triplets"), "{}", triplets.recipe);
    // Leaning late puts the hits near the end, leaning early near the start.
    let (early_start, early_end) = quarters(&beat(r#"{"lean":-1,"density":0.9,"perc":1}"#, 3));
    let (late_start, late_end) = quarters(&beat(r#"{"lean":1,"density":0.9,"perc":1}"#, 3));
    assert!(
        late_end / late_start > early_end / early_start,
        "lean: early {early_start:.1}/{early_end:.1}, late {late_start:.1}/{late_end:.1}"
    );
    for (json, word) in [
        (r#"{"fills":1}"#, "fills"),
        (r#"{"ghosts":1}"#, "ghost notes"),
        (r#"{"ratchet":1}"#, "rolls"),
        (r#"{"poly":1}"#, "perc in"),
        (r#"{"open":1}"#, "open hats"),
        (r#"{"hats":0}"#, "every half beat"),
        (r#"{"hats":1}"#, "every step"),
    ] {
        let r = beat(json, 2);
        assert!(r.recipe.contains(word), "{json}: {}", r.recipe);
    }
}

/// The pitch class (0 C to 11 B) a frequency is nearest, and how far off it is in cents.
fn pitch_class(hz: f64) -> (usize, f64) {
    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
    ((midi.round() as i64).rem_euclid(12) as usize, (midi - midi.round()) * 100.0)
}

#[test]
fn a_melody_plays_in_its_key_scale_and_tuning() {
    let sources = [bell(), pad(), kick()];
    let melody = |json: &str, seed: u64| {
        let r = run(&request(Op::Fuse, Some(Mode::Loop), &sources, looped(json, 100.0, 2)), seed, &sources);
        sound(&r, json);
        seamless(&r, json);
        assert_eq!(r.audio[0].len(), (0.6 * 4.0 * 2.0 * RATE).round() as usize, "{json}: exactly two bars at 100 BPM");
        r
    };
    let plain =
        r#"{"style":"melody","key":"A","scale":"minor-pentatonic","chords":0,"glide":0,"micro":0,"drone":0,"gate":0.9,"density":0.4}"#;
    let r = melody(plain, 4);
    assert!(r.recipe.starts_with("Melody") && r.recipe.contains("in A minor pentatonic"), "{}", r.recipe);
    // Every note heard is one of A minor pentatonic's: A, C, D, E, G.
    let mono = mono_of(&r);
    let heard: Vec<(usize, f64)> =
        analysis::onsets(&mono).into_iter().filter_map(|at| analysis::pitch(&mono, at)).map(pitch_class).collect();
    assert!(heard.len() >= 3, "notes heard: {heard:?}");
    let in_scale = heard.iter().filter(|(class, off)| [9, 0, 2, 4, 7].contains(class) && off.abs() < 35.0).count();
    assert!(in_scale * 10 >= heard.len() * 8, "{in_scale} of {} notes in A minor pentatonic: {heard:?}", heard.len());
    assert!(melody(plain, 4).wav() == r.wav() && melody(plain, 5).wav() != r.wav(), "one seed, one melody; another, another");
    let bent = melody(r#"{"style":"melody","tuning":"24-tet","micro":1}"#, 2);
    assert!(bent.recipe.contains("24-TET") && bent.recipe.contains("bent off the scale"), "{}", bent.recipe);
    let tritave = melody(r#"{"style":"melody","tuning":"bohlen-pierce","scale":"major"}"#, 3);
    assert!(tritave.recipe.contains("Bohlen–Pierce"), "{}", tritave.recipe);
    // A held root fills the whole loop: no eighth of it is silent.
    let held = melody(r#"{"style":"melody","drone":0.9,"density":0.2}"#, 6);
    assert!(held.recipe.contains("a held root"), "{}", held.recipe);
    let mono = mono_of(&held);
    let eighth = mono.len() / 8;
    for part in mono.chunks(eighth).take(8) {
        let rms = (part.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / part.len() as f64).sqrt();
        assert!(rms > 1e-3, "the drone holds through every eighth of the loop ({rms:.5})");
    }
}
