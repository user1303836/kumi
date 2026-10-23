//! FX in any chain: a track's main chain, its input chain (the master's monitoring FX), a take's chain, and a
//! container's children. They are inserted by name, deleted, moved, bypassed and set. An FX is found by its GUID, which
//! REAPER keeps when the FX moves or comes back.
//!
//! A deleted FX is kept as its entry in its owner's chunk (its state, its modulation and parameter envelopes, a
//! container's children) and put back as an FX chain file that holds just that entry: REAPER adds it with the same id and
//! state, next to the FX it was next to, and nothing else in the chain restarts.

use serde_json::{json, Value};

use super::fxchain::{self, changed, gone_if_missing, went, ChainFile, FxAt, Holder, QuietAutomation};
use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Fx, FxOwner};
use crate::refs::{self, Ref};
use crate::rows;

/// The longest a plug-in name may be.
const NAME_LIMIT: usize = 256;

#[derive(Clone, Copy, PartialEq)]
pub enum Which {
    Add,
    Delete,
    Move,
    Bypass,
    Param,
}

/// A plug-in's name as a producer says it: without the format in front and the maker behind.
pub(crate) fn short_name(name: &str) -> String {
    let rest = match name.split_once(": ") {
        Some((format, rest)) if format.len() <= 6 && !format.contains(' ') => rest,
        _ => name,
    };
    let rest = match rest.rfind(" (") {
        Some(at) if rest.ends_with(')') && at > 0 => &rest[..at],
        _ => rest,
    };
    rest.to_string()
}

pub(super) fn fx_label(ctx: &Ctx, fx: Fx) -> String {
    let renamed = ctx.reaper.fx_config(fx, "renamed_name").unwrap_or_default();
    rows::clip(&if renamed.is_empty() { short_name(&ctx.reaper.fx_name(fx)) } else { renamed })
}

/// Where an FX is, as a title's sentence ends: "on “Bass”", "in “Parallel” on “Bass”", "on take “vox.wav”".
pub(super) fn place_phrase(ctx: &Ctx, holder: Holder) -> String {
    match holder {
        Holder::Container(_) => format!("in {}", fxchain::holder_label(ctx, holder)),
        _ => format!("on {}", fxchain::holder_label(ctx, holder)),
    }
}

fn chain_fence(chain: &[String]) -> Value {
    json!({"chain": chain})
}

/// What tells an FX apart from how Kumi left it: its name, whether it runs, its preset and its parameters' values, less the
/// parameters that move by themselves (`volatile`, NIT-7: found without holding REAPER's main thread).
pub(super) fn signature(ctx: &Ctx, fx: Fx) -> Value {
    let volatile = super::volatile::moving(ctx, fx);
    let signature = signature_without(ctx, fx, &volatile);
    super::volatile::remember(ctx, fx, &signature);
    signature
}

/// The signature again, as one that was taken before: a parameter it found to move by itself is left out of this one too,
/// and so is one the watch right after it found (then the recorded signature is answered back: nothing else moved).
pub(super) fn signature_as(ctx: &Ctx, fx: Fx, recorded: &Value) -> Value {
    let volatile: Vec<i32> = recorded["volatile"].as_array().into_iter().flatten().filter_map(Value::as_i64).map(|n| n as i32).collect();
    let now = signature_without(ctx, fx, &volatile);
    if now["hash"] != recorded["hash"] && same_but_hash(&now, recorded) && super::volatile::same_but_for_moving(ctx, fx, recorded) {
        return recorded.clone();
    }
    now
}

/// An FX's signature and, for a container, every FX inside it at any depth: each one's GUID and its own signature, so a
/// knob turned or a plug-in dropped in inside a container counts as what it is, the producer's.
pub(super) fn deep_signature(ctx: &Ctx, fx: Fx) -> Value {
    let mut signature = signature(ctx, fx);
    if fxchain::is_container(ctx.reaper, fx) {
        signature["children"] = ctx
            .reaper
            .fx_children(fx)
            .into_iter()
            .map(|child| json!({"guid": ctx.reaper.fx_guid(child), "signature": deep_signature(ctx, child)}))
            .collect::<Vec<_>>()
            .into();
    }
    signature
}

/// Whether the FX, and every FX inside it, is as its `deep_signature` says (a signature taken without the FX inside,
/// before they were kept, holds only while there are none).
pub(super) fn deep_as_recorded(ctx: &Ctx, fx: Fx, recorded: &Value) -> bool {
    let mut own = recorded.clone();
    let inside = own.as_object_mut().and_then(|fields| fields.remove("children"));
    if signature_as(ctx, fx, &own) != own {
        return false;
    }
    let now = ctx.reaper.fx_children(fx);
    match inside.as_ref().and_then(Value::as_array) {
        None => now.is_empty(),
        Some(kept) => {
            kept.len() == now.len()
                && kept.iter().zip(&now).all(|(kept, child)| {
                    kept["guid"].as_str().is_some_and(|guid| guid.eq_ignore_ascii_case(&ctx.reaper.fx_guid(*child)))
                        && deep_as_recorded(ctx, *child, &kept["signature"])
                })
        }
    }
}

/// Whether two signatures differ only in their parameters' values.
fn same_but_hash(a: &Value, b: &Value) -> bool {
    ["name", "enabled", "offline", "preset", "params"].iter().all(|key| a[key] == b[key])
}

fn signature_without(ctx: &Ctx, fx: Fx, volatile: &[i32]) -> Value {
    let reaper = ctx.reaper;
    json!({
        "name": reaper.fx_name(fx),
        "enabled": reaper.fx_enabled(fx),
        "offline": reaper.fx_offline(fx),
        "preset": reaper.fx_preset(fx),
        "params": reaper.fx_param_count(fx),
        "volatile": volatile,
        "hash": super::volatile::hash(&super::volatile::values(reaper, fx), volatile),
    })
}

pub(super) fn fx_target(ctx: &Ctx, at: &FxAt) -> Value {
    let mut target = json!({"ref": refs::fx_ref(&at.guid), "kind": "fx"});
    // (the track a track FX is on, for a restore made before the chain was kept: `trackRef`)
    if let Some(track) = at.holder.track(ctx.reaper) {
        target["trackRef"] = refs::track_ref(&rows::track_id(ctx.reaper, track)).into();
    }
    target
}

pub fn preview(ctx: &Ctx, input: &Value, which: Which) -> Done<Preview> {
    match which {
        Which::Add => add_preview(ctx, input),
        Which::Delete => delete_preview(ctx, input),
        Which::Move => move_preview(ctx, input),
        Which::Bypass => bypass_preview(ctx, input),
        Which::Param => param_preview(ctx, input),
    }
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value, which: Which) -> Done<Applied> {
    match which {
        Which::Add => add_apply(ctx, input, fence),
        Which::Delete => delete_apply(ctx, input, fence),
        Which::Move => move_apply(ctx, input, fence),
        Which::Bypass => bypass_apply(ctx, input, fence),
        Which::Param => param_apply(ctx, input, fence),
    }
}

pub fn restore(ctx: &Ctx, restore: &Restore, which: Which) -> Done<Restored> {
    match which {
        Which::Add => add_restore(ctx, restore),
        Which::Delete => delete_restore(ctx, restore),
        Which::Move => move_restore(ctx, restore),
        Which::Bypass => bypass_restore(ctx, restore),
        Which::Param => param_restore(ctx, restore),
    }
}

// ----- insert -----

fn plugin_name(input: &Value) -> Done<String> {
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| bad("name must be the plug-in's name"))?;
    if name.chars().count() > NAME_LIMIT || name.chars().any(char::is_control) {
        return Err(bad(format!("name must be one line of at most {NAME_LIMIT} characters")));
    }
    if name.to_ascii_lowercase().ends_with(".rfxchain") {
        return Err(bad("name must be a plug-in's name, not an FX chain file (fx_chain_load loads one)"));
    }
    // (REAPER takes "FXADD:" and a list for several plug-ins at once; one name is one plug-in.)
    if name.to_ascii_uppercase().starts_with("FXADD:") {
        return Err(bad("name must be one plug-in's name, not an FXADD: list"));
    }
    Ok(name.to_string())
}

fn input_flag(input: &Value) -> Done<bool> {
    match input.get("input") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad("input must be true (the track's input FX) or false")),
    }
}

/// The chain an add names: a track (its input chain with `input`), a take or an item (its active take), or a container.
pub(super) fn add_holder(ctx: &Ctx, input: &Value, what: &str) -> Done<Holder> {
    fxchain::holder_named(ctx, input.get("ref").unwrap_or(&Value::Null), input_flag(input)?, what)
}

pub(super) fn add_position(input: &Value, count: usize) -> Done<Option<usize>> {
    match input.get("index") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|i| *i as usize <= count)
            .map(|i| Some(i as usize))
            .ok_or_else(|| bad(format!("index must be a place in the chain, from 0 to {count}"))),
    }
}

fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let holder = add_holder(ctx, input, "fx-add")?;
    let name = plugin_name(input)?;
    let before = fxchain::guids(ctx.reaper, holder);
    let place = add_position(input, before.len())?.unwrap_or(before.len());
    fxchain::add_place(ctx.reaper, holder, place)?;
    Ok(Preview {
        target: fxchain::holder_target(ctx, holder),
        prior: chain_fence(&before),
        fence: chain_fence(&before),
        display: json!({"title": format!("Added {} to {}", super::quoted(&short_name(&name)), fxchain::holder_label(ctx, holder)), "position": place}),
    })
}

/// What an add that went wrong left is taken off again (every FX that wasn't in the chain before): what is left would be
/// there and unknown to HISTORY.
pub(super) fn take_back_added(ctx: &Ctx, holder: Holder, before: &[String], message: &str) -> super::Fault {
    for fx in fxchain::members(ctx.reaper, holder).into_iter().rev() {
        if fxchain::position_of(before, &ctx.reaper.fx_guid(fx)).is_none() {
            ctx.reaper.fx_delete(fx);
        }
    }
    if fxchain::guids(ctx.reaper, holder) == before {
        failed(format!("{message}; what was added is taken back"))
    } else {
        ("uncertain".into(), format!("{message}, and Kumi could not take back what it wrote: read the chain's FX"))
    }
}

fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let holder = add_holder(ctx, input, "fx-add").map_err(went)?;
    let name = plugin_name(input)?;
    let before = fxchain::guids(ctx.reaper, holder);
    if chain_fence(&before) != *fence {
        return Err(changed("the FX in that chain changed since the preview"));
    }
    let place = add_position(input, before.len())?.unwrap_or(before.len());
    let (address, rec) = fxchain::add_place(ctx.reaper, holder, place)?;
    let answer = ctx.reaper.fx_add(holder.owner(), &name, rec, Some(address));
    let after = fxchain::guids(ctx.reaper, holder);
    if answer < 0 && after == before {
        return Err(failed(format!(
            "REAPER has no plug-in named \"{name}\": use the name the FX browser shows, such as \"ReaEQ (Cockos)\" or \"VST3: Pro-Q 3 (FabFilter)\""
        )));
    }
    let guid = after.get(place).cloned().unwrap_or_default();
    if after.len() != before.len() + 1 || guid.is_empty() || fxchain::position_of(&before, &guid).is_some() {
        return Err(take_back_added(ctx, holder, &before, "the FX chain isn't what it should be after adding the plug-in"));
    }
    let at = fxchain::adopt(ctx, holder, &guid)?;
    let label = fx_label(ctx, at.fx);
    Ok(Applied {
        applied: json!({"guid": guid, "index": place, "signature": signature(ctx, at.fx)}),
        created: Some(json!({"ref": refs::fx_ref(&guid), "kind": "fx"})),
        material: None,
        display: json!({"title": format!("Added {} to {}", super::quoted(&label), fxchain::holder_label(ctx, holder)), "ref": refs::fx_ref(&guid), "index": place}),
        ..Default::default()
    })
}

fn add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = restore.applied["guid"].as_str().ok_or_else(|| bad("applied.guid: the FX that was added"))?;
    let at = fxchain::locate_guid(ctx, guid).map_err(gone_if_missing)?;
    // Its automation goes with it, and is the producer's.
    if !restore.discard && !fxchain::fx_envelopes(ctx.reaper, &at).is_empty() {
        return Err(changed("the FX Kumi added has automation in it now"));
    }
    if !restore.discard && fxchain::is_container(ctx.reaper, at.fx) && !ctx.reaper.fx_children(at.fx).is_empty() {
        return Err(changed("the container Kumi added holds FX now"));
    }
    if !restore.discard && signature_as(ctx, at.fx, &restore.applied["signature"]) != restore.applied["signature"] {
        return Err(changed("the FX Kumi added has been changed in REAPER since"));
    }
    let label = fx_label(ctx, at.fx);
    let before = fxchain::guids(ctx.reaper, at.holder);
    if !ctx.reaper.fx_delete(at.fx) || fxchain::guids(ctx.reaper, at.holder).len() + 1 != before.len() {
        return Err(failed("REAPER wouldn't delete the FX"));
    }
    Ok(Restored {
        restored: json!({"deleted": guid}),
        display: json!({"title": format!("{} taken off {}", super::quoted(&label), fxchain::holder_label(ctx, at.holder))}),
        ..Default::default()
    })
}

// ----- delete -----

/// The keys of a parameter link, after `param.N.` (`modulation`'s fx-param-link), and the modulation switch it needs.
const LINK_KEYS: [&str; 9] = [
    "plink.effect",
    "plink.param",
    "plink.scale",
    "plink.offset",
    "plink.midi_bus",
    "plink.midi_chan",
    "plink.midi_msg",
    "plink.midi_msg2",
    "mod.active",
];

/// The parameter links of other FX in its chain that follow an FX (REAPER keeps a link's source by its place there), each
/// with every key of the link: REAPER clears them when the FX is deleted (7.82 lab), and its undo makes them again.
fn links_to(ctx: &Ctx, at: &FxAt) -> Vec<Value> {
    let reaper = ctx.reaper;
    if matches!(at.holder, Holder::Container(_)) {
        return vec![];
    }
    let chain = fxchain::members(reaper, at.holder);
    let Some(place) = chain.iter().position(|f| *f == at.fx) else { return vec![] };
    let mut links = Vec::new();
    for other in chain.iter().filter(|f| **f != at.fx) {
        for p in 0..reaper.fx_param_count(*other).min(4096) {
            let key = |k: &str| format!("param.{p}.{k}");
            let active = reaper.fx_config(*other, &key("plink.active")).is_some_and(|v| v.trim() == "1");
            let effect = reaper.fx_config(*other, &key("plink.effect")).and_then(|v| v.trim().parse::<i64>().ok());
            if active && effect == Some(place as i64) {
                let keys: serde_json::Map<String, Value> = LINK_KEYS
                    .iter()
                    .filter_map(|k| reaper.fx_config(*other, &key(k)).map(|v| (k.to_string(), Value::from(v.trim().to_string()))))
                    .collect();
                links.push(json!({"fx": reaper.fx_guid(*other), "param": p, "keys": keys}));
            }
        }
    }
    links
}

/// The macros of the container an FX is in that map its parameters: REAPER takes them off with it.
fn macros_of(ctx: &Ctx, at: &FxAt) -> Vec<i64> {
    let reaper = ctx.reaper;
    let Holder::Container(container) = at.holder else { return vec![] };
    let Some(place) = reaper.fx_children(container).iter().position(|c| *c == at.fx) else { return vec![] };
    let number = |p: i32, key: &str| {
        reaper.fx_config(container, &format!("param.{p}.container_map.{key}")).and_then(|v| v.trim().parse::<i64>().ok())
    };
    (0..reaper.fx_param_count(container))
        .filter(|p| number(*p, "fx_index") == Some(place as i64))
        .filter_map(|p| number(p, "fx_parm"))
        .collect()
}

/// Makes again the links and macros that followed a deleted FX, now back at `at`: how many couldn't be (their FX gone),
/// and how many were left as the producer has set them since (a link REAPER cleared reads effect −1 and off, 7.82 lab;
/// one that reads otherwise is theirs now).
fn relink(ctx: &Ctx, at: &FxAt, material: &Value) -> (usize, usize) {
    let reaper = ctx.reaper;
    let (mut lost, mut theirs) = (0, 0);
    let place = fxchain::position_of(&fxchain::guids(reaper, at.holder), &at.guid);
    for link in material["links"].as_array().into_iter().flatten() {
        let other = link["fx"].as_str().and_then(|guid| fxchain::locate_guid(ctx, guid).ok()).filter(|other| other.holder == at.holder);
        let (Some(other), Some(place), Some(p)) = (other, place, link["param"].as_i64()) else {
            lost += 1;
            continue;
        };
        let reads = |k: &str| reaper.fx_config(other.fx, &format!("param.{p}.{k}")).map(|v| v.trim().to_string());
        let cleared = reads("plink.active").is_none_or(|v| v == "0") && reads("plink.effect").is_none_or(|v| v == "-1");
        if !cleared {
            theirs += 1;
            continue;
        }
        for (k, value) in link["keys"].as_object().into_iter().flatten() {
            let value = if k == "plink.effect" { place.to_string() } else { value.as_str().unwrap_or_default().to_string() };
            reaper.fx_set_config(other.fx, &format!("param.{p}.{k}"), &value);
        }
        reaper.fx_set_config(other.fx, &format!("param.{p}.plink.active"), "1");
        fxchain::mark_fx(reaper, other.fx);
    }
    if let (Holder::Container(container), Some(place)) = (at.holder, place) {
        for param in material["macros"].as_array().into_iter().flatten().filter_map(Value::as_i64) {
            reaper.fx_config(container, &format!("container_map.add.{place}.{param}"));
        }
    }
    (lost, theirs)
}

fn delete_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let now = json!({"guid": at.guid, "signature": signature(ctx, at.fx), "chain": fxchain::guids(ctx.reaper, at.holder)});
    let mut display =
        json!({"title": format!("Deleted {} from {}", super::quoted(&fx_label(ctx, at.fx)), fxchain::holder_label(ctx, at.holder))});
    let mut notes = Vec::new();
    let inside = ctx.reaper.fx_children(at.fx).len();
    if inside > 0 {
        notes.push(format!("The {inside} FX inside the container go with it and come back with an undo."));
    }
    let envelopes = fxchain::fx_envelopes_deep(ctx.reaper, &at);
    if envelopes > 0 {
        notes.push(format!(
            "Its {envelopes} automation envelope(s) go with it; an undo brings them back as new envelopes (their refs change)."
        ));
    }
    let (links, macros) = (links_to(ctx, &at).len(), macros_of(ctx, &at).len());
    if links + macros > 0 {
        notes.push(format!(
            "{} follow it: REAPER clears them with it, and an undo makes them again.",
            [(links, "parameter link(s) of other FX"), (macros, "macro(s) of its container")]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect::<Vec<_>>()
                .join(" and ")
        ));
    }
    if !notes.is_empty() {
        display["note"] = notes.join(" ").into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: now.clone(), fence: now, display })
}

/// Where an FX goes back in its chain: after the FX that was before it, else before the one that was after it, else at
/// the place it had (or the end, if the chain is shorter now). The neighbours are kept by id: a place counted by number
/// is wherever the producer's later inserts and deletes moved it.
pub(super) fn back_place(chain: &[String], material: &Value) -> usize {
    if let Some(before) = material["before"].as_str().and_then(|g| fxchain::position_of(chain, g)) {
        return before + 1;
    }
    if let Some(after) = material["after"].as_str().and_then(|g| fxchain::position_of(chain, g)) {
        return after;
    }
    (material["index"].as_u64().unwrap_or(0) as usize).min(chain.len())
}

fn delete_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let chain = fxchain::guids(ctx.reaper, at.holder);
    let now = json!({"guid": at.guid, "signature": signature_as(ctx, at.fx, &fence["signature"]), "chain": chain});
    if now != *fence {
        return Err(changed("the FX changed since the preview"));
    }
    let reaper = ctx.reaper;
    let chunk = fxchain::owner_chunk(reaper, at.fx.owner)?;
    let entry = fxchain::fx_entry(&chunk, matches!(at.fx.owner, FxOwner::Take(_)), &at.guid)
        .ok_or_else(|| failed("the FX isn't in its owner's state (a plug-in REAPER can't describe)"))?;
    let (label, where_) = (fx_label(ctx, at.fx), fxchain::holder_label(ctx, at.holder));
    let name = reaper.fx_name(at.fx);
    let index = fxchain::position_of(&chain, &at.guid).unwrap_or(0);
    let envelopes = fxchain::fx_envelopes_deep(reaper, &at);
    let (links, macros) = (links_to(ctx, &at), macros_of(ctx, &at));
    if !reaper.fx_delete(at.fx) {
        return Err(failed("REAPER wouldn't delete the FX"));
    }
    let after = fxchain::guids(reaper, at.holder);
    if after.len() + 1 != chain.len() || fxchain::position_of(&after, &at.guid).is_some() {
        return Err(failed("the FX is still in the chain after it was deleted"));
    }
    Ok(Applied {
        applied: json!({"deleted": at.guid}),
        created: None,
        material: Some(json!({
            "entry": entry,
            "guid": at.guid,
            "name": name,
            "index": index,
            "before": index.checked_sub(1).and_then(|i| chain.get(i)),
            "after": chain.get(index + 1),
            "chain": fxchain::holder_value(ctx, at.holder),
            "envelopes": envelopes,
            "links": links,
            "macros": macros,
        })),
        display: json!({"title": format!("Deleted {} from {where_}", super::quoted(&label))}),
        ..Default::default()
    })
}

/// Puts an entry back into a chain at a place, by an FX chain file; the FX it made.
pub(super) fn add_entry(ctx: &Ctx, holder: Holder, place: usize, entry: &str, tag: &str) -> Done<FxAt> {
    let before = fxchain::guids(ctx.reaper, holder);
    let (address, rec) = fxchain::add_place(ctx.reaper, holder, place)?;
    let file = ChainFile::write(ctx.reaper, entry, tag)?;
    ctx.reaper.fx_add(holder.owner(), &file.name, rec, Some(address));
    drop(file);
    let after = fxchain::guids(ctx.reaper, holder);
    let made = after.get(place).cloned().unwrap_or_default();
    if after.len() != before.len() + 1 || made.is_empty() || fxchain::position_of(&before, &made).is_some() {
        return Err(take_back_added(ctx, holder, &before, "REAPER wouldn't add the FX back where it was"));
    }
    fxchain::adopt(ctx, holder, &made)
}

fn delete_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let guid = material["guid"].as_str().ok_or_else(|| bad("material.guid"))?;
    let entry = material["entry"].as_str().ok_or_else(|| bad("material.entry"))?;
    if let Ok(at) = fxchain::locate_guid(ctx, guid) {
        // Back already (a retry, or the producer undid it): nothing to write.
        return Ok(Restored {
            restored: json!({"ref": refs::fx_ref(guid), "was": refs::fx_ref(guid)}),
            display: json!({"title": format!("{} is back already", super::quoted(&fx_label(ctx, at.fx)))}),
            unwritten: true,
        });
    }
    let holder = match material.get("chain").filter(|c| c.is_object()) {
        Some(chain) => fxchain::holder_from(ctx, chain)?,
        // (a record made before the chain was kept: the track's main chain)
        None => {
            let track = restore.target["trackRef"].as_str().or(restore.target["track"].as_str()).ok_or_else(|| bad("target.trackRef"))?;
            fxchain::holder_named(ctx, &json!(track), false, "the FX's track").map_err(gone_if_missing)?
        }
    };
    let chain = fxchain::guids(ctx.reaper, holder);
    let place = back_place(&chain, material);
    let at = add_entry(ctx, holder, place, entry, guid)?;
    let (lost, theirs) = relink(ctx, &at, material);
    let mut display = json!({"title": format!("{} put back {}", super::quoted(&fx_label(ctx, at.fx)), place_phrase(ctx, holder))});
    let mut notes = Vec::new();
    if material["envelopes"].as_u64().unwrap_or(0) > 0 {
        notes.push("Its automation came back as new envelopes: read them again for their refs.".to_string());
    }
    if lost > 0 {
        notes.push(format!("{lost} parameter link(s) that followed it weren't made again: their FX are gone."));
    }
    if theirs > 0 {
        notes.push(format!("{theirs} parameter link(s) that followed it were set otherwise in REAPER since: Kumi left them as they are."));
    }
    if !notes.is_empty() {
        display["note"] = notes.join(" ").into();
    }
    Ok(Restored {
        restored: json!({"ref": refs::fx_ref(&at.guid), "was": refs::fx_ref(guid), "index": place}),
        display,
        ..Default::default()
    })
}

// ----- move -----

fn move_index(input: &Value, count: usize) -> Done<usize> {
    input
        .get("index")
        .and_then(Value::as_u64)
        .map(|i| i as usize)
        .filter(|i| *i < count)
        .ok_or_else(|| bad(format!("index must be the FX's new place in its chain, from 0 to {}", count.saturating_sub(1))))
}

pub(super) fn moved(chain: &[String], guid: &str, to: usize) -> Vec<String> {
    let mut out: Vec<String> = chain.iter().filter(|g| !g.eq_ignore_ascii_case(guid)).cloned().collect();
    out.insert(to.min(out.len()), guid.to_string());
    out
}

fn move_title(ctx: &Ctx, at: &FxAt, to: usize) -> String {
    format!("Moved {} to place {} {}", super::quoted(&fx_label(ctx, at.fx)), to + 1, place_phrase(ctx, at.holder))
}

fn move_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let before = fxchain::guids(ctx.reaper, at.holder);
    let to = move_index(input, before.len())?;
    fxchain::move_address(ctx.reaper, at.holder, to)?;
    let from = fxchain::position_of(&before, &at.guid).unwrap_or(0);
    let mut display = json!({"title": move_title(ctx, &at, to), "from": from, "to": to});
    if to == from {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: chain_fence(&before), fence: chain_fence(&before), display })
}

/// Moves an FX to `to` in its own chain; whether the chain is as expected after.
fn move_within(ctx: &Ctx, at: &FxAt, to: usize, expected: &[String]) -> Done<bool> {
    let destination = fxchain::move_address(ctx.reaper, at.holder, to)?;
    if !ctx.reaper.fx_copy(at.fx, at.fx.owner, destination, true) {
        return Err(failed("REAPER wouldn't move the FX"));
    }
    Ok(fxchain::guids(ctx.reaper, at.holder) == expected)
}

fn move_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let before = fxchain::guids(ctx.reaper, at.holder);
    if chain_fence(&before) != *fence {
        return Err(changed("the FX in that chain changed since the preview"));
    }
    let to = move_index(input, before.len())?;
    let from = fxchain::position_of(&before, &at.guid).unwrap_or(0);
    let expected = moved(&before, &at.guid, to);
    if to != from && !move_within(ctx, &at, to, &expected)? {
        // Moved back to where it was, inside this call; else nobody knows how the chain stands.
        let back = fxchain::locate_guid(ctx, &at.guid).ok().is_some_and(|now| move_within(ctx, &now, from, &before).unwrap_or(false));
        let message = "the FX chain isn't in the order it should be after the move";
        return Err(if back {
            failed(format!("{message}; the FX is moved back"))
        } else {
            ("uncertain".into(), format!("{message}, and Kumi could not move the FX back: read the chain's FX"))
        });
    }
    let after = fxchain::guids(ctx.reaper, at.holder);
    Ok(Applied {
        applied: chain_fence(&after),
        created: None,
        material: None,
        display: json!({"title": move_title(ctx, &at, to)}),
        unwritten: to == from,
    })
}

pub(super) fn chain_of_value(value: &Value) -> Done<Vec<String>> {
    value["chain"]
        .as_array()
        .ok_or_else(|| bad("chain: the FX ids in order"))?
        .iter()
        .map(|g| g.as_str().map(str::to_string).ok_or_else(|| bad("chain: FX ids")))
        .collect()
}

fn target_guid(target: &Value) -> Done<String> {
    let text = target["ref"].as_str().ok_or_else(|| bad("target.ref: the FX"))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::Fx(guid) => Ok(guid),
        Ref::Param { fx, .. } => Ok(fx),
        other => Err(bad(format!("target.ref: an FX ref, not r:{}", other.kind()))),
    }
}

fn move_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = target_guid(restore.target)?;
    let at = fxchain::locate_guid(ctx, &guid).map_err(gone_if_missing)?;
    let (prior, applied) = (chain_of_value(restore.prior)?, chain_of_value(restore.applied)?);
    let now = fxchain::guids(ctx.reaper, at.holder);
    let label = fx_label(ctx, at.fx);
    if now == prior {
        return Ok(Restored {
            restored: chain_fence(&now),
            display: json!({"title": format!("{} is where it was", super::quoted(&label))}),
            unwritten: true,
        });
    }
    if !restore.discard && now != applied {
        return Err(changed("the FX in that chain have been rearranged in REAPER since Kumi moved one (or it is in another chain now)"));
    }
    let to = fxchain::position_of(&prior, &guid).unwrap_or(0).min(now.len().saturating_sub(1));
    let expected = moved(&now, &guid, to);
    if !move_within(ctx, &at, to, &expected)? {
        return Err(failed("REAPER wouldn't put the FX back"));
    }
    Ok(Restored {
        restored: chain_fence(&expected),
        display: json!({"title": format!("{} back to place {} {}", super::quoted(&label), to + 1, place_phrase(ctx, at.holder))}),
        ..Default::default()
    })
}

// ----- bypass -----

fn bypass_input(input: &Value) -> Done<bool> {
    input.get("bypass").and_then(Value::as_bool).ok_or_else(|| bad("bypass must be true or false"))
}

fn bypass_title(ctx: &Ctx, at: &FxAt, bypass: bool) -> String {
    format!("{} {} {}", if bypass { "Bypassed" } else { "Enabled" }, super::quoted(&fx_label(ctx, at.fx)), place_phrase(ctx, at.holder))
}

fn bypass_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let bypass = bypass_input(input)?;
    let now = json!({"enabled": ctx.reaper.fx_enabled(at.fx)});
    let mut display = json!({"title": bypass_title(ctx, &at, bypass)});
    if now["enabled"] == !bypass {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: now.clone(), fence: now, display })
}

fn bypass_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let bypass = bypass_input(input)?;
    if json!({"enabled": ctx.reaper.fx_enabled(at.fx)}) != *fence {
        return Err(changed("the FX was switched in REAPER since the preview"));
    }
    let already = ctx.reaper.fx_enabled(at.fx) != bypass;
    if !already {
        let _quiet = QuietAutomation::around(ctx.reaper, at.holder.track(ctx.reaper));
        if !ctx.reaper.fx_set_enabled(at.fx, !bypass) {
            return Err(failed("REAPER wouldn't switch the FX"));
        }
    }
    if ctx.reaper.fx_enabled(at.fx) == bypass {
        return Err(failed("REAPER kept the FX as it was"));
    }
    Ok(Applied {
        applied: json!({"enabled": !bypass}),
        created: None,
        material: None,
        display: json!({"title": bypass_title(ctx, &at, bypass)}),
        unwritten: already,
    })
}

fn bypass_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let (prior, applied) = (restore.prior["enabled"] == true, restore.applied["enabled"] == true);
    let now = ctx.reaper.fx_enabled(at.fx);
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed("the FX has been switched in REAPER since Kumi did"));
        }
        let _quiet = QuietAutomation::around(ctx.reaper, at.holder.track(ctx.reaper));
        if !ctx.reaper.fx_set_enabled(at.fx, prior) || ctx.reaper.fx_enabled(at.fx) != prior {
            return Err(failed("REAPER wouldn't switch the FX back"));
        }
    }
    Ok(Restored { restored: json!({"enabled": prior}), display: json!({"title": bypass_title(ctx, &at, !prior)}), unwritten: now == prior })
}

/// The FX a restore's target names; one that is gone is `gone`.
pub(super) fn target_fx(ctx: &Ctx, target: &Value) -> Done<FxAt> {
    let guid = target_guid(target)?;
    fxchain::locate_guid(ctx, &guid).map_err(gone_if_missing)
}

// ----- a parameter -----

pub(super) struct ParamAt {
    pub at: FxAt,
    pub param: i32,
}

pub(super) fn param_index(ctx: &Ctx, at: &FxAt, which: Option<&Value>) -> Done<i32> {
    let count = ctx.reaper.fx_param_count(at.fx);
    match which {
        Some(Value::Number(n)) => n
            .as_i64()
            .filter(|i| (0..count as i64).contains(i))
            .map(|i| i as i32)
            .ok_or_else(|| bad(format!("param must be a parameter number from 0 to {}", count - 1))),
        Some(Value::String(name)) => {
            let found: Vec<i32> = (0..count).filter(|&i| ctx.reaper.param_name(at.fx, i).eq_ignore_ascii_case(name.trim())).collect();
            match found.as_slice() {
                [one] => Ok(*one),
                [] => Err(bad(format!("that FX has no parameter named \"{name}\": read its parameters"))),
                _ => Err(bad(format!("several parameters are named \"{name}\": use the param ref"))),
            }
        }
        _ => Err(bad("pass the param ref as ref, or the FX ref as ref and param (its number or name)")),
    }
}

pub(super) fn locate_param(ctx: &Ctx, input: &Value) -> Done<ParamAt> {
    let text = input.get("ref").and_then(Value::as_str).ok_or_else(|| bad("ref must be a param ref"))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::Param { fx, index } => {
            let at = fxchain::locate_guid(ctx, &fx)?;
            let count = ctx.reaper.fx_param_count(at.fx);
            if index < 0 || index >= count {
                return Err(bad(format!("that FX has {count} parameters")));
            }
            Ok(ParamAt { at, param: index })
        }
        Ref::Fx(guid) => {
            let at = fxchain::locate_guid(ctx, &guid)?;
            let param = param_index(ctx, &at, input.get("param"))?;
            Ok(ParamAt { at, param })
        }
        other => Err(bad(format!("ref must be a param ref, not r:{}", other.kind()))),
    }
}

/// What the plug-in prints for a value, with its unit when it prints one: the formatted value of a parameter leaves the unit
/// off in some plug-ins (ReaEQ's "2000.0"), where asking the plug-in to print the value keeps it.
pub(super) fn shown(ctx: &Ctx, at: &ParamAt, normalized: f64) -> String {
    let printed = ctx.reaper.fx_format(at.at.fx, at.param, normalized).filter(|text| !text.trim().is_empty());
    printed.unwrap_or_else(|| ctx.reaper.param_formatted(at.at.fx, at.param))
}

pub(super) fn param_state(ctx: &Ctx, at: &ParamAt) -> Value {
    let normalized = ctx.reaper.param_normalized(at.at.fx, at.param);
    json!({"normalized": normalized, "display": shown(ctx, at, normalized)})
}

pub(super) fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6
}

fn same_param(a: &Value, b: &Value) -> bool {
    near(a["normalized"].as_f64().unwrap_or(f64::NAN), b["normalized"].as_f64().unwrap_or(f64::NAN))
}

/// The first number in what a plug-in prints for a value: `-3.0 dB`, `250 Hz`, `100 %`.
pub(super) fn reading(text: &str) -> Option<f64> {
    let text = text.replace('\u{2212}', "-");
    let chars: Vec<char> = text.chars().collect();
    let start = (0..chars.len()).find(|&i| {
        chars[i].is_ascii_digit()
            || (matches!(chars[i], '-' | '+' | '.') && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == '.'))
    })?;
    let mut end = start + 1;
    let mut seen_dot = chars[start] == '.';
    while end < chars.len() && (chars[end].is_ascii_digit() || (chars[end] == '.' && !seen_dot)) {
        seen_dot |= chars[end] == '.';
        end += 1;
    }
    chars[start..end].iter().collect::<String>().parse().ok()
}

/// How many decimals the producer typed: the precision a reading must match to.
pub(super) fn decimals(text: &str) -> usize {
    text.split_once('.').map_or(0, |(_, rest)| rest.chars().take_while(char::is_ascii_digit).count())
}

/// The value in `0..=1` where `ask` prints `wanted`: found by asking a printer, never by setting anything (bisection over
/// the range for numbers, a scan for words).
pub(super) fn find_printed(ask: &dyn Fn(f64) -> Option<String>, wanted: &str) -> Done<f64> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return Err(bad("value must say what the plug-in should show"));
    }
    if let Some(target) =
        reading(wanted).filter(|_| wanted.chars().next().is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | '\u{2212}')))
    {
        let at_ends = ask(0.0).as_deref().and_then(reading).zip(ask(1.0).as_deref().and_then(reading));
        if let Some((low, high)) = at_ends {
            let rising = high >= low;
            let tolerance = 0.5 * 10f64.powi(-(decimals(wanted) as i32));
            if target < low.min(high) - tolerance || target > low.max(high) + tolerance {
                let (a, b) = (ask(0.0).unwrap_or_default(), ask(1.0).unwrap_or_default());
                return Err(failed(format!("that parameter runs from {a} to {b}: {wanted} is outside it")));
            }
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..60 {
                let mid = (lo + hi) / 2.0;
                let Some(read) = ask(mid).as_deref().and_then(reading) else { break };
                if (read < target) == rising {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let best = [lo, hi, (lo + hi) / 2.0]
                .into_iter()
                .filter_map(|v| ask(v).as_deref().and_then(reading).map(|r| (v, r)))
                .min_by(|a, b| (a.1 - target).abs().total_cmp(&(b.1 - target).abs()));
            if let Some((value, read)) = best {
                if (read - target).abs() <= tolerance {
                    return Ok(centre_of(ask, value, read));
                }
                return Err(failed(format!(
                    "no value of that parameter reads {wanted}; the closest reads {}",
                    ask(value).unwrap_or_default()
                )));
            }
        }
    }
    // Words and steps: the middle of the first run of values that print exactly this.
    let matches: Vec<usize> =
        (0..=1000).filter(|&n| ask(n as f64 / 1000.0).is_some_and(|text| text.trim().eq_ignore_ascii_case(wanted))).collect();
    let Some(&first) = matches.first() else {
        return Err(failed(format!("no value of that parameter reads \"{wanted}\": read the parameter to see what it shows")));
    };
    let run = matches.iter().zip(first..).take_while(|(n, expected)| **n == *expected).count();
    Ok((first + run / 2) as f64 / 1000.0)
}

/// The middle of the values around `value` that print the same number as it: a plug-in prints a rounded number, and the
/// value the number stands for is the middle of the values that round to it (the edge the bisection stops at is up to half
/// a printed step off).
fn centre_of(ask: &dyn Fn(f64) -> Option<String>, value: f64, read: f64) -> f64 {
    let same = |v: f64| ask(v).as_deref().and_then(reading).is_some_and(|r| r == read);
    let edge = |far: f64| {
        if same(far) {
            return far;
        }
        let (mut out, mut inside) = (far, value);
        for _ in 0..50 {
            let mid = (out + inside) / 2.0;
            if same(mid) {
                inside = mid;
            } else {
                out = mid;
            }
        }
        inside
    };
    let centre = (edge(0.0) + edge(1.0)) / 2.0;
    if same(centre) {
        centre
    } else {
        value
    }
}

/// The normalized value a plug-in prints as `wanted`: found by asking it to print values, never by setting them.
fn find_value(ctx: &Ctx, at: &ParamAt, wanted: &str) -> Done<f64> {
    let ask = |value: f64| ctx.reaper.fx_format(at.at.fx, at.param, value);
    find_printed(&ask, wanted)
}

/// What a parameter change asks for: a normalized value, or (a JSFX slider in its own units) a raw one.
pub(super) enum Wanted {
    Normalized(f64),
    Raw(f64),
}

/// A value in the parameter's own units: a ReaEQ band's frequency (Hz), gain (dB) or bandwidth (octaves), found by what
/// the plug-in prints (REAPER's `TrackFX_SetEQParam` is not used: after it, REAPER ignores normalized writes to that
/// parameter, which would break every later undo); a JSFX slider's own value, written as the slider takes it.
fn units_value(ctx: &Ctx, at: &ParamAt, units: f64, typed: &str) -> Done<Wanted> {
    let fx = at.at.fx;
    if ctx.reaper.fx_eq_param(fx, at.param).is_some_and(|eq| (0..=2).contains(&eq.kind) && eq.band_type >= 0) || take_reaeq_band(ctx, at) {
        return find_value(ctx, at, typed).map(Wanted::Normalized);
    }
    let kind = ctx.reaper.fx_config(fx, "fx_type").unwrap_or_default();
    if kind.trim().eq_ignore_ascii_case("JS") {
        let (_, min, max) = ctx.reaper.fx_param_raw(fx, at.param).ok_or_else(|| failed("REAPER doesn't say the slider's range"))?;
        let (low, high) = (min.min(max), min.max(max));
        if units < low - 1e-9 || units > high + 1e-9 {
            return Err(failed(format!("that slider runs from {low} to {high}: {typed} is outside it")));
        }
        return Ok(Wanted::Raw(units));
    }
    Err(bad(format!(
        "{} has no units of its own Kumi can set: give value, the text it shows for what you want (such as \"-3 dB\")",
        fx_label(ctx, fx)
    )))
}

/// A ReaEQ band's frequency, gain or bandwidth on a take, where REAPER gives no `GetEQParam` (lab 7.82): told by the
/// plug-in (its original name, whatever the producer renamed it) and the parameter's name ("Freq-Band 2", "Gain-Low
/// Shelf", "BW-High Pass").
fn take_reaeq_band(ctx: &Ctx, at: &ParamAt) -> bool {
    let fx = at.at.fx;
    let original = ctx.reaper.fx_config(fx, "original_name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| ctx.reaper.fx_name(fx));
    matches!(fx.owner, FxOwner::Take(_))
        && short_name(&original) == "ReaEQ"
        && ["Freq-", "Gain-", "BW-"].iter().any(|prefix| ctx.reaper.param_name(fx, at.param).starts_with(prefix))
}

pub(super) fn wanted_value(ctx: &Ctx, at: &ParamAt, input: &Value) -> Done<Wanted> {
    let normalized = input.get("normalized").filter(|v| !v.is_null());
    let value = input.get("value").filter(|v| !v.is_null());
    let units = input.get("units").filter(|v| !v.is_null());
    match (normalized, value, units) {
        (Some(n), None, None) => n
            .as_f64()
            .filter(|n| (0.0..=1.0).contains(n))
            .map(Wanted::Normalized)
            .ok_or_else(|| bad("normalized must be a number from 0 to 1")),
        (None, Some(Value::String(text)), None) => find_value(ctx, at, text).map(Wanted::Normalized),
        (None, Some(_), None) => Err(bad("value must be text: what the plug-in shows, such as \"-3 dB\"")),
        (None, None, Some(Value::Number(n))) => {
            let typed = n.to_string();
            units_value(ctx, at, n.as_f64().unwrap_or(f64::NAN), &typed)
        }
        (None, None, Some(_)) => Err(bad("units must be a number in the parameter's own units")),
        _ => Err(bad("pass one of normalized (0 to 1), value (what the plug-in shows) or units (its own units)")),
    }
}

/// The normalized value a wanted value is, without writing it: a raw one is printed by its slider's own scale.
pub(super) fn wanted_normalized(ctx: &Ctx, at: &ParamAt, wanted: &Wanted) -> f64 {
    match *wanted {
        Wanted::Normalized(n) => n,
        Wanted::Raw(raw) => match ctx.reaper.fx_param_raw(at.at.fx, at.param) {
            Some((_, min, max)) if max != min => ((raw - min) / (max - min)).clamp(0.0, 1.0),
            _ => 0.0,
        },
    }
}

fn param_title(ctx: &Ctx, at: &ParamAt, display: &str) -> String {
    format!(
        "{} {} {} {}",
        fx_label(ctx, at.at.fx),
        rows::clip(&ctx.reaper.param_name(at.at.fx, at.param)),
        display,
        place_phrase(ctx, at.at.holder)
    )
}

pub(super) fn param_target(ctx: &Ctx, at: &ParamAt) -> Value {
    let mut target = fx_target(ctx, &at.at);
    target["ref"] = refs::param_ref(&at.at.guid, at.param).into();
    target["kind"] = "param".into();
    target["fxRef"] = refs::fx_ref(&at.at.guid).into();
    target["param"] = at.param.into();
    target
}

fn param_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = locate_param(ctx, input)?;
    let wanted = wanted_value(ctx, &at, input)?;
    let now = param_state(ctx, &at);
    let normalized = wanted_normalized(ctx, &at, &wanted);
    let after = shown(ctx, &at, normalized);
    let mut display = json!({"title": param_title(ctx, &at, &after), "before": now["display"], "after": after, "normalized": normalized});
    if near(normalized, now["normalized"].as_f64().unwrap_or(f64::NAN)) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: param_target(ctx, &at), prior: now.clone(), fence: now, display })
}

fn param_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = locate_param(ctx, input).map_err(went)?;
    let wanted = wanted_value(ctx, &at, input)?;
    let now = param_state(ctx, &at);
    if !same_param(&now, fence) {
        return Err(changed("the parameter changed since the preview"));
    }
    let before = now["normalized"].as_f64().unwrap_or(f64::NAN);
    let target = wanted_normalized(ctx, &at, &wanted);
    let unwritten = match wanted {
        Wanted::Normalized(n) => near(n, before),
        Wanted::Raw(raw) => ctx.reaper.fx_param_raw(at.at.fx, at.param).is_some_and(|(value, _, _)| (value - raw).abs() <= 1e-9),
    };
    if !unwritten {
        let _quiet = QuietAutomation::around(ctx.reaper, at.at.holder.track(ctx.reaper));
        let wrote = match wanted {
            Wanted::Normalized(n) => ctx.reaper.fx_set_param(at.at.fx, at.param, n),
            Wanted::Raw(raw) => ctx.reaper.fx_set_param_raw(at.at.fx, at.param, raw),
        };
        if !wrote {
            return Err(failed("REAPER wouldn't set the parameter"));
        }
    }
    let after = param_state(ctx, &at);
    let read = after["normalized"].as_f64().unwrap_or(f64::NAN);
    if !unwritten && (read - target).abs() > 1e-3 && near(read, before) {
        return Err(failed("the plug-in kept the parameter as it was"));
    }
    let title = param_title(ctx, &at, after["display"].as_str().unwrap_or_default());
    // (for HISTORY's picture only: the parameter's name, and where it was and is)
    let moved = json!({
        "name": rows::clip(&ctx.reaper.param_name(at.at.fx, at.param)),
        "from": before,
        "to": read,
        "min": 0.0,
        "max": 1.0,
        "fromText": now["display"],
        "toText": after["display"],
    });
    let display = json!({"title": title, "values": [moved]});
    Ok(Applied { applied: after, created: None, material: None, display, unwritten })
}

fn param_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let param = restore.target["param"].as_i64().ok_or_else(|| bad("target.param"))? as i32;
    let at = ParamAt { at, param };
    let now = param_state(ctx, &at);
    let already = same_param(&now, restore.prior);
    if !already {
        if !restore.discard && !same_param(&now, restore.applied) {
            return Err(changed("the parameter has been changed in REAPER since Kumi set it"));
        }
        let prior = restore.prior["normalized"].as_f64().ok_or_else(|| bad("prior.normalized"))?;
        let _quiet = QuietAutomation::around(ctx.reaper, at.at.holder.track(ctx.reaper));
        if !ctx.reaper.fx_set_param(at.at.fx, at.param, prior) {
            return Err(failed("REAPER wouldn't put the parameter back"));
        }
    }
    let after = param_state(ctx, &at);
    if (after["normalized"].as_f64().unwrap_or(f64::NAN) - restore.prior["normalized"].as_f64().unwrap_or(f64::NAN)).abs() > 1e-3 {
        return Err(failed("the plug-in kept the parameter as it was"));
    }
    let title = param_title(ctx, &at, after["display"].as_str().unwrap_or_default());
    Ok(Restored { restored: after, display: json!({"title": format!("{title} (put back)")}), unwritten: already })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plug_in_is_named_without_its_format_and_maker() {
        assert_eq!(short_name("VST3: ReaEQ (Cockos)"), "ReaEQ");
        assert_eq!(short_name("JS: utility/volume"), "utility/volume");
        assert_eq!(short_name("CLAP: Vital (Vital Audio)"), "Vital");
        assert_eq!(short_name("Pro-Q 3"), "Pro-Q 3");
        assert_eq!(short_name("AU: Kontakt 7 (Native Instruments)"), "Kontakt 7");
    }

    #[test]
    fn what_a_plug_in_prints_is_read_as_a_number() {
        assert_eq!(reading("-3.0 dB"), Some(-3.0));
        assert_eq!(reading("\u{2212}6.5 dB"), Some(-6.5));
        assert_eq!(reading("250 Hz"), Some(250.0));
        assert_eq!(reading("100 %"), Some(100.0));
        assert_eq!(reading("+12.00 st"), Some(12.0));
        assert_eq!(reading("Mid"), None);
        assert_eq!(reading("1.5 s"), Some(1.5));
        assert_eq!((decimals("-3.25 dB"), decimals("250 Hz"), decimals("1.5")), (2, 0, 1));
    }

    #[test]
    fn an_fx_is_found_in_a_chunk_by_its_id_whatever_nests_in_its_entry() {
        let track = false;
        let chunk = "<TRACK\nNAME x\n<FXCHAIN\nSHOW 0\nBYPASS 0 0 0\n<JS a \"\"\n0.1 - -\n>\nFLOATPOS 0 0 0 0\nFXID {AAA}\n<PROGRAMENV 0 0\nLFO 1\n>\nWAK 0 0\nPARALLEL 1\nBYPASS 1 0 0\n<CONTAINER Container 0 \"\" 0\nBYPASS 0 0 0\n<VST \"in\" x.vst 0 \"\" 1\nQUJD\n>\nFXID {CHILD}\nWAK 0 0\n>\nFXID {BBB}\nWAK 0 0\nBYPASS 0 0 0\n<VST \"c\" c.vst 0 \"\" 2\nREVG\n>\nPRESETNAME \"p\"\nFXID {CCC}\nWAK 0 0\n>\n<FXCHAIN_REC\nBYPASS 0 0 0\n<VST \"r\" r.vst 0 \"\" 3\n>\nFXID {REC}\nWAK 0 0\n>\n<ITEM\n<TAKEFX\nBYPASS 0 0 0\nFXID {NOTME}\nWAK 0 0\n>\n>\n>";
        let a = fxchain::fx_entry(chunk, track, "{AAA}").unwrap();
        assert!(a.starts_with("BYPASS 0 0 0\n<JS a") && a.contains("<PROGRAMENV") && a.ends_with("WAK 0 0\nPARALLEL 1"), "{a}");
        let b = fxchain::fx_entry(chunk, track, "{BBB}").unwrap();
        assert!(b.starts_with("BYPASS 1 0 0\n<CONTAINER") && b.contains("{CHILD}") && b.ends_with("FXID {BBB}\nWAK 0 0"), "{b}");
        let child = fxchain::fx_entry(chunk, track, "{CHILD}").unwrap();
        assert!(child.starts_with("BYPASS 0 0 0\n<VST \"in\"") && child.ends_with("FXID {CHILD}\nWAK 0 0"), "{child}");
        let c = fxchain::fx_entry(chunk, track, "{CCC}").unwrap();
        assert!(c.contains("PRESETNAME \"p\"") && !c.contains("{BBB}"), "{c}");
        assert!(fxchain::fx_entry(chunk, track, "{REC}").unwrap().contains("FXID {REC}"), "the input chain too");
        assert_eq!(fxchain::fx_entry(chunk, track, "{NOTME}"), None, "not a take's FX inside the track's items");
        assert_eq!(fxchain::fx_entry("<TRACK\n>", track, "{AAA}"), None);
        assert_eq!(fxchain::ids_in(&b), ["{CHILD}", "{BBB}"]);
        assert!(!fxchain::without_ids(&b).contains("FXID"));
    }
}
