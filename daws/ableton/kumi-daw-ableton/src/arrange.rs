//! Compile the producer's clips into a form, then build it as one guarded change.
use crate::core::{
    contracts::{JsonObject, ToolResult},
    errors::RuntimeError,
};
use async_trait::async_trait;
use futures::{
    future::{join_all, LocalBoxFuture},
    FutureExt,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
        string::{head, trim, utf16_len},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::LazyLock};
mod compile;
mod run;
pub use compile::{compile, describe, is_part, section_lines, Shortens};
pub use run::{arrange, build};
pub const ARRANGE_TOOL: &str = "arrange";
pub static ARRANGE_SCHEMA: LazyLock<JsonObject> =
    LazyLock::new(|| serde_json::from_str::<Value>(include_str!("arrange-data.json")).unwrap()["schema"].as_object().unwrap().clone());
pub static ARRANGE_DESCRIPTION: LazyLock<String> =
    LazyLock::new(|| serde_json::from_str::<Value>(include_str!("arrange-data.json")).unwrap()["description"].as_str().unwrap().into());
const EPSILON: f64 = 1e-6;
const SHORTEST: f64 = 0.25;
const LISTING_BYTES: usize = 40_000;
const MOST_CLIPS: usize = 512;
fn record(value: Option<&Value>) -> &JsonObject {
    super::changes::record(value)
}
fn number(value: Option<&Value>) -> Option<f64> {
    super::changes::finite(value)
}
fn integer(value: Option<&Value>) -> Option<f64> {
    number(value).filter(|n| *n >= 0.0 && n.fract() == 0.0)
}
fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).map(trim).filter(|s| !s.is_empty())
}
fn fields(names: &[&str]) -> JsonObject {
    json!({"fields":names}).as_object().unwrap().clone()
}
fn extra(parent: &str, names: &[&str]) -> JsonObject {
    json!({"parent":parent,"fields":names}).as_object().unwrap().clone()
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipChoice {
    pub track: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    pub beats: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionRequest {
    pub name: String,
    pub bars: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<ClipChoice>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<Gap>,
    pub fill: Vec<ClipChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub riser: Option<ClipChoice>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopRequest {
    pub from_bar: f64,
    pub bars: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrangeRequest {
    pub sections: Vec<SectionRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_bar: Option<f64>,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_: Option<LoopRequest>,
    pub r#final: bool,
}
fn choice(value: &Value, where_: &str) -> Result<ClipChoice, String> {
    if let Some(value) = value.as_str() {
        return if trim(value).is_empty() {
            Err(format!("{where_}: name the track."))
        } else {
            Ok(ClipChoice { track: trim(value).into(), scene: None })
        };
    }
    let row = record(Some(value));
    let track =
        text(row.get("track")).ok_or_else(|| format!("{where_}: give the track (its name or ref) and the scene its clip is in."))?;
    Ok(ClipChoice { track: track.into(), scene: integer(row.get("scene")) })
}
pub fn arrange_request(input: &JsonObject) -> Result<ArrangeRequest, String> {
    let empty = vec![];
    let given = match input.get("sections") {
        None => &empty,
        Some(Value::Array(items)) => items,
        _ => return Err("sections is a list: each section's name and bars, and which tracks play.".into()),
    };
    let mut sections = vec![];
    for (index, raw) in given.iter().enumerate() {
        let row = record(Some(raw));
        let where_ = format!("Section {}", index + 1);
        let (name, length) = match (text(row.get("name")), integer(row.get("bars"))) {
            (Some(name), Some(length)) if length > 0.0 => (name, length),
            _ => return Err(format!("{where_}: give its name and its length in bars (a whole number).")),
        };
        let tracks = match row.get("tracks") {
            None => None,
            Some(Value::Array(rows)) => Some(rows.iter().map(|v| choice(v, &where_)).collect::<Result<Vec<_>, _>>()?),
            _ => return Err(format!("{where_}: tracks is a list of track names (or {{track, scene}}).")),
        };
        let gap = if let Some(value) = row.get("gap") {
            let gap = record(Some(value));
            let beats = number(gap.get("beats"))
                .filter(|v| *v > 0.0)
                .ok_or_else(|| format!("{where_}: a gap is how many beats before the section's end the tracks stop (beats)."))?;
            Some(Gap {
                beats,
                tracks: gap
                    .get("tracks")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|v| text(Some(v)).map(str::to_owned)).collect()),
            })
        } else {
            None
        };
        let fills = match row.get("fill") {
            None => vec![],
            Some(Value::Array(items)) => items.iter().collect(),
            Some(v) => vec![v],
        };
        let mut fill = vec![];
        for item in fills {
            let parsed = choice(item, &format!("{where_}'s fill"))?;
            if parsed.scene.is_none() {
                return Err(format!("{where_}'s fill: say which scene the fill's clip is in."));
            }
            fill.push(parsed);
        }
        let riser = if let Some(item) = row.get("riser") {
            let parsed = choice(item, &format!("{where_}'s riser"))?;
            if parsed.scene.is_none() {
                return Err(format!("{where_}'s riser: say which scene the riser's clip is in."));
            }
            Some(parsed)
        } else {
            None
        };
        sections.push(SectionRequest { name: head(name, 48), bars: length, scene: integer(row.get("scene")), tracks, gap, fill, riser });
    }
    let loop_ = if let Some(value) = input.get("loop") {
        let row = record(Some(value));
        match (integer(row.get("from_bar")), integer(row.get("bars"))) {
            (Some(from_bar), Some(bars)) if from_bar > 0.0 && bars > 0.0 => Some(LoopRequest { from_bar, bars }),
            _ => return Err("loop is the bars in the Arrangement to arrange from: from_bar (1 is the first) and bars.".into()),
        }
    } else {
        None
    };
    Ok(ArrangeRequest {
        sections,
        scene: integer(input.get("scene")),
        start_bar: integer(input.get("start_bar")).filter(|n| *n > 0.0),
        loop_,
        r#final: input.get("final") == Some(&json!(true)),
    })
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopNote {
    pub pitch: f64,
    pub start: f64,
    pub duration: f64,
    pub velocity: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity_deviation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_velocity: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceClip {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    pub beats: f64,
    pub loop_start: f64,
    pub audio: bool,
    pub scene: f64,
    pub shortens: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<LoopNote>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceTrack {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    pub clips: Vec<SourceClip>,
    pub busy: Vec<[f64; 2]>,
    pub empty: Vec<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub index: f64,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    pub name: String,
    pub position: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopSpan {
    pub from: f64,
    pub to: f64,
    pub audio: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    pub beats_per_bar: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    pub tracks: Vec<SourceTrack>,
    pub scenes: Vec<Scene>,
    pub locators: Vec<Locator>,
    pub end: f64,
    pub playing: bool,
    pub session_playing: bool,
    pub unread: usize,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_: Option<LoopSpan>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub track: SourceTrack,
    pub clip: SourceClip,
    pub at: f64,
    pub beats: f64,
    pub section: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip)]
    track_index: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedSection {
    pub name: String,
    pub from: f64,
    pub to: f64,
    pub tracks: Vec<String>,
    pub everything: bool,
    pub extras: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub start: f64,
    pub end: f64,
    pub beats_per_bar: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    pub sections: Vec<PlannedSection>,
    pub placements: Vec<Placement>,
    pub locators: Vec<Locator>,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Built {
    pub copies: usize,
    pub parts: usize,
    pub locators: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playhead: Option<String>,
    pub opened: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_in: Option<String>,
    pub notes: Vec<String>,
}
pub struct Made {
    pub id: String,
    pub reference: Option<String>,
}
pub struct QuietBuilt {
    pub value: Built,
    pub ids: Vec<String>,
}
pub struct UndoStep {
    pub opened: bool,
    pub close: Box<dyn FnOnce() -> LocalBoxFuture<'static, Result<(), RuntimeError>>>,
}
#[async_trait(?Send)]
pub trait ArrangeHost {
    fn tempo(&self) -> Option<f64>;
    fn beats_per_bar(&self) -> f64;
    async fn read(&self, kind: &str, extra: JsonObject, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError>;
    fn offers(&self, tool: &str) -> bool;
    async fn change(&self, tool: &str, input: JsonObject, signal: Signal) -> Result<Made, RuntimeError>;
    async fn undo(&self, id: &str, signal: Signal) -> Result<bool, RuntimeError>;
    async fn quietly(&self, work: LocalBoxFuture<'_, Result<Built, RuntimeError>>) -> Result<QuietBuilt, RuntimeError>;
    fn record(&self, title: &str, ids: &[String], apart: &[String]) -> Option<String>;
    async fn undo_step(&self) -> Result<UndoStep, RuntimeError>;
    async fn keep_copy(&self, signal: Signal) -> Result<Option<String>, RuntimeError>;
    fn tell(&self, title: &str);
}
fn scenes_of(request: &ArrangeRequest, with_clips: &[f64]) -> Vec<f64> {
    let mut scenes = vec![];
    for section in &request.sections {
        scenes.extend(section.scene);
        if let Some(tracks) = &section.tracks {
            scenes.extend(tracks.iter().filter_map(|c| c.scene));
        }
        scenes.extend(section.fill.iter().filter_map(|c| c.scene));
        scenes.extend(section.riser.as_ref().and_then(|c| c.scene));
    }
    if request.sections.iter().any(|s| s.scene.is_none()) && request.loop_.is_none() {
        scenes.extend(request.scene.or_else(|| with_clips.iter().copied().reduce(f64::min)));
    }
    scenes
}
pub async fn read_material(
    host: &dyn ArrangeHost,
    signal: Signal,
    loop_: Option<&LoopRequest>,
    request: Option<&ArrangeRequest>,
) -> Result<Material, RuntimeError> {
    let initial = eager_all(vec![
        host.read("set", fields(&["playing"]), signal.clone()),
        host.read("track", fields(&["name", "kind", "playingSlotIndex"]), signal.clone()),
        host.read("scene", fields(&["name", "index"]), signal.clone()),
        async { Ok::<_, RuntimeError>(host.read("locator", fields(&["name", "position"]), signal.clone()).await.unwrap_or_default()) }
            .boxed_local(),
    ])
    .await?;
    let mut initial = initial.into_iter();
    let (sets, track_rows, scene_rows, locator_rows) =
        (initial.next().unwrap(), initial.next().unwrap(), initial.next().unwrap(), initial.next().unwrap());
    let tracks: Vec<_> = track_rows
        .iter()
        .filter(|row| row.get("ref").is_some_and(Value::is_string) && row.get("kind").and_then(Value::as_str) != Some("group"))
        .collect();
    let reads = eager_all(vec![
        eager_all(
            tracks
                .iter()
                .map(|track| host.read("clip-slot", extra(track["ref"].as_str().unwrap(), &["sceneIndex", "clipRef"]), signal.clone())),
        )
        .boxed_local(),
        async {
            Ok::<_, RuntimeError>(
                join_all(tracks.iter().map(|track| async {
                    host.read(
                        "arrangement-clip",
                        extra(
                            track["ref"].as_str().unwrap(),
                            &["name", "start", "endTime", "length", "isAudio", "looping", "loopStart", "loopEnd"],
                        ),
                        signal.clone(),
                    )
                    .await
                    .unwrap_or_default()
                }))
                .await,
            )
        }
        .boxed_local(),
    ])
    .await?;
    let mut reads = reads.into_iter();
    let (slot_rows, arrangement_rows) = (reads.next().unwrap(), reads.next().unwrap());
    let every: Vec<_> = slot_rows
        .iter()
        .enumerate()
        .flat_map(|(index, slots)| {
            slots
                .iter()
                .filter(|slot| slot.get("clipRef").is_some_and(Value::is_string) && slot.get("ref").is_some_and(Value::is_string))
                .map(move |slot| (index, slot))
        })
        .collect();
    let scenes = request
        .filter(|r| !r.sections.is_empty())
        .map(|r| scenes_of(r, &every.iter().map(|(_, slot)| integer(slot.get("sceneIndex")).unwrap_or(-1.0)).collect::<Vec<_>>()));
    let alone: HashSet<_> = slot_rows
        .iter()
        .enumerate()
        .filter(|(_, slots)| slots.iter().filter(|slot| slot.get("clipRef").is_some_and(Value::is_string)).count() == 1)
        .map(|(index, _)| index)
        .collect();
    let filled: Vec<_> = every
        .into_iter()
        .filter(|(index, slot)| {
            scenes.as_ref().is_none_or(|scenes| scenes.contains(&integer(slot.get("sceneIndex")).unwrap_or(-1.0)) || alone.contains(index))
        })
        .collect();
    let clip_rows = eager_all(filled.iter().take(MOST_CLIPS).map(|(_, slot)| async {
        Ok::<_, RuntimeError>(
            host.read(
                "session-clip",
                extra(slot["ref"].as_str().unwrap(), &["name", "length", "looping", "loopStart", "isAudio", "warping"]),
                signal.clone(),
            )
            .await?
            .into_iter()
            .next(),
        )
    }))
    .await?;
    let mut sources: Vec<_> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| SourceTrack {
            reference: track["ref"].as_str().unwrap().into(),
            name: track.get("name").and_then(Value::as_str).map(|s| head(s, 120)).unwrap_or_else(|| format!("Track {}", index + 1)),
            busy: arrangement_rows[index]
                .iter()
                .filter_map(|row| {
                    let start = number(row.get("start"))?;
                    let end = number(row.get("endTime")).unwrap_or(start + number(row.get("length")).unwrap_or(0.0));
                    Some([start, end])
                })
                .collect(),
            clips: vec![],
            empty: slot_rows[index]
                .iter()
                .filter(|row| !row.get("clipRef").is_some_and(Value::is_string))
                .filter_map(|row| integer(row.get("sceneIndex")))
                .collect(),
        })
        .collect();
    for (at, (index, slot)) in filled.iter().take(MOST_CLIPS).enumerate() {
        let Some(clip) = &clip_rows[at] else { continue };
        let Some(reference) = clip.get("ref").and_then(Value::as_str) else { continue };
        let Some(beats) = number(clip.get("length")).filter(|v| *v > 0.0) else { continue };
        let Some(scene) = integer(slot.get("sceneIndex")) else { continue };
        let audio = clip.get("isAudio") == Some(&json!(true));
        sources[*index].clips.push(SourceClip {
            reference: reference.into(),
            name: clip.get("name").and_then(Value::as_str).map(|s| head(s, 120)).unwrap_or_default(),
            beats,
            loop_start: number(clip.get("loopStart")).unwrap_or(0.0),
            audio,
            scene,
            shortens: !audio || clip.get("warping") == Some(&json!(true)),
            notes: None,
        });
    }
    let bpb = host.beats_per_bar();
    let mut span = loop_.map(|l| LoopSpan { from: (l.from_bar - 1.0) * bpb, to: (l.from_bar - 1.0 + l.bars) * bpb, audio: vec![] });
    if let Some(span) = &mut span {
        let additions = eager_all(sources.iter().enumerate().map(|(index, source)| {
            let span = &*span;
            let signal = signal.clone();
            let rows = &arrangement_rows[index];
            async move {
                let inside: Vec<_> = rows
                    .iter()
                    .filter(|row| {
                        let start = number(row.get("start")).unwrap_or(f64::INFINITY);
                        let end = number(row.get("endTime")).unwrap_or(start);
                        start < span.to - EPSILON && end > span.from + EPSILON
                    })
                    .collect();
                if inside.is_empty() {
                    return Ok::<_, RuntimeError>((None, None));
                }
                if inside.iter().any(|row| row.get("isAudio") == Some(&json!(true))) {
                    return Ok((None, Some(source.name.clone())));
                }
                let groups = eager_all(inside.iter().map(|row| async {
                    let rows = host
                        .read(
                            "note",
                            extra(
                                row.get("ref").and_then(Value::as_str).unwrap_or(""),
                                &["pitch", "start", "duration", "velocity", "mute", "probability", "velocityDeviation", "releaseVelocity"],
                            ),
                            signal.clone(),
                        )
                        .await?;
                    Ok::<_, RuntimeError>(loop_notes(row, &rows, span))
                }))
                .await?;
                let mut notes: Vec<_> = groups.into_iter().flatten().collect();
                notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.total_cmp(&b.pitch)));
                Ok((
                    Some(SourceClip {
                        reference: format!("loop:{}", source.reference),
                        name: inside[0]
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(|s| head(s, 120))
                            .unwrap_or_else(|| source.name.clone()),
                        beats: span.to - span.from,
                        loop_start: 0.0,
                        audio: false,
                        scene: -1.0,
                        shortens: true,
                        notes: Some(notes),
                    }),
                    None,
                ))
            }
        }))
        .await?;
        for (index, (clip, audio)) in additions.into_iter().enumerate() {
            if let Some(clip) = clip {
                sources[index].clips.push(clip);
            }
            span.audio.extend(audio);
        }
    }
    let end = sources.iter().flat_map(|s| s.busy.iter().map(|pair| pair[1])).fold(0.0, f64::max);
    Ok(Material {
        beats_per_bar: bpb,
        tempo: host.tempo().filter(|n| *n != 0.0 && !n.is_nan()),
        tracks: sources,
        scenes: scene_rows
            .iter()
            .enumerate()
            .map(|(index, row)| Scene {
                index: integer(row.get("index")).unwrap_or(index as f64),
                name: row.get("name").and_then(Value::as_str).map(|s| head(s, 120)).unwrap_or_default(),
            })
            .collect(),
        locators: locator_rows
            .iter()
            .filter_map(|row| {
                Some(Locator { name: row.get("name").and_then(Value::as_str).unwrap_or("").into(), position: number(row.get("position"))? })
            })
            .collect(),
        end,
        playing: sets.first().and_then(|s| s.get("playing")) == Some(&json!(true)),
        session_playing: tracks.iter().any(|t| integer(t.get("playingSlotIndex")).is_some()),
        unread: filled.len().saturating_sub(MOST_CLIPS),
        loop_: span,
    })
}
fn loop_notes(clip: &JsonObject, rows: &[JsonObject], span: &LoopSpan) -> Vec<LoopNote> {
    let start = number(clip.get("start")).unwrap_or(0.0);
    let end = number(clip.get("endTime")).unwrap_or(start + number(clip.get("length")).unwrap_or(0.0));
    let loop_start = number(clip.get("loopStart")).unwrap_or(0.0);
    let period =
        number(clip.get("loopEnd")).filter(|n| clip.get("looping") == Some(&json!(true)) && *n > loop_start).map(|n| n - loop_start);
    let mut notes = vec![];
    for row in rows {
        let (Some(pitch), Some(at), Some(duration)) = (number(row.get("pitch")), number(row.get("start")), number(row.get("duration")))
        else {
            continue;
        };
        if duration <= 0.0 {
            continue;
        }
        let (count, first) = if let Some(period) = period {
            if at < loop_start || at >= loop_start + period {
                continue;
            }
            ((((end - start) / period).ceil() + 1.0).max(0.0) as usize, start + at - loop_start)
        } else {
            (1, start + at)
        };
        for turn in 0..count {
            let time = first + turn as f64 * period.unwrap_or(0.0);
            if time < start.max(span.from) - EPSILON || time >= end.min(span.to) - EPSILON {
                continue;
            }
            notes.push(LoopNote {
                pitch,
                start: time - span.from,
                duration: duration.min(span.to - time),
                velocity: number(row.get("velocity")).unwrap_or(100.0),
                mute: row.get("mute").and_then(Value::as_bool),
                probability: number(row.get("probability")),
                velocity_deviation: number(row.get("velocityDeviation")),
                release_velocity: number(row.get("releaseVelocity")),
            });
        }
    }
    notes
}

use super::concurrent::eager_all;

fn ordered(value: Value, keys: &[&str]) -> Value {
    let mut remaining = value.as_object().unwrap().clone();
    let mut result = JsonObject::new();
    for key in keys {
        if let Some(value) = remaining.shift_remove(*key) {
            result.insert((*key).into(), value);
        }
    }
    result.extend(remaining);
    Value::Object(result)
}
