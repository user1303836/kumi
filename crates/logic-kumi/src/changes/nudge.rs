//! Moving Logic's controls through Accessibility, as measured on Logic Pro 12.3.1: a slider or a scrubber's segment
//! takes one step toward the value a set asks for, whatever the value, and ten steps on `AXIncrement` or
//! `AXDecrement`; a checkbox or switch toggles on `AXPress`; a text field takes a value and commits it on `AXConfirm`.
//! So an exact value is reached by tens, then by ones, with the control read back after each step, and a value that
//! only shows as text (a fader's dB) is stepped toward until the text says it.

use serde_json::{json, Value};

use crate::ax::{Ax, AxError, Root};
use crate::changes::Done;

/// How long the helper waits after a step before it reads the control back.
pub const STEP_SETTLE_MS: u64 = 40;
/// How long a control that didn't show its step gets before it is read once more.
pub const LAG_MS: u64 = 80;
/// What one increment or decrement moves a control by.
pub const BIG_STEP: i64 = 10;
/// The most steps one value is reached in: a fader runs 0 to 233 (24 tens and 9 ones), a tick segment to 239.
pub const MOST_STEPS: usize = 64;

/// A control to step, below a root.
#[derive(Debug, Clone)]
pub struct Control {
    pub root: Root,
    pub path: String,
}

impl Control {
    pub fn new(root: &Root, path: &str) -> Control {
        Control { root: root.clone(), path: path.to_string() }
    }
}

/// The value a `set` or `action` reply shows after the step, as a number.
fn after(reply: &Value) -> Option<i64> {
    reply.get("after").and_then(Value::as_f64).map(|n| n.round() as i64)
}

/// One step of a control toward `toward`: by ten when `big`, else by one. The value after it, as the helper read it
/// around the step; none when it couldn't (the control went away or was rebuilt).
pub fn step(ax: &mut Ax, control: &Control, now: i64, toward: i64, big: bool) -> Result<Option<i64>, AxError> {
    let reply = if big {
        let action = if toward > now { "AXIncrement" } else { "AXDecrement" };
        ax.action(&control.root, &control.path, action, STEP_SETTLE_MS)?
    } else {
        ax.set(&control.root, &control.path, "AXValue", json!(toward), STEP_SETTLE_MS)?
    };
    Ok(after(&reply))
}

/// A control's value as it reads at its path now, if it can be read at all.
fn value_at(ax: &mut Ax, control: &Control) -> Option<i64> {
    let attrs = ax.get(&control.root, &control.path, &["AXValue"]).ok()?;
    attrs.get("AXValue").and_then(Value::as_f64).map(|n| n.round() as i64)
}

/// [`step_to`] for a control that every step rebuilds (a cell of the Tempo or Signature list): the helper can't
/// read it back around a step, so it is read again at its path after each one, once more after a moment when it
/// still shows the old value. The value reached.
pub fn step_to_rebuilt(ax: &mut Ax, control: &Control, now: i64, target: i64) -> Result<i64, AxError> {
    let mut now = now;
    for _ in 0..MOST_STEPS {
        if now == target {
            return Ok(now);
        }
        let big = (target - now).abs() >= BIG_STEP;
        let mut moved = match step(ax, control, now, target, big)? {
            Some(moved) => Some(moved),
            None => value_at(ax, control),
        };
        if moved == Some(now) {
            std::thread::sleep(std::time::Duration::from_millis(LAG_MS));
            moved = value_at(ax, control);
        }
        match moved {
            Some(moved) if moved != now => now = moved,
            _ => return Ok(now),
        }
    }
    Ok(now)
}

/// Moves a whole-number control from `now` to `target`, by tens then ones: the value reached, which is `target`
/// unless the control stopped moving (its end, or a value Logic refuses).
pub fn step_to(ax: &mut Ax, control: &Control, now: i64, target: i64) -> Result<i64, AxError> {
    let mut now = now;
    for _ in 0..MOST_STEPS {
        if now == target {
            return Ok(now);
        }
        let big = (target - now).abs() >= BIG_STEP;
        let Some(moved) = step(ax, control, now, target, big)? else { return Ok(now) };
        if moved == now {
            // A big step past the end lands where a small one would: finish by ones.
            if big {
                let Some(moved) = step(ax, control, now, target, false)? else { return Ok(now) };
                if moved == now {
                    return Ok(now);
                }
                now = moved;
                continue;
            }
            return Ok(now);
        }
        now = moved;
    }
    Ok(now)
}

/// Where a control that only shows its value as text stands: the reading, and the raw value it is at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub value: f64,
    pub raw: i64,
}

/// Moves a control toward a value its text shows (a fader's dB, a velocity): tens while the reading is more than
/// `coarse` away, then ones until it is within `close` or crosses the target, keeping the nearer side. `read` gives
/// the reading now. A small step asks for the raw value `small` away: one for a fader, and for a control whose raw
/// value packs its reading with other fields (Logic's velocity: 2^25 a step, the low bits its own) exactly one step,
/// so nothing else of it moves. The reading reached is returned.
pub fn step_reading(
    ax: &mut Ax,
    control: &Control,
    target: f64,
    coarse: f64,
    close: f64,
    small: i64,
    mut read: impl FnMut(&mut Ax) -> Result<Reading, AxError>,
) -> Result<Reading, AxError> {
    let mut now = read(ax)?;
    for _ in 0..MOST_STEPS {
        let away = target - now.value;
        if away.abs() <= close {
            return Ok(now);
        }
        let big = away.abs() > coarse;
        let up = away > 0.0;
        let toward = if big { now.raw + if up { BIG_STEP } else { -BIG_STEP } } else { now.raw + if up { small } else { -small } };
        let moved = step(ax, control, now.raw, toward, big)?;
        let mut next = read(ax)?;
        if next == now {
            // A list's cell shows its new value a moment after the step: one more look before it counts as the end.
            super::pause(std::time::Duration::from_millis(LAG_MS));
            next = read(ax)?;
        }
        if moved == Some(now.raw) && next == now {
            // The end of the control.
            return Ok(now);
        }
        // Crossed the target by a small step: the nearer of the two sides wins.
        if !big && (target - next.value).signum() != away.signum() {
            if (target - next.value).abs() > away.abs() {
                let _ = step(ax, control, next.raw, now.raw, false)?;
                return read(ax);
            }
            return Ok(next);
        }
        now = next;
    }
    Ok(now)
}

/// Presses a toggle (a header's checkbox, a strip's switch) and reads whether it is on after.
pub fn press(ax: &mut Ax, control: &Control, settle_ms: u64) -> Result<Value, AxError> {
    let reply = ax.action(&control.root, &control.path, "AXPress", settle_ms)?;
    Ok(reply.get("after").cloned().unwrap_or(Value::Null))
}

/// Sets a text field and commits it with `AXConfirm`, as Logic's name fields need.
pub fn confirm_text(ax: &mut Ax, control: &Control, text: &str, settle_ms: u64) -> Result<(), AxError> {
    ax.set(&control.root, &control.path, "AXValue", json!(text), STEP_SETTLE_MS)?;
    ax.action(&control.root, &control.path, "AXConfirm", settle_ms)?;
    Ok(())
}

/// Whether a value read back is "on": a number other than 0, `true`, or the text `on`.
pub fn is_on(value: &Value) -> bool {
    crate::ax::Node { value: value.clone(), ..Default::default() }.is_on()
}

/// A write that Logic did not take as asked: the control reads `shown` where `wanted` was asked.
pub fn kept(what: &str, wanted: impl std::fmt::Display, shown: impl std::fmt::Display) -> crate::refs::Fault {
    super::uncertain(format!("Logic Pro shows {what} as {shown} after Kumi asked for {wanted}"))
}

/// A `Done` from an Accessibility result inside a write: a failure after something was sent is `uncertain`.
pub fn wrote<T>(result: Result<T, AxError>) -> Done<T> {
    result.map_err(|error| super::ax_fault(&error, true))
}

/// A `Done` from an Accessibility result before anything was written.
pub fn read<T>(result: Result<T, AxError>) -> Done<T> {
    result.map_err(|error| super::ax_fault(&error, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::{HelperError, Launch, MockHands};

    #[test]
    fn a_value_is_reached_by_tens_then_ones_and_a_control_that_stops_is_left_where_it_is() {
        let hands = MockHands::new();
        // 173 to 150: two decrements (163, 153), then three sets (152, 151, 150).
        for after in [163, 153] {
            hands.reply("action", Ok(json!({"before": after + 10, "after": after, "result": "success"})));
        }
        for after in [152, 151, 150] {
            hands.reply("set", Ok(json!({"before": after + 1, "after": after, "result": "success"})));
        }
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let control = Control::new(&Root::Main, "6.0.2.0.0.5");
        assert_eq!(step_to(&mut ax, &control, 173, 150).unwrap(), 150);
        let ops: Vec<String> = hands.calls().into_iter().map(|(op, _)| op).collect();
        assert_eq!(ops, ["action", "action", "set", "set", "set"]);
        let (_, params) = &hands.calls()[0];
        assert_eq!((params["name"].as_str(), params["path"].as_str()), (Some("AXDecrement"), Some("6.0.2.0.0.5")));
        assert_eq!(hands.calls()[2].1["value"], 150);
        // At its end a control doesn't move: the value stays where it is, after one small try.
        hands.reply("action", Ok(json!({"before": 2, "after": 2, "result": "success"})));
        hands.reply("set", Ok(json!({"before": 2, "after": 2, "result": "success"})));
        assert_eq!(step_to(&mut ax, &control, 2, 20).unwrap(), 2);
        assert_eq!(step_to(&mut ax, &control, 5, 5).unwrap(), 5, "nothing to do");
        hands.reply("set", Err(HelperError::Timeout));
        assert!(wrote(step_to(&mut ax, &control, 5, 6)).is_err());
    }

    #[test]
    fn a_reading_is_stepped_toward_by_tens_then_ones_and_the_nearer_side_of_the_target_is_kept() {
        let hands = MockHands::new();
        // A fader at 0.0 dB (raw 173) asked for −6 dB: the readings as Logic shows them along the way.
        let readings = std::sync::Arc::new(std::sync::Mutex::new(vec![
            (173, 0.0),
            (163, -1.0),
            (153, -2.1),
            (143, -3.3),
            (133, -4.6),
            (123, -5.9),
            (122, -6.15),
            (123, -5.9),
        ]));
        // Replies in the order the steps come: five decrements, one set down, one set back up.
        for raw in [163, 153, 143, 133, 123] {
            hands.reply("action", Ok(json!({"before": raw + 10, "after": raw, "result": "success"})));
        }
        hands.reply("set", Ok(json!({"before": 123, "after": 122, "result": "success"})));
        hands.reply("set", Ok(json!({"before": 122, "after": 123, "result": "success"})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let control = Control::new(&Root::Main, "7.1.0.1.0.0.0.7");
        let at = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let reached = step_reading(&mut ax, &control, -6.0, 1.2, 0.05, 1, |_| {
            let mut index = at.lock().unwrap();
            let list = readings.lock().unwrap();
            let (raw, value) = list[(*index).min(list.len() - 1)];
            *index += 1;
            Ok(Reading { raw, value })
        })
        .unwrap();
        // −5.9 is nearer −6 than −6.15 is: the fader comes back up one step.
        assert_eq!(reached, Reading { raw: 123, value: -5.9 });
        let ops: Vec<String> = hands.calls().into_iter().map(|(op, _)| op).collect();
        assert_eq!(ops, ["action", "action", "action", "action", "action", "set", "set"]);
    }

    #[test]
    fn toggles_and_text_fields_go_through_press_and_confirm() {
        let hands = MockHands::new();
        hands.reply("action", Ok(json!({"before": "off", "after": "on", "result": "success"})));
        hands.reply("set", Ok(json!({"before": "Vocal", "after": "Vox", "result": "success"})));
        hands.reply("action", Ok(json!({"before": "Vox", "after": "Vox", "result": "success"})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let on = press(&mut ax, &Control::new(&Root::Main, "1.2"), 50).unwrap();
        assert!(is_on(&on));
        assert!(!is_on(&json!("off")) && is_on(&json!(1)) && !is_on(&json!(0)) && is_on(&json!(true)));
        confirm_text(&mut ax, &Control::new(&Root::Main, "6.0.1.2"), "Vox", 100).unwrap();
        let calls = hands.calls();
        assert_eq!((calls[1].1["attr"].as_str(), calls[1].1["value"].as_str()), (Some("AXValue"), Some("Vox")));
        assert_eq!(calls[2].1["name"], "AXConfirm");
        assert_eq!(kept("the volume", "−6 dB", "0 dB").0, "uncertain");
    }
}
