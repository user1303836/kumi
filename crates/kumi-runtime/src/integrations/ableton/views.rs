//! Live's paged collections and the producer's FOCUS views.
use super::concurrent::eager_all;
use super::{
    bridge_version::{at_least, SCALE_BRIDGE},
    context::payload,
};
use crate::{
    core::{contracts::*, errors::RuntimeError},
    mcp::types::CallToolResult,
};
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{json::stringify, string::head},
};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::LazyLock};

/// Internal reads bypass the model's reference authority and never register their own references.
#[async_trait(?Send)]
pub trait ViewHost {
    fn available(&self) -> bool;
    fn has(&self, name: &str) -> bool;
    fn version(&self) -> Option<String>;
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError>;
    fn page_limit(&self) -> usize {
        if at_least(self.version().as_deref(), SCALE_BRIDGE) {
            100_000
        } else {
            100
        }
    }
    fn whole_budget(&self) -> usize {
        if at_least(self.version().as_deref(), SCALE_BRIDGE) {
            10_000_000
        } else {
            1000
        }
    }
}
pub(crate) fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
pub(crate) fn rows(body: &JsonObject) -> Vec<Value> {
    body.get("items").and_then(Value::as_array).cloned().unwrap_or_default()
}
fn cursor(body: &JsonObject) -> Option<String> {
    body.get("nextCursor").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}
pub(crate) fn wrapped(body: JsonObject) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&Value::Object(body.clone()))}],"structuredContent":body}))
        .unwrap()
}
fn failure(text: &str) -> CallToolResult {
    serde_json::from_value(json!({"isError":true,"content":[{"type":"text","text":text}]})).unwrap()
}
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(Value::Object(_) | Value::Array(_)), _) | (_, Some(Value::Object(_) | Value::Array(_))) => false,
        _ => a == b,
    }
}
/// Every page up to the requested row limit; repeating cursors and changed epochs fail the read.
pub async fn pages(host: &dyn ViewHost, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
    let first = host.call("live_discover", args.clone(), signal.clone()).await?;
    if first.is_error == Some(true) {
        return Ok(first);
    }
    let Ok(mut body) = payload(&first) else { return Ok(first) };
    let Some(mut next) = cursor(&body) else { return Ok(first) };
    let mut items = rows(&body);
    let limit = args.get("limit").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    let mut seen = HashSet::from([next.clone()]);
    let mut remaining = true;
    for _ in 1..100_000 {
        if !remaining || items.len() as f64 >= limit {
            break;
        }
        let mut query = args.clone();
        query.insert("cursor".into(), json!(next));
        let more = host.call("live_discover", query, signal.clone()).await?;
        if more.is_error == Some(true) {
            return Ok(more);
        }
        let page = payload(&more)?;
        if !same(page.get("epoch"), body.get("epoch")) || !same(page.get("kind"), body.get("kind")) {
            return Ok(failure("Live changed while Kumi read it; read it again"));
        }
        items.extend(rows(&page));
        match cursor(&page) {
            Some(value) if !seen.insert(value.clone()) => return Ok(failure("Live's pages of that read didn't end; read it again")),
            Some(value) => next = value,
            None => remaining = false,
        }
    }
    body.shift_remove("nextCursor");
    body.insert("items".into(), json!(items));
    body.insert("truncated".into(), json!(remaining));
    if remaining {
        body.insert("nextCursor".into(), json!(next));
    }
    Ok(wrapped(body))
}
/// The most notes a clip's view draws.
const CLIP_VIEW_NOTES: usize = 512;
static TRACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:track:[0-9]+$").unwrap());
static SLOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:clip_slot:[0-9]+:[0-9]+$").unwrap());
fn readable(host: &dyn ViewHost) -> bool {
    host.available() && host.has("live_discover")
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(a)) => {
            a.iter().map(|v| if v.is_null() { String::new() } else { js_string(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(v) => stringify(v),
    }
}
fn node(row: Value) -> DeviceNode {
    DeviceNode {
        r#ref: js_string(row.get("ref")),
        name: row.get("name").and_then(Value::as_str).map(|s| head(s, 256)).unwrap_or_else(|| "Device".into()),
        class_name: row.get("className").and_then(Value::as_str).map(|s| head(s, 128)),
        can_have_chains: row.get("canHaveChains").and_then(Value::as_bool),
        can_have_drum_pads: row.get("canHaveDrumPads").and_then(Value::as_bool),
        device_type: match row.get("deviceType").and_then(Value::as_str) {
            Some("instrument") => Some(DeviceType::Instrument),
            Some("audio_effect") => Some(DeviceType::AudioEffect),
            Some("midi_effect") => Some(DeviceType::MidiEffect),
            _ => None,
        },
        chains: row.get("chainList").and_then(Value::as_array).map(|chains| {
            chains
                .iter()
                .filter_map(|chain| {
                    Some(ChainNode {
                        r#ref: chain.get("ref")?.as_str()?.into(),
                        name: chain.get("name").and_then(Value::as_str).map(|s| head(s, 256)).unwrap_or_else(|| "Chain".into()),
                        devices: None,
                    })
                })
                .take(128)
                .collect()
        }),
    }
}
async fn read_devices(host: &dyn ViewHost, parent: &str, signal: Signal) -> Result<Vec<DeviceNode>, RuntimeError> {
    let mut all = Vec::new();
    let mut next = None;
    for _ in 0..10_000 {
        let mut args = object(
            json!({"kind":"device","parent":parent,"fields":["parentRef","name","className","canHaveChains","canHaveDrumPads","chainList","deviceType"],"limit":host.page_limit()}),
        );
        if let Some(next) = next {
            args.insert("cursor".into(), json!(next));
        }
        let read = host.call("live_discover", args, signal.clone()).await?;
        if read.is_error == Some(true) {
            return Err(RuntimeError::plain("Live didn't list the devices"));
        }
        let body = payload(&read)?;
        all.extend(rows(&body).into_iter().map(node));
        next = cursor(&body);
        if next.is_none() {
            break;
        }
    }
    Ok(all)
}
// Breadth-first traversal preserves the source's parallel reads and its depth/size bounds.
fn chain_paths(devices: &[DeviceNode], base: &[usize]) -> Vec<(Vec<usize>, String)> {
    let mut out = Vec::new();
    for (d, device) in devices.iter().enumerate().filter(|(_, d)| d.can_have_drum_pads != Some(true)) {
        for (c, chain) in device.chains.as_deref().unwrap_or_default().iter().enumerate() {
            let mut path = base.to_vec();
            path.extend([d, c]);
            out.push((path, chain.r#ref.clone()));
        }
    }
    out
}
fn chain_mut<'a>(devices: &'a mut [DeviceNode], path: &[usize]) -> &'a mut ChainNode {
    let chain = &mut devices[path[0]].chains.as_mut().unwrap()[path[1]];
    if path.len() == 2 {
        chain
    } else {
        chain_mut(chain.devices.as_mut().unwrap(), &path[2..])
    }
}
pub async fn device_tree(host: &dyn ViewHost, track_ref: &str, signal: Signal) -> Result<Option<DeviceTree>, RuntimeError> {
    if !readable(host) || !TRACK.is_match(track_ref) {
        return Ok(None);
    }
    let result: Result<Option<DeviceTree>, RuntimeError> = async {
        let mut devices = read_devices(host, track_ref, signal.clone()).await?;
        let mut level = chain_paths(&devices, &[]);
        let mut count = devices.len();
        for _ in 0..32 {
            if level.is_empty() || count >= 100_000 {
                break;
            }
            let reads = eager_all(level.iter().map(|(_, reference)| read_devices(host, reference, signal.clone()))).await?;
            let mut next = Vec::new();
            for ((path, _), read) in level.into_iter().zip(reads) {
                let children = read;
                count += children.len();
                next.extend(chain_paths(&children, &path));
                chain_mut(&mut devices, &path).devices = Some(children);
            }
            level = next;
        }
        Ok(Some(DeviceTree { track_ref: track_ref.into(), devices }))
    }
    .await;
    match result {
        Ok(value) => Ok(value),
        Err(_) => {
            signal.check()?;
            Ok(None)
        }
    }
}
pub async fn session_strip(host: &dyn ViewHost, track_ref: &str, scene: f64, signal: Signal) -> Result<Option<SessionStrip>, RuntimeError> {
    if !readable(host) || !TRACK.is_match(track_ref) {
        return Ok(None);
    }
    let result:Result<Option<SessionStrip>,RuntimeError>=async {
        let read=pages(host,object(json!({"kind":"clip-slot","parent":track_ref,"fields":["sceneIndex","clipRef","playingStatus"],"limit":host.page_limit(),"budget":host.whole_budget()})),signal.clone()).await?;
        if read.is_error==Some(true) {return Ok(None)}
        let rows=rows(&payload(&read)?);let start_number=if scene.is_nan(){f64::NAN}else{0f64.max((rows.len() as f64-7.).min(scene-3.))};
        let start=start_number.trunc() as usize;
        let end=(start_number+7.).trunc() as usize;
        let window:Vec<_>=rows.into_iter().skip(start).take(end.saturating_sub(start)).collect();
        let clips=eager_all(window.iter().map(|row|async {
            if row.get("clipRef").and_then(Value::as_str).is_none()||row.get("ref").and_then(Value::as_str).is_none() {return Ok::<_,RuntimeError>(None)}
            let found=host.call("live_discover",object(json!({"kind":"session-clip","parent":row["ref"],"fields":["name","isAudio"],"limit":1})),signal.clone()).await.ok();
            let clip=match found.filter(|read|read.is_error!=Some(true)) {Some(read)=>rows_first(&payload(&read)?),None=>None};
            Ok(Some(SlotClip{name:clip.as_ref().and_then(|c|c.get("name")).and_then(Value::as_str).map(|s|head(s,256)).unwrap_or_default(),audio:clip.as_ref().and_then(|c|c.get("isAudio"))==Some(&Value::Bool(true))}))
        })).await?;
        let mut slots=Vec::new();for (index,(row,clip)) in window.into_iter().zip(clips).enumerate(){slots.push(SessionSlot{index:row.get("sceneIndex").and_then(Value::as_f64).unwrap_or(start_number+index as f64),clip,playing:(row.get("playingStatus").and_then(Value::as_f64)==Some(1.)).then_some(true),queued:(row.get("playingStatus").and_then(Value::as_f64)==Some(2.)).then_some(true)});}
        Ok(Some(SessionStrip{track_ref:track_ref.into(),scene,slots}))
    }.await;
    match result {
        Ok(value) => Ok(value),
        Err(_) => {
            signal.check()?;
            Ok(None)
        }
    }
}
fn rows_first(body: &JsonObject) -> Option<Value> {
    body.get("items").and_then(Value::as_array).and_then(|a| a.first()).cloned()
}
pub async fn arrangement_strip(host: &dyn ViewHost, signal: Signal) -> Result<Option<ArrangementStrip>, RuntimeError> {
    if !readable(host) {
        return Ok(None);
    }
    let result: Result<Option<ArrangementStrip>, RuntimeError> = async {
        let mut results = eager_all(vec![
            host.call("live_discover", object(json!({"kind":"set","fields":["position","loop","playing"],"limit":1})), signal.clone())
                .map(|r| r.map(Some))
                .boxed_local(),
            pages(
                host,
                object(json!({"kind":"locator","fields":["name","position"],"limit":host.page_limit(),"budget":host.whole_budget()})),
                signal.clone(),
            )
            .map(|r| r.map(Some))
            .boxed_local(),
            async {
                if host.has("live_song_state") {
                    host.call("live_song_state", JsonObject::new(), signal.clone()).await.map(Some)
                } else {
                    Ok(None)
                }
            }
            .boxed_local(),
        ])
        .await?
        .into_iter();
        let set = results.next().unwrap().unwrap();
        let locators = results.next().unwrap().unwrap();
        let song = results.next().unwrap();
        if set.is_error == Some(true) {
            return Ok(None);
        }
        let Some(row) = rows_first(&payload(&set)?) else { return Ok(None) };
        let Some(position) = row.get("position").and_then(Value::as_f64) else { return Ok(None) };
        let length = match song.filter(|s| s.is_error != Some(true)) {
            Some(song) => payload(&song)?.get("songLength").and_then(Value::as_f64),
            None => None,
        };
        let marks = if locators.is_error == Some(true) {
            Vec::new()
        } else {
            rows(&payload(&locators)?)
                .iter()
                .filter_map(|row| {
                    Some(Locator {
                        name: row.get("name").and_then(Value::as_str).map(|s| head(s, 128)).unwrap_or_default(),
                        position: row.get("position")?.as_f64()?,
                    })
                })
                .collect::<Vec<_>>()
        };
        Ok(Some(ArrangementStrip {
            length: length.filter(|n| *n > 0.).unwrap_or_else(|| marks.iter().fold(position.max(16.), |n, m| n.max(m.position))),
            position,
            playing: row.get("playing") == Some(&Value::Bool(true)),
            r#loop: row.get("loop").and_then(|v| {
                Some(ArrangementLoop {
                    length: v.get("length")?.as_f64()?,
                    start: v.get("start").and_then(Value::as_f64).unwrap_or(0.),
                    enabled: v.get("enabled") == Some(&Value::Bool(true)),
                })
            }),
            locators: marks,
        }))
    }
    .await;
    match result {
        Ok(value) => Ok(value),
        Err(_) => {
            signal.check()?;
            Ok(None)
        }
    }
}
pub async fn clip_view(host: &dyn ViewHost, slot_ref: &str, signal: Signal) -> Result<Option<ClipView>, RuntimeError> {
    if !readable(host) || !SLOT.is_match(slot_ref) {
        return Ok(None);
    }
    let clip_ref = slot_ref.replacen(":clip_slot:", ":clip:", 1);
    let result: Result<Option<ClipView>, RuntimeError> = async {
        let clips = host
            .call(
                "live_discover",
                object(json!({"kind":"session-clip","parent":slot_ref,"fields":["name","length","isAudio"],"limit":1})),
                signal.clone(),
            )
            .await?;
        if clips.is_error == Some(true) {
            return Ok(None);
        }
        let Some(clip) = rows_first(&payload(&clips)?) else { return Ok(None) };
        let Some(length) = clip.get("length").and_then(Value::as_f64).filter(|n| *n > 0.) else { return Ok(None) };
        if clip.get("isAudio") == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        let mut notes = Vec::new();
        let mut next = None;
        for _ in 0..10_000 {
            // The view draws the first CLIP_VIEW_NOTES: no more are asked for.
            if notes.len() >= CLIP_VIEW_NOTES {
                break;
            }
            let mut args = object(json!({"kind":"note","parent":clip_ref,"limit":host.page_limit().min(CLIP_VIEW_NOTES - notes.len())}));
            if let Some(next) = next {
                args.insert("cursor".into(), json!(next));
            }
            let read = host.call("live_discover", args, signal.clone()).await?;
            if read.is_error == Some(true) {
                break;
            }
            let body = payload(&read)?;
            notes.extend(rows(&body));
            next = cursor(&body);
            if next.is_none() {
                break;
            }
        }
        // The selected notes' ids (as their bits: ids are whole numbers), looked up once for each note drawn.
        let mut selected = std::collections::HashSet::new();
        if host.has("live_note_read") {
            if let Ok(read) = host.call("live_note_read", object(json!({"clipRef":clip_ref,"selected":true})), signal.clone()).await {
                if read.is_error != Some(true) {
                    if let Some(notes) = payload(&read)?.get("notes").and_then(Value::as_array) {
                        selected = notes.iter().filter_map(|n| n.get("id").and_then(Value::as_f64)).map(f64::to_bits).collect();
                    }
                }
            }
        }
        Ok(Some(ClipView {
            slot_ref: slot_ref.into(),
            name: clip.get("name").and_then(Value::as_str).map(|s| head(s, 256)).unwrap_or_default(),
            length,
            notes: notes
                .iter()
                .take(CLIP_VIEW_NOTES)
                .filter_map(|note| {
                    Some(ClipViewNote {
                        note: ClipNote {
                            pitch: note.get("pitch")?.as_f64()?,
                            start: note.get("start")?.as_f64()?,
                            duration: note.get("duration")?.as_f64()?,
                            velocity: note.get("velocity").and_then(Value::as_f64).unwrap_or(100.),
                        },
                        selected: note.get("id").and_then(Value::as_f64).filter(|id| selected.contains(&id.to_bits())).map(|_| true),
                    })
                })
                .collect(),
        }))
    }
    .await;
    match result {
        Ok(value) => Ok(value),
        Err(_) => {
            signal.check()?;
            Ok(None)
        }
    }
}
