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
    kind: &'static str,
    checkbox: Node,
    bypassed: bool,
}

/// Identity excludes values, selection, enabled state, frames and UI paths. Optional stable AX labels are kept
/// when present; Logic does not expose a plug-in instance id, manufacturer or component class through this slot.
fn ax_identity(node: &Node) -> Value {
    let mut identity = json!({"role": node.role});
    for (key, value) in [("subrole", &node.subrole), ("id", &node.id)] {
        if !value.trim().is_empty() {
            identity[key] = value.trim().into();
        }
    }
    identity
}

fn switch_value(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_f64().filter(|value| value.is_finite()).map(|value| value != 0.0),
        Value::String(value) => match value.as_str() {
            "on" | "1" | "true" => Some(true),
            "off" | "0" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// The strip's occupied slots, in the order the rows count them: the instrument first, then the inserts from the top.
fn slots(strip: &Node, strings: &crate::ax::strings::Strings) -> Done<Vec<Slot>> {
    if strip.more != 0 {
        return Err(failed("Logic Pro's Mixer strip is incomplete; the plug-in chain can't be identified safely"));
    }
    let parts = Strip::of(strip, strings);
    // An unreadable occupied slot must not disappear from the chain and make the next slot inherit its ref.
    for group in strip.children.iter().filter(|child| child.role == "AXGroup") {
        let known_slot = parts.instrument.is_some_and(|slot| std::ptr::eq(slot, group))
            || parts.inserts.iter().chain(parts.sends.iter()).any(|slot| std::ptr::eq(*slot, group));
        let automation = group.children.iter().any(|child| child.role == "AXCheckBox" && strings.is(&child.desc, "automation"));
        if !known_slot && !automation {
            return Err(failed("Logic Pro's Mixer has an unrecognized slot group; the full plug-in chain can't be identified safely"));
        }
    }
    let slot = |group: &Node, kind: &'static str| -> Done<Slot> {
        let name = group.desc.trim();
        if name.is_empty() || group.more != 0 || group.role != "AXGroup" {
            return Err(failed("Logic Pro's occupied plug-in slot has no complete readable identity"));
        }
        let controls: Vec<_> = group.children.iter().filter(|c| c.role == "AXCheckBox" && strings.is(&c.desc, "bypass")).collect();
        let checkbox: &Node = match controls.as_slice() {
            [checkbox] => checkbox,
            _ => return Err(failed("Logic Pro's plug-in slot has no unique bypass control")),
        };
        let bypassed = switch_value(&checkbox.value).ok_or_else(|| failed("Logic Pro's plug-in bypass state isn't readable"))?;
        Ok(Slot { group: group.clone(), name: name.into(), kind, checkbox: Node::clone(checkbox), bypassed })
    };
    let mut out = Vec::new();
    if let Some(instrument) = parts.instrument.filter(|s| paths::occupied(s)) {
        out.push(slot(instrument, "instrument")?);
    }
    for insert in parts.inserts.iter().rev().filter(|s| paths::occupied(s)) {
        out.push(slot(insert, "effect")?);
    }
    Ok(out)
}

/// The strip of a track in the open Mixer pane, matched by the header's name as the readers match them.
fn strip_of(ctx: &mut Ctx, track: usize) -> Done<(Node, Value)> {
    let headers = nudge::read(tracks::headers(ctx))?;
    tracks::guard_shape(ctx, &headers)?;
    let header = headers.get(track - 1).ok_or_else(|| crate::refs::not_in_project(&ref_text(ctx, "track", &[track])))?;
    let name = header.header.name.trim();
    if name.is_empty() || headers.iter().filter(|header| header.header.name.trim() == name).count() != 1 {
        return Err(failed("Logic Pro's plug-in owning track has no unique readable name"));
    }
    let strips = nudge::read(ctx.project.mixer_strip_nodes(ctx.ax))?;
    let strings = &ctx.project.strings;
    let mut found = Vec::new();
    for strip in &strips {
        let parts = Strip::of(strip, strings);
        let strip_name = parts.name.map(Node::value_text).unwrap_or_else(|| strip.desc.clone());
        let output = parts.bounce.is_some();
        if output {
            continue;
        }
        if strip_name.trim() == name {
            found.push(strip);
        }
    }
    let strip: &Node = match found.as_slice() {
        [strip] => strip,
        _ => return Err(failed(format!("Logic Pro's Mixer shows no unique strip for track {track} ({})", quoted(name)))),
    };
    if strip.role != "AXLayoutItem" {
        return Err(failed("Logic Pro's plug-in owning strip has no readable identity"));
    }
    let kind = crate::rows::StripState::of(strip, strings).kind;
    let owner = json!({"name": name, "kind": kind, "ax": ax_identity(strip)});
    Ok((Node::clone(strip), owner))
}

/// Opens the Mixer, finds the slot and runs `with` on it, then closes the Mixer if it was closed before.
fn with_slot<T>(ctx: &mut Ctx, track: usize, slot: usize, with: impl FnOnce(&mut Ctx, &Slot, &Value) -> Done<T>) -> Done<T> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let opened = nudge::read(ctx.project.open_pane(ctx.ax, "Mixer"))?;
    let result = (|| {
        let (strip, owner) = strip_of(ctx, track)?;
        let slots = slots(&strip, &ctx.project.strings)?;
        let found = slots.get(slot - 1).ok_or_else(|| crate::refs::not_in_project(&ref_text(ctx, "plugin", &[track, slot])))?;
        let chain: Vec<_> = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                json!({
                    "slot": index + 1, "name": slot.name, "kind": slot.kind,
                    "ax": ax_identity(&slot.group), "bypass": ax_identity(&slot.checkbox),
                })
            })
            .collect();
        let identity = json!({"owner": owner, "slot": slot, "chain": chain});
        with(ctx, found, &identity)
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
    tracks::guard_shape(ctx, &headers)?;
    Ok(headers.get(track - 1).map(tracks::Found::label).unwrap_or_else(|| format!("Track {track}")))
}

pub fn bypass_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (track, slot) = plugin_at(ctx, input, "ref", Epoch::Current)?;
    let bypass = bypass_input(input)?;
    let name = track_name(ctx, track)?;
    let (now, plugin, identity) =
        with_slot(ctx, track, slot, |_, found, identity| Ok((state(found.bypassed), found.name.clone(), identity.clone())))?;
    let mut display = json!({"title": title(&plugin, &name, bypass), "before": now, "after": state(bypass)});
    if now["bypassed"] == bypass {
        display["unchanged"] = true.into();
    }
    let mut target = target(ctx, track, slot);
    target["identity"] = identity.clone();
    let mut fence = now.clone();
    fence["identity"] = identity;
    Ok(Preview { target, prior: now, fence, display })
}

/// Presses the slot's bypass and reads it back: whether bypassed after.
fn press_bypass(ctx: &mut Ctx, found: &Slot) -> Done<bool> {
    let control = Control::new(&ctx.project.root, &found.checkbox.path);
    let after = nudge::wrote(nudge::press(ctx.ax, &control, PRESS_SETTLE_MS))?;
    switch_value(&after).ok_or_else(|| super::uncertain("Logic Pro's bypass press has no readable result"))
}

fn expected_identity(value: &Value) -> Done<&Value> {
    value
        .get("identity")
        .filter(|identity| identity.is_object())
        .ok_or_else(|| failed("the plug-in identity is missing; read and preview it again before changing or restoring its bypass"))
}

pub fn bypass_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let identity = expected_identity(fence)?;
    let (track, slot) = plugin_at(ctx, input, "ref", Epoch::Current)?;
    let bypass = bypass_input(input)?;
    let name = track_name(ctx, track)?;
    let (plugin, before, after) = with_slot(ctx, track, slot, |ctx, found, current_identity| {
        if current_identity != identity {
            return Err(changed("the plug-in, its owning track or its complete chain changed since the preview; nothing changed"));
        }
        if fence["bypassed"].as_bool() != Some(found.bypassed) {
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
        target: None,
        created: None,
        material: None,
        display: json!({"title": title(&plugin, &name, after), "before": state(before), "after": state(after)}),
        step: None,
        probe: None,
    })
}

pub fn bypass_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let identity = expected_identity(restore.target)?;
    let (track, slot) = plugin_at(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let prior = restore.prior["bypassed"].as_bool().ok_or_else(|| bad("prior.bypassed must be true or false"))?;
    let applied = restore.applied["bypassed"].as_bool().ok_or_else(|| bad("applied.bypassed must be true or false"))?;
    let name = track_name(ctx, track)?;
    let discard = restore.discard;
    let (plugin, after) = with_slot(ctx, track, slot, |ctx, found, current_identity| {
        // Identity is always checked, including already-reverted values and discard: true.
        if current_identity != identity {
            return Err(changed("the plug-in, its owning track or its complete chain changed since Kumi set it; nothing changed"));
        }
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
    use crate::ax::Ax;
    use crate::fixtures::Scene;
    use crate::helper::{Launch, MockHands};
    use crate::project::Project;

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
        let found = slots(&strip, &strings).unwrap();
        assert_eq!(
            found.iter().map(|s| (s.name.as_str(), s.bypassed)).collect::<Vec<_>>(),
            [("Piano", false), ("Channel EQ", true), ("ChromaVerb", false)]
        );
        assert_eq!(found[1].group.path, "5");
        assert_eq!(title("Channel EQ", "Vocal", true), "Bypassed “Channel EQ” on “Vocal”");
        assert!(bypass_input(&json!({"bypass": "yes"})).is_err());
    }

    fn recorded_tree(scene: &Scene, description: &str) -> Value {
        scene
            .calls
            .iter()
            .find(|call| call.op == "tree" && call.reply["node"]["desc"] == description)
            .unwrap_or_else(|| panic!("the original bypass scene records {description}"))
            .reply["node"]
            .clone()
    }

    fn target_group(mixer: &mut Value) -> &mut Value {
        let strip = mixer["c"].as_array_mut().unwrap().iter_mut().find(|strip| strip["desc"] == "Vocal").unwrap();
        strip["c"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|group| {
                group["role"] == "AXGroup"
                    && group["c"].as_array().is_some_and(|children| children.iter().any(|child| child["desc"] == "bypass"))
            })
            .unwrap()
    }

    fn set_bypass(mixer: &mut Value, bypassed: bool) {
        let group = target_group(mixer);
        let checkbox = group["c"].as_array_mut().unwrap().iter_mut().find(|child| child["desc"] == "bypass").unwrap();
        checkbox["value"] = bypassed.into();
    }

    /// Every edit reads headers, the visible Mixer button, headers again, and the complete Mixer. These nodes are
    /// copied from the real recording; the derived scene keeps Mixer open so any helper write is a regression.
    fn fresh_snapshot(hands: &MockHands, headers: &Value, bar: &Value, mixer: &Value) {
        for node in [headers, bar, headers, mixer] {
            hands.reply("tree", Ok(json!({"node": node})));
        }
    }

    #[test]
    fn a_same_slot_replacement_with_the_same_bypass_value_never_receives_an_apply_or_restore() {
        let scene = Scene::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/change-fx-bypass")).unwrap();
        let headers = recorded_tree(&scene, "Tracks header");
        let mut bar = recorded_tree(&scene, "Control Bar");
        let mixer = recorded_tree(&scene, "Mixer");
        let button = bar["c"].as_array_mut().unwrap().iter_mut().find(|node| node["desc"] == "Mixer").unwrap();
        button["value"] = 1.into();
        let hands = MockHands::new();
        let mut helper = Launch::launch(&mut hands.clone()).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let mut project = Project::new(Strings::english());
        project.layout.headers = Some(headers["i"].as_str().unwrap().into());
        project.layout.control_bar = Some(bar["i"].as_str().unwrap().into());
        project.layout.mixer = Some(mixer["i"].as_str().unwrap().into());
        let mut ctx = Ctx { project: &mut project, ax: &mut ax };
        let input = json!({"ref": "1:plugin:1.1", "bypass": true});
        fresh_snapshot(&hands, &headers, &bar, &mixer);
        let preview = bypass_preview(&mut ctx, &input).unwrap();
        assert_eq!(preview.target["identity"], preview.fence["identity"]);
        assert_eq!(preview.fence["identity"]["owner"]["name"], "Vocal");
        assert_eq!(preview.fence["identity"]["chain"][0]["name"], "Channel EQ");
        assert!(preview.fence["identity"]["chain"][0].get("bypassed").is_none(), "identity excludes the value being controlled");

        let mut replacement = mixer.clone();
        target_group(&mut replacement)["desc"] = "Compressor".into();
        fresh_snapshot(&hands, &headers, &bar, &replacement);
        assert_eq!(bypass_apply(&mut ctx, &input, &preview.fence).err().unwrap().0, "changed");
        assert!(hands.calls().iter().all(|(op, _)| op == "tree"), "a same-boolean replacement gets no helper action");
        let mut other_ax_identity = mixer.clone();
        target_group(&mut other_ax_identity)["id"] = "another-visible-plug-in-id".into();
        fresh_snapshot(&hands, &headers, &bar, &other_ax_identity);
        assert_eq!(bypass_apply(&mut ctx, &input, &preview.fence).err().unwrap().0, "changed");

        // The original plug-in can still be bypassed. A changed chain is refused even if the target itself stays
        // in slot one with the original name, and unavailable identity is never treated as an empty chain.
        let mut extended = mixer.clone();
        let mut neighbour = target_group(&mut extended).clone();
        neighbour["desc"] = "ChromaVerb".into();
        let children = extended["c"][0]["c"].as_array_mut().unwrap();
        let index = children.iter().position(|node| node["role"] == "AXGroup" && node["desc"] == "Channel EQ").unwrap();
        children.insert(index, neighbour);
        fresh_snapshot(&hands, &headers, &bar, &extended);
        assert_eq!(bypass_apply(&mut ctx, &input, &preview.fence).err().unwrap().0, "changed");
        for (index, mut unreadable) in [mixer.clone(), mixer.clone()].into_iter().enumerate() {
            if index == 0 {
                target_group(&mut unreadable)["desc"] = Value::String(String::new());
            } else {
                let checkbox =
                    target_group(&mut unreadable)["c"].as_array_mut().unwrap().iter_mut().find(|node| node["desc"] == "bypass").unwrap();
                checkbox["value"] = Value::Null;
            }
            fresh_snapshot(&hands, &headers, &bar, &unreadable);
            assert_eq!(bypass_apply(&mut ctx, &input, &preview.fence).err().unwrap().0, "failed");
        }
        let before = hands.calls().len();
        assert_eq!(bypass_apply(&mut ctx, &input, &state(false)).err().unwrap().0, "failed");
        assert_eq!(hands.calls().len(), before, "a legacy boolean-only fence never reaches the helper");
        assert!(hands.calls().iter().all(|(op, _)| op == "tree"));

        fresh_snapshot(&hands, &headers, &bar, &mixer);
        hands.reply("action", Ok(json!({"after": true})));
        let applied = bypass_apply(&mut ctx, &input, &preview.fence).unwrap();
        assert_eq!(applied.applied, state(true));
        let desc = "Kumi: restore bypass";
        let key = "fx-bypass 1:plugin:1.1".into();
        let mut restore = Restore {
            target: &preview.target,
            prior: &preview.prior,
            applied: &applied.applied,
            created: None,
            material: None,
            discard: false,
            desc,
            key,
        };
        for bypassed in [true, false] {
            for discard in [false, true] {
                set_bypass(&mut replacement, bypassed);
                restore.discard = discard;
                let before = hands.calls().len();
                fresh_snapshot(&hands, &headers, &bar, &replacement);
                assert_eq!(bypass_restore(&mut ctx, &restore).err().unwrap().0, "changed");
                assert!(
                    hands.calls()[before..].iter().all(|(op, _)| op == "tree"),
                    "identity is checked even for an already-reverted boolean and discard: true"
                );
            }
        }
        let mut applied_mixer = mixer.clone();
        set_bypass(&mut applied_mixer, true);
        fresh_snapshot(&hands, &headers, &bar, &applied_mixer);
        hands.reply("action", Ok(json!({"after": false})));
        assert_eq!(bypass_restore(&mut ctx, &restore).unwrap().restored, state(false), "ordinary value restore still works");
        let before = hands.calls().len();
        fresh_snapshot(&hands, &headers, &bar, &mixer);
        assert_eq!(bypass_restore(&mut ctx, &restore).unwrap().restored, state(false));
        assert!(
            hands.calls()[before..].iter().all(|(op, _)| op == "tree"),
            "an unchanged identity permits recognizing a manual value reversion"
        );
        let legacy_target = target(&ctx, 1, 1);
        let legacy_restore = Restore { target: &legacy_target, key: restore.key.clone(), ..restore };
        let before = hands.calls().len();
        assert_eq!(bypass_restore(&mut ctx, &legacy_restore).err().unwrap().0, "failed");
        assert_eq!(hands.calls().len(), before, "a missing restore identity is refused before any helper call");
    }
}
