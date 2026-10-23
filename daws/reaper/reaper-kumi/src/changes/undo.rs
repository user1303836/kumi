//! Noticing that the producer undid or redid a change of Kumi's in REAPER. REAPER tells an extension nothing of its own undo
//! (the control surface's callbacks fire for every undo and say nothing of what changed), so the history is looked at: Kumi's
//! points carry a nonce in their description, and a point that moves from the undone side of the history to the applied one,
//! or back, is the producer's Edit > Undo or Redo.
//!
//! The history is a list of points with the project at one of them. The points after it can be redone. When that list gains
//! entries at its front, the producer undid them; when it loses entries from its front, they were redone (the points the
//! project moved over are checked, since a new edit by the producer drops the whole list the same way).

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::json;

use crate::extension::Outbox;
use crate::reaper::{Project, Reaper};

/// The redoable points looked at, at least: the history can hold hundreds, and what the producer undid is at its front.
const REDO_SCAN: i32 = 256;
/// The points looked at behind the one the project is at, at least: what the producer can have undone in the time of a look.
const TAIL_SCAN: i32 = 128;
/// The most points one look reads on either side, however far the producer jumped in the Undo History at once.
const MOST_SCAN: i32 = 4096;
/// How many of Kumi's points are remembered with the side of the history they were last seen on.
const KNOWN_KEPT: usize = 4096;

/// Whether a point's description is one of Kumi's: `Kumi: …` ending in a nonce, `k` and four base-36 characters.
pub fn is_kumi(desc: &str) -> bool {
    let Some(rest) = desc.strip_suffix(')') else { return false };
    let Some(open) = rest.rfind("(k") else { return false };
    let nonce = &rest[open + 2..];
    desc.starts_with("Kumi: ") && nonce.len() == 4 && nonce.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// The history as last looked at.
#[derive(Clone, Debug, PartialEq, Eq)]
struct View {
    entries: i32,
    at: i32,
    count: i32,
    /// The descriptions of the points up to `at` (the last `TAIL_SCAN` of them), oldest first.
    tail: Vec<String>,
    /// The descriptions of the points after `at`, nearest first.
    redo: Vec<String>,
}

/// A look at the history: `tail` points behind the one the project is at and `redo` after it, each at most `MOST_SCAN`.
fn view(reaper: &Reaper, proj: Project, tail: i32, redo: i32) -> Option<View> {
    let (entries, at) = reaper.undo_position(proj)?;
    let count = reaper.state_change_count(proj).unwrap_or(0);
    let first = (at + 1 - tail.clamp(TAIL_SCAN, MOST_SCAN)).max(0);
    let tail = (first..=at).map(|index| reaper.undo_entry(proj, index).unwrap_or_default()).collect();
    let last = (entries - 1).min(at + redo.clamp(REDO_SCAN, MOST_SCAN));
    let redo = (at + 1..=last).map(|index| reaper.undo_entry(proj, index).unwrap_or_default()).collect();
    Some(View { entries, at, count, tail, redo })
}

/// Which side of the history one of Kumi's points was last seen on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Applied,
    Redoable,
}

/// Kumi's points, each with the side it was last seen on, so that a point the producer has since buried deeper than a look
/// reads (300 fader moves on top of it) is still known for what it was when a jump brings it into sight (NIT-4). Each
/// description is unique (its nonce), so one that vanished from the history never comes back to be mistaken.
#[derive(Default)]
struct Known {
    sides: HashMap<String, Side>,
    order: VecDeque<String>,
}

impl Known {
    fn side(&self, desc: &str) -> Option<Side> {
        self.sides.get(desc).copied()
    }

    fn set(&mut self, desc: &str, side: Side) {
        if self.sides.insert(desc.to_string(), side).is_none() {
            self.order.push_back(desc.to_string());
            while self.order.len() > KNOWN_KEPT {
                if let Some(oldest) = self.order.pop_front() {
                    self.sides.remove(&oldest);
                }
            }
        }
    }

    /// What a look saw of Kumi's points.
    fn learn(&mut self, view: &View) {
        for (list, side) in [(&view.tail, Side::Applied), (&view.redo, Side::Redoable)] {
            for desc in list.iter().filter(|desc| is_kumi(desc)) {
                self.set(desc, side);
            }
        }
    }
}

#[derive(Default)]
pub struct Watch {
    seen: Option<View>,
    epoch: String,
    nudged: bool,
    known: Known,
}

impl Watch {
    /// Kumi just made a point of its own (or closed a plan's): it is the new baseline, not the producer's undo.
    pub fn resync(&mut self, reaper: &Reaper, proj: Project) {
        self.seen = view(reaper, proj, TAIL_SCAN, REDO_SCAN);
        if let Some(seen) = &self.seen {
            self.known.learn(seen);
        }
    }

    /// REAPER ran an action: the history is looked at on this tick even if nothing else says it moved.
    pub fn nudge(&mut self) {
        self.nudged = true;
    }

    /// Looks at the history; the producer's undo or redo of Kumi's points is an `undone_in_daw` event. True when an undo
    /// or redo (of anyone's point) moved the project since the last look: what it brought back may need sweeping.
    pub fn poll(&mut self, reaper: &Reaper, proj: Project, epoch: &str, out: &dyn Outbox) -> bool {
        let moved = self.epoch == epoch
            && match (reaper.undo_position(proj), &self.seen) {
                (Some((entries, at)), Some(seen)) => seen.entries == entries && seen.at != at,
                _ => false,
            };
        self.look(reaper, proj, epoch, out);
        moved
    }

    fn look(&mut self, reaper: &Reaper, proj: Project, epoch: &str, out: &dyn Outbox) {
        if self.epoch != epoch {
            // Another project in the tab: its history is not this one's to compare with.
            self.epoch = epoch.to_string();
            self.seen = None;
        }
        let nudged = std::mem::take(&mut self.nudged);
        let Some((entries, at)) = reaper.undo_position(proj) else { return };
        let count = reaper.state_change_count(proj).unwrap_or(0);
        if let Some(seen) = &mut self.seen {
            if !nudged && seen.entries == entries && seen.at == at {
                if seen.count == count {
                    return;
                }
                // Bare edits, unless the point the project is at is another one: an undo and a new edit between two looks
                // leave the same number of points with the project at the same place.
                if reaper.undo_entry(proj, at).as_ref() == seen.tail.last() {
                    seen.count = count;
                    return;
                }
            }
        }
        // As far as the project moved since the last look, in either direction: a jump of 300 points in the Undo History
        // is read whole. New points since (the same number of entries no longer) leave only what was undone after the last
        // of them on the redo side, which is read whole too.
        let (tail, redo) = match &self.seen {
            Some(seen) if seen.entries == entries => (at - seen.at + 1, seen.at - at + 1),
            Some(seen) => (at - seen.at + 1, entries - at),
            None => (TAIL_SCAN, REDO_SCAN),
        };
        let Some(now) = view(reaper, proj, tail, redo) else { return };
        if let Some(before) = self.seen.take() {
            for (how, descs) in changes(&before, &now, &self.known) {
                out.event("undone_in_daw", json!({"descs": descs, "how": how, "epoch": epoch}));
            }
        }
        self.known.learn(&now);
        self.seen = Some(now);
    }
}

/// What the producer did to Kumi's points between two looks: `("undo" | "redo", their descriptions)`.
///
/// Only Kumi's own points are looked at, and each is told by its description, which its nonce makes unique (REAPER gives the
/// producer's points like descriptions, "Adjust track volume" for every fader move, so no place is told by theirs). A point
/// of Kumi's that was applied and is redoable now was undone; one that was redoable and is applied now was redone. One that
/// vanished from both lists is evidence of nothing: REAPER merges its oldest points away when its undo memory is full, and
/// a look sees only so far. Its row keeps its state, and Kumi's undo checks the values as ever. (So an undo of Kumi's point
/// and a new edit within one look leave the row at Applied: a known limit, safe because the undo checks.)
///
/// What a point was is what the last look saw of it, or, for one buried deeper than that look read, the side it was last
/// seen on (`known`): a jump that brings it into sight on the other side is told however deep it was.
fn changes(before: &View, now: &View, known: &Known) -> Vec<(&'static str, Vec<String>)> {
    let kumi = |list: &[String]| list.iter().filter(|desc| is_kumi(desc)).cloned().collect::<HashSet<String>>();
    let (applied_before, redoable_before) = (kumi(&before.tail), kumi(&before.redo));
    let was = |desc: &String, side: Side| {
        let seen = if side == Side::Applied { &applied_before } else { &redoable_before };
        seen.contains(desc) || (!applied_before.contains(desc) && !redoable_before.contains(desc) && known.side(desc) == Some(side))
    };
    let undone: Vec<String> = now.redo.iter().filter(|desc| is_kumi(desc) && was(desc, Side::Applied)).cloned().collect();
    let redone: Vec<String> = now.tail.iter().filter(|desc| is_kumi(desc) && was(desc, Side::Redoable)).cloned().collect();
    let mut found = Vec::new();
    if !undone.is_empty() {
        found.push(("undo", undone));
    }
    if !redone.is_empty() {
        found.push(("redo", redone));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(tail: &[&str], redo: &[&str]) -> View {
        let tail: Vec<String> = tail.iter().map(|d| d.to_string()).collect();
        View { entries: 0, at: tail.len() as i32 - 1, count: 0, tail, redo: redo.iter().map(|d| d.to_string()).collect() }
    }

    /// A history of `n` producer points named by their number, with Kumi's point `depth` points under the newest.
    fn history(n: usize, kumi_depth: usize) -> Vec<String> {
        (0..n).map(|k| if k == n - 1 - kumi_depth { "Kumi: Vocal volume −8 dB (kd3db)".to_string() } else { format!("Edit {k}") }).collect()
    }

    fn told(before: &View, now: &View) -> Vec<(&'static str, Vec<String>)> {
        changes(before, now, &Known::default())
    }

    // (r-tracks, NIT-4)
    #[test]
    fn a_jump_back_past_what_a_look_reads_is_told_from_what_kumis_point_was_last_seen_as() {
        let kumi = "Kumi: Vocal volume −8 dB (kd3db)".to_string();
        // Kumi's change, then 300 fader moves on top: a look reads the last 128, so it is out of sight, known as applied.
        let all: Vec<String> = std::iter::once(kumi.clone()).chain((0..300).map(|k| format!("Adjust track volume {k}"))).collect();
        let before = look(&all[all.len() - 128..].iter().map(String::as_str).collect::<Vec<_>>(), &[]);
        let mut known = Known::default();
        known.set(&kumi, Side::Applied);
        // The producer jumps back 301 points at once: Kumi's point is the nearest one to redo.
        let now = look(&["Initial"], &all.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(changes(&before, &now, &known), [("undo", vec![kumi.clone()])]);
        // Without what it was last seen as, the jump tells nothing (the looks alone never saw it applied).
        assert_eq!(told(&before, &now), []);
        // Undone, then 300 earlier points undone too: Kumi's is far down the redo side, out of sight of the next look; the
        // producer then jumps forward past it, and it is applied 10 points down.
        known.set(&kumi, Side::Redoable);
        let earlier: Vec<String> = (0..300).map(|k| format!("Adjust track pan {k}")).collect();
        let back = look(&["Initial"], &earlier[..256].iter().map(String::as_str).collect::<Vec<_>>());
        let mut applied: Vec<&str> = vec!["Initial"];
        applied.extend(earlier.iter().map(String::as_str));
        applied.extend(all[..11].iter().map(String::as_str));
        let forward = look(&applied[applied.len() - 128..], &[]);
        assert_eq!(changes(&back, &forward, &known), [("redo", vec![kumi])]);
        assert_eq!(told(&back, &forward), []);
    }

    #[test]
    fn reaper_dropping_its_oldest_points_is_no_undo() {
        // 745 points, Kumi's 60 down; the producer's next edit makes REAPER drop the 246 oldest at once.
        let full = history(745, 60);
        let before = look(&full[745 - 128..].iter().map(String::as_str).collect::<Vec<_>>(), &[]);
        let mut trimmed: Vec<String> = full[246..].to_vec();
        trimmed[0] = "Multiple changes".into();
        trimmed.push("Adjust track pan".into());
        let now = look(&trimmed[trimmed.len() - 128..].iter().map(String::as_str).collect::<Vec<_>>(), &[]);
        assert_eq!(told(&before, &now), []);
        // And when the producer undid Kumi's point too, before the edit that trimmed: that is told.
        let mut undone: Vec<String> = full[246..745 - 61].to_vec();
        undone.push("Adjust track pan".into());
        let now = look(&undone[undone.len() - 128..].iter().map(String::as_str).collect::<Vec<_>>(), &[]);
        // (undone and replaced: it vanished, which tells nothing)
        assert_eq!(told(&before, &now), []);
    }

    #[test]
    fn like_descriptions_dont_hide_an_undo() {
        // Kumi's point among the producer's like edits; the producer undoes nine points (Kumi's among them) in one look.
        let mut before: Vec<&str> = vec!["Adjust track volume"; 10];
        before.push("Kumi: Vocal volume −8 dB (kd3db)");
        before.extend(["Adjust track volume"; 8]);
        let mut redo: Vec<&str> = vec!["Adjust track volume"; 8];
        redo.push("Kumi: Vocal volume −8 dB (kd3db)");
        let now = vec!["Adjust track volume"; 10];
        assert_eq!(told(&look(&before, &[]), &look(&now, &redo)), [("undo", vec!["Kumi: Vocal volume −8 dB (kd3db)".to_string()])]);
        // Undone and replaced by new edits within one look: it vanished, which tells nothing (a known limit).
        let now = vec!["Adjust track volume"; 19];
        assert_eq!(told(&look(&before, &[]), &look(&now, &[])), []);
    }

    /// The last `n` points of a history of like fader moves with Kumi's change `depth` under the newest, and what can be
    /// redone after it.
    fn fader_history(n: usize, depth: usize, redo: usize) -> (Vec<String>, Vec<String>) {
        let all: Vec<String> = (0..n)
            .map(|k| if k == n - 1 - depth { "Kumi: Vocal volume −8 dB (kd3db)".into() } else { "Adjust track volume".into() })
            .collect();
        let tail = all[n.saturating_sub(128)..].to_vec();
        (tail, vec!["Adjust track volume".to_string(); redo])
    }

    fn view_of((tail, redo): &(Vec<String>, Vec<String>)) -> View {
        look(&tail.iter().map(String::as_str).collect::<Vec<_>>(), &redo.iter().map(String::as_str).collect::<Vec<_>>())
    }

    #[test]
    fn a_jump_back_over_kumis_point_among_like_fader_moves_is_an_undo_and_a_jump_forward_a_redo() {
        let kumi = vec!["Kumi: Vocal volume −8 dB (kd3db)".to_string()];
        // 300 like points, Kumi's 10 down; the producer jumps back 21 points in the Undo History.
        let before = view_of(&fader_history(300, 10, 0));
        let (tail, _) = fader_history(300, 10, 0);
        let mut redo: Vec<String> = tail[tail.len() - 21..].iter().rev().cloned().collect();
        redo.truncate(21);
        let back = look(
            &tail[..tail.len() - 21].iter().map(String::as_str).collect::<Vec<_>>(),
            &redo.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(told(&before, &back), [("undo", kumi.clone())]);
        // And forward again over it, many points at once.
        assert_eq!(told(&back, &before), [("redo", kumi)]);
    }

    #[test]
    fn many_points_made_between_two_looks_are_no_undo_of_kumis_point() {
        // A script makes 120 points: Kumi's change is still applied, 130 down, past what a look sees.
        let before = view_of(&fader_history(300, 10, 0));
        let after = view_of(&fader_history(420, 130, 0));
        assert_eq!(told(&before, &after), []);
        // 100 points: it is still in sight, and applied.
        let after = view_of(&fader_history(400, 110, 0));
        assert_eq!(told(&before, &after), []);
    }

    #[test]
    fn an_undo_a_redo_and_new_edits_are_told_as_before() {
        let before = look(&["Init", "A", "Kumi: x (k0001)"], &["Kumi: y (k0002)"]);
        // Redone.
        assert_eq!(
            told(&before, &look(&["Init", "A", "Kumi: x (k0001)", "Kumi: y (k0002)"], &[])),
            [("redo", vec!["Kumi: y (k0002)".to_string()])]
        );
        // Undone.
        assert_eq!(
            told(&before, &look(&["Init", "A"], &["Kumi: x (k0001)", "Kumi: y (k0002)"])),
            [("undo", vec!["Kumi: x (k0001)".to_string()])]
        );
        // A new edit of the producer's.
        assert_eq!(told(&before, &look(&["Init", "A", "Kumi: x (k0001)", "B"], &[])), []);
    }

    #[test]
    fn kumis_points_are_told_by_the_nonce_at_the_end() {
        assert!(is_kumi("Kumi: Bass volume −6 dB (k7f3a)"));
        assert!(is_kumi("Kumi: undo Deleted track Pad (k0000)"));
        for other in [
            "Kumi: Bass volume (k7f3)",
            "Bass volume (k7f3a)",
            "Kumi: x (K7f3a)",
            "Kumi: x (k7f3a) ",
            "Kumi: x (k7F3a)",
            "Adjust track volume",
            "",
        ] {
            assert!(!is_kumi(other), "{other:?}");
        }
    }
}
