//! Containers (REAPER 7): a container added, an FX moved into or out of one (or between two), an FX run in parallel with
//! the one before it, and a container's macros (a child's parameter mapped as one of the container's own).
//!
//! REAPER's copy into a container hung REAPER in the lab, so an FX goes into a container as its own chunk entry (an FX
//! chain file at the container's address) right after it is taken out of where it was: REAPER keeps the FX's id when no
//! other FX has it (measured), and its state, modulation and automation come with it. Out of a container REAPER's own
//! move is used, which keeps the id.

use serde_json::{json, Value};

use super::fx::{self, fx_label, fx_target, place_phrase, signature, target_fx};
use super::fxchain::{self, changed, gone, gone_if_missing, mark_fx, went, FxAt, Holder};
use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Fx, FxOwner};
use crate::refs::{self, Ref};

fn label(ctx: &Ctx, at: &FxAt) -> String {
    format!("{} {}", super::quoted(&fx_label(ctx, at.fx)), place_phrase(ctx, at.holder))
}

// ----- a container added -----

fn container_name(input: &Value) -> Done<Option<String>> {
    match input.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => {
            Ok(Some(name.trim().to_string()).filter(|n| !n.is_empty()))
        }
        Some(_) => Err(bad("name must be one line of at most 256 characters")),
    }
}

fn add_title(ctx: &Ctx, holder: Holder, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("Added the container {} to {}", super::quoted(name), fxchain::holder_label(ctx, holder)),
        None => format!("Added a container to {}", fxchain::holder_label(ctx, holder)),
    }
}

pub fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let holder = fx::add_holder(ctx, input, "fx-container-add")?;
    let name = container_name(input)?;
    let before = fxchain::guids(ctx.reaper, holder);
    let place = fx::add_position(input, before.len())?.unwrap_or(before.len());
    fxchain::add_place(ctx.reaper, holder, place)?;
    let fence = json!({"chain": before});
    Ok(Preview {
        target: fxchain::holder_target(ctx, holder),
        prior: fence.clone(),
        fence,
        display: json!({"title": add_title(ctx, holder, name.as_deref()), "position": place}),
    })
}

pub fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let holder = fx::add_holder(ctx, input, "fx-container-add").map_err(went)?;
    let name = container_name(input)?;
    let before = fxchain::guids(ctx.reaper, holder);
    if json!({"chain": before}) != *fence {
        return Err(changed("the FX in that chain changed since the preview"));
    }
    let place = fx::add_position(input, before.len())?.unwrap_or(before.len());
    let (address, rec) = fxchain::add_place(ctx.reaper, holder, place)?;
    ctx.reaper.fx_add(holder.owner(), "Container", rec, Some(address));
    let after = fxchain::guids(ctx.reaper, holder);
    let guid = after.get(place).cloned().unwrap_or_default();
    if after.len() != before.len() + 1 || guid.is_empty() || fxchain::position_of(&before, &guid).is_some() {
        return Err(fx::take_back_added(ctx, holder, &before, "the chain isn't what it should be after adding a container"));
    }
    let at = fxchain::adopt(ctx, holder, &guid)?;
    if !fxchain::is_container(ctx.reaper, at.fx) {
        return Err(fx::take_back_added(ctx, holder, &before, "REAPER added something other than a container"));
    }
    if let Some(name) = &name {
        ctx.reaper.fx_set_config(at.fx, "renamed_name", name);
    }
    Ok(Applied {
        applied: json!({"guid": guid, "index": place, "signature": signature(ctx, at.fx)}),
        created: Some(json!({"ref": refs::fx_ref(&guid), "kind": "fx"})),
        display: json!({"title": add_title(ctx, holder, name.as_deref()), "ref": refs::fx_ref(&guid)}),
        ..Default::default()
    })
}

pub fn add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    // The same as taking off an FX Kumi added: refused when the producer put FX in it or automated it.
    fx::restore(ctx, restore, fx::Which::Add)
}

// ----- an FX moved into, out of, or between containers -----

/// Whether `fx` sits (at any depth) inside `container`.
fn inside(ctx: &Ctx, holder: Holder, fx: Fx) -> bool {
    let mut holder = holder;
    loop {
        match holder {
            Holder::Container(parent) if parent == fx => return true,
            Holder::Container(parent) => holder = fxchain::holder_of(ctx.reaper, parent),
            _ => return false,
        }
    }
}

/// What a move out of the FX's chain would break, said before anything is written: REAPER clears a parameter link to an
/// FX that is taken out and put back (measured), and a child's macros go when it leaves its container.
fn move_blockers(ctx: &Ctx, at: &FxAt) -> Option<String> {
    let reaper = ctx.reaper;
    if let Holder::Container(container) = at.holder {
        let place = reaper.fx_children(container).iter().position(|c| *c == at.fx)?;
        let macros = (0..reaper.fx_param_count(container))
            .filter(|&p| {
                reaper.fx_config(container, &format!("param.{p}.container_map.fx_index")).and_then(|v| v.trim().parse::<usize>().ok())
                    == Some(place)
            })
            .count();
        if macros > 0 {
            return Some(format!(
                "{macros} of its parameters are macros of {}: they would go with the move (fx_container_unmap first)",
                super::quoted(&fx_label(ctx, container))
            ));
        }
    }
    let linked = |fx: Fx| {
        (0..reaper.fx_param_count(fx).min(4096))
            .any(|p| reaper.fx_config(fx, &format!("param.{p}.plink.active")).is_some_and(|v| v.trim() == "1"))
    };
    if linked(at.fx) {
        return Some("its parameters are linked to other FX: a move to another chain would break the links".into());
    }
    let chain = fxchain::members(reaper, at.holder);
    let place = chain.iter().position(|f| *f == at.fx)?;
    for other in chain.iter().filter(|f| **f != at.fx) {
        for p in 0..reaper.fx_param_count(*other).min(4096) {
            let active = reaper.fx_config(*other, &format!("param.{p}.plink.active")).is_some_and(|v| v.trim() == "1");
            let effect = reaper.fx_config(*other, &format!("param.{p}.plink.effect")).and_then(|v| v.trim().parse::<i64>().ok());
            if active && effect == Some(place as i64) {
                return Some(format!(
                    "{}'s parameters are linked to it: the move would break that link",
                    super::quoted(&fx_label(ctx, *other))
                ));
            }
        }
    }
    None
}

fn move_title(ctx: &Ctx, at: &FxAt, to: Holder, place: usize) -> String {
    match to {
        Holder::Container(_) => format!("Moved {} into {} at place {}", label(ctx, at), fxchain::holder_label(ctx, to), place + 1),
        _ => format!("Moved {} out to {} at place {}", label(ctx, at), fxchain::holder_label(ctx, to), place + 1),
    }
}

fn move_destination(ctx: &Ctx, at: &FxAt, input: &Value) -> Done<Holder> {
    let input_chain = match input.get("input") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("input must be true or false")),
    };
    let to = fxchain::holder_named(ctx, input.get("toRef").unwrap_or(&Value::Null), input_chain, "toRef")?;
    if to.owner() != at.fx.owner {
        return Err(bad("an FX moves between the chains of its own track or take: fx_copy copies it elsewhere"));
    }
    if to == at.holder {
        return Err(bad("the FX is in that chain already: fx_move moves it within its chain"));
    }
    if let Holder::Container(container) = to {
        if container == at.fx || inside(ctx, to, at.fx) {
            return Err(bad("a container can't go inside itself"));
        }
    }
    if !matches!(to, Holder::Container(_)) && !matches!(at.holder, Holder::Container(_)) {
        return Err(bad("fx_container_move moves an FX into or out of a container"));
    }
    Ok(to)
}

pub fn move_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let to = move_destination(ctx, &at, input)?;
    if let Some(why) = move_blockers(ctx, &at) {
        return Err(bad(format!("Kumi won't move it: {why}")));
    }
    let to_chain = fxchain::guids(ctx.reaper, to);
    let place = fx::add_position(input, to_chain.len())?.unwrap_or(to_chain.len());
    fxchain::add_place(ctx.reaper, to, place)?;
    let from_chain = fxchain::guids(ctx.reaper, at.holder);
    let index = fxchain::position_of(&from_chain, &at.guid).unwrap_or(0);
    let prior = json!({
        "from": fxchain::holder_value(ctx, at.holder),
        "index": index,
        "before": index.checked_sub(1).and_then(|i| from_chain.get(i)),
        "after": from_chain.get(index + 1),
    });
    let fence = json!({"fromChain": from_chain, "toChain": to_chain, "signature": signature(ctx, at.fx)});
    let mut display = json!({"title": move_title(ctx, &at, to, place)});
    if !fxchain::fx_envelopes(ctx.reaper, &at).is_empty() && matches!(to, Holder::Container(_)) {
        display["note"] = "Its automation moves with it, as new envelopes (their refs change).".into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior, fence, display })
}

/// Moves an FX to `place` of another chain of its owner (named by `to`, as a record names a chain: by ids, which a move
/// doesn't change): into a container by its entry (taken out first, so REAPER keeps its id), out of one by REAPER's move.
/// The FX where it is now.
fn move_between(ctx: &Ctx, at: &FxAt, to: &Value, place: usize) -> Done<FxAt> {
    let reaper = ctx.reaper;
    match fxchain::holder_from(ctx, to)? {
        Holder::Container(_) => {
            let chunk = fxchain::owner_chunk(reaper, at.fx.owner)?;
            let entry = fxchain::fx_entry(&chunk, matches!(at.fx.owner, FxOwner::Take(_)), &at.guid)
                .ok_or_else(|| failed("the FX isn't in its owner's state (a plug-in REAPER can't describe)"))?;
            let from = fxchain::holder_value(ctx, at.holder);
            let index = fxchain::position_of(&fxchain::guids(reaper, at.holder), &at.guid).unwrap_or(0);
            if !reaper.fx_delete(at.fx) {
                return Err(failed("REAPER wouldn't take the FX out of its chain"));
            }
            // (the container's address is worked out again after the FX left: an FX before it moves every address)
            let added = fxchain::holder_from(ctx, to).and_then(|to| fx::add_entry(ctx, to, place, &entry, &at.guid));
            match added {
                Ok(made) => Ok(made),
                Err(fault) => {
                    // Put back where it was, from the same entry: nothing is lost.
                    let back = fxchain::holder_from(ctx, &from).and_then(|from| {
                        let count = fxchain::guids(reaper, from).len();
                        fx::add_entry(ctx, from, index.min(count), &entry, &at.guid)
                    });
                    Err(match back {
                        Ok(_) => failed(format!("{}; the FX is back where it was", fault.1)),
                        Err(_) => ("uncertain".into(), format!("{}, and Kumi could not put the FX back: read the chain", fault.1)),
                    })
                }
            }
        }
        holder => {
            let destination = fxchain::move_address(reaper, holder, place)?;
            let before = fxchain::guids(reaper, holder);
            if !reaper.fx_copy(at.fx, holder.owner(), destination, true) {
                return Err(failed("REAPER wouldn't move the FX"));
            }
            let after = fxchain::guids(reaper, holder);
            if after.len() != before.len() + 1 || after.get(place).map(String::as_str) != Some(at.guid.as_str()) {
                return Err(("uncertain".into(), "REAPER moved the FX somewhere other than asked: read the chains".into()));
            }
            fxchain::adopt(ctx, holder, &at.guid)
        }
    }
}

pub fn move_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let to = move_destination(ctx, &at, input).map_err(went)?;
    if let Some(why) = move_blockers(ctx, &at) {
        return Err(changed(format!("Kumi won't move it now: {why}")));
    }
    let (from_chain, to_chain) = (fxchain::guids(ctx.reaper, at.holder), fxchain::guids(ctx.reaper, to));
    let now = json!({"fromChain": from_chain, "toChain": to_chain, "signature": fx::signature_as(ctx, at.fx, &fence["signature"])});
    if now != *fence {
        return Err(changed("the FX, or the chains it moves between, changed since the preview"));
    }
    let place = fx::add_position(input, to_chain.len())?.unwrap_or(to_chain.len());
    // Named by the container's id, which a move out of the FX's chain doesn't change.
    let to_value = fxchain::holder_value(ctx, to);
    let title = move_title(ctx, &at, to, place);
    let made = move_between(ctx, &at, &to_value, place)?;
    let to = fxchain::holder_from(ctx, &to_value)?;
    Ok(Applied {
        applied: json!({"guid": made.guid, "toChain": fxchain::guids(ctx.reaper, to), "to": to_value}),
        display: json!({"title": title, "ref": refs::fx_ref(&made.guid)}),
        ..Default::default()
    })
}

pub fn move_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = restore.applied["guid"].as_str().ok_or_else(|| bad("applied.guid"))?;
    let at = fxchain::locate_guid(ctx, guid).map_err(gone_if_missing)?;
    let from = fxchain::holder_from(ctx, &restore.prior["from"])?;
    if at.holder == from {
        return Ok(Restored {
            restored: json!({"ref": refs::fx_ref(guid)}),
            display: json!({"title": format!("{} is back where it was", label(ctx, &at))}),
            unwritten: true,
        });
    }
    let applied_chain = fx::chain_of_value(&json!({"chain": restore.applied["toChain"]}))?;
    if !restore.discard && fxchain::guids(ctx.reaper, at.holder) != applied_chain {
        return Err(changed("the chain Kumi moved the FX into has been changed in REAPER since (or the FX moved again)"));
    }
    if let Some(why) = move_blockers(ctx, &at) {
        return Err(changed(format!("Kumi won't move it back: {why}")));
    }
    let place = fx::back_place(&fxchain::guids(ctx.reaper, from), restore.prior);
    let from_value = restore.prior["from"].clone();
    let made = move_between(ctx, &at, &from_value, place)?;
    let from = fxchain::holder_from(ctx, &from_value)?;
    Ok(Restored {
        restored: json!({"ref": refs::fx_ref(&made.guid), "was": refs::fx_ref(guid)}),
        display: json!({"title": format!("{} back {} at place {}", super::quoted(&fx_label(ctx, made.fx)), place_phrase(ctx, from), place + 1)}),
        ..Default::default()
    })
}

// ----- parallel -----

const PARALLEL: [(&str, i64, &str); 3] = [
    ("off", 0, "in series"),
    ("parallel", 1, "in parallel with the FX before it"),
    ("parallel-midi", 2, "in parallel with the FX before it, MIDI merged"),
];

fn parallel_input(input: &Value) -> Done<i64> {
    let text = input.get("parallel").and_then(Value::as_str).ok_or_else(|| bad("parallel must be off, parallel or parallel-midi"))?;
    PARALLEL
        .iter()
        .find(|(key, _, _)| *key == text)
        .map(|(_, code, _)| *code)
        .ok_or_else(|| bad("parallel must be off, parallel or parallel-midi"))
}

fn parallel_now(ctx: &Ctx, fx: Fx) -> i64 {
    ctx.reaper.fx_config(fx, "parallel").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0)
}

fn parallel_title(ctx: &Ctx, at: &FxAt, code: i64) -> String {
    let words = PARALLEL.iter().find(|(_, c, _)| *c == code).map_or("as REAPER has it", |(_, _, words)| words);
    format!("{} runs {words}", label(ctx, at))
}

pub fn parallel_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = fxchain::locate(ctx, &input["ref"])?;
    let wanted = parallel_input(input)?;
    let now = json!({"parallel": parallel_now(ctx, at.fx)});
    let mut display = json!({"title": parallel_title(ctx, &at, wanted)});
    if now["parallel"] == wanted {
        display["unchanged"] = true.into();
    }
    if wanted != 0 && fxchain::position_of(&fxchain::guids(ctx.reaper, at.holder), &at.guid) == Some(0) {
        display["note"] = "It is first in its chain, with no FX before it to run alongside.".into();
    }
    Ok(Preview { target: fx_target(ctx, &at), prior: now.clone(), fence: now, display })
}

pub fn parallel_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = fxchain::locate(ctx, &input["ref"]).map_err(went)?;
    let wanted = parallel_input(input)?;
    let now = parallel_now(ctx, at.fx);
    if json!({"parallel": now}) != *fence {
        return Err(changed("the FX was put in or out of parallel in REAPER since the preview"));
    }
    if now != wanted {
        ctx.reaper.fx_set_config(at.fx, "parallel", &wanted.to_string());
        if parallel_now(ctx, at.fx) != wanted {
            return Err(failed("REAPER kept the FX as it ran"));
        }
    }
    Ok(Applied {
        applied: json!({"parallel": wanted}),
        display: json!({"title": parallel_title(ctx, &at, wanted)}),
        unwritten: now == wanted,
        ..Default::default()
    })
}

pub fn parallel_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = target_fx(ctx, restore.target)?;
    let (prior, applied) = (restore.prior["parallel"].as_i64().unwrap_or(0), restore.applied["parallel"].as_i64().unwrap_or(0));
    let now = parallel_now(ctx, at.fx);
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed("the FX was put in or out of parallel in REAPER since Kumi did"));
        }
        ctx.reaper.fx_set_config(at.fx, "parallel", &prior.to_string());
        if parallel_now(ctx, at.fx) != prior {
            return Err(failed("REAPER wouldn't put the FX back as it ran"));
        }
    }
    Ok(Restored {
        restored: json!({"parallel": prior}),
        display: json!({"title": parallel_title(ctx, &at, prior)}),
        unwritten: now == prior,
    })
}

// ----- macros -----

/// The container's macros: (its parameter, the mapping's stable id, the child's place, the child's parameter).
fn macros(ctx: &Ctx, container: Fx) -> Vec<(i32, i64, usize, i32)> {
    let reaper = ctx.reaper;
    let number = |p: i32, key: &str| {
        reaper.fx_config(container, &format!("param.{p}.container_map.{key}")).and_then(|v| v.trim().parse::<i64>().ok())
    };
    (0..reaper.fx_param_count(container))
        .filter_map(|p| Some((p, number(p, "hint_id")?, number(p, "fx_index")? as usize, number(p, "fx_parm")? as i32)))
        .collect()
}

fn container_at(ctx: &Ctx, reference: &Value) -> Done<FxAt> {
    let at = fxchain::locate(ctx, reference)?;
    if !fxchain::is_container(ctx.reaper, at.fx) {
        return Err(bad("ref must be a container"));
    }
    Ok(at)
}

/// A macro's title: "“Parallel” on “Bass” macro: ReaComp Ratio".
fn macro_title(ctx: &Ctx, container: &FxAt, child: Fx, param: i32, verb: &str) -> String {
    format!(
        "{verb} {} {} as a macro of {}",
        fx_label(ctx, child),
        crate::rows::clip(&ctx.reaper.param_name(child, param)),
        label(ctx, container)
    )
}

fn map_input(ctx: &Ctx, input: &Value) -> Done<(FxAt, FxAt, usize, i32)> {
    let container = container_at(ctx, &input["ref"])?;
    let child = fxchain::locate(ctx, input.get("childRef").unwrap_or(&Value::Null))?;
    let place = ctx
        .reaper
        .fx_children(container.fx)
        .iter()
        .position(|c| *c == child.fx)
        .ok_or_else(|| bad("childRef must be an FX directly inside that container (map a deeper one at its own container first)"))?;
    let param =
        fx::locate_param(ctx, &json!({"ref": refs::fx_ref(&child.guid), "param": input.get("param").cloned().unwrap_or(Value::Null)}))?
            .param;
    Ok((container, child, place, param))
}

pub fn map_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let (container, child, place, param) = map_input(ctx, input)?;
    let existing = macros(ctx, container.fx).into_iter().find(|m| m.2 == place && m.3 == param);
    let fence = json!({"children": fxchain::guids(ctx.reaper, Holder::Container(container.fx)), "macros": macros(ctx, container.fx).len()});
    let mut display = json!({"title": macro_title(ctx, &container, child.fx, param, "Mapped")});
    if existing.is_some() {
        display["unchanged"] = true.into();
    }
    let mut target = fx_target(ctx, &container);
    target["childRef"] = refs::fx_ref(&child.guid).into();
    target["param"] = param.into();
    Ok(Preview { target, prior: json!({"mapped": existing.map(|m| m.1)}), fence, display })
}

pub fn map_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (container, child, place, param) = map_input(ctx, input).map_err(went)?;
    let children = fxchain::guids(ctx.reaper, Holder::Container(container.fx));
    if json!({"children": children, "macros": macros(ctx, container.fx).len()}) != *fence {
        return Err(changed("the container's FX or macros changed since the preview"));
    }
    let title = macro_title(ctx, &container, child.fx, param, "Mapped");
    if let Some(existing) = macros(ctx, container.fx).into_iter().find(|m| m.2 == place && m.3 == param) {
        return Ok(Applied {
            applied: json!({"macro": existing.0, "hint": existing.1}),
            created: Some(json!({"ref": refs::param_ref(&container.guid, existing.0), "kind": "param"})),
            display: json!({"title": title}),
            unwritten: true,
            ..Default::default()
        });
    }
    // (REAPER maps it as it is asked for the new parameter's index)
    ctx.reaper.fx_config(container.fx, &format!("container_map.add.{place}.{param}"));
    let made = macros(ctx, container.fx)
        .into_iter()
        .find(|m| m.2 == place && m.3 == param)
        .ok_or_else(|| failed("REAPER wouldn't map that parameter"))?;
    Ok(Applied {
        applied: json!({"macro": made.0, "hint": made.1}),
        created: Some(json!({"ref": refs::param_ref(&container.guid, made.0), "kind": "param"})),
        display: json!({"title": title, "ref": refs::param_ref(&container.guid, made.0)}),
        ..Default::default()
    })
}

/// Takes a macro off a container by its stable id (its parameter number moves as other macros come and go).
fn unmap(ctx: &Ctx, container: Fx, hint: i64) -> Done<()> {
    let (p, ..) = macros(ctx, container).into_iter().find(|m| m.1 == hint).ok_or_else(|| gone("that macro is gone from the container"))?;
    ctx.reaper.fx_config(container, &format!("param.{p}.container_map.delete"));
    // (REAPER keeps no undo point of a macro taken off by itself: the nudge makes it keep one)
    mark_fx(ctx.reaper, container);
    if macros(ctx, container).iter().any(|m| m.1 == hint) {
        return Err(failed("REAPER kept the macro"));
    }
    Ok(())
}

pub fn map_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    if restore.prior["mapped"].is_i64() {
        return Ok(Restored {
            restored: json!({}),
            display: json!({"title": "The macro was there before Kumi mapped it"}),
            unwritten: true,
        });
    }
    let container = target_fx(ctx, restore.target)?;
    let hint = restore.applied["hint"].as_i64().ok_or_else(|| bad("applied.hint"))?;
    let Some(found) = macros(ctx, container.fx).into_iter().find(|m| m.1 == hint) else {
        return Ok(Restored {
            restored: json!({}),
            display: json!({"title": format!("The macro of {} is gone already", label(ctx, &container))}),
            unwritten: true,
        });
    };
    let child_guid = match Ref::parse(restore.target["childRef"].as_str().unwrap_or_default()) {
        Ok(Ref::Fx(guid)) => guid,
        _ => return Err(bad("target.childRef")),
    };
    let child_now = ctx.reaper.fx_children(container.fx).get(found.2).map(|c| ctx.reaper.fx_guid(*c));
    if !restore.discard {
        if child_now.as_deref().is_none_or(|g| !g.eq_ignore_ascii_case(&child_guid))
            || Some(found.3 as i64) != restore.target["param"].as_i64()
        {
            return Err(changed("that macro maps another parameter now"));
        }
        if ctx.reaper.fx_envelope(container.fx, found.0, false).is_some() {
            return Err(changed("the macro has automation now"));
        }
        if ctx.reaper.fx_config(container.fx, &format!("param.{}.mod.active", found.0)).is_some_and(|v| v.trim() == "1") {
            return Err(changed("the macro has parameter modulation now"));
        }
    }
    unmap(ctx, container.fx, hint)?;
    Ok(Restored {
        restored: json!({"unmapped": hint}),
        display: json!({"title": format!("Took the macro off {}", label(ctx, &container))}),
        ..Default::default()
    })
}

// ----- a macro taken off -----

fn unmap_input(ctx: &Ctx, input: &Value) -> Done<(FxAt, (i32, i64, usize, i32))> {
    let text = input.get("ref").and_then(Value::as_str).ok_or_else(|| bad("ref: the container's macro (a param ref)"))?;
    let Ref::Param { fx: guid, index } = Ref::parse(text).map_err(bad)? else { return Err(bad("ref must be the macro's param ref")) };
    let container = container_at(ctx, &json!(refs::fx_ref(&guid)))?;
    let found =
        macros(ctx, container.fx).into_iter().find(|m| m.0 == index).ok_or_else(|| bad("that parameter of the container isn't a macro"))?;
    Ok((container, found))
}

pub fn unmap_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let (container, (p, hint, place, param)) = unmap_input(ctx, input)?;
    let child = *ctx.reaper.fx_children(container.fx).get(place).ok_or_else(|| failed("the macro's FX isn't in the container"))?;
    let now = json!({"hint": hint, "childRef": refs::fx_ref(&ctx.reaper.fx_guid(child)), "param": param});
    let mut display = json!({"title": macro_title(ctx, &container, child, param, "Unmapped")});
    let mut notes = Vec::new();
    if ctx.reaper.fx_envelope(container.fx, p, false).is_some() {
        notes.push("Its automation goes with it and doesn't come back with an undo.");
    }
    if ctx.reaper.fx_config(container.fx, &format!("param.{p}.mod.active")).is_some_and(|v| v.trim() == "1") {
        notes.push("Its parameter modulation goes with it.");
    }
    if !notes.is_empty() {
        display["note"] = notes.join(" ").into();
    }
    Ok(Preview { target: fx_target(ctx, &container), prior: now.clone(), fence: now, display })
}

pub fn unmap_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (container, (_, hint, place, param)) = unmap_input(ctx, input).map_err(went)?;
    let child = *ctx.reaper.fx_children(container.fx).get(place).ok_or_else(|| changed("the macro's FX left the container"))?;
    let now = json!({"hint": hint, "childRef": refs::fx_ref(&ctx.reaper.fx_guid(child)), "param": param});
    if now != *fence {
        return Err(changed("the macro changed since the preview"));
    }
    let title = macro_title(ctx, &container, child, param, "Unmapped");
    unmap(ctx, container.fx, hint)?;
    Ok(Applied { applied: json!({"unmapped": hint}), display: json!({"title": title}), ..Default::default() })
}

pub fn unmap_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let container = target_fx(ctx, restore.target)?;
    let child_guid = match Ref::parse(restore.prior["childRef"].as_str().unwrap_or_default()) {
        Ok(Ref::Fx(guid)) => guid,
        _ => return Err(bad("prior.childRef")),
    };
    let param = restore.prior["param"].as_i64().ok_or_else(|| bad("prior.param"))? as i32;
    let children = ctx.reaper.fx_children(container.fx);
    let Some(place) = children.iter().position(|c| ctx.reaper.fx_guid(*c).eq_ignore_ascii_case(&child_guid)) else {
        return Err(gone("the FX whose parameter was the macro isn't in the container any more"));
    };
    if let Some(existing) = macros(ctx, container.fx).into_iter().find(|m| m.2 == place && m.3 == param) {
        return Ok(Restored {
            restored: json!({"macro": existing.0, "hint": existing.1}),
            display: json!({"title": format!("The macro of {} is back already", label(ctx, &container))}),
            unwritten: true,
        });
    }
    ctx.reaper.fx_config(container.fx, &format!("container_map.add.{place}.{param}"));
    let made = macros(ctx, container.fx)
        .into_iter()
        .find(|m| m.2 == place && m.3 == param)
        .ok_or_else(|| failed("REAPER wouldn't map the parameter again"))?;
    let mut display = json!({"title": macro_title(ctx, &container, children[place], param, "Mapped again")});
    display["note"] = "It is mapped again as a new macro: its ref may change.".into();
    Ok(Restored {
        restored: json!({"macro": made.0, "hint": made.1, "ref": refs::param_ref(&container.guid, made.0)}),
        display,
        ..Default::default()
    })
}
