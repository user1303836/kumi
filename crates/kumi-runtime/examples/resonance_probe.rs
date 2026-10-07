//! What the judge reads for a peak in a capture, and what a cut there would read as: for checking the resonance measure
//! and a planned cut against real audio. resonance_probe <wav> <low hz> <high hz> [<cut hz> <cut dB> <cut Q>]
use kumi_runtime::{
    audio::decode::open_audio,
    listening::{
        checklist::{plan_cut, region_excess, worst_stretch},
        detect,
        fit::{filter, Band, Shape},
        measure::{fine_hz, measure_file, measure_samples, MeasureOptions, FINE_BINS},
    },
};

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    runtime.block_on(async {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let number = |at: usize| argv.get(at).and_then(|value| value.parse::<f64>().ok());
        let (Some(path), Some(low), Some(high)) = (argv.first(), number(1), number(2)) else {
            eprintln!("resonance_probe <wav> <low hz> <high hz> [<cut hz> <cut dB> <cut Q>]");
            std::process::exit(2);
        };
        let heard = measure_file(path, MeasureOptions::default()).await.expect("a readable file");
        let frames = &heard.frames;
        println!("{} frames every {:.3} s", frames.fine.len(), frames.hop);
        for problem in detect::harshness(&heard) {
            println!(
                "found {:?} {:?}: {} (excess {}, steady {:?})",
                problem.kind, problem.hz, problem.what, problem.excess, problem.steady
            );
        }
        println!(
            "steady excess {:.1} dB, intermittent {:.1} dB",
            region_excess(&heard, low, high, true),
            region_excess(&heard, low, high, false)
        );
        if let Some(at) = worst_stretch(&heard, low, high, true, 14., 1.7) {
            println!("stands out most from {at:.1} s");
        }
        // The median level of each fine bin near the band, over the loud frames.
        let mut levels: Vec<f32> = frames.level.clone();
        levels.sort_by(f32::total_cmp);
        let loud = levels.get(levels.len() * 95 / 100).copied().unwrap_or(0.) - 20.;
        for bin in 0..FINE_BINS {
            let hz = fine_hz(bin);
            if hz < low / 2. || hz > high * 2. {
                continue;
            }
            let mut values: Vec<f32> = (0..frames.fine.len()).filter(|f| frames.level[*f] >= loud).map(|f| frames.fine[f][bin]).collect();
            values.sort_by(f32::total_cmp);
            let median = values.get(values.len() / 2).copied().unwrap_or(f32::NAN);
            println!("{:>8.0} Hz {:>7.1} dB{}", hz, median, if (low..=high).contains(&hz) { "  <" } else { "" });
        }
        let (band, predicted) = plan_cut(&heard, low, high, true, 5.5, 4., 48_000.);
        println!("planned: {} dB at {:.0} Hz, Q {:.1} → predicted {:.1} dB", band.db, band.hz, band.q, predicted);
        if let (Some(hz), Some(db), Some(q)) = (number(3), number(4), number(5)) {
            let mut source = open_audio(path, None).await.expect("a readable file");
            let rate = source.sample_rate;
            let (mut left, mut right) = (vec![], vec![]);
            while let Some(block) = source.read(65536).await.expect("audio") {
                left.extend_from_slice(&block[0]);
                right.extend_from_slice(block.get(1).unwrap_or(&block[0]));
            }
            let cut = Band { shape: Shape::Bell, hz, db, q };
            filter(&mut left, &cut, rate);
            filter(&mut right, &cut, rate);
            let after = measure_samples(&left, &right, rate);
            println!("with {db} dB at {hz} Hz, Q {q}: steady excess {:.1} dB", region_excess(&after, low, high, true));
        }
    });
}
