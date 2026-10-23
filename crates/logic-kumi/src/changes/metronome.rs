//! `metronome-set`: the control bar's Metronome Click button, which turns Logic's click on and off. The button is read
//! before (what the preview fences), pressed only when it differs from what is asked, and read back: `applied` only when
//! the control bar shows the click as asked. Logic keeps no step of its undo for it (a setting of the project, as the
//! cycle is), so the restore presses it back by value. The listening passes (M5) turn the same button off and on.

use serde_json::{json, Value};

use super::nudge;
use super::{changed, timeline, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::refs::{bad, failed};

/// The click as the journal keeps it.
fn state(on: bool) -> Value {
    json!({"on": on})
}

fn wanted(input: &Value) -> Done<bool> {
    input.get("on").and_then(Value::as_bool).ok_or_else(|| bad("on must be true (the metronome's click on) or false (off)"))
}

fn title(on: bool) -> &'static str {
    if on {
        "Metronome on"
    } else {
        "Metronome off"
    }
}

/// Whether the click is on, as the control bar shows it; `failed` when the bar shows no Metronome Click button (the
/// producer took it off the control bar).
fn read(ctx: &mut Ctx) -> Done<bool> {
    let (bar, _) = timeline::control_bar(ctx)?;
    if !timeline::shows_metronome(&bar, &ctx.project.strings) {
        return Err(failed(
            "Logic Pro's control bar doesn't show its Metronome Click button (Customize Control Bar and Display can show it again)",
        ));
    }
    Ok(timeline::metronome_on(&bar, &ctx.project.strings))
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let on = wanted(input)?;
    let now = read(ctx)?;
    let mut display = json!({"title": title(on), "before": state(now), "after": state(on)});
    if now == on {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: state(now), fence: state(now), display })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let on = wanted(input)?;
    let now = read(ctx)?;
    if state(now) != *fence {
        return Err(changed("the metronome was turned on or off in Logic Pro since the preview"));
    }
    timeline::set_metronome(ctx, on)?;
    let after = read(ctx).map_err(|(_, why)| super::uncertain(why))?;
    if after != on {
        return Err(nudge::kept("the metronome", if on { "on" } else { "off" }, if after { "on" } else { "off" }));
    }
    Ok(Applied {
        applied: state(after),
        display: json!({"title": title(after), "before": state(now), "after": state(after)}),
        step: None,
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let prior = restore.prior.get("on").and_then(Value::as_bool).ok_or_else(|| bad("prior.on"))?;
    let applied = restore.applied.get("on").and_then(Value::as_bool);
    let now = read(ctx)?;
    if now != prior {
        if !restore.discard && applied.is_some_and(|applied| applied != now) {
            return Err(changed("the metronome has been turned on or off in Logic Pro since Kumi set it"));
        }
        timeline::set_metronome(ctx, prior)?;
        let after = read(ctx).map_err(|(_, why)| super::uncertain(why))?;
        if after != prior {
            return Err(nudge::kept("the metronome", if prior { "on" } else { "off" }, if after { "on" } else { "off" }));
        }
    }
    Ok(Restored::by_value(
        state(prior),
        json!({"title": format!("{} (put back)", title(prior)), "before": state(now), "after": state(prior)}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_click_is_asked_as_on_or_off() {
        assert!(wanted(&json!({"on": true})).unwrap());
        assert!(!wanted(&json!({"on": false})).unwrap());
        assert_eq!(wanted(&json!({"on": "yes"})).unwrap_err().0, "bad-params");
        assert_eq!(wanted(&json!({})).unwrap_err().0, "bad-params");
        assert_eq!((title(true), title(false)), ("Metronome on", "Metronome off"));
        assert_eq!(state(true), json!({"on": true}));
    }
}
