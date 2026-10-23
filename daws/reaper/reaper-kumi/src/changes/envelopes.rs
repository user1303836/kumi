//! Envelopes (M4): a track's, a send's, an FX parameter's and a take's. One is shown and turned on (made, where REAPER
//! has none yet), its points are added, changed, deleted or cleared from a range, automation items are added (a new pool
//! or a pooled copy), changed and deleted, and a track's automation mode and the global override are set.
//!
//! An envelope is found by its GUID; an automation item, which REAPER gives no id of its own, by its pool and where it
//! starts (two of one pool at one place name neither, and Kumi refuses). Points have no identity either: an edit is fenced
//! by the whole lane it touches (the envelope's own points, or an automation item's pool), and an undo puts that lane
//! back point for point, only while it holds exactly what Kumi wrote.
//!
//! Values are in the envelope's own units: dB for volume (and trim), -1 (left) to 1 (right) for pan, -1 to 1 for width, 1
//! for muted, semitones for a take's pitch, and an FX parameter's own value (its raw value, or the text the envelope shows
//! for it). Times are positionBeats or position (seconds) on the project's timeline; a take envelope's points are kept from
//! the item's start, which Kumi works out.

use serde_json::{json, Value};

use super::fxchain::{changed, gone, gone_if_missing, mark_envelope, went, Holder};
use super::{marks, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{EnvPoint, Envelope, Reaper, Take, Track};
use crate::refs::{self, EnvelopeId, Ref, SendEnd, TrackId};
use crate::rows;

/// The most points a lane may hold for Kumi to change it: an undo keeps them all.
const MAX_POINTS: usize = 20_000;
/// The most points one change adds.
const MAX_NEW_POINTS: usize = 10_000;
/// An automation item's points as one loop of its pool (REAPER's flag on the item's index).
const ONE_LOOP: i32 = 0x1000_0000;

#[derive(Clone, Copy)]
pub enum Edit {
    Add,
    Change,
    Delete,
    Clear,
}

// ----- finding an envelope -----

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum EnvOwner {
    Track(Track),
    Take(Take),
}

#[derive(Clone)]
pub(crate) struct EnvAt {
    pub envelope: Envelope,
    pub guid: String,
    pub owner: EnvOwner,
}

fn take_envelope(reaper: &Reaper, take: Take, guid: &str) -> Option<Envelope> {
    (0..reaper.take_envelope_count(take))
        .filter_map(|i| reaper.take_envelope(take, i))
        .find(|&envelope| reaper.envelope_guid(envelope).is_some_and(|g| g.eq_ignore_ascii_case(guid)))
}

pub(crate) fn envelope_at(ctx: &Ctx, reference: &str) -> Done<EnvAt> {
    match Ref::parse(reference).map_err(bad)? {
        Ref::Envelope { track, which } => {
            let envelope = ctx.reads.envelope_by_ref(ctx.reaper, ctx.proj, &track, &which)?;
            let owner = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &track)?;
            let guid = ctx.reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
            Ok(EnvAt { envelope, guid, owner: EnvOwner::Track(owner) })
        }
        Ref::TakeEnvelope { take, guid } => {
            let owner = ctx.reads.take_by_ref(ctx.reaper, ctx.proj, &take)?;
            let envelope = take_envelope(ctx.reaper, owner, &guid)
                .ok_or_else(|| failed("that take has no such envelope (deleted, or another take)"))?;
            Ok(EnvAt { envelope, guid, owner: EnvOwner::Take(owner) })
        }
        other => Err(bad(format!("ref must be an envelope ref, not r:{}", other.kind()))),
    }
}

pub(crate) fn env_ref(ctx: &Ctx, at: &EnvAt) -> String {
    match at.owner {
        EnvOwner::Track(track) => refs::envelope_ref(&rows::track_id(ctx.reaper, track), &EnvelopeId::Guid(at.guid.clone())),
        EnvOwner::Take(take) => refs::take_envelope_ref(&ctx.reaper.take_guid(take), &at.guid),
    }
}

/// An envelope as a title says it: "“Bass”'s Volume envelope", "take “vox.wav”'s Pitch envelope".
fn env_label(ctx: &Ctx, at: &EnvAt) -> String {
    let name = rows::clip(&ctx.reaper.envelope_name(at.envelope));
    let owner = match at.owner {
        EnvOwner::Track(track) if rows::track_id(ctx.reaper, track) == TrackId::Master => "the master".to_string(),
        EnvOwner::Track(track) => super::quoted(&super::track_name(ctx.reaper, track)),
        EnvOwner::Take(take) => {
            let name = rows::clip(&ctx.reaper.take_name(take));
            if name.is_empty() {
                "a take".into()
            } else {
                format!("take {}", super::quoted(&name))
            }
        }
    };
    format!("{owner}'s {name} envelope")
}

// ----- units -----

#[derive(Clone, Copy, Debug, PartialEq)]
enum Units {
    /// Volume (fader scaling, its scaling mode).
    Db(i32),
    Pan,
    Width,
    Mute,
    Semitones,
    Tempo,
    /// An FX parameter's own value, from `min` to `max`; any other envelope Kumi has no units for.
    Raw {
        min: f64,
        max: f64,
    },
}

fn units_of(ctx: &Ctx, envelope: Envelope) -> Done<Units> {
    let chunk = ctx.reaper.env_chunk(envelope).ok_or_else(|| failed("REAPER wouldn't give the envelope's state"))?;
    let head = chunk.lines().next().unwrap_or_default().trim().trim_start_matches('<').to_string();
    let words: Vec<&str> = head.split_whitespace().collect();
    Ok(match words.first().copied().unwrap_or_default() {
        "VOLENV" | "VOLENV2" | "VOLENV3" | "AUXVOLENV" => Units::Db(ctx.reaper.env_scaling(envelope)),
        "PANENV" | "PANENV2" | "AUXPANENV" => Units::Pan,
        "WIDTHENV" | "WIDTHENV2" => Units::Width,
        "MUTEENV" | "AUXMUTEENV" => Units::Mute,
        "PITCHENV" => Units::Semitones,
        "TEMPOENVEX" => Units::Tempo,
        "PARMENV" => {
            let number = |i: usize| words.get(i).and_then(|w| w.parse::<f64>().ok());
            match (number(2), number(3)) {
                (Some(min), Some(max)) if max > min => Units::Raw { min, max },
                _ => Units::Raw { min: f64::NEG_INFINITY, max: f64::INFINITY },
            }
        }
        _ => Units::Raw { min: f64::NEG_INFINITY, max: f64::INFINITY },
    })
}

fn units_word(units: Units) -> &'static str {
    match units {
        Units::Db(_) => "dB",
        Units::Pan => "pan",
        Units::Width => "width",
        Units::Mute => "mute",
        Units::Semitones => "semitones",
        Units::Tempo => "bpm",
        Units::Raw { .. } => "raw",
    }
}

/// A point's value in the envelope's units, as REAPER keeps it.
fn to_raw(reaper: &Reaper, units: Units, value: f64) -> Done<f64> {
    let check = |ok: bool, words: &str| if ok { Ok(()) } else { Err(bad(words.to_string())) };
    match units {
        Units::Db(mode) => {
            check(value.is_finite() && value <= 24.0, "a volume point's value is dB, at most 24 (-150 or less is silence)")?;
            Ok(reaper.scale_to_envelope(mode, if value <= -150.0 { 0.0 } else { 10f64.powf(value / 20.0) }))
        }
        Units::Pan => check((-1.0..=1.0).contains(&value), "a pan point's value is from -1 (left) to 1 (right)").map(|_| -value),
        Units::Width => check((-1.0..=1.0).contains(&value), "a width point's value is from -1 to 1").map(|_| value),
        Units::Mute => check(value == 0.0 || value == 1.0, "a mute point's value is 1 (muted) or 0 (playing)").map(|_| 1.0 - value),
        Units::Semitones => {
            check(value.is_finite() && value.abs() <= 96.0, "a pitch point's value is semitones, from -96 to 96").map(|_| value)
        }
        Units::Tempo => Err(bad("the tempo map changes through tempo_marker_set, not its envelope")),
        Units::Raw { min, max } => check(
            value.is_finite() && value >= min - 1e-9 && value <= max + 1e-9,
            &format!("that envelope's values run from {min} to {max}"),
        )
        .map(|_| value),
    }
}

/// A raw value in the envelope's units (the inverse of `to_raw`).
fn from_raw(reaper: &Reaper, units: Units, raw: f64) -> f64 {
    match units {
        Units::Db(mode) => {
            let gain = reaper.scale_from_envelope(mode, raw);
            if gain <= 0.0 {
                -150.0
            } else {
                (20.0 * gain.log10() * 100.0).round() / 100.0
            }
        }
        Units::Pan => -raw,
        Units::Mute => 1.0 - raw,
        _ => raw,
    }
}

// ----- lanes: the envelope's own points, or an automation item's -----

#[derive(Clone, Copy, Debug)]
struct ItemAt {
    index: i32,
    pool: i64,
    position: f64,
}

struct Lane {
    at: EnvAt,
    item: Option<ItemAt>,
}

impl Lane {
    /// The index REAPER's point reads take (an item's: one loop of its pool).
    fn read(&self) -> i32 {
        self.item.map_or(-1, |item| item.index | ONE_LOOP)
    }

    /// The index REAPER's point writes take.
    fn write(&self) -> i32 {
        self.item.map_or(-1, |item| item.index)
    }
}

fn find_item(ctx: &Ctx, at: &EnvAt, pool: i64, position: f64) -> Done<ItemAt> {
    let reaper = ctx.reaper;
    let found: Vec<i32> = (0..reaper.ai_count(at.envelope))
        // (to a nanosecond: REAPER's undo and redo bring a position back from text, its last bits changed)
        .filter(|&i| {
            reaper.ai_value(at.envelope, i, "D_POOL_ID") as i64 == pool
                && (reaper.ai_value(at.envelope, i, "D_POSITION") - position).abs() <= 1e-9
        })
        .collect();
    match found.as_slice() {
        [index] => Ok(ItemAt { index: *index, pool, position }),
        [] => Err(failed("that automation item isn't there (moved, or deleted): read the envelope's automation items again")),
        _ => Err(changed("two automation items of that pool start at that place: Kumi can't tell which is meant")),
    }
}

fn item_ref(ctx: &Ctx, at: &EnvAt, pool: i64, position: f64) -> Done<String> {
    match at.owner {
        EnvOwner::Track(track) => Ok(refs::auto_item_ref(&rows::track_id(ctx.reaper, track), &at.guid, pool, position)),
        EnvOwner::Take(_) => Err(bad("a take's envelopes have no automation items")),
    }
}

fn lane_named(ctx: &Ctx, reference: &Value) -> Done<Lane> {
    let text = reference.as_str().ok_or_else(|| bad("ref must be an envelope or automation item ref"))?;
    match Ref::parse(text).map_err(bad)? {
        Ref::AutoItem { track, envelope, pool, position } => {
            let at = envelope_at(ctx, &refs::envelope_ref(&track, &EnvelopeId::Guid(envelope)))?;
            let position: f64 = position.parse().map_err(|_| bad("that automation item ref's position isn't a number"))?;
            let item = find_item(ctx, &at, pool, position)?;
            Ok(Lane { at, item: Some(item) })
        }
        _ => Ok(Lane { at: envelope_at(ctx, text)?, item: None }),
    }
}

fn lane_ref(ctx: &Ctx, lane: &Lane) -> Done<String> {
    match lane.item {
        Some(item) => item_ref(ctx, &lane.at, item.pool, item.position),
        None => Ok(env_ref(ctx, &lane.at)),
    }
}

fn lane_label(ctx: &Ctx, lane: &Lane) -> String {
    match lane.item {
        Some(item) => format!("the automation item at {} on {}", bar_at(ctx, item.position), env_label(ctx, &lane.at)),
        None => env_label(ctx, &lane.at),
    }
}

/// A length or an offset, in seconds.
fn seconds(time: f64) -> String {
    format!("{:.3} s", time).replace(".000 s", " s")
}

/// Where a time on the project's timeline is, as items and markers say it: `bar 6`, `bar 6 beat 2.5`.
fn bar_at(ctx: &Ctx, time: f64) -> String {
    super::marks::bar_text(ctx.reaper, ctx.proj, time)
}

fn lane_points(ctx: &Ctx, lane: &Lane) -> Vec<EnvPoint> {
    let count = ctx.reaper.env_point_count(lane.at.envelope, lane.read());
    (0..count).filter_map(|i| ctx.reaper.env_point(lane.at.envelope, lane.read(), i)).collect()
}

/// A number as REAPER's envelope state writes it, `places` after the point. REAPER's undo and redo bring envelopes back
/// through that state (lab 7.82: a point keeps 8 places, its value as the state stores it; an automation item 14 for its
/// times and 6 for baseline, amplitude and playrate), so Kumi compares what survives them, never the last bits.
fn kept(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    // (−0 is 0)
    if text.starts_with('-') && text.bytes().skip(1).all(|b| b == b'0' || b == b'.') {
        return text[1..].to_string();
    }
    text
}

/// A lane's points as one hash: time, value, shape and tension as the envelope's state keeps them (a volume's value
/// unscaled, as the state stores it), not the selection, which the producer's clicks change.
fn points_hash(reaper: &Reaper, envelope: Envelope, points: &[EnvPoint]) -> String {
    let mode = reaper.env_scaling(envelope);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for point in points {
        let value = if mode == 0 { point.value } else { reaper.scale_from_envelope(mode, point.value) };
        let text = format!("{} {} {} {};", kept(point.time, 8), kept(value, 8), point.shape, kept(point.tension, 8));
        for byte in text.bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}-{}", points.len())
}

fn points_json(points: &[EnvPoint]) -> Value {
    points.iter().map(|p| json!([p.time, p.value, p.shape, p.tension, p.selected])).collect::<Vec<_>>().into()
}

fn points_from(value: &Value) -> Done<Vec<EnvPoint>> {
    value
        .as_array()
        .ok_or_else(|| bad("points: the lane's points"))?
        .iter()
        .map(|p| {
            Some(EnvPoint {
                time: p.get(0)?.as_f64()?,
                value: p.get(1)?.as_f64()?,
                shape: p.get(2)?.as_i64()? as i32,
                tension: p.get(3)?.as_f64()?,
                selected: p.get(4)?.as_bool()?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad("points: [time, value, shape, tension, selected] each"))
}

/// Writes a lane's points back as `points` holds them: everything in the lane goes, the points go in, it is sorted.
fn write_lane(ctx: &Ctx, lane: &Lane, points: &[EnvPoint]) {
    let reaper = ctx.reaper;
    let now = lane_points(ctx, lane);
    let (low, high) = now.iter().chain(points).fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| (l.min(p.time), h.max(p.time)));
    if low.is_finite() {
        reaper.env_delete_range(lane.at.envelope, lane.write(), low - 1.0, high + 1.0);
    }
    for point in points {
        reaper.env_insert_point(lane.at.envelope, lane.write(), *point, true);
    }
    reaper.env_sort(lane.at.envelope, lane.write());
    if lane.item.is_some() {
        mark_envelope(reaper, lane.at.envelope);
    }
}

const SHAPES: [(&str, i32); 6] = [("linear", 0), ("square", 1), ("slow", 2), ("fast-start", 3), ("fast-end", 4), ("bezier", 5)];

fn shape_of(value: Option<&Value>) -> Done<Option<i32>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => SHAPES
            .iter()
            .find(|(word, _)| word == name)
            .map(|(_, code)| Some(*code))
            .ok_or_else(|| bad(format!("shape must be one of {}", SHAPES.map(|s| s.0).join(", ")))),
        Some(_) => Err(bad("shape must be a shape's name")),
    }
}

fn shape_word(code: i32) -> &'static str {
    SHAPES.iter().find(|(_, c)| *c == code).map_or("other", |(word, _)| word)
}

/// Where a point given on the project's timeline goes in the lane: a take envelope's from the item's start (inside the
/// item), an automation item's inside the item.
fn lane_time(ctx: &Ctx, lane: &Lane, time: f64) -> Done<f64> {
    if let Some(item) = lane.item {
        let length = ctx.reaper.ai_value(lane.at.envelope, item.index, "D_LENGTH");
        if time < item.position - 1e-9 || time > item.position + length + 1e-9 {
            return Err(bad(format!(
                "that automation item runs from {} to {}",
                bar_at(ctx, item.position),
                bar_at(ctx, item.position + length)
            )));
        }
        return Ok(time);
    }
    match lane.at.owner {
        EnvOwner::Track(_) => Ok(time),
        EnvOwner::Take(take) => {
            let item = ctx.reaper.take_item(take).ok_or_else(|| failed("REAPER doesn't say which item the take is on"))?;
            let (start, length) = (ctx.reaper.item_value(item, "D_POSITION"), ctx.reaper.item_value(item, "D_LENGTH"));
            if time < start - 1e-9 || time > start + length + 1e-9 {
                return Err(bad(format!(
                    "a take envelope's points are inside its item, from {} to {}",
                    bar_at(ctx, start),
                    bar_at(ctx, start + length)
                )));
            }
            Ok((time - start).max(0.0))
        }
    }
}

/// A point's time on the project's timeline (the inverse of `lane_time`), as a read shows it.
fn project_time(ctx: &Ctx, lane: &Lane, time: f64) -> f64 {
    match (lane.item, lane.at.owner) {
        (None, EnvOwner::Take(take)) => time + ctx.reaper.take_item(take).map_or(0.0, |item| ctx.reaper.item_value(item, "D_POSITION")),
        _ => time,
    }
}

/// The value a point asks for, as REAPER keeps it: `value` in the envelope's units, `raw`, or `text` (what the envelope
/// shows, found by asking it to print values: an FX parameter's own words).
fn point_value(ctx: &Ctx, lane: &Lane, units: Units, point: &Value) -> Done<Option<f64>> {
    let (value, raw, text) = (
        point.get("value").filter(|v| !v.is_null()),
        point.get("raw").filter(|v| !v.is_null()),
        point.get("text").filter(|v| !v.is_null()),
    );
    match (value, raw, text) {
        (None, None, None) => Ok(None),
        (Some(v), None, None) => {
            v.as_f64().ok_or_else(|| bad("value must be a number")).and_then(|v| to_raw(ctx.reaper, units, v)).map(Some)
        }
        (None, Some(r), None) => {
            let raw = r.as_f64().filter(|r| r.is_finite()).ok_or_else(|| bad("raw must be a number"))?;
            if let Units::Raw { min, max } = units {
                if raw < min - 1e-9 || raw > max + 1e-9 {
                    return Err(bad(format!("that envelope's values run from {min} to {max}")));
                }
            }
            Ok(Some(raw))
        }
        (None, None, Some(Value::String(words))) => {
            let Units::Raw { min, max } = units else { return Err(bad("text is for an FX parameter's envelope: give value for this one")) };
            if !min.is_finite() || !max.is_finite() {
                return Err(bad("REAPER doesn't say that envelope's range: give raw"));
            }
            let ask = |n: f64| Some(ctx.reaper.env_format(lane.at.envelope, min + n * (max - min)));
            super::fx::find_printed(&ask, words).map(|n| Some(min + n * (max - min)))
        }
        _ => Err(bad("give one of value (the envelope's units), raw or text for a point")),
    }
}

fn point_row(ctx: &Ctx, lane: &Lane, units: Units, point: &EnvPoint) -> Value {
    json!({
        "position": project_time(ctx, lane, point.time),
        "value": from_raw(ctx.reaper, units, point.value),
        "raw": point.value,
        "display": ctx.reaper.env_format(lane.at.envelope, point.value),
        "shape": shape_word(point.shape),
        "tension": point.tension,
        "selected": point.selected,
    })
}

// ----- points added, changed, deleted, cleared -----

/// What an edit of a lane asks for, read and checked against the lane before anything is written.
enum Plan {
    Add(Vec<EnvPoint>),
    Change(Vec<(i32, EnvPoint)>),
    Delete(Vec<i32>),
    Clear(f64, f64),
}

fn plan_of(ctx: &Ctx, edit: Edit, lane: &Lane, units: Units, now: &[EnvPoint], input: &Value) -> Done<Plan> {
    match edit {
        Edit::Add => {
            let list =
                input.get("points").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or_else(|| bad("points: a list of points"))?;
            if list.len() > MAX_NEW_POINTS || now.len() + list.len() > MAX_POINTS {
                return Err(bad(format!("at most {MAX_NEW_POINTS} points at once, and {MAX_POINTS} in a lane")));
            }
            list.iter()
                .map(|p| {
                    let time = marks::moment(ctx, p, "positionBeats", "position")?
                        .ok_or_else(|| bad("each point needs positionBeats or position"))?;
                    let raw = point_value(ctx, lane, units, p)?.ok_or_else(|| bad("each point needs a value"))?;
                    let tension = p.get("tension").and_then(Value::as_f64).unwrap_or(0.0);
                    if !(-1.0..=1.0).contains(&tension) {
                        return Err(bad("tension must be from -1 to 1"));
                    }
                    Ok(EnvPoint {
                        time: lane_time(ctx, lane, time)?,
                        value: raw,
                        shape: shape_of(p.get("shape"))?.unwrap_or(0),
                        tension,
                        selected: p.get("selected").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect::<Done<Vec<_>>>()
                .map(Plan::Add)
        }
        Edit::Change => {
            let list = input
                .get("changes")
                .and_then(Value::as_array)
                .filter(|l| !l.is_empty())
                .ok_or_else(|| bad("changes: a list of {index, …}"))?;
            let mut seen = std::collections::HashSet::new();
            list.iter()
                .map(|c| {
                    let index = c.get("index").and_then(Value::as_u64).filter(|i| (*i as usize) < now.len()).ok_or_else(|| {
                        bad(format!("changes[].index must be a point of the lane, from 0 to {}", now.len().saturating_sub(1)))
                    })? as i32;
                    if !seen.insert(index) {
                        return Err(bad(format!("point {index} is changed twice")));
                    }
                    let mut point = now[index as usize];
                    if let Some(time) = marks::moment(ctx, c, "positionBeats", "position")? {
                        point.time = lane_time(ctx, lane, time)?;
                    }
                    if let Some(raw) = point_value(ctx, lane, units, c)? {
                        point.value = raw;
                    }
                    if let Some(shape) = shape_of(c.get("shape"))? {
                        point.shape = shape;
                    }
                    if let Some(tension) = c.get("tension").filter(|v| !v.is_null()) {
                        point.tension =
                            tension.as_f64().filter(|t| (-1.0..=1.0).contains(t)).ok_or_else(|| bad("tension must be from -1 to 1"))?;
                    }
                    if let Some(selected) = c.get("selected").and_then(Value::as_bool) {
                        point.selected = selected;
                    }
                    Ok((index, point))
                })
                .collect::<Done<Vec<_>>>()
                .map(Plan::Change)
        }
        Edit::Delete => {
            let list = input
                .get("indexes")
                .and_then(Value::as_array)
                .filter(|l| !l.is_empty())
                .ok_or_else(|| bad("indexes: the points to delete"))?;
            let mut indexes = list
                .iter()
                .map(|i| i.as_u64().filter(|i| (*i as usize) < now.len()).map(|i| i as i32))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| bad(format!("indexes are points of the lane, from 0 to {}", now.len().saturating_sub(1))))?;
            indexes.sort_unstable();
            indexes.dedup();
            Ok(Plan::Delete(indexes))
        }
        Edit::Clear => {
            let start = marks::moment(ctx, input, "startBeats", "start")?.ok_or_else(|| bad("pass startBeats or start"))?;
            let end = marks::moment(ctx, input, "endBeats", "end")?.ok_or_else(|| bad("pass endBeats or end"))?;
            if end <= start {
                return Err(bad("the range ends after it starts"));
            }
            let (start, end) = match (lane.item, lane.at.owner) {
                (None, EnvOwner::Take(take)) => {
                    let base = ctx.reaper.take_item(take).map_or(0.0, |item| ctx.reaper.item_value(item, "D_POSITION"));
                    (start - base, end - base)
                }
                _ => (start, end),
            };
            Ok(Plan::Clear(start, end))
        }
    }
}

fn plan_count(plan: &Plan, now: &[EnvPoint]) -> usize {
    match plan {
        Plan::Add(points) => points.len(),
        Plan::Change(changes) => changes.len(),
        Plan::Delete(indexes) => indexes.len(),
        Plan::Clear(start, end) => now.iter().filter(|p| p.time >= *start && p.time < *end).count(),
    }
}

fn plan_title(ctx: &Ctx, lane: &Lane, plan: &Plan, count: usize) -> String {
    let points = if count == 1 { "1 point".to_string() } else { format!("{count} points") };
    let label = lane_label(ctx, lane);
    match plan {
        Plan::Add(_) => format!("Added {points} to {label}"),
        Plan::Change(_) => format!("Changed {points} of {label}"),
        Plan::Delete(_) => format!("Deleted {points} from {label}"),
        Plan::Clear(start, end) => {
            format!(
                "Cleared {label} from {} to {} ({points})",
                bar_at(ctx, project_time(ctx, lane, *start)),
                bar_at(ctx, project_time(ctx, lane, *end))
            )
        }
    }
}

/// The most points of each side HISTORY's picture of a lane keeps.
const DRAWN_POINTS: usize = 128;

/// A lane's points before and after as HISTORY draws them (for the display only: no undo reads it): `[time, value]`, the
/// time in seconds as the lane keeps it, the value in the envelope's units, at most `DRAWN_POINTS` a side (spread evenly,
/// the ends kept); the units' range, and the envelope's name.
fn drawn_curve(ctx: &Ctx, lane: &Lane, units: Units, before: &[EnvPoint], after: &[EnvPoint]) -> Value {
    let (floor, top) = match units {
        Units::Db(_) => (-60.0, 12.0),
        Units::Pan | Units::Width => (-1.0, 1.0),
        Units::Mute => (0.0, 1.0),
        Units::Semitones => (-24.0, 24.0),
        Units::Raw { min, max } if min.is_finite() && max.is_finite() => (min, max),
        _ => (f64::NAN, f64::NAN),
    };
    let side = |points: &[EnvPoint]| -> Vec<Value> {
        let shown = |p: &EnvPoint| {
            let value = from_raw(ctx.reaper, units, p.value);
            json!([p.time, if floor.is_finite() { value.max(floor) } else { value }])
        };
        if points.len() <= DRAWN_POINTS {
            return points.iter().map(shown).collect();
        }
        let last = points.len() - 1;
        (0..DRAWN_POINTS).map(|i| shown(&points[i * last / (DRAWN_POINTS - 1)])).collect()
    };
    let words = curve_words(ctx, units, if after.is_empty() { before } else { after });
    let (before, after) = (side(before), side(after));
    // A lane in units of its own (an FX parameter of no known range): the range its points span.
    let (min, max) = if floor.is_finite() {
        (floor, top)
    } else {
        let values = || before.iter().chain(&after).filter_map(|point| point[1].as_f64());
        (values().fold(f64::INFINITY, f64::min), values().fold(f64::NEG_INFINITY, f64::max))
    };
    let (min, max) = if max > min { (min, max) } else { (min - 1.0, min + 1.0) };
    json!({"parameter": rows::clip(&ctx.reaper.envelope_name(lane.at.envelope)), "min": min, "max": max, "before": before, "after": after, "words": words})
}

/// What a lane's points move between and where, as the producer reads them: "-18 to 0 dB over bars 5–9".
fn curve_words(ctx: &Ctx, units: Units, points: &[EnvPoint]) -> Option<String> {
    let (first, last) = (points.first()?, points.last()?);
    let (low, high) = points
        .iter()
        .map(|point| from_raw(ctx.reaper, units, point.value))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| (low.min(value), high.max(value)));
    let said = |value: f64| match units {
        Units::Db(_) if value <= -150.0 => "-inf".to_string(),
        _ => marks::trimmed(value),
    };
    let unit = match units {
        Units::Db(_) => " dB",
        Units::Semitones => " semitones",
        Units::Tempo => " BPM",
        _ => "",
    };
    let range = if said(low) == said(high) { format!("{}{unit}", said(low)) } else { format!("{} to {}{unit}", said(low), said(high)) };
    let (from, to) = (marks::bar_text(ctx.reaper, ctx.proj, first.time), marks::bar_text(ctx.reaper, ctx.proj, last.time));
    let bar = |text: &str| text.strip_prefix("bar ").filter(|n| n.parse::<i64>().is_ok()).map(str::to_owned);
    Some(match (bar(&from), bar(&to)) {
        _ if from == to => format!("{range} at {from}"),
        (Some(a), Some(b)) => format!("{range} over bars {a}–{b}"),
        _ => format!("{range} from {from} to {to}"),
    })
}

fn lane_target(ctx: &Ctx, lane: &Lane) -> Done<Value> {
    Ok(json!({"ref": lane_ref(ctx, lane)?, "kind": if lane.item.is_some() { "aitem" } else { "env" }}))
}

pub fn points_preview(ctx: &Ctx, edit: Edit, input: &Value) -> Done<Preview> {
    let lane = lane_named(ctx, &input["ref"])?;
    let units = units_of(ctx, lane.at.envelope)?;
    if units == Units::Tempo {
        return Err(bad("the tempo map changes through tempo_marker_set, not its envelope"));
    }
    let now = lane_points(ctx, &lane);
    if now.len() > MAX_POINTS {
        return Err(failed(format!(
            "that lane has {} points: Kumi changes lanes of at most {MAX_POINTS}, which an undo can put back",
            now.len()
        )));
    }
    let plan = plan_of(ctx, edit, &lane, units, &now, input)?;
    let count = plan_count(&plan, &now);
    let hash = points_hash(ctx.reaper, lane.at.envelope, &now);
    let mut display = json!({"title": plan_title(ctx, &lane, &plan, count), "units": units_word(units)});
    if count == 0 {
        display["unchanged"] = true.into();
    }
    Ok(Preview {
        target: lane_target(ctx, &lane)?,
        prior: json!({"points": points_json(&now), "hash": hash}),
        fence: json!({"hash": hash}),
        display,
    })
}

pub fn points_apply(ctx: &Ctx, edit: Edit, input: &Value, fence: &Value) -> Done<Applied> {
    let lane = lane_named(ctx, &input["ref"]).map_err(went)?;
    let units = units_of(ctx, lane.at.envelope)?;
    let now = lane_points(ctx, &lane);
    if points_hash(ctx.reaper, lane.at.envelope, &now) != fence["hash"].as_str().unwrap_or_default() {
        return Err(changed(format!("{} changed since the preview", lane_label(ctx, &lane))));
    }
    let plan = plan_of(ctx, edit, &lane, units, &now, input)?;
    let count = plan_count(&plan, &now);
    let title = plan_title(ctx, &lane, &plan, count);
    if count == 0 {
        return Ok(Applied {
            applied: json!({"hash": points_hash(ctx.reaper, lane.at.envelope, &now)}),
            display: json!({"title": title}),
            unwritten: true,
            ..Default::default()
        });
    }
    let reaper = ctx.reaper;
    let (envelope, write) = (lane.at.envelope, lane.write());
    let expected = match &plan {
        Plan::Add(points) => {
            for point in points {
                reaper.env_insert_point(envelope, write, *point, true);
            }
            Some(now.len() + points.len())
        }
        Plan::Change(changes) => {
            for (index, point) in changes {
                reaper.env_set_point(envelope, lane.read(), *index, *point, true);
            }
            Some(now.len())
        }
        Plan::Delete(indexes) => {
            for index in indexes.iter().rev() {
                reaper.env_delete_point(envelope, lane.read(), *index);
            }
            Some(now.len() - indexes.len())
        }
        Plan::Clear(start, end) => {
            reaper.env_delete_range(envelope, write, *start, *end);
            Some(now.len() - count)
        }
    };
    reaper.env_sort(envelope, write);
    if lane.item.is_some() {
        mark_envelope(reaper, envelope);
    }
    let after = lane_points(ctx, &lane);
    if expected.is_some_and(|n| n != after.len()) {
        // What was written goes back: nothing half-done stays.
        write_lane(ctx, &lane, &now);
        let back = points_hash(reaper, envelope, &lane_points(ctx, &lane)) == points_hash(reaper, envelope, &now);
        return Err(if back {
            failed("REAPER didn't take the points as asked; the lane is put back")
        } else {
            ("uncertain".into(), "REAPER didn't take the points as asked, and Kumi could not put the lane back: read it".into())
        });
    }
    let display = json!({"title": title, "curve": drawn_curve(ctx, &lane, units, &now, &after)});
    Ok(Applied { applied: json!({"hash": points_hash(reaper, envelope, &after)}), display, ..Default::default() })
}

pub fn points_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let lane = lane_named(ctx, &restore.target["ref"]).map_err(gone_if_missing)?;
    let prior = points_from(&restore.prior["points"])?;
    let now = lane_points(ctx, &lane);
    let label = lane_label(ctx, &lane);
    let hash = |points: &[EnvPoint]| points_hash(ctx.reaper, lane.at.envelope, points);
    if hash(&now) == restore.prior["hash"].as_str().unwrap_or_default() {
        return Ok(Restored {
            restored: json!({"hash": hash(&now)}),
            display: json!({"title": format!("{label} is as it was")}),
            unwritten: true,
        });
    }
    if !restore.discard && hash(&now) != restore.applied["hash"].as_str().unwrap_or_default() {
        return Err(changed(format!("{label} has been changed in REAPER since Kumi changed it")));
    }
    write_lane(ctx, &lane, &prior);
    let after = lane_points(ctx, &lane);
    if hash(&after) != hash(&prior) {
        return Err(failed(format!("REAPER wouldn't put {label}'s points back")));
    }
    Ok(Restored {
        restored: json!({"hash": hash(&after)}),
        display: json!({"title": format!("{label} put back ({} points)", after.len())}),
        ..Default::default()
    })
}

// ----- an envelope shown and turned on -----

/// A track's built-in envelopes, by the name the model gives and REAPER's chunk name.
const TRACK_ENVELOPES: [(&str, &str); 8] = [
    ("volume", "<VOLENV2"),
    ("pan", "<PANENV2"),
    ("width", "<WIDTHENV2"),
    ("volume-pre-fx", "<VOLENV"),
    ("pan-pre-fx", "<PANENV"),
    ("width-pre-fx", "<WIDTHENV"),
    ("trim", "<VOLENV3"),
    ("mute", "<MUTEENV"),
];
/// A take's envelopes: REAPER's name for each, and the action that makes it on the selected items.
const TAKE_ENVELOPES: [(&str, &str, i32); 4] =
    [("volume", "Volume", 40693), ("pan", "Pan", 40694), ("mute", "Mute", 40695), ("pitch", "Pitch", 41612)];
/// A send's envelopes, by chunk name.
const SEND_ENVELOPES: [(&str, &str); 3] = [("volume", "<VOLENV"), ("pan", "<PANENV"), ("mute", "<MUTEENV")];

/// What an envelope-show is about: an envelope REAPER has (a track's and a send's always exist, hidden and off until
/// shown), or one it makes (an FX parameter's, a take's).
enum Shown {
    Has(EnvAt),
    MakeFx(super::fx::ParamAt),
    MakeTake(Take, &'static str, i32),
}

fn name_of(input: &Value) -> Done<&str> {
    input.get("name").and_then(Value::as_str).ok_or_else(|| bad("name: which envelope (volume, pan, …)"))
}

/// The send a send ref names (REAPER gives a send no id: the ref says which of the sends to one track it is, of how many,
/// and as two or more were then).
fn send_index(ctx: &Ctx, reference: &str, field: &str) -> Done<(Track, i32)> {
    // (a receive's ref names its send: r-tracks' `sends::canonical`)
    let reference =
        &super::sends::canonical(reference).map_err(|(code, message)| (code, message.replacen("ref must", &format!("{field} must"), 1)))?;
    let Ref::Send { track, category, other, repeat, of, seen } = Ref::parse(reference).map_err(bad)? else {
        return Err(bad(format!("{field} must be a send ref")));
    };
    if category != 0 || !matches!(other, SendEnd::Track(_)) {
        return Err(bad(format!("{field} must be a send to a track (a receive's is its send's)")));
    }
    let owner = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &track)?;
    let keys = rows::send_keys(ctx.reaper, owner, 0);
    let key = rows::SendKey { other, repeat, of };
    let index = keys
        .iter()
        .position(|k| *k == key)
        .ok_or_else(|| failed("that send isn't there (deleted, or the sends between those tracks changed): read them again"))?;
    if of >= 2 {
        let receiver = ctx.reaper.send_peer(owner, 0, index as i32);
        if !receiver.is_some_and(|receiver| super::send_seen_holds(ctx.reaper, owner, receiver, seen.as_deref())) {
            return Err(failed("the sends between those tracks changed since that ref was read: read the track's sends again"));
        }
    }
    Ok((owner, index as i32))
}

fn shown_of(ctx: &Ctx, input: &Value) -> Done<Shown> {
    let given: Vec<&str> =
        ["ref", "trackRef", "paramRef", "takeRef", "sendRef"].into_iter().filter(|k| input.get(*k).is_some_and(|v| !v.is_null())).collect();
    let reaper = ctx.reaper;
    match given.as_slice() {
        ["ref"] => Ok(Shown::Has(envelope_at(ctx, input["ref"].as_str().unwrap_or_default())?)),
        ["trackRef"] => {
            let name = name_of(input)?;
            let (_, chunk) = TRACK_ENVELOPES
                .iter()
                .find(|(word, _)| *word == name)
                .ok_or_else(|| bad(format!("a track's envelope is one of {}", TRACK_ENVELOPES.map(|e| e.0).join(", "))))?;
            let id = super::track_ref_of(&json!({"ref": input["trackRef"]}), "trackRef")?;
            let track = ctx.reads.track_by_id(reaper, ctx.proj, &id)?;
            let envelope =
                reaper.track_envelope_by_chunk_name(track, chunk).ok_or_else(|| failed("REAPER gives that track no such envelope"))?;
            let guid = reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
            Ok(Shown::Has(EnvAt { envelope, guid, owner: EnvOwner::Track(track) }))
        }
        ["sendRef"] => {
            let name = name_of(input)?;
            let (_, chunk) =
                SEND_ENVELOPES.iter().find(|(word, _)| *word == name).ok_or_else(|| bad("a send's envelope is volume, pan or mute"))?;
            let (track, index) = send_index(ctx, input["sendRef"].as_str().unwrap_or_default(), "sendRef")?;
            let envelope = reaper.send_envelope(track, index, chunk).ok_or_else(|| failed("REAPER gives that send no such envelope"))?;
            let guid = reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
            Ok(Shown::Has(EnvAt { envelope, guid, owner: EnvOwner::Track(track) }))
        }
        ["paramRef"] => {
            let found = super::fx::locate_param(ctx, &json!({"ref": input["paramRef"]}))?;
            if matches!(found.at.holder, Holder::Container(_)) {
                return Err(bad("REAPER automates a parameter inside a container through a macro: map it (fx_container_map) and show the macro's envelope"));
            }
            match reaper.fx_envelope(found.at.fx, found.param, false) {
                Some(envelope) => {
                    let guid = reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
                    let owner = match found.at.fx.owner {
                        crate::reaper::FxOwner::Track(track) => EnvOwner::Track(track),
                        crate::reaper::FxOwner::Take(take) => EnvOwner::Take(take),
                    };
                    Ok(Shown::Has(EnvAt { envelope, guid, owner }))
                }
                None => Ok(Shown::MakeFx(found)),
            }
        }
        ["takeRef"] => {
            let name = name_of(input)?;
            let (_, reaper_name, action) = TAKE_ENVELOPES
                .iter()
                .find(|(word, _, _)| *word == name)
                .ok_or_else(|| bad("a take's envelope is volume, pan, mute or pitch"))?;
            let take = match Ref::parse(input["takeRef"].as_str().unwrap_or_default()).map_err(bad)? {
                Ref::Take(guid) => ctx.reads.take_by_ref(reaper, ctx.proj, &guid)?,
                Ref::Item(guid) => {
                    let item = ctx.reads.item_by_ref(reaper, ctx.proj, &guid)?;
                    reaper.active_take(item).ok_or_else(|| failed("that item has no take"))?
                }
                other => return Err(bad(format!("takeRef must be a take or item ref, not r:{}", other.kind()))),
            };
            match reaper.take_envelope_by_name(take, reaper_name) {
                Some(envelope) => {
                    let guid = reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
                    Ok(Shown::Has(EnvAt { envelope, guid, owner: EnvOwner::Take(take) }))
                }
                None => Ok(Shown::MakeTake(take, reaper_name, *action)),
            }
        }
        [] => Err(bad("name the envelope: ref, trackRef and name, sendRef and name, paramRef, or takeRef and name")),
        _ => Err(bad("name the envelope one way only")),
    }
}

/// A write to an envelope, noted for the sends between two tracks when it is one of a send's: whether it is on is part of
/// what a ref to one of several sends between the same tracks carries (`sends::noting_writes`).
fn writing<T>(ctx: &Ctx, at: &EnvAt, write: impl FnOnce() -> T) -> T {
    let reaper = ctx.reaper;
    let send = match at.owner {
        EnvOwner::Track(track) => (0..reaper.send_count(track, 0)).find_map(|index| {
            SEND_ENVELOPES.iter().any(|(_, chunk)| reaper.send_envelope(track, index, chunk) == Some(at.envelope)).then_some((track, index))
        }),
        EnvOwner::Take(_) => None,
    };
    match send {
        Some((track, index)) => super::sends::noting_writes(reaper, track, index, write),
        None => write(),
    }
}

/// The flags a show asks for: shown and on unless said otherwise; armed only when given.
fn wanted_flags(input: &Value) -> Done<(Option<bool>, Option<bool>, Option<bool>)> {
    let flag = |key: &str| -> Done<Option<bool>> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(bad(format!("{key} must be true or false"))),
        }
    };
    let (visible, active, armed) = (flag("visible")?, flag("active")?, flag("armed")?);
    if visible.is_none() && active.is_none() && armed.is_none() {
        return Ok((Some(true), Some(true), None));
    }
    Ok((visible, active, armed))
}

fn flags_of(reaper: &Reaper, envelope: Envelope) -> Value {
    json!({"visible": reaper.envelope_flag(envelope, "VISIBLE"), "active": reaper.envelope_flag(envelope, "ACTIVE"), "armed": reaper.envelope_flag(envelope, "ARM")})
}

/// What envelope-show fences: the envelope's flags, its points, and its automation items, each as its state keeps them
/// (an undo writes the whole state from before back, items' places, lengths, loops and mutes among it).
fn env_state(ctx: &Ctx, envelope: Envelope) -> Value {
    let lane_hash = {
        let count = ctx.reaper.env_point_count(envelope, -1);
        let points: Vec<EnvPoint> = (0..count).filter_map(|i| ctx.reaper.env_point(envelope, -1, i)).collect();
        points_hash(ctx.reaper, envelope, &points)
    };
    json!({
        "flags": flags_of(ctx.reaper, envelope),
        "points": lane_hash,
        "items": ctx.reaper.ai_count(envelope),
        "itemsPrint": items_print(ctx, envelope),
    })
}

/// Whether the envelope's state now is the recorded one (a state recorded before items were kept by their print is
/// compared without it).
fn same_env(now: &Value, recorded: &Value) -> bool {
    if recorded.get("itemsPrint").is_some() {
        return now == recorded;
    }
    let mut now = now.clone();
    if let Some(fields) = now.as_object_mut() {
        fields.remove("itemsPrint");
    }
    now == *recorded
}

fn set_flags(reaper: &Reaper, envelope: Envelope, (visible, active, armed): (Option<bool>, Option<bool>, Option<bool>)) {
    if let Some(on) = active {
        reaper.set_env_flag(envelope, "ACTIVE", on);
    }
    if let Some(on) = visible {
        reaper.set_env_flag(envelope, "VISIBLE", on);
    }
    if let Some(on) = armed {
        reaper.set_env_flag(envelope, "ARM", on);
    }
    mark_envelope(reaper, envelope);
}

fn show_title(label: &str, (visible, active, armed): (Option<bool>, Option<bool>, Option<bool>)) -> String {
    let mut parts = Vec::new();
    match (visible, active) {
        (Some(true), Some(true)) => parts.push("shown and on".to_string()),
        _ => {
            if let Some(v) = visible {
                parts.push(if v { "shown" } else { "hidden" }.into());
            }
            if let Some(a) = active {
                parts.push(if a { "on" } else { "off (bypassed)" }.into());
            }
        }
    }
    if let Some(a) = armed {
        parts.push(if a { "armed".into() } else { "disarmed".to_string() });
    }
    let mut chars = label.chars();
    let label = chars.next().map_or(String::new(), |c| c.to_uppercase().collect::<String>() + chars.as_str());
    format!("{label} {}", parts.join(", "))
}

fn shown_label(ctx: &Ctx, shown: &Shown) -> String {
    match shown {
        Shown::Has(at) => env_label(ctx, at),
        Shown::MakeFx(found) => format!(
            "{}'s {} / {} envelope",
            super::fx::place_phrase(ctx, found.at.holder).trim_start_matches("on ").trim_start_matches("in "),
            rows::clip(&ctx.reaper.param_name(found.at.fx, found.param)),
            super::fx::fx_label(ctx, found.at.fx)
        ),
        Shown::MakeTake(take, name, _) => format!("take {}'s {name} envelope", super::quoted(&rows::clip(&ctx.reaper.take_name(*take)))),
    }
}

/// An envelope's chunk with nothing in it: hidden, off, unarmed and without points (REAPER takes an FX parameter's
/// envelope so written out of its list).
fn emptied(chunk: &str) -> String {
    chunk
        .lines()
        .filter(|line| !line.trim_start().starts_with("PT "))
        .map(|line| {
            let trimmed = line.trim_start();
            for (key, off) in [("ACT ", "ACT 0"), ("VIS ", "VIS 0"), ("ARM ", "ARM 0")] {
                if let Some(rest) = trimmed.strip_prefix(key) {
                    let tail: Vec<&str> = rest.split_whitespace().skip(1).collect();
                    return if tail.is_empty() { off.to_string() } else { format!("{off} {}", tail.join(" ")) };
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs a take-envelope action on one item, the producer's item selection kept as it was.
fn on_item_alone(ctx: &Ctx, take: Take, action: i32) -> Done<()> {
    let reaper = ctx.reaper;
    let item = reaper.take_item(take).ok_or_else(|| failed("REAPER doesn't say which item the take is on"))?;
    let (_, selected) = reaper.selected_items_up_to(ctx.proj, usize::MAX);
    let active_before = reaper.active_take(item);
    reaper.select_all_items(ctx.proj, false);
    reaper.set_item_selected(item, true);
    if reaper.item_value(item, "B_UISEL") == 0.0 {
        return Err(failed("REAPER can't select the item to make its envelope"));
    }
    // (the action works on the item's active take: the take asked for is made active for it, and put back)
    let take_index = (0..reaper.take_count(item)).find(|&i| reaper.take_at(item, i) == Some(take));
    let switched = active_before != Some(take);
    if switched {
        if let Some(i) = take_index {
            if let Some(take) = reaper.take_at(item, i) {
                reaper.set_active_take(take);
            }
        }
    }
    reaper.run_action(action);
    if switched {
        if let Some(before) = active_before {
            reaper.set_active_take(before);
        }
    }
    reaper.select_all_items(ctx.proj, false);
    for other in selected {
        reaper.set_item_selected(other, true);
    }
    Ok(())
}

pub fn show_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let shown = shown_of(ctx, input)?;
    let flags = wanted_flags(input)?;
    let label = shown_label(ctx, &shown);
    let (target, prior, fence, unchanged) = match &shown {
        Shown::Has(at) => {
            let chunk = ctx.reaper.env_chunk(at.envelope).ok_or_else(|| failed("REAPER wouldn't give the envelope's state"))?;
            if chunk.len() > 8 << 20 {
                return Err(failed("that envelope's state is over 8 MB: Kumi won't change what it can't put back"));
            }
            let state = env_state(ctx, at.envelope);
            let now = &state["flags"];
            let unchanged =
                [("visible", flags.0), ("active", flags.1), ("armed", flags.2)].iter().all(|(k, v)| v.is_none_or(|v| now[*k] == v));
            (
                json!({"ref": env_ref(ctx, at), "kind": "env"}),
                json!({"exists": true, "chunk": chunk, "state": state}),
                json!({"state": state}),
                unchanged,
            )
        }
        Shown::MakeFx(found) => (super::fx::param_target(ctx, found), json!({"exists": false}), json!({"exists": false}), false),
        Shown::MakeTake(take, name, _) => (
            json!({"ref": refs::take_ref(&ctx.reaper.take_guid(*take)), "kind": "take", "name": name}),
            json!({"exists": false}),
            json!({"exists": false}),
            false,
        ),
    };
    let mut display = json!({"title": show_title(&label, flags)});
    if unchanged {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target, prior, fence, display })
}

pub fn show_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let shown = shown_of(ctx, input).map_err(went)?;
    let flags = wanted_flags(input)?;
    let label = shown_label(ctx, &shown);
    let reaper = ctx.reaper;
    let (at, created) = match shown {
        Shown::Has(at) => {
            if fence["exists"] == false || !same_env(&env_state(ctx, at.envelope), &fence["state"]) {
                return Err(changed(format!("{label} changed since the preview")));
            }
            (at, None)
        }
        Shown::MakeFx(found) => {
            if fence["exists"] != false {
                return Err(changed(format!("{label} was made in REAPER since the preview")));
            }
            let envelope = reaper
                .fx_envelope(found.at.fx, found.param, true)
                .ok_or_else(|| failed("REAPER wouldn't make that parameter's envelope"))?;
            let guid = reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
            let owner = match found.at.fx.owner {
                crate::reaper::FxOwner::Track(track) => EnvOwner::Track(track),
                crate::reaper::FxOwner::Take(take) => EnvOwner::Take(take),
            };
            let at = EnvAt { envelope, guid, owner };
            let created = env_ref(ctx, &at);
            (at, Some(created))
        }
        Shown::MakeTake(take, name, action) => {
            if fence["exists"] != false {
                return Err(changed(format!("{label} was made in REAPER since the preview")));
            }
            on_item_alone(ctx, take, action)?;
            let envelope = reaper.take_envelope_by_name(take, name).ok_or_else(|| failed("REAPER wouldn't make the take's envelope"))?;
            let guid = reaper.envelope_guid(envelope).ok_or_else(|| failed("REAPER gives that envelope no id"))?;
            let at = EnvAt { envelope, guid, owner: EnvOwner::Take(take) };
            let created = env_ref(ctx, &at);
            (at, Some(created))
        }
    };
    let before = flags_of(reaper, at.envelope);
    let unwritten = created.is_none()
        && [("visible", flags.0), ("active", flags.1), ("armed", flags.2)].iter().all(|(k, v)| v.is_none_or(|v| before[*k] == v));
    if !unwritten {
        writing(ctx, &at, || set_flags(reaper, at.envelope, flags));
    }
    let after = flags_of(reaper, at.envelope);
    if [("visible", flags.0), ("active", flags.1), ("armed", flags.2)].iter().any(|(k, v)| v.is_some_and(|v| after[*k] != v)) {
        return Err(failed(format!("REAPER kept {label} as it was")));
    }
    let reference = env_ref(ctx, &at);
    Ok(Applied {
        applied: json!({"state": env_state(ctx, at.envelope), "ref": reference}),
        created: created.map(|r| json!({"ref": r, "kind": "env"})),
        display: json!({"title": show_title(&label, flags), "ref": reference}),
        unwritten,
        ..Default::default()
    })
}

pub fn show_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let reference = restore.applied["ref"].as_str().ok_or_else(|| bad("applied.ref: the envelope"))?;
    let reaper = ctx.reaper;
    // One Kumi made, or one REAPER handed out hidden and Kumi's undo took out of the list again: not found is how it was.
    let unlisted = |state: &Value| {
        state["flags"].as_object().is_some_and(|flags| flags.values().all(|v| *v == false))
            && state["items"] == 0
            && state["points"].as_str().is_some_and(|hash| hash.ends_with("-0"))
    };
    let at = match envelope_at(ctx, reference) {
        Ok(at) => at,
        Err(fault) if fault.0 == "failed" && (restore.prior["exists"] == false || unlisted(&restore.prior["state"])) => {
            return Ok(Restored { restored: json!({}), display: json!({"title": "The envelope is as it was"}), unwritten: true });
        }
        Err(fault) => return Err(gone_if_missing(fault)),
    };
    let label = env_label(ctx, &at);
    let now = env_state(ctx, at.envelope);
    if restore.prior["exists"] == true && same_env(&now, &restore.prior["state"]) {
        return Ok(Restored { restored: now, display: json!({"title": format!("{label} is as it was")}), unwritten: true });
    }
    // (discard too: the envelope's whole state from before goes back, which would take points the producer drew since,
    // and put their automation items back where they were)
    if !same_env(&now, &restore.applied["state"]) {
        return Err(changed(format!(
            "{label} has been changed in REAPER since Kumi showed it: putting it back as it was would write over those changes \
             too (its points and items), so Kumi leaves it, even with discard"
        )));
    }
    let mut display = json!({"title": format!("{label} put back as it was")});
    if restore.prior["exists"] == true {
        let chunk = restore.prior["chunk"].as_str().ok_or_else(|| bad("prior.chunk"))?;
        writing(ctx, &at, || {
            reaper.set_env_chunk(at.envelope, chunk);
            mark_envelope(reaper, at.envelope);
        });
        if !same_env(&env_state(ctx, at.envelope), &restore.prior["state"]) {
            return Err(("uncertain".into(), format!("REAPER didn't put {label} back exactly: read it")));
        }
    } else {
        let chunk = reaper.env_chunk(at.envelope).ok_or_else(|| failed("REAPER wouldn't give the envelope's state"))?;
        writing(ctx, &at, || {
            reaper.set_env_chunk(at.envelope, &emptied(&chunk));
            mark_envelope(reaper, at.envelope);
        });
        let flags = flags_of(reaper, at.envelope);
        if flags["visible"] == true || flags["active"] == true {
            return Err(failed(format!("REAPER kept {label}")));
        }
        // (REAPER takes an emptied take envelope out of the take's list, lab 7.82; said if it ever keeps one)
        if let EnvOwner::Take(take) = at.owner {
            let guid = &at.guid;
            if take_envelope(reaper, take, guid).is_some() {
                display["note"] = "REAPER keeps the take's envelope: it is hidden and off, with no points.".into();
            }
        }
    }
    Ok(Restored { restored: env_state(ctx, at.envelope), display, ..Default::default() })
}

// ----- automation items -----

const ITEM_KEYS: [&str; 8] = ["D_LENGTH", "D_STARTOFFS", "D_PLAYRATE", "D_BASELINE", "D_AMPLITUDE", "D_LOOPSRC", "D_MUTE", "D_POSITION"];

fn item_state(reaper: &Reaper, envelope: Envelope, index: i32) -> Value {
    json!({
        "pool": reaper.ai_value(envelope, index, "D_POOL_ID") as i64,
        "position": reaper.ai_value(envelope, index, "D_POSITION"),
        "length": reaper.ai_value(envelope, index, "D_LENGTH"),
        "startOffset": reaper.ai_value(envelope, index, "D_STARTOFFS"),
        "playrate": reaper.ai_value(envelope, index, "D_PLAYRATE"),
        "baseline": reaper.ai_value(envelope, index, "D_BASELINE"),
        "amplitude": reaper.ai_value(envelope, index, "D_AMPLITUDE"),
        "loop": reaper.ai_value(envelope, index, "D_LOOPSRC") != 0.0,
        "mute": reaper.ai_value(envelope, index, "D_MUTE") != 0.0,
    })
}

/// An automation item's state as its envelope's state keeps it (times to 9 places, baseline, amplitude and playrate to 6):
/// what a fence or an undo compares, so REAPER's own undo and redo in between don't read as a change.
fn item_print(state: &Value) -> Value {
    let time = |key: &str| kept(state[key].as_f64().unwrap_or(f64::NAN), 9);
    let level = |key: &str| kept(state[key].as_f64().unwrap_or(f64::NAN), 6);
    json!({
        "pool": state["pool"],
        "position": time("position"),
        "length": time("length"),
        "startOffset": time("startOffset"),
        "playrate": level("playrate"),
        "baseline": level("baseline"),
        "amplitude": level("amplitude"),
        "loop": state["loop"],
        "mute": state["mute"],
    })
}

fn same_item(a: &Value, b: &Value) -> bool {
    item_print(a) == item_print(b)
}

/// Every automation item of an envelope, as kept: what an edit of its items is fenced by.
fn items_print(ctx: &Ctx, envelope: Envelope) -> Value {
    (0..ctx.reaper.ai_count(envelope)).map(|i| item_print(&item_state(ctx.reaper, envelope, i))).collect::<Vec<_>>().into()
}

fn track_env(ctx: &Ctx, reference: &Value) -> Done<EnvAt> {
    let at = envelope_at(ctx, reference.as_str().ok_or_else(|| bad("ref: the track envelope"))?)?;
    if matches!(at.owner, EnvOwner::Take(_)) {
        return Err(bad("a take's envelopes have no automation items"));
    }
    if units_of(ctx, at.envelope)? == Units::Tempo {
        return Err(bad("REAPER puts no automation items on the tempo map"));
    }
    Ok(at)
}

fn length_of(ctx: &Ctx, input: &Value, position: f64) -> Done<Option<f64>> {
    let beats = input.get("lengthBeats").filter(|v| !v.is_null());
    let secs = input.get("length").filter(|v| !v.is_null());
    match (beats, secs) {
        (Some(_), Some(_)) => Err(bad("pass lengthBeats or length, not both")),
        (Some(b), None) => {
            let b = b.as_f64().filter(|b| *b > 0.0 && b.is_finite()).ok_or_else(|| bad("lengthBeats must be more than 0"))?;
            let start = ctx.reaper.quarter_notes_at(ctx.proj, position).ok_or_else(|| failed("this REAPER can't turn time into beats"))?;
            let end =
                ctx.reaper.time_at_quarter_notes(ctx.proj, start + b).ok_or_else(|| failed("this REAPER can't turn beats into time"))?;
            Ok(Some(end - position))
        }
        (None, Some(s)) => {
            s.as_f64().filter(|s| *s > 0.0 && s.is_finite()).map(Some).ok_or_else(|| bad("length must be more than 0 seconds"))
        }
        (None, None) => Ok(None),
    }
}

fn underlying(ctx: &Ctx, at: &EnvAt) -> Vec<EnvPoint> {
    let count = ctx.reaper.env_point_count(at.envelope, -1);
    (0..count).filter_map(|i| ctx.reaper.env_point(at.envelope, -1, i)).collect()
}

pub fn item_add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let at = track_env(ctx, &input["ref"])?;
    let position = marks::moment(ctx, input, "positionBeats", "position")?.ok_or_else(|| bad("pass positionBeats or position"))?;
    let length = length_of(ctx, input, position)?.ok_or_else(|| bad("pass lengthBeats or length"))?;
    let pool = pool_of(ctx, input)?;
    if pool.is_some() && input.get("points").is_some_and(|p| !p.is_null()) {
        return Err(bad("a pooled copy takes its pool's points: points are for a new pool"));
    }
    let points = underlying(ctx, &at);
    if points.len() > MAX_POINTS {
        return Err(failed(format!("that envelope has {} points: Kumi changes envelopes of at most {MAX_POINTS}", points.len())));
    }
    let prior = json!({"underlying": points_json(&points), "hash": points_hash(ctx.reaper, at.envelope, &points), "items": items_print(ctx, at.envelope)});
    let what = match pool {
        Some(pool) => format!("a pooled copy (pool {pool})"),
        None => "an automation item".into(),
    };
    let mut display = json!({"title": format!("Added {what} to {} from {} to {}", env_label(ctx, &at), bar_at(ctx, position), bar_at(ctx, position + length))});
    if pool.is_none() && points.iter().any(|p| p.time >= position && p.time < position + length) {
        display["note"] = "The envelope's points in its range go into the new item (REAPER's way); an undo puts them back.".into();
    }
    Ok(Preview {
        target: json!({"ref": env_ref(ctx, &at), "kind": "env"}),
        fence: json!({"hash": prior["hash"], "items": prior["items"]}),
        prior,
        display,
    })
}

/// The pool a pooled copy takes: the pool of the automation item `poolRef` names.
fn pool_of(ctx: &Ctx, input: &Value) -> Done<Option<i64>> {
    let Some(reference) = input.get("poolRef").filter(|v| !v.is_null()) else { return Ok(None) };
    let lane = lane_named(ctx, reference)?;
    lane.item.map(|item| Some(item.pool)).ok_or_else(|| bad("poolRef must be an automation item ref"))
}

pub fn item_add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let at = track_env(ctx, &input["ref"]).map_err(went)?;
    let position = marks::moment(ctx, input, "positionBeats", "position")?.ok_or_else(|| bad("pass positionBeats or position"))?;
    let length = length_of(ctx, input, position)?.ok_or_else(|| bad("pass lengthBeats or length"))?;
    let pool = pool_of(ctx, input)?;
    let reaper = ctx.reaper;
    if points_hash(reaper, at.envelope, &underlying(ctx, &at)) != fence["hash"].as_str().unwrap_or_default()
        || items_print(ctx, at.envelope) != fence["items"]
    {
        return Err(changed(format!("{} changed since the preview", env_label(ctx, &at))));
    }
    let index = reaper.ai_insert(at.envelope, pool.map_or(-1, |p| p as i32), position, length);
    if index < 0 || index >= reaper.ai_count(at.envelope) {
        return Err(failed("REAPER wouldn't add the automation item"));
    }
    let state = item_state(reaper, at.envelope, index);
    let (made_pool, made_position) = (state["pool"].as_i64().unwrap_or(-1), state["position"].as_f64().unwrap_or(position));
    let label = env_label(ctx, &at);
    if let Some(list) = input.get("points").filter(|p| !p.is_null()) {
        let lane = Lane { at: at.clone(), item: Some(ItemAt { index, pool: made_pool, position: made_position }) };
        let units = units_of(ctx, at.envelope)?;
        let plan = plan_of(ctx, Edit::Add, &lane, units, &[], &json!({"points": list}))?;
        if let Plan::Add(points) = plan {
            for point in &points {
                reaper.env_insert_point(at.envelope, index, *point, true);
            }
            reaper.env_sort(at.envelope, index);
        }
    }
    mark_envelope(reaper, at.envelope);
    let reference = item_ref(ctx, &at, made_pool, made_position)?;
    let points = underlying(ctx, &at);
    Ok(Applied {
        applied: json!({"pool": made_pool, "position": made_position, "hash": points_hash(reaper, at.envelope, &points), "items": items_print(ctx, at.envelope)}),
        created: Some(json!({"ref": reference, "kind": "aitem"})),
        display: json!({"title": format!("Added an automation item to {label} from {} to {}", bar_at(ctx, made_position), bar_at(ctx, made_position + length)), "ref": reference}),
        ..Default::default()
    })
}

/// Takes an automation item off its envelope: its line goes from the envelope's state (REAPER's own delete works on the
/// selected items of every envelope, so it would need the producer's selection changed everywhere).
fn delete_item(ctx: &Ctx, at: &EnvAt, item: ItemAt) -> Done<()> {
    let reaper = ctx.reaper;
    let chunk = reaper.env_chunk(at.envelope).ok_or_else(|| failed("REAPER wouldn't give the envelope's state"))?;
    let lines: Vec<&str> = chunk.lines().collect();
    let items: Vec<usize> =
        lines.iter().enumerate().filter(|(_, l)| l.trim_start().starts_with("POOLEDENVINST ")).map(|(i, _)| i).collect();
    if items.len() != reaper.ai_count(at.envelope) as usize {
        return Err(failed("REAPER's envelope state doesn't list its automation items as REAPER counts them"));
    }
    let line = lines[items[item.index as usize]];
    let fields: Vec<&str> = line.split_whitespace().collect();
    let (pool, position) = (fields.get(1).and_then(|f| f.parse::<i64>().ok()), fields.get(2).and_then(|f| f.parse::<f64>().ok()));
    if pool != Some(item.pool) || !position.is_some_and(|p| (p - item.position).abs() <= 1e-6 * item.position.abs().max(1.0)) {
        return Err(failed("REAPER's envelope state doesn't hold that automation item where Kumi looked"));
    }
    let others: Vec<Value> =
        (0..reaper.ai_count(at.envelope)).filter(|&i| i != item.index).map(|i| item_state(reaper, at.envelope, i)).collect();
    let kept: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| *i != items[item.index as usize]).map(|(_, l)| *l).collect();
    if !reaper.set_env_chunk(at.envelope, &kept.join("\n")) {
        return Err(failed("REAPER wouldn't take the automation item off"));
    }
    mark_envelope(reaper, at.envelope);
    if items_print(ctx, at.envelope) != Value::from(others.iter().map(item_print).collect::<Vec<_>>()) {
        return Err(("uncertain".into(), "the envelope's automation items aren't what they should be after the delete: read them".into()));
    }
    Ok(())
}

pub fn item_add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = envelope_at(ctx, restore.target["ref"].as_str().unwrap_or_default()).map_err(gone_if_missing)?;
    let label = env_label(ctx, &at);
    let (pool, position) = (restore.applied["pool"].as_i64().unwrap_or(-1), restore.applied["position"].as_f64().unwrap_or(f64::NAN));
    let item = match find_item(ctx, &at, pool, position) {
        Ok(item) => item,
        Err(fault) if fault.0 == "failed" => {
            // Not where Kumi put it. Gone only when no item of its pool is left beyond the ones that were there besides
            // it: one more is Kumi's, moved, which can't be told from the others now (discard or not).
            if let Some(then) = restore.applied["items"].as_array() {
                let then = then.iter().filter(|item| item["pool"].as_i64() == Some(pool)).count();
                let now = (0..ctx.reaper.ai_count(at.envelope))
                    .filter(|i| ctx.reaper.ai_value(at.envelope, *i, "D_POOL_ID") as i64 == pool)
                    .count();
                if now >= then.max(1) {
                    return Err(changed(format!(
                        "the automation item Kumi added to {label} has been moved in REAPER since: Kumi leaves it, as it can't tell it \
                         apart now"
                    )));
                }
            }
            return Ok(Restored {
                restored: json!({}),
                display: json!({"title": format!("The automation item is gone from {label} already")}),
                unwritten: true,
            });
        }
        Err(fault) => return Err(fault),
    };
    if !restore.discard
        && (items_print(ctx, at.envelope) != restore.applied["items"]
            || points_hash(ctx.reaper, at.envelope, &underlying(ctx, &at)) != restore.applied["hash"].as_str().unwrap_or_default())
    {
        return Err(changed(format!("{label} has been changed in REAPER since Kumi added the automation item")));
    }
    delete_item(ctx, &at, item)?;
    let prior = points_from(&restore.prior["underlying"])?;
    if points_hash(ctx.reaper, at.envelope, &underlying(ctx, &at)) != points_hash(ctx.reaper, at.envelope, &prior) {
        write_lane(ctx, &Lane { at: at.clone(), item: None }, &prior);
    }
    Ok(Restored {
        restored: json!({"deleted": {"pool": pool, "position": position}}),
        display: json!({"title": format!("Took the automation item off {label}")}),
        ..Default::default()
    })
}

/// The fields an automation item change takes: (input, REAPER's key).
const ITEM_FIELDS: [(&str, &str); 6] = [
    ("loop", "D_LOOPSRC"),
    ("baseline", "D_BASELINE"),
    ("amplitude", "D_AMPLITUDE"),
    ("playrate", "D_PLAYRATE"),
    ("startOffset", "D_STARTOFFS"),
    ("mute", "D_MUTE"),
];

fn item_set_parts(ctx: &Ctx, lane: &Lane, input: &Value) -> Done<Vec<(&'static str, f64)>> {
    let item = lane.item.ok_or_else(|| bad("ref must be an automation item ref"))?;
    let mut out = Vec::new();
    for (name, key) in ITEM_FIELDS {
        let Some(value) = input.get(name).filter(|v| !v.is_null()) else { continue };
        let number = match name {
            "loop" | "mute" => {
                value.as_bool().map(|b| if b { 1.0 } else { 0.0 }).ok_or_else(|| bad(format!("{name} must be true or false")))?
            }
            "baseline" => value.as_f64().filter(|v| (0.0..=1.0).contains(v)).ok_or_else(|| bad("baseline must be from 0 to 1"))?,
            "amplitude" => value.as_f64().filter(|v| (-1.0..=1.0).contains(v)).ok_or_else(|| bad("amplitude must be from -1 to 1"))?,
            "playrate" => value.as_f64().filter(|v| (0.01..=100.0).contains(v)).ok_or_else(|| bad("playrate must be from 0.01 to 100"))?,
            _ => value.as_f64().filter(|v| v.is_finite() && v.abs() <= 1.0e6).ok_or_else(|| bad("startOffset must be seconds"))?,
        };
        out.push((key, number));
    }
    let position = marks::moment(ctx, input, "positionBeats", "position")?;
    if let Some(position) = position {
        out.push(("D_POSITION", position));
    }
    // (a length in beats counts from where the item will start)
    if let Some(length) = length_of(ctx, input, position.unwrap_or(item.position))? {
        out.push(("D_LENGTH", length));
    }
    if out.is_empty() {
        return Err(bad(
            "pass at least one of loop, baseline, amplitude, playrate, startOffset, mute, positionBeats/position, lengthBeats/length",
        ));
    }
    Ok(out)
}

fn item_title(ctx: &Ctx, at: &EnvAt, position: f64, writes: &[(&str, f64)]) -> String {
    let parts: Vec<String> = writes
        .iter()
        .map(|(key, v)| match *key {
            "D_LOOPSRC" => format!("loop {}", if *v != 0.0 { "on" } else { "off" }),
            "D_MUTE" => {
                if *v != 0.0 {
                    "muted".into()
                } else {
                    "unmuted".into()
                }
            }
            "D_BASELINE" => format!("baseline {v:.2}"),
            "D_AMPLITUDE" => format!("amplitude {v:.2}"),
            "D_PLAYRATE" => format!("rate {v}"),
            "D_STARTOFFS" => format!("start offset {}", seconds(*v)),
            "D_POSITION" => format!("moved to {}", bar_at(ctx, *v)),
            _ => format!("length {}", seconds(*v)),
        })
        .collect();
    format!("The automation item at {} on {}: {}", bar_at(ctx, position), env_label(ctx, at), parts.join(", "))
}

pub fn item_set_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let lane = lane_named(ctx, &input["ref"])?;
    let item = lane.item.ok_or_else(|| bad("ref must be an automation item ref"))?;
    let writes = item_set_parts(ctx, &lane, input)?;
    let now = item_state(ctx.reaper, lane.at.envelope, item.index);
    let target = json!({"ref": item_ref(ctx, &lane.at, item.pool, item.position)?, "kind": "aitem", "envelopeRef": env_ref(ctx, &lane.at)});
    let display = json!({"title": item_title(ctx, &lane.at, item.position, &writes)});
    Ok(Preview { target, fence: item_print(&now), prior: now, display })
}

fn write_item(reaper: &Reaper, envelope: Envelope, index: i32, state: &Value, keys: &[&str]) {
    for key in keys {
        let value = match *key {
            "D_LOOPSRC" => {
                if state["loop"] == true {
                    1.0
                } else {
                    0.0
                }
            }
            "D_MUTE" => {
                if state["mute"] == true {
                    1.0
                } else {
                    0.0
                }
            }
            "D_LENGTH" => state["length"].as_f64().unwrap_or(0.0),
            "D_STARTOFFS" => state["startOffset"].as_f64().unwrap_or(0.0),
            "D_PLAYRATE" => state["playrate"].as_f64().unwrap_or(1.0),
            "D_BASELINE" => state["baseline"].as_f64().unwrap_or(0.5),
            "D_AMPLITUDE" => state["amplitude"].as_f64().unwrap_or(1.0),
            _ => state["position"].as_f64().unwrap_or(0.0),
        };
        reaper.set_ai_value(envelope, index, key, value);
    }
}

pub fn item_set_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let lane = lane_named(ctx, &input["ref"]).map_err(went)?;
    let item = lane.item.ok_or_else(|| bad("ref must be an automation item ref"))?;
    let writes = item_set_parts(ctx, &lane, input)?;
    let reaper = ctx.reaper;
    let now = item_state(reaper, lane.at.envelope, item.index);
    if item_print(&now) != *fence {
        return Err(changed("the automation item changed since the preview"));
    }
    for (key, value) in &writes {
        reaper.set_ai_value(lane.at.envelope, item.index, key, *value);
    }
    mark_envelope(reaper, lane.at.envelope);
    let after = item_state(reaper, lane.at.envelope, item.index);
    let landed = writes.iter().all(|(key, value)| {
        let read = match *key {
            "D_LOOPSRC" => {
                if after["loop"] == true {
                    1.0
                } else {
                    0.0
                }
            }
            "D_MUTE" => {
                if after["mute"] == true {
                    1.0
                } else {
                    0.0
                }
            }
            "D_LENGTH" => after["length"].as_f64().unwrap_or(f64::NAN),
            "D_STARTOFFS" => after["startOffset"].as_f64().unwrap_or(f64::NAN),
            "D_PLAYRATE" => after["playrate"].as_f64().unwrap_or(f64::NAN),
            "D_BASELINE" => after["baseline"].as_f64().unwrap_or(f64::NAN),
            "D_AMPLITUDE" => after["amplitude"].as_f64().unwrap_or(f64::NAN),
            _ => after["position"].as_f64().unwrap_or(f64::NAN),
        };
        (read - value).abs() <= 1e-9 * value.abs().max(1.0)
    });
    if !landed {
        write_item(reaper, lane.at.envelope, item.index, &now, &ITEM_KEYS);
        return Err(failed("REAPER didn't take the automation item's change; it is put back"));
    }
    let position = after["position"].as_f64().unwrap_or(item.position);
    Ok(Applied {
        applied: after.clone(),
        display: json!({"title": item_title(ctx, &lane.at, item.position, &writes), "ref": item_ref(ctx, &lane.at, item.pool, position)?}),
        ..Default::default()
    })
}

pub fn item_set_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = envelope_at(ctx, restore.target["envelopeRef"].as_str().unwrap_or_default()).map_err(gone_if_missing)?;
    let (pool, position) = (restore.applied["pool"].as_i64().unwrap_or(-1), restore.applied["position"].as_f64().unwrap_or(f64::NAN));
    let item = match find_item(ctx, &at, pool, position) {
        Ok(item) => item,
        Err(fault) if fault.0 == "failed" => {
            // Back at its old place already (a retry)?
            let prior_position = restore.prior["position"].as_f64().unwrap_or(f64::NAN);
            if let Ok(back) = find_item(ctx, &at, pool, prior_position) {
                if same_item(&item_state(ctx.reaper, at.envelope, back.index), restore.prior) {
                    return Ok(Restored {
                        restored: restore.prior.clone(),
                        display: json!({"title": "The automation item is as it was"}),
                        unwritten: true,
                    });
                }
            }
            return Err(gone("that automation item is gone (moved or deleted in REAPER)"));
        }
        Err(fault) => return Err(fault),
    };
    let now = item_state(ctx.reaper, at.envelope, item.index);
    if same_item(&now, restore.prior) {
        return Ok(Restored { restored: now, display: json!({"title": "The automation item is as it was"}), unwritten: true });
    }
    if !restore.discard && !same_item(&now, restore.applied) {
        return Err(changed("the automation item has been changed in REAPER since Kumi changed it"));
    }
    write_item(ctx.reaper, at.envelope, item.index, restore.prior, &ITEM_KEYS);
    mark_envelope(ctx.reaper, at.envelope);
    if !same_item(&item_state(ctx.reaper, at.envelope, item.index), restore.prior) {
        return Err(failed("REAPER wouldn't put the automation item back"));
    }
    Ok(Restored {
        restored: restore.prior.clone(),
        display: json!({"title": format!("The automation item on {} put back", env_label(ctx, &at))}),
        ..Default::default()
    })
}

pub fn item_delete_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let lane = lane_named(ctx, &input["ref"])?;
    let item = lane.item.ok_or_else(|| bad("ref must be an automation item ref"))?;
    let state = item_state(ctx.reaper, lane.at.envelope, item.index);
    let target = json!({"ref": item_ref(ctx, &lane.at, item.pool, item.position)?, "kind": "aitem", "envelopeRef": env_ref(ctx, &lane.at)});
    Ok(Preview {
        target,
        prior: state.clone(),
        fence: json!({"items": items_print(ctx, lane.at.envelope)}),
        display: json!({"title": format!("Deleted the automation item at {} from {}", bar_at(ctx, item.position), env_label(ctx, &lane.at))}),
    })
}

pub fn item_delete_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let lane = lane_named(ctx, &input["ref"]).map_err(went)?;
    let item = lane.item.ok_or_else(|| bad("ref must be an automation item ref"))?;
    if items_print(ctx, lane.at.envelope) != fence["items"] {
        return Err(changed("the envelope's automation items changed since the preview"));
    }
    let state = item_state(ctx.reaper, lane.at.envelope, item.index);
    // One loop of its pool's points: what makes it again when no other copy of the pool is left.
    let points = lane_points(ctx, &lane);
    let name = ctx.reaper.ai_text(lane.at.envelope, item.index, "P_POOL_NAME").unwrap_or_default();
    let label = env_label(ctx, &lane.at);
    delete_item(ctx, &lane.at, item)?;
    Ok(Applied {
        applied: json!({"deleted": {"pool": item.pool, "position": item.position}}),
        material: Some(json!({"state": state, "points": points_json(&points), "poolName": name})),
        display: json!({"title": format!("Deleted the automation item at {} from {label}", bar_at(ctx, item.position))}),
        ..Default::default()
    })
}

pub fn item_delete_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let at = envelope_at(ctx, restore.target["envelopeRef"].as_str().unwrap_or_default()).map_err(gone_if_missing)?;
    let material = restore.material.ok_or_else(|| bad("material: the automation item"))?;
    let state = &material["state"];
    let (pool, position) = (state["pool"].as_i64().unwrap_or(-1), state["position"].as_f64().unwrap_or(f64::NAN));
    let label = env_label(ctx, &at);
    if find_item(ctx, &at, pool, position).is_ok() {
        return Ok(Restored {
            restored: state.clone(),
            display: json!({"title": format!("The automation item is back on {label} already")}),
            unwritten: true,
        });
    }
    let reaper = ctx.reaper;
    let length = state["length"].as_f64().unwrap_or(1.0);
    let index = reaper.ai_insert(at.envelope, pool as i32, position, length);
    if index < 0 {
        return Err(failed("REAPER wouldn't add the automation item back"));
    }
    let lane = Lane { at: at.clone(), item: Some(ItemAt { index, pool, position }) };
    let points = points_from(&material["points"])?;
    let mut display = json!({"title": format!("The automation item at {} put back on {label}", bar_at(ctx, position))});
    if lane_points(ctx, &lane).is_empty() && !points.is_empty() {
        // No other copy of its pool was left, so REAPER emptied the pool: its points go back in.
        for point in &points {
            reaper.env_insert_point(at.envelope, index, *point, true);
        }
        reaper.env_sort(at.envelope, index);
        display["note"] = "No other copy of its pool was left: its points were put back into the pool.".into();
    }
    write_item(reaper, at.envelope, index, state, &ITEM_KEYS[..7]);
    mark_envelope(reaper, at.envelope);
    let back = find_item(ctx, &at, pool, position).map(|item| item_state(reaper, at.envelope, item.index));
    if !back.is_ok_and(|back| same_item(&back, state)) {
        return Err(failed("REAPER didn't put the automation item back as it was"));
    }
    Ok(Restored { restored: state.clone(), display, ..Default::default() })
}

// ----- automation modes -----

const MODES: [(&str, i64); 6] = [("trim", 0), ("read", 1), ("touch", 2), ("write", 3), ("latch", 4), ("latch-preview", 5)];
const OVERRIDES: [(&str, i64); 8] =
    [("none", -1), ("trim", 0), ("read", 1), ("touch", 2), ("write", 3), ("latch", 4), ("latch-preview", 5), ("bypass", 6)];

fn mode_input(input: &Value, list: &[(&str, i64)]) -> Done<i64> {
    let text = input.get("mode").and_then(Value::as_str).ok_or_else(|| bad("mode: the automation mode's name"))?;
    list.iter()
        .find(|(word, _)| *word == text)
        .map(|(_, code)| *code)
        .ok_or_else(|| bad(format!("mode must be one of {}", list.iter().map(|m| m.0).collect::<Vec<_>>().join(", "))))
}

fn mode_word(list: &[(&str, i64)], code: i64) -> String {
    list.iter().find(|(_, c)| *c == code).map_or_else(|| format!("{code}"), |(word, _)| word.replace('-', " "))
}

fn mode_track(ctx: &Ctx, reference: &Value) -> Done<(Track, String)> {
    let id = super::track_ref_of(&json!({"ref": reference}), "track-automation-mode")?;
    let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id)?;
    Ok((track, super::track_name(ctx.reaper, track)))
}

pub fn mode_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let (track, name) = mode_track(ctx, &input["ref"])?;
    let wanted = mode_input(input, &MODES)?;
    let now = json!({"mode": ctx.reaper.track_value(track, "I_AUTOMODE") as i64});
    let mut display = json!({"title": format!("{name} automation {}", mode_word(&MODES, wanted))});
    if now["mode"] == wanted {
        display["unchanged"] = true.into();
    }
    if (2..=5).contains(&wanted) {
        display["note"] = "In this mode REAPER records the track's moves into its armed envelopes while it plays.".into();
    }
    Ok(Preview { target: json!({"ref": input["ref"], "kind": "track"}), prior: now.clone(), fence: now, display })
}

pub fn mode_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (track, name) = mode_track(ctx, &input["ref"]).map_err(went)?;
    let wanted = mode_input(input, &MODES)?;
    let now = ctx.reaper.track_value(track, "I_AUTOMODE") as i64;
    if json!({"mode": now}) != *fence {
        return Err(changed(format!("{name}'s automation mode changed since the preview")));
    }
    if now != wanted
        && (!ctx.reaper.set_track_value(track, "I_AUTOMODE", wanted as f64) || ctx.reaper.track_value(track, "I_AUTOMODE") as i64 != wanted)
    {
        return Err(failed(format!("REAPER kept {name}'s automation mode")));
    }
    Ok(Applied {
        applied: json!({"mode": wanted}),
        display: json!({"title": format!("{name} automation {}", mode_word(&MODES, wanted))}),
        unwritten: now == wanted,
        ..Default::default()
    })
}

pub fn mode_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (track, name) = mode_track(ctx, &restore.target["ref"]).map_err(gone_if_missing)?;
    let (prior, applied) = (restore.prior["mode"].as_i64().unwrap_or(0), restore.applied["mode"].as_i64().unwrap_or(0));
    let now = ctx.reaper.track_value(track, "I_AUTOMODE") as i64;
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed(format!("{name}'s automation mode has been changed in REAPER since Kumi set it")));
        }
        if !ctx.reaper.set_track_value(track, "I_AUTOMODE", prior as f64) || ctx.reaper.track_value(track, "I_AUTOMODE") as i64 != prior {
            return Err(failed(format!("REAPER kept {name}'s automation mode")));
        }
    }
    Ok(Restored {
        restored: json!({"mode": prior}),
        display: json!({"title": format!("{name} automation {}", mode_word(&MODES, prior))}),
        unwritten: now == prior,
    })
}

fn override_title(code: i64) -> String {
    match code {
        -1 => "No global automation override".into(),
        6 => "Global automation override: bypass all automation".into(),
        other => format!("Global automation override: all automation in {} mode", mode_word(&OVERRIDES, other)),
    }
}

pub fn override_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = mode_input(input, &OVERRIDES)?;
    let now = json!({"mode": ctx.reaper.automation_override().ok_or_else(|| failed("this REAPER has no global automation override"))?});
    let mut display = json!({"title": override_title(wanted), "note": "The override is REAPER's own setting, not a step of its undo: only Kumi's undo takes it back."});
    if now["mode"] == wanted {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn override_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = mode_input(input, &OVERRIDES)?;
    let now = ctx.reaper.automation_override().ok_or_else(|| failed("this REAPER has no global automation override"))? as i64;
    if json!({"mode": now}) != *fence {
        return Err(changed("the global automation override changed since the preview"));
    }
    if now != wanted && (!ctx.reaper.set_automation_override(wanted as i32) || ctx.reaper.automation_override() != Some(wanted as i32)) {
        return Err(failed("REAPER kept the global automation override"));
    }
    Ok(Applied {
        applied: json!({"mode": wanted}),
        display: json!({"title": override_title(wanted)}),
        unwritten: now == wanted,
        ..Default::default()
    })
}

pub fn override_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (prior, applied) = (restore.prior["mode"].as_i64().unwrap_or(-1), restore.applied["mode"].as_i64().unwrap_or(-1));
    let now = ctx.reaper.automation_override().ok_or_else(|| failed("this REAPER has no global automation override"))? as i64;
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed("the global automation override has been changed in REAPER since Kumi set it"));
        }
        if !ctx.reaper.set_automation_override(prior as i32) || ctx.reaper.automation_override() != Some(prior as i32) {
            return Err(failed("REAPER kept the global automation override"));
        }
    }
    Ok(Restored { restored: json!({"mode": prior}), display: json!({"title": override_title(prior)}), unwritten: now == prior })
}

// ----- a send's automation mode -----

/// A send's automation modes: its own, or its track's (-1).
const SEND_MODES: [(&str, i64); 7] =
    [("track", -1), ("trim", 0), ("read", 1), ("touch", 2), ("write", 3), ("latch", 4), ("latch-preview", 5)];

/// The send a ref names (its track and place among the track's sends), and how the titles say it: `“Bass” → “Reverb”`.
fn mode_send(ctx: &Ctx, reference: &Value) -> Done<(Track, i32, String)> {
    let (track, index) = send_index(ctx, reference.as_str().ok_or_else(|| bad("ref: the send"))?, "ref")?;
    let to = ctx.reaper.send_peer(track, 0, index).map_or_else(String::new, |peer| super::track_name(ctx.reaper, peer));
    Ok((track, index, format!("{} → {}", super::quoted(&super::track_name(ctx.reaper, track)), super::quoted(&to))))
}

fn send_mode_title(route: &str, code: i64) -> String {
    match code {
        -1 => format!("Send {route} automation as its track's"),
        _ => format!("Send {route} automation {}", mode_word(&SEND_MODES, code)),
    }
}

pub fn send_mode_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = mode_input(input, &SEND_MODES)?;
    let (track, index, route) = mode_send(ctx, &input["ref"])?;
    let now = json!({"mode": ctx.reaper.send_value(track, 0, index, "I_AUTOMODE") as i64});
    let mut display = json!({"title": send_mode_title(&route, wanted)});
    if now["mode"] == wanted {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": input["ref"], "kind": "send"}), prior: now.clone(), fence: now, display })
}

pub fn send_mode_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = mode_input(input, &SEND_MODES)?;
    let (track, index, route) = mode_send(ctx, &input["ref"]).map_err(went)?;
    let now = ctx.reaper.send_value(track, 0, index, "I_AUTOMODE") as i64;
    if json!({"mode": now}) != *fence {
        return Err(changed(format!("the automation mode of send {route} changed since the preview")));
    }
    if now != wanted
        && (!super::sends::noting_writes(ctx.reaper, track, index, || {
            ctx.reaper.set_send_value(track, 0, index, "I_AUTOMODE", wanted as f64)
        }) || ctx.reaper.send_value(track, 0, index, "I_AUTOMODE") as i64 != wanted)
    {
        return Err(failed(format!("REAPER kept the automation mode of send {route}")));
    }
    Ok(Applied {
        applied: json!({"mode": wanted}),
        display: json!({"title": send_mode_title(&route, wanted)}),
        unwritten: now == wanted,
        ..Default::default()
    })
}

pub fn send_mode_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (track, index, route) = mode_send(ctx, &restore.target["ref"]).map_err(gone_if_missing)?;
    let (prior, applied) = (restore.prior["mode"].as_i64().unwrap_or(-1), restore.applied["mode"].as_i64().unwrap_or(-1));
    let now = ctx.reaper.send_value(track, 0, index, "I_AUTOMODE") as i64;
    if now != prior {
        if !restore.discard && now != applied {
            return Err(changed(format!("the automation mode of send {route} has been changed in REAPER since Kumi set it")));
        }
        if !super::sends::noting_writes(ctx.reaper, track, index, || ctx.reaper.set_send_value(track, 0, index, "I_AUTOMODE", prior as f64))
            || ctx.reaper.send_value(track, 0, index, "I_AUTOMODE") as i64 != prior
        {
            return Err(failed(format!("REAPER kept the automation mode of send {route}")));
        }
    }
    Ok(Restored {
        restored: json!({"mode": prior}),
        display: json!({"title": format!("{} (put back)", send_mode_title(&route, prior))}),
        unwritten: now == prior,
    })
}

/// A lane's points as a read pages them (the read of `point` kind): only the page's points are read, so a page of a big
/// lane costs what a page of a small one does.
pub(crate) struct PointSource {
    lane: Lane,
    units: Units,
    count: usize,
}

impl PointSource {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn units(&self) -> &'static str {
        units_word(self.units)
    }

    pub fn row(&self, ctx: &Ctx, index: usize) -> Value {
        let Some(point) = ctx.reaper.env_point(self.lane.at.envelope, self.lane.read(), index as i32) else {
            return json!({"index": index, "gone": true});
        };
        let mut row = point_row(ctx, &self.lane, self.units, &point);
        row["index"] = index.into();
        if let Some(qn) = ctx.reaper.quarter_notes_at(ctx.proj, row["position"].as_f64().unwrap_or(0.0)) {
            row["positionBeats"] = qn.into();
        }
        row
    }
}

pub(crate) fn point_source(ctx: &Ctx, reference: &str) -> Done<PointSource> {
    let lane = lane_named(ctx, &json!(reference))?;
    let units = units_of(ctx, lane.at.envelope)?;
    let count = ctx.reaper.env_point_count(lane.at.envelope, lane.read()).max(0) as usize;
    Ok(PointSource { lane, units, count })
}

/// The rows a read gives of an envelope's automation items.
pub(crate) fn item_rows(ctx: &Ctx, reference: &str) -> Done<Vec<Value>> {
    let at = envelope_at(ctx, reference)?;
    let reaper = ctx.reaper;
    (0..reaper.ai_count(at.envelope))
        .map(|i| {
            let mut row = item_state(reaper, at.envelope, i);
            let (pool, position) = (row["pool"].as_i64().unwrap_or(-1), row["position"].as_f64().unwrap_or(0.0));
            row["ref"] = item_ref(ctx, &at, pool, position)?.into();
            row["index"] = i.into();
            row["positionBeats"] = reaper.quarter_notes_at(ctx.proj, position).into();
            row["poolName"] = reaper.ai_text(at.envelope, i, "P_POOL_NAME").unwrap_or_default().into();
            row["pointCount"] = reaper.env_point_count(at.envelope, i | ONE_LOOP).into();
            Ok(row)
        })
        .collect()
}

/// What an envelope's row adds (M4): its units, and how many automation items it has.
pub(crate) fn envelope_extras(ctx: &Ctx, envelope: Envelope) -> Value {
    json!({"units": units_of(ctx, envelope).map(units_word).unwrap_or("raw"), "automationItems": ctx.reaper.ai_count(envelope)})
}
