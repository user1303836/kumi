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

/// Willington's bindings, as their switch stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WillingtonSwitch {
    Off,
    On,
    /// Turned on moments ago: the bridge may still be loading them, so their tools may not be offered yet.
    JustOn,
}

/// The instructions for Willington: `switch` is how its bindings stand, None when Kumi's bridge doesn't
/// carry them; `offered` whether the model has a tool by a name. Nothing while Live isn't connected.
pub fn willington_instructions(switch: Option<WillingtonSwitch>, offered: impl Fn(&str) -> bool) -> &'static str {
    match switch {
        Some(_) if !offered(CHANGES_TOOL) => "",
        None => "",
        Some(WillingtonSwitch::Off) => OFF,
        Some(_) if offered(MAPPING_TOOL) => ON,
        // Not "none fit" yet: once the bridge has loaded them, their tools arrive and the session is made again.
        Some(WillingtonSwitch::JustOn) => "",
        Some(WillingtonSwitch::On) => UNSUPPORTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_has_its_own_words_and_none_without_live_or_willington() {
        use WillingtonSwitch::*;
        let connected = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        assert_eq!(willington_instructions(None, connected(&[CHANGES_TOOL, MAPPING_TOOL])), "");
        for switch in [Off, On, JustOn] {
            assert_eq!(willington_instructions(Some(switch), connected(&[])), "");
        }
        assert_eq!(willington_instructions(Some(Off), connected(&[CHANGES_TOOL])), OFF);
        for switch in [On, JustOn] {
            assert_eq!(willington_instructions(Some(switch), connected(&[CHANGES_TOOL, MAPPING_TOOL])), ON);
        }
        assert_eq!(willington_instructions(Some(On), connected(&[CHANGES_TOOL])), UNSUPPORTED);
        // Turned on a moment ago, the bridge may not have loaded them yet: nothing said, rather than "none fit".
        assert_eq!(willington_instructions(Some(JustOn), connected(&[CHANGES_TOOL])), "");
        assert!(OFF.contains("/willington") && !ON.contains("/willington") && !UNSUPPORTED.contains("/willington"));
    }
}
