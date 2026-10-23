//! A track's Smart Controls: the pane (B, the control bar's Smart Controls button) shows the selected track's, as
//! sliders named after their controls ("Tone", "Ambience"), each a share of its range from 0 to 1 with its text beside
//! it ("38%"); a set moves one a hundredth toward the value asked, an increment a tenth (measured on Logic Pro
//! 12.3.1). `smart-control-set` selects the track, opens the pane, steps the control and reads it back, and puts the
//! pane and the selection back; `read {kind: smart, parent: <track>}` lists them. A Smart Control moved is no step of
//! Logic's project undo (it moves a plug-in's parameter), so the restore is by value.

use serde_json::{json, Value};

use super::nudge;
use super::panes;
use super::{changed, gone, quoted, tracks, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::ax::Node;
use crate::refs::{bad, failed, Ref};

/// How close a control must come to the value asked (its set's step is a hundredth).
const CLOSE: f64 = 0.0051;
/// The most steps one value is reached in.
const MOST_STEPS: usize = 40;
/// Controls whose tops are within this many points share a row.
const ROW: f64 = 24.0;

/// A Smart Control as the pane shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub index: usize,
    pub name: String,
    pub path: String,
    pub value: f64,
    pub text: String,
}

/// How long a pane drawn without controls gets before it is read once more and taken as having none.
const EMPTY_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// The pane's controls: the sliders of its layout area named "Smart Controls", in the order the pane shows them (by
/// rows, then from the left, from their frames: Accessibility lists them in an order of its own, which changes from
/// one showing of the pane to the next, measured on Logic Pro 12.3.1). A track whose strip has nothing Smart Controls
/// map (an audio track with only a Channel EQ, or nothing, on it) shows the pane's header, its Controls and EQ views'
/// buttons, over no layout area: no controls. A pane without even its header hasn't drawn, and that is `failed`.
fn controls(ctx: &mut Ctx, pane: &Node) -> Done<Vec<Control>> {
    let root = ctx.project.root.clone();
    let read = |ctx: &mut Ctx| nudge::read(ctx.ax.tree(&root, &pane.path, 5, 100, Some(&["role", "desc", "value", "valueDesc"])));
    let strings = ctx.project.strings.clone();
    let has_area = |tree: &Node| tree.find(&|n| n.role == "AXLayoutArea" && strings.is(&n.desc, "Smart Controls")).is_some();
    let mut tree = read(ctx)?;
    if !has_area(&tree) {
        super::pause(EMPTY_SETTLE);
        tree = read(ctx)?;
    }
    let Some(area) = tree.find(&|n| n.role == "AXLayoutArea" && strings.is(&n.desc, "Smart Controls")) else {
        if tree.find(&|n| n.role == "AXRadioButton" && strings.is(&n.desc, "Controls")).is_some() {
            return Ok(vec![]);
        }
        return Err(failed("Logic Pro's Smart Controls pane shows no controls for this track"));
    };
    let sliders: Vec<Node> = area.children.iter().filter(|c| c.role == "AXSlider" && !c.desc.is_empty()).cloned().collect();
    let mut placed = Vec::new();
    for slider in sliders {
        let (x, y, _, _) = nudge::read(ctx.ax.frame(&root, &slider.path))?;
        placed.push(((y / ROW).round() as i64, x.round() as i64, slider));
    }
    placed.sort_by_key(|(row, x, _)| (*row, *x));
    Ok(placed
        .into_iter()
        .enumerate()
        .map(|(i, (_, _, slider))| Control {
            index: i + 1,
            name: slider.desc.clone(),
            path: slider.path.clone(),
            value: slider.number().unwrap_or(0.0),
            text: slider.value_desc.clone(),
        })
        .collect())
}

fn row(control: &Control, reference: &str, track: &str) -> Value {
    json!({"ref": reference, "trackRef": track, "index": control.index, "name": control.name, "value": round(control.value), "text": control.text})
}

fn round(value: f64) -> f64 {
    (value * 1e4).round() / 1e4
}

/// The control a request names: by its name (any case) or its place (from 1).
fn find<'a>(all: &'a [Control], which: &Value) -> Done<&'a Control> {
    match which {
        Value::Number(n) => n
            .as_u64()
            .and_then(|i| all.get((i as usize).wrapping_sub(1)))
            .ok_or_else(|| bad(format!("control must be a Smart Control's place from 1 to {}, or its name", all.len()))),
        Value::String(name) => all.iter().find(|c| c.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
            bad(format!(
                "this track has no Smart Control named {name:?}: it has {}",
                all.iter().map(|c| quoted(&c.name)).collect::<Vec<_>>().join(", ")
            ))
        }),
        _ => Err(bad("control must be the Smart Control's name or its place (from 1)")),
    }
}

fn value_of(input: &Value) -> Done<f64> {
    input
        .get("value")
        .and_then(Value::as_f64)
        .filter(|v| (0.0..=1.0).contains(v))
        .ok_or_else(|| bad("value must be a number from 0 to 1: the control's share of its range"))
}

/// Steps a control to a value: tenths by increments while far, hundredths by sets after. The control after.
fn step(ctx: &mut Ctx, control: &Control, target: f64) -> Done<Control> {
    let root = ctx.project.root.clone();
    // The control read where it is, its value and its text.
    let read = |ctx: &mut Ctx| -> Done<Control> {
        let attrs = nudge::wrote(ctx.ax.get(&root, &control.path, &["AXDescription", "AXValue", "AXValueDescription"]))?;
        if attrs.get("AXDescription").and_then(Value::as_str) != Some(control.name.as_str()) {
            return Err(super::uncertain("the Smart Control went"));
        }
        let mut now = control.clone();
        now.value = attrs.get("AXValue").and_then(Value::as_f64).unwrap_or(now.value);
        now.text = attrs.get("AXValueDescription").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(now)
    };
    let mut now = control.clone();
    for _ in 0..MOST_STEPS {
        let away = target - now.value;
        if away.abs() <= CLOSE {
            break;
        }
        if away.abs() >= 0.1 {
            let action = if away > 0.0 { "AXIncrement" } else { "AXDecrement" };
            nudge::wrote(ctx.ax.action(&root, &now.path, action, 60))?;
        } else {
            let toward = if away > 0.0 { (now.value + 0.01).min(1.0) } else { (now.value - 0.01).max(0.0) };
            nudge::wrote(ctx.ax.set(&root, &now.path, "AXValue", json!(toward), 60))?;
        }
        let next = read(ctx)?;
        if (next.value - now.value).abs() < 1e-9 {
            break;
        }
        // An increment past the value comes back by sets from there.
        now = next;
    }
    Ok(now)
}

fn track_of(ctx: &mut Ctx, input: &Value, epoch: Epoch) -> Done<(usize, String)> {
    let found = tracks::find(ctx, input, "ref", epoch)?;
    Ok((found.place, found.label()))
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (place, label) = track_of(ctx, input, Epoch::Current)?;
    let which = input.get("control").filter(|v| !v.is_null()).cloned().ok_or_else(|| bad("control: the Smart Control's name or place"))?;
    let value = value_of(input)?;
    let control = panes::with_selected(ctx, place, |ctx| {
        panes::with_pane(ctx, "Smart Controls", |ctx, pane| Ok(find(&controls(ctx, pane)?, &which)?.clone()))
    })?;
    let now = json!({"name": control.name, "value": round(control.value), "text": control.text});
    let mut display = json!({"title": format!("{}'s Smart Control {} at {}%", quoted(&label), quoted(&control.name), (value * 100.0).round()), "before": now});
    if (control.value - value).abs() <= CLOSE {
        display["unchanged"] = true.into();
    }
    Ok(Preview {
        target: json!({"ref": Ref::new(ctx.project.epoch, "smart", &[place, control.index]).text(), "kind": "smart", "track": Ref::new(ctx.project.epoch, "track", &[place]).text()}),
        prior: now.clone(),
        fence: now,
        display,
    })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (place, label) = track_of(ctx, input, Epoch::Current).map_err(|f| if f.0 == "failed" { changed(f.1) } else { f })?;
    let which = input.get("control").filter(|v| !v.is_null()).cloned().ok_or_else(|| bad("control: the Smart Control's name or place"))?;
    let value = value_of(input)?;
    let (before, after) = panes::with_selected(ctx, place, |ctx| {
        panes::with_pane(ctx, "Smart Controls", |ctx, pane| {
            let control = find(&controls(ctx, pane)?, &which)?.clone();
            if json!({"name": control.name, "value": round(control.value), "text": control.text}) != *fence {
                return Err(changed(format!("{}'s Smart Control {} changed since the preview", quoted(&label), quoted(&control.name))));
            }
            let after = step(ctx, &control, value)?;
            Ok((control, after))
        })
    })?;
    if (after.value - value).abs() > CLOSE {
        return Err(nudge::kept(&format!("the Smart Control {}", quoted(&after.name)), value, round(after.value)));
    }
    let state = json!({"name": after.name, "value": round(after.value), "text": after.text});
    Ok(Applied {
        applied: state.clone(),
        display: json!({"title": format!("{}'s Smart Control {} at {}", quoted(&label), quoted(&after.name), after.text), "before": {"value": round(before.value), "text": before.text}, "after": state}),
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let target = restore.target;
    let track = json!({"ref": target["track"]});
    let (place, label) = track_of(ctx, &track, Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let name = restore.prior["name"].as_str().unwrap_or_default().to_string();
    let prior = restore.prior["value"].as_f64().ok_or_else(|| bad("prior.value"))?;
    let applied = restore.applied["value"].as_f64();
    let after = panes::with_selected(ctx, place, |ctx| {
        panes::with_pane(ctx, "Smart Controls", |ctx, pane| {
            let control = find(&controls(ctx, pane)?, &json!(name))?.clone();
            if (control.value - prior).abs() <= CLOSE {
                return Ok(control);
            }
            if !restore.discard && !applied.is_some_and(|a| (control.value - a).abs() <= CLOSE) {
                return Err(changed(format!(
                    "{}'s Smart Control {} has been changed in Logic Pro since Kumi set it",
                    quoted(&label),
                    quoted(&name)
                )));
            }
            step(ctx, &control, prior)
        })
    })?;
    if (after.value - prior).abs() > CLOSE {
        return Err(nudge::kept(&format!("the Smart Control {}", quoted(&name)), prior, round(after.value)));
    }
    Ok(Restored::by_value(
        json!({"name": after.name, "value": round(after.value), "text": after.text}),
        json!({"title": format!("{}'s Smart Control {} at {} (put back)", quoted(&label), quoted(&after.name), after.text)}),
    ))
}

/// `read {kind: smart, parent: <track>}`: the selected track's Smart Controls, for the track named (selected for the
/// read and put back).
pub fn rows(ctx: &mut Ctx, place: usize) -> Done<Vec<Value>> {
    let epoch = ctx.project.epoch;
    let track = Ref::new(epoch, "track", &[place]).text();
    let all = panes::with_selected(ctx, place, |ctx| panes::with_pane(ctx, "Smart Controls", |ctx, pane| controls(ctx, pane)))?;
    Ok(all.iter().map(|c| row(c, &Ref::new(epoch, "smart", &[place, c.index]).text(), &track)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smart_control_is_named_or_placed_and_set_as_a_share() {
        let all = vec![
            Control { index: 1, name: "Comp".into(), path: "a".into(), value: 0.375, text: "38%".into() },
            Control { index: 2, name: "Tone".into(), path: "b".into(), value: 0.0, text: "0%".into() },
        ];
        assert_eq!(find(&all, &json!("tone")).unwrap().index, 2);
        assert_eq!(find(&all, &json!(1)).unwrap().name, "Comp");
        assert!(find(&all, &json!("Drive")).unwrap_err().1.contains("“Comp”"));
        assert_eq!(value_of(&json!({"value": 0.5})).unwrap(), 0.5);
        assert!(value_of(&json!({"value": 50})).is_err());
        assert_eq!(
            row(&all[0], "1:smart:2.1", "1:track:2"),
            json!({"ref": "1:smart:2.1", "trackRef": "1:track:2", "index": 1, "name": "Comp", "value": 0.375, "text": "38%"})
        );
    }
}
