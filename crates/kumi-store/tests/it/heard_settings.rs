use kumi_store::{
    heard_settings::{self, HeardSetting, Source, PER_DEVICE},
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
    assert_eq!(read.len(), PER_DEVICE, "at most {PER_DEVICE} a device");
    let ids: Vec<&str> = read.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&old[0].id.as_str()) && ids.contains(&old[1].id.as_str()), "a search's best stays, however old");
    assert!(!ids.contains(&old[2].id.as_str()) && !ids.contains(&old[3].id.as_str()), "the oldest others went");
    let others: Vec<i64> = read.iter().filter(|s| !s.best).map(|s| s.at).collect();
    assert_eq!(others.last(), Some(&12), "then the oldest of the full search's, two of them");
    assert_eq!(store.read(|c| heard_settings::on_device(c, "ableton", "AU: Pro-Q 3", 10)).unwrap(), vec![other]);
    assert_eq!(store.read(heard_settings::counts).unwrap(), (PER_DEVICE + 1, 2));
}
