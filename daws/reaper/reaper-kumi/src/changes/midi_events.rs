//! A MIDI take's events beyond notes (M4): controllers (14-bit ones as the MSB/LSB pairs REAPER stores), pitch bend,
//! program change (with a bank), channel and poly pressure, text and sysex. All of them go through the take's one MIDI
//! buffer (`MIDI_GetAllEvts`/`SetAllEvts`), so a big edit is one call each way and every other event goes back byte for
//! byte, in its place.
//!
//! In the buffer, a controller with a bezier shape is followed by a REAPER notation event holding its tension (`CCBZ`):
//! the two are one event here. The take's last event, an all-notes-off at the source's end, marks where its source ends:
//! it is never listed, moved or deleted. An event has no identity of its own, so a change fences the take's whole list of
//! these events (count and hash), and an undo finds the events it wrote by their values, as notes are found.

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Map, Value};

use super::items::{changed, gone};
use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::midi::Event;
use crate::read::{bad, failed, take_start_qn};
use crate::reaper::Take;
use crate::{refs, rows};

/// The most events one change writes or deletes.
const MOST: usize = 10_000;
/// The rows a preview or an answer shows.
const SHOWN: usize = 8;
/// REAPER's end-of-source marker: all notes off on channel 1.
const END_MARKER: [u8; 3] = [0xB0, 0x7B, 0x00];
/// A CC shape's flag bits in the buffer (`&0xF0`), by REAPER's names.
const SHAPES: [(&str, u8); 6] =
    [("square", 0), ("linear", 0x10), ("slow", 0x20), ("fast-start", 0x30), ("fast-end", 0x40), ("bezier", 0x50)];
/// MIDI text event types 1 to 9 by name (10 to 14 are numbered).
const TEXT_TYPES: [&str; 9] = ["text", "copyright", "track-name", "instrument", "lyric", "marker", "cue", "program-name", "device-name"];

/// One event as the changes see it: its place in ticks, its flag (selected, muted, a CC's shape) and message, and a bezier
/// CC's tension event.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ev {
    pub pos: i64,
    pub flag: u8,
    pub msg: Vec<u8>,
    pub bezier: Option<Vec<u8>>,
}

fn is_note(msg: &[u8]) -> bool {
    matches!(msg.first(), Some(status) if (0x80..0xA0).contains(status))
}

fn is_note_on(event: &Event) -> bool {
    matches!(event.msg.as_slice(), [status, _, velocity] if status & 0xF0 == 0x90 && *velocity > 0)
}

/// A bank select (controller 0 or 32) or a program change, which REAPER orders first among the events at a tick (lab, 7.82).
fn leads(msg: &[u8]) -> bool {
    match msg {
        [status, ..] if status & 0xF0 == 0xC0 => true,
        [status, 0 | 32, ..] if status & 0xF0 == 0xB0 => true,
        _ => false,
    }
}

fn is_bezier_data(msg: &[u8]) -> bool {
    msg.len() >= 7 && msg[0] == 0xFF && msg[1] == 0x0F && &msg[2..7] == b"CCBZ "
}

/// Where the take's source ends: the end marker's tick, when the last event is one.
pub(super) fn source_end(events: &[Event]) -> Option<i64> {
    events.last().filter(|e| e.msg == END_MARKER).map(|e| e.pos)
}

/// The take's events that aren't notes, in buffer order, each with the buffer positions it takes (one, or two for a bezier
/// controller and its tension).
pub(crate) fn events_of(buffer: &[Event]) -> Vec<(Ev, Vec<usize>)> {
    let end = buffer.len().checked_sub(1).filter(|_| source_end(buffer).is_some());
    let mut out = Vec::new();
    let mut at = 0;
    while at < buffer.len() {
        let event = &buffer[at];
        if Some(at) == end || is_note(&event.msg) {
            at += 1;
            continue;
        }
        let next = buffer.get(at + 1).filter(|n| n.pos == event.pos && is_bezier_data(&n.msg) && event.flag & 0x50 == 0x50);
        match next {
            Some(data) => {
                out.push((
                    Ev { pos: event.pos, flag: event.flag, msg: event.msg.clone(), bezier: Some(data.msg.clone()) },
                    vec![at, at + 1],
                ));
                at += 2;
            }
            None => {
                out.push((Ev { pos: event.pos, flag: event.flag, msg: event.msg.clone(), bezier: None }, vec![at]));
                at += 1;
            }
        }
    }
    out
}

fn fence_of(events: &[Ev]) -> Value {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for event in events {
        eat(&event.pos.to_le_bytes());
        eat(&[event.flag & !1]);
        eat(&event.msg);
        eat(event.bezier.as_deref().unwrap_or_default());
        eat(&[0xFE]);
    }
    json!({"count": events.len(), "hash": format!("{hash:016x}")})
}

/// What tells an event apart: everything but whether it is selected.
type Key<'a> = (i64, u8, &'a [u8], Option<&'a [u8]>);

fn key(event: &Ev) -> Key<'_> {
    (event.pos, event.flag & !1, &event.msg, event.bezier.as_deref())
}

fn ev_json(event: &Ev) -> Value {
    json!([event.pos, event.flag, hex(&event.msg), event.bezier.as_deref().map(hex)])
}

fn ev_from(value: &Value) -> Option<Ev> {
    Some(Ev {
        pos: value.get(0)?.as_i64()?,
        flag: value.get(1)?.as_u64()? as u8,
        msg: unhex(value.get(2)?.as_str()?)?,
        bezier: match value.get(3)? {
            Value::Null => None,
            other => Some(unhex(other.as_str()?)?),
        },
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    text.split_whitespace().map(|word| u8::from_str_radix(word, 16).ok()).collect()
}

/// The events of `a` that `b` has no event for (each counted once), by counting `b`'s: a take of tens of thousands is
/// compared in one pass each way, not one search of the other's events per event.
fn difference(a: &[Ev], b: &[Ev]) -> Vec<Ev> {
    let mut left: HashMap<Key, usize> = HashMap::new();
    for event in b {
        *left.entry(key(event)).or_insert(0) += 1;
    }
    a.iter()
        .filter(|event| match left.get_mut(&key(event)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// The buffer with the events in `remove` taken out and those in `add` put in, everything else as it was. An event goes in
/// after what is at its tick already, but before any note-on there (a program change before its note); events added at
/// one tick keep the order given; nothing goes after the source's end marker.
pub(crate) fn edit(buffer: &[Event], remove: &[Ev], add: &[Ev]) -> Result<Vec<Event>, String> {
    let listed = events_of(buffer);
    let mut gone = vec![false; buffer.len()];
    // (each event's places in the buffer by what it is, in buffer order: a removal takes the first one left)
    let mut places: HashMap<Key, VecDeque<usize>> = HashMap::new();
    for (n, (event, _)) in listed.iter().enumerate() {
        places.entry(key(event)).or_default().push_back(n);
    }
    for event in remove {
        let at = places
            .get_mut(&key(event))
            .and_then(VecDeque::pop_front)
            .ok_or_else(|| format!("an event at tick {} isn't in the take as given", event.pos))?;
        for place in &listed[at].1 {
            gone[*place] = true;
        }
    }
    let mut pending: Vec<&Ev> = add.iter().collect();
    // (a bank select or program change first at its tick: REAPER puts them first when it reads a take back)
    pending.sort_by_key(|event| (event.pos, !leads(&event.msg)));
    let mut pending = pending.into_iter().peekable();
    let mut out = Vec::with_capacity(buffer.len() + add.len() * 2);
    let end = source_end(buffer).map(|_| buffer.len() - 1);
    for (at, event) in buffer.iter().enumerate() {
        // New events at or before this tick go first when this is a note-on (or the end marker), a bank select or program
        // change before the other events at its tick, else those strictly before; never between a controller and its shape.
        let before_this = |next: &Ev| {
            next.pos < event.pos
                || (next.pos == event.pos
                    && !is_bezier_data(&event.msg)
                    && (is_note_on(event) || Some(at) == end || (leads(&next.msg) && !leads(&event.msg))))
        };
        while let Some(next) = pending.next_if(|next| before_this(next)) {
            push(&mut out, next);
        }
        if !gone[at] {
            out.push(event.clone());
        }
    }
    for next in pending {
        push(&mut out, next);
    }
    Ok(out)
}

fn push(out: &mut Vec<Event>, event: &Ev) {
    out.push(Event { pos: event.pos, flag: event.flag, msg: event.msg.clone() });
    if let Some(data) = &event.bezier {
        out.push(Event { pos: event.pos, flag: 0, msg: data.clone() });
    }
}

// ----- events as the model reads and writes them -----

fn channel_of(msg: &[u8]) -> i64 {
    (msg[0] & 0x0F) as i64 + 1
}

fn shape_of(flag: u8) -> &'static str {
    SHAPES.iter().find(|(_, bits)| *bits == flag & 0xF0).map_or("square", |(name, _)| name)
}

fn tension_of(bezier: Option<&[u8]>) -> Option<f64> {
    let data = bezier?;
    let bytes: [u8; 4] = data.get(8..12)?.try_into().ok()?;
    Some((f32::from_le_bytes(bytes) as f64 * 10_000.0).round() / 10_000.0)
}

/// An event as a read row says it, its place as quarter notes from the item's start.
pub(crate) fn row(event: &Ev, index: usize, beats: f64) -> Value {
    let msg = &event.msg;
    let mut row = Map::new();
    row.insert("index".into(), index.into());
    let kind = match msg.first().copied().unwrap_or(0) {
        status if status & 0xF0 == 0xB0 => {
            row.insert("channel".into(), channel_of(msg).into());
            row.insert("controller".into(), msg.get(1).copied().unwrap_or(0).into());
            row.insert("value".into(), msg.get(2).copied().unwrap_or(0).into());
            "cc"
        }
        status if status & 0xF0 == 0xE0 => {
            row.insert("channel".into(), channel_of(msg).into());
            let value = (msg.get(1).copied().unwrap_or(0) as i64) | ((msg.get(2).copied().unwrap_or(0x40) as i64) << 7);
            row.insert("value".into(), (value - 8192).into());
            "pitchbend"
        }
        status if status & 0xF0 == 0xC0 => {
            row.insert("channel".into(), channel_of(msg).into());
            row.insert("program".into(), msg.get(1).copied().unwrap_or(0).into());
            "program"
        }
        status if status & 0xF0 == 0xD0 => {
            row.insert("channel".into(), channel_of(msg).into());
            row.insert("value".into(), msg.get(1).copied().unwrap_or(0).into());
            "pressure"
        }
        status if status & 0xF0 == 0xA0 => {
            row.insert("channel".into(), channel_of(msg).into());
            row.insert("pitch".into(), msg.get(1).copied().unwrap_or(0).into());
            row.insert("value".into(), msg.get(2).copied().unwrap_or(0).into());
            "poly-pressure"
        }
        0xF0 => {
            let inner = msg.get(1..msg.len().saturating_sub(usize::from(msg.last() == Some(&0xF7)))).unwrap_or_default();
            row.insert("bytes".into(), rows::clip(&hex(inner)).into());
            "sysex"
        }
        0xFF => {
            let kind = msg.get(1).copied().unwrap_or(1);
            let text = String::from_utf8_lossy(msg.get(2..).unwrap_or_default()).into_owned();
            row.insert("text".into(), rows::clip(&text).into());
            if kind == 0x0F {
                "notation"
            } else {
                let name = TEXT_TYPES.get((kind as usize).wrapping_sub(1)).map_or_else(|| kind.to_string(), |name| name.to_string());
                row.insert("textType".into(), name.into());
                "text"
            }
        }
        _ => {
            row.insert("bytes".into(), hex(msg).into());
            "other"
        }
    };
    row.insert("type".into(), kind.into());
    row.insert("start".into(), ((beats * 10_000.0).round() / 10_000.0).into());
    if matches!(kind, "cc" | "pitchbend" | "pressure" | "program") {
        row.insert("shape".into(), shape_of(event.flag).into());
        if let Some(tension) = tension_of(event.bezier.as_deref()) {
            row.insert("tension".into(), tension.into());
        }
    }
    if event.flag & 2 != 0 {
        row.insert("muted".into(), true.into());
    }
    if event.flag & 1 != 0 {
        row.insert("selected".into(), true.into());
    }
    Value::Object(row)
}

fn whole(spec: &Value, key: &str, low: i64, high: i64) -> Done<Option<i64>> {
    match spec.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|n| (low..=high).contains(n))
            .map(Some)
            .ok_or_else(|| bad(format!("{key} must be a whole number from {low} to {high}"))),
    }
}

fn need(spec: &Value, key: &str, low: i64, high: i64) -> Done<i64> {
    whole(spec, key, low, high)?.ok_or_else(|| bad(format!("{key}: a whole number from {low} to {high}")))
}

fn shape_flag(spec: &Value) -> Done<(u8, Option<f64>)> {
    let shape = match spec.get("shape").and_then(Value::as_str) {
        None => None,
        Some(name) => Some(
            SHAPES
                .iter()
                .find(|(word, _)| *word == name)
                .map(|(_, bits)| *bits)
                .ok_or_else(|| bad("shape must be square, linear, slow, fast-start, fast-end or bezier"))?,
        ),
    };
    let tension = match spec.get("tension").filter(|v| !v.is_null()) {
        None => None,
        Some(value) => Some(value.as_f64().filter(|n| (-1.0..=1.0).contains(n)).ok_or_else(|| bad("tension must be from -1 to 1"))?),
    };
    if tension.is_some() && shape != Some(0x50) {
        return Err(bad("tension is a bezier shape's: pass shape bezier with it"));
    }
    Ok((shape.unwrap_or(0), tension))
}

fn bezier_data(tension: f64) -> Vec<u8> {
    let mut data = vec![0xFF, 0x0F, b'C', b'C', b'B', b'Z', b' ', 0x00];
    data.extend_from_slice(&(tension as f32).to_le_bytes());
    data
}

fn muted_flag(spec: &Value) -> Done<u8> {
    match spec.get("muted") {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Bool(muted)) => Ok(if *muted { 2 } else { 0 }),
        Some(_) => Err(bad("muted must be true or false")),
    }
}

/// The events one entry of a request stands for (a 14-bit controller and a program change with a bank are two or three).
fn new_events(spec: &Value, pos: i64) -> Done<Vec<Ev>> {
    let kind = spec
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("every event needs a type: cc, pitchbend, program, pressure, poly-pressure, text or sysex"))?;
    // (channel 1 when not given, as Kumi's schema has it optional)
    let channel = || -> Done<u8> { Ok(whole(spec, "channel", 1, 16)?.unwrap_or(1) as u8 - 1) };
    let muted = muted_flag(spec)?;
    let shaped = |msg: Vec<u8>| -> Done<Ev> {
        let (shape, tension) = shape_flag(spec)?;
        let bezier = (shape == 0x50).then(|| bezier_data(tension.unwrap_or(0.0)));
        Ok(Ev { pos, flag: shape | muted, msg, bezier })
    };
    let plain = |msg: Vec<u8>| Ev { pos, flag: muted, msg, bezier: None };
    Ok(match kind {
        "cc" => {
            let ch = channel()?;
            if spec.get("fourteenBit") == Some(&Value::Bool(true)) {
                // REAPER's 14-bit controller: the MSB at the controller, the LSB 32 above it, at one tick.
                let controller = need(spec, "controller", 0, 31)? as u8;
                let value = need(spec, "value", 0, 16383)?;
                vec![
                    shaped(vec![0xB0 | ch, controller, (value >> 7) as u8])?,
                    shaped(vec![0xB0 | ch, controller + 32, (value & 0x7F) as u8])?,
                ]
            } else {
                vec![shaped(vec![0xB0 | ch, need(spec, "controller", 0, 127)? as u8, need(spec, "value", 0, 127)? as u8])?]
            }
        }
        "pitchbend" => {
            let value = need(spec, "value", -8192, 8191)? + 8192;
            vec![shaped(vec![0xE0 | channel()?, (value & 0x7F) as u8, (value >> 7) as u8])?]
        }
        "program" => {
            let ch = channel()?;
            let mut out = Vec::new();
            if let Some(bank) = whole(spec, "bank", 0, 16383)? {
                out.push(plain(vec![0xB0 | ch, 0, (bank >> 7) as u8]));
                out.push(plain(vec![0xB0 | ch, 32, (bank & 0x7F) as u8]));
            }
            out.push(plain(vec![0xC0 | ch, need(spec, "program", 0, 127)? as u8]));
            out
        }
        "pressure" => vec![shaped(vec![0xD0 | channel()?, need(spec, "value", 0, 127)? as u8])?],
        "poly-pressure" => vec![plain(vec![0xA0 | channel()?, need(spec, "pitch", 0, 127)? as u8, need(spec, "value", 0, 127)? as u8])],
        "text" => {
            let text = spec.get("text").and_then(Value::as_str).ok_or_else(|| bad("a text event needs its text"))?;
            if text.len() > 4096 {
                return Err(bad("a text event holds at most 4096 bytes"));
            }
            let kind = match spec.get("textType") {
                None | Some(Value::Null) => 1,
                Some(Value::String(name)) => TEXT_TYPES
                    .iter()
                    .position(|t| t == name)
                    .map(|n| n as u8 + 1)
                    .ok_or_else(|| bad(format!("textType must be one of {} (or 1 to 14)", TEXT_TYPES.join(", "))))?,
                Some(value) => value
                    .as_u64()
                    .filter(|n| (1..=14).contains(n))
                    .map(|n| n as u8)
                    .ok_or_else(|| bad("textType must be a name or 1 to 14"))?,
            };
            let mut msg = vec![0xFF, kind];
            msg.extend_from_slice(text.as_bytes());
            vec![plain(msg)]
        }
        "sysex" => {
            let text =
                spec.get("bytes").and_then(Value::as_str).ok_or_else(|| bad("a sysex event needs its bytes, in hex (\"7E 7F 09 01\")"))?;
            let mut bytes = unhex(text).ok_or_else(|| bad("bytes must be hex bytes apart by spaces"))?;
            if bytes.first() == Some(&0xF0) {
                bytes.remove(0);
            }
            if bytes.last() == Some(&0xF7) {
                bytes.pop();
            }
            if bytes.is_empty() || bytes.len() > 65_536 || bytes.iter().any(|b| *b >= 0x80) {
                return Err(bad("a sysex message is 1 to 65536 data bytes (each below 80), its F0 and F7 aside"));
            }
            let mut msg = vec![0xF0];
            msg.extend(bytes);
            msg.push(0xF7);
            vec![plain(msg)]
        }
        other => {
            return Err(bad(format!(
                "REAPER's MIDI has no event type {other:?}: cc, pitchbend, program, pressure, poly-pressure, text or sysex"
            )))
        }
    })
}

/// An event changed as a request asks: only what it names.
fn changed_event(was: &Ev, spec: &Value, pos: i64) -> Done<Ev> {
    let mut event = Ev { pos, ..was.clone() };
    let msg = &mut event.msg;
    let status = msg.first().copied().unwrap_or(0);
    if let Some(channel) = whole(spec, "channel", 1, 16)? {
        if !(0x80..0xF0).contains(&status) {
            return Err(bad("a channel is a channel message's: this event has none"));
        }
        msg[0] = (status & 0xF0) | (channel as u8 - 1);
    }
    match status & 0xF0 {
        0xB0 => {
            if let Some(controller) = whole(spec, "controller", 0, 127)? {
                msg[1] = controller as u8;
            }
            if let Some(value) = whole(spec, "value", 0, 127)? {
                msg[2] = value as u8;
            }
        }
        0xE0 => {
            if let Some(value) = whole(spec, "value", -8192, 8191)? {
                let value = value + 8192;
                msg[1] = (value & 0x7F) as u8;
                msg[2] = (value >> 7) as u8;
            }
        }
        0xC0 => {
            if let Some(program) = whole(spec, "program", 0, 127)? {
                msg[1] = program as u8;
            }
        }
        0xD0 => {
            if let Some(value) = whole(spec, "value", 0, 127)? {
                msg[1] = value as u8;
            }
        }
        0xA0 => {
            if let Some(pitch) = whole(spec, "pitch", 0, 127)? {
                msg[1] = pitch as u8;
            }
            if let Some(value) = whole(spec, "value", 0, 127)? {
                msg[2] = value as u8;
            }
        }
        _ if status == 0xFF => {
            if let Some(text) = spec.get("text").and_then(Value::as_str) {
                msg.truncate(2);
                msg.extend_from_slice(text.as_bytes());
            }
        }
        _ if status == 0xF0 => {
            if let Some(text) = spec.get("bytes").and_then(Value::as_str) {
                let new = new_events(&json!({"type": "sysex", "bytes": text}), pos)?;
                *msg = new[0].msg.clone();
            }
        }
        _ => {}
    }
    if spec.get("shape").is_some() || spec.get("tension").is_some() {
        if !matches!(status & 0xF0, 0xB0 | 0xE0 | 0xD0 | 0xC0) || status >= 0xF0 {
            return Err(bad("a shape is a controller lane event's (cc, pitchbend, pressure, program)"));
        }
        let (shape, tension) = shape_flag(spec)?;
        event.flag = (event.flag & 0x0F) | shape;
        event.bezier = (shape == 0x50).then(|| bezier_data(tension.or_else(|| tension_of(was.bezier.as_deref())).unwrap_or(0.0)));
    }
    if let Some(muted) = spec.get("muted").and_then(Value::as_bool) {
        event.flag = (event.flag & !2) | if muted { 2 } else { 0 };
    }
    Ok(event)
}

// ----- what a request asks -----

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    Add,
    Change,
    Delete,
}

struct Plan {
    take: Take,
    guid: String,
    item_qn: f64,
    buffer: Vec<Event>,
    before: Vec<Ev>,
    remove: Vec<usize>,
    add: Vec<Ev>,
}

fn take_of(ctx: &Ctx, value: &Value) -> Done<Take> {
    let text = value.as_str().ok_or_else(|| bad("ref must be a MIDI take or item ref"))?;
    ctx.reads.midi_take(ctx.reaper, ctx.proj, Some(text))
}

fn list<'a>(input: &'a Value, key: &str) -> Done<&'a Vec<Value>> {
    let list = input.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("{key} must be a list")))?;
    if list.is_empty() || list.len() > MOST {
        return Err(bad(format!("{key} must hold from 1 to {MOST} entries")));
    }
    Ok(list)
}

fn tick_at(ctx: &Ctx, take: Take, item_qn: f64, start: f64) -> Done<i64> {
    ctx.reaper
        .quarter_notes_to_ppq(take, item_qn + start)
        .map(|ppq| ppq.round() as i64)
        .ok_or_else(|| failed("this REAPER can't turn beats into ticks"))
}

fn start_of(spec: &Value) -> Done<Option<f64>> {
    match spec.get("start") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .map(Some)
            .ok_or_else(|| bad("start must be quarter notes from the start of the item, from 0")),
    }
}

/// The events a delete's filter names: of a type (and a channel, a controller), from `from` up to `to` quarter notes.
fn filtered(ctx: &Ctx, plan_take: Take, item_qn: f64, before: &[Ev], filter: &Value) -> Done<Vec<usize>> {
    if !filter.is_object() {
        return Err(bad("filter must be {type?, channel?, controller?, from?, to?}"));
    }
    let kind = filter.get("type").and_then(Value::as_str);
    let channel = whole(filter, "channel", 1, 16)?;
    let controller = whole(filter, "controller", 0, 127)?;
    let from = match filter.get("from").and_then(Value::as_f64) {
        Some(qn) => Some(tick_at(ctx, plan_take, item_qn, qn)?),
        None => None,
    };
    let to = match filter.get("to").and_then(Value::as_f64) {
        Some(qn) => Some(tick_at(ctx, plan_take, item_qn, qn)?),
        None => None,
    };
    Ok(before
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            let row = row(event, 0, 0.0);
            kind.is_none_or(|k| row["type"] == k)
                && channel.is_none_or(|c| row["channel"].as_i64() == Some(c))
                && controller.is_none_or(|c| row["controller"].as_i64() == Some(c))
                && from.is_none_or(|f| event.pos >= f)
                && to.is_none_or(|t| event.pos < t)
        })
        .map(|(at, _)| at)
        .collect())
}

fn plan(ctx: &Ctx, input: &Value, which: Which) -> Done<Plan> {
    let take = take_of(ctx, &input["ref"])?;
    let reaper = ctx.reaper;
    let buffer = reaper.midi_events(take).ok_or_else(|| failed("REAPER wouldn't give the take's MIDI"))?;
    let before: Vec<Ev> = events_of(&buffer).into_iter().map(|(event, _)| event).collect();
    let item_qn = take_start_qn(reaper, ctx.proj, take);
    let end = source_end(&buffer);
    let (mut remove, mut add) = (Vec::new(), Vec::new());
    match which {
        Which::Add => {
            for spec in list(input, "events")? {
                let start = start_of(spec)?.ok_or_else(|| bad("every event needs a start (quarter notes from the start of the item)"))?;
                let pos = tick_at(ctx, take, item_qn, start)?;
                add.extend(new_events(spec, pos)?);
            }
        }
        Which::Change => {
            for spec in list(input, "changes")? {
                let at = spec.get("index").and_then(Value::as_u64).map(|n| n as usize).filter(|n| *n < before.len()).ok_or_else(|| {
                    bad(format!(
                        "every change needs the index of an event in that take (it has {}, from 0, as read): read it again",
                        before.len()
                    ))
                })?;
                if remove.contains(&at) {
                    return Err(bad("an event is named twice"));
                }
                let pos = match start_of(spec)? {
                    Some(start) => tick_at(ctx, take, item_qn, start)?,
                    None => before[at].pos,
                };
                add.push(changed_event(&before[at], spec, pos)?);
                remove.push(at);
            }
        }
        Which::Delete => {
            if let Some(indexes) = input.get("indexes").filter(|v| !v.is_null()) {
                for value in indexes
                    .as_array()
                    .filter(|l| !l.is_empty() && l.len() <= MOST)
                    .ok_or_else(|| bad(format!("indexes must hold from 1 to {MOST} entries")))?
                {
                    let at = value.as_u64().map(|n| n as usize).filter(|n| *n < before.len()).ok_or_else(|| {
                        bad(format!(
                            "indexes must be positions of events in that take (it has {}, from 0, as read): read it again",
                            before.len()
                        ))
                    })?;
                    if remove.contains(&at) {
                        return Err(bad("an event is named twice"));
                    }
                    remove.push(at);
                }
            }
            if let Some(filter) = input.get("filter").filter(|v| !v.is_null()) {
                for at in filtered(ctx, take, item_qn, &before, filter)? {
                    if !remove.contains(&at) {
                        remove.push(at);
                    }
                }
            }
            if input.get("indexes").is_none_or(Value::is_null) && input.get("filter").is_none_or(Value::is_null) {
                return Err(bad("pass indexes, a filter, or both"));
            }
            if remove.is_empty() {
                return Err(bad("no event of that take matches the filter"));
            }
            if remove.len() > MOST {
                return Err(bad(format!("that's {} events: delete at most {MOST} at a time", remove.len())));
            }
        }
    }
    // Inside the take's source: nothing goes at or past the marker that ends it.
    if let Some(end) = end {
        if add.iter().any(|event| event.pos >= end) {
            let end_qn = reaper.ppq_to_quarter_notes(take, end as f64).map(|qn| qn - item_qn).unwrap_or(0.0);
            return Err(bad(format!(
                "an event must be before the end of the take's source, {end_qn:.3} quarter notes from the item's start"
            )));
        }
    }
    if add.iter().any(|event| event.pos < 0) {
        return Err(bad("an event must be at or after the start of the take's source"));
    }
    remove.sort_unstable();
    Ok(Plan { take, guid: reaper.take_guid(take), item_qn, buffer, before, remove, add })
}

fn label(ctx: &Ctx, take: Take) -> String {
    super::takes::label(ctx, take)
}

fn title(which: Which, count: usize, label: &str) -> String {
    let verb = match which {
        Which::Add => "Wrote",
        Which::Change => "Changed",
        Which::Delete => "Deleted",
    };
    format!("{verb} {count} MIDI event{} in {label}", if count == 1 { "" } else { "s" })
}

fn rows_of(ctx: &Ctx, take: Take, item_qn: f64, events: impl Iterator<Item = (usize, Ev)>) -> Vec<Value> {
    events
        .take(SHOWN)
        .map(|(index, event)| {
            let beats = ctx.reaper.ppq_to_quarter_notes(take, event.pos as f64).map_or(0.0, |qn| qn - item_qn);
            row(&event, index, beats)
        })
        .collect()
}

pub fn preview(ctx: &Ctx, input: &Value, which: Which) -> Done<Preview> {
    let plan = plan(ctx, input, which)?;
    let count = plan.add.len().max(plan.remove.len());
    let fence = fence_of(&plan.before);
    let gone = plan.remove.iter().map(|&at| (at, plan.before[at].clone()));
    let mut display = json!({"title": title(which, count, &label(ctx, plan.take)), "count": count});
    if which != Which::Add {
        display["before"] = rows_of(ctx, plan.take, plan.item_qn, gone).into();
    }
    if which != Which::Delete {
        display["after"] = rows_of(ctx, plan.take, plan.item_qn, plan.add.iter().cloned().enumerate()).into();
    }
    Ok(Preview {
        target: json!({"ref": refs::take_ref(&plan.guid), "kind": "take"}),
        prior: json!({"events": plan.before.len(), "fence": fence.clone()}),
        fence,
        display,
    })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value, which: Which) -> Done<Applied> {
    let plan = plan(ctx, input, which).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    if fence_of(&plan.before) != *fence {
        return Err(changed(format!("the MIDI events of {} changed since the preview", label(ctx, plan.take))));
    }
    let reaper = ctx.reaper;
    let removed: Vec<Ev> = plan.remove.iter().map(|&at| plan.before[at].clone()).collect();
    let edited = edit(&plan.buffer, &removed, &plan.add).map_err(|why| changed(format!("the take changed: {why}")))?;
    if !reaper.set_midi_events(plan.take, &edited) {
        return Err(failed("REAPER wouldn't take the new MIDI"));
    }
    let after_buffer = reaper
        .midi_events(plan.take)
        .ok_or_else(|| ("uncertain".to_string(), "REAPER wouldn't give the take's MIDI after the change: read it".to_string()))?;
    let after: Vec<Ev> = events_of(&after_buffer).into_iter().map(|(event, _)| event).collect();
    let (gone_now, added_now) = (difference(&plan.before, &after), difference(&after, &plan.before));
    let as_asked = difference(&removed, &gone_now).is_empty()
        && difference(&plan.add, &added_now).is_empty()
        && source_end(&after_buffer) == source_end(&plan.buffer);
    if !as_asked {
        let back =
            reaper.set_midi_events(plan.take, &plan.buffer) && reaper.midi_events(plan.take).as_deref() == Some(plan.buffer.as_slice());
        return Err(if back {
            failed("the take doesn't hold the events it should after the change; what was written is taken back")
        } else {
            (
                "uncertain".into(),
                "the take doesn't hold the events it should after the change, and Kumi could not take back what it wrote: read the take"
                    .into(),
            )
        });
    }
    let count = plan.add.len().max(plan.remove.len());
    Ok(Applied {
        applied: json!({"added": added_now.iter().map(ev_json).collect::<Vec<_>>(), "removed": gone_now.iter().map(ev_json).collect::<Vec<_>>(), "events": after.len()}),
        display: json!({"title": title(which, count, &label(ctx, plan.take)), "events": after.len(), "written": rows_of(ctx, plan.take, plan.item_qn, added_now.iter().cloned().enumerate())}),
        unwritten: gone_now.is_empty() && added_now.is_empty(),
        ..Default::default()
    })
}

fn evs_of(value: &Value, key: &str) -> Done<Vec<Ev>> {
    value[key]
        .as_array()
        .ok_or_else(|| bad(format!("applied.{key}: the events the change wrote")))?
        .iter()
        .map(|e| ev_from(e).ok_or_else(|| bad("applied: an event is not [tick, flag, bytes, bezier]")))
        .collect()
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let take =
        take_of(ctx, &restore.target["ref"]).map_err(|(code, message)| if code == "failed" { gone(message) } else { (code, message) })?;
    let reaper = ctx.reaper;
    let (added, removed) = (evs_of(restore.applied, "added")?, evs_of(restore.applied, "removed")?);
    let buffer = reaper.midi_events(take).ok_or_else(|| failed("REAPER wouldn't give the take's MIDI"))?;
    let current: Vec<Ev> = events_of(&buffer).into_iter().map(|(event, _)| event).collect();
    if fence_of(&current) == restore.prior["fence"] {
        return Ok(Restored {
            restored: json!({"events": current.len()}),
            display: json!({"title": format!("The MIDI events of {} are as they were", label(ctx, take))}),
            unwritten: true,
        });
    }
    let missing = difference(&added, &current);
    if !missing.is_empty() && !restore.discard {
        return Err(changed(format!("the MIDI events Kumi wrote in {} have been changed in REAPER since", label(ctx, take))));
    }
    let present = difference(&added, &missing);
    let kept = difference(&current, &present);
    let to_bring_back = difference(&removed, &kept);
    let edited = edit(&buffer, &present, &to_bring_back).map_err(|why| changed(format!("the take changed: {why}")))?;
    if !reaper.set_midi_events(take, &edited) {
        return Err(failed("REAPER wouldn't take the MIDI back"));
    }
    let after: Vec<Ev> = reaper.midi_events(take).map(|b| events_of(&b).into_iter().map(|(event, _)| event).collect()).unwrap_or_default();
    if !difference(&present, &difference(&current, &after)).is_empty() || !difference(&to_bring_back, &after).is_empty() {
        let back = reaper.set_midi_events(take, &buffer);
        return Err(if back {
            failed("the take doesn't hold the events it should after the undo; it is as it was")
        } else {
            (
                "uncertain".into(),
                "the take doesn't hold the events it should after the undo, and Kumi could not put it as it was: read the take".into(),
            )
        });
    }
    Ok(Restored {
        restored: json!({"events": after.len()}),
        display: json!({"title": format!("MIDI events in {} put back ({} removed, {} written again)", label(ctx, take), present.len(), to_bring_back.len())}),
        unwritten: present.is_empty() && to_bring_back.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(pos: i64, msg: &[u8]) -> Event {
        Event { pos, flag: 0, msg: msg.to_vec() }
    }

    fn buffer() -> Vec<Event> {
        vec![
            ev(0, &[0xB0, 1, 64]),
            ev(0, &[0x90, 60, 100]),
            Event { pos: 480, flag: 0x50, msg: vec![0xB0, 74, 20] },
            Event { pos: 480, flag: 0, msg: bezier_data(0.4) },
            ev(480, &[0x80, 60, 0]),
            ev(960, &[0xF0, 0x7E, 0x7F, 0xF7]),
            ev(7680, &END_MARKER),
        ]
    }

    #[test]
    fn a_bezier_controller_and_its_tension_are_one_event_and_the_end_marker_none() {
        let events = events_of(&buffer());
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].1, vec![2, 3]);
        assert_eq!(tension_of(events[1].0.bezier.as_deref()), Some(0.4));
        let row = row(&events[1].0, 1, 0.5);
        assert_eq!((row["type"].as_str(), row["shape"].as_str(), row["tension"].as_f64()), (Some("cc"), Some("bezier"), Some(0.4)));
        assert_eq!(super::row(&events[2].0, 2, 1.0)["bytes"], "7E 7F");
    }

    #[test]
    fn new_events_go_before_a_note_on_at_their_tick_and_never_after_the_end_marker() {
        let program = Ev { pos: 0, flag: 0, msg: vec![0xC0, 5], bezier: None };
        let late = Ev { pos: 960, flag: 0x50, msg: vec![0xB0, 7, 1], bezier: Some(bezier_data(-0.5)) };
        let out = edit(&buffer(), &[], &[program, late]).unwrap();
        let order: Vec<Vec<u8>> = out.iter().map(|e| e.msg.clone()).collect();
        // A program change before the CC already at 0 (REAPER orders it first when it reads the take back), and so before
        // the note-on there.
        assert_eq!(order[0], vec![0xC0, 5]);
        assert_eq!(order[1], vec![0xB0, 1, 64]);
        assert_eq!(order[2], vec![0x90, 60, 100]);
        // A bezier controller's tension right after it; the end marker last.
        let at = order.iter().position(|m| *m == vec![0xB0, 7, 1]).unwrap();
        assert!(is_bezier_data(&order[at + 1]));
        assert_eq!(out.last().unwrap().msg, END_MARKER.to_vec());
        // Taking events out takes a bezier controller's tension with it, and nothing else.
        let events = events_of(&buffer());
        let less = edit(&buffer(), &[events[1].0.clone()], &[]).unwrap();
        assert_eq!(less.len(), buffer().len() - 2);
        assert!(edit(&buffer(), &[Ev { pos: 1, flag: 0, msg: vec![0xB0, 1, 64], bezier: None }], &[]).is_err());
    }

    #[test]
    fn a_bank_select_and_program_go_first_at_their_tick_but_never_between_a_controller_and_its_shape() {
        let at_zero = vec![
            Event { pos: 0, flag: 0x50, msg: vec![0xB0, 0, 1] },
            Event { pos: 0, flag: 0, msg: bezier_data(0.2) },
            ev(0, &[0xB0, 7, 100]),
            ev(7680, &END_MARKER),
        ];
        let new = |msg: &[u8]| Ev { pos: 0, flag: 0, msg: msg.to_vec(), bezier: None };
        // Given with a controller first: the bank select and program still lead, in the order given.
        let out = edit(&at_zero, &[], &[new(&[0xB0, 74, 9]), new(&[0xB0, 32, 2]), new(&[0xC0, 7])]).unwrap();
        let order: Vec<Vec<u8>> = out.iter().map(|e| e.msg.clone()).collect();
        assert_eq!(order[0], vec![0xB0, 0, 1]);
        assert!(is_bezier_data(&order[1]), "the shape stays with its controller");
        assert_eq!(&order[2..5], &[vec![0xB0, 32, 2], vec![0xC0, 7], vec![0xB0, 7, 100]]);
        assert_eq!(order[5], vec![0xB0, 74, 9], "a controller goes after those already at its tick");
        assert_eq!(out.last().unwrap().msg, END_MARKER.to_vec());
    }

    #[test]
    fn deleting_half_of_a_big_takes_controllers_is_read_in_linear_time() {
        // 60,000 controller events, every second one deleted: a search of the take per event held REAPER's main thread
        // for seconds.
        let mut buffer: Vec<Event> = (0..60_000).map(|i| ev(i * 10, &[0xB0, 1, (i % 128) as u8])).collect();
        buffer.push(ev(600_000, &END_MARKER));
        let events: Vec<Ev> = events_of(&buffer).into_iter().map(|(event, _)| event).collect();
        let remove: Vec<Ev> = events.iter().step_by(2).cloned().collect();
        let started = std::time::Instant::now();
        let edited = edit(&buffer, &remove, &[]).unwrap();
        let after: Vec<Ev> = events_of(&edited).into_iter().map(|(event, _)| event).collect();
        let gone = difference(&events, &after);
        let kept = difference(&events, &gone);
        let took = started.elapsed();
        assert_eq!((edited.len(), after.len()), (30_001, 30_000));
        assert_eq!((gone, kept), (remove, after));
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn requests_become_reapers_bytes() {
        let at = |spec: Value| new_events(&spec, 0).unwrap().into_iter().map(|e| e.msg).collect::<Vec<_>>();
        assert_eq!(
            at(json!({"type": "cc", "channel": 2, "controller": 1, "value": 9000, "fourteenBit": true})),
            vec![vec![0xB1, 1, 70], vec![0xB1, 33, 40]]
        );
        assert_eq!(at(json!({"type": "pitchbend", "channel": 1, "value": 0})), vec![vec![0xE0, 0, 64]]);
        assert_eq!(at(json!({"type": "pitchbend", "channel": 1, "value": -8192})), vec![vec![0xE0, 0, 0]]);
        assert_eq!(
            at(json!({"type": "program", "channel": 10, "program": 5, "bank": 129})),
            vec![vec![0xB9, 0, 1], vec![0xB9, 32, 1], vec![0xC9, 5]]
        );
        assert_eq!(at(json!({"type": "text", "textType": "lyric", "text": "la"})), vec![vec![0xFF, 5, b'l', b'a']]);
        assert_eq!(at(json!({"type": "sysex", "bytes": "F0 7E 7F 09 01 F7"})), vec![vec![0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7]]);
        let bezier =
            new_events(&json!({"type": "cc", "channel": 1, "controller": 74, "value": 1, "shape": "bezier", "tension": 0.25}), 0).unwrap();
        assert_eq!((bezier[0].flag, tension_of(bezier[0].bezier.as_deref())), (0x50, Some(0.25)));
        for (spec, needle) in [
            (json!({"type": "cc", "channel": 0, "controller": 1, "value": 1}), "channel"),
            (json!({"type": "cc", "channel": 1, "controller": 40, "value": 1, "fourteenBit": true}), "controller"),
            (json!({"type": "cc", "channel": 1, "controller": 1, "value": 1, "tension": 0.5}), "bezier"),
            (json!({"type": "sysex", "bytes": "F0 90 F7"}), "data bytes"),
            (json!({"type": "aftertouch"}), "no event type"),
        ] {
            let (code, message) = new_events(&spec, 0).unwrap_err();
            assert_eq!(code, "bad-params", "{spec}");
            assert!(message.contains(needle), "{spec}: {message}");
        }
    }
}
