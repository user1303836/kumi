//! Kumi's database against its speed budgets (the music plan's §6), on this machine:
//! `cargo run --release -p kumi-store --example store_budgets`. Prints each measure and exits 1 when
//! one is over budget.

use kumi_store::{notes, techniques, Scope, Store};
use std::time::{Duration, Instant};

fn percentile(samples: &mut [Duration], p: f64) -> Duration {
    samples.sort();
    samples[((samples.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("kumi.db");
    let mut rows: Vec<(String, Duration, Duration)> = vec![];

    let started = Instant::now();
    let store = Store::open(&path).unwrap();
    let first_open = started.elapsed();
    drop(store);
    let mut opens: Vec<Duration> = (0..50)
        .map(|_| {
            let started = Instant::now();
            drop(Store::open(&path).unwrap());
            started.elapsed()
        })
        .collect();
    let store = Store::open(&path).unwrap();
    rows.push(("open a new database (create + schema)".into(), first_open, Duration::from_millis(20)));
    rows.push(("open + migrate an existing one, p50".into(), percentile(&mut opens, 0.5), Duration::from_millis(1)));

    let scope = Scope::Project("0123456789abcdef0123456789abcdef".into());
    let kept: Vec<notes::Note> = (0..24)
        .map(|n| notes::Note { label: format!("s{}", n + 1), text: format!("A note about the Set, number {n}."), pinned: n == 0, at: n })
        .collect();
    let all: Vec<techniques::Technique> = (0..40)
        .map(|n| techniques::Technique {
            label: format!("t{}", n + 1),
            name: format!("Technique {n}"),
            fits: "dark basses and pads".into(),
            idea: "x".repeat(600),
            settings: Some("cutoff 400 Hz, resonance 30%".into()),
            substitutes: None,
            recipe: None,
            source_title: Some("A tutorial".into()),
            source_url: Some("https://example.com/t".into()),
            request: Some("a darker reese".into()),
            used: 3.0,
            undone: 0.0,
            at: n,
            updated: None,
            last_used: None,
        })
        .collect();
    let (scope_for, kept_for) = (scope.clone(), kept.clone());
    store.write_wait(move |c| notes::keep(c, &scope_for, &kept_for, 0)).unwrap();
    store.write_wait(move |c| techniques::keep(c, &all, 0)).unwrap();
    let mut loads: Vec<Duration> = (0..200)
        .map(|_| {
            let started = Instant::now();
            assert_eq!(store.read(|c| notes::in_use(c, &scope)).unwrap().len(), 24);
            started.elapsed()
        })
        .collect();
    rows.push(("read 24 notes, p50".into(), percentile(&mut loads, 0.5), Duration::from_millis(1)));
    let mut loads: Vec<Duration> = (0..200)
        .map(|_| {
            let started = Instant::now();
            assert_eq!(store.read(techniques::in_use).unwrap().len(), 40);
            started.elapsed()
        })
        .collect();
    rows.push(("read 40 techniques, p50".into(), percentile(&mut loads, 0.5), Duration::from_millis(1)));

    let mut enqueues = vec![];
    let mut commits = vec![];
    for n in 0..1000 {
        let (sender, answer) = std::sync::mpsc::sync_channel(1);
        let (scope, mut kept) = (scope.clone(), kept.clone());
        kept[(n % 23) + 1].text = format!("Edited {n}");
        let started = Instant::now();
        store.write(move |c| notes::keep(c, &scope, &kept, n as i64), move |result| sender.send(result).unwrap());
        enqueues.push(started.elapsed());
        answer.recv().unwrap().unwrap();
        commits.push(started.elapsed());
    }
    rows.push(("queue a write (the caller's cost), p99".into(), percentile(&mut enqueues, 0.99), Duration::from_micros(50)));
    rows.push(("a write committed, p50".into(), percentile(&mut commits, 0.5), Duration::from_millis(10)));
    rows.push(("a write committed, p99".into(), percentile(&mut commits, 0.99), Duration::from_millis(10)));

    let started = Instant::now();
    let (sender, answers) = std::sync::mpsc::channel();
    for n in 0..10_000 {
        let sender = sender.clone();
        store.write(
            move |c| {
                kumi_store::gaps::add(
                    c,
                    &kumi_store::gaps::Gap {
                        kumi_version: "bench".into(),
                        missing: format!("gap {n}"),
                        asked: None,
                        workaround: None,
                        at: n,
                    },
                )
            },
            move |result| sender.send(result).unwrap(),
        );
    }
    for _ in 0..10_000 {
        answers.recv().unwrap().unwrap();
    }
    rows.push(("10,000 writes queued at once, all committed".into(), started.elapsed(), Duration::from_secs(2)));

    let mut over = false;
    println!("{:<46} {:>12} {:>12}", "measure", "took", "budget");
    for (name, took, budget) in rows {
        let flag = if took > budget { " OVER" } else { "" };
        over |= took > budget;
        println!("{name:<46} {:>12} {:>12}{flag}", format!("{took:.2?}"), format!("{budget:.0?}"));
    }
    std::process::exit(if over { 1 } else { 0 });
}
