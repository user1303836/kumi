use kumi_store::{
    heard_settings::{self, HeardSetting, Source, BESTS_PER_DEVICE, PER_DEVICE},
    ids::new_id,
    Store,
};
use serde_json::json;

fn setting(at: i64, device: &str, run: &str, cost: f64) -> HeardSetting {
    HeardSetting {
        id: new_id(),
        at,
        daw: "ableton".into(),
        device: device.into(),
        device_version: None,
        track: Some("Bass".into()),
        source: Source::Tune,
        run_id: run.into(),
        knobs: json!({"Frequency": 0.42, "Gain": 0.61}),
        target: json!({"kind": "region", "region": 1, "item": "bass", "wanted": {"kind": "between", "low": -9.0, "high": -6.0}}),
        before: Some(json!({"values": {"bass": -11.5}, "knobs": {"Frequency": 0.5, "Gain": 0.5}})),
        measured: json!({"bass": -7.2}),
        cost,
        best: false,
        generation: Some(1),
        remembered: None,
    }
}

#[test]
fn settings_are_kept_whole_and_a_search_has_one_best() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let run: Vec<HeardSetting> = (0..6).map(|n| setting(1_000 + n, "Eq8", "run-1", [3.0, 1.5, 2.0, 0.5, 4.0, 0.9][n as usize])).collect();
    let mut other = setting(2_000, "Eq8", "run-2", 0.1);
    other.source = Source::Goal;
    other.before = None;
    let (kept, first) = (run.clone(), other.clone());
    store.write_wait(move |c| heard_settings::add(c, &kept).and_then(|_| heard_settings::add(c, &[first]))).unwrap();
    let best = run[3].id.clone();
    assert_eq!(store.write_wait(move |c| heard_settings::mark_best(c, "run-1", &[best])).unwrap(), 6, "the run's six rows");
    let read = store.read(|c| heard_settings::on_device(c, "ableton", "Eq8", 100)).unwrap();
    assert_eq!(read.len(), 7);
    assert_eq!(read[0], other, "newest first, and kept as it was");
    let marked: Vec<_> = read.iter().filter(|s| s.best).map(|s| s.id.clone()).collect();
    assert_eq!(marked, [run[3].id.clone()], "the other run's rows aren't touched");
    let mut expected = run[3].clone();
    expected.best = true;
    assert!(read.contains(&expected));
    // A better one found later moves the run's best to it.
    let later = run[5].id.clone();
    store.write_wait(move |c| heard_settings::mark_best(c, "run-1", &[later])).unwrap();
    let read = store.read(|c| heard_settings::on_device(c, "ableton", "Eq8", 100)).unwrap();
    assert_eq!(read.iter().filter(|s| s.best).map(|s| s.id.clone()).collect::<Vec<_>>(), [run[5].id.clone()]);
    assert_eq!(store.read(heard_settings::counts).unwrap(), (7, 1));
    assert!(store.read(|c| heard_settings::on_device(c, "reaper", "Eq8", 100)).unwrap().is_empty(), "a DAW's own devices");
}

#[test]
fn a_device_keeps_its_newest_settings_and_every_searchs_best() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    // The two oldest are their searches' best; another device's rows are its own.
    let mut old: Vec<HeardSetting> = (0..4).map(|n| setting(n, "Compressor2", &format!("old-{n}"), 1.0)).collect();
    old[0].best = true;
    old[1].best = true;
    let other = setting(5, "AU: Pro-Q 3", "pro-q", 1.0);
    let (kept, another) = (old.clone(), other.clone());
    store.write_wait(move |c| heard_settings::add(c, &kept).and_then(|_| heard_settings::add(c, &[another]))).unwrap();
    let full: Vec<HeardSetting> = (0..PER_DEVICE as i64).map(|n| setting(10 + n, "Compressor2", "full", 2.0)).collect();
    store.write_wait(move |c| heard_settings::add(c, &full)).unwrap();
    let read = store.read(|c| heard_settings::on_device(c, "ableton", "Compressor2", PER_DEVICE * 2)).unwrap();
    assert_eq!(read.len(), PER_DEVICE + 2, "at most {PER_DEVICE} a device, besides its searches' bests");
    let ids: Vec<&str> = read.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&old[0].id.as_str()) && ids.contains(&old[1].id.as_str()), "a search's best stays, however old");
    assert!(!ids.contains(&old[2].id.as_str()) && !ids.contains(&old[3].id.as_str()), "the oldest others went");
    let others: Vec<i64> = read.iter().filter(|s| !s.best).map(|s| s.at).collect();
    assert_eq!((others.len(), others.last()), (PER_DEVICE, Some(&10)), "the search being added, whole");
    assert_eq!(store.read(|c| heard_settings::on_device(c, "ableton", "AU: Pro-Q 3", 10)).unwrap(), vec![other]);
    assert_eq!(store.read(heard_settings::counts).unwrap(), (PER_DEVICE + 3, 2));
    // The next search makes room a search at a time: the oldest goes whole, its best aside.
    let next: Vec<HeardSetting> = (0..10).map(|n| setting(20_000 + n, "Compressor2", "next", 1.0)).collect();
    store.write_wait(move |c| heard_settings::add(c, &next)).unwrap();
    let read = store.read(|c| heard_settings::on_device(c, "ableton", "Compressor2", PER_DEVICE * 2)).unwrap();
    assert_eq!(read.iter().filter(|s| !s.best).map(|s| s.run_id.as_str()).collect::<Vec<_>>(), ["next"; 10]);
    assert_eq!(read.iter().filter(|s| s.best).count(), 2);
}

#[test]
fn a_search_under_way_keeps_its_rows_however_many_bests_its_device_has() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    // As many past searches' bests as a device keeps rows: a new search's rows were trimmed as they came, and its best
    // could never be marked.
    let bests: Vec<HeardSetting> =
        (0..PER_DEVICE as i64).map(|n| HeardSetting { best: true, ..setting(n, "Eq8", &format!("past-{n}"), 1.0) }).collect();
    store.write_wait(move |c| heard_settings::add(c, &bests)).unwrap();
    let new: Vec<HeardSetting> = (0..10).map(|n| setting(10_000 + n, "Eq8", "new", 2.0 - n as f64 / 10.)).collect();
    let (kept, best) = (new.clone(), new[9].id.clone());
    store.write_wait(move |c| heard_settings::add(c, &kept)).unwrap();
    assert_eq!(store.write_wait(move |c| heard_settings::mark_best(c, "new", &[best])).unwrap(), 10, "all ten kept");
    let bests = store.read(|c| heard_settings::bests_on_device(c, "ableton", "Eq8", &[Source::Tune], PER_DEVICE * 2)).unwrap();
    assert_eq!(bests.len(), BESTS_PER_DEVICE + 1, "the newest bests, and the new search's");
    assert_eq!((bests[0].run_id.as_str(), bests.last().map(|best| best.at)), ("new", Some((PER_DEVICE - BESTS_PER_DEVICE) as i64)));
}

#[test]
fn a_new_search_finds_each_past_searchs_best_on_its_device() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let mut rows = vec![];
    for (n, source) in [Source::Tune, Source::Tune, Source::Goal, Source::Match].into_iter().enumerate() {
        let mut best = setting(100 * n as i64, "Eq8", &format!("run-{n}"), 1.);
        (best.source, best.best) = (source, true);
        let mut other = setting(100 * n as i64 + 1, "Eq8", &format!("run-{n}"), 2.);
        other.source = source;
        rows.extend([best, other]);
    }
    let mut elsewhere = setting(500, "Compressor2", "run-9", 1.);
    elsewhere.best = true;
    rows.push(elsewhere);
    let kept = rows.clone();
    store.write_wait(move |c| heard_settings::add(c, &kept)).unwrap();
    let tuned = store.read(|c| heard_settings::bests_on_device(c, "ableton", "Eq8", &[Source::Tune], 10)).unwrap();
    assert_eq!(tuned.iter().map(|row| row.run_id.as_str()).collect::<Vec<_>>(), ["run-1", "run-0"], "tune's, newest first");
    assert!(tuned.iter().all(|row| row.best));
    let searched = store.read(|c| heard_settings::bests_on_device(c, "ableton", "Eq8", &[Source::Goal, Source::Match], 1)).unwrap();
    assert_eq!(searched.iter().map(|row| row.run_id.as_str()).collect::<Vec<_>>(), ["run-3"], "at most the limit");
}

#[test]
fn a_remembered_setting_that_won_a_searchs_first_round_is_counted() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let remembered = |mut row: HeardSetting, from: &str| {
        row.remembered = Some(from.into());
        row
    };
    let rows = vec![
        // Won its first round.
        remembered(setting(1, "Eq8", "won", 0.5), "old-1"),
        setting(2, "Eq8", "won", 0.9),
        // Tried, beaten in the first round (and its later best doesn't count).
        remembered(setting(3, "Eq8", "lost", 2.0), "old-2"),
        setting(4, "Eq8", "lost", 1.0),
        HeardSetting { generation: Some(2), ..remembered(setting(5, "Eq8", "lost", 0.1), "old-3") },
        // Started from nothing remembered.
        setting(6, "Eq8", "cold", 0.2),
    ];
    store.write_wait(move |c| heard_settings::add(c, &rows)).unwrap();
    assert_eq!(store.read(heard_settings::first_rounds).unwrap(), (2, 1), "two searches started from one; it won one first round");
}

#[test]
fn a_remembered_setting_is_judged_in_the_round_it_was_tried_and_a_search_trimmed_away_isnt_counted() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let heard = |at: i64, run: &str, cost: f64, generation: u32, remembered: bool| HeardSetting {
        source: Source::Goal,
        generation: Some(generation),
        remembered: remembered.then(|| "old".to_string()),
        ..setting(at, "Eq8", run, cost)
    };
    let rows = vec![
        // A goal's slot hears its start first, then the remembered setting beside two others: it won that round.
        heard(1, "won", 3.0, 1, false),
        heard(2, "won", 0.5, 2, true),
        heard(3, "won", 0.9, 2, false),
        heard(4, "won", 1.1, 2, false),
        // Another goal's remembered setting lost its round.
        heard(5, "lost", 2.0, 1, false),
        heard(6, "lost", 1.5, 3, true),
        heard(7, "lost", 1.0, 3, false),
        // Kept only by its best (the rest trimmed away): how its round went can't be told.
        HeardSetting { best: true, ..heard(8, "trimmed", 0.1, 2, true) },
    ];
    store.write_wait(move |c| heard_settings::add(c, &rows)).unwrap();
    assert_eq!(store.read(heard_settings::first_rounds).unwrap(), (2, 1));
}
