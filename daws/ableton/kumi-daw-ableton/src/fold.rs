//! Live's words for a folded track list; the folding itself is core's (core/fold.rs).
use crate::core::contracts::JsonObject;
pub use crate::core::fold::{track_line, FoldedTracks, OBSERVATION_TRACK_BYTES};

pub const FOLDED_NOTE:&str="A big Set: only the tracks in focus (selected in Live, pinned, or changed lately) list their devices; each other track is one line: \"ref name (type, in its group) · its first devices +how many more\" (or \"· N devices\"). For another track's devices, discover kind device with parent that track's ref.";

/// The tracks folded to fit, in Live's words.
pub fn fold_tracks(tracks: &[JsonObject], in_focus: impl Fn(&JsonObject) -> bool, budget: Option<usize>) -> FoldedTracks {
    crate::core::fold::fold_tracks_with(tracks, in_focus, budget, FOLDED_NOTE, |left| {
        format!("{left} more tracks aren't listed; discover kind track (with cursor) for them")
    })
}
