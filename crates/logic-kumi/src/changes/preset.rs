//! `fx-preset`: a plug-in's preset (its setting) changed through its window's header ([`super::window`]), in its
//! editor (the Controls View's header has no undo): `step` presses the next or previous setting button that many
//! times, `preset` presses next until the setting menu names the preset asked (Logic's setting menu is its own, which
//! Accessibility can't read, so the buttons are the way through). Read back from the setting pop-up's name. Each press
//! is a step of the plug-in window's own undo, never of the project's, so the restore presses the window's Undo as
//! many times, while the plug-in still shows the preset Kumi left: that brings back "Default Preset" too, which no
//! setting button reaches (measured on Logic Pro 12.3.1: next from "Default Preset" is the first preset, previous from
//! there stays on it).

use serde_json::{json, Value};

use super::strips;
use super::window::{self, Shown};
use super::{changed, gone, quoted, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::refs::{bad, failed};

/// The most presses one change makes, and a step may ask.
const MOST: usize = 64;

/// What a request asks: a number of presses (next when above 0), or a preset by its name.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    Step(i64),
    Named(String),
}

fn ask_of(input: &Value) -> Done<Ask> {
    match (input.get("preset").filter(|v| !v.is_null()), input.get("step").filter(|v| !v.is_null())) {
        (Some(Value::String(name)), None) if !name.trim().is_empty() => Ok(Ask::Named(name.trim().to_string())),
        (None, Some(step)) => step.as_i64().filter(|n| *n != 0 && n.unsigned_abs() as usize <= MOST).map(Ask::Step).ok_or_else(|| {
            bad(format!("step must be a whole number from -{MOST} to {MOST}, not 0: the next (above 0) or previous settings"))
        }),
        _ => {
            Err(bad("give preset (a setting's name as the plug-in window's setting menu shows it) or step (+1 the next, -1 the previous)"))
        }
    }
}

/// Presses next or previous once and reads the setting after.
fn press(ctx: &mut Ctx, shown: &Shown, next: bool) -> Done<String> {
    let label = if next { "next" } else { "previous" };
    window::reveal(ctx, shown, label)?;
    window::press(ctx, shown, label)?;
    let (_, now) = window::preset(ctx, shown).map_err(|(_, why)| super::uncertain(why))?;
    Ok(now)
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (owner, slot) = strips::plugin(ctx, input, "ref", Epoch::Current)?;
    let ask = ask_of(input)?;
    let shown = window::show_in(ctx, owner, slot, window::View::Editor)?;
    let result = (|| {
        let (_, now) = window::preset(ctx, &shown)?;
        let what = match &ask {
            Ask::Named(name) => quoted(name),
            Ask::Step(1) => "the next setting".into(),
            Ask::Step(-1) => "the previous setting".into(),
            Ask::Step(n) if *n > 0 => format!("the setting {n} on"),
            Ask::Step(n) => format!("the setting {} back", -n),
        };
        let mut display = json!({"title": format!("{} on {}: {what}", quoted(&shown.plugin), quoted(&shown.strip)), "before": now});
        if matches!(&ask, Ask::Named(name) if *name == now) {
            display["unchanged"] = true.into();
        }
        let state = json!({"plugin": shown.plugin, "preset": now});
        Ok(Preview { target: strips::plugin_target(ctx.project.epoch, owner, slot), prior: state.clone(), fence: state, display })
    })();
    window::hide(ctx, shown);
    result
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (owner, slot) = strips::plugin(ctx, input, "ref", Epoch::Current).map_err(strips::went)?;
    let ask = ask_of(input)?;
    let shown = window::show_in(ctx, owner, slot, window::View::Editor)?;
    let result = (|| {
        let (_, before) = window::preset(ctx, &shown)?;
        if json!({"plugin": shown.plugin, "preset": before}) != *fence {
            return Err(changed(format!("{}'s setting changed in Logic Pro since the preview", quoted(&shown.plugin))));
        }
        let mut now = before.clone();
        let mut steps = 0usize;
        match &ask {
            Ask::Named(name) if *name == before => {}
            Ask::Named(name) => {
                while now != *name {
                    if steps >= MOST {
                        undo_presses(ctx, &shown, steps);
                        return Err(failed(format!(
                            "{} isn't among the next {MOST} settings of {} (its setting menu, which Kumi can't read, lists them all)",
                            quoted(name),
                            quoted(&shown.plugin)
                        )));
                    }
                    let was = now.clone();
                    now = press(ctx, &shown, true)?;
                    steps += 1;
                    if now == was {
                        undo_presses(ctx, &shown, steps);
                        return Err(super::uncertain(format!(
                            "{}'s setting stayed {} after Kumi pressed next",
                            quoted(&shown.plugin),
                            quoted(&was)
                        )));
                    }
                }
            }
            Ask::Step(n) => {
                for _ in 0..n.unsigned_abs() {
                    now = press(ctx, &shown, *n > 0)?;
                    steps += 1;
                }
                if now == before {
                    return Err(super::uncertain(format!(
                        "{}'s setting is still {} after Kumi stepped it",
                        quoted(&shown.plugin),
                        quoted(&before)
                    )));
                }
            }
        }
        let mut display = json!({"title": format!("{} on {} set to {}", quoted(&shown.plugin), quoted(&shown.strip), quoted(&now)), "before": before, "after": now});
        if steps == 0 {
            display["unchanged"] = true.into();
        }
        Ok(Applied {
            applied: json!({"plugin": shown.plugin, "preset": now, "steps": steps}),
            material: Some(json!({"steps": steps})),
            display,
            ..Applied::default()
        })
    })();
    window::hide(ctx, shown);
    result
}

/// Presses the window's Undo as many times as Kumi pressed a setting button, as far as it goes.
fn undo_presses(ctx: &mut Ctx, shown: &Shown, steps: usize) {
    if window::reveal(ctx, shown, "Undo").is_err() {
        return;
    }
    for _ in 0..steps {
        if window::press(ctx, shown, "Undo").is_err() {
            break;
        }
    }
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let (owner, slot) = strips::plugin(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let prior = restore.prior["preset"].as_str().unwrap_or_default().to_string();
    let applied = restore.applied["preset"].as_str().unwrap_or_default().to_string();
    let steps = restore.material.and_then(|m| m["steps"].as_u64()).unwrap_or(0) as usize;
    let shown = window::show_in(ctx, owner, slot, window::View::Editor)?;
    let result = (|| {
        let (_, now) = window::preset(ctx, &shown)?;
        if now == prior {
            return Ok(Restored::by_value(
                json!({"preset": now}),
                json!({"title": format!("{} is on {} already", quoted(&shown.plugin), quoted(&now))}),
            ));
        }
        if !restore.discard && now != applied {
            return Err(changed(format!("{}'s setting has been changed in Logic Pro since Kumi set it", quoted(&shown.plugin))));
        }
        undo_presses(ctx, &shown, steps);
        let (_, after) = window::preset(ctx, &shown).map_err(|(_, why)| super::uncertain(why))?;
        if after != prior {
            return Err(super::uncertain(format!(
                "{}'s window's own undo shows {} after Kumi pressed it {steps} time(s), not {}",
                quoted(&shown.plugin),
                quoted(&after),
                quoted(&prior)
            )));
        }
        Ok(Restored::by_value(
            json!({"preset": after}),
            json!({"title": format!("{} back on {}", quoted(&shown.plugin), quoted(&after)), "note": "Put back with the plug-in window's own undo."}),
        ))
    })();
    window::hide(ctx, shown);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_is_asked_by_name_or_by_steps() {
        assert_eq!(ask_of(&json!({"preset": "Tighten Subs"})).unwrap(), Ask::Named("Tighten Subs".into()));
        assert_eq!(ask_of(&json!({"step": -1})).unwrap(), Ask::Step(-1));
        assert!(ask_of(&json!({"step": 0})).is_err());
        assert!(ask_of(&json!({"step": 65})).is_err());
        assert!(ask_of(&json!({})).is_err());
        assert!(ask_of(&json!({"preset": "A", "step": 1})).is_err());
    }
}
