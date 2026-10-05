//! What the model is told about Willington: the native bindings Kumi's bridge can carry, off until the
//! producer turns them on with /willington. With them on, the macro mapping the instructions otherwise
//! rule out is a tool (assets/changes.json).

/// Offered while Kumi is connected to Live with changes it can make.
const CHANGES_TOOL: &str = "make_changes";
/// Willington's macro mapping, macro and variation names and chain zones.
const MAPPING_TOOL: &str = "edit_rack_mapping";

const OFF: &str = "Willington's bindings are off. Turned on, they let Kumi map a rack's parameters to its macros with their ranges, name macros and variations, set rack chain zones (edit_rack_mapping) and set Session clips' Follow Actions (set_clip_follow_actions), on the Live versions Willington supports. When a request needs one of those, say in a sentence that /willington turns the bindings on, then carry on as Live allows without them.";
const ON: &str = "Willington's bindings are on, so the macro mapping Live otherwise doesn't allow is here: edit_rack_mapping maps a parameter in a rack to one of its macros with the mapping's range, and names macros and variations. Map macros with it, playback stopped, rather than asking the producer to; modulators still can't be mapped.";
const UNSUPPORTED: &str = "Willington's bindings are on, but none fit the Live that's open: they're made for exact Live versions. Its macro mapping isn't here, so go on as Live allows without it.";

/// The instructions for Willington: `on` is whether its bindings are on, None when Kumi's bridge doesn't
/// carry them; `offered` whether the model has a tool by a name. Nothing while Live isn't connected.
pub fn willington_instructions(on: Option<bool>, offered: impl Fn(&str) -> bool) -> &'static str {
    match on {
        Some(_) if !offered(CHANGES_TOOL) => "",
        None => "",
        Some(false) => OFF,
        Some(true) if offered(MAPPING_TOOL) => ON,
        Some(true) => UNSUPPORTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_has_its_own_words_and_none_without_live_or_willington() {
        let connected = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        assert_eq!(willington_instructions(None, connected(&[CHANGES_TOOL, MAPPING_TOOL])), "");
        for on in [false, true] {
            assert_eq!(willington_instructions(Some(on), connected(&[])), "");
        }
        assert_eq!(willington_instructions(Some(false), connected(&[CHANGES_TOOL])), OFF);
        assert_eq!(willington_instructions(Some(true), connected(&[CHANGES_TOOL, MAPPING_TOOL])), ON);
        assert_eq!(willington_instructions(Some(true), connected(&[CHANGES_TOOL])), UNSUPPORTED);
        assert!(OFF.contains("/willington") && !ON.contains("/willington") && !UNSUPPORTED.contains("/willington"));
    }
}
