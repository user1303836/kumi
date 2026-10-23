//! Sends on a track's strip, through the Mixer (the pane opened for the change and closed after): `send-add` presses
//! the strip's empty send slot and walks Logic's send menu by keys ([`super::menus`]: Down twice to Bus, Right into
//! its list of busses, Down to the bus, Return), Logic making an aux channel strip for a bus nothing used yet;
//! `send-remove` presses the send's list button and takes No Send (the menu's sixth item, after the send's routing
//! modes); `send-volume` steps the send's knob, a slider whose text is its level in dB (a set moves it one fine step,
//! an increment a coarse one), which Logic keeps no undo step for. Read back from the strip: a send is its group,
//! named after its bus, and the knob listed right after it. Logic's history names a send made or removed "Change Send
//! in Channel Strip". `send-mode` isn't made: a send's mode is a mark in its menu, which Logic draws itself and
//! Accessibility can't read, so it could never be read back.

use serde_json::{json, Value};

use super::menus::{self, Key};
use super::nudge::{self, Control, Reading};
use super::strips::{self, Owner};
use super::undo::{Probe, StripPart};
use super::{changed, gone, quoted, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::ax::paths::{self, Strip};
use crate::ax::strings::Strings;
use crate::ax::{AxError, Node};
use crate::refs::{bad, failed};

/// Logic's name for the step a send made or removed takes.
const STEP: &str = "Change Send in Channel Strip";
/// The most busses Logic's send menu offers.
const MOST_BUS: u64 = 64;
/// The keys that take No Send from an occupied send's menu.
const NO_SEND: [Key; 7] = [
    Key::Named("down"),
    Key::Named("down"),
    Key::Named("down"),
    Key::Named("down"),
    Key::Named("down"),
    Key::Named("down"),
    Key::Named("return"),
];
/// How long the send menu's Bus item gets to open its list, and Logic to make the send.
const SUBMENU_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
const SEND_SETTLE: std::time::Duration = std::time::Duration::from_millis(900);
/// The fader's top and how close a level must come (the knob's fine step is 0.1 dB near 0 dB).
const MAX_DB: f64 = 6.0;
const CLOSE_DB: f64 = 0.051;
/// A set toward a value this far from the knob's raw value moves it one fine step that way.
const FINE: i64 = 100_000_000;

/// A send on a strip: its group, the knob after it, its bus and its level.
#[derive(Debug, Clone)]
pub struct Send {
    pub group: Node,
    pub knob: Option<Node>,
    pub bus: String,
    pub level: Option<f64>,
}

/// A strip's sends from the top, and its empty send slot.
#[derive(Debug, Clone, Default)]
pub struct Sends {
    pub sends: Vec<Send>,
    pub empty: Option<Node>,
}

impl Sends {
    pub fn of(strip: &Node, strings: &Strings) -> Sends {
        let parts = Strip::of(strip, strings);
        let knobs = crate::rows::send_knobs(strip, &parts.sends, strings);
        let mut found = Sends::default();
        for (slot, knob) in parts.sends.iter().zip(knobs).rev() {
            if paths::occupied(slot) {
                let level = knob.and_then(|k| crate::rows::send_db(&k.value_desc));
                found.sends.push(Send { group: (*slot).clone(), knob: knob.cloned(), bus: slot.desc.clone(), level });
            } else if found.empty.is_none() || strings.is(&slot.desc, "send button") {
                found.empty = Some((*slot).clone());
            }
        }
        found
    }

    pub fn busses(&self) -> Vec<String> {
        self.sends.iter().map(|send| send.bus.clone()).collect()
    }
}

/// The bus a request names: `bus` or `toRef`, a number or "Bus n".
fn bus_of(input: &Value) -> Done<u64> {
    let value = input.get("bus").filter(|v| !v.is_null()).or_else(|| input.get("toRef").filter(|v| !v.is_null()));
    let number = match value {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(text)) => text.trim().strip_prefix("Bus").map(str::trim).unwrap_or(text.trim()).parse().ok(),
        _ => None,
    };
    number
        .filter(|n| (1..=MOST_BUS).contains(n))
        .ok_or_else(|| bad(format!("bus (or toRef) must name a bus from 1 to {MOST_BUS}, such as 2 or \"Bus 2\": Logic Pro sends to busses, and makes an aux for one nothing uses yet")))
}

fn level_of(input: &Value) -> Done<Option<f64>> {
    match input.get("volumeDb") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let db = value.as_f64().filter(|db| db.is_finite()).ok_or_else(|| bad("volumeDb must be a number"))?;
            if db > MAX_DB {
                return Err(bad(format!("volumeDb must be at most {MAX_DB} (a send's knob goes no higher)")));
            }
            Ok(Some(db.max(crate::rows::MIN_DB)))
        }
    }
}

fn db_text(db: f64) -> String {
    super::tracks::db_text(db)
}

/// The strip's label and sends, read in the open Mixer.
fn now(ctx: &mut Ctx, place: usize, strip: &Node) -> Done<(String, Sends)> {
    let label = strips::label(ctx, Owner::Track(place), Some(strip))?;
    Ok((label, Sends::of(strip, &ctx.project.strings)))
}

/// The names of the Mixer's strips (an aux made by a send shows as a new one).
fn strip_names(ctx: &mut Ctx) -> Done<Vec<String>> {
    let strips = nudge::read(ctx.project.mixer_strip_nodes(ctx.ax))?;
    let strings = ctx.project.strings.clone();
    Ok(strips.iter().map(|strip| Strip::of(strip, &strings).name.map(Node::value_text).unwrap_or_else(|| strip.desc.clone())).collect())
}

/// A send's level reached, and how close it is held to the level asked: a knob's fine step is a tenth of a dB near
/// 0 dB and a whole one low down (measured on Logic Pro 12.3.1), so a level comes within half the step there.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Level {
    db: f64,
    close: f64,
}

impl Level {
    fn holds(&self, wanted: f64) -> bool {
        (self.db - wanted).abs() <= self.close || (self.db <= crate::rows::MIN_DB && wanted <= crate::rows::MIN_DB)
    }
}

/// The most moves one level takes, and how many times the increments may pass it before the sets take over.
const MOST_MOVES: usize = 160;
const MOST_PASSES: usize = 2;
/// Increments go on while the level is further than this from the one asked.
const COARSE_DB: f64 = 1.05;

/// Steps a send's knob to a level: increments while it is more than a dB away (one is 1 dB near 0 dB and 10 dB lower
/// down, so they may pass it and come back), then sets, each a fine step toward it (a tenth of a dB above -6 dB, a
/// whole one below, measured on Logic Pro 12.3.1), until it is within the step or passed, the nearer side kept. The
/// level it reads after.
fn set_level(ctx: &mut Ctx, knob: &Node, db: f64) -> Done<Level> {
    let control = Control::new(&ctx.project.root, &knob.path);
    let (root, path) = (ctx.project.root.clone(), knob.path.clone());
    let read = |ax: &mut crate::ax::Ax| -> Result<Reading, AxError> {
        let attrs = ax.get(&root, &path, &["AXValue", "AXValueDescription"])?;
        let raw = attrs.get("AXValue").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        let value = attrs.get("AXValueDescription").and_then(Value::as_str).and_then(crate::rows::send_db).unwrap_or(f64::NAN);
        Ok(Reading { value, raw })
    };
    if db <= crate::rows::MIN_DB {
        let now = nudge::read(read(ctx.ax))?;
        nudge::wrote(nudge::step_to(ctx.ax, &control, now.raw, 0))?;
        let now = nudge::read(read(ctx.ax))?;
        return Ok(Level { db: now.value, close: CLOSE_DB });
    }
    let set = |ax: &mut crate::ax::Ax, raw: i64| ax.set(&control.root, &control.path, "AXValue", json!(raw), 40).map(|_| ());
    let mut now = nudge::read(read(ctx.ax))?;
    let mut passes = 0;
    let mut close = CLOSE_DB;
    for _ in 0..MOST_MOVES {
        let away = db - now.value;
        if away.abs() <= close {
            break;
        }
        let up = away > 0.0;
        let coarse = passes < MOST_PASSES && away.abs() > COARSE_DB;
        if coarse {
            nudge::wrote(ctx.ax.action(&control.root, &control.path, if up { "AXIncrement" } else { "AXDecrement" }, 40))?;
        } else {
            nudge::wrote(set(ctx.ax, now.raw + if up { FINE } else { -FINE }))?;
        }
        let next = nudge::wrote(read(ctx.ax))?;
        if next.raw == now.raw {
            if coarse {
                // The end of the knob for increments: the sets go on from here.
                passes = MOST_PASSES;
                continue;
            }
            break;
        }
        let passed = (db - next.value).signum() != away.signum();
        if !coarse {
            close = ((next.value - now.value).abs() / 2.0 + 0.001).max(CLOSE_DB);
            if passed {
                // A set past the level: the nearer of the two sides, a set back when it was the one before.
                if (db - next.value).abs() > away.abs() {
                    nudge::wrote(set(ctx.ax, next.raw + if up { -FINE } else { FINE }))?;
                    now = nudge::wrote(read(ctx.ax))?;
                } else {
                    now = next;
                }
                break;
            }
        } else if passed {
            passes += 1;
        }
        now = next;
    }
    Ok(Level { db: now.value, close })
}

fn same_db(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= CLOSE_DB || (a <= crate::rows::MIN_DB && b <= crate::rows::MIN_DB),
        (None, None) => true,
        _ => false,
    }
}

fn send_target(ctx: &Ctx, place: usize, slot: usize) -> Value {
    json!({"ref": Owner::Track(place).send(ctx.project.epoch, slot), "kind": "send", "track": Owner::Track(place).text(ctx.project.epoch)})
}

// ----- add -----

pub fn add_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let owner = strips::owner(ctx, input, "ref", Epoch::Current)?;
    let Owner::Track(place) = owner else { return Err(bad("a send goes from a track's strip: an output has none")) };
    let bus = bus_of(input)?;
    let level = level_of(input)?;
    if input.get("mode").is_some_and(|v| !v.is_null()) {
        return Err(bad("Logic Pro shows a send's mode only as a mark in its menu, which Kumi can't read back: leave mode out"));
    }
    let (label, sends) = strips::with_strip(ctx, owner, |ctx, strip| now(ctx, place, strip))?;
    let fence = json!({"sends": sends.busses()});
    let mut display = json!({"title": format!("Added send from {} to Bus {bus}", quoted(&label))});
    match level {
        Some(db) => display["level"] = db_text(db).into(),
        None => display["note"] = "Logic Pro makes a send at its own default level (−∞ on this Mac): give volumeDb to hear it.".into(),
    }
    Ok(Preview { target: json!({"ref": owner.text(ctx.project.epoch), "kind": "track"}), prior: fence.clone(), fence, display })
}

pub fn add_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let owner = strips::owner(ctx, input, "ref", Epoch::Current).map_err(strips::went)?;
    let Owner::Track(place) = owner else { return Err(bad("a send goes from a track's strip: an output has none")) };
    let bus = bus_of(input)?;
    let level = level_of(input)?;
    super::front::ensure(ctx.ax)?;
    let (label, after, aux, reached) = strips::with_strip(ctx, owner, |ctx, strip| {
        let (label, sends) = now(ctx, place, strip)?;
        if json!({"sends": sends.busses()}) != *fence {
            return Err(changed(format!("the sends of {} changed since the preview", quoted(&label))));
        }
        let empty = sends.empty.clone().ok_or_else(|| failed(format!("Logic Pro's strip {} shows no free send slot", quoted(&label))))?;
        let strips_before = strip_names(ctx)?;
        menus::open(ctx, &Control::new(&ctx.project.root, &empty.path))?;
        let mut keys = vec![Key::Named("down"), Key::Named("down")];
        let walked = (|| {
            menus::keys(ctx, &keys)?;
            super::pause(SUBMENU_SETTLE);
            keys = vec![Key::Named("right")];
            keys.extend(std::iter::repeat_n(Key::Named("down"), (bus - 1) as usize));
            keys.push(Key::Named("return"));
            menus::keys(ctx, &keys)
        })();
        if let Err(fault) = walked {
            menus::close(ctx);
            menus::close(ctx);
            return Err(fault);
        }
        super::pause(SEND_SETTLE);
        let strip = strips::strip_of(ctx, owner).map_err(|(_, why)| super::uncertain(why))?;
        let after = Sends::of(&strip, &ctx.project.strings);
        let aux: Vec<String> = strip_names(ctx)?.into_iter().filter(|name| !strips_before.contains(name)).collect();
        let mut reached = None;
        if let (Some(db), Some(send)) = (level, after.sends.last().filter(|s| s.bus == format!("Bus {bus}"))) {
            if let Some(knob) = &send.knob {
                reached = Some(set_level(ctx, knob, db)?);
            }
        }
        Ok((label, after, aux, reached))
    })?;
    let wanted = format!("Bus {bus}");
    let mut expected = fence["sends"].as_array().cloned().unwrap_or_default();
    expected.push(wanted.clone().into());
    let shown: Vec<Value> = after.busses().into_iter().map(Value::from).collect();
    if shown != expected {
        return Err(super::uncertain(format!(
            "Logic Pro's strip {} shows the sends {:?} after Kumi asked for one to {wanted}",
            quoted(&label),
            after.busses()
        )));
    }
    let send = after.sends.last().cloned().ok_or_else(|| super::uncertain("the new send isn't on the strip"))?;
    let level_now = reached.map(|r: Level| r.db).or(send.level);
    if let Some(db) = level {
        if !reached.is_some_and(|r| r.holds(db)) {
            return Err(nudge::kept(&format!("the send to {wanted}'s level"), db_text(db), level_now.map_or("nothing".into(), db_text)));
        }
    }
    let slot = after.sends.len();
    let mut created = json!({"ref": owner.send(ctx.project.epoch, slot), "kind": "send", "bus": bus});
    if let Some(aux) = aux.first() {
        created["aux"] = aux.clone().into();
    }
    let mut display = json!({"title": format!("Added send from {} to {wanted}", quoted(&label))});
    if let Some(aux) = aux.first() {
        display["note"] = format!("Logic Pro made the aux {} for {wanted}.", quoted(aux)).into();
    }
    let prior: Vec<String> = fence["sends"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    Ok(Applied {
        applied: json!({"destination": wanted, "slot": slot, "level": level_now}),
        created: Some(created),
        display,
        step: Some(STEP),
        probe: Some(Probe::Strip { place, part: StripPart::Sends, prior, applied: after.busses() }),
        ..Applied::default()
    })
}

/// Takes a send off its strip: its list button pressed, No Send taken. The strip's sends after.
fn take_off(ctx: &mut Ctx, owner: Owner, send: &Send) -> Done<Sends> {
    let strings = &ctx.project.strings;
    let list = send.group.children.iter().find(|c| c.role == "AXButton" && strings.is(&c.desc, "list"));
    let point = Control::new(&ctx.project.root, list.map_or(send.group.path.as_str(), |button| button.path.as_str()));
    menus::open(ctx, &point)?;
    if let Err(fault) = menus::keys(ctx, &NO_SEND) {
        menus::close(ctx);
        return Err(fault);
    }
    super::pause(SEND_SETTLE);
    let strip = strips::strip_of(ctx, owner).map_err(|(_, why)| super::uncertain(why))?;
    Ok(Sends::of(&strip, &ctx.project.strings))
}

pub fn add_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the send that was added"))?;
    let (place, slot) = strips::send(ctx, created, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let owner = Owner::Track(place);
    let bus = restore.applied["destination"].as_str().unwrap_or_default().to_string();
    let (label, sends) = strips::with_strip(ctx, owner, |ctx, strip| now(ctx, place, strip))?;
    let Some(send) = sends.sends.get(slot - 1).filter(|send| send.bus == bus).cloned() else {
        if !sends.busses().contains(&bus) {
            return Ok(Restored::by_value(
                json!({"removed": created["ref"]}),
                json!({"title": format!("The send to {bus} is off {} already", quoted(&label))}),
            ));
        }
        return Err(changed(format!("the sends of {} have been rearranged in Logic Pro since Kumi added one", quoted(&label))));
    };
    if !restore.discard && !same_db(send.level, restore.applied["level"].as_f64()) && restore.applied["level"].is_number() {
        return Err(changed(format!("the send to {bus} has been changed in Logic Pro since Kumi added it")));
    }
    // Logic's own undo while the send is its latest step: it takes the aux it made away too.
    if watch.top_is(&restore.key) {
        let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, STEP, None)?;
        let (_, after) = strips::with_strip(ctx, owner, |ctx, strip| now(ctx, place, strip))?;
        if after.busses().len() >= sends.busses().len() {
            return Err(super::uncertain(format!(
                "Logic Pro's {pressed:?} was pressed, but the send to {bus} is still on {}",
                quoted(&label)
            )));
        }
        return Ok(Restored {
            restored: json!({"removed": created["ref"], "pressed": pressed}),
            display: json!({"title": format!("The send from {} to {bus} taken away again", quoted(&label)), "note": "Taken back with Logic Pro's own undo, with the aux it made."}),
            undone_in_logic: true,
        });
    }
    super::front::ensure(ctx.ax)?;
    let after = strips::with_strip(ctx, owner, |ctx, _| take_off(ctx, owner, &send))?;
    if after.busses().len() + 1 != sends.busses().len() {
        return Err(super::uncertain(format!("Logic Pro's strip {} still shows the send to {bus}", quoted(&label))));
    }
    let mut display = json!({"title": format!("The send from {} to {bus} taken away again", quoted(&label))});
    if let Some(aux) = created["aux"].as_str() {
        display["note"] = format!("The aux {} Logic Pro made for it stays in the Mixer.", quoted(aux)).into();
    }
    Ok(Restored::by_value(json!({"removed": created["ref"]}), display))
}

// ----- remove -----

fn found_send(ctx: &mut Ctx, input: &Value, epoch: Epoch) -> Done<(usize, usize, String, Send, Sends)> {
    let (place, slot) = strips::send(ctx, input, "ref", epoch)?;
    let (label, sends) = strips::with_strip(ctx, Owner::Track(place), |ctx, strip| now(ctx, place, strip))?;
    let send = sends.sends.get(slot - 1).cloned().ok_or_else(|| crate::refs::not_in_project(input["ref"].as_str().unwrap_or_default()))?;
    Ok((place, slot, label, send, sends))
}

pub fn remove_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (place, slot, label, send, sends) = found_send(ctx, input, Epoch::Current)?;
    let state = json!({"destination": send.bus, "level": send.level, "slot": slot});
    Ok(Preview {
        target: send_target(ctx, place, slot),
        prior: state.clone(),
        fence: json!({"send": state, "sends": sends.busses()}),
        display: json!({"title": format!("Removed the send from {} to {}", quoted(&label), send.bus)}),
    })
}

pub fn remove_apply(ctx: &mut Ctx, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
    let (place, slot, label, send, sends) = found_send(ctx, input, Epoch::Current).map_err(strips::went)?;
    let state = json!({"destination": send.bus, "level": send.level, "slot": slot});
    if json!({"send": state, "sends": sends.busses()}) != *fence {
        return Err(changed(format!("the sends of {} changed since the preview", quoted(&label))));
    }
    super::front::ensure(ctx.ax)?;
    let owner = Owner::Track(place);
    let after = strips::with_strip(ctx, owner, |ctx, strip| {
        let (_, sends) = now(ctx, place, strip)?;
        let send = sends.sends.get(slot - 1).cloned().ok_or_else(|| changed("the send went since the preview"))?;
        take_off(ctx, owner, &send)
    })?;
    let mut expected = sends.busses();
    expected.remove(slot - 1);
    if after.busses() != expected {
        return Err(super::uncertain(format!(
            "Logic Pro's strip {} shows the sends {:?} after Kumi took one off",
            quoted(&label),
            after.busses()
        )));
    }
    let mut material = state.clone();
    material["step"] = desc.into();
    Ok(Applied {
        applied: json!({"removed": true, "sends": after.busses()}),
        material: Some(material),
        display: json!({"title": format!("Removed the send from {} to {}", quoted(&label), send.bus)}),
        step: Some(STEP),
        probe: Some(Probe::Strip { place, part: StripPart::Sends, prior: sends.busses(), applied: after.busses() }),
        ..Applied::default()
    })
}

pub fn remove_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the send that was removed"))?;
    let bus = material["destination"].as_str().unwrap_or_default().to_string();
    let (place, _) = strips::send(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let owner = Owner::Track(place);
    let (label, sends) = strips::with_strip(ctx, owner, |ctx, strip| now(ctx, place, strip))?;
    if sends.busses().contains(&bus) {
        return Ok(Restored::by_value(
            json!({"destination": bus}),
            json!({"title": format!("The send to {bus} is on {} already", quoted(&label))}),
        ));
    }
    if watch.top_is(&restore.key) {
        let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, STEP, material["step"].as_str())?;
        let (_, after) = strips::with_strip(ctx, owner, |ctx, strip| now(ctx, place, strip))?;
        if !after.busses().contains(&bus) {
            return Err(super::uncertain(format!(
                "Logic Pro's {pressed:?} was pressed, but the send to {bus} isn't back on {}",
                quoted(&label)
            )));
        }
        return Ok(Restored {
            restored: json!({"destination": bus, "pressed": pressed}),
            display: json!({"title": format!("The send from {} to {bus} put back", quoted(&label)), "note": "Put back with Logic Pro's own undo."}),
            undone_in_logic: true,
        });
    }
    // By value: a send to the same bus at the level it had.
    let number = bus_of(&json!({"bus": bus}))?;
    let mut input = json!({"ref": owner.text(ctx.project.epoch), "bus": number});
    if let Some(level) = material["level"].as_f64() {
        input["volumeDb"] = level.into();
    }
    let fence = json!({"sends": sends.busses()});
    let applied = add_apply(ctx, &input, &fence)?;
    Ok(Restored::by_value(
        json!({"destination": bus, "ref": applied.created.map(|c| c["ref"].clone()).unwrap_or(Value::Null)}),
        json!({"title": format!("The send from {} to {bus} put back", quoted(&label)), "note": "Put back by value: Logic Pro's own undo had other steps on top."}),
    ))
}

// ----- volume -----

pub fn volume_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (place, slot, label, send, _) = found_send(ctx, input, Epoch::Current)?;
    let db = level_of(input)?.ok_or_else(|| bad("volumeDb: the send's level in dB"))?;
    let now = json!({"level": send.level});
    let mut display = json!({"title": format!("Send from {} to {} at {} dB", quoted(&label), send.bus, db_text(db)), "before": now, "after": {"level": db}});
    if same_db(send.level, Some(db)) {
        display["unchanged"] = true.into();
    }
    Ok(Preview {
        target: send_target(ctx, place, slot),
        prior: now.clone(),
        fence: json!({"send": send.bus, "level": send.level}),
        display,
    })
}

pub fn volume_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (place, slot, label, send, _) = found_send(ctx, input, Epoch::Current).map_err(strips::went)?;
    let db = level_of(input)?.ok_or_else(|| bad("volumeDb: the send's level in dB"))?;
    if send.bus != fence["send"].as_str().unwrap_or_default() || !same_db(send.level, fence["level"].as_f64()) {
        return Err(changed(format!("the send from {} to {} changed since the preview", quoted(&label), send.bus)));
    }
    let owner = Owner::Track(place);
    let reached = if same_db(send.level, Some(db)) {
        Level { db: send.level.unwrap_or(db), close: CLOSE_DB }
    } else {
        strips::with_strip(ctx, owner, |ctx, strip| {
            let (_, sends) = now(ctx, place, strip)?;
            let knob = sends.sends.get(slot - 1).and_then(|s| s.knob.clone()).ok_or_else(|| failed("the send shows no level knob"))?;
            set_level(ctx, &knob, db)
        })?
    };
    if !reached.holds(db) {
        return Err(nudge::kept(&format!("the send to {}'s level", send.bus), db_text(db), db_text(reached.db)));
    }
    let reached = reached.db;
    Ok(Applied {
        applied: json!({"level": reached}),
        display: json!({"title": format!("Send from {} to {} at {} dB", quoted(&label), send.bus, db_text(reached)), "before": {"level": send.level}, "after": {"level": reached}}),
        ..Applied::default()
    })
}

pub fn volume_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let (place, slot, label, send, _) =
        found_send(ctx, restore.target, Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let prior = restore.prior["level"].as_f64();
    if same_db(send.level, prior) {
        return Ok(Restored::by_value(json!({"level": send.level}), json!({"title": format!("The send to {} is where it was", send.bus)})));
    }
    if !restore.discard && !same_db(send.level, restore.applied["level"].as_f64()) {
        return Err(changed(format!("the send to {} has been changed in Logic Pro since Kumi set it", send.bus)));
    }
    let db = prior.unwrap_or(crate::rows::MIN_DB);
    let reached = strips::with_strip(ctx, Owner::Track(place), |ctx, strip| {
        let (_, sends) = now(ctx, place, strip)?;
        let knob = sends.sends.get(slot - 1).and_then(|s| s.knob.clone()).ok_or_else(|| failed("the send shows no level knob"))?;
        set_level(ctx, &knob, db)
    })?;
    if !reached.holds(db) {
        return Err(nudge::kept(&format!("the send to {}'s level", send.bus), db_text(db), db_text(reached.db)));
    }
    let reached = reached.db;
    Ok(Restored::by_value(
        json!({"level": reached}),
        json!({"title": format!("Send from {} to {} at {} dB (put back)", quoted(&label), send.bus, db_text(reached))}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strips_sends_are_its_bus_groups_with_the_knobs_after_them() {
        // As Logic Pro 12.3.1 lists two sends, bottom-up: the empty slot, the groups, a stray send button, then the
        // knobs in the groups' order.
        let strip = Node::from_json(&json!({"role": "AXLayoutItem", "desc": "Vocal", "c": [
            {"i": "0", "role": "AXTextField", "desc": "name", "value": "Vocal"},
            {"i": "11", "role": "AXButton", "desc": "Stereo Output", "help": "Output slot. Click and hold."},
            {"i": "12", "role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal to an aux channel strip."},
            {"i": "13", "role": "AXGroup", "desc": "Bus 3", "c": [{"i": "13.0", "role": "AXCheckBox", "desc": "bypass", "value": false}, {"i": "13.1", "role": "AXButton", "desc": "list"}]},
            {"i": "14", "role": "AXGroup", "desc": "Bus 2", "c": [{"i": "14.0", "role": "AXCheckBox", "desc": "bypass", "value": false}, {"i": "14.1", "role": "AXButton", "desc": "list"}]},
            {"i": "15", "role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal to an aux channel strip."},
            {"i": "16", "role": "AXSlider", "desc": "send knob", "value": 0, "valueDesc": "-∞"},
            {"i": "17", "role": "AXSlider", "desc": "send knob", "value": 178427952, "valueDesc": "-37.1"},
            {"i": "18", "role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."}
        ]}));
        let sends = Sends::of(&strip, &Strings::english());
        assert_eq!(sends.busses(), ["Bus 2", "Bus 3"]);
        assert_eq!(sends.sends[0].knob.as_ref().map(|k| k.path.as_str()), Some("17"));
        assert_eq!((sends.sends[0].level, sends.sends[1].level), (Some(-37.1), Some(crate::rows::MIN_DB)));
        assert_eq!(sends.empty.as_ref().map(|e| e.path.as_str()), Some("12"));
        assert_eq!(bus_of(&json!({"bus": 2})).unwrap(), 2);
        assert_eq!(bus_of(&json!({"toRef": "Bus 3"})).unwrap(), 3);
        assert!(bus_of(&json!({"toRef": "1:track:2"})).is_err());
        assert!(bus_of(&json!({})).is_err());
        assert_eq!(level_of(&json!({"volumeDb": -6})).unwrap(), Some(-6.0));
        assert!(level_of(&json!({"volumeDb": 9})).is_err());
        assert!(same_db(Some(-6.0), Some(-6.04)) && !same_db(Some(-6.0), Some(-6.2)) && same_db(Some(-120.0), Some(-96.0)));
    }
}
