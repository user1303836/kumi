//! The project beyond its tracks (r-tracks, M4): the time selection, the notes, title and author, the ruler lanes, a
//! region's render matrix, the grid and the timebase. Each is read back after it is written, and put back by value if the
//! producer has not touched it since. REAPER's own undo takes back the time selection, title and author, ruler lanes and
//! render matrix; not the notes, grid or timebase (7.82 lab: no point), which Kumi's undo alone puts back.

use serde_json::{json, Map, Value};

use super::marks::{bar_text, moment, trimmed};
use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::refs::{self, Ref, TrackId};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// Whether `a` holds what `b` says, for every field `b` names (numbers within REAPER's rounding).
fn holds(a: &Value, b: &Value) -> bool {
    b.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, want)| match (a.get(key), want) {
            (Some(have), Value::Number(want)) => have.as_f64().zip(want.as_f64()).is_some_and(|(x, y)| close(x, y)),
            (Some(have), want) => have == want,
            (None, _) => false,
        })
    })
}

fn touched(state: &Value, of: &Value) -> Value {
    let mut fields = Map::new();
    for key in of.as_object().into_iter().flatten().map(|(key, _)| key) {
        if let Some(value) = state.get(key) {
            fields.insert(key.clone(), value.clone());
        }
    }
    Value::Object(fields)
}

fn project_target(kind: &str) -> Value {
    json!({"ref": null, "kind": kind})
}

/// A change of a few fields of something read whole: prior and applied are the fields asked for, the fence the whole.
struct Fields<'c, 'a, 'r> {
    ctx: &'c Ctx<'a, 'r>,
    read: &'c dyn Fn() -> Value,
    write: &'c dyn Fn(&Value) -> bool,
    noun: &'static str,
}

impl Fields<'_, '_, '_> {
    fn preview(&self, target: Value, wanted: &Value, title: String, note: Option<String>) -> Preview {
        let now = (self.read)();
        let mut display = json!({"title": title, "before": touched(&now, wanted), "after": wanted});
        if holds(&now, wanted) {
            display["unchanged"] = true.into();
        }
        if let Some(note) = note {
            display["note"] = note.into();
        }
        Preview { target, prior: touched(&now, wanted), fence: now, display }
    }

    fn apply(&self, wanted: &Value, fence: &Value, title: String, note: Option<String>) -> Done<Applied> {
        let now = (self.read)();
        if !holds(&now, fence) {
            return Err(changed(format!("the {} changed since the preview", self.noun)));
        }
        let unwritten = holds(&now, wanted);
        if !unwritten && !(self.write)(wanted) {
            return Err(failed(format!("REAPER wouldn't take the {}", self.noun)));
        }
        let after = (self.read)();
        if !holds(&after, wanted) {
            (self.write)(&touched(&now, wanted));
            return Err(failed(format!("REAPER kept the {} as it was", self.noun)));
        }
        let mut display = json!({"title": title, "before": touched(&now, wanted), "after": touched(&after, wanted)});
        if let Some(note) = note {
            display["note"] = note.into();
        }
        let _ = self.ctx;
        Ok(Applied { applied: touched(&after, wanted), created: None, material: None, display, unwritten })
    }

    fn restore(&self, restore: &Restore, title: String) -> Done<Restored> {
        let now = (self.read)();
        let already = holds(&now, restore.prior);
        if !already {
            if !restore.discard && !holds(&now, restore.applied) {
                return Err(changed(format!("the {} has been changed in REAPER since Kumi set it", self.noun)));
            }
            if !(self.write)(restore.prior) || !holds(&(self.read)(), restore.prior) {
                return Err(failed(format!("REAPER wouldn't put back the {}", self.noun)));
            }
        }
        let after = touched(&(self.read)(), restore.prior);
        Ok(Restored {
            restored: after.clone(),
            display: json!({"title": format!("{title} (put back)"), "before": touched(&now, restore.prior), "after": after}),
            unwritten: already,
        })
    }
}

// ----- the time selection -----

fn time_selection(ctx: &Ctx) -> Value {
    let (start, end) = ctx.reaper.time_selection(ctx.proj);
    json!({"start": start, "end": end})
}

fn time_selection_wanted(ctx: &Ctx, input: &Value) -> Done<Value> {
    let start = moment(ctx, input, "startBeats", "start")?
        .ok_or_else(|| bad("pass startBeats (quarter notes from the start) or start (seconds)"))?;
    let end = moment(ctx, input, "endBeats", "end")?.ok_or_else(|| bad("pass endBeats or end"))?;
    if end <= start && !(start == 0.0 && end == 0.0) {
        return Err(bad("the time selection must end after it starts (0 and 0 clears it)"));
    }
    Ok(json!({"start": start, "end": end}))
}

fn time_selection_title(ctx: &Ctx, state: &Value) -> String {
    let (start, end) = (number(state, "start"), number(state, "end"));
    if end <= start {
        "Time selection cleared".into()
    } else {
        format!("Time selection {} to {}", bar_text(ctx.reaper, ctx.proj, start), bar_text(ctx.reaper, ctx.proj, end))
    }
}

fn time_fields<'c, 'a, 'r>(ctx: &'c Ctx<'a, 'r>, read: &'c dyn Fn() -> Value, write: &'c dyn Fn(&Value) -> bool) -> Fields<'c, 'a, 'r> {
    Fields { ctx, read, write, noun: "time selection" }
}

fn write_time_selection(ctx: &Ctx, state: &Value) -> bool {
    ctx.reaper.set_time_selection(ctx.proj, number(state, "start"), number(state, "end"))
}

pub fn time_selection_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = time_selection_wanted(ctx, input)?;
    let (read, write) = (|| time_selection(ctx), |state: &Value| write_time_selection(ctx, state));
    Ok(time_fields(ctx, &read, &write).preview(project_target("project"), &wanted, time_selection_title(ctx, &wanted), None))
}

pub fn time_selection_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = time_selection_wanted(ctx, input)?;
    let (read, write) = (|| time_selection(ctx), |state: &Value| write_time_selection(ctx, state));
    time_fields(ctx, &read, &write).apply(&wanted, fence, time_selection_title(ctx, &wanted), None)
}

pub fn time_selection_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (read, write) = (|| time_selection(ctx), |state: &Value| write_time_selection(ctx, state));
    time_fields(ctx, &read, &write).restore(restore, time_selection_title(ctx, restore.prior))
}

// ----- notes, title and author -----

/// The most of the project's notes Kumi writes or keeps to put back.
const MOST_NOTES: usize = 256 * 1024;

fn notes(ctx: &Ctx) -> Done<Value> {
    let text = ctx.reaper.project_notes(ctx.proj).ok_or_else(|| failed("this REAPER can't read the project's notes"))?;
    if text.len() > MOST_NOTES {
        return Err(failed(format!(
            "the project's notes are {} KB, more than the {} KB Kumi keeps to put back: change them in REAPER",
            text.len() >> 10,
            MOST_NOTES >> 10
        )));
    }
    Ok(json!({"notes": text}))
}

fn notes_wanted(input: &Value) -> Done<Value> {
    let text = input.get("notes").and_then(Value::as_str).ok_or_else(|| bad("notes must be text (\"\" clears them)"))?;
    if text.len() > MOST_NOTES {
        return Err(bad(format!("notes must be at most {} KB", MOST_NOTES >> 10)));
    }
    Ok(json!({"notes": text}))
}

fn notes_title(state: &Value) -> String {
    if state["notes"].as_str().is_some_and(str::is_empty) {
        "Project notes cleared".into()
    } else {
        "Project notes changed".into()
    }
}

const NOTES_NOTE: &str = "REAPER's own undo won't take back the project's notes; Kumi can.";

pub fn notes_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = notes_wanted(input)?;
    notes(ctx)?;
    let read = || notes(ctx).unwrap_or(Value::Null);
    let write = |state: &Value| ctx.reaper.set_project_notes(ctx.proj, state["notes"].as_str().unwrap_or_default());
    let fields = Fields { ctx, read: &read, write: &write, noun: "project's notes" };
    Ok(fields.preview(project_target("project"), &wanted, notes_title(&wanted), Some(NOTES_NOTE.into())))
}

pub fn notes_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = notes_wanted(input)?;
    notes(ctx)?;
    let read = || notes(ctx).unwrap_or(Value::Null);
    let write = |state: &Value| ctx.reaper.set_project_notes(ctx.proj, state["notes"].as_str().unwrap_or_default());
    Fields { ctx, read: &read, write: &write, noun: "project's notes" }.apply(&wanted, fence, notes_title(&wanted), Some(NOTES_NOTE.into()))
}

pub fn notes_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let read = || notes(ctx).unwrap_or(Value::Null);
    let write = |state: &Value| ctx.reaper.set_project_notes(ctx.proj, state["notes"].as_str().unwrap_or_default());
    Fields { ctx, read: &read, write: &write, noun: "project's notes" }.restore(restore, notes_title(restore.prior))
}

fn info(ctx: &Ctx) -> Value {
    let text = |key: &str| ctx.reaper.project_text(ctx.proj, key).unwrap_or_default();
    json!({"title": text("PROJECT_TITLE"), "author": text("PROJECT_AUTHOR")})
}

fn info_wanted(input: &Value) -> Done<Value> {
    let mut wanted = json!({});
    for key in ["title", "author"] {
        match input.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if text.chars().count() <= 1024 && !text.contains(['\n', '\r']) => wanted[key] = text.clone().into(),
            Some(_) => return Err(bad(format!("{key} must be one line of at most 1024 characters"))),
        }
    }
    if wanted.as_object().is_some_and(Map::is_empty) {
        return Err(bad("give title, author or both"));
    }
    Ok(wanted)
}

fn info_title(state: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(title) = state.get("title").and_then(Value::as_str) {
        parts.push(if title.is_empty() { "Project title cleared".to_string() } else { format!("Project title {}", super::quoted(title)) });
    }
    if let Some(author) = state.get("author").and_then(Value::as_str) {
        parts.push(if author.is_empty() { "author cleared".to_string() } else { format!("author {}", super::quoted(author)) });
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn write_info(ctx: &Ctx, state: &Value) -> bool {
    [("title", "PROJECT_TITLE"), ("author", "PROJECT_AUTHOR")]
        .iter()
        .all(|(key, info)| state.get(*key).and_then(Value::as_str).is_none_or(|text| ctx.reaper.set_project_text(ctx.proj, info, text)))
}

/// (7.82 lab: setting them makes no point in REAPER's history.)
const INFO_NOTE: &str = "REAPER's own undo won't take back the project's title and author; Kumi can.";

pub fn info_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = info_wanted(input)?;
    let (read, write) = (|| info(ctx), |state: &Value| write_info(ctx, state));
    Ok(Fields { ctx, read: &read, write: &write, noun: "project's title and author" }.preview(
        project_target("project"),
        &wanted,
        info_title(&wanted),
        Some(INFO_NOTE.into()),
    ))
}

pub fn info_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = info_wanted(input)?;
    let (read, write) = (|| info(ctx), |state: &Value| write_info(ctx, state));
    Fields { ctx, read: &read, write: &write, noun: "project's title and author" }.apply(
        &wanted,
        fence,
        info_title(&wanted),
        Some(INFO_NOTE.into()),
    )
}

pub fn info_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (read, write) = (|| info(ctx), |state: &Value| write_info(ctx, state));
    Fields { ctx, read: &read, write: &write, noun: "project's title and author" }.restore(restore, info_title(restore.prior))
}

// ----- the grid and the timebase -----

/// A grid division in whole notes, from a number or a note value: "1/16", "1/8T" (triplet), "1/4." (dotted).
fn division(value: &Value) -> Done<f64> {
    let shape = "division must be a note value (\"1/16\", \"1/8T\" for a triplet, \"1/4.\" dotted) or a number of whole notes";
    if let Some(n) = value.as_f64() {
        return Some(n).filter(|n| n.is_finite() && *n > 0.0 && *n <= 16.0).ok_or_else(|| bad(shape));
    }
    let text = value.as_str().ok_or_else(|| bad(shape))?.trim();
    let (text, factor) = if let Some(rest) = text.strip_suffix(['T', 't']) {
        (rest, 2.0 / 3.0)
    } else if let Some(rest) = text.strip_suffix('.') {
        (rest, 1.5)
    } else {
        (text, 1.0)
    };
    let (top, bottom) = text.split_once('/').ok_or_else(|| bad(shape))?;
    let (top, bottom) = (top.trim().parse::<f64>().map_err(|_| bad(shape))?, bottom.trim().parse::<f64>().map_err(|_| bad(shape))?);
    if top <= 0.0 || bottom <= 0.0 || top / bottom > 16.0 {
        return Err(bad(shape));
    }
    Ok(top / bottom * factor)
}

/// A division in words: "1/16", "1/8 triplet", "1/4 dotted"; a note value of a power of two first, as the grid menu has.
fn division_text(division: f64) -> String {
    let whole = |value: f64| (value - value.round()).abs() < 1e-6 && value.round() >= 1.0;
    for power_of_two in [true, false] {
        for (factor, words) in [(1.0, ""), (2.0 / 3.0, " triplet"), (1.5, " dotted")] {
            let plain = division / factor;
            let note = 1.0 / plain;
            if whole(note) && (!power_of_two || (note.round() as i64).count_ones() == 1) {
                return format!("1/{}{words}", note.round() as i64);
            }
            if whole(plain) && (!power_of_two || (plain.round() as i64).count_ones() == 1) {
                return format!("{}{words}", plain.round() as i64);
            }
        }
    }
    format!("{} whole notes", trimmed(division))
}

fn grid(ctx: &Ctx) -> Done<Value> {
    let (division, mode, amount) = ctx.reaper.grid(ctx.proj).ok_or_else(|| failed("this REAPER can't read the grid"))?;
    Ok(json!({"division": division, "mode": mode, "swing": amount}))
}

fn grid_wanted(input: &Value) -> Done<Value> {
    let mut wanted = json!({});
    if let Some(value) = input.get("division").filter(|v| !v.is_null()) {
        wanted["division"] = division(value)?.into();
    }
    match (input.get("swing").filter(|v| !v.is_null()), input.get("measure").and_then(Value::as_bool)) {
        (_, Some(true)) => wanted["mode"] = 3.into(),
        (Some(swing), _) => {
            let amount =
                swing.as_f64().filter(|a| (-1.0..=1.0).contains(a)).ok_or_else(|| bad("swing must be from -1 to 1 (0 is none)"))?;
            wanted["mode"] = if amount == 0.0 { 0 } else { 1 }.into();
            wanted["swing"] = amount.into();
        }
        (None, Some(false)) => wanted["mode"] = 0.into(),
        (None, None) => {}
    }
    if wanted.as_object().is_some_and(Map::is_empty) {
        return Err(bad("give division, swing or measure"));
    }
    Ok(wanted)
}

fn grid_title(state: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(division) = state.get("division").and_then(Value::as_f64) {
        parts.push(division_text(division));
    }
    match state.get("mode").and_then(Value::as_i64) {
        Some(3) => parts.push("the measure grid".into()),
        Some(1) => parts.push(format!("swing {}%", (number(state, "swing") * 100.0).round() as i64)),
        Some(0) => parts.push("no swing".into()),
        _ => {}
    }
    format!("Grid {}", parts.join(", "))
}

fn write_grid(ctx: &Ctx, state: &Value) -> bool {
    let Ok(now) = grid(ctx) else { return false };
    let pick = |key: &str| state.get(key).and_then(Value::as_f64).unwrap_or_else(|| number(&now, key));
    ctx.reaper.set_grid(ctx.proj, pick("division"), pick("mode") as i32, pick("swing"))
}

const GRID_NOTE: &str = "REAPER's own undo won't take back the grid; Kumi can.";

pub fn grid_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = grid_wanted(input)?;
    let (read, write) = (|| grid(ctx).unwrap_or(Value::Null), |state: &Value| write_grid(ctx, state));
    Ok(Fields { ctx, read: &read, write: &write, noun: "grid" }.preview(
        project_target("project"),
        &wanted,
        grid_title(&wanted),
        Some(GRID_NOTE.into()),
    ))
}

pub fn grid_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = grid_wanted(input)?;
    let (read, write) = (|| grid(ctx).unwrap_or(Value::Null), |state: &Value| write_grid(ctx, state));
    Fields { ctx, read: &read, write: &write, noun: "grid" }.apply(&wanted, fence, grid_title(&wanted), Some(GRID_NOTE.into()))
}

pub fn grid_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (read, write) = (|| grid(ctx).unwrap_or(Value::Null), |state: &Value| write_grid(ctx, state));
    Fields { ctx, read: &read, write: &write, noun: "grid" }.restore(restore, grid_title(restore.prior))
}

const TIMEBASES: [(&str, i64, &str); 3] =
    [("time", 0, "time"), ("beats", 1, "beats (position, length, rate)"), ("beats-position", 2, "beats (position only)")];

fn timebase(ctx: &Ctx) -> Done<Value> {
    let read = |name: &str| -> Done<i64> {
        ctx.reaper
            .config_text(name)
            .and_then(|text| text.trim().parse::<i64>().ok())
            .ok_or_else(|| failed("this REAPER can't say the project's timebase"))
    };
    Ok(json!({"items": read("itemtimelock")?, "tempoEnvelopes": read("tempoenvtimelock")?}))
}

fn timebase_wanted(input: &Value) -> Done<Value> {
    let mut wanted = json!({});
    if let Some(value) = input.get("items").filter(|v| !v.is_null()) {
        let code = TIMEBASES
            .iter()
            .find(|(name, _, _)| value == name)
            .map(|(_, code, _)| *code)
            .ok_or_else(|| bad("items must be \"time\", \"beats\" or \"beats-position\""))?;
        wanted["items"] = code.into();
    }
    if let Some(value) = input.get("tempoEnvelopes").filter(|v| !v.is_null()) {
        let code = TIMEBASES[..2]
            .iter()
            .find(|(name, _, _)| value == name)
            .map(|(_, code, _)| *code)
            .ok_or_else(|| bad("tempoEnvelopes must be \"time\" or \"beats\""))?;
        wanted["tempoEnvelopes"] = code.into();
    }
    if wanted.as_object().is_some_and(Map::is_empty) {
        return Err(bad("give items (the timebase of items, envelopes and markers) or tempoEnvelopes"));
    }
    Ok(wanted)
}

fn timebase_title(state: &Value) -> String {
    let words = |code: i64| TIMEBASES.iter().find(|(_, c, _)| *c == code).map_or("other", |(_, _, words)| *words);
    let mut parts = Vec::new();
    if let Some(items) = state.get("items").and_then(Value::as_i64) {
        parts.push(format!("Project timebase {}", words(items)));
    }
    if let Some(tempo) = state.get("tempoEnvelopes").and_then(Value::as_i64) {
        parts.push(format!("tempo envelope timebase {}", words(tempo)));
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn write_timebase(ctx: &Ctx, state: &Value) -> bool {
    [("items", "itemtimelock"), ("tempoEnvelopes", "tempoenvtimelock")].iter().all(|(key, name)| {
        state.get(*key).and_then(Value::as_i64).is_none_or(|code| ctx.reaper.set_config_text(name, &code.to_string()) == 2)
    })
}

const TIMEBASE_NOTE: &str =
    "REAPER's own undo won't take back the project's timebase; Kumi can. Items already in the project keep their own.";

pub fn timebase_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = timebase_wanted(input)?;
    timebase(ctx)?;
    let (read, write) = (|| timebase(ctx).unwrap_or(Value::Null), |state: &Value| write_timebase(ctx, state));
    Ok(Fields { ctx, read: &read, write: &write, noun: "project's timebase" }.preview(
        project_target("project"),
        &wanted,
        timebase_title(&wanted),
        Some(TIMEBASE_NOTE.into()),
    ))
}

pub fn timebase_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = timebase_wanted(input)?;
    let (read, write) = (|| timebase(ctx).unwrap_or(Value::Null), |state: &Value| write_timebase(ctx, state));
    Fields { ctx, read: &read, write: &write, noun: "project's timebase" }.apply(
        &wanted,
        fence,
        timebase_title(&wanted),
        Some(TIMEBASE_NOTE.into()),
    )
}

pub fn timebase_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let (read, write) = (|| timebase(ctx).unwrap_or(Value::Null), |state: &Value| write_timebase(ctx, state));
    Fields { ctx, read: &read, write: &write, noun: "project's timebase" }.restore(restore, timebase_title(restore.prior))
}

// ----- ruler lanes -----

/// A ruler lane is found by its GUID (REAPER keeps one for each), not its number, which reordering lanes moves.
pub(super) fn lane_by_guid(ctx: &Ctx, guid: &str) -> Option<i64> {
    let count = ctx.reaper.project_info(ctx.proj, "RULER_LANE_COUNT")? as i64;
    let at = ctx.reaper.project_info(ctx.proj, &format!("RULER_LANE_FROM_GUID:{guid}"))? as i64;
    (0..count).contains(&at).then_some(at).filter(|at| lane_guid(ctx, *at).as_deref() == Some(guid))
}

/// The id of a lane the project has.
pub(super) fn lane_guid(ctx: &Ctx, lane: i64) -> Option<String> {
    if !(0..lanes(ctx)).contains(&lane) {
        return None;
    }
    ctx.reaper.project_text(ctx.proj, &format!("RULER_LANE_GUID:{lane}")).filter(|guid| !guid.is_empty())
}

fn lanes(ctx: &Ctx) -> i64 {
    ctx.reaper.project_info(ctx.proj, "RULER_LANE_COUNT").unwrap_or(0.0) as i64
}

fn lane_state(ctx: &Ctx, lane: i64) -> Value {
    let info = |key: &str| ctx.reaper.project_info(ctx.proj, &format!("{key}:{lane}")).unwrap_or(0.0);
    json!({
        "name": ctx.reaper.project_text(ctx.proj, &format!("RULER_LANE_NAME:{lane}")).unwrap_or_default(),
        "hidden": info("RULER_LANE_HIDDEN") != 0.0,
        "locked": info("RULER_LANE_LOCKED") != 0.0,
        "color": info("RULER_LANE_COLOR") as i64,
        "timebase": info("RULER_LANE_TIMEBASE") as i64,
    })
}

fn write_lane(ctx: &Ctx, lane: i64, state: &Value) -> bool {
    let set = |key: &str, value: f64| ctx.reaper.set_project_info(ctx.proj, &format!("{key}:{lane}"), value);
    state
        .get("name")
        .and_then(Value::as_str)
        .is_none_or(|name| ctx.reaper.set_project_text(ctx.proj, &format!("RULER_LANE_NAME:{lane}"), name))
        && state.get("hidden").and_then(Value::as_bool).is_none_or(|on| set("RULER_LANE_HIDDEN", f64::from(u8::from(on))))
        && state.get("locked").and_then(Value::as_bool).is_none_or(|on| set("RULER_LANE_LOCKED", f64::from(u8::from(on))))
        && state.get("color").and_then(Value::as_f64).is_none_or(|color| set("RULER_LANE_COLOR", color))
        && state.get("timebase").and_then(Value::as_f64).is_none_or(|timebase| set("RULER_LANE_TIMEBASE", timebase))
}

fn lane_label(ctx: &Ctx, lane: i64) -> String {
    let name = ctx.reaper.project_text(ctx.proj, &format!("RULER_LANE_NAME:{lane}")).unwrap_or_default();
    if name.is_empty() {
        format!("Ruler lane {lane}")
    } else {
        format!("Ruler lane {lane} ({})", super::quoted(&name))
    }
}

fn lane_wanted(ctx: &Ctx, input: &Value) -> Done<Value> {
    let mut wanted = json!({});
    match input.get("name") {
        None | Some(Value::Null) => {}
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => {
            wanted["name"] = name.clone().into()
        }
        Some(_) => return Err(bad("name must be one line of at most 256 characters")),
    }
    for key in ["hidden", "locked"] {
        match input.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(on)) => wanted[key] = (*on).into(),
            Some(_) => return Err(bad(format!("{key} must be true or false"))),
        }
    }
    match input.get("color") {
        None => {}
        Some(Value::Null) => wanted["color"] = 0.into(),
        Some(Value::String(hex)) => {
            let digits = hex
                .strip_prefix('#')
                .filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| bad("color must be #rrggbb, or null for none"))?;
            let channel = |at: usize| i32::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
            let native =
                ctx.reaper.color_to_native(channel(0), channel(2), channel(4)).ok_or_else(|| failed("this REAPER can't make a colour"))?;
            wanted["color"] = (native | 0x100_0000).into();
        }
        Some(_) => return Err(bad("color must be #rrggbb, or null for none")),
    }
    if let Some(value) = input.get("timebase").filter(|v| !v.is_null()) {
        let code = match value.as_str() {
            Some("default") => -1,
            Some(name) => TIMEBASES
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, c, _)| *c)
                .ok_or_else(|| bad("timebase must be \"default\", \"time\", \"beats\" or \"beats-position\""))?,
            None => return Err(bad("timebase must be \"default\", \"time\", \"beats\" or \"beats-position\"")),
        };
        wanted["timebase"] = code.into();
    }
    if wanted.as_object().is_some_and(Map::is_empty) {
        return Err(bad("give name, hidden, locked, color or timebase"));
    }
    Ok(wanted)
}

fn lane_title(label: &str, state: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(name) = state.get("name").and_then(Value::as_str) {
        parts.push(if name.is_empty() { "unnamed".to_string() } else { format!("named {}", super::quoted(name)) });
    }
    for (key, on, off) in [("hidden", "hidden", "shown"), ("locked", "locked", "unlocked")] {
        if let Some(value) = state.get(key).and_then(Value::as_bool) {
            parts.push(if value { on } else { off }.to_string());
        }
    }
    if state.get("color").is_some() {
        parts.push(if state["color"].as_i64().unwrap_or(0) & 0x100_0000 == 0 { "colour cleared".into() } else { "colour changed".into() });
    }
    if let Some(timebase) = state.get("timebase").and_then(Value::as_i64) {
        let words =
            if timebase < 0 { "the project's" } else { TIMEBASES.iter().find(|(_, c, _)| *c == timebase).map_or("other", |(_, _, w)| *w) };
        parts.push(format!("timebase {words}"));
    }
    format!("{label} {}", parts.join(", "))
}

fn lane_of(ctx: &Ctx, input: &Value) -> Done<i64> {
    let count = lanes(ctx);
    input
        .get("lane")
        .and_then(Value::as_i64)
        .filter(|lane| (0..count).contains(lane))
        .ok_or_else(|| bad(format!("lane must be a ruler lane from 0 to {} (the marker rows' lane says which)", count - 1)))
}

pub fn lane_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let lane = lane_of(ctx, input)?;
    let guid = lane_guid(ctx, lane).ok_or_else(|| failed("REAPER gives that ruler lane no id"))?;
    let wanted = lane_wanted(ctx, input)?;
    let (read, write) = (|| lane_state(ctx, lane), |state: &Value| write_lane(ctx, lane, state));
    let target = json!({"ref": null, "kind": "ruler-lane", "lane": lane, "guid": guid});
    let mut preview = Fields { ctx, read: &read, write: &write, noun: "ruler lane" }.preview(
        target,
        &wanted,
        lane_title(&lane_label(ctx, lane), &wanted),
        None,
    );
    // The lane the number named at the preview: the apply writes to that one or none.
    preview.fence = json!({"state": preview.fence, "guid": guid});
    Ok(preview)
}

pub fn lane_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let lane = lane_of(ctx, input)?;
    let guid = fence["guid"].as_str().ok_or_else(|| bad("fence: the ruler lane's id"))?;
    if lane_by_guid(ctx, guid) != Some(lane) {
        return Err(changed("the ruler lanes were reordered, added or taken away since the preview: that number is another lane now"));
    }
    let wanted = lane_wanted(ctx, input)?;
    let (read, write) = (|| lane_state(ctx, lane), |state: &Value| write_lane(ctx, lane, state));
    let label = lane_label(ctx, lane);
    let mut applied = Fields { ctx, read: &read, write: &write, noun: "ruler lane" }.apply(
        &wanted,
        &fence["state"],
        lane_title(&label, &wanted),
        None,
    )?;
    applied.display["title"] = lane_title(&label, &wanted).into();
    Ok(applied)
}

pub fn lane_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let guid = restore.target["guid"].as_str().ok_or_else(|| bad("target: the ruler lane's id"))?;
    let lane = lane_by_guid(ctx, guid).ok_or_else(|| ("gone".to_string(), "that ruler lane isn't in the project any more".to_string()))?;
    let (read, write) = (|| lane_state(ctx, lane), |state: &Value| write_lane(ctx, lane, state));
    Fields { ctx, read: &read, write: &write, noun: "ruler lane" }.restore(restore, lane_title(&lane_label(ctx, lane), restore.prior))
}

/// The markers and regions on a lane.
fn on_lane(ctx: &Ctx, lane: i64) -> usize {
    (0..ctx.reaper.marker_count(ctx.proj))
        .filter_map(|index| ctx.reaper.marker(ctx.proj, index))
        .filter(|marker| marker.lane as i64 == lane)
        .count()
}

pub fn lane_add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let wanted = lane_add_wanted(ctx, input)?;
    let count = lanes(ctx);
    Ok(Preview {
        target: json!({"ref": null, "kind": "ruler-lane", "lane": count}),
        prior: json!({"lanes": count}),
        fence: json!({"lanes": count}),
        display: json!({"title": lane_title(&format!("Added ruler lane {count}"), &wanted)}),
    })
}

fn lane_add_wanted(ctx: &Ctx, input: &Value) -> Done<Value> {
    if input.as_object().is_none_or(|fields| fields.keys().all(|key| key == "lane")) {
        return Ok(json!({}));
    }
    lane_wanted(ctx, input)
}

pub fn lane_add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = lane_add_wanted(ctx, input)?;
    let count = lanes(ctx);
    if json!({"lanes": count}) != *fence {
        return Err(changed("the ruler lanes changed since the preview"));
    }
    if !ctx.reaper.set_project_info(ctx.proj, "RULER_LANE_COUNT", (count + 1) as f64) || lanes(ctx) != count + 1 {
        return Err(failed("REAPER wouldn't add a ruler lane"));
    }
    let Some(guid) = lane_guid(ctx, count) else {
        ctx.reaper.set_project_info(ctx.proj, "RULER_LANE_COUNT", count as f64);
        return Err(failed("REAPER gave the new ruler lane no id: it is taken away again"));
    };
    if wanted.as_object().is_some_and(|fields| !fields.is_empty()) && !write_lane(ctx, count, &wanted) {
        ctx.reaper.set_project_info(ctx.proj, "RULER_LANE_COUNT", count as f64);
        return Err(failed("REAPER wouldn't take the new ruler lane's settings: it is taken away again"));
    }
    Ok(Applied {
        applied: json!({"lane": count, "state": lane_state(ctx, count)}),
        created: Some(json!({"ref": null, "kind": "ruler-lane", "lane": count, "guid": guid})),
        material: None,
        display: json!({"title": lane_title(&format!("Added ruler lane {count}"), &wanted)}),
        ..Default::default()
    })
}

pub fn lane_add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the ruler lane that was added"))?;
    let guid = created["guid"].as_str().ok_or_else(|| bad("created: the ruler lane's id"))?;
    let lane = lane_by_guid(ctx, guid).ok_or_else(|| ("gone".to_string(), "that ruler lane isn't in the project any more".to_string()))?;
    if lane != lanes(ctx) - 1 {
        return Err(changed("other ruler lanes come after the one Kumi added now: REAPER takes lanes away from the end only"));
    }
    // The markers and regions on it are the producer's, told to undo anyway or not.
    if on_lane(ctx, lane) > 0 {
        return Err(changed("the ruler lane Kumi added has markers or regions on it now: move them off it first"));
    }
    if !restore.discard && !holds(&lane_state(ctx, lane), &restore.applied["state"]) {
        return Err(changed("the ruler lane Kumi added has been changed in REAPER since"));
    }
    if !ctx.reaper.set_project_info(ctx.proj, "RULER_LANE_COUNT", lane as f64) || lanes(ctx) != lane {
        return Err(failed("REAPER wouldn't take the ruler lane away"));
    }
    Ok(Restored {
        restored: json!({"lanes": lane}),
        display: json!({"title": format!("Ruler lane {lane} taken away again")}),
        ..Default::default()
    })
}

// ----- a region's render matrix -----

/// The tracks a request names for the matrix, by id ("master" for the master).
pub(super) fn matrix_tracks(input: &Value, key: &str) -> Done<Vec<TrackId>> {
    let Some(list) = input.get(key).filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let list = list.as_array().filter(|list| list.len() <= 1000).ok_or_else(|| bad(format!("{key} must list track refs")))?;
    list.iter()
        .map(|reference| match Ref::parse(reference.as_str().unwrap_or_default()).map_err(bad)? {
            Ref::Track(id) => Ok(id),
            other => Err(bad(format!("{key}: a track ref, not r:{}", other.kind()))),
        })
        .collect()
}

pub(super) fn id_text(id: &TrackId) -> String {
    match id {
        TrackId::Master => "master".into(),
        TrackId::Guid(guid) => guid.clone(),
    }
}

fn region_of(ctx: &Ctx, reference: &Value) -> Done<crate::reaper::Marker> {
    let text = reference.as_str().ok_or_else(|| bad("ref must be a region ref"))?;
    let Ref::Region(guid) = Ref::parse(text).map_err(bad)? else { return Err(bad("ref must be a region ref (the matrix is a region's)")) };
    ctx.reaper
        .marker_by_guid(ctx.proj, &guid)
        .filter(|marker| marker.is_region)
        .ok_or_else(|| failed("that region isn't in the project (deleted, or another tab)"))
}

/// Which of these tracks the region renders now: track id → in the matrix.
pub(super) fn matrix_state(ctx: &Ctx, number: i32, ids: &[String]) -> Value {
    let rendered: Vec<String> = ctx
        .reaper
        .region_render_tracks(ctx.proj, number)
        .into_iter()
        .map(|track| id_text(&super::tracks::track_id_of(ctx, track)))
        .collect();
    let mut state = Map::new();
    for id in ids {
        state.insert(id.clone(), rendered.contains(id).into());
    }
    Value::Object(state)
}

struct Matrix {
    region: crate::reaper::Marker,
    wanted: Value,
    tracks: Vec<(String, crate::reaper::Track)>,
}

fn matrix_plan(ctx: &Ctx, input: &Value) -> Done<Matrix> {
    let region = region_of(ctx, &input["ref"])?;
    let (add, remove) = (matrix_tracks(input, "addRefs")?, matrix_tracks(input, "removeRefs")?);
    if add.is_empty() && remove.is_empty() {
        return Err(bad("give addRefs, removeRefs or both: the tracks the region renders, or no longer renders"));
    }
    let mut wanted = Map::new();
    let mut tracks = Vec::new();
    for (ids, on) in [(&add, true), (&remove, false)] {
        for id in ids {
            let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, id)?;
            if wanted.insert(id_text(id), on.into()).is_some() {
                return Err(bad("a track can't be both added to and taken out of the matrix"));
            }
            tracks.push((id_text(id), track));
        }
    }
    Ok(Matrix { region, wanted: Value::Object(wanted), tracks })
}

fn matrix_title(ctx: &Ctx, region: &crate::reaper::Marker, wanted: &Value) -> String {
    let name = |id: &str| match id {
        "master" => "the master".to_string(),
        guid => ctx
            .reads
            .track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string()))
            .map(|t| super::quoted(&super::track_name(ctx.reaper, t)))
            .unwrap_or_else(|_| "a track".into()),
    };
    let pick =
        |on: bool| -> Vec<String> { wanted.as_object().into_iter().flatten().filter(|(_, v)| **v == on).map(|(id, _)| name(id)).collect() };
    let region_name =
        if region.name.is_empty() { format!("region {}", region.number) } else { format!("region {}", super::quoted(&region.name)) };
    let mut parts = Vec::new();
    let added = pick(true);
    if !added.is_empty() {
        parts.push(format!("renders {}", added.join(", ")));
    }
    let removed = pick(false);
    if !removed.is_empty() {
        parts.push(format!("no longer renders {}", removed.join(", ")));
    }
    let text = format!("{} {}", region_name, parts.join("; "));
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub(super) fn write_matrix(ctx: &Ctx, number: i32, state: &Value) -> bool {
    state.as_object().into_iter().flatten().all(|(id, on)| {
        let track = match id.as_str() {
            "master" => ctx.reaper.master_track(ctx.proj),
            guid => ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string())).ok(),
        };
        track.is_some_and(|track| {
            if *on == true {
                ctx.reaper.add_to_region_render(ctx.proj, number, track)
            } else {
                ctx.reaper.remove_from_region_render(ctx.proj, number, track)
            }
        })
    })
}

pub fn matrix_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let plan = matrix_plan(ctx, input)?;
    let ids: Vec<String> = plan.tracks.iter().map(|(id, _)| id.clone()).collect();
    let number = plan.region.number;
    let (read, write) = (|| matrix_state(ctx, number, &ids), |state: &Value| write_matrix(ctx, number, state));
    let target = json!({"ref": refs::region_ref(&plan.region.guid), "kind": "region"});
    Ok(Fields { ctx, read: &read, write: &write, noun: "region's render matrix" }.preview(
        target,
        &plan.wanted,
        matrix_title(ctx, &plan.region, &plan.wanted),
        None,
    ))
}

pub fn matrix_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = matrix_plan(ctx, input).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    let ids: Vec<String> = plan.tracks.iter().map(|(id, _)| id.clone()).collect();
    let number = plan.region.number;
    let (read, write) = (|| matrix_state(ctx, number, &ids), |state: &Value| write_matrix(ctx, number, state));
    Fields { ctx, read: &read, write: &write, noun: "region's render matrix" }.apply(
        &plan.wanted,
        fence,
        matrix_title(ctx, &plan.region, &plan.wanted),
        None,
    )
}

pub fn matrix_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let region =
        region_of(ctx, &restore.target["ref"]).map_err(|fault| if fault.0 == "failed" { ("gone".to_string(), fault.1) } else { fault })?;
    let ids: Vec<String> = restore.prior.as_object().into_iter().flatten().map(|(id, _)| id.clone()).collect();
    // A track that went since is no longer in any matrix: what is left is put back.
    let present: Vec<String> = ids
        .iter()
        .filter(|id| id.as_str() == "master" || ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid((*id).clone())).is_ok())
        .cloned()
        .collect();
    if present.len() != ids.len() && !restore.discard {
        return Err(("gone".into(), "a track whose render matrix entry Kumi changed isn't in the project any more".into()));
    }
    let number = region.number;
    let prior = touched(restore.prior, &Value::Object(present.iter().map(|id| (id.clone(), Value::Null)).collect()));
    let applied = touched(restore.applied, &prior);
    let narrowed = Restore {
        target: restore.target,
        prior: &prior,
        applied: &applied,
        created: restore.created,
        material: restore.material,
        discard: restore.discard,
    };
    let (read, write) = (|| matrix_state(ctx, number, &present), |state: &Value| write_matrix(ctx, number, state));
    Fields { ctx, read: &read, write: &write, noun: "region's render matrix" }.restore(&narrowed, matrix_title(ctx, &region, &prior))
}
