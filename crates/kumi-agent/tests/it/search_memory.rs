use indexmap::IndexMap;
use kumi_agent::core::{
    contracts::{Generation, GenerationTrial},
    evolve::Knob,
    search_memory::{trials_heard, SearchMemory, SlotSearch},
    store_client::StoreClient,
};
use kumi_listening::heard_settings::{DeviceIdentity, SearchKind, Setting, SettingsHeard, SettingsMemory};
use kumi_store::{
    heard_settings::{self, HeardSetting, Source},
    Store,
};
use kumi_types::daw::DawId;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    rc::Rc,
};

fn memory(dir: &std::path::Path) -> (Rc<SearchMemory>, StoreClient) {
    let client = StoreClient::new(Store::open(dir.join("kumi.db")).unwrap());
    (SearchMemory::with_clock(client.clone(), DawId::Reaper, Rc::new(|| 1_000)), client)
}

/// The rows kept for a device, oldest first, once every queued write has committed.
fn kept(client: &StoreClient, device: &str) -> Vec<HeardSetting> {
    client.store().write_wait(|_| Ok(())).unwrap();
    let device = device.to_string();
    let mut rows = client.store().read(move |c| heard_settings::on_device(c, "reaper", &device, 1_000)).unwrap();
    rows.reverse();
    rows
}

/// A generation of tune's search as it hands it out: each candidate's knobs (fractions of their perceptual ranges),
/// readings and cost; one that didn't come through is heard as infinitely far.
fn generation(run: &str, costs: &[f64]) -> SettingsHeard {
    let settings = costs
        .iter()
        .enumerate()
        .map(|(n, cost)| Setting {
            knobs: IndexMap::from([("Gain-Band 2".to_string(), n as f64 / 10.), ("Freq-Band 2".to_string(), 0.5)]),
            measured: json!({"bass": -9.0 + n as f64, "integrated": -14.2}),
            cost: *cost,
            generation: Some(1),
            remembered: None,
        })
        .filter(|setting| setting.cost.is_finite())
        .collect();
    SettingsHeard::Heard {
        run: run.into(),
        kind: SearchKind::Tune,
        daw: None,
        device: DeviceIdentity::named("VST: ReaEQ (Cockos)"),
        track: Some("Bass".into()),
        target: json!({"kind": "region", "region": 1, "item": "bass", "unit": "dB", "wanted": {"kind": "between", "low": -9.0, "high": -6.0}}),
        before: Some(json!({"values": {"bass": -11.5}, "knobs": {"Gain-Band 2": 0.5, "Freq-Band 2": 0.5}})),
        settings,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_tune_search_keeps_a_row_for_each_setting_it_heard_and_marks_its_best() {
    let dir = tempfile::tempdir().unwrap();
    let (memory, client) = memory(dir.path());
    // Four generations of six, one in each that nothing came through for.
    let generations = [
        [3.0, 2.5, f64::INFINITY, 4.0, 3.5, 2.8],
        [2.2, f64::INFINITY, 1.9, 2.6, 3.1, 2.4],
        [1.2, 1.6, 2.0, f64::INFINITY, 0.7, 1.4],
        [0.9, 1.1, 0.8, 1.3, f64::INFINITY, 1.0],
    ];
    for costs in &generations {
        memory.heard(generation("tune-1", costs));
    }
    let rows = kept(&client, "VST: ReaEQ (Cockos)");
    assert_eq!(rows.len(), 20, "one row for each setting heard");
    assert!(rows.iter().all(|row| !row.best), "its best is told once the search ends");
    memory.heard(SettingsHeard::Ended { run: "tune-1".into(), kept: true });
    let rows = kept(&client, "VST: ReaEQ (Cockos)");
    let best: Vec<&HeardSetting> = rows.iter().filter(|row| row.best).collect();
    assert_eq!(best.len(), 1);
    assert_eq!(best[0].cost, 0.7, "the cheapest it heard");
    let row = &rows[0];
    assert_eq!((row.daw.as_str(), row.source, row.run_id.as_str(), row.track.as_deref()), ("reaper", Source::Tune, "tune-1", Some("Bass")));
    assert_eq!(row.knobs, json!({"Gain-Band 2": 0.0, "Freq-Band 2": 0.5}), "knobs by name, as fractions");
    assert_eq!(row.measured, json!({"bass": -9.0, "integrated": -14.2}));
    assert_eq!(row.target["item"], "bass");
    assert_eq!(row.before.as_ref().unwrap()["values"]["bass"], -11.5);
    assert_eq!((row.at, row.cost), (1_000, 3.0));
    // Another search on the same device keeps its own best, and leaves the first's.
    memory.heard(generation("tune-2", &[5.0, 4.0]));
    memory.heard(SettingsHeard::Ended { run: "tune-2".into(), kept: true });
    let rows = kept(&client, "VST: ReaEQ (Cockos)");
    let best: Vec<(String, f64)> = rows.iter().filter(|row| row.best).map(|row| (row.run_id.clone(), row.cost)).collect();
    assert_eq!(best, [("tune-1".to_string(), 0.7), ("tune-2".to_string(), 4.0)]);
    assert_eq!(client.store().read(heard_settings::counts).unwrap(), (22, 1));
    let status = memory.status().await.unwrap();
    assert_eq!((status.settings, status.devices, status.started, status.won), (22, 1, 0, 0));
}

#[tokio::test(flavor = "current_thread")]
async fn a_setting_heard_at_full_length_is_a_better_best_than_one_heard_on_the_short_window() {
    let dir = tempfile::tempdir().unwrap();
    let (memory, client) = memory(dir.path());
    let heard = |cost: f64, screened: bool| SettingsHeard::Heard {
        run: "goal-1".into(),
        kind: SearchKind::Goal,
        daw: Some(DawId::Reaper),
        device: DeviceIdentity::named("Saturn 2"),
        track: Some("Candidate".into()),
        target: json!({"kind": "reference"}),
        before: None,
        settings: vec![Setting {
            knobs: IndexMap::from([("Drive".to_string(), cost / 100.)]),
            measured: json!({"screened": screened}),
            cost,
            generation: None,
            remembered: None,
        }],
    };
    memory.heard(heard(10., true));
    memory.heard(heard(30., false));
    memory.heard(heard(20., false));
    memory.heard(SettingsHeard::Ended { run: "goal-1".into(), kept: true });
    let best: Vec<f64> = kept(&client, "Saturn 2").iter().filter(|row| row.best).map(|row| row.cost).collect();
    assert_eq!(best, [20.], "the cheapest heard whole, not the cheaper short one");
    // A search that never heard anything marks nothing, and an ending told twice changes nothing.
    memory.heard(SettingsHeard::Ended { run: "goal-1".into(), kept: true });
    memory.heard(SettingsHeard::Ended { run: "nothing".into(), kept: true });
    assert_eq!(kept(&client, "Saturn 2").iter().filter(|row| row.best).count(), 1);
    // A search whose best the device didn't take (nothing beat where the knobs were) isn't what worked: no best.
    let mut other = heard(5., false);
    if let SettingsHeard::Heard { run, .. } = &mut other {
        *run = "goal-2".into();
    }
    memory.heard(other);
    memory.heard(SettingsHeard::Ended { run: "goal-2".into(), kept: false });
    let bests: Vec<String> = kept(&client, "Saturn 2").iter().filter(|row| row.best).map(|row| row.run_id.clone()).collect();
    assert_eq!(bests, ["goal-1"]);
}

fn knob(device: &str, name: &str, min: f64, max: f64, value: f64) -> Knob {
    Knob { r#ref: format!("{device}/{name}"), device: device.into(), name: name.into(), min, max, step: None, value }
}

#[test]
fn a_goals_trials_are_settings_on_each_device_of_their_chain() {
    let knobs = vec![
        knob("0:My EQ", "Gain", -15., 15., 0.),
        knob("0:My EQ", "Freq", 20., 20_000., 1_000.),
        knob("1:Glue", "Threshold", -60., 0., -10.),
    ];
    // The rig named the EQ (renamed by the producer) by who it is; the limiter it didn't name goes by its name.
    let devices = BTreeMap::from([("0:My EQ".to_string(), "VST3: Pro-Q 3 (FabFilter)".to_string())]);
    let searches = HashMap::from([("Candidate".to_string(), SlotSearch::new("Candidate", &knobs, devices))]);
    let trials = vec![
        GenerationTrial { slot: "Candidate".into(), knobs: knobs.clone(), values: vec![7.5, 20_000., -30.], fresh: None },
        GenerationTrial { slot: "Unscored".into(), knobs: knobs.clone(), values: vec![0., 0., 0.], fresh: None },
    ];
    let generation = Generation {
        scores: HashMap::from([("Candidate".to_string(), 82.5)]),
        gaps: HashMap::from([("Candidate".to_string(), vec!["brighter".to_string()])]),
        screened: true,
        ..Default::default()
    };
    let target = json!({"kind": "reference", "reference": "/refs/pad.wav", "goal": "make my pad sound like it"});
    let heard = trials_heard(SearchKind::Goal, &searches, &trials, &generation, &target, Some(3), &[]);
    let run = searches["Candidate"].run.clone();
    let expect = |device: &str, knobs: Value, before: Value| SettingsHeard::Heard {
        run: run.clone(),
        kind: SearchKind::Goal,
        daw: None,
        device: DeviceIdentity::named(device),
        track: Some("Candidate".into()),
        target: target.clone(),
        before: Some(json!({ "knobs": before })),
        settings: vec![Setting {
            knobs: knobs.as_object().unwrap().iter().map(|(name, at)| (name.clone(), at.as_f64().unwrap())).collect(),
            measured: json!({"score": 82.5, "screened": true, "gaps": ["brighter"]}),
            cost: 17.5,
            generation: Some(3),
            remembered: None,
        }],
    };
    assert_eq!(
        heard,
        [
            expect("VST3: Pro-Q 3 (FabFilter)", json!({"Gain": 0.75, "Freq": 1.0}), json!({"Gain": 0.5, "Freq": 980. / 19_980.})),
            expect("Glue", json!({"Threshold": 0.5}), json!({"Threshold": 50. / 60.})),
        ]
    );
}

/// A goal's (or a match's) setting, as a search heard and kept it, from a slot whose knobs are `knobs`.
fn goal_heard(run: &str, device: &str, knobs: &[(&str, f64)], score: f64, reference: Option<&str>) -> SettingsHeard {
    SettingsHeard::Heard {
        run: run.into(),
        kind: SearchKind::Goal,
        daw: None,
        device: DeviceIdentity::named(device),
        track: Some("Pad".into()),
        target: match reference {
            Some(reference) => json!({"kind": "reference", "reference": reference}),
            None => json!({"kind": "region"}),
        },
        before: None,
        settings: vec![Setting {
            knobs: knobs.iter().map(|(name, at)| (name.to_string(), *at)).collect(),
            measured: json!({"score": score, "screened": false}),
            cost: 100. - score,
            generation: Some(1),
            remembered: None,
        }],
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_search_reads_each_past_searchs_best_on_its_device() {
    let dir = tempfile::tempdir().unwrap();
    let (memory, client) = memory(dir.path());
    memory.heard(generation("tune-1", &[3.0, 1.0, 2.0]));
    memory.heard(SettingsHeard::Ended { run: "tune-1".into(), kept: true });
    memory.heard(goal_heard("goal-1", "VST: ReaEQ (Cockos)", &[("Gain-Band 2", 0.9)], 80., Some("/refs/pad.wav")));
    memory.heard(SettingsHeard::Ended { run: "goal-1".into(), kept: true });
    client.store().write_wait(|_| Ok(())).unwrap();
    let tuned = memory.bests(None, "VST: ReaEQ (Cockos)", &[SearchKind::Tune]).await;
    assert_eq!(tuned.len(), 1, "one search, its best");
    assert_eq!((tuned[0].kind, tuned[0].cost, tuned[0].track.as_deref()), (SearchKind::Tune, 1.0, Some("Bass")));
    assert_eq!(tuned[0].knobs, IndexMap::from([("Gain-Band 2".to_string(), 0.1), ("Freq-Band 2".to_string(), 0.5)]));
    assert_eq!(tuned[0].target["item"], "bass");
    let searched = memory.bests(Some(DawId::Reaper), "VST: ReaEQ (Cockos)", &[SearchKind::Goal, SearchKind::Match]).await;
    assert_eq!(searched.iter().map(|best| best.kind).collect::<Vec<_>>(), [SearchKind::Goal]);
    assert!(memory.bests(Some(DawId::Ableton), "VST: ReaEQ (Cockos)", &[SearchKind::Tune]).await.is_empty(), "another DAW's");
}

#[tokio::test(flavor = "current_thread")]
async fn a_goals_new_slot_tries_the_closest_match_its_devices_came_to_before() {
    let dir = tempfile::tempdir().unwrap();
    let (memory, client) = memory(dir.path());
    // The EQ's closest match to its reference (90%) beat an older 70%; a region's search isn't a match's.
    memory.heard(goal_heard("old", "VST3: Pro-Q 3 (FabFilter)", &[("Gain", 0.25)], 70., Some("/refs/pad.wav")));
    memory.heard(goal_heard("close", "VST3: Pro-Q 3 (FabFilter)", &[("Gain", 0.75), ("Freq", 0.5)], 90., Some("/refs/pad.wav")));
    memory.heard(goal_heard("region", "VST3: Pro-Q 3 (FabFilter)", &[("Gain", 0.1)], 99., None));
    for run in ["old", "close", "region"] {
        memory.heard(SettingsHeard::Ended { run: run.into(), kept: true });
    }
    client.store().write_wait(|_| Ok(())).unwrap();
    let knobs = vec![
        knob("0:My EQ", "Gain", -15., 15., 0.),
        Knob { step: Some(1.), ..knob("0:My EQ", "Freq", 20., 20_000., 1_000.) },
        knob("1:Glue", "Threshold", -60., 0., -10.),
    ];
    let devices = BTreeMap::from([("0:My EQ".to_string(), "VST3: Pro-Q 3 (FabFilter)".to_string())]);
    let search = SlotSearch::new("Candidate", &knobs, devices);
    let elite = vec![0., 1_000., -10.];
    let pad = json!("/refs/pad.wav");
    let (values, from) = memory.suggestion(&search, &pad, &knobs, &elite).await.expect("a suggestion");
    assert_eq!(values, [7.5, 10_010., -10.], "the EQ's knobs where they matched best (on its steps); Glue as it is");
    assert_eq!(from.keys().collect::<Vec<_>>(), ["VST3: Pro-Q 3 (FabFilter)"]);
    // Where the slot already is, or a device nothing is remembered for: nothing to try.
    assert_eq!(memory.suggestion(&search, &pad, &knobs, &values).await, None);
    let glue = SlotSearch::new("Other", &knobs[2..], BTreeMap::new());
    assert_eq!(memory.suggestion(&glue, &pad, &knobs[2..], &[-10.]).await, None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_slot_starts_from_the_closest_match_toward_its_own_reference_else_the_newest_search() {
    let dir = tempfile::tempdir().unwrap();
    let client = StoreClient::new(Store::open(dir.path().join("kumi.db")).unwrap());
    let clock = Rc::new(std::cell::Cell::new(1_000));
    let ticks = clock.clone();
    let memory = SearchMemory::with_clock(client.clone(), DawId::Reaper, Rc::new(move || ticks.replace(ticks.get() + 1)));
    // A kick was easy to match (95%), a pad less so (72%): how close each came is to its own reference.
    memory.heard(goal_heard("pad", "Operator", &[("Drive", 0.2)], 72., Some("/refs/pad.wav")));
    memory.heard(goal_heard("kick", "Operator", &[("Drive", 0.9)], 95., Some("/refs/kick.wav")));
    for run in ["pad", "kick"] {
        memory.heard(SettingsHeard::Ended { run: run.into(), kept: true });
    }
    client.store().write_wait(|_| Ok(())).unwrap();
    let knobs = vec![knob("0:Operator", "Drive", 0., 1., 0.5)];
    let search = SlotSearch::new("Pad", &knobs, BTreeMap::new());
    let tried = |reference: &'static str| {
        let (memory, search, knobs) = (memory.clone(), &search, &knobs);
        async move { memory.suggestion(search, &json!(reference), knobs, &[0.5]).await.map(|(values, _)| values) }
    };
    assert_eq!(tried("/refs/pad.wav").await, Some(vec![0.2]), "the pad's own, not the kick's");
    assert_eq!(tried("/refs/new.wav").await, Some(vec![0.9]), "toward a reference no search went for: the newest search's");
    assert!(clock.get() > 1_000);
}

#[test]
fn the_same_device_twice_in_a_chain_is_remembered_as_two() {
    // EQ Eight, Glue, EQ Eight: each EQ's knobs are its own, and the second is "EQ Eight #2".
    let knobs = vec![
        knob("0:EQ Eight", "Gain", -15., 15., 0.),
        knob("1:Glue", "Threshold", -60., 0., -10.),
        knob("2:EQ Eight", "Gain", -15., 15., 0.),
    ];
    let searches = HashMap::from([("Bus".to_string(), SlotSearch::new("Bus", &knobs, BTreeMap::new()))]);
    let trials = vec![GenerationTrial { slot: "Bus".into(), knobs: knobs.clone(), values: vec![7.5, -30., -7.5], fresh: None }];
    let generation = Generation { scores: HashMap::from([("Bus".to_string(), 80.)]), ..Default::default() };
    let heard = trials_heard(SearchKind::Goal, &searches, &trials, &generation, &json!({}), Some(1), &[]);
    let devices: Vec<(String, Vec<(String, f64)>)> = heard
        .into_iter()
        .map(|heard| match heard {
            SettingsHeard::Heard { device, settings, .. } => (device.name, settings[0].knobs.clone().into_iter().collect()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        devices,
        [
            ("EQ Eight".to_string(), vec![("Gain".to_string(), 0.75)]),
            ("Glue".to_string(), vec![("Threshold".to_string(), 0.5)]),
            ("EQ Eight #2".to_string(), vec![("Gain".to_string(), 0.25)]),
        ]
    );
}

#[test]
fn a_remembered_trial_says_which_setting_it_was() {
    let knobs = vec![knob("0:Saturn", "Drive", 0., 1., 0.2)];
    let mut search = SlotSearch::new("Candidate", &knobs, BTreeMap::from([("0:Saturn".to_string(), "Saturn 2".to_string())]));
    search.suggested = BTreeMap::from([("Saturn 2".to_string(), "row-7".to_string())]);
    let searches = HashMap::from([("Candidate".to_string(), search)]);
    let trials = vec![GenerationTrial { slot: "Candidate".into(), knobs, values: vec![0.6], fresh: None }];
    let generation = Generation { scores: HashMap::from([("Candidate".to_string(), 75.)]), ..Default::default() };
    let heard = |remembered: &[String]| trials_heard(SearchKind::Goal, &searches, &trials, &generation, &json!({}), Some(2), remembered);
    let setting = |heard: Vec<SettingsHeard>| match heard.into_iter().next() {
        Some(SettingsHeard::Heard { settings, .. }) => settings.into_iter().next().unwrap(),
        other => panic!("{other:?}"),
    };
    let tried = setting(heard(&["Candidate".to_string()]));
    assert_eq!((tried.generation, tried.remembered.as_deref()), (Some(2), Some("row-7")));
    assert_eq!(setting(heard(&[])).remembered, None, "a trial of its own");
}
