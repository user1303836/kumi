//! How a device looks in Live: Kumi lays every face out the way Live's own devices are, from what the controls are and
//! the groups the model gives them. Nothing on a face is coloured by hand. Live's objects follow the producer's theme,
//! and what Kumi draws itself takes the theme's own colours (`themecolor.…`), so a face reads in a dark theme and a light
//! one.
//!
//! - **Sections.** Each group is a section with its name over it, and its controls stand in columns as tall as the face.
//!   A thin line between sections divides them. Kumi's own Mix and Output come last.
//! - **Looks.** A number is a dial with its name and value, a small box of its value under its name, or a fader. A
//!   choice is a menu, or its options as tabs. A switch is a button with its name, lit while on, and a trigger is a
//!   button that's on only while it's pressed.
//! - **Steps.** Controls named for a step ("Gate 1" … "Gate 16", or "Step 1 Note") that the patch reads by step make a
//!   step grid: a row a lane, a column a step. The playing step lights when the patch sends its index to `playhead`.
//! - **Pictures.** What a [v8ui] draws gets a section of its own.
//! - **Pages.** A device that modulates has a page of mapping slots. Tabs at the face's left edge show its main page or
//!   its Map page, and switching them adds nothing to Live's undo history or automation.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::spec::{Control, Show};

/// Live shows a device 169 pixels tall.
pub const HEIGHT: f64 = 169.0;
/// The room at the face's top and left.
const MARGIN: f64 = 6.0;
/// A section's heading, over its controls.
const HEADING: f64 = 15.0;
/// Between two sections, with the line that divides them in the middle.
const GAP: f64 = 13.0;
/// The tallest a column of controls is.
const COLUMN: f64 = HEIGHT - MARGIN * 2.0 - HEADING;
/// A step grid's cell and its lanes' name.
const STEP: f64 = 22.0;
const LANE: f64 = 18.0;
const LANE_NAME: f64 = 46.0;
/// The fewest steps that make a lane.
const FEWEST_STEPS: usize = 4;
/// The page tabs at the face's left edge.
const TABS: f64 = 38.0;

/// A theme colour as Max saves it: the colour now (Live's dark theme's), and the expression that keeps it the theme's.
pub fn theme(attribute: &str, colour: &str) -> (Value, Value) {
    let rgba = match colour {
        "live_lcd_bg" => [0.094, 0.094, 0.094, 1.0],
        "live_lcd_control_fg" | "live_display_line_one" | "live_control_selection" => [1.0, 0.678, 0.337, 1.0],
        "live_lcd_control_fg_zombie" | "live_display_scale_text" => [0.525, 0.525, 0.525, 1.0],
        "live_lcd_frame" => [0.243, 0.243, 0.243, 1.0],
        "live_surface_frame" | "live_control_text_bg" => [0.141, 0.141, 0.141, 1.0],
        _ => [0.71, 0.71, 0.71, 1.0],
    };
    (json!({ attribute: rgba }), json!({ attribute: { "expression": format!("themecolor.{colour}") } }))
}

/// What a control shows as, from its type and its look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell {
    Dial,
    Number,
    Slider,
    Menu,
    Tabs(usize),
    Button,
    Trigger,
}

impl Cell {
    pub fn of(control: &Control) -> Cell {
        let show = control.place().show;
        match control {
            Control::Number(_) | Control::Integer(_) => match show {
                Show::Number => Cell::Number,
                Show::Slider => Cell::Slider,
                _ => Cell::Dial,
            },
            Control::Choice(choice) => match show {
                Show::Tabs => Cell::Tabs(choice.options.len()),
                _ => Cell::Menu,
            },
            Control::Switch(_) if show == Show::Trigger => Cell::Trigger,
            Control::Switch(_) => Cell::Button,
        }
    }

    /// The width and height it takes, its name included where the object doesn't show it.
    fn size(self, name: &str) -> (f64, f64) {
        let text = text_width(name);
        match self {
            Cell::Dial => (52.0f64.max(text.min(68.0)), 56.0),
            Cell::Number => (52.0f64.max(text.min(68.0)), 32.0),
            Cell::Slider => (36.0f64.max(text.min(56.0)), 106.0),
            Cell::Menu => (76.0, 32.0),
            Cell::Tabs(options) => ((options as f64 * 22.0).clamp(52.0, 132.0), 32.0),
            Cell::Button | Cell::Trigger => ((text + 16.0).clamp(52.0, 96.0), 22.0),
        }
    }

    /// Whether a label with the control's name goes over it (a dial and a button show their own).
    fn labelled(self) -> bool {
        matches!(self, Cell::Number | Cell::Slider | Cell::Menu | Cell::Tabs(_))
    }
}

/// About how wide Live draws a short name at the face's size.
fn text_width(text: &str) -> f64 {
    text.chars().count() as f64 * 5.6 + 4.0
}

/// One control's place on the face: its object's rect, and a label's over it when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub rect: [f64; 4],
    pub label: Option<[f64; 4]>,
    /// In a step grid: its cell's own small look.
    pub step: bool,
}

/// Something else on the face: a heading, a line between sections, a step grid's numbers and playhead.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    Heading {
        text: String,
        rect: [f64; 4],
    },
    Line {
        rect: [f64; 4],
    },
    /// A step grid's playhead: a tab of each step's number, lit at the playing step.
    Playhead {
        steps: usize,
        rect: [f64; 4],
    },
    /// A lane's name, at the left of its row.
    Lane {
        text: String,
        rect: [f64; 4],
    },
}

/// What a face holds besides the controls: each picture's size, and the mapping page's (a page of its own).
#[derive(Debug, Clone, Default)]
pub struct Extras {
    pub pictures: Vec<(f64, f64)>,
    pub map_page: Option<(f64, f64)>,
    /// The step lanes shown as a step grid, by name: those the patch reads by step, or every lane when it sends the
    /// playing step to playhead. Numbered controls are a grid only then ("Band 1" … "Band 4" stay controls).
    pub lanes: Vec<String>,
}

/// The face laid out: each control's spot (by its index), the marks, each picture's rect, the mapping page's, and the
/// device's width. With a mapping page, `main_x` is where the main page starts, right of the page tabs.
#[derive(Debug, Clone)]
pub struct Face {
    pub spots: Vec<Spot>,
    pub marks: Vec<Mark>,
    pub pictures: Vec<[f64; 4]>,
    pub map_page: Option<[f64; 4]>,
    pub tabs: Option<[f64; 4]>,
    pub width: f64,
    /// Whether a step grid's playhead is on the face.
    pub playhead: bool,
}

/// A name's step and lane: "Gate 16" is step 16 of lane "Gate", "Step 01 Note" step 1 of "Note".
fn step_of(name: &str) -> Option<(usize, String)> {
    let words: Vec<&str> = name.split_whitespace().collect();
    let numbers: Vec<usize> =
        words.iter().enumerate().filter(|(_, word)| word.chars().all(|c| c.is_ascii_digit())).map(|(at, _)| at).collect();
    let [at] = numbers[..] else { return None };
    let step: usize = words[at].parse().ok().filter(|step| (1..=128).contains(step))?;
    let lane: Vec<&str> =
        words.iter().enumerate().filter(|(i, word)| *i != at && !word.eq_ignore_ascii_case("step")).map(|(_, word)| *word).collect();
    Some((step, if lane.is_empty() { "Step".to_string() } else { lane.join(" ") }))
}

/// The step lanes among the controls: each lane's name and its controls' indexes by step, for lanes of the same type
/// whose steps run 1 to N without a gap (N at least 4), all lanes of one grid with the same N.
pub fn lanes(controls: &[Control]) -> Vec<(String, Vec<usize>)> {
    let mut found: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (index, control) in controls.iter().enumerate() {
        if let Some((step, lane)) = step_of(control.name()) {
            found.entry(lane).or_default().push((step, index));
        }
    }
    let mut lanes: Vec<(String, Vec<usize>)> = found
        .into_iter()
        .filter_map(|(lane, mut steps)| {
            steps.sort();
            let whole = steps.len() >= FEWEST_STEPS && steps.iter().enumerate().all(|(at, (step, _))| *step == at + 1);
            let first = controls[steps[0].1].type_name();
            let alike = steps.iter().all(|(_, index)| controls[*index].type_name() == first);
            (whole && alike).then(|| (lane, steps.into_iter().map(|(_, index)| index).collect()))
        })
        .collect();
    // One grid: the step count most lanes have. Lanes with another count are ordinary controls.
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, steps) in &lanes {
        *counts.entry(steps.len()).or_default() += 1;
    }
    if let Some((&steps, _)) = counts.iter().max_by_key(|(steps, lanes)| (**lanes, **steps)) {
        lanes.retain(|(_, lane)| lane.len() == steps);
    }
    // In the order their first steps were given.
    lanes.sort_by_key(|(_, steps)| steps[0]);
    lanes
}

/// Lays a face out: `controls` in their groups (Kumi's own Mix and Output among them, in their group), then `extras`.
pub fn lay_out(controls: &[Control], extras: &Extras) -> Face {
    let lanes: Vec<(String, Vec<usize>)> = lanes(controls).into_iter().filter(|(lane, _)| extras.lanes.contains(lane)).collect();
    let in_grid: Vec<bool> = (0..controls.len()).map(|index| lanes.iter().any(|(_, steps)| steps.contains(&index))).collect();
    // The sections, in the order their first control was given; the step grid where its first step was.
    let mut sections: Vec<(String, Vec<usize>)> = Vec::new();
    let mut grid_at: Option<usize> = None;
    for (index, control) in controls.iter().enumerate() {
        if in_grid[index] {
            if grid_at.is_none() {
                grid_at = Some(sections.len());
                sections.push((String::new(), Vec::new()));
            }
            continue;
        }
        let group = control.place().group.clone();
        match sections.iter_mut().enumerate().find(|(at, (name, _))| *name == group && Some(*at) != grid_at) {
            Some((_, (_, members))) => members.push(index),
            None => sections.push((group, vec![index])),
        }
    }
    // Kumi's own Output group goes last.
    if let Some(at) = sections.iter().position(|(name, members)| name == "Output" && members.iter().any(|i| is_kumis(&controls[*i]))) {
        let output = sections.remove(at);
        if grid_at.is_some_and(|grid| grid > at) {
            grid_at = grid_at.map(|grid| grid - 1);
        }
        sections.push(output);
    }
    let mut spots: Vec<Spot> = vec![Spot { rect: [0.0; 4], label: None, step: false }; controls.len()];
    let mut marks: Vec<Mark> = Vec::new();
    let left = if extras.map_page.is_some() { MARGIN + TABS } else { MARGIN };
    let mut x = left;
    let mut playhead = false;
    let mut first = true;
    let divide = |x: &mut f64, marks: &mut Vec<Mark>, first: &mut bool| {
        if !*first {
            marks.push(Mark::Line { rect: [*x + GAP / 2.0 - 0.5, MARGIN + 2.0, 1.0, HEIGHT - MARGIN * 2.0 - 4.0] });
            *x += GAP;
        }
        *first = false;
    };
    for (at, (name, members)) in sections.iter().enumerate() {
        if grid_at == Some(at) {
            divide(&mut x, &mut marks, &mut first);
            let steps = lanes[0].1.len();
            let shared = lanes.iter().flat_map(|(_, steps)| steps).map(|index| controls[*index].place().group.as_str()).collect::<Vec<_>>();
            let heading = if !shared.is_empty() && shared.iter().all(|group| *group == shared[0]) && !shared[0].is_empty() {
                shared[0].to_string()
            } else {
                "Steps".to_string()
            };
            marks.push(Mark::Heading { text: heading.clone(), rect: [x, MARGIN, text_width(&heading).max(40.0), HEADING - 2.0] });
            let top = MARGIN + HEADING;
            // The playhead row of step numbers, then a row a lane; past the face's height, lanes go on to the right.
            let rows_fit = ((COLUMN - LANE) / LANE).floor().max(1.0) as usize;
            let blocks = lanes.len().div_ceil(rows_fit);
            let block_width = LANE_NAME + steps as f64 * STEP;
            for block in 0..blocks {
                let bx = x + block as f64 * (block_width + GAP);
                marks.push(Mark::Playhead { steps, rect: [bx + LANE_NAME, top, steps as f64 * STEP - 2.0, LANE - 3.0] });
                for (row, (lane, members)) in lanes.iter().enumerate().skip(block * rows_fit).take(rows_fit) {
                    let y = top + LANE * (1 + row - block * rows_fit) as f64;
                    marks.push(Mark::Lane { text: lane.clone(), rect: [bx, y, LANE_NAME - 4.0, LANE - 3.0] });
                    for (step, index) in members.iter().enumerate() {
                        spots[*index] =
                            Spot { rect: [bx + LANE_NAME + step as f64 * STEP, y, STEP - 2.0, LANE - 3.0], label: None, step: true };
                    }
                }
            }
            playhead = true;
            x += blocks as f64 * (block_width + GAP) - GAP;
            continue;
        }
        divide(&mut x, &mut marks, &mut first);
        let top = if name.is_empty() { MARGIN } else { MARGIN + HEADING };
        let tall = HEIGHT - MARGIN - top;
        if !name.is_empty() {
            marks.push(Mark::Heading { text: name.clone(), rect: [x, MARGIN, text_width(name).max(40.0), HEADING - 2.0] });
        }
        // Columns as tall as the section has room for, top to bottom, each as wide as its widest control, so the
        // controls in a column line up.
        let mut columns: Vec<Vec<(usize, Cell, f64, f64)>> = vec![Vec::new()];
        let mut y = top;
        for index in members {
            let control = &controls[*index];
            let cell = Cell::of(control);
            let (w, h) = cell.size(control.name());
            if y + h > top + tall + 0.5 && y > top {
                columns.push(Vec::new());
                y = top;
            }
            columns.last_mut().unwrap().push((*index, cell, w, h));
            y += h + 2.0;
        }
        let mut column_x = x;
        for column in &columns {
            let widest = column.iter().fold(0.0f64, |widest, (_, _, w, _)| widest.max(*w));
            let mut y = top;
            for (index, cell, _, h) in column {
                spots[*index] = place(*cell, column_x, y, widest, *h);
                y += h + 2.0;
            }
            column_x += widest + 6.0;
        }
        x = column_x - 6.0;
    }
    let mut pictures = Vec::new();
    for (w, h) in &extras.pictures {
        divide(&mut x, &mut marks, &mut first);
        let (w, h) = (w.clamp(40.0, 800.0), h.clamp(20.0, HEIGHT - MARGIN * 2.0));
        pictures.push([x, MARGIN + ((HEIGHT - MARGIN * 2.0 - h) / 2.0).floor(), w, h]);
        x += w;
    }
    let main_right = x;
    let (map_page, tabs) = match extras.map_page {
        Some((w, h)) => {
            // The Map page stands where the main page does, shown in its place.
            let page = [left, MARGIN, w, h.min(HEIGHT - MARGIN * 2.0)];
            (Some(page), Some([MARGIN, MARGIN, TABS - 8.0, 36.0]))
        }
        None => (None, None),
    };
    let right = map_page.map_or(main_right, |page| main_right.max(page[0] + page[2]));
    Face { spots, marks, pictures, map_page, tabs, width: (right + MARGIN).max(120.0).ceil(), playhead }
}

/// Whether a control is one Kumi adds itself (Mix and Output).
fn is_kumis(control: &Control) -> bool {
    matches!(control.name(), "Mix" | "Output") && control.place().group == "Output"
}

/// A control's object and its label in its cell, centred.
fn place(cell: Cell, x: f64, y: f64, w: f64, h: f64) -> Spot {
    let label = cell.labelled().then_some([x, y, w, 13.0]);
    let below = if cell.labelled() { 14.0 } else { 0.0 };
    let rect = match cell {
        Cell::Dial => [x + ((w - 44.0) / 2.0).floor(), y, 44.0, 48.0],
        Cell::Number => [x + ((w - 44.0) / 2.0).floor(), y + below, 44.0, 15.0],
        Cell::Slider => [x + ((w - 20.0) / 2.0).floor(), y + below, 20.0, h - below - 2.0],
        Cell::Menu => [x, y + below, w - 4.0, 15.0],
        Cell::Tabs(_) => [x, y + below, w - 4.0, 15.0],
        Cell::Button | Cell::Trigger => [x, y + 2.0, w - 4.0, 16.0],
    };
    Spot { rect, label, step: false }
}

/// A control's box before it's placed: its object, by how it shows, and its `valueof` (how Live sees the parameter).
pub fn control_box(control: &Control) -> (Value, Value) {
    let name = control.name();
    let short: String = name.chars().take(12).collect();
    let mut valueof = json!({ "parameter_longname": name, "parameter_shortname": short, "parameter_initial_enable": 1 });
    let set = |target: &mut Value, more: Value| {
        if let (Value::Object(target), Value::Object(more)) = (target, more) {
            target.extend(more);
        }
    };
    let item = match (control, Cell::of(control)) {
        (Control::Choice(choice), cell) => {
            let initial = choice.options.iter().position(|option| *option == choice.default).unwrap_or(0);
            set(
                &mut valueof,
                json!({ "parameter_type": 2, "parameter_enum": choice.options, "parameter_mmax": choice.options.len() as i64 - 1, "parameter_initial": [initial] }),
            );
            if let Cell::Tabs(_) = cell {
                json!({ "maxclass": "live.tab", "numinlets": 1, "numoutlets": 3, "outlettype": ["", "", "float"], "num_lines_presentation": 1, "num_lines_patching": 1 })
            } else {
                json!({ "maxclass": "live.menu", "numinlets": 1, "numoutlets": 3, "outlettype": ["", "", "float"] })
            }
        }
        (Control::Switch(switch), cell) => {
            set(
                &mut valueof,
                json!({ "parameter_type": 2, "parameter_enum": ["off", "on"], "parameter_mmax": 1, "parameter_initial": [if switch.default { 1 } else { 0 }] }),
            );
            // A button with its name: lit while on, or (a trigger) only while it's pressed.
            let mode = if cell == Cell::Trigger { 0 } else { 1 };
            json!({ "maxclass": "live.text", "numinlets": 1, "numoutlets": 2, "outlettype": ["", ""], "mode": mode, "text": short, "texton": short })
        }
        (Control::Number(numeric) | Control::Integer(numeric), cell) => {
            let integer = matches!(control, Control::Integer(_));
            let unit = super::midi::unit_style(numeric, integer);
            set(
                &mut valueof,
                json!({ "parameter_type": if integer { 1 } else { 0 }, "parameter_mmin": numeric.min, "parameter_mmax": numeric.max, "parameter_initial": [numeric.default], "parameter_unitstyle": unit.style }),
            );
            if let Some(units) = unit.units {
                set(&mut valueof, json!({ "parameter_units": units }));
            }
            // Frequencies and times spread over decades turn more evenly on a curve.
            let wide = matches!(numeric.unit, super::spec::Unit::Hz | super::spec::Unit::Ms)
                && numeric.min > 0.0
                && numeric.max / numeric.min >= 20.0;
            if wide {
                set(&mut valueof, json!({ "parameter_exponent": 3.0 }));
            }
            let class = match cell {
                Cell::Number => "live.numbox",
                Cell::Slider => "live.slider",
                _ => "live.dial",
            };
            json!({ "maxclass": class, "numinlets": 1, "numoutlets": 2, "outlettype": ["", "float"] })
        }
    };
    (item, valueof)
}

/// A control's box in its spot: its rect, and in a step grid the step's own small look (no name on a step's button).
pub fn put(item: &mut Value, spot: &Spot) {
    item["presentation"] = json!(1);
    item["presentation_rect"] = json!(spot.rect);
    if spot.step && item["maxclass"] == "live.text" {
        item["text"] = json!("");
        item["texton"] = json!("");
    }
    if spot.step && item["maxclass"] == "live.dial" {
        // A step's number is a small box of its value: a dial doesn't fit a step's cell.
        item["maxclass"] = json!("live.numbox");
    }
}

/// A box of Kumi's own on the face, nothing wired to it: a label, a heading, a line, a step's number.
fn decoration(id: &str, fields: Value) -> Value {
    let mut item =
        json!({ "id": id, "varname": id, "presentation": 1, "numinlets": 1, "numoutlets": 0, "patching_rect": [40.0, 600.0, 60.0, 18.0] });
    if let (Value::Object(item), Value::Object(fields)) = (&mut item, fields) {
        item.extend(fields);
    }
    item
}

/// The face's own boxes for a layout: each control's label, the headings, lines and lane names, and the playhead's step
/// numbers (`playhead` is the id of the box the patch's `playhead` reaches, which lights them). Each box's id is its
/// scripting name, so the page tabs can show and hide it.
pub fn marks(controls: &[Control], face: &Face, playhead: &str) -> (Vec<Value>, Vec<(String, u32, String, u32)>) {
    let mut boxes = Vec::new();
    let mut cords: Vec<(String, u32, String, u32)> = Vec::new();
    let mut count = 0;
    let next = |count: &mut usize| {
        *count += 1;
        format!("kumi-face-{count}")
    };
    for (control, spot) in controls.iter().zip(&face.spots) {
        if let Some(rect) = spot.label {
            let centred = matches!(Cell::of(control), Cell::Number | Cell::Slider);
            boxes.push(decoration(
                &next(&mut count),
                json!({ "maxclass": "live.comment", "text": control.name(), "presentation_rect": rect, "textjustification": if centred { 1 } else { 0 } }),
            ));
        }
    }
    for mark in &face.marks {
        match mark {
            Mark::Heading { text, rect } => boxes.push(decoration(
                &next(&mut count),
                json!({ "maxclass": "live.comment", "text": text, "presentation_rect": rect, "fontface": 1 }),
            )),
            Mark::Lane { text, rect } => boxes.push(decoration(
                &next(&mut count),
                json!({ "maxclass": "live.comment", "text": text, "presentation_rect": rect, "textjustification": 0 }),
            )),
            Mark::Line { rect } => boxes.push(decoration(&next(&mut count), json!({ "maxclass": "live.line", "presentation_rect": rect }))),
            Mark::Playhead { steps, rect } => {
                // Each step's number, lit while it plays: the step that was lit goes off, then the new one comes on.
                let held = next(&mut count);
                let off = next(&mut count);
                let on = next(&mut count);
                let indexes: Vec<String> = (0..*steps).map(|step| step.to_string()).collect();
                boxes.push(json!({ "id": playhead, "maxclass": "newobj", "text": "t i i b", "numinlets": 1, "numoutlets": 3, "outlettype": ["int", "int", "bang"], "patching_rect": [40.0, 600.0, 50.0, 22.0] }));
                boxes.push(json!({ "id": held, "maxclass": "newobj", "text": "i -1", "numinlets": 2, "numoutlets": 1, "outlettype": ["int"], "patching_rect": [40.0, 640.0, 40.0, 22.0] }));
                boxes.push(json!({ "id": off, "maxclass": "newobj", "text": format!("sel {}", indexes.join(" ")), "numinlets": 2, "numoutlets": steps + 1, "outlettype": vec![""; steps + 1], "patching_rect": [40.0, 680.0, 200.0, 22.0] }));
                boxes.push(json!({ "id": on, "maxclass": "newobj", "text": format!("sel {}", indexes.join(" ")), "numinlets": 2, "numoutlets": steps + 1, "outlettype": vec![""; steps + 1], "patching_rect": [40.0, 720.0, 200.0, 22.0] }));
                cords.push((playhead.to_string(), 2, held.clone(), 0));
                cords.push((held.clone(), 0, off.clone(), 0));
                cords.push((playhead.to_string(), 1, held.clone(), 1));
                cords.push((playhead.to_string(), 0, on.clone(), 0));
                let cell = (rect[2] + 2.0) / *steps as f64;
                for step in 0..*steps {
                    let id = next(&mut count);
                    let dark = next(&mut count);
                    let lit = next(&mut count);
                    let number = (step + 1).to_string();
                    boxes.push(decoration(
                        &id,
                        json!({ "maxclass": "live.text", "parameter_enable": 0, "ignoreclick": 1, "mode": 1, "text": number, "texton": number,
                            "numinlets": 1, "numoutlets": 2, "outlettype": ["", ""], "presentation_rect": [rect[0] + step as f64 * cell, rect[1], cell - 2.0, rect[3]] }),
                    ));
                    boxes.push(json!({ "id": dark, "maxclass": "message", "text": "set 0", "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [40.0, 760.0, 40.0, 22.0] }));
                    boxes.push(json!({ "id": lit, "maxclass": "message", "text": "set 1", "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [40.0, 800.0, 40.0, 22.0] }));
                    cords.push((off.clone(), step as u32, dark.clone(), 0));
                    cords.push((dark, 0, id.clone(), 0));
                    cords.push((on.clone(), step as u32, lit.clone(), 0));
                    cords.push((lit, 0, id, 0));
                }
            }
        }
    }
    (boxes, cords)
}

/// The page tabs and what they switch, for a face with a Map page: the boxes and their cords. `main` names every box on
/// the main page (by its scripting name) and `page` the Map page's box. The tabs aren't parameters, so switching pages
/// leaves Live's undo history and automation alone; the device opens on its main page.
pub fn pages(tabs: [f64; 4], main: &[String], page: &str) -> (Vec<Value>, Vec<(String, u32, String, u32)>) {
    let quoted = |name: &str| if name.contains(' ') { format!("\"{name}\"") } else { name.to_string() };
    let show_main: Vec<String> =
        main.iter().map(|name| format!("script show {}", quoted(name))).chain([format!("script hide {page}")]).collect();
    let show_map: Vec<String> =
        main.iter().map(|name| format!("script hide {}", quoted(name))).chain([format!("script show {page}")]).collect();
    let half = (tabs[3] - 2.0) / 2.0;
    let tab = |id: &str, text: &str, y: f64| {
        json!({ "id": id, "varname": id, "maxclass": "live.text", "parameter_enable": 0, "mode": 1, "text": text, "texton": text, "presentation": 1,
            "presentation_rect": [tabs[0], y, tabs[2], half], "numinlets": 1, "numoutlets": 2, "outlettype": ["", ""], "patching_rect": [40.0, 900.0, 44.0, 15.0] })
    };
    let message = |id: &str, text: String| json!({ "id": id, "maxclass": "message", "text": text, "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [40.0, 940.0, 60.0, 22.0] });
    let object = |id: &str, text: &str, ins: usize, outs: usize| json!({ "id": id, "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs], "patching_rect": [40.0, 980.0, 60.0, 22.0] });
    let boxes = vec![
        tab("kumi-page-main", "Main", tabs[1]),
        tab("kumi-page-map", "Map", tabs[1] + half + 2.0),
        object("kumi-page-to-main", "t b b b", 1, 3),
        object("kumi-page-to-map", "t b b b", 1, 3),
        message("kumi-page-main-on", "set 1".into()),
        message("kumi-page-main-off", "set 0".into()),
        message("kumi-page-map-on", "set 1".into()),
        message("kumi-page-map-off", "set 0".into()),
        message("kumi-page-show-main", show_main.join(", ")),
        message("kumi-page-show-map", show_map.join(", ")),
        object("kumi-page-patcher", "thispatcher", 1, 2),
        object("kumi-page-ready", "live.thisdevice", 1, 3),
    ];
    let cord = |a: &str, b: u32, c: &str, d: u32| (a.to_string(), b, c.to_string(), d);
    let cords = vec![
        cord("kumi-page-main", 0, "kumi-page-to-main", 0),
        cord("kumi-page-ready", 0, "kumi-page-to-main", 0),
        cord("kumi-page-to-main", 2, "kumi-page-main-on", 0),
        cord("kumi-page-to-main", 1, "kumi-page-map-off", 0),
        cord("kumi-page-to-main", 0, "kumi-page-show-main", 0),
        cord("kumi-page-main-on", 0, "kumi-page-main", 0),
        cord("kumi-page-map-off", 0, "kumi-page-map", 0),
        cord("kumi-page-show-main", 0, "kumi-page-patcher", 0),
        cord("kumi-page-map", 0, "kumi-page-to-map", 0),
        cord("kumi-page-to-map", 2, "kumi-page-map-on", 0),
        cord("kumi-page-to-map", 1, "kumi-page-main-off", 0),
        cord("kumi-page-to-map", 0, "kumi-page-show-map", 0),
        cord("kumi-page-map-on", 0, "kumi-page-map", 0),
        cord("kumi-page-main-off", 0, "kumi-page-main", 0),
        cord("kumi-page-show-map", 0, "kumi-page-patcher", 0),
    ];
    (boxes, cords)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::spec::{ChoiceControl, NumericControl, Place, SwitchControl, Unit};

    fn number(name: &str, group: &str) -> Control {
        Control::Number(NumericControl {
            name: name.into(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::None,
            place: Place { group: group.into(), show: Show::Auto },
        })
    }
    fn switch(name: &str, group: &str) -> Control {
        Control::Switch(SwitchControl { name: name.into(), default: false, place: Place { group: group.into(), show: Show::Auto } })
    }
    fn choice(name: &str, group: &str) -> Control {
        Control::Choice(ChoiceControl {
            name: name.into(),
            options: vec!["a".into(), "b".into()],
            default: "a".into(),
            place: Place { group: group.into(), show: Show::Auto },
        })
    }

    fn overlaps(a: &[f64; 4], b: &[f64; 4]) -> bool {
        a[0] < b[0] + b[2] - 0.5 && b[0] < a[0] + a[2] - 0.5 && a[1] < b[1] + b[3] - 0.5 && b[1] < a[1] + a[3] - 0.5
    }

    /// Every rect on the face: inside it, and none over another.
    fn tidy(face: &Face) {
        let mut rects: Vec<[f64; 4]> = Vec::new();
        for spot in &face.spots {
            rects.push(spot.rect);
            rects.extend(spot.label);
        }
        for mark in &face.marks {
            match mark {
                Mark::Heading { rect, .. } | Mark::Lane { rect, .. } | Mark::Playhead { rect, .. } => rects.push(*rect),
                Mark::Line { .. } => {}
            }
        }
        rects.extend(face.pictures.iter().copied());
        rects.extend(face.tabs);
        for rect in &rects {
            assert!(
                rect[0] >= 0.0 && rect[1] >= 0.0 && rect[0] + rect[2] <= face.width && rect[1] + rect[3] <= HEIGHT,
                "{rect:?} past {}",
                face.width
            );
        }
        for (at, a) in rects.iter().enumerate() {
            for b in &rects[at + 1..] {
                assert!(!overlaps(a, b), "{a:?} over {b:?}");
            }
        }
    }

    #[test]
    fn groups_are_sections_with_headings_in_the_order_given_and_kumis_output_last() {
        let controls = vec![
            number("Cutoff", "Filter"),
            number("Output", "Output"),
            number("Resonance", "Filter"),
            number("Rate", "LFO"),
            switch("Sync", "LFO"),
            choice("Shape", "LFO"),
            number("Depth", "LFO"),
        ];
        let mut controls = controls;
        controls[1] = Control::Number(NumericControl {
            name: "Output".into(),
            min: -36.0,
            max: 12.0,
            default: 0.0,
            unit: Unit::Db,
            place: Place { group: "Output".into(), show: Show::Auto },
        });
        let face = lay_out(&controls, &Extras::default());
        tidy(&face);
        let headings: Vec<&str> =
            face.marks.iter().filter_map(|mark| if let Mark::Heading { text, .. } = mark { Some(text.as_str()) } else { None }).collect();
        assert_eq!(headings, ["Filter", "LFO", "Output"]);
        assert_eq!(face.marks.iter().filter(|mark| matches!(mark, Mark::Line { .. })).count(), 2, "a line between sections");
        // The Filter's two dials stand in one column; the LFO's controls fill columns to the face's height.
        assert_eq!(face.spots[0].rect[0], face.spots[2].rect[0]);
        assert!(face.spots[2].rect[1] > face.spots[0].rect[1]);
        assert!(face.spots[1].rect[0] > face.spots[6].rect[0], "Output is right of everything");
        assert!(face.spots[5].label.is_some() && face.spots[0].label.is_none(), "a menu has a label; a dial shows its own name");
    }

    #[test]
    fn controls_named_for_steps_make_a_grid_with_a_playhead() {
        let mut controls = vec![choice("Rate", "Clock")];
        for step in 1..=16 {
            controls.push(switch(&format!("Gate {step}"), ""));
        }
        for step in 1..=16 {
            controls.push(number(&format!("Step {step:02} Note"), ""));
        }
        controls.push(number("Velocity 3", ""));
        let face = lay_out(&controls, &Extras { lanes: vec!["Gate".into(), "Note".into()], ..Extras::default() });
        tidy(&face);
        assert!(face.playhead);
        let lanes: Vec<&str> =
            face.marks.iter().filter_map(|mark| if let Mark::Lane { text, .. } = mark { Some(text.as_str()) } else { None }).collect();
        assert_eq!(lanes, ["Gate", "Note"]);
        assert!(face.spots[1].step && face.spots[17].step && !face.spots[33].step, "one Velocity isn't a lane");
        assert_eq!(face.spots[1].rect[1], face.spots[16].rect[1], "a lane is a row");
        assert_eq!(face.spots[1].rect[0], face.spots[17].rect[0], "a step is a column");
        assert!(face.spots[2].rect[0] > face.spots[1].rect[0]);
    }

    #[test]
    fn a_mapping_page_stands_in_the_main_page_s_place_with_tabs_to_switch() {
        let controls = vec![number("Rate", "LFO"), number("Depth", "LFO")];
        let face = lay_out(&controls, &Extras { pictures: vec![(160.0, 150.0)], map_page: Some((260.0, 158.0)), lanes: Vec::new() });
        let page = face.map_page.unwrap();
        let tabs = face.tabs.unwrap();
        assert!(tabs[0] + tabs[2] <= page[0], "the tabs left of the pages");
        assert_eq!(page[0], face.spots[0].rect[0].min(page[0]), "both pages start at the same place");
        assert!(face.width >= page[0] + page[2]);
        assert!(face.pictures[0][0] > face.spots[1].rect[0]);
    }

    #[test]
    fn many_controls_fit_the_face_s_height() {
        let controls: Vec<Control> = (0..40).map(|n| number(&format!("Knob {n}x"), if n < 20 { "A" } else { "B" })).collect();
        let face = lay_out(&controls, &Extras::default());
        tidy(&face);
    }

    #[test]
    fn numbered_controls_the_patch_doesnt_read_by_step_stay_controls() {
        let controls: Vec<Control> = (1..=4).map(|band| number(&format!("Band {band} Freq"), "EQ")).collect();
        let face = lay_out(&controls, &Extras::default());
        tidy(&face);
        assert!(!face.playhead && face.spots.iter().all(|spot| !spot.step), "four bands are four dials");
    }

    #[test]
    fn names_with_a_step_are_read_and_others_arent() {
        assert_eq!(step_of("Gate 16"), Some((16, "Gate".into())));
        assert_eq!(step_of("Step 01 Note"), Some((1, "Note".into())));
        assert_eq!(step_of("Step 3"), Some((3, "Step".into())));
        assert_eq!(step_of("Cutoff"), None);
        assert_eq!(step_of("A 1 B 2"), None, "two numbers");
        assert_eq!(step_of("Gate 0"), None);
    }
}
