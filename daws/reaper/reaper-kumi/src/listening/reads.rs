//! The reads listening adds: what plays where (`read {kind: "spans"}`), and a parameter's text across its range
//! (`read {kind: "param", grid}`). The paging is `read`'s own (read.rs); these build the rows.

use serde_json::{json, Value};

use crate::reaper::{Fx, FxOwner, Project, Reaper, Track};
use crate::refs;
use crate::rows;

/// The tracks a page of spans holds by default.
pub const DEFAULT_LIMIT: usize = 64;
/// The most stretches one track's row lists: a track with more has its shortest gaps closed until it fits.
const MAX_SPANS: usize = 2000;
/// Points across a parameter's range, at most; stepped parameters list at most this many values.
pub const MAX_GRID: u64 = 64;
const MAX_ITEMS: usize = 128;

/// What REAPER's `fx_type` starts with for a plug-in format (an instrument's adds "i": "VSTi", "AUi"; measured in 7.82,
/// where ReaPlugs say "VST", JSFX "JS" and a container "Container").
const PLUGIN_TYPES: [&str; 5] = ["VST", "AU", "CLAP", "LV2", "DX"];

fn round(value: f64) -> f64 {
    rows::seconds(value)
}

/// What listening needs on an FX row (contract §9): the track whose chain it is on (`trackRef`; a take's FX also says
/// its `takeRef`, an input FX already says `input`), whether it is a plug-in from outside REAPER (`plugin`: a VST, VST3,
/// AU, CLAP, LV2 or DX plug-in not made by Cockos; ReaEQ and the other ReaPlugs, JSFX and containers are REAPER's own),
/// and the plug-in's own name when the producer renamed the FX (`originalName`, REAPER's `fx_name`).
pub fn add_fx_place(reaper: &Reaper, fx: Fx, row: &mut Value) {
    let track = match fx.owner {
        FxOwner::Track(track) => Some(track),
        FxOwner::Take(take) => {
            row["takeRef"] = refs::take_ref(&reaper.take_guid(take)).into();
            reaper.take_item(take).and_then(|item| reaper.item_track(item))
        }
    };
    if let Some(track) = track {
        row["trackRef"] = refs::track_ref(&rows::track_id(reaper, track)).into();
    }
    let kind = reaper.fx_config(fx, "fx_type").unwrap_or_default().to_ascii_uppercase();
    let own = reaper.fx_config(fx, "fx_name").filter(|name| !name.is_empty());
    let made_by_cockos = own.clone().unwrap_or_else(|| reaper.fx_name(fx)).trim_end().ends_with("(Cockos)");
    row["plugin"] = (PLUGIN_TYPES.iter().any(|prefix| kind.starts_with(prefix)) && !made_by_cockos).into();
    // An instrument (VSTi, VST3i, AUi, CLAPi, LV2i, DXi): a gain Kumi adds goes after it, where the track's sound is.
    if kind.ends_with('I') {
        row["instrument"] = true.into();
    }
    let renamed = reaper.fx_config(fx, "renamed_name").is_some_and(|name| !name.is_empty());
    if let Some(own) = own.filter(|_| renamed) {
        row["originalName"] = rows::clip(&own).into();
    }
}

/// Stretches merged where they touch or overlap, in order.
pub fn merge(mut spans: Vec<(f64, f64)>, gap: f64) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
    for (from, to) in spans {
        match merged.last_mut() {
            Some(last) if from <= last.1 + gap => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    merged
}

/// Whether an item plays: not muted, and on a track with fixed lanes, on a lane that plays.
fn plays(reaper: &Reaper, track: Track, item: crate::reaper::Item) -> bool {
    if reaper.item_value(item, "B_MUTE") != 0.0 {
        return false;
    }
    let lanes = reaper.track_value(track, "I_FREEMODE") as i64 == 2;
    !lanes || reaper.item_value(item, "C_LANEPLAYS") as i64 != 0
}

/// A track's row of spans: where it plays, in quarter notes from the project start (its items' spans through the tempo
/// map), merged where they touch.
pub fn spans_row(reaper: &Reaper, proj: Project, track: Track) -> Value {
    let mut spans = Vec::new();
    for index in 0..reaper.track_item_count(track) {
        let Some(item) = reaper.track_item(track, index) else { continue };
        if !plays(reaper, track, item) {
            continue;
        }
        let position = reaper.item_value(item, "D_POSITION");
        let length = reaper.item_value(item, "D_LENGTH");
        if let (Some(from), Some(to)) = (reaper.quarter_notes_at(proj, position), reaper.quarter_notes_at(proj, position + length)) {
            if to > from {
                spans.push((from, to));
            }
        }
    }
    let mut merged = merge(spans, 1e-9);
    let mut coarse = false;
    let mut gap = 1.0;
    while merged.len() > MAX_SPANS {
        merged = merge(merged, gap);
        gap *= 2.0;
        coarse = true;
    }
    let mut row = json!({
        "ref": refs::track_ref(&rows::track_id(reaper, track)),
        "name": rows::clip(&reaper.track_name(track)),
        "spans": merged.iter().map(|(from, to)| json!([round(*from), round(*to)])).collect::<Vec<_>>(),
    });
    if reaper.track_value(track, "B_MUTE") != 0.0 {
        row["mute"] = true.into();
    }
    if coarse {
        // Gaps shorter than this many beats were closed to keep the row small.
        row["coarse"] = (gap / 2.0).into();
    }
    row
}

/// `grid` in a read's params: how many points across each parameter's range (2 to 64), or none.
pub fn grid_points(params: &Value) -> Result<Option<usize>, (String, String)> {
    match params.get("grid") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(n) if (2..=MAX_GRID).contains(&n) => Ok(Some(n as usize)),
            _ => Err(("bad-params".into(), format!("grid: 2 to {MAX_GRID} points across each parameter's range"))),
        },
    }
}

/// Adds to a parameter's row what the plug-in prints across its range, asked of the plug-in without setting anything: a
/// stepped parameter's values (`items`, a toggle's two), else `grid` points from 0 to 1 (normalized, as the row's value).
/// A plug-in that prints no value but its current one gives only that point, with `gridShort`.
pub fn add_grid(reaper: &Reaper, fx: Fx, param: i32, points: usize, row: &mut Value) {
    let current = reaper.param_normalized(fx, param);
    let short = |row: &mut Value| {
        row["grid"] = json!([[rows::seconds(current), rows::clip(&reaper.param_formatted(fx, param))]]);
        row["gridShort"] = true.into();
    };
    if let Some(steps) = reaper.param_steps(fx, param) {
        let (min, max) = reaper.param_range(fx, param).unwrap_or((0.0, 1.0));
        let count = if steps.toggle {
            2
        } else if steps.step > 0.0 && max > min {
            ((max - min) / steps.step).round() as usize + 1
        } else {
            0
        };
        if (2..=MAX_ITEMS).contains(&count) {
            let texts: Option<Vec<String>> = (0..count).map(|k| reaper.format_param(fx, param, k as f64 / (count - 1) as f64)).collect();
            match texts {
                Some(texts) => row["items"] = texts.iter().map(|t| Value::from(rows::clip(t))).collect::<Vec<_>>().into(),
                None => short(row),
            }
            return;
        }
    }
    let texts: Option<Vec<String>> = (0..points).map(|k| reaper.format_param(fx, param, k as f64 / (points - 1) as f64)).collect();
    let Some(texts) = texts else { return short(row) };
    // Every point printed as the current value: the plug-in only prints that.
    let shown = reaper.param_formatted(fx, param);
    if points > 2 && texts.iter().all(|t| *t == texts[0]) && texts[0] == shown {
        return short(row);
    }
    row["grid"] = texts
        .iter()
        .enumerate()
        .map(|(k, text)| json!([rows::seconds(k as f64 / (points - 1) as f64), rows::clip(text)]))
        .collect::<Vec<_>>()
        .into();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_merge_where_they_touch_and_a_gap_closes_only_when_asked() {
        assert_eq!(merge(vec![(4.0, 8.0), (0.0, 4.0), (10.0, 12.0), (11.0, 11.5)], 1e-9), [(0.0, 8.0), (10.0, 12.0)]);
        assert_eq!(merge(vec![(0.0, 4.0), (5.0, 6.0)], 1.0), [(0.0, 6.0)]);
        assert!(merge(vec![], 1.0).is_empty());
    }
}
