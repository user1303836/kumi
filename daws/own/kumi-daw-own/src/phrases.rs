//! The fixed phrases ("tempo 124", "mute drums", core/phrases.rs) in a DAW of Kumi's own: each DAW says which of its
//! change kinds make a phrase in the project as the look just found it, or why the phrase doesn't apply there, in its
//! words. The integration runs the steps as one plan (`own_bridge`), so they land in HISTORY with Kumi's undo.
use crate::core::phrases::{phrase_track, Intent, PhraseTrack, TrackPick};
use serde_json::Value;

/// What a phrase is made from: the project as the look (and, for a big project, reads after it) found it.
pub struct PhraseLook<'a> {
    /// Every track, in the project's order, with this turn's short refs.
    pub tracks: &'a [PhraseTrack<'a>],
    /// The project's row and the transport, as the look read them.
    pub project: &'a Value,
    pub transport: &'a Value,
    /// The sequence's slots in order, each its first beat and length in beats: only for a DAW that loops by whole slots,
    /// and only for a loop.
    pub slots: Option<&'a [(f64, f64)]>,
}

/// What a phrase comes to in a DAW.
#[derive(Debug, PartialEq)]
pub enum Phrased {
    /// The plan's steps, in order.
    Steps(Vec<Value>),
    /// The phrase doesn't apply in this DAW: why, in its words. Said at once, with no model call.
    Refused(String),
    /// It can't run as asked here (no such track, two that fit, a tempo map): the model takes the request as written.
    Model,
}

/// How a DAW makes the fixed phrases.
pub struct OwnPhrases {
    pub plan: fn(&Intent, &PhraseLook) -> Phrased,
    /// The DAW loops whole slots of its sequence: a loop by bars needs their lengths (`PhraseLook::slots`).
    pub loops_by_slot: bool,
}

/// The track a phrase names, by its ref: None when none or several fit.
pub fn track(pick: &TrackPick, look: &PhraseLook) -> Option<String> {
    phrase_track(pick, look.tracks)
}

/// Bars counted from 1 (both included) as beats from the start: where the first starts and where the last ends.
pub fn bar_span(from_bar: u32, to_bar: u32, beats_per_bar: f64) -> (f64, f64) {
    let bar = if beats_per_bar.is_finite() && beats_per_bar > 0.0 { beats_per_bar } else { 4.0 };
    (f64::from(from_bar - 1) * bar, f64::from(to_bar) * bar)
}

/// The slots (from 1) that start exactly at `start` and end exactly at `end`, in beats; None when the span doesn't fall
/// on slots' edges.
pub fn slots_spanning(slots: &[(f64, f64)], start: f64, end: f64) -> Option<(usize, usize)> {
    const EDGE: f64 = 1e-6;
    let first = slots.iter().position(|(at, _)| (at - start).abs() < EDGE)?;
    let last = slots.iter().position(|(at, length)| (at + length - end).abs() < EDGE)?;
    (last >= first).then_some((first + 1, last + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_beats_from_the_start_and_a_span_is_whole_slots_or_none() {
        assert_eq!(bar_span(9, 16, 4.0), (32.0, 64.0));
        assert_eq!(bar_span(1, 1, 3.0), (0.0, 3.0));
        assert_eq!(bar_span(2, 2, 0.0), (4.0, 8.0), "no bar length: four beats");
        // Slots of 16, 16, 8 and 16 beats.
        let slots = [(0.0, 16.0), (16.0, 16.0), (32.0, 8.0), (40.0, 16.0)];
        assert_eq!(slots_spanning(&slots, 16.0, 40.0), Some((2, 3)));
        assert_eq!(slots_spanning(&slots, 0.0, 16.0), Some((1, 1)));
        assert_eq!(slots_spanning(&slots, 0.0, 56.0), Some((1, 4)));
        assert_eq!(slots_spanning(&slots, 8.0, 16.0), None, "starts inside a slot");
        assert_eq!(slots_spanning(&slots, 16.0, 36.0), None, "ends inside one");
        assert_eq!(slots_spanning(&slots, 56.0, 64.0), None, "past the sequence");
    }
}
