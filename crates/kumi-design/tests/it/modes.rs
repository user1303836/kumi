//! Resonate's and modulate's choices: every pairing renders a finished sound, the choices left out render the same bytes
//! as no choices at all, and each choice does what it names (who rings through whom, how each resonator rings, which
//! sound is the carrier, which ways shape it and how many times).

use kumi_design::{
    choices::{Carrier, Modulate, Modulation, Pairing, Resonate, Ring},
    render, Mode, Op, Options, Render, Request, Source, SourceRef, RATE,
};
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

fn named(sources: &[Source]) -> Vec<SourceRef> {
    sources.iter().map(|s| SourceRef { file: Some(format!("/tmp/{}", s.name)), name: Some(s.name.clone()), ..Default::default() }).collect()
}

fn request(mode: Mode, sources: &[Source], options: Options) -> Request {
    Request { op: Op::Cross, mode: Some(mode), sources: named(sources), seed: None, options }
}

fn run(request: &Request, seed: u64, sources: &[Source]) -> Render {
    render(request, seed, sources, &mut |_| {}).unwrap_or_else(|e| panic!("{:?}: {e}", request.options))
}

fn resonate(sources: &[Source], seed: u64, choices: Resonate) -> Render {
    run(&request(Mode::Resonate, sources, Options { resonate: Some(choices), ..Default::default() }), seed, sources)
}

fn modulate(sources: &[Source], seed: u64, choices: Modulate) -> Render {
    run(&request(Mode::Modulate, sources, Options { modulate: Some(choices), ..Default::default() }), seed, sources)
}

/// Finite, under -1 dBTP (and normalized to it), audible, a one-line recipe, and starting and ending at silence.
fn finished(render: &Render, what: &str) {
    let len = render.audio[0].len();
    assert!(render.audio.iter().all(|c| c.len() == len && c.iter().all(|v| v.is_finite())), "{what}: finite");
    let tp = 20.0 * render.audio.iter().map(|c| kumi_design::true_peak(c, false)).fold(0.0, f64::max).log10();
    assert!(tp <= -0.99 && tp > -1.2, "{what}: true peak {tp:.2} dBTP");
    let rms = (render.audio[0].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / len as f64).sqrt();
    assert!(20.0 * rms.log10() > -45.0, "{what}: audible ({:.1} dBFS RMS)", 20.0 * rms.log10());
    assert!(!render.recipe.is_empty() && !render.recipe.contains('\n'), "{what}: a one-line recipe");
    for channel in &render.audio {
        assert!(channel[0].abs() < 0.02, "{what}: starts at silence ({})", channel[0]);
        let last = channel[channel.len() - 1].abs();
        assert!(last < 0.002, "{what}: ends at silence ({last})");
    }
}

/// A modulate recipe's stages: what's between the carrier and the depth, one per shaping.
fn stages(render: &Render) -> Vec<String> {
    let (_, rest) = render.recipe.split_once(": ").unwrap();
    let (stages, _) = rest.rsplit_once("; depth").unwrap();
    stages.split(" → ").map(str::to_string).collect()
}

#[test]
fn every_pairing_and_carrier_renders_a_finished_sound() {
    let sources = [kick(), snare(), hat(), pad(), bell()];
    for pairing in Pairing::ALL {
        let r = resonate(&sources, 7, Resonate { pairing: Some(pairing), ..Default::default() });
        finished(&r, &format!("resonate {pairing:?}: {}", r.recipe));
    }
    for ring in Ring::ALL {
        for decay in [0.0, 1.0] {
            let r = resonate(&sources[..3], 4, Resonate { ring: Some(ring), decay: Some(decay), ..Default::default() });
            finished(&r, &format!("resonate {ring:?} decay {decay}: {}", r.recipe));
        }
    }
    for carrier in Carrier::ALL {
        let r = modulate(&sources, 7, Modulate { carrier: Some(carrier), ..Default::default() });
        finished(&r, &format!("modulate {carrier:?}: {}", r.recipe));
    }
    for way in Modulation::ALL {
        let r =
            modulate(&sources[..3], 5, Modulate { kinds: Some(vec![way]), stages: Some(3), retrigger: Some(1.0), ..Default::default() });
        finished(&r, &format!("modulate {way:?}: {}", r.recipe));
    }
}

#[test]
fn choices_left_out_render_the_same_bytes_as_none_and_a_seed_its_own() {
    let sources = [kick(), snare(), hat(), bell()];
    for seed in [1, 2, 3] {
        let none = run(&request(Mode::Resonate, &sources, Options::default()), seed, &sources);
        let empty = resonate(&sources, seed, Resonate::default());
        assert_eq!(none.audio, empty.audio, "resonate, seed {seed}: no choices given renders as none");
        assert_eq!(none.recipe, empty.recipe);
        let halves = resonate(&sources, seed, Resonate { pairing: Some(Pairing::Halves), ..Default::default() });
        assert_eq!(none.audio, halves.audio, "resonate, seed {seed}: halves is the default");
        assert_eq!(halves.recipe, format!("halves: {}", none.recipe));

        let none = run(&request(Mode::Modulate, &sources, Options::default()), seed, &sources);
        let empty = modulate(&sources, seed, Modulate::default());
        assert_eq!(none.audio, empty.audio, "modulate, seed {seed}: no choices given renders as none");
        assert_eq!(none.recipe, empty.recipe);
        let random = modulate(&sources, seed, Modulate { carrier: Some(Carrier::Random), ..Default::default() });
        assert_eq!(none.audio, random.audio, "modulate, seed {seed}: a random carrier is the default");
        assert_eq!(none.recipe, random.recipe);
    }
    let chain = Resonate { pairing: Some(Pairing::Chain), ring: Some(Ring::Both), decay: Some(0.3) };
    assert_eq!(resonate(&sources, 9, chain.clone()).audio, resonate(&sources, 9, chain.clone()).audio, "the same seed, the same bytes");
    assert_ne!(resonate(&sources, 9, chain.clone()).audio, resonate(&sources, 10, chain).audio, "another seed, a variation");
    let shaped = Modulate {
        carrier: Some(Carrier::Tonal),
        kinds: Some(vec![Modulation::Am, Modulation::Filter]),
        stages: Some(4),
        retrigger: Some(0.2),
    };
    assert_eq!(modulate(&sources, 9, shaped.clone()).audio, modulate(&sources, 9, shaped).audio, "the same seed, the same bytes");
}

#[test]
fn bright_dark_rings_the_bright_sound_through_the_dark() {
    for sources in [[hat(), kick()], [kick(), hat()]] {
        let r = resonate(&sources, 3, Resonate { pairing: Some(Pairing::BrightDark), ..Default::default() });
        assert!(r.recipe.starts_with("bright through dark: Hat 03 through Kick 01 ("), "{}", r.recipe);
        let r = resonate(&sources, 3, Resonate { pairing: Some(Pairing::DarkBright), ..Default::default() });
        assert!(r.recipe.starts_with("dark through bright: Kick 01 through Hat 03 ("), "{}", r.recipe);
    }
    // Hits are short and punchy, tones long and pitched.
    let sources = [bell(), hat(), pad(), snare()];
    let r = resonate(&sources, 3, Resonate { pairing: Some(Pairing::HitsTones), ..Default::default() });
    assert!(r.recipe.starts_with("hits through tones: Hat 03 + Snare 02 through "), "{}", r.recipe);
    let r = resonate(&sources, 3, Resonate { pairing: Some(Pairing::TonesHits), ..Default::default() });
    assert!(r.recipe.starts_with("tones through hits: Bell + Pad Chord through "), "{}", r.recipe);
}

#[test]
fn pairings_put_the_sounds_where_they_say() {
    let sources = [kick(), snare(), hat(), bell(), pad()];
    let says = |pairing: Pairing| resonate(&sources, 5, Resonate { pairing: Some(pairing), ..Default::default() }).recipe;
    assert!(says(Pairing::InOrder).starts_with("in order: Kick 01 + Snare 02 + Hat 03 through Bell ("), "{}", says(Pairing::InOrder));
    assert!(
        says(Pairing::Interleave).starts_with("interleaved: Kick 01 + Hat 03 + Pad Chord through Snare 02 ("),
        "{}",
        says(Pairing::Interleave)
    );
    assert!(says(Pairing::OneAll).starts_with("one through all: Kick 01 through Snare 02 ("), "{}", says(Pairing::OneAll));
    assert!(
        says(Pairing::AllOne).starts_with("all through one: Kick 01 + Snare 02 + Hat 03 + Bell through Pad Chord ("),
        "{}",
        says(Pairing::AllOne)
    );
    // A split still names the exciters itself.
    let split = Options {
        split: Some(vec![3]),
        resonate: Some(Resonate { pairing: Some(Pairing::Chain), ..Default::default() }),
        ..Default::default()
    };
    let r = run(&request(Mode::Resonate, &sources, split), 5, &sources);
    assert!(r.recipe.starts_with("Bell through Kick 01 ("), "{}", r.recipe);
}

#[test]
fn a_chain_names_each_link() {
    let sources = [kick(), snare(), hat(), bell()];
    let r = resonate(&sources, 2, Resonate { pairing: Some(Pairing::Chain), ..Default::default() });
    let (links, _) = r.recipe.strip_prefix("chain: Kick 01 through ").unwrap_or_else(|| panic!("{}", r.recipe)).rsplit_once(';').unwrap();
    let names: Vec<&str> = links.split(" → ").map(|link| link.split(" (").next().unwrap()).collect();
    assert_eq!(names, ["Snare 02", "Hat 03", "Bell"], "{}", r.recipe);
}

#[test]
fn a_chosen_ring_rings_every_resonator_that_way() {
    let sources = [kick(), snare(), hat(), bell()];
    for seed in [1, 2] {
        let r = resonate(&sources, seed, Resonate { pairing: Some(Pairing::OneAll), ring: Some(Ring::Body), ..Default::default() });
        assert_eq!(r.recipe.matches("(body)").count(), 3, "{}", r.recipe);
        assert!(!r.recipe.contains("partials"), "{}", r.recipe);
        let tonal = [snare(), bell(), kick()];
        let r = resonate(&tonal, seed, Resonate { pairing: Some(Pairing::OneAll), ring: Some(Ring::Partials), ..Default::default() });
        assert_eq!(r.recipe.matches(" partials, decay").count(), 2, "{}", r.recipe);
        assert!(!r.recipe.contains("body"), "{}", r.recipe);
    }
}

#[test]
fn modulate_takes_the_carrier_ways_and_stages_chosen() {
    let sources = [hat(), kick(), snare()];
    for seed in [1, 2, 3] {
        let r = modulate(&sources, seed, Modulate { carrier: Some(Carrier::Darkest), ..Default::default() });
        assert!(r.recipe.starts_with("Modulate Kick 01 (the darkest): "), "{}", r.recipe);
        let r = modulate(&sources, seed, Modulate { carrier: Some(Carrier::Brightest), ..Default::default() });
        assert!(r.recipe.starts_with("Modulate Hat 03 (the brightest): "), "{}", r.recipe);
        let r = modulate(&sources, seed, Modulate { kinds: Some(vec![Modulation::Filter]), ..Default::default() });
        assert!(stages(&r).iter().all(|s| s.contains(" filter ")), "{}", r.recipe);
        let r = modulate(&sources, seed, Modulate { stages: Some(1), ..Default::default() });
        assert_eq!(stages(&r).len(), 1, "{}", r.recipe);
        let r = modulate(&sources, seed, Modulate { kinds: Some(vec![Modulation::Ring]), stages: Some(3), ..Default::default() });
        assert!(stages(&r).iter().all(|s| s.starts_with("ring by ")), "{}", r.recipe);
        // FM runs once while another way is left to take its turn; never the same way twice running.
        let r = modulate(
            &sources,
            seed,
            Modulate { kinds: Some(vec![Modulation::Fm, Modulation::Gain]), stages: Some(6), ..Default::default() },
        );
        let ways = stages(&r);
        assert_eq!(ways.len(), 6, "{}", r.recipe);
        assert!(ways.windows(2).all(|w| w[0].starts_with("FM") != w[1].starts_with("FM")), "{}", r.recipe);
    }
}
