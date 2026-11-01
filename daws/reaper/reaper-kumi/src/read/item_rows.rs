//! What the reads say of items, takes and a track's razor areas for the item changes (M4): an item's own gain, lock,
//! loop, group, fades, timebase and snap offset (each only when it isn't REAPER's default, so a page of items stays
//! small); a take's properties and its markers' counts; and, read alone, a take's stretch and take markers with the
//! indexes the marker changes take.

use serde_json::{json, Value};

use crate::reaper::{Item, Project, Reaper, Take, Track};
use crate::refs::{self, EnvelopeId};
use crate::rows;

/// The most stretch or take markers a take's own read lists (the rest are counted).
const MARKERS_LISTED: usize = 256;
/// How far (ticks) a MIDI take's ticks may stray from a straight line across its item and still keep to the project's beats.
const BENT: f64 = 1e-3;

const CHANNEL_MODES: [&str; 5] = ["normal", "reverse", "mono", "left", "right"];

fn round(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn fade(reaper: &Reaper, item: Item, side: &str) -> Option<Value> {
    let length = reaper.item_value(item, &format!("D_FADE{side}LEN"));
    (length > 0.0).then(|| {
        json!({
            "length": round(length),
            "shape": reaper.item_value(item, &format!("C_FADE{side}SHAPE")) as i64,
            "curve": round(reaper.item_value(item, &format!("D_FADE{side}DIR_NEW"))),
            "s": round(reaper.item_value(item, &format!("D_FADE{side}DIR2_NEW"))),
        })
    })
}

/// An item's own properties, beside what its row says already.
pub fn item_details(reaper: &Reaper, item: Item, row: &mut Value) {
    row["locked"] = (reaper.item_value(item, "C_LOCK") as i64 & 1 != 0).into();
    row["loop"] = (reaper.item_value(item, "B_LOOPSRC") != 0.0).into();
    let volume = reaper.item_value(item, "D_VOL");
    if (volume - 1.0).abs() > 1e-9 {
        row["volumeDb"] = round(rows::db(volume)).into();
    }
    let group = reaper.item_value(item, "I_GROUPID") as i64;
    if group != 0 {
        row["group"] = group.into();
    }
    if let Some(fade) = fade(reaper, item, "IN") {
        row["fadeIn"] = fade;
    }
    if let Some(fade) = fade(reaper, item, "OUT") {
        row["fadeOut"] = fade;
    }
    let timebase = match reaper.item_value(item, "C_BEATATTACHMODE") as i64 {
        0 => Some("time"),
        1 => Some("beats"),
        2 => Some("beats-position"),
        _ => None,
    };
    if let Some(timebase) = timebase {
        row["timebase"] = timebase.into();
    }
    let snap = reaper.item_value(item, "D_SNAPOFFSET");
    if snap > 0.0 {
        row["snapOffset"] = round(snap).into();
    }
}

/// A take's properties and how many markers it has; `alone` (a read of this one take) lists the markers too.
pub fn take_details(reaper: &Reaper, take: Take, row: &mut Value, alone: bool) {
    let volume = reaper.take_value(take, "D_VOL");
    row["volumeDb"] = round(rows::db(volume.abs())).into();
    if volume < 0.0 {
        row["polarity"] = "inverted".into();
    }
    row["pan"] = round(reaper.take_value(take, "D_PAN")).into();
    row["pitch"] = round(reaper.take_value(take, "D_PITCH")).into();
    row["playrate"] = round(reaper.take_value(take, "D_PLAYRATE")).into();
    row["preservePitch"] = (reaper.take_value(take, "B_PPITCH") != 0.0).into();
    let mode = reaper.take_value(take, "I_CHANMODE") as usize;
    row["channelMode"] = CHANNEL_MODES.get(mode).copied().unwrap_or("normal").into();
    row["startOffset"] = round(reaper.take_value(take, "D_STARTOFFS")).into();
    let (stretch, markers) = (reaper.stretch_markers(take), reaper.take_markers(take));
    row["stretchMarkerCount"] = stretch.len().into();
    row["takeMarkerCount"] = markers.len().into();
    // Its file's name, not the folders it is in (as the project row has it).
    if let Some(file) = reaper.take_file(take) {
        let name = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
        row["file"] = rows::clip(&name).into();
    }
    if !alone {
        return;
    }
    // From 0, as the marker changes count them.
    row["stretchMarkers"] = stretch
        .iter()
        .take(MARKERS_LISTED)
        .enumerate()
        .map(
            |(index, m)| json!({"index": index, "position": round(m.position), "sourcePosition": round(m.source), "slope": round(m.slope)}),
        )
        .collect::<Vec<_>>()
        .into();
    row["takeMarkers"] = markers
        .iter()
        .take(MARKERS_LISTED)
        .enumerate()
        .map(|(index, m)| {
            let color = (m.color & 0x100_0000 != 0).then(|| reaper.color_hex(m.color)).flatten();
            json!({"index": index, "sourcePosition": round(m.source), "name": rows::clip(&m.name), "color": color})
        })
        .collect::<Vec<_>>()
        .into();
}

/// A MIDI take's ticks (#606), so a client knows the tick REAPER puts a note it writes on: `ticksPerQuarter`, how many a
/// quarter note of the project at playrate 1 (its MIDI's own, unless it ignores the project tempo), and `tickAtStart`, the one
/// at its item's start (a start offset moves it). Both null when its ticks don't keep to the project's beats across the item:
/// one that ignores the project tempo follows time, which a tempo change inside it bends. Nothing from a REAPER that can't say.
pub fn midi_ticks(reaper: &Reaper, proj: Project, take: Take, row: &mut Value) {
    let Some(item) = reaper.take_item(take) else { return };
    let position = reaper.item_value(item, "D_POSITION");
    let at = |time: f64| reaper.quarter_notes_at(proj, time);
    let (Some(start), Some(end)) = (at(position), at(position + reaper.item_value(item, "D_LENGTH"))) else { return };
    if reaper.quarter_notes_to_ppq(take, start).is_none() {
        return;
    }
    let playrate = reaper.take_value(take, "D_PLAYRATE");
    match straight(|qn| reaper.quarter_notes_to_ppq(take, qn).unwrap_or(f64::NAN), start, end) {
        Some((per_quarter, at_start)) if playrate > 0.0 => {
            row["ticksPerQuarter"] = round(per_quarter / playrate).into();
            row["tickAtStart"] = round(at_start).into();
        }
        _ => {
            row["ticksPerQuarter"] = Value::Null;
            row["tickAtStart"] = Value::Null;
        }
    }
}

/// How many ticks a quarter note `tick` (a position in quarter notes from the project start, to ticks) runs from `start` to
/// `end` and the tick at `start`, when it runs straight there: within `BENT` of that line at seven points inside (uneven ones,
/// so tempo markers on bar lines can't all fall on them). None when it bends.
pub(crate) fn straight(tick: impl Fn(f64) -> f64, start: f64, end: f64) -> Option<(f64, f64)> {
    let span = (end - start).max(1.0);
    let first = tick(start);
    let per_quarter = (tick(start + span) - first) / span;
    let bends = [0.13, 0.29, 0.41, 0.53, 0.67, 0.79, 0.91].iter().any(|share| {
        let qn = start + span * share;
        let off = (tick(qn) - (first + (qn - start) * per_quarter)).abs();
        off.is_nan() || off > BENT
    });
    (!bends && per_quarter.is_finite() && per_quarter > 0.0).then_some((per_quarter, first))
}

/// A track's razor areas, when it has any: start and end in seconds, the envelope's ref (null for the media lane) and
/// the share of the track's height they cover (a fixed lane's).
pub fn razor_areas(reaper: &Reaper, track: Track, row: &mut Value) {
    let Some(text) = reaper.track_string(track, "P_RAZOREDITS_EXT").filter(|t| !t.trim().is_empty()) else { return };
    let id = rows::track_id(reaper, track);
    let areas: Vec<Value> = text
        .split(',')
        .filter_map(|area| {
            let parts: Vec<&str> = area.split_whitespace().collect();
            let number = |at: usize| parts.get(at).and_then(|p| p.parse::<f64>().ok());
            let (start, end) = (number(0)?, number(1)?);
            let envelope = parts.get(2).map(|e| e.trim_matches('"').to_string()).unwrap_or_default();
            // The media lane, or an envelope by its ref.
            let envelope_ref = (!envelope.is_empty()).then(|| refs::envelope_ref(&id, &EnvelopeId::Guid(envelope)));
            Some(json!({"start": round(start), "end": round(end), "envelopeRef": envelope_ref, "top": number(3).map(round), "bottom": number(4).map(round)}))
        })
        .collect();
    if !areas.is_empty() {
        row["razorAreas"] = areas.into();
    }
}
