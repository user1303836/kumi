//! Keeping a scratch copy (M5 listening, `scratch-keep {ref: copy, name}`): a goal's best copy becomes a track of the
//! producer's, named as Kumi says and sending to the master as a track does, and leaves the scratch copies (its mark is
//! taken off, so no drop or sweep removes it). Undone by value: the track is deleted while it is as Kumi left it, all of
//! it (its state's hash less the plug-ins' own data: items, takes, fades, take FX, envelopes, automation items; and its
//! FX's parameters, those automation moves left to their points).

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::listening::scratch::MARK;
use crate::read::{bad, failed};
use crate::reaper::{FxOwner, Reaper, Track};
use crate::refs::{self, Ref, TrackId};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

fn gone(message: impl Into<String>) -> Fault {
    ("gone".into(), message.into())
}

fn guid_of(input: &Value, field: &str) -> Done<String> {
    let text = input.get(field).and_then(Value::as_str).ok_or_else(|| bad(format!("{field}: the copy's track ref")))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::Track(TrackId::Guid(guid)) => Ok(guid),
        Ref::Track(TrackId::Master) => Err(bad("the master is no scratch copy")),
        other => Err(bad(format!("{field} must be a track ref, not r:{}", other.kind()))),
    }
}

fn find(ctx: &Ctx, guid: &str) -> Option<Track> {
    ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string())).ok()
}

fn name_of(input: &Value) -> Done<String> {
    match input.get("name") {
        Some(Value::String(name)) if !name.trim().is_empty() && name.chars().count() <= 256 && !name.chars().any(char::is_control) => {
            Ok(name.clone())
        }
        _ => Err(bad("name: what the kept track is called, one line of at most 256 characters")),
    }
}

/// A hash of the track's items: their GUIDs, places and lengths.
fn items_hash(reaper: &Reaper, track: Track) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for index in 0..reaper.track_item_count(track) {
        let Some(item) = reaper.track_item(track, index) else { continue };
        feed(reaper.item_guid(item).as_bytes());
        feed(&reaper.item_value(item, "D_POSITION").to_bits().to_le_bytes());
        feed(&reaper.item_value(item, "D_LENGTH").to_bits().to_le_bytes());
    }
    format!("{hash:016x}")
}

/// A hash of what the producer works on inside the track: each FX's parameter values (one an envelope moves as the
/// project plays is left to its points), each item's active take (its MIDI events, offset, rate, pitch, volume), and
/// every point of the track's envelopes.
fn content_hash(reaper: &Reaper, track: Track) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let owner = FxOwner::Track(track);
    // (each container's FX after it, at any depth)
    let mut all = Vec::new();
    let mut stack: Vec<crate::reaper::Fx> = (0..reaper.fx_count(owner)).rev().map(|index| crate::reaper::Fx { owner, index }).collect();
    while let Some(fx) = stack.pop() {
        all.push(fx);
        stack.extend(reaper.fx_children(fx).into_iter().rev());
    }
    for fx in all {
        for param in 0..reaper.fx_param_count(fx) {
            if reaper.fx_param_envelope(track, fx.index, param).is_some() {
                continue;
            }
            if let Some((value, _, _)) = reaper.fx_param_raw(fx, param) {
                feed(&param.to_le_bytes());
                feed(&value.to_bits().to_le_bytes());
            }
        }
        feed(b"|");
    }
    for index in 0..reaper.track_item_count(track) {
        let Some(take) = reaper.track_item(track, index).and_then(|item| reaper.active_take(item)) else { continue };
        for parm in ["D_STARTOFFS", "D_PLAYRATE", "D_PITCH", "D_VOL"] {
            feed(&reaper.take_value(take, parm).to_bits().to_le_bytes());
        }
        for event in reaper.midi_events(take).unwrap_or_default() {
            feed(&event.pos.to_le_bytes());
            feed(&[event.flag & !1]);
            feed(&event.msg);
        }
        feed(b"|");
    }
    for index in 0..reaper.track_envelope_count(track) {
        let Some(envelope) = reaper.track_envelope(track, index) else { continue };
        for point in (0..reaper.env_point_count(envelope, -1)).filter_map(|at| reaper.env_point(envelope, -1, at)) {
            feed(&point.time.to_bits().to_le_bytes());
            feed(&point.value.to_bits().to_le_bytes());
            feed(&point.shape.to_le_bytes());
            feed(&point.tension.to_bits().to_le_bytes());
        }
        feed(b"|");
    }
    format!("{hash:016x}")
}

/// What the copy is, besides its name and where it sends: its items, its FX, its routing, its fader, its folder, and what
/// it holds (`content_hash`). A producer's edit of any of it is theirs, and Kumi's undo then leaves the track be.
fn signature(reaper: &Reaper, track: Track) -> Value {
    let owner = FxOwner::Track(track);
    let fx: Vec<Value> = (0..reaper.fx_count(owner))
        .map(|index| {
            let fx = crate::reaper::Fx { owner, index };
            json!([reaper.fx_guid(fx), reaper.fx_enabled(fx), reaper.fx_offline(fx)])
        })
        .collect();
    let points: i32 = (0..reaper.track_envelope_count(track))
        .filter_map(|i| reaper.track_envelope(track, i))
        .map(|envelope| reaper.envelope_point_count(envelope))
        .sum();
    json!({
        "items": {"count": reaper.track_item_count(track), "hash": items_hash(reaper, track)},
        "fx": fx,
        "sends": reaper.send_count(track, 0),
        "receives": reaper.send_count(track, -1),
        "hwOuts": reaper.send_count(track, 1),
        "volume": reaper.track_value(track, "D_VOL"),
        "pan": reaper.track_value(track, "D_PAN"),
        "mute": reaper.track_value(track, "B_MUTE") != 0.0,
        "parent": reaper.track_parent(track).map(|parent| reaper.track_guid(parent)),
        "depth": reaper.track_value(track, "I_FOLDERDEPTH") as i64,
        "envelopePoints": points,
        "content": content_hash(reaper, track),
    })
}

fn mark_of(reaper: &Reaper, track: Track) -> String {
    reaper.track_string(track, MARK).unwrap_or_default()
}

/// A hash of the track's whole state as the producer changes it (`templates::content_hash`: items, takes, fades, take
/// FX, envelopes, automation items, routing, the FX chains' structure), less what only the screen or a selection changes
/// and less the plug-ins' own data: an automated parameter moves that as the project plays, and the signature's
/// parameter hash (which leaves those to their points) covers the rest. None when REAPER won't give it.
fn print(reaper: &Reaper, track: Track) -> Option<String> {
    reaper.track_chunk(track).map(|chunk| super::templates::content_hash(&chunk))
}

/// What the preview fences and the apply checks again: the copy by its GUID, still a scratch copy, as it was.
fn fence(reaper: &Reaper, track: Track, guid: &str) -> Value {
    json!({
        "guid": guid,
        "mark": mark_of(reaper, track),
        "name": reaper.track_name(track),
        "mainSend": reaper.track_value(track, "B_MAINSEND") != 0.0,
        "signature": signature(reaper, track),
        "print": print(reaper, track),
    })
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let guid = guid_of(input, "ref")?;
    let name = name_of(input)?;
    let track = find(ctx, &guid).ok_or_else(|| failed("that copy isn't in the project any more: read again"))?;
    let mark = mark_of(ctx.reaper, track);
    if mark.is_empty() {
        return Err(bad("that track isn't one of Kumi's scratch copies (scratch make makes them)"));
    }
    let now = ctx.reaper.track_name(track);
    Ok(Preview {
        target: json!({"ref": refs::track_ref(&TrackId::Guid(guid.clone())), "kind": "track"}),
        prior: json!({"name": now, "mainSend": ctx.reaper.track_value(track, "B_MAINSEND") != 0.0, "mark": mark}),
        fence: fence(ctx.reaper, track, &guid),
        display: json!({"title": format!("Kept {}", super::quoted(&name)), "before": now, "after": name}),
    })
}

pub fn apply(ctx: &Ctx, input: &Value, fenced: &Value) -> Done<Applied> {
    let guid = guid_of(input, "ref")?;
    let name = name_of(input)?;
    let reaper = ctx.reaper;
    let track = find(ctx, &guid).ok_or_else(|| changed("that copy went since the preview"))?;
    if fence(reaper, track, &guid) != *fenced {
        let was = fenced["name"].as_str().unwrap_or_default();
        return Err(changed(format!("{} changed since the preview", super::quoted(was))));
    }
    if !reaper.set_track_string(track, "P_NAME", &name) {
        return Err(failed("REAPER wouldn't name the track"));
    }
    reaper.set_track_value(track, "B_MAINSEND", 1.0);
    // Out of the scratch copies: no drop or sweep takes a track without Kumi's mark.
    reaper.set_track_string(track, MARK, "");
    let reference = refs::track_ref(&TrackId::Guid(guid.clone()));
    // The whole track as Kumi leaves it: its undo deletes the track only while it is still so (by the signature alone when
    // REAPER won't give the track's state).
    let print = print(reaper, track);
    Ok(Applied {
        applied: json!({"name": reaper.track_name(track), "mainSend": true, "signature": signature(reaper, track), "print": print}),
        created: Some(json!({"ref": reference, "kind": "track"})),
        material: None,
        display: json!({"title": format!("Kept {}", super::quoted(&name))}),
        ..Default::default()
    })
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the track that was kept"))?;
    let guid = guid_of(created, "ref")?;
    let reaper = ctx.reaper;
    let Some(track) = find(ctx, &guid) else {
        return Err(gone("the track Kumi kept isn't in the project any more"));
    };
    let name = reaper.track_name(track);
    let applied = restore.applied;
    // All of the track: its state less the plug-ins' own data, and its FX's parameters by the signature (those automation
    // moves as the project plays left to their points).
    let same = applied.get("signature") == Some(&signature(reaper, track))
        && applied["print"].as_str().is_none_or(|kept| print(reaper, track).as_deref() == Some(kept));
    let as_left = applied["name"].as_str() == Some(name.as_str()) && reaper.track_value(track, "B_MAINSEND") != 0.0 && same;
    if !as_left && !restore.discard {
        return Err(changed(format!("{} changed in REAPER since Kumi kept it", super::quoted(&name))));
    }
    super::tracks::delete_keeping_folders(ctx, &[track], if restore.discard { None } else { Some(&[]) })?;
    Ok(Restored {
        restored: json!({"deletedRef": refs::track_ref(&TrackId::Guid(guid))}),
        display: json!({"title": format!("{} taken away again", super::quoted(&name))}),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake;

    fn hash_of(at: i32) -> String {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let proj = reaper.current_project().unwrap().ptr;
        content_hash(&reaper, reaper.track_at(proj, at).unwrap())
    }

    /// The hash the kept track's undo checks moves with an edit of its MIDI or of its envelopes' points, and not with
    /// what only selects them. (The track's print, the signature's other half, reads REAPER's own state text: its
    /// selections are left out there, as `a_track_state_reads_the_same_selected_or_not_and_without_its_plug_ins_data`
    /// shows.)
    #[test]
    fn the_content_hash_moves_with_a_midi_or_envelope_edit_and_not_with_a_selection() {
        fake::lab_project();
        // Drums (0) has a volume envelope, the snare (2) a MIDI take.
        fake::with(|s| s.tracks[0].envelopes[0].pts = Some(vec![fake::FakePoint::at(0.0, 1.0), fake::FakePoint::at(2.0, 0.5)]));
        let (drums, snare) = (hash_of(0), hash_of(2));
        fake::with(|s| {
            s.tracks[0].envelopes[0].pts.as_mut().unwrap()[1].selected = true;
            s.tracks[2].items[0].takes[0].notes[0].selected = true;
        });
        assert_eq!((hash_of(0), hash_of(2)), (drums.clone(), snare.clone()), "selected");
        type Edit = fn(&mut fake::State);
        let edits: [(&str, usize, Edit); 5] = [
            ("a note moved", 2, |s| s.tracks[2].items[0].takes[0].notes[0].start_ppq = 240.0),
            ("a note's pitch", 2, |s| s.tracks[2].items[0].takes[0].notes[1].pitch = 41),
            ("a controller's value", 2, |s| s.tracks[2].items[0].takes[0].ccs[1].data2 = 90),
            ("a point moved", 0, |s| s.tracks[0].envelopes[0].pts.as_mut().unwrap()[1].time = 3.0),
            ("a point's shape", 0, |s| s.tracks[0].envelopes[0].pts.as_mut().unwrap()[0].shape = 2),
        ];
        for (what, at, edit) in edits {
            let was = fake::with(|s| s.tracks.clone());
            fake::with(edit);
            let before = if at == 0 { &drums } else { &snare };
            assert_ne!(&hash_of(at as i32), before, "{what}");
            fake::with(|s| s.tracks = was);
        }
    }
}
