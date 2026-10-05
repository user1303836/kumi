use kumi_runtime::core::evolve::*;
use std::collections::{HashMap, HashSet};
fn knobs(device: &str, values: &[f64]) -> Vec<Knob> {
    values
        .iter()
        .enumerate()
        .map(|(i, &value)| Knob {
            r#ref: format!("{device}:{i}"),
            device: device.into(),
            name: format!("Knob {i}"),
            min: 0.0,
            max: 1.0,
            step: None,
            value,
        })
        .collect()
}
fn slot(name: &str, chain: &str, values: &[f64]) -> NewSlot {
    NewSlot { name: name.into(), label: name.into(), chain: chain.into(), knobs: knobs(chain, values), score: None, heard: None }
}
fn scored(e: &mut Evolution, scores: &[(&str, f64)]) {
    let trials = e.propose();
    e.scored(&trials, &scores.iter().map(|(n, v)| (n.to_string(), *v)).collect());
}
fn score(values: &[f64], offset: f64) -> f64 {
    let target = [0.8, 0.2, 0.65, 0.4, 0.9];
    (100.0 - offset - 120.0 * (values.iter().zip(target).map(|(v, t)| (v - t).powi(2)).sum::<f64>() / values.len() as f64).sqrt()).max(0.0)
}
#[test]
fn search_climbs_keeps_each_slots_best_and_never_loses_the_leader() {
    let mut e = Evolution::new(seeded(7), EVOLVE);
    e.add(slot("Kumi · Goal · A", "Operator", &[0.5; 5]));
    e.add(slot("Kumi · Goal · B", "Operator", &[0.1, 0.9, 0.1, 0.9, 0.1]));
    e.add(slot("Kumi · Goal · C", "Drift", &[0.5; 5]));
    let mut bests = vec![];
    for _ in 0..60 {
        let trials = e.propose();
        assert_eq!(trials.len(), 3);
        let scores = trials.iter().map(|t| (t.slot.clone(), score(&t.values, if t.slot.ends_with('C') { 25.0 } else { 0.0 }))).collect();
        e.scored(&trials, &scores);
        bests.push(e.best().unwrap());
    }
    assert!(bests.windows(2).all(|w| w[1] >= w[0] - 1e-9));
    assert!(bests[0] < 75.0 && bests[59] > 88.0, "{} → {}", bests[0], bests[59]);
    assert_eq!(e.leader().unwrap().chain, "Operator");
    assert!(e.slots.iter().any(|s| s.chain == "Drift"));
    assert_eq!(e.rendered, 180);
    assert_eq!(e.trend.len(), 60);
}
#[test]
fn crossover_only_mixes_one_chain_and_stuck_slots_are_reseeded() {
    let mut e = Evolution::new(seeded(3), EvolveOptions { moves: 2, crossover: 1.0, random: 0.0, patience: 2, recheck: 99 });
    e.add(slot("A", "Operator", &[0.0; 3]));
    e.add(slot("B", "Drift", &[1.0; 3]));
    assert_eq!(e.propose().iter().map(|t| t.how).collect::<Vec<_>>(), vec![TrialHow::Start; 2]);
    scored(&mut e, &[("A", 50.0), ("B", 40.0)]);
    assert!(e.propose().iter().all(|t| t.how == TrialHow::Nudge));
    for _ in 0..2 {
        scored(&mut e, &[("A", 50.0), ("B", 10.0)]);
    }
    assert_eq!(e.slots.iter().find(|s| s.name == "B").unwrap().score, None);
    assert_eq!(e.propose().iter().find(|t| t.slot == "B").unwrap().how, TrialHow::Start);
}
#[test]
fn search_leaves_silencing_switches_levels_and_safety_limiter_alone_and_snaps_steps() {
    let mut all = knobs("Operator", &[1.0, 0.8, 0.5, 0.5, 3.0, 1.0]);
    for (k, name) in all.iter_mut().zip(["Device On", "Volume", "Filter Freq", "Gain", "Algorithm", "Fixed"]) {
        k.name = name.into();
    }
    all[3].device = "Limiter".into();
    all[4].max = 10.0;
    all[4].step = Some(1.0);
    all[5].min = 1.0;
    assert_eq!(searchable(&all, MOST_KNOBS).iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), vec!["Filter Freq", "Algorithm"]);
    let mut e = Evolution::new(seeded(11), EvolveOptions { moves: 2, crossover: 0.0, random: 1.0, patience: 9, recheck: 99 });
    let mut s = slot("A", "Operator", &[]);
    s.knobs = all;
    e.add(s);
    scored(&mut e, &[("A", 10.0)]);
    for _ in 0..20 {
        let trials = e.propose();
        assert_eq!(trials[0].values[1].fract(), 0.0);
        e.scored(&trials, &HashMap::from([("A".into(), 5.0)]));
    }
}
#[test]
fn refused_knobs_leave_search_and_sound_shaping_knobs_come_first() {
    let mut e = Evolution::new(seeded(2), EVOLVE);
    e.add(slot("A", "Operator", &[0.1, 0.2, 0.3]));
    scored(&mut e, &[("A", 40.0)]);
    e.freeze("A", &HashSet::from(["Operator|Knob 1".into()]));
    assert_eq!(e.slots[0].knobs.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), vec!["Knob 0", "Knob 2"]);
    assert_eq!(e.slots[0].elite, vec![0.1, 0.3]);
    assert_eq!(e.propose()[0].values.len(), 2);
    let mut many = knobs("Operator", &[0.0; 60]);
    for (i, k) in many.iter_mut().enumerate() {
        k.name = if i == 59 { "Filter Freq".into() } else { format!("Osc-B Unused {i}") };
    }
    let chosen = searchable(&many, MOST_KNOBS);
    assert_eq!(chosen.len(), 24);
    assert_eq!(chosen[0].name, "Filter Freq");
}
#[test]
fn held_best_is_heard_again_so_a_lucky_render_cannot_hold_the_search() {
    let mut e = Evolution::new(seeded(4), EvolveOptions { moves: 1, crossover: 0.0, random: 0.0, patience: 99, recheck: 2 });
    e.add(slot("A", "Operator", &[0.5]));
    scored(&mut e, &[("A", 90.0)]);
    let mut hows = vec![];
    for _ in 0..6 {
        let trials = e.propose();
        hows.push(trials[0].how);
        e.scored(&trials, &HashMap::from([("A".into(), if trials[0].how == TrialHow::Recheck { 70.0 } else { 60.0 })]));
    }
    assert!(hows.contains(&TrialHow::Recheck));
    assert!(e.best().unwrap() < 90.0);
}
#[test]
fn a_knob_live_refused_on_the_leader_never_breaks_a_reseeded_slot() {
    // Live refuses one of the leader's knobs (frozen out of its elite); a stuck slot of the same chain
    // is then reseeded from the leader. Every slot keeps one value per knob, so the search goes on.
    let mut e = Evolution::new(seeded(5), EvolveOptions { moves: 3, crossover: 0.0, random: 0.0, patience: 2, recheck: 99 });
    e.add(slot("A", "Operator", &[0.5, 0.6, 0.7, 0.8]));
    e.add(slot("B", "Operator", &[0.1, 0.1, 0.1, 0.1]));
    scored(&mut e, &[("A", 50.0), ("B", 40.0)]);
    e.freeze("A", &HashSet::from(["Operator|Knob 1".to_string()]));
    for _ in 0..12 {
        scored(&mut e, &[("A", 50.0), ("B", 10.0)]);
        for slot in &e.slots {
            assert_eq!(slot.elite.len(), slot.knobs.len(), "{}", slot.name);
        }
        for trial in e.propose() {
            let knobs = e.slots.iter().find(|s| s.name == trial.slot).unwrap().knobs.len();
            assert_eq!(trial.values.len(), knobs, "{}", trial.slot);
        }
    }
    // B was reseeded from A: A's values for the knobs they share, B's own where A has none.
    let a = e.slots.iter().find(|s| s.name == "A").unwrap();
    assert_eq!(a.knobs.len(), 3);
    let b = e.slots.iter().find(|s| s.name == "B").unwrap();
    assert_eq!(b.knobs.len(), 4);
}
