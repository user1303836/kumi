//! A judged round's moves, apart from Live: the whole stretch predicted from an excerpt, the two takes a listener
//! compares brought to one loudness, a kept change rebalanced (each step heard, then the round judged again at the
//! loudness it ends at), and a round taken back (what Live wouldn't undo removed only when it's exactly what the round
//! made). Rendering does these in Live; a test does them on a pretend chain.

use super::checklist::{Checklist, Verdict};
use async_trait::async_trait;
use kumi_common::js::number::to_string;
use std::path::PathBuf;

/// One listen to the round's excerpt: what each checklist item reads there, its loudness, and the capture.
#[derive(Debug, Clone, PartialEq)]
pub struct Listen {
    pub values: Vec<Option<f64>>,
    pub loudness: Option<f64>,
    pub file: PathBuf,
    pub start: f64,
}

/// A listen that didn't come through: why, and whether Esc (or Live going away) stopped it.
#[derive(Debug, Clone, PartialEq)]
pub struct Unheard {
    pub why: String,
    pub stopped: bool,
}

/// A device on the run's chain: Live's identity for it, its name and its ref.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub identity: String,
    pub name: String,
    pub reference: String,
}

/// What a round has done in Live.
#[async_trait(?Send)]
pub trait RoundHost {
    /// Turns the level by `gain` dB where rebalancing goes; says what it turned.
    async fn turn(&self, gain: f64) -> Result<String, String>;
    /// Hears the round's excerpt as things stand.
    async fn hear(&self) -> Result<Listen, Unheard>;
    /// Kumi's changes in HISTORY now.
    fn applied(&self) -> Vec<String>;
    /// Those not in `mark`, oldest first: ids and titles.
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)>;
    /// Kumi's undo of one change; why not, when Live wouldn't.
    async fn undo(&self, id: &str) -> Result<(), String>;
    /// How many devices these changes made (loaded or duplicated).
    fn created_by(&self, changes: &[(String, String)]) -> usize;
    /// The run's chain as it is now.
    async fn chain(&self) -> Result<Vec<Placed>, String>;
    /// Deletes a device; whether it went.
    async fn delete(&self, reference: &str) -> bool;
}

/// The whole stretch as it would read now: what the excerpt moved (`before` to `after`), moved there too. An item the
/// excerpt doesn't read (the part doesn't play in those bars) keeps its value; one it read before and can't now is
/// lost (None).
pub fn predict(checklist: &Checklist, whole: &[Option<f64>], before: &[Option<f64>], after: &[Option<f64>]) -> Vec<Option<f64>> {
    whole
        .iter()
        .zip(before)
        .zip(after)
        .zip(&checklist.items)
        .map(|(((whole, before), after), item)| match (whole, before, after) {
            (Some(whole), Some(before), Some(after)) => Some(item.quantity.moved(*whole, *before, *after)),
            (_, Some(_), None) => None,
            (Some(whole), None, _) => Some(*whole),
            (None, _, after) => *after,
        })
        .collect()
}

/// The gains (dB) that bring two takes to one loudness by turning the louder one down: raising the quieter one could
/// clip it, which a listener would hear as the change's fault.
pub fn matched(before: Option<f64>, after: Option<f64>) -> (f64, f64) {
    let louder = match (before, after) {
        (Some(before), Some(after)) => after - before,
        _ => 0.,
    };
    (louder.min(0.), -louder.max(0.))
}

/// A kept change, rebalanced: what the round's verdict ends as, what rebalancing did, the whole stretch and the
/// excerpt as things stand, the listens it took, and whether it was stopped.
#[derive(Debug, Clone)]
pub struct Settled {
    pub verdict: Verdict,
    pub rebalanced: Option<String>,
    pub whole: Vec<Option<f64>>,
    pub excerpt: Listen,
    pub listens: u32,
    pub stopped: bool,
}

/// Brings loudness back after a kept change: `gain` first, each step heard and the next sized by what the last one did
/// (a limiter after the gain holds peaks and eats some of it), at most three. A step nobody heard is taken back (Live
/// stays as it was last heard). Then the round is judged again as it ends up in Live: at the loudness it ends at, what
/// rebalancing moved (peaks, clipping) counts; with nothing heard, as the change left it.
#[allow(clippy::too_many_arguments)]
pub async fn rebalance(
    host: &dyn RoundHost,
    checklist: &Checklist,
    target: Option<usize>,
    whole: &[Option<f64>],
    predicted: Vec<Option<f64>>,
    excerpt: Listen,
    gain: f64,
    verdict: Verdict,
) -> Settled {
    let mut settled = Settled { verdict, rebalanced: None, whole: predicted, excerpt, listens: 0, stopped: false };
    let (mut step, mut total, mut steps, mut label) = (Some(gain), 0., 0, String::new());
    while let Some(gain) = step {
        let mark = host.applied();
        match host.turn(gain).await {
            Ok(stage) => label = stage,
            Err(why) => {
                settled.rebalanced = Some(format!("loudness is {} dB off; {why}", to_string(-gain)));
                break;
            }
        }
        total += gain;
        steps += 1;
        let again = match host.hear().await {
            Ok(again) => again,
            Err(unheard) => {
                for (id, _) in host.applied_since(&mark).iter().rev() {
                    let _ = host.undo(id).await;
                }
                total -= gain;
                steps -= 1;
                settled.stopped = unheard.stopped;
                settled.rebalanced = Some(format!("couldn't hear the rebalance ({}), so its last step was taken back", unheard.why));
                break;
            }
        };
        settled.listens += 1;
        let was = settled.excerpt.loudness;
        settled.whole = predict(checklist, &settled.whole, &settled.excerpt.values, &again.values);
        settled.excerpt = again;
        let still = checklist.rebalance(whole, &settled.whole);
        let slope = match (was, settled.excerpt.loudness) {
            (Some(was), Some(now)) if gain.abs() >= 0.1 => ((now - was) / gain).clamp(0.2, 1.),
            _ => 1.,
        };
        step = still.map(|more| round1((more / slope).clamp(-12., 12.))).filter(|next| next.abs() >= 0.2 && steps < 3);
    }
    if settled.rebalanced.is_none() && steps > 0 {
        settled.rebalanced = Some(format!(
            "{label} {}{} dB{}",
            if total > 0. { "+" } else { "" },
            to_string(round1(total)),
            if steps > 1 { format!(" (homed in over {steps} listens)") } else { String::new() }
        ));
    }
    let again = checklist.verdict(target, whole, &settled.whole);
    settled.verdict = match (again.kept, settled.listens) {
        (false, 0) => Verdict { why: format!("{} (its loudness couldn't be matched)", again.why), ..again },
        (false, _) => Verdict { why: format!("{} once loudness was matched", again.why), ..again },
        (true, _) => again,
    };
    settled
}

/// Processing has a cost: each device a change made has to earn half a step on its target (`target`, the item's id).
/// Why it didn't, when it didn't.
pub fn processing(verdict: &Verdict, target: Option<&str>, made: usize) -> Option<String> {
    let earned = target.and_then(|id| verdict.rows.iter().find(|row| row.id == id)).map_or(0., |row| row.gap_before - row.gap_after);
    (made > 0 && earned < 0.5 * made as f64).then(|| {
        format!(
            "it closed {} of a step for {made} more device{}, less than the processing costs",
            to_string(round1(earned)),
            if made == 1 { "" } else { "s" }
        )
    })
}

/// Takes a round back: Kumi's changes since `checkpoint` undone, newest first. What Live wouldn't undo is said; a
/// device such a change made goes instead, but only when the chain shows exactly the new devices (against `known`,
/// the chain at the checkpoint) those changes made: one the producer added by hand would make them more, and then
/// nothing is removed. The words that end the round's why.
pub async fn take_back(host: &dyn RoundHost, checkpoint: &[String], known: Option<&[Placed]>) -> String {
    let all = host.applied_since(checkpoint);
    if all.is_empty() {
        return "nothing in HISTORY to take back (change it back yourself if you changed it another way)".into();
    }
    let mut stuck = vec![];
    for (id, title) in all.iter().rev() {
        if let Err(why) = host.undo(id).await {
            stuck.push((id.clone(), format!("{title} ({why})")));
        }
    }
    if stuck.is_empty() {
        return format!("taken back: {}", all.iter().map(|(_, title)| title.as_str()).collect::<Vec<_>>().join(", "));
    }
    let titles = stuck.iter().map(|(_, title)| title.as_str()).collect::<Vec<_>>().join(", ");
    let made = host.created_by(&stuck);
    if made == 0 {
        return format!("Live wouldn't take back {titles}: undo it yourself");
    }
    let found = match known {
        None => Err("Kumi couldn't read the chain before the change".to_string()),
        Some(known) => host.chain().await.and_then(|now| removable(known, &now, made)),
    };
    match found {
        Err(why) => format!("Live wouldn't take back {titles} ({why}): undo it yourself"),
        Ok(new) => {
            let mut removed = vec![];
            for device in new.iter().rev() {
                if host.delete(&device.reference).await {
                    removed.push(device.name.clone());
                }
            }
            if removed.is_empty() {
                format!("Live wouldn't take back {titles}: undo it yourself")
            } else {
                format!("Live wouldn't undo {titles}, so Kumi removed {} instead", removed.join(", "))
            }
        }
    }
}

/// What a searched candidate costs: the target's gap in steps (as the whole would read), anything it makes audibly
/// worse (as the judge would count it: each costs ten steps and how far past its tolerance it went), and how far the
/// knobs moved from `start` (processing that has to earn its place). One whose target, or anything else, can't be
/// read costs everything: silence isn't on target.
pub fn candidate_cost(
    checklist: &Checklist,
    target: usize,
    before: &[Option<f64>],
    whole: &[Option<f64>],
    after: &[Option<f64>],
    point: &[f64],
    start: &[f64],
) -> f64 {
    let predicted = predict(checklist, whole, before, after);
    let Some(reached) = predicted[target] else { return f64::INFINITY };
    if predicted.iter().zip(whole).any(|(now, was)| was.is_some() && now.is_none()) {
        return f64::INFINITY;
    }
    let verdict = checklist.verdict(Some(target), whole, &predicted);
    let gap = checklist.items[target].gap(Some(reached));
    let worse: f64 =
        verdict.rows.iter().filter(|row| verdict.hurt.contains(&row.id)).map(|row| 10. + (row.gap_after - row.gap_before).max(0.)).sum();
    let distance: f64 = point.iter().zip(start).map(|(a, b)| (a - b).abs()).sum();
    let cost = gap + worse + distance * 0.5;
    if cost.is_finite() {
        cost
    } else {
        f64::INFINITY
    }
}

/// The devices on the chain `now` that weren't on it at the checkpoint (`known`), when they're exactly the `made` a
/// round's changes made; else why not.
pub fn removable(known: &[Placed], now: &[Placed], made: usize) -> Result<Vec<Placed>, String> {
    if now.iter().any(|device| device.identity.is_empty()) {
        return Err("Live didn't say which devices are which".into());
    }
    let new: Vec<Placed> = now.iter().filter(|device| !known.iter().any(|was| was.identity == device.identity)).cloned().collect();
    if new.len() != made {
        return Err(format!("the chain has {} new devices and the change made {made}", new.len()));
    }
    Ok(new)
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
