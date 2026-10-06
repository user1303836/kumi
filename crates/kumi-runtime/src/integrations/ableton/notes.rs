//! Kumi's notation on Live's clips. The write tools take `notation` beside `notes`, turned into notes here before Live
//! is asked anything; `read_notes` prints a clip's notes as notation.
use super::{
    connection::LiveConnection,
    context, more_changes,
    views::{self, ViewHost},
};
use crate::{
    core::contracts::{JsonObject, ToolResult},
    mcp::types::CallToolResult,
    notation::{self, Frame, Note},
};
use kumi_common::{abort::Signal, js::json::stringify};
use serde_json::{json, Value};

/// The fields `read_notes` reads of each note.
const NOTE_FIELDS: [&str; 9] =
    ["pitch", "start", "duration", "velocity", "mute", "probability", "velocityDeviation", "releaseVelocity", "channel"];

/// A write tool's input with its `notation` turned into `notes` (and the clip's length, and an Arrangement clip's
/// start, filled in when left out).
pub async fn expand(
    tool: &str,
    mut input: JsonObject,
    connection: &LiveConnection,
    tempo: Option<f64>,
    signal: &Signal,
) -> Result<JsonObject, String> {
    match tool {
        "write_midi_clip" => {
            if let Some(text) = notation_of(&mut input)? {
                let frame = frame(0., &input, tempo, connection, &text, signal).await;
                let notes = notation::parse(&text, &frame).map_err(|error| error.to_string())?;
                fill(&mut input, &notes, &frame);
            }
        }
        "write_arrangement_clip" => {
            if let Some(text) = notation_of(&mut input)? {
                arrangement(&mut input, &text, tempo, connection, signal).await?;
            }
            if let Some(clips) = input.get_mut("clips").and_then(Value::as_array_mut) {
                for (index, clip) in clips.iter_mut().enumerate() {
                    let Some(clip) = clip.as_object_mut() else { continue };
                    if let Some(text) = notation_of(clip).map_err(|error| format!("clips[{index}]: {error}"))? {
                        arrangement(clip, &text, tempo, connection, signal).await.map_err(|error| format!("clips[{index}]: {error}"))?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(input)
}
/// The notation an input gives (taken out of it), if any.
fn notation_of(input: &mut JsonObject) -> Result<Option<String>, String> {
    match input.remove("notation") {
        None => Ok(None),
        Some(_) if input.contains_key("notes") => Err("give the notes as notes or as notation, not both".into()),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err("notation is text: the notes in Kumi's notation".into()),
    }
}
/// The frame a write's notation is read in: the Set's meter and tempo, the clip's length when given, and the track's
/// Drum Rack pads when lanes are named for drums.
async fn frame(origin: f64, input: &JsonObject, tempo: Option<f64>, connection: &LiveConnection, text: &str, signal: &Signal) -> Frame {
    let (numerator, denominator) = more_changes::meter();
    let track = input.get("trackRef").and_then(Value::as_str);
    let pads = match track {
        Some(track) if notation::names_drums(text) => pads(connection, track, signal).await,
        _ => vec![],
    };
    Frame {
        origin,
        numerator,
        denominator,
        tempo: tempo.unwrap_or(120.),
        length: input.get("length").and_then(Value::as_f64),
        pads,
        drums: false,
    }
}
/// An Arrangement clip's notation, in song time: the clip starts where `start` says, or at the bar of its first note.
async fn arrangement(
    clip: &mut JsonObject,
    text: &str,
    tempo: Option<f64>,
    connection: &LiveConnection,
    signal: &Signal,
) -> Result<(), String> {
    let given = clip.get("start").and_then(Value::as_f64);
    let mut frame = frame(given.unwrap_or(0.), clip, tempo, connection, text, signal).await;
    if given.is_none() {
        frame.length = None;
    }
    let mut notes = notation::parse(text, &frame).map_err(|error| error.to_string())?;
    if given.is_none() {
        let start = notes.first().map_or(0., |note| frame.bar_start(frame.bar_of(note.start)));
        for note in &mut notes {
            note.start -= start;
        }
        frame.origin = start;
        clip.insert("start".into(), json!(start));
        if let Some(length) = clip.get("length").and_then(Value::as_f64) {
            if let Some(note) = notes.iter().find(|note| note.start + note.duration > length + 1e-9) {
                return Err(format!(
                    "a note at {} ends after the clip, which ends at {}: shorten it, or make the clip longer",
                    frame.position(start + note.start),
                    frame.position(start + length)
                ));
            }
        }
    }
    fill(clip, &notes, &frame);
    Ok(())
}
/// The notes into the input, Live's way, and the clip's length when left out: whole bars covering them.
fn fill(input: &mut JsonObject, notes: &[Note], frame: &Frame) {
    if !input.contains_key("length") {
        let end = notes.iter().map(|note| note.start + note.duration).fold(0., f64::max);
        let bars = (end / frame.bar() - 1e-9).ceil().max(1.);
        input.insert("length".into(), json!(bars * frame.bar()));
    }
    input.insert("notes".into(), Value::Array(notes.iter().map(note_json).collect()));
}
/// A note as Live's tools take it; whole velocities as whole numbers, and only the settings it changes.
fn note_json(note: &Note) -> Value {
    let velocity = if note.velocity.fract() == 0. { json!(note.velocity as i64) } else { json!(note.velocity) };
    let mut row = json!({"pitch": note.pitch, "start": note.start, "duration": note.duration, "velocity": velocity});
    if note.mute {
        row["mute"] = json!(true);
    }
    if note.probability != 1. {
        row["probability"] = json!(note.probability);
    }
    if note.velocity_deviation != 0. {
        row["velocityDeviation"] = json!(note.velocity_deviation);
    }
    row
}

/// The pads of the track's Drum Rack that hold something (name and note), which lane names go by; none without one.
pub async fn pads(connection: &LiveConnection, track: &str, signal: &Signal) -> Vec<(String, u8)> {
    let read = |args: Value| views::pages(connection, args.as_object().cloned().unwrap_or_default(), signal.clone());
    let Ok(devices) =
        read(json!({"kind":"device","parent":track,"fields":["className","canHaveDrumPads"],"limit":connection.page_limit()})).await
    else {
        return vec![];
    };
    let Some(rack) = rows(&devices)
        .into_iter()
        .find(|row| row.get("canHaveDrumPads") == Some(&json!(true)))
        .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
    else {
        return vec![];
    };
    let Ok(rack) = read(json!({"kind":"device","filters":{"ref":rack},"fields":["drumPads"],"limit":1})).await else { return vec![] };
    let pads = rows(&rack).into_iter().next().and_then(|row| row.get("drumPads").and_then(Value::as_array).cloned()).unwrap_or_default();
    pads.iter()
        .filter(|pad| pad.get("chains").and_then(Value::as_array).is_some_and(|chains| !chains.is_empty()))
        .filter_map(|pad| Some((pad.get("name")?.as_str()?.to_owned(), u8::try_from(pad.get("note")?.as_u64()?).ok()?)))
        .collect()
}
fn rows(read: &CallToolResult) -> Vec<JsonObject> {
    if read.is_error == Some(true) {
        return vec![];
    }
    let Ok(page) = context::payload(read) else { return vec![] };
    page.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(|row| row.as_object().cloned()).collect()
}

/// `read_notes`: each clip's notes as notation (or, with format "json", as Live's rows).
pub async fn read_notes(input: &JsonObject, connection: &LiveConnection, tempo: Option<f64>, signal: Signal) -> ToolResult {
    let mut named: Vec<String> =
        input.get("clipRefs").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    named.extend(input.get("clipRef").and_then(Value::as_str).map(str::to_owned));
    if named.is_empty() {
        return ToolResult::error("Name a clip: clipRef, or clipRefs for several.");
    }
    let json = input.get("format").and_then(Value::as_str) == Some("json");
    let mut clips = vec![];
    for short in named.iter().take(16) {
        let long = connection.references.borrow().lengthen(&json!({"clipRef": short}))["clipRef"].as_str().unwrap_or(short).to_owned();
        let mut row = match read_clip(connection, &long, json, tempo, &signal).await {
            Ok(row) => row,
            Err(error) => json!({"error": error}).as_object().cloned().unwrap_or_default(),
        };
        row.insert("clip".into(), json!(short));
        clips.push(row);
    }
    ToolResult::text(stringify(&json!({"clips": clips})))
}
/// One clip's notes, printed in its frame: song time for an Arrangement clip, its own time (and meter) for a Session
/// clip, with drums as lanes on a Drum Rack track.
async fn read_clip(connection: &LiveConnection, clip: &str, json: bool, tempo: Option<f64>, signal: &Signal) -> Result<JsonObject, String> {
    let read = |args: Value| views::pages(connection, args.as_object().cloned().unwrap_or_default(), signal.clone());
    let wrong =
        || format!("“{clip}” isn't a clip's ref from this turn: take one from the observation or a read (clip:… or arrangement_clip:…)");
    let parts: Vec<&str> = clip.split(':').collect();
    let (session, epoch, track) = match parts[..] {
        [epoch, "clip", track, _] => (true, epoch, track),
        [epoch, "arrangement_clip", track, _] => (false, epoch, track),
        _ => return Err(wrong()),
    };
    let track = format!("{epoch}:track:{track}");
    let row = if session {
        let slot = clip.replacen(":clip:", ":clip_slot:", 1);
        let fields = ["name", "length", "isAudio", "signatureNumerator", "signatureDenominator"];
        let read = read(json!({"kind":"session-clip","parent":slot,"fields":fields,"limit":1})).await.map_err(|error| error.to_string())?;
        rows(&read).into_iter().next()
    } else {
        let read = read(
            json!({"kind":"arrangement-clip","parent":track,"fields":["name","start","length","isAudio"],"limit":connection.page_limit()}),
        )
        .await
        .map_err(|error| error.to_string())?;
        rows(&read).into_iter().find(|row| row.get("ref").and_then(Value::as_str) == Some(clip))
    };
    let row = row.ok_or_else(wrong)?;
    if row.get("isAudio") == Some(&json!(true)) {
        return Err("that's an audio clip: it has no notes".into());
    }
    let read = read(json!({"kind":"note","parent":clip,"fields":NOTE_FIELDS,"limit":connection.page_limit()}))
        .await
        .map_err(|error| error.to_string())?;
    let found = rows(&read);
    let name = row.get("name").cloned().unwrap_or(Value::Null);
    if json {
        return Ok(json!({"name": name, "notes": found}).as_object().cloned().unwrap_or_default());
    }
    let notes: Vec<Note> = found.iter().filter_map(note_of).collect();
    let (numerator, denominator) =
        match (row.get("signatureNumerator").and_then(Value::as_u64), row.get("signatureDenominator").and_then(Value::as_u64)) {
            (Some(numerator), Some(denominator)) if session && numerator > 0 && denominator > 0 => (numerator as u32, denominator as u32),
            _ => more_changes::meter(),
        };
    let pads = pads(connection, &track, signal).await;
    let origin = if session { 0. } else { row.get("start").and_then(Value::as_f64).unwrap_or(0.) };
    let length = row.get("length").and_then(Value::as_f64);
    let frame = Frame { origin, numerator, denominator, tempo: tempo.unwrap_or(120.), length, drums: !pads.is_empty(), pads };
    let printed = notation::print(&notes, &frame);
    let time = match (session, length) {
        (true, _) => format!("the clip's own time, in {numerator}/{denominator}"),
        (false, Some(length)) => format!("song time: the clip runs from {} to {}", frame.position(origin), frame.position(origin + length)),
        (false, None) => format!("song time: the clip starts at {}", frame.position(origin)),
    };
    Ok(json!({"name": name, "time": time, "notes": notes.len(), "exact": printed.exact, "notation": printed.text})
        .as_object()
        .cloned()
        .unwrap_or_default())
}
/// A note from a row Live read.
fn note_of(row: &JsonObject) -> Option<Note> {
    let number = |field: &str| row.get(field).and_then(Value::as_f64);
    Some(Note {
        pitch: u8::try_from(row.get("pitch")?.as_u64()?).ok()?,
        start: number("start")?,
        duration: number("duration")?,
        velocity: number("velocity")?,
        mute: row.get("mute").and_then(Value::as_bool).unwrap_or(false),
        probability: number("probability").unwrap_or(1.),
        velocity_deviation: number("velocityDeviation").unwrap_or(0.),
        release_velocity: number("releaseVelocity"),
        channel: row.get("channel").and_then(Value::as_u64).and_then(|channel| u8::try_from(channel).ok()),
    })
}
