//! A judged round's moves, apart from Live: the whole stretch predicted from an excerpt, the two takes a listener
//! compares brought to one loudness, a kept change rebalanced (each step heard, then the round judged again at the
//! loudness it ends at), and a round taken back (what Live wouldn't undo removed only when it's a device the round
//! itself made, known by Live's identity for it). Rendering does these in Live; a test does them on a pretend Set.

use super::{
    checklist::{Checklist, Item, Quantity, Verdict},
    listener::{Choice, Opinion},
};
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
    /// Whether a change can change what's heard (a rename, a colour, a locator, the transport or an empty track
    /// can't): a round takes back only what it could have heard.
    fn audible(&self, id: &str) -> bool {
        let _ = id;
        true
    }
    /// What changed Live outside HISTORY since the run's last round (Python run in Live, a command, Live's own undo),
    /// in words: none of it can be taken back.
    fn outside(&self) -> Option<String> {
        None
    }
    /// Kumi's undo of one change; why not, when Live wouldn't.
    async fn undo(&self, id: &str) -> Result<(), String>;
    /// Live's identities for the devices these changes made (loaded or duplicated), as HISTORY recorded them.
    fn made(&self, changes: &[(String, String)]) -> Vec<String>;
    /// The run's chain as it is now.
    async fn chain(&self) -> Result<Vec<Placed>, String>;
    /// Whether Esc (or Live going away) has stopped the round.
    fn stopped(&self) -> bool;
    /// Whether a limiter after the gain rebalancing turns holds the peaks (then they don't move with it).
    async fn limited(&self) -> bool;
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

/// What a heard change comes to: the verdict (with what was taken back, when it wasn't kept), what rebalancing did,
/// the whole stretch and the excerpt as things stand in Live when it was kept, the listens it took, and whether Esc
/// stopped it.
#[derive(Debug, Clone)]
pub struct Decided {
    pub verdict: Verdict,
    pub rebalanced: Option<String>,
    pub whole: Vec<Option<f64>>,
    pub excerpt: Listen,
    pub listens: u32,
    pub stopped: bool,
    /// Not kept, and something of it stayed in Live (it wouldn't take it back): the run's readings no longer describe
    /// the Set.
    pub stayed: bool,
}

/// A round's decision once its change is heard (`after`, on the excerpt whose "before" read `before`):
/// - the whole stretch predicted from what the excerpt moved;
/// - peaks and clipping judged at the loudness rebalancing will bring back;
/// - the listening model's veto when it hears an artifact both ways (`opinion`);
/// - the cost of the devices the change made (`made`);
/// - rebalancing, and the round judged again as it ends up.
///
/// Not kept, Kumi's changes since `checkpoint` are taken back, and the verdict says what went.
#[allow(clippy::too_many_arguments)]
pub async fn decide(
    host: &dyn RoundHost,
    checklist: &Checklist,
    target: Option<usize>,
    whole: &[Option<f64>],
    before: &[Option<f64>],
    after: Listen,
    opinion: Option<&Opinion>,
    made: usize,
    checkpoint: &[String],
) -> Decided {
    let predicted = predict(checklist, whole, before, &after.values);
    let gain = checklist.rebalance_round(whole, &predicted, target);
    // Rebalancing will bring loudness back by `gain`: peaks move with it, unless a limiter after the gain holds them.
    let level = match gain {
        Some(gain) if !host.limited().await => gain,
        _ => 0.,
    };
    let mut verdict = checklist.verdict(target, whole, &checklist.at_level(whole, &predicted, level));
    if let Some(opinion) = opinion {
        let artifacts: Vec<&str> = opinion
            .new_problems
            .iter()
            .map(String::as_str)
            .filter(|problem| ["distorted", "pumping", "clipped"].contains(problem))
            .collect();
        if verdict.kept && opinion.closer == Some(Choice::Before) && !artifacts.is_empty() {
            verdict.kept = false;
            verdict.why = format!("the listening model heard it {} both ways", artifacts.join(" and "));
        }
    }
    let target_id = target.map(|index| checklist.items[index].id.clone());
    if let (true, Some(why)) = (verdict.kept, processing(&verdict, target_id.as_deref(), made)) {
        verdict.kept = false;
        verdict.why = why;
    }
    let mut decided = Decided { verdict, rebalanced: None, whole: predicted, excerpt: after, listens: 0, stopped: false, stayed: false };
    if let (true, Some(gain)) = (decided.verdict.kept, gain) {
        let settled = rebalance(host, checklist, target, whole, decided.whole, decided.excerpt, gain, decided.verdict).await;
        decided = Decided {
            verdict: settled.verdict,
            rebalanced: settled.rebalanced,
            whole: settled.whole,
            excerpt: settled.excerpt,
            listens: settled.listens,
            stopped: settled.stopped,
            stayed: false,
        };
        if let (true, Some(why)) = (decided.verdict.kept, processing(&decided.verdict, target_id.as_deref(), made)) {
            decided.verdict.kept = false;
            decided.verdict.why = why;
        }
    }
    if !decided.verdict.kept {
        // Kumi's undo takes the round's changes back (a rebalance's too), newest first.
        let taken = take_back(host, checkpoint).await;
        decided.verdict.why.push_str(&format!("; {}", taken.said));
        decided.stayed = taken.stayed;
    }
    decided
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
/// rebalancing moved (peaks, clipping) counts; with nothing heard, as the change left it. A round whose loudness
/// couldn't be matched by a step or more, whose step moved it under a fifth of what it asked while it's still off (a
/// knob at its end, a maximizer's Threshold near 0 dB), or whose unheard step Live wouldn't take back, isn't kept: it
/// can't be judged fairly.
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
    // Why the round can't be judged fairly, when it can't.
    let mut unfair = None;
    let off = if gain < 0. { "louder" } else { "quieter" };
    while let Some(gain) = step {
        let mark = host.applied();
        match host.turn(gain).await {
            Ok(stage) => label = stage,
            Err(why) => {
                settled.stopped = host.stopped();
                settled.rebalanced = Some(format!("loudness is {} dB off; {why}", to_string(-gain)));
                if settled.listens == 0 {
                    unfair = Some(format!("its loudness couldn't be matched (it's {} dB {off}: {why})", to_string(round1(gain.abs()))));
                }
                break;
            }
        }
        total += gain;
        steps += 1;
        let again = match host.hear().await {
            Ok(again) => again,
            Err(unheard) => {
                let mut stuck = vec![];
                for (id, title) in host.applied_since(&mark).iter().rev() {
                    if let Err(why) = host.undo(id).await {
                        stuck.push(format!("{title} ({why})"));
                    }
                }
                settled.stopped = unheard.stopped;
                if stuck.is_empty() {
                    total -= gain;
                    steps -= 1;
                    settled.rebalanced = Some(format!("couldn't hear the rebalance ({}), so its last step was taken back", unheard.why));
                    if settled.listens == 0 {
                        unfair = Some(format!(
                            "its loudness couldn't be matched (it's {} dB {off}: Kumi couldn't hear the rebalance)",
                            to_string(round1(gain.abs()))
                        ));
                    }
                } else {
                    let why =
                        format!("Kumi couldn't hear its rebalance ({}) and Live wouldn't take it back: {}", unheard.why, stuck.join(", "));
                    settled.rebalanced = Some(why.clone());
                    unfair = Some(why);
                }
                break;
            }
        };
        settled.listens += 1;
        let was = settled.excerpt.loudness;
        settled.whole = predict(checklist, &settled.whole, &settled.excerpt.values, &again.values);
        settled.excerpt = again;
        let still = checklist.rebalance_round(whole, &settled.whole, target);
        // What share of the step loudness moved by.
        let share = match (was, settled.excerpt.loudness) {
            (Some(was), Some(now)) if gain.abs() >= 0.1 => Some((now - was) / gain),
            _ => None,
        };
        if let (Some(share), Some(more)) = (share.filter(|share| *share < 0.2), still) {
            let moved = format!("{label} {} dB moved loudness {} dB", to_string(round1(gain)), to_string(round1(share * gain)));
            settled.rebalanced = Some(format!("{moved}; it's still {} dB off", to_string(round1(more.abs()))));
            unfair = Some(format!(
                "its loudness couldn't be matched (it's {} dB {}: {moved})",
                to_string(round1(more.abs())),
                if more < 0. { "louder" } else { "quieter" }
            ));
            break;
        }
        let slope = share.map_or(1., |share| share.clamp(0.2, 1.));
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
    settled.verdict = match (unfair, again.kept, settled.listens) {
        (Some(why), _, _) => Verdict { kept: false, why, ..again },
        (None, false, 0) => Verdict { why: format!("{} (its loudness couldn't be matched)", again.why), ..again },
        (None, false, _) => Verdict { why: format!("{} once loudness was matched", again.why), ..again },
        (None, true, _) => again,
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

/// A name after renames (old → new, oldest first): a run finds its track and focus by name, so it follows them.
pub fn renamed(name: &str, renames: &[(String, String)]) -> String {
    renames.iter().fold(name.to_owned(), |name, (from, to)| if *from == name { to.clone() } else { name })
}

/// What a take-back did: its words for the round's why, and whether any of it stayed in Live.
#[derive(Debug, Clone, PartialEq)]
pub struct TakenBack {
    pub said: String,
    pub stayed: bool,
}

/// Takes a round back: Kumi's changes since `checkpoint` that can change what's heard undone, newest first (a rename,
/// a colour, a locator or the transport isn't the round's to take back). What Live wouldn't undo is said; a device
/// such a change made goes instead when it's on the run's chain, known by Live's identity for it (recorded when it was
/// made), so a device the producer added is never one of them. Anything else Live wouldn't undo stays. What changed
/// the sound outside HISTORY (Python run in Live, a command, Live's own undo, a plug-in's own window) stays too, and is
/// said when Kumi knows of it: the run hears its bars again before the next change.
pub async fn take_back(host: &dyn RoundHost, checkpoint: &[String]) -> TakenBack {
    let all: Vec<(String, String)> = host.applied_since(checkpoint).into_iter().filter(|(id, _)| host.audible(id)).collect();
    let outside = host.outside();
    let mut taken = match (&outside, all.is_empty()) {
        (Some(_), true) => TakenBack { said: "nothing in HISTORY to take back".into(), stayed: true },
        _ => take_back_these(host, all).await,
    };
    if let Some(what) = outside {
        taken.said.push_str(&format!("; whatever {what} changed isn't in HISTORY, so it stays (change it back yourself if it should go)"));
        taken.stayed = true;
    }
    taken
}

/// Takes these changes back (ids and titles, oldest first), newest first, as `take_back` does.
pub async fn take_back_these(host: &dyn RoundHost, all: Vec<(String, String)>) -> TakenBack {
    let stayed = |said: String| TakenBack { said, stayed: true };
    if all.is_empty() {
        return stayed("nothing in HISTORY to take back (change it back yourself if you changed it another way)".into());
    }
    let mut stuck = vec![];
    let mut said = vec![];
    for (id, title) in all.iter().rev() {
        if let Err(why) = host.undo(id).await {
            stuck.push((id.clone(), title.clone()));
            said.push(format!("{title} ({why})"));
        }
    }
    if stuck.is_empty() {
        return TakenBack {
            said: format!("taken back: {}", all.iter().map(|(_, title)| title.as_str()).collect::<Vec<_>>().join(", ")),
            stayed: false,
        };
    }
    let titles = said.join(", ");
    let made = host.made(&stuck);
    if made.is_empty() {
        return stayed(format!("Live wouldn't take back {titles}: undo it yourself"));
    }
    let found = match host.chain().await {
        Ok(now) => removable(&now, &made),
        Err(why) => return stayed(format!("Live wouldn't take back {titles} ({why}): undo it yourself")),
    };
    let mut removed = vec![];
    for device in found.iter().rev() {
        if host.delete(&device.reference).await {
            removed.push(device.name.clone());
        }
    }
    // All of it went only when each stuck change made a device and each of those was removed.
    let left = made.len() < stuck.len() || removed.len() < made.len();
    if removed.is_empty() {
        stayed(format!("Live wouldn't take back {titles}: undo it yourself"))
    } else {
        TakenBack { said: format!("Live wouldn't undo {titles}, so Kumi removed {} instead", removed.join(", ")), stayed: left }
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
    // A reading lost is silence, never on target. As the verdict reads them, the learned models' guards are only
    // unheard (copies aren't embedded), and a sound's or an effect's own measure can go unread while the rest of the
    // copy is heard (a longer tail filling the gaps a decay was read in).
    let heard = checklist.items.iter().zip(&predicted).any(|(item, value)| {
        value.is_some()
            && matches!(
                item.quantity,
                Quantity::Integrated | Quantity::TruePeak | Quantity::Plr | Quantity::Psr | Quantity::Crest | Quantity::Distortion
            )
    });
    let excused =
        |item: &Item| matches!(item.quantity, Quantity::Vibe { .. } | Quantity::EffectStyle { .. }) || (heard && item.quantity.of_sound());
    if predicted.iter().zip(whole).zip(&checklist.items).any(|((now, was), item)| was.is_some() && now.is_none() && !excused(item)) {
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

/// The devices on the chain `now` that a round made (`made`, Live's identities for them); a device Live doesn't say the
/// identity of is never one.
pub fn removable(now: &[Placed], made: &[String]) -> Vec<Placed> {
    now.iter().filter(|device| !device.identity.is_empty() && made.contains(&device.identity)).cloned().collect()
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
