//! `focus_body`: what FOCUS draws for the view the producer is in, from the catalog, in the host simulator's shapes:
//! `tree` (the selected track's strip: its inserts and sends), `notes` (the selected region's notes, paged),
//! `timeline` (the tracks as lanes with their regions in a window of beats, paged by lanes) and `routing` (a strip's
//! inputs, sends and output). A body names a track or region by `ref`, else takes the selected one.

use serde_json::{json, Value};

use crate::read::{self, Catalog};
use crate::refs::{self, bad, check_epoch, Fault};

pub const BODIES: [&str; 4] = ["tree", "notes", "timeline", "routing"];
/// The timeline body's page, in tracks.
pub const MAX_LANES: usize = 500;

/// One row by its ref, or the selected one of its kind when none is named.
fn one(catalog: &Catalog, kind: &str, wanted: Option<&str>) -> Result<Value, Fault> {
    let wanted = match wanted {
        Some(wanted) => wanted.to_string(),
        None => {
            let key = if kind == "track" { "trackRef" } else { "regionRef" };
            match catalog.selection.get(key).and_then(Value::as_str) {
                Some(selected) => selected.to_string(),
                None => format!("{}:{kind}:1", catalog.epoch),
            }
        }
    };
    catalog.rows(kind).into_iter().find(|row| row["ref"] == wanted).ok_or_else(|| refs::not_in_project(&wanted))
}

fn sends_of(catalog: &Catalog, track_ref: &Value) -> Vec<Value> {
    catalog
        .sends
        .iter()
        .filter(|send| send["trackRef"] == *track_ref)
        .map(|send| json!({"slot": send["slot"], "destination": send["destination"], "level": send["level"], "sendRef": send["ref"]}))
        .collect()
}

/// Answers a body. The notes body needs the region's notes in the catalog; the caller gathers them first (see
/// [`notes_region`]).
pub fn body(catalog: &Catalog, params: &Value) -> Result<Value, Fault> {
    check_epoch(params, catalog.epoch)?;
    let epoch = catalog.epoch_text();
    let wanted = params.get("ref").and_then(Value::as_str);
    match params.get("kind").and_then(Value::as_str).unwrap_or("") {
        "tree" => {
            let track = one(catalog, "track", wanted)?;
            let inserts: Vec<Value> = catalog
                .plugins
                .iter()
                .filter(|plugin| plugin["trackRef"] == track["ref"])
                .map(|plugin| json!({"slot": plugin["slot"], "name": plugin["name"], "bypassed": plugin["bypassed"], "pluginRef": plugin["ref"], "kind": plugin["kind"]}))
                .collect();
            Ok(json!({
                "kind": "tree",
                "epoch": epoch,
                "trackRef": track["ref"],
                "strip": {
                    "name": track["name"], "kind": track["kind"], "volumeDb": track["volumeDb"], "pan": track["pan"],
                    "mute": track["mute"], "solo": track["solo"], "armed": track["armed"],
                    "inputs": track["inputs"], "output": track["output"],
                    "inserts": inserts, "sends": sends_of(catalog, &track["ref"]),
                },
            }))
        }
        "notes" => {
            let region = one(catalog, "region", wanted)?;
            let notes: Vec<Value> = catalog.rows("note").into_iter().filter(|note| note["regionRef"] == region["ref"]).collect();
            let (limit, start) = (read::limit_of(params, read::MAX_LIMIT), read::start_of(params)?);
            let page: Vec<Value> = notes.iter().skip(start).take(limit).cloned().collect();
            let mut body = json!({
                "kind": "notes",
                "epoch": epoch,
                "regionRef": region["ref"],
                "name": region["name"],
                "startBeats": region["startBeats"],
                "endBeats": region["endBeats"],
                "timeSignature": catalog.transport.get("timeSignature").cloned().unwrap_or(Value::Null),
                "notes": page,
                "total": notes.len(),
            });
            if start + limit < notes.len() {
                body["nextCursor"] = refs::cursor(catalog.epoch, start + limit).into();
            }
            Ok(body)
        }
        "timeline" => {
            let window = params.get("window").cloned().unwrap_or(json!({"from": 0.0, "to": 16.0}));
            let (from, to) = (window["from"].as_f64().unwrap_or(0.0), window["to"].as_f64().unwrap_or(16.0));
            let tracks = &catalog.tracks;
            let limit = params.get("limit").and_then(Value::as_u64).map_or(MAX_LANES, |n| (n as usize).clamp(1, MAX_LANES));
            let start = match params.get("around").and_then(Value::as_str) {
                Some(around) => {
                    let place = tracks.iter().position(|track| track["ref"] == around).ok_or_else(|| refs::not_in_project(around))?;
                    place.saturating_sub(limit / 2).min(tracks.len().saturating_sub(limit))
                }
                None => read::start_of(params)?,
            };
            let lanes: Vec<Value> = tracks
                .iter()
                .skip(start)
                .take(limit)
                .map(|track| {
                    let regions: Vec<Value> = catalog
                        .regions
                        .iter()
                        .filter(|region| {
                            region["trackRef"] == track["ref"] && region["startBeats"].as_f64().unwrap_or(0.0) < to && region["endBeats"].as_f64().unwrap_or(0.0) > from
                        })
                        .map(|region| json!({"ref": region["ref"], "name": region["name"], "startBeats": region["startBeats"], "endBeats": region["endBeats"], "kind": region["kind"], "selected": region["selected"]}))
                        .collect();
                    json!({"trackRef": track["ref"], "name": track["name"], "kind": track["kind"], "regions": regions})
                })
                .collect();
            let playhead = catalog.transport.get("playhead").cloned().unwrap_or(Value::Null);
            let beats_per_bar = catalog.transport.get("timeSignature").and_then(Value::as_str).map_or(4, crate::rows::beats_per_bar_of);
            let playhead_beats = playhead.get("bar").and_then(Value::as_u64).map(|bar| {
                let beat = playhead.get("beat").and_then(Value::as_u64).unwrap_or(1);
                ((bar.max(1) - 1) * beats_per_bar as u64 + (beat.max(1) - 1)) as f64
            });
            let markers: Vec<Value> = catalog
                .markers
                .iter()
                .map(|marker| {
                    let position = &marker["position"];
                    let bar = position.get("bar").and_then(Value::as_u64).unwrap_or(1);
                    let beat = position.get("beat").and_then(Value::as_u64).unwrap_or(1);
                    json!({"name": marker["name"], "positionBeats": ((bar.max(1) - 1) * beats_per_bar as u64 + (beat.max(1) - 1)) as f64})
                })
                .collect();
            let mut body = json!({
                "kind": "timeline",
                "epoch": epoch,
                "window": {"from": from, "to": to},
                "lanes": lanes,
                "trackCount": tracks.len(),
                "from": start,
                "beatsPerBar": beats_per_bar,
                "playheadBeats": playhead_beats,
                "playing": catalog.transport.get("playing").cloned().unwrap_or(Value::Bool(false)),
                "markers": markers,
            });
            if start + limit < tracks.len() {
                body["nextCursor"] = refs::cursor(catalog.epoch, start + limit).into();
            }
            Ok(body)
        }
        "routing" => {
            let track = one(catalog, "track", wanted)?;
            Ok(json!({
                "kind": "routing",
                "epoch": epoch,
                "trackRef": track["ref"],
                "inputs": track["inputs"],
                "output": track["output"],
                "sends": sends_of(catalog, &track["ref"]),
            }))
        }
        _ => Err(bad(format!("Logic Pro has no body of that kind: use one of {}", BODIES.join(", ")))),
    }
}

/// The region a `notes` body is about (named, else selected, else the first), as its place through the project: what
/// the caller reads the notes of before answering.
pub fn notes_region(catalog: &Catalog, params: &Value) -> Result<usize, Fault> {
    check_epoch(params, catalog.epoch)?;
    let region = one(catalog, "region", params.get("ref").and_then(Value::as_str))?;
    let reference = region["ref"].as_str().unwrap_or_default();
    refs::Ref::parse(reference).map(|r| r.first()).ok_or_else(|| refs::not_in_project(reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::tests::scratch;

    #[test]
    fn the_bodies_answer_in_the_simulators_shapes_from_the_catalog() {
        let catalog = scratch(1);
        let tree = body(&catalog, &json!({"kind": "tree", "ref": "1:track:1"})).unwrap();
        assert_eq!(tree["strip"]["inserts"][0]["pluginRef"], "1:plugin:1.1");
        assert_eq!(tree["strip"]["sends"][0], json!({"slot": 1, "destination": "Bus 1", "level": -6.0, "sendRef": "1:send:1.1"}));
        assert_eq!(tree["strip"]["name"], "Vocal");
        assert_eq!(body(&catalog, &json!({"kind": "tree"})).unwrap()["trackRef"], "1:track:1", "the selected track");
        let notes = body(&catalog, &json!({"kind": "notes", "ref": "1:region:1"})).unwrap();
        assert_eq!(
            (notes["total"].as_u64(), notes["notes"].as_array().unwrap().len(), notes["endBeats"].as_f64()),
            (Some(4), 4, Some(16.0))
        );
        let paged = body(&catalog, &json!({"kind": "notes", "limit": 3})).unwrap();
        assert_eq!((paged["notes"].as_array().unwrap().len(), paged["nextCursor"].as_str()), (3, Some("1.3")));
        assert_eq!(notes_region(&catalog, &json!({"kind": "notes"})), Ok(1));
        assert_eq!(notes_region(&catalog, &json!({"kind": "notes", "ref": "1:region:2"})), Err(refs::not_in_project("1:region:2")));
        let timeline = body(&catalog, &json!({"kind": "timeline"})).unwrap();
        assert_eq!(timeline["lanes"].as_array().unwrap().len(), 2);
        assert_eq!(timeline["lanes"][1]["regions"][0]["ref"], "1:region:1");
        assert_eq!(
            (timeline["trackCount"].as_u64(), timeline["from"].as_u64(), timeline["playheadBeats"].as_f64()),
            (Some(2), Some(0), Some(0.0))
        );
        assert_eq!(timeline["markers"][0], json!({"name": "Chorus", "positionBeats": 32.0}));
        let around =
            body(&catalog, &json!({"kind": "timeline", "around": "1:track:2", "window": {"from": 0, "to": 16}, "limit": 1})).unwrap();
        assert_eq!(
            (around["lanes"].as_array().unwrap().len(), around["lanes"][0]["name"].as_str(), around["from"].as_u64()),
            (1, Some("Keys"), Some(1))
        );
        assert_eq!(around.get("nextCursor"), None);
        let first = body(&catalog, &json!({"kind": "timeline", "limit": 1})).unwrap();
        assert_eq!(first["nextCursor"], "1.1");
        let routing = body(&catalog, &json!({"kind": "routing", "ref": "1:track:1"})).unwrap();
        assert_eq!(
            (routing["inputs"][0].as_str(), routing["output"].as_str(), routing["sends"][0]["destination"].as_str()),
            (Some("Input 1"), Some("Stereo Out"), Some("Bus 1"))
        );
        assert_eq!(body(&catalog, &json!({"kind": "tree", "ref": "1:track:9"})), Err(refs::not_in_project("1:track:9")));
        assert_eq!(body(&catalog, &json!({"kind": "tree", "ref": "0:track:1"})).unwrap_err().0, "stale");
        assert_eq!(
            body(&catalog, &json!({"kind": "bogus"})),
            Err(bad("Logic Pro has no body of that kind: use one of tree, notes, timeline, routing"))
        );
        assert_eq!(body(&catalog, &json!({"kind": "timeline", "around": "1:track:7"})), Err(refs::not_in_project("1:track:7")));
    }
}
