//! History v0 in the change flow: what a change will cut or delete is read before it's applied, and what Live left of
//! it after, so Kumi's undo can make it again (see `snapshots`). A deleted clip, a cleared stretch of the
//! Arrangement, a new clip laid over others, and a move that replaces what's in its new place.
use super::{
    bridge_version::{at_least, PYTHON_BRIDGE},
    changes::{laid_over, ChangeKind},
    connection::LiveConnection,
    history::History,
    snapshots::{capture, Captured, KeptClip, Material, Remnant},
    views::ViewHost,
};
use crate::core::contracts::JsonObject;
use kumi_common::abort::Signal;
use serde_json::{json, Value};

/// The most later clips an audio clip laid over others is read with (its length shows only once it's made).
const MAX_LATER_CLIPS: usize = 64;

/// What a change will cut or delete, read before it's applied.
pub struct Cut {
    /// What to read again after, to find what Live left (a track, or a clip on it); none for a deletion.
    anchor: Option<String>,
    clips: Vec<Captured>,
    /// The change has its own undo (deleting the new clip, or moving the clip back) before the clips come back.
    host_undo: bool,
}

/// Whether Live can be asked: Kumi runs its Python in Live from bridge 1.0.68.
pub fn can_snapshot(connection: &LiveConnection) -> bool {
    connection.has("live_run_python") && at_least(connection.version().as_deref(), PYTHON_BRIDGE)
}

fn beats(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// Read what `kind` will cut or delete, given its args, its preview and (for a new Arrangement clip) the track's
/// clips read before. None when it cuts nothing, or Live couldn't say.
pub async fn before(
    history: &History,
    kind: &ChangeKind,
    args: &JsonObject,
    preview: &JsonObject,
    under: Option<&[JsonObject]>,
    signal: &Signal,
) -> Option<Cut> {
    if !can_snapshot(&history.connection) {
        return None;
    }
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    let (anchor, read, host_undo) = match kind.tool.as_str() {
        "delete_clip" => {
            let clip = text("clipRef").filter(|clip| !clip.contains("take_lane"))?;
            (None, json!({"clips":[clip]}), false)
        }
        "clear_range" => {
            let track = text("trackRef")?;
            (Some(track.clone()), json!({"track":track,"from":args.get("fromBeat")?,"to":args.get("toBeat")?}), false)
        }
        "add_arrangement_clip" if !args.contains_key("takeLaneRef") => {
            let (track, start, under) = (text("trackRef")?, beats(args.get("position"))?, under?);
            let read = match beats(args.get("length")) {
                Some(length) if !laid_over(under, start, start + length).is_empty() => {
                    json!({"track":track,"from":start,"to":start + length})
                }
                Some(_) => return None,
                // An audio clip's length shows once it's made: every clip it could reach is read, and those it didn't
                // are let go after.
                None => {
                    let later = under.iter().filter(|clip| beats(clip.get("endTime")).is_some_and(|end| end > start + 1e-6)).count();
                    if later == 0 || later > MAX_LATER_CLIPS {
                        return None;
                    }
                    json!({"track":track,"from":start,"to":1e9})
                }
            };
            (Some(track), read, true)
        }
        // A copy, or a move to another track, lands where the clip's own track isn't: not yet. The preview's payload
        // says which it is (a false keepSource, a null targetTrackRef or the clip's own track are neither).
        "move_clip"
            if preview.get("payload").is_some_and(|payload| {
                payload.get("keepSource") == Some(&json!(true)) || payload.get("targetTrackRef").is_some_and(|track| !track.is_null())
            }) =>
        {
            return None
        }
        "move_clip" => {
            // What an Arrangement move replaces, as its preview names it: the span from the first to the last of them
            // holds just them, the moving clip aside.
            let replaces = preview.get("replaces").and_then(Value::as_array).filter(|replaces| !replaces.is_empty())?;
            let from = replaces.iter().filter_map(|r| beats(r.get("start"))).fold(f64::INFINITY, f64::min);
            let to = replaces.iter().filter_map(|r| beats(r.get("end"))).fold(f64::NEG_INFINITY, f64::max);
            let moving = preview.get("payload").and_then(|p| p.get("expectedObjectIdentity")).cloned().unwrap_or(Value::Null);
            let clip = text("clipRef")?;
            (None, json!({"track":clip,"from":from,"to":to,"except":[moving]}), true)
        }
        _ => return None,
    };
    // A clip Kumi's undo can't make again (drawn out past its file, or too big for one call to Live, its notes aside)
    // leaves the change to Live's undo.
    let clips = capture(history, read, signal.clone()).await.filter(|clips| !clips.is_empty() && clips.iter().all(Captured::makeable))?;
    Some(Cut { anchor, clips, host_undo })
}

impl Cut {
    /// What Live left, read once the change is applied: `made` is the clip the change put there (a new clip, or the
    /// moved one: its ref, to find the track by, and its identity, which isn't a remnant), and `span` where it is.
    /// The kept clips (with their objects' hashes, from `keep`) go into the Material.
    pub async fn after(
        mut self,
        history: &History,
        made: Option<(&str, &str)>,
        span: Option<(f64, f64)>,
        signal: &Signal,
    ) -> Option<(Material, Vec<Captured>)> {
        if let Some((start, end)) = span {
            // Only what the new clip reached.
            self.clips.retain(|clip| clip.span().is_some_and(|(from, to)| from < end - 1e-6 && to > start + 1e-6));
        }
        let first = self.clips.first()?;
        let (track, track_name, track_id) = (first.track.clone(), first.track_name.clone(), first.track_id.clone());
        let anchor = self.anchor.clone().or_else(|| made.map(|(reference, _)| reference.to_owned()));
        let remnants = match anchor {
            // A deletion leaves nothing of the clip.
            None => vec![],
            Some(anchor) => {
                let from = self.clips.iter().filter_map(|clip| clip.span()).map(|(start, _)| start).fold(f64::INFINITY, f64::min);
                let to = self.clips.iter().filter_map(|clip| clip.span()).map(|(_, end)| end).fold(f64::NEG_INFINITY, f64::max);
                let except: Vec<&str> = made.map(|(_, identity)| identity).into_iter().collect();
                capture(history, json!({"track":anchor,"from":from,"to":to,"except":except}), signal.clone())
                    .await?
                    .into_iter()
                    .map(|clip| Remnant { name: clip.name(), identity: clip.identity, hash: clip.hash })
                    .collect()
            }
        };
        let material = Material {
            track,
            track_name,
            track_id,
            clips: vec![],
            remnants,
            leaving: made.map(|(_, identity)| vec![identity.to_owned()]).unwrap_or_default(),
            host_undo: self.host_undo,
        };
        Some((material, self.clips))
    }
}

/// The kept clips, by where they were and the objects `keep` gave them.
pub fn kept(clips: &[Captured], objects: &[String]) -> Vec<KeptClip> {
    clips
        .iter()
        .zip(objects)
        .map(|(clip, object)| KeptClip {
            place: clip.place.clone(),
            object: object.clone(),
            name: clip.name(),
            notes_hash: clip.notes_hash.clone(),
        })
        .collect()
}
