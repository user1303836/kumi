//! Changes to a track: its volume and pan through its header's sliders (a fader read back as dB text, a pan knob of
//! 0 to 127), which take a set without the track being selected or Logic in front; its mute, solo and record arm
//! through its header's checkboxes, which take no press or set from Accessibility (as the header's Has Focus button
//! takes none) but do take a click at their frame, posted at the system's event tap with Logic brought in front; and
//! its name through the Inspector's "Track:" field, which shows the selected track, so a track not selected is
//! selected first with a click on its header and the selection put back after. Logic keeps no undo step for the
//! header's controls, and one named "Renaming" for the name.

use serde_json::{json, Value};

use super::nudge::{self, Control, Reading};
use super::undo::Probe;
use super::{changed, gone, quoted, ref_text, timeline, track_place, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::ax::paths::Inspector;
use crate::ax::{AxError, Node};
use crate::refs::bad;
use crate::rows::{self, Header, PAN_CENTRE};

/// The header's fader runs 0 (silence) to 233 (+6 dB), with 0 dB at 173; the dB text is what is compared. How far a fader reading may be from what was asked and still count as applied: Logic's fader steps 0.1 dB near
/// 0 dB and more at the bottom.
pub const CLOSE_DB: f64 = 0.051;
/// The most dB the fader goes to.
pub const MAX_DB: f64 = 6.0;
/// How long a clicked toggle gets before the header is read back.
const CLICK_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);
/// How long a renamed field gets before the header is read back.
const RENAME_SETTLE_MS: u64 = 150;
/// How long a click on a header gets before the Inspector shows the track.
const SELECT_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// The control a prop writes through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Volume,
    Pan,
    Toggle(&'static str),
}

/// A track attribute Kumi changes: how it is read from the header, parsed from the model's input, compared and
/// worded. The state is an object whose first key is the exact value.
pub struct Prop {
    pub kind: &'static str,
    pub noun: &'static str,
    which: Which,
    read: fn(&Header) -> Value,
    wanted: fn(&Value) -> Done<Value>,
    same: fn(&Value, &Value) -> bool,
    title: fn(&str, &Value, &Value) -> String,
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// dB with the true minus sign, as the titles show it: `−6`, `+3`, `0`, `−6.5`, `−∞`.
pub fn db_text(db: f64) -> String {
    if db <= rows::MIN_DB + 0.5 {
        return "−∞".into();
    }
    let rounded = (db * 10.0).round() / 10.0;
    let text = format!("{:.1}", rounded.abs());
    let text = text.strip_suffix(".0").map(str::to_string).unwrap_or(text);
    if rounded == 0.0 {
        "0".into()
    } else if rounded < 0.0 {
        format!("−{text}")
    } else {
        format!("+{text}")
    }
}

/// A pan as a producer says it: `centre`, `30% left`, `100% right`.
pub fn pan_text(steps: i64) -> String {
    let percent = ((steps.abs() as f64 / PAN_CENTRE as f64) * 100.0).round() as i64;
    match steps {
        0 => "centre".into(),
        n if n < 0 => format!("{percent}% left"),
        _ => format!("{percent}% right"),
    }
}

fn volume_state(db: f64) -> Value {
    json!({"volumeDb": (db * 10.0).round() / 10.0})
}

fn pan_state(steps: i64) -> Value {
    json!({"pan": (steps as f64 / PAN_CENTRE as f64 * 1000.0).round() / 1000.0, "steps": steps})
}

fn flag_state(key: &str, on: bool) -> Value {
    json!({key: on})
}

fn same_db(a: &Value, b: &Value) -> bool {
    (number(a, "volumeDb") - number(b, "volumeDb")).abs() <= CLOSE_DB
}

pub static PROPS: [Prop; 5] = [
    Prop {
        kind: "track-volume",
        noun: "volume",
        which: Which::Volume,
        read: |header| volume_state(header.volume_db.unwrap_or(0.0)),
        wanted: |input| {
            let db = input
                .get("volumeDb")
                .and_then(Value::as_f64)
                .filter(|db| db.is_finite())
                .ok_or_else(|| bad("volumeDb must be a number"))?;
            if db > MAX_DB {
                return Err(bad(format!("volumeDb must be at most {MAX_DB} (Logic's fader goes no higher)")));
            }
            Ok(volume_state(db.max(rows::MIN_DB)))
        },
        same: same_db,
        title: |name, _, to| format!("{name} volume {} dB", db_text(number(to, "volumeDb"))),
    },
    Prop {
        kind: "track-pan",
        noun: "pan",
        which: Which::Pan,
        read: |header| pan_state(header.pan),
        wanted: |input| {
            let pan = input
                .get("pan")
                .and_then(Value::as_f64)
                .filter(|pan| (-1.0..=1.0).contains(pan))
                .ok_or_else(|| bad("pan must be a number from -1 (left) to 1 (right)"))?;
            Ok(pan_state(((pan * PAN_CENTRE as f64).round() as i64).clamp(-PAN_CENTRE, PAN_CENTRE - 1)))
        },
        same: |a, b| a["steps"] == b["steps"],
        title: |name, _, to| format!("{name} pan {}", pan_text(to["steps"].as_i64().unwrap_or(0))),
    },
    Prop {
        kind: "track-mute",
        noun: "mute",
        which: Which::Toggle("Mute"),
        read: |header| flag_state("mute", header.mute),
        wanted: |input| {
            Ok(flag_state("mute", input.get("mute").and_then(Value::as_bool).ok_or_else(|| bad("mute must be true or false"))?))
        },
        same: |a, b| a["mute"] == b["mute"],
        title: |name, _, to| format!("{name} {}", if to["mute"] == true { "muted" } else { "unmuted" }),
    },
    Prop {
        kind: "track-solo",
        noun: "solo",
        which: Which::Toggle("Solo"),
        read: |header| flag_state("solo", header.solo),
        wanted: |input| {
            Ok(flag_state("solo", input.get("solo").and_then(Value::as_bool).ok_or_else(|| bad("solo must be true or false"))?))
        },
        same: |a, b| a["solo"] == b["solo"],
        title: |name, _, to| format!("{name} {}", if to["solo"] == true { "soloed" } else { "unsoloed" }),
    },
    Prop {
        kind: "track-arm",
        noun: "record arm",
        which: Which::Toggle("Record Enable"),
        read: |header| flag_state("armed", header.armed),
        wanted: |input| {
            let armed = input
                .get("armed")
                .or_else(|| input.get("arm"))
                .and_then(Value::as_bool)
                .ok_or_else(|| bad("armed must be true or false"))?;
            Ok(flag_state("armed", armed))
        },
        same: |a, b| a["armed"] == b["armed"],
        title: |name, _, to| format!("{name} {}", if to["armed"] == true { "armed" } else { "disarmed" }),
    },
];

/// A track's header as read now: its item (with its controls' paths) and its row.
pub struct Found {
    pub place: usize,
    pub item: Node,
    pub header: Header,
}

impl Found {
    /// The track as a title says it: its name, or its number when it has none.
    pub fn label(&self) -> String {
        if self.header.name.is_empty() {
            format!("Track {}", self.place)
        } else {
            self.header.name.clone()
        }
    }
}

/// The window's layout, found when the reader hasn't yet.
pub(crate) fn ensure_layout(ctx: &mut Ctx) -> Result<(), AxError> {
    if ctx.project.layout.headers.is_none() {
        ctx.project.find_layout(ctx.ax)?;
    }
    Ok(())
}

/// Every header as read now, with its item.
pub(crate) fn headers(ctx: &mut Ctx) -> Result<Vec<Found>, AxError> {
    ensure_layout(ctx)?;
    let group = ctx.project.headers(ctx.ax)?;
    let strings = &ctx.project.strings;
    Ok(group
        .children
        .iter()
        .filter(|c| c.role == "AXLayoutItem")
        .enumerate()
        .map(|(index, item)| Found { place: index + 1, item: item.clone(), header: Header::of(item, index, strings) })
        .collect())
}

/// The names of the tracks, in order.
pub(crate) fn names(found: &[Found]) -> Vec<String> {
    found.iter().map(|f| f.header.name.clone()).collect()
}

/// The track a ref names, read now.
pub(crate) fn find(ctx: &mut Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<Found> {
    let place = track_place(ctx, input, key, epoch)?;
    let mut all = nudge::read(headers(ctx))?;
    if place > all.len() {
        return Err(crate::refs::not_in_project(input[key].as_str().unwrap_or_default()));
    }
    Ok(all.swap_remove(place - 1))
}

/// The track at a place, read now.
pub(crate) fn at(ctx: &mut Ctx, place: usize) -> Done<Found> {
    let mut all = nudge::read(headers(ctx))?;
    if place == 0 || place > all.len() {
        return Err(crate::refs::not_in_project(&ref_text(ctx, "track", &[place])));
    }
    Ok(all.swap_remove(place - 1))
}

fn find_for_apply(ctx: &mut Ctx, input: &Value, key: &str) -> Done<Found> {
    find(ctx, input, key, Epoch::Current).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })
}

fn find_for_restore(ctx: &mut Ctx, input: &Value, key: &str) -> Done<Found> {
    find(ctx, input, key, Epoch::Any).map_err(|fault| if fault.0 == "failed" { gone(fault.1) } else { fault })
}

fn target(ctx: &Ctx, place: usize) -> Value {
    json!({"ref": ref_text(ctx, "track", &[place]), "kind": "track"})
}

/// The header's control for a prop.
fn control(ctx: &Ctx, found: &Found, which: Which) -> Done<Control> {
    let strings = &ctx.project.strings;
    let node = match which {
        Which::Volume => found.item.child_where(|c| c.role == "AXSlider" && strings.is(&c.desc, "Volume")),
        Which::Pan => found.item.child_where(|c| c.role == "AXSlider" && !strings.is(&c.desc, "Volume")),
        Which::Toggle(label) => found.item.child_where(|c| c.role == "AXCheckBox" && strings.is(&c.desc, label)),
    };
    let node =
        node.ok_or_else(|| crate::refs::failed(format!("Logic Pro's header for {} doesn't show its {:?} control", found.label(), which)))?;
    Ok(Control::new(&ctx.project.root, &node.path))
}

/// Writes a prop's wanted state through the header's control and reads the header back: the state after.
fn write(ctx: &mut Ctx, prop: &Prop, found: &Found, wanted: &Value) -> Done<Value> {
    let control = control(ctx, found, prop.which)?;
    match prop.which {
        Which::Volume => {
            let target = number(wanted, "volumeDb");
            let root = ctx.project.root.clone();
            let path = control.path.clone();
            let read = move |ax: &mut crate::ax::Ax| -> Result<Reading, AxError> {
                let attrs = ax.get(&root, &path, &["AXValue", "AXValueDescription"])?;
                let raw = attrs.get("AXValue").and_then(Value::as_f64).unwrap_or(0.0).round() as i64;
                let value = attrs.get("AXValueDescription").and_then(Value::as_str).and_then(rows::db_of).unwrap_or(f64::NAN);
                Ok(Reading { value, raw })
            };
            if target <= rows::MIN_DB {
                // Silence: the fader's bottom.
                let now = nudge::read(read(ctx.ax))?;
                nudge::wrote(nudge::step_to(ctx.ax, &control, now.raw, 0))?;
            } else {
                nudge::wrote(nudge::step_reading(ctx.ax, &control, target, 1.2, CLOSE_DB, 1, read))?;
            }
        }
        Which::Pan => {
            let now = found.header.pan + PAN_CENTRE;
            let target = wanted["steps"].as_i64().unwrap_or(0) + PAN_CENTRE;
            nudge::wrote(nudge::step_to(ctx.ax, &control, now, target))?;
        }
        Which::Toggle(_) => {
            super::front::ensure(ctx.ax)?;
            let (x, y, w, h) = nudge::read(ctx.ax.frame(&control.root, &control.path))?;
            nudge::wrote(ctx.ax.click(x + w / 2.0, y + h / 2.0))?;
            super::pause(CLICK_SETTLE);
        }
    }
    let after = nudge::wrote(at(ctx, found.place).map_err(|fault| AxError::Missing(fault.1)))?;
    Ok((prop.read)(&after.header))
}

/// A track's solo turned on or off through its header's checkbox (a click with Logic in front), as `track-solo` does
/// it; whether it shows soloed after.
pub(crate) fn set_solo(ctx: &mut Ctx, place: usize, on: bool) -> Done<bool> {
    let found = at(ctx, place)?;
    if found.header.solo == on {
        return Ok(on);
    }
    let after = write(ctx, &PROPS[3], &found, &json!({"solo": on}))?;
    Ok(after["solo"] == Value::Bool(true))
}

fn probe(prop: &'static Prop, place: usize, prior: &Value, applied: &Value) -> Probe {
    Probe::Header { place, read: prop.read, same: prop.same, prior: prior.clone(), applied: applied.clone() }
}

pub fn prop_preview(ctx: &mut Ctx, prop: &'static Prop, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let now = (prop.read)(&found.header);
    let wanted = (prop.wanted)(input)?;
    let title = (prop.title)(&found.label(), &now, &wanted);
    let mut display = json!({"title": title, "before": now, "after": wanted});
    if (prop.same)(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, found.place), prior: now.clone(), fence: now, display })
}

pub fn prop_apply(ctx: &mut Ctx, prop: &'static Prop, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input, "ref")?;
    let now = (prop.read)(&found.header);
    if !(prop.same)(&now, fence) {
        return Err(changed(format!("{}'s {} changed since the preview", found.label(), prop.noun)));
    }
    let wanted = (prop.wanted)(input)?;
    let after = if (prop.same)(&now, &wanted) { now.clone() } else { write(ctx, prop, &found, &wanted)? };
    if !(prop.same)(&after, &wanted) {
        return Err(nudge::kept(&format!("{}'s {}", found.label(), prop.noun), wanted, after));
    }
    let title = (prop.title)(&found.label(), &now, &after);
    Ok(Applied {
        applied: after.clone(),
        created: None,
        target: None,
        material: None,
        display: json!({"title": title, "before": now, "after": after}),
        step: None,
        probe: Some(probe(prop, found.place, &now, &after)),
    })
}

pub fn prop_restore(ctx: &mut Ctx, prop: &'static Prop, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, restore.target, "ref")?;
    let now = (prop.read)(&found.header);
    let label = found.label();
    if !(prop.same)(&now, restore.prior) {
        if !restore.discard && !(prop.same)(&now, restore.applied) {
            return Err(changed(format!("{label}'s {} changed in Logic Pro since Kumi set it", prop.noun)));
        }
        let after = write(ctx, prop, &found, restore.prior)?;
        if !(prop.same)(&after, restore.prior) {
            return Err(nudge::kept(&format!("{label}'s {}", prop.noun), restore.prior, after));
        }
    }
    let after = (prop.read)(&at(ctx, found.place)?.header);
    let title = (prop.title)(&label, &now, &after);
    Ok(Restored::by_value(after.clone(), json!({"title": format!("{title} (put back)"), "before": now, "after": after})))
}

// ----- the name -----

fn name_input(input: &Value) -> Done<String> {
    let name = input.get("name").and_then(Value::as_str).ok_or_else(|| bad("name must be text"))?;
    if name.trim().is_empty() {
        return Err(bad("name must not be empty: Logic Pro names a track itself when its name is cleared"));
    }
    if name.chars().count() > 256 || name.chars().any(char::is_control) {
        return Err(bad("name must be one line of at most 256 characters"));
    }
    Ok(name.to_string())
}

fn name_state(name: &str) -> Value {
    json!({"name": name})
}

fn rename_title(from: &str, to: &str) -> String {
    format!("Renamed track {} → {}", quoted(from), quoted(to))
}

pub fn rename_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let name = name_input(input)?;
    let now = name_state(&found.header.name);
    let mut display = json!({"title": rename_title(&found.header.name, &name), "before": now, "after": name_state(&name)});
    if found.header.name == name {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, found.place), prior: now.clone(), fence: now, display })
}

/// The selected track's place, from the headers.
fn selected_place(all: &[Found]) -> Option<usize> {
    all.iter().find(|f| f.header.selected).map(|f| f.place)
}

/// Renames the track at a place through the Inspector's field: selects it first when it isn't (a click, which
/// brings Logic in front), and puts the selection back after. The name the header shows after.
pub(crate) fn rename(ctx: &mut Ctx, place: usize, name: &str) -> Done<String> {
    let all = nudge::read(headers(ctx))?;
    let was_selected = selected_place(&all);
    if was_selected != Some(place) {
        super::front::ensure(ctx.ax)?;
        nudge::read(ctx.project.select_track(ctx.ax, place))?;
        super::pause(SELECT_SETTLE);
        let now = nudge::read(headers(ctx))?;
        if selected_place(&now) != Some(place) {
            return Err(crate::refs::failed(format!("Logic Pro didn't select track {place} for its name to be changed; nothing changed")));
        }
    }
    let inspector = nudge::read(ctx.project.inspector(ctx.ax))?;
    let field = Inspector::of(&inspector, &ctx.project.strings)
        .track_name
        .map(|node| node.path.clone())
        .ok_or_else(|| crate::refs::failed("Logic Pro's Inspector doesn't show the track's name field; nothing changed"))?;
    let control = Control::new(&ctx.project.root, &field);
    nudge::wrote(nudge::confirm_text(ctx.ax, &control, name, RENAME_SETTLE_MS))?;
    let after = nudge::wrote(headers(ctx))?;
    let shown = after.get(place - 1).map(|f| f.header.name.clone()).unwrap_or_default();
    if let Some(was) = was_selected.filter(|was| *was != place && *was <= after.len()) {
        let _ = ctx.project.select_track(ctx.ax, was);
    }
    // A renamed header moves the epoch, as the observers count it: the answer carries the new one.
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    Ok(shown)
}

pub fn rename_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input, "ref")?;
    let now = name_state(&found.header.name);
    if now != *fence {
        return Err(changed(format!("{}'s name changed since the preview", found.label())));
    }
    let name = name_input(input)?;
    let shown = if found.header.name == name { name.clone() } else { rename(ctx, found.place, &name)? };
    if shown != name {
        return Err(nudge::kept("the track's name", quoted(&name), quoted(&shown)));
    }
    let after = name_state(&shown);
    Ok(Applied {
        applied: after.clone(),
        created: None,
        target: None,
        material: None,
        display: json!({"title": rename_title(&found.header.name, &shown), "before": now, "after": after}),
        step: Some("Renaming"),
        probe: Some(Probe::Header {
            place: found.place,
            read: |header| json!(header.name),
            same: |a, b| a == b,
            prior: json!(found.header.name),
            applied: json!(shown),
        }),
    })
}

pub fn rename_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, restore.target, "ref")?;
    let now = name_state(&found.header.name);
    let prior = restore.prior["name"].as_str().unwrap_or_default().to_string();
    if now != *restore.prior {
        if !restore.discard && now != *restore.applied {
            return Err(changed(format!("{}'s name changed in Logic Pro since Kumi set it", found.label())));
        }
        let shown = rename(ctx, found.place, &prior)?;
        if shown != prior {
            return Err(nudge::kept("the track's name", quoted(&prior), quoted(&shown)));
        }
    }
    Ok(Restored {
        restored: name_state(&prior),
        display: json!({"title": format!("{} (put back)", rename_title(&found.header.name, &prior)), "before": now, "after": name_state(&prior)}),
        undone_in_logic: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_pans_are_worded_as_the_titles_show_them() {
        assert_eq!(db_text(-6.0), "−6");
        assert_eq!(db_text(-6.04), "−6");
        assert_eq!(db_text(3.5), "+3.5");
        assert_eq!(db_text(0.0), "0");
        assert_eq!(db_text(-0.04), "0");
        assert_eq!(db_text(-96.0), "−∞");
        assert_eq!(pan_text(0), "centre");
        assert_eq!(pan_text(-32), "50% left");
        assert_eq!(pan_text(63), "98% right");
        let pan = &PROPS[1];
        assert_eq!((pan.wanted)(&json!({"pan": -0.5})).unwrap(), json!({"pan": -0.5, "steps": -32}));
        assert_eq!((pan.wanted)(&json!({"pan": 1.0})).unwrap()["steps"], 63);
        assert!((pan.wanted)(&json!({"pan": 2})).is_err());
        let volume = &PROPS[0];
        assert_eq!((volume.wanted)(&json!({"volumeDb": -6})).unwrap(), json!({"volumeDb": -6.0}));
        assert!((volume.wanted)(&json!({"volumeDb": 7})).is_err());
        assert!((volume.same)(&json!({"volumeDb": -6.0}), &json!({"volumeDb": -6.04})));
        assert!(!(volume.same)(&json!({"volumeDb": -6.0}), &json!({"volumeDb": -6.1})));
        let arm = &PROPS[4];
        assert_eq!((arm.wanted)(&json!({"arm": true})).unwrap(), json!({"armed": true}));
        assert_eq!((arm.title)("Vocal", &json!({}), &json!({"armed": false})), "Vocal disarmed");
        assert_eq!(rename_title("Vocal", "Vox"), "Renamed track “Vocal” → “Vox”");
        assert!(name_input(&json!({"name": " "})).is_err());
        assert!(name_input(&json!({"name": "a\nb"})).is_err());
        assert_eq!(name_input(&json!({"name": "Vox"})).unwrap(), "Vox");
    }
}
