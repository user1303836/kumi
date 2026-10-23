//! `loop-range`: the cycle (Logic's loop) through the control bar. The Cycle button turns it on and off, and the LCD's
//! Custom display shows the two locators as four-segment scrubbers (bar, beat, division, tick), each segment a slider
//! that takes a set: the display is switched to Custom through its Display Mode pop-up (whose menu Accessibility
//! lists once the pop-up is pressed), the locators read or stepped, and the display put back as it was. The ruler's
//! cycle area says only pixels, and Set Left/Right Locator by Playhead are unbound in Logic's factory set, so the LCD
//! is the one channel that reads back exactly. Logic keeps no undo step for the locators or the Cycle button.

use std::time::Duration;

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::{changed, timeline, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::ax::paths;
use crate::refs::{bad, failed};
use crate::rows::Position;

/// The LCD display that shows the locators, and how long the control bar gets to redraw after a switch.
const CUSTOM: &str = "Custom";
const SWITCH_SETTLE_MS: u64 = 400;
const SWITCH_SETTLE: Duration = Duration::from_millis(300);

/// The locators and whether the cycle is on, as the journal keeps them.
fn state(on: bool, left: &Position, right: &Position, beats_per_bar: u32) -> Value {
    json!({"on": on, "startBeats": left.beats(beats_per_bar), "endBeats": right.beats(beats_per_bar)})
}

fn number(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(f64::NAN)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0 / 960.0
}

fn same(a: &Value, b: &Value) -> bool {
    a["on"] == b["on"] && close(number(a, "startBeats"), number(b, "startBeats")) && close(number(a, "endBeats"), number(b, "endBeats"))
}

fn wanted(input: &Value) -> Done<(bool, f64, f64)> {
    let number = |key: &str| -> Done<Option<f64>> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                .map(Some)
                .ok_or_else(|| bad(format!("{key} must be a number from 0: beats from the project's start"))),
        }
    };
    if (input.get("start").is_some_and(|v| !v.is_null()) || input.get("end").is_some_and(|v| !v.is_null()))
        && input.get("startBeats").is_none_or(Value::is_null)
    {
        return Err(bad("Logic Pro's bridge places the cycle by startBeats and endBeats (beats from the project's start), not seconds"));
    }
    let start = number("startBeats")?
        .ok_or_else(|| bad("pass startBeats (beats from the project's start) and endBeats; 0 and 0 turn the cycle off"))?;
    let end = number("endBeats")?.ok_or_else(|| bad("pass endBeats"))?;
    if start == 0.0 && end == 0.0 {
        return Ok((false, 0.0, 0.0));
    }
    if end <= start {
        return Err(bad("the cycle must end after it starts (0 and 0 turn it off)"));
    }
    Ok((true, start, end))
}

fn title(state: &Value, beats_per_bar: u32) -> String {
    if state["on"] == false {
        return "Cycle off".into();
    }
    format!("Cycle {}", range_text(state, beats_per_bar))
}

fn range_text(state: &Value, beats_per_bar: u32) -> String {
    format!(
        "{} to {}",
        timeline::bar_text(&Position::from_beats(number(state, "startBeats"), beats_per_bar)),
        timeline::bar_text(&Position::from_beats(number(state, "endBeats"), beats_per_bar))
    )
}

/// The state in words for a refusal: an off cycle still has its locators.
fn describe(state: &Value, beats_per_bar: u32) -> String {
    if state["on"] == false {
        return format!("Cycle off, locators {}", range_text(state, beats_per_bar));
    }
    title(state, beats_per_bar)
}

// ----- the LCD -----

/// The LCD's Display Mode pop-up: its control and the mode shown.
fn display_mode(ctx: &mut Ctx) -> Done<(Control, String)> {
    let (bar, _) = timeline::control_bar(ctx)?;
    let strings = &ctx.project.strings;
    let lcd = paths::lcd(&bar, strings).ok_or_else(|| failed("Logic Pro's control bar doesn't show its LCD"))?;
    let popup = lcd
        .children
        .iter()
        .find(|n| n.role == "AXPopUpButton" && strings.is(&n.desc, "Display Mode"))
        .ok_or_else(|| failed("Logic Pro's LCD shows no Display Mode pop-up"))?;
    Ok((Control::new(&ctx.project.root, &popup.path), popup.value_text()))
}

/// Switches the LCD to a display mode through the pop-up's menu: pressed open, its item pressed.
fn switch_display(ctx: &mut Ctx, mode: &str) -> Done<()> {
    let (popup, shown) = display_mode(ctx)?;
    if shown == mode {
        return Ok(());
    }
    nudge::wrote(ctx.ax.action(&popup.root, &popup.path, "AXPress", SWITCH_SETTLE_MS))?;
    let menu = nudge::wrote(ctx.ax.tree_in_popup(&popup.root, &popup.path, 2, 40, Some(&["role", "title"])))?;
    let strings = &ctx.project.strings;
    let item = menu.find(&|n| n.role == "AXMenuItem" && strings.is(&n.title, mode));
    let Some(item) = item else {
        // The menu open with nothing to choose: closed again, so Logic takes input.
        let _ = ctx.ax.action_in_popup(&popup.root, &format!("{}.0", popup.path), "AXCancel", 100);
        return Err(failed(format!("Logic Pro's LCD has no {mode} display")));
    };
    nudge::wrote(ctx.ax.action_in_popup(&popup.root, &item.path, "AXPress", SWITCH_SETTLE_MS))?;
    super::pause(SWITCH_SETTLE);
    let (_, now) = display_mode(ctx).map_err(|(_, why)| super::uncertain(why))?;
    if now != mode {
        return Err(super::uncertain(format!("Logic Pro's LCD shows {now:?} after Kumi asked for {mode:?}")));
    }
    Ok(())
}

/// The locator groups of the Custom LCD: the groups of bar, beat, division and tick sliders that aren't the
/// playhead's, left then right, with their sliders' paths.
fn locator_groups(ctx: &mut Ctx) -> Done<Vec<Vec<Control>>> {
    let (bar, _) = timeline::control_bar(ctx)?;
    let strings = &ctx.project.strings;
    let lcd = paths::lcd(&bar, strings).ok_or_else(|| failed("Logic Pro's control bar doesn't show its LCD"))?;
    let groups: Vec<Vec<Control>> = lcd
        .children
        .iter()
        .filter(|n| n.role == "AXGroup" && !strings.is(&n.desc, "Playhead Position"))
        .filter(|n| {
            n.children.iter().filter(|c| c.role == "AXSlider").count() >= 2 && n.children.iter().any(|c| strings.is(&c.desc, "bar"))
        })
        .map(|n| n.children.iter().filter(|c| c.role == "AXSlider").map(|c| Control::new(&ctx.project.root, &c.path)).collect())
        .collect();
    if groups.len() < 2 {
        return Err(failed("Logic Pro's Custom LCD doesn't show the locators (Customize Control Bar and Display: Locators)"));
    }
    Ok(groups)
}

/// A locator as its group's sliders read.
fn read_locator(ctx: &mut Ctx, group: &[Control]) -> Done<Position> {
    let mut values = [1u32; 4];
    for (slot, control) in group.iter().take(4).enumerate() {
        let attrs = nudge::read(ctx.ax.get(&control.root, &control.path, &["AXValue"]))?;
        values[slot] = attrs.get("AXValue").and_then(Value::as_f64).unwrap_or(1.0).max(0.0) as u32;
    }
    Ok(Position { bar: values[0].max(1), beat: values[1].max(1), division: values[2].max(1), tick: values[3].max(1) })
}

/// Steps a locator's sliders to a position, the bar first (a bar change may reset the beat), reading back after.
fn write_locator(ctx: &mut Ctx, group: &[Control], to: Position) -> Done<Position> {
    let targets = [to.bar, to.beat, to.division, to.tick];
    for (slot, control) in group.iter().take(4).enumerate() {
        let now = read_locator(ctx, group)?;
        let current = [now.bar, now.beat, now.division, now.tick][slot];
        if current != targets[slot] {
            nudge::wrote(nudge::step_to(ctx.ax, control, i64::from(current), i64::from(targets[slot])))?;
        }
    }
    read_locator(ctx, group).map_err(|(_, why)| super::uncertain(why))
}

/// The control bar's Cycle button, pressed until it reads `on`; whether it is on after.
fn put_cycle(ctx: &mut Ctx, on: bool) -> Done<bool> {
    let (bar, transport) = timeline::control_bar(ctx)?;
    if transport.cycle == on {
        return Ok(on);
    }
    let button = paths::transport_button(&bar, &ctx.project.strings, "Cycle")
        .ok_or_else(|| failed("Logic Pro's control bar doesn't show its Cycle button"))?;
    let control = Control::new(&ctx.project.root, &button.path);
    nudge::wrote(nudge::press(ctx.ax, &control, SWITCH_SETTLE_MS))?;
    let (_, after) = timeline::control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
    Ok(after.cycle)
}

/// The Cycle button turned on or off (its locators left as they are), for a listening pass that plays through; whether
/// it is on after.
pub(crate) fn set_cycle(ctx: &mut Ctx, on: bool) -> Done<bool> {
    put_cycle(ctx, on)
}

/// Reads the cycle (on, left, right) with the LCD on Custom, and puts the display back.
fn read_cycle(ctx: &mut Ctx) -> Done<Value> {
    let (_, shown) = display_mode(ctx)?;
    switch_display(ctx, CUSTOM)?;
    let result = (|| {
        let (_, transport) = timeline::control_bar(ctx)?;
        let groups = locator_groups(ctx)?;
        let (left, right) = (read_locator(ctx, &groups[0])?, read_locator(ctx, &groups[1])?);
        Ok(state(transport.cycle, &left, &right, transport.beats_per_bar()))
    })();
    if shown != CUSTOM {
        let _ = switch_display(ctx, &shown);
    }
    result
}

/// Writes the cycle: the locators with the LCD on Custom (whenever a range is given, on or off: the locators
/// stay visible with the cycle off and a restore puts them back too), the Cycle button, the display put back; the
/// cycle as read after.
fn write_cycle(ctx: &mut Ctx, on: bool, start: f64, end: f64) -> Done<Value> {
    let (_, shown) = display_mode(ctx)?;
    switch_display(ctx, CUSTOM)?;
    let result = (|| {
        let (_, transport) = timeline::control_bar(ctx)?;
        let beats_per_bar = transport.beats_per_bar();
        let groups = locator_groups(ctx)?;
        if end > start {
            let (left, right) = (Position::from_beats(start, beats_per_bar), Position::from_beats(end, beats_per_bar));
            // The right locator first when the range moves later, so the left never passes it.
            let now_right = read_locator(ctx, &groups[1])?;
            if right.beats(beats_per_bar) >= now_right.beats(beats_per_bar) {
                write_locator(ctx, &groups[1], right)?;
                write_locator(ctx, &groups[0], left)?;
            } else {
                write_locator(ctx, &groups[0], left)?;
                write_locator(ctx, &groups[1], right)?;
            }
        }
        let cycle = put_cycle(ctx, on)?;
        let (left, right) = (read_locator(ctx, &groups[0])?, read_locator(ctx, &groups[1])?);
        Ok(state(cycle, &left, &right, beats_per_bar))
    })();
    if shown != CUSTOM {
        let _ = switch_display(ctx, &shown);
    }
    result
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (on, start, end) = wanted(input)?;
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let now = read_cycle(ctx)?;
    let after = if on {
        json!({"on": true, "startBeats": start, "endBeats": end})
    } else {
        json!({"on": false, "startBeats": now["startBeats"], "endBeats": now["endBeats"]})
    };
    let mut display = json!({"title": title(&after, beats_per_bar), "before": now, "after": after});
    if same(&now, &after) {
        display["unchanged"] = true.into();
    }
    if on {
        let (left, right) = (Position::from_beats(start, beats_per_bar), Position::from_beats(end, beats_per_bar));
        if left.beats(beats_per_bar) != start || right.beats(beats_per_bar) != end {
            display["note"] = "The locators sit on Logic Pro's ticks (960 to a beat): the range is rounded to them.".into();
        }
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (on, start, end) = wanted(input)?;
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let now = read_cycle(ctx)?;
    if !same(&now, fence) {
        return Err(changed("the cycle changed since the preview"));
    }
    let after_wanted = if on {
        json!({"on": true, "startBeats": start, "endBeats": end})
    } else {
        json!({"on": false, "startBeats": now["startBeats"], "endBeats": now["endBeats"]})
    };
    let after = if same(&now, &after_wanted) { now.clone() } else { write_cycle(ctx, on, start, end)? };
    if !same(&after, &after_wanted) {
        return Err(nudge::kept("the cycle", describe(&after_wanted, beats_per_bar), describe(&after, beats_per_bar)));
    }
    Ok(Applied {
        applied: after.clone(),
        display: json!({"title": title(&after, beats_per_bar), "before": now, "after": after}),
        step: None,
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let beats_per_bar = timeline::control_bar(ctx)?.1.beats_per_bar();
    let now = read_cycle(ctx)?;
    let after = if same(&now, restore.prior) {
        now.clone()
    } else if restore.discard || same(&now, restore.applied) {
        let on = restore.prior["on"] == true;
        let written = write_cycle(ctx, on, number(restore.prior, "startBeats"), number(restore.prior, "endBeats"))?;
        if !same(&written, restore.prior) {
            return Err(nudge::kept("the cycle", describe(restore.prior, beats_per_bar), describe(&written, beats_per_bar)));
        }
        written
    } else {
        return Err(changed("the cycle has been changed in Logic Pro since Kumi set it"));
    };
    Ok(Restored::by_value(
        after.clone(),
        json!({"title": format!("{} (put back)", title(&after, beats_per_bar)), "before": now, "after": after}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cycle_is_asked_in_beats_and_zero_turns_it_off() {
        assert_eq!(wanted(&json!({"startBeats": 16, "endBeats": 32})).unwrap(), (true, 16.0, 32.0));
        assert_eq!(wanted(&json!({"startBeats": 0, "endBeats": 0})).unwrap(), (false, 0.0, 0.0));
        assert!(wanted(&json!({"startBeats": 8, "endBeats": 8})).is_err());
        assert!(wanted(&json!({"start": 1.0, "end": 2.0})).is_err());
        assert!(wanted(&json!({})).is_err());
        let on = state(true, &Position { bar: 5, ..Position::START }, &Position { bar: 9, ..Position::START }, 4);
        assert_eq!(on, json!({"on": true, "startBeats": 16.0, "endBeats": 32.0}));
        assert_eq!(title(&on, 4), "Cycle bar 5 to bar 9");
        assert_eq!(title(&json!({"on": false}), 4), "Cycle off");
        assert!(same(&on, &json!({"on": true, "startBeats": 16.0, "endBeats": 32.0})));
        assert!(!same(&on, &json!({"on": false, "startBeats": 16.0, "endBeats": 32.0})));
    }
}
