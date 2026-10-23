//! Quantize and humanize (M4): transforms of a take's notes, made and undone as a notes change is. The notes they move are
//! fenced by the take's whole note list, `applied` holds the notes taken out and put in (by value), and the undo is the
//! notes undo, which finds them by value and touches no other note.
//!
//! Quantize snaps to a grid in quarter notes from the project's start (so bars line up whatever the item's place), with a
//! strength and a swing: every second grid line moves later, 100% swing putting it on the triplet. Humanize moves notes
//! by up to a number of milliseconds and changes velocities by up to an amount, from a seed, so a retry or a preview and
//! its apply move the same notes the same way. A note pushed into another of its pitch and channel ends where that one
//! starts (as REAPER's own "fix overlaps" does); two that would start together are refused.

use serde_json::{json, Value};

use super::notes::{self, Note};
use super::{Applied, Ctx, Done, Preview};
use crate::read::{bad, failed, take_start_qn};
use crate::reaper::Take;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    Quantize,
    Humanize,
}

/// What a transform moves: the take's notes, the places of those it touches, and what they become.
struct Plan {
    take: Take,
    item_qn: f64,
    before: Vec<Note>,
    events: Vec<crate::midi::Event>,
    remove: Vec<usize>,
    add: Vec<Note>,
    /// How many notes were shortened to end where the next of their pitch starts.
    shortened: usize,
    seed: Option<u64>,
}

fn percent(input: &Value, key: &str, default: f64) -> Done<f64> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value.as_f64().filter(|n| (0.0..=100.0).contains(n)).ok_or_else(|| bad(format!("{key} must be from 0 to 100"))),
    }
}

/// The notes a request names: `indexes` (from 0, as read), else every note.
fn chosen(input: &Value, count: usize) -> Done<Vec<usize>> {
    match input.get("indexes").filter(|v| !v.is_null()) {
        None => Ok((0..count).collect()),
        Some(list) => {
            let list = list
                .as_array()
                .filter(|l| !l.is_empty() && l.len() <= 10_000)
                .ok_or_else(|| bad("indexes must hold from 1 to 10000 entries"))?;
            let mut out = Vec::new();
            for value in list {
                let at = value.as_u64().map(|n| n as usize).filter(|n| *n < count).ok_or_else(|| {
                    bad(format!("indexes must be positions of notes in that take (it has {count}, from 0): read it again"))
                })?;
                if out.contains(&at) {
                    return Err(bad("a note is named twice"));
                }
                out.push(at);
            }
            Ok(out)
        }
    }
}

/// A grid line's place in quarter notes: every second line later by the swing (100% = two thirds of the way to the next).
fn grid_line(index: i64, grid: f64, swing: f64) -> f64 {
    let base = index as f64 * grid;
    if index.rem_euclid(2) == 1 {
        base + swing / 100.0 * grid / 3.0
    } else {
        base
    }
}

/// The grid line nearest to a place in quarter notes.
fn nearest_line(qn: f64, grid: f64, swing: f64) -> f64 {
    let around = (qn / grid).floor() as i64;
    (around - 1..=around + 2).map(|k| grid_line(k, grid, swing)).min_by(|a, b| (a - qn).abs().total_cmp(&(b - qn).abs())).unwrap_or(qn)
}

/// The first grid line after a place in quarter notes.
fn line_after(qn: f64, grid: f64, swing: f64) -> f64 {
    let mut k = (qn / grid).floor() as i64 - 1;
    while grid_line(k, grid, swing) <= qn + 1e-9 {
        k += 1;
    }
    grid_line(k, grid, swing)
}

/// A seeded generator (splitmix64): the same seed moves the same notes the same way.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from -1 to 1.
    fn spread(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

fn qn_of(ctx: &Ctx, take: Take, ppq: f64) -> Done<f64> {
    ctx.reaper.ppq_to_quarter_notes(take, ppq).ok_or_else(|| failed("this REAPER can't turn ticks into beats"))
}

fn ppq_of(ctx: &Ctx, take: Take, qn: f64) -> Done<f64> {
    ctx.reaper.quarter_notes_to_ppq(take, qn).map(f64::round).ok_or_else(|| failed("this REAPER can't turn beats into ticks"))
}

/// Notes moved into others of their pitch and channel end where the next starts; two starting together are refused.
fn fix_overlaps(before: &[Note], remove: &mut Vec<usize>, add: &mut Vec<Note>) -> Done<usize> {
    let mut all: Vec<(Note, Option<usize>)> =
        before.iter().enumerate().filter(|(i, _)| !remove.contains(i)).map(|(i, n)| (*n, Some(i))).collect();
    all.extend(add.iter().map(|n| (*n, None)));
    all.sort_by(|a, b| (a.0.channel, a.0.pitch).cmp(&(b.0.channel, b.0.pitch)).then(a.0.start.total_cmp(&b.0.start)));
    let mut shortened = 0;
    for at in 1..all.len() {
        let (next, prev) = (all[at].0, all[at - 1].0);
        if next.channel != prev.channel || next.pitch != prev.pitch || prev.end <= next.start {
            continue;
        }
        if prev.start >= next.start {
            return Err(bad(format!(
                "two notes of pitch {} on channel {} would start together: transform fewer notes, or use a finer grid",
                next.pitch,
                next.channel + 1
            )));
        }
        // The earlier note ends where the later starts; one Kumi wasn't asked to move becomes part of the change.
        let mut fixed = prev;
        fixed.end = next.start;
        match all[at - 1].1 {
            Some(index) => {
                remove.push(index);
                add.push(fixed);
            }
            None => {
                if let Some(slot) = add
                    .iter_mut()
                    .find(|n| n.channel == prev.channel && n.pitch == prev.pitch && n.start == prev.start && n.end == prev.end)
                {
                    *slot = fixed;
                }
            }
        }
        all[at - 1].0 = fixed;
        shortened += 1;
    }
    Ok(shortened)
}

fn plan(ctx: &Ctx, input: &Value, which: Which, fence_seed: Option<u64>) -> Done<Plan> {
    let take = notes::take_of(ctx, &input["ref"])?;
    let item_qn = take_start_qn(ctx.reaper, ctx.proj, take);
    let (before, events) = notes::read_all(ctx, take)?;
    if before.is_empty() {
        return Err(bad("that take has no notes"));
    }
    let picked = chosen(input, before.len())?;
    let mut add = Vec::new();
    let mut seed = None;
    match which {
        Which::Quantize => {
            let grid = input
                .get("grid")
                .and_then(Value::as_f64)
                .filter(|g| *g > 0.0 && *g <= 16.0)
                .ok_or_else(|| bad("grid must be quarter notes, above 0 and up to 16 (0.25 is a sixteenth)"))?;
            let strength = percent(input, "strength", 100.0)?;
            let swing = percent(input, "swing", 0.0)?;
            let ends = input.get("ends").and_then(Value::as_str).unwrap_or("start");
            if !matches!(ends, "start" | "end" | "both") {
                return Err(bad("ends must be start (the notes move, keeping their lengths), end (their ends snap) or both"));
            }
            for &at in &picked {
                let note = before[at];
                let mut moved = note;
                let (start_qn, end_qn) = (qn_of(ctx, take, note.start)?, qn_of(ctx, take, note.end)?);
                if ends != "end" {
                    let target = start_qn + (nearest_line(start_qn, grid, swing) - start_qn) * strength / 100.0;
                    moved.start = ppq_of(ctx, take, target)?;
                    if ends == "start" {
                        moved.end = moved.start + (note.end - note.start);
                    }
                }
                if ends != "start" {
                    let target = end_qn + (nearest_line(end_qn, grid, swing) - end_qn) * strength / 100.0;
                    moved.end = ppq_of(ctx, take, target)?;
                }
                if moved.end <= moved.start {
                    // An end snapped to or before its start goes to the first grid line after it.
                    moved.end = ppq_of(ctx, take, line_after(qn_of(ctx, take, moved.start)?, grid, swing))?;
                }
                add.push(moved);
            }
        }
        Which::Humanize => {
            let timing = match input.get("timing").filter(|v| !v.is_null()) {
                None => 0.0,
                Some(value) => value
                    .as_f64()
                    .filter(|ms| (0.0..=200.0).contains(ms))
                    .ok_or_else(|| bad("timing must be milliseconds from 0 to 200"))?,
            };
            let velocity = match input.get("velocity").filter(|v| !v.is_null()) {
                None => 0,
                Some(value) => {
                    value.as_i64().filter(|n| (0..=64).contains(n)).ok_or_else(|| bad("velocity must be a whole number from 0 to 64"))?
                }
            };
            if timing == 0.0 && velocity == 0 {
                return Err(bad("pass timing (milliseconds), velocity (an amount) or both"));
            }
            let given = input.get("seed").and_then(Value::as_u64);
            let chosen_seed = given.or(fence_seed).unwrap_or_else(|| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64 & 0x1F_FFFF_FFFF_FFFF)
            });
            seed = Some(chosen_seed);
            let mut dice = Dice(chosen_seed);
            for &at in &picked {
                let note = before[at];
                let mut moved = note;
                let (shift, change) = (dice.spread() * timing / 1000.0, (dice.spread() * velocity as f64).round() as i32);
                if timing > 0.0 {
                    let time = ctx
                        .reaper
                        .time_at_quarter_notes(ctx.proj, qn_of(ctx, take, note.start)?)
                        .ok_or_else(|| failed("this REAPER can't turn beats into time"))?;
                    let qn = ctx
                        .reaper
                        .quarter_notes_at(ctx.proj, (time + shift).max(0.0))
                        .ok_or_else(|| failed("this REAPER can't turn time into beats"))?;
                    moved.start = ppq_of(ctx, take, qn)?.max(0.0);
                    moved.end = moved.start + (note.end - note.start);
                }
                moved.velocity = (note.velocity + change).clamp(1, 127);
                add.push(moved);
            }
        }
    }
    let mut remove = picked;
    let shortened = fix_overlaps(&before, &mut remove, &mut add)?;
    remove.sort_unstable();
    remove.dedup();
    notes::check_inside(ctx, take, item_qn, &events, &add)?;
    Ok(Plan { take, item_qn, before, events, remove, add, shortened, seed })
}

fn title(ctx: &Ctx, plan: &Plan, which: Which, input: &Value) -> String {
    let label = notes::take_label(ctx, plan.take);
    let count = plan.remove.len();
    let notes = format!("{count} note{}", if count == 1 { "" } else { "s" });
    match which {
        Which::Quantize => {
            let grid = input.get("grid").and_then(Value::as_f64).unwrap_or(0.25);
            let strength = input.get("strength").and_then(Value::as_f64).unwrap_or(100.0);
            let swing = input.get("swing").and_then(Value::as_f64).unwrap_or(0.0);
            let mut words = format!("Quantized {notes} in {label} to {} of a beat", trimmed(grid));
            if strength < 100.0 {
                words += &format!(", {strength:.0}%");
            }
            if swing > 0.0 {
                words += &format!(", swing {swing:.0}%");
            }
            words
        }
        Which::Humanize => format!("Humanized {notes} in {label}"),
    }
}

fn trimmed(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn note_word(shortened: usize) -> Option<String> {
    (shortened > 0).then(|| format!("{shortened} note(s) end where the next of their pitch starts, as they'd overlap otherwise."))
}

pub fn preview(ctx: &Ctx, input: &Value, which: Which) -> Done<Preview> {
    let plan = plan(ctx, input, which, None)?;
    let mut fence = notes::fence_of(&plan.before);
    if let Some(seed) = plan.seed {
        fence["seed"] = seed.into();
    }
    let gone: Vec<(usize, Note)> = plan.remove.iter().map(|&i| (i, plan.before[i])).collect();
    let mut display = json!({
        "title": title(ctx, &plan, which, input),
        "count": plan.remove.len(),
        "before": notes::rows_of(ctx, plan.take, plan.item_qn, gone.iter().copied()),
        "after": notes::rows_of(ctx, plan.take, plan.item_qn, plan.add.iter().copied().enumerate()),
    });
    if let Some(note) = note_word(plan.shortened) {
        display["note"] = note.into();
    }
    if let Some(seed) = plan.seed {
        display["seed"] = seed.into();
    }
    let prior = json!({"notes": plan.before.len(), "fence": notes::fence_of(&plan.before), "removed": notes::rows_of(ctx, plan.take, plan.item_qn, gone.into_iter())});
    Ok(Preview { target: notes::target(ctx, plan.take), prior, fence, display })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value, which: Which) -> Done<Applied> {
    let plan = plan(ctx, input, which, fence.get("seed").and_then(Value::as_u64))?;
    let mut now = notes::fence_of(&plan.before);
    if let Some(seed) = plan.seed {
        now["seed"] = seed.into();
    }
    if now != *fence {
        return Err(("changed".into(), "the notes in that take changed since the preview".into()));
    }
    let take = plan.take;
    let removed: Vec<Note> = plan.remove.iter().map(|&i| plan.before[i]).collect();
    notes::write_notes(ctx, take, &plan.events, &removed, &plan.add)?;
    let (after, _) = notes::read_all(ctx, take)?;
    if after.len() + plan.remove.len() != plan.before.len() + plan.add.len() {
        return Err(notes::take_back(
            ctx,
            take,
            &plan.events,
            &notes::fence_of(&plan.before),
            "the take doesn't hold the notes it should after the change",
        ));
    }
    let (gone_now, added_now) = (notes::difference(&plan.before, &after), notes::difference(&after, &plan.before));
    let mut display = json!({"title": title(ctx, &plan, which, input), "notes": after.len()});
    if let Some(note) = note_word(plan.shortened) {
        display["note"] = note.into();
    }
    if let Some(seed) = plan.seed {
        display["seed"] = seed.into();
    }
    Ok(Applied {
        applied: json!({"added": added_now.iter().map(Note::json).collect::<Vec<_>>(), "removed": gone_now.iter().map(Note::json).collect::<Vec<_>>(), "notes": after.len()}),
        display,
        unwritten: gone_now.is_empty() && added_now.is_empty(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_swings_every_second_line_up_to_the_triplet() {
        assert_eq!(nearest_line(0.27, 0.25, 0.0), 0.25);
        assert_eq!(nearest_line(0.6, 0.5, 100.0), 0.5 + 0.5 / 3.0);
        assert!((nearest_line(0.6, 0.5, 100.0) - 2.0 / 3.0).abs() < 1e-12, "a swung eighth at 100% is the triplet");
        assert_eq!(nearest_line(0.95, 0.5, 100.0), 1.0, "even lines don't move");
        assert_eq!(nearest_line(-0.1, 0.25, 0.0), 0.0);
        assert_eq!(line_after(1.0, 0.5, 0.0), 1.5);
        assert_eq!(line_after(1.04, 0.5, 100.0), 1.5 + 0.5 / 3.0);
    }

    #[test]
    fn the_same_seed_rolls_the_same_dice() {
        let (mut a, mut b) = (Dice(42), Dice(42));
        let rolls: Vec<f64> = (0..100).map(|_| a.spread()).collect();
        assert_eq!(rolls, (0..100).map(|_| b.spread()).collect::<Vec<_>>());
        assert!(rolls.iter().all(|r| (-1.0..=1.0).contains(r)));
        assert_ne!(rolls[0], Dice(43).spread());
    }
}
