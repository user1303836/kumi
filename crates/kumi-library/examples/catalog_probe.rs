//! Measure the first built-in format-catalog initialization in a fresh process.
//! Run an optimized binary repeatedly; Linux RSS includes the allocator's retained
//! pages and is not an exact heap-allocation count. Other platforms omit RSS.

use std::time::Instant;

use kumi_library::plugins::formats::structure;
use serde_json::json;

#[cfg(target_os = "linux")]
fn rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status").ok()?.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == "VmRSS:").then(|| words.next()?.parse().ok()).flatten()
    })
}

#[cfg(not(target_os = "linux"))]
fn rss_kib() -> Option<u64> {
    None
}

fn main() {
    let before = rss_kib();
    let started = Instant::now();
    let formats = structure::builtins();
    let elapsed = started.elapsed();
    let after = rss_kib();
    println!(
        "{}",
        json!({
            "products": formats.len(),
            "fields": formats.iter().map(|format| format.structure.fields.len()).sum::<usize>(),
            "first_builtins_ms": elapsed.as_secs_f64() * 1000.0,
            "rss_before_kib": before,
            "rss_after_kib": after,
            "rss_increase_kib": before.zip(after).map(|(before, after)| after.saturating_sub(before)),
        })
    );
}
