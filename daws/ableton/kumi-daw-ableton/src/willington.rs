//! What the model is told about Willington: the native bindings Kumi's bridge can carry, off until the
//! producer turns them on with /willington. With them on, the macro mapping the instructions otherwise
//! rule out is a tool (assets/changes.json), and so is mapping Live's modulators (map_modulator), with
//! the map_modulation that Willington's DeviceTools gives their devices.

/// Offered while Kumi is connected to Live with changes it can make.
const CHANGES_TOOL: &str = "make_changes";
/// Willington's macro mapping, macro and variation names and chain zones.
const MAPPING_TOOL: &str = "edit_rack_mapping";
/// Python in Live, which map_modulator runs through: Willington's DeviceTools give a modulator's device
/// map_modulation there.
const PYTHON_TOOL: &str = "run_python";

const OFF: &str = "Willington's bindings are off. Turned on, they let Kumi map a rack's parameters to its macros with their ranges, name macros and variations, and set rack chain zones (edit_rack_mapping), on the Live versions Willington supports; once Willington's Follow Action self-test has passed, they also set Session clips' Follow Actions (set_clip_follow_actions). When a request needs one of those, say in a sentence that /willington turns the bindings on (for Follow Actions, after the self-test), then carry on as Live allows without them.";
const OFF_MODULATORS: &str = "Willington's bindings are off. Turned on, they let Kumi map a rack's parameters to its macros with their ranges, name macros and variations, set rack chain zones (edit_rack_mapping), and map Live's LFO, Shaper, Envelope Follower and Expression Control modulators to parameters (map_modulator), on the Live versions Willington supports; once Willington's Follow Action self-test has passed, they also set Session clips' Follow Actions (set_clip_follow_actions). When a request needs one of those, say in a sentence that /willington turns the bindings on (for Follow Actions, after the self-test), then carry on as Live allows without them.";
const ON: &str = "Willington's bindings are on, so the macro mapping Live otherwise doesn't allow is here: edit_rack_mapping maps a parameter in a rack to one of its macros with the mapping's range, and names macros and variations. Map macros with it, playback stopped, rather than asking the producer to; modulators still can't be mapped.";
const ON_MODULATORS: &str = "Willington's bindings are on, so the mappings Live otherwise doesn't allow are here, whatever the Racks instructions say. edit_rack_mapping maps a parameter in a rack to one of its macros with the mapping's range, and names macros and variations: map macros with it, playback stopped, rather than asking the producer to. Modulators (LFO, Shaper, Envelope Follower, Expression Control) map with map_modulator: when what you build has mapped modulators, such as a tutorial's LFOs on a rack's parameters, load and map them in the same plan, not with automation or by asking the producer.";
const UNSUPPORTED: &str = "Willington's bindings are on, but none fit the Live that's open: they're made for exact Live versions. Its mappings aren't here, so go on as Live allows without them.";

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
/// Modulators map only through run_python, so only where it's offered is the model told they do.
pub fn willington_instructions(switch: Option<WillingtonSwitch>, offered: impl Fn(&str) -> bool) -> &'static str {
    let python = offered(PYTHON_TOOL);
    match switch {
        Some(_) if !offered(CHANGES_TOOL) => "",
        None => "",
        Some(WillingtonSwitch::Off) if python => OFF_MODULATORS,
        Some(WillingtonSwitch::Off) => OFF,
        Some(_) if offered(MAPPING_TOOL) && python => ON_MODULATORS,
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
        assert_eq!(willington_instructions(Some(Off), connected(&[CHANGES_TOOL, PYTHON_TOOL])), OFF_MODULATORS);
        for switch in [On, JustOn] {
            assert_eq!(willington_instructions(Some(switch), connected(&[CHANGES_TOOL, MAPPING_TOOL])), ON);
            assert_eq!(willington_instructions(Some(switch), connected(&[CHANGES_TOOL, MAPPING_TOOL, PYTHON_TOOL])), ON_MODULATORS);
        }
        for tools in [&[CHANGES_TOOL][..], &[CHANGES_TOOL, PYTHON_TOOL]] {
            assert_eq!(willington_instructions(Some(On), connected(tools)), UNSUPPORTED);
        }
        // Turned on a moment ago, the bridge may not have loaded them yet: nothing said, rather than "none fit".
        assert_eq!(willington_instructions(Some(JustOn), connected(&[CHANGES_TOOL])), "");
        assert!(OFF.contains("/willington") && !ON.contains("/willington") && !UNSUPPORTED.contains("/willington"));
        // /willington turns Follow Actions on only with a passing self-test: off, the model is told so.
        assert!(OFF.contains("set_clip_follow_actions") && OFF.contains("self-test"));
        // Modulators map through run_python: where it's offered, the model is told how, and that the Racks
        // rule against mapping doesn't hold; without it, that they can't be mapped, and /willington doesn't
        // promise them.
        assert!(OFF_MODULATORS.contains("modulators to parameters (map_modulator)") && !OFF.contains("modulator"));
        assert!(OFF_MODULATORS.contains("set_clip_follow_actions") && OFF_MODULATORS.contains("/willington"));
        assert!(ON_MODULATORS.contains("map with map_modulator") && ON_MODULATORS.contains("in the same plan"));
        assert!(ON_MODULATORS.contains("whatever the Racks instructions say") && !ON_MODULATORS.contains("can't be mapped"));
        assert!(ON.contains("modulators still can't be mapped") && !ON.contains("run_python"));
    }
}
