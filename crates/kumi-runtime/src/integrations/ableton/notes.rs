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
/// The fields `read_notes` reads of an Arrangement clip: where it sits, and the part of its notes it plays.
const ARRANGEMENT_FIELDS: [&str; 9] = ["name", "start", "endTime", "length", "isAudio", "looping", "loopStart", "loopEnd", "startMarker"];
/// The most clips one `read_notes` reads.
const CLIPS: usize = 16;

/// A write tool's input with its notation read: the notes Live takes, and what Kumi read for itself in it.
#[derive(Debug)]
pub struct Expanded {
    pub input: JsonObject,
    /// What Kumi read for itself in the notation (a pitch's octave, a note past the clip's end), said with the change.
    pub fixed: Vec<String>,
    /// The clips of a several-clip write whose notation has mistakes, by their place in `clips`, each with them: the
    /// rest are written, so a fix resends one clip rather than all of them (#257).
    pub unwritten: Vec<Value>,
}
/// A write tool's input with its `notation` turned into `notes` (and the clip's length, and an Arrangement clip's
/// start, filled in when left out). A mistake comes back with every other the text has, so one fix mends them all.
pub async fn expand(
    tool: &str,
    mut input: JsonObject,
    connection: &LiveConnection,
    tempo: Option<f64>,
    signal: &Signal,
) -> Result<Expanded, String> {
    let (mut fixed, mut unwritten) = (vec![], vec![]);
    match tool {
        "write_midi_clip" => {
            if let Some(text) = notation_of(&mut input)? {
                let frame = frame(0., &input, tempo, connection, &text, signal).await;
                let reading = notation::read(&text, &frame).map_err(|errors| notation::errors_text(&errors))?;
                fill(&mut input, &reading.notes, &frame);
                fixed = reading.fixed;
            }
        }
        "write_arrangement_clip" => {
            if let Some(text) = notation_of(&mut input)? {
                fixed = arrangement(&mut input, &text, tempo, connection, signal).await?;
            }
            if let Some(clips) = input.get_mut("clips").and_then(Value::as_array_mut) {
                let mut kept = Vec::with_capacity(clips.len());
                for (index, mut clip) in std::mem::take(clips).into_iter().enumerate() {
                    let read = match clip.as_object_mut() {
                        Some(row) => match notation_of(row) {
                            Ok(Some(text)) => arrangement(row, &text, tempo, connection, signal).await,
                            Ok(None) => Ok(vec![]),
                            Err(error) => Err(error),
                        },
                        None => Ok(vec![]),
                    };
                    match read {
                        Ok(read) => {
                            fixed.extend(read.into_iter().map(|said| format!("clips[{index}]: {said}")));
                            kept.push(clip);
                        }
                        Err(error) => unwritten.push(json!({"clip":index,"error":error})),
                    }
                }
                // None to write: the whole write is refused, with every clip's mistakes.
                if kept.is_empty() && !unwritten.is_empty() {
                    let all: Vec<String> = unwritten
                        .iter()
                        .map(|clip| format!("clips[{}]: {}", clip["clip"], clip["error"].as_str().unwrap_or_default()))
                        .collect();
                    return Err(all.join("\n"));
                }
                *clips = kept;
            }
        }
        _ => {}
    }
    Ok(Expanded { input, fixed, unwritten })
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
/// The frame a write's notation is read in: the Set's meter, tempo and scale (the key roman numerals start in), the
/// clip's length when given, and the track's Drum Rack pads when lanes are named for drums.
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
        key: more_changes::scale().as_deref().and_then(notation::Key::parse),
    }
}
/// An Arrangement clip's notation, in song time: the clip starts where `start` says, or at the bar of its first note.
/// What Kumi read for itself comes back.
async fn arrangement(
    clip: &mut JsonObject,
    text: &str,
    tempo: Option<f64>,
    connection: &LiveConnection,
    signal: &Signal,
) -> Result<Vec<String>, String> {
    let given = clip.get("start").and_then(Value::as_f64);
    let mut frame = frame(given.unwrap_or(0.), clip, tempo, connection, text, signal).await;
    if given.is_none() {
        frame.length = None;
    }
    let reading = notation::read(text, &frame).map_err(|errors| notation::errors_text(&errors))?;
    let (mut notes, mut fixed) = (reading.notes, reading.fixed);
    if given.is_none() {
        let start = notes.first().map_or(0., |note| frame.bar_start(frame.bar_of(note.start)));
        for note in &mut notes {
            note.start -= start;
        }
        frame.origin = start;
        clip.insert("start".into(), json!(start));
        // The clip's length was given without its start: notes past its end are left out, or cut there.
        if let Some(length) = clip.get("length").and_then(Value::as_f64) {
            let (mut left_out, mut cut) = (vec![], 0);
            notes.retain_mut(|note| {
                if note.start > length - 1e-9 {
                    left_out.push(frame.position(start + note.start));
                    return false;
                }
                if note.start + note.duration > length + 1e-9 {
                    note.duration = length - note.start;
                    cut += 1;
                }
                true
            });
            fixed.extend(notation::past_the_end(&left_out, cut, &frame.position(start + length)));
        }
    }
    fill(clip, &notes, &frame);
    Ok(fixed)
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
    for short in named.iter().take(CLIPS) {
        let long = connection.references.borrow().lengthen(&json!({"clipRef": short}))["clipRef"].as_str().unwrap_or(short).to_owned();
        let mut row = match read_clip(connection, &long, json, tempo, &signal).await {
            Ok(row) => row,
            Err(error) => json!({"error": error}).as_object().cloned().unwrap_or_default(),
        };
        row.insert("clip".into(), json!(short));
        clips.push(row);
    }
    let mut result = json!({"clips": clips});
    if named.len() > CLIPS {
        result["more"] = json!(format!("{CLIPS} clips at a time: ask again for the other {}", named.len() - CLIPS));
    }
    ToolResult::text(stringify(&result))
}
/// One clip's notes, printed in its frame: song time for an Arrangement clip (what it plays, where it plays it), its
/// own time (and meter) for a Session clip, with drums as lanes on a Drum Rack track.
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
        let read = read(json!({"kind":"arrangement-clip","parent":track,"fields":ARRANGEMENT_FIELDS,"limit":connection.page_limit()}))
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
    // A clip past the page's limit is read only in part: said, so a write-back doesn't lose the rest.
    let partial = (context::payload(&read).ok().and_then(|page| page.get("truncated").and_then(Value::as_bool)) == Some(true))
        .then(|| format!("only its first {} notes were read: a clip this big is read in part", found.len()));
    let name = row.get("name").cloned().unwrap_or(Value::Null);
    if json {
        let mut result = json!({"name": name, "notes": found});
        if !session {
            // Live's rows are in the clip's own time; this says where the clip plays them.
            let placed: JsonObject = ARRANGEMENT_FIELDS[1..]
                .iter()
                .filter(|field| **field != "isAudio")
                .filter_map(|field| Some((field.to_string(), row.get(*field)?.clone())))
                .collect();
            result["time"] = json!("the clip's own time, in beats; placement says where it plays them (its start marker at its start)");
            result["placement"] = Value::Object(placed);
        }
        if let Some(partial) = partial {
            result["truncated"] = json!(partial);
        }
        return Ok(result.as_object().cloned().unwrap_or_default());
    }
    let notes: Vec<Note> = found.iter().filter_map(note_of).collect();
    let (numerator, denominator) =
        match (row.get("signatureNumerator").and_then(Value::as_u64), row.get("signatureDenominator").and_then(Value::as_u64)) {
            (Some(numerator), Some(denominator)) if session && numerator > 0 && denominator > 0 => (numerator as u32, denominator as u32),
            _ => more_changes::meter(),
        };
    let pads = pads(connection, &track, signal).await;
    let start = row.get("start").and_then(Value::as_f64).unwrap_or(0.);
    let frame = |origin: f64, length: Option<f64>| Frame {
        origin,
        numerator,
        denominator,
        tempo: tempo.unwrap_or(120.),
        length,
        drums: !pads.is_empty(),
        pads: pads.clone(),
        key: None,
    };
    let mut result = JsonObject::new();
    // Where a looped clip starts in its loop, when the bridge doesn't say (it's placed from the loop's start).
    let mut guessed = None;
    let (shown, frame) = if session {
        result.insert("time".into(), json!(format!("the clip's own time, in {numerator}/{denominator}")));
        (notes, frame(0., row.get("length").and_then(Value::as_f64)))
    } else {
        let song = frame(start, None);
        match played(&notes, &row) {
            Some(Played { notes: heard, unheard, span, looping, assumed }) => {
                let mut time = format!(
                    "song time in {numerator}/{denominator}: the clip plays from {} to {}",
                    song.position(start),
                    song.position(start + span)
                );
                if looping {
                    time += ", looping: each pass is shown";
                }
                if assumed {
                    guessed =
                        Some("this bridge doesn't say where in its loop the clip starts, so it's placed from the loop's start".to_owned());
                }
                result.insert("time".into(), json!(time));
                if unheard > 0 {
                    result.insert(
                        "unheard".into(),
                        json!(format!(
                            "{unheard} of its notes lie outside what it plays (before its start or past its end), so they aren't shown"
                        )),
                    );
                }
                (heard, frame(start, Some(span)))
            }
            None => {
                let (from, to) = (number(&row, "loopStart").unwrap_or(0.), number(&row, "loopEnd").unwrap_or(0.));
                let own = frame(0., None);
                result.insert(
                    "time".into(),
                    json!(format!(
                        "the clip's own time, in {numerator}/{denominator}: it plays from {} to {} in song time, its loop ({} to {}) repeating too many times to show each pass",
                        song.position(start),
                        song.position(number(&row, "endTime").unwrap_or(start)),
                        own.position(from),
                        own.position(to)
                    )),
                );
                (notes, own)
            }
        }
    };
    let printed = notation::print(&shown, &frame);
    let mut left_out: Vec<String> = printed.left_out.iter().map(|why| format!("{why} (format \"json\" has them)")).collect();
    left_out.extend(partial);
    left_out.extend(guessed);
    result.insert("name".into(), name);
    result.insert("notes".into(), json!(shown.len()));
    result.insert("exact".into(), json!(left_out.is_empty()));
    if !left_out.is_empty() {
        result.insert("leftOut".into(), json!(left_out.join("; ")));
    }
    result.insert("notation".into(), json!(printed.text));
    Ok(result)
}
fn number(row: &JsonObject, field: &str) -> Option<f64> {
    row.get(field).and_then(Value::as_f64).filter(|value| value.is_finite())
}
/// What an Arrangement clip plays of its notes.
struct Played {
    /// From the clip's start, cut where the clip (or a loop's pass) ends.
    notes: Vec<Note>,
    /// The notes it never plays: before its start marker, past its end, or never reached.
    unheard: usize,
    /// How long it plays, in beats.
    span: f64,
    looping: bool,
    /// Whether where it starts in its loop was taken to be the loop's start (an older bridge doesn't give the marker).
    assumed: bool,
}
/// What an Arrangement clip plays of its notes, where it plays them: an unlooped clip its window (from its start
/// marker, which Live also gives as its loop start), a looped one its first pass from the start marker and then each
/// pass of its loop (a split moves the marker, not the loop). None when the passes would come to more notes than a
/// print takes.
fn played(notes: &[Note], clip: &JsonObject) -> Option<Played> {
    let start = number(clip, "start").unwrap_or(0.);
    let span = number(clip, "endTime").map(|end| end - start).or_else(|| number(clip, "length")).unwrap_or(0.).max(0.);
    let loop_start = number(clip, "loopStart").unwrap_or(0.);
    let loop_end = number(clip, "loopEnd").filter(|end| *end > loop_start + 1e-9);
    let looping = clip.get("looping") == Some(&json!(true)) && loop_end.is_some();
    // A pass: the notes from `from` to `to` in the clip's own time, heard from `at` on, from its start.
    let mut passes: Vec<(f64, f64, f64)> = vec![];
    let mut assumed = false;
    match loop_end.filter(|_| looping) {
        Some(loop_end) => {
            let marker = number(clip, "startMarker").unwrap_or_else(|| {
                assumed = true;
                loop_start
            });
            let period = loop_end - loop_start;
            let first = (loop_end - marker).max(0.);
            let laps = ((span - first) / period).ceil().max(0.);
            let per_lap = notes.iter().filter(|note| note.start >= loop_start - 1e-9 && note.start < loop_end - 1e-9).count();
            if laps * per_lap as f64 > notation::MOST as f64 {
                return None;
            }
            passes.push((marker, loop_end, 0.));
            if per_lap > 0 {
                for lap in 0..laps as usize {
                    passes.push((loop_start, loop_end, first + lap as f64 * period));
                }
            }
        }
        None => {
            let marker = number(clip, "startMarker").unwrap_or(loop_start);
            passes.push((marker, marker + span, 0.));
        }
    }
    let mut heard = vec![];
    let mut unheard = 0;
    for note in notes {
        let before = heard.len();
        for &(from, to, at) in &passes {
            if note.start < from - 1e-9 || note.start >= to - 1e-9 {
                continue;
            }
            let time = at + note.start - from;
            if time >= span - 1e-9 {
                continue;
            }
            let end = (at + to - from).min(span);
            heard.push(Note { start: time, duration: note.duration.min(end - time), ..note.clone() });
        }
        unheard += usize::from(heard.len() == before);
    }
    Some(Played { notes: heard, unheard, span, looping, assumed })
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
