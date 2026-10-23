//! A knob search that starts from what worked before, on a simulated landscape: a bowl whose lowest point is away from
//! where the knobs start, searched as tune searches (generations of CMA-ES, each one listen), from the remembered
//! settings `warm::starts` picks (the nearest past searches' bests), against the same search started cold.
use kumi_listening::{
    cmaes::{seeded, Cmaes},
    heard_settings::{Remembered, SearchKind},
    warm::{starts, Now},
};
use serde_json::{json, Value};

const START: [f64; 3] = [0.5, 0.5, 0.5];
const TARGET: [f64; 3] = [0.85, 0.2, 0.7];
/// One just-noticeable step of each knob, as a fraction of its range.
const STEP: f64 = 0.04;
/// Listens a search may take here before it's counted as never getting there.
const MOST: u32 = 30;

/// How far a setting is from the bowl's lowest point, in steps.
fn steps_off(point: &[f64]) -> f64 {
    point.iter().zip(TARGET).map(|(at, target)| ((at - target) / STEP).powi(2)).sum::<f64>().sqrt()
}

/// The listens (generations) a search takes until its best is within one step of the lowest point (`MOST + 1`: never),
/// and how far off its best was after `budget` listens.
fn listens(seed: u32, remembered: &[Vec<f64>], budget: u32) -> (u32, f64) {
    let mut search = Cmaes::new(&START, 0.2, None, seeded(seed));
    search.inject(remembered);
    let (mut reached, mut after) = (MOST + 1, f64::INFINITY);
    for listen in 1..=MOST {
        let points = search.ask();
        let costs: Vec<f64> = points.iter().map(|point| steps_off(point)).collect();
        search.tell(&points, &costs);
        let best = search.best.as_ref().map_or(f64::INFINITY, |(_, cost)| *cost);
        if listen == budget {
            after = best;
        }
        if best <= 1. && reached > MOST {
            reached = listen;
        }
        if reached <= MOST && listen >= budget {
            break;
        }
    }
    (reached, after)
}

fn past(id: &str, knobs: [f64; 3], target: Value, bass: f64) -> Remembered {
    Remembered {
        id: id.into(),
        at: 1,
        track: Some("Bass".into()),
        kind: SearchKind::Tune,
        knobs: ["Gain", "Freq", "Q"].iter().zip(knobs).map(|(name, at)| (name.to_string(), at)).collect(),
        target,
        before: Some(json!({"values": {"bass": bass}})),
        cost: 0.5,
    }
}

/// What the memory holds for one seed: a past search like this one (much the same aim and sound), whose best lies within
/// a step or so of this search's lowest point; one that aimed elsewhere; and one for another kind of target. What
/// `warm::starts` picks from it.
fn remembered(seed: u32) -> Vec<Vec<f64>> {
    let mut random = seeded(seed + 1_000);
    let like: Vec<f64> = TARGET.iter().map(|at| at + (random() * 2. - 1.) * 1.2 * STEP).collect();
    let region = |aim: f64| json!({"kind": "region", "region": 1, "aim": aim});
    let memory = vec![
        past("like this one", [like[0], like[1], like[2]], region(-7.2), -11.3),
        past("aimed elsewhere", [0.3, 0.8, 0.2], region(-2.0), -5.0),
        past("another kind", [0.1, 0.1, 0.1], json!({"kind": "integrated", "aim": -14.0}), -11.0),
    ];
    let knobs = ["Gain".to_string(), "Freq".to_string(), "Q".to_string()];
    let target = region(-7.5);
    let sounds = [("bass".to_string(), -11.0, 1.0)];
    let now = Now { knobs: &knobs, start: &START, target: &target, aim: Some((-7.5, 1.0)), sounds: &sounds };
    let picked = starts(&memory, &now);
    assert_eq!(picked.iter().map(|start| start.remembered.id.as_str()).collect::<Vec<_>>(), ["like this one"]);
    picked.into_iter().map(|start| start.point).collect()
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let all: Vec<f64> = values.collect();
    all.iter().sum::<f64>() / all.len() as f64
}

#[test]
fn a_warm_start_reaches_the_target_in_fewer_listens_than_a_cold_one() {
    // (tune spends four listens on a search)
    let cold: Vec<(u32, f64)> = (0..50).map(|seed| listens(seed, &[], 4)).collect();
    let warm: Vec<(u32, f64)> = (0..50).map(|seed| listens(seed, &remembered(seed), 4)).collect();
    let (cold_listens, warm_listens) = (mean(cold.iter().map(|run| run.0 as f64)), mean(warm.iter().map(|run| run.0 as f64)));
    let (cold_after, warm_after) = (mean(cold.iter().map(|run| run.1)), mean(warm.iter().map(|run| run.1)));
    let within = |runs: &[(u32, f64)]| runs.iter().filter(|run| run.0 <= 4).count();
    eprintln!(
        "50 seeds: to within one step, cold {cold_listens:.2} listens on average, warm {warm_listens:.2}; within tune's four, cold {} \
         and warm {} of 50; after four listens the best is {cold_after:.2} steps off cold and {warm_after:.2} warm",
        within(&cold),
        within(&warm)
    );
    assert!(warm_listens < cold_listens, "warm {warm:?} cold {cold:?}");
    assert!(within(&warm) > within(&cold) && warm_after < cold_after, "warm {warm:?} cold {cold:?}");
}

#[test]
fn a_remembered_setting_far_from_this_target_does_no_great_harm() {
    let cold: Vec<(u32, f64)> = (0..50).map(|seed| listens(seed, &[], 4)).collect();
    let misled: Vec<(u32, f64)> = (0..50).map(|seed| listens(seed, &[vec![0.1, 0.9, 0.1]], 4)).collect();
    let (cold, misled) = (mean(cold.iter().map(|run| run.0 as f64)), mean(misled.iter().map(|run| run.0 as f64)));
    eprintln!("50 seeds: cold {cold:.2} listens on average, from a wrong memory {misled:.2}");
    assert!(misled <= cold + 1., "misled {misled} cold {cold}");
}
