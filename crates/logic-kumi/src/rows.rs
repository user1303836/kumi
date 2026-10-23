//! The rows `read`, `snapshot` and the bodies answer with, shaped from Logic's elements as the contract has them
//! (docs/en/LOGIC_CONTRACT.md) and exactly as the host's simulator writes them. What Logic shows is parsed here: a
//! fader's "+0.0 dB", a pan knob's "0 Pan", a region's help text ("Region starts at 1 bar  and ends at 5 bars ,
//! MIDI region…"), the list editors' position scrubbers (bar, beat, division, tick) and their lengths.

use serde_json::{json, Value};

use crate::ax::paths::{self, Strip};
use crate::ax::strings::Strings;
use crate::ax::Node;

/// Logic's resolution: 960 ticks to a beat, a division being a sixteenth (four to a beat) of 240 ticks, as the
/// lists count them unless the project's division is set otherwise.
pub const TICKS_PER_BEAT: u32 = 960;
pub const DIVISIONS_PER_BEAT: u32 = 4;
pub const TICKS_PER_DIVISION: u32 = TICKS_PER_BEAT / DIVISIONS_PER_BEAT;
/// The longest text a row carries from Logic (a name, a marker's text).
pub const TEXT_LIMIT: usize = 1024;

pub fn clip(text: &str) -> String {
    if text.chars().count() <= TEXT_LIMIT {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(TEXT_LIMIT - 1).collect();
    clipped.push('…');
    clipped
}

fn round(value: f64, places: i32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The first number in a text, with its sign and decimals (a comma counts as a decimal point: Logic writes what the
/// system's locale says).
pub fn first_number(text: &str) -> Option<f64> {
    let mut number = String::new();
    let mut seen_digit = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            seen_digit = true;
        } else if (c == '.' || c == ',') && seen_digit && !number.contains('.') {
            number.push('.');
        } else if (c == '-' || c == '+' || c == '−') && !seen_digit && number.is_empty() {
            number.push(if c == '+' { '+' } else { '-' });
        } else if seen_digit {
            break;
        } else {
            number.clear();
        }
    }
    number.trim_end_matches('.').parse().ok()
}

/// Every whole number in a text, in order.
pub fn numbers(text: &str) -> Vec<i64> {
    let mut out = vec![];
    let mut current = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            current.push(c);
        } else if !current.is_empty() {
            if let Ok(n) = current.parse() {
                out.push(n);
            }
            current.clear();
        }
    }
    out
}

/// A fader's level in dB from its description ("+0.0 dB", "-6.0 dB", "-∞ dB"): the fader's floor stands for silence.
pub const MIN_DB: f64 = -96.0;

pub fn db_of(description: &str) -> Option<f64> {
    if description.contains('∞') {
        return Some(MIN_DB);
    }
    first_number(description).map(|db| round(db, 1))
}

/// A header's pan knob runs 0 to 127 with 64 in the middle, so its pan is the value less 64; without a value, its
/// description ("0 Pan", "-12 Pan", "12 of 64, Left Channel Pan") is read, left being negative.
pub const PAN_CENTRE: i64 = 64;

pub fn pan_of(knob: &Node) -> i64 {
    if let Some(value) = knob.number() {
        return value as i64 - PAN_CENTRE;
    }
    let text = knob.value_desc.trim();
    let Some(pan) = first_number(text) else { return 0 };
    let left = text.contains("Left");
    if left {
        -(pan.abs() as i64)
    } else {
        pan as i64
    }
}

/// A musical position as Logic shows it, each part from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub bar: u32,
    pub beat: u32,
    pub division: u32,
    pub tick: u32,
}

impl Position {
    pub const START: Position = Position { bar: 1, beat: 1, division: 1, tick: 1 };

    pub fn to_json(&self) -> Value {
        json!({"bar": self.bar, "beat": self.beat, "division": self.division, "tick": self.tick})
    }

    /// The position `beats` from the project's start, in bars of `beats_per_bar` beats, to the nearest tick.
    pub fn from_beats(beats: f64, beats_per_bar: u32) -> Position {
        let beats_per_bar = beats_per_bar.max(1);
        let ticks = (beats.max(0.0) * TICKS_PER_BEAT as f64).round() as u64;
        let bar_ticks = u64::from(beats_per_bar) * u64::from(TICKS_PER_BEAT);
        let bar = ticks / bar_ticks;
        let in_bar = ticks % bar_ticks;
        let beat = in_bar / u64::from(TICKS_PER_BEAT);
        let in_beat = in_bar % u64::from(TICKS_PER_BEAT);
        Position {
            bar: bar as u32 + 1,
            beat: beat as u32 + 1,
            division: (in_beat / u64::from(TICKS_PER_DIVISION)) as u32 + 1,
            tick: (in_beat % u64::from(TICKS_PER_DIVISION)) as u32 + 1,
        }
    }

    /// Beats from the project's start, in a bar of `beats_per_bar` beats.
    pub fn beats(&self, beats_per_bar: u32) -> f64 {
        let bars = (self.bar.max(1) - 1) as f64 * beats_per_bar.max(1) as f64;
        let beats = (self.beat.max(1) - 1) as f64;
        let divisions = (self.division.max(1) - 1) as f64 / DIVISIONS_PER_BEAT as f64;
        let ticks = (self.tick.max(1) - 1) as f64 / TICKS_PER_BEAT as f64;
        round(bars + beats + divisions + ticks, 6)
    }
}

/// A scrubber of bar, beat, division and tick sliders (a list's Position cell, "1 2 1 1 "): its sliders' values,
/// else the numbers in the group's description.
pub fn position_of(group: &Node) -> Option<Position> {
    let values = segment_values(group);
    let parts: Vec<u32> = if values.len() >= 2 { values } else { numbers(&group.desc).into_iter().map(|n| n.max(0) as u32).collect() };
    let at = |index: usize, fallback: u32| parts.get(index).copied().unwrap_or(fallback);
    (parts.len() >= 2).then(|| Position { bar: at(0, 1), beat: at(1, 1), division: at(2, 1), tick: at(3, 1) })
}

/// A scrubber's segment sliders' values, in order.
fn segment_values(group: &Node) -> Vec<u32> {
    group.children.iter().filter(|c| c.role == "AXSlider").filter_map(|c| c.number().map(|n| n.max(0.0) as u32)).collect()
}

/// A length scrubber (bars, beats, divisions, ticks from 0: "0 0 1 0") in ticks.
pub fn length_ticks_of(group: &Node, beats_per_bar: u32) -> Option<u32> {
    let values = segment_values(group);
    let parts: Vec<u32> = if values.len() >= 2 { values } else { numbers(&group.desc).into_iter().map(|n| n.max(0) as u32).collect() };
    if parts.len() < 2 {
        return None;
    }
    let at = |index: usize| parts.get(index).copied().unwrap_or(0);
    Some(((at(0) * beats_per_bar.max(1) + at(1)) * DIVISIONS_PER_BEAT + at(2)) * TICKS_PER_DIVISION + at(3))
}

/// A length in beats from a length scrubber.
pub fn length_beats_of(group: &Node, beats_per_bar: u32) -> Option<f64> {
    length_ticks_of(group, beats_per_bar).map(|ticks| round(ticks as f64 / TICKS_PER_BEAT as f64, 6))
}

/// The numerator of a time signature ("4/4", "6/8"): beats to a bar as Logic counts them.
pub fn beats_per_bar_of(signature: &str) -> u32 {
    signature.split('/').next().and_then(|n| n.trim().parse().ok()).filter(|n| *n > 0).unwrap_or(4)
}

/// A track header: `Track 2 “Keys”` with its controls.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub index: usize,
    pub name: String,
    pub mute: bool,
    pub solo: bool,
    pub armed: bool,
    pub monitoring: bool,
    pub volume_db: Option<f64>,
    pub pan: i64,
    pub selected: bool,
    /// A Track Stack's header: it carries the disclosure triangle that shows or hides the subtracks.
    pub stack: bool,
}

impl Header {
    /// The header at `place` (from 0) among the headers.
    pub fn of(item: &Node, place: usize, strings: &Strings) -> Header {
        let checkbox = |label: &str| item.children.iter().any(|c| c.role == "AXCheckBox" && strings.is(&c.desc, label) && c.is_on());
        let name_field = item.child_where(|c| c.role == "AXTextField");
        let name = name_field.map(|f| f.desc.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| quoted(&item.desc).unwrap_or_default());
        let volume = item.child_where(|c| c.role == "AXSlider" && strings.is(&c.desc, "Volume"));
        let pan = item.child_where(|c| c.role == "AXSlider" && !strings.is(&c.desc, "Volume"));
        Header {
            index: numbers(&item.desc).first().map_or(place + 1, |n| *n as usize),
            name: clip(&name),
            mute: checkbox("Mute"),
            solo: checkbox("Solo"),
            armed: checkbox("Record Enable"),
            monitoring: checkbox("Input Monitoring"),
            volume_db: volume.and_then(|v| db_of(&v.value_desc)),
            pan: pan.map_or(0, pan_of),
            selected: item.is_selected(),
            stack: item.children.iter().any(|c| c.role == "AXDisclosureTriangle"),
        }
    }
}

/// The text between a label's quotes: `Track 2 “Keys”` → `Keys` (Logic quotes with the language's marks).
pub fn quoted(text: &str) -> Option<String> {
    let open = text.find(['“', '"', '„', '«', '「'])?;
    let rest = &text[open..];
    let mut chars = rest.chars();
    chars.next();
    let inner: String = chars.collect();
    let close = inner.rfind(['”', '"', '“', '»', '」'])?;
    Some(inner[..close].to_string())
}

/// A region in the Tracks area: its item's name, its help text's start and end, its selection.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub track_index: usize,
    pub name: String,
    pub start: Position,
    pub end: Position,
    pub midi: bool,
    pub selected: bool,
    pub looped: bool,
}

/// The positions a region's help text names ("Region starts at 1 bar  and ends at 5 bars , MIDI region…"): each
/// unit's number is read by the word after it, in the producer's language, so a part Logic leaves out (a beat of 1)
/// stays 1.
pub fn help_positions(help: &str, strings: &Strings) -> Vec<Position> {
    let words: Vec<&str> = help.split(|c: char| c.is_whitespace() || c == ',' || c == '.').filter(|w| !w.is_empty()).collect();
    let mut positions: Vec<Position> = vec![];
    let unit = |word: &str| -> Option<usize> {
        for (index, labels) in [["bar", "bars"], ["beat", "beats"], ["division", "divisions"], ["tick", "ticks"]].iter().enumerate() {
            if labels.iter().any(|label| strings.is(word, label)) {
                return Some(index);
            }
        }
        None
    };
    for pair in words.windows(2) {
        let (Ok(number), Some(unit)) = (pair[0].parse::<u32>(), unit(pair[1])) else { continue };
        if unit == 0 {
            positions.push(Position { bar: number, ..Position::START });
            continue;
        }
        let Some(position) = positions.last_mut() else { continue };
        match unit {
            1 => position.beat = number,
            2 => position.division = number,
            _ => position.tick = number,
        }
    }
    positions
}

impl Region {
    pub fn of(item: &Node, track_index: usize, strings: &Strings) -> Option<Region> {
        let positions = help_positions(&item.help, strings);
        let (start, end) = match positions.as_slice() {
            [start, end, ..] => (*start, *end),
            [start] => (*start, *start),
            [] => return None,
        };
        let name = if item.desc.is_empty() {
            item.child_where(|c| c.role == "AXTextField").map(|c| c.desc.clone()).unwrap_or_default()
        } else {
            item.desc.clone()
        };
        Some(Region {
            track_index,
            name: clip(&name),
            start,
            end,
            midi: item.help.contains("MIDI") || !strings.is("", "x") && item.help.to_lowercase().contains("midi"),
            selected: item.is_selected(),
            looped: item.children.iter().any(|c| c.role == "AXHandle" && strings.is(&c.desc, "loop") && c.is_on()),
        })
    }

    pub fn kind(&self) -> &'static str {
        if self.midi {
            "midi"
        } else {
            "audio"
        }
    }
}

/// A note from an Event List row: the cells by their columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub position: Position,
    pub pitch: u32,
    pub velocity: u32,
    pub length_ticks: u32,
    pub channel: u32,
    pub selected: bool,
}

/// The cell of a row under a column id, by the columns' order.
fn cell<'a>(row: &'a Node, columns: &[(String, usize)], id: &str) -> Option<&'a Node> {
    let index = columns.iter().find(|(name, _)| name == id)?.1;
    row.children.iter().filter(|c| c.role == "AXCell").nth(index)
}

/// A cell's slider, or its first child.
fn cell_control(cell: &Node) -> Option<&Node> {
    cell.child_where(|c| c.role == "AXSlider").or_else(|| cell.children.first())
}

impl Note {
    /// A row of the Event List inside a region, when it is a note (its Status cell says so).
    pub fn of(row: &Node, columns: &[(String, usize)], beats_per_bar: u32, strings: &Strings) -> Option<Note> {
        let status = cell(row, columns, "Status").and_then(|c| c.children.first()).map(|c| c.desc.clone()).unwrap_or_default();
        if !strings.is(&status, "Note") {
            return None;
        }
        let position = cell(row, columns, "Position").and_then(|c| c.children.first()).and_then(position_of)?;
        let pitch = cell(row, columns, "Num").and_then(cell_control).and_then(Node::number)? as u32;
        let velocity_cell = cell(row, columns, "Val").and_then(cell_control)?;
        // The velocity slider's value is scaled oddly; its description says the number.
        let velocity = first_number(&velocity_cell.value_desc).or_else(|| velocity_cell.number())? as u32;
        let channel = cell(row, columns, "Ch").and_then(cell_control).and_then(Node::number).unwrap_or(1.0) as u32;
        let length_ticks =
            cell(row, columns, "Length").and_then(|c| c.children.first()).and_then(|g| length_ticks_of(g, beats_per_bar)).unwrap_or(0);
        Some(Note { position, pitch, velocity, length_ticks, channel, selected: row.is_selected() })
    }
}

/// An automation event from the Track Automation Event List: a "Fader" row, its position, the parameter's number
/// (Num) and name (Logic writes it in the Length/Info cell: "Volume", "Balance"), and its value (Val, 0 to 127).
#[derive(Debug, Clone, PartialEq)]
pub struct AutomationEvent {
    pub position: Position,
    pub number: u32,
    pub parameter: String,
    pub value: u32,
}

impl AutomationEvent {
    pub fn of(row: &Node, columns: &[(String, usize)], strings: &Strings) -> Option<AutomationEvent> {
        let status = cell(row, columns, "Status").and_then(|c| c.children.first()).map(|c| c.desc.clone()).unwrap_or_default();
        if !strings.is(&status, "Fader") {
            return None;
        }
        let position = cell(row, columns, "Position").and_then(|c| c.children.first()).and_then(position_of)?;
        let number = cell(row, columns, "Num").and_then(cell_control).and_then(Node::number)? as u32;
        let value_cell = cell(row, columns, "Val").and_then(cell_control)?;
        let value = first_number(&value_cell.value_desc).or_else(|| value_cell.number())? as u32;
        let parameter = cell(row, columns, "Length").map(|c| {
            let inner = c.children.first();
            inner.map(|n| if n.desc.is_empty() { n.value_text() } else { n.desc.clone() }).unwrap_or_default()
        });
        Some(AutomationEvent { position, number, parameter: clip(parameter.unwrap_or_default().trim()), value })
    }
}

/// A marker from the Marker List: its position, name, length and text.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub position: Position,
    pub name: String,
    pub length_beats: f64,
    pub text: String,
}

impl Marker {
    pub fn of(row: &Node, columns: &[(String, usize)], beats_per_bar: u32) -> Option<Marker> {
        let position = cell(row, columns, "Position").and_then(|c| c.children.first()).and_then(position_of)?;
        let name = cell(row, columns, "Name").map(cell_text).unwrap_or_default();
        let length_beats =
            cell(row, columns, "Length").and_then(|c| c.children.first()).and_then(|g| length_beats_of(g, beats_per_bar)).unwrap_or(0.0);
        let text = cell(row, columns, "Text").map(cell_text).unwrap_or_default();
        Some(Marker { position, name: clip(&name), length_beats, text: clip(&text) })
    }
}

/// A cell's text: its text field's value, else its first child's value or description.
fn cell_text(cell: &Node) -> String {
    let field = cell.find(&|n| n.role == "AXTextField" || n.role == "AXStaticText");
    match field {
        Some(field) if !field.value_text().is_empty() => field.value_text(),
        Some(field) => field.desc.clone(),
        None => cell.children.first().map(|c| if c.value_text().is_empty() { c.desc.clone() } else { c.value_text() }).unwrap_or_default(),
    }
}

/// A tempo event from the Tempo List: the tempo cell's group says the number ("160.0000").
pub fn tempo_of(row: &Node, columns: &[(String, usize)]) -> Option<(Position, f64)> {
    let position = cell(row, columns, "Position").and_then(|c| c.children.first()).and_then(position_of)?;
    let tempo = cell(row, columns, "Tempo").and_then(|c| c.children.first())?;
    let bpm = first_number(&tempo.desc).or_else(|| tempo.children.iter().find(|c| c.role == "AXSlider").and_then(Node::number))?;
    Some((position, round(bpm, 4)))
}

/// A signature event from the Signature List: a time signature (a numerator slider and a "/4" pop-up) or a key
/// signature (a pop-up), at a position.
#[derive(Debug, Clone, PartialEq)]
pub enum SignatureEvent {
    Time { position: Position, signature: String },
    Key { position: Position, key: String },
}

pub fn signature_of(row: &Node, columns: &[(String, usize)], strings: &Strings) -> Option<SignatureEvent> {
    let position = cell(row, columns, "Position").and_then(|c| c.children.first()).and_then(position_of).unwrap_or(Position::START);
    let kind = cell(row, columns, "Type").and_then(|c| c.children.first()).map(|c| c.desc.clone()).unwrap_or_default();
    let value = cell(row, columns, "Val")?;
    if strings.is(&kind, "Key") {
        let key = value.find(&|n| n.role == "AXPopUpButton").map(Node::value_text).unwrap_or_default();
        return Some(SignatureEvent::Key { position, key });
    }
    let numerator = value.find(&|n| n.role == "AXSlider").and_then(Node::number).map(|n| n as u32);
    let denominator = value.find(&|n| n.role == "AXPopUpButton").map(Node::value_text).unwrap_or_default();
    let numerator = numerator?;
    Some(SignatureEvent::Time { position, signature: format!("{numerator}{}", denominator.trim()) })
}

/// What a channel strip says of a track: its kind, its controls, its routing and its inserts.
#[derive(Debug, Clone, PartialEq)]
pub struct StripState {
    pub name: String,
    pub kind: &'static str,
    pub mute: bool,
    pub solo: bool,
    pub armed: bool,
    pub monitoring: bool,
    pub volume_db: Option<f64>,
    pub pan: i64,
    pub output: String,
    pub inputs: Vec<String>,
    /// The inserts in slot order: name, bypassed, kind (instrument or effect).
    pub inserts: Vec<(String, bool, &'static str)>,
    /// The sends in slot order: destination and level in dB.
    pub sends: Vec<(String, Option<f64>)>,
    /// Whether the Mixer drew the strip for Accessibility. A strip it hasn't drawn (right of the Mixer's view, in a
    /// project wider than the pane) is listed with numbers for its description ("224 40 4") and generic slots ("input",
    /// "audio plug-in", each bypassed): its routing, plug-ins and sends aren't taken from it.
    pub drawn: bool,
}

/// Whether a strip's description is the Mixer's placeholder for a strip it hasn't drawn: numbers alone ("224 40 4"),
/// where a drawn strip's is its name.
pub fn undrawn(description: &str, name: &str) -> bool {
    let tokens: Vec<&str> = description.split_whitespace().collect();
    description != name && tokens.len() >= 2 && tokens.iter().all(|token| token.parse::<u32>().is_ok())
}

impl StripState {
    pub fn of(strip: &Node, strings: &Strings) -> StripState {
        let parts = Strip::of(strip, strings);
        let on = |control: Option<&Node>| control.is_some_and(Node::is_on);
        let kind = if parts.bounce.is_some() {
            "output"
        } else if parts.instrument.is_some() {
            "instrument"
        } else if parts.inputs.iter().any(|input| strings.starts_with(&input.desc, "Bus")) {
            "aux"
        } else if parts.inputs.is_empty() && parts.record.is_none() {
            "external"
        } else {
            "audio"
        };
        // Logic lists the slots bottom-up; the rows count them from the top.
        let mut inserts: Vec<(String, bool, &'static str)> = vec![];
        if let Some(instrument) = parts.instrument.filter(|slot| paths::occupied(slot)) {
            inserts.push((clip(&instrument.desc), bypassed(instrument, strings), "instrument"));
        }
        for insert in parts.inserts.iter().rev().filter(|slot| paths::occupied(slot)) {
            inserts.push((clip(&insert.desc), bypassed(insert, strings), "effect"));
        }
        let knobs = send_knobs(strip, &parts.sends, strings);
        let sends = parts
            .sends
            .iter()
            .zip(knobs)
            .rev()
            .filter(|(send, _)| paths::occupied(send) || !(strings.is(&send.desc, "send button") || send.desc.is_empty()))
            .map(|(send, knob)| (clip(&send.desc), knob.and_then(|k| db_of(&k.value_desc)).or_else(|| db_of(&send.value_desc))))
            .collect();
        let name = parts.name.map(Node::value_text).unwrap_or_else(|| strip.desc.clone());
        if undrawn(&strip.desc, &name) {
            return StripState {
                name,
                kind,
                mute: on(parts.mute),
                solo: on(parts.solo),
                armed: on(parts.record),
                monitoring: on(parts.monitoring),
                volume_db: None,
                pan: 0,
                output: String::new(),
                inputs: vec![],
                inserts: vec![],
                sends: vec![],
                drawn: false,
            };
        }
        StripState {
            name,
            kind,
            mute: on(parts.mute),
            solo: on(parts.solo),
            armed: on(parts.record),
            monitoring: on(parts.monitoring),
            volume_db: parts.volume.and_then(|v| db_of(&v.value_desc)),
            pan: parts.pan.map_or(0, |p| first_number(&p.value_desc).map_or(0, |n| n as i64)),
            output: parts.output.map(|o| clip(&o.desc)).unwrap_or_default(),
            inputs: parts.inputs.iter().map(|i| clip(&i.desc)).filter(|i| !i.is_empty()).collect(),
            inserts,
            sends,
            drawn: true,
        }
    }
}

/// Whether an occupied slot's bypass checkbox is on.
fn bypassed(slot: &Node, strings: &Strings) -> bool {
    slot.children.iter().any(|c| c.role == "AXCheckBox" && strings.is(&c.desc, "bypass") && c.is_on())
}

/// The level knob of each send slot of a strip (`slots` as `Strip::of` lists them, bottom-up), `None` for an empty
/// slot. Logic lists the knobs as the strip's own sliders after the send groups, in the groups' order: right after a
/// lone send's group, after all of them when there are several (measured on Logic Pro 12.3.1); a knob inside a group
/// is taken first. When the knobs and the sends don't come out even, no level is read rather than a wrong one.
pub fn send_knobs<'a>(strip: &'a Node, slots: &[&'a Node], strings: &Strings) -> Vec<Option<&'a Node>> {
    let is_knob = |n: &Node| n.role == "AXSlider" && (strings.is(&n.desc, "send knob") || strings.is(&n.desc, "send level"));
    let loose: Vec<&Node> = strip.children.iter().filter(|c| is_knob(c)).collect();
    let occupied = slots.iter().filter(|slot| paths::occupied(slot) && slot.find(&is_knob).is_none()).count();
    let mut next = loose.iter();
    slots
        .iter()
        .map(|slot| {
            if !paths::occupied(slot) {
                return None;
            }
            if let Some(inside) = slot.find(&is_knob) {
                return Some(inside);
            }
            next.next().copied().filter(|_| loose.len() == occupied)
        })
        .collect()
}

/// A send knob's description as dB: Logic writes the number alone ("-6.0", "-∞").
pub fn send_db(description: &str) -> Option<f64> {
    db_of(description)
}

/// A track's row: the header's live state, the strip's kind and routing when known.
pub fn track_row(reference: &str, header: &Header, strip: Option<&StripState>) -> Value {
    let mut row = json!({
        "ref": reference,
        "index": header.index,
        "name": header.name,
        "kind": strip.map_or("audio", |s| s.kind),
        "mute": header.mute,
        "solo": header.solo,
        "armed": header.armed,
        "monitoring": header.monitoring,
        "volumeDb": header.volume_db.or(strip.and_then(|s| s.volume_db)).unwrap_or(0.0),
        "pan": header.pan,
        "color": Value::Null,
        "output": strip.map(|s| s.output.clone()).unwrap_or_default(),
        "inputs": strip.map(|s| s.inputs.clone()).unwrap_or_default(),
        "selected": header.selected,
    });
    // Only a stack's header says so: the rows of plain tracks are as they were.
    if header.stack {
        row["stack"] = true.into();
    }
    // Only a track whose Mixer strip wasn't drawn says so: its kind is the strip's, its routing, plug-ins and sends
    // unread.
    if strip.is_some_and(|s| !s.drawn) {
        row["stripUnread"] = true.into();
    }
    row
}

pub fn region_row(reference: &str, track_ref: &str, region: &Region, beats_per_bar: u32) -> Value {
    json!({
        "ref": reference,
        "trackRef": track_ref,
        "name": region.name,
        "startBeats": region.start.beats(beats_per_bar),
        "endBeats": region.end.beats(beats_per_bar),
        "kind": region.kind(),
        "selected": region.selected,
        "looped": region.looped,
    })
}

pub fn note_row(reference: &str, region_ref: &str, note: &Note) -> Value {
    let mut row = json!({
        "ref": reference,
        "regionRef": region_ref,
        "position": note.position.to_json(),
        "pitch": note.pitch,
        "velocity": note.velocity,
        "lengthTicks": note.length_ticks,
        "channel": note.channel,
    });
    if note.selected {
        row["selected"] = true.into();
    }
    row
}

pub fn plugin_row(reference: &str, track_ref: &str, slot: usize, insert: &(String, bool, &'static str)) -> Value {
    json!({"ref": reference, "trackRef": track_ref, "slot": slot, "name": insert.0, "bypassed": insert.1, "kind": insert.2})
}

pub fn send_row(reference: &str, track_ref: &str, slot: usize, send: &(String, Option<f64>)) -> Value {
    json!({"ref": reference, "trackRef": track_ref, "slot": slot, "destination": send.0, "level": send.1.unwrap_or(0.0)})
}

pub fn marker_row(reference: &str, marker: &Marker) -> Value {
    json!({"ref": reference, "position": marker.position.to_json(), "name": marker.name, "length": marker.length_beats, "text": marker.text})
}

/// The tempo row: the project's tempo, the Tempo List's events (`tempos`, the first the project's tempo at its start,
/// then each change of tempo after it, by position) and its signatures (time and key, each at a position).
pub fn tempo_row(bpm: f64, tempos: &[(Position, f64)], signatures: &[Value]) -> Value {
    let mut events: Vec<Value> = tempos.iter().map(|(position, bpm)| json!({"position": position.to_json(), "bpm": bpm})).collect();
    if events.is_empty() {
        events.push(json!({"position": Position::START.to_json(), "bpm": bpm}));
    }
    json!({"bpm": bpm, "tempos": events, "signatures": signatures})
}

/// The signatures as the tempo row lists them: one entry per position, with the time and key in force there.
pub fn signatures(events: &[SignatureEvent], first_time: &str, first_key: &str) -> Vec<Value> {
    let mut out: Vec<(Position, String, String)> = vec![];
    let (mut time, mut key) = (first_time.to_string(), first_key.to_string());
    for event in events {
        let position = match event {
            SignatureEvent::Time { position, signature } => {
                time = signature.clone();
                *position
            }
            SignatureEvent::Key { position, key: k } => {
                key = k.clone();
                *position
            }
        };
        match out.iter_mut().find(|(at, _, _)| *at == position) {
            Some(entry) => {
                entry.1 = time.clone();
                entry.2 = key.clone();
            }
            None => out.push((position, time.clone(), key.clone())),
        }
    }
    if out.is_empty() {
        out.push((Position::START, time, key));
    }
    out.into_iter().map(|(position, time, key)| json!({"position": position.to_json(), "time": time, "key": key})).collect()
}

pub fn project_row(
    id: Option<&str>,
    identity: &str,
    path: Option<&str>,
    tempo: f64,
    time_signature: &str,
    key_signature: &str,
    sample_rate: Option<u32>,
    track_count: usize,
    middle_c: u8,
) -> Value {
    json!({
        "id": id,
        "identity": identity,
        "path": path,
        "tempo": tempo,
        "timeSignature": time_signature,
        "keySignature": key_signature,
        "sampleRate": sample_rate,
        "trackCount": track_count,
        "middleC": middle_c,
    })
}

/// The octave Logic names middle C (note 60) in, from its "Display Middle C As" setting (Settings > View > General):
/// the preferences' `MiddleC` switch on is C4 (Roland), off or never set C3 (Yamaha), Logic's default.
pub fn middle_c_of(preference: Option<&str>) -> u8 {
    match preference.map(str::trim) {
        Some("1") | Some("true") | Some("YES") => 4,
        _ => 3,
    }
}

/// A list editor's rows of one kind, with the table's columns resolved once.
pub fn table_rows<'a>(table: &'a Node) -> (Vec<(String, usize)>, Vec<&'a Node>) {
    (paths::columns(table), paths::rows(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(value: Value) -> Node {
        Node::from_json(&value)
    }

    #[test]
    fn numbers_and_levels_are_read_from_what_logic_writes() {
        assert_eq!(first_number("+0.0 dB"), Some(0.0));
        assert_eq!(first_number("-6.0 dB"), Some(-6.0));
        assert_eq!(first_number("-6,5 dB"), Some(-6.5));
        assert_eq!(first_number("peak level meter, 23.9 dB"), Some(23.9));
        assert_eq!(first_number("0 Pan"), Some(0.0));
        assert_eq!(first_number("-12 Pan"), Some(-12.0));
        assert_eq!(first_number("C3"), Some(3.0));
        assert_eq!(first_number("none"), None);
        assert_eq!(db_of("-∞ dB"), Some(MIN_DB));
        assert_eq!(db_of("+0.0 dB"), Some(0.0));
        assert_eq!(numbers("Track 12 “Keys 3”"), [12, 3]);
        assert_eq!(quoted("Track 2 “Keys”").as_deref(), Some("Keys"));
        assert_eq!(quoted("Spur 2 „Keys“").as_deref(), Some("Keys"));
        assert_eq!(quoted("Track 2"), None);
        assert_eq!(beats_per_bar_of("6/8"), 6);
        assert_eq!(beats_per_bar_of("x"), 4);
        let knob = node(json!({"role": "AXSlider", "value": 64, "valueDesc": "0 Pan"}));
        assert_eq!(pan_of(&knob), 0);
        assert_eq!(pan_of(&node(json!({"role": "AXSlider", "value": 52}))), -12);
        assert_eq!(pan_of(&node(json!({"role": "AXSlider", "value": 62, "valueDesc": "2 of 64, Left Channel Pan"}))), -2);
        assert_eq!(pan_of(&node(json!({"role": "AXSlider", "valueDesc": "2 of 64, Left Channel Pan"}))), -2);
        assert_eq!(pan_of(&node(json!({"role": "AXSlider", "valueDesc": "12 Pan"}))), 12);
    }

    #[test]
    fn positions_and_lengths_come_from_the_lists_scrubbers() {
        let group = node(json!({"role": "AXGroup", "desc": "1 2 1 1 ", "c": [
            {"role": "AXSlider", "desc": "Segment 0", "value": 1}, {"role": "AXSlider", "desc": "Segment 1", "value": 2},
            {"role": "AXSlider", "desc": "Segment 2", "value": 1}, {"role": "AXSlider", "desc": "Segment 3", "value": 1}
        ]}));
        let position = position_of(&group).unwrap();
        assert_eq!(position, Position { bar: 1, beat: 2, division: 1, tick: 1 });
        assert_eq!(position.beats(4), 1.0);
        assert_eq!(Position { bar: 9, beat: 1, division: 1, tick: 1 }.beats(4), 32.0);
        assert_eq!(Position { bar: 2, beat: 2, division: 3, tick: 121 }.beats(3), 4.0 + 0.5 + 0.125);
        assert_eq!(Position::from_beats(4.625, 3), Position { bar: 2, beat: 2, division: 3, tick: 121 });
        assert_eq!(Position::from_beats(32.0, 4), Position { bar: 9, ..Position::START });
        assert_eq!(Position::from_beats(0.0, 4), Position::START);
        assert_eq!(Position::from_beats(1.0, 4).beats(4), 1.0);
        // Without sliders, the description's numbers serve.
        let bare = node(json!({"role": "AXGroup", "desc": "9 1 1 1 "}));
        assert_eq!(position_of(&bare).map(|p| p.bar), Some(9));
        assert_eq!(position_of(&node(json!({"role": "AXGroup"}))), None);
        let length = node(json!({"role": "AXGroup", "desc": "0 0 1 0", "c": [
            {"role": "AXSlider", "value": 0}, {"role": "AXSlider", "value": 0}, {"role": "AXSlider", "value": 1}, {"role": "AXSlider", "value": 0}
        ]}));
        assert_eq!(length_ticks_of(&length, 4), Some(240));
        assert_eq!(length_ticks_of(&node(json!({"role": "AXGroup", "desc": "0 0 2 0"})), 4), Some(480));
        assert_eq!(length_ticks_of(&node(json!({"role": "AXGroup", "desc": "8 0 0 0"})), 4), Some(8 * 4 * 960));
        assert_eq!(length_beats_of(&node(json!({"role": "AXGroup", "desc": "8 0 0 0"})), 4), Some(32.0));
    }

    #[test]
    fn a_header_and_a_region_are_read_from_the_tracks_area() {
        let strings = Strings::english();
        let item = node(json!({"role": "AXLayoutItem", "desc": "Track 2 “Keys”", "selected": false, "c": [
            {"role": "AXRadioButton", "desc": "Has Focus", "value": 0},
            {"role": "AXCheckBox", "desc": "Mute", "value": 1},
            {"role": "AXCheckBox", "desc": "Solo", "value": 0},
            {"role": "AXCheckBox", "desc": "Record Enable", "value": 0},
            {"role": "AXCheckBox", "desc": "Input Monitoring", "value": 1},
            {"role": "AXSlider", "value": 52, "valueDesc": "-12 Pan"},
            {"role": "AXSlider", "desc": "Volume", "value": 150, "valueDesc": "-3.0 dB"},
            {"role": "AXTextField", "desc": "Keys", "value": 0}
        ]}));
        let header = Header::of(&item, 1, &strings);
        assert_eq!(
            header,
            Header {
                index: 2,
                name: "Keys".into(),
                mute: true,
                solo: false,
                armed: false,
                monitoring: true,
                volume_db: Some(-3.0),
                pan: -12,
                selected: false,
                stack: false,
            }
        );
        let row = track_row("1:track:2", &header, None);
        assert_eq!(row["kind"], "audio");
        assert_eq!(row["volumeDb"], -3.0);
        assert_eq!(row["inputs"], json!([]));

        let region = node(
            json!({"role": "AXLayoutItem", "desc": "Keys", "selected": true, "help": "Region starts at 1 bar  and ends at 5 bars , MIDI region. Contains MIDI note and controller events. ", "c": [
                {"role": "AXTextField", "desc": "Keys"}, {"role": "AXHandle", "desc": "loop"}
            ]}),
        );
        let region = Region::of(&region, 2, &strings).unwrap();
        assert_eq!((region.start.bar, region.end.bar, region.midi, region.selected, region.looped), (1, 5, true, true, false));
        let row = region_row("1:region:1", "1:track:2", &region, 4);
        assert_eq!((row["startBeats"].as_f64(), row["endBeats"].as_f64(), row["kind"].as_str()), (Some(0.0), Some(16.0), Some("midi")));
        let audio = node(
            json!({"role": "AXLayoutItem", "desc": "Vocal", "help": "Region starts at 2 bars 3 beats  and ends at 4 bars 1 beat 3 divisions , audio region. "}),
        );
        let audio = Region::of(&audio, 1, &strings).unwrap();
        assert_eq!((audio.start, audio.end.division, audio.kind()), (Position { bar: 2, beat: 3, division: 1, tick: 1 }, 3, "audio"));
        assert_eq!(audio.start.beats(4), 6.0);
        assert_eq!(Region::of(&node(json!({"role": "AXLayoutItem", "desc": "x"})), 1, &strings), None);
    }

    #[test]
    fn notes_markers_tempo_and_signatures_are_read_from_the_lists() {
        let strings = Strings::english();
        let columns = vec![
            ("Lock".to_string(), 0),
            ("Muted".to_string(), 1),
            ("Position".to_string(), 2),
            ("Status".to_string(), 3),
            ("Ch".to_string(), 4),
            ("Num".to_string(), 5),
            ("Val".to_string(), 6),
            ("Length".to_string(), 7),
        ];
        let row = node(json!({"role": "AXRow", "selected": false, "c": [
            {"role": "AXCell"}, {"role": "AXCell"},
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "1 4 1 1 ", "c": [{"role": "AXSlider", "value": 1}, {"role": "AXSlider", "value": 4}, {"role": "AXSlider", "value": 1}, {"role": "AXSlider", "value": 1}]}]},
            {"role": "AXCell", "c": [{"role": "AXCell", "desc": "Note"}]},
            {"role": "AXCell", "c": [{"role": "AXSlider", "value": 1, "valueDesc": "1"}]},
            {"role": "AXCell", "c": [{"role": "AXSlider", "value": 72, "valueDesc": "C4"}]},
            {"role": "AXCell", "c": [{"role": "AXSlider", "value": 3715432448u64, "valueDesc": "110"}]},
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "0 0 2 0", "c": [{"role": "AXSlider", "value": 0}, {"role": "AXSlider", "value": 0}, {"role": "AXSlider", "value": 2}, {"role": "AXSlider", "value": 0}]}]}
        ]}));
        let note = Note::of(&row, &columns, 4, &strings).unwrap();
        assert_eq!(
            note,
            Note {
                position: Position { bar: 1, beat: 4, division: 1, tick: 1 },
                pitch: 72,
                velocity: 110,
                length_ticks: 480,
                channel: 1,
                selected: false
            }
        );
        assert_eq!(note_row("1:note:1.4", "1:region:1", &note)["lengthTicks"], 480);
        let controller = node(
            json!({"role": "AXRow", "c": [{"role": "AXCell"}, {"role": "AXCell"}, {"role": "AXCell"}, {"role": "AXCell", "c": [{"role": "AXCell", "desc": "Control"}]}]}),
        );
        assert_eq!(Note::of(&controller, &columns, 4, &strings), None);

        let marker_columns = vec![("Lock".to_string(), 0), ("Position".to_string(), 1), ("Name".to_string(), 2), ("Length".to_string(), 3)];
        let marker = node(json!({"role": "AXRow", "c": [
            {"role": "AXCell"},
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "9 1 1 1 "}]},
            {"role": "AXCell", "c": [{"role": "AXTextField", "value": "Chorus"}]},
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "8 0 0 0"}]}
        ]}));
        let marker = Marker::of(&marker, &marker_columns, 4).unwrap();
        assert_eq!((marker.position.bar, marker.name.as_str(), marker.length_beats, marker.text.as_str()), (9, "Chorus", 32.0, ""));
        assert_eq!(marker_row("1:marker:1", &marker)["length"], 32.0);

        let tempo_columns = vec![("Position".to_string(), 0), ("Tempo".to_string(), 1), ("SMPTEPos".to_string(), 2)];
        let tempo = node(json!({"role": "AXRow", "c": [
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "1 1 1 1 "}]},
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "160.0000", "c": [{"role": "AXSlider", "value": 160}]}]},
            {"role": "AXCell"}
        ]}));
        assert_eq!(tempo_of(&tempo, &tempo_columns), Some((Position::START, 160.0)));

        let signature_columns = vec![("Position".to_string(), 0), ("Type".to_string(), 1), ("Val".to_string(), 2)];
        let time = node(json!({"role": "AXRow", "c": [
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "1 1 1 1 "}]},
            {"role": "AXCell", "c": [{"role": "AXCell", "desc": "Time"}]},
            {"role": "AXCell", "c": [{"role": "AXSlider", "value": 4}, {"role": "AXPopUpButton", "value": "/4"}]}
        ]}));
        let key = node(json!({"role": "AXRow", "c": [
            {"role": "AXCell", "c": [{"role": "AXGroup", "desc": "1 1 1 1 "}]},
            {"role": "AXCell", "c": [{"role": "AXCell", "desc": "Key"}]},
            {"role": "AXCell", "c": [{"role": "AXPopUpButton", "value": "C Major"}]}
        ]}));
        let events =
            vec![signature_of(&time, &signature_columns, &strings).unwrap(), signature_of(&key, &signature_columns, &strings).unwrap()];
        assert_eq!(events[0], SignatureEvent::Time { position: Position::START, signature: "4/4".into() });
        assert_eq!(events[1], SignatureEvent::Key { position: Position::START, key: "C Major".into() });
        let listed = signatures(&events, "4/4", "C Major");
        assert_eq!(listed, vec![json!({"position": Position::START.to_json(), "time": "4/4", "key": "C Major"})]);
        assert_eq!(signatures(&[], "3/4", "A minor")[0]["time"], "3/4");
        let row = tempo_row(160.0, &[], &listed);
        assert_eq!(row["bpm"], 160.0);
        assert_eq!(
            row["tempos"],
            json!([{"position": Position::START.to_json(), "bpm": 160.0}]),
            "the project's tempo when no list was read"
        );
        let later = Position { bar: 9, ..Position::START };
        let row = tempo_row(160.0, &[(Position::START, 160.0), (later, 120.0)], &listed);
        assert_eq!(row["tempos"][1], json!({"position": later.to_json(), "bpm": 120.0}));
    }

    #[test]
    fn a_strip_the_mixer_hasnt_drawn_gives_no_routing_plug_ins_or_sends() {
        // Recorded over 40 tracks: Keys' strip, right of the Mixer's view, as Accessibility listed it (Piano, Channel
        // EQ, Compressor and ChromaVerb on it read as "input" and "audio plug-in", bypassed).
        let strings = Strings::english();
        let bypassed = |desc: &str| json!({"role": "AXGroup", "desc": desc, "c": [{"role": "AXCheckBox", "desc": "bypass", "value": true}, {"role": "AXButton", "desc": "open"}, {"role": "AXButton", "desc": "list"}]});
        let strip = node(json!({"role": "AXLayoutItem", "desc": "224 40 4", "c": [
            {"role": "AXTextField", "desc": "name", "value": "Keys"},
            {"role": "AXButton", "subrole": "AXSwitch", "desc": "mute", "value": "off"},
            {"role": "AXButton", "subrole": "AXSwitch", "desc": "solo", "value": "off"},
            {"role": "AXSlider", "desc": "volume fader", "value": 173, "valueDesc": "0.0 dB"},
            {"role": "AXSlider", "desc": "pan", "value": 0, "valueDesc": "0"},
            {"role": "AXButton", "desc": "output", "help": "Output slot. Click and hold to choose the channel strip output."},
            {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal to an aux channel strip."},
            {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
            {"role": "AXButton", "desc": "insert bar"}, bypassed("audio plug-in"),
            {"role": "AXButton", "desc": "insert bar"}, bypassed("audio plug-in"),
            {"role": "AXButton", "desc": "insert bar"}, bypassed("audio plug-in"),
            bypassed("input"),
            {"role": "AXButton", "desc": "MIDI plug-in", "help": "MIDI Effect slot. Insert a MIDI effect."}
        ]}));
        let state = StripState::of(&strip, &strings);
        assert!(!state.drawn);
        assert_eq!((state.name.as_str(), state.kind), ("Keys", "instrument"), "its kind follows its slots' shape");
        assert!(state.inserts.is_empty() && state.sends.is_empty() && state.output.is_empty());
        let header = Header {
            index: 40,
            name: "Keys".into(),
            mute: false,
            solo: false,
            armed: false,
            monitoring: false,
            volume_db: Some(0.0),
            pan: 0,
            selected: false,
            stack: false,
        };
        assert_eq!(track_row("1:track:40", &header, Some(&state))["stripUnread"], true);
        assert!(track_row("1:track:40", &header, None).get("stripUnread").is_none());
        assert!(undrawn("84 44 8", "Master") && !undrawn("Audio 1", "Vocal") && !undrawn("12 34", "12 34"));
    }

    #[test]
    fn a_strip_says_the_tracks_kind_its_routing_its_inserts_and_its_sends() {
        let strings = Strings::english();
        let strip = node(json!({"role": "AXLayoutItem", "desc": "Audio 1", "c": [
            {"role": "AXTextField", "desc": "name", "value": "Vocal"},
            {"role": "AXButton", "subrole": "AXSwitch", "desc": "mute", "value": "on"},
            {"role": "AXButton", "subrole": "AXSwitch", "desc": "solo", "value": "off"},
            {"role": "AXButton", "subrole": "AXSwitch", "desc": "record", "value": "off"},
            {"role": "AXSlider", "desc": "volume fader", "value": 173, "valueDesc": "0.0 dB"},
            {"role": "AXSlider", "desc": "pan", "value": 0, "valueDesc": "0"},
            {"role": "AXButton", "desc": "Stereo Output", "help": "Output slot. Click and hold."},
            {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal."},
            {"role": "AXGroup", "desc": "Bus 1", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}, {"role": "AXSlider", "desc": "send knob", "valueDesc": "-6.0 dB"}]},
            {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
            {"role": "AXButton", "desc": "insert bar"},
            {"role": "AXGroup", "desc": "Channel EQ", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"role": "AXButton", "desc": "Input 1", "help": "Input slot. Choose the input."},
            {"role": "AXButton", "desc": "EQ", "value": "on"}
        ]}));
        let state = StripState::of(&strip, &strings);
        assert_eq!((state.name.as_str(), state.kind, state.mute, state.solo), ("Vocal", "audio", true, false));
        assert_eq!(state.output, "Stereo Output");
        assert_eq!(state.inputs, ["Input 1"]);
        assert_eq!(state.inserts, [("Channel EQ".to_string(), false, "effect")]);
        assert_eq!(state.sends, [("Bus 1".to_string(), Some(-6.0))]);
        // As Logic Pro 12.3.1 lists a lone send: its knob right after its group, the level written alone; several
        // sends' knobs come after all the groups, in their order.
        let logic = node(json!({"role": "AXLayoutItem", "desc": "Vocal", "c": [
            {"role": "AXTextField", "desc": "name", "value": "Vocal"},
            {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal."},
            {"role": "AXGroup", "desc": "Bus 2", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}, {"role": "AXButton", "desc": "list"}]},
            {"role": "AXSlider", "desc": "send knob", "value": 1330339456, "valueDesc": "-2.2"},
            {"role": "AXGroup", "desc": "Bus 1", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}, {"role": "AXButton", "desc": "list"}]},
            {"role": "AXSlider", "desc": "send knob", "value": 0, "valueDesc": "-∞"},
            {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."}
        ]}));
        let state = StripState::of(&logic, &Strings::english());
        assert_eq!(state.sends, [("Bus 1".to_string(), Some(MIN_DB)), ("Bus 2".to_string(), Some(-2.2))]);
        let several = node(json!({"role": "AXLayoutItem", "desc": "Vocal", "c": [
            {"role": "AXTextField", "desc": "name", "value": "Vocal"},
            {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal."},
            {"role": "AXGroup", "desc": "Bus 3", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}, {"role": "AXButton", "desc": "list"}]},
            {"role": "AXGroup", "desc": "Bus 2", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}, {"role": "AXButton", "desc": "list"}]},
            {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal."},
            {"role": "AXSlider", "desc": "send knob", "value": 0, "valueDesc": "-∞"},
            {"role": "AXSlider", "desc": "send knob", "value": 178427952, "valueDesc": "-37.1"},
            {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."}
        ]}));
        let state = StripState::of(&several, &Strings::english());
        assert_eq!(state.sends, [("Bus 2".to_string(), Some(-37.1)), ("Bus 3".to_string(), Some(MIN_DB))]);
        let instrument = node(json!({"role": "AXLayoutItem", "c": [
            {"role": "AXTextField", "desc": "name", "value": "Keys"},
            {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
            {"role": "AXButton", "desc": "insert bar"},
            {"role": "AXGroup", "desc": "ChromaVerb", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"role": "AXButton", "desc": "insert bar"},
            {"role": "AXGroup", "desc": "Compressor", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": true}]},
            {"role": "AXGroup", "desc": "Piano", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"role": "AXButton", "desc": "MIDI plug-in", "help": "MIDI Effect slot. Insert a MIDI effect."}
        ]}));
        let state = StripState::of(&instrument, &strings);
        assert_eq!((state.kind, state.inserts.len()), ("instrument", 3));
        assert_eq!(state.inserts[0], ("Piano".to_string(), false, "instrument"));
        assert_eq!(state.inserts[1], ("Compressor".to_string(), true, "effect"));
        assert_eq!(state.inserts[2], ("ChromaVerb".to_string(), false, "effect"));
        let output = node(
            json!({"role": "AXLayoutItem", "c": [{"role": "AXTextField", "desc": "name", "value": "Stereo Out"}, {"role": "AXButton", "subrole": "AXSwitch", "desc": "bounce", "value": "off"}]}),
        );
        assert_eq!(StripState::of(&output, &strings).kind, "output");
        let aux = node(
            json!({"role": "AXLayoutItem", "c": [{"role": "AXTextField", "desc": "name", "value": "Aux 1"}, {"role": "AXButton", "desc": "Bus 1", "help": "Input slot. Choose the input."}]}),
        );
        assert_eq!(StripState::of(&aux, &strings).kind, "aux");
        assert_eq!(plugin_row("1:plugin:1.1", "1:track:1", 1, &("Channel EQ".into(), false, "effect"))["kind"], "effect");
        assert_eq!(send_row("1:send:1.1", "1:track:1", 1, &("Bus 1".into(), Some(-6.0)))["level"], -6.0);
        let project = project_row(Some("abc"), "Scratch", Some("/m/Scratch.logicx"), 160.0, "4/4", "C Major", None, 2, 3);
        assert_eq!(project["sampleRate"], Value::Null);
        assert_eq!((project["trackCount"].as_u64(), project["middleC"].as_u64()), (Some(2), Some(3)));
        // "Display Middle C As": the preferences' switch on is C4 (Roland); off, or never set, C3 (Yamaha).
        assert_eq!((middle_c_of(Some("1")), middle_c_of(Some("0")), middle_c_of(None)), (4, 3, 3));
    }
}
