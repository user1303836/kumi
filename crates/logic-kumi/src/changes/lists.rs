//! The tempo map and the time signatures, through the List Editors' Tempo and Signature tabs (which M2 reads):
//!
//! - `tempo-marker-set`: the Tempo List's row at the position, its Tempo cell's segments stepped (three sliders
//!   carry the whole BPM, four the decimals), or a row made first with "Create new Tempo Event" at the playhead
//!   (moved there and back) and stepped from the tempo it comes with. Read back from the rows.
//! - `tempo-marker-delete`: the row's Delete action; the project's first tempo (bar 1) stays.
//! - `signature-set`: the Signature List's Time row at the position, its numerator slider stepped and its
//!   denominator chosen from the pop-up beside it, or a row made first with "Create new Time/Key Signature Change"
//!   (the list's type pop-up on Time). A signature row has no Delete action of its own: a row Kumi made goes
//!   through the list's Edit > Delete, selected.
//!
//! A restore is by value where the row was there before; a row Kumi made is deleted again. The cells of these two
//! lists are rebuilt by every step, so a step is read back at its path rather than around the step (see `nudge`).

use std::time::Duration;

use serde_json::{json, Value};

use super::marks::{segments, DELETE_ACTION};
use super::nudge::{self, Control};
use super::{changed, timeline, Applied, Ctx, Done, Preview, Restore, Restored, Watch};
use crate::ax::{paths, Node};
use crate::project::ListOpen;
use crate::refs::{bad, failed};
use crate::rows::{self, Position, SignatureEvent};

const LIST_SETTLE: Duration = Duration::from_millis(250);
const LIST_SETTLE_MS: u64 = 250;
const TEMPO_MIN: f64 = 5.0;
const TEMPO_MAX: f64 = 990.0;
const DENOMINATORS: [u32; 6] = [1, 2, 4, 8, 16, 32];

/// Presses an item of the shown list's Edit menu, `["Select", "All"]` going into its Select submenu: the menu
/// opened from its button in the list's header, read with its submenus, the last item named pressed. The menu is
/// closed again when an item isn't there.
pub(super) fn press_edit_menu(ctx: &mut Ctx, titles: &[&str]) -> Done<()> {
    let header = nudge::read(ctx.project.list_header(ctx.ax))?;
    let strings = &ctx.project.strings;
    let edit = header
        .find(&|n| n.role == "AXMenuButton" && strings.is(&n.desc, "Edit"))
        .ok_or_else(|| failed("Logic Pro's list shows no Edit menu"))?;
    let edit = Control::new(&ctx.project.root, &edit.path);
    nudge::wrote(ctx.ax.action(&edit.root, &edit.path, "AXPress", LIST_SETTLE_MS))?;
    let menu = nudge::wrote(ctx.ax.tree_in_popup(&edit.root, &edit.path, 2 * titles.len(), 60, Some(&["role", "title"])))?;
    let strings = &ctx.project.strings;
    let mut node = &menu;
    for title in titles {
        let Some(item) = node.find(&|n| n.role == "AXMenuItem" && strings.is(&n.title, title)) else {
            if ctx.ax.action_in_popup(&edit.root, &format!("{}.0", edit.path), "AXCancel", 100).is_err() {
                let _ = ctx.ax.keys("escape", &[]);
            }
            return Err(failed(format!("Logic Pro's list Edit menu has no {}", titles.join(" > "))));
        };
        node = item;
    }
    nudge::wrote(ctx.ax.action_in_popup(&edit.root, &node.path, "AXPress", LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    Ok(())
}

/// A list tab shown for a run of reads and writes.
struct Shown {
    open: ListOpen,
    beats_per_bar: u32,
    columns: Vec<(String, usize)>,
}

fn show(ctx: &mut Ctx, tab: &'static str) -> Done<Shown> {
    let (_, transport) = timeline::control_bar(ctx)?;
    let open = nudge::read(ctx.project.show_list(ctx.ax, tab))?;
    let (columns, _) = nudge::read(ctx.project.list_table(ctx.ax, tab))?;
    Ok(Shown { open, beats_per_bar: transport.beats_per_bar(), columns })
}

fn hide(ctx: &mut Ctx, shown: Shown) {
    let _ = ctx.project.hide_list(ctx.ax, shown.open);
    ctx.project.forget();
}

fn position_input(input: &Value, beats_per_bar: u32) -> Done<Position> {
    if input.get("position").is_some_and(|v| !v.is_null()) && input.get("positionBeats").is_none_or(Value::is_null) {
        return Err(bad(
            "Logic Pro's bridge places tempo and signature changes by positionBeats (beats from the project's start), not seconds",
        ));
    }
    let beats = input
        .get("positionBeats")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
        .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))?;
    Ok(Position::from_beats(beats, beats_per_bar))
}

/// The list's create button, by its label.
fn create_button(ctx: &mut Ctx, label: &str) -> Done<Control> {
    let group = nudge::read(ctx.project.list_group(ctx.ax))?;
    let strings = &ctx.project.strings;
    let button = group
        .find(&|n| n.role == "AXButton" && strings.is(&n.desc, label))
        .ok_or_else(|| failed(format!("Logic Pro's list doesn't show its {label} button")))?;
    Ok(Control::new(&ctx.project.root, &button.path))
}

/// Presses the create button with the playhead at a position (moved there and put back).
fn create_at(ctx: &mut Ctx, position: Position, label: &str) -> Done<()> {
    let (_, before) = timeline::control_bar(ctx)?;
    let was = before.position();
    timeline::set_playhead(ctx, Position { bar: position.bar, beat: position.beat, ..Position::START })?;
    let button = create_button(ctx, label)?;
    let pressed = nudge::wrote(nudge::press(ctx.ax, &button, LIST_SETTLE_MS));
    super::pause(LIST_SETTLE);
    let _ = timeline::set_playhead(ctx, was);
    pressed.map(|_| ())
}

fn on_the_beat(position: &Position) -> Position {
    Position { bar: position.bar, beat: position.beat, ..Position::START }
}

// ----- tempo events -----

struct TempoRow {
    position: Position,
    bpm: f64,
    node: Node,
}

fn tempo_rows(ctx: &mut Ctx, shown: &Shown) -> Done<Vec<TempoRow>> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Tempo"))?;
    let _ = &shown.columns;
    Ok(paths::rows(&table)
        .into_iter()
        .filter_map(|row| rows::tempo_of(row, &columns).map(|(position, bpm)| TempoRow { position, bpm, node: row.clone() }))
        .collect())
}

fn tempo_at(list: &[TempoRow], position: &Position) -> Option<usize> {
    list.iter().position(|row| row.position == *position)
}

fn tempo_state(row: Option<&TempoRow>) -> Value {
    match row {
        Some(row) => json!({"exists": true, "bpm": row.bpm}),
        None => json!({"exists": false}),
    }
}

fn same_tempo(a: &Value, b: &Value) -> bool {
    a["exists"] == b["exists"]
        && (a["exists"] == false || (a["bpm"].as_f64().unwrap_or(f64::NAN) - b["bpm"].as_f64().unwrap_or(f64::NAN)).abs() < 0.0001)
}

fn wanted_bpm(input: &Value) -> Done<f64> {
    let bpm = input.get("bpm").and_then(Value::as_f64).filter(|b| b.is_finite()).ok_or_else(|| bad("bpm must be a number"))?;
    if !(TEMPO_MIN..=TEMPO_MAX).contains(&bpm) {
        return Err(bad(format!("bpm must be from {TEMPO_MIN} to {TEMPO_MAX}")));
    }
    Ok((bpm * 10000.0).round() / 10000.0)
}

/// Steps a tempo row's cell to `bpm`: the whole part on the first segment, the four decimals on the last four.
fn set_tempo(ctx: &mut Ctx, row: &Node, columns: &[(String, usize)], now: f64, bpm: f64) -> Done<()> {
    let sliders = segments(ctx, row, columns, "Tempo", "the tempo event")?;
    let (whole_now, whole) = (now.floor() as i64, bpm.floor() as i64);
    if whole_now != whole {
        nudge::wrote(nudge::step_to_rebuilt(ctx.ax, &sliders[0], whole_now, whole))?;
    }
    let digits = |value: f64| -> [i64; 4] {
        let fraction = ((value - value.floor()) * 10000.0).round() as i64;
        [fraction / 1000 % 10, fraction / 100 % 10, fraction / 10 % 10, fraction % 10]
    };
    let (from, to) = (digits(now), digits(bpm));
    for (slot, control) in sliders.iter().skip(3).take(4).enumerate() {
        if from[slot] != to[slot] {
            nudge::wrote(nudge::step_to_rebuilt(ctx.ax, control, from[slot], to[slot]))?;
        }
    }
    super::pause(LIST_SETTLE);
    Ok(())
}

fn tempo_title(position: &Position, bpm: f64) -> String {
    format!("Tempo {} BPM at {}", marks_trim(bpm), timeline::bar_text(position))
}

fn marks_trim(bpm: f64) -> String {
    let text = format!("{bpm:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn tempo_set_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let bpm = wanted_bpm(input)?;
    let shown = show(ctx, "Tempo")?;
    let result = (|| {
        let position = on_the_beat(&position_input(input, shown.beats_per_bar)?);
        let list = tempo_rows(ctx, &shown)?;
        let now = tempo_state(tempo_at(&list, &position).map(|i| &list[i]));
        let mut display = json!({"title": tempo_title(&position, bpm), "before": now, "after": {"exists": true, "bpm": bpm}});
        if same_tempo(&now, &json!({"exists": true, "bpm": bpm})) {
            display["unchanged"] = true.into();
        }
        Ok(Preview {
            target: json!({"ref": null, "kind": "tempo-marker", "positionBeats": position.beats(shown.beats_per_bar)}),
            prior: now.clone(),
            fence: json!({"tempo": now, "events": list.len()}),
            display,
        })
    })();
    hide(ctx, shown);
    result
}

/// Sets the tempo at a position: the row there stepped, or one made and stepped. The state after, and whether a
/// row was made.
fn put_tempo(ctx: &mut Ctx, shown: &Shown, position: Position, bpm: f64) -> Done<(Value, bool)> {
    let list = tempo_rows(ctx, shown)?;
    let made = match tempo_at(&list, &position) {
        Some(index) => {
            let row = &list[index];
            if (row.bpm - bpm).abs() >= 0.0001 {
                set_tempo(ctx, &row.node, &shown.columns, row.bpm, bpm)?;
            }
            false
        }
        None => {
            create_at(ctx, position, "Create new Tempo Event")?;
            let after = tempo_rows(ctx, shown).map_err(|(_, why)| super::uncertain(why))?;
            let index = tempo_at(&after, &position).ok_or_else(|| {
                super::uncertain(format!(
                    "Logic Pro's Tempo List shows no event at {} after Kumi asked for one",
                    timeline::bar_text(&position)
                ))
            })?;
            let row = &after[index];
            if (row.bpm - bpm).abs() >= 0.0001 {
                set_tempo(ctx, &row.node, &shown.columns, row.bpm, bpm)?;
            }
            true
        }
    };
    let after = tempo_rows(ctx, shown).map_err(|(_, why)| super::uncertain(why))?;
    Ok((tempo_state(tempo_at(&after, &position).map(|i| &after[i])), made))
}

pub fn tempo_set_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let bpm = wanted_bpm(input)?;
    let shown = show(ctx, "Tempo")?;
    let result = (|| {
        let position = on_the_beat(&position_input(input, shown.beats_per_bar)?);
        let list = tempo_rows(ctx, &shown)?;
        let now = tempo_state(tempo_at(&list, &position).map(|i| &list[i]));
        if json!({"tempo": now, "events": list.len()}) != *fence {
            return Err(changed("the tempo events changed since the preview"));
        }
        let wanted = json!({"exists": true, "bpm": bpm});
        if same_tempo(&now, &wanted) {
            return Ok(Applied {
                applied: now.clone(),
                display: json!({"title": tempo_title(&position, bpm), "unchanged": true}),
                ..Applied::default()
            });
        }
        let (after, made) = put_tempo(ctx, &shown, position, bpm)?;
        if !same_tempo(&after, &wanted) {
            return Err(nudge::kept(&format!("the tempo at {}", timeline::bar_text(&position)), bpm, after["bpm"].clone()));
        }
        let mut applied = after.clone();
        applied["made"] = made.into();
        Ok(Applied {
            applied,
            display: json!({"title": tempo_title(&position, bpm), "before": now, "after": after}),
            step: Some(if made { "Create Tempo Event" } else { "Value Change" }),
            ..Applied::default()
        })
    })();
    hide(ctx, shown);
    result
}

pub fn tempo_set_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let shown = show(ctx, "Tempo")?;
    let result = (|| {
        let beats = restore.target["positionBeats"].as_f64().ok_or_else(|| bad("target.positionBeats"))?;
        let position = on_the_beat(&Position::from_beats(beats, shown.beats_per_bar));
        let list = tempo_rows(ctx, &shown)?;
        let now = tempo_state(tempo_at(&list, &position).map(|i| &list[i]));
        let title = format!("Tempo at {} put back", timeline::bar_text(&position));
        if same_tempo(&now, restore.prior) {
            return Ok(Restored::by_value(now, json!({"title": title})));
        }
        if !restore.discard && !same_tempo(&now, restore.applied) {
            return Err(changed(format!("the tempo at {} has been changed in Logic Pro since Kumi set it", timeline::bar_text(&position))));
        }
        if restore.prior["exists"] == true {
            let (after, _) = put_tempo(ctx, &shown, position, restore.prior["bpm"].as_f64().unwrap_or(120.0))?;
            if !same_tempo(&after, restore.prior) {
                return Err(nudge::kept(
                    &format!("the tempo at {}", timeline::bar_text(&position)),
                    restore.prior["bpm"].clone(),
                    after["bpm"].clone(),
                ));
            }
            return Ok(Restored::by_value(after, json!({"title": title})));
        }
        // Kumi made the row: taken away again.
        let Some(index) = tempo_at(&list, &position) else {
            return Ok(Restored::by_value(json!({"exists": false}), json!({"title": title})));
        };
        nudge::wrote(ctx.ax.action(&ctx.project.root, &list[index].node.path, DELETE_ACTION, LIST_SETTLE_MS))?;
        super::pause(LIST_SETTLE);
        let after = tempo_rows(ctx, &shown).map_err(|(_, why)| super::uncertain(why))?;
        if tempo_at(&after, &position).is_some() {
            return Err(super::uncertain(format!(
                "Logic Pro's Tempo List still shows an event at {} after Kumi asked to delete it",
                timeline::bar_text(&position)
            )));
        }
        Ok(Restored::by_value(
            json!({"exists": false}),
            json!({"title": format!("Tempo event at {} taken away again", timeline::bar_text(&position))}),
        ))
    })();
    hide(ctx, shown);
    result
}

pub fn tempo_delete_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let shown = show(ctx, "Tempo")?;
    let result = (|| {
        let position = on_the_beat(&position_input(input, shown.beats_per_bar)?);
        let list = tempo_rows(ctx, &shown)?;
        let index =
            tempo_at(&list, &position).ok_or_else(|| failed(format!("there is no tempo event at {}", timeline::bar_text(&position))))?;
        if index == 0 {
            return Err(bad("the project's first tempo event stays: change it with tempo_marker_set"));
        }
        let now = tempo_state(Some(&list[index]));
        Ok(Preview {
            target: json!({"ref": null, "kind": "tempo-marker", "positionBeats": position.beats(shown.beats_per_bar)}),
            prior: now.clone(),
            fence: json!({"tempo": now, "events": list.len()}),
            display: json!({"title": format!("Deleted tempo event at {}", timeline::bar_text(&position))}),
        })
    })();
    hide(ctx, shown);
    result
}

pub fn tempo_delete_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let shown = show(ctx, "Tempo")?;
    let result = (|| {
        let position = on_the_beat(&position_input(input, shown.beats_per_bar)?);
        let list = tempo_rows(ctx, &shown)?;
        let index = tempo_at(&list, &position).ok_or_else(|| changed("the tempo event went since the preview"))?;
        let now = tempo_state(Some(&list[index]));
        if json!({"tempo": now, "events": list.len()}) != *fence {
            return Err(changed("the tempo events changed since the preview"));
        }
        nudge::wrote(ctx.ax.action(&ctx.project.root, &list[index].node.path, DELETE_ACTION, LIST_SETTLE_MS))?;
        super::pause(LIST_SETTLE);
        let after = tempo_rows(ctx, &shown).map_err(|(_, why)| super::uncertain(why))?;
        if after.len() != list.len() - 1 || tempo_at(&after, &position).is_some() {
            return Err(super::uncertain(format!("Logic Pro's Tempo List shows {} events after Kumi asked to delete one", after.len())));
        }
        Ok(Applied {
            applied: json!({"deleted": true}),
            material: Some(json!({"positionBeats": position.beats(shown.beats_per_bar), "bpm": now["bpm"]})),
            display: json!({"title": format!("Deleted tempo event at {}", timeline::bar_text(&position))}),
            step: Some("Delete"),
            ..Applied::default()
        })
    })();
    hide(ctx, shown);
    result
}

pub fn tempo_delete_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let bpm = material["bpm"].as_f64().ok_or_else(|| bad("material.bpm"))?;
    let shown = show(ctx, "Tempo")?;
    let result = (|| {
        let position = on_the_beat(&Position::from_beats(material["positionBeats"].as_f64().unwrap_or(0.0), shown.beats_per_bar));
        let list = tempo_rows(ctx, &shown)?;
        let title = format!("Tempo event at {} put back", timeline::bar_text(&position));
        if let Some(index) = tempo_at(&list, &position) {
            if restore.discard || (list[index].bpm - bpm).abs() < 0.0001 {
                return Ok(Restored::by_value(tempo_state(Some(&list[index])), json!({"title": format!("{title} already")})));
            }
            return Err(changed(format!("another tempo event is at {} now", timeline::bar_text(&position))));
        }
        if let Ok(pressed) = super::structure::undo_in_logic(ctx, watch, &restore.key, "Delete", None) {
            let after = tempo_rows(ctx, &shown).map_err(|(_, why)| super::uncertain(why))?;
            return match tempo_at(&after, &position) {
                Some(index) => Ok(Restored {
                    restored: tempo_state(Some(&after[index])),
                    display: json!({"title": title, "note": "Put back with Logic Pro's own undo."}),
                    undone_in_logic: true,
                }),
                None => Err(super::uncertain(format!(
                    "Logic Pro's {pressed:?} was pressed, but no tempo event is back at {}",
                    timeline::bar_text(&position)
                ))),
            };
        }
        let (after, _) = put_tempo(ctx, &shown, position, bpm)?;
        if !same_tempo(&after, &json!({"exists": true, "bpm": bpm})) {
            return Err(nudge::kept(&format!("the tempo at {}", timeline::bar_text(&position)), bpm, after["bpm"].clone()));
        }
        Ok(Restored::by_value(after, json!({"title": title})))
    })();
    hide(ctx, shown);
    result
}

// ----- time signatures -----

struct SignatureRow {
    position: Position,
    numerator: u32,
    denominator: u32,
    node: Node,
}

fn signature_rows(ctx: &mut Ctx, shown: &Shown) -> Done<Vec<SignatureRow>> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Signature"))?;
    let _ = &shown.columns;
    let strings = &ctx.project.strings;
    Ok(paths::rows(&table)
        .into_iter()
        .filter_map(|row| match rows::signature_of(row, &columns, strings)? {
            SignatureEvent::Time { position, signature } => {
                let (n, d) = signature.split_once('/')?;
                Some(SignatureRow { position, numerator: n.trim().parse().ok()?, denominator: d.trim().parse().ok()?, node: row.clone() })
            }
            SignatureEvent::Key { .. } => None,
        })
        .collect())
}

fn signature_at(list: &[SignatureRow], position: &Position) -> Option<usize> {
    list.iter().position(|row| row.position == *position)
}

fn signature_state(row: Option<&SignatureRow>) -> Value {
    match row {
        Some(row) => json!({"exists": true, "timeSignature": [row.numerator, row.denominator]}),
        None => json!({"exists": false}),
    }
}

fn signature_input(input: &Value) -> Done<(u32, u32)> {
    let pair = input.get("timeSignature").ok_or_else(|| bad("timeSignature must be [numerator, denominator]"))?;
    let (n, d) = match pair {
        Value::Array(pair) if pair.len() == 2 => (pair[0].as_u64(), pair[1].as_u64()),
        Value::Object(_) => (pair["num"].as_u64(), pair["den"].as_u64()),
        _ => (None, None),
    };
    match (n, d) {
        (Some(n), Some(d)) if (1..=99).contains(&n) && DENOMINATORS.contains(&(d as u32)) => Ok((n as u32, d as u32)),
        _ => Err(bad("timeSignature must be [numerator, denominator]: 1 to 99 over 1, 2, 4, 8, 16 or 32")),
    }
}

fn signature_title(position: &Position, n: u32, d: u32) -> String {
    format!("Time signature {n}/{d} at {}", timeline::bar_text(position))
}

/// The row's numerator slider and denominator pop-up.
fn signature_controls(ctx: &mut Ctx, row: &Node, columns: &[(String, usize)]) -> Done<(Control, Control)> {
    let index =
        columns.iter().find(|(name, _)| name == "Val").map(|(_, i)| *i).ok_or_else(|| failed("the Signature List has no Val column"))?;
    let cell =
        row.children.iter().filter(|c| c.role == "AXCell").nth(index).ok_or_else(|| failed("the signature's row has no Val cell"))?;
    let slider = cell.find(&|n| n.role == "AXSlider").ok_or_else(|| failed("the signature's row has no numerator slider"))?;
    let popup = cell.find(&|n| n.role == "AXPopUpButton").ok_or_else(|| failed("the signature's row has no denominator pop-up"))?;
    Ok((Control::new(&ctx.project.root, &slider.path), Control::new(&ctx.project.root, &popup.path)))
}

/// Chooses an item of a pop-up by its title through its menu.
fn choose(ctx: &mut Ctx, popup: &Control, item: &str) -> Done<()> {
    nudge::wrote(ctx.ax.action(&popup.root, &popup.path, "AXPress", LIST_SETTLE_MS))?;
    let menu = nudge::wrote(ctx.ax.tree_in_popup(&popup.root, &popup.path, 2, 40, Some(&["role", "title"])))?;
    let strings = &ctx.project.strings;
    let Some(found) = menu.find(&|n| n.role == "AXMenuItem" && strings.is(&n.title, item)) else {
        let _ = ctx.ax.action_in_popup(&popup.root, &format!("{}.0", popup.path), "AXCancel", 100);
        return Err(failed(format!("Logic Pro's pop-up has no {item:?}")));
    };
    nudge::wrote(ctx.ax.action_in_popup(&popup.root, &found.path, "AXPress", LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    Ok(())
}

fn set_signature(ctx: &mut Ctx, row: &SignatureRow, columns: &[(String, usize)], n: u32, d: u32) -> Done<()> {
    let (slider, popup) = signature_controls(ctx, &row.node, columns)?;
    if row.numerator != n {
        nudge::wrote(nudge::step_to_rebuilt(ctx.ax, &slider, i64::from(row.numerator), i64::from(n)))?;
    }
    if row.denominator != d {
        choose(ctx, &popup, &format!("/{d}"))?;
    }
    super::pause(LIST_SETTLE);
    Ok(())
}

/// Deletes a signature row: selected, then the list's Edit > Delete.
fn delete_signature_row(ctx: &mut Ctx, row: &SignatureRow) -> Done<()> {
    nudge::wrote(ctx.ax.set(&ctx.project.root, &row.node.path, "AXSelected", json!(true), LIST_SETTLE_MS))?;
    press_edit_menu(ctx, &["Delete"])?;
    Ok(())
}

/// The list's type pop-up (Time or Key), set to Time for a new row.
fn type_time(ctx: &mut Ctx) -> Done<()> {
    let group = nudge::read(ctx.project.list_group(ctx.ax))?;
    let strings = &ctx.project.strings;
    let popup = group
        .children
        .first()
        .and_then(|g| {
            g.children
                .iter()
                .find(|n| n.role == "AXPopUpButton" && (strings.is(&n.value_text(), "Time") || strings.is(&n.value_text(), "Key")))
        })
        .ok_or_else(|| failed("Logic Pro's Signature List shows no type pop-up"))?;
    if strings.is(&popup.value_text(), "Time") {
        return Ok(());
    }
    let control = Control::new(&ctx.project.root, &popup.path);
    choose(ctx, &control, "Time")
}

pub fn signature_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (n, d) = signature_input(input)?;
    let shown = show(ctx, "Signature")?;
    let result = (|| {
        let position = on_the_beat(&position_input(input, shown.beats_per_bar)?);
        if position.beat != 1 {
            return Err(bad("a time signature changes at a bar: give the bar's first beat"));
        }
        let list = signature_rows(ctx, &shown)?;
        let now = signature_state(signature_at(&list, &position).map(|i| &list[i]));
        let wanted = json!({"exists": true, "timeSignature": [n, d]});
        let mut display = json!({"title": signature_title(&position, n, d), "before": now, "after": wanted});
        if now == wanted {
            display["unchanged"] = true.into();
        }
        Ok(Preview {
            target: json!({"ref": null, "kind": "signature", "positionBeats": position.beats(shown.beats_per_bar)}),
            prior: now.clone(),
            fence: json!({"signature": now, "events": list.len()}),
            display,
        })
    })();
    hide(ctx, shown);
    result
}

fn put_signature(ctx: &mut Ctx, shown: &Shown, position: Position, n: u32, d: u32) -> Done<(Value, bool)> {
    let list = signature_rows(ctx, shown)?;
    let made = match signature_at(&list, &position) {
        Some(index) => {
            set_signature(ctx, &list[index], &shown.columns, n, d)?;
            false
        }
        None => {
            type_time(ctx)?;
            create_at(ctx, position, "Create new Time/Key Signature Change")?;
            let after = signature_rows(ctx, shown).map_err(|(_, why)| super::uncertain(why))?;
            let index = signature_at(&after, &position).ok_or_else(|| {
                super::uncertain(format!(
                    "Logic Pro's Signature List shows no time signature at {} after Kumi asked for one",
                    timeline::bar_text(&position)
                ))
            })?;
            set_signature(ctx, &after[index], &shown.columns, n, d)?;
            true
        }
    };
    let after = signature_rows(ctx, shown).map_err(|(_, why)| super::uncertain(why))?;
    Ok((signature_state(signature_at(&after, &position).map(|i| &after[i])), made))
}

pub fn signature_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (n, d) = signature_input(input)?;
    let shown = show(ctx, "Signature")?;
    let result = (|| {
        let position = on_the_beat(&position_input(input, shown.beats_per_bar)?);
        let list = signature_rows(ctx, &shown)?;
        let now = signature_state(signature_at(&list, &position).map(|i| &list[i]));
        if json!({"signature": now, "events": list.len()}) != *fence {
            return Err(changed("the time signatures changed since the preview"));
        }
        let wanted = json!({"exists": true, "timeSignature": [n, d]});
        if now == wanted {
            return Ok(Applied {
                applied: now.clone(),
                display: json!({"title": signature_title(&position, n, d), "unchanged": true}),
                ..Applied::default()
            });
        }
        let (after, made) = put_signature(ctx, &shown, position, n, d)?;
        if after != wanted {
            return Err(nudge::kept(
                &format!("the time signature at {}", timeline::bar_text(&position)),
                format!("{n}/{d}"),
                after["timeSignature"].clone(),
            ));
        }
        let mut applied = after.clone();
        applied["made"] = made.into();
        Ok(Applied {
            applied,
            display: json!({"title": signature_title(&position, n, d), "before": now, "after": after}),
            step: Some(if made { "Create Signature Event" } else { "Value Change" }),
            ..Applied::default()
        })
    })();
    hide(ctx, shown);
    result
}

pub fn signature_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let shown = show(ctx, "Signature")?;
    let result = (|| {
        let beats = restore.target["positionBeats"].as_f64().ok_or_else(|| bad("target.positionBeats"))?;
        let position = on_the_beat(&Position::from_beats(beats, shown.beats_per_bar));
        let list = signature_rows(ctx, &shown)?;
        let now = signature_state(signature_at(&list, &position).map(|i| &list[i]));
        let title = format!("Time signature at {} put back", timeline::bar_text(&position));
        if now == *restore.prior {
            return Ok(Restored::by_value(now, json!({"title": title})));
        }
        let mut applied = restore.applied.clone();
        if let Some(fields) = applied.as_object_mut() {
            fields.remove("made");
        }
        if !restore.discard && now != applied {
            return Err(changed(format!(
                "the time signature at {} has been changed in Logic Pro since Kumi set it",
                timeline::bar_text(&position)
            )));
        }
        if restore.prior["exists"] == true {
            let (n, d) = signature_input(&json!({"timeSignature": restore.prior["timeSignature"]}))?;
            let (after, _) = put_signature(ctx, &shown, position, n, d)?;
            if after != *restore.prior {
                return Err(nudge::kept(
                    &format!("the time signature at {}", timeline::bar_text(&position)),
                    format!("{n}/{d}"),
                    after["timeSignature"].clone(),
                ));
            }
            return Ok(Restored::by_value(after, json!({"title": title})));
        }
        // Kumi made the row, and a signature row has no Delete action of its own: selected in the list, the list's
        // Edit menu deletes it; failing that, Logic's own undo while the step is on top.
        let index = signature_at(&list, &position).ok_or_else(|| super::uncertain("the time signature isn't in the list as it was"))?;
        let deleted = delete_signature_row(ctx, &list[index]);
        let after = signature_rows(ctx, &shown).map_err(|(_, why)| super::uncertain(why))?;
        if signature_at(&after, &position).is_none() {
            return Ok(Restored::by_value(
                json!({"exists": false}),
                json!({"title": format!("Time signature at {} taken away again", timeline::bar_text(&position))}),
            ));
        }
        deleted?;
        let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, "Value Change", None)
            .or_else(|_| super::structure::undo_in_logic(ctx, watch, &restore.key, "Create Signature Event", None))?;
        let after = signature_rows(ctx, &shown).map_err(|(_, why)| super::uncertain(why))?;
        if signature_at(&after, &position).is_some() {
            return Err(super::uncertain(format!(
                "Logic Pro's {pressed:?} was pressed, but a time signature is still at {}",
                timeline::bar_text(&position)
            )));
        }
        Ok(Restored {
            restored: json!({"exists": false, "pressed": pressed}),
            display: json!({"title": format!("Time signature at {} taken away again", timeline::bar_text(&position)), "note": "Put back with Logic Pro's own undo."}),
            undone_in_logic: true,
        })
    })();
    hide(ctx, shown);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempo_and_signature_inputs_are_checked_and_worded() {
        assert_eq!(wanted_bpm(&json!({"bpm": 120.5})).unwrap(), 120.5);
        assert_eq!(wanted_bpm(&json!({"bpm": 123.45678})).unwrap(), 123.4568);
        assert!(wanted_bpm(&json!({"bpm": 2})).is_err());
        assert_eq!(signature_input(&json!({"timeSignature": [3, 4]})).unwrap(), (3, 4));
        assert_eq!(signature_input(&json!({"timeSignature": {"num": 6, "den": 8}})).unwrap(), (6, 8));
        assert!(signature_input(&json!({"timeSignature": [3, 5]})).is_err());
        assert!(signature_input(&json!({})).is_err());
        let at = Position { bar: 5, ..Position::START };
        assert_eq!(tempo_title(&at, 120.0), "Tempo 120 BPM at bar 5");
        assert_eq!(tempo_title(&at, 120.25), "Tempo 120.25 BPM at bar 5");
        assert_eq!(signature_title(&at, 3, 4), "Time signature 3/4 at bar 5");
        assert!(same_tempo(&json!({"exists": true, "bpm": 120.0}), &json!({"exists": true, "bpm": 120.00001})));
        assert!(!same_tempo(&json!({"exists": false}), &json!({"exists": true, "bpm": 120.0})));
        assert_eq!(on_the_beat(&Position { bar: 2, beat: 3, division: 2, tick: 5 }), Position { bar: 2, beat: 3, division: 1, tick: 1 });
        assert_eq!(position_input(&json!({"positionBeats": 16}), 4).unwrap(), Position { bar: 5, ..Position::START });
        assert!(position_input(&json!({"position": 2.0}), 4).is_err());
    }
}
