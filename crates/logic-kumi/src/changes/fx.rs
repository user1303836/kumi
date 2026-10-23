//! A plug-in's bypass, through its slot's bypass checkbox on the track's strip in the Mixer pane (opened for the
//! write and closed after, as the readers do). Logic keeps no undo step for it. Adding, deleting, moving a plug-in
//! and its parameters wait: the slot's pop-up menu can't be driven from outside, and the plug-in window isn't
//! reached yet.

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::{changed, gone, quoted, ref_text, tracks, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::ax::paths::{self, Strip};
use crate::ax::Node;
use crate::refs::{bad, failed};

/// How long a bypass press gets before the checkbox is read back.
const PRESS_SETTLE_MS: u64 = 120;

/// A plug-in ref: the track's place and the slot (both from 1); a restore's track is carried to where it is now.
fn plugin_at(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<(usize, usize)> {
    let numbers = super::parse_ref(ctx, input, key, "plugin", epoch, 2)?;
    let track = super::track_place(
        ctx,
        &json!({key: format!("{}:track:{}", crate::refs::epoch_of(input[key].as_str().unwrap_or_default()).unwrap_or(ctx.project.epoch), numbers[0])}),
        key,
        epoch,
    )?;
    Ok((track, numbers[1]))
}

/// A plug-in's slot as the strip shows it: its group's node (with the bypass checkbox), its name, whether bypassed.
struct Slot {
    group: Node,
    name: String,
    bypassed: bool,
}

/// The strip's occupied slots, in the order the rows count them: the instrument first, then the inserts from the top.
fn slots(strip: &Node, strings: &crate::ax::strings::Strings) -> Vec<Slot> {
    let parts = Strip::of(strip, strings);
    let slot = |group: &Node| Slot {
        group: group.clone(),
        name: group.desc.clone(),
        bypassed: group.children.iter().any(|c| c.role == "AXCheckBox" && strings.is(&c.desc, "bypass") && c.is_on()),
    };
    let mut out = Vec::new();
    if let Some(instrument) = parts.instrument.filter(|s| paths::occupied(s)) {
        out.push(slot(instrument));
    }
    for insert in parts.inserts.iter().rev().filter(|s| paths::occupied(s)) {
        out.push(slot(insert));
    }
    out
}

/// The strip of a track in the open Mixer pane, matched by the header's name as the readers match them.
fn strip_of(ctx: &mut Ctx, track: usize) -> Done<Node> {
    let headers = nudge::read(tracks::headers(ctx))?;
    let header = headers.get(track - 1).ok_or_else(|| crate::refs::not_in_project(&ref_text(ctx, "track", &[track])))?;
    let strips = nudge::read(ctx.project.mixer_strip_nodes(ctx.ax))?;
    let strings = &ctx.project.strings;
    let mut used = 0usize;
    for (index, strip) in strips.iter().enumerate() {
        let parts = Strip::of(strip, strings);
        let name = parts.name.map(Node::value_text).unwrap_or_else(|| strip.desc.clone());
        let output = parts.bounce.is_some();
        if output {
            continue;
        }
        let matches = headers.iter().take(track).filter(|h| h.header.name == name).count();
        if name == header.header.name {
            used += 1;
            if used == matches {
                return Ok(strips[index].clone());
            }
        }
    }
    Err(failed(format!("Logic Pro's Mixer shows no strip for track {track} ({})", quoted(&header.header.name))))
}

/// Opens the Mixer, finds the slot and runs `with` on it, then closes the Mixer if it was closed before.
fn with_slot<T>(ctx: &mut Ctx, track: usize, slot: usize, with: impl FnOnce(&mut Ctx, &Slot) -> Done<T>) -> Done<T> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let opened = nudge::read(ctx.project.open_pane(ctx.ax, "Mixer"))?;
    let result = (|| {
        let strip = strip_of(ctx, track)?;
        let slots = slots(&strip, &ctx.project.strings);
        let found = slots.get(slot - 1).ok_or_else(|| crate::refs::not_in_project(&ref_text(ctx, "plugin", &[track, slot])))?;
        with(ctx, found)
    })();
    if opened {
        let _ = ctx.project.close_pane(ctx.ax, "Mixer");
    }
    ctx.project.forget();
    result
}

fn bypass_input(input: &Value) -> Done<bool> {
    input.get("bypass").and_then(Value::as_bool).ok_or_else(|| bad("bypass must be true or false"))
}

fn state(bypassed: bool) -> Value {
    json!({"bypassed": bypassed})
}

fn title(name: &str, track: &str, bypassed: bool) -> String {
    format!("{} {} on {}", if bypassed { "Bypassed" } else { "Enabled" }, quoted(name), quoted(track))
}

fn target(ctx: &Ctx, track: usize, slot: usize) -> Value {
    json!({"ref": ref_text(ctx, "plugin", &[track, slot]), "kind": "plugin", "track": ref_text(ctx, "track", &[track])})
}

fn track_name(ctx: &mut Ctx, track: usize) -> Done<String> {
    let headers = nudge::read(tracks::headers(ctx))?;
    Ok(headers.get(track - 1).map(tracks::Found::label).unwrap_or_else(|| format!("Track {track}")))
}

pub fn bypass_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (track, slot) = plugin_at(ctx, input, "ref", Epoch::Current)?;
    let bypass = bypass_input(input)?;
    let name = track_name(ctx, track)?;
    let (now, plugin) = with_slot(ctx, track, slot, |_, found| Ok((state(found.bypassed), found.name.clone())))?;
    let mut display = json!({"title": title(&plugin, &name, bypass), "before": now, "after": state(bypass)});
    if now["bypassed"] == bypass {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, track, slot), prior: now.clone(), fence: now, display })
}

/// Presses the slot's bypass and reads it back: whether bypassed after.
fn press_bypass(ctx: &mut Ctx, found: &Slot) -> Done<bool> {
    let strings = &ctx.project.strings;
    let checkbox = found
        .group
        .children
        .iter()
        .find(|c| c.role == "AXCheckBox" && strings.is(&c.desc, "bypass"))
        .ok_or_else(|| failed(format!("the slot of {} shows no bypass", quoted(&found.name))))?;
    let control = Control::new(&ctx.project.root, &checkbox.path);
    let after = nudge::wrote(nudge::press(ctx.ax, &control, PRESS_SETTLE_MS))?;
    Ok(nudge::is_on(&after))
}

pub fn bypass_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (track, slot) = plugin_at(ctx, input, "ref", Epoch::Current)?;
    let bypass = bypass_input(input)?;
    let name = track_name(ctx, track)?;
    let (plugin, before, after) = with_slot(ctx, track, slot, |ctx, found| {
        if state(found.bypassed) != *fence {
            return Err(changed(format!("{} was switched in Logic Pro since the preview", quoted(&found.name))));
        }
        let after = if found.bypassed == bypass { bypass } else { press_bypass(ctx, found)? };
        Ok((found.name.clone(), found.bypassed, after))
    })?;
    if after != bypass {
        return Err(nudge::kept(&format!("{}'s bypass", quoted(&plugin)), bypass, after));
    }
    Ok(Applied {
        applied: state(after),
        created: None,
        target: None,
        material: None,
        display: json!({"title": title(&plugin, &name, after), "before": state(before), "after": state(after)}),
        step: None,
        probe: None,
    })
}

pub fn bypass_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let (track, slot) = plugin_at(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let (prior, applied) = (restore.prior["bypassed"] == true, restore.applied["bypassed"] == true);
    let name = track_name(ctx, track)?;
    let discard = restore.discard;
    let (plugin, after) = with_slot(ctx, track, slot, |ctx, found| {
        if found.bypassed == prior {
            return Ok((found.name.clone(), prior));
        }
        if !discard && found.bypassed != applied {
            return Err(changed(format!("{} has been switched in Logic Pro since Kumi did", quoted(&found.name))));
        }
        Ok((found.name.clone(), press_bypass(ctx, found)?))
    })?;
    if after != prior {
        return Err(nudge::kept(&format!("{}'s bypass", quoted(&plugin)), prior, after));
    }
    Ok(Restored::by_value(state(prior), json!({"title": format!("{} (put back)", title(&plugin, &name, prior))})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::strings::Strings;

    #[test]
    fn a_strips_slots_are_counted_from_the_top_with_the_instrument_first() {
        let strings = Strings::english();
        let strip = Node::from_json(&json!({"role": "AXLayoutItem", "desc": "Keys", "c": [
            {"i": "0", "role": "AXTextField", "desc": "name", "value": "Keys"},
            {"i": "1", "role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
            {"i": "2", "role": "AXButton", "desc": "insert bar"},
            {"i": "3", "role": "AXGroup", "desc": "ChromaVerb", "c": [{"i": "3.0", "role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"i": "4", "role": "AXButton", "desc": "insert bar"},
            {"i": "5", "role": "AXGroup", "desc": "Channel EQ", "c": [{"i": "5.0", "role": "AXCheckBox", "desc": "bypass", "value": true}]},
            {"i": "6", "role": "AXGroup", "desc": "Piano", "c": [{"i": "6.0", "role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"i": "7", "role": "AXButton", "desc": "MIDI plug-in", "help": "MIDI Effect slot. Insert a MIDI effect."}
        ]}));
        let found = slots(&strip, &strings);
        assert_eq!(
            found.iter().map(|s| (s.name.as_str(), s.bypassed)).collect::<Vec<_>>(),
            [("Piano", false), ("Channel EQ", true), ("ChromaVerb", false)]
        );
        assert_eq!(found[1].group.path, "5");
        assert_eq!(title("Channel EQ", "Vocal", true), "Bypassed “Channel EQ” on “Vocal”");
        assert!(bypass_input(&json!({"bypass": "yes"})).is_err());
    }
}
