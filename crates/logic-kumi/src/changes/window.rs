//! A plug-in's window, for its parameters and presets: opened from its slot's open button in the Mixer (the pane
//! opened for it and closed after), its view switched to Controls through the window's view menu (a standard menu,
//! read through the pop-up), read or written, then its view put back and the window closed. Measured on Logic Pro
//! 12.3.1: the window is a dialog titled after the strip ("Vocal"), its header holds a close button, the view menu
//! (Controls, Editor, the editor's sizes), the bypass, the setting pop-up (the preset's name: its menu is Logic's
//! own, which Accessibility can't read), the previous and next setting buttons and the window's own undo and redo;
//! the Controls View is a table with a row per parameter: its name ("Gain:"), its value as text, and a slider that
//! takes an exact value (or radio buttons for a choice, a checkbox for a switch). A change made there is a step of the
//! plug-in window's own undo, never of the project's: Logic's Edit menu says a plain "Undo" while the window is up.
//! A narrow header hides what doesn't fit: the Controls View's header has no undo and redo, and a small editor (a
//! Gain at 75%) not even the setting buttons, so a preset is changed in the editor, made a size larger at a time
//! until the header shows the button it needs (the size put back after).
//!
//! The Compressor's window (measured on Logic Pro 12.3.1, Scratch's Keys) shows what a plug-in with an editor of its
//! own and a side chain does: its editor's controls (knobs, the circuit type buttons, their fields) sit at the
//! window's top level with the header's, 44 in all, the texts naming the plug-in and the strip last, so a header read
//! cut at 40 is made again whole; the header lists the Side Chain pop-up ("Internal") before the setting pop-up
//! ("Custom"); the editor keeps a size of its own besides the view menu's (71%); and its Controls View lists 29 rows,
//! some a value it shows with no control to set it (Knee "0.7", Peak/RMS, the side chain filter's Gain, Gain
//! Reduction: a disabled slider in the text's group), which read as a parameter Kumi can't set.

use std::time::Duration;

use serde_json::{json, Value};

use super::nudge;
use super::strips::{self, Inserts, Owner};
use super::{quoted, Ctx, Done};
use crate::ax::{Node, Root};
use crate::refs::failed;

/// How long the window gets to come up after its open button, and the view to change.
const OPEN_WAIT: Duration = Duration::from_millis(2500);
const OPEN_POLL: Duration = Duration::from_millis(150);
const VIEW_SETTLE_MS: u64 = 500;
const MENU_SETTLE_MS: u64 = 300;
/// The view that lists every parameter.
const CONTROLS: &str = "Controls";
const EDITOR: &str = "Editor";
/// The editor's sizes, as the view menu lists them (Logic Pro 12.3.1); an editor may show a size of its own too.
const SIZES: [&str; 7] = ["50%", "75%", "100%", "125%", "150%", "175%", "200%"];
/// The window's top-level children a header read lists first: enough for a plug-in whose editor keeps its controls
/// below the header (Channel EQ's, the Gain's); a read cut there is made again with all of them.
const HEADER_CHILDREN: usize = 40;

/// The view a kind shows the window in: the Controls View (the parameters), or the editor (a preset's buttons and the
/// window's own undo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Controls,
    Editor,
}

/// A plug-in window shown for a read or a write: its title, the view it had, and whether Kumi opened it.
#[derive(Debug, Clone)]
pub struct Shown {
    pub root: Root,
    pub plugin: String,
    pub strip: String,
    view_was: String,
    opened: bool,
}

/// A parameter of the Controls View.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// From 1, in the view's order.
    pub index: usize,
    pub name: String,
    pub kind: Kind,
    /// The control a write sets: the slider, the checkbox; the choices' radio buttons for a choice.
    pub path: String,
    pub value: Option<f64>,
    pub text: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// A choice's options: their titles and paths, the one on first.
    pub choices: Vec<(String, String)>,
    /// Where a slider's text is read: the group beside it whose value is the text.
    text_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Slider,
    Choice,
    Switch,
    Menu,
    /// A value the view shows with no control to set it (the Compressor's Knee): its text alone.
    Readout,
}

impl Param {
    /// The parameter as a row: `ref`, `pluginRef`, `index`, `name`, `value`, `text`, `min`, `max`, `normalized`.
    pub fn row(&self, reference: &str, plugin: &str) -> Value {
        json!({"ref": reference, "pluginRef": plugin, "index": self.index, "name": self.name, "value": self.value, "text": self.text, "min": self.min, "max": self.max, "normalized": self.normalized()})
    }

    /// The value as a share of its range, when it has one.
    pub fn normalized(&self) -> Option<f64> {
        let (value, min, max) = (self.value?, self.min?, self.max?);
        (max > min).then(|| ((value - min) / (max - min) * 1e6).round() / 1e6)
    }
}

/// The windows Logic shows now: their titles and subroles.
fn windows(ctx: &mut Ctx) -> Done<Vec<(String, String)>> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    Ok(windows.into_iter().map(|window| (window.title, window.subrole)).collect())
}

/// The window's top level: its header controls and the static texts naming the plug-in and the strip. A plug-in whose
/// editor puts its own controls there (the Compressor's 44) has those texts past the first read, so a read cut short
/// is made again with every child.
fn header(ctx: &mut Ctx, root: &Root) -> Done<Node> {
    const ATTRS: [&str; 5] = ["role", "subrole", "title", "desc", "value"];
    let first = nudge::read(ctx.ax.tree(root, "", 1, HEADER_CHILDREN, Some(&ATTRS)))?;
    if first.more == 0 {
        return Ok(first);
    }
    nudge::read(ctx.ax.tree(root, "", 1, crate::ax::MAX_CHILDREN, Some(&ATTRS)))
}

/// Whether a window's static texts name the plug-in (the window shows one plug-in at a time): its slot's name, or a
/// name holding it (an instrument's window says "Studio Piano" where its slot says "Piano").
fn shows(window: &Node, plugin: &str) -> bool {
    let wanted = plugin.to_lowercase();
    window.children.iter().any(|c| c.role == "AXStaticText" && c.value_text().to_lowercase().contains(&wanted))
}

/// The view menu button: its path and what it shows (Controls, or the editor's size).
fn view_button(window: &Node, strings: &crate::ax::strings::Strings) -> Option<(String, String)> {
    window.children.iter().find(|c| c.role == "AXMenuButton" && strings.is(&c.desc, "view")).map(|c| (c.path.clone(), c.title.clone()))
}

/// Chooses an item of the view menu.
fn choose_view(ctx: &mut Ctx, root: &Root, button: &str, item: &str) -> Done<()> {
    nudge::wrote(ctx.ax.action(root, button, "AXPress", MENU_SETTLE_MS))?;
    let menu = nudge::wrote(ctx.ax.tree_in_popup(root, button, 3, 40, Some(&["role", "title"])))?;
    let strings = &ctx.project.strings;
    let Some(found) = menu.find(&|n| n.role == "AXMenuItem" && strings.is(&n.title, item)) else {
        if let Some(open) = menu.find(&|n| n.role == "AXMenu") {
            let path = open.path.clone();
            let _ = ctx.ax.action_in_popup(root, &path, "AXCancel", 100);
        }
        return Err(failed(format!("the plug-in window's view menu has no {item}")));
    };
    let path = found.path.clone();
    nudge::wrote(ctx.ax.action_in_popup(root, &path, "AXPress", VIEW_SETTLE_MS))?;
    Ok(())
}

/// Opens the window of the plug-in at a slot (from the top, the instrument first) and shows its Controls View.
pub fn show(ctx: &mut Ctx, owner: Owner, slot: usize) -> Done<Shown> {
    show_in(ctx, owner, slot, View::Controls)
}

/// Opens the window of the plug-in at a slot in a view.
pub fn show_in(ctx: &mut Ctx, owner: Owner, slot: usize, view: View) -> Done<Shown> {
    let before = windows(ctx)?;
    let (plugin, strip_name) = strips::with_strip(ctx, owner, |ctx, strip| {
        let inserts = Inserts::of(strip, &ctx.project.strings);
        let label = strips::label(ctx, owner, Some(strip))?;
        let (node, _) =
            strips::plugin_at(&inserts, slot).ok_or_else(|| crate::refs::not_in_project(&owner.plugin(ctx.project.epoch, slot)))?;
        let strings = &ctx.project.strings;
        let open = node
            .children
            .iter()
            .find(|c| c.role == "AXButton" && strings.is(&c.desc, "open"))
            .ok_or_else(|| failed(format!("the slot of {} shows no open button", quoted(&node.desc))))?;
        let (path, plugin) = (open.path.clone(), node.desc.clone());
        // A window already showing this plug-in stays as it is; another strip's window is left alone.
        if !before.iter().any(|(title, subrole)| title == &label && subrole == "AXDialog") {
            nudge::read(ctx.ax.action(&ctx.project.root.clone(), &path, "AXPress", 300))?;
        }
        Ok((plugin, label))
    })?;
    let root = Root::Title(strip_name.clone());
    let opened = !before.iter().any(|(title, subrole)| title == &strip_name && subrole == "AXDialog");
    let until = std::time::Instant::now() + OPEN_WAIT;
    let window = loop {
        let now = windows(ctx)?;
        if now.iter().any(|(title, subrole)| title == &strip_name && subrole == "AXDialog") {
            let window = header(ctx, &root)?;
            if window.title == strip_name && shows(&window, &plugin) {
                break window;
            }
        }
        if std::time::Instant::now() >= until {
            let window = header(ctx, &root).ok().filter(|w| w.title == strip_name && shows(w, &plugin));
            match window {
                Some(window) => break window,
                None => {
                    // A window that came up showing something else is closed again: nothing is left open.
                    if opened {
                        if let Ok(window) = header(ctx, &root) {
                            let strings = ctx.project.strings.clone();
                            if let Some(close) = window.children.iter().find(|c| c.role == "AXButton" && strings.is(&c.desc, "close")) {
                                let _ = ctx.ax.action(&root, &close.path, "AXPress", 200);
                            }
                        }
                    }
                    return Err(failed(format!("Logic Pro didn't show the window of {} on {}", quoted(&plugin), quoted(&strip_name))));
                }
            }
        }
        std::thread::sleep(OPEN_POLL);
    };
    let strings = ctx.project.strings.clone();
    let (button, view_was) =
        view_button(&window, &strings).ok_or_else(|| failed(format!("the window of {} shows no view menu", quoted(&plugin))))?;
    let shown = Shown { root: root.clone(), plugin, strip: strip_name, view_was: view_was.clone(), opened };
    let wanted = match view {
        View::Controls if !strings.is(&view_was, CONTROLS) => Some(CONTROLS),
        View::Editor if strings.is(&view_was, CONTROLS) => Some(EDITOR),
        _ => None,
    };
    if let Some(wanted) = wanted {
        if let Err(fault) = choose_view(ctx, &root, &button, wanted) {
            hide(ctx, shown);
            return Err(fault);
        }
    }
    Ok(shown)
}

/// Makes the header show one of its buttons (`next`, `Undo`) by its label: a header too narrow hides some, so the
/// editor is made a size larger at a time until it shows (`hide` puts the size back).
pub fn reveal(ctx: &mut Ctx, shown: &Shown, label: &str) -> Done<()> {
    let strings = ctx.project.strings.clone();
    for _ in 0..SIZES.len() {
        let window = nudge::read(ctx.ax.tree(&shown.root, "", 2, 40, Some(&["role", "subrole", "title", "desc"])))?;
        if window.find(&|n| n.role == "AXButton" && strings.is(&n.desc, label)).is_some() {
            return Ok(());
        }
        let (button, now) =
            view_button(&window, &strings).ok_or_else(|| failed(format!("the window of {} shows no view menu", quoted(&shown.plugin))))?;
        let Some(larger) = larger_size(&now) else { break };
        choose_view(ctx, &shown.root, &button, larger)?;
    }
    Err(failed(format!("the window of {} shows no {label} button at any size", quoted(&shown.plugin))))
}

/// The view menu's next size up from the one the editor shows, its own (the Compressor's "71%") as well as the menu's.
fn larger_size(now: &str) -> Option<&'static str> {
    let percent = |size: &str| size.trim().strip_suffix('%').and_then(|n| n.trim().parse::<f64>().ok());
    let now = percent(now)?;
    SIZES.iter().copied().find(|size| percent(size).is_some_and(|size| size > now))
}

/// Puts the view back as it was and closes the window when Kumi opened it.
pub fn hide(ctx: &mut Ctx, shown: Shown) {
    let strings = ctx.project.strings.clone();
    if let Ok(window) = header(ctx, &shown.root) {
        if let Some((button, now)) = view_button(&window, &strings) {
            if now != shown.view_was && strings.is(&shown.view_was, CONTROLS) {
                let _ = choose_view(ctx, &shown.root, &button, CONTROLS);
            } else if now != shown.view_was {
                // The editor at the size it had: Editor (from the Controls View), then the size when it shows another.
                if strings.is(&now, CONTROLS) {
                    let _ = choose_view(ctx, &shown.root, &button, EDITOR);
                }
                if let Ok(window) = header(ctx, &shown.root) {
                    if view_button(&window, &strings).is_some_and(|(_, now)| now != shown.view_was) {
                        let _ = choose_view(ctx, &shown.root, &button, &shown.view_was);
                    }
                }
            }
        }
        if shown.opened {
            if let Some(close) = window.children.iter().find(|c| c.role == "AXButton" && strings.is(&c.desc, "close")) {
                let _ = ctx.ax.action(&shown.root, &close.path, "AXPress", 200);
            }
        }
    }
}

/// The Controls View's table as the window shows it now.
fn table(ctx: &mut Ctx, shown: &Shown) -> Done<Node> {
    let window = nudge::read(ctx.ax.tree(&shown.root, "", 6, 256, Some(&["role", "subrole", "title", "desc", "value", "valueDesc"])))?;
    window
        .find(&|n| n.role == "AXTable")
        .cloned()
        .ok_or_else(|| failed(format!("the window of {} shows no Controls View", quoted(&shown.plugin))))
}

/// The parameters of the Controls View, with their ranges.
pub fn params(ctx: &mut Ctx, shown: &Shown) -> Done<Vec<Param>> {
    params_with(ctx, shown, None)
}

/// The parameters of the Controls View, the sliders' ranges read for `only` (a place from 1) or for all of them.
fn params_with(ctx: &mut Ctx, shown: &Shown, only: Option<usize>) -> Done<Vec<Param>> {
    let table = table(ctx, shown)?;
    let mut params = Vec::new();
    for row in table.children.iter().filter(|c| c.role == "AXRow") {
        let Some(cell) = row.children.iter().find(|c| c.role == "AXCell") else { continue };
        let Some(label) = cell.children.iter().find(|c| c.role == "AXStaticText") else { continue };
        let name = label.value_text().trim().trim_end_matches(':').trim().to_string();
        if name.is_empty() {
            continue;
        }
        let index = params.len() + 1;
        let sliders: Vec<&Node> = cell.children.iter().filter(|c| c.role == "AXSlider").collect();
        let group = cell.children.iter().find(|c| c.role == "AXGroup");
        let radios: Vec<&Node> = cell.children.iter().filter(|c| c.role == "AXRadioButton").collect();
        let param = if let Some(slider) = sliders.first() {
            let text = group
                .map(Node::value_text)
                .filter(|t| !t.is_empty())
                .or_else(|| group.and_then(|g| g.children.iter().find(|c| c.role == "AXSlider")).map(|s| s.value_desc.clone()))
                .unwrap_or_else(|| slider.value_desc.clone());
            let attrs = if only.is_none_or(|only| only == index) {
                nudge::read(ctx.ax.get(&shown.root, &slider.path, &["AXMinValue", "AXMaxValue"]))?
            } else {
                serde_json::Map::new()
            };
            Param {
                index,
                name,
                kind: Kind::Slider,
                path: slider.path.clone(),
                value: slider.number(),
                text,
                min: attrs.get("AXMinValue").and_then(Value::as_f64),
                max: attrs.get("AXMaxValue").and_then(Value::as_f64),
                choices: vec![],
                text_path: group.map(|g| g.path.clone()),
            }
        } else if !radios.is_empty() {
            let on = radios.iter().position(|r| r.is_on());
            Param {
                index,
                name,
                kind: Kind::Choice,
                path: radios[0].path.clone(),
                value: on.map(|i| i as f64),
                text: on.map(|i| radios[i].title.clone()).unwrap_or_default(),
                min: Some(0.0),
                max: Some((radios.len() - 1) as f64),
                choices: radios.iter().map(|r| (r.title.clone(), r.path.clone())).collect(),
                text_path: None,
            }
        } else if let Some(check) = cell.children.iter().find(|c| c.role == "AXCheckBox") {
            Param {
                index,
                name,
                kind: Kind::Switch,
                path: check.path.clone(),
                value: Some(if check.is_on() { 1.0 } else { 0.0 }),
                text: if check.is_on() { "on".into() } else { "off".into() },
                min: Some(0.0),
                max: Some(1.0),
                choices: vec![],
                text_path: None,
            }
        } else if let Some(popup) = cell.children.iter().find(|c| c.role == "AXPopUpButton") {
            Param {
                index,
                name,
                kind: Kind::Menu,
                path: popup.path.clone(),
                value: None,
                text: popup.value_text(),
                min: None,
                max: None,
                choices: vec![],
                text_path: None,
            }
        } else if let Some((group, inner)) = group.and_then(|g| g.children.iter().find(|c| c.role == "AXSlider").map(|s| (g, s))) {
            // The text's group alone, its slider disabled with no range: a value shown, never set from here.
            let text = Some(group.value_text()).filter(|t| !t.is_empty()).unwrap_or_else(|| inner.value_desc.clone());
            Param {
                index,
                name,
                kind: Kind::Readout,
                path: inner.path.clone(),
                value: None,
                text,
                min: None,
                max: None,
                choices: vec![],
                text_path: None,
            }
        } else {
            continue;
        };
        params.push(param);
    }
    if params.is_empty() {
        return Err(failed(format!("the Controls View of {} lists no parameters", quoted(&shown.plugin))));
    }
    Ok(params)
}

/// The parameter at `index` (from 1) as it reads now.
pub fn param(ctx: &mut Ctx, shown: &Shown, index: usize) -> Done<Param> {
    params_with(ctx, shown, Some(index))?
        .into_iter()
        .nth(index.wrapping_sub(1))
        .ok_or_else(|| failed(format!("{} has no parameter {index} in its Controls View", quoted(&shown.plugin))))
}

/// Sets a parameter: a slider to a raw value (exact, as the Controls View's sliders take it), a choice by its option's
/// place, a switch on or off. The parameter as it reads after.
pub fn write(ctx: &mut Ctx, shown: &Shown, param: &Param, raw: f64) -> Done<Param> {
    match param.kind {
        Kind::Slider => {
            nudge::wrote(ctx.ax.set(&shown.root, &param.path, "AXValue", json!(raw), 150))?;
        }
        Kind::Choice => {
            let (_, path) = param.choices.get(raw.round().max(0.0) as usize).ok_or_else(|| failed("no such option"))?;
            let path = path.clone();
            nudge::wrote(ctx.ax.action(&shown.root, &path, "AXPress", 150))?;
        }
        Kind::Switch => {
            if (param.value.unwrap_or(0.0) >= 0.5) != (raw >= 0.5) {
                nudge::wrote(ctx.ax.action(&shown.root, &param.path, "AXPress", 150))?;
            }
        }
        Kind::Menu => return Err(failed(format!("{}'s {} is a menu Kumi can't set from outside", quoted(&shown.plugin), param.name))),
        Kind::Readout => return Err(failed(readout_words(&shown.plugin, &param.name))),
    }
    super::pause(Duration::from_millis(100));
    match (param.kind, &param.text_path) {
        // A slider is read back where it is: its value and the text beside it, without the whole view.
        (Kind::Slider, Some(text_path)) => {
            let value = nudge::wrote(ctx.ax.get(&shown.root, &param.path, &["AXValue"]))?;
            let text = nudge::wrote(ctx.ax.get(&shown.root, text_path, &["AXValue"]))?;
            let mut now = param.clone();
            now.value = value.get("AXValue").and_then(Value::as_f64);
            now.text = text.get("AXValue").and_then(Value::as_str).unwrap_or_default().to_string();
            Ok(now)
        }
        _ => param_named(ctx, shown, param).map_err(|(_, why)| super::uncertain(why)),
    }
}

/// The parameter read again, by its place and checked by its name.
fn param_named(ctx: &mut Ctx, shown: &Shown, wanted: &Param) -> Done<Param> {
    let now = param(ctx, shown, wanted.index)?;
    if now.name != wanted.name {
        return Err(failed(format!("{}'s Controls View changed while Kumi read it", quoted(&shown.plugin))));
    }
    Ok(now)
}

/// Why a value the Controls View only shows isn't set.
pub fn readout_words(plugin: &str, name: &str) -> String {
    format!("{}'s Controls View shows its {name} with no control to set it: Kumi can read it, not change it", quoted(plugin))
}

/// The preset the window's setting pop-up names: the header's last pop-up (a plug-in with a side chain, the
/// Compressor, lists its Side Chain pop-up first).
pub fn preset(ctx: &mut Ctx, shown: &Shown) -> Done<(String, String)> {
    let window = header(ctx, &shown.root)?;
    let popup = window
        .children
        .iter()
        .rfind(|c| c.role == "AXPopUpButton")
        .ok_or_else(|| failed(format!("the window of {} shows no setting menu", quoted(&shown.plugin))))?;
    Ok((popup.path.clone(), popup.value_text()))
}

/// Presses one of the window's header buttons by its label (`next`, `previous`, `Undo`, `Redo`), found in its groups.
pub fn press(ctx: &mut Ctx, shown: &Shown, label: &str) -> Done<()> {
    let window = nudge::read(ctx.ax.tree(&shown.root, "", 2, 40, Some(&["role", "subrole", "desc"])))?;
    let strings = &ctx.project.strings;
    let button = window
        .find(&|n| n.role == "AXButton" && strings.is(&n.desc, label))
        .ok_or_else(|| failed(format!("the window of {} shows no {label} button", quoted(&shown.plugin))))?;
    let path = button.path.clone();
    nudge::wrote(ctx.ax.action(&shown.root, &path, "AXPress", 400))?;
    Ok(())
}

/// A slider's text as a number (the first number in it: "-6.0 dB" is -6).
pub fn reading(text: &str) -> Option<f64> {
    crate::rows::first_number(&text.replace('\u{2212}', "-"))
}

/// A slider's raw value found for a number asked: the raw value, what its text reads, and, when no raw value reads the
/// number exactly, the slider's resolution there (how far its text moves for one raw step).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    pub raw: f64,
    pub reading: f64,
    pub step: Option<f64>,
}

/// The raw value of a slider whose text reads `wanted`, found by setting it (each set is exact) and reading the text:
/// a bisection over the range, the text rising or falling with the value, kept on until a value reads `wanted`
/// exactly or the range is down to two neighbours (a reading merely close, -3.1 for -3, doesn't end it), then the
/// range's two ends read; when none of those reads `wanted` exactly, the best one's neighbours too (a slider whose
/// text rounds may show the number a step over), and the slider's resolution there is said. None when no value reads
/// within `tolerance`.
pub fn find_raw(ctx: &mut Ctx, shown: &Shown, param: &Param, wanted: f64, tolerance: f64) -> Done<Option<Found>> {
    let (Some(min), Some(max)) = (param.min, param.max) else { return Ok(None) };
    let read_at = |ctx: &mut Ctx, raw: f64| -> Done<Option<f64>> {
        let after = write(ctx, shown, param, raw)?;
        Ok(reading(&after.text))
    };
    let exact = |at: f64| (at - wanted).abs() < 1e-6;
    let (Some(low), Some(high)) = (read_at(ctx, min)?, read_at(ctx, max)?) else { return Ok(None) };
    if wanted < low.min(high) - tolerance || wanted > low.max(high) + tolerance {
        return Ok(None);
    }
    let rising = high >= low;
    let (mut lo, mut hi) = (min, max);
    let mut best: Option<(f64, f64)> = None;
    let better = |best: &Option<(f64, f64)>, at: f64| best.is_none_or(|(_, b)| (at - wanted).abs() < (b - wanted).abs());
    for _ in 0..32 {
        if hi - lo <= 1.0 {
            break;
        }
        let mid = ((lo + hi) / 2.0).round();
        let Some(at) = read_at(ctx, mid)? else { break };
        if better(&best, at) {
            best = Some((mid, at));
        }
        if exact(at) {
            break;
        }
        if (at < wanted) == rising {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    for raw in [lo, hi] {
        if let Some(at) = read_at(ctx, raw)? {
            if better(&best, at) {
                best = Some((raw, at));
            }
        }
    }
    let mut step = None;
    if let Some((raw, _)) = best.filter(|(_, at)| !exact(*at)) {
        let mut around = vec![];
        for near in [raw - 1.0, raw + 1.0] {
            if (min..=max).contains(&near) {
                if let Some(at) = read_at(ctx, near)? {
                    around.push(at);
                    if better(&best, at) {
                        best = Some((near, at));
                    }
                }
            }
        }
        if let (Some((_, at)), [below, above]) = (best, around.as_slice()) {
            if !exact(at) {
                step = Some(((above - below) / 2.0).abs());
            }
        }
    }
    Ok(best.filter(|(_, at)| (at - wanted).abs() <= tolerance).map(|(raw, reading)| Found { raw, reading, step }))
}

/// A parameter as a test makes one.
#[cfg(test)]
pub fn test_param(
    index: usize,
    name: &str,
    kind: Kind,
    value: Option<f64>,
    text: &str,
    range: Option<(f64, f64)>,
    choices: Vec<(String, String)>,
) -> Param {
    Param {
        index,
        name: name.into(),
        kind,
        path: format!("{index}"),
        value,
        text: text.into(),
        min: range.map(|r| r.0),
        max: range.map(|r| r.1),
        choices,
        text_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_controls_view_row_is_a_parameter_by_its_name_value_and_range() {
        let gain = Param {
            index: 1,
            name: "Gain".into(),
            kind: Kind::Slider,
            path: "9.0.0.0.2".into(),
            value: Some(960.0),
            text: "0.0 dB".into(),
            min: Some(0.0),
            max: Some(1200.0),
            choices: vec![],
            text_path: Some("9.0.0.0.1".into()),
        };
        assert_eq!(gain.normalized(), Some(0.8));
        assert_eq!(
            gain.row("1:param:0.1.1.1", "1:plugin:0.1.1"),
            json!({"ref": "1:param:0.1.1.1", "pluginRef": "1:plugin:0.1.1", "index": 1, "name": "Gain", "value": 960.0, "text": "0.0 dB", "min": 0.0, "max": 1200.0, "normalized": 0.8})
        );
        assert_eq!(reading("−16.0 dB"), Some(-16.0));
        assert_eq!(reading("250 Hz"), Some(250.0));
        assert_eq!(reading("Norm"), None);
    }

    #[test]
    fn an_editor_is_made_a_size_larger_from_the_menus_sizes_or_its_own() {
        assert_eq!(larger_size("75%"), Some("100%"));
        // The Compressor's editor keeps 71%, which the view menu lists after its own sizes.
        assert_eq!(larger_size("71%"), Some("75%"));
        assert_eq!(larger_size("200%"), None);
        assert_eq!(larger_size("Controls"), None);
        assert!(readout_words("Compressor", "Knee").starts_with("“Compressor”'s Controls View shows its Knee with no control"));
    }
}
