use serde_json::Value;

/// Keep replaying the frozen source for Kumi's words from before set_mixer took levels as Live shows them (volumeDb,
/// panDisplay, sendsDb): its description, (now, then).
const SINCE: (&str, &str) = (
    "Change one track's mixer. volume is the fader position from 0 to 1 (0.85 is 0 dB, 1 is +6 dB), or volumeDb the level \
     as Live shows it in dB (-6, 2.5, \"-inf\"), set exactly where Live shows that level; pan goes from -1 (left) to 1 \
     (right), or panDisplay as Live shows it (\"25L\", \"C\", \"50R\"); mute and solo are on/off; sends are levels from 0 \
     to 1 for return tracks A, B, … in order (a shorter list leaves the rest), or sendsDb in dB. trackRef comes from \
     discovery in this turn; return tracks and the main track work too.",
    "Change one track's mixer. volume is the fader position from 0 to 1 (0.85 is 0 dB, 1 is +6 dB); pan goes from -1 \
     (left) to 1 (right); mute and solo are on/off; sends are levels from 0 to 1 for return tracks A, B, … in order (a \
     shorter list leaves the rest). trackRef comes from discovery in this turn; return tracks and the main track work too.",
);

/// The change kinds as the source had them: set_mixer's description without the levels as Live shows them.
pub fn legacy_kinds(kinds: &Value) -> Value {
    let mut kinds = kinds.clone();
    for kind in kinds.as_array_mut().into_iter().flatten() {
        if kind["tool"] == "set_mixer" && kind["description"] == SINCE.0 {
            kind["description"] = SINCE.1.into();
        }
    }
    kinds
}
