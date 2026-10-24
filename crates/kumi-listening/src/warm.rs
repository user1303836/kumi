//! Where a knob search starts from what worked before: of the settings Kumi remembers on the same device (each past
//! search's best), those for the same knobs and the same kind of target, from the past searches nearest this one: what
//! they aimed at and how the track sounded before them, each in just-noticeable steps. The nearest few are heard in the
//! search's first generation (`Cmaes::inject`).

use crate::heard_settings::Remembered;
use serde_json::Value;

/// The most remembered settings a search starts from.
pub const MOST: usize = 3;
/// Two settings whose knobs are each this close (as fractions of their ranges) are one: hearing both teaches nothing.
const SAME: f64 = 0.02;
/// How far apart two searches' sounds are taken to be when nothing they measured can be compared, in steps.
const UNKNOWN: f64 = 3.;
/// The farthest a past search can be from this one (what it aimed at and how the track sounded, in steps) for its best to
/// be worth hearing here: one further off was another job, and its setting would pull the search away.
const FARTHEST: f64 = 4.;

/// A search about to start, as the remembered ones are compared with it.
pub struct Now<'a> {
    /// The knobs it searches, by name, and where each is (a fraction of its range).
    pub knobs: &'a [String],
    pub start: &'a [f64],
    /// What it aims at, as remembered (the checklist item's kind and what it wants).
    pub target: &'a Value,
    /// Where a knob turned for it should take its item, and that item's just-noticeable step.
    pub aim: Option<(f64, f64)>,
    /// How the track sounds now: each checklist item's id, value and just-noticeable step.
    pub sounds: &'a [(String, f64, f64)],
}

/// A remembered setting to start from: its knobs in the search's order, and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Start {
    pub point: Vec<f64>,
    pub remembered: Remembered,
    /// How far its search was from this one (in steps: what it aimed at, and how the track sounded before).
    pub distance: f64,
}

/// The remembered settings a search starts from, nearest first: at most `MOST`, none from a search far from this one,
/// none for other knobs or another kind of target, none twice, and none where the search already is.
pub fn starts(remembered: &[Remembered], now: &Now) -> Vec<Start> {
    let mut near: Vec<Start> = remembered
        .iter()
        .filter(|past| same_kind(&past.target, now.target))
        .filter_map(|past| {
            let point = now.knobs.iter().map(|name| past.knobs.get(name).copied()).collect::<Option<Vec<f64>>>()?;
            Some(Start { point, remembered: past.clone(), distance: distance(past, now) })
        })
        .filter(|start| start.distance <= FARTHEST)
        .collect();
    near.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(b.remembered.at.cmp(&a.remembered.at)));
    let mut picked: Vec<Start> = vec![];
    for start in near {
        let known = |point: &[f64]| point.iter().zip(&start.point).all(|(a, b)| (a - b).abs() <= SAME);
        if known(now.start) || picked.iter().any(|kept| known(&kept.point)) {
            continue;
        }
        picked.push(start);
        if picked.len() == MOST {
            break;
        }
    }
    picked
}

/// Whether two searches aimed at the same kind of item: what it reads; for a region or a problem, which (a problem's
/// band overlapping the other's); for a reference's style or effects, the same reference (`towards`).
fn same_kind(a: &Value, b: &Value) -> bool {
    let band = |target: &Value| Some((target.get("low")?.as_f64()?, target.get("high")?.as_f64()?));
    let overlapping = match (band(a), band(b)) {
        (Some((low, high)), Some((other_low, other_high))) => low <= other_high && other_low <= high,
        (None, None) => true,
        _ => false,
    };
    ["kind", "region", "problem", "towards"].iter().all(|key| a.get(key) == b.get(key)) && a.get("kind").is_some() && overlapping
}

/// How far a past search was from this one, in just-noticeable steps: where it aimed, and how the track sounded before
/// it (the items both measured).
fn distance(past: &Remembered, now: &Now) -> f64 {
    let aimed = match (past.target.get("aim").and_then(Value::as_f64), now.aim) {
        (Some(then), Some((aim, step))) if step > 0. => (then - aim) / step,
        _ => 0.,
    };
    let values = past.before.as_ref().and_then(|before| before.get("values"));
    let apart: Vec<f64> = now
        .sounds
        .iter()
        .filter(|(_, _, step)| *step > 0.)
        .filter_map(|(id, value, step)| Some((values?.get(id)?.as_f64()? - value) / step))
        .collect();
    let sounded = if apart.is_empty() { UNKNOWN } else { (apart.iter().map(|d| d * d).sum::<f64>() / apart.len() as f64).sqrt() };
    (aimed * aimed + sounded * sounded).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heard_settings::SearchKind;
    use serde_json::json;

    fn remembered(id: &str, at: i64, knobs: &[(&str, f64)], target: Value, bass: f64) -> Remembered {
        Remembered {
            id: id.into(),
            at,
            track: Some("Bass".into()),
            kind: SearchKind::Tune,
            knobs: knobs.iter().map(|(name, at)| (name.to_string(), *at)).collect(),
            target,
            before: Some(json!({"values": {"bass": bass, "loudness": -14.0}})),
            cost: 1.,
        }
    }

    fn region(region: u64, aim: f64) -> Value {
        json!({"kind": "region", "region": region, "item": "bass", "aim": aim})
    }

    #[test]
    fn the_nearest_searches_for_the_same_knobs_and_kind_of_target_are_heard_first() {
        let knobs = ["Gain".to_string(), "Freq".to_string()];
        let target = region(1, -7.5);
        let sounds = [("bass".to_string(), -11.0, 1.0), ("loudness".to_string(), -14.0, 0.5)];
        let now = Now { knobs: &knobs, start: &[0.5, 0.5], target: &target, aim: Some((-7.5, 1.0)), sounds: &sounds };
        let past = vec![
            remembered("far", 1, &[("Gain", 0.9), ("Freq", 0.1)], region(1, -3.5), -6.0),
            remembered("near", 2, &[("Gain", 0.7), ("Freq", 0.6), ("Q", 0.2)], region(1, -7.0), -11.5),
            remembered("other region", 3, &[("Gain", 0.2), ("Freq", 0.2)], region(5, -7.5), -11.0),
            remembered("other knobs", 4, &[("Gain", 0.3)], region(1, -7.5), -11.0),
            remembered("same as near", 5, &[("Gain", 0.71), ("Freq", 0.59)], region(1, -7.6), -11.2),
            remembered("where it is", 6, &[("Gain", 0.5), ("Freq", 0.51)], region(1, -7.5), -11.0),
            remembered("middle", 7, &[("Gain", 0.6), ("Freq", 0.3)], region(1, -6.0), -10.0),
            remembered("fourth", 8, &[("Gain", 0.1), ("Freq", 0.9)], region(1, -6.5), -10.5),
            remembered("fifth", 9, &[("Gain", 0.9), ("Freq", 0.9)], region(1, -6.6), -10.4),
        ];
        let picked = starts(&past, &now);
        let ids: Vec<&str> = picked.iter().map(|start| start.remembered.id.as_str()).collect();
        // "same as near" is nearer still, so "near" (its double) goes; another region, other knobs and where the search
        // already is don't count; three at most ("middle" is the fourth).
        assert_eq!(ids, ["same as near", "fifth", "fourth"], "{picked:?}");
        assert_eq!(picked[0].point, [0.71, 0.59], "in the search's own knob order");
        assert!(picked.windows(2).all(|pair| pair[0].distance <= pair[1].distance));
        assert!(starts(&[], &now).is_empty(), "no memory, nothing changes");
        assert!(starts(&past[..1], &now).is_empty(), "a search far from this one (it aimed 4 steps off, and sounded 5 off)");
    }

    #[test]
    fn a_problem_in_another_band_or_a_style_toward_another_reference_is_another_kind_of_target() {
        let harsh = |low: f64, high: f64| json!({"kind": "problem", "problem": "harshness", "low": low, "high": high, "item": "harshness"});
        assert!(same_kind(&harsh(2_800., 3_600.), &harsh(3_200., 4_000.)), "overlapping bands");
        assert!(!same_kind(&harsh(2_800., 3_600.), &harsh(7_000., 9_000.)), "another band");
        let vibe = |towards: &str| json!({"kind": "vibe", "item": "vibe", "towards": towards});
        assert!(same_kind(&vibe("0a1b2c3d4e5f6a7b"), &vibe("0a1b2c3d4e5f6a7b")));
        assert!(!same_kind(&vibe("0a1b2c3d4e5f6a7b"), &vibe("ffff2c3d4e5f6a7b")), "toward another reference");
        assert!(!same_kind(&vibe("0a1b2c3d4e5f6a7b"), &json!({"kind": "vibe", "item": "vibe"})), "one remembered before Kumi kept which");
        assert!(same_kind(&region(1, -7.), &region(1, -5.)) && !same_kind(&region(1, -7.), &region(2, -7.)));
    }
}
