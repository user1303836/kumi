//! Isolated maximum-input benchmark. Allocation fields conservatively report total native heap;
//! they do not imply a V8 heap/external/ArrayBuffer distinction. RSS is measured by the OS.
use crate::{
    analysis::{analyze_pcm, PcmAnalysisInput},
    benchmark::{audio_fixture, memory, ANALYSIS_MEASUREMENTS},
};
use kumi_common::{
    js::{json, number},
    time::perf_now,
};
use serde_json::{json as json_value, Value};

fn run(args: &[String]) -> Result<Value, String> {
    let slow = args.iter().find_map(|v| v.strip_prefix("--slow=")).and_then(number::parse).unwrap_or(0.);
    let allocate = args.iter().any(|v| v == "--allocate");
    let samples = audio_fixture();
    let input = PcmAnalysisInput { samples: &samples, sample_rate: 48_000., channels: None, channel_layout: None, frame_size: None };
    let mut retained = Vec::new();
    let mut latencies = Vec::new();
    let mut result = analyze_pcm(&input).map_err(|e| e.to_string())?;
    for _ in 0..ANALYSIS_MEASUREMENTS {
        let started = perf_now();
        if slow > 0. {
            let until = perf_now() + slow;
            while perf_now() < until {
                std::hint::spin_loop();
            }
        }
        result = analyze_pcm(&input).map_err(|e| e.to_string())?;
        if allocate {
            retained.push(vec![0.0_f64; samples.len()]);
            retained.push(vec![0.0_f64; samples.len()]);
        }
        latencies.push(perf_now() - started);
    }
    let output = json::stringify(&serde_json::to_value(result).unwrap()).len();
    std::hint::black_box(&retained);
    let (_, peak) = memory::allocation_bytes()?;
    Ok(
        json_value!({"peakRssBytes":memory::peak_rss_bytes()?,"peakHeapUsedBytes":peak,"peakExternalBytes":peak,"peakArrayBuffersBytes":peak,"latencyMeasurements":latencies,"outputBytes":output}),
    )
}
pub fn main(args: &[String]) -> i32 {
    match run(args) {
        Ok(result) => {
            println!("{}", json::stringify(&result));
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
