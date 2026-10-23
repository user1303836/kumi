//! The channel strips the plug-in and send kinds work on, in the Mixer pane (opened for the change and closed after,
//! as the readers do). A strip is a track's, by its place in the Tracks area and its name as the readers match them,
//! or an output's, which no track has: `<epoch>:output:<n>`, the Mixer's output strips in their order (the Stereo Out
//! first), where Kumi puts the plug-ins that measure or limit the whole mix. A plug-in on a strip is
//! `<epoch>:plugin:<track>.<slot>` on a track's, `<epoch>:plugin:0.<output>.<slot>` on an output's; a send
//! `<epoch>:send:<track>.<slot>`; a parameter `<epoch>:param:<plug-in's numbers>.<n>`. Slots count from the top: a
//! plug-in's the strip's plug-ins with the instrument first (the `plugin` rows' count), a send's the strip's sends.
//!
//! Logic lists a strip's controls from the bottom up ([`paths::Strip`]); here the insert slots are kept from the top,
//! empty ones included, each occupied one with the insert bar Logic draws along its top edge (a line one point high,
//! listed just before its slot): a plug-in goes into the empty slot under the last, or through the bar above the
//! plug-in it should go before.

use serde_json::{json, Value};

use super::{changed, gone, nudge, quoted, tracks, Ctx, Done, Epoch};
use crate::ax::paths::{self, Strip};
use crate::ax::strings::Strings;
use crate::ax::Node;
use crate::refs::{bad, failed, Ref};

/// Whose strip: a track's (its place from 1) or an output's (its place among the Mixer's outputs, from 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Track(usize),
    Output(usize),
}

impl Owner {
    /// The strip's own ref: `<epoch>:track:<n>` or `<epoch>:output:<n>`.
    pub fn text(self, epoch: u32) -> String {
        match self {
            Owner::Track(place) => Ref::new(epoch, "track", &[place]).text(),
            Owner::Output(place) => Ref::new(epoch, "output", &[place]).text(),
        }
    }

    /// The numbers a ref under this strip starts with.
    fn numbers(self) -> Vec<usize> {
        match self {
            Owner::Track(place) => vec![place],
            Owner::Output(place) => vec![0, place],
        }
    }

    /// A plug-in's ref on this strip.
    pub fn plugin(self, epoch: u32, slot: usize) -> String {
        let mut numbers = self.numbers();
        numbers.push(slot);
        Ref::new(epoch, "plugin", &numbers).text()
    }

    /// A send's ref on this strip (outputs have none).
    pub fn send(self, epoch: u32, slot: usize) -> String {
        let mut numbers = self.numbers();
        numbers.push(slot);
        Ref::new(epoch, "send", &numbers).text()
    }

    /// A parameter's ref, under its plug-in on this strip.
    pub fn param(self, epoch: u32, slot: usize, index: usize) -> String {
        let mut numbers = self.numbers();
        numbers.push(slot);
        numbers.push(index);
        Ref::new(epoch, "param", &numbers).text()
    }
}

/// A ref's text and its parts, checked for its kind and epoch.
fn parsed(ctx: &Ctx, input: &Value, key: &str, kinds: &[&str], epoch: Epoch) -> Done<(String, Ref)> {
    let text = input.get(key).and_then(Value::as_str).ok_or_else(|| bad(format!("{key} must be a {} ref", kinds.join(" or "))))?;
    let reference = Ref::parse(text).filter(|r| kinds.contains(&r.kind.as_str())).ok_or_else(|| {
        let e = ctx.project.epoch;
        let example = match kinds[0] {
            "plugin" => format!("{e}:plugin:1.1, or {e}:plugin:0.1.1 on the Stereo Out"),
            "param" => format!("{e}:param:1.1.1"),
            "send" => format!("{e}:send:1.1"),
            _ => format!("{e}:track:1, or {e}:output:1 for the Stereo Out"),
        };
        bad(format!("{key} must be a {} ref, such as {example}, not {text}", kinds.join(" or ")))
    })?;
    if epoch == Epoch::Current && reference.epoch != ctx.project.epoch {
        return Err(crate::refs::stale("ref", reference.epoch, ctx.project.epoch));
    }
    Ok((text.to_string(), reference))
}

/// The strip a list of numbers starts with (`[track, …]` or `[0, output, …]`), and the numbers after it; a track's
/// place from an earlier epoch carried to where it is now.
fn split(ctx: &Ctx, text: &str, reference: &Ref, rest: usize) -> Done<(Owner, Vec<usize>)> {
    let numbers = &reference.numbers;
    let malformed = || bad(format!("{text} isn't a ref Logic Pro's bridge gave"));
    let (owner, tail) = match numbers.first() {
        Some(0) if numbers.len() == rest + 2 && numbers[1] > 0 => (Owner::Output(numbers[1]), &numbers[2..]),
        Some(place) if *place > 0 && numbers.len() == rest + 1 => (Owner::Track(*place), &numbers[1..]),
        _ => return Err(malformed()),
    };
    if tail.iter().any(|n| *n == 0) {
        return Err(malformed());
    }
    let owner = match owner {
        Owner::Track(place) if reference.epoch != ctx.project.epoch => {
            if reference.epoch > ctx.project.epoch {
                return Err(crate::refs::stale("ref", reference.epoch, ctx.project.epoch));
            }
            let now = ctx
                .project
                .carry_track(reference.epoch, place)
                .ok_or_else(|| gone(format!("the track of {text} isn't in the project any more (or that epoch is too far back)")))?;
            Owner::Track(now)
        }
        other => other,
    };
    Ok((owner, tail.to_vec()))
}

/// The strip a `track` or `output` ref names.
pub fn owner(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<Owner> {
    let (text, reference) = parsed(ctx, input, key, &["track", "output"], epoch)?;
    if reference.numbers.len() != 1 || reference.first() == 0 {
        return Err(bad(format!("{key} must be a track or output ref, not {text}")));
    }
    match reference.kind.as_str() {
        "output" => Ok(Owner::Output(reference.first())),
        _ => {
            let place = super::track_place(ctx, input, key, epoch)?;
            Ok(Owner::Track(place))
        }
    }
}

/// The strip and slot a `plugin` ref names.
pub fn plugin(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<(Owner, usize)> {
    let (text, reference) = parsed(ctx, input, key, &["plugin"], epoch)?;
    let (owner, tail) = split(ctx, &text, &reference, 1)?;
    Ok((owner, tail[0]))
}

/// The strip, plug-in slot and parameter a `param` ref names.
pub fn param(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<(Owner, usize, usize)> {
    let (text, reference) = parsed(ctx, input, key, &["param"], epoch)?;
    let (owner, tail) = split(ctx, &text, &reference, 2)?;
    Ok((owner, tail[0], tail[1]))
}

/// The strip and send slot a `send` ref names (a track's: outputs have no sends).
pub fn send(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<(usize, usize)> {
    let (text, reference) = parsed(ctx, input, key, &["send"], epoch)?;
    match split(ctx, &text, &reference, 1)? {
        (Owner::Track(place), tail) => Ok((place, tail[0])),
        (Owner::Output(_), _) => Err(bad("an output strip has no sends")),
    }
}

/// What a strip is called in a title: its track's name, or the output's.
pub fn label(ctx: &mut Ctx, owner: Owner, strip: Option<&Node>) -> Done<String> {
    if let Some(strip) = strip {
        let parts = Strip::of(strip, &ctx.project.strings);
        let name = parts.name.map(Node::value_text).unwrap_or_else(|| strip.desc.clone());
        if !name.is_empty() {
            return Ok(name);
        }
    }
    match owner {
        Owner::Track(place) => {
            let headers = nudge::read(tracks::headers(ctx))?;
            Ok(headers.get(place - 1).map(tracks::Found::label).unwrap_or_else(|| format!("Track {place}")))
        }
        Owner::Output(place) => Ok(format!("Output {place}")),
    }
}

/// The Mixer's strip of an owner, the Mixer being open: a track's matched to its header by name, in order (as the
/// readers match them), an output's by its place among the strips with a Bounce button.
pub fn strip_of(ctx: &mut Ctx, owner: Owner) -> Done<Node> {
    let strips = nudge::read(ctx.project.mixer_strip_nodes(ctx.ax))?;
    let strings = ctx.project.strings.clone();
    match owner {
        Owner::Output(place) => strips
            .iter()
            .filter(|strip| Strip::of(strip, &strings).bounce.is_some())
            .nth(place - 1)
            .cloned()
            .ok_or_else(|| crate::refs::not_in_project(&owner.text(ctx.project.epoch))),
        Owner::Track(place) => {
            let headers = nudge::read(tracks::headers(ctx))?;
            let header = headers.get(place - 1).ok_or_else(|| crate::refs::not_in_project(&owner.text(ctx.project.epoch)))?;
            let mut used = 0usize;
            let wanted = headers.iter().take(place).filter(|h| h.header.name == header.header.name).count();
            for strip in &strips {
                let parts = Strip::of(strip, &strings);
                if parts.bounce.is_some() {
                    continue;
                }
                let name = parts.name.map(Node::value_text).unwrap_or_else(|| strip.desc.clone());
                if name == header.header.name {
                    used += 1;
                    if used == wanted {
                        return Ok(strip.clone());
                    }
                }
            }
            Err(failed(format!("Logic Pro's Mixer shows no strip for track {place} ({})", quoted(&header.header.name))))
        }
    }
}

/// Opens the Mixer, finds the owner's strip and runs `with` on it, then closes the Mixer if it was closed before (and
/// forgets what the readers kept of the strips).
pub fn with_strip<T>(ctx: &mut Ctx, owner: Owner, with: impl FnOnce(&mut Ctx, &Node) -> Done<T>) -> Done<T> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let opened = nudge::read(ctx.project.open_pane(ctx.ax, "Mixer"))?;
    let result = (|| {
        let strip = strip_of(ctx, owner)?;
        with(ctx, &strip)
    })();
    if opened {
        let _ = ctx.project.close_pane(ctx.ax, "Mixer");
    }
    ctx.project.forget();
    result
}

/// A strip's insert slots from the top, as a plug-in kind needs them.
#[derive(Debug, Clone, Default)]
pub struct Inserts {
    /// The instrument's slot, on an instrument strip.
    pub instrument: Option<Node>,
    /// The audio effect slots from the top (slot 1 first): occupied slots' groups and empty slots' buttons alike.
    pub effects: Vec<Node>,
    /// The insert bars, each with the place in `effects` of the slot it sits above.
    pub bars: Vec<(usize, Node)>,
}

impl Inserts {
    pub fn of(strip: &Node, strings: &Strings) -> Inserts {
        let parts = Strip::of(strip, strings);
        // The slots in the strip's own (bottom-up) order, each with the bar listed just before it (its top edge).
        let mut bottom_up: Vec<(Option<&Node>, &Node)> = Vec::new();
        let mut bar: Option<&Node> = None;
        for child in &strip.children {
            if parts.inserts.iter().any(|slot| std::ptr::eq(*slot, child)) {
                bottom_up.push((bar.take(), child));
            } else if child.role == "AXButton" && strings.is(&child.desc, "insert bar") {
                bar = Some(child);
            } else {
                bar = None;
            }
        }
        let mut found = Inserts { instrument: parts.instrument.cloned(), ..Inserts::default() };
        for (bar, slot) in bottom_up.into_iter().rev() {
            if let Some(bar) = bar {
                found.bars.push((found.effects.len(), bar.clone()));
            }
            found.effects.push(slot.clone());
        }
        found
    }

    /// The occupied effect slots, from the top, with their place in `effects`.
    pub fn occupied(&self) -> Vec<(usize, &Node)> {
        self.effects.iter().enumerate().filter(|(_, slot)| paths::occupied(slot)).collect()
    }

    /// The effects' names from the top, as their slots show them.
    pub fn names(&self) -> Vec<String> {
        self.occupied().into_iter().map(|(_, slot)| slot.desc.clone()).collect()
    }
}

/// The plug-in at a slot (as the rows count them), with whether it is the instrument.
pub fn plugin_at<'a>(inserts: &'a Inserts, slot: usize) -> Option<(&'a Node, bool)> {
    let instrument = inserts.instrument.as_ref().filter(|slot| paths::occupied(slot));
    match (instrument, slot) {
        (Some(node), 1) => Some((node, true)),
        (Some(_), n) => inserts.effects.iter().filter(|s| paths::occupied(s)).nth(n.checked_sub(2)?).map(|node| (node, false)),
        (None, n) => inserts.effects.iter().filter(|s| paths::occupied(s)).nth(n.checked_sub(1)?).map(|node| (node, false)),
    }
}

/// A target for a change on a strip's plug-in: its ref, kind and its strip's ref.
pub fn plugin_target(epoch: u32, owner: Owner, slot: usize) -> Value {
    json!({"ref": owner.plugin(epoch, slot), "kind": "plugin", "track": owner.text(epoch)})
}

/// A plug-in that went (or moved) since the preview is `changed`; one a restore can't find is `gone`.
pub fn went(fault: crate::refs::Fault) -> crate::refs::Fault {
    if fault.0 == "failed" {
        changed(format!("{}: it went since the preview", fault.1))
    } else {
        fault
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Node {
        // The Keys strip of Scratch as the Mixer lists it, bottom-up.
        Node::from_json(&json!({"role": "AXLayoutItem", "desc": "Keys", "c": [
            {"i": "0", "role": "AXTextField", "desc": "name", "value": "Keys"},
            {"i": "9", "role": "AXButton", "desc": "Stereo Output", "help": "Output slot. Click and hold."},
            {"i": "10", "role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal to an aux channel strip."},
            {"i": "11", "role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
            {"i": "12", "role": "AXButton", "desc": "insert bar"},
            {"i": "13", "role": "AXGroup", "desc": "ChromaVerb", "c": [{"i": "13.0", "role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"i": "14", "role": "AXButton", "desc": "insert bar"},
            {"i": "15", "role": "AXGroup", "desc": "Compressor", "c": [{"i": "15.0", "role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"i": "16", "role": "AXButton", "desc": "insert bar"},
            {"i": "17", "role": "AXGroup", "desc": "Channel EQ", "c": [{"i": "17.0", "role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"i": "18", "role": "AXGroup", "desc": "Piano", "c": [{"i": "18.0", "role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"i": "19", "role": "AXButton", "desc": "MIDI plug-in", "help": "MIDI Effect slot. Insert a MIDI effect."}
        ]}))
    }

    #[test]
    fn a_strips_inserts_are_kept_from_the_top_with_their_bars_under_the_slots_they_follow() {
        let strings = Strings::english();
        let inserts = Inserts::of(&keys(), &strings);
        assert_eq!(inserts.instrument.as_ref().map(|n| n.desc.as_str()), Some("Piano"));
        assert_eq!(
            inserts.effects.iter().map(|n| n.desc.as_str()).collect::<Vec<_>>(),
            ["Channel EQ", "Compressor", "ChromaVerb", "audio plug-in"]
        );
        // Each bar is listed just before the slot whose top edge it is: above Channel EQ, Compressor and ChromaVerb.
        assert_eq!(
            inserts.bars.iter().map(|(above, node)| (*above, node.path.as_str())).collect::<Vec<_>>(),
            [(0, "16"), (1, "14"), (2, "12")]
        );
        assert_eq!(inserts.names(), ["Channel EQ", "Compressor", "ChromaVerb"]);
        assert_eq!(plugin_at(&inserts, 1).map(|(n, i)| (n.desc.as_str(), i)), Some(("Piano", true)));
        assert_eq!(plugin_at(&inserts, 3).map(|(n, i)| (n.desc.as_str(), i)), Some(("Compressor", false)));
        assert!(plugin_at(&inserts, 5).is_none() && plugin_at(&inserts, 0).is_none());
        assert_eq!(Owner::Track(2).plugin(3, 1), "3:plugin:2.1");
        assert_eq!(Owner::Output(1).plugin(3, 2), "3:plugin:0.1.2");
        assert_eq!(Owner::Output(1).param(3, 2, 4), "3:param:0.1.2.4");
        assert_eq!(Owner::Track(1).send(3, 1), "3:send:1.1");
        assert_eq!(Owner::Output(1).text(3), "3:output:1");
    }

    #[test]
    fn refs_name_a_tracks_strip_or_an_outputs() {
        let mut project = crate::project::Project::new(Strings::english());
        let hands = crate::helper::MockHands::new();
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut ax = crate::ax::Ax::new(helper.as_mut(), 7);
        let ctx = Ctx { project: &mut project, ax: &mut ax };
        assert_eq!(plugin(&ctx, &json!({"ref": "1:plugin:2.3"}), "ref", Epoch::Current).unwrap(), (Owner::Track(2), 3));
        assert_eq!(plugin(&ctx, &json!({"ref": "1:plugin:0.1.2"}), "ref", Epoch::Current).unwrap(), (Owner::Output(1), 2));
        assert_eq!(param(&ctx, &json!({"ref": "1:param:0.1.2.5"}), "ref", Epoch::Current).unwrap(), (Owner::Output(1), 2, 5));
        assert_eq!(param(&ctx, &json!({"ref": "1:param:2.1.4"}), "ref", Epoch::Current).unwrap(), (Owner::Track(2), 1, 4));
        assert_eq!(owner(&ctx, &json!({"ref": "1:output:1"}), "ref", Epoch::Current).unwrap(), Owner::Output(1));
        assert_eq!(send(&ctx, &json!({"ref": "1:send:1.2"}), "ref", Epoch::Current).unwrap(), (1, 2));
        for refused in ["1:plugin:2", "1:plugin:0.1", "1:plugin:2.0", "1:track:1", "1:plugin:0.0.1"] {
            assert_eq!(plugin(&ctx, &json!({"ref": refused}), "ref", Epoch::Current).unwrap_err().0, "bad-params", "{refused}");
        }
        assert_eq!(plugin(&ctx, &json!({"ref": "0:plugin:1.1"}), "ref", Epoch::Current).unwrap_err().0, "stale");
        assert!(send(&ctx, &json!({"ref": "1:send:0.1.1"}), "ref", Epoch::Current).is_err());
    }
}
