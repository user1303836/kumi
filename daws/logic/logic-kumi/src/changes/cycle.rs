//! The Cycle button and Custom LCD locators, adapted from 6f8f6e9b's controller. This first port deliberately
//! requires the producer to show Custom and the complete Signature List already. It never posts setup, cleanup,
//! keys or native Undo. Every observation binds the raw locators to a saved project and a proven constant meter.

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::{changed, timeline, uncertain, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::ax::{paths, Node, Root, BRIEF, CHEAP};
use crate::logic;
use crate::refs::{bad, failed};
use crate::rows::Position;

const GUIDE: &str = "Show Logic Pro's Custom LCD with both complete locators and /16 division, and its complete Signature List; stop playback and recording. Kumi supports one known N/4 meter at the project's start.";
const LEGACY: &str = "The saved cycle has no complete project, meter and locator identity. Restore it manually in Logic Pro.";
/// A request cannot start a long, partly completed scrubber walk. Count dispatches, conservatively including
/// lower segments that a higher segment may reset. This bound is independent of elapsed time.
const MAX_STEPS: u64 = 64;
const FIELDS: &[&str] = &["role", "subrole", "title", "desc", "value", "valueDesc", "id", "enabled", "set", "act"];

#[derive(Clone)]
struct Seen {
    state: Value,
    locators: [Vec<Control>; 2],
    cycle: Control,
}

fn unsupported(why: &str) -> crate::refs::Fault {
    failed(format!("{why}. {GUIDE}"))
}

fn integer(value: &Value, low: u32, high: u32) -> Option<u32> {
    let n = value.as_f64()?;
    (n.is_finite() && n.fract() == 0.0 && n >= f64::from(low) && n <= f64::from(high)).then_some(n as u32)
}

fn flag(node: &Node) -> Option<bool> {
    match &node.value {
        Value::Bool(on) => Some(*on),
        value => integer(value, 0, 1).map(|n| n == 1),
    }
}

fn complete(node: &Node) -> bool {
    node.more == 0 && !node.role.is_empty() && node.children.iter().all(complete)
}

fn position(values: &[u32], numerator: u32) -> Option<Position> {
    let [bar, beat, division, tick] = *values else { return None };
    (bar >= 1 && bar <= 1_000_001 && (1..=numerator).contains(&beat) && (1..=4).contains(&division) && (1..=240).contains(&tick))
        .then_some(Position { bar, beat, division, tick })
}

fn raw_position(value: &Value, numerator: u32) -> Option<Position> {
    let values = ["bar", "beat", "division", "tick"].map(|key| integer(&value[key], 1, 1_000_001));
    position(&values.into_iter().collect::<Option<Vec<_>>>()?, numerator)
}

fn segments(node: &Node, numerator: u32) -> Option<Position> {
    if !complete(node) || node.children.len() != 4 || node.children.iter().any(|n| n.role != "AXSlider") {
        return None;
    }
    let values = node.children.iter().map(|n| integer(&n.value, 1, 1_000_001)).collect::<Option<Vec<_>>>()?;
    position(&values, numerator)
}

fn state(on: bool, start: Position, end: Position, timebase: Value) -> Value {
    let numerator = timebase["meter"][0].as_u64().unwrap_or(0) as u32;
    json!({"on": on, "startBeats": start.beats(numerator), "endBeats": end.beats(numerator),
        "start": start.to_json(), "end": end.to_json(), "timebase": timebase})
}

/// Historical or malformed journals never authorize a write, including discard and an already-restored claim.
fn valid_state(value: &Value) -> Option<(Position, Position, u32)> {
    value["on"].as_bool()?;
    let tb = &value["timebase"];
    let numerator = integer(&tb["meter"][0], 1, 16)?;
    if tb["meter"] != json!([numerator, 4]) || tb["division"] != 16 || tb["ticksPerBeat"] != 960 {
        return None;
    }
    let p = &tb["project"];
    let path = p["path"].as_str().filter(|p| p.starts_with('/') && p.len() <= logic::MAX_PATH)?;
    if p["id"].as_str()? != logic::project_id(path)
        || p["name"].as_str()?.is_empty()
        || tb["signature"] != json!([{"position": Position::START.to_json(), "meter": [numerator, 4]}])
    {
        return None;
    }
    let (start, end) = (raw_position(&value["start"], numerator)?, raw_position(&value["end"], numerator)?);
    if start.beats(numerator) >= end.beats(numerator)
        || value["startBeats"].as_f64()? != start.beats(numerator)
        || value["endBeats"].as_f64()? != end.beats(numerator)
    {
        return None;
    }
    Some((start, end, numerator))
}

fn wanted(input: &Value) -> Done<(bool, f64, f64)> {
    if input.get("start").is_some_and(|v| !v.is_null()) || input.get("end").is_some_and(|v| !v.is_null()) {
        return Err(bad("Logic Pro's cycle takes startBeats and endBeats, not seconds"));
    }
    let number = |key: &str| {
        input[key]
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n) && (n * 960.0 - (n * 960.0).round()).abs() < 1.0e-7)
            .ok_or_else(|| bad(format!("{key} must be from 0 to 1000000 beats, exactly on a 1/960-beat tick")))
    };
    let (start, end) = (number("startBeats")?, number("endBeats")?);
    if start == 0.0 && end == 0.0 {
        return Ok((false, start, end));
    }
    if end <= start {
        return Err(bad("the cycle must end after it starts; 0 and 0 turn it off without moving the locators"));
    }
    Ok((true, start, end))
}

/// Require a fresh unambiguous saved main project, rather than trusting the last polling cycle's cached identity.
fn project(ctx: &mut Ctx) -> Done<Value> {
    if ctx.project.root != Root::Main {
        return Err(unsupported("The cycle needs the project's main window"));
    }
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = logic::parse_windows(&reply);
    if windows.iter().any(|w| w.sheet() || w.dialog() || w.modal) {
        return Err(unsupported("Logic Pro has a dialog open"));
    }
    let main: Vec<_> = windows.iter().filter(|w| w.main).collect();
    let [window] = main.as_slice() else {
        return Err(unsupported("The saved main project is ambiguous"));
    };
    if !window.standard() || window.minimized {
        return Err(unsupported("The saved main project isn't visible"));
    }
    let document = window.document.as_deref().ok_or_else(|| unsupported("The cycle needs a saved project identity"))?;
    let path = logic::document_path(document);
    let id = logic::project_id(&path);
    let name = logic::project_of(window).and_then(|p| p.name).ok_or_else(|| unsupported("The saved project name isn't readable"))?;
    if !path.starts_with('/')
        || path.len() > logic::MAX_PATH
        || ctx.project.path.as_deref() != Some(path.as_str())
        || ctx.project.id.as_deref() != Some(id.as_str())
        || ctx.project.identity != name
        || name.is_empty()
    {
        return Err(changed("the saved project changed or its complete identity is unavailable"));
    }
    Ok(json!({"id": id, "path": path, "name": name}))
}

/// Unlike the public M2 reader, this cannot default or omit a malformed signature event. Every row is accounted
/// for, with exactly one Time event at 1.1.1.1. Key events do not affect locator conversion but must be readable.
fn signature(ctx: &mut Ctx, numerator: u32) -> Done<Value> {
    let tabs_path = ctx.project.layout.list_tabs.clone().ok_or_else(|| unsupported("The Signature List isn't shown"))?;
    let tabs = nudge::read(ctx.ax.tree(&ctx.project.root, &tabs_path, 1, 8, Some(BRIEF)))?;
    let selected: Vec<_> = tabs.children.iter().filter(|n| flag(n) == Some(true)).collect();
    if tabs.more != 0 || selected.len() != 1 || !ctx.project.strings.is(&selected[0].desc, "Signature") {
        return Err(unsupported("The complete Signature List must already be selected"));
    }
    let list_path = ctx.project.layout.list.clone().ok_or_else(|| unsupported("The complete Signature List isn't shown"))?;
    let list = nudge::read(ctx.ax.tree(&ctx.project.root, &list_path, 7, 256, Some(CHEAP)))?;
    let mut tables = vec![];
    list.find_all(&|n| n.role == "AXTable", &mut tables);
    let [table] = tables.as_slice() else {
        return Err(unsupported("The Signature List table isn't uniquely readable"));
    };
    if !complete(&list) {
        return Err(unsupported("The complete Signature List is clipped or unreadable"));
    }
    let columns = paths::columns(table);
    let ids: Vec<_> = columns.iter().map(|(id, _)| id.as_str()).collect();
    if table.more != 0 || ids.len() != 3 || !["Position", "Type", "Val"].iter().all(|id| ids.contains(id)) {
        return Err(unsupported("The Signature List's complete columns are unavailable"));
    }
    let cell = |row: &Node, id: &str| columns.iter().find(|(name, _)| name == id).and_then(|(_, index)| row.children.get(*index)).cloned();
    let mut times = vec![];
    for row in paths::rows(&table) {
        if !complete(row) || row.children.len() != columns.len() {
            return Err(unsupported("A Signature List row is incomplete"));
        }
        let p = cell(row, "Position")
            .and_then(|c| c.children.first().cloned())
            .and_then(|group| segments(&group, numerator))
            .ok_or_else(|| unsupported("A signature position isn't completely readable"))?;
        let kind = cell(row, "Type")
            .and_then(|c| c.children.first().map(|n| n.desc.clone()))
            .ok_or_else(|| unsupported("A signature event has no known type"))?;
        let val = cell(row, "Val").ok_or_else(|| unsupported("A signature event has no value"))?;
        if ctx.project.strings.is(&kind, "Time") {
            let sliders: Vec<_> = val.children.iter().filter(|n| n.role == "AXSlider").collect();
            let popups: Vec<_> = val.children.iter().filter(|n| n.role == "AXPopUpButton").collect();
            if sliders.len() != 1
                || popups.len() != 1
                || integer(&sliders[0].value, 1, 16) != Some(numerator)
                || popups[0].value.as_str() != Some("/4")
            {
                return Err(unsupported("The project meter isn't one known N/4 signature"));
            }
            times.push(json!({"position": p.to_json(), "meter": [numerator, 4]}));
        } else if ctx.project.strings.is(&kind, "Key") {
            if val.children.len() != 1
                || val.children[0].role != "AXPopUpButton"
                || val.children[0].value.as_str().is_none_or(str::is_empty)
            {
                return Err(unsupported("A key signature event is unreadable"));
            }
        } else {
            return Err(unsupported("The Signature List contains an unknown event type"));
        }
    }
    let expected = json!([{"position": Position::START.to_json(), "meter": [numerator, 4]}]);
    if Value::Array(times) != expected {
        return Err(unsupported("The complete project meter isn't one N/4 event at the project's start"));
    }
    Ok(expected)
}

fn observe(ctx: &mut Ctx) -> Done<Seen> {
    nudge::read(super::tracks::ensure_layout(ctx))?;
    let identity = project(ctx)?;
    let path = ctx.project.layout.control_bar.clone().ok_or_else(|| unsupported("The control bar isn't shown"))?;
    let bar = nudge::read(ctx.ax.tree(&ctx.project.root, &path, 3, 40, Some(FIELDS)))?;
    if !complete(&bar) || bar.path != path {
        return Err(unsupported("The control bar is incomplete"));
    }
    let lcds: Vec<_> = bar.children.iter().filter(|n| n.role == "AXGroup" && paths::tempo(n, &ctx.project.strings).is_some()).collect();
    let [lcd] = lcds.as_slice() else {
        return Err(unsupported("The LCD isn't uniquely readable"));
    };
    let mode: Vec<_> =
        lcd.children.iter().filter(|n| n.role == "AXPopUpButton" && ctx.project.strings.is(&n.desc, "Display Mode")).collect();
    if mode.len() != 1 || !ctx.project.strings.is(&mode[0].value_text(), "Custom") {
        return Err(unsupported("The LCD must already show Custom"));
    }
    let button = |label: &str| -> Done<&Node> {
        let matches: Vec<_> = bar
            .children
            .iter()
            .filter(|n| {
                matches!(n.role.as_str(), "AXCheckBox" | "AXButton")
                    && (ctx.project.strings.is(&n.desc, label) || ctx.project.strings.is(&n.title, label))
            })
            .collect();
        match matches.as_slice() {
            [node] => Ok(*node),
            _ => Err(unsupported("A required control bar button isn't uniquely readable")),
        }
    };
    for label in ["Play", "Record"] {
        let node = button(label)?;
        if flag(node) != Some(false) {
            return Err(unsupported("Playback and recording must be stopped"));
        }
    }
    let cycle = button("Cycle")?;
    let on = flag(cycle).ok_or_else(|| unsupported("The Cycle button's state is unreadable"))?;
    if cycle.enabled != Some(true) || cycle.path.is_empty() || !cycle.actions.iter().any(|a| a == "AXPress") {
        return Err(unsupported("The Cycle button isn't known writable"));
    }
    let list = button("List Editors")?;
    if flag(list) != Some(true) {
        return Err(unsupported("The Signature List must already be shown"));
    }
    let meter =
        paths::time_signature(lcd, &ctx.project.strings).map(Node::value_text).ok_or_else(|| unsupported("The LCD meter is unknown"))?;
    let numerator = meter
        .strip_suffix("/4")
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=16).contains(n))
        .ok_or_else(|| unsupported("The LCD meter isn't a supported N/4 signature"))?;
    let divisions: Vec<_> =
        lcd.children.iter().filter(|n| n.role == "AXPopUpButton" && n.value.as_str().is_some_and(|s| s.starts_with('/'))).collect();
    if divisions.len() != 1 || divisions[0].value.as_str() != Some("/16") {
        return Err(unsupported("The LCD division isn't known /16"));
    }
    let groups: Vec<_> = lcd
        .children
        .iter()
        .filter(|n| {
            n.role == "AXGroup"
                && !ctx.project.strings.is(&n.desc, "Playhead Position")
                && n.children.iter().any(|s| ctx.project.strings.is(&s.desc, "bar"))
        })
        .collect();
    if groups.len() != 2 {
        return Err(unsupported("The LCD doesn't show exactly two musical locators"));
    }
    let mut positions = vec![];
    let mut controls = vec![];
    let mut control_paths = vec![cycle.path.as_str()];
    for group in groups {
        let p = segments(group, numerator).ok_or_else(|| unsupported("A locator's four segments are incomplete or invalid"))?;
        if group.path.is_empty() {
            return Err(unsupported("A locator has no native path identity"));
        }
        for (index, (node, label)) in group.children.iter().zip(["bar", "beat", "division", "tick"]).enumerate() {
            if !ctx.project.strings.is(&node.desc, label)
                || node.enabled != Some(true)
                || node.path != crate::ax::join(&group.path, index)
                || control_paths.contains(&node.path.as_str())
                || !node.settable.iter().any(|a| a == "AXValue")
                || !node.actions.iter().any(|a| a == "AXIncrement")
                || !node.actions.iter().any(|a| a == "AXDecrement")
            {
                return Err(unsupported("A locator segment isn't identified and known writable"));
            }
            control_paths.push(&node.path);
        }
        positions.push(p);
        controls.push(group.children.iter().map(|n| Control::new(&ctx.project.root, &n.path)).collect());
    }
    if positions[0].beats(numerator) >= positions[1].beats(numerator) {
        return Err(unsupported("The locator order is invalid"));
    }
    let timebase =
        json!({"project": identity, "meter": [numerator, 4], "division": 16, "ticksPerBeat": 960, "signature": signature(ctx, numerator)?});
    if project(ctx)? != timebase["project"] {
        return Err(changed("the saved project changed during the cycle read"));
    }
    Ok(Seen {
        state: state(on, positions[0], positions[1], timebase),
        locators: controls.try_into().unwrap(),
        cycle: Control::new(&ctx.project.root, &cycle.path),
    })
}

fn after_input(input: &Value, now: &Value) -> Done<Value> {
    let (on, start, end) = wanted(input)?;
    let (left, right, numerator) = valid_state(now).ok_or_else(|| failed(LEGACY))?;
    let (left, right) = if on { (Position::from_beats(start, numerator), Position::from_beats(end, numerator)) } else { (left, right) };
    Ok(state(on, left, right, now["timebase"].clone()))
}

fn preflight(now: &Value, to: &Value) -> Done<()> {
    let (left, right, numerator) = valid_state(now).ok_or_else(|| failed(LEGACY))?;
    let (to_left, to_right, _) = valid_state(to).ok_or_else(|| failed(LEGACY))?;
    if now["timebase"] != to["timebase"] {
        return Err(changed("the cycle's project or timebase changed"));
    }
    let parts = |p: Position| [p.bar, p.beat, p.division, p.tick];
    let mut steps = u64::from(now["on"] != to["on"]);
    for (from, to) in [(left, to_left), (right, to_right)] {
        let mut higher_changed = false;
        for (from, to) in parts(from).into_iter().zip(parts(to)) {
            let cost = |distance: u32| u64::from(distance / 10 + distance % 10);
            steps += cost(from.abs_diff(to)).max(if higher_changed { cost(1u32.abs_diff(to)) } else { 0 });
            higher_changed |= from != to;
        }
    }
    if steps > MAX_STEPS {
        return Err(bad("the cycle edit exceeds the bounded locator step count; choose a nearer range"));
    }
    // Higher segments may reset any lower segment to one. Prove that every such known behavior preserves the
    // locator order before starting, rather than discovering an unsupported crossing after a partial edit. Within
    // one segment the numeric motion is monotonic, so its endpoint bounds cover the intermediate steps too.
    let targets = [parts(to_left), parts(to_right)];
    let order = if to_right.beats(numerator) >= right.beats(numerator) { [1, 0] } else { [0, 1] };
    let mut possible = vec![[parts(left), parts(right)]];
    for index in order {
        for segment in 0..4 {
            let mut next = vec![];
            for mut pair in possible {
                if pair[index][segment] == targets[index][segment] {
                    if !next.contains(&pair) {
                        next.push(pair);
                    }
                    continue;
                }
                pair[index][segment] = targets[index][segment];
                for reset in 0..(1 << (3 - segment)) {
                    let mut branch = pair;
                    for lower in segment + 1..4 {
                        if reset & (1 << (lower - segment - 1)) != 0 {
                            branch[index][lower] = 1;
                        }
                    }
                    let start = position(&branch[0], numerator).unwrap();
                    let end = position(&branch[1], numerator).unwrap();
                    if start.beats(numerator) >= end.beats(numerator) {
                        return Err(bad("this locator transition could cross the other locator while a higher segment resets lower fields; set this range manually"));
                    }
                    if !next.contains(&branch) {
                        next.push(branch);
                    }
                }
            }
            possible = next;
        }
    }
    Ok(())
}

/// A fresh fence before every dispatch, plus complete readback afterwards. A lost or malformed reply returns
/// immediately; there is no retry, rollback, pane cleanup or compensating write after an uncertain operation.
fn write(ctx: &mut Ctx, mut seen: Seen, to: &Value) -> Done<Value> {
    preflight(&seen.state, to)?;
    let order = if to["endBeats"].as_f64() >= seen.state["endBeats"].as_f64() { [1, 0] } else { [0, 1] };
    let mut dispatched = false;
    let mut steps = 0;
    let lift = |fault: crate::refs::Fault, wrote: bool| if wrote { uncertain(fault.1) } else { fault };
    for index in order {
        let key = ["start", "end"][index];
        for segment in 0..4 {
            let field = ["bar", "beat", "division", "tick"][segment];
            while seen.state[key][field] != to[key][field] {
                let fresh = observe(ctx).map_err(|f| lift(f, dispatched))?;
                if fresh.state != seen.state {
                    return Err(lift(changed("the cycle changed before the next locator step"), dispatched));
                }
                seen = fresh;
                steps += 1;
                if steps > MAX_STEPS {
                    return Err(lift(bad("the cycle exceeded its bounded step count"), dispatched));
                }
                let now = seen.state[key][field].as_i64().unwrap();
                let target = to[key][field].as_i64().unwrap();
                let big = (target - now).abs() >= nudge::BIG_STEP;
                let expected = now + (target - now).signum() * if big { nudge::BIG_STEP } else { 1 };
                let control = &seen.locators[index][segment];
                dispatched = true;
                let reply = if big {
                    ctx.ax.action(
                        &control.root,
                        &control.path,
                        if target > now { "AXIncrement" } else { "AXDecrement" },
                        nudge::STEP_SETTLE_MS,
                    )
                } else {
                    ctx.ax.set(&control.root, &control.path, "AXValue", json!(target), nudge::STEP_SETTLE_MS)
                };
                let reply = nudge::wrote(reply)?;
                if reply["after"].as_f64() != Some(expected as f64) {
                    return Err(uncertain("Logic Pro didn't confirm the locator step; no further action was sent"));
                }
                let after = observe(ctx).map_err(|f| uncertain(f.1))?;
                let mut expected_state = seen.state.clone();
                expected_state[key][field] = expected.into();
                // A higher segment may reset lower segments to one. No unrelated value may change.
                for lower in segment + 1..4 {
                    let field = ["bar", "beat", "division", "tick"][lower];
                    if after.state[key][field] == 1 {
                        expected_state[key][field] = 1.into();
                    }
                }
                let numerator = expected_state["timebase"]["meter"][0].as_u64().unwrap() as u32;
                let p = raw_position(&expected_state[key], numerator).unwrap();
                expected_state[["startBeats", "endBeats"][index]] = p.beats(numerator).into();
                if after.state != expected_state {
                    return Err(uncertain("The cycle or its project/timebase changed after a locator step; no further action was sent"));
                }
                seen = after;
            }
        }
    }
    if seen.state["on"] != to["on"] {
        let fresh = observe(ctx).map_err(|f| lift(f, dispatched))?;
        if fresh.state != seen.state {
            return Err(lift(changed("the cycle changed before its button could be pressed"), dispatched));
        }
        let reply = nudge::wrote(ctx.ax.action(&fresh.cycle.root, &fresh.cycle.path, "AXPress", 150))?;
        if flag(&Node { value: reply["after"].clone(), ..Node::default() }) != to["on"].as_bool() {
            return Err(uncertain("Logic Pro didn't confirm its Cycle button; no further action was sent"));
        }
        seen = observe(ctx).map_err(|f| uncertain(f.1))?;
    }
    if seen.state != *to {
        return Err(uncertain("Logic Pro's complete cycle readback differs from the requested state"));
    }
    Ok(seen.state)
}

fn public(state: &Value) -> Value {
    json!({"on": state["on"], "startBeats": state["startBeats"], "endBeats": state["endBeats"]})
}
fn title(state: &Value) -> String {
    if state["on"] == false {
        "Cycle off".into()
    } else {
        let (start, end, _) = valid_state(state).unwrap();
        format!("Cycle {} to {}", timeline::bar_text(&start), timeline::bar_text(&end))
    }
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    wanted(input)?;
    let now = observe(ctx)?.state;
    let after = after_input(input, &now)?;
    preflight(&now, &after)?;
    let mut display = json!({"title": title(&after), "before": public(&now), "after": public(&after)});
    if now == after {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    wanted(input)?;
    valid_state(fence).ok_or_else(|| failed(LEGACY))?;
    let seen = observe(ctx)?;
    if seen.state != *fence {
        return Err(changed("the cycle, saved project or timebase changed since the preview"));
    }
    let now = seen.state.clone();
    let wanted = after_input(input, &now)?;
    let after = write(ctx, seen, &wanted)?;
    Ok(Applied {
        applied: after.clone(),
        material: Some(json!({"undo": {"mode": "value"}})),
        display: json!({"title": title(&after), "before": public(&now), "after": public(&after)}),
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    valid_state(restore.prior).ok_or_else(|| failed(LEGACY))?;
    valid_state(restore.applied).ok_or_else(|| failed(LEGACY))?;
    if restore.prior["timebase"] != restore.applied["timebase"] {
        return Err(failed(LEGACY));
    }
    let seen = observe(ctx)?;
    let now = seen.state.clone();
    if now["timebase"] != restore.prior["timebase"] {
        return Err(changed("the saved project or musical timebase changed; restore the cycle manually"));
    }
    let after = if now == *restore.prior {
        now.clone()
    } else {
        if now != *restore.applied {
            return Err(changed("the complete cycle changed since Kumi set it; restore it manually"));
        }
        write(ctx, seen, restore.prior)?
    };
    Ok(Restored::by_value(
        after.clone(),
        json!({"title": format!("{} (put back)", title(&after)), "before": public(&now), "after": public(&after)}),
    ))
}

#[cfg(test)]
#[path = "cycle_tests.rs"]
mod tests;
