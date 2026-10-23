//! Track groups and VCAs (r-tracks, M4): a track's roles in one of REAPER's 64 groups (lead or follow for volume, VCA,
//! pan, width, mute, solo, record arm, polarity, automation mode and media edits, and the group's options), and a group's
//! name. REAPER keeps a track's membership as one bit a group for each role (`GetSetTrackGroupMembership`, groups 33-64
//! through its `High` twin), so a change touches exactly the bits it names, and its undo puts back exactly those.

use serde_json::{json, Map, Value};

use super::{track_ref_of, Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, Track};
use crate::refs::{self, TrackId};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

use crate::rows::GROUP_ROLES as ROLES;

fn reaper_role(role: &str) -> Option<&'static str> {
    ROLES.iter().find(|(name, _)| *name == role).map(|(_, reaper)| *reaper)
}

/// Group `group` (1-64): which of REAPER's two calls holds it, and its bit there.
fn place(group: i64) -> (bool, u32) {
    let high = group > 32;
    (high, 1u32 << ((group - 1) % 32))
}

/// Whether the track has `role` in `group`.
fn member(reaper: &Reaper, track: Track, role: &str, group: i64) -> Option<bool> {
    let (high, bit) = place(group);
    reaper.group_membership(track, reaper_role(role)?, high).map(|mask| mask & bit != 0)
}

fn group_of(input: &Value) -> Done<i64> {
    input
        .get("groupNumber")
        .and_then(Value::as_i64)
        .filter(|g| (1..=64).contains(g))
        .ok_or_else(|| bad("groupNumber must be one of REAPER's track groups, 1 to 64"))
}

fn group_name(ctx: &Ctx, group: i64) -> String {
    ctx.reaper.project_text(ctx.proj, &format!("TRACK_GROUP_NAME:{group}")).unwrap_or_default()
}

fn group_label(ctx: &Ctx, group: i64) -> String {
    match group_name(ctx, group) {
        name if name.is_empty() => format!("group {group}"),
        name => format!("group {group} {}", super::quoted(&name)),
    }
}

/// The roles a request names, as {role: in the group}.
fn wanted(input: &Value) -> Done<Value> {
    let mut wanted = Map::new();
    for (key, on) in [("add", true), ("remove", false)] {
        let Some(list) = input.get(key).filter(|v| !v.is_null()) else { continue };
        let list = list.as_array().ok_or_else(|| bad(format!("{key} must list roles")))?;
        for role in list {
            let role = role.as_str().filter(|role| reaper_role(role).is_some()).ok_or_else(|| {
                bad(format!("{key}: a role is one of {}", ROLES.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")))
            })?;
            if wanted.insert(role.to_string(), on.into()).is_some() {
                return Err(bad(format!("{role} is named twice")));
            }
        }
    }
    if wanted.is_empty() {
        return Err(bad("give add, remove or both: the track's roles in the group (volume-lead, vca-follow, mute-follow…)"));
    }
    Ok(Value::Object(wanted))
}

fn roles_now(reaper: &Reaper, track: Track, group: i64, of: &Value) -> Done<Value> {
    let mut state = Map::new();
    for role in of.as_object().into_iter().flatten().map(|(role, _)| role) {
        let on = member(reaper, track, role, group).ok_or_else(|| failed("this REAPER can't read track groups"))?;
        state.insert(role.clone(), on.into());
    }
    Ok(Value::Object(state))
}

fn write_roles(reaper: &Reaper, track: Track, group: i64, state: &Value) -> bool {
    let (high, bit) = place(group);
    state.as_object().into_iter().flatten().all(|(role, on)| {
        reaper_role(role)
            .is_some_and(|name| reaper.set_group_membership(track, name, high, bit, if *on == true { bit } else { 0 }).is_some())
    })
}

fn title(ctx: &Ctx, name: &str, group: i64, state: &Value) -> String {
    let pick = |on: bool| -> Vec<&str> {
        state.as_object().into_iter().flatten().filter(|(_, v)| **v == on).map(|(role, _)| role.as_str()).collect()
    };
    let mut parts = Vec::new();
    let joined = pick(true);
    if !joined.is_empty() {
        parts.push(format!("in {} as {}", group_label(ctx, group), joined.join(", ")));
    }
    let left = pick(false);
    if !left.is_empty() {
        parts.push(format!("no longer {} in {}", left.join(", "), group_label(ctx, group)));
    }
    format!("{name} {}", parts.join("; "))
}

struct Found {
    track: Track,
    id: TrackId,
    name: String,
}

fn find(ctx: &Ctx, reference: &Value) -> Done<Found> {
    let id = track_ref_of(&json!({"ref": reference}), "track-group")?;
    if id == TrackId::Master {
        return Err(bad("the master track is in no track group"));
    }
    let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id)?;
    Ok(Found { track, id, name: super::track_name(ctx.reaper, track) })
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, &input["ref"])?;
    let group = group_of(input)?;
    let wanted = wanted(input)?;
    let now = roles_now(ctx.reaper, found.track, group, &wanted)?;
    let mut display = json!({"title": title(ctx, &found.name, group, &wanted), "before": now, "after": wanted});
    if now == wanted {
        display["unchanged"] = true.into();
    }
    Ok(Preview {
        target: json!({"ref": refs::track_ref(&found.id), "kind": "track", "groupNumber": group}),
        prior: now.clone(),
        fence: now,
        display,
    })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find(ctx, &input["ref"]).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    let group = group_of(input)?;
    let wanted = wanted(input)?;
    let now = roles_now(ctx.reaper, found.track, group, &wanted)?;
    if &now != fence {
        return Err(changed(format!("{}'s roles in {} changed since the preview", found.name, group_label(ctx, group))));
    }
    let unwritten = now == wanted;
    if !unwritten && !write_roles(ctx.reaper, found.track, group, &wanted) {
        return Err(failed("REAPER wouldn't take the track's group roles"));
    }
    let after = roles_now(ctx.reaper, found.track, group, &wanted)?;
    if after != wanted {
        write_roles(ctx.reaper, found.track, group, &now);
        return Err(failed("REAPER kept the track's group roles as they were"));
    }
    Ok(Applied {
        applied: after.clone(),
        created: None,
        material: None,
        display: json!({"title": title(ctx, &found.name, group, &wanted), "before": now, "after": after}),
        unwritten,
    })
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found =
        find(ctx, &restore.target["ref"]).map_err(|fault| if fault.0 == "failed" { ("gone".to_string(), fault.1) } else { fault })?;
    let group = restore.target["groupNumber"].as_i64().filter(|g| (1..=64).contains(g)).ok_or_else(|| bad("target: the group"))?;
    let now = roles_now(ctx.reaper, found.track, group, restore.prior)?;
    let already = &now == restore.prior;
    if !already {
        if !restore.discard && &now != restore.applied {
            return Err(changed(format!(
                "{}'s roles in {} have been changed in REAPER since Kumi set them",
                found.name,
                group_label(ctx, group)
            )));
        }
        if !write_roles(ctx.reaper, found.track, group, restore.prior)
            || &roles_now(ctx.reaper, found.track, group, restore.prior)? != restore.prior
        {
            return Err(failed("REAPER wouldn't put back the track's group roles"));
        }
    }
    Ok(Restored {
        restored: restore.prior.clone(),
        display: json!({"title": format!("{} (put back)", title(ctx, &found.name, group, restore.prior)), "before": now, "after": restore.prior}),
        unwritten: already,
    })
}

// ----- a group's name -----

fn name_wanted(input: &Value) -> Done<String> {
    match input.get("name") {
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Ok(name.clone()),
        _ => Err(bad("name must be one line of at most 256 characters (\"\" clears it)")),
    }
}

fn name_title(group: i64, name: &str) -> String {
    if name.is_empty() {
        format!("Group {group} name cleared")
    } else {
        format!("Group {group} named {}", super::quoted(name))
    }
}

pub fn name_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let group = group_of(input)?;
    let name = name_wanted(input)?;
    let now = json!({"name": group_name(ctx, group)});
    let mut display = json!({"title": name_title(group, &name), "before": now, "after": {"name": name}});
    if now["name"] == name {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "group", "groupNumber": group}), prior: now.clone(), fence: now, display })
}

pub fn name_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let group = group_of(input)?;
    let name = name_wanted(input)?;
    let now = json!({"name": group_name(ctx, group)});
    if &now != fence {
        return Err(changed(format!("group {group}'s name changed since the preview")));
    }
    let unwritten = now["name"] == name;
    if !unwritten && (!ctx.reaper.set_project_text(ctx.proj, &format!("TRACK_GROUP_NAME:{group}"), &name) || group_name(ctx, group) != name)
    {
        return Err(failed("REAPER wouldn't take the group's name"));
    }
    Ok(Applied {
        applied: json!({"name": name}),
        created: None,
        material: None,
        display: json!({"title": name_title(group, &name), "before": now}),
        unwritten,
    })
}

pub fn name_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let group = restore.target["groupNumber"].as_i64().filter(|g| (1..=64).contains(g)).ok_or_else(|| bad("target: the group"))?;
    let now = group_name(ctx, group);
    let prior = restore.prior["name"].as_str().unwrap_or_default().to_string();
    let already = now == prior;
    if !already {
        if !restore.discard && now != restore.applied["name"].as_str().unwrap_or_default() {
            return Err(changed(format!("group {group}'s name has been changed in REAPER since Kumi set it")));
        }
        if !ctx.reaper.set_project_text(ctx.proj, &format!("TRACK_GROUP_NAME:{group}"), &prior) || group_name(ctx, group) != prior {
            return Err(failed("REAPER wouldn't put back the group's name"));
        }
    }
    Ok(Restored {
        restored: json!({"name": prior}),
        display: json!({"title": format!("{} (put back)", name_title(group, &prior))}),
        unwritten: already,
    })
}
